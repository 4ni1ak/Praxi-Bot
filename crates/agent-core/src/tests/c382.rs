//! Edit & resend / regenerate of the last turn. No real provider or tool is used.
use super::*;
use crate::{Delivery, LoopError, MessageMode};
use protocol::RunId;
use uuid::Uuid;

fn last_user_id(env: &Env) -> Uuid {
    env.storage
        .list_messages(env.chat)
        .unwrap()
        .iter()
        .rfind(|m| m.role == "user")
        .unwrap()
        .id
}

#[tokio::test]
async fn edit_supersedes_the_old_turn_out_of_context_and_keeps_it_in_storage() {
    let env = setup();
    let fake = Fake::new(vec![text("answer one"), text("answer two")]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    let Delivery::NewTurn(first) = lp
        .send_user_message_from(env.chat, "first question", MessageMode::Queue, "owner")
        .unwrap()
    else {
        panic!("expected a new turn");
    };
    lp.wait(first).await.unwrap();
    let old_user = last_user_id(&env);

    // A stale id and a message of another source are refused; nothing changes.
    assert!(matches!(
        lp.resend_last_turn(env.chat, Uuid::now_v7(), None),
        Err(LoopError::NotLastMessage)
    ));
    env.storage
        .set_message_source(old_user, "telegram")
        .unwrap();
    assert!(matches!(
        lp.resend_last_turn(env.chat, old_user, Some("x")),
        Err(LoopError::NotResendable(_))
    ));
    env.storage.set_message_source(old_user, "owner").unwrap();
    assert_eq!(env.storage.list_messages(env.chat).unwrap().len(), 2);

    let second = lp
        .resend_last_turn(env.chat, old_user, Some("edited question"))
        .unwrap();
    lp.wait(second).await.unwrap();

    // The model saw the edit and neither the old question nor the old answer.
    let sent = format!("{:?}", fake.request(1).messages);
    assert!(sent.contains("edited question"));
    assert!(!sent.contains("first question") && !sent.contains("answer one"));
    // The old turn is still stored, marked superseded; the new one is the owner's.
    let all = env.storage.list_messages_with_superseded(env.chat).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(
        env.storage.superseded_message_ids(env.chat).unwrap().len(),
        2
    );
    let live = env.storage.list_messages(env.chat).unwrap();
    assert_eq!(live.len(), 2);
    assert_eq!(live[0].content, "edited question");
    assert_eq!(env.storage.message_source(live[0].id).unwrap(), "owner");
}

async fn finished_turn(env: &Env, answers: Vec<Reply>) -> (AgentLoop, Arc<Fake>, RunId, Uuid) {
    let fake = Fake::new(answers);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    let Delivery::NewTurn(run) = lp
        .send_user_message_from(env.chat, "first question", MessageMode::Queue, "owner")
        .unwrap()
    else {
        panic!("expected a new turn");
    };
    lp.wait(run).await.unwrap();
    (lp, fake, run, last_user_id(env))
}

fn add_step(env: &Env, run: RunId, idx: u32, name: &str, idempotent: bool) {
    env.storage
        .insert_step(&storage::RunStep {
            id: Uuid::now_v7(),
            run_id: run,
            user_id: env.user,
            idx,
            name: name.into(),
            input: json!({}),
            idempotent,
            retry: None,
            status: "succeeded".into(),
            attempt: 1,
            output: None,
            error: None,
            started_at_ms: 1,
            finished_at_ms: Some(1),
        })
        .unwrap();
}

#[tokio::test]
async fn acted_warning_covers_cli_model_and_workflow_steps_but_not_pure_reads() {
    let env = setup();
    let (lp, _fake, run, _) = finished_turn(&env, vec![text("a")]).await;
    // A plain run: user message + replay-safe model step.
    assert!(!lp.run_used_non_read_tools(run).unwrap());
    // A CLI-engine model step is non-idempotent (the CLI runs its own tools).
    add_step(&env, run, 90, "model", false);
    assert!(lp.run_used_non_read_tools(run).unwrap());

    let env2 = setup();
    let (lp2, _f, run2, _) = finished_turn(&env2, vec![text("a")]).await;
    add_step(&env2, run2, 90, "workflow", false);
    assert!(lp2.run_used_non_read_tools(run2).unwrap());

    let env3 = setup();
    let (lp3, _f, run3, _) = finished_turn(&env3, vec![text("a")]).await;
    add_step(&env3, run3, 90, "tools", true);
    assert!(!lp3.run_used_non_read_tools(run3).unwrap());
}

#[tokio::test]
async fn regenerate_without_text_repeats_the_question_and_supersedes_the_answer() {
    let env = setup();
    let (lp, fake, _first, old_user) =
        finished_turn(&env, vec![text("answer one"), text("answer two")]).await;
    let second = lp.resend_last_turn(env.chat, old_user, None).unwrap();
    lp.wait(second).await.unwrap();
    let sent = format!("{:?}", fake.request(1).messages);
    assert!(sent.contains("first question") && !sent.contains("answer one"));
    let live = env.storage.list_messages(env.chat).unwrap();
    assert_eq!(live.len(), 2);
    assert_eq!(live[0].content, "first question");
    assert_eq!(live[1].content, "answer two");
    assert_eq!(
        env.storage.superseded_message_ids(env.chat).unwrap().len(),
        2
    );
}

#[tokio::test]
async fn unfinished_run_is_refused_and_nothing_is_superseded() {
    let env = setup();
    let (lp, _fake, run, old_user) = finished_turn(&env, vec![text("a")]).await;
    env.storage.update_run_state(run, "running", None).unwrap();
    assert!(matches!(
        lp.resend_last_turn(env.chat, old_user, None),
        Err(LoopError::NotResendable(_))
    ));
    assert!(
        env.storage
            .superseded_message_ids(env.chat)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn busy_chat_is_refused_and_nothing_is_superseded() {
    let env = setup();
    let fake = Fake::new(vec![Reply::Hang]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile(""),
        cfg(Policy::full()),
    );
    lp.send_user_message_from(env.chat, "q", MessageMode::Queue, "owner")
        .unwrap();
    let id = last_user_id(&env);
    assert!(matches!(
        lp.resend_last_turn(env.chat, id, None),
        Err(LoopError::Busy(_))
    ));
    assert!(
        env.storage
            .superseded_message_ids(env.chat)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn failed_start_restores_the_old_turn() {
    let env = setup();
    let (lp, _fake, _run, old_user) = finished_turn(&env, vec![text("a")]).await;
    lp.shutdown(Duration::from_millis(10)).await;
    assert!(lp.resend_last_turn(env.chat, old_user, Some("x")).is_err());
    assert!(
        env.storage
            .superseded_message_ids(env.chat)
            .unwrap()
            .is_empty()
    );
    assert_eq!(env.storage.list_messages(env.chat).unwrap().len(), 2);
}

#[tokio::test]
async fn superseded_taint_and_memory_index_do_not_reach_the_next_run() {
    let env = setup();
    let (store, mem) = memory_cfg();
    let fake = Fake::new(vec![text("a"), text("b")]);
    let mut config = cfg(Policy::full());
    config.memory = Some(mem);
    let lp = make_loop(&env.storage, env.bot, &fake, &[], profile(""), config);
    let Delivery::NewTurn(first) = lp
        .send_user_message_from(env.chat, "zebrafish secret", MessageMode::Queue, "owner")
        .unwrap()
    else {
        panic!("expected a new turn");
    };
    lp.wait(first).await.unwrap();
    let old_user = last_user_id(&env);
    // A tainted answer of the old turn.
    env.storage
        .insert_message(&storage::Message {
            id: Uuid::now_v7(),
            user_id: env.user,
            chat_id: env.chat,
            run_id: Some(first),
            role: "assistant".into(),
            content: "tainted".into(),
            created_at_ms: now_ms(),
            exposure_untrusted: true,
            exposure_private: false,
            exposure_sources: vec!["web".into()],
        })
        .unwrap();
    assert_eq!(
        store
            .search_sessions(env.user, "zebrafish", 5)
            .unwrap()
            .len(),
        1
    );
    let second = lp
        .resend_last_turn(env.chat, old_user, Some("edited"))
        .unwrap();
    lp.wait(second).await.unwrap();
    let run = env.storage.get_run(second).unwrap().unwrap();
    assert!(!lp.workflow_run_carried_exposure(&run));
    assert!(
        store
            .search_sessions(env.user, "zebrafish", 5)
            .unwrap()
            .is_empty()
    );
}
