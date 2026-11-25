//! Pop-out chat and live-view windows (Issue #233).
//!
//! Each pop-out window (`chat-<uuid>` or `live-<home|local>`) runs its own webview against the
//! app's own origin (`WebviewUrl::App`) and connects to the daemon via SSE/HTTP like a separate
//! client. Closing a pop-out window never stops a bot run or exits the app. Window position, size
//! and pin-on-top state are remembered per window and clamped safely across monitors and DPI scales.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, Url, WebviewUrl,
    WebviewWindowBuilder, Window,
};

use crate::daemon::{self, Connection, DaemonHandle};
use crate::quick_ask::plausible_chat_id;
use crate::window_gate::{MAIN_ONLY, POPOUT_ONLY, require_window};

/// Emitted to an already-open live-view pop-out when the user opens or focuses it with a target.
pub const EVENT_LIVE_TARGET: &str = "popout:live_target";

const BOUNDS_FILE_NAME: &str = "popout-windows.json";
const MAX_SAVED_WINDOWS: usize = 64;

const CHAT_DEFAULT_LOGICAL: (f64, f64) = (520.0, 680.0);
const CHAT_MIN_LOGICAL: (f64, f64) = (360.0, 420.0);
const LIVE_DEFAULT_LOGICAL: (f64, f64) = (920.0, 620.0);
const LIVE_MIN_LOGICAL: (f64, f64) = (480.0, 360.0);

/// Kind of pop-out window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopoutKind {
    Chat,
    Live,
}

/// Which daemon live-view source a live pop-out displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSource {
    Home,
    Local,
}

impl LiveSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Local => "local",
        }
    }
}

/// Payload from the main window's `open_popout_window` command.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPopoutRequest {
    pub kind: PopoutKind,
    pub chat_id: Option<String>,
    pub live_source: Option<LiveSource>,
    pub bot_id: Option<String>,
    pub target_id: Option<String>,
    pub computer_id: Option<String>,
    /// When `true` (e.g. dragged out), place the new window near the OS cursor.
    pub near_cursor: Option<bool>,
    /// Current `settings.windowFrame` so the new window spawns with matching decorations.
    pub window_frame: Option<bool>,
    /// Optional window title supplied by the UI (sanitized/bounded).
    pub title: Option<String>,
}

/// Validated target for opening or focusing a pop-out window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedTarget {
    Chat {
        chat_id: String,
        computer_id: Option<String>,
    },
    Live {
        source: LiveSource,
        bot_id: Option<String>,
        target_id: Option<String>,
        computer_id: Option<String>,
    },
}

/// Payload sent when focusing an already-open live-view pop-out with updated context.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTargetPayload {
    pub bot_id: Option<String>,
    pub target_id: Option<String>,
    pub computer_id: Option<String>,
}

/// Safe identifier characters for query parameters (`botId`, `targetId`, `computerId`).
#[must_use]
pub fn safe_param_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn clean_opt_id(raw: Option<String>) -> Result<Option<String>, String> {
    raw.map(|id| {
        let id = id.trim();
        if safe_param_id(id) {
            Ok(id.to_owned())
        } else {
            Err("invalid target identifier for pop-out window".to_owned())
        }
    })
    .transpose()
}

