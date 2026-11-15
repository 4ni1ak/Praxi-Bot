//! Free decision-model resolution **by rule**, never by a wired-in id.
//!
//! Free OpenRouter model ids come and go (`...-v1.3:free` is replaced by
//! `...-v1.4:free`, a free period ends on a date). The owner therefore writes a
//! *rule* (`match = "*decider*:free"`, `prefer = ["flash"]`) and the
//! [`ModelResolver`] picks the id from OpenRouter's live model list
//! (`GET /api/v1/models`): only models whose every listed price is zero, whose
//! id fits the pattern, whose `expiration_date` has not passed and whose id has
//! a safe shape. The choice is cached on disk (refreshed every 6 hours), the
//! last good id is kept when the list cannot be fetched, and an id the API
//! answers with 404 / 410 is dropped at once and resolved again.
//!
//! Nothing here sends user text: the list request carries no key and no
//! content.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::DeciderError;

/// Default model-list endpoint of `OpenRouter`.
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// How long a resolved id is trusted before the list is fetched again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Longest id accepted from the list.
const MAX_ID: usize = 128;

/// Which model to pick: a glob over the id plus a preference order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRule {
    /// Glob over the model id; `*` matches any run of characters, the match is
    /// case-insensitive (`*decider*:free`).
    pub pattern: String,
    /// Substrings in order of preference; the first one an id contains wins,
    /// then the highest version.
    #[serde(default)]
    pub prefer: Vec<String>,
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // Iterative wildcard matching with backtracking over the last `*`.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A model id that is safe to put in a request body and a file name.
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

/// Compare ids so that `v1.10` is higher than `v1.9`.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let runs = |s: &str| {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let d = c.is_ascii_digit();
            match out.last_mut() {
                Some((kind, run)) if *kind == d => run.push(c),
                _ => out.push((d, c.to_string())),
            }
        }
        out
    };
    let (ra, rb) = (runs(a), runs(b));
    for (x, y) in ra.iter().zip(&rb) {
        let o = if x.0 && y.0 {
            let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
            nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
        } else {
            x.1.cmp(&y.1)
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ra.len().cmp(&rb.len())
}

/// Whether a `pricing` object lists only zero prices (and has the two that
/// every model must have).
fn is_free(pricing: &Value) -> bool {
    let Some(map) = pricing.as_object() else {
        return false;
    };
    let price = |k: &str| -> Option<f64> {
        let v = map.get(k)?;
        v.as_str()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .or_else(|| v.as_f64())
    };
    if price("prompt") != Some(0.0) || price("completion") != Some(0.0) {
        return false;
    }
    map.iter().all(|(k, v)| {
        if matches!(k.as_str(), "prompt" | "completion") {
            return true;
        }
        match v {
            Value::Null => true,
            Value::String(s) => s.trim().parse::<f64>().is_ok_and(|p| p == 0.0),
            Value::Number(n) => n.as_f64() == Some(0.0),
            _ => false,
        }
    })
}

/// Pick a model from an `OpenRouter` `/models` response (`{"data": [...]}`):
/// free, matching the rule, not expired on `today` (`YYYY-MM-DD`), a safe id.
/// Preference first, then the highest version. `None`: nothing qualifies.
#[must_use]
pub fn pick_free_model(list: &Value, rule: &ModelRule, today: &str) -> Option<String> {
    let data = list.get("data")?.as_array()?;
    let mut best: Option<(usize, &str)> = None;
    for m in data {
        let Some(id) = m.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !safe_id(id) || !glob_match(&rule.pattern, id) {
            continue;
        }
        if !m.get("pricing").is_some_and(is_free) {
            continue;
        }
        // `expiration_date` is a date (or null). Expiring today counts as
        // gone: the model may vanish at any time of that day.
        if let Some(exp) = m.get("expiration_date").and_then(Value::as_str)
            && !exp.is_empty()
            && exp.get(..10).unwrap_or(exp) <= today
        {
            continue;
        }
        let lower = id.to_lowercase();
        let rank = rule
            .prefer
            .iter()
            .position(|p| !p.is_empty() && lower.contains(&p.to_lowercase()))
            .unwrap_or(rule.prefer.len());
        let better = match best {
            None => true,
            Some((r, cur)) => rank < r || (rank == r && natural_cmp(id, cur) == Ordering::Greater),
        };
        if better {
            best = Some((rank, id));
        }
    }
    best.map(|(_, id)| id.to_owned())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    /// Last good id.
    #[serde(default)]
    id: Option<String>,
    /// Rule the id was resolved for; a changed rule invalidates the cache.
    #[serde(default)]
    rule: Option<ModelRule>,
}

struct State {
    id: Option<String>,
    fetched: Option<Instant>,
    /// Ids the API refused (404 / 410) this session.
    dropped: Vec<String>,
}

/// Resolves and remembers the model id for one rule.
pub struct ModelResolver {
    client: reqwest::Client,
    url: String,
    rule: ModelRule,
    cache_path: Option<PathBuf>,
    today: Box<dyn Fn() -> String + Send + Sync>,
    state: Mutex<State>,
    refreshing: AtomicBool,
}

impl std::fmt::Debug for ModelResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelResolver")
            .field("rule", &self.rule)
            .field("current", &self.current())
            .finish_non_exhaustive()
    }
}

