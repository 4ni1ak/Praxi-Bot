//! The decider role of the loop: settings and construction.
//!
//! A [`TurnDecider`] is a [`Decider`] (a chain of cheap decision models with
//! typed, validated answers; see `agent_router::decider`) plus the settings
//! that say what the loop may use it for. This module only *configures and
//! builds* it:
//!
//! * [`DeciderSettings`]: `<data dir>/decider.toml`. Provider and model id of
//!   every backend are plain configuration; nothing in the code depends on a
//!   specific model id, and the default chain is only a starting point.
//! * [`build_decider`]: resolves each backend's key from the provider it
//!   names (`providers.toml` / secret store / the catalog's environment
//!   variable) and skips what is unusable. Disabled, or no usable backend,
//!   yields `None`, and the loop behaves exactly as it does without one.
//!
//! The uses (tool pre-selection, trivial-turn routing, history trimming) are
//! wired in a follow-up; the switches for them already live in the settings.
//!
//! # Security
//! A decider answer is advisory: a number in `0..=1` or one of the option ids
//! the caller supplied. This module has no dependency on the permission
//! engine and nothing here can allow, approve or run a tool.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_router::decider::{
    ChatJsonBackend, Decider, DeciderConfig, DeciderMetrics, DecisionBackend, GeminiDecisions,
    OpenRouterDecisions, RateLimits,
};
use provider_catalog::{Auth, Catalog, ProviderConfig, config_path};
use secret_store::SecretStore;
use serde::{Deserialize, Serialize};

use crate::router::ProviderClassifier;
use crate::shared::Providers;

/// File name of the decider settings inside the data directory.
pub const DECIDER_FILE: &str = "decider.toml";

/// Which HTTP API a decider backend speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeciderApi {
    /// Google's Decisions API (`/v1beta/decisions`).
    GeminiDecisions,
    /// `OpenRouter`'s Decisions API (`/api/alpha/decisions`).
    OpenrouterDecisions,
    /// The provider's normal chat endpoint with a strict JSON answer.
    ChatJson,
}

/// One backend of the decider's fallback chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderBackendSettings {
    /// Provider id as in `providers.toml` / the catalog (`gemini`,
    /// `openrouter`, ...). Its API key is the provider's key.
    pub provider: String,
    /// Model id at that provider.
    pub model: String,
    /// API to use; `None` = by provider (`gemini` -> Decisions API,
    /// `openrouter` -> Decisions API, anything else -> chat JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<DeciderApi>,
    /// Endpoint override for the Decisions APIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Requests per minute (`0` = unlimited).
    #[serde(default)]
    pub rpm: u32,
    /// Input tokens per minute (`0` = unlimited).
    #[serde(default)]
    pub tpm: u64,
    /// Requests per day (`0` = unlimited).
    #[serde(default)]
    pub rpd: u32,
}

impl DeciderBackendSettings {
    /// The API this backend speaks.
    #[must_use]
    pub fn effective_api(&self) -> DeciderApi {
        self.api.unwrap_or(match self.provider.as_str() {
            "gemini" | "google" => DeciderApi::GeminiDecisions,
            "openrouter" => DeciderApi::OpenrouterDecisions,
            _ => DeciderApi::ChatJson,
        })
    }
}

/// Settings of the decider role (`<data dir>/decider.toml`, `/v1/decider`).
///
/// Model ids of free decision models change often: nothing in the code
/// depends on a specific id, the defaults below are only the starting chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // independent feature switches of a settings file
pub struct DeciderSettings {
    /// Master switch (default off: it needs a provider key).
    pub enabled: bool,
    /// Send the main model only the tool families the request needs.
    pub tool_preselect: bool,
    /// Give a message that needs no tools the meta-tools only (and
    /// `light_model`, when set).
    pub trivial_route: bool,
    /// Send a short recent history when the message stands alone.
    pub history_trim: bool,
    /// `provider/model` that answers the first model call of a trivial turn;
    /// the bot's executor (and its fallbacks) remain the fallback.
    pub light_model: Option<String>,
    /// Wall-time cap of one decision in milliseconds (whole chain).
    pub timeout_ms: u64,
    /// Decider calls one turn may make.
    pub max_calls_per_turn: u32,
    /// Lifetime of a cached decision in seconds (`0` = no cache).
    pub cache_ttl_secs: u64,
    /// A turn is trivial when P(needs tools) is at most this.
    pub trivial_max: f64,
    /// A tool family is kept when P(useful) is at least this.
    pub family_min: f64,
    /// The history is trimmed when P(depends on earlier turns) is at most this.
    pub standalone_max: f64,
    /// The fallback chain, tried in order.
    pub backends: Vec<DeciderBackendSettings>,
}

