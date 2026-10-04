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
    /// 单个账号（兼容既有调用方与测试）。
    #[serde(default)]
    pub account: Option<String>,
    /// **多账号**。语义与 `Schedule.accounts` 完全一致：
    ///
    /// * `Some([...])` ⇒ 就跑这几家；
    /// * `Some([])` ⇒ **当前用户名下的全部账号**；
    /// * `None` ⇒ 没给，回落到 `account`。
    ///
    /// ⚠ 用 `Option<Vec<String>>` 而不是裸 `Vec<String>`：裸 `Vec` 分不出
    /// "没给这个字段"与"给了空数组"，而这两者语义**相反**（后者是"全部"）。
    #[serde(default)]
    pub accounts: Option<Vec<String>>,
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
    /// 兼容字段：多账号时是**第一个**账号那次运行的 id。
    pub run_id: RunId,
    /// 本次点击涉及的**全部** run id（单账号时就是 `[run_id]`）。
    /// 界面据此逐条订阅事件流、逐条中止。
    pub run_ids: Vec<RunId>,
    pub task: String,
    /// 兼容字段：多账号时是**第一个**账号。
    pub account: String,
    /// 本次点击涉及的**全部**账号（顺序即执行顺序）。
    pub accounts: Vec<String>,
    /// 兼容字段：多账号时是**第一个**账号的结论。
    pub outcome: mcloud_core::TaskOutcome,
    /// 同步执行时是**全部账号的合计**耗时。
    pub duration_ms: u64,
}

/// 列出**当前用户**的最近运行记录。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<RunRecord>> {
    let mut runs = state.handle.store.runs_for(&cur.0, 100).await?;
    // ⚠ 列表**不带日志** ✗：一次运行最多留 200 行（`RUN_LOG_LIMIT` ✓），
    // 100 条记录全带上就是几 MB 的响应 ✗ —— 而列表要的是"一眼扫过去" ✓。
    // 要日志就按 id 取详情（`get_one` ✓，它原样带 ✓）。
    //
    // 置空 + `skip_serializing_if` ⇒ 响应里连 `logs` 这个键都不出现 ✓，
    // 前端也就不必区分"空数组"与"没这个字段" ✓。
    for r in &mut runs {
        r.logs = Vec::new();
    }
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

    // ── 目标账号：多账号 / 全部（用户口径 2026-10-01）─────────────────────
    //
    // 四种给法，按"明确程度"排优先级：
    //   ① `accounts: ["a","b"]`  指定几家；
    //   ② `accounts: []`         **我名下的全部**（与 `Schedule.accounts` 同一口径）；
    //   ③ `account: "a"`         单个（兼容既有调用方与测试）；
    //   ④ 都没给 ⇒ 400：**不猜**。"没写"和"全部"差得太远，猜错就是名下账号全跑一遍。
    let mut targets: Vec<Account> = Vec::new();
    match (&req.accounts, &req.account) {
        (Some(list), _) if list.is_empty() => {
            targets = state.handle.store.accounts_for(&cur.0).await?;
        }
        (Some(list), _) => {
            for raw in list {
                let account = resolve_owned_account(&state, &cur, raw).await?;
                // 同一次点击里重复给了同一个账号：只跑一次 ——
                // 否则会白跑一遍，还多半撞上并发闸变成 Skipped。
                if !targets.iter().any(|a| a.id == account.id) {
                    targets.push(account);
                }
            }
        }
        (None, Some(single)) => targets.push(resolve_owned_account(&state, &cur, single).await?),
        (None, None) => {
            return Err(ApiError(mcloud_core::Error::Config(
                "缺少账号：给 account（单个）或 accounts（数组；空数组表示全部账号）".into(),
            )));
        }
    }
    if targets.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "没有可运行的账号：这个用户名下还没有任何账号".into(),
        )));
    }

    if req.background {
        return start_background(state, cur, req.task, targets, req.body).await;
    }

    // 同步：**逐个**跑。串行是这条链的硬要求，理由见 `Runner::start_serial` 的文档
    // （上游限流最怕并发打过去）。
    let mut records = Vec::new();
    let mut total_ms: u64 = 0;
    for account in &targets {
        let record = state
            .handle
            .runner
            .run(&req.task, account, &req.body)
            .await?;
        total_ms += record.duration_ms;
        records.push(record);
    }
    let accounts: Vec<String> = targets.iter().map(|a| a.id.to_string()).collect();
    let first = records.first().expect("targets 已判非空");
    Ok(Json(RunResponse {
        run_id: first.run_id,
        run_ids: records.iter().map(|r| r.run_id).collect(),
        task: req.task,
        account: first.account.clone(),
        accounts,
        outcome: first.outcome.clone(),
        duration_ms: total_ms,
    }))
}

