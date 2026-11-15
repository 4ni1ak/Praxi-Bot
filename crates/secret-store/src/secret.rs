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

/// A reference to a secret by name. Safe to persist and to show to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SecretRef(String);

impl SecretRef {
    /// Create a reference after validating the name.
    ///
    /// # Errors
    /// [`SecretError::InvalidName`] for empty or malformed names.
    pub fn new(name: impl Into<String>) -> Result<Self, SecretError> {
        let name = name.into();
        validate_name(&name)?;
        if is_host_credential(&name) {
            return Err(SecretError::InvalidName("reserved host credential".into()));
        }
        Ok(Self(name))
    }

    /// The secret name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Host credentials cannot be selected as provider/connector secrets or
/// exported in a portable state bundle. The host accesses them by exact name.
#[must_use]
pub fn is_host_credential(name: &str) -> bool {
    // Windows credential targets are case-insensitive. Reserve every ASCII
    // case spelling on all backends so aliases cannot cross this boundary.
    name.get(.."mcp-server/".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("mcp-server/"))
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
    store
        .get(r.name())?
        .ok_or_else(|| SecretError::NotFound(r.name().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Portable Windows-like credential lookup, including case-variant enumeration.
    // No OS keychain is accessed by this fixture.
    #[derive(Default)]
    struct CaseInsensitiveStore(crate::MemoryStore);

    impl crate::SecretStore for CaseInsensitiveStore {
        fn get(
            &self,
            name: &str,
        ) -> std::result::Result<Option<crate::SecretValue>, crate::SecretError> {
            crate::SecretStore::get(&self.0, &name.to_ascii_lowercase())
        }
        fn set(&self, name: &str, value: &str) -> std::result::Result<(), crate::SecretError> {
            crate::SecretStore::set(&self.0, &name.to_ascii_lowercase(), value)
        }
        fn delete(&self, name: &str) -> std::result::Result<bool, crate::SecretError> {
            crate::SecretStore::delete(&self.0, &name.to_ascii_lowercase())
        }
        fn list_names(&self) -> std::result::Result<Vec<String>, crate::SecretError> {
            crate::SecretStore::list_names(&self.0).map(|names| {
                names
                    .into_iter()
                    .map(|name| name.to_ascii_uppercase())
                    .collect()
            })
        }
    }

    #[test]
    fn host_credentials_cannot_become_provider_or_connector_references() {
        let store = CaseInsensitiveStore::default();
        store.set("mcp-server/token/id", "host value").unwrap();
        store.set("mcp-server/meta/id", "host value").unwrap();
        for name in [
            "mcp-server/token/id",
            "mcp-server/meta/id",
            "MCP-SERVER/TOKEN/ID",
            "MCP-SERVER/META/ID",
            "McP-SeRvEr/ToKeN/Id",
            "McP-SeRvEr/MeTa/Id",
        ] {
            assert_eq!(store.get(name).unwrap().unwrap().expose(), "host value");
            assert!(SecretRef::new(name).is_err());
            assert!(serde_json::from_value::<SecretRef>(serde_json::json!(name)).is_err());
        }
        let ordinary: SecretRef = serde_json::from_str("\"provider/key\"").unwrap();
        assert_eq!(ordinary.name(), "provider/key");
        store.set("provider/key", "ordinary value").unwrap();
        assert_eq!(
            resolve(&store, &ordinary).unwrap().expose(),
            "ordinary value"
        );
    }
}
