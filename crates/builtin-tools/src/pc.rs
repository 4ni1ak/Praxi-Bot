//! The user's own computer as a tool target of a server-homed bot (card 272).
//!
//! A bot's tools act on ITS computer by default. Only when the user asks for
//! something on THEIR computer does the model reach for the `pc_*` family, which
//! this module builds. Everything in the family goes over a [`PcLink`] (the
//! existing paired link to the user's desktop app) and is a SEPARATE set of tool
//! names: a grant, a "this chat" allowance or a control grant given to
//! `shell_run` / `desktop_type` / `browser_open` (the bot's own computer) is
//! keyed by those names and so can never authorise `pc_shell_run` /
//! `pc_desktop_type` / `pc_browser_open`, and the other way round. The PC then
//! runs its own consent path (owner switch per family, cards, latch) on top of
//! whatever the server side decided.
//!
//! The PC-side counterpart lives in `apps/cli/src/pc_target.rs`; the wire names
//! are the `PC_*` constants below.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use desktop_api::{
    Action, ActionResult, Capabilities, DesktopBackend, DesktopError, Image, Quality, Region,
    Snapshot, WindowInfo,
    snapshot::{AppSpec, Pid},
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tool_registry::{
    ContentClass, Effect, PermissionCategory, Platform, StandingGrant, Tool, ToolContext,
    ToolError, ToolOutput, ToolSpec, ToolWrapper,
};

use crate::browser::{BrowserDriver, Wait};

/// Prefix of every tool of the family.
pub const PC_PREFIX: &str = "pc_";

/// `pc.info`: the computer's OS, shell, home and which families the owner allowed.
pub const RPC_INFO: &str = "pc.info";
/// `pc.tool`: run one of [`SERVED_TOOLS`] on the computer (`{name, input}`).
pub const RPC_TOOL: &str = "pc.tool";
/// `pc.browser`: one browser operation (`{op, ..}`).
pub const RPC_BROWSER: &str = "pc.browser";

/// Tools the computer serves through [`RPC_TOOL`]; nothing else is reachable.
pub const SERVED_TOOLS: [&str; 4] = ["shell_run", "fs_read", "fs_list", "fs_write"];

/// How long a call that waits for the person at the computer may take (the card
/// wait of 120 s plus the work itself).
pub const PC_CALL_TIMEOUT: Duration = Duration::from_secs(150);
/// Deadline of a call that never asks (info, a read once allowed).
pub const PC_QUICK_TIMEOUT: Duration = Duration::from_secs(20);

/// The four things the owner switches on separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcFamily {
    /// Looking at the screen, clicking, typing, launching programs.
    Screen,
    /// The computer's managed browser.
    Browser,
    /// Shell commands.
    Commands,
    /// Reading files; writing asks every time.
    Files,
}

impl PcFamily {
    /// Wire and settings name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Browser => "browser",
            Self::Commands => "commands",
            Self::Files => "files",
        }
    }

    /// Family a served tool belongs to.
    #[must_use]
    pub fn of_tool(name: &str) -> Option<Self> {
        match name {
            "shell_run" => Some(Self::Commands),
            "fs_read" | "fs_list" | "fs_write" => Some(Self::Files),
            _ => None,
        }
    }
}

/// What the computer reports about itself (`pc.info`).
// One flag per family the owner switches separately.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcInfo {
    /// `windows`, `macos` or `linux`.
    pub os: String,
    /// The shell `pc_shell_run` uses there (PowerShell on Windows).
    pub shell: String,
    /// The user's home folder on that computer.
    pub home: Option<String>,
    pub hostname: Option<String>,
    pub screen: bool,
    pub browser: bool,
    pub commands: bool,
    pub files: bool,
}

/// Why the computer cannot be used right now. `Display` is the sentence the
/// bot relays to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcError {
    /// No computer of the user is paired with this server.
    NotPaired,
    /// Paired, but not connected (the computer or the Praxi app is off).
    Offline,
    /// The computer's own refusal (switch off, owner denied, ...), in words.
    Refused(String),
    /// A transport or tool failure.
    Failed(String),
}

impl std::fmt::Display for PcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPaired => f.write_str(
                "No computer of the user is connected to this server, so their computer cannot be used. Tell the user to pair their computer in the Praxi app.",
            ),
            Self::Offline => f.write_str(
                "The user's computer is off or the Praxi app is not running on it, so it cannot be reached right now. Tell the user to turn it on and open Praxi, then try again.",
            ),
            Self::Refused(why) => write!(f, "The user's computer refused: {why}"),
            Self::Failed(why) => write!(f, "The user's computer did not complete this: {why}"),
        }
    }
}

