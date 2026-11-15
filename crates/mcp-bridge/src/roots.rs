//! The folder a filesystem MCP server may touch must be an explicit user
//! choice: absolute, existing, a directory, and not a protected location.
//! Without this the server would be rooted wherever the daemon was started
//! (the install folder).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// npm package of the reference filesystem server.
pub const FS_SERVER_PACKAGE: &str = "@modelcontextprotocol/server-filesystem";

/// Host-supplied check of one root: `Err(reason)` rejects it (data folder,
/// credential locations, ...). The reason must be safe to show to the user.
pub type PathGuard = Arc<dyn Fn(&Path) -> Result<PathBuf, String> + Send + Sync>;

/// The root arguments of a filesystem server launch, or `None` when `args`
/// do not launch that server. Every argument after the package spec is treated as a root; flags are refused.
#[must_use]
pub fn filesystem_roots(args: &[String]) -> Option<Vec<&str>> {
    let at = args
        .iter()
        .position(|a| a == FS_SERVER_PACKAGE || a.starts_with(&format!("{FS_SERVER_PACKAGE}@")))?;
    Some(args[at + 1..].iter().map(String::as_str).collect())
}

/// Why `roots` cannot be used, in a sentence for the user; `None` = fine.
#[must_use]
pub fn root_problem(roots: &[&str], guard: Option<&PathGuard>) -> Option<String> {
    resolve_roots(roots, guard).err()
}

fn resolve_roots(roots: &[&str], guard: Option<&PathGuard>) -> Result<Vec<String>, String> {
    if roots.is_empty() {
        return Err("no folder has been chosen".to_owned());
    }
    roots
        .iter()
        .map(|root| {
            let path = Path::new(root);
            if root.starts_with('-') || !path.is_absolute() {
                return Err("not a full folder path".to_owned());
            }
            // The host resolver must run BEFORE is_dir/canonicalize: those can
            // follow network shares, mapped drives and reparse points.
            let canonical = if let Some(guard) = guard {
                guard(path)?
            } else {
                // Without a host's safe resolver there is no protection against
                // remote targets hidden behind local links. Fail closed.
                return Err("a safe folder resolver is required".to_owned());
            };
            // Rust Command handles .cmd argv quoting. Refuse expansion syntax
            // too: a literal %NAME%/!NAME! folder must never expand in a shim.
            #[cfg(windows)]
            if canonical
                .to_string_lossy()
                .contains(['%', '!', '"', '\r', '\n'])
            {
                return Err("folder contains shell expansion characters".to_owned());
            }
            canonical
                .into_os_string()
                .into_string()
                .map_err(|_| "folder is not valid Unicode".to_owned())
        })
        .collect()
}

/// Resolve every filesystem root and replace each argv element in place.
/// No shell command string is constructed; flag-shaped roots are refused.
///
/// # Errors
/// A filesystem root is missing or the host resolver refuses it.
pub fn canonicalize_filesystem_args(
    args: &mut [String],
    guard: Option<&PathGuard>,
) -> Result<(), String> {
    let Some(roots) = filesystem_roots(args) else {
        return Ok(());
    };
    let canonical = resolve_roots(&roots, guard)?;
    let start = args.len() - canonical.len();
    args[start..].clone_from_slice(&canonical);
    Ok(())
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
        assert_eq!(filesystem_roots(&a), Some(vec!["/x", "--flag"]));
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
        let guard: PathGuard =
            Arc::new(|p| std::fs::canonicalize(p).map_err(|_| "not an existing folder".to_owned()));
        assert!(
            root_problem(&[&missing], Some(&guard))
                .unwrap()
                .contains("not an existing folder")
        );
    }

    #[test]
    fn filesystem_arguments_are_resolved_and_flag_roots_are_refused() {
        let folder = std::env::temp_dir().join("a folder & another");
        let expected = folder.display().to_string();
        let result = folder.clone();
        let guard: PathGuard = Arc::new(move |_| Ok(result.clone()));
        let entered = std::env::temp_dir().display().to_string();
        let mut argv = args(&["-y", FS_SERVER_PACKAGE, &entered]);
        canonicalize_filesystem_args(&mut argv, Some(&guard)).unwrap();
        assert_eq!(
            argv,
            vec!["-y".to_owned(), FS_SERVER_PACKAGE.to_owned(), expected]
        );
        for root in ["-folder", "--flag"] {
            let mut argv = args(&["-y", FS_SERVER_PACKAGE, root]);
            assert!(canonicalize_filesystem_args(&mut argv, Some(&guard)).is_err());
        }
        #[cfg(windows)]
        for root in ["C:/%PATH%", "C:/!PATH!"] {
            let guard: PathGuard = Arc::new(|p| Ok(p.to_path_buf()));
            let mut argv = args(&["-y", FS_SERVER_PACKAGE, root]);
            assert!(canonicalize_filesystem_args(&mut argv, Some(&guard)).is_err());
        }
    }

    #[test]
    fn an_existing_absolute_folder_passes_unless_the_guard_refuses_it() {
        let dir = std::env::temp_dir();
        let d = dir.to_string_lossy().into_owned();
        let allow: PathGuard = Arc::new(|p| std::fs::canonicalize(p).map_err(|e| e.to_string()));
        assert!(root_problem(&[&d], Some(&allow)).is_none());
        assert!(root_problem(&[&d], None).is_some());
        let guard: PathGuard = Arc::new(|_| Err("protected".to_owned()));
        assert!(
            root_problem(&[&d], Some(&guard))
                .unwrap()
                .contains("protected")
        );
    }
}
