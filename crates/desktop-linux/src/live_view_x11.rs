//! Linux X11 live-view adapter: persistent `XGetImage` capture, `RandR` monitor
//! geometry tracking, and cancel-safe `XTest` input injection.
//!
//! Wayland sessions, missing `DISPLAY`, and servers lacking `XTest` are
//! cleanly reported with explicit [`live_view::Reason`] codes instead of guessing
//! or invoking screenshots/AT-SPI/OCR tools per frame.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use desktop_api::DesktopError;
use desktop_api::live_view::{
    AvailabilityProbeFn, CaptureDriver, CaptureDriverFactory, CaptureFailure, CaptureScope,
    CapturedRawInfo, DesktopViewTarget, DesktopViewTargetConfig, InputDriver, InputDriverFactory,
    LiveViewOptions, MonitorDescriptor, PixelFormat, PixelRect, map_normalized_point,
    select_capture_rect, validate_positional_geometry, wheel_pixels_to_x11_steps,
};
use live_view::{
    Button, InputEvent, KeyPhase, Modifier, PointerPhase, Reason, SourceRect, ViewError,
};

use crate::pure::{char_to_keysym, detect_display_session};

// ── X11 capability & environment validation ───────────────────────────────────

/// Validate X11 display session environment variables for live view.
///
/// # Errors
///
/// Returns [`Reason::Unsupported`] on Wayland sessions and
/// [`Reason::Unavailable`] when no X11 `DISPLAY` is configured.
pub fn validate_x11_env(
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    x11_display: Option<&str>,
) -> Result<String, Reason> {
    match detect_display_session(xdg_session_type, wayland_display, x11_display) {
        Ok(dpy) => Ok(dpy),
        Err(DesktopError::Unsupported(_)) => Err(Reason::Unsupported),
        Err(_) => Err(Reason::Unavailable),
    }
}

// ── Mockable X11 capture & input abstractions ─────────────────────────────────

/// Pixmap metadata queried from the X server setup before image allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11PixmapFormat {
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
    pub big_endian: bool,
}

/// Complete display topology returned by an X11 capture connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11DisplayLayout {
    pub root_rect: PixelRect,
    pub monitors: Vec<MonitorDescriptor>,
    pub format: X11PixmapFormat,
    pub display_token: u64,
}

/// Abstract X11 display connection used for screen capture and geometry queries.
pub trait X11CaptureConnection: Send + 'static {
    /// Query current root dimensions, `RandR` monitor geometry, and pixmap format.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFailure`] if the X server connection fails or returns unsupported metadata.
    fn query_layout(&mut self) -> Result<X11DisplayLayout, CaptureFailure>;

    /// Capture `rect` via `XGetImage` (`ZPixmap`) into `out_raw`, returning pixel format and stride.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureFailure`] if capture fails or dimensions/format violate bounds.
    fn get_image(
        &mut self,
        rect: PixelRect,
        out_raw: &mut Vec<u8>,
    ) -> Result<(PixelFormat, usize), CaptureFailure>;
}

/// Snapshot of X11 keyboard mapping metadata for a display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11KeymapSnapshot {
    pub min_keycode: u8,
    pub max_keycode: u8,
    pub keysyms_per_keycode: u8,
    pub keysyms: Vec<u32>,
}

/// Snapshot of currently depressed X11 keycodes (`XQueryKeymap`) and pointer buttons (`XQueryPointer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11ServerPressedState {
    pub keys: [u8; 32],
    pub buttons_1_to_5: u8,
}

impl X11ServerPressedState {
    /// Returns whether `keycode` is currently marked pressed in `keys`.
    #[must_use]
    pub fn is_key_down(&self, keycode: u8) -> bool {
        let byte_idx = usize::from(keycode / 8);
        let bit_mask = 1_u8 << (keycode % 8);
        (self.keys[byte_idx] & bit_mask) != 0
    }

    /// Returns whether pointer button `1..=5` is depressed in `XQueryPointer`'s modifier mask.
    #[must_use]
    pub fn is_button_down(&self, button: u8) -> Option<bool> {
        if (1..=5).contains(&button) {
            Some((self.buttons_1_to_5 & (1_u8 << (button - 1))) != 0)
        } else {
            None
        }
    }
}

/// Abstract X11 connection used for `XTest` input injection and keymap updates.
pub trait X11InputConnection: Send + 'static {
    /// Check if `XTest` is present on this X11 display.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if the X11 connection cannot be queried.
    fn has_xtest(&mut self) -> Result<bool, ViewError>;

    /// Fetch the server's keyboard mapping table.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if querying X11 fails.
    fn keymap(&mut self) -> Result<X11KeymapSnapshot, ViewError>;

    /// Temporarily bind a single scratch keycode to `keysym` in both shift columns.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] or [`ViewError::Unsupported`] on failure.
    fn bind_scratch_keycode(&mut self, keycode: u8, keysym: u32) -> Result<(), ViewError>;

    /// Unbind a previously allocated scratch keycode back to `NoSymbol`.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if the X11 connection fails.
    fn unbind_scratch_keycode(&mut self, keycode: u8) -> Result<(), ViewError>;

    /// Inject absolute pointer motion in root coordinates.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if `XTest` injection fails.
    fn motion(&mut self, x: i16, y: i16) -> Result<(), ViewError>;

    /// Inject an X11 mouse button press (`press = true`) or release (`false`).
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if `XTest` injection fails.
    fn button(&mut self, button: u8, press: bool) -> Result<(), ViewError>;

    /// Inject an X11 key press (`press = true`) or release (`false`).
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if `XTest` injection fails.
    fn key(&mut self, keycode: u8, press: bool) -> Result<(), ViewError>;

    /// Optionally query the current X11 root window rectangle before injecting
    /// positional pointer/wheel events so stale coordinates after a resolution
    /// change are rejected immediately.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if querying root geometry fails.
    fn root_rect(&mut self) -> Result<Option<PixelRect>, ViewError> {
        Ok(None)
    }

    /// Query currently held keycodes (`XQueryKeymap`) and core pointer buttons (`XQueryPointer`)
    /// on the X server, if supported by this connection.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unavailable`] if the X11 connection fails while querying state.
    fn query_server_pressed_state(&mut self) -> Result<Option<X11ServerPressedState>, ViewError> {
        Ok(None)
    }

    /// Returns `true` when the underlying X11 socket connection has been lost
    /// (for example on server restart or broken pipe) and can no longer carry requests.
    fn is_connection_lost(&self) -> bool {
        false
    }
}

/// Map X11 depth/bpp/endianness into a supported [`PixelFormat`].
///
/// # Errors
///
/// Returns [`CaptureFailure::Unsupported`] for unsupported X11 visuals/depths.
pub const fn x11_pixel_format(fmt: X11PixmapFormat) -> Result<PixelFormat, CaptureFailure> {
    match (fmt.depth, fmt.bits_per_pixel) {
        (24 | 32, 32) => Ok(PixelFormat::Bgra32),
        (24 | 32, 24) => Ok(PixelFormat::Bgr24),
        (16, 16) => {
            if fmt.big_endian {
                Ok(PixelFormat::Rgb565Be)
            } else {
                Ok(PixelFormat::Rgb565Le)
            }
        }
        _ => Err(CaptureFailure::Unsupported(Reason::Unsupported)),
    }
}

/// Derive validated X11 `ZPixmap` row stride from raw reply length without panics.
///
/// # Errors
///
/// Returns [`CaptureFailure::LimitExceeded`] if dimensions or buffer length mismatch.
pub fn compute_x11_stride(
    raw_len: usize,
    width: u32,
    height: u32,
    fmt: PixelFormat,
) -> Result<usize, CaptureFailure> {
    let bpp = match fmt {
        PixelFormat::Bgra32 => 4usize,
        PixelFormat::Bgr24 => 3,
        PixelFormat::Rgb565Le | PixelFormat::Rgb565Be => 2,
    };
    let min_row = (width as usize)
        .checked_mul(bpp)
        .ok_or(CaptureFailure::LimitExceeded)?;
    if height == 0 {
        return Err(CaptureFailure::LimitExceeded);
    }
    let row_stride = raw_len / (height as usize);
    if row_stride < min_row {
        return Err(CaptureFailure::LimitExceeded);
    }
    let expected = row_stride
        .checked_mul(height as usize)
        .ok_or(CaptureFailure::LimitExceeded)?;
    if raw_len < expected {
        return Err(CaptureFailure::LimitExceeded);
    }
    Ok(row_stride)
}

// ── Generic X11 CaptureDriver ─────────────────────────────────────────────────

/// Persistent X11 capture driver generic over [`X11CaptureConnection`].
pub struct X11CaptureDriver<C: X11CaptureConnection> {
    conn: C,
    scope: CaptureScope,
}

impl<C: X11CaptureConnection> X11CaptureDriver<C> {
    /// Wrap a connected X11 capture connection and requested scope.
    #[must_use]
    pub const fn new(conn: C, scope: CaptureScope) -> Self {
        Self { conn, scope }
    }
}

impl<C: X11CaptureConnection> CaptureDriver for X11CaptureDriver<C> {
    fn capture(&mut self, raw_buf: &mut Vec<u8>) -> Result<CapturedRawInfo, CaptureFailure> {
        let layout = self.conn.query_layout()?;
        let _ = x11_pixel_format(layout.format)?;
        let geom = select_capture_rect(
            &self.scope,
            Some(layout.root_rect),
            &layout.monitors,
            layout.display_token,
        )
        .map_err(|r| {
            if r == Reason::FrameBudgetExceeded {
                CaptureFailure::LimitExceeded
            } else {
                CaptureFailure::Unavailable(r)
            }
        })?;
        // Always enforce 16 Mpixel raw limit BEFORE requesting image bytes from X11.
        geom.rect
            .validate_for_capture()
            .map_err(|_| CaptureFailure::LimitExceeded)?;
        let (pixel_format, stride_bytes) = self.conn.get_image(geom.rect, raw_buf)?;
        Ok(CapturedRawInfo {
            geometry: geom,
            format: pixel_format,
            stride_bytes,
        })
    }
}

// ── Generic X11 InputDriver ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
struct PressedKeyRecord {
    keycode: u8,
    modifier: Option<Modifier>,
    scratch: bool,
}

/// Persistent X11 `XTest` input driver tracking session-owned pressed keys,
/// mouse buttons, and temporary scratch keycodes.
pub struct X11InputDriver<C: X11InputConnection> {
    conn: C,
    reconnect: Option<Arc<dyn Fn() -> Result<C, ViewError> + Send + Sync>>,
    pressed_buttons: BTreeSet<u8>,
    pressed_keys: BTreeMap<String, PressedKeyRecord>,
    transient_mods: Vec<(Modifier, u8)>,
    scratch_bindings: BTreeMap<u8, u32>,
    scratch_order: VecDeque<u8>,
    cached_keymap: Option<X11KeymapSnapshot>,
}

impl<C: X11InputConnection> X11InputDriver<C> {
    /// Verify `XTest` support on `conn` and construct a session-tracked input driver.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unsupported`] if the `XTest` extension is missing.
    pub fn new(mut conn: C) -> Result<Self, ViewError> {
        if !conn.has_xtest()? {
            return Err(ViewError::Unsupported);
        }
        Ok(Self {
            conn,
            reconnect: None,
            pressed_buttons: BTreeSet::new(),
            pressed_keys: BTreeMap::new(),
            transient_mods: Vec::new(),
            scratch_bindings: BTreeMap::new(),
            scratch_order: VecDeque::new(),
            cached_keymap: None,
        })
    }

    /// Construct a session-tracked `XTest` input driver with a reconnection factory
    /// invoked automatically when the X11 socket connection is lost.
    ///
    /// # Errors
    ///
    /// Returns [`ViewError::Unsupported`] if the `XTest` extension is missing.
    pub fn with_reconnect(
        conn: C,
        reconnect: Arc<dyn Fn() -> Result<C, ViewError> + Send + Sync>,
    ) -> Result<Self, ViewError> {
        let mut drv = Self::new(conn)?;
        drv.reconnect = Some(reconnect);
        Ok(drv)
    }

    fn clear_local_session_state(&mut self) {
        self.pressed_buttons.clear();
        self.pressed_keys.clear();
        self.transient_mods.clear();
        self.scratch_bindings.clear();
        self.scratch_order.clear();
        self.cached_keymap = None;
    }

    /// Reconnect to the X11 server while preserving locally tracked held keys/buttons/scratches
    /// so `release_all` can still release `XTest` server state over the new connection.
    ///
    /// Returns:
    /// - `Ok(true)`: connected to a live server with `XTest`.
    /// - `Ok(false)`: reconnection factory failed, indicating the X11 server itself is gone.
    /// - `Err(_)`: no reconnection factory is configured, or post-connect validation failed.
    fn try_reconnect_preserving_state(&mut self) -> Result<bool, ViewError> {
        let Some(factory) = self.reconnect.as_ref().map(Arc::clone) else {
            return Err(ViewError::Unavailable);
        };
        let Ok(mut new_conn) = factory() else {
            return Ok(false);
        };
        if !new_conn.has_xtest()? {
            return Err(ViewError::Unsupported);
        }
        self.conn = new_conn;
        self.cached_keymap = None;
        Ok(true)
    }

    fn reconcile_with_server_state(&mut self) {
        let Ok(Some(server)) = self.conn.query_server_pressed_state() else {
            return;
        };
        self.pressed_keys
            .retain(|_, rec| server.is_key_down(rec.keycode));
        self.transient_mods
            .retain(|&(_, kc)| server.is_key_down(kc));
        self.pressed_buttons
            .retain(|&btn| server.is_button_down(btn) != Some(false));
    }

    fn is_shift_held(&self) -> bool {
        self.pressed_keys
            .values()
            .any(|r| r.modifier.as_ref() == Some(&Modifier::Shift))
            || self
                .transient_mods
                .iter()
                .any(|(m, _)| *m == Modifier::Shift)
    }

