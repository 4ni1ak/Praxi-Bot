//! The decider inside the loop: one cheap decision per turn that saves
//! main-model calls and tokens.
//!
//! When a top-level turn reaches K4 (the ladder found nothing cheaper), the
//! loop asks the [`Decider`] one batched question set about the user's message
//! and turns the answers into a [`TurnPlan`]:
//!
//! * **tool pre-selection**: only the tool families the request plausibly
//!   needs are sent to the main model (plus the discovery meta-tools, so the
//!   model can still reach every other tool through `find_tool` / `use_tool`);
//! * **trivial-turn routing**: a message that needs no tools gets the
//!   meta-tools only and, when configured, the cheaper `light_model` for its
//!   first model call;
//! * **history trimming**: a message that does not depend on the earlier
//!   conversation is sent with a short recent history instead of the full one.
//!
//! The plan is stored in the turn's first step, so a restart replays the same
//! plan and every model call of the turn sees the same tool list (stable
//! prompt-cache prefix).
//!
//! Every part has its own switch and a deterministic fallback: a question
//! that was not answered (decider off, timeout, rate limit, invalid answer)
//! leaves that part exactly as it is without a decider.
//!
//! # Security
//! The plan only narrows what the main model is *shown* and which model
//! answers. Tool calls are still resolved by `Shared::resolve_call` and
//! decided by the permission engine, whatever the plan says; the plan cannot
//! reveal a tool the bot's profile hides (names are re-filtered when applied)
//! and carries nothing but tool names from the registry, a model id from the
//! settings and booleans.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_router::decider::{
    ChatJsonBackend, Decider, DeciderConfig, DeciderMetrics, DecisionBackend, DecisionRequest,
    GeminiDecisions, OpenRouterDecisions, RateLimits, TurnBudget, yes,
};
use protocol::PermissionCategory;
use provider_catalog::{Auth, Catalog, ProviderConfig, config_path};
use secret_store::SecretStore;
use serde::{Deserialize, Serialize};
use storage::RunStep;
use tool_registry::ToolSpec;

use crate::history::truncate_utf8;
use crate::router::ProviderClassifier;
use crate::shared::{Providers, RunCtx, STEP_USER, Shared};

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
pub struct DeciderSettings {
    /// Master switch (default off: it needs a provider key).
    pub enabled: bool,
    /// Send the main model only the tool families the request needs.
    pub tool_preselect: bool,
    /// Give a message that needs no tools the meta-tools only (and
    /// `light_model`, when set).
    pub trivial_route: bool,
    /// Send a short recent history when the message stands alone.
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
            backends: vec![
                DeciderBackendSettings {
                    provider: "gemini".into(),
                    model: "gemini-decision-1".into(),
                    api: None,
                    base_url: None,
                    rpm: 200,
                    tpm: 50_000,
                    rpd: 10_000,
                },
                DeciderBackendSettings {
                    provider: "openrouter".into(),
                    model: "perplexity/pplx-decider-flash-v1.3:free".into(),
                    api: None,
                    base_url: None,
                    rpm: 20,
                    tpm: 0,
                    rpd: 200,
                },
            ],
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
    /// Turns sent with the short recent history.
    pub history_trimmed_turns: u64,
}

#[derive(Default)]
pub(crate) struct Savings {
    turns_planned: AtomicU64,
    turns_unplanned: AtomicU64,
    narrowed_calls: AtomicU64,
    tool_schema_bytes_full: AtomicU64,
    tool_schema_bytes_sent: AtomicU64,
    main_calls_saved: AtomicU64,
    history_trimmed_turns: AtomicU64,
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
            history_trimmed_turns: g(&self.history_trimmed_turns),
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

