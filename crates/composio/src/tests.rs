//! Service and tool tests against a fake Composio server on loopback.  No
//! network, no real key.

use std::sync::Arc;

use parking_lot::Mutex;
use secret_store::{MemoryStore, SecretStore};
use serde_json::{Value, json};
use tool_registry::{ToolContext, ToolError, ToolRegistry};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{ComposioError, ComposioService, ConnState, KEY_SECRET, ServiceConfig};

const KEY: &str = "ck_test_SECRETKEY_0123456789";

#[derive(Default)]
struct Rows(Mutex<Vec<(String, Value)>>);

impl crate::Audit for Rows {
    fn record(&self, action: &str, detail: Value) -> Result<(), String> {
        self.0.lock().push((action.to_owned(), detail));
        Ok(())
    }
}

struct Harness {
    svc: ComposioService,
    registry: ToolRegistry,
    store: Arc<MemoryStore>,
    rows: Arc<Rows>,
    dir: tempfile::TempDir,
    server: MockServer,
}

async fn harness() -> Harness {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::new());
    let registry = ToolRegistry::new();
    let rows = Arc::new(Rows::default());
    let svc = ComposioService::new(ServiceConfig {
        data_dir: dir.path().to_path_buf(),
        keys: Arc::new(crate::LocalKeySource::new(store.clone())),
        registry: registry.clone(),
        redact: Arc::new(|s| s.replace(KEY, "[REDACTED]")),
        audit: rows.clone(),
        loopback_base: Some(server.uri()),
    })
    .unwrap();
    Harness {
        svc,
        registry,
        store,
        rows,
        dir,
        server,
    }
}

async fn ok_key(h: &Harness) {
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .and(header("x-api-key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": []})))
        .mount(&h.server)
        .await;
}

fn accounts(status: &str) -> Value {
    json!({"items": [{"id": "ca_secret_conn_1", "status": status, "toolkit": {"slug": "github"}}]})
}

async fn mount_accounts(h: &Harness, status: &str) {
    h.server.reset().await;
    ok_key(h).await;
    Mock::given(method("GET"))
        .and(path("/connected_accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(accounts(status)))
        .mount(&h.server)
        .await;
    let mut items = Vec::new();
    for i in 0..30 {
        items.push(
            json!({"slug": format!("GITHUB_DO_THING_{i}"), "description": format!("thing {i}"),
            "input_parameters": {"type": "object", "properties": {"x": {"type": "string"}}}}),
        );
    }
    items.push(json!({"slug": "GITHUB_LIST_REPOSITORY_ISSUES", "description": "list issues",
        "input_parameters": {"type": "object", "properties": {"owner": {"type": "string"}, "repo": {"type": "string"}},
        "required": ["owner", "repo"]}}));
    Mock::given(method("GET"))
        .and(path("/tools"))
        .and(query_param("toolkit_slug", "github"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": items})))
        .mount(&h.server)
        .await;
}

fn ctx() -> ToolContext {
    ToolContext {
        run_id: "run-1".into(),
        bot_id: Some("bot-1".into()),
        ..ToolContext::default()
    }
}

/// Every place the key or a connection id could leak to, concatenated.
fn leak_surface(h: &Harness) -> String {
    use std::fmt::Write as _;
    let mut s = std::fs::read_to_string(h.svc.state_path()).unwrap_or_default();
    let _ = write!(s, "{:?}", h.rows.0.lock());
    for sp in h.registry.list() {
        let _ = write!(s, "{} {} {}", sp.name, sp.description, sp.input_schema);
    }
    let _ = write!(s, "{:?}", h.svc.apps());
    s
}

async fn connected(h: &Harness) {
    ok_key(h).await;
    h.svc.set_key(KEY).await.unwrap();
    mount_accounts(h, "INITIATED").await;
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .and(query_param("toolkit_slug", "github"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [{"id": "ac_1"}]})))
        .mount(&h.server)
        .await;
    Mock::given(method("POST")).and(path("/connected_accounts/link"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "link_token": "lt_secret_link_token", "redirect_url": "https://connect.composio.dev/link/lk_1",
            "expires_at": "2030-01-01T00:00:00Z", "connected_account_id": "ca_secret_conn_1"})))
        .mount(&h.server).await;
    Mock::given(method("POST"))
        .and(path("/auth_configs"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"auth_config": {"id": "ac_1"}})),
        )
        .mount(&h.server)
        .await;
    h.svc.connect("github").await.unwrap();
    mount_accounts(h, "ACTIVE").await;
    h.svc.app_status("github").await.unwrap();
}

