//! 「凭据自动刷新定时」的**组装层**：把预设钟点写成 cron → upsert 那条定时。
//!
//! # 这件事的形状（用户口径 2026-10-11）
//!
//! ```text
//! 每个**登录用户**一条「凭据自动刷新定时」
//!   id 形如 auto-auth-refresh-<owner>   ⇒ 确定性 ⇒ upsert 幂等 ✓
//!   带一个"系统维护"标记（managed_by）   ⇒ 系统只动自己这条，绝不碰用户手建的 ✗
//!   跑什么    `auth-refresh`（**不是**注册表里的任务 ✓，见 core 那个常量的说明 ✓）
//!   作用于    该定时上配置的**一个或多个云盘账号**（复用 Schedule::accounts ✓）
//!   触发时刻  **每天**那个钟点（**每用户一份** ✓，见下）
//!   怎么更新  改预设时间 / 翻开关 ⇒ 立刻 upsert 一次 ✓（[`reschedule_auth_refresh`] ✓）
//!   到点之后  **先走既有判据**（`refresh_token_days` ✓）——
//!             不需要刷就跳过，并把"跳过的原因"记进日志与那条定时的 `last_status` ✓
//!   它从哪来  **开箱就有**：每个用户在自己的默认数据那一步被种上一条
//!             （[`seed_auth_refresh_schedule`] ✓，意愿=开、`enabled=true`、时刻已算好 ✓）
//! ```
//!
//! # ⚠⚠ 「预设刷新时间」是**钟点**，而且是**每用户一个**（两轮用户口径，别搞混）
//!
//! * 「**明天检查是否到达预设刷新时间**，到了就刷」（2026-10-11 第一轮 ✓）⇒
//!   这里的"时间"读作**墙钟钟点**（例如「每天 03:00」✓），**不是**"剩余天数"——
//!   依据是"到没到"是**一个时刻**的事，而 `refresh_token_days` 那套是"剩余不足 N 天"
//!   的**阈值**（没有"到没到"可言 ✓，见 `mcloud_core::auth_refresh` 模块头 ✓）；
//! * 「**要每用户一个钟点**」（2026-10-11 第二轮 ✓）⇒ 那个钟点**按 owner 存**
//!   （`data/auth-refresh.json` ✓），于是两个用户各按各自的钟点响 ✓。
//!   升级前它是实例级一格（`AppSettings::auth_refresh_at` ✓，**那个键留着** ✓）——
//!   谁没自己设过就用它 ⇒ 老实例里用户改过的钟点**不会凭空回到 03:00** ✓。
//!   判据只有一处：[`mcloud_core::auth_refresh::effective_at`] ✓，本模块只调它 ✓。
//!
//! # ⚠⚠ 「预设刷新时间」是**钟点**（用户口径的读法，见 `mcloud_core::auth_refresh` 模块头 ✓）
//!
//! 到点**不等于**"每天都真去刷上游" ✗ —— 到点之后走的是**既有判据**
//!（`mcloud_app::auth_refresh::AuthRefreshService::refresh` ✓），
//! 于是三种结局都会出现，而且**逐条记原因** ✓：
//!
//! | 结局 | 含义 | 上游 |
//! |---|---|---|
//! | `Rotated` | 换了新凭据 | 打了一枪 ✓ |
//! | `Skipped` | 还没到提前量（还剩 N 天） | **一枪都不打** ✓ |
//! | `ManualOnly` | 库里没有有效的过期时刻 ⇒ 要重新登录 | **一枪都不打** ✓ |
//!
//! ⇒ 一个"还剩 300 天"的账号每天都会被检查一次、但**永远不打上游** ✓
//!（这正是"到点必刷"与用户口径的差别：用户要的是"到点**检查**，该刷就刷" ✓）。
//!
//! # ⚠ 判据与上游调用**一个字都不改**（那是既有正确逻辑 ✓）
//!
//! 本模块只做三件事：算 cron、upsert 那条定时、把调度器的钩子接到
//! [`AuthRefreshService`] 上 ✓。**不重写** `decide` / `lead_days_for` / 窄写落库 ✗。
//!
//! # ⚠ 两条入口别搞混：**种**（seed）与**重算**（reschedule）
//!
//! * [`seed_auth_refresh_schedule`]：**幂等**种下那条定时
//!   （`AppHandle::seed_defaults_for` 调它 ✓ —— 启动与登录都经那里 ✓）。
//!   已经有这条就**一个字节都不动** ✓（尤其**不覆盖用户关掉的意愿** ✗）；
//! * [`reschedule_auth_refresh`]：把当前设置里的预设钟点**写成 cron** 并 upsert ✓
//!   （界面上改了时间 / 翻了开关立刻调一次 ✓）。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use mcloud_core::auth_refresh::{
    auth_refresh_schedule_id, daily_expr, effective_at, DEFAULT_AUTH_REFRESH_AT,
    MANAGED_BY_AUTH_REFRESH,
};
use mcloud_core::{Account, Error, Result, Schedule, ScheduleId, Store, UserId};

