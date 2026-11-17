//! "Sign in on the server": the owner's paired desktop signs a coding CLI in ON THE HOST.
//!
//! Same authority as the engine install route (`host_admin`): a paired desktop whose
//! `Device.admin` is read from the pairing store on every request, host mode only; the client
//! names an engine id and nothing else. One login runs at a time (a one-slot semaphore).
//!
//! The login itself is the vendor's own command from the fixed registry in
//! `cli_provider::login`. This module is only the job state machine around it:
//!
//! ```text
//! starting -> needs_browser {url, code}   (device-code flow: open the page, type the code)
//!          -> needs_code {url}            (paste-code flow: sign in, paste the page's code back)
//!          -> verifying -> done           (the host's own detection reports "signed in")
//!                       -> failed {code} | timed_out | cancelled
//! ```
//!
//! Nothing secret is ever kept or sent: the output stays private to `LoginRun`, a pasted code
//! goes to the process' stdin and is dropped, and the audit rows name the engine and the
//! outcome only. The whole process tree is killed on cancel, timeout or shutdown.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use cli_provider::login::{self, FlowKind, LoginSpec, Prompt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;

use crate::{
    ApiError, AppState, PeerIdentity,
    host_admin::{Gate, audit, require_admin},
    routes::ApiJson,
};

/// The whole login, from start to a confirmed sign-in.
pub(crate) const LOGIN_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long the CLI may take to show its page.
const STARTUP: Duration = Duration::from_secs(30);

/// Tests shorten the two limits above (milliseconds; 0 = production values).
#[cfg(test)]
pub(crate) static LIFETIME_TEST_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
pub(crate) static STARTUP_TEST_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn lifetime() -> Duration {
    #[cfg(test)]
    if let ms @ 1.. = LIFETIME_TEST_MS.load(std::sync::atomic::Ordering::SeqCst) {
        return Duration::from_millis(ms);
    }
    LOGIN_LIFETIME
}

fn startup() -> Duration {
    #[cfg(test)]
    if let ms @ 1.. = STARTUP_TEST_MS.load(std::sync::atomic::Ordering::SeqCst) {
        return Duration::from_millis(ms);
    }
    STARTUP
}
/// How long the CLI may take after the code was handed to it.
const AFTER_CODE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct Job {
    id: u64,
    state: &'static str,
    url: Option<String>,
    code: Option<String>,
    error: Option<&'static str>,
    cancel: CancellationToken,
    code_tx: Option<tokio::sync::mpsc::Sender<String>>,
}

pub(crate) struct LoginBook {
    pub(crate) running: Arc<tokio::sync::Semaphore>,
    jobs: parking_lot::Mutex<HashMap<String, Job>>,
    next: std::sync::atomic::AtomicU64,
}

impl Default for LoginBook {
    fn default() -> Self {
        Self {
            running: Arc::new(tokio::sync::Semaphore::new(1)),
            jobs: parking_lot::Mutex::new(HashMap::new()),
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

/// `GET/POST/DELETE /v1/admin/cli-engines/{id}/login`: where the sign-in stands.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AdminCliLoginDto {
    pub engine: String,
    /// `idle`, `starting`, `needs_browser`, `needs_code`, `verifying`, `done`, `failed`,
    /// `timed_out` or `cancelled`.
    pub state: String,
    /// How this engine can be signed in from the desktop: `device_code`, `paste_code` or
    /// `manual` (it cannot be driven remotely: run `command` over SSH).
    pub flow: String,
    /// The vendor's sign-in page (verified against its allow-listed hosts).
    pub url: Option<String>,
    /// The one-time code to type on that page (device-code flow).
    pub code: Option<String>,
    /// Stable code of the failure (`login_start_timeout`, `login_code_rejected`, ...).
    pub error: Option<String>,
    /// The exact command to run over SSH for a `manual` engine.
    pub command: Option<String>,
    /// Found on the host right now.
    pub installed: bool,
    /// Signed in, when the CLI can say.
    pub signed_in: Option<bool>,
}

/// `POST .../login/code`: the code the vendor's page showed.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminLoginCodeInput {
    /// One line, 8-1024 plain characters. Never logged, echoed or stored.
    pub code: String,
}

impl std::fmt::Debug for AdminLoginCodeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminLoginCodeInput")
            .field("code", &"<redacted>")
            .finish()
    }
}

