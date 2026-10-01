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

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::Json;
use serde::Serialize;

use mcloud_core::HttpClient;
use mcloud_tasks::beans::{read_today, BeanAccount, BeansEnv, Today};

use crate::auth_gate::CurrentUser;
use crate::error::ApiResult;
use crate::ServerState;

pub fn routes() -> axum::Router<ServerState> {
    axum::Router::new().route("/api/beans/today", axum::routing::get(today))
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
