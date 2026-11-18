//! Live model lists of the coding CLIs.
//!
//! Order per engine: (1) ask the CLI (fixed argv, bounded time and output),
//! (2) the CLI's own local model cache file, (3) the bundled registry list.
//! Output is parsed defensively and never interpreted as a command. The result is
//! kept in memory and on disk with a timestamp; reading it never starts a process.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// A cached list older than this is refreshed in the background.
pub const STALE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
/// Hard bounds for one list.
const MAX_MODELS: usize = 400;
const MAX_ID_LEN: usize = 200;
const MAX_LABEL_LEN: usize = 120;
const MAX_CACHE_FILE: u64 = 4 * 1024 * 1024;

/// One model a CLI offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliModel {
    pub id: String,
    /// Display name as the CLI reports it (`None`: the catalog or the id decides).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Where a list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// The CLI answered a list command.
    Cli,
    /// The CLI's own local model cache file.
    LocalCache,
    /// The bundled registry list (last resort).
    Bundled,
}

impl ModelSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::LocalCache => "local_cache",
            Self::Bundled => "bundled",
        }
    }
}

/// A model list with its provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelList {
    pub models: Vec<CliModel>,
    pub source: ModelSource,
    pub fetched_at_ms: i64,
}

impl ModelList {
    /// The bundled fallback of an engine (`fetched_at_ms` is 0: never fetched).
    #[must_use]
    pub fn bundled(engine: &str) -> Self {
        let models = crate::engine_spec(engine)
            .map(|spec| {
                spec.models
                    .iter()
                    .map(|id| CliModel {
                        id: (*id).to_owned(),
                        label: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            models,
            source: ModelSource::Bundled,
            fetched_at_ms: 0,
        }
    }
}

#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('-')
        && id.bytes().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/' | b':' | b'@' | b'+')
        })
}

/// Drop control and zero-width characters (Cursor appends some), collapse spaces, bound it.
fn clean_label(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}')
        })
        .collect();
    let text = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then(|| text.chars().take(MAX_LABEL_LEN).collect())
}

fn push_unique(
    out: &mut Vec<CliModel>,
    seen: &mut HashSet<String>,
    id: &str,
    label: Option<String>,
) {
    if out.len() < MAX_MODELS && safe_id(id) && seen.insert(id.to_owned()) {
        out.push(CliModel {
            id: id.to_owned(),
            label,
        });
    }
}

/// `codex debug models` / `~/.codex/models_cache.json`: `{"models":[{slug,display_name,visibility,priority}]}`.
#[must_use]
pub fn parse_codex_json(text: &str) -> Vec<CliModel> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = value.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    let mut rows: Vec<(i64, &serde_json::Value)> = items
        .iter()
        .filter(|m| m.get("visibility").and_then(|v| v.as_str()) != Some("hide"))
        .map(|m| {
            (
                m.get("priority")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(i64::MAX),
                m,
            )
        })
        .collect();
    rows.sort_by_key(|(p, _)| *p);
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    for (_, m) in rows {
        let Some(slug) = m.get("slug").and_then(|s| s.as_str()) else {
            continue;
        };
        let label = m
            .get("display_name")
            .and_then(|s| s.as_str())
            .and_then(clean_label);
        push_unique(&mut out, &mut seen, slug, label);
    }
    out
}

fn strip_status_marker(label: &str) -> &str {
    let t = label.trim_end();
    for marker in ["(default)", "(current)"] {
        if let Some(rest) = t.strip_suffix(marker) {
            return rest.trim_end();
        }
    }
    t
}

/// `cursor-agent models`: a heading, then `slug - Label` lines.
#[must_use]
pub fn parse_cursor_text(text: &str) -> Vec<CliModel> {
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    for line in text.lines() {
        let Some((id, label)) = line.split_once(" - ") else {
            continue;
        };
        let id = id.trim();
        let label = clean_label(label).map(|l| strip_status_marker(&l).to_owned());
        push_unique(&mut out, &mut seen, id, label.filter(|l| !l.is_empty()));
    }
    collapse_variants(out)
}

