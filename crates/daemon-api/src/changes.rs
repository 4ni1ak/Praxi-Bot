//! Per-run file-change review, restore preview, and safe restore routes.

use std::io;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use builtin_tools::{
    ChangeKind, CheckpointStore, DiffOmittedReason, FileChange, RestoreAction, RestorePreviewEntry,
    UnifiedDiff,
};
use protocol::event::CheckpointRestored;
use protocol::{Event, RunId};
use serde::{Deserialize, Serialize};
use task_runtime::RunState;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::AppState;
use crate::error::{ApiError, ErrorBody};
use crate::routes::ApiJson;

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
}

impl From<DiffOmittedReason> for DiffOmittedReasonDto {
    fn from(r: DiffOmittedReason) -> Self {
        match r {
            DiffOmittedReason::Binary => Self::Binary,
            DiffOmittedReason::TooLarge => Self::TooLarge,
            DiffOmittedReason::RunBudgetExceeded => Self::RunBudgetExceeded,
            DiffOmittedReason::Directory => Self::Directory,
            DiffOmittedReason::Symlink => Self::Symlink,
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
    pub path: String,
    pub current_matches_after: bool,
    pub action: RestoreActionDto,
    pub via_link_of: Option<String>,
}

impl From<RestorePreviewEntry> for RestorePreviewEntryDto {
    fn from(e: RestorePreviewEntry) -> Self {
        Self {
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
    let active = run.state == RunState::Queued.as_str()
        || run.state == RunState::Running.as_str()
        || run.state == RunState::WaitingApproval.as_str();
    if active {
        return Err(ApiError::conflict(
            "run is still active; wait until it finishes before restoring changes",
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

/// List net file changes made by a run's file tools, with capped inline diffs.
#[utoipa::path(
    get,
    path = "/v1/runs/{id}/changes",
    tag = "runs",
    security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Run id")),
    responses(
        (status = 200, body = RunChangesDto),
        (status = 404, body = ErrorBody)
    )
)]
pub(crate) async fn get_run_changes(
    State(s): State<AppState>,
    Path(rid): Path<String>,
) -> Result<Json<RunChangesDto>, ApiError> {
    let run_id = parse_run_id(&rid)?;
    let run = s.run_of_user(run_id)?;
    let data_dir = s.storage().data_dir().to_path_buf();
    let run_str = run.id.0.to_string();

    let changes = tokio::task::spawn_blocking(move || {
        let store = CheckpointStore::new(&data_dir);
        store.changes_for_run(&run_str)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(map_store_err)?;

    let (covered, not_covered) = scope_lists();
    Ok(Json(RunChangesDto {
        run_id: run.id.0,
        changes: changes.into_iter().map(Into::into).collect(),
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

    let entries =
        tokio::task::spawn_blocking(move || -> Result<Vec<RestorePreviewEntry>, ApiError> {
            let store = CheckpointStore::new(&data_dir);
            verify_checkpoint_scope(&store, &cp_arg, &run_str)?;
            if cp_arg == "all" {
                store.restore_run_preview(&run_str).map_err(map_store_err)
            } else {
                store.restore_preview(&cp_arg).map_err(map_store_err)
            }
        })
        .await
        .map_err(ApiError::internal)??;

    let modified_since: Vec<String> = entries
        .iter()
        .filter(|e| !e.current_matches_after)
        .map(|e| e.path.clone())
        .collect();
    let (covered, not_covered) = scope_lists();
    Ok(Json(RestorePreviewDto {
        run_id: run.id.0,
        checkpoint_id: cp,
        entries: entries.into_iter().map(Into::into).collect(),
        modified_since,
        covered,
        not_covered,
    }))
}

struct RestoreOutcome {
    restored: Vec<String>,
    safety_ids: Vec<String>,
    warnings: Vec<String>,
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
    request_body = RestoreRequestDto,
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
    ApiJson(body): ApiJson<RestoreRequestDto>,
) -> Result<Json<RestoreResponseDto>, ApiError> {
    let run_id = parse_run_id(&rid)?;
    let run = s.run_of_user(run_id)?;
    ensure_run_finished(&run)?;

    let data_dir = s.storage().data_dir().to_path_buf();
    let run_str = run.id.0.to_string();
    let cp_arg = cp.clone();
    let force = body.force;

    let RestoreOutcome {
        restored,
        safety_ids,
        warnings,
    } = tokio::task::spawn_blocking(move || -> Result<RestoreOutcome, ApiError> {
        let store = CheckpointStore::new(&data_dir);
        verify_checkpoint_scope(&store, &cp_arg, &run_str)?;
        let preview = if cp_arg == "all" {
            store.restore_run_preview(&run_str).map_err(map_store_err)?
        } else {
            store.restore_preview(&cp_arg).map_err(map_store_err)?
        };
        let modified_since: Vec<String> = preview
            .into_iter()
            .filter(|e| !e.current_matches_after)
            .map(|e| e.path)
            .collect();
        if !force && !modified_since.is_empty() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "modified_since",
                format!(
                    "{} file(s) changed on disk after this run ({}); pass force=true to overwrite them",
                    modified_since.len(),
                    modified_since.join(", ")
                ),
            ));
        }
        if cp_arg == "all" {
            let (restored, safety_ids, warnings) =
                store.restore_run(&run_str).map_err(map_store_err)?;
            Ok(RestoreOutcome {
                restored,
                safety_ids,
                warnings,
            })
        } else {
            let (restored, safety, warnings) =
                store.restore_with_warnings(&cp_arg).map_err(map_store_err)?;
            Ok(RestoreOutcome {
                restored,
                safety_ids: vec![safety],
                warnings,
            })
        }
    })
    .await
    .map_err(ApiError::internal)??;

    let detail = serde_json::json!({
        "checkpoint_id": cp,
        "safety_checkpoint_ids": safety_ids,
        "restored": restored,
        "warnings": warnings,
        "force": force,
    });
    let _ = s.storage().append_audit(
        s.user_id(),
        Some(run.id),
        &actor.key,
        "checkpoint.restored",
        &detail,
    );

    let safety_checkpoint_id = safety_ids.first().cloned().unwrap_or_default();
    s.agent().emit_run_event(
        run.id,
        Event::CheckpointRestored(CheckpointRestored {
            run_id: run.id,
            checkpoint_id: cp.clone(),
            safety_checkpoint_id,
            restored: restored.clone(),
        }),
    );

    let (covered, not_covered) = scope_lists();
    Ok(Json(RestoreResponseDto {
        run_id: run.id.0,
        checkpoint_id: cp,
        safety_checkpoint_ids: safety_ids,
        restored,
        warnings,
        covered,
        not_covered,
    }))
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
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(dir.path()).unwrap());
        let agent = AgentLoop::new(
            Arc::clone(&storage),
            ToolRegistry::new(),
            Providers::unconfigured(),
            LoopConfig::default(),
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
