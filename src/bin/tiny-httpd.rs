// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io::ErrorKind,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{ArgAction, Parser, ValueHint};
use telemetry_setup::{TelemetryBuilder, TelemetryError};
use thiserror::Error;
use tiny_httpd::{ServerParams, run_with_shutdown};
use tokio::net::TcpListener;
use tracing::{error, warn};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_CONTENT_ROOT: &str = "/app/public";
const DEFAULT_SERVICE_NAME: &str = "tiny-httpd";
// These CLI defaults are intentionally independent from `ServerParams::default()`
// so the binary can own runtime policy separately from the library API.
const DEFAULT_HEADER_READ_TIMEOUT: &str = "30s";
const DEFAULT_IDLE_CONNECTION_TIMEOUT: &str = "60s";
const DEFAULT_GRACEFUL_CLOSE_TIMEOUT: &str = "5s";
const DEFAULT_DRAIN_TIMEOUT: &str = "10s";
const HELP_TEMPLATE: &str =
    "{name} {version}\n{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}{after-help}";
const TIMEOUTS_HEADING: &str = "Timeouts (durations like \"30s\", \"2m\", \"1h30m\")";
const AFTER_HELP: &str = r#"Examples:
  # Serve local files during development
  tiny-httpd -l 127.0.0.1:3000 -r ./public

  # Run with container-friendly defaults
  tiny-httpd

  # Custom timeouts for slow networks
  tiny-httpd --header-read-timeout 60s --idle-connection-timeout 2m

  # Serve content from a different directory
  tiny-httpd -r ./dist

Repository: https://github.com/mattwend/tiny-httpd"#;

#[derive(Debug, Clone, Parser)]
#[command(
    version,
    about = "Minimal static-file HTTP server for containers and Kubernetes",
    long_about = None,
    help_template = HELP_TEMPLATE,
    after_help = AFTER_HELP,
    disable_help_flag = true,
    disable_version_flag = true,
)]
struct Config {
    /// Bind address.
    #[arg(
        short = 'l',
        long,
        env = "TINY_HTTPD_LISTEN_ADDR",
        default_value = DEFAULT_LISTEN_ADDR,
        value_name = "ADDR",
        value_hint = ValueHint::Other,
        value_parser = parse_listen_addr,
        help_heading = "Server",
    )]
    listen_addr: String,
    /// Static content root.
    #[arg(
        short = 'r',
        long,
        env = "TINY_HTTPD_CONTENT_ROOT",
        default_value = DEFAULT_CONTENT_ROOT,
        value_name = "PATH",
        value_hint = ValueHint::DirPath,
        help_heading = "Server",
    )]
    content_root: PathBuf,
    /// Telemetry service name.
    #[arg(
        long,
        env = "TINY_HTTPD_SERVICE_NAME",
        default_value = DEFAULT_SERVICE_NAME,
        value_name = "NAME",
        value_hint = ValueHint::Other,
        help_heading = "Server",
    )]
    service_name: String,
    /// HTTP/1 header read timeout.
    #[arg(
        long,
        env = "TINY_HTTPD_HEADER_READ_TIMEOUT",
        default_value = DEFAULT_HEADER_READ_TIMEOUT,
        value_name = "DUR",
        value_parser = parse_duration,
        help_heading = TIMEOUTS_HEADING,
    )]
    header_read_timeout: Duration,
    /// Maximum idle time before server closes an inactive connection.
    #[arg(
        long,
        env = "TINY_HTTPD_IDLE_CONN_TIMEOUT",
        default_value = DEFAULT_IDLE_CONNECTION_TIMEOUT,
        value_name = "DUR",
        value_parser = parse_duration,
        help_heading = TIMEOUTS_HEADING,
    )]
    idle_connection_timeout: Duration,
    /// Maximum graceful-close time before a draining connection is dropped.
    #[arg(
        long,
        env = "TINY_HTTPD_GRACEFUL_CLOSE_TIMEOUT",
        default_value = DEFAULT_GRACEFUL_CLOSE_TIMEOUT,
        value_name = "DUR",
        value_parser = parse_duration,
        help_heading = TIMEOUTS_HEADING,
    )]
    graceful_close_timeout: Duration,
    /// Maximum process-level drain time before remaining connections are aborted.
    #[arg(
        long,
        env = "TINY_HTTPD_DRAIN_TIMEOUT",
        default_value = DEFAULT_DRAIN_TIMEOUT,
        value_name = "DUR",
        value_parser = parse_duration,
        help_heading = TIMEOUTS_HEADING,
    )]
    drain_timeout: Duration,
    #[arg(
        short = 'h',
        long = "help",
        action = ArgAction::HelpShort,
        help = "Print help",
        help_heading = "Options",
    )]
    // Dummy field used to customize clap's generated short-help flag while global auto-help is disabled.
    help: Option<bool>,
    #[arg(
        short = 'V',
        long = "version",
        action = ArgAction::Version,
        help = "Print version",
        help_heading = "Options",
    )]
    // Dummy field used to customize clap's generated version flag while global auto-version is disabled.
    version: Option<bool>,
}

