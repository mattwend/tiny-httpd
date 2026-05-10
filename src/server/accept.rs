use std::{net::SocketAddr, sync::Arc, time::Duration};

use hyper::service::service_fn;
use hyper_util::{
    rt::TokioIo,
    server::{conn::auto::Builder as AutoBuilder, graceful::GracefulShutdown},
};
use tokio::{
    net::TcpListener,
    task::JoinSet,
    time::{Instant, Sleep, timeout},
};
use tracing::{error, info, warn};

use crate::{
    handler::{AppState, handle_with_peer_addr},
    server::{DRAIN_TIMEOUT_SECS, ServerError, Startup},
};

/// Time to keep listener accepting only readiness observations after shutdown starts.
const READINESS_DRAIN_WINDOW_MILLIS: u64 = 250;

/// Runs the Hyper accept loop until a shutdown signal is received.
///
/// # Arguments
/// * `startup` - Validated startup resources.
///
/// # Returns
/// `Ok(())` after accepting stops and tracked connections have drained or timed out.
///
/// # Errors
/// Returns [`ServerError`] for startup state inspection failures before entering
/// the accept loop. Transient listener accept failures during serving are logged
/// and the server continues accepting subsequent connections.
pub async fn run(startup: Startup) -> Result<(), ServerError> {
    run_with_shutdown(startup, super::signal::shutdown_signal).await
}

/// Runs the server with an injectable shutdown future for tests.
///
/// # Arguments
/// * `startup` - Validated startup resources.
/// * `shutdown` - Factory producing a future that resolves when shutdown begins.
///
/// # Returns
/// `Ok(())` after accepting stops and tracked connections have drained or timed out.
///
/// # Errors
/// Returns [`ServerError`] for startup state inspection failures before entering
/// the accept loop. Transient listener accept failures during serving are logged
/// and the server continues accepting subsequent connections.
pub async fn run_with_shutdown<F, Fut>(startup: Startup, shutdown: F) -> Result<(), ServerError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), std::io::Error>>,
{
    let local_addr = startup.listener.local_addr()?;
    let state = Arc::new(AppState::new(startup.content_root));
    run_with_state(startup.listener, state, local_addr, shutdown).await
}

/// Runs the server loop with explicit state and an injectable shutdown future.
///
/// # Arguments
/// * `listener` - Bound TCP listener to accept connections from.
/// * `state` - Shared request handling state.
/// * `local_addr` - Listener address for startup logging.
/// * `shutdown` - Factory producing a future that resolves when shutdown begins.
///
/// # Returns
/// `Ok(())` after the accept loop stops and tracked connections have drained.
///
/// # Errors
/// Returns [`ServerError`] for listener-local-address lookup before the accept
/// loop begins. Transient listener accept failures while serving are logged and
/// do not stop the server.
///
/// # Notes
/// This implementation uses hyper-util graceful connection shutdown so
/// in-flight requests can complete while idle keep-alive connections are asked
/// to close promptly. On shutdown, readiness is flipped before the accept loop
/// exits, and the listener remains open for a short bounded drain window so a
/// final readiness probe can observe `503 Service Unavailable` before new
/// accepts stop. A fixed drain timeout remains a hard upper bound for stuck
/// connections, after which remaining tasks are aborted.
pub(crate) async fn run_with_state<F, Fut>(
    listener: TcpListener,
    state: Arc<AppState>,
    local_addr: SocketAddr,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), std::io::Error>>,
{
    let graceful = GracefulShutdown::new();
    let mut connections = JoinSet::new();
    let builder = AutoBuilder::new(hyper_util::rt::TokioExecutor::new());
    let mut shutdown = std::pin::pin!(shutdown());
    let mut shutdown_deadline: Option<std::pin::Pin<Box<Sleep>>> = None;

    info!(%local_addr, "tiny-httpd listening");

    loop {
        tokio::select! {
            signal = &mut shutdown, if shutdown_deadline.is_none() => {
                if let Err(error) = signal {
                    warn!(error = %error, "shutdown signal handler failed; shutting down anyway");
                }
                state.mark_not_ready();
                state.mark_shutting_down();
                shutdown_deadline = Some(Box::pin(tokio::time::sleep_until(
                    Instant::now() + Duration::from_millis(READINESS_DRAIN_WINDOW_MILLIS),
                )));
                info!(
                    drain_window_ms = READINESS_DRAIN_WINDOW_MILLIS,
                    "shutdown requested; continuing to serve readiness responses before stopping accepts"
                );
            }
            () = async {
                if let Some(deadline) = shutdown_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if shutdown_deadline.is_some() => {
                info!("shutdown requested; stopping accepts and draining existing connections");
                break;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        warn!(error = %error, "listener accept failed");
                        continue;
                    }
                };
                let io = TokioIo::new(stream);
                let service_state = Arc::clone(&state);
                let connection_builder = builder.clone();
                let watcher = graceful.watcher();
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let request_state = Arc::clone(&service_state);
                        handle_with_peer_addr(request_state, request, Some(peer_addr))
                    });
                    let connection = connection_builder.serve_connection(io, service);
                    if let Err(error) = watcher.watch(connection).await {
                        warn!(%peer_addr, error = %error, "connection failed");
                    }
                });
            }
        }
    }

    match timeout(Duration::from_secs(DRAIN_TIMEOUT_SECS), graceful.shutdown()).await {
        Ok(()) => info!("all connections drained gracefully"),
        Err(_) => {
            warn!(
                timeout_secs = DRAIN_TIMEOUT_SECS,
                "connection drain timed out; aborting remaining tasks"
            );
            connections.abort_all();
        }
    }

    drain_connections(&mut connections).await;

    Ok(())
}

/// Waits for spawned connection tasks and logs task-level failures.
async fn drain_connections(connections: &mut JoinSet<()>) {
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            error!(error = %error, "connection task failed");
        }
    }
}
