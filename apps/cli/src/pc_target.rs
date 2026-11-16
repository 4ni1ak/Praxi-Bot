//! "My computer is a target for my bots" (card 258-A): this computer's daemon
//! serves the paired host's desktop requests, under the owner's consent.
//!
//! Authority model, all enforced here before the shared desktop RPC runs
//! anything:
//! 1. Only the paired device with role `host` is served, and the pairing is
//!    looked up again (role, generation, switch) right before every dispatch,
//!    so unpairing or switching off stops what is already parked.
//! 2. Off by default, per host, by the owner (`PUT /v1/hosts/{id}/computer-access`,
//!    owner authority only). Serving also needs the owner's desktop app to be
//!    running (its heartbeat is the lease).
//! 3. Reads: the first look each session asks the owner (unless the owner chose
//!    "always"); Praxi's own windows never appear in a read (listing, snapshot,
//!    screenshot); an observation is discarded when the human took control
//!    while it was captured; every read is audited (an audit failure blocks it)
//!    and shows a "looking" indicator.
//! 4. Input (`desktop.act`, `desktop.launch`) always waits for the owner on a
//!    card built HERE from the parsed request and this OS (the program and
//!    window that would receive it, the complete masked text, the keys), is
//!    bound to that program at dispatch (`act_guarded`) and is refused for
//!    Praxi's own windows, password fields and launcher key chords.
//! 5. Every decision is written to the local audit log.
//!
//! Nothing new listens: requests arrive over the existing paired iroh link.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_core::ControlGate;
use async_trait::async_trait;
use daemon_api::pc_access::{PcPreview, PcRequests};
use desktop_api::{
    ActGuard, Action, ActionResult, Capabilities, ClickMethod, CoordinateSpace, DesktopBackend,
    DesktopError, Image, InputTarget, Quality, Region, Snapshot, Target, WindowInfo,
    snapshot::{AppSpec, Pid, Rect},
};
use parking_lot::Mutex;
use peer_network::proto::{ERR_FORBIDDEN, ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND, RpcError};
use peer_network::{DesktopRpcHandler, Device, Role, RpcHandler, desktop_error_to_rpc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long the owner has to answer a card before it is denied. Must stay below
/// `peer_network::ACTION_RPC_TIMEOUT` (the transport deadline of such calls).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(120);

/// Longest text / command a card will show completely. Anything longer is
/// refused instead of being cut.
const MAX_PAYLOAD_CHARS: usize = 1500;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HostAccess {
    /// The first look each session needs no card.
    #[serde(default)]
    always_read: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct AccessFile {
    #[serde(default)]
    hosts: BTreeMap<String, HostAccess>,
}

/// Hosts whose bots may use this computer. Persisted next to the pairing file.
pub struct AccessStore {
    path: PathBuf,
    state: Mutex<AccessFile>,
    /// Hosts whose first look this session was allowed (memory only).
    session_reads: Mutex<BTreeSet<String>>,
}

impl AccessStore {
    #[must_use]
    pub fn open(data: &Path) -> Self {
        let path = data.join("peer").join("computer-access.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<AccessFile>(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
            session_reads: Mutex::new(BTreeSet::new()),
        }
    }

    #[must_use]
    pub fn enabled(&self, host_id: &str) -> bool {
        self.state.lock().hosts.contains_key(host_id)
    }

    #[must_use]
    pub fn always_read(&self, host_id: &str) -> bool {
        self.state
            .lock()
            .hosts
            .get(host_id)
            .is_some_and(|h| h.always_read)
    }

    #[must_use]
    pub fn enabled_ids(&self) -> Vec<String> {
        self.state.lock().hosts.keys().cloned().collect()
    }

    fn session_read_allowed(&self, host_id: &str) -> bool {
        self.session_reads.lock().contains(host_id)
    }

    fn allow_session_read(&self, host_id: &str) {
        self.session_reads.lock().insert(host_id.to_owned());
    }

    /// # Errors
    /// The file could not be written; the in-memory switch is left unchanged.
    pub fn set(&self, host_id: &str, on: bool, always_read: bool) -> std::io::Result<()> {
        let mut state = self.state.lock();
        let mut next = AccessFile {
            hosts: state.hosts.clone(),
        };
        if on {
            next.hosts
                .insert(host_id.to_owned(), HostAccess { always_read });
        } else {
            next.hosts.remove(host_id);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_vec(&next)?)?;
        *state = next;
        if !on {
            self.session_reads.lock().remove(host_id);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct PairingFile {
    #[serde(default)]
    devices: Vec<Device>,
}

/// The pairing as it is on disk right now (revoking removes the entry).
fn paired_device(data: &Path, id: &str) -> Option<Device> {
    let text = std::fs::read_to_string(data.join("peer").join("devices.json")).ok()?;
    serde_json::from_str::<PairingFile>(&text)
        .ok()?
        .devices
        .into_iter()
        .find(|d| d.id == id)
}

// ── Praxi's own windows ───────────────────────────────────────────────────────

fn is_praxi_title(title: &str) -> bool {
    title.to_lowercase().contains("praxi")
}

/// A window of Praxi itself (the app, its approval cards, the overlay): this
/// daemon's own process, or a title naming Praxi. A title is only a heuristic;
/// it fails toward hiding more.
fn is_praxi_window(w: &WindowInfo, own_pid: u32) -> bool {
    w.pid == own_pid || is_praxi_title(&w.title)
}

fn is_praxi_target(t: &InputTarget, own_pid: u32) -> bool {
    t.identity.pid == own_pid
        || is_praxi_title(&t.title)
        || t.identity.program_stem().contains("praxi")
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn contains(r: &Rect, x: f64, y: f64) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

// ── Read channels: a backend that never shows Praxi's own UI ─────────────────

/// Wraps the real backend for every READ the host can make. Input never goes
/// through it (the handler dispatches input itself, guarded).
struct ReadBackend {
    inner: Arc<dyn DesktopBackend>,
    gate: Arc<dyn ControlGate>,
    own_pid: u32,
}

impl ReadBackend {
    fn control_refusal(&self) -> Option<DesktopError> {
        self.gate
            .observation_refused()
            .map(DesktopError::Accessibility)
    }

    fn discarded() -> DesktopError {
        DesktopError::Accessibility(
            "the user took control of the screen while this was captured, so it was discarded"
                .into(),
        )
    }
}

#[async_trait]
impl DesktopBackend for ReadBackend {
    async fn snapshot(&self, scope: Option<u64>) -> Result<Snapshot, DesktopError> {
        if let Some(e) = self.control_refusal() {
            return Err(e);
        }
        let epoch = self.gate.observation_epoch();
        let snap = self.inner.snapshot(scope).await?;
        if self.gate.observation_epoch() != epoch || self.gate.observation_refused().is_some() {
            return Err(Self::discarded());
        }
        if is_praxi_window(&snap.window, self.own_pid) {
            return Err(DesktopError::Accessibility(
                "that window belongs to Praxi and is not shared".into(),
            ));
        }
        Ok(snap)
    }

    async fn act(&self, _action: Action) -> Result<ActionResult, DesktopError> {
        Err(DesktopError::GuardRefused(
            "input is not served on this channel".into(),
        ))
    }

    async fn screenshot(&self, region: Region, quality: Quality) -> Result<Image, DesktopError> {
        if let Some(e) = self.control_refusal() {
            return Err(e);
        }
        let windows = self.inner.windows().await?;
        let area = match &region {
            Region::FullScreen => None,
            Region::Window { window_id } => {
                let w = windows
                    .iter()
                    .find(|w| w.id == *window_id)
                    .ok_or_else(|| DesktopError::Screenshot("that window is not visible".into()))?;
                if is_praxi_window(w, self.own_pid) {
                    return Err(DesktopError::Screenshot(
                        "that window belongs to Praxi and is not shared".into(),
                    ));
                }
                Some(w.bounds)
            }
            Region::Rect(r) => Some(*r),
        };
        // Pixels of a Praxi window (a pending card, the key fields) must never
        // leave this computer: refuse when one lies in the captured area.
        let covered = windows
            .iter()
            .filter(|w| is_praxi_window(w, self.own_pid))
            .any(|w| area.as_ref().is_none_or(|a| overlaps(a, &w.bounds)));
        if covered {
            return Err(DesktopError::Screenshot(
                "a Praxi window is open in this area, so the screenshot was not taken; ask for a specific other window".into(),
            ));
        }
        let epoch = self.gate.observation_epoch();
        let image = self.inner.screenshot(region, quality).await?;
        if self.gate.observation_epoch() != epoch || self.gate.observation_refused().is_some() {
            return Err(Self::discarded());
        }
        Ok(image)
    }

    async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
        if let Some(e) = self.control_refusal() {
            return Err(e);
        }
        let epoch = self.gate.observation_epoch();
        let all = self.inner.windows().await?;
        if self.gate.observation_epoch() != epoch {
            return Err(Self::discarded());
        }
        Ok(all
            .into_iter()
            .filter(|w| !is_praxi_window(w, self.own_pid))
            .collect())
    }

    async fn launch(&self, _app: AppSpec) -> Result<Pid, DesktopError> {
        Err(DesktopError::GuardRefused(
            "input is not served on this channel".into(),
        ))
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

// ── The handler ──────────────────────────────────────────────────────────────

/// Writes one audit row; an error blocks what it was written for.
pub type AuditFn = Arc<dyn Fn(&str, Value) -> Result<(), String> + Send + Sync>;

/// Everything the handler needs from the daemon.
pub struct PcTargetDeps {
    pub raw: Arc<dyn DesktopBackend>,
    pub data: PathBuf,
    pub access: Arc<AccessStore>,
    pub requests: Arc<PcRequests>,
    pub gate: Arc<dyn ControlGate>,
    /// The owner's desktop app is running (its heartbeat is live).
    pub owner_ui_live: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Write an audit row; a failure blocks reads.
    pub audit: AuditFn,
    /// Mask owner secrets in text shown on a card.
    pub redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    pub own_pid: u32,
}

/// The shared desktop RPC surface behind the owner's consent.
pub struct HostDesktopHandler {
    reads: DesktopRpcHandler,
    deps: PcTargetDeps,
    ask_timeout: Duration,
}

impl HostDesktopHandler {
    #[must_use]
    pub fn new(deps: PcTargetDeps) -> Self {
        let reads = DesktopRpcHandler::new(Arc::new(ReadBackend {
            inner: Arc::clone(&deps.raw),
            gate: Arc::clone(&deps.gate),
            own_pid: deps.own_pid,
        }));
        Self {
            reads,
            deps,
            ask_timeout: ASK_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_ask_timeout(mut self, t: Duration) -> Self {
        self.ask_timeout = t;
        self
    }

    fn audit(&self, action: &str, detail: Value) -> Result<(), String> {
        (self.deps.audit)(action, detail)
    }

    fn refuse(&self, peer: &Device, method: &str, why: &str) -> RpcError {
        let _ = self.audit(
            "pc_target.refused",
            serde_json::json!({"host": peer.id, "method": method, "why": why}),
        );
        RpcError::new(ERR_FORBIDDEN, why)
    }

    /// The pairing, the role and the switch as they are NOW. Called right
    /// before anything is dispatched, not only when the request arrived.
    fn still_authorized(&self, peer: &Device) -> Result<(), &'static str> {
        let Some(now) = paired_device(&self.deps.data, &peer.id) else {
            return Err("this computer is no longer paired with that server");
        };
        if now.role != Role::Host || now.pairing_generation != peer.pairing_generation {
            return Err("this computer was paired again since this request started");
        }
        if !self.deps.access.enabled(&peer.id) {
            return Err("the owner turned bots' use of this computer off");
        }
        if !(self.deps.owner_ui_live)() {
            return Err("the Praxi app is not running on this computer, so it is not shared");
        }
        Ok(())
    }

    async fn read(&self, peer: &Device, method: &str, params: Value) -> Result<Value, RpcError> {
        if !self.deps.access.always_read(&peer.id)
            && !self.deps.access.session_read_allowed(&peer.id)
        {
            let allowed = self
                .deps
                .requests
                .ask(
                    &peer.id,
                    &peer.name,
                    PcPreview {
                        kind: "session_read",
                        action: "look".into(),
                        target: "this computer's screen".into(),
                        payload: "Allow this server's bots to look at this screen for this session? Praxi's own windows stay hidden.".into(),
                    },
                    self.ask_timeout,
                )
                .await;
            if !allowed {
                return Err(self.refuse(
                    peer,
                    method,
                    "the person at this computer did not allow looking at the screen",
                ));
            }
            if let Err(why) = self.still_authorized(peer) {
                return Err(self.refuse(peer, method, why));
            }
            self.deps.access.allow_session_read(&peer.id);
        }
        if let Err(e) = self.audit(
            "pc_target.read",
            serde_json::json!({"host": peer.id, "method": method}),
        ) {
            return Err(RpcError::new(
                ERR_FORBIDDEN,
                format!("not shared: the local audit log could not be written ({e})"),
            ));
        }
        self.deps.requests.note_read(&peer.id, &peer.name);
        self.reads.handle(peer, method, params).await
    }

    async fn act(&self, peer: &Device, params: Value) -> Result<Value, RpcError> {
        let action = parse_action(&params)?;
        let prepared = self.prepare(peer, "desktop.act", &action).await?;
        let preview = prepared.preview("act", action_name(&action));
        self.approve_and_run(peer, "desktop.act", preview).await?;
        let outcome = self
            .deps
            .raw
            .act_guarded(
                action,
                ActGuard {
                    expect: prepared.target.identity.clone(),
                },
            )
            .await
            .map_err(|e| desktop_error_to_rpc(&e))?;
        serde_json::to_value(outcome).map_err(|e| RpcError::new(-32603, e.to_string()))
    }

    async fn launch(&self, peer: &Device, params: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct P {
            app: AppSpec,
        }
        let p: P = serde_json::from_value(params)
            .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, format!("invalid params: {e}")))?;
        let command = std::iter::once(p.app.executable.as_str())
            .chain(p.app.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        complete_text(&command).map_err(|why| self.refuse(peer, "desktop.launch", &why))?;
        if let Some(why) = self.base_refusal(peer, "desktop.launch") {
            return Err(self.refuse(peer, "desktop.launch", &why));
        }
        let preview = PcPreview {
            kind: "act",
            action: "launch".into(),
            target: "starts a new program on this computer".into(),
            payload: (self.deps.redact)(&command),
        };
        self.approve_and_run(peer, "desktop.launch", preview)
            .await?;
        let pid = self
            .deps
            .raw
            .launch(p.app)
            .await
            .map_err(|e| desktop_error_to_rpc(&e))?;
        Ok(serde_json::json!({ "pid": pid }))
    }

    /// Latch and lease: the same refusal for every kind of input.
    fn base_refusal(&self, peer: &Device, _method: &str) -> Option<String> {
        if let Some(msg) = self.deps.gate.latch_refusal() {
            return Some(msg);
        }
        self.still_authorized(peer).err().map(str::to_owned)
    }

    /// Park a card and wait; then re-check everything the answer depends on.
    async fn approve_and_run(
        &self,
        peer: &Device,
        method: &str,
        preview: PcPreview,
    ) -> Result<(), RpcError> {
        let allowed = self
            .deps
            .requests
            .ask(&peer.id, &peer.name, preview.clone(), self.ask_timeout)
            .await;
        if !allowed {
            return Err(self.refuse(
                peer,
                method,
                "the person at this computer did not approve this action",
            ));
        }
        // The switch, the pairing or the human's hands may have changed while
        // the card was open.
        if let Some(why) = self.base_refusal(peer, method) {
            return Err(self.refuse(peer, method, &why));
        }
        self.audit(
            "pc_target.act",
            serde_json::json!({"host": peer.id, "method": method, "action": preview.action, "target": preview.target}),
        )
        .map_err(|e| {
            RpcError::new(
                ERR_FORBIDDEN,
                format!("not done: the local audit log could not be written ({e})"),
            )
        })?;
        Ok(())
    }

    /// Validate the request and resolve, on this OS, what it would reach.
    async fn prepare(
        &self,
        peer: &Device,
        method: &str,
        action: &Action,
    ) -> Result<Prepared, RpcError> {
        if let Some(why) = self.base_refusal(peer, method) {
            return Err(self.refuse(peer, method, &why));
        }
        let payload = describe_action(action, &self.deps.redact)
            .map_err(|why| self.refuse(peer, method, &why))?;
        if let Action::KeyPress { keys } = action
            && let Some(r) = self.deps.raw.key_chord_refusal(keys)
        {
            return Err(self.refuse(peer, method, r.why()));
        }
        let target = match self.deps.raw.input_target().await {
            Ok(Some(t)) if t.identity.is_complete() => t,
            _ => {
                return Err(self.refuse(
                    peer,
                    method,
                    "could not identify the program that would receive this input",
                ));
            }
        };
        if is_praxi_target(&target, self.deps.own_pid) {
            return Err(self.refuse(
                peer,
                method,
                "input to Praxi's own windows is never allowed",
            ));
        }
        if target.password_focus && !matches!(action, Action::Click { .. } | Action::Scroll { .. })
        {
            return Err(self.refuse(peer, method, "the focused element is a password field"));
        }
        // Coordinates may land on a Praxi window that is not in front.
        if let Some((x, y)) = absolute_point(action) {
            let windows = self.deps.raw.windows().await.unwrap_or_default();
            if windows
                .iter()
                .any(|w| is_praxi_window(w, self.deps.own_pid) && contains(&w.bounds, x, y))
            {
                return Err(self.refuse(
                    peer,
                    method,
                    "that point is inside a Praxi window; input there is never allowed",
                ));
            }
        }
        Ok(Prepared {
            description: format!(
                "\"{}\" — {} (process {})",
                target.title, target.identity.image_path, target.identity.pid
            ),
            payload,
            target,
        })
    }
}

struct Prepared {
    target: InputTarget,
    description: String,
    payload: String,
}

impl Prepared {
    fn preview(&self, kind: &'static str, action: &str) -> PcPreview {
        PcPreview {
            kind,
            action: action.to_owned(),
            target: self.description.clone(),
            payload: self.payload.clone(),
        }
    }
}

fn parse_action(params: &Value) -> Result<Action, RpcError> {
    let action = params
        .get("action")
        .cloned()
        .ok_or_else(|| RpcError::new(ERR_INVALID_PARAMS, "missing action"))?;
    serde_json::from_value(action)
        .map_err(|e| RpcError::new(ERR_INVALID_PARAMS, format!("invalid action: {e}")))
}

fn action_name(a: &Action) -> &'static str {
    match a {
        Action::Click { .. } => "click",
        Action::TypeText { .. } => "type",
        Action::SetValue { .. } => "set value",
        Action::Scroll { .. } => "scroll",
        Action::KeyPress { .. } => "key",
        Action::Drag { .. } => "drag",
    }
}

fn absolute_point(a: &Action) -> Option<(f64, f64)> {
    fn at(t: &Target) -> Option<(f64, f64)> {
        match t {
            Target::ByCoordinate {
                x,
                y,
                space: CoordinateSpace::Absolute,
            } => Some((*x, *y)),
            _ => None,
        }
    }
    match a {
        Action::Click { target, .. }
        | Action::SetValue { target, .. }
        | Action::Scroll { target, .. } => at(target),
        Action::Drag { from, to, .. } => at(from).or_else(|| at(to)),
        _ => None,
    }
}

/// Text a card can show in full: not too long, nothing invisible.
fn complete_text(text: &str) -> Result<(), String> {
    if text.chars().count() > MAX_PAYLOAD_CHARS {
        return Err(format!(
            "the text is longer than {MAX_PAYLOAD_CHARS} characters, so it cannot be shown completely and was refused"
        ));
    }
    if protocol::approval_text::hides_characters(text) {
        return Err(
            "the text contains invisible or direction-changing characters, so it was refused"
                .into(),
        );
    }
    Ok(())
}

fn target_text(t: &Target) -> String {
    match t {
        Target::ByIndex { index, generation } => {
            format!("element #{index} of the last view (generation {generation})")
        }
        Target::ByCoordinate { x, y, space } => match space {
            CoordinateSpace::Absolute => format!("screen point ({x:.0}, {y:.0})"),
            CoordinateSpace::Normalized999 => {
                format!("point ({x:.0}, {y:.0}) of 999 in the window")
            }
        },
    }
}

/// The complete payload of an action as a person reads it; secrets masked.
fn describe_action(
    a: &Action,
    redact: &Arc<dyn Fn(&str) -> String + Send + Sync>,
) -> Result<String, String> {
    match a {
        Action::Click { target, method } => {
            let how = match method {
                ClickMethod::Accessibility => "accessibility press",
                ClickMethod::AppPost => "posted mouse click",
                ClickMethod::Global => "real mouse click",
            };
            Ok(format!("click on {} ({how})", target_text(target)))
        }
        Action::TypeText { text } => {
            complete_text(text)?;
            Ok(format!("type exactly:\n{}", redact(text)))
        }
        Action::SetValue { target, value } => {
            complete_text(value)?;
            Ok(format!(
                "set {} to:\n{}",
                target_text(target),
                redact(value)
            ))
        }
        Action::Scroll {
            target,
            delta_x,
            delta_y,
        } => Ok(format!(
            "scroll {} by ({delta_x:.0}, {delta_y:.0})",
            target_text(target)
        )),
        Action::KeyPress { keys } => {
            if keys.is_empty() || keys.len() > 8 || keys.iter().any(|k| k.chars().count() > 32) {
                return Err("the key list is empty or too long".into());
            }
            let joined = keys.join(" + ");
            complete_text(&joined)?;
            Ok(format!("press keys: {joined}"))
        }
        Action::Drag { from, to, .. } => Ok(format!(
            "drag from {} to {}",
            target_text(from),
            target_text(to)
        )),
    }
}

#[async_trait]
impl RpcHandler for HostDesktopHandler {
    async fn handle(&self, peer: &Device, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "node.ping" {
            return self.reads.handle(peer, method, params).await;
        }
        if peer.role != Role::Host {
            return Err(self.refuse(
                peer,
                method,
                "only the server this computer is paired with may use it",
            ));
        }
        if !self.deps.access.enabled(&peer.id) {
            return Err(self.refuse(
                peer,
                method,
                "the owner has not allowed bots to use this computer",
            ));
        }
        if let Err(why) = self.still_authorized(peer) {
            return Err(self.refuse(peer, method, why));
        }
        match method {
            "node.info" | "desktop.capabilities" => self.reads.handle(peer, method, params).await,
            "desktop.snapshot" | "desktop.screenshot" | "desktop.windows" => {
                self.read(peer, method, params).await
            }
            "desktop.act" => self.act(peer, params).await,
            "desktop.launch" => self.launch(peer, params).await,
            other => Err(RpcError::new(
                ERR_METHOD_NOT_FOUND,
                format!("unknown method '{other}'"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_api::ProcessIdentity;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    const OWN_PID: u32 = 4242;

    fn identity(pid: u32, path: &str) -> ProcessIdentity {
        ProcessIdentity {
            image_path: path.into(),
            pid,
            start_time: 1,
        }
    }

    fn window(id: u64, title: &str, pid: u32, x: f64) -> WindowInfo {
        WindowInfo {
            id,
            title: title.into(),
            pid,
            bounds: Rect {
                x,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            focused: false,
        }
    }

    struct Fake {
        acts: AtomicUsize,
        guarded_for: Mutex<Option<ProcessIdentity>>,
        shots: AtomicUsize,
        epoch: Arc<AtomicU64>,
        bump_in_capture: bool,
        windows: Vec<WindowInfo>,
        foreground: Mutex<InputTarget>,
    }

    impl Fake {
        fn new(epoch: Arc<AtomicU64>) -> Arc<Self> {
            Arc::new(Self {
                acts: AtomicUsize::new(0),
                guarded_for: Mutex::new(None),
                shots: AtomicUsize::new(0),
                epoch,
                bump_in_capture: false,
                windows: vec![
                    window(1, "Notes", 100, 0.0),
                    window(2, "Praxi-Bot", 200, 500.0),
                ],
                foreground: Mutex::new(InputTarget {
                    window_id: 1,
                    title: "Notes".into(),
                    class: "Notepad".into(),
                    identity: identity(100, "C:\\Windows\\notepad.exe"),
                    elevated: Some(false),
                    password_focus: false,
                    owned: false,
                }),
            })
        }
    }

    #[async_trait]
    impl DesktopBackend for Fake {
        async fn snapshot(&self, _: Option<u64>) -> Result<Snapshot, DesktopError> {
            Err(DesktopError::Unsupported("test".into()))
        }
        async fn act(&self, _: Action) -> Result<ActionResult, DesktopError> {
            self.acts.fetch_add(100, Ordering::SeqCst);
            Err(DesktopError::Unsupported(
                "unguarded act must not be used".into(),
            ))
        }
        async fn act_guarded(&self, _: Action, g: ActGuard) -> Result<ActionResult, DesktopError> {
            self.acts.fetch_add(1, Ordering::SeqCst);
            *self.guarded_for.lock() = Some(g.expect);
            Ok(ActionResult {
                dispatched: true,
                verified: None,
                new_snapshot: None,
            })
        }
        async fn screenshot(&self, _: Region, _: Quality) -> Result<Image, DesktopError> {
            self.shots.fetch_add(1, Ordering::SeqCst);
            if self.bump_in_capture {
                self.epoch.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Image {
                data: vec![1],
                width: 1,
                height: 1,
                mime_type: "image/png".into(),
            })
        }
        async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
            Ok(self.windows.clone())
        }
        async fn launch(&self, _: AppSpec) -> Result<Pid, DesktopError> {
            Err(DesktopError::Unsupported("test".into()))
        }
        async fn input_target(&self) -> Result<Option<InputTarget>, DesktopError> {
            Ok(Some(self.foreground.lock().clone()))
        }
        fn key_chord_refusal(&self, keys: &[String]) -> Option<desktop_api::Refusal> {
            keys.iter()
                .any(|k| k.eq_ignore_ascii_case("win"))
                .then_some(desktop_api::Refusal::Chord)
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                platform: "test".into(),
                accessibility_tree: false,
                screenshot: true,
                app_launch: false,
                has_display: true,
            }
        }
    }

    struct Gate {
        epoch: Arc<AtomicU64>,
    }
    #[async_trait]
    impl ControlGate for Gate {
        fn observation_epoch(&self) -> u64 {
            self.epoch.load(Ordering::SeqCst)
        }
        async fn before_action(
            &self,
            _: agent_core::ControlRequest,
            _: &(dyn Fn() + Send + Sync),
        ) -> Result<(), String> {
            Ok(())
        }
    }

    struct Rig {
        h: HostDesktopHandler,
        fake: Arc<Fake>,
        access: Arc<AccessStore>,
        requests: Arc<PcRequests>,
        dir: tempfile::TempDir,
        live: Arc<std::sync::atomic::AtomicBool>,
        audit_ok: Arc<std::sync::atomic::AtomicBool>,
    }

    fn device(role: Role) -> Device {
        Device {
            id: "host1".into(),
            name: "Server".into(),
            role,
            capabilities: vec![],
            addr: None,
            paired_at: 1,
            pairing_generation: "g1".into(),
            admin: false,
        }
    }

    fn write_pairing(dir: &Path, role: &str, generation: &str) {
        std::fs::create_dir_all(dir.join("peer")).unwrap();
        std::fs::write(
            dir.join("peer").join("devices.json"),
            serde_json::json!({"version":1,"devices":[{
                "id":"host1","name":"Server","role":role,"capabilities":[],
                "addr":null,"paired_at":1,"pairing_generation":generation
            }]})
            .to_string(),
        )
        .unwrap();
    }

    fn rig(bump_in_capture: bool, always_read: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        write_pairing(dir.path(), "host", "g1");
        let epoch = Arc::new(AtomicU64::new(0));
        let mut fake = Fake::new(epoch.clone());
        Arc::get_mut(&mut fake).unwrap().bump_in_capture = bump_in_capture;
        let access = Arc::new(AccessStore::open(dir.path()));
        access.set("host1", true, always_read).unwrap();
        let requests = Arc::new(PcRequests::new());
        let live = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let audit_ok = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (l, a) = (live.clone(), audit_ok.clone());
        let h = HostDesktopHandler::new(PcTargetDeps {
            raw: fake.clone(),
            data: dir.path().to_path_buf(),
            access: access.clone(),
            requests: requests.clone(),
            gate: Arc::new(Gate { epoch }),
            owner_ui_live: Arc::new(move || l.load(Ordering::SeqCst)),
            audit: Arc::new(move |_, _| {
                a.load(Ordering::SeqCst)
                    .then_some(())
                    .ok_or("disk full".into())
            }),
            redact: Arc::new(|t| t.replace("hunter2", "••••")),
            own_pid: OWN_PID,
        })
        .with_ask_timeout(Duration::from_secs(5));
        Rig {
            h,
            fake,
            access,
            requests,
            dir,
            live,
            audit_ok,
        }
    }

    async fn answer(requests: &PcRequests, allow: bool) -> daemon_api::pc_access::PcRequestInfo {
        let info = loop {
            if let Some(r) = requests.list().first() {
                break r.clone();
            }
            tokio::task::yield_now().await;
        };
        assert!(requests.decide(&info.id, allow));
        info
    }

    fn type_params(text: &str) -> Value {
        serde_json::json!({ "action": Action::TypeText { text: text.into() } })
    }

    /// Blocker 4: the card shows the COMPLETE masked payload and the program
    /// that would receive it, and what cannot be shown completely is refused.
    #[tokio::test]
    async fn card_shows_the_full_masked_payload_and_the_resolved_target() {
        let r = rig(false, true);
        let host = device(Role::Host);
        let long = format!("{}{}", "harmless ".repeat(40), "and then hunter2 DELETE");
        let call = r.h.handle(&host, "desktop.act", type_params(&long));
        let (res, info) = tokio::join!(call, answer(&r.requests, true));
        assert!(res.is_ok(), "{res:?}");
        assert!(
            info.payload.contains("and then •••• DELETE"),
            "{}",
            info.payload
        );
        assert!(!info.payload.contains("hunter2"));
        assert!(info.target.contains("notepad.exe") && info.target.contains("Notes"));
        // Too long to show completely, or hiding characters: refused, no card.
        let huge = "x".repeat(MAX_PAYLOAD_CHARS + 1);
        for text in [huge.as_str(), "ok\u{202e}evil"] {
            let res = r.h.handle(&host, "desktop.act", type_params(text)).await;
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        }
        assert!(r.requests.list().is_empty());
    }

    /// Blocker 5: approved input is bound to the program shown and never
    /// reaches Praxi's own windows by keyboard or by coordinates.
    #[tokio::test]
    async fn input_is_bound_to_the_shown_program_and_never_reaches_praxi() {
        let r = rig(false, true);
        let host = device(Role::Host);
        let call = r.h.handle(&host, "desktop.act", type_params("hello"));
        let (res, _) = tokio::join!(call, answer(&r.requests, true));
        assert!(res.is_ok());
        assert_eq!(r.fake.acts.load(Ordering::SeqCst), 1, "guarded send only");
        assert_eq!(
            r.fake.guarded_for.lock().as_ref().map(|i| i.pid),
            Some(100),
            "dispatch is bound to the program on the card"
        );
        // Foreground is Praxi: keys and typing are refused outright.
        r.fake.foreground.lock().title = "Praxi-Bot".into();
        let res = r.h.handle(&host, "desktop.act", type_params("x")).await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        r.fake.foreground.lock().title = "Notes".into();
        // A coordinate click inside the Praxi window (x 500..600) is refused.
        let click = |x: f64| {
            serde_json::json!({ "action": Action::Click {
                target: Target::ByCoordinate { x, y: 10.0, space: CoordinateSpace::Absolute },
                method: ClickMethod::Global } })
        };
        let res = r.h.handle(&host, "desktop.act", click(550.0)).await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
        assert!(
            r.requests.list().is_empty(),
            "no card for a forbidden target"
        );
        assert_eq!(r.fake.acts.load(Ordering::SeqCst), 1);
    }

    /// Blocker 3: switching off (or unpairing) while a card is open stops the
    /// action even if the card is answered "allow".
    #[tokio::test]
    async fn a_parked_action_does_not_run_after_the_switch_or_pairing_changed() {
        for unpair in [false, true] {
            let r = rig(false, true);
            let host = device(Role::Host);
            let call = r.h.handle(&host, "desktop.act", type_params("hello"));
            let change = async {
                let id = loop {
                    if let Some(i) = r.requests.list().first() {
                        break i.id.clone();
                    }
                    tokio::task::yield_now().await;
                };
                if unpair {
                    std::fs::remove_file(r.dir.path().join("peer").join("devices.json")).unwrap();
                } else {
                    r.access.set("host1", false, false).unwrap();
                }
                // Even a late "allow" must not run it.
                r.requests.decide(&id, true);
            };
            let (res, ()) = tokio::join!(call, change);
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN, "unpair={unpair}");
            assert_eq!(r.fake.acts.load(Ordering::SeqCst), 0, "unpair={unpair}");
        }
    }

    /// Blocker 2: reads hide Praxi's windows, are discarded across a control
    /// transition, need the owner's app and a working audit, and the first
    /// look each session asks.
    #[tokio::test]
    async fn reads_hide_praxi_respect_latch_lease_audit_and_ask_first() {
        // Praxi window is not listed, and a full-screen capture is refused
        // while it is open; a capture of another window is allowed.
        let r = rig(false, true);
        let host = device(Role::Host);
        let list =
            r.h.handle(&host, "desktop.windows", Value::Null)
                .await
                .unwrap();
        let titles: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w["title"].as_str())
            .collect();
        assert_eq!(titles, vec!["Notes"]);
        let shot =
            |region: Region| serde_json::json!({"region": region, "quality": Quality::Medium});
        let full =
            r.h.handle(&host, "desktop.screenshot", shot(Region::FullScreen))
                .await;
        assert!(full.is_err(), "full screen overlaps the Praxi window");
        assert_eq!(r.fake.shots.load(Ordering::SeqCst), 0);
        let one =
            r.h.handle(
                &host,
                "desktop.screenshot",
                shot(Region::Window { window_id: 1 }),
            )
            .await;
        assert!(one.is_ok(), "{one:?}");
        assert_eq!(r.requests.reading(), vec!["Server".to_string()]);

        // The human took control during the capture: discarded.
        let r2 = rig(true, true);
        let res =
            r2.h.handle(
                &host,
                "desktop.screenshot",
                shot(Region::Window { window_id: 1 }),
            )
            .await;
        assert!(
            res.is_err(),
            "observation across a latch transition is discarded"
        );

        // Owner app not running / audit failing: nothing is served.
        r.live.store(false, Ordering::SeqCst);
        assert!(
            r.h.handle(&host, "desktop.windows", Value::Null)
                .await
                .is_err()
        );
        r.live.store(true, Ordering::SeqCst);
        r.audit_ok.store(false, Ordering::SeqCst);
        assert!(
            r.h.handle(&host, "desktop.windows", Value::Null)
                .await
                .is_err()
        );

        // Default: the first look asks one card; "always" skips it.
        let r3 = rig(false, false);
        let call = r3.h.handle(&host, "desktop.windows", Value::Null);
        let (res, info) = tokio::join!(call, answer(&r3.requests, true));
        assert!(res.is_ok());
        assert_eq!(info.kind, "session_read");
        assert!(
            r3.h.handle(&host, "desktop.windows", Value::Null)
                .await
                .is_ok(),
            "asked once per session"
        );
    }

    /// Switch off, other roles and unknown peers never reach the backend.
    #[tokio::test]
    async fn only_the_adopted_host_with_the_switch_on_is_served() {
        let r = rig(false, true);
        for role in [Role::Desktop, Role::Phone, Role::Node] {
            let res =
                r.h.handle(&device(role), "desktop.windows", Value::Null)
                    .await;
            assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN, "{role:?}");
        }
        r.access.set("host1", false, false).unwrap();
        let res =
            r.h.handle(&device(Role::Host), "desktop.windows", Value::Null)
                .await;
        assert_eq!(res.unwrap_err().code, ERR_FORBIDDEN);
    }
}
