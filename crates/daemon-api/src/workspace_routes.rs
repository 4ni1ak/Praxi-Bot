//! Per-bot persistent workspace browsing, disk usage, thresholds, and cleanup routes.
//!
//! Every route here belongs in `OWNER_ROUTES` in [`crate::owner`] and enforces
//! strict path containment inside the target bot's own workspace directory
//! (no `..`, no absolute paths, no symlinks or Windows reparse points).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use protocol::BotId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use storage::{Bot, BotWorkspaceRecord, WorkspaceSettingsRecord, now_ms};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

const TRASH_DIR_NAME: &str = "workspace-trash";
const UNDO_TTL_MS: i64 = 10 * 60 * 1000;
const MAX_SEARCH_RESULTS: usize = 500;
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024 * 1024; // 10 TiB
const MAX_AGE_DAYS: u32 = 3650;
const DEFAULT_BIG_LIMIT: usize = 5;

/// Host filesystem usage statistics for the workspaces disk.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiskStatsDto {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: u8,
}

/// Editable disk-threshold and cleanup-age settings (`PUT /v1/workspaces/settings`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceSettingsDto {
    pub warn_percent_1: u8,
    pub warn_percent_2: u8,
    pub warn_percent_3: u8,
    pub min_free_bytes: u64,
    pub downloads_max_age_days: Option<u32>,
    pub logs_max_age_days: Option<u32>,
}

/// Summary of one bot's persistent workspace directory.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BotWorkspaceSummaryDto {
    pub bot_id: Uuid,
    pub bot_name: String,
    pub folder_name: String,
    pub workspace_path: Option<String>,
    pub exists: bool,
    pub used_bytes: u64,
    pub file_count: u64,
    pub soft_limit_bytes: Option<u64>,
    pub hard_limit_bytes: Option<u64>,
    pub soft_limit_exceeded: bool,
    pub hard_limit_exceeded: bool,
}

/// Largest item entry surfaced inside warnings and overview suggestions.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceBigItemDto {
    pub bot_id: Option<Uuid>,
    pub bot_name: Option<String>,
    pub folder_name: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub modified_at_ms: i64,
    pub category: String,
}

/// Allowed safe cleanup targets (`POST /v1/workspaces/cleanup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CleanupTarget {
    OldDownloads,
    OldLogs,
    AllOld,
    DaemonCache,
}

/// One-click cleanup suggestion showing reclaimable space.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CleanupSuggestionDto {
    pub target: CleanupTarget,
    pub reclaimable_bytes: u64,
    pub file_count: u64,
}

/// Actionable disk/limit warning inside the owner-gated `GET /v1/workspaces`
/// (the SSE `workspace_warning` event is the name-free [`WorkspaceWarningEventDto`]).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceWarningDto {
    pub severity: String,
    pub threshold_percent: Option<u8>,
    pub below_min_free: bool,
    pub disk_used_percent: Option<u8>,
    pub available_bytes: Option<u64>,
    pub min_free_bytes: u64,
    pub soft_limit_bots: Vec<Uuid>,
    pub largest_items: Vec<WorkspaceBigItemDto>,
    pub suggestions: Vec<CleanupSuggestionDto>,
}

/// Response of `GET /v1/workspaces`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspacesOverviewDto {
    pub configured: bool,
    pub root_path: Option<String>,
    pub disk: Option<DiskStatsDto>,
    pub total_workspace_bytes: u64,
    pub settings: WorkspaceSettingsDto,
    pub bots: Vec<BotWorkspaceSummaryDto>,
    pub largest_items: Vec<WorkspaceBigItemDto>,
    pub suggestions: Vec<CleanupSuggestionDto>,
    pub active_warning: Option<WorkspaceWarningDto>,
}

/// Request body of `PUT /v1/workspaces/settings`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct WorkspaceSettingsInput {
    pub warn_percent_1: u8,
    pub warn_percent_2: u8,
    pub warn_percent_3: u8,
    pub min_free_bytes: u64,
    pub downloads_max_age_days: Option<u32>,
    pub logs_max_age_days: Option<u32>,
}

/// Request body of `POST /v1/workspaces/cleanup`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct WorkspaceCleanupInput {
    pub target: CleanupTarget,
    #[serde(default)]
    pub bot_id: Option<Uuid>,
}

/// Response of `POST /v1/workspaces/cleanup`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceCleanupResultDto {
    pub target: CleanupTarget,
    pub deleted_files: u64,
    pub freed_bytes: u64,
}

/// Kind of entry in a bot's workspace file browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceEntryKind {
    Directory,
    File,
}

/// Preview kind supported safely by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePreviewKind {
    None,
    Text,
    Image,
}

/// One file or directory inside a bot's workspace.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceEntryDto {
    pub name: String,
    pub path: String,
    pub kind: WorkspaceEntryKind,
    pub size_bytes: u64,
    pub modified_at_ms: i64,
    pub preview_kind: WorkspacePreviewKind,
}

/// Deleted entry held briefly in trash for Undo (`POST /v1/bots/{id}/workspace/restore`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceTrashEntryDto {
    pub undo_id: String,
    pub name: String,
    pub original_path: String,
    pub size_bytes: u64,
    pub deleted_at_ms: i64,
}

/// Response of `GET /v1/bots/{id}/workspace`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BotWorkspaceListDto {
    pub bot_id: Uuid,
    pub bot_name: String,
    pub folder_name: String,
    pub workspace_path: Option<String>,
    pub current_path: String,
    pub used_bytes: u64,
    pub soft_limit_bytes: Option<u64>,
    pub hard_limit_bytes: Option<u64>,
    pub soft_limit_exceeded: bool,
    pub hard_limit_exceeded: bool,
    pub entries: Vec<WorkspaceEntryDto>,
    pub undo_items: Vec<WorkspaceTrashEntryDto>,
}

/// Query params for `GET /v1/bots/{id}/workspace`.
#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceListQuery {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
}

/// Query params for `GET /v1/bots/{id}/workspace/file`.
#[derive(Debug, Deserialize)]
pub struct WorkspaceFileQuery {
    pub path: String,
    #[serde(default)]
    pub download: bool,
}

/// Request body of `POST /v1/bots/{id}/workspace/rename`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkspaceRenameInput {
    pub path: String,
    pub new_name: String,
}

/// Request body of `POST /v1/bots/{id}/workspace/delete`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkspaceDeleteInput {
    pub path: String,
}

/// Request body of `POST /v1/bots/{id}/workspace/restore`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WorkspaceRestoreInput {
    pub undo_id: String,
}

