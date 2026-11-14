use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::time::Duration;

use provider_catalog::{EmbeddingInput, EmbeddingRequest, EmbeddingTask, ModelRef, ProviderError};
use rusqlite::params;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::embedding_runtime::Reservation;
use super::super::search::validate_request;
use super::super::store::{integer_at, uuid_at};
use super::super::{SearchHit, SearchMode, SearchRequest, SearchResponse};
use super::index::decode_vector;
use super::{KnowledgeEmbeddingService, QueryCacheEntry, cache_key, invalid, normalize};
use crate::{MemoryError, Result, Trust, rrf_fuse};

const CANDIDATES: usize = 256;

struct Scored {
    id: String,
    score: f64,
}
impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl KnowledgeEmbeddingService {
    /// Search locally, adding semantic results only when their query embedding
    /// is authorized and available. Provider errors never disable lexical search.
    ///
    /// # Errors
    /// Invalid query, cancellation, corrupt index, or SQLite failure.
    pub async fn search(
        &self,
        request: &SearchRequest,
        cancel: &CancellationToken,
    ) -> Result<SearchResponse> {
        let search_cancel = cancel.child_token();
        let _cancel_on_drop = search_cancel.clone().drop_guard();
        let cancel = &search_cancel;
        validate_request(request)?;
        if cancel.is_cancelled() {
            return Err(MemoryError::Embed("knowledge search cancelled".into()));
        }
        if request.query.trim().is_empty() || (self.provider.is_remote() && !self.may_egress()) {
            return self
                .pack_lexical(request, SearchMode::Lexical, cancel)
                .await;
        }
        let count = self.vector_count(request)?;
        if count == 0 {
            return self
                .pack_lexical(request, SearchMode::Lexical, cancel)
                .await;
        }
        if count > self.config.max_scan_vectors {
            return self
                .pack_lexical(request, SearchMode::LexicalDegraded, cancel)
                .await;
        }
        let Some(query) = self.query_vector(&request.query, cancel).await? else {
            return self
                .pack_lexical(request, SearchMode::LexicalDegraded, cancel)
                .await;
        };
        if cancel.is_cancelled() {
            return Err(MemoryError::Embed("knowledge search cancelled".into()));
        }
        // Re-read current source state after any network wait; a deleted source's
        // earlier lexical snapshot must not be returned by a late response.
        let lexical = self.store.lexical_candidates(request)?;
        let store = self.store.clone();
        let fingerprint = self.fingerprint.clone();
        let vector_request = request.clone();
        let scan_cancel = cancel.clone();
        let max_scan = self.config.max_scan_vectors;
        let timeout = Duration::from_millis(self.config.query_timeout_ms);
        let scanned = tokio::task::spawn_blocking(move || {
            semantic_candidates(
                &store,
                &fingerprint,
                &vector_request,
                &query,
                max_scan,
                timeout,
                &scan_cancel,
            )
        })
        .await
        .map_err(|_| invalid("semantic search worker failed"))?;
        let semantic = match scanned {
            Ok(Some(candidates)) => candidates,
            Ok(None) => {
                return self
                    .pack_with_rerank(lexical, request, SearchMode::LexicalDegraded, cancel)
                    .await;
            }
            Err(error) => return Err(error),
        };
        let lexical_ids = lexical.iter().map(|hit| hit.id).collect();
        let semantic_ids = semantic.iter().map(|hit| hit.id).collect();
        let ranks = rrf_fuse(&[lexical_ids, semantic_ids], 60.);
        let mut hits: HashMap<Uuid, SearchHit> = lexical
            .into_iter()
            .chain(semantic)
            .map(|hit| (hit.id, hit))
            .collect();
        let merged = ranks
            .into_iter()
            .filter_map(|(id, score)| {
                hits.remove(&id).map(|mut hit| {
                    hit.score = score;
                    hit
                })
            })
            .collect();
        self.pack_with_rerank(merged, request, SearchMode::Hybrid, cancel)
            .await
    }

    async fn pack_lexical(
        &self,
        request: &SearchRequest,
        mode: SearchMode,
        cancel: &CancellationToken,
    ) -> Result<SearchResponse> {
        self.pack_with_rerank(
            self.store.lexical_candidates(request)?,
            request,
            mode,
            cancel,
        )
        .await
    }

    async fn query_vector(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<f32>>> {
        let key = cache_key(EmbeddingTask::Query, query, None);
        let now = storage::now_ms();
        {
            let mut cache = self
                .query_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.retain(|entry| entry.expires_ms > now);
            if let Some(entry) = cache.iter().find(|entry| entry.key == key) {
                return Ok(Some(entry.vector.clone()));
            }
        }
        if self.provider.is_remote()
            && !matches!(
                self.runtime.reserve(
                    &self.config.account_group,
                    &self.config.budget,
                    1,
                    u64::try_from(query.len() + 96).unwrap_or(u64::MAX),
                    now
                ),
                Ok(Reservation::Granted)
            )
        {
            return Ok(None);
        }
        let request = EmbeddingRequest {
            model: ModelRef(format!(
                "{}/{}",
                self.config.profile.provider_id, self.config.profile.model
            )),
            inputs: vec![EmbeddingInput {
                text: query.into(),
                title: None,
            }],
            dimensions: self.config.profile.dimensions,
            request_dimensions: self.config.profile.request_dimensions,
            task: EmbeddingTask::Query,
            format: self.config.profile.format,
        };
        let response = tokio::select! {
            biased;
            ()=cancel.cancelled()=>return Err(MemoryError::Embed("knowledge search cancelled".into())),
            result=tokio::time::timeout(Duration::from_millis(self.config.query_timeout_ms),self.provider.embed(request))=>result,
        };
        let response = match response {
            Ok(Ok(response)) => response,
            Ok(Err(ProviderError::RateLimited { retry_after })) => {
                let duration = retry_after.unwrap_or(Duration::from_secs(60));
                self.runtime.defer(
                    &self.config.account_group,
                    storage::now_ms()
                        .saturating_add(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)),
                )?;
                return Ok(None);
            }
            _ => return Ok(None),
        };
        if response.vectors.len() != 1 {
            return Ok(None);
        }
        let Some(vector) = response.vectors.into_iter().next() else {
            return Ok(None);
        };
        let Ok(vector) = normalize(vector, self.config.profile.dimensions) else {
            return Ok(None);
        };
        if cancel.is_cancelled() {
            return Err(MemoryError::Embed("knowledge search cancelled".into()));
        }
        let mut cache = self
            .query_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while cache.len() >= 64 {
            cache.pop_front();
        }
        cache.push_back(QueryCacheEntry {
            key,
            expires_ms: now.saturating_add(300_000),
            vector: vector.clone(),
        });
        Ok(Some(vector))
    }

    fn vector_count(&self, request: &SearchRequest) -> Result<usize> {
        let sources = serde_json::to_string(&request.source_ids)?;
        let count:i64=self.store.conn().query_row(
            "SELECT COUNT(*) FROM knowledge_embeddings e JOIN sources s ON s.id=e.source_id
             WHERE e.profile=?1 AND s.enabled=1 AND (?2='[]' OR e.source_id IN(SELECT value FROM json_each(?2)))
             AND (e.remote=0 OR json_extract(s.remote_embedding_json,'$.profile_fingerprint')=?1)
             AND EXISTS(SELECT 1 FROM knowledge_meta WHERE key='embedding_profile' AND value=?1)",
            params![self.fingerprint,sources],|row|row.get(0))?;
        usize::try_from(count).map_err(|_| invalid("embedding count overflow"))
    }
}