fn clean_title(raw: Option<String>, fallback: &str) -> String {
    let Some(t) = raw else {
        return fallback.to_owned();
    };
    let clean: String = t.chars().filter(|c| !c.is_control()).take(80).collect();
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Validate a pop-out open request before creating or focusing any window.
///
/// # Errors
/// When the request is missing a valid chat UUID or uses an invalid identifier.
pub fn validate_request(req: OpenPopoutRequest) -> Result<ValidatedTarget, String> {
    let computer_id = clean_opt_id(req.computer_id)?;
    match req.kind {
        PopoutKind::Chat => {
            let chat_id = req
                .chat_id
                .as_deref()
                .map(str::trim)
                .filter(|id| plausible_chat_id(id))
                .ok_or_else(|| "invalid chat id for pop-out window".to_owned())?;
            Ok(ValidatedTarget::Chat {
                chat_id: chat_id.to_owned(),
                computer_id,
            })
        }
        PopoutKind::Live => {
            let source = req.live_source.unwrap_or(LiveSource::Home);
            Ok(ValidatedTarget::Live {
                source,
                bot_id: clean_opt_id(req.bot_id)?,
                target_id: clean_opt_id(req.target_id)?,
                computer_id,
            })
        }
    }
}

/// Window label for a chat pop-out (`None` if `chat_id` is not a valid UUID).
#[must_use]
pub fn chat_window_label(chat_id: &str) -> Option<String> {
    plausible_chat_id(chat_id).then(|| format!("chat-{chat_id}"))
}

/// Window label for a live-view pop-out.
#[must_use]
pub fn live_window_label(source: LiveSource) -> String {
    format!("live-{}", source.as_str())
}

impl ValidatedTarget {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Chat { chat_id, .. } => format!("chat-{chat_id}"),
            Self::Live { source, .. } => live_window_label(*source),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PopoutKind {
        match self {
            Self::Chat { .. } => PopoutKind::Chat,
            Self::Live { .. } => PopoutKind::Live,
        }
    }

    /// Relative `index.html?...` URL inside the app's own origin.
    #[must_use]
    pub fn relative_url(&self, pinned: bool) -> String {
        let pin = if pinned { "1" } else { "0" };
        match self {
            Self::Chat {
                chat_id,
                computer_id,
            } => {
                let mut url = format!("index.html?window=chat&chatId={chat_id}&pinned={pin}");
                if let Some(cid) = computer_id {
                    url.push_str("&computerId=");
                    url.push_str(cid);
                }
                url
            }
            Self::Live {
                source,
                bot_id,
                target_id,
                computer_id,
            } => {
                let mut url = format!(
                    "index.html?window=live&source={}&pinned={pin}",
                    source.as_str()
                );
                if let Some(bid) = bot_id {
                    url.push_str("&botId=");
                    url.push_str(bid);
                }
                if let Some(tid) = target_id {
                    url.push_str("&targetId=");
                    url.push_str(tid);
                }
                if let Some(cid) = computer_id {
                    url.push_str("&computerId=");
                    url.push_str(cid);
                }
                url
            }
        }
    }
}

/// Whether `label` belongs to a chat pop-out window.
#[must_use]
pub fn is_chat_popout_label(label: &str) -> bool {
    label.strip_prefix("chat-").is_some_and(plausible_chat_id)
}

/// Whether `label` belongs to a live-view pop-out window.
#[must_use]
pub fn is_live_popout_label(label: &str) -> bool {
    matches!(label, "live-home" | "live-local")
}

/// Whether `label` belongs to any pop-out window.
#[must_use]
pub fn is_popout_label(label: &str) -> bool {
    is_chat_popout_label(label) || is_live_popout_label(label)
}

fn kind_key_for_label(label: &str) -> Option<&'static str> {
    if is_chat_popout_label(label) {
        Some("default:chat")
    } else if is_live_popout_label(label) {
        Some("default:live")
    } else {
        None
    }
}

/// Security gate: pop-out windows may ONLY load the app's own origin.
/// Any external site, daemon URL, `file://`, `data:` or `javascript:` navigation is blocked.
#[must_use]
pub fn is_app_origin(url: &Url, dev: bool) -> bool {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("");
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if scheme == "tauri" && host == "localhost" && url.port().is_none() {
        return true;
    }
    if matches!(scheme, "http" | "https") && host == "tauri.localhost" && url.port().is_none() {
        return true;
    }
    if dev
        && scheme == "http"
        && matches!(host, "localhost" | "127.0.0.1")
        && url.port() == Some(1420)
    {
        return true;
    }
    false
}

// ── Multi-monitor / DPI placement & persistence ──────────────────────────────

/// Physical monitor bounds and DPI scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorRect {
    pub physical_x: f64,
    pub physical_y: f64,
    pub physical_width: f64,
    pub physical_height: f64,
    pub scale_factor: f64,
}

