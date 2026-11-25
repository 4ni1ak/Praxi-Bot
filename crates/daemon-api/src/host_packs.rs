//! `/v1/hosts/setup/packs…`: capability packs for a server (`LibreOffice`, pandoc, ffmpeg, git ...).
//!
//! Three steps, all for the owner of this computer only (local, never a peer, and the owner key):
//! read what a server has (`GET .../packs/{host_id}`), preview what an install or removal would
//! do (`POST .../packs/plan`, a dry run that changes nothing), and run exactly that preview
//! (`POST .../packs/apply`, a job polled through `GET /v1/hosts/setup/{job}`).
//!
//! The server side is the `host-provision` engine over the owner's own SSH login: the host's
//! service user is unprivileged, so a bot can never install software itself. A pack is addressed
//! by its catalog id only; there is no field that takes a package name or a command.
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    host_setup::{HostSetupError, HostSetupJob},
    routes::ApiJson,
};

/// What the owner wants done with the chosen packs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackAction {
    Install,
    Remove,
}

/// One pack on one server.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostPackInfo {
    /// Catalog id (`office`, `docs`, `ocr`, `media`, `dev`). The app shows its own localised name.
    pub id: String,
    /// `installed`, `partial` (an interrupted install) or `missing`.
    pub status: String,
    /// Rough download size.
    pub size_mb: u32,
    /// Part of the wizard's one-switch basic set.
    pub base: bool,
    /// False for shared system tools that Praxi never removes.
    pub removable: bool,
}

/// One program the fact sheet looked for.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostToolFact {
    pub name: String,
    pub present: bool,
}

/// `GET /v1/hosts/setup/packs/{host_id}`: the server's "installed tools" fact sheet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostPacks {
    pub host_id: String,
    /// Debian and Ubuntu servers only.
    pub supported: bool,
    /// The saved login can install software without a password prompt.
    pub can_admin: bool,
    pub packs: Vec<HostPackInfo>,
    pub tools: Vec<HostToolFact>,
    /// Set when the server could not be read: `connection`, `unsupported` or `failed`.
    #[serde(default)]
    pub error: Option<String>,
    /// Technical detail for "copy details" (sanitised, redacted).
    #[serde(default)]
    pub error_detail: Option<String>,
}

/// `POST /v1/hosts/setup/packs/plan`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PackPlanInput {
    /// Paired host id (`GET /v1/hosts`).
    pub host_id: String,
    pub action: PackAction,
    /// Catalog ids. Anything else is refused.
    pub packs: Vec<String>,
}

/// What would happen to one pack. Nothing has been changed yet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PackPlanItem {
    pub pack: String,
    /// The catalog's packages that would be installed or removed.
    pub packages: Vec<String>,
    /// Packages apt would unpack in total, dependencies included (0 when unknown).
    pub total_packages: u32,
    pub size_mb: u32,
    /// Already as wanted: nothing to do.
    pub noop: bool,
    /// Why it cannot run: `shared`, `dependants`, `unavailable`, `needs_admin`, `unsupported`,
    /// `connection` or `failed`.
    #[serde(default)]
    pub blocked: Option<String>,
}

/// The dry-run preview the owner approves. `plan_id` is single use and expires.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PackPlan {
    pub plan_id: String,
    pub host_id: String,
    pub action: PackAction,
    pub items: Vec<PackPlanItem>,
    /// Rough download size of what would be installed (0 for a removal).
    pub size_mb: u32,
    /// True when at least one item can run.
    pub runnable: bool,
    pub expires_in_secs: u32,
}

/// `POST /v1/hosts/setup/packs/apply`: run exactly the plan the owner saw.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PackApplyInput {
    pub plan_id: String,
}

fn valid_host_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn valid_pack_id(id: &str) -> bool {
    (2..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase())
}

fn setup(s: &AppState) -> Result<std::sync::Arc<dyn crate::host_setup::HostSetup>, ApiError> {
    s.host_setup()
        .ok_or_else(|| ApiError::unavailable("server setup is not available in this daemon"))
}

fn local_owner(
    s: &AppState,
    peer: Option<&Extension<crate::PeerIdentity>>,
    headers: &axum::http::HeaderMap,
) -> Result<(), ApiError> {
    if peer.is_some() {
        return Err(ApiError::forbidden("server setup is local only"));
    }
    s.require_owner(headers)
}

fn audit(s: &AppState, action: &str, detail: &serde_json::Value) -> Result<(), ApiError> {
    s.storage()
        .append_audit(s.user_id(), None, "local-ui", action, detail)
        .map(|_| ())
        .map_err(ApiError::internal)
}

/// Run a blocking SSH call off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, HostSetupError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(ApiError::from)
}

#[utoipa::path(get, operation_id = "hosts_packs_get", path = "/v1/hosts/setup/packs/{host_id}", tag = "host", security(("bearer" = [])), params(("host_id" = String, Path)), responses((status = 200, body = HostPacks), (status = 400, body = crate::error::ErrorBody)))]
pub(crate) async fn get(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
    headers: axum::http::HeaderMap,
    Path(host_id): Path<String>,
) -> Result<Json<HostPacks>, ApiError> {
    local_owner(&s, peer.as_ref(), &headers)?;
    if !valid_host_id(&host_id) {
        return Err(ApiError::bad_request("invalid host id"));
    }
    let setup = setup(&s)?;
    Ok(Json(blocking(move || setup.packs(&host_id)).await?))
}

#[utoipa::path(post, operation_id = "hosts_packs_plan", path = "/v1/hosts/setup/packs/plan", tag = "host", security(("bearer" = [])), request_body = PackPlanInput, responses((status = 200, body = PackPlan), (status = 400, body = crate::error::ErrorBody)))]
pub(crate) async fn plan(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
    headers: axum::http::HeaderMap,
    ApiJson(input): ApiJson<PackPlanInput>,
) -> Result<Json<PackPlan>, ApiError> {
    local_owner(&s, peer.as_ref(), &headers)?;
    if !valid_host_id(&input.host_id) {
        return Err(ApiError::bad_request("invalid host id"));
    }
    if input.packs.is_empty()
        || input.packs.len() > 16
        || !input.packs.iter().all(|p| valid_pack_id(p))
    {
        return Err(ApiError::bad_request("choose at least one tool pack"));
    }
    let setup = setup(&s)?;
    audit(
        &s,
        "host_packs.plan",
        &serde_json::json!({"host": input.host_id, "action": input.action, "packs": input.packs}),
    )?;
    Ok(Json(blocking(move || setup.packs_plan(input)).await?))
}

#[utoipa::path(post, operation_id = "hosts_packs_apply", path = "/v1/hosts/setup/packs/apply", tag = "host", security(("bearer" = [])), request_body = PackApplyInput, responses((status = 200, body = HostSetupJob), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub(crate) async fn apply(
    State(s): State<AppState>,
    peer: Option<Extension<crate::PeerIdentity>>,
    headers: axum::http::HeaderMap,
    ApiJson(input): ApiJson<PackApplyInput>,
) -> Result<Json<HostSetupJob>, ApiError> {
    local_owner(&s, peer.as_ref(), &headers)?;
    if input.plan_id.is_empty()
        || input.plan_id.len() > 64
        || !input
            .plan_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ApiError::bad_request("invalid plan id"));
    }
    let setup = setup(&s)?;
    // The plan (host id, action, pack ids) was audited when it was made; no address is logged.
    audit(
        &s,
        "host_packs.apply",
        &serde_json::json!({"plan": input.plan_id}),
    )?;
    Ok(Json(setup.packs_apply(input)?))
}
