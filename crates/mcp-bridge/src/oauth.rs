//! Building blocks of OAuth for remote (streamable HTTP) MCP servers.
//!
//! The protocol work (protected-resource and authorization-server metadata
//! discovery, PKCE, issuer validation, refresh) is rmcp's `auth` module. This
//! module supplies the two parts that are ours:
//!
//! * [`SecretCredentialStore`]: rmcp's credential storage over the project's
//!   secret store. Tokens live under `mcp/<server>/oauth` and nowhere else;
//!   every token value is handed to the host redactor before it can be used.
//! * [`CallbackListener`]: the one-shot loopback redirect endpoint. It binds
//!   to `127.0.0.1` only, accepts exactly one request, requires the expected
//!   `state` and times out.
//!
//! Nothing here starts a flow by itself: a flow is started by the host on an
//! explicit user action.

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, AuthorizationRequest,
    AuthorizationSession, CredentialStore, StoredCredentials,
};
use secret_store::SecretStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{McpError, Result};

/// How long a user has to finish the sign-in in the browser.
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Largest redirect request that is read.
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// Told about every token value before it is used, so the host can mask it
/// in logs, events and tool output (the daemon's `SharedRedactor::register`).
pub type SecretRegistrar = Arc<dyn Fn(&str) + Send + Sync>;

/// Secret-store name of a server's OAuth credentials.
#[must_use]
pub fn credential_name(server: &str) -> String {
    format!("mcp/{server}/oauth")
}

/// OAuth credentials of one MCP server, kept in the secret store.
pub struct SecretCredentialStore {
    store: Arc<dyn SecretStore>,
    name: String,
    /// Canonical URL of the MCP server the credentials were issued for.
    resource: String,
    registrar: Option<SecretRegistrar>,
}

/// What is written to the secret store: the credentials and the server they
/// belong to. rmcp binds stored tokens to the issuer; the resource binding is
/// ours, so a token is never offered to another URL configured under the
/// same server name.
#[derive(serde::Serialize, serde::Deserialize)]
struct Bound {
    resource: String,
    credentials: StoredCredentials,
}

/// Scheme, host and port in lower case plus the path, without query or
/// fragment: the identity of an MCP server for credential binding.
///
/// # Errors
/// `url` is not an absolute `http(s)` URL.
pub fn canonical_resource(url: &str) -> Result<String> {
    let bad = || McpError::Config(format!("`{url}` is not an absolute http(s) URL"));
    let uri: http::Uri = url
        .split('#')
        .next()
        .unwrap_or(url)
        .parse()
        .map_err(|_| bad())?;
    let scheme = uri
        .scheme_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(bad)?;
    let authority = uri
        .authority()
        .map(|a| a.as_str().to_ascii_lowercase())
        .ok_or_else(bad)?;
    if !matches!(scheme.as_str(), "http" | "https") || authority.contains('@') {
        return Err(bad());
    }
    Ok(format!("{scheme}://{authority}{}", uri.path()))
}

impl std::fmt::Debug for SecretCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretCredentialStore")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl SecretCredentialStore {
    /// Credentials of `server`, reachable at `url`, in `store`.
    ///
    /// # Errors
    /// `url` is not an absolute `http(s)` URL.
    pub fn new(
        store: Arc<dyn SecretStore>,
        server: &str,
        url: &str,
        registrar: Option<SecretRegistrar>,
    ) -> Result<Self> {
        Ok(Self {
            store,
            name: credential_name(server),
            resource: canonical_resource(url)?,
            registrar,
        })
    }

    /// Hand the token values inside `serialized` to the redactor.
    fn register(&self, serialized: &str) {
        let Some(registrar) = &self.registrar else {
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(serialized) else {
            return;
        };
        for field in ["access_token", "refresh_token", "id_token"] {
            if let Some(token) = value["credentials"]["token_response"][field].as_str()
                && !token.is_empty()
            {
                registrar(token);
            }
        }
    }
}

/// A backend error may quote what it was asked to store: keep only its kind.
fn store_error(action: &str) -> AuthError {
    AuthError::CredentialStoreError(format!("secret store could not {action} OAuth credentials"))
}

