//! 运行相关路由：启动、查询、SSE 事件流。
//!
//! # SSE 契约（前端必须遵守）
//!
//! 1. **收到终局事件（`finished`）后客户端要自己 `close()`。**
//!    `EventSource` 按规范会自动重连，不关的话一次已结束的运行会被
//!    无限重连（每次带 `Last-Event-ID`，服务端补发 0 条再关）。
//!
//! 2. **服务端在终局事件后立即结束响应**，不挂着连接。
//!    挂着的话每个打开的页面都占一个永久任务。
//!
//! 3. 断线重连带上 `Last-Event-ID`（浏览器自动）或 `?after=`（手动），
//!    两者都表示"**已经看到**的序号"。
//!
//! # 双源：先订阅，再读库
//!
//! 顺序**不能反**。订阅会把 `since` 之后的内存历史补进队列，
//! 于是"读库"与"订阅"之间那个窗口里发布的事件不会两头都漏掉。

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::Stream;
use serde::{Deserialize, Serialize};

use mcloud_core::{Account, AccountId, RunEvent, RunId, RunRecord, Seq};

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 启动运行的请求体。
#[derive(Debug, Deserialize)]
pub struct RunRequest {
    pub task: String,
    pub account: String,
    #[serde(default)]
    pub body: serde_json::Value,
    /// 立即返回、后台执行。
    ///
    /// 界面上要能点"中止"，就必须让运行在后台跑 —— 同步执行的话
    /// 请求本身还没返回，用户根本没有可中止的时机。
    /// 默认 `false` 保持既有调用方的行为不变。
    #[serde(default)]
    pub background: bool,
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
/// 默认同步执行并返回最终结果；`background: true` 时立即返回并后台执行。
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

