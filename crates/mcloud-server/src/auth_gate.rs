//! 登录闸中间件。
//!
//! # 只拦 `/api/`，不拦静态资源
//!
//! 静态资源（`index.html`、JS、CSS）**必须**放行 —— 否则登录页自己都加载不出来，
//! 用户看到的是一个永远转圈的空白页，而不是登录框。
//!
//! 这不会泄漏任何东西：前端包里没有数据，数据全在 `/api/` 后面。
//!
//! # 三态
//!
//! | 状态 | 除公开路由外 |
//! |---|---|
//! | 未初始化（0 个用户） | **503** `setup_required` |
//! | 已初始化、未登录 | **401** `unauthorized` |
//! | 已登录 | 放行 |
//!
//! ⚠ 503 与 401 必须分开：401 让前端弹登录框（而这时根本没有账号可登），
//! 503 让前端跳到初始化向导。混成一个码，用户会看到"登录失败"
//! 而不是"还没建管理员" —— 而后者才是他该做的事。

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use mcloud_core::UserId;

use crate::routes::auth::{is_initialized, require_session};
use crate::ServerState;

/// **当前请求的登录用户**，由 [`gate`] 在放行前放进 request extensions。
///
/// # 为什么不自己再解一遍 cookie
///
/// `require_session` 已经验过会话并拿到了 `user_id`（它还要查过期、
/// 顺手清过期会话）。业务路由只该**取用**那份结论，另造一套解析必然与它
/// 漂移 —— 而漂移的表现是「登录闸放行的人」与「归属判定认的人」不是同一个。
///
/// 归属过滤一律以它为准：不是自己的数据返回 **404**，不泄露存在性
/// （`docs/architecture.md`：归属错误一律 404）。
#[derive(Debug, Clone)]
pub struct CurrentUser(pub UserId);

/// 不需要登录就能访问的 `/api/` 路由。
///
/// 判据是"**没有它就没法登录**"：
///
/// - `GET /api/setup` —— 前端要知道该弹登录框还是初始化向导
/// - `POST /api/setup` —— 首次建管理员（自己会在已初始化时拒绝）
/// - `POST /api/session` —— 登录
/// - `DELETE /api/session` —— 登出（幂等，没登录也不该报错）
/// - `GET /api/health` / `GET /api/version` —— 存活探测与版本，
///   监控要在未登录时也能问
///
/// ⚠ `GET /api/session`（查当前会话）**不在**这里：它必须能回答
/// "我登录了吗"，而"没登录"的正确答案是 401 而不是一个空对象。
pub const PUBLIC: &[(&str, &str)] = &[
    ("GET", "/api/setup"),
    ("POST", "/api/setup"),
    ("POST", "/api/session"),
    ("DELETE", "/api/session"),
    ("GET", "/api/health"),
    ("GET", "/api/version"),
];

fn is_public(method: &str, path: &str) -> bool {
    PUBLIC
        .iter()
        .any(|(m, p)| *m == method && *p == path.trim_end_matches('/'))
}

/// 登录闸。
pub async fn gate(State(state): State<ServerState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().to_string();

    // 静态资源放行：登录页自己得能加载
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }

    if is_public(&method, &path) {
        return next.run(req).await;
    }

    // 未初始化 ⇒ 503（前端据此跳初始化向导）
    match is_initialized(&state).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "实例尚未初始化，请先创建管理员",
                    "code": "setup_required",
                })),
            )
                .into_response();
        }
        Err(e) => {
            // 读不出用户数 ⇒ 无法判定。**失败关闭**：放行等于
            // "存储坏了就没人需要登录"，那是最糟的失效方向。
            // `ApiError` 没有 Display，记内层的 `mcloud_core::Error`
            tracing::error!(error = %e.0, "读取用户数失败，拒绝请求");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "无法确认登录状态" })),
            )
                .into_response();
        }
    }

    // 已初始化 ⇒ 必须登录
    match require_session(&state, req.headers()).await {
        Ok(session) => {
            // 归属判定需要「当前用户是谁」。会话里已经有 `user_id`，
            // 放进 extensions 交给处理器取 —— 不再解析一遍 cookie。
            req.extensions_mut().insert(CurrentUser(session.user_id));
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_routes_are_exactly_the_login_critical_ones() {
        assert!(is_public("GET", "/api/setup"));
        assert!(is_public("POST", "/api/setup"));
        assert!(is_public("POST", "/api/session"));
        assert!(is_public("DELETE", "/api/session"));
        assert!(is_public("GET", "/api/health"));
        assert!(is_public("GET", "/api/version"));
    }

    /// ⭐ 查当前会话**必须**要认证 —— 它得能回答"我登录了吗"，
    /// 而"没登录"的正确答案是 401。
    #[test]
    fn session_read_requires_auth() {
        assert!(!is_public("GET", "/api/session"));
    }

    #[test]
    fn business_routes_are_not_public() {
        for (m, p) in [
            ("GET", "/api/accounts"),
            ("POST", "/api/accounts"),
            ("GET", "/api/runs"),
            ("POST", "/api/runs"),
            ("GET", "/api/schedules"),
            ("PUT", "/api/schedules/x"),
            ("GET", "/api/pipelines"),
            ("GET", "/api/status"),
            ("GET", "/api/capabilities"),
        ] {
            assert!(!is_public(m, p), "{m} {p} 不该是公开的");
        }
    }

    /// 方法不匹配就不是同一条路由 —— `GET /api/setup` 公开
    /// **不代表** `DELETE /api/setup` 也公开。
    #[test]
    fn method_must_match() {
        assert!(!is_public("PUT", "/api/setup"));
        assert!(
            !is_public("GET", "/api/session"),
            "只有 POST/DELETE 是公开的"
        );
    }

    /// 末尾斜杠不该绕过判定（`/api/setup/` 与 `/api/setup` 等价）。
    #[test]
    fn trailing_slash_does_not_escape_the_gate() {
        assert!(is_public("GET", "/api/setup/"));
        assert!(!is_public("GET", "/api/accounts/"));
    }

    /// 公开清单本身不该混进业务路由 —— 这条防的是"顺手加一条"。
    #[test]
    fn public_list_has_no_business_routes() {
        for (_, p) in PUBLIC {
            assert!(
                !p.contains("account") && !p.contains("run") && !p.contains("schedule"),
                "公开清单里混进了业务路由：{p}"
            );
        }
    }
}
