//! Core types: [`SecretStore`], [`SecretRef`], [`SecretValue`].

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::SecretError;
use crate::wipe::wipe_string;

/// A secret value. `Debug` never prints the content.
///
/// The bytes are wiped when the value is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    /// Wrap a value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Access the raw value. Call only at the provider / MCP boundary.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Zeroize for SecretValue {
    fn zeroize(&mut self) {
        wipe_string(&mut self.0);
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl ZeroizeOnDrop for SecretValue {}

/// Whether `name` is an OAuth record the MCP bridge manages itself
/// (`mcp/<server>/oauth`, `mcp/<server>/oauth-client`, ...): access and
/// refresh tokens and issued client secrets.  Such records are read and
/// written only by the bridge's credential store; no configuration field may
/// reference them ([`SecretRef::new`] and [`resolve`] refuse), so they can
/// never be sent as a header, put into a child's environment or used as a
/// provider key.  Compared case-insensitively (some keychains fold case).
#[must_use]
pub fn is_managed_oauth_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let mut parts = lower.split('/');
    if parts.next() != Some("mcp") {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    rest.len() >= 2 && rest.last().is_some_and(|last| last.starts_with("oauth"))
}

/// A reference to a secret by name. Safe to persist and to show to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(String);

impl SecretRef {
    /// Create a reference after validating the name.
    ///
    /// # Errors
    /// [`SecretError::InvalidName`] for empty or malformed names, and for
    /// managed OAuth records ([`is_managed_oauth_name`]).
    pub fn new(name: impl Into<String>) -> Result<Self, SecretError> {
        let name = name.into();
        validate_name(&name)?;
        if is_managed_oauth_name(&name) {
            return Err(SecretError::InvalidName(name));
        }
        Ok(Self(name))
    }

    /// The secret name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validate a secret name.
///
/// # Errors
/// [`SecretError::InvalidName`] if empty, longer than 128 bytes, starting with
/// the reserved `__` prefix, or containing characters outside `[A-Za-z0-9._:/-]`.
pub fn validate_name(name: &str) -> Result<(), SecretError> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && !name.starts_with("__")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/'));
    if ok {
        Ok(())
    } else {
        Err(SecretError::InvalidName(name.to_string()))
    }
}

/// Key/value secret storage.
pub trait SecretStore: Send + Sync {
    /// Fetch a secret, `None` when absent.
    ///
    /// # Errors
    /// Backend failures.
    fn get(&self, name: &str) -> Result<Option<SecretValue>, SecretError>;

    /// Create or overwrite a secret.
    ///
    /// # Errors
    /// Invalid name or backend failures.
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError>;

    /// Delete a secret; returns whether it existed.
    ///
    /// # Errors
    /// Backend failures.
    fn delete(&self, name: &str) -> Result<bool, SecretError>;

    /// Names of all stored secrets, sorted.
    ///
    /// # Errors
    /// Backend failures.
    fn list_names(&self) -> Result<Vec<String>, SecretError>;
}

/// Resolve a [`SecretRef`] to its value. Call only at the provider / MCP boundary.
///
/// # Errors
/// [`SecretError::NotFound`] if the secret does not exist.
pub fn resolve(store: &dyn SecretStore, r: &SecretRef) -> Result<SecretValue, SecretError> {
    // A deserialized reference skipped `SecretRef::new`: check again here,
    // the one place every configured reference is resolved.
    if is_managed_oauth_name(r.name()) {
        return Err(SecretError::InvalidName(r.name().to_string()));
    }
    store
        .get(r.name())?
        .ok_or_else(|| SecretError::NotFound(r.name().to_string()))
}

#[cfg(test)]
mod managed_oauth_tests {
    use super::*;

    #[test]
    fn managed_oauth_records_can_never_be_referenced_or_resolved() {
        let store = crate::MemoryStore::new();
        store
            .set("mcp/victim/oauth", "{\"access_token\":\"t\"}")
            .unwrap();
        store.set("mcp/victim/oauth-client", "s").unwrap();
        store.set("mcp/victim/key", "plain").unwrap();
        for name in [
            "mcp/victim/oauth",
            "MCP/Victim/OAuth",
            "mcp/victim/oauth-client",
            "mcp//victim/oauth",
        ] {
            assert!(is_managed_oauth_name(name), "{name}");
            assert!(SecretRef::new(name).is_err(), "{name}");
            // A reference that came in through serde skips `new`.
            let smuggled: SecretRef = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert!(resolve(&store, &smuggled).is_err(), "{name}");
        }
        for name in [
            "mcp/victim/key",
            "mcp_victim_oauth",
            "oauth",
            "mcp/oauth",
            "github/oauth",
        ] {
            assert!(!is_managed_oauth_name(name), "{name}");
        }
        assert_eq!(
            resolve(&store, &SecretRef::new("mcp/victim/key").unwrap())
                .unwrap()
                .expose(),
            "plain"
        );
    }
}
