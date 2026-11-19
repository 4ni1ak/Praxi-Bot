//! Conservative control-plane boundary, independent of catalog display classes.

pub(crate) fn toolkit_allowed(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && ![
            "composio",
            "code",
            "bash",
            "shell",
            "workbench",
            "tool_router",
            "auth",
            "control",
            "meta",
            "connect",
            "router",
            "browser",
            "automation",
            "connect_link",
            "session",
            "remote",
            "e2b",
            "python",
            "terminal",
            "proxy",
            "pipedream",
            "zapier",
            "n8n",
        ]
        .iter()
        .any(|p| slug.contains(p))
}

pub(crate) fn action_allowed(toolkit: &str, slug: &str) -> bool {
    toolkit_allowed(toolkit)
        && slug.starts_with(&format!("{}_", toolkit.to_ascii_uppercase()))
        && slug
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && ![
            "COMPOSIO",
            "EXECUTE",
            "EXECUTION",
            "BASH",
            "SHELL",
            "WORKBENCH",
            "CODE",
            "AUTH",
            "CONTROL",
            "META",
            "ROUTER",
            "CONNECT_LINK",
            "CONNECTION",
            "TOOL_ROUTER",
            "SESSION",
            "PROXY",
            "TERMINAL",
            "SCRIPT",
            "COMMAND",
            "EVAL",
            "WORKFLOW",
            "RUN_",
        ]
        .iter()
        .any(|p| slug.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_and_execution_families_are_denied_independent_of_display_class() {
        for toolkit in [
            "composio",
            "composio_search",
            "codeinterpreter",
            "remote_workbench",
            "auth0",
            "tool_router",
            "control_plane",
            "metatools",
            "browserbase",
        ] {
            assert!(!toolkit_allowed(toolkit), "{toolkit}");
        }
        for action in [
            "COMPOSIO_EXECUTE_TOOL",
            "GITHUB_REMOTE_BASH",
            "GITHUB_REMOTE_WORKBENCH",
            "GITHUB_CREATE_CONNECT_LINK",
            "GITHUB_CREATE_AUTH_CONFIG",
            "GITHUB_GET_SESSION",
            "GITHUB_TOOL_ROUTER",
            "GITHUB_EVAL_SCRIPT",
        ] {
            assert!(!action_allowed("github", action), "{action}");
        }
        assert!(action_allowed("github", "GITHUB_LIST_REPOSITORY_ISSUES"));
    }
}
