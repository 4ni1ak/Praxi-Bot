//! A bot's page-reader setting (`/v1/bots/{id}/page-reader`, stored as `browser.reader`).
//!
//! `off`, `auto` (the default) or `always`: whether web pages, mails and other outside text
//! reach the bot only as the result of a separate, tool-less reader model (see
//! `agent_core::page_guard`). Changing it is owner-only (`owner.rs`) and peers cannot change it
//! (`peer.rs`); every change is written to the audit log.

use agent_core::page_guard::ReaderMode;
use axum::http::HeaderMap;
use axum::{
    Json,
    extract::{Path as ApiPath, State},
};
use protocol::BotId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

/// How outside text reaches a bot.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PageReaderDto {
    /// `off`: the bot reads pages itself (inside a data block). `auto`: through the reader
    /// while the run holds private data (files, mail, memory). `always`: every page.
    pub mode: String,
}

/// `PUT /v1/bots/{id}/page-reader`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PutPageReaderInput {
    /// `off`, `auto` or `always`.
    pub mode: String,
}

/// A bot's page-reader setting.
#[utoipa::path(get, path = "/v1/bots/{id}/page-reader", tag = "bots", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    responses((status = 200, body = PageReaderDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn get_page_reader(
    State(s): State<AppState>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<PageReaderDto>, ApiError> {
    let bot = s.bot_of_user(BotId(id))?;
    Ok(Json(PageReaderDto {
        mode: ReaderMode::from_bot_config(&bot.config).as_str().to_owned(),
    }))
}

/// Change a bot's page-reader setting (owner only). Effective from the bot's next tool result.
#[utoipa::path(put, path = "/v1/bots/{id}/page-reader", tag = "bots", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = PutPageReaderInput,
    responses((status = 200, body = PageReaderDto), (status = 400, body = ErrorBody),
              (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn put_page_reader(
    State(s): State<AppState>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<PutPageReaderInput>,
) -> Result<Json<PageReaderDto>, ApiError> {
    // Defence in depth next to `OWNER_ROUTES` and the peer denial.
    s.require_owner(&headers)?;
    let bot = s.bot_of_user(BotId(id))?;
    let mode = ReaderMode::parse(&input.mode)
        .ok_or_else(|| ApiError::bad_request("mode must be off, auto or always"))?;
    let (row, version) = s
        .storage()
        .get_bot_versioned(bot.id)?
        .ok_or_else(|| ApiError::not_found("bot"))?;
    let before = ReaderMode::from_bot_config(&row.config);
    let mut config = row.config.clone();
    let obj = config
        .as_object_mut()
        .ok_or_else(|| ApiError::bad_request("bot config must be an object"))?;
    let browser = obj
        .entry("browser")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| ApiError::conflict("the bot's browser settings are not an object"))?;
    browser.insert("reader".into(), json!(mode.as_str()));
    s.storage().append_audit(
        s.user_id(),
        None,
        "user",
        "browser.reader",
        &json!({
            "bot_id": bot.id.0,
            "before": before.as_str(),
            "after": mode.as_str(),
        }),
    )?;
    s.storage()
        .update_bot(bot.id, &row.name, &config, version)
        .map_err(|_| ApiError::conflict("the bot changed meanwhile; try again"))?;
    s.agent().forget_profile(bot.id);
    Ok(Json(PageReaderDto {
        mode: mode.as_str().to_owned(),
    }))
}