    fn effective_modifiers(&self, event_mods: &[Modifier]) -> Vec<Modifier> {
        let mut out = event_mods.to_vec();
        for rec in self.pressed_keys.values() {
            if let Some(m) = &rec.modifier
                && !out.contains(m)
            {
                out.push(m.clone());
            }
        }
        for (m, _) in &self.transient_mods {
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
        out
    }

    fn release_non_shift_modifiers(&mut self) -> Result<(), ViewError> {
        let mut had_err = false;
        let held_mod_codes: Vec<(String, u8)> = self
            .pressed_keys
            .iter()
            .filter(|(_, r)| matches!(r.modifier, Some(Modifier::Ctrl | Modifier::Alt)))
            .map(|(code, r)| (code.clone(), r.keycode))
            .collect();
        for (code, kc) in held_mod_codes {
            if self.conn.key(kc, false).is_ok() {
                self.pressed_keys.remove(&code);
            } else {
                had_err = true;
            }
        }
        let mut idx = self.transient_mods.len();
        while idx > 0 {
            idx -= 1;
            if matches!(self.transient_mods[idx].0, Modifier::Ctrl | Modifier::Alt) {
                let kc = self.transient_mods[idx].1;
                if self.conn.key(kc, false).is_ok() {
                    let _ = self.transient_mods.remove(idx);
                } else {
                    had_err = true;
                }
            }
        }
        if had_err {
            Err(ViewError::PartialInjection)
        } else {
            Ok(())
        }
    }

    fn ensure_keymap(&mut self) -> Result<&X11KeymapSnapshot, ViewError> {
        if self.cached_keymap.is_none() {
            self.cached_keymap = Some(self.conn.keymap()?);
        }
        Ok(self.cached_keymap.as_ref().expect("populated above"))
    }

    fn find_permanent_keycode_in_col(
        &mut self,
        keysym: u32,
        col: usize,
    ) -> Result<Option<u8>, ViewError> {
        let scratch_copy = self.scratch_bindings.clone();
        let km = self.ensure_keymap()?;
        let per = usize::from(km.keysyms_per_keycode.max(1));
        for (idx, chunk) in km.keysyms.chunks(per).enumerate() {
            let Ok(offset) = u8::try_from(idx) else {
                break;
            };
            let kc = km.min_keycode.saturating_add(offset);
            if kc > km.max_keycode {
                break;
            }
            if !scratch_copy.contains_key(&kc) {
                let effective = chunk
                    .get(col)
                    .copied()
                    .filter(|&sym| sym != 0)
                    .or_else(|| (chunk.len() == 1).then(|| chunk[0]).filter(|&s| s != 0));
                if effective == Some(keysym) {
                    return Ok(Some(kc));
                }
            }
        }
        Ok(None)
    }

    fn resolve_keycode(&mut self, keysym: u32) -> Result<(u8, bool), ViewError> {
        if keysym == 0 {
            return Err(ViewError::Unsupported);
        }
        let col = usize::from(self.is_shift_held());
        if let Some(kc) = self.find_permanent_keycode_in_col(keysym, col)? {
            return Ok((kc, false));
        }
        self.allocate_or_reuse_scratch_keycode(keysym)
    }

    fn allocate_or_reuse_scratch_keycode(&mut self, keysym: u32) -> Result<(u8, bool), ViewError> {
        // Reuse an existing scratch binding for the same keysym without issuing a new
        // `ChangeKeyboardMapping` (no redundant `MappingNotify` storm).
        if let Some((&kc, _)) = self.scratch_bindings.iter().find(|&(&kc, &sym)| {
            sym == keysym
                && !self.pressed_keys.values().any(|r| r.keycode == kc)
                && !self.transient_mods.iter().any(|&(_, m_kc)| m_kc == kc)
        }) {
            self.scratch_order.retain(|&x| x != kc);
            self.scratch_order.push_back(kc);
            return Ok((kc, true));
        }

        let km = self.ensure_keymap()?.clone();
        let per = usize::from(km.keysyms_per_keycode.max(1));
        let mut unused_kc: Option<u8> = None;
        for (idx, chunk) in km.keysyms.chunks(per).enumerate() {
            let Ok(offset) = u8::try_from(idx) else {
                break;
            };
            let kc = km.min_keycode.saturating_add(offset);
            if kc > km.max_keycode {
                break;
            }
            let pressed_now = self.pressed_keys.values().any(|r| r.keycode == kc)
                || self.transient_mods.iter().any(|&(_, m_kc)| m_kc == kc);
            if kc != 8
                && !pressed_now
                && !self.scratch_bindings.contains_key(&kc)
                && chunk.iter().all(|&s| s == 0)
            {
                unused_kc = Some(kc);
                break;
            }
        }

        // If all unused slots in the keymap are currently occupied by earlier scratch bindings
        // from this session, evict the oldest non-held scratch slot. Never clobber permanent
        // server keycodes when the server keymap had zero unused slots.
        let scratch = if let Some(kc) = unused_kc {
            kc
        } else if let Some(pos) = self.scratch_order.iter().position(|&kc| {
            !self.pressed_keys.values().any(|r| r.keycode == kc)
                && !self.transient_mods.iter().any(|&(_, m_kc)| m_kc == kc)
        }) {
            self.scratch_order.remove(pos).expect("valid index")
        } else {
            return Err(ViewError::Unsupported);
        };

        self.conn.bind_scratch_keycode(scratch, keysym)?;
        self.scratch_bindings.insert(scratch, keysym);
        self.scratch_order.retain(|&x| x != scratch);
        self.scratch_order.push_back(scratch);
        Ok((scratch, true))
    }

    fn apply_transient_modifiers(&mut self, mods: &[Modifier]) -> Result<(), ViewError> {
        for m in mods {
            let already = self
                .pressed_keys
                .values()
                .any(|r| r.modifier.as_ref() == Some(m))
                || self.transient_mods.iter().any(|(tm, _)| tm == m);
            if already {
                continue;
            }
            let sym = modifier_to_x11_keysym(m);
            let (kc, _scratch) = self.resolve_keycode(sym)?;
            // Record the transient modifier before sending `key(kc, true)` so that if
            // `xtest_fake_input` reaches the server before `sync()` fails, `release_all`
            // still releases the held modifier keycode.
            self.transient_mods.push((m.clone(), kc));
            if self.conn.key(kc, true).is_err() {
                let _ = self.release_transient_modifiers();
                return Err(ViewError::PartialInjection);
            }
        }
        Ok(())
    }

    fn release_transient_modifiers(&mut self) -> Result<(), ViewError> {
        let mut had_err = false;
        let mut idx = self.transient_mods.len();
        while idx > 0 {
            idx -= 1;
            let kc = self.transient_mods[idx].1;
            if self.conn.key(kc, false).is_ok() {
                let _ = self.transient_mods.remove(idx);
            } else {
                had_err = true;
            }
        }
        if had_err {
            Err(ViewError::PartialInjection)
        } else {
            Ok(())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_pointer(
        &mut self,
        phase: &PointerPhase,
        x: f64,
        y: f64,
        button: Option<&Button>,
        buttons: u8,
        modifiers: &[Modifier],
        source_rect: &SourceRect,
    ) -> Result<(), ViewError> {
        let btn_code = match phase {
            PointerPhase::Move => None,
            PointerPhase::Down | PointerPhase::Up => Some(
                button
                    .map(x11_button_code)
                    .ok_or(ViewError::InvalidRecord)?,
            ),
        };
        let (px, py) = map_normalized_point(x, y, source_rect);
        // Physical coordinates are finite; clamp cleanly to X11 `i16` range.
        #[allow(clippy::cast_possible_truncation)]
        let rx = px.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        #[allow(clippy::cast_possible_truncation)]
        let ry = py.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;

        if matches!(phase, PointerPhase::Down) && !modifiers.is_empty() {
            self.apply_transient_modifiers(modifiers)?;
        }

        if self.conn.motion(rx, ry).is_err() {
            let _ = self.release_transient_modifiers();
            return Err(ViewError::Unavailable);
        }

        match phase {
            PointerPhase::Move => Ok(()),
            PointerPhase::Down => {
                let btn = btn_code.expect("checked above");
                if !self.pressed_buttons.contains(&btn) {
                    // Record button as held before sending so a `sync()` failure after
                    // `xtest_fake_input` is still cleaned up on `release_all`.
                    self.pressed_buttons.insert(btn);
                    if self.conn.button(btn, true).is_err() {
                        let _ = self.release_transient_modifiers();
                        return Err(ViewError::PartialInjection);
                    }
                }
                Ok(())
            }
            PointerPhase::Up => {
                let btn = btn_code.expect("checked above");
                let res = if self.pressed_buttons.contains(&btn) {
                    match self.conn.button(btn, false) {
                        Ok(()) => {
                            self.pressed_buttons.remove(&btn);
                            Ok(())
                        }
                        Err(_) => Err(ViewError::PartialInjection),
                    }
                } else {
                    // Preserve physical mouse state if we did not press this button.
                    Ok(())
                };
                if buttons == 0 && self.pressed_buttons.is_empty() {
                    let mod_res = self.release_transient_modifiers();
                    if res.is_ok() {
                        return mod_res;
                    }
                }
                res
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_wheel(
        &mut self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: &[Modifier],
        source_rect: &SourceRect,
        cancelled: &AtomicBool,
    ) -> Result<(), ViewError> {
        let (px, py) = map_normalized_point(x, y, source_rect);
        #[allow(clippy::cast_possible_truncation)]
        let rx = px.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        #[allow(clippy::cast_possible_truncation)]
        let ry = py.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;

        if !modifiers.is_empty() {
            self.apply_transient_modifiers(modifiers)?;
        }
        if self.conn.motion(rx, ry).is_err() {
            let _ = self.release_transient_modifiers();
            return Err(ViewError::Unavailable);
        }

        let mut sent_any = false;
        let v_steps = wheel_pixels_to_x11_steps(delta_y);
        let v_btn = if delta_y > 0.0 { 5 } else { 4 };
        for _ in 0..v_steps {
            if cancelled.load(Ordering::Acquire) {
                let _ = self.release_transient_modifiers();
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            self.pressed_buttons.insert(v_btn);
            if self.conn.button(v_btn, true).is_err() {
                let _ = self.release_transient_modifiers();
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            if self.conn.button(v_btn, false).is_err() {
                let _ = self.release_transient_modifiers();
                return Err(ViewError::PartialInjection);
            }
            self.pressed_buttons.remove(&v_btn);
            sent_any = true;
        }

        let h_steps = wheel_pixels_to_x11_steps(delta_x);
        let h_btn = if delta_x > 0.0 { 7 } else { 6 };
        for _ in 0..h_steps {
            if cancelled.load(Ordering::Acquire) {
                let _ = self.release_transient_modifiers();
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            self.pressed_buttons.insert(h_btn);
            if self.conn.button(h_btn, true).is_err() {
                let _ = self.release_transient_modifiers();
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            if self.conn.button(h_btn, false).is_err() {
                let _ = self.release_transient_modifiers();
                return Err(ViewError::PartialInjection);
            }
            self.pressed_buttons.remove(&h_btn);
            sent_any = true;
        }

        self.release_transient_modifiers()
    }

    fn press_printable_key_char(
        &mut self,
        code: &str,
        ch: char,
        effective_mods: &[Modifier],
    ) -> Result<(), ViewError> {
        // Printable character (plain, Shift, or `AltGr`): release any held Ctrl/Alt
        // modifiers from an `AltGr` sequence and inject the exact character keysym via a
        // temporary scratch keycode rather than relying on the host's active keyboard layout.
        // Limitation: because unmodified printable characters are bound as Unicode keysyms on
        // dynamically assigned scratch keycodes (rather than fixed physical keycodes/scancodes),
        // applications that read raw scancodes directly (such as games and some terminals) will
        // not observe them as hardware keystrokes.
        self.release_non_shift_modifiers()?;
        let sym = char_to_keysym(ch);
        if sym == 0 {
            return Err(ViewError::Unsupported);
        }
        let mut added_transient_shift = false;
        let (kc, scratch) = if self.is_shift_held() {
            if let Some(kc) = self.find_permanent_keycode_in_col(sym, 1)? {
                (kc, false)
            } else {
                self.allocate_or_reuse_scratch_keycode(sym)?
            }
        } else if let Some(kc) = self.find_permanent_keycode_in_col(sym, 0)? {
            (kc, false)
        } else if effective_mods.contains(&Modifier::Shift)
            && let Some(kc) = self.find_permanent_keycode_in_col(sym, 1)?
        {
            self.apply_transient_modifiers(&[Modifier::Shift])?;
            added_transient_shift = true;
            (kc, false)
        } else {
            self.allocate_or_reuse_scratch_keycode(sym)?
        };

        // Record before sending `key(kc, true)` so a `sync()` failure is still
        // released on `release_all()`.
        self.pressed_keys.insert(
            code.to_string(),
            PressedKeyRecord {
                keycode: kc,
                modifier: None,
                scratch,
            },
        );
        if self.conn.key(kc, true).is_err() {
            if added_transient_shift {
                let _ = self.release_transient_modifiers();
                return Err(ViewError::PartialInjection);
            }
            return Err(ViewError::Unavailable);
        }
        Ok(())
    }

    fn handle_key(
        &mut self,
        phase: &KeyPhase,
        code: &str,
        key: &str,
        repeat: bool,
        modifiers: &[Modifier],
    ) -> Result<(), ViewError> {
        // Browser `AltGraph` keydown/keyup is a client-side layout shift latch; pressing
        // `XK_Alt_R` on the X server would leave `Mod1` held during subsequent character keys.
        if key == "AltGraph" {
            return Ok(());
        }
        match phase {
            KeyPhase::Down => {
                if let Some(existing) = self.pressed_keys.get(code) {
                    let kc = existing.keycode;
                    if repeat {
                        return self
                            .conn
                            .key(kc, true)
                            .map_err(|_| ViewError::PartialInjection);
                    }
                    return Ok(());
                }

                let effective_mods = self.effective_modifiers(modifiers);
                if let Some(ch) = is_printable_text_key(code, key, &effective_mods) {
                    return self.press_printable_key_char(code, ch, &effective_mods);
                }

                let chord = effective_mods.iter().any(|m| !matches!(m, Modifier::Shift));
                let (sym, mod_tag) = dom_key_to_x11_keysym(code, key, chord)?;
                let mod_only: Vec<Modifier> = modifiers
                    .iter()
                    .filter(|m| Some(*m) != mod_tag.as_ref())
                    .cloned()
                    .collect();
                let had_transient_before = !self.transient_mods.is_empty();
                if !mod_only.is_empty() {
                    self.apply_transient_modifiers(&mod_only)?;
                }
                let added_transient = !had_transient_before && !self.transient_mods.is_empty();
                let (kc, scratch) = match self.resolve_keycode(sym) {
                    Ok(v) => v,
                    Err(e) => {
                        if added_transient {
                            let _ = self.release_transient_modifiers();
                        }
                        return Err(e);
                    }
                };
                self.pressed_keys.insert(
                    code.to_string(),
                    PressedKeyRecord {
                        keycode: kc,
                        modifier: mod_tag,
                        scratch,
                    },
                );
                if self.conn.key(kc, true).is_err() {
                    if added_transient {
                        let _ = self.release_transient_modifiers();
                        return Err(ViewError::PartialInjection);
                    }
                    return Err(ViewError::Unavailable);
                }
                Ok(())
            }
            KeyPhase::Up => {
                let Some(rec) = self.pressed_keys.get(code).cloned() else {
                    // Only release keys originally pressed by our session.
                    return Ok(());
                };
                if self.conn.key(rec.keycode, false).is_err() {
                    // Keep `code` recorded in `pressed_keys` (and keep any scratch keycode bound)
                    // so `release_all` will retry releasing the stuck key.
                    return Err(ViewError::PartialInjection);
                }
                self.pressed_keys.remove(code);
                if self.pressed_keys.is_empty() {
                    return self.release_transient_modifiers();
                }
                Ok(())
            }
        }
    }

    fn handle_text(&mut self, text: &str, cancelled: &AtomicBool) -> Result<(), ViewError> {
        if text.is_empty() || text.len() > 4096 {
            return Err(ViewError::InvalidRecord);
        }
        // Validate all characters upfront before sending any X11 events.
        let mut syms = Vec::with_capacity(text.chars().count());
        for ch in text.chars() {
            if ch < ' ' && !matches!(ch, '\n' | '\r' | '\t') || ch == '\u{007f}' {
                return Err(ViewError::Unsupported);
            }
            syms.push(char_to_keysym(ch));
        }

        let mut sent_any = false;
        for (idx, sym) in syms.into_iter().enumerate() {
            if cancelled.load(Ordering::Acquire) {
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            let (kc, scratch) = match self.resolve_keycode(sym) {
                Ok(v) => v,
                Err(e) => {
                    return Err(if sent_any {
                        ViewError::PartialInjection
                    } else {
                        e
                    });
                }
            };
            let track_key = format!("__text_{kc}_{idx}");
            self.pressed_keys.insert(
                track_key.clone(),
                PressedKeyRecord {
                    keycode: kc,
                    modifier: None,
                    scratch,
                },
            );
            if self.conn.key(kc, true).is_err() {
                return Err(if sent_any {
                    ViewError::PartialInjection
                } else {
                    ViewError::Unavailable
                });
            }
            if self.conn.key(kc, false).is_err() {
                // Keep `track_key` in `pressed_keys` so `release_all` retries releasing it.
                return Err(ViewError::PartialInjection);
            }
            self.pressed_keys.remove(&track_key);
            sent_any = true;
        }
        Ok(())
    }
}

impl<C: X11InputConnection> InputDriver for X11InputDriver<C> {
    fn inject(
        &mut self,
        event: &InputEvent,
        source_rect: SourceRect,
        virtual_rect: PixelRect,
        cancelled: &AtomicBool,
    ) -> Result<(), ViewError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ViewError::Unavailable);
        }
        if self.conn.is_connection_lost() {
            if !self.try_reconnect_preserving_state()? {
                self.clear_local_session_state();
                return Err(ViewError::Unavailable);
            }
            self.reconcile_with_server_state();
        }
        if event.positional() {
            validate_positional_geometry(&source_rect, virtual_rect)?;
            if let Some(live_root) = self.conn.root_rect()? {
                if live_root != virtual_rect {
                    return Err(ViewError::Unavailable);
                }
                validate_positional_geometry(&source_rect, live_root)?;
            }
        }
        match event {
            InputEvent::Pointer {
                phase,
                x,
                y,
                button,
                buttons,
                modifiers,
            } => self.handle_pointer(
                phase,
                *x,
                *y,
                button.as_ref(),
                *buttons,
                modifiers,
                &source_rect,
            ),
            InputEvent::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            } => self.handle_wheel(
                *x,
                *y,
                *delta_x,
                *delta_y,
                modifiers,
                &source_rect,
                cancelled,
            ),
            InputEvent::Key {
                phase,
                code,
                key,
                repeat,
                modifiers,
                ..
            } => self.handle_key(phase, code, key, *repeat, modifiers),
            InputEvent::Text { text } => self.handle_text(text, cancelled),
            InputEvent::Reset => self.release_all(),
        }
    }

    fn release_all(&mut self) -> Result<(), ViewError> {
        // Check whether any local state was actually held before reconnecting: we never
        // discard owed releases when reconnecting to a live server, because XTest key and
        // button state lives on the X server and survives closing a client socket.
        if self.conn.is_connection_lost() && !self.try_reconnect_preserving_state()? {
            // The X server itself is unreachable/gone, so no live XTest state remains.
            self.clear_local_session_state();
            return Ok(());
        }

        let res = self.send_owed_releases_on_current_conn();
        if res.is_err() && self.conn.is_connection_lost() {
            if !self.try_reconnect_preserving_state()? {
                self.clear_local_session_state();
                return Ok(());
            }
            return self.send_owed_releases_on_current_conn();
        }
        res
    }

    fn is_connection_lost(&self) -> bool {
        self.conn.is_connection_lost()
    }
}

impl<C: X11InputConnection> X11InputDriver<C> {
    fn send_owed_releases_on_current_conn(&mut self) -> Result<(), ViewError> {
        let server_state = match self.conn.query_server_pressed_state() {
            Ok(state) => state,
            Err(err) => {
                if self.conn.is_connection_lost() {
                    return Err(err);
                }
                None
            }
        };

        let mut failed = false;

        let keys: Vec<(String, PressedKeyRecord)> = self
            .pressed_keys
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (code, rec) in keys.into_iter().rev() {
            if let Some(st) = &server_state
                && !st.is_key_down(rec.keycode)
            {
                self.pressed_keys.remove(&code);
                continue;
            }
            if self.conn.key(rec.keycode, false).is_ok() {
                self.pressed_keys.remove(&code);
            } else {
                failed = true;
            }
        }

        let mut idx = self.transient_mods.len();
        while idx > 0 {
            idx -= 1;
            let kc = self.transient_mods[idx].1;
            if let Some(st) = &server_state
                && !st.is_key_down(kc)
            {
                let _ = self.transient_mods.remove(idx);
                continue;
            }
            if self.conn.key(kc, false).is_ok() {
                let _ = self.transient_mods.remove(idx);
            } else {
                failed = true;
            }
        }

        let buttons: Vec<u8> = self.pressed_buttons.iter().copied().collect();
        for btn in buttons {
            if let Some(st) = &server_state
                && st.is_button_down(btn) == Some(false)
            {
                self.pressed_buttons.remove(&btn);
                continue;
            }
            if self.conn.button(btn, false).is_ok() {
                self.pressed_buttons.remove(&btn);
            } else {
                failed = true;
            }
        }

        let scratches: Vec<u8> = self.scratch_bindings.keys().copied().collect();
        for kc in scratches {
            let still_held = self.pressed_keys.values().any(|r| r.keycode == kc)
                || self.transient_mods.iter().any(|&(_, mkc)| mkc == kc);
            if still_held {
                failed = true;
                continue;
            }
            if self.conn.unbind_scratch_keycode(kc).is_ok() {
                self.scratch_bindings.remove(&kc);
                self.scratch_order.retain(|&x| x != kc);
            } else {
                failed = true;
            }
        }

        if failed {
            Err(ViewError::Unavailable)
        } else {
            Ok(())
        }
    }
}

// ── DOM KeyboardEvent -> X11 Keysym mapping ───────────────────────────────────

const fn x11_button_code(b: &Button) -> u8 {
    match b {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
    }
}

const fn modifier_to_x11_keysym(m: &Modifier) -> u32 {
    match m {
        Modifier::Shift => 0xFFE1, // XK_Shift_L
        Modifier::Ctrl => 0xFFE3,  // XK_Control_L
        Modifier::Alt => 0xFFE9,   // XK_Alt_L
        Modifier::Meta => 0xFFEB,  // XK_Super_L
    }
}

fn is_fixed_non_text_code(code: &str) -> bool {
    if matches!(
        code,
        "ShiftLeft"
            | "ShiftRight"
            | "ControlLeft"
            | "ControlRight"
            | "AltLeft"
            | "AltRight"
            | "MetaLeft"
            | "MetaRight"
            | "CapsLock"
            | "Enter"
            | "Tab"
            | "Escape"
            | "Backspace"
            | "Delete"
            | "Space"
            | "ArrowLeft"
            | "ArrowUp"
            | "ArrowRight"
            | "ArrowDown"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
    ) {
        return true;
    }
    if let Some(n) = code.strip_prefix('F').and_then(|s| s.parse::<u32>().ok())
        && (1..=12).contains(&n)
    {
        return true;
    }
    false
}

/// Classify whether a DOM `Key` event represents a layout-resolved printable character
/// (including Shift and `AltGr` combinations such as Turkish `ş ğ ü ı ö ç İ` or `@ € { [ ] } \`)
/// rather than a physical key / shortcut chord (`Ctrl+C`, `Alt+F`, `Meta+...`).
#[must_use]
pub fn is_printable_text_key(code: &str, key: &str, effective_mods: &[Modifier]) -> Option<char> {
    if matches!(
        key,
        "Dead" | "Unidentified" | "Process" | "Compose" | "AltGraph"
    ) || is_fixed_non_text_code(code)
    {
        return None;
    }
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if ch < ' ' || ch == '\u{007f}' {
        return None;
    }
    let has_ctrl = effective_mods.contains(&Modifier::Ctrl);
    let has_alt = effective_mods.contains(&Modifier::Alt);
    let has_meta = effective_mods.contains(&Modifier::Meta);
    if has_meta {
        return None;
    }
    if !has_ctrl && !has_alt {
        return Some(ch);
    }
    // AltGr on Windows/Linux browsers reports `modifiers: [Ctrl, Alt]` (or `[Alt]` after
    // `AltRight`), producing non-ASCII characters (`€`, `₺`, `ß`) or layout symbols (`@`, `{`, `}`).
    if has_alt && !ch.is_ascii() {
        return Some(ch);
    }
    if has_alt && !ch.is_ascii_alphanumeric() {
        return Some(ch);
    }
    None
}

/// Resolve DOM `(code, key)` plus active chord modifier status to `(keysym, modifier_tag)`.
///
/// Explicitly rejects `Dead`/`Unidentified`/`Process`/`Compose` or unmappable
/// inputs with [`ViewError::Unsupported`] rather than typing the wrong character.
///
/// # Errors
///
/// Returns [`ViewError::Unsupported`] if the key name or layout mapping is unsupported.
pub fn dom_key_to_x11_keysym(
    code: &str,
    key: &str,
    chord_modifier_active: bool,
) -> Result<(u32, Option<Modifier>), ViewError> {
    if matches!(key, "Dead" | "Unidentified" | "Process" | "Compose") {
        return Err(ViewError::Unsupported);
    }

    let fixed = match code {
        "ShiftLeft" => Some((0xFFE1, Some(Modifier::Shift))),
        "ShiftRight" => Some((0xFFE2, Some(Modifier::Shift))),
        "ControlLeft" => Some((0xFFE3, Some(Modifier::Ctrl))),
        "ControlRight" => Some((0xFFE4, Some(Modifier::Ctrl))),
        "AltLeft" => Some((0xFFE9, Some(Modifier::Alt))),
        "AltRight" => Some((0xFFEA, Some(Modifier::Alt))),
        "MetaLeft" => Some((0xFFEB, Some(Modifier::Meta))),
        "MetaRight" => Some((0xFFEC, Some(Modifier::Meta))),
        "CapsLock" => Some((0xFFE5, None)),
        "Enter" => Some((0xFF0D, None)),
        "Tab" => Some((0xFF09, None)),
        "Escape" => Some((0xFF1B, None)),
        "Backspace" => Some((0xFF08, None)),
        "Delete" => Some((0xFFFF, None)),
        "Space" => Some((0x0020, None)),
        "ArrowLeft" => Some((0xFF51, None)),
        "ArrowUp" => Some((0xFF52, None)),
        "ArrowRight" => Some((0xFF53, None)),
        "ArrowDown" => Some((0xFF54, None)),
        "Home" => Some((0xFF50, None)),
        "End" => Some((0xFF57, None)),
        "PageUp" => Some((0xFF55, None)),
        "PageDown" => Some((0xFF56, None)),
        _ => None,
    };
    if let Some(pair) = fixed {
        return Ok(pair);
    }

    if let Some(n) = code.strip_prefix('F').and_then(|s| s.parse::<u32>().ok())
        && (1..=12).contains(&n)
    {
        return Ok((0xFFBE + n - 1, None));
    }

    if chord_modifier_active {
        let mut chars = key.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && ch.is_ascii_graphic()
        {
            return Ok((char_to_keysym(ch.to_ascii_lowercase()), None));
        }
        return canonical_us_code_keysym(code).map(|s| (s, None));
    }

    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch >= ' ' && ch != '\u{007f}' => Ok((char_to_keysym(ch), None)),
        _ => Err(ViewError::Unsupported),
    }
}

fn canonical_us_code_keysym(code: &str) -> Result<u32, ViewError> {
    if let Some(rest) = code.strip_prefix("Key")
        && rest.len() == 1
        && rest.as_bytes()[0].is_ascii_uppercase()
    {
        return Ok(u32::from(rest.as_bytes()[0].to_ascii_lowercase()));
    }
    if let Some(rest) = code.strip_prefix("Digit")
        && rest.len() == 1
        && rest.as_bytes()[0].is_ascii_digit()
    {
        return Ok(u32::from(rest.as_bytes()[0]));
    }
    match code {
        "Minus" => Ok(u32::from(b'-')),
        "Equal" => Ok(u32::from(b'=')),
        "BracketLeft" => Ok(u32::from(b'[')),
        "BracketRight" => Ok(u32::from(b']')),
        "Backslash" => Ok(u32::from(b'\\')),
        "Semicolon" => Ok(u32::from(b';')),
        "Quote" => Ok(u32::from(b'\'')),
        "Backquote" => Ok(u32::from(b'`')),
        "Comma" => Ok(u32::from(b',')),
        "Period" => Ok(u32::from(b'.')),
        "Slash" => Ok(u32::from(b'/')),
        _ => Err(ViewError::Unsupported),
    }
}

// ── Constructor helper (used by real X11 and hermetic mocks) ──────────────────

/// Build a [`DesktopViewTarget`] from X11 capture/input driver factories.
///
/// # Errors
///
/// Fails with [`Reason::Unavailable`] or [`Reason::FrameBudgetExceeded`] if the
/// initial geometry query / scope selection fails.
pub fn build_x11_view_target<C, I, P>(
    options: &LiveViewOptions,
    mut initial_probe_conn: P,
    capture_factory: Arc<dyn Fn() -> Result<C, CaptureFailure> + Send + Sync>,
    probe: AvailabilityProbeFn,
    input_factory: Option<Arc<dyn Fn() -> Result<I, ViewError> + Send + Sync>>,
    input_reason: Option<Reason>,
) -> Result<Arc<DesktopViewTarget>, Reason>
where
    C: X11CaptureConnection,
    I: X11InputConnection,
    P: X11CaptureConnection,
{
    let layout = initial_probe_conn
        .query_layout()
        .map_err(|_| Reason::Unavailable)?;
    let initial_geometry = select_capture_rect(
        &options.scope,
        Some(layout.root_rect),
        &layout.monitors,
        layout.display_token,
    )?;

    let wrapped_capture: CaptureDriverFactory = Arc::new(move |scope| {
        let conn = capture_factory()?;
        Ok(Box::new(X11CaptureDriver::new(conn, scope.clone())))
    });

    let wrapped_input: Option<InputDriverFactory> = input_factory.map(|f| {
        Arc::new(move || -> Result<Box<dyn InputDriver>, ViewError> {
            let conn = f()?;
            Ok(Box::new(X11InputDriver::with_reconnect(
                conn,
                Arc::clone(&f),
            )?))
        }) as InputDriverFactory
    });

    Ok(Arc::new(DesktopViewTarget::new(DesktopViewTargetConfig {
        options: options.clone(),
        initial_geometry,
        capture_factory: wrapped_capture,
        probe,
        input_factory: wrapped_input,
        input_reason,
    })))
}

// ── Concrete Linux x11rb implementation ───────────────────────────────────────

#[cfg(target_os = "linux")]
mod linux_x11 {
    use std::io::IoSlice;
    use std::net::{TcpStream, ToSocketAddrs};
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::{
        AvailabilityProbeFn, CaptureFailure, DesktopViewTarget, LiveViewOptions, MonitorDescriptor,
        PixelFormat, PixelRect, Reason, ViewError, X11CaptureConnection, X11DisplayLayout,
        X11InputConnection, X11KeymapSnapshot, X11PixmapFormat, X11ServerPressedState,
        build_x11_view_target, compute_x11_stride, select_capture_rect, validate_x11_env,
        x11_pixel_format,
    };
    use x11rb::{
        CURRENT_TIME, NONE,
        connection::Connection as _,
        protocol::{
            randr::ConnectionExt as _,
            xproto::{
                BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, ImageFormat,
                ImageOrder, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyButMask, MOTION_NOTIFY_EVENT,
            },
            xtest::ConnectionExt as _,
        },
        reexports::x11rb_protocol::{
            parse_display::{ConnectAddress, parse_display},
            xauth::get_auth,
        },
        rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
        utils::RawFdContainer,
        wrapper::ConnectionExt as _,
    };

    const X11_PROBE_IO_TIMEOUT: Duration = Duration::from_millis(1500);
    /// Total budget for the connection, `XTest` query and layout read made when
    /// the target is created. A server that accepts connections but stalls
    /// fails the open within this, instead of hanging the caller.
    const X11_START_IO_TIMEOUT: Duration = Duration::from_secs(4);

    pub(super) struct BoundedStream {
        inner: DefaultStream,
        deadline: Instant,
    }

    impl Stream for BoundedStream {
        fn poll(&self, mode: PollMode) -> std::io::Result<()> {
            let mut poll_flags = rustix::event::PollFlags::empty();
            if mode.readable() {
                poll_flags |= rustix::event::PollFlags::IN;
            }
            if mode.writable() {
                poll_flags |= rustix::event::PollFlags::OUT;
            }
            let fd = self.inner.as_fd();
            let mut poll_fds = [rustix::event::PollFd::from_borrowed_fd(fd, poll_flags)];
            loop {
                let remaining = self.deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "X11 probe I/O timed out",
                    ));
                }
                let ts = rustix::event::Timespec::try_from(remaining)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                match rustix::event::poll(&mut poll_fds, Some(&ts)) {
                    Ok(0) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "X11 probe I/O timed out",
                        ));
                    }
                    Ok(_) => return Ok(()),
                    Err(rustix::io::Errno::INTR) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }

        fn read(
            &self,
            buf: &mut [u8],
            fd_storage: &mut Vec<RawFdContainer>,
        ) -> std::io::Result<usize> {
            self.inner.read(buf, fd_storage)
        }

        fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> std::io::Result<usize> {
            self.inner.write(buf, fds)
        }

        fn write_vectored(
            &self,
            bufs: &[IoSlice<'_>],
            fds: &mut Vec<RawFdContainer>,
        ) -> std::io::Result<usize> {
            self.inner.write_vectored(bufs, fds)
        }
    }

    pub(super) fn connect_bounded_address(
        addr: &ConnectAddress<'_>,
        display: u16,
        screen: usize,
        deadline: Instant,
    ) -> Result<RustConnection<BoundedStream>, CaptureFailure> {
        let (stream, (family, address)) = match addr {
            ConnectAddress::Hostname(host, port) => {
                let addrs = (*host, *port)
                    .to_socket_addrs()
                    .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?;
                let mut connected = None;
                for sock_addr in addrs {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(CaptureFailure::Unavailable(Reason::Unavailable));
                    }
                    if let Ok(tcp) = TcpStream::connect_timeout(&sock_addr, remaining) {
                        connected =
                            Some(DefaultStream::from_tcp_stream(tcp).map_err(|_| {
                                CaptureFailure::ConnectionLost(Reason::Disconnected)
                            })?);
                        break;
                    }
                }
                let Some(res) = connected else {
                    return Err(CaptureFailure::ConnectionLost(Reason::Disconnected));
                };
                res
            }
            ConnectAddress::Socket(path) => {
                let unix = UnixStream::connect(path)
                    .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?;
                DefaultStream::from_unix_stream(unix)
                    .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?
            }
            _ => return Err(CaptureFailure::ConnectionLost(Reason::Disconnected)),
        };

        let (auth_name, auth_data) = get_auth(family, &address, display)
            .unwrap_or(None)
            .unwrap_or_default();
        let bounded = BoundedStream {
            inner: stream,
            deadline,
        };
        RustConnection::connect_to_stream_with_auth_info(bounded, screen, auth_name, auth_data)
            .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))
    }

