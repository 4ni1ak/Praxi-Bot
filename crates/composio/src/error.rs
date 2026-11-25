//! Errors of the Composio integration.  No variant ever carries the API key,
//! a link token or a connection id: server messages pass through the redactor
//! and are truncated before they get here.

use thiserror::Error;

/// Why a Composio operation failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ComposioError {
    /// No API key is stored yet.
    #[error("Composio is not set up: no API key is stored")]
    NotConfigured,
    /// The request is invalid (bad key shape, unknown app, bad arguments).
    #[error("{0}")]
    Invalid(String),
    /// No such app, or the app is not connected.
    #[error("{0}")]
    NotFound(String),
    /// Composio rejected the key (401/403).
    #[error("Composio rejected the API key")]
    Unauthorized,
    /// HTTP 429.
    #[error("Composio rate limit reached{}", retry_suffix(*.retry_after_secs))]
    RateLimited {
        /// `Retry-After` of the response, in seconds.
        retry_after_secs: Option<u64>,
    },
    /// Composio answered with an error.
    #[error("Composio error: {0}")]
    Upstream(String),
    /// The request could not be sent or the response not read.
    #[error(
        "Cannot reach the app service. Check your connection before trying again; an action may already have happened. Details: {0}"
    )]
    Transport(String),
    /// Local failure (state file, secret store).
    #[error("{0}")]
    Internal(String),
}

fn retry_suffix(secs: Option<u64>) -> String {
    secs.map(|s| format!("; retry after {s} s"))
        .unwrap_or_default()
}

/// Result alias.
pub type Result<T, E = ComposioError> = std::result::Result<T, E>;
