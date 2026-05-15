mod common;

use http_body_util::BodyExt;
use hyper::{Method, StatusCode};

use common::TestServer;

#[tokio::test]
async fn invalid_percent_encoding_and_traversal_return_400() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let invalid = server.request(Method::GET, "/%zz").await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let traversal = server.request(Method::GET, "/%2e%2e/secret").await;
    assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);

    let encoded_slash = server.request(Method::GET, "/a%2f%2fb").await;
    assert_eq!(encoded_slash.status(), StatusCode::BAD_REQUEST);

    server.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;

    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("public");
    let outside = tempdir.path().join("secret.txt");
    tokio::fs::create_dir(&root).await.expect("create root");
    tokio::fs::write(&outside, "secret")
        .await
        .expect("write outside");
    symlink(&outside, root.join("escape.txt")).expect("symlink");

    let mut server = TestServer::spawn(root).await;
    let response = server.request(Method::GET, "/escape.txt").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    server.shutdown().await;
}

#[tokio::test]
async fn missing_direct_file_falls_back_to_directory_index() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path();
    tokio::fs::create_dir_all(root.join("docs"))
        .await
        .expect("create docs dir");
    tokio::fs::write(root.join("docs/index.html"), "docs index\n")
        .await
        .expect("write docs index");

    let mut server = TestServer::spawn(root.to_path_buf()).await;
    let response = server.request(Method::GET, "/docs").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(&body[..], b"docs index\n");

    server.shutdown().await;
}
