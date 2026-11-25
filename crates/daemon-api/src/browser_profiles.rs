//! Owner-only browser session housekeeping. Never expose profile paths or tokens.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use builtin_tools::browser::BrowserProfiles;
use protocol::BotId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

#[derive(Serialize, ToSchema)]
pub struct BrowserProfileStatus {
    /// Sites with saved cookies. A login may have expired; no cookie values returned.
    pub domains: Vec<String>,
    /// False when the browser has locked its cookie database or site names cannot be read.
    pub sites_available: bool,
    pub size_bytes: u64,
    pub signing_in: bool,
    /// True only when filesystem directory encryption can be established.
    pub directory_encrypted: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgetBrowserSite {
    pub domain: String,
}

fn manager(s: &AppState, bot: Uuid) -> Result<Arc<BrowserProfiles>, ApiError> {
    let stored = s.bot_of_user(BotId(bot))?;
    if stored
        .config
        .pointer("/runtime/computer")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|target| target.starts_with("server:"))
    {
        return Err(failure(
            "this bot uses a paired server's browser; manage its sign-in on that server".into(),
        ));
    }
    s.agent()
        .browser_profiles()
        .ok_or_else(|| failure("the managed browser is not enabled on this daemon".into()))
}

fn failure(message: String) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "browser_profile_error", message)
}

#[utoipa::path(get, path = "/v1/bots/{bot}/browser-profile", tag = "bots",
    params(("bot" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 200, body = BrowserProfileStatus), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn get(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<Json<BrowserProfileStatus>, ApiError> {
    let status = manager(&s, bot)?
        .status(&bot.to_string())
        .await
        .map_err(failure)?;
    Ok(Json(BrowserProfileStatus {
        domains: status.domains,
        sites_available: status.sites_available,
        size_bytes: status.size_bytes,
        signing_in: status.signing_in,
        directory_encrypted: status.directory_encrypted,
    }))
}

#[utoipa::path(post, path = "/v1/bots/{bot}/browser-profile/sign-in", tag = "bots",
    params(("bot" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)))]
pub(crate) async fn sign_in(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    manager(&s, bot)?
        .sign_in(&bot.to_string())
        .await
        .map_err(failure)?;
    audit(&s, bot, "browser_profile.sign_in")?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/v1/bots/{bot}/browser-profile/finish-sign-in", tag = "bots",
    params(("bot" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)))]
pub(crate) async fn finish(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    manager(&s, bot)?
        .finish_sign_in(&bot.to_string())
        .await
        .map_err(failure)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/v1/bots/{bot}/browser-profile/forget-site", tag = "bots",
    params(("bot" = Uuid, Path)), security(("bearer" = [])), request_body = ForgetBrowserSite,
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)))]
pub(crate) async fn forget_site(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
    ApiJson(input): ApiJson<ForgetBrowserSite>,
) -> Result<StatusCode, ApiError> {
    manager(&s, bot)?
        .forget(&bot.to_string(), Some(&input.domain))
        .await
        .map_err(failure)?;
    audit(&s, bot, "browser_profile.forget_site")?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/v1/bots/{bot}/browser-profile", tag = "bots",
    params(("bot" = Uuid, Path)), security(("bearer" = [])),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)))]
pub(crate) async fn forget_all(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    manager(&s, bot)?
        .forget(&bot.to_string(), None)
        .await
        .map_err(failure)?;
    audit(&s, bot, "browser_profile.forget_all")?;
    Ok(StatusCode::NO_CONTENT)
}

fn audit(s: &AppState, bot: Uuid, action: &str) -> Result<(), ApiError> {
    s.storage()
        .append_audit(
            s.user_id(),
            None,
            "user",
            action,
            &serde_json::json!({"bot_id": bot}),
        )
        .map_err(ApiError::internal)?;
    Ok(())
}

pub(crate) fn is_profile_path(path: &str) -> bool {
    path.strip_prefix("/v1/bots/")
        .and_then(|s| s.split_once('/'))
        .is_some_and(|(_, rest)| rest == "browser-profile" || rest.starts_with("browser-profile/"))
}
