//! The user's own computer as a per-request target (card 272): a permission
//! given for the bot's own computer never covers an action on the user's
//! computer, and the other way round.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use permission_engine::Policy;
use protocol::{Event, PermissionCategory};
use serde_json::json;
use task_runtime::RunState;
use tool_registry::{ContentClass, Effect};

use crate::tests::{
    MACHINE_A, calls, cfg, machine_provider, make_loop, profile, setup, text, until, with_class,
};
use crate::{AgentLoop, ApprovalScope};

fn writer(name: &str) -> Arc<crate::tests::TestTool> {
    Arc::new(with_class(
        name,
        Effect::Write,
        PermissionCategory::Fs,
        ContentClass::none(),
        "ran",
    ))
}

/// A bot that opted into the user's computer (`pc_*` in its allow list).
const PC_ON: &str = "[tools]\nallow = [\"*\", \"pc_*\"]\n[files]\nmode = \"computer\"";

fn loop_with(
    env: &crate::tests::Env,
    script: Vec<crate::tests::Reply>,
    tools: &[&Arc<crate::tests::TestTool>],
) -> (AgentLoop, Arc<crate::tests::Fake>) {
    loop_with_policy(env, script, tools, Policy::approve_destructive(), PC_ON)
}

fn loop_with_policy(
    env: &crate::tests::Env,
    script: Vec<crate::tests::Reply>,
    tools: &[&Arc<crate::tests::TestTool>],
    policy: Policy,
    extra: &str,
) -> (AgentLoop, Arc<crate::tests::Fake>) {
    let fake = crate::tests::Fake::new(script);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        tools,
        profile(extra),
        cfg(policy),
    );
    // A paired PC exists ("pc"), so the run has a user's computer identity.
    lp.set_computer_provider(machine_provider(MACHINE_A, Some("host"), Some("pc"), false));
    (lp, fake)
}

/// A turn whose message the route tagged with `source`.
fn start_from(lp: &AgentLoop, env: &crate::tests::Env, source: &'static str) -> protocol::RunId {
    match lp
        .send_user_message_from(env.chat, "go", crate::MessageMode::Steer, source)
        .unwrap()
    {
        crate::Delivery::NewTurn(r) => r,
        _ => unreachable!(),
    }
}

fn reader(name: &str) -> Arc<crate::tests::TestTool> {
    Arc::new(with_class(
        name,
        Effect::Read,
        PermissionCategory::Fs,
        ContentClass::untrusted_private(),
        "secret file text",
    ))
}

/// Answer the next card with `scope`.
async fn answer(
    lp: &AgentLoop,
    env: &crate::tests::Env,
    rx: &mut tokio::sync::broadcast::Receiver<crate::StreamItem>,
    scope: ApprovalScope,
) -> protocol::event::ApprovalNeeded {
    let Event::ApprovalNeeded(a) = until(rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };
    lp.approve(a.call_id, scope, env.user).unwrap();
    a
}

/// "Always allow file writes" given for the bot's own computer covers the
/// bot's own computer and nothing on the user's computer, and the reverse.
#[tokio::test]
async fn a_grant_for_one_computer_never_covers_the_other() {
    for host_first in [true, false] {
        let env = setup();
        let (own, theirs) = (writer("fs_write"), writer("pc_fs_write"));
        let (first, second) = if host_first {
            ("fs_write", "pc_fs_write")
        } else {
            ("pc_fs_write", "fs_write")
        };
        let (lp, _fake) = loop_with(
            &env,
            vec![
                calls(&[("c1", first, json!({"path": "a"}))]),
                calls(&[("c2", first, json!({"path": "b"}))]),
                calls(&[("c3", second, json!({"path": "c"}))]),
                text("done"),
            ],
            &[&own, &theirs],
        );
        let mut rx = lp.subscribe();
        let run = start_from(&lp, &env, "owner");
        let card = answer(&lp, &env, &mut rx, ApprovalScope::AlwaysCategory).await;
        if !host_first {
            // The card says whose computer it is about.
            assert!(card.summary.contains("your computer"), "{}", card.summary);
        }
        // c2 is covered by the grant just given; c3 is on the OTHER computer
        // and must ask again.
        let second_card = answer(&lp, &env, &mut rx, ApprovalScope::Once).await;
        assert_eq!(second_card.tool_name, second, "host_first={host_first}");
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        let (a, b) = (
            own.count.load(Ordering::SeqCst),
            theirs.count.load(Ordering::SeqCst),
        );
        assert_eq!(a + b, 3, "host_first={host_first}");
        assert_eq!(
            env.storage.list_approvals(run).unwrap().len(),
            2,
            "the second computer asked again, host_first={host_first}"
        );
    }
}