impl MonitorRect {
    #[must_use]
    pub fn effective_scale(&self) -> f64 {
        if self.scale_factor.is_finite() && self.scale_factor > 0.0 {
            self.scale_factor
        } else {
            1.0
        }
    }

    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.physical_x
            && px < self.physical_x + self.physical_width
            && py >= self.physical_y
            && py < self.physical_y + self.physical_height
    }

    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (
            self.physical_x + self.physical_width / 2.0,
            self.physical_y + self.physical_height / 2.0,
        )
    }
}

/// Remembered window bounds in physical pixels plus the DPI scale they were recorded at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub updated_at_ms: u64,
}

/// Resolved initial geometry for creating a pop-out window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedBounds {
    pub physical_x: i32,
    pub physical_y: i32,
    pub physical_width: u32,
    pub physical_height: u32,
    pub logical_width: f64,
    pub logical_height: f64,
    pub always_on_top: bool,
}

fn default_monitor() -> MonitorRect {
    MonitorRect {
        physical_x: 0.0,
        physical_y: 0.0,
        physical_width: 1920.0,
        physical_height: 1080.0,
        scale_factor: 1.0,
    }
}

fn pick_monitor(
    monitors: &[MonitorRect],
    cursor: Option<(f64, f64)>,
    saved: Option<&SavedBounds>,
) -> MonitorRect {
    if monitors.is_empty() {
        return default_monitor();
    }
    if let Some((cx, cy)) = cursor {
        if let Some(m) = monitors.iter().find(|m| m.contains(cx, cy)) {
            return *m;
        }
        return *monitors
            .iter()
            .min_by(|a, b| {
                let (ax, ay) = a.center();
                let (bx, by) = b.center();
                let da = (ax - cx).hypot(ay - cy);
                let db = (bx - cx).hypot(by - cy);
                da.total_cmp(&db)
            })
            .unwrap_or(&monitors[0]);
    }
    if let Some(s) = saved {
        let cx = f64::from(s.x) + f64::from(s.width) / 2.0;
        let cy = f64::from(s.y) + f64::from(s.height) / 2.0;
        if let Some(m) = monitors.iter().find(|m| m.contains(cx, cy)) {
            return *m;
        }
    }
    monitors[0]
}

