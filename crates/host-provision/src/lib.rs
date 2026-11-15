//! Turn a rented Linux server into a paired Praxi host over SSH.
//!
//! Library only: the owner's terminal (`bot host add ...`) and the desktop
//! app's "Add server" wizard (local-owner-only `/v1/hosts/setup` routes, never
//! open to a peer) drive it. There is deliberately no model tool for it.
//!
//! Transport is the system OpenSSH client (key file or ssh-agent; passwords
//! are not supported in this version). The engine only knows the
//! [`Transport`] trait, so tests use a fake and a pure-Rust SSH client could
//! replace OpenSSH later.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::format_push_string,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_panics_doc
)]

pub mod engine;
pub mod facts;
pub mod known_hosts;
pub mod remote;
pub mod result;
pub mod ssh;
pub mod store;
pub mod transport;
pub mod validate;

pub use engine::{Engine, HostHealth, LocalHost, Mode, PairedHost, Params, RemoveLevel, Ui};
pub use known_hosts::{HostKeyState, KnownHosts, ScannedKey};
pub use result::{Report, Status, StepResult};
pub use ssh::{OpenSshTransport, SshPaths, SshTarget};
pub use store::{HostRecord, Store};
pub use transport::{Output, RemoteCmd, StageFile, Sub, Transport, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// User input that failed validation (safe to show).
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod engine_tests;
