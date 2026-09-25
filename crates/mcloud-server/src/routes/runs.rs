//! 运行相关路由：启动、查询、SSE 事件流。

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use mcloud_core::{Account, AccountId, RunId, RunRecord};

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 启动运行的请求体。
#[derive(Debug, Deserialize)]
pub struct RunRequest {
    pub task: String,
    pub account: String,
    #[serde(default)]
    pub body: serde_json::Value,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub run_id: RunId,
    pub task: String,
    pub account: String,
    pub outcome: mcloud_core::TaskOutcome,
    pub duration_ms: u64,
}

/// 列出最近运行记录。
pub async fn list(State(state): State<ServerState>) -> ApiResult<Vec<RunRecord>> {
    let runs = state.handle.store.runs(100).await?;
    Ok(Json(runs))
}

/// 查询单次运行。
pub async fn get_one(
    State(state): State<ServerState>,
    Path(id): Path<u64>,
) -> Result<Json<RunRecord>, ApiError> {
    match state.handle.store.run(RunId::new(id)).await? {
        Some(r) => Ok(Json(r)),
        None => Err(ApiError(mcloud_core::Error::NotFound(format!(
            "运行记录不存在：{id}"
        )))),
    }
}

/// 启动一次运行。
///
/// 同步执行并返回结果。异步模式（立即返回 run_id、结果走 SSE）留待
/// P4 接入执行器后补上。
pub async fn create(
    State(state): State<ServerState>,
    Json(req): Json<RunRequest>,
) -> Result<Json<RunResponse>, ApiError> {
    // 先校验任务名：否则未知任务会被误报成「账号不存在」，误导排查
    if !state.handle.registry.contains(&req.task) {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "未知任务：{}",
            req.task
        ))));
    }

    let id = AccountId::new(&req.account);
    let account: Account = state.handle.store.account(&id).await?.ok_or_else(|| {
        ApiError(mcloud_core::Error::Config(format!(
            "账号不存在：{}",
            req.account
        )))
    })?;

    let record = state
        .handle
        .runner
        .run(&req.task, &account, &req.body)
        .await?;

    Ok(Json(RunResponse {
        run_id: record.run_id,
        task: record.task,
        account: record.account,
        outcome: record.outcome,
        duration_ms: record.duration_ms,
    }))
}

/// SSE：订阅运行事件。
///
/// 前端用 `EventSource('/api/runs/{id}/events')` 消费，实现运行过程实时可见。
pub async fn events(
    State(state): State<ServerState>,
    Path(id): Path<u64>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let want = RunId::new(id);
    let rx = state.handle.sink.subscribe();

    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let ev = match item {
            Ok(ev) => ev,
            // 广播滞后（Lagged）时跳过该条，不中断整条流
            Err(_) => return None,
        };
        // 只推送本次运行的事件
        if ev.run_id() != want {
            return None;
        }
        let name = ev.event_name();
        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
        Some(Ok(Event::default().event(name).data(data)))
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
