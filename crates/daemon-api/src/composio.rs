//! Composio app connections (`/v1/composio/*`): DTOs and the
//! [`ComposioControl`] trait the embedder implements (see
//! `apps/cli/src/composio_host.rs`).  The daemon holds no Composio state.
//!
//! Every route is owner-only: the local bearer token, or a paired Desktop
//! peer.  Phone, node and host peers are refused for every method.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Connection state of an app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComposioConnState {
    /// Nothing connected.
    NotConnected,
    /// Sign-in is not finished.
    Pending,
    /// Connected.
    Active,
    /// The authorization ran out; reconnect.
    Expired,
}

/// `GET /v1/composio`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioStatusDto {
    /// A key is stored (the key itself is never returned).
    pub configured: bool,
    /// Where the owner creates the key (https, on Composio's domain).
    pub key_setup_url: String,
}

/// Body of `PUT /v1/composio/key`.  Write-only.
#[derive(Deserialize, ToSchema)]
pub struct ComposioKeyInput {
    /// The Composio project API key.
    pub value: String,
}

impl std::fmt::Debug for ComposioKeyInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComposioKeyInput(<redacted>)")
    }
}

/// One toolkit of the Composio catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioToolkitDto {
    /// Toolkit slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Short description (third-party text).
    pub description: String,
    /// Category names.
    pub categories: Vec<String>,
    /// Number of actions, when Composio reports it.
    pub tools_count: Option<u64>,
    /// Praxi connects it directly (always `false` until a zero-setup direct
    /// path exists); the UI notes it but never blocks the choice.
    pub native_first: bool,
    /// State for this installation.
    pub state: ComposioConnState,
    /// The owner enabled it.
    pub enabled: bool,
}

/// `GET /v1/composio/toolkits`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioCatalogDto {
    /// Pinned popular row (first page without filters only; empty otherwise).
    pub popular: Vec<ComposioToolkitDto>,
    /// The page, in Composio's popularity order.
    pub items: Vec<ComposioToolkitDto>,
    /// Cursor of the next page.
    pub next_cursor: Option<String>,
}

/// Query of `GET /v1/composio/toolkits`.
#[derive(Debug, Clone, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ComposioCatalogQuery {
    /// Text matched against name, slug and description.
    pub search: Option<String>,
    /// Category.
    pub category: Option<String>,
    /// Cursor from the previous page.
    pub cursor: Option<String>,
    /// Page size, 1 to 100 (default 48).
    pub limit: Option<u32>,
}

/// A connected (or connecting) app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioAppDto {
    /// Toolkit slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// State.
    pub state: ComposioConnState,
    /// The owner enabled it (tools exist only while enabled and active).
    pub enabled: bool,
    /// Praxi connects it directly.
    pub native_first: bool,
    /// Bots allowed; `null` = all.
    pub bots: Option<Vec<String>>,
    /// Direct tools currently offered to the model.
    pub tools: u32,
}

/// `POST /v1/composio/apps/{slug}/connect`: the one-time consent link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioConnectDto {
    /// https URL on Composio's domain; open it in the system browser.
    pub redirect_url: String,
    /// When the link expires (ISO timestamp, may be empty).
    pub expires_at: String,
}

/// Body of `PUT /v1/composio/apps/{slug}/enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioEnabledInput {
    /// New value.
    pub enabled: bool,
}

/// `DELETE /v1/composio/apps/{slug}` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposioDisconnectDto {
    /// The account was deleted on Composio's side too.  When `false` the app
    /// is forgotten locally and the owner can remove it in Composio's dashboard.
    pub remote_removed: bool,
}

/// Why a [`ComposioControl`] operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposioControlError {
    /// Unknown or not connected app (`404`).
    NotFound(String),
    /// Invalid input (`422`).
    Invalid(String),
    /// No key yet (`409`).
    NotConfigured,
    /// Composio rate limit (`429`).
    RateLimited(String),
    /// Composio failed or is unreachable (`502`).
    Upstream(String),
    /// Local failure (`500`).
    Internal(String),
}

/// What `/v1/composio/*` needs from the embedder.
#[async_trait]
pub trait ComposioControl: Send + Sync {
    /// Key status.
    fn status(&self) -> ComposioStatusDto;
    /// Validate and store the key.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn set_key(&self, key: &str) -> Result<(), ComposioControlError>;
    /// Delete the key.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn remove_key(&self) -> Result<(), ComposioControlError>;
    /// One catalog page.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn catalog(
        &self,
        query: ComposioCatalogQuery,
    ) -> Result<ComposioCatalogDto, ComposioControlError>;
    /// Apps the owner connected or started to connect.
    fn apps(&self) -> Vec<ComposioAppDto>;
    /// Refresh and return one app.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn app(&self, slug: &str) -> Result<ComposioAppDto, ComposioControlError>;
    /// Start connecting an app.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn connect(&self, slug: &str) -> Result<ComposioConnectDto, ComposioControlError>;
    /// Switch an app on or off.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn set_enabled(
        &self,
        slug: &str,
        enabled: bool,
    ) -> Result<ComposioAppDto, ComposioControlError>;
    /// Disconnect an app.
    ///
    /// # Errors
    /// [`ComposioControlError`].
    async fn disconnect(&self, slug: &str) -> Result<ComposioDisconnectDto, ComposioControlError>;
}
