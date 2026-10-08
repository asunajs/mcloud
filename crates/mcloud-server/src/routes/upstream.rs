//! 上游状态（**按需取**）—— 给「上游状态」那一页用的只读通路。
//!
//! # 为什么单独一条通路（用户口径 2026-10-07）
//!
//! 原话：「这个表单独放一个专门查看上游各种状态的页面，**需要主动获取**，而不是跟着
//! 其他任务一起」✓。
//!
//! ⇒ 这些数**不落运行历史、不占一条任务**：页面点一下 ⇒ 现去上游取一份 ⇒ 回给前端 ✓。
//! 形状与 [`super::beans`]（今日AI豆）**逐条相同**：走 `Runner::http_for` 而**不**走
//! `Runner::run` —— 后者会过并发闸并落一条运行历史，那是给**写**操作用的 ✓。
//!
//! ⚠ 别把这条链做成"跑一条只读任务"：2026-10-07 之前就是这么干的（`app-task-matrix`），
//! 用户当场否了 ——「app-task-matrix 这个任务本身就没有意义」✗。任务是要**跑出结果**的，
//! 而它什么都不做；要"看一眼"就去历史里翻一条假任务，这个模型本身就不对 ✓。
//!
//! # 这一族是只读的
//!
//! 今天只有 `taskListV2` 十六枪（2 个市场 × 8 个 group），没有一条会改用户账上的东西
//! ⇒ 刷新多少次都行 ✓。
//!
//! # 读不到必须报错，**不能回空表**
//!
//! 十六组**全**挂 ⇒ 502（`Error::Network`）。回 200 + 空清单会被读成"这个号没有任务" ✗
//! —— 那正是本仓最忌讳的"不发错、不报错，只是东西不对"。
//! 部分失败**照样 200**，但把 `succeeded` / `failures` 一起给出去：界面上必须能看出
//! "这份清单少了几组" ✓（少掉的那几组里的条目**根本不会出现**，而它和"表里没有"
//! 在界面上长得一模一样 ✗）。

use axum::extract::{Extension, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mcloud_core::Error;
use mcloud_tasks::{app_task_matrix, signin};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> axum::Router<ServerState> {
    axum::Router::new()
        .route(
            "/api/upstream/app-task-list",
            axum::routing::get(app_task_list),
        )
        // 签到历史 = `infoV3` 的当月日历（`cal[]`）—— **协议层早就解析了，
        // 一直没有出口**（见调研：`app_signin.rs` 的 `SignInDay` / `SignInStatus.days` ✓）。
        .route(
            "/api/upstream/signin-history",
            axum::routing::get(signin_history),
        )
        // 待领取奖品 = `signin/public/record`（只读，只查第一页 ✓）。
        .route(
            "/api/upstream/pending-prizes",
            axum::routing::get(pending_prizes),
        )
}

/// 要读哪个账号。
///
/// ⚠ **必填**（与 `/api/beans/records` 同口径 ✓）：这些读数一次只对一个账号有意义
///（票/会话是按账号建的，不能共用 ✓）。不给账号就"把所有账号都拉一遍"是白花几十条
/// 上游请求 ✗。
#[derive(Debug, Deserialize)]
pub struct AccountQuery {
    pub account: Option<String>,
}

/// 状态分布。三个数**分开**报（`unknown` 不能并进"未完成" ✗）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateCounts {
    pub done: usize,
    pub waiting: usize,
    pub unknown: usize,
}

/// 一次"按需取"的读数。
///
/// ⚠ 字段名与**以前那条只读任务**报的明细**逐字相同**（`taskId` / `currStep` /
/// `buttonAppLink` …，见 `app_task_matrix::items_json` ✓）⇒ 前端那份清单组件不用改 ✓。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppTaskListResponse {
    pub account: String,
    /// 取数时刻（东八区展示由前端管 ✓）。
    pub fetched_at_ms: i64,
    /// 要拉几枪（常量 16 ✓）。
    pub fetches: usize,
    /// 真成了几枪 ✓。
    pub succeeded: usize,
    /// 去重**前**的条数 ✓。
    pub raw_count: usize,
    /// 去重后的条数（= `items.len()` ✓）。
    pub deduped_count: usize,
    pub state_counts: StateCounts,
    /// 逐条。形状见 `app_task_matrix::items_json` ✓。
    pub items: Vec<Value>,
    /// 没读到的那几组（空表 = 十六组全成 ✓）。
    pub failures: Vec<String>,
}

