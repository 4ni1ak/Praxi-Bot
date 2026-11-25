//! Cheap, text-free reachability checks against configured model services.
//! An HTTP error still proves connectivity; keys, quota and outages are not offline.

use std::sync::Arc;
use std::time::Duration;

use futures::{StreamExt, stream};
use tokio::sync::Mutex;

const FRESH_FOR: Duration = Duration::from_secs(15);
const DEADLINE: Duration = Duration::from_secs(3);
type CachedObservation = Option<(tokio::time::Instant, Vec<Observation>)>;

/// A configured service, with no key or user content.
#[derive(Clone, Debug)]
pub struct Target {
    /// Provider identifier.
    pub id: String,
    /// Configured HTTP origin or models endpoint. Never returned to clients.
    pub url: String,
}

/// Last bounded observation of one service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Provider identifier.
    pub id: String,
    /// Only literal loopback addresses qualify as a local model.
    pub local: bool,
    /// Whether an HTTP response arrived (including 401, 429 or 503).
    pub reachable: bool,
}

/// Clones share a short cache and one in-flight probe, including on headless hosts.
#[derive(Clone, Default)]
pub struct Connectivity {
    targets: Arc<Vec<Target>>,
    cache: Arc<Mutex<CachedObservation>>,
}

/// Only loopback URLs qualify; provider names such as `local` are not evidence.
#[must_use]
pub fn is_local(url: &str) -> bool {
    reqwest::Url::parse(url).ok().is_some_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
    })
}

impl Connectivity {
    /// Endpoints come only from enabled, loaded provider configuration.
    #[must_use]
    pub fn new(targets: Vec<Target>) -> Self {
        let targets = targets
            .into_iter()
            .filter_map(|mut target| {
                let mut url = reqwest::Url::parse(&target.url).ok()?;
                if !matches!(url.scheme(), "http" | "https") {
                    return None;
                }
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                target.url = url.into();
                Some(target)
            })
            .collect();
        Self {
            targets: Arc::new(targets),
            cache: Arc::default(),
        }
    }

    /// A local model must actually be configured at a loopback endpoint.
    #[must_use]
    pub fn local_provider(&self, id: &str) -> bool {
        self.targets.iter().any(|t| t.id == id && is_local(&t.url))
    }

    /// Probe with no retries, redirects, credentials, inference or response body reads.
    /// The normal inference path retains the provider reliability layer's backoff.
    pub async fn check(&self) -> Vec<Observation> {
        let mut cache = self.cache.lock().await;
        if let Some((at, rows)) = &*cache {
            if at.elapsed() < FRESH_FOR {
                return rows.clone();
            }
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(DEADLINE)
            .connect_timeout(DEADLINE)
            .build();
        let rows: Vec<_> = stream::iter(self.targets.iter())
            .map(|target| {
                let client = client.as_ref().ok();
                async move {
                    let reachable = match client {
                        Some(c) => c.head(&target.url).send().await.is_ok(),
                        None => false,
                    };
                    Observation {
                        id: target.id.clone(),
                        local: is_local(&target.url),
                        reachable,
                    }
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;
        *cache = Some((tokio::time::Instant::now(), rows.clone()));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[tokio::test]
    async fn fake_network_distinguishes_rejection_from_disconnection_and_caches() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let monitor = Connectivity::new(vec![
            Target {
                id: "rejected".into(),
                url: server.uri(),
            },
            Target {
                id: "disconnected".into(),
                url: format!("http://{address}"),
            },
        ]);
        let rows = monitor.check().await;
        assert!(
            rows.iter()
                .any(|r| r.id == "rejected" && r.reachable && r.local)
        );
        assert!(rows.iter().any(|r| r.id == "disconnected" && !r.reachable));
        assert_eq!(rows, monitor.clone().check().await);
        assert!(!is_local("https://localhost.example.com"));
        assert!(!is_local("http://192.168.1.2"));
        assert!(is_local("http://[::1]:8080"));
    }
}