use crate::auth_refresh::{AuthRefreshService, RefreshOutcome};

/// 那条自动定时的显示名（只在**新建**时用；用户改过名字就留着他的 ✓）。
pub const AUTO_AUTH_REFRESH_NAME: &str = "凭据自动刷新";

/// 用户在界面上改的那两件事（`PUT` 专用 ✓）。
///
/// ⚠ 与 `Schedule` 的编辑分开：这里**只能**改"开关"与"预设钟点" ✓ ——
/// 时刻是**算出来的**（`expr` 由 [`reschedule_auth_refresh`] 写 ✓），
/// 不在这个结构里 ✓。
///
/// ⚠ 这里**没有"改谁"这一格** ✗：钟点是**每用户一份**的，而"我是谁"由调用方
/// （HTTP 的当前登录用户 / CLI 解析出来的那个用户 ✓）定 ⇒ 这个结构只描述
/// "把**他的**那两格改成什么" ✓。想改别人的钟点，在这条路上**连指都指不到** ✓。
#[derive(Debug, Clone, Default)]
pub struct AuthRefreshPatch {
    /// 用户意愿（`None` = 不改 ✓）。
    pub wanted: Option<bool>,
    /// 预设刷新时间（`HH:MM`，`None` = 不改 ✓）。
    ///
    /// ⚠ 给了就**只写这个用户自己那一格**（`data/auth-refresh.json` ✓）——
    /// 不动 `AppSettings::auth_refresh_at`（实例级默认值 ✓）、更不动别人的 ✓。
    pub at: Option<String>,
}

/// 给前端那张卡的状态快照（`GET/PUT /api/auth/refresh-schedule` 的响应体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRefreshState {
    /// **现在能不能跑**（= 用户想要 ✓）。
    ///
    /// ⚠ 它与那条定时自己的 `enabled` **同一格**（不是"另一份状态"）——
    /// 卡片上那个开关显示的就是调度器会看到的那一格 ✓。
    pub enabled: bool,
    /// **这个用户**的预设刷新时间（东八区墙钟 `HH:MM` ✓）—— 界面上那个输入框的当前值 ✓。
    ///
    /// ⚠ 它是**他设过的那个值**；他还没自己设过时，这里显示的是**实例级默认值**
    ///（`AppSettings::auth_refresh_at` ✓）—— 也就是"他改之前会用的那个钟点" ✓
    ///（见 [`user_at`] ✓）。两种情况在界面上长得一样，因为**行为**本来就一样 ✓。
    pub at: String,
    /// **下一次**触发时刻（东八区墙钟串，形如 `2026-10-12 03:00:00` ✓）。
    ///
    /// `None` = 现在算不出来（预设时间不合法 ✓ / 这条定时是用户手建的（id 撞车）✓）。
    /// ⚠ 不给"看起来像时间"的占位值 —— 那是骗人 ✓。
    pub next_at: Option<String>,
    /// 一句人话，说清"为什么现在是这个状态"（卡片直接显示 ✓）。
    ///
    /// ⚠ 只在**说不清**的时候才给：正常时是 `None` ✓。
    pub note: Option<String>,
}

