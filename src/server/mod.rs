mod accept;
mod activity_io;

pub use accept::run_with_shutdown;

/// Default graceful-drain time before remaining connection tasks are aborted.
pub const DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 10;