/// Review blocker 1: `pc_*` is opt-in per bot and every per-bot restriction
/// of the same tool on the bot's own computer applies to it too.
#[test]
fn pc_tools_are_hidden_unless_the_bot_explicitly_allows_them() {
    // No allow list, or a bare "*": not an opt-in.
    for extra in ["", "[tools]\nallow = [\"*\"]"] {
        let p = profile(extra).with_unset_files_denied(false);
        for t in ["pc_shell_run", "pc_fs_read", "pc_desktop_click", "pc_info"] {
            assert!(!p.tool_visible(t), "{t} visible with {extra:?}");
        }
        assert!(p.tool_visible("shell_run"));
    }
    let on = profile(PC_ON);
    for t in ["pc_shell_run", "pc_fs_read", "pc_browser_open", "pc_info"] {
        assert!(on.tool_visible(t), "{t}");
    }
    // A "Folder only" bot: no commands and no files of the user's computer.
    let folder = profile(
        "[tools]\nallow = [\"*\", \"pc_*\"]\n[files]\nmode = \"folder\"\nfolders = [\"/w\"]",
    );
    for t in [
        "pc_shell_run",
        "pc_script_run",
        "pc_fs_read",
        "pc_fs_list",
        "pc_fs_write",
    ] {
        assert!(!folder.tool_visible(t), "{t} visible to a Folder-only bot");
    }
    assert!(folder.tool_visible("pc_desktop_click"));
    // Unset file mode where unset is denied (an always-on host).
    let unset = profile("[tools]\nallow = [\"*\", \"pc_*\"]");
    assert!(!unset.tool_visible("pc_fs_read"));
    assert!(
        unset
            .with_unset_files_denied(false)
            .tool_visible("pc_fs_read")
    );
    // `exclude` names the tool without the prefix.
    let excluded = profile(
        "[tools]\nallow = [\"*\", \"pc_*\"]\nexclude = [\"shell_*\"]\n[files]\nmode = \"computer\"",
    );
    assert!(!excluded.tool_visible("pc_shell_run"));
    assert!(!excluded.tool_visible("shell_run"));
    // A narrowed (subagent) surface must name the pc tool itself.
    let narrowed = on.narrowed(Some(&["fs_read".to_owned()]), None).unwrap();
    assert!(!narrowed.tool_visible("pc_fs_read"));
}

/// Review blocker 1: a bot without the opt-in (here also "Folder only")
/// cannot even call `pc_shell_run` by name.
#[tokio::test]
async fn a_bot_without_the_opt_in_cannot_use_pc_shell_run() {
    let env = setup();
    let shell = Arc::new(with_class(
        "pc_shell_run",
        Effect::Destructive,
        PermissionCategory::Shell,
        ContentClass::all().executes(),
        "ran",
    ));
    let (lp, _fake) = loop_with_policy(
        &env,
        vec![
            calls(&[("c1", "pc_shell_run", json!({"command": "dir"}))]),
            text("done"),
        ],
        &[&shell],
        Policy::full(),
        "[tools]\nallow = [\"*\"]\n[files]\nmode = \"folder\"\nfolders = [\"/w\"]",
    );
    let run = start_from(&lp, &env, "owner");
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(
        shell.count.load(Ordering::SeqCst),
        0,
        "the command never ran"
    );
    assert_eq!(env.storage.list_approvals(run).unwrap().len(), 0);
}