/// 那条自动定时**种下去时的形态**（"开箱就有"的那一格 ✓）。
///
/// # 四格值（用户口径 2026-10-11 定稿 ✓）
///
/// | 格 | 值 | 为什么 |
/// |---|---|---|
/// | `managed_by` | `Some(MANAGED_BY_AUTH_REFRESH)` | 系统只动自己这条，绝不碰用户手建的 ✓ |
/// | `managed_wanted` | `Some(true)` | **用户意愿 = 要** —— 口径就是「自动设置定时」✓ |
/// | `enabled` | **`true`** | 预设钟点**当下就算得出来** ⇒ 直接开 ✓ |
/// | `expr` | 预设钟点写成的 cron | `0 0 3 * * *` 那种（每天那一刻 ✓）|
///
/// ⚠ `enabled=true` 与「直播自动定时」种下去时 `enabled=false` **不一样**，这不是笔误：
/// 那一条的时刻依赖**上游**（算不出来就不能开 ✓），而这一条的时刻**只依赖一个设置值**
/// ⇒ 种下去的当刻就是可跑的 ✓。用户口径要的正是"**自动设置定时**"（开箱就有一条能跑的）✓。
fn seeded_auth_refresh_schedule(owner: &UserId, at: &str) -> Option<Schedule> {
    let expr = daily_expr(at)?;
    Some(Schedule {
        id: auth_refresh_schedule_id(owner),
        name: AUTO_AUTH_REFRESH_NAME.to_string(),
        task: mcloud_core::AUTH_REFRESH_TASK.to_string(),
        // 空 = **该用户全部已启用账号**（`Schedule::accounts` 的既有语义 ✓）。
        accounts: Vec::new(),
        expr,
        // ⚠ 必须与那个"东八区墙钟"配套（cron 的字段按这个时区解释 ✓）——
        // 写成 UTC 的话 `0 0 3 * * *` 会变成北京 11:00 ✗。
        timezone: "Asia/Shanghai".to_string(),
        body: serde_json::json!({}),
        enabled: true,
        managed_by: Some(MANAGED_BY_AUTH_REFRESH.to_string()),
        managed_wanted: Some(true),
        owner: Some(owner.clone()),
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    })
}

/// **幂等**种下那条自动定时；返回"这次是不是**新种出来的**" ✓。
///
/// # 已经有就**一个字节都不动**（尤其别把用户的意愿冲掉 ✗）
///
/// 判据按 **id 全局查**（`Store::schedule`，不是 `schedule_for`）：
/// `ScheduleId` 是**全局唯一**的，而 id 是**用户也能写**的东西 ——
/// 别人完全可以手建一条恰好叫 `auto-auth-refresh-<我的id>` 的定时 ✗。
/// 按 `(owner, id)` 查会查不到它 ⇒ 接着 `save_schedule` 就把它**覆盖**掉了 ✗
///（与 `live_auto::seed_auto_schedule` 逐字同一条理由 ✓）。
///
/// ⚠ 于是"已经有"分两种，两种都**什么都不写** ✓：
/// * 系统维护的那条（`managed_by = Some(auth-refresh)` ✓）—— 尤其**不覆盖
///   `managed_wanted: Some(false)`**：那是用户**明确关过** ✓；
/// * 用户手建的（`managed_by = None` ✓）—— 那种撞车由 [`reschedule_auth_refresh`]
///   的撞车保护如实说明（`squat_note` ✓），种这一步同样不碰它 ✓。
pub async fn seed_auth_refresh_schedule(store: &Arc<dyn Store>, owner: &UserId) -> Result<bool> {
    let id = auth_refresh_schedule_id(owner);
    if store.schedule(&id).await?.is_some() {
        return Ok(false);
    }
    // 预设钟点取**这个用户现在生效的那一个**（他自己设过就是他的，否则实例级那一格 ✓）；
    // 设置读不出来 / 那个值不合法（**只可能是手改坏了文件** ✓）
    // ⇒ 回落默认值种下去，**不 panic、也不不种** ✗：一条都没有的话，界面上那张卡
    // 连"下一次什么时候"都说不出来 ✓。
    let at = user_at(store, owner).await;
    let schedule = seeded_auth_refresh_schedule(owner, &at).ok_or_else(|| {
        Error::Config(format!(
            "凭据自动刷新的预设时间不合法（{at}），种不出那条定时"
        ))
    })?;
    store.save_schedule(&schedule).await?;
    tracing::info!(
        owner = %owner,
        schedule = %id,
        at = %at,
        "已种下「凭据自动刷新定时」（意愿=开、到点先走既有判据）"
    );
    Ok(true)
}

