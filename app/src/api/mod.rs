pub mod me;

use axum::Router;
use crate::AppState;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .nest("/me", axum::Router::new().route("/", axum::routing::get(me::handler_me)))
}
