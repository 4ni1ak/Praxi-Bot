//! `/v1/bots/{id}/site-rules*`: the owner's per-site and per-app permission rules
//! (issue #227).
//!
//! * Creating and removing a rule needs the owner key (see `owner.rs`) and is refused
//!   to paired devices: an `allow` rule is an explicit owner grant.
//! * Every change is an audit-log event written before the rule takes effect (see
//!   `agent_core::AgentLoop::add_profile_rule`).
//! * Rules are not part of the bot profile file, so profile sharing and export never
//!   carry them.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use permission_engine::profiles::NewRule;
use permission_engine::{Capability, ProfileAction, ProfileRule, SubjectKind};
use protocol::BotId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{ApiError, ErrorBody};
use crate::routes::ApiJson;

/// One rule as the owner sees it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SiteRuleDto {
    /// Rule id (use it to remove the rule).
    pub id: String,
    /// `site` or `app`.
    pub kind: String,
    /// Host name (sites) or program path (apps).
    pub key: String,
    /// Sites only: the rule also covers every subdomain.
    pub include_subdomains: bool,
    /// `read` (opening the site), `interact` (click, type, pick), `submit` (key presses,
    /// page scripts) or `any`.
    pub capability: String,
    /// `block`, `ask` or `allow`.
    pub action: String,
    /// When the rule was created (ms since the Unix epoch).
    pub created_at_ms: i64,
    /// When the rule stops applying, if it expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    /// The expiry has passed: the rule no longer applies.
    pub expired: bool,
}

/// `POST /v1/bots/{id}/site-rules`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SiteRuleInput {
    /// `site` or `app`.
    pub kind: String,
    /// A plain host name (`example.com`, no `https://`, port or path) or an absolute
    /// program path.
    pub key: String,
    /// Sites only, and never with `allow`: also cover every subdomain.
    #[serde(default)]
    pub include_subdomains: bool,
    /// `read`, `interact`, `submit` or `any` (`any` is not allowed with `allow`).
    pub capability: String,
    /// `block`, `ask` or `allow`.
    pub action: String,
    /// The rule stops applying after this many hours (omit for no expiry).
    #[serde(default)]
    pub expires_in_hours: Option<u32>,
}

/// A site or program a recent approval was for ("add from a recent approval").
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SiteRuleSuggestionDto {
    /// `site` or `app`.
    pub kind: String,
    /// Host name or program path.
    pub key: String,
    /// The class of the approved call.
    pub capability: String,
    /// When it was approved (ms since the Unix epoch).
    pub approved_at_ms: i64,
}

fn dto(r: &ProfileRule, now_ms: i64) -> SiteRuleDto {
    SiteRuleDto {
        id: r.id.clone(),
        kind: r.kind.as_str().to_owned(),
        key: r.key.clone(),
        include_subdomains: r.include_subdomains,
        capability: r.capability.as_str().to_owned(),
        action: r.action.as_str().to_owned(),
        created_at_ms: r.created_at_ms,
        expires_at_ms: r.expires_at_ms,
        expired: !r.active(now_ms),
    }
}

fn bad(msg: &str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "bad_profile", msg)
}

fn bot_id(raw: &str) -> Result<BotId, ApiError> {
    raw.parse::<uuid::Uuid>()
        .map(BotId)
        .map_err(|_| ApiError::not_found("bot"))
}

/// The rules of one bot.
#[utoipa::path(get, path = "/v1/bots/{id}/site-rules", tag = "bots", security(("bearer" = [])),
    params(("id" = uuid::Uuid, Path, description = "Bot id")),
    responses((status = 200, body = Vec<SiteRuleDto>), (status = 404, body = ErrorBody)))]
