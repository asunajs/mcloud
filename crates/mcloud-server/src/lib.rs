//! mcloud-server —— HTTP 服务。
//!
//! 路由 + SSE 事件流 + 前端产物内嵌。
//!
//! 生产形态是**同源**：后端同时发 API 与前端产物。
//! 开发态走 Vite dev server，用 `vite.config.ts` 的 `/api` 代理抹平差异
//! （见 webui-code-guide §7.4：刻意不用 CORS，避免"开发能登录、打包上去就登录不了"）。

pub mod auth_gate;
pub mod cli;
pub mod error;
pub mod qr;
pub mod routes;
pub mod state;
pub mod static_files;

pub use state::ServerState;

use axum::middleware;
use axum::Router;
use tower_http::trace::TraceLayer;

/// 构建路由。
pub fn app(state: ServerState) -> Router {
    Router::new()
        .merge(routes::router())
        // 兜底：非 /api 的路径交给内嵌前端（SPA）。放在最后，
        // 保证接口路由优先匹配 —— 顺序反了会把 API 也吞进前端回落。
        .fallback(static_files::serve)
        // ⚠ 登录闸必须在 `with_state` **之后**加：
        // `from_fn_with_state` 要拿到 state 才能查会话。
        .layer(middleware::from_fn_with_state(state.clone(), auth_gate::gate))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
