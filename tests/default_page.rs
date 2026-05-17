// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

mod common;

use http_body_util::BodyExt;
use hyper::{
    Method, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use tiny_httpd::ServerParams;
use tokio::net::TcpListener;

use common::TestServer;

#[tokio::test]
async fn empty_content_root_dir_serves_default_page_at_root() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body_text = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(body_text.contains("<h1>tiny-httpd</h1>"));
    assert!(body_text.contains("No content root configured yet."));

    server.shutdown().await;
}

#[tokio::test]
async fn empty_content_root_dir_returns_404_for_other_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/other").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("not found body")
        .to_bytes();
    assert_eq!(&body[..], b"not found\n");

    server.shutdown().await;
}

#[tokio::test]
async fn missing_content_root_starts_and_serves_default_page() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let mut server = TestServer::spawn_with_params(
        listener,
        ServerParams {
            content_root: None,
            ..ServerParams::default()
        },
    )
    .await;

    let response = server.request(Method::GET, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body_text = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(body_text.contains("<h1>tiny-httpd</h1>"));

    server.shutdown().await;
}

#[tokio::test]
async fn missing_content_root_returns_404_for_other_paths() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let mut server = TestServer::spawn_with_params(
        listener,
        ServerParams {
            content_root: None,
            ..ServerParams::default()
        },
    )
    .await;

    let response = server.request(Method::GET, "/other").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("not found body")
        .to_bytes();
    assert_eq!(&body[..], b"not found\n");

    server.shutdown().await;
}

#[tokio::test]
async fn user_index_takes_precedence_over_default_page() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "user content")
        .await
        .expect("write index");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(&body[..], b"user content");

    server.shutdown().await;
}

#[tokio::test]
async fn head_default_page_returns_headers_with_empty_body() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::HEAD, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert!(response.headers().get(CONTENT_LENGTH).is_some());
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(body.is_empty());

    server.shutdown().await;
}
