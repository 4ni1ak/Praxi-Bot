//! Document and data tools for the agent: PDF, DOCX, XLSX, CSV and JSON.
//!
//! Tools (all permission category `Fs`):
//!
//! | tool            | effect     | what it does                                       |
//! |-----------------|------------|----------------------------------------------------|
//! | `pdf_extract`   | read       | text per page, best-effort tables                  |
//! | `docx_read`     | read       | headings, paragraphs and tables of a `.docx`       |
//! | `docx_write`    | write      | create a `.docx` from headings/paragraphs/tables   |
//! | `xlsx_read`     | read       | sheets, ranges and typed cells (`.xlsx/.xlsb/.xls`) |
//! | `xlsx_write`    | write      | create an `.xlsx` with typed cells and formulas    |
//! | `csv_read`      | read       | paged CSV/TSV reading                              |
//! | `csv_transform` | read/write | filter / select / sort / group / limit pipeline    |
//! | `json_query`    | read       | JSONPath (RFC 9535) query over a file or inline    |
//!
//! Every tool returns two content parts: a compact markdown preview for the
//! model (`ContentPart::Text`) and the structured result (`ContentPart::Json`).
//!
//! # Permissions
//!
//! File paths are always passed in the `path` (and, for `csv_transform`, the
//! optional `output_path`) input fields, exactly like the `fs_*` tools of
//! `builtin-tools`. The permission engine gates them by category and effect
//! before `call`. `write_file` also refuses the daemon data directory and
//! native CLI configuration, so a caller that skips that gate still cannot
//! plant a skill or a workflow there. Write tools never overwrite an existing
//! file unless `overwrite: true` is given. Inputs larger than
//! [`DocToolsConfig::max_input_bytes`] are rejected.
//!
//! # Wiring
//!
//! ```ignore
//! doc_tools::register(&registry, &doc_tools::DocToolsConfig::default());
//! ```

// Flat declarative tool tables and dispatchers; short names (r, c, i) are idiomatic here.
#![allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::doc_markdown
)]

mod common;
mod csv_tools;
mod docx;
mod json_query;
mod pdf;
mod pdf_budget;
mod pdf_cmap;
mod xlsx;

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tool_registry::{
    Effect, PermissionCategory, Platform, Tool, ToolContext, ToolError, ToolOutput, ToolRegistry,
    ToolSpec,
};

pub use common::Reply;

/// Extract an already-authorized PDF or DOCX snapshot without reopening a path.
/// The caller owns path authorization and supplies a display label, never an
/// instruction. Both input and rendered output are bounded by `cfg`.
///
/// # Errors
/// Rejects unsupported formats, oversized input and malformed documents.
pub fn extract_bytes(
    cfg: &DocToolsConfig,
    extension: &str,
    label: &str,
    bytes: &[u8],
) -> Result<Reply, ToolError> {
    if bytes.len() as u64 > cfg.max_input_bytes {
        return Err(ToolError::InvalidInput(
            "document exceeds input limit".into(),
        ));
    }
    let mut reply = std::panic::catch_unwind(|| match extension {
        "pdf" => pdf::from_bytes(cfg, label, bytes),
        "docx" => docx::read_bytes(cfg, label, bytes),
        _ => Err(ToolError::InvalidInput(
            "unsupported document format".into(),
        )),
    })
    .map_err(|_| ToolError::Execution("document parser failed; source was not indexed".into()))??;
    if reply.data["truncated"].as_bool() == Some(true) {
        return Err(ToolError::InvalidInput(
            "document source exceeds extraction limits; no truncated snapshot was indexed".into(),
        ));
    }
    // Tool previews deliberately shorten tables/cells and prose. A knowledge
    // snapshot instead renders all retained structured text or rejects it.
    let complete = if extension == "pdf" {
        let mut text = format!("# {label}\n");
        for page in reply.data["pages"]
            .as_array()
            .ok_or_else(|| ToolError::Execution("missing PDF extraction pages".into()))?
        {
            if page.get("error").is_some() {
                return Err(ToolError::Execution(
                    "PDF source text extraction failed".into(),
                ));
            }
            let _ = write!(
                text,
                "\n## Page {}\n\n{}\n",
                page["page"],
                page["text"].as_str().unwrap_or_default()
            );
        }
        text
    } else {
        complete_docx_text(&reply.data)?
    };
    if complete.len() > cfg.max_output_bytes || complete.chars().count() > cfg.max_preview_chars {
        return Err(ToolError::InvalidInput(
            "document source exceeds extracted text limit; no truncated snapshot was indexed"
                .into(),
        ));
    }
    reply.markdown = complete;
    Ok(reply)
}

