//! Real agent-loop regression tests for the external-agent authority boundary.

use super::*;
use crate::ScheduleWork;
use protocol::{InitiatorKind, RunId};
use uuid::Uuid;

async fn approve_once(
    lp: &AgentLoop,
    env: &Env,
    rx: &mut broadcast::Receiver<StreamItem>,
) -> RunId {
    let Event::ApprovalNeeded(a) = until(rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };
    assert!(a.summary.contains("External agent"));
    for scope in [
        ApprovalScope::Always,
        ApprovalScope::AlwaysCategory,
        ApprovalScope::ThisChat,
    ] {
        assert!(matches!(
            lp.approve(a.call_id, scope, env.user),
            Err(crate::LoopError::OnceOnly(_))
        ));
    }
    lp.approve(a.call_id, ApprovalScope::Once, env.user)
        .unwrap();
    a.run_id
}

#[tokio::test]
async fn external_agent_read_gate_precedes_full_policy_persisted_grants_and_read_shortcuts() {
    for policy in [Policy::full(), Policy::approve_destructive()] {
        let env = setup();
        let read = Arc::new(tool("fs_read", Effect::Read, "private data"));
        let fake = Fake::new(vec![
            calls(&[("a", "fs_read", json!({}))]),
            calls(&[("b", "fs_read", json!({}))]),
            text("done"),
        ]);
        let mut config = cfg(policy);
        config.read_tools_skip_ask = true;
        let lp = make_loop(&env.storage, env.bot, &fake, &[&read], profile(""), config);
        lp.sh.grants.lock().always.push(crate::Grant {
            bot_id: env.bot,
            target: GrantTarget::Category(PermissionCategory::Fs),
            granted_at_ms: 1,
            computer: None,
            inert: false,
        });
        lp.sh
            .grants
            .lock()
            .this_chat
            .insert((env.chat, "fs_read".into(), None));
        let mut rx = lp.subscribe();
        let run = lp
            .run_external(
                env.chat,
                Uuid::now_v7(),
                ScheduleWork::Prompt("read".into()),
            )
            .unwrap();
        assert_eq!(read.count.load(Ordering::SeqCst), 0);
        assert_eq!(approve_once(&lp, &env, &mut rx).await, run);
        assert_eq!(approve_once(&lp, &env, &mut rx).await, run);
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        assert_eq!(read.count.load(Ordering::SeqCst), 2);
        assert_eq!(
            lp.list_grants(env.bot).len(),
            1,
            "no new grants were minted"
        );
        assert!(
            lp.send_user_message(env.chat, "reuse", MessageMode::Steer)
                .is_err()
        );
    }
}

#[tokio::test]
async fn external_agent_hard_deny_survives_and_human_read_remains_enabled() {
    let env = setup();
    let read = Arc::new(tool("fs_read", Effect::Read, "data"));
    let fake = Fake::new(vec![calls(&[("a", "fs_read", json!({}))]), text("done")]);
    let mut policy = Policy::full();
    policy
        .categories
        .insert(PermissionCategory::Fs, Rule::new(Level::Deny));
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[&read],
        profile(""),
        cfg(policy),
    );
    let run = lp
        .run_external(
            env.chat,
            Uuid::now_v7(),
            ScheduleWork::Prompt("read".into()),
        )
        .unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 0);
    assert!(env.storage.list_approvals(run).unwrap().is_empty());
    let human = setup();
    let fake = Fake::new(vec![calls(&[("h", "fs_read", json!({}))]), text("done")]);
    let lp = make_loop(
        &human.storage,
        human.bot,
        &fake,
        &[&read],
        profile(""),
        cfg(Policy::full()),
    );
    let run = lp.run_turn(human.chat, "read").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 1);
    assert!(human.storage.list_approvals(run).unwrap().is_empty());
}

#[tokio::test]
async fn external_agent_cannot_fan_out_past_the_token_run_limit() {
    let env = setup();
    let fake = Fake::new(vec![
        calls(&[("spawn", "spawn_subagent", json!({"task":"child read"}))]),
        text("no child"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    )
    .with_subagents();
    let run = lp
        .run_external(
            env.chat,
            Uuid::now_v7(),
            ScheduleWork::Prompt("delegate".into()),
        )
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), lp.wait(run))
            .await
            .expect("external fan-out must be refused without opening an approval")
            .unwrap(),
        RunState::Succeeded
    );
    assert!(env.storage.list_child_runs(run).unwrap().is_empty());
    assert!(env.storage.list_approvals(run).unwrap().is_empty());
    assert!(tool_messages(&fake.request(1))[0].contains("Permission denied"));
    // Positive control: the same registered host subagent tool works normally.
    let human = setup();
    let fake = Fake::new(vec![
        calls(&[("spawn", "spawn_subagent", json!({"task":"child"}))]),
        text("child done"),
        text("parent done"),
    ]);
    let lp = make_loop(
        &human.storage,
        human.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    )
    .with_subagents();
    let run = lp.run_turn(human.chat, "delegate").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(human.storage.list_child_runs(run).unwrap().len(), 1);
}