#[async_trait::async_trait]
impl CredentialStore for SecretCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let Some(value) = self
            .store
            .get(&self.name)
            .map_err(|_| store_error("read"))?
        else {
            return Ok(None);
        };
        // Registered even when it is refused below: it is still a live token.
        self.register(value.expose());
        let bound: Bound =
            serde_json::from_str(value.expose()).map_err(|_| store_error("decode"))?;
        if bound.resource != self.resource {
            // The server name now points somewhere else. These credentials
            // belong to the old URL and must not be sent to the new one.
            tracing::warn!(
                secret = %self.name,
                "stored MCP OAuth credentials belong to a different server URL; discarding them"
            );
            self.store
                .delete(&self.name)
                .map_err(|_| store_error("delete"))?;
            return Ok(None);
        }
        Ok(Some(bound.credentials))
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let serialized = serde_json::to_string(&Bound {
            resource: self.resource.clone(),
            credentials,
        })
        .map_err(|_| store_error("encode"))?;
        // Before the store write: the value must be masked by the time anything
        // could log it.
        self.register(&serialized);
        self.store
            .set(&self.name, &serialized)
            .map_err(|_| store_error("write"))
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        self.store
            .delete(&self.name)
            .map(|_| ())
            .map_err(|_| store_error("delete"))
    }
}

/// Refuse authorization-server metadata the flow must not be run against.
///
/// rmcp tolerates metadata without `code_challenge_methods_supported`; the MCP
/// authorization spec says a client must refuse to proceed then, so that is
/// enforced here. Endpoints must be `https`; plain `http` is accepted only for
/// a loopback host and only when the host opted in (local development).
///
/// # Errors
/// [`McpError::Config`] naming the rule that was broken.
pub fn check_metadata(
    server: &str,
    metadata: &AuthorizationMetadata,
    allow_loopback_http: bool,
) -> Result<()> {
    let refuse = |why: &str| {
        Err(McpError::Config(format!(
            "server `{server}`: authorization server refused: {why}"
        )))
    };
    if !metadata
        .code_challenge_methods_supported
        .as_ref()
        .is_some_and(|methods| methods.iter().any(|m| m == "S256"))
    {
        return refuse("it does not advertise PKCE with S256");
    }
    for endpoint in [&metadata.authorization_endpoint, &metadata.token_endpoint] {
        let Ok(uri) = endpoint.parse::<http::Uri>() else {
            return refuse("an endpoint is not a valid URL");
        };
        let host = uri.host().unwrap_or_default().trim_matches(['[', ']']);
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        match uri.scheme_str() {
            Some("https") => {}
            Some("http") if loopback && allow_loopback_http => {}
            _ => return refuse("its endpoints must use https"),
        }
    }
    Ok(())
}

/// Client identity and policy of one sign-in.
#[derive(Clone, Default)]
pub struct SignInOptions {
    /// Client id registered with the authorization server out of band.
    pub client_id: Option<String>,
    /// Secret of that client (resolved from the secret store by the host).
    pub client_secret: Option<String>,
    /// HTTPS URL of our Client ID Metadata Document, if one is published.
    pub client_metadata_url: Option<String>,
    /// `WWW-Authenticate` value of the server's 401, to seed discovery.
    pub challenge: Option<String>,
    /// Accept `http` endpoints on loopback (local development only).
    pub allow_loopback_http: bool,
}

impl std::fmt::Debug for SignInOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInOptions")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("client_metadata_url", &self.client_metadata_url)
            .field("allow_loopback_http", &self.allow_loopback_http)
            .finish_non_exhaustive()
    }
}

/// An OAuth error may quote a server response; keep it short and one line.
fn auth_error(server: &str, error: &AuthError) -> McpError {
    let text: String = error
        .to_string()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect();
    flow_error(server, &text)
}

