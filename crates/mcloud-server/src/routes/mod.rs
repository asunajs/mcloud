//! 路由定义。

pub mod accounts;
pub mod auth;
pub mod beans;
pub mod cancel;
pub mod capabilities;
pub mod exchange;
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
        // 今日AI豆（云朵记录汇总）—— **只读**：按账号读流水，绝不合计。
        // 一个账号读不到进响应的 `failed`，不打整张卡（见 beans.rs 模块头）。
        .merge(beans::routes())
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::get_one))
        .route("/api/runs/{id}/events", get(runs::events))
        // 中止这次运行。202 语义：请求已受理，不表示已经停了。
        .route("/api/runs/{id}/cancel", post(cancel::cancel_run))
        // 运行日志（支持 ?after= 断线续传）
        .route("/api/runs/{id}/logs", get(logs::list))
        // ── 云盘账号（列表 / 导入凭据 / 编辑 / 移除）────────────────
        // `POST` = 用一条凭据建账号（手机号已存在则**只换凭据**，见 accounts::import）；
        // `PATCH` = 改昵称 / 设备号 / 凭据 —— **部分更新**，理由见 accounts::update。
        .route("/api/accounts", get(accounts::list).post(accounts::import))
        .route(
            "/api/accounts/{id}",
            axum::routing::delete(accounts::remove).patch(accounts::update),
        )
        // 每账号设置：GET 读、PUT **整体替换**（语义与取舍见 accounts::put_settings）。
        // 与上面那条 `{id}` 是不同的路由：一条删账号、一条读/写设置。
        .route(
            "/api/accounts/{id}/settings",
            get(accounts::get_settings).put(accounts::put_settings),
        )
        // ── 兑换（抢兑）────────────────────────────────────────────
        // ⚠ 上面那条 `/api/accounts/{id}` 与下面两条是**不同**的路由：
        // 一条删账号、一条读奖品清单、一条**真花AI豆**。别把后两条也
        // 合并进 `{id}` 那条 —— 写操作必须看得出它是写操作。
        .merge(exchange::routes())
        // ── 云盘账号登录（与上面的「管理员登录闸」是两件事）──────────
        .route("/api/login/qr", post(login::start_qr))
        .route("/api/login/qr/poll", post(login::poll_qr))
        .route("/api/login/qr/{session_id}", axum::routing::delete(login::cancel_qr))
        .route("/api/login/sms/start", post(login::start_sms))
        .route("/api/login/sms", post(login::login_sms))
        // App 端（手机客户端）：**不同的协议**，不是 PC 那条加了参数
        .route("/api/login/app/sms/start", post(login::app_start_sms))
        .route("/api/login/app/sms", post(login::app_login_sms))
        .route("/api/login/password", post(login::login_password))
        .route("/api/login/device", get(login::get_device))
        .route("/api/login/device/rotate", post(login::rotate_device))
        .merge(schedules::routes())
        .merge(pipelines::routes())
}
