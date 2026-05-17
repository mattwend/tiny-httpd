// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

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

pub use server::{ServerParams, run_with_shutdown};