impl Default for DeciderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tool_preselect: true,
            trivial_route: true,
            history_trim: true,
            light_model: None,
            timeout_ms: 1500,
            max_calls_per_turn: 2,
            cache_ttl_secs: 600,
            trivial_max: 0.10,
            family_min: 0.25,
            standalone_max: 0.10,
            backends: vec![DeciderBackendSettings {
                provider: "gemini".into(),
                model: "gemini-decision-1".into(),
                api: None,
                base_url: None,
                rpm: 200,
                tpm: 50_000,
                rpd: 10_000,
            }],
        }
    }
}

/// Most backends a chain may have.
pub const MAX_BACKENDS: usize = 8;

impl DeciderSettings {
    /// Path of the settings file under `data_dir`.
    #[must_use]
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(DECIDER_FILE)
    }

    /// Load the settings; a missing file is the defaults.
    ///
    /// # Errors
    /// An unreadable, malformed or invalid file.
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path(data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{DECIDER_FILE}: {e}")),
        };
        let s: Self = toml::from_str(&text).map_err(|e| format!("{DECIDER_FILE}: {e}"))?;
        s.validate()?;
        Ok(s)
    }

    /// Write the settings (atomically: temp file, then rename).
    ///
    /// # Errors
    /// Invalid settings or an I/O failure.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{DECIDER_FILE}: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{DECIDER_FILE}: {e}"))
    }

    /// Check ranges and shapes.
    ///
    /// # Errors
    /// The first problem found.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |name: &str, v: f64| {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("`{name}` must be between 0 and 1"))
            }
        };
        unit("trivial_max", self.trivial_max)?;
        unit("family_min", self.family_min)?;
        unit("standalone_max", self.standalone_max)?;
        if !(100..=30_000).contains(&self.timeout_ms) {
            return Err("`timeout_ms` must be between 100 and 30000".into());
        }
        if self.max_calls_per_turn > 16 {
            return Err("`max_calls_per_turn` must be at most 16".into());
        }
        if self.backends.len() > MAX_BACKENDS {
            return Err(format!("at most {MAX_BACKENDS} backends"));
        }
        let plain = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control() || c == ' ')
        };
        for b in &self.backends {
            if !plain(&b.provider, 64) || b.provider.contains('/') {
                return Err(format!("invalid backend provider {:?}", b.provider));
            }
            if !plain(&b.model, 200) {
                return Err(format!("invalid backend model {:?}", b.model));
            }
            if let Some(u) = &b.base_url
                && !(u.starts_with("https://") || u.starts_with("http://"))
            {
                return Err("backend `base_url` must be an http(s) URL".into());
            }
        }
        if let Some(m) = &self.light_model
            && (!plain(m, 200) || m.starts_with("cli:"))
        {
            return Err("`light_model` must be an HTTP `provider/model`".into());
        }
        Ok(())
    }
}

/// The decider installed in a loop: the chain plus the switches.
pub struct TurnDecider {
    decider: Arc<Decider>,
    settings: DeciderSettings,
}

impl std::fmt::Debug for TurnDecider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnDecider")
            .field("decider", &self.decider)
            .finish_non_exhaustive()
    }
}

impl TurnDecider {
    /// Wrap a ready [`Decider`] (tests, embedders with their own backends).
    #[must_use]
    pub fn new(decider: Arc<Decider>, settings: DeciderSettings) -> Arc<Self> {
        Arc::new(Self { decider, settings })
    }

    /// The settings it was built from.
    #[must_use]
    pub fn settings(&self) -> &DeciderSettings {
        &self.settings
    }

