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

use crate::error::ApiError;
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

/// **实例管理员**是不是他（安全审计 SEC-07）。
///
/// # 判据：`users` 表里**创建最早**的那个
///
/// 与 [`mcloud_core::legacy_owner`] 同一个判据 —— 那一处是"老数据没有归属字段时
/// 算谁的"，这一处是"实例级动作谁能做"，**两者必须是同一个人**：
/// 否则会出现"他能看到全部账号、却不能配置任务"这种自相矛盾的权限面 ✗。
///
/// # 为什么不做一套真正的角色表
///
/// 本仓的多用户今天**不可达**（`POST /api/setup` 只在 0 用户时可建，没有用户管理路由）
/// ⇒ 真正的角色模型（`role` 字段 + 迁移 + 前端）是在给一条走不通的路修护栏 ✗。
/// 先用"最早的那个用户"把**实例级动作**（重启、改安全策略、看全局日志、改任务参数）
/// 收回到管理员手里，等真的要多用户时再换成显式角色 —— 那时的改动点只有本函数 ✓。
pub async fn is_admin(state: &ServerState, user: &UserId) -> Result<bool, ApiError> {
    let users = state.handle.store.users().await?;
    Ok(mcloud_core::legacy_owner(&users).as_ref() == Some(user))
}

/// 实例级动作的闸：不是管理员 ⇒ **403**（不是 401，见 [`mcloud_core::Error::Forbidden`]）。
///
/// ⚠ 调用点必须是**已经拿到 `CurrentUser`** 的 handler（即闸后面的业务路由）。
pub async fn require_admin(state: &ServerState, cur: &CurrentUser) -> Result<(), ApiError> {
    if is_admin(state, &cur.0).await? {
        return Ok(());
    }
    Err(ApiError(mcloud_core::Error::Forbidden(
        "这个操作只有实例管理员可以做".into(),
    )))
}

/// 这个请求路径是不是"本应用不可能产生的形状"（安全审计 `SEC-21`）。
///
/// 判据（任一命中即拒）：
/// * `//`（空段）—— `/api//accounts`、`//api/accounts`
/// * `.` / `..` 段 —— `/api/./accounts`、`/../api/accounts`
/// * 上面两者的百分号编码 —— `/api%2faccounts`、`/%2e%2e/api/accounts`
///
/// ⚠ 编码判定**不区分大小写**（`%2F` 与 `%2f` 等价）✓。
/// ⚠ 这是**防御纵深**，不是"修了一个正在被利用的洞"：实测这些形状在
/// matchit（axum 0.8.9）上本来就匹配不到任何 handler、会落到 SPA 回落
///（见报告 §4 的交叉核对）。它挡的是"框架行为变了/有人加了个奇怪 handler"
/// 这一类将来才会发生的事 ✓。
fn path_is_suspicious(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    path.contains("//")
        || lower.contains("%2f")
        || lower.contains("%2e")
        || path.split('/').any(|seg| seg == "." || seg == "..")
}

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

    // ⚠⚠ 2026-10-06 安全审计 `SEC-21`（防御纵深）：下面那句
    // `!path.starts_with("/api/") ⇒ 放行` 的安全性**完全依赖**
    // "matchit 不做路径归一化" 这个外部事实 —— 一旦哪天的框架开始归一化
    // （或者有人挂了一个 handler 在 `/api/` 之外的写法上），这句话就会
    // 变成一条绕闸的路 ✗。
    //
    // ⇒ 先把"**永远不可能是本应用合法 URL**"的形状直接拒掉：
    // 空段、`.` / `..` 段、以及它们的百分号编码。合法请求里一个都不会出现
    //（前端只用 `/`、`/assets/...`、`/api/...` 这些规规矩矩的路径）✓。
    // 这样"闸的判定"与"路由的判定"之间就不存在可利用的差额了 ✓。
    if path_is_suspicious(&path) {
        tracing::warn!(path = %path, "拒绝形状可疑的请求路径");
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "资源不存在" })),
        )
            .into_response();
    }

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

    /// ⭐ 审计 `SEC-21`（防御纵深）：**形状可疑的路径一律拒**。
    ///
    /// 那句"非 `/api/` 直接放行"的安全性原本完全依赖"matchit 不做归一化"这个
    /// 外部事实；这一组把"本应用不可能产生的形状"直接挡掉，闸与路由之间
    /// 就不存在可利用的差额 ✓。
    ///
    /// 红测：把 `gate` 里那句 `if path_is_suspicious(&path)` 删掉 ⇒ 本测试红。
    #[test]
    fn suspicious_path_shapes_are_rejected() {
        for bad in [
            "//api/accounts",
            "/api//accounts",
            "/api/./accounts",
            "/../api/accounts",
            "/api/../api/accounts",
            "/api%2faccounts",
            "/api%2Faccounts",
            "/%2e%2e/api/accounts",
            "/api/%2E/accounts",
        ] {
            assert!(path_is_suspicious(bad), "这条本该被判可疑：{bad}");
        }

        // 反向：正常路径一个都不许误伤
        for ok in [
            "/",
            "/index.html",
            "/assets/index-abc123.js",
            "/api/accounts",
            "/api/runs/abc/logs",
            "/api/tasks/live-room/config",
            "/api/login/qr/poll",
        ] {
            assert!(!path_is_suspicious(ok), "这条不该被判可疑：{ok}");
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
