#![forbid(unsafe_code)]

pub mod config;
mod fs;
mod handler;
mod metrics;
pub mod server;
pub mod telemetry;

pub use config::{Config, ConfigError};
pub use server::{ServerError, Startup, run, run_with_shutdown, startup};
pub use telemetry::{TelemetryInitError, init, init_with_stdout_filter};
