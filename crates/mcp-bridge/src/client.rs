//! The client-side handler of every MCP session.
//!
//! The bridge advertises no client capabilities (no sampling, roots or
//! elicitation), so server-initiated requests keep rmcp's defaults: sampling is
//! `method not found`, roots are empty and elicitation is declined. What the
//! handler adds is reacting to `notifications/tools/list_changed`: the owning
//! server is told to list its tools again, so a tool a server adds, removes or
//! redefines at runtime never stays stale in the registry.

use std::sync::Arc;

use rmcp::{
    ClientHandler, RoleClient,
    service::{NotificationContext, RunningService},
};

/// Signals a session raises for the server that owns it. They outlive a
/// single session: a reconnect keeps the same instance.
#[derive(Default)]
pub(crate) struct Signals {
    /// The server announced that its tool list changed.
    pub(crate) tools_changed: tokio::sync::Notify,
}

/// Handler installed on every session.
#[derive(Clone, Default)]
pub(crate) struct BridgeClient {
    pub(crate) signals: Arc<Signals>,
}

/// A running client session with the bridge's handler.
pub(crate) type Running = RunningService<RoleClient, BridgeClient>;

impl ClientHandler for BridgeClient {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        // `notify_one` keeps one permit, so a notification that arrives while
        // the watcher is busy listing is not lost and a burst collapses.
        self.signals.tools_changed.notify_one();
    }
}
