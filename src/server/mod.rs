use thiserror::Error;

mod accept;
mod activity_io;

pub use accept::run_with_shutdown;

/// Default graceful-drain time before remaining connection tasks are aborted.
pub const DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 10;

/// Errors returned during server execution.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Generic server I/O failure outside more specific variants.
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
}
