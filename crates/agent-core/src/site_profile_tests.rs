//! Per-site / per-app permission rules in the decision pipeline (issue #227).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use permission_engine::profiles::NewRule;
use permission_engine::{Capability, Policy, ProfileAction, SubjectKind};
use protocol::{Event, PermissionCategory};
use serde_json::json;
use task_runtime::RunState;
use tool_registry::{ContentClass, Effect};

use crate::AgentLoop;
use crate::tests::{
    Env, Fake, TestTool, calls, cfg, make_loop, profile, setup, t, text, tool_messages, until,
    with_class,
};

const ASK: &str = "[permissions]\npreset = \"read-and-draft\"\n";

fn browser(name: &str, class: ContentClass) -> Arc<TestTool> {
    Arc::new(with_class(
        name,
        Effect::Write,
        PermissionCategory::Browser,
        class,
        "ok",
    ))
}

fn rule(key: &str, sub: bool, capability: Capability, action: ProfileAction) -> NewRule<'_> {
    NewRule {
        kind: SubjectKind::Site,
        raw_key: key,
        include_subdomains: sub,
        capability,
        action,
        expires_at_ms: None,
    }
}

/// Run one turn; true when it stopped at an approval card.
async fn asks(env: &Env, lp: &AgentLoop) -> bool {
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "go").unwrap();
    let done = lp.wait(run);
    tokio::pin!(done);
    tokio::select! {
        _ = until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))) => {
            lp.cancel(run).unwrap();
            let _ = done.await;
            true
        }
        state = &mut done => {
            assert_eq!(state.unwrap(), RunState::Succeeded);
            false
        }
    }
}

fn one_call(id: &str, tool: &str, input: &serde_json::Value) -> Vec<crate::tests::Reply> {
    vec![calls(&[(id, tool, input.clone())]), text("ok")]
}

#[tokio::test]
async fn a_block_rule_refuses_that_site_even_at_full_and_not_a_lookalike_host() {
    let env = setup();
    let open = browser("browser_open", ContentClass::none());
    // The browser reports a readable page after each call (an unreadable one is withheld).
    *open.page_origin.lock() = Some("https://evil.test:443".into());
    let urls = [
        "https://www.example.com/a", // subdomain of the blocked domain: refused
        "https://example.com@evil.test/", // userinfo trick: the host is evil.test
        "https://example.com.evil.test/", // label confusion
        "https://notexample.com/",   // suffix confusion
        "https://EXAMPLE.com./x",    // case and trailing dot: refused
    ];
    let script = urls
        .iter()
        .enumerate()
        .flat_map(|(i, u)| one_call(&format!("c{i}"), "browser_open", &json!({ "url": u })))
        .collect::<Vec<_>>();
    let fake = Fake::new(script);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&open)],
        profile(""),
        cfg(Policy::full()),
    );
    lp.add_profile_rule(
        env.bot,
        &rule("example.com", true, Capability::Any, ProfileAction::Block),
    )
    .unwrap();
    let mut ran = Vec::new();
    for _ in &urls {
        let before = open.count.load(Ordering::SeqCst);
        assert!(!asks(&env, &lp).await);
        ran.push(open.count.load(Ordering::SeqCst) - before);
    }
    assert_eq!(
        ran,
        vec![0, 1, 1, 1, 0],
        "only the real example.com hosts are blocked"
    );

    // The rule is durable and audited, and so is each time it decided.
    let lp2 = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(vec![]),
        &[t(&open)],
        profile(""),
        cfg(Policy::full()),
    );
    assert_eq!(lp2.list_profile_rules(env.bot).len(), 1);
    let log = env.storage.list_audit(env.user).unwrap();
    let count = |a: &str| log.iter().filter(|e| e.action == a).count();
    assert_eq!(count("profile_rule.set"), 1);
    assert_eq!(count("profile_rule.applied"), 2);
    let id = lp2.list_profile_rules(env.bot)[0].id.clone();
    assert!(lp2.remove_profile_rule(env.bot, &id).unwrap());
    assert_eq!(lp2.list_profile_rules(env.bot).len(), 0);
}

