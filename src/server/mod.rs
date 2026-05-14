use thiserror::Error;

mod accept;
mod activity_io;

pub use accept::run_with_shutdown;

/// Maximum graceful-drain time before remaining connection tasks are aborted.
const DRAIN_TIMEOUT_SECS: u64 = 10;
/// Time to keep listener accepting only readiness observations after shutdown starts.
const READINESS_DRAIN_WINDOW_MILLIS: u64 = 250;

/// Errors returned during server execution.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Generic server I/O failure outside more specific variants.
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
}
