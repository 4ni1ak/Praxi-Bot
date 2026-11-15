//! The guarded send flow of a run-scoped control grant, free of any OS call.
//!
//! A backend plugs its real window/UIA queries into [`InputProbe`] and its real
//! `SendInput`/`XTest` calls into [`InputSink`]; the order of checks and sends
//! lives here, so a test with a fake probe and a fake sink exercises exactly the
//! production flow (and removing a check here fails a test).
//!
//! Rules enforced here, all fail-closed:
//! * the keyboard **receivers** (the foreground window AND the live focused
//!   element, which may belong to another process) must each be the granted
//!   program and pass [`guard::judge`](crate::guard::judge);
//! * no modifier may be held before a send;
//! * text goes out in short chunks with a fresh check before each chunk;
//! * a key press is one validated shape only ([`chord_allowed`]);
//! * every exit path releases all keys, and an unconfirmed release is an error
//!   (the loop then revokes the grant for the run);
//! * a failure reports truthfully how much text was already delivered.

use std::fmt::Write as _;

use crate::guard::{ActGuard, InputTarget, Refusal, check_guard};

/// UTF-16 units per text chunk; a check runs before each chunk.
pub const TEXT_CHUNK_UNITS: usize = 24;

/// Live facts the guard needs, supplied by the backend.
pub trait InputProbe {
    /// Everything that would receive keyboard input right now: the foreground
    /// window and the live focused element (with its own process, elevation,
    /// class/ownership of its top-level window and live password state). Every
    /// returned fact must pass the guard.
    ///
    /// # Errors
    /// A needed fact could not be read.
    fn keyboard_receivers(&mut self) -> Result<Vec<InputTarget>, Refusal>;

    /// Everything a mouse click at `point` would reach: the hit child window
    /// and its top-level window.
    ///
    /// # Errors
    /// A needed fact could not be read.
    fn point_receivers(&mut self, point: (f64, f64)) -> Result<Vec<InputTarget>, Refusal>;

    /// Whether Ctrl, Alt, Shift or Win is physically or logically down.
    ///
    /// # Errors
    /// The key state could not be read.
    fn modifiers_down(&mut self) -> Result<bool, Refusal>;
}

/// The OS calls that send input.
pub trait InputSink {
    /// Type UTF-16 units as text.
    ///
    /// # Errors
    /// The OS refused or sent fewer events than asked.
    fn type_units(&mut self, units: &[u16]) -> Result<(), String>;

    /// Press one validated chord.
    ///
    /// # Errors
    /// As [`Self::type_units`].
    fn key_chord(&mut self, keys: &[String]) -> Result<(), String>;

    /// Click at a screen point.
    ///
    /// # Errors
    /// As [`Self::type_units`].
    fn click(&mut self, point: (f64, f64)) -> Result<(), String>;

    /// Release every key and mouse button this process may have pressed and
    /// verify (key state) that no modifier is left down. `true` = confirmed.
    fn release_all(&mut self) -> bool;
}

/// Why a guarded send did not (fully) happen, and what did happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardFailure {
    /// The guard's reason, when the guard stopped the send.
    pub refusal: Option<Refusal>,
    /// The OS error, when a send itself failed.
    pub os_error: Option<String>,
    /// UTF-16 units of text already delivered before the stop.
    pub delivered_units: usize,
    /// UTF-16 units of text asked for.
    pub total_units: usize,
    /// All keys were confirmed released.
    pub release_confirmed: bool,
}

impl GuardFailure {
    /// The error text the model and the user see. It never claims "not sent"
    /// when part of the text went out, and it says the permission is withdrawn.
    #[must_use]
    pub fn message(&self) -> String {
        let mut out = if let Some(r) = self.refusal {
            format!("stopped by the guard ({r}): {}", r.why())
        } else if let Some(e) = &self.os_error {
            format!("the send failed: {e}")
        } else {
            "the send did not complete".to_owned()
        };
        if self.delivered_units == 0 {
            out.push_str("; nothing was sent");
        } else {
            let _ = write!(
                out,
                "; {} of {} text units had already been typed before this stop and the rest was not sent (do not type the delivered part again)",
                self.delivered_units, self.total_units
            );
        }
        if !self.release_confirmed {
            out.push_str("; the keyboard state could not be confirmed clean");
        }
        out.push_str(
            "; the task-wide permission is withdrawn for this run, the next call asks again",
        );
        out
    }
}

