//! mcloud-server —— HTTP 服务。
//!
//! 路由 + SSE 事件流。前端产物在 P7 阶段用 `rust-embed` 内嵌；
//! 当前开发态由 Vite dev server 提供，生产态可从 `web/dist` 直接读。

pub mod error;
pub mod routes;
pub mod state;

pub use state::ServerState;

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// 构建路由。
pub fn app(state: ServerState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .merge(routes::router())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
