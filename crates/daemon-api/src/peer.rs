//! Authenticated ingress for paired devices. The transport supplies identity;
//! HTTP headers can never grant a role.
use crate::{ApiError, AppState};
use axum::{
    Router,
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Role granted by the pairing store, never by the remote HTTP client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRole {
    Desktop,
    Phone,
    Node,
    Host,
}

/// Authenticated transport identity.
#[derive(Clone, Debug)]
pub struct PeerIdentity {
    pub key: String,
    pub role: PeerRole,
}

/// Generic JSON/TOML profile editors retain ordinary bot editing, but cannot
/// create, replace or clear host-local knowledge settings. Omission preserves
/// the current subtree rather than implicitly disabling an owner configuration.
pub(crate) fn protect_knowledge_config(
    peer: Option<&PeerIdentity>,
    current: Option<&serde_json::Value>,
    next: &mut serde_json::Value,
) -> Result<(), ApiError> {
    if peer.is_none() {
        return Ok(());
    }
    if let Some(incoming) = next.get("knowledge") {
        if Some(incoming) != current {
            return Err(ApiError::forbidden(
                "knowledge settings are managed by the local owner",
            ));
        }
    } else if let Some(current) = current {
        next.as_object_mut()
            .ok_or_else(|| ApiError::bad_request("bot config must be an object"))?
            .insert("knowledge".into(), current.clone());
    }
    Ok(())
}

/// Future host transfer endpoints forbidden to phones.
pub const PHONE_DENIED_POST_PATHS: &[&str] = &["/v1/host/import", "/v1/host/handover"];

/// `POST /v1/runs/stop`: only phones and desktops may stop runs (and then only
/// with scope `listed`: the handler refuses `all_existing` to every peer). The
/// request is attributed in the audit log like an approval. Pinned here so the
/// route stays closed to node and host peers even if the blanket role rule
/// below is ever loosened.
pub const PEER_RUN_STOP_PATH: &str = "/v1/runs/stop";

/// Host-local provider management endpoints forbidden to ALL paired peers.
pub const PEER_DENIED_POST_PATHS: &[&str] = &["/v1/providers/cli/enable", "/v1/providers/reload"];

/// Host-local diagnostics forbidden to ALL paired peers, for every method.
/// Repairs (`/v1/doctor/repair/*`) are actions of the user at this host's
/// desktop UI; the check list and the support report describe the host (paths,
/// platform, provider set), so peers do not read them either.
pub const PEER_DENIED_PREFIX_DOCTOR: &str = "/v1/doctor";

/// `/v1/hosts…` manages THIS computer's links to paired hosts (pair, connect,
/// disconnect). It is an action of the local user at this computer's UI and is
/// closed to every paired peer role, for every method: a peer must never be
/// able to make this daemon open a channel to another host or read the
/// loopback proxy token it hands out.
pub const PEER_DENIED_PREFIX_HOSTS: &str = "/v1/hosts";

fn is_hosts_path(path: &str) -> bool {
    path.strip_prefix(PEER_DENIED_PREFIX_HOSTS)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn is_mutating(method: &Method) -> bool {
    *method != Method::GET && *method != Method::HEAD && *method != Method::OPTIONS
}

/// `path` is `prefix` or below it (decoded and slash-normalized).
fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Profile parts that decide what a bot may do without asking: the permission
/// preset, per-category levels and unattended rules (`permissions`) and the
/// tool allow/exclude lists (`tools`).
const EXECUTION_KEYS: [&str; 2] = ["permissions", "tools"];

/// On a host a paired peer may edit a bot (name, persona, language, model
/// choice among configured providers) but never its execution authority:
/// `permissions`, `tools`, or switching the executor to an external `cli:`
/// engine. A save that repeats the stored values is fine; creating a bot
/// with these parts, or changing them, is refused; omitting them on update
/// keeps the stored ones.
pub(crate) fn protect_execution_config(
    peer: Option<&PeerIdentity>,
    host_mode: bool,
    current: Option<&serde_json::Value>,
    next: &mut serde_json::Value,
) -> Result<(), ApiError> {
    if peer.is_none() || !host_mode {
        return Ok(());
    }
    let refuse = || {
        ApiError::forbidden(
            "permissions and tools of a bot on a host are managed on the host itself",
        )
    };
    for key in EXECUTION_KEYS {
        let cur = current.and_then(|c| c.get(key));
        match (next.get(key), cur) {
            (Some(incoming), cur) if Some(incoming) != cur => return Err(refuse()),
            (None, Some(cur)) => {
                next.as_object_mut()
                    .ok_or_else(|| ApiError::bad_request("bot config must be an object"))?
                    .insert(key.into(), cur.clone());
            }
            _ => {}
        }
    }
    let executor = |v: &serde_json::Value| v.pointer("/models/executor").cloned();
    if let Some(new) = executor(next)
        && new.as_str().is_some_and(|e| e.starts_with("cli:"))
        && Some(new) != current.and_then(executor)
    {
        return Err(refuse());
    }
    Ok(())
}

/// What a Desktop peer may NOT administer on a daemon that runs as an
/// always-on host (`mode = host`). On such a host the desktop is an operator
/// of bots, runs, chats and the screen, not an administrator of credentials,
/// tool servers, skills or pairing: those stay with the host's own local user
/// (CLI on the host). Returns the refusal reason for `method path`.
fn host_mode_desktop_denial(method: &Method, check_path: &str) -> Option<&'static str> {
    let mutating = is_mutating(method);
    let under = |prefix: &str| under(check_path, prefix);
    if under("/v1/policy") && mutating {
        return Some("the execution policy is managed on the host itself");
    }
    if under("/v1/secrets") && mutating {
        return Some("secrets are managed on the host itself");
    }
    if (under("/v1/mcp") || under("/v1/skills") || is_bot_skills_path(check_path)) && mutating {
        return Some("tool servers and skills are managed on the host itself");
    }
    if (under("/v1/email") || under("/v1/web/search")) && mutating {
        return Some("account credentials are managed on the host itself");
    }
    if *method == Method::POST
        && (PHONE_DENIED_POST_PATHS.contains(&check_path)
            || check_path == "/v1/channels/telegram/pair-code")
    {
        return Some("host transfer and channel pairing are done on the host itself");
    }
    None
}

/// `POST /v1/chats` and `POST /v1/chats/{id}/messages` start work on this
/// daemon; a paired peer doing so is attributed in the audit log.
fn starts_chat_work(method: &Method, check_path: &str) -> bool {
    if *method != Method::POST {
        return false;
    }
    if check_path == "/v1/chats" {
        return true;
    }
    check_path
        .strip_prefix("/v1/chats/")
        .and_then(|rest| rest.strip_suffix("/messages"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// Whether `path` (decoded and slash-normalized) is a diagnostics endpoint.
fn is_doctor_path(path: &str) -> bool {
    path.strip_prefix(PEER_DENIED_PREFIX_DOCTOR)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Same API and permissions as loopback, with transport authentication.
pub fn peer_router(state: AppState) -> Router {
    crate::api_routes()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::live_view::audit_ingress,
        ))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Optional paired device display name for audit attribution.
#[derive(Clone, Debug)]
pub struct PeerName(pub String);

fn normalize_deny_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
                prev_slash = true;
            }
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Why `method path` is refused to every paired peer role, if it is: the
/// host-local provider management and diagnostics endpoints.
fn host_local_denial(method: &Method, check_path: &str) -> Option<&'static str> {
    if check_path
        .strip_prefix("/v1/mcp/servers/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(_, rest)| rest == "auth" || rest.starts_with("auth/"))
    {
        return Some("MCP sign-in is a host-local user action, forbidden to peers");
    }
    // MCP configuration decides which secrets are sent where and which
    // commands run on this host: create / update / delete / reconnect are
    // host-local user actions for every peer role (no card or ADR grants a
    // peer role MCP management).  Reading the list and tools stays allowed.
    if *method != Method::GET && is_mcp_servers_path(check_path) {
        return Some("MCP server management is a host-local user action, forbidden to peers");
    }
    if *method == Method::POST && PEER_DENIED_POST_PATHS.contains(&check_path) {
        return Some("host-local provider management is not allowed for peers");
    }
    if is_hosts_path(check_path) {
        return Some("host links are managed by the local user only");
    }
    if !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        && check_path
            .strip_prefix("/v1/bots/")
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(bot, rest)| {
                !bot.is_empty() && (rest == "knowledge" || rest.starts_with("knowledge/"))
            })
    {
        return Some("knowledge sources and embedding authority are managed by the local owner");
    }
    is_doctor_path(check_path).then_some("host-local diagnostics are not allowed for peers")
}

/// `/v1/mcp/servers` or anything below it (decoded and slash-normalized).
fn is_mcp_servers_path(path: &str) -> bool {
    path.strip_prefix("/v1/mcp/servers")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `POST /v1/control/{request_id}` answers an overlay question: local only.
fn answers_overlay_question(path: &str) -> bool {
    path.strip_prefix("/v1/control/")
        .is_some_and(|rest| uuid::Uuid::parse_str(rest).is_ok())
}

/// `POST /v1/runs/stop` from a role that may not stop runs.
fn stop_refused(method: &Method, check_path: &str, role: PeerRole) -> bool {
    *method == Method::POST
        && check_path == PEER_RUN_STOP_PATH
        && !matches!(role, PeerRole::Desktop | PeerRole::Phone)
}

// File contents and host restores require a desktop user.
fn host_file_review(path: &str) -> bool {
    path.starts_with("/v1/runs/") && path.split('/').nth(4) == Some("changes")
}

/// `/v1/bots/{id}/skills` mutates a bot's prompt-trust boundary.
fn is_bot_skills_path(path: &str) -> bool {
    path.strip_prefix("/v1/bots/")
        .and_then(|rest| rest.strip_suffix("/skills"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

fn audit_chat_start(
    state: &AppState,
    peer: &PeerIdentity,
    req: &Request,
    check_path: &str,
) -> Result<(), storage::StorageError> {
    let name = req
        .extensions()
        .get::<PeerName>()
        .map_or(peer.key.as_str(), |n| n.0.as_str());
    let role = if peer.role == PeerRole::Phone {
        "phone"
    } else {
        "desktop"
    };
    state
        .storage()
        .append_audit(
            state.user_id(),
            None,
            &format!("{role}:{name}"),
            "peer.chat_requested",
            &serde_json::json!({"peer_key":peer.key,"path":check_path}),
        )
        .map(|_| ())
}

#[allow(clippy::too_many_lines)] // one ordered chain of guard checks; order is the contract
async fn guard(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(peer) = req.extensions().get::<PeerIdentity>().cloned() else {
        return ApiError::forbidden("paired peer identity required").into_response();
    };
    // Decode and collapse redundant slashes before checking: encoded or
    // non-canonical path characters must not bypass scopes.
    let path = match percent_encoding::percent_decode_str(req.uri().path()).decode_utf8() {
        Ok(path) => path.into_owned(),
        Err(_) => return ApiError::forbidden("invalid path").into_response(),
    };
    let check_path = normalize_deny_path(&path);
    if let Some(why) = host_local_denial(req.method(), &check_path) {
        return ApiError::forbidden(why).into_response();
    }
    let denied = peer.role != PeerRole::Desktop
        && (host_file_review(&check_path)
            || peer.role != PeerRole::Phone
            || (req.method() == Method::POST
                && PHONE_DENIED_POST_PATHS.contains(&check_path.as_str()))
            || (matches!(*req.method(), Method::POST | Method::PUT)
                && check_path.starts_with("/v1/secrets"))
            || (req.method() != Method::GET
                && (is_bot_skills_path(&check_path) || check_path == "/v1/skills")));
    if denied || stop_refused(req.method(), &check_path, peer.role) {
        return ApiError::forbidden("peer role does not allow this operation").into_response();
    }
    if state.leader().config().mode == crate::host::HostMode::Host {
        // Pairing and the host lifecycle (invite, revoke, standby, activate,
        // update, import, handover) are done on the host itself: a paired
        // peer, whatever its role, cannot mint tickets, revoke devices or
        // change who leads.
        if is_mutating(req.method()) && under(&check_path, "/v1/host") {
            return ApiError::forbidden(
                "pairing and host lifecycle are managed on the host itself",
            )
            .into_response();
        }
        if peer.role == PeerRole::Desktop
            && let Some(why) = host_mode_desktop_denial(req.method(), &check_path)
        {
            return ApiError::forbidden(why).into_response();
        }
    }
    if req.method() == Method::POST && answers_overlay_question(&check_path) {
        return ApiError::forbidden("only the local user answers overlay questions")
            .into_response();
    }
    if crate::live_view::reject_query_credentials(&req) {
        return ApiError::bad_request("URL credentials are forbidden").into_response();
    }
    req.extensions_mut()
        .insert(crate::live_view::Actor::peer(&peer));
    let (req, control_transition) = if req.method() == Method::PUT && path == "/v1/control/state" {
        let (parts, body) = req.into_parts();
        let Ok(bytes) = axum::body::to_bytes(body, 4096).await else {
            return ApiError::bad_request("control state body exceeds 4096 bytes").into_response();
        };
        let input: crate::dto::ControlStateInput = match serde_json::from_slice(&bytes) {
            Ok(input) => input,
            Err(e) => return ApiError::bad_request(e.to_string()).into_response(),
        };
        // A device without the input grant is refused by the handler (and that
        // denial is coalesced there): it must not write a decision row per try.
        let granted = state
            .live_view()
            .allowed(&crate::live_view::Actor::peer(&peer), true);
        let changed = granted && (input.release || state.hub().reported_mode() != Some(input.mode));
        (
            Request::from_parts(parts, axum::body::Body::from(bytes)),
            changed,
        )
    } else {
        (req, false)
    };
    // Record the initiator in the existing SQLite audit log before dispatch.
    // Fail closed if attribution cannot be persisted. The normal approval audit
    // and permission engine remain in place and record the resulting decision.
    let mut audit_actor = None;
    let refused_control = matches!(
        path.as_str(),
        "/v1/control/release"
            | "/v1/control/takeover"
            | "/v1/control/override"
            | "/v1/control/recover"
    ) && !state
        .live_view()
        .allowed(&crate::live_view::Actor::peer(&peer), true);
    if (req.method() == Method::POST
        && !refused_control
        && (path.starts_with("/v1/approvals/")
            || path.starts_with("/v1/questions/")
            || path.starts_with("/v1/control/")
            || check_path == PEER_RUN_STOP_PATH))
        || control_transition
    {
        let role = if peer.role == PeerRole::Phone {
            "phone"
        } else {
            "desktop"
        };
        let name = req
            .extensions()
            .get::<PeerName>()
            .map_or(peer.key.as_str(), |n| n.0.as_str());
        let actor = format!("{role}:{name}");
        if let Err(error) = state.storage().append_audit(
            state.user_id(),
            None,
            &actor,
            "peer.decision_requested",
            &serde_json::json!({"peer_key":peer.key,"path":path}),
        ) {
            return ApiError::internal(error).into_response();
        }
        audit_actor = Some(actor);
    }
    if starts_chat_work(req.method(), &check_path)
        && let Err(error) = audit_chat_start(&state, &peer, &req, &check_path)
    {
        // Fail closed: remote work is not started without its attribution row.
        return ApiError::internal(error).into_response();
    }
    let response = next.run(req).await;
    if let Some(actor) = audit_actor
        && let Err(error) = state.storage().append_audit(
            state.user_id(), None, &actor, "peer.decision_completed",
            &serde_json::json!({"peer_key":peer.key,"path":path,"status":response.status().as_u16()}),
        ) { return ApiError::internal(error).into_response(); }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_phone_and_desktop_may_post_run_stop() {
        for (role, refused) in [
            (PeerRole::Phone, false),
            (PeerRole::Desktop, false),
            (PeerRole::Node, true),
            (PeerRole::Host, true),
        ] {
            assert_eq!(
                stop_refused(&Method::POST, PEER_RUN_STOP_PATH, role),
                refused,
                "{role:?}"
            );
        }
        // Other paths and methods are not this rule's business.
        assert!(!stop_refused(&Method::POST, "/v1/runs", PeerRole::Node));
        assert!(!stop_refused(
            &Method::GET,
            PEER_RUN_STOP_PATH,
            PeerRole::Node
        ));
    }
}
