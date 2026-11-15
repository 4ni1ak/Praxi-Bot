//! `GET` / `PUT /v1/decider`: the small "Decider" block of the settings.
//!
//! The decider is a cheap decision model that answers routing questions so the
//! main model is called less. It is **off until the owner enables it**. This
//! surface only edits the switch, the preset (which provider / model), the
//! daily request budget and the Gemini model name; the full file
//! (`decider.toml`) stays editable by hand.

use std::sync::Arc;

use agent_core::{
    DEFAULT_GEMINI_DECISION_MODEL, DeciderApi, DeciderBackendSettings, DeciderSettings, Providers,
    TurnDecider,
};
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::ErrorBody;
use crate::routes::ApiJson;
use crate::{ApiError, AppState};

/// Embedder hook: builds the decider from the settings, the providers and the
/// secret store (which only the embedder can open).
pub trait DeciderHost: Send + Sync {
    /// The decider for `settings`; `None` when disabled or nothing is usable.
    /// Problems are reported through `warn` (never containing keys).
    fn build(
        &self,
        settings: &DeciderSettings,
        providers: &Providers,
        warn: &mut dyn FnMut(&str),
    ) -> Option<Arc<TurnDecider>>;
}

/// Backends the owner can pick in the UI.
const PRESETS: [&str; 3] = ["gemini", "openrouter_free", "gemini_then_openrouter_free"];

/// Pattern of the free OpenRouter decision models (resolved by rule from the
/// live model list, never a fixed id).
const OPENROUTER_FREE_MATCH: &str = "*decider*:free";

/// The Decider block.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeciderDto {
    /// Master switch. Default `false`.
    pub enabled: bool,
    /// Which chain is configured: `gemini`, `openrouter_free`,
    /// `gemini_then_openrouter_free` or `custom` (hand-edited `decider.toml`).
    pub preset: String,
    /// Gemini decision model name (editable: ids change).
    pub gemini_model: String,
    /// Backend requests allowed per day (`0` = unlimited).
    pub daily_budget: u64,
    /// Backends that are usable right now (`provider/model-or-rule`).
    pub active: Vec<String>,
    /// Decider requests sent today.
    pub calls_made_today: u64,
    /// Main-model calls a cheaper model answered today.
    pub main_calls_saved_today: u64,
    /// Why a configured backend is not usable (no key, ...). No secrets.
    pub warnings: Vec<String>,
    /// A configured backend is a free tier whose prompts may be used for
    /// provider training.
    pub free_tier_notice: bool,
}

/// `PUT /v1/decider`: every field is optional.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct DeciderInput {
    pub enabled: Option<bool>,
    /// One of `gemini`, `openrouter_free`, `gemini_then_openrouter_free`.
    pub preset: Option<String>,
    pub gemini_model: Option<String>,
    pub daily_budget: Option<u64>,
}

fn gemini_backend(model: &str) -> DeciderBackendSettings {
    DeciderBackendSettings {
        provider: "gemini".into(),
        model: model.into(),
        model_match: None,
        prefer: Vec::new(),
        api: None,
        base_url: None,
        rpm: 200,
        tpm: 50_000,
        rpd: 10_000,
    }
}

fn openrouter_free_backend() -> DeciderBackendSettings {
    DeciderBackendSettings {
        provider: "openrouter".into(),
        model: String::new(),
        model_match: Some(OPENROUTER_FREE_MATCH.into()),
        prefer: vec!["flash".into()],
        api: Some(DeciderApi::OpenrouterDecisions),
        base_url: None,
        rpm: 20,
        tpm: 0,
        rpd: 50,
    }
}

fn gemini_model_of(s: &DeciderSettings) -> String {
    s.backends
        .iter()
        .find(|b| b.effective_api() == DeciderApi::GeminiDecisions && !b.model.is_empty())
        .map_or_else(
            || DEFAULT_GEMINI_DECISION_MODEL.to_owned(),
            |b| b.model.clone(),
        )
}

fn preset_of(s: &DeciderSettings) -> &'static str {
    let is_gemini = |b: &DeciderBackendSettings| {
        b.provider == "gemini" && b.model_match.is_none() && b.api.is_none()
    };
    let is_free = |b: &DeciderBackendSettings| *b == openrouter_free_backend();
    match s.backends.as_slice() {
        [g] if is_gemini(g) => "gemini",
        [o] if is_free(o) => "openrouter_free",
        [g, o] if is_gemini(g) && is_free(o) => "gemini_then_openrouter_free",
        _ => "custom",
    }
}

