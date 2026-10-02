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

use std::collections::HashMap;

use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mcloud_core::{Account, AccountId, Error, UserId};
use mcloud_tasks::exchange::{
    read_judged_prizes_with_balance, read_prize_groups, ExchangeEnv, ExchangePlan, ExchangePrize,
    JudgedPrize,
};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 与 `mcloud_tasks::exchange::ExchangeTask::name()` **同一个串**。
///
/// 走 `Runner` 就绕不开任务名，而两处各写一遍字面量的话，将来改名
/// 会得到一条"未知任务：exchange"的 400 —— 那时前端只看到兑换按钮失灵。
const EXCHANGE_TASK: &str = "exchange";

/// 余额读不到时那句说明。写在服务端，让前后端**同一句话**。
///
/// ⚠ 只在**真读不到余额**时才会发给前端（正常情况 `balanceNote` 是 `null`）。
/// 2026-10-02 之前这句话是无条件发的：那时这份清单刻意不读余额，
/// 于是界面每条都要挂一句"这里的「可兑」偏宽"。用户口径是
/// 「这是为啥这么说，无余额你就后端请求啊」—— 对的，一次只读请求换准确判据很值。
const BALANCE_NOTE: &str =
    "这一份没读到余额（上游那次查询失败了）：余额不足这一档判不出来，所以这里的「可兑」比下单前的真判据宽 —— 点了仍可能被挡下，那次挡下会带回具体档位与上游错误码。点「刷新清单」可以再试一次。";

pub fn routes() -> axum::Router<ServerState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/accounts/{id}/exchange-prizes", get(list_prizes))
        .route("/api/accounts/{id}/exchange", post(run_exchange))
        // 订阅：钉在顶部的那几件（用户口径 2026-10-02「按单个奖品订阅」+「存服务端」）
        .route("/api/exchange/subscriptions", get(list_subscriptions))
        .route(
            "/api/exchange/subscriptions/toggle",
            post(toggle_subscription),
        )
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
    ///
    /// ⚠⚠ [`Self::count`] 是**这一场活动的总池剩余**，**不是**"今天还能不能抢"：
    /// 2026-10-01 实拍 `251230032`（白银会员月卡）`count = 43,739` 而
    /// `minRemainderCount = 0`（H5 判"抢光"）—— 池子里还有货、**今日份额**没了。
    /// 界面上必须把这两件事分开说，否则"总池 4 万"和"已抢光"看着自相矛盾。
    pub count: Option<i64>,
    /// 上游 `totalCount`：这一场活动的**总投放量**（配 [`Self::count`] 就是"现量/总量"）。
    pub total_count: Option<i64>,
    /// 上游 `dailyCount`：**今日投放量**（配 `dailyRemainderCount` 是"今日还剩几份"）。
    pub daily_count: Option<i64>,
    pub daily_remainder_count: Option<i64>,
    pub min_remainder_count: Option<i64>,
    pub on_line: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    /// 这一件的**活动/品类组**（取自 `prizeGroup.json`），拿不到时为 `None`。
    ///
    /// 组是"共享状态"的单位：同组每月共用一个名额（`limit == 4` ⇒ `exchanged_this_group`）、
    /// 同组还共享同一个活动时间窗。所以界面按组分区，而**组名是上游给的**、不是我们编的。
    pub group_title: Option<String>,
    /// 该组的限兑规则，**上游原话**（实拍 `"(月卡每月多选一限兑)"`）。
    pub group_sub_title: Option<String>,
    /// 上游 `quota.monthQuantity`：这个组**每月能兑几件**（实拍音乐类是 `1`）。
    ///
    /// ⚠ 它是**展示用**的读数：判据链并不读它（判据读 `limit`）。别拿它算"还剩几个名额"
    /// —— 上游没给"本组已用几件"，用 `limit == 2` 的件数去凑会数错。
    pub month_quantity: Option<i64>,
    /// 上游 `quota.groupPrizeIds`：**与这一件互斥**的其它 prizeId（上游直接给的名单）。
    ///
    /// 用它回答"兑了这件，同组哪几件本月就兑不了了"；`None` = 上游没给这个键。
    pub group_prize_ids: Option<Vec<String>>,
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
    /// **这一份判据里有没有余额读数。**
    ///
    /// `true` = 读了（`infoV3`），于是 [`ExchangePlan::InsufficientCloud`]
    /// 这一档能判出来 —— 这是常态。
    /// `false` = 那次余额查询**失败**了，退回"无余额读数"口径：余额不够的会显示成"可兑"，
    /// 点了仍可能被挡下（那一次挡下带错误码）。此时 `balanceNote` 会给一句说明。
    ///
    /// ⚠ 2026-10-02 之前这里恒为 `false`：那时**刻意不读**余额（"省一枪"）。
    /// 用户口径：「这是为啥这么说，无余额你就后端请求啊」⇒ 改成**默认读**，
    /// 只有读失败才退回旧口径。代价是这条只读路多一枪 `infoV3`。
    pub balance_aware: bool,
    /// 这一份读到的余额（AI豆）。`null` = 没读到（原因见 `balanceNote`）。
    ///
    /// 显示它有用：判据里"余额不足"那一档就是拿它比出来的，
    /// 用户看到数字才能自己决定要不要先去做任务攒豆。
    pub balance: Option<i64>,
    /// 读不到余额时的人话说明（前端直接显示，不在前端另写一份）。
    ///
    /// ⚠ 常态是 `null`：**没问题就不说话**。原先这句话是无条件发的，
    /// 于是每条清单头顶都挂着一段"这里的可兑偏宽"，用户直接反问为什么不去请求余额。
    pub balance_note: Option<String>,
    /// 组名（`prizeGroup.json`）这一份**拿到了没有**；`None` = 拿到了。
    ///
    /// `Some(原因)` 时清单照常渲染，只是分区标题退回"第 N 组" —— 组名是**文案**，
    /// 不该因为它读失败就让整份清单失败。原因原样显示（别在前端另写一句）。
    pub groups_note: Option<String>,
}