/// The paired link to the user's computer, as the tools see it.
#[async_trait]
pub trait PcLink: Send + Sync {
    /// One RPC to the computer.
    async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, PcError>;
    /// The computer's desktop (observe, input, launch) over the same link.
    ///
    /// # Errors
    /// Why the computer cannot be used right now.
    fn desktop(&self) -> Result<Arc<dyn DesktopBackend>, PcError>;
}

/// A late-bound [`PcLink`]: the tools are registered before the peer stack
/// exists, the link is installed once it does. Empty = nothing paired.
#[derive(Default)]
pub struct PcLinkSlot {
    inner: RwLock<Option<Arc<dyn PcLink>>>,
}

impl PcLinkSlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, link: Arc<dyn PcLink>) {
        *self.inner.write() = Some(link);
    }

    fn get(&self) -> Result<Arc<dyn PcLink>, PcError> {
        self.inner.read().clone().ok_or(PcError::NotPaired)
    }
}

#[async_trait]
impl PcLink for PcLinkSlot {
    async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, PcError> {
        self.get()?.call(method, params, timeout).await
    }

    fn desktop(&self) -> Result<Arc<dyn DesktopBackend>, PcError> {
        self.get()?.desktop()
    }
}

// ── The family's tools ───────────────────────────────────────────────────────

/// Replace the names of the bot's own tools in text the model reads, so the
/// descriptions of the copies point at the `pc_*` copies.
fn retarget(text: &str) -> String {
    text.replace("desktop_", "pc_desktop_")
        .replace("browser_", "pc_browser_")
}

/// A tool of the bot's own computer re-registered under `pc_<name>`, acting on
/// the user's computer through the backend it was built with.
struct Renamed {
    inner: Arc<dyn Tool>,
    spec: ToolSpec,
}

impl Renamed {
    fn wrap(inner: Arc<dyn Tool>) -> Arc<dyn Tool> {
        let mut spec = inner.spec().clone();
        spec.name = format!("{PC_PREFIX}{}", spec.name);
        spec.description = format!(
            "[ON THE USER'S COMPUTER, not yours; only when the user asked for something on THEIR computer] {}",
            retarget(&spec.description)
        );
        if let Ok(text) = serde_json::to_string(&spec.input_schema)
            && let Ok(schema) = serde_json::from_str(&retarget(&text))
        {
            spec.input_schema = schema;
        }
        Arc::new(Self { inner, spec })
    }
}

#[async_trait]
impl ToolWrapper for Renamed {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn egress_for(&self, bot_id: Option<&str>) -> Option<bool> {
        self.inner.egress_for(bot_id)
    }
    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        // The user's computer never takes a run-scoped control grant or a page
        // allowance of the bot's own computer.
        let ctx = ToolContext {
            run_id: ctx.run_id.clone(),
            max_inline_bytes: ctx.max_inline_bytes,
            bot_id: ctx.bot_id.clone(),
            content_tainted: ctx.content_tainted,
            exposure_untrusted: ctx.exposure_untrusted,
            expected_origin: None,
            control_guard: None,
            lane: ctx.lane.clone(),
        };
        self.inner.call(&ctx, input).await
    }
    fn always_ask(&self) -> bool {
        self.inner.always_ask()
    }
    fn always_ask_for(&self, input: &Value) -> bool {
        self.inner.always_ask_for(input)
    }
    async fn standing_grant(&self, _ctx: &ToolContext, _input: &Value) -> Option<StandingGrant> {
        // Owner standing grants are written for the bot's own computer.
        None
    }
    async fn approval_details(
        &self,
        ctx: &ToolContext,
        input: &Value,
    ) -> Result<Option<Value>, ToolError> {
        self.inner.approval_details(ctx, input).await
    }
    fn approval_closed(&self, ctx: &ToolContext, input: &Value, token: Option<&str>) {
        self.inner.approval_closed(ctx, input, token);
    }
}

/// The user's desktop as a [`DesktopBackend`]; a refusal or an offline
/// computer is the error text the model reads.
struct PcDesktop {
    link: Arc<dyn PcLink>,
}

