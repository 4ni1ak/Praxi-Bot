//! Missing local components (`/v1/components/*`): the runtimes local apps need
//! (uv, Node.js), whether they are installed, and the one-button install.
//!
//! Owner-only (local token or paired Desktop peer).  The set of installable
//! components is a fixed allow-list in the embedder; the client only names an
//! id from [`ComponentId`], never a package or a command.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{ApiError, ErrorBody};
use crate::live_view::Actor;

/// A component Praxi can install (fixed list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentId {
    /// uv, the Python tool runner (`uvx`).
    Uv,
    /// Node.js (`npx`).
    Node,
}

impl ComponentId {
    /// Parse a path segment.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "uv" => Some(Self::Uv),
            "node" => Some(Self::Node),
            _ => None,
        }
    }
}

/// Progress of an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentInstallState {
    /// Nothing running.
    Idle,
    /// The installer is running.
    Installing,
    /// Finished and the component was found.
    Done,
    /// Failed; see `error`.
    Failed,
}

/// One component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComponentDto {
    /// Id.
    pub id: ComponentId,
    /// The launcher is found (checked now, without restarting the app).
    pub installed: bool,
    /// The app can run an official installer here.
    pub can_install: bool,
    /// The exact command that would run (display only; empty when not possible).
    pub command: String,
    /// Where it comes from (shown in the confirm dialog).
    pub source: String,
    /// What to run by hand when `can_install` is false.
    pub manual_command: String,
    /// Install progress.
    pub state: ComponentInstallState,
    /// Why the last install failed: `exit`, `not_detected`, `timeout` or `unavailable`.
    pub error: Option<String>,
}

/// Why a [`ComponentControl`] call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentError {
    /// Unknown component (`404`).
    NotFound,
    /// Cannot be installed here, or is already installed or running (`409`).
    Conflict(String),
}

/// What `/v1/components/*` needs from the embedder.
#[async_trait]
pub trait ComponentControl: Send + Sync {
    /// Every component.
    fn list(&self) -> Vec<ComponentDto>;
    /// One component, re-detected now.
    fn get(&self, id: ComponentId) -> ComponentDto;
    /// Start the installer in the background.
    ///
    /// # Errors
    /// [`ComponentError`].
    async fn install(&self, id: ComponentId) -> Result<ComponentDto, ComponentError>;
}

impl From<ComponentError> for ApiError {
    fn from(e: ComponentError) -> Self {
        match e {
            ComponentError::NotFound => Self::not_found("unknown component"),
            ComponentError::Conflict(m) => {
                Self::new(axum::http::StatusCode::CONFLICT, "component_conflict", m)
            }
        }
    }
}

fn control(s: &AppState) -> Result<Arc<dyn ComponentControl>, ApiError> {
    s.components()
        .ok_or_else(|| ApiError::unavailable("components are not available in this daemon"))
}

fn parse(id: &str) -> Result<ComponentId, ApiError> {
    ComponentId::parse(id).ok_or_else(|| ApiError::not_found("unknown component"))
}

/// Components local apps may need and whether each is installed.
#[utoipa::path(get, path = "/v1/components", tag = "components", security(("bearer" = [])),
    responses((status = 200, body = Vec<ComponentDto>)))]
pub(crate) async fn list_components(
    State(s): State<AppState>,
) -> Result<Json<Vec<ComponentDto>>, ApiError> {
    Ok(Json(control(&s)?.list()))
}

/// One component (poll this while an install runs).
#[utoipa::path(get, path = "/v1/components/{id}", tag = "components", security(("bearer" = [])),
    params(("id" = String, Path, description = "`uv` or `node`")),
    responses((status = 200, body = ComponentDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn get_component(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ComponentDto>, ApiError> {
    Ok(Json(control(&s)?.get(parse(&id)?)))
}

/// Run the official installer for a component (the client confirmed the
/// command first).  Returns at once; poll `GET /v1/components/{id}`.
#[utoipa::path(post, path = "/v1/components/{id}/install", tag = "components",
    security(("bearer" = [])),
    params(("id" = String, Path, description = "`uv` or `node`")),
    responses((status = 200, body = ComponentDto), (status = 404, body = ErrorBody),
              (status = 409, body = ErrorBody)))]
pub(crate) async fn install_component(
    State(s): State<AppState>,
    axum::Extension(actor): axum::Extension<Actor>,
    Path(id): Path<String>,
) -> Result<Json<ComponentDto>, ApiError> {
    let id = parse(&id)?;
    let c = control(&s)?;
    s.storage()
        .append_audit(
            s.user_id(),
            None,
            &actor.key,
            "components.install_started",
            &json!({"component": id}),
        )
        .map_err(|_| ApiError::unavailable("audit log unavailable; nothing was changed"))?;
    Ok(Json(c.install(id).await?))
}
