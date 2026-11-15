//! Deterministic GUI-safe lookup. No shell startup files are executed.
use crate::{CliEngineSpec, PathEnv};
use std::path::{Path, PathBuf};

/// Drop the verbatim prefix that `canonicalize` adds on Windows (dunce-style).
/// Node and many CLIs cannot `realpath` such a path (`EISDIR: lstat 'C:'`), and it is
/// noise to show. The UNC form becomes a plain `\\server\share` path; anything that is
/// not a plain drive or UNC form is returned unchanged. A no-op off Windows.
#[must_use]
pub fn plain_path(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let mut chars = rest.chars();
        if chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.next() == Some(':')
            && matches!(chars.next(), Some('\\') | None)
        {
            return PathBuf::from(rest);
        }
    }
    path
}

pub(crate) fn trusted_directory(dir: &Path) -> bool {
    dir.is_absolute() && trusted_candidate(dir)
}

pub(crate) fn trusted_candidate(path: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(cwd) = std::env::current_dir().and_then(std::fs::canonicalize) else {
        return false;
    };
    !real.starts_with(cwd)
}

pub(crate) fn augment(env: &mut PathEnv) {
    if let Some(home) = &env.home_dir {
        for rel in [
            ".local/bin",
            ".npm-global/bin",
            ".opencode/bin",
            ".claude/local",
            ".volta/bin",
            ".bun/bin",
            "bin",
        ] {
            env.path_dirs.push(home.join(rel));
        }
        // npm's user prefix on Windows, even after a GUI was launched.
        env.path_dirs.push(home.join("AppData/Roaming/npm"));
        env.path_dirs.push(home.join(".npm-global"));
        // Scoop puts one .exe shim per tool here (`scoop install opencode`).
        env.path_dirs.push(home.join("scoop/shims"));
        if let Some(prefix) = env.env_vars.get("NPM_CONFIG_PREFIX") {
            env.path_dirs.push(PathBuf::from(prefix));
            env.path_dirs.push(PathBuf::from(prefix).join("bin"));
        }
        let nvm = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(nvm) {
            let mut bins: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|e| e.path().join("bin"))
                .collect();
            bins.sort();
            bins.reverse();
            env.path_dirs.extend(bins);
        }
    }
    if let Some(local) = &env.local_app_data {
        env.path_dirs.push(local.join("cursor-agent"));
        // Antigravity CLI (`agy`) installs here on Windows.
        env.path_dirs.push(local.join("agy/bin"));
        env.path_dirs.push(local.join("Microsoft/WinGet/Links"));
    }
    if let Some(roaming) = env.env_vars.get("APPDATA") {
        env.path_dirs.push(PathBuf::from(roaming).join("npm"));
    }
    for key in ["ProgramFiles", "PROGRAMFILES", "NVM_SYMLINK"] {
        if let Some(dir) = env.env_vars.get(key) {
            let path = PathBuf::from(dir);
            env.path_dirs.push(if key == "NVM_SYMLINK" {
                path
            } else {
                path.join("nodejs")
            });
        }
    }
    #[cfg(unix)]
    env.path_dirs
        .extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    env.path_dirs.retain(|p| p.is_absolute());
    let mut seen = std::collections::HashSet::new();
    env.path_dirs.retain(|p| seen.insert(p.clone()));
}

#[must_use]
pub fn find_engine(spec: &CliEngineSpec, env: &PathEnv) -> Option<PathBuf> {
    crate::detect::find_candidate(spec.command, env).or_else(|| {
        // Generic `agent` is shared by unrelated products. Accept only from the
        // vendor-specific installation directory, never an arbitrary PATH hit.
        let mut vendor_env = env.clone();
        vendor_env.path_dirs.retain(|dir| {
            dir.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("cursor-agent"))
        });
        spec.aliases.iter().find_map(|alias| {
            let candidate = crate::detect::find_candidate(alias, &vendor_env)?;
            let parent = candidate.parent()?.file_name()?.to_str()?;
            parent
                .eq_ignore_ascii_case("cursor-agent")
                .then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_relative_cwd_and_redirected_candidates() {
        let cwd = std::env::current_dir().unwrap();
        assert!(!trusted_directory(Path::new(".")));
        assert!(!trusted_directory(&cwd));
        assert!(!trusted_candidate(&cwd.join("Cargo.toml")));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("claude.cmd"), "fixture").unwrap();
        let env = PathEnv::for_test(dir.path().to_path_buf());
        assert!(
            crate::detect::find_candidate("claude", &env)
                .unwrap()
                .is_absolute()
        );
        assert!(crate::detect::find_candidate("../claude", &env).is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(cwd.join("Cargo.toml"), dir.path().join("hijack")).unwrap();
            assert!(crate::detect::find_candidate("hijack", &env).is_none());
        }
    }
    #[test]
    fn verbatim_prefix_is_removed_only_for_drive_and_unc_paths() {
        let p = |s: &str| plain_path(PathBuf::from(s));
        assert_eq!(
            p(r"\\?\C:\Users\a\x.cmd"),
            PathBuf::from(r"C:\Users\a\x.cmd")
        );
        assert_eq!(p(r"\\?\UNC\srv\share\x"), PathBuf::from(r"\\srv\share\x"));
        assert_eq!(p(r"\\?\Volume{1}\x"), PathBuf::from(r"\\?\Volume{1}\x"));
        assert_eq!(p("/usr/bin/x"), PathBuf::from("/usr/bin/x"));
    }
    #[test]
    fn scoop_shims_are_searched_under_home() {
        let mut env =
            PathEnv::for_test(std::env::temp_dir()).with_home(std::env::temp_dir().join("h"));
        augment(&mut env);
        assert!(env.path_dirs.iter().any(|d| d.ends_with("scoop/shims")));
    }
    #[test]
    fn generic_agent_is_not_cursor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agent.cmd"), "fixture").unwrap();
        let env = PathEnv::for_test(dir.path().to_path_buf());
        assert!(find_engine(crate::engine_spec("cursor").unwrap(), &env).is_none());
        let vendor = dir.path().join("cursor-agent");
        std::fs::create_dir(&vendor).unwrap();
        std::fs::write(vendor.join("agent.cmd"), b"fixture").unwrap();
        let mut env = env;
        env.path_dirs.push(vendor.clone());
        let found = find_engine(crate::engine_spec("cursor").unwrap(), &env).unwrap();
        let vendor = plain_path(std::fs::canonicalize(vendor).unwrap());
        assert_eq!(found.parent(), Some(vendor.as_path()));
    }

    #[test]
    fn gui_fallback_directories_find_installations_created_after_startup() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = PathEnv::for_test(dir.path().join("empty-path"))
            .with_home(dir.path().join("home"))
            .with_local_app_data(dir.path().join("local"));
        augment(&mut env);
        let installed = dir.path().join("home/.local/bin");
        std::fs::create_dir_all(&installed).unwrap();
        let name = format!("claude{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(installed.join(name), b"fixture").unwrap();
        assert!(find_engine(crate::engine_spec("claude").unwrap(), &env).is_some());
        let cursor = dir.path().join("local/cursor-agent");
        std::fs::create_dir_all(&cursor).unwrap();
        std::fs::write(cursor.join("agent.cmd"), b"fixture").unwrap();
        assert!(find_engine(crate::engine_spec("cursor").unwrap(), &env).is_some());
    }
}
