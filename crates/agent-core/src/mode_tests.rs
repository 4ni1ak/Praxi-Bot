//! Autonomy modes: what a Full, Balanced and Ask bot is still asked about.

#![allow(clippy::assert_is_empty)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use permission_engine::Policy;
use protocol::{Event, PermissionCategory};
use serde_json::json;
use task_runtime::RunState;
use tool_registry::{ContentClass, Effect};

use crate::tests::{
    Fake, TestTool, calls, cfg, make_loop, profile, setup, t, text, until, with_class,
};

const BALANCED: &str = "[permissions]\npreset = \"approve-destructive\"\n";
const ASK: &str = "[permissions]\npreset = \"read-and-draft\"\n";
const FULL: &str = "[permissions]\npreset = \"full\"\n";

fn tool(name: &str, effect: Effect, category: PermissionCategory) -> Arc<TestTool> {
    Arc::new(with_class(
        name,
        effect,
        category,
        ContentClass::none(),
        "ok",
    ))
}

/// Run one call of `tool` and report whether it asked.
async fn asks(extra: &str, tool: &Arc<TestTool>) -> bool {
    let env = setup();
    let fake = Fake::new(vec![
        calls(&[("c1", &tool.spec.name, json!({"path": "a.txt"}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(tool)],
        profile(extra),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "go").unwrap();
    let ran = tokio::select! {
        _ = until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))) => false,
        state = lp.wait(run) => {
            assert_eq!(state.unwrap(), RunState::Succeeded);
            true
        }
    };
    if !ran {
        lp.cancel(run).unwrap();
        assert_eq!(tool.count.load(Ordering::SeqCst), 0);
    }
    !ran
}

#[tokio::test]
async fn balanced_edits_files_without_asking_and_audits_why() {
    let env = setup();
    let write = tool("fs_write", Effect::Write, PermissionCategory::Fs);
    let fake = Fake::new(vec![
        calls(&[("c1", "fs_write", json!({"path": "a.txt"}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&write)],
        profile(BALANCED),
        cfg(Policy::approve_destructive()),
    );
    let run = lp.run_turn(env.chat, "go").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(write.count.load(Ordering::SeqCst), 1);
    assert!(env.storage.list_approvals(run).unwrap().is_empty());
    let audit = env.storage.list_audit(env.user).unwrap();
    let entry = audit
        .iter()
        .find(|e| e.action == "approval.auto_allowed")
        .expect("the lane is audited");
    assert_eq!(entry.detail["reason"], "mode.balanced.workspace_edit");
    assert_eq!(entry.detail["mode"], "balanced");
}

#[tokio::test]
async fn ask_mode_still_asks_for_the_same_edit() {
    let write = tool("fs_write", Effect::Write, PermissionCategory::Fs);
    assert!(asks(ASK, &write).await);
}

#[tokio::test]
async fn balanced_still_asks_for_deletes_and_sends() {
    let delete = tool("fs_delete", Effect::Destructive, PermissionCategory::Fs);
    assert!(asks(BALANCED, &delete).await);
    let send = tool(
        "email_send",
        Effect::Destructive,
        PermissionCategory::Messaging,
    );
    assert!(asks(BALANCED, &send).await);
}

#[tokio::test]
async fn full_runs_edits_but_money_is_a_hard_stop() {
    let write = tool("fs_write", Effect::Write, PermissionCategory::Fs);
    assert!(!asks(FULL, &write).await);
    let pay = tool("pay", Effect::Destructive, PermissionCategory::Money);
    assert!(asks(FULL, &pay).await);
}

#[tokio::test]
async fn a_tainted_balanced_run_still_asks_before_sending_data_out() {
    let env = setup();
    let page = Arc::new(with_class(
        "web_read",
        Effect::Read,
        PermissionCategory::Browser,
        ContentClass::untrusted_private(),
        "page text",
    ));
    let post = Arc::new(with_class(
        "browser_post",
        Effect::Write,
        PermissionCategory::Browser,
        ContentClass::untrusted_egress(),
        "sent",
    ));
    let fake = Fake::new(vec![
        calls(&[("c1", "web_read", json!({"url": "https://a.example"}))]),
        calls(&[("c2", "browser_post", json!({"url": "https://b.example"}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&page), t(&post)],
        profile(BALANCED),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "go").unwrap();
    until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await;
    assert_eq!(post.count.load(Ordering::SeqCst), 0);
    lp.cancel(run).unwrap();
}
