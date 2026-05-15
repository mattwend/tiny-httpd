mod common;

use std::time::{Duration, Instant};

use hyper::{Method, StatusCode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use common::{
    TEST_DEFAULT_DRAIN_TIMEOUT_SECS, TEST_DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS,
    TEST_DEFAULT_HEADER_READ_TIMEOUT_SECS, TestServer, client,
};
use tiny_httpd::ServerParams;

async fn spawn_server(content_root: std::path::PathBuf) -> TestServer {
    TestServer::spawn(content_root).await
}

async fn spawn_server_with_idle_timeout(
    content_root: std::path::PathBuf,
    idle_connection_timeout: Duration,
) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");

    TestServer::spawn_with_params(
        listener,
        ServerParams {
            content_root: Some(content_root),
            header_read_timeout: Duration::from_secs(TEST_DEFAULT_HEADER_READ_TIMEOUT_SECS),
            idle_connection_timeout,
            graceful_close_timeout: Duration::from_secs(TEST_DEFAULT_GRACEFUL_CLOSE_TIMEOUT_SECS),
            drain_timeout: Duration::from_secs(TEST_DEFAULT_DRAIN_TIMEOUT_SECS),
        },
    )
    .await
}

async fn write_index(tempdir: &tempfile::TempDir) {
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");
}

fn server_addr(server: &TestServer) -> std::net::SocketAddr {
    server
        .uri("/")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .expect("server addr")
}

async fn read_until_hello(stream: &mut TcpStream, buffer: &mut [u8]) -> Vec<u8> {
    let mut response = Vec::new();
    loop {
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(buffer))
            .await
            .expect("response read timeout")
            .expect("read response");
        assert!(
            read > 0,
            "connection closed before first response completed"
        );
        response.extend_from_slice(&buffer[..read]);
        if response.windows(5).any(|window| window == b"hello") {
            return response;
        }
    }
}

#[tokio::test]
async fn idle_keep_alive_connections_close_promptly_on_shutdown() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let mut server = spawn_server(tempdir.path().to_path_buf()).await;

    let mut stream = TcpStream::connect(server_addr(&server))
        .await
        .expect("connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write request");

    let mut buffer = [0_u8; 1024];
    read_until_hello(&mut stream, &mut buffer).await;

    server.trigger_shutdown();

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

    server.wait().await;
}

#[tokio::test]
async fn idle_keep_alive_connections_close_promptly_after_idle_timeout() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let server =
        spawn_server_with_idle_timeout(tempdir.path().to_path_buf(), Duration::from_secs(1)).await;

    let mut stream = TcpStream::connect(server_addr(&server))
        .await
        .expect("connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write request");

    let mut buffer = [0_u8; 1024];
    read_until_hello(&mut stream, &mut buffer).await;

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

    server.shutdown().await;
}

#[tokio::test]
async fn active_keep_alive_connections_do_not_hit_idle_timeout() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let server =
        spawn_server_with_idle_timeout(tempdir.path().to_path_buf(), Duration::from_secs(1)).await;

    let mut stream = TcpStream::connect(server_addr(&server))
        .await
        .expect("connect");
    let mut buffer = [0_u8; 1024];

    for _ in 0..3 {
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .await
            .expect("write request");

        let response = read_until_hello(&mut stream, &mut buffer).await;
        let response = String::from_utf8_lossy(&response);
        assert!(response.contains("HTTP/1.1 200 OK"));

        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    server.shutdown().await;
}

#[tokio::test]
async fn shutdown_after_completed_request_still_drains_promptly() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let server = spawn_server(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/").await;
    assert_eq!(response.status(), StatusCode::OK);

    server.shutdown().await;
}

#[tokio::test]
async fn graceful_shutdown_keeps_liveness_ok_while_readiness_fails_during_drain_window() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let mut server = spawn_server(tempdir.path().to_path_buf()).await;

    let before = client()
        .get(server.uri("/readyz").parse().expect("uri"))
        .await
        .expect("readyz before shutdown");
    assert_eq!(before.status(), StatusCode::OK);

    server.trigger_shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let during_drain = client()
        .get(server.uri("/readyz").parse().expect("uri"))
        .await
        .expect("fresh readyz connection during shutdown drain window");
    assert_eq!(during_drain.status(), StatusCode::SERVICE_UNAVAILABLE);

    let livez_during_drain = client()
        .get(server.uri("/livez").parse().expect("uri"))
        .await
        .expect("fresh livez connection during shutdown drain window");
    assert_eq!(livez_during_drain.status(), StatusCode::OK);

    server.wait().await;
}

#[tokio::test]
async fn shutdown_flips_probe_states_before_listener_stops_accepting() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let mut server = spawn_server(tempdir.path().to_path_buf()).await;

    let mut stream = TcpStream::connect(server_addr(&server))
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

    server.trigger_shutdown();

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
    tokio::time::sleep(Duration::from_millis(300)).await;

    let refused = client
        .get(server.uri("/readyz").parse().expect("uri"))
        .await;
    assert!(
        refused.is_err(),
        "listener should stop accepting new connections after the readiness drain window"
    );

    drop(client);

    server.wait().await;
}

#[tokio::test]
async fn graceful_shutdown_stops_accepting_promptly_without_new_connections() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let server = spawn_server(tempdir.path().to_path_buf()).await;

    server.shutdown().await;
}

#[tokio::test]
async fn server_serves_http_requests_before_shutdown() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_index(&tempdir).await;
    let server = spawn_server(tempdir.path().to_path_buf()).await;

    let client = client();

    let readyz = client
        .get(server.uri("/readyz").parse().expect("uri"))
        .await
        .expect("readyz before shutdown");
    assert_eq!(readyz.status(), StatusCode::OK);

    let root = client
        .get(server.uri("/").parse().expect("uri"))
        .await
        .expect("root before shutdown");
    assert_eq!(root.status(), StatusCode::OK);

    server.shutdown().await;
}
