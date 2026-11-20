//! Monthly spending limit (`/v1/spend-limit`), per installation, default off.
//!
//! Enforcement lives in `agent-core` (`spend.rs`, checked before every model
//! call). This module persists the owner's value in the data dir and reports
//! month-to-date spend. Changing the limit is owner-only (see `owner.rs`).

use std::path::Path;

use agent_core::spend::{MAX_LIMIT_USD, MIN_LIMIT_USD, forecast_usd};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

const LIMIT_FILE: &str = "spend-limit.json";

/// Month-to-date spend against the monthly limit.
#[derive(Debug, Serialize, ToSchema)]
pub struct SpendLimitDto {
    /// Monthly limit in USD; `null` = no limit.
    pub limit_usd: Option<f64>,
    /// Start of the current local calendar month (ms since epoch).
    pub month_start_ms: i64,
    /// Start of the next month (ms since epoch).
    pub month_end_ms: i64,
    /// Now, as the daemon sees it (ms since epoch).
    pub now_ms: i64,
    /// Recorded cost this month (USD). Calls with unknown pricing are not counted.
    pub spent_usd: f64,
    /// `limit_usd - spent_usd`, never negative; `null` without a limit.
    pub remaining_usd: Option<f64>,
    /// Linear projection of month-end spend (USD); `null` until something was spent.
    pub forecast_usd: Option<f64>,
    /// Finished calls this month whose model has no known price (not counted).
    pub unpriced_calls: u64,
}

/// `PUT /v1/spend-limit`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SpendLimitInput {
    /// New monthly limit in USD (1 to 1,000,000); `null` removes the limit.
    pub limit_usd: Option<f64>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    limit_usd: f64,
}

/// The saved limit, if any. A missing or unreadable file means no limit.
pub(crate) fn load_limit(data_dir: &Path) -> Option<f64> {
    let raw = std::fs::read(data_dir.join(LIMIT_FILE)).ok()?;
    let s: Stored = serde_json::from_slice(&raw).ok()?;
    valid(s.limit_usd).then_some(s.limit_usd)
}

fn valid(v: f64) -> bool {
    v.is_finite() && (MIN_LIMIT_USD..=MAX_LIMIT_USD).contains(&v)
}

static SAVE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn save_limit(data_dir: &Path, limit: Option<f64>) -> std::io::Result<()> {
    let _g = SAVE_LOCK.lock();
    let path = data_dir.join(LIMIT_FILE);
    let Some(limit_usd) = limit else {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    };
    let tmp = path.with_extension("json.tmp");
    let result = std::fs::write(&tmp, serde_json::to_vec(&Stored { limit_usd })?)
        .and_then(|()| std::fs::rename(&tmp, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn dto(s: &AppState) -> Result<SpendLimitDto, ApiError> {
    let status = s.agent().spend_status(s.user_id())?;
    let limit_usd = s.agent().spend_limit();
    let now_ms = storage::now_ms();
    Ok(SpendLimitDto {
        limit_usd,
        month_start_ms: status.month_start_ms,
        month_end_ms: status.month_end_ms,
        now_ms,
        spent_usd: status.spent_usd,
        remaining_usd: limit_usd.map(|l| (l - status.spent_usd).max(0.0)),
        forecast_usd: forecast_usd(&status, now_ms),
        unpriced_calls: status.unpriced_calls,
    })
}

/// The monthly spending limit and this month's spend.
#[utoipa::path(get, path = "/v1/spend-limit", tag = "usage", security(("bearer" = [])),
    responses((status = 200, body = SpendLimitDto)))]
pub(crate) async fn get_spend_limit(
    State(s): State<AppState>,
) -> Result<Json<SpendLimitDto>, ApiError> {
    Ok(Json(dto(&s)?))
}

/// Set or remove the monthly spending limit (owner only). Effective for the
/// next model call of every run, including background and scheduled runs.
#[utoipa::path(put, path = "/v1/spend-limit", tag = "usage", security(("bearer" = [])),
    request_body = SpendLimitInput,
    responses((status = 200, body = SpendLimitDto), (status = 400, body = ErrorBody)))]
pub(crate) async fn put_spend_limit(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<SpendLimitInput>,
) -> Result<Json<SpendLimitDto>, ApiError> {
    if let Some(v) = input.limit_usd
        && !valid(v)
    {
        return Err(ApiError::bad_request(format!(
            "the limit must be between ${MIN_LIMIT_USD} and ${MAX_LIMIT_USD} per month"
        )));
    }
    save_limit(&s.data_dir(), input.limit_usd).map_err(ApiError::internal)?;
    s.agent().set_spend_limit(input.limit_usd);
    Ok(Json(dto(&s)?))
}
