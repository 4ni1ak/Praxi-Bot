//! Host administration: who may send keys to a host, and where the key lands.
use super::*;
use crate::host::{HostConfig, HostMode, HostState};
use peer_network::{PairTicket, PeerConfig, PeerNetwork, RelayConfig, Role};

const KEY: &str = "sk-SECRET-VALUE-0123456789";
const OWNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
    let r = call(&t, Method::POST, url, Some(json!({"providers":["alpha"]}))).await;
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
    let bearer = [(k, v.as_str())];
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
    // An ordinary invite is still fine with the token alone.
    let plain = send(
        &r.t.app,
        Method::POST,
        "/v1/host/invite",
        Some(json!({"role": "desktop"})),
        &bearer,
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
