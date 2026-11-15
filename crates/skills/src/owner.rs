//! Local-owner skill management. No tool calls this API and imports never run
//! code. A bounded in-memory snapshot is validated before the scope is locked
//! and published, so readers see a complete old or new document/tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::hub::util::{ensure_deletable, refuse_reparse};
use crate::index::{SKILL_FILE, load_dir_locked};
use crate::scoped_store::{ScopeLock, is_device_name, next_version, with_version};
use crate::{Skill, Source, parse_skill, validate_name};

/// Maximum regular files in an imported skill.
pub const MAX_IMPORT_FILES: usize = 128;
/// Maximum entries, including empty directories, in an import.
pub const MAX_IMPORT_ENTRIES: usize = 256;
/// Maximum total uncompressed bytes (and compressed archive bytes).
pub const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum bytes in a single auxiliary file.
pub const MAX_IMPORT_FILE_BYTES: usize = 1024 * 1024;

/// Errors contain no source text, paths, archive names or parser diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OwnerError {
    /// An invalid document or unsafe filename/tree.
    #[error("invalid skill document or unsafe skill file")]
    Invalid,
    /// An import exceeded a fixed resource budget.
    #[error("skill import exceeds its file, depth or byte limit")]
    TooLarge,
    /// A create/update conflicts with an existing document.
    #[error("skill already exists, has ambiguous copies, or its version changed")]
    Conflict,
    /// No shared skill with that name exists.
    #[error("shared skill not found")]
    NotFound,
    /// An I/O operation failed; do not expose platform error strings.
    #[error("skill storage operation failed")]
    Storage,
}

type Result<T> = std::result::Result<T, OwnerError>;

/// Immutable, bounded snapshot ready for an owner-approved install.
pub struct PreparedSkill {
    skill: Skill,
    files: BTreeMap<String, Vec<u8>>,
}

impl PreparedSkill {
    /// Validate one complete SKILL.md, using the ordinary runtime parser.
    ///
    /// # Errors
    /// Invalid frontmatter, reserved name, empty body or size limit.
    pub fn markdown(content: &str) -> Result<Self> {
        Self::from_files(BTreeMap::from([(
            SKILL_FILE.to_owned(),
            content.as_bytes().to_vec(),
        )]))
    }

    /// Snapshot a local folder without following links or preserving modes.
    ///
    /// # Errors
    /// Unsafe/special/executable files, invalid document or exceeded budgets.
    pub fn folder(root: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            Self::from_files(snapshot::folder(root)?)
        }
        #[cfg(not(unix))]
        {
            plain_ancestors(root)?;
            #[cfg(windows)]
            let _ancestors = windows_ancestors(root)?;
            if !fs::symlink_metadata(root)
                .map_err(|_| OwnerError::Storage)?
                .is_dir()
            {
                return Err(OwnerError::Invalid);
            }
            let mut files = BTreeMap::new();
            let mut seen = BTreeSet::new();
            let mut entries = 0;
            let mut total = 0;
            snapshot_dir(root, "", &mut files, &mut seen, &mut entries, &mut total)?;
            Self::from_files(files)
        }
    }

    /// Decode a ZIP into memory. Nothing is extracted to a caller-chosen path.
    ///
    /// # Errors
    /// Invalid/encrypted ZIP, traversal, duplicate paths, links, executable or
    /// special files, invalid SKILL.md, or any resource limit violation.
    pub fn zip(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(OwnerError::TooLarge);
        }
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| OwnerError::Invalid)?;
        if archive.len() > MAX_IMPORT_ENTRIES {
            return Err(OwnerError::TooLarge);
        }
        let mut files = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut total = 0;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|_| OwnerError::Invalid)?;
            let is_dir = file.is_dir();
            let name = file
                .name()
                .map_err(|_| OwnerError::Invalid)?
                .trim_end_matches('/')
                .to_owned();
            safe_relative(&name)?;
            if !seen.insert(name.to_ascii_lowercase()) {
                return Err(OwnerError::Invalid);
            }
            if let Some(mode) = file.unix_mode() {
                let kind = mode & 0o170_000;
                if (is_dir && !matches!(kind, 0 | 0o040_000))
                    || (!is_dir && (!matches!(kind, 0 | 0o100_000) || mode & 0o111 != 0))
                {
                    return Err(OwnerError::Invalid);
                }
            }
            if !is_dir {
                let data = read_bounded(&mut file)?;
                add_file(&mut files, &mut total, name, data)?;
            }
        }
        Self::from_files(files)
    }

    fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        // Reject file/directory collisions before publication, including their
        // case-insensitive spelling on Windows.
        let folded: BTreeSet<String> = files.keys().map(|s| s.to_ascii_lowercase()).collect();
        for name in &folded {
            let mut parent = Path::new(name).parent();
            while let Some(path) = parent {
                if folded.contains(path.to_str().ok_or(OwnerError::Invalid)?) {
                    return Err(OwnerError::Invalid);
                }
                parent = path.parent();
            }
        }
        let text = std::str::from_utf8(files.get(SKILL_FILE).ok_or(OwnerError::Invalid)?)
            .map_err(|_| OwnerError::Invalid)?;
        let skill = parse_skill(text, Source::User, None).map_err(|_| OwnerError::Invalid)?;
        validate_owner_name(&skill.name)?;
        Ok(Self { skill, files })
    }

    /// Validated name from the document.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.skill.name
    }

    /// Check every imported byte with the daemon's production secret filter.
    /// Binary files are lossily rendered only for matching, never rewritten.
    #[must_use]
    pub fn contains_filtered_content(&self, filter: impl Fn(&str) -> String) -> bool {
        self.files.values().any(|bytes| {
            let text = String::from_utf8_lossy(bytes);
            filter(&text) != text
        })
    }

    /// Number of regular files in the bounded snapshot.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

