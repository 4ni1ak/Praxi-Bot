//! Hermetic semantic-path contract, not a benchmark of a downloaded ML model.
//! Query vocabulary is held out from the documents. Disabling query vectors
//! must destroy measured recall, making this gate sensitive to broken fusion.

use async_trait::async_trait;
use memory_store::knowledge::embedding::{
    EmbeddingConfig, EmbeddingProfile, EmbeddingRuntime, EmbeddingStatus, KnowledgeEmbeddingService,
};
use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, Scope, SearchRequest, SourceKind,
};
use protocol::{BotId, UserId};
use provider_catalog::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResponse, EmbeddingTask, EmbeddingTaskFormat,
    ProviderError,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CASES: [(&str, &str); 8] = [
    (
        "recover yesterday following drive loss",
        "Nightly snapshots retain seven rotations and restore transactional consistency.",
    ),
    (
        "remove obsolete cached entries",
        "Evict memoized values whenever the deployment revision changes.",
    ),
    (
        "reach machines without passwords",
        "SSH authenticates administrators using Ed25519 keypairs.",
    ),
    (
        "split purchases into regular instalments",
        "Invoices support recurring billing with prorated credits.",
    ),
    (
        "which vegetables tolerate frost",
        "Kale and spinach thrive in chilly garden beds.",
    ),
    (
        "switch the interface to Turkish",
        "Localization catalogs contain tr locale strings.",
    ),
    (
        "nighttime colours strain my eyes",
        "Dark themes use muted backgrounds and accessible foreground contrast.",
    ),
    (
        "prevent duplicate customer records",
        "MySQL unique constraints enforce email identity atomically.",
    ),
];

struct ConceptFixture {
    broken_query: bool,
}
#[async_trait]
impl EmbeddingProvider for ConceptFixture {
    fn id(&self) -> &'static str {
        "semantic-fixture"
    }
    fn endpoint_identity(&self) -> Result<String, ProviderError> {
        Ok(format!("local-onnx:{}", "b".repeat(64)))
    }
    fn is_remote(&self) -> bool {
        false
    }
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        let vectors = request
            .inputs
            .iter()
            .map(|input| {
                let index = if request.task == EmbeddingTask::Query {
                    if self.broken_query {
                        None
                    } else {
                        CASES.iter().position(|(query, _)| *query == input.text)
                    }
                } else {
                    CASES
                        .iter()
                        .position(|(_, document)| input.text.starts_with(document))
                };
                let mut vector = vec![0.; CASES.len() + 1];
                vector[index.unwrap_or(CASES.len())] = 1.;
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

async fn fixture(broken_query: bool) -> (Arc<KnowledgeStore>, KnowledgeEmbeddingService, String) {
    let store = Arc::new(
        KnowledgeStore::open_in_memory(Scope {
            user_id: UserId::new(),
            bot_id: BotId::new(),
        })
        .unwrap(),
    );
    let mut stuffed = String::new();
    for (index, (_, body)) in CASES.iter().enumerate() {
        let source = store
            .add_source(NewSource::new(
                SourceKind::Note,
                format!("Reference {index}"),
                format!("note:{index}"),
            ))
            .unwrap();
        let body = format!(
            "{body}\n{}",
            "Record the outcome in the operational log. Verify consistency before publication. "
                .repeat(12)
        );
        stuffed.push_str(&body);
        store
            .index_document(
                source.id,
                source.revision,
                &DocumentInput {
                    locator: format!("reference-{index}.md"),
                    title: "Reference".into(),
                    format: DocumentFormat::PlainText,
                    content: body,
                    mtime_ms: None,
                },
            )
            .unwrap();
    }
    let provider = Arc::new(ConceptFixture { broken_query });
    let profile = EmbeddingProfile {
        provider_id: provider.id().into(),
        model: "held-out-concepts".into(),
        model_revision: Some("v1".into()),
        endpoint_identity: provider.endpoint_identity().unwrap(),
        dimensions: CASES.len() + 1,
        request_dimensions: false,
        format: EmbeddingTaskFormat::E5,
    };
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider,
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        EmbeddingConfig::new(profile, "local-eval"),
    )
    .unwrap();
    for source in store.list_sources().unwrap() {
        loop {
            let progress = service
                .embed_source_batch(source.id, &CancellationToken::new())
                .await
                .unwrap();
            if progress.status == EmbeddingStatus::Complete {
                break;
            }
            assert_eq!(progress.status, EmbeddingStatus::Progress);
        }
    }
    (store, service, stuffed)
}

#[tokio::test]
async fn held_out_semantic_queries_require_vectors_and_measure_token_savings() {
    let force_broken = std::env::var_os("PRAXI_EVAL_BREAK_QUERY_VECTORS").is_some();
    let (store, service, stuffed) = fixture(force_broken).await;
    let tokenizer = tiktoken_rs::cl100k_base().unwrap();
    let stuffed_tokens = tokenizer.encode_ordinary(&stuffed).len();
    let mut correct = 0_u32;
    let mut token_total = 0_usize;
    for (index, (query, _)) in CASES.iter().enumerate() {
        let request = SearchRequest {
            top_k: 1,
            budget_tokens: 600,
            ..SearchRequest::new(*query)
        };
        assert!(
            store.search(&request).unwrap().hits.is_empty(),
            "case {index} must not be solvable lexically"
        );
        let response = service
            .search(&request, &CancellationToken::new())
            .await
            .unwrap();
        if response
            .hits
            .first()
            .is_some_and(|hit| hit.locator == format!("reference-{index}.md"))
        {
            correct += 1;
        }
        let tokens = tokenizer
            .encode_ordinary(&serde_json::to_string(&response).unwrap())
            .len();
        assert!(
            tokens <= 600,
            "serialized response exceeded reference tokenizer budget: {tokens}"
        );
        token_total += tokens;
    }
    let recall = f64::from(correct) / 8.;
    let mean_tokens = token_total / CASES.len();
    eprintln!(
        "semantic-only held-out Recall@1={recall:.3} MRR@1={recall:.3}; lexical Recall@1=0; mean response={mean_tokens} tokens; stuffed corpus={stuffed_tokens} tokens"
    );
    assert_eq!(
        correct, 8,
        "semantic recall gate must fail when vectors are disconnected"
    );
    assert!(
        mean_tokens * 2 < stuffed_tokens,
        "retrieval must save at least half the fixture context"
    );
    assert_eq!(
        service
            .search(
                &SearchRequest::new("unrelated orbital mechanics"),
                &CancellationToken::new()
            )
            .await
            .unwrap()
            .hits
            .len(),
        0
    );
    // A separately built negative control proves lexical/fusion cannot mask a
    // disconnected vector channel with this fixture.
    let (_, broken, _) = fixture(true).await;
    for (query, _) in CASES {
        assert_eq!(
            broken
                .search(&SearchRequest::new(query), &CancellationToken::new())
                .await
                .unwrap()
                .hits
                .len(),
            0
        );
    }
}
