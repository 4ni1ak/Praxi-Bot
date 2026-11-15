//! `SendInput` helpers: mouse, Unicode text and key chords.
#![allow(unsafe_code)]

use desktop_api::DesktopError;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS,
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VIRTUAL_KEY,
    },
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

use crate::pure::{Key, is_extended, parse_key};

fn send(inputs: &[INPUT]) -> Result<(), DesktopError> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of fully initialised INPUT structs and
    // the size argument is the size of one element.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        // A blocked send can stop midway. Do not leave a chord's keys held down.
        release_stuck_keys(&inputs[..(sent as usize).min(inputs.len())]);
        Err(DesktopError::Accessibility(format!(
            "SendInput injected {sent} of {} events (blocked by UIPI or another desktop?)",
            inputs.len()
        )))
    }
}

/// Key-ups, in reverse order, for every virtual key that `done` pressed and
/// did not release. Unicode text events carry no held state.
fn stuck_key_ups(done: &[INPUT]) -> Vec<INPUT> {
    let mut held: Vec<(u16, bool)> = Vec::new();
    for e in done {
        if e.r#type != INPUT_KEYBOARD {
            continue;
        }
        // SAFETY: `r#type` says this is the keyboard variant of the union.
        let ki = unsafe { e.Anonymous.ki };
        if ki.wVk.0 == 0 || ki.dwFlags.contains(KEYEVENTF_UNICODE) {
            continue;
        }
        if ki.dwFlags.contains(KEYEVENTF_KEYUP) {
            if let Some(pos) = held.iter().rposition(|(vk, _)| *vk == ki.wVk.0) {
                held.remove(pos);
            }
        } else {
            held.push((ki.wVk.0, ki.dwFlags.contains(KEYEVENTF_EXTENDEDKEY)));
        }
    }
    held.iter()
        .rev()
        .map(|(vk, _)| vk_event(*vk, true))
        .collect()
}

fn release_stuck_keys(done: &[INPUT]) {
    let ups = stuck_key_ups(done);
    if ups.is_empty() {
        return;
    }
    // SAFETY: a valid slice of initialised INPUT structs; best effort, the
    // result is ignored because there is nothing further to do on failure.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let _ = unsafe { SendInput(&ups, std::mem::size_of::<INPUT>() as i32) };
}

fn kbd(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Convert physical screen pixels to the 0..=65535 virtual-desktop range.
fn to_absolute(x: f64, y: f64) -> (i32, i32) {
    // SAFETY: GetSystemMetrics takes a constant and returns an int.
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
        )
    };
    let nx = (x - f64::from(vx)) * 65535.0 / f64::from(vw - 1);
    let ny = (y - f64::from(vy)) * 65535.0 / f64::from(vh - 1);
    // Clamped to the valid range first, so the cast cannot overflow.
    #[allow(clippy::cast_possible_truncation)]
    (
        nx.round().clamp(0.0, 65535.0) as i32,
        ny.round().clamp(0.0, 65535.0) as i32,
    )
}

