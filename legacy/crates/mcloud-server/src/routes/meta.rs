//! 元信息：版本与能力注册表。
//!
//! # 为什么这两个最先做
//!
//! 它们的响应**完全由代码推导**：不查库、不看会话之外的任何东西。
//! 而它们又恰恰是前端起手就要用的两样 —— 界面按 `capabilities` 渲染表单与结果卡片、
//! 按 `version` 显示产品名与版本号。
//!
//! 两者的 payload 都不在这里拼：实现住在 `mcloud-app::payload`，
//! 因为 CLI 也要打印同一张表，而 CLI 不该为了打个表把 axum 拖进来。
//! 这里只负责"把它挂到一条路径上"。

use axum::Json;
use axum::Router;
use axum::routing::get;
use serde_json::Value;

use crate::auth::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/version", get(get_version))
        .route("/api/capabilities", get(get_capabilities))
}

/// `GET /api/version`
///
/// 取代原构建期的 `__APP_VERSION__` 注入：版本号只有一个来源（workspace 的 Cargo.toml），
/// 前端不必再"打包时塞一份、运行时和后端对不上"。
async fn get_version() -> Json<Value> {
    Json(crate::version_payload())
}

/// `GET /api/capabilities`
///
/// 响应完全由能力注册表推导（含每个能力的参数与出参声明），
/// 所以**新增一个能力不需要动这里、也不需要改前端** —— 这条承诺由
/// `mcloud-app::payload` 的单测钉住。
async fn get_capabilities() -> Json<Value> {
    Json(crate::capabilities_payload())
}
