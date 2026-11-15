//! The [`Storage`] repository.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use protocol::{BotId, ChatId, RunId, UserId};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{Result, StorageError};
use crate::migrate;
use crate::records::{
    Approval, AuditEntry, Bot, BotSkillAssignment, Chat, EventIndexRow, Message, Run, RunStep,
    SkillAssignmentMode, User,
};

/// Current Unix time in milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn conv_err<E: std::error::Error + Send + Sync + 'static>(idx: usize, e: E) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e))
}

fn uuid_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Uuid> {
    let s: String = row.get(idx)?;
    Uuid::parse_str(&s).map_err(|e| conv_err(idx, e))
}

fn opt_uuid_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Uuid>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| Uuid::parse_str(&s).map_err(|e| conv_err(idx, e)))
        .transpose()
}

fn json_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Value> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| conv_err(idx, e))
}

fn opt_json_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Value>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| serde_json::from_str(&s).map_err(|e| conv_err(idx, e)))
        .transpose()
}

fn to_u32(row: &Row<'_>, idx: usize) -> rusqlite::Result<u32> {
    let v: i64 = row.get(idx)?;
    u32::try_from(v).map_err(|e| conv_err(idx, e))
}

fn opt_json_text(v: Option<&Value>) -> Result<Option<String>> {
    Ok(v.map(serde_json::to_string).transpose()?)
}

/// Synchronous SQLite repository.  Cheap to share behind an `Arc`; calls
/// serialise on an internal mutex.
#[derive(Debug)]
pub struct Storage {
    conn: Mutex<Connection>,
    data_dir: PathBuf,
}

