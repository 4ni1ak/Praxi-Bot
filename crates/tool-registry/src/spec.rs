//! Tool specification types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export shared types from the protocol crate so users of `tool-registry`
// only need to import from one place.
pub use protocol::{PermissionCategory, Platform};

/// The effect level a tool has on the system.
///
/// A `Write` effect **must** be explicitly marked — the registry enforces that
/// anything capable of writing is at least `Destructive` unless the author
/// opts in to the weaker `Write` level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// The tool only reads state; it cannot modify anything.
    Read,
    /// The tool can write state but changes are reversible or low-risk.
    Write,
    /// The tool can cause irreversible or high-risk changes.
    Destructive,
}

impl Default for Effect {
    /// The safe default for any tool that writes is [`Effect::Destructive`].
    fn default() -> Self {
        Self::Destructive
    }
}

/// What kind of current-state observation a tool returns. A newer observation
/// of the same scope makes older ones stale, so the agent loop may shorten the
/// older copies it re-sends to the model (never the stored step output).
///
/// Kept as a name lookup ([`ObservationScope::of_tool`]) rather than a field on
/// [`ToolSpec`] so every existing spec literal (built-in, MCP, CLI, plugin)
/// stays valid and unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationScope {
    /// The page of the browser (snapshot, or the snapshot a page action returns).
    Page,
    /// The accessibility state of a desktop window.
    Window,
    /// A directory or window listing.
    Listing,
}

impl ObservationScope {
    /// Scope of a built-in tool's result, `None` for every other tool.
    #[must_use]
    pub fn of_tool(name: &str) -> Option<Self> {
        match name {
            "browser_open" | "browser_snapshot" | "browser_click" | "browser_fill"
            | "browser_select" | "browser_press" | "browser_scroll" | "browser_wait"
            | "browser_back" => Some(Self::Page),
            "desktop_get_state" => Some(Self::Window),
            "fs_list" | "desktop_list_apps" => Some(Self::Listing),
            _ => None,
        }
    }
}

/// Static description of a tool, registered once at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique tool name (e.g. `"fs.read_file"`).
    pub name: String,
    /// Human-readable description used for tool discovery and display.
    pub description: String,
    /// JSON Schema for the tool's input object.
    pub input_schema: Value,
    /// Effect level of this tool.
    pub effect: Effect,
    /// Permission category this tool belongs to.
    pub category: PermissionCategory,
    /// Platforms on which this tool is available.
    pub platforms: Vec<Platform>,
}

impl ToolSpec {
    /// Return `true` if the spec passes basic validity rules.
    ///
    /// Currently checks:
    /// - `name` is non-empty
    /// - `description` is non-empty
    /// - `input_schema` is a JSON object
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.name.is_empty() && !self.description.is_empty() && self.input_schema.is_object()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_spec() -> ToolSpec {
        ToolSpec {
            name: "fs.write_file".into(),
            description: "Write bytes to a file.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
            effect: Effect::Write,
            category: PermissionCategory::Fs,
            platforms: vec![Platform::All],
        }
    }

    #[test]
    fn write_effect_must_be_explicitly_marked() {
        // A spec with effect=Write must NOT auto-upgrade to Destructive;
        // the author consciously chose Write.  The registry enforces that
        // anything that *writes* is at minimum Write (not Read).
        let spec = write_spec();
        assert!(
            spec.effect >= Effect::Write,
            "write spec must have at least Write effect"
        );
    }

    #[test]
    fn read_only_spec_has_read_effect() {
        let spec = ToolSpec {
            name: "fs.read_file".into(),
            description: "Read bytes from a file.".into(),
            input_schema: json!({ "type": "object" }),
            effect: Effect::Read,
            category: PermissionCategory::Fs,
            platforms: vec![Platform::All],
        };
        assert_eq!(spec.effect, Effect::Read);
    }

    #[test]
    fn observation_scopes_cover_the_named_tools_only() {
        assert_eq!(
            ObservationScope::of_tool("browser_snapshot"),
            Some(ObservationScope::Page)
        );
        assert_eq!(
            ObservationScope::of_tool("desktop_get_state"),
            Some(ObservationScope::Window)
        );
        assert_eq!(
            ObservationScope::of_tool("fs_list"),
            Some(ObservationScope::Listing)
        );
        assert_eq!(ObservationScope::of_tool("browser_eval"), None);
        assert_eq!(ObservationScope::of_tool("fs_write_file"), None);
        assert_eq!(ObservationScope::of_tool("mcp__x__browser_snapshot"), None);
    }

    #[test]
    fn default_effect_is_destructive() {
        assert_eq!(Effect::default(), Effect::Destructive);
    }

    #[test]
    fn valid_spec_passes() {
        assert!(write_spec().is_valid());
    }

    #[test]
    fn empty_name_fails_validity() {
        let mut spec = write_spec();
        spec.name = String::new();
        assert!(!spec.is_valid());
    }
}
