//! Host administration: who may send keys to a host, and where the key lands.
use super::*;
use crate::host::{HostConfig, HostMode, HostState};
use peer_network::{PairTicket, PeerConfig, PeerNetwork, RelayConfig, Role};

const KEY: &str = "sk-SECRET-VALUE-0123456789";
const OWNER: &str = super::OWNER_KEY;

struct Rig {
    t: T,
    net: PeerNetwork,
    store: Arc<secret_store::MemoryStore>,
    admin_key: String,
    legacy_key: String,
    _joiners: Vec<PeerNetwork>,
}

async fn joiner(t: &T, name: &str, admin: bool, net: &PeerNetwork) -> (PeerNetwork, String) {
    let mut config = PeerConfig::new(t._dir.path().join(name));
    config.local_role = Role::Desktop;
    config.relay = RelayConfig::Disabled;
    config.bind_addr = Some("127.0.0.1:0".parse().unwrap());
    let j = PeerNetwork::start(config, None).await.unwrap();
    let invite = net
        .create_invite_with_options(Role::Desktop, std::time::Duration::from_secs(60), admin)
        .await;
    j.pair(&PairTicket::parse(&invite.ticket).unwrap())
        .await
        .unwrap();
    let key = peer_network::key_to_string(&j.id());
    (j, key)
}

async fn rig() -> Rig {
    let t = setup_with(|data| {
        HostConfig {
            mode: HostMode::Host,
            state: HostState::Active,
            ..HostConfig::default()
        }
        .save(data)
        .unwrap();
    });
    let store = Arc::new(secret_store::MemoryStore::new());
    t.state.set_secret_store(store.clone());
    t.state.set_owner_key(OWNER.into());
    t.state.set_provider_host(crate::ProviderHost {
        catalog: provider_catalog::Catalog::from_entries([catalog_entry("second")]),
        reload: Box::new(|| Ok(Providers::single(Arc::new(FakeProvider)))),
    });
    let mut config = PeerConfig::new(t._dir.path().join("peer"));
    config.local_role = Role::Host;
    config.relay = RelayConfig::Disabled;
    config.bind_addr = Some("127.0.0.1:0".parse().unwrap());
    let net = PeerNetwork::start(config, None).await.unwrap();
    t.state.leader().set_network(net.clone());
    let (a, admin_key) = joiner(&t, "j1", true, &net).await;
    let (b, legacy_key) = joiner(&t, "j2", false, &net).await;
    Rig {
        t,
        net,
        store,
        admin_key,
        legacy_key,
        _joiners: vec![a, b],
    }
}

