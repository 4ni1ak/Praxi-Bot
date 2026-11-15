//! The decider role of the loop: settings and construction.
//!
//! A [`TurnDecider`] is a [`Decider`] (a chain of cheap decision models with
//! typed, validated answers; see `agent_router::decider`) plus the settings
//! that say what the loop may use it for. This module only *configures and
//! builds* it:
//!
//! * [`DeciderSettings`]: `<data dir>/decider.toml`. Provider and model id of
//!   every backend are plain configuration; nothing in the code depends on a
//!   specific model id, and the default chain is only a starting point.
//! * [`build_decider`]: resolves each backend's key from the provider it
//!   names (`providers.toml` / secret store / the catalog's environment
//!   variable) and skips what is unusable. Disabled, or no usable backend,
//!   yields `None`, and the loop behaves exactly as it does without one.
//!
//! * [`TurnPlan`]: when a top-level turn reaches K4 (the ladder found nothing
//!   cheaper), the loop asks one batched question set about the user's
//!   message and stores the derived plan in the turn's first step:
//!   - **tool pre-selection**: only the tool families the request plausibly
//!     needs are listed for the main model (plus the discovery meta-tools, so
//!     every other tool stays reachable through `find_tool` / `use_tool`);
//!   - **trivial-turn routing**: a message that needs no tools gets the
//!     meta-tools only and, when configured, the cheaper `light_model` for
//!     its first model call.
//!
//! Every part has its own switch and a deterministic fallback: a question
//! that was not answered (decider off, timeout, rate limit, invalid answer,
//! provider not allowed for the bot) leaves that part exactly as it is
//! without a decider. History trimming (`history_trim`) is not wired yet.
//!
//! # Security
//! A decider answer is *advice*, never authority:
//! * It is a number in `0..=1` or one of the option ids the caller supplied.
//!   The plan derived from it holds tool names taken from the registry, a
//!   model id taken from the owner's settings and nothing else.
//! * The plan only narrows what the main model is *shown* and which model
//!   answers first. A tool call is still resolved by `Shared::resolve_call`
//!   (profile filter re-applied) and decided by the permission engine on the
//!   resolved target. No answer is read anywhere on the permission path;
//!   this module does not depend on the permission engine.
//! * The decider never proposes tool calls or arguments, never sees tool
//!   output or page content (only the user's message and tool names /
//!   one-line descriptions of non-MCP tools), and its request passes the
//!   loop's redactor in [`Shared::ask_decider`], the only place a request
//!   leaves the loop.
//! * Egress: a request is sent only to backends whose provider the bot may
//!   already send its conversation to (the providers of its own model roles,
//!   plus `[router] decider_providers` of its profile). The same rule gates
//!   `light_model`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_router::decider::{
    Answers, ChatJsonBackend, Decider, DeciderConfig, DeciderMetrics, DecisionBackend,
    DecisionRequest, GeminiDecisions, OpenRouterDecisions, Outcome, Question, RateLimits,
    TurnBudget, yes,
};
use protocol::PermissionCategory;
use provider_catalog::{Auth, Catalog, ModelRef, ProviderConfig, config_path};
use secret_store::SecretStore;
use serde::{Deserialize, Serialize};
use storage::RunStep;
use tool_registry::ToolSpec;

use crate::history::truncate_utf8;
use crate::profile::BotProfile;
use crate::router::ProviderClassifier;
use crate::shared::{Providers, RunCtx, STEP_USER, Shared};

/// Purpose tag of a decider call in audit records (`decider.call`), for the
/// LLM call ledger: decider calls are counted under this key, never as
/// main-model (`executor`) calls.
pub const DECIDER_CALL_PURPOSE: &str = "decider";

/// File name of the decider settings inside the data directory.
pub const DECIDER_FILE: &str = "decider.toml";

/// Which HTTP API a decider backend speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeciderApi {
    /// Google's Decisions API (`/v1beta/decisions`).
    GeminiDecisions,
    /// `OpenRouter`'s Decisions API (`/api/alpha/decisions`).
    OpenrouterDecisions,
    /// The provider's normal chat endpoint with a strict JSON answer.
    ChatJson,
}

