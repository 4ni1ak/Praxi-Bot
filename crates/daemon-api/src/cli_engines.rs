//! Host-owner engine onboarding. Intentionally absent from the tool registry.
use crate::{ApiError, AppState, dto::DetectedProviderDto, live_view::Actor, routes::ApiJson};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct CliInstallDto {
    pub program: String,
    pub args: Vec<String>,
    pub command: String,
    pub verified: bool,
    pub executable: bool,
}

/// One approval level for a CLI that runs with its own tools.
#[derive(Debug, Serialize, ToSchema)]
pub struct CliLevelDto {
    /// `ask`, `accept_edits`, `auto` or `full`.
    pub level: String,
    /// `false` when the CLI has no such mode: it would run as `applied`.
    pub offered: bool,
    pub applied: String,
    /// The CLI's own setting this level maps to (shown to the owner).
    pub native_setting: String,
}

/// Native-tools mode of a CLI: the CLI runs files and shell itself.
#[derive(Debug, Serialize, ToSchema)]
pub struct CliNativeDto {
    /// `verified` (flags confirmed in the installed CLI) or `experimental`.
    pub support: String,
    pub levels: Vec<CliLevelDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CliEngineDto {
    /// Present when bots can use this CLI with its own tools.
    pub native: Option<CliNativeDto>,
    pub detected: DetectedProviderDto,
    pub executable_names: Vec<String>,
    pub version_args: Vec<String>,
    /// Empty when no safe, locally verified CLI auth-status command is known.
    pub auth_args: Vec<String>,
    pub path: Option<String>,
    pub minimum_version: Option<String>,
    pub outdated: bool,
    pub login: String,
    pub docs_url: String,
    pub install: Option<CliInstallDto>,
    pub limitation: Option<String>,
    /// `cli:<id>` is switched on in `providers.toml`.
    pub enabled: bool,
    /// The owner switched this engine off. Recorded as a `cli:<id>` entry with
    /// `enabled = false` in `providers.toml`; such an engine is never auto-enabled.
    pub user_disabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CliInstallInput {
    pub id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CliInstallApproval {
    pub approval_id: Uuid,
    pub engine_id: String,
    pub command: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CliInstallDecision {
    pub decision: crate::dto::Decision,
}

struct Pending {
    engine: &'static str,
    actor: String,
    expires: i64,
}
pub(crate) struct InstallBook {
    pending: parking_lot::Mutex<HashMap<Uuid, Pending>>,
    running: Arc<tokio::sync::Semaphore>,
}
impl Default for InstallBook {
    fn default() -> Self {
        Self {
            pending: parking_lot::Mutex::new(HashMap::new()),
            running: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

pub(crate) fn local_owner(actor: &Actor) -> Result<(), ApiError> {
    if actor.local {
        Ok(())
    } else {
        Err(ApiError::forbidden("CLI settings require the local owner"))
    }
}

fn audit(s: &AppState, action: &str, detail: &serde_json::Value) -> Result<(), ApiError> {
    s.storage()
        .append_audit(s.user_id(), None, "user", action, detail)
        .map_err(ApiError::internal)?;
    Ok(())
}

#[utoipa::path(get, path = "/v1/providers/cli", tag = "providers", security(("bearer" = [])),
    params(crate::routes::DetectProvidersQuery), responses((status = 200, body = Vec<CliEngineDto>)))]
#[allow(clippy::too_many_lines)] // one flat DTO mapping
pub(crate) async fn list(
    State(s): State<AppState>,
    Extension(actor): Extension<Actor>,
    Query(q): Query<crate::routes::DetectProvidersQuery>,
) -> Result<Json<Vec<CliEngineDto>>, ApiError> {
    local_owner(&actor)?;
    let detector = s.detector();
    let rows = detector
        .detect(
            q.refresh
                .as_deref()
                .is_some_and(|r| r == "1" || r == "true"),
        )
        .await;
    let auto_enabled = auto_enable_new(&s, &rows).await?;
    let configured =
        provider_catalog::ProviderConfig::load(&provider_catalog::config_path(&s.data_dir())).ok();
    let entry_enabled = |id: &str| {
        configured
            .as_ref()
            .and_then(|cfg| cfg.get(&format!("cli:{id}")))
            .map(|p| p.enabled)
    };
    let rows = rows
        .into_iter()
        .map(|mut detected| {
            let spec = cli_provider::engine_spec(&detected.id).expect("registered engine");
            let outdated = spec
                .minimum_version
                .zip(detected.version.as_deref())
                .is_some_and(|(min, version)| {
                    numeric_version(version).is_some_and(|version| version < min)
                });
            if outdated {
                detected.runnable = false;
                detected.detail = Some("outdated".into());
            } else if spec.minimum_version.is_some()
                && detected.installed
                && detected
                    .version
                    .as_deref()
                    .and_then(numeric_version)
                    .is_none()
            {
                detected.runnable = false;
                detected.detail = Some("version_unverified".into());
            }
            let entry = entry_enabled(&detected.id);
            CliEngineDto {
                enabled: entry.unwrap_or(false) || auto_enabled.contains(&detected.id),
                user_disabled: entry == Some(false),
                executable_names: std::iter::once(spec.command)
                    .chain(spec.aliases.iter().copied())
                    .flat_map(|name| {
                        if cfg!(windows) {
                            vec![
                                format!("{name}.exe"),
                                format!("{name}.cmd"),
                                format!("{name}.bat"),
                                name.to_owned(),
                            ]
                        } else {
                            vec![name.to_owned()]
                        }
                    })
                    .collect(),
                version_args: spec.version_args.iter().map(|s| (*s).into()).collect(),
                auth_args: spec.auth_args.iter().map(|s| (*s).into()).collect(),
                path: cli_provider::search::find_engine(spec, detector.environment())
                    .map(|p| p.to_string_lossy().into_owned()),
                minimum_version: spec
                    .minimum_version
                    .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}")),
                outdated,
                login: spec.login.into(),
                docs_url: spec.docs_url.into(),
                install: spec.install().map(|i| CliInstallDto {
                    program: i.program.into(),
                    args: i.args.iter().map(|a| (*a).into()).collect(),
                    command: i.display(),
                    verified: i.verified,
                    executable: i.verified && i.executable,
                }),
                native: spec.driver().map(|driver| CliNativeDto {
                    support: match driver.support() {
                        cli_provider::Support::Verified => "verified",
                        cli_provider::Support::Experimental => "experimental",
                    }
                    .into(),
                    levels: cli_provider::ApprovalLevel::ALL
                        .into_iter()
                        .map(|level| {
                            let plan = cli_provider::level_plan(driver, level);
                            CliLevelDto {
                                level: level.as_str().into(),
                                offered: plan.offered,
                                applied: plan.applied.as_str().into(),
                                native_setting: plan.native.into(),
                            }
                        })
                        .collect(),
                }),
                // Not runnable under Praxi's own tools; usable with its own tools per bot.
                limitation: (!spec.runnable).then(|| {
                    if spec.driver().is_some() {
                        "native_only"
                    } else {
                        "detection_only"
                    }
                    .into()
                }),
                detected: detected.into(),
            }
        })
        .collect();
    Ok(Json(rows))
}

/// A detected, signed-in engine is switched on the first time it is seen. That only makes
/// it selectable in the model picker: no bot changes engine or approval level (those live
/// in each bot's `engine` table, written only by `PUT /v1/bots/{id}/engine`). "Seen" is
/// the `cli:<id>` entry in `providers.toml`: the owner's switch-off writes `enabled = false`
/// there, so it is never turned on again. Engines whose sign-in cannot be probed
/// (no `auth_args`, for example `OpenCode`) count as signed in unless detection says signed out.
pub(crate) async fn auto_enable_new(
    s: &AppState,
    rows: &[cli_provider::DetectedCli],
) -> Result<Vec<String>, ApiError> {
    let path = provider_catalog::config_path(&s.data_dir());
    let Ok(cfg) = provider_catalog::ProviderConfig::load(&path) else {
        return Ok(Vec::new());
    };
    let mut done = Vec::new();
    for row in rows {
        let Some(spec) = cli_provider::engine_spec(&row.id) else {
            continue;
        };
        let signed_in = match row.auth {
            cli_provider::AuthState::SignedIn => true,
            cli_provider::AuthState::Unknown => spec.auth_args.is_empty(),
            cli_provider::AuthState::SignedOut => false,
        };
        let eligible = spec.can_enable()
            && row.installed
            && row.version.is_some()
            && signed_in
            && spec.minimum_version.is_none_or(|min| {
                row.version
                    .as_deref()
                    .and_then(numeric_version)
                    .is_some_and(|version| version >= min)
            })
            && cfg.get(&format!("cli:{}", spec.id)).is_none();
        let Some(engine) = crate::routes::cli_engine_kind(spec.id).filter(|_| eligible) else {
            continue;
        };
        let st = s.clone();
        tokio::task::spawn_blocking(move || {
            crate::routes::persist_cli_enable(&st, spec, engine, true)
        })
        .await
        .map_err(ApiError::internal)??;
        done.push(spec.id.to_owned());
    }
    if !done.is_empty() && s.provider_host().is_some() {
        let _ = crate::routes::reload_from_host(s).await?;
    }
    Ok(done)
}

fn numeric_version(text: &str) -> Option<(u32, u32, u32)> {
    text.split_whitespace().find_map(|word| {
        let parts: Vec<_> = word
            .trim_start_matches('v')
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<_, _>>()
            .ok()?;
        (parts.len() == 3).then(|| (parts[0], parts[1], parts[2]))
    })
}

/// Creates an expiring approval; never starts a process.
#[utoipa::path(post, path = "/v1/providers/cli/install", tag = "providers", security(("bearer" = [])),
    request_body = CliInstallInput, responses((status = 200, body = CliInstallApproval)))]
pub(crate) async fn preview(
    State(s): State<AppState>,
    Extension(actor): Extension<Actor>,
    ApiJson(input): ApiJson<CliInstallInput>,
) -> Result<Json<CliInstallApproval>, ApiError> {
    local_owner(&actor)?;
    let spec = cli_provider::engine_spec(&input.id)
        .ok_or_else(|| ApiError::bad_request("unknown engine"))?;
    let install = spec
        .install()
        .filter(|i| i.verified && i.executable)
        .ok_or_else(|| ApiError::bad_request("copy and run yourself only"))?;
    let mut pending = s.0.cli_installs.pending.lock();
    let now = storage::now_ms();
    pending.retain(|_, p| p.expires > now);
    if pending.len() >= 16 {
        return Err(ApiError::conflict("too many pending installs"));
    }
    let approval = CliInstallApproval {
        approval_id: Uuid::now_v7(),
        engine_id: spec.id.into(),
        command: install.display(),
        expires_at_ms: now + 120_000,
    };
    audit(
        &s,
        "provider.cli_install.requested",
        &serde_json::json!({"approval_id": approval.approval_id, "engine": spec.id, "command": approval.command}),
    )?;
    pending.insert(
        approval.approval_id,
        Pending {
            engine: spec.id,
            actor: actor.key,
            expires: approval.expires_at_ms,
        },
    );
    Ok(Json(approval))
}

/// Approve once (or deny). SSE `output` is untrusted text; `done` includes a
/// stable error code and follows re-detection. Disconnect cancels the owned tree.
#[utoipa::path(post, path = "/v1/providers/cli/install/{approval_id}/decision", tag = "providers", security(("bearer" = [])),
    params(("approval_id" = Uuid, Path)), request_body = CliInstallDecision,
    responses((status = 200, description = "SSE output and done events; denied approvals emit done only", content_type = "text/event-stream")))]