fn spec_of(engine: &str) -> Result<&'static cli_provider::registry::CliEngineSpec, ApiError> {
    cli_provider::engine_spec(engine).ok_or_else(|| ApiError::not_found("unknown engine"))
}

fn flow_name(engine: &str) -> &'static str {
    match login::login_spec(engine).map(|s| s.kind) {
        Some(FlowKind::DeviceCode) => "device_code",
        Some(FlowKind::PasteCode) => "paste_code",
        None => "manual",
    }
}

async fn dto(
    s: &AppState,
    engine: &'static cli_provider::registry::CliEngineSpec,
) -> AdminCliLoginDto {
    let job = s.0.cli_logins.jobs.lock().get(engine.id).cloned();
    let row = s
        .detector()
        .detect(false)
        .await
        .into_iter()
        .find(|r| r.id == engine.id);
    let flow = flow_name(engine.id);
    AdminCliLoginDto {
        engine: engine.id.into(),
        state: job.as_ref().map_or("idle", |j| j.state).into(),
        flow: flow.into(),
        url: job.as_ref().and_then(|j| j.url.clone()),
        code: job.as_ref().and_then(|j| j.code.clone()),
        error: job.as_ref().and_then(|j| j.error).map(str::to_owned),
        command: (flow == "manual").then(|| engine.login.to_owned()),
        installed: row.as_ref().is_some_and(|r| r.installed),
        signed_in: row.and_then(|r| match r.auth {
            cli_provider::AuthState::SignedIn => Some(true),
            cli_provider::AuthState::SignedOut => Some(false),
            cli_provider::AuthState::Unknown => None,
        }),
    }
}

fn update(s: &AppState, engine: &str, id: u64, f: impl FnOnce(&mut Job)) {
    if let Some(job) = s.0.cli_logins.jobs.lock().get_mut(engine)
        && job.id == id
    {
        f(job);
    }
}

fn finish(s: &AppState, engine: &str, id: u64, state: &'static str, error: Option<&'static str>) {
    update(s, engine, id, |j| {
        j.state = state;
        j.error = error;
        j.code_tx = None;
    });
}

/// A stable reason for a login that ended without a sign-in, read from the CLI's private output.
fn classify(output: &str) -> &'static str {
    let lower = output.to_ascii_lowercase();
    if lower.contains("device-auth")
        && (lower.contains("unexpected argument")
            || lower.contains("unrecognized")
            || lower.contains("unknown option"))
    {
        "login_cli_outdated"
    } else if lower.contains("device")
        && (lower.contains("disabled")
            || lower.contains("not enabled")
            || lower.contains("not allowed"))
    {
        "device_auth_disabled"
    } else if lower.contains("expired") || lower.contains("timed out") {
        "login_expired"
    } else if lower.contains("invalid") && lower.contains("code") {
        "login_code_rejected"
    } else {
        "login_failed"
    }
}