/// One backend of the decider's fallback chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderBackendSettings {
    /// Provider id as in `providers.toml` / the catalog (`gemini`,
    /// `openrouter`, ...). Its API key is the provider's key.
    pub provider: String,
    /// Model id at that provider.
    pub model: String,
    /// API to use; `None` = by provider (`gemini` -> Decisions API,
    /// `openrouter` -> Decisions API, anything else -> chat JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<DeciderApi>,
    /// Endpoint override for the Decisions APIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Requests per minute (`0` = unlimited).
    #[serde(default)]
    pub rpm: u32,
    /// Input tokens per minute (`0` = unlimited).
    #[serde(default)]
    pub tpm: u64,
    /// Requests per day (`0` = unlimited).
    #[serde(default)]
    pub rpd: u32,
}

impl DeciderBackendSettings {
    /// The API this backend speaks.
    #[must_use]
    pub fn effective_api(&self) -> DeciderApi {
        self.api.unwrap_or(match self.provider.as_str() {
            "gemini" | "google" => DeciderApi::GeminiDecisions,
            "openrouter" => DeciderApi::OpenrouterDecisions,
            _ => DeciderApi::ChatJson,
        })
    }
}

/// Settings of the decider role (`<data dir>/decider.toml`, `/v1/decider`).
///
/// Model ids of free decision models change often: nothing in the code
/// depends on a specific id, the defaults below are only the starting chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // independent feature switches of a settings file
pub struct DeciderSettings {
    /// Master switch (default off: it needs a provider key).
    pub enabled: bool,
    /// Send the main model only the tool families the request needs.
    pub tool_preselect: bool,
    /// Give a message that needs no tools the meta-tools only (and
    /// `light_model`, when set).
    pub trivial_route: bool,
    /// Send a short recent history when the message stands alone. Reserved:
    /// stored and validated, but no turn uses it yet.
    pub history_trim: bool,
    /// `provider/model` that answers the first model call of a trivial turn;
    /// the bot's executor (and its fallbacks) remain the fallback.
    pub light_model: Option<String>,
    /// Wall-time cap of one decision in milliseconds (whole chain).
    pub timeout_ms: u64,
    /// Decider calls one turn may make.
    pub max_calls_per_turn: u32,
    /// Lifetime of a cached decision in seconds (`0` = no cache).
    pub cache_ttl_secs: u64,
    /// A turn is trivial when P(needs tools) is at most this.
    pub trivial_max: f64,
    /// A tool family is kept when P(useful) is at least this.
    pub family_min: f64,
    /// The history is trimmed when P(depends on earlier turns) is at most this.
    pub standalone_max: f64,
    /// The fallback chain, tried in order.
    pub backends: Vec<DeciderBackendSettings>,
}

impl Default for DeciderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tool_preselect: true,
            trivial_route: true,
            history_trim: true,
            light_model: None,
            timeout_ms: 1500,
            max_calls_per_turn: 2,
            cache_ttl_secs: 600,
            trivial_max: 0.10,
            family_min: 0.25,
            standalone_max: 0.10,
            backends: vec![DeciderBackendSettings {
                provider: "gemini".into(),
                model: "gemini-decision-1".into(),
                api: None,
                base_url: None,
                rpm: 200,
                tpm: 50_000,
                rpd: 10_000,
            }],
        }
    }
}

/// Most backends a chain may have.
pub const MAX_BACKENDS: usize = 8;

