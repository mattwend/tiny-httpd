use hyper::{
    Response, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
};
use tracing::error;

use crate::handler::response::{ResponseBody, empty_response_body, full_body, response_builder};

const DEFAULT_INDEX: &str = include_str!("../default_index.html");

pub(crate) fn default_index_response(head_only: bool) -> Response<ResponseBody> {
    let body = if head_only {
        empty_response_body()
    } else {
        full_body(DEFAULT_INDEX)
    };

    response_builder(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(CONTENT_LENGTH, DEFAULT_INDEX.len())
        .body(body)
        .unwrap_or_else(|error| {
            error!(error = %error, "failed to build embedded default page response");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(CONTENT_TYPE, "text/plain; charset=utf-8")
                .header(CONTENT_LENGTH, "22")
                .body(full_body("internal server error\n"))
                .unwrap_or_else(|fallback_error| {
                    error!(error = %fallback_error, "failed to build fallback internal error response for embedded default page");
                    Response::new(empty_response_body())
                })
        })
}