/// `GET /api/upstream/app-task-list?account=<id>`
pub async fn app_task_list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<AccountQuery>,
) -> ApiResult<AppTaskListResponse> {
    let account = own_account(&state, &cur, q.account.as_deref()).await?;

    // 一个账号一条自己的传输（会话/票是按账号建的，不能共用），与 `beans` 同一条 ✓。
    let http = state.handle.runner.http_for(&account);
    // ⚠ 服务器这条链**没有**"取消"这个概念（页面关掉就没人要了，由连接断开体现 ✓）
    // ⇒ 那个回调恒为 `false` ✓。
    let (pages, failures) = app_task_matrix::fetch_all_with(http.as_ref(), || false).await?;

    // ⚠ 十六组**全**挂 ⇒ 502，不回空表（模块头那条 ✓）。
    if pages.is_empty() {
        return Err(ApiError(Error::Network(format!(
            "App 任务表一组都没读到（{} / {} 组失败）：{}",
            failures.len(),
            app_task_matrix::app_task_matrix_fetches(),
            failures.first().cloned().unwrap_or_default()
        ))));
    }

    let (items, raw_count) = app_task_matrix::merge_pages(&pages);
    let counts = app_task_matrix::state_counts_json(&items);
    let read = |k: &str| counts.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;

    Ok(Json(AppTaskListResponse {
        account: account.id.as_str().to_string(),
        fetched_at_ms: state.handle.clock.now_ms(),
        fetches: app_task_matrix::app_task_matrix_fetches(),
        succeeded: pages.len(),
        raw_count,
        deduped_count: items.len(),
        state_counts: StateCounts {
            done: read("done"),
            waiting: read("waiting"),
            unknown: read("unknown"),
        },
        items: app_task_matrix::items_json(&items),
        failures,
    }))
}

// ══════════════════════════ 签到历史（当月日历） ══════════════════════════

/// 日历里的一格。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInDayView {
    pub day: u8,
    pub signed: bool,
    /// 是否属于**本月** —— 非本月的格子是上下月的补位，**不算今天** ✓。
    pub current_month: bool,
    pub points: i64,
    pub today: bool,
}

/// 签到历史：当月日历 + 几个计数。
///
/// ⚠ 这是**上游那一格的读数**（`infoV3` 的 `cal[]` ✓），不是本仓的运行记录 ——
/// 后者是"我们做过什么"，前者是"上游账上发生了什么"，对不上是常态 ✓。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInHistoryResponse {
    pub account: String,
    pub fetched_at_ms: i64,
    /// 现行余额（豆）—— 上游字段 `total`。
    pub total: i64,
    /// 已连续签到天数（`signCount` ✓）。
    pub sign_count: i64,
    /// 本月已签天数（`monthDays` ✓）。
    pub month_days: i64,
    /// 今天签到**能拿**多少（不是已到手 ✓）。
    pub sign_in_points: i64,
    /// 待领取AI豆数（上游没给就是 `null` ✓）。
    pub to_receive: Option<i64>,
    pub days: Vec<SignInDayView>,
    /// **AI豆中心当下可做的事**（`receiveList` ✓）。
    ///
    /// ⚠ 它与上面的日历**来自同一发 `infoV3`** ⇒ 报出来**不多花一枪** ✓。
    /// 2026-10-07 之前这份清单是一条独立任务（`task-list`）—— 用户当场否了
    /// （「这个不是说没用吗」）：它只读、什么都不做，而数据本来就在这一发里 ✗。
    pub receive_list: Vec<ReceiveItemView>,
    /// 签到翻倍机会**这一趟能不能用**（`should_claim_multiple` ✓）。
    pub multiple_available: bool,
}

/// `receiveList` 里的一条。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveItemView {
    /// 上游**原始码**（`0` 现在可领 / `1` 已到期可领 / `2` 下月可领 ✓）——
    /// 原样带出来：认不出的码要让人能查，别只写"未知类型" ✗。
    pub cloud_type: i64,
    pub cloud_num: i64,
    /// 中文档位（**后端**给 —— 前端别各写一套映射 ✗）。
    pub label: String,
    pub record_id: Option<String>,
    pub accept_date: Option<String>,
}

