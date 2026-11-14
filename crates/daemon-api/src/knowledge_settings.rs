//! Knowledge configuration authored by the authenticated user, never by a
//! retrieved file. Provider endpoints and quota groups are resolved server-side.

use axum::{
    Json,
    extract::{Path, State},
};
use memory_store::knowledge::{
    KnowledgeStore, RemoteEmbeddingConsent, Scope, SourceUpdate,
    embedding::{
        EmbeddingConfig, EmbeddingProfile, LocalEmbeddingArtifacts, PinnedEmbeddingArtifact,
        local_embeddings_available,
    },
};
use protocol::BotId;
use provider_catalog::{EmbeddingTaskFormat, ProviderConfig, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ApiError, AppState, error::ErrorBody, routes::ApiJson};

#[derive(Debug, Serialize, ToSchema)]
pub struct KnowledgeSettingsDto {
    pub auto_retrieve: bool,
    pub embedding: Option<KnowledgeEmbeddingDto>,
    pub local_embeddings_available: bool,
}

/// An explicit operator-selected artifact. Files are hash-verified before load.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeArtifact {
    pub path: String,
    pub sha256: String,
}

/// Complete offline E5 bundle, including the explicitly selected native runtime.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeLocalArtifacts {
    pub runtime: KnowledgeArtifact,
    pub model: KnowledgeArtifact,
    pub tokenizer: KnowledgeArtifact,
    pub config: KnowledgeArtifact,
    pub special_tokens_map: KnowledgeArtifact,
    pub tokenizer_config: KnowledgeArtifact,
}

impl From<KnowledgeArtifact> for PinnedEmbeddingArtifact {
    fn from(value: KnowledgeArtifact) -> Self {
        Self {
            path: value.path.into(),
            sha256: value.sha256,
        }
    }
}

impl From<PinnedEmbeddingArtifact> for KnowledgeArtifact {
    fn from(value: PinnedEmbeddingArtifact) -> Self {
        Self {
            path: value.path.to_string_lossy().into_owned(),
            sha256: value.sha256,
        }
    }
}

impl From<KnowledgeLocalArtifacts> for LocalEmbeddingArtifacts {
    fn from(value: KnowledgeLocalArtifacts) -> Self {
        Self {
            runtime: value.runtime.into(),
            model: value.model.into(),
            tokenizer: value.tokenizer.into(),
            config: value.config.into(),
            special_tokens_map: value.special_tokens_map.into(),
            tokenizer_config: value.tokenizer_config.into(),
        }
    }
}

