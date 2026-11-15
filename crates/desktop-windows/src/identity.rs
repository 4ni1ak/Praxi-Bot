//! Process identity, elevation and window class for the run-scoped control
//! grant (card 243-1). Every function returns `None` / an empty value on any
//! failure; the callers treat that as "unknown" and refuse.
#![allow(unsafe_code)]

use desktop_api::{InputTarget, ProcessIdentity};
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, HWND, POINT},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{
            GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetClassNameW, WindowFromPoint},
    },
    core::PWSTR,
};

use crate::winlist;

struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and owns.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open(pid: u32) -> Option<Owned> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain API call; a failure (protected process, gone) is `None`.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .map(Owned)
}

fn filetime_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Image path, pid and creation time of a process; `None` when any is unknown.
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    let h = open(pid)?;
    let mut buf = vec![0u16; 1024];
    let mut len = u32::try_from(buf.len()).ok()?;
    // SAFETY: `buf` is writable for `len` UTF-16 units; `len` is an in/out size.
    unsafe {
        QueryFullProcessImageNameW(
            h.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &raw mut len,
        )
    }
    .ok()?;
    let image_path = String::from_utf16_lossy(buf.get(..len as usize)?);
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: four valid out pointers to locals, and a live process handle.
    unsafe {
        GetProcessTimes(
            h.0,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .ok()?;
    let id = ProcessIdentity {
        image_path,
        pid,
        start_time: filetime_u64(created),
    };
    id.is_complete().then_some(id)
}

/// Whether the process runs elevated. `None` when the token cannot be read,
/// which is the normal result for an elevated process seen from a normal one.
pub fn process_elevated(pid: u32) -> Option<bool> {
    let h = open(pid)?;
    let mut tok = HANDLE::default();
    // SAFETY: valid process handle and an out pointer to a local.
    unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &raw mut tok) }.ok()?;
    let tok = Owned(tok);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the buffer is a `TOKEN_ELEVATION` and the size passed matches it.
    unsafe {
        GetTokenInformation(
            tok.0,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).ok()?,
            &raw mut len,
        )
    }
    .ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// Window class name, empty when unknown.
pub fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer is writable for its whole length.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    usize::try_from(n)
        .ok()
        .and_then(|n| buf.get(..n))
        .map(String::from_utf16_lossy)
        .unwrap_or_default()
}

/// Top-level window under a screen point.
pub fn root_window_at(x: f64, y: f64) -> Option<HWND> {
    #[allow(clippy::cast_possible_truncation)]
    let pt = POINT {
        x: x.round() as i32,
        y: y.round() as i32,
    };
    // SAFETY: plain queries by value.
    let hit = unsafe { WindowFromPoint(pt) };
    if hit.0.is_null() {
        return None;
    }
    // SAFETY: plain query on a handle the OS just returned.
    let root = unsafe { GetAncestor(hit, GA_ROOT) };
    Some(if root.0.is_null() { hit } else { root })
}

/// Facts about the window `hwnd` and its program. `None` when the program
/// cannot be identified. `password_focus` is filled in by the caller.
pub fn window_target(hwnd: HWND) -> Option<InputTarget> {
    let pid = winlist::window_pid(hwnd);
    let identity = process_identity(pid)?;
    let info = winlist::window_info(hwnd)?;
    Some(InputTarget {
        window_id: info.id,
        title: info.title,
        class: window_class(hwnd),
        identity,
        elevated: process_elevated(pid),
        password_focus: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_has_a_complete_identity_and_a_readable_token() {
        let pid = std::process::id();
        let id = process_identity(pid).expect("own identity");
        assert!(id.is_complete());
        assert_eq!(id.pid, pid);
        let exe = std::env::current_exe().unwrap();
        assert_eq!(
            desktop_api::guard::program_stem(&id.image_path),
            desktop_api::guard::program_stem(&exe.to_string_lossy())
        );
        assert!(process_elevated(pid).is_some());
        // A second read gives the same start time: the pair names one process.
        assert_eq!(process_identity(pid), Some(id));
    }

    #[test]
    fn pid_zero_and_a_missing_process_have_no_identity() {
        assert_eq!(process_identity(0), None);
        assert_eq!(process_elevated(0), None);
        assert_eq!(process_identity(u32::MAX - 3), None);
    }
}
