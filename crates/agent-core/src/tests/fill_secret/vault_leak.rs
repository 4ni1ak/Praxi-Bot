//! Vault leak suite (#220): one scripted run types a canary secret through
//! `browser_fill`, `desktop_type` and `desktop_set_value` while the scripted
//! model tries to coax the value out, then every sink the run can reach is
//! scanned for the canary and its encodings. A negative control proves the
//! scanner and the sink collection still see a leak.
//!
//! Coverage (leak channels of #218 / #220):
//!
//! | Sink | Covered by |
//! |---|---|
//! | Requests sent to the model provider (whole `ChatRequest`) | `canary_reaches_no_sink_while_the_model_coaxes_it` |
//! | Tool results and tool errors (scrubbed echo, throwing handler) | same test, "tool results" + "provider requests" |
//! | Event stream (SSE source) | same test, "event stream" |
//! | Approval cards (event and stored row) | same test, "approval cards" + "stored rows" |
//! | Run journal, step records, audit rows, chat messages | same test, "stored rows" (`sinks_text`) |
//! | Files on disk (databases, event log, journal, tool data dir) | same test, disk scan of the data dir |
//! | Captured `tracing` output | same test, "tracing" |
//! | Page text / accessibility snapshots (browser + desktop tree) | same test: the page echoes the value into DOM, title, URL, console; the desktop tree shows the typed value |
//! | Secret listing (`secrets_list`, owner listing) | same test, "secret listing" |
//! | Screenshots and live view | not covered hermetically: the fakes return fixed bytes, there is no renderer that could draw the value |
//! | Memory and knowledge stores | disk scan only (their files sit under the data dir); no memory write is scripted |
//! | Telegram / other channels | not covered: no channel fake is wired into the agent loop rig |
//! | MCP server mode responses | not covered: lives in another crate, no hermetic rig here |
//! | Daemon API responses and audit rows | `daemon-api` `tests/fill_secrets.rs` (per-path, not canary based) |
//! | Clipboard, crash dumps, swap | out of scope (documented limits) |
//!
//! Adding a case: script the new tool call into the run (or a new run), push
//! its sink onto `Sinks`, and add a row above.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use desktop_api::snapshot::{AppSpec, Image, Pid, UiNode, WindowInfo};
use desktop_api::{
    Action, ActionResult, Capabilities, DesktopBackend, DesktopError, Quality, Region, Snapshot,
    TreeQuality,
};

use super::*;

// ------------------------------------------------------------------ canary

/// A unique, high-entropy secret value and the renderings a leak could take.
/// The encodings are computed here, independently of the production redactor,
/// so a regression there cannot silently blind the scanner.
struct Canary {
    value: String,
    forms: Vec<(&'static str, String)>,
}

impl Canary {
    fn new() -> Self {
        let core = format!("Vk{}", uuid::Uuid::new_v4().simple());
        // Characters that every encoding renders differently from the raw value.
        let value = format!("{core}\"/+= !");
        let json = serde_json::to_string(&value).unwrap();
        let split = |sep: &str| {
            core.as_bytes()
                .chunks(4)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect::<Vec<_>>()
                .join(sep)
        };
        let mut url = String::new();
        for b in value.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                url.push(char::from(b));
            } else {
                let _ = write!(url, "%{b:02X}");
            }
        }
        let forms = vec![
            ("raw", value.clone()),
            ("json-escaped", json[1..json.len() - 1].to_owned()),
            ("url-encoded", url),
            ("base64", STANDARD.encode(&value)),
            ("base64url", URL_SAFE_NO_PAD.encode(&value)),
            ("split-spaces", split(" ")),
            ("split-dashes", split("-")),
            ("truncated-prefix", core[..26].to_owned()),
        ];
        Self { value, forms }
    }

    fn form(&self, name: &str) -> &str {
        &self.forms.iter().find(|(n, _)| *n == name).unwrap().1
    }

    /// `Err` names the sink and the encoding that leaked.
    fn scan(&self, sink: &str, text: &str) -> Result<(), String> {
        match self.forms.iter().find(|(_, f)| text.contains(f.as_str())) {
            Some((enc, _)) => Err(format!("canary leaked into {sink} ({enc})")),
            None => Ok(()),
        }
    }

    fn assert_no_canary(&self, sink: &str, text: &str) {
        if let Err(e) = self.scan(sink, text) {
            panic!("{e}");
        }
    }
}

