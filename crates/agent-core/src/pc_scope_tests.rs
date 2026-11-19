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

fn loop_with(
    env: &crate::tests::Env,
    script: Vec<crate::tests::Reply>,
    tools: &[&Arc<crate::tests::TestTool>],
) -> (AgentLoop, Arc<crate::tests::Fake>) {
    let fake = crate::tests::Fake::new(script);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        tools,
        profile(""),
        cfg(Policy::approve_destructive()),
    );
    // A paired PC exists ("pc"), so the run has a user's computer identity.
    lp.set_computer_provider(machine_provider(MACHINE_A, Some("host"), Some("pc"), false));
    (lp, fake)
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
        let run = lp.run_turn(env.chat, "go").unwrap();
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