/// `agy models`: `id<TAB>Label` lines after a "Fetching..." line.
#[must_use]
pub fn parse_agy_text(text: &str) -> Vec<CliModel> {
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    for line in text.lines() {
        let Some((id, label)) = line.split_once('\t') else {
            continue;
        };
        push_unique(&mut out, &mut seen, id.trim(), clean_label(label));
    }
    collapse_variants(out)
}

/// `opencode models`: one `provider/model` per line.
#[must_use]
pub fn parse_opencode_text(text: &str) -> Vec<CliModel> {
    let (mut out, mut seen) = (Vec::new(), HashSet::new());
    for line in text.lines() {
        let id = line.trim();
        if id.contains('/') {
            push_unique(&mut out, &mut seen, id, None);
        }
    }
    out
}

const EFFORT_WORDS: &[&str] = &["low", "medium", "high", "xhigh", "max", "minimal", "none"];

fn is_variant_token(token: &str) -> bool {
    token == "fast" || token == "thinking" || EFFORT_WORDS.contains(&token)
}

fn base_of(id: &str) -> String {
    let kept: Vec<&str> = id.split('-').filter(|t| !is_variant_token(t)).collect();
    if kept.is_empty() {
        id.to_owned()
    } else {
        kept.join("-")
    }
}

/// Strip trailing effort / speed words from a label (`Grok 4.7 Medium Fast` -> `Grok 4.7`).
fn base_label(label: &str) -> String {
    let mut t = label.trim().to_owned();
    loop {
        let before = t.clone();
        if let Some(open) = t.rfind('(')
            && t.ends_with(')')
        {
            let inner = t[open + 1..t.len() - 1].to_ascii_lowercase();
            if inner
                .split_whitespace()
                .all(|w| matches!(w, "fast" | "extra") || EFFORT_WORDS.contains(&w))
            {
                t.truncate(open);
            }
        }
        t.truncate(t.trim_end().len());
        let lower = t.to_ascii_lowercase();
        for word in [
            "extra high",
            "fast",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "minimal",
        ] {
            if lower.ends_with(&format!(" {word}")) {
                t.truncate(t.len() - word.len());
                t.truncate(t.trim_end().len());
                break;
            }
        }
        if t == before {
            return t;
        }
    }
}

/// Cursor and Antigravity list one row per effort/speed variant (hundreds of rows). The picker
/// shows each model once: the plain id, else the medium, else the high variant, else the first
/// non-fast one. The id is always a real id the CLI reported.
#[must_use]
pub fn collapse_variants(models: Vec<CliModel>) -> Vec<CliModel> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<CliModel>> = BTreeMap::new();
    for m in models {
        let base = base_of(&m.id);
        if !groups.contains_key(&base) {
            order.push(base.clone());
        }
        groups.entry(base).or_default().push(m);
    }
    let has = |m: &CliModel, word: &str| m.id.split('-').any(|t| t == word);
    order
        .into_iter()
        .filter_map(|base| {
            let g = groups.remove(&base)?;
            let pick = g
                .iter()
                .find(|m| m.id == base)
                .or_else(|| g.iter().find(|m| has(m, "medium") && !has(m, "fast")))
                .or_else(|| g.iter().find(|m| has(m, "high") && !has(m, "fast")))
                .or_else(|| g.iter().find(|m| !has(m, "fast")))
                .or_else(|| g.first())?;
            Some(CliModel {
                id: pick.id.clone(),
                label: pick
                    .label
                    .as_deref()
                    .map(base_label)
                    .filter(|l| !l.is_empty()),
            })
        })
        .collect()
}

