#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Tiny HTTP file server with readiness and graceful shutdown support.
//!
//! The library exposes only the server accept loop API. Configuration parsing,
//! telemetry initialization, signal handling, and startup orchestration belong
//! to the binary.

mod fs;
mod handler;
mod metrics;
/// Server accept loop and shutdown APIs.
pub mod server;

pub use server::{ServerError, run_with_shutdown};
