//! Per-run file-change review, restore preview, and safe restore routes.

use std::io;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use builtin_tools::{
    ChangeKind, CheckpointStore, DiffOmittedReason, FileChange, RestoreAction, RestorePreviewEntry,
    UnifiedDiff,
};
use protocol::event::CheckpointRestored;
use protocol::{Event, RunId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::error::{ApiError, ErrorBody};

pub(crate) const COVERED_TOOLS: &[&str] = &["fs_write", "fs_edit", "fs_move", "fs_delete"];
pub(crate) const NOT_COVERED_TOOLS: &[&str] = &["shell", "web", "email", "desktop"];

fn scope_lists() -> (Vec<String>, Vec<String>) {
    (
        COVERED_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
        NOT_COVERED_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
    )
}

/// Net file change classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKindDto {
    Created,
    Modified,
    Deleted,
    Moved,
}

impl From<ChangeKind> for ChangeKindDto {
    fn from(k: ChangeKind) -> Self {
        match k {
            ChangeKind::Created => Self::Created,
            ChangeKind::Modified => Self::Modified,
            ChangeKind::Deleted => Self::Deleted,
            ChangeKind::Moved => Self::Moved,
        }
    }
}

/// Why inline text diffing was omitted for a changed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiffOmittedReasonDto {
    Binary,
    TooLarge,
    RunBudgetExceeded,
    Directory,
    Symlink,
    NotAccessible,
    LineEndings,
}

impl From<DiffOmittedReason> for DiffOmittedReasonDto {
    fn from(r: DiffOmittedReason) -> Self {
        match r {
            DiffOmittedReason::Binary => Self::Binary,
            DiffOmittedReason::TooLarge => Self::TooLarge,
            DiffOmittedReason::RunBudgetExceeded => Self::RunBudgetExceeded,
            DiffOmittedReason::Directory => Self::Directory,
            DiffOmittedReason::Symlink => Self::Symlink,
            DiffOmittedReason::NotAccessible => Self::NotAccessible,
            DiffOmittedReason::LineEndings => Self::LineEndings,
        }
    }
}

/// Unified text diff payload for a single changed text file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UnifiedDiffDto {
    pub unified: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub hunks: usize,
}

impl From<UnifiedDiff> for UnifiedDiffDto {
    fn from(d: UnifiedDiff) -> Self {
        Self {
            unified: d.unified,
            added_lines: d.added_lines,
            removed_lines: d.removed_lines,
            hunks: d.hunks,
        }
    }
}

/// One collapsed file change recorded during a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct FileChangeDto {
    pub accessible: bool,
    pub changed_by_run: Option<Uuid>,
    pub path: String,
    pub moved_from: Option<String>,
    pub op: String,
    pub checkpoint_id: String,
    pub existed: bool,
    pub kind: ChangeKindDto,
    pub before_size: Option<u64>,
    pub after_size: Option<u64>,
    pub text: Option<UnifiedDiffDto>,
    pub diff_omitted_reason: Option<DiffOmittedReasonDto>,
    pub current_matches_after: bool,
    pub via_link_of: Option<String>,
    pub is_dir: bool,
    pub is_link: bool,
}

impl From<FileChange> for FileChangeDto {
    fn from(c: FileChange) -> Self {
        Self {
            accessible: c.accessible,
            changed_by_run: None,
            path: c.path,
            moved_from: c.moved_from,
            op: c.op,
            checkpoint_id: c.checkpoint_id,
            existed: c.existed,
            kind: c.kind.into(),
            before_size: c.before_size,
            after_size: c.after_size,
            text: c.text.map(Into::into),
            diff_omitted_reason: c.diff_omitted_reason.map(Into::into),
            current_matches_after: c.current_matches_after,
            via_link_of: c.via_link_of,
            is_dir: c.is_dir,
            is_link: c.is_link,
        }
    }
}

/// Response for `GET /v1/runs/{id}/changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunChangesDto {
    pub run_id: Uuid,
    pub changes: Vec<FileChangeDto>,
    pub next_offset: Option<usize>,
    pub covered: Vec<String>,
    pub not_covered: Vec<String>,
}

/// Action a restore operation will apply to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestoreActionDto {
    Restore,
    Delete,
    Keep,
}

impl From<RestoreAction> for RestoreActionDto {
    fn from(a: RestoreAction) -> Self {
        match a {
            RestoreAction::Restore => Self::Restore,
            RestoreAction::Delete => Self::Delete,
            RestoreAction::Keep => Self::Keep,
        }
    }
}

/// Per-path dry-run entry for a checkpoint or whole-run restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RestorePreviewEntryDto {
    pub accessible: bool,
    pub changed_by_run: Option<Uuid>,
    pub path: String,
    pub current_matches_after: bool,
    pub action: RestoreActionDto,
    pub via_link_of: Option<String>,
}

impl From<RestorePreviewEntry> for RestorePreviewEntryDto {
    fn from(e: RestorePreviewEntry) -> Self {
        Self {
            accessible: e.accessible,
            changed_by_run: None,
            path: e.path,
            current_matches_after: e.current_matches_after,
            action: e.action.into(),
            via_link_of: e.via_link_of,
        }
    }
}

/// Response for `GET /v1/runs/{id}/changes/{cp}/preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RestorePreviewDto {
    pub run_id: Uuid,
    pub checkpoint_id: String,
    pub entries: Vec<RestorePreviewEntryDto>,
    pub modified_since: Vec<String>,
    pub covered: Vec<String>,
    pub not_covered: Vec<String>,
}

