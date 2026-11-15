//! `POST /v1/runs/stop`: one Stop, one target set, fixed at first receipt.
//!
//! The caller cannot know which runs "existed" when the user pressed Stop, so
//! the daemon decides: the first time a `stop_id` arrives it reads the user's
//! runs once and stores the target set. A repeat of the same `stop_id` only
//! retries members whose cancel failed; it never adds a run, and a run created
//! after the first receipt is never cancelled by it. No clock of the caller is
//! compared with anything.
//!
//! The book of target sets is in memory (bounded: [`MAX_STOPS`] entries, each
//! for [`STOP_TTL`]). After a daemon restart a repeat is a first receipt; the
//! restart pauses every running run (`AgentLoop::recover`), so a repeat then
//! stops the runs that were live before the restart.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Extension, State};
use protocol::RunId;
use task_runtime::RunState;
use uuid::Uuid;

use crate::dto::{StopOutcome, StopRunResult, StopRunsInput, StopRunsResponse, StopScope};
use crate::live_view::{Actor, require_local};
use crate::routes::ApiJson;
use crate::{ApiError, AppState};

/// Stops kept for repeats.
pub(crate) const MAX_STOPS: usize = 256;
/// How long a stop's target set is remembered.
pub(crate) const STOP_TTL: Duration = Duration::from_secs(3600);
/// Largest `run_ids` list.
pub(crate) const MAX_RUN_IDS: usize = 1024;
/// Request body limit (1024 ids plus slack).
pub(crate) const MAX_BODY_BYTES: usize = 128 * 1024;

struct Member {
    run: RunId,
    /// The run's state when the target set was fixed (`None`: unknown id).
    at_receipt: Option<RunState>,
    outcome: StopOutcome,
    error: Option<String>,
}

struct Entry {
    at: Instant,
    members: Vec<Member>,
}

/// Target sets by `(actor, stop_id)`: another actor reusing an id gets its own
/// stop and never sees or retries somebody else's.
#[derive(Default)]
pub(crate) struct StopBook {
    entries: HashMap<(String, Uuid), Entry>,
}