#[tokio::test]
async fn a_rejected_key_is_not_stored_and_a_good_one_is_never_echoed() {
    let h = harness().await;
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": format!("bad key {KEY}")}})),
        )
        .mount(&h.server)
        .await;
    let err = h.svc.set_key(KEY).await.unwrap_err();
    assert!(matches!(err, ComposioError::Invalid(_)), "{err}");
    assert!(!err.to_string().contains(KEY));
    assert!(!h.svc.configured());
    assert!(h.store.get(KEY_SECRET).unwrap().is_none());
    for bad in ["", "has space", "line\nbreak", &"x".repeat(600)] {
        assert!(matches!(
            h.svc.set_key(bad).await,
            Err(ComposioError::Invalid(_))
        ));
    }
    h.server.reset().await;
    ok_key(&h).await;
    h.svc.set_key(&format!("  {KEY}  ")).await.unwrap();
    assert!(h.svc.configured());
    assert_eq!(h.store.get(KEY_SECRET).unwrap().unwrap().expose(), KEY);
    assert!(!leak_surface(&h).contains(KEY));
    // Positive control: the scan really sees a planted key.
    std::fs::write(
        h.dir.path().join("composio.toml"),
        format!("planted = \"{KEY}\""),
    )
    .unwrap();
    assert!(leak_surface(&h).contains(KEY));
}

#[tokio::test]
async fn connect_pending_active_and_expired_drive_the_tools() {
    let h = harness().await;
    ok_key(&h).await;
    h.svc.set_key(KEY).await.unwrap();
    // Not connected: no tools but the two native ones, and they are hidden.
    assert_eq!(h.svc.registered_tools("github"), Vec::<String>::new());
    connected(&h).await;
    let apps = h.svc.apps();
    assert_eq!(apps[0].state, ConnState::Active);
    assert!(
        !apps[0].native_first,
        "native_first is empty until a zero-setup path exists"
    );
    let tools = h.svc.registered_tools("github");
    // Cap 20; the known read-only action is selected first although listed last.
    assert_eq!(tools.len(), 20);
    assert!(tools.contains(&"app:github/list_repository_issues".to_owned()));
    let t = h.registry.get("app:github/list_repository_issues").unwrap();
    assert!(t.visible_to(Some("bot-1")));
    assert!(
        !t.always_ask(),
        "a known read-only action may take a standing grant"
    );
    assert!(
        h.registry
            .get("app:github/do_thing_0")
            .unwrap()
            .always_ask()
    );
    // Disabled: tools vanish.
    h.svc.set_enabled("github", false).await.unwrap();
    assert!(h.registry.get("app:github/do_thing_0").is_none());
    h.svc.set_enabled("github", true).await.unwrap();
    assert!(h.registry.get("app:github/do_thing_0").is_some());
    // Expired: tools vanish and the state says so.
    mount_accounts(&h, "EXPIRED").await;
    assert_eq!(
        h.svc.app_status("github").await.unwrap().state,
        ConnState::Expired
    );
    assert!(h.registry.get("app:github/do_thing_0").is_none());
    let actions: Vec<String> = h.rows.0.lock().iter().map(|r| r.0.clone()).collect();
    assert!(actions.contains(&"composio.connected".to_owned()));
    assert!(actions.contains(&"composio.expired".to_owned()));
    let surface = leak_surface(&h);
    for secret in [KEY, "lt_secret_link_token", "ca_secret_conn_1"] {
        assert!(!surface.contains(secret), "{secret} leaked");
    }
}

