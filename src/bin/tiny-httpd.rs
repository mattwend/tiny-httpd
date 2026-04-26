use thiserror::Error;
use tiny_httpd::{Config, ConfigError, ServerError, TelemetryInitError, run, startup, telemetry};
use tracing::{error, warn};

/// Top-level application errors.
#[derive(Debug, Error)]
enum MainError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Telemetry(#[from] TelemetryInitError),
    #[error(transparent)]
    Server(#[from] ServerError),
}

#[tokio::main]
async fn main() -> Result<(), MainError> {
    let config = Config::from_env_and_args()?;
    let mut guard = telemetry::init(&config.service_name)?;
    let startup = startup(&config).await?;

    if let Err(error) = run(startup).await {
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