impl Storage {
    /// Validate and migrate an existing staged database without creating it.
    /// Checks the columns used by this repository and foreign-key consistency;
    /// a schema version and SQLite page integrity alone do not establish either.
    /// Normal [`Self::open`] behavior is unchanged.
    ///
    /// # Errors
    /// Fails on corruption, an incompatible schema, or migration errors.
    pub fn validate_and_migrate(path: impl AsRef<Path>) -> Result<u32> {
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let integrity: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some(integrity),
            )
            .into());
        }
        migrate::migrate(&mut conn)?;
        for select in [
            "SELECT id, name, created_at_ms FROM users",
            "SELECT id, user_id, name, config_json, created_at_ms FROM bots",
            "SELECT id, user_id, bot_id, title, created_at_ms FROM chats",
            "SELECT id, user_id, chat_id, run_id, role, content, created_at_ms FROM messages",
            RUN_SELECT,
            STEP_SELECT,
            APPROVAL_SELECT,
            "SELECT id, user_id, run_id, actor, action, detail_json, ts_ms FROM audit_log",
            "SELECT gseq, run_id, seq FROM event_index",
            "SELECT bot_id, mode, names_json, updated_at_ms, version FROM bot_skill_assignment",
            "SELECT id, generation FROM bot_skill_assignment_state",
        ] {
            conn.prepare(select)?;
        }
        {
            let mut st = conn.prepare(
                "SELECT bot_id, mode, names_json, updated_at_ms, version FROM bot_skill_assignment",
            )?;
            let mut rows = st.query([])?;
            while let Some(row) = rows.next()? {
                let _ = skill_assignment_row(row)?;
            }
        }
        if conn.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY),
                Some("database contains invalid foreign-key references".into()),
            )
            .into());
        }
        migrate::current_version(&conn)
    }

    /// Open (creating if needed) `<data_dir>/bot.db`, enable WAL + foreign
    /// keys and run pending migrations.
    ///
    /// # Errors
    /// Fails on filesystem, SQLite or migration errors.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)?;
        let conn = Connection::open(data_dir.join("bot.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn, data_dir)
    }

    /// In-memory database (tests); `data_dir` is still used for event logs.
    ///
    /// # Errors
    /// Fails on SQLite or migration errors.
    pub fn open_in_memory(data_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, data_dir.into())
    }

    fn init(mut conn: Connection, data_dir: PathBuf) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            data_dir,
        })
    }

    /// Data directory (event logs live under `runs/`).
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---- users ----

    /// Insert a user.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn insert_user(&self, u: &User) -> Result<()> {
        self.conn().execute(
            "INSERT INTO users (id, name, created_at_ms) VALUES (?1, ?2, ?3)",
            params![u.id.to_string(), u.name, u.created_at_ms],
        )?;
        Ok(())
    }

    /// Fetch a user.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_user(&self, id: UserId) -> Result<Option<User>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, name, created_at_ms FROM users WHERE id = ?1",
                [id.to_string()],
                user_row,
            )
            .optional()?)
    }

    /// List all users, oldest first.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_users(&self) -> Result<Vec<User>> {
        let conn = self.conn();
        let mut st = conn.prepare("SELECT id, name, created_at_ms FROM users ORDER BY id")?;
        Ok(st
            .query_map([], user_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---- bots ----

    /// Insert a bot.
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors.
    pub fn insert_bot(&self, b: &Bot) -> Result<()> {
        self.conn().execute(
            "INSERT INTO bots (id, user_id, name, config_json, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                b.id.to_string(),
                b.user_id.to_string(),
                b.name,
                serde_json::to_string(&b.config)?,
                b.created_at_ms
            ],
        )?;
        Ok(())
    }

    /// Fetch a bot.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_bot(&self, id: BotId) -> Result<Option<Bot>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, user_id, name, config_json, created_at_ms FROM bots WHERE id = ?1",
                [id.to_string()],
                bot_row,
            )
            .optional()?)
    }

    /// List a user's bots.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_bots(&self, user_id: UserId) -> Result<Vec<Bot>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, name, config_json, created_at_ms FROM bots WHERE user_id = ?1 ORDER BY id",
        )?;
        Ok(st
            .query_map([user_id.to_string()], bot_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Replace a bot's name and configuration.  Returns `false` if it does not exist.
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors.
    pub fn update_bot(&self, id: BotId, name: &str, config: &Value) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE bots SET name = ?2, config_json = ?3 WHERE id = ?1",
            params![id.to_string(), name, serde_json::to_string(config)?],
        )?;
        Ok(n > 0)
    }

    /// Delete a bot and its skill assignment row in the same transaction.
    /// Returns `false` if it does not exist.
    ///
    /// # Errors
    /// Fails on SQLite errors, in particular a foreign-key violation while
    /// chats or runs still reference the bot.
    pub fn delete_bot(&self, id: BotId) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let bid = id.to_string();
        tx.execute("DELETE FROM bot_skill_assignment WHERE bot_id = ?1", [&bid])?;
        let n = tx.execute("DELETE FROM bots WHERE id = ?1", [&bid])?;
        tx.commit()?;
        Ok(n > 0)
    }

    // ---- bot_skill_assignment ----

    /// Fetch a bot's skill assignment row, or `None` when the bot uses the
    /// default (`all` shared and bundled skills).
    ///
    /// # Errors
    /// Fails on SQLite or malformed row errors.
    pub fn get_bot_skill_assignment(&self, bot_id: BotId) -> Result<Option<BotSkillAssignment>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT bot_id, mode, names_json, updated_at_ms, version FROM bot_skill_assignment WHERE bot_id = ?1",
                [bot_id.to_string()],
                skill_assignment_row,
            )
            .optional()?)
    }

    /// Inspect a bot's skill assignment row without failing on malformed row
    /// contents, so management surfaces can report and repair corrupt rows
    /// while runtime callers continue to fail closed.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn inspect_bot_skill_assignment(&self, bot_id: BotId) -> Result<SkillAssignmentLookup> {
        let conn = self.conn();
        let bid = bot_id.to_string();
        let res = conn
            .query_row(
                "SELECT bot_id, mode, names_json, updated_at_ms, version FROM bot_skill_assignment WHERE bot_id = ?1",
                [&bid],
                skill_assignment_row,
            )
            .optional();
        match res {
            Ok(row) => Ok(SkillAssignmentLookup::Ok(row)),
            Err(rusqlite::Error::FromSqlConversionFailure(_, _, err)) => {
                let raw_ver: Option<i64> = conn
                    .query_row(
                        "SELECT version FROM bot_skill_assignment WHERE bot_id = ?1",
                        [&bid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                let version = raw_ver.and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
                Ok(SkillAssignmentLookup::Corrupt {
                    version,
                    reason: err.to_string(),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Current DB-backed generation counter incremented on every insert, update,
    /// or delete on `bot_skill_assignment`.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn bot_skill_assignment_generation(&self) -> Result<u64> {
        let gen_val: i64 = self.conn().query_row(
            "SELECT generation FROM bot_skill_assignment_state WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(gen_val).unwrap_or(0))
    }

    /// Set a bot's skill assignment. `SkillAssignmentMode::All` deletes any
    /// existing row (restoring the default).
    ///
    /// # Errors
    /// Fails on invalid skill names, too many names (> 200), SQLite/JSON
    /// errors, or a missing bot foreign key.
    pub fn set_bot_skill_assignment(
        &self,
        bot_id: BotId,
        mode: SkillAssignmentMode,
        names: &[String],
    ) -> Result<BotSkillAssignment> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row = write_skill_assignment_tx(&tx, bot_id, mode, names, None)?;
        tx.commit()?;
        Ok(row)
    }

    /// Write a `bot.skills_assigned` audit entry and update the bot's skill
    /// assignment in a single transaction, ensuring the audit record is
    /// persisted before the change takes effect.
    ///
    /// # Errors
    /// Fails on invalid skill names, too many names (> 200), or SQLite/JSON
    /// errors.
    pub fn set_bot_skill_assignment_audited(
        &self,
        user_id: UserId,
        bot_id: BotId,
        mode: SkillAssignmentMode,
        names: &[String],
        actor: &str,
        detail: &Value,
    ) -> Result<BotSkillAssignment> {
        self.set_bot_skill_assignment_audited_versioned(
            user_id, bot_id, mode, names, actor, detail, None,
        )
    }

    /// Version-checked variant of [`Self::set_bot_skill_assignment_audited`].
    /// The audit diff is derived from the stored assignment inside the transaction;
    /// caller-supplied `bot_id`, `mode`, `added` and `removed` are replaced.
    ///
    /// # Errors
    /// Fails with [`StorageError::Conflict`] if `expected_version` is `Some`
    /// and does not match the stored row's version (`0` when no row exists).
    #[allow(clippy::too_many_arguments)]
    pub fn set_bot_skill_assignment_audited_versioned(
        &self,
        user_id: UserId,
        bot_id: BotId,
        mode: SkillAssignmentMode,
        names: &[String],
        actor: &str,
        detail: &Value,
        expected_version: Option<u64>,
    ) -> Result<BotSkillAssignment> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let previous = tx.query_row(
            "SELECT bot_id, mode, names_json, updated_at_ms, version FROM bot_skill_assignment WHERE bot_id = ?1",
            [bot_id.to_string()],
            skill_assignment_row,
        ).optional();
        let previous_names: BTreeSet<String> = match previous {
            Ok(Some(row)) if row.mode == SkillAssignmentMode::Selected => {
                row.names.into_iter().collect()
            }
            Ok(_) | Err(rusqlite::Error::FromSqlConversionFailure(..)) => BTreeSet::default(),
            Err(error) => return Err(error.into()),
        };
        let row = write_skill_assignment_tx(&tx, bot_id, mode, names, expected_version)?;
        let next_names: BTreeSet<String> = row.names.iter().cloned().collect();
        let added: Vec<_> = next_names.difference(&previous_names).collect();
        let removed: Vec<_> = previous_names.difference(&next_names).collect();
        let mut detail = detail.as_object().cloned().unwrap_or_default();
        detail.insert("bot_id".into(), serde_json::json!(bot_id.to_string()));
        detail.insert(
            "mode".into(),
            serde_json::json!(match mode {
                SkillAssignmentMode::All => "all",
                SkillAssignmentMode::Selected => "selected",
            }),
        );
        detail.insert("added".into(), serde_json::json!(added));
        detail.insert("removed".into(), serde_json::json!(removed));
        tx.execute(
            "INSERT INTO audit_log (user_id, run_id, actor, action, detail_json, ts_ms) VALUES (?1, NULL, ?2, ?3, ?4, ?5)",
            params![
                user_id.to_string(),
                actor,
                "bot.skills_assigned",
                serde_json::to_string(&detail)?,
                now_ms()
            ],
        )?;
        tx.commit()?;
        Ok(row)
    }

    /// Delete a bot's skill assignment row, returning `true` if one existed.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn delete_bot_skill_assignment(&self, bot_id: BotId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM bot_skill_assignment WHERE bot_id = ?1",
            [bot_id.to_string()],
        )?;
        Ok(n > 0)
    }

    // ---- chats ----

    /// Insert a chat.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn insert_chat(&self, c: &Chat) -> Result<()> {
        self.conn().execute(
            "INSERT INTO chats (id, user_id, bot_id, title, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                c.id.to_string(),
                c.user_id.to_string(),
                c.bot_id.to_string(),
                c.title,
                c.created_at_ms
            ],
        )?;
        Ok(())
    }

    /// Fetch a chat.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_chat(&self, id: ChatId) -> Result<Option<Chat>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, user_id, bot_id, title, created_at_ms FROM chats WHERE id = ?1",
                [id.to_string()],
                chat_row,
            )
            .optional()?)
    }

    /// List a user's chats.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_chats(&self, user_id: UserId) -> Result<Vec<Chat>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, bot_id, title, created_at_ms FROM chats WHERE user_id = ?1 ORDER BY id",
        )?;
        Ok(st
            .query_map([user_id.to_string()], chat_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---- messages ----

    /// Insert a message.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn insert_message(&self, m: &Message) -> Result<()> {
        self.conn().execute(
            "INSERT INTO messages (id, user_id, chat_id, run_id, role, content, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.id.to_string(),
                m.user_id.to_string(),
                m.chat_id.to_string(),
                m.run_id.map(|r| r.to_string()),
                m.role,
                m.content,
                m.created_at_ms
            ],
        )?;
        Ok(())
    }

    /// Fetch a message.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_message(&self, id: Uuid) -> Result<Option<Message>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, user_id, chat_id, run_id, role, content, created_at_ms FROM messages WHERE id = ?1",
                [id.to_string()],
                message_row,
            )
            .optional()?)
    }

    /// List a chat's messages in order.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_messages(&self, chat_id: ChatId) -> Result<Vec<Message>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, chat_id, run_id, role, content, created_at_ms FROM messages WHERE chat_id = ?1 ORDER BY created_at_ms, id",
        )?;
        Ok(st
            .query_map([chat_id.to_string()], message_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Read the newest contiguous suffix of a chat within both limits.
    ///
    /// Returns `(messages, omitted_older_messages)` in chronological order.
    /// `exclude_run` is filtered before applying the limits, so messages that
    /// the caller reconstructs from that run's steps do not consume the budget.
    /// A message that does not fit stops the scan; its text is never copied
    /// into Rust, and older rows are not substituted for it. Text is never
    /// truncated, including at UTF-8 boundaries. At most `max_messages + 1`
    /// rows are considered and returned content totals at most `max_bytes`.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_messages_bounded(
        &self,
        chat_id: ChatId,
        exclude_run: Option<RunId>,
        max_messages: usize,
        max_bytes: usize,
    ) -> Result<(Vec<Message>, bool)> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, chat_id, run_id, role, content, created_at_ms, length(CAST(content AS BLOB)) \
             FROM messages WHERE chat_id = ?1 AND (?2 IS NULL OR run_id IS NULL OR run_id <> ?2) \
             ORDER BY created_at_ms DESC, id DESC LIMIT ?3",
        )?;
        let limit = i64::try_from(max_messages.saturating_add(1)).unwrap_or(i64::MAX);
        let mut rows = st.query(params![
            chat_id.to_string(),
            exclude_run.map(|r| r.to_string()),
            limit
        ])?;
        let mut messages = Vec::new();
        let mut used = 0usize;
        let mut omitted = false;
        while let Some(row) = rows.next()? {
            let bytes = usize::try_from(row.get::<_, i64>(7)?).map_err(|e| conv_err(7, e))?;
            if messages.len() == max_messages || bytes > max_bytes.saturating_sub(used) {
                omitted = true;
                break;
            }
            messages.push(message_row(row)?);
            used += bytes;
        }
        messages.reverse();
        Ok((messages, omitted))
    }

    // ---- runs ----

    /// Insert a run.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn insert_run(&self, r: &Run) -> Result<()> {
        self.conn().execute(
            "INSERT INTO runs (id, user_id, bot_id, chat_id, kind, state, error, created_at_ms, updated_at_ms, parent_run_id, spec_json, computer_target, pause_reason, computer_node, computer_node_name, paused_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                r.id.to_string(),
                r.user_id.to_string(),
                r.bot_id.to_string(),
                r.chat_id.map(|c| c.to_string()),
                r.kind,
                r.state,
                r.error,
                r.created_at_ms,
                r.updated_at_ms,
                r.parent_run_id.map(|p| p.to_string()),
                r.spec.as_ref().map(serde_json::to_string).transpose()?,
                r.computer_target,
                r.pause_reason,
                r.computer_node, r.computer_node_name, r.paused_at_ms,
            ],
        )?;
        Ok(())
    }

    /// Fetch a run.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_run(&self, id: RunId) -> Result<Option<Run>> {
        Ok(self
            .conn()
            .query_row(
                &format!("{RUN_SELECT} WHERE id = ?1"),
                [id.to_string()],
                run_row,
            )
            .optional()?)
    }

    /// List a user's runs.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_runs(&self, user_id: UserId) -> Result<Vec<Run>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("{RUN_SELECT} WHERE user_id = ?1 ORDER BY id"))?;
        Ok(st
            .query_map([user_id.to_string()], run_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Ids and states of the user's runs that are not terminal (`queued`,
    /// `running`, `waiting_approval`, `paused`): one statement, no spec
    /// payloads, so it is cheap on a long history and a consistent snapshot.
    /// The state list is a literal: `daemon-api` tests pin it against
    /// `RunState::ALL` minus the terminal states.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_open_run_states(&self, user_id: UserId) -> Result<Vec<(RunId, String)>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, state FROM runs WHERE user_id = ?1 AND state IN ('queued', 'running', 'waiting_approval', 'paused') ORDER BY id",
        )?;
        Ok(st
            .query_map([user_id.to_string()], |r| {
                Ok((RunId(uuid_at(r, 0)?), r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The state of one run of `user_id` (`None`: no such run for that user).
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn run_state_of_user(&self, user_id: UserId, id: RunId) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT state FROM runs WHERE id = ?1 AND user_id = ?2",
                [id.to_string(), user_id.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The direct children (subagent runs) of a run, oldest first.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_child_runs(&self, parent: RunId) -> Result<Vec<Run>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "{RUN_SELECT} WHERE parent_run_id = ?1 ORDER BY id"
        ))?;
        Ok(st
            .query_map([parent.to_string()], run_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// List runs (all users) currently in the given serialised state.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_runs_by_state(&self, state: &str) -> Result<Vec<Run>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("{RUN_SELECT} WHERE state = ?1 ORDER BY id"))?;
        Ok(st
            .query_map([state], run_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Set a run's state (and error text).  Transition legality is the
    /// caller's responsibility. Transitions out of `paused` clear `pause_reason`.
    ///
    /// # Errors
    /// Fails on SQLite errors or if the run does not exist.
    pub fn update_run_state(&self, id: RunId, state: &str, error: Option<&str>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE runs SET state = ?2, error = ?3, updated_at_ms = ?4, paused_at_ms = CASE WHEN ?2 = 'paused' THEN COALESCE(paused_at_ms, ?4) ELSE NULL END, pause_reason = CASE WHEN ?2 = 'paused' THEN pause_reason ELSE NULL END WHERE id = ?1",
            params![id.to_string(), state, error, now_ms()],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    /// Record the resolved computer target label (`pc`, `host`, `browser`,
    /// `off`) fixed at run start.
    ///
    /// # Errors
    /// Fails on SQLite errors or if the run does not exist.
    pub fn set_run_computer_target(&self, id: RunId, target: Option<&str>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE runs SET computer_target = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), target, now_ms()],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    /// Persist label and concrete target together before any tool can run.
    ///
    /// # Errors
    /// Fails on SQLite errors or if the run does not exist.
    pub fn bind_run_computer(
        &self,
        id: RunId,
        label: &str,
        node: Option<&str>,
        name: Option<&str>,
    ) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE runs SET computer_target = ?2, computer_node = ?3, computer_node_name = ?4 WHERE id = ?1 AND (computer_target IS NULL OR computer_target = ?2) AND (computer_node IS NULL OR computer_node = ?3)",
            params![id.to_string(), label, node, name],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    /// Update the pause reason on a paused run (e.g. `waiting_for_pc`).
    ///
    /// # Errors
    /// Fails on SQLite errors or if the run does not exist.
    pub fn set_run_pause_reason(&self, id: RunId, reason: Option<&str>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE runs SET pause_reason = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id.to_string(), reason, now_ms()],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    /// List all runs currently in `paused` state with the given `pause_reason`.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_paused_runs_with_reason(&self, reason: &str) -> Result<Vec<Run>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "{RUN_SELECT} WHERE state = 'paused' AND pause_reason = ?1 ORDER BY id"
        ))?;
        Ok(st
            .query_map([reason], run_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---- run_steps ----

    /// Insert a step (the persisted intent).
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors.
    pub fn insert_step(&self, s: &RunStep) -> Result<()> {
        self.conn().execute(
            "INSERT INTO run_steps (id, run_id, user_id, idx, name, input_json, idempotent, retry_json, status, attempt, output_json, error, started_at_ms, finished_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                s.id.to_string(),
                s.run_id.to_string(),
                s.user_id.to_string(),
                s.idx,
                s.name,
                serde_json::to_string(&s.input)?,
                s.idempotent,
                opt_json_text(s.retry.as_ref())?,
                s.status,
                s.attempt,
                opt_json_text(s.output.as_ref())?,
                s.error,
                s.started_at_ms,
                s.finished_at_ms
            ],
        )?;
        Ok(())
    }

    /// Fetch a step.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_step(&self, id: Uuid) -> Result<Option<RunStep>> {
        Ok(self
            .conn()
            .query_row(
                &format!("{STEP_SELECT} WHERE id = ?1"),
                [id.to_string()],
                step_row,
            )
            .optional()?)
    }

    /// List a run's steps in order.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_steps(&self, run_id: RunId) -> Result<Vec<RunStep>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("{STEP_SELECT} WHERE run_id = ?1 ORDER BY idx"))?;
        Ok(st
            .query_map([run_id.to_string()], step_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Update a step's status/attempt/result.  `finished_at_ms` is set to now
    /// when `finished` is true, else cleared.
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors or if the step does not exist.
    pub fn update_step(
        &self,
        id: Uuid,
        status: &str,
        attempt: u32,
        output: Option<&Value>,
        error: Option<&str>,
        finished: bool,
    ) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE run_steps SET status = ?2, attempt = ?3, output_json = ?4, error = ?5, finished_at_ms = ?6 WHERE id = ?1",
            params![
                id.to_string(),
                status,
                attempt,
                opt_json_text(output)?,
                error,
                finished.then(now_ms)
            ],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("step {id}")));
        }
        Ok(())
    }

    /// Replace a step's stored input and/or output JSON without touching its
    /// status, attempt, error or timestamps. For scrubbing a value that must
    /// not stay in the record once the step has run.
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors or if the step does not exist.
    pub fn rewrite_step_json(
        &self,
        id: Uuid,
        input: Option<&Value>,
        output: Option<&Value>,
    ) -> Result<()> {
        let conn = self.conn();
        let mut n = 0;
        if let Some(input) = input {
            n += conn.execute(
                "UPDATE run_steps SET input_json = ?2 WHERE id = ?1",
                params![id.to_string(), serde_json::to_string(input)?],
            )?;
        }
        if let Some(output) = output {
            n += conn.execute(
                "UPDATE run_steps SET output_json = ?2 WHERE id = ?1",
                params![id.to_string(), serde_json::to_string(output)?],
            )?;
        }
        if n == 0 {
            return Err(StorageError::NotFound(format!("step {id}")));
        }
        Ok(())
    }

    /// Re-label every step of `run_id` whose status is `from` as `to`;
    /// returns the number changed.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn relabel_steps(&self, run_id: RunId, from: &str, to: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE run_steps SET status = ?3, finished_at_ms = ?4 WHERE run_id = ?1 AND status = ?2",
            params![run_id.to_string(), from, to, now_ms()],
        )?)
    }

    // ---- approvals ----

    /// Insert an approval request.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn insert_approval(&self, a: &Approval) -> Result<()> {
        self.conn().execute(
            "INSERT INTO approvals (id, user_id, run_id, step_id, summary, decision, requested_at_ms, decided_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                a.id.to_string(),
                a.user_id.to_string(),
                a.run_id.to_string(),
                a.step_id.map(|s| s.to_string()),
                a.summary,
                a.decision,
                a.requested_at_ms,
                a.decided_at_ms
            ],
        )?;
        Ok(())
    }

    /// Fetch an approval.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn get_approval(&self, id: Uuid) -> Result<Option<Approval>> {
        Ok(self
            .conn()
            .query_row(
                &format!("{APPROVAL_SELECT} WHERE id = ?1"),
                [id.to_string()],
                approval_row,
            )
            .optional()?)
    }

    /// List a run's approvals.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_approvals(&self, run_id: RunId) -> Result<Vec<Approval>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("{APPROVAL_SELECT} WHERE run_id = ?1 ORDER BY id"))?;
        Ok(st
            .query_map([run_id.to_string()], approval_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Record the decision on a pending approval.
    ///
    /// # Errors
    /// Fails on SQLite errors or if the approval does not exist / is decided.
    pub fn decide_approval(&self, id: Uuid, decision: &str) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE approvals SET decision = ?2, decided_at_ms = ?3 WHERE id = ?1 AND decision IS NULL",
            params![id.to_string(), decision, now_ms()],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound(format!("pending approval {id}")));
        }
        Ok(())
    }

    // ---- audit_log (append-only: no update/delete API) ----

    /// Append an audit entry; returns its id.
    ///
    /// # Errors
    /// Fails on SQLite/JSON errors.
    pub fn append_audit(
        &self,
        user_id: UserId,
        run_id: Option<RunId>,
        actor: &str,
        action: &str,
        detail: &Value,
    ) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO audit_log (user_id, run_id, actor, action, detail_json, ts_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                user_id.to_string(),
                run_id.map(|r| r.to_string()),
                actor,
                action,
                serde_json::to_string(detail)?,
                now_ms()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// List a user's audit entries in insertion order.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_audit(&self, user_id: UserId) -> Result<Vec<AuditEntry>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, run_id, actor, action, detail_json, ts_ms FROM audit_log WHERE user_id = ?1 ORDER BY id",
        )?;
        Ok(st
            .query_map([user_id.to_string()], audit_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// The newest audit row of one action for a user (indexed by `id`, no full scan).
    ///
    /// # Errors
    /// Fails on a database error.
    pub fn last_audit_by_action(
        &self,
        user_id: UserId,
        action: &str,
    ) -> Result<Option<AuditEntry>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, user_id, run_id, actor, action, detail_json, ts_ms FROM audit_log WHERE user_id = ?1 AND action = ?2 ORDER BY id DESC LIMIT 1",
        )?;
        Ok(st
            .query_map(params![user_id.to_string(), action], audit_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .next())
    }

    // ---- global event order ----

    /// Highest `gseq` handed out so far (`0` when no event was indexed yet).
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn last_gseq(&self) -> Result<u64> {
        let v: i64 =
            self.conn()
                .query_row("SELECT COALESCE(MAX(gseq), 0) FROM event_index", [], |r| {
                    r.get(0)
                })?;
        Ok(u64::try_from(v).unwrap_or(0))
    }

    /// Record that event `seq` of `run_id` has the daemon-wide number `gseq`.
    ///
    /// # Errors
    /// Fails on SQLite errors (including a reused `gseq`).
    pub fn index_event(&self, gseq: u64, run_id: RunId, seq: u64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO event_index (gseq, run_id, seq) VALUES (?1, ?2, ?3)",
            params![
                i64::try_from(gseq).unwrap_or(i64::MAX),
                run_id.to_string(),
                i64::try_from(seq).unwrap_or(i64::MAX)
            ],
        )?;
        Ok(())
    }

    /// Up to `limit` index rows with `gseq > after`, in order.
    ///
    /// # Errors
    /// Fails on SQLite errors.
    pub fn list_event_index(&self, after: u64, limit: u32) -> Result<Vec<EventIndexRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT gseq, run_id, seq FROM event_index WHERE gseq > ?1 ORDER BY gseq LIMIT ?2",
        )?;
        let rows = st.query_map(
            params![i64::try_from(after).unwrap_or(i64::MAX), limit],
            |r| {
                let gseq: i64 = r.get(0)?;
                let seq: i64 = r.get(2)?;
                Ok(EventIndexRow {
                    gseq: u64::try_from(gseq).unwrap_or(0),
                    run_id: RunId(uuid_at(r, 1)?),
                    seq: u64::try_from(seq).unwrap_or(0),
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Run a raw statement (test hook to prove schema-level guarantees).
    #[cfg(test)]
    pub(crate) fn raw_execute(&self, sql: &str) -> rusqlite::Result<usize> {
        self.conn().execute(sql, [])
    }
}

const RUN_SELECT: &str = "SELECT id, user_id, bot_id, chat_id, kind, state, error, created_at_ms, updated_at_ms, parent_run_id, spec_json, computer_target, pause_reason, computer_node, computer_node_name, paused_at_ms FROM runs";
const STEP_SELECT: &str = "SELECT id, run_id, user_id, idx, name, input_json, idempotent, retry_json, status, attempt, output_json, error, started_at_ms, finished_at_ms FROM run_steps";
const APPROVAL_SELECT: &str = "SELECT id, user_id, run_id, step_id, summary, decision, requested_at_ms, decided_at_ms FROM approvals";

fn user_row(r: &Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: UserId(uuid_at(r, 0)?),
        name: r.get(1)?,
        created_at_ms: r.get(2)?,
    })
}

/// Outcome of inspecting a bot's skill assignment row for UI/management repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillAssignmentLookup {
    /// Well-formed row, or `None` when the bot uses the default (`All`).
    Ok(Option<BotSkillAssignment>),
    /// Row exists in `bot_skill_assignment` but its mode or `names_json` is
    /// malformed; runtime enforcement fails closed while management surfaces can
    /// overwrite it.
    Corrupt {
        /// Stored row version (or `0` if unreadable).
        version: u64,
        /// Validation/parse error.
        reason: String,
    },
}

