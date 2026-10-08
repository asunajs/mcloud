//! 「直播自动定时」的**组装层**：取「下一场」→ 算时刻 → upsert 那条定时。
//!
//! # 这件事的形状（用户口径 2026-10-08，逐条锁死）
//!
//! ```text
//! 每个**登录用户**一条「自动直播定时」
//!   id 形如 auto-live-<owner>          ⇒ 确定性 ⇒ upsert 幂等 ✓
//!   带一个"系统维护"标记（managed_by）  ⇒ 系统只动自己这条，绝不碰用户手建的 ✗
//!   跑什么    直播任务（live-room：检测口令 + 领取）
//!   作用于    该定时上配置的**一个或多个云盘账号**（复用 Schedule::accounts ✓）
//!   触发时刻  这些账号里**最早**的那一场「下一场直播开始时间」+ **1 小时**
//!   怎么更新  每次直播任务跑完就重算并 upsert（延期 / 改期自动跟上 ✓）
//!   算不出下一场 ⇒ **停用**（enabled=false ✓，**不删** ✓）
//!   它从哪来  **开箱就有**：每个用户在自己的默认数据那一步被种上一条
//!             （`seed_auto_schedule` ✓，意愿=开、先关着、还没算出时刻 ✓）
//! ```
//!
//! # ⚠ 两条入口别搞混：**种**（seed）与**重算**（reschedule）
//!
//! * [`seed_and_reschedule_auto`]：**只在新种出来时**才顺手重算一次
//!   （`AppHandle::seed_defaults_for` 调它 ✓ —— 启动与登录都经那里 ✓）。
//!   已经有这条就**一个字节都不动、一个上游请求都不发** ✓；
//! * [`reschedule_live_auto`]：**每次直播任务跑完**都重算并 upsert（`spawn_after_run` ✓），
//!   以及界面上的 `PUT` 立刻重算一次 ✓ —— 它才是有下一场时"把时刻写进去"的那条路 ✓。
//!
//! # ⚠ 「下一场」**向上游现取**（用户口径 2026-10-08）
//!
//! 用户原话：「**下一场应该从上游获取啊**」✓ —— 上一版读的是**共享口令池里缓存
//! 的那一份**（`store.kouling_pool()` ✓），那是"最近一次探测的结论" ✗。
//!
//! 现取的那条路是 `mcloud_tasks::live_room::fetch_next_start`（门的三步：换 SSO →
//! 换直播侧 JWT → 拉两个页签 ✓，请求构造全用 `mcloud_upstream` 现成的那套 ✓），
//! **每个账号各取一次**（用那个账号自己的票 ✓）。
//!
//! ## ⚠⚠ 上游那份日程是**按主播的**（平台级）—— 事实，别写成"按账号各不相同" ✗
//!
//! 第③步 `tabContentList?channelId=134` 的入参只有 `tabId` 与"我是谁"（JWT ✓），
//! 内容按**固定频道**给，真正干活的判据是 `anchor_nickname == KOULING_ANCHOR` ✓
//! ⇒ 上游这份日程是**按主播的**（平台级、**不是**按账号分的 ✓）：
//! 逐账号取回来的是**同一份** ✓ —— 凭据只决定"这个账号**能不能**取到" ✓
//!（"取最早"因此在今天退化成"取那一个值" ✓）。
//!
//! ⇒ 仍然**逐账号各取一次**（用户口径 ✓）：票是每个账号自己的（凭据坏了 / 未登录时
//! 只有那个账号取不到 ✓），而"上游哪天按账号分时"时这一层**不需要改结构** ✓。
//! 顺带解掉一个已知退化：池是**实例级共享**的 ⇒ 全仓本来没有"按云盘账号的下一场"
//! 这种东西 ✗，现在这条定时是"每个登录用户一条、作用在他选中的多个账号上" ✓，
//! **必须按账号各取一次**才谈得上"取最早那一场" ✓。
//!
//! ## 判据**不在这一层**（别在这儿重写一份 ✗）
//!
//! 「下一场」的判据只有一份：`mcloud_core::kouling::next_room_at`
//!（按唯一有效主播筛 → 只认预告场 → 跳过已过去的场次 → 取最早 ✓）。
//! 池那条路与上游这条路**都调它** —— 两处各写一遍迟早分叉，
//! 而用户已经见过一次那样的后果（「16:31 还显示 15:00」✗）。
//! 「取最早 + 1 小时」同样只有一份：[`earliest_plus_one_hour`] ✓。
//!
//! # ⚠ 兜底链：**上游 → 池 → 停用**（三段各有理由）
//!
//! ```text
//! ① 上游现取（首选）   唯一"现在真的几点"的来源 ✓；用户口径就是它 ✓
//! ② 池里那份（兜底）   上游取不到时（网络错 / 未登录 / 上游没给这一格 ✓）
//!                      仍然给用户一个**最近探测到的**时刻 —— 比"什么都不做"
//!                      更接近真相（池里那份最多旧一次探测，而场次是几小时/几天后的 ✓）；
//!                      代价是它可能已经过期 ⇒ 判据照旧跳过已过去的场次 ✓
//! ③ 两个都没有 ⇒ 停用  **不删**（用户在这条上挑的账号与他的意愿都要留着 ✓，
//!                      见 [`note_for`] 与 `mcloud_core::live_auto` 的模块头 ✓）
//! ```
//!
//! ⚠ **部分账号成功就够**：只要有一个账号给出了时刻，就**不再**混进池里那份 ——
//! 上游这条路是通的，把"这次现取"与"上次探测"两个不同口径的时刻混在一批里比，
//! 只会让"最早"变成一件说不清的事 ✗。
//!
//! # ⚠ 为什么"算不出就停用而不是删"（用户口径）
//!
//! 用户原话：「查不到『下一场』时 ⇒ **停用**（条目留着，下次算出来自动启用）」✓。
//! 删掉的话：用户在这条上做过的两件事会一起消失 —— **账号多选**（他挑的那几个
//! 云盘账号 ✓）与**意愿**（他关掉它这件事 ✓）。而"算不出下一场"是个**暂时**状态
//!（今天没开播 / 上游还没放预告 / 场次都过去了 ✓），不是"以后都不要了" ✓
//! ⇒ 留一条 `enabled=false` 的条目，下次算出来直接改回 `true` ✓。
//!
//! 真想不要了就**删掉整条** ✓ —— 那是用户的动作，系统下次重算会再建一条
//!（"每个用户一条"就是这个语义 ✓）。
//!
//! # ⚠ 为什么用户手改的时刻会被覆盖
//!
//! 见 `Schedule::managed_by` 那段长注释：**时刻归系统、开关与账号归用户** ✓。
//! 覆盖只发生在 `expr` 这一格 ✓（[`reschedule_live_auto`] 里那一行），
//! `accounts` 与 `managed_wanted` 每次都原样带过去 ✓。

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use mcloud_core::live_auto::{
    beijing_now, earliest_plus_one_hour, live_auto_schedule_id, one_shot_expr, LIVE_ROOM_TASK,
    MANAGED_BY_LIVE_AUTO,
};
use mcloud_core::{Account, AccountId, Clock, Result, Schedule, ScheduleId, Store, UserId};