/// `GET /api/upstream/signin-history?account=<id>`
pub async fn signin_history(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<AccountQuery>,
) -> ApiResult<SignInHistoryResponse> {
    let account = own_account(&state, &cur, q.account.as_deref()).await?;
    let http = state.handle.runner.http_for(&account);
    // ⚠ `deviceId` 要**从 store 里那份 smid 挑**（不是 `Account::device_id` ✓）——
    // 与任务侧同一套选择逻辑 ✓。
    let device_id = signin::h5_device_id_for(state.handle.store.as_ref(), &account, None).await;
    let st = signin::read_status(http.as_ref(), device_id.as_deref()).await?;

    // ⚠ 先把要**搬走**的两格取出来再构造响应：`days` 一旦 `into_iter()` 就把 `st` 部分
    // 移走了，后面再读 `st.xxx` 是借用已移走的值 ✗（`beans.rs` 里同款注释 ✓）。
    let multiple_available = st.should_claim_multiple();
    let receive_list: Vec<ReceiveItemView> = st
        .receive_list
        .iter()
        .map(|it| ReceiveItemView {
            label: receive_label(it.cloud_type),
            cloud_type: it.cloud_type,
            cloud_num: it.cloud_num,
            record_id: it.record_id.clone(),
            accept_date: it.accept_date.clone(),
        })
        .collect();

    Ok(Json(SignInHistoryResponse {
        account: account.id.as_str().to_string(),
        fetched_at_ms: state.handle.clock.now_ms(),
        total: st.total,
        sign_count: st.sign_count,
        month_days: st.month_days,
        sign_in_points: st.sign_in_points,
        to_receive: st.to_receive,
        days: st
            .days
            .into_iter()
            .map(|d| SignInDayView {
                day: d.day,
                signed: d.signed,
                current_month: d.current_month,
                points: d.points,
                today: d.today,
            })
            .collect(),
        multiple_available,
        receive_list,
    }))
}

// ══════════════════════════ 待领取奖品 ══════════════════════════

/// 一条奖品记录。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrizeView {
    pub prize_name: String,
    /// `flag == 1` ⇒ 待领取 ✓。
    pub pending: bool,
    pub market: String,
    pub oid: String,
    pub prize_id: i64,
    pub expire_time: Option<String>,
    pub insert_time: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPrizesResponse {
    pub account: String,
    pub fetched_at_ms: i64,
    /// 待领取的条数（`records` 里 `pending = true` 的那些 ✓）。
    pub pending_count: usize,
    /// ⚠ **只查了第一页**（10 条上限 ✓）—— 界面上必须说出来，
    /// 否则"就这些"会被读成"全部就这些" ✗。
    pub records: Vec<PrizeView>,
}

/// `GET /api/upstream/pending-prizes?account=<id>`
pub async fn pending_prizes(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Query(q): Query<AccountQuery>,
) -> ApiResult<PendingPrizesResponse> {
    let account = own_account(&state, &cur, q.account.as_deref()).await?;
    let http = state.handle.runner.http_for(&account);
    let records = signin::read_prizes(http.as_ref()).await?;
    let pending_count = records.iter().filter(|r| r.pending()).count();

    Ok(Json(PendingPrizesResponse {
        account: account.id.as_str().to_string(),
        fetched_at_ms: state.handle.clock.now_ms(),
        pending_count,
        records: records
            .into_iter()
            .map(|r| PrizeView {
                pending: r.pending(),
                prize_name: r.prize_name,
                market: r.market,
                oid: r.oid,
                prize_id: r.prize_id,
                expire_time: r.expire_time,
                insert_time: r.insert_time,
            })
            .collect(),
    }))
}

/// 归属：只认**当前用户自己的**账号（与 `/api/accounts` 同一条判据 ✓）。
///
/// ⚠ 不存在的账号与"不是自己的"回**同一个 404** —— 不泄露存在性 ✓（`beans.rs` 同口径）。
async fn own_account(
    state: &ServerState,
    cur: &CurrentUser,
    key: Option<&str>,
) -> Result<mcloud_core::Account, ApiError> {
    let key = key.unwrap_or("").trim();
    if key.is_empty() {
        return Err(ApiError(Error::Config("缺少 account 参数".into())));
    }
    let accounts = state.handle.store.accounts_for(&cur.0).await?;
    accounts
        .into_iter()
        .find(|a| a.id.as_str() == key)
        .ok_or_else(|| ApiError(Error::NotFound(format!("账号 {key} 不在当前用户名下"))))
}

/// `cloudType` → 中文档位。
///
/// ⚠ 认不出的码**原样报出来**（`类型 9`），不写"未知类型" ✗ ——
/// 用户原话「未知又是啥，不明不白」✓：看图的人要能查、也要知道该往哪加分支 ✓。
/// 三个已知码见 `mcloud_upstream::app_signin::cloud_type` ✓。
fn receive_label(cloud_type: i64) -> String {
    use mcloud_upstream::app_signin::cloud_type;
    match cloud_type {
        cloud_type::NOW => "现在可领".to_string(),
        cloud_type::EXPANSION => "已到期可领（延期领取）".to_string(),
        cloud_type::NEXT_MONTH => "下月可领".to_string(),
        other => format!("未识别的类型 {other}"),
    }
}