const MAX_SKILL_ASSIGNMENTS: usize = 200;
const MAX_SKILL_NAME_LEN: usize = 64;

/// Validate a skill name using the same grammar as `skills::validate_name`.
#[must_use]
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SKILL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && !name.contains("..")
        && !name.ends_with('.')
}

fn normalize_skill_names(names: &[String]) -> std::result::Result<Vec<String>, String> {
    if names.len() > MAX_SKILL_ASSIGNMENTS {
        return Err(format!(
            "at most {MAX_SKILL_ASSIGNMENTS} assigned skills are allowed"
        ));
    }
    let mut set = std::collections::BTreeSet::new();
    for name in names {
        if !is_valid_skill_name(name) {
            return Err(format!("invalid skill name `{name}`"));
        }
        set.insert(name.clone());
    }
    Ok(set.into_iter().collect())
}

fn write_skill_assignment_tx(
    tx: &rusqlite::Transaction<'_>,
    bot_id: BotId,
    mode: SkillAssignmentMode,
    names: &[String],
    expected_version: Option<u64>,
) -> Result<BotSkillAssignment> {
    let bid = bot_id.to_string();
    if let Some(expected) = expected_version {
        let current_raw: Option<i64> = tx
            .query_row(
                "SELECT version FROM bot_skill_assignment WHERE bot_id = ?1",
                [&bid],
                |r| r.get(0),
            )
            .optional()?;
        let current = current_raw.and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
        if current != expected {
            return Err(StorageError::Conflict(format!(
                "bot skill assignment changed (expected version {expected}, current {current})"
            )));
        }
    }
    let ts = now_ms();
    let clean = normalize_skill_names(names)
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg))?;
    match mode {
        SkillAssignmentMode::All => {
            tx.execute("DELETE FROM bot_skill_assignment WHERE bot_id = ?1", [&bid])?;
            Ok(BotSkillAssignment {
                bot_id,
                mode,
                names: Vec::new(),
                updated_at_ms: ts,
                version: 0,
            })
        }
        SkillAssignmentMode::Selected => {
            let json = serde_json::to_string(&clean)?;
            let cur_gen: i64 = tx.query_row(
                "SELECT generation FROM bot_skill_assignment_state WHERE id = 1",
                [],
                |r| r.get(0),
            )?;
            let next_ver = cur_gen.saturating_add(1);
            tx.execute(
                "INSERT INTO bot_skill_assignment (bot_id, mode, names_json, updated_at_ms, version) \
                 VALUES (?1, 'selected', ?2, ?3, ?4) \
                 ON CONFLICT(bot_id) DO UPDATE SET \
                 mode = excluded.mode, names_json = excluded.names_json, updated_at_ms = excluded.updated_at_ms, version = excluded.version",
                params![bid, json, ts, next_ver],
            )?;
            Ok(BotSkillAssignment {
                bot_id,
                mode,
                names: clean,
                updated_at_ms: ts,
                version: u64::try_from(next_ver).unwrap_or(1),
            })
        }
    }
}

