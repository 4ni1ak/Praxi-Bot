use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use provider_catalog::{EmbeddingRequest, EmbeddingResponse, EmbeddingTask, ProviderError};
use tokio::sync::Notify;

use super::*;
use crate::knowledge::{DocumentFormat, DocumentInput, NewSource, Scope, SourceKind, SourceUpdate};
use protocol::{BotId, UserId};

struct Fake {
    calls: AtomicUsize,
    remote: bool,
    fail: AtomicBool,
    started: Notify,
    release: Notify,
    block: AtomicBool,
}

impl Fake {
    fn new(remote: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            remote,
            fail: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
            block: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn endpoint_identity(&self) -> std::result::Result<String, ProviderError> {
        Ok(if self.remote {
            "http://127.0.0.1:1/v1".into()
        } else {
            format!("local-onnx:{}", "0".repeat(64))
        })
    }
    fn is_remote(&self) -> bool {
        self.remote
    }
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> std::result::Result<EmbeddingResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        if self.block.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(ProviderError::RateLimited {
                retry_after: Some(Duration::from_secs(3600)),
            });
        }
        let vectors = request
            .inputs
            .iter()
            .map(|input| {
                let text = input.text.to_lowercase();
                let mut vector = vec![0.; request.dimensions];
                let groups = [
                    &["invoice", "billing", "payment"][..],
                    &["salad", "cooking", "recipe"][..],
                    &["ssh", "server", "login", "access"][..],
                ];
                let index = groups
                    .iter()
                    .position(|group| group.iter().any(|word| text.contains(word)))
                    .unwrap_or(3);
                vector[index % request.dimensions] = 1.;
                vector
            })
            .collect();
        Ok(EmbeddingResponse {
            vectors,
            model: None,
            input_tokens: None,
        })
    }
}

fn scope() -> Scope {
    Scope {
        user_id: UserId::new(),
        bot_id: BotId::new(),
    }
}

fn config(provider: &Fake) -> EmbeddingConfig {
    EmbeddingConfig::new(
        EmbeddingProfile {
            provider_id: "fake".into(),
            model: "fixture-model".into(),
            model_revision: Some("fixture-v1".into()),
            endpoint_identity: provider.endpoint_identity().unwrap(),
            dimensions: 4,
            request_dimensions: false,
            format: if provider.remote {
                EmbeddingTaskFormat::Plain
            } else {
                EmbeddingTaskFormat::E5
            },
        },
        "test-account",
    )
}

fn store() -> Arc<KnowledgeStore> {
    Arc::new(KnowledgeStore::open_in_memory(scope()).unwrap())
}

fn add_document(store: &KnowledgeStore, source_id: Uuid, locator: &str, body: &str) {
    let source = store.source(source_id).unwrap().unwrap();
    store
        .index_document(
            source.id,
            source.revision,
            &DocumentInput {
                locator: locator.into(),
                title: "Notes".into(),
                format: DocumentFormat::PlainText,
                content: body.into(),
                mtime_ms: None,
            },
        )
        .unwrap();
}

fn source(store: &KnowledgeStore, body: &str) -> Uuid {
    let source = store
        .add_source(NewSource::new(SourceKind::Note, "Fixture", "fixture"))
        .unwrap();
    add_document(store, source.id, "one.md", body);
    source.id
}

fn grant(store: &KnowledgeStore, id: Uuid, profile: &EmbeddingProfile) {
    let source = store.source(id).unwrap().unwrap();
    store
        .update_source(
            id,
            source.revision,
            SourceUpdate {
                remote_embedding: Some(Some(RemoteEmbeddingConsent {
                    profile_fingerprint: profile.fingerprint().unwrap(),
                    granted_at_ms: storage::now_ms(),
                })),
                ..SourceUpdate::default()
            },
        )
        .unwrap();
}

fn count(store: &KnowledgeStore, table: &str) -> i64 {
    // Test-only fixed table names supplied by the tests, never user/model input.
    store
        .conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn remote_source_and_query_need_separate_explicit_grants() {
    let store = store();
    let id = source(&store, "Invoice details and billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    let runtime = Arc::new(EmbeddingRuntime::open_in_memory().unwrap());
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        runtime.clone(),
        config.clone(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        service
            .embed_source_batch(id, &cancel)
            .await
            .unwrap()
            .status,
        EmbeddingStatus::AwaitingConsent
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    grant(&store, id, &config.profile);
    assert_eq!(
        service
            .embed_source_batch(id, &cancel)
            .await
            .unwrap()
            .status,
        EmbeddingStatus::Complete
    );
    let response = service
        .search(&SearchRequest::new("billing"), &cancel)
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Lexical);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let mut query_config = config.clone();
    query_config.query_consent = Some(RemoteEmbeddingConsent {
        profile_fingerprint: config.profile.fingerprint().unwrap(),
        granted_at_ms: storage::now_ms(),
    });
    let query_service =
        KnowledgeEmbeddingService::new(store, provider.clone(), runtime, query_config).unwrap();
    let response = query_service
        .search(&SearchRequest::new("payment"), &cancel)
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Hybrid);
    assert_eq!(response.hits[0].source_id, id);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    query_service
        .search(&SearchRequest::new("payment"), &cancel)
        .await
        .unwrap();
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "repeat query uses only the bounded memory cache"
    );
}

