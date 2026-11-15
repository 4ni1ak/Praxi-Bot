//! BM25 tool discovery index over tool name + description + parameter names.
//!
//! Terms are stemmed (stemming-lite) and queries are expanded with an EN + TR
//! synonym table, see [`crate::lexical`].

use serde_json::Value;
use tool_registry::ToolSpec;

use crate::bm25::{Bm25Index, tokenize};
use crate::lexical::{expand_query, stem_all};

/// How many times the tool *name* is repeated in the indexed document, i.e. a
/// cheap field boost: a hit in the name outweighs a hit in the description.
const NAME_BOOST: usize = 3;

/// Leaf verbs a person reaches for first; hits named after them rank higher.
const PRIMARY_VERBS: &[&str] = &[
    "open", "navigate", "search", "read", "write", "run", "fetch", "launch",
];
/// Niche leaf verbs: relevant only when asked for by name.
const NICHE_VERBS: &[&str] = &[
    "resize", "close", "drag", "hover", "console", "network", "install", "dialog", "upload",
    "handle",
];
/// Ranking multipliers (see [`rank_factor`]).
const PRIMARY_BOOST: f64 = 1.25;
const NICHE_PENALTY: f64 = 0.7;
const BUILTIN_BOOST: f64 = 1.1;

/// Multiplier applied to a BM25 score so the tool a person expects first wins:
/// primary verbs up, niche verbs down, built-in tools slightly above MCP ones
/// that expose the same family (`browser_open` vs `mcp:playwright/browser_*`).
fn rank_factor(name: &str) -> f64 {
    let (builtin, leaf) = match name.split_once(':') {
        Some((_, rest)) => (false, rest.rsplit('/').next().unwrap_or(rest)),
        None => (true, name),
    };
    let leaf = leaf.to_lowercase();
    let has = |verbs: &[&str]| leaf.split(['_', '.', '-']).any(|t| verbs.contains(&t));
    let mut f = 1.0;
    if has(PRIMARY_VERBS) {
        f *= PRIMARY_BOOST;
    } else if has(NICHE_VERBS) {
        f *= NICHE_PENALTY;
    }
    if builtin {
        f *= BUILTIN_BOOST;
    }
    f
}

/// One discovery result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolHit {
    /// Full tool name (`mcp:<server>/<tool>` for MCP tools).
    pub name: String,
    /// The tool's description.
    pub description: String,
    /// BM25 score (only meaningful relative to other hits of the same query).
    pub score: f64,
}

/// An immutable BM25 index over a snapshot of tool specs.
#[derive(Debug, Clone, Default)]
pub struct ToolIndex {
    entries: Vec<(String, String)>,
    index: Bm25Index,
}

/// Names of the top-level properties declared by a JSON-Schema input object.
fn param_names(schema: &Value) -> Vec<&str> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| props.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

impl ToolIndex {
    /// Index `specs`.
    #[must_use]
    pub fn build(specs: &[ToolSpec]) -> Self {
        let docs = specs.iter().map(|spec| {
            let mut terms = Vec::new();
            let name_terms = stem_all(&tokenize(&spec.name));
            for _ in 0..NAME_BOOST {
                terms.extend(name_terms.iter().cloned());
            }
            terms.extend(stem_all(&tokenize(&spec.description)));
            for param in param_names(&spec.input_schema) {
                terms.extend(stem_all(&tokenize(param)));
            }
            terms
        });
        Self {
            index: Bm25Index::new(docs),
            entries: specs
                .iter()
                .map(|s| (s.name.clone(), s.description.clone()))
                .collect(),
        }
    }

    /// Number of indexed tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The best `k` tools for a natural-language `query`, best first.
    #[must_use]
    pub fn search(&self, query: &str, k: usize) -> Vec<ToolHit> {
        let mut hits: Vec<ToolHit> = self
            .index
            .search(&expand_query(&tokenize(query)), self.entries.len())
            .into_iter()
            .map(|(doc, score)| ToolHit {
                name: self.entries[doc].0.clone(),
                description: self.entries[doc].1.clone(),
                score: score * rank_factor(&self.entries[doc].0),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.name.cmp(&b.name))
        });
        hits.truncate(k);
        hits
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tool_registry::{Effect, PermissionCategory, Platform};

    use super::*;

