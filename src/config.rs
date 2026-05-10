use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use thiserror::Error;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_CONTENT_ROOT: &str = "/app/public";
const DEFAULT_SERVICE_NAME: &str = "tiny-httpd";

/// Runtime configuration loaded from command-line flags and environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Socket address server binds during startup.
    pub listen_addr: SocketAddr,
    /// Filesystem root used for static file lookup.
    pub content_root: PathBuf,
    /// Service name reported through telemetry.
    pub service_name: String,
}

/// Errors produced while parsing runtime configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Listen address could not be parsed as `IP:PORT`.
    #[error("invalid listen address `{value}`: {source}")]
    ListenAddr {
        /// Original configured value that failed to parse.
        value: String,
        /// Address parser failure from standard library.
        #[source]
        source: std::net::AddrParseError,
    },
    /// Known command-line flag was provided without required value.
    #[error("missing value for command-line flag `{0}`")]
    MissingFlagValue(String),
    /// Command-line argument did not match supported flags.
    #[error("unsupported command-line argument `{0}`")]
    UnsupportedArgument(String),
}

impl Config {
    /// Loads configuration from the process environment and command-line args.
    ///
    /// Environment variables are read first and then overridden by flags:
    /// `TINY_HTTPD_LISTEN_ADDR`, `TINY_HTTPD_CONTENT_ROOT`, and
    /// `TINY_HTTPD_SERVICE_NAME`; `--listen-addr`, `--content-root`, and
    /// `--service-name`.
    ///
    /// # Returns
    /// A parsed [`Config`] with defaults for unset values.
    ///
    /// # Errors
    /// Returns [`ConfigError`] when an environment or flag listen address cannot
    /// be parsed, or when a flag is unknown or missing a value.
    pub fn from_env_and_args() -> Result<Self, ConfigError> {
        Self::from_env()?.apply_args(env::args().skip(1))
    }

    /// Loads configuration from the process environment only.
    ///
    /// # Returns
    /// A [`Config`] initialized from environment variables with defaults for
    /// unset values.
    ///
    /// # Errors
    /// Returns [`ConfigError`] when `TINY_HTTPD_LISTEN_ADDR` is present but
    /// cannot be parsed as a socket address.
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_addr = parse_listen_addr(
            env::var("TINY_HTTPD_LISTEN_ADDR")
                .ok()
                .as_deref()
                .unwrap_or(DEFAULT_LISTEN_ADDR),
        )?;

        Ok(Self {
            listen_addr,
            content_root: PathBuf::from(
                env::var("TINY_HTTPD_CONTENT_ROOT")
                    .unwrap_or_else(|_| DEFAULT_CONTENT_ROOT.to_string()),
            ),
            service_name: env::var("TINY_HTTPD_SERVICE_NAME")
                .unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_string()),
        })
    }

    /// Loads configuration from an argument iterator without reading the environment.
    ///
    /// `args` must not include the executable name.
    ///
    /// # Returns
    /// A parsed [`Config`] with built-in defaults overridden by command-line flags.
    ///
    /// # Errors
    /// Returns [`ConfigError`] for invalid arguments or listen addresses.
    pub fn from_args<I, S>(args: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::default().apply_args(args)
    }

    /// Applies command-line flag overrides to an existing configuration.
    ///
    /// # Arguments
    /// * `args` - Command-line arguments excluding the executable name.
    ///
    /// # Returns
    /// The updated [`Config`].
    ///
    /// # Errors
    /// Returns [`ConfigError`] for invalid arguments or listen addresses.
    pub fn apply_args<I, S>(mut self, args: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut listen_addr = self.listen_addr.to_string();
        let mut content_root = self.content_root.display().to_string();
        let mut service_name = self.service_name;

        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            let value = match arg.as_str() {
                "--listen-addr" | "--content-root" | "--service-name" => args
                    .next()
                    .ok_or_else(|| ConfigError::MissingFlagValue(arg.clone()))?,
                _ => return Err(ConfigError::UnsupportedArgument(arg)),
            };

            match arg.as_str() {
                "--listen-addr" => listen_addr = value,
                "--content-root" => content_root = value,
                "--service-name" => service_name = value,
                _ => unreachable!("validated flag names above"),
            }
        }

        self.listen_addr = parse_listen_addr(&listen_addr)?;
        self.content_root = PathBuf::from(content_root);
        self.service_name = service_name;

        Ok(self)
    }
}

/// Parses configured listen address and preserves original value in errors.
fn parse_listen_addr(value: &str) -> Result<SocketAddr, ConfigError> {
    value.parse().map_err(|source| ConfigError::ListenAddr {
        value: value.to_string(),
        source,
    })
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080),
            content_root: PathBuf::from(DEFAULT_CONTENT_ROOT),
            service_name: DEFAULT_SERVICE_NAME.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_overrides() {
        let config = Config::from_args([
            "--listen-addr",
            "127.0.0.1:9090",
            "--content-root",
            "/tmp/public",
            "--service-name",
            "custom",
        ])
        .unwrap();

        assert_eq!(config.listen_addr, "127.0.0.1:9090".parse().unwrap());
        assert_eq!(config.content_root, PathBuf::from("/tmp/public"));
        assert_eq!(config.service_name, "custom");
    }

    #[test]
    fn rejects_invalid_cli() {
        assert!(matches!(
            Config::from_args(["--listen-addr"]),
            Err(ConfigError::MissingFlagValue(_))
        ));
        assert!(matches!(
            Config::from_args(["--unknown"]),
            Err(ConfigError::UnsupportedArgument(_))
        ));
        assert!(matches!(
            Config::from_args(["--listen-addr", "not-an-addr"]),
            Err(ConfigError::ListenAddr { .. })
        ));
    }

    #[test]
    fn rejects_invalid_listen_addr_value() {
        assert!(matches!(
            parse_listen_addr("not-an-addr"),
            Err(ConfigError::ListenAddr { .. })
        ));
    }
}
