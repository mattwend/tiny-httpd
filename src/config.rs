use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use clap::Parser;
use thiserror::Error;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_CONTENT_ROOT: &str = "/app/public";
const DEFAULT_SERVICE_NAME: &str = "tiny-httpd";
const DEFAULT_HEADER_READ_TIMEOUT_SECS: u64 = 30;
const DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS: u64 = 60;
const DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS: u64 = 5;

/// Runtime configuration loaded from command-line flags and environment variables.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Socket address server binds during startup.
    pub listen_addr: SocketAddr,
    /// Filesystem root used for static file lookup.
    pub content_root: PathBuf,
    /// Service name reported through telemetry.
    pub service_name: String,
    /// Maximum time allowed to receive complete HTTP/1 request headers.
    pub header_read_timeout_secs: u64,
    /// Maximum idle time before server closes an inactive connection.
    pub idle_connection_timeout_secs: u64,
    /// Maximum graceful-close time before a draining connection is dropped.
    pub graceful_close_timeout_secs: u64,
}

/// Errors produced while parsing runtime configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Clap reported parsing, validation, help, or version output.
    #[error(transparent)]
    Clap(#[from] clap::Error),
}

#[derive(Debug, Clone, Parser)]
#[command(version)]
struct CliArgs {
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
    /// Loads runtime configuration from command-line flags and environment
    /// variables.
    ///
    /// Uses clap's standard precedence: command-line flags override
    /// environment variables, which override built-in defaults.
    ///
    /// # Supported flags / environment variables
    ///
    /// | Flag | Env var | Default |
    /// |---|---|---|
    /// | `--listen-addr` | `TINY_HTTPD_LISTEN_ADDR` | `0.0.0.0:8080` |
    /// | `--content-root` | `TINY_HTTPD_CONTENT_ROOT` | `/app/public` |
    /// | `--service-name` | `TINY_HTTPD_SERVICE_NAME` | `tiny-httpd` |
    /// | `--header-read-timeout-secs` | `TINY_HTTPD_HEADER_READ_TIMEOUT_SECS` | `30` |
    /// | `--idle-connection-timeout-secs` | `TINY_HTTPD_IDLE_CONNECTION_TIMEOUT_SECS` | `60` |
    /// | `--graceful-close-timeout-secs` | `TINY_HTTPD_GRACEFUL_CLOSE_TIMEOUT_SECS` | `5` |
    ///
    /// # Returns
    /// A parsed [`Config`] with defaults for unset values.
    ///
    /// # Errors
    /// Returns [`ConfigError`] when a value cannot be parsed, a flag is
    /// unknown or missing a value, or for clap display requests such as
    /// `--help` and `--version`.
    pub fn load() -> Result<Self, ConfigError> {
        let cli = CliArgs::try_parse().map_err(|error| {
            if error.use_stderr() {
                ConfigError::from(error)
            } else {
                let _ = error.print();
                std::process::exit(0)
            }
        })?;

        Ok(cli.into())
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

impl From<CliArgs> for Config {
    fn from(cli: CliArgs) -> Self {
        Self {
            listen_addr: cli.listen_addr,
            content_root: cli.content_root,
            service_name: cli.service_name,
            header_read_timeout_secs: cli.header_read_timeout_secs,
            idle_connection_timeout_secs: cli.idle_connection_timeout_secs,
            graceful_close_timeout_secs: cli.graceful_close_timeout_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::CliArgs;

    #[test]
    fn rejects_zero_header_read_timeout() {
        let error = CliArgs::try_parse_from(["tiny-httpd", "--header-read-timeout-secs", "0"])
            .expect_err("zero header timeout should be rejected");

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn rejects_zero_idle_connection_timeout() {
        let error = CliArgs::try_parse_from(["tiny-httpd", "--idle-connection-timeout-secs", "0"])
            .expect_err("zero idle timeout should be rejected");

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_graceful_close_timeout() {
        let cli = CliArgs::try_parse_from(["tiny-httpd", "--graceful-close-timeout-secs", "7"])
            .expect("graceful close timeout should parse");

        assert_eq!(cli.graceful_close_timeout_secs, 7);
    }
}
