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

/// Where the owner creates the project API key (Composio dashboard; sign in,
/// Settings, API Keys).
///
/// UNVERIFIED: the docs describe the menu path (Settings, Project Settings or
/// Platform > project > Settings > API Keys) but give no stable deep link; this
/// is the dashboard root, checked against its separate exact-host allowlist.
/// The live spike should replace it with the
/// exact page.
pub const KEY_SETUP_URL: &str = "https://platform.composio.dev/";

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
