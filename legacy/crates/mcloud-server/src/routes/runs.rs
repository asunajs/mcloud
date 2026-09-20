//! 运行历史与事件流。
//!
//! # 这一组为什么能提前落地
//!
//! `runs` / `run_events` 两张表、事件总线（`mcloud-exec`）都已经在了，
//! 缺的只是"把它们端出去"。所以这里实现的正是**读**的那一半：
//! 历史列表与事件流。写的另一半（`POST /api/runs` 真的跑一个能力）要等 P4 ——
//! 路由可以提前写，但后面没有东西可跑。
//!
//! # 事件流的两条路，各管一半
//!
//! - `run_events` 表：**持久**的那一半。进程重启、用户后来才打开页面、
//!   断线重连补历史，都靠它；
//! - 事件总线：**在线**的那一半。刚发布的事件不该等到落库才被看到
//!   （落库按账号批量做，延迟可达一整趟账号的执行时间）。
//!
//! 两者合起来才是"实时的、且断线可续的"。只发总线则刷新页面就丢历史，
//! 只读库则前端得轮询 —— 那正是这一条要替掉的东西。
//!
//! # `seq` 是续传的锚点
//!
//! 每条事件带 `id: <seq>`，浏览器重连时把它放进 `Last-Event-ID` 头。
//! 这里两者都收：`?after=` 给手工调用（curl / 测试）用，
//! `Last-Event-ID` 是浏览器自动重连时带的。语义相同，都是
//! "[`Seq::seen`]：这一条我已经看到了"（**不是**"下一条要取的值" —— 差 1 会静默跳事件）。

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use axum::routing::get;
use axum::{Extension, Json};
use mcloud_core::{AccountId, Error, RunId, RunRecord, Scope, Seq, Sequenced, StateStore, UserId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::{AppState, AuthUser};
use crate::error::{ApiError, ApiQuery};
use crate::routes::clamp_limit;

/// 历史列表的默认与最大条数。
const DEFAULT_RUNS: usize = 50;
const MAX_RUNS: usize = 200;

/// 一次从库里取多少条事件。断线续传靠 `seq` 往后接着要，不靠这一页装下全部。
const EVENT_PAGE: usize = 500;

/// 向总线拉取新事件的间隔。
///
/// 总线是**拉取式**的（`mcloud-exec` 不依赖 tokio，没有通知机制可用），
/// 所以这里每 50ms 拉一次**内存队列** —— 不查库，只是拿一把锁看一眼，
/// 成本与一次 map 查找同级。要替掉的"轮询"是**浏览器侧**的 HTTP 轮询：
/// 那是每个客户端、每秒一次真实请求；这里是每连接、每 50ms 一次本地检查。
const POLL: Duration = Duration::from_millis(50);

/// SSE 通道的容量。
///
/// 取 64 而不是 1024：这条通道的下游是**网络**，慢客户端会让它满，
/// 而满了的正确处置是让上面那个循环 `await`（背压），不是把事件堆在内存里。
const CHANNEL: usize = 64;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/{id}/events", get(run_events))
}

/// `GET /api/runs?limit=50`
///
/// 按开始时间倒序。旧数据里"匹配不到账号"的历史（`account_id` 为空）不会出现 ——
/// 那类记录无法归属，它们的去处是迁移报告。
async fn list_runs(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = clamp_limit(query.limit, DEFAULT_RUNS, MAX_RUNS);
    let runs = state
        .store()
        .list_runs(Scope::user(&auth.user.id), limit)
        .await?;

    Ok(Json(json!({
        "limit": limit,
        "runs": runs.iter().map(run_payload).collect::<Vec<_>>(),
    })))
}

