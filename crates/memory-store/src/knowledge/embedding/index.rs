use std::time::Duration;

use provider_catalog::{EmbeddingInput, EmbeddingRequest, EmbeddingTask, ModelRef, ProviderError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::embedding_runtime::Reservation;
use super::super::store::{source_in, sql_int};
use super::{
    EmbeddingProgress, EmbeddingStatus, KnowledgeEmbeddingService, cache_key, invalid, normalize,
};
use crate::{MemoryError, Result};

pub(super) const SCHEMA: &str = "
CREATE UNIQUE INDEX chunks_identity_source ON chunks(id,source_id);
CREATE TABLE knowledge_embeddings(
    chunk_id TEXT NOT NULL,source_id TEXT NOT NULL,profile TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK(dimensions BETWEEN 1 AND 8192),
    vector BLOB NOT NULL CHECK(length(vector)=dimensions*4),
    remote INTEGER NOT NULL CHECK(remote IN(0,1)),created_ms INTEGER NOT NULL,
    PRIMARY KEY(chunk_id,profile),
    FOREIGN KEY(chunk_id,source_id) REFERENCES chunks(id,source_id) ON DELETE CASCADE,
    FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE);
CREATE INDEX knowledge_embeddings_profile ON knowledge_embeddings(profile,source_id);
CREATE TABLE embedding_cache(
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,profile TEXT NOT NULL,content_key TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK(dimensions BETWEEN 1 AND 8192),
    vector BLOB NOT NULL CHECK(length(vector)=dimensions*4),
    remote INTEGER NOT NULL CHECK(remote IN(0,1)),last_used_ms INTEGER NOT NULL,
    PRIMARY KEY(source_id,profile,content_key));
CREATE TABLE embedding_jobs(
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,profile TEXT NOT NULL,
    source_revision INTEGER NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,
    not_before_ms INTEGER NOT NULL DEFAULT 0,lease TEXT,lease_until_ms INTEGER NOT NULL DEFAULT 0,
    failure_code TEXT,PRIMARY KEY(source_id,profile));
";
pub(super) const GRANT_TRIGGER:&str="
CREATE TRIGGER knowledge_embedding_grant_changed AFTER UPDATE OF remote_embedding_json,enabled ON sources
WHEN new.remote_embedding_json IS NOT old.remote_embedding_json OR new.enabled=0 BEGIN
    DELETE FROM knowledge_embeddings WHERE source_id=new.id AND remote=1;
    DELETE FROM embedding_cache WHERE source_id=new.id AND remote=1;
    DELETE FROM embedding_jobs WHERE source_id=new.id;
END
";

pub(super) fn validate_schema(conn: &Connection) -> Result<()> {
    for (kind, name, prefix) in [
        (
            "table",
            "knowledge_embeddings",
            "CREATE TABLE knowledge_embeddings(",
        ),
        ("table", "embedding_cache", "CREATE TABLE embedding_cache("),
        ("table", "embedding_jobs", "CREATE TABLE embedding_jobs("),
        (
            "index",
            "chunks_identity_source",
            "CREATE UNIQUE INDEX chunks_identity_source ",
        ),
        (
            "index",
            "knowledge_embeddings_profile",
            "CREATE INDEX knowledge_embeddings_profile ",
        ),
    ] {
        let expected = SCHEMA
            .split(';')
            .map(str::trim)
            .find(|sql| sql.starts_with(prefix))
            .ok_or_else(|| invalid("missing built-in embedding schema definition"))?;
        let actual: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type=?1 AND name=?2",
                params![kind, name],
                |row| row.get(0),
            )
            .optional()?;
        if actual.as_deref().map(str::trim) != Some(expected) {
            return Err(invalid(
                "embedding schema lacks required constraints or foreign keys",
            ));
        }
    }
    let trigger:Option<String>=conn.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='knowledge_embedding_grant_changed'",[],|row|row.get(0)).optional()?;
    if trigger.as_deref().map(str::trim) != Some(GRANT_TRIGGER.trim()) {
        return Err(invalid(
            "embedding grant-revocation maintenance is missing or changed",
        ));
    }
    let extra_trigger:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name NOT IN('chunks_insert','chunks_delete','knowledge_embedding_grant_changed'))",[],|row|row.get(0))?;
    if extra_trigger {
        return Err(invalid("knowledge index contains an unexpected trigger"));
    }
    Ok(())
}

