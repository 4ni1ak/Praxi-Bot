//! Moving schedules and chat history to a host (card 270-B).
//!
//! The owner's computer (the LOCAL daemon, owner key required, see `host_copy.rs`) sends
//! schedules, triggers, chats, run metadata and messages of the bots it moved to the host's
//! administration routes below. They need exactly what the rest of the copy flow needs: a
//! paired desktop that holds host administration (`host_admin::require_admin`); a phone, a
//! node, a bot or a desktop without the capability is refused.
//!
//! What travels, and what deliberately does not:
//! * schedules keep their id (so a second sync finds them and never duplicates), their paused
//!   state and their time zone (which is part of the cron spec, `0 9 * * * Europe/Istanbul`;
//!   a spec without one means UTC on every computer, so a host in another zone fires at the
//!   same instants). Next-run times are recomputed on the host's clock. A schedule whose bot
//!   did not move is skipped. Webhook secrets never travel (the host must set its own).
//! * chats and messages keep ids, authors (role), timestamps and exposure labels. Message text
//!   goes through the redactor first. Runs travel as metadata only (kind, state, times): no
//!   error text, no spawn parameters, no computer identifiers. Files and attachments that a
//!   message merely names by path are NOT copied.
//! * memory has no export path in the daemon and is not moved.
//!
//! Logs and audit rows carry counts and ids, never text.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use axum::{Extension, Json, extract::State};
use protocol::{BotId, ChatId, RunId, TaskId};
use serde::{Deserialize, Serialize};
use storage::{Chat, Message, Run};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    ApiError, AppState, PeerIdentity,
    dto::ScheduleInput,
    host_admin::{Gate, audit, require_admin},
    host_copy::CopyItemResult,
    host_links::HostLinks,
    routes::ApiJson,
};

/// Most schedules, chats, runs and messages one import call carries.
const MAX_SCHEDULES: usize = 4;
const MAX_CHATS: usize = 8;
const MAX_RUNS: usize = 32;
const MAX_MESSAGES: usize = 48;
/// Longest message text that is moved; a longer one is counted as skipped.
pub(crate) const MAX_CONTENT: usize = 24 * 1024;
/// Serialised size one page may reach (the admin body limit is 64 KiB).
const PAGE_BYTES: usize = 40 * 1024;
/// Most messages one move sends (the admin API allows 60 calls a minute).
const MAX_MESSAGES_TOTAL: usize = 4000;
const MAX_SCHEDULES_TOTAL: usize = 200;

// ------------------------------------------------------------------ host side

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportScheduleItem {
    /// Preserved: the same id on the host means "already moved".
    pub id: Uuid,
    /// The bot's id ON THE HOST.
    pub bot_id: Uuid,
    /// A chat that was moved too (host id = local id); dropped when the host lacks it.
    #[serde(default)]
    pub chat_id: Option<Uuid>,
    pub name: String,
    /// `{"cron": "0 9 * * 1-5 Europe/Istanbul"}`: the zone travels inside the spec.
    pub spec: serde_json::Value,
    pub payload: serde_json::Value,
    /// Paused state as the owner had it.
    pub enabled: bool,
    #[serde(default)]
    pub misfire_policy: Option<String>,
    #[serde(default)]
    pub precheck: Option<serde_json::Value>,
    /// File-watch or e-mail trigger; never a webhook (its secret stays behind).
    #[serde(default)]
    pub trigger: Option<serde_json::Value>,
    pub created_at_ms: i64,
    /// `user` or `bot`.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSchedulesInput {
    pub schedules: Vec<ImportScheduleItem>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ImportScheduleResult {
    pub id: Uuid,
    /// `done`, `exists` or `failed`.
    pub status: String,
    pub code: Option<String>,
    /// Next firing, recomputed on the host's clock.
    pub next_run_at_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ImportSchedulesReport {
    pub items: Vec<ImportScheduleResult>,
}

fn result(id: Uuid, status: &str, code: Option<&str>, next: Option<i64>) -> ImportScheduleResult {
    ImportScheduleResult {
        id,
        status: status.into(),
        code: code.map(str::to_owned),
        next_run_at_ms: next,
    }
}

#[utoipa::path(post, operation_id = "admin_import_schedules", path = "/v1/admin/import/schedules", tag = "admin", security(("bearer" = [])),
    request_body = ImportSchedulesInput,
    responses((status = 200, body = ImportSchedulesReport), (status = 403, body = crate::error::ErrorBody)))]