/// `GET /api/runs/:id/events` —— SSE 事件流。
///
/// # 前端契约：收到终局事件后请自己 `close()`
///
/// 终局事件（`finished` / `errored`）发出去之后，这个响应**就结束了**。
/// `EventSource` 在连接关闭后会按规范自动重连，所以前端拿到终局事件必须主动
/// `close()`。不这么做的话，一个跑完的 run 会被浏览器一直重连
/// （每次带上 `Last-Event-ID`，服务端重放 0 条再关闭），日志里会看到一串无意义的请求。
///
/// 反过来"发完不关"更糟：那是每个打开的页面挂着一个永久任务 + 每 50ms 一次拉取。
///
/// # 归属
///
/// 第一次查库就完成了"这个 run 存在吗/是不是你的"两个判断，结果都是 404 ——
/// 归属错误**不能报成 403**，那等于确认了"这个 id 确实存在"。
async fn run_events(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<AfterQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run_id = RunId::parse(&id).ok_or_else(|| {
        // 形状不对的字符串**不可能是我们生成的**，所以"不存在"是最贴切的结论
        //（`RunId` 的文档就是这么约定的），不必先查一次库
        ApiError::Domain(Error::NotFound(format!("运行记录 {id}")))
    })?;

    // `after` = "客户端已经看到的那一条"
    let seen = query
        .after
        .map(Seq::seen)
        .or_else(|| last_event_id(&headers).map(Seq::seen));

    let scope = Scope::user(&auth.user.id);

    // 顺序是「先订阅总线、再读库」，两个原因：
    // ① 订阅会把总线里 `seen` 之后的历史灌进队列，于是"读库"与"订阅"之间发布的事件
    //    不会掉 —— 反过来的话那个窗口里的事件两条路都赶不上；
    // ② 订阅不需要知道 run 是否存在，而"是否存在"由紧接着的查库回答。
    let subscriber = state.bus().subscribe(&run_id, seen);

    // 这一次查询同时完成三件事：存在性/归属校验（不属于你 → NotFound）、
    // 取回总线里没有的那部分（进程重启后总线是空的）、给出历史。
    let persisted = state
        .store()
        .run_events_after(scope, &run_id, seen, EVENT_PAGE)
        .await?;

    let user_id = auth.user.id.clone();
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(CHANNEL);
    tokio::spawn(pump(state, user_id, run_id, subscriber, persisted, tx));

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

/// 把事件推给一个 SSE 连接，直到它断开或这个 run 出现终局事件。
///
/// 起一个独立任务（而不是在处理器里等）的理由很直接：处理器一返回，
/// 那个 future 就结束了 —— 而 SSE 的响应体必须**活得比处理器调用更久**。
async fn pump(
    state: AppState,
    user_id: UserId,
    run_id: RunId,
    mut subscriber: u64,
    initial: Vec<Sequenced>,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) {
    let bus = state.bus();
    let scope = Scope::user(&user_id);
    let mut last = Seq::start();

    // ① 库里已有的（总线里会有重叠 —— 落库的那批同时也发布过，靠 `last` 去重）
    match deliver(&tx, initial, &mut last).await {
        Some(false) => {}
        // 库里就带着终局事件（"来看一次已经跑完的 run"），或者客户端已经走了：
        // 两种都直接收尾。
        Some(true) | None => {
            bus.unsubscribe(&run_id, subscriber);
            return;
        }
    }

    // ② 在线的：轮询总线
    loop {
        if let Some(terminal) = deliver(&tx, bus.drain(&run_id, subscriber), &mut last).await {
            if terminal {
                break;
            }
        } else {
            // 客户端走了：**必须**退订，否则那条队列会一直留在总线里
            break;
        }

        if bus.dropped(&run_id, subscriber) > 0 {
            // 队列满时丢的是**最新**的那几条（见 `EventBus::publish`）。
            // 处置是重订阅（把 `last` 之后的历史再灌一遍，顺带把计数清零）
            // 加一次回库 —— 后者兜住"总线已经 `forget` 过这个 run"的情况。
            // 这里**不静默跳过**：丢事件不报出来，表现就是"日志少了半截"而没人知道。
            tracing::warn!(run_id = %run_id, "SSE 订阅者跟不上，正在补齐事件");
            bus.unsubscribe(&run_id, subscriber);
            subscriber = bus.subscribe(&run_id, Some(last));

            match state
                .store()
                .run_events_after(scope, &run_id, Some(last), EVENT_PAGE)
                .await
            {
                Ok(missing) => {
                    if deliver(&tx, missing, &mut last).await.is_none() {
                        break;
                    }
                }
                // 补不齐也不能把流打断：已经发出去的部分仍然有效，
                // 而客户端下次重连（带 `Last-Event-ID`）会再要一次。
                Err(err) => tracing::warn!(run_id = %run_id, error = %err, "补齐事件失败"),
            }
        }

        tokio::time::sleep(POLL).await;
    }

    bus.unsubscribe(&run_id, subscriber);
}

/// 发一批事件。
///
/// 返回 `None` = 客户端已经走了（通道关闭）；`Some(terminal)` 里的 `terminal`
/// 表示这一批里出现了终局事件（`finished` / `errored`）。
///
/// 序号 `<= last` 的一律跳过：订阅回放与库里取回的两批一定重叠，
/// 而**重复推**和**漏推**一样会让前端显示一个错的进度。
async fn deliver(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    batch: Vec<Sequenced>,
    last: &mut Seq,
) -> Option<bool> {
    let mut terminal = false;
    for item in batch {
        if item.seq <= *last {
            continue;
        }
        if tx.send(Ok(sse_event(&item))).await.is_err() {
            return None;
        }
        *last = item.seq;
        terminal |= item.event.is_terminal();
    }
    Some(terminal)
}

/// 一条事件 → 一条 SSE 消息。
///
/// `id` 就是序号（客户端重连时原样带回来），`data` 就是事件本身的 JSON ——
/// 形状由 `mcloud_core::RunEvent` 的 `#[serde(tag = "type")]` 定死，
/// 前端直接 `switch (ev.type)`。
fn sse_event(item: &Sequenced) -> Event {
    let data = serde_json::to_string(&item.event).unwrap_or_else(|e| {
        // 序列化失败理论上不可能（字段全是字符串与数字）。真发生了也不能让一条事件
        // 把整条流打断 —— 退化成一条看得见的错误，比静默断线强。
        json!({"type": "log", "level": "error", "message": format!("事件序列化失败: {e}")})
            .to_string()
    });
    Event::default().id(item.seq.value().to_string()).data(data)
}

/// 从 `Last-Event-ID` 里取序号（浏览器自动重连时带的那个头）。
///
/// 解析不出来时按"从头开始"处理而不是报错：这个头不在客户端的完全控制之下，
/// 为一个畸形值拒绝整条流，会让页面卡在"连不上"而看不出原因。
fn last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
}