pub(super) fn validate_data(conn: &Connection) -> Result<()> {
    validate_schema(conn)?;
    let invalid_rows:bool=conn.query_row(
        "SELECT (SELECT COUNT(*) FROM knowledge_embeddings)>250000 OR (SELECT COUNT(*) FROM embedding_cache)>100000 OR (SELECT COUNT(*) FROM embedding_jobs)>4096
         OR EXISTS(SELECT 1 FROM knowledge_embeddings WHERE dimensions NOT BETWEEN 1 AND 8192 OR length(vector)<>dimensions*4 OR remote NOT IN(0,1) OR length(profile)<>64 OR profile GLOB '*[^0-9a-f]*')
         OR EXISTS(SELECT 1 FROM embedding_cache WHERE dimensions NOT BETWEEN 1 AND 8192 OR length(vector)<>dimensions*4 OR remote NOT IN(0,1) OR length(profile)<>64 OR profile GLOB '*[^0-9a-f]*' OR length(content_key)<>64 OR content_key GLOB '*[^0-9a-f]*')
         OR EXISTS(SELECT 1 FROM embedding_jobs WHERE source_revision<1 OR attempts NOT BETWEEN 0 AND 10 OR not_before_ms<0 OR lease_until_ms<0 OR length(lease)>64 OR length(failure_code)>128 OR length(profile)<>64 OR profile GLOB '*[^0-9a-f]*')",
        [],|row|row.get(0))?;
    if invalid_rows {
        return Err(invalid(
            "embedding index contains invalid or oversized rows",
        ));
    }
    let mut statement=conn.prepare("SELECT dimensions,vector FROM knowledge_embeddings UNION ALL SELECT dimensions,vector FROM embedding_cache")?;
    let rows = statement.query_map([], |row| {
        Ok((
            super::super::store::integer_at(row, 0)?,
            row.get::<_, Vec<u8>>(1)?,
        ))
    })?;
    for row in rows {
        let (dimensions, blob) = row?;
        decode_vector(&blob, dimensions)?;
    }
    Ok(())
}

struct ChunkInput {
    id: String,
    text: String,
    title: String,
    key: String,
}
struct Batch {
    source: Uuid,
    revision: u64,
    lease: String,
    inputs: Vec<ChunkInput>,
    cached: usize,
}
enum Prepared {
    Batch(Batch),
    Done(EmbeddingProgress),
}

pub(super) fn activate_profile(conn: &mut Connection, fingerprint: &str) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active: Option<String> = tx
        .query_row(
            "SELECT value FROM knowledge_meta WHERE key='embedding_profile'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if active.as_deref() != Some(fingerprint) {
        tx.execute(
            "DELETE FROM knowledge_embeddings WHERE profile<>?1",
            [fingerprint],
        )?;
        tx.execute(
            "DELETE FROM embedding_cache WHERE profile<>?1",
            [fingerprint],
        )?;
        tx.execute(
            "DELETE FROM embedding_jobs WHERE profile<>?1",
            [fingerprint],
        )?;
        tx.execute("INSERT INTO knowledge_meta(key,value) VALUES('embedding_profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[fingerprint])?;
    }
    tx.commit()?;
    Ok(())
}

