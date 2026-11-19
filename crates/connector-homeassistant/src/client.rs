//! Home Assistant REST client (`/api/states`, `/api/services`).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use secret_store::{SecretRef, SecretStore, resolve};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::{Host, Url};

/// Largest response body read from the hub.
const MAX_BODY: usize = 4 << 20;

/// Services that run arbitrary code or stop the hub: never callable by a bot.
const DENIED: &[(&str, &str)] = &[
    ("shell_command", "*"),
    ("python_script", "*"),
    ("pyscript", "*"),
    ("hassio", "*"),
    ("rest_command", "*"),
    ("homeassistant", "restart"),
    ("homeassistant", "stop"),
    ("homeassistant", "reload_all"),
    ("recorder", "purge"),
    ("recorder", "purge_entities"),
];

/// How to reach the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeAssistantConfig {
    /// Base URL, e.g. `http://homeassistant.local:8123/`.
    pub url: String,
    /// Secret-store reference of the long-lived access token.
    pub token: SecretRef,
    /// When set, only these domains (`light`, `switch` ...) may be called.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Per-request timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    15
}

/// Why a call failed. Never contains the token.
#[derive(Debug, thiserror::Error)]
pub enum HaError {
    /// Bad configuration or arguments.
    #[error("{0}")]
    Invalid(String),
    /// The secret could not be read.
    #[error("cannot read the Home Assistant token from the secret store: {0}")]
    Credential(String),
    /// Network or HTTP failure.
    #[error("home assistant: {0}")]
    Server(String),
}

/// A configured hub.
pub struct HomeAssistantClient {
    base: Url,
    token: SecretRef,
    allowed: Vec<String>,
    store: Arc<dyn SecretStore>,
    http: reqwest::Client,
}

fn is_private_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => {
            IpAddr::V4(ip).is_loopback() || ip.is_private() || ip.is_link_local()
        }
        Some(Host::Ipv6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Some(Host::Domain(d)) => {
            d == "localhost" || d.rsplit('.').next() == Some("local") || !d.contains('.')
        }
        None => false,
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl HomeAssistantClient {
    /// # Errors
    /// [`HaError::Invalid`] for a bad URL or plain HTTP to a public host.
    pub fn new(cfg: HomeAssistantConfig, store: Arc<dyn SecretStore>) -> Result<Self, HaError> {
        let mut base =
            Url::parse(&cfg.url).map_err(|e| HaError::Invalid(format!("bad url: {e}")))?;
        match base.scheme() {
            "https" => {}
            "http" if is_private_host(&base) => {}
            _ => {
                return Err(HaError::Invalid(
                    "the url must be https (plain http only for a home-network or localhost address)"
                        .into(),
                ));
            }
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(HaError::Invalid("do not put credentials in the url".into()));
        }
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(cfg.timeout_secs.clamp(1, 60)))
            .build()
            .map_err(|e| HaError::Server(e.to_string()))?;
        Ok(Self {
            base,
            token: cfg.token,
            allowed: cfg.allowed_domains,
            store,
            http,
        })
    }

    async fn token(&self) -> Result<String, HaError> {
        let (store, r) = (Arc::clone(&self.store), self.token.clone());
        tokio::task::spawn_blocking(move || {
            resolve(store.as_ref(), &r).map(|v| v.expose().to_string())
        })
        .await
        .map_err(|e| HaError::Credential(e.to_string()))?
        .map_err(|e| HaError::Credential(e.to_string()))
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, HaError> {
        let url = self
            .base
            .join(path)
            .map_err(|e| HaError::Invalid(e.to_string()))?;
        let mut req = self
            .http
            .request(method, url)
            .bearer_auth(self.token().await?);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| HaError::Server(scrub(&e.to_string())))?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(HaError::Server(format!(
                "the hub answered with a redirect ({status}); use its final address"
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HaError::Server(scrub(&e.to_string())))?;
        if bytes.len() > MAX_BODY {
            return Err(HaError::Server("response too large".into()));
        }
        if !status.is_success() {
            return Err(HaError::Server(match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    format!("the hub refused the access token ({status})")
                }
                StatusCode::NOT_FOUND => "not found (unknown entity or service)".to_owned(),
                _ => format!("HTTP {status}"),
            }));
        }
        serde_json::from_slice(&bytes).map_err(|e| HaError::Server(format!("bad JSON: {e}")))
    }

    /// Entities, optionally of one domain, as `{entity_id, state, name}`.
    ///
    /// # Errors
    /// See [`HaError`].
    pub async fn list(&self, domain: Option<&str>, limit: usize) -> Result<Vec<Value>, HaError> {
        if let Some(d) = domain
            && !valid_name(d)
        {
            return Err(HaError::Invalid(format!("`{d}` is not a domain name")));
        }
        let all = self
            .request(reqwest::Method::GET, "api/states", None)
            .await?;
        let prefix = domain.map(|d| format!("{d}."));
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| {
                let id = s["entity_id"].as_str().unwrap_or_default();
                prefix.as_deref().is_none_or(|p| id.starts_with(p))
            })
            .take(limit)
            .map(|s| {
                serde_json::json!({
                    "entity_id": s["entity_id"],
                    "state": s["state"],
                    "name": s["attributes"]["friendly_name"],
                })
            })
            .collect())
    }

    /// One entity with its attributes.
    ///
    /// # Errors
    /// See [`HaError`].
    pub async fn state(&self, entity_id: &str) -> Result<Value, HaError> {
        let ok = entity_id
            .split_once('.')
            .is_some_and(|(d, o)| valid_name(d) && valid_name(o));
        if !ok {
            return Err(HaError::Invalid(format!(
                "`{entity_id}` is not an entity id (domain.object)"
            )));
        }
        self.request(
            reqwest::Method::GET,
            &format!("api/states/{entity_id}"),
            None,
        )
        .await
    }

    /// Call `domain.service` with optional target and data; returns the
    /// entities whose state changed.
    ///
    /// # Errors
    /// [`HaError::Invalid`] for a denied or not allowed service.
    pub async fn call(
        &self,
        domain: &str,
        service: &str,
        entity_id: Option<&str>,
        data: Option<&Value>,
    ) -> Result<Value, HaError> {
        if !valid_name(domain) || !valid_name(service) {
            return Err(HaError::Invalid(
                "domain and service are lowercase names".into(),
            ));
        }
        if DENIED
            .iter()
            .any(|(d, s)| *d == domain && (*s == "*" || *s == service))
        {
            return Err(HaError::Invalid(format!(
                "`{domain}.{service}` is blocked: it can run code or stop the hub"
            )));
        }
        if !self.allowed.is_empty() && !self.allowed.iter().any(|d| d == domain) {
            return Err(HaError::Invalid(format!(
                "domain `{domain}` is not in `allowed_domains`"
            )));
        }
        let mut body = match data {
            Some(Value::Object(m)) => Value::Object(m.clone()),
            Some(_) => return Err(HaError::Invalid("`data` must be an object".into())),
            None => serde_json::json!({}),
        };
        if let Some(e) = entity_id {
            body["entity_id"] = Value::String(e.to_owned());
        }
        self.request(
            reqwest::Method::POST,
            &format!("api/services/{domain}/{service}"),
            Some(&body),
        )
        .await
    }
}

/// Drop anything that looks like a credential-bearing URL from a transport error.
fn scrub(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            if w.contains("://") && w.contains('@') {
                "<url>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
