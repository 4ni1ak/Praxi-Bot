//! Per-day counters that survive a restart (requests per day of each backend,
//! the owner's daily budget, calls made / saved today).
//!
//! The day is the local calendar day, so a quota that resets "at midnight"
//! resets at the owner's midnight. The file is a tiny JSON object; every bump
//! rewrites it through a temporary file and a rename. A missing, unreadable or
//! stale file means "nothing counted today" (the limits are then enforced from
//! zero, never skipped).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Source of the current day key (`YYYY-MM-DD`); a test passes its own.
pub type DayClock = dyn Fn() -> String + Send + Sync;

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    day: String,
    #[serde(default)]
    counts: BTreeMap<String, u64>,
}

/// Counters that reset when the local day changes.
pub struct DayCounters {
    path: Option<PathBuf>,
    clock: Box<DayClock>,
    state: Mutex<State>,
}

impl std::fmt::Debug for DayCounters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DayCounters")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Today's local date as `YYYY-MM-DD`.
#[must_use]
pub fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl DayCounters {
    /// Counters stored at `path` (`None`: memory only), on the local clock.
    #[must_use]
    pub fn open(path: Option<PathBuf>) -> Arc<Self> {
        Self::with_clock(path, Box::new(local_day))
    }

    /// [`Self::open`] with an explicit day source.
    #[must_use]
    pub fn with_clock(path: Option<PathBuf>, clock: Box<DayClock>) -> Arc<Self> {
        let today = clock();
        let loaded = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .filter(|s| s.day == today);
        Arc::new(Self {
            path,
            clock,
            state: Mutex::new(loaded.unwrap_or(State {
                day: today,
                counts: BTreeMap::new(),
            })),
        })
    }

    fn roll(&self, st: &mut State) {
        let today = (self.clock)();
        if st.day != today {
            st.day = today;
            st.counts.clear();
        }
    }

    /// Count of `key` today.
    #[must_use]
    pub fn get(&self, key: &str) -> u64 {
        let mut st = self.state.lock();
        self.roll(&mut st);
        st.counts.get(key).copied().unwrap_or(0)
    }

    /// Add `n` to `key` and persist. Returns the new count.
    pub fn add(&self, key: &str, n: u64) -> u64 {
        let mut st = self.state.lock();
        self.roll(&mut st);
        let v = st.counts.entry(key.to_owned()).or_insert(0);
        *v = v.saturating_add(n);
        let v = *v;
        self.persist(&st);
        v
    }

    /// Add one.
    pub fn bump(&self, key: &str) -> u64 {
        self.add(key, 1)
    }

    /// All of today's counts.
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<String, u64> {
        let mut st = self.state.lock();
        self.roll(&mut st);
        st.counts.clone()
    }

    fn persist(&self, st: &State) {
        let Some(path) = &self.path else { return };
        let Ok(bytes) = serde_json::to_vec(st) else {
            return;
        };
        let tmp = path.with_extension("tmp");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}
