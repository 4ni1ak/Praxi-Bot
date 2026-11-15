//! Deterministic GUI-safe lookup. No shell startup files are executed.
use crate::{CliEngineSpec, PathEnv};
use std::path::{Path, PathBuf};

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
        spec.aliases.iter().find_map(|alias| {
            let candidate = crate::detect::find_candidate(alias, env)?;
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
    fn generic_agent_is_not_cursor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agent.cmd"), "fixture").unwrap();
        let env = PathEnv::for_test(dir.path().to_path_buf());
        assert!(find_engine(crate::engine_spec("cursor").unwrap(), &env).is_none());
    }
}
