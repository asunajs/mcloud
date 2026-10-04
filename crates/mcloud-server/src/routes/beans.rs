//! 今日AI豆（云朵记录汇总）—— **只读**。
//!
//! 总览页那张「今日AI豆」卡的后端（前端契约见 `web/src/api.ts` 的
//! `BeanToday` / `getBeanToday`，口径见 `docs/task-inventory.md` §5）。
//!
//! # 这一条是**只读**的
//!
//! 整条链只有一枪 `GET cloudRecordV2`（见 `mcloud_tasks::beans`）。
//! 不取滑块、不下单、没有一条会改用户账上东西的请求 —— 刷新多少次都行。
//! 所以它走 `Runner::http_for`（与任务体同一条会话链）而**不**走
//! `Runner::run`：后者会过并发闸并落一条运行历史，那是给写操作用的。
//!
//! # 归属：只覆盖**当前用户自己的**账号
//!
//! 用 `store.accounts_for(&cur.0)`（与 `/api/accounts` 同一条判据），
//! 而 `cur` 是登录闸放行时塞进来的 [`CurrentUser`] —— 归属判定只有
//! 一处，不在这里另写一套。
//!
//! # 失败隔离：一个账号读不到，不该把整张卡打没
//!
//! 账号级失败由 `mcloud_tasks::beans::read_today` 收进 `failed`，
//! 路由这里**不会**因为它返回 4xx/5xx —— 那会把"这一格没数"说成"服务挂了"
//! （前端对这一格单独 `.catch`，见 `dashboard-page.tsx` 那段说明）。
//! 只有"连自己有哪些账号都读不出来"才是一个真错误。
//!
//! # `GET /api/beans/records`：明细（**不限于今日**）
//!
//! 点开某个账号看"这些数是怎么来的"。与 `/today` 两条硬区别：
//!
//! 1. **一次只读一个账号**（`?account=`）。明细是"我要看这一个号的账"，
//!    把所有账号的流水都拉回来是白花好几条上游请求。
//! 2. **失败就是失败**：这里不像 `/today` 那样把账号级失败收进 `failed` 字段 ——
//!    用户指名要这一个账号，读不到必须回一个错误码（上游读不到 ⇒ 502），
//!    200 + 空列表会被读成"这个号没有流水"。
//!
//! 数据源仍是**上游流水**（`cloudRecordV2`），与本仓的运行记录无关：
//! 后者是"我们做过什么"，前者是"上游账上发生了什么"。

use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use mcloud_core::{Error, HttpClient};
use mcloud_tasks::beans::{
    range_since_ms, read_records, read_today, BeanAccount, BeanRecord, BeansEnv, Today,
    MAX_DETAIL_RECORDS,
};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> axum::Router<ServerState> {
    axum::Router::new()
        .route("/api/beans/today", axum::routing::get(today))
        .route("/api/beans/records", axum::routing::get(records))
}

// ══════════════════════════ 响应形状 ══════════════════════════
//
// ⚠ 逐字段照 `web/src/api.ts` 的 `BeanToday`
// （`api.ts:341-348` 与 `api.ts:310-321`）。改这里必须同时改那边；
// 少一个字段前端那一格就静默空掉，多一个字段没人用。
//
// ⚠ **没有**跨账号的 `net` / `gained` 总额字段：豆是按账号各记一本账，
// 合计出来的数哪个账号都用不了（`api.ts:303-308` 那段与 §5 都写死了这一条）。
// 哪天想"顺手加一个总数"，先回去读那两段。

/// `BeanToday.accounts[]`（`api.ts:310-321`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeanTodayAccountView {
    /// 账号主键（哈希）—— 前端显示走 `accountName(id)`（`api.ts:312`）。
    pub account: String,
    /// 今日净增（进 − 出），**可负**。
    pub net: i64,
    /// 今日收入合计（正数）。
    pub gained: i64,
    /// 今日支出合计（取正数）。
    pub spent: i64,
    /// 今日读到几笔流水。
    pub count: i64,
}

