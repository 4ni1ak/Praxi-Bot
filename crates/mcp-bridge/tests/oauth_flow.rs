//! OAuth sign-in against a fake authorization server on loopback: discovery,
//! PKCE code exchange, `resource`, storage and redaction. No network.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use base64::Engine as _;
use mcp_bridge::oauth::{SecretRegistrar, SignIn, SignInOptions, credential_name};
use parking_lot::Mutex;
use secret_store::{SecretError, SecretStore, SecretValue};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ACCESS: &str = "access-sentinel-e2e-7f3a";
const REFRESH: &str = "refresh-sentinel-e2e-91bc";

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
struct Seen {
    /// `code_challenge` of the authorization request (set by the test, which
    /// plays the browser).
    challenge: Option<String>,
    /// Decoded form of every token request.
    token_requests: Vec<BTreeMap<String, String>>,
    /// Request paths, in order.
    paths: Vec<String>,
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
                        ("200 OK", metadata)
                    } else if path == "/token" {
                        let form = pairs(&body);
                        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(Sha256::digest(verifier.as_bytes()));
                        let expected = seen.lock().challenge.clone();
                        let valid = form.get("grant_type").map(String::as_str)
                            == Some("authorization_code")
                            && form.get("code").map(String::as_str) == Some("code-1")
                            && Some(challenge) == expected;
                        seen.lock().token_requests.push(form);
                        if valid {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "access_token": ACCESS,
                                    "token_type": "Bearer",
                                    "expires_in": 3600,
                                    "refresh_token": REFRESH,
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

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_discovers_exchanges_with_pkce_and_stores_only_in_the_secret_store() {
    let (base, seen, server) = fake_authorization_server(true).await;
    let store = Arc::new(MemoryStore::default());
    let registered = Arc::new(Mutex::new(Vec::<String>::new()));
    let registrar: SecretRegistrar = {
        let registered = Arc::clone(&registered);
        Arc::new(move |value: &str| registered.lock().push(value.to_owned()))
    };
    let sign_in = SignIn::start(
        "e2e",
        &format!("{base}/mcp"),
        store.clone(),
        Some(registrar),
        options(),
    )
    .await
    .unwrap();
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
    // A replayed redirect has nowhere to go.
    assert!(
        redirect(&url, &format!("code=code-1&state={}", params["state"]))
            .await
            .is_err()
    );
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
