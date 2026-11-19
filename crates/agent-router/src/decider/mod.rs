//! The decider: a cheap decision model in front of the main model.
//!
//! A [`Decider`] answers *typed* questions (yes/no with a probability, or one
//! choice out of a fixed list) about a piece of input. It exists to save
//! main-model calls and tokens (tool pre-selection, trivial-turn routing,
//! history trimming), so everything about it is bounded:
//!
//! * **Output**: an [`Answer`] is a number in `0..=1` or one of the option ids
//!   the caller supplied. No free text of the model ever leaves this module;
//!   an answer that does not validate is dropped (the question stays
//!   unanswered and the caller applies its deterministic fallback).
//! * **Time**: one [`DeciderConfig::timeout`] covers the whole fallback chain.
//! * **Budget**: a [`TurnBudget`] caps the calls of one turn.
//! * **Rate**: each backend has RPM / TPM / RPD counters ([`RateLimits`]); a
//!   backend at its limit (or one that answered HTTP 429) is skipped.
//! * **Cache**: identical requests are answered from memory.
//! * **Fallback chain**: backends are tried in order; when none answers the
//!   result is empty and the caller behaves exactly as without a decider.
//!
//! # Security
//! A decider answer is *advisory*. It can narrow what the main model is shown
//! or pick a cheaper model; it is never consulted by the permission engine and
//! cannot approve, allow or execute anything. The input may contain untrusted
//! text (web pages, e-mails, tool output): it is sent as data (`input` /
//! `state`), never as instructions, and because the output is a validated
//! number or option id, a prompt-injected answer can at worst pick a different
//! allowed option, which costs quality but not safety.

pub mod backends;
mod day;
pub mod resolver;
#[cfg(test)]
mod tests;

pub use day::{DayClock, DayCounters, local_day};
pub use resolver::{ModelResolver, ModelRule, pick_free_model};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use backends::{ChatJsonBackend, GeminiDecisions, OpenRouterDecisions};

/// Why a backend call failed. Never contains secrets or response bodies.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeciderError {
    /// The provider said "too many requests" (HTTP 429).
    #[error("rate limited by the provider")]
    RateLimited,
    /// Any other non-success HTTP status.
    #[error("HTTP {0}")]
    Http(u16),
    /// The request could not be sent or the body could not be read.
    #[error("transport: {0}")]
    Transport(String),
    /// The response was not the documented shape.
    #[error("bad response: {0}")]
    BadResponse(String),
}

/// One option of a [`Question::Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// Stable id (`[A-Za-z0-9_-]{1,64}`); the only string an answer may carry.
    pub id: String,
    /// What the option means.
    pub description: String,
}

/// A typed question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// Answered with the probability of "yes".
    YesNo {
        /// The (trusted, caller-written) question.
        instructions: String,
    },
    /// Answered with one of `options`.
    Choice {
        /// The (trusted, caller-written) question.
        instructions: String,
        /// 2..=255 options.
        options: Vec<ChoiceOption>,
    },
}

/// Input of [`Decider::decide`]: shared data plus named questions about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionRequest {
    /// The data the questions are about. May be untrusted; it is never placed
    /// where a backend reads instructions.
    pub input: String,
    /// Questions by key (`[A-Za-z0-9_-]{1,64}`).
    pub questions: BTreeMap<String, Question>,
}

