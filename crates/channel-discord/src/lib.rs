//! Discord channel adapter (issue #207, ADR-0019).
//!
//! Lets the owner talk to their bots from Discord: in a private chat with the
//! bot, or in a server channel they allowed. Built on `channel-api`:
//!
//! - [`transport::DiscordTransport`] implements [`channel_api::Channel`]: the
//!   gateway (websocket) delivers messages and button presses, REST sends.
//! - [`bridge::DiscordChannel`] adds pairing, the owner-only rules, run
//!   following and approval cards on top.
//!
//! Security: one paired owner account; strangers never reach the daemon;
//! approval buttons are "approve once" / "deny" for low-risk requests only;
//! outgoing text is redacted, then mention-escaped; attachments are only
//! described (untrusted), never downloaded; the bot token lives in the secret
//! store.

pub mod bridge;
pub mod error;
pub mod gateway;
pub mod messages;
pub mod rest;
pub mod state;
pub mod text;
pub mod transport;

#[cfg(test)]
mod tests;

pub use bridge::{DISCORD_BOT_TOKEN_SECRET, DiscordChannel, DiscordConfig, store_bot_token};
pub use error::{DiscordError, Result};
pub use messages::Lang;
pub use rest::{DEFAULT_API_URL, DiscordRest};
pub use state::DiscordState;
