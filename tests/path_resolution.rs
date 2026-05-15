mod common;

use http_body_util::BodyExt;
use hyper::{Method, StatusCode, header::CONTENT_LENGTH};

use common::TestServer;

#[tokio::test]
async fn resolves_root_dir_and_dir_slash_to_index() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "home")
        .await
        .expect("write root index");
    tokio::fs::create_dir(tempdir.path().join("docs"))
        .await
        .expect("create docs dir");
    tokio::fs::write(tempdir.path().join("docs/index.html"), "docs")
        .await
        .expect("write docs index");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let root = server.request(Method::GET, "/").await;
    assert_eq!(root.status(), StatusCode::OK);
    let root_body = root
        .into_body()
        .collect()
        .await
        .expect("root body")
        .to_bytes();
    assert_eq!(&root_body[..], b"home");

    let dir = server.request(Method::GET, "/docs").await;
    assert_eq!(dir.status(), StatusCode::OK);
    let dir_body = dir
        .into_body()
        .collect()
        .await
        .expect("dir body")
        .to_bytes();
    assert_eq!(&dir_body[..], b"docs");

    let dir_slash = server.request(Method::GET, "/docs/").await;
    assert_eq!(dir_slash.status(), StatusCode::OK);
    let dir_slash_body = dir_slash
        .into_body()
        .collect()
        .await
        .expect("dir slash body")
        .to_bytes();
    assert_eq!(&dir_slash_body[..], b"docs");

    server.shutdown().await;
}

#[tokio::test]
async fn explicit_file_paths_resolve_directly() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("foo.html"), "page")
        .await
        .expect("write foo.html");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let file = server.request(Method::GET, "/foo.html").await;
    assert_eq!(file.status(), StatusCode::OK);
    let file_body = file
        .into_body()
        .collect()
        .await
        .expect("file body")
        .to_bytes();
    assert_eq!(&file_body[..], b"page");

    server.shutdown().await;
}

#[tokio::test]
async fn file_then_directory_index_fallback_matches_rfc() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::create_dir(tempdir.path().join("docs"))
        .await
        .expect("create docs dir");
    tokio::fs::write(tempdir.path().join("docs/index.html"), "docs")
        .await
        .expect("write docs index");
    tokio::fs::create_dir(tempdir.path().join("other"))
        .await
        .expect("create other dir");
    tokio::fs::write(tempdir.path().join("other/index.html"), "other")
        .await
        .expect("write other index");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let docs = server.request(Method::GET, "/docs").await;
    assert_eq!(docs.status(), StatusCode::OK);

    let missing = server.request(Method::GET, "/lonely").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    server.shutdown().await;
}

#[tokio::test]
async fn missing_files_return_404() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/missing").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some("10")
    );

    server.shutdown().await;
}
