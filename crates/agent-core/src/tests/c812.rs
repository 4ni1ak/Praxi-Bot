//! Stop hardening: a queued message keeps the initiator kind it was queued
//! under, and the set of concluded runs is bounded.
use super::*;
use crate::shared::{CONCLUDED_CAP, RecentRuns};
use protocol::RunId;

/// A message queued behind a schedule run starts as a schedule turn, even when
/// Stop left it queued and a different (human) run ended first. Before the
/// fix it took the kind of whichever run ended next (human here).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_message_keeps_the_initiator_it_was_queued_under() {
    let env = setup();
    let mut slow = tool("t.slow", Effect::Read, "r");
    slow.delay = Duration::from_secs(30);
    let slow = Arc::new(slow);
    let fake = Fake::new(vec![
        calls(&[("c1", "t.slow", json!({}))]),
        text("human answer"),
        text("queued answer"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&slow)],
        profile(""),
        cfg(Policy::full()),
    );
    let mut rx = lp.subscribe();
    let first = lp
        .run_turn_as(env.chat, "scheduled work", InitiatorKind::Schedule)
        .unwrap();
    until(&mut rx, |e| matches!(e, Event::ToolCallRequested(_))).await;
    // The kind is reported to the caller and stored with the message.
    assert_eq!(
        lp.send_user_message(env.chat, "later", MessageMode::Queue)
            .unwrap(),
        Delivery::Queued(InitiatorKind::Schedule)
    );
    assert_eq!(
        lp.sh.queued.lock().get(&env.chat).unwrap()[0].initiator,
        InitiatorKind::Schedule
    );
    // Stop keeps it queued; a human turn then ends first and starts it.
    lp.cancel(first).unwrap();
    for _ in 0..100 {
        if lp.runtime().state(first).unwrap().is_terminal()
            && !lp.sh.active.lock().contains_key(&env.chat)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let second = lp.run_turn(env.chat, "human turn").unwrap();
    assert_eq!(lp.wait(second).await.unwrap(), RunState::Succeeded);
    let mut started = None;
    for _ in 0..200 {
        started = env
            .storage
            .list_runs(env.user)
            .unwrap()
            .into_iter()
            .find(|r| r.id != first && r.id != second);
        if started.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = started.expect("the queued message started a turn");
    assert_eq!(
        crate::shared::initiator_of(&started.kind),
        InitiatorKind::Schedule,
        "the queued turn took the kind of the run that ended before it: {}",
        started.kind
    );
    assert!(
        lp.sh
            .queued
            .lock()
            .get(&env.chat)
            .is_none_or(VecDeque::is_empty)
    );
}

/// `concluded` never grows past its cap, drops the oldest ids first, and still
/// refuses a second conclude of a recent run.
#[test]
fn the_concluded_set_is_bounded_and_forgets_the_oldest_first() {
    let mut set = RecentRuns::new(CONCLUDED_CAP);
    let ids: Vec<RunId> = (0..CONCLUDED_CAP + 10).map(|_| RunId::new()).collect();
    for id in &ids {
        assert!(set.insert(*id));
    }
    assert_eq!(set.len(), CONCLUDED_CAP);
    assert!(!set.contains(&ids[0]), "the oldest id is forgotten");
    let newest = *ids.last().unwrap();
    assert!(set.contains(&newest));
    assert!(
        !set.insert(newest),
        "a second conclude of a recent run is refused"
    );
    assert_eq!(set.len(), CONCLUDED_CAP);
}
