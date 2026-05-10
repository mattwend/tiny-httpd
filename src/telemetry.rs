use telemetry_setup::{TelemetryBuilder, TelemetryError, TelemetryGuard};
use thiserror::Error;

/// Errors returned by the telemetry adapter.
#[derive(Debug, Error)]
pub enum TelemetryInitError {
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
pub fn init(service_name: &str) -> Result<TelemetryGuard, TelemetryInitError> {
    TelemetryBuilder::new(service_name)
        .init()
        .map_err(Into::into)
}

/// Initializes telemetry with an explicit stdout filter and no environment lookup.
///
/// # Arguments
/// * `service_name` - OpenTelemetry service name for this process.
/// * `stdout_filter` - `tracing_subscriber::EnvFilter` expression for local logs.
///
/// # Returns
/// A telemetry guard that must stay alive until process shutdown.
///
/// # Errors
/// Returns [`TelemetryInitError`] when the shared telemetry setup fails.
pub fn init_with_stdout_filter(
    service_name: &str,
    stdout_filter: &str,
) -> Result<TelemetryGuard, TelemetryInitError> {
    TelemetryBuilder::new(service_name)
        .with_env_var("")
        .with_stdout_filter(stdout_filter)
        .init()
        .map_err(Into::into)
}
