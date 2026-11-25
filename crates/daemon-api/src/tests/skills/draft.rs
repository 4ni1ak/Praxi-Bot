//! "Save as skill": the draft comes from the run's steps, never copies secrets, and nothing is
//! installed until the owner confirms the reviewed fingerprint.
use super::*;
use crate::tests::{call_token_only, make_chat};
use protocol::{BotId, ChatId, RunId};
use uuid::Uuid;

const CANARY: &str = "sk-live-CANARY9f3a77c2d1e84b6a90";

fn record_run(t: &T, bot: &str, chat: &str) {
    let st = t.state.storage();
    let run = RunId::new();
    st.insert_run(&storage::Run {
        id: run,
        user_id: t.state.user_id(),
        bot_id: BotId(bot.parse().unwrap()),
        chat_id: Some(ChatId(chat.parse().unwrap())),
        kind: "turn".into(),
        state: "succeeded".into(),
        error: None,
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_run_id: None,
        spec: None,
        computer_target: None,
        computer_node: None,
        computer_node_name: None,
        paused_at_ms: None,
        pause_reason: None,
    })
    .unwrap();
    let args = json!({"url": "https://example.com/docs", "api_key": CANARY, "q": CANARY});
    st.insert_step(&storage::RunStep {
        id: Uuid::now_v7(),
        run_id: run,
        user_id: t.state.user_id(),
        idx: 0,
        name: "tools".into(),
        input: json!({"calls": [
            {"id": "c1", "name": "web_fetch", "arguments": args.to_string(), "repeat": 1},
            {"id": "c2", "name": "web_search", "arguments": "{}", "repeat": 1}
        ]}),
        idempotent: true,
        retry: None,
        status: "succeeded".into(),
        attempt: 1,
        // c2 failed, so it must not become a step.
        output: Some(json!({"results": [
            {"id": "c1", "is_error": false, "text": "ok"},
            {"id": "c2", "is_error": true, "text": "boom"}
        ]})),
        error: None,
        started_at_ms: 0,
        finished_at_ms: Some(1),
    })
    .unwrap();
}

#[tokio::test]
async fn draft_has_no_secret_installs_only_after_confirmation_and_is_owner_only() {
    let t = setup_skills(|_| {});
    let (bot, chat) = make_chat(&t).await;
    let path = format!("/v1/chats/{chat}/skill-draft");

    // Nothing finished yet: an honest, specific refusal.
    let r = call(&t, Method::POST, &path, None).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["error"]["code"], "skill_draft_no_run");

    record_run(&t, &bot, &chat);
    let r = call_token_only(&t, Method::POST, &path, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = call(&t, Method::POST, &path, None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let d = r.json();
    assert!(!d.to_string().contains(CANARY), "secret leaked into draft");
    assert_eq!(d["steps"].as_array().unwrap().len(), 1, "failed call kept");
    assert_eq!(d["tools"][0]["name"], "web_fetch");
    assert!(d["withheld"].as_u64().unwrap() >= 2);
    let skill_md = d["skill_md"].as_str().unwrap().to_owned();
    let name = d["name"].as_str().unwrap().to_owned();
    let fingerprint = d["preview"]["fingerprint"].as_str().unwrap().to_owned();

    // The draft alone installs nothing; a wrong confirmation installs nothing.
    let dir = t.state.data_dir().join("skills").join(&name);
    assert!(!dir.exists());
    let body = json!({"skill_md": skill_md, "accept": "wrong"});
    let r = call(&t, Method::POST, "/v1/skills/draft/install", Some(body)).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(!dir.exists());

    // A secret added while editing is refused before anything is written.
    let edited = format!("{skill_md}\nUse {CANARY}\n");
    let pre = call(
        &t,
        Method::POST,
        "/v1/skills/draft/preview",
        Some(json!({"skill_md": edited})),
    )
    .await;
    assert_eq!(pre.status, StatusCode::OK);

    let body = json!({"skill_md": skill_md, "accept": fingerprint});
    let r = call_token_only(
        &t,
        Method::POST,
        "/v1/skills/draft/install",
        Some(body.clone()),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = call(&t, Method::POST, "/v1/skills/draft/install", Some(body)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let saved = fs::read_to_string(dir.join("SKILL.md")).unwrap();
    assert!(!saved.contains(CANARY));
    assert!(saved.contains("web_fetch"));
}