pub(crate) async fn import_schedules(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    ApiJson(input): ApiJson<ImportSchedulesInput>,
) -> Result<Json<ImportSchedulesReport>, ApiError> {
    let admin = require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    if input.schedules.len() > MAX_SCHEDULES {
        return Err(ApiError::bad_request("too many schedules in one call"));
    }
    let store = s.schedule_store()?;
    let mut items = Vec::new();
    for item in input.schedules {
        let id = TaskId(item.id);
        if store.get(id)?.is_some() {
            items.push(result(item.id, "exists", None, None));
            continue;
        }
        if s.bot_of_user(BotId(item.bot_id)).is_err() {
            items.push(result(item.id, "failed", Some("bot_not_on_host"), None));
            continue;
        }
        let origin = item
            .origin
            .as_deref()
            .and_then(scheduler::ScheduleOrigin::parse)
            .unwrap_or_default();
        let wire = ScheduleInput {
            bot_id: item.bot_id,
            chat_id: item.chat_id.filter(|c| s.chat_of_user(ChatId(*c)).is_ok()),
            name: item.name,
            spec: item.spec,
            payload: item.payload,
            enabled: Some(item.enabled),
            misfire_policy: item.misfire_policy,
            webhook_secret: None,
            trigger: item.trigger,
            precheck: item.precheck,
        };
        match crate::routes::import_schedule(&s, &wire, id, item.created_at_ms, origin).await {
            Ok(sc) => items.push(result(item.id, "done", None, sc.next_run_at)),
            Err(e) => items.push(result(item.id, "failed", Some(&e.code_str()), None)),
        }
    }
    audit(
        &s,
        &admin,
        "host.admin.import_schedules",
        &serde_json::json!({
            "done": items.iter().filter(|i| i.status == "done").count(),
            "exists": items.iter().filter(|i| i.status == "exists").count(),
            "failed": items.iter().filter(|i| i.status == "failed").count(),
        }),
    )?;
    Ok(Json(ImportSchedulesReport { items }))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportChat {
    pub id: Uuid,
    /// The bot's id ON THE HOST.
    pub bot_id: Uuid,
    pub title: String,
    pub created_at_ms: i64,
}

/// Run metadata only: no error text, no spawn parameters, no computer identity.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRun {
    pub id: Uuid,
    pub bot_id: Uuid,
    pub chat_id: Option<Uuid>,
    pub kind: String,
    /// `succeeded`, `failed` or `cancelled`; anything else arrives as `cancelled`.
    pub state: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportMessage {
    pub id: Uuid,
    pub chat_id: Uuid,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    /// The author: `user`, `assistant`, `system`, `tool`, ...
    pub role: String,
    pub content: String,
    pub created_at_ms: i64,
    #[serde(default)]
    pub exposure_untrusted: bool,
    #[serde(default)]
    pub exposure_private: bool,
    #[serde(default)]
    pub exposure_sources: Vec<String>,
}

/// One page of a chat move. Chats first, then runs, then messages: send the pages in that order.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportChatsInput {
    #[serde(default)]
    pub chats: Vec<ImportChat>,
    #[serde(default)]
    pub runs: Vec<ImportRun>,
    #[serde(default)]
    pub messages: Vec<ImportMessage>,
}

/// What a page did. Counts only.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct ImportChatsReport {
    pub chats: u32,
    pub runs: u32,
    pub messages: u32,
    /// Items the host already had (an earlier or interrupted sync).
    pub existing: u32,
    /// Items refused: unknown bot or chat, malformed, too long.
    pub rejected: u32,
}

fn plain(text: &str, max: usize) -> bool {
    text.len() <= max
        && !text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

#[utoipa::path(post, operation_id = "admin_import_chats", path = "/v1/admin/import/chats", tag = "admin", security(("bearer" = [])),
    request_body = ImportChatsInput,
    responses((status = 200, body = ImportChatsReport), (status = 403, body = crate::error::ErrorBody)))]
