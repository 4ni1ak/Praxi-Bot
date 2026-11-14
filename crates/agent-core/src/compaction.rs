//! Deterministic, model-free shortening of the copy of a turn's conversation
//! that is sent to the model (stored steps, audit log and taint records are
//! never touched).
//!
//! Two passes, both in strides so a shortened text never changes again:
//!
//! 1. *Superseded observations*: only FULL observations of the SAME target
//!    supersede each other (`browser_open` / `browser_snapshot` of one URL,
//!    `desktop_get_state` of one window, `fs_list` of one path,
//!    `desktop_list_apps`).  Diff-returning browser actions are not
//!    observations here: they are never shortened, and the newest full
//!    snapshot they depend on always stays.
//! 2. *Soft byte budget*: above `soft_bytes` the oldest read-only results are
//!    cut to a preview, after their full text is on disk and its path is in
//!    the shortened text (if that is not possible the result stays whole).

use std::fmt::Write as _;
use std::path::PathBuf;

use provider_catalog::Message;
use serde_json::Value;
use tool_registry::{ObservationScope, ToolSpec};
use uuid::Uuid;

use crate::config::Guards;
use crate::history::truncate_utf8;

/// Pass 1 only considers observations whose model step lies below the
/// largest multiple of this that is `<= n_model_steps`.  The boundary only
/// moves forward, and only in strides, so the shortened prefix is stable
/// (prompt cache) between the calls of a stride.
pub(crate) const COMPACT_STRIDE: usize = 2;
/// Results shorter than this are never replaced by a marker.
const MIN_COMPACT_BYTES: usize = 256;
/// Longest tool-output title copied into a marker, in characters.
const MARKER_TITLE_CHARS: usize = 120;
/// Longest preserved untrusted-content label line.
const MARKER_LABEL_BYTES: usize = 240;
/// Size of the preview a result keeps when the soft byte budget is exceeded.
pub(crate) const BUDGET_PREVIEW_BYTES: usize = 512;

/// Looks a tool up in the live registry (by registered name).
pub(crate) type SpecLookup<'a> = &'a dyn Fn(&str) -> Option<ToolSpec>;

/// Settings of the within-turn compaction.
#[derive(Clone, Copy)]
pub(crate) struct Compaction<'a> {
    /// Newest observations kept in full per target; `0` disables compaction.
    pub keep_observations: usize,
    /// Soft byte budget of one request's messages; `0` disables the budget.
    pub soft_bytes: usize,
    /// Registry lookup used to make sure a bare tool name really is the
    /// built-in tool (a CLI/plugin/custom tool with a colliding name is never
    /// compacted).  `None`: names are trusted (unit tests only).
    pub lookup: Option<SpecLookup<'a>>,
    /// Where full texts are saved before the budget pass cuts them
    /// (`<data_dir>/spill/<run>`); `None`: the budget pass only cuts results
    /// that already have a file.
    pub spill_dir: Option<&'a PathBuf>,
}

impl<'a> Compaction<'a> {
    pub(crate) fn from_guards(
        g: &Guards,
        lookup: Option<SpecLookup<'a>>,
        spill_dir: Option<&'a PathBuf>,
    ) -> Self {
        Self {
            keep_observations: g.keep_observations,
            soft_bytes: g.turn_context_soft_bytes,
            lookup,
            spill_dir,
        }
    }

    /// No compaction at all.
    #[cfg(test)]
    pub(crate) const OFF: Compaction<'static> = Compaction {
        keep_observations: 0,
        soft_bytes: 0,
        lookup: None,
        spill_dir: None,
    };
}

/// A tool result message of the rebuilt conversation, with what the
/// compaction needs to know about it.  Read from the stored step record and
/// the call arguments, not from model-visible text (the one exception is the
/// page URL, which only identifies the target).
pub(crate) struct ToolMeta {
    pub msg_idx: usize,
    pub is_error: bool,
    /// Existing full-output file (the loop's own spill or the tool's).
    pub spill_path: Option<String>,
    pub call_uuid: Uuid,
    /// Index of the model step that issued the call.
    pub ordinal: usize,
    /// The resolved (not `use_tool`-wrapped) built-in tool, if it is one.
    pub scope: Option<ObservationScope>,
    /// Target identity inside the scope; `None`: cannot be told apart, never
    /// shortened by pass 1.
    pub key: Option<String>,
    /// Read-only, non-meta tool: pass 2 may cut its output.
    pub cuttable: bool,
    /// Pass 1 already replaced this result.
    pub shortened: bool,
}