/// The scanner recognises every encoding it claims to.
#[test]
fn the_canary_scanner_names_each_encoding() {
    let c = Canary::new();
    assert_eq!(c.scan("x", "nothing here"), Ok(()));
    for (enc, form) in &c.forms {
        let err = c
            .scan("sink-a", &format!("prefix {form} suffix"))
            .unwrap_err();
        assert!(err.contains("sink-a"), "{err}");
        // The raw value also contains the truncated prefix: the first match wins.
        if *enc != "truncated-prefix" {
            assert!(err.contains(enc), "{enc}: {err}");
        }
    }
    // Inside serialized JSON the raw value only exists escaped.
    let json = serde_json::to_string(&json!({"v": c.value})).unwrap();
    assert!(c.scan("j", &json).is_err());
}

// ------------------------------------------------------------------ desktop fake

const PROGRAM: &str = "win:sha256:vault-leak";

/// One window whose edit (index 3) shows the last value typed or set, like a
/// real accessibility tree of a field without masking.
#[derive(Default)]
struct Desk {
    generation: Mutex<u64>,
    typed: Mutex<Vec<String>>,
}

fn desk_window() -> WindowInfo {
    WindowInfo {
        id: 77,
        title: "VPN client".into(),
        pid: 4242,
        bounds: desktop_api::snapshot::Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
        },
        focused: true,
    }
}

#[async_trait]
impl DesktopBackend for Desk {
    async fn snapshot(&self, _: Option<u64>) -> Result<Snapshot, DesktopError> {
        let mut g = self.generation.lock();
        *g += 1;
        let shown = self.typed.lock().last().cloned().unwrap_or_default();
        let nodes: Vec<UiNode> = (0..5)
            .map(|i| {
                let role = if i == 3 { "edit" } else { "button" };
                UiNode::new(
                    i,
                    role,
                    format!("el{i}"),
                    (i == 3).then(|| shown.clone()),
                    None,
                    vec![],
                )
            })
            .collect();
        Ok(Snapshot {
            window: desk_window(),
            quality: TreeQuality::assess(&nodes),
            nodes,
            generation: *g,
        })
    }
    async fn act(&self, action: Action) -> Result<ActionResult, DesktopError> {
        match &action {
            Action::TypeText { text } => self.typed.lock().push(text.clone()),
            Action::SetValue { value, .. } => self.typed.lock().push(value.clone()),
            _ => {}
        }
        Ok(ActionResult {
            dispatched: true,
            verified: None,
            new_snapshot: None,
        })
    }
    async fn screenshot(&self, _: Region, _: Quality) -> Result<Image, DesktopError> {
        Ok(Image {
            data: Vec::new(),
            width: 10,
            height: 10,
            mime_type: "image/png".into(),
        })
    }
    async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        Ok(vec![desk_window()])
    }
    async fn launch(&self, _: AppSpec) -> Result<Pid, DesktopError> {
        Ok(4242)
    }
    async fn program_identity(&self, _: &WindowInfo) -> Option<String> {
        Some(PROGRAM.into())
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: "fake".into(),
            accessibility_tree: true,
            screenshot: true,
            app_launch: false,
            has_display: true,
        }
    }
}

// ------------------------------------------------------------------ the run

