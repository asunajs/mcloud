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

use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::Stream;
use serde::{Deserialize, Serialize};

use mcloud_core::{visible_to, Account, AccountId, RunEvent, RunId, RunRecord, Seq, UserId};

use crate::auth_gate::CurrentUser;
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

/// 列出**当前用户**的最近运行记录。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<RunRecord>> {
    let runs = state.handle.store.runs_for(&cur.0, 100).await?;
    Ok(Json(runs))
}

/// 查询单次运行。
///
/// 不是自己的记录与不存在的记录**回同一个 404** —— 不泄露存在性。
pub async fn get_one(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<u64>,
) -> Result<Json<RunRecord>, ApiError> {
    match state.handle.store.run_for(&cur.0, RunId::new(id)).await? {
        Some(r) => Ok(Json(r)),
        None => Err(ApiError(mcloud_core::Error::NotFound(format!(
            "运行记录不存在：{id}"
        )))),
    }
}

/// 一次运行对某个用户的可见性。
///
/// `Unknown`（库里没有）与 `Denied`（有但不是自己的）**必须回同一个 404**，
/// 否则「存在性」就从响应码里漏出去了。
pub(crate) enum RunAccess {
    /// 库里 / 登记表 / 内存事件里都没有这个 run。
    Unknown,
    /// 存在，但不属于这个用户。
    Denied,
    /// 是这个用户的。
    Allowed,
}

/// 判定一次运行能不能被 `viewer` 看见 / 操作。
///
/// # 为什么不能只看存储层
///
/// 运行记录**结束时**才落盘，而客户端拿到 run id 后会立刻订阅 SSE / 请求中止。
/// 那一刻记录还不存在，只查存储层会把**正在跑的**运行判成「不存在」——
/// 而 `EventSource` 遇到 404 **不会重连**，界面就永远停在「执行中」。
/// 所以按三条来源依次判：
///
/// 1. 本服务发起时登记的归属（`ServerState::run_owners`，最稳、无时序问题）；
/// 2. 已落盘的记录（归属由它引用的账号决定，见 `JsonStore::run_visible`）；
/// 3. 内存中枢里的 `started` 事件（服务重启后仍在跑、或调度器发起的运行）。
pub(crate) async fn run_access(
    state: &ServerState,
    viewer: &UserId,
    run_id: RunId,
) -> Result<RunAccess, ApiError> {
    // ① 本服务发起的运行：归属在发起那一刻就登记了，不依赖事件先到。
    if let Ok(map) = state.run_owners.lock() {
        if let Some(owner) = map.get(&run_id) {
            return Ok(if owner == viewer {
                RunAccess::Allowed
            } else {
                RunAccess::Denied
            });
        }
    }

    // ② 已落盘的记录。
    if state.handle.store.run(run_id).await?.is_some() {
        return Ok(match state.handle.store.run_for(viewer, run_id).await? {
            Some(_) => RunAccess::Allowed,
            None => RunAccess::Denied,
        });
    }

    // ③ 还在跑、记录尚未落盘：从内存事件里取它的账号，再按账号归属判。
    let account = state
        .handle
        .hub
        .replay(run_id, Seq::ZERO)
        .into_iter()
        .find_map(|s| match s.event {
            RunEvent::Started { account, .. } => Some(account),
            _ => None,
        });
    if let Some(account) = account {
        let id = AccountId::new(account);
        if state.handle.store.account_for(viewer, &id).await?.is_some() {
            return Ok(RunAccess::Allowed);
        }
        if state.handle.store.account(&id).await?.is_some() {
            return Ok(RunAccess::Denied);
        }
        // 账号已被删：按「未归属」处理 —— 只有实例**唯一**的那个用户能看
        //（与存储层 `visible_to` 同一条规则：无主数据不对多用户摊开）。
        let sole = match state.handle.store.users().await?.as_slice() {
            [u] => Some(u.id.clone()),
            _ => None,
        };
        return Ok(if visible_to(viewer, None, sole.as_ref()) {
            RunAccess::Allowed
        } else {
            RunAccess::Denied
        });
    }

    Ok(RunAccess::Unknown)
}

