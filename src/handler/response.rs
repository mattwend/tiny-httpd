use std::{error::Error, fmt::Display};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{BodyExt, Empty, Full, StreamBody, combinators::BoxBody};
use hyper::{
    Response, StatusCode,
    body::Frame,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use tracing::error;

use crate::fs::{ResolveError, resolve_file};

/// Boxed response body type used by all HTTP responses.
pub(crate) type ResponseBody = BoxBody<Bytes, Box<dyn Error + Send + Sync>>;

/// HTTP response plus body-size metadata for metrics and tracing.
#[must_use]
pub(crate) struct ResponseOutcome {
    /// Final HTTP response.
    pub(crate) response: Response<ResponseBody>,
    /// Response body size in bytes before HEAD-body stripping.
    pub(crate) body_size: u64,
}

impl ResponseOutcome {
    /// Wraps a response with explicit body-size metadata.
    ///
    /// # Arguments
    /// * `response` - Final HTTP response to return to the caller.
    /// * `body_size` - Response body size in bytes before HEAD-body stripping.
    ///
    /// # Returns
    /// A response outcome with caller-supplied body-size metadata.
    pub(crate) fn new(response: Response<ResponseBody>, body_size: u64) -> Self {
        Self {
            response,
            body_size,
        }
    }
}

/// Builds a file-serving response for a resolved request path.
///
/// # Arguments
/// * `content_root` - Canonical content root used for safe path resolution.
/// * `path` - Request URI path to resolve and serve.
/// * `head_only` - When `true`, omits the body while preserving headers.
///
/// # Returns
/// A `200 OK` response with guessed MIME type and content length headers.
///
/// # Errors
/// Returns [`ResolveError`] when request-path resolution or file access fails.
pub(crate) async fn file_response(
    content_root: &std::path::Path,
    path: &str,
    head_only: bool,
) -> Result<ResponseOutcome, ResolveError> {
    let resolved = resolve_file(content_root, path).await?;
    let content_type = mime_guess::from_path(&resolved.canonical_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();

    let body = if head_only {
        empty_response_body()
    } else {
        stream_body(resolved.file)
    };

    match Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, resolved.content_length)
        .body(body)
    {
        Ok(response) => Ok(ResponseOutcome::new(response, resolved.content_length)),
        Err(error) => Ok(internal_error_response(
            "failed to build file response",
            error,
        )),
    }
}

/// Builds a plain-text response outcome with default text content type.
///
/// # Arguments
/// * `status` - HTTP status code for response.
/// * `body` - Static UTF-8 response body.
///
/// # Returns
/// A response outcome with the text response and exact body size.
pub(crate) fn text_response(status: StatusCode, body: &'static str) -> ResponseOutcome {
    text_response_with_headers(status, body, &[])
}

/// Builds a plain-text response outcome with caller-supplied extra headers.
///
/// # Arguments
/// * `status` - HTTP status code for response.
/// * `body` - Static UTF-8 response body.
/// * `headers` - Extra header name/value pairs appended to response.
///
/// # Returns
/// A response outcome with the text response and exact body size.
pub(crate) fn text_response_with_headers(
    status: StatusCode,
    body: &'static str,
    headers: &[(&'static str, &'static str)],
) -> ResponseOutcome {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, body.len());

    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }

    match builder.body(full_body(body)) {
        Ok(response) => ResponseOutcome::new(response, body.len() as u64),
        Err(error) => internal_error_response("failed to build text response", error),
    }
}

/// Boxes in-memory body bytes into shared response body type.
pub(crate) fn full_body<T>(body: T) -> ResponseBody
where
    T: Into<Bytes>,
{
    Full::new(body.into())
        .map_err(|never| match never {})
        .boxed()
}

/// Returns empty boxed response body for bodyless responses.
pub(crate) fn empty_response_body() -> ResponseBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// Streams file contents into shared response body type.
fn stream_body(file: File) -> ResponseBody {
    let stream = ReaderStream::new(file)
        .map_ok(Frame::data)
        .map(|result| result.map_err(|error| -> Box<dyn Error + Send + Sync> { Box::new(error) }));
    BodyExt::boxed(StreamBody::new(stream))
}

/// Logs response-construction failure and falls back to generic `500`.
pub(crate) fn internal_error_response<T>(context: &'static str, error: T) -> ResponseOutcome
where
    T: Display,
{
    error!(error = %error, context, "failed to construct HTTP response");
    let body = "internal server error\n";
    let response = Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, body.len())
        .body(full_body(body))
        .unwrap_or_else(|_| Response::new(full_body(body)));

    ResponseOutcome::new(response, body.len() as u64)
}

#[cfg(test)]
mod tests {
    use hyper::StatusCode;

    use super::{internal_error_response, text_response};

    #[test]
    fn text_response_carries_explicit_body_size() {
        let body = "hello\n";
        let outcome = text_response(StatusCode::OK, body);

        assert_eq!(outcome.body_size, body.len() as u64);
    }

    #[test]
    fn internal_error_response_reports_fallback_body_size() {
        let outcome = internal_error_response("test fallback", "boom");

        assert_eq!(outcome.body_size, "internal server error\n".len() as u64);
    }
}