#[tokio::test]
async fn source_revocation_purges_remote_vectors_cache_and_blocks_future_calls() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(count(&store, "knowledge_embeddings"), 1);
    assert_eq!(count(&store, "embedding_cache"), 1);
    let source = store.source(id).unwrap().unwrap();
    store
        .update_source(
            id,
            source.revision,
            SourceUpdate {
                remote_embedding: Some(None),
                ..SourceUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
    assert_eq!(count(&store, "embedding_cache"), 0);
    assert_eq!(
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::AwaitingConsent
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn deletion_during_remote_request_cannot_resurrect_chunks_or_vectors() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    provider.block.store(true, Ordering::SeqCst);
    let service = Arc::new(
        KnowledgeEmbeddingService::new(
            store.clone(),
            provider.clone(),
            Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
            config,
        )
        .unwrap(),
    );
    let worker_service = service.clone();
    let worker = tokio::spawn(async move {
        worker_service
            .embed_source_batch(id, &CancellationToken::new())
            .await
    });
    provider.started.notified().await;
    let source = store.source(id).unwrap().unwrap();
    store.delete_source(id, source.revision).unwrap();
    provider.release.notify_one();
    assert_eq!(
        worker.await.unwrap().unwrap().status,
        EmbeddingStatus::Cancelled
    );
    assert_eq!(count(&store, "chunks"), 0);
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
    assert_eq!(count(&store, "embedding_cache"), 0);
}

#[tokio::test]
async fn cancellation_drops_inflight_request_without_publishing() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    provider.block.store(true, Ordering::SeqCst);
    let service = Arc::new(
        KnowledgeEmbeddingService::new(
            store.clone(),
            provider.clone(),
            Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
            config,
        )
        .unwrap(),
    );
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::spawn(async move { service.embed_source_batch(id, &worker_cancel).await });
    provider.started.notified().await;
    cancel.cancel();
    assert_eq!(
        worker.await.unwrap().unwrap().status,
        EmbeddingStatus::Cancelled
    );
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
}

#[tokio::test]
async fn incremental_document_cache_is_source_scoped_and_delete_cascades() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(false));
    let config = config(&provider);
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    add_document(&store, id, "two.md", "Invoice billing.");
    let progress = service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(progress.cache_hits, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&store, "knowledge_embeddings"), 2);
    let source = store.source(id).unwrap().unwrap();
    store.delete_source(id, source.revision).unwrap();
    assert_eq!(count(&store, "embedding_cache"), 0);
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
}

