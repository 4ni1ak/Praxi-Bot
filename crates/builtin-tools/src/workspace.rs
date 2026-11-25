//! Per-bot persistent workspace folders, disk usage metadata, and threshold checks.
//!
//! Desktop default root: `<Documents>/Praxi/<bot>/`
//! Always-on Linux host default root: `/var/lib/praxi-work/<bot>/`
//!
//! Security boundaries:
//! * Never inside the daemon data directory (`data_dir`) or credential/CLI-config folders.
//! * Single-component folder names only; symlinks and reparse points are never traversed.
//! * Disk usage walks metadata only (`symlink_metadata`), never opening file contents.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// Standard subfolders created inside every bot's workspace.
pub const WORKSPACE_SUBDIRS: [&str; 5] = ["leads", "drafts", "downloads", "notes", "logs"];

/// Optional environment override for the workspaces root (used in tests and custom hosts).
pub const WORKSPACES_DIR_ENV: &str = "BOT_WORKSPACES_DIR";

/// Default workspaces root on an always-on Linux host (`HostMode::Host`).
pub const HOST_WORKSPACES_ROOT: &str = "/var/lib/praxi-work";

/// Default host disk usage percentage thresholds (80 / 90 / 95%).
pub const DEFAULT_WARN_PERCENT_1: u8 = 80;
pub const DEFAULT_WARN_PERCENT_2: u8 = 90;
pub const DEFAULT_WARN_PERCENT_3: u8 = 95;
/// Default minimum free space left after a write before warning (5 GiB).
pub const DEFAULT_MIN_FREE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Default age threshold for cleanup suggestions in `downloads/` and `logs/`.
pub const DEFAULT_CLEANUP_MAX_AGE_DAYS: u32 = 30;

/// Host filesystem space snapshot around the workspaces root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStats {
    /// Total disk capacity in bytes.
    pub total_bytes: u64,
    /// Free bytes available to the service user.
    pub free_bytes: u64,
    /// Used bytes (`total_bytes.saturating_sub(free_bytes)`).
    pub used_bytes: u64,
    /// Integer percentage `0..=100` of disk used.
    pub used_percent: u8,
}

/// Resolve the workspaces root path for this installation.
#[must_use]
pub fn resolve_workspaces_root(is_host: bool, override_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        return Some(dir.to_path_buf());
    }
    if let Some(env) = std::env::var_os(WORKSPACES_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(env));
    }
    if is_host {
        return Some(PathBuf::from(HOST_WORKSPACES_ROOT));
    }
    desktop_documents_dir().map(|docs| docs.join("Praxi"))
}

/// The user's Documents folder on desktop platforms.
#[must_use]
pub fn desktop_documents_dir() -> Option<PathBuf> {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.document_dir().map(Path::to_path_buf))
        .filter(|p| p.is_absolute())
        .or_else(|| crate::user_home().map(|h| h.join("Documents")))
}

/// Check whether `name` is a single safe directory name (no separators, traversal,
/// reserved Windows device names, trailing dots/spaces, or control characters).
#[must_use]
pub fn is_valid_folder_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 80 {
        return false;
    }
    if name == "." || name == ".." || name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    !reserved
}

/// Derive a readable, safe single-component folder name from a bot's display name
/// and UUID.
#[must_use]
pub fn sanitize_bot_folder_name(bot_name: &str, bot_id: &str) -> String {
    let mut out = String::with_capacity(bot_name.len().min(48));
    let mut prev_dash = false;
    for ch in bot_name.trim().chars() {
        let forbidden =
            ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        if forbidden {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(ch);
            prev_dash = ch == '-';
        }
        if out.chars().count() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches([' ', '.', '-']).to_owned();
    if is_valid_folder_name(&trimmed) {
        trimmed
    } else {
        let short: String = bot_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        if short.is_empty() {
            "bot".to_owned()
        } else {
            format!("bot-{short}")
        }
    }
}

/// Validate that `root` is an absolute local path outside `data_dir` and outside
/// any credential/CLI-config directory. Creates `root` if missing.
///
/// # Errors
/// Returns a refusal reason if `root` is unsafe or cannot be created/resolved.
pub fn validate_workspaces_root(root: &Path, data_dir: &Path) -> Result<PathBuf, String> {
    if !root.is_absolute() {
        return Err("workspaces root must be an absolute path".into());
    }
    if root.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("workspaces root cannot contain parent traversal".into());
    }
    if !root.components().any(|c| matches!(c, Component::Normal(_))) {
        return Err("a drive or filesystem root cannot be the workspaces root".into());
    }
    if crate::files::is_protected_root(root, data_dir) {
        return Err(
            "workspaces root cannot be inside Praxi's data folder or a credential folder".into(),
        );
    }
    ensure_plain_dir_chain(root)?;
    let resolved = crate::files::resolve_local_folder(root).map_err(|e| e.to_string())?;
    if crate::files::is_protected_root(&resolved, data_dir) {
        return Err(
            "workspaces root resolves inside Praxi's data folder or a credential folder".into(),
        );
    }
    Ok(storage::without_verbatim_prefix(resolved))
}

