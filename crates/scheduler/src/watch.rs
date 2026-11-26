//! Clock-driven event coalescing. The daemon provides permission-gated snapshots.
use crate::{FileEvent, Trigger};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// File metadata polling interval; never read file contents.
pub const FILE_POLL_MS: i64 = 30_000;
/// Header-only mailbox polling interval.
pub const EMAIL_POLL_MS: i64 = 120_000;
/// Bound snapshot size and queued events per schedule.
pub const MAX_ENTRIES: usize = 1_000;

/// One bounded source observation, supplied by a real connector or a fake.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Stable filename or mailbox message identity.
    pub key: String,
    /// Metadata fingerprint; no file or message body.
    pub fingerprint: String,
    /// Filename, or message id/from/subject only.
    pub input: Value,
}

/// Durable state for an enabled event trigger.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchState {
    baseline: Option<BTreeMap<String, String>>,
    #[serde(default)]
    email_uid_high: Option<u64>,
    pending: BTreeMap<String, Value>,
    pending_since: Option<i64>,
    fired: VecDeque<i64>,
    /// Polling cadence follows the scheduler's injectable clock.
    pub next_poll_ms: i64,
}

impl WatchState {
    /// Update the snapshot. The first poll establishes the baseline.
    ///
    /// # Errors
    /// Source exceeded the bounded snapshot/queue/output size.
    pub fn observe(
        &mut self,
        trigger: &Trigger,
        now: i64,
        entries: Vec<Observation>,
    ) -> Result<(), &'static str> {
        if entries.len() > MAX_ENTRIES {
            return Err("watch snapshot limit exceeded");
        }
        let mut current = if matches!(trigger, Trigger::Email { .. }) {
            self.baseline.clone().unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        let mut pending = self.pending.clone();
        let mut uid_high = self.email_uid_high;
        for entry in entries {
            if entry.key.len() > 4096 || entry.fingerprint.len() > 256 {
                return Err("invalid source observation");
            }
            let uid = if matches!(trigger, Trigger::Email { .. }) {
                entry
                    .key
                    .strip_prefix("uid:")
                    .and_then(|v| v.parse::<u64>().ok())
            } else {
                None
            };
            if let Some(uid) = uid {
                uid_high = Some(uid_high.map_or(uid, |high| high.max(uid)));
            }
            if let Some(previous) = &self.baseline {
                let created = uid
                    .and_then(|u| self.email_uid_high.map(|high| u > high))
                    .unwrap_or_else(|| !previous.contains_key(&entry.key));
                let modified = previous
                    .get(&entry.key)
                    .is_some_and(|v| v != &entry.fingerprint);
                let selected = match trigger {
                    Trigger::FileWatch { events, .. } => {
                        (created && events.contains(&FileEvent::Created))
                            || (modified && events.contains(&FileEvent::Modified))
                    }
                    Trigger::Email { .. } => created,
                    Trigger::Webhook { .. } => false,
                };
                if selected {
                    pending.insert(entry.key.clone(), entry.input);
                }
            }
            if uid.is_none() {
                current.insert(entry.key, entry.fingerprint);
            }
        }
        if current.len() > 10_000
            || pending.len() > MAX_ENTRIES
            || serde_json::to_vec(&pending).map_or(true, |v| v.len() > crate::precheck::MAX_BYTES)
        {
            return Err("watch queue limit exceeded");
        }
        if !pending.is_empty() && self.pending.is_empty() {
            self.pending_since = Some(now);
        }
        self.pending = pending;
        self.baseline = Some(current);
        self.email_uid_high = uid_high;
        self.next_poll_ms = now.saturating_add(match trigger {
            Trigger::Email { .. } => EMAIL_POLL_MS,
            _ => FILE_POLL_MS,
        });
        Ok(())
    }

    /// Take one coalesced batch after debounce, at most six per rolling minute.
    /// Excess stays queued, including while the schedule's previous run is busy.
    pub fn take_ready(&mut self, trigger: &Trigger, now: i64) -> Option<String> {
        let since = self.pending_since?;
        let debounce = match trigger {
            Trigger::FileWatch { debounce_ms, .. } => i64::try_from(*debounce_ms).unwrap_or(60_000),
            _ => 0,
        };
        if now.saturating_sub(since) < debounce {
            return None;
        }
        self.fired.retain(|t| now.saturating_sub(*t) < 60_000);
        if self.fired.len() >= 6 {
            return None;
        }
        let values: Vec<_> = self.pending.values().collect();
        let input = serde_json::to_string(&values).ok()?;
        self.pending.clear();
        self.pending_since = None;
        self.fired.push_back(now);
        Some(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trigger() -> Trigger {
        Trigger::FileWatch {
            path: "workspace".into(),
            glob: "*.txt".into(),
            events: vec![FileEvent::Created, FileEvent::Modified],
            debounce_ms: 1000,
        }
    }
    fn entries(n: usize) -> Vec<Observation> {
        (0..n)
            .map(|i| Observation {
                key: format!("file{i}.txt"),
                fingerprint: "1".into(),
                input: Value::String(format!("file{i}.txt")),
            })
            .collect()
    }
    #[test]
    fn five_files_coalesce_debounce_and_persist_without_content() {
        let mut state = WatchState::default();
        let trigger = trigger();
        state.observe(&trigger, 0, vec![]).unwrap();
        state.observe(&trigger, 10, entries(5)).unwrap();
        assert!(state.take_ready(&trigger, 1009).is_none());
        let bytes = serde_json::to_string(&state).unwrap();
        let mut state: WatchState = serde_json::from_str(&bytes).unwrap();
        let payload = state.take_ready(&trigger, 1010).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&payload).unwrap().len(),
            5
        );
        assert!(state.take_ready(&trigger, 1011).is_none());
    }
    #[test]
    fn sixth_fire_caps_rate_and_excess_coalesces_until_next_window() {
        let mut state = WatchState::default();
        let trigger = Trigger::Email {
            account: "fake".into(),
            from_filter: "safe@example.test".into(),
            subject_filter: None,
        };
        state.observe(&trigger, 0, vec![]).unwrap();
        for n in 1..=6 {
            state
                .observe(&trigger, i64::try_from(n).unwrap(), entries(n))
                .unwrap();
            assert!(
                state
                    .take_ready(&trigger, i64::try_from(n).unwrap())
                    .is_some()
            );
        }
        state.observe(&trigger, 7, entries(7)).unwrap();
        assert!(state.take_ready(&trigger, 7).is_none());
        state.observe(&trigger, 8, entries(8)).unwrap();
        let batch = state.take_ready(&trigger, 60_001).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&batch).unwrap().len(),
            2
        );
    }
}