fn complete_docx_text(data: &Value) -> Result<String, ToolError> {
    use std::fmt::Write as _;
    let mut text = String::new();
    for block in data["blocks"]
        .as_array()
        .ok_or_else(|| ToolError::Execution("missing DOCX extraction blocks".into()))?
    {
        match block["type"].as_str() {
            Some("heading") => {
                let level = usize::try_from(block["level"].as_u64().unwrap_or(1))
                    .unwrap_or(1)
                    .clamp(1, 9);
                let _ = writeln!(
                    text,
                    "{} {}\n",
                    "#".repeat(level),
                    block["text"].as_str().unwrap_or_default()
                );
            }
            Some("paragraph") => {
                let _ = writeln!(text, "{}\n", block["text"].as_str().unwrap_or_default());
            }
            Some("table") => {
                let rows: Vec<Vec<String>> = serde_json::from_value(block["rows"].clone())
                    .map_err(|_| ToolError::Execution("invalid extracted DOCX table".into()))?;
                for (index, row) in rows.iter().enumerate() {
                    let cells: Vec<_> = row
                        .iter()
                        .map(|cell| cell.replace('|', "\\|").replace(['\r', '\n'], " "))
                        .collect();
                    let _ = writeln!(text, "| {} |", cells.join(" | "));
                    if index == 0 {
                        let _ = writeln!(text, "|{}|", vec![" --- "; row.len()].join("|"));
                    }
                }
                text.push('\n');
            }
            _ => return Err(ToolError::Execution("invalid extracted DOCX block".into())),
        }
    }
    Ok(text)
}

/// Limits applied by every doc tool.
#[derive(Debug, Clone)]
pub struct DocToolsConfig {
    /// Largest input file accepted, in bytes (default 25 MiB).
    pub max_input_bytes: u64,
    /// Most rows / items placed in the structured JSON result (default 500).
    pub max_rows: usize,
    /// Longest markdown preview in characters (default 6000).
    pub max_preview_chars: usize,
    /// Longest single cell / paragraph kept in structured output (default 4000).
    pub max_cell_chars: usize,
    /// Largest written output (docx/xlsx/csv), in bytes (default 25 MiB).
    /// Also bounds DOCX read JSON, which has an additional hard 1 MiB cap.
    pub max_output_bytes: usize,
    /// Extra check on every output path, run by the writer itself. The daemon
    /// sets it to refuse its data directory and native CLI configuration, so a
    /// caller that skips the permission check still cannot plant a skill or a
    /// workflow there. It is a hook, not a dependency: `builtin-tools` already
    /// depends on this crate's consumers, so the guard cannot be imported here.
    pub write_guard: Option<WriteGuard>,
}

type GuardFn = dyn Fn(&str) -> Result<(), String> + Send + Sync + std::panic::RefUnwindSafe;

/// A path check supplied by the embedder; `Err` carries the refusal reason.
#[derive(Clone)]
pub struct WriteGuard(Arc<GuardFn>);

impl WriteGuard {
    /// Wrap a check.
    pub fn new(
        check: impl Fn(&str) -> Result<(), String> + Send + Sync + std::panic::RefUnwindSafe + 'static,
    ) -> Self {
        Self(Arc::new(check))
    }

    pub(crate) fn check(&self, path: &str) -> Result<(), String> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for WriteGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriteGuard")
    }
}

impl Default for DocToolsConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: 25 * 1024 * 1024,
            max_rows: 500,
            max_preview_chars: 6000,
            max_cell_chars: 4000,
            max_output_bytes: 25 * 1024 * 1024,
            write_guard: None,
        }
    }
}

type Handler = fn(&DocToolsConfig, Value) -> Result<Reply, ToolError>;

struct DocTool {
    spec: ToolSpec,
    cfg: Arc<DocToolsConfig>,
    handler: Handler,
}

#[async_trait]
impl Tool for DocTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn call(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let cfg = self.cfg.clone();
        let handler = self.handler;
        let reply = tokio::task::spawn_blocking(move || handler(&cfg, input))
            .await
            .map_err(|e| ToolError::Execution(format!("doc tool task failed: {e}")))??;
        Ok(reply.into_output())
    }
}

