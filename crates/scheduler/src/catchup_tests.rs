//! Missed-run catch-up: policies, the catch-up window, one-shots, disabled
//! schedules and no duplicate runs. All on a `FakeClock`, no real sleeping.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use protocol::BotId;

use crate::{
    FakeClock, MisfirePolicy, Payload, RUN_ALL_CAP, Result, Schedule, ScheduleExecutor,
    ScheduleSpec, ScheduleStore, Scheduler,
};

const T0: i64 = 1_700_000_000_000;
const HOUR: i64 = 3_600_000;

#[derive(Default)]
struct Rec(Mutex<usize>);

#[async_trait]
impl ScheduleExecutor for Rec {
    async fn execute(&self, _schedule: &Schedule) -> Result<()> {
        *self.0.lock() += 1;
        Ok(())
    }
}

fn hourly(policy: MisfirePolicy, window_secs: Option<u64>) -> Schedule {
    let mut s = Schedule::new(
        BotId::new(),
        None,
        "hourly",
        ScheduleSpec::Every { seconds: 3600 },
        Payload::AgentPrompt { text: "go".into() },
        policy,
        T0,
    )
    .unwrap();
    s.catchup_window_secs = window_secs;
    s
}

struct Fx {
    store: Arc<ScheduleStore>,
    exec: Arc<Rec>,
    clock: Arc<FakeClock>,
    scheduler: Scheduler,
}

async fn start(s: &Schedule) -> Fx {
    let store = Arc::new(ScheduleStore::open_in_memory().unwrap());
    store.insert(s).unwrap();
    let exec = Arc::new(Rec::default());
    let clock = Arc::new(FakeClock::new(T0));
    let scheduler = Scheduler::start_with_clock(Arc::clone(&store), exec.clone(), clock.clone());
    scheduler.settle().await;
    Fx {
        store,
        exec,
        clock,
        scheduler,
    }
}

fn missed(fx: &Fx, s: &Schedule) -> u32 {
    fx.store.get(s.id).unwrap().unwrap().missed_occurrences
}

#[tokio::test]
async fn run_once_with_a_window_runs_the_first_fresh_slot_and_lists_older_ones_as_missed() {
    // Computer asleep for 5 h: slots at +1h..+5h; window 2 h keeps +3h,+4h,+5h.
    let s = hourly(MisfirePolicy::RunOnce, Some(2 * 3600));
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(5 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 1, "RunOnce runs once");
    assert_eq!(
        missed(&fx, &s),
        2,
        "+1h and +2h are too old: listed as missed"
    );
    assert_eq!(
        fx.store.get(s.id).unwrap().unwrap().next_run_at,
        Some(T0 + 6 * HOUR)
    );
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn run_all_with_a_window_runs_only_in_window_slots() {
    let s = hourly(MisfirePolicy::RunAll, Some(150 * 60));
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(5 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 3, "+3h, +4h, +5h");
    assert_eq!(missed(&fx, &s), 2);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn a_miss_outside_the_window_is_recorded_not_silently_dropped() {
    let s = hourly(MisfirePolicy::RunOnce, Some(600));
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(5 * HOUR + HOUR / 2);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 0, "nothing runs late");
    assert_eq!(missed(&fx, &s), 5);
    assert!(fx.store.get(s.id).unwrap().unwrap().next_run_at.unwrap() > T0 + 5 * HOUR);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn without_a_window_catch_up_is_unchanged_and_capped() {
    let s = hourly(MisfirePolicy::RunAll, None);
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(25 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), RUN_ALL_CAP);
    assert_eq!(missed(&fx, &s), 0);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn skip_ignores_the_window_and_never_runs_late() {
    let s = hourly(MisfirePolicy::Skip, Some(24 * 3600));
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(5 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 0);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn a_missed_one_shot_outside_the_window_is_listed_and_finished() {
    let mut s = Schedule::new(
        BotId::new(),
        None,
        "once",
        ScheduleSpec::At {
            unix_ms: T0 + 60_000,
        },
        Payload::AgentPrompt { text: "go".into() },
        MisfirePolicy::RunOnce,
        T0,
    )
    .unwrap();
    s.catchup_window_secs = Some(3600);
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(24 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 0);
    assert_eq!(missed(&fx, &s), 1);
    assert_eq!(fx.store.get(s.id).unwrap().unwrap().next_run_at, None);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn a_disabled_schedule_is_never_caught_up() {
    let mut s = hourly(MisfirePolicy::RunAll, None);
    s.enabled = false;
    s.next_run_at = None;
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(5 * HOUR);
    fx.scheduler.settle().await;
    assert_eq!(*fx.exec.0.lock(), 0);
    assert_eq!(missed(&fx, &s), 0);
    fx.scheduler.shutdown().await;
}

#[tokio::test]
async fn a_restart_after_catch_up_does_not_run_the_same_slots_again() {
    let s = hourly(MisfirePolicy::RunAll, Some(3 * 3600));
    let fx = start(&s).await;
    fx.clock.jump_wall_ms(4 * HOUR);
    fx.scheduler.settle().await;
    let first = *fx.exec.0.lock();
    assert_eq!(
        first, 4,
        "+1h..+4h all inside a 3h window? +1h is exactly 3h old"
    );
    fx.scheduler.shutdown().await;
    let exec2 = Arc::new(Rec::default());
    let sched2 =
        Scheduler::start_with_clock(Arc::clone(&fx.store), exec2.clone(), fx.clock.clone());
    sched2.settle().await;
    assert_eq!(
        *exec2.0.lock(),
        0,
        "idempotent: slots already ran are not caught up again"
    );
    sched2.shutdown().await;
}