/// Body for `POST /v1/runs/{id}/changes/{cp}/restore`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct RestoreRequestDto {
    #[serde(default)]
    pub force: bool,
}

/// Response for `POST /v1/runs/{id}/changes/{cp}/restore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RestoreResponseDto {
    pub partial: bool,
    pub error: Option<String>,
    pub run_id: Uuid,
    pub checkpoint_id: String,
    pub safety_checkpoint_ids: Vec<String>,
    pub restored: Vec<String>,
    pub warnings: Vec<String>,
    pub covered: Vec<String>,
    pub not_covered: Vec<String>,
}

fn parse_run_id(raw: &str) -> Result<RunId, ApiError> {
    raw.parse::<Uuid>()
        .map(RunId)
        .map_err(|_| ApiError::bad_request("malformed id"))
}

fn ensure_run_finished(run: &storage::Run) -> Result<(), ApiError> {
    if !matches!(run.state.as_str(), "succeeded" | "failed" | "cancelled") {
        return Err(ApiError::conflict(
            "run is not terminal; finish or cancel it before restoring changes",
        ));
    }
    Ok(())
}

fn map_store_err(err: io::Error) -> ApiError {
    match err.kind() {
        io::ErrorKind::NotFound => ApiError::not_found(err.to_string()),
        io::ErrorKind::InvalidInput => ApiError::bad_request(err.to_string()),
        io::ErrorKind::PermissionDenied => {
            ApiError::new(StatusCode::FORBIDDEN, "forbidden", err.to_string())
        }
        _ => ApiError::internal(err),
    }
}

fn verify_checkpoint_scope(
    store: &CheckpointStore,
    cp: &str,
    run_id: &str,
) -> Result<(), ApiError> {
    if cp == "all" {
        return Ok(());
    }
    let belongs = store
        .checkpoint_belongs_to_run(cp, run_id)
        .map_err(map_store_err)?;
    if !belongs {
        return Err(ApiError::not_found(format!("checkpoint {cp}")));
    }
    Ok(())
}

#[derive(Default, Deserialize)]
pub(crate) struct ChangesQuery {
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

struct RestoreLock {
    state: AppState,
    run: RunId,
}
impl RestoreLock {
    fn acquire(state: &AppState, run: RunId) -> Result<Self, ApiError> {
        // Also serialize different runs' restores: they can touch the same files.
        let mut locks = state.0.restore_locks.lock();
        if !locks.is_empty() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "restore_in_progress",
                "another restore is in progress",
            ));
        }
        locks.insert(run);
        Ok(Self {
            state: state.clone(),
            run,
        })
    }
}
impl Drop for RestoreLock {
    fn drop(&mut self) {
        self.state.0.restore_locks.lock().remove(&self.run);
    }
}

fn later_writers(
    s: &AppState,
    run: &storage::Run,
    paths: &std::collections::HashSet<&str>,
) -> std::collections::HashMap<String, Uuid> {
    // Blocking-pool metadata scan once per response, with bounded history.
    // Absence means unknown; never attribute an unmatched hash to the user.
    let store = CheckpointStore::new(s.storage().data_dir());
    let mut out = std::collections::HashMap::new();
    if paths.is_empty() {
        return out;
    }
    if let Ok(mut runs) = s.storage().list_runs(s.user_id()) {
        runs.sort_by_key(|r| r.created_at_ms);
        for other in runs
            .into_iter()
            .rev()
            .filter(|r| r.id != run.id && r.created_at_ms >= run.created_at_ms)
            .take(100)
        {
            if let Ok(cps) = store.list(&other.id.0.to_string()) {
                for cp in cps
                    .into_iter()
                    .filter(|cp| !cp.op.starts_with("pre-restore:"))
                {
                    for entry in cp.entries {
                        if paths.contains(entry.path.as_str()) {
                            out.entry(entry.path).or_insert(other.id.0);
                        }
                    }
                }
            }
        }
    }
    out
}

