//! Offline source-quality and reference-token measurement over versioned notes.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use memory_store::estimate_tokens;
use memory_store::knowledge::embedding::{
    EmbeddingConfig, EmbeddingProfile, EmbeddingRuntime, EmbeddingStatus, KnowledgeEmbeddingService,
};
use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, Scope, SearchMode, SearchRequest,
    SourceKind,
};
use protocol::{BotId, UserId};
use provider_catalog::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResponse, EmbeddingTask, EmbeddingTaskFormat,
    ProviderError,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    query: String,
    expected_sources: Vec<String>,
    #[serde(default)]
    expected_first: Option<String>,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/retrieval")
}

fn corpus() -> BTreeMap<String, String> {
    std::fs::read_dir(fixtures().join("corpus"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_str().unwrap().to_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect()
}

fn indexed_corpus(corpus: &BTreeMap<String, String>) -> KnowledgeStore {
    let store = KnowledgeStore::open_in_memory(Scope {
        user_id: UserId::new(),
        bot_id: BotId::new(),
    })
    .unwrap();
    for (name, text) in corpus {
        let source = store
            .add_source(NewSource::new(SourceKind::File, name, name))
            .unwrap();
        store
            .index_document(
                source.id,
                source.revision,
                &DocumentInput {
                    locator: name.clone(),
                    title: name.clone(),
                    format: DocumentFormat::Markdown,
                    content: text.clone(),
                    mtime_ms: Some(1),
                },
            )
            .unwrap();
    }
    store
}

fn stuffed_context(corpus: &BTreeMap<String, String>) -> String {
    let mut text = String::new();
    for (name, body) in corpus {
        writeln!(text, "Source: {name}\n{body}").unwrap();
    }
    text
}

fn bounded_cost(
    store: &KnowledgeStore,
    query: &EvalCase,
    tokenizer: &tiktoken_rs::CoreBPE,
    stuffed_tokens: usize,
) -> (usize, usize, usize) {
    let bounded = store.search(&SearchRequest::new(&query.query)).unwrap();
    let wire = serde_json::to_string(&bounded).unwrap();
    assert!(estimate_tokens(&wire) <= 600 && wire.len() <= 8192);
    assert!(
        !bounded.hits.is_empty(),
        "token saving must not be an empty response"
    );
    assert!(
        query
            .expected_sources
            .iter()
            .any(|expected| bounded.hits.iter().any(|hit| &hit.locator == expected)),
        "bounded response lost relevant evidence for {}",
        query.id
    );
    let actual = tokenizer.encode_ordinary(&wire).len();
    assert!(
        actual * 4 < stuffed_tokens,
        "{} retrieval must save >75% corpus tokens including citations: {actual}/{stuffed_tokens}",
        query.id
    );
    (actual, bounded.estimated_tokens, wire.len())
}

#[test]
fn lexical_quality_and_reference_token_economy() {
    let corpus = corpus();
    assert!(corpus.len() >= 12);
    let store = indexed_corpus(&corpus);
    let mut queries: Vec<EvalCase> =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join("queries.json")).unwrap())
            .unwrap();
    queries.extend(
        serde_json::from_str::<Vec<EvalCase>>(
            &std::fs::read_to_string(fixtures().join("ranking-cases.json")).unwrap(),
        )
        .unwrap(),
    );
    let tokenizer = tiktoken_rs::cl100k_base().unwrap();
    let stuffed = stuffed_context(&corpus);
    let stuffed_tokens = tokenizer.encode_ordinary(&stuffed).len();
    let mut recall_sum = 0.0;
    let mut reciprocal_rank_sum = 0.0;
    let mut positives = 0_u32;
    let mut abstentions = 0_u32;
    let mut retrieval_tokens = Vec::new();
    for query in &queries {
        let response = store
            .search(&SearchRequest {
                top_k: 5,
                budget_tokens: 2000,
                max_bytes: 32768,
                ..SearchRequest::new(&query.query)
            })
            .unwrap();
        let mut seen = HashSet::new();
        let ranked_sources: Vec<&str> = response
            .hits
            .iter()
            .map(|hit| hit.locator.as_str())
            .filter(|name| seen.insert(*name))
            .collect();
        if query.expected_sources.is_empty() {
            assert!(ranked_sources.is_empty(), "{} must abstain", query.id);
            abstentions += 1;
            continue;
        }
        if let Some(expected) = &query.expected_first {
            assert_eq!(
                ranked_sources.first().copied(),
                Some(expected.as_str()),
                "{} ranked a same-vocabulary distractor above the answer",
                query.id
            );
        }
        positives += 1;
        let relevant = |name: &&str| {
            query
                .expected_sources
                .iter()
                .any(|expected| expected == *name)
        };
        let found =
            u32::try_from(ranked_sources.iter().filter(|name| relevant(name)).count()).unwrap();
        let expected = u32::try_from(query.expected_sources.len()).unwrap();
        let recall = f64::from(found) / f64::from(expected);
        let rr = ranked_sources
            .iter()
            .position(relevant)
            .map_or(0.0, |rank| {
                1.0 / f64::from(u32::try_from(rank + 1).unwrap())
            });
        recall_sum += recall;
        reciprocal_rank_sum += rr;
        assert!(
            rr > 0.0,
            "{} has no relevant source: {ranked_sources:?}",
            query.id
        );
        let (actual, estimated, bytes) = bounded_cost(&store, query, &tokenizer, stuffed_tokens);
        retrieval_tokens.push(actual);
        println!(
            "{} recall@5={recall:.3} rr={rr:.3} cl100k={actual} estimated={} bytes={}",
            query.id, estimated, bytes
        );
    }
    let recall = recall_sum / f64::from(positives);
    let mrr = reciprocal_rank_sum / f64::from(positives);
    assert!(recall >= 0.95, "recall@5 below quality gate: {recall}");
    assert!(mrr >= 0.95, "MRR below quality gate: {mrr}");
    let mean = retrieval_tokens.iter().sum::<usize>() / retrieval_tokens.len();
    println!(
        "RETRIEVAL_EVAL corpus_sources={} positive_queries={positives} abstentions={abstentions} recall@5={recall:.6} mrr={mrr:.6} stuffed_cl100k={stuffed_tokens} retrieval_cl100k_mean={mean} retrieval_cl100k_max={} stuffed_estimated={} stuffed_bytes={}",
        corpus.len(),
        retrieval_tokens.iter().max().unwrap(),
        estimate_tokens(&stuffed),
        stuffed.len()
    );
}

