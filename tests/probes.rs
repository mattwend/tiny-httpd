// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

mod common;

use http_body_util::BodyExt;
use hyper::{Method, StatusCode, header::CONTENT_TYPE};
use tiny_httpd::ServerParams;

use common::TestServer;

#[tokio::test]
async fn livez_and_readyz_have_reserved_precedence_and_plain_text_content_type() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("livez"), "static-livez")
        .await
        .expect("write livez file");
    tokio::fs::write(tempdir.path().join("readyz"), "static-readyz")
        .await
        .expect("write readyz file");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let livez = server.request(Method::GET, "/livez").await;
    assert_eq!(livez.status(), StatusCode::OK);
    assert_eq!(
        livez
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    let livez_body = livez
        .into_body()
        .collect()
        .await
        .expect("livez body")
        .to_bytes();
    assert_eq!(&livez_body[..], b"ok\n");

    let readyz = server.request(Method::GET, "/readyz").await;
    assert_eq!(readyz.status(), StatusCode::OK);
    assert_eq!(
        readyz
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    let readyz_body = readyz
        .into_body()
        .collect()
        .await
        .expect("readyz body")
        .to_bytes();
    assert_eq!(&readyz_body[..], b"ready\n");

    server.shutdown().await;
}

#[tokio::test]
async fn head_probes_return_success_with_empty_body() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let livez = server.request(Method::HEAD, "/livez").await;
    assert_eq!(livez.status(), StatusCode::OK);
    let livez_body = livez
        .into_body()
        .collect()
        .await
        .expect("livez body")
        .to_bytes();
    assert!(livez_body.is_empty());

    let readyz = server.request(Method::HEAD, "/readyz").await;
    assert_eq!(readyz.status(), StatusCode::OK);
    let readyz_body = readyz
        .into_body()
        .collect()
        .await
        .expect("readyz body")
        .to_bytes();
    assert!(readyz_body.is_empty());

    server.shutdown().await;
}

#[tokio::test]
async fn probe_routes_accept_non_get_methods() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::POST, "/livez").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("livez body")
        .to_bytes();
    assert_eq!(&body[..], b"ok\n");

    let response = server.request(Method::POST, "/readyz").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("readyz body")
        .to_bytes();
    assert_eq!(&body[..], b"ready\n");

    server.shutdown().await;
}

#[tokio::test]
async fn readyz_returns_200_when_content_root_missing_at_startup() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
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

    let ready = server.request(Method::GET, "/readyz").await;
    assert_eq!(ready.status(), StatusCode::OK);
    let body = ready
        .into_body()
        .collect()
        .await
        .expect("readyz body")
        .to_bytes();
    assert_eq!(&body[..], b"ready\n");

    server.shutdown().await;
}

#[tokio::test]
async fn readyz_returns_503_after_content_root_loss() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("public");
    tokio::fs::create_dir(&root).await.expect("create root");

    let mut server = TestServer::spawn(root.clone()).await;

    let ready = server.request(Method::GET, "/readyz").await;
    assert_eq!(ready.status(), StatusCode::OK);

    let head_ready = server.request(Method::HEAD, "/readyz").await;
    assert_eq!(head_ready.status(), StatusCode::OK);
    let head_ready_body = head_ready
        .into_body()
        .collect()
        .await
        .expect("head readyz body")
        .to_bytes();
    assert!(head_ready_body.is_empty());

    drop(tempdir);

    let unreadable = server.request(Method::GET, "/readyz").await;
    assert_eq!(unreadable.status(), StatusCode::SERVICE_UNAVAILABLE);

    server.shutdown().await;
}