impl From<LocalEmbeddingArtifacts> for KnowledgeLocalArtifacts {
    fn from(value: LocalEmbeddingArtifacts) -> Self {
        Self {
            runtime: value.runtime.into(),
            model: value.model.into(),
            tokenizer: value.tokenizer.into(),
            config: value.config.into(),
            special_tokens_map: value.special_tokens_map.into(),
            tokenizer_config: value.tokenizer_config.into(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct KnowledgeEmbeddingDto {
    pub provider_id: String,
    pub model: String,
    pub dimensions: usize,
    pub request_dimensions: bool,
    pub endpoint: String,
    pub profile_fingerprint: String,
    pub query_remote: bool,
    pub requests_per_minute: u32,
    pub requests_per_day: u32,
    pub local_artifacts: Option<KnowledgeLocalArtifacts>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEmbeddingInput {
    pub provider_id: String,
    pub model: String,
    pub dimensions: usize,
    #[serde(default)]
    pub request_dimensions: bool,
    /// Separate explicit permission to send retrieval queries to this provider.
    #[serde(default)]
    pub query_remote: bool,
    /// Exact profile previously displayed to the user; required for query egress.
    pub consent_profile_fingerprint: Option<String>,
    pub requests_per_minute: u32,
    pub requests_per_day: u32,
    pub local_artifacts: Option<KnowledgeLocalArtifacts>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSettingsInput {
    pub auto_retrieve: bool,
    /// Null explicitly disables embeddings; lexical retrieval remains available.
    pub embedding: Option<KnowledgeEmbeddingInput>,
}

fn embedding_config(bot: &storage::Bot) -> Result<Option<EmbeddingConfig>, ApiError> {
    let value = &bot.config["knowledge"]["embedding"];
    if value.is_null() {
        return Ok(None);
    }
    let config: EmbeddingConfig = serde_json::from_value(value.clone())
        .map_err(|_| ApiError::conflict("invalid knowledge embedding settings; save them again"))?;
    config
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Some(config))
}

fn profile_fingerprint(config: &EmbeddingConfig) -> Result<String, ApiError> {
    config
        .profile
        .fingerprint()
        .map_err(|e| ApiError::bad_request(e.to_string()))
}

fn settings_dto(bot: &storage::Bot) -> Result<KnowledgeSettingsDto, ApiError> {
    let embedding = embedding_config(bot)?
        .map(|config| -> Result<KnowledgeEmbeddingDto, ApiError> {
            let fingerprint = profile_fingerprint(&config)?;
            Ok(KnowledgeEmbeddingDto {
                provider_id: config.profile.provider_id,
                model: config.profile.model,
                dimensions: config.profile.dimensions,
                request_dimensions: config.profile.request_dimensions,
                endpoint: config.profile.endpoint_identity,
                query_remote: config
                    .query_consent
                    .is_some_and(|g| g.profile_fingerprint == fingerprint),
                profile_fingerprint: fingerprint,
                requests_per_minute: config.budget.requests_per_minute,
                requests_per_day: config.budget.requests_per_day,
                local_artifacts: config.local_artifacts.map(Into::into),
            })
        })
        .transpose()?;
    Ok(KnowledgeSettingsDto {
        auto_retrieve: bot.config["knowledge"]["auto_retrieve"]
            .as_bool()
            .unwrap_or(false),
        embedding,
        local_embeddings_available: local_embeddings_available(),
    })
}

fn configured_provider(
    s: &AppState,
    id: &str,
) -> Result<
    (
        provider_catalog::ProviderSettings,
        provider_catalog::ProviderEntry,
    ),
    ApiError,
> {
    let providers = ProviderConfig::load(&provider_catalog::config_path(&s.data_dir()))
        .map_err(|_| ApiError::internal("provider settings unavailable"))?;
    let settings = providers
        .get(id)
        .filter(|p| p.enabled && p.cli.is_none())
        .cloned()
        .ok_or_else(|| ApiError::bad_request("choose an enabled HTTP provider"))?;
    let entry = settings
        .custom
        .clone()
        .or_else(|| s.provider_host().and_then(|h| h.catalog.get(id)).cloned())
        .ok_or_else(|| ApiError::bad_request("provider is not in the configured catalog"))?;
    if !matches!(
        entry.kind,
        ProviderKind::Gemini | ProviderKind::OpenAiCompatible
    ) {
        return Err(ApiError::bad_request(
            "provider does not support embeddings",
        ));
    }
    Ok((settings, entry))
}

fn make_embedding(
    s: &AppState,
    input: KnowledgeEmbeddingInput,
) -> Result<EmbeddingConfig, ApiError> {
    if input.provider_id == "local-onnx" {
        if input.query_remote {
            return Err(ApiError::bad_request(
                "local embeddings do not accept remote query consent",
            ));
        }
        let artifacts: LocalEmbeddingArtifacts = input
            .local_artifacts
            .ok_or_else(|| {
                ApiError::bad_request("local embeddings require all six pinned artifacts")
            })?
            .into();
        let profile = artifacts
            .profile()
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let account = profile.endpoint_identity.clone();
        let mut config = EmbeddingConfig::new(profile, account);
        config.local_artifacts = Some(artifacts);
        config
            .validate()
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        return Ok(config);
    }
    if input.local_artifacts.is_some() {
        return Err(ApiError::bad_request(
            "local artifacts require the local-onnx provider",
        ));
    }
    let (settings, entry) = configured_provider(s, &input.provider_id)?;
    let endpoint = provider_catalog::embedding::endpoint_identity(&entry.base_url)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let format = if entry.kind == ProviderKind::Gemini {
        match input.model.as_str() {
            "gemini-embedding-001" => EmbeddingTaskFormat::Gemini001,
            "gemini-embedding-2" | "gemini-embedding-2-preview" => EmbeddingTaskFormat::Gemini2,
            _ => {
                return Err(ApiError::bad_request(
                    "unsupported Gemini embedding model; choose gemini-embedding-001, gemini-embedding-2 or gemini-embedding-2-preview",
                ));
            }
        }
    } else {
        EmbeddingTaskFormat::Plain
    };
    let profile = EmbeddingProfile {
        provider_id: settings.id.clone(),
        model: input.model,
        model_revision: None,
        endpoint_identity: endpoint,
        dimensions: input.dimensions,
        request_dimensions: input.request_dimensions,
        format,
    };
    // Aliases of one saved credential debit a single host-wide quota. Only a
    // name is hashed; secret bytes are never read here. Without a saved handle,
    // the provider identity is the available conservative local grouping key.
    let account = settings.key.as_ref().map_or_else(
        || format!("environment-provider:{}", settings.id),
        |reference| format!("secret-ref:{}", reference.name()),
    );
    let mut config =
        EmbeddingConfig::new(profile, format!("{:x}", Sha256::digest(account.as_bytes())));
    config.budget.requests_per_minute = input.requests_per_minute;
    config.budget.requests_per_day = input.requests_per_day;
    config
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if input.query_remote {
        let fingerprint = profile_fingerprint(&config)?;
        if input.consent_profile_fingerprint.as_deref() != Some(&fingerprint) {
            return Err(ApiError::conflict(
                "save and review the embedding destination before enabling query egress",
            ));
        }
        config.query_consent = Some(RemoteEmbeddingConsent {
            profile_fingerprint: fingerprint,
            granted_at_ms: storage::now_ms(),
        });
    }
    Ok(config)
}

/// Grant only the current configured provider/model/destination identity.
pub(crate) fn source_consent(s: &AppState, bot: Uuid) -> Result<RemoteEmbeddingConsent, ApiError> {
    let bot = s.bot_of_user(BotId(bot))?;
    let config = embedding_config(&bot)?
        .ok_or_else(|| ApiError::conflict("configure an embedding provider first"))?;
    if config.local_artifacts.is_some() {
        return Err(ApiError::bad_request(
            "local embeddings do not need a remote source grant",
        ));
    }
    let (_, entry) = configured_provider(s, &config.profile.provider_id)?;
    let endpoint = provider_catalog::embedding::endpoint_identity(&entry.base_url)
        .map_err(|_| ApiError::bad_request("invalid embedding endpoint"))?;
    if endpoint != config.profile.endpoint_identity {
        return Err(ApiError::conflict(
            "provider destination changed; save the embedding settings again",
        ));
    }
    Ok(RemoteEmbeddingConsent {
        profile_fingerprint: profile_fingerprint(&config)?,
        granted_at_ms: storage::now_ms(),
    })
}

#[utoipa::path(get, path = "/v1/bots/{bot}/knowledge/settings", tag = "knowledge", security(("bearer" = [])), params(("bot" = Uuid, Path)), responses((status = 200, body = KnowledgeSettingsDto), (status = 404, body = ErrorBody)))]
pub(crate) async fn get_settings(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
) -> Result<Json<KnowledgeSettingsDto>, ApiError> {
    Ok(Json(settings_dto(&s.bot_of_user(BotId(bot))?)?))
}

#[utoipa::path(put, path = "/v1/bots/{bot}/knowledge/settings", tag = "knowledge", security(("bearer" = [])), params(("bot" = Uuid, Path)), request_body = KnowledgeSettingsInput, responses((status = 200, body = KnowledgeSettingsDto), (status = 400, body = ErrorBody)))]
pub(crate) async fn put_settings(
    State(s): State<AppState>,
    Path(bot): Path<Uuid>,
    ApiJson(input): ApiJson<KnowledgeSettingsInput>,
) -> Result<Json<KnowledgeSettingsDto>, ApiError> {
    let _authority = crate::knowledge::authority_lock(&s)
        .try_lock_owned()
        .map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "knowledge_busy",
                "a knowledge authority change is already running",
            )
        })?;
    let mut bot = s.bot_of_user(BotId(bot))?;
    let embedding = input
        .embedding
        .map(|settings| make_embedding(&s, settings))
        .transpose()?;
    let new_fingerprint = embedding.as_ref().map(profile_fingerprint).transpose()?;
    let store = KnowledgeStore::open(
        s.data_dir(),
        Scope {
            user_id: bot.user_id,
            bot_id: bot.id,
        },
    )
    .map_err(|_| ApiError::internal("knowledge index unavailable"))?;
    for source in store
        .list_sources()
        .map_err(|_| ApiError::internal("knowledge sources unavailable"))?
    {
        if source
            .remote_embedding
            .as_ref()
            .is_some_and(|grant| Some(&grant.profile_fingerprint) != new_fingerprint.as_ref())
        {
            store
                .update_source(
                    source.id,
                    source.revision,
                    SourceUpdate {
                        remote_embedding: Some(None),
                        ..Default::default()
                    },
                )
                .map_err(|_| {
                    ApiError::conflict("source changed while revoking old embedding consent")
                })?;
        }
    }
    bot.config["knowledge"] = json!({"auto_retrieve":input.auto_retrieve,"embedding":embedding});
    if !s.storage().update_bot(bot.id, &bot.name, &bot.config)? {
        return Err(ApiError::not_found("bot"));
    }
    // The authority lock also guards runtime dispatch, so no old query/model
    // generation can remain active after the new settings become visible.
    crate::knowledge_runtime::forget_bot(&s, bot.id);
    s.refresh_profile(&bot);
    Ok(Json(settings_dto(&bot)?))
}
