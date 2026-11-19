//! Edit & resend / regenerate of the last turn. No real provider or tool is used.
use super::*;
use crate::{Delivery, LoopError, MessageMode};
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
