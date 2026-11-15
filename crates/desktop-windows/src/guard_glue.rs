//! The decisions of the run-scoped control grant on Windows, free of any Win32
//! or UI Automation call so they can be tested without a desktop.
//!
//! `uia.rs` / `identity.rs` supply the real answers through [`WinFacts`] and
//! the raw element properties; this module turns them into the facts the
//! platform-neutral guard judges. The rule is one: a fact that is unavailable
//! is a refusal, never a substitute (in particular never "the foreground
//! window" in place of an element's own window).

use desktop_api::{
    Action, ClickMethod, DesktopError, GuardFailure, InputProbe, InputSink, InputTarget,
    ProcessIdentity, Refusal, Target,
};

/// Window and process queries. `Window` is the OS handle (`HWND`).
pub trait WinFacts {
    type Window: Copy + PartialEq;

    /// The foreground top-level window.
    fn foreground(&self) -> Option<Self::Window>;
    /// The top-level ancestor of a window.
    fn root_of(&self, w: Self::Window) -> Option<Self::Window>;
    /// Facts about a top-level window (class, ownership, its own program).
    fn top_level_facts(&self, w: Self::Window) -> Option<InputTarget>;
    /// Facts about a child window under a point.
    fn child_facts(&self, w: Self::Window) -> Option<InputTarget>;
    /// Image path, pid and start time of a process.
    fn process_identity(&self, pid: u32) -> Option<ProcessIdentity>;
    /// Whether the process is elevated; `None` when unreadable.
    fn process_elevated(&self, pid: u32) -> Option<bool>;
    /// The window under a screen point and its top-level window.
    fn point_hit(&self, x: f64, y: f64) -> Option<(Self::Window, Self::Window)>;
}

/// Raw properties of one UIA element; every failed read is already the
/// refusal it must become. `native` is `Ok(None)` for a null window handle.
#[derive(Clone, Copy)]
pub struct ElementRaw<W> {
    pub pid: Result<i32, Refusal>,
    pub password: Result<bool, Refusal>,
    pub native: Result<Option<W>, Refusal>,
}

/// Facts about one UIA element: its own process, elevation, password state and
/// the class and ownership of its top-level window.
///
/// # Errors
/// Any unavailable fact. An element without a native window handle has no
/// window of its own to judge and is refused (`NoIdentity`).
pub fn element_fact<O: WinFacts>(
    os: &O,
    raw: ElementRaw<O::Window>,
) -> Result<InputTarget, Refusal> {
    let pid = raw.pid?;
    let password = raw.password?;
    let native = raw.native?.ok_or(Refusal::NoIdentity)?;
    let pid = u32::try_from(pid).map_err(|_| Refusal::NoIdentity)?;
    let top = os.root_of(native).ok_or(Refusal::NoIdentity)?;
    let mut t = os.top_level_facts(top).ok_or(Refusal::NoIdentity)?;
    // The element's own program, not its window's: they differ for an embedded
    // child that belongs to another process.
    t.identity = os.process_identity(pid).ok_or(Refusal::NoIdentity)?;
    t.elevated = os.process_elevated(pid);
    t.password_focus = password;
    Ok(t)
}

/// The facts that must all pass before keyboard input: the foreground window
/// and the live focused element.
///
/// # Errors
/// No foreground, or any unavailable fact.
pub fn keyboard_facts<O: WinFacts>(
    os: &O,
    focused: Result<ElementRaw<O::Window>, Refusal>,
) -> Result<Vec<InputTarget>, Refusal> {
    let fg = os.foreground().ok_or(Refusal::NoIdentity)?;
    let window = os.top_level_facts(fg).ok_or(Refusal::NoIdentity)?;
    Ok(vec![window, element_fact(os, focused?)?])
}

/// What a click at `(x, y)` would reach: the hit child and its top-level window.
///
/// # Errors
/// Nothing under the point, or any unavailable fact.
pub fn point_facts<O: WinFacts>(os: &O, x: f64, y: f64) -> Result<Vec<InputTarget>, Refusal> {
    let (child, root) = os.point_hit(x, y).ok_or(Refusal::NoIdentity)?;
    let c = os.child_facts(child).ok_or(Refusal::NoIdentity)?;
    let r = os.top_level_facts(root).ok_or(Refusal::NoIdentity)?;
    Ok(vec![c, r])
}

