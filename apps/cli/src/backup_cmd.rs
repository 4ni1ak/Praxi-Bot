//! `bot backup` and `bot restore`: one file with the installation's portable
//! state (bots, chats, memory, skills, workflows, schedules, provider
//! settings). Built on `state-bundle`.
//!
//! Secrets (API keys, tokens) are never written to a backup: after a restore
//! the keys are entered again (`bot secrets set`, or the app's settings).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use secret_store::{MemoryStore, SecretStore};
use state_bundle::{ExportOptions, ImportError, ImportOptions};

#[derive(Args)]
pub struct BackupCmd {
    /// File to write (must not exist yet), for example `praxi-backup.tar`.
    pub file: PathBuf,
}

#[derive(Args)]
pub struct RestoreCmd {
    /// Backup file written by `bot backup`.
    pub file: PathBuf,
    /// Replace the bots and chats that are already here. The previous state is
    /// kept in a `backup-*` folder inside the data directory.
    #[arg(long)]
    pub replace: bool,
    /// Accept a backup whose permission policy is more permissive than the
    /// current one.
    #[arg(long)]
    pub allow_policy_relaxation: bool,
}

/// Write a backup of `data_dir` to `cmd.file`. Safe while the daemon runs (SQLite
/// databases are copied with the online backup API), but only a stopped daemon
/// gives a snapshot that is consistent across files.
pub fn backup(data_dir: &Path, cmd: &BackupCmd, out: &mut dyn Write) -> crate::Result<()> {
    if !data_dir.join("bot.db").is_file() {
        return Err(format!(
            "nothing to back up: no Praxi data found in {} (use --data-dir if it lives elsewhere)",
            data_dir.display()
        )
        .into());
    }
    // Written next to the target and renamed when complete, so a failed backup
    // never leaves something that looks valid.
    let partial = partial_path(&cmd.file);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&cmd.file)
        .map_err(|e| format!("cannot create {}: {e}", cmd.file.display()))?;
    drop(file);
    let result = (|| -> crate::Result<state_bundle::Manifest> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&partial)?;
        let sink = tokio::fs::File::from_std(file);
        let manifest = crate::current_thread_runtime()?.block_on(state_bundle::export(
            data_dir,
            // Nothing is selected, so the store is never read.
            &MemoryStore::new(),
            ExportOptions::default(),
            sink,
        ))?;
        std::fs::rename(&partial, &cmd.file)?;
        Ok(manifest)
    })();
    match result {
        Ok(manifest) => {
            let bytes: u64 = manifest.files.iter().map(|f| f.size).sum();
            writeln!(
                out,
                "Backup written to {} ({} files, {} bytes).",
                cmd.file.display(),
                manifest.files.len(),
                bytes
            )?;
            writeln!(
                out,
                "It does not contain API keys or tokens; enter them again after a restore."
            )?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            let _ = std::fs::remove_file(&cmd.file);
            Err(e)
        }
    }
}

fn partial_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

/// Restore `cmd.file` into `data_dir`. The daemon must be stopped.
pub fn restore(
    data_dir: &Path,
    port: u16,
    store: &dyn SecretStore,
    cmd: &RestoreCmd,
    out: &mut dyn Write,
) -> crate::Result<()> {
    if daemon_listening(port) {
        return Err(format!(
            "a Praxi daemon is answering on port {port}; close the app (or stop `bot serve`) before restoring"
        )
        .into());
    }
    let source = std::fs::File::open(&cmd.file)
        .map_err(|e| format!("cannot open {}: {e}", cmd.file.display()))?;
    let opts = ImportOptions {
        replace: cmd.replace,
        allow_policy_relaxation: cmd.allow_policy_relaxation,
        ..ImportOptions::default()
    };
    let outcome = crate::current_thread_runtime()?
        .block_on(state_bundle::import(
            data_dir,
            store,
            tokio::fs::File::from_std(source),
            opts,
        ))
        .map_err(|e| match e {
            ImportError::TargetNotEmpty => {
                "this installation already has bots or chats; run again with --replace to swap them for the backup (the current state is kept in a backup-* folder)".to_owned()
            }
            ImportError::PolicyRelaxation => {
                "the backup's permission policy is more permissive than the current one; run again with --allow-policy-relaxation if that is what you want".to_owned()
            }
            other => other.to_string(),
        })?;
    writeln!(
        out,
        "Restored {} files from {} (created with Praxi {}).",
        outcome.manifest.files.len(),
        cmd.file.display(),
        outcome.manifest.app_version
    )?;
    if let Some(previous) = outcome.backup_dir {
        writeln!(
            out,
            "The previous state was kept in {}.",
            previous.display()
        )?;
    }
    writeln!(
        out,
        "API keys are not part of a backup: enter them again (`bot secrets set`, or the app's settings)."
    )?;
    Ok(())
}

fn daemon_listening(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_with_bot(dir: &Path) {
        let storage = storage::Storage::open(dir).unwrap();
        let user = protocol::UserId::new();
        storage
            .insert_user(&storage::User {
                id: user,
                name: "owner".into(),
                created_at_ms: 1,
            })
            .unwrap();
        storage
            .insert_bot(&storage::Bot {
                id: protocol::BotId::new(),
                user_id: user,
                name: "Atlas".into(),
                config: serde_json::json!({"bot": {"name": "Atlas"}}),
                created_at_ms: 1,
            })
            .unwrap();
    }

    #[test]
    fn a_backup_restores_into_an_empty_data_dir_and_never_overwrites_a_file() {
        let src = tempfile::tempdir().unwrap();
        data_with_bot(src.path());
        let out_dir = tempfile::tempdir().unwrap();
        let file = out_dir.path().join("praxi.tar");
        let mut said = Vec::new();
        backup(src.path(), &BackupCmd { file: file.clone() }, &mut said).unwrap();
        assert!(file.exists());
        assert!(!partial_path(&file).exists());
        assert!(
            backup(src.path(), &BackupCmd { file: file.clone() }, &mut said).is_err(),
            "an existing file is never overwritten"
        );

        let dst = tempfile::tempdir().unwrap();
        // Port 1 is never a Praxi daemon.
        restore(
            dst.path(),
            1,
            &MemoryStore::new(),
            &RestoreCmd {
                file,
                replace: false,
                allow_policy_relaxation: false,
            },
            &mut said,
        )
        .unwrap();
        let restored = storage::Storage::open(dst.path()).unwrap();
        let user = restored.list_users().unwrap()[0].id;
        let bots = restored.list_bots(user).unwrap();
        assert_eq!(bots.len(), 1);
        assert_eq!(bots[0].name, "Atlas");
    }

    #[test]
    fn backup_of_an_empty_directory_says_so_and_creates_no_file() {
        let empty = tempfile::tempdir().unwrap();
        let file = empty.path().join("out.tar");
        let err = backup(
            empty.path(),
            &BackupCmd { file: file.clone() },
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("nothing to back up"), "{err}");
        assert!(!file.exists());
    }

    #[test]
    fn restore_refuses_a_populated_install_without_replace() {
        let src = tempfile::tempdir().unwrap();
        data_with_bot(src.path());
        let file = tempfile::tempdir().unwrap().keep().join("b.tar");
        backup(
            src.path(),
            &BackupCmd { file: file.clone() },
            &mut Vec::new(),
        )
        .unwrap();
        let err = restore(
            src.path(),
            1,
            &MemoryStore::new(),
            &RestoreCmd {
                file,
                replace: false,
                allow_policy_relaxation: false,
            },
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--replace"), "{err}");
    }
}
