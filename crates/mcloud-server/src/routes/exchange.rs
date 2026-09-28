//! 兑换（抢兑）的 HTTP 接口：**只读清单** + **跑一次兑换**。
//!
//! # 两条路由的性质**完全不同**，不要混着读
//!
//! | 路由 | 方法 | 碰网络吗 | 花豆吗 |
//! |---|---|---|---|
//! | `/api/accounts/{id}/exchange-prizes` | `GET` | 读一次清单（只读） | 否 |
//! | `/api/accounts/{id}/exchange` | `POST` | 清单 → 余额 → **下单** | ⚠ **真扣AI豆** |
//!
//! 第二条是**唯一**一条会花掉真AI豆的接口，所以它不自动重试、
//! 不做"顺手也兑一下别的"这类扩写，返回值里的档位如实回给前端。
//!
//! # 写路由走 `Runner`，不是自己建链
//!
//! 直接调 `mcloud_tasks::exchange::exchange` 会更省事，但那会丢掉两样东西，
//! 而它们恰好是这条链最要紧的：
//!
//! 1. **同 `task:account` 的并发闸**。用户连点两下"兑换"就是两条并发请求，
//!    而这条链的写操作**零幂等键**（`oid` 由服务端生成，见
//!    `mcloud_tasks::exchange` 开头那段）—— 并发的表现是同一件被扣两次豆。
//!    `Runner::run` 在第二条上直接回 `skipped("同一任务正在运行中")`，不排队。
//! 2. **运行历史落盘**。花真豆的那一下必须在历史里查得到。
//!
//! 代价是每个请求多一条 `runs` 记录与一次本地 JSON 写 —— 毫秒级，
//! 而那条记录正是我们要的。
//!
//! # 结论从 `details.reported` 读回来（**不重跑链**）
//!
//! `Runner::run` 只回 `TaskOutcome`（三档）与一句话，粒度比链上那 17 档
//! `ExchangePlan` + 8 档 `ExchangeStatus` 粗一层。硬把三档映射成结论，
//! 会把"余额不够"（跳过）与"上游回 415"（失败）压成同一格。
//!
//! 所以 `ExchangeTask::run` 通过 `ctx.details(...)` 自报一份结构化读数，
//! runner 把它并进 `RunRecord.details["reported"]`，本路由从那里读回来。
//!
//! ⚠ **绝不能**为了拿档位而在路由里再跑一遍链：那是第二次下单。

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mcloud_core::{Account, AccountId, Error};
use mcloud_tasks::exchange::{read_judged_prizes, ExchangeEnv, ExchangePlan, ExchangePrize};

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 与 `mcloud_tasks::exchange::ExchangeTask::name()` **同一个串**。
///
/// 走 `Runner` 就绕不开任务名，而两处各写一遍字面量的话，将来改名
/// 会得到一条"未知任务：exchange"的 400 —— 那时前端只看到兑换按钮失灵。
const EXCHANGE_TASK: &str = "exchange";

/// 没有余额读数时那句说明。写在服务端，让前后端**同一句话**。
const BALANCE_NOTE: &str =
    "这一份按「无余额读数」判定：余额不足这一档判不出来，所以这里的「可兑」比下单前的真判据宽 —— 点了仍可能被挡下，那次挡下会带回具体档位与上游错误码。";

pub fn routes() -> axum::Router<ServerState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/accounts/{id}/exchange-prizes", get(list_prizes))
        .route("/api/accounts/{id}/exchange", post(run_exchange))
}

// ══════════════════════════ 只读：奖品清单 ══════════════════════════

/// 清单里的一件，附**这一件现在能不能兑**的档位。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrizeView {
    pub prize_id: i64,
    /// 上游没给名字时为 `null`（**不补空串**：补了就看不出"没名字"还是"名字是空的"）。
    pub name: Option<String>,
    /// 兑换所需AI豆数。`null` = 上游没给。
    pub price: Option<i64>,
    pub group_id: Option<i64>,
    /// 判定档位的**机器可读名**（[`ExchangePlan::slug`]）。
    ///
    /// ⚠ 前端那张颜色表按**串**对齐，不是按数：档位顺序一变，
    /// 按数对齐的界面会把"本月已兑"染成"可兑"的绿。
    pub plan: &'static str,
    /// [`ExchangePlan::reason`] 的原话 —— 它就是"为什么不能兑"。
    pub plan_reason: &'static str,
    /// 能不能兑。即 `plan.blocks()`，给前端一个不必查表的布尔。
    pub blocks: bool,
    /// 上游读到的可兑状态（`limit` 五档的原始值）。排障用，界面别拿它当判据。
    pub limit: Option<i64>,
    /// 上游报的当前剩余 / 今日余量 / 余量阈值。
    ///
    /// ⚠ **时刻量**：会因为**别的用户**在同一分钟里兑掉而变化（实拍 `839 → 837`）。
    /// 只能用来判"现在还有没有"，不能当个人流水。
    pub count: Option<i64>,
    pub daily_remainder_count: Option<i64>,
    pub min_remainder_count: Option<i64>,
    pub on_line: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