/// `BeanToday.failed[]`（`api.ts:347`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeanTodayFailedView {
    pub account: String,
    /// **后端原话**的原因（原样透出，前端只显示不改写）。
    pub error: String,
}

/// `GET /api/beans/today` 的响应（`api.ts:341-348`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeanTodayResponse {
    /// 服务端算"今天"用的时刻（epoch 毫秒）—— 前端**不自己算日界**。
    pub now_ms: i64,
    /// 每个账号一份。空数组 = 一个账号都没配。
    pub accounts: Vec<BeanTodayAccountView>,
    /// 没读到流水的账号，以及原因。
    pub failed: Vec<BeanTodayFailedView>,
}

impl From<Today> for BeanTodayResponse {
    fn from(t: Today) -> Self {
        Self {
            now_ms: t.now_ms,
            accounts: t
                .accounts
                .into_iter()
                .map(|a| BeanTodayAccountView {
                    account: a.account.as_str().to_string(),
                    net: a.net,
                    gained: a.gained,
                    spent: a.spent,
                    count: a.count,
                })
                .collect(),
            failed: t
                .failed
                .into_iter()
                .map(|f| BeanTodayFailedView {
                    account: f.account.as_str().to_string(),
                    error: f.error,
                })
                .collect(),
        }
    }
}

/// `GET /api/beans/today` —— 读当前用户**每个**账号今天的AI豆。
///
/// 返回 `200` 即"这一趟读完了"：某个账号读不到也在 200 里，由 `failed`
/// 如实说出是谁、为什么。**不**把账号级失败映射成 4xx/5xx。
pub async fn today(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<BeanTodayResponse> {
    // 归属：只列**当前用户自己的**账号。别人的账号根本不会出现在这一列里。
    let accounts = state.handle.store.accounts_for(&cur.0).await?;

    // 一个账号一条自己的传输（会话/票是按账号建的，不能共用）。
    // `http_for` 不登记 run、不过并发闸 —— 只读端点正该如此。
    let transports: Vec<Arc<dyn HttpClient>> = accounts
        .iter()
        .map(|account| state.handle.runner.http_for(account))
        .collect();
    let sources: Vec<BeanAccount<'_>> = accounts
        .iter()
        .zip(transports.iter())
        .map(|(account, http)| BeanAccount {
            account: &account.id,
            http: http.as_ref(),
        })
        .collect();

    let env = BeansEnv {
        clock: state.handle.clock.as_ref(),
    };
    // ⚠ 逐账号失败在这里被收进 `failed`，整趟不会失败。
    Ok(Json(read_today(&env, &sources).await.into()))
}

// ══════════════════════════ 明细（不限于今日）══════════════════════════

/// 明细没给 `days` 时的窗口。**7 个自然日**：总览那张卡说的是"今天"，
/// 点开是想看"最近这几天的账"，只给今天等于点了个寂寞。
const DEFAULT_DETAIL_DAYS: i64 = 7;

/// `days` 的上限。十年足够覆盖任何真实用法，同时挡住
/// `days=999999999` 那种把 `since` 算到溢出亿年外的输入（乘法会 `overflow` panic ✗）。
const MAX_DETAIL_DAYS: i64 = 3_650;

/// `GET /api/beans/records` 的查询串。
#[derive(Debug, Deserialize)]
pub struct RecordsQuery {
    /// 账号主键（哈希）。必须是**当前用户自己的**账号 —— 别人的一律 404。
    pub account: String,
    /// 窗口：`0` = 全部；`n` = **含今天在内**的最近 `n` 个自然日（东八区日界）。
    /// 缺省 [`DEFAULT_DETAIL_DAYS`]。
    #[serde(default)]
    pub days: Option<i64>,
}

/// 一笔流水（`api.ts` 的 `BeanRecord`）。
///
/// ⚠ `at_ms` 是 `number | null`：上游的 `insertTime` **可能解不出**
/// （换了格式、或压根没有）。那条路**不丢这一笔**，而是把原串放进 `rawTime`
/// 让界面照原样显示 —— 明细列表里"少了一笔"和"这一笔时间看不懂"必须分得开。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeanRecordView {
    /// `insertTime` 归一成的 epoch 毫秒（**服务端**时区口径，见 `beans` 模块）。
    pub at_ms: Option<i64>,
    /// 原样的 `insertTime`。`atMs` 为 `null` 时界面显示它。
    pub raw_time: Option<String>,
    /// 正负号就是进出（**可负**）。
    pub num: i64,
    /// 上游给的摘要。
    pub summary: String,
    /// 上游给的备注/批次号。
    pub mark: String,
}