/// 启动一次运行。
///
/// 默认同步执行并返回最终结果；`background: true` 时立即返回并后台执行。
pub async fn create(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<RunRequest>,
) -> Result<Json<RunResponse>, ApiError> {
    // 先校验任务名：否则未知任务会被误报成「账号不存在」，误导排查。
    //
    // 用 `Config`（→ 400）而不是 `NotFound`（→ 404）：错的是**请求体里的
    // 一个字段**，不是 URL 指向的资源。返回 404 会让客户端以为
    // `/api/runs` 这个端点不存在。engine 对同一情况也用 `Config`
    // （`runner.rs` 的 `unknown_task_is_config_error` 锁着），两边必须一致。
    if !state.handle.registry.contains(&req.task) {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "未知任务：{}",
            req.task
        ))));
    }

    let id = AccountId::new(&req.account);
    // 归属：只认自己的账号。**别人的账号回 404**，不泄露存在性；
    // 真不存在仍是 400（错的是请求体里的一个字段，与既有契约一致）。
    let account: Account = match state.handle.store.account_for(&cur.0, &id).await? {
        Some(a) => a,
        None if state.handle.store.account(&id).await?.is_some() => {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "账号不存在：{}",
                req.account
            ))));
        }
        None => {
            return Err(ApiError(mcloud_core::Error::Config(format!(
                "账号不存在：{}",
                req.account
            ))));
        }
    };

    if req.background {
        return start_background(state, cur, req.task, account, req.body).await;
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
    cur: CurrentUser,
    task: String,
    account: Account,
    body: serde_json::Value,
) -> Result<Json<RunResponse>, ApiError> {
    // Runner::start 内部自己分配 id 并 spawn，因此这里返回的 id 是真的。
    // ⚠ spawn 出去的 future 不持有请求上下文 —— 请求一返回它的上下文就被取消了，
    // 而这一趟任务要跑几十秒。
    let run_id = state.handle.runner.start(&task, account.clone(), body);
    // 记录这次运行的归属：记录要到**结束时**才落盘，而客户端拿到 id 后
    // 会立刻订阅 SSE / 请求中止 —— 那时只能靠这张登记表判归属。
    state.note_run_owner(run_id, &cur.0);
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
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<u64>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run_id = RunId::new(id);

    // ⚠ 归属判定必须在 `subscribe` **之前**：`subscribe` 会给中枢建一个空条目，
    // 之后再问就永远是「存在」了。
    //
    // 这里同时替代了原来那段「这个 run 存不存在」的三处判定：
    // `run_access` 内部就把存储层 / 取消登记 / 内存中枢三处都看了，
    // 并且**别人的运行与不存在回同一个 404**（不泄露存在性）。
    match run_access(&state, &cur.0, run_id).await? {
        RunAccess::Allowed => {}
        RunAccess::Denied | RunAccess::Unknown => {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "运行记录不存在：{id}"
            ))));
        }
    }

    // 断线续传的起点：`Last-Event-ID` 优先（浏览器自动带），其次 `?after=`
    let since = parse_resume_point(&headers, q.after);

    let hub = std::sync::Arc::clone(&state.handle.hub);
    // 订阅会把 `since` 之后的内存历史补进队列，因此"判存在"到"订阅"之间
    // 发布的事件不会漏。通知接收端要在订阅**之后**建：这样订阅到建接收端
    // 之间发布的事件已经在队列里（drain 会取到），之后发布的会唤醒我们。
    let subscriber = hub.subscribe(run_id, since);
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
                    // 发送端没了（进程在收尾）⇒ 结束这条流。
                    //
                    // 这里**不置 `st.done`**：下面的 `return None` 把 `st` 一起丢掉了，
                    // `done` 再也不会被读到（第 199 行那次读取在本轮之前）。
                    // 置了它只会得到 `unused_assignments` 告警 —— 而 `RUSTFLAGS=-D warnings`
                    // 会把它变成 CI 失败。与上面那条终局分支不同：那条要把 `st` 交回去，
                    // 所以 `st.done = true` 是真有意义的。
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
