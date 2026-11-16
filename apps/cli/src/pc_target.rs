//! "My computer is a target for my bots" (card 258-A): this computer's daemon
//! serves the paired host's desktop requests, under the owner's consent.
//!
//! Authority model, all enforced here before the shared [`DesktopRpcHandler`]
//! runs anything:
//! 1. Only the paired device with role `host` may call; phones, nodes and
//!    other desktops are refused, and so is everything once unpaired.
//! 2. Off by default; the local owner switches it on per host
//!    (`PUT /v1/hosts/{id}/computer-access`, closed to every paired role).
//! 3. Reads (snapshot, screenshot, windows) run while the switch is on and the
//!    human is not driving; everything else (`desktop.act`, `desktop.launch`)
//!    waits for the person at this computer in the desktop app. Silence denies.
//! 4. The pause / human-driving latch is consulted before and after the ask.
//! 5. Every decision is written to the local audit log.
//!
//! Nothing new listens: requests arrive over the existing paired iroh link.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::ControlGate;
use async_trait::async_trait;
use daemon_api::pc_access::PcRequests;
use parking_lot::Mutex;
use peer_network::proto::{ERR_FORBIDDEN, ERR_METHOD_NOT_FOUND, RpcError};
use peer_network::{DesktopRpcHandler, Device, Role, RpcHandler};
use serde_json::Value;

/// How long the owner has to answer a request before it is denied.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(120);

/// Hosts whose bots may use this computer. Persisted next to the pairing file.
pub struct AccessStore {
    path: PathBuf,
    enabled: Mutex<BTreeSet<String>>,
}

impl AccessStore {
    #[must_use]
    pub fn open(data: &std::path::Path) -> Self {
        let path = data.join("peer").join("computer-access.json");
        let enabled = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<BTreeSet<String>>(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            enabled: Mutex::new(enabled),
        }
    }

    #[must_use]
    pub fn enabled(&self, host_id: &str) -> bool {
        self.enabled.lock().contains(host_id)
    }

    #[must_use]
    pub fn enabled_ids(&self) -> Vec<String> {
        self.enabled.lock().iter().cloned().collect()
    }

    /// # Errors
    /// The file could not be written; the in-memory switch is left unchanged.
    pub fn set(&self, host_id: &str, on: bool) -> std::io::Result<()> {
        let mut set = self.enabled.lock();
        let mut next = set.clone();
        if on {
            next.insert(host_id.to_owned());
        } else {
            next.remove(host_id);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_vec(&next)?)?;
        *set = next;
        Ok(())
    }
}

type Audit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// The shared desktop RPC surface behind the owner's consent.
pub struct HostDesktopHandler {
    inner: DesktopRpcHandler,
    access: Arc<AccessStore>,
    requests: Arc<PcRequests>,
    gate: Arc<dyn ControlGate>,
    audit: Audit,
    ask_timeout: Duration,
}