/// List net file changes made by a run's file tools, with capped inline diffs.
#[utoipa::path(
    get,
    path = "/v1/runs/{id}/changes",
    tag = "runs",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Run id"),
        ("offset" = Option<usize>, Query, description = "File offset"),
        ("limit" = Option<usize>, Query, description = "Page size, maximum 100")),
    responses(
        (status = 200, body = RunChangesDto),
        (status = 404, body = ErrorBody)
    )
)]
pub(crate) async fn get_run_changes(
    State(s): State<AppState>,
    Path(rid): Path<String>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<RunChangesDto>, ApiError> {
    let run_id = parse_run_id(&rid)?;
    let run = s.run_of_user(run_id)?;
    let data_dir = s.storage().data_dir().to_path_buf();
    let run_str = run.id.0.to_string();

    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let state = s.clone();
    let review_run = run.clone();
    let (roots, grants) = s.agent().checkpoint_access();
    let changes = tokio::task::spawn_blocking(move || {
        builtin_tools::with_checkpoint_access(&data_dir, &roots, &grants, || {
            let store = CheckpointStore::new(&data_dir);
            let changes = store.changes_page(&run_str, query.offset, limit)?;
            let writers = later_writers(
                &state,
                &review_run,
                &changes
                    .iter()
                    .filter(|c| c.accessible && !c.current_matches_after)
                    .map(|c| c.path.as_str())
                    .collect(),
            );
            Ok::<_, io::Error>(
                changes
                    .into_iter()
                    .map(|c| {
                        let mut dto = FileChangeDto::from(c);
                        if let Some(text) = &mut dto.text {
                            text.unified = state
                                .agent()
                                .redact_host_text(std::mem::take(&mut text.unified));
                        }
                        if !dto.current_matches_after && dto.accessible {
                            dto.changed_by_run = writers.get(&dto.path).copied();
                        }
                        dto
                    })
                    .collect::<Vec<_>>(),
            )
        })
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(map_store_err)?;

    let (covered, not_covered) = scope_lists();
    Ok(Json(RunChangesDto {
        run_id: run.id.0,
        next_offset: (changes.len() == limit).then_some(query.offset.saturating_add(limit)),
        changes,
        covered,
        not_covered,
    }))
}

/// Dry-run preview of restoring checkpoint `cp` (or `"all"` across the run).
#[utoipa::path(
    get,
    path = "/v1/runs/{id}/changes/{cp}/preview",
    tag = "runs",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Run id"),
        ("cp" = String, Path, description = "Checkpoint id or 'all'")
    ),
    responses(
        (status = 200, body = RestorePreviewDto),
        (status = 404, body = ErrorBody)
    )
)]
pub(crate) async fn preview_restore(
    State(s): State<AppState>,
    Path((rid, cp)): Path<(String, String)>,
) -> Result<Json<RestorePreviewDto>, ApiError> {
    let run_id = parse_run_id(&rid)?;
    let run = s.run_of_user(run_id)?;
    let data_dir = s.storage().data_dir().to_path_buf();
    let run_str = run.id.0.to_string();
    let cp_arg = cp.clone();

    let (roots, grants) = s.agent().checkpoint_access();
    let state = s.clone();
    let review_run = run.clone();
    let entries =
        tokio::task::spawn_blocking(move || -> Result<Vec<RestorePreviewEntryDto>, ApiError> {
            builtin_tools::with_checkpoint_access(&data_dir, &roots, &grants, || {
                let store = CheckpointStore::new(&data_dir);
                verify_checkpoint_scope(&store, &cp_arg, &run_str)?;
                store
                    .selection_preview(&run_str, &cp_arg)
                    .map_err(map_store_err)
                    .map(|entries| {
                        let writers = later_writers(
                            &state,
                            &review_run,
                            &entries
                                .iter()
                                .filter(|e| e.accessible && !e.current_matches_after)
                                .map(|e| e.path.as_str())
                                .collect(),
                        );
                        entries
                            .into_iter()
                            .map(|e| {
                                let mut dto = RestorePreviewEntryDto::from(e);
                                if dto.accessible && !dto.current_matches_after {
                                    dto.changed_by_run = writers.get(&dto.path).copied();
                                }
                                dto
                            })
                            .collect()
                    })
            })
        })
        .await
        .map_err(ApiError::internal)??;

    let modified_since: Vec<String> = entries
        .iter()
        .filter(|e| e.accessible && !e.current_matches_after)
        .map(|e| e.path.clone())
        .collect();
    let (covered, not_covered) = scope_lists();
    Ok(Json(RestorePreviewDto {
        run_id: run.id.0,
        checkpoint_id: cp,
        entries,
        modified_since,
        covered,
        not_covered,
    }))
}

/// Restore checkpoint `cp` (or `"all"` across the run), refusing with `409 modified_since`
/// unless `force: true` is provided.
#[utoipa::path(
    post,
    path = "/v1/runs/{id}/changes/{cp}/restore",
    tag = "runs",
    security(("bearer" = [])),
    params(
        ("id" = Uuid, Path, description = "Run id"),
        ("cp" = String, Path, description = "Checkpoint id or 'all'")
    ),
    request_body = Option<RestoreRequestDto>,
    responses(
        (status = 200, body = RestoreResponseDto),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "modified_since or run still active")
    )
)]
pub(crate) async fn restore_changes(
    State(s): State<AppState>,
    Path((rid, cp)): Path<(String, String)>,
    axum::Extension(actor): axum::Extension<crate::live_view::Actor>,
    body: axum::body::Bytes,
) -> Result<Json<RestoreResponseDto>, ApiError> {
    let body: RestoreRequestDto = if body.is_empty() {
        RestoreRequestDto::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError::bad_request(e.to_string()))?
    };
    let run_id = parse_run_id(&rid)?;
    let run = s.run_of_user(run_id)?;
    ensure_run_finished(&run)?;
    let lock = RestoreLock::acquire(&s, run_id)?;
    let data_dir = s.storage().data_dir().to_path_buf();
    let run_str = run.id.0.to_string();
    let cp_arg = cp.clone();
    let force = body.force;
    let (roots, grants) = s.agent().checkpoint_access();
    let state = s.clone();
    let audit_actor = actor.key.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        let report = builtin_tools::try_with_exclusive_file_access(&data_dir, || builtin_tools::with_checkpoint_access(&data_dir, &roots, &grants, || {
            // Refuse conservatively while any other run can still write. This
            // includes Paused runs that can resume with stale filesystem state.
            let runs = state.storage().list_runs(state.user_id()).map_err(ApiError::internal)?;
            if runs.iter().any(|r| r.id != run_id && ensure_run_finished(r).is_err()) {
                return Err(ApiError::conflict("another non-terminal run may be writing files; finish or cancel it first"));
            }
            ensure_run_finished(&state.run_of_user(run_id)?)?;
            let store = CheckpointStore::new(&data_dir);
            verify_checkpoint_scope(&store, &cp_arg, &run_str)?;
            let preview = store.selection_preview(&run_str, &cp_arg).map_err(map_store_err)?;
            let modified: Vec<_> = preview.into_iter().filter(|e| e.accessible && !e.current_matches_after).map(|e| e.path).collect();
            if !force && !modified.is_empty() {
                return Err(ApiError::new(StatusCode::CONFLICT, "modified_since", format!("{} file(s) changed or their after-state is unavailable ({}); explicit overwrite confirmation required", modified.len(), modified.join(", "))));
            }
            state.storage().append_audit(state.user_id(), Some(run_id), &audit_actor,
                "checkpoint.restore_started", &serde_json::json!({"checkpoint_id": cp_arg, "force": force}))
                .map_err(ApiError::internal)?;
            store.restore_selection_using(&run_str, &cp_arg, |index| {
                #[cfg(test)]
                if index == 0 {
                    if let Some(pause) = state.0.restore_pause.lock().take() {
                        let _ = pause.entered.send(());
                        pause.release.recv().map_err(io::Error::other)?;
                    }
                }
                #[cfg(test)]
                if *state.0.restore_fail_at.lock() == Some(index) { return Err(io::Error::other("injected checkpoint failure")); }
                let runs = state.storage().list_runs(state.user_id()).map_err(io::Error::other)?;
                if runs.iter().any(|r| ensure_run_finished(r).is_err()) {
                    return Err(io::Error::other(format!("a run became active during restore checkpoint {index}")));
                }
                Ok(())
            }).map_err(map_store_err)
        })).map_err(|e| ApiError::conflict(e.to_string()))??;
        // This blocking task owns mutation AND finalization. Dropping the HTTP
        // future must never discard the durable audit, recovery ids or event.
        Ok(Json(finalize_restore(&state, run_id, cp_arg, actor, force, report)))
    }).await.map_err(ApiError::internal)?
}

