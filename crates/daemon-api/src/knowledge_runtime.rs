//! Host-owned embedding dispatch. Tool arguments never select a provider or
//! grant egress. Settings/source mutations share the authority lock with each
//! bounded dispatch, so revocation is ordered before the next request.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use builtin_tools::knowledge::{KnowledgeBackend, LocalKnowledgeBackend};
use memory_store::knowledge::embedding::{
    EmbeddingConfig, EmbeddingProfile, EmbeddingProgress, EmbeddingRuntime,
    KnowledgeEmbeddingService,
};
use memory_store::knowledge::{KnowledgeStore, Scope, SearchRequest, SearchResponse};
use protocol::BotId;
use provider_catalog::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResponse, OpenAiCompatConfig, ProviderConfig,
    ProviderError,
};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tool_registry::ToolError;
use uuid::Uuid;

use crate::{ApiError, AppState, WeakAppState};

type Service = Arc<KnowledgeEmbeddingService>;
type Prepared = (Service, OwnedMutexGuard<()>);
type AuthorityLocks = parking_lot::Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>;

struct CachedService {
    bot: BotId,
    key: String,
    service: Service,
}

/// One authority lane per data directory, shared even by multiple state handles.
/// Independent hosts do not block one another. Idle entries retain no mutex;
/// blocking initialization keeps its guard if the HTTP caller disconnects.
pub(crate) fn authority_lock(state: &AppState) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<AuthorityLocks> = OnceLock::new();
    let path = state.data_dir();
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let mut locks = LOCKS.get_or_init(AuthorityLocks::default).lock();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path, Arc::downgrade(&lock));
    lock
}

#[cfg(test)]
pub(crate) async fn test_guard() -> OwnedMutexGuard<()> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    Arc::clone(LOCK.get_or_init(|| Arc::new(Mutex::new(()))))
        .lock_owned()
        .await
}

pub(crate) struct KnowledgeRuntime {
    state: WeakAppState,
    local: LocalKnowledgeBackend,
    services: Arc<parking_lot::Mutex<VecDeque<CachedService>>>,
}

pub(crate) fn install(state: &AppState) {
    let runtime = Arc::new(KnowledgeRuntime {
        state: state.downgrade(),
        local: LocalKnowledgeBackend::new(state.storage_arc()),
        services: Arc::default(),
    });
    if state.0.knowledge_runtime.set(Arc::clone(&runtime)).is_ok() {
        let tools = state.agent().tool_registry();
        tools.unregister(builtin_tools::knowledge::KNOWLEDGE_SEARCH);
        tools.register(builtin_tools::knowledge::tool(runtime));
        tools.unregister(builtin_tools::knowledge_brain::KNOWLEDGE_BRAIN_WRITE);
        tools.register(builtin_tools::knowledge_brain::tool(state.storage_arc()));
    }
}

/// Drop cached database/model handles before deleting a bot's scoped index.
/// The caller must hold [`authority_lock`] throughout cleanup.
pub(crate) fn forget_bot(state: &AppState, bot: BotId) {
    if let Some(runtime) = state.0.knowledge_runtime.get() {
        runtime.services.lock().retain(|entry| entry.bot != bot);
    }
}

fn settings(state: &AppState, bot: BotId) -> Result<EmbeddingConfig, ApiError> {
    let bot = state.bot_of_user(bot)?;
    let value = bot
        .config
        .get("knowledge")
        .and_then(|value| value.get("embedding"))
        .filter(|value| !value.is_null())
        .ok_or_else(|| ApiError::conflict("no embedding provider is configured"))?;
    let config: EmbeddingConfig = serde_json::from_value(value.clone())
        .map_err(|_| ApiError::bad_request("invalid embedding settings"))?;
    config
        .validate()
        .map_err(|_| ApiError::bad_request("invalid embedding settings"))?;
    Ok(config)
}