async fn peer_call(
    t: &T,
    peer: Option<(&str, crate::PeerRole)>,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Resp {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some((key, role)) = peer {
        b = b.extension(crate::PeerIdentity {
            key: key.into(),
            role,
        });
    }
    let req = b
        .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap();
    let r = crate::peer_router(t.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = r.status();
    let headers = r.headers().clone();
    let body = axum::body::to_bytes(r.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

#[tokio::test]
async fn admin_desktop_stores_the_key_in_the_secret_store_and_nowhere_else() {
    let r = rig().await;
    let path = "/v1/admin/providers/second";
    let body = json!({"api_key": KEY, "model": "m1", "make_default": true});
    let ok = peer_call(
        &r.t,
        Some((&r.admin_key, crate::PeerRole::Desktop)),
        Method::PUT,
        path,
        Some(body.clone()),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(
        !String::from_utf8_lossy(&ok.body).contains(KEY),
        "never echoed"
    );
    assert_eq!(ok.json()["has_key"], true);
    let stored = secret_store::SecretStore::get(&*r.store, "provider.second.api_key")
        .unwrap()
        .unwrap();
    assert_eq!(stored.expose(), KEY);
    let toml = std::fs::read_to_string(provider_catalog::config_path(r.t._dir.path())).unwrap();
    assert!(!toml.contains(KEY));
    let audit = format!(
        "{:?}",
        r.t.state.storage().list_audit(r.t.state.user_id()).unwrap()
    );
    assert!(audit.contains("host.admin.provider_set") && !audit.contains(KEY));

    // Refused for every other caller, and nothing is written.
    secret_store::SecretStore::delete(&*r.store, "provider.second.api_key").unwrap();
    for (peer, code) in [
        (
            Some((r.legacy_key.as_str(), crate::PeerRole::Desktop)),
            "host_admin_required",
        ),
        (Some((r.admin_key.as_str(), crate::PeerRole::Phone)), "-"),
        (Some((r.admin_key.as_str(), crate::PeerRole::Node)), "-"),
        (None, "-"),
    ] {
        let denied = peer_call(&r.t, peer, Method::PUT, path, Some(body.clone())).await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN, "{code}");
        if code != "-" {
            assert_eq!(denied.json()["error"]["code"], code);
        }
    }
    assert!(
        secret_store::SecretStore::get(&*r.store, "provider.second.api_key")
            .unwrap()
            .is_none()
    );
    // The owner can withdraw the capability; the live peer loses it at once.
    let k = peer_network::key_from_string(&r.admin_key).unwrap();
    r.net.set_device_admin(&k, false).unwrap();
    let gone = peer_call(
        &r.t,
        Some((&r.admin_key, crate::PeerRole::Desktop)),
        Method::PUT,
        path,
        Some(body),
    )
    .await;
    assert_eq!(gone.json()["error"]["code"], "host_admin_required");
    r.net.shutdown().await;
}

struct Recorder(parking_lot::Mutex<Vec<(String, String, Option<Value>)>>);

#[async_trait]
impl crate::host_links::HostLinks for Recorder {
    fn list(&self) -> Vec<crate::host_links::HostLinkInfo> {
        vec![crate::host_links::HostLinkInfo {
            computer_access: false,
            computer_access_always: false,
            computer_browser: false,
            computer_commands: false,
            computer_files: false,
            id: "abc123".into(),
            name: "h".into(),
            state: crate::host_links::HostLinkState::Idle,
            detail: None,
        }]
    }
    async fn pair(
        &self,
        _: &str,
    ) -> Result<crate::host_links::HostLinkInfo, crate::host_links::HostLinkError> {
        Err(crate::host_links::HostLinkError::NotFound)
    }
    async fn connect(
        &self,
        _: &str,
    ) -> Result<crate::host_links::HostLinkConnection, crate::host_links::HostLinkError> {
        Err(crate::host_links::HostLinkError::NotFound)
    }
    fn disconnect(&self, _: &str) -> bool {
        false
    }
    async fn call(
        &self,
        _: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<crate::host_links::HostCall, crate::host_links::HostLinkError> {
        self.0.lock().push((method.into(), path.into(), body));
        Ok(crate::host_links::HostCall {
            status: 200,
            body: json!([]),
        })
    }
}

#[tokio::test]
async fn copy_setup_is_local_owner_only_and_sends_only_ticked_keys() {
    let _turn = MOVE_TURN.lock().await;
    let t = setup();
    let store = Arc::new(secret_store::MemoryStore::new());
    t.state.set_secret_store(store.clone());
    t.state.set_owner_key(OWNER.into());
    let mut cfg = provider_catalog::ProviderConfig::default();
    for id in ["alpha", "beta"] {
        let name = provider_catalog::api_key_secret_name(id);
        secret_store::SecretStore::set(&*store, &name, &format!("key-of-{id}-xxxxxx")).unwrap();
        let mut s = provider_catalog::ProviderSettings::new(id, 1);
        s.key = Some(secret_store::SecretRef::new(name).unwrap());
        cfg.upsert(s);
    }
    cfg.save(&provider_catalog::config_path(t._dir.path()))
        .unwrap();
    let rec = Arc::new(Recorder(parking_lot::Mutex::default()));
    t.state.set_host_links(rec.clone());
    let url = "/v1/hosts/abc123/copy-setup";
    // A paired peer (so a bot acting through one) is refused; the MCP facade cannot reach it.
    for role in [crate::PeerRole::Desktop, crate::PeerRole::Phone] {
        let r = peer_call(
            &t,
            Some(("k", role)),
            Method::POST,
            url,
            Some(json!({"providers":["alpha"]})),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    assert!(!crate::mcp_server::scoped_path_allowed(&Method::POST, url));
    assert!(rec.0.lock().is_empty());
    // The general API token alone cannot start an export.
    let r = call_token_only(&t, Method::POST, url, Some(json!({"providers":["alpha"]}))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["error"]["code"], "owner_key_required");
    assert!(rec.0.lock().is_empty());
    let (k, v) = auth();
    let owner = [(k, v.as_str()), ("x-praxi-owner-key", OWNER)];
    // A host that is not a paired one is refused.
    let r = send(
        &t.app,
        Method::POST,
        "/v1/hosts/zzz/copy-setup",
        Some(json!({"providers":["alpha"]})),
        &owner,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = send(
        &t.app,
        Method::POST,
        url,
        Some(json!({"providers":["alpha"]})),
        &owner,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let all = format!("{:?}", rec.0.lock().clone());
    assert!(all.contains("key-of-alpha-xxxxxx") && !all.contains("key-of-beta"));
    assert_eq!(r.json()["items"][0]["status"], "done");
    assert!(!String::from_utf8_lossy(&r.body).contains("key-of-alpha"));
}

#[tokio::test]
async fn the_general_token_cannot_issue_admin_invites_or_grants() {
    let r = rig().await;
    let (k, v) = auth();
    let bearer = [(k, v.as_str()), ("x-test-token-only", "1")];
    let owner = [(k, v.as_str()), ("x-praxi-owner-key", OWNER)];
    let grant = format!("/v1/host/devices/{}/admin", r.legacy_key);
    let body = || Some(json!({"enabled": true}));
    let denied = send(&r.t.app, Method::PUT, &grant, body(), &bearer).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.json()["error"]["code"], "owner_key_required");
    let k2 = peer_network::key_from_string(&r.legacy_key).unwrap();
    assert!(!r.net.device(&k2).unwrap().admin);
    let invite = Some(json!({"role": "desktop", "admin": true}));
    let denied = send(
        &r.t.app,
        Method::POST,
        "/v1/host/invite",
        invite.clone(),
        &bearer,
    )
    .await;
    assert_eq!(
        denied.json()["error"]["code"],
        "owner_key_required",
        "{} {}",
        denied.status,
        String::from_utf8_lossy(&denied.body)
    );
    // Since card 262 every invite (pairing a device) needs the owner key.
    let plain_denied = send(
        &r.t.app,
        Method::POST,
        "/v1/host/invite",
        Some(json!({"role": "desktop"})),
        &bearer,
    )
    .await;
    assert_eq!(plain_denied.status, StatusCode::FORBIDDEN);
    let plain = send(
        &r.t.app,
        Method::POST,
        "/v1/host/invite",
        Some(json!({"role": "desktop"})),
        &owner,
    )
    .await;
    assert_eq!(plain.status, StatusCode::OK);
    let ok = send(&r.t.app, Method::PUT, &grant, body(), &owner).await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert!(r.net.device(&k2).unwrap().admin);
    let ok = send(&r.t.app, Method::POST, "/v1/host/invite", invite, &owner).await;
    assert_eq!(ok.status, StatusCode::OK);
    r.net.shutdown().await;
}

#[tokio::test]
async fn non_admin_peers_cannot_patch_authority_files_or_engine() {
    let r = rig().await;
    let created = call(
        &r.t,
        Method::POST,
        "/v1/bots",
        Some(json!({"profile_toml": "[bot]
id=\"t\"
name=\"T\"
[models]
executor=\"fake/echo\"
"})),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let bot = created.json();
    let path = format!("/v1/bots/{}", bot["id"].as_str().unwrap());
    let ops = |op: Value| json!({"expected_version": 1, "ops": [op]});
    let set = |p: &str, v: Value| json!({"op":"set","path":p,"value":v});
    let cases = [
        set("/permissions/preset", json!("full")),
        set("/tools/allow", json!(["*"])),
        set("/files", json!({"mode": "everything"})),
        set("/models/executor", json!("cli:claude/x")),
        set(
            "/engine",
            json!({"mode": "native_tools", "level": "full", "consent": {"at_ms": 1}}),
        ),
    ];
    for peer in [
        (r.legacy_key.as_str(), crate::PeerRole::Desktop),
        (r.admin_key.as_str(), crate::PeerRole::Phone),
    ] {
        for case in &cases {
            let resp = peer_call(
                &r.t,
                Some(peer),
                Method::PATCH,
                &path,
                Some(ops(case.clone())),
            )
            .await;
            assert!(
                resp.status == StatusCode::FORBIDDEN
                    || resp.status == StatusCode::UNPROCESSABLE_ENTITY,
                "{case}: {}",
                resp.status
            );
        }
    }
    // Nothing changed, and the owner's ordinary edit still works. Engine changes
    // are refused even to the local owner (they have their own endpoint).
    let got = call(&r.t, Method::GET, &path, None).await.json();
    assert_eq!(got["version"], 1);
    let engine = call(
        &r.t,
        Method::PATCH,
        &path,
        Some(ops(set("/engine", json!({"mode":"native_tools"})))),
    )
    .await;
    assert_eq!(engine.status, StatusCode::UNPROCESSABLE_ENTITY);
    let ok = call(
        &r.t,
        Method::PATCH,
        &path,
        Some(ops(set("/bot/persona", json!("Calm.")))),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    r.net.shutdown().await;
}

// ------------------------------------------------ 270-B: schedules and chats move too

/// The host's own router, called as a paired desktop: what the real peer link does.
/// `copy-setup` refuses a second concurrent move (one process-wide guard), so the tests that
/// run one take turns.
static MOVE_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

static LOSE_ACTIVATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LOSE_RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Loopback {
    host: AppState,
    key: String,
}

#[async_trait]
impl crate::host_links::HostLinks for Loopback {
    fn list(&self) -> Vec<crate::host_links::HostLinkInfo> {
        vec![crate::host_links::HostLinkInfo {
            computer_access: false,
            computer_access_always: false,
            id: "h1".into(),
            name: "Oracle".into(),
            state: crate::host_links::HostLinkState::Idle,
            detail: None,
        }]
    }
    async fn pair(
        &self,
        _: &str,
    ) -> Result<crate::host_links::HostLinkInfo, crate::host_links::HostLinkError> {
        Err(crate::host_links::HostLinkError::NotFound)
    }
    async fn connect(
        &self,
        _: &str,
    ) -> Result<crate::host_links::HostLinkConnection, crate::host_links::HostLinkError> {
        Err(crate::host_links::HostLinkError::NotFound)
    }
    fn disconnect(&self, _: &str) -> bool {
        false
    }
    async fn call(
        &self,
        _id: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<crate::host_links::HostCall, crate::host_links::HostLinkError> {
        // A lost response / unreachable host, for the staged-handoff tests.
        let lost = [(&LOSE_ACTIVATE, "/activate"), (&LOSE_RELEASE, "/release")];
        if lost.iter().any(|(flag, tail)| {
            flag.load(std::sync::atomic::Ordering::SeqCst) && path.ends_with(tail)
        }) {
            return Err(crate::host_links::HostLinkError::Failed("lost".into()));
        }
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .extension(crate::PeerIdentity {
                key: self.key.clone(),
                role: crate::PeerRole::Desktop,
            })
            .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
            .unwrap();
        let r = crate::peer_router(self.host.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = r.status().as_u16();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        Ok(crate::host_links::HostCall {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        })
    }
}

async fn owner_post(t: &T, path: &str, body: Value) -> Resp {
    let (k, v) = auth();
    send(
        &t.app,
        Method::POST,
        path,
        Some(body),
        &[(k, v.as_str()), ("x-praxi-owner-key", OWNER)],
    )
    .await
}

fn add_message(t: &T, chat: &str, role: &str, text: &str, at: i64) {
    t.state
        .storage()
        .insert_message(&storage::Message {
            id: uuid::Uuid::now_v7(),
            user_id: t.state.user_id(),
            chat_id: protocol::ChatId(chat.parse().unwrap()),
            run_id: None,
            role: role.into(),
            content: text.into(),
            created_at_ms: at,
            exposure_untrusted: false,
            exposure_private: false,
            exposure_sources: vec![],
        })
        .unwrap();
}

async fn local_with_work(host: &Rig, key: &str) -> (T, String, String, Vec<String>) {
    let local = setup();
    local.state.set_host_links(Arc::new(Loopback {
        host: host.t.state.clone(),
        key: key.into(),
    }));
    let (bot, chat) = make_chat(&local).await;
    add_message(&local, &chat, "user", "good morning", 1_700_000_000_000);
    add_message(
        &local,
        &chat,
        "assistant",
        "good morning, boss",
        1_700_000_005_000,
    );
    let mut ids = Vec::new();
    for (name, spec, enabled) in [
        ("nightly", json!({"every": {"seconds": 3600}}), true),
        (
            "weekday report",
            json!({"cron": "0 9 * * 1-5 Europe/Istanbul"}),
            false,
        ),
    ] {
        let r = call(
            &local,
            Method::POST,
            "/v1/schedules",
            Some(json!({"bot_id": bot, "chat_id": chat, "name": name, "spec": spec, "enabled": enabled,
                "payload": {"kind": "agent_prompt", "text": "do the thing"}})),
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
        ids.push(r.json()["id"].as_str().unwrap().to_owned());
    }
    (local, bot, chat, ids)
}

fn report_line(r: &Resp, kind: &str) -> Value {
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind)
        .cloned()
        .unwrap_or(Value::Null)
}

fn host_messages(host: &Rig, chat: &str) -> Vec<storage::Message> {
    host.t
        .state
        .storage()
        .list_messages(protocol::ChatId(chat.parse().unwrap()))
        .unwrap()
}

#[tokio::test]
async fn moving_schedules_and_chats_disables_the_local_copies_and_a_second_sync_adds_nothing() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, chat, ids) = local_with_work(&host, &host.admin_key).await;
    let body = json!({"bots": [bot], "schedules": true, "chats": true});

    let first = owner_post(&local, "/v1/hosts/h1/copy-setup", body.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json());
    assert_eq!(report_line(&first, "bot")["status"], "done");
    assert_eq!(report_line(&first, "schedules")["count"], 2);
    assert_eq!(report_line(&first, "chats")["count"], 2);

    // The host has both schedules under the SAME ids, on its own bot, with the zone and the
    // paused state intact and its own next-run time.
    let on_host = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    let on_host = on_host.as_array().unwrap();
    assert_eq!(on_host.len(), 2);
    let paused = on_host.iter().find(|s| s["id"] == ids[1]).unwrap();
    assert_eq!(paused["enabled"], false);
    assert_eq!(
        paused["spec"],
        json!({"cron": "0 9 * * 1-5 Europe/Istanbul"})
    );
    assert_eq!(paused["next_run_at_ms"], Value::Null);
    let live = on_host.iter().find(|s| s["id"] == ids[0]).unwrap();
    assert_eq!(live["enabled"], true);
    assert!(live["next_run_at_ms"].as_i64().unwrap() > 0);
    assert_ne!(live["bot_id"], json!(bot), "the host has its own bot id");
    assert_eq!(live["chat_id"], json!(chat), "the chat kept its id");

    // The chat looks the same: authors, text and timestamps.
    let msgs = host_messages(&host, &chat);
    let got: Vec<_> = msgs
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str(), m.created_at_ms))
        .collect();
    assert_eq!(
        got,
        vec![
            ("user", "good morning", 1_700_000_000_000),
            ("assistant", "good morning, boss", 1_700_000_005_000)
        ]
    );

    // Local copies: disabled (not deleted) and marked.
    for id in &ids {
        let sc = call(&local, Method::GET, &format!("/v1/schedules/{id}"), None)
            .await
            .json();
        assert_eq!(sc["enabled"], false, "{sc}");
        assert_eq!(sc["moved_to"], "Oracle");
    }

    // Idempotent: nothing is created twice.
    let second = owner_post(&local, "/v1/hosts/h1/copy-setup", body).await;
    assert_eq!(report_line(&second, "schedules")["count"], 0);
    assert_eq!(report_line(&second, "schedules")["existing"], 2);
    assert_eq!(report_line(&second, "chats")["count"], 0);
    let after = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    assert_eq!(after.as_array().unwrap().len(), 2);
    assert_eq!(host_messages(&host, &chat).len(), 2);
    // The paused one stays paused on the host after the local copy was disabled.
    let sc = call(
        &host.t,
        Method::GET,
        &format!("/v1/schedules/{}", ids[1]),
        None,
    )
    .await
    .json();
    assert_eq!(sc["enabled"], false);
    // Only counts reach the audit log.
    let audit = format!(
        "{:?}",
        host.t
            .state
            .storage()
            .list_audit(host.t.state.user_id())
            .unwrap()
    );
    assert!(audit.contains("host.admin.import_chats") && !audit.contains("good morning"));
}

#[tokio::test]
async fn a_desktop_without_host_admin_cannot_import_and_nothing_is_disabled_locally() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    for peer in [
        (host.legacy_key.as_str(), crate::PeerRole::Desktop),
        (host.admin_key.as_str(), crate::PeerRole::Phone),
    ] {
        for path in ["/v1/admin/import/schedules", "/v1/admin/import/chats"] {
            let body = if path.ends_with("schedules") {
                json!({"schedules": []})
            } else {
                json!({})
            };
            let r = peer_call(&host.t, Some(peer), Method::POST, path, Some(body)).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
        }
    }
    let (local, bot, _chat, ids) = local_with_work(&host, &host.legacy_key).await;
    let r = owner_post(
        &local,
        "/v1/hosts/h1/copy-setup",
        json!({"bots": [bot], "schedules": true, "chats": true}),
    )
    .await;
    // The host refuses at the first step; nothing after it is attempted.
    let bot_line = report_line(&r, "bot");
    assert_eq!(bot_line["status"], "failed", "{:?}", r.json());
    assert_eq!(bot_line["code"], "host_admin_required");
    assert_eq!(report_line(&r, "schedules"), Value::Null);
    let none = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    assert_eq!(none.as_array().unwrap().len(), 0);
    let sc = call(
        &local,
        Method::GET,
        &format!("/v1/schedules/{}", ids[0]),
        None,
    )
    .await
    .json();
    assert_eq!(
        sc["enabled"], true,
        "a failed move leaves the local schedule running"
    );
    assert!(sc.get("moved_to").is_none());
}

fn lose(flag: &std::sync::atomic::AtomicBool, on: bool) {
    flag.store(on, std::sync::atomic::Ordering::SeqCst);
}

async fn schedule_on(t: &T, id: &str) -> Value {
    call(t, Method::GET, &format!("/v1/schedules/{id}"), None)
        .await
        .json()
}

#[tokio::test]
async fn a_lost_activation_leaves_the_host_staged_and_never_both_enabled_and_the_next_sync_finishes()
 {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, _chat, ids) = local_with_work(&host, &host.admin_key).await;
    let body = json!({"bots": [bot], "schedules": true});

    // The response to the activation is lost: the host holds the schedule DISABLED (staged),
    // the local copy is already frozen. At no point are both enabled.
    lose(&LOSE_ACTIVATE, true);
    let first = owner_post(&local, "/v1/hosts/h1/copy-setup", body.clone()).await;
    lose(&LOSE_ACTIVATE, false);
    assert_eq!(report_line(&first, "schedules")["status"], "failed");
    let on_host = schedule_on(&host.t, &ids[0]).await;
    assert_eq!(on_host["enabled"], false, "staged on the host: {on_host}");
    let here = schedule_on(&local, &ids[0]).await;
    assert_eq!(here["enabled"], false);
    assert_eq!(here["moved_to"], "Oracle");

    // The next sync reconciles whatever was interrupted: the host activates it.
    let second = owner_post(&local, "/v1/hosts/h1/copy-setup", body).await;
    assert_eq!(
        report_line(&second, "schedules")["failed"],
        0,
        "{:?}",
        second.json()
    );
    assert_eq!(schedule_on(&host.t, &ids[0]).await["enabled"], true);
    assert_eq!(schedule_on(&local, &ids[0]).await["enabled"], false);
    // The owner's paused one stays paused on the host.
    assert_eq!(schedule_on(&host.t, &ids[1]).await["enabled"], false);
}

#[tokio::test]
async fn a_bot_that_only_shares_a_name_is_refused_and_schedules_only_follow_the_mapping() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, _chat, ids) = local_with_work(&host, &host.admin_key).await;
    // The host has its OWN bot with the same name (it may hold other permissions).
    let r = call(
        &host.t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": super::PROFILE })),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let host_bot = r.json()["id"].as_str().unwrap().to_owned();

    let body = json!({"bots": [bot], "schedules": true, "chats": true});
    let refused = owner_post(&local, "/v1/hosts/h1/copy-setup", body.clone()).await;
    let line = report_line(&refused, "bot");
    assert_eq!(line["status"], "failed");
    assert_eq!(line["code"], "name_taken");
    let none = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    assert_eq!(
        none.as_array().unwrap().len(),
        0,
        "nothing goes to the same-named bot"
    );
    assert_eq!(schedule_on(&local, &ids[0]).await["enabled"], true);
    // A schedule naming an unmapped source bot is refused by the host itself.
    let r = peer_call(
        &host.t,
        Some((&host.admin_key, crate::PeerRole::Desktop)),
        Method::POST,
        "/v1/admin/import/schedules",
        Some(
            json!({"schedules": [{"id": uuid::Uuid::now_v7(), "source_bot_id": bot, "name": "x",
            "spec": {"every": {"seconds": 60}}, "payload": {"kind": "agent_prompt", "text": "t"},
            "enabled": true, "created_at_ms": 1}]}),
        ),
    )
    .await;
    assert_eq!(r.json()["items"][0]["code"], "bot_not_moved");

    // Only an explicit adoption binds to it, and then schedules follow that mapping.
    let mut adopt = body;
    adopt["adopt"] = json!([bot]);
    let ok = owner_post(&local, "/v1/hosts/h1/copy-setup", adopt).await;
    assert_eq!(report_line(&ok, "bot")["status"], "done", "{:?}", ok.json());
    let on_host = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    assert!(
        on_host
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["bot_id"] == json!(host_bot))
    );
    assert_eq!(on_host.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_import_cannot_extend_a_chat_it_did_not_create_nor_link_runs_across_bots() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, _chat, _ids) = local_with_work(&host, &host.admin_key).await;
    // A host chat of the host's own bot A.
    let r = call(
        &host.t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": super::PROFILE.replace("Tester", "HostOwn") })),
    )
    .await;
    let a_bot = r.json()["id"].as_str().unwrap().to_owned();
    let r = call(
        &host.t,
        Method::POST,
        "/v1/chats",
        Some(json!({ "bot_id": a_bot })),
    )
    .await;
    let a_chat = r.json()["id"].as_str().unwrap().to_owned();
    // Map the local bot (a moved bot B) through the real flow.
    let moved = owner_post(&local, "/v1/hosts/h1/copy-setup", json!({"bots": [bot]})).await;
    assert_eq!(report_line(&moved, "bot")["status"], "done");
    let admin = Some((host.admin_key.as_str(), crate::PeerRole::Desktop));
    let before = host_messages(&host, &a_chat).len();
    let page = json!({
        "chats": [{"id": a_chat, "source_bot_id": bot, "title": "takeover", "created_at_ms": 1}],
        "runs": [{"id": uuid::Uuid::now_v7(), "source_bot_id": bot, "chat_id": a_chat, "kind": "turn",
                  "state": "succeeded", "created_at_ms": 1, "updated_at_ms": 2}],
        "messages": [{"id": uuid::Uuid::now_v7(), "chat_id": a_chat, "role": "assistant",
                      "content": "forged history", "created_at_ms": 5}]
    });
    let r = peer_call(
        &host.t,
        admin,
        Method::POST,
        "/v1/admin/import/chats",
        Some(page),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["chats"], 0);
    assert_eq!(r.json()["runs"], 0);
    assert_eq!(r.json()["messages"], 0);
    assert_eq!(r.json()["rejected"], 3, "{:?}", r.json());
    assert_eq!(
        host_messages(&host, &a_chat).len(),
        before,
        "host chat A is untouched"
    );
}

#[tokio::test]
async fn imported_rows_are_never_owner_input_and_a_foreign_authors_reply_keeps_its_name() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, chat, _ids) = local_with_work(&host, &host.admin_key).await;
    let chat_id = protocol::ChatId(chat.parse().unwrap());
    let user = local.state.user_id();
    // A reply by a bot that is NOT moved (an @mention partner), with its own run.
    let atlas = protocol::BotId::new();
    local
        .state
        .storage()
        .insert_bot(&storage::Bot {
            id: atlas,
            user_id: user,
            name: "Atlas".into(),
            config: json!({}),
            created_at_ms: 1,
        })
        .unwrap();
    let run = protocol::RunId::new();
    local
        .state
        .storage()
        .insert_run(&storage::Run {
            id: run,
            user_id: user,
            bot_id: atlas,
            chat_id: Some(chat_id),
            kind: "turn".into(),
            state: "succeeded".into(),
            error: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            parent_run_id: None,
            spec: None,
            computer_target: None,
            computer_node: None,
            computer_node_name: None,
            paused_at_ms: None,
            pause_reason: None,
        })
        .unwrap();
    let reply = uuid::Uuid::now_v7();
    local
        .state
        .storage()
        .insert_message(&storage::Message {
            id: reply,
            user_id: user,
            chat_id,
            run_id: Some(run),
            role: "assistant".into(),
            content: "visit https://evil.example/x".into(),
            created_at_ms: 1_700_000_009_000,
            exposure_untrusted: false,
            exposure_private: false,
            exposure_sources: vec![],
        })
        .unwrap();
    add_message(
        &local,
        &chat,
        "user",
        "open https://example.com/typed",
        1_700_000_010_000,
    );

    let r = owner_post(
        &local,
        "/v1/hosts/h1/copy-setup",
        json!({"bots": [bot], "chats": true}),
    )
    .await;
    assert_eq!(report_line(&r, "chats")["status"], "done", "{:?}", r.json());
    let storage = host.t.state.storage();
    let rows = host_messages(&host, &chat);
    assert_eq!(rows.len(), 4);
    for m in &rows {
        let source = storage.message_source(m.id).unwrap();
        assert_ne!(source, "owner", "imported rows are never owner input");
        assert!(source.starts_with("imported"), "{source}");
    }
    // No imported row can satisfy the typed-URL waiver (it reads exactly this).
    assert_eq!(storage.owner_message_texts(chat_id).unwrap().len(), 0);
    // The partner's reply lost its run (its bot did not move) but kept its name, as foreign data.
    let imported = rows.iter().find(|m| m.id == reply).unwrap();
    assert_eq!(imported.run_id, None);
    assert_eq!(
        storage.message_source(reply).unwrap(),
        "imported_foreign:Atlas"
    );
}

#[tokio::test]
async fn a_moved_schedule_is_frozen_cannot_be_enabled_and_only_an_owner_reclaim_brings_it_back() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, _chat, ids) = local_with_work(&host, &host.admin_key).await;
    let moved = owner_post(
        &local,
        "/v1/hosts/h1/copy-setup",
        json!({"bots": [bot], "schedules": true}),
    )
    .await;
    assert_eq!(
        report_line(&moved, "schedules")["count"],
        2,
        "{:?}",
        moved.json()
    );
    let path = format!("/v1/schedules/{}", ids[0]);

    // An ordinary update cannot enable it (nor change it), however it is sent.
    let put = json!({"bot_id": bot, "name": "nightly", "spec": {"every": {"seconds": 3600}},
        "payload": {"kind": "agent_prompt", "text": "do the thing"}, "enabled": true});
    let r = call(&local, Method::PUT, &path, Some(put)).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.json());
    assert_eq!(r.json()["error"]["code"], "schedule_moved");
    // The database itself refuses to enable a frozen row, whatever code path asks.
    let store = local.state.schedule_store().unwrap();
    assert!(
        store
            .set_enabled(protocol::TaskId(ids[0].parse().unwrap()), true, None)
            .is_err()
    );
    let run_now = call(&local, Method::POST, &format!("{path}/run-now"), None).await;
    assert_eq!(run_now.status, StatusCode::CONFLICT);
    assert_eq!(schedule_on(&local, &ids[0]).await["enabled"], false);

    // Reclaim needs the owner key; the general token is refused.
    let (k, v) = auth();
    let bearer = [(k, v.as_str()), ("x-test-token-only", "1")];
    let denied = send(
        &local.app,
        Method::POST,
        &format!("{path}/reclaim"),
        Some(json!({})),
        &bearer,
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    // The server cannot be asked: the owner must confirm, and is told its copy may be active.
    lose(&LOSE_RELEASE, true);
    let r = owner_post(&local, &format!("{path}/reclaim"), json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["error"]["code"], "host_unreachable");
    assert_eq!(
        schedule_on(&local, &ids[0]).await["enabled"],
        false,
        "still frozen"
    );
    let r = owner_post(
        &local,
        &format!("{path}/reclaim"),
        json!({"confirm_host_unreachable": true}),
    )
    .await;
    lose(&LOSE_RELEASE, false);
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["host_copy_may_be_active"], true);
    assert_eq!(r.json()["enabled"], true);
    let back = schedule_on(&local, &ids[0]).await;
    assert_eq!(back["enabled"], true);
    assert!(back.get("moved_to").is_none());

    // The normal path switches the server's copy off first, then restores the local one.
    let path1 = format!("/v1/schedules/{}", ids[1]);
    let r = owner_post(&local, &format!("{path1}/reclaim"), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["host_released"], true);
    assert_eq!(r.json()["enabled"], false, "it was paused before the move");
    assert_eq!(schedule_on(&host.t, &ids[1]).await["enabled"], false);
    // A reclaimed schedule is never synced to the server again.
    let again = owner_post(
        &local,
        "/v1/hosts/h1/copy-setup",
        json!({"bots": [bot], "schedules": true}),
    )
    .await;
    assert_eq!(
        report_line(&again, "schedules")["skipped"],
        2,
        "{:?}",
        again.json()
    );
}

#[tokio::test]
async fn engine_install_on_the_host_is_admin_only_and_allow_listed() {
    let r = rig().await;
    let ok_peer = Some((r.admin_key.as_str(), crate::PeerRole::Desktop));
    // Every other caller is refused before anything is looked at or started.
    for (peer, code) in [
        (
            Some((r.legacy_key.as_str(), crate::PeerRole::Desktop)),
            "host_admin_required",
        ),
        (Some((r.admin_key.as_str(), crate::PeerRole::Phone)), "-"),
        (None, "-"),
    ] {
        for path in [
            "/v1/admin/cli-engines/claude/install",
            "/v1/admin/cli-engines/agy/install",
        ] {
            let denied = peer_call(&r.t, peer, Method::POST, path, None).await;
            assert_eq!(denied.status, StatusCode::FORBIDDEN, "{path}");
            if code != "-" {
                assert_eq!(denied.json()["error"]["code"], code);
            }
        }
    }
    // The admin can only name an engine id: unknown ids and engines without a
    // verified installer (Gemini CLI is retired) are refused, never run.
    let unknown = peer_call(
        &r.t,
        ok_peer,
        Method::POST,
        "/v1/admin/cli-engines/claude;whoami/install",
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let retired = peer_call(
        &r.t,
        ok_peer,
        Method::POST,
        "/v1/admin/cli-engines/gemini/install",
        None,
    )
    .await;
    assert_eq!(retired.status, StatusCode::CONFLICT);
    assert_eq!(retired.json()["error"]["code"], "install_copy_only");
    r.net.shutdown().await;
}

#[tokio::test]
async fn the_plan_asks_about_same_named_bots_and_the_answer_is_remembered() {
    let _turn = MOVE_TURN.lock().await;
    let host = rig().await;
    let (local, bot, _chat, _ids) = local_with_work(&host, &host.admin_key).await;
    // A bot of the same name that this flow did not create (made by hand or by the old flow).
    let r = call(
        &host.t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": super::PROFILE })),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let state_of = |plan: &Value| {
        plan["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == json!(bot))
            .unwrap()["host_state"]
            .clone()
    };
    let plan = call(&local, Method::GET, "/v1/hosts/h1/copy-setup", None).await;
    assert_eq!(state_of(&plan.json()), "name_taken", "{:?}", plan.json());

    // "Keep them separate": nothing of the bot moves and the question is not asked again.
    let body = json!({"bots": [bot], "schedules": true, "chats": true, "separate": [bot]});
    let kept = owner_post(&local, "/v1/hosts/h1/copy-setup", body).await;
    let line = report_line(&kept, "bot");
    assert_eq!(line["status"], "skipped");
    assert_eq!(line["code"], "kept_separate");
    let none = call(&host.t, Method::GET, "/v1/schedules", None)
        .await
        .json();
    assert_eq!(none.as_array().unwrap().len(), 0);
    let plan = call(&local, Method::GET, "/v1/hosts/h1/copy-setup", None).await;
    assert_eq!(state_of(&plan.json()), "separate");

    // "It is the same bot - link them" is the only way to bind; afterwards it is `mapped`.
    let body = json!({"bots": [bot], "schedules": true, "adopt": [bot]});
    let linked = owner_post(&local, "/v1/hosts/h1/copy-setup", body).await;
    assert_eq!(
        report_line(&linked, "bot")["status"],
        "done",
        "{:?}",
        linked.json()
    );
    assert_eq!(report_line(&linked, "schedules")["count"], 2);
    let plan = call(&local, Method::GET, "/v1/hosts/h1/copy-setup", None).await;
    assert_eq!(state_of(&plan.json()), "mapped");
}
