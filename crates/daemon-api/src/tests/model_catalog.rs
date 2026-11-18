use super::*;
use provider_catalog::{ChatProvider, ProviderError, ProviderSettings};

const SECRET: &str = "fixture-provider-secret-123456";

#[test]
fn provider_models_openapi_declares_optional_query() {
    let document: Value = serde_json::from_str(&crate::openapi::openapi_json()).unwrap();
    let parameters = document["paths"]["/v1/providers"]["get"]["parameters"]
        .as_array()
        .unwrap();
    let chat_only = parameters
        .iter()
        .find(|p| p["name"] == "chat_only")
        .unwrap();
    assert_eq!(chat_only["in"], "query");
    assert_eq!(chat_only["required"], false);
}

#[derive(serde::Deserialize)]
struct Legacy {
    id: String,
    models: Vec<String>,
}

struct Models {
    fail: bool,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl ChatProvider for Models {
    fn id(&self) -> &'static str {
        "gemini"
    }

    async fn stream(
        &self,
        req: provider_catalog::ChatRequest,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<provider_catalog::ChatChunk, ProviderError>>,
        ProviderError,
    > {
        self.seen.lock().unwrap().push(req.model.0.clone());
        FakeProvider.stream(req).await
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        if self.fail {
            return Err(ProviderError::Connection(format!(
                "upstream refused {SECRET}"
            )));
        }
        Ok([
            "new-model-preview",
            "gemini-3.5-flash-lite",
            "gemini-4-carbon",
            "gemini-4-hydrogen",
            "gemini-4-carbon",
            "gemini-2.0-flash",
            "gemini-3.8-flash-tts",
            "text-embedding-004",
            "veo-3.1-generate-preview",
            "imagen-4.0-generate-001",
            SECRET,
        ]
        .map(str::to_owned)
        .into())
    }
}

fn catalog_setup(fail: bool) -> (T, Arc<std::sync::Mutex<Vec<String>>>) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = provider_catalog::ProviderConfig::default();
    cfg.upsert(ProviderSettings::new("gemini", 1));
    cfg.save(&provider_catalog::config_path(dir.path()))
        .unwrap();
    let store = Arc::new(Storage::open(dir.path()).unwrap());
    let redactor = agent_core::SharedRedactor::new(secret_store::Redactor::new());
    // Model a secret added/rotated after startup. The production refresh path
    // must be used; a store-only/static scrubber cannot satisfy this fixture.
    redactor.set_refresher(Arc::new(|redactor| redactor.register(SECRET)));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let agent = AgentLoop::new(
        Arc::clone(&store),
        ToolRegistry::new(),
        Providers::single(Arc::new(Models {
            fail,
            seen: Arc::clone(&seen),
        })),
        LoopConfig {
            redactor: Some(redactor),
            ..LoopConfig::default()
        },
    );
    let state = AppState::new(agent.clone(), store, DaemonConfig::new(TOKEN)).unwrap();
    state.set_owner_key(OWNER_KEY.into());
    (
        T {
            app: router(state.clone()),
            state,
            agent,
            _dir: dir,
        },
        seen,
    )
}

#[tokio::test]
async fn provider_models_api_keeps_legacy_shape_and_returns_ranked_names() {
    // The sentinel must reach the redactor, not disappear incidentally through
    // the non-chat filter (for example, an id containing "-live" is audio).
    assert_eq!(
        provider_catalog::model_metadata("gemini", &[SECRET.into()], true).len(),
        1
    );
    let (t, _) = catalog_setup(false);
    let response = call(&t, Method::GET, "/v1/providers", None).await;
    assert_eq!(response.status, StatusCode::OK);
    let json = response.json();
    let provider = &json["configured"][0];
    assert_eq!(
        provider["models"],
        json!([
            "gemini-4-carbon",
            "gemini-4-hydrogen",
            "gemini-3.5-flash-lite",
            "new-model-preview"
        ])
    );
    let details = &provider["model_details"];
    assert_eq!(
        details[0],
        json!({
            "id":"gemini-4-carbon", "display_name":"Gemini 4 Carbon", "kind":"chat",
            "recommended":true, "recommended_rank":1, "deprecated":false, "note":null
        })
    );
    assert_eq!(details[1]["recommended_rank"], 3);
    assert_eq!(details[3]["recommended"], false);
    assert_eq!(details[3]["display_name"], "New Model Preview");
    assert_eq!(details[3]["note"], "preview");
    assert!(
        !response
            .body
            .windows(SECRET.len())
            .any(|part| part == SECRET.as_bytes())
    );

    let legacy: Legacy = serde_json::from_value(provider.clone()).unwrap();
    assert_eq!(legacy.id, "gemini");
    assert_eq!(legacy.models.len(), 4);
}