fn move_event(x: f64, y: f64) -> INPUT {
    let (ax, ay) = to_absolute(x, y);
    mouse(
        ax,
        ay,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

/// Move the real cursor to `(x, y)` and left-click.
pub fn click_at(x: f64, y: f64) -> Result<(), DesktopError> {
    send(&[
        move_event(x, y),
        mouse(0, 0, MOUSEEVENTF_LEFTDOWN),
        mouse(0, 0, MOUSEEVENTF_LEFTUP),
    ])
}

/// Press at `from`, move to `to`, release.
pub fn drag(from: (f64, f64), to: (f64, f64)) -> Result<(), DesktopError> {
    send(&[
        move_event(from.0, from.1),
        mouse(0, 0, MOUSEEVENTF_LEFTDOWN),
    ])?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    let moved = send(&[move_event(to.0, to.1)]);
    std::thread::sleep(std::time::Duration::from_millis(50));
    // Always try to release the button, even if the move failed.
    let up = send(&[mouse(0, 0, MOUSEEVENTF_LEFTUP)]);
    moved.and(up)
}

/// Type text as Unicode key events (layout independent).
pub fn type_text(text: &str) -> Result<(), DesktopError> {
    let mut events = Vec::new();
    for unit in text.encode_utf16() {
        events.push(kbd(0, unit, KEYEVENTF_UNICODE));
        events.push(kbd(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    send(&events)
}

/// Type text in short chunks, running `check` immediately before each
/// `SendInput`. A window that takes the focus mid-text then receives at most
/// one chunk. Surrogate pairs are never split.
pub fn type_text_checked(
    text: &str,
    check: &mut dyn FnMut() -> Result<(), DesktopError>,
) -> Result<(), DesktopError> {
    const CHUNK_UNITS: usize = 24;
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return check();
    }
    let mut i = 0;
    while i < units.len() {
        let mut end = (i + CHUNK_UNITS).min(units.len());
        if end < units.len() && (0xD800..0xDC00).contains(&units[end - 1]) {
            end += 1;
        }
        let mut events = Vec::with_capacity((end - i) * 2);
        for unit in &units[i..end] {
            events.push(kbd(0, *unit, KEYEVENTF_UNICODE));
            events.push(kbd(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
        check()?;
        send(&events)?;
        i = end;
    }
    Ok(())
}

fn vk_event(vk: u16, up: bool) -> INPUT {
    let mut flags = if up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    kbd(vk, 0, flags)
}

/// Press a chord: all keys down in order, then released in reverse order.
pub fn key_chord(keys: &[String]) -> Result<(), DesktopError> {
    let mut parsed = Vec::with_capacity(keys.len());
    for k in keys {
        parsed.push(
            parse_key(k).ok_or_else(|| DesktopError::Unsupported(format!("unknown key '{k}'")))?,
        );
    }
    let mut events = Vec::new();
    for k in &parsed {
        match *k {
            Key::Vk(vk) => events.push(vk_event(vk, false)),
            Key::Char(c) => {
                let mut b = [0u16; 2];
                for unit in c.encode_utf16(&mut b) {
                    events.push(kbd(0, *unit, KEYEVENTF_UNICODE));
                }
            }
        }
    }
    for k in parsed.iter().rev() {
        match *k {
            Key::Vk(vk) => events.push(vk_event(vk, true)),
            Key::Char(c) => {
                let mut b = [0u16; 2];
                for unit in c.encode_utf16(&mut b) {
                    events.push(kbd(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
        }
    }
    send(&events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_send_releases_the_keys_it_already_pressed() {
        // ctrl down, shift down, then the send stopped before the key-ups.
        let done = [
            vk_event(0x11, false),
            vk_event(0x10, false),
            kbd(0, u16::from(b'x'), KEYEVENTF_UNICODE),
        ];
        let ups = stuck_key_ups(&done);
        let vks: Vec<(u16, bool)> = ups
            .iter()
            .map(|e| {
                // SAFETY: built by `vk_event`, the keyboard variant.
                let ki = unsafe { e.Anonymous.ki };
                (ki.wVk.0, ki.dwFlags.contains(KEYEVENTF_KEYUP))
            })
            .collect();
        assert_eq!(
            vks,
            vec![(0x10, true), (0x11, true)],
            "reverse order, key-ups"
        );
    }

    #[test]
    fn nothing_is_released_when_every_press_was_released() {
        let done = [
            vk_event(0x11, false),
            vk_event(0x41, false),
            vk_event(0x41, true),
            vk_event(0x11, true),
        ];
        assert!(stuck_key_ups(&done).is_empty());
    }

    #[test]
    fn text_is_checked_before_every_chunk_and_stops_at_the_first_refusal() {
        // No SendInput is reached when the very first check refuses.
        let mut calls = 0;
        let r = type_text_checked(&"a".repeat(100), &mut || {
            calls += 1;
            Err(DesktopError::GuardRefused("moved".into()))
        });
        assert!(matches!(r, Err(DesktopError::GuardRefused(_))));
        assert_eq!(calls, 1);
    }
}
