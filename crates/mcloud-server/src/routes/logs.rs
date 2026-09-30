//! 运行日志路由。

use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::Deserialize;

use mcloud_core::{LogLine, RunId, Seq};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
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
    // 先确认运行记录**是自己的**，避免"不存在的 run"与"还没日志"被混为一谈，
    // 同时把别人的运行与不存在的运行回成同一个 404（不泄露存在性）。
    if state.handle.store.run_for(&cur.0, run_id).await?.is_none() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "运行记录不存在：{id}"
        ))));
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