fn skill_assignment_row(r: &Row<'_>) -> rusqlite::Result<BotSkillAssignment> {
    let bot_id = BotId(uuid_at(r, 0)?);
    let mode_str: String = r.get(1)?;
    let mode = match mode_str.as_str() {
        "all" => SkillAssignmentMode::All,
        "selected" => SkillAssignmentMode::Selected,
        other => {
            return Err(conv_err(
                1,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid skill assignment mode `{other}`"),
                ),
            ));
        }
    };
    let names_raw: String = r.get(2)?;
    let parsed: Vec<String> = serde_json::from_str(&names_raw).map_err(|e| conv_err(2, e))?;
    let names = normalize_skill_names(&parsed)
        .map_err(|msg| conv_err(2, std::io::Error::new(std::io::ErrorKind::InvalidData, msg)))?;
    let ver: i64 = r.get(4)?;
    Ok(BotSkillAssignment {
        bot_id,
        mode,
        names,
        updated_at_ms: r.get(3)?,
        version: u64::try_from(ver).unwrap_or(0),
    })
}

fn bot_row(r: &Row<'_>) -> rusqlite::Result<Bot> {
    Ok(Bot {
        id: BotId(uuid_at(r, 0)?),
        user_id: UserId(uuid_at(r, 1)?),
        name: r.get(2)?,
        config: json_at(r, 3)?,
        created_at_ms: r.get(4)?,
    })
}