#[tokio::test]
async fn an_ask_rule_beats_full_and_an_allow_rule_answers_only_its_own_site_and_class() {
    let env = setup();
    let click = browser("browser_click", ContentClass::none());
    *click.page_origin.lock() = Some("https://example.com:443".into());
    let input = json!({"ref": "e1"});
    let mk = |policy: Policy, extra: &str| {
        let script = (0..8)
            .flat_map(|i| one_call(&format!("c{i}"), "browser_click", &input))
            .collect();
        make_loop(
            &env.storage,
            env.bot,
            &Fake::new(script),
            &[t(&click)],
            profile(extra),
            cfg(policy),
        )
    };
    // Full level: nothing asks until the owner says "ask on this site".
    let full = mk(Policy::full(), "");
    assert!(!asks(&env, &full).await);
    let ask_rule = full
        .add_profile_rule(
            env.bot,
            &rule(
                "example.com",
                false,
                Capability::Interact,
                ProfileAction::Ask,
            ),
        )
        .unwrap();
    assert!(asks(&env, &full).await, "an ask rule is stricter than Full");
    full.remove_profile_rule(env.bot, &ask_rule.id).unwrap();

    // Asking level: an allow rule for this site and class answers; another site does not.
    let lp = mk(Policy::approve_destructive(), ASK);
    assert!(asks(&env, &lp).await);
    lp.add_profile_rule(
        env.bot,
        &rule(
            "example.com",
            false,
            Capability::Interact,
            ProfileAction::Allow,
        ),
    )
    .unwrap();
    assert!(
        !asks(&env, &lp).await,
        "the owner's allow rule answers the plain ask"
    );
    *click.page_origin.lock() = Some("https://other.test:443".into());
    assert!(asks(&env, &lp).await, "another site still asks");
    *click.page_origin.lock() = None;
    let before = click.count.load(Ordering::SeqCst);
    let _ = asks(&env, &lp).await;
    assert_eq!(
        click.count.load(Ordering::SeqCst),
        before,
        "an unknown page never matches an allow rule: the click does not run"
    );
}

#[tokio::test]
async fn an_allow_rule_does_not_waive_the_untrusted_content_gate() {
    let env = setup();
    let read = Arc::new(with_class(
        "fs_read",
        Effect::Read,
        PermissionCategory::Fs,
        ContentClass::untrusted_private(),
        "page",
    ));
    let click = browser("browser_fill", ContentClass::all());
    *click.page_origin.lock() = Some("https://example.com:443".into());
    let script = vec![
        calls(&[("c1", "fs_read", json!({"path": "p.html"}))]),
        calls(&[("c2", "browser_fill", json!({"ref": "e1", "text": "hello"}))]),
        text("ok"),
    ];
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(script),
        &[t(&read), t(&click)],
        profile(""),
        cfg(Policy::full()),
    );
    lp.add_profile_rule(
        env.bot,
        &rule(
            "example.com",
            false,
            Capability::Interact,
            ProfileAction::Allow,
        ),
    )
    .unwrap();
    assert!(
        asks(&env, &lp).await,
        "after reading untrusted content the click still needs a card"
    );
    assert_eq!(click.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_rule_is_validated_when_added_and_stops_applying_when_it_expires() {
    let env = setup();
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(vec![]),
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    let bad = [
        rule(
            "https://example.com",
            false,
            Capability::Read,
            ProfileAction::Block,
        ),
        rule("example.com", true, Capability::Read, ProfileAction::Allow),
        rule("example.com", false, Capability::Any, ProfileAction::Allow),
    ];
    for b in &bad {
        assert!(lp.add_profile_rule(env.bot, b).is_err(), "{}", b.raw_key);
    }
    assert_eq!(lp.list_profile_rules(env.bot).len(), 0);
    let mut soon = rule("example.com", false, Capability::Any, ProfileAction::Block);
    soon.expires_at_ms = Some(storage::now_ms() + 20);
    lp.add_profile_rule(env.bot, &soon).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(60));
    let open = browser("browser_open", ContentClass::none());
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(one_call(
            "c1",
            "browser_open",
            &json!({"url": "https://example.com/"}),
        )),
        &[t(&open)],
        profile(""),
        cfg(Policy::full()),
    );
    assert!(!asks(&env, &lp).await);
    assert_eq!(
        open.count.load(Ordering::SeqCst),
        1,
        "the expired block no longer applies"
    );
}