fn safe_relative(path: &str) -> Result<()> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > 16 || path.len() > 1024 {
        return Err(OwnerError::TooLarge);
    }
    if parts.iter().any(|part| {
        part.is_empty()
            || part.starts_with('.')
            || part.ends_with('.')
            || is_device_name(part)
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    }) {
        return Err(OwnerError::Invalid);
    }
    Ok(())
}

/// Owner-route names additionally reject reserved Windows device names.
///
/// # Errors
/// Invalid name or a platform-reserved name.
pub fn validate_owner_name(name: &str) -> Result<()> {
    validate_name(name).map_err(|_| OwnerError::Invalid)?;
    if is_device_name(name) || name == "import" {
        return Err(OwnerError::Invalid);
    }
    Ok(())
}

fn plain_ancestors(path: &Path) -> Result<()> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(OwnerError::Invalid);
    }
    for ancestor in path.ancestors() {
        refuse_reparse(ancestor).map_err(|_| OwnerError::Invalid)?;
    }
    Ok(())
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_IMPORT_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| OwnerError::Invalid)?;
    if bytes.len() > MAX_IMPORT_FILE_BYTES {
        return Err(OwnerError::TooLarge);
    }
    Ok(bytes)
}

fn add_file(
    files: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
    name: String,
    data: Vec<u8>,
) -> Result<()> {
    *total = total.checked_add(data.len()).ok_or(OwnerError::TooLarge)?;
    if files.len() >= MAX_IMPORT_FILES || *total > MAX_IMPORT_BYTES {
        return Err(OwnerError::TooLarge);
    }
    files.insert(name, data);
    Ok(())
}

