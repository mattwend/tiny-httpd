use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::Empty;
use hyper::{Request, StatusCode};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use tiny_httpd::run_with_shutdown;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

fn client() -> Client<HttpConnector, Empty<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

async fn bind_listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener")
}

#[tokio::test]
async fn idle_keep_alive_connections_close_promptly_on_shutdown() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write request");

    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
            .await
            .expect("response read timeout")
            .expect("read response");
        assert!(
            read > 0,
            "connection closed before first response completed"
        );
        response.extend_from_slice(&buffer[..read]);
        if response.windows(5).any(|window| window == b"hello") {
            break;
        }
    }

    shutdown_tx.send(()).expect("send shutdown");

    let started = Instant::now();
    let eof = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let read = stream.read(&mut buffer).await.expect("read after shutdown");
            if read == 0 {
                return;
            }
        }
    })
    .await;
    assert!(
        eof.is_ok(),
        "idle keep-alive connection did not close promptly"
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
    result.expect("server result");
}

#[tokio::test]
async fn idle_keep_alive_connections_close_promptly_after_idle_timeout() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(1),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write request");

    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
            .await
            .expect("response read timeout")
            .expect("read response");
        assert!(
            read > 0,
            "connection closed before first response completed"
        );
        response.extend_from_slice(&buffer[..read]);
        if response.windows(5).any(|window| window == b"hello") {
            break;
        }
    }

    let started = Instant::now();
    let eof = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let read = stream
                .read(&mut buffer)
                .await
                .expect("read after idle timeout");
            if read == 0 {
                return;
            }
        }
    })
    .await;
    assert!(
        eof.is_ok(),
        "idle keep-alive connection did not close promptly"
    );
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(2));

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn active_keep_alive_connections_do_not_hit_idle_timeout() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(1),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut buffer = [0_u8; 1024];

    for _ in 0..3 {
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .expect("write request");

        let mut response = Vec::new();
        loop {
            let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
                .await
                .expect("response read timeout")
                .expect("read response");
            assert!(read > 0, "connection closed unexpectedly while active");
            response.extend_from_slice(&buffer[..read]);
            if response.windows(5).any(|window| window == b"hello") {
                break;
            }
        }

        let response = String::from_utf8_lossy(&response);
        assert!(response.contains("HTTP/1.1 200 OK"));

        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn shutdown_after_completed_request_still_drains_promptly() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let client = client();
    let request = Request::builder()
        .uri(format!("http://{addr}/"))
        .body(Empty::<Bytes>::new())
        .expect("request");
    let response = client.request(request).await.expect("http response");
    assert_eq!(response.status(), StatusCode::OK);

    shutdown_tx.send(()).expect("send shutdown");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
    result.expect("server result");
}

#[tokio::test]
async fn graceful_shutdown_keeps_liveness_ok_while_readiness_fails_during_drain_window() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let uri = |path: &str| format!("http://{addr}{path}").parse().expect("uri");

    let before = client()
        .get(uri("/readyz"))
        .await
        .expect("readyz before shutdown");
    assert_eq!(before.status(), StatusCode::OK);

    shutdown_tx.send(()).expect("send shutdown");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let during_drain = client()
        .get(uri("/readyz"))
        .await
        .expect("fresh readyz connection during shutdown drain window");
    assert_eq!(during_drain.status(), StatusCode::SERVICE_UNAVAILABLE);

    let livez_during_drain = client()
        .get(uri("/livez"))
        .await
        .expect("fresh livez connection during shutdown drain window");
    assert_eq!(livez_during_drain.status(), StatusCode::OK);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
    result.expect("server result");
}

#[tokio::test]
async fn shutdown_flips_probe_states_before_listener_stops_accepting() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect keep-alive stream");
    stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write readyz before shutdown");

    let mut buffer = vec![0_u8; 2048];
    let first_read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
        .await
        .expect("readyz before shutdown timeout")
        .expect("readyz before shutdown read");
    let first_response = String::from_utf8_lossy(&buffer[..first_read]);
    assert!(first_response.starts_with("HTTP/1.1 200 OK"));

    shutdown_tx.send(()).expect("send shutdown");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_readyz_503 = false;
    while Instant::now() < deadline {
        if stream
            .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .is_err()
        {
            break;
        }
        let read =
            match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer)).await {
                Ok(Ok(read)) => read,
                Ok(Err(_)) | Err(_) => break,
            };
        if read == 0 {
            break;
        }
        let response = String::from_utf8_lossy(&buffer[..read]);
        saw_readyz_503 |= response.contains("HTTP/1.1 503 Service Unavailable");
        if saw_readyz_503 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        saw_readyz_503,
        "shutdown should expose readiness=false before the listener fully stops accepting"
    );

    let client = client();
    let uri = |path: &str| format!("http://{addr}{path}").parse().expect("uri");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let refused = client.get(uri("/readyz")).await;
    assert!(
        refused.is_err(),
        "listener should stop accepting new connections after the readiness drain window"
    );

    drop(client);

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
    result.expect("server result");
}

#[tokio::test]
async fn graceful_shutdown_stops_accepting_promptly_without_new_connections() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let content_root = Some(tempdir.path().to_path_buf());

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    shutdown_tx.send(()).expect("send shutdown");

    let result = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task should exit promptly")
        .expect("join server task");
    result.expect("server result");
}

#[tokio::test]
async fn server_serves_http_requests_before_shutdown() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let listener = bind_listener().await;
    let addr = listener.local_addr().expect("local addr");
    let content_root = Some(tempdir.path().to_path_buf());

    let (_shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        run_with_shutdown(
            listener,
            content_root,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(5),
            || async move {
                let _ = shutdown_rx.await;
                Ok(())
            },
        )
        .await
    });

    let client = client();
    let uri = |path: &str| format!("http://{addr}{path}").parse().expect("uri");

    let readyz = client
        .get(uri("/readyz"))
        .await
        .expect("readyz before shutdown");
    assert_eq!(readyz.status(), StatusCode::OK);

    let root = client.get(uri("/")).await.expect("root before shutdown");
    assert_eq!(root.status(), StatusCode::OK);

    server_task.abort();
    let _ = server_task.await;
}
