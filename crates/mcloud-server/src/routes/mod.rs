//! 路由定义。

pub mod accounts;
pub mod auth;
pub mod beans;
pub mod cancel;
pub mod capabilities;
pub mod exchange;
pub mod health;
pub mod kouling;
pub mod login;
pub mod logs;
pub mod pipelines;
pub mod runs;
pub mod schedules;
pub mod security;
pub mod server_logs;
pub mod system;
pub mod task_config;

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
        // ── 账户安全（都要登录）──────────────────────────────────────
        // 口令策略与会话时长是**登录闸自己的设置**，所以和 users/sessions 一样
        // 必须落在闸后面：没登录就能改下限，等于没有闸。
        .route(
            "/api/admin/security",
            get(security::get_security).put(security::put_security),
        )
        // 改口令：只认"当前口令"，不认身份 —— 会话被盗时这一条拦不住，
        // 但那是会话本身的语义（能改口令的会话本来就有全部权限）。
        .route("/api/admin/password", axum::routing::put(security::change_password))
        // ── 系统（存储 / 重启）───────────────────────────────────────
        // ── 共享口令池（直播页看它决定"还要不要抓"）──────────────────
        .route("/api/kouling", get(kouling::get_pool))
        // ── 任务的参数配置（任务页「配置」那一页）──────────────────────
        .route(
            "/api/tasks/{name}/config",
            get(task_config::get_one).put(task_config::put_one),
        )
        .route("/api/system/about", get(system::about))
        .route("/api/system/storage", get(system::storage))
        .route("/api/system/restart", post(system::restart))
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
        // **全量**服务端日志（环形缓冲 ✓，含调度器与上游 http ✓）——
        // 与上面那条的区别见 server_logs.rs 的模块头 ✓。
        .route("/api/logs", get(server_logs::list))
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