/// Run the whole login. The permit is held until the process tree is gone.
#[allow(clippy::too_many_lines)] // one ordered state machine: start, prompt, code, exit, verify
async fn run_job(
    s: AppState,
    spec: &'static LoginSpec,
    id: u64,
    cancel: CancellationToken,
    mut code_rx: tokio::sync::mpsc::Receiver<String>,
) {
    let engine = spec.engine;
    let shutdown = s.shutdown_token();
    let deadline = tokio::time::Instant::now() + lifetime();
    let detector = s.detector();
    let mut run = match login::start(spec, detector.environment()) {
        Ok(run) => run,
        Err(login::StartError::NotInstalled) => {
            finish(&s, engine, id, "failed", Some("not_installed"));
            return;
        }
        Err(login::StartError::SpawnFailed) => {
            finish(&s, engine, id, "failed", Some("spawn_failed"));
            return;
        }
    };
    // 1. The page (and code) the vendor's CLI wants the person to use.
    let prompt = tokio::select! {
        p = run.next_prompt(spec, startup()) => p,
        () = cancel.cancelled() => { run.terminate().await; finish(&s, engine, id, "cancelled", None); return; }
        () = shutdown.cancelled() => { run.terminate().await; finish(&s, engine, id, "cancelled", None); return; }
    };
    let Some(Prompt::Page { url, code }) = prompt else {
        let reason = classify(run.private_output());
        run.terminate().await;
        finish(
            &s,
            engine,
            id,
            "failed",
            Some(if reason == "login_failed" {
                "login_start_timeout"
            } else {
                reason
            }),
        );
        return;
    };
    let paste = spec.kind == FlowKind::PasteCode;
    update(&s, engine, id, |j| {
        j.state = if paste { "needs_code" } else { "needs_browser" };
        j.url = Some(url);
        j.code = code;
    });
    // 2. A pasted code (paste flow), then the exit and the confirmation.
    let mut sent_at: Option<tokio::time::Instant> = None;
    let exit = loop {
        let limit = sent_at.map_or(deadline, |t| (t + AFTER_CODE).min(deadline));
        tokio::select! {
            biased;
            () = cancel.cancelled() => { run.terminate().await; finish(&s, engine, id, "cancelled", None); return; }
            () = shutdown.cancelled() => { run.terminate().await; finish(&s, engine, id, "cancelled", None); return; }
            () = tokio::time::sleep_until(limit) => {
                run.terminate().await;
                if sent_at.is_some() && tokio::time::Instant::now() < deadline {
                    finish(&s, engine, id, "failed", Some("login_not_confirmed"));
                } else {
                    finish(&s, engine, id, "timed_out", Some("login_timed_out"));
                }
                return;
            }
            Some(pasted) = code_rx.recv(), if paste && sent_at.is_none() => {
                // Straight to the process' stdin; the value is dropped right after.
                if run.send_line(&pasted).await.is_err() {
                    run.terminate().await;
                    finish(&s, engine, id, "failed", Some("login_failed"));
                    return;
                }
                drop(pasted);
                sent_at = Some(tokio::time::Instant::now());
                update(&s, engine, id, |j| j.state = "verifying");
            }
            exit = run.wait_exit(Duration::from_secs(3600)) => break exit,
        }
    };
    // 3. Confirm with the host's own detection, the way turns will see it.
    let exited_ok = matches!(exit, Some(Some(0)));
    if !exited_ok {
        let reason = classify(run.private_output());
        run.terminate().await;
        finish(&s, engine, id, "failed", Some(reason));
        return;
    }
    run.terminate().await;
    update(&s, engine, id, |j| j.state = "verifying");
    let signed_in = s
        .detector()
        .detect(true)
        .await
        .into_iter()
        .find(|r| r.id == engine)
        .is_some_and(|r| r.auth == cli_provider::AuthState::SignedIn);
    if signed_in {
        finish(&s, engine, id, "done", None);
    } else {
        finish(&s, engine, id, "failed", Some("login_not_confirmed"));
    }
}

/// Start the engine's own sign-in ON THE HOST. Admin desktop only; one at a time.
#[utoipa::path(post, operation_id = "admin_cli_engine_login", path = "/v1/admin/cli-engines/{id}/login", tag = "admin", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 202, body = AdminCliLoginDto), (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub(crate) async fn start_login(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<AdminCliLoginDto>), ApiError> {
    let admin = require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    let engine = spec_of(&id)?;
    let Some(spec) = login::login_spec(engine.id) else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "login_manual",
            format!(
                "this engine cannot be signed in from here: run `{}` on the server",
                engine.login
            ),
        ));
    };
    let row = s
        .detector()
        .detect(false)
        .await
        .into_iter()
        .find(|r| r.id == engine.id);
    if !row.as_ref().is_some_and(|r| r.installed) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "not_installed",
            "install this engine on the server first",
        ));
    }
    // A working sign-in is never replaced from here.
    if row.is_some_and(|r| r.auth == cli_provider::AuthState::SignedIn) {
        let job = Job {
            id: s
                .0
                .cli_logins
                .next
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            state: "done",
            url: None,
            code: None,
            error: None,
            cancel: CancellationToken::new(),
            code_tx: None,
        };
        s.0.cli_logins.jobs.lock().insert(engine.id.to_owned(), job);
        return Ok((StatusCode::ACCEPTED, Json(dto(&s, engine).await)));
    }
    let permit = Arc::clone(&s.0.cli_logins.running)
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::CONFLICT,
                "login_busy",
                "a sign-in is already running on this server",
            )
        })?;
    audit(
        &s,
        &admin,
        "provider.cli_login.admin_started",
        &serde_json::json!({"engine": engine.id}),
    )?;
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let cancel = CancellationToken::new();
    let job_id =
        s.0.cli_logins
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    s.0.cli_logins.jobs.lock().insert(
        engine.id.to_owned(),
        Job {
            id: job_id,
            state: "starting",
            url: None,
            code: None,
            error: None,
            cancel: cancel.clone(),
            code_tx: Some(tx),
        },
    );
    let task = s.clone();
    tokio::spawn(async move {
        let _permit = permit;
        run_job(task.clone(), spec, job_id, cancel, rx).await;
        let outcome = task
            .0
            .cli_logins
            .jobs
            .lock()
            .get(spec.engine)
            .filter(|j| j.id == job_id)
            .map(|j| (j.state, j.error));
        let _ = task.detector().detect(true).await;
        let _ = audit(
            &task,
            &admin,
            "provider.cli_login.admin_finished",
            &serde_json::json!({"engine": spec.engine, "state": outcome.map(|o| o.0), "error": outcome.and_then(|o| o.1)}),
        );
    });
    Ok((StatusCode::ACCEPTED, Json(dto(&s, engine).await)))
}