impl KnowledgeRuntime {
    async fn prepare(
        &self,
        state: AppState,
        bot: BotId,
        gate: OwnedMutexGuard<()>,
    ) -> Result<Prepared, ApiError> {
        let config = settings(&state, bot).inspect_err(|_| forget_bot(&state, bot))?;
        let bytes = serde_json::to_vec(&config)
            .map_err(|_| ApiError::bad_request("invalid embedding settings"))?;
        let key = format!("{:x}", Sha256::digest(bytes));
        if let Some(service) = self
            .services
            .lock()
            .iter()
            .find(|entry| entry.bot == bot && entry.key == key)
            .map(|entry| Arc::clone(&entry.service))
        {
            return Ok((service, gate));
        }
        let services = Arc::clone(&self.services);
        tokio::task::spawn_blocking(move || {
            // Release the previous model generation before loading another.
            forget_bot(&state, bot);
            let provider = provider(&state, &config)?;
            let store = Arc::new(
                KnowledgeStore::open(
                    state.data_dir(),
                    Scope {
                        user_id: state.user_id(),
                        bot_id: bot,
                    },
                )
                .map_err(|_| ApiError::unavailable("knowledge index unavailable"))?,
            );
            let ledger = Arc::new(
                EmbeddingRuntime::open(state.data_dir())
                    .map_err(|_| ApiError::unavailable("embedding budget ledger unavailable"))?,
            );
            let service = Arc::new(
                KnowledgeEmbeddingService::new(store, provider, ledger, config)
                    .map_err(|_| ApiError::bad_request("embedding profile is unavailable"))?,
            );
            let mut cache = services.lock();
            while cache.len() >= 16 {
                cache.pop_front();
            }
            cache.push_back(CachedService {
                bot,
                key,
                service: Arc::clone(&service),
            });
            Ok((service, gate))
        })
        .await
        .map_err(|_| ApiError::unavailable("embedding worker unavailable"))?
    }
}

#[async_trait]
impl KnowledgeBackend for KnowledgeRuntime {
    fn may_egress(&self) -> bool {
        // One registered tool serves bots whose settings can change. An
        // optimistic static classification would bypass PR #46's pre-call gate.
        true
    }

    async fn search(
        &self,
        bot: BotId,
        request: SearchRequest,
    ) -> Result<SearchResponse, ToolError> {
        let state = self
            .state
            .upgrade()
            .ok_or_else(|| ToolError::Execution("knowledge host unavailable".into()))?;
        state
            .bot_of_user(bot)
            .map_err(|_| ToolError::Execution("knowledge bot unavailable".into()))?;
        if let Ok(gate) = authority_lock(&state).try_lock_owned()
            && let Ok((service, _gate)) = self.prepare(state.clone(), bot, gate).await
            && let Ok(response) = service.search(&request, &state.shutdown_token()).await
        {
            return Ok(response);
        }
        self.local.search(bot, request).await
    }
}

/// Execute one resumable source-embedding batch with the current owner settings.
///
/// # Errors
/// Unknown source/bot, stale revision, concurrent management, unavailable local
/// artifacts, or invalid settings. Provider response bodies are never returned.
pub async fn embedding_batch(
    state: &AppState,
    bot: BotId,
    source: Uuid,
    expected_revision: u64,
) -> Result<EmbeddingProgress, ApiError> {
    run_embedding_batch(state, bot, source, expected_revision, false).await
}

/// Explicit owner retry of a failed source. Automatic continuation must use
/// [`embedding_batch`] so it cannot repeatedly reset the bounded attempt count.
/// The durable account quota and provider cooldown are never reset here.
///
/// # Errors
/// The same ownership, revision and configuration errors as [`embedding_batch`].
pub async fn retry_embedding_batch(
    state: &AppState,
    bot: BotId,
    source: Uuid,
    expected_revision: u64,
) -> Result<EmbeddingProgress, ApiError> {
    run_embedding_batch(state, bot, source, expected_revision, true).await
}

