//! Descriptor-relative traversal: a concurrent rename or symlink swap must
//! never redirect an import outside the owner-selected source tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use super::{MAX_IMPORT_ENTRIES, OwnerError, Result, add_file, read_bounded, safe_relative};
use rustix::fd::OwnedFd;
use rustix::fs::{Dir, FileType, Mode, OFlags, fstat, open, openat};

pub(super) fn folder(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    if !root.is_absolute() {
        return Err(OwnerError::Invalid);
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty()).map_err(|_| OwnerError::Storage)?;
    for part in root.components() {
        match part {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, flags, Mode::empty())
                    .map_err(|_| OwnerError::Invalid)?;
            }
            _ => return Err(OwnerError::Invalid),
        }
    }
    let mut snapshot = Snapshot {
        files: BTreeMap::new(),
        seen: BTreeSet::new(),
        entries: 0,
        total: 0,
    };
    snapshot.visit(&directory, "")?;
    Ok(snapshot.files)
}

struct Snapshot {
    files: BTreeMap<String, Vec<u8>>,
    seen: BTreeSet<String>,
    entries: usize,
    total: usize,
}

impl Snapshot {
    fn visit(&mut self, directory: &OwnedFd, prefix: &str) -> Result<()> {
        let entries = Dir::read_from(directory).map_err(|_| OwnerError::Storage)?;
        for entry in entries {
            let entry = entry.map_err(|_| OwnerError::Storage)?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| OwnerError::Invalid)?;
            if matches!(name, "." | "..") {
                continue;
            }
            self.entries += 1;
            if self.entries > MAX_IMPORT_ENTRIES {
                return Err(OwnerError::TooLarge);
            }
            let rel = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            safe_relative(&rel)?;
            if !self.seen.insert(rel.to_ascii_lowercase()) {
                return Err(OwnerError::Invalid);
            }
            // NONBLOCK prevents a raced-in FIFO/device from parking the worker.
            // The descriptor's type, never the earlier directory entry, decides.
            let child = openat(
                directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| OwnerError::Invalid)?;
            let stat = fstat(&child).map_err(|_| OwnerError::Storage)?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => self.visit(&child, &rel)?,
                FileType::RegularFile if stat.st_mode & 0o111 == 0 => {
                    let mut file = fs::File::from(child);
                    let data = read_bounded(&mut file)?;
                    add_file(&mut self.files, &mut self.total, rel, data)?;
                }
                _ => return Err(OwnerError::Invalid),
            }
        }
        Ok(())
    }
}