impl ModelResolver {
    /// A resolver for `rule`; `cache_path` keeps the last good id on disk.
    #[must_use]
    pub fn new(rule: ModelRule, cache_path: Option<PathBuf>) -> Self {
        let cached = cache_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
            .filter(|c| c.rule.as_ref() == Some(&rule))
            .and_then(|c| c.id)
            .filter(|id| safe_id(id));
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            url: OPENROUTER_MODELS_URL.to_owned(),
            rule,
            cache_path,
            today: Box::new(super::local_day),
            state: Mutex::new(State {
                id: cached,
                fetched: None,
                dropped: Vec::new(),
            }),
            refreshing: AtomicBool::new(false),
        }
    }

    /// Refresh in the background (at most one at a time); the decision that
    /// noticed the id is stale does not wait for it.
    pub fn spawn_refresh(self: &Arc<Self>) {
        if self.refreshing.swap(true, AtomicOrdering::SeqCst) {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let _ = me.refresh().await;
            me.refreshing.store(false, AtomicOrdering::SeqCst);
        });
    }

    /// Override the model-list endpoint (tests, proxies).
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Override the day source (tests).
    #[must_use]
    pub fn with_today(mut self, today: Box<dyn Fn() -> String + Send + Sync>) -> Self {
        self.today = today;
        self
    }

    /// The id to use now (the cached or freshly resolved one), if any.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.state.lock().id.clone()
    }

    /// Whether the list should be fetched again (never fetched this session,
    /// or older than [`REFRESH_EVERY`]).
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.state
            .lock()
            .fetched
            .is_none_or(|t| t.elapsed() >= REFRESH_EVERY)
    }

    /// The API refused `id` (404 / 410): never use it again this session and
    /// resolve a new one on the next refresh.
    pub fn drop_id(&self, id: &str) {
        let mut st = self.state.lock();
        if st.id.as_deref() == Some(id) {
            st.id = None;
            st.fetched = None;
        }
        if !st.dropped.iter().any(|d| d == id) {
            st.dropped.push(id.to_owned());
        }
        drop(st);
        self.save();
    }

    /// Fetch the model list and re-resolve. On any failure the previous id is
    /// kept (the last known good one); an empty result keeps it too, but a
    /// list that no longer holds a qualifying model clears an id that was
    /// dropped.
    ///
    /// # Errors
    /// The list could not be fetched or parsed.
    pub async fn refresh(&self) -> Result<Option<String>, DeciderError> {
        let resp = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| DeciderError::Transport(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(DeciderError::Http(status.as_u16()));
        }
        let list: Value = resp
            .json()
            .await
            .map_err(|_| DeciderError::BadResponse("model list".into()))?;
        let mut picked_list = list;
        // Models the API refused this session are not candidates.
        let dropped = self.state.lock().dropped.clone();
        if let Some(data) = picked_list.get_mut("data").and_then(Value::as_array_mut) {
            data.retain(|m| {
                m.get("id")
                    .and_then(Value::as_str)
                    .is_none_or(|id| !dropped.iter().any(|d| d == id))
            });
        }
        let picked = pick_free_model(&picked_list, &self.rule, &(self.today)());
        {
            let mut st = self.state.lock();
            st.fetched = Some(Instant::now());
            if picked.is_some() {
                st.id.clone_from(&picked);
            } else if st
                .id
                .as_ref()
                .is_some_and(|cur| dropped.iter().any(|d| d == cur))
            {
                st.id = None;
            }
        }
        self.save();
        Ok(self.current())
    }

    fn save(&self) {
        let Some(path) = &self.cache_path else { return };
        let cache = Cache {
            id: self.current(),
            rule: Some(self.rule.clone()),
        };
        let Ok(bytes) = serde_json::to_vec(&cache) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}
