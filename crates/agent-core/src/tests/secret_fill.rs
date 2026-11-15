//! A secret typed into a password field never reaches the approval card, the
//! event stream and journal, the audit rows, the stored step records or the
//! history shown to the model, while the tool still types the real value.
//!
//! Every test runs the real agent loop with the real `browser_fill`,
//! `desktop_type` and `desktop_set_value` tools over fake backends, scripts a
//! model that fills a password field and an ordinary search field, approves
//! every card, and then scans each sink for the literal values. The ordinary
//! value is the positive control: the scan can see a value that is not secret.

use builtin_tools::{BrowserDriver, BuiltinConfig, FieldFacts, GateRequest, ToolGate, Wait};
use desktop_api::snapshot::{AppSpec, Pid, Rect};
use desktop_api::{
    Action, ActionResult, Capabilities, DesktopBackend, DesktopError, Image, Quality, Region,
    Snapshot, TreeQuality, UiNode, WindowInfo,
};
use protocol::event::ApprovalNeeded;

use super::*;

const SECRET: &str = "hunter2-fake-fixture-value";
const PLAIN: &str = "plain search words";

struct AllowAll;

#[async_trait]
impl ToolGate for AllowAll {
    async fn authorize(&self, _req: &GateRequest) -> Result<(), String> {
        Ok(())
    }
}

/// A page with a password field `e1` and a search field `e2`.
#[derive(Default)]
struct LoginPage {
    fills: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl BrowserDriver for LoginPage {
    async fn open(&self, _url: &str, _profile: Option<&str>) -> Result<String, String> {
        Err("unused".into())
    }
    async fn snapshot(&self, _i: bool) -> Result<String, String> {
        Err("unused".into())
    }
    async fn click(&self, _r: &str) -> Result<String, String> {
        Err("unused".into())
    }
    async fn fill(&self, r: &str, t: &str) -> Result<String, String> {
        self.fills.lock().push((r.to_owned(), t.to_owned()));
        Ok("filled".into())
    }
    async fn select(&self, _r: &str, _v: &str) -> Result<String, String> {
        Err("unused".into())
    }
    async fn press(&self, _k: &str) -> Result<String, String> {
        Err("unused".into())
    }
    async fn scroll(&self, _r: Option<&str>, _x: f64, _y: f64) -> Result<String, String> {
        Err("unused".into())
    }
    async fn wait(&self, _c: Wait, _t: Option<Duration>) -> Result<String, String> {
        Err("unused".into())
    }
    async fn back(&self) -> Result<String, String> {
        Err("unused".into())
    }
    async fn tabs(&self, _s: Option<usize>) -> Result<String, String> {
        Err("unused".into())
    }
    async fn screenshot(
        &self,
        _f: bool,
        _e: Option<&str>,
    ) -> Result<(Vec<u8>, &'static str), String> {
        Err("unused".into())
    }
    async fn eval(&self, _js: &str) -> Result<Value, String> {
        Err("unused".into())
    }
    async fn field_facts(&self, r: &str) -> Option<FieldFacts> {
        let (name, sensitive) = match r {
            "e1" => ("Password", true),
            "e2" => ("Search", false),
            _ => return None,
        };
        Some(FieldFacts {
            role: "textbox".into(),
            name: name.into(),
            sensitive,
            url: "https://login.example.test/signin?next=%2F".into(),
        })
    }
}

/// A window with a search box (index 1) and a password box (index 2, `secure`).
#[derive(Default)]
struct LoginWindow {
    actions: Mutex<Vec<Action>>,
}

fn login_window() -> WindowInfo {
    WindowInfo {
        id: 77,
        title: "Sign in".into(),
        pid: 4242,
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
        },
        focused: true,
    }
}

#[async_trait]
impl DesktopBackend for LoginWindow {
    async fn snapshot(&self, _scope: Option<u64>) -> Result<Snapshot, DesktopError> {
        let nodes = vec![
            UiNode::new(0, "button", "Go", None, None, vec![]),
            UiNode::new(1, "edit", "Search", None, None, vec![]),
            UiNode::new(2, "edit", "Password", None, None, vec!["secure".into()]),
        ];
        let quality = TreeQuality::assess(&nodes);
        Ok(Snapshot {
            window: login_window(),
            nodes,
            generation: 1,
            quality,
        })
    }
    async fn act(&self, action: Action) -> Result<ActionResult, DesktopError> {
        self.actions.lock().push(action);
        Ok(ActionResult {
            dispatched: true,
            verified: None,
            new_snapshot: None,
        })
    }
    async fn screenshot(&self, _r: Region, _q: Quality) -> Result<Image, DesktopError> {
        Err(DesktopError::Unsupported("screenshot".into()))
    }
    async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        Ok(vec![login_window()])
    }
    async fn launch(&self, _app: AppSpec) -> Result<Pid, DesktopError> {
        Err(DesktopError::Unsupported("launch".into()))
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            platform: "fake".into(),
            accessibility_tree: true,
            screenshot: false,
            app_launch: false,
            has_display: true,
        }
    }
}