impl HostDesktopHandler {
    #[must_use]
    pub fn new(
        inner: DesktopRpcHandler,
        access: Arc<AccessStore>,
        requests: Arc<PcRequests>,
        gate: Arc<dyn ControlGate>,
        audit: Audit,
    ) -> Self {
        Self {
            inner,
            access,
            requests,
            gate,
            audit,
            ask_timeout: ASK_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_ask_timeout(mut self, t: Duration) -> Self {
        self.ask_timeout = t;
        self
    }

    fn refuse(&self, peer: &Device, method: &str, why: &str) -> RpcError {
        (self.audit)(
            "pc_target.refused",
            serde_json::json!({"host": peer.id, "method": method, "why": why}),
        );
        RpcError::new(ERR_FORBIDDEN, why)
    }
}

const READS: [&str; 4] = [
    "desktop.snapshot",
    "desktop.screenshot",
    "desktop.windows",
    "desktop.capabilities",
];

/// Short description of a mutating call for the approval card.
fn describe(method: &str, params: &Value) -> (String, String) {
    let action = params
        .pointer("/action/kind")
        .or_else(|| params.pointer("/action/type"))
        .and_then(Value::as_str)
        .map_or_else(
            || method.trim_start_matches("desktop.").to_owned(),
            str::to_owned,
        );
    let detail = params
        .get("action")
        .or_else(|| params.get("app"))
        .map(Value::to_string)
        .unwrap_or_default();
    (action, detail)
}

#[async_trait]
impl RpcHandler for HostDesktopHandler {
    async fn handle(&self, peer: &Device, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "node.ping" {
            return self.inner.handle(peer, method, params).await;
        }
        if peer.role != Role::Host {
            return Err(self.refuse(
                peer,
                method,
                "only the server this computer is paired with may use it",
            ));
        }
        if !self.access.enabled(&peer.id) {
            return Err(self.refuse(
                peer,
                method,
                "the owner has not allowed bots to use this computer",
            ));
        }
        if method == "node.info" {
            return self.inner.handle(peer, method, params).await;
        }
        if !method.starts_with("desktop.") {
            return Err(RpcError::new(
                ERR_METHOD_NOT_FOUND,
                format!("unknown method '{method}'"),
            ));
        }
        if READS.contains(&method) {
            if let Some(msg) = self.gate.observation_refused() {
                return Err(self.refuse(peer, method, &msg));
            }
            (self.audit)(
                "pc_target.read",
                serde_json::json!({"host": peer.id, "method": method}),
            );
            return self.inner.handle(peer, method, params).await;
        }
        if let Some(msg) = self.gate.latch_refusal() {
            return Err(self.refuse(peer, method, &msg));
        }
        let (action, detail) = describe(method, &params);
        let allowed = self
            .requests
            .ask(&peer.id, &peer.name, &action, &detail, self.ask_timeout)
            .await;
        if !allowed {
            return Err(self.refuse(
                peer,
                method,
                "the person at this computer did not approve this action",
            ));
        }
        // The human may have taken the wheel while the card was open.
        if let Some(msg) = self.gate.latch_refusal() {
            return Err(self.refuse(peer, method, &msg));
        }
        (self.audit)(
            "pc_target.act",
            serde_json::json!({"host": peer.id, "method": method, "action": action}),
        );
        self.inner.handle(peer, method, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_api::{
        Action, ActionResult, Capabilities, DesktopBackend, DesktopError, Image, Quality, Region,
        Snapshot, WindowInfo,
        snapshot::{AppSpec, Pid},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counting {
        acts: AtomicUsize,
        reads: AtomicUsize,
    }

    #[async_trait]
    impl DesktopBackend for Counting {
        async fn snapshot(&self, _: Option<u64>) -> Result<Snapshot, DesktopError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Err(DesktopError::Unsupported("test".into()))
        }
        async fn act(&self, _: Action) -> Result<ActionResult, DesktopError> {
            self.acts.fetch_add(1, Ordering::SeqCst);
            Err(DesktopError::Unsupported("test".into()))
        }
        async fn screenshot(&self, _: Region, _: Quality) -> Result<Image, DesktopError> {
            Err(DesktopError::Unsupported("test".into()))
        }
        async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
            Ok(vec![])
        }
        async fn launch(&self, _: AppSpec) -> Result<Pid, DesktopError> {
            Err(DesktopError::Unsupported("test".into()))
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                platform: "test".into(),
                accessibility_tree: false,
                screenshot: false,
                app_launch: false,
                has_display: true,
            }
        }
    }

    struct NoLatch;
    #[async_trait]
    impl ControlGate for NoLatch {
        async fn before_action(
            &self,
            _: agent_core::ControlRequest,
            _: &(dyn Fn() + Send + Sync),
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn device(id: &str, role: Role) -> Device {
        Device {
            id: id.into(),
            name: "Server".into(),
            role,
            capabilities: vec![],
            addr: None,
            paired_at: 1,
            pairing_generation: String::new(),
        }
    }

    fn setup(
        dir: &std::path::Path,
    ) -> (
        HostDesktopHandler,
        Arc<Counting>,
        Arc<AccessStore>,
        Arc<PcRequests>,
    ) {
        let backend = Arc::new(Counting::default());
        let access = Arc::new(AccessStore::open(dir));
        let requests = Arc::new(PcRequests::new());
        let handler = HostDesktopHandler::new(
            DesktopRpcHandler::new(backend.clone()),
            access.clone(),
            requests.clone(),
            Arc::new(NoLatch),
            Arc::new(|_, _| {}),
        )
        .with_ask_timeout(Duration::from_secs(5));
        (handler, backend, access, requests)
    }

    const ACT: &str = "desktop.act";

    fn act_params() -> Value {
        serde_json::json!({ "action": Action::TypeText { text: "hi".into() } })
    }

    /// Off by default, refused for any other role even when on; reads need the
    /// switch; an input action never runs without the owner's approval.
    #[tokio::test]
    async fn host_desktop_calls_need_the_switch_the_right_peer_and_an_approval() {
        let dir = tempfile::tempdir().unwrap();
        let (h, backend, access, requests) = setup(dir.path());
        let host = device("host1", Role::Host);

        // Switch off: even a read from the paired host is refused.
        let off = h.handle(&host, "desktop.snapshot", Value::Null).await;
        assert_eq!(off.unwrap_err().code, ERR_FORBIDDEN);
        assert_eq!(backend.reads.load(Ordering::SeqCst), 0);

        access.set("host1", true).unwrap();
        // Another role / another desktop / a phone is refused with the switch on.
        for role in [Role::Desktop, Role::Phone, Role::Node] {
            let other = device("host1", role);
            let r = h.handle(&other, ACT, act_params()).await;
            assert_eq!(r.unwrap_err().code, ERR_FORBIDDEN, "{role:?}");
        }
        assert_eq!(backend.acts.load(Ordering::SeqCst), 0);

        // A read runs without a card.
        let _ = h.handle(&host, "desktop.snapshot", Value::Null).await;
        assert_eq!(backend.reads.load(Ordering::SeqCst), 1);

        // An input action waits for the owner; denied -> never runs.
        let denied = {
            let call = h.handle(&host, ACT, act_params());
            let answer = async {
                let id = loop {
                    if let Some(r) = requests.list().first() {
                        break r.id.clone();
                    }
                    tokio::task::yield_now().await;
                };
                assert!(requests.decide(&id, false));
            };
            tokio::join!(call, answer).0
        };
        assert_eq!(denied.unwrap_err().code, ERR_FORBIDDEN);
        assert_eq!(backend.acts.load(Ordering::SeqCst), 0);

        // Approved -> runs exactly once.
        let call = h.handle(&host, ACT, act_params());
        let answer = async {
            let id = loop {
                if let Some(r) = requests.list().first() {
                    break r.id.clone();
                }
                tokio::task::yield_now().await;
            };
            assert!(requests.decide(&id, true));
        };
        let _ = tokio::join!(call, answer);
        assert_eq!(backend.acts.load(Ordering::SeqCst), 1);
    }
}