    fn spec(name: &str, description: &str, params: &[&str]) -> ToolSpec {
        let props: serde_json::Map<String, Value> = params
            .iter()
            .map(|p| ((*p).to_owned(), json!({ "type": "string" })))
            .collect();
        ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: json!({ "type": "object", "properties": props }),
            effect: Effect::Read,
            category: PermissionCategory::Accounts,
            platforms: vec![Platform::All],
        }
    }

    /// ~50 fake tools across a handful of fake servers.
    fn corpus() -> Vec<ToolSpec> {
        let mut specs = vec![
            spec(
                "mcp:github/create_issue",
                "Open a new issue in a repository",
                &["repo", "title", "body"],
            ),
            spec(
                "mcp:github/list_pull_requests",
                "List pull requests of a repository",
                &["repo", "state"],
            ),
            spec(
                "mcp:slack/post_message",
                "Post a message to a Slack channel",
                &["channel", "text"],
            ),
            spec(
                "mcp:gmail/send_email",
                "Send an e-mail to a recipient",
                &["to", "subject", "body"],
            ),
            spec(
                "mcp:fs/read_file",
                "Read the contents of a file from disk",
                &["path"],
            ),
            spec(
                "mcp:calendar/create_event",
                "Schedule a meeting on the calendar",
                &["title", "start", "attendees"],
            ),
            spec(
                "mcp:weather/get_forecast",
                "Weather forecast temperature for a city",
                &["city", "days"],
            ),
            spec(
                "mcp:db/run_query",
                "Execute a SQL query against the database",
                &["sql"],
            ),
        ];
        // Filler tools that share generic vocabulary but none of the above topics.
        for i in 0..42 {
            specs.push(spec(
                &format!("mcp:filler{i}/do_thing_{i}"),
                &format!("Generic helper number {i} that processes an item"),
                &["item", "option"],
            ));
        }
        assert_eq!(specs.len(), 50);
        specs
    }

    fn top(index: &ToolIndex, query: &str) -> String {
        index.search(query, 3)[0].name.clone()
    }

    #[test]
    fn ranks_the_right_tool_first_among_fifty() {
        let index = ToolIndex::build(&corpus());
        assert_eq!(index.len(), 50);
        assert_eq!(
            top(&index, "open an issue on github"),
            "mcp:github/create_issue"
        );
        assert_eq!(top(&index, "send email"), "mcp:gmail/send_email");
        assert_eq!(top(&index, "read a file"), "mcp:fs/read_file");
        assert_eq!(
            top(&index, "weather forecast for a city"),
            "mcp:weather/get_forecast"
        );
        assert_eq!(top(&index, "sql query"), "mcp:db/run_query");
        assert_eq!(
            top(&index, "schedule meeting attendees"),
            "mcp:calendar/create_event"
        );
    }

    #[test]
    fn parameter_names_are_searchable() {
        let index = ToolIndex::build(&corpus());
        // "channel" only appears as a parameter (and in the description) of slack.
        assert_eq!(top(&index, "channel"), "mcp:slack/post_message");
        // "attendees" is a parameter name only.
        assert_eq!(top(&index, "attendees"), "mcp:calendar/create_event");
    }

    #[test]
    fn name_hits_outweigh_description_hits() {
        let specs = vec![
            spec("alpha", "reports on widget status", &[]),
            spec("widget", "does something else entirely", &[]),
        ];
        let index = ToolIndex::build(&specs);
        assert_eq!(top(&index, "widget"), "widget");
    }

    #[test]
    fn respects_k_and_returns_nothing_for_unknown_terms() {
        let index = ToolIndex::build(&corpus());
        assert_eq!(index.search("helper", 5).len(), 5);
        assert_eq!(index.search("zzzzqqq", 5).len(), 0);
        assert_eq!(index.search("", 5).len(), 0);
    }

    #[test]
    fn synonyms_and_stems_find_desktop_tools() {
        let specs = vec![
            spec(
                "desktop.type_text",
                "Send keystrokes to the focused window",
                &["text"],
            ),
            spec(
                "desktop.launch_app",
                "Start an application by name",
                &["app"],
            ),
            spec("desktop.click", "Click at a screen position", &["x", "y"]),
            spec("fs.list_files", "List the files of a directory", &["path"]),
            spec(
                "desktop.list_windows",
                "List the open top-level windows",
                &[],
            ),
            spec("process.list", "List running processes", &[]),
            spec("misc.noise", "Unrelated helper", &[]),
        ];
        let index = ToolIndex::build(&specs);
        assert_eq!(top(&index, "keyboard"), "desktop.type_text");
        assert_eq!(top(&index, "klavye"), "desktop.type_text");
        assert_eq!(top(&index, "yaz"), "desktop.type_text");
        assert_eq!(top(&index, "typing"), "desktop.type_text");
        assert_eq!(top(&index, "başlat"), "desktop.launch_app");
        assert_eq!(top(&index, "tıkla"), "desktop.click");
        assert_eq!(top(&index, "dosyalar"), "fs.list_files");
        assert_eq!(top(&index, "pencere"), "desktop.list_windows");
        assert_eq!(top(&index, "işlem"), "process.list");
        assert_eq!(index.search("xyzzy", 3).len(), 0);
    }

    #[test]
    fn browser_query_puts_open_first_with_builtin_and_mcp_families() {
        let specs = vec![
            spec("browser_open", "Open a URL in the browser", &["url"]),
            spec("browser_click", "Click an element on the page", &["ref"]),
            spec(
                "mcp:playwright/browser_resize",
                "Resize the browser window",
                &["width", "height"],
            ),
            spec(
                "mcp:playwright/browser_navigate",
                "Navigate to a URL",
                &["url"],
            ),
            spec("mcp:playwright/browser_close", "Close the page", &[]),
            spec("fs_read", "Read a file", &["path"]),
        ];
        let index = ToolIndex::build(&specs);
        for q in ["browser", "open website", "youtube"] {
            assert_eq!(top(&index, q), "browser_open", "query {q}");
        }
        let names: Vec<_> = index
            .search("browser", 6)
            .into_iter()
            .map(|h| h.name)
            .collect();
        let pos = |n: &str| names.iter().position(|x| x == n).unwrap();
        assert!(pos("mcp:playwright/browser_navigate") < pos("mcp:playwright/browser_resize"));
        // Without the builtin family the MCP navigate tool still wins over resize.
        let mcp_only = ToolIndex::build(&specs[2..5]);
        assert_eq!(top(&mcp_only, "browser"), "mcp:playwright/browser_navigate");
    }
}
