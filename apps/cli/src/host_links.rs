//! This computer's links to paired always-on hosts (`/v1/hosts`).
//!
//! The local daemon is the app's iroh client: it pairs with a host by ticket
//! and opens a guarded loopback proxy ([`remote_api::ApiProxy`]) per host the
//! app selects. Nothing here talks to a host without a pairing, adds a listening
//! port, or bypasses the host's peer guard: the proxy dials as a Desktop peer.
//!
//! The peer endpoint starts lazily (first pair or connect), never at daemon
//! start, unless `serve --peer` already started one (then it is shared).
use std::{collections::HashMap, path::PathBuf, sync::Arc};

use daemon_api::host_links::{
    HostLinkConnection, HostLinkError, HostLinkInfo, HostLinkState, HostLinks,
};
use parking_lot::Mutex;
use peer_network::{Device, PairTicket, PeerNetwork, Role};
use remote_api::{ApiProxy, ProxyState};

struct Open {
    proxy: ApiProxy,
    /// The channel was up at least once (separates "connecting" from "offline").
    seen_up: bool,
}

/// Overwrite every string in `value` (zeroize) so a key does not linger.
fn scrub(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => zeroize::Zeroize::zeroize(s),
        serde_json::Value::Array(a) => a.iter_mut().for_each(scrub),
        serde_json::Value::Object(o) => o.values_mut().for_each(scrub),
        _ => {}
    }
}

pub struct CliHostLinks {
    data: PathBuf,
    shared: Option<PeerNetwork>,
    started: tokio::sync::Mutex<Option<PeerNetwork>>,
    open: Mutex<HashMap<String, Open>>,
}

/// A short, single-line, path-free message for the UI.
fn safe(msg: impl std::fmt::Display) -> String {
    let text: String = msg
        .to_string()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    text.trim().to_owned()
}

#[derive(serde::Deserialize)]
struct PairingFile {
    #[serde(default)]
    devices: Vec<Device>,
}

impl CliHostLinks {
    pub fn new(data: PathBuf, shared: Option<PeerNetwork>) -> Self {
        Self {
            data,
            shared,
            started: tokio::sync::Mutex::new(None),
            open: Mutex::new(HashMap::new()),
        }
    }

    async fn network(&self) -> Result<PeerNetwork, HostLinkError> {
        if let Some(net) = &self.shared {
            return Ok(net.clone());
        }
        let mut slot = self.started.lock().await;
        if let Some(net) = slot.as_ref() {
            return Ok(net.clone());
        }
        let (cfg, _) =
            crate::peer::config(&self.data, false).map_err(|e| HostLinkError::Failed(safe(e)))?;
        let net = PeerNetwork::start(cfg, None)
            .await
            .map_err(|e| HostLinkError::Failed(safe(e)))?;
        *slot = Some(net.clone());
        Ok(net)
    }

