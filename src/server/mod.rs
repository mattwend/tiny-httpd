use std::{
    io::ErrorKind,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use thiserror::Error;
use tokio::net::TcpListener;

use crate::config::Config;

mod accept;
mod signal;

pub use accept::{run, run_with_shutdown};

/// Maximum graceful-drain time before remaining connection tasks are aborted.
const DRAIN_TIMEOUT_SECS: u64 = 10;

/// Validated startup state returned by [`startup`] and consumed by [`run`] or
/// [`run_with_shutdown`].
#[must_use]
pub struct Startup {
    /// Bound TCP listener ready to accept connections.
    pub listener: TcpListener,
    /// Canonical content-root path validated during startup when available.
    pub(crate) content_root: Option<PathBuf>,
    /// Maximum time allowed to receive complete HTTP/1 request headers.
    pub(crate) header_read_timeout: std::time::Duration,
    /// Maximum idle time before server closes an inactive connection.
    pub(crate) idle_connection_timeout: std::time::Duration,
    /// Maximum graceful-close time before a draining connection is dropped.
    pub(crate) graceful_close_timeout: std::time::Duration,
}

impl Startup {
    /// Returns canonical content-root path when startup validated one.
    pub fn content_root(&self) -> Option<&Path> {
        self.content_root.as_deref()
    }
}

/// Errors returned during startup validation or server execution.
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

/// Validates the content root and binds the configured listener.
///
/// # Arguments
/// * `config` - Runtime configuration.
///
/// # Returns
/// A [`Startup`] value containing the bound listener and canonical content root
/// when one exists and is usable.
///
/// # Errors
/// Returns [`ServerError`] if an existing content root is not a directory,
/// cannot be canonicalized, or if binding the listener fails.
pub async fn startup(config: &Config) -> Result<Startup, ServerError> {
    let content_root = match tokio::fs::metadata(&config.content_root).await {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(ServerError::ContentRootNotDirectory(
                    config.content_root.clone(),
                ));
            }

            Some(
                tokio::fs::canonicalize(&config.content_root)
                    .await
                    .map_err(|source| ServerError::ContentRootCanonicalize {
                        path: config.content_root.clone(),
                        source,
                    })?,
            )
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            tracing::warn!(
                path = %config.content_root.display(),
                "content root missing; serving embedded default page for /"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %config.content_root.display(),
                "content root unavailable; serving embedded default page for /"
            );
            None
        }
    };

    let listener = TcpListener::bind(config.listen_addr)
        .await
        .map_err(|source| ServerError::Bind {
            addr: config.listen_addr,
            source,
        })?;

    Ok(Startup {
        listener,
        content_root,
        header_read_timeout: std::time::Duration::from_secs(config.header_read_timeout_secs),
        idle_connection_timeout: std::time::Duration::from_secs(
            config.idle_connection_timeout_secs,
        ),
        graceful_close_timeout: std::time::Duration::from_secs(config.graceful_close_timeout_secs),
    })
}