/// 某个用户**现在生效**的预设钟点（`HH:MM` ✓）—— 本模块读钟点的**唯一入口** ✓。
///
/// # 三层，顺序不能换
///
/// 1. **他自己设过的**（`data/auth-refresh.json` 里他那一行 ✓）—— 有就用它 ✓；
/// 2. 没有 ⇒ **实例级那一格**（`AppSettings::auth_refresh_at` ✓，**老键、留着** ✓）——
///    它是"每用户值的初始值"，于是老实例里用户改过的钟点**不会凭空回到默认** ✓
///    （判据在 [`effective_at`] ✓，这里只调它 ✓）；
/// 3. 连那一格也不合法（**只可能是手改坏了 `settings.json`** ✓）⇒ 回默认值，
///    并留一条 warn（[`normalized_at`] ✓，不静默 ✓）。
///
/// ⚠ 它**不读盘失败也不报错**（回 `Result` 的那两处读取一律 `unwrap_or_default` ✓）：
/// 这是"界面上那张卡能不能画出来"的前置，为一个坏文件把整张卡打没了不划算 ✓
///（而坏值会**如实显示出来** —— 卡片上那个输入框就是它 ✓，不骗人 ✓）。
async fn user_at(store: &Arc<dyn Store>, owner: &UserId) -> String {
    let own = store.auth_refresh_at(owner).await.unwrap_or(None);
    if let Some(v) = &own {
        // 手改坏了那一格：**不认它**、回落实例级 —— 但要留痕（不静默 ✓）
        //（认了的话 `daily_expr` 当场报"写不成 cron"，表现是"卡上写着 半夜、定时却装不上"✗）。
        if mcloud_core::auth_refresh::parse_clock(v).is_none() {
            tracing::warn!(
                owner = %owner,
                at = %v,
                "auth-refresh.json 里这个用户的钟点不合法 ⇒ 这次按实例级默认值处理"
            );
        }
    }
    let instance = store.settings().await.unwrap_or_default().auth_refresh_at;
    normalized_at(&effective_at(own.as_deref(), &instance))
}

/// 预设时间**归一化**：不合法就回默认值。
///
/// ⚠ 只有**手改坏了落盘文件**才会走到那一支（`data/auth-refresh.json` 里他自己那一格
/// 或 `data/settings.json` 那一格 ✓）—— HTTP 与 CLI 两条路都先过了
/// `mcloud_core::auth_refresh::parse_clock` ✓，存储那层也守了一道 ✓
/// ⇒ 这里回落默认值是"让实例还能跑"的处置，并**留一条 warn**（不静默 ✓）。
fn normalized_at(raw: &str) -> String {
    if mcloud_core::auth_refresh::parse_clock(raw).is_some() {
        return raw.trim().to_string();
    }
    tracing::warn!(
        at = %raw,
        "设置里的预设刷新时间不合法 ⇒ 这次按默认值处理（界面上会显示出来）"
    );
    DEFAULT_AUTH_REFRESH_AT.to_string()
}

