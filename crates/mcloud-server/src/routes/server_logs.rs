//! 服务端**全量**日志（只读）—— 给界面上那个「日志」窗口用 ✓。
//!
//! 与隔壁 `logs.rs` 的分工（别混 ✓）：
//! * `logs.rs` = **一次运行**的日志流（SSE ✓，带 `?after=` 续传 ✓）；
//! * 本文件 = **整个进程**看到的日志（环形缓冲 ✓，含调度器与上游 http ✓）。
//!
//! 用户 2026-10-04 口径：「我其实想要一个能看所有日志的窗口，**不仅仅是每次任务**」✓。

use axum::extract::{Extension, Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth_gate::{require_admin, CurrentUser};
use crate::error::ApiResult;
use crate::logbuf::{self, ServerLog};
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
}

/// 取最近的服务端日志。
///
/// ⚠ 要 `CurrentUser`：这与"运行记录"同级 —— 日志里会出现账号标识与上游响应
/// 片段 ✓，**不能**给未登录的人看 ✗（`auth_gate` 那层负责挡 ✓）。
///
/// ⚠⚠ **还要是管理员**（安全审计 SEC-11）：日志缓冲区是**实例级**的一个环
/// （`logbuf` 里没有"这条属于谁"），所以"按归属过滤"这件事在数据结构上就做不到 ——
/// 硬做要么给每行加归属（改动面很大），要么按当前用户猜（更糟 ✗）。
/// ⇒ 先收成管理员专属：单管理员部署下读数**完全不变** ✓，而第二个用户
/// 至少读不到别人的账号标识与上游响应片段了 ✓。
/// 真正的"每人只看自己的日志"要等日志行带上 `user_id`（见报告 SEC-11）。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Vec<ServerLog>> {
    require_admin(&state, &cur).await?;
    Ok(Json(logbuf::recent(
        q.limit.unwrap_or(500),
        q.level.as_deref(),
        q.q.as_deref(),
    )))
}
