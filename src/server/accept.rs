use std::{
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use hyper::service::service_fn;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::conn::auto::Builder as AutoBuilder,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::{Notify, watch},
    task::JoinSet,
    time::{Instant, Sleep, timeout},
};

use tracing::{error, info, warn};

use crate::{
    handler::{AppState, handle_with_peer_addr},
    server::{DRAIN_TIMEOUT_SECS, ServerError},
};

/// Time to keep listener accepting only readiness observations after shutdown starts.
const READINESS_DRAIN_WINDOW_MILLIS: u64 = 250;

/// IO wrapper that signals activity on reads/writes via a [`Notify`].
///
/// When any bytes flow through the connection the shared `Notify` is signalled,
/// allowing an external idle-timeout sleep to be reset.
///
/// `Notify::notify_one()` is intentionally used as a lossy edge trigger here.
/// Multiple completed reads/writes may collapse into one pending notification,
/// but idle-timeout handling only needs to know that some real byte activity
/// happened since the last reset.
struct ActivityIo<T> {
    inner: T,
    activity: Arc<Notify>,
}

impl<T> ActivityIo<T> {
    fn new(inner: T, activity: Arc<Notify>) -> Self {
        Self { inner, activity }
    }
}

impl<T> AsyncRead for ActivityIo<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > filled_before {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<T> AsyncWrite for ActivityIo<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write_vectored(cx, bufs) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    self.activity.notify_one();
                }
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

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
    run_with_state(
        listener,
        state,
        local_addr,
        header_read_timeout,
        idle_connection_timeout,
        graceful_close_timeout,
        shutdown,
    )
    .await
}

/// Runs the server loop with explicit state and an injectable shutdown future.
///
/// # Arguments
/// * `listener` - Bound TCP listener to accept connections from.
/// * `state` - Shared request handling state.
/// * `local_addr` - Listener address for startup logging.
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
pub(crate) async fn run_with_state<F, Fut>(
    listener: TcpListener,
    state: Arc<AppState>,
    local_addr: SocketAddr,
    header_read_timeout: Duration,
    idle_connection_timeout: Duration,
    graceful_close_timeout: Duration,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), std::io::Error>>,
{
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
                let activity = Arc::new(Notify::new());
                let io = TokioIo::new(ActivityIo::new(stream, Arc::clone(&activity)));
                let service_state = Arc::clone(&state);
                let connection_builder = builder.clone();
                let mut shutdown_rx = shutdown_tx.subscribe();
                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        let request_state = Arc::clone(&service_state);
                        handle_with_peer_addr(request_state, request, Some(peer_addr))
                    });
                    let connection = connection_builder.serve_connection(io, service).into_owned();
                    tokio::pin!(connection);

                    let deadline = tokio::time::sleep(idle_connection_timeout);
                    tokio::pin!(deadline);
                    let mut shutting_down = false;

                    macro_rules! start_graceful_shutdown {
                        () => {
                            debug_assert!(!shutting_down, "graceful shutdown started twice");
                            connection.as_mut().graceful_shutdown();
                            shutting_down = true;
                            deadline
                                .as_mut()
                                .reset(Instant::now() + graceful_close_timeout);
                        };
                    }

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
                                start_graceful_shutdown!();
                            }
                            changed = shutdown_rx.changed(), if !shutting_down => {
                                match changed {
                                    Ok(()) => {
                                        if *shutdown_rx.borrow() {
                                            start_graceful_shutdown!();
                                        }
                                    }
                                    Err(error) => {
                                        warn!(%peer_addr, error = %error, "shutdown signal channel closed unexpectedly; starting graceful connection shutdown");
                                        start_graceful_shutdown!();
                                    }
                                }
                            }
                            () = activity.notified(), if !shutting_down => {
                                deadline.as_mut().reset(Instant::now() + idle_connection_timeout);
                            }
                        }
                    }
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

/// Waits for spawned connection tasks and logs task-level failures.
async fn drain_connections(connections: &mut JoinSet<()>) {
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            error!(error = %error, "connection task failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

    #[derive(Default)]
    struct MockIo {
        read_data: Vec<u8>,
        read_offset: usize,
        write_data: Vec<u8>,
    }

    impl MockIo {
        fn with_read_data(read_data: &[u8]) -> Self {
            Self {
                read_data: read_data.to_vec(),
                read_offset: 0,
                write_data: Vec::new(),
            }
        }
    }

    impl AsyncRead for MockIo {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.read_offset >= self.read_data.len() {
                return Poll::Ready(Ok(()));
            }

            let remaining = &self.read_data[self.read_offset..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.read_offset += to_copy;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for MockIo {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.write_data.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn activity_io_notifies_on_non_empty_read_and_write() {
        let activity = Arc::new(Notify::new());
        let mut io = ActivityIo::new(MockIo::with_read_data(b"hello"), Arc::clone(&activity));

        let read_notification = activity.notified();
        let mut buffer = [0_u8; 5];
        let read = io.read(&mut buffer).await.expect("read succeeds");
        assert_eq!(read, 5);
        tokio::time::timeout(Duration::from_millis(50), read_notification)
            .await
            .expect("read should notify");

        let write_notification = activity.notified();
        let written = io.write(b"world").await.expect("write succeeds");
        assert_eq!(written, 5);
        tokio::time::timeout(Duration::from_millis(50), write_notification)
            .await
            .expect("write should notify");
    }

    #[tokio::test]
    async fn activity_io_does_not_notify_on_zero_byte_read() {
        let activity = Arc::new(Notify::new());
        let mut io = ActivityIo::new(MockIo::default(), Arc::clone(&activity));

        let notification = activity.notified();
        let mut buffer = [0_u8; 8];
        let read = io.read(&mut buffer).await.expect("read succeeds");
        assert_eq!(read, 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notification)
                .await
                .is_err(),
            "zero-byte read should not notify"
        );
    }
}