/// `GET /api/accounts/{id}/exchange-prizes` —— **只读**：读清单并逐件判定档位。
///
/// # 它是只读的，所以可以随意刷新
///
/// 发**两枪只读**：`exchangeList`（清单）+ `infoV3`（余额，用来判"余额不足"那一档）；
/// 不取滑块题、更不发 `exchangeV3`。刷新多少次都不花豆，也不给滑块接口计失败次数。
///
/// ⚠ `infoV3` 那一枪是 2026-10-02 按用户口径加的（「无余额你就后端请求啊」）：
/// 拿一次只读请求换判据准确，值。
pub async fn list_prizes(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<ExchangePrizesResponse> {
    let account = load_account(&state, &cur.0, &id).await?;
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
    // 清单 + **余额**：这样"余额不足"那一档才判得出来（余额读失败不影响清单，
    // 退回无余额口径，由 `balance_note` 说明 —— 见 `read_judged_prizes_with_balance`）。
    let (judged, balance) = read_judged_prizes_with_balance(&env, now_ms).await?;

    // 组名与"每月多选一限兑"那条规则在**另一条静态接口**里（`prizeGroup.json`）。
    // 单独一枪、只读；拿不到就降级 —— 清单才是判据，组名只是文案。
    let (groups, groups_note) = read_groups(&env, &judged).await;

    let prizes = judged
        .into_iter()
        .map(|j| {
            let group = j
                .prize
                .group_id
                .map(|id| id.to_string())
                .and_then(|id| groups.get(&id).cloned());
            prize_view(&j.prize, j.plan, group.as_ref())
        })
        .collect();

    Ok(Json(ExchangePrizesResponse {
        prizes,
        now_ms,
        balance_aware: balance.is_some(),
        balance,
        // 读到了就**不说话**（常态）；只有读失败才挂那句说明。
        balance_note: balance.is_none().then(|| BALANCE_NOTE.to_string()),
        groups_note,
    }))
}

/// 组字典里、界面要用的那两样（组名 + 该组的限兑规则）。
#[derive(Clone)]
struct GroupInfo {
    title: Option<String>,
    sub_title: Option<String>,
}

/// 读组字典，把三档结果都收成"人话 + 空表"。**永不失败**：这一枪读不到不该拖垮清单。
///
/// `marketName` 取清单里第一个给了值的（同一份清单同属一场活动，实拍全是 `sign_in_3_ex`）。
async fn read_groups(
    env: &ExchangeEnv<'_>,
    judged: &[JudgedPrize],
) -> (HashMap<String, GroupInfo>, Option<String>) {
    let Some(market_name) = judged.iter().find_map(|j| j.prize.market_name.as_deref()) else {
        return (
            HashMap::new(),
            Some("这份清单没给 marketName，组名取不到（清单本身不受影响）".to_string()),
        );
    };
    match read_prize_groups(env, market_name).await {
        Ok(Some(list)) => (
            list.into_iter()
                .map(|g| {
                    (
                        g.group_id,
                        GroupInfo {
                            title: g.title,
                            sub_title: g.sub_title,
                        },
                    )
                })
                .collect(),
            None,
        ),
        // "推不出目录名"与"问了没成"要分开说：前者是我们没问，后者是上游那一下没成。
        Ok(None) => (
            HashMap::new(),
            Some(format!(
                "从 marketName「{market_name}」推不出组字典目录，本次没取组名"
            )),
        ),
        Err(e) => (
            HashMap::new(),
            Some(format!("组名读取失败（清单不受影响）：{e}")),
        ),
    }
}

fn prize_view(prize: &ExchangePrize, plan: ExchangePlan, group: Option<&GroupInfo>) -> PrizeView {
    let quota = prize.quota.as_ref();
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
        total_count: prize.total_count,
        daily_count: prize.daily_count,
        daily_remainder_count: prize.daily_remainder_count,
        min_remainder_count: prize.min_remainder_count,
        on_line: prize.on_line,
        start_time: prize.start_time.clone(),
        end_time: prize.end_time.clone(),
        group_title: group.and_then(|g| g.title.clone()),
        group_sub_title: group.and_then(|g| g.sub_title.clone()),
        month_quantity: quota.and_then(|q| q.month_quantity),
        group_prize_ids: quota.and_then(|q| q.group_prize_ids.clone()),
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
    /// **强制发单**（用户口径 2026-10-02：所有档都放行、点击就是真发请求）。
    ///
    /// `false`（缺省）⇒ 本地判成不能兑就如实说"一枪未发"，不扣豆；
    /// `true` ⇒ 抢光/未开始/已兑过都不再由本地挡下，**真发下单请求**（可能真扣豆）。
    /// 界面把它做成开关（`mcloud-exchange-force`），显式传。
    #[serde(default)]
    pub force: bool,
    /// **传输延迟补偿**（毫秒，缺省 ⇒ 任务那边的 10）。
    ///
    /// 用户口径 2026-10-02：「10ms 是本地与云端的传输时间差」—— 提前这么久发出，
    /// 让请求正好在整点抵达云端。界面把它做成可输入项，缺省不带（任务自己用 10）。
    #[serde(default)]
    pub offset: Option<i64>,
}

fn default_skip_wait() -> bool {
    // ⚠⚠ **有意与 TS 不同** —— 用户口径（2026-10-02）：「**用户点按钮就不用等**」。
    // 这个路由就是"用户点按钮"那条路，所以缺省**不等**；
    // TS（长跑脚本）默认 `skipWait=false` ⇒ 一直等到整点，那是另一种调用形态。
    // 而这里是**一个 HTTP 请求** —— 缺省等下去，最长一小时的静默会让它看起来
    // 就是"卡死了"（上面那段注释说的就是这个）。所以路由缺省**不等**，
    // 想等的用户在界面上显式打开那个开关（每次都会传这个字段）。
    // 任务层的缺省是 `false`（与 TS 一致，见 `mcloud_tasks::exchange`）——
    // 两处不同不是疏忽，是"脚本"与"请求"两种调用形态的差别。
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
    Extension(cur): Extension<CurrentUser>,
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

    let account = load_account(&state, &cur.0, &id).await?;

    // `prizeIdOrName` 交给任务自己解析：纯数字 vs 名字那条规则
    // （**精确相等**，不是包含）只在 `locate_exchange_prize` 里有一份实现。
    // 在这里先解析一遍等于把它复制到第二处，而两处的分歧会是
    // "页面选的这件"与"实际下单那件"不是同一件 —— 花真豆的那种分歧。
    // ⚠ `deviceId` 是**下单体里**那个键（`exchangeV3` 的 body），**不是**请求头 ——
    // 请求头那份由 `SmidClient` 自动补（见 `AppHandle.smid` 的说明），两者不是一回事：
    // `SmidClient` 补的是 `deviceid` 头，而这里要的是 body 里的 `deviceId` 键。
    //
    // 取的来源是**H5 那条 smid 指纹链**换回来的 id（用户口径："device 是 smid"）：
    // `SmidService::device_id` 是"库里有就用、没有才上报换一次"那条业务路径
    // （`report_device_id` 是"每次都真打上游"的换 id 路径，别用错）。
    //
    // ⚠ 不是 `account.device_id` —— 那一份是登录时**冒充 PC 客户端**用的，
    // 与 H5 兑换这条链不是同一件事。
    let device_id = state.handle.smid.device_id(&account.id).await?;

    let task_body = serde_json::json!({
        "prizeIdOrName": prize,
        "skipWait": request.skip_wait,
        "force": request.force,
        "offset": request.offset,
        "deviceId": device_id,
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

/// 取**当前用户自己的**账号；不存在或不是自己的，都是 404。
///
/// ⚠ 后一条是审计 P1-6 的重点：兑换会**真扣 AI豆**，绝不能让 B 用 A 的账号兑。
/// 「不是自己的」与「不存在」回同一句 404，不泄露存在性。
async fn load_account(state: &ServerState, viewer: &UserId, id: &str) -> Result<Account, ApiError> {
    let account_id = AccountId::new(id);
    state
        .handle
        .store
        .account_for(viewer, &account_id)
        .await?
        .ok_or_else(|| ApiError(Error::NotFound(format!("账号不存在：{id}"))))
}

// ══════════════════════ 订阅：钉在顶部的那几件 ══════════════════════

/// 一条订阅的**对外**形态。
///
/// ⚠ 字段名与持久化那份（`mcloud_core::ExchangeSubscription`）**不同**：
/// 存储用蛇形（与 `data/*.json` 其余文件一致 ✓），对外一律驼峰（本仓 HTTP 的惯例，
/// 见 `PrizeView`）✓ —— 所以这里不直接序列化核心类型，免得以后改存储顺手改了 API ✗。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub prize_id: i64,
    pub name: String,
    pub group_id: Option<i64>,
    pub group_title: Option<String>,
    pub subscribed_at_ms: i64,
}

impl From<&mcloud_core::ExchangeSubscription> for SubscriptionView {
    fn from(s: &mcloud_core::ExchangeSubscription) -> Self {
        Self {
            prize_id: s.prize_id,
            name: s.name.clone(),
            group_id: s.group_id,
            group_title: s.group_title.clone(),
            subscribed_at_ms: s.subscribed_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionsResponse {
    pub items: Vec<SubscriptionView>,
}

/// 切换一件的订阅状态。
///
/// ⚠ 取消订阅**只发 `prizeId` 就够**（`name` 缺省为空串 ✓）—— 顶部那几行要能
/// "一键取消"，而它手上不一定还留着当初的名字（换了云盘账号、清单里就没这件了 ✓）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleSubscriptionInput {
    pub prize_id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub group_id: Option<i64>,
    #[serde(default)]
    pub group_title: Option<String>,
}

/// `GET /api/exchange/subscriptions` —— 我订阅了哪几件。
pub async fn list_subscriptions(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<SubscriptionsResponse> {
    let items = state.handle.store.exchange_subscriptions(&cur.0).await?;
    Ok(Json(SubscriptionsResponse {
        items: items.iter().map(SubscriptionView::from).collect(),
    }))
}

/// `POST /api/exchange/subscriptions/toggle` —— 订阅 / 取消订阅一件。
///
/// # 为什么是"切换"而不是"加/删"两条路由
///
/// 用户那一下点的就是**一个开关**（订阅 ↔ 取消）✓。做成切换之后，
/// **判断依据在服务端**（它手里那份列表才是准的）⇒ 两个标签页各点一下不会
/// 变成"都加了一遍"或"都删掉了" ✗。规则本体在 `mcloud_core::exchange::toggled`
/// （纯函数，四条单测钉着：往返、快照、顺序、老文件可读 ✓）。
pub async fn toggle_subscription(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(body): Json<ToggleSubscriptionInput>,
) -> ApiResult<SubscriptionsResponse> {
    let current = state.handle.store.exchange_subscriptions(&cur.0).await?;
    let next = mcloud_core::exchange::toggled(
        &current,
        body.prize_id,
        &body.name,
        body.group_id,
        body.group_title,
        state.handle.clock.now_ms(),
    );
    state
        .handle
        .store
        .save_exchange_subscriptions(&cur.0, &next)
        .await?;
    Ok(Json(SubscriptionsResponse {
        items: next.iter().map(SubscriptionView::from).collect(),
    }))
}

#[cfg(test)]
mod tests;