fn app_rule(path: &str, capability: Capability, action: ProfileAction) -> NewRule<'_> {
    NewRule {
        kind: SubjectKind::App,
        raw_key: path,
        include_subdomains: false,
        capability,
        action,
        expires_at_ms: None,
    }
}

#[tokio::test]
async fn a_site_block_also_covers_reading_the_page_and_going_back() {
    let env = setup();
    for name in ["browser_snapshot", "browser_screenshot", "browser_back"] {
        let tool = browser(name, ContentClass::none());
        *tool.page_origin.lock() = Some("https://bank.example:443".into());
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call("c1", name, &json!({}))),
            &[t(&tool)],
            profile(""),
            cfg(Policy::full()),
        );
        lp.add_profile_rule(
            env.bot,
            &rule("bank.example", false, Capability::Any, ProfileAction::Block),
        )
        .unwrap();
        assert!(!asks(&env, &lp).await);
        assert_eq!(
            tool.count.load(Ordering::SeqCst),
            0,
            "{name} on a blocked page must not run"
        );
        // Another page is not affected.
        *tool.page_origin.lock() = Some("https://news.example:443".into());
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call("c1", name, &json!({}))),
            &[t(&tool)],
            profile(""),
            cfg(Policy::full()),
        );
        assert!(!asks(&env, &lp).await);
        assert_eq!(
            tool.count.load(Ordering::SeqCst),
            1,
            "{name} elsewhere runs"
        );
        for r in lp.list_profile_rules(env.bot) {
            lp.remove_profile_rule(env.bot, &r.id).unwrap();
        }
    }
}

#[tokio::test]
async fn a_redirect_onto_a_blocked_site_withholds_the_page_from_the_model() {
    let env = setup();
    let open = Arc::new(with_class(
        "browser_open",
        Effect::Write,
        PermissionCategory::Browser,
        ContentClass::none(),
        "SECRET BANK PAGE",
    ));
    // The address is allowed, but the browser ends up on the blocked site.
    *open.page_origin.lock() = Some("https://bank.example:443".into());
    let fake = Fake::new(one_call(
        "c1",
        "browser_open",
        &json!({"url": "https://short.example/go"}),
    ));
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&open)],
        profile(""),
        cfg(Policy::full()),
    );
    lp.add_profile_rule(
        env.bot,
        &rule("bank.example", false, Capability::Any, ProfileAction::Block),
    )
    .unwrap();
    assert!(!asks(&env, &lp).await);
    let seen = tool_messages(&fake.request(1)).join(
        "
",
    );
    assert!(!seen.contains("SECRET BANK PAGE"), "leaked: {seen}");
    assert!(seen.contains("blocked"), "the model is told why: {seen}");
}

#[tokio::test]
async fn a_pending_approval_is_denied_when_a_block_for_it_is_added() {
    let env = setup();
    let click = browser("browser_click", ContentClass::none());
    *click.page_origin.lock() = Some("https://example.com:443".into());
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(one_call("c1", "browser_click", &json!({"ref": "e1"}))),
        &[t(&click)],
        profile(ASK),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "go").unwrap();
    until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await;
    // The page moves on, then the owner blocks the site the card was shown for.
    *click.page_origin.lock() = Some("https://other.test:443".into());
    lp.add_profile_rule(
        env.bot,
        &rule("example.com", false, Capability::Any, ProfileAction::Block),
    )
    .unwrap();
    let state = tokio::time::timeout(std::time::Duration::from_secs(10), lp.wait(run))
        .await
        .expect("the open card was withdrawn without a click")
        .unwrap();
    assert_eq!(state, RunState::Succeeded);
    assert_eq!(click.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_app_block_or_ask_fails_closed_when_the_program_is_not_identified() {
    use crate::control_grant_tests::ControlTool;
    let env = setup();
    let path = r"C:\Windows\System32\notepad.exe";
    for (action, expect_ask) in [(ProfileAction::Block, false), (ProfileAction::Ask, true)] {
        let typed = ControlTool::new("desktop_type", vec![None]);
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call("c1", "desktop_type", &json!({"text": "hi"}))),
            &[],
            profile("[runtime]\ncomputer = \"host\"\n"),
            cfg(Policy::full()),
        );
        lp.sh
            .tools
            .register(typed.clone() as Arc<dyn tool_registry::Tool>);
        lp.set_computer_provider(crate::tests::machine_provider(
            crate::tests::MACHINE_A,
            Some("host"),
            None,
            false,
        ));
        lp.add_profile_rule(env.bot, &app_rule(path, Capability::Any, action))
            .unwrap();
        assert_eq!(asks(&env, &lp).await, expect_ask, "{action:?}");
        assert_eq!(
            typed.count.load(Ordering::SeqCst),
            0,
            "{action:?}: the call must not run"
        );
        for r in lp.list_profile_rules(env.bot) {
            lp.remove_profile_rule(env.bot, &r.id).unwrap();
        }
    }
}