fn finalize_restore(
    s: &AppState,
    run_id: RunId,
    cp: String,
    actor: crate::live_view::Actor,
    force: bool,
    report: builtin_tools::RestoreReport,
) -> RestoreResponseDto {
    let builtin_tools::RestoreReport {
        restored,
        safety_checkpoint_ids: safety_ids,
        mut warnings,
        partial,
        error,
    } = report;

    let detail = serde_json::json!({
        "checkpoint_id": cp,
        "safety_checkpoint_ids": safety_ids,
        "restored": restored,
        "warnings": warnings,
        "force": force,
        "partial": partial,
        "error": error,
        "actor": {"key": actor.key, "local": actor.local},
    });
    let audit = s.storage().append_audit(
        s.user_id(),
        Some(run_id),
        &actor.key,
        "checkpoint.restored",
        &detail,
    );

    let safety_checkpoint_id = safety_ids.first().cloned().unwrap_or_default();
    s.agent().emit_run_event(
        run_id,
        Event::CheckpointRestored(CheckpointRestored {
            run_id,
            checkpoint_id: cp.clone(),
            safety_checkpoint_id,
            restored: restored.clone(),
            partial,
        }),
    );

    if let Err(error) = audit {
        // Never discard recovery ids because the disk/audit sink failed after
        // mutation. The durable start row records the attempt; return recovery.
        warnings.push(format!(
            "restore audit completion could not be written: {error}"
        ));
    }
    let (covered, not_covered) = scope_lists();
    RestoreResponseDto {
        partial,
        error,
        run_id: run_id.0,
        checkpoint_id: cp,
        safety_checkpoint_ids: safety_ids,
        restored,
        warnings,
        covered,
        not_covered,
    }
}

#[cfg(test)]
pub(crate) struct TestRestorePause {
    pub entered: tokio::sync::oneshot::Sender<()>,
    pub release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use agent_core::{AgentLoop, LoopConfig, Providers};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use protocol::{BotId, ChatId, RunId};
    use serde_json::{Value, json};
    use storage::{Bot, Chat, Run, Storage, now_ms, read_event_log};
    use task_runtime::RunState;
    use tool_registry::ToolRegistry;
    use tower::ServiceExt;

    use super::*;
    use crate::{DaemonConfig, router};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Harness {
        app: Router,
        state: AppState,
        dir: tempfile::TempDir,
        work: tempfile::TempDir,
        run_id: RunId,
        other_run_id: RunId,
    }

    fn setup_harness() -> Harness {
        setup_harness_with_config(LoopConfig::default())
    }

