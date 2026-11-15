//! Existing provider-list route's additive, presentation-only model metadata.

use agent_core::AgentLoop;
use provider_catalog::ProviderSettings;
use serde::Deserialize;

use crate::dto::ModelMetadataDto;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(crate) struct ProvidersQuery {
    /// Default true: omit deprecated and non-chat models. False exposes all
    /// kinds for advanced clients; it does not change bot execution.
    pub chat_only: Option<bool>,
}

pub(crate) fn for_picker(
    agent: &AgentLoop,
    settings: &ProviderSettings,
    mut ids: Vec<String>,
    chat_only: bool,
) -> Vec<ModelMetadataDto> {
    if settings.cli.is_some() {
        // The CLI has no live models endpoint. Keep its existing configured
        // model/default behavior; never infer account entitlements from docs.
        ids.push(
            settings
                .default_model
                .clone()
                .unwrap_or_else(|| "default".into()),
        );
    }
    let models: Vec<ModelMetadataDto> =
        provider_catalog::model_metadata(&settings.id, &ids, chat_only)
            .into_iter()
            .map(Into::into)
            .collect();
    // Refresh the production redactor once for the whole list, not once per
    // id (a refresh may read the OS secret store). All presentation fields are
    // scrubbed too. If redaction damages the JSON structure, withhold the list.
    let Ok(json) = serde_json::to_string(&models) else {
        return Vec::new();
    };
    let redacted = agent.redact_host_text(json.clone());
    if redacted == json {
        return models;
    }
    let Ok(scrubbed) = serde_json::from_str::<Vec<ModelMetadataDto>>(&redacted) else {
        return Vec::new();
    };
    if scrubbed.len() != models.len() {
        return Vec::new();
    }
    models
        .into_iter()
        .zip(scrubbed)
        // Never turn a secret-bearing wire id into a different executable id.
        .filter_map(|(original, safe)| (original.id == safe.id).then_some(safe))
        .collect()
}
