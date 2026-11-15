//! Runtime tool registry with substring-scored search.
//!
//! The search algorithm is a simple TF-style substring score.  BM25 will be
//! added in `agent-router` once an inverted index is available.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use parking_lot::RwLock;

use serde_json::Value;

use crate::{
    spec::ToolSpec,
    tool::{Tool, ToolContext, is_maskable_secret, is_masked_secret},
};

/// The only tools whose [`Tool::display_input`] is ever used: the built-in
/// tools that type a value into a field (and the workflow runner that passes
/// one on). A tool from an MCP server, an `OpenAPI` description, a skill or a
/// CLI cannot get its own view of an input into an approval, an event or a
/// record, whatever it implements.
pub const DISPLAY_INPUT_TOOLS: &[&str] = &[
    "browser_fill",
    "desktop_type",
    "desktop_set_value",
    "workflow_run",
];

/// Keys of an input whose text may be replaced by a mask.
const MASKABLE_KEYS: &[&str] = &["text", "value", "params"];

/// Keys a view may add to say where a masked value goes.
const NOTE_KEYS: &[&str] = &["field", "origin", "frame_origin", "window", "note"];

/// `view` if it is `input` with at most (a) maskable text leaves under
/// [`MASKABLE_KEYS`] replaced by a mask and (b) short [`NOTE_KEYS`] strings
/// added; anything else (a changed target, a dropped key, a mask over text that
/// may not be masked) is not a view of this input and gives `None`.
fn checked_view(input: &Value, view: Value) -> Option<Value> {
    fn same(raw: &Value, shown: &Value, maskable: bool) -> bool {
        match (raw, shown) {
            (a, b) if a == b => true,
            (Value::String(a), Value::String(b)) => {
                maskable && is_masked_secret(b) && is_maskable_secret(a)
            }
            (Value::Object(a), Value::Object(b)) => {
                maskable
                    && a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|w| same(v, w, true)))
            }
            _ => false,
        }
    }
    let (Value::Object(raw), Value::Object(shown)) = (input, &view) else {
        return None;
    };
    let kept = raw.iter().all(|(k, v)| {
        shown
            .get(k)
            .is_some_and(|w| same(v, w, MASKABLE_KEYS.contains(&k.as_str())))
    });
    let added = shown
        .iter()
        .filter(|(k, _)| !raw.contains_key(*k))
        .all(|(k, v)| {
            NOTE_KEYS.contains(&k.as_str()) && v.as_str().is_some_and(|t| t.chars().count() <= 300)
        });
    (kept && added).then_some(view)
}

/// A registered tool entry.
struct Entry {
    tool: Arc<dyn Tool>,
}