/// **重算入口**：把预设钟点写成 cron 并 upsert 那条定时。
///
/// 做四件事，顺序要紧：
///
/// 1. 读那条自动定时（可能还不存在 ✓），取出**用户意愿**；
/// 2. 读**这个用户**的预设钟点（`patch.at` 给了就用它 ✓，并**只写他自己那一格** ✓）；
/// 3. 把钟点写成 cron（[`daily_expr`] ✓）；
/// 4. upsert：`expr` 与 `enabled` 每次都写 ✓，`accounts` / 记账字段原样保留 ✓。
///
/// # ⚠ 改钟点**只动这个用户**（2026-10-11 用户口径：「要每用户一个钟点」✓）
///
/// 写的是 `data/auth-refresh.json` 里**他那一行**（`save_auth_refresh_at` ✓），
/// **不碰** `AppSettings::auth_refresh_at` ✗ —— 那一格是**实例级默认值**
///（老键 ✓），写它等于把"还没自己设过的所有人"的钟点一起改了 ✗。
/// 同理，这里 upsert 的只有 `auto-auth-refresh-<他自己>` 这一条 ✓
///（别人的那条连读都不读 ✓）。
///
/// # ⚠ `enabled` = **用户意愿**（与「直播自动定时」不同的那一点）
///
/// 那一条的 `enabled` 是**算出来的**（有下一场才开 ✓）；这一条的预设钟点
/// **永远算得出来**（它就是设置里的一个值 ✓）⇒ `enabled` 就是意愿本身 ✓。
/// 于是"用户关掉后不许再打开"这条口径由**同一个判据**保住：
/// `managed_wanted = Some(false)` 一直保留着，重算读到的 `wanted` 就是 `false` ✓
///（`None` 才按"要"算 ✓，见下面那句 `unwrap_or(true)` ✓）。
/// ⚠ 改钟点**不许**把它打开 ✗（用户口径点名的那条 ✓）。
///
/// # 撞车保护（两条都算"撞车"）
///
/// `auto-auth-refresh-<owner>` 这个 id 用户**也能**手建 ✗（id 是用户可写的 ✓）。
/// 而 `ScheduleId` 是**全局唯一**的 ⇒ 这个 id 上可能有三种东西：
///
/// | id 上是什么 | 处置 |
/// |---|---|
/// | 没有 | 正常：按这个用户的钟点建一条 ✓ |
/// | **他自己**那条（`managed_by = auth-refresh` ✓） | 正常：重算它 ✓ |
/// | 用户手建的（`managed_by = None` ✓）/ **别人**名下的 | **绝不碰** ✗，只回一句可操作的说明 ✓ |
///
/// ⚠ 判据必须**按 id 全局查**（`Store::schedule` ✓，不是 `schedule_for` ✗）——
/// 与 [`seed_auth_refresh_schedule`] 逐字同一条理由，而且这里更要紧：
/// 写的时候 `save_schedule` 是**按 id 全局覆盖**的 ⇒ 只按 `(owner, id)` 查的话，
/// 别人手建的同名定时会**查不到**、然后被这一下**覆盖掉** ✗
///（"别人的一个字节都改不了"就是这么破的 ✓）。撞车时回 `enabled: false` ✓，
/// 说明见 [`squat_note`] ✓。
pub async fn reschedule_auth_refresh(
    store: &Arc<dyn Store>,
    owner: &UserId,
    patch: Option<AuthRefreshPatch>,
) -> Result<AuthRefreshState> {
    let id = auth_refresh_schedule_id(owner);
    let existing = store.schedule(&id).await?;

    // ── 撞车保护（必须在任何写入之前 ✓）──────────────────────────────
    //
    // 判据两条，缺一不可：**带系统标记** ✓ 且 **归属就是这个用户** ✓ ——
    // 只看标记的话，一条被别人复制过去的 `auth-refresh` 定时会被当成"自己那条"
    // 就地改掉 ✗（`owner` 那一格是数据，不是身份 ✓）。
    if let Some(s) = &existing {
        if !is_ours(s, owner) {
            tracing::warn!(
                schedule = %id,
                owner = %owner,
                "auto-auth-refresh-<owner> 这个 id 上是用户手建的（或别人的）定时 ⇒ 不碰它（自动刷新这一轮跳过）"
            );
            return Ok(AuthRefreshState {
                enabled: false,
                at: user_at(store, owner).await,
                next_at: None,
                note: Some(squat_note(&id)),
            });
        }
    }

    // ── 预设钟点：patch 优先（**只写他自己那一格** ✓），否则读现在生效的 ✓ ──
    if let Some(at) = patch.as_ref().and_then(|p| p.at.as_ref()) {
        // ⚠ 非法值**当场拒**（不静默兜默认值 ✗）—— 与 `AppSettings::validate` 同一条判据 ✓
        //（存储那层也守了一道 ✓，这里这一道是为了**给人看的那句话** ✓）。
        if mcloud_core::auth_refresh::parse_clock(at).is_none() {
            return Err(Error::Config(format!(
                "预设刷新时间要是 24 小时制的 HH:MM（例如 03:00），收到的是「{at}」"
            )));
        }
        // ⚠ 这一下**只动他那一行**（`insert` 按 owner 覆盖 ✓）—— 别人的钟点碰不到 ✓。
        // 也**不动** `AppSettings::auth_refresh_at` ✗（那是实例级默认值 ✓）。
        store.save_auth_refresh_at(owner, at).await?;
    }
    let at = user_at(store, owner).await;

    // ── 用户意愿：**每次都保留** ✓ ──────────────────────────────────
    //
    // ⚠ `None`（从没表过态）⇒ 按"开"算 ✓；`Some(false)`（他明确关过）
    // **原样保留、绝不覆盖** ✗ —— 与 `live_auto` 里那条逐字同一条口径 ✓
    //（"我关了的开关自己又开了"是用户明确不要的那种表现 ✓）。
    let mut wanted = existing
        .as_ref()
        .and_then(|s| s.managed_wanted)
        .unwrap_or(true);
    if let Some(p) = &patch {
        if let Some(w) = p.wanted {
            wanted = w;
        }
    }

    let expr = daily_expr(&at)
        .ok_or_else(|| Error::Config(format!("预设刷新时间「{at}」写不成 cron（不该发生）")))?;

    // ── upsert：`expr` / `enabled` 每次覆盖，其余原样保留 ✓ ────────────
    //
    // 第一次时以"种下去的那一形态"为底（名字 / 归属 / 标记 / 账号全在那里 ✓），
    // 再把下面那两格写进去 ✓。
    let mut next = existing
        .clone()
        .or_else(|| seeded_auth_refresh_schedule(owner, &at))
        .ok_or_else(|| Error::Config(format!("预设刷新时间「{at}」不合法，建不出那条定时")))?;
    next.expr = expr;
    next.enabled = wanted;
    next.managed_by = Some(MANAGED_BY_AUTH_REFRESH.to_string());
    next.managed_wanted = Some(wanted);
    store.save_schedule(&next).await?;

    Ok(AuthRefreshState {
        enabled: wanted,
        at: at.clone(),
        next_at: Some(next_stamp(&at)),
        note: None,
    })
}

