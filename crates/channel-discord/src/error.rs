//! Errors of the Discord adapter.

use thiserror::Error;

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, DiscordError>;

/// Everything that can go wrong inside the adapter.
#[derive(Debug, Error)]
pub enum DiscordError {
    /// Bad or missing configuration (token, address).
    #[error("{0}")]
    Config(String),
    /// Discord answered with an error.
    #[error("Discord answered {status}: {message}")]
    Api {
        /// HTTP status.
        status: u16,
        /// Discord's own message (never contains our token).
        message: String,
    },
    /// Discord asked us to slow down.
    #[error("Discord asked to wait {retry_after_ms} ms")]
    RateLimited {
        /// Time to wait, in milliseconds.
        retry_after_ms: u64,
    },
    /// The network or the gateway connection failed.
    #[error("network error: {0}")]
    Network(String),
    /// The gateway refused us for good (bad token, missing permission).
    #[error("{0}")]
    GatewayFatal(String),
    /// The bot's own daemon refused or failed.
    #[error("daemon: {0}")]
    Daemon(String),
    /// State file problem.
    #[error("state file: {0}")]
    State(String),
    /// Secret store problem.
    #[error("secret store: {0}")]
    Secret(String),
}

impl From<DiscordError> for channel_api::ChannelError {
    fn from(e: DiscordError) -> Self {
        Self::Adapter(Box::new(e))
    }
}

impl From<channel_telegram::TelegramError> for DiscordError {
    fn from(e: channel_telegram::TelegramError) -> Self {
        Self::Daemon(e.to_string())
    }
}

impl From<secret_store::SecretError> for DiscordError {
    fn from(e: secret_store::SecretError) -> Self {
        Self::Secret(e.to_string())
    }
}

impl From<channel_api::ChannelError> for DiscordError {
    fn from(e: channel_api::ChannelError) -> Self {
        Self::Network(e.to_string())
    }
}
