//! Narrow system-browser opener, available only to the main window.

fn allowed(href: &str, dashboard: bool) -> bool {
    let Some(rest) = href.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !href.chars().any(|c| c.is_control() || c == '\\')
        && if dashboard {
            matches!(host, "platform.composio.dev" | "dashboard.composio.dev")
        } else {
            matches!(host, "connect.composio.dev" | "backend.composio.dev")
        }
}

/// Validate and hand a URL to the operating system browser.
///
/// # Errors
/// Unapproved URL or OS opener failure. Success means OS accepted the request.
pub fn open_composio(href: &str, dashboard: bool) -> Result<(), String> {
    if !allowed(href, dashboard) {
        return Err("unapproved Composio URL".into());
    }
    open(href)
}

fn open(href: &str) -> Result<(), String> {
    #[cfg(windows)]
    let result = open::that_detached(href);
    #[cfg(not(windows))]
    let result = open::that(href);
    result.map_err(|_| "system browser could not be opened".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_browser_hosts_and_separate_dashboard() {
        assert!(allowed("https://connect.composio.dev/link/x", false));
        assert!(allowed("https://platform.composio.dev/", true));
        assert!(allowed("https://dashboard.composio.dev/?cliKey=abc", true));
        assert!(!allowed("https://dashboard.composio.dev/", false));
        assert!(!allowed("https://platform.composio.dev/", false));
        for bad in [
            "https://logos.composio.dev/x",
            "https://unexpected.composio.dev/x",
            "https://user@connect.composio.dev/x",
            "https://connect.composio.dev:443/x",
            "https://%63onnect.composio.dev/x",
            "https://connect.composio.dev.evil/x",
            "https://connect.composio.dev\\@evil/x",
        ] {
            assert!(!allowed(bad, false), "{bad}");
        }
    }
}