/// Request body of `PUT /v1/bots/{id}/workspace/limits`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BotWorkspaceLimitsInput {
    pub soft_limit_bytes: Option<u64>,
    pub hard_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrashMeta {
    undo_id: String,
    name: String,
    original_path: String,
    size_bytes: u64,
    deleted_at_ms: i64,
}

impl From<WorkspaceSettingsRecord> for WorkspaceSettingsDto {
    fn from(r: WorkspaceSettingsRecord) -> Self {
        Self {
            warn_percent_1: r.warn_percent_1,
            warn_percent_2: r.warn_percent_2,
            warn_percent_3: r.warn_percent_3,
            min_free_bytes: r.min_free_bytes,
            downloads_max_age_days: r.downloads_max_age_days,
            logs_max_age_days: r.logs_max_age_days,
        }
    }
}

fn ensure_bot_record(state: &AppState, bot: &Bot) -> Result<BotWorkspaceRecord, ApiError> {
    let preferred =
        builtin_tools::workspace::sanitize_bot_folder_name(&bot.name, &bot.id.to_string());
    state
        .storage()
        .ensure_bot_workspace(state.user_id(), bot.id, &preferred)
        .map_err(Into::into)
}

fn ensure_bot_dir_if_configured(
    state: &AppState,
    bot: &Bot,
) -> Result<(BotWorkspaceRecord, Option<PathBuf>), ApiError> {
    let rec = ensure_bot_record(state, bot)?;
    let Some(root) = state.workspaces_root() else {
        return Ok((rec, None));
    };
    let dir = builtin_tools::workspace::ensure_bot_workspace_dir(
        &root,
        &rec.folder_name,
        &state.data_dir(),
    )
    .map_err(ApiError::bad_request)?;
    Ok((rec, Some(dir)))
}

fn system_time_to_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn file_extension_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn preview_kind_for(path: &Path, is_dir: bool) -> WorkspacePreviewKind {
    if is_dir {
        return WorkspacePreviewKind::None;
    }
    let ext = file_extension_lower(path);
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => WorkspacePreviewKind::Image,
        // Note: svg/html/xml are intentionally previewed as plain text, never rendered inline.
        "txt" | "md" | "markdown" | "json" | "csv" | "tsv" | "log" | "toml" | "yaml" | "yml"
        | "ini" | "cfg" | "conf" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "css" | "sql"
        | "sh" | "ps1" | "svg" | "html" | "htm" | "xml" => WorkspacePreviewKind::Text,
        _ => WorkspacePreviewKind::None,
    }
}

fn image_mime_for(path: &Path) -> Option<&'static str> {
    match file_extension_lower(path).as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn normalize_rel_segments(raw: &str) -> Result<Vec<String>, ApiError> {
    let trimmed = raw.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('/') || trimmed.contains('\0') {
        return Err(ApiError::bad_request("invalid workspace relative path"));
    }
    let mut parts = Vec::new();
    for seg in trimmed.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains(':') {
            return Err(ApiError::bad_request("parent traversal is not allowed"));
        }
        parts.push(seg.to_owned());
    }
    Ok(parts)
}

fn resolve_existing_in_workspace(
    ws_root: &Path,
    segments: &[String],
    allow_root: bool,
) -> Result<PathBuf, ApiError> {
    if segments.is_empty() {
        if !allow_root {
            return Err(ApiError::bad_request("cannot modify the workspace root"));
        }
        let meta = fs::symlink_metadata(ws_root)
            .map_err(|e| ApiError::not_found(format!("workspace: {e}")))?;
        if meta.file_type().is_symlink()
            || crate::data_routes::is_reparse_point(&meta)
            || !meta.is_dir()
        {
            return Err(ApiError::forbidden(
                "workspace root is not a plain directory",
            ));
        }
        return Ok(ws_root.to_path_buf());
    }

    let root_canon =
        fs::canonicalize(ws_root).map_err(|e| ApiError::not_found(format!("workspace: {e}")))?;
    let mut cur = ws_root.to_path_buf();
    for seg in segments {
        cur.push(seg);
        let meta = fs::symlink_metadata(&cur)
            .map_err(|_| ApiError::not_found(format!("path '{}'", segments.join("/"))))?;
        if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
            return Err(ApiError::forbidden(
                "symlinks and reparse points are not accessible",
            ));
        }
    }
    let cur_canon = fs::canonicalize(&cur)
        .map_err(|_| ApiError::not_found(format!("path '{}'", segments.join("/"))))?;
    if !cur_canon.starts_with(&root_canon) || (!allow_root && cur_canon == root_canon) {
        return Err(ApiError::forbidden("path escapes the bot workspace"));
    }
    Ok(cur)
}

fn is_valid_entry_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 || name == "." || name == ".." {
        return false;
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

fn entry_dto_from_path(abs: &Path, rel_path: String) -> Option<WorkspaceEntryDto> {
    let meta = fs::symlink_metadata(abs).ok()?;
    if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
        return None;
    }
    let is_dir = meta.is_dir();
    if !is_dir && !meta.is_file() {
        return None;
    }
    let name = abs
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let modified_at_ms = meta.modified().map_or(0, system_time_to_ms);
    Some(WorkspaceEntryDto {
        name,
        path: rel_path,
        kind: if is_dir {
            WorkspaceEntryKind::Directory
        } else {
            WorkspaceEntryKind::File
        },
        size_bytes: if is_dir { 0 } else { meta.len() },
        modified_at_ms,
        preview_kind: preview_kind_for(abs, is_dir),
    })
}

