//! Host administration: who may send keys to a host, and where the key lands.
use super::*;
use crate::host::{HostConfig, HostMode, HostState};
use peer_network::{PairTicket, PeerConfig, PeerNetwork, RelayConfig, Role};

const KEY: &str = "sk-SECRET-VALUE-0123456789";

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
        Vec::new()
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
    let r = call(&t, Method::POST, url, Some(json!({"providers":["alpha"]}))).await;
    assert_eq!(r.status, StatusCode::OK);
    let all = format!("{:?}", rec.0.lock().clone());
    assert!(all.contains("key-of-alpha-xxxxxx") && !all.contains("key-of-beta"));
    assert_eq!(r.json()["items"][0]["status"], "done");
    assert!(!String::from_utf8_lossy(&r.body).contains("key-of-alpha"));
}