impl PcDesktop {
    fn backend(&self) -> Result<Arc<dyn DesktopBackend>, DesktopError> {
        self.link
            .desktop()
            .map_err(|e| DesktopError::Unsupported(e.to_string()))
    }
}

#[async_trait]
impl DesktopBackend for PcDesktop {
    async fn snapshot(&self, scope: Option<u64>) -> Result<Snapshot, DesktopError> {
        self.backend()?.snapshot(scope).await
    }
    async fn act(&self, action: Action) -> Result<ActionResult, DesktopError> {
        self.backend()?.act(action).await
    }
    async fn screenshot(&self, region: Region, quality: Quality) -> Result<Image, DesktopError> {
        self.backend()?.screenshot(region, quality).await
    }
    async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        self.backend()?.windows().await
    }
    async fn launch(&self, app: AppSpec) -> Result<Pid, DesktopError> {
        self.backend()?.launch(app).await
    }
    fn capabilities(&self) -> Capabilities {
        self.backend().map_or(
            Capabilities {
                platform: "unknown".into(),
                accessibility_tree: false,
                screenshot: false,
                app_launch: false,
                has_display: false,
            },
            |b| b.capabilities(),
        )
    }
}

/// A paired computer's managed browser as a [`BrowserDriver`]: each call is
/// one `pc.browser` RPC. Typing owner secrets and page scripts are not served.
pub struct RemoteBrowserDriver {
    link: Arc<dyn PcLink>,
}

impl RemoteBrowserDriver {
    /// The user's own computer.
    #[must_use]
    pub fn new(link: Arc<dyn PcLink>) -> Self {
        Self { link }
    }

    async fn op(&self, op: &str, mut args: Value) -> Result<Value, String> {
        args["op"] = json!(op);
        self.link
            .call(RPC_BROWSER, args, PC_CALL_TIMEOUT)
            .await
            .map_err(|e| e.to_string())
    }

    async fn text(&self, op: &str, args: Value) -> Result<String, String> {
        let v = self.op(op, args).await?;
        v.get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the computer sent an unreadable browser answer".to_owned())
    }
}

#[async_trait]
impl BrowserDriver for RemoteBrowserDriver {
    async fn open(&self, url: &str, _profile: Option<&str>) -> Result<String, String> {
        self.text("open", json!({"url": url})).await
    }
    async fn open_opts(
        &self,
        url: &str,
        _profile: Option<&str>,
        _dialog: Option<&str>,
        _prompt_text: Option<&str>,
    ) -> Result<String, String> {
        self.open(url, None).await
    }
    async fn snapshot(&self, interactive_only: bool) -> Result<String, String> {
        self.text("snapshot", json!({"interactive_only": interactive_only}))
            .await
    }
    async fn click(&self, r: &str) -> Result<String, String> {
        self.text("click", json!({"ref": r})).await
    }
    async fn fill(&self, r: &str, text: &str) -> Result<String, String> {
        self.text("fill", json!({"ref": r, "text": text})).await
    }
    async fn select(&self, r: &str, value: &str) -> Result<String, String> {
        self.text("select", json!({"ref": r, "value": value})).await
    }
    async fn press(&self, key: &str) -> Result<String, String> {
        self.text("press", json!({"key": key})).await
    }
    async fn scroll(&self, r: Option<&str>, dx: f64, dy: f64) -> Result<String, String> {
        self.text("scroll", json!({"ref": r, "dx": dx, "dy": dy}))
            .await
    }
    async fn wait(&self, cond: Wait, timeout: Option<Duration>) -> Result<String, String> {
        let cond = match cond {
            Wait::Text(t) => json!({"text": t}),
            Wait::Selector(s) => json!({"selector": s}),
            Wait::Idle => json!({"idle": true}),
        };
        self.text(
            "wait",
            json!({"cond": cond, "timeout_ms": timeout.map(|t| u64::try_from(t.as_millis()).unwrap_or(u64::MAX))}),
        )
        .await
    }
    async fn back(&self) -> Result<String, String> {
        self.text("back", json!({})).await
    }
    async fn tabs(&self, switch_to: Option<usize>) -> Result<String, String> {
        self.text("tabs", json!({"switch_to": switch_to})).await
    }
    async fn screenshot(
        &self,
        full_page: bool,
        element: Option<&str>,
    ) -> Result<(Vec<u8>, &'static str), String> {
        use base64::Engine as _;
        let v = self
            .op(
                "screenshot",
                json!({"full_page": full_page, "element": element}),
            )
            .await?;
        let data = v
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| "the computer sent no image".to_owned())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("bad image data from the computer: {e}"))?;
        let ext = match v.get("ext").and_then(Value::as_str) {
            Some("jpg") => "jpg",
            _ => "png",
        };
        Ok((bytes, ext))
    }
    async fn eval(&self, _js: &str) -> Result<Value, String> {
        Err("running page scripts on the user's computer is not allowed".into())
    }
    async fn current_url(&self) -> Option<String> {
        self.op("current_url", json!({}))
            .await
            .ok()?
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}