/// Parses and validates a listen address accepted by the CLI.
///
/// # Arguments
/// * `value` - Raw CLI or environment value for `--listen-addr`.
///
/// # Returns
/// The original string when it is either a valid `SocketAddr` or a `host:port`
/// pair with a non-empty host and a valid, non-zero `u16` port.
///
/// # Errors
/// Returns a descriptive string when the value is neither a socket address nor a
/// syntactically valid `host:port` pair.
fn parse_listen_addr(value: &str) -> Result<String, String> {
    if let Ok(socket_addr) = value.parse::<SocketAddr>() {
        if socket_addr.port() == 0 {
            return Err("port must be greater than 0".to_owned());
        }

        return Ok(value.to_owned());
    }

    let Some((host, port)) = value.rsplit_once(':') else {
        return Err("must be a SocketAddr or host:port".to_owned());
    };

    if host.is_empty() {
        return Err("host must not be empty".to_owned());
    }

    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        return Err("IPv6 addresses must be enclosed in brackets, e.g. [::1]:8080".to_owned());
    }

    let port = port
        .parse::<u16>()
        .map_err(|_| "port must be a valid 16-bit unsigned integer".to_owned())?;

    if port == 0 {
        return Err("port must be greater than 0".to_owned());
    }

    Ok(value.to_owned())
}

