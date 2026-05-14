use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use hyper::service::service_fn;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::conn::auto::Builder as AutoBuilder,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Notify, watch},
    task::JoinSet,
    time::{Instant, Sleep, timeout},
};
use tracing::{error, info, warn};

use crate::{
    handler::{AppState, handle_with_peer_addr},
    server::{
        DRAIN_TIMEOUT_SECS, READINESS_DRAIN_WINDOW_MILLIS, ServerError, activity_io::ActivityIo,
    },
};

/// Runs the server with an injectable shutdown future for tests or binaries.
///
/// # Arguments
/// * `listener` - Bound TCP listener to accept connections from.
/// * `content_root` - Canonical static content root when one is available.
/// * `header_read_timeout` - Maximum time allowed to receive complete HTTP/1 request headers.
/// * `idle_connection_timeout` - Maximum idle time allowed for one open connection.
/// * `graceful_close_timeout` - Maximum graceful-close time for one draining connection.
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
/// This implementation manually coordinates per-connection graceful shutdown so
/// in-flight requests can complete while idle keep-alive connections are asked
/// to close promptly. On shutdown, readiness is flipped before the accept loop
/// exits, and the listener remains open for a short bounded drain window so a
/// final readiness probe can observe `503 Service Unavailable` before new
/// accepts stop. A fixed drain timeout remains a hard upper bound for stuck
/// connections, after which remaining tasks are aborted.
pub async fn run_with_shutdown<F, Fut>(
    listener: TcpListener,
    content_root: Option<PathBuf>,
    header_read_timeout: Duration,
    idle_connection_timeout: Duration,
    graceful_close_timeout: Duration,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), std::io::Error>>,
{
    let local_addr = listener.local_addr()?;
    let state = Arc::new(AppState::new(content_root));
    let (shutdown_tx, _) = watch::channel(false);
    let mut connections = JoinSet::new();
    let mut builder = AutoBuilder::new(hyper_util::rt::TokioExecutor::new());
    builder.http1().timer(TokioTimer::new());
    builder.http1().header_read_timeout(header_read_timeout);
    let mut shutdown = std::pin::pin!(shutdown());
    let mut shutdown_deadline: Option<std::pin::Pin<Box<Sleep>>> = None;

    info!(
        %local_addr,
        header_read_timeout_secs = header_read_timeout.as_secs(),
        idle_connection_timeout_secs = idle_connection_timeout.as_secs(),
        graceful_close_timeout_secs = graceful_close_timeout.as_secs(),
        "tiny-httpd listening"
    );

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
                let service_state = Arc::clone(&state);
                let connection_builder = builder.clone();
                let shutdown_rx = shutdown_tx.subscribe();
                connections.spawn(async move {
                    serve_connection(
                        stream,
                        peer_addr,
                        service_state,
                        connection_builder,
                        shutdown_rx,
                        idle_connection_timeout,
                        graceful_close_timeout,
                    )
                    .await;
                });
            }
        }
    }

    let _ = shutdown_tx.send(true);

    match timeout(
        Duration::from_secs(DRAIN_TIMEOUT_SECS),
        drain_connections(&mut connections),
    )
    .await
    {
        Ok(()) => info!("all connections drained gracefully"),
        Err(_) => {
            warn!(
                timeout_secs = DRAIN_TIMEOUT_SECS,
                "connection drain timed out; aborting remaining tasks"
            );
            connections.abort_all();
            drain_connections(&mut connections).await;
        }
    }

    Ok(())
}

/// Serves one accepted TCP connection until completion or shutdown.
///
/// # Arguments
/// * `stream` - Accepted TCP stream.
/// * `peer_addr` - Remote peer socket address for logging and request metadata.
/// * `state` - Shared request handling state.
/// * `builder` - Hyper connection builder configured by the server.
/// * `shutdown_rx` - Shutdown notification receiver for graceful drain.
/// * `idle_connection_timeout` - Maximum idle time before graceful shutdown begins.
/// * `graceful_close_timeout` - Maximum time to wait after graceful shutdown starts.
///
/// # Returns
/// Completes when the connection finishes, times out, or is dropped.
async fn serve_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    state: Arc<AppState>,
    builder: AutoBuilder<hyper_util::rt::TokioExecutor>,
    mut shutdown_rx: watch::Receiver<bool>,
    idle_connection_timeout: Duration,
    graceful_close_timeout: Duration,
) {
    let activity = Arc::new(Notify::new());
    let io = TokioIo::new(ActivityIo::new(stream, Arc::clone(&activity)));
    let service = service_fn(move |request| {
        let request_state = Arc::clone(&state);
        handle_with_peer_addr(request_state, request, Some(peer_addr))
    });
    let connection = builder.serve_connection(io, service).into_owned();
    tokio::pin!(connection);

    let deadline = tokio::time::sleep(idle_connection_timeout);
    tokio::pin!(deadline);
    let mut shutting_down = false;

    loop {
        tokio::select! {
            result = &mut connection => {
                match result {
                    Ok(()) => {}
                    Err(error) => {
                        warn!(%peer_addr, error = %error, "connection failed");
                    }
                }
                break;
            }
            () = &mut deadline => {
                if shutting_down {
                    warn!(%peer_addr, "graceful connection shutdown timed out; dropping connection");
                    break;
                }
                warn!(
                    %peer_addr,
                    idle_connection_timeout_secs = idle_connection_timeout.as_secs(),
                    "idle connection timeout reached; starting graceful connection shutdown"
                );
                connection.as_mut().graceful_shutdown();
                shutting_down = true;
                deadline
                    .as_mut()
                    .reset(Instant::now() + graceful_close_timeout);
            }
            changed = shutdown_rx.changed(), if !shutting_down => {
                match changed {
                    Ok(()) => {
                        if *shutdown_rx.borrow() {
                            connection.as_mut().graceful_shutdown();
                            shutting_down = true;
                            deadline
                                .as_mut()
                                .reset(Instant::now() + graceful_close_timeout);
                        }
                    }
                    Err(error) => {
                        warn!(%peer_addr, error = %error, "shutdown signal channel closed unexpectedly; starting graceful connection shutdown");
                        connection.as_mut().graceful_shutdown();
                        shutting_down = true;
                        deadline
                            .as_mut()
                            .reset(Instant::now() + graceful_close_timeout);
                    }
                }
            }
            () = activity.notified(), if !shutting_down => {
                deadline.as_mut().reset(Instant::now() + idle_connection_timeout);
            }
        }
    }
}

/// Waits for spawned connection tasks and logs task-level failures.
async fn drain_connections(connections: &mut JoinSet<()>) {
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            error!(error = %error, "connection task failed");
        }
    }
}