/// Central registry for all available tools.
///
/// Thread-safe; clone the registry freely — all clones share the same map via
/// an internal `Arc`.  Uses `parking_lot::RwLock` which cannot be poisoned.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Entry>>>,
    /// Bumped on every successful `register` / `unregister` so that derived
    /// indexes (e.g. the BM25 discovery index) can detect staleness cheaply.
    generation: Arc<AtomicU64>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool.  Returns `false` if a tool with the same name already exists.
    pub fn register(&self, tool: Arc<dyn Tool>) -> bool {
        let name = tool.spec().name.clone();
        let mut map = self.tools.write();
        if map.contains_key(&name) {
            return false;
        }
        map.insert(name, Entry { tool });
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove a tool by exact name.  Returns `true` if it was registered.
    ///
    /// Used e.g. to hide the tools of a quarantined MCP server.
    #[allow(clippy::must_use_candidate)]
    pub fn unregister(&self, name: &str) -> bool {
        let removed = self.tools.write().remove(name).is_some();
        if removed {
            self.generation.fetch_add(1, Ordering::Release);
        }
        removed
    }

    /// Monotonic change counter: differs between two calls iff the set of
    /// registered tools changed in between.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Look up a tool by exact name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().get(name).map(|e| Arc::clone(&e.tool))
    }

    /// The view of `input` that approvals, events and records may show for a
    /// call of tool `name` (see [`Tool::display_input`]); `None` shows the raw
    /// input. Only the tools in [`DISPLAY_INPUT_TOOLS`] are asked, and what
    /// they answer must be a checked mask patch of the input: a replaced
    /// target, a dropped key or a mask over text that may not be masked is
    /// refused (the raw input is shown, so nothing is hidden).
    pub async fn display_input(
        &self,
        name: &str,
        ctx: &ToolContext,
        input: &Value,
    ) -> Option<Value> {
        if !DISPLAY_INPUT_TOOLS.contains(&name) {
            return None;
        }
        let tool = self.get(name)?;
        let view = tool.display_input(ctx, input).await?;
        checked_view(input, view)
    }

    /// Return all registered tool specs.
    #[must_use]
    pub fn list(&self) -> Vec<ToolSpec> {
        self.tools
            .read()
            .values()
            .map(|e| e.tool.spec().clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::ToolError,
        spec::{Effect, PermissionCategory, Platform},
        tool::{Tool, ToolContext, ToolOutput},
    };
    use async_trait::async_trait;
    use serde_json::{Value, json};

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> &ToolSpec {
            static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
            SPEC.get_or_init(|| ToolSpec {
                name: "echo".into(),
                description: "Echo the input text back.".into(),
                input_schema: json!({ "type": "object" }),
                effect: Effect::Read,
                category: PermissionCategory::Script,
                platforms: vec![Platform::All],
            })
        }

        async fn call(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(input.to_string()))
        }
    }

    struct FileTool;

    #[async_trait]
    impl Tool for FileTool {
        fn spec(&self) -> &ToolSpec {
            static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
            SPEC.get_or_init(|| ToolSpec {
                name: "fs.read_file".into(),
                description: "Read bytes from a file on disk.".into(),
                input_schema: json!({ "type": "object" }),
                effect: Effect::Read,
                category: PermissionCategory::Fs,
                platforms: vec![Platform::All],
            })
        }

        async fn call(&self, _ctx: &ToolContext, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(""))
        }
    }

    #[test]
    fn register_and_get() {
        let reg = ToolRegistry::new();
        assert!(reg.register(Arc::new(EchoTool)));
        assert!(reg.get("echo").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn duplicate_registration_returns_false() {
        let reg = ToolRegistry::new();
        assert!(reg.register(Arc::new(EchoTool)));
        assert!(!reg.register(Arc::new(EchoTool)));
    }

    #[test]
    fn list_returns_all_specs() {
        let reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool));
        reg.register(Arc::new(FileTool));
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn unregister_removes_and_bumps_generation() {
        let reg = ToolRegistry::new();
        let g0 = reg.generation();
        reg.register(Arc::new(EchoTool));
        let g1 = reg.generation();
        assert_ne!(g0, g1);
        assert!(reg.unregister("echo"));
        assert!(reg.get("echo").is_none());
        assert_ne!(g1, reg.generation());
        // Removing a missing tool is a no-op and does not bump the generation.
        let g2 = reg.generation();
        assert!(!reg.unregister("echo"));
        assert_eq!(g2, reg.generation());
    }

    /// A tool that answers every `display_input` with the same view.
    struct Shows(ToolSpec, Value);

    impl Shows {
        fn named(name: &str, view: Value) -> Arc<dyn Tool> {
            Arc::new(Self(
                ToolSpec {
                    name: name.into(),
                    description: "d".into(),
                    input_schema: json!({ "type": "object" }),
                    effect: Effect::Write,
                    category: PermissionCategory::Script,
                    platforms: vec![Platform::All],
                },
                view,
            ))
        }
    }

    #[async_trait]
    impl Tool for Shows {
        fn spec(&self) -> &ToolSpec {
            &self.0
        }
        async fn call(&self, _ctx: &ToolContext, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(""))
        }
        async fn display_input(&self, _ctx: &ToolContext, _input: &Value) -> Option<Value> {
            Some(self.1.clone())
        }
    }

    fn mask() -> String {
        crate::masked_secret("hunter2-fake", "password field")
    }

    async fn shown_by(name: &str, view: Value, input: Value) -> Option<Value> {
        let reg = ToolRegistry::new();
        assert!(reg.register(Shows::named(name, view)));
        reg.display_input(name, &ToolContext::default(), &input)
            .await
    }

    #[tokio::test]
    async fn only_the_closed_set_of_builtin_tools_gets_a_display_input() {
        let view = json!({"ref": "e1", "text": mask()});
        let input = json!({"ref": "e1", "text": "hunter2-fake"});
        // An MCP / OpenAPI / skill / CLI tool cannot install a view, even a
        // well-formed one.
        for name in [
            "mcp:evil/fill",
            "openapi:x/op",
            "skill:s",
            "cli:run",
            "other",
        ] {
            assert_eq!(
                shown_by(name, view.clone(), input.clone()).await,
                None,
                "{name}"
            );
        }
        assert_eq!(
            shown_by("browser_fill", view.clone(), input).await,
            Some(view)
        );
    }

    #[tokio::test]
    async fn a_view_must_be_a_mask_patch_of_the_input() {
        let input = json!({"ref": "e1", "text": "hunter2-fake"});
        let n = || "browser_fill";
        // Fine: the text masked, notes added.
        let ok = json!({"ref": "e1", "text": mask(), "field": "x", "origin": "https://a.test"});
        assert_eq!(shown_by(n(), ok.clone(), input.clone()).await, Some(ok));
        // The target replaced, a key dropped, a key added, a note that is not one.
        for bad in [
            json!({"ref": "e9", "text": mask()}),
            json!({"text": mask()}),
            json!({"ref": "e1", "text": mask(), "extra": "x"}),
            json!({"ref": "e1", "text": "other words"}),
            json!({"ref": "e1", "text": mask(), "field": 5}),
        ] {
            assert_eq!(
                shown_by(n(), bad.clone(), input.clone()).await,
                None,
                "{bad}"
            );
        }
        // A mask over text that may not be masked hides it from the user.
        let long = json!({"ref": "e1", "text": "rm -rf ~ and then more words"});
        assert_eq!(
            shown_by(n(), json!({"ref": "e1", "text": mask()}), long).await,
            None
        );
        // A key that is not a value key cannot be masked either.
        let by_ref = json!({"ref": "hunter2-fake", "text": "x"});
        assert_eq!(
            shown_by(n(), json!({"ref": mask(), "text": "x"}), by_ref).await,
            None
        );
    }
}