#[tokio::test]
async fn provider_models_all_opt_out_exposes_nonchat_and_deprecation_metadata() {
    let (t, _) = catalog_setup(false);
    let response = call(&t, Method::GET, "/v1/providers?chat_only=false", None).await;
    assert_eq!(response.status, StatusCode::OK);
    let json = response.json();
    let models = json["configured"][0]["model_details"].as_array().unwrap();
    assert_eq!(models.len(), 9);
    assert!(models.iter().any(|m| m["kind"] == "embedding"));
    assert!(models.iter().any(|m| m["kind"] == "image"));
    assert!(models.iter().any(|m| m["kind"] == "video"));
    assert!(models.iter().any(|m| m["kind"] == "audio"));
    assert!(
        models
            .iter()
            .any(|m| m["id"] == "gemini-2.0-flash" && m["deprecated"] == true)
    );
    assert!(
        !response
            .body
            .windows(SECRET.len())
            .any(|part| part == SECRET.as_bytes())
    );
    assert_eq!(
        call(&t, Method::GET, "/v1/providers?chat_only=invalid", None)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn provider_models_failure_is_redacted_and_never_injects_curated_candidates() {
    let (t, _) = catalog_setup(true);
    let response = call(&t, Method::GET, "/v1/providers", None).await;
    let json = response.json();
    assert_eq!(json["configured"][0]["models"], json!([]));
    assert_eq!(json["configured"][0]["model_details"], json!([]));
    assert!(
        json["configured"][0]["models_error"]
            .as_str()
            .unwrap()
            .contains("upstream refused")
    );
    assert!(
        !response
            .body
            .windows(SECRET.len())
            .any(|part| part == SECRET.as_bytes())
    );
}

#[tokio::test]
async fn bot_with_model_missing_from_table_and_live_list_still_runs() {
    let (t, seen) = catalog_setup(false);
    let profile = PROFILE.replace("fake/echo", "gemini/owner-custom-model");
    let response = call(
        &t,
        Method::POST,
        "/v1/bots",
        Some(json!({"profile_toml":profile})),
    )
    .await;
    assert_eq!(response.status, StatusCode::CREATED);
    let bot = response.json()["id"].as_str().unwrap().to_owned();
    let response = call(&t, Method::POST, "/v1/chats", Some(json!({"bot_id":bot}))).await;
    let chat = response.json()["id"].as_str().unwrap().to_owned();
    assert!(
        run_and_reply(&t, &chat, "hello from custom model")
            .await
            .contains("hello from custom model")
    );
    assert_eq!(*seen.lock().unwrap(), ["gemini/owner-custom-model"]);
}

#[tokio::test]
async fn provider_models_query_preserves_existing_peer_roles() {
    let (t, _) = catalog_setup(false);
    for (role, allowed) in [
        (crate::PeerRole::Desktop, true),
        (crate::PeerRole::Phone, true),
        (crate::PeerRole::Node, false),
        (crate::PeerRole::Host, false),
    ] {
        for path in ["/v1/providers", "/v1/providers?chat_only=false"] {
            let request = Request::builder()
                .uri(path)
                .extension(crate::PeerIdentity {
                    key: "fixture-peer".into(),
                    role,
                })
                .body(Body::empty())
                .unwrap();
            let response = crate::peer_router(t.state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if allowed {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                },
                "{role:?} {path}"
            );
        }
    }
}