/// The CLI's own model cache file, when it keeps one (read-only, size-capped, names only).
#[must_use]
pub fn read_local_cache(
    engine: &str,
    home: Option<&Path>,
    codex_home: Option<&Path>,
) -> Vec<CliModel> {
    let path = match engine {
        "codex" => codex_home
            .map(Path::to_path_buf)
            .or_else(|| home.map(|h| h.join(".codex")))
            .map(|d| d.join("models_cache.json")),
        _ => None,
    };
    let Some(path) = path else {
        return Vec::new();
    };
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() <= MAX_CACHE_FILE => {}
        _ => return Vec::new(),
    }
    std::fs::read_to_string(&path)
        .map(|text| parse_codex_json(&text))
        .unwrap_or_default()
}

/// The CLI can report its models (a list command or a local cache file). Claude Code and
/// Gemini CLI cannot: their bundled aliases are the list, not a fallback.
#[must_use]
pub fn can_list(engine: &str) -> bool {
    list_args(engine).is_some()
}

/// Fixed argv of an engine's list command (`None`: the CLI has no such command).
#[must_use]
pub fn list_args(engine: &str) -> Option<&'static [&'static str]> {
    match engine {
        "codex" => Some(&["debug", "models"]),
        "cursor" | "opencode" | "agy" => Some(&["models"]),
        _ => None,
    }
}

