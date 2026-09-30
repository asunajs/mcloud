//! 中止运行的路由。
//!
//! # 语义
//!
//! 中止是**协作式**的：这里只置标志，任务在安全点自己退出。
//! 返回 202 表示"请求已受理"，不表示"已经停了"。
//!
//! 对不存在的/已结束的运行返回 `{"cancelled": false}` 而不是 404：
//! "用户点中止时任务刚好跑完"是完全正常的竞态，报错只会让人困惑。

use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::Serialize;

use mcloud_core::RunId;

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::routes::runs::{run_access, RunAccess};
use crate::ServerState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResponse {
    /// 是否成功置上了中止标志。
    ///
    /// `false` 通常意味着任务刚好跑完 —— 正常竞态，不是错误。
    pub cancelled: bool,
    /// 该运行**此刻是否仍在运行**。
    ///
    /// 与 `cancelled` 是两件事：已请求中止的运行在任务退到安全点之前
    /// **仍在运行**。前端据此区分"已请求中止、还在收尾"与"已经停了"。
    pub running: bool,
}

pub async fn cancel_run(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<u64>,
) -> ApiResult<CancelResponse> {
    let run_id = RunId::new(id);

    // 归属：别人的运行**不能中止**，且与「不存在」区分不开（回 404）。
    // 真不存在的运行保持既有幂等语义（200 + cancelled:false）——
    // 「点中止时任务刚好跑完」是正常竞态，报错只会让人困惑。
    match run_access(&state, &cur.0, run_id).await? {
        RunAccess::Allowed => {}
        RunAccess::Denied => {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "运行记录不存在：{id}"
            ))));
        }
        RunAccess::Unknown => {
            return Ok(Json(CancelResponse {
                cancelled: false,
                running: false,
            }));
        }
    }
    // 先取"是否在运行"，再请求中止：顺序反过来的话，一个恰好在此期间
    // 结束的运行会被报成"仍在运行"
    let was_running = state.handle.cancel.is_running(run_id);
    let cancelled = state.handle.cancel.cancel(run_id);
    Ok(Json(CancelResponse {
        cancelled,
        running: was_running,
    }))
}
