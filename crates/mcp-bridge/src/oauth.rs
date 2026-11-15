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

use oauth2::TokenResponse as _;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, AuthorizationRequest,
    AuthorizationSession, CredentialStore, OAuthClientConfig, StoredCredentials,
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
        check_endpoint_addresses(server, url, &resolution.metadata).await?;
        let use_metadata_document = options.client_metadata_url.is_some()
            && resolution
                .metadata
                .additional_fields
                .get("client_id_metadata_document_supported")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
        manager.set_metadata(resolution.metadata);

        let listener = CallbackListener::bind(server).await?;
        let mut request =
            AuthorizationRequest::new(listener.redirect_uri()).with_client_name("Praxi");
        if let Some(id) = options.client_id.filter(|_| !use_metadata_document) {
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
        self.finish_or_pasted(timeout, None).await
    }

    /// [`SignIn::finish`] for a host without a local browser: whichever comes
    /// first decides, the redirect on the loopback listener or the redirect
    /// URL the user copied from the browser's address bar and pasted (sent on
    /// `pasted`). Both pass the same checks: the URL must be our
    /// `redirect_uri`, the `state` is compared before anything else, and the
    /// flow is consumed by the first answer, right or wrong.
    ///
    /// # Errors
    /// As [`SignIn::finish`]; a pasted URL that is not our redirect.
    pub async fn finish_or_pasted(
        self,
        timeout: Duration,
        pasted: Option<tokio::sync::oneshot::Receiver<String>>,
    ) -> Result<()> {
        let redirect_uri = self.listener.redirect_uri();
        let callback = match pasted {
            None => {
                self.listener
                    .wait(&self.server, &self.state, timeout)
                    .await?
            }
            Some(pasted) => {
                tokio::select! {
                    received = self.listener.wait(&self.server, &self.state, timeout) => received?,
                    Ok(url) = pasted => {
                        pasted_callback(&self.server, &redirect_uri, &url, &self.state)?
                    }
                }
            }
        };
        self.session
            .handle_callback_with_issuer(&callback.code, &self.state, callback.issuer.as_deref())
            .await
            .map(|_| ())
            .map_err(|e| auth_error(&self.server, &e))
    }
}

/// The access token to connect `server` (at `url`) with, refreshed first when
/// at most 60 seconds remain. `Ok(None)` when no sign-in was ever stored for this
/// server and URL: nothing is contacted then.
///
/// # Errors
/// [`McpError::AuthRequired`] when stored credentials can no longer be used
/// (issuer changed, refresh token missing or rejected); a transport error for
/// a transient failure of the authorization server.
pub async fn stored_bearer(
    server: &str,
    url: &str,
    store: &Arc<dyn SecretStore>,
    registrar: Option<&SecretRegistrar>,
    force_refresh: bool,
    options: &SignInOptions,
) -> Result<Option<String>> {
    let probe = SecretCredentialStore::new(Arc::clone(store), server, url, registrar.cloned())?;
    let Some(stored) = probe
        .load()
        .await
        .map_err(|e| auth_error(server, &e))?
        .filter(|c| c.token_response.is_some())
    else {
        return Ok(None);
    };
    let refresh_due = {
        let expiry = stored
            .token_response
            .as_ref()
            .and_then(oauth2::TokenResponse::expires_in);
        match (expiry, stored.token_received_at) {
            (Some(expiry), Some(received)) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                received
                    .saturating_add(expiry.as_secs())
                    .saturating_sub(now)
                    <= 60
            }
            _ => false,
        }
    };
    // rmcp's own HTTP client: its address filter stays in place.
    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|e| auth_error(server, &e))?;
    manager.set_credential_store(probe);
    // Apply our proxy-independent endpoint policy on refresh as well as sign-in.
    let resolution = manager
        .resolve_metadata()
        .await
        .map_err(|e| auth_error(server, &e))?;
    check_metadata(server, &resolution.metadata, options.allow_loopback_http)?;
    check_endpoint_addresses(server, url, &resolution.metadata).await?;
    manager.set_metadata(resolution.metadata);
    match manager.initialize_from_store().await {
        Ok(true) => {}
        Ok(false) => return Err(McpError::AuthRequired(server.to_owned())),
        Err(e) => return Err(auth_error(server, &e)),
    }
    let stored_id = &stored.client_id;
    let metadata_client = options.client_metadata_url.as_ref() == Some(stored_id);
    if !metadata_client && options.client_id.as_ref().is_some_and(|id| id != stored_id) {
        return Err(McpError::AuthRequired(server.to_owned()));
    }
    // Reinstall the confidential identity after initialize_from_store, which
    // restores only the public client id. Refresh never reads secrets from config.
    let mut client = OAuthClientConfig::new(stored_id, "http://127.0.0.1/callback");
    if !metadata_client && let Some(secret) = &options.client_secret {
        client = client.with_client_secret(secret);
    }
    manager
        .configure_client(client)
        .map_err(|e| auth_error(server, &e))?;
    let token = if force_refresh || refresh_due {
        manager
            .refresh_token()
            .await
            .map(|t| t.access_token().secret().clone())
    } else {
        manager.get_access_token().await
    };
    match token {
        Ok(token) => {
            if let Some(registrar) = registrar {
                registrar(&token);
            }
            Ok(Some(token))
        }
        Err(AuthError::AuthorizationRequired | AuthError::TokenRefreshRejected(_)) => {
            Err(McpError::AuthRequired(server.to_owned()))
        }
        Err(e) => Err(auth_error(server, &e)),
    }
}

