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
    Env, Fake, TestTool, calls, cfg, make_loop, profile, setup, t, text, until, with_class,
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
    assert!(
        asks(&env, &lp).await,
        "an unknown page never matches an allow rule"
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
    let click = browser("browser_click", ContentClass::all());
    *click.page_origin.lock() = Some("https://example.com:443".into());
    let script = vec![
        calls(&[("c1", "fs_read", json!({"path": "p.html"}))]),
        calls(&[("c2", "browser_click", json!({"ref": "e1"}))]),
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
