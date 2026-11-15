//! Separate, scope-bound knowledge database migrations.

use protocol::{BotId, UserId};
use rusqlite::{Connection, OptionalExtension, params};

use super::Scope;
use crate::{MemoryError, Result};

pub(super) const VERSION: u32 = 1;

const INITIAL: &str = r"
CREATE TABLE knowledge_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT NOT NULL CHECK(length(CAST(label AS BLOB)) BETWEEN 1 AND 1024),
    locator TEXT NOT NULL CHECK(length(CAST(locator AS BLOB)) BETWEEN 1 AND 4096),
    trust TEXT NOT NULL CHECK(trust IN ('user','untrusted')),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision > 0),
    remote_embedding_json TEXT CHECK(remote_embedding_json IS NULL OR length(CAST(remote_embedding_json AS BLOB))<=1024),
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE TABLE documents (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    locator TEXT NOT NULL CHECK(length(CAST(locator AS BLOB)) BETWEEN 1 AND 4096),
    title TEXT NOT NULL CHECK(length(CAST(title AS BLOB)) <= 1024),
    format TEXT NOT NULL,
    content_hash TEXT NOT NULL CHECK(length(content_hash)=64),
    mtime_ms INTEGER,
    revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision > 0),
    UNIQUE(source_id, locator)
);
CREATE TABLE chunks (
    rowid INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    section TEXT NOT NULL CHECK(length(CAST(section AS BLOB))<=8192),
    line_start INTEGER NOT NULL CHECK(line_start > 0),
    line_end INTEGER NOT NULL CHECK(line_end >= line_start),
    content_hash TEXT NOT NULL CHECK(length(content_hash)=64),
    body TEXT NOT NULL CHECK(length(CAST(body AS BLOB))<=32000),
    UNIQUE(document_id, ordinal)
);
CREATE INDEX chunks_source ON chunks(source_id, id);
CREATE VIRTUAL TABLE chunks_fts USING fts5(title, section, body, tokenize='unicode61 remove_diacritics 2');
";

const INSERT_TRIGGER: &str = "CREATE TRIGGER chunks_insert AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, title, section, body)
    VALUES (new.rowid, (SELECT title FROM documents WHERE id=new.document_id), new.section, new.body);
END";
const DELETE_TRIGGER: &str = "CREATE TRIGGER chunks_delete AFTER DELETE ON chunks BEGIN
    DELETE FROM chunks_fts WHERE rowid=old.rowid;
END";

pub(super) fn migrate(conn: &mut Connection, expected: Option<Scope>) -> Result<u32> {
    // Acquire the writer reservation before observing the version. Another
    // process may have completed the same migration while this one waited.
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let found: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > VERSION {
        return Err(MemoryError::SchemaTooNew {
            found,
            supported: VERSION,
        });
    }
    if found > 0 {
        let actual = read_scope(&tx)?;
        if expected.is_some_and(|scope| scope != actual) {
            return Err(MemoryError::Invalid(
                "knowledge index scope mismatch".into(),
            ));
        }
    } else {
        let scope =
            expected.ok_or_else(|| MemoryError::Invalid("knowledge index has no scope".into()))?;
        tx.execute_batch(INITIAL)?;
        tx.execute_batch(INSERT_TRIGGER)?;
        tx.execute_batch(DELETE_TRIGGER)?;
        tx.execute(
            "INSERT INTO knowledge_meta(key,value) VALUES ('user_id',?1),('bot_id',?2)",
            params![scope.user_id.to_string(), scope.bot_id.to_string()],
        )?;
        tx.pragma_update(None, "user_version", VERSION)?;
    }
    tx.commit()?;
    Ok(VERSION)
}

pub(super) fn validate_read_binding(conn: &Connection, expected: Scope) -> Result<()> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > VERSION {
        return Err(MemoryError::SchemaTooNew {
            found: version,
            supported: VERSION,
        });
    }
    if version != VERSION {
        return Err(MemoryError::Invalid(
            "knowledge index requires a writable migration".into(),
        ));
    }
    if read_scope(conn)? != expected {
        return Err(MemoryError::Invalid(
            "knowledge index scope mismatch".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_structure(conn: &Connection) -> Result<()> {
    validate_schema(conn)?;
    let oversized: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sources WHERE length(CAST(label AS BLOB))>1024 OR length(CAST(locator AS BLOB))>4096 OR length(CAST(remote_embedding_json AS BLOB))>1024)
         OR EXISTS(SELECT 1 FROM documents WHERE length(CAST(title AS BLOB))>1024 OR length(CAST(locator AS BLOB))>4096 OR length(content_hash)<>64)
         OR EXISTS(SELECT 1 FROM chunks WHERE length(CAST(body AS BLOB))>32000 OR length(CAST(section AS BLOB))>8192 OR line_start<1 OR line_end<line_start OR length(content_hash)<>64)
         OR (SELECT COUNT(*) FROM sources)>4096 OR (SELECT COUNT(*) FROM documents)>100000 OR (SELECT COUNT(*) FROM chunks)>250000",
        [], |r| r.get(0))?;
    if oversized {
        return Err(MemoryError::Invalid(
            "knowledge index exceeds resource bounds".into(),
        ));
    }
    Ok(())
}

fn validate_schema(conn: &Connection) -> Result<()> {
    // Checking required triggers alone admits extra triggers that can undo a
    // consent reset or silently ignore deletion. Foreign-key checks likewise
    // only check the relationships a database chose to declare. Compare every
    // schema object with our own migrations, including FTS shadow tables and
    // indexes. Root pages and user data are deliberately not part of the match.
    // Reusing migrations also includes future schema versions automatically.
    let mut canonical = Connection::open_in_memory()?;
    migrate(&mut canonical, Some(read_scope(conn)?))?;
    if schema_objects(conn)? != schema_objects(&canonical)? {
        return Err(MemoryError::Invalid(
            "knowledge index schema differs from the supported migrations".into(),
        ));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

fn schema_objects(conn: &Connection) -> Result<Vec<SchemaObject>> {
    // Current migrations create fewer than 32 objects. Bound untrusted schema
    // rows and SQL text; an additional or oversized object cannot match.
    let mut statement = conn.prepare(
        "SELECT type,name,tbl_name,substr(sql,1,16384) FROM sqlite_schema ORDER BY type,name LIMIT 128",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                table: row.get(2)?,
                sql: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(super) fn read_scope(conn: &Connection) -> Result<Scope> {
    let key = |key: &str| -> Result<String> {
        conn.query_row(
            "SELECT value FROM knowledge_meta WHERE key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| MemoryError::Invalid(format!("knowledge index lacks {key}")))
    };
    let user_id = UserId(
        key("user_id")?
            .parse()
            .map_err(|_| MemoryError::Invalid("invalid knowledge user id".into()))?,
    );
    let bot_id = BotId(
        key("bot_id")?
            .parse()
            .map_err(|_| MemoryError::Invalid("invalid knowledge bot id".into()))?,
    );
    Ok(Scope { user_id, bot_id })
}
