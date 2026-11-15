//! Typed secrets held in memory only.
//!
//! A value typed into a password field must reach the field and nowhere else:
//! not an approval card, an event, the journal, a stored step, the history the
//! model reads next turn, nor the disk at any moment. So the display form of a
//! call (see `Tool::display_input`) is computed first and is the ONLY form that
//! is ever stored; the real arguments live here, keyed by an opaque handle that
//! the stored display form carries, until the call has run. After a restart
//! (or once the call ran) the handle resolves to nothing, and a call that still
//! needs the secret is answered with an error asking for it again.

use std::collections::VecDeque;
use std::hash::{BuildHasher, RandomState};

use parking_lot::Mutex;
use protocol::RunId;
use serde_json::Value;

/// Handles kept at most; the oldest is dropped first.
const MAX_ENTRIES: usize = 1024;

struct Entry {
    id: String,
    run: RunId,
    /// The call's real arguments, exactly as the model wrote them; dropped
    /// (the fingerprint stays) once the call has run.
    raw: Option<String>,
    /// Keyed hash of `raw` (see [`SecretVault::fingerprint`]).
    fingerprint: u64,
}

/// In-memory store of the real arguments of calls whose display form hides a secret.
pub(crate) struct SecretVault {
    /// Per-process random key: a fingerprint says "same arguments" within this
    /// process and cannot be recomputed from outside it.
    key: RandomState,
    entries: Mutex<VecDeque<Entry>>,
}

impl Default for SecretVault {
    fn default() -> Self {
        Self {
            key: RandomState::new(),
            entries: Mutex::new(VecDeque::new()),
        }
    }
}

impl SecretVault {
    /// Keyed hash of canonical call arguments.
    pub(crate) fn fingerprint(&self, raw: &str) -> u64 {
        let canon =
            serde_json::from_str::<Value>(raw).map_or_else(|_| raw.to_owned(), |v| v.to_string());
        self.key.hash_one(canon)
    }

    /// Keep `raw` for `run`; the returned handle goes into the stored display form.
    pub(crate) fn put(&self, run: RunId, raw: &str) -> String {
        let id = uuid::Uuid::now_v7().simple().to_string();
        let mut all = self.entries.lock();
        if all.len() >= MAX_ENTRIES {
            all.pop_front();
        }
        all.push_back(Entry {
            id: id.clone(),
            run,
            raw: Some(raw.to_owned()),
            fingerprint: self.fingerprint(raw),
        });
        id
    }

    /// Forget every held value, as a restart would.
    #[cfg(test)]
    pub(crate) fn clear(&self) {
        self.entries.lock().clear();
    }

    /// The real arguments behind `id`, if this process still holds them for `run`.
    pub(crate) fn raw(&self, run: RunId, id: &str) -> Option<String> {
        self.entries
            .lock()
            .iter()
            .find(|e| e.id == id && e.run == run)
            .and_then(|e| e.raw.clone())
    }

    /// The fingerprint of the arguments behind `id`.
    pub(crate) fn fingerprint_of(&self, run: RunId, id: &str) -> Option<u64> {
        self.entries
            .lock()
            .iter()
            .find(|e| e.id == id && e.run == run)
            .map(|e| e.fingerprint)
    }

    /// Drop the real arguments behind `id` (the call has run); the keyed
    /// fingerprint stays so the repeat guard still tells identical calls apart.
    pub(crate) fn forget(&self, id: &str) {
        for e in self.entries.lock().iter_mut().filter(|e| e.id == id) {
            if let Some(raw) = e.raw.as_mut() {
                raw.clear();
            }
            e.raw = None;
        }
    }
}

/// Every handle that tags a masked placeholder anywhere in `v`.
pub(crate) fn refs_in(v: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                if let Some(id) = tool_registry::secret_ref_of(s)
                    && !out.iter().any(|x| x == id)
                {
                    out.push(id.to_owned());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

/// `v` with the handle removed from every placeholder that carries one: text a
/// model wrote (typically copied from its own masked history) is a plain
/// placeholder, never a way to name a held secret.
pub(crate) fn untag(v: &mut Value) {
    match v {
        Value::String(s) if tool_registry::secret_ref_of(s).is_some() => {
            if let Some((head, _)) = s.rsplit_once(" [ref ") {
                *s = head.to_owned();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(untag),
        Value::Object(o) => o.values_mut().for_each(untag),
        _ => {}
    }
}

/// `view` with every masked placeholder string tagged with `id`.
pub(crate) fn tag(view: &mut Value, id: &str) {
    match view {
        Value::String(s) if tool_registry::is_masked_secret(s) => {
            *s = tool_registry::with_secret_ref(s, id);
        }
        Value::Array(a) => a.iter_mut().for_each(|x| tag(x, id)),
        Value::Object(o) => o.values_mut().for_each(|x| tag(x, id)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_handle_resolves_for_its_run_only_and_is_forgotten() {
        let v = SecretVault::default();
        let (a, b) = (RunId::new(), RunId::new());
        let id = v.put(a, r#"{"text":"pw"}"#);
        assert_eq!(v.raw(a, &id).as_deref(), Some(r#"{"text":"pw"}"#));
        assert_eq!(v.raw(b, &id), None, "another run cannot use it");
        v.forget(&id);
        assert_eq!(v.raw(a, &id), None);
        assert!(v.fingerprint_of(a, &id).is_some(), "the fingerprint stays");
    }

    #[test]
    fn fingerprints_tell_identical_calls_from_different_ones() {
        let v = SecretVault::default();
        let run = RunId::new();
        let one = v.put(run, r#"{"ref":"e1","text":"alpha"}"#);
        let same = v.put(run, r#"{ "text": "alpha", "ref": "e1" }"#);
        let other = v.put(run, r#"{"ref":"e1","text":"bravo"}"#);
        let (f1, f2, f3) = (
            v.fingerprint_of(run, &one).unwrap(),
            v.fingerprint_of(run, &same).unwrap(),
            v.fingerprint_of(run, &other).unwrap(),
        );
        assert_eq!(f1, f2, "the same call, however it is written");
        assert_ne!(f1, f3, "a different password is a different call");
    }

    #[test]
    fn tagging_and_finding_refs_round_trips() {
        let mut view =
            json!({"ref": "e1", "text": tool_registry::masked_secret("abc", "x"), "n": 1});
        tag(&mut view, "h1");
        assert_eq!(refs_in(&view), ["h1"]);
        assert!(!view["ref"].as_str().unwrap().contains("h1"));
        untag(&mut view);
        assert!(
            refs_in(&view).is_empty(),
            "a model-written handle is stripped"
        );
        assert!(tool_registry::is_masked_secret(
            view["text"].as_str().unwrap()
        ));
    }
}