impl DecisionRequest {
    /// A request about `input` without questions yet.
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            questions: BTreeMap::new(),
        }
    }

    /// Add a yes/no question.
    #[must_use]
    pub fn yes_no(mut self, key: &str, instructions: impl Into<String>) -> Self {
        self.questions.insert(
            key.to_owned(),
            Question::YesNo {
                instructions: instructions.into(),
            },
        );
        self
    }

    /// Add a choice question.
    #[must_use]
    pub fn choice(
        mut self,
        key: &str,
        instructions: impl Into<String>,
        options: &[(&str, &str)],
    ) -> Self {
        self.questions.insert(
            key.to_owned(),
            Question::Choice {
                instructions: instructions.into(),
                options: options
                    .iter()
                    .map(|(id, d)| ChoiceOption {
                        id: (*id).to_owned(),
                        description: (*d).to_owned(),
                    })
                    .collect(),
            },
        );
        self
    }

    /// Whether every key and option id is well formed and every choice has
    /// 2..=255 options.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.questions.is_empty()
            && self.questions.len() <= 1000
            && self.questions.iter().all(|(k, q)| {
                valid_id(k)
                    && match q {
                        Question::YesNo { .. } => true,
                        Question::Choice { options, .. } => {
                            (2..=255).contains(&options.len())
                                && options.iter().all(|o| valid_id(&o.id))
                        }
                    }
            })
    }

    /// Rough token estimate for the rate limiter (bytes / 3, rounded up).
    #[must_use]
    pub fn estimated_tokens(&self) -> u64 {
        let q = serde_json::to_string(&self.questions).map_or(0, |s| s.len());
        ((self.input.len() + q) as u64).div_ceil(3)
    }
}

fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    /// Probability of "yes" in `0..=1`.
    YesNo {
        /// Probability of "yes".
        yes: f64,
    },
    /// One of the offered option ids.
    Choice {
        /// The chosen option id (always one the caller supplied).
        choice: String,
        /// Confidence in `0..=1`.
        confidence: f64,
    },
}

/// Validated answers by question key. A missing key means "not answered".
pub type Answers = BTreeMap<String, Answer>;

/// Probability of "yes" for `key`, when answered.
#[must_use]
pub fn yes(answers: &Answers, key: &str) -> Option<f64> {
    match answers.get(key)? {
        Answer::YesNo { yes } => Some(*yes),
        Answer::Choice { .. } => None,
    }
}

/// The chosen option and its confidence for `key`, when answered.
#[must_use]
pub fn choice<'a>(answers: &'a Answers, key: &str) -> Option<(&'a str, f64)> {
    match answers.get(key)? {
        Answer::Choice { choice, confidence } => Some((choice.as_str(), *confidence)),
        Answer::YesNo { .. } => None,
    }
}

fn prob(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|p| p.is_finite())
        .map(|p| p.clamp(0.0, 1.0))
}

/// Keep only the answers that match their question: the right `type`, a
/// finite probability, a choice that is one of the offered ids. Everything
/// else (refusals, unknown keys, free text, extra fields) is dropped.
#[must_use]
pub fn validate(req: &DecisionRequest, raw: &Value) -> Answers {
    let mut out = Answers::new();
    let Some(map) = raw.as_object() else {
        return out;
    };
    for (key, question) in &req.questions {
        let Some(a) = map.get(key).filter(|a| a.is_object()) else {
            continue;
        };
        let ty = a.get("type").and_then(Value::as_str);
        match question {
            Question::YesNo { .. } => {
                if ty == Some("noul")
                    && let Some(yes) = prob(a.get("noul"))
                {
                    out.insert(key.clone(), Answer::YesNo { yes });
                }
            }
            Question::Choice { options, .. } => {
                let chosen = a.get("choice").and_then(Value::as_str);
                if ty == Some("choice")
                    && let Some(c) = chosen
                    && options.iter().any(|o| o.id == c)
                {
                    out.insert(
                        key.clone(),
                        Answer::Choice {
                            choice: c.to_owned(),
                            confidence: prob(a.get("confidence")).unwrap_or(0.0),
                        },
                    );
                }
            }
        }
    }
    out
}

/// A decision model endpoint.
///
/// `decide` returns the raw `answers` object in the Decisions-API shape
/// (`{"<key>": {"type":"noul","noul":0.93} | {"type":"choice","choice":"id",
/// "confidence":0.9}}`); the [`Decider`] validates it with [`validate`].
#[async_trait]
pub trait DecisionBackend: Send + Sync {
    /// `provider/model`, for logs and metrics.
    fn id(&self) -> &str;

