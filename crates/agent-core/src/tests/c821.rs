//! The owner writes while a tool approval of their own turn is open: the approval is denied
//! (never approved), the turn stops the way Stop does it, and the message starts the next turn
//! at once, told that the previous action did not run. Runs that someone else started are
//! left alone.
use super::*;
use crate::shared::initiator_of;

fn write_tool() -> Arc<TestTool> {
    Arc::new(tool("t.write", Effect::Write, "ran"))
}

async fn settled_runs(env: &Env, want: usize) -> Vec<storage::Run> {
    for _ in 0..300 {
        let runs = env.storage.list_runs(env.user).unwrap();
        if runs.len() >= want
            && runs
                .iter()
                .all(|r| RunState::parse(&r.state).is_some_and(RunState::is_terminal))
        {
            return runs;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the runs did not settle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writing_a_new_message_denies_the_pending_approval_and_starts_the_next_turn() {
    let env = setup();
    let write = write_tool();
    let fake = Fake::new(vec![
        calls(&[("c1", "t.write", json!({}))]),
        text("must not be asked for the first run"),
        text("second turn answer"),
    ]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&write)],
        profile(""),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let first = lp.run_turn(env.chat, "go").unwrap();
    let Event::ApprovalNeeded(a) = until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await
    else {
        unreachable!()
    };

    // The owner's app writes instead of answering.
    let delivery = lp
        .send_user_message_from(env.chat, "do something else", MessageMode::Queue, "owner")
        .unwrap();
    assert_eq!(delivery, Delivery::Redirected(first));

    // The card is closed as a denial with the redirect reason; no tool ran.
    let Event::ToolCallDenied(d) = until(
        &mut rx,
        |e| matches!(e, Event::ToolCallDenied(d) if d.call_id == a.call_id),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        d.reason.as_deref(),
        Some("denied: the owner wrote a new message instead")
    );
    assert_eq!(
        write.count.load(Ordering::SeqCst),
        0,
        "the action never ran"
    );
    assert_eq!(
        env.storage
            .list_approvals(first)
            .unwrap()
            .into_iter()
            .map(|a| a.decision)
            .collect::<Vec<_>>(),
        [Some("denied".to_owned())]
    );
    assert!(
        lp.approve(a.call_id, ApprovalScope::Once, env.user)
            .is_err(),
        "the closed card cannot be approved afterwards"
    );

    // The first turn is stopped, the message started the next one right away.
    let runs = settled_runs(&env, 2).await;
    assert_eq!(runs.len(), 2);
    assert_eq!(
        RunState::parse(&runs.iter().find(|r| r.id == first).unwrap().state),
        Some(RunState::Cancelled)
    );
    let second = runs.iter().find(|r| r.id != first).unwrap();
    assert_eq!(second.state, "succeeded");
    assert_eq!(initiator_of(&second.kind), InitiatorKind::Human);
    assert_eq!(
        fake.n_requests(),
        2,
        "one model call for the first turn, one for the next: nothing after the denial"
    );

    // The model of the next turn is told the previous action did not run, and sees the message.
    let req = fake.request(1);
    let system_notes: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .filter_map(|m| m.content.as_str())
        .collect();
    assert!(
        system_notes
            .iter()
            .any(|n| n.contains("NOT approved") && n.contains("did NOT run")),
        "{system_notes:?}"
    );
    assert!(req.messages.iter().any(|m| {
        m.role == MessageRole::User
            && m.content
                .as_str()
                .is_some_and(|c| c.contains("do something else"))
    }));
    // What did not run rides along as quoted, untrusted data (never inside the system note).
    assert!(req.messages.iter().any(|m| {
        m.role == MessageRole::User
            && m.content
                .as_str()
                .is_some_and(|c| c.contains("approval.skipped") && c.contains("UNTRUSTED"))
    }));
    // The note is used once.
    assert!(lp.sh.turn_notes.lock().is_empty());
    assert!(lp.sh.redirected.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_from_an_unknown_source_or_to_an_external_run_is_only_queued() {
    let env = setup();
    let write = write_tool();
    let fake = Fake::new(vec![calls(&[("c1", "t.write", json!({}))]), text("done")]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&write)],
        profile(""),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let first = lp.run_turn(env.chat, "go").unwrap();
    until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await;
    // Not the owner's app: queued behind the approval as before.
    assert_eq!(
        lp.send_user_message_from(env.chat, "later", MessageMode::Queue, "local_api")
            .unwrap(),
        Delivery::Queued(InitiatorKind::Human)
    );
    assert!(lp.sh.redirected.lock().is_empty());
    assert!(lp.sh.turn_notes.lock().is_empty());
    assert_eq!(
        lp.runtime().state(first).unwrap(),
        RunState::WaitingApproval
    );
    lp.cancel(first).unwrap();

    // A run another initiator started is never redirected, whoever writes.
    let env = setup();
    let write = write_tool();
    let fake = Fake::new(vec![calls(&[("c1", "t.write", json!({}))]), text("done")]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[t(&write)],
        profile(""),
        cfg(Policy::approve_destructive()),
    );
    let mut rx = lp.subscribe();
    let external = lp
        .run_turn_as(env.chat, "scheduled work", InitiatorKind::Schedule)
        .unwrap();
    until(&mut rx, |e| matches!(e, Event::ApprovalNeeded(_))).await;
    assert_eq!(
        lp.send_user_message_from(env.chat, "hello", MessageMode::Queue, "owner")
            .unwrap(),
        Delivery::Queued(InitiatorKind::Schedule)
    );
    assert!(lp.sh.redirected.lock().is_empty());
    assert_eq!(
        lp.runtime().state(external).unwrap(),
        RunState::WaitingApproval,
        "the scheduled run still waits for its approval"
    );
    assert_eq!(write.count.load(Ordering::SeqCst), 0);
    lp.cancel(external).unwrap();
}
