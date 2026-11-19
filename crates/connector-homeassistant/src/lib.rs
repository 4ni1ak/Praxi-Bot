//! Home Assistant connector as agent tools.
//!
//! - [`tools::LIST`] and [`tools::STATE`] read entity states (`Effect::Read`;
//!   names and attributes are user-editable text and are marked untrusted).
//! - [`tools::CALL`] calls a service such as `light.turn_on`. It acts on the
//!   physical world, so it is `Effect::Destructive` (always asks), and a
//!   fixed deny list refuses services that run code or stop the hub.
//! - The long-lived access token is a [`secret_store::SecretRef`]. The URL must
//!   be `https`, or plain `http` for a private/loopback address (Home Assistant
//!   on a home network usually has no certificate); redirects are refused.

pub mod client;
pub mod tools;

use std::sync::Arc;

use secret_store::SecretStore;
use tool_registry::ToolRegistry;

pub use client::{HaError, HomeAssistantClient, HomeAssistantConfig};
pub use tools::TOOL_NAMES;

/// Build the client and register the tools (they stay hidden while the
/// configuration is unusable). Returns the number of tools added and, when the
/// configuration was refused, why.
#[must_use]
pub fn register(
    registry: &ToolRegistry,
    config: Option<HomeAssistantConfig>,
    store: &Arc<dyn SecretStore>,
) -> (usize, Option<String>) {
    let (client, problem) = match config.map(|c| HomeAssistantClient::new(c, Arc::clone(store))) {
        Some(Ok(c)) => (Some(Arc::new(c)), None),
        Some(Err(e)) => (None, Some(e.to_string())),
        None => (None, None),
    };
    let added = tools::tools(client.as_ref())
        .into_iter()
        .filter(|t| registry.register(t.clone()))
        .count();
    (added, problem)
}

#[cfg(test)]
mod tests;