    /// Ask the questions of `req`.
    async fn decide(&self, req: &DecisionRequest) -> Result<Value, DeciderError>;
}

/// Limits of one backend; `0` = unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimits {
    /// Requests per minute.
    pub rpm: u32,
    /// (Estimated) input tokens per minute.
    pub tpm: u64,
    /// Requests per day.
    pub rpd: u32,
}

impl RateLimits {
    /// No limits.
    pub const UNLIMITED: Self = Self {
        rpm: 0,
        tpm: 0,
        rpd: 0,
    };
}

/// Day-counter key of all backend requests made today.
pub const CALLS_KEY: &str = "calls";
const MINUTE: Duration = Duration::from_secs(60);
const DAY: Duration = Duration::from_secs(24 * 3600);
/// How long a backend is skipped after an HTTP 429.
const COOLDOWN_429: Duration = Duration::from_secs(60);
/// How long a backend is skipped after another failure.
const COOLDOWN_ERROR: Duration = Duration::from_secs(15);

/// Sliding-window counters of one backend.
#[derive(Debug, Default)]
pub(crate) struct RateWindow {
    minute: VecDeque<(Instant, u64)>,
    day_start: Option<Instant>,
    day_count: u32,
    cooldown_until: Option<Instant>,
}

impl RateWindow {
    /// Reserve one request of `tokens` at `now`, or refuse.
    pub(crate) fn try_acquire(&mut self, limits: RateLimits, now: Instant, tokens: u64) -> bool {
        if self.cooldown_until.is_some_and(|t| now < t) {
            return false;
        }
        while self
            .minute
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= MINUTE)
        {
            self.minute.pop_front();
        }
        if self.day_start.is_none_or(|s| now.duration_since(s) >= DAY) {
            self.day_start = Some(now);
            self.day_count = 0;
        }
        let used: u64 = self.minute.iter().map(|(_, t)| *t).sum();
        if (limits.rpm != 0 && self.minute.len() >= limits.rpm as usize)
            || (limits.tpm != 0 && used.saturating_add(tokens) > limits.tpm)
            || (limits.rpd != 0 && self.day_count >= limits.rpd)
        {
            return false;
        }
        self.minute.push_back((now, tokens));
        self.day_count += 1;
        true
    }

    pub(crate) fn cool_down(&mut self, now: Instant, d: Duration) {
        self.cooldown_until = Some(now + d);
    }
}

struct Slot {
    backend: Arc<dyn DecisionBackend>,
    limits: RateLimits,
    window: Mutex<RateWindow>,
}

/// Budgets of a [`Decider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeciderConfig {
    /// Wall-time cap of one decision, across the whole fallback chain.
    pub timeout: Duration,
    /// Lifetime of a cached answer (`0` disables the cache).
    pub cache_ttl: Duration,
    /// Cached answers kept.
    pub cache_entries: usize,
    /// The input is cut to this many bytes before it is sent.
    pub max_input_bytes: usize,
}

impl Default for DeciderConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(1500),
            cache_ttl: Duration::from_secs(600),
            cache_entries: 512,
            max_input_bytes: 6 * 1024,
        }
    }
}

/// Decider calls one turn may still make.
#[derive(Debug)]
pub struct TurnBudget(AtomicU32);

impl TurnBudget {
    /// A budget of `calls` decisions.
    #[must_use]
    pub fn new(calls: u32) -> Self {
        Self(AtomicU32::new(calls))
    }

