//! Host desktop selection, shared with `bot-node`.
use std::sync::Arc;

use desktop_api::DesktopBackend;
use live_view::UnavailableCode;

/// Why the desktop backend did not start: a fixed public [`UnavailableCode`]
/// plus the raw message, which is for the LOCAL daemon log only (it can name
/// paths, user names, display sockets or environment values).
#[derive(Debug, Clone)]
pub struct BackendFailure {
    pub code: UnavailableCode,
    pub detail: String,
}

impl BackendFailure {
    fn new(code: UnavailableCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// `BOT_DESKTOP=off|0|false`.
#[must_use]
pub fn desktop_switched_off() -> bool {
    desktop_off(std::env::var("BOT_DESKTOP").ok().as_deref())
}

pub fn local_desktop(warn: &mut dyn FnMut(&str)) -> Option<Arc<dyn DesktopBackend>> {
    if desktop_off(std::env::var("BOT_DESKTOP").ok().as_deref()) {
        return None;
    }
    match backend() {
        Ok(backend) => Some(backend),
        Err(error) => {
            warn(&format!("desktop tools disabled: {error}"));
            None
        }
    }
}

fn desktop_off(value: Option<&str>) -> bool {
    matches!(value, Some("off" | "0" | "false"))
}

#[cfg(any(target_os = "linux", test))]
fn display_session(
    display: Option<&str>,
    wayland: Option<&str>,
    session: Option<&str>,
) -> Result<String, BackendFailure> {
    desktop_linux::pure::detect_display_session(session, wayland, display).map_err(|e| {
        let code = match e {
            desktop_api::DesktopError::Unsupported(_) => UnavailableCode::Wayland,
            _ => UnavailableCode::NoDisplay,
        };
        BackendFailure::new(code, e.to_string())
    })
}

pub fn backend() -> Result<Arc<dyn DesktopBackend>, String> {
    backend_checked().map_err(|f| f.detail)
}

/// [`backend`] with a typed, publishable reason for the failure.
pub fn backend_checked() -> Result<Arc<dyn DesktopBackend>, BackendFailure> {
    #[cfg(windows)]
    {
        desktop_windows::WindowsBackend::new()
            .map(|b| Arc::new(b) as Arc<dyn DesktopBackend>)
            .map_err(|e| {
                BackendFailure::new(
                    UnavailableCode::StartFailed,
                    format!("cannot start the Windows backend: {e}"),
                )
            })
    }
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").ok();
        let session = std::env::var("XDG_SESSION_TYPE").ok();
        let wayland = std::env::var("WAYLAND_DISPLAY").ok();
        display_session(display.as_deref(), wayland.as_deref(), session.as_deref())?;
        if !browser_cdp::host::has_display() {
            return Err(BackendFailure::new(
                UnavailableCode::NoDisplay,
                "X11 DISPLAY is unreachable; check Xvfb, DISPLAY and XAUTHORITY",
            ));
        }
        let backend = desktop_linux::LinuxBackend::new()
            .map_err(|e| BackendFailure::new(UnavailableCode::StartFailed, e.to_string()))?;
        Ok(Arc::new(backend))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    Err(BackendFailure::new(
        UnavailableCode::UnsupportedPlatform,
        format!(
            "desktop control is not supported on {} yet",
            std::env::consts::OS
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_selection_is_pure_and_desktop_off_wins() {
        for value in [Some("off"), Some("0"), Some("false")] {
            assert!(desktop_off(value));
        }
        assert!(!desktop_off(None));
        assert!(!desktop_off(Some("on")));
        assert!(display_session(None, None, None).is_err());
        assert!(display_session(Some("  "), None, Some("x11")).is_err());
        assert!(display_session(None, Some("wayland-0"), Some("wayland")).is_err());
        assert_eq!(display_session(Some(":99"), None, None).unwrap(), ":99");
        // Wayland and a missing display are typed; the raw message (which can
        // carry a socket path) stays out of the public code.
        let wayland =
            display_session(None, Some("/home/alice/private/wayland-0"), None).unwrap_err();
        assert_eq!(wayland.code, UnavailableCode::Wayland);
        assert!(wayland.detail.contains("alice"), "kept for the local log");
        assert_eq!(
            display_session(None, None, None).unwrap_err().code,
            UnavailableCode::NoDisplay
        );
        // An explicit X11 session (the node --display child) takes precedence.
        assert_eq!(
            display_session(Some(":99"), Some("wayland-0"), Some("x11")).unwrap(),
            ":99"
        );
    }
}
