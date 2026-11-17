//! The server as the tool target of its paired ADMIN desktop (card 272, gap 2).
//!
//! The bot's brain (model providers, keys, subscription CLIs, approvals) stays
//! on the owner's computer; this host only executes what that one desktop asks
//! for, with the same tool code it runs locally, and audits every call.
//!
//! Authority, all checked here before anything runs:
//! 1. Only the paired device with role `desktop` AND the `admin` flag, read from
//!    the pairing file again for every request (revoking, re-pairing or removing
//!    the admin flag stops the next call). A phone, a node, a host, another
//!    desktop without the flag and an unknown key are refused and the refusal is
//!    audited.
//! 2. Only the browser family is served in this version, and only what the PC
//!    side serves too: web pages (`http`/`https`), no page scripts, no stored
//!    secrets, no profile choice. No shell, no files, no desktop.
//! 3. Approvals are not asked here: the owner is at the brain's computer and
//!    answers there. Every call, allowed or refused, is written to the audit
//!    log; a call whose audit row cannot be written does not run.
//!
//! Nothing new listens: requests arrive over the existing paired iroh link.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use builtin_tools::BrowserDriver;
use builtin_tools::pc::{PcInfo, RPC_BROWSER, RPC_INFO};
use peer_network::proto::{ERR_FORBIDDEN, ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND, RpcError};
use peer_network::{Device, Role, RpcHandler};
use serde_json::Value;

use crate::pc_target::{AuditFn, paired_device, run_browser_op, web_url};

/// Everything the handler needs from the host daemon.
pub struct ServerTargetDeps {
    /// The host's data directory (the pairing file lives under `peer/`).
    pub data: PathBuf,
    pub audit: AuditFn,
    /// Mask owner secrets in anything that leaves this host.
    pub redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// This host's own browser (headed on a display, else headless).
    pub browser: Option<Arc<dyn BrowserDriver>>,
}

/// Serves `pc.info` and `pc.browser` to the paired admin desktop.
pub struct ServerTargetHandler {
    deps: ServerTargetDeps,
}

impl ServerTargetHandler {
    #[must_use]
    pub fn new(deps: ServerTargetDeps) -> Self {
        if let Some(browser) = &deps.browser {
            // The served browser never reaches this server's own network: not
            // loopback (the daemon API, admin UIs), not link-local or cloud
            // metadata, and no private range either (nobody at this host can
            // allow one; the owner answers on the brain side).
            browser.set_destination_check(browser_cdp::DestinationCheck::new(|url| async move {
                match browser_cdp::classify_url(&url).await {
                    browser_cdp::Verdict::Allow => Ok(()),
                    browser_cdp::Verdict::Block(why) => Err(format!("blocked: {why}")),
                    browser_cdp::Verdict::Private(host) => Err(format!(
                        "blocked: {host} is inside this server's own network"
                    )),
                }
            }));
        }
        Self { deps }
    }

    fn refuse(&self, peer: &Device, method: &str, why: &str) -> RpcError {
        let _ = (self.deps.audit)(
            "server_target.refused",
            serde_json::json!({"peer": peer.id, "role": peer.role.to_string(), "method": method, "why": why}),
        );
        RpcError::new(ERR_FORBIDDEN, why)
    }

    /// The pairing as it is on disk now: the same device, the same pairing,
    /// role `desktop`, administration granted by this host's owner.
    fn authorize(&self, peer: &Device, method: &str) -> Result<(), RpcError> {
        let Some(now) = paired_device(&self.deps.data, &peer.id) else {
            return Err(self.refuse(peer, method, "not paired with this server"));
        };
        if now.pairing_generation != peer.pairing_generation {
            return Err(self.refuse(peer, method, "paired again since this request started"));
        }
        if now.role != Role::Desktop || !now.admin {
            return Err(self.refuse(
                peer,
                method,
                "only the paired administrator desktop may use this server as a computer",
            ));
        }
        Ok(())
    }

    fn info() -> Result<Value, RpcError> {
        let info = PcInfo {
            os: std::env::consts::OS.to_owned(),
            shell: if cfg!(windows) { "PowerShell" } else { "sh" }.to_owned(),
            home: None,
            hostname: std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .ok()
                .filter(|h| !h.is_empty()),
            screen: false,
            browser: true,
            commands: false,
            files: false,
        };
        serde_json::to_value(info).map_err(|e| RpcError::new(-32603, e.to_string()))
    }