impl StopBook {
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| now.duration_since(e.at) < STOP_TTL);
    }

    fn make_room(&mut self) {
        while self.entries.len() >= MAX_STOPS {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.at)
                .map(|(k, _)| k.clone())
            else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn is_open(state: Option<RunState>) -> bool {
    state.is_some_and(|s| !s.is_terminal())
}

fn state_of(s: &AppState, run: RunId) -> Option<(String, Option<RunState>)> {
    s.storage()
        .get_run(run)
        .ok()
        .flatten()
        .map(|r| (r.state.clone(), RunState::parse(&r.state)))
}

/// Cancel one member through the per-run cancel path and classify the result.
fn cancel_member(s: &AppState, m: &mut Member) {
    m.error = None;
    match s.agent().cancel(m.run) {
        Ok(()) => m.outcome = StopOutcome::Cancelled,
        Err(e) => match state_of(s, m.run) {
            // Cancelled meanwhile: by this stop (a parent took the child with
            // it) if the run was open when the set was fixed.
            Some((_, Some(RunState::Cancelled))) if is_open(m.at_receipt) => {
                m.outcome = StopOutcome::Cancelled;
            }
            Some((_, Some(st))) if st.is_terminal() => m.outcome = StopOutcome::AlreadyTerminal,
            None => m.outcome = StopOutcome::NotFound,
            Some(_) => {
                m.outcome = StopOutcome::Failed;
                m.error = Some(e.to_string());
            }
        },
    }
}

fn result_of(s: &AppState, m: &Member) -> StopRunResult {
    StopRunResult {
        run_id: m.run.0,
        outcome: m.outcome,
        state: state_of(s, m.run).map(|(st, _)| st),
        error: m.error.clone(),
    }
}

fn audit(
    s: &AppState,
    actor: &Actor,
    action: &str,
    meta: &serde_json::Value,
) -> Result<(), ApiError> {
    let who = if actor.local {
        "user"
    } else {
        actor.key.as_str()
    };
    s.storage()
        .append_audit(s.user_id(), None, who, action, meta)
        .map(|_| ())
        .map_err(ApiError::internal)
}

fn process(
    s: &AppState,
    actor: &Actor,
    input: &StopRunsInput,
) -> Result<StopRunsResponse, ApiError> {
    // The whole first receipt (read, audit, cancels) and every repeat run under
    // this lock: two requests with one `stop_id` cannot fix two sets, and the
    // second one sees the first one's outcomes.
    let mut book = s.stops().lock();
    book.prune(Instant::now());
    let key = (actor.key.clone(), input.stop_id);
    if let Some(entry) = book.entries.get_mut(&key) {
        // Retry what is still open: failed members, and cancelled ones that
        // are somehow not terminal. Terminal and unknown members stay as they were.
        let retry: Vec<usize> = entry
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                matches!(m.outcome, StopOutcome::Failed | StopOutcome::Cancelled)
                    && state_of(s, m.run)
                        .is_none_or(|(_, st)| !st.is_some_and(RunState::is_terminal))
            })
            .map(|(i, _)| i)
            .collect();
        if !retry.is_empty() {
            audit(
                s,
                actor,
                "runs.stop_retry",
                &serde_json::json!({"stop_id": input.stop_id, "count": retry.len()}),
            )?;
            for i in retry {
                cancel_member(s, &mut entry.members[i]);
            }
        }
        return Ok(StopRunsResponse {
            stop_id: input.stop_id,
            first_seen: false,
            results: entry.members.iter().map(|m| result_of(s, m)).collect(),
        });
    }

    // First receipt: one statement reads the user's runs; a run inserted after
    // it is not in the set. This read is the causal boundary.
    let runs: HashMap<Uuid, Option<RunState>> = s
        .storage()
        .list_runs(s.user_id())?
        .into_iter()
        .map(|r| (r.id.0, RunState::parse(&r.state)))
        .collect();
    let mut seen = HashSet::new();
    let mut members = Vec::new();
    let mut add = |id: Uuid| {
        if !seen.insert(id) {
            return;
        }
        let (at_receipt, outcome) = match runs.get(&id).copied() {
            None => (None, StopOutcome::NotFound),
            Some(st) if st.is_some_and(RunState::is_terminal) => (st, StopOutcome::AlreadyTerminal),
            // Pending: cancelled below.
            Some(st) => (st, StopOutcome::Failed),
        };
        members.push(Member {
            run: RunId(id),
            at_receipt,
            outcome,
            error: None,
        });
    };
    for id in &input.run_ids {
        add(*id);
    }
    if input.scope == StopScope::AllExisting {
        let mut open: Vec<Uuid> = runs
            .iter()
            .filter(|(_, st)| is_open(**st))
            .map(|(id, _)| *id)
            .collect();
        open.sort_unstable(); // UUIDv7: oldest first
        for id in open {
            add(id);
        }
    }
    // The audit row is durable before any cancel is issued; if it cannot be
    // written nothing is cancelled and the id stays unseen.
    audit(
        s,
        actor,
        "runs.stop",
        &serde_json::json!({
            "stop_id": input.stop_id,
            "scope": input.scope,
            "count": members.len(),
        }),
    )?;
    for m in &mut members {
        if m.outcome == StopOutcome::Failed {
            cancel_member(s, m);
        }
    }
    let results = members.iter().map(|m| result_of(s, m)).collect();
    book.make_room();
    book.entries.insert(
        key,
        Entry {
            at: Instant::now(),
            members,
        },
    );
    Ok(StopRunsResponse {
        stop_id: input.stop_id,
        first_seen: true,
        results,
    })
}

/// Stop runs: cancel a target set that is fixed at the first receipt of
/// `stop_id`. Idempotent per `stop_id`.
#[utoipa::path(post, path = "/v1/runs/stop", tag = "runs", security(("bearer" = [])),
    request_body = StopRunsInput,
    responses((status = 200, body = StopRunsResponse),
              (status = 400, body = crate::error::ErrorBody),
              (status = 403, body = crate::error::ErrorBody,
               description = "scope all_existing is for the local user only")))]
pub(crate) async fn stop_runs(
    State(s): State<AppState>,
    Extension(actor): Extension<Actor>,
    ApiJson(input): ApiJson<StopRunsInput>,
) -> Result<Json<StopRunsResponse>, ApiError> {
    if input.run_ids.len() > MAX_RUN_IDS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_RUN_IDS} run ids"
        )));
    }
    // Paired peers may stop runs they can cancel one by one (the per-run cancel
    // route has no stricter rule); sweeping everything is the local user's.
    if input.scope == StopScope::AllExisting {
        require_local(&s, &actor, "runs_stop_all")?;
    }
    // Storage and the agent's cancel are blocking calls.
    let out = tokio::task::spawn_blocking(move || process(&s, &actor, &input))
        .await
        .map_err(ApiError::internal)??;
    Ok(Json(out))
}