/// A deterministic, corpus-vocabulary fixture. It has no semantic model and
/// cannot inspect source IDs, filenames, expected answers, or query labels.
/// Unknown-only input gets a separate orthogonal coordinate, so the no-match
/// case tests real vector abstention rather than a scripted search response.
struct FixtureEmbedder {
    vocabulary: BTreeMap<String, usize>,
    documents: AtomicUsize,
    queries: AtomicUsize,
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

impl FixtureEmbedder {
    fn new(corpus: &BTreeMap<String, String>) -> Self {
        let vocabulary = corpus
            .values()
            .flat_map(|text| words(text))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, word)| (word, index))
            .collect();
        Self {
            vocabulary,
            documents: AtomicUsize::new(0),
            queries: AtomicUsize::new(0),
        }
    }

    fn dimensions(&self) -> usize {
        self.vocabulary.len() + 1
    }
}

#[async_trait]
impl EmbeddingProvider for FixtureEmbedder {
    fn id(&self) -> &'static str {
        "fixture"
    }

    fn endpoint_identity(&self) -> Result<String, ProviderError> {
        Ok(format!("local-onnx:{}", "f".repeat(64)))
    }

    fn is_remote(&self) -> bool {
        false
    }

    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        assert_eq!(request.dimensions, self.dimensions());
        match request.task {
            EmbeddingTask::Document => &self.documents,
            EmbeddingTask::Query => &self.queries,
        }
        .fetch_add(request.inputs.len(), Ordering::SeqCst);
        let vectors = request
            .inputs
            .into_iter()
            .map(|input| {
                let mut vector = vec![0.0_f32; self.dimensions()];
                for word in words(&input.text) {
                    if let Some(&index) = self.vocabulary.get(&word) {
                        vector[index] = 1.;
                    }
                }
                if vector.iter().all(|&value| value == 0.) {
                    vector[self.vocabulary.len()] = 1.;
                }
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

async fn embedded_fixture(
    corpus: &BTreeMap<String, String>,
) -> (KnowledgeEmbeddingService, Arc<FixtureEmbedder>) {
    let store = Arc::new(indexed_corpus(corpus));
    let provider = Arc::new(FixtureEmbedder::new(corpus));
    let config = EmbeddingConfig::new(
        EmbeddingProfile {
            provider_id: provider.id().into(),
            model: "fixture-vocabulary".into(),
            model_revision: Some("binary-term-v1".into()),
            endpoint_identity: provider.endpoint_identity().unwrap(),
            dimensions: provider.dimensions(),
            request_dimensions: false,
            format: EmbeddingTaskFormat::E5,
        },
        "hermetic-eval",
    );
    let service = KnowledgeEmbeddingService::new(
        store.clone(),
        provider.clone(),
        Arc::new(EmbeddingRuntime::open_in_memory().unwrap()),
        config,
    )
    .unwrap();
    let cancel = CancellationToken::new();
    for source in store.list_sources().unwrap() {
        loop {
            let progress = service
                .embed_source_batch(source.id, &cancel)
                .await
                .unwrap();
            if progress.status == EmbeddingStatus::Complete {
                break;
            }
            assert_eq!(progress.status, EmbeddingStatus::Progress);
        }
    }
    let document_inputs = provider.documents.load(Ordering::SeqCst);
    assert!(document_inputs >= corpus.len());
    // Restarting the indexing loop must reuse durable vectors, not re-embed.
    for source in store.list_sources().unwrap() {
        assert_eq!(
            service
                .embed_source_batch(source.id, &cancel)
                .await
                .unwrap()
                .status,
            EmbeddingStatus::Complete
        );
    }
    assert_eq!(provider.documents.load(Ordering::SeqCst), document_inputs);
    (service, provider)
}

#[tokio::test]
async fn deterministic_hybrid_quality_and_reference_token_economy() {
    let corpus = corpus();
    let (service, provider) = embedded_fixture(&corpus).await;
    let document_inputs = provider.documents.load(Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let queries: Vec<EvalCase> =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join("queries.json")).unwrap())
            .unwrap();
    let tokenizer = tiktoken_rs::cl100k_base().unwrap();
    let stuffed_tokens = tokenizer.encode_ordinary(&stuffed_context(&corpus)).len();
    let mut recall_sum = 0.0;
    let mut reciprocal_rank_sum = 0.0;
    let mut positives = 0_u32;
    let mut retrieval_tokens = Vec::new();
    for query in &queries {
        let response = service
            .search(
                &SearchRequest {
                    top_k: 5,
                    budget_tokens: 2000,
                    max_bytes: 32768,
                    ..SearchRequest::new(&query.query)
                },
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(response.mode, SearchMode::Hybrid);
        let mut seen = HashSet::new();
        let ranked_sources: Vec<&str> = response
            .hits
            .iter()
            .map(|hit| hit.locator.as_str())
            .filter(|name| seen.insert(*name))
            .collect();
        if query.expected_sources.is_empty() {
            assert!(ranked_sources.is_empty(), "{} must abstain", query.id);
            continue;
        }
        positives += 1;
        let relevant = |name: &&str| {
            query
                .expected_sources
                .iter()
                .any(|expected| expected == *name)
        };
        let found =
            u32::try_from(ranked_sources.iter().filter(|name| relevant(name)).count()).unwrap();
        let expected = u32::try_from(query.expected_sources.len()).unwrap();
        let recall = f64::from(found) / f64::from(expected);
        let rr = ranked_sources
            .iter()
            .position(relevant)
            .map_or(0.0, |rank| {
                1.0 / f64::from(u32::try_from(rank + 1).unwrap())
            });
        recall_sum += recall;
        reciprocal_rank_sum += rr;
        assert!(rr > 0.0, "{} lost relevant evidence", query.id);
        let bounded = service
            .search(&SearchRequest::new(&query.query), &cancel)
            .await
            .unwrap();
        let wire = serde_json::to_string(&bounded).unwrap();
        assert!(estimate_tokens(&wire) <= 600 && wire.len() <= 8192);
        assert!(
            query
                .expected_sources
                .iter()
                .any(|expected| bounded.hits.iter().any(|hit| &hit.locator == expected)),
            "bounded hybrid response lost relevant evidence for {}",
            query.id
        );
        let actual = tokenizer.encode_ordinary(&wire).len();
        assert!(actual * 4 < stuffed_tokens);
        retrieval_tokens.push(actual);
        println!(
            "hybrid {} recall@5={recall:.3} rr={rr:.3} cl100k={actual} estimated={} bytes={}",
            query.id,
            bounded.estimated_tokens,
            wire.len()
        );
    }
    let recall = recall_sum / f64::from(positives);
    let mrr = reciprocal_rank_sum / f64::from(positives);
    assert!(
        recall >= 0.90,
        "hybrid recall@5 below quality gate: {recall}"
    );
    assert!(mrr >= 0.90, "hybrid MRR below quality gate: {mrr}");
    assert_eq!(provider.queries.load(Ordering::SeqCst), queries.len());
    let mean = retrieval_tokens.iter().sum::<usize>() / retrieval_tokens.len();
    println!(
        "HYBRID_RETRIEVAL_EVAL corpus_sources={} positive_queries={positives} recall@5={recall:.6} mrr={mrr:.6} stuffed_cl100k={stuffed_tokens} retrieval_cl100k_mean={mean} retrieval_cl100k_max={} document_inputs={document_inputs} query_inputs={} dimensions={}",
        corpus.len(),
        retrieval_tokens.iter().max().unwrap(),
        provider.queries.load(Ordering::SeqCst),
        provider.dimensions(),
    );
}