#[allow(clippy::too_many_lines)] // three short insert loops over one report
pub(crate) async fn import_chats(
    State(s): State<AppState>,
    peer: Option<Extension<PeerIdentity>>,
    _gate: Gate,
    ApiJson(input): ApiJson<ImportChatsInput>,
) -> Result<Json<ImportChatsReport>, ApiError> {
    let admin = require_admin(&s, peer.as_ref().map(|p| &p.0))?;
    if input.chats.len() > MAX_CHATS
        || input.runs.len() > MAX_RUNS
        || input.messages.len() > MAX_MESSAGES
    {
        return Err(ApiError::bad_request("page too large"));
    }
    let storage = s.storage();
    let user = s.user_id();
    let mut report = ImportChatsReport::default();
    for c in input.chats {
        if s.bot_of_user(BotId(c.bot_id)).is_err() || !plain(&c.title, 512) {
            report.rejected += 1;
        } else if storage.get_chat(ChatId(c.id))?.is_some() {
            report.existing += 1;
        } else {
            storage.insert_chat(&Chat {
                id: ChatId(c.id),
                user_id: user,
                bot_id: BotId(c.bot_id),
                title: c.title,
                created_at_ms: c.created_at_ms,
            })?;
            report.chats += 1;
        }
    }
    for r in input.runs {
        let chat = r.chat_id.map(ChatId);
        let chat_ok = chat.is_none_or(|c| s.chat_of_user(c).is_ok());
        if s.bot_of_user(BotId(r.bot_id)).is_err() || !chat_ok || !plain(&r.kind, 64) {
            report.rejected += 1;
        } else if storage.get_run(RunId(r.id))?.is_some() {
            report.existing += 1;
        } else {
            let state = match r.state.as_str() {
                "succeeded" | "failed" | "cancelled" => r.state,
                _ => "cancelled".to_owned(),
            };
            storage.insert_run(&Run {
                id: RunId(r.id),
                user_id: user,
                bot_id: BotId(r.bot_id),
                chat_id: chat,
                kind: r.kind,
                state,
                error: None,
                created_at_ms: r.created_at_ms,
                updated_at_ms: r.updated_at_ms,
                parent_run_id: None,
                spec: None,
                computer_target: None,
                computer_node: None,
                computer_node_name: None,
                paused_at_ms: None,
                pause_reason: None,
            })?;
            report.runs += 1;
        }
    }
    for m in input.messages {
        if s.chat_of_user(ChatId(m.chat_id)).is_err()
            || m.content.len() > MAX_CONTENT
            || !plain(&m.role, 32)
            || m.exposure_sources.len() > 16
            || m.exposure_sources.iter().any(|x| !plain(x, 256))
        {
            report.rejected += 1;
        } else if storage.get_message(m.id)?.is_some() {
            report.existing += 1;
        } else {
            let run_id = m
                .run_id
                .map(RunId)
                .filter(|r| storage.get_run(*r).ok().flatten().is_some());
            storage.insert_message(&Message {
                id: m.id,
                user_id: user,
                chat_id: ChatId(m.chat_id),
                run_id,
                role: m.role,
                // The host's own redactor, on top of the sender's.
                content: s.0.agent.redact_host_text(m.content),
                created_at_ms: m.created_at_ms,
                exposure_untrusted: m.exposure_untrusted,
                exposure_private: m.exposure_private,
                exposure_sources: m.exposure_sources,
            })?;
            report.messages += 1;
        }
    }
    audit(
        &s,
        &admin,
        "host.admin.import_chats",
        &serde_json::json!({"chats": report.chats, "runs": report.runs,
            "messages": report.messages, "existing": report.existing, "rejected": report.rejected}),
    )?;
    Ok(Json(report))
}

// ----------------------------------------------------------------- local side

const MOVED_FILE: &str = "moved-schedules.json";

/// The mark a moved schedule keeps on this computer: it is disabled (not deleted) and shows
/// "moved to <server>"; `was_enabled` is what the owner had, so a later sync stays faithful.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Moved {
    host_id: String,
    host_name: String,
    was_enabled: bool,
}