use crate::notify::{DeliveryGuard, Notifier};

/// 那条自动定时的显示名（只在**新建**时用；用户改过名字就留着他的 ✓）。
pub const AUTO_LIVE_NAME: &str = "直播自动定时";

/// 种下那条定时时的**占位表达式**（= "还没算出下一场" ✓）。
///
/// ⚠ 与 [`reschedule_live_auto`] 首次创建时写的那一格**必须同一个值** ⇒ 抽成常量、
/// 两处共用（各写一遍的话，"这条现在是什么形态"迟早各说各话 ✗）。
///
/// 它是一条**能解析的合法 cron**（每天 08:00），**不是**一个坏串：坏串会让
/// "这条定时为什么解析不了"变成一个假故障 ✗。⚠ 而它**永远不会真的触发** ——
/// 种下去时 `enabled=false`，而 `mcloud_engine::scheduler::decide` 对停用的定时
/// **直接返回 `Disabled`**、连表达式都不解析 ✓；真要跑，得先有一次重算把 `expr`
/// 换成算出来的那一刻（[`reschedule_live_auto`] ✓）。
pub const SEED_PLACEHOLDER_EXPR: &str = "0 8 * * *";

/// 一次重算里**每个账号**去上游取「下一场」的总超时。
///
/// # 为什么必须有这个超时
///
/// 重算挂在**运行收尾的后台任务**上（`spawn_after_run` ✓），而 CLI 的 `flush()`
/// 会等它（同一个在飞计数 ✓）—— 上游卡住时不能让收尾无限期挂着 ✗。
///
/// 一个账号最多 4 枪（① SSO ② JWT ③④ 两个页签 ✓），10 秒对"4 枪正常请求"已经很宽；
/// 真超了就按"这个账号这次没取到"走兜底 ✓（**不**把整次重算判失败 ✗）。
pub const UPSTREAM_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 给前端那张卡的状态快照（`GET/PUT /api/live-room/auto-schedule` 的响应体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoLiveState {
    /// **现在能不能跑**（= 用户想要 **且** 算得出下一场 ✓）。
    ///
    /// ⚠ 它与那条定时自己的 `enabled` **同一格**（不是"另一份状态"）——
    /// 卡片上那个开关显示的就是调度器会看到的那一格 ✓。
    pub enabled: bool,
    /// 算出来的触发时刻：**下一场 + 1 小时**（东八区墙钟串，与上游同形 ✓）。
    ///
    /// `None` = 现在算不出来（没探测过 / 场次都过去了 / 已被停用 ✓）。
    /// ⚠ 不给"看起来像时间"的占位值 —— 那是骗人 ✓。
    pub at: Option<String>,
    /// 这条定时作用的云盘账号（用户多选 ✓；空 = **全部已启用账号** ✓，
    /// 与 `Schedule::accounts` 同一条语义 ✓）。
    pub accounts: Vec<String>,
    /// 一句人话，说清"为什么现在是这个状态"（卡片直接显示 ✓）。
    ///
    /// ⚠ 只在**说不清**的时候才给：正常有下一场时是 `None` ✓
    ///（界面上那行时刻已经说明了一切，再挂一句话是噪音 ✗）。
    pub note: Option<String>,
}

/// 用户在界面上改的那两件事（`PUT` 专用 ✓）。
///
/// ⚠ 与 `Schedule` 的编辑分开：这里**只能**改"开关"与"账号多选" ✓ ——
/// 时刻是算出来的（用户改了也会被覆盖 ✓），`expr` 不在这个结构里 ✓。
#[derive(Debug, Clone, Default)]
pub struct AutoLivePatch {
    /// 用户意愿（`None` = 不改 ✓）。
    pub wanted: Option<bool>,
    /// 账号多选（`None` = 不改 ✓；`Some(vec![])` = 清空 = 全部已启用账号 ✓）。
    pub accounts: Option<Vec<String>>,
}

// ════════════ 种下那条定时（用户口径 2026-10-08：「小红花定时默认开启」✓）════════════
//
// 「默认开启」在**这一层**的意思是「**开箱就有这条定时**」（用户口径 2026-10-07：
// 「不自动新建怎么算默认」✓）：每个用户在自己的**默认数据**那一步
//（`AppHandle::seed_defaults_for` ✓ —— 服务启动与登录都调它 ✓）顺手被种上一条，
// 而不是"等他跑过一次直播任务才出现" ✗。
//
// ⚠ 种下去的**不是**"已经能跑"：`enabled=false`（还没算出下一场 ✓），
// 而**意愿**那一格是 `Some(true)`（口径里的"默认开启"✓）—— 两格各记一件事，
// 理由见 `mcloud_core::Schedule::managed_wanted` ✓。

