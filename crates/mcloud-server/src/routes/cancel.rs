//! 中止运行的路由。
//!
//! # 语义
//!
//! 中止是**协作式**的：这里只置标志，任务在安全点自己退出。
//! 返回 202 表示"请求已受理"，不表示"已经停了"。
//!
//! 对不存在的/已结束的运行返回 `{"cancelled": false}` 而不是 404：
//! "用户点中止时任务刚好跑完"是完全正常的竞态，报错只会让人困惑。

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use mcloud_core::RunId;

use crate::error::ApiResult;
use crate::ServerState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResponse {
    /// 是否成功置上了中止标志。
    pub cancelled: bool,
    /// 该运行当前是否仍在运行中。
    pub running: bool,
}

pub async fn cancel_run(
    State(state): State<ServerState>,
    Path(id): Path<u64>,
) -> ApiResult<CancelResponse> {
    let run_id = RunId::new(id);
    let cancelled = state.handle.cancel.cancel(run_id);
    Ok(Json(CancelResponse {
        cancelled,
        running: state.handle.cancel.is_cancelled(run_id) || cancelled,
    }))
}