    async fn browser(&self, peer: &Device, params: Value) -> Result<Value, RpcError> {
        let Some(driver) = self.deps.browser.clone() else {
            return Err(self.refuse(peer, RPC_BROWSER, "this server has no browser to use"));
        };
        let op = params
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(ERR_INVALID_PARAMS, "missing op"))?
            .to_owned();
        if !matches!(
            op.as_str(),
            "open"
                | "snapshot"
                | "click"
                | "fill"
                | "select"
                | "press"
                | "back"
                | "scroll"
                | "tabs"
                | "wait"
                | "screenshot"
                | "current_url"
        ) {
            return Err(self.refuse(peer, RPC_BROWSER, "that browser operation is not served"));
        }
        let mut detail = serde_json::json!({"peer": peer.id, "op": op});
        if op == "open" {
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if web_url(url).is_none() || url.chars().count() > 2000 {
                return Err(self.refuse(
                    peer,
                    RPC_BROWSER,
                    "only http and https pages can be opened",
                ));
            }
            if browser_cdp::classify_url(url).await != browser_cdp::Verdict::Allow {
                return Err(self.refuse(
                    peer,
                    RPC_BROWSER,
                    "that address is inside this server's own network and is not reachable",
                ));
            }
            detail["url"] = Value::String((self.deps.redact)(url));
        }
        (self.deps.audit)("server_target.browser", detail).map_err(|e| {
            RpcError::new(
                ERR_FORBIDDEN,
                format!("not done: the audit log of this server could not be written ({e})"),
            )
        })?;
        run_browser_op(&driver, &op, &params, &self.deps.redact).await
    }
}

