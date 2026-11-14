//! User-only workflow recording and persistence. No model-facing save tool.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use protocol::{ChatId, RunId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;
use workflow_replay::{
    Param, RecordOptions, Recorder, Recording, StoreError, Workflow, WorkflowStore,
};

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

mod privacy;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowParam {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    default: Option<String>,
    description: String,
}

/// Complete step data is intentional: commands and parameters must be reviewable.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowDto {
    name: String,
    description: String,
    params: Vec<WorkflowParam>,
    steps: Vec<Value>,
}

impl WorkflowDto {
    fn from_workflow(wf: &Workflow) -> Result<Self, ApiError> {
        serde_json::from_value(json!({
            "name": wf.name, "description": wf.description,
            "params": wf.params, "steps": wf.steps,
        }))
        .map_err(ApiError::internal)
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct SkippedCall {
    tool: String,
    reason: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowProposal {
    run_id: Uuid,
    workflow: WorkflowDto,
    skipped: Vec<SkippedCall>,
    warnings: Vec<String>,
    /// These defaults must remain absent, including after user edits.
    required_params: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveWorkflow {
    run_id: Uuid,
    name: String,
    description: String,
    params: Vec<WorkflowParam>,
    /// The complete reviewed steps, which must equal the rebuilt proposal.
    steps: Vec<Value>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunWorkflow {
    chat_id: Uuid,
    /// Values containing double quotes are rejected: the existing K0 command
    /// parser cannot preserve them. Other whitespace and backslashes are data.
    #[serde(default)]
    params: BTreeMap<String, String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct WorkflowRun {
    run_id: Uuid,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/runs/{id}/workflow-proposal", post(propose))
        .route("/v1/workflows", get(list).post(save))
        .route("/v1/workflows/{name}", get(show).delete(delete))
        .route("/v1/workflows/{name}/run", post(run))
}

fn store(s: &AppState) -> WorkflowStore {
    WorkflowStore::new(s.storage().data_dir())
}

fn store_error(error: StoreError) -> ApiError {
    match error {
        StoreError::NotFound(name) => ApiError::not_found(name),
        StoreError::BadName(_) | StoreError::Workflow(_) => {
            ApiError::bad_request(error.to_string())
        }
        other => ApiError::internal(other),
    }
}

fn recording(
    s: &AppState,
    id: RunId,
) -> Result<(Recording, Vec<SkippedCall>, Vec<String>), ApiError> {
    let run = s.run_of_user(id)?;
    // Same eligibility as Remember, including ownership, top-level, success,
    // chat, enabled router and a meaningful recorded route.
    if run.parent_run_id.is_some() || run.state != "succeeded" || !s.agent().can_remember(id) {
        return Err(ApiError::conflict("this run cannot be recorded"));
    }
    s.agent().flush_journal();
    let privacy::ReviewSteps {
        steps,
        mut skipped,
        secrets,
    } = privacy::review_steps(s, id)?;
    let mut recording = Recorder::new()
        .with_registry(s.agent().tool_registry())
        .from_steps(&steps, &RecordOptions::default())
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let required = privacy::finish(&mut recording, &secrets)?;
    skipped.extend(recording.dropped.iter().map(|d| SkippedCall {
        tool: d.tool.clone(),
        reason: d.reason.clone(),
    }));
    Ok((recording, skipped, required))
}

#[utoipa::path(post, path = "/v1/runs/{id}/workflow-proposal", tag = "workflows", security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = WorkflowProposal), (status = 409, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(crate) async fn propose(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkflowProposal>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let (recording, skipped, required_params) = recording(&s, RunId(id))?;
        Ok(Json(WorkflowProposal {
            run_id: id,
            workflow: WorkflowDto::from_workflow(&recording.workflow)?,
            skipped,
            warnings: recording.warnings,
            required_params,
        }))
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(get, path = "/v1/workflows", tag = "workflows", security(("bearer" = [])),
    responses((status = 200, body = Vec<WorkflowDto>)))]
pub(crate) async fn list(State(s): State<AppState>) -> Result<Json<Vec<WorkflowDto>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = store(&s);
        let rows = store.list().map_err(store_error)?;
        rows.iter()
            .map(|row| WorkflowDto::from_workflow(&store.get(&row.name).map_err(store_error)?))
            .collect::<Result<Vec<_>, _>>()
            .map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(get, path = "/v1/workflows/{name}", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path)), responses((status = 200, body = WorkflowDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn show(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<WorkflowDto>, ApiError> {
    tokio::task::spawn_blocking(move || {
        WorkflowDto::from_workflow(&store(&s).get(&name).map_err(store_error)?).map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(post, path = "/v1/workflows", tag = "workflows", security(("bearer" = [])), request_body = SaveWorkflow,
    responses((status = 201, body = WorkflowDto), (status = 409, body = ErrorBody), (status = 400, body = ErrorBody)))]
pub(crate) async fn save(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<SaveWorkflow>,
) -> Result<(StatusCode, Json<WorkflowDto>), ApiError> {
    tokio::task::spawn_blocking(move || {
        // Rebuild the source server-side: callers can edit metadata/parameter
        // defaults, never replace steps with instructions supplied by a page.
        let (mut recording, _, required) = recording(&s, RunId(input.run_id))?;
        let reviewed_steps = serde_json::to_value(&recording.workflow.steps).map_err(ApiError::internal)?;
        if Value::Array(input.steps) != reviewed_steps {
            return Err(ApiError::bad_request("steps changed; review a fresh proposal before saving"));
        }
        let params: Vec<Param> = serde_json::from_value(serde_json::to_value(input.params).map_err(ApiError::internal)?)
            .map_err(|_| ApiError::bad_request("invalid parameters"))?;
        if params.len() != recording.workflow.params.len() || params.iter().zip(&recording.workflow.params).any(|(a,b)| a.name != b.name || a.ty != b.ty)
            || params.iter().any(|p| required.contains(&p.name) && p.default.is_some()) {
            return Err(ApiError::bad_request("parameter declarations must match the proposal; sensitive defaults must stay absent"));
        }
        recording.workflow.name = input.name;
        recording.workflow.description = input.description;
        recording.workflow.params = params;
        privacy::redact_metadata(&mut recording.workflow)?;
        recording.save_new(&store(&s)).map_err(|e| match e {
            workflow_replay::store::SaveError::AlreadyExists(_) => ApiError::conflict(e.to_string()),
            workflow_replay::store::SaveError::Store(error) => store_error(error),
            other => ApiError::internal(other),
        })?;
        s.agent().workflow_changed(&recording.workflow.name);
        Ok((StatusCode::CREATED, Json(WorkflowDto::from_workflow(&recording.workflow)?)))
    }).await.map_err(ApiError::internal)?
}

#[utoipa::path(delete, path = "/v1/workflows/{name}", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path)), responses((status = 204), (status = 404, body = ErrorBody)))]
pub(crate) async fn delete(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        if !store(&s).delete(&name).map_err(store_error)? {
            return Err(ApiError::not_found(name));
        }
        s.agent().workflow_changed(&name);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(post, path = "/v1/workflows/{name}/run", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path)), request_body = RunWorkflow,
    responses((status = 202, body = WorkflowRun), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(crate) async fn run(
    State(s): State<AppState>,
    Path(name): Path<String>,
    ApiJson(input): ApiJson<RunWorkflow>,
) -> Result<(StatusCode, Json<WorkflowRun>), ApiError> {
    let chat = s.chat_of_user(ChatId(input.chat_id))?;
    let bot = s.bot_of_user(chat.bot_id)?;
    let profile: agent_core::BotProfile = serde_json::from_value(bot.config)
        .map_err(|_| ApiError::conflict("invalid bot profile"))?;
    if !profile.router.enabled {
        return Err(ApiError::conflict(
            "workflow commands require the bot router",
        ));
    }
    let wf = store(&s).get(&name).map_err(store_error)?;
    wf.bind_params(
        &input
            .params
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    // Keep the human initiator and the existing K0 -> LoopGate replay path.
    // The current slash parser strips double quotes and has no escape syntax;
    // refuse unrepresentable values instead of silently changing tool inputs.
    let mut command = format!("/workflow {name}");
    for (key, value) in input.params {
        if value.contains('"') {
            return Err(ApiError::bad_request(
                "workflow command parameters cannot contain double quotes",
            ));
        }
        let _ = write!(command, " \"{key}={value}\"");
    }
    let run_id = s
        .leader()
        .with_active(|| s.agent().run_turn(chat.id, &command))??;
    Ok((StatusCode::ACCEPTED, Json(WorkflowRun { run_id: run_id.0 })))
}
