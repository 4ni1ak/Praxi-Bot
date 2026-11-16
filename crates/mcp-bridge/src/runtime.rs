//! Finding a launcher (`uvx`, `npx`, ...) without trusting a stale `PATH`.
//!
//! The daemon's `PATH` is fixed when it starts; an installer run later (the
//! in-app "Install uv" button) changes the user's `PATH` in the registry or
//! profile only.  So after the normal lookup fails, the folders such
//! installers use are searched too, which makes a fresh install visible
//! without restarting the app.

use std::path::{Path, PathBuf};

/// Folders where per-user installers put launchers (existing or not).
#[must_use]
pub fn known_tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(home) = var("USERPROFILE").or_else(|| var("HOME")) {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
    }
    if let Some(local) = var("LOCALAPPDATA") {
        dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    if let Some(pf) = var("ProgramFiles") {
        dirs.push(pf.join("nodejs"));
    }
    if let Some(appdata) = var("APPDATA") {
        dirs.push(appdata.join("npm"));
    }
    for fixed in ["/opt/homebrew/bin", "/usr/local/bin"] {
        dirs.push(PathBuf::from(fixed));
    }
    dirs
}

fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| dir.join(format!("{name}.{ext}")))
            .collect()
    } else {
        vec![dir.join(name)]
    }
}

/// The launcher `name` (a bare command, no path separators) in `PATH`, then in
/// `extra`, then in [`known_tool_dirs`].
#[must_use]
pub fn find_tool(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_in(
        name,
        std::env::split_paths(&path)
            .chain(extra.iter().cloned())
            .chain(known_tool_dirs()),
    )
}

/// The launcher `name` in exactly these folders.
#[must_use]
pub fn find_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    dirs.into_iter()
        .flat_map(|d| candidates(&d, name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_dirs_are_searched_and_paths_are_not_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = if cfg!(windows) {
            "tool-xyz.exe"
        } else {
            "tool-xyz"
        };
        assert!(find_tool("tool-xyz", &[dir.path().to_path_buf()]).is_none());
        std::fs::write(dir.path().join(file), "").unwrap();
        assert!(find_tool("tool-xyz", &[dir.path().to_path_buf()]).is_some());
        assert!(find_tool("../tool-xyz", &[dir.path().to_path_buf()]).is_none());
    }
}