    struct RealX11Capture<S: Stream = DefaultStream> {
        dpy: String,
        conn: RustConnection<S>,
        screen_num: usize,
    }

    impl RealX11Capture {
        fn open(dpy: String) -> Result<Self, CaptureFailure> {
            let (conn, screen_num) = x11rb::connect(Some(&dpy))
                .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?;
            Ok(Self {
                dpy,
                conn,
                screen_num,
            })
        }
    }

    impl RealX11Capture<BoundedStream> {
        /// Whether the `XTest` extension answers, within this connection's deadline.
        fn xtest_available(&self) -> bool {
            self.conn
                .xtest_get_version(2, 1)
                .is_ok_and(|cookie| cookie.reply().is_ok())
        }

        fn open_bounded(dpy: String, timeout: Duration) -> Result<Self, CaptureFailure> {
            let deadline = Instant::now() + timeout;
            let parsed = parse_display(Some(&dpy))
                .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?;
            let screen_num = usize::from(parsed.screen);
            for addr in parsed.connect_instruction() {
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    return Err(CaptureFailure::Unavailable(Reason::Unavailable));
                }
                if let Ok(conn) =
                    connect_bounded_address(&addr, parsed.display, screen_num, deadline)
                {
                    return Ok(Self {
                        dpy,
                        conn,
                        screen_num,
                    });
                }
            }
            Err(CaptureFailure::ConnectionLost(Reason::Disconnected))
        }
    }

    fn estimate_dpi(width_px: u32, width_mm: u32) -> u32 {
        if width_mm == 0 {
            return 96;
        }
        ((u64::from(width_px) * 254) / (u64::from(width_mm) * 10))
            .clamp(72, 480)
            .try_into()
            .unwrap_or(96)
    }

    fn hash_layout(root: PixelRect, monitors: &[MonitorDescriptor], timestamp: u32) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(u64::from(root.width));
        mix(u64::from(root.height));
        mix(u64::from(timestamp));
        for m in monitors {
            mix(i64::from(m.rect.x).cast_unsigned());
            mix(i64::from(m.rect.y).cast_unsigned());
            mix(u64::from(m.rect.width));
            mix(u64::from(m.rect.height));
            mix(u64::from(m.dpi));
            mix(u64::from(m.is_primary));
        }
        h
    }

    impl<S: Stream + Send + Sync + 'static> X11CaptureConnection for RealX11Capture<S> {
        fn query_layout(&mut self) -> Result<X11DisplayLayout, CaptureFailure> {
            let setup = self.conn.setup();
            let screen = setup
                .roots
                .get(self.screen_num)
                .ok_or(CaptureFailure::ConnectionLost(Reason::Disconnected))?;
            let root = screen.root;
            let root_depth = screen.root_depth;
            let root_w = u32::from(screen.width_in_pixels);
            let root_h = u32::from(screen.height_in_pixels);
            let geom_reply = self
                .conn
                .get_geometry(root)
                .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?
                .reply()
                .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?;
            let root_rect = PixelRect {
                x: 0,
                y: 0,
                width: if geom_reply.width > 0 {
                    u32::from(geom_reply.width)
                } else {
                    root_w.max(1)
                },
                height: if geom_reply.height > 0 {
                    u32::from(geom_reply.height)
                } else {
                    root_h.max(1)
                },
            };

            let pf = setup
                .pixmap_formats
                .iter()
                .find(|pf| pf.depth == root_depth);
            let format = X11PixmapFormat {
                depth: root_depth,
                bits_per_pixel: pf.map_or(32, |p| p.bits_per_pixel),
                scanline_pad: pf.map_or(32, |p| p.scanline_pad),
                big_endian: setup.image_byte_order == ImageOrder::MSB_FIRST,
            };

            let mut monitors = Vec::new();
            let mut timestamp = 0u32;
            if let Ok(Ok(ver)) = self
                .conn
                .randr_query_version(1, 5)
                .map(x11rb::cookie::Cookie::reply)
                && (ver.major_version > 1 || (ver.major_version == 1 && ver.minor_version >= 5))
                && let Ok(Ok(rep)) = self
                    .conn
                    .randr_get_monitors(root, true)
                    .map(x11rb::cookie::Cookie::reply)
            {
                timestamp = rep.timestamp;
                for m in rep.monitors {
                    if m.width == 0 || m.height == 0 {
                        continue;
                    }
                    let name = self
                        .conn
                        .get_atom_name(m.name)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .map(|r| String::from_utf8_lossy(&r.name).to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("monitor-{}", monitors.len()));
                    monitors.push(MonitorDescriptor {
                        name,
                        is_primary: m.primary,
                        rect: PixelRect {
                            x: i32::from(m.x),
                            y: i32::from(m.y),
                            width: u32::from(m.width),
                            height: u32::from(m.height),
                        },
                        dpi: estimate_dpi(u32::from(m.width), m.width_in_millimeters),
                    });
                }
            }
            if monitors.is_empty() {
                monitors.push(MonitorDescriptor {
                    name: format!("x11:{}", self.dpy),
                    is_primary: true,
                    rect: root_rect,
                    dpi: estimate_dpi(root_rect.width, u32::from(screen.width_in_millimeters)),
                });
            }

            let display_token = hash_layout(root_rect, &monitors, timestamp);
            Ok(X11DisplayLayout {
                root_rect,
                monitors,
                format,
                display_token,
            })
        }

        fn get_image(
            &mut self,
            rect: PixelRect,
            out_raw: &mut Vec<u8>,
        ) -> Result<(PixelFormat, usize), CaptureFailure> {
            rect.validate_for_capture()
                .map_err(|_| CaptureFailure::LimitExceeded)?;
            let left = i16::try_from(rect.x).map_err(|_| CaptureFailure::LimitExceeded)?;
            let top = i16::try_from(rect.y).map_err(|_| CaptureFailure::LimitExceeded)?;
            let width = u16::try_from(rect.width).map_err(|_| CaptureFailure::LimitExceeded)?;
            let height = u16::try_from(rect.height).map_err(|_| CaptureFailure::LimitExceeded)?;

            let setup = self.conn.setup();
            let screen = setup
                .roots
                .get(self.screen_num)
                .ok_or(CaptureFailure::ConnectionLost(Reason::Disconnected))?;
            let root = screen.root;
            let root_depth = screen.root_depth;
            let bpp = setup
                .pixmap_formats
                .iter()
                .find(|pf| pf.depth == root_depth)
                .map_or(32, |pf| pf.bits_per_pixel);
            let scanline_pad = setup
                .pixmap_formats
                .iter()
                .find(|pf| pf.depth == root_depth)
                .map_or(32, |pf| pf.scanline_pad);
            let px_fmt = x11_pixel_format(X11PixmapFormat {
                depth: root_depth,
                bits_per_pixel: bpp,
                scanline_pad,
                big_endian: setup.image_byte_order == ImageOrder::MSB_FIRST,
            })?;
            let reply = self
                .conn
                .get_image(ImageFormat::Z_PIXMAP, root, left, top, width, height, !0)
                .map_err(|_| CaptureFailure::ConnectionLost(Reason::Disconnected))?
                .reply()
                .map_err(|err| match err {
                    x11rb::errors::ReplyError::X11Error(_) => {
                        CaptureFailure::Unavailable(Reason::GeometryChanged)
                    }
                    x11rb::errors::ReplyError::ConnectionError(_) => {
                        CaptureFailure::ConnectionLost(Reason::Disconnected)
                    }
                })?;

            let stride = compute_x11_stride(reply.data.len(), rect.width, rect.height, px_fmt)?;
            out_raw.clear();
            out_raw.extend_from_slice(&reply.data);
            Ok((px_fmt, stride))
        }
    }

    struct RealX11Input {
        conn: RustConnection,
        root: u32,
        conn_lost: std::sync::atomic::AtomicBool,
    }

    impl RealX11Input {
        fn open(dpy: &str) -> Result<Self, ViewError> {
            let (conn, screen_num) =
                x11rb::connect(Some(dpy)).map_err(|_| ViewError::Unavailable)?;
            let root = conn.setup().roots[screen_num].root;
            Ok(Self {
                conn,
                root,
                conn_lost: std::sync::atomic::AtomicBool::new(false),
            })
        }

        fn map_conn_err<T>(
            &self,
            res: Result<T, x11rb::errors::ConnectionError>,
        ) -> Result<T, ViewError> {
            res.map_err(|err| {
                // Only actual socket / I/O errors mean the X11 connection died; request-encoding
                // or protocol limits (`ParseError`, `MaximumRequestLengthExceeded`, etc.) must not
                // poison a live socket as `conn_lost`.
                if matches!(err, x11rb::errors::ConnectionError::IoError(_)) {
                    self.conn_lost
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                ViewError::Unavailable
            })
        }

        fn map_reply_err<T>(
            &self,
            res: Result<T, x11rb::errors::ReplyError>,
        ) -> Result<T, ViewError> {
            res.map_err(|err| {
                if matches!(
                    err,
                    x11rb::errors::ReplyError::ConnectionError(
                        x11rb::errors::ConnectionError::IoError(_)
                    )
                ) {
                    self.conn_lost
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                ViewError::Unavailable
            })
        }
    }

    impl X11InputConnection for RealX11Input {
        fn has_xtest(&mut self) -> Result<bool, ViewError> {
            match self.conn.xtest_get_version(2, 1) {
                Ok(cookie) => Ok(cookie.reply().is_ok()),
                Err(_) => Ok(false),
            }
        }

        fn keymap(&mut self) -> Result<X11KeymapSnapshot, ViewError> {
            let setup = self.conn.setup();
            let min_kc = setup.min_keycode;
            let max_kc = setup.max_keycode;
            let count = max_kc.saturating_sub(min_kc).saturating_add(1);
            let cookie = self.map_conn_err(self.conn.get_keyboard_mapping(min_kc, count))?;
            let rep = self.map_reply_err(cookie.reply())?;
            Ok(X11KeymapSnapshot {
                min_keycode: min_kc,
                max_keycode: max_kc,
                keysyms_per_keycode: rep.keysyms_per_keycode,
                keysyms: rep.keysyms,
            })
        }

        fn bind_scratch_keycode(&mut self, keycode: u8, keysym: u32) -> Result<(), ViewError> {
            let _ = self.map_conn_err(self.conn.change_keyboard_mapping(
                1,
                keycode,
                2,
                &[keysym, keysym],
            ))?;
            self.map_reply_err(self.conn.sync())
        }

        fn unbind_scratch_keycode(&mut self, keycode: u8) -> Result<(), ViewError> {
            let _ = self.map_conn_err(self.conn.change_keyboard_mapping(1, keycode, 2, &[0, 0]))?;
            self.map_reply_err(self.conn.sync())
        }

        fn motion(&mut self, x: i16, y: i16) -> Result<(), ViewError> {
            let _ = self.map_conn_err(self.conn.xtest_fake_input(
                MOTION_NOTIFY_EVENT,
                0,
                CURRENT_TIME,
                NONE,
                x,
                y,
                0,
            ))?;
            self.map_reply_err(self.conn.sync())
        }

        fn button(&mut self, button: u8, press: bool) -> Result<(), ViewError> {
            let ev = if press {
                BUTTON_PRESS_EVENT
            } else {
                BUTTON_RELEASE_EVENT
            };
            let _ = self.map_conn_err(self.conn.xtest_fake_input(
                ev,
                button,
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            ))?;
            self.map_reply_err(self.conn.sync())
        }

        fn key(&mut self, keycode: u8, press: bool) -> Result<(), ViewError> {
            let ev = if press {
                KEY_PRESS_EVENT
            } else {
                KEY_RELEASE_EVENT
            };
            let _ = self.map_conn_err(self.conn.xtest_fake_input(
                ev,
                keycode,
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            ))?;
            self.map_reply_err(self.conn.sync())
        }

        fn root_rect(&mut self) -> Result<Option<PixelRect>, ViewError> {
            let cookie = self.map_conn_err(self.conn.get_geometry(self.root))?;
            let geom = self.map_reply_err(cookie.reply())?;
            Ok(Some(PixelRect {
                x: 0,
                y: 0,
                width: u32::from(geom.width).max(1),
                height: u32::from(geom.height).max(1),
            }))
        }

        fn query_server_pressed_state(
            &mut self,
        ) -> Result<Option<X11ServerPressedState>, ViewError> {
            let keymap_cookie = self.map_conn_err(self.conn.query_keymap())?;
            let pointer_cookie = self.map_conn_err(self.conn.query_pointer(self.root))?;
            let keymap = self.map_reply_err(keymap_cookie.reply())?;
            let pointer = self.map_reply_err(pointer_cookie.reply())?;
            let mask_bits = u16::from(pointer.mask);
            let mut buttons_1_to_5 = 0_u8;
            for (idx, bit) in [
                KeyButMask::BUTTON1,
                KeyButMask::BUTTON2,
                KeyButMask::BUTTON3,
                KeyButMask::BUTTON4,
                KeyButMask::BUTTON5,
            ]
            .into_iter()
            .enumerate()
            {
                if (mask_bits & u16::from(bit)) != 0 {
                    buttons_1_to_5 |= 1_u8 << idx;
                }
            }
            Ok(Some(X11ServerPressedState {
                keys: keymap.keys,
                buttons_1_to_5,
            }))
        }

        fn is_connection_lost(&self) -> bool {
            self.conn_lost.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    /// Create an X11 live-view target on Linux, checking session type, `DISPLAY`,
    /// `XGetImage`, `RandR`, and `XTest` availability.
    ///
    /// # Errors
    ///
    /// Returns [`Reason::Unsupported`] on Wayland or [`Reason::Unavailable`] when
    /// no X11 server is reachable.
    pub fn open_x11_live_view_target(
        options: &LiveViewOptions,
    ) -> Result<Arc<DesktopViewTarget>, Reason> {
        let xdg = std::env::var("XDG_SESSION_TYPE").ok();
        let wayland = std::env::var("WAYLAND_DISPLAY").ok();
        let x11 = std::env::var("DISPLAY").ok();
        let dpy = validate_x11_env(xdg.as_deref(), wayland.as_deref(), x11.as_deref())?;
        open_x11_live_view_target_on(options, dpy, X11_START_IO_TIMEOUT)
    }

    /// [`open_x11_live_view_target`] for an explicit display. Every X11 round
    /// trip made here (handshake, `XTest` query, layout) shares one `io_timeout`
    /// deadline, so a stalled server fails the call instead of hanging it.
    pub(super) fn open_x11_live_view_target_on(
        options: &LiveViewOptions,
        dpy: String,
        io_timeout: Duration,
    ) -> Result<Arc<DesktopViewTarget>, Reason> {
        type RealCaptureFactory =
            Arc<dyn Fn() -> Result<RealX11Capture, CaptureFailure> + Send + Sync>;
        type RealInputFactory = Arc<dyn Fn() -> Result<RealX11Input, ViewError> + Send + Sync>;

        let init_cap = RealX11Capture::open_bounded(dpy.clone(), io_timeout)
            .map_err(|_| Reason::Unavailable)?;
        let xtest_ok = init_cap.xtest_available();

        let dpy_cap = dpy.clone();
        let dpy_probe = dpy.clone();
        let dpy_in = dpy;

        let capture_factory: RealCaptureFactory =
            Arc::new(move || RealX11Capture::open(dpy_cap.clone()));
        let probe: AvailabilityProbeFn = Arc::new(move |scope| {
            let xdg = std::env::var("XDG_SESSION_TYPE").ok();
            let wayland = std::env::var("WAYLAND_DISPLAY").ok();
            let x11 = std::env::var("DISPLAY").ok();
            let _ = validate_x11_env(xdg.as_deref(), wayland.as_deref(), x11.as_deref())?;
            let mut c = RealX11Capture::open_bounded(dpy_probe.clone(), X11_PROBE_IO_TIMEOUT)
                .map_err(|_| Reason::Unavailable)?;
            let layout = c.query_layout().map_err(|_| Reason::Unavailable)?;
            let _ = select_capture_rect(
                scope,
                Some(layout.root_rect),
                &layout.monitors,
                layout.display_token,
            )?;
            Ok(())
        });

        let (input_factory, input_reason): (Option<RealInputFactory>, Option<Reason>) = if xtest_ok
        {
            (Some(Arc::new(move || RealX11Input::open(&dpy_in))), None)
        } else {
            (None, Some(Reason::Unsupported))
        };

        build_x11_view_target(
            options,
            init_cap,
            capture_factory,
            probe,
            input_factory,
            input_reason,
        )
    }
}

#[cfg(target_os = "linux")]
pub use linux_x11::open_x11_live_view_target;

// ── Hermetic cross-platform tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use live_view::{Codec, EffectiveProfile, ViewTarget};
    use parking_lot::Mutex;
    use std::sync::atomic::AtomicU64;

    #[derive(Clone)]
    struct MockX11CapState {
        layout: X11DisplayLayout,
        raw: Vec<u8>,
        stride: usize,
        format: PixelFormat,
        get_image_calls: Arc<AtomicU64>,
        drop_conn_once: Arc<AtomicBool>,
    }

    struct MockX11Capture {
        state: Arc<Mutex<MockX11CapState>>,
    }

    impl X11CaptureConnection for MockX11Capture {
        fn query_layout(&mut self) -> Result<X11DisplayLayout, CaptureFailure> {
            Ok(self.state.lock().layout.clone())
        }

        fn get_image(
            &mut self,
            rect: PixelRect,
            out_raw: &mut Vec<u8>,
        ) -> Result<(PixelFormat, usize), CaptureFailure> {
            let st = self.state.lock();
            if st.drop_conn_once.swap(false, Ordering::SeqCst) {
                return Err(CaptureFailure::ConnectionLost(Reason::Disconnected));
            }
            st.get_image_calls.fetch_add(1, Ordering::SeqCst);
            assert!(rect.width * rect.height <= 16_000_000);
            out_raw.clear();
            out_raw.extend_from_slice(&st.raw);
            Ok((st.format, st.stride))
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum X11Call {
        Motion(i16, i16),
        Button(u8, bool),
        Key(u8, bool),
        Bind(u8, u32),
        Unbind(u8),
    }

    struct MockX11InputState {
        has_xtest: bool,
        keymap: X11KeymapSnapshot,
        keymap_calls: usize,
        next_conn_id: u64,
        calls_with_conn: Vec<(u64, X11Call)>,
        server_keys: BTreeSet<u8>,
        server_buttons: BTreeSet<u8>,
        calls: Vec<X11Call>,
        fail_after_n: Option<usize>,
        conn_lost: bool,
        root_rect: Option<PixelRect>,
    }

    struct MockX11Input {
        state: Arc<Mutex<MockX11InputState>>,
        conn_id: u64,
        local_conn_lost: bool,
    }

    impl MockX11Input {
        fn new(state: Arc<Mutex<MockX11InputState>>) -> Self {
            let conn_id = {
                let mut st = state.lock();
                st.next_conn_id += 1;
                st.next_conn_id
            };
            Self {
                state,
                conn_id,
                local_conn_lost: false,
            }
        }

        fn record(&mut self, call: X11Call) -> Result<(), ViewError> {
            let mut st = self.state.lock();
            if self.local_conn_lost || st.conn_lost {
                self.local_conn_lost = true;
                return Err(ViewError::Unavailable);
            }
            if let Some(rem) = st.fail_after_n.as_mut() {
                if *rem == 0 {
                    return Err(ViewError::Unavailable);
                }
                *rem -= 1;
            }
            match &call {
                X11Call::Key(kc, true) => {
                    st.server_keys.insert(*kc);
                }
                X11Call::Key(kc, false) => {
                    st.server_keys.remove(kc);
                }
                X11Call::Button(btn, true) => {
                    st.server_buttons.insert(*btn);
                }
                X11Call::Button(btn, false) => {
                    st.server_buttons.remove(btn);
                }
                _ => {}
            }
            st.calls_with_conn.push((self.conn_id, call.clone()));
            st.calls.push(call);
            Ok(())
        }
    }

    impl X11InputConnection for MockX11Input {
        fn has_xtest(&mut self) -> Result<bool, ViewError> {
            let st = self.state.lock();
            if self.local_conn_lost || st.conn_lost {
                self.local_conn_lost = true;
                return Err(ViewError::Unavailable);
            }
            Ok(st.has_xtest)
        }

        fn keymap(&mut self) -> Result<X11KeymapSnapshot, ViewError> {
            let mut st = self.state.lock();
            if self.local_conn_lost || st.conn_lost {
                self.local_conn_lost = true;
                return Err(ViewError::Unavailable);
            }
            st.keymap_calls += 1;
            Ok(st.keymap.clone())
        }

        fn bind_scratch_keycode(&mut self, keycode: u8, keysym: u32) -> Result<(), ViewError> {
            self.record(X11Call::Bind(keycode, keysym))?;
            let mut st = self.state.lock();
            let offset = usize::from(keycode - st.keymap.min_keycode);
            let per = usize::from(st.keymap.keysyms_per_keycode);
            st.keymap.keysyms[offset * per] = keysym;
            st.keymap.keysyms[offset * per + 1] = keysym;
            Ok(())
        }

        fn unbind_scratch_keycode(&mut self, keycode: u8) -> Result<(), ViewError> {
            self.record(X11Call::Unbind(keycode))?;
            let mut st = self.state.lock();
            let offset = usize::from(keycode - st.keymap.min_keycode);
            let per = usize::from(st.keymap.keysyms_per_keycode);
            st.keymap.keysyms[offset * per] = 0;
            st.keymap.keysyms[offset * per + 1] = 0;
            Ok(())
        }

        fn motion(&mut self, x: i16, y: i16) -> Result<(), ViewError> {
            self.record(X11Call::Motion(x, y))
        }

        fn button(&mut self, button: u8, press: bool) -> Result<(), ViewError> {
            self.record(X11Call::Button(button, press))
        }

        fn key(&mut self, keycode: u8, press: bool) -> Result<(), ViewError> {
            self.record(X11Call::Key(keycode, press))
        }

        fn root_rect(&mut self) -> Result<Option<PixelRect>, ViewError> {
            let st = self.state.lock();
            if self.local_conn_lost || st.conn_lost {
                self.local_conn_lost = true;
                return Err(ViewError::Unavailable);
            }
            Ok(st.root_rect)
        }

        fn query_server_pressed_state(
            &mut self,
        ) -> Result<Option<X11ServerPressedState>, ViewError> {
            let st = self.state.lock();
            if self.local_conn_lost || st.conn_lost {
                self.local_conn_lost = true;
                return Err(ViewError::Unavailable);
            }
            let mut keys = [0_u8; 32];
            for &kc in &st.server_keys {
                keys[usize::from(kc / 8)] |= 1_u8 << (kc % 8);
            }
            let mut buttons_1_to_5 = 0_u8;
            for &btn in &st.server_buttons {
                if (1..=5).contains(&btn) {
                    buttons_1_to_5 |= 1_u8 << (btn - 1);
                }
            }
            Ok(Some(X11ServerPressedState {
                keys,
                buttons_1_to_5,
            }))
        }

        fn is_connection_lost(&self) -> bool {
            self.local_conn_lost || self.state.lock().conn_lost
        }
    }

    fn base_keymap(full: bool) -> X11KeymapSnapshot {
        // Keycodes 9..=13 (2 keysyms per keycode):
        // kc 9: a / A, kc 10: Shift_L, kc 11: Control_L, kc 12: free (unless full), kc 13: Return
        let mut keysyms = vec![
            u32::from(b'a'),
            u32::from(b'A'),
            0xFFE1,
            0xFFE1,
            0xFFE3,
            0xFFE3,
            0,
            0,
            0xFF0D,
            0xFF0D,
        ];
        if full {
            keysyms[6] = u32::from(b'z');
            keysyms[7] = u32::from(b'Z');
        }
        X11KeymapSnapshot {
            min_keycode: 9,
            max_keycode: 13,
            keysyms_per_keycode: 2,
            keysyms,
        }
    }

    fn multi_scratch_keymap() -> X11KeymapSnapshot {
        // Keycodes 9..=16 (2 keysyms per keycode):
        // kc 9: a / A, kc 10: Shift_L, kc 11: Control_L, kc 12: Alt_L, kc 13: Alt_R,
        // kc 14..=16: 3 free scratch slots, kc 17: i / I
        let keysyms = vec![
            u32::from(b'a'),
            u32::from(b'A'),
            0xFFE1,
            0xFFE1,
            0xFFE3,
            0xFFE3,
            0xFFE9,
            0xFFE9,
            0xFFEA,
            0xFFEA,
            0,
            0,
            0,
            0,
            0,
            0,
            u32::from(b'i'),
            u32::from(b'I'),
        ];
        X11KeymapSnapshot {
            min_keycode: 9,
            max_keycode: 17,
            keysyms_per_keycode: 2,
            keysyms,
        }
    }

    #[test]
    fn env_detection_refuses_wayland_as_unsupported_and_missing_display_as_unavailable() {
        assert_eq!(
            validate_x11_env(Some("wayland"), Some("wayland-0"), Some(":0")),
            Err(Reason::Unsupported)
        );
        assert_eq!(
            validate_x11_env(Some("x11"), None, None),
            Err(Reason::Unavailable)
        );
        assert_eq!(
            validate_x11_env(Some("x11"), None, Some(":99")),
            Ok(":99".into())
        );
    }

    #[test]
    fn oversized_x11_virtual_desktop_fails_before_calling_get_image() {
        let state = Arc::new(Mutex::new(MockX11CapState {
            layout: X11DisplayLayout {
                root_rect: PixelRect {
                    x: 0,
                    y: 0,
                    width: 8000,
                    height: 3000,
                },
                monitors: vec![MonitorDescriptor {
                    name: "DP-1".into(),
                    is_primary: true,
                    rect: PixelRect {
                        x: 0,
                        y: 0,
                        width: 8000,
                        height: 3000,
                    },
                    dpi: 96,
                }],
                format: X11PixmapFormat {
                    depth: 24,
                    bits_per_pixel: 32,
                    scanline_pad: 32,
                    big_endian: false,
                },
                display_token: 1,
            },
            raw: Vec::new(),
            stride: 0,
            format: PixelFormat::Bgra32,
            get_image_calls: Arc::new(AtomicU64::new(0)),
            drop_conn_once: Arc::new(AtomicBool::new(false)),
        }));
        let mut drv = X11CaptureDriver::new(
            MockX11Capture {
                state: Arc::clone(&state),
            },
            CaptureScope::VirtualDesktop,
        );
        let mut buf = Vec::new();
        assert_eq!(
            drv.capture(&mut buf).err(),
            Some(CaptureFailure::LimitExceeded)
        );
        assert_eq!(state.lock().get_image_calls.load(Ordering::SeqCst), 0);
    }

    /// 244-V #5: an X server on a Unix socket that accepts the connection but
    /// never answers must fail the open within the I/O budget; it can never
    /// hang daemon start.
    ///
    /// This drives `connect_bounded_address` with the socket address directly:
    /// x11rb rewrites a `DISPLAY` that is a socket path to `/tmp/.X11-unix/X0`,
    /// so the client would never reach this fake server through
    /// `open_x11_live_view_target_on`, and a join on a thread blocked in
    /// `accept` would hang the test binary forever.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_server_that_accepts_but_never_replies_fails_the_open_within_the_budget() {
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use x11rb::reexports::x11rb_protocol::parse_display::ConnectAddress;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("X-stalled");
        let listener = UnixListener::bind(&sock).unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        // Detached on purpose (never joined): holds the accepted connection
        // open without writing a byte until the test is done.
        std::thread::spawn(move || {
            if let Ok((_stream, _)) = listener.accept() {
                let _ = accepted_tx.send(());
                let _ = done_rx.recv_timeout(Duration::from_secs(10));
            }
        });

        let started = Instant::now();
        let res = super::linux_x11::connect_bounded_address(
            &ConnectAddress::Socket(sock.to_str().unwrap().to_owned()),
            0,
            0,
            started + Duration::from_millis(300),
        );
        let took = started.elapsed();
        let _ = done_tx.send(());

        assert!(
            accepted_rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the client never reached the fake server"
        );
        assert_eq!(
            res.err(),
            Some(CaptureFailure::ConnectionLost(Reason::Disconnected))
        );
        assert!(took < Duration::from_secs(2), "open took {took:?}");
    }

    /// 244-V #8: building the target (what the real constructor does after its
    /// OS probe) starts no capture and no input work; only a viewer's subscribe
    /// creates a capture connection, and watching never creates an input one.
    #[tokio::test]
    async fn building_the_target_starts_no_capture_or_input_until_a_viewer_subscribes() {
        let cap_state = make_mock_x11_cap_state(64, 32, 64 * 4, vec![0x22; 64 * 32 * 4], 7);
        let capture_made = Arc::new(AtomicU64::new(0));
        let input_made = Arc::new(AtomicU64::new(0));
        let target = build_x11_view_target(
            &LiveViewOptions::new("counting"),
            MockX11Capture {
                state: Arc::clone(&cap_state),
            },
            {
                let (state, made) = (Arc::clone(&cap_state), Arc::clone(&capture_made));
                Arc::new(move || {
                    made.fetch_add(1, Ordering::SeqCst);
                    Ok(MockX11Capture {
                        state: Arc::clone(&state),
                    })
                })
            },
            Arc::new(|_| Ok(())),
            Some({
                let made = Arc::clone(&input_made);
                Arc::new(move || -> Result<MockX11Input, ViewError> {
                    made.fetch_add(1, Ordering::SeqCst);
                    Err(ViewError::Unavailable)
                })
            }),
            None,
        )
        .unwrap();
        let _ = target.info();
        let source = target.frame_source();
        let _ = target.input_sink();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            capture_made.load(Ordering::SeqCst),
            0,
            "capture before a viewer"
        );
        assert_eq!(
            input_made.load(Ordering::SeqCst),
            0,
            "input before a viewer"
        );
        assert_eq!(cap_state.lock().get_image_calls.load(Ordering::SeqCst), 0);
        let mut sub = source.subscribe(EffectiveProfile::watch()).await.unwrap();
        sub.frames.changed().await.unwrap();
        assert!(capture_made.load(Ordering::SeqCst) >= 1);
        assert_eq!(
            input_made.load(Ordering::SeqCst),
            0,
            "watching never drives input"
        );
        drop(sub);
        let _ = source.stop().await;
    }

    fn make_mock_x11_cap_state(
        width: u32,
        height: u32,
        stride: usize,
        raw: Vec<u8>,
        display_token: u64,
    ) -> Arc<Mutex<MockX11CapState>> {
        let rect = PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        Arc::new(Mutex::new(MockX11CapState {
            layout: X11DisplayLayout {
                root_rect: rect,
                monitors: vec![MonitorDescriptor {
                    name: "HDMI-1".into(),
                    is_primary: true,
                    rect,
                    dpi: 96,
                }],
                format: X11PixmapFormat {
                    depth: 24,
                    bits_per_pixel: 32,
                    scanline_pad: 32,
                    big_endian: false,
                },
                display_token,
            },
            raw,
            stride,
            format: PixelFormat::Bgra32,
            get_image_calls: Arc::new(AtomicU64::new(0)),
            drop_conn_once: Arc::new(AtomicBool::new(false)),
        }))
    }

    #[tokio::test]
    async fn x11_capture_and_randr_layout_changes_publish_jpeg_with_zero_temp_files() {
        let prod_x11_src = include_str!("live_view_x11.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("prod source");
        for forbidden in [
            "std::fs",
            "tokio::fs",
            "File::create",
            "OpenOptions",
            "tempfile",
            "temp_dir",
            "screenshot(",
        ] {
            assert!(
                !prod_x11_src.contains(forbidden),
                "live_view_x11 production code must not use {forbidden}"
            );
        }

        let stride = 80 * 4 + 16; // Deliberate odd scanline padding!
        let mut raw = vec![0xCD; stride * 60];
        for y in 0..60 {
            for x in 0..80 {
                let off = y * stride + x * 4;
                raw[off] = u8::try_from(x & 0xFF).unwrap_or(0);
                raw[off + 1] = u8::try_from(y & 0xFF).unwrap_or(0);
                raw[off + 2] = 0x55;
                raw[off + 3] = 0x00;
            }
        }
        let cap_state = make_mock_x11_cap_state(80, 60, stride, raw, 10);

        let target = build_x11_view_target::<_, MockX11Input, _>(
            &LiveViewOptions::new("linux-x11-host"),
            MockX11Capture {
                state: Arc::clone(&cap_state),
            },
            Arc::new({
                let s = Arc::clone(&cap_state);
                move || {
                    Ok(MockX11Capture {
                        state: Arc::clone(&s),
                    })
                }
            }),
            Arc::new(|_| Ok(())),
            None,
            Some(Reason::Unsupported),
        )
        .unwrap();

        let info = target.info();
        assert!(info.capabilities.view && !info.capabilities.input);
        assert_eq!(info.capabilities.codecs, vec![Codec::Jpeg, Codec::Webp]);
        assert!(target.input_sink().is_none());

        let src = target.frame_source();
        let mut sub = src.subscribe(EffectiveProfile::watch()).await.unwrap();
        let f1 = sub.frames.borrow_and_update().clone().unwrap();
        assert_eq!(f1.metadata.geometry_revision, 1);
        assert_eq!(&f1.payload[..2], &[0xFF, 0xD8]);
        assert_eq!(f1.metadata.source_rect.units, live_view::Units::PhysicalPx);

        // Simulate RandR resolution switch from 80x60 -> 100x80
        {
            let mut st = cap_state.lock();
            st.layout.root_rect.width = 100;
            st.layout.root_rect.height = 80;
            st.layout.monitors[0].rect.width = 100;
            st.layout.monitors[0].rect.height = 80;
            st.layout.display_token = 11;
            st.stride = 100 * 4;
            st.raw = vec![0x77; 100 * 80 * 4];
        }
        src.request_refresh(42).await.unwrap();
        let f2 = loop {
            if let Some(f) = sub.frames.borrow_and_update().clone()
                && f.metadata.action_seq == Some(42)
            {
                break f;
            }
            sub.frames.changed().await.unwrap();
        };
        assert_eq!(f2.metadata.geometry_revision, 2);
        assert_eq!(f2.metadata.source_rect.width, 100);
        assert_eq!(f2.metadata.action_seq, Some(42));

        src.stop().await.unwrap();
        drop(sub);
    }

    #[tokio::test]
    async fn xtest_driver_tracks_pressed_inputs_rejects_unmappable_keys_and_handles_unicode() {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let sr = SourceRect {
            x: 100.0,
            y: 50.0,
            width: 1000,
            height: 500,
            units: live_view::Units::PhysicalPx,
        };
        let vr = PixelRect {
            x: 100,
            y: 50,
            width: 1000,
            height: 500,
        };
        let cancel = AtomicBool::new(false);

        // Pointer down at normalized (0.5, 0.5) -> maps cleanly to physical pixels (600, 300)
        drv.inject(
            &InputEvent::Pointer {
                phase: PointerPhase::Down,
                x: 0.5,
                y: 0.5,
                button: Some(Button::Middle),
                buttons: 2,
                modifiers: vec![],
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        // Hold ShiftLeft + KeyA ('A' hits col 1 of permanent kc 9, NO scratch binding!)
        for (code, key) in [("ShiftLeft", "Shift"), ("KeyA", "A")] {
            drv.inject(
                &InputEvent::Key {
                    phase: KeyPhase::Down,
                    code: code.into(),
                    key: key.into(),
                    location: 0,
                    repeat: false,
                    modifiers: vec![Modifier::Shift],
                },
                sr.clone(),
                vr,
                &cancel,
            )
            .unwrap();
        }

        // Turkish Unicode text binds scratch kc 12 and keeps it bound until `release_all()`.
        drv.inject(
            &InputEvent::Text {
                text: "ğİ".into()
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        // Dead / Unidentified key explicitly rejected without guessing.
        assert_eq!(
            drv.inject(
                &InputEvent::Key {
                    phase: KeyPhase::Down,
                    code: "KeyE".into(),
                    key: "Dead".into(),
                    location: 0,
                    repeat: false,
                    modifiers: vec![],
                },
                sr,
                vr,
                &cancel,
            ),
            Err(ViewError::Unsupported)
        );

        // release_all releases held button 2, kc 9 (KeyA), kc 10 (ShiftLeft) and unbinds scratch kc 12.
        in_state.lock().calls.clear();
        drv.release_all().unwrap();
        let calls = in_state.lock().calls.clone();
        assert!(calls.contains(&X11Call::Key(9, false)));
        assert!(calls.contains(&X11Call::Key(10, false)));
        assert!(calls.contains(&X11Call::Button(2, false)));
        assert!(calls.contains(&X11Call::Unbind(12)));
        assert_eq!(calls.len(), 4);

        // Repeating release_all is an idempotent no-op.
        in_state.lock().calls.clear();
        drv.release_all().unwrap();
        assert_eq!(in_state.lock().calls.len(), 0);
    }

    #[test]
    fn xtest_full_keymap_rejects_unmapped_unicode() {
        let mut drv_full =
            X11InputDriver::new(MockX11Input::new(Arc::new(Mutex::new(MockX11InputState {
                has_xtest: true,
                keymap: base_keymap(true),
                keymap_calls: 0,
                next_conn_id: 0,
                calls_with_conn: Vec::new(),
                server_keys: BTreeSet::new(),
                server_buttons: BTreeSet::new(),
                calls: Vec::new(),
                fail_after_n: None,
                conn_lost: false,
                root_rect: None,
            }))))
            .unwrap();
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 100,
            height: 100,
            units: live_view::Units::PhysicalPx,
        };
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(
            drv_full.inject(
                &InputEvent::Text { text: "ş".into() },
                sr,
                vr,
                &AtomicBool::new(false)
            ),
            Err(ViewError::Unsupported)
        );
    }

    #[test]
    fn xtest_mid_text_failure_returns_partial_injection() {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: Some(2), // first char down+up succeeds, second char down fails
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 100,
            height: 100,
            units: live_view::Units::PhysicalPx,
        };
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(
            drv.inject(
                &InputEvent::Text { text: "aa".into() },
                sr,
                vr,
                &AtomicBool::new(false)
            ),
            Err(ViewError::PartialInjection)
        );
    }

    #[test]
    fn xtest_failed_key_up_and_button_up_stay_held_until_release_all_succeeds() {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 200,
            height: 100,
            units: live_view::Units::PhysicalPx,
        };
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let cancel = AtomicBool::new(false);

        // Press Left mouse button (btn 1) and scratch-bound 'ğ' key (binds scratch kc 12).
        for ev in [
            InputEvent::Pointer {
                phase: PointerPhase::Down,
                x: 0.25,
                y: 0.25,
                button: Some(Button::Left),
                buttons: 1,
                modifiers: vec![],
            },
            InputEvent::Key {
                phase: KeyPhase::Down,
                code: "BracketRight".into(),
                key: "ğ".into(),
                location: 0,
                repeat: false,
                modifiers: vec![],
            },
        ] {
            drv.inject(&ev, sr.clone(), vr, &cancel).unwrap();
        }

        // Simulate button-up failing after motion succeeds (`fail_after_n = Some(1)`).
        in_state.lock().fail_after_n = Some(1);
        assert_eq!(
            drv.inject(
                &InputEvent::Pointer {
                    phase: PointerPhase::Up,
                    x: 0.25,
                    y: 0.25,
                    button: Some(Button::Left),
                    buttons: 0,
                    modifiers: vec![],
                },
                sr.clone(),
                vr,
                &cancel,
            ),
            Err(ViewError::PartialInjection)
        );

        // Simulate key-up failing immediately (`fail_after_n = Some(0)`).
        // Must NOT forget the held key AND must NOT unbind scratch kc 12 while it is still held!
        in_state.lock().calls.clear();
        in_state.lock().fail_after_n = Some(0);
        assert_eq!(
            drv.inject(
                &InputEvent::Key {
                    phase: KeyPhase::Up,
                    code: "BracketRight".into(),
                    key: "ğ".into(),
                    location: 0,
                    repeat: false,
                    modifiers: vec![],
                },
                sr,
                vr,
                &cancel,
            ),
            Err(ViewError::PartialInjection)
        );
        assert!(!in_state.lock().calls.contains(&X11Call::Unbind(12)));

        // First `release_all()` attempt: allow key(12, false) to succeed (1 call) then fail button(1, false).
        in_state.lock().calls.clear();
        in_state.lock().fail_after_n = Some(1);
        assert_eq!(drv.release_all(), Err(ViewError::Unavailable));
        assert_eq!(in_state.lock().calls, vec![X11Call::Key(12, false)]);

        // Second `release_all()` attempt with healthy connection: must release still-held button 1
        // and unbind scratch kc 12.
        in_state.lock().calls.clear();
        in_state.lock().fail_after_n = None;
        drv.release_all().expect("retry release_all succeeds");
        let retry_calls = in_state.lock().calls.clone();
        assert!(retry_calls.contains(&X11Call::Button(1, false)));
        assert!(retry_calls.contains(&X11Call::Unbind(12)));

        // Third `release_all()` is an idempotent no-op.
        in_state.lock().calls.clear();
        drv.release_all().unwrap();
        assert_eq!(in_state.lock().calls.len(), 0);
    }

    fn x11_test_key_event(
        phase: KeyPhase,
        code: &str,
        key: &str,
        location: u8,
        modifiers: Vec<Modifier>,
    ) -> InputEvent {
        InputEvent::Key {
            phase,
            code: code.into(),
            key: key.into(),
            location,
            repeat: false,
            modifiers,
        }
    }

    #[tokio::test]
    async fn xtest_connection_loss_while_key_held_recovers_on_release_all_and_rebuilds_driver() {
        let cap_state = make_mock_x11_cap_state(200, 100, 200 * 4, vec![0x44; 200 * 100 * 4], 1);
        let active_in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let conn_creations = Arc::new(AtomicU64::new(0));

        let target = build_x11_view_target(
            &LiveViewOptions::new("x11-reconnect-host"),
            MockX11Capture {
                state: Arc::clone(&cap_state),
            },
            Arc::new({
                let s = Arc::clone(&cap_state);
                move || {
                    Ok(MockX11Capture {
                        state: Arc::clone(&s),
                    })
                }
            }),
            Arc::new(|_| Ok(())),
            Some(Arc::new({
                let s = Arc::clone(&active_in_state);
                let c = Arc::clone(&conn_creations);
                move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    let mut guard = s.lock();
                    guard.conn_lost = false;
                    guard.keymap = base_keymap(false);
                    drop(guard);
                    Ok(MockX11Input::new(Arc::clone(&s)))
                }
            })),
            None,
        )
        .unwrap();

        let sink = target.input_sink().unwrap();
        sink.set_takeover_active(true).await.unwrap();
        assert_eq!(conn_creations.load(Ordering::SeqCst), 1);

        // Hold down ShiftLeft (kc 10), a scratch-bound key ('ğ', kc 12), and Left mouse button (1)
        // on connection 1.
        sink.inject(&x11_test_key_event(
            KeyPhase::Down,
            "ShiftLeft",
            "Shift",
            1,
            vec![Modifier::Shift],
        ))
        .await
        .unwrap();
        sink.inject(&x11_test_key_event(
            KeyPhase::Down,
            "BracketRight",
            "ğ",
            0,
            vec![],
        ))
        .await
        .unwrap();
        sink.inject_at_revision(
            &InputEvent::Pointer {
                phase: PointerPhase::Down,
                x: 0.5,
                y: 0.5,
                button: Some(Button::Left),
                buttons: 1,
                modifiers: vec![],
            },
            1,
            1,
        )
        .await
        .unwrap();

        assert!(active_in_state.lock().server_keys.contains(&10));
        assert!(active_in_state.lock().server_keys.contains(&12));
        assert!(active_in_state.lock().server_buttons.contains(&1));

        // Drop the X11 client socket while the X server is still alive and holding those XTest
        // keys/buttons.
        active_in_state.lock().conn_lost = true;

        // `release_pressed_inputs` must reconnect WITHOUT discarding held debt, query the live
        // server, send Shift/key/button releases and scratch unbind on the NEW connection (conn_id 2),
        // and only then return `Ok(())`.
        assert_eq!(sink.release_pressed_inputs().await, Ok(()));
        assert_eq!(conn_creations.load(Ordering::SeqCst), 2);
        {
            let st = active_in_state.lock();
            assert!(st.calls_with_conn.contains(&(2, X11Call::Key(10, false))));
            assert!(st.calls_with_conn.contains(&(2, X11Call::Key(12, false))));
            assert!(st.calls_with_conn.contains(&(2, X11Call::Button(1, false))));
            assert!(st.calls_with_conn.contains(&(2, X11Call::Unbind(12))));
            assert!(st.server_keys.is_empty());
            assert!(st.server_buttons.is_empty());
        }

        // Subsequent input injection on the recovered session works cleanly.
        active_in_state.lock().calls.clear();
        sink.inject(&x11_test_key_event(KeyPhase::Down, "KeyA", "a", 0, vec![]))
            .await
            .unwrap();
        assert_eq!(active_in_state.lock().calls, vec![X11Call::Key(9, true)]);
        sink.set_takeover_active(false).await.unwrap();
    }

    #[tokio::test]
    async fn xtest_server_gone_clears_held_debt_and_returns_ok() {
        let cap_state = make_mock_x11_cap_state(200, 100, 200 * 4, vec![0x44; 200 * 100 * 4], 1);
        let active_in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let server_gone = Arc::new(AtomicBool::new(false));

        let target = build_x11_view_target(
            &LiveViewOptions::new("x11-server-gone"),
            MockX11Capture {
                state: Arc::clone(&cap_state),
            },
            Arc::new({
                let s = Arc::clone(&cap_state);
                move || {
                    Ok(MockX11Capture {
                        state: Arc::clone(&s),
                    })
                }
            }),
            Arc::new(|_| Ok(())),
            Some(Arc::new({
                let s = Arc::clone(&active_in_state);
                let gone = Arc::clone(&server_gone);
                move || {
                    if gone.load(Ordering::SeqCst) {
                        return Err(ViewError::Unavailable);
                    }
                    Ok(MockX11Input::new(Arc::clone(&s)))
                }
            })),
            None,
        )
        .unwrap();

        let sink = target.input_sink().unwrap();
        sink.set_takeover_active(true).await.unwrap();
        sink.inject(&x11_test_key_event(
            KeyPhase::Down,
            "ShiftLeft",
            "Shift",
            1,
            vec![Modifier::Shift],
        ))
        .await
        .unwrap();

        // Now both the socket dies AND the X server itself disappears (reconnect fails).
        server_gone.store(true, Ordering::SeqCst);
        active_in_state.lock().conn_lost = true;
        assert_eq!(sink.release_pressed_inputs().await, Ok(()));
    }

    #[test]
    fn xtest_transient_error_without_connection_loss_keeps_held_until_replacement_connection_releases()
     {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::with_reconnect(
            MockX11Input::new(Arc::clone(&in_state)),
            Arc::new({
                let s = Arc::clone(&in_state);
                move || {
                    let mut guard = s.lock();
                    guard.conn_lost = false;
                    guard.fail_after_n = None;
                    drop(guard);
                    Ok(MockX11Input::new(Arc::clone(&s)))
                }
            }),
        )
        .unwrap();

        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let sr = vr.to_source_rect();
        let cancel = AtomicBool::new(false);

        // Hold ShiftLeft (kc 10) and Left mouse button (1).
        drv.inject(
            &x11_test_key_event(
                KeyPhase::Down,
                "ShiftLeft",
                "Shift",
                1,
                vec![Modifier::Shift],
            ),
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        drv.inject(
            &InputEvent::Pointer {
                phase: PointerPhase::Down,
                x: 0.5,
                y: 0.5,
                button: Some(Button::Left),
                buttons: 1,
                modifiers: vec![],
            },
            sr,
            vr,
            &cancel,
        )
        .unwrap();

        // Fail `key(10, false)` once WITHOUT losing the connection (`conn_lost == false`):
        // `release_all` must return `Err(ViewError::Unavailable)` and retain held keys/buttons.
        in_state.lock().fail_after_n = Some(0);
        assert_eq!(drv.release_all(), Err(ViewError::Unavailable));
        assert!(!drv.is_connection_lost());
        assert!(!drv.pressed_keys.is_empty());
        assert!(!drv.pressed_buttons.is_empty());

        // Clear transient failure and force reconnect (e.g. replacement connection on next reset):
        // the preserved held debt must still be sent over the replacement connection.
        in_state.lock().fail_after_n = None;
        in_state.lock().conn_lost = true;
        assert_eq!(drv.release_all(), Ok(()));
        let st = in_state.lock();
        assert!(st.calls_with_conn.contains(&(2, X11Call::Key(10, false))));
        assert!(st.calls_with_conn.contains(&(2, X11Call::Button(1, false))));
        assert!(st.server_keys.is_empty());
        assert!(st.server_buttons.is_empty());
    }

    /// Verifies layout-independent printable characters (Turkish letters plus `AltGr` `€`/`₺`)
    /// use exact X11 keysyms via scratch keycodes while shortcuts retain physical keycodes.
    /// Limitation: apps reading raw scancodes (games, some terminals) will not observe
    /// scratch-bound Unicode keysyms as hardware scancodes.
    #[test]
    fn xtest_turkish_layout_and_altgr_characters_inject_exact_keysyms_on_us_host() {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: multi_scratch_keymap(),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let sr = vr.to_source_rect();
        let cancel = AtomicBool::new(false);

        // 1. Turkish lowercase and Shift+KeyI ('İ') + Shift+Digit7 ('/') via `InputEvent::Key`.
        for (code, key, mods, expected_sym) in [
            ("Semicolon", "ş", vec![], 0x0100_015f),
            ("BracketRight", "ğ", vec![], 0x0100_011f),
            ("BracketLeft", "ü", vec![], 0x00fc),
            ("KeyI", "ı", vec![], 0x0100_0131),
            ("Comma", "ö", vec![], 0x00f6),
            ("Period", "ç", vec![], 0x00e7),
            ("KeyI", "İ", vec![Modifier::Shift], 0x0100_0130),
            ("Digit7", "/", vec![Modifier::Shift], u32::from(b'/')),
        ] {
            in_state.lock().calls.clear();
            for phase in [KeyPhase::Down, KeyPhase::Up] {
                drv.inject(
                    &x11_test_key_event(phase, code, key, 0, mods.clone()),
                    sr.clone(),
                    vr,
                    &cancel,
                )
                .unwrap();
            }
            let calls = in_state.lock().calls.clone();
            assert!(
                calls
                    .iter()
                    .any(|c| matches!(c, X11Call::Bind(_, sym) if *sym == expected_sym)),
                "expected Bind for {key} (0x{expected_sym:x}), got {calls:?}"
            );
        }

        // 2. Browser `AltGraph` latch + `Ctrl+Alt` (`AltGr`) sequence for `@`, `€`, and `₺`.
        in_state.lock().calls.clear();
        let altgr = vec![Modifier::Ctrl, Modifier::Alt];
        for ev in [
            x11_test_key_event(
                KeyPhase::Down,
                "ControlLeft",
                "Control",
                1,
                vec![Modifier::Ctrl],
            ),
            x11_test_key_event(KeyPhase::Down, "AltRight", "AltGraph", 2, altgr.clone()),
            x11_test_key_event(KeyPhase::Down, "KeyQ", "@", 0, altgr.clone()),
            x11_test_key_event(KeyPhase::Up, "KeyQ", "@", 0, altgr.clone()),
            x11_test_key_event(KeyPhase::Down, "KeyE", "€", 0, altgr.clone()),
            x11_test_key_event(KeyPhase::Up, "KeyE", "€", 0, altgr.clone()),
            x11_test_key_event(KeyPhase::Down, "KeyT", "₺", 0, altgr.clone()),
            x11_test_key_event(KeyPhase::Up, "KeyT", "₺", 0, altgr),
        ] {
            drv.inject(&ev, sr.clone(), vr, &cancel).unwrap();
        }
        let altgr_calls = in_state.lock().calls.clone();
        // `ControlLeft` (kc 11) must be released BEFORE `@` is pressed, and `AltGraph` must not press Alt_R!
        assert!(altgr_calls.contains(&X11Call::Key(11, false)));
        assert!(!altgr_calls.contains(&X11Call::Key(13, true)));
        for expected_sym in [u32::from(b'@'), char_to_keysym('€'), char_to_keysym('₺')] {
            assert!(
                altgr_calls
                    .iter()
                    .any(|c| matches!(c, X11Call::Bind(_, sym) if *sym == expected_sym)),
                "expected Bind for 0x{expected_sym:x}, got {altgr_calls:?}"
            );
        }

        // 3. Real shortcut `Ctrl + KeyI` on a Turkish layout (`key: "ı", modifiers: [Ctrl]`)
        // must fall back to physical `KeyI` (`'i'`, kc 17) rather than failing with `Unsupported`.
        in_state.lock().calls.clear();
        drv.inject(
            &x11_test_key_event(KeyPhase::Down, "KeyI", "ı", 0, vec![Modifier::Ctrl]),
            sr,
            vr,
            &cancel,
        )
        .unwrap();
        let chord_calls = in_state.lock().calls.clone();
        assert!(chord_calls.contains(&X11Call::Key(11, true))); // Control_L
        assert!(chord_calls.contains(&X11Call::Key(17, true))); // 'i'
    }

    #[test]
    fn xtest_unicode_text_caches_keymap_keeps_distinct_scratch_slots_and_unbinds_lazily() {
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: multi_scratch_keymap(),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: None,
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 800,
            height: 600,
            units: live_view::Units::PhysicalPx,
        };
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };

        drv.inject(
            &InputEvent::Text {
                text: "ğşğ".into()
            },
            sr,
            vr,
            &AtomicBool::new(false),
        )
        .unwrap();

        let st = in_state.lock();
        // Keymap was fetched only ONCE across the entire 3-character Unicode text batch.
        assert_eq!(st.keymap_calls, 1);
        // 'ğ' (0x0100_011f) bound to scratch kc 14, 'ş' (0x0100_015f) bound to distinct scratch kc 15,
        // and the second 'ğ' reused kc 14 without rebinding or unbinding!
        assert_eq!(
            st.calls,
            vec![
                X11Call::Bind(14, 0x0100_011f),
                X11Call::Key(14, true),
                X11Call::Key(14, false),
                X11Call::Bind(15, 0x0100_015f),
                X11Call::Key(15, true),
                X11Call::Key(15, false),
                X11Call::Key(14, true),
                X11Call::Key(14, false),
            ]
        );
        drop(st);

        // Scratch keycodes are unbound lazily when `release_all()` runs.
        in_state.lock().calls.clear();
        drv.release_all().unwrap();
        let release_calls = in_state.lock().calls.clone();
        assert_eq!(
            release_calls,
            vec![X11Call::Unbind(14), X11Call::Unbind(15)]
        );
    }

    #[test]
    fn xtest_drag_wheel_and_stale_geometry_rejection() {
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 1000,
            height: 800,
            units: live_view::Units::PhysicalPx,
        };
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: Some(vr),
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let cancel = AtomicBool::new(false);

        // Multi-event drag: Down -> Move -> Up
        for (phase, x, y, buttons) in [
            (PointerPhase::Down, 0.1, 0.1, 1),
            (PointerPhase::Move, 0.5, 0.5, 1),
            (PointerPhase::Up, 0.9, 0.9, 0),
        ] {
            drv.inject(
                &InputEvent::Pointer {
                    phase,
                    x,
                    y,
                    button: Some(Button::Left),
                    buttons,
                    modifiers: vec![],
                },
                sr.clone(),
                vr,
                &cancel,
            )
            .unwrap();
        }
        let drag_calls = in_state.lock().calls.clone();
        assert!(drag_calls.contains(&X11Call::Button(1, true)));
        assert!(drag_calls.contains(&X11Call::Motion(500, 400)));
        assert!(drag_calls.contains(&X11Call::Button(1, false)));

        // Wheel vertical + horizontal steps
        in_state.lock().calls.clear();
        drv.inject(
            &InputEvent::Wheel {
                x: 0.5,
                y: 0.5,
                delta_x: 80.0,
                delta_y: -40.0,
                modifiers: vec![],
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        let wheel_calls = in_state.lock().calls.clone();
        assert!(wheel_calls.contains(&X11Call::Button(4, true)));
        assert!(wheel_calls.contains(&X11Call::Button(7, true)));

        // Simulate X11 root window resize before frame geometry catches up: positional input rejected!
        in_state.lock().root_rect = Some(PixelRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        });
        in_state.lock().calls.clear();
        assert_eq!(
            drv.inject(
                &InputEvent::Pointer {
                    phase: PointerPhase::Down,
                    x: 0.5,
                    y: 0.5,
                    button: Some(Button::Left),
                    buttons: 1,
                    modifiers: vec![],
                },
                sr,
                vr,
                &cancel,
            ),
            Err(ViewError::Unavailable)
        );
        assert_eq!(in_state.lock().calls.len(), 0);
    }

    #[test]
    fn xtest_key_chord_and_repeat_handling() {
        let vr = PixelRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 800,
        };
        let sr = SourceRect {
            x: 0.0,
            y: 0.0,
            width: 1000,
            height: 800,
            units: live_view::Units::PhysicalPx,
        };
        let in_state = Arc::new(Mutex::new(MockX11InputState {
            has_xtest: true,
            keymap: base_keymap(false),
            keymap_calls: 0,
            next_conn_id: 0,
            calls_with_conn: Vec::new(),
            server_keys: BTreeSet::new(),
            server_buttons: BTreeSet::new(),
            calls: Vec::new(),
            fail_after_n: None,
            conn_lost: false,
            root_rect: Some(vr),
        }));
        let mut drv = X11InputDriver::new(MockX11Input::new(Arc::clone(&in_state))).unwrap();
        let cancel = AtomicBool::new(false);

        drv.inject(
            &InputEvent::Key {
                phase: KeyPhase::Down,
                code: "KeyA".into(),
                key: "a".into(),
                location: 0,
                repeat: false,
                modifiers: vec![Modifier::Ctrl],
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        // Duplicate Down with repeat=false is a no-op; repeat=true re-sends Key(9, true).
        let len_after_first = in_state.lock().calls.len();
        drv.inject(
            &InputEvent::Key {
                phase: KeyPhase::Down,
                code: "KeyA".into(),
                key: "a".into(),
                location: 0,
                repeat: false,
                modifiers: vec![Modifier::Ctrl],
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        assert_eq!(in_state.lock().calls.len(), len_after_first);
        drv.inject(
            &InputEvent::Key {
                phase: KeyPhase::Down,
                code: "KeyA".into(),
                key: "a".into(),
                location: 0,
                repeat: true,
                modifiers: vec![Modifier::Ctrl],
            },
            sr.clone(),
            vr,
            &cancel,
        )
        .unwrap();
        assert_eq!(in_state.lock().calls.len(), len_after_first + 1);
        drv.inject(
            &InputEvent::Key {
                phase: KeyPhase::Up,
                code: "KeyA".into(),
                key: "a".into(),
                location: 0,
                repeat: false,
                modifiers: vec![],
            },
            sr,
            vr,
            &cancel,
        )
        .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn x11_bounded_probe_times_out_when_server_accepts_and_never_answers() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use x11rb::reexports::x11rb_protocol::parse_display::ConnectAddress;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let accept_thread = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            let _ = done_rx.recv();
        });

        let started = Instant::now();
        let res = super::linux_x11::connect_bounded_address(
            &ConnectAddress::Hostname("127.0.0.1", port),
            0,
            0,
            started + Duration::from_millis(80),
        );
        let elapsed = started.elapsed();
        let _ = done_tx.send(());
        let _ = accept_thread.join();

        assert!(res.is_err());
        assert!(
            elapsed < Duration::from_millis(500),
            "bounded X11 connect/read took {elapsed:?}"
        );
    }
}