pub(crate) async fn list_site_rules(
    State(s): State<AppState>,
    Path(bid): Path<String>,
) -> Result<Json<Vec<SiteRuleDto>>, ApiError> {
    let bot = s.bot_of_user(bot_id(&bid)?)?;
    let now = storage::now_ms();
    Ok(Json(
        s.agent()
            .list_profile_rules(bot.id)
            .iter()
            .map(|r| dto(r, now))
            .collect(),
    ))
}

/// Add a rule. Owner key required.
#[utoipa::path(post, path = "/v1/bots/{id}/site-rules", tag = "bots", security(("bearer" = [])),
    params(("id" = uuid::Uuid, Path, description = "Bot id")),
    request_body = SiteRuleInput,
    responses((status = 201, body = SiteRuleDto), (status = 404, body = ErrorBody),
              (status = 422, body = ErrorBody, description = "the rule is not valid")))]
pub(crate) async fn add_site_rule(
    State(s): State<AppState>,
    Path(bid): Path<String>,
    ApiJson(input): ApiJson<SiteRuleInput>,
) -> Result<(StatusCode, Json<SiteRuleDto>), ApiError> {
    let bot = s.bot_of_user(bot_id(&bid)?)?;
    let kind = match input.kind.as_str() {
        "site" => SubjectKind::Site,
        "app" => SubjectKind::App,
        _ => return Err(bad("kind must be site or app")),
    };
    let capability = match input.capability.as_str() {
        "read" => Capability::Read,
        "interact" => Capability::Interact,
        "submit" => Capability::Submit,
        "any" => Capability::Any,
        _ => return Err(bad("capability must be read, interact, submit or any")),
    };
    let action = match input.action.as_str() {
        "block" => ProfileAction::Block,
        "ask" => ProfileAction::Ask,
        "allow" => ProfileAction::Allow,
        _ => return Err(bad("action must be block, ask or allow")),
    };
    let now = storage::now_ms();
    let expires_at_ms = input
        .expires_in_hours
        .map(|h| now.saturating_add(i64::from(h.clamp(1, 24 * 365)) * 3_600_000));
    let rule = s.agent().add_profile_rule(
        bot.id,
        &NewRule {
            kind,
            raw_key: &input.key,
            include_subdomains: input.include_subdomains,
            capability,
            action,
            expires_at_ms,
        },
    )?;
    Ok((StatusCode::CREATED, Json(dto(&rule, now))))
}

/// Remove a rule. Owner key required.
#[utoipa::path(delete, path = "/v1/bots/{id}/site-rules/{rule_id}", tag = "bots", security(("bearer" = [])),
    params(("id" = uuid::Uuid, Path, description = "Bot id"),
           ("rule_id" = String, Path, description = "Rule id")),
    responses((status = 204), (status = 404, body = ErrorBody)))]
pub(crate) async fn remove_site_rule(
    State(s): State<AppState>,
    Path((bid, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let bot = s.bot_of_user(bot_id(&bid)?)?;
    if s.agent().remove_profile_rule(bot.id, &rule_id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("site rule"))
    }
}

/// Sites and programs this bot's recent approvals were for, newest first.
#[utoipa::path(get, path = "/v1/bots/{id}/site-rules/recent", tag = "bots", security(("bearer" = [])),
    params(("id" = uuid::Uuid, Path, description = "Bot id")),
    responses((status = 200, body = Vec<SiteRuleSuggestionDto>), (status = 404, body = ErrorBody)))]
pub(crate) async fn recent_site_approvals(
    State(s): State<AppState>,
    Path(bid): Path<String>,
) -> Result<Json<Vec<SiteRuleSuggestionDto>>, ApiError> {
    let bot = s.bot_of_user(bot_id(&bid)?)?;
    Ok(Json(
        s.agent()
            .recent_approval_subjects(bot.id)?
            .into_iter()
            .map(|a| SiteRuleSuggestionDto {
                kind: a.kind.as_str().to_owned(),
                key: a.key,
                capability: a.capability.as_str().to_owned(),
                approved_at_ms: a.approved_at_ms,
            })
            .collect(),
    ))
}