/// Build all document tools.
#[must_use]
pub fn tools(cfg: &DocToolsConfig) -> Vec<Arc<dyn Tool>> {
    let cfg = Arc::new(cfg.clone());
    let defs: Vec<(ToolSpec, Handler)> = vec![
        (pdf::spec(), pdf::run),
        (docx::read_spec(), docx::run_read),
        (docx::write_spec(), docx::run_write),
        (xlsx::read_spec(), xlsx::run_read),
        (xlsx::write_spec(), xlsx::run_write),
        (csv_tools::read_spec(), csv_tools::run_read),
        (csv_tools::transform_spec(), csv_tools::run_transform),
        (json_query::spec(), json_query::run),
    ];
    defs.into_iter()
        .map(|(spec, handler)| {
            Arc::new(DocTool {
                spec,
                cfg: cfg.clone(),
                handler,
            }) as Arc<dyn Tool>
        })
        .collect()
}

/// Register every document tool in `registry`.
pub fn register(registry: &ToolRegistry, cfg: &DocToolsConfig) {
    for tool in tools(cfg) {
        registry.register(tool);
    }
}

/// Names of all tools in this crate.
pub const TOOL_NAMES: [&str; 8] = [
    "pdf_extract",
    "docx_read",
    "docx_write",
    "xlsx_read",
    "xlsx_write",
    "csv_read",
    "csv_transform",
    "json_query",
];

pub(crate) fn mk_spec(
    name: &str,
    description: &str,
    input_schema: Value,
    effect: Effect,
) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        effect,
        category: PermissionCategory::Fs,
        platforms: vec![Platform::All],
        content: match name {
            // Documents can carry prompt injection and are private files.
            "pdf_extract" | "docx_read" | "xlsx_read" | "csv_read" | "csv_transform"
            | "json_query" => tool_registry::ContentClass::untrusted_private(),
            // The model already holds the document it is writing.
            "docx_write" | "xlsx_write" => tool_registry::ContentClass::none(),
            _ => tool_registry::ContentClass::unspecified(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_tools_with_valid_specs() {
        let reg = ToolRegistry::new();
        register(&reg, &DocToolsConfig::default());
        for name in TOOL_NAMES {
            let t = reg.get(name).unwrap_or_else(|| panic!("{name} missing"));
            assert!(t.spec().is_valid(), "{name}");
        }
        assert_eq!(reg.get("docx_write").unwrap().spec().effect, Effect::Write);
        assert!(!reg.get("docx_write").unwrap().spec().content.reads_private);
        assert!(!reg.get("xlsx_write").unwrap().spec().content.reads_private);
        assert_eq!(reg.get("pdf_extract").unwrap().spec().effect, Effect::Read);
    }

    #[test]
    fn write_file_refuses_the_daemon_data_directory() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().to_path_buf();
        let cfg = DocToolsConfig {
            write_guard: Some(WriteGuard::new(move |path| {
                if std::path::Path::new(path).starts_with(&root) {
                    Err("daemon data is not writable".into())
                } else {
                    Ok(())
                }
            })),
            ..DocToolsConfig::default()
        };
        let inside = data.path().join("skills.md");
        let err = crate::common::write_file(&cfg, inside.to_str().unwrap(), b"hi", true)
            .expect_err("data dir");
        assert!(err.to_string().contains("daemon"), "{err}");
        assert!(!inside.exists());
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("ok.txt");
        crate::common::write_file(&cfg, path.to_str().unwrap(), b"hi", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hi");
    }
}

#[cfg(test)]
mod call_tests {
    use super::*;
    use serde_json::json;
    use tool_registry::ContentPart;

    #[tokio::test]
    async fn call_returns_markdown_and_json_parts() {
        let reg = ToolRegistry::new();
        register(&reg, &DocToolsConfig::default());
        let tool = reg.get("json_query").unwrap();
        let out = tool
            .call(
                &ToolContext::default(),
                json!({"json": {"a": [1, 2]}, "query": "$.a[*]"}),
            )
            .await
            .unwrap();
        assert!(matches!(&out.content[0], ContentPart::Text { text } if text.contains("2 match")));
        assert!(matches!(&out.content[1], ContentPart::Json { data } if data["total"] == 2));
        let err = tool
            .call(&ToolContext::default(), json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
