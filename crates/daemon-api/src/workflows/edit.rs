//! Owner-only editing, duplicating, exporting, importing and run history of
//! saved workflows.
//!
//! Security shape: an edit can only keep, reorder or drop steps that already
//! exist and add `wait_for` steps; it can never author a command, a tool call
//! or an agent instruction. A file import is untrusted input: it is size
//! capped, parsed strictly, scanned for credentials and always saved as
//! "carries outside content" so replay applies the tainted-egress rules.

use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use protocol::{BotId, Event};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;
use uuid::Uuid;
use workflow_replay::{
    Param, Workflow,
    format::{Step, SuccessCheck, WaitFor, WorkflowSource, valid_name},
    store::{SaveError, SaveMode},
};

use super::{WorkflowDto, WorkflowScope, privacy, store, store_error};
use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

/// Largest import file accepted, in bytes.
pub(crate) const MAX_IMPORT_BYTES: usize = 256 * 1024;
const MAX_STEPS: usize = 200;
const MAX_PARAMS: usize = 50;
const MAX_TEXT: usize = 2000;
/// Newest runs scanned for history; older ones are not shown.
const HISTORY_RUNS: usize = 60;
const HISTORY_SHOWN: usize = 15;
/// Top-level keys of a workflow file; anything else is refused.
const FILE_KEYS: [&str; 8] = [
    "version",
    "revision",
    "name",
    "description",
    "params",
    "steps",
    "source",
    "success_check",
];

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditParam {
    name: String,
    /// Absent means "ask every time". A parameter that had no default cannot
    /// gain one here (it may be sensitive).
    default: Option<String>,
    description: String,
}

/// Wait step from the palette. Exactly one condition.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditWait {
    /// Page contains this text.
    text: Option<String>,
    /// A window title matches.
    window_title: Option<String>,
    /// Page network is idle.
    #[serde(default)]
    idle: bool,
    timeout_secs: Option<u64>,
}

/// One step of the edited list: an existing step by its current index, or a
/// new wait step.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditStep {
    keep: Option<usize>,
    wait: Option<EditWait>,
}

