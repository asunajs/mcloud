//! 路由定义。

pub mod accounts;
pub mod auth;
/// 「凭据刷新」：手动一键刷新 + 自动定时的开关与预设时间
///（`POST /api/auth/refresh` / `GET|PUT /api/auth/refresh-schedule` ✓）。
pub mod auth_refresh;
pub mod beans;
pub mod cancel;
pub mod capabilities;
/// 「导出配置」（设置页那枚按钮）：`GET /api/config/export`。
///
/// ⚠ 与 CLI 的 `mcloud config export` **共用** [`crate::config_doc`] 那一份实现；
/// 区别只在"导谁的" —— 这里导**当前登录用户自己**那份（见模块头）。
pub mod config;
pub mod exchange;
pub mod health;
pub mod kouling;
/// 「直播自动定时」（设置页那张卡）：`GET/PUT /api/live-room/auto-schedule`。
/// 落盘是**那条普通定时**（`schedules.json` 里的 `auto-live-<owner>` ✓），
/// 不是新开的一张表 ✓ —— 见模块头。
pub mod live_room;
pub mod login;
pub mod logs;
pub mod pipelines;
pub mod push;
pub mod runs;
pub mod schedules;
pub mod security;
pub mod server_logs;
pub mod system;
pub mod task_config;
pub mod upstream;

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::ServerState;

/// 实例级**条数上限**（2026-10-06 安全审计 `SEC-16`）。
///
/// # 为什么需要它
///
/// 存储层每次写都是"**整文件重写 + fsync**"（`json_store.rs` 的
/// `persist_*`），所以 n 条记录的写入总代价是 O(n²)：1000 条账号意味着
/// 每加一条就重写一次越来越大的文件 ✗。今天没有上限，而"加账号"是
/// 免认证的？不 —— 它要登录；但**单管理员也可能脚本化地灌进去**，
/// 而且灌爆之后恢复要手工改 JSON。
///
/// 上限取"正常用量的两个数量级"：本工具是一个人管几个到几十个云盘账号，
/// 200 个账号 / 500 条定时 / 200 条编排**远超**正常使用，只挡"灌爆"这一种形态 ✓。
/// 真到了那个量级，该换的是存储后端（SQLite），不是把上限调大。
pub(crate) const MAX_ACCOUNTS: usize = 200;
pub(crate) const MAX_SCHEDULES: usize = 500;
pub(crate) const MAX_PIPELINES: usize = 200;

/// 认证面两条路由的**硬超时**（2026-10-06 安全审计 `SEC-01` 的一部分）。
///
/// # 为什么只给这两条、而不是整站挂一层
///
/// `TimeoutLayer` 掐的是**整个响应 future**，而本仓有两条路由是**故意长跑**的：
/// `/api/runs/{id}/events`（SSE，运行结束才收流）与同步的 `POST /api/runs`
/// （直播类任务分钟级）。整站挂一层会把它们拦腰砍断 ✗ —— 那不是加固，是破坏功能。
///
/// 而这两条认证路由的 handler 只碰本地存储（一次 argon2 + 几次文件读写），
/// 30 秒**远远**够：真卡住只可能是死锁或磁盘挂了，那时回 408 比让连接一直挂着好 ✓。
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

pub fn router() -> Router<ServerState> {
    Router::new()
        // ── 登录闸（公开部分见 auth_gate::PUBLIC）────────────────────
        // ⚠ 这两条是本进程**唯一**免凭据且要算 argon2 的路由 ⇒ 单独套一层超时，
        // 见 [`AUTH_TIMEOUT`]。
        .route(
            "/api/setup",
            get(auth::setup_status)
                .post(auth::setup_create)
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    AUTH_TIMEOUT,
                )),
        )
        .route(
            "/api/session",
            get(auth::current_session)
                .post(auth::login)
                .delete(auth::logout)
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    AUTH_TIMEOUT,
                )),
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
        // ── 直播自动定时（设置页那张卡：开关 + 账号多选 + 当前算出的时刻）──
        // ⚠ 它落到**定时那张表**上（`auto-live-<owner>` ✓），不是新开的一张表 ✓。
        .merge(live_room::routes())
        // ── 凭据刷新（账号页那枚「一键刷新」按钮 + 账号设置里那两格）──────────
        // ⚠ 与 `live_room::routes()` 同一条形状：也是落到**定时那张表**上
        //（`auto-auth-refresh-<owner>` ✓），不是新开的一张表 ✓。
        // ⚠ 路由名 `auth/refresh` **不与** `auth.rs` 那套（`/api/session` 等）撞 ✓
        // —— 那套管的是"登录闸"，这套管的是"云盘凭据" ✓（两件事，见 `accounts.rs` 模块头 ✓）。
        .merge(auth_refresh::routes())
        // ── 任务的参数配置（任务页「配置」那一页）──────────────────────
        .route(
            "/api/tasks/{name}/config",
            get(task_config::get_one).put(task_config::put_one),
        )
        .route("/api/system/about", get(system::about))
        .route("/api/system/storage", get(system::storage))
        // 容量上限（历史保留条数 / 单次运行日志行数 / 日志总量 MB）——
        // **只有管理员**（实例级设置 ✓，与 `restart` 同一条口径 ✓）。
        .route(
            "/api/admin/storage",
            axum::routing::put(system::put_storage),
        )
        .route("/api/system/restart", post(system::restart))
        // ── 导出配置（设置页「服务」节那枚按钮）────────────────────────
        // ⚠ **只读**（GET）：它导的是**当前登录用户自己**那份配置，一个字节都不改；
        // ⚠ 导入**没有**界面 —— 那是覆盖式操作，走 CLI `mcloud config import <文件>` ✓。
        .merge(config::routes())
        // 今日AI豆（云朵记录汇总）—— **只读**：按账号读流水，绝不合计。
        // 一个账号读不到进响应的 `failed`，不打整张卡（见 beans.rs 模块头）。
        .merge(beans::routes())
        // 上游状态那一页**按需取**的只读通路（不落运行历史、不占任务）——
        // 见 `upstream.rs` 模块头（用户口径 2026-10-07）。
        .merge(upstream::routes())
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
        // 推送规则（用户口径 2026-10-07 的「规则」那一层）：CRUD + 事件/渠道清单 +
        // 试发（`POST /api/push/test`，用**某条规则自己的**平台与凭据真发一条 ✓，
        // 见 `push.rs` 的 `test_send`）✓。
        .merge(push::routes())
}