#[tokio::test]
async fn incompatible_profile_generation_cannot_mix_vectors_or_reuse_grants() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    let runtime = Arc::new(EmbeddingRuntime::open_in_memory().unwrap());
    let first = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        runtime.clone(),
        config.clone(),
    )
    .unwrap();
    first
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    let mut changed = config;
    changed.profile.model_revision = Some("fixture-v2".into());
    let second =
        KnowledgeEmbeddingService::new(store.clone(), provider.clone(), runtime, changed).unwrap();
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
    assert_eq!(
        second
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::AwaitingConsent
    );
    assert_eq!(
        first
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::AwaitingConsent
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn remote_failure_is_durable_and_query_falls_back_to_lexical() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let mut config = config(&provider);
    grant(&store, id, &config.profile);
    config.query_consent = Some(RemoteEmbeddingConsent {
        profile_fingerprint: config.profile.fingerprint().unwrap(),
        granted_at_ms: storage::now_ms(),
    });
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    provider.fail.store(true, Ordering::SeqCst);
    let response = service
        .search(&SearchRequest::new("billing"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::LexicalDegraded);
    assert_eq!(response.hits[0].source_id, id);
    add_document(&store, id, "two.md", "Different invoice.");
    assert_eq!(
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::Deferred
    );
    let calls = provider.calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::Deferred
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn semantic_scan_cap_is_visible_and_source_allowlist_is_applied_first() {
    let store = store();
    let invoice = source(&store, "Invoice billing.");
    let food = source(&store, "Salad recipe.");
    let provider = Arc::new(Fake::new(false));
    let mut config = config(&provider);
    config.max_scan_vectors = 1;
    let service = KnowledgeEmbeddingService::new(
        store,
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(invoice, &CancellationToken::new())
        .await
        .unwrap();
    service
        .embed_source_batch(food, &CancellationToken::new())
        .await
        .unwrap();
    let response = service
        .search(&SearchRequest::new("billing"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::LexicalDegraded);
    let mut request = SearchRequest::new("payment");
    request.source_ids = vec![invoice];
    let response = service
        .search(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::Hybrid);
    assert!(response.hits.iter().all(|hit| hit.source_id == invoice));
}

struct InventingReranker;
#[async_trait]
impl LocalReranker for InventingReranker {
    async fn rerank(&self, _query: &str, candidates: &[RerankCandidate]) -> Result<Vec<Uuid>> {
        Ok(vec![Uuid::nil(); candidates.len()])
    }
}

#[tokio::test]
async fn reranker_cannot_invent_or_duplicate_result_ids() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(false));
    let config = config(&provider);
    let service = KnowledgeEmbeddingService::new(
        store,
        provider,
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap()
    .with_local_reranker(Arc::new(InventingReranker));
    let response = service
        .search(&SearchRequest::new("billing"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(response.hits[0].source_id, id);
    assert!(!response.hits[0].id.is_nil());
}

#[test]
fn fingerprint_and_cache_key_separate_model_dimension_task_title_and_destination() {
    let config = config(&Fake::new(true));
    let baseline = config.profile.fingerprint().unwrap();
    for changed in [
        EmbeddingProfile {
            model: "new".into(),
            ..config.profile.clone()
        },
        EmbeddingProfile {
            dimensions: 8,
            ..config.profile.clone()
        },
        EmbeddingProfile {
            endpoint_identity: "https://example.com/v1".into(),
            ..config.profile.clone()
        },
        EmbeddingProfile {
            format: EmbeddingTaskFormat::Gemini2,
            ..config.profile.clone()
        },
    ] {
        assert_ne!(baseline, changed.fingerprint().unwrap());
    }
    assert_ne!(
        cache_key(EmbeddingTask::Document, "same", None),
        cache_key(EmbeddingTask::Query, "same", None)
    );
    assert_ne!(
        cache_key(EmbeddingTask::Document, "same", Some("one")),
        cache_key(EmbeddingTask::Document, "same", Some("two"))
    );
    assert!(normalize(vec![f32::NAN; 4], 4).is_err());
    assert!(normalize(vec![0.; 4], 4).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::await_holding_lock)] // Deliberately freeze quota admission while the source grant changes.
async fn revocation_while_waiting_for_quota_sends_zero_requests() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    let runtime = Arc::new(EmbeddingRuntime::open_in_memory().unwrap());
    let service =
        KnowledgeEmbeddingService::new(store.clone(), provider.clone(), runtime.clone(), config)
            .unwrap();
    let quota_barrier = runtime.conn();
    let worker = tokio::spawn(async move {
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while count(&store, "embedding_jobs") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let source = store.source(id).unwrap().unwrap();
    store
        .update_source(
            id,
            source.revision,
            SourceUpdate {
                remote_embedding: Some(None),
                ..SourceUpdate::default()
            },
        )
        .unwrap();
    drop(quota_barrier);
    assert_eq!(
        worker.await.unwrap().unwrap().status,
        EmbeddingStatus::Cancelled
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn query_larger_than_local_token_budget_still_returns_lexical_results() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let mut config = config(&provider);
    grant(&store, id, &config.profile);
    let runtime = Arc::new(EmbeddingRuntime::open_in_memory().unwrap());
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        runtime.clone(),
        config.clone(),
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    config.budget.tokens_per_minute = Some(1);
    config.query_consent = Some(RemoteEmbeddingConsent {
        profile_fingerprint: config.profile.fingerprint().unwrap(),
        granted_at_ms: storage::now_ms(),
    });
    let service = KnowledgeEmbeddingService::new(store, provider.clone(), runtime, config).unwrap();
    let response = service
        .search(&SearchRequest::new("billing"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(response.mode, SearchMode::LexicalDegraded);
    assert_eq!(response.hits[0].source_id, id);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

struct BlockingReranker {
    started: Notify,
    release: Notify,
}
#[async_trait]
impl LocalReranker for BlockingReranker {
    async fn rerank(&self, _query: &str, candidates: &[RerankCandidate]) -> Result<Vec<Uuid>> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(candidates.iter().map(|candidate| candidate.id).collect())
    }
}

#[tokio::test]
async fn source_deleted_during_local_reranking_is_not_returned_from_a_stale_snapshot() {
    let store = store();
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(false));
    let config = config(&provider);
    let reranker = Arc::new(BlockingReranker {
        started: Notify::new(),
        release: Notify::new(),
    });
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider,
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap()
    .with_local_reranker(reranker.clone());
    let worker = tokio::spawn(async move {
        service
            .search(&SearchRequest::new("billing"), &CancellationToken::new())
            .await
    });
    reranker.started.notified().await;
    let source = store.source(id).unwrap().unwrap();
    store.delete_source(id, source.revision).unwrap();
    reranker.release.notify_one();
    assert_eq!(worker.await.unwrap().unwrap().hits, Vec::new());
}

#[tokio::test]
async fn token_budget_packs_a_smaller_batch_instead_of_failing_a_valid_source() {
    let store = store();
    let id = source(&store, &format!("Invoice billing {}", "a".repeat(1300)));
    for index in 0..5 {
        add_document(
            &store,
            id,
            &format!("{index}.md"),
            &format!("Invoice {index} {}", "b".repeat(1300)),
        );
    }
    let provider = Arc::new(Fake::new(true));
    let mut config = config(&provider);
    config.budget.tokens_per_minute = Some(5000);
    grant(&store, id, &config.profile);
    let service = KnowledgeEmbeddingService::new(
        store,
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    let progress = service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(progress.status, EmbeddingStatus::Progress);
    assert!(progress.embedded_chunks > 0 && progress.embedded_chunks < 6);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::Deferred
    );
}

#[tokio::test]
async fn portable_index_validation_rejects_missing_policy_trigger_and_invalid_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let owner = scope();
    let store = Arc::new(KnowledgeStore::open(dir.path(), owner).unwrap());
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider,
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    let path = dir
        .path()
        .join("bots")
        .join(owner.bot_id.to_string())
        .join("knowledge.db");
    assert_eq!(KnowledgeStore::validate_and_migrate(&path).unwrap(), 2);
    store
        .conn()
        .execute_batch("DROP TRIGGER knowledge_embedding_grant_changed")
        .unwrap();
    assert!(KnowledgeStore::validate_and_migrate(&path).is_err());
    store.conn().execute_batch(index::GRANT_TRIGGER).unwrap();
    store
        .conn()
        .execute(
            "UPDATE knowledge_embeddings SET vector=?1",
            [index::encode_vector(&[f32::NAN, 0., 0., 0.])],
        )
        .unwrap();
    assert!(KnowledgeStore::validate_and_migrate(&path).is_err());
    store
        .conn()
        .execute(
            "UPDATE knowledge_embeddings SET vector=?1",
            [index::encode_vector(&[1., 0., 0., 0.])],
        )
        .unwrap();
    store
        .conn()
        .execute_batch(
            "CREATE TRIGGER unexpected_policy AFTER UPDATE ON sources BEGIN SELECT 1; END",
        )
        .unwrap();
    assert!(KnowledgeStore::validate_and_migrate(&path).is_err());
}

#[tokio::test]
async fn imported_index_suspends_grants_and_discards_jobs_and_remote_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let owner = scope();
    let store = Arc::new(KnowledgeStore::open(dir.path(), owner).unwrap());
    let id = source(&store, "Invoice billing.");
    let provider = Arc::new(Fake::new(true));
    let config = config(&provider);
    grant(&store, id, &config.profile);
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    service
        .embed_source_batch(id, &CancellationToken::new())
        .await
        .unwrap();
    add_document(&store, id, "two.md", "Other invoice.");
    provider.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .embed_source_batch(id, &CancellationToken::new())
            .await
            .unwrap()
            .status,
        EmbeddingStatus::Deferred
    );
    assert_eq!(count(&store, "embedding_jobs"), 1);
    let path = dir
        .path()
        .join("bots")
        .join(owner.bot_id.to_string())
        .join("knowledge.db");
    KnowledgeStore::prepare_import(&path, owner).unwrap();
    assert_eq!(count(&store, "embedding_jobs"), 0);
    assert_eq!(count(&store, "knowledge_embeddings"), 0);
    assert_eq!(count(&store, "embedding_cache"), 0);
    let source = store.source(id).unwrap().unwrap();
    assert!(!source.enabled);
    assert!(source.remote_embedding.is_none());
}