/// Final check after the last step ("assert").
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditCheck {
    /// `window_title`, `browser_text` or `desktop_text`.
    kind: String,
    text: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditWorkflow {
    bot_id: Uuid,
    /// Revision the editor started from; a stale edit is refused.
    revision: u64,
    /// New name (a rename keeps the old file's content under the new name).
    name: String,
    description: String,
    params: Vec<EditParam>,
    steps: Vec<EditStep>,
    success_check: Option<EditCheck>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DuplicateWorkflow {
    bot_id: Uuid,
    new_name: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportWorkflow {
    bot_id: Uuid,
    /// Contents of a workflow file (JSON), at most 256 KiB.
    content: String,
    /// Save under this name instead of the file's own.
    name: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WorkflowExport {
    file_name: String,
    content: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct HistoryStep {
    step_index: u32,
    kind: String,
    /// `ok`, `failed` or `interrupted`.
    status: String,
    reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WorkflowRunRecord {
    run_id: Uuid,
    state: String,
    started_at_ms: i64,
    steps: Vec<HistoryStep>,
}

fn long(text: &str) -> bool {
    text.chars().count() > MAX_TEXT
}

fn bad(message: &str) -> ApiError {
    ApiError::bad_request(message)
}

fn save_error(error: SaveError) -> ApiError {
    match error {
        SaveError::AlreadyExists(_) => {
            ApiError::conflict("a workflow with this name already exists")
        }
        SaveError::Conflict { .. } => ApiError::conflict(
            "this workflow changed since you opened it; reload it and edit again",
        ),
        SaveError::Store(error) => store_error(error),
        other @ SaveError::RevisionExhausted(_) => ApiError::internal(other),
    }
}

fn wait_step(wait: &EditWait) -> Result<WaitFor, ApiError> {
    let set = usize::from(wait.text.is_some())
        + usize::from(wait.window_title.is_some())
        + usize::from(wait.idle);
    if set != 1 {
        return Err(bad("a wait step needs exactly one condition"));
    }
    if wait
        .text
        .as_deref()
        .is_some_and(|t| t.trim().is_empty() || long(t))
        || wait
            .window_title
            .as_deref()
            .is_some_and(|t| t.trim().is_empty() || long(t))
    {
        return Err(bad("wait text is empty or too long"));
    }
    if wait.timeout_secs.is_some_and(|s| s == 0 || s > 600) {
        return Err(bad("a wait can last 1 to 600 seconds"));
    }
    Ok(WaitFor {
        window_title_pattern: wait.window_title.clone(),
        text: wait.text.clone(),
        selector: None,
        idle: wait.idle,
        timeout_secs: wait.timeout_secs,
    })
}

fn success_check(check: &EditCheck) -> Result<SuccessCheck, ApiError> {
    if check.text.trim().is_empty() || long(&check.text) {
        return Err(bad("the final check text is empty or too long"));
    }
    match check.kind.as_str() {
        "window_title" => Ok(SuccessCheck::WindowTitle {
            pattern: check.text.clone(),
        }),
        "browser_text" => Ok(SuccessCheck::BrowserText {
            contains: check.text.clone(),
        }),
        "desktop_text" => Ok(SuccessCheck::DesktopText {
            window_title_pattern: None,
            contains: check.text.clone(),
        }),
        _ => Err(bad("unknown final check")),
    }
}

/// Build the edited workflow from `old` and the request. Pure, so it is easy
/// to test and cannot touch the disk.
fn apply_edit(old: &Workflow, input: &EditWorkflow) -> Result<Workflow, ApiError> {
    if !valid_name(&input.name) {
        return Err(bad(
            "names use lowercase letters, digits, - and _ (up to 64)",
        ));
    }
    if input.steps.is_empty() || input.steps.len() > MAX_STEPS {
        return Err(bad("a workflow needs between 1 and 200 steps"));
    }
    if long(&input.description) {
        return Err(bad("description is too long"));
    }
    if input.params.len() != old.params.len() {
        return Err(bad("parameters cannot be added or removed here"));
    }
    let mut params: Vec<Param> = Vec::new();
    for old_param in &old.params {
        let new = input
            .params
            .iter()
            .find(|p| p.name == old_param.name)
            .ok_or_else(|| bad("parameters cannot be added or removed here"))?;
        if new.default.is_some() && old_param.default.is_none() {
            return Err(bad(
                "a value that is asked for every time cannot get a saved default",
            ));
        }
        if long(&new.description) || new.default.as_deref().is_some_and(long) {
            return Err(bad("parameter text is too long"));
        }
        params.push(Param {
            default: new.default.clone(),
            description: new.description.clone(),
            ..old_param.clone()
        });
    }
    let origins = old.source.as_ref().map(|s| s.step_origins.clone());
    let mut seen = BTreeSet::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut new_origins: Vec<String> = Vec::new();
    for item in &input.steps {
        match (item.keep, &item.wait) {
            (Some(index), None) => {
                let step = old.steps.get(index).ok_or_else(|| bad("unknown step"))?;
                if !seen.insert(index) {
                    return Err(bad("a step appears twice"));
                }
                steps.push(step.clone());
                new_origins.push(
                    origins
                        .as_ref()
                        .and_then(|o| o.get(index).cloned())
                        .unwrap_or_else(|| "run_content_or_model".to_owned()),
                );
            }
            (None, Some(wait)) => {
                steps.push(Step::WaitFor(wait_step(wait)?));
                new_origins.push("user_request".to_owned());
            }
            _ => return Err(bad("each step is either kept or a new wait")),
        }
    }
    let mut wf = old.clone();
    wf.name.clone_from(&input.name);
    wf.description.clone_from(&input.description);
    wf.params = params;
    wf.steps = steps;
    wf.success_check = input
        .success_check
        .as_ref()
        .map(success_check)
        .transpose()?;
    if let Some(source) = wf.source.as_mut() {
        source.step_origins = new_origins;
    }
    wf.validate().map_err(|e| bad(&e.to_string()))?;
    Ok(wf)
}

fn reject_secrets(s: &AppState, wf: &Workflow) -> Result<(), ApiError> {
    if s.agent()
        .workflow_contains_secret(&serde_json::to_string(wf).map_err(ApiError::internal)?)
    {
        return Err(ApiError::conflict(
            "this workflow would contain a credential; remove it first",
        ));
    }
    Ok(())
}

#[utoipa::path(put, operation_id = "edit_workflow", path = "/v1/workflows/{name}", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path)), request_body = EditWorkflow,
    responses((status = 200, body = WorkflowDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn edit(
    State(s): State<AppState>,
    Path(name): Path<String>,
    ApiJson(input): ApiJson<EditWorkflow>,
) -> Result<Json<WorkflowDto>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = store(&s, BotId(input.bot_id))?;
        let old = store.get(&name).map_err(store_error)?;
        if old.revision != input.revision {
            return Err(save_error(SaveError::Conflict {
                name,
                current_version: old.revision,
            }));
        }
        let mut wf = apply_edit(&old, &input)?;
        privacy::redact_metadata(&mut wf)?;
        reject_secrets(&s, &wf)?;
        if wf.name == old.name {
            store
                .save_with_mode(
                    &wf,
                    SaveMode::Overwrite {
                        expected_version: Some(old.revision),
                    },
                )
                .map_err(save_error)?;
        } else {
            wf.revision = 1;
            store
                .save_with_mode(&wf, SaveMode::CreateNew)
                .map_err(save_error)?;
            store.delete(&old.name).map_err(store_error)?;
            s.agent().workflow_changed(&old.name);
        }
        s.agent().workflow_changed(&wf.name);
        let saved = store.get(&wf.name).map_err(store_error)?;
        WorkflowDto::from_workflow(&saved).map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(post, operation_id = "duplicate_workflow", path = "/v1/workflows/{name}/duplicate", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path)), request_body = DuplicateWorkflow,
    responses((status = 201, body = WorkflowDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn duplicate(
    State(s): State<AppState>,
    Path(name): Path<String>,
    ApiJson(input): ApiJson<DuplicateWorkflow>,
) -> Result<(StatusCode, Json<WorkflowDto>), ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = store(&s, BotId(input.bot_id))?;
        let mut wf = store.get(&name).map_err(store_error)?;
        if !valid_name(&input.new_name) {
            return Err(bad(
                "names use lowercase letters, digits, - and _ (up to 64)",
            ));
        }
        wf.name = input.new_name;
        wf.revision = 1;
        store
            .save_with_mode(&wf, SaveMode::CreateNew)
            .map_err(save_error)?;
        s.agent().workflow_changed(&wf.name);
        let saved = store.get(&wf.name).map_err(store_error)?;
        Ok((
            StatusCode::CREATED,
            Json(WorkflowDto::from_workflow(&saved)?),
        ))
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(get, operation_id = "export_workflow", path = "/v1/workflows/{name}/export", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path), ("bot_id" = Uuid, Query)),
    responses((status = 200, body = WorkflowExport), (status = 404, body = ErrorBody)))]
pub(crate) async fn export(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(scope): Query<WorkflowScope>,
) -> Result<Json<WorkflowExport>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let mut wf = store(&s, BotId(scope.bot_id))?
            .get(&name)
            .map_err(store_error)?;
        // Where it was recorded stays on this computer.
        wf.source = None;
        wf.revision = 1;
        Ok(Json(WorkflowExport {
            file_name: format!("{}.workflow.json", wf.name),
            content: wf.to_json().map_err(ApiError::internal)?,
        }))
    })
    .await
    .map_err(ApiError::internal)?
}

/// Strictly parse an imported file. Pure.
fn parse_import(content: &str, rename: Option<&str>, bot: BotId) -> Result<Workflow, ApiError> {
    if content.len() > MAX_IMPORT_BYTES {
        return Err(bad(
            "this file is too large to be a workflow (limit 256 KB)",
        ));
    }
    let value: Value = serde_json::from_str(content)
        .map_err(|_| bad("this is not a workflow file (it is not valid JSON)"))?;
    let object: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| bad("this is not a workflow file"))?;
    if object.keys().any(|k| !FILE_KEYS.contains(&k.as_str())) {
        return Err(bad(
            "this file has fields this app does not know; it was not imported",
        ));
    }
    if object
        .get("steps")
        .and_then(Value::as_array)
        .is_none_or(|a| a.is_empty() || a.len() > MAX_STEPS)
        || object
            .get("params")
            .and_then(Value::as_array)
            .is_some_and(|a| a.len() > MAX_PARAMS)
    {
        return Err(bad(
            "the file has no steps, or too many steps or parameters",
        ));
    }
    let mut wf: Workflow = serde_json::from_value(value)
        .map_err(|_| bad("this file does not match the workflow format"))?;
    if let Some(name) = rename {
        name.clone_into(&mut wf.name);
    }
    wf.validate().map_err(|e| bad(&e.to_string()))?;
    // Whatever the file claims, it did not come from a run here: mark every
    // step as outside content and the whole workflow as untrusted.
    wf.revision = 1;
    wf.source = Some(WorkflowSource {
        run_id: "imported".to_owned(),
        bot_id: bot.to_string(),
        untrusted_content: true,
        step_origins: vec!["run_content_or_model".to_owned(); wf.steps.len()],
    });
    Ok(wf)
}

#[utoipa::path(post, operation_id = "import_workflow", path = "/v1/workflows/import", tag = "workflows", security(("bearer" = [])),
    request_body = ImportWorkflow,
    responses((status = 201, body = WorkflowDto), (status = 400, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn import(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<ImportWorkflow>,
) -> Result<(StatusCode, Json<WorkflowDto>), ApiError> {
    tokio::task::spawn_blocking(move || {
        let bot = BotId(input.bot_id);
        let store = store(&s, bot)?;
        let mut wf = parse_import(&input.content, input.name.as_deref(), bot)?;
        privacy::redact_metadata(&mut wf)?;
        reject_secrets(&s, &wf)?;
        store
            .save_with_mode(&wf, SaveMode::CreateNew)
            .map_err(save_error)?;
        s.agent().workflow_changed(&wf.name);
        let saved = store.get(&wf.name).map_err(store_error)?;
        Ok((
            StatusCode::CREATED,
            Json(WorkflowDto::from_workflow(&saved)?),
        ))
    })
    .await
    .map_err(ApiError::internal)?
}

fn history_of(s: &AppState, bot: BotId, name: &str) -> Result<Vec<WorkflowRunRecord>, ApiError> {
    let mut runs: Vec<_> = s
        .storage()
        .list_runs(s.user_id())?
        .into_iter()
        .filter(|r| r.bot_id == bot && r.parent_run_id.is_none())
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    runs.truncate(HISTORY_RUNS);
    let mut out = Vec::new();
    for run in runs {
        let log = storage::event_log::read_event_log(s.storage().data_dir(), run.id)
            .map_err(ApiError::internal)?;
        let mut steps: Vec<HistoryStep> = Vec::new();
        for envelope in log {
            match envelope.event {
                Event::WorkflowStepStarted(e) if e.workflow == name => steps.push(HistoryStep {
                    step_index: e.step_index,
                    kind: e.kind,
                    status: "interrupted".to_owned(),
                    reason: None,
                }),
                Event::WorkflowStepCompleted(e) if e.workflow == name => {
                    if let Some(step) = steps
                        .iter_mut()
                        .rev()
                        .find(|x| x.step_index == e.step_index)
                    {
                        "ok".clone_into(&mut step.status);
                    }
                }
                Event::WorkflowDrift(e) if e.workflow == name => {
                    match steps
                        .iter_mut()
                        .rev()
                        .find(|x| x.step_index == e.step_index)
                    {
                        Some(step) => {
                            "failed".clone_into(&mut step.status);
                            step.reason = Some(e.reason);
                        }
                        None => steps.push(HistoryStep {
                            step_index: e.step_index,
                            kind: String::new(),
                            status: "failed".to_owned(),
                            reason: Some(e.reason),
                        }),
                    }
                }
                _ => {}
            }
        }
        if !steps.is_empty() {
            out.push(WorkflowRunRecord {
                run_id: run.id.0,
                state: run.state,
                started_at_ms: run.created_at_ms,
                steps,
            });
            if out.len() == HISTORY_SHOWN {
                break;
            }
        }
    }
    Ok(out)
}

#[utoipa::path(get, operation_id = "workflow_history", path = "/v1/workflows/{name}/history", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path), ("bot_id" = Uuid, Query)),
    responses((status = 200, body = Vec<WorkflowRunRecord>), (status = 404, body = ErrorBody)))]
pub(crate) async fn history(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(scope): Query<WorkflowScope>,
) -> Result<Json<Vec<WorkflowRunRecord>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let bot = BotId(scope.bot_id);
        store(&s, bot)?.get(&name).map_err(store_error)?;
        history_of(&s, bot, &name).map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}