/// `pc_info`: what the computer is, so the model writes its commands and paths
/// for the right system, and which families the owner allowed.
struct InfoTool {
    spec: ToolSpec,
    link: Arc<dyn PcLink>,
}

#[async_trait]
impl Tool for InfoTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn call(&self, _ctx: &ToolContext, _input: Value) -> Result<ToolOutput, ToolError> {
        let v = self
            .link
            .call(RPC_INFO, Value::Null, PC_QUICK_TIMEOUT)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let info: PcInfo = serde_json::from_value(v)
            .map_err(|_| ToolError::Execution("the computer sent unreadable facts".into()))?;
        Ok(ToolOutput::text(info_text(&info)))
    }
}

/// The facts of a computer as text; every field is clipped (they come from
/// another machine).
#[must_use]
pub fn info_text(info: &PcInfo) -> String {
    fn clip(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect::<String>()
    }
    let yn = |b: bool| if b { "allowed" } else { "NOT allowed" };
    format!(
        "The user's computer: {} ({}); commands run in {}; home folder: {}.\nScreen and input: {}. Browser: {}. Commands: {}. Files: {}.\nUse only {} commands and paths for it. An action that is not allowed fails at once; tell the user to allow it in Settings > Computer.",
        clip(&info.os),
        clip(info.hostname.as_deref().unwrap_or("unnamed")),
        clip(&info.shell),
        clip(info.home.as_deref().unwrap_or("unknown")),
        yn(info.screen),
        yn(info.browser),
        yn(info.commands),
        yn(info.files),
        clip(&info.os),
    )
}

/// A served tool (`shell_run`, `fs_*`) re-registered as `pc_<name>`: the call
/// is one [`RPC_TOOL`] to the computer, which runs the real tool there.
struct RemoteTool {
    spec: ToolSpec,
    remote_name: &'static str,
    link: Arc<dyn PcLink>,
}

#[async_trait]
impl Tool for RemoteTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    async fn call(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let v = self
            .link
            .call(
                RPC_TOOL,
                json!({"name": self.remote_name, "input": input}),
                PC_CALL_TIMEOUT,
            )
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let mut out: ToolOutput = serde_json::from_value(v)
            .map_err(|_| ToolError::Execution("the computer sent an unreadable result".into()))?;
        // A spill file would name a path on the other machine.
        if out.spill.take().is_some() {
            out.content.push(tool_registry::ContentPart::Text {
                text: "[the result was longer than shown; ask for a smaller part]".into(),
            });
        }
        // Whatever the other machine says about the trust of its own output is
        // not taken over; the declared class of this tool applies.
        out.exposure = None;
        Ok(out)
    }
}

fn remote_tool(inner: &Arc<dyn Tool>, name: &'static str, link: &Arc<dyn PcLink>) -> Arc<dyn Tool> {
    let mut spec = inner.spec().clone();
    spec.name = format!("{PC_PREFIX}{name}");
    spec.description = format!(
        "[ON THE USER'S COMPUTER, not yours; only when the user asked for something on THEIR computer; call pc_info first to learn its OS] {}",
        spec.description
    );
    // The classes of the user's machine: its files and command output are
    // untrusted and private content; a command starts a program there.
    spec.content = match name {
        "shell_run" => ContentClass::all().executes(),
        "fs_write" => ContentClass::none(),
        _ => ContentClass::untrusted_private(),
    };
    Arc::new(RemoteTool {
        spec,
        remote_name: name,
        link: Arc::clone(link),
    })
}