/// Facts checked before a direct `SetValue`: the keyboard facts plus the very
/// element written.
///
/// # Errors
/// As [`keyboard_facts`] and [`element_fact`].
pub fn write_facts<O: WinFacts>(
    os: &O,
    element: ElementRaw<O::Window>,
    focused: Result<ElementRaw<O::Window>, Refusal>,
) -> Result<Vec<InputTarget>, Refusal> {
    let e = element_fact(os, element)?;
    let mut all = keyboard_facts(os, focused)?;
    all.push(e);
    Ok(all)
}

/// [`InputProbe`] over [`WinFacts`], the live focused element and the key state.
pub struct Probe<'a, O: WinFacts> {
    pub os: &'a O,
    pub focused: &'a dyn Fn() -> Result<ElementRaw<O::Window>, Refusal>,
    pub modifiers_down: &'a dyn Fn() -> bool,
}

impl<O: WinFacts> InputProbe for Probe<'_, O> {
    fn keyboard_receivers(&mut self) -> Result<Vec<InputTarget>, Refusal> {
        keyboard_facts(self.os, (self.focused)())
    }

    fn point_receivers(&mut self, (x, y): (f64, f64)) -> Result<Vec<InputTarget>, Refusal> {
        point_facts(self.os, x, y)
    }

    fn modifiers_down(&mut self) -> Result<bool, Refusal> {
        Ok((self.modifiers_down)())
    }
}

/// The OS calls that send input, as `DesktopError`-returning functions.
pub trait InputApi {
    /// # Errors
    /// The OS refused or sent fewer events than asked.
    fn type_units(&mut self, units: &[u16]) -> Result<(), DesktopError>;
    /// # Errors
    /// As [`Self::type_units`].
    fn key_chord(&mut self, keys: &[String]) -> Result<(), DesktopError>;
    /// # Errors
    /// As [`Self::type_units`].
    fn click_at(&mut self, x: f64, y: f64) -> Result<(), DesktopError>;
    /// Release every key and verify none is left down.
    fn release_all(&mut self) -> bool;
}

/// [`InputSink`] over [`InputApi`]: errors become the text the guard reports.
pub struct Sink<A: InputApi>(pub A);

impl<A: InputApi> InputSink for Sink<A> {
    fn type_units(&mut self, units: &[u16]) -> Result<(), String> {
        self.0.type_units(units).map_err(|e| e.to_string())
    }

    fn key_chord(&mut self, keys: &[String]) -> Result<(), String> {
        self.0.key_chord(keys).map_err(|e| e.to_string())
    }

    fn click(&mut self, (x, y): (f64, f64)) -> Result<(), String> {
        self.0.click_at(x, y).map_err(|e| e.to_string())
    }

    fn release_all(&mut self) -> bool {
        self.0.release_all()
    }
}

/// The one thing a guarded action may do.
#[derive(Debug)]
pub enum Plan<'a> {
    /// The focusing click, at a target resolved by the caller.
    Click(&'a Target),
    Type(&'a str),
    Key(&'a [String]),
    SetValue {
        index: u32,
        generation: u64,
        value: &'a str,
    },
}

/// What a task-wide permission covers: type, keys, the focusing (global) click
/// and `SetValue` by element index. Everything else is refused.
///
/// # Errors
/// [`DesktopError::GuardRefused`] for any other action.
pub fn plan(action: &Action) -> Result<Plan<'_>, DesktopError> {
    match action {
        Action::Click {
            target,
            method: ClickMethod::Global,
        } => Ok(Plan::Click(target)),
        Action::TypeText { text } => Ok(Plan::Type(text)),
        Action::KeyPress { keys } => Ok(Plan::Key(keys)),
        Action::SetValue {
            target: Target::ByIndex { index, generation },
            value,
        } => Ok(Plan::SetValue {
            index: *index,
            generation: *generation,
            value,
        }),
        _ => Err(DesktopError::GuardRefused(
            "not sent: this action is not covered by a task-wide permission (drag, scroll, clicks other than the focusing click, and coordinate set_value always ask)"
                .into(),
        )),
    }
}

/// A guard stop as the error the tool returns. The text already says what was
/// and was not delivered.
#[must_use]
pub fn to_error(f: &GuardFailure) -> DesktopError {
    DesktopError::GuardRefused(f.message())
}

#[cfg(test)]
mod tests;