    fn take(&self) -> bool {
        let mut cur = self.0.load(Ordering::SeqCst);
        while cur > 0 {
            match self
                .0
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(now) => cur = now,
            }
        }
        false
    }

    /// Calls left.
    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Counters of a [`Decider`] (monotonic since it was built).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderMetrics {
    /// `decide` calls.
    pub decisions: u64,
    /// Served from the cache (no network).
    pub cache_hits: u64,
    /// Requests sent to a backend.
    pub backend_calls: u64,
    /// Backend calls that failed (HTTP error, bad body).
    pub backend_errors: u64,
    /// Backends skipped because a local RPM / TPM / RPD counter or a cooldown
    /// said no.
    pub rate_limited: u64,
    /// Decisions cut off by the timeout.
    pub timeouts: u64,
    /// Decisions refused because the turn budget was spent.
    pub budget_denied: u64,
    /// Decisions answered by a backend other than the first.
    pub fallbacks: u64,
    /// Decisions that ended without any validated answer.
    pub unanswered: u64,
    /// Sum of the wall time of all decisions, in milliseconds.
    pub latency_ms_total: u64,
    /// Slowest decision, in milliseconds.
    pub latency_ms_max: u64,
}

#[derive(Default)]
struct Counters {
    decisions: AtomicU64,
    cache_hits: AtomicU64,
    backend_calls: AtomicU64,
    backend_errors: AtomicU64,
    rate_limited: AtomicU64,
    timeouts: AtomicU64,
    budget_denied: AtomicU64,
    fallbacks: AtomicU64,
    unanswered: AtomicU64,
    latency_ms_total: AtomicU64,
    latency_ms_max: AtomicU64,
}

fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// Which backends (by id, `provider/model`) a decision may be sent to.
pub type BackendFilter<'a> = dyn Fn(&str) -> bool + Send + Sync + 'a;

/// Result of one decision.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    /// Validated answers; empty when nothing could be decided.
    pub answers: Answers,
    /// Backend that answered (`None`: cache miss and no backend answered).
    pub backend: Option<String>,
    /// Served from the cache.
    pub cached: bool,
    /// Wall time.
    pub latency_ms: u64,
    /// Requests sent to a backend for this decision (0 on a cache hit).
    pub backend_calls: u32,
}

struct Cached {
    at: Instant,
    backend: String,
    answers: Answers,
}

/// A chain of decision backends with a timeout, rate limits, a cache and
/// metrics. See the [module docs](self).
pub struct Decider {
    slots: Vec<Slot>,
    cfg: DeciderConfig,
    cache: Mutex<HashMap<u128, Cached>>,
    counters: Counters,
    /// Persisted per-day counters (requests per day of each backend, calls
    /// made today); `None` keeps the in-memory sliding day only.
    day: Option<Arc<DayCounters>>,
    /// Backend requests allowed per local day (`0` = unlimited).
    daily_budget: u64,
}