fn verify(g: &ActGuard, facts: &[InputTarget]) -> Result<(), Refusal> {
    if facts.is_empty() {
        return Err(Refusal::NoIdentity);
    }
    facts.iter().try_for_each(|f| check_guard(g, f))
}

fn finish(
    sink: &mut dyn InputSink,
    refusal: Option<Refusal>,
    os_error: Option<String>,
    delivered_units: usize,
    total_units: usize,
) -> Result<(), GuardFailure> {
    // Every exit path, success included, releases everything and verifies.
    let release_confirmed = sink.release_all();
    if refusal.is_none() && os_error.is_none() && release_confirmed {
        return Ok(());
    }
    Err(GuardFailure {
        refusal: refusal.or(if release_confirmed {
            None
        } else {
            Some(Refusal::ReleaseUnconfirmed)
        }),
        os_error,
        delivered_units,
        total_units,
        release_confirmed,
    })
}

/// Type `text` as short chunks, checking before each one.
///
/// # Errors
/// A [`GuardFailure`] with the delivered count; the caller revokes the grant.
pub fn guarded_type(
    g: &ActGuard,
    text: &str,
    probe: &mut dyn InputProbe,
    sink: &mut dyn InputSink,
) -> Result<(), GuardFailure> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let total = units.len();
    let mut delivered = 0usize;
    let mut stop: (Option<Refusal>, Option<String>) = (None, None);
    loop {
        let check = (|| {
            if probe.modifiers_down()? {
                return Err(Refusal::ModifierHeld);
            }
            verify(g, &probe.keyboard_receivers()?)
        })();
        if let Err(r) = check {
            stop.0 = Some(r);
            break;
        }
        if delivered >= total {
            break;
        }
        let mut end = (delivered + TEXT_CHUNK_UNITS).min(total);
        if end < total && (0xD800..0xDC00).contains(&units[end - 1]) {
            end += 1;
        }
        if let Err(e) = sink.type_units(&units[delivered..end]) {
            stop.1 = Some(e);
            break;
        }
        delivered = end;
    }
    finish(sink, stop.0, stop.1, delivered, total)
}

/// Press one chord: only a validated shape, with the receivers checked first.
///
/// # Errors
/// A [`GuardFailure`]; the caller revokes the grant.
pub fn guarded_key(
    g: &ActGuard,
    keys: &[String],
    probe: &mut dyn InputProbe,
    sink: &mut dyn InputSink,
) -> Result<(), GuardFailure> {
    let check = (|| {
        if !chord_allowed(keys) {
            return Err(Refusal::Chord);
        }
        if probe.modifiers_down()? {
            return Err(Refusal::ModifierHeld);
        }
        verify(g, &probe.keyboard_receivers()?)
    })();
    if let Err(r) = check {
        return finish(sink, Some(r), None, 0, 0);
    }
    let sent = sink.key_chord(keys);
    finish(sink, None, sent.err(), 0, 0)
}

/// Click at a point (the focusing click before typing), receivers checked first.
///
/// # Errors
/// A [`GuardFailure`]; the caller revokes the grant.
pub fn guarded_click(
    g: &ActGuard,
    point: (f64, f64),
    probe: &mut dyn InputProbe,
    sink: &mut dyn InputSink,
) -> Result<(), GuardFailure> {
    let check = (|| {
        if probe.modifiers_down()? {
            return Err(Refusal::ModifierHeld);
        }
        verify(g, &probe.point_receivers(point)?)
    })();
    if let Err(r) = check {
        return finish(sink, Some(r), None, 0, 0);
    }
    let sent = sink.click(point);
    finish(sink, None, sent.err(), 0, 0)
}

/// Run a direct write (UIA `SetValue`) after validating the facts about its
/// receiver. `facts` is read by the caller immediately before this call.
///
/// # Errors
/// A [`GuardFailure`] (delivered 0) when a fact is missing or fails the guard,
/// or when the write itself fails.
pub fn guarded_write(
    g: &ActGuard,
    facts: Result<Vec<InputTarget>, Refusal>,
    write: impl FnOnce() -> Result<(), String>,
) -> Result<(), GuardFailure> {
    let fail = |refusal, os_error| GuardFailure {
        refusal,
        os_error,
        delivered_units: 0,
        total_units: 0,
        release_confirmed: true,
    };
    match facts.and_then(|f| verify(g, &f)) {
        Err(r) => Err(fail(Some(r), None)),
        Ok(()) => write().map_err(|e| fail(None, Some(e))),
    }
}

