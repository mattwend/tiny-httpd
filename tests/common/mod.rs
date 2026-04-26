use bytes::Bytes;
use http_body_util::Empty;
use hyper::{Method, Request, Response};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use tiny_httpd::{Config, Startup, run_with_shutdown, startup};
use tokio::{sync::oneshot, task::JoinHandle};

pub fn client() -> Client<HttpConnector, Empty<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

pub struct TestServer {
    addr: std::net::SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), tiny_httpd::ServerError>>,
}

impl TestServer {
    pub async fn spawn(content_root: std::path::PathBuf) -> Self {
        let config = Config {
            listen_addr: "127.0.0.1:0".parse().expect("listen addr"),
            content_root,
            service_name: "tiny-httpd-test".to_string(),
        };

        let startup: Startup = startup(&config).await.expect("startup");
        let addr = startup.listener.local_addr().expect("local addr");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            run_with_shutdown(startup, || async move {
                let _ = shutdown_rx.await;
                Ok(())
            })
            .await
        });

        Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
            task,
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

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), self.task)
            .await
            .expect("server task should exit promptly")
            .expect("join server task");
        result.expect("server result");
    }
}
