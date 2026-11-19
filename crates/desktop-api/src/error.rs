//! Error type for desktop operations.

use thiserror::Error;

/// Errors from OS desktop operations.
#[derive(Debug, Error)]
pub enum DesktopError {
    /// The target element could not be found (stale index, different generation).
    #[error("element not found: index={index} generation={generation}")]
    ElementNotFound { index: u32, generation: u64 },

    /// The accessibility API returned an unexpected response.
    #[error("accessibility error: {0}")]
    Accessibility(String),

    /// A screenshot could not be captured.
    #[error("screenshot error: {0}")]
    Screenshot(String),

    /// The requested operation is not supported on this platform.
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),

    /// The window that would receive input was not the program the call was
    /// approved for, or could not be checked, or a guarded send stopped. The
    /// text says truthfully what was and was not delivered; it is never
    /// prefixed here (a partial send is not "not sent").
    #[error("{0}")]
    GuardRefused(String),

    /// An unrecoverable internal error.
    #[error("internal error: {0}")]
    Internal(String),
}