/// `use_tool` hides the real tool: its name and input are in the arguments.
pub(crate) fn resolve_call(name: &str, arguments: &str) -> (String, Value) {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    if name == "use_tool" {
        let target = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let input = args.get("input").cloned().unwrap_or(Value::Null);
        return (target.to_owned(), input);
    }
    (name.to_owned(), args)
}

/// `# title (url)` first line of a browser observation -> `url`.
fn page_url(text: &str) -> Option<&str> {
    // A label line may come first; the page line is the first `# ` line.
    let line = text
        .lines()
        .take(3)
        .find(|l| l.starts_with("# "))?
        .trim_end();
    let rest = line.strip_prefix("# ")?;
    let inner = rest.strip_suffix(')')?;
    let at = inner.rfind(" (")?;
    let url = &inner[at + 2..];
    (!url.is_empty()).then_some(url)
}

/// Target identity of an observation result, from its tool, arguments and text.
fn target_key(scope: ObservationScope, tool: &str, args: &Value, text: &str) -> Option<String> {
    match scope {
        ObservationScope::Page => {
            let url = page_url(text)?;
            let interactive = args
                .get("interactive_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(format!("{tool}|{url}|{interactive}"))
        }
        // `desktop_get_state` without a window id or title reads whichever
        // window is in front: not a stable target.
        ObservationScope::Window => (args.get("window_id").is_some()
            || args.get("title").is_some())
        .then(|| args.to_string()),
        ObservationScope::FsListing | ObservationScope::AppListing => Some(args.to_string()),
    }
}

/// Builds the record for one tool result.
pub(crate) fn tool_meta(
    c: &Compaction<'_>,
    msg_idx: usize,
    ordinal: usize,
    call_name: &str,
    call_args: &str,
    r: &crate::shared::ResultRec,
) -> ToolMeta {
    let (name, args) = resolve_call(call_name, call_args);
    let spec = match c.lookup {
        Some(look) => look(&name),
        None => None,
    };
    let scope = match (&spec, c.lookup) {
        (Some(s), _) => ObservationScope::of_builtin(s),
        (None, None) => ObservationScope::of_name(&name),
        (None, Some(_)) => None,
    };
    let key = scope.and_then(|s| target_key(s, &name, &args, &r.text));
    let cuttable = spec.as_ref().is_some_and(|s| {
        s.effect == tool_registry::Effect::Read
            && s.category != tool_registry::PermissionCategory::Meta
    });
    ToolMeta {
        msg_idx,
        is_error: r.is_error,
        spill_path: r.spill_path.clone().or_else(|| self_spill_path(&r.text)),
        call_uuid: r.call_uuid,
        ordinal,
        scope,
        key,
        cuttable,
        shortened: false,
    }
}

/// A tool that spilled its own output ends its text with
/// `[full output (N bytes) at PATH]` (the loop's own rendering).
fn self_spill_path(text: &str) -> Option<String> {
    let last = text.lines().next_back()?.trim();
    let rest = last.strip_prefix("[full output (")?.strip_suffix(']')?;
    let (_, path) = rest.split_once(" bytes) at ")?;
    (!path.is_empty()).then(|| path.to_owned())
}

pub(crate) fn message_bytes(m: &Message) -> usize {
    let body = match &m.content {
        Value::String(s) => s.len(),
        Value::Null => 0,
        other => other.to_string().len(),
    };
    body + m
        .tool_calls
        .iter()
        .map(|c| c.name.len() + c.arguments.len() + c.id.len())
        .sum::<usize>()
}

/// Total bytes of the messages of a request (content and tool-call payloads).
#[cfg(test)]
pub(crate) fn total_bytes(messages: &[Message]) -> usize {
    messages.iter().map(message_bytes).sum()
}

/// First non-empty line of a tool output as a quoted, inert title: control
/// characters removed, brackets neutralised (so it cannot close the marker's
/// bracket), quotes escaped, at most [`MARKER_TITLE_CHARS`] characters.
pub(crate) fn marker_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !is_label(l))
        .unwrap_or_default();
    line.chars()
        .filter(|c| !c.is_control())
        .take(MARKER_TITLE_CHARS)
        .map(|c| match c {
            '[' => '(',
            ']' => ')',
            other => other,
        })
        .collect::<String>()
        .replace('"', "\\\"")
}

fn is_label(line: &str) -> bool {
    line.starts_with('[') && line.to_ascii_lowercase().contains("untrusted")
}