/// Resolve window bounds cleanly onto an available monitor, converting between DPI scales
/// and clamping so a disconnected monitor or higher DPI never hides or clips the window.
#[must_use]
pub fn resolve_popout_bounds(
    saved: Option<&SavedBounds>,
    use_saved_position: bool,
    cursor: Option<(f64, f64)>,
    monitors: &[MonitorRect],
    min_logical: (f64, f64),
    default_logical: (f64, f64),
    cascade_index: usize,
) -> ResolvedBounds {
    let mon = pick_monitor(monitors, cursor, saved.filter(|_| use_saved_position));
    let scale = mon.effective_scale();
    let mon_w = mon.physical_width.max(200.0);
    let mon_h = mon.physical_height.max(200.0);
    let mon_log_w = mon_w / scale;
    let mon_log_h = mon_h / scale;

    let (raw_log_w, raw_log_h) = if let Some(s) = saved
        && s.width >= 200
        && s.height >= 200
    {
        let s_scale = if s.scale_factor.is_finite() && s.scale_factor > 0.0 {
            s.scale_factor
        } else {
            1.0
        };
        (f64::from(s.width) / s_scale, f64::from(s.height) / s_scale)
    } else {
        default_logical
    };

    let logical_w = raw_log_w.clamp(min_logical.0.min(mon_log_w), mon_log_w);
    let logical_h = raw_log_h.clamp(min_logical.1.min(mon_log_h), mon_log_h);
    let width_pixels = (logical_w * scale).round().clamp(100.0, mon_w);
    let height_pixels = (logical_h * scale).round().clamp(100.0, mon_h);

    let max_x = (mon.physical_x + mon_w - width_pixels).max(mon.physical_x);
    let max_y = (mon.physical_y + mon_h - height_pixels).max(mon.physical_y);

    let (raw_x, raw_y) = if let Some((cx, cy)) = cursor {
        (cx - width_pixels / 2.0, cy - 22.0 * scale)
    } else if use_saved_position
        && let Some(s) = saved
        && mon.contains(
            f64::from(s.x) + f64::from(s.width) / 2.0,
            f64::from(s.y) + f64::from(s.height) / 2.0,
        )
    {
        (f64::from(s.x), f64::from(s.y))
    } else {
        let step = f64::from(u32::try_from(cascade_index % 6).unwrap_or(0)) * 28.0 * scale;
        (
            mon.physical_x + (mon_w - width_pixels) / 2.0 + step,
            mon.physical_y + (mon_h - height_pixels) / 2.0 + step,
        )
    };

    let clamped_x = raw_x.clamp(mon.physical_x, max_x);
    let clamped_y = raw_y.clamp(mon.physical_y, max_y);

    #[allow(clippy::cast_possible_truncation)]
    let physical_x = clamped_x.round() as i32;
    #[allow(clippy::cast_possible_truncation)]
    let physical_y = clamped_y.round() as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let physical_width = width_pixels.round() as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let physical_height = height_pixels.round() as u32;

    ResolvedBounds {
        physical_x,
        physical_y,
        physical_width,
        physical_height,
        logical_width: logical_w,
        logical_height: logical_h,
        always_on_top: saved.is_some_and(|s| s.always_on_top),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoundsStoreData {
    #[serde(default)]
    windows: BTreeMap<String, SavedBounds>,
    #[serde(default)]
    defaults: BTreeMap<String, SavedBounds>,
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(u64::MAX)
}

fn prune_oldest(map: &mut BTreeMap<String, SavedBounds>) {
    while map.len() > MAX_SAVED_WINDOWS {
        let oldest = map
            .iter()
            .min_by_key(|(_, b)| b.updated_at_ms)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
}

/// Thread-safe store of remembered pop-out window positions, sizes and pin states.
pub struct PopoutState {
    path: Option<PathBuf>,
    inner: Mutex<InnerState>,
}

struct InnerState {
    data: BoundsStoreData,
    dirty: bool,
}

impl PopoutState {
    /// Load persisted pop-out bounds from the daemon's data directory.
    #[must_use]
    pub fn load() -> Self {
        let path = daemon::data_dir().ok().map(|d| d.join(BOUNDS_FILE_NAME));
        Self::load_from(path)
    }

    /// Load from an explicit path (used by tests).
    #[must_use]
    pub fn load_from(path: Option<PathBuf>) -> Self {
        let data = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str::<BoundsStoreData>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(InnerState { data, dirty: false }),
        }
    }

    fn resolve_for(
        &self,
        label: &str,
        kind: PopoutKind,
        cursor: Option<(f64, f64)>,
        monitors: &[MonitorRect],
        cascade_index: usize,
    ) -> ResolvedBounds {
        let guard = self.inner.lock();
        let exact = guard.data.windows.get(label);
        let fallback_key = match kind {
            PopoutKind::Chat => "default:chat",
            PopoutKind::Live => "default:live",
        };
        let fallback = guard.data.defaults.get(fallback_key);
        let (min_logical, default_logical) = match kind {
            PopoutKind::Chat => (CHAT_MIN_LOGICAL, CHAT_DEFAULT_LOGICAL),
            PopoutKind::Live => (LIVE_MIN_LOGICAL, LIVE_DEFAULT_LOGICAL),
        };
        resolve_popout_bounds(
            exact.or(fallback),
            exact.is_some(),
            cursor,
            monitors,
            min_logical,
            default_logical,
            cascade_index,
        )
    }

    /// Record a window move in physical screen coordinates.
    pub fn record_move(&self, label: &str, x: i32, y: i32, scale_factor: f64) {
        let Some(kind_key) = kind_key_for_label(label) else {
            return;
        };
        // Windows reports -32000 when a window is minimized; never persist that.
        if x <= -30_000 || y <= -30_000 {
            return;
        }
        let mut guard = self.inner.lock();
        let ts = now_ms();
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let entry = guard
            .data
            .windows
            .entry(label.to_owned())
            .or_insert_with(|| SavedBounds {
                x,
                y,
                width: 520,
                height: 680,
                scale_factor: scale,
                always_on_top: false,
                updated_at_ms: ts,
            });
        entry.x = x;
        entry.y = y;
        entry.scale_factor = scale;
        entry.updated_at_ms = ts;
        let snapshot = entry.clone();
        guard.data.defaults.insert(kind_key.to_owned(), snapshot);
        prune_oldest(&mut guard.data.windows);
        guard.dirty = true;
    }

    /// Record a window resize in physical pixels.
    pub fn record_resize(&self, label: &str, width: u32, height: u32, scale_factor: f64) {
        let Some(kind_key) = kind_key_for_label(label) else {
            return;
        };
        if width < 200 || height < 200 {
            return;
        }
        let mut guard = self.inner.lock();
        let ts = now_ms();
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let entry = guard
            .data
            .windows
            .entry(label.to_owned())
            .or_insert_with(|| SavedBounds {
                x: 80,
                y: 80,
                width,
                height,
                scale_factor: scale,
                always_on_top: false,
                updated_at_ms: ts,
            });
        entry.width = width;
        entry.height = height;
        entry.scale_factor = scale;
        entry.updated_at_ms = ts;
        let snapshot = entry.clone();
        guard.data.defaults.insert(kind_key.to_owned(), snapshot);
        prune_oldest(&mut guard.data.windows);
        guard.dirty = true;
    }

    /// Record whether a pop-out window is pinned on top.
    pub fn set_always_on_top(&self, label: &str, pinned: bool) {
        let Some(kind_key) = kind_key_for_label(label) else {
            return;
        };
        let mut guard = self.inner.lock();
        let ts = now_ms();
        if let Some(entry) = guard.data.windows.get_mut(label) {
            entry.always_on_top = pinned;
            entry.updated_at_ms = ts;
        } else {
            guard.data.windows.insert(
                label.to_owned(),
                SavedBounds {
                    x: 80,
                    y: 80,
                    width: if kind_key == "default:live" { 920 } else { 520 },
                    height: if kind_key == "default:live" { 620 } else { 680 },
                    scale_factor: 1.0,
                    always_on_top: pinned,
                    updated_at_ms: ts,
                },
            );
        }
        if let Some(def) = guard.data.defaults.get_mut(kind_key) {
            def.always_on_top = pinned;
            def.updated_at_ms = ts;
        }
        guard.dirty = true;
    }

    /// Persist dirty bounds atomically to disk.
    pub fn persist(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let mut guard = self.inner.lock();
        if !guard.dirty {
            return;
        }
        let Ok(payload) = serde_json::to_vec_pretty(&guard.data) else {
            return;
        };
        // Serialize writes too: a pin change must not race a close using the same temporary file.
        if write_atomic(path, &payload).is_ok() {
            guard.dirty = false;
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ── Tauri integration & commands ──────────────────────────────────────────────

fn collect_monitor_rects(app: &AppHandle) -> Vec<MonitorRect> {
    let Ok(monitors) = app.available_monitors() else {
        return vec![default_monitor()];
    };
    if monitors.is_empty() {
        return vec![default_monitor()];
    }
    monitors
        .into_iter()
        .map(|m| {
            let area = m.work_area();
            let pos = &area.position;
            let size = &area.size;
            MonitorRect {
                physical_x: f64::from(pos.x),
                physical_y: f64::from(pos.y),
                physical_width: f64::from(size.width),
                physical_height: f64::from(size.height),
                scale_factor: m.scale_factor(),
            }
        })
        .collect()
}

/// If a pop-out window already exists for `chat_id`, show/unminimize/focus it and return `true`.
#[must_use]
pub fn focus_existing_chat(app: &AppHandle, chat_id: &str) -> bool {
    let Some(label) = chat_window_label(chat_id) else {
        return false;
    };
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        true
    } else {
        false
    }
}

fn open_or_focus_sync(
    app: &AppHandle,
    popouts: &PopoutState,
    target: &ValidatedTarget,
    near_cursor: bool,
    window_frame: Option<bool>,
    title: &str,
) -> Result<String, String> {
    let label = target.label();
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        if let ValidatedTarget::Live {
            bot_id,
            target_id,
            computer_id,
            ..
        } = target
        {
            let _ = app.emit_to(
                &label,
                EVENT_LIVE_TARGET,
                LiveTargetPayload {
                    bot_id: bot_id.clone(),
                    target_id: target_id.clone(),
                    computer_id: computer_id.clone(),
                },
            );
        }
        return Ok(label);
    }

    let monitors = collect_monitor_rects(app);
    let cursor = if near_cursor {
        app.cursor_position().ok().map(|p| (p.x, p.y))
    } else {
        None
    };
    let cascade_index = app
        .webview_windows()
        .keys()
        .filter(|l| match target.kind() {
            PopoutKind::Chat => is_chat_popout_label(l),
            PopoutKind::Live => is_live_popout_label(l),
        })
        .count();
    let resolved = popouts.resolve_for(&label, target.kind(), cursor, &monitors, cascade_index);
    let url = target.relative_url(resolved.always_on_top);
    let (min_w, min_h) = match target.kind() {
        PopoutKind::Chat => CHAT_MIN_LOGICAL,
        PopoutKind::Live => LIVE_MIN_LOGICAL,
    };

    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(resolved.logical_width, resolved.logical_height)
        .min_inner_size(
            min_w.min(resolved.logical_width),
            min_h.min(resolved.logical_height),
        )
        .decorations(window_frame.unwrap_or(false))
        .always_on_top(resolved.always_on_top)
        .resizable(true)
        .visible(false)
        .on_navigation(|nav_url| is_app_origin(nav_url, cfg!(debug_assertions)))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()
        .map_err(|e| format!("could not open pop-out window: {e}"))?;

    let _ = win.set_position(PhysicalPosition::new(
        resolved.physical_x,
        resolved.physical_y,
    ));
    let _ = win.set_size(PhysicalSize::new(
        resolved.physical_width,
        resolved.physical_height,
    ));
    // Seed both dimensions and the pin state before any later move/resize events.
    let scale = win.scale_factor().unwrap_or(1.0);
    popouts.record_move(&label, resolved.physical_x, resolved.physical_y, scale);
    popouts.record_resize(
        &label,
        resolved.physical_width,
        resolved.physical_height,
        scale,
    );
    popouts.set_always_on_top(&label, resolved.always_on_top);
    let _ = win.show();
    let _ = win.set_focus();
    Ok(label)
}

/// Open or focus a pop-out chat or live-view window. Main window only.
///
/// # Errors
/// When called from a non-main window, with invalid identifiers, or if window creation fails.
#[tauri::command]
pub async fn open_popout_window(
    window: Window,
    app: AppHandle,
    request: OpenPopoutRequest,
) -> Result<String, String> {
    require_window(&window, MAIN_ONLY)?;
    let req = request;
    let near_cursor = req.near_cursor.unwrap_or(false);
    let window_frame = req.window_frame;
    let fallback_title = match req.kind {
        PopoutKind::Chat => "Praxi-Bot Chat",
        PopoutKind::Live => "Praxi-Bot Live View",
    };
    let title = clean_title(req.title.clone(), fallback_title);
    let target = validate_request(req)?;
    tokio::task::spawn_blocking(move || {
        let popouts = app.state::<Arc<PopoutState>>();
        open_or_focus_sync(&app, &popouts, &target, near_cursor, window_frame, &title)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Daemon address and bearer token for a pop-out window. Never starts/restarts the daemon.
///
/// # Errors
/// When called from a non-popout window or when the daemon cannot be reached.
#[tauri::command]
pub async fn popout_connect(
    window: Window,
    daemon: State<'_, Arc<DaemonHandle>>,
) -> Result<Connection, String> {
    require_window(&window, POPOUT_ONLY)?;
    daemon::connect_existing(&daemon).await
}

/// The owner key of the daemon this app started, for pop-out windows to answer approvals,
/// questions, and takeover controls through the daemon API.
///
/// # Errors
/// When called from a non-popout window or when the owner key is unavailable.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri hands the window over by value
pub fn popout_owner_key(window: Window) -> Result<String, String> {
    require_window(&window, POPOUT_ONLY)?;
    daemon::owner_key().map(str::to_owned)
}

/// Toggle pin-on-top (`alwaysOnTop`) for the calling pop-out window and remember the choice.
///
/// # Errors
/// When called from a non-popout window or if the OS window refuses the flag.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri hands the window over by value
pub fn popout_set_always_on_top(
    window: Window,
    popouts: State<'_, Arc<PopoutState>>,
    pinned: bool,
) -> Result<bool, String> {
    require_window(&window, POPOUT_ONLY)?;
    window
        .set_always_on_top(pinned)
        .map_err(|e| e.to_string())?;
    popouts.set_always_on_top(window.label(), pinned);
    popouts.persist();
    Ok(pinned)
}

/// Bring the main window to the front from a pop-out window.
///
/// # Errors
/// When called from a non-popout window.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // tauri hands the window over by value
pub fn popout_focus_main(window: Window, app: AppHandle) -> Result<(), String> {
    require_window(&window, POPOUT_ONLY)?;
    crate::shell::focus_main(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT_ID: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn validates_chat_and_live_requests_and_rejects_smuggled_ids() {
        let chat = validate_request(OpenPopoutRequest {
            kind: PopoutKind::Chat,
            chat_id: Some(CHAT_ID.to_owned()),
            live_source: None,
            bot_id: None,
            target_id: None,
            computer_id: Some("local".to_owned()),
            near_cursor: None,
            window_frame: None,
            title: None,
        })
        .expect("valid chat");
        assert_eq!(chat.label(), format!("chat-{CHAT_ID}"));
        assert_eq!(
            chat.relative_url(false),
            format!("index.html?window=chat&chatId={CHAT_ID}&pinned=0&computerId=local")
        );

        let bad_chat = validate_request(OpenPopoutRequest {
            kind: PopoutKind::Chat,
            chat_id: Some("not-a-uuid&admin=1".to_owned()),
            live_source: None,
            bot_id: None,
            target_id: None,
            computer_id: None,
            near_cursor: None,
            window_frame: None,
            title: None,
        });
        assert!(bad_chat.is_err());
        assert!(clean_opt_id(Some("bad?target=1".to_owned())).is_err());

        let live = validate_request(OpenPopoutRequest {
            kind: PopoutKind::Live,
            chat_id: None,
            live_source: Some(LiveSource::Local),
            bot_id: Some("bot-1".to_owned()),
            target_id: Some("desktop:0".to_owned()),
            computer_id: None,
            near_cursor: None,
            window_frame: None,
            title: None,
        })
        .expect("live request");
        assert_eq!(live.label(), "live-local");
        assert_eq!(
            live.relative_url(true),
            "index.html?window=live&source=local&pinned=1&botId=bot-1&targetId=desktop:0"
        );
    }

    #[test]
    fn origin_guard_allows_only_app_origins() {
        assert!(is_app_origin(
            &Url::parse("tauri://localhost/index.html?window=chat").unwrap(),
            false
        ));
        assert!(is_app_origin(
            &Url::parse("http://tauri.localhost/index.html").unwrap(),
            false
        ));
        assert!(is_app_origin(
            &Url::parse("https://tauri.localhost/index.html").unwrap(),
            false
        ));
        assert!(is_app_origin(
            &Url::parse("http://localhost:1420/index.html").unwrap(),
            true
        ));
        assert!(!is_app_origin(
            &Url::parse("http://localhost:1420/index.html").unwrap(),
            false
        ));
        assert!(!is_app_origin(
            &Url::parse("http://127.0.0.1:47821/v1/health").unwrap(),
            true
        ));
        assert!(!is_app_origin(
            &Url::parse("https://evil.example.com/").unwrap(),
            true
        ));
        assert!(!is_app_origin(
            &Url::parse("file:///C:/secret.txt").unwrap(),
            true
        ));
        assert!(!is_app_origin(
            &Url::parse("http://tauri.localhost:47821/").unwrap(),
            false
        ));
        assert!(!is_app_origin(
            &Url::parse("http://tauri.localhost.evil.test/").unwrap(),
            false
        ));
        assert!(!is_app_origin(
            &Url::parse("data:text/html,evil").unwrap(),
            false
        ));
        assert_eq!(
            serde_json::to_value(LiveTargetPayload {
                bot_id: Some("bot-1".into()),
                target_id: None,
                computer_id: Some("local".into())
            })
            .unwrap(),
            serde_json::json!({ "botId": "bot-1", "targetId": null, "computerId": "local" })
        );
    }

    #[test]
    fn multi_monitor_and_dpi_resolution_scales_and_clamps_safely() {
        let primary = MonitorRect {
            physical_x: 0.0,
            physical_y: 0.0,
            physical_width: 1920.0,
            physical_height: 1080.0,
            scale_factor: 1.0,
        };
        let secondary_hidpi = MonitorRect {
            physical_x: 1920.0,
            physical_y: 0.0,
            physical_width: 3840.0,
            physical_height: 2160.0,
            scale_factor: 2.0,
        };
        let monitors = [primary, secondary_hidpi];

        // Saved on primary at 1x, dragged/restored to secondary 2x via cursor on secondary:
        // logical size (600x700) is preserved and scaled to 1200x1400 physical on 2x monitor.
        let saved = SavedBounds {
            x: 100,
            y: 100,
            width: 600,
            height: 700,
            scale_factor: 1.0,
            always_on_top: true,
            updated_at_ms: 1,
        };
        let on_hidpi = resolve_popout_bounds(
            Some(&saved),
            false,
            Some((2500.0, 500.0)),
            &monitors,
            CHAT_MIN_LOGICAL,
            CHAT_DEFAULT_LOGICAL,
            0,
        );
        assert_eq!(on_hidpi.physical_width, 1200);
        assert_eq!(on_hidpi.physical_height, 1400);
        assert!(on_hidpi.physical_x >= 1920);
        assert!(on_hidpi.physical_x + i32::try_from(on_hidpi.physical_width).unwrap() <= 5760);
        assert!(on_hidpi.always_on_top);

        // Disconnected monitor: saved bounds on monitor at x=6000 clamp back onto primary.
        let disconnected = SavedBounds {
            x: 6200,
            y: 200,
            width: 520,
            height: 680,
            scale_factor: 1.0,
            always_on_top: false,
            updated_at_ms: 2,
        };
        let recovered = resolve_popout_bounds(
            Some(&disconnected),
            true,
            None,
            &[primary],
            CHAT_MIN_LOGICAL,
            CHAT_DEFAULT_LOGICAL,
            0,
        );
        assert!(recovered.physical_x >= 0 && recovered.physical_x < 1920);
        assert!(recovered.physical_y >= 0 && recovered.physical_y < 1080);
    }

    #[test]
    fn bounds_store_persists_round_trip_and_ignores_minimized() {
        let dir = std::env::temp_dir().join(format!("praxi-popout-test-{}", std::process::id()));
        let path = dir.join("popout-windows.json");
        let _ = std::fs::remove_dir_all(&dir);

        let label = format!("chat-{CHAT_ID}");
        let state = PopoutState::load_from(Some(path.clone()));
        state.record_move(&label, 120, 140, 1.5);
        state.record_resize(&label, 780, 960, 1.5);
        state.set_always_on_top(&label, true);
        // Minimized sentinel values must be ignored.
        state.record_move(&label, -32000, -32000, 1.5);
        state.record_resize(&label, 0, 0, 1.5);
        state.persist();

        let reloaded = PopoutState::load_from(Some(path.clone()));
        let resolved =
            reloaded.resolve_for(&label, PopoutKind::Chat, None, &[default_monitor()], 0);
        assert_eq!(resolved.physical_x, 120);
        assert_eq!(resolved.physical_y, 140);
        assert_eq!(resolved.physical_width, 520); // 780 / 1.5 * 1.0
        assert_eq!(resolved.physical_height, 640); // 960 / 1.5 * 1.0
        assert!(resolved.always_on_top);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