impl DeciderSettings {
    /// Path of the settings file under `data_dir`.
    #[must_use]
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(DECIDER_FILE)
    }

    /// Load the settings; a missing file is the defaults.
    ///
    /// # Errors
    /// An unreadable, malformed or invalid file.
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = Self::path(data_dir);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{DECIDER_FILE}: {e}")),
        };
        let s: Self = toml::from_str(&text).map_err(|e| format!("{DECIDER_FILE}: {e}"))?;
        s.validate()?;
        Ok(s)
    }

    /// Write the settings (atomically: temp file, then rename).
    ///
    /// # Errors
    /// Invalid settings or an I/O failure.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{DECIDER_FILE}: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{DECIDER_FILE}: {e}"))
    }

    /// Check ranges and shapes.
    ///
    /// # Errors
    /// The first problem found.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |name: &str, v: f64| {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("`{name}` must be between 0 and 1"))
            }
        };
        unit("trivial_max", self.trivial_max)?;
        unit("family_min", self.family_min)?;
        unit("standalone_max", self.standalone_max)?;
        if !(100..=30_000).contains(&self.timeout_ms) {
            return Err("`timeout_ms` must be between 100 and 30000".into());
        }
        if self.max_calls_per_turn > 16 {
            return Err("`max_calls_per_turn` must be at most 16".into());
        }
        if self.backends.len() > MAX_BACKENDS {
            return Err(format!("at most {MAX_BACKENDS} backends"));
        }
        let plain = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control() || c == ' ')
        };
        for b in &self.backends {
            if !plain(&b.provider, 64) || b.provider.contains('/') {
                return Err(format!("invalid backend provider {:?}", b.provider));
            }
            if !plain(&b.model, 200) {
                return Err(format!("invalid backend model {:?}", b.model));
            }
            if let Some(u) = &b.base_url
                && !(u.starts_with("https://") || u.starts_with("http://"))
            {
                return Err("backend `base_url` must be an http(s) URL".into());
            }
        }
        if let Some(m) = &self.light_model
            && (!plain(m, 200) || m.starts_with("cli:"))
        {
            return Err("`light_model` must be an HTTP `provider/model`".into());
        }
        Ok(())
    }
}

/// What the decider saved the main model (monotonic since it was installed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeciderSavings {
    /// Turns that got a plan from the decider.
    pub turns_planned: u64,
    /// Turns where no question was answered (deterministic fallback applied).
    pub turns_unplanned: u64,
    /// Main-model calls sent with a narrowed tool list.
    pub narrowed_calls: u64,
    /// Bytes of tool schemas those calls would have carried.
    pub tool_schema_bytes_full: u64,
    /// Bytes of tool schemas they carried.
    pub tool_schema_bytes_sent: u64,
    /// Model calls the light model answered instead of the main model.
    pub main_calls_saved: u64,
}

#[derive(Default)]
pub(crate) struct Savings {
    turns_planned: AtomicU64,
    turns_unplanned: AtomicU64,
    narrowed_calls: AtomicU64,
    tool_schema_bytes_full: AtomicU64,
    tool_schema_bytes_sent: AtomicU64,
    main_calls_saved: AtomicU64,
}

impl Savings {
    fn snapshot(&self) -> DeciderSavings {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        DeciderSavings {
            turns_planned: g(&self.turns_planned),
            turns_unplanned: g(&self.turns_unplanned),
            narrowed_calls: g(&self.narrowed_calls),
            tool_schema_bytes_full: g(&self.tool_schema_bytes_full),
            tool_schema_bytes_sent: g(&self.tool_schema_bytes_sent),
            main_calls_saved: g(&self.main_calls_saved),
        }
    }

    pub(crate) fn narrowed_call(&self, full: usize, sent: usize) {
        self.narrowed_calls.fetch_add(1, Ordering::Relaxed);
        self.tool_schema_bytes_full
            .fetch_add(full as u64, Ordering::Relaxed);
        self.tool_schema_bytes_sent
            .fetch_add(sent as u64, Ordering::Relaxed);
    }

    pub(crate) fn main_call_saved(&self) {
        self.main_calls_saved.fetch_add(1, Ordering::Relaxed);
    }
}

/// The decider installed in a loop: the chain plus the switches.
pub struct TurnDecider {
    decider: Arc<Decider>,
    settings: DeciderSettings,
    pub(crate) savings: Savings,
}

impl std::fmt::Debug for TurnDecider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnDecider")
            .field("decider", &self.decider)
            .finish_non_exhaustive()
    }
}

