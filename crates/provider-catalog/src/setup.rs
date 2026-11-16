//! Adding a provider to `providers.toml` together with its API key.
//!
//! One implementation for every caller: `bot providers add`, the setup wizard
//! and the daemon's host-administration route. The key goes straight to the
//! secret store; `providers.toml` only ever holds the secret's *name*.

use std::path::Path;

use secret_store::{SecretRef, SecretStore};

use crate::{
    Auth, Catalog, CompatFlags, ConfigError, ModelCapabilities, ProviderConfig, ProviderEntry,
    ProviderKind, ProviderSettings, api_key_secret_name, config_path,
};

/// Which provider was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderChoice {
    /// A catalog entry by id.
    Catalog(String),
    /// A custom OpenAI-compatible endpoint.
    Custom {
        /// Id to register it under.
        id: String,
        /// Base URL (e.g. `http://localhost:8000/v1`).
        base_url: String,
    },
}

/// Why a provider could not be configured.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// The id is not in the catalog and no base URL was given.
    #[error("unknown provider {0:?}; give a base URL for a custom one")]
    Unknown(String),
    /// The secret store refused the key.
    #[error("secret store: {0}")]
    Secret(#[from] secret_store::SecretError),
    /// `providers.toml` could not be read or written.
    #[error("{0}")]
    Config(#[from] ConfigError),
    /// The provider vanished between save and read-back.
    #[error("provider vanished after save")]
    Vanished,
}

/// Entry for a custom OpenAI-compatible endpoint.
#[must_use]
pub fn custom_entry(id: &str, base_url: &str) -> ProviderEntry {
    let base = base_url.trim_end_matches('/').to_string();
    ProviderEntry {
        id: id.to_string(),
        kind: ProviderKind::OpenAiCompatible,
        models_endpoint: Some(format!("{base}/models")),
        models: Vec::new(),
        base_url: base,
        auth: Auth::None,
        capabilities: ModelCapabilities {
            tools: true,
            vision: false,
            streaming: true,
            json_mode: false,
        },
        compat_flags: CompatFlags::default(),
    }
}

/// Resolve the catalog entry for a choice.
///
/// # Errors
/// Unknown catalog id.
pub fn entry_for(choice: &ProviderChoice, catalog: &Catalog) -> Result<ProviderEntry, SetupError> {
    match choice {
        ProviderChoice::Catalog(id) => catalog
            .get(id)
            .cloned()
            .ok_or_else(|| SetupError::Unknown(id.clone())),
        ProviderChoice::Custom { id, base_url } => Ok(custom_entry(id, base_url)),
    }
}

/// Store the key, register the provider in `providers.toml`, set its model.
/// Re-adding without a new key or model keeps what was there.
///
/// # Errors
/// Unknown catalog id, secret store or config errors.
pub fn configure_provider(
    catalog: &Catalog,
    store: &dyn SecretStore,
    data_dir: &Path,
    choice: &ProviderChoice,
    key: Option<&str>,
    model: Option<&str>,
    now_ms: i64,
) -> Result<(ProviderSettings, ProviderConfig), SetupError> {
    let entry = entry_for(choice, catalog)?;
    let secret_ref = if let Some(k) = key {
        let name = api_key_secret_name(&entry.id);
        let secret_ref = SecretRef::new(name.clone())?;
        // Write to the secret store before taking providers.lock so an
        // interactive unlock prompt never stalls concurrent config writers.
        store.set(&name, k)?;
        Some(secret_ref)
    } else {
        None
    };
    let path = config_path(data_dir);
    let _guard = ProviderConfig::lock(&path)?;
    let mut cfg = ProviderConfig::load(&path)?;
    let mut s = ProviderSettings::new(&entry.id, now_ms);
    s.key = secret_ref;
    if matches!(choice, ProviderChoice::Custom { .. }) {
        s.custom = Some(entry);
    }
    s.default_model = model.map(str::to_string);
    let id = s.id.clone();
    if let Some(old) = cfg.get(&id) {
        if s.key.is_none() {
            s.key.clone_from(&old.key);
        }
        if s.default_model.is_none() {
            s.default_model.clone_from(&old.default_model);
        }
        if s.model_caps.is_empty() {
            s.model_caps.clone_from(&old.model_caps);
        }
    }
    cfg.upsert(s);
    cfg.save(&path)?;
    let settings = cfg.get(&id).cloned().ok_or(SetupError::Vanished)?;
    Ok((settings, cfg))
}
