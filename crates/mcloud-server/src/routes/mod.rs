//! API 路由 —— P5 的第一批，只包含**不依赖 P3 / P4 的那些**。
//!
//! # 为什么这一批能提前落地，而"跑任务"的那批不能
//!
//! 方案 §7 把服务端的路由整块挂在 P5，但那一块其实分两半：
//!
//! - **读**：能力清单、账号、运行历史、事件流、定时任务。它们要的数据
//!   （`StateStore` / `ConfigStore` 两个 trait、`runs` / `run_events` 两张表、
//!   执行器的事件总线）**都已经在了**，缺的只是"端出去"。就是下面这四个模块。
//! - **写**：`POST /api/runs`（真的执行一个能力）、`/api/exchange/*`。它们要等 P4
//!   有可执行的能力 —— 现在注册表里 19 项只登记了 3 项，且只有登录那条链路有实现。
//!   路由当然可以提前写，但**后面没有东西可跑**，写出来就是一条永远返回
//!   "未实现"的接口，而前端照着它接反而会以为功能已经有了。
//!
//! `openapi.json` 也不在这里：它是 P5 的收口（`utoipa` 生成 + 与提交物 `--check`），
//! 该在接口面稳定之后一次做完，而不是每加一条路由就手工补一次文档 ——
//! 手工补的那份一定会漂移。
//!
//! # 鉴权不在这里
//!
//! 中间件（初始化门禁、会话解析、`/api/*` 的 404 兜底）统一由 `auth::router` 套，
//! 所以本模块的处理器可以放心地 `Extension<AuthUser>` —— 取不到就编不过。
//! 也因此**本模块只产出需要会话的路由**：往这里加一条"免登录"的接口是不行的，
//! 那要同时改 `auth::is_public` 的白名单，这是故意的。
//!
//! # 范围一律来自会话
//!
//! 每个处理器都用 `Scope::user(&auth.user.id)` 取数。**不得**用 `Scope::Local` ——
//! 那等于把所有人的数据交给任何一个已登录用户（见 `mcloud_core::Scope` 的说明）。

pub mod accounts;
pub mod meta;
pub mod runs;
pub mod schedules;

use axum::Router;

use crate::auth::AppState;

/// 全部需要会话的 API 路由（尚未 `with_state`）。
///
/// 由 `auth::router` 统一套中间件与状态，这样"哪条路径免登录"只有一处判断。
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(meta::router())
        .merge(accounts::router())
        .merge(runs::router())
        .merge(schedules::router())
}

/// 把一个可选的条数参数夹到 `[1, max]`。
///
/// 这里选择**夹**而不是拒绝：`?limit=0` 或 `?limit=99999` 都是"多要/少要了一些"，
/// 为它报一个 400 是把小事升级成事故，而前端还得为此写一条错误分支。
/// `max` 的存在理由不是洁癖：不加它，一个 `?limit=999999999` 就能让服务端
/// 去装配一个巨大的响应体。
pub(crate) fn clamp_limit(raw: Option<usize>, default: usize, max: usize) -> usize {
    raw.unwrap_or(default).clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::clamp_limit;

    #[test]
    fn missing_limit_falls_back_to_default() {
        assert_eq!(clamp_limit(None, 50, 200), 50);
    }

    #[test]
    fn limits_are_clamped_instead_of_rejected() {
        assert_eq!(clamp_limit(Some(0), 50, 200), 1);
        assert_eq!(clamp_limit(Some(10_000), 50, 200), 200);
        assert_eq!(clamp_limit(Some(20), 50, 200), 20);
    }
}
