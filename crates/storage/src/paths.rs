//! Data directory resolution.

use std::ffi::OsString;
use std::path::PathBuf;

/// Environment variable overriding the data directory.
pub const DATA_DIR_ENV: &str = "BOT_DATA_DIR";

/// Resolve the data directory: `BOT_DATA_DIR` if set and non-empty, otherwise
/// the OS-specific application data directory.  Returns `None` only if the OS
/// provides no home directory.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    resolve_data_dir(std::env::var_os(DATA_DIR_ENV))
}

/// Pure resolver behind [`data_dir`] (testable without touching the process env).
#[must_use]
pub fn resolve_data_dir(override_dir: Option<OsString>) -> Option<PathBuf> {
    match override_dir {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => directories::ProjectDirs::from("dev", "praxi", "praxi-bot")
            .map(|d| d.data_dir().to_path_buf()),
    }
}

/// Strip the Windows verbatim prefix from a canonicalized drive path
/// (`\\?\C:\x` becomes `C:\x`) so paths shown to models, compared by file
/// tools or passed to child processes keep the ordinary spelling. UNC verbatim
/// paths and any path the plain form would reinterpret (very long, trailing dot
/// or space, reserved device names) stay verbatim. A no-op off Windows.
#[must_use]
pub fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return path;
        };
        let Prefix::VerbatimDisk(drive) = prefix.kind() else {
            return path;
        };
        let mut simplified = PathBuf::from(format!("{}:\\", char::from(drive)));
        for part in components {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    let text = name.to_string_lossy();
                    let stem = text
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                            && stem.len() == 4
                            && stem.as_bytes()[3].is_ascii_digit());
                    if reserved || text.ends_with('.') || text.ends_with(' ') {
                        return path;
                    }
                    simplified.push(name);
                }
                _ => return path,
            }
        }
        if simplified.as_os_str().len() > 240 {
            return path;
        }
        simplified
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn verbatim_drive_prefix_is_stripped_only_when_safe() {
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\a\data")),
            PathBuf::from(r"C:\Users\a\data")
        );
        for kept in [
            r"\\?\UNC\server\share\x",
            r"\\?\C:\a\NUL",
            r"\\?\C:\a\com1.txt",
            r"\\?\C:\a\trailing.",
            r"\\?\C:\a\trailing ",
        ] {
            assert_eq!(
                without_verbatim_prefix(PathBuf::from(kept)),
                PathBuf::from(kept)
            );
        }
        let long = format!(r"\\?\C:\{}", "a\\".repeat(130));
        assert_eq!(
            without_verbatim_prefix(PathBuf::from(&long)),
            PathBuf::from(&long)
        );
    }

    #[test]
    fn override_wins() {
        assert_eq!(
            resolve_data_dir(Some("/tmp/x".into())),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn empty_override_falls_back_to_os_default() {
        let d = resolve_data_dir(Some(OsString::new()));
        assert_eq!(d, resolve_data_dir(None));
    }
}
