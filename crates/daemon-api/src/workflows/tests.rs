//! HTTP recording, user-only persistence and permission-gated replay.
#![allow(clippy::too_many_lines)]
use super::*;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use futures::{StreamExt, stream::BoxStream};
use http_body_util::BodyExt;
use provider_catalog::{ChatChunk, ChatProvider, ChatRequest, FinishReason, ProviderError};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tool_registry::{
    Effect, PermissionCategory, Platform, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry,
    ToolSpec,
};
use tower::ServiceExt;

struct Provider {
    saved: AtomicBool,
    executor_calls: AtomicUsize,
}
#[async_trait]
impl ChatProvider for Provider {
    fn id(&self) -> &'static str {
        "fake"
    }
    async fn stream(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        if req.model.0 == "fake/classifier" {
            let text = if self.saved.load(Ordering::SeqCst) {
                r#"{"route":"workflow","name":"recorded","confidence":1.0}"#
            } else {
                r#"{"route":"tool","name":"mutate","confidence":1.0}"#
            };
            return Ok(futures::stream::iter(vec![
                Ok(ChatChunk::TextDelta(text.into())),
                Ok(ChatChunk::Done {
                    finish_reason: FinishReason::Stop,
                }),
            ])
            .boxed());
        }
        self.executor_calls.fetch_add(1, Ordering::SeqCst);
        crate::fake::FakeProvider.stream(req).await
    }
}

struct TestTool {
    spec: ToolSpec,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for TestTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn call(&self, _: &ToolContext, _: Value) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::text("done"))
    }
}

struct Env {
    state: AppState,
    chat: ChatId,
    dir: tempfile::TempDir,
    provider: Arc<Provider>,
    calls: Arc<AtomicUsize>,
}
fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(storage::Storage::open(dir.path()).unwrap());
    let registry = ToolRegistry::new();
    let calls = Arc::new(AtomicUsize::new(0));
    for (name, effect) in [
        ("mutate", Effect::Write),
        ("fs_read", Effect::Read),
        ("browser_snapshot", Effect::Read),
    ] {
        registry.register(Arc::new(TestTool {
            spec: ToolSpec {
                name: name.into(),
                description: name.into(),
                input_schema: json!({"type":"object"}),
                effect,
                category: PermissionCategory::Fs,
                platforms: vec![Platform::All],
            },
            calls: calls.clone(),
        }));
    }
    let provider = Arc::new(Provider {
        saved: AtomicBool::new(false),
        executor_calls: AtomicUsize::new(0),
    });
    let agent = agent_core::AgentLoop::new(
        storage.clone(),
        registry,
        agent_core::Providers::single(provider.clone()),
        agent_core::LoopConfig {
            global_policy: permission_engine::Policy::full(),
            ..Default::default()
        },
    )
    .with_workflows(
        WorkflowStore::new(dir.path()),
        workflow_replay::Handles {
            desktop: None,
            browser: None,
            tools: ToolRegistry::new(),
        },
    );
    let state = AppState::new(
        agent,
        storage.clone(),
        crate::DaemonConfig::new("test-token"),
    )
    .unwrap();
    let profile = agent_core::BotProfile::from_toml_str("[bot]\nid='test'\nname='test'\n[models]\nexecutor='fake/echo'\nclassifier='fake/classifier'\n").unwrap();
    let bot = storage::Bot {
        id: protocol::BotId::new(),
        user_id: state.user_id(),
        name: "test".into(),
        config: json!({"bot":{"id":"test","name":"test"},"models":{"executor":"fake/echo","classifier":"fake/classifier"}}),
        created_at_ms: 0,
    };
    storage.insert_bot(&bot).unwrap();
    state.agent().add_profile(bot.id, profile);
    let chat = storage::Chat {
        id: ChatId::new(),
        user_id: state.user_id(),
        bot_id: bot.id,
        title: "test".into(),
        created_at_ms: 0,
    };
    storage.insert_chat(&chat).unwrap();
    Env {
        state,
        chat: chat.id,
        dir,
        provider,
        calls,
    }
}