/// 一次清单读取的结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangePrizesResponse {
    pub prizes: Vec<PrizeView>,
    /// 判档位用的时刻（epoch 毫秒，UTC）。
    ///
    /// 必须回给前端：`not_started` / `ended` 是**相对时刻**的结论，
    /// 页面显示"还没开售"却不说是什么时候判的，用户没法判断该不该刷新。
    pub now_ms: i64,
    /// ⚠⚠ **这一份"可兑"比真判据宽，必须让用户知道。**
    ///
    /// 本路由用 [`plan_exchange`] 的 `balance = None` 那一档（与参考实现给
    /// 选品器的那一档一致）：**没有余额读数**，所以判不出
    /// [`ExchangePlan::InsufficientCloud`] —— 余额不够的商品在这里显示为"可兑"。
    ///
    /// 代价当面说清：用户点了仍可能被链挡下，而**那一次挡下是有错误码的**
    /// （`status` + `plan` + `code` 一起回来），不会静默不发。
    ///
    /// 之所以不在这里顺手查一次余额：`infoV3` 是**账号级**请求，
    /// 多打一枪就多一分风控噪声，而"看一眼清单"这件事的收益不足以付它。
    /// 余额那一栏页面上本来就有别的路。
    pub balance_aware: bool,
    /// 上面那件事的人话说明（前端直接显示，不在前端另写一份）。
    pub balance_note: &'static str,
}

/// `GET /api/accounts/{id}/exchange-prizes` —— **只读**：读清单并逐件判定档位。
///
/// # 它是只读的，所以可以随意刷新
///
/// 只发一枪 `exchangeList`，不取滑块题、不碰 `infoV3`、更不发 `exchangeV3`。
/// 刷新多少次都不花豆，也不给滑块接口计失败次数。
pub async fn list_prizes(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> ApiResult<ExchangePrizesResponse> {
    let account = load_account(&state, &id).await?;
    // 与任务链**同一条**装饰链：会话由 `H5Client` 按主机透明注入。
    //
    // ⚠ `http_for` **不**过并发闸、不登记 run —— 所以它只给这一条**只读**路用。
    // 下面那条写路由必须走 `runner.run(...)`（理由见模块开头）。
    let http = state.handle.runner.http_for(&account);
    let now_ms = state.handle.clock.now_ms();

    // 走 `mcloud-tasks` 那一份读取与判定：那段逻辑它已经有一份
    // （`mcloud-server` 没有、也不该有 `mcloud-upstream` 依赖）。
    // 抄一份到这里的话，两份的差别会是"任务看得见的清单"与
    // "页面上看得见的清单"不是同一份。
    let env = ExchangeEnv {
        http: http.as_ref(),
        clock: state.handle.clock.as_ref(),
        // 只读路径**不碰** `deviceId`（那只在下单前才取，见 `ExchangeEnv`）。
        device_id: None,
    };
    let judged = read_judged_prizes(&env, now_ms).await?;

    let prizes = judged
        .into_iter()
        .map(|j| prize_view(&j.prize, j.plan))
        .collect();

    Ok(Json(ExchangePrizesResponse {
        prizes,
        now_ms,
        balance_aware: false,
        balance_note: BALANCE_NOTE,
    }))
}

fn prize_view(prize: &ExchangePrize, plan: ExchangePlan) -> PrizeView {
    PrizeView {
        prize_id: prize.prize_id,
        name: prize.name.clone(),
        price: prize.price,
        group_id: prize.group_id,
        plan: plan.slug(),
        plan_reason: plan.reason(),
        blocks: plan.blocks(),
        limit: prize.limit,
        count: prize.count,
        daily_remainder_count: prize.daily_remainder_count,
        min_remainder_count: prize.min_remainder_count,
        on_line: prize.on_line,
        start_time: prize.start_time.clone(),
        end_time: prize.end_time.clone(),
    }
}

// ══════════════════════════ 写：跑一次兑换 ══════════════════════════

/// 兑换请求体。
///
/// # 键名与任务参数**同名**（驼峰），这不是巧合
///
/// `prizeIdOrName` 与 `skipWait` 正是 `ExchangeTask::params()` 那两个名字，
/// 本路由把它们原样交给任务。取一套自己的名字会让"页面上点的那个"与
/// "定时任务里配的那个"变成两套入参，而匹配规则（纯数字按 ID、
/// 其余按名字**精确**相等）只有一处实现。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeRequest {
    /// 兑哪一件：纯数字按 `prizeId`，其余按商品名**精确**找。
    ///
    /// `#[serde(default)]` + 自己判空：缺字段时给出我们那句中文 400，
    /// 而不是 axum 的 `missing field`（Rust 字段名，对调用方没有指导意义）。
    #[serde(default)]
    pub prize_id_or_name: String,
    /// `false` ⇒ 先等到下一个整点再判一次。
    ///
    /// **缺省为 `true`**（立即试），与 `ExchangeTask::run` 的默认一致。
    /// ⚠ 这里**不能**图省事写成裸 `bool` 再靠 `Default`：那样缺字段时
    /// 零值 `false` 会让每个不带这个字段的请求都挂在那里等整点
    /// （最长一小时），而请求看起来只是"很慢"。
    #[serde(default = "default_skip_wait")]
    pub skip_wait: bool,
}