/// Addresses no authorization or token endpoint may have, whatever the
/// server: link-local (cloud metadata), unspecified, multicast, broadcast.
fn always_blocked(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast()
        }
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => always_blocked(std::net::IpAddr::V4(v4)),
            None => {
                v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        },
    }
}

fn internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => internal(std::net::IpAddr::V4(v4)),
            None => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        },
    }
}

/// DNS lookup used for the endpoint policy, independent of the HTTP client
/// and any configured proxy. Injectable so policy tests never use real DNS.
#[async_trait::async_trait]
pub trait EndpointResolver: Send + Sync {
    /// Resolve all answers; absence or an empty answer fails closed.
    async fn lookup(&self, host: &str, port: u16) -> Option<Vec<std::net::IpAddr>>;
}

struct SystemEndpointResolver;

#[async_trait::async_trait]
impl EndpointResolver for SystemEndpointResolver {
    async fn lookup(&self, host: &str, port: u16) -> Option<Vec<std::net::IpAddr>> {
        Some(
            tokio::net::lookup_host((host, port))
                .await
                .ok()?
                .map(|a| a.ip())
                .collect(),
        )
    }
}

async fn addresses(url: &str, resolver: &dyn EndpointResolver) -> Option<Vec<std::net::IpAddr>> {
    let uri: http::Uri = url.parse().ok()?;
    let host = uri.host()?.trim_matches(['[', ']']).to_owned();
    if let Ok(ip) = host.parse() {
        return Some(vec![ip]);
    }
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });
    resolver.lookup(&host, port).await
}

/// Resolve the authorization and token endpoints ourselves and refuse
/// internal targets. rmcp filters the addresses its own client connects to,
/// but not when a proxy (`HTTPS_PROXY`) does the connecting; this check does
/// not depend on who connects. Link-local and other reserved addresses are
/// always refused; loopback and private ones only pass when the MCP server
/// itself is at such an address.
///
/// # Errors
/// [`McpError::Config`] when an endpoint cannot be resolved or is internal.
pub async fn check_endpoint_addresses(
    server: &str,
    server_url: &str,
    metadata: &AuthorizationMetadata,
) -> Result<()> {
    check_endpoint_addresses_with_resolver(server, server_url, metadata, &SystemEndpointResolver)
        .await
}