#[cfg(not(unix))]
fn snapshot_dir(
    dir: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
    seen: &mut BTreeSet<String>,
    entries: &mut usize,
    total: &mut usize,
) -> Result<()> {
    plain_ancestors(dir)?;
    #[cfg(windows)]
    let _directory = windows_open(dir, true)?;
    for entry in fs::read_dir(dir).map_err(|_| OwnerError::Storage)? {
        *entries += 1;
        if *entries > MAX_IMPORT_ENTRIES {
            return Err(OwnerError::TooLarge);
        }
        let entry = entry.map_err(|_| OwnerError::Storage)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| OwnerError::Invalid)?;
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        safe_relative(&rel)?;
        if !seen.insert(rel.to_ascii_lowercase()) {
            return Err(OwnerError::Invalid);
        }
        let path = entry.path();
        plain_ancestors(&path)?;
        let meta = fs::symlink_metadata(&path).map_err(|_| OwnerError::Storage)?;
        if meta.is_dir() {
            snapshot_dir(&path, &rel, files, seen, entries, total)?;
        } else if meta.is_file() {
            #[cfg(windows)]
            let mut file = windows_open(&path, false)?;
            #[cfg(not(windows))]
            let mut file = fs::File::open(&path).map_err(|_| OwnerError::Storage)?;
            let data = read_bounded(&mut file)?;
            plain_ancestors(&path)?;
            add_file(files, total, rel, data)?;
        } else {
            return Err(OwnerError::Invalid);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_open(path: &Path, directory: bool) -> Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    // Open the reparse point itself, never its target. Omitting WRITE/DELETE
    // sharing pins both contents and directory names while the snapshot runs.
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const SHARE_READ: u32 = 1;
    const REPARSE_POINT: u32 = 0x400;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS)
        .open(path)
        .map_err(|_| OwnerError::Invalid)?;
    let metadata = file.metadata().map_err(|_| OwnerError::Storage)?;
    if metadata.file_attributes() & REPARSE_POINT != 0
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(OwnerError::Invalid);
    }
    Ok(file)
}

#[cfg(windows)]
fn windows_ancestors(path: &Path) -> Result<Vec<fs::File>> {
    if !path.is_absolute() {
        return Err(OwnerError::Invalid);
    }
    let ancestors: Vec<_> = path.ancestors().collect();
    ancestors
        .iter()
        .rev()
        .map(|p| windows_open(p, true))
        .collect()
}

/// Owner mutations of the shared scope. Existing bot write/publication paths
/// keep their own trust semantics and never call these methods.
pub struct OwnerSkillStore {
    root: PathBuf,
}

