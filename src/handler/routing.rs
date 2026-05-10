use std::{
    convert::Infallible,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use hyper::{Method, Request, Response, StatusCode, header::ALLOW};
use tokio::fs;
use tracing::{Instrument, Span, debug, debug_span, info_span, warn};

use crate::{
    fs::ResolveError,
    handler::{
        default_page::default_index_outcome,
        response::{
            ResponseBody, ResponseOutcome, empty_response_body, file_response, text_response,
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
///
/// Generic body type `B` is accepted because request bodies are not consumed by
/// this handler.
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
    let span = request_span(
        &method,
        &path,
        peer_addr.as_deref().unwrap_or(""),
        path == "/livez" || path == "/readyz",
    );

    async move {
        let started = Instant::now();
        let mut in_flight = state.metrics().request_started();
        let outcome = route(Arc::clone(&state), &method, &path).await;
        let status = outcome.response.status().as_u16();
        let response_body_size = outcome.body_size;
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
        Ok(outcome.response)
    }
    .instrument(span)
    .await
}

/// Routes one request path and method to probes, static files, or fallback page.
async fn route(state: Arc<AppState>, method: &Method, path: &str) -> ResponseOutcome {
    if path == "/livez" {
        debug!("liveness probe handled");
        return probe_response(method == Method::HEAD, StatusCode::OK, "ok\n");
    }

    if path == "/readyz" {
        let readable = match &state.content_root {
            None => true,
            Some(content_root) => match fs::metadata(content_root).await {
                Ok(metadata) => metadata.is_dir(),
                Err(error) => {
                    warn!(error = %error, path = %content_root.display(), "failed to inspect content root for readiness probe");
                    false
                }
            },
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

    let Some(content_root) = &state.content_root else {
        return fallback_default_response(path, method == Method::HEAD);
    };

    match file_response(content_root, path, method == Method::HEAD).await {
        Ok(response) => response,
        Err(ResolveError::NotFound) if path == "/" => default_index_outcome(method == Method::HEAD),
        Err(error) => map_file_error(error),
    }
}

/// Serves embedded index for `/` when content root unavailable, else `404`.
fn fallback_default_response(path: &str, head_only: bool) -> ResponseOutcome {
    if path == "/" {
        default_index_outcome(head_only)
    } else {
        text_response(StatusCode::NOT_FOUND, "not found\n")
    }
}

/// Builds request span with common HTTP tracing fields.
fn request_span(method: &Method, path: &str, peer_addr: &str, debug_probe: bool) -> Span {
    macro_rules! http_request_span {
        ($span_macro:ident) => {
            $span_macro!(
                "http.request",
                http.request.method = %method,
                url.path = %path,
                network.peer.address = peer_addr,
                http.response.status_code = tracing::field::Empty,
                http.response.status_class = tracing::field::Empty,
                http.response.body.size = tracing::field::Empty,
                http.server.request.duration_us = tracing::field::Empty,
            )
        };
    }

    if debug_probe {
        http_request_span!(debug_span)
    } else {
        http_request_span!(info_span)
    }
}

/// Maps file-resolution failures into client-safe HTTP response outcomes.
///
/// # Arguments
/// * `error` - The file resolution error to map.
///
/// # Returns
/// A response outcome with the appropriate HTTP status and explicit body size.
fn map_file_error(error: ResolveError) -> ResponseOutcome {
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

/// Builds liveness or readiness response outcome, preserving `Content-Length` for `HEAD`.
///
/// # Arguments
/// * `head_only` - When `true`, omits the body while preserving headers.
/// * `status` - HTTP status code for the probe response.
/// * `body` - Static UTF-8 response body text.
///
/// # Returns
/// A response outcome with explicit body size metadata.
fn probe_response(head_only: bool, status: StatusCode, body: &'static str) -> ResponseOutcome {
    let mut outcome = text_response(status, body);
    if head_only {
        *outcome.response.body_mut() = empty_response_body();
    }
    outcome
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
        let outcome = map_file_error(ResolveError::Io(io::Error::other("synthetic failure")));

        assert_eq!(outcome.body_size, 22);
        assert_eq!(outcome.response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            outcome
                .response
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .expect("content type header"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            outcome
                .response
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .expect("content length header"),
            "22"
        );

        let body = outcome
            .response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&body[..], b"internal server error\n");
    }

    #[tokio::test]
    async fn not_found_maps_to_404() {
        let outcome = map_file_error(ResolveError::NotFound);

        assert_eq!(outcome.body_size, 10);
        assert_eq!(outcome.response.status(), StatusCode::NOT_FOUND);
        let body = outcome
            .response
            .into_body()
            .collect()
            .await
            .expect("collect body")
            .to_bytes();
        assert_eq!(&body[..], b"not found\n");
    }

    #[tokio::test]
    async fn malformed_paths_map_to_400() {
        let outcome = map_file_error(ResolveError::Traversal);

        assert_eq!(outcome.body_size, 12);
        assert_eq!(outcome.response.status(), StatusCode::BAD_REQUEST);
        let body = outcome
            .response
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
        assert_eq!(livez.response.status(), StatusCode::OK);

        let readyz = probe_response(false, StatusCode::SERVICE_UNAVAILABLE, "not ready\n");
        assert_eq!(readyz.response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