fn chat_row(r: &Row<'_>) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: ChatId(uuid_at(r, 0)?),
        user_id: UserId(uuid_at(r, 1)?),
        bot_id: BotId(uuid_at(r, 2)?),
        title: r.get(3)?,
        created_at_ms: r.get(4)?,
    })
}

fn message_row(r: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: uuid_at(r, 0)?,
        user_id: UserId(uuid_at(r, 1)?),
        chat_id: ChatId(uuid_at(r, 2)?),
        run_id: opt_uuid_at(r, 3)?.map(RunId),
        role: r.get(4)?,
        content: r.get(5)?,
        created_at_ms: r.get(6)?,
    })
}

fn run_row(r: &Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: RunId(uuid_at(r, 0)?),
        user_id: UserId(uuid_at(r, 1)?),
        bot_id: BotId(uuid_at(r, 2)?),
        chat_id: opt_uuid_at(r, 3)?.map(ChatId),
        kind: r.get(4)?,
        state: r.get(5)?,
        error: r.get(6)?,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
        parent_run_id: opt_uuid_at(r, 9)?.map(RunId),
        spec: opt_json_at(r, 10)?,
        computer_target: r.get(11)?,
        computer_node: r.get(13)?,
        computer_node_name: r.get(14)?,
        paused_at_ms: r.get(15)?,
        pause_reason: r.get(12)?,
    })
}

fn step_row(r: &Row<'_>) -> rusqlite::Result<RunStep> {
    Ok(RunStep {
        id: uuid_at(r, 0)?,
        run_id: RunId(uuid_at(r, 1)?),
        user_id: UserId(uuid_at(r, 2)?),
        idx: to_u32(r, 3)?,
        name: r.get(4)?,
        input: json_at(r, 5)?,
        idempotent: r.get(6)?,
        retry: opt_json_at(r, 7)?,
        status: r.get(8)?,
        attempt: to_u32(r, 9)?,
        output: opt_json_at(r, 10)?,
        error: r.get(11)?,
        started_at_ms: r.get(12)?,
        finished_at_ms: r.get(13)?,
    })
}

fn approval_row(r: &Row<'_>) -> rusqlite::Result<Approval> {
    Ok(Approval {
        id: uuid_at(r, 0)?,
        user_id: UserId(uuid_at(r, 1)?),
        run_id: RunId(uuid_at(r, 2)?),
        step_id: opt_uuid_at(r, 3)?,
        summary: r.get(4)?,
        decision: r.get(5)?,
        requested_at_ms: r.get(6)?,
        decided_at_ms: r.get(7)?,
    })
}

