//! Scoped knowledge snapshots and import reauthorization. Runtime quota ledgers
//! and source permissions are installation-local and never copied as grants.

use std::path::Path;

use memory_store::knowledge::{KnowledgeStore, Scope};
use protocol::{BotId, UserId};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{ImportError, Manifest, Result, invalid};

#[derive(PartialEq, Eq)]
struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

fn schema_objects(connection: &Connection) -> Result<Vec<SchemaObject>> {
    let mut statement = connection.prepare("SELECT type,substr(name,1,256),substr(tbl_name,1,256),substr(sql,1,16384) FROM sqlite_schema ORDER BY type,name LIMIT 128")?;
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

/// Imported triggers can undo source/local-runtime reauthorization or alter it
/// later. Attest the complete migrated bot schema before any sanitizer writes,
/// including the canonical append-only audit triggers and all indexes.
pub(crate) fn attest_bot_schema(connection: &Connection) -> Result<()> {
    let mut canonical = Connection::open_in_memory()?;
    storage::migrate::migrate(&mut canonical).map_err(|error| invalid(&error.to_string()))?;
    if schema_objects(connection)? != schema_objects(&canonical)? {
        return Err(ImportError::InvalidDatabase {
            db: "bot.db".into(),
            reason: "bot schema differs from the supported migrations".into(),
        });
    }
    Ok(())
}

/// Only this exact path is a knowledge database, never arbitrary nested `.db`.
pub(crate) fn bot_from_path(path: &str) -> Option<BotId> {
    let mut parts = path.split('/');
    if parts.next()? != "bots" {
        return None;
    }
    let raw = parts.next()?;
    let id = raw.parse().ok().map(BotId)?;
    if id.to_string() != raw || parts.next()? != "knowledge.db" || parts.next().is_some() {
        return None;
    }
    Some(id)
}

pub(crate) fn excluded(path: &str) -> bool {
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        matches!(
            part.as_str(),
            "knowledge-runtime.db"
                | "knowledge-runtime.db-wal"
                | "knowledge-runtime.db-shm"
                | "knowledge-runtime.db-journal"
                | "knowledge-authority.db"
                | "knowledge-authority.db-wal"
                | "knowledge-authority.db-shm"
                | "knowledge-authority.db-journal"
                | "knowledge-host.json"
                | "knowledge.db-wal"
                | "knowledge.db-shm"
                | "knowledge.db-journal"
        )
    })
}

pub(crate) fn prepare(staging: &Path, manifest: &Manifest) -> Result<()> {
    let bot_db =
        Connection::open_with_flags(staging.join("bot.db"), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    for record in &manifest.files {
        let Some(bot_id) = bot_from_path(&record.path) else {
            if record
                .path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("knowledge.db"))
            {
                return Err(invalid("invalid knowledge index owner path"));
            }
            continue;
        };
        let user_id: Option<String> = bot_db
            .query_row(
                "SELECT user_id FROM bots WHERE id=?1",
                [bot_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let user_id = user_id
            .and_then(|v| v.parse().ok())
            .map(UserId)
            .ok_or_else(|| invalid("knowledge index has no owning bot"))?;
        KnowledgeStore::prepare_import(staging.join(&record.path), Scope { user_id, bot_id })
            .map_err(|e| ImportError::InvalidDatabase {
                db: record.path.clone(),
                reason: e.to_string(),
            })?;
    }
    // The owner must re-enable automatic retrieval and query egress on this
    // installation. Cached content remains on disk but no source is active.
    let rows: Vec<(String, String)> = {
        let mut stmt = bot_db.prepare("SELECT id,config_json FROM bots")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, raw) in rows {
        let mut config: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| invalid("invalid bot config"))?;
        if let Some(knowledge) = config
            .get_mut("knowledge")
            .and_then(serde_json::Value::as_object_mut)
        {
            knowledge.insert("auto_retrieve".into(), false.into());
            // Provider endpoints, credential quota groups and native artifact
            // paths are authority on this installation. The dedicated settings
            // endpoint must derive them again from the owner's selection.
            knowledge.insert("embedding".into(), serde_json::Value::Null);
            bot_db.execute(
                "UPDATE bots SET config_json=?2 WHERE id=?1",
                rusqlite::params![id, serde_json::to_string(&config)?],
            )?;
        }
    }
    drop(bot_db);
    // Persist the authorization reset as part of the staged installation.
    for path in std::iter::once("bot.db").chain(
        manifest
            .files
            .iter()
            .filter(|record| bot_from_path(&record.path).is_some())
            .map(|record| record.path.as_str()),
    ) {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(staging.join(path))?
            .sync_all()?;
    }
    Ok(())
}