/// **只读**状态（`GET` ✓）—— 不写盘、不重算 ✓。
///
/// 判据与 [`reschedule_auth_refresh`] **同一套**（意愿 / 撞车 / 预设时间 ✓），
/// 只是不落盘 —— 两处各写一遍的话，"界面显示的下一次"与"实际会响的那一刻"
/// 迟早分叉 ✗（撞车那一条尤其：读那边若按 `(owner, id)` 查、写那边按 id 全局查，
/// 同一个 id 在两处就会得到两个结论 ✓）。
pub async fn read_auth_refresh_state(
    store: &Arc<dyn Store>,
    owner: &UserId,
) -> Result<AuthRefreshState> {
    let id = auth_refresh_schedule_id(owner);
    let existing = store.schedule(&id).await?;
    let at = user_at(store, owner).await;

    if let Some(s) = &existing {
        if !is_ours(s, owner) {
            return Ok(AuthRefreshState {
                enabled: false,
                at,
                next_at: None,
                note: Some(squat_note(&id)),
            });
        }
    }

    // ⚠ `None`（还没种过 / 从没表过态）⇒ 按"开"算 ✓（与重算那一处**同一格** ✓）。
    let wanted = existing
        .as_ref()
        .and_then(|s| s.managed_wanted)
        .unwrap_or(true);

    Ok(AuthRefreshState {
        enabled: wanted,
        at: at.clone(),
        next_at: Some(next_stamp(&at)),
        note: None,
    })
}

/// 这个 id 上那条定时**是不是系统为这个用户维护的那一条** ✓。
///
/// 两条判据缺一不可（理由见 [`reschedule_auth_refresh`] 的撞车那一段 ✓）：
/// * `managed_by = Some(auth-refresh)` —— 系统只动自己那条 ✓；
/// * `owner = Some(这个用户)` —— `owner` 那一格是数据，不能只靠它、但也不能不看它 ✓。
fn is_ours(s: &Schedule, owner: &UserId) -> bool {
    s.is_managed_by(MANAGED_BY_AUTH_REFRESH) && s.owner.as_ref() == Some(owner)
}

/// 下一次触发时刻（东八区墙钟串 ✓）。
///
/// ⚠ 它必须与调度器实际会响的那一刻**一致**：那条定时的表达式就是"每天那个钟点"
///（[`daily_expr`] ✓）⇒ "下一次"= 今天那个钟点**还没到就今天、过了就明天** ✓，
/// 而**钟点本身恒等于预设时间** ✓（`next_clock` 就是它 ✓）。
///
/// ⚠ 这里**不引入时钟**：引入的话就要自己处理"今天 / 明天"那个分支，而它与调度器
/// 真正用的 cron（带时区、带水位线 ✓）是**两套算法** —— 各算一遍迟早分叉 ✗。
/// 于是这里只报**钟点**（与 `Schedule::expr` 逐字对应 ✓），"每天"这层意思由这句话说清 ✓。
fn next_stamp(at: &str) -> String {
    match mcloud_core::auth_refresh::next_clock(at) {
        Some(clock) => format!("每天 {clock}（东八区）"),
        None => String::new(),
    }
}