impl TurnDecider {
    /// Wrap a ready [`Decider`] (tests, embedders with their own backends).
    #[must_use]
    pub fn new(decider: Arc<Decider>, settings: DeciderSettings) -> Arc<Self> {
        Arc::new(Self {
            decider,
            settings,
            savings: Savings::default(),
        })
    }

    /// What the plans saved the main model.
    #[must_use]
    pub fn savings(&self) -> DeciderSavings {
        self.savings.snapshot()
    }

    /// The settings it was built from.
    #[must_use]
    pub fn settings(&self) -> &DeciderSettings {
        &self.settings
    }

    /// Ids of the usable backends, in chain order.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<String> {
        self.decider.backend_ids()
    }

    /// Counters of the decision chain.
    #[must_use]
    pub fn metrics(&self) -> DeciderMetrics {
        self.decider.metrics()
    }

    /// The underlying chain (live probes).
    #[must_use]
    pub fn decider(&self) -> &Arc<Decider> {
        &self.decider
    }
}

/// The API key of provider `id`: its `providers.toml` entry (stored secret or
/// the catalog's environment variable), or just the catalog's environment
/// variable when the provider is not configured.
fn provider_key(
    cfg: &ProviderConfig,
    catalog: &Catalog,
    store: &dyn SecretStore,
    id: &str,
) -> Option<String> {
    let settings = cfg.providers.iter().find(|p| p.enabled && p.id == id);
    let entry = settings
        .and_then(|s| s.custom.as_ref())
        .or_else(|| catalog.get(id));
    let key = match (
        settings.and_then(|s| s.key.as_ref()),
        entry.map(|e| &e.auth),
    ) {
        (Some(r), _) => secret_store::resolve(store, r)
            .ok()
            .map(|k| k.expose().to_string()),
        (None, Some(Auth::ApiKeyEnv(var))) => std::env::var(var).ok(),
        _ => None,
    };
    key.filter(|k| !k.trim().is_empty())
}

/// Build the decider described by `settings`: every backend that has a key
/// (Decisions APIs) or a configured chat provider. `None` when it is disabled
/// or no backend is usable; skipped backends are reported through `warn`.
/// Keys are registered with the loop's redactor by the usual secret refresh;
/// they never appear in a warning.
pub fn build_decider(
    settings: &DeciderSettings,
    data_dir: &Path,
    catalog: &Catalog,
    store: &dyn SecretStore,
    providers: &Providers,
    warn: &mut dyn FnMut(&str),
) -> Option<Arc<TurnDecider>> {
    if !settings.enabled {
        return None;
    }
    let cfg = ProviderConfig::load(&config_path(data_dir)).unwrap_or_default();
    let mut chain: Vec<(Arc<dyn DecisionBackend>, RateLimits)> = Vec::new();
    for b in &settings.backends {
        let id = format!("{}/{}", b.provider, b.model);
        let limits = RateLimits {
            rpm: b.rpm,
            tpm: b.tpm,
            rpd: b.rpd,
        };
        let api = b.effective_api();
        let backend: Arc<dyn DecisionBackend> = match api {
            DeciderApi::ChatJson => {
                if providers.get(&b.provider).is_none() {
                    warn(&format!(
                        "decider backend {id} skipped: provider is not configured"
                    ));
                    continue;
                }
                Arc::new(ChatJsonBackend::new(
                    id.clone(),
                    Arc::new(ProviderClassifier {
                        providers: providers.clone(),
                        model: id.clone(),
                    }),
                ))
            }
            DeciderApi::GeminiDecisions | DeciderApi::OpenrouterDecisions => {
                let Some(key) = provider_key(&cfg, catalog, store, &b.provider) else {
                    warn(&format!("decider backend {id} skipped: no API key"));
                    continue;
                };
                if api == DeciderApi::GeminiDecisions {
                    let mut g = GeminiDecisions::new(key, b.model.clone()).with_id(id.clone());
                    if let Some(u) = &b.base_url {
                        g = g.with_base_url(u.clone());
                    }
                    Arc::new(g)
                } else {
                    let mut o = OpenRouterDecisions::new(key, b.model.clone()).with_id(id.clone());
                    if let Some(u) = &b.base_url {
                        o = o.with_base_url(u.clone());
                    }
                    Arc::new(o)
                }
            }
        };
        chain.push((backend, limits));
    }
    if chain.is_empty() {
        warn("decider is enabled but no backend is usable; turns run without it");
        return None;
    }
    let decider = Decider::new(
        chain,
        DeciderConfig {
            timeout: Duration::from_millis(settings.timeout_ms),
            cache_ttl: Duration::from_secs(settings.cache_ttl_secs),
            ..DeciderConfig::default()
        },
    );
    Some(TurnDecider::new(Arc::new(decider), settings.clone()))
}

