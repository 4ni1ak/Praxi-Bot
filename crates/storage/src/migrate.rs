//! Forward-only versioned migration runner with embedded SQL.

use rusqlite::Connection;

use crate::error::{Result, StorageError};

/// Embedded migrations, ordered by their explicit numeric prefix.
/// 0007 belongs to PR #89; 0008..0012 are reserved by other open drafts.
/// The ledger allows those migrations to be integrated later without being
/// skipped just because this build has already applied 0013.
const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    (
        "0002_event_index",
        include_str!("../migrations/0002_event_index.sql"),
    ),
    (
        "0003_subagents",
        include_str!("../migrations/0003_subagents.sql"),
    ),
    (
        "0004_computer_target",
        include_str!("../migrations/0004_computer_target.sql"),
    ),
    (
        "0005_computer_identity",
        include_str!("../migrations/0005_computer_identity.sql"),
    ),
    (
        "0006_bot_skill_assignment",
        include_str!("../migrations/0006_bot_skill_assignment.sql"),
    ),
    (
        "0013_bot_version",
        include_str!("../migrations/0013_bot_version.sql"),
    ),
];

/// Highest schema version this build supports.
///
/// # Panics
/// Panics if the embedded migration list is empty or its last numeric prefix is invalid.
#[must_use]
pub fn latest_version() -> u32 {
    migration_version(MIGRATIONS.last().expect("migrations exist").0)
}

/// Current schema version of the database (0 for an empty DB).
///
/// # Errors
/// Returns an error on SQLite failure.
pub fn current_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Apply all pending migrations, each in its own transaction.  Returns the
/// number applied.
///
/// # Errors
/// Fails on SQL errors or if the DB is newer than this build.
pub fn migrate(conn: &mut Connection) -> Result<u32> {
    migrate_list(conn, MIGRATIONS)
}

fn migration_version(name: &str) -> u32 {
    name.split('_')
        .next()
        .expect("migration prefix")
        .parse()
        .expect("numeric migration prefix")
}