fn read_moved(data: &Path) -> BTreeMap<String, Moved> {
    std::fs::read(data.join(MOVED_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_moved(data: &Path, moved: &BTreeMap<String, Moved>) -> std::io::Result<()> {
    let tmp = data.join(format!("{MOVED_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(moved)?)?;
    std::fs::rename(tmp, data.join(MOVED_FILE))
}

/// The server this local schedule was moved to, if any.
pub(crate) fn moved_to(data: &Path, schedule: Uuid) -> Option<String> {
    read_moved(data)
        .remove(&schedule.to_string())
        .map(|m| m.host_name)
}

/// `POST` with a pause and a retry when the host's administration budget is used up.
async fn post(
    links: &dyn HostLinks,
    host: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    for attempt in 0..4 {
        let call = links
            .call(host, "POST", path, Some(body.clone()))
            .await
            .map_err(|_| "host_unreachable".to_owned())?;
        if (200..300).contains(&call.status) {
            return Ok(call.body);
        }
        let code = call.body["error"]["code"]
            .as_str()
            .unwrap_or("rejected")
            .to_owned();
        if code == "admin_rate_limit" && attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_secs(61)).await;
            continue;
        }
        return Err(code);
    }
    Err("admin_rate_limit".into())
}

fn counts(kind: &str, done: u32, existing: u32, skipped: u32, failed: u32) -> CopyItemResult {
    CopyItemResult {
        kind: kind.into(),
        id: "all".into(),
        name: "all".into(),
        status: if failed > 0 { "failed" } else { "done" }.into(),
        code: None,
        count: Some(done),
        existing: Some(existing),
        skipped: Some(skipped),
        failed: Some(failed),
    }
}

fn failed_item(kind: &str, code: &str, done: u32, existing: u32, skipped: u32) -> CopyItemResult {
    let mut r = counts(kind, done, existing, skipped, 1);
    r.code = Some(code.to_owned());
    r
}

/// Send the chats (with run metadata and messages) of the moved bots. Idempotent and
/// resumable: a host that already has an item counts it as existing, so an interrupted move
/// is finished by running it again. Returns the report line and the chats the host now has.
#[allow(clippy::too_many_lines)] // one ordered export: chats, runs, then message pages
pub(crate) async fn send_chats(
    s: &AppState,
    links: &dyn HostLinks,
    host: &str,
    bots: &HashMap<Uuid, Uuid>,
) -> (CopyItemResult, HashSet<Uuid>) {
    let (mut done, mut existing, mut skipped) = (0u32, 0u32, 0u32);
    let mut chats_on_host = HashSet::new();
    let storage = s.storage();
    let all_runs = storage.list_runs(s.user_id()).unwrap_or_default();
    let Ok(chats) = storage.list_chats(s.user_id()) else {
        return (failed_item("chats", "unreadable", 0, 0, 0), chats_on_host);
    };
    let mut budget = MAX_MESSAGES_TOTAL;
    for chat in chats.into_iter().filter(|c| bots.contains_key(&c.bot_id.0)) {
        let hbot = bots[&chat.bot_id.0];
        let runs: Vec<_> = all_runs
            .iter()
            .filter(|r| r.chat_id == Some(chat.id) && bots.contains_key(&r.bot_id.0))
            .collect();
        let head = ImportChatsInput {
            chats: vec![ImportChat {
                id: chat.id.0,
                bot_id: hbot,
                title: s.0.agent.redact_host_text(chat.title.clone()),
                created_at_ms: chat.created_at_ms,
            }],
            ..ImportChatsInput::default()
        };
        let mut pages = vec![head];
        for part in runs.chunks(MAX_RUNS) {
            pages.push(ImportChatsInput {
                runs: part
                    .iter()
                    .map(|r| ImportRun {
                        id: r.id.0,
                        bot_id: bots[&r.bot_id.0],
                        chat_id: Some(chat.id.0),
                        kind: r.kind.clone(),
                        state: r.state.clone(),
                        created_at_ms: r.created_at_ms,
                        updated_at_ms: r.updated_at_ms,
                    })
                    .collect(),
                ..ImportChatsInput::default()
            });
        }
        let run_ids: HashSet<Uuid> = runs.iter().map(|r| r.id.0).collect();
        let mut page = ImportChatsInput::default();
        let mut bytes = 0usize;
        let messages = storage.list_messages(chat.id).unwrap_or_default();
        for m in messages {
            if m.content.len() > MAX_CONTENT || budget == 0 {
                skipped += 1;
                continue;
            }
            budget -= 1;
            let item = ImportMessage {
                id: m.id,
                chat_id: chat.id.0,
                run_id: m.run_id.map(|r| r.0).filter(|r| run_ids.contains(r)),
                role: m.role,
                content: s.0.agent.redact_host_text(m.content),
                created_at_ms: m.created_at_ms,
                exposure_untrusted: m.exposure_untrusted,
                exposure_private: m.exposure_private,
                exposure_sources: m.exposure_sources,
            };
            let size = item.content.len() + 400;
            if !page.messages.is_empty()
                && (bytes + size > PAGE_BYTES || page.messages.len() >= MAX_MESSAGES)
            {
                pages.push(std::mem::take(&mut page));
                bytes = 0;
            }
            bytes += size;
            page.messages.push(item);
        }
        if !page.messages.is_empty() {
            pages.push(page);
        }
        for page in pages {
            let Ok(body) = serde_json::to_value(&page) else {
                continue;
            };
            match post(links, host, "/v1/admin/import/chats", body).await {
                Ok(v) => {
                    let n = |k: &str| u32::try_from(v[k].as_u64().unwrap_or(0)).unwrap_or(0);
                    done += n("messages");
                    existing += n("existing");
                    skipped += n("rejected");
                }
                Err(code) => {
                    return (
                        failed_item("chats", &code, done, existing, skipped),
                        chats_on_host,
                    );
                }
            }
        }
        chats_on_host.insert(chat.id.0);
    }
    (counts("chats", done, existing, skipped, 0), chats_on_host)
}

/// Send the schedules (and their triggers) of the moved bots, then disable the local copies
/// and mark them "moved to <server>" so nothing fires twice. Idempotent: a schedule the host
/// already has is "exists", and the local copy is simply left disabled.
#[allow(clippy::too_many_lines)] // one ordered pass: build, send, disable, mark
pub(crate) async fn send_schedules(
    s: &AppState,
    links: &dyn HostLinks,
    host: &str,
    host_name: &str,
    bots: &HashMap<Uuid, Uuid>,
    chats_on_host: &HashSet<Uuid>,
) -> CopyItemResult {
    let Ok(store) = s.schedule_store() else {
        return failed_item("schedules", "unreadable", 0, 0, 0);
    };
    let Ok(all) = store.list() else {
        return failed_item("schedules", "unreadable", 0, 0, 0);
    };
    let mut moved = read_moved(&s.data_dir());
    let (mut done, mut existing, mut skipped, mut failed) = (0u32, 0u32, 0u32, 0u32);
    let mut stop: Option<String> = None;
    for sc in all
        .into_iter()
        .filter(|sc| bots.contains_key(&sc.bot_id.0))
        .take(MAX_SCHEDULES_TOTAL)
    {
        let trigger = store.get_trigger(sc.id).ok().flatten();
        if matches!(trigger, Some(scheduler::Trigger::Webhook { .. })) {
            // The shared secret is not copied; the owner sets a new one on the server.
            skipped += 1;
            continue;
        }
        let key = sc.id.0.to_string();
        let was_enabled = moved.get(&key).map_or(sc.enabled, |m| m.was_enabled);
        let item = ImportScheduleItem {
            id: sc.id.0,
            bot_id: bots[&sc.bot_id.0],
            chat_id: sc
                .chat_id
                .map(|c| c.0)
                .filter(|c| chats_on_host.contains(c)),
            name: sc.name.clone(),
            spec: serde_json::to_value(&sc.spec).unwrap_or_default(),
            payload: serde_json::to_value(&sc.payload).unwrap_or_default(),
            enabled: was_enabled,
            misfire_policy: Some(sc.misfire_policy.as_str().to_owned()),
            precheck: sc
                .precheck
                .as_ref()
                .and_then(|p| serde_json::to_value(p).ok()),
            trigger: trigger.and_then(|t| serde_json::to_value(t).ok()),
            created_at_ms: sc.created_at,
            origin: Some(sc.origin.as_str().to_owned()),
        };
        if let Some(code) = &stop {
            let _ = code;
            skipped += 1;
            continue;
        }
        let body = serde_json::json!({"schedules": [item]});
        let outcome = match post(links, host, "/v1/admin/import/schedules", body).await {
            Ok(v) => v["items"][0]["status"]
                .as_str()
                .unwrap_or("failed")
                .to_owned(),
            Err(code) => {
                stop = Some(code);
                failed += 1;
                continue;
            }
        };
        match outcome.as_str() {
            "done" | "exists" => {
                if outcome == "done" {
                    done += 1;
                } else {
                    existing += 1;
                }
                moved.insert(
                    key,
                    Moved {
                        host_id: host.to_owned(),
                        host_name: host_name.to_owned(),
                        was_enabled,
                    },
                );
                if sc.enabled {
                    // Disabled, not deleted: it can be re-enabled by hand, and nothing runs twice.
                    let _ = store.set_enabled(sc.id, false, None);
                    s.publish_schedule_changed(sc.id.0, crate::dto::ScheduleChangeKind::Toggled);
                }
            }
            _ => failed += 1,
        }
    }
    let _ = write_moved(&s.data_dir(), &moved);
    let mut r = counts("schedules", done, existing, skipped, failed);
    r.code = stop;
    r
}
