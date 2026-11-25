//! Defence in depth for the custom Tauri commands.
//!
//! The capability files decide which window label may call which command; this
//! module repeats the decision inside the highest-impact commands, so one
//! mistaken capability edit (or a future wildcard) cannot hand them to the
//! floating widget, an overlay window or a pop-out window.

/// The only window that may call the commands that start, finish or configure
/// runs, spawn processes, hand out the main window's owner key or change OS state.
pub const MAIN_ONLY: &[&str] = &["main"];

/// The windows that show the Stop control (main window and floating widget).
pub const MAIN_AND_WIDGET: &[&str] = &["main", "widget"];

/// Windows that show chat run controls for a native desktop-driving run.
pub const MAIN_AND_WIDGET_AND_CHAT: &[&str] = &["main", "widget", "chat-*"];

/// Windows that show an emergency Stop control (main, widget, and pop-outs).
pub const MAIN_AND_WIDGET_AND_POPOUT: &[&str] = &["main", "widget", "chat-*", "live-*"];

/// The Quick ask window: it may read the daemon connection, hide itself and hand a chat to the
/// main window, nothing else.
pub const QUICK_ASK_ONLY: &[&str] = &["quick-ask"];

/// Pop-out windows (`chat-*` and `live-*`): they connect to the daemon, read the owner key
/// for approvals/control, toggle their own pin-on-top flag and focus the main window.
pub const POPOUT_ONLY: &[&str] = &["chat-*", "live-*"];

/// Whether `label` matches an exact allowed label or a validated pop-out label pattern.
#[must_use]
pub fn label_allowed(label: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|pattern| match *pattern {
        "chat-*" => crate::popout::is_chat_popout_label(label),
        "live-*" => crate::popout::is_live_popout_label(label),
        exact => exact == label,
    })
}

/// Rejects a command call that does not come from one of `allowed`.
///
/// # Errors
/// When the calling window's label is not in `allowed`.
pub fn require_window<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    allowed: &[&str],
) -> Result<(), String> {
    check_label(window.label(), allowed)
}

fn check_label(label: &str, allowed: &[&str]) -> Result<(), String> {
    if label_allowed(label, allowed) {
        Ok(())
    } else {
        Err(format!("command not allowed from window \"{label}\""))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAIN_AND_WIDGET, MAIN_AND_WIDGET_AND_CHAT, MAIN_AND_WIDGET_AND_POPOUT, MAIN_ONLY,
        POPOUT_ONLY, check_label, label_allowed,
    };

    #[test]
    fn only_listed_labels_pass() {
        assert!(label_allowed("main", MAIN_ONLY));
        assert!(!label_allowed("widget", MAIN_ONLY));
        assert!(!label_allowed("overlay-0", MAIN_ONLY));
        assert!(!label_allowed("", MAIN_ONLY));
        assert!(!label_allowed("main-evil", MAIN_ONLY));
        assert!(label_allowed("widget", MAIN_AND_WIDGET));
        assert!(!label_allowed("overlay-1", MAIN_AND_WIDGET));
        assert!(label_allowed(
            "chat-33333333-3333-4333-8333-333333333333",
            POPOUT_ONLY
        ));
        assert!(label_allowed("live-home", POPOUT_ONLY));
        assert!(!label_allowed("chat-", POPOUT_ONLY));
        assert!(!label_allowed("chat-invalid", POPOUT_ONLY));
        assert!(!label_allowed("live-unknown", POPOUT_ONLY));
        assert!(!label_allowed("live-", POPOUT_ONLY));
        assert!(!label_allowed("main", POPOUT_ONLY));
        assert!(!label_allowed("quick-ask", POPOUT_ONLY));
        assert!(label_allowed(
            "chat-33333333-3333-4333-8333-333333333333",
            MAIN_AND_WIDGET_AND_CHAT
        ));
        assert!(!label_allowed("live-home", MAIN_AND_WIDGET_AND_CHAT));
        assert!(label_allowed("live-home", MAIN_AND_WIDGET_AND_POPOUT));
    }

    #[test]
    fn a_denied_call_returns_an_error_naming_the_window() {
        assert_eq!(check_label("main", MAIN_ONLY), Ok(()));
        assert_eq!(
            check_label("widget", MAIN_ONLY),
            Err("command not allowed from window \"widget\"".to_owned())
        );
        assert_eq!(
            check_label("main", POPOUT_ONLY),
            Err("command not allowed from window \"main\"".to_owned())
        );
    }
}
