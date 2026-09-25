//! 路由定义。

pub mod cancel;
pub mod capabilities;
pub mod health;
pub mod logs;
pub mod pipelines;
pub mod runs;
pub mod schedules;

use axum::routing::{get, post};
use axum::Router;

use crate::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/api/version", get(health::version))
        .route("/api/status", get(health::status))
        .route("/api/health", get(health::health))
        .route("/api/capabilities", get(capabilities::list))
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::get_one))
        .route("/api/runs/{id}/events", get(runs::events))
        // 中止这次运行。202 语义：请求已受理，不表示已经停了。
        .route("/api/runs/{id}/cancel", post(cancel::cancel_run))
        // 运行日志（支持 ?after= 断线续传）
        .route("/api/runs/{id}/logs", get(logs::list))
        .merge(schedules::routes())
        .merge(pipelines::routes())
}