fn semantic_candidates(
    store: &super::super::KnowledgeStore,
    fingerprint: &str,
    request: &SearchRequest,
    query: &[f32],
    max_scan_vectors: usize,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Option<Vec<SearchHit>>> {
    let sources = serde_json::to_string(&request.source_ids)?;
    let conn = store.conn();
    let started = std::time::Instant::now();
    let mut heap = BinaryHeap::<Reverse<Scored>>::new();
    {
        let mut statement=conn.prepare(
                "SELECT e.chunk_id,e.vector,e.dimensions FROM knowledge_embeddings e JOIN sources s ON s.id=e.source_id
                 WHERE e.profile=?1 AND s.enabled=1 AND (?2='[]' OR e.source_id IN(SELECT value FROM json_each(?2)))
                 AND (e.remote=0 OR json_extract(s.remote_embedding_json,'$.profile_fingerprint')=?1)
                 AND EXISTS(SELECT 1 FROM knowledge_meta WHERE key='embedding_profile' AND value=?1) ORDER BY e.chunk_id")?;
        let rows = statement.query_map(params![fingerprint, sources], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                integer_at(row, 2)?,
            ))
        })?;
        for (index, row) in rows.enumerate() {
            if cancel.is_cancelled() {
                return Err(MemoryError::Embed("knowledge search cancelled".into()));
            }
            if index >= max_scan_vectors || started.elapsed() > timeout {
                return Ok(None);
            }
            let (id, blob, dimensions) = row?;
            if dimensions != query.len() {
                return Ok(None);
            }
            let Ok(vector) = decode_vector(&blob, dimensions) else {
                return Ok(None);
            };
            let similarity = query
                .iter()
                .zip(vector)
                .map(|(&left, right)| f64::from(left) * f64::from(right))
                .sum::<f64>();
            // Zero/negative matches contribute no useful semantic evidence.
            if !similarity.is_finite() || similarity <= 0. {
                continue;
            }
            heap.push(Reverse(Scored {
                id,
                score: similarity,
            }));
            if heap.len() > CANDIDATES {
                heap.pop();
            }
        }
    }
    let mut ranks = heap
        .into_iter()
        .map(|Reverse(score)| score)
        .collect::<Vec<_>>();
    ranks.sort_by(|left, right| right.cmp(left));
    let mut statement=conn.prepare("SELECT c.id,c.source_id,c.document_id,s.label,d.locator,c.section,c.line_start,c.line_end,s.trust,c.body
            FROM chunks c JOIN sources s ON s.id=c.source_id JOIN documents d ON d.id=c.document_id WHERE c.id=?1 AND s.enabled=1")?;
    let mut hits = Vec::with_capacity(ranks.len());
    for rank in ranks {
        let hit = statement.query_row([rank.id], |row| {
            Ok(SearchHit {
                id: uuid_at(row, 0)?,
                source_id: uuid_at(row, 1)?,
                document_id: uuid_at(row, 2)?,
                source_label: row.get(3)?,
                locator: row.get(4)?,
                section: row.get(5)?,
                line_start: integer_at(row, 6)?,
                line_end: integer_at(row, 7)?,
                source_trust: if row.get::<_, String>(8)? == "user" {
                    Trust::User
                } else {
                    Trust::Untrusted
                },
                content: row.get(9)?,
                score: rank.score,
            })
        })?;
        hits.push(hit);
    }
    Ok(Some(hits))
}