impl std::fmt::Debug for Decider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decider")
            .field("backends", &self.backend_ids())
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn cache_key(req: &DecisionRequest) -> u128 {
    let bytes = serde_json::to_vec(req).unwrap_or_default();
    (u128::from(fnv(0xcbf2_9ce4_8422_2325, &bytes)) << 64)
        | u128::from(fnv(0x8422_2325_cbf2_9ce4, &bytes))
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl Decider {
    /// A decider over `backends`, tried in order.
    #[must_use]
    pub fn new(backends: Vec<(Arc<dyn DecisionBackend>, RateLimits)>, cfg: DeciderConfig) -> Self {
        Self {
            slots: backends
                .into_iter()
                .map(|(backend, limits)| Slot {
                    backend,
                    limits,
                    window: Mutex::new(RateWindow::default()),
                })
                .collect(),
            cfg,
            cache: Mutex::new(HashMap::new()),
            counters: Counters::default(),
            day: None,
            daily_budget: 0,
        }
    }

    /// Persist the per-day counters (RPD of each backend, calls made today):
    /// they survive a restart and reset at the local midnight.
    #[must_use]
    pub fn with_day_counters(mut self, day: Arc<DayCounters>) -> Self {
        self.day = Some(day);
        self
    }

    /// Cap the backend requests of a local day (`0` = unlimited). Needs
    /// [`Self::with_day_counters`] to count across restarts.
    #[must_use]
    pub fn with_daily_budget(mut self, calls: u64) -> Self {
        self.daily_budget = calls;
        self
    }

    /// The per-day counters, if installed.
    #[must_use]
    pub fn day_counters(&self) -> Option<&Arc<DayCounters>> {
        self.day.as_ref()
    }

    /// Backend requests made today (0 without day counters).
    #[must_use]
    pub fn calls_today(&self) -> u64 {
        self.day.as_ref().map_or(0, |d| d.get(CALLS_KEY))
    }

    /// Ids of the configured backends, in chain order.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<String> {
        self.slots
            .iter()
            .map(|s| s.backend.id().to_owned())
            .collect()
    }

    /// Current counters.
    #[must_use]
    pub fn metrics(&self) -> DeciderMetrics {
        let c = &self.counters;
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        DeciderMetrics {
            decisions: g(&c.decisions),
            cache_hits: g(&c.cache_hits),
            backend_calls: g(&c.backend_calls),
            backend_errors: g(&c.backend_errors),
            rate_limited: g(&c.rate_limited),
            timeouts: g(&c.timeouts),
            budget_denied: g(&c.budget_denied),
            fallbacks: g(&c.fallbacks),
            unanswered: g(&c.unanswered),
            latency_ms_total: g(&c.latency_ms_total),
            latency_ms_max: g(&c.latency_ms_max),
        }
    }

    /// Decide. Never fails: an invalid request, a spent budget, a timeout or
    /// a chain where every backend failed yields an empty [`Outcome`], and
    /// the caller applies its deterministic fallback.
    pub async fn decide(&self, req: &DecisionRequest, budget: &TurnBudget) -> Outcome {
        self.decide_via(req, budget, &|_| true).await
    }

    /// [`Self::decide`] restricted to the backends whose id passes `allow`
    /// (the caller's egress rule: which providers this request may be sent
    /// to). A cached answer of a backend that is not allowed is not used.
    pub async fn decide_via(
        &self,
        req: &DecisionRequest,
        budget: &TurnBudget,
        allow: &BackendFilter<'_>,
    ) -> Outcome {
        let started = Instant::now();
        bump(&self.counters.decisions);
        let mut outcome = self.decide_inner(req, budget, started, allow).await;
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        outcome.latency_ms = ms;
        self.counters
            .latency_ms_total
            .fetch_add(ms, Ordering::Relaxed);
        self.counters
            .latency_ms_max
            .fetch_max(ms, Ordering::Relaxed);
        if outcome.answers.is_empty() {
            bump(&self.counters.unanswered);
        }
        outcome
    }

    /// Cache `answers` for `key`, evicting expired entries (then the oldest)
    /// when the cache is full.
    fn remember(&self, key: u128, at: Instant, backend: &str, answers: &Answers) {
        if self.cfg.cache_ttl.is_zero() || self.cfg.cache_entries == 0 {
            return;
        }
        let mut cache = self.cache.lock();
        if cache.len() >= self.cfg.cache_entries {
            let ttl = self.cfg.cache_ttl;
            cache.retain(|_, c| at.duration_since(c.at) < ttl);
            if cache.len() >= self.cfg.cache_entries
                && let Some(oldest) = cache.iter().min_by_key(|(_, c)| c.at).map(|(k, _)| *k)
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            key,
            Cached {
                at,
                backend: backend.to_owned(),
                answers: answers.clone(),
            },
        );
    }

    /// The cached answer for `key`, when fresh and from an allowed backend.
    fn cached(&self, key: u128, now: Instant, allow: &BackendFilter<'_>) -> Option<Outcome> {
        if self.cfg.cache_ttl.is_zero() {
            return None;
        }
        let cache = self.cache.lock();
        let hit = cache.get(&key)?;
        if now.duration_since(hit.at) >= self.cfg.cache_ttl || !allow(&hit.backend) {
            return None;
        }
        Some(Outcome {
            answers: hit.answers.clone(),
            backend: Some(hit.backend.clone()),
            cached: true,
            latency_ms: 0,
            backend_calls: 0,
        })
    }

    #[allow(clippy::too_many_lines)] // one ordered pass: cache, budget, cooldowns, backends, validation
    async fn decide_inner(
        &self,
        req: &DecisionRequest,
        budget: &TurnBudget,
        started: Instant,
        allow: &BackendFilter<'_>,
    ) -> Outcome {
        if !req.is_valid() {
            return Outcome::default();
        }
        let bounded;
        let req = if req.input.len() > self.cfg.max_input_bytes {
            bounded = DecisionRequest {
                input: truncate(&req.input, self.cfg.max_input_bytes).to_owned(),
                questions: req.questions.clone(),
            };
            &bounded
        } else {
            req
        };
        let key = cache_key(req);
        if let Some(hit) = self.cached(key, started, allow) {
            bump(&self.counters.cache_hits);
            return hit;
        }
        if !self.slots.iter().any(|s| allow(s.backend.id())) {
            // Nothing this caller may send to: no call, no budget spent.
            return Outcome::default();
        }
        if self.daily_budget != 0 && self.calls_today() >= self.daily_budget {
            bump(&self.counters.budget_denied);
            return Outcome::default();
        }
        if !budget.take() {
            bump(&self.counters.budget_denied);
            return Outcome::default();
        }
        let tokens = req.estimated_tokens();
        let mut sent = 0u32;
        for (i, slot) in self.slots.iter().enumerate() {
            if !allow(slot.backend.id()) {
                continue;
            }
            let Some(left) = self.cfg.timeout.checked_sub(started.elapsed()) else {
                bump(&self.counters.timeouts);
                break;
            };
            // With persisted day counters they are the authority for RPD (the
            // in-memory window then only does RPM / TPM / cooldown).
            let rpd_key = format!("rpd:{}", slot.backend.id());
            let mut limits = slot.limits;
            if let Some(day) = &self.day
                && limits.rpd != 0
            {
                if day.get(&rpd_key) >= u64::from(limits.rpd) {
                    bump(&self.counters.rate_limited);
                    continue;
                }
                limits.rpd = 0;
            }
            if !slot
                .window
                .lock()
                .try_acquire(limits, Instant::now(), tokens)
            {
                bump(&self.counters.rate_limited);
                continue;
            }
            if let Some(day) = &self.day {
                day.bump(&rpd_key);
                day.bump(CALLS_KEY);
            }
            bump(&self.counters.backend_calls);
            sent += 1;
            let raw = match tokio::time::timeout(left, slot.backend.decide(req)).await {
                Err(_) => {
                    bump(&self.counters.timeouts);
                    break;
                }
                Ok(Err(e)) => {
                    bump(&self.counters.backend_errors);
                    let d = if e == DeciderError::RateLimited {
                        COOLDOWN_429
                    } else {
                        COOLDOWN_ERROR
                    };
                    slot.window.lock().cool_down(Instant::now(), d);
                    tracing::debug!(backend = slot.backend.id(), error = %e, "decider backend failed");
                    continue;
                }
                Ok(Ok(raw)) => raw,
            };
            let answers = validate(req, &raw);
            if answers.is_empty() {
                // Nothing usable (refusals, wrong shape): try the next one.
                bump(&self.counters.backend_errors);
                continue;
            }
            if i > 0 {
                bump(&self.counters.fallbacks);
            }
            let backend = slot.backend.id().to_owned();
            self.remember(key, started, &backend, &answers);
            return Outcome {
                answers,
                backend: Some(backend),
                cached: false,
                latency_ms: 0,
                backend_calls: sent,
            };
        }
        Outcome {
            backend_calls: sent,
            ..Outcome::default()
        }
    }
}