fn default_skip_wait() -> bool {
    true
}

/// 一次兑换的响应：**结论档位 + 判据 + 上游码**，全部如实回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeRunResponse {
    /// 这次运行在历史里的 id。**花真豆的那一下必须查得到**。
    pub run_id: u64,
    /// 结局档位（`mcloud_tasks::exchange::status_slug`）。
    ///
    /// ⚠ 它与 HTTP 状态码**不是一回事**：`skipped` 是正常答复
    /// （"判据没过，本次没发请求"），照样回 200 —— 把它变成 4xx
    /// 会让"这件本月已兑过"在界面上显示成一条红色的故障。
    pub status: String,
    /// 这一趟最终**生效**的那次判定档位。
    pub plan: String,
    /// "为什么不能兑"的人话。
    pub plan_reason: String,
    /// 一句话结论（与运行历史里那条一样）。给人看。
    pub summary: String,
    /// 上游回的业务码。`0` = 没发单。
    pub code: i64,
    /// 上游那句原文。
    ///
    /// 逐字透出而不是改写：`420`/`421`/`415` 这几档的区别**恰恰在那句文案里**，
    /// 改写会把它们抹平。它是上游写给人看的提示，不含凭据。
    pub message: String,
    /// 这一趟跑了几轮"取题 → 解算 → 下单"。
    pub rounds: u32,
    /// **实际发出的下单枪数**。`0` 就是"一枪都没发，一分豆都没花"。
    pub attempts: u32,
    /// 最后一枪带出去的 `puzzleOffset`；`null` = 没发过单。
    pub offset: Option<i64>,
    /// 下单**前**的余额读数。
    ///
    /// 判据没过时是 `0` —— 那是**没有读数**，不是"余额为 0"：
    /// 定位失败时链上不读余额（见 `judge_exchange`），原因由 `plan` 负责说。
    pub balance: i64,
    /// 下单**后**复查的余额；`null` = 没读到（不判成交）。
    pub after_balance: Option<i64>,
    /// 兑到的那一单（只在 `status == "ordered"` 时非 `null`）。
    ///
    /// 用 `Value` 透传而不是再声明一个结构：形状由
    /// `mcloud_tasks::exchange::detail_payload` 一处定义，
    /// 这里再抄一遍只会多一处要同步的地方。
    pub order: Option<Value>,
    /// 换不到 `deviceId` 的原因（非空 ⇒ 一枪没发）。
    pub device_id_failed: String,
    /// 本地失败原因（解算报错、等待被取消、余额复查失败…）。
    pub error: String,
    /// 这一趟到底**走到没走到**能报结论的那一步。
    ///
    /// `false` ⇒ 断在只读查询上（取票 / 清单 / 余额），一枪未发；
    /// 此时 `status` 是 `not_attempted`，而 `plan` / `plan_reason` 是**空串**
    /// —— 链根本没判过，编一个档位出来就是误报。
    pub reached: bool,
}

