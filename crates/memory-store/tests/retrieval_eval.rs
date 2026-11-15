//! Offline source-quality and reference-token measurement over versioned notes.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use memory_store::estimate_tokens;
use memory_store::knowledge::{
    DocumentFormat, DocumentInput, KnowledgeStore, NewSource, Scope, SearchRequest, SourceKind,
};
use protocol::{BotId, UserId};
use serde::Deserialize;

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