/// Ensure `<root>/<folder_name>` and its standard subfolders exist as real
/// directories inside `root` (no symlinks or reparse points).
///
/// # Errors
/// Refuses invalid folder names, protected roots, symlinks, or I/O failures.
pub fn ensure_bot_workspace_dir(
    root: &Path,
    folder_name: &str,
    data_dir: &Path,
) -> Result<PathBuf, String> {
    if !is_valid_folder_name(folder_name) {
        return Err(format!(
            "invalid bot workspace folder name: {folder_name:?}"
        ));
    }
    let root_real = validate_workspaces_root(root, data_dir)?;
    let bot_dir = root_real.join(folder_name);
    ensure_plain_child_dir(&root_real, &bot_dir)?;
    let bot_real = crate::files::resolve_local_folder(&bot_dir).map_err(|e| e.to_string())?;
    if !bot_real.starts_with(&root_real) || bot_real == root_real {
        return Err("bot workspace escaped workspaces root".into());
    }
    if crate::files::is_protected_root(&bot_real, data_dir) {
        return Err("bot workspace cannot overlap Praxi's data folder or credentials".into());
    }
    for sub in WORKSPACE_SUBDIRS {
        let sub_path = bot_real.join(sub);
        // If a file or symlink already sits at a subdir name, leave it alone rather
        // than overwriting user data, unless it is a missing entry.
        if fs::symlink_metadata(&sub_path).is_err() {
            fs::create_dir(&sub_path)
                .map_err(|e| format!("create workspace subfolder {sub}: {e}"))?;
        }
    }
    Ok(storage::without_verbatim_prefix(bot_real))
}

fn ensure_plain_dir_chain(dir: &Path) -> Result<(), String> {
    // Check existing ancestors too: an otherwise plain directory can sit below
    // a symlink/junction pointing into a different bot's tree.
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        ensure_plain_dir_chain(parent)?;
    }
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            if is_link_or_reparse(&meta) || !meta.is_dir() {
                return Err(format!("{} is not a plain directory", dir.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(dir).map_err(|err| format!("create {}: {err}", dir.display()))
        }
        Err(e) => Err(format!("stat {}: {e}", dir.display())),
    }
}