fn run_response(run_id: u64, summary: &str, reported: Option<&Value>) -> ExchangeRunResponse {
    let Some(v) = reported else {
        // 任务没自报 —— 不该发生（`ExchangeTask::run` 的 `Ok` 与 `Err` 两条
        // 路都自报），但真发生时**如实说**，不编一份"可以下单"的空结论。
        return ExchangeRunResponse {
            run_id,
            status: "not_attempted".into(),
            plan: String::new(),
            plan_reason: String::new(),
            summary: summary.to_string(),
            code: 0,
            message: String::new(),
            rounds: 0,
            attempts: 0,
            offset: None,
            balance: 0,
            after_balance: None,
            order: None,
            device_id_failed: String::new(),
            error: "任务没有自报结构化明细（details.reported 缺失），这一趟的档位读不到".into(),
            reached: false,
        };
    };

    let s = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default();
    let i = |key: &str| v.get(key).and_then(Value::as_i64).unwrap_or_default();
    let u = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or_default() as u32;

    ExchangeRunResponse {
        run_id,
        status: s("status").to_string(),
        plan: s("plan").to_string(),
        plan_reason: s("plan_reason").to_string(),
        summary: summary.to_string(),
        code: i("code"),
        message: s("message").to_string(),
        rounds: u("rounds"),
        attempts: u("attempts"),
        offset: v.get("offset").and_then(Value::as_i64),
        balance: i("balance"),
        after_balance: v.get("after_balance").and_then(Value::as_i64),
        order: v.get("order").filter(|o| !o.is_null()).cloned(),
        device_id_failed: s("device_id_failed").to_string(),
        error: s("error").to_string(),
        reached: v.get("reached").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// 从运行记录的 `details` 里取回任务自报的那一份。
///
/// # 两种形状都要容忍
///
/// runner 的约定是"只自报一条时不裹数组，多条时是数组"（见 `runner.rs` 收尾段）。
/// 兑换今天只自报一条，所以这里是**对象**；但将来有人在同一个任务体里
/// 自报多条时，只按对象读会让这一格**静默变成 `None`** ——
/// 那表现是"兑成功了但页面什么都不显示"。所以两种都读，取最后一条。
fn reported_of(details: &Value) -> Option<&Value> {
    match details.get("reported")? {
        Value::Array(items) => items.last(),
        other if !other.is_null() => Some(other),
        _ => None,
    }
}

/// `POST /api/accounts/{id}/exchange` —— **跑一次兑换（真扣AI豆）**。
///
/// # 200 与"成功"不是一回事
///
/// 只要走到了能报结论的那一步，就回 200 + 一个档位。这些都算正常答复：
///
/// | `status` | 意思 | 花豆了吗 |
/// |---|---|---|
/// | `ordered` | 下单成功且余额差对得上 | ⚠ 花了 |
/// | `already_exchanged` | 上游回"重复兑奖" ⇒ 目标本来已达成 | 没花 |
/// | `skipped` | **判据没过，一条写请求都没发** | 没花 |
/// | `failed` / `slider_unresolved` / `unknown` / `not_target_user` | 见各档说明 | 可能花了 |
///
/// ⚠ `skipped` 尤其**不能**映射成 4xx：它就是"这件本月已兑过 / 余额不够 /
/// 查无此件"这类**正常结论**。映射成错误码，历史里会多一条红色假故障，
/// 前端也没法把 `plan` 那一栏显示出来。
///
/// 只有"**没走到能报结论的那一步**"（账号不存在、取票失败、清单读不出来）
/// 才回错误，由 [`ApiError`] 映射成 4xx/5xx。
pub async fn run_exchange(
    State(state): State<ServerState>,
    Path(id): Path<String>,
    body: Option<Json<ExchangeRequest>>,
) -> ApiResult<ExchangeRunResponse> {
    // 空请求体是合法的（让下面那句中文 400 说话），所以用 `Option<Json<_>>`
    // 而不是 `Json<_>` —— 后者对空体直接回 422，而 422 的文本是 axum 的英文。
    let request = body.map(|Json(b)| b).unwrap_or_default();

    let prize = request.prize_id_or_name.trim().to_string();
    if prize.is_empty() {
        return Err(ApiError(Error::Config(
            "兑换没给奖品：prizeIdOrName 是必填（纯数字按 prizeId，其余按商品名精确找）".into(),
        )));
    }
    if !state.handle.registry.contains(EXCHANGE_TASK) {
        // 注册表里没有它 ⇒ 配置问题，不是用户输入问题。
        return Err(ApiError(Error::Config(format!(
            "任务池里没有 {EXCHANGE_TASK}（register_all 未注册？）"
        ))));
    }

    let account = load_account(&state, &id).await?;

    // `prizeIdOrName` 交给任务自己解析：纯数字 vs 名字那条规则
    // （**精确相等**，不是包含）只在 `locate_exchange_prize` 里有一份实现。
    // 在这里先解析一遍等于把它复制到第二处，而两处的分歧会是
    // "页面选的这件"与"实际下单那件"不是同一件 —— 花真豆的那种分歧。
    let task_body = serde_json::json!({
        "prizeIdOrName": prize,
        "skipWait": request.skip_wait,
    });

    let record = state
        .handle
        .runner
        .run(EXCHANGE_TASK, &account, &task_body)
        .await?;

    let reported = reported_of(&record.details);
    Ok(Json(run_response(
        record.run_id.get(),
        record.outcome.summary(),
        reported,
    )))
}

/// 把账号从存储里取出来；不存在时给一句能照着排查的话。
async fn load_account(state: &ServerState, id: &str) -> Result<Account, ApiError> {
    let account_id = AccountId::new(id);
    state
        .handle
        .store
        .account(&account_id)
        .await?
        .ok_or_else(|| ApiError(Error::NotFound(format!("账号不存在：{id}"))))
}

#[cfg(test)]
mod tests;
