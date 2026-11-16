//! Requests from a paired host to act on THIS computer, waiting for the person
//! at it (card 258-A).
//!
//! The host's bots may ask the desktop daemon to click, type or launch on the
//! user's PC. Each such request parks here until the local owner allows or
//! denies it in the desktop app; no answer in time is a denial. The queue is
//! only reachable through `/v1/hosts/computer-requests` (local user only, closed
//! to every paired role).

use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use utoipa::ToSchema;

/// One pending request, as the desktop app shows it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PcRequestInfo {
    pub id: String,
    /// Paired host the request comes from (public key).
    pub host_id: String,
    pub host_name: String,
    /// `click`, `type`, `key`, `scroll`, `launch`, ...
    pub action: String,
    /// Short single-line description of the target / input.
    pub detail: String,
    pub created_ms: i64,
}

/// The local owner's answer.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PcRequestDecision {
    pub allow: bool,
}

/// Switch for one host.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ComputerAccessInput {
    /// Let this host's bots use this computer.
    pub enabled: bool,
}

struct Entry {
    info: PcRequestInfo,
    tx: Option<oneshot::Sender<bool>>,
}

/// Pending requests of paired hosts.
#[derive(Default)]
pub struct PcRequests {
    entries: Mutex<Vec<Entry>>,
}

/// Cap on parked requests: a flood from a host cannot grow the queue.
const MAX_PENDING: usize = 16;

impl PcRequests {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Park a request and wait for the owner. `false` = denied, timed out or
    /// the queue is full; the caller must not act.
    pub async fn ask(
        &self,
        host_id: &str,
        host_name: &str,
        action: &str,
        detail: &str,
        timeout: Duration,
    ) -> bool {
        let id = uuid::Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut entries = self.entries.lock();
            if entries.len() >= MAX_PENDING {
                return false;
            }
            entries.push(Entry {
                info: PcRequestInfo {
                    id: id.clone(),
                    host_id: host_id.to_owned(),
                    host_name: clean(host_name, 64),
                    action: clean(action, 32),
                    detail: clean(detail, 200),
                    created_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0)),
                },
                tx: Some(tx),
            });
        }
        let answer = tokio::time::timeout(timeout, rx).await;
        self.entries.lock().retain(|e| e.info.id != id);
        matches!(answer, Ok(Ok(true)))
    }

    #[must_use]
    pub fn list(&self) -> Vec<PcRequestInfo> {
        self.entries.lock().iter().map(|e| e.info.clone()).collect()
    }

    /// Answer a pending request; `false` when it is gone (answered, timed out).
    pub fn decide(&self, id: &str, allow: bool) -> bool {
        let mut entries = self.entries.lock();
        match entries.iter_mut().find(|e| e.info.id == id) {
            Some(entry) => entry.tx.take().is_some_and(|tx| tx.send(allow).is_ok()),
            None => false,
        }
    }
}

/// Single line, no control characters, bounded: peer-supplied text on a card.
fn clean(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn waits_for_the_owner_and_times_out_as_a_denial() {
        let q = Arc::new(PcRequests::new());
        let q2 = q.clone();
        let asked = tokio::spawn(async move {
            q2.ask("h", "Server", "click", "OK", Duration::from_secs(5))
                .await
        });
        let id = loop {
            if let Some(r) = q.list().first() {
                break r.id.clone();
            }
            tokio::task::yield_now().await;
        };
        assert!(q.decide(&id, true));
        assert!(asked.await.unwrap());
        assert!(
            !q.ask("h", "Server", "click", "OK", Duration::from_millis(20))
                .await,
            "silence is a denial"
        );
        assert!(q.list().is_empty());
    }
}