/// A sign-in the user started: discovery and client registration are done,
/// the browser has to be sent to [`SignIn::authorization_url`], and
/// [`SignIn::finish`] waits for the redirect and stores the tokens.
///
/// Only the host may create one, on an explicit user action. Nothing in this
/// crate starts a sign-in on behalf of a server, a tool or a model.
pub struct SignIn {
    server: String,
    session: AuthorizationSession,
    listener: CallbackListener,
    state: String,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl SignIn {
    /// Discover the authorization server of the MCP server at `url`, check
    /// it, pick a client identity (pre-registered, then Client ID Metadata
    /// Document, then dynamic registration) and open the redirect listener.
    ///
    /// # Errors
    /// Discovery, policy ([`check_metadata`], rmcp's address filter) or
    /// registration failure. Nothing is stored in that case.
    pub async fn start(
        server: &str,
        url: &str,
        store: Arc<dyn SecretStore>,
        registrar: Option<SecretRegistrar>,
        options: SignInOptions,
    ) -> Result<Self> {
        let credentials = SecretCredentialStore::new(store, server, url, registrar)?;
        let mut manager = AuthorizationManager::new(url)
            .await
            .map_err(|e| auth_error(server, &e))?;
        manager.set_credential_store(credentials);
        let resolution = manager
            .resolve_metadata_from_challenge(options.challenge.as_deref())
            .await
            .map_err(|e| auth_error(server, &e))?;
        check_metadata(server, &resolution.metadata, options.allow_loopback_http)?;
        manager.set_metadata(resolution.metadata);

        let listener = CallbackListener::bind(server).await?;
        let mut request =
            AuthorizationRequest::new(listener.redirect_uri()).with_client_name("Praxi");
        if let Some(id) = options.client_id {
            request = request.with_preregistered_client(id);
            if let Some(secret) = options.client_secret {
                request = request.with_client_secret(secret);
            }
        }
        if let Some(document) = options.client_metadata_url {
            request = request.with_client_metadata_url(document);
        }
        if let Some(challenge) = options.challenge {
            request = request.with_challenge(challenge);
        }
        let session = AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| auth_error(server, &e))?;
        let state = query_param(session.get_authorization_url(), "state")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| flow_error(server, "the authorization URL carries no `state`"))?;
        Ok(Self {
            server: server.to_owned(),
            session,
            listener,
            state,
        })
    }

    /// The URL the user's browser must open.
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        self.session.get_authorization_url()
    }

    /// Wait for the redirect, exchange the code (PKCE verifier, `resource`,
    /// RFC 9207 issuer check are rmcp's) and store the tokens.
    ///
    /// # Errors
    /// Timeout, a wrong or replayed redirect, or a refused exchange.
    pub async fn finish(self, timeout: Duration) -> Result<()> {
        let callback = self
            .listener
            .wait(&self.server, &self.state, timeout)
            .await?;
        self.session
            .handle_callback_with_issuer(&callback.code, &self.state, callback.issuer.as_deref())
            .await
            .map(|_| ())
            .map_err(|e| auth_error(&self.server, &e))
    }
}

/// What the authorization server sent back on the redirect.
#[derive(Clone, PartialEq, Eq)]
pub struct Callback {
    /// The authorization code.
    pub code: String,
    /// The `iss` parameter (RFC 9207), when the server sent one.
    pub issuer: Option<String>,
}

impl std::fmt::Debug for Callback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Callback")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

/// One-shot redirect endpoint on the loopback interface.
#[derive(Debug)]
pub struct CallbackListener {
    listener: tokio::net::TcpListener,
    port: u16,
}

fn flow_error(server: &str, message: &str) -> McpError {
    McpError::Transport {
        server: server.to_owned(),
        message: format!("OAuth sign-in failed: {message}"),
    }
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = raw.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
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
    String::from_utf8(out).ok()
}

/// Query parameter `key` of the request line `GET /callback?... HTTP/1.1`.
fn query_param(target: &str, key: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| percent_decode(v))
}