/// Parse the stdout of [`list_args`] for the engine.
#[must_use]
pub fn parse_list_output(engine: &str, stdout: &str) -> Vec<CliModel> {
    match engine {
        "codex" => parse_codex_json(stdout),
        "cursor" => parse_cursor_text(stdout),
        "opencode" => parse_opencode_text(stdout),
        "agy" => parse_agy_text(stdout),
        _ => Vec::new(),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    engines: BTreeMap<String, ModelList>,
}

/// In-memory + on-disk store of the lists. Reads are instant.
pub struct ModelCache {
    path: Option<PathBuf>,
    lists: parking_lot::RwLock<BTreeMap<String, ModelList>>,
    refreshing: parking_lot::Mutex<HashSet<String>>,
    /// When each engine was last asked (any outcome), so a failing CLI is not retried per request.
    attempted: parking_lot::Mutex<BTreeMap<String, i64>>,
}

impl ModelCache {
    /// Load `path` (a missing or damaged file is an empty cache). `None`: memory only.
    #[must_use]
    pub fn load(path: Option<PathBuf>) -> Arc<Self> {
        let lists = path
            .as_deref()
            .and_then(|p| {
                std::fs::metadata(p)
                    .ok()
                    .filter(|m| m.len() <= MAX_CACHE_FILE)
                    .map(|_| p)
            })
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<CacheFile>(&t).ok())
            .map(|f| f.engines)
            .unwrap_or_default();
        Arc::new(Self {
            path,
            lists: parking_lot::RwLock::new(lists),
            refreshing: parking_lot::Mutex::new(HashSet::new()),
            attempted: parking_lot::Mutex::new(BTreeMap::new()),
        })
    }

    /// The stored list, if any.
    #[must_use]
    pub fn get(&self, engine: &str) -> Option<ModelList> {
        self.lists.read().get(engine).cloned()
    }

    /// The stored list or the bundled fallback; never blocks.
    #[must_use]
    pub fn get_or_bundled(&self, engine: &str) -> ModelList {
        self.get(engine)
            .unwrap_or_else(|| ModelList::bundled(engine))
    }

    #[must_use]
    pub fn is_stale(&self, engine: &str, now: i64) -> bool {
        self.get(engine).is_none_or(|l| {
            l.source == ModelSource::Bundled || now - l.fetched_at_ms >= STALE_AFTER_MS
        })
    }

    /// Claim the right to refresh `engine` (one refresh at a time per engine).
    #[must_use]
    pub fn begin_refresh(&self, engine: &str) -> bool {
        let claimed = self.refreshing.lock().insert(engine.to_owned());
        if claimed {
            self.attempted.lock().insert(engine.to_owned(), now_ms());
        }
        claimed
    }

    /// Stale, and not asked within the last 10 minutes (an uninstalled or failing CLI).
    #[must_use]
    pub fn should_auto_refresh(&self, engine: &str, now: i64) -> bool {
        const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;
        can_list(engine)
            && self.is_stale(engine, now)
            && self
                .attempted
                .lock()
                .get(engine)
                .is_none_or(|at| now - at >= RETRY_AFTER_MS)
    }

    pub fn end_refresh(&self, engine: &str) {
        self.refreshing.lock().remove(engine);
    }

    /// Store a fetched list (a bundled result is kept in memory only, so a later start retries).
    pub fn put(&self, engine: &str, list: ModelList) {
        let persist = list.source != ModelSource::Bundled;
        self.lists.write().insert(engine.to_owned(), list);
        if persist {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let file = CacheFile {
            engines: self
                .lists
                .read()
                .iter()
                .filter(|(_, l)| l.source != ModelSource::Bundled)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let Ok(json) = serde_json::to_vec(&file) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_list_keeps_visible_models_in_priority_order() {
        let json = r#"{"models":[
            {"slug":"gpt-6-astra","display_name":"GPT-6-Astra","visibility":"list","priority":3},
            {"slug":"gpt-reserve","display_name":"x","visibility":"hide","priority":5},
            {"slug":"gpt-6.1-sol","display_name":"GPT-6.1-Sol","visibility":"list","priority":1}]}"#;
        let m = parse_codex_json(json);
        assert_eq!(
            m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["gpt-6.1-sol", "gpt-6-astra"]
        );
        assert_eq!(m[0].label.as_deref(), Some("GPT-6.1-Sol"));
        assert_eq!(parse_codex_json("not json"), Vec::<CliModel>::new());
    }

    #[test]
    fn cursor_variants_collapse_to_one_row_per_model_with_zero_width_noise_removed() {
        let text = "Available models\n\nauto - Auto (default)\n\
            grok-4.7-low - Grok 4.7  Low\ngrok-4.7-medium - Grok 4.7  Medium\n\
            grok-4.7-medium-fast - Grok 4.7  Medium Fast\u{200b}\u{200b}\n\
            gpt-5.2 - GPT-5.2\ncomposer-2.5 - Composer 2.5\n--bad - x\nhas space - y\n";
        let m = parse_cursor_text(text);
        let ids: Vec<_> = m.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["auto", "grok-4.7-medium", "gpt-5.2", "composer-2.5"]);
        assert_eq!(m[0].label.as_deref(), Some("Auto"));
        assert_eq!(m[1].label.as_deref(), Some("Grok 4.7"));
    }

    #[test]
    fn agy_rows_collapse_and_cache_round_trips_without_bundled() {
        let text = "Fetching available models...\ngemini-4-argon-medium\tGemini 4 Argon (Medium)\n\
            gemini-4-argon-high\tGemini 4 Argon (High)\nclaude-opus-5-5-low\tClaude Opus 5.5 (Low)\n";
        let m = parse_agy_text(text);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].label.as_deref(), Some("Gemini 4 Argon"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("cli-models.json");
        let cache = ModelCache::load(Some(path.clone()));
        assert!(cache.is_stale("agy", now_ms()));
        cache.put(
            "agy",
            ModelList {
                models: m,
                source: ModelSource::Cli,
                fetched_at_ms: now_ms(),
            },
        );
        cache.put("codex", ModelList::bundled("codex"));
        let again = ModelCache::load(Some(path));
        assert!(!again.is_stale("agy", now_ms()));
        assert_eq!(again.get("agy").unwrap().models.len(), 2);
        assert!(
            again.get("codex").is_none(),
            "the bundled fallback is never persisted"
        );
        assert_eq!(again.get_or_bundled("codex").source, ModelSource::Bundled);
    }
}
