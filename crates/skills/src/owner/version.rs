//! Complete an interrupted owner update without ever hiding the installed
//! skill. Markers contain versions only, never caller-supplied paths.

use std::fs;
use std::io::Write;
use std::path::Path;

use super::{OwnerError, Result, plain_ancestors, read_bounded, validate_owner_name};
use crate::hub::util::ensure_deletable;
use crate::index::SKILL_FILE;
use crate::scoped_store::next_version;
use crate::{Source, parse_skill};

pub(super) const MARKER: &str = ".praxi-owner-version.json";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    old: String,
    next: String,
}

pub(super) fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(OwnerError::Conflict),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(OwnerError::Storage),
    }
}

pub(super) fn begin(dir: &Path, old: &str, next: &str) -> Result<()> {
    if next_version(old).map_err(|_| OwnerError::Invalid)? != next {
        return Err(OwnerError::Invalid);
    }
    require_absent(&dir.join(MARKER))?;
    let bytes = serde_json::to_vec(&Transition {
        old: old.into(),
        next: next.into(),
    })
    .map_err(|_| OwnerError::Invalid)?;
    let mut marker = tempfile::Builder::new()
        .prefix(".praxi-owner-marker-")
        .tempfile_in(dir)
        .map_err(|_| OwnerError::Storage)?;
    marker.write_all(&bytes).map_err(|_| OwnerError::Storage)?;
    marker
        .as_file()
        .sync_all()
        .map_err(|_| OwnerError::Storage)?;
    marker
        .persist(dir.join(MARKER))
        .map_err(|_| OwnerError::Storage)?;
    Ok(())
}

pub(super) fn finish(dir: &Path) -> Result<()> {
    fs::remove_file(dir.join(MARKER)).map_err(|_| OwnerError::Storage)
}

pub(super) fn recover(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(|_| OwnerError::Storage)? {
        let entry = entry.map_err(|_| OwnerError::Storage)?;
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        let Some((name, folder_version)) = filename.split_once('@') else {
            continue;
        };
        let dir = entry.path();
        let marker = dir.join(MARKER);
        let metadata = match fs::symlink_metadata(&marker) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(OwnerError::Storage),
        };
        validate_owner_name(name)?;
        ensure_deletable(root, &dir).map_err(|_| OwnerError::Invalid)?;
        plain_ancestors(&marker)?;
        if !metadata.is_file() || metadata.len() > 256 {
            return Err(OwnerError::Invalid);
        }
        let bytes = read_bounded(&mut fs::File::open(&marker).map_err(|_| OwnerError::Storage)?)?;
        if bytes.len() > 256 {
            return Err(OwnerError::Invalid);
        }
        let transition: Transition =
            serde_json::from_slice(&bytes).map_err(|_| OwnerError::Invalid)?;
        if next_version(&transition.old).map_err(|_| OwnerError::Invalid)? != transition.next {
            return Err(OwnerError::Invalid);
        }
        let file = dir.join(SKILL_FILE);
        plain_ancestors(&file)?;
        let bytes = read_bounded(&mut fs::File::open(&file).map_err(|_| OwnerError::Storage)?)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| OwnerError::Invalid)?;
        let skill = parse_skill(text, Source::User, None).map_err(|_| OwnerError::Invalid)?;
        if skill.name != name {
            return Err(OwnerError::Invalid);
        }
        match (folder_version, skill.version.as_str()) {
            (folder, document) if folder == transition.old && document == transition.old => {
                // The process stopped before replacing SKILL.md.
                finish(&dir)?;
            }
            (folder, document) if folder == transition.next && document == transition.next => {
                // The rename committed; only marker cleanup was interrupted.
                finish(&dir)?;
            }
            (folder, document) if folder == transition.old && document == transition.next => {
                let target = root.join(format!("{name}@{}", transition.next));
                require_absent(&target)?;
                fs::rename(&dir, &target).map_err(|_| OwnerError::Storage)?;
                finish(&target)?;
            }
            _ => return Err(OwnerError::Invalid),
        }
    }
    Ok(())
}
