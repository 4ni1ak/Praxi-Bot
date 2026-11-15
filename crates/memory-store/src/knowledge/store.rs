//! Scope-bound catalog and atomic publication of one bounded document at a time.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    DocumentInfo, DocumentInput, IndexOutcome, KnowledgeLimits, NewSource, Scope, Source,
    SourceUpdate, chunk_text, schema,
};
use crate::{MemoryError, Result, Trust};

pub(super) const SOURCE_COLS: &str =
    "id,kind,label,locator,trust,enabled,revision,remote_embedding_json,created_ms,updated_ms";

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// One bot's local knowledge database. No embedder is required.
#[derive(Debug)]
pub struct KnowledgeStore {
    connection: Mutex<Connection>,
    scope: Scope,
    pub(super) limits: KnowledgeLimits,
}

impl KnowledgeStore {
    /// Explicitly named alias for [`Self::open`]; every index is scope-bound.
    ///
    /// # Errors
    /// The same filesystem, schema and owner-binding errors as [`Self::open`].
    pub fn open_scoped(data_dir: impl AsRef<Path>, scope: Scope) -> Result<Self> {
        Self::open(data_dir, scope)
    }

    /// Open `data_dir/bots/<bot UUID>/knowledge.db`, validating the owner binding.
    ///
    /// # Errors
    /// Filesystem, incompatible schema, corrupt database, or mismatched scope.
    pub fn open(data_dir: impl AsRef<Path>, scope: Scope) -> Result<Self> {
        let bots = data_dir.as_ref().join("bots");
        reject_symlink(&bots)?;
        let dir = bots.join(scope.bot_id.to_string());
        reject_symlink(&dir)?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("knowledge.db");
        reject_symlink(&path)?;
        let conn = Connection::open(path)?;
        configure(&conn)?;
        enable_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn, scope)
    }

    /// Open an existing, already migrated index for reads only. A missing bot
    /// directory or index returns `None`; discovery/search never creates it.
    ///
    /// # Errors
    /// Unsupported schema, corrupt binding, symlink, or SQLite/IO failure.
    pub fn open_existing(data_dir: impl AsRef<Path>, scope: Scope) -> Result<Option<Self>> {
        let bots = data_dir.as_ref().join("bots");
        reject_symlink(&bots)?;
        let dir = bots.join(scope.bot_id.to_string());
        reject_symlink(&dir)?;
        let path = dir.join("knowledge.db");
        reject_symlink(&path)?;
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure(&conn)?;
        schema::validate_read_binding(&conn, scope)?;
        Ok(Some(Self {
            connection: Mutex::new(conn),
            scope,
            limits: KnowledgeLimits::default(),
        }))
    }

    /// Whether this bot has a currently enabled source. The check reads only
    /// the scoped catalog and is used by the live tool-visibility boundary.
    ///
    /// # Errors
    /// SQLite failure.
    pub fn has_enabled_sources(&self) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM sources WHERE enabled=1)",
            [],
            |row| row.get(0),
        )?)
    }

    /// In-memory scoped index, for tests and ephemeral evaluation.
    ///
    /// # Errors
    /// SQLite initialization failure.
    pub fn open_in_memory(scope: Scope) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, scope)
    }

    fn init(mut conn: Connection, scope: Scope) -> Result<Self> {
        configure(&conn)?;
        schema::migrate(&mut conn, Some(scope))?;
        Ok(Self {
            connection: Mutex::new(conn),
            scope,
            limits: KnowledgeLimits::default(),
        })
    }

    /// Change resource limits within the supported hard ceilings.
    ///
    /// # Errors
    /// Invalid or excessive limits.
    pub fn with_limits(mut self, limits: KnowledgeLimits) -> Result<Self> {
        super::chunk::validate_limits(limits)?;
        self.limits = limits;
        Ok(self)
    }

    /// Authenticated scope recorded in the index at creation.
    #[must_use]
    pub const fn scope(&self) -> Scope {
        self.scope
    }

    /// Run bounded local publication while the approved sources stay unchanged.
    ///
    /// An IMMEDIATE transaction serializes source changes across connections.
    /// `work` must be synchronous, bounded, and must not reenter this store;
    /// perform any model/network work and document indexing outside this closure.
    ///
    /// # Errors
    /// More than 32 sources, stale/disabled sources, SQLite failure, or the error
    /// returned by the publication closure. No closure runs on a failed check.
    pub fn with_current_sources<T>(
        &self,
        expected: &[(Uuid, u64)],
        work: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        if expected.len() > 32 {
            return Err(MemoryError::Invalid(
                "too many knowledge source snapshots".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for &(id, revision) in expected {
            require_source(&tx, id, revision, true)?;
        }
        let result = work()?;
        tx.commit()?;
        Ok(result)
    }

    pub(super) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Validate a staged knowledge database without creating one.
    ///
    /// # Errors
    /// Unsupported schema, inconsistent indexes, foreign keys, or corruption.
    pub fn validate_and_migrate(path: impl AsRef<Path>) -> Result<u32> {
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&conn)?;
        // Bundled SQLite invokes FTS5's integrity check here as well. The
        // forged-inverted-index regression keeps ordinary content/schema
        // intact and proves this rejects a semantic shadow-table substitution.
        let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(MemoryError::Invalid("corrupt knowledge database".into()));
        }
        let version = schema::migrate(&mut conn, None)?;
        schema::validate_structure(&conn)?;
        conn.prepare(&format!("SELECT {SOURCE_COLS} FROM sources"))?;
        conn.prepare("SELECT id,source_id,locator,title,format,content_hash,mtime_ms,revision FROM documents")?;
        conn.prepare("SELECT rowid,id,source_id,document_id,ordinal,section,line_start,line_end,content_hash,body FROM chunks")?;
        if conn.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(MemoryError::Invalid(
                "knowledge index has invalid foreign keys".into(),
            ));
        }
        let mismatch: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks c JOIN documents d ON d.id=c.document_id WHERE c.source_id<>d.source_id)
             OR EXISTS(SELECT 1 FROM chunks c JOIN documents d ON d.id=c.document_id LEFT JOIN chunks_fts f ON f.rowid=c.rowid WHERE f.rowid IS NULL OR f.body IS NOT c.body OR f.section IS NOT c.section OR f.title IS NOT d.title)
             OR EXISTS(SELECT 1 FROM chunks_fts f LEFT JOIN chunks c ON c.rowid=f.rowid WHERE c.rowid IS NULL)",
            [], |r| r.get(0))?;
        if mismatch {
            return Err(MemoryError::Invalid(
                "knowledge index content is inconsistent".into(),
            ));
        }
        Ok(version)
    }

    /// Validate an imported database and suspend every external source/grant.
    ///
    /// # Errors
    /// Validation failure or an unexpected owner/bot binding.
    pub fn prepare_import(path: impl AsRef<Path>, scope: Scope) -> Result<()> {
        Self::validate_and_migrate(path.as_ref())?;
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&conn)?;
        schema::migrate(&mut conn, Some(scope))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE sources SET enabled=0,remote_embedding_json=NULL,revision=revision+1,updated_ms=?1", [storage::now_ms()])?;
        tx.commit()?;
        Ok(())
    }

    /// Register an explicitly chosen source, with no remote embedding consent.
    ///
    /// # Errors
    /// Invalid metadata, source quota, or SQLite failure.
    pub fn add_source(&self, new: NewSource) -> Result<Source> {
        validate_metadata(&new.label, 1024, false, "source label")?;
        validate_metadata(&new.locator, 4096, false, "source locator")?;
        let now = storage::now_ms();
        let source = Source {
            id: Uuid::now_v7(),
            kind: new.kind,
            label: new.label,
            locator: new.locator,
            trust: new.trust,
            enabled: true,
            revision: 1,
            remote_embedding: None,
            created_ms: now,
            updated_ms: now,
        };
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if count(&tx, "SELECT COUNT(*) FROM sources")? >= self.limits.sources {
            return Err(MemoryError::Invalid(
                "knowledge source quota reached".into(),
            ));
        }
        tx.execute("INSERT INTO sources(id,kind,label,locator,trust,enabled,revision,remote_embedding_json,created_ms,updated_ms) VALUES(?1,?2,?3,?4,?5,1,1,NULL,?6,?6)",
            params![source.id.to_string(), serde_json::to_string(&source.kind)?, source.label, source.locator, source.trust.as_str(), now])?;
        tx.commit()?;
        Ok(source)
    }

    /// Fetch a source in this already-scoped index.
    ///
    /// # Errors
    /// SQLite failure or corrupt metadata.
    pub fn source(&self, id: Uuid) -> Result<Option<Source>> {
        source_in(&self.conn(), id)
    }

    /// List source metadata in stable creation/id order.
    ///
    /// # Errors
    /// SQLite failure or corrupt metadata.
    pub fn list_sources(&self) -> Result<Vec<Source>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SOURCE_COLS} FROM sources ORDER BY created_ms,id LIMIT 4096"
        ))?;
        Ok(stmt
            .query_map([], source_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Apply a host-authorized change only if the source revision still matches.
    /// The caller must obtain explicit user consent before granting egress.
    ///
    /// # Errors
    /// Missing source, stale revision, invalid input, or SQLite failure.
    pub fn update_source(
        &self,
        id: Uuid,
        expected_revision: u64,
        update: SourceUpdate,
    ) -> Result<Source> {
        if let Some(label) = &update.label {
            validate_metadata(label, 1024, false, "source label")?;
        }
        if let Some(Some(consent)) = &update.remote_embedding
            && (consent.profile_fingerprint.len() != 64
                || !consent
                    .profile_fingerprint
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || consent.granted_at_ms < 0)
        {
            return Err(MemoryError::Invalid(
                "invalid embedding profile consent".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut source = require_source(&tx, id, expected_revision, false)?;
        let before = source.clone();
        if let Some(label) = update.label {
            source.label = label;
        }
        if let Some(enabled) = update.enabled {
            source.enabled = enabled;
        }
        if let Some(consent) = update.remote_embedding {
            source.remote_embedding = consent;
        }
        if source == before {
            return Ok(source);
        }
        source.revision = next_revision(source.revision)?;
        source.updated_ms = storage::now_ms();
        tx.execute("UPDATE sources SET label=?2,enabled=?3,revision=?4,remote_embedding_json=?5,updated_ms=?6 WHERE id=?1", params![id.to_string(), source.label, source.enabled, sql_int(source.revision)?, source.remote_embedding.as_ref().map(serde_json::to_string).transpose()?, source.updated_ms])?;
        tx.commit()?;
        Ok(source)
    }

    /// Permanently remove a source and all indexed content in one transaction.
    ///
    /// # Errors
    /// Missing source, stale revision, or SQLite failure.
    pub fn delete_source(&self, id: Uuid, expected_revision: u64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        require_source(&tx, id, expected_revision, false)?;
        tx.execute("DELETE FROM sources WHERE id=?1", [id.to_string()])?;
        tx.commit()?;
        Ok(())
    }

    /// Publish all chunks of one bounded document atomically. Identical text is
    /// detected by SHA-256, even when its modification timestamp is unchanged.
    ///
    /// # Errors
    /// Invalid or excessive input, stale/disabled source, quota, or SQLite error.
    pub fn index_document(
        &self,
        source_id: Uuid,
        expected_revision: u64,
        input: &DocumentInput,
    ) -> Result<IndexOutcome> {
        validate_metadata(&input.locator, 4096, false, "document locator")?;
        validate_metadata(&input.title, 1024, true, "document title")?;
        require_source(&self.conn(), source_id, expected_revision, true)?;
        let pieces = chunk_text(&input.content, input.format, self.limits)?;
        let hash = document_hash(input, self.limits.chunk_chars)?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let source = require_source(&tx, source_id, expected_revision, true)?;
        let old = document_by_locator(&tx, source_id, &input.locator)?;
        if let Some(old) = &old
            && old.content_hash == hash
            && old.mtime_ms == input.mtime_ms
        {
            return Ok(IndexOutcome {
                unchanged: true,
                document_id: old.id,
                source_revision: source.revision,
                chunks: old.chunks,
            });
        }
        if old.is_none() && count(&tx, "SELECT COUNT(*) FROM documents")? >= self.limits.documents {
            return Err(MemoryError::Invalid(
                "knowledge document quota reached".into(),
            ));
        }
        let previous_chunks = old.as_ref().map_or(0, |d| d.chunks);
        if count(&tx, "SELECT COUNT(*) FROM chunks")?
            .saturating_sub(previous_chunks)
            .saturating_add(pieces.len())
            > self.limits.chunks
        {
            return Err(MemoryError::Invalid("knowledge chunk quota reached".into()));
        }
        let id = old.as_ref().map_or_else(Uuid::now_v7, |doc| doc.id);
        let unchanged = old.as_ref().is_some_and(|d| d.content_hash == hash);
        let revision = next_revision(old.as_ref().map_or(0, |doc| doc.revision))?;
        let source_revision = next_revision(source.revision)?;
        if unchanged {
            tx.execute(
                "UPDATE documents SET mtime_ms=?2,revision=?3 WHERE id=?1",
                params![id.to_string(), input.mtime_ms, sql_int(revision)?],
            )?;
        } else {
            tx.execute("INSERT INTO documents(id,source_id,locator,title,format,content_hash,mtime_ms,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET title=excluded.title,format=excluded.format,content_hash=excluded.content_hash,mtime_ms=excluded.mtime_ms,revision=excluded.revision",
                params![id.to_string(), source_id.to_string(), input.locator, input.title, serde_json::to_string(&input.format)?, hash, input.mtime_ms, sql_int(revision)?])?;
            tx.execute("DELETE FROM chunks WHERE document_id=?1", [id.to_string()])?;
            let mut insert = tx.prepare("INSERT INTO chunks(id,source_id,document_id,ordinal,section,line_start,line_end,content_hash,body) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
            for (ordinal, piece) in pieces.iter().enumerate() {
                insert.execute(params![
                    Uuid::now_v7().to_string(),
                    source_id.to_string(),
                    id.to_string(),
                    sql_int(ordinal)?,
                    piece.section,
                    sql_int(piece.line_start)?,
                    sql_int(piece.line_end)?,
                    text_hash(&piece.content),
                    piece.content
                ])?;
            }
        }
        bump_source(&tx, source_id, source_revision)?;
        tx.commit()?;
        Ok(IndexOutcome {
            unchanged,
            document_id: id,
            source_revision,
            chunks: pieces.len(),
        })
    }

    /// Document inventory used to resume indexing or prune removed folder entries.
    ///
    /// # Errors
    /// Missing source, SQLite failure or malformed metadata.
    pub fn list_documents(&self, source_id: Uuid) -> Result<Vec<DocumentInfo>> {
        let conn = self.conn();
        if source_in(&conn, source_id)?.is_none() {
            return Err(MemoryError::NotFound("knowledge source".into()));
        }
        let mut stmt = conn.prepare(&format!(
            "{DOCUMENT_SELECT} WHERE d.source_id=?1 GROUP BY d.id ORDER BY d.locator LIMIT 100000"
        ))?;
        Ok(stmt
            .query_map([source_id.to_string()], document_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Remove one vanished document, its chunks and FTS rows atomically.
    ///
    /// # Errors
    /// Missing/foreign document, stale source revision or SQLite failure.
    pub fn delete_document(
        &self,
        source_id: Uuid,
        document_id: Uuid,
        expected_revision: u64,
    ) -> Result<u64> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let source = require_source(&tx, source_id, expected_revision, false)?;
        if tx.execute(
            "DELETE FROM documents WHERE id=?1 AND source_id=?2",
            params![document_id.to_string(), source_id.to_string()],
        )? == 0
        {
            return Err(MemoryError::NotFound("knowledge document".into()));
        }
        let revision = next_revision(source.revision)?;
        bump_source(&tx, source_id, revision)?;
        tx.commit()?;
        Ok(revision)
    }
}

fn enable_wal(conn: &Connection) -> Result<()> {
    // Concurrent first opens may return SQLITE_BUSY while promoting the
    // journal lock without invoking SQLite's busy handler. Retry that narrow
    // transition within the same connection-wide wait budget. Never retry
    // schema, binding, corruption or other SQLite failures.
    let deadline = Instant::now() + SQLITE_BUSY_TIMEOUT;
    let result = loop {
        conn.busy_timeout(deadline.saturating_duration_since(Instant::now()))?;
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Err(error)
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(
                    Duration::from_millis(5)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => break result,
        }
    };
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    result.map_err(Into::into)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -16_384)?;
    conn.pragma_update(None, "temp_store", "FILE")?;
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(MemoryError::Invalid(
            "knowledge index path must not be a symlink".into(),
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn validate_metadata(
    value: &str,
    bytes: usize,
    allow_empty: bool,
    name: &str,
) -> Result<()> {
    if value.len() > bytes
        || (!allow_empty && value.trim().is_empty())
        || value.chars().any(char::is_control)
    {
        return Err(MemoryError::Invalid(format!("invalid knowledge {name}")));
    }
    Ok(())
}

pub(super) fn source_in(conn: &Connection, id: Uuid) -> Result<Option<Source>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SOURCE_COLS} FROM sources WHERE id=?1"),
            [id.to_string()],
            source_row,
        )
        .optional()?)
}

pub(super) fn require_source(
    conn: &Connection,
    id: Uuid,
    expected_revision: u64,
    enabled: bool,
) -> Result<Source> {
    let source =
        source_in(conn, id)?.ok_or_else(|| MemoryError::NotFound("knowledge source".into()))?;
    if source.revision != expected_revision {
        return Err(MemoryError::Invalid(
            "knowledge source revision changed".into(),
        ));
    }
    if enabled && !source.enabled {
        return Err(MemoryError::Invalid("knowledge source is disabled".into()));
    }
    Ok(source)
}

fn next_revision(revision: u64) -> Result<u64> {
    revision
        .checked_add(1)
        .filter(|n| i64::try_from(*n).is_ok())
        .ok_or_else(|| MemoryError::Invalid("knowledge revision exhausted".into()))
}

fn bump_source(conn: &Connection, source_id: Uuid, revision: u64) -> Result<()> {
    conn.execute(
        "UPDATE sources SET revision=?2,updated_ms=?3 WHERE id=?1",
        params![source_id.to_string(), sql_int(revision)?, storage::now_ms()],
    )?;
    Ok(())
}

fn count(conn: &Connection, sql: &str) -> Result<usize> {
    Ok(conn.query_row(sql, [], |row| integer_at(row, 0))?)
}

pub(super) fn sql_int<T: TryInto<i64>>(value: T) -> Result<i64> {
    value
        .try_into()
        .map_err(|_| MemoryError::Invalid("knowledge integer exceeds database range".into()))
}

pub(super) fn integer_at<T: TryFrom<i64>>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    T::try_from(row.get::<_, i64>(index)?)
        .map_err(|_| conversion(index, std::io::Error::other("invalid knowledge integer")))
}

pub(super) fn uuid_at(row: &Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&row.get::<_, String>(index)?).map_err(|e| conversion(index, e))
}

fn json_at<T: DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&row.get::<_, String>(index)?).map_err(|e| conversion(index, e))
}

fn conversion(
    index: usize,
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
}

fn source_row(row: &Row<'_>) -> rusqlite::Result<Source> {
    let trust = match row.get::<_, String>(4)?.as_str() {
        "user" => Trust::User,
        "untrusted" => Trust::Untrusted,
        _ => return Err(conversion(4, std::io::Error::other("invalid trust"))),
    };
    Ok(Source {
        id: uuid_at(row, 0)?,
        kind: json_at(row, 1)?,
        label: row.get(2)?,
        locator: row.get(3)?,
        trust,
        enabled: row.get(5)?,
        revision: integer_at(row, 6)?,
        remote_embedding: row
            .get::<_, Option<String>>(7)?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| conversion(7, e))?,
        created_ms: row.get(8)?,
        updated_ms: row.get(9)?,
    })
}

