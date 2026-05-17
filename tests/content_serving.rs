// SPDX-FileCopyrightText: 2026 Matthias Wende
// SPDX-License-Identifier: GPL-3.0-or-later

mod common;

use http_body_util::BodyExt;
use hyper::{Method, StatusCode, header::CONTENT_TYPE};

use common::TestServer;

#[tokio::test]
async fn serves_mime_types_for_common_extensions() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let fixtures = [
        ("site.css", "body {}", "text/css"),
        ("app.js", "console.log('ok');", "text/javascript"),
        ("data.json", "{\"ok\":true}", "application/json"),
    ];

    for (name, body, _) in fixtures {
        tokio::fs::write(tempdir.path().join(name), body)
            .await
            .expect("write fixture");
    }

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    for (name, body, content_type) in fixtures {
        let response = server.request(Method::GET, &format!("/{name}")).await;
        assert_eq!(response.status(), StatusCode::OK, "path: /{name}");
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some(content_type),
            "path: /{name}"
        );
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&bytes[..], body.as_bytes(), "path: /{name}");
    }

    server.shutdown().await;
}

#[tokio::test]
async fn serves_percent_encoded_filenames_end_to_end() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(tempdir.path().join("space name.html"), "space")
        .await
        .expect("write spaced file");
    tokio::fs::write(tempdir.path().join("café.html"), "accent")
        .await
        .expect("write unicode file");

    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let spaced = server.request(Method::GET, "/space%20name.html").await;
    assert_eq!(spaced.status(), StatusCode::OK);
    let spaced_body = spaced
        .into_body()
        .collect()
        .await
        .expect("collect spaced body")
        .to_bytes();
    assert_eq!(&spaced_body[..], b"space");

    let unicode = server.request(Method::GET, "/caf%C3%A9.html").await;
    assert_eq!(unicode.status(), StatusCode::OK);
    let unicode_body = unicode
        .into_body()
        .collect()
        .await
        .expect("collect unicode body")
        .to_bytes();
    assert_eq!(&unicode_body[..], b"accent");

    server.shutdown().await;
}

#[tokio::test]
async fn rejects_null_byte_in_request_path_over_http() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut server = TestServer::spawn(tempdir.path().to_path_buf()).await;

    let response = server.request(Method::GET, "/%00secret").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert_eq!(&body[..], b"bad request\n");

    server.shutdown().await;
}