async fn request(
    env: &Env,
    method: Method,
    path: &str,
    body: Option<Value>,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1");
    if authenticated {
        req = req.header("authorization", "Bearer test-token");
    }
    let req = req
        .header("content-type", "application/json")
        .body(Body::from(body.map_or(String::new(), |v| v.to_string())))
        .unwrap();
    let response = crate::router(env.state.clone()).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn source(env: &Env) -> RunId {
    let run = env.state.agent().run_turn(env.chat, "call:fs_read {}\ncall:mutate {}\ncall:browser_snapshot {}\ncall:mutate {}\ncall:mutate {}").unwrap();
    assert_eq!(
        env.state.agent().wait(run).await.unwrap(),
        task_runtime::RunState::Succeeded
    );
    run
}

#[tokio::test]
async fn propose_review_save_list_replay_and_delete() {
    let env = setup();
    let id = source(&env).await;
    let path = format!("/v1/runs/{id}/workflow-proposal");
    assert_eq!(
        request(&env, Method::POST, &path, None, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, proposal) = request(&env, Method::POST, &path, None, true).await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert_eq!(proposal["workflow"]["steps"].as_array().unwrap().len(), 3);
    assert_eq!(proposal["skipped"].as_array().unwrap().len(), 2);
    assert!(
        WorkflowStore::new(env.dir.path())
            .list()
            .unwrap()
            .is_empty(),
        "proposals never save"
    );
    let save = json!({"run_id":id.0, "name":"recorded", "description":"call:fs_read {} call:mutate {} call:browser_snapshot {} call:mutate {} call:mutate {}", "params":proposal["workflow"]["params"], "steps":proposal["workflow"]["steps"]});
    assert_eq!(
        request(
            &env,
            Method::POST,
            "/v1/workflows",
            Some(save.clone()),
            true
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&env, Method::POST, "/v1/workflows", Some(save), true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(&env, Method::GET, "/v1/workflows", None, true)
            .await
            .1[0]["name"],
        "recorded"
    );
    assert!(
        !env.state
            .agent()
            .tool_names()
            .iter()
            .any(|n| n == "workflow_save")
    );
    let listed = env
        .state
        .agent()
        .tool_registry()
        .get("workflow_list")
        .unwrap()
        .call(&ToolContext::default(), json!({}))
        .await
        .unwrap();
    assert!(serde_json::to_string(&listed).unwrap().contains("recorded"));
    env.provider.saved.store(true, Ordering::SeqCst);
    let before = env.provider.executor_calls.load(Ordering::SeqCst);
    let run = env.state.agent().run_turn(env.chat, "call:fs_read {}\ncall:mutate {}\ncall:browser_snapshot {}\ncall:mutate {}\ncall:mutate {}").unwrap();
    assert_eq!(
        env.state.agent().wait(run).await.unwrap(),
        task_runtime::RunState::Succeeded
    );
    assert_eq!(
        env.provider.executor_calls.load(Ordering::SeqCst),
        before,
        "replay never calls executor model"
    );
    env.state.agent().flush_journal();
    let log = storage::read_event_log(env.dir.path(), run).unwrap();
    assert!(log.iter().any(|e| matches!(&e.event, protocol::Event::RouteDecided(r) if (r.level == "K2" || r.level == "K3") && r.route["kind"] == "workflow")));
    assert_eq!(
        log.iter()
            .filter(|e| matches!(&e.event, protocol::Event::WorkflowStepCompleted(_)))
            .count(),
        3
    );
    env.state.agent().remember_route(run).unwrap();
    assert_eq!(
        request(&env, Method::DELETE, "/v1/workflows/recorded", None, true)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&env, Method::GET, "/v1/workflows/recorded", None, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let cache = rusqlite::Connection::open(env.dir.path().join("router").join(
        format!("{}.sqlite", env.state.storage().get_chat(env.chat).unwrap().unwrap().bot_id),
    ))
    .unwrap();
    let cached: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM router_intent_cache WHERE workflow = 'recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        cached, 0,
        "workflow_changed must remove K1 entries on deletion"
    );
}

#[tokio::test]
async fn password_never_reaches_proposal_or_saved_toml_and_cannot_get_a_default() {
    let env = setup();
    let id = env
        .state
        .agent()
        .run_turn(env.chat, "call:mutate {\"password\":\"p4ssword-218\"}")
        .unwrap();
    assert_eq!(
        env.state.agent().wait(id).await.unwrap(),
        task_runtime::RunState::Succeeded
    );
    let (status, proposal) = request(
        &env,
        Method::POST,
        &format!("/v1/runs/{id}/workflow-proposal"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert!(!proposal.to_string().contains("p4ssword-218"));
    let mut params = proposal["workflow"]["params"].clone();
    params[0]["default"] = json!("do-not-persist");
    let mut input = json!({"run_id":id.0,"name":"password","description":"safe","params":params,"steps":proposal["workflow"]["steps"]});
    assert_eq!(
        request(
            &env,
            Method::POST,
            "/v1/workflows",
            Some(input.clone()),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    input["params"] = proposal["workflow"]["params"].clone();
    input["steps"] = json!([{"kind":"shell","command":"injected"}]);
    assert_eq!(
        request(
            &env,
            Method::POST,
            "/v1/workflows",
            Some(input.clone()),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    input["steps"] = proposal["workflow"]["steps"].clone();
    assert_eq!(
        request(&env, Method::POST, "/v1/workflows", Some(input), true)
            .await
            .0,
        StatusCode::CREATED
    );
    let toml = std::fs::read_to_string(env.dir.path().join("workflows/password.toml")).unwrap();
    assert!(!toml.contains("p4ssword-218"));
    assert!(!toml.contains("default ="));
    let (status, _) = request(
        &env,
        Method::POST,
        "/v1/workflows/password/run",
        Some(json!({"chat_id":env.chat.0,"params":{"recorded_secret_1":"a\"b"}})),
        true,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unrepresentable values must never be changed silently"
    );
    let (status, body) = request(&env, Method::POST, "/v1/workflows/password/run",
        Some(json!({"chat_id":env.chat.0,"params":{"recorded_secret_1":"reviewed value=with spaces\\and\nnewlines"}})), true).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let replay = RunId(body["run_id"].as_str().unwrap().parse().unwrap());
    assert_eq!(
        env.state.agent().wait(replay).await.unwrap(),
        task_runtime::RunState::Succeeded
    );
    let row = env.state.storage().get_run(replay).unwrap().unwrap();
    assert_eq!(
        row.kind, "turn",
        "human permissions must apply to the user action"
    );
    let steps = env.state.storage().list_steps(replay).unwrap();
    assert_eq!(
        steps[0].input["route"]["route"]["params"]["recorded_secret_1"],
        "reviewed value=with spaces\\and\nnewlines"
    );
}

#[tokio::test]
async fn short_passwords_do_not_corrupt_tool_correlation_ids() {
    let env = setup();
    let id = env
        .state
        .agent()
        .run_turn(env.chat, "call:mutate {\"password\":\"a\"}")
        .unwrap();
    assert_eq!(
        env.state.agent().wait(id).await.unwrap(),
        task_runtime::RunState::Succeeded
    );
    let (status, proposal) = request(
        &env,
        Method::POST,
        &format!("/v1/runs/{id}/workflow-proposal"),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert_eq!(proposal["workflow"]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(proposal["required_params"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_and_child_runs_conflict_and_run_endpoint_keeps_permissions() {
    let env = setup();
    let id = source(&env).await;
    let original = env.state.storage().get_run(id).unwrap().unwrap();
    for (state, parent) in [("failed", None), ("succeeded", Some(id))] {
        let mut row = original.clone();
        row.id = RunId::new();
        row.state = state.into();
        row.parent_run_id = parent;
        env.state.storage().insert_run(&row).unwrap();
        assert_eq!(
            request(
                &env,
                Method::POST,
                &format!("/v1/runs/{}/workflow-proposal", row.id),
                None,
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let (_, proposal) = request(
        &env,
        Method::POST,
        &format!("/v1/runs/{id}/workflow-proposal"),
        None,
        true,
    )
    .await;
    let save = json!({"run_id":id.0,"name":"recorded","description":"task","params":proposal["workflow"]["params"], "steps":proposal["workflow"]["steps"]});
    assert_eq!(
        request(&env, Method::POST, "/v1/workflows", Some(save), true)
            .await
            .0,
        StatusCode::CREATED
    );
    env.state
        .agent()
        .set_global_policy(permission_engine::Policy::read_only());
    let before = env.calls.load(Ordering::SeqCst);
    let (status, body) = request(
        &env,
        Method::POST,
        "/v1/workflows/recorded/run",
        Some(json!({"chat_id":env.chat.0,"params":{}})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let run = RunId(body["run_id"].as_str().unwrap().parse().unwrap());
    let _ = env.state.agent().wait(run).await.unwrap();
    assert_eq!(
        env.calls.load(Ordering::SeqCst),
        before,
        "read-only policy must deny replay writes"
    );
}
