//! Where the Composio API key comes from.  Call sites only see [`KeySource`]:
//! today the key is the owner's own, kept in the OS secret store; a future
//! Praxi-operated broker with an anonymous installation token would be another
//! implementation (not built here).

use std::sync::Arc;

use async_trait::async_trait;
use secret_store::SecretStore;

use crate::error::{ComposioError, Result};

/// The one secret this crate owns.  The `composio/` prefix is reserved: no
/// config, tool, bot, peer or state export can reference or read it
/// (`SecretRef::new` refuses it, the secrets route refuses it, bundles skip it).
pub const KEY_SECRET: &str = "composio/api-key";

/// Where an owner who wants their own Composio project creates a project API
/// key (the advanced path; the normal path is the browser login, [`LOGIN_URL`]).
///
/// UNVERIFIED: the docs describe the menu path (Settings, Project Settings or
/// Platform > project > Settings > API Keys) but give no stable deep link; this
/// is the dashboard root, checked against its separate exact-host allowlist.
/// The live spike should replace it with the
/// exact page.
pub const KEY_SETUP_URL: &str = "https://platform.composio.dev/";

/// Where the browser login of Composio starts (the CLI's default web URL);
/// `?cliKey=<session id>` is appended.  Opened in the system browser only.
pub const LOGIN_URL: &str = "https://dashboard.composio.dev/";

/// Stored value prefix of a user key obtained through the browser login.  A
/// value without the prefix is a project API key the owner pasted.
const LOGIN_PREFIX: &str = "user-login:";

/// The stored credential, decoded.
#[derive(Clone, PartialEq, Eq)]
pub enum Stored {
    /// A pasted project API key.
    Project(String),
    /// A user key from the browser login.
    User(String),
}

impl Stored {
    /// Decode the secret-store value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        value.strip_prefix(LOGIN_PREFIX).map_or_else(
            || Self::Project(value.to_owned()),
            |k| Self::User(k.to_owned()),
        )
    }

    /// The secret-store value of a user key.
    #[must_use]
    pub fn encode_user(key: &str) -> String {
        format!("{LOGIN_PREFIX}{key}")
    }
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stored(<key hidden>)")
    }
}

/// Source of the API key.
#[async_trait]
pub trait KeySource: Send + Sync {
    /// The key, if one is available.
    async fn get(&self) -> Result<Option<String>>;
    /// Store a key the owner pasted (local sources only).
    async fn set(&self, key: &str) -> Result<()>;
    /// Forget the key.
    async fn clear(&self) -> Result<()>;
    /// Cheap, non-blocking guess whether a key is available.
    fn present(&self) -> bool;
}

/// The key in the OS secret store under [`KEY_SECRET`].
pub struct LocalKeySource {
    store: Arc<dyn SecretStore>,
}

impl LocalKeySource {
    /// Over `store` (the daemon's registering store, so a stored key is masked
    /// by the redactor at once).
    #[must_use]
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self { store }
    }
}

fn join_err(_: tokio::task::JoinError) -> ComposioError {
    ComposioError::Internal("secret store task failed".into())
}

#[async_trait]
impl KeySource for LocalKeySource {
    async fn get(&self) -> Result<Option<String>> {
        let s = Arc::clone(&self.store);
        let v = tokio::task::spawn_blocking(move || s.get(KEY_SECRET))
            .await
            .map_err(join_err)?
            .map_err(|_| ComposioError::Internal("cannot read the secret store".into()))?;
        Ok(v.map(|v| v.expose().to_owned()))
    }

    async fn set(&self, key: &str) -> Result<()> {
        let s = Arc::clone(&self.store);
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || s.set(KEY_SECRET, &key))
            .await
            .map_err(join_err)?
            .map_err(|_| ComposioError::Internal("cannot store the key".into()))
    }

    async fn clear(&self) -> Result<()> {
        let s = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || s.delete(KEY_SECRET))
            .await
            .map_err(join_err)?
            .map(|_| ())
            .map_err(|_| ComposioError::Internal("cannot delete the key".into()))
    }

    fn present(&self) -> bool {
        matches!(self.store.get(KEY_SECRET), Ok(Some(_)))
    }
}
