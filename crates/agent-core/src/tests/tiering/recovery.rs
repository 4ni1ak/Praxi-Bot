use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use storage::{Run, RunStep};
use task_runtime::{ExecError, Runtime, StepExecutor, StepSpec};
use tokio_util::sync::CancellationToken;

use super::*;

struct PauseAfterResponse {
    inner: TurnExecutor,
    paused: AtomicBool,
    runtime: OnceLock<std::sync::Weak<Runtime>>,
}

#[async_trait]
impl StepExecutor for PauseAfterResponse {
    async fn next_step(
        &self,
        run: &Run,
        history: &[RunStep],
    ) -> Result<Option<StepSpec>, ExecError> {
        if history.last().is_some_and(|step| {
            step.name == crate::shared::STEP_MODEL
                && step.output.as_ref().is_some_and(|out| {
                    out["tool_calls"]
                        .as_array()
                        .is_some_and(|calls| !calls.is_empty())
                })
        }) && !self.paused.swap(true, Ordering::SeqCst)
        {
            // The real light response is already durable. Pause before calling
            // the real planner: there is no prepared tool-step provenance yet.
            assert!(
                self.runtime
                    .get()
                    .unwrap()
                    .upgrade()
                    .unwrap()
                    .request_pause(run.id, false)
            );
            return Ok(None);
        }
        self.inner.next_step(run, history).await
    }

    async fn execute(
        &self,
        run: &Run,
        step: &StepSpec,
        cancel: &CancellationToken,
    ) -> Result<Value, ExecError> {
        self.inner.execute(run, step, cancel).await
    }
}

fn pause_after_response(lp: &AgentLoop) -> AgentLoop {
    let executor = Arc::new(PauseAfterResponse {
        inner: TurnExecutor { sh: lp.sh.clone() },
        paused: AtomicBool::new(false),
        runtime: OnceLock::new(),
    });
    let runtime = Arc::new(Runtime::new(lp.sh.storage.clone(), executor.clone()));
    executor.runtime.set(Arc::downgrade(&runtime)).unwrap();
    AgentLoop::from_parts(lp.sh.clone(), runtime)
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Cross-product covers three profile changes and both permission/effect changes.
async fn completed_light_authority_survives_profile_changes_and_tool_reclassification_on_restart() {
    for changed_write in [false, true] {
        for extra in [
            "",
            "light = \"fake/replacement\"\n",
            "light = \"fake/light\"\nfallback = [\"cli:codex/default\"]\n",
            LIGHT_CONFIG,
        ] {
            let old_read = Arc::new(tool("t.read", Effect::Read, "must remain unexecuted"));
            let (env, fake, lp) = rig(
                vec![
                    calls(&[("old-light-call", "t.read", json!({"source":"old-light"}))]),
                    text("standard decision"),
                ],
                &[&old_read],
                "",
                cfg(Policy::full()),
            );
            let lp = pause_after_response(&lp);
            let run = lp.run_turn(env.chat, "Merhaba").unwrap();
            assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
            assert_eq!(models(&fake), ["fake/light"]);
            let steps = env.storage.list_steps(run).unwrap();
            assert_eq!(steps.len(), 2, "pause before tools are planned");
            assert_eq!(steps[1].output.as_ref().unwrap()["tier"], "light");
            assert_eq!(old_read.count.load(Ordering::SeqCst), 0);
            drop(lp);

            let current_tool = Arc::new(tool(
                "t.read",
                if changed_write {
                    Effect::Write
                } else {
                    Effect::Read
                },
                "never",
            ));
            let mut config = cfg(if changed_write {
                Policy::full()
            } else {
                Policy::approve_destructive()
            });
            config.read_tools_skip_ask = false;
            let restarted = make_loop(
                &env.storage,
                env.bot,
                &fake,
                &[&current_tool],
                profile(extra),
                config,
            );
            let mut rx = restarted.subscribe();
            restarted.resume(run).unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(3), restarted.wait(run))
                    .await
                    .unwrap()
                    .unwrap(),
                RunState::Succeeded
            );
            assert_eq!(models(&fake), ["fake/light", "fake/m"]);
            assert_eq!(current_tool.count.load(Ordering::SeqCst), 0);
            assert!(
                env.storage.list_approvals(run).unwrap().is_empty(),
                "no approval for the old light call"
            );
            assert_eq!(
                serde_json::to_value(fake.request(0).messages).unwrap(),
                serde_json::to_value(fake.request(1).messages).unwrap()
            );
            let reason = if extra == LIGHT_CONFIG {
                if changed_write {
                    "non_read_tool"
                } else {
                    "approval_required"
                }
            } else {
                "profile_changed"
            };
            assert_upgrade(&events(&mut rx), reason);
            let audit = env.storage.list_audit(env.user).unwrap();
            let facts: Vec<_> = audit
                .iter()
                .filter(|row| row.action == "model.tier_escalated")
                .collect();
            assert_eq!(facts.len(), 1);
            assert_eq!(
                facts[0].detail["from"], "fake/light",
                "persisted original producer"
            );
            assert_eq!(
                env.storage
                    .list_steps(run)
                    .unwrap()
                    .last()
                    .unwrap()
                    .output
                    .as_ref()
                    .unwrap()["tier"],
                "standard"
            );
        }
    }
}

