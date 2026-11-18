use std::sync::atomic::AtomicBool;
use std::sync::{OnceLock, Weak};

use storage::{Run, RunStep};
use task_runtime::{ExecError, Runtime, StepExecutor, StepSpec, StepStatus};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::exec::TurnExecutor;
use crate::shared::{STEP_MODEL, STEP_USER};

const STEER: &str = "actually, do X";

struct PauseAfterSteer {
    inner: TurnExecutor,
    runtime: OnceLock<Weak<Runtime>>,
    interrupt: bool,
    injected: AtomicBool,
}

#[async_trait]
impl StepExecutor for PauseAfterSteer {
    async fn next_step(
        &self,
        run: &Run,
        history: &[RunStep],
    ) -> Result<Option<StepSpec>, ExecError> {
        let rt = self.runtime.get().unwrap().upgrade().unwrap();
        if history.last().is_some_and(|s| s.name == STEP_MODEL)
            && !self.injected.swap(true, Ordering::SeqCst)
        {
            let lp = AgentLoop::from_parts(self.inner.sh.clone(), rt.clone());
            assert_eq!(
                lp.send_user_message(run.chat_id.unwrap(), STEER, MessageMode::Steer)
                    .unwrap(),
                Delivery::Steered(run.id)
            );
        }
        let next = self.inner.next_step(run, history).await?;
        if next
            .as_ref()
            .is_some_and(|s| s.name == STEP_USER && s.input["text"] == STEER)
        {
            // No await between the real planner popping the steering queue
            // and the pause: exercise the post-planning boundary exactly.
            assert!(
                self.inner
                    .sh
                    .ctx(run)
                    .unwrap()
                    .state
                    .lock()
                    .steer
                    .is_empty()
            );
            assert!(rt.request_pause(run.id, self.interrupt));
        }
        Ok(next)
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

#[tokio::test]
async fn pause_after_planning_preserves_popped_steering_exactly_once() {
    for interrupt in [false, true] {
        let env = setup();
        let fake = Fake::new(vec![text("first answer"), text("adjusted")]);
        let lp = make_loop(
            &env.storage,
            env.bot,
            &fake,
            &[],
            profile(""),
            cfg(Policy::full()),
        );
        let planner = Arc::new(PauseAfterSteer {
            inner: TurnExecutor { sh: lp.sh.clone() },
            runtime: OnceLock::new(),
            interrupt,
            injected: AtomicBool::new(false),
        });
        let rt = Arc::new(Runtime::new(env.storage.clone(), planner.clone()));
        planner.runtime.set(Arc::downgrade(&rt)).unwrap();
        let lp = AgentLoop::from_parts(lp.sh.clone(), rt);
        let run = lp.run_turn(env.chat, "start").unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
        assert_eq!(fake.n_requests(), 1);

        lp.resume(run).unwrap();
        assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
        assert_eq!(fake.n_requests(), 2);
        assert_eq!(
            fake.request(1)
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::User && m.content == json!(STEER))
                .count(),
            1
        );
        let steps = env.storage.list_steps(run).unwrap();
        let steering: Vec<_> = steps
            .iter()
            .filter(|s| s.name == STEP_USER && s.input["text"] == STEER)
            .collect();
        assert_eq!(steering.len(), 1);
        assert_eq!(steering[0].status, StepStatus::Succeeded.as_str());
        assert_eq!(steering[0].attempt, 1);
    }
}

/// Both mechanisms apply at once: #53 keeps the popped steering as a durable
/// (unexecuted) user-step intent, and the restart recovery rebuilds the steer
/// queue from the stored chat rows. The message must still be delivered once.
#[test]
fn pause_after_planning_then_restart_delivers_steering_exactly_once() {
    for interrupt in [false, true] {
        let env = setup();
        let path = env.dir.path().to_path_buf();
        let bot = env.bot;
        let rt1 = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let run = rt1.block_on(async {
            let fake = Fake::new(vec![text("first answer")]);
            let lp = make_loop(
                &env.storage,
                env.bot,
                &fake,
                &[],
                profile(""),
                cfg(Policy::full()),
            );
            let planner = Arc::new(PauseAfterSteer {
                inner: TurnExecutor { sh: lp.sh.clone() },
                runtime: OnceLock::new(),
                interrupt,
                injected: AtomicBool::new(false),
            });
            let rt = Arc::new(Runtime::new(env.storage.clone(), planner.clone()));
            planner.runtime.set(Arc::downgrade(&rt)).unwrap();
            let lp = AgentLoop::from_parts(lp.sh.clone(), rt);
            let run = lp.run_turn(env.chat, "start").unwrap();
            assert_eq!(lp.wait(run).await.unwrap(), RunState::Paused);
            assert_eq!(fake.n_requests(), 1);
            run
        });
        drop(rt1);
        let _data_dir = env.crash();

        let storage = Arc::new(Storage::open(&path).unwrap());
        let rt2 = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt2.block_on(async {
            let fake = Fake::new(vec![text("adjusted")]);
            let lp = make_loop(&storage, bot, &fake, &[], profile(""), cfg(Policy::full()));
            lp.recover().unwrap();
            lp.resume(run).unwrap();
            assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
            assert_eq!(fake.n_requests(), 1);
            let users: Vec<_> = fake
                .request(0)
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::User)
                .filter_map(|m| m.content.as_str().map(str::to_owned))
                .collect();
            assert_eq!(
                users,
                ["start", STEER],
                "steering must be delivered exactly once (interrupt={interrupt})"
            );
            let steering = storage
                .list_steps(run)
                .unwrap()
                .into_iter()
                .filter(|s| s.name == STEP_USER && s.input["text"] == STEER)
                .count();
            assert_eq!(steering, 1);
            let fresh = make_loop(&storage, bot, &fake, &[], profile(""), cfg(Policy::full()));
            assert!(fresh.ctx_of(run).unwrap().state.lock().steer.is_empty());
        });
    }
}