// ---- review of #880: every page tool, web fetches, fail-closed landing, program keys ----

/// The names of the real built-in browser tools (the registry the model is given).
fn registry_browser_names() -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let tools = builtin_tools::browser::managed_tools(
        builtin_tools::browser::BrowserSettings::isolated(dir.path()),
        dir.path(),
        None,
        None,
    );
    let names: Vec<String> = tools
        .iter()
        .map(|t| t.spec().name.clone())
        .filter(|n| n.starts_with("browser_"))
        .collect();
    assert!(
        names.len() >= 15,
        "the browser registry looks empty: {names:?}"
    );
    names
}

#[test]
fn every_tool_of_the_real_browser_registry_has_a_site_rule_class() {
    for name in registry_browser_names() {
        assert!(
            crate::site_profiles::BROWSER_CLASSES
                .iter()
                .any(|(n, _)| *n == name),
            "{name} is a browser tool but has no class in site_profiles::BROWSER_CLASSES: a \
             site block would not cover it. Add it there."
        );
        assert!(crate::site_profiles::classify(&name).is_some(), "{name}");
        assert!(
            name == "browser_open" || crate::site_profiles::is_page_judged(&name),
            "{name} acts on the open page, so it must be judged by the page's site"
        );
    }
}

#[tokio::test]
async fn a_site_block_stops_every_browser_tool_on_that_page_and_never_turns_into_a_card() {
    let env = setup();
    for name in registry_browser_names() {
        let tool = browser(&name, ContentClass::none());
        *tool.page_origin.lock() = Some("https://bank.example:443".into());
        let input = if name == "browser_open" {
            json!({"url": "https://bank.example/"})
        } else {
            json!({})
        };
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call("c1", &name, &input)),
            &[t(&tool)],
            profile(""),
            cfg(Policy::full()),
        );
        lp.add_profile_rule(
            env.bot,
            &rule("bank.example", false, Capability::Any, ProfileAction::Block),
        )
        .unwrap();
        assert!(
            !asks(&env, &lp).await,
            "{name}: a block is a deny, not a card"
        );
        assert_eq!(
            tool.count.load(Ordering::SeqCst),
            0,
            "{name} must not run on a blocked page"
        );
        for r in lp.list_profile_rules(env.bot) {
            lp.remove_profile_rule(env.bot, &r.id).unwrap();
        }
    }
}

