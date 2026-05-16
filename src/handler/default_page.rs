use hyper::{
    Response, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use tracing::error;

use crate::handler::response::{
    ResponseOutcome, empty_response_body, full_body, internal_error_response,
};

pub(crate) const DEFAULT_INDEX_HTML: &str = include_str!("../default_index.html");

/// Builds response outcome for embedded fallback index page.
///
/// # Arguments
/// * `head_only` - When `true`, omits the body while preserving headers.
///
/// # Returns
/// A response outcome with the embedded page response and exact body size.
pub(crate) fn default_index_outcome(head_only: bool) -> ResponseOutcome {
    let body = if head_only {
        empty_response_body()
    } else {
        full_body(DEFAULT_INDEX_HTML)
    };

    match Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CONTENT_LENGTH, DEFAULT_INDEX_HTML.len())
        .body(body)
    {
        Ok(response) => ResponseOutcome::new(response, DEFAULT_INDEX_HTML.len() as u64),
        Err(error) => {
            error!(error = %error, "failed to build embedded default page response");
            internal_error_response("failed to build embedded default page response", error)
        }
    }
}