/// Where the sign-in above stands (poll while it is `starting`, `needs_*` or `verifying`).
#[utoipa::path(get, operation_id = "admin_cli_engine_login_status", path = "/v1/admin/cli-engines/{id}/login", tag = "admin", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = AdminCliLoginDto), (status = 403, body = crate::error::ErrorBody)))]
pub(crate) async fn login_status(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    Path(id): Path<String>,
) -> Result<Json<AdminCliLoginDto>, ApiError> {
    require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    Ok(Json(dto(&s, spec_of(&id)?).await))
}

/// Hand the code the vendor's page showed to the waiting CLI. Exactly one code per sign-in.
#[utoipa::path(post, operation_id = "admin_cli_engine_login_code", path = "/v1/admin/cli-engines/{id}/login/code", tag = "admin", security(("bearer" = [])),
    params(("id" = String, Path)), request_body = AdminLoginCodeInput,
    responses((status = 202, body = AdminCliLoginDto), (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub(crate) async fn login_code(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<AdminLoginCodeInput>,
) -> Result<(StatusCode, Json<AdminCliLoginDto>), ApiError> {
    require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    let engine = spec_of(&id)?;
    let code = login::valid_code(&input.code)
        .ok_or_else(|| ApiError::bad_request("paste the whole code shown on the vendor's page"))?
        .to_owned();
    let tx = {
        let mut jobs = s.0.cli_logins.jobs.lock();
        let job = jobs
            .get_mut(engine.id)
            .filter(|j| j.state == "needs_code")
            .ok_or_else(|| ApiError::conflict("no sign-in is waiting for a code"))?;
        // One code per sign-in: the channel is consumed here.
        job.code_tx.take()
    };
    let tx = tx.ok_or_else(|| ApiError::conflict("a code was already sent for this sign-in"))?;
    tx.send(code)
        .await
        .map_err(|_| ApiError::conflict("the sign-in is no longer running"))?;
    Ok((StatusCode::ACCEPTED, Json(dto(&s, engine).await)))
}

/// Stop the sign-in (the whole process tree is killed).
#[utoipa::path(delete, operation_id = "admin_cli_engine_login_cancel", path = "/v1/admin/cli-engines/{id}/login", tag = "admin", security(("bearer" = [])),
    params(("id" = String, Path)),
    responses((status = 200, body = AdminCliLoginDto), (status = 403, body = crate::error::ErrorBody)))]
pub(crate) async fn cancel_login(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    Path(id): Path<String>,
) -> Result<Json<AdminCliLoginDto>, ApiError> {
    require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    let engine = spec_of(&id)?;
    if let Some(job) = s.0.cli_logins.jobs.lock().get(engine.id) {
        job.cancel.cancel();
    }
    // Give the task a moment to kill the tree and record the state.
    for _ in 0..50 {
        let state = s.0.cli_logins.jobs.lock().get(engine.id).map(|j| j.state);
        if !matches!(
            state,
            Some("starting" | "needs_browser" | "needs_code" | "verifying")
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(Json(dto(&s, engine).await))
}
