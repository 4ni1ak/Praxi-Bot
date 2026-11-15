//! Non-secret local records: `<data>/hosts/<name>/host.json` and the log.
//! Never stored: passwords, passphrases, key bytes, tickets.
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    pub name: String,
    pub user: String,
    pub host: String,
    pub port: u16,
    /// Path of the private key file (the path only), if one was given.
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    /// Device key of the host as paired on this computer.
    #[serde(default)]
    pub paired_host_id: Option<String>,
    /// This computer's device key as the host knows it.
    #[serde(default)]
    pub desktop_key: Option<String>,
    #[serde(default)]
    pub installed_version: Option<String>,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// `root` is `<praxi data dir>/hosts`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_data_dir(data: &Path) -> Self {
        Self::new(data.join("hosts"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn known_hosts_path(&self) -> PathBuf {
        self.root.join("known_hosts")
    }

    pub fn ssh_config_path(&self) -> PathBuf {
        self.root.join("ssh_config")
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn log_path(&self, name: &str) -> PathBuf {
        self.dir(name).join("provision.log")
    }

    pub fn load(&self, name: &str) -> Option<HostRecord> {
        let text = fs::read_to_string(self.dir(name).join("host.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, rec: &HostRecord) -> Result<(), Error> {
        let dir = self.dir(&rec.name);
        fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(rec).map_err(|e| Error::Failed(e.to_string()))?;
        fs::write(dir.join("host.json"), text)?;
        Ok(())
    }

    /// Delete the record; the log stays for a post-mortem.
    pub fn delete(&self, name: &str) -> Result<(), Error> {
        match fs::remove_file(self.dir(name).join("host.json")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn list(&self) -> Vec<HostRecord> {
        let Ok(rd) = fs::read_dir(&self.root) else { return Vec::new() };
        let mut v: Vec<HostRecord> = rd
            .flatten()
            .filter_map(|e| self.load(&e.file_name().to_string_lossy()))
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Find a saved record by name or by `user@host`.
    pub fn find(&self, spec: &str) -> Option<HostRecord> {
        self.load(spec).or_else(|| {
            let (user, host) = spec.split_once('@')?;
            self.list().into_iter().find(|r| r.user == user && r.host == host)
        })
    }
}