fn migrate_list(conn: &mut Connection, migrations: &[(&str, &str)]) -> Result<u32> {
    let current = current_version(conn)?;
    let latest = migration_version(migrations.last().expect("migrations exist").0);
    if current > latest {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    // Bootstrap once from legacy contiguous user_version. After this, record
    // actual applied IDs, never fill holes using the high-water mark.
    {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let has_ledger: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='storage_migrations')",
            [], |r| r.get(0),
        )?;
        if !has_ledger {
            let legacy = current_version(&tx)?;
            if legacy >= 13 {
                return Err(StorageError::Conflict(
                    "sparse schema is missing its migration ledger".into(),
                ));
            }
            tx.execute_batch("CREATE TABLE storage_migrations (version INTEGER PRIMARY KEY)")?;
            for version in 1..=legacy {
                tx.execute(
                    "INSERT INTO storage_migrations (version) VALUES (?1)",
                    [version],
                )?;
            }
        }
        tx.commit()?;
    }
    let mut applied = 0;
    for (name, sql) in migrations {
        let version = migration_version(name);
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM storage_migrations WHERE version=?1)",
            [version],
            |r| r.get(0),
        )?;
        if exists {
            continue;
        }
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO storage_migrations (version) VALUES (?1)",
            [version],
        )?;
        tx.pragma_update(None, "user_version", current_version(&tx)?.max(version))?;
        tx.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(std::result::Result::unwrap)
            .collect()
    }

    #[test]
    fn migrates_empty_db_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(current_version(&conn).unwrap(), 0);
        assert_eq!(latest_version(), 13);
        assert_eq!(migrate(&mut conn).unwrap(), 7);
        assert_eq!(current_version(&conn).unwrap(), 13);
        assert_eq!(migrate(&mut conn).unwrap(), 0);
        assert_eq!(
            tables(&conn),
            [
                "approvals",
                "audit_log",
                "bot_skill_assignment",
                "bot_skill_assignment_state",
                "bots",
                "chats",
                "event_index",
                "messages",
                "run_steps",
                "runs",
                "storage_migrations",
                "users"
            ]
        );
    }

    #[test]
    fn pre_058_paused_run_survives_upgrade_with_null_target_and_pause_clock() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (_, sql) in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 3;
            INSERT INTO users (id,name,created_at_ms) VALUES ('user','u',100);
            INSERT INTO bots (id,user_id,name,created_at_ms) VALUES ('bot','user','b',100);
            INSERT INTO runs (id,user_id,bot_id,kind,state,created_at_ms,updated_at_ms)
            VALUES ('run','user','bot','turn','paused',100,200);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let row: (String, Option<String>, Option<String>, i64) = conn
            .query_row(
                "SELECT state,computer_target,computer_node,paused_at_ms FROM runs WHERE id='run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, ("paused".into(), None, None, 200));
    }

    #[test]
    fn main_v5_upgrades_by_the_skill_assignment_and_bot_version_migrations() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (_, sql) in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 5;
            INSERT INTO users (id,name,created_at_ms) VALUES ('user','u',100);
            INSERT INTO bots (id,user_id,name,created_at_ms) VALUES ('bot','user','b',100);
            INSERT INTO runs (id,user_id,bot_id,kind,state,created_at_ms,updated_at_ms,computer_target,computer_node)
            VALUES ('run','user','bot','turn','paused',100,200,'pc','pc-A');").unwrap();
        assert!(
            !tables(&conn)
                .iter()
                .any(|t| t.starts_with("bot_skill_assignment"))
        );
        assert_eq!(migrate(&mut conn).unwrap(), 2);
        assert_eq!(current_version(&conn).unwrap(), 13);
        // An existing bot starts at version 1.
        let v: i64 = conn
            .query_row("SELECT version FROM bots WHERE id='bot'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 1);
        assert_eq!(migrate(&mut conn).unwrap(), 0);
        for table in ["bot_skill_assignment", "bot_skill_assignment_state"] {
            assert!(tables(&conn).contains(&table.to_owned()));
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_skill_assignment", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0); // Existing bots retain the all-skills default.
        let target: (String, String) = conn
            .query_row(
                "SELECT computer_target,computer_node FROM runs WHERE id='run'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(target, ("pc".into(), "pc-A".into()));
        let generation: i64 = conn
            .query_row(
                "SELECT generation FROM bot_skill_assignment_state WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(generation, 1);
        let triggers: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='bot_skill_assignment'", [], |r| r.get(0)).unwrap();
        assert_eq!(triggers, 6);
    }

    #[test]
    fn review95_reserved_migrations_upgrade_v6_and_pr89_v7_without_skipping_later_integration() {
        for legacy in [6, 7] {
            let mut conn = Connection::open_in_memory().unwrap();
            for (_, sql) in &MIGRATIONS[..6] {
                conn.execute_batch(sql).unwrap();
            }
            conn.execute_batch("INSERT INTO users VALUES ('u','owner',1); INSERT INTO bots (id,user_id,name,created_at_ms) VALUES ('b','u','kept',2)").unwrap();
            // Exact additive schema from PR #89's reserved 0007_message_exposure.
            let exposure =
                "ALTER TABLE messages ADD COLUMN exposure_untrusted INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE messages ADD COLUMN exposure_private INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE messages ADD COLUMN exposure_sources TEXT NOT NULL DEFAULT '[]';";
            if legacy == 7 {
                conn.execute_batch(exposure).unwrap();
            }
            conn.pragma_update(None, "user_version", legacy).unwrap();
            assert_eq!(migrate(&mut conn).unwrap(), 1);
            assert_eq!(current_version(&conn).unwrap(), 13);
            let kept: (String, i64) = conn
                .query_row("SELECT name,version FROM bots WHERE id='b'", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(kept, ("kept".into(), 1));
            // Simulate incorporating #89 after this PR. v6 must run it; v7 must skip it.
            let mut integrated = MIGRATIONS[..6].to_vec();
            integrated.push(("0007_message_exposure", exposure));
            integrated.push(MIGRATIONS[6]);
            assert_eq!(
                migrate_list(&mut conn, &integrated).unwrap(),
                u32::from(legacy == 6)
            );
            assert_eq!(migrate_list(&mut conn, &integrated).unwrap(), 0);
            conn.prepare(
                "SELECT exposure_untrusted,exposure_private,exposure_sources FROM messages",
            )
            .unwrap();
            assert_eq!(current_version(&conn).unwrap(), 13);
        }
    }

    #[test]
    fn rejects_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999").unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(StorageError::SchemaTooNew { found: 999, .. })
        ));
    }
}