#[tokio::test]
async fn external_agent_legacy_child_resume_inherits_once_only_source_from_storage() {
    let env = setup();
    let read = Arc::new(tool("fs_read", Effect::Read, "private"));
    let fake = Fake::new(vec![
        calls(&[("read", "fs_read", json!({}))]),
        text("child done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[&read],
        profile(""),
        cfg(Policy::full()),
    )
    .with_subagents();
    let mut rx = lp.subscribe();
    let run = lp
        .runtime()
        .create_run(
            env.user,
            env.bot,
            Some(env.chat),
            &format!("turn:external_agent:{}", Uuid::now_v7()),
        )
        .unwrap();
    let child = lp.runtime().create_child_run(env.user,env.bot,Some(env.chat),"turn:subagent",Some(run),
        Some(json!({"task":"legacy child read","context":"","tools":null,"model_role":null,"max_steps":15,"token_budget":10000,"depth":1,"spawn_key":"legacy"}))).unwrap();
    env.storage.update_run_state(child, "paused", None).unwrap();
    lp.resume(child).unwrap();
    assert_eq!(approve_once(&lp, &env, &mut rx).await, child);
    assert_ne!(child, run);
    assert_eq!(lp.wait(child).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 1);
    let child_row = env.storage.get_run(child).unwrap().unwrap();
    lp.sh.runs.lock().clear();
    assert_eq!(
        lp.sh.ctx(&child_row).unwrap().initiator,
        InitiatorKind::ExternalAgent
    );
    assert!(lp.list_grants(env.bot).is_empty());
}

#[tokio::test]
async fn external_agent_resume_keeps_once_only_gate_and_untrusted_input() {
    let env = setup();
    let read = Arc::new(tool("fs_read", Effect::Read, "private"));
    let fake = Fake::new(vec![
        Reply::Hang,
        calls(&[("read", "fs_read", json!({}))]),
        text("done"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[&read],
        profile(""),
        cfg(Policy::full()),
    );
    let mut rx = lp.subscribe();
    let run = lp
        .run_external(
            env.chat,
            Uuid::now_v7(),
            ScheduleWork::Prompt("external-source-marker".into()),
        )
        .unwrap();
    while fake.n_requests() == 0 {
        tokio::task::yield_now().await;
    }
    assert!(lp.runtime().request_pause(run, true));
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
    lp.sh.runs.lock().clear();
    lp.resume(run).unwrap();
    assert_eq!(approve_once(&lp, &env, &mut rx).await, run);
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 1);
    assert!(fake.request(1).messages.iter().any(|m| {
        m.role == MessageRole::User
            && m.content
                .as_str()
                .is_some_and(|s| s.contains("UNTRUSTED") && s.contains("external-source-marker"))
    }));
}

#[tokio::test]
async fn external_agent_native_cli_primary_and_fallback_are_refused_before_launch() {
    for fallback in [false, true] {
        let env = setup();
        let fake = Fake::new(if fallback {
            vec![Reply::Fail("unsupported")]
        } else {
            vec![]
        });
        let mut prof = profile("");
        if fallback {
            prof.models.fallback = vec!["cli:claude".into()];
        } else {
            prof.models.executor = "cli:claude".into();
        }
        let lp = make_loop(&env.storage, env.bot, &fake, &[], prof, cfg(Policy::full()));
        let run = lp
            .run_external(
                env.chat,
                Uuid::now_v7(),
                ScheduleWork::Prompt("no native escape".into()),
            )
            .unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Failed);
        let record = env.storage.get_run(run).unwrap().unwrap();
        assert!(
            record
                .error
                .unwrap()
                .contains("native CLI sessions are not available to external agents")
        );
        assert_eq!(fake.n_requests(), usize::from(fallback));
    }
}

#[tokio::test]
async fn external_agent_direct_tool_keeps_the_existing_control_gate() {
    for refuse in [false, true] {
        let env = setup();
        let fake = Fake::new(vec![]);
        let lp = gate_loop(&env, &fake);
        let gate = Arc::new(TestGate {
            refuse,
            seen: Mutex::new(Vec::new()),
        });
        lp.set_control_gate(gate.clone());
        let mut rx = lp.subscribe();
        let run = lp
            .run_external(
                env.chat,
                Uuid::now_v7(),
                ScheduleWork::Tool {
                    name: "t.desk".into(),
                    input: json!({}),
                },
            )
            .unwrap();
        assert_eq!(approve_once(&lp, &env, &mut rx).await, run);
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        let seen = gate.seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].run_id, run);
        assert_eq!(seen[0].bot_id, env.bot);
        let steps = env.storage.list_steps(run).unwrap();
        let result = steps
            .iter()
            .find(|s| s.name == "tools")
            .unwrap()
            .output
            .as_ref()
            .unwrap();
        assert_eq!(result["results"][0]["is_error"], refuse);
    }
}

#[tokio::test]
async fn external_agent_entry_points_cannot_reuse_prior_human_history() {
    let env = setup();
    let fake = Fake::new(vec![text("trusted private conversation")]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    let run = lp.run_turn(env.chat, "human history").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert!(
        lp.run_turn_as(env.chat, "external", InitiatorKind::ExternalAgent)
            .is_err()
    );
    assert!(
        lp.run_external(
            env.chat,
            Uuid::now_v7(),
            ScheduleWork::Prompt("external".into())
        )
        .is_err()
    );
    assert_eq!(env.storage.list_runs(env.user).unwrap().len(), 1);
}
