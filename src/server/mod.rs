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

const DRAIN_TIMEOUT_SECS: u64 = 10;

/// Validated startup state returned by [`startup`] and consumed by [`run`] or
/// [`run_with_shutdown`].
pub struct Startup {
    /// Bound TCP listener ready to accept connections.
    pub listener: TcpListener,
    /// Canonical content-root path validated during startup when available.
    pub(crate) content_root: Option<PathBuf>,
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
    #[error("content root `{0}` is not a directory")]
    ContentRootNotDirectory(PathBuf),
    #[error("failed to canonicalize content root `{path}`: {source}")]
    ContentRootCanonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to bind listener on `{addr}`: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
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
    })
}