fn collect_search_entries(
    dir: &Path,
    prefix: &str,
    query_lower: &str,
    out: &mut Vec<WorkspaceEntryDto>,
) {
    if out.len() >= MAX_SEARCH_RESULTS {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = rd.filter_map(Result::ok).collect();
    children.sort_by_key(fs::DirEntry::file_name);
    for entry in children {
        if out.len() >= MAX_SEARCH_RESULTS {
            break;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = meta.is_dir();
        if name.to_ascii_lowercase().contains(query_lower)
            && let Some(dto) = entry_dto_from_path(&entry.path(), rel.clone())
        {
            out.push(dto);
        }
        if is_dir {
            collect_search_entries(&entry.path(), &rel, query_lower, out);
        }
    }
}

fn scan_dir_stats_and_items(
    bot: Option<(&Bot, &BotWorkspaceRecord)>,
    ws_dir: &Path,
    settings: &WorkspaceSettingsRecord,
    big_items: &mut Vec<WorkspaceBigItemDto>,
    old_downloads: &mut (u64, u64),
    old_logs: &mut (u64, u64),
) -> (u64, u64) {
    let mut total_bytes = 0u64;
    let mut file_count = 0u64;
    let now = SystemTime::now();
    let dl_cutoff = settings
        .downloads_max_age_days
        .map(|d| Duration::from_secs(u64::from(d) * 86_400));
    let log_cutoff = settings
        .logs_max_age_days
        .map(|d| Duration::from_secs(u64::from(d) * 86_400));

    let folder_name = bot
        .map(|(_, r)| r.folder_name.clone())
        .or_else(|| {
            ws_dir
                .file_name()
                .and_then(|s| s.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();

    let mut stack = vec![(ws_dir.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if meta.is_dir() {
                stack.push((path, rel));
            } else if meta.is_file() {
                let sz = meta.len();
                total_bytes = total_bytes.saturating_add(sz);
                file_count = file_count.saturating_add(1);
                let category = if rel == "downloads" || rel.starts_with("downloads/") {
                    "downloads"
                } else if rel == "logs" || rel.starts_with("logs/") {
                    "logs"
                } else {
                    "workspace"
                };
                if let Ok(mod_time) = meta.modified()
                    && let Ok(age) = now.duration_since(mod_time)
                {
                    if category == "downloads"
                        && let Some(cutoff) = dl_cutoff
                        && age >= cutoff
                    {
                        old_downloads.0 = old_downloads.0.saturating_add(sz);
                        old_downloads.1 = old_downloads.1.saturating_add(1);
                    }
                    if category == "logs"
                        && let Some(cutoff) = log_cutoff
                        && age >= cutoff
                    {
                        old_logs.0 = old_logs.0.saturating_add(sz);
                        old_logs.1 = old_logs.1.saturating_add(1);
                    }
                }
                let modified_at_ms = meta.modified().map_or(0, system_time_to_ms);
                if sz > 0 {
                    big_items.push(WorkspaceBigItemDto {
                        bot_id: bot.map(|(b, _)| b.id.0),
                        bot_name: bot.map(|(b, _)| b.name.clone()),
                        folder_name: folder_name.clone(),
                        relative_path: rel,
                        size_bytes: sz,
                        modified_at_ms,
                        category: category.to_owned(),
                    });
                }
            }
        }
    }
    (total_bytes, file_count)
}

fn cache_reclaimable_bytes(data_dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut add_tree = |p: &Path, min_age: Option<Duration>| {
        let Ok(meta) = fs::symlink_metadata(p) else {
            return;
        };
        if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
            return;
        }
        let now = SystemTime::now();
        let mut stack = vec![p.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(dir) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let Ok(m) = fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.file_type().is_symlink() || crate::data_routes::is_reparse_point(&m) {
                    continue;
                }
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.is_file() {
                    let ok = match min_age {
                        Some(cutoff) => m
                            .modified()
                            .ok()
                            .and_then(|t| now.duration_since(t).ok())
                            .is_some_and(|a| a >= cutoff),
                        None => true,
                    };
                    if ok {
                        bytes = bytes.saturating_add(m.len());
                        files = files.saturating_add(1);
                    }
                }
            }
        }
    };
    add_tree(&data_dir.join("cache"), None);
    add_tree(&data_dir.join("router"), None);
    add_tree(&data_dir.join("spill"), Some(Duration::from_secs(86_400)));
    (bytes, files)
}

fn cleanup_old_subdir(root_sub: &Path, max_age_days: u32) -> (u64, u64) {
    let Ok(meta) = fs::symlink_metadata(root_sub) else {
        return (0, 0);
    };
    if meta.file_type().is_symlink()
        || crate::data_routes::is_reparse_point(&meta)
        || !meta.is_dir()
    {
        return (0, 0);
    }
    let cutoff = Duration::from_secs(u64::from(max_age_days) * 86_400);
    let now = SystemTime::now();
    cleanup_dir_recursive(root_sub, cutoff, now, false)
}

fn cleanup_dir_recursive(
    dir: &Path,
    cutoff: Duration,
    now: SystemTime,
    remove_if_empty: bool,
) -> (u64, u64) {
    let mut freed = 0u64;
    let mut deleted = 0u64;
    let Ok(rd) = fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in rd.filter_map(Result::ok) {
        let p = entry.path();
        let Ok(m) = fs::symlink_metadata(&p) else {
            continue;
        };
        if m.file_type().is_symlink() || crate::data_routes::is_reparse_point(&m) {
            continue;
        }
        if m.is_dir() {
            let (f, d) = cleanup_dir_recursive(&p, cutoff, now, true);
            freed = freed.saturating_add(f);
            deleted = deleted.saturating_add(d);
        } else if m.is_file() {
            let old = m
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= cutoff);
            if old && fs::remove_file(&p).is_ok() {
                freed = freed.saturating_add(m.len());
                deleted = deleted.saturating_add(1);
            }
        }
    }
    if remove_if_empty && fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none()) {
        let _ = fs::remove_dir(dir);
    }
    (freed, deleted)
}

fn trash_bot_dir(state: &AppState, bot_id: BotId) -> PathBuf {
    state
        .data_dir()
        .join(TRASH_DIR_NAME)
        .join(bot_id.to_string())
}

fn list_and_prune_trash(state: &AppState, bot_id: BotId) -> Vec<WorkspaceTrashEntryDto> {
    let base = trash_bot_dir(state, bot_id);
    let Ok(rd) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let cutoff = now_ms().saturating_sub(UNDO_TTL_MS);
    let mut items = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let item_dir = entry.path();
        let meta_path = item_dir.join("meta.json");
        let Ok(raw) = fs::read_to_string(&meta_path) else {
            let _ = fs::remove_dir_all(&item_dir);
            continue;
        };
        let Ok(meta) = serde_json::from_str::<TrashMeta>(&raw) else {
            let _ = fs::remove_dir_all(&item_dir);
            continue;
        };
        if meta.deleted_at_ms < cutoff || !item_dir.join("payload").exists() {
            let _ = fs::remove_dir_all(&item_dir);
            continue;
        }
        items.push(WorkspaceTrashEntryDto {
            undo_id: meta.undo_id,
            name: meta.name,
            original_path: meta.original_path,
            size_bytes: meta.size_bytes,
            deleted_at_ms: meta.deleted_at_ms,
        });
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.deleted_at_ms));
    items
}

