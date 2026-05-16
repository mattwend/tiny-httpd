use std::io;

use tiny_httpd::run_with_shutdown;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn shutdown_signal_failure_marks_readiness_unavailable() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        run_with_shutdown(listener, Default::default(), || async {
            Err(io::Error::other("synthetic shutdown failure"))
        })
        .await
    });

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut request = Vec::new();
    request.extend_from_slice(b"GET /readyz HTTP/1.1\r\n");
    request.extend_from_slice(b"Host: localhost\r\n");
    request.extend_from_slice(b"Connection: close\r\n\r\n");
    stream.write_all(&request).await.expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");

    let response_text = String::from_utf8(response).expect("response utf-8");
    assert!(
        response_text.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "unexpected response: {response_text}"
    );
    assert!(response_text.ends_with("\r\n\r\nnot ready\n"));

    server
        .await
        .expect("server task join")
        .expect("server result");
}