/// 那条自动定时**种下去时的形态**（"开箱就有"的那一格 ✓）。
///
/// # 四格值（用户口径 2026-10-08 定稿 ✓）
///
/// | 格 | 值 | 为什么 |
/// |---|---|---|
/// | `managed_by` | `Some(MANAGED_BY_LIVE_AUTO)` | 系统只动自己这条，绝不碰用户手建的 ✓ |
/// | `managed_wanted` | `Some(true)` | **用户意愿 = 要** —— 口径就是「小红花定时默认开启」✓ |
/// | `enabled` | `false` | **先关着**：还没算出下一场；写 `true` 等于骗调度器 ✗ |
/// | `expr` | [`SEED_PLACEHOLDER_EXPR`] | 占位（停用 ⇒ 永不触发 ✓，见那个常量的说明 ✓）|
///
/// 其余：任务 `live-room` ✓、账号留空（= 全部**已启用**账号，照 `Schedule::accounts`
/// 的既有语义 ✓）、时区 `Asia/Shanghai`（与 [`one_shot_expr`] 那个东八区墙钟配套 ✓）。
///
/// ⚠ 归属写**这个用户**（`Some(owner)` ✓）：只有他自己看得见、改得动 ✓
///（与默认编排 `daily-<owner>` 同一条口径 ✓）。
fn seeded_auto_schedule(owner: &UserId) -> Schedule {
    Schedule {
        id: live_auto_schedule_id(owner),
        name: AUTO_LIVE_NAME.to_string(),
        task: LIVE_ROOM_TASK.to_string(),
        accounts: Vec::new(),
        expr: SEED_PLACEHOLDER_EXPR.to_string(),
        timezone: "Asia/Shanghai".to_string(),
        body: serde_json::json!({}),
        enabled: false,
        managed_by: Some(MANAGED_BY_LIVE_AUTO.to_string()),
        managed_wanted: Some(true),
        owner: Some(owner.clone()),
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    }
}

/// **幂等**种下那条自动定时；返回"这次是不是**新种出来的**" ✓。
///
/// # 已经有就**一个字节都不动**（尤其别把用户的意愿冲掉 ✗）
///
/// 判据按 **id 全局查**（`Store::schedule`，不是 `schedule_for`）：
/// `ScheduleId` 是**全局唯一**的（`Store::schedule` 按 id 查 ✓），而 id 是
/// **用户也能写**的东西 —— 别人完全可以手建一条恰好叫 `auto-live-<我的id>` 的定时 ✗。
/// 按 `(owner, id)` 查会查不到它 ⇒ 接着 `save_schedule` 就把它**覆盖**掉了 ✗
/// （那是"系统动了用户手建的东西"，正是 `Schedule::managed_by` 要避免的事 ✓）。
///
/// ⚠ 于是"已经有"分两种，两种都**什么都不写** ✓：
/// * 系统维护的那条（`managed_by = Some(live-auto)` ✓）—— 尤其**不覆盖
///   `managed_wanted: Some(false)`**：那是用户**明确关过**（界面上那个开关 ✓），
///   覆盖成 `Some(true)` 就是"我关了的开关自己又开了" ✗；
/// * 用户手建的（`managed_by = None` ✓）—— 那种撞车由 [`reschedule_live_auto`]
///   的撞车保护如实说明（`squat_note` ✓），种这一步同样不碰它 ✓。
pub async fn seed_auto_schedule(store: &Arc<dyn Store>, owner: &UserId) -> Result<bool> {
    let id = live_auto_schedule_id(owner);
    if store.schedule(&id).await?.is_some() {
        return Ok(false);
    }
    store.save_schedule(&seeded_auto_schedule(owner)).await?;
    tracing::info!(
        owner = %owner,
        schedule = %id,
        "已种下「直播自动定时」（意愿=开、暂不可跑：还没算出下一场）"
    );
    Ok(true)
}

/// **种下 + 只在确实需要时在后台重算一次** —— `AppHandle::seed_defaults_for` 调它 ✓
/// （服务启动与**每次登录**两条路都经这里 ✓）。
///
/// # 判据：**只有"这一次真的把它种出来了"才算需要** ✓
///
/// 种出来的那一刻它必然是 `enabled=false`（还没算出下一场 ✓）⇒ 顺手算一次，
/// 否则用户要等到第一次直播任务跑完才看得见时刻 ✗。
///
/// # ⚠ 为什么**不**把"已经有这条、但还没算出时刻"也算成需要
///
/// 这一处只有 `seed_defaults_for` 一个调用点，而它的两个调用者都**很频繁**：
/// 服务启动（`main.rs` ✓）与**每一次登录**（`routes/auth.rs` ✓）。把
/// "已有、但 `enabled=false`"也算成需要的话：
///
/// * "想要、但这阵子没有下一场"的用户 —— **每登录一次**就被打一轮上游
///   （每账号 4 枪：SSO → JWT → 两个页签 ✗）；
/// * 而**全站都没有下一场**时（主播没排期），那就是**每次启动给所有用户各打一轮** ✗
///   —— 正是用户口径里要避免的那件事（「别在每次启动时给所有用户都打一轮上游」✓）。
/// * 要把它与"算过、但上游没给下一场"区分开，得在 `Schedule` 上加一格**持久标记**：
///   那一格是**全仓共享**的 `mcloud-core` 模型，动它牵动存储 / 路由 / 调度器好几处 ✗
///   —— 为这点收益不值得（代价见下 ✓）。
///
/// ⇒ 只认"新种出来"。剩下那条缝（种完、进程恰好在下一次重算前挂掉）由
/// **常规重算**兜住：每次 `live-room` 跑完都会重算并 upsert（`spawn_after_run` ✓），
/// 而用户界面上那条说明本来就是"下次算出来自动启用" ✓。
pub async fn seed_and_reschedule_auto(
    store: &Arc<dyn Store>,
    clock: &Arc<dyn Clock>,
    http_for: Option<&mcloud_engine::HttpClientFactory>,
    owner: &UserId,
) -> Result<()> {
    if !seed_auto_schedule(store, owner).await? {
        // 已经有这条（或这个 id 被别人占了）⇒ **什么都不做**：
        // 不覆盖、也**一个上游请求都不发** ✓（判据与理由见上面那段 ✓）。
        return Ok(());
    }
    spawn_seed_reschedule(store, clock, http_for, owner);
    Ok(())
}

