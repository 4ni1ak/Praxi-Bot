//! Native code paths are installation authority, never portable bot settings.

use std::io::Read;

use memory_store::knowledge::embedding::LocalEmbeddingArtifacts;
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState, BTreeMap, EmbeddingProfile};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostFile {
    profiles: BTreeMap<String, HostProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostProfile {
    label: String,
    artifacts: LocalEmbeddingArtifacts,
}

/// The browser receives identities and labels, never executable file paths.
#[derive(Debug, Clone, Serialize)]
pub struct HostLocalProfile {
    pub id: String,
    pub label: String,
    pub profile: EmbeddingProfile,
}

fn configured(state: &AppState) -> Result<BTreeMap<String, HostProfile>, ApiError> {
    let path = state.data_dir().join("knowledge-host.json");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
                .map_err(|_| ApiError::unavailable("host file flags unavailable"))?,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(1); // OPEN_REPARSE_POINT, read sharing only.
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => {
            return Err(ApiError::forbidden(
                "host embedding profiles cannot be opened safely",
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|_| ApiError::unavailable("host embedding profiles unavailable"))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x0400 != 0 {
            return Err(ApiError::forbidden(
                "host embedding profiles cannot be a reparse point",
            ));
        }
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1_048_576 {
        return Err(ApiError::forbidden(
            "host embedding profiles require a bounded regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o022 != 0 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(ApiError::forbidden(
                "host embedding profiles must be owned and writable only by the daemon user",
            ));
        }
    }
    let mut contents = Vec::new();
    file.take(1_048_577)
        .read_to_end(&mut contents)
        .map_err(|_| ApiError::unavailable("host embedding profiles unavailable"))?;
    if contents.len() > 1_048_576 {
        return Err(ApiError::bad_request("host embedding profiles too large"));
    }
    let file: HostFile = serde_json::from_slice(&contents)
        .map_err(|_| ApiError::bad_request("invalid host embedding profiles"))?;
    if file.profiles.len() > 16
        || file.profiles.iter().any(|(id, profile)| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                || profile.label.trim().is_empty()
                || profile.label.len() > 256
                || profile.label.chars().any(char::is_control)
        })
    {
        return Err(ApiError::bad_request(
            "invalid host embedding profile identity",
        ));
    }
    Ok(file.profiles)
}

/// List operator-installed local profiles. This does not load native code.
///
/// # Errors
/// Untrusted file permissions, malformed configuration, or invalid pins.
pub fn list_local_profiles(state: &AppState) -> Result<Vec<HostLocalProfile>, ApiError> {
    configured(state)?
        .into_iter()
        .map(|(id, entry)| {
            Ok(HostLocalProfile {
                id,
                label: entry.label,
                profile: entry.artifacts.profile().map_err(|_| {
                    ApiError::bad_request("invalid host embedding artifact identity")
                })?,
            })
        })
        .collect()
}

/// Resolve an installed profile ID to its immutable identity.
///
/// # Errors
/// Unknown/untrusted host profile or malformed pins.
pub fn resolve_local_profile(state: &AppState, id: &str) -> Result<EmbeddingProfile, ApiError> {
    artifacts(state, id)?
        .profile()
        .map_err(|_| ApiError::bad_request("invalid host embedding artifact identity"))
}

pub(super) fn artifacts(state: &AppState, id: &str) -> Result<LocalEmbeddingArtifacts, ApiError> {
    configured(state)?
        .remove(id)
        .map(|entry| entry.artifacts)
        .ok_or_else(|| {
            ApiError::bad_request("local embedding profile is not installed on this host")
        })
}