#[tokio::test]
async fn completed_executor_fallback_retains_standard_authority_after_profile_change() {
    let read = Arc::new(tool("t.read", Effect::Read, "read-result"));
    let (env, fake, lp) = rig(
        vec![
            Reply::Fail("unsupported"),
            calls(&[("standard-call", "t.read", json!({}))]),
            text("done"),
        ],
        &[&read],
        "",
        cfg(Policy::full()),
    );
    let lp = pause_after_response(&lp);
    let run = lp.run_turn(env.chat, "hello").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
    let steps = env.storage.list_steps(run).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].input["tier"], "light");
    assert_eq!(steps[1].output.as_ref().unwrap()["tier"], "standard");
    drop(lp);
    let mut config = cfg(Policy::approve_destructive());
    config.read_tools_skip_ask = false;
    let restarted = make_loop(&env.storage, env.bot, &fake, &[&read], profile(""), config);
    let mut rx = restarted.subscribe();
    restarted.resume(run).unwrap();
    let Event::ApprovalNeeded(card) =
        until(&mut rx, |event| matches!(event, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };
    assert_eq!(
        models(&fake),
        ["fake/light", "fake/m"],
        "the existing standard decision owns the approval"
    );
    restarted
        .approve(card.call_id, ApprovalScope::Once, env.user)
        .unwrap();
    assert_eq!(restarted.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(read.count.load(Ordering::SeqCst), 1);
    assert_eq!(models(&fake), ["fake/light", "fake/m", "fake/m"]);
    assert_eq!(
        env.storage
            .list_audit(env.user)
            .unwrap()
            .iter()
            .filter(|row| row.action == "model.tier_escalated")
            .count(),
        1
    );
}

#[tokio::test]
async fn historical_light_model_cannot_reenter_a_changed_fallback_list() {
    let read = Arc::new(tool("t.read", Effect::Read, "never"));
    let (env, fake, lp) = rig(
        vec![
            calls(&[("old-light", "t.read", json!({}))]),
            Reply::Fail("unsupported"),
            text("backup answer"),
        ],
        &[&read],
        "",
        cfg(Policy::full()),
    );
    let lp = pause_after_response(&lp);
    let run = lp.run_turn(env.chat, "hi").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
    drop(lp);
    let restarted = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[&read],
        profile("fallback = [\"fake/light\", \"fake/backup\"]\n"),
        cfg(Policy::full()),
    );
    let mut rx = restarted.subscribe();
    restarted.resume(run).unwrap();
    assert_eq!(restarted.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(models(&fake), ["fake/light", "fake/m", "fake/backup"]);
    assert_eq!(read.count.load(Ordering::SeqCst), 0);
    assert_upgrade(&events(&mut rx), "profile_changed");
}

async fn wait_requests(fake: &Fake, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while fake.n_requests() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected fake provider call");
}

#[tokio::test]
async fn interrupted_executor_after_unsupported_resumes_standard_once() {
    for restart in [false, true] {
        let (env, fake, lp) = rig(
            vec![Reply::Fail("unsupported"), Reply::Hang, text("resumed")],
            &[],
            "",
            cfg(Policy::full()),
        );
        let mut rx = lp.subscribe();
        let run = lp.run_turn(env.chat, "Merhaba").unwrap();
        wait_requests(&fake, 2).await;
        assert!(
            env.storage
                .has_run_audit_action(env.user, run, "model.tier_escalated")
                .unwrap()
        );
        assert!(lp.runtime().request_pause(run, true));
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
        let mut all_events = events(&mut rx);
        let lp = if restart {
            drop(lp);
            make_loop(
                &env.storage,
                env.bot,
                &fake,
                &[],
                profile(LIGHT_CONFIG),
                cfg(Policy::full()),
            )
        } else {
            lp
        };
        let mut resumed_events = lp.subscribe();
        lp.resume(run).unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        all_events.extend(events(&mut resumed_events));
        assert_eq!(models(&fake), ["fake/light", "fake/m", "fake/m"]);
        assert_upgrade(&all_events, "support");
        let rows = env.storage.list_audit(env.user).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|a| a.action == "model.tier_escalated")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn escalation_audit_failure_prevents_executor_dispatch_and_hides_db_error() {
    for unsupported in [true, false] {
        let write = Arc::new(tool("t.write", Effect::Write, "never"));
        let first = if unsupported {
            Reply::Fail("unsupported")
        } else {
            calls(&[("unsafe", "t.write", json!({}))])
        };
        let (env, fake, lp) = rig(
            vec![first, text("must not run")],
            &[&write],
            "",
            cfg(Policy::full()),
        );
        let connection = rusqlite::Connection::open(env.dir.path().join("bot.db")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_tier_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'model.tier_escalated' BEGIN SELECT RAISE(FAIL, 'private-database-detail'); END;").unwrap();
        let mut rx = lp.subscribe();
        let run = lp.run_turn(env.chat, "hi").unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Failed);
        assert_eq!(models(&fake), ["fake/light"]);
        assert_eq!(write.count.load(Ordering::SeqCst), 0);
        let evs = events(&mut rx);
        assert!(
            !serde_json::to_string(&evs)
                .unwrap()
                .contains("private-database-detail")
        );
        assert!(!evs.iter().any(|e| matches!(e, Event::ModelFallback(_))));
        assert!(
            env.storage
                .get_run(run)
                .unwrap()
                .unwrap()
                .error
                .unwrap()
                .contains("persist model tier")
        );
    }
}

#[tokio::test]
async fn unreadable_tier_latch_prevents_standard_dispatch_after_resume() {
    let (env, fake, lp) = rig(
        vec![
            calls(&[("invalid", "missing.tool", json!({}))]),
            text("must not run"),
        ],
        &[],
        "",
        cfg(Policy::full()),
    );
    let lp = pause_after_response(&lp);
    let run = lp.run_turn(env.chat, "hi").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
    assert_eq!(models(&fake), ["fake/light"]);
    let connection = rusqlite::Connection::open(env.dir.path().join("bot.db")).unwrap();
    connection
        .execute_batch("ALTER TABLE audit_log RENAME TO unavailable_audit_table")
        .unwrap();
    let mut rx = lp.subscribe();
    lp.resume(run).unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Failed);
    assert_eq!(models(&fake), ["fake/light"]);
    assert_eq!(
        env.storage.get_run(run).unwrap().unwrap().error.as_deref(),
        Some("step 'model' failed: could not read the persisted model tier")
    );
    let evs = events(&mut rx);
    assert!(!evs.iter().any(|e| matches!(e, Event::ModelFallback(_))));
    assert!(!serde_json::to_string(&evs).unwrap().contains("audit_log"));
}

/// Change policy precisely after the provider reply was checked, before the
/// prepared tools are dispatched. Nothing runs outside the fake tool registry.
struct ReviseBeforeTools {
    inner: TurnExecutor,
    changed: AtomicBool,
    runtime: OnceLock<std::sync::Weak<Runtime>>,
}

#[async_trait]
impl StepExecutor for ReviseBeforeTools {
    async fn next_step(
        &self,
        run: &Run,
        history: &[RunStep],
    ) -> Result<Option<StepSpec>, ExecError> {
        self.inner.next_step(run, history).await
    }
    async fn execute(
        &self,
        run: &Run,
        step: &StepSpec,
        cancel: &CancellationToken,
    ) -> Result<Value, ExecError> {
        if step.name == STEP_TOOLS && !self.changed.swap(true, Ordering::SeqCst) {
            let lp = AgentLoop::from_parts(
                self.inner.sh.clone(),
                self.runtime.get().unwrap().upgrade().unwrap(),
            );
            lp.set_global_policy(Policy::approve_destructive());
        }
        self.inner.execute(run, step, cancel).await
    }
}

#[tokio::test]
async fn permission_change_after_light_reply_discards_the_unexecuted_call() {
    let read = Arc::new(tool("t.read", Effect::Read, "never"));
    let mut config = cfg(Policy::full());
    config.read_tools_skip_ask = false;
    let (env, fake, lp) = rig(
        vec![
            calls(&[("old-light-call", "t.read", json!({}))]),
            text("standard decision"),
        ],
        &[&read],
        "",
        config,
    );
    let executor = Arc::new(ReviseBeforeTools {
        inner: TurnExecutor { sh: lp.sh.clone() },
        changed: AtomicBool::new(false),
        runtime: OnceLock::new(),
    });
    let runtime = Arc::new(Runtime::new(env.storage.clone(), executor.clone()));
    executor.runtime.set(Arc::downgrade(&runtime)).unwrap();
    let lp = AgentLoop::from_parts(lp.sh.clone(), runtime);
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "hello").unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), lp.wait(run))
            .await
            .unwrap()
            .unwrap(),
        RunState::Succeeded
    );
    assert_eq!(models(&fake), ["fake/light", "fake/m"]);
    assert_eq!(read.count.load(Ordering::SeqCst), 0);
    assert!(env.storage.list_approvals(run).unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(fake.request(0).messages).unwrap(),
        serde_json::to_value(fake.request(1).messages).unwrap()
    );
    assert_upgrade(&events(&mut rx), "approval_required");
}