/// 把"种完那一次重算"丢进后台（**不 await** ✓）。
///
/// # 为什么不能 await（`seed_and_reschedule_auto` 明明是 async 的）
///
/// 它的两个调用点都落在**用户要等**的路径上：服务启动（`main.rs` ✓）与
/// **登录响应**（`routes/auth.rs` 的登录处理 ✓）。await 的话，登录要多等最多
/// `账号数 × `[`UPSTREAM_FETCH_TIMEOUT`]（上游卡住 = **登录卡住** ✗），
/// 而这次重算**只是把那张卡填得好看一点**：算不出来那条定时照旧是停用的 ✓，
/// 对"能不能登录""服务起没起来"零影响 ⇒ 没有任何理由让它挡在前面 ✓。
///
/// # ⚠ 失败 / 超时只 warn：绝不 panic、绝不让启动或登录失败
///
/// 它跑在一个**独立任务**里，`reschedule_live_auto` 的 `Err`（存储读不出来那一档 ✓）
/// 只记一条 warn ✓；"上游取不到"连 `Err` 都不是（[`reschedule_live_auto`] 内部
/// 走池里那份兜底 ✓）。连"没有 tokio 运行时"这种极端情形也**只是少做一次重算**，
/// 记一条 warn 就返回 ✓（**不** `block_on`、**不** panic ✗）。
///
/// # ⚠⚠ 这次重算走的**全是读**：**不会领小红花**（这句必须写清楚）
///
/// 重算只做三件读事：换 SSO → 换直播侧 JWT → 拉 `tabContentList`
/// （`mcloud_tasks::live_room::fetch_next_start`，函数头自述"**只读地**取下一场" ✓）。
/// **领小红花只在任务真的跑起来那一次发生**（`live-room` 任务被调度器触发 ✓，
/// 见 `mcloud_tasks::live_room` 的领取那一段 ✓）⇒ 这次重算**不会**替用户领任何东西、
/// 也不会在上游那边多出一次"领取"记录 ✓（用户与 reviewer 都会问这一句 ✓）。
///
/// ⚠ 也**不写**除那条定时之外的任何东西：落盘只有一次 `save_schedule` ✓。
fn spawn_seed_reschedule(
    store: &Arc<dyn Store>,
    clock: &Arc<dyn Clock>,
    http_for: Option<&mcloud_engine::HttpClientFactory>,
    owner: &UserId,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!(
            owner = %owner,
            "种下「直播自动定时」时没有 tokio 运行时 ⇒ 这次不做后台重算\
             （下次启动 / 直播任务跑完时会重算，条目本身已经落盘 ✓）"
        );
        return;
    };
    let (store, clock) = (Arc::clone(store), Arc::clone(clock));
    let http_for = http_for.cloned();
    let owner = owner.clone();
    handle.spawn(async move {
        match reschedule_live_auto(&store, &clock, http_for.as_ref(), &owner, None).await {
            Ok(state) => tracing::info!(
                owner = %owner,
                enabled = state.enabled,
                at = ?state.at,
                "种下「直播自动定时」后已重算一次"
            ),
            Err(e) => tracing::warn!(
                owner = %owner,
                error = %e,
                "种下「直播自动定时」后重算失败（条目留着；下次启动或直播任务跑完时会再算）"
            ),
        }
    });
}

