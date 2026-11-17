//! Requests from a paired host to act on THIS computer, waiting for the person
//! at it (card 258-A).
//!
//! The host's bots may ask the desktop daemon to look at the screen, click,
//! type or launch on the user's PC. Each such request parks here until the
//! local owner allows or denies it in the desktop app; no answer in time is a
//! denial. The queue is only reachable through `/v1/hosts/computer-requests`
//! (owner authority only, closed to every paired role).
//!
//! The card text is built on THIS computer from the parsed request and the
//! local OS (see `apps/cli/src/pc_target.rs`); the host's own words (its name)
//! are the only peer-supplied text and the UI labels them as such.

use std::time::{Duration, Instant};

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
    /// Name the host announced (untrusted text, shown as "from the server").
    pub host_name: String,
    /// `session_read` (first look at the screen this session) or `act`.
    pub kind: String,
    /// `click`, `type`, `key`, `scroll`, `launch`, ...
    pub action: String,
    /// What the input would reach, resolved by this computer's OS.
    pub target: String,
    /// The complete, masked payload (text to type, keys, launch command).
    pub payload: String,
    pub created_ms: i64,
}

/// Pending requests and who is looking at the screen right now.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PcRequestList {
    pub requests: Vec<PcRequestInfo>,
    /// Names of hosts whose bots read this screen in the last few seconds.
    pub reading: Vec<String>,
}

/// The local owner's answer.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PcRequestDecision {
    pub allow: bool,
}

/// Switches for one host. Each family is its own switch; all are off until
/// the person at this computer turns them on.
// One flag per family (wire shape).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize, ToSchema)]
pub struct ComputerAccessInput {
    /// Screen and input: let this host's bots look at the screen, click, type
    /// and start programs on this computer.
    pub enabled: bool,
    /// Do not ask before the first look at the screen each session.
    #[serde(default)]
    pub always_read: bool,
    /// Let them use this computer's managed browser.
    #[serde(default)]
    pub browser: bool,
    /// Let them run shell commands here (each one asks).
    #[serde(default)]
    pub commands: bool,
    /// Let them read files here (writing asks every time).
    #[serde(default)]
    pub files: bool,
}

/// What a card says; built by the embedder from the parsed request.
#[derive(Debug, Clone)]
pub struct PcPreview {
    pub kind: &'static str,
    pub action: String,
    pub target: String,
    pub payload: String,
}

struct Entry {
    info: PcRequestInfo,
    tx: Option<oneshot::Sender<bool>>,
}

/// Pending requests of paired hosts.
#[derive(Default)]
pub struct PcRequests {
    entries: Mutex<Vec<Entry>>,
    reads: Mutex<Vec<(String, String, Instant)>>,
}

/// Cap on parked requests: a flood from a host cannot grow the queue.
const MAX_PENDING: usize = 16;
/// One host cannot occupy more than this many cards at once.
const MAX_PER_HOST: usize = 2;
/// A read counts as "looking right now" for this long.
const READING_WINDOW: Duration = Duration::from_secs(8);

/// Removes the entry when the waiting call ends for ANY reason, including the
/// caller being dropped (transport deadline, closed connection).
struct EntryGuard<'a> {
    queue: &'a PcRequests,
    id: String,
}

impl Drop for EntryGuard<'_> {
    fn drop(&mut self) {
        self.queue.entries.lock().retain(|e| e.info.id != self.id);
    }
}

