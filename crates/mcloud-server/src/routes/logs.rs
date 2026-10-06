//! 运行日志路由。

use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::Deserialize;

use mcloud_core::{LogLine, RunId, Seq};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::routes::runs::{run_access, RunAccess};
use crate::ServerState;

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// 客户端**已经看到**的序号，用于断线续传。
    ///
    /// 语义是"已看到 n"，返回 n 之后的。
    /// 混淆成"下一个要取"会**静默跳过一条**。
    #[serde(default)]
    pub after: Option<u64>,
}

/// 取某次运行的日志。
///
/// 内存里有的优先（最新），没有则回落到存储层。
///
/// 返回 **404**（运行记录不存在）与"存在但暂无日志"是两回事：
/// 前者该报错，后者是正常运行状态（刚启动还没打日志）。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<u64>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Vec<LogLine>> {
    let run_id = RunId::new(id);

    // ⚠⚠ 2026-10-06 审计 `SEC-20`：可见性口径必须与 `/api/runs/{id}/events` **一致**。
    //
    // 旧代码只查**存储层**（`store.run_for`）⇒ 一条**正在跑**的 run 一律 404
    //（运行记录是**结束时**才落盘的），而同一时刻 `/events` 是 200 ——
    // 同一个 run、两个端点给出相反的答案 ✗。方向虽然是失败关闭（不泄露），
    // 但"运行中读不到日志"是**功能损失**：前端想边跑边看日志就只能走 SSE。
    //
    // `run_access` 三处都看（本服务发起登记 / 存储层 / 内存中枢），并且
    // "别人的"与"压根不存在"回**同一个 404**（不泄露存在性）✓。
    match run_access(&state, &cur.0, run_id).await? {
        RunAccess::Allowed => {}
        RunAccess::Denied | RunAccess::Unknown => {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "运行记录不存在：{id}"
            ))));
        }
    }

    let since = q.after.map(Seq::seen);

    // 内存历史优先：它还包含尚未落盘的那部分
    let in_memory: Vec<LogLine> = state
        .handle
        .hub
        .replay(run_id, since.unwrap_or(Seq::ZERO))
        .into_iter()
        .filter_map(|s| match s.event {
            mcloud_core::RunEvent::Log { level, message, .. } => {
                Some(LogLine::new(id, level, message, 0, s.seq))
            }
            _ => None,
        })
        .collect();

    if !in_memory.is_empty() {
        return Ok(Json(in_memory));
    }

    // 回落到存储层（进程重启后、或事件已被回收）
    Ok(Json(state.handle.store.logs(run_id, since).await?))
}