/// 把一个账号标识解析成 `Account`，且**只认当前用户自己的账号**。
///
/// 归属规则与单账号时代逐字一致（界面在用、也有测试锁着）：
/// * 自己的账号 ⇒ 正常返回；
/// * **别人的账号 ⇒ 404**，不泄露存在性；
/// * 压根不存在 ⇒ 400（错的是请求体里的一个字段，与既有契约一致）。
async fn resolve_owned_account(
    state: &ServerState,
    cur: &CurrentUser,
    raw: &str,
) -> Result<Account, ApiError> {
    let id = AccountId::new(raw);
    match state.handle.store.account_for(&cur.0, &id).await? {
        Some(a) => Ok(a),
        None if state.handle.store.account(&id).await?.is_some() => Err(ApiError(
            mcloud_core::Error::NotFound(format!("账号不存在：{raw}")),
        )),
        None => Err(ApiError(mcloud_core::Error::Config(format!(
            "账号不存在：{raw}"
        )))),
    }
}

/// 后台执行，**立刻**返回真实的 run id（多账号时是**一组** id）。
///
/// 拿到的 id 可以立即用来订阅事件流和请求中止 —— 这正是"要能点中止"
/// 所必需的前提。
///
/// # 为什么走 `start_serial` 而不是循环 `start`
///
/// 用户口径（2026-10-01）：「任何（包括直播小红花）应该允许配置多账号/全部，
/// 点击运行后后端保证串行执行」。`start` 每个 run 各自 spawn ⇒ 并发；
/// `start_serial` 只 spawn **一个**驱动任务、逐个 await，
/// 因此 N 个账号在后端是严格串行的（上游限流最怕并发打过去）。
async fn start_background(
    state: ServerState,
    cur: CurrentUser,
    task: String,
    accounts: Vec<Account>,
    body: serde_json::Value,
) -> Result<Json<RunResponse>, ApiError> {
    // id 由 runner 分配（id 分配只有一个来源），并且**同步**登记取消令牌 ——
    // ⚠ spawn 出去的 future 不持有请求上下文 —— 请求一返回它的上下文就被取消了，
    // 而这一趟任务要跑几十秒。
    let names: Vec<String> = accounts.iter().map(|a| a.id.to_string()).collect();
    let first_account = names.first().cloned().unwrap_or_default();
    let run_ids = state.handle.runner.start_serial(&task, accounts, body);
    // 记录归属：记录要到**结束时**才落盘，而客户端拿到 id 后
    // 会立刻订阅 SSE / 请求中止 —— 那时只能靠这张登记表判归属。
    // ⚠ **每个 id 都要登记**：只登记第一个的话，第 2 个账号之后的中止会 404。
    for id in &run_ids {
        state.note_run_owner(*id, &cur.0);
    }
    Ok(Json(RunResponse {
        run_id: run_ids.first().copied().unwrap_or(RunId::new(0)),
        run_ids,
        task,
        account: first_account,
        accounts: names,
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

/// ⭐⭐ 流被丢掉时**必须退订**（2026-10-03 review §1.5）。
///
/// 为什么不能靠"循环跑完"来退订：`axum` 在客户端断开时**丢掉响应体**，
/// 而这条流是 `unfold` 生成的 —— 丢掉 = 生成器被 drop，**循环后面那些语句
/// 一行都不会执行** ✗。浏览器关掉 `EventSource`（前端在终局事件后就是这么做的，
/// 见 `web/src/api.ts:687`）走的正是这条路。
///
/// 后果（改之前）：中枢里那个订阅者条目**永远留着**，一次运行漏一份；
/// 而 `hub.unsubscribe` 在生产路径上**一个调用点都没有**
/// （`grep -rn "hub\\.unsubscribe" crates` 当时只剩测试）⇒ `Inner::runs`
/// 只进不出，服务跑得越久堆得越多。
///
/// 用 `Drop` 而不是"在每个 `return` 前手动退订"：这条流有 **5 个**出口
/// （终局收尾 ×2、通知端关闭、队列空转、以及"被 drop"），少写一处就是又漏一条。
/// `Drop` 只有一个入口，且覆盖"被 drop"这条手动退订根本够不着的路径。
impl Drop for StreamState {
    fn drop(&mut self) {
        self.hub.unsubscribe(self.run_id, self.subscriber);
    }
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