    /// Ids of the usable backends, in chain order.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<String> {
        self.decider.backend_ids()
    }

    /// Counters of the decision chain.
    #[must_use]
    pub fn metrics(&self) -> DeciderMetrics {
        self.decider.metrics()
    }

    /// The underlying chain (live probes).
    #[must_use]
    pub fn decider(&self) -> &Arc<Decider> {
        &self.decider
    }
}

/// The API key of provider `id`: its `providers.toml` entry (stored secret or
/// the catalog's environment variable), or just the catalog's environment
/// variable when the provider is not configured.
fn provider_key(
    cfg: &ProviderConfig,
    catalog: &Catalog,
    store: &dyn SecretStore,
    id: &str,
) -> Option<String> {
    let settings = cfg.providers.iter().find(|p| p.enabled && p.id == id);
    let entry = settings
        .and_then(|s| s.custom.as_ref())
        .or_else(|| catalog.get(id));
    let key = match (
        settings.and_then(|s| s.key.as_ref()),
        entry.map(|e| &e.auth),
    ) {
        (Some(r), _) => secret_store::resolve(store, r)
            .ok()
            .map(|k| k.expose().to_string()),
        (None, Some(Auth::ApiKeyEnv(var))) => std::env::var(var).ok(),
        _ => None,
    };
    key.filter(|k| !k.trim().is_empty())
}

/// Build the decider described by `settings`: every backend that has a key
/// (Decisions APIs) or a configured chat provider. `None` when it is disabled
/// or no backend is usable; skipped backends are reported through `warn`.
/// Keys are registered with the loop's redactor by the usual secret refresh;
/// they never appear in a warning.
pub fn build_decider(
    settings: &DeciderSettings,
    data_dir: &Path,
    catalog: &Catalog,
    store: &dyn SecretStore,
    providers: &Providers,
    warn: &mut dyn FnMut(&str),
) -> Option<Arc<TurnDecider>> {
    if !settings.enabled {
        return None;
    }
    let cfg = ProviderConfig::load(&config_path(data_dir)).unwrap_or_default();
    let mut chain: Vec<(Arc<dyn DecisionBackend>, RateLimits)> = Vec::new();
    for b in &settings.backends {
        let id = format!("{}/{}", b.provider, b.model);
        let limits = RateLimits {
            rpm: b.rpm,
            tpm: b.tpm,
            rpd: b.rpd,
        };
        let api = b.effective_api();
        let backend: Arc<dyn DecisionBackend> = match api {
            DeciderApi::ChatJson => {
                if providers.get(&b.provider).is_none() {
                    warn(&format!(
                        "decider backend {id} skipped: provider is not configured"
                    ));
                    continue;
                }
                Arc::new(ChatJsonBackend::new(
                    id.clone(),
                    Arc::new(ProviderClassifier {
                        providers: providers.clone(),
                        model: id.clone(),
                    }),
                ))
            }
            DeciderApi::GeminiDecisions | DeciderApi::OpenrouterDecisions => {
                let Some(key) = provider_key(&cfg, catalog, store, &b.provider) else {
                    warn(&format!("decider backend {id} skipped: no API key"));
                    continue;
                };
                if api == DeciderApi::GeminiDecisions {
                    let mut g = GeminiDecisions::new(key, b.model.clone()).with_id(id.clone());
                    if let Some(u) = &b.base_url {
                        g = g.with_base_url(u.clone());
                    }
                    Arc::new(g)
                } else {
                    let mut o = OpenRouterDecisions::new(key, b.model.clone()).with_id(id.clone());
                    if let Some(u) = &b.base_url {
                        o = o.with_base_url(u.clone());
                    }
                    Arc::new(o)
                }
            }
        };
        chain.push((backend, limits));
    }
    if chain.is_empty() {
        warn("decider is enabled but no backend is usable; turns run without it");
        return None;
    }
    let decider = Decider::new(
        chain,
        DeciderConfig {
            timeout: Duration::from_millis(settings.timeout_ms),
            cache_ttl: Duration::from_secs(settings.cache_ttl_secs),
            ..DeciderConfig::default()
        },
    );
    Some(TurnDecider::new(Arc::new(decider), settings.clone()))
}
