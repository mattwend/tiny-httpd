use std::{
    io::ErrorKind,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use clap::Parser;
use telemetry_setup::{TelemetryBuilder, TelemetryError, TelemetryGuard};
use thiserror::Error;
use tiny_httpd::{ServerError, run_with_shutdown};
use tokio::net::TcpListener;
use tracing::{error, warn};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_CONTENT_ROOT: &str = "/app/public";
const DEFAULT_SERVICE_NAME: &str = "tiny-httpd";
const DEFAULT_HEADER_READ_TIMEOUT_SECS: u64 = 30;
const DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS: u64 = 60;
const DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS: u64 = 5;

/// Errors produced while parsing runtime configuration.
#[derive(Debug, Error)]
enum ConfigError {
    /// Clap reported parsing, validation, help, or version output.
    #[error(transparent)]
    Clap(#[from] clap::Error),
}

/// Runtime configuration for the HTTP server.
#[derive(Debug, Clone, Parser)]
#[command(version)]
struct Config {
    /// Bind address.
    #[arg(long, env = "TINY_HTTPD_LISTEN_ADDR", default_value = DEFAULT_LISTEN_ADDR)]
    listen_addr: SocketAddr,
    /// Static content root.
    #[arg(long, env = "TINY_HTTPD_CONTENT_ROOT", default_value = DEFAULT_CONTENT_ROOT)]
    content_root: PathBuf,
    /// Telemetry service name.
    #[arg(long, env = "TINY_HTTPD_SERVICE_NAME", default_value = DEFAULT_SERVICE_NAME)]
    service_name: String,
    /// HTTP/1 header read timeout in seconds.
    #[arg(
        long,
        env = "TINY_HTTPD_HEADER_READ_TIMEOUT_SECS",
        default_value_t = DEFAULT_HEADER_READ_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    header_read_timeout_secs: u64,
    /// Maximum idle time (seconds) before server closes an inactive connection.
    #[arg(
        long,
        env = "TINY_HTTPD_IDLE_CONNECTION_TIMEOUT_SECS",
        default_value_t = DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    idle_connection_timeout_secs: u64,
    /// Maximum graceful-close time (seconds) before a draining connection is dropped.
    #[arg(
        long,
        env = "TINY_HTTPD_GRACEFUL_CLOSE_TIMEOUT_SECS",
        default_value_t = DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    graceful_close_timeout_secs: u64,
}

impl Config {
    /// Parses configuration from CLI arguments and environment variables.
    ///
    /// # Returns
    /// Parsed [`Config`] values.
    ///
    /// # Errors
    /// Returns [`ConfigError`] when clap reports parsing, validation, help, or
    /// version output.
    fn parse() -> Result<Self, ConfigError> {
        <Self as Parser>::try_parse().map_err(Into::into)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
            content_root: PathBuf::from(DEFAULT_CONTENT_ROOT),
            service_name: DEFAULT_SERVICE_NAME.to_string(),
            header_read_timeout_secs: DEFAULT_HEADER_READ_TIMEOUT_SECS,
            idle_connection_timeout_secs: DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS,
            graceful_close_timeout_secs: DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS,
        }
    }
}

/// Errors returned by the telemetry adapter.
#[derive(Debug, Error)]
enum TelemetryInitError {
    /// Shared telemetry setup crate failed during initialization.
    #[error("failed to initialize telemetry: {0}")]
    Init(#[from] TelemetryError),
}

/// Initializes process telemetry through the shared `telemetry-setup` crate.
///
/// # Arguments
/// * `service_name` - OpenTelemetry service name for this process.
///
/// # Returns
/// A telemetry guard that must stay alive until process shutdown.
///
/// # Errors
/// Returns [`TelemetryInitError`] when the shared telemetry setup fails.
fn init_telemetry(service_name: &str) -> Result<TelemetryGuard, TelemetryInitError> {
    TelemetryBuilder::new(service_name)
        .init()
        .map_err(Into::into)
}

/// Waits for process shutdown signal.
///
/// On Unix, resolves on either `SIGTERM` or Ctrl-C (`SIGINT`). On non-Unix
/// platforms, resolves on Ctrl-C only.
async fn shutdown_signal() -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            _ = sigterm.recv() => Ok(()),
            result = tokio::signal::ctrl_c() => result,
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

/// Errors returned during binary startup orchestration.
#[derive(Debug, Error)]
enum StartupError {
    /// Configured content root exists but is not a directory.
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
}

/// Validates the content root and canonicalizes it when available.
///
/// # Arguments
/// * `content_root` - Configured static content root path.
///
/// # Returns
/// Canonical content-root path when the directory exists and is readable,
/// otherwise `None` when the path is missing or unavailable.
///
/// # Errors
/// Returns [`StartupError::ContentRootNotDirectory`] when the path exists but is
/// not a directory, or [`StartupError::ContentRootCanonicalize`] when
/// canonicalization fails after a successful directory metadata check.
async fn prepare_content_root(content_root: &PathBuf) -> Result<Option<PathBuf>, StartupError> {
    match tokio::fs::metadata(content_root).await {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(StartupError::ContentRootNotDirectory(content_root.clone()));
            }

            Ok(Some(tokio::fs::canonicalize(content_root).await.map_err(
                |source| StartupError::ContentRootCanonicalize {
                    path: content_root.clone(),
                    source,
                },
            )?))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            warn!(
                path = %content_root.display(),
                "content root missing; serving embedded default page for /"
            );
            Ok(None)
        }
        Err(error) => {
            warn!(
                error = %error,
                path = %content_root.display(),
                "content root unavailable; serving embedded default page for /"
            );
            Ok(None)
        }
    }
}

/// Binds the server TCP listener.
///
/// # Arguments
/// * `listen_addr` - Socket address the server should bind.
///
/// # Returns
/// A bound [`TcpListener`] ready to accept connections.
///
/// # Errors
/// Returns [`StartupError::Bind`] when the operating system refuses the bind.
async fn bind_listener(listen_addr: SocketAddr) -> Result<TcpListener, StartupError> {
    TcpListener::bind(listen_addr)
        .await
        .map_err(|source| StartupError::Bind {
            addr: listen_addr,
            source,
        })
}

/// Top-level application errors.
#[derive(Debug, Error)]
enum MainError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Telemetry(#[from] TelemetryInitError),
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Server(#[from] ServerError),
}

/// Loads configuration, initializes telemetry, and runs server until shutdown.
#[tokio::main]
async fn main() -> Result<(), MainError> {
    let config = Config::parse()?;
    let mut guard = init_telemetry(&config.service_name)?;
    let content_root = prepare_content_root(&config.content_root).await?;
    let listener = bind_listener(config.listen_addr).await?;

    if let Err(error) = run_with_shutdown(
        listener,
        content_root,
        Duration::from_secs(config.header_read_timeout_secs),
        Duration::from_secs(config.idle_connection_timeout_secs),
        Duration::from_secs(config.graceful_close_timeout_secs),
        shutdown_signal,
    )
    .await
    {
        error!(%error, "server exited with error");
        if let Err(shutdown_error) = guard.shutdown().await {
            warn!(%shutdown_error, "telemetry shutdown failed");
        }
        return Err(error.into());
    }

    if let Err(shutdown_error) = guard.shutdown().await {
        warn!(%shutdown_error, "telemetry shutdown failed");
    }
    Ok(())
}
