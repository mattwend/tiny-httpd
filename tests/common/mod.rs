use bytes::Bytes;
use http_body_util::Empty;
use hyper::{Method, Request, Response};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use tiny_httpd::run_with_shutdown;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

pub const TEST_DEFAULT_HEADER_READ_TIMEOUT_SECS: u64 = 30;
pub const TEST_DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS: u64 = 60;
pub const TEST_DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS: u64 = 5;
pub const TEST_DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 10;

pub fn client() -> Client<HttpConnector, Empty<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

pub struct TestServer {
    addr: std::net::SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), tiny_httpd::ServerError>>>,
}

impl TestServer {
    pub async fn spawn(content_root: std::path::PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        Self::spawn_with_params(
            listener,
            Some(content_root),
            std::time::Duration::from_secs(TEST_DEFAULT_HEADER_READ_TIMEOUT_SECS),
            std::time::Duration::from_secs(TEST_DEFAULT_IDLE_CONNECTION_TIMEOUT_SECS),
            std::time::Duration::from_secs(TEST_DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS),
            std::time::Duration::from_secs(TEST_DEFAULT_DRAIN_TIMEOUT_SECS),
        )
        .await
    }

    pub async fn spawn_with_params(
        listener: TcpListener,
        content_root: Option<std::path::PathBuf>,
        header_read_timeout: std::time::Duration,
        idle_connection_timeout: std::time::Duration,
        graceful_close_timeout: std::time::Duration,
        drain_timeout: std::time::Duration,
    ) -> Self {
        let addr = listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            run_with_shutdown(
                listener,
                content_root,
                header_read_timeout,
                idle_connection_timeout,
                graceful_close_timeout,
                drain_timeout,
                || async move {
                    let _ = shutdown_rx.await;
                    Ok(())
                },
            )
            .await
        });

        Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        }
    }

    pub fn uri(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub async fn request(&self, method: Method, path: &str) -> Response<hyper::body::Incoming> {
        client()
            .request(
                Request::builder()
                    .method(method)
                    .uri(self.uri(path))
                    .body(Empty::<Bytes>::new())
                    .expect("request"),
            )
            .await
            .expect("http response")
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.task.take().expect("server task handle"),
        )
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
        result.expect("server result");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if self.shutdown_tx.is_some() {
            eprintln!(
                "TestServer at {} dropped without shutdown(); aborting server task",
                self.addr
            );
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }
}