/// The whole family for the model. `data_dir` holds the screenshots of the
/// desktop tools. Built once; every call resolves the link when it runs.
#[must_use]
pub fn pc_tools(link: &Arc<dyn PcLink>, data_dir: &Path) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    out.push(Arc::new(InfoTool {
        spec: ToolSpec {
            name: "pc_info".into(),
            description: "Facts about the USER'S computer: its operating system, shell, home folder and which of screen/browser/commands/files the owner allowed. Call it before using any other pc_* tool. Example: {}".into(),
            input_schema: json!({"type":"object","properties":{}}),
            effect: Effect::Read,
            category: PermissionCategory::Meta,
            platforms: vec![Platform::All],
            content: ContentClass::none(),
        },
        link: Arc::clone(link),
    }));
    // Screen and input: the same tools as the bot's own desktop, on the PC's
    // screen, with their own observation state.
    let backend: Arc<dyn DesktopBackend> = Arc::new(PcDesktop {
        link: Arc::clone(link),
    });
    out.extend(
        crate::desktop::tools_dynamic_with_secrets(
            Some(backend),
            None,
            &data_dir.join("pc"),
            crate::DesktopSettings::default(),
            None,
            None,
        )
        .into_iter()
        .map(Renamed::wrap),
    );
    // Browser: the computer's managed browser (no page scripts, no secrets).
    let driver: Arc<dyn BrowserDriver> = Arc::new(RemoteBrowserDriver::new(Arc::clone(link)));
    out.extend(
        crate::browser::tools_with_secrets(driver, &data_dir.join("pc"), None, None)
            .into_iter()
            .filter(|t| t.spec().name != "browser_eval")
            .map(Renamed::wrap),
    );
    // Commands and files: specs of the real tools, executed over there.
    for inner in local_tools(data_dir) {
        let name = inner.spec().name.clone();
        if let Some(served) = SERVED_TOOLS.iter().find(|n| **n == name) {
            out.push(remote_tool(&inner, served, link));
        }
    }
    out
}

/// The real `shell_run` and `fs_*` tools a computer serves: `fs_*` may reach
/// anything the user can read except credentials, other tools' configuration,
/// Praxi's own data and OS-critical places (the "This computer" scope).
#[must_use]
pub fn local_tools(data_dir: &Path) -> Vec<Arc<dyn Tool>> {
    struct Computer;
    impl crate::FsAccessProvider for Computer {
        fn scope(&self, _bot_id: Option<&str>) -> crate::FsScope {
            crate::FsScope::Computer
        }
    }
    let mut out = crate::files::tools_scoped(
        data_dir,
        &[],
        &[],
        Some(Arc::new(Computer)),
        crate::user_home(),
        None,
    );
    out.push(crate::shell::tool_with_provenance(data_dir, None));
    out.retain(|t| SERVED_TOOLS.contains(&t.spec().name.as_str()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Off;

    #[async_trait]
    impl PcLink for Off {
        async fn call(&self, _: &str, _: Value, _: Duration) -> Result<Value, PcError> {
            Err(PcError::Offline)
        }
        fn desktop(&self) -> Result<Arc<dyn DesktopBackend>, PcError> {
            Err(PcError::Offline)
        }
    }

    /// The family has its own names, its descriptions point at them, and an
    /// offline computer is a plain sentence, not a hang.
    #[tokio::test]
    async fn the_family_is_separate_and_fails_fast_when_the_computer_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let link: Arc<dyn PcLink> = Arc::new(Off);
        let tools = pc_tools(&link, dir.path());
        let names: Vec<String> = tools.iter().map(|t| t.spec().name.clone()).collect();
        for t in &names {
            assert!(t.starts_with("pc_"), "{t}");
        }
        for want in [
            "pc_info",
            "pc_shell_run",
            "pc_fs_read",
            "pc_fs_write",
            "pc_browser_open",
            "pc_desktop_get_state",
            "pc_desktop_launch",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} missing: {names:?}");
        }
        assert!(!names.iter().any(|n| n == "pc_browser_eval"));
        let open = tools.iter().find(|t| t.spec().name == "pc_browser_open");
        let spec = open.unwrap().spec();
        assert!(spec.description.contains("USER'S COMPUTER"));
        let shell = tools.iter().find(|t| t.spec().name == "pc_shell_run");
        let started = std::time::Instant::now();
        let err = shell
            .unwrap()
            .call(&ToolContext::default(), json!({"command": "dir"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("off"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
        let desktop = tools
            .iter()
            .find(|t| t.spec().name == "pc_desktop_list_apps")
            .unwrap();
        let err = desktop
            .call(&ToolContext::default(), json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("off"), "{err}");
    }
}