/// Parses a human-readable duration string accepted by the CLI.
///
/// # Arguments
/// * `value` - Raw CLI or environment value for a timeout option.
///
/// # Returns
/// The parsed [`Duration`] when `value` uses a valid `humantime` format and is
/// at least one second.
///
/// # Errors
/// Returns a descriptive string when the duration cannot be parsed or is less
/// than one second.
fn parse_duration(value: &str) -> Result<Duration, String> {
    let duration = humantime::parse_duration(value).map_err(|error| error.to_string())?;

    if duration < Duration::from_secs(1) {
        return Err("duration must be at least 1s".to_owned());
    }

    Ok(duration)
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
        /// Address (host:port) server attempted to bind.
        addr: String,
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
async fn prepare_content_root(content_root: &Path) -> Result<Option<PathBuf>, StartupError> {
    match tokio::fs::metadata(content_root).await {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(StartupError::ContentRootNotDirectory(
                    content_root.to_path_buf(),
                ));
            }

            Ok(Some(tokio::fs::canonicalize(content_root).await.map_err(
                |source| StartupError::ContentRootCanonicalize {
                    path: content_root.to_path_buf(),
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

/// Top-level application errors.
#[derive(Debug, Error)]
enum MainError {
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Server(#[from] std::io::Error),
}

/// Loads configuration, initializes telemetry, and runs server until shutdown.
#[tokio::main]
async fn main() -> Result<(), MainError> {
    let config = Config::parse();
    let mut telemetry_guard = TelemetryBuilder::new(&config.service_name).init()?;
    let content_root = prepare_content_root(&config.content_root).await?;
    let listener = TcpListener::bind(&config.listen_addr)
        .await
        .map_err(|source| StartupError::Bind {
            addr: config.listen_addr,
            source,
        })?;

    let result = run_with_shutdown(
        listener,
        ServerParams {
            content_root,
            header_read_timeout: config.header_read_timeout,
            idle_connection_timeout: config.idle_connection_timeout,
            graceful_close_timeout: config.graceful_close_timeout,
            drain_timeout: config.drain_timeout,
        },
        shutdown_signal,
    )
    .await;

    if let Err(error) = &result {
        error!(%error, "server exited with error");
    }

    if let Err(shutdown_error) = telemetry_guard.shutdown().await {
        warn!(%shutdown_error, "telemetry shutdown failed");
    }

    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use clap::{CommandFactory, Parser};

    use super::{Config, parse_duration, parse_listen_addr};

    #[test]
    fn parse_listen_addr_accepts_socket_addrs_and_hostnames() {
        for value in ["127.0.0.1:8080", "[::1]:8080", "localhost:443"] {
            assert_eq!(parse_listen_addr(value), Ok(value.to_owned()));
        }
    }

    #[test]
    fn parse_listen_addr_rejects_port_zero() {
        for value in ["127.0.0.1:0", "localhost:0", "[::1]:0"] {
            assert_eq!(
                parse_listen_addr(value),
                Err("port must be greater than 0".to_owned())
            );
        }
    }

    #[test]
    fn parse_listen_addr_rejects_missing_separator_or_empty_host() {
        assert_eq!(
            parse_listen_addr("nocolon"),
            Err("must be a SocketAddr or host:port".to_owned())
        );
        assert_eq!(
            parse_listen_addr(":8080"),
            Err("host must not be empty".to_owned())
        );
    }

    #[test]
    fn parse_listen_addr_rejects_unbracketed_ipv6_and_invalid_port() {
        assert_eq!(
            parse_listen_addr("::1:8080"),
            Err("IPv6 addresses must be enclosed in brackets, e.g. [::1]:8080".to_owned())
        );
        assert_eq!(
            parse_listen_addr("host:99999"),
            Err("port must be a valid 16-bit unsigned integer".to_owned())
        );
    }

    #[test]
    fn parse_duration_accepts_human_readable_values() {
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5_400)));
    }

    #[test]
    fn parse_duration_rejects_values_below_one_second() {
        assert_eq!(
            parse_duration("999ms"),
            Err("duration must be at least 1s".to_owned())
        );
    }

    #[test]
    fn config_parses_new_flags_and_defaults() {
        let config = Config::parse_from(["tiny-httpd"]);

        assert_eq!(config.listen_addr, "0.0.0.0:8080");
        assert_eq!(config.content_root, PathBuf::from("/app/public"));
        assert_eq!(config.service_name, "tiny-httpd");
        assert_eq!(config.header_read_timeout, Duration::from_secs(30));
        assert_eq!(config.idle_connection_timeout, Duration::from_secs(60));
        assert_eq!(config.graceful_close_timeout, Duration::from_secs(5));
        assert_eq!(config.drain_timeout, Duration::from_secs(10));
    }

    #[test]
    fn config_parses_short_flags_and_human_readable_durations() {
        let config = Config::parse_from([
            "tiny-httpd",
            "-l",
            "127.0.0.1:3000",
            "-r",
            "./public",
            "--header-read-timeout",
            "45s",
            "--idle-connection-timeout",
            "2m",
            "--graceful-close-timeout",
            "6s",
            "--drain-timeout",
            "12s",
        ]);

        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.content_root, PathBuf::from("./public"));
        assert_eq!(config.header_read_timeout, Duration::from_secs(45));
        assert_eq!(config.idle_connection_timeout, Duration::from_secs(120));
        assert_eq!(config.graceful_close_timeout, Duration::from_secs(6));
        assert_eq!(config.drain_timeout, Duration::from_secs(12));
    }

    #[test]
    fn config_rejects_legacy_timeout_flag_names() {
        let error = Config::try_parse_from(["tiny-httpd", "--header-read-timeout-secs", "30"])
            .expect_err("legacy timeout flag should be rejected");

        assert!(
            error
                .to_string()
                .contains("unexpected argument '--header-read-timeout-secs'")
        );
    }

    #[test]
    fn help_output_orders_server_and_timeout_sections_before_options() {
        let mut command = Config::command();
        let help = command.render_help().to_string();

        let server_index = help.find("Server:").expect("server section present");
        let timeouts_index = help.find("Timeouts (").expect("timeouts section present");
        let options_index = help.find("Options:").expect("options section present");

        assert!(server_index < timeouts_index);
        assert!(timeouts_index < options_index);
        assert!(help.contains("Examples:"));
        assert!(help.contains("Repository: https://github.com/mattwend/tiny-httpd"));
    }
}
