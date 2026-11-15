//! `composio.toml`: non-secret local state.  No connection ids, no tokens, no
//! key: only our opaque user id and which apps the owner switched on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ComposioError, Result};

/// File name inside the data directory.
pub const FILE_NAME: &str = "composio.toml";

/// Per-app local settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    /// The owner opted in (the Connect click) and tools may exist.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Bots that may use the app; absent = every bot, `[]` = no bot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bots: Option<Vec<String>>,
}

fn yes() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bots: None,
        }
    }
}

/// Contents of `composio.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFile {
    /// Opaque end-user id sent to Composio (no personal data).
    #[serde(default)]
    pub user_id: String,
    /// Apps the owner connected or started to connect, by toolkit slug.
    #[serde(default)]
    pub apps: BTreeMap<String, AppConfig>,
}

impl StateFile {
    /// Load, or start empty when the file does not exist.  A user id is
    /// generated when missing (saved on the next [`Self::save`]).
    ///
    /// # Errors
    /// The file exists but cannot be parsed (it is never overwritten then).
    pub fn load(path: &Path) -> Result<Self> {
        let mut s = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map_err(|e| ComposioError::Internal(format!("{FILE_NAME}: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(ComposioError::Internal(format!("{FILE_NAME}: {e}"))),
        };
        if s.user_id.is_empty() {
            s.user_id = format!("praxi-{}", uuid::Uuid::new_v4());
        }
        Ok(s)
    }

    /// Write atomically (temp file + rename).
    ///
    /// # Errors
    /// I/O errors.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| ComposioError::Internal(format!("{FILE_NAME}: {e}")))?;
        let tmp: PathBuf = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| ComposioError::Internal(format!("{FILE_NAME}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_generates_a_stable_opaque_user_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut s = StateFile::load(&path).unwrap();
        assert!(s.user_id.starts_with("praxi-"));
        s.apps.insert("slack".into(), AppConfig::default());
        s.save(&path).unwrap();
        let again = StateFile::load(&path).unwrap();
        assert_eq!(again, s);
    }

    #[test]
    fn a_broken_file_is_an_error_not_a_silent_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "this is [not toml").unwrap();
        assert!(StateFile::load(&path).is_err());
    }
}
