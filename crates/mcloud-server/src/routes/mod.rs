//! 路由定义。

pub mod accounts;
pub mod auth;
pub mod cancel;
pub mod capabilities;
pub mod health;
pub mod login;
pub mod logs;
pub mod pipelines;
pub mod runs;
pub mod schedules;

use axum::routing::{get, post};
use axum::Router;

use crate::ServerState;

pub fn router() -> Router<ServerState> {
    Router::new()
        // ── 登录闸（公开部分见 auth_gate::PUBLIC）────────────────────
        .route("/api/setup", get(auth::setup_status).post(auth::setup_create))
        .route(
            "/api/session",
            get(auth::current_session)
                .post(auth::login)
                .delete(auth::logout),
        )
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
        // ── 云盘账号（列表 / 移除）──────────────────────────────────
        .route("/api/accounts", get(accounts::list))
        .route("/api/accounts/{id}", axum::routing::delete(accounts::remove))
        // ── 云盘账号登录（与上面的「管理员登录闸」是两件事）──────────
        .route("/api/login/qr", post(login::start_qr))
        .route("/api/login/qr/poll", post(login::poll_qr))
        .route("/api/login/qr/{session_id}", axum::routing::delete(login::cancel_qr))
        .route("/api/login/sms/start", post(login::start_sms))
        .route("/api/login/sms", post(login::login_sms))
        .route("/api/login/password", post(login::login_password))
        .route("/api/login/device", get(login::get_device))
        .route("/api/login/device/rotate", post(login::rotate_device))
        .merge(schedules::routes())
        .merge(pipelines::routes())
}