    if req.background {
        return start_background(state, req.task, account, req.body).await;
    }

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

/// 后台执行，**立刻**返回真实的 run id。
///
/// 拿到的 id 可以立即用来订阅事件流和请求中止 —— 这正是"要能点中止"
/// 所必需的前提。
async fn start_background(
    state: ServerState,
    task: String,
    account: Account,
    body: serde_json::Value,
) -> Result<Json<RunResponse>, ApiError> {
    // Runner::start 内部自己分配 id 并 spawn，因此这里返回的 id 是真的。
    // ⚠ spawn 出去的 future 不持有请求上下文 —— 请求一返回它的上下文就被取消了，
    // 而这一趟任务要跑几十秒。
    let run_id = state.handle.runner.start(&task, account.clone(), body);
    Ok(Json(RunResponse {
        run_id,
        task,
        account: account.id.to_string(),
        outcome: mcloud_core::TaskOutcome::skipped("已在后台开始"),
        duration_ms: 0,
    }))
}

/// SSE：订阅运行事件。
///
/// 实现是"内存历史 + 存储层"双源，用**拉取**而不是广播订阅：
/// 广播做不到"迟到的订阅者拿到历史"，而"运行已开始、用户才打开页面"
/// 正是最常见的场景。
pub async fn events(
    State(state): State<ServerState>,
    Path(id): Path<u64>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run_id = RunId::new(id);

    // 断线续传的起点：`Last-Event-ID` 优先（浏览器自动带），其次 `?after=`
    let since = parse_resume_point(&headers, q.after);

    // ⚠ 顺序：先订阅，再读库。
    // 订阅会把 since 之后的内存历史补进队列，因此两步之间那个窗口里的
    // 事件一定在队列里；反过来则两头都漏。
    let subscriber = state.handle.hub.subscribe(run_id, since);

    // 运行记录必须存在 —— 否则"不存在的 run"会挂成一条永不结束的空流。
    // 注意 404 与"存在但还没事件"必须区分开。
    if state.handle.store.run(run_id).await?.is_none() {
        state.handle.hub.unsubscribe(run_id, subscriber);
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "运行记录不存在：{id}"
        ))));
    }

    let hub = std::sync::Arc::clone(&state.handle.hub);
    // 在**订阅之后**建通知接收端：这样"订阅到建接收端"之间发布的事件
    // 已经在队列里（drain 会取到），而之后发布的会唤醒我们。
    let waiter = hub.waiter();

    let stream = futures::stream::unfold(
        StreamState {
            hub,
            run_id,
            subscriber,
            waiter,
            done: false,
            last_sent: since,
        },
        |mut st| async move {
            if st.done {
                return None;
            }
            loop {
                let delivery = st.hub.drain(st.run_id, st.subscriber);
                if delivery.dropped > 0 {
                    // 丢事件必须报出来：不报的表现是"日志少了半截"而没人知道。
                    // 前端看到这条提示应重新订阅（带上已收到的最大序号）。
                    tracing::warn!(
                        run = %st.run_id,
                        dropped = delivery.dropped,
                        "SSE 订阅者落后，已丢弃部分事件"
                    );
                }

                let mut out = Vec::new();
                let mut terminal = false;
                for item in delivery.events {
                    // 内存历史与存储层总会重叠；重复推送与漏推一样
                    // 会让前端显示一个错的进度
                    if item.seq <= st.last_sent {
                        continue;
                    }
                    st.last_sent = item.seq;
                    if matches!(item.event, RunEvent::Finished { .. }) {
                        terminal = true;
                    }
                    match sse_event(&item) {
                        Ok(ev) => out.push(Ok(ev)),
                        Err(e) => {
                            // 序列化失败不该断掉整条流：发一条可见的错误事件，
                            // 让前端知道"这里少了一条"
                            tracing::error!(error = %e, "事件序列化失败");
                        }
                    }
                }

                if terminal {
                    // 终局事件之后立即收尾 —— 不挂着连接。
                    // 注意：靠"结束流"收尾，不是靠"内容发对了"。
                    st.done = true;
                    if out.is_empty() {
                        return None;
                    }
                    return Some((Box::pin(futures::stream::iter(out)) as EventChunk, st));
                }
                if !out.is_empty() {
                    return Some((Box::pin(futures::stream::iter(out)) as EventChunk, st));
                }
                // 队列空：等新事件。用通知唤醒，不忙轮询。
                if st.waiter.recv().await.is_err() {
                    // 发送端没了（进程在收尾）⇒ 结束这条流
                    st.done = true;
                    return None;
                }
            }
        },
    );

    // unfold 产出的是"一批一批"的事件，flatten 摊平成一条流
    let stream = futures::StreamExt::flatten(stream);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// 一批已就绪的事件。用 box 是因为 unfold 的两个分支要返回同一个具体类型。
type EventChunk = std::pin::Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>>;

/// 流的状态。
struct StreamState {
    hub: std::sync::Arc<mcloud_app::EventHub>,
    run_id: RunId,
    subscriber: u64,
    /// 新事件通知。用它唤醒，不忙轮询。
    waiter: tokio::sync::broadcast::Receiver<RunId>,
    done: bool,
    /// 已经发出去的最大序号，用于去重。
    last_sent: Seq,
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// 手动指定起点（curl / 测试用），语义同 `Last-Event-ID`。
    #[serde(default)]
    pub after: Option<u64>,
}

/// 解析续传起点。
///
/// 缺省或格式非法都当作"从头开始"，**不**拒绝这条流 ——
/// 一个坏 header 不该让用户看不到日志。
fn parse_resume_point(headers: &axum::http::HeaderMap, query: Option<u64>) -> Seq {
    if let Some(after) = query {
        return Seq::seen(after);
    }
    headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Seq::seen)
        .unwrap_or(Seq::ZERO)
}

/// 组装一条 SSE 事件：带 `id:` 供浏览器续传，`data:` 是事件 JSON。
fn sse_event(item: &mcloud_core::Sequenced) -> Result<Event, serde_json::Error> {
    let data = serde_json::to_string(&item.event)?;
    Ok(Event::default()
        .id(item.seq.get().to_string())
        .event(item.event.event_name())
        .data(data))
}