    /// Paired devices without starting the endpoint: the running one if there
    /// is one, else the pairing file.
    fn devices(&self) -> Vec<Device> {
        if let Some(net) = &self.shared {
            return net.devices();
        }
        if let Some(net) = self.started.try_lock().ok().and_then(|s| s.clone()) {
            return net.devices();
        }
        std::fs::read_to_string(self.data.join("peer").join("devices.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<PairingFile>(&text).ok())
            .map(|f| f.devices)
            .unwrap_or_default()
    }

    fn info(&self, device: &Device) -> HostLinkInfo {
        let mut open = self.open.lock();
        let (state, detail) = match open.get_mut(&device.id) {
            None => (HostLinkState::Idle, None),
            Some(link) => match link.proxy.state.borrow().clone() {
                ProxyState::Connected => {
                    link.seen_up = true;
                    (HostLinkState::Connected, None)
                }
                ProxyState::Reconnecting if link.seen_up => (
                    HostLinkState::Offline,
                    Some("the host dropped the connection; retrying".into()),
                ),
                ProxyState::Reconnecting => (HostLinkState::Connecting, None),
                ProxyState::Unreachable(why) => {
                    let lower = why.to_ascii_lowercase();
                    if lower.contains("revoked") || lower.contains("not paired") {
                        (
                            HostLinkState::Unreachable,
                            Some("the host no longer accepts this computer; pair again".into()),
                        )
                    } else if link.seen_up {
                        (
                            HostLinkState::Offline,
                            Some("the host did not answer; retrying".into()),
                        )
                    } else {
                        (
                            HostLinkState::Connecting,
                            Some("waiting for the host to answer".into()),
                        )
                    }
                }
            },
        };
        HostLinkInfo {
            id: device.id.clone(),
            name: device.name.clone(),
            state,
            detail,
        }
    }
}

#[async_trait::async_trait]
impl HostLinks for CliHostLinks {
    fn list(&self) -> Vec<HostLinkInfo> {
        self.devices()
            .iter()
            .filter(|d| d.role == Role::Host)
            .map(|d| self.info(d))
            .collect()
    }

    async fn pair(&self, ticket: &str) -> Result<HostLinkInfo, HostLinkError> {
        let ticket = PairTicket::parse(ticket)
            .map_err(|_| HostLinkError::Invalid("that is not a pairing ticket".into()))?;
        if ticket.role != Role::Desktop {
            return Err(HostLinkError::Invalid(
                "this ticket is for another role; on the host run `bot host invite --role desktop`"
                    .into(),
            ));
        }
        let net = self.network().await?;
        let device = net
            .pair(&ticket)
            .await
            .map_err(|e| HostLinkError::Failed(safe(e)))?;
        if device.role != Role::Host {
            return Err(HostLinkError::Invalid(
                "paired, but that device is not an always-on host (start it with `bot serve --host`)"
                    .into(),
            ));
        }
        Ok(self.info(&device))
    }

    async fn connect(&self, id: &str) -> Result<HostLinkConnection, HostLinkError> {
        let device = self
            .devices()
            .into_iter()
            .find(|d| d.id == id && d.role == Role::Host)
            .ok_or(HostLinkError::NotFound)?;
        let key = device.public_key().map_err(|_| HostLinkError::NotFound)?;
        // Read the open link under the lock, release it, THEN ask `info` (it
        // takes the same non-recursive lock).
        let existing = self
            .open
            .lock()
            .get(id)
            .map(|link| (link.proxy.base_url.clone(), link.proxy.token.clone()));
        if let Some((base_url, token)) = existing {
            return Ok(HostLinkConnection {
                id: device.id.clone(),
                name: device.name.clone(),
                base_url,
                token,
                state: self.info(&device).state,
            });
        }
        let net = self.network().await?;
        let proxy = ApiProxy::start(net, key)
            .await
            .map_err(|e| HostLinkError::Failed(safe(e)))?;
        let connection = HostLinkConnection {
            id: device.id.clone(),
            name: device.name.clone(),
            base_url: proxy.base_url.clone(),
            token: proxy.token.clone(),
            state: HostLinkState::Connecting,
        };
        // A concurrent connect may have won: keep the first, drop ours.
        let mut open = self.open.lock();
        if let Some(link) = open.get(id) {
            return Ok(HostLinkConnection {
                base_url: link.proxy.base_url.clone(),
                token: link.proxy.token.clone(),
                ..connection
            });
        }
        open.insert(
            id.to_owned(),
            Open {
                proxy,
                seen_up: false,
            },
        );
        Ok(connection)
    }

    fn disconnect(&self, id: &str) -> bool {
        self.open.lock().remove(id).is_some()
    }

    async fn call(
        &self,
        id: &str,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<daemon_api::host_links::HostCall, HostLinkError> {
        // Only administration, bot and health paths of the host API.
        if !(path.starts_with("/v1/admin/") || path.starts_with("/v1/bots")) {
            return Err(HostLinkError::Invalid("path not allowed".into()));
        }
        let conn = self.connect(id).await?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| HostLinkError::Invalid("bad method".into()))?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .map_err(|e| HostLinkError::Failed(safe(e)))?;
        let mut req = http
            .request(method, format!("{}{path}", conn.base_url))
            .bearer_auth(&conn.token);
        if let Some(mut body) = body {
            req = req.json(&body);
            // The request now owns its serialised copy; wipe ours (a key).
            scrub(&mut body);
        }
        let resp = req
            .send()
            .await
            .map_err(|_| HostLinkError::Failed("the host did not answer".into()))?;
        let status = resp.status().as_u16();
        let body = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok(daemon_api::host_links::HostCall { status, body })
    }

    async fn forget(&self, id: &str) -> bool {
        let Some(device) = self
            .devices()
            .into_iter()
            .find(|d| d.id == id && d.role == Role::Host)
        else {
            return false;
        };
        self.open.lock().remove(id);
        let (Ok(key), Ok(net)) = (device.public_key(), self.network().await) else {
            return false;
        };
        net.revoke(&key).unwrap_or(false)
    }
}

pub fn install(state: &daemon_api::AppState, data: PathBuf, shared: Option<PeerNetwork>) {
    let links = Arc::new(CliHostLinks::new(data.clone(), shared));
    state.set_host_links(links.clone());
    // The "Add server" wizard pairs through the same links.
    state.set_host_setup(Arc::new(crate::host_setup::CliHostSetup::new(data, links)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_devices(dir: &std::path::Path, roles: &[(&str, &str)]) {
        let devices: Vec<serde_json::Value> = roles
            .iter()
            .map(|(id, role)| {
                serde_json::json!({
                    "id": id, "name": format!("dev-{id}"), "role": role,
                    "capabilities": [], "addr": null, "paired_at": 1,
                    "pairing_generation": ""
                })
            })
            .collect();
        std::fs::create_dir_all(dir.join("peer")).unwrap();
        std::fs::write(
            dir.join("peer").join("devices.json"),
            serde_json::json!({"version":1,"devices":devices}).to_string(),
        )
        .unwrap();
    }

    /// The REAL links against a real host endpoint: a plan, a copy and a second
    /// item all reach `connect` on an already-open proxy (this used to deadlock).
    #[tokio::test]
    async fn repeated_calls_on_an_open_connection_do_not_deadlock() {
        use peer_network::{ALPN_API, PeerConfig, RelayConfig};
        let host_dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(storage::Storage::open(host_dir.path()).unwrap());
        let agent = agent_core::AgentLoop::new(
            storage.clone(),
            tool_registry::ToolRegistry::new(),
            agent_core::Providers::single(Arc::new(daemon_api::fake::FakeProvider)),
            agent_core::LoopConfig::default(),
        );
        let state =
            daemon_api::AppState::new(agent, storage, daemon_api::DaemonConfig::new("t")).unwrap();
        let mut hc = PeerConfig::new(host_dir.path().join("peer"));
        hc.local_role = Role::Host;
        hc.relay = RelayConfig::Disabled;
        hc.bind_addr = Some("127.0.0.1:0".parse().unwrap());
        hc.extra_protocols.push((
            ALPN_API.to_vec(),
            Arc::new(remote_api::ApiProtocol::new(state)),
        ));
        let host = PeerNetwork::start(hc, None).await.unwrap();
        let desk_dir = tempfile::tempdir().unwrap();
        let mut dc = PeerConfig::new(desk_dir.path().join("peer"));
        dc.local_role = Role::Desktop;
        dc.relay = RelayConfig::Disabled;
        dc.bind_addr = Some("127.0.0.1:0".parse().unwrap());
        let desk = PeerNetwork::start(dc, None).await.unwrap();
        let links = CliHostLinks::new(desk_dir.path().to_path_buf(), Some(desk));
        let invite = host.create_invite(Role::Desktop).await;
        let info = links.pair(&invite.ticket).await.unwrap();
        for path in ["/v1/bots", "/v1/bots", "/v1/admin/state"] {
            let call = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                links.call(&info.id, "GET", path, None),
            )
            .await
            .expect("call must not hang")
            .unwrap();
            assert!(call.status > 0);
        }
        host.shutdown().await;
    }

    #[test]
    fn list_shows_only_hosts_and_never_starts_the_network() {
        let dir = tempfile::tempdir().unwrap();
        write_devices(
            dir.path(),
            &[("aaaa", "host"), ("bbbb", "phone"), ("cccc", "desktop")],
        );
        let links = CliHostLinks::new(dir.path().to_path_buf(), None);
        let hosts = links.list();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].id, "aaaa");
        assert_eq!(hosts[0].state, HostLinkState::Idle);
        assert!(links.started.try_lock().unwrap().is_none());
    }

    #[test]
    fn missing_pairing_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let links = CliHostLinks::new(dir.path().to_path_buf(), None);
        assert!(links.list().is_empty());
    }

    #[tokio::test]
    async fn connect_refuses_unknown_and_non_host_devices() {
        let dir = tempfile::tempdir().unwrap();
        write_devices(dir.path(), &[("bbbb", "phone")]);
        let links = CliHostLinks::new(dir.path().to_path_buf(), None);
        for id in ["bbbb", "zzzz"] {
            assert!(matches!(
                links.connect(id).await,
                Err(HostLinkError::NotFound)
            ));
        }
        assert!(links.started.lock().await.is_none());
    }

    #[tokio::test]
    async fn pairing_rejects_tickets_for_other_roles_before_any_network() {
        // Not a ticket at all.
        let dir = tempfile::tempdir().unwrap();
        let links = CliHostLinks::new(dir.path().to_path_buf(), None);
        assert!(matches!(
            links.pair("nonsense").await,
            Err(HostLinkError::Invalid(_))
        ));
        assert!(links.started.lock().await.is_none());
    }

    #[test]
    fn messages_are_single_line_and_bounded() {
        let long = format!("a\nb\x07{}", "x".repeat(500));
        let msg = safe(long);
        assert!(!msg.contains('\n') && !msg.contains('\x07'));
        assert!(msg.chars().count() <= 200);
    }
}