/// **重算入口**（用户口径里的 `reschedule_live_auto(owner)` ✓）。
///
/// 做五件事，顺序要紧：
///
/// 1. 读那条自动定时（可能还不存在 ✓），取出**用户意愿**与**账号多选**；
/// 2. 把账号列表**展开成真实账号**（空 = 该用户全部**已启用**账号 ✓，见 [`target_accounts`]）；
/// 3. 对**每个账号**各去上游取一次「下一场」（[`fetch_nexts`] ✓，用那个账号自己的票 ✓）；
/// 4. 取**最早**那一场 + 1 小时（[`earliest_plus_one_hour`] ✓）；上游一个都没给出
///    ⇒ 用**池里那份兜底**（见模块头的兜底链 ✓）；
/// 5. upsert：`expr` 每次覆盖 ✓，`accounts` / `managed_wanted` / 记账字段原样保留 ✓。
///
/// # ⚠ `http_for` 为什么是 `Option`
///
/// 它是"按账号造 HTTP 客户端"的那份工厂（生产 = `AppBuilder` 里那份 ✓）。
/// `None` = **没接上**（只有测试与"没接线的组装"会这样 ✓）⇒ 不去上游，直接走池里那份
/// 兜底 —— **不是** panic ✗，也**不是**"算不出下一场"✗（那会把"没接线"表现成
/// "今天没直播"，一个假结论 ✗）。线上由 `AppBuilder` 与 `Notifier` 各接一次 ✓。
///
/// ⚠ **一次运行只该调一次**（接在 `runner.rs` 的 `RunEvent::Finished` 那一跳 ✓，
/// 见 `notify.rs` 的 `NotifySink`）。写在任务的某个步骤里的话，
/// 一趟 N 个账号就是 N 次读改写，而且中途失败会让它停在一个中间态 ✗。
///
/// ⚠ `id` 撞车保护：`auto-live-<owner>` 这条 id 用户**也能**手建
///（id 是用户可写的 ✓）。那条定时的 `managed_by` 是 `None` ⇒ **绝不碰** ✗，
/// 只回一句可操作的说明（`enabled: false` ✓，见 `squat_note` ✓）——
/// 见 [`Schedule::managed_by`] 的注释 ✓。
pub async fn reschedule_live_auto(
    store: &Arc<dyn Store>,
    clock: &Arc<dyn Clock>,
    http_for: Option<&mcloud_engine::HttpClientFactory>,
    owner: &UserId,
    patch: Option<AutoLivePatch>,
) -> Result<AutoLiveState> {
    let id = live_auto_schedule_id(owner);
    let existing = store.schedule_for(owner, &id).await?;

    // ── 撞车保护（必须在任何写入之前 ✓）──────────────────────────────
    if let Some(s) = &existing {
        if !s.is_managed_by(MANAGED_BY_LIVE_AUTO) {
            tracing::warn!(
                schedule = %id,
                owner = %owner,
                "auto-live-<owner> 这个 id 上是用户手建的定时 ⇒ 不碰它（自动定时这一轮跳过）"
            );
            // ⚠ 报 `enabled: false`（而不是那条手建定时的 `enabled`）：
            // 这张卡说的是**自动直播定时**这个功能，而它此刻**装不上** ✓ ——
            // 报一个"true"会让界面显示"已开启"而实际上什么都没发生 ✗。
            return Ok(AutoLiveState {
                enabled: false,
                at: None,
                accounts: Vec::new(),
                note: Some(squat_note(&id)),
            });
        }
    }

    // ── 用户意愿与账号多选：**每次都保留**（用户口径：「保留用户对账号多选的修改」✓）
    //
    // ⚠ 这两个值只在"还没有那条定时"时才取默认 ✓：
    //   * 意愿默认 `true` ✓ —— **用户口径 2026-10-08：「小红花定时默认开启」** ✓：
    //     `None` = 他从没表过态 ⇒ **按"开"算** ✓（下面那句 `.unwrap_or(true)` 就是它 ✓）；
    //   * 账号默认空 —— `Schedule::accounts` 里空 = 全部已启用账号 ✓，
    //     正是"所有指定的一个多个云盘账号"在没指定时的自然读法 ✓。
    //
    // ⚠⚠ **这一处是"用户意愿"唯一的判据**（"`None` ⇒ 开"只写在这里 ✓）：
    // `read_auto_state` 里那句 `unwrap_or(true)` 只是**把同一格读出来说给人听**
    // （给 `note_for` 用 ✓），它跟着这里走、不是第二个判据 ✓ —— 真要改口径，
    // 改这一行就够了 ✓（两处各写一遍的话，迟早各说各话 ✗）。
    //
    // ⚠ **`Some(false)` 原样保留、绝不覆盖** ✓：`Some(false)` 是用户**明确关掉过**
    //（界面上那个开关 ✓），`None` 是"从没表过态" —— 两者不是一回事 ✗。
    // 把 `None` 也当开**不会**把"他关过的"再打开：`Some(false)` 走的是 `and_then`
    // 的 `Some` 那一支、**根本不经过** `unwrap_or` ✓。
    //（"时刻/能不能跑"与"意愿"拆成两格，理由见
    //  `mcloud_core::Schedule::managed_wanted` 的注释 ✓。）
    let mut wanted = existing
        .as_ref()
        .and_then(|s| s.managed_wanted)
        .unwrap_or(true);
    let mut accounts = existing
        .as_ref()
        .map(|s| s.accounts.clone())
        .unwrap_or_default();
    // 界面上的改动（只有 PUT 会带 ✓）先落进来，再算时刻 ——
    // 顺序反了的话"改完账号列表立刻重算"会用旧列表算一次 ✗。
    if let Some(p) = &patch {
        if let Some(w) = p.wanted {
            wanted = w;
        }
        if let Some(a) = &p.accounts {
            accounts = a.clone();
        }
    }

    let now_local = beijing_now(clock.as_ref());

    // ── 逐账号向上游现取 → 取最早 → +1 小时 ───────────────────────────
    //
    // ⚠⚠ 先**展开**成真实账号列表（见 [`target_accounts`]）：`accounts` 为空
    // 表示"全部已启用账号"，直接拿空列表去循环 ⇒ 一个都不取 ⇒ 永远算不出 ✗。
    let targets = target_accounts(store, owner, &accounts).await?;
    let nexts = fetch_nexts(http_for, &targets, &now_local).await;
    let upstream_at = earliest_plus_one_hour(nexts.iter().map(String::as_str));

    // ── 兜底：上游一个都没给出时刻 ⇒ 用池里那份 ───────────────────────
    //
    // ⚠ 只在**上游整条路都没给出时刻**时兜底（不是"某个账号失败就兜"）：
    // 只要有一个账号给出了时刻，上游这条路就是通的 ⇒ 那时再混进池里那份
    //（"上次探测"的口径 ✓）只会让"取最早"在两个不同口径之间比 ✗。
    let at = match upstream_at {
        Some(t) => Some(t),
        None => {
            let pool = store.kouling_pool().await?;
            // ⚠ 用 `next_start_at`（**跳过已经过去的场次** ✓）而不是 `next_start`：
            // 池里那份是"上次探测的结论"，它挂着开过的场次时，不筛就会算出一个
            // **过去**的时刻 ✗（用户实测 2026-10-02：16:31 时界面显示 15:00 ✓）。
            let cached = pool.next_start_at(&now_local).map(str::to_string);
            earliest_plus_one_hour(cached.as_deref().into_iter())
        }
    };

    // ── 写回 ─────────────────────────────────────────────────────────
    let mut s = existing.unwrap_or_else(|| Schedule {
        id: id.clone(),
        name: AUTO_LIVE_NAME.to_string(),
        task: LIVE_ROOM_TASK.to_string(),
        accounts: accounts.clone(),
        // 占位：下面一定被 `at` 那一支覆盖（没有下一场时它是**停用**的，
        // 而停用的定时调度器根本不看 `expr` ✓）—— 写个能解析的合法值，
        // 免得"这条定时为什么解析不了"变成一个假故障 ✗。
        // ⚠ 与"种下那条定时"写的是**同一个值** ⇒ 共用 [`SEED_PLACEHOLDER_EXPR`] ✓。
        expr: SEED_PLACEHOLDER_EXPR.to_string(),
        timezone: "Asia/Shanghai".to_string(),
        body: serde_json::json!({}),
        enabled: false,
        managed_by: Some(MANAGED_BY_LIVE_AUTO.to_string()),
        // 首次创建就把意愿**落成明确的一格** ✓ —— `None`（还没表态 ✓）在上面已经
        // 按"开"算过 ⇒ 这里落盘就是 `Some(true)` ✓，不靠"读的时候缺省"推断 ✓
        //（用户口径 2026-10-08「小红花定时默认开启」✓；他明确关过的那种是
        //  `Some(false)`，走不到这个分支 —— 它连定时都已经存在了 ✓）。
        managed_wanted: Some(wanted),
        owner: Some(owner.clone()),
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    });

    // ⚠ 下面这几格**每次都由系统写死**（用户改了也会被改回来 ✓，见模块头）：
    // 标记（免得被谁清掉之后系统就不认它了 ✓）、意愿、账号列表、任务名、时区。
    s.managed_by = Some(MANAGED_BY_LIVE_AUTO.to_string());
    s.managed_wanted = Some(wanted);
    s.accounts = accounts.clone();
    s.task = LIVE_ROOM_TASK.to_string();
    // 时区**必须**跟着 `one_shot_expr` 走：那个表达式里的"11 时"是**东八区墙钟** ✓，
    // 挂到别的时区上就会在错的钟点跑（而 `parse_timezone` 只认固定偏移 ✓）。
    s.timezone = "Asia/Shanghai".to_string();
    // ⚠ `name` **不覆盖**：用户改过名字就留着他的 ✓（它不是判据，只是显示 ✓）。
    // `body` 同理不动：那是这条定时跑 `live-room` 的参数（口令 / 听多久 ✓），
    // 用户可能自己填过 codes ✓ —— 系统没资格替他清掉 ✗。

    match &at {
        Some(local) => {
            // ⚠ 这一行就是"用户手改时刻会被覆盖"的落点（`expr` 每次重写 ✓）。
            // cron 没有年字段 ⇒ 这条表达式是"每年今天"，兜底是"每次直播任务跑完
            // 就重算"这条链本身 —— 代价见 `mcloud_core::live_auto::one_shot_expr` ✓。
            s.expr = one_shot_expr(local).ok_or_else(|| {
                mcloud_core::Error::Config(format!("算出来的时刻无法写成 cron：{local}"))
            })?;
            // 能不能跑 = **用户想要 且 算得出下一场** ✓（两格各记一件事，见模型注释 ✓）。
            s.enabled = wanted;
        }
        None => {
            // **停用但保留**（用户口径 ✓）：`expr` 留着不动（下次算出来覆盖 ✓），
            // 用户挑的账号与他的意愿也一并留着 ✓。
            s.enabled = false;
        }
    }

    store.save_schedule(&s).await?;

    let note = note_for(s.enabled, wanted, at.is_some());
    Ok(AutoLiveState {
        enabled: s.enabled,
        at,
        accounts,
        note,
    })
}

