//! OAuth sign-in against a fake authorization server on loopback: discovery,
//! PKCE code exchange, `resource`, storage and redaction. No network.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use base64::Engine as _;
use mcp_bridge::oauth::{SecretRegistrar, SignIn, SignInOptions, credential_name};
use mcp_bridge::{Backoff, McpError, McpManager, McpPolicy, McpServerConfig, ServerStatus};
use parking_lot::Mutex;
use secret_store::{SecretError, SecretStore, SecretValue};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ACCESS: &str = "access-sentinel-e2e-7f3a";
const REFRESH: &str = "refresh-sentinel-e2e-91bc";
const REFRESHED: &str = "refreshed-access-sentinel-e2e-637c";
const ROTATED: &str = "rotated-refresh-sentinel-e2e-0d4e";

#[derive(Default)]
struct MemoryStore(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<SecretValue>, SecretError> {
        Ok(self.0.lock().get(name).map(SecretValue::new))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        self.0.lock().insert(name.to_owned(), value.to_owned());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<bool, SecretError> {
        Ok(self.0.lock().remove(name).is_some())
    }
    fn list_names(&self) -> Result<Vec<String>, SecretError> {
        Ok(self.0.lock().keys().cloned().collect())
    }
}

/// What the fake authorization server saw and what it expects.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // independent fixture switches
struct Seen {
    /// When set, a refresh answer waits for one permit (the test decides
    /// exactly when the refreshed tokens arrive).
    refresh_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// Refresh requests received (counted before waiting at the gate).
    refreshes_received: usize,
    /// Answer refreshes with a new refresh token ([`ROTATED`]).
    rotate_refresh: bool,
    expires_in: Option<u64>,
    refresh_allowed: bool,
    /// When set, speak MCP for this bearer instead of always returning 401.
    mcp_bearer: Option<String>,
    tool_calls: Vec<Option<String>>,
    reject_tool_calls: bool,
    client_secret: Option<String>,
    cimd_supported: bool,
    /// `code_challenge` of the authorization request (set by the test, which
    /// plays the browser).
    challenge: Option<String>,
    /// Decoded form of every token request.
    token_requests: Vec<BTreeMap<String, String>>,
    /// Request paths, in order.
    paths: Vec<String>,
    /// `Authorization` header of every request to the MCP endpoint itself.
    mcp_authorization: Vec<Option<String>>,
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(&raw[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pairs(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

/// Authorization server and protected-resource metadata on one loopback port.
#[allow(clippy::too_many_lines)] // One linear fixture: read a request, answer by path.
async fn fake_authorization_server(
    advertise_pkce: bool,
) -> (String, Arc<Mutex<Seen>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Seen::default()));
    let task = tokio::spawn({
        let base = base.clone();
        let seen = Arc::clone(&seen);
        async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let base = base.clone();
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let Ok(n) = socket.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        let Some((head, body)) = text.split_once("\r\n\r\n") else {
                            continue;
                        };
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break (head.to_owned(), body.to_owned());
                        }
                    };
                    let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
                    seen.lock().paths.push(path.clone());
                    if path == "/mcp" {
                        // By default this endpoint challenges every request;
                        // session tests enable MCP for one expected bearer.
                        let authorization = head.lines().find_map(|l| {
                            let (name, value) = l.split_once(':')?;
                            name.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim().to_owned())
                        });
                        seen.lock().mcp_authorization.push(authorization.clone());
                        let request: serde_json::Value =
                            serde_json::from_str(&body).unwrap_or_default();
                        let method = request["method"].as_str().unwrap_or_default();
                        if method == "tools/call" {
                            seen.lock().tool_calls.push(authorization.clone());
                        }
                        let allowed = seen.lock().mcp_bearer.as_ref().is_some_and(|token| {
                            authorization.as_deref() == Some(format!("Bearer {token}").as_str())
                        });
                        if allowed && !(method == "tools/call" && seen.lock().reject_tool_calls) {
                            if request.get("id").is_none() {
                                let response: &[u8] = if method.starts_with("notifications/") {
                                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                } else {
                                    b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                };
                                let _ = socket.write_all(response).await;
                                let _ = socket.shutdown().await;
                                return;
                            }
                            let result = match method {
                                "initialize" => serde_json::json!({
                                    "protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                                    "serverInfo": {"name": "oauth-fixture", "version": "1"},
                                }),
                                "server/discover" => serde_json::json!({
                                    "resultType": "complete", "supportedVersions": ["2026-07-28"],
                                    "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "private",
                                    "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "oauth-fixture", "version": "1"}},
                                }),
                                "tools/list" => serde_json::json!({
                                    "resultType": "complete", "ttlMs": 0, "cacheScope": "private",
                                    "tools": [{"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}}],
                                }),
                                "tools/call" => serde_json::json!({
                                    "resultType": "complete", "content": [{"type": "text", "text": request["params"]["arguments"]["text"].as_str().unwrap_or("echo")}],
                                }),
                                _ => serde_json::json!({"resultType": "complete"}),
                            };
                            let response = serde_json::json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string();
                            let _ = socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await;
                            let _ = socket.shutdown().await;
                            return;
                        }
                        let _ = socket
                            .write_all(
                                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\n\
                                  Content-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                        let _ = socket.shutdown().await;
                        return;
                    }
                    let (status, json) = if path.contains("oauth-protected-resource") {
                        (
                            "200 OK",
                            serde_json::json!({
                                "resource": format!("{base}/mcp"),
                                "authorization_servers": [base],
                            }),
                        )
                    } else if path.contains("oauth-authorization-server")
                        || path.contains("openid-configuration")
                    {
                        let mut metadata = serde_json::json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                            "response_types_supported": ["code"],
                            "grant_types_supported": ["authorization_code", "refresh_token"],
                            "authorization_response_iss_parameter_supported": true,
                        });
                        if advertise_pkce {
                            metadata["code_challenge_methods_supported"] =
                                serde_json::json!(["S256"]);
                        }
                        metadata["client_id_metadata_document_supported"] =
                            serde_json::json!(seen.lock().cimd_supported);
                        metadata["token_endpoint_auth_methods_supported"] =
                            serde_json::json!(["client_secret_post"]);
                        ("200 OK", metadata)
                    } else if path == "/token" {
                        let form = pairs(&body);
                        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(Sha256::digest(verifier.as_bytes()));
                        let expected = seen.lock().challenge.clone();
                        let secret_valid = seen
                            .lock()
                            .client_secret
                            .as_ref()
                            .is_none_or(|secret| form.get("client_secret") == Some(secret));
                        let valid = secret_valid
                            && form.get("grant_type").map(String::as_str)
                                == Some("authorization_code")
                            && form.get("code").map(String::as_str) == Some("code-1")
                            && Some(challenge) == expected;
                        let refreshing = secret_valid
                            && form.get("grant_type").map(String::as_str) == Some("refresh_token")
                            && form.get("refresh_token").map(String::as_str) == Some(REFRESH)
                            && seen.lock().refresh_allowed;
                        let expires_in = seen.lock().expires_in.unwrap_or(3600);
                        seen.lock().token_requests.push(form);
                        if refreshing {
                            let gate = {
                                let mut seen = seen.lock();
                                seen.refreshes_received += 1;
                                seen.refresh_gate.clone()
                            };
                            if let Some(gate) = gate {
                                gate.acquire().await.unwrap().forget();
                            }
                        }
                        let rotated = seen.lock().rotate_refresh;
                        if valid || refreshing {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "access_token": if refreshing { REFRESHED } else { ACCESS },
                                    "token_type": "Bearer",
                                    "expires_in": if refreshing { 3600 } else { expires_in },
                                    "refresh_token": if refreshing && rotated { ROTATED } else { REFRESH },
                                }),
                            )
                        } else {
                            (
                                "400 Bad Request",
                                serde_json::json!({"error": "invalid_grant"}),
                            )
                        }
                    } else {
                        ("404 Not Found", serde_json::json!({"error": "not_found"}))
                    };
                    let body = json.to_string();
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = socket.shutdown().await;
                });
            }
        }
    });
    (base, seen, task)
}