/// 撞车时给用户看的一句话（可操作 ✓，不是"出错了" ✓）。
///
/// ⚠ 说的是"这个 id 上有一条**不归系统维护**的定时"——它可能是用户手建的（常见 ✓），
/// 也可能是别人名下恰好叫这个名的（id 是全局唯一的 ✓）。两种都不碰 ✓，
/// 而对用户来说"改名或删掉那条"是同一个动作 ⇒ 一句话就够 ✓。
fn squat_note(id: &ScheduleId) -> String {
    format!(
        "定时 `{id}` 这个名字上有一条**不是系统为你维护的**定时（多半是你自己手建的）\
         ⇒ 系统不会去动它，所以「凭据自动刷新」这一条装不上。\
         改名或删掉那条定时之后，这里会自动恢复。"
    )
}

/// 调度器那条钩子（见 `mcloud_engine::scheduler::AuthRefreshHook` ✓）。
///
/// # 它干什么
///
/// 到点了 ⇒ 对**每个账号**调一次既有判据（[`AuthRefreshService::refresh`] ✓），
/// 把逐条结局汇总成一句人话写进那条定时的 `last_status` / `last_error` ✓，
/// 并返回调度器要的 [`mcloud_engine::FireOutcome`] ✓。
///
/// # ⚠ 三件**不做**的事
///
/// 1. **不重写判据** ✗：`force` 恒为 `false`（"到点必刷"不是用户口径 ✓ ——
///    用户要的是"到点**检查**，该刷就刷" ✓），提前量走账号配置 ✓；
/// 2. **不越过"需人工"** ✗：`ManualOnly` 记成失败并说清"请重新登录" ✓
///    （它与 `Skipped` 的处置完全不同，见 `RefreshOutcome` 的说明 ✓）；
/// 3. **不 panic、不冒泡** ✗：它跑在调度器的后台任务里（`tick_inner` 的
///    `Some(this)` 分支 ✓），任何错误都只能记下来 ✓。
///
/// # 为什么"没有账号"记 `ok` 而不是 `skipped`
///
/// `skipped` 在「定时」页上的语义是"**到点了但没跑成**"（被并发闸挡住 ✓）。
/// 而"这个用户名下一个账号都没有"是**正常状态**（他还没加账号 ✓）——
/// 记成 `skipped` 会让他以为定时坏了 ✓。⚠ 也**不动** `last_run_ms`
///（传 `None` ✓：这一趟什么都没做，抹掉真实的上次运行时间会显示成"从未执行" ✓）。
///
/// # ⚠ 记账在这里做（`Store::record_schedule_run` ✓）
///
/// 钩子手里本来就有 `Store` ✓ ⇒ 由它写那三列（`last_status` / `last_error` /
/// `last_run_ms`）✓ —— 调度器那边就不必再开一个"代别人记账"的公开口子 ✓
///（理由见 `AuthRefreshHook` 的说明 ✓）。
pub fn auth_refresh_hook(
    service: Arc<AuthRefreshService>,
    store: Arc<dyn Store>,
) -> mcloud_engine::AuthRefreshHook {
    Arc::new(
        move |sch: &Schedule,
              accounts: &[Account],
              _point_ms: i64|
              -> std::pin::Pin<
            Box<dyn std::future::Future<Output = mcloud_engine::FireOutcome> + Send + 'static>,
        > {
            let service = Arc::clone(&service);
            let store = Arc::clone(&store);
            let sch = sch.clone();
            let accounts = accounts.to_vec();
            Box::pin(async move {
                if accounts.is_empty() {
                    tracing::info!(
                        schedule = %sch.id,
                        "凭据自动刷新：这个用户名下没有账号，这一趟什么都不做"
                    );
                    record(&store, &sch, None, "ok", None).await;
                    // ⚠ `SkippedBusy` 在这里只是"没有可跑的东西"的中性表达
                    //（凭据刷新不产生 `RunRecord` ⇒ 没有 `run_id` 可报 ✓）。
                    return mcloud_engine::FireOutcome::SkippedBusy;
                }

                let mut rotated = 0usize;
                let mut skipped = 0usize;
                let mut manual: Vec<String> = Vec::new();
                let mut failures: Vec<String> = Vec::new();

                for account in &accounts {
                    // 提前量走**账号配置**（`refresh_token_days` ✓）；
                    // `force = false` —— 到点只是"检查"，该刷才刷 ✓。
                    match service.refresh(account, None, false).await {
                        Ok(RefreshOutcome::Rotated { forced, .. }) => {
                            rotated += 1;
                            tracing::info!(
                                schedule = %sch.id,
                                account = %account.id,
                                forced,
                                "凭据自动刷新：已换新凭据"
                            );
                        }
                        Ok(RefreshOutcome::Skipped { remaining_days }) => {
                            skipped += 1;
                            // ⭐ **"跳过的原因"必须写进日志**（用户口径 ✓）：
                            // 不写的话，用户看到"到点了却什么都没发生"会以为定时坏了 ✓。
                            tracing::info!(
                                schedule = %sch.id,
                                account = %account.id,
                                remaining_days,
                                "凭据自动刷新：未到提前量（还剩 {remaining_days:.1} 天），未打上游"
                            );
                        }
                        Ok(RefreshOutcome::ManualOnly { expire_ms }) => {
                            manual.push(account.id.to_string());
                            tracing::warn!(
                                schedule = %sch.id,
                                account = %account.id,
                                expire_ms,
                                "凭据自动刷新：库里没有有效的过期时刻 ⇒ 请重新登录该账号"
                            );
                        }
                        Err(e) => {
                            failures.push(format!("{}：{e}", account.id));
                            tracing::warn!(
                                schedule = %sch.id,
                                account = %account.id,
                                error = %e,
                                "凭据自动刷新：刷新失败"
                            );
                        }
                    }
                }

                // ── 记账：**部分失败记 error，不记 ok** ✓ ────────────────
                //（与 `fire()` 那条取向逐字相同：定时没人当场看着，
                //  "一半没成"记成 ok 就等于把它藏起来 ✓）
                let total = rotated + skipped + manual.len() + failures.len();
                let summary =
                    format!("已换 {rotated} 个、未到提前量跳过 {skipped} 个、共 {total} 个");
                let mut errs: Vec<String> = Vec::new();
                if !manual.is_empty() {
                    errs.push(format!(
                        "需重新登录（库里没有有效的过期时刻）：{}",
                        manual.join("、")
                    ));
                }
                if !failures.is_empty() {
                    errs.push(format!("刷新失败：{}", failures.join("；")));
                }
                let status = if errs.is_empty() { "ok" } else { "error" };
                let err_text = if errs.is_empty() {
                    None
                } else {
                    Some(format!("{summary}；{}", errs.join("；")))
                };
                tracing::info!(schedule = %sch.id, status, %summary, "凭据自动刷新：这一趟的结论");

                // ⚠ `last_run_ms` 传 `None`：凭据刷新**不产生 `RunRecord`**，
                // 「上次运行」那一格拿它没有意义 ✓（真实结论在 `last_status` 上 ✓）。
                record(&store, &sch, None, status, err_text.as_deref()).await;

                // 凭据刷新不产生 `RunRecord` ⇒ 没有 `run_id` 可报 ✓。
                mcloud_engine::FireOutcome::SkippedBusy
            })
        },
    )
}

/// 写那条定时的记账三列（`last_status` / `last_error` / `last_run_ms` ✓）。
///
/// ⚠ 失败只 warn：调度器的触发路径**不该**因为"记账写不进去"而失败 ✓
///（与 `Scheduler::record` 那条处置逐字相同 ✓）。
async fn record(
    store: &Arc<dyn Store>,
    sch: &Schedule,
    ran_at_ms: Option<i64>,
    status: &str,
    error: Option<&str>,
) {
    if let Err(e) = store
        .record_schedule_run(&sch.id, ran_at_ms, status, error)
        .await
    {
        tracing::warn!(schedule = %sch.id, error = %e, "凭据自动刷新的结论记账失败");
    }
}

#[cfg(test)]
mod tests;