/// Every sink of one run, by name, as text.
struct Sinks(Vec<(&'static str, String)>);

impl Sinks {
    fn get(&self, name: &str) -> &str {
        &self.0.iter().find(|(n, _)| *n == name).unwrap().1
    }

    /// Scan every sink and every file under `dir`.
    fn scan(&self, c: &Canary, dir: &Path) -> Result<(), String> {
        for (sink, text) in &self.0 {
            c.scan(sink, text)?;
        }
        for (enc, form) in &c.forms {
            if let Some(file) = files_containing(dir, form).first() {
                return Err(format!("canary leaked into file {file} ({enc})"));
            }
        }
        Ok(())
    }
}

/// A rig whose `login/github` secret holds `c` and is also allowed for the
/// fake desktop program, with the desktop tools registered.
fn leak_rig(c: &Canary, replies: Vec<Reply>) -> (Rig, Arc<Desk>) {
    let r = rig(Policy::full(), true, replies);
    r.secrets
        .upsert_with(
            "login/github",
            secret_store::FillSecretUpdate {
                value: Some(&c.value),
                programs: Some(&[PROGRAM.to_owned()]),
                ..secret_store::FillSecretUpdate::default()
            },
        )
        .unwrap();
    let desk = Arc::new(Desk::default());
    for t in builtin_tools::desktop::tools_dynamic_with_secrets(
        Some(desk.clone() as Arc<dyn DesktopBackend>),
        None,
        &r.env.dir.path().join("desk"),
        builtin_tools::DesktopSettings {
            action_settle: Duration::ZERO,
            launch_settle: Duration::ZERO,
        },
        None,
        Some(Arc::clone(&r.secrets)),
    ) {
        r.lp.sh.tools.register(t);
    }
    (r, desk)
}

/// Run one turn, approving every card, and collect every sink.
async fn run_and_collect(r: &Rig, log: &Mutex<String>) -> (Sinks, usize) {
    let mut rx = r.lp.subscribe();
    let mut whole = r.lp.subscribe();
    let run =
        r.lp.run_turn(r.env.chat, "log in to github and the vpn")
            .unwrap();
    let wait = r.lp.wait(run);
    tokio::pin!(wait);
    let mut approvals = 0;
    let state = loop {
        tokio::select! {
            done = &mut wait => break done.unwrap(),
            Ok(i) = rx.recv() => {
                if let Event::ApprovalNeeded(a) = &i.envelope.event {
                    approvals += 1;
                    r.lp.approve(a.call_id, ApprovalScope::Once, r.env.user).unwrap();
                }
            }
        }
    };
    assert_eq!(state, RunState::Succeeded);
    r.lp.shutdown(Duration::from_millis(300)).await;

    let mut requests = String::new();
    for i in 0..r.fake.n_requests() {
        requests.push_str(&serde_json::to_string(&r.fake.request(i)).unwrap());
    }
    let (mut events, mut cards) = (String::new(), String::new());
    while let Ok(i) = whole.try_recv() {
        let e = serde_json::to_string(&i.envelope.event).unwrap();
        if matches!(i.envelope.event, Event::ApprovalNeeded(_)) {
            cards.push_str(&e);
        }
        events.push_str(&e);
    }
    let last = r.fake.request(r.fake.n_requests() - 1);
    let sinks = Sinks(vec![
        ("provider requests", requests),
        ("event stream", events),
        ("approval cards", cards),
        ("stored rows", sinks_text(r)),
        ("tracing", log.lock().clone()),
        ("tool results", tool_messages(&last).join("\n")),
        (
            "secret listing",
            format!("{:?}{:?}", r.secrets.list(), r.secrets.accounts(None)),
        ),
    ]);
    (sinks, approvals)
}

/// Guard: the canary is typed through all three fill paths and reaches no
/// sink, in any encoding, while the model asks for it in every way it can and
/// the page and the desktop tree echo it back.
#[tokio::test]
async fn canary_reaches_no_sink_while_the_model_coaxes_it() {
    let log = Arc::new(Mutex::new(String::new()));
    let _guard = tracing::subscriber::set_default(crate::account_register_tests::LogCapture(
        Arc::clone(&log),
    ));
    let c = Canary::new();
    let secret = json!({"ref": "e5", "secret": "login/github"});
    let (r, desk) = leak_rig(
        &c,
        vec![
            calls(&[("l1", "secrets_list", json!({}))]),
            calls(&[("l2", "secrets_list", json!({}))]),
            // The page echoes what it receives (`echo: true`).
            calls(&[("f1", "browser_fill", secret.clone())]),
            // The page's handler throws an error quoting the typed value.
            calls(&[(
                "f2",
                "browser_fill",
                json!({"ref": "e9", "secret": "login/github"}),
            )]),
            // Read the page right after the fill, and screenshot it.
            calls(&[("s1", "browser_snapshot", json!({}))]),
            calls(&[("s2", "browser_screenshot", json!({}))]),
            calls(&[("d1", "desktop_get_state", json!({}))]),
            calls(&[(
                "d2",
                "desktop_type",
                json!({"index": 3, "gen": 1, "secret": "login/github"}),
            )]),
            calls(&[(
                "d3",
                "desktop_set_value",
                json!({"index": 3, "gen": 2, "secret": "login/github"}),
            )]),
            // Read the tree that now shows the value.
            calls(&[("d4", "desktop_get_state", json!({}))]),
            text("done"),
        ],
    );
    r.page.targets.lock().insert("e9".into(), target(GITHUB));
    *r.page.throw_on.lock() = Some("e9".into());
    // A hostile page: the value in the URL, the title, the DOM, the console.
    *r.page.snapshot_text.lock() = format!(
        "url: {GITHUB}/login?pw={url}\ntitle: Welcome {raw}\n\
         [e5] textbox \"Password\" value=\"{raw}\" (shown)\n\
         console: error {{\"typed\":\"{json}\"}}\ndata-b64={b64} data-b64url={b64u}",
        url = c.form("url-encoded"),
        raw = c.value,
        json = c.form("json-escaped"),
        b64 = c.form("base64"),
        b64u = c.form("base64url"),
    );

    let (sinks, approvals) = run_and_collect(&r, &log).await;

    // The value really was typed: once by the browser, twice on the desktop.
    assert_eq!(r.page.typed_secrets.lock().as_slice(), [c.value.as_str()]);
    assert_eq!(
        desk.typed.lock().as_slice(),
        [c.value.as_str(), c.value.as_str()],
        "desktop_type and desktop_set_value both reached the program; tool results: {}",
        sinks.get("tool results")
    );
    assert_eq!(approvals, 4, "each secret fill asked");

    // Positive controls: every sink holds something from this run.
    for sink in [
        "provider requests",
        "event stream",
        "approval cards",
        "stored rows",
        "secret listing",
    ] {
        assert!(sinks.get(sink).contains("login/github"), "control: {sink}");
    }
    assert!(!sinks.get("tracing").is_empty(), "control: tracing");
    assert!(
        !files_containing(r.env.dir.path(), "login/github").is_empty(),
        "control: files"
    );
    let requests = sinks.get("provider requests");
    assert!(requests.contains("filled e5"), "control: echo result");
    assert!(requests.contains("input handler threw"), "control: error");
    assert!(requests.contains("[REDACTED]"), "control: echoes masked");

    for (sink, text) in &sinks.0 {
        c.assert_no_canary(sink, text);
    }
    sinks.scan(&c, r.env.dir.path()).unwrap();
}

/// Negative control: a leaky tool that returns a rendering the redactor does
/// not know (the value split by dashes) and a log line with the raw value are
/// both caught, naming the sink and the encoding.
#[tokio::test]
async fn the_scan_catches_a_leaky_tool_and_a_leaky_log_line() {
    let log = Arc::new(Mutex::new(String::new()));
    let _guard = tracing::subscriber::set_default(crate::account_register_tests::LogCapture(
        Arc::clone(&log),
    ));
    let c = Canary::new();
    let (r, _desk) = leak_rig(
        &c,
        vec![calls(&[("x1", "leaky_echo", json!({}))]), text("done")],
    );
    let leaky = tool(
        "leaky_echo",
        tool_registry::Effect::Read,
        c.form("split-dashes"),
    );
    r.lp.sh.tools.register(Arc::new(leaky));
    tracing::info!(typed = %c.value, "a careless log line");

    let (sinks, _) = run_and_collect(&r, &log).await;

    let err = sinks.scan(&c, r.env.dir.path()).unwrap_err();
    assert!(
        err.contains("provider requests") && err.contains("split-dashes"),
        "{err}"
    );
    let err = c.scan("tracing", sinks.get("tracing")).unwrap_err();
    assert!(err.contains("tracing (raw)"), "{err}");
    assert!(c.scan("tool results", sinks.get("tool results")).is_err());
}
