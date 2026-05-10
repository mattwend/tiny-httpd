//! HTTP request handling, routing, and response helpers.

mod default_page;
mod response;
mod routing;
mod state;

pub(crate) use routing::handle_with_peer_addr;
pub(crate) use state::AppState;