#[tokio::test]
async fn a_browser_tool_nobody_classified_is_judged_by_every_class() {
    let env = setup();
    let tool = browser("browser_future_thing", ContentClass::none());
    *tool.page_origin.lock() = Some("https://bank.example:443".into());
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(one_call("c1", "browser_future_thing", &json!({}))),
        &[t(&tool)],
        profile(""),
        cfg(Policy::full()),
    );
    // Only reading is blocked: the unlisted tool could read, so it is stopped anyway.
    lp.add_profile_rule(
        env.bot,
        &rule(
            "bank.example",
            false,
            Capability::Read,
            ProfileAction::Block,
        ),
    )
    .unwrap();
    assert!(!asks(&env, &lp).await);
    assert_eq!(tool.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_site_block_covers_the_web_fetch_tools_by_the_address_asked_for() {
    let env = setup();
    for name in [
        "web_extract",
        "web_feed",
        "web_sitemap",
        "web_metadata",
        "web_scrape",
    ] {
        let tool = browser(name, ContentClass::none());
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call(
                "c1",
                name,
                &json!({"url": "https://www.bank.example/x"}),
            )),
            &[t(&tool)],
            profile(""),
            cfg(Policy::full()),
        );
        lp.add_profile_rule(
            env.bot,
            &rule("bank.example", true, Capability::Read, ProfileAction::Block),
        )
        .unwrap();
        assert!(!asks(&env, &lp).await, "{name}");
        assert_eq!(tool.count.load(Ordering::SeqCst), 0, "{name}: blocked site");
        // An unrelated address still runs.
        let lp = make_loop(
            &env.storage,
            env.bot,
            &Fake::new(one_call(
                "c1",
                name,
                &json!({"url": "https://news.example/x"}),
            )),
            &[t(&tool)],
            profile(""),
            cfg(Policy::full()),
        );
        assert!(!asks(&env, &lp).await, "{name}");
        assert_eq!(tool.count.load(Ordering::SeqCst), 1, "{name}: other site");
        for r in lp.list_profile_rules(env.bot) {
            lp.remove_profile_rule(env.bot, &r.id).unwrap();
        }
    }
}

/// A web tool that asks the guard the loop installed about a few addresses.
struct HopProbe {
    spec: tool_registry::ToolSpec,
}

#[async_trait::async_trait]
impl tool_registry::Tool for HopProbe {
    fn spec(&self) -> &tool_registry::ToolSpec {
        &self.spec
    }
    async fn call(
        &self,
        _: &tool_registry::ToolContext,
        _: serde_json::Value,
    ) -> Result<tool_registry::ToolOutput, tool_registry::ToolError> {
        let asked = tool_registry::url_blocked("https://news.example/x", false);
        let hop_bank = tool_registry::url_blocked("https://bank.example/login", true);
        let first_bank = tool_registry::url_blocked("https://bank.example/login", false);
        let hop_ask = tool_registry::url_blocked("https://ask.example/", true);
        let first_ask = tool_registry::url_blocked("https://ask.example/", false);
        let hop_ok = tool_registry::url_blocked("https://other.example/", true);
        Ok(tool_registry::ToolOutput::text(format!(
            "asked={asked} hop_bank={hop_bank} first_bank={first_bank} hop_ask={hop_ask} \
             first_ask={first_ask} hop_ok={hop_ok}"
        )))
    }
}

#[tokio::test]
async fn a_site_block_also_holds_for_every_redirect_hop_of_a_web_fetch() {
    let env = setup();
    let probe = Arc::new(HopProbe {
        spec: with_class(
            "web_extract",
            Effect::Write,
            PermissionCategory::Browser,
            ContentClass::none(),
            "",
        )
        .spec,
    });
    let fake = Fake::new(one_call(
        "c1",
        "web_extract",
        &json!({"url": "https://news.example/x"}),
    ));
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    lp.sh.tools.register(probe as Arc<dyn tool_registry::Tool>);
    lp.add_profile_rule(
        env.bot,
        &rule("bank.example", false, Capability::Any, ProfileAction::Block),
    )
    .unwrap();
    // An ask rule is put to the owner for the address asked for; a redirect target cannot be
    // put to anyone mid-call, so it is refused.
    lp.add_profile_rule(
        env.bot,
        &rule("ask.example", false, Capability::Read, ProfileAction::Ask),
    )
    .unwrap();
    assert!(!asks(&env, &lp).await, "the address asked for is fine");
    let seen = tool_messages(&fake.request(1)).join("\n");
    assert!(
        seen.contains(
            "asked=false hop_bank=true first_bank=true hop_ask=true first_ask=false hop_ok=false"
        ),
        "{seen}"
    );
}

/// A browser tool whose page moves: it reports `before` when asked where the page is, `landed`
/// after it ran, and its call returns `result`.
struct MovingPage {
    spec: tool_registry::ToolSpec,
    before: Option<String>,
    landed: Option<String>,
    result: Result<String, String>,
}

