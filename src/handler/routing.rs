use std::{
    convert::Infallible,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use hyper::{
    Method, Request, Response, StatusCode,
    header::{ALLOW, CONTENT_LENGTH},
};
use tokio::fs;
use tracing::{Instrument, debug, debug_span, info_span, warn};

use crate::{
    fs::ResolveError,
    handler::{
        response::{
            ResponseBody, empty_response_body, file_response, text_response,
            text_response_with_headers,
        },
        state::AppState,
    },
};

/// Handles one HTTP request with optional peer address metadata.
///
/// # Arguments
/// * `state` - Shared application state.
/// * `request` - Hyper request to process.
/// * `peer_addr` - Remote peer socket address when available from the accept loop.
///
/// # Returns
/// A Hyper response. Handler errors are converted into HTTP status codes, so the
/// outer result is infallible for Hyper service integration.
pub async fn handle_with_peer_addr<B>(
    state: Arc<AppState>,
    request: Request<B>,
    peer_addr: Option<std::net::SocketAddr>,
) -> Result<Response<ResponseBody>, Infallible>
where
    B: Send + 'static,
{
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let peer_addr = peer_addr.map(|addr| addr.to_string());
    let span = if path == "/livez" || path == "/readyz" {
        debug_span!(
            "http.request",
            http.request.method = %method,
            url.path = %path,
            network.peer.address = peer_addr.as_deref().unwrap_or(""),
            http.response.status_code = tracing::field::Empty,
            http.response.status_class = tracing::field::Empty,
            http.response.body.size = tracing::field::Empty,
            http.server.request.duration_us = tracing::field::Empty,
        )
    } else {
        info_span!(
            "http.request",
            http.request.method = %method,
            url.path = %path,
            network.peer.address = peer_addr.as_deref().unwrap_or(""),
            http.response.status_code = tracing::field::Empty,
            http.response.status_class = tracing::field::Empty,
            http.response.body.size = tracing::field::Empty,
            http.server.request.duration_us = tracing::field::Empty,
        )
    };

    async move {
        let started = Instant::now();
        let mut in_flight = state.metrics().request_started();
        let response = route(Arc::clone(&state), &method, &path).await;
        let status = response.status().as_u16();
        let response_body_size = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let elapsed = started.elapsed();
        tracing::Span::current().record("http.response.status_code", status);
        tracing::Span::current().record(
            "http.response.status_class",
            crate::metrics::status_class(status),
        );
        tracing::Span::current().record("http.response.body.size", response_body_size);
        tracing::Span::current().record(
            "http.server.request.duration_us",
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
        );
        in_flight.finish();
        state
            .metrics()
            .request_finished(method.as_str(), status, elapsed, response_body_size);
        Ok(response)
    }
    .instrument(span)
    .await
}

async fn route(state: Arc<AppState>, method: &Method, path: &str) -> Response<ResponseBody> {
    if path == "/livez" {
        debug!("liveness probe handled");
        return probe_response(method == Method::HEAD, StatusCode::OK, "ok\n");
    }

    if path == "/readyz" {
        let readable = match fs::metadata(&state.content_root).await {
            Ok(metadata) => metadata.is_dir(),
            Err(error) => {
                warn!(error = %error, path = %state.content_root.display(), "failed to inspect content root for readiness probe");
                false
            }
        };
        if state.ready.load(Ordering::SeqCst) && readable {
            debug!("readiness probe passed");
            return probe_response(method == Method::HEAD, StatusCode::OK, "ready\n");
        }
        warn!(
            ready = state.ready.load(Ordering::SeqCst),
            readable, "readiness probe failed"
        );
        return probe_response(
            method == Method::HEAD,
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready\n",
        );
    }

    if method != Method::GET && method != Method::HEAD {
        return text_response_with_headers(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed\n",
            &[(ALLOW.as_str(), "GET, HEAD")],
        );
    }

    if state.shutting_down.load(Ordering::SeqCst) {
        debug!("rejecting non-probe request during shutdown drain");
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "not ready\n");
    }

    match file_response(&state.content_root, path, method == Method::HEAD).await {
        Ok(response) => response,
        Err(error) => map_file_error(error),
    }
}

fn map_file_error(error: ResolveError) -> Response<ResponseBody> {
    match error {
        ResolveError::BadTarget
        | ResolveError::InvalidPercentEncoding
        | ResolveError::InvalidUtf8
        | ResolveError::EncodedSlash
        | ResolveError::NullByte
        | ResolveError::Traversal
        | ResolveError::Escape => text_response(StatusCode::BAD_REQUEST, "bad request\n"),
        ResolveError::NotFound => text_response(StatusCode::NOT_FOUND, "not found\n"),
        ResolveError::Io(error) => {
            warn!(error = %error, "I/O error while serving file");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "internal server error\n")
        }
    }
}

fn probe_response(
    head_only: bool,
    status: StatusCode,
    body: &'static str,
) -> Response<ResponseBody> {
    let mut response = text_response(status, body);
    if head_only {
        *response.body_mut() = empty_response_body();
    }
    response
}

#[cfg(test)]
mod tests {
    use std::io;

    use http_body_util::BodyExt;
    use hyper::StatusCode;

    use super::{map_file_error, probe_response};
    use crate::fs::ResolveError;

    #[tokio::test]
    async fn io_errors_map_to_500_with_internal_error_body() {
        let response = map_file_error(ResolveError::Io(io::Error::other("synthetic failure")));

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .expect("content type header"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .expect("content length header"),
            "22"
        );

        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&body[..], b"internal server error\n");
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let response = map_file_error(ResolveError::NotFound);

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&body[..], b"not found\n");
    }

    #[tokio::test]
    async fn malformed_paths_map_to_400() {
        let response = map_file_error(ResolveError::Traversal);

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&body[..], b"bad request\n");
    }

    #[tokio::test]
    async fn probe_responses_ignore_http_method() {
        let livez = probe_response(false, StatusCode::OK, "ok\n");
        assert_eq!(livez.status(), StatusCode::OK);

        let readyz = probe_response(false, StatusCode::SERVICE_UNAVAILABLE, "not ready\n");
        assert_eq!(readyz.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
