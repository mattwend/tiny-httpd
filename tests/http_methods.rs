// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

mod common;

use http_body_util::BodyExt;
use hyper::{
    Method, StatusCode,
    header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE},
};

use common::TestServer;

#[tokio::test]
async fn get_and_head_behavior() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let get = server.request(Method::GET, "/").await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_content_type = get
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("GET content type");
    let get_content_length = get
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("GET content length");
    let get_body = get
        .into_body()
        .collect()
        .await
        .expect("get body")
        .to_bytes();
    assert_eq!(&get_body[..], b"hello");

    let head = server.request(Method::HEAD, "/").await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(get_content_type.as_str())
    );
    assert_eq!(
        head.headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(get_content_length.as_str())
    );
    let head_body = head
        .into_body()
        .collect()
        .await
        .expect("head body")
        .to_bytes();
    assert!(head_body.is_empty());

    server.shutdown().await;
}

#[tokio::test]
async fn head_for_non_root_file_preserves_headers_without_body() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("foo.html"), "page")
        .await
        .expect("write file");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let get = server.request(Method::GET, "/foo.html").await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_content_type = get
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("GET content type");
    let get_content_length = get
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("GET content length");
    let get_body = get
        .into_body()
        .collect()
        .await
        .expect("get body")
        .to_bytes();
    assert_eq!(&get_body[..], b"page");

    let head = server.request(Method::HEAD, "/foo.html").await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(get_content_type.as_str())
    );
    assert_eq!(
        head.headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some(get_content_length.as_str())
    );
    let head_body = head
        .into_body()
        .collect()
        .await
        .expect("head body")
        .to_bytes();
    assert!(head_body.is_empty());

    server.shutdown().await;
}

#[tokio::test]
async fn unsupported_methods_return_405_and_allow_header() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::POST, "/").await;

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers().get(ALLOW).and_then(|v| v.to_str().ok()),
        Some("GET, HEAD")
    );

    server.shutdown().await;
}