#[async_trait::async_trait]
impl tool_registry::Tool for MovingPage {
    fn spec(&self) -> &tool_registry::ToolSpec {
        &self.spec
    }
    async fn current_origin(
        &self,
        _: &tool_registry::ToolContext,
        _: &serde_json::Value,
    ) -> Option<String> {
        self.before.clone()
    }
    async fn landed_origin(&self, _: &tool_registry::ToolContext) -> Option<String> {
        self.landed.clone()
    }
    async fn call(
        &self,
        _: &tool_registry::ToolContext,
        _: serde_json::Value,
    ) -> Result<tool_registry::ToolOutput, tool_registry::ToolError> {
        match &self.result {
            Ok(t) => Ok(tool_registry::ToolOutput::text(t.clone())),
            Err(e) => Err(tool_registry::ToolError::Execution(e.clone())),
        }
    }
}

async fn moving_page_says(
    env: &Env,
    tool: &str,
    input: &serde_json::Value,
    before: Option<&str>,
    landed: Option<&str>,
    result: Result<&str, &str>,
) -> String {
    let page = Arc::new(MovingPage {
        spec: with_class(
            tool,
            Effect::Write,
            PermissionCategory::Browser,
            ContentClass::none(),
            "",
        )
        .spec,
        before: before.map(str::to_owned),
        landed: landed.map(str::to_owned),
        result: result.map(str::to_owned).map_err(str::to_owned),
    });
    let fake = Fake::new(one_call("c1", tool, input));
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    lp.sh.tools.register(page as Arc<dyn tool_registry::Tool>);
    lp.add_profile_rule(
        env.bot,
        &rule("bank.example", false, Capability::Any, ProfileAction::Block),
    )
    .unwrap();
    assert!(!asks(env, &lp).await);
    tool_messages(&fake.request(1)).join("\n")
}

#[tokio::test]
async fn the_landing_check_fails_closed_when_the_page_origin_cannot_be_verified() {
    let env = setup();
    let url = json!({"url": "https://news.example/x"});
    // The browser reports nothing, or a page that is not http(s): nothing may pass.
    for landed in [
        None,
        Some("blob:https://bank.example/uuid"),
        Some("data:text/html,x"),
        Some("about:blank"),
    ] {
        let seen = moving_page_says(
            &env,
            "browser_open",
            &url,
            None,
            landed,
            Ok("SECRET PAGE TEXT"),
        )
        .await;
        assert!(
            !seen.contains("SECRET PAGE TEXT"),
            "{landed:?} leaked: {seen}"
        );
        assert!(seen.contains("could not be verified"), "{landed:?}: {seen}");
    }
    // A readable, unblocked page still passes.
    let seen = moving_page_says(
        &env,
        "browser_open",
        &url,
        None,
        Some("https://news.example:443"),
        Ok("FINE PAGE TEXT"),
    )
    .await;
    assert!(seen.contains("FINE PAGE TEXT"), "{seen}");
}

#[tokio::test]
async fn an_error_from_a_call_that_ended_on_a_blocked_page_does_not_leak_page_content() {
    let env = setup();
    let click = json!({"ref": "e1"});
    for landed in [Some("https://bank.example:443"), None] {
        let seen = moving_page_says(
            &env,
            "browser_click",
            &click,
            Some("https://news.example:443"),
            landed,
            Err("no element e1; the page says: SECRET ACCOUNT NUMBER"),
        )
        .await;
        assert!(
            !seen.contains("SECRET ACCOUNT NUMBER"),
            "{landed:?}: {seen}"
        );
        assert!(seen.contains("withheld"), "{landed:?}: {seen}");
    }
}

fn windows_target(path: &str) -> tool_registry::ControlTarget {
    tool_registry::ControlTarget {
        kind: tool_registry::ControlKind::Type,
        identity: tool_registry::ControlIdentity {
            image_path: path.to_owned(),
            pid: 10,
            start_time: 99,
        },
        window_title: "Untitled".into(),
        refusal: None,
    }
}