/// Review blocker 1: only a run the owner started by hand reaches the user's
/// computer; a scheduled run, a Telegram message, another paired device or a
/// local API client cannot read a file there, even under Full.
#[tokio::test]
async fn a_scheduled_or_telegram_run_cannot_read_the_users_files() {
    for how in ["schedule", "telegram", "peer", "local_api"] {
        let env = setup();
        let read = reader("pc_fs_read");
        let (lp, _fake) = loop_with_policy(
            &env,
            vec![
                calls(&[("c1", "pc_fs_read", json!({"path": "C:/Users/me/a.txt"}))]),
                text("done"),
            ],
            &[&read],
            Policy::full(),
            PC_ON,
        );
        let run = if how == "schedule" {
            lp.run_turn_as(env.chat, "go", protocol::InitiatorKind::Schedule)
                .unwrap()
        } else {
            start_from(&lp, &env, how)
        };
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded, "{how}");
        assert_eq!(
            read.count.load(Ordering::SeqCst),
            0,
            "{how}: nothing was read"
        );
        assert_eq!(env.storage.list_approvals(run).unwrap().len(), 0, "{how}");
    }
}

/// Review blocker 1: a read of the user's computer asks once per run (even
/// under Full, where reads of the bot's own computer never ask), and the
/// approval does not carry over to the next run.
#[tokio::test]
async fn a_read_of_the_users_computer_asks_once_per_run() {
    let env = setup();
    let read = reader("pc_fs_read");
    let (lp, _fake) = loop_with_policy(
        &env,
        vec![
            calls(&[("c1", "pc_fs_read", json!({"path": "a"}))]),
            calls(&[("c2", "pc_fs_read", json!({"path": "b"}))]),
            text("done"),
            calls(&[("c3", "pc_fs_read", json!({"path": "c"}))]),
            text("done"),
        ],
        &[&read],
        Policy::full(),
        PC_ON,
    );
    let mut rx = lp.subscribe();
    let run = start_from(&lp, &env, "peer_admin");
    answer(&lp, &env, &mut rx, ApprovalScope::Once).await;
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        env.storage.list_approvals(run).unwrap().len(),
        1,
        "once per run"
    );
    let next = start_from(&lp, &env, "owner");
    answer(&lp, &env, &mut rx, ApprovalScope::Once).await;
    assert_eq!(lp.wait(next).await.unwrap(), RunState::Succeeded);
    assert_eq!(
        env.storage.list_approvals(next).unwrap().len(),
        1,
        "asked again"
    );
}

/// Review note 1: the Balanced mode's lane (file edits run freely on the
/// bot's own computer) never covers the user's computer: the server asks.
#[tokio::test]
async fn balanced_mode_never_skips_the_ask_for_the_users_computer() {
    let env = setup();
    let (own, theirs) = (writer("fs_write"), writer("pc_fs_write"));
    let (lp, _fake) = loop_with_policy(
        &env,
        vec![
            calls(&[("c1", "fs_write", json!({"path": "a"}))]),
            calls(&[("c2", "pc_fs_write", json!({"path": "b"}))]),
            text("done"),
        ],
        &[&own, &theirs],
        Policy::approve_destructive(),
        &format!("[permissions]\npreset = \"approve-destructive\"\n{PC_ON}"),
    );
    let mut rx = lp.subscribe();
    let run = start_from(&lp, &env, "owner");
    let card = answer(&lp, &env, &mut rx, ApprovalScope::Once).await;
    assert_eq!(
        card.tool_name, "pc_fs_write",
        "the own-computer edit ran freely"
    );
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(env.storage.list_approvals(run).unwrap().len(), 1);
}