#[tokio::test]
async fn subagent_model_role_is_unchanged_with_light_configured() {
    let (env, fake, lp) = rig(
        vec![
            calls(&[(
                "child",
                "spawn_subagent",
                json!({"task":"hello","tools":[]}),
            )]),
            text("child answer"),
            text("parent answer"),
        ],
        &[],
        "",
        cfg(Policy::full()),
    );
    let lp = lp.with_subagents();
    let run = lp
        .run_turn(env.chat, "Delegate this complex planning task")
        .unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(models(&fake), ["fake/m", "fake/m", "fake/m"]);
    let children = env.storage.list_child_runs(run).unwrap();
    assert_eq!(children.len(), 1);
    for step in env.storage.list_steps(children[0].id).unwrap() {
        assert!(step.input.get("tier").is_none());
    }
}

#[tokio::test]
async fn cli_executor_fallback_and_default_provider_disable_tiering_without_launching_cli() {
    let (env, fake, lp) = rig(vec![text("initial")], &[], "", cfg(Policy::full()));
    let id = lp.run_turn(env.chat, "hello").unwrap();
    assert_eq!(lp.wait(id).await.unwrap(), RunState::Succeeded);
    let template = env.storage.get_run(id).unwrap().unwrap();
    for (executor, fallback, default_cli) in [
        ("cli:codex/default", vec![], false),
        ("fake/m", vec!["cli:codex/default"], false),
        ("default", vec![], true),
    ] {
        let mut prof = profile(LIGHT_CONFIG);
        prof.models.executor = executor.into();
        prof.models.fallback = fallback.into_iter().map(str::to_owned).collect();
        lp.add_profile(env.bot, prof);
        if default_cli {
            lp.set_providers(Providers::single(fake.clone()).with_default_id("cli:codex"));
        }
        let run = Run {
            id: protocol::RunId::new(),
            ..template.clone()
        };
        env.storage.insert_run(&run).unwrap();
        let ctx = lp.sh.ctx(&run).unwrap();
        let planner = TurnExecutor { sh: lp.sh.clone() };
        assert_eq!(planner.light_model(&ctx), None);
        assert_eq!(planner.initial_tier(&ctx, "hello", None), None);
    }
    assert_eq!(
        fake.n_requests(),
        1,
        "no CLI subprocess or additional provider was invoked"
    );
}

