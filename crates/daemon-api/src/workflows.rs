//! User-only workflow recording and persistence. No model-facing save tool.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
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
    source: Option<Value>,
}

impl WorkflowDto {
    fn from_workflow(wf: &Workflow) -> Result<Self, ApiError> {
        serde_json::from_value(json!({
            "name": wf.name, "description": wf.description,
            "params": wf.params, "steps": wf.steps, "source":wf.source,
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
pub(crate) struct ParameterOrigin {
    pointer: String,
    /// Exact user-request match before tool output, or conservatively unknown.
    origin: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct StepProvenance {
    step_index: usize,
    origin: String,
    parameters: Vec<ParameterOrigin>,
    needs_confirmation: bool,
    untrusted_read_before: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct WorkflowProposal {
    run_id: Uuid,
    workflow: WorkflowDto,
    skipped: Vec<SkippedCall>,
    warnings: Vec<String>,
    /// These defaults must remain absent, including after user edits.
    required_params: Vec<String>,
    step_provenance: Vec<StepProvenance>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveWorkflow {
    run_id: Uuid,
    name: String,
    description: String,
    params: Vec<WorkflowParam>,
    /// Step indexes explicitly acknowledged after reviewing external influence.
    #[serde(default)]
    confirmed_steps: Vec<usize>,
    /// The complete reviewed steps, which must equal the rebuilt proposal.
    steps: Vec<Value>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunWorkflow {
    chat_id: Uuid,
    /// Out-of-band values, kept only in replay memory and registered for redaction.
    #[serde(default)]
    params: BTreeMap<String, String>,
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct WorkflowScope {
    bot_id: Uuid,
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

fn store(s: &AppState, bot: protocol::BotId) -> Result<WorkflowStore, ApiError> {
    s.bot_of_user(bot)?;
    WorkflowStore::new(s.storage().data_dir())
        .for_bot(&bot.to_string())
        .map_err(store_error)
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

struct ReviewedRecording {
    recording: Recording,
    skipped: Vec<SkippedCall>,
    required_params: Vec<String>,
    step_provenance: Vec<StepProvenance>,
}

fn recording(s: &AppState, id: RunId) -> Result<ReviewedRecording, ApiError> {
    let run = s.run_of_user(id)?;
    // Successful human top-level runs are recordable, including ordinary K4.
    if run.parent_run_id.is_some() || run.state != "succeeded" || run.chat_id.is_none() {
        return Err(ApiError::conflict("this run cannot be recorded"));
    }
    let bot = s.bot_of_user(run.bot_id)?;
    let profile: agent_core::BotProfile = serde_json::from_value(bot.config)
        .map_err(|_| ApiError::conflict("invalid bot profile"))?;
    if !profile.router.enabled {
        return Err(ApiError::conflict(
            "workflow commands require the bot router",
        ));
    }
    s.agent().flush_journal();
    let privacy::ReviewSteps {
        steps,
        mut skipped,
        secrets,
        provenance,
        untrusted_content,
    } = privacy::review_steps(s, id)?;
    let mut recording = Recorder::new()
        .with_registry(s.agent().workflow_recording_tools(run.bot_id)?)
        .from_steps(&steps, &RecordOptions::default())
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    if !recording.dropped.is_empty() || skipped.iter().any(|d| d.reason == "call failed") {
        return Err(ApiError::conflict(
            "incomplete workflow: a mutating call was dropped or failed",
        ));
    }
    recording.workflow.source = Some(workflow_replay::format::WorkflowSource {
        run_id: id.to_string(),
        bot_id: run.bot_id.to_string(),
        untrusted_content,
        step_origins: provenance.iter().map(|p| p.origin.clone()).collect(),
    });
    let required = privacy::finish(&mut recording, &secrets)?;
    if s.agent().workflow_contains_secret(
        &serde_json::to_string(&recording.workflow).map_err(ApiError::internal)?,
    ) {
        return Err(ApiError::conflict(
            "credential in structural recording data",
        ));
    }
    skipped.extend(recording.dropped.iter().map(|d| SkippedCall {
        tool: d.tool.clone(),
        reason: d.reason.clone(),
    }));
    Ok(ReviewedRecording {
        recording,
        skipped,
        required_params: required,
        step_provenance: provenance,
    })
}

#[utoipa::path(post, path = "/v1/runs/{id}/workflow-proposal", tag = "workflows", security(("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses((status = 200, body = WorkflowProposal), (status = 409, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(crate) async fn propose(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkflowProposal>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let ReviewedRecording {
            recording,
            skipped,
            required_params,
            step_provenance,
        } = recording(&s, RunId(id))?;
        Ok(Json(WorkflowProposal {
            run_id: id,
            workflow: WorkflowDto::from_workflow(&recording.workflow)?,
            skipped,
            warnings: recording.warnings,
            required_params,
            step_provenance,
        }))
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(get, path = "/v1/workflows", tag = "workflows", security(("bearer" = [])),
    params(("bot_id" = Uuid, Query)), responses((status = 200, body = Vec<WorkflowDto>)))]
pub(crate) async fn list(
    State(s): State<AppState>,
    Query(scope): Query<WorkflowScope>,
) -> Result<Json<Vec<WorkflowDto>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let store = store(&s, protocol::BotId(scope.bot_id))?;
        let rows = store.list().map_err(store_error)?;
        rows.iter()
            .filter_map(|row| store.get(&row.name).ok())
            .map(|wf| WorkflowDto::from_workflow(&wf))
            .collect::<Result<Vec<_>, _>>()
            .map(Json)
    })
    .await
    .map_err(ApiError::internal)?
}

#[utoipa::path(get, path = "/v1/workflows/{name}", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path), ("bot_id" = Uuid, Query)), responses((status = 200, body = WorkflowDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn show(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(scope): Query<WorkflowScope>,
) -> Result<Json<WorkflowDto>, ApiError> {
    tokio::task::spawn_blocking(move || {
        WorkflowDto::from_workflow(
            &store(&s, protocol::BotId(scope.bot_id))?
                .get(&name)
                .map_err(store_error)?,
        )
        .map(Json)
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
        let ReviewedRecording {mut recording, required_params:required, step_provenance:provenance,..} = recording(&s, RunId(input.run_id))?;
        if provenance.iter().enumerate().any(|(index, step)| step.needs_confirmation && !input.confirmed_steps.contains(&index)) {
            return Err(ApiError::conflict("review and confirm each step influenced by external content"));
        }
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
        recording.save_new(&store(&s, s.run_of_user(RunId(input.run_id))?.bot_id)?).map_err(|e| match e {
            workflow_replay::store::SaveError::AlreadyExists(_) => ApiError::conflict(e.to_string()),
            workflow_replay::store::SaveError::Store(error) => store_error(error),
            other => ApiError::internal(other),
        })?;
        s.agent().workflow_changed(&recording.workflow.name);
        Ok((StatusCode::CREATED, Json(WorkflowDto::from_workflow(&recording.workflow)?)))
    }).await.map_err(ApiError::internal)?
}

#[utoipa::path(delete, path = "/v1/workflows/{name}", tag = "workflows", security(("bearer" = [])),
    params(("name" = String, Path), ("bot_id" = Uuid, Query)), responses((status = 204), (status = 404, body = ErrorBody)))]
pub(crate) async fn delete(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(scope): Query<WorkflowScope>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        if !store(&s, protocol::BotId(scope.bot_id))?
            .delete(&name)
            .map_err(store_error)?
        {
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
    let agent = s.agent().clone();
    let chat_id = ChatId(input.chat_id);
    let params = input.params;
    let run_id = tokio::task::spawn_blocking(move || {
        let chat = s.chat_of_user(chat_id)?;
        let wf = store(&s, chat.bot_id)?.get(&name).map_err(store_error)?;
        wf.bind_params(
            &params
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        )
        .map_err(|_| ApiError::bad_request("invalid workflow parameters"))?;
        s.leader()
            .with_active(|| agent.run_workflow(chat_id, &name, params))?
            .map_err(|_| {
                ApiError::conflict(
                    "cannot start workflow; values require at least six bytes and an idle chat",
                )
            })
    })
    .await
    .map_err(ApiError::internal)??;
    Ok((StatusCode::ACCEPTED, Json(WorkflowRun { run_id: run_id.0 })))
}