fn dto(s: &AppState, settings: &DeciderSettings, warnings: Vec<String>) -> DeciderDto {
    let td = s.agent().decider();
    let today = td.as_ref().map(|t| t.today()).unwrap_or_default();
    DeciderDto {
        enabled: settings.enabled,
        preset: preset_of(settings).to_owned(),
        gemini_model: gemini_model_of(settings),
        daily_budget: settings.daily_budget,
        active: td.as_ref().map(|t| t.backend_ids()).unwrap_or_default(),
        calls_made_today: today.calls_made,
        main_calls_saved_today: today.main_calls_saved,
        warnings,
        free_tier_notice: settings
            .backends
            .iter()
            .any(|b| b.model_match.is_some() || b.provider == "openrouter"),
    }
}

fn apply(host: &dyn DeciderHost, s: &AppState, settings: &DeciderSettings) -> Vec<String> {
    let mut warnings = Vec::new();
    let providers = s.agent().providers();
    let td = host.build(settings, &providers, &mut |w| warnings.push(w.to_owned()));
    s.agent().set_decider(td);
    warnings
}

/// Build and install the decider from `decider.toml` (startup, providers
/// reload). A broken file leaves the loop without a decider.
pub(crate) fn reinstall(s: &AppState) {
    let Some(host) = s.decider_host() else { return };
    match DeciderSettings::load(&s.data_dir()) {
        Ok(settings) => {
            for w in apply(host.as_ref(), s, &settings) {
                eprintln!("daemon-api: decider: {w}");
            }
        }
        Err(e) => {
            s.agent().set_decider(None);
            eprintln!("daemon-api: decider settings ignored: {e}");
        }
    }
}

/// The Decider settings and today's numbers.
#[utoipa::path(get, path = "/v1/decider", tag = "decider", security(("bearer" = [])),
    responses((status = 200, body = DeciderDto)))]
pub(crate) async fn get_decider(State(s): State<AppState>) -> Result<Json<DeciderDto>, ApiError> {
    let settings = DeciderSettings::load(&s.data_dir()).map_err(ApiError::internal)?;
    Ok(Json(dto(&s, &settings, Vec::new())))
}

/// Change the Decider settings; they take effect for the next turn.
#[utoipa::path(put, path = "/v1/decider", tag = "decider", security(("bearer" = [])),
    request_body = DeciderInput,
    responses((status = 200, body = DeciderDto), (status = 400, body = ErrorBody)))]
pub(crate) async fn put_decider(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<DeciderInput>,
) -> Result<Json<DeciderDto>, ApiError> {
    let mut settings = DeciderSettings::load(&s.data_dir()).map_err(ApiError::internal)?;
    let gemini_model = input
        .gemini_model
        .clone()
        .unwrap_or_else(|| gemini_model_of(&settings));
    if let Some(preset) = input.preset.as_deref() {
        if !PRESETS.contains(&preset) {
            return Err(ApiError::bad_request(format!("unknown preset {preset:?}")));
        }
        settings.backends = match preset {
            "gemini" => vec![gemini_backend(&gemini_model)],
            "openrouter_free" => vec![openrouter_free_backend()],
            _ => vec![gemini_backend(&gemini_model), openrouter_free_backend()],
        };
    } else if let Some(m) = &input.gemini_model {
        for b in &mut settings.backends {
            if b.provider == "gemini" && b.model_match.is_none() {
                b.model.clone_from(m);
            }
        }
    }
    if let Some(e) = input.enabled {
        settings.enabled = e;
    }
    if let Some(b) = input.daily_budget {
        settings.daily_budget = b;
    }
    settings
        .save(&s.data_dir())
        .map_err(ApiError::bad_request)?;
    let warnings = match s.decider_host() {
        Some(host) => apply(host.as_ref(), &s, &settings),
        None => Vec::new(),
    };
    s.storage()
        .append_audit(
            s.user_id(),
            None,
            "user",
            "decider.settings",
            &serde_json::json!({
                "enabled": settings.enabled,
                "preset": preset_of(&settings),
                "daily_budget": settings.daily_budget,
            }),
        )
        .map_err(ApiError::internal)?;
    Ok(Json(dto(&s, &settings, warnings)))
}
