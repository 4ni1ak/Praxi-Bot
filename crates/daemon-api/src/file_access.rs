//! Per-bot file access mode (`[files]` in the bot profile).
//!
//! `computer` (default): the built-in file tools reach anywhere the user can,
//! minus protected locations. `folder`: only the owner-chosen folders. Only the
//! local owner changes it: paired peers are refused on every route that could.

use axum::{
    Json,
    extract::{Path, State},
};
use protocol::BotId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

/// A bot's file access mode.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileAccessDto {
    /// `computer` (This computer, default) or `folder` (Folder only).
    pub mode: String,
    /// Absolute folders for `folder` mode.
    #[serde(default)]
    pub folders: Vec<String>,
}

/// The effective setting. A bot that never chose one is on "This computer",
/// except on an always-on host where it has no file access until the host's
/// owner chooses (shown as an empty "Folder only").
fn current(bot: &storage::Bot, host: bool) -> FileAccessDto {
    let files = bot.config.get("files");
    let unset = if host { "folder" } else { "computer" };
    FileAccessDto {
        mode: files
            .and_then(|f| f.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or(unset)
            .to_owned(),
        folders: files
            .and_then(|f| f.get("folders"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Validate and canonicalise an access setting.
fn normalize(s: &AppState, input: &FileAccessDto) -> Result<Value, ApiError> {
    match input.mode.as_str() {
        "computer" => Ok(json!({"mode": "computer"})),
        "folder" => {
            if input.folders.is_empty() {
                return Err(ApiError::bad_request("choose at least one folder"));
            }
            let data_dir = builtin_tools::files::resolve_local_folder(&s.data_dir())
                .unwrap_or_else(|_| s.data_dir());
            let mut folders = Vec::new();
            for raw in &input.folders {
                let folder = builtin_tools::files::resolve_local_folder(std::path::Path::new(raw))
                    .map_err(|e| ApiError::bad_request(format!("folder {raw}: {e}")))?;
                if builtin_tools::files::is_protected_root(&folder, &data_dir) {
                    return Err(ApiError::bad_request(format!(
                        "folder {raw} holds Praxi's own data or credentials"
                    )));
                }
                let text = folder.to_string_lossy().into_owned();
                if !folders.contains(&text) {
                    folders.push(text);
                }
            }
            Ok(json!({"mode": "folder", "folders": folders}))
        }
        other => Err(ApiError::bad_request(format!(
            "unknown file access mode {other:?}"
        ))),
    }
}

/// Generic profile editors keep ordinary editing but cannot change file access
/// as a peer; an omitted `files` keeps the stored value so a stale snapshot
/// cannot silently widen or narrow it. The owner may include `files`, which is
/// validated like the dedicated route.
pub(crate) fn guard_files_config(
    s: &AppState,
    peer: Option<&crate::PeerIdentity>,
    current: Option<&Value>,
    next: &mut Value,
) -> Result<(), ApiError> {
    let obj = next
        .as_object_mut()
        .ok_or_else(|| ApiError::bad_request("bot config must be an object"))?;
    match obj.get("files").cloned() {
        None => {
            if let Some(c) = current {
                obj.insert("files".into(), c.clone());
            }
        }
        Some(incoming) if Some(&incoming) == current => {}
        Some(_) if peer.is_some() => {
            return Err(ApiError::forbidden(
                "file access is managed by the local owner",
            ));
        }
        Some(incoming) => {
            let dto: FileAccessDto = serde_json::from_value(incoming)
                .map_err(|e| ApiError::bad_request(format!("files: {e}")))?;
            obj.insert("files".into(), normalize(s, &dto)?);
        }
    }
    Ok(())
}

/// The bot's file access mode.
#[utoipa::path(get, path = "/v1/bots/{bot}/files", tag = "bots", security(("bearer" = [])),
    params(("bot" = Uuid, Path, description = "Bot id")),
    responses((status = 200, body = FileAccessDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn get_files(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<Json<FileAccessDto>, ApiError> {
    let host = s.leader().config().mode == crate::host::HostMode::Host;
    Ok(Json(current(&s.bot_of_user(BotId(bot))?, host)))
}

/// Set the bot's file access mode. Local owner only.
#[utoipa::path(put, path = "/v1/bots/{bot}/files", tag = "bots", security(("bearer" = [])),
    params(("bot" = Uuid, Path, description = "Bot id")),
    request_body = FileAccessDto,
    responses((status = 200, body = FileAccessDto), (status = 400, body = ErrorBody),
              (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(crate) async fn put_files(
    State(s): State<AppState>,
    peer: Option<axum::Extension<crate::PeerIdentity>>,
    Path(bot): Path<Uuid>,
    ApiJson(input): ApiJson<FileAccessDto>,
) -> Result<Json<FileAccessDto>, ApiError> {
    if s.bot_authority_peer(peer.as_ref().map(|p| &p.0)).is_some() {
        return Err(ApiError::forbidden(
            "file access is managed by the local owner",
        ));
    }
    let mut bot = s.bot_of_user(BotId(bot))?;
    let value = normalize(&s, &input)?;
    let object = bot
        .config
        .as_object_mut()
        .ok_or_else(|| ApiError::bad_request("bot config must be an object"))?;
    object.insert("files".into(), value);
    let (_, version) = s
        .storage()
        .get_bot_versioned(bot.id)?
        .ok_or_else(|| ApiError::not_found("bot"))?;
    match s
        .storage()
        .update_bot(bot.id, &bot.name, &bot.config, version)?
    {
        storage::BotPatch::Applied { bot: stored, .. } => {
            s.refresh_profile(&stored);
            bot = stored;
        }
        storage::BotPatch::Stale { .. } => {
            return Err(ApiError::conflict(
                "the bot changed while file access was being saved; retry",
            ));
        }
        _ => return Err(ApiError::not_found("bot")),
    }
    let host = s.leader().config().mode == crate::host::HostMode::Host;
    Ok(Json(current(&bot, host)))
}
