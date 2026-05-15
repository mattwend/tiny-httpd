mod common;

use hyper::{
    Method, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};

use common::TestServer;

#[tokio::test]
async fn successful_file_responses_set_content_type_and_content_length() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("index.html"), "hello")
        .await
        .expect("write index");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/html")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()),
        Some("5")
    );

    server.shutdown().await;
}