/// Add a program rule with `key`, then type into a program whose OS image is `image`.
/// `Err`: the rule was refused. `Ok(true)`: the call ran.
async fn typing_with_app_rule(
    env: &Env,
    key: &str,
    action: ProfileAction,
    image: &str,
) -> Result<bool, String> {
    use crate::control_grant_tests::ControlTool;
    let typed = ControlTool::new("desktop_type", vec![Some(windows_target(image))]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &Fake::new(one_call("c1", "desktop_type", &json!({"text": "hi"}))),
        &[],
        profile("[runtime]\ncomputer = \"host\"\n"),
        cfg(Policy::full()),
    );
    lp.sh
        .tools
        .register(typed.clone() as Arc<dyn tool_registry::Tool>);
    lp.set_computer_provider(crate::tests::machine_provider(
        crate::tests::MACHINE_A,
        Some("host"),
        None,
        false,
    ));
    lp.add_profile_rule(env.bot, &app_rule(key, Capability::Any, action))
        .map_err(|e| e.to_string())?;
    let _ = asks(env, &lp).await;
    Ok(typed.count.load(Ordering::SeqCst) > 0)
}

#[tokio::test]
async fn an_app_block_holds_however_the_program_path_was_typed() {
    let env = setup();
    let image = r"C:\Windows\System32\notepad.exe";
    for key in [
        r"C:\Windows\System32\notepad",         // no extension
        r"c:\WINDOWS\system32\NOTEPAD.EXE",     // case
        r#""C:\Windows\System32\notepad.exe""#, // quotes
        r"  C:/Windows/System32/notepad.exe  ", // spaces and slashes
        r"C:\Windows\System32\notepad.exe.",    // trailing dot Windows ignores
    ] {
        assert_eq!(
            typing_with_app_rule(&env, key, ProfileAction::Block, image).await,
            Ok(false),
            "a block typed as {key:?} must stop notepad"
        );
    }
    // A different program is not caught by it.
    assert_eq!(
        typing_with_app_rule(
            &env,
            r"C:\Windows\System32\notepad",
            ProfileAction::Block,
            r"C:\Windows\System32\calc.exe"
        )
        .await,
        Ok(true)
    );
}

#[tokio::test]
async fn an_app_rule_that_can_never_match_is_refused_instead_of_stored() {
    let env = setup();
    for key in [
        r"C:\PROGRA~1\NoSuchVendor\x.exe", // 8.3 short name this computer cannot expand
        r"C:\Users\NoSuchUser\run.bat",    // a script is never the program image
        r"C:\Users\NoSuchUser\run.cmd",
        "notepad.exe", // not absolute
    ] {
        let r = typing_with_app_rule(
            &env,
            key,
            ProfileAction::Block,
            r"C:\Windows\System32\notepad.exe",
        )
        .await;
        assert!(r.is_err(), "{key} must be refused, got {r:?}");
    }
}

/// A directory junction (Windows) or symlink (elsewhere) `link` pointing at `real`.
fn link_dir(real: &std::path::Path, link: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(real)
            .output()
            .is_ok_and(|o| o.status.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(real, link).is_ok()
    }
}

#[tokio::test]
async fn an_app_block_holds_through_a_junction_in_the_rule_or_in_the_program_path() {
    let env = setup();
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let exe = if cfg!(windows) { "app.exe" } else { "app" };
    std::fs::write(real.join(exe), b"x").unwrap();
    let link = dir.path().join("link");
    if !link_dir(&real, &link) {
        eprintln!("cannot create a junction or symlink here; skipping");
        return;
    }
    let real_path = real.join(exe).to_string_lossy().into_owned();
    let link_path = link.join(exe).to_string_lossy().into_owned();
    // Rule through the junction, the OS reports the real path.
    assert_eq!(
        typing_with_app_rule(&env, &link_path, ProfileAction::Block, &real_path).await,
        Ok(false),
        "a rule typed through a junction must stop the real program"
    );
    // Rule on the real path, the program is reached through the junction.
    assert_eq!(
        typing_with_app_rule(&env, &real_path, ProfileAction::Block, &link_path).await,
        Ok(false),
        "a program path through a junction must still match the rule"
    );
}