#[async_trait]
impl RpcHandler for ServerTargetHandler {
    async fn handle(&self, peer: &Device, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "node.ping" {
            return Ok(serde_json::json!({ "pong": true }));
        }
        self.authorize(peer, method)?;
        match method {
            RPC_INFO => Self::info(),
            RPC_BROWSER => self.browser(peer, params).await,
            other => Err(RpcError::new(
                ERR_METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Browser {
        opened: Mutex<Vec<String>>,
        check: Mutex<Option<browser_cdp::DestinationCheck>>,
    }

    #[async_trait]
    impl BrowserDriver for Browser {
        fn set_destination_check(&self, check: browser_cdp::DestinationCheck) {
            *self.check.lock() = Some(check);
        }
        async fn open(&self, url: &str, _: Option<&str>) -> Result<String, String> {
            self.opened.lock().push(url.to_owned());
            Ok(format!("opened {url} token hunter2"))
        }
        async fn snapshot(&self, _: bool) -> Result<String, String> {
            Ok("page".into())
        }
        async fn click(&self, _: &str) -> Result<String, String> {
            Ok("clicked".into())
        }
        async fn fill(&self, _: &str, _: &str) -> Result<String, String> {
            Ok("filled".into())
        }
        async fn select(&self, _: &str, _: &str) -> Result<String, String> {
            Ok("selected".into())
        }
        async fn press(&self, _: &str) -> Result<String, String> {
            Ok("pressed".into())
        }
        async fn scroll(&self, _: Option<&str>, _: f64, _: f64) -> Result<String, String> {
            Ok("scrolled".into())
        }
        async fn wait(
            &self,
            _: builtin_tools::Wait,
            _: Option<std::time::Duration>,
        ) -> Result<String, String> {
            Ok("waited".into())
        }
        async fn back(&self) -> Result<String, String> {
            Ok("back".into())
        }
        async fn tabs(&self, _: Option<usize>) -> Result<String, String> {
            Ok("tabs".into())
        }
        async fn screenshot(
            &self,
            _: bool,
            _: Option<&str>,
        ) -> Result<(Vec<u8>, &'static str), String> {
            Ok((vec![1], "png"))
        }
        async fn eval(&self, _: &str) -> Result<Value, String> {
            Err("no".into())
        }
    }

    fn device(role: Role, admin: bool) -> Device {
        Device {
            id: "desk1".into(),
            name: "My desktop".into(),
            role,
            capabilities: vec![],
            addr: None,
            paired_at: 1,
            pairing_generation: "g1".into(),
            admin,
        }
    }

    fn write_pairing(dir: &std::path::Path, role: &str, admin: bool) {
        std::fs::create_dir_all(dir.join("peer")).unwrap();
        std::fs::write(
            dir.join("peer").join("devices.json"),
            serde_json::json!({"version":1,"devices":[{
                "id":"desk1","name":"My desktop","role":role,"capabilities":[],
                "addr":null,"paired_at":1,"pairing_generation":"g1","admin":admin
            }]})
            .to_string(),
        )
        .unwrap();
    }

    fn rig(role: &str, admin: bool) -> (ServerTargetHandler, Arc<Browser>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        write_pairing(dir.path(), role, admin);
        let browser = Arc::new(Browser::default());
        let h = ServerTargetHandler::new(ServerTargetDeps {
            data: dir.path().to_path_buf(),
            audit: Arc::new(|_, _| Ok(())),
            redact: Arc::new(|t| t.replace("hunter2", "••••")),
            browser: Some(browser.clone() as Arc<dyn BrowserDriver>),
        });
        (h, browser, dir)
    }

    fn open(url: &str) -> Value {
        serde_json::json!({"op": "open", "url": url})
    }

    /// The server's browser is served to the paired administrator desktop and to
    /// nobody else: not a phone, a node, another host, a desktop without the
    /// administrator flag, and not a device the pairing file does not list.
    #[tokio::test]
    async fn only_the_paired_admin_desktop_may_use_the_servers_browser() {
        let (h, browser, _dir) = rig("desktop", true);
        let ok = h
            .handle(
                &device(Role::Desktop, true),
                RPC_BROWSER,
                open("https://example.com/"),
            )
            .await
            .unwrap();
        assert!(ok.to_string().contains("opened https://example.com/"));
        assert!(!ok.to_string().contains("hunter2"), "output is masked");
        assert_eq!(browser.opened.lock().len(), 1);
        // The role the peer connection claims is not what counts: the pairing file is.
        for role in ["phone", "node", "host"] {
            let (h, browser, _d) = rig(role, true);
            let res = h
                .handle(
                    &device(Role::Desktop, true),
                    RPC_BROWSER,
                    open("https://example.com/"),
                )
                .await;
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN, "{role}");
            assert!(browser.opened.lock().is_empty());
        }
        // A desktop the owner did not make an administrator.
        let (h, browser, _d) = rig("desktop", false);
        let res = h
            .handle(
                &device(Role::Desktop, true),
                RPC_BROWSER,
                open("https://example.com/"),
            )
            .await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        // Unpaired between two requests.
        let (h, browser2, dir) = rig("desktop", true);
        std::fs::remove_file(dir.path().join("peer").join("devices.json")).unwrap();
        let res = h
            .handle(
                &device(Role::Desktop, true),
                RPC_BROWSER,
                open("https://example.com/"),
            )
            .await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        assert!(browser.opened.lock().is_empty() && browser2.opened.lock().is_empty());
    }

    /// Blocker 4 (server direction): the served browser never reaches the
    /// server's own network, directly or through a redirect or subresource.
    #[tokio::test]
    async fn the_served_browser_cannot_reach_the_servers_internal_services() {
        let (h, browser, _dir) = rig("desktop", true);
        let me = device(Role::Desktop, true);
        for url in [
            "http://127.0.0.1:8123/v1/status",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
        ] {
            let res = h.handle(&me, RPC_BROWSER, open(url)).await;
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN, "{url}");
        }
        assert!(browser.opened.lock().is_empty());
        // The policy installed on the browser itself (redirect hops, subresources).
        let check = browser.check.lock().clone().expect("a policy is installed");
        for url in [
            "http://127.0.0.1:8123/",
            "http://[::1]/",
            "http://169.254.169.254/",
            "http://192.168.0.10/",
        ] {
            assert!(check.check(url.into()).await.is_err(), "{url}");
        }
        assert!(check.check("https://93.184.216.34/".into()).await.is_ok());
    }

    /// Only web pages open; nothing but the browser family exists here.
    #[tokio::test]
    async fn only_web_pages_and_only_the_browser_family_are_served() {
        let (h, browser, _dir) = rig("desktop", true);
        let me = device(Role::Desktop, true);
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "about:blank"] {
            let res = h.handle(&me, RPC_BROWSER, open(bad)).await;
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN, "{bad}");
        }
        let res = h
            .handle(
                &me,
                RPC_BROWSER,
                serde_json::json!({"op": "eval", "js": "1"}),
            )
            .await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        for method in ["pc.tool", "desktop.windows", "desktop.act"] {
            let res = h.handle(&me, method, Value::Null).await;
            assert_eq!(res.unwrap_err().code, ERR_METHOD_NOT_FOUND, "{method}");
        }
        assert!(browser.opened.lock().is_empty());
    }
}
