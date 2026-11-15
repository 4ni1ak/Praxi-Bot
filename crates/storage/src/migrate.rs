//! Forward-only versioned migration runner with embedded SQL.

use rusqlite::Connection;

use crate::error::{Result, StorageError};

/// Embedded migrations, ordered; version = index + 1.
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
        "0007_message_exposure",
        include_str!("../migrations/0007_message_exposure.sql"),
    ),
];

/// Highest schema version this build supports.
#[must_use]
pub fn latest_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
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
    let current = current_version(conn)?;
    let latest = latest_version();
    if current > latest {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    let mut applied = 0;
    for (i, (_name, sql)) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = i + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))?;
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
        assert_eq!(latest_version(), 7);
        assert_eq!(migrate(&mut conn).unwrap(), 7);
        assert_eq!(current_version(&conn).unwrap(), 7);
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
    fn fresh_database_ends_at_version_seven() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(latest_version(), 7);
        assert_eq!(current_version(&conn).unwrap(), 7);
        assert_eq!(MIGRATIONS[3].0, "0004_computer_target");
        assert_eq!(MIGRATIONS[4].0, "0005_computer_identity");
        assert_eq!(MIGRATIONS[5].0, "0006_bot_skill_assignment");
        assert_eq!(MIGRATIONS[6].0, "0007_message_exposure");
    }

    #[test]
    fn version_six_database_upgrades_to_message_exposure() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (i, (_, sql)) in MIGRATIONS[..6].iter().enumerate() {
            conn.execute_batch(sql).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {}", i + 1))
                .unwrap();
        }
        assert_eq!(current_version(&conn).unwrap(), 6);
        assert_eq!(migrate(&mut conn).unwrap(), 1);
        assert_eq!(current_version(&conn).unwrap(), 7);
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('messages')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(std::result::Result::unwrap)
            .collect();
        for c in ["exposure_untrusted", "exposure_private", "exposure_sources"] {
            assert!(cols.iter().any(|x| x == c), "missing column {c}");
        }
    }

    #[test]
    fn main_v5_upgrades_through_the_skill_assignment_and_exposure_migrations() {
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
        assert_eq!(current_version(&conn).unwrap(), 7);
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
        assert_eq!(triggers, 3);
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
