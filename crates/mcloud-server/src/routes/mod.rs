//! 路由定义。

pub mod capabilities;
pub mod health;
pub mod runs;

use axum::routing::get;
use axum::Router;

use crate::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/api/version", get(health::version))
        .route("/api/health", get(health::health))
        .route("/api/capabilities", get(capabilities::list))
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::get_one))
        .route("/api/runs/{id}/events", get(runs::events))
}