/// `GET /api/beans/records` 的响应（`api.ts` 的 `BeanRecords`）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeanRecordsResponse {
    /// 这一份明细是哪个账号的（原样回显，前端据此确认"弹窗里是哪个号"）。
    pub account: String,
    /// 服务端算窗口用的时刻（epoch 毫秒）—— 与 `/today` 同一条口径，前端不自己算日界。
    pub now_ms: i64,
    /// 回显请求的窗口档位（`0` = 全部）。
    pub days: i64,
    /// 窗口下界（epoch 毫秒）；`days = 0` 时为 `null`。
    pub since_ms: Option<i64>,
    /// 按时间**倒序**的流水。
    pub records: Vec<BeanRecordView>,
    /// 时间解不出的笔数（**已经在 `records` 里**）。
    pub undated: i64,
    /// 还有没读到的（撞上页数/条数上限）⇒ 这一份可能不全，界面要说出来。
    pub has_more: bool,
}

impl From<BeanRecord> for BeanRecordView {
    fn from(r: BeanRecord) -> Self {
        Self {
            at_ms: r.at_ms,
            raw_time: r.raw_time,
            num: r.num,
            summary: r.summary,
            mark: r.mark,
        }
    }
}

/// `GET /api/beans/records?account=<id>&days=<n>` —— 读**一个**账号的 AI豆流水明细。
///
/// # 错误码
///
/// * 缺 `account` / `days` 越界 ⇒ `400`（`Error::Config`）；
/// * 账号不在当前用户名下（含"根本没这个号"）⇒ `404` —— 归属错误一律 404，
///   不泄露"这个号存在但不是你的"（与 `routes/accounts.rs::remove` 同一条口径）；
/// * 上游读不到（网络 / 非 0 码 / 解析失败）⇒ 由 `Error` 自己映射（多为 `502`）。
///   **不**做"200 + 空列表"：那会被读成"这个号没有流水"。
pub async fn records(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<RecordsQuery>,
) -> ApiResult<BeanRecordsResponse> {
    let account_key = q.account.trim();
    if account_key.is_empty() {
        return Err(ApiError(Error::Config("缺少 account 参数".into())));
    }
    let days = q.days.unwrap_or(DEFAULT_DETAIL_DAYS);
    if !(0..=MAX_DETAIL_DAYS).contains(&days) {
        return Err(ApiError(Error::Config(format!(
            "days 只认 0（全部）或 1..={MAX_DETAIL_DAYS}（自然日），收到 {days}"
        ))));
    }

    // 归属：只认**当前用户自己的**账号（与 `/api/accounts` 同一条判据）。
    let accounts = state.handle.store.accounts_for(&cur.0).await?;
    let Some(account) = accounts.iter().find(|a| a.id.as_str() == account_key) else {
        return Err(ApiError(Error::NotFound(format!(
            "账号 {account_key} 不在当前用户名下"
        ))));
    };

    let now_ms = state.handle.clock.now_ms();
    let since_ms = range_since_ms(days, now_ms);
    // 一个账号一条自己的传输（会话/票是按账号建的，不能共用），与 `today` 同一条。
    let http = state.handle.runner.http_for(account);
    let got = read_records(http.as_ref(), since_ms, MAX_DETAIL_RECORDS).await?;
    // ⚠ 先把两个标志位取出来再搬 `records`：顺序反了会撞上"部分移动"。
    let (undated, has_more) = (got.undated, got.has_more);

    Ok(Json(BeanRecordsResponse {
        account: account.id.as_str().to_string(),
        now_ms,
        days,
        since_ms,
        records: got.records.into_iter().map(BeanRecordView::from).collect(),
        undated,
        has_more,
    }))
}
