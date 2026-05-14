use std::{net::SocketAddr, path::PathBuf};

use thiserror::Error;

mod accept;

pub use accept::run_with_shutdown;

/// Maximum graceful-drain time before remaining connection tasks are aborted.
const DRAIN_TIMEOUT_SECS: u64 = 10;

/// Errors returned during server execution or binary-side startup orchestration.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Configured content root exists but is not directory.
    #[error("content root `{0}` is not a directory")]
    ContentRootNotDirectory(PathBuf),
    /// Canonicalization of configured content root failed.
    #[error("failed to canonicalize content root `{path}`: {source}")]
    ContentRootCanonicalize {
        /// Original configured content-root path.
        path: PathBuf,
        /// Filesystem error from canonicalization.
        #[source]
        source: std::io::Error,
    },
    /// TCP listener bind failed for configured address.
    #[error("failed to bind listener on `{addr}`: {source}")]
    Bind {
        /// Socket address server attempted to bind.
        addr: SocketAddr,
        /// OS error returned by bind operation.
        #[source]
        source: std::io::Error,
    },
    /// Generic server I/O failure outside more specific variants.
    #[error("server I/O error: {0}")]
    Io(#[from] std::io::Error),
}
