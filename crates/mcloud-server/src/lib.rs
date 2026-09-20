//! mcloud-server —— HTTP 服务层（**产物之一**）。
//!
//! 职责（P5 实现）：axum 路由 + 参数校验 + SSE 事件流 + `utoipa` 导出 `openapi.json`
//! + `rust-embed` 内嵌前端产物（单二进制可跑）。
//!
//! # 组装与 payload 已经下移到 `mcloud-app`
//!
//! 原因很实际：CLI 也要打印能力注册表、也要跑迁移，而它**不该为了这两件事
//! 把 axum 整棵树拖进来**。所以那些代码住在 `mcloud-app`，这里只重新导出，
//! 让"服务层依赖组装层"这一个方向成立（方案 §3.2 的两个产物）。
//!
//! # 契约没变
//!
//! 前端不消费 Rust 结构体，只消费 JSON 形状；形状一旦改动，
//! P5 的 OpenAPI `--check` 会立刻发现（防止前后端类型静默漂移）。
//!
//! # 服务由**本产物**启动，不由 CLI 的 `serve` 子命令启动
//!
//! 这是方案 §7 遗留的待拍板项（"CLI 是否需要 `serve` 子命令"），
//! 而依赖门禁已经给出了答案：`mcloud-cli -> mcloud-server` 被
//! `scripts/check-deps.sh` 的 `FORBIDDEN_EDGES` 挡住。理由也成立 ——
//! CLI 一旦拿到 axum，就不再是"能在任何没有 HTTP 环境的地方跑"的命令行工具。

#![forbid(unsafe_code)]

use std::path::PathBuf;

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

pub mod auth;
pub mod error;
/// API 路由。**不在这里 `with_state`** —— 中间件与状态由 [`auth::router`] 统一套，
/// 这样"哪条路径免登录"只有一处判断。
pub mod routes;

pub use auth::{AppState, AuthUser, ServerConfig, router};
pub use error::{ApiError, ApiJson, ApiQuery};

/// 旧数据迁移的执行策略（实现见 `mcloud-app`）。
pub use mcloud_app::migration;

/// 能力注册表 / 版本信息 → 前端要吃的 JSON（实现见 `mcloud-app`）。
///
/// 重新导出而不是原地保留：CLI 与 HTTP 两条入口必须用**同一份**渲染逻辑，
/// 否则"CLI 看到的字段"与"浏览器看到的字段"迟早不一致。
pub use mcloud_app::payload::{capabilities_payload, capability_payload, version_payload};

/// 组织完整的应用：API 路由 + 前端产物。
///
/// # 为什么要 `web_dir` 而不是内嵌
///
/// `rust-embed` 内嵌是 P7 的收口（"单二进制可跑"）。在那之前用目录更实用：
/// 前端改一行只需 `npm run build`，不必重编 Rust。
/// 目录**不存在**时只挂 API —— 开发期常见的情况是先跑 `npm run dev`（vite 在 5173），
/// 这时后端没有 `dist` 也该能起来。
pub fn app(state: AppState, web_dir: Option<PathBuf>) -> Router {
    let router = router(state);
    let Some(dir) = web_dir.filter(|dir| dir.is_dir()) else {
        return router;
    };
    // SPA 回退：非 `/api` 的未知路径交给 index.html（前端路由自己处理）。
    // `/api` 下的未知路径不会走到这里 —— `auth::router` 里有专门的兜底路由，
    // 否则"接口路径写错"会表现成"返回一段 HTML"。
    router.fallback_service(ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html"))))
}

/// 起服务。
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    web_dir: Option<PathBuf>,
) -> std::io::Result<()> {
    axum::serve(listener, app(state, web_dir)).await
}

#[cfg(test)]
mod tests {
    // 这些断言原本跟随实现，现在实现搬到了 `mcloud-app`，
    // 但**契约属于服务层**（它是对外 API 的形状），所以测试留在这里：
    // 哪天有人改坏了 payload 形状，这里会红，而不是等到 P5 的 OpenAPI 对拍。
    use mcloud_capabilities::REGISTRY;
    use mcloud_core::PRODUCT_NAME;

    use crate::{capabilities_payload, version_payload};

    #[test]
    fn payload_exposes_declarative_form_metadata() {
        let payload = capabilities_payload();
        let exchange = payload["all"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["code"] == "exchange")
            .expect("exchange 应在 payload 中");

        assert_eq!(exchange["hidden"], true);
        assert_eq!(exchange["risk"], "high");
        assert_eq!(exchange["needsConfirmation"], true);
        assert_eq!(exchange["runInBatch"], false);
        assert_eq!(exchange["defaultAccountMode"], "sequential");
    }

    #[test]
    fn adding_a_capability_changes_only_the_registry() {
        // 设计承诺：payload 完全由 REGISTRY 推导，没有任何一处硬编码能力名。
        let payload = capabilities_payload();
        let all = payload["all"].as_array().unwrap();
        assert_eq!(all.len(), REGISTRY.len());
        for (index, cap) in REGISTRY.iter().enumerate() {
            assert_eq!(all[index]["code"], cap.code);
        }
    }

    #[test]
    fn version_payload_reports_the_product_not_the_crate() {
        let payload = version_payload();
        assert_eq!(payload["name"], PRODUCT_NAME);
        assert_eq!(payload["timezone"], "Asia/Shanghai");
        assert!(!payload["version"].as_str().unwrap().is_empty());
    }
}