/// Everything a run left behind, as text, per sink.
struct Sinks {
    approvals: Vec<ApprovalNeeded>,
    /// Name, text.
    sinks: Vec<(&'static str, String)>,
}

impl Sinks {
    fn text(&self, name: &str) -> &str {
        &self.sinks.iter().find(|(n, _)| *n == name).unwrap().1
    }
}

/// Run one turn whose single model reply makes `script` calls, approve every
/// card, finish, and gather every sink.
async fn run(
    page: &Arc<LoginPage>,
    window: &Arc<LoginWindow>,
    script: Vec<(&str, &str, Value)>,
) -> Sinks {
    let env = setup();
    let fake = Fake::new(vec![calls(&script), text("done")]);
    let registry = ToolRegistry::new();
    let mut tools = BuiltinConfig::new(env.dir.path().to_path_buf(), Arc::new(AllowAll));
    tools.browser = Some(Arc::clone(page) as Arc<dyn BrowserDriver>);
    tools.desktop = Some(Arc::clone(window) as Arc<dyn DesktopBackend>);
    let _ = builtin_tools::register_builtin_tools(&registry, tools);
    let lp = AgentLoop::new(
        Arc::clone(&env.storage),
        registry,
        Providers::single(Arc::clone(&fake) as Arc<dyn ChatProvider>),
        cfg(Policy::approve_destructive()),
    );
    lp.add_profile(env.bot, profile(""));
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "log me in").unwrap();

    let mut stream: Vec<Event> = Vec::new();
    let mut approvals = Vec::new();
    loop {
        let item = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out")
            .expect("stream closed");
        let event = item.envelope.event;
        if let Event::ApprovalNeeded(a) = &event {
            approvals.push(a.clone());
            lp.approve(a.call_id, ApprovalScope::Once, env.user)
                .unwrap();
        }
        let finished = matches!(event, Event::RunFinished(_));
        stream.push(event);
        if finished {
            break;
        }
    }
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    lp.sh.flush_journal();

    let sinks = vec![
        (
            "approval payload",
            approvals
                .iter()
                .map(|a| format!("{} | {} | {:?}", a.summary, a.input, a.details))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ("sse stream", format!("{stream:?}")),
        (
            "event journal",
            format!("{:?}", read_event_log(env.dir.path(), run).unwrap()),
        ),
        (
            "audit rows",
            format!("{:?}", env.storage.list_audit(env.user).unwrap()),
        ),
        (
            "approval rows",
            format!("{:?}", env.storage.list_approvals(run).unwrap()),
        ),
        (
            "stored steps",
            format!("{:?}", env.storage.list_steps(run).unwrap()),
        ),
        (
            "history shown to the model",
            format!("{:?}", fake.requests.lock().clone()),
        ),
    ];
    Sinks { approvals, sinks }
}

/// The secret is nowhere; the ordinary value (positive control) is where a
/// readable value belongs; the tool was given the real value.
fn assert_masked(s: &Sinks) {
    for (name, text) in &s.sinks {
        assert!(!text.contains(SECRET), "{name} holds the secret:\n{text}");
        assert!(!text.contains("hunter"), "{name} holds a prefix:\n{text}");
    }
    for name in [
        "approval payload",
        "sse stream",
        "event journal",
        "stored steps",
        "history shown to the model",
    ] {
        assert!(
            s.text(name).contains(PLAIN),
            "positive control: {name} should show the ordinary text:\n{}",
            s.text(name)
        );
    }
    // The audit rows never carry call arguments; the card rows only the summary.
    assert!(!s.text("audit rows").contains(PLAIN));
    assert!(s.text("approval rows").contains(PLAIN));
}

#[tokio::test]
async fn browser_fill_of_a_password_field_is_masked_everywhere_and_typed_for_real() {
    let (page, window) = (
        Arc::new(LoginPage::default()),
        Arc::new(LoginWindow::default()),
    );
    let s = run(
        &page,
        &window,
        vec![
            ("c1", "browser_fill", json!({"ref": "e1", "text": SECRET})),
            ("c2", "browser_fill", json!({"ref": "e2", "text": PLAIN})),
        ],
    )
    .await;
    assert_masked(&s);

    // The user still sees which field of which origin gets a masked value.
    let card = &s.approvals[0];
    assert_eq!(card.tool_name, "browser_fill");
    assert_eq!(card.input["ref"], "e1");
    assert_eq!(card.input["field"], "textbox \"Password\"");
    assert_eq!(card.input["origin"], "https://login.example.test");
    assert!(tool_registry::is_masked_secret(
        card.input["text"].as_str().unwrap()
    ));
    assert!(card.summary.contains("Password"), "{}", card.summary);
    assert!(
        card.summary.contains("login.example.test"),
        "{}",
        card.summary
    );
    // The ordinary field stays readable and unannotated.
    assert_eq!(s.approvals[1].input, json!({"ref": "e2", "text": PLAIN}));

    // The page was given the real values.
    assert_eq!(
        *page.fills.lock(),
        vec![
            ("e1".to_owned(), SECRET.to_owned()),
            ("e2".to_owned(), PLAIN.to_owned())
        ]
    );
}

#[tokio::test]
async fn desktop_type_and_set_value_into_a_password_box_are_masked_everywhere_and_typed_for_real() {
    let (page, window) = (
        Arc::new(LoginPage::default()),
        Arc::new(LoginWindow::default()),
    );
    let s = run(
        &page,
        &window,
        vec![
            ("d0", "desktop_get_state", json!({})),
            (
                "c1",
                "desktop_type",
                json!({"index": 2, "gen": 1, "text": SECRET}),
            ),
            (
                "c2",
                "desktop_set_value",
                json!({"index": 2, "gen": 1, "value": SECRET}),
            ),
            (
                "c3",
                "desktop_type",
                json!({"index": 1, "gen": 1, "text": PLAIN}),
            ),
        ],
    )
    .await;
    assert_masked(&s);

    for (card, key) in [(&s.approvals[0], "text"), (&s.approvals[1], "value")] {
        assert_eq!(card.input["index"], 2);
        assert_eq!(card.input["field"], "edit \"Password\"");
        assert_eq!(card.input["window"], "Sign in");
        assert!(tool_registry::is_masked_secret(
            card.input[key].as_str().unwrap()
        ));
    }
    assert_eq!(
        s.approvals[2].input,
        json!({"index": 1, "gen": 1, "text": PLAIN})
    );

    let actions = window.actions.lock();
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::TypeText { text } if text == SECRET)),
        "desktop_type typed the real value"
    );
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::SetValue { value, .. } if value == SECRET)),
        "desktop_set_value set the real value"
    );
}