// ---- the plan ----

/// Which tools the main model is shown this turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolPlan {
    /// Unchanged (the deterministic fallback).
    #[default]
    All,
    /// Only the discovery meta-tools.
    MetaOnly,
    /// These tools plus the discovery meta-tools.
    Only(Vec<String>),
}

/// The decider's plan for one turn, persisted in the first step's input.
///
/// It is a *display* plan: tool names to list and the model of the first
/// call. Nothing in it is read by the permission path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TurnPlan {
    #[serde(default)]
    pub tools: ToolPlan,
    /// Model of the first model call (a trivial turn with a light model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_model: Option<String>,
    /// Audit: backend that answered, whether it was cached, wall time and the
    /// probabilities the plan was derived from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub p: BTreeMap<String, f64>,
}

impl TurnPlan {
    /// Whether the plan changes anything.
    pub(crate) fn is_noop(&self) -> bool {
        self.tools == ToolPlan::All && self.light_model.is_none()
    }
}

/// The plan stored in a run's first step, if any.
pub(crate) fn stored_plan(steps: &[RunStep]) -> Option<TurnPlan> {
    let first = steps.first().filter(|s| s.name == STEP_USER)?;
    serde_json::from_value(first.input.get("plan")?.clone()).ok()
}

/// Family of a tool name: the part before the first `.` or `_`.
pub(crate) fn family_of(name: &str) -> &str {
    name.split(['.', '_']).next().unwrap_or(name)
}

/// Most families asked about; tools of further families are always kept.
const MAX_FAMILIES: usize = 48;
/// Bytes of a family's tool list inside its question.
const FAMILY_DESC_BYTES: usize = 480;
/// Bytes of the user message sent to the decider.
const TEXT_BYTES: usize = 2000;

const Q_NEEDS_TOOLS: &str = "needs_tools";

/// Tools that are never dropped by pre-selection: the loop's own meta tools
/// (todo list, questions to the user, skills, subagents).
fn always_kept(spec: &ToolSpec) -> bool {
    spec.category == PermissionCategory::Meta
}

/// The decider request for one turn and the families it asks about (question
/// key -> tool names). The user's text is the `input` (data); the questions
/// are written here and only quote tool names and one-line descriptions.
pub(crate) fn build_request(
    settings: &DeciderSettings,
    text: &str,
    specs: &[ToolSpec],
) -> (DecisionRequest, BTreeMap<String, Vec<String>>) {
    let mut req = DecisionRequest::new(format!(
        "Latest user request:\n{}\n",
        truncate_utf8(text, TEXT_BYTES)
    ));
    let mut families: BTreeMap<String, Vec<&ToolSpec>> = BTreeMap::new();
    if settings.tool_preselect || settings.trivial_route {
        req = req.yes_no(
            Q_NEEDS_TOOLS,
            "Does fulfilling the latest user request require the assistant to use tools \
             (files, web, browser, desktop apps, shell, e-mail, schedules, memory or other \
             actions and lookups) instead of simply replying from its own knowledge and the \
             conversation?",
        );
        for s in specs.iter().filter(|s| !always_kept(s)) {
            families
                .entry(family_of(&s.name).to_owned())
                .or_default()
                .push(s);
        }
    }
    let mut asked = BTreeMap::new();
    for (i, (family, tools)) in families.iter().take(MAX_FAMILIES).enumerate() {
        let mut list = String::new();
        for t in tools {
            let line = format!(
                "{} - {}; ",
                t.name,
                truncate_utf8(t.description.lines().next().unwrap_or_default(), 90)
            );
            if list.len() + line.len() > FAMILY_DESC_BYTES {
                list.push_str("...");
                break;
            }
            list.push_str(&line);
        }
        let key = format!("fam_{i}");
        req = req.yes_no(
            &key,
            format!(
                "Could the assistant plausibly need a tool of the group \"{family}\" to fulfil \
                 the latest user request? Tools in the group: {list}"
            ),
        );
        asked.insert(key, tools.iter().map(|t| t.name.clone()).collect());
    }
    (req, asked)
}

