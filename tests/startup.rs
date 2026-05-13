use tiny_httpd::{Config, ServerError, startup, telemetry};

#[test]
fn telemetry_init_errors_are_propagated_at_startup() {
    assert!(telemetry::init_with_stdout_filter("tiny-httpd-test", "tiny-httpd=[").is_err());
}

#[tokio::test]
async fn startup_succeeds_when_content_root_is_missing() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let missing = tempdir.path().join("missing");
    let config = Config {
        listen_addr: "127.0.0.1:0".parse().expect("listen addr"),
        content_root: missing,
        service_name: "tiny-httpd-test".to_string(),
        ..Config::default()
    };

    let startup = startup(&config)
        .await
        .expect("missing content root should not fail");
    assert!(startup.content_root().is_none());
}

#[tokio::test]
async fn startup_fails_when_content_root_is_not_a_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let file = tempdir.path().join("index.html");
    tokio::fs::write(&file, "hello").await.expect("write file");
    let config = Config {
        listen_addr: "127.0.0.1:0".parse().expect("listen addr"),
        content_root: file.clone(),
        service_name: "tiny-httpd-test".to_string(),
        ..Config::default()
    };

    let error = startup(&config)
        .await
        .err()
        .expect("file content root should fail");
    assert!(matches!(error, ServerError::ContentRootNotDirectory(path) if path == file));
}