fn move_entry(src: &Path, dst: &Path) -> std::io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Cross-device move (EXDEV on Unix when DATA_DIR and /var/lib/praxi-work sit on
            // different filesystems) falls back to symlink-safe recursive copy + remove.
            copy_plain_recursive(src, dst).map_err(|copy_err| {
                let _ = fs::remove_dir_all(dst);
                let _ = fs::remove_file(dst);
                if err.raw_os_error() == Some(18) {
                    copy_err
                } else {
                    err
                }
            })?;
            let meta = fs::symlink_metadata(src)?;
            if meta.is_dir() {
                fs::remove_dir_all(src)?;
            } else {
                fs::remove_file(src)?;
            }
            Ok(())
        }
    }
}

fn copy_plain_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() || crate::data_routes::is_reparse_point(&meta) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to copy symlink or reparse point",
        ));
    }
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_plain_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else if meta.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)?;
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported file type",
        ))
    }
}

fn ensure_plain_parents(ws_root: &Path, rel_segments: &[String]) -> Result<PathBuf, ApiError> {
    let root_canon = fs::canonicalize(ws_root)
        .map_err(|e| ApiError::internal(format!("workspace canonicalize: {e}")))?;
    let mut cur = ws_root.to_path_buf();
    for seg in &rel_segments[..rel_segments.len().saturating_sub(1)] {
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(m) => {
                if m.file_type().is_symlink()
                    || crate::data_routes::is_reparse_point(&m)
                    || !m.is_dir()
                {
                    return Err(ApiError::forbidden("parent path is not a plain directory"));
                }
            }
            Err(_) => {
                fs::create_dir(&cur).map_err(|e| ApiError::internal(format!("mkdir: {e}")))?;
            }
        }
        let canon = fs::canonicalize(&cur)
            .map_err(|e| ApiError::internal(format!("parent canonicalize: {e}")))?;
        if !canon.starts_with(&root_canon) {
            return Err(ApiError::forbidden("target parent escapes workspace"));
        }
    }
    let last = rel_segments
        .last()
        .ok_or_else(|| ApiError::bad_request("missing target name"))?;
    Ok(cur.join(last))
}

fn build_cleanup_suggestions(
    data_dir: &Path,
    old_downloads: (u64, u64),
    old_logs: (u64, u64),
) -> Vec<CleanupSuggestionDto> {
    let (cache_bytes, cache_files) = cache_reclaimable_bytes(data_dir);
    let mut suggestions = Vec::new();
    if old_downloads.1 > 0 {
        suggestions.push(CleanupSuggestionDto {
            target: CleanupTarget::OldDownloads,
            reclaimable_bytes: old_downloads.0,
            file_count: old_downloads.1,
        });
    }
    if old_logs.1 > 0 {
        suggestions.push(CleanupSuggestionDto {
            target: CleanupTarget::OldLogs,
            reclaimable_bytes: old_logs.0,
            file_count: old_logs.1,
        });
    }
    if old_downloads.1 + old_logs.1 > 0 {
        suggestions.push(CleanupSuggestionDto {
            target: CleanupTarget::AllOld,
            reclaimable_bytes: old_downloads.0.saturating_add(old_logs.0),
            file_count: old_downloads.1.saturating_add(old_logs.1),
        });
    }
    if cache_bytes > 0 {
        suggestions.push(CleanupSuggestionDto {
            target: CleanupTarget::DaemonCache,
            reclaimable_bytes: cache_bytes,
            file_count: cache_files,
        });
    }
    suggestions
}

fn evaluate_overview_warning(
    disk: Option<&DiskStatsDto>,
    settings_rec: &WorkspaceSettingsRecord,
    soft_limit_bots: Vec<Uuid>,
    largest_items: &[WorkspaceBigItemDto],
    suggestions: &[CleanupSuggestionDto],
) -> Option<WorkspaceWarningDto> {
    let threshold_percent = disk.and_then(|d| {
        if d.used_percent >= settings_rec.warn_percent_3 {
            Some(settings_rec.warn_percent_3)
        } else if d.used_percent >= settings_rec.warn_percent_2 {
            Some(settings_rec.warn_percent_2)
        } else if d.used_percent >= settings_rec.warn_percent_1 {
            Some(settings_rec.warn_percent_1)
        } else {
            None
        }
    });
    let below_min_free = disk.is_some_and(|d| d.available_bytes < settings_rec.min_free_bytes);
    if threshold_percent.is_none() && !below_min_free && soft_limit_bots.is_empty() {
        return None;
    }
    let critical =
        threshold_percent.is_some_and(|t| t >= settings_rec.warn_percent_3) || below_min_free;
    Some(WorkspaceWarningDto {
        severity: if critical { "critical" } else { "warning" }.to_owned(),
        threshold_percent,
        below_min_free,
        disk_used_percent: disk.map(|d| d.used_percent),
        available_bytes: disk.map(|d| d.available_bytes),
        min_free_bytes: settings_rec.min_free_bytes,
        soft_limit_bots,
        largest_items: largest_items.to_vec(),
        suggestions: suggestions.to_vec(),
    })
}