impl CallbackListener {
    /// Bind a random port on `127.0.0.1` (never another interface).
    ///
    /// # Errors
    /// The socket cannot be bound.
    pub async fn bind(server: &str) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| flow_error(server, &format!("cannot open the redirect listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| flow_error(server, &e.to_string()))?
            .port();
        Ok(Self { listener, port })
    }

    /// The `redirect_uri` to register and to send in the authorization request.
    #[must_use]
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// Wait for the redirect. The first request decides: it must be
    /// `GET /callback` with the expected `state` and a `code`. The listener is
    /// consumed, so a second redirect finds the port closed.
    ///
    /// # Errors
    /// Timeout, a request that is not the expected redirect, a `state`
    /// mismatch, or an `error` answer from the authorization server.
    pub async fn wait(
        self,
        server: &str,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<Callback> {
        let Self { listener, .. } = self;
        let received = tokio::time::timeout(timeout, async {
            let (mut socket, peer) = listener.accept().await?;
            // Stop listening before the request is even read.
            drop(listener);
            if !peer.ip().is_loopback() {
                return Ok::<_, std::io::Error>((socket, None));
            }
            let mut buf = Vec::new();
            let mut chunk = [0u8; 2048];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_REQUEST_BYTES {
                let n = socket.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf);
            let target = head
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("GET "))
                .and_then(|rest| rest.split(' ').next())
                .map(str::to_owned);
            Ok((socket, target))
        })
        .await
        .map_err(|_| flow_error(server, "no redirect arrived before the time limit"))?
        .map_err(|e| flow_error(server, &e.to_string()))?;
        let (mut socket, target) = received;

        let outcome = match target.as_deref() {
            Some(target) if target == "/callback" || target.starts_with("/callback?") => {
                // Compare the state before anything else is looked at.
                if query_param(target, "state").as_deref() != Some(expected_state) {
                    Err("the redirect carried a wrong `state`")
                } else if query_param(target, "error").is_some() {
                    Err("the authorization server refused the request")
                } else {
                    match query_param(target, "code").filter(|c| !c.is_empty()) {
                        Some(code) => Ok(Callback {
                            code,
                            issuer: query_param(target, "iss"),
                        }),
                        None => Err("the redirect carried no authorization code"),
                    }
                }
            }
            _ => Err("the redirect listener received an unexpected request"),
        };
        let (status, body) = match &outcome {
            Ok(_) => ("200 OK", "Signed in. You can close this window."),
            Err(_) => (
                "400 Bad Request",
                "Sign-in failed. Return to the app and try again.",
            ),
        };
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
                     Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        let _ = socket.shutdown().await;
        outcome.map_err(|message| flow_error(server, message))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use parking_lot::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<String, String>>);

    impl SecretStore for MemoryStore {
        fn get(
            &self,
            name: &str,
        ) -> std::result::Result<Option<secret_store::SecretValue>, secret_store::SecretError>
        {
            Ok(self.0.lock().get(name).map(secret_store::SecretValue::new))
        }
        fn set(
            &self,
            name: &str,
            value: &str,
        ) -> std::result::Result<(), secret_store::SecretError> {
            self.0.lock().insert(name.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(&self, name: &str) -> std::result::Result<bool, secret_store::SecretError> {
            Ok(self.0.lock().remove(name).is_some())
        }
        fn list_names(&self) -> std::result::Result<Vec<String>, secret_store::SecretError> {
            Ok(self.0.lock().keys().cloned().collect())
        }
    }

    fn credentials(access: &str, refresh: &str) -> StoredCredentials {
        let token = serde_json::from_value(serde_json::json!({
            "access_token": access,
            "token_type": "bearer",
            "refresh_token": refresh,
            "expires_in": 3600
        }))
        .unwrap();
        StoredCredentials::new("client-1".into(), Some(token), vec!["read".into()], Some(1))
            .with_issuer(Some("https://as.example".into()))
    }

    #[tokio::test]
    async fn credentials_live_only_in_the_secret_store_and_are_registered_for_redaction() {
        let backend = Arc::new(MemoryStore::default());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let registrar: SecretRegistrar = {
            let seen = Arc::clone(&seen);
            Arc::new(move |value: &str| seen.lock().push(value.to_owned()))
        };
        let store = SecretCredentialStore::new(
            backend.clone(),
            "notion",
            "https://MCP.notion.example/mcp#frag",
            Some(registrar.clone()),
        )
        .unwrap();
        assert!(store.load().await.unwrap().is_none());

        store
            .save(credentials("access-sentinel-1", "refresh-sentinel-1"))
            .await
            .unwrap();
        assert_eq!(
            backend.list_names().unwrap(),
            vec!["mcp/notion/oauth".to_owned()]
        );
        assert_eq!(
            *seen.lock(),
            vec![
                "access-sentinel-1".to_owned(),
                "refresh-sentinel-1".to_owned()
            ]
        );
        assert!(!format!("{store:?}").contains("sentinel"));

        // A fresh process registers the stored tokens again when it loads them.
        seen.lock().clear();
        let loaded = store.load().await.unwrap().unwrap();
        assert_eq!(loaded.client_id, "client-1");
        assert_eq!(loaded.issuer.as_deref(), Some("https://as.example"));
        assert!(!format!("{loaded:?}").contains("sentinel"));
        assert_eq!(seen.lock().len(), 2);

        // Another server's credentials are a different secret.
        let other =
            SecretCredentialStore::new(backend.clone(), "linear", "https://l.example/mcp", None)
                .unwrap();
        assert!(other.load().await.unwrap().is_none());

        // Same server name, another URL: the token is not handed out and is
        // removed, but still registered for redaction on the way.
        seen.lock().clear();
        let moved = SecretCredentialStore::new(
            backend.clone(),
            "notion",
            "https://attacker.example/mcp",
            Some(registrar),
        )
        .unwrap();
        assert!(moved.load().await.unwrap().is_none());
        assert_eq!(seen.lock().len(), 2);
        assert_eq!(backend.list_names().unwrap(), Vec::<String>::new());
        assert!(store.load().await.unwrap().is_none());
        store
            .save(credentials("access-sentinel-1", "refresh-sentinel-1"))
            .await
            .unwrap();

        store.clear().await.unwrap();
        assert_eq!(backend.list_names().unwrap(), Vec::<String>::new());
        assert!(store.load().await.unwrap().is_none());
    }

    struct Failing;

    impl SecretStore for Failing {
        fn get(
            &self,
            _: &str,
        ) -> std::result::Result<Option<secret_store::SecretValue>, secret_store::SecretError>
        {
            Err(secret_store::SecretError::Keyring(
                "backend-sentinel".into(),
            ))
        }
        fn set(&self, _: &str, value: &str) -> std::result::Result<(), secret_store::SecretError> {
            Err(secret_store::SecretError::Keyring(value.to_owned()))
        }
        fn delete(&self, _: &str) -> std::result::Result<bool, secret_store::SecretError> {
            Err(secret_store::SecretError::Keyring(
                "backend-sentinel".into(),
            ))
        }
        fn list_names(&self) -> std::result::Result<Vec<String>, secret_store::SecretError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn backend_errors_never_carry_token_or_backend_text() {
        let store =
            SecretCredentialStore::new(Arc::new(Failing), "s", "https://s.example/mcp", None)
                .unwrap();
        let save = store
            .save(credentials("access-sentinel-2", "refresh-sentinel-2"))
            .await
            .unwrap_err()
            .to_string();
        let load = store.load().await.unwrap_err().to_string();
        for text in [save, load] {
            assert!(!text.contains("sentinel"), "{text}");
            assert!(text.contains("secret store"), "{text}");
        }
    }

    #[test]
    fn resources_are_canonical_and_metadata_is_checked() {
        assert_eq!(
            canonical_resource("HTTPS://Mcp.Example.com:8443/mcp?x=1#f").unwrap(),
            "https://mcp.example.com:8443/mcp"
        );
        for bad in [
            "mcp.example.com/mcp",
            "ftp://x/y",
            "https://user:pw@x/y",
            "",
        ] {
            assert!(canonical_resource(bad).is_err(), "{bad}");
        }

        let metadata = |authorize: &str, token: &str, methods: Option<&[&str]>| {
            let mut m = AuthorizationMetadata::default();
            m.authorization_endpoint = authorize.into();
            m.token_endpoint = token.into();
            m.code_challenge_methods_supported =
                methods.map(|list| list.iter().map(|s| (*s).to_owned()).collect());
            m
        };
        let s256: Option<&[&str]> = Some(&["S256"]);
        let ok = metadata(
            "https://as.example/authorize",
            "https://as.example/token",
            s256,
        );
        check_metadata("s", &ok, false).unwrap();
        // The spec requires refusing a server that does not advertise PKCE.
        for methods in [None, Some(&["plain"][..])] {
            let m = metadata(
                "https://as.example/authorize",
                "https://as.example/token",
                methods,
            );
            let error = check_metadata("s", &m, true).unwrap_err().to_string();
            assert!(error.contains("S256"), "{error}");
        }
        for (authorize, token) in [
            ("http://as.example/authorize", "https://as.example/token"),
            (
                "https://as.example/authorize",
                "http://169.254.169.254/token",
            ),
            ("https://as.example/authorize", "http://10.0.0.5/token"),
            ("file:///etc/passwd", "https://as.example/token"),
        ] {
            let m = metadata(authorize, token, s256);
            assert!(
                check_metadata("s", &m, true).is_err(),
                "{authorize} {token}"
            );
        }
        let local = metadata(
            "http://127.0.0.1:9/authorize",
            "http://localhost:9/token",
            s256,
        );
        assert!(check_metadata("s", &local, false).is_err());
        check_metadata("s", &local, true).unwrap();
    }

    async fn get(port: u16, target: &str) -> std::io::Result<String> {
        let mut socket = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        socket
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
            .await?;
        let mut out = String::new();
        socket.read_to_string(&mut out).await?;
        Ok(out)
    }

    fn port_of(listener: &CallbackListener) -> u16 {
        let uri = listener.redirect_uri();
        assert!(uri.starts_with("http://127.0.0.1:"), "{uri}");
        assert!(uri.ends_with("/callback"), "{uri}");
        assert!(
            listener.listener.local_addr().unwrap().ip().is_loopback(),
            "the redirect listener must only bind to loopback"
        );
        listener.port
    }

    #[tokio::test]
    async fn the_redirect_is_accepted_once_with_the_expected_state() {
        let listener = CallbackListener::bind("s").await.unwrap();
        let port = port_of(&listener);
        let waiting = tokio::spawn(listener.wait("s", "state-123", Duration::from_secs(10)));
        let page = get(
            port,
            "/callback?code=abc%2Fdef&state=state-123&iss=https%3A%2F%2Fas.example",
        )
        .await
        .unwrap();
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        let callback = waiting.await.unwrap().unwrap();
        assert_eq!(callback.code, "abc/def");
        assert_eq!(callback.issuer.as_deref(), Some("https://as.example"));
        assert!(!format!("{callback:?}").contains("abc"));
        // The listener is gone: a replayed redirect cannot be delivered.
        assert!(
            get(port, "/callback?code=again&state=state-123")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_wrong_state_ends_the_flow_and_closes_the_listener() {
        for target in [
            "/callback?code=stolen&state=other",
            "/callback?code=stolen",
            "/callback?state=state-123",
            "/callback?state=state-123&error=access_denied&code=x",
            "/elsewhere?code=x&state=state-123",
        ] {
            let listener = CallbackListener::bind("s").await.unwrap();
            let port = port_of(&listener);
            let waiting = tokio::spawn(listener.wait("s", "state-123", Duration::from_secs(10)));
            let page = get(port, target).await.unwrap();
            assert!(page.starts_with("HTTP/1.1 400"), "{target}: {page}");
            let error = waiting.await.unwrap().unwrap_err().to_string();
            assert!(error.contains("OAuth sign-in failed"), "{target}: {error}");
            assert!(!error.contains("stolen"), "{error}");
            // No second chance for the correct state either.
            assert!(
                get(port, "/callback?code=abc&state=state-123")
                    .await
                    .is_err(),
                "{target}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_listener_times_out() {
        let listener = CallbackListener::bind("s").await.unwrap();
        let error = listener
            .wait("s", "state-123", CALLBACK_TIMEOUT)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("time limit"), "{error}");
    }
}