/// The only key shapes a task-wide permission may send, one per call.
///
/// * one navigation/editing key: Enter, Tab, Esc, Backspace, Delete, the arrows,
///   Home, End, `PageUp`, `PageDown`;
/// * Shift plus Tab, an arrow, Home, End, `PageUp` or `PageDown` (selection);
/// * Ctrl plus one of A, C, X, V, Z, Y, S, F.
///
/// Everything else is refused, on purpose: no Alt combination (Alt+Space and
/// Alt+F4 act outside the program), no function key, no Win/Super/Cmd, no
/// Ctrl+Esc, no Ctrl+N/O/P/W/T, no array of several keys, no repeated key.
/// A global hotkey registered by another program for one of the allowed chords
/// is not excluded by this list; that residual risk is documented.
#[must_use]
pub fn chord_allowed(keys: &[String]) -> bool {
    let norm: Vec<String> = keys.iter().map(|k| k.trim().to_ascii_lowercase()).collect();
    let nav = |k: &str| {
        matches!(
            k,
            "enter"
                | "return"
                | "tab"
                | "esc"
                | "escape"
                | "backspace"
                | "delete"
                | "del"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pgup"
                | "pagedown"
                | "pgdn"
        )
    };
    let selectable = |k: &str| {
        matches!(
            k,
            "tab"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pgup"
                | "pagedown"
                | "pgdn"
        )
    };
    match norm.as_slice() {
        [k] => nav(k),
        [m, k] if m == "shift" => selectable(k),
        [m, k] if m == "ctrl" || m == "control" => {
            matches!(k.as_str(), "a" | "c" | "x" | "v" | "z" | "y" | "s" | "f")
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use super::*;
    use crate::guard::ProcessIdentity;

    fn ident(pid: u32) -> ProcessIdentity {
        ProcessIdentity {
            image_path: "C:\\Windows\\notepad.exe".into(),
            pid,
            start_time: 99,
        }
    }

    fn fact(pid: u32, class: &str) -> InputTarget {
        InputTarget {
            window_id: 1,
            title: "t".into(),
            class: class.into(),
            identity: ident(pid),
            elevated: Some(false),
            password_focus: false,
            owned: false,
        }
    }

    fn guard() -> ActGuard {
        ActGuard { expect: ident(10) }
    }

    /// Scripted probe: one answer per call, the last repeats.
    struct Probe {
        keyboard: Vec<Result<Vec<InputTarget>, Refusal>>,
        point: Result<Vec<InputTarget>, Refusal>,
        modifiers: Result<bool, Refusal>,
        calls: usize,
    }

    impl Probe {
        fn ok() -> Self {
            Self {
                keyboard: vec![Ok(vec![fact(10, "Notepad"), fact(10, "Edit")])],
                point: Ok(vec![fact(10, "Edit"), fact(10, "Notepad")]),
                modifiers: Ok(false),
                calls: 0,
            }
        }
    }

    impl InputProbe for Probe {
        fn keyboard_receivers(&mut self) -> Result<Vec<InputTarget>, Refusal> {
            let i = self.calls.min(self.keyboard.len() - 1);
            self.calls += 1;
            self.keyboard[i].clone()
        }
        fn point_receivers(&mut self, _: (f64, f64)) -> Result<Vec<InputTarget>, Refusal> {
            self.point.clone()
        }
        fn modifiers_down(&mut self) -> Result<bool, Refusal> {
            self.modifiers
        }
    }

    #[derive(Default)]
    struct Sink {
        typed: Vec<Vec<u16>>,
        chords: Vec<Vec<String>>,
        clicks: usize,
        releases: usize,
        release_ok: bool,
        fail_type_at: Option<usize>,
    }

    impl Sink {
        fn new() -> Self {
            Self {
                release_ok: true,
                ..Self::default()
            }
        }
    }

    impl InputSink for Sink {
        fn type_units(&mut self, units: &[u16]) -> Result<(), String> {
            if self.fail_type_at == Some(self.typed.len()) {
                return Err("blocked by UIPI".into());
            }
            self.typed.push(units.to_vec());
            Ok(())
        }
        fn key_chord(&mut self, keys: &[String]) -> Result<(), String> {
            self.chords.push(keys.to_vec());
            Ok(())
        }
        fn click(&mut self, _: (f64, f64)) -> Result<(), String> {
            self.clicks += 1;
            Ok(())
        }
        fn release_all(&mut self) -> bool {
            self.releases += 1;
            self.release_ok
        }
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn typing_is_checked_before_every_chunk_and_releases_on_success() {
        let mut probe = Probe::ok();
        let mut sink = Sink::new();
        let text = "a".repeat(60); // 3 chunks of 24, 24, 12
        guarded_type(&guard(), &text, &mut probe, &mut sink).unwrap();
        assert_eq!(sink.typed.len(), 3);
        assert_eq!(
            probe.calls, 4,
            "a check before each chunk and one at the end"
        );
        assert_eq!(sink.releases, 1);
    }

    #[test]
    fn a_refusal_between_chunks_reports_the_delivered_prefix_and_releases() {
        let mut probe = Probe::ok();
        probe.keyboard = vec![
            Ok(vec![fact(10, "Notepad")]),
            Err(Refusal::ForegroundChanged),
        ];
        let mut sink = Sink::new();
        let err = guarded_type(&guard(), &"b".repeat(60), &mut probe, &mut sink).unwrap_err();
        assert_eq!(err.refusal, Some(Refusal::ForegroundChanged));
        assert_eq!(err.delivered_units, 24, "exactly the first chunk went out");
        assert_eq!(err.total_units, 60);
        assert_eq!(sink.releases, 1, "released on the refusal exit");
        let m = err.message();
        assert!(m.contains("24 of 60"), "{m}");
        assert!(!m.contains("nothing was sent"), "{m}");
        assert!(m.contains("withdrawn"), "{m}");
    }

    #[test]
    fn a_foreign_process_element_inside_the_granted_window_refuses_every_send() {
        // The window is Notepad, the focused element belongs to another process.
        let mut probe = Probe::ok();
        probe.keyboard = vec![Ok(vec![
            fact(10, "Notepad"),
            fact(77, "Chrome_RenderWidgetHostHWND"),
        ])];
        let mut sink = Sink::new();
        let e = guarded_type(&guard(), "x", &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::ForegroundChanged));
        assert!(sink.typed.is_empty());
        let e = guarded_key(&guard(), &keys(&["enter"]), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::ForegroundChanged));
        assert!(sink.chords.is_empty());
    }

    #[test]
    fn a_live_password_element_refuses_even_when_the_window_is_fine() {
        let mut probe = Probe::ok();
        let mut pw = fact(10, "Edit");
        pw.password_focus = true;
        probe.keyboard = vec![Ok(vec![fact(10, "Notepad"), pw])];
        let mut sink = Sink::new();
        let e = guarded_type(&guard(), "secret", &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::PasswordField));
        assert!(sink.typed.is_empty());
    }

    #[test]
    fn a_custom_class_owned_dialog_refuses() {
        let mut probe = Probe::ok();
        let mut dlg = fact(10, "MyCustomSaveBox");
        dlg.owned = true;
        probe.keyboard = vec![Ok(vec![dlg])];
        let mut sink = Sink::new();
        let e = guarded_key(&guard(), &keys(&["enter"]), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::Dialog));
    }

    #[test]
    fn an_unavailable_fact_or_an_empty_answer_refuses() {
        for answer in [Err(Refusal::NoIdentity), Ok(vec![])] {
            let mut probe = Probe::ok();
            probe.keyboard = vec![answer];
            let mut sink = Sink::new();
            assert!(guarded_type(&guard(), "x", &mut probe, &mut sink).is_err());
            assert!(sink.typed.is_empty());
        }
        let mut empty_class = fact(10, "");
        empty_class.class = String::new();
        let mut probe = Probe::ok();
        probe.keyboard = vec![Ok(vec![empty_class])];
        let mut sink = Sink::new();
        let e = guarded_type(&guard(), "x", &mut probe, &mut sink).unwrap_err();
        assert_eq!(
            e.refusal,
            Some(Refusal::NoIdentity),
            "failed class lookup is a refusal"
        );
    }

    #[test]
    fn a_held_modifier_refuses_before_any_send() {
        let mut probe = Probe::ok();
        probe.modifiers = Ok(true);
        let mut sink = Sink::new();
        let e = guarded_key(&guard(), &keys(&["enter"]), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::ModifierHeld));
        assert!(sink.chords.is_empty());
        probe.modifiers = Err(Refusal::NoIdentity);
        assert!(guarded_type(&guard(), "x", &mut probe, &mut sink).is_err());
    }

    #[test]
    fn a_failed_send_releases_everything_and_reports_the_os_error() {
        let mut probe = Probe::ok();
        let mut sink = Sink::new();
        sink.fail_type_at = Some(1);
        let e = guarded_type(&guard(), &"c".repeat(60), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.os_error.as_deref(), Some("blocked by UIPI"));
        assert_eq!(e.delivered_units, 24);
        assert_eq!(sink.releases, 1);
    }

    #[test]
    fn an_unconfirmed_release_is_an_error_even_after_a_successful_send() {
        let mut probe = Probe::ok();
        let mut sink = Sink::new();
        sink.release_ok = false;
        let e = guarded_key(&guard(), &keys(&["ctrl", "a"]), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::ReleaseUnconfirmed));
        assert!(!e.release_confirmed);
        assert_eq!(
            sink.chords.len(),
            1,
            "the chord went out; the release is what failed"
        );
        assert!(e.message().contains("clean"), "{}", e.message());
    }

    #[test]
    fn the_click_checks_the_hit_child_and_its_root() {
        let mut probe = Probe::ok();
        probe.point = Ok(vec![
            fact(77, "Chrome_RenderWidgetHostHWND"),
            fact(10, "Notepad"),
        ]);
        let mut sink = Sink::new();
        let e = guarded_click(&guard(), (1.0, 2.0), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::ForegroundChanged));
        assert_eq!(sink.clicks, 0);
        probe.point = Ok(vec![fact(10, "Edit"), fact(10, "Notepad")]);
        guarded_click(&guard(), (1.0, 2.0), &mut probe, &mut sink).unwrap();
        assert_eq!(sink.clicks, 1);
    }

    #[test]
    fn a_direct_write_validates_its_receiver_first() {
        let mut wrote = false;
        let r = guarded_write(&guard(), Err(Refusal::NoIdentity), || {
            wrote = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!wrote, "no SetValue when a fact is unavailable");
        let mut pw = fact(10, "Edit");
        pw.password_focus = true;
        assert!(guarded_write(&guard(), Ok(vec![pw]), || Ok(())).is_err());
        assert!(guarded_write(&guard(), Ok(vec![fact(77, "Edit")]), || Ok(())).is_err());
        assert!(guarded_write(&guard(), Ok(vec![fact(10, "Edit")]), || Ok(())).is_ok());
    }

    #[test]
    fn only_the_short_allowlist_of_chords_passes() {
        for ok in [
            &["enter"][..],
            &["Return"],
            &["tab"],
            &["esc"],
            &["backspace"],
            &["delete"],
            &["left"],
            &["down"],
            &["home"],
            &["end"],
            &["pageup"],
            &["pagedown"],
            &["shift", "tab"],
            &["shift", "left"],
            &["Shift", "End"],
            &["ctrl", "a"],
            &["ctrl", "c"],
            &["control", "v"],
            &["ctrl", "x"],
            &["ctrl", "z"],
            &["ctrl", "y"],
            &["ctrl", "s"],
            &["ctrl", "f"],
        ] {
            assert!(chord_allowed(&keys(ok)), "{ok:?}");
        }
        for bad in [
            &["alt", "space"][..],
            &["alt", "f2"],
            &["alt", "f4"],
            &["f4"],
            &["f1"],
            &["f12"],
            &["win", "r"],
            &["super"],
            &["cmd", "space"],
            &["meta"],
            &["ctrl", "esc"],
            &["ctrl", "shift", "esc"],
            &["alt", "tab"],
            &["ctrl", "n"],
            &["ctrl", "o"],
            &["ctrl", "w"],
            &["ctrl", "alt", "delete"],
            &["a"],
            &["shift", "a"],
            &["shift", "enter"],
            &["enter", "enter"],
            &["alt", "f4", "alt", "f4"],
            &["alt", "f4", "f4"],
            &["enter", "tab"],
            &[],
            &["ctrl"],
            &["ctrl", "a", "c"],
            &["nosuchkey"],
        ] {
            assert!(!chord_allowed(&keys(bad)), "{bad:?}");
        }
    }

    #[test]
    fn guarded_key_refuses_a_disallowed_chord_without_touching_the_receivers() {
        let mut probe = Probe::ok();
        let mut sink = Sink::new();
        let e = guarded_key(&guard(), &keys(&["alt", "space"]), &mut probe, &mut sink).unwrap_err();
        assert_eq!(e.refusal, Some(Refusal::Chord));
        assert_eq!(probe.calls, 0);
        assert!(sink.chords.is_empty());
        assert_eq!(sink.releases, 1, "released on this exit too");
    }
}