    /// What the plans saved the main model.
    #[must_use]
    pub fn savings(&self) -> DeciderSavings {
        self.savings.snapshot()
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TurnPlan {
    #[serde(default)]
    pub tools: ToolPlan,
    /// Model of the first model call (a trivial turn with a light model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_model: Option<String>,
    /// Send only the recent chat history.
    #[serde(default)]
    pub recent_history: bool,
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
        self.tools == ToolPlan::All && self.light_model.is_none() && !self.recent_history
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
/// Chat history smaller than this is never trimmed (nothing to save).
pub(crate) const TRIM_MIN_HISTORY_BYTES: usize = 6 * 1024;
/// Messages / bytes of the short recent history.
pub(crate) const RECENT_MESSAGES: usize = 6;
pub(crate) const RECENT_BYTES: usize = 6 * 1024;

const Q_NEEDS_TOOLS: &str = "needs_tools";
const Q_FOLLOW_UP: &str = "follow_up";

/// Tools that are never dropped by pre-selection: the loop's own meta tools
/// (todo list, questions to the user, skills, subagents).
fn always_kept(spec: &ToolSpec) -> bool {
    spec.category == PermissionCategory::Meta
}

/// The decider request for one turn and the families it asks about (question
/// key -> tool names).
pub(crate) fn build_request(
    settings: &DeciderSettings,
    text: &str,
    recent: &[(String, String)],
    specs: &[ToolSpec],
    ask_follow_up: bool,
) -> (DecisionRequest, BTreeMap<String, Vec<String>>) {
    use std::fmt::Write as _;
    let mut input = format!(
        "Latest user request:\n{}\n",
        truncate_utf8(text, TEXT_BYTES)
    );
    if ask_follow_up && !recent.is_empty() {
        input.push_str("\nEarlier conversation (oldest first):\n");
        for (role, content) in recent {
            let flat: String = content
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let _ = writeln!(input, "{role}: {}", truncate_utf8(&flat, 300));
        }
    }
    let mut req = DecisionRequest::new(input);
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
    if ask_follow_up {
        req = req.yes_no(
            Q_FOLLOW_UP,
            "Does understanding or fulfilling the latest user request depend on the earlier \
             conversation (it refers to, continues or corrects something said before)?",
        );
    }
    (req, asked)
}

/// Turn validated answers into a plan. Pure and deterministic; an unanswered
/// question leaves its part at the fallback.
pub(crate) fn derive_plan(
    settings: &DeciderSettings,
    answers: &agent_router::decider::Answers,
    asked: &BTreeMap<String, Vec<String>>,
    specs: &[ToolSpec],
    ask_follow_up: bool,
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
        plan.light_model.clone_from(&settings.light_model);
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
    if ask_follow_up
        && settings.history_trim
        && let Some(p) = yes(answers, Q_FOLLOW_UP)
    {
        plan.p.insert(Q_FOLLOW_UP.to_owned(), p);
        plan.recent_history = p <= settings.standalone_max;
    }
    plan
}

impl Shared {
    /// The installed decider, if any.
    pub(crate) fn turn_decider(&self) -> Option<Arc<TurnDecider>> {
        self.decider.read().clone()
    }

    /// Ask the decider how to run this turn. `None`: no decider, nothing to
    /// ask, or no answer (the turn runs exactly as without a decider).
    pub(crate) async fn plan_turn(&self, ctx: &RunCtx, text: &str) -> Option<TurnPlan> {
        let td = self.turn_decider()?;
        let s = td.settings();
        if !(s.tool_preselect || s.trivial_route || s.history_trim) || s.max_calls_per_turn == 0 {
            return None;
        }
        // Non-namespaced tools the bot may see: what a direct surface would
        // send (namespaced `mcp:` tools are discovery-only anyway).
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
        let (recent, history_bytes) = self.recent_chat(ctx);
        let ask_follow_up = s.history_trim && history_bytes >= TRIM_MIN_HISTORY_BYTES;
        let (req, asked) = build_request(s, text, &recent, &specs, ask_follow_up);
        if req.questions.is_empty() {
            return None;
        }
        let budget = TurnBudget::new(s.max_calls_per_turn);
        let out = td.decider.decide(&req, &budget).await;
        if out.answers.is_empty() {
            td.savings.turns_unplanned.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let mut plan = derive_plan(s, &out.answers, &asked, &specs, ask_follow_up);
        plan.backend = out.backend;
        plan.cached = out.cached;
        plan.latency_ms = out.latency_ms;
        td.savings.turns_planned.fetch_add(1, Ordering::Relaxed);
        if plan.recent_history {
            td.savings
                .history_trimmed_turns
                .fetch_add(1, Ordering::Relaxed);
        }
        self.audit(
            ctx,
            "bot",
            "decider.plan",
            &serde_json::to_value(&plan).unwrap_or_default(),
        );
        Some(plan)
    }

    /// The last few chat messages before this run (role, text) and the size
    /// of the history a model call would carry.
    fn recent_chat(&self, ctx: &RunCtx) -> (Vec<(String, String)>, usize) {
        let Ok((msgs, _)) = self.storage.list_messages_bounded(
            ctx.chat_id,
            Some(ctx.run_id),
            crate::history::MAX_HISTORY_MESSAGES,
            crate::history::MAX_HISTORY_BYTES,
        ) else {
            return (Vec::new(), 0);
        };
        let convo: Vec<_> = msgs
            .into_iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .collect();
        let bytes = convo.iter().map(|m| m.content.len()).sum();
        let recent = convo
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();
        (recent, bytes)
    }
}