fn ensure_plain_child_dir(parent: &Path, child: &Path) -> Result<(), String> {
    if child.parent() != Some(parent) {
        return Err("workspace folder must be a direct child of workspaces root".into());
    }
    match fs::symlink_metadata(child) {
        Ok(meta) => {
            if is_link_or_reparse(&meta) || !meta.is_dir() {
                return Err(format!("{} is not a plain directory", child.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(child).map_err(|err| format!("create {}: {err}", child.display()))
        }
        Err(e) => Err(format!("stat {}: {e}", child.display())),
    }
}

#[cfg(windows)]
fn is_link_or_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || (meta.file_attributes() & 0x400 != 0)
}

#[cfg(not(windows))]
fn is_link_or_reparse(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// Sum file bytes under `path` using `symlink_metadata` only. Symlinks and
/// reparse points are skipped and file contents are never opened.
#[must_use]
pub fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if is_link_or_reparse(&meta) {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let Ok(rd) = fs::read_dir(path) else {
        return 0;
    };
    rd.flatten().map(|e| dir_size_bytes(&e.path())).sum()
}

/// Query host disk capacity and free bytes for the volume holding `path`
/// (walking up to the nearest existing ancestor if `path` does not exist yet).
#[must_use]
pub fn host_disk_stats(path: &Path) -> Option<DiskStats> {
    let mut probe = Some(path);
    while let Some(p) = probe {
        if p.exists() {
            let total = fs4::total_space(p).ok()?;
            let free = fs4::available_space(p).ok()?;
            let used = total.saturating_sub(free);
            let used_percent = percent_of(used, total);
            return Some(DiskStats {
                total_bytes: total,
                free_bytes: free,
                used_bytes: used,
                used_percent,
            });
        }
        probe = p.parent().filter(|parent| !parent.as_os_str().is_empty());
    }
    None
}

/// Integer percentage `0..=100` of `used / total`.
#[must_use]
pub fn percent_of(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let pct = (u128::from(used) * 100) / u128::from(total);
    u8::try_from(pct.min(100)).unwrap_or(100)
}

/// Why a write triggered a quota error or warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteBudgetOutcome {
    /// Allowed without warning.
    Ok,
    /// Allowed, with a pre-fill warning to surface to the owner.
    Warn(WriteWarningKind),
    /// Refused because a per-bot hard limit would be exceeded.
    Blocked {
        used_bytes: u64,
        added_bytes: u64,
        hard_limit_bytes: u64,
    },
}

/// Classification of a disk or workspace warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteWarningKind {
    /// Host disk usage crossed one of the configured percentage thresholds (80 / 90 / 95).
    DiskPercent {
        threshold_percent: u8,
        used_percent: u8,
        free_after_bytes: u64,
    },
    /// Write would leave fewer than `min_free_bytes` (default 5 GiB) free on the host disk.
    LowFreeSpace {
        free_after_bytes: u64,
        min_free_bytes: u64,
        used_percent: u8,
    },
    /// Bot workspace usage crossed the owner-configured per-bot soft limit.
    BotSoftLimit {
        used_after_bytes: u64,
        soft_limit_bytes: u64,
    },
}

/// Pure evaluation of per-bot hard/soft limits and host disk warning thresholds
/// before a write of `added_bytes`.
#[must_use]
pub fn evaluate_write_budget(
    bot_used_bytes: u64,
    added_bytes: u64,
    soft_limit_bytes: Option<u64>,
    hard_limit_bytes: Option<u64>,
    disk: Option<DiskStats>,
    thresholds: [u8; 3],
    min_free_bytes: u64,
) -> WriteBudgetOutcome {
    let used_after = bot_used_bytes.saturating_add(added_bytes);
    if let Some(hard) = hard_limit_bytes
        && used_after > hard
    {
        return WriteBudgetOutcome::Blocked {
            used_bytes: bot_used_bytes,
            added_bytes,
            hard_limit_bytes: hard,
        };
    }

    if let Some(d) = disk {
        let free_after = d.free_bytes.saturating_sub(added_bytes);
        let used_after_disk = d.total_bytes.saturating_sub(free_after);
        let used_after_pct = percent_of(used_after_disk, d.total_bytes);

        if min_free_bytes > 0 && free_after < min_free_bytes {
            return WriteBudgetOutcome::Warn(WriteWarningKind::LowFreeSpace {
                free_after_bytes: free_after,
                min_free_bytes,
                used_percent: used_after_pct,
            });
        }

        let mut sorted = thresholds;
        sorted.sort_unstable();
        if let Some(&crossed) = sorted.iter().rev().find(|&&t| t > 0 && used_after_pct >= t) {
            return WriteBudgetOutcome::Warn(WriteWarningKind::DiskPercent {
                threshold_percent: crossed,
                used_percent: used_after_pct,
                free_after_bytes: free_after,
            });
        }
    }

    if let Some(soft) = soft_limit_bytes
        && used_after > soft
    {
        return WriteBudgetOutcome::Warn(WriteWarningKind::BotSoftLimit {
            used_after_bytes: used_after,
            soft_limit_bytes: soft,
        });
    }

    WriteBudgetOutcome::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_sanitizes_traversal_and_reserved_names() {
        assert_eq!(
            sanitize_bot_folder_name("Research Bot", "12345678"),
            "Research Bot"
        );
        assert_eq!(
            sanitize_bot_folder_name("../etc/passwd", "12345678"),
            "etc-passwd"
        );
        assert_eq!(
            sanitize_bot_folder_name("CON", "abcdef01-2345"),
            "bot-abcdef01"
        );
        assert_eq!(
            sanitize_bot_folder_name("...", "abcdef01-2345"),
            "bot-abcdef01"
        );
        assert!(!is_valid_folder_name(".."));
        assert!(!is_valid_folder_name("a/b"));
        assert!(!is_valid_folder_name("NUL"));
    }

    #[test]
    fn workspaces_root_refuses_data_dir_and_credential_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("praxi-data");
        let creds = tmp.path().join("home").join(".ssh");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&creds).unwrap();

        assert!(validate_workspaces_root(&data, &data).is_err());
        assert!(validate_workspaces_root(&data.join("work"), &data).is_err());
        assert!(validate_workspaces_root(&creds, &data).is_err());

        let ok_root = tmp.path().join("Documents").join("Praxi");
        let ensured = ensure_bot_workspace_dir(&ok_root, "Ada", &data).unwrap();
        for sub in WORKSPACE_SUBDIRS {
            assert!(ensured.join(sub).is_dir(), "missing subfolder {sub}");
        }
    }

    #[test]
    fn write_budget_evaluates_hard_limit_percent_thresholds_and_low_free_space() {
        let disk_ok = Some(DiskStats {
            total_bytes: 100 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            used_bytes: 50 * 1024 * 1024 * 1024,
            used_percent: 50,
        });
        assert_eq!(
            evaluate_write_budget(
                0,
                1024,
                None,
                None,
                disk_ok,
                [80, 90, 95],
                DEFAULT_MIN_FREE_BYTES
            ),
            WriteBudgetOutcome::Ok
        );

        // Per-bot hard limit blocks when set.
        assert!(matches!(
            evaluate_write_budget(
                900,
                200,
                None,
                Some(1000),
                disk_ok,
                [80, 90, 95],
                DEFAULT_MIN_FREE_BYTES
            ),
            WriteBudgetOutcome::Blocked { .. }
        ));

        // 80/90/95 thresholds warn without blocking when hard limit is off.
        let disk_91 = Some(DiskStats {
            total_bytes: 100 * 1024 * 1024 * 1024,
            free_bytes: 9 * 1024 * 1024 * 1024,
            used_bytes: 91 * 1024 * 1024 * 1024,
            used_percent: 91,
        });
        assert_eq!(
            evaluate_write_budget(
                0,
                1024,
                None,
                None,
                disk_91,
                [80, 90, 95],
                DEFAULT_MIN_FREE_BYTES
            ),
            WriteBudgetOutcome::Warn(WriteWarningKind::DiskPercent {
                threshold_percent: 90,
                used_percent: 91,
                free_after_bytes: 9 * 1024 * 1024 * 1024 - 1024,
            })
        );

        // Leaving less than 5 GiB free warns even below 80% on a huge disk.
        let disk_huge_low_free = Some(DiskStats {
            total_bytes: 1000 * 1024 * 1024 * 1024,
            free_bytes: 5 * 1024 * 1024 * 1024,
            used_bytes: 995 * 1024 * 1024 * 1024,
            used_percent: 70, // suppose thresholds are [80,90,95] and we pass a 10 TB disk:
        });
        // Use 1000 GiB total, 5 GiB free -> 99% used triggers DiskPercent; test < 5 GiB with 10 GiB disk and thresholds [99,99,99]:
        let disk_low_free = Some(DiskStats {
            total_bytes: 100 * 1024 * 1024 * 1024,
            free_bytes: 5 * 1024 * 1024 * 1024,
            used_bytes: 95 * 1024 * 1024 * 1024,
            used_percent: 95,
        });
        let _ = disk_huge_low_free;
        assert!(matches!(
            evaluate_write_budget(
                0,
                1024 * 1024,
                None,
                None,
                disk_low_free,
                [98, 99, 99],
                DEFAULT_MIN_FREE_BYTES
            ),
            WriteBudgetOutcome::Warn(WriteWarningKind::LowFreeSpace { .. })
        ));
    }
}
