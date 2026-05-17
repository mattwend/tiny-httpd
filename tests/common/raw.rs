// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use super::TestServer;

/// Returns the TCP socket address backing a test server.
pub fn server_addr(server: &TestServer) -> SocketAddr {
    server
        .uri("/")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .expect("server addr")
}

/// Sends one raw HTTP/1 keep-alive root request.
pub async fn write_keep_alive_root_request(stream: &mut TcpStream) {
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write request");
}

/// Reads a raw HTTP response until its body contains `hello`.
pub async fn read_until_hello(stream: &mut TcpStream, buffer: &mut [u8]) -> Vec<u8> {
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