impl PcRequests {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Park a request and wait for the owner. `false` = denied, timed out,
    /// cancelled or the queue is full; the caller must not act.
    pub async fn ask(
        &self,
        host_id: &str,
        host_name: &str,
        preview: PcPreview,
        timeout: Duration,
    ) -> bool {
        let id = uuid::Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut entries = self.entries.lock();
            if entries.len() >= MAX_PENDING
                || entries.iter().filter(|e| e.info.host_id == host_id).count() >= MAX_PER_HOST
            {
                return false;
            }
            entries.push(Entry {
                info: PcRequestInfo {
                    id: id.clone(),
                    host_id: host_id.to_owned(),
                    host_name: clean(host_name, 64),
                    kind: preview.kind.to_owned(),
                    action: clean(&preview.action, 32),
                    target: clean(&preview.target, 300),
                    payload: bounded(&preview.payload, 4000),
                    created_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0)),
                },
                tx: Some(tx),
            });
        }
        let _guard = EntryGuard {
            queue: self,
            id: id.clone(),
        };
        matches!(tokio::time::timeout(timeout, rx).await, Ok(Ok(true)))
    }

    #[must_use]
    pub fn list(&self) -> Vec<PcRequestInfo> {
        self.entries.lock().iter().map(|e| e.info.clone()).collect()
    }

    /// Pending requests plus the hosts reading the screen right now.
    #[must_use]
    pub fn snapshot(&self) -> PcRequestList {
        PcRequestList {
            requests: self.list(),
            reading: self.reading(),
        }
    }

    /// Answer a pending request; `false` when it is gone (answered, timed out).
    pub fn decide(&self, id: &str, allow: bool) -> bool {
        let mut entries = self.entries.lock();
        match entries.iter_mut().find(|e| e.info.id == id) {
            Some(entry) => entry.tx.take().is_some_and(|tx| tx.send(allow).is_ok()),
            None => false,
        }
    }

    /// Deny and drop everything a host has parked (switch turned off, host
    /// removed or revoked).
    pub fn cancel_host(&self, host_id: &str) {
        let mut entries = self.entries.lock();
        for entry in entries.iter_mut().filter(|e| e.info.host_id == host_id) {
            if let Some(tx) = entry.tx.take() {
                let _ = tx.send(false);
            }
        }
        entries.retain(|e| e.info.host_id != host_id);
        self.reads.lock().retain(|(id, _, _)| id != host_id);
    }

    /// A host's bot just read the screen (drives the "looking" indicator).
    pub fn note_read(&self, host_id: &str, host_name: &str) {
        let mut reads = self.reads.lock();
        reads.retain(|(id, _, at)| id != host_id && at.elapsed() < READING_WINDOW);
        reads.push((host_id.to_owned(), clean(host_name, 64), Instant::now()));
    }

    #[must_use]
    pub fn reading(&self) -> Vec<String> {
        self.reads
            .lock()
            .iter()
            .filter(|(_, _, at)| at.elapsed() < READING_WINDOW)
            .map(|(_, name, _)| name.clone())
            .collect()
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

/// Multi-line payload text: control characters other than newline/tab become
/// spaces. The embedder rejects payloads longer than the cap BEFORE asking, so
/// this never hides a tail.
fn bounded(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn preview() -> PcPreview {
        PcPreview {
            kind: "act",
            action: "click".into(),
            target: "Notepad".into(),
            payload: "OK".into(),
        }
    }

    #[tokio::test]
    async fn waits_for_the_owner_and_times_out_as_a_denial() {
        let q = Arc::new(PcRequests::new());
        let q2 = q.clone();
        let asked = tokio::spawn(async move {
            q2.ask("h", "Server", preview(), Duration::from_secs(5))
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
            !q.ask("h", "Server", preview(), Duration::from_millis(20))
                .await,
            "silence is a denial"
        );
        assert!(q.list().is_empty());
    }

    /// Callers dropped by a transport deadline must not keep their slot, and
    /// switching a host off denies what it had parked.
    #[tokio::test]
    async fn cancelled_waits_free_their_slot_and_cancel_host_denies() {
        let q = Arc::new(PcRequests::new());
        for _ in 0..40 {
            // The caller is dropped mid-wait, like an RPC that hit its deadline.
            let _ = tokio::time::timeout(
                Duration::from_millis(5),
                q.ask("h", "Server", preview(), Duration::from_secs(60)),
            )
            .await;
        }
        assert!(q.list().is_empty(), "no leaked entries after cancellation");
        let q2 = q.clone();
        let parked = tokio::spawn(async move {
            q2.ask("h2", "Other", preview(), Duration::from_secs(60))
                .await
        });
        while q.list().is_empty() {
            tokio::task::yield_now().await;
        }
        q.cancel_host("h2");
        assert!(
            !parked.await.unwrap(),
            "switching off denies the parked ask"
        );
        assert!(q.list().is_empty());
    }
}