    fn setup_harness_with_config(config: LoopConfig) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(dir.path()).unwrap());
        let agent = AgentLoop::new(
            Arc::clone(&storage),
            ToolRegistry::new(),
            Providers::unconfigured(),
            config,
        );
        let state = AppState::new(agent, Arc::clone(&storage), DaemonConfig::new(TOKEN)).unwrap();

        let user_id = state.user_id();
        let bot_id = BotId::new();
        let chat_id = ChatId::new();
        storage
            .insert_bot(&Bot {
                id: bot_id,
                user_id,
                name: "tester".into(),
                config: json!({}),
                created_at_ms: now_ms(),
            })
            .unwrap();
        storage
            .insert_chat(&Chat {
                id: chat_id,
                user_id,
                bot_id,
                title: "chat".into(),
                created_at_ms: now_ms(),
            })
            .unwrap();

        let run_id = RunId::new();
        storage
            .insert_run(&Run {
                id: run_id,
                user_id,
                bot_id,
                chat_id: Some(chat_id),
                parent_run_id: None,
                kind: "turn".into(),
                spec: None,
                state: RunState::Succeeded.as_str().into(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                error: None,
                computer_target: None,
                computer_node: None,
                computer_node_name: None,
                paused_at_ms: None,
                pause_reason: None,
            })
            .unwrap();

        let other_run_id = RunId::new();
        storage
            .insert_run(&Run {
                id: other_run_id,
                user_id,
                bot_id,
                chat_id: Some(chat_id),
                parent_run_id: None,
                kind: "turn".into(),
                spec: None,
                state: RunState::Succeeded.as_str().into(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                error: None,
                computer_target: None,
                computer_node: None,
                computer_node_name: None,
                paused_at_ms: None,
                pause_reason: None,
            })
            .unwrap();

        Harness {
            app: router(state.clone()),
            state,
            dir,
            work,
            run_id,
            other_run_id,
        }
    }

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
        with_auth: bool,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:47821");
        if with_auth {
            builder = builder.header("authorization", format!("Bearer {TOKEN}"));
        }
        let req = match body {
            Some(v) => builder
                .header("content-type", "application/json")
                .body(Body::from(v.to_string())),
            None => builder.body(Body::empty()),
        }
        .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    fn install_access(
        h: &Harness,
        roots: Vec<std::path::PathBuf>,
        grants: Vec<std::path::PathBuf>,
    ) {
        struct UnusedGate;
        #[async_trait::async_trait]
        impl builtin_tools::ToolGate for UnusedGate {
            async fn authorize(&self, _: &builtin_tools::GateRequest) -> Result<(), String> {
                Err("unused".into())
            }
        }
        let mut config =
            builtin_tools::BuiltinConfig::new(h.dir.path().to_path_buf(), Arc::new(UnusedGate));
        config.fs_allowed_roots = roots;
        config.fs_granted_paths = grants;
        for name in [
            "fs_read",
            "fs_write",
            "fs_edit",
            "fs_move",
            "fs_delete",
            "fs_list",
            "fs_search",
            "checkpoint_list",
            "checkpoint_restore",
        ] {
            h.state.agent().tool_registry().unregister(name);
        }
        let _ = h.state.agent().clone().with_builtin_tools(config);
    }

    fn edit(h: &Harness, run: RunId, name: &str) -> (std::path::PathBuf, String) {
        let path = h.work.path().join(name);
        fs::write(&path, "before").unwrap();
        let store = CheckpointStore::new(h.dir.path());
        let cp = store
            .create(&run.0.to_string(), "fs_edit", &[&path])
            .unwrap();
        fs::write(&path, "after").unwrap();
        store.record_after(&cp).unwrap();
        (path, cp)
    }

    #[tokio::test]
    async fn disconnected_restore_still_finalizes_audit_event_and_recovery() {
        let h = setup_harness();
        let (path, cp) = edit(&h, h.run_id, "disconnect.txt");
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *h.state.0.restore_pause.lock() = Some(TestRestorePause {
            entered,
            release: resume,
        });
        let app = h.app.clone();
        let url = format!("/v1/runs/{}/changes/all/restore", h.run_id);
        let request = tokio::spawn(async move { call(&app, Method::POST, &url, None, true).await });
        ready.await.unwrap(); // Safety snapshot exists; mutation is still in flight.
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !h.state.0.restore_locks.lock().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "before");
        let audits = h.state.storage().list_audit(h.state.user_id()).unwrap();
        let completed = audits
            .iter()
            .find(|a| a.action == "checkpoint.restored")
            .unwrap();
        let safety = completed.detail["safety_checkpoint_ids"][0]
            .as_str()
            .unwrap();
        let checkpoints = CheckpointStore::new(h.dir.path())
            .list(&h.run_id.0.to_string())
            .unwrap();
        assert!(
            checkpoints
                .iter()
                .any(|c| c.id == safety && c.op == "pre-restore:all")
        );
        let log = read_event_log(h.dir.path(), h.run_id).unwrap();
        assert!(
            log.iter()
                .any(|e| matches!(&e.event, Event::CheckpointRestored(ev)
            if ev.checkpoint_id == "all" && ev.safety_checkpoint_id == safety))
        );
        assert_eq!(completed.detail["checkpoint_id"], "all");
        assert!(checkpoints.iter().any(|c| c.id == cp));
    }

    #[tokio::test]
    async fn audit_completion_failure_returns_recovery_ids_and_failed_start_mutates_nothing() {
        let h = setup_harness();
        let (path, _) = edit(&h, h.run_id, "audit.txt");
        let conn = rusqlite::Connection::open(h.dir.path().join("bot.db")).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_restore_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'checkpoint.restored' BEGIN SELECT RAISE(FAIL, 'injected completion failure'); END;").unwrap();
        let url = format!("/v1/runs/{}/changes/all/restore", h.run_id);
        let (status, body) = call(&h.app, Method::POST, &url, None, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fs::read_to_string(&path).unwrap(), "before");
        assert_ne!(body["safety_checkpoint_ids"][0].as_str().unwrap(), "");
        assert!(
            body["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("audit completion"))
        );
        assert!(
            h.state
                .storage()
                .list_audit(h.state.user_id())
                .unwrap()
                .iter()
                .any(|a| a.action == "checkpoint.restore_started")
        );
        conn.execute_batch("DROP TRIGGER fail_restore_audit; CREATE TRIGGER fail_start BEFORE INSERT ON audit_log BEGIN SELECT RAISE(FAIL, 'injected start failure'); END;").unwrap();
        fs::write(&path, "user new work").unwrap();
        let (status, _) = call(
            &h.app,
            Method::POST,
            &url,
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(fs::read_to_string(&path).unwrap(), "user new work");
    }

    #[tokio::test]
    async fn changes_http_paginates_without_losing_remaining_files() {
        let h = setup_harness();
        for name in ["a.txt", "b.txt", "c.txt"] {
            edit(&h, h.run_id, name);
        }
        let base = format!("/v1/runs/{}/changes", h.run_id);
        let (status, first) =
            call(&h.app, Method::GET, &format!("{base}?limit=2"), None, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["changes"].as_array().unwrap().len(), 2);
        assert_eq!(first["next_offset"], 2);
        let (status, second) = call(
            &h.app,
            Method::GET,
            &format!("{base}?limit=2&offset=2"),
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(second["changes"].as_array().unwrap().len(), 1);
        assert!(second["next_offset"].is_null());
        assert_ne!(first["changes"][0]["path"], second["changes"][0]["path"]);
    }

    #[tokio::test]
    async fn restore_all_injected_failure_rolls_back_and_audits_partial_with_safety() {
        let h = setup_harness();
        let (a, _) = edit(&h, h.run_id, "a.txt");
        let (b, _) = edit(&h, h.run_id, "b.txt");
        fs::write(&a, "user a").unwrap();
        *h.state.0.restore_fail_at.lock() = Some(1);
        let (status, body) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{}/changes/all/restore", h.run_id),
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["partial"], true);
        assert_eq!(body["restored"], json!([]));
        assert_eq!(fs::read_to_string(&a).unwrap(), "user a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "after");
        let safety = body["safety_checkpoint_ids"][0].as_str().unwrap();
        assert_ne!(safety, "");
        let audits = h.state.storage().list_audit(h.state.user_id()).unwrap();
        let audit = audits
            .iter()
            .find(|a| a.action == "checkpoint.restored")
            .unwrap();
        assert_eq!(audit.detail["partial"], true);
        assert_eq!(audit.detail["safety_checkpoint_ids"][0], safety);
        let log = read_event_log(h.dir.path(), h.run_id).unwrap();
        assert!(
            matches!(&log.last().unwrap().event, Event::CheckpointRestored(e) if e.partial && e.safety_checkpoint_id == safety)
        );
    }

    #[tokio::test]
    async fn restore_all_http_requires_force_and_preserves_recreated_moved_and_cross_run_files() {
        let h = setup_harness();
        let store = CheckpointStore::new(h.dir.path());
        let run = h.run_id.0.to_string();
        let deleted = h.work.path().join("deleted.txt");
        fs::write(&deleted, "original deleted").unwrap();
        let cp = store.create(&run, "fs_delete", &[&deleted]).unwrap();
        fs::remove_file(&deleted).unwrap();
        store.record_after(&cp).unwrap();
        fs::write(&deleted, "user recreated").unwrap();
        let (moved, _) = edit(&h, h.run_id, "moved.txt");
        let user_destination = h.work.path().join("user-move.txt");
        fs::rename(&moved, &user_destination).unwrap();
        let (cross, _) = edit(&h, h.run_id, "cross.txt");
        let cp2 = store
            .create(&h.other_run_id.0.to_string(), "fs_edit", &[&cross])
            .unwrap();
        fs::write(&cross, "second run").unwrap();
        store.record_after(&cp2).unwrap();
        let url = format!("/v1/runs/{run}/changes/all/restore");
        // Empty POST body is force=false, including when edits exist.
        let (status, body) = call(&h.app, Method::POST, &url, None, true).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "modified_since");
        assert_eq!(fs::read_to_string(&deleted).unwrap(), "user recreated");
        assert_eq!(fs::read_to_string(&cross).unwrap(), "second run");
        assert!(!moved.exists());
        let (_, preview) = call(
            &h.app,
            Method::GET,
            &format!("/v1/runs/{run}/changes/all/preview"),
            None,
            true,
        )
        .await;
        assert_eq!(preview["modified_since"].as_array().unwrap().len(), 3);
        assert!(
            preview["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["changed_by_run"] == h.other_run_id.0.to_string())
        );
        let (status, body) = call(
            &h.app,
            Method::POST,
            &url,
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["partial"], false);
        assert_eq!(fs::read_to_string(&deleted).unwrap(), "original deleted");
        assert_eq!(fs::read_to_string(&cross).unwrap(), "before");
        assert_eq!(fs::read_to_string(&moved).unwrap(), "before");
        assert_eq!(fs::read_to_string(&user_destination).unwrap(), "after");
        let safety = body["safety_checkpoint_ids"][0].as_str().unwrap();
        // The same authenticated route can undo the undo, with its own preview.
        let (status, _) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{run}/changes/{safety}/restore"),
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fs::read_to_string(&deleted).unwrap(), "user recreated");
        assert_eq!(fs::read_to_string(&cross).unwrap(), "second run");
        assert!(!moved.exists());
    }

    #[tokio::test]
    async fn current_roots_and_exact_grants_degrade_only_denied_entries_and_redact_allowed_diffs() {
        let redactor = agent_core::SharedRedactor::new(secret_store::Redactor::new());
        redactor.register("fixture-production-secret-123456");
        let config = LoopConfig {
            redactor: Some(redactor),
            ..LoopConfig::default()
        };
        let h = setup_harness_with_config(config);
        let run = h.run_id.0.to_string();
        let store = CheckpointStore::new(h.dir.path());
        let (ordinary, _) = edit(&h, h.run_id, "ordinary.txt");
        let outside = tempfile::tempdir().unwrap();
        let excluded = outside.path().join("excluded.txt");
        fs::write(&excluded, "before excluded").unwrap();
        let cp = store.create(&run, "fs_edit", &[&excluded]).unwrap();
        fs::write(&excluded, "after excluded").unwrap();
        store.record_after(&cp).unwrap();
        let secret = h.work.path().join(".env");
        let roots = vec![h.work.path().to_path_buf()];
        let grants = vec![secret.clone()];
        builtin_tools::with_checkpoint_access(h.dir.path(), &roots, &grants, || {
            fs::write(&secret, "TOKEN=old").unwrap();
            let cp = store.create(&run, "fs_edit", &[&secret]).unwrap();
            fs::write(&secret, "TOKEN=fixture-production-secret-123456").unwrap();
            store.record_after(&cp).unwrap();
        });
        install_access(&h, roots.clone(), Vec::new());
        let list_url = format!("/v1/runs/{run}/changes");
        let (status, body) = call(&h.app, Method::GET, &list_url, None, true).await;
        assert_eq!(status, StatusCode::OK);
        let entries = body["changes"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries.iter().filter(|e| e["accessible"] == false).count(),
            2
        );
        assert!(
            entries
                .iter()
                .filter(|e| e["accessible"] == false)
                .all(|e| e["text"].is_null() && e["diff_omitted_reason"] == "not_accessible")
        );
        install_access(&h, roots, grants);
        let (status, body) = call(&h.app, Method::GET, &list_url, None, true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body
                .to_string()
                .contains("fixture-production-secret-123456")
        );
        let env = body["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["path"].as_str().is_some_and(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .is_some_and(|name| name == ".env")
                })
            })
            .unwrap();
        assert_eq!(env["accessible"], true);
        assert!(
            env["text"]["unified"]
                .as_str()
                .unwrap()
                .contains(secret_store::REDACTED)
        );
        let (status, body) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{run}/changes/all/restore"),
            Some(json!({"force": true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fs::read_to_string(&ordinary).unwrap(), "before");
        assert_eq!(fs::read_to_string(&excluded).unwrap(), "after excluded");
        assert_eq!(fs::read_to_string(&secret).unwrap(), "TOKEN=old");
        assert!(
            body["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains("not accessible"))
        );
    }

    #[tokio::test]
    async fn restore_refuses_paused_other_active_run_and_double_restore() {
        let h = setup_harness();
        let (path, _) = edit(&h, h.run_id, "busy.txt");
        let url = format!("/v1/runs/{}/changes/all/restore", h.run_id);
        for state in [
            RunState::Paused,
            RunState::Queued,
            RunState::Running,
            RunState::WaitingApproval,
        ] {
            h.state
                .storage()
                .update_run_state(h.run_id, state.as_str(), None)
                .unwrap();
            assert_eq!(
                call(
                    &h.app,
                    Method::POST,
                    &url,
                    Some(json!({"force":true})),
                    true
                )
                .await
                .0,
                StatusCode::CONFLICT
            );
        }
        h.state
            .storage()
            .update_run_state(h.run_id, RunState::Succeeded.as_str(), None)
            .unwrap();
        h.state
            .storage()
            .update_run_state(h.other_run_id, RunState::Running.as_str(), None)
            .unwrap();
        assert_eq!(
            call(
                &h.app,
                Method::POST,
                &url,
                Some(json!({"force":true})),
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        h.state
            .storage()
            .update_run_state(h.other_run_id, RunState::Succeeded.as_str(), None)
            .unwrap();
        // Hold the exact production admission guard to deterministically model
        // the first request paused between preview, hash check and mutation.
        let lock = RestoreLock::acquire(&h.state, h.run_id).unwrap();
        let (status, body) = call(
            &h.app,
            Method::POST,
            &url,
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "restore_in_progress");
        assert_eq!(fs::read_to_string(&path).unwrap(), "after");
        drop(lock);
        assert_eq!(
            call(&h.app, Method::POST, &url, None, true).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn changes_reject_traversal_and_scope_phone_node_host_peers() {
        let h = setup_harness();
        let (_, cp) = edit(&h, h.run_id, "peer.txt");
        let run = h.run_id;
        for bad in ["..", "%2e%2e", "cp-..", "%2e%2e%5coutside"] {
            for suffix in ["preview", "restore"] {
                let method = if suffix == "preview" {
                    Method::GET
                } else {
                    Method::POST
                };
                let (status, _) = call(
                    &h.app,
                    method,
                    &format!("/v1/runs/{run}/changes/{bad}/{suffix}"),
                    None,
                    true,
                )
                .await;
                assert_eq!(status, StatusCode::NOT_FOUND);
            }
        }
        for role in [
            crate::PeerRole::Desktop,
            crate::PeerRole::Phone,
            crate::PeerRole::Node,
            crate::PeerRole::Host,
        ] {
            let app =
                crate::peer_router(h.state.clone()).layer(axum::Extension(crate::PeerIdentity {
                    key: "fixture-peer".into(),
                    role,
                }));
            for (method, suffix) in [
                (Method::GET, String::new()),
                (Method::GET, format!("/{cp}/preview")),
                (Method::POST, format!("/{cp}/restore")),
            ] {
                let (status, _) = call(
                    &app,
                    method,
                    &format!("/v1/runs/{run}/changes{suffix}"),
                    None,
                    false,
                )
                .await;
                assert_eq!(
                    status,
                    if role == crate::PeerRole::Desktop {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    },
                    "role {role:?}"
                );
            }
        }
        let audits = h.state.storage().list_audit(h.state.user_id()).unwrap();
        assert!(audits.iter().any(|a| a.action == "checkpoint.restored"
            && a.detail["actor"]["key"] == "peer:fixture-peer"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_swap_is_not_followed_or_overwritten_by_restore_route() {
        let h = setup_harness();
        let (path, cp) = edit(&h, h.run_id, "link.txt");
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("private.txt");
        fs::write(&target, "untouched").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        install_access(&h, vec![h.work.path().to_path_buf()], Vec::new());
        let (status, body) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{}/changes/{cp}/restore", h.run_id),
            Some(json!({"force":true})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["restored"], json!([]));
        assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn changes_routes_enforce_auth_ownership_modified_since_audit_and_event() {
        let h = setup_harness();
        let store = CheckpointStore::new(h.dir.path());
        let target = h.work.path().join("notes.txt");
        fs::write(&target, "before line\n").unwrap();

        let run_str = h.run_id.0.to_string();
        let cp = store.create(&run_str, "fs_edit", &[&target]).unwrap();
        fs::write(&target, "after line\n").unwrap();
        store.record_after(&cp).unwrap();

        // Unauthenticated requests are rejected cleanly.
        let (st, _) = call(
            &h.app,
            Method::GET,
            &format!("/v1/runs/{run_str}/changes"),
            None,
            false,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // Wrong run id cannot preview or restore another run's checkpoint.
        let other_str = h.other_run_id.0.to_string();
        let (st, _) = call(
            &h.app,
            Method::GET,
            &format!("/v1/runs/{other_str}/changes/{cp}/preview"),
            None,
            true,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Listing changes shows unified diff and scope disclaimer.
        let (st, list_body) = call(
            &h.app,
            Method::GET,
            &format!("/v1/runs/{run_str}/changes"),
            None,
            true,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list_body["changes"].as_array().unwrap().len(), 1);
        assert!(
            list_body["changes"][0]["text"]["unified"]
                .as_str()
                .unwrap()
                .contains("+after line")
        );
        assert_eq!(
            list_body["not_covered"],
            json!(["shell", "web", "email", "desktop"])
        );

        // Simulate external post-run user edit.
        fs::write(&target, "user newer edit\n").unwrap();

        // Preview flags modified_since.
        let (st, preview) = call(
            &h.app,
            Method::GET,
            &format!("/v1/runs/{run_str}/changes/{cp}/preview"),
            None,
            true,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(preview["modified_since"].as_array().unwrap().len(), 1);
        assert_eq!(preview["entries"][0]["current_matches_after"], false);
        assert_eq!(preview["entries"][0]["action"], "restore");

        // Restore without force=true returns 409 modified_since and leaves file untouched.
        let (st, err_body) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{run_str}/changes/{cp}/restore"),
            Some(json!({ "force": false })),
            true,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err_body["error"]["code"], "modified_since");
        assert_eq!(fs::read_to_string(&target).unwrap(), "user newer edit\n");

        // Restore with force=true restores content, writes safety snapshot, audit record, and event.
        let (st, restored) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{run_str}/changes/{cp}/restore"),
            Some(json!({ "force": true })),
            true,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(fs::read_to_string(&target).unwrap(), "before line\n");
        let safety_id = restored["safety_checkpoint_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(safety_id.starts_with("cp-"));

        let audits = h.state.storage().list_audit(h.state.user_id()).unwrap();
        let audit_entry = audits
            .iter()
            .find(|a| a.action == "checkpoint.restored")
            .expect("checkpoint.restored audit row written");
        assert_eq!(audit_entry.detail["checkpoint_id"], cp);

        let log = read_event_log(h.dir.path(), h.run_id).unwrap();
        assert!(matches!(
            &log.last().unwrap().event,
            Event::CheckpointRestored(ev)
                if ev.checkpoint_id == cp && ev.safety_checkpoint_id == safety_id
        ));

        // Restoring the safety snapshot recovers the user's pre-undo content.
        store.restore(&safety_id).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "user newer edit\n");
    }

    #[tokio::test]
    async fn restore_all_is_refused_while_run_is_active_and_succeeds_after_finish() {
        let h = setup_harness();
        let active_run = RunId::new();
        let user_id = h.state.user_id();
        let bot_id = BotId::new();
        h.state
            .storage()
            .insert_bot(&Bot {
                id: bot_id,
                user_id,
                name: "b2".into(),
                config: json!({}),
                created_at_ms: now_ms(),
            })
            .unwrap();
        h.state
            .storage()
            .insert_run(&Run {
                id: active_run,
                user_id,
                bot_id,
                chat_id: None,
                parent_run_id: None,
                kind: "turn".into(),
                spec: None,
                state: RunState::Running.as_str().into(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                error: None,
                computer_target: None,
                computer_node: None,
                computer_node_name: None,
                paused_at_ms: None,
                pause_reason: None,
            })
            .unwrap();

        let active_str = active_run.0.to_string();
        let (st, _) = call(
            &h.app,
            Method::POST,
            &format!("/v1/runs/{active_str}/changes/all/restore"),
            Some(json!({ "force": false })),
            true,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }
}