/// [`check_endpoint_addresses`] using a supplied DNS resolver. This controls
/// the policy lookup only; rmcp's address-filtered HTTP client is unchanged.
///
/// # Errors
/// Unresolvable, reserved or disallowed private endpoint addresses.
pub async fn check_endpoint_addresses_with_resolver(
    server: &str,
    server_url: &str,
    metadata: &AuthorizationMetadata,
    resolver: &dyn EndpointResolver,
) -> Result<()> {
    let refuse = |why: &str| {
        Err(McpError::Config(format!(
            "server `{server}`: authorization server refused: {why}"
        )))
    };
    let server_internal = addresses(server_url, resolver)
        .await
        .is_some_and(|found| !found.is_empty() && found.iter().all(|ip| internal(*ip)));
    for endpoint in [&metadata.authorization_endpoint, &metadata.token_endpoint] {
        let Some(found) = addresses(endpoint, resolver)
            .await
            .filter(|f| !f.is_empty())
        else {
            return refuse("an endpoint host cannot be resolved");
        };
        if found.iter().any(|ip| always_blocked(*ip)) {
            return refuse("an endpoint is at a link-local or reserved address");
        }
        if !server_internal && found.iter().any(|ip| internal(*ip)) {
            return refuse("an endpoint of a public server is at a private or loopback address");
        }
    }
    Ok(())
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

/// Check a redirect target (`/callback?...`) against the expected `state`.
fn parse_callback(
    target: &str,
    expected_state: &str,
) -> std::result::Result<Callback, &'static str> {
    if !(target == "/callback" || target.starts_with("/callback?")) {
        return Err("the redirect listener received an unexpected request");
    }
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

/// A redirect URL the user pasted: it must be exactly our `redirect_uri`
/// (scheme, loopback host, port, path) plus the query.
fn pasted_callback(
    server: &str,
    redirect_uri: &str,
    pasted: &str,
    expected_state: &str,
) -> Result<Callback> {
    let pasted = pasted.trim();
    let target = pasted
        .strip_prefix(redirect_uri)
        .filter(|rest| rest.is_empty() || rest.starts_with('?'))
        .map(|rest| format!("/callback{rest}"))
        .ok_or_else(|| flow_error(server, "the pasted address is not this sign-in's redirect"))?;
    parse_callback(&target, expected_state).map_err(|message| flow_error(server, message))
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
            Some(target) => parse_callback(target, expected_state),
            None => Err("the redirect listener received an unexpected request"),
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
mod paste_tests {
    use super::*;

    const REDIRECT: &str = "http://127.0.0.1:4711/callback";

    #[test]
    fn a_pasted_redirect_passes_the_same_checks_as_the_listener() {
        let ok = pasted_callback(
            "s",
            REDIRECT,
            "  http://127.0.0.1:4711/callback?code=abc&state=st&iss=https%3A%2F%2Fa.example \n",
            "st",
        )
        .unwrap();
        assert_eq!(ok.code, "abc");
        assert_eq!(ok.issuer.as_deref(), Some("https://a.example"));
        for (url, why) in [
            (
                "http://127.0.0.1:4711/callback?code=abc&state=other",
                "wrong `state`",
            ),
            (
                "http://127.0.0.1:4711/callback?state=st",
                "no authorization code",
            ),
            (
                "http://127.0.0.1:4711/callback?error=access_denied&state=st",
                "refused",
            ),
            (
                "http://127.0.0.1:4712/callback?code=abc&state=st",
                "not this sign-in",
            ),
            (
                "http://evil.example/callback?code=abc&state=st",
                "not this sign-in",
            ),
            (
                "http://127.0.0.1:4711/callbackx?code=abc&state=st",
                "not this sign-in",
            ),
            (
                "https://127.0.0.1:4711/callback?code=abc&state=st",
                "not this sign-in",
            ),
            ("code=abc&state=st", "not this sign-in"),
        ] {
            let err = pasted_callback("s", REDIRECT, url, "st")
                .unwrap_err()
                .to_string();
            assert!(err.contains(why), "{url}: {err}");
        }
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

    #[tokio::test]
    async fn internal_authorization_endpoints_are_refused_whoever_connects() {
        let metadata = |authorize: &str, token: &str| {
            let mut m = AuthorizationMetadata::default();
            m.authorization_endpoint = authorize.into();
            m.token_endpoint = token.into();
            m
        };
        let public = "https://93.184.216.34/mcp";
        let ok = "https://93.184.216.34/authorize";
        // Cloud metadata and other reserved targets: never, even for a
        // server that is itself on loopback or a private network.
        for bad in [
            "https://169.254.169.254/latest/meta-data/",
            "https://[::ffff:169.254.169.254]/token",
            "https://[fe80::1]/token",
            "https://0.0.0.0/token",
        ] {
            for server_url in [public, "http://127.0.0.1:9/mcp", "https://10.0.0.2/mcp"] {
                let error = check_endpoint_addresses("s", server_url, &metadata(ok, bad))
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("link-local or reserved"), "{bad}: {error}");
            }
        }
        // Private and loopback targets: not for a public server.
        for bad in [
            "https://10.0.0.5/token",
            "https://192.168.1.1/token",
            "https://172.16.0.9/token",
            "https://100.64.0.1/token",
            "https://127.0.0.1/token",
            "https://[::1]/token",
            "https://[fd00::1]/token",
        ] {
            let error = check_endpoint_addresses("s", public, &metadata(bad, ok))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("private or loopback"), "{bad}: {error}");
        }
        check_endpoint_addresses("s", public, &metadata(ok, ok))
            .await
            .unwrap();
        check_endpoint_addresses(
            "s",
            "http://127.0.0.1:9/mcp",
            &metadata("http://127.0.0.1:9/authorize", "http://127.0.0.1:9/token"),
        )
        .await
        .unwrap();
    }

    struct TableResolver;

    #[async_trait::async_trait]
    impl EndpointResolver for TableResolver {
        async fn lookup(&self, host: &str, _: u16) -> Option<Vec<std::net::IpAddr>> {
            let answers: &[&str] = match host {
                "public.test" => &["93.184.216.34"],
                "imds.attacker.test" => &["169.254.169.254"],
                "internal.attacker.test" => &["10.0.0.5"],
                "mixed.attacker.test" => &["93.184.216.34", "10.0.0.5"],
                "mixed-reversed.attacker.test" => &["10.0.0.5", "93.184.216.34"],
                "mapped.attacker.test" => &["::ffff:169.254.169.254"],
                "empty.test" => &[],
                _ => return None,
            };
            Some(answers.iter().map(|ip| ip.parse().unwrap()).collect())
        }
    }

    #[tokio::test]
    async fn resolved_hostnames_and_mixed_dns_answers_are_filtered_without_real_dns() {
        for host in [
            "imds.attacker.test",
            "internal.attacker.test",
            "mixed.attacker.test",
            "mixed-reversed.attacker.test",
            "mapped.attacker.test",
            "empty.test",
            "missing.test",
        ] {
            for authorization_endpoint in [true, false] {
                let good = "https://public.test/oauth".to_owned();
                let bad = format!("https://{host}/oauth");
                let mut metadata = AuthorizationMetadata::default();
                metadata.authorization_endpoint = if authorization_endpoint {
                    bad.clone()
                } else {
                    good.clone()
                };
                metadata.token_endpoint = if authorization_endpoint { good } else { bad };
                assert!(
                    check_endpoint_addresses_with_resolver(
                        "s",
                        "https://public.test/mcp",
                        &metadata,
                        &TableResolver
                    )
                    .await
                    .is_err(),
                    "{host}"
                );
            }
        }
        let mut metadata = AuthorizationMetadata::default();
        metadata.authorization_endpoint = "https://public.test/authorize".into();
        metadata.token_endpoint = "https://public.test/token".into();
        check_endpoint_addresses_with_resolver(
            "s",
            "https://public.test/mcp",
            &metadata,
            &TableResolver,
        )
        .await
        .unwrap();
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
