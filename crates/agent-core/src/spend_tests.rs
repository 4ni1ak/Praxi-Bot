//! Monthly spending limit: enforced before the model call, never when unset.

use permission_engine::Policy;
use protocol::Event;
use task_runtime::RunState;

use crate::tests::{Fake, cfg, make_loop, profile, setup, text};
use crate::{ModelPrice, spend};

/// $1 per token, so one test call (10 in, 5 out) costs $15 and the first-call
/// estimate (4000 prompt + `max_tokens` 10) is $4010.
fn priced() -> crate::LoopConfig {
    let mut c = cfg(Policy::full());
    c.max_tokens = Some(10);
    c.prices.insert(
        "fake/m".into(),
        ModelPrice {
            input_per_mtok: 1_000_000.0,
            output_per_mtok: 1_000_000.0,
            cached_per_mtok: None,
        },
    );
    c
}

async fn run_once(limit: Option<f64>) -> (RunState, usize, Vec<Event>, Vec<String>, f64) {
    let env = setup();
    let fake = Fake::new(vec![text("hello")]);
    let lp = make_loop(&env.storage, env.bot, &fake, &[], profile(""), priced());
    lp.set_spend_limit(limit);
    let mut rx = lp.subscribe();
    let run = lp.run_turn(env.chat, "hi").unwrap();
    let state = lp.wait(run).await.unwrap();
    let mut evs = Vec::new();
    while let Ok(i) = rx.try_recv() {
        evs.push(i.envelope.event);
    }
    let actions = env
        .storage
        .list_audit(env.user)
        .unwrap()
        .into_iter()
        .map(|a| a.action)
        .collect();
    let spent = lp.spend_status(env.user).unwrap().spent_usd;
    (state, fake.n_requests(), evs, actions, spent)
}

#[tokio::test]
async fn call_that_could_exceed_the_limit_is_refused_and_recorded() {
    let (state, requests, evs, actions, _) = run_once(Some(100.0)).await;
    assert_eq!(state, RunState::Failed);
    assert_eq!(requests, 0, "no provider request may be made");
    assert!(actions.iter().any(|a| a == "spend.limit_blocked"));
    let msg = evs
        .iter()
        .find_map(|e| match e {
            Event::RunFinished(f) => f.error.clone(),
            _ => None,
        })
        .expect("run ends with an error");
    assert!(msg.contains(spend::SPEND_LIMIT_REACHED), "{msg}");
    assert!(msg.contains("Raise or remove the limit"), "{msg}");
}

#[tokio::test]
async fn call_under_the_limit_proceeds_and_is_counted() {
    let (state, requests, _, actions, spent) = run_once(Some(5_000.0)).await;
    assert_eq!(state, RunState::Succeeded);
    assert_eq!(requests, 1);
    assert!(!actions.iter().any(|a| a == "spend.limit_blocked"));
    assert!((spent - 15.0).abs() < 1e-6, "{spent}");
}

#[tokio::test]
async fn unset_limit_changes_nothing() {
    let (state, requests, _, actions, _) = run_once(None).await;
    assert_eq!(state, RunState::Succeeded);
    assert_eq!(requests, 1);
    assert!(!actions.iter().any(|a| a.starts_with("spend.")));
}

#[tokio::test]
async fn parallel_runs_cannot_all_pass_just_under_the_limit() {
    // Limit fits ONE first-call estimate ($4010) but not two.
    let env = setup();
    let chat2 = protocol::ChatId::new();
    env.storage
        .insert_chat(&storage::Chat {
            id: chat2,
            user_id: env.user,
            bot_id: env.bot,
            title: "c2".into(),
            created_at_ms: storage::now_ms(),
        })
        .unwrap();
    let fake = Fake::new(vec![crate::tests::Reply::Hang, text("never")]);
    let lp = make_loop(&env.storage, env.bot, &fake, &[], profile(""), priced());
    lp.set_spend_limit(Some(5_000.0));
    let first = lp.run_turn(env.chat, "one").unwrap();
    for _ in 0..200 {
        if fake.n_requests() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(fake.n_requests(), 1, "first call is in flight");
    let second = lp.run_turn(chat2, "two").unwrap();
    let state = lp.wait(second).await.unwrap();
    assert_eq!(state, RunState::Failed, "in-flight estimate must count");
    assert_eq!(
        fake.n_requests(),
        1,
        "second call never reached the provider"
    );
    lp.cancel(first).unwrap();
}
