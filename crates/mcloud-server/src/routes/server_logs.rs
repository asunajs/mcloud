//! 服务端**全量**日志（只读）—— 给界面上那个「日志」窗口用 ✓。
//!
//! 与隔壁 `logs.rs` 的分工（别混 ✓）：
//! * `logs.rs` = **一次运行**的日志流（SSE ✓，带 `?after=` 续传 ✓）；
//! * 本文件 = **整个进程**看到的日志（环形缓冲 ✓，含调度器与上游 http ✓）。
//!
//! 用户 2026-10-04 口径：「我其实想要一个能看所有日志的窗口，**不仅仅是每次任务**」✓。
//!
//! # 两档，可见性不同（2026-10-06 用户口径：「这种应该分为运维日志，且只有管理员可见」）
//!
//! 环里现在有 `category` 那一格（`logbuf::category_of` ✓），判据是"**谁打的**"：
//!
//! | 档 | 里面是什么 | 谁能看 |
//! |---|---|---|
//! | `task` **任务日志** | 某次运行的日志（`trace_event` 打的，带账号 ✓） | **按账号归属**：自己账号的那些 ✓ |
//! | `ops` **运维日志** | 认证失败、路径被拒、调度器、上游 http（`account` 可能有也可能没有） | **只有管理员** ✓ |
//!
//! 普通用户拿到的永远是"我的任务日志"这个子集 —— 运维那一档**一条都不给** ✓。
//! 方向是**失败关闭**的：认不出归属的行（没有 `account` 的任务日志、以及所有
//! 非 `trace_event` 来源的行）都落在 `ops` 里 ⇒ 只给管理员 ✓。
//!
//! ⚠ 为什么不能"按有没有 account 字段"分档：上游 http 那层
//! （`mcloud_upstream::http_log`）也打 `account=…`（`上游应答 account=… status=200` ✓），
//! 但它是**实例**的日志（凭证续期、限流），不是某次运行的 ✗ —— 详见 `logbuf::category_of`。

use axum::extract::{Extension, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth_gate::{is_admin, require_admin, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::logbuf::{self, Filter, ServerLog, CATEGORY_OPS, CATEGORY_TASK};
use crate::ServerState;

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    /// 最多回多少行（缺省 500 ✓，内部会夹到缓冲容量 ✓）。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 只看**不低于**这个级别的（`info` / `warn` / `error` ✓）。
    #[serde(default)]
    pub level: Option<String>,
    /// 关键词（大小写不敏感 ✓，匹配正文与来源 ✓）。
    #[serde(default)]
    pub q: Option<String>,
    /// `all`（缺省）/ `task` / `ops` ✓。
    ///
    /// ⚠ `ops` 是**管理员专属**的（普通用户拿到 403 ✓）；
    /// `all` 对普通用户**等价于** `task` —— 他本来也只能看自己的任务日志 ✓
    /// （这不是"降级"，而是"他能看到的全部" ✓）。
    #[serde(default)]
    pub category: Option<String>,
}

/// 取最近的服务端日志。
///
/// ⚠ 要 `CurrentUser`：日志里会出现账号标识与上游响应片段 ✓，
/// **不能**给未登录的人看 ✗（`auth_gate` 那层负责挡 ✓）。
///
/// ⚠⚠ 登录还不够，还要**按档**判（见文件头那张表）：运维日志只有管理员能看 ✓。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Vec<ServerLog>> {
    let category = q.category.as_deref().unwrap_or("all");
    if !matches!(category, "all" | CATEGORY_TASK | CATEGORY_OPS) {
        // ⚠ 不静默当成 all：写错一个字母就"看到全部"，那是失败**开放**的方向 ✗
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "category 只认 all / task / ops，收到：{category}"
        ))));
    }

    // 运维日志 ⇒ **管理员专属**（消息与其它实例级动作一致 ✓）
    if category == CATEGORY_OPS {
        require_admin(&state, &cur).await?;
    }

    let mut f = Filter::new(q.limit.unwrap_or(500));
    f.min_level = q.level.as_deref();
    f.query = q.q.as_deref();
    f.category = match category {
        CATEGORY_TASK => Some(CATEGORY_TASK),
        CATEGORY_OPS => Some(CATEGORY_OPS),
        _ => None,
    };

    // 普通用户：只给**任务日志**、且只给**自己的账号**那些 ✓
    // ⚠ 无条件覆盖 `f.category` —— 他即使请求 `all`（或漏传参数）也拿不到运维那一档 ✓
    let mine = if is_admin(&state, &cur.0).await? {
        None
    } else {
        Some(
            state
                .handle
                .store
                .accounts_for(&cur.0)
                .await?
                .iter()
                .map(|a| a.id.to_string())
                .collect::<std::collections::HashSet<String>>(),
        )
    };
    if let Some(set) = &mine {
        f.category = Some(CATEGORY_TASK);
        f.accounts = Some(set);
    }

    Ok(Json(logbuf::recent(&f)))
}