#[tokio::test]
async fn a_connect_link_outside_composios_domain_is_discarded() {
    let h = harness().await;
    ok_key(&h).await;
    h.svc.set_key(KEY).await.unwrap();
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .and(query_param("toolkit_slug", "slack"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [{"id": "ac_1"}]})))
        .mount(&h.server)
        .await;
    for bad in [
        "http://169.254.169.254/latest/meta-data",
        "https://evil.example.com/x",
        "http://connect.composio.dev/x",
    ] {
        h.server.reset().await;
        Mock::given(method("GET"))
            .and(path("/auth_configs"))
            .and(query_param("toolkit_slug", "slack"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"items": [{"id": "ac_1"}]})),
            )
            .mount(&h.server)
            .await;
        Mock::given(method("POST")).and(path("/connected_accounts/link"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"redirect_url": bad, "expires_at": "x", "link_token": "t", "connected_account_id": "c"})))
            .mount(&h.server).await;
        let err = h.svc.connect("slack").await.unwrap_err();
        assert!(matches!(err, ComposioError::Upstream(_)), "{bad}: {err}");
        assert!(!err.to_string().contains("169.254"));
    }
    assert!(h.svc.apps().is_empty(), "a failed connect opts nothing in");
    assert!(h.svc.connect("../etc").await.is_err());
}

#[tokio::test]
async fn execute_sends_only_host_chosen_identity_and_validated_arguments() {
    let h = harness().await;
    connected(&h).await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_LIST_REPOSITORY_ISSUES"))
        .and(header("x-api-key", KEY))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"data": {"issues": ["a"]}, "successful": true, "error": null}),
            ),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let t = h.registry.get("app:github/list_repository_issues").unwrap();
    // Model-chosen identity or unknown arguments are refused before anything is sent.
    for bad in [
        json!({"owner": "o", "repo": "r", "user_id": "someone-else"}),
        json!({"owner": "o"}),
        json!({"owner": 1, "repo": "r"}),
    ] {
        assert!(matches!(
            t.call(&ctx(), bad).await,
            Err(ToolError::InvalidInput(_))
        ));
    }
    let out = t
        .call(&ctx(), json!({"owner": "o", "repo": "r"}))
        .await
        .unwrap();
    let text = format!("{:?}", out.content);
    assert!(text.contains("Untrusted") && text.contains("issues"));
    let sent = h.server.received_requests().await.unwrap();
    let call = sent
        .iter()
        .find(|r| r.url.path().starts_with("/tools/execute"))
        .unwrap();
    let body: Value = serde_json::from_slice(&call.body).unwrap();
    assert_eq!(body["arguments"], json!({"owner": "o", "repo": "r"}));
    assert!(body["user_id"].as_str().unwrap().starts_with("praxi-"));
    let rows = h.rows.0.lock();
    let started = rows.iter().filter(|r| r.0 == "composio.call").count();
    assert_eq!(started, 2, "started + ok rows");
    let row = &rows.iter().rfind(|r| r.0 == "composio.call").unwrap().1;
    assert_eq!(row["status"], "ok");
    assert_eq!(row["args_sha256"].as_str().unwrap().len(), 64);
    assert!(
        !row.to_string().contains("\"repo\""),
        "arguments are hashed, never stored"
    );
}

#[tokio::test]
async fn approval_card_shows_app_action_and_exact_arguments() {
    let h = harness().await;
    connected(&h).await;
    let t = h.registry.get("app:github/do_thing_3").unwrap();
    let args = json!({"x": "exact value"});
    let d = t.approval_details(&ctx(), &args).await.unwrap().unwrap();
    assert_eq!(d["app"], "github");
    assert_eq!(d["action"], "do_thing_3");
    assert_eq!(d["via"], "Composio");
    assert_eq!(d["arguments"], args);
    assert!(
        t.approval_details(&ctx(), &json!({"nope": 1}))
            .await
            .is_err()
    );
    assert_eq!(t.spec().effect, tool_registry::Effect::Destructive);
}