fn build_overview_inner(state: &AppState) -> Result<WorkspacesOverviewDto, ApiError> {
    let user_id = state.user_id();
    let bots = state.storage().list_bots(user_id)?;
    let settings_rec = state.storage().get_workspace_settings(user_id)?;
    let settings: WorkspaceSettingsDto = settings_rec.clone().into();
    let validated_root = match state.workspaces_root().as_deref() {
        Some(r) => Some(
            builtin_tools::workspace::validate_workspaces_root(r, &state.data_dir())
                .map_err(ApiError::bad_request)?,
        ),
        None => None,
    };

    let disk = validated_root
        .as_deref()
        .and_then(builtin_tools::workspace::host_disk_stats)
        .map(|d| DiskStatsDto {
            total_bytes: d.total_bytes,
            available_bytes: d.free_bytes,
            used_bytes: d.used_bytes,
            used_percent: d.used_percent,
        });

    let mut bot_summaries = Vec::with_capacity(bots.len());
    let mut total_workspace_bytes = 0u64;
    let mut big_items = Vec::new();
    let mut old_downloads = (0u64, 0u64);
    let mut old_logs = (0u64, 0u64);
    let mut soft_limit_bots = Vec::new();

    for bot in &bots {
        let rec = ensure_bot_record(state, bot)?;
        let ws_dir = validated_root.as_deref().and_then(|r| {
            builtin_tools::workspace::ensure_bot_workspace_dir(
                r,
                &rec.folder_name,
                &state.data_dir(),
            )
            .ok()
        });
        let (used_bytes, file_count, exists) = match ws_dir.as_deref() {
            Some(dir) if dir.is_dir() => {
                let (b, c) = scan_dir_stats_and_items(
                    Some((bot, &rec)),
                    dir,
                    &settings_rec,
                    &mut big_items,
                    &mut old_downloads,
                    &mut old_logs,
                );
                (b, c, true)
            }
            _ => (0, 0, false),
        };
        total_workspace_bytes = total_workspace_bytes.saturating_add(used_bytes);
        let soft_exceeded = rec.soft_limit_bytes.is_some_and(|l| used_bytes >= l);
        let hard_exceeded = rec.hard_limit_bytes.is_some_and(|l| used_bytes >= l);
        if soft_exceeded || hard_exceeded {
            soft_limit_bots.push(bot.id.0);
        }
        bot_summaries.push(BotWorkspaceSummaryDto {
            bot_id: bot.id.0,
            bot_name: bot.name.clone(),
            folder_name: rec.folder_name,
            workspace_path: ws_dir.map(|p| p.display().to_string()),
            exists,
            used_bytes,
            file_count,
            soft_limit_bytes: rec.soft_limit_bytes,
            hard_limit_bytes: rec.hard_limit_bytes,
            soft_limit_exceeded: soft_exceeded,
            hard_limit_exceeded: hard_exceeded,
        });
    }

    bot_summaries.sort_by(|a, b| {
        b.used_bytes
            .cmp(&a.used_bytes)
            .then_with(|| a.bot_name.cmp(&b.bot_name))
    });
    big_items.sort_by_key(|item| std::cmp::Reverse(item.size_bytes));
    big_items.truncate(DEFAULT_BIG_LIMIT);

    let suggestions = build_cleanup_suggestions(&state.data_dir(), old_downloads, old_logs);
    let active_warning = evaluate_overview_warning(
        disk.as_ref(),
        &settings_rec,
        soft_limit_bots,
        &big_items,
        &suggestions,
    );

    Ok(WorkspacesOverviewDto {
        configured: validated_root.is_some(),
        root_path: validated_root.map(|p| p.display().to_string()),
        disk,
        total_workspace_bytes,
        settings,
        bots: bot_summaries,
        largest_items: big_items,
        suggestions,
        active_warning,
    })
}

/// Payload of the SSE `workspace_warning` event.
///
/// `GET /v1/events` is not owner-gated: any local bearer token (a CLI engine, an MCP
/// client, a script) can read it. The event therefore carries numbers and bot ids
/// only. It never names a bot, a folder or a file; the owner fetches those from the
/// owner-gated `GET /v1/workspaces` when this event arrives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceWarningEventDto {
    pub severity: String,
    pub threshold_percent: Option<u8>,
    pub below_min_free: bool,
    pub disk_used_percent: Option<u8>,
    pub available_bytes: Option<u64>,
    pub min_free_bytes: u64,
    pub soft_limit_bots: Vec<Uuid>,
}

/// Build the name-free [`WorkspaceWarningEventDto`] for a write-time warning event.
pub(crate) fn warning_event_dto(
    state: &AppState,
    event: &agent_core::WorkspaceWarningEvent,
) -> WorkspaceWarningEventDto {
    use builtin_tools::workspace::WriteWarningKind;
    let settings = state.storage().get_workspace_settings(state.user_id()).ok();
    let (threshold_percent, below_min_free, disk_used_percent, available_bytes, min_free_bytes) =
        match event.kind {
            WriteWarningKind::DiskPercent {
                threshold_percent,
                used_percent,
                free_after_bytes,
            } => (
                Some(threshold_percent),
                false,
                Some(used_percent),
                Some(free_after_bytes),
                settings.as_ref().map(|s| s.min_free_bytes),
            ),
            WriteWarningKind::LowFreeSpace {
                free_after_bytes,
                min_free_bytes,
                used_percent,
            } => (
                None,
                true,
                Some(used_percent),
                Some(free_after_bytes),
                Some(min_free_bytes),
            ),
            WriteWarningKind::BotSoftLimit { .. } => (
                None,
                false,
                None,
                None,
                settings.as_ref().map(|s| s.min_free_bytes),
            ),
        };
    let critical_percent = settings
        .as_ref()
        .map_or(builtin_tools::workspace::DEFAULT_WARN_PERCENT_3, |s| {
            s.warn_percent_3
        });
    let critical = below_min_free || threshold_percent.is_some_and(|t| t >= critical_percent);
    let soft_limit_bots = if matches!(event.kind, WriteWarningKind::BotSoftLimit { .. }) {
        event.bot_id.map(|b| b.0).into_iter().collect()
    } else {
        Vec::new()
    };
    WorkspaceWarningEventDto {
        severity: if critical { "critical" } else { "warning" }.to_owned(),
        threshold_percent,
        below_min_free,
        disk_used_percent,
        available_bytes,
        min_free_bytes: min_free_bytes.unwrap_or(builtin_tools::workspace::DEFAULT_MIN_FREE_BYTES),
        soft_limit_bots,
    }
}