/// The bracketed untrusted-data label line of an output, if it has one.  It
/// goes first in every shortened text so the result stays marked as data.
pub(crate) fn marker_label(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| is_label(l)).map(|l| {
        let l: String = l.chars().filter(|c| !c.is_control()).collect();
        truncate_utf8(&l, MARKER_LABEL_BYTES).to_owned()
    })
}

/// Fixed-template replacement of a superseded observation.
fn observation_marker(text: &str, spill_path: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(label) = marker_label(text) {
        s.push_str(&label);
        s.push('\n');
    }
    let _ = write!(
        s,
        "[older observation shortened: {} bytes; first line (untrusted): \"{}\"; \
         observe again for the current state]",
        text.len(),
        marker_title(text)
    );
    if let Some(p) = spill_path {
        let _ = write!(s, "\n[full output saved at {p}]");
    }
    s
}

/// Pass 2 text: label first, then a preview, then where the rest is.
fn budget_preview(text: &str, path: &str) -> String {
    let preview = truncate_utf8(text, BUDGET_PREVIEW_BYTES);
    let mut s = String::new();
    if let Some(label) = marker_label(text)
        && !preview.contains(&label)
    {
        s.push_str(&label);
        s.push('\n');
    }
    s.push_str(preview);
    let _ = write!(
        s,
        "\n[output shortened: {} bytes; full output saved at {path}]",
        text.len()
    );
    s
}

/// Save `text` under the spill dir; the path of the file, `None` on failure.
fn save_full_text(dir: &PathBuf, call: Uuid, text: &str) -> Option<String> {
    let path = dir.join(format!("{call}.txt"));
    if !path.exists() {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&path, text).ok()?;
    }
    Some(path.display().to_string())
}

/// Applies both passes to `out` (see the module docs).
pub(crate) fn compact(out: &mut [Message], metas: &mut [ToolMeta], n_model: usize, c: &Compaction) {
    if c.keep_observations > 0 {
        pass_superseded(out, metas, n_model, c.keep_observations);
    }
    if c.soft_bytes > 0 {
        pass_budget(out, metas, n_model, c);
    }
}

fn pass_superseded(out: &mut [Message], metas: &mut [ToolMeta], n_model: usize, keep: usize) {
    let horizon = n_model / COMPACT_STRIDE * COMPACT_STRIDE;
    let mut groups: Vec<((ObservationScope, String), Vec<usize>)> = Vec::new();
    for (i, m) in metas.iter().enumerate() {
        let (Some(scope), Some(key)) = (m.scope, m.key.clone()) else {
            continue;
        };
        let long = out[m.msg_idx]
            .content
            .as_str()
            .is_some_and(|t| t.len() >= MIN_COMPACT_BYTES);
        if m.is_error || !long || m.ordinal >= horizon {
            continue;
        }
        match groups.iter_mut().find(|(k, _)| k.0 == scope && k.1 == key) {
            Some((_, v)) => v.push(i),
            None => groups.push(((scope, key), vec![i])),
        }
    }
    for (_, idxs) in groups {
        let stale = idxs.len().saturating_sub(keep);
        for i in idxs.into_iter().take(stale) {
            let m = &mut metas[i];
            let text = out[m.msg_idx].content.as_str().unwrap_or_default();
            let marker = observation_marker(text, m.spill_path.as_deref());
            out[m.msg_idx].content = Value::String(marker);
            m.shortened = true;
        }
    }
}

fn pass_budget(out: &mut [Message], metas: &mut [ToolMeta], n_model: usize, c: &Compaction) {
    let mut total: usize = out.iter().map(message_bytes).sum();
    for m in metas.iter_mut() {
        if total <= c.soft_bytes {
            break;
        }
        if m.is_error || m.shortened || !m.cuttable || m.ordinal + 1 >= n_model {
            continue;
        }
        let Some(text) = out[m.msg_idx].content.as_str() else {
            continue;
        };
        if text.len() <= BUDGET_PREVIEW_BYTES {
            continue;
        }
        // An untrusted notice somewhere in the middle of a payload would be
        // lost by a cut: leave such a result whole.
        if marker_label(text).is_none() && text.to_ascii_lowercase().contains("untrusted") {
            continue;
        }
        let path = match (&m.spill_path, c.spill_dir) {
            (Some(p), _) => p.clone(),
            (None, Some(dir)) => match save_full_text(dir, m.call_uuid, text) {
                Some(p) => p,
                None => continue,
            },
            (None, None) => continue,
        };
        let short = budget_preview(text, &path);
        total = total.saturating_sub(text.len()) + short.len();
        out[m.msg_idx].content = Value::String(short);
        m.shortened = true;
    }
}
