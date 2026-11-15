//! The folder a filesystem MCP server may touch must be an explicit user
//! choice: absolute, existing, a directory, and not a protected location.
//! Without this the server would be rooted wherever the daemon was started
//! (the install folder).

use std::{path::Path, sync::Arc};

/// npm package of the reference filesystem server.
pub const FS_SERVER_PACKAGE: &str = "@modelcontextprotocol/server-filesystem";

/// Host-supplied check of one root: `Some(reason)` rejects it (data folder,
/// credential locations, ...). The reason must be safe to show to the user.
pub type PathGuard = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// The root arguments of a filesystem server launch, or `None` when `args`
/// do not launch that server. Roots are the non-flag arguments after the
/// package spec.
#[must_use]
pub fn filesystem_roots(args: &[String]) -> Option<Vec<&str>> {
    let at = args
        .iter()
        .position(|a| a == FS_SERVER_PACKAGE || a.starts_with(&format!("{FS_SERVER_PACKAGE}@")))?;
    Some(
        args[at + 1..]
            .iter()
            .map(String::as_str)
            .filter(|a| !a.starts_with('-'))
            .collect(),
    )
}

/// Why `roots` cannot be used, in a sentence for the user; `None` = fine.
#[must_use]
pub fn root_problem(roots: &[&str], guard: Option<&PathGuard>) -> Option<String> {
    if roots.is_empty() {
        return Some("no folder has been chosen".to_owned());
    }
    for root in roots {
        let path = Path::new(root);
        if !path.is_absolute() {
            return Some(format!(
                "`{root}` is not a full folder path (a relative path would depend on where Praxi-Bot was started)"
            ));
        }
        if !path.is_dir() {
            return Some(format!("`{root}` is not an existing folder"));
        }
        if let Some(why) = guard.and_then(|g| g(path)) {
            return Some(format!("`{root}`: {why}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn detects_the_filesystem_server_and_its_roots() {
        let a = args(&[
            "-y",
            "@modelcontextprotocol/server-filesystem@2.2.0",
            "/x",
            "--flag",
        ]);
        assert_eq!(filesystem_roots(&a), Some(vec!["/x"]));
        assert_eq!(
            filesystem_roots(&args(&[
                "-y",
                "@modelcontextprotocol/server-filesystem@2.2.0"
            ])),
            Some(vec![])
        );
        assert_eq!(
            filesystem_roots(&args(&["-y", "@playwright/mcp@0.0.84"])),
            None
        );
    }

    #[test]
    fn missing_relative_or_nonexistent_roots_are_refused() {
        assert!(root_problem(&[], None).unwrap().contains("no folder"));
        assert!(
            root_problem(&["."], None)
                .unwrap()
                .contains("not a full folder path")
        );
        let missing = std::env::temp_dir().join("praxi-no-such-dir-244b");
        let missing = missing.to_string_lossy().into_owned();
        assert!(
            root_problem(&[&missing], None)
                .unwrap()
                .contains("not an existing folder")
        );
    }

    #[test]
    fn an_existing_absolute_folder_passes_unless_the_guard_refuses_it() {
        let dir = std::env::temp_dir();
        let d = dir.to_string_lossy().into_owned();
        assert!(root_problem(&[&d], None).is_none());
        let guard: PathGuard = Arc::new(|_| Some("protected".to_owned()));
        assert!(
            root_problem(&[&d], Some(&guard))
                .unwrap()
                .contains("protected")
        );
    }
}