#[tokio::test]
async fn find_and_run_reach_actions_without_a_direct_tool() {
    let h = harness().await;
    connected(&h).await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_DO_THING_29"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": {"done": true}, "successful": true})),
        )
        .mount(&h.server)
        .await;
    let find = h.registry.get("apps_find_action").unwrap();
    let out = find
        .call(&ctx(), json!({"app": "github", "query": "thing 29"}))
        .await
        .unwrap();
    let text = format!("{:?}", out.content);
    assert!(
        text.contains("do_thing_29") && text.contains("\\\"direct_tool\\\":false"),
        "{text}"
    );
    let run = h.registry.get("apps_run_action").unwrap();
    assert!(run.always_ask());
    assert!(
        run.call(&ctx(), json!({"app": "github", "action": "does_not_exist"}))
            .await
            .is_err()
    );
    assert!(
        run.call(&ctx(), json!({"app": "slack", "action": "x"}))
            .await
            .is_err(),
        "unconnected app"
    );
    let ok = run
        .call(
            &ctx(),
            json!({"app": "github", "action": "do_thing_29", "arguments": {"x": "v"}}),
        )
        .await
        .unwrap();
    assert!(format!("{:?}", ok.content).contains("done"));
    // Hidden from a bot the app is not enabled for.
    assert!(find.visible_to(Some("bot-1")));
}

#[tokio::test]
async fn rate_limit_oversize_and_redirects_are_handled() {
    let h = harness().await;
    connected(&h).await;
    let t = h.registry.get("app:github/list_repository_issues").unwrap();
    let args = json!({"owner": "o", "repo": "r"});
    h.server.reset().await;
    ok_key(&h).await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_LIST_REPOSITORY_ISSUES"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
        .mount(&h.server)
        .await;
    let err = t.call(&ctx(), args.clone()).await.unwrap_err().to_string();
    assert!(err.contains("retry after 7"), "{err}");
    h.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_LIST_REPOSITORY_ISSUES"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 2 * 1024 * 1024]))
        .mount(&h.server)
        .await;
    assert!(
        t.call(&ctx(), args.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("allowed size")
    );
    // A redirect is not followed: the target never gets a request (or the key).
    h.server.reset().await;
    let target = format!("{}/stolen", h.server.uri());
    Mock::given(method("POST"))
        .and(path("/tools/execute/GITHUB_LIST_REPOSITORY_ISSUES"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", target.as_str()))
        .mount(&h.server)
        .await;
    Mock::given(path("/stolen"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&h.server)
        .await;
    assert!(t.call(&ctx(), args).await.is_err());
}

#[tokio::test]
async fn disconnect_deletes_the_remote_account_and_forgets_the_app() {
    let h = harness().await;
    connected(&h).await;
    Mock::given(method("DELETE"))
        .and(path("/connected_accounts/ca_secret_conn_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .expect(1)
        .mount(&h.server)
        .await;
    assert!(h.svc.disconnect("github").await.unwrap());
    assert_eq!(h.svc.apps(), vec![]);
    assert!(h.registry.get("app:github/do_thing_0").is_none());
    assert!(matches!(
        h.svc.disconnect("github").await,
        Err(ComposioError::NotFound(_))
    ));
}

#[tokio::test]
async fn removing_the_key_hides_every_tool() {
    let h = harness().await;
    connected(&h).await;
    h.svc.remove_key().await.unwrap();
    assert!(!h.svc.configured());
    assert!(h.store.get(KEY_SECRET).unwrap().is_none());
    assert!(h.registry.get("app:github/do_thing_0").is_none());
    let t = h.registry.get("apps_find_action").unwrap();
    assert!(!t.visible_to(None));
    assert!(matches!(
        h.svc.connect("github").await,
        Err(ComposioError::NotConfigured)
    ));
}

#[tokio::test]
async fn the_catalog_is_paged_searchable_and_cached() {
    let h = harness().await;
    ok_key(&h).await;
    h.svc.set_key(KEY).await.unwrap();
    Mock::given(method("GET")).and(path("/toolkits")).and(query_param("search", "insta"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"slug": "instagram", "name": "Instagram", "meta": {"description": "Photos", "categories": [{"name": "social"}, "media"], "tools_count": 42}}],
            "next_cursor": "c2"})))
        .expect(1).mount(&h.server).await;
    for _ in 0..2 {
        let p = h.svc.toolkits(Some("insta"), None, None, 50).await.unwrap();
        assert_eq!(p.items[0].slug, "instagram");
        assert_eq!(p.items[0].categories, ["social", "media"]);
        assert_eq!(p.next_cursor.as_deref(), Some("c2"));
    }
}