/// 这次重算要**逐个去上游问**的那批账号。
///
/// # `accounts` 为空 = 该用户**全部已启用**账号（用户口径 ✓）
///
/// `Schedule::accounts` 的既有语义就是这条（`mcloud-engine` 的
/// `Scheduler::target_accounts` 同一读法 ✓）。⚠ 必须**展开成真实账号**再逐个取：
/// 直接拿空列表去循环 ⇒ 一个都不取 ⇒ 永远算不出下一场 ✗。
///
/// # 两条细节
///
/// * **只展开"已启用"的**：账号上的 `enabled` 是用户自己关掉的（账号列表页默认
///   就不显示它们 ✓）—— 拿一个他关掉的账号去打上游是替他做决定 ✗；
/// * **用户手挑的那些不按 `enabled` 再筛一遍**：他点名要的就是他要的
///   （路由那层已经验过"必须是自己的"✓）；真挑了一个停用的，那也照他说的取 ✓。
///   查不到的（账号被删了 / 不是他的 ✓）**跳过并留痕** —— 猜一个出来更坏 ✗。
async fn target_accounts(
    store: &Arc<dyn Store>,
    owner: &UserId,
    picked: &[String],
) -> Result<Vec<Account>> {
    if picked.is_empty() {
        let mut all: Vec<Account> = store
            .accounts_for(owner)
            .await?
            .into_iter()
            .filter(|a| a.enabled)
            .collect();
        // 顺序稳定（按 id 排）：并发取的时候谁先回来是随机的，
        // 但"这次问了哪几个账号"在日志/测试里要能复现 ✓。
        all.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        return Ok(all);
    }
    let mut out = Vec::with_capacity(picked.len());
    for id in picked {
        match store
            .account_for(owner, &AccountId::new(id.clone()))
            .await?
        {
            Some(a) => out.push(a),
            None => tracing::warn!(
                owner = %owner,
                account = %id,
                "自动直播定时里点名的账号不在库里（或不属于这个用户）⇒ 这次不取它的「下一场」"
            ),
        }
    }
    Ok(out)
}

