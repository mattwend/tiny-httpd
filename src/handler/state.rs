use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::metrics::HttpMetrics;

/// Shared application state for request handling.
#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) content_root: Option<PathBuf>,
    pub(crate) ready: AtomicBool,
    pub(crate) shutting_down: AtomicBool,
    metrics: HttpMetrics,
}

impl AppState {
    /// Creates request handling state.
    ///
    /// # Arguments
    /// * `content_root` - Canonical content root validated during startup when available.
    ///
    /// # Returns
    /// A ready [`AppState`] instance.
    pub(crate) fn new(content_root: Option<PathBuf>) -> Self {
        Self {
            content_root,
            ready: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            metrics: HttpMetrics::new(),
        }
    }

    /// Marks the application as not ready.
    pub(crate) fn mark_not_ready(&self) {
        self.ready.store(false, Ordering::SeqCst);
    }

    /// Marks the application as draining for graceful shutdown.
    pub(crate) fn mark_shutting_down(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    /// Returns shared HTTP metrics recorder.
    pub(crate) fn metrics(&self) -> &HttpMetrics {
        &self.metrics
    }
}