const DOCUMENT_SELECT: &str = "SELECT d.id,d.source_id,d.locator,d.title,d.content_hash,d.mtime_ms,d.revision,COUNT(c.id) FROM documents d LEFT JOIN chunks c ON c.document_id=d.id";

fn document_by_locator(
    conn: &Connection,
    source_id: Uuid,
    locator: &str,
) -> Result<Option<DocumentInfo>> {
    Ok(conn
        .query_row(
            &format!("{DOCUMENT_SELECT} WHERE d.source_id=?1 AND d.locator=?2 GROUP BY d.id"),
            params![source_id.to_string(), locator],
            document_row,
        )
        .optional()?)
}

fn document_row(row: &Row<'_>) -> rusqlite::Result<DocumentInfo> {
    Ok(DocumentInfo {
        id: uuid_at(row, 0)?,
        source_id: uuid_at(row, 1)?,
        locator: row.get(2)?,
        title: row.get(3)?,
        content_hash: row.get(4)?,
        mtime_ms: row.get(5)?,
        revision: integer_at(row, 6)?,
        chunks: integer_at(row, 7)?,
    })
}

pub(super) fn text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn document_hash(input: &DocumentInput, chunk_chars: usize) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(b"praxi-knowledge-chunker-v1\0");
    let format = serde_json::to_string(&input.format)?;
    let chunk_chars = chunk_chars.to_string();
    for field in [
        format.as_str(),
        chunk_chars.as_str(),
        input.title.as_str(),
        input.content.as_str(),
    ] {
        digest.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
        digest.update(field.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}