pub(crate) async fn decide(
    State(s): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    ApiJson(input): ApiJson<CliInstallDecision>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    local_owner(&actor)?;
    let permit = Arc::clone(&s.0.cli_installs.running)
        .try_acquire_owned()
        .map_err(|_| ApiError::conflict("an install is already running"))?;
    let pending =
        s.0.cli_installs
            .pending
            .lock()
            .remove(&id)
            .ok_or_else(|| ApiError::not_found("install approval"))?;
    if pending.actor != actor.key || pending.expires <= storage::now_ms() {
        return Err(ApiError::forbidden(
            "install approval expired or belongs to another owner",
        ));
    }
    let approved = matches!(input.decision, crate::dto::Decision::Approve);
    audit(
        &s,
        "provider.cli_install.decision",
        &serde_json::json!({"approval_id": id, "engine": pending.engine, "approved": approved}),
    )?;
    // This durable row precedes every possible spawn, including resolver failure.
    if approved {
        audit(
            &s,
            "provider.cli_install.started",
            &serde_json::json!({"approval_id": id, "engine": pending.engine}),
        )?;
    }
    // Each output event consumes at least one of the runner's capped 64 KiB
    // input bytes. This finite capacity plus one terminal event cannot drop
    // output even when a child writes one-byte lines faster than HTTP drains.
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(65_537);
    tokio::spawn(async move {
        let _permit = permit;
        let detector = s.detector();
        let shutdown = s.shutdown_token();
        let work = async {
            if !approved {
                return Err("denied");
            }
            cli_provider::install::run_install(
                pending.engine,
                detector.environment(),
                &s.data_dir(),
                cli_provider::install::INSTALL_TIMEOUT,
                |text| {
                    let _ = tx.try_send(
                        Event::default()
                            .event("output")
                            .data(serde_json::json!({"text": text}).to_string()),
                    );
                },
            )
            .await
        };
        let result = tokio::select! {
            result = work => result,
            () = tx.closed() => Err("disconnected"),
            () = shutdown.cancelled() => Err("shutdown"),
        };
        let _ = detector.detect(true).await;
        let error = result.err();
        let audited = audit(
            &s,
            "provider.cli_install.finished",
            &serde_json::json!({"approval_id": id, "engine": pending.engine, "error": error}),
        )
        .is_ok();
        let _ = tx.send(Event::default().event("done").data(serde_json::json!({"ok": error.is_none() && audited, "error": if audited { error } else { Some("audit_failed") }}).to_string())).await;
    });
    Ok(Sse::new(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (Ok(event), rx))
    })))
}

#[cfg(test)]
pub(crate) fn expire_for_test(s: &AppState, id: &str) {
    s.0.cli_installs
        .pending
        .lock()
        .get_mut(&Uuid::parse_str(id).unwrap())
        .unwrap()
        .expires = 0;
}