impl KnowledgeEmbeddingService {
    /// Process one bounded, resumable source batch. The host schedules the next
    /// invocation at `next_attempt_ms`; this method never sleeps through backoff.
    ///
    /// # Errors
    /// Invalid/missing source, corrupt state, or SQLite errors. Provider failures
    /// are sanitized durable progress states, leaving lexical retrieval usable.
    // Keep one bounded batch admission/publication state machine together.
    #[allow(clippy::too_many_lines)]
    pub async fn embed_source_batch(
        &self,
        source_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<EmbeddingProgress> {
        if cancel.is_cancelled() {
            return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
        }
        let batch = match self.prepare(source_id, storage::now_ms())? {
            Prepared::Done(progress) => return Ok(progress),
            Prepared::Batch(batch) => batch,
        };
        if cancel.is_cancelled() {
            self.release(&batch)?;
            return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
        }
        let token_units = batch
            .inputs
            .iter()
            .map(|input| {
                u64::try_from(input.text.len() + input.title.len() + 96).unwrap_or(u64::MAX)
            })
            .sum();
        if self.provider.is_remote() {
            match self.runtime.reserve(
                &self.config.account_group,
                &self.config.budget,
                batch.inputs.len(),
                token_units,
                storage::now_ms(),
            ) {
                Ok(Reservation::Granted) => {}
                Ok(Reservation::Deferred(deadline)) => {
                    return self.defer_batch(&batch, deadline, "local_budget");
                }
                Err(MemoryError::Invalid(_)) => {
                    return self.fail_batch(&batch, "input_budget", false, None);
                }
                Err(error) => {
                    self.release(&batch)?;
                    return Err(error);
                }
            }
        }
        // This transaction is the request's admission point. It checks consent,
        // source revision, active generation and the lease immediately before
        // polling the HTTP future. Later revocation cannot retract dispatched
        // bytes, but always prevents the stale result from being published.
        if cancel.is_cancelled() || !self.admit(&batch, storage::now_ms())? {
            self.release(&batch)?;
            return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
        }
        let request = EmbeddingRequest {
            model: ModelRef(format!(
                "{}/{}",
                self.config.profile.provider_id, self.config.profile.model
            )),
            inputs: batch
                .inputs
                .iter()
                .map(|input| EmbeddingInput {
                    text: input.text.clone(),
                    title: Some(input.title.clone()),
                })
                .collect(),
            dimensions: self.config.profile.dimensions,
            request_dimensions: self.config.profile.request_dimensions,
            task: EmbeddingTask::Document,
            format: self.config.profile.format,
        };
        let result = tokio::select! {
            biased;
            ()=cancel.cancelled()=>{self.release(&batch)?;return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));},
            result=tokio::time::timeout(Duration::from_millis(self.config.index_timeout_ms),self.provider.embed(request))=>{
                result.unwrap_or(Err(ProviderError::Timeout))
            }
        };
        match result {
            Ok(response) => {
                if response.vectors.len() != batch.inputs.len() {
                    return self.fail_batch(&batch, "response_count", false, None);
                }
                let vectors = response
                    .vectors
                    .into_iter()
                    .map(|vector| normalize(vector, self.config.profile.dimensions))
                    .collect::<Result<Vec<_>>>();
                let Ok(vectors) = vectors else {
                    return self.fail_batch(&batch, "response_vector", false, None);
                };
                if cancel.is_cancelled() {
                    self.release(&batch)?;
                    return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
                }
                self.publish(&batch, &vectors, storage::now_ms())
            }
            Err(error) => {
                let (code, retry, wait) = match error {
                    ProviderError::RateLimited { retry_after } => {
                        ("provider_rate_limit", true, retry_after)
                    }
                    ProviderError::Upstream { .. } => ("provider_unavailable", true, None),
                    ProviderError::Timeout => ("provider_timeout", true, None),
                    ProviderError::Connection(_) => ("provider_connection", true, None),
                    ProviderError::Auth(_) => ("provider_authentication", false, None),
                    _ => ("provider_invalid_response", false, None),
                };
                if code == "provider_rate_limit" {
                    let delay = wait.unwrap_or(Duration::from_secs(60));
                    self.runtime.defer(
                        &self.config.account_group,
                        storage::now_ms()
                            .saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX)),
                    )?;
                }
                self.fail_batch(&batch, code, retry, wait)
            }
        }
    }

    /// Reset a failed source's retry state after an explicit host/user request.
    /// Quota reservations are intentionally preserved.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn reset_source_retry(&self, source_id: Uuid) -> Result<()> {
        self.store.conn().execute(
            "DELETE FROM embedding_jobs WHERE source_id=?1 AND profile=?2",
            params![source_id.to_string(), self.fingerprint],
        )?;
        Ok(())
    }

    // Every cache restore and lease claim belongs to this one transaction.
    #[allow(clippy::too_many_lines)]
    fn prepare(&self, source_id: Uuid, now: i64) -> Result<Prepared> {
        let mut connection = self.store.conn();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source = source_in(&tx, source_id)?
            .ok_or_else(|| MemoryError::NotFound("knowledge source".into()))?;
        if !source.enabled
            || !self.current_profile(&tx)?
            || (self.provider.is_remote()
                && source
                    .remote_embedding
                    .as_ref()
                    .is_none_or(|grant| grant.profile_fingerprint != self.fingerprint))
        {
            return Ok(Prepared::Done(EmbeddingProgress::status(
                EmbeddingStatus::AwaitingConsent,
            )));
        }
        let job:Option<(i64,u32,i64,i64,Option<String>)>=tx.query_row(
            "SELECT source_revision,attempts,not_before_ms,lease_until_ms,failure_code FROM embedding_jobs WHERE source_id=?1 AND profile=?2",
            params![source_id.to_string(),self.fingerprint],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
        if let Some((revision, attempts, next, lease_until, failure)) = job
            && u64::try_from(revision).ok() == Some(source.revision)
        {
            if attempts >= self.config.max_attempts {
                let mut progress = EmbeddingProgress::status(EmbeddingStatus::Failed);
                progress.failure_code = failure;
                return Ok(Prepared::Done(progress));
            }
            if next.max(lease_until) > now {
                let mut progress = EmbeddingProgress::status(EmbeddingStatus::Deferred);
                progress.next_attempt_ms = Some(next.max(lease_until));
                return Ok(Prepared::Done(progress));
            }
        }
        let limit = if self.provider.is_remote()
            && self.config.budget.request_unit == super::RequestUnit::Input
        {
            self.config
                .max_batch_inputs
                .min(self.config.budget.requests_per_minute as usize)
                .min(self.config.budget.requests_per_day as usize)
        } else {
            self.config.max_batch_inputs
        };
        let mut rows = Vec::new();
        {
            let mut statement=tx.prepare(
                "SELECT c.id,c.body,d.title,c.section FROM chunks c JOIN documents d ON d.id=c.document_id
                 LEFT JOIN knowledge_embeddings e ON e.chunk_id=c.id AND e.profile=?2
                 WHERE c.source_id=?1 AND e.chunk_id IS NULL ORDER BY d.locator,c.ordinal,c.id LIMIT ?3")?;
            let inputs = statement.query_map(
                params![source_id.to_string(), self.fingerprint, sql_int(limit)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            for input in inputs {
                let (id, text, title, section) = input?;
                let title = format!("{title} / {section}")
                    .chars()
                    .take(512)
                    .collect::<String>();
                let key = cache_key(EmbeddingTask::Document, &text, Some(&title));
                rows.push(ChunkInput {
                    id,
                    text,
                    title,
                    key,
                });
            }
        }
        let mut pending = Vec::new();
        let mut cached = 0;
        for input in rows {
            let cached_blob:Option<Vec<u8>>=tx.query_row("SELECT vector FROM embedding_cache WHERE source_id=?1 AND profile=?2 AND content_key=?3 AND dimensions=?4",
                params![source_id.to_string(),self.fingerprint,input.key,sql_int(self.config.profile.dimensions)?],|row|row.get(0)).optional()?;
            if let Some(blob) = cached_blob
                && decode_vector(&blob, self.config.profile.dimensions).is_ok()
            {
                self.insert_vector(&tx, source_id, &input.id, &blob, now)?;
                tx.execute("UPDATE embedding_cache SET last_used_ms=?4 WHERE source_id=?1 AND profile=?2 AND content_key=?3",params![source_id.to_string(),self.fingerprint,input.key,now])?;
                cached += 1;
            } else {
                pending.push(input);
            }
        }
        if pending.is_empty() {
            let remaining = self.remaining(&tx, source_id)?;
            tx.execute(
                "DELETE FROM embedding_jobs WHERE source_id=?1 AND profile=?2",
                params![source_id.to_string(), self.fingerprint],
            )?;
            tx.commit()?;
            return Ok(Prepared::Done(EmbeddingProgress {
                status: if remaining == 0 {
                    EmbeddingStatus::Complete
                } else {
                    EmbeddingStatus::Progress
                },
                embedded_chunks: 0,
                cache_hits: cached,
                remaining_chunks: remaining,
                next_attempt_ms: None,
                failure_code: None,
            }));
        }
        // A per-minute token cap is also the largest useful single attempt.
        // Smaller fitting batches make progress; one oversized input remains a
        // clear terminal input_budget failure instead of silently truncating it.
        let wire_cap = provider_catalog::embedding::MAX_EMBEDDING_BATCH_BYTES as u64;
        let token_cap = if self.provider.is_remote() {
            self.config
                .budget
                .tokens_per_minute
                .unwrap_or(wire_cap)
                .min(wire_cap)
        } else {
            wire_cap
        };
        let mut used = 0_u64;
        let accepted = pending
            .iter()
            .take_while(|input| {
                let cost =
                    u64::try_from(input.text.len() + input.title.len() + 96).unwrap_or(u64::MAX);
                if used.saturating_add(cost) > token_cap {
                    false
                } else {
                    used += cost;
                    true
                }
            })
            .count();
        pending.truncate(accepted.max(1));
        let lease = Uuid::now_v7().to_string();
        tx.execute("INSERT INTO embedding_jobs(source_id,profile,source_revision,lease,lease_until_ms) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(source_id,profile) DO UPDATE SET attempts=CASE WHEN source_revision=excluded.source_revision THEN attempts ELSE 0 END,
            source_revision=excluded.source_revision,lease=excluded.lease,lease_until_ms=excluded.lease_until_ms,not_before_ms=0,failure_code=NULL",
            params![source_id.to_string(),self.fingerprint,sql_revision(source.revision)?,lease,now.saturating_add(i64::try_from(self.config.index_timeout_ms).unwrap_or(i64::MAX)).saturating_add(5000)])?;
        tx.commit()?;
        Ok(Prepared::Batch(Batch {
            source: source_id,
            revision: source.revision,
            lease,
            inputs: pending,
            cached,
        }))
    }

    fn current_profile(&self, conn: &Connection) -> Result<bool> {
        let active: Option<String> = conn
            .query_row(
                "SELECT value FROM knowledge_meta WHERE key='embedding_profile'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(active.as_deref() == Some(&self.fingerprint))
    }

    fn current_batch(&self, conn: &Connection, batch: &Batch) -> Result<bool> {
        let Some(source) = source_in(conn, batch.source)? else {
            return Ok(false);
        };
        if !source.enabled
            || source.revision != batch.revision
            || !self.current_profile(conn)?
            || (self.provider.is_remote()
                && source
                    .remote_embedding
                    .as_ref()
                    .is_none_or(|grant| grant.profile_fingerprint != self.fingerprint))
        {
            return Ok(false);
        }
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM embedding_jobs WHERE source_id=?1 AND profile=?2 AND lease=?3 AND source_revision=?4)",
            params![batch.source.to_string(),self.fingerprint,batch.lease,sql_revision(batch.revision)?],|row|row.get(0))?)
    }

    fn admit(&self, batch: &Batch, now: i64) -> Result<bool> {
        let mut connection = self.store.conn();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !self.current_batch(&tx, batch)? {
            return Ok(false);
        }
        let changed=tx.execute("UPDATE embedding_jobs SET attempts=attempts+1 WHERE source_id=?1 AND profile=?2 AND lease=?3 AND lease_until_ms>?4",
            params![batch.source.to_string(),self.fingerprint,batch.lease,now])?;
        tx.commit()?;
        Ok(changed == 1)
    }

    fn release(&self, batch: &Batch) -> Result<()> {
        self.store.conn().execute("UPDATE embedding_jobs SET lease=NULL,lease_until_ms=0 WHERE source_id=?1 AND profile=?2 AND lease=?3",
            params![batch.source.to_string(),self.fingerprint,batch.lease])?;
        Ok(())
    }

    fn remaining(&self, conn: &Connection, source: Uuid) -> Result<usize> {
        let count:i64=conn.query_row("SELECT COUNT(*) FROM chunks c LEFT JOIN knowledge_embeddings e ON e.chunk_id=c.id AND e.profile=?2 WHERE c.source_id=?1 AND e.chunk_id IS NULL",
            params![source.to_string(),self.fingerprint],|row|row.get(0))?;
        usize::try_from(count).map_err(|_| invalid("embedding count overflow"))
    }

    fn insert_vector(
        &self,
        conn: &Connection,
        source: Uuid,
        chunk: &str,
        blob: &[u8],
        now: i64,
    ) -> Result<()> {
        conn.execute("INSERT INTO knowledge_embeddings(chunk_id,source_id,profile,dimensions,vector,remote,created_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(chunk_id,profile) DO UPDATE SET vector=excluded.vector,dimensions=excluded.dimensions,remote=excluded.remote,created_ms=excluded.created_ms",
            params![chunk,source.to_string(),self.fingerprint,sql_int(self.config.profile.dimensions)?,blob,self.provider.is_remote(),now])?;
        Ok(())
    }

    fn publish(&self, batch: &Batch, vectors: &[Vec<f32>], now: i64) -> Result<EmbeddingProgress> {
        let mut connection = self.store.conn();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !self.current_batch(&tx, batch)? {
            return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
        }
        for (input, vector) in batch.inputs.iter().zip(vectors) {
            let blob = encode_vector(vector);
            self.insert_vector(&tx, batch.source, &input.id, &blob, now)?;
            tx.execute("INSERT INTO embedding_cache(source_id,profile,content_key,dimensions,vector,remote,last_used_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)
                ON CONFLICT(source_id,profile,content_key) DO UPDATE SET vector=excluded.vector,dimensions=excluded.dimensions,remote=excluded.remote,last_used_ms=excluded.last_used_ms",
                params![batch.source.to_string(),self.fingerprint,input.key,sql_int(self.config.profile.dimensions)?,blob,self.provider.is_remote(),now])?;
        }
        tx.execute("DELETE FROM embedding_cache WHERE rowid IN (SELECT rowid FROM embedding_cache ORDER BY last_used_ms,rowid LIMIT MAX(0,(SELECT COUNT(*) FROM embedding_cache)-?1))",[sql_int(self.config.max_cache_entries)?])?;
        tx.execute(
            "DELETE FROM embedding_jobs WHERE source_id=?1 AND profile=?2 AND lease=?3",
            params![batch.source.to_string(), self.fingerprint, batch.lease],
        )?;
        let remaining = self.remaining(&tx, batch.source)?;
        tx.commit()?;
        Ok(EmbeddingProgress {
            status: if remaining == 0 {
                EmbeddingStatus::Complete
            } else {
                EmbeddingStatus::Progress
            },
            embedded_chunks: batch.inputs.len(),
            cache_hits: batch.cached,
            remaining_chunks: remaining,
            next_attempt_ms: None,
            failure_code: None,
        })
    }

    fn defer_batch(&self, batch: &Batch, deadline: i64, code: &str) -> Result<EmbeddingProgress> {
        let conn = self.store.conn();
        conn.execute("UPDATE embedding_jobs SET lease=NULL,lease_until_ms=0,not_before_ms=?4,failure_code=?5 WHERE source_id=?1 AND profile=?2 AND lease=?3",
            params![batch.source.to_string(),self.fingerprint,batch.lease,deadline,code])?;
        Ok(EmbeddingProgress {
            status: EmbeddingStatus::Deferred,
            embedded_chunks: 0,
            cache_hits: batch.cached,
            remaining_chunks: self.remaining(&conn, batch.source)?,
            next_attempt_ms: Some(deadline),
            failure_code: Some(code.into()),
        })
    }

    fn fail_batch(
        &self,
        batch: &Batch,
        code: &str,
        retry: bool,
        wait: Option<Duration>,
    ) -> Result<EmbeddingProgress> {
        let conn = self.store.conn();
        let attempts:Option<u32>=conn.query_row("SELECT attempts FROM embedding_jobs WHERE source_id=?1 AND profile=?2 AND lease=?3",
            params![batch.source.to_string(),self.fingerprint,batch.lease],|row|row.get(0)).optional()?;
        let Some(attempts) = attempts else {
            return Ok(EmbeddingProgress::status(EmbeddingStatus::Cancelled));
        };
        if retry && attempts < self.config.max_attempts {
            let exponential = 500_u64
                .saturating_mul(2_u64.saturating_pow(attempts))
                .min(300_000);
            // Stable per-lease jitter; no global random state or sleeps required.
            let jitter = batch.lease.bytes().fold(0u64, |value, byte| {
                value.wrapping_mul(31).wrapping_add(u64::from(byte))
            }) % (exponential / 2 + 1);
            let delay = wait.map_or(exponential / 2 + jitter, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
            let deadline =
                storage::now_ms().saturating_add(i64::try_from(delay).unwrap_or(i64::MAX));
            drop(conn);
            return self.defer_batch(batch, deadline, code);
        }
        conn.execute("UPDATE embedding_jobs SET attempts=?4,lease=NULL,lease_until_ms=0,failure_code=?5 WHERE source_id=?1 AND profile=?2 AND lease=?3",
            params![batch.source.to_string(),self.fingerprint,batch.lease,self.config.max_attempts,code])?;
        Ok(EmbeddingProgress {
            status: EmbeddingStatus::Failed,
            embedded_chunks: 0,
            cache_hits: batch.cached,
            remaining_chunks: self.remaining(&conn, batch.source)?,
            next_attempt_ms: None,
            failure_code: Some(code.into()),
        })
    }
}

pub(super) fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub(super) fn decode_vector(blob: &[u8], dimensions: usize) -> Result<Vec<f32>> {
    if blob.len() != dimensions.saturating_mul(4) {
        return Err(invalid("stored embedding vector size mismatch"));
    }
    let vector = blob
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(invalid("stored embedding vector is not finite"));
    }
    normalize(vector, dimensions)
}

fn sql_revision(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| invalid("embedding source revision overflow"))
}