#[tokio::test]
async fn an_unapproved_secret_fill_is_masked_too_and_never_typed() {
    // Denied by policy: the card was shown (masked) and nothing was typed.
    let env = setup();
    let page = Arc::new(LoginPage::default());
    let fake = Fake::new(vec![
        calls(&[("c1", "browser_fill", json!({"ref": "e1", "text": SECRET}))]),
        text("done"),
    ]);
    let registry = ToolRegistry::new();
    let mut tools = BuiltinConfig::new(env.dir.path().to_path_buf(), Arc::new(AllowAll));
    tools.browser = Some(Arc::clone(&page) as Arc<dyn BrowserDriver>);
    let _ = builtin_tools::register_builtin_tools(&registry, tools);
    let lp = AgentLoop::new(
        Arc::clone(&env.storage),
        registry,
        Providers::single(Arc::clone(&fake) as Arc<dyn ChatProvider>),
        cfg(Policy::approve_destructive()),
    );
    lp.add_profile(env.bot, profile(""));
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "log me in").unwrap();
    let Event::ApprovalNeeded(a) = until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };
    lp.deny(a.call_id, env.user, Some("no".into())).unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert!(page.fills.lock().is_empty(), "a denied fill types nothing");
    let all = format!(
        "{:?} {:?} {:?}",
        a,
        read_event_log(env.dir.path(), run).unwrap(),
        env.storage.list_steps(run).unwrap()
    );
    assert!(!all.contains(SECRET), "{all}");
    assert!(!all.contains("hunter"), "{all}");
}

#[test]
fn the_repeat_guard_still_sees_a_masked_call_as_the_same_call() {
    use crate::exec::same_call;
    let masked = tool_registry::masked_secret(SECRET, "password field");
    let stored = (
        "browser_fill".to_owned(),
        json!({"ref": "e1", "field": "textbox \"Password\"", "text": masked}),
    );
    let again = (
        "browser_fill".to_owned(),
        json!({"ref": "e1", "text": SECRET}),
    );
    assert!(same_call(&stored, &again));
    // Another field or another tool is another call.
    let other_ref = (
        "browser_fill".to_owned(),
        json!({"ref": "e7", "text": SECRET}),
    );
    assert!(!same_call(&stored, &other_ref));
    assert!(!same_call(
        &stored,
        &("browser_select".to_owned(), again.1.clone())
    ));
    // An unmasked stored call is compared exactly, as before.
    let plain = ("t".to_owned(), json!({"a": "x"}));
    assert!(same_call(&plain, &("t".to_owned(), json!({"a": "x"}))));
    assert!(!same_call(&plain, &("t".to_owned(), json!({"a": "y"}))));
    assert!(!same_call(
        &plain,
        &("t".to_owned(), json!({"a": "x", "b": 1}))
    ));
}
