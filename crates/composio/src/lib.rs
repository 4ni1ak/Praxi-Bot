//! Composio app connections (card 245).
//!
//! Praxi connects the services that are painful to connect itself through
//! Composio's REST API: one API key (the owner's own, in the OS secret store),
//! Composio-hosted consent pages, an open catalog, and per-app tools that are
//! discovered, capped and always asked about.  Nothing here follows a URL from
//! a response, and nothing exposes Composio's meta, bash, workbench or
//! connect-link tools to the model.

mod client;
mod curated;
mod error;
mod keys;
mod schema;
mod service;
mod state;
mod tool;

pub use client::{
    API_BASE, API_ORIGIN, Account, ComposioClient, Link, Redact, RemoteTool, Toolkit, ToolkitPage,
    is_composio_https_url,
};
pub use curated::{Curated, Popular, display_class, short_name};
pub use error::{ComposioError, Result};
pub use keys::{KEY_SECRET, KEY_SETUP_URL, KeySource, LocalKeySource};
pub use schema::{clean_text, sanitize, validate};
pub use service::{ActionInfo, AppView, Audit, ComposioService, ConnState, ServiceConfig};
pub use state::{AppConfig, FILE_NAME, StateFile};

#[cfg(test)]
mod tests;
