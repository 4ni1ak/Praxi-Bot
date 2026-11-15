//! Host-wide durable local budgets. This file is never exported in a state bundle.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{MemoryError, Result};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
const SMALL_CLOCK_ROLLBACK_MS: i64 = 300_000;
const MAX_COOLDOWN_MS: i64 = 2 * DAY_MS;

/// Local accounting policy, not a claim about a vendor's quota units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestUnit {
    /// One reservation per actual HTTP attempt.
    HttpRequest,
    /// Conservatively count each input in a batched HTTP attempt.
    #[default]
    Input,
}

/// Configurable account budget shared by all bots on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmbeddingBudget {
    pub requests_per_minute: u32,
    /// A rolling 24-hour local cap. Vendor daily reset rules can be different.
    pub requests_per_day: u32,
    /// Conservative UTF-8 input bytes are reserved as token units before send.
    pub tokens_per_minute: Option<u64>,
    pub request_unit: RequestUnit,
}

impl Default for EmbeddingBudget {
    fn default() -> Self {
        Self {
            requests_per_minute: 100,
            requests_per_day: 1000,
            tokens_per_minute: None,
            request_unit: RequestUnit::Input,
        }
    }
}

impl EmbeddingBudget {
    pub(super) fn validate(&self) -> Result<()> {
        if self.requests_per_minute == 0
            || self.requests_per_minute > 1_000_000
            || self.requests_per_day == 0
            || self.requests_per_day > 10_000_000
            || self
                .tokens_per_minute
                .is_some_and(|tokens| tokens == 0 || tokens > 1_000_000_000)
        {
            return Err(MemoryError::Invalid("invalid embedding budget".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reservation {
    Granted,
    Deferred(i64),
}

/// Persistent, transactional request reservations and provider cooldowns.
/// Cancellation/failure never refunds a reservation: a server may have received
/// and billed the request even if this process could not read its response.
#[derive(Debug)]
pub struct EmbeddingRuntime {
    connection: Mutex<Connection>,
}

impl EmbeddingRuntime {
    /// Open `<data_dir>/knowledge-runtime.db`, shared by bots, not other hosts.
    ///
    /// # Errors
    /// Filesystem/SQLite errors or a symlink at the runtime database location.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(data_dir.as_ref())?;
        let path = data_dir.as_ref().join("knowledge-runtime.db");
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(MemoryError::Invalid(
                "knowledge runtime database cannot be a symlink".into(),
            ));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Self::init(connection)
    }

    /// Ephemeral ledger for hermetic tests.
    ///
    /// # Errors
    /// SQLite initialization errors.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS embedding_runtime_meta(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
             INSERT OR IGNORE INTO embedding_runtime_meta(key,value) VALUES('clock_ms',0);
             CREATE TABLE IF NOT EXISTS embedding_quota(
                id INTEGER PRIMARY KEY,account_group TEXT NOT NULL,reserved_ms INTEGER NOT NULL,
                request_units INTEGER NOT NULL CHECK(request_units>0),token_units INTEGER NOT NULL CHECK(token_units>=0));
             CREATE INDEX IF NOT EXISTS embedding_quota_group_time ON embedding_quota(account_group,reserved_ms);
             CREATE TABLE IF NOT EXISTS embedding_cooldown(account_group TEXT PRIMARY KEY,not_before_ms INTEGER NOT NULL);"
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub(super) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn reserve(
        &self,
        account: &str,
        budget: &EmbeddingBudget,
        inputs: usize,
        token_units: u64,
        now: i64,
    ) -> Result<Reservation> {
        budget.validate()?;
        if account.trim().is_empty()
            || account.len() > 256
            || account.chars().any(char::is_control)
            || inputs == 0
        {
            return Err(MemoryError::Invalid(
                "invalid embedding quota account or batch".into(),
            ));
        }
        let units = match budget.request_unit {
            RequestUnit::HttpRequest => 1,
            RequestUnit::Input => u32::try_from(inputs)
                .map_err(|_| MemoryError::Invalid("embedding batch is too large".into()))?,
        };
        if units > budget.requests_per_minute
            || units > budget.requests_per_day
            || budget
                .tokens_per_minute
                .is_some_and(|limit| token_units > limit)
            || token_units > i64::MAX as u64
        {
            return Err(MemoryError::Invalid(
                "embedding batch exceeds the configured account budget".into(),
            ));
        }
        let mut connection = self.conn();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: i64 = tx.query_row(
            "SELECT value FROM embedding_runtime_meta WHERE key='clock_ms'",
            [],
            |r| r.get(0),
        )?;
        // Small clock corrections remain conservative. A bad RTC/NTP jump
        // must not hold quota for a year: translate future reservations back
        // to the corrected clock, keeping their charge for at most one day.
        let now = if previous.saturating_sub(now) > SMALL_CLOCK_ROLLBACK_MS {
            let correction = previous.saturating_sub(now);
            tx.execute("UPDATE embedding_quota SET reserved_ms=MAX(?1,reserved_ms-?2) WHERE reserved_ms>?1",params![now,correction])?;
            tx.execute("UPDATE embedding_cooldown SET not_before_ms=MIN(?1,MAX(?2,not_before_ms-?3)) WHERE not_before_ms>?2",params![now.saturating_add(MAX_COOLDOWN_MS),now,correction])?;
            eprintln!(
                "knowledge embedding clock corrected after a large rollback; quota windows were rebased"
            );
            now
        } else {
            now.max(previous)
        };
        tx.execute(
            "UPDATE embedding_runtime_meta SET value=?1 WHERE key='clock_ms'",
            [now],
        )?;
        tx.execute(
            "DELETE FROM embedding_quota WHERE reserved_ms<=?1",
            [now.saturating_sub(DAY_MS)],
        )?;
        // Repair old unbounded cooldown rows once. Persist the bounded deadline
        // so subsequent reads age toward it instead of sliding it forward.
        tx.execute("UPDATE embedding_cooldown SET not_before_ms=?2 WHERE account_group=?1 AND not_before_ms>?2",params![account,now.saturating_add(MAX_COOLDOWN_MS)])?;
        let cooldown: Option<i64> = tx
            .query_row(
                "SELECT not_before_ms FROM embedding_cooldown WHERE account_group=?1",
                [account],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(deadline) = cooldown.filter(|&deadline| deadline > now) {
            tx.commit()?;
            return Ok(Reservation::Deferred(deadline));
        }
        let (minute_units, minute_tokens, oldest_minute): (i64,i64,Option<i64>) = tx.query_row(
            "SELECT COALESCE(SUM(request_units),0),COALESCE(SUM(token_units),0),MIN(reserved_ms) FROM embedding_quota WHERE account_group=?1 AND reserved_ms>?2",
            params![account,now.saturating_sub(MINUTE_MS)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let (day_units,oldest_day):(i64,Option<i64>)=tx.query_row(
            "SELECT COALESCE(SUM(request_units),0),MIN(reserved_ms) FROM embedding_quota WHERE account_group=?1",
            [account],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let mut next = now;
        if minute_units.saturating_add(i64::from(units)) > i64::from(budget.requests_per_minute)
            || budget.tokens_per_minute.is_some_and(|limit| {
                minute_tokens.saturating_add(i64::try_from(token_units).unwrap_or(i64::MAX))
                    > i64::try_from(limit).unwrap_or(i64::MAX)
            })
        {
            next = next.max(oldest_minute.unwrap_or(now).saturating_add(MINUTE_MS));
        }
        if day_units.saturating_add(i64::from(units)) > i64::from(budget.requests_per_day) {
            next = next.max(oldest_day.unwrap_or(now).saturating_add(DAY_MS));
        }
        if next > now {
            tx.commit()?;
            return Ok(Reservation::Deferred(next));
        }
        tx.execute("INSERT INTO embedding_quota(account_group,reserved_ms,request_units,token_units) VALUES(?1,?2,?3,?4)",params![account,now,units,i64::try_from(token_units).unwrap_or(i64::MAX)])?;
        tx.commit()?;
        Ok(Reservation::Granted)
    }

    pub(super) fn defer(&self, account: &str, not_before_ms: i64) -> Result<()> {
        let not_before_ms = not_before_ms.min(storage::now_ms().saturating_add(MAX_COOLDOWN_MS));
        self.conn().execute(
            "INSERT INTO embedding_cooldown(account_group,not_before_ms) VALUES(?1,?2) ON CONFLICT(account_group) DO UPDATE SET not_before_ms=MAX(not_before_ms,excluded.not_before_ms)",
            params![account,not_before_ms])?;
        Ok(())
    }

    /// Clear a provider cooldown after explicit owner recovery. Reservations
    /// remain charged, so this cannot reset the account's local quota.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn reset_cooldown(&self, account: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM embedding_cooldown WHERE account_group=?1",
            [account],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_clock_rollback_recovers_within_one_budget_window() {
        let runtime = EmbeddingRuntime::open_in_memory().unwrap();
        let budget = EmbeddingBudget {
            requests_per_day: 1,
            ..EmbeddingBudget::default()
        };
        let now = 1_800_000_000_000;
        let future = now + 366 * DAY_MS;
        assert_eq!(
            runtime.reserve("account", &budget, 1, 1, future).unwrap(),
            Reservation::Granted
        );
        let Reservation::Deferred(deadline) =
            runtime.reserve("account", &budget, 1, 1, now).unwrap()
        else {
            panic!("reservation must remain charged")
        };
        assert!(
            deadline <= now + DAY_MS,
            "clock correction trapped budget until {deadline}"
        );
        assert_eq!(
            runtime
                .reserve("account", &budget, 1, 1, now + DAY_MS)
                .unwrap(),
            Reservation::Granted
        );
    }

    #[test]
    fn impossible_provider_cooldown_is_bounded() {
        let runtime = EmbeddingRuntime::open_in_memory().unwrap();
        let now = storage::now_ms();
        runtime.defer("account", i64::MAX).unwrap();
        let Reservation::Deferred(deadline) = runtime
            .reserve("account", &EmbeddingBudget::default(), 1, 1, now)
            .unwrap()
        else {
            panic!("provider cooldown expected")
        };
        assert!(
            deadline <= now + 2 * DAY_MS + 1000,
            "unbounded provider cooldown"
        );
    }

    #[test]
    fn quota_survives_restart_and_is_shared_between_workers() {
        let dir = tempfile::tempdir().unwrap();
        let budget = EmbeddingBudget {
            requests_per_minute: 2,
            requests_per_day: 3,
            ..EmbeddingBudget::default()
        };
        let first = EmbeddingRuntime::open(dir.path()).unwrap();
        assert_eq!(
            first.reserve("account", &budget, 2, 10, 100_000).unwrap(),
            Reservation::Granted
        );
        let second = EmbeddingRuntime::open(dir.path()).unwrap();
        assert_eq!(
            second.reserve("account", &budget, 1, 10, 100_001).unwrap(),
            Reservation::Deferred(160_000)
        );
        drop(first);
        drop(second);
        let restarted = EmbeddingRuntime::open(dir.path()).unwrap();
        assert_eq!(
            restarted
                .reserve("account", &budget, 1, 10, 160_000)
                .unwrap(),
            Reservation::Granted
        );
        assert_eq!(
            restarted
                .reserve("account", &budget, 1, 10, 220_000)
                .unwrap(),
            Reservation::Deferred(86_500_000)
        );
    }

    #[test]
    fn token_budget_cooldown_and_clock_rollback_are_conservative() {
        let runtime = EmbeddingRuntime::open_in_memory().unwrap();
        let budget = EmbeddingBudget {
            tokens_per_minute: Some(20),
            ..EmbeddingBudget::default()
        };
        assert_eq!(
            runtime.reserve("account", &budget, 1, 20, 100_000).unwrap(),
            Reservation::Granted
        );
        assert_eq!(
            runtime.reserve("account", &budget, 1, 1, 0).unwrap(),
            Reservation::Deferred(160_000)
        );
        runtime.defer("account", 999_999).unwrap();
        assert_eq!(
            runtime.reserve("account", &budget, 1, 1, 200_000).unwrap(),
            Reservation::Deferred(999_999)
        );
        assert_eq!(
            runtime
                .reserve("other-account", &budget, 1, 1, 200_000)
                .unwrap(),
            Reservation::Granted
        );
    }

    #[test]
    fn legacy_poisoned_cooldown_is_persistently_capped_without_sliding() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = EmbeddingRuntime::open(dir.path()).unwrap();
        let now = 1_800_000_000_000;
        let deadline = now + MAX_COOLDOWN_MS;
        runtime
            .conn()
            .execute(
                "INSERT INTO embedding_cooldown(account_group,not_before_ms) VALUES(?1,?2)",
                params!["account", i64::MAX],
            )
            .unwrap();
        let budget = EmbeddingBudget::default();
        assert_eq!(
            runtime.reserve("account", &budget, 1, 1, now).unwrap(),
            Reservation::Deferred(deadline)
        );
        let stored_deadline: i64 = runtime
            .conn()
            .query_row(
                "SELECT not_before_ms FROM embedding_cooldown WHERE account_group='account'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_deadline, deadline);
        drop(runtime);

        let restarted = EmbeddingRuntime::open(dir.path()).unwrap();
        assert_eq!(
            restarted
                .reserve("account", &budget, 1, 1, now + DAY_MS)
                .unwrap(),
            Reservation::Deferred(deadline),
            "reopening or retrying must not move the cap another 48 hours"
        );
        assert_eq!(
            restarted
                .reserve("account", &budget, 1, 1, deadline)
                .unwrap(),
            Reservation::Granted
        );
    }

    #[test]
    fn cooldown_reset_preserves_quota_and_other_accounts() {
        let runtime = EmbeddingRuntime::open_in_memory().unwrap();
        let now = storage::now_ms();
        let budget = EmbeddingBudget {
            requests_per_day: 1,
            ..EmbeddingBudget::default()
        };
        assert_eq!(
            runtime.reserve("account", &budget, 1, 37, now).unwrap(),
            Reservation::Granted
        );
        let deadline = now + 2 * DAY_MS;
        runtime.defer("account", deadline).unwrap();
        runtime.defer("other-account", deadline).unwrap();
        runtime.reset_cooldown("account").unwrap();
        assert_eq!(
            runtime
                .reserve("account", &budget, 1, 1, now + MINUTE_MS)
                .unwrap(),
            Reservation::Deferred(now + DAY_MS),
            "owner recovery clears a provider cooldown, never the paid reservation"
        );
        assert_eq!(
            runtime
                .reserve("other-account", &budget, 1, 1, now + MINUTE_MS)
                .unwrap(),
            Reservation::Deferred(deadline)
        );
        let charge: (i64, i64) = runtime
            .conn()
            .query_row(
                "SELECT SUM(request_units),SUM(token_units) FROM embedding_quota WHERE account_group='account'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(charge, (1, 37));
    }
}