async fn run_embedding_batch(
    state: &AppState,
    bot: BotId,
    source: Uuid,
    expected_revision: u64,
    reset_retry: bool,
) -> Result<EmbeddingProgress, ApiError> {
    let gate = authority_lock(state)
        .try_lock_owned()
        .map_err(|_| ApiError::conflict("knowledge management is busy; retry shortly"))?;
    state.bot_of_user(bot)?;
    let runtime = state
        .0
        .knowledge_runtime
        .get()
        .ok_or_else(|| ApiError::unavailable("knowledge runtime unavailable"))?;
    let (service, _gate) = runtime.prepare(state.clone(), bot, gate).await?;
    // Check the browser's revision before any egress; the embedding service
    // rechecks grants and chunk revisions before dispatch and publication.
    let store = KnowledgeStore::open(
        state.data_dir(),
        Scope {
            user_id: state.user_id(),
            bot_id: bot,
        },
    )
    .map_err(|_| ApiError::unavailable("knowledge index unavailable"))?;
    let current = store
        .source(source)
        .map_err(|_| ApiError::unavailable("knowledge index unavailable"))?
        .ok_or_else(|| ApiError::not_found("knowledge source"))?;
    if current.revision != expected_revision || !current.enabled {
        return Err(ApiError::conflict(
            "knowledge source changed or is disabled",
        ));
    }
    drop(store);
    if reset_retry {
        service
            .reset_source_retry(source)
            .map_err(|_| ApiError::unavailable("embedding retry state unavailable"))?;
    }
    service
        .embed_source_batch(source, &state.shutdown_token())
        .await
        .map_err(|_| ApiError::unavailable("embedding batch unavailable"))
}

fn provider(
    state: &AppState,
    config: &EmbeddingConfig,
) -> Result<Arc<dyn EmbeddingProvider>, ApiError> {
    if config.profile.endpoint_identity.starts_with("local-onnx:") {
        return local_provider(config);
    }
    // Fail closed on stale endpoint/provider settings, even for a cache miss.
    resolve_provider(state, &config.profile).map_err(|_| {
        ApiError::conflict("configured embedding provider changed or is unavailable")
    })?;
    Ok(Arc::new(ConfiguredProvider {
        state: state.downgrade(),
        profile: config.profile.clone(),
    }))
}

#[cfg(feature = "local-embeddings")]
fn local_provider(config: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>, ApiError> {
    let artifacts = config
        .local_artifacts
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("local embedding artifacts are required"))?;
    let provider = memory_store::knowledge::embedding::LocalOnnxEmbedder::load(artifacts)
        .map_err(|_| ApiError::bad_request("local embedding artifacts failed validation"))?;
    Ok(Arc::new(provider))
}

#[cfg(not(feature = "local-embeddings"))]
fn local_provider(_: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>, ApiError> {
    Err(ApiError::bad_request(
        "this build does not include local embeddings",
    ))
}

struct ConfiguredProvider {
    state: WeakAppState,
    profile: EmbeddingProfile,
}

fn resolve_provider(
    state: &AppState,
    profile: &EmbeddingProfile,
) -> Result<Box<dyn EmbeddingProvider>, ProviderError> {
    let _lock = state.providers_lock();
    let config = ProviderConfig::load(&provider_catalog::config_path(&state.data_dir()))
        .map_err(|_| invalid_provider())?;
    let settings = config
        .get(&profile.provider_id)
        .ok_or_else(invalid_provider)?;
    let entry = settings
        .custom
        .as_ref()
        .or_else(|| state.provider_host()?.catalog.get(&profile.provider_id))
        .ok_or_else(invalid_provider)?;
    if provider_catalog::embedding::endpoint_identity(&entry.base_url)? != profile.endpoint_identity
    {
        return Err(invalid_provider());
    }
    let secrets = state
        .secret_store()
        .unwrap_or_else(|| Arc::new(secret_store::MemoryStore::new()));
    settings.build_embedding_provider(entry, secrets.as_ref(), OpenAiCompatConfig::default())
}

fn invalid_provider() -> ProviderError {
    ProviderError::InvalidRequest("embedding provider settings changed or are unavailable".into())
}

#[async_trait]
impl EmbeddingProvider for ConfiguredProvider {
    fn id(&self) -> &str {
        &self.profile.provider_id
    }

    fn endpoint_identity(&self) -> Result<String, ProviderError> {
        Ok(self.profile.endpoint_identity.clone())
    }

    async fn embed(
        &self,
        mut request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let state = self.state.upgrade().ok_or_else(invalid_provider)?;
        // Resolve every attempt, so cached services never retain old credentials
        // or authorize a changed destination. The HTTP adapter makes one attempt.
        let provider = resolve_provider(&state, &self.profile)?;
        state.agent().refresh_external_redactor();
        for input in &mut request.inputs {
            input.text = state
                .agent()
                .redact_external_text(std::mem::take(&mut input.text));
            input.title = input
                .title
                .take()
                .map(|title| state.agent().redact_external_text(title));
        }
        provider.embed(request).await
    }
}

#[cfg(test)]
mod tests;