/// 历史记录的对外形状。
fn run_payload(run: &RunRecord) -> Value {
    json!({
        "id": run.id,
        "accountId": run.account_id.as_ref().map(AccountId::as_str),
        "capability": run.capability,
        // `status` 是**存储层的生命周期**（running / done / failed），
        // 不等于业务结论：done 可以是"全成 / 部分成 / 需人工"三种。
        // 业务结论在事件流的 `finished` 里，别在这里补一个假的 outcome。
        "status": run.status,
        "startedAt": run.started_at.to_rfc3339(),
        "finishedAt": run.finished_at.map(|t| t.to_rfc3339()),
        "cloudGained": run.cloud_gained,
        "cloudAfter": run.cloud_after,
        "message": run.message,
    })
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct AfterQuery {
    after: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use mcloud_core::{LogLevel, RunEvent, RunOutcome};

    fn run_id() -> RunId {
        RunId::parse("550e8400-e29b-41d4-a716-446655440000").expect("测试用的固定 id")
    }

    #[test]
    fn last_event_id_is_read_from_the_header() {
        let mut headers = HeaderMap::new();
        assert_eq!(last_event_id(&headers), None);

        headers.insert("last-event-id", HeaderValue::from_static("7"));
        assert_eq!(last_event_id(&headers), Some(7));

        // 畸形值按"从头开始"处理，而不是拒绝整条流
        headers.insert("last-event-id", HeaderValue::from_static("abc"));
        assert_eq!(last_event_id(&headers), None);
    }

    /// `data:` 里是**事件本身**，不是一个包了一层的信封 —— 前端直接 `switch (ev.type)`。
    #[test]
    fn sse_data_is_the_event_itself() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("4"));
        assert_eq!(last_event_id(&headers), Some(4));

        let item = Sequenced::new(Seq::seen(4), RunEvent::progress(run_id(), 1, 3));
        let json = serde_json::to_string(&item.event).unwrap();
        assert!(json.contains("\"type\":\"progress\""), "{json}");
        assert!(json.contains("\"done\":1"), "{json}");
    }

    /// 终局事件是这条流唯一的结束条件 —— 它靠事件自己说，不靠"跑完了"这种猜测。
    #[test]
    fn terminal_events_are_exactly_finished_and_errored() {
        let id = run_id();
        assert!(!RunEvent::log(id.clone(), LogLevel::Info, "hi").is_terminal());
        assert!(!RunEvent::progress(id.clone(), 1, 2).is_terminal());
        assert!(RunEvent::finished(id.clone(), RunOutcome::Success, &[], None).is_terminal());
        assert!(RunEvent::errored(id, "boom").is_terminal());
    }
}
