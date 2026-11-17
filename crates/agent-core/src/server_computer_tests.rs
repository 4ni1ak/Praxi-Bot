//! A bot that lives on THIS computer but uses a paired server as ITS computer
//! (card 272, gap 2): the model is still called here, the browser tools go to
//! the server over the paired link, and nothing else changes places.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use builtin_tools::browser::BrowserDriver;
use builtin_tools::pc::{PcError, PcLink, RPC_BROWSER, ServerLinks};
use parking_lot::Mutex;
use permission_engine::Policy;
use serde_json::{Value, json};
use task_runtime::RunState;
use tool_registry::Tool;

use crate::tests::{Fake, MACHINE_B, calls, cfg, make_loop, profile, setup, text, tool_messages};

/// The server's end of the link: records what it was asked.
#[derive(Default)]
struct ServerLink {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl PcLink for ServerLink {
    async fn call(&self, method: &str, params: Value, _: Duration) -> Result<Value, PcError> {
        self.calls.lock().push((method.to_owned(), params.clone()));
        Ok(match params["op"].as_str() {
            Some("current_url") => json!({"url": null}),
            Some("open") => json!({"text": "opened on the server"}),
            _ => json!({"text": "ok"}),
        })
    }
    fn desktop(&self) -> Result<Arc<dyn desktop_api::DesktopBackend>, PcError> {
        Err(PcError::Failed("no desktop".into()))
    }
}

struct Links(Arc<ServerLink>);

impl ServerLinks for Links {
    fn link(&self, id: &str) -> Option<(String, Arc<dyn PcLink>)> {
        (id == MACHINE_B).then(|| {
            (
                "Oracle server".to_owned(),
                self.0.clone() as Arc<dyn PcLink>,
            )
        })
    }
}

/// This computer's own browser: must never be used by such a bot.
#[derive(Default)]
struct LocalBrowser {
    used: AtomicUsize,
}

#[async_trait]
impl BrowserDriver for LocalBrowser {
    async fn open(&self, _: &str, _: Option<&str>) -> Result<String, String> {
        self.used.fetch_add(1, Ordering::SeqCst);
        Ok("opened HERE".into())
    }
    async fn snapshot(&self, _: bool) -> Result<String, String> {
        self.used.fetch_add(1, Ordering::SeqCst);
        Ok("here".into())
    }
    async fn click(&self, _: &str) -> Result<String, String> {
        Ok("c".into())
    }
    async fn fill(&self, _: &str, _: &str) -> Result<String, String> {
        Ok("f".into())
    }
    async fn select(&self, _: &str, _: &str) -> Result<String, String> {
        Ok("s".into())
    }
    async fn press(&self, _: &str) -> Result<String, String> {
        Ok("p".into())
    }
    async fn scroll(&self, _: Option<&str>, _: f64, _: f64) -> Result<String, String> {
        Ok("s".into())
    }
    async fn wait(&self, _: builtin_tools::Wait, _: Option<Duration>) -> Result<String, String> {
        Ok("w".into())
    }
    async fn back(&self) -> Result<String, String> {
        Ok("b".into())
    }
    async fn tabs(&self, _: Option<usize>) -> Result<String, String> {
        Ok("t".into())
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

/// `browser_open` of a bot whose computer is the server goes to the server;
/// this computer's browser is not touched; the model is called here.
#[tokio::test]
async fn a_local_bot_with_the_server_as_its_computer_browses_on_the_server() {
    let env = setup();
    let fake = Fake::new(vec![
        calls(&[("c1", "browser_open", json!({"url": "https://example.com/"}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(&format!("[runtime]\ncomputer = \"server:{MACHINE_B}\"\n")),
        cfg(Policy::full()),
    );
    let server = Arc::new(ServerLink::default());
    lp.set_server_links(Arc::new(Links(server.clone())));
    let local = Arc::new(LocalBrowser::default());
    let control: Arc<dyn builtin_tools::DesktopControlGate> =
        Arc::new(crate::control::LoopControl {
            sh: Arc::downgrade(&lp.sh),
        });
    let dir = tempfile::tempdir().unwrap();
    for tool in builtin_tools::browser::tools_with_secrets(
        local.clone() as Arc<dyn BrowserDriver>,
        dir.path(),
        Some(control),
        None,
    ) {
        assert!(lp.sh.tools.register(tool as Arc<dyn Tool>));
    }
    let run = lp.run_turn(env.chat, "open example.com").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    // The model ran here: it was asked twice (the tool call, then the answer),
    // and the second request carries the server's page.
    let second = tool_messages(&fake.request(1)).join(" ");
    assert!(second.contains("opened on the server"), "{second}");
    // The server got the open; this computer's browser got nothing.
    let seen = server.calls.lock().clone();
    assert!(
        seen.iter().any(|(m, p)| m == RPC_BROWSER
            && p["op"] == "open"
            && p["url"] == "https://example.com/"),
        "{seen:?}"
    );
    assert_eq!(local.used.load(Ordering::SeqCst), 0);
}

/// The same bot with its server unpaired has no computer tools and says why,
/// instead of quietly browsing on this computer.
#[tokio::test]
async fn an_unpaired_server_never_falls_back_to_this_computers_browser() {
    let env = setup();
    let fake = Fake::new(vec![
        calls(&[("c1", "browser_open", json!({"url": "https://example.com/"}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(&format!("[runtime]\ncomputer = \"server:{MACHINE_B}\"\n")),
        cfg(Policy::full()),
    );
    lp.set_server_links(Arc::new(Links(Arc::new(ServerLink::default()))));
    // A link table that knows no such server.
    lp.set_server_links(Arc::new(NoServers));
    let local = Arc::new(LocalBrowser::default());
    let control: Arc<dyn builtin_tools::DesktopControlGate> =
        Arc::new(crate::control::LoopControl {
            sh: Arc::downgrade(&lp.sh),
        });
    let dir = tempfile::tempdir().unwrap();
    for tool in builtin_tools::browser::tools_with_secrets(
        local.clone() as Arc<dyn BrowserDriver>,
        dir.path(),
        Some(control),
        None,
    ) {
        let _ = lp.sh.tools.register(tool as Arc<dyn Tool>);
    }
    let run = lp.run_turn(env.chat, "open example.com").unwrap();
    let _ = lp.wait(run).await;
    assert_eq!(local.used.load(Ordering::SeqCst), 0);
}

struct NoServers;

impl ServerLinks for NoServers {
    fn link(&self, _: &str) -> Option<(String, Arc<dyn PcLink>)> {
        None
    }
}