fn options() -> SignInOptions {
    SignInOptions {
        client_id: Some("praxi-test-client".into()),
        allow_loopback_http: true,
        ..SignInOptions::default()
    }
}

/// Play the browser: follow the authorization URL's `redirect_uri` with `query`.
async fn redirect(authorization_url: &str, query: &str) -> std::io::Result<String> {
    let params = pairs(authorization_url.split_once('?').unwrap().1);
    let target = params["redirect_uri"]
        .strip_prefix("http://")
        .unwrap()
        .to_owned();
    let (address, path) = target.split_once('/').unwrap();
    let mut socket = tokio::net::TcpStream::connect(address).await?;
    socket
        .write_all(format!("GET /{path}?{query} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await?;
    let mut out = String::new();
    socket.read_to_string(&mut out).await?;
    Ok(out)
}

async fn finish_sign_in(sign_in: SignIn, base: &str, seen: &Mutex<Seen>) {
    let url = sign_in.authorization_url().to_owned();
    let params = pairs(url.split_once('?').unwrap().1);
    seen.lock().challenge = Some(params["code_challenge"].clone());
    let finishing = tokio::spawn(sign_in.finish(Duration::from_secs(20)));
    redirect(
        &url,
        &format!("code=code-1&state={}&iss={base}", params["state"]),
    )
    .await
    .unwrap();
    finishing.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn expiry_and_the_sixty_second_window_refresh_before_sending_a_bearer() {
    for (expiry, accepted) in [(1, true), (45, true), (1, false)] {
        let (base, seen, server) = fake_authorization_server(true).await;
        seen.lock().expires_in = Some(expiry);
        seen.lock().refresh_allowed = accepted;
        let store = Arc::new(MemoryStore::default());
        let manager = McpManager::with_policy(
            tool_registry::ToolRegistry::new(),
            McpPolicy {
                quarantine_after: 1,
                ..McpPolicy::default()
            },
        )
        .with_secret_store(store);
        assert!(matches!(
            manager
                .add_server(http_config("expiry", format!("{base}/mcp")))
                .await,
            Err(McpError::AuthRequired(_))
        ));
        finish_sign_in(
            manager.start_sign_in("expiry", options()).await.unwrap(),
            &base,
            &seen,
        )
        .await;
        if expiry == 1 {
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
        // This fixture deliberately still rejects MCP; the refresh must happen
        // first, and authentication failures must never quarantine it.
        assert!(matches!(
            manager.reenable("expiry").await,
            Err(McpError::AuthRequired(_))
        ));
        let requests = seen.lock().token_requests.clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["grant_type"], "refresh_token");
        assert_eq!(requests[1]["resource"], format!("{base}/mcp"));
        let bearers = seen.lock().mcp_authorization.clone();
        if accepted {
            assert_eq!(
                bearers.last().cloned().flatten(),
                Some(format!("Bearer {REFRESHED}"))
            );
        } else {
            assert!(bearers.iter().all(Option::is_none));
        }
        let snapshot = manager.snapshot("expiry").unwrap();
        assert!(snapshot.auth_required);
        assert_eq!(snapshot.consecutive_failures, 0);
        assert_eq!(snapshot.status, ServerStatus::Disconnected);
        manager.shutdown().await;
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_unauthorized_request_reconnects_and_retries_only_once_without_failures() {
    for accepted in [true, false] {
        let (base, seen, server) = fake_authorization_server(true).await;
        seen.lock().mcp_bearer = Some(ACCESS.into());
        seen.lock().refresh_allowed = true;
        let registry = tool_registry::ToolRegistry::new();
        let manager = McpManager::with_policy(
            registry.clone(),
            McpPolicy {
                quarantine_after: 1,
                ..McpPolicy::default()
            },
        )
        .with_secret_store(Arc::new(MemoryStore::default()));
        assert!(matches!(
            manager
                .add_server(http_config("live", format!("{base}/mcp")))
                .await,
            Err(McpError::AuthRequired(_))
        ));
        finish_sign_in(
            manager.start_sign_in("live", options()).await.unwrap(),
            &base,
            &seen,
        )
        .await;
        assert_eq!(manager.reenable("live").await.unwrap(), 1);
        let echo = registry.get("mcp:live/echo").unwrap();
        let context = tool_registry::ToolContext::default();
        echo.call(&context, serde_json::json!({"text": "before"}))
            .await
            .unwrap();
        seen.lock().tool_calls.clear();
        seen.lock().mcp_bearer = Some(REFRESHED.into());
        if !accepted {
            // Reconnect succeeds, but the retried tool request is still denied.
            seen.lock().reject_tool_calls = true;
        }
        let call = echo
            .call(&context, serde_json::json!({"text": "after"}))
            .await;
        let snapshot = manager.snapshot("live").unwrap();
        assert_eq!(snapshot.consecutive_failures, 0, "{snapshot:?}");
        assert_ne!(snapshot.status, ServerStatus::Quarantined);
        let requests = seen.lock().token_requests.clone();
        assert_eq!(requests.len(), 2, "one refresh per failed request");
        assert_eq!(
            seen.lock().tool_calls,
            vec![
                Some(format!("Bearer {ACCESS}")),
                Some(format!("Bearer {REFRESHED}"))
            ]
        );
        if accepted {
            assert!(call.is_ok(), "{call:?}");
            assert_eq!(snapshot.status, ServerStatus::Connected);
        } else {
            assert!(call.unwrap_err().to_string().contains("authorization"));
            assert!(snapshot.auth_required);
        }
        manager.shutdown().await;
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_discovers_exchanges_with_pkce_and_stores_only_in_the_secret_store() {
    let (base, seen, server) = fake_authorization_server(true).await;
    seen.lock().mcp_bearer = Some(ACCESS.into());
    let store = Arc::new(MemoryStore::default());
    let registered = Arc::new(Mutex::new(Vec::<String>::new()));
    let registrar: SecretRegistrar = {
        let registered = Arc::clone(&registered);
        Arc::new(move |value: &str| registered.lock().push(value.to_owned()))
    };
    let registry = tool_registry::ToolRegistry::new();
    let manager = McpManager::new(registry.clone())
        .with_secret_store(store.clone())
        .with_secret_registrar(registrar);
    assert!(matches!(
        manager
            .add_server(http_config("e2e", format!("{base}/mcp")))
            .await,
        Err(McpError::AuthRequired(_))
    ));
    assert!(registry.get("mcp:e2e/echo").is_none());
    let sign_in = manager.start_sign_in("e2e", options()).await.unwrap();
    let url = sign_in.authorization_url().to_owned();
    let params = pairs(url.split_once('?').unwrap().1);
    assert!(url.starts_with(&format!("{base}/authorize?")), "{url}");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["client_id"], "praxi-test-client");
    assert_eq!(params["resource"], format!("{base}/mcp"));
    assert!(params["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert_ne!(params["state"], "");
    seen.lock().challenge = Some(params["code_challenge"].clone());
    assert!(!format!("{sign_in:?}").contains(&params["state"]));

    let finishing = tokio::spawn(sign_in.finish(Duration::from_secs(20)));
    let page = redirect(
        &url,
        &format!("code=code-1&state={}&iss={base}", params["state"]),
    )
    .await
    .unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    finishing.await.unwrap().unwrap();

    // The verifier matched the challenge (the fake answers 400 otherwise) and
    // the token request named the resource.
    let token_requests = seen.lock().token_requests.clone();
    assert_eq!(token_requests.len(), 1);
    assert_eq!(token_requests[0]["resource"], format!("{base}/mcp"));
    assert!(token_requests[0]["code_verifier"].len() >= 43);

    // Tokens: in the secret store under the server's name, registered for
    // redaction, bound to the server URL.
    assert_eq!(store.list_names().unwrap(), vec![credential_name("e2e")]);
    let stored = store.get("mcp/e2e/oauth").unwrap().unwrap();
    assert!(stored.expose().contains(ACCESS));
    assert!(stored.expose().contains(REFRESH));
    assert!(stored.expose().contains(&format!("{base}/mcp")));
    let registered = registered.lock().clone();
    assert!(registered.contains(&ACCESS.to_owned()), "{registered:?}");
    assert!(registered.contains(&REFRESH.to_owned()), "{registered:?}");
    assert_eq!(manager.reenable("e2e").await.unwrap(), 1);
    let echo = registry.get("mcp:e2e/echo").unwrap();
    // Signing in grants no permission to the server's tools.
    assert_eq!(echo.spec().effect, tool_registry::Effect::Destructive);
    assert_eq!(
        echo.spec().category,
        tool_registry::PermissionCategory::Accounts
    );
    let result = echo
        .call(
            &tool_registry::ToolContext::default(),
            serde_json::json!({"text": "signed in"}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["content"][0]["text"],
        "signed in"
    );
    assert_eq!(
        manager.snapshot("e2e").unwrap().status,
        ServerStatus::Connected
    );
    assert!(
        seen.lock()
            .mcp_authorization
            .iter()
            .any(|v| v.as_deref() == Some(&format!("Bearer {ACCESS}")))
    );
    // A replayed redirect has nowhere to go.
    assert!(
        redirect(&url, &format!("code=code-1&state={}", params["state"]))
            .await
            .is_err()
    );
    manager.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_state_or_a_refused_code_stores_nothing_and_leaks_nothing() {
    let (base, seen, server) = fake_authorization_server(true).await;
    for query in ["code=code-1&state=forged", "code=stolen-code&state={state}"] {
        let store = Arc::new(MemoryStore::default());
        let sign_in = SignIn::start(
            "bad",
            &format!("{base}/mcp"),
            store.clone(),
            None,
            options(),
        )
        .await
        .unwrap();
        let url = sign_in.authorization_url().to_owned();
        let params = pairs(url.split_once('?').unwrap().1);
        seen.lock().challenge = Some(params["code_challenge"].clone());
        let finishing = tokio::spawn(sign_in.finish(Duration::from_secs(20)));
        let query = query.replace("{state}", &params["state"]);
        let _ = redirect(&url, &format!("{query}&iss={base}")).await;
        let error = finishing.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("OAuth sign-in failed"), "{error}");
        assert!(!error.contains("stolen-code"), "{error}");
        assert!(!error.contains(&params["state"]), "{error}");
        assert_eq!(store.list_names().unwrap(), Vec::<String>::new(), "{query}");
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_authorization_server_without_pkce_is_refused_before_any_browser_step() {
    let (base, seen, server) = fake_authorization_server(false).await;
    let store = Arc::new(MemoryStore::default());
    let error = SignIn::start(
        "nopkce",
        &format!("{base}/mcp"),
        store.clone(),
        None,
        options(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("S256"), "{error}");
    assert_eq!(seen.lock().token_requests.len(), 0);
    assert_eq!(store.list_names().unwrap(), Vec::<String>::new());

    // Loopback http endpoints need the explicit development opt-in.
    let (base, _, strict) = fake_authorization_server(true).await;
    let mut opts = options();
    opts.allow_loopback_http = false;
    let error = SignIn::start("strict", &format!("{base}/mcp"), store, None, opts)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("https"), "{error}");
    server.abort();
    strict.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_is_sent_only_to_its_own_server_and_waiting_for_one_is_not_a_failure() {
    let (a, seen_a, server_a) = fake_authorization_server(true).await;
    let (b, seen_b, server_b) = fake_authorization_server(true).await;
    let store = Arc::new(MemoryStore::default());
    let registered = Arc::new(Mutex::new(Vec::<String>::new()));
    let registrar: SecretRegistrar = {
        let registered = Arc::clone(&registered);
        Arc::new(move |value: &str| registered.lock().push(value.to_owned()))
    };
    let manager = McpManager::with_policy(
        tool_registry::ToolRegistry::new(),
        McpPolicy {
            // One counted failure would quarantine a server.
            quarantine_after: 1,
            backoff: Backoff {
                initial: Duration::from_secs(30),
                max: Duration::from_secs(30),
                max_attempts: 3,
            },
            connect_timeout: Duration::from_secs(20),
            call_timeout: Duration::from_secs(20),
        },
    )
    .with_secret_store(store.clone())
    .with_secret_registrar(registrar);

    // Both servers answer 401: they wait for the user, unquarantined.
    for (id, base) in [("a", &a), ("b", &b)] {
        let error = manager
            .add_server(http_config(id, format!("{base}/mcp")))
            .await
            .unwrap_err();
        assert!(matches!(error, McpError::AuthRequired(_)), "{error}");
        let snapshot = manager.snapshot(id).unwrap();
        assert!(snapshot.auth_required, "{snapshot:?}");
        assert_eq!(snapshot.consecutive_failures, 0, "{snapshot:?}");
        assert_eq!(snapshot.status, ServerStatus::Disconnected, "{snapshot:?}");
    }
    assert_eq!(seen_a.lock().mcp_authorization, vec![None]);

    // The user signs in to A (and only to A).
    let sign_in = manager.start_sign_in("a", options()).await.unwrap();
    let url = sign_in.authorization_url().to_owned();
    let params = pairs(url.split_once('?').unwrap().1);
    seen_a.lock().challenge = Some(params["code_challenge"].clone());
    let finishing = tokio::spawn(sign_in.finish(Duration::from_secs(20)));
    redirect(
        &url,
        &format!("code=code-1&state={}&iss={a}", params["state"]),
    )
    .await
    .unwrap();
    finishing.await.unwrap().unwrap();
    assert_eq!(store.list_names().unwrap(), vec![credential_name("a")]);

    // Reconnect both. On the wire: A gets its bearer, B gets nothing.
    let after_a = manager.reenable("a").await.unwrap_err().to_string();
    let after_b = manager.reenable("b").await.unwrap_err().to_string();
    assert_eq!(
        seen_a.lock().mcp_authorization.last().cloned().flatten(),
        Some(format!("Bearer {ACCESS}"))
    );
    let to_b = seen_b.lock().mcp_authorization.clone();
    assert!(to_b.len() >= 2, "{to_b:?}");
    assert!(to_b.iter().all(Option::is_none), "{to_b:?}");
    for path in &seen_b.lock().paths {
        assert!(!path.contains(ACCESS), "{path}");
    }
    // Nothing the manager reports carries a token.
    let reported = format!(
        "{after_a} {after_b} {:?} {:?}",
        manager.snapshots(),
        manager.stderr_tail("a")
    );
    assert!(!reported.contains(ACCESS), "{reported}");
    assert!(!reported.contains(REFRESH), "{reported}");
    assert!(registered.lock().contains(&ACCESS.to_owned()));
    assert_eq!(manager.snapshot("a").unwrap().consecutive_failures, 0);

    // The same server name pointed at B's URL must not carry A's token there.
    manager.remove_server("a").await.unwrap();
    let _ = manager
        .add_server(http_config("a", format!("{b}/mcp")))
        .await;
    let to_b = seen_b.lock().mcp_authorization.clone();
    assert!(to_b.iter().all(Option::is_none), "{to_b:?}");
    assert_eq!(store.list_names().unwrap(), Vec::<String>::new());

    // Signing out removes the secret.
    assert!(!manager.sign_out("a").await.unwrap());
    manager.shutdown().await;
    server_a.abort();
    server_b.abort();
}

fn http_config(id: &str, url: String) -> McpServerConfig {
    let mut config = McpServerConfig::http(id, url);
    config.oauth = Some(mcp_bridge::OAuthConfig {
        client_id: Some("praxi-test-client".into()),
        allow_loopback_http: true,
        ..mcp_bridge::OAuthConfig::default()
    });
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn confidential_refresh_reads_the_secret_reference_on_every_connection() {
    const CLIENT_SECRET: &str = "confidential-client-sentinel-98fd";
    const ROTATED_SECRET: &str = "rotated-client-sentinel-62be";
    let (base, seen, server) = fake_authorization_server(true).await;
    seen.lock().client_secret = Some(CLIENT_SECRET.into());
    seen.lock().expires_in = Some(45);
    seen.lock().refresh_allowed = true;
    seen.lock().mcp_bearer = Some(REFRESHED.into());
    let store = Arc::new(MemoryStore::default());
    store.set("mcp/client-secret", CLIENT_SECRET).unwrap();
    let registry = tool_registry::ToolRegistry::new();
    let registered = Arc::new(Mutex::new(Vec::new()));
    let registrar: SecretRegistrar = {
        let registered = registered.clone();
        Arc::new(move |value| registered.lock().push(value.to_owned()))
    };
    let manager = McpManager::new(registry.clone())
        .with_secret_store(store.clone())
        .with_secret_registrar(registrar);
    let mut config = http_config("confidential", format!("{base}/mcp"));
    config.oauth.as_mut().unwrap().client_secret_ref = Some("mcp/client-secret".into());
    assert!(!toml::to_string(&config).unwrap().contains(CLIENT_SECRET));
    assert!(!format!("{config:?}").contains(CLIENT_SECRET));
    assert!(matches!(
        manager.add_server(config).await,
        Err(McpError::AuthRequired(_))
    ));
    finish_sign_in(
        manager
            .start_sign_in("confidential", SignInOptions::default())
            .await
            .unwrap(),
        &base,
        &seen,
    )
    .await;
    store.set("mcp/client-secret", ROTATED_SECRET).unwrap();
    seen.lock().client_secret = Some(ROTATED_SECRET.into());
    assert_eq!(manager.reenable("confidential").await.unwrap(), 1);
    let requests = seen.lock().token_requests.clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].get("client_secret").map(String::as_str),
        Some(CLIENT_SECRET)
    );
    assert_eq!(
        requests[1].get("client_secret").map(String::as_str),
        Some(ROTATED_SECRET)
    );
    assert_eq!(requests[1]["grant_type"], "refresh_token");
    assert!(registered.lock().iter().any(|v| v == CLIENT_SECRET));
    assert!(registered.lock().iter().any(|v| v == ROTATED_SECRET));
    assert!(registry.get("mcp:confidential/echo").is_some());
    manager.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_document_is_preferred_to_a_preregistered_client_when_supported() {
    let (base, seen, server) = fake_authorization_server(true).await;
    seen.lock().cimd_supported = true;
    let mut opts = options();
    opts.client_metadata_url = Some("https://client.example/metadata.json".into());
    let sign_in = SignIn::start(
        "cimd",
        &format!("{base}/mcp"),
        Arc::new(MemoryStore::default()),
        None,
        opts,
    )
    .await
    .unwrap();
    let params = pairs(sign_in.authorization_url().split_once('?').unwrap().1);
    assert_eq!(params["client_id"], "https://client.example/metadata.json");
    finish_sign_in(sign_in, &base, &seen).await;
    server.abort();
}

/// Review finding 3: a refresh that is waiting for its answer when the user
/// signs out must not store the refreshed (rotated) credentials afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn sign_out_during_a_blocked_refresh_leaves_no_credentials() {
    let (base, seen, server) = fake_authorization_server(true).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    {
        let mut seen = seen.lock();
        seen.expires_in = Some(30); // inside the 60 s window: refresh on connect
        seen.refresh_allowed = true;
        seen.rotate_refresh = true;
        seen.mcp_bearer = Some(REFRESHED.into());
        seen.refresh_gate = Some(Arc::clone(&gate));
    }
    let store = Arc::new(MemoryStore::default());
    let manager = Arc::new(
        McpManager::with_policy(tool_registry::ToolRegistry::new(), McpPolicy::default())
            .with_secret_store(store.clone()),
    );
    assert!(matches!(
        manager
            .add_server(http_config("race", format!("{base}/mcp")))
            .await,
        Err(McpError::AuthRequired(_))
    ));
    finish_sign_in(
        manager.start_sign_in("race", options()).await.unwrap(),
        &base,
        &seen,
    )
    .await;
    assert!(store.0.lock().contains_key(&credential_name("race")));

    // Connect: the refresh request reaches the token endpoint and waits.
    let connecting = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.reenable("race").await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen.lock().refreshes_received == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Sign out while it waits: the record is gone right away.
    let signing_out = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.sign_out("race").await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while store.0.lock().contains_key(&credential_name("race")) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Now the rotated tokens arrive.
    gate.add_permits(1);
    assert!(signing_out.await.unwrap().unwrap());
    let connected = connecting.await.unwrap();
    assert!(
        matches!(connected, Err(McpError::AuthRequired(_))),
        "{connected:?}"
    );
    assert!(
        !store.0.lock().contains_key(&credential_name("race")),
        "a refresh re-stored credentials after sign-out"
    );
    let all = format!("{:?}", store.0.lock());
    assert!(!all.contains(ROTATED) && !all.contains(REFRESHED));
    // No session was opened with the refreshed bearer.
    assert!(
        seen.lock()
            .mcp_authorization
            .iter()
            .all(|a| a.as_deref() != Some(&format!("Bearer {REFRESHED}")))
    );
    manager.shutdown().await;
    server.abort();
}