fn audit_row(r: &Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        id: r.get(0)?,
        user_id: UserId(uuid_at(r, 1)?),
        run_id: opt_uuid_at(r, 2)?.map(RunId),
        actor: r.get(3)?,
        action: r.get(4)?,
        detail: json_at(r, 5)?,
        ts_ms: r.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fx {
        st: Storage,
        user: User,
        bot: Bot,
        chat: Chat,
    }

    fn fx() -> Fx {
        let st = Storage::open_in_memory(std::env::temp_dir()).unwrap();
        let user = User {
            id: UserId::new(),
            name: "emre".into(),
            created_at_ms: 1,
        };
        st.insert_user(&user).unwrap();
        let bot = Bot {
            id: BotId::new(),
            user_id: user.id,
            name: "b".into(),
            config: json!({"model": "x"}),
            created_at_ms: 2,
        };
        st.insert_bot(&bot).unwrap();
        let chat = Chat {
            id: ChatId::new(),
            user_id: user.id,
            bot_id: bot.id,
            title: "t".into(),
            created_at_ms: 3,
        };
        st.insert_chat(&chat).unwrap();
        Fx {
            st,
            user,
            bot,
            chat,
        }
    }

    fn run(f: &Fx) -> Run {
        let r = Run {
            id: RunId::new(),
            user_id: f.user.id,
            bot_id: f.bot.id,
            chat_id: Some(f.chat.id),
            kind: "turn".into(),
            state: "queued".into(),
            error: None,
            created_at_ms: 4,
            updated_at_ms: 4,
            parent_run_id: None,
            spec: None,
            computer_target: None,
            computer_node: None,
            computer_node_name: None,
            paused_at_ms: None,
            pause_reason: None,
        };
        f.st.insert_run(&r).unwrap();
        r
    }

    #[test]
    fn user_bot_chat_message_round_trip() {
        let f = fx();
        assert_eq!(f.st.get_user(f.user.id).unwrap().unwrap(), f.user);
        assert_eq!(f.st.list_users().unwrap(), vec![f.user.clone()]);
        assert_eq!(f.st.get_bot(f.bot.id).unwrap().unwrap(), f.bot);
        assert_eq!(f.st.list_bots(f.user.id).unwrap(), vec![f.bot.clone()]);
        assert_eq!(f.st.get_chat(f.chat.id).unwrap().unwrap(), f.chat);
        assert_eq!(f.st.list_chats(f.user.id).unwrap(), vec![f.chat.clone()]);
        let r = run(&f);
        let m = Message {
            id: Uuid::now_v7(),
            user_id: f.user.id,
            chat_id: f.chat.id,
            run_id: Some(r.id),
            role: "user".into(),
            content: "hi".into(),
            created_at_ms: 5,
        };
        f.st.insert_message(&m).unwrap();
        assert_eq!(f.st.get_message(m.id).unwrap().unwrap(), m);
        assert_eq!(f.st.list_messages(f.chat.id).unwrap(), vec![m]);
        assert!(f.st.get_user(UserId::new()).unwrap().is_none());
    }

    #[test]
    fn user_isolation_in_lists() {
        let f = fx();
        let other = User {
            id: UserId::new(),
            name: "o".into(),
            created_at_ms: 1,
        };
        f.st.insert_user(&other).unwrap();
        assert_eq!(f.st.list_bots(other.id).unwrap().len(), 0);
        assert_eq!(f.st.list_chats(other.id).unwrap().len(), 0);
        run(&f);
        assert_eq!(f.st.list_runs(other.id).unwrap().len(), 0);
    }

    #[test]
    fn bounded_messages_keep_latest_rows_and_filter_run_before_limiting() {
        let f = fx();
        let excluded = run(&f);
        for i in 0..10u32 {
            f.st.insert_message(&Message {
                id: Uuid::from_u128(u128::from(i) + 1),
                user_id: f.user.id,
                chat_id: f.chat.id,
                run_id: (i >= 6).then_some(excluded.id),
                role: "user".into(),
                content: format!("message {i}"),
                // Equal timestamps exercise the deterministic id tie-break.
                created_at_ms: 10,
            })
            .unwrap();
        }
        let (messages, omitted) =
            f.st.list_messages_bounded(f.chat.id, Some(excluded.id), 3, 100)
                .unwrap();
        assert!(omitted);
        assert_eq!(
            messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            ["message 3", "message 4", "message 5"]
        );
        let (all, omitted) =
            f.st.list_messages_bounded(f.chat.id, Some(excluded.id), 6, 100)
                .unwrap();
        assert_eq!(all.len(), 6);
        assert!(!omitted, "exact row boundary is not truncation");
        assert_eq!(
            f.st.list_messages_bounded(ChatId::new(), None, 3, 100)
                .unwrap(),
            (vec![], false)
        );
    }

    #[test]
    fn bounded_messages_count_utf8_bytes_without_splitting_text() {
        let f = fx();
        for i in 0..3u32 {
            f.st.insert_message(&Message {
                id: Uuid::from_u128(u128::from(i) + 1),
                user_id: f.user.id,
                chat_id: f.chat.id,
                run_id: None,
                role: "user".into(),
                content: "é🙂".into(),
                created_at_ms: i64::from(i),
            })
            .unwrap();
        }
        let (messages, omitted) = f.st.list_messages_bounded(f.chat.id, None, 10, 12).unwrap();
        assert!(omitted);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages.iter().map(|m| m.content.len()).sum::<usize>(), 12);
        assert!(messages.iter().all(|m| m.content == "é🙂"));
        assert_eq!(
            f.st.list_messages_bounded(f.chat.id, None, 10, 5).unwrap(),
            (vec![], true)
        );
        assert_eq!(
            f.st.list_messages_bounded(f.chat.id, None, 0, 100).unwrap(),
            (vec![], true)
        );
        assert_eq!(
            f.st.list_messages_bounded(f.chat.id, None, 10, 0).unwrap(),
            (vec![], true)
        );
    }

    #[test]
    fn foreign_keys_enforced() {
        let f = fx();
        let bad = Bot {
            id: BotId::new(),
            user_id: UserId::new(),
            name: "x".into(),
            config: json!({}),
            created_at_ms: 0,
        };
        assert!(f.st.insert_bot(&bad).is_err());
    }

    #[test]
    fn update_and_delete_bot() {
        let f = fx();
        assert!(
            f.st.update_bot(f.bot.id, "renamed", &json!({"a": 1}))
                .unwrap()
        );
        let got = f.st.get_bot(f.bot.id).unwrap().unwrap();
        assert_eq!(
            (got.name.as_str(), got.config),
            ("renamed", json!({"a": 1}))
        );
        assert!(!f.st.update_bot(BotId::new(), "x", &json!({})).unwrap());
        // The fixture chat still references the bot.
        assert!(f.st.delete_bot(f.bot.id).is_err());
        let spare = Bot {
            id: BotId::new(),
            user_id: f.user.id,
            name: "spare".into(),
            config: json!({}),
            created_at_ms: 3,
        };
        f.st.insert_bot(&spare).unwrap();
        assert!(f.st.delete_bot(spare.id).unwrap());
        assert!(!f.st.delete_bot(spare.id).unwrap());
    }

    #[test]
    fn child_runs_persist_parent_and_spec() {
        let f = fx();
        let parent = run(&f);
        let mut child = run(&f);
        // `run` already inserted a copy; build a second, linked one.
        child.id = RunId::new();
        child.parent_run_id = Some(parent.id);
        child.spec = Some(serde_json::json!({ "task": "t", "depth": 1 }));
        f.st.insert_run(&child).unwrap();
        assert_eq!(f.st.get_run(child.id).unwrap().unwrap(), child);
        assert_eq!(f.st.list_child_runs(parent.id).unwrap(), vec![child]);
        assert!(
            f.st.get_run(parent.id)
                .unwrap()
                .unwrap()
                .parent_run_id
                .is_none()
        );
    }

    #[test]
    fn run_state_and_steps() {
        let f = fx();
        let r = run(&f);
        f.st.update_run_state(r.id, "failed", Some("boom")).unwrap();
        let got = f.st.get_run(r.id).unwrap().unwrap();
        assert_eq!(
            (got.state.as_str(), got.error.as_deref()),
            ("failed", Some("boom"))
        );
        assert_eq!(f.st.list_runs_by_state("failed").unwrap().len(), 1);
        assert_eq!(f.st.list_runs_by_state("queued").unwrap().len(), 0);
        assert!(matches!(
            f.st.update_run_state(RunId::new(), "x", None),
            Err(StorageError::NotFound(_))
        ));

        let s = RunStep {
            id: Uuid::now_v7(),
            run_id: r.id,
            user_id: f.user.id,
            idx: 0,
            name: "s".into(),
            input: json!({"a": 1}),
            idempotent: true,
            retry: Some(json!({"max_attempts": 3})),
            status: "intent".into(),
            attempt: 1,
            output: None,
            error: None,
            started_at_ms: 6,
            finished_at_ms: None,
        };
        f.st.insert_step(&s).unwrap();
        assert_eq!(f.st.get_step(s.id).unwrap().unwrap(), s);
        let dup = RunStep {
            id: Uuid::now_v7(),
            ..s.clone()
        };
        assert!(f.st.insert_step(&dup).is_err(), "(run_id, idx) is unique");
        f.st.update_step(s.id, "succeeded", 1, Some(&json!("ok")), None, true)
            .unwrap();
        let got = f.st.list_steps(r.id).unwrap().remove(0);
        assert_eq!(got.status, "succeeded");
        assert_eq!(got.output, Some(json!("ok")));
        assert!(got.finished_at_ms.is_some());
        assert_eq!(f.st.relabel_steps(r.id, "succeeded", "x").unwrap(), 1);
    }

    #[test]
    fn rewrite_step_json_changes_only_the_json_columns() {
        let f = fx();
        let r = run(&f);
        let s = RunStep {
            id: Uuid::now_v7(),
            run_id: r.id,
            user_id: f.user.id,
            idx: 0,
            name: "s".into(),
            input: json!({"a": "raw"}),
            idempotent: false,
            retry: None,
            status: "succeeded".into(),
            attempt: 2,
            output: Some(json!({"b": "raw"})),
            error: Some("e".into()),
            started_at_ms: 6,
            finished_at_ms: Some(9),
        };
        f.st.insert_step(&s).unwrap();
        f.st.rewrite_step_json(s.id, Some(&json!({"a": "x"})), Some(&json!({"b": "y"})))
            .unwrap();
        let got = f.st.get_step(s.id).unwrap().unwrap();
        assert_eq!(got.input, json!({"a": "x"}));
        assert_eq!(got.output, Some(json!({"b": "y"})));
        assert_eq!(
            (
                got.status.as_str(),
                got.attempt,
                got.error.as_deref(),
                got.finished_at_ms
            ),
            ("succeeded", 2, Some("e"), Some(9))
        );
        assert!(matches!(
            f.st.rewrite_step_json(Uuid::now_v7(), Some(&json!(1)), None),
            Err(StorageError::NotFound(_))
        ));
    }

    #[test]
    fn approvals_decide_once() {
        let f = fx();
        let r = run(&f);
        let a = Approval {
            id: Uuid::now_v7(),
            user_id: f.user.id,
            run_id: r.id,
            step_id: None,
            summary: "send mail".into(),
            decision: None,
            requested_at_ms: 7,
            decided_at_ms: None,
        };
        f.st.insert_approval(&a).unwrap();
        assert_eq!(f.st.get_approval(a.id).unwrap().unwrap(), a);
        f.st.decide_approval(a.id, "approved").unwrap();
        assert!(f.st.decide_approval(a.id, "denied").is_err());
        let got = f.st.list_approvals(r.id).unwrap().remove(0);
        assert_eq!(got.decision.as_deref(), Some("approved"));
        assert!(got.decided_at_ms.is_some());
    }

    #[test]
    fn audit_log_is_append_only() {
        let f = fx();
        let id =
            f.st.append_audit(f.user.id, None, "system", "boot", &json!({"k": 1}))
                .unwrap();
        let entries = f.st.list_audit(f.user.id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].detail, json!({"k": 1}));
        assert!(
            f.st.raw_execute("UPDATE audit_log SET actor = 'x'")
                .is_err()
        );
        assert!(f.st.raw_execute("DELETE FROM audit_log").is_err());
        assert_eq!(f.st.list_audit(f.user.id).unwrap().len(), 1);
    }

    #[test]
    fn event_index_orders_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (RunId::new(), RunId::new());
        {
            let st = Storage::open(dir.path()).unwrap();
            assert_eq!(st.last_gseq().unwrap(), 0);
            st.index_event(1, a, 0).unwrap();
            st.index_event(2, b, 0).unwrap();
            st.index_event(3, a, 1).unwrap();
            assert!(st.index_event(3, b, 1).is_err(), "gseq is never reused");
        }
        let st = Storage::open(dir.path()).unwrap();
        assert_eq!(st.last_gseq().unwrap(), 3);
        let rows = st.list_event_index(1, 10).unwrap();
        let got: Vec<_> = rows.iter().map(|r| (r.gseq, r.run_id, r.seq)).collect();
        assert_eq!(got, [(2, b, 0), (3, a, 1)]);
        assert_eq!(st.list_event_index(0, 2).unwrap().len(), 2);
        assert_eq!(st.list_event_index(3, 10).unwrap(), vec![]);
    }

    #[test]
    fn file_db_uses_wal_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let user = User {
            id: UserId::new(),
            name: "u".into(),
            created_at_ms: 1,
        };
        {
            let st = Storage::open(dir.path()).unwrap();
            let mode: String = st
                .conn()
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
            st.insert_user(&user).unwrap();
        }
        let st = Storage::open(dir.path()).unwrap();
        assert_eq!(st.get_user(user.id).unwrap().unwrap(), user);
    }

    #[test]
    fn bot_skill_assignment_migration_and_crud() {
        let dir = tempfile::tempdir().unwrap();
        let user = User {
            id: UserId::new(),
            name: "u".into(),
            created_at_ms: 1,
        };
        let existing_bot = Bot {
            id: BotId::new(),
            user_id: user.id,
            name: "preexisting".into(),
            config: json!({}),
            created_at_ms: 2,
        };
        // Simulate a pre-existing v3 database with an existing bot.
        {
            let conn = Connection::open(dir.path().join("bot.db")).unwrap();
            conn.execute_batch(include_str!("../migrations/0001_init.sql"))
                .unwrap();
            conn.execute_batch(include_str!("../migrations/0002_event_index.sql"))
                .unwrap();
            conn.execute_batch(include_str!("../migrations/0003_subagents.sql"))
                .unwrap();
            conn.execute_batch("PRAGMA user_version = 3").unwrap();
            conn.execute(
                "INSERT INTO users (id, name, created_at_ms) VALUES (?1, ?2, ?3)",
                params![user.id.to_string(), user.name, user.created_at_ms],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO bots (id, user_id, name, config_json, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    existing_bot.id.to_string(),
                    existing_bot.user_id.to_string(),
                    existing_bot.name,
                    "{}",
                    existing_bot.created_at_ms
                ],
            )
            .unwrap();
        }
        let st = Storage::open(dir.path()).unwrap();
        assert_eq!(
            migrate::current_version(&st.conn()).unwrap(),
            migrate::latest_version()
        );
        // Existing bot has no row => defaults to all.
        assert_eq!(st.get_bot_skill_assignment(existing_bot.id).unwrap(), None);

        // Set selected (deduplicates and sorts).
        let saved = st
            .set_bot_skill_assignment_audited(
                user.id,
                existing_bot.id,
                SkillAssignmentMode::Selected,
                &["zeta".into(), "alpha".into(), "zeta".into()],
                "user",
                &json!({"bot_id": existing_bot.id.to_string(), "mode": "selected"}),
            )
            .unwrap();
        assert_eq!(saved.mode, SkillAssignmentMode::Selected);
        assert_eq!(saved.names, vec!["alpha", "zeta"]);
        assert_eq!(
            st.get_bot_skill_assignment(existing_bot.id)
                .unwrap()
                .unwrap()
                .names,
            vec!["alpha", "zeta"]
        );
        assert_eq!(st.list_audit(user.id).unwrap().len(), 1);

        // If the bot has a chat, delete_bot fails and rolls back the skill-assignment delete too.
        let chat = Chat {
            id: ChatId::new(),
            user_id: user.id,
            bot_id: existing_bot.id,
            title: "c".into(),
            created_at_ms: 3,
        };
        st.insert_chat(&chat).unwrap();
        assert!(st.delete_bot(existing_bot.id).is_err());
        assert!(
            st.get_bot_skill_assignment(existing_bot.id)
                .unwrap()
                .is_some()
        );

        // Setting mode = All deletes the row.
        st.set_bot_skill_assignment(existing_bot.id, SkillAssignmentMode::All, &[])
            .unwrap();
        assert_eq!(st.get_bot_skill_assignment(existing_bot.id).unwrap(), None);

        // Deleting a bot with a skill assignment removes the assignment row atomically.
        let spare = Bot {
            id: BotId::new(),
            user_id: user.id,
            name: "spare".into(),
            config: json!({}),
            created_at_ms: 4,
        };
        st.insert_bot(&spare).unwrap();
        st.set_bot_skill_assignment(spare.id, SkillAssignmentMode::Selected, &["alpha".into()])
            .unwrap();
        assert!(st.delete_bot(spare.id).unwrap());
        assert_eq!(st.get_bot_skill_assignment(spare.id).unwrap(), None);
        assert!(!st.delete_bot_skill_assignment(spare.id).unwrap());
    }

    #[test]
    fn bot_skill_assignment_generation_tracks_db_writes_and_version_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let st = Storage::open(dir.path()).unwrap();
        let user = User {
            id: UserId::new(),
            name: "u".into(),
            created_at_ms: 1,
        };
        let bot = Bot {
            id: BotId::new(),
            user_id: user.id,
            name: "b".into(),
            config: json!({}),
            created_at_ms: 2,
        };
        st.insert_user(&user).unwrap();
        st.insert_bot(&bot).unwrap();

        let g0 = st.bot_skill_assignment_generation().unwrap();
        assert_eq!(g0, 1);

        // First write from expected_version = 0 (no row) succeeds and bumps DB generation.
        let r1 = st
            .set_bot_skill_assignment_audited_versioned(
                user.id,
                bot.id,
                SkillAssignmentMode::Selected,
                &["alpha".into()],
                "user",
                &json!({}),
                Some(0),
            )
            .unwrap();
        let g1 = st.bot_skill_assignment_generation().unwrap();
        assert_eq!(r1.version, g1);
        assert!(g1 > g0);

        // Stale expected_version (0 instead of r1.version) returns Conflict without modifying row.
        let err = st
            .set_bot_skill_assignment_audited_versioned(
                user.id,
                bot.id,
                SkillAssignmentMode::Selected,
                &["beta".into()],
                "user",
                &json!({}),
                Some(0),
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Conflict(_)), "{err:?}");
        assert_eq!(
            st.get_bot_skill_assignment(bot.id).unwrap().unwrap().names,
            vec!["alpha"]
        );

        // External SQLite writer on the same DB file bumps generation via trigger.
        let external = Storage::open(dir.path()).unwrap();
        external
            .set_bot_skill_assignment(bot.id, SkillAssignmentMode::Selected, &["gamma".into()])
            .unwrap();
        let g2 = st.bot_skill_assignment_generation().unwrap();
        assert!(g2 > g1);

        // Corrupt row: get_bot_skill_assignment fails, inspect_bot_skill_assignment reports Corrupt
        // with raw version so management UI can overwrite and repair it.
        st.conn()
            .execute(
                "UPDATE bot_skill_assignment SET names_json = 'not-json' WHERE bot_id = ?1",
                [bot.id.to_string()],
            )
            .unwrap();
        assert!(st.get_bot_skill_assignment(bot.id).is_err());
        let inspected = st.inspect_bot_skill_assignment(bot.id).unwrap();
        match inspected {
            SkillAssignmentLookup::Corrupt { version, .. } => {
                assert_eq!(version, g2);
                let repaired = st
                    .set_bot_skill_assignment_audited_versioned(
                        user.id,
                        bot.id,
                        SkillAssignmentMode::All,
                        &[],
                        "user",
                        &json!({"repaired": true}),
                        Some(version),
                    )
                    .unwrap();
                assert_eq!(repaired.version, 0);
            }
            other @ SkillAssignmentLookup::Ok(_) => panic!("expected Corrupt, got {other:?}"),
        }
        assert_eq!(st.get_bot_skill_assignment(bot.id).unwrap(), None);
        assert!(st.bot_skill_assignment_generation().unwrap() > g2);
    }

    #[test]
    fn assignment_audit_diff_uses_current_database_state() {
        let dir = tempfile::tempdir().unwrap();
        let st = Storage::open(dir.path()).unwrap();
        let user = User {
            id: UserId::new(),
            name: "u".into(),
            created_at_ms: 1,
        };
        let bot = Bot {
            id: BotId::new(),
            user_id: user.id,
            name: "b".into(),
            config: json!({}),
            created_at_ms: 1,
        };
        st.insert_user(&user).unwrap();
        st.insert_bot(&bot).unwrap();
        st.set_bot_skill_assignment(bot.id, SkillAssignmentMode::Selected, &["stale".into()])
            .unwrap();
        let external = Storage::open(dir.path()).unwrap();
        external
            .set_bot_skill_assignment(
                bot.id,
                SkillAssignmentMode::Selected,
                &["current".into(), "keep".into()],
            )
            .unwrap();
        st.set_bot_skill_assignment_audited_versioned(
            user.id,
            bot.id,
            SkillAssignmentMode::Selected,
            &["next".into(), "keep".into(), "next".into()],
            "user",
            &json!({"added": ["stale"], "removed": [], "custom": true}),
            None,
        )
        .unwrap();
        let audit = st.list_audit(user.id).unwrap().pop().unwrap();
        assert_eq!(audit.detail["added"], json!(["next"]));
        assert_eq!(audit.detail["removed"], json!(["current"]));
        assert_eq!(audit.detail["custom"], true);
        st.set_bot_skill_assignment_audited_versioned(
            user.id,
            bot.id,
            SkillAssignmentMode::All,
            &[],
            "user",
            &json!({}),
            None,
        )
        .unwrap();
        let audit = st.list_audit(user.id).unwrap().pop().unwrap();
        assert_eq!(audit.detail["added"], json!([]));
        assert_eq!(audit.detail["removed"], json!(["keep", "next"]));
    }

    #[test]
    fn computer_target_and_pause_reason_persist_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let run_id;
        {
            let st = Storage::open(dir.path()).unwrap();
            let u = User {
                id: UserId::new(),
                name: "u".into(),
                created_at_ms: 1,
            };
            st.insert_user(&u).unwrap();
            let b = Bot {
                id: BotId::new(),
                user_id: u.id,
                name: "b".into(),
                config: json!({}),
                created_at_ms: 1,
            };
            st.insert_bot(&b).unwrap();
            let r = Run {
                id: RunId::new(),
                user_id: u.id,
                bot_id: b.id,
                chat_id: None,
                kind: "turn".into(),
                state: "queued".into(),
                error: None,
                created_at_ms: 2,
                updated_at_ms: 2,
                parent_run_id: None,
                spec: None,
                computer_target: None,
                computer_node: None,
                computer_node_name: None,
                paused_at_ms: None,
                pause_reason: None,
            };
            run_id = r.id;
            st.insert_run(&r).unwrap();
            st.bind_run_computer(run_id, "pc", Some("pc-A"), Some("My PC"))
                .unwrap();
            st.update_run_state(run_id, "paused", None).unwrap();
            st.set_run_pause_reason(run_id, Some("waiting_for_pc"))
                .unwrap();
            assert_eq!(
                st.list_paused_runs_with_reason("waiting_for_pc")
                    .unwrap()
                    .len(),
                1
            );
        }
        let st = Storage::open(dir.path()).unwrap();
        let got = st.get_run(run_id).unwrap().unwrap();
        assert_eq!(got.computer_target.as_deref(), Some("pc"));
        assert_eq!(got.pause_reason.as_deref(), Some("waiting_for_pc"));
        assert_eq!(got.computer_node.as_deref(), Some("pc-A"));
        assert_eq!(got.computer_node_name.as_deref(), Some("My PC"));
        assert!(
            st.bind_run_computer(run_id, "pc", Some("pc-B"), Some("Other PC"))
                .is_err()
        );
        assert_eq!(
            st.get_run(run_id)
                .unwrap()
                .unwrap()
                .computer_node
                .as_deref(),
            Some("pc-A")
        );
        assert!(got.paused_at_ms.is_some());
        st.conn()
            .execute(
                "UPDATE runs SET paused_at_ms = 123 WHERE id = ?1",
                [run_id.to_string()],
            )
            .unwrap();
        st.set_run_computer_target(run_id, Some("pc")).unwrap();
        st.set_run_pause_reason(run_id, Some("waiting_for_pc"))
            .unwrap();
        st.update_run_state(run_id, "paused", None).unwrap();
        assert_eq!(st.get_run(run_id).unwrap().unwrap().paused_at_ms, Some(123));
        st.update_run_state(run_id, "running", None).unwrap();
        let running = st.get_run(run_id).unwrap().unwrap();
        assert_eq!(running.pause_reason, None);
        assert_eq!(running.paused_at_ms, None);
    }
}