/// 对**每个账号各取一次**「下一场」（用户口径 2026-10-08：「下一场应该从上游获取啊」✓）。
///
/// # 为什么并发 + 每账号一个总超时
///
/// 这条重算挂在**运行收尾的后台任务**上（`spawn_after_run` ✓），而 CLI 的 `flush()`
/// 会等它（同一个在飞计数 ✓）⇒ 不能让上游把它拖住。串行的话最坏是
/// `账号数 × 超时`（一次运行最多 N 个账号 ✓），并发之后最坏只有**一个**超时 ✓。
///
/// # ⚠ 任何一个账号失败/超时/没有下一场都**不算这次重算失败**
///
/// 它只是"这个账号这次没给出时刻" ⇒ 记一条 warn/debug 就够了 ✓。
/// 把整次重算判失败的话，"上游抖一下"会变成"定时没更新"✗ ——
/// 而那正是用户最不容易发现的一种退化 ✓。
///
/// ⚠ 任务**panic** 也在这里被收成一条 warn（`JoinError` ✓）——
/// 收尾那一跳上任何 panic 都绝不能冒泡 ✗（见 [`after_run`] 的说明 ✓）。
///
/// ⚠ `http_for` 为 `None`（没接线）⇒ **一个请求都不发**、回空表 ⇒ 调用方走池里那份 ✓。
async fn fetch_nexts(
    http_for: Option<&mcloud_engine::HttpClientFactory>,
    targets: &[Account],
    now_local: &str,
) -> Vec<String> {
    let Some(factory) = http_for else {
        tracing::debug!("没有按账号的 HTTP 客户端工厂 ⇒ 这次不去上游现取（走池里那份兜底）");
        return Vec::new();
    };
    if targets.is_empty() {
        return Vec::new();
    }

    let mut set = tokio::task::JoinSet::new();
    for account in targets {
        // 用**这个账号自己的**票（工厂按账号注入 `Authorization` ✓）——
        // 凭据坏了 / 未登录时，只有那个账号取不到 ✓。
        let http = factory(account);
        let id = account.id.to_string();
        let account = account.clone();
        let now_local = now_local.to_string();
        set.spawn(async move {
            let r = tokio::time::timeout(
                UPSTREAM_FETCH_TIMEOUT,
                mcloud_tasks::live_room::fetch_next_start(&*http, &account, &now_local),
            )
            .await;
            (id, r)
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((id, Ok(Ok(Some(start))))) => {
                tracing::debug!(account = %id, next_start = %start, "已从上游取到「下一场」");
                out.push(start);
            }
            // "上游这次没给这一格"（没开播 / 还没放预告 / 场次都过去了 ✓）——
            // 这不是错误，是**正常**的一种结局 ✓（池那边同一档也是 `None` ✓）。
            Ok((id, Ok(Ok(None)))) => {
                tracing::debug!(account = %id, "上游这次没给出这个账号的「下一场」");
            }
            Ok((id, Ok(Err(e)))) => {
                tracing::warn!(
                    account = %id,
                    error = %e,
                    "去上游取「下一场」失败（这次用池里那份兜底）"
                );
            }
            Ok((id, Err(_))) => {
                tracing::warn!(
                    account = %id,
                    timeout_secs = UPSTREAM_FETCH_TIMEOUT.as_secs(),
                    "去上游取「下一场」超时（这次用池里那份兜底）"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "取「下一场」的任务没有正常结束（这次用池里那份兜底）");
            }
        }
    }
    out
}

/// **只读**地取当前状态（`GET` 用 ✓）。
///
/// # 为什么 GET 不顺手重算一次
///
/// 重算会**写盘**（改 `expr` / `enabled`）。`GET /api/kouling` 有同款副作用、
/// 而且它那样做是对的（周期翻篇"读的时候才发现"✓，不写回两边就不一致 ✗）——
/// 但这条不一样：它的输入（上游那份日程 / 池里那份 ✓）在**每次直播任务跑完**时
/// 都会变，而这张卡不是"每次刷新都要最新"的东西 ✓。
/// 更要紧的是：GET 顺手改一条**用户自己的定时**，会让"我没点保存，它怎么变了"
/// 变成一件查不清的事 ✗。⇒ 想看最新的就 `PUT` 一次（或等直播任务跑完 ✓）。
///
/// ⚠ 时刻从那条定时的 `expr` **反推**（`mcloud_engine::next_fire_at` ✓）而不是
/// 另存一份：两份就一定会分叉 ✗。代价写在下面那行注释里 ✓。
pub async fn read_auto_state(
    store: &Arc<dyn Store>,
    clock: &Arc<dyn Clock>,
    owner: &UserId,
) -> Result<AutoLiveState> {
    let id: ScheduleId = live_auto_schedule_id(owner);
    let Some(s) = store.schedule_for(owner, &id).await? else {
        return Ok(AutoLiveState {
            enabled: false,
            at: None,
            accounts: Vec::new(),
            note: Some("还没有这条自动定时（跑过一次直播任务之后就会出现）".into()),
        });
    };
    if !s.is_managed_by(MANAGED_BY_LIVE_AUTO) {
        // id 撞车：这条是用户手建的 ⇒ 如实说明，别把它当成"我们的那条"✗
        return Ok(AutoLiveState {
            enabled: false,
            at: None,
            accounts: Vec::new(),
            note: Some(squat_note(&id)),
        });
    }
    // ⚠ 反推的时刻是"这条定时**下一次会触发**的时刻"✓ —— 与"下一场 + 1 小时"
    // 在**正常情况下是同一个**（重算刚写过它 ✓）。两者唯一的差别是
    // "刚跑完、还没重算"那一小段：那时 cron 里那一刻已经过去，
    // `next_fire_at` 会回**明年**同一刻 ✗（cron 没有年字段 ✓）。
    // 窗口只有"收尾那一跳"那么长，而收尾里就会重算 ⇒ 实测看不到 ✓；
    // 真看到了也不要"顺手改成显示空"—— 那会把正常状态也说成"算不出来" ✗。
    let at = if s.enabled {
        mcloud_engine::next_fire_at(&s, clock.now_ms())
            .map(mcloud_core::live_auto::beijing_stamp)
            .filter(|t| !t.is_empty())
    } else {
        None
    };
    let note = note_for(s.enabled, s.managed_wanted.unwrap_or(true), at.is_some());
    Ok(AutoLiveState {
        enabled: s.enabled,
        at,
        accounts: s.accounts.clone(),
        note,
    })
}

/// id 撞车时那句说明（`reschedule_live_auto` 与 `read_auto_state` **共用一份** ✓）——
/// 两边各写一句的话，同一件事会有两种说法，而用户看到的是哪一句取决于他先点了什么 ✗。
///
/// ⚠ 必须**可操作**：只说"系统不会动它"用户就卡住了（他既开不了、也删不掉那条 ✓），
/// 所以要给出下一步（改名或删掉那条 ✓）。
fn squat_note(id: &ScheduleId) -> String {
    format!(
        "id {id} 上已经有一条你自己建的定时 ⇒ 系统不会动它（自动直播定时这一轮跳过）。\
         想用这个功能，请把那条定时改名或删掉。"
    )
}

/// 把"现在是什么状态"说成一句人话（`None` = 一切正常，不必解释 ✓）。
///
/// # ⚠ 为什么**不给"这次用的是兜底"单独一句**
///
/// 兜底（上游取不到 ⇒ 用池里那份 ✓）确实是一个**降级**状态，但它没法在
/// [`read_auto_state`]（`GET`）那边还原：GET 是从 cron **反推**时刻的 ✓，
/// 它不知道那一刻是现取的还是兜来的 ⇒ 同一份状态在 `PUT` 与 `GET` 上会显示
/// 两句不同的话 ✗（用户刷新一下卡片就变了 —— 比不说更糟 ✓）。
/// 这条降级由 `fetch_nexts` 的 warn 日志留痕 ✓，卡片上不写 ✓。
fn note_for(enabled: bool, wanted: bool, has_at: bool) -> Option<String> {
    if !wanted {
        return Some("已按你的设置关闭（定时条目留着，随时可以再打开）".into());
    }
    if !has_at {
        // ⚠ 两种"没有"都落在这里（**说清是哪种情况** ✓）：
        // ① 上游这次没取到（网络错 / 未登录 / 上游没给这一格 ✓）；
        // ② 池里那份兜底也没有（从没探测过 / 场次都过去了 ✓）。
        // 用户看到的下一句是"下次算出来自动启用"，所以他不用做任何事 ✓。
        return Some(
            "上游这次没取到下一场（未登录 / 网络错 / 还没放预告），池里那份兜底也没有\
             ⇒ 已停用，条目留着，下次算出来自动启用"
                .into(),
        );
    }
    if !enabled {
        // 理论到不了（有 at 且想要 ⇒ enabled=true ✓），但别让状态与说明互相矛盾 ✗
        return Some("算出了时刻，但这条定时当前是停用状态".into());
    }
    None
}

/// **运行收尾那一跳的接线**（`NotifySink` 的 `Finished` 分支调它 ✓）。
///
/// 只做三件事：不是直播任务就**立刻返回**（守卫随手 drop ⇒ 计数减一 ✓）、
/// 没有存储就返回、否则把 [`after_run`] 丢进后台任务 ✓。
///
/// # 为什么 `emit` 里不能直接 `.await` 它
///
/// `emit` 是**同步**的，而且它在 runner 的收尾路径上 ✓ —— 在那里等一次
/// "读池 + 写 schedules.json（整文件重写 + fsync ✓）"就是把这段 IO 算进运行收尾 ✗。
/// 与推送那条路同一个理由 ✓（见 `notify.rs` 里那句"必须 spawn"）。
/// ⚠ 现在还要多一层理由：重算会**去上游现取**（最多 4 枪 × 每个账号 ✓）——
/// 那更不可能在同步的 `emit` 里等 ✓。
///
/// # 为什么连 `handle` 与 `guard` 都从调用方传进来
///
/// `handle`：那边刚刚 `try_current()` 过，再取一次是白写 ✓。
/// `guard`：**计数必须在 spawn 之前加上**（否则 CLI 的 `flush()` 可能在计数还没
/// 加上时就看到 0 而直接返回 ✗，见 `begin_delivery` ✓）⇒ 由调用方先 `begin_delivery`
/// 再把守卫交过来 ✓。不是直播任务时它在这里被 drop ⇒ 计数立刻归零 ✓（不会漏减）。
pub(crate) fn spawn_after_run(
    notifier: &Arc<Notifier>,
    task: String,
    account: String,
    handle: tokio::runtime::Handle,
    guard: DeliveryGuard,
) {
    if task != LIVE_ROOM_TASK {
        return;
    }
    // 没接存储（只有测试会这样 ✓）⇒ 零行为，绝不 panic ✓（与推送同一条口径 ✓）。
    let Some(store) = notifier.store().cloned() else {
        return;
    };
    let clock = Arc::clone(notifier.clock());
    // ⚠ 按账号造客户端的工厂（重算要拿**那个账号的票**去上游现取 ✓）。
    // 没接线（`None`，只有测试会这样 ✓）⇒ 照旧走池里那份兜底，不 panic ✗（见
    // [`reschedule_live_auto`] 里那段说明 ✓）。
    let http_for = notifier.http_for().cloned();
    handle.spawn(async move {
        let _guard = guard;
        after_run(store, clock, http_for, &task, &account).await;
    });
}

/// **运行收尾那一跳**（`NotifySink` 的 `Finished` 分支调它 ✓）。
///
/// # 为什么接在收尾，而不是任务里的某一步
///
/// 用户口径：「**每次直播任务跑完就重算并 upsert**」✓。收尾那一跳是
/// `runner.rs` 里**唯一**的运行终点（`sink.emit(RunEvent::Finished)` ✓，
/// 而且它在**记录落库之后**发 ✓，见那边的哨兵用例 ✓）。
/// 写在任务步骤里的话：一趟 N 个账号就是 N 次读改写 ✗，而且中途失败会把这条定时
/// 停在一个中间态 ✗ —— 用户要的是"这次跑完，下次该什么时候跑"这一个结论 ✓。
///
/// # 为什么按**账号的归属**认用户
///
/// `Finished` 只带 `task` 与 `account`（`RunMeta` ✓），不带"谁发起的"之外的
/// 身份信息。而云盘账号身上有 `owner`（`Account::owner` ✓，启动时回填过 ✓）
/// ⇒ 这次跑的是谁的账号，就重算谁的那条 ✓。
/// 定时发起的运行也走这条：那条定时本来就属于同一个用户 ✓。
///
/// # 为什么**不是** live-room 就直接返回
///
/// 别的任务跑完与"下一场直播"毫无关系，重算只是白写一次盘 ✗。
///
/// # 失败只记日志（不 panic、不冒泡）
///
/// 它是在**运行收尾那一跳**上被 `spawn` 出来的（`spawn_after_run` ✓，而 `emit`
/// 是同步的、就在 runner 的收尾路径上 ✓）⇒ 这里任何 panic 或错误都绝不能
/// 回到那条运行路径上 ✗。重算失败最多让那条定时晚一轮更新，
/// 代价远小于"把一次成功的运行判成失败"✗。
///
/// ⚠ **"上游取不到"不算重算失败** ✗：它是 [`reschedule_live_auto`] 内部的一条
/// 正常分支（走池里那份兜底 ✓），根本不会以 `Err` 冒到这里 ✓。
/// 冒到这里的 `Err` 是"存储读不出来"那一档 ✓。
pub(crate) async fn after_run(
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    http_for: Option<mcloud_engine::HttpClientFactory>,
    task: &str,
    account: &str,
) {
    if task != LIVE_ROOM_TASK {
        return;
    }
    let owner = match store
        .account(&mcloud_core::AccountId::new(account.to_string()))
        .await
    {
        Ok(Some(a)) => a.owner,
        // 账号查不到（被删了 ✓）或读失败 ⇒ 认不出人 ⇒ 什么都不做并留痕 ✓
        Ok(None) => {
            tracing::debug!(account, "直播任务跑完，但账号已不在库里 ⇒ 跳过自动定时重算");
            return;
        }
        Err(e) => {
            tracing::warn!(account, error = %e, "直播任务跑完，读账号归属失败 ⇒ 跳过自动定时重算");
            return;
        }
    };
    let Some(owner) = owner else {
        // `owner` 为空是"老数据还没回填"那一档（启动时会回填 ✓）——
        // 认不出人就不猜（猜错会去改**别人**的定时 ✗）。
        tracing::debug!(account, "直播任务跑完，但账号没有归属 ⇒ 跳过自动定时重算");
        return;
    };
    match reschedule_live_auto(&store, &clock, http_for.as_ref(), &owner, None).await {
        Ok(state) => tracing::info!(
            owner = %owner,
            enabled = state.enabled,
            at = ?state.at,
            "直播任务跑完，已重算「直播自动定时」"
        ),
        Err(e) => tracing::warn!(
            owner = %owner,
            error = %e,
            "直播任务跑完，重算「直播自动定时」失败（下次收尾会再试）"
        ),
    }
}

#[cfg(test)]
mod tests;
