//! API 路由 —— P5 的第一批，只包含**不依赖 P3 / P4 的那些**。
//!
//! # ⚠ 出网契约以 Go 版为唯一准绳（2026-09-24 约定）
//!
//! WebUI 只维护 mcloud（Go 侧）那一版 —— 本仓库根的 `web/` 是指向
//! `../mcloud/web` 的符号链接。因此**本 server 的接口必须与 Go 版
//! `internal/server/` 逐路由对齐**：路径、方法、payload 字段名与形状、
//! 错误体 `{code, message}` 都以 Go 侧为契约来源。新增路由先看 Go 怎么定义；
//! 改已有路由的出网字段 = 先改 Go，再跟过来 —— 反过来会让共享的那份
//! WebUI 在某一侧静默坏掉。
//!
//! 已知差异（对齐前的欠账）见文件末尾「与 Go 版的差异清单」。
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

// ─────────────────────────────────────────────────────────────────────────────
// 与 Go 版的差异清单（2026-09-24 盘点；对齐一份划掉一份，别让清单变墓碑）
// ─────────────────────────────────────────────────────────────────────────────
//
// 契约来源：`../mcloud/internal/server/auth.go`（路由注册表）与同目录处理器。
// 这里只记**已知不一致**；一致的（version / capabilities 同源 payload、
// accounts 列表与 cloud-stats、runs 列表与 SSE、error 的 {code,message} 码表、
// session / setup / health 的路径与方法）不列 —— 清单是给"还没对齐"用的。
//
// ## A. 已有路由的出网差异
//
// 1. **RFC3339 时间格式**：Go 是 `Format(time.RFC3339)` ⇒ UTC 落 `…Z`；
//    本侧 `to_rfc3339()` ⇒ `…+00:00`。两者都合法，但**不是同一段字节**。
//    波及：session `expiresAt`、account `cloudUpdatedAt` / `lastOkAt`、
//    schedule `nextFireAt` / `createdAt` / `lastRunAt`、run `startedAt` / `finishedAt`。
// 2. **`GET /api/schedules` 少两个字段**：Go 的 `schedulePayload` 有
//    `lastFireAt` / `missPolicy`，本侧没有 —— 而且 `mcloud_core::Schedule`
//    模型与 SQLite schema 也还没有这两列（补齐 = 模型 + 迁移 + 读写 + payload）。
// 3. **query 解析宽松度**：Go 的 `clampLimit` 对 `?days=abc` **静默回默认值**；
//    本侧 `ApiQuery` 反序列化失败 → 400 `bad_request`。同一手势两种结论。
// 4. **`PUT /api/schedules/{id}` 的 `enabled: null`**：Go 是 `*bool`，
//    null ⇒ 保持缺省 true；本侧是 `bool`（`serde(default)` 只管**缺席**不管
//    null）⇒ 400。`timezone: null` 两边都落默认，已一致。
//
// ## B. Go 有、本侧尚无的路由（按 auth.go 注册表逐条数）
//
// - `GET /api/status`
// - 账号写面：`POST /api/accounts`、`PUT /api/accounts/{id}/enabled`、
//   `DELETE /api/accounts/{id}`、`POST /api/accounts/{id}/refresh-auth`
// - 账号只读：`GET /api/accounts/{id}/live-room`、`GET /api/accounts/{id}/exchange-prizes`
// - `POST /api/runs`（执行能力 —— 等 P4 有可跑的东西）
// - 登录族 7 条：`POST /api/login/qr`、`GET /api/login/qr/{id}`、
//   `GET /api/login/qr/{id}/image.svg`、`POST /api/login/sms/app`、
//   `POST /api/login/sms/app/{id}/verify`、`POST /api/login/sms/pc`、
//   `POST /api/login/sms/pc/{id}/verify`
// - 通知 3 条：`GET /api/notify/config`、`PUT /api/notify/config`、`POST /api/notify/test`
//
// ⚠ 补 B 组不是"照着 HTTP 层抄一遍"就完 —— 每条后面都要有真的实现
// （登录族要接 `mcloud-capabilities::login`，账号写要登录链路，notify 要新能力）。
// 路由空转返回"未实现"比 404 更糟：共享的 WebUI 会以为功能已经有了。
