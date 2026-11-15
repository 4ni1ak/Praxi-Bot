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
    if *method == Method::POST && PEER_DENIED_POST_PATHS.contains(&check_path) {
        return Some("host-local provider management is not allowed for peers");
    }
    is_doctor_path(check_path).then_some("host-local diagnostics are not allowed for peers")
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

#[allow(clippy::too_many_lines)] // Keep the existing ordered peer guards together.
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
    if req.method() == Method::POST && answers_overlay_question(&check_path) {
        return ApiError::forbidden("only the local user answers overlay questions")
            .into_response();
    }
    if crate::live_view::reject_query_credentials(&req) {
        return ApiError::bad_request("URL credentials are forbidden").into_response();
    }
    req.extensions_mut()
        .insert(crate::live_view::Actor::peer(&peer));
    // New schedule source/read capabilities remain local-owner only even on
    // encoded or slash-normalized aliases. Existing plain timer requests keep
    // their earlier peer policy; handlers also guard already-configured rows.
    let req = if matches!(*req.method(), Method::POST | Method::PUT)
        && (check_path == "/v1/schedules"
            || check_path
                .strip_prefix("/v1/schedules/")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()))
    {
        let (parts, body) = req.into_parts();
        let Ok(bytes) = axum::body::to_bytes(body, 128 * 1024).await else {
            return ApiError::bad_request("schedule body exceeds limit").into_response();
        };
        if serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|v| v.get("precheck").is_some() || v.get("trigger").is_some())
        {
            return ApiError::forbidden("schedule triggers and prechecks are local-owner only")
                .into_response();
        }
        Request::from_parts(parts, axum::body::Body::from(bytes))
    } else {
        req
    };
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