/// Turn validated answers into a plan. Pure and deterministic; an unanswered
/// question leaves its part at the fallback. `light_model` is the settings'
/// light model when the bot may use it.
pub(crate) fn derive_plan(
    settings: &DeciderSettings,
    answers: &Answers,
    asked: &BTreeMap<String, Vec<String>>,
    specs: &[ToolSpec],
    light_model: Option<&str>,
) -> TurnPlan {
    let mut plan = TurnPlan::default();
    let needs_tools = yes(answers, Q_NEEDS_TOOLS);
    if let Some(p) = needs_tools {
        plan.p.insert(Q_NEEDS_TOOLS.to_owned(), p);
    }
    // Families the decider considers useful; an unanswered family is kept.
    let mut kept: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    let mut any_family_wanted = false;
    for (key, tools) in asked {
        match yes(answers, key) {
            Some(p) if p < settings.family_min => dropped += tools.len(),
            Some(_) => {
                any_family_wanted = true;
                kept.extend(tools.iter().cloned());
            }
            None => kept.extend(tools.iter().cloned()),
        }
    }
    let all_answered = asked.keys().all(|k| answers.contains_key(k));
    let trivial = settings.trivial_route
        && needs_tools.is_some_and(|p| p <= settings.trivial_max)
        && all_answered
        && !any_family_wanted;
    if trivial {
        plan.tools = ToolPlan::MetaOnly;
        plan.light_model = light_model.map(str::to_owned);
    } else if settings.tool_preselect && any_family_wanted && dropped > 0 {
        // Tools the questions did not cover (meta tools, families past the
        // cap) are always kept.
        let asked_names: std::collections::HashSet<&str> =
            asked.values().flatten().map(String::as_str).collect();
        kept.extend(
            specs
                .iter()
                .filter(|s| !asked_names.contains(s.name.as_str()))
                .map(|s| s.name.clone()),
        );
        kept.sort();
        kept.dedup();
        plan.tools = ToolPlan::Only(kept);
    }
    plan
}

/// Provider part of a `provider/model` id.
fn provider_of(id: &str) -> Option<&str> {
    id.split_once('/').map(|(p, _)| p)
}

impl Shared {
    /// The installed decider, if any.
    pub(crate) fn turn_decider(&self) -> Option<Arc<TurnDecider>> {
        self.decider.read().clone()
    }

