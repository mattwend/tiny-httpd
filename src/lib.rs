#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Tiny HTTP file server with readiness, graceful shutdown, and telemetry.
//!
//! Library exports configuration loading, telemetry initialization, and server
//! startup/run entry points for embedding or binary use. See project README for
//! deployment, configuration, and operational details.

/// Runtime configuration loading and parsing.
pub mod config;
mod fs;
mod handler;
mod metrics;
/// Server startup, binding, shutdown, and accept loop APIs.
pub mod server;
/// Telemetry initialization and shutdown helpers.
pub mod telemetry;

pub use config::{Config, ConfigError};
pub use server::{ServerError, Startup, run, run_with_shutdown, startup};
pub use telemetry::{TelemetryInitError, init, init_with_stdout_filter};