impl OwnerSkillStore {
    /// Shared skill directory, normally `<data>/skills`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn lock(&self) -> Result<ScopeLock> {
        plain_ancestors(&self.root)?;
        let guard = ScopeLock::acquire(&self.root).map_err(|_| OwnerError::Storage)?;
        recover_published_versions(&self.root)?;
        Ok(guard)
    }

    fn existing(&self, name: &str) -> Result<Option<Skill>> {
        let mut skills = Vec::new();
        let mut issues = Vec::new();
        load_dir_locked(&self.root, Source::User, &mut skills, &mut issues);
        let mut matching = skills.into_iter().filter(|s| s.name == name);
        let found = matching.next();
        if matching.next().is_some() {
            return Err(OwnerError::Conflict);
        }
        if let Some(skill) = &found {
            let dir = skill.dir.as_ref().ok_or(OwnerError::Invalid)?;
            ensure_deletable(&self.root, dir).map_err(|_| OwnerError::Invalid)?;
            plain_ancestors(&dir.join(SKILL_FILE))?;
        }
        Ok(found)
    }

    /// Read shared metadata before checking bot assignments for a mutation.
    ///
    /// # Errors
    /// Invalid, missing or ambiguous name, unsafe storage or failed I/O.
    pub fn get(&self, name: &str) -> Result<Skill> {
        validate_owner_name(name)?;
        plain_ancestors(&self.root)?;
        let _lock = ScopeLock::acquire_read(&self.root).map_err(|_| OwnerError::Storage)?;
        self.existing(name)?.ok_or(OwnerError::NotFound)
    }

    /// Atomically create a complete skill; the same-name race has one winner.
    ///
    /// # Errors
    /// Existing logical name, unsafe target or failed I/O.
    pub fn create(&self, prepared: &PreparedSkill) -> Result<Skill> {
        let _lock = self.lock()?;
        let target = self.root.join(prepared.name());
        if self.existing(prepared.name())?.is_some() || fs::symlink_metadata(&target).is_ok() {
            return Err(OwnerError::Conflict);
        }
        let staged = tempfile::Builder::new()
            .prefix(".praxi-owner-")
            .tempdir_in(&self.root)
            .map_err(|_| OwnerError::Storage)?;
        for (name, bytes) in &prepared.files {
            let path = staged.path().join(name);
            fs::create_dir_all(path.parent().ok_or(OwnerError::Invalid)?)
                .map_err(|_| OwnerError::Storage)?;
            let mut file = fs::File::create(&path).map_err(|_| OwnerError::Storage)?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| OwnerError::Storage)?;
        }
        fs::rename(staged.path(), &target).map_err(|_| OwnerError::Storage)?;
        let mut result = prepared.skill.clone();
        result.dir = Some(target);
        Ok(result)
    }

    /// Replace only SKILL.md atomically, preserving auxiliary files.
    ///
    /// # Errors
    /// Missing/ambiguous name, stale expected version, invalid target or I/O.
    pub fn update(
        &self,
        name: &str,
        prepared: &PreparedSkill,
        expected_version: Option<&str>,
    ) -> Result<Skill> {
        validate_owner_name(name)?;
        if prepared.name() != name || prepared.files.len() != 1 {
            return Err(OwnerError::Invalid);
        }
        let _lock = self.lock()?;
        let old = self.existing(name)?.ok_or(OwnerError::NotFound)?;
        if expected_version.is_some_and(|v| v != old.version) {
            return Err(OwnerError::Conflict);
        }
        let dir = old.dir.ok_or(OwnerError::Invalid)?;
        let version = next_version(&old.version).map_err(|_| OwnerError::Invalid)?;
        let text =
            std::str::from_utf8(&prepared.files[SKILL_FILE]).map_err(|_| OwnerError::Invalid)?;
        let text = with_version(text, &version).map_err(|_| OwnerError::Invalid)?;
        let mut updated =
            parse_skill(&text, Source::User, Some(dir.clone())).map_err(|_| OwnerError::Invalid)?;
        // Published directories encode their version in the folder name.
        // Keeping the old suffix would make a later bot publication skip this
        // version and leave two conflicting copies of the same logical skill.
        let published_target = if dir.file_name().and_then(|n| n.to_str())
            == Some(&format!("{name}@{}", old.version))
        {
            let target = self.root.join(format!("{name}@{version}"));
            version::require_absent(&target)?;
            version::begin(&dir, &old.version, &version)?;
            Some(target)
        } else {
            None
        };
        let mut staged = tempfile::Builder::new()
            .prefix(".praxi-owner-")
            .tempfile_in(&dir)
            .map_err(|_| OwnerError::Storage)?;
        staged
            .write_all(text.as_bytes())
            .map_err(|_| OwnerError::Storage)?;
        staged
            .as_file()
            .sync_all()
            .map_err(|_| OwnerError::Storage)?;
        staged
            .persist(dir.join(SKILL_FILE))
            .map_err(|_| OwnerError::Storage)?;
        if let Some(target) = published_target {
            // Both the old and new names are visible index entries. A crash
            // before this atomic directory rename leaves the complete new
            // document readable at the old name; the marker permits repair.
            fs::rename(&dir, &target).map_err(|_| OwnerError::Storage)?;
            version::finish(&target)?;
            updated.dir = Some(target);
        }
        Ok(updated)
    }

    /// Atomically remove a shared skill from the readable scope.
    ///
    /// # Errors
    /// Missing/ambiguous name, unsafe target or failed I/O.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_owner_name(name)?;
        let _lock = self.lock()?;
        let old = self.existing(name)?.ok_or(OwnerError::NotFound)?;
        let dir = old.dir.ok_or(OwnerError::Invalid)?;
        let trash = tempfile::Builder::new()
            .prefix(".praxi-owner-delete-")
            .tempdir_in(&self.root)
            .map_err(|_| OwnerError::Storage)?;
        fs::rename(dir, trash.path().join("skill")).map_err(|_| OwnerError::Storage)?;
        trash.close().map_err(|_| OwnerError::Storage)
    }
}

// Only called while the shared-scope exclusive lock is held by an already
// audited owner mutation or bot publication. Read-only loading never repairs.
pub(crate) fn recover_published_versions(root: &Path) -> Result<()> {
    version::recover(root)
}

mod version;

#[cfg(test)]
mod tests;

#[cfg(unix)]
mod snapshot;