    /// Providers this bot's text may be sent to for a decider question or a
    /// light model: the providers its own model roles already use, plus the
    /// profile's `[router] decider_providers`. This is the egress rule of the
    /// decider; a backend at any other provider is not asked for this bot.
    pub(crate) fn decider_providers(&self, profile: &BotProfile) -> BTreeSet<String> {
        let default_id = self.providers.read().default_id().to_owned();
        let m = &profile.models;
        std::iter::once(m.executor.as_str())
            .chain(m.fallback.iter().map(String::as_str))
            .chain(m.classifier.as_deref())
            .chain(m.planner.as_deref())
            .filter(|r| !r.starts_with("cli:"))
            .map(|r| {
                ModelRef(r.to_owned())
                    .provider()
                    .map_or_else(|| default_id.clone(), str::to_owned)
            })
            .chain(profile.router.decider_providers.iter().cloned())
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// The only place a decision request leaves the loop: the request is
    /// redacted (input, instructions and option texts) by the loop's
    /// redactor, sent only to backends whose provider `allowed` contains, and
    /// recorded as a `decider.call` audit entry with the ledger purpose tag
    /// (numbers and ids only, never the text).
    pub(crate) async fn ask_decider(
        &self,
        ctx: &RunCtx,
        td: &TurnDecider,
        mut req: DecisionRequest,
        budget: &TurnBudget,
        allowed: &BTreeSet<String>,
        use_: &str,
    ) -> Outcome {
        req.input = self.redact(std::mem::take(&mut req.input));
        for q in req.questions.values_mut() {
            match q {
                Question::YesNo { instructions } => {
                    *instructions = self.redact(std::mem::take(instructions));
                }
                Question::Choice {
                    instructions,
                    options,
                } => {
                    *instructions = self.redact(std::mem::take(instructions));
                    for o in options {
                        o.description = self.redact(std::mem::take(&mut o.description));
                    }
                }
            }
        }
        let allow = |id: &str| provider_of(id).is_some_and(|p| allowed.contains(p));
        let out = td.decider.decide_via(&req, budget, &allow).await;
        if out.backend_calls > 0 || out.cached {
            self.audit(
                ctx,
                "bot",
                "decider.call",
                &serde_json::json!({
                    "purpose": DECIDER_CALL_PURPOSE,
                    "use": use_,
                    "model": out.backend,
                    "calls": out.backend_calls,
                    "cached": out.cached,
                    "answered": !out.answers.is_empty(),
                    "latency_ms": out.latency_ms,
                    "input_bytes": req.input.len(),
                }),
            );
        }
        out
    }

    /// Ask the decider how to run this turn. `None`: no decider, the bot
    /// opted out, no allowed backend, nothing to ask, or no answer (the turn
    /// runs exactly as without a decider).
    pub(crate) async fn plan_turn(&self, ctx: &RunCtx, text: &str) -> Option<TurnPlan> {
        let td = self.turn_decider()?;
        let s = td.settings();
        if !ctx.profile.router.decider
            || !(s.tool_preselect || s.trivial_route)
            || s.max_calls_per_turn == 0
        {
            return None;
        }
        let allowed = self.decider_providers(&ctx.profile);
        // Non-namespaced tools the bot may see: what a direct surface would
        // send (namespaced `mcp:` tools are discovery-only anyway, and their
        // descriptions are third-party text that is never sent to a decider).
        let specs: Vec<ToolSpec> = self
            .visible_specs(&ctx.profile, ctx.bot_id)
            .into_iter()
            .filter(|t| {
                !agent_router::surface::is_namespaced(&t.name)
                    && !matches!(
                        t.name.as_str(),
                        agent_router::FIND_TOOL
                            | agent_router::GET_TOOL_DETAILS
                            | agent_router::USE_TOOL
                    )
            })
            .collect();
        let (req, asked) = build_request(s, text, &specs);
        if req.questions.is_empty() {
            return None;
        }
        let budget = TurnBudget::new(s.max_calls_per_turn);
        let out = self
            .ask_decider(ctx, &td, req, &budget, &allowed, "turn_plan")
            .await;
        if out.answers.is_empty() {
            td.savings.turns_unplanned.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let light = s
            .light_model
            .as_deref()
            .filter(|m| provider_of(m).is_some_and(|p| allowed.contains(p)));
        let mut plan = derive_plan(s, &out.answers, &asked, &specs, light);
        plan.backend = out.backend;
        plan.cached = out.cached;
        plan.latency_ms = out.latency_ms;
        td.savings.turns_planned.fetch_add(1, Ordering::Relaxed);
        self.audit(
            ctx,
            "bot",
            "decider.plan",
            &serde_json::to_value(&plan).unwrap_or_default(),
        );
        Some(plan)
    }
}