#[tokio::test]
async fn escalation_fact_uses_production_redaction_and_is_scoped_to_the_run() {
    let secret = "private-model-name";
    let env = setup();
    let fake = Fake::new(vec![
        Reply::Fail("unsupported"),
        text("done"),
        text("new turn"),
    ]);
    let mut config = cfg(Policy::full());
    config.redactor = Some(redactor_with(secret));
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(&format!("light = \"fake/{secret}\"\n")),
        config,
    );
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "hi").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    let rows = env.storage.list_audit(env.user).unwrap();
    assert!(
        rows.iter()
            .all(|row| !row.detail.to_string().contains(secret))
    );
    let tier = rows
        .iter()
        .find(|a| a.action == "model.tier_escalated")
        .unwrap();
    assert!(!tier.detail.to_string().contains(secret));
    assert!(
        !serde_json::to_string(&events(&mut rx))
            .unwrap()
            .contains(secret)
    );
    assert!(
        env.storage
            .has_run_audit_action(env.user, run, "model.tier_escalated")
            .unwrap()
    );
    assert!(
        !env.storage
            .has_run_audit_action(UserId::new(), run, "model.tier_escalated")
            .unwrap()
    );
    assert!(
        !env.storage
            .has_run_audit_action(env.user, protocol::RunId::new(), "model.tier_escalated")
            .unwrap()
    );
    assert!(
        !env.storage
            .has_run_audit_action(env.user, run, "other")
            .unwrap()
    );
}