/// Host disk stats, per-bot workspace usage, settings, and cleanup suggestions.
#[utoipa::path(
    get,
    path = "/v1/workspaces",
    tag = "workspaces",
    security(("bearer" = [])),
    responses(
        (status = 200, body = WorkspacesOverviewDto),
        (status = 403, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn list_workspaces(
    State(state): State<AppState>,
) -> Result<Json<WorkspacesOverviewDto>, ApiError> {
    let overview = build_overview_inner(&state)?;
    Ok(Json(overview))
}

/// Update workspace disk warning thresholds and cleanup retention policy.
#[utoipa::path(
    put,
    path = "/v1/workspaces/settings",
    tag = "workspaces",
    security(("bearer" = [])),
    request_body = WorkspaceSettingsInput,
    responses(
        (status = 200, body = WorkspaceSettingsDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn put_workspace_settings(
    State(state): State<AppState>,
    Json(input): Json<WorkspaceSettingsInput>,
) -> Result<Json<WorkspaceSettingsDto>, ApiError> {
    if input.warn_percent_1 < 1
        || input.warn_percent_1 >= input.warn_percent_2
        || input.warn_percent_2 >= input.warn_percent_3
        || input.warn_percent_3 > 99
    {
        return Err(ApiError::bad_request(
            "warning percentages must strictly increase between 1 and 99",
        ));
    }
    if input.min_free_bytes > MAX_MIN_FREE_BYTES {
        return Err(ApiError::bad_request(
            "minimum free space threshold is too large",
        ));
    }
    if let Some(d) = input.downloads_max_age_days
        && (d == 0 || d > MAX_AGE_DAYS)
    {
        return Err(ApiError::bad_request(
            "downloads max age days must be between 1 and 3650",
        ));
    }
    if let Some(d) = input.logs_max_age_days
        && (d == 0 || d > MAX_AGE_DAYS)
    {
        return Err(ApiError::bad_request(
            "logs max age days must be between 1 and 3650",
        ));
    }

    let rec = WorkspaceSettingsRecord {
        warn_percent_1: input.warn_percent_1,
        warn_percent_2: input.warn_percent_2,
        warn_percent_3: input.warn_percent_3,
        min_free_bytes: input.min_free_bytes,
        downloads_max_age_days: input.downloads_max_age_days,
        logs_max_age_days: input.logs_max_age_days,
        updated_at_ms: now_ms(),
    };
    state.storage().set_workspace_settings(
        state.user_id(),
        &rec,
        &json!({
            "warn_percent_1": rec.warn_percent_1,
            "warn_percent_2": rec.warn_percent_2,
            "warn_percent_3": rec.warn_percent_3,
            "min_free_bytes": rec.min_free_bytes,
            "downloads_max_age_days": rec.downloads_max_age_days,
            "logs_max_age_days": rec.logs_max_age_days,
        }),
    )?;
    Ok(Json(rec.into()))
}

/// Safely remove expired files in `downloads/`, `logs/`, or daemon cache dirs.
#[utoipa::path(
    post,
    path = "/v1/workspaces/cleanup",
    tag = "workspaces",
    security(("bearer" = [])),
    request_body = WorkspaceCleanupInput,
    responses(
        (status = 200, body = WorkspaceCleanupResultDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn run_workspace_cleanup(
    State(state): State<AppState>,
    Json(input): Json<WorkspaceCleanupInput>,
) -> Result<Json<WorkspaceCleanupResultDto>, ApiError> {
    if input.target == CleanupTarget::DaemonCache {
        let freed_bytes = crate::data_routes::clear_cache(&state.data_dir());
        state.storage().append_audit(
            state.user_id(),
            None,
            "user",
            "workspace.cleanup",
            &json!({ "target": "daemon_cache", "freed_bytes": freed_bytes }),
        )?;
        return Ok(Json(WorkspaceCleanupResultDto {
            target: CleanupTarget::DaemonCache,
            deleted_files: 0,
            freed_bytes,
        }));
    }

    let settings = state.storage().get_workspace_settings(state.user_id())?;
    let target_bots = match input.bot_id {
        Some(id) => vec![state.bot_of_user(BotId(id))?],
        None => state.storage().list_bots(state.user_id())?,
    };

    let Some(root) = state.workspaces_root() else {
        return Ok(Json(WorkspaceCleanupResultDto {
            target: input.target,
            deleted_files: 0,
            freed_bytes: 0,
        }));
    };
    let root_real = builtin_tools::workspace::validate_workspaces_root(&root, &state.data_dir())
        .map_err(ApiError::bad_request)?;

    let mut freed_bytes = 0u64;
    let mut deleted_files = 0u64;

    for bot in &target_bots {
        let rec = ensure_bot_record(&state, bot)?;
        let Ok(ws_dir) = builtin_tools::workspace::ensure_bot_workspace_dir(
            &root_real,
            &rec.folder_name,
            &state.data_dir(),
        ) else {
            continue;
        };
        if matches!(
            input.target,
            CleanupTarget::OldDownloads | CleanupTarget::AllOld
        ) && let Some(days) = settings.downloads_max_age_days
        {
            let (f, d) = cleanup_old_subdir(&ws_dir.join("downloads"), days);
            freed_bytes = freed_bytes.saturating_add(f);
            deleted_files = deleted_files.saturating_add(d);
        }
        if matches!(input.target, CleanupTarget::OldLogs | CleanupTarget::AllOld)
            && let Some(days) = settings.logs_max_age_days
        {
            let (f, d) = cleanup_old_subdir(&ws_dir.join("logs"), days);
            freed_bytes = freed_bytes.saturating_add(f);
            deleted_files = deleted_files.saturating_add(d);
        }
    }

    state.storage().append_audit(
        state.user_id(),
        None,
        "user",
        "workspace.cleanup",
        &json!({
            "target": input.target,
            "bot_id": input.bot_id,
            "deleted_files": deleted_files,
            "freed_bytes": freed_bytes,
        }),
    )?;

    Ok(Json(WorkspaceCleanupResultDto {
        target: input.target,
        deleted_files,
        freed_bytes,
    }))
}

/// Browse or search files strictly within a bot's own workspace directory.
#[utoipa::path(
    get,
    path = "/v1/bots/{id}/workspace",
    tag = "workspaces",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Bot id"),
        ("path" = Option<String>, Query, description = "Relative subfolder path inside the bot workspace"),
        ("q" = Option<String>, Query, description = "Case-insensitive filename search filter")
    ),
    responses(
        (status = 200, body = BotWorkspaceListDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn get_bot_workspace(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Query(query): Query<WorkspaceListQuery>,
) -> Result<Json<BotWorkspaceListDto>, ApiError> {
    let bot = state.bot_of_user(BotId(id))?;
    let (rec, ws_dir) = ensure_bot_dir_if_configured(&state, &bot)?;
    let segments = normalize_rel_segments(query.path.as_deref().unwrap_or(""))?;
    let current_path = segments.join("/");
    let search_q = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);

    let mut entries = Vec::new();
    let used_bytes = match ws_dir.as_deref() {
        Some(root) => {
            let target = resolve_existing_in_workspace(root, &segments, true)?;
            let target_meta = fs::symlink_metadata(&target)
                .map_err(|e| ApiError::not_found(format!("folder: {e}")))?;
            if !target_meta.is_dir() {
                return Err(ApiError::bad_request(
                    "requested workspace path is not a folder",
                ));
            }
            if let Some(q) = search_q.as_deref() {
                collect_search_entries(&target, &current_path, q, &mut entries);
            } else {
                let rd = fs::read_dir(&target)
                    .map_err(|e| ApiError::internal(format!("read workspace dir: {e}")))?;
                for child in rd.filter_map(Result::ok) {
                    let name = child.file_name().to_string_lossy().into_owned();
                    let rel = if current_path.is_empty() {
                        name
                    } else {
                        format!("{current_path}/{name}")
                    };
                    if let Some(dto) = entry_dto_from_path(&child.path(), rel) {
                        entries.push(dto);
                    }
                }
            }
            entries.sort_by(|a, b| {
                let a_dir = a.kind == WorkspaceEntryKind::Directory;
                let b_dir = b.kind == WorkspaceEntryKind::Directory;
                b_dir.cmp(&a_dir).then_with(|| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                })
            });
            builtin_tools::workspace::dir_size_bytes(root)
        }
        None => 0,
    };

    let undo_items = list_and_prune_trash(&state, bot.id);
    Ok(Json(BotWorkspaceListDto {
        bot_id: bot.id.0,
        bot_name: bot.name,
        folder_name: rec.folder_name,
        workspace_path: ws_dir.map(|p| p.display().to_string()),
        current_path,
        used_bytes,
        soft_limit_bytes: rec.soft_limit_bytes,
        hard_limit_bytes: rec.hard_limit_bytes,
        soft_limit_exceeded: rec.soft_limit_bytes.is_some_and(|l| used_bytes >= l),
        hard_limit_exceeded: rec.hard_limit_bytes.is_some_and(|l| used_bytes >= l),
        entries,
        undo_items,
    }))
}

/// Read or download a single file strictly within a bot's workspace.
#[utoipa::path(
    get,
    path = "/v1/bots/{id}/workspace/file",
    tag = "workspaces",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Bot id"),
        ("path" = String, Query, description = "Relative file path inside the bot workspace"),
        ("download" = Option<bool>, Query, description = "When true, send as attachment download")
    ),
    responses(
        (status = 200, description = "File bytes with nosniff and sandbox headers", content_type = "application/octet-stream", body = String),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn get_bot_workspace_file(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<Response, ApiError> {
    let bot = state.bot_of_user(BotId(id))?;
    let (_, ws_dir) = ensure_bot_dir_if_configured(&state, &bot)?;
    let ws_root =
        ws_dir.ok_or_else(|| ApiError::bad_request("workspaces root is not configured"))?;
    let segments = normalize_rel_segments(&query.path)?;
    let file_path = resolve_existing_in_workspace(&ws_root, &segments, false)?;
    let meta =
        fs::symlink_metadata(&file_path).map_err(|_| ApiError::not_found("workspace file"))?;
    if !meta.is_file() {
        return Err(ApiError::bad_request(
            "requested path is not a regular file",
        ));
    }
    let max = if query.download {
        MAX_DOWNLOAD_BYTES
    } else {
        MAX_PREVIEW_BYTES
    };
    if meta.len() > max {
        return Err(ApiError::bad_request(format!(
            "file exceeds the {} MiB limit for {}",
            max / (1024 * 1024),
            if query.download {
                "download"
            } else {
                "preview"
            }
        )));
    }
    let bytes = fs::read(&file_path).map_err(|e| ApiError::internal(format!("read file: {e}")))?;

    let content_type = if query.download {
        "application/octet-stream"
    } else if let Some(img) = image_mime_for(&file_path) {
        img
    } else if preview_kind_for(&file_path, false) == WorkspacePreviewKind::Text {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };

    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "sandbox")
        .header(header::CACHE_CONTROL, "no-store");

    if query.download {
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .replace(['"', '\r', '\n', '\\'], "_");
        if let Ok(val) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
            resp = resp.header(header::CONTENT_DISPOSITION, val);
        }
    }

    resp.body(Body::from(bytes))
        .map_err(|e| ApiError::internal(e.to_string()))
        .map(IntoResponse::into_response)
}

/// Rename a file or folder within the same parent directory inside a bot's workspace.
#[utoipa::path(
    post,
    path = "/v1/bots/{id}/workspace/rename",
    tag = "workspaces",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = WorkspaceRenameInput,
    responses(
        (status = 200, body = WorkspaceEntryDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn rename_bot_workspace_entry(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Json(input): Json<WorkspaceRenameInput>,
) -> Result<Json<WorkspaceEntryDto>, ApiError> {
    let bot = state.bot_of_user(BotId(id))?;
    let (_, ws_dir) = ensure_bot_dir_if_configured(&state, &bot)?;
    let ws_root =
        ws_dir.ok_or_else(|| ApiError::bad_request("workspaces root is not configured"))?;
    let new_name = input.new_name.trim();
    if !is_valid_entry_name(new_name) {
        return Err(ApiError::bad_request("invalid destination file name"));
    }
    let segments = normalize_rel_segments(&input.path)?;
    let src = resolve_existing_in_workspace(&ws_root, &segments, false)?;
    let mut dst_segments = segments;
    let Some(last) = dst_segments.last_mut() else {
        return Err(ApiError::bad_request("cannot rename workspace root"));
    };
    new_name.clone_into(last);
    let dst = src
        .parent()
        .ok_or_else(|| ApiError::bad_request("missing parent folder"))?
        .join(new_name);
    if fs::symlink_metadata(&dst).is_ok() {
        return Err(ApiError::conflict("an item with that name already exists"));
    }
    fs::rename(&src, &dst).map_err(|e| ApiError::internal(format!("rename failed: {e}")))?;
    let rel = dst_segments.join("/");
    let dto = entry_dto_from_path(&dst, rel)
        .ok_or_else(|| ApiError::internal("renamed entry disappeared"))?;
    state.storage().append_audit(
        state.user_id(),
        None,
        "user",
        "workspace.entry_renamed",
        &json!({ "bot_id": bot.id.0, "from": input.path, "to": dto.path }),
    )?;
    Ok(Json(dto))
}

/// Delete a file or subfolder inside a bot's workspace, returning an undo token.
#[utoipa::path(
    post,
    path = "/v1/bots/{id}/workspace/delete",
    tag = "workspaces",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = WorkspaceDeleteInput,
    responses(
        (status = 200, body = WorkspaceTrashEntryDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn delete_bot_workspace_entry(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Json(input): Json<WorkspaceDeleteInput>,
) -> Result<Json<WorkspaceTrashEntryDto>, ApiError> {
    let bot = state.bot_of_user(BotId(id))?;
    let (_, ws_dir) = ensure_bot_dir_if_configured(&state, &bot)?;
    let ws_root =
        ws_dir.ok_or_else(|| ApiError::bad_request("workspaces root is not configured"))?;
    let segments = normalize_rel_segments(&input.path)?;
    let src = resolve_existing_in_workspace(&ws_root, &segments, false)?;
    let meta = fs::symlink_metadata(&src).map_err(|_| ApiError::not_found("workspace entry"))?;
    let size_bytes = if meta.is_dir() {
        builtin_tools::workspace::dir_size_bytes(&src)
    } else {
        meta.len()
    };
    let name = segments
        .last()
        .cloned()
        .unwrap_or_else(|| "item".to_owned());
    let original_path = segments.join("/");
    let undo_id = Uuid::now_v7().to_string();
    let deleted_at_ms = now_ms();

    let item_dir = trash_bot_dir(&state, bot.id).join(&undo_id);
    fs::create_dir_all(&item_dir).map_err(|e| ApiError::internal(format!("trash mkdir: {e}")))?;
    let payload_path = item_dir.join("payload");
    move_entry(&src, &payload_path).map_err(|e| ApiError::internal(format!("trash move: {e}")))?;

    let trash_meta = TrashMeta {
        undo_id: undo_id.clone(),
        name: name.clone(),
        original_path: original_path.clone(),
        size_bytes,
        deleted_at_ms,
    };
    let meta_json =
        serde_json::to_vec(&trash_meta).map_err(|e| ApiError::internal(e.to_string()))?;
    fs::write(item_dir.join("meta.json"), meta_json)
        .map_err(|e| ApiError::internal(format!("write trash meta: {e}")))?;

    state.storage().append_audit(
        state.user_id(),
        None,
        "user",
        "workspace.entry_deleted",
        &json!({ "bot_id": bot.id.0, "path": original_path, "undo_id": undo_id, "size_bytes": size_bytes }),
    )?;

    Ok(Json(WorkspaceTrashEntryDto {
        undo_id,
        name,
        original_path,
        size_bytes,
        deleted_at_ms,
    }))
}

/// Restore a recently deleted workspace entry from trash (`undo_id`).
#[utoipa::path(
    post,
    path = "/v1/bots/{id}/workspace/restore",
    tag = "workspaces",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = WorkspaceRestoreInput,
    responses(
        (status = 200, body = WorkspaceEntryDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn restore_bot_workspace_entry(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Json(input): Json<WorkspaceRestoreInput>,
) -> Result<Json<WorkspaceEntryDto>, ApiError> {
    let undo_uuid = Uuid::parse_str(input.undo_id.trim())
        .map_err(|_| ApiError::bad_request("invalid undo id"))?;
    let bot = state.bot_of_user(BotId(id))?;
    let (rec, ws_dir) = ensure_bot_dir_if_configured(&state, &bot)?;
    let ws_root =
        ws_dir.ok_or_else(|| ApiError::bad_request("workspaces root is not configured"))?;

    let item_dir = trash_bot_dir(&state, bot.id).join(undo_uuid.to_string());
    let meta_path = item_dir.join("meta.json");
    let payload_path = item_dir.join("payload");
    let raw = fs::read_to_string(&meta_path)
        .map_err(|_| ApiError::not_found("deleted item expired or unknown"))?;
    let trash_meta = serde_json::from_str::<TrashMeta>(&raw)
        .map_err(|e| ApiError::internal(format!("corrupt trash meta: {e}")))?;
    if trash_meta.deleted_at_ms < now_ms().saturating_sub(UNDO_TTL_MS) {
        return Err(ApiError::not_found("deleted item expired"));
    }
    if !payload_path.exists() {
        return Err(ApiError::not_found("deleted payload missing"));
    }
    let restored_bytes = builtin_tools::workspace::dir_size_bytes(&payload_path);
    if rec.hard_limit_bytes.is_some_and(|limit| {
        builtin_tools::workspace::dir_size_bytes(&ws_root).saturating_add(restored_bytes) > limit
    }) {
        return Err(ApiError::bad_request(
            "restoring this item would exceed the workspace hard limit",
        ));
    }

    let segments = normalize_rel_segments(&trash_meta.original_path)?;
    if segments.is_empty() {
        return Err(ApiError::bad_request("invalid restore path"));
    }
    let dst = ensure_plain_parents(&ws_root, &segments)?;
    if fs::symlink_metadata(&dst).is_ok() {
        return Err(ApiError::conflict(
            "an item already exists at the original path",
        ));
    }
    move_entry(&payload_path, &dst)
        .map_err(|e| ApiError::internal(format!("restore move: {e}")))?;
    let _ = fs::remove_dir_all(&item_dir);

    let dto = entry_dto_from_path(&dst, trash_meta.original_path.clone())
        .ok_or_else(|| ApiError::internal("restored entry disappeared"))?;
    state.storage().append_audit(
        state.user_id(),
        None,
        "user",
        "workspace.entry_restored",
        &json!({ "bot_id": bot.id.0, "path": trash_meta.original_path, "undo_id": trash_meta.undo_id }),
    )?;
    Ok(Json(dto))
}

/// Set or clear optional soft/hard disk limits (bytes) on a single bot's workspace.
#[utoipa::path(
    put,
    path = "/v1/bots/{id}/workspace/limits",
    tag = "workspaces",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Bot id")),
    request_body = BotWorkspaceLimitsInput,
    responses(
        (status = 200, body = BotWorkspaceSummaryDto),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub(crate) async fn put_bot_workspace_limits(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Uuid>,
    Json(input): Json<BotWorkspaceLimitsInput>,
) -> Result<Json<BotWorkspaceSummaryDto>, ApiError> {
    if input.soft_limit_bytes == Some(0) || input.hard_limit_bytes == Some(0) {
        return Err(ApiError::bad_request(
            "limits must be positive byte counts (omit or set null to disable)",
        ));
    }
    if let (Some(soft), Some(hard)) = (input.soft_limit_bytes, input.hard_limit_bytes)
        && soft > hard
    {
        return Err(ApiError::bad_request("soft limit cannot exceed hard limit"));
    }
    let bot = state.bot_of_user(BotId(id))?;
    let preferred =
        builtin_tools::workspace::sanitize_bot_folder_name(&bot.name, &bot.id.to_string());
    let rec = state.storage().set_bot_workspace_limits(
        state.user_id(),
        bot.id,
        &preferred,
        input.soft_limit_bytes,
        input.hard_limit_bytes,
        &json!({
            "soft_limit_bytes": input.soft_limit_bytes,
            "hard_limit_bytes": input.hard_limit_bytes,
        }),
    )?;
    let ws_dir = state.workspaces_root().and_then(|r| {
        builtin_tools::workspace::ensure_bot_workspace_dir(&r, &rec.folder_name, &state.data_dir())
            .ok()
    });
    let used_bytes = ws_dir
        .as_deref()
        .map_or(0, builtin_tools::workspace::dir_size_bytes);
    Ok(Json(BotWorkspaceSummaryDto {
        bot_id: bot.id.0,
        bot_name: bot.name,
        folder_name: rec.folder_name,
        workspace_path: ws_dir.as_ref().map(|p| p.display().to_string()),
        exists: ws_dir.as_deref().is_some_and(Path::is_dir),
        used_bytes,
        file_count: 0,
        soft_limit_bytes: rec.soft_limit_bytes,
        hard_limit_bytes: rec.hard_limit_bytes,
        soft_limit_exceeded: rec.soft_limit_bytes.is_some_and(|l| used_bytes >= l),
        hard_limit_exceeded: rec.hard_limit_bytes.is_some_and(|l| used_bytes >= l),
    }))
}
