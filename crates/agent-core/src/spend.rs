//! Monthly spending limit (per installation, default off).
//!
//! The limit is checked before every model call in [`crate::usage::CallGuard::start`],
//! the one place all paid calls pass through (foreground, scheduled, subagent,
//! memory and router calls alike). Month-to-date spend is the sum of the
//! `cost_usd` the usage ledger recorded for calls that finished in the current
//! local calendar month. Calls whose model has no known price record
//! `cost_usd: null`: they are not counted and never guessed, and a call to such a
//! model is not blocked (it cannot add a known cost).

use chrono::{DateTime, Datelike, Local, TimeZone};
use parking_lot::{Mutex, RwLock};
use protocol::{RunId, UserId};
use storage::Storage;

/// Stable code at the start of the run error message when the limit stops a call.
pub const SPEND_LIMIT_REACHED: &str = "spend_limit_reached";

/// Smallest limit the API accepts (USD).
pub const MIN_LIMIT_USD: f64 = 1.0;
/// Largest limit the API accepts (USD); keeps the value sane, not a policy.
pub const MAX_LIMIT_USD: f64 = 1_000_000.0;

/// Prompt size assumed for the first call of a run (no earlier call to learn from).
const DEFAULT_PROMPT_TOKENS: u64 = 4_000;
/// Completion cap assumed when the loop config sets no `max_tokens`.
const DEFAULT_COMPLETION_TOKENS: u64 = 4_096;

/// Live limit and the "already warned this month" marker.
#[derive(Default)]
pub(crate) struct SpendState {
    pub limit: RwLock<Option<f64>>,
    pub warned_month: Mutex<Option<i64>>,
}

/// Month-to-date accounting.
#[derive(Debug, Clone, PartialEq)]
pub struct SpendStatus {
    /// Start of the local calendar month, ms since epoch.
    pub month_start_ms: i64,
    /// Start of the next local month, ms since epoch.
    pub month_end_ms: i64,
    /// Sum of recorded `cost_usd` this month.
    pub spent_usd: f64,
    /// Finished calls this month with unknown price (not counted).
    pub unpriced_calls: u64,
}

/// `[start, end)` of the local calendar month containing `now_ms`.
#[must_use]
pub fn month_bounds_ms(now_ms: i64) -> (i64, i64) {
    let now: DateTime<Local> = Local
        .timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_else(Local::now);
    let first = |y: i32, m: u32| {
        Local
            .with_ymd_and_hms(y, m, 1, 0, 0, 0)
            .earliest()
            .map_or(0, |d| d.timestamp_millis())
    };
    let (y, m) = (now.year(), now.month());
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (first(y, m), first(ny, nm))
}

/// Linear projection of month-end spend, `None` before any time has elapsed or
/// any spend was recorded.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn forecast_usd(status: &SpendStatus, now_ms: i64) -> Option<f64> {
    let elapsed = (now_ms - status.month_start_ms) as f64;
    let total = (status.month_end_ms - status.month_start_ms) as f64;
    (elapsed > 0.0 && total > 0.0 && status.spent_usd > 0.0)
        .then(|| status.spent_usd / elapsed * total)
}

/// Month-to-date spend of `user` from the usage ledger.
///
/// # Errors
/// Fails on a database error.
pub fn spend_status(st: &Storage, user: UserId, now_ms: i64) -> storage::Result<SpendStatus> {
    let (month_start_ms, month_end_ms) = month_bounds_ms(now_ms);
    let mut spent_usd = 0.0;
    let mut unpriced_calls = 0;
    for row in st.list_audit_action_since(user, "usage", month_start_ms)? {
        if row.detail["ledger_version"] != 1 {
            continue;
        }
        let cost = row.detail["cost_usd"].as_f64().filter(|c| c.is_finite());
        if let Some(c) = cost {
            spent_usd += c.max(0.0);
        } else if row.detail["tokens"].is_object() {
            // Only calls that reported tokens but have no price are "unpriced";
            // interrupted calls (no tokens) carry no cost information at all.
            unpriced_calls += 1;
        }
    }
    Ok(SpendStatus {
        month_start_ms,
        month_end_ms,
        spent_usd,
        unpriced_calls,
    })
}

/// Conservative cost of the next call: the largest prompt this run already
/// sent (or a default) plus the full completion cap, at the model's price.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn estimate_usd(
    st: &Storage,
    user: UserId,
    run: RunId,
    price: crate::ModelPrice,
    max_tokens: Option<u32>,
) -> f64 {
    let prompt = st
        .list_audit_for_run(user, run)
        .ok()
        .and_then(|rows| {
            rows.iter()
                .filter(|r| r.action == "usage")
                .filter_map(|r| r.detail["prompt_tokens"].as_u64())
                .max()
        })
        .unwrap_or(DEFAULT_PROMPT_TOKENS);
    let completion = max_tokens.map_or(DEFAULT_COMPLETION_TOKENS, u64::from);
    price.cost(prompt, completion, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_bounds_contain_now_and_forecast_is_linear() {
        let now = Local::now().timestamp_millis();
        let (start, end) = month_bounds_ms(now);
        assert!(start <= now && now < end);
        let status = SpendStatus {
            month_start_ms: 0,
            month_end_ms: 1_000,
            spent_usd: 10.0,
            unpriced_calls: 0,
        };
        let f = forecast_usd(&status, 250).unwrap();
        assert!((f - 40.0).abs() < 1e-9, "{f}");
        assert!(forecast_usd(&status, 0).is_none());
    }
}
