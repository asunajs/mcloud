//! 定时任务路由。

use axum::extract::{Extension, Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use mcloud_core::push::{PushEvent, PushRuleId};
use mcloud_core::{AccountId, Schedule, ScheduleId};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new()
        .route("/api/schedules", get(list))
        .route("/api/schedules/{id}", put(upsert).delete(remove))
}

/// 列表项：定时 + 服务端算出的下次触发时间。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulePayload {
    #[serde(flatten)]
    schedule: Schedule,
    /// 下次触发时间（毫秒）。
    ///
    /// 服务端算而不是各客户端各算一遍：5/6 段、周日的 0/7、
    /// 日与周的 OR 语义这些细节，各实现一遍必然分叉。
    ///
    /// **未启用时为 null** —— 给一个"下次执行时间"就是在骗人。
    next_fire_at: Option<i64>,
}

impl SchedulePayload {
    fn build(schedule: Schedule, now_ms: i64) -> Self {
        let next = if schedule.enabled {
            mcloud_engine::next_fire_at(&schedule, now_ms)
        } else {
            None
        };
        Self {
            schedule,
            next_fire_at: next,
        }
    }
}

pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<SchedulePayload>> {
    let now = state.handle.clock.now_ms();
    // 只看自己的：别人的定时既不显示、也不能改。
    let all = state.handle.store.schedules_for(&cur.0).await?;
    // 一条坏数据不能让整个页面打不开：算不出下次时间的，nextFireAt 给 null
    Ok(Json(
        all.into_iter()
            .map(|s| SchedulePayload::build(s, now))
            .collect(),
    ))
}

/// 创建或更新（id 在路径里，幂等）。
///
/// 前端生成 id：服务端不发明 id —— 那样"改一个任务"与"建一个任务"
/// 就分不出来了。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleInput {
    pub name: String,
    pub task: String,
    #[serde(default)]
    pub accounts: Vec<String>,
    pub expr: String,
    #[serde(default)]
    pub timezone: Option<String>,
    /// 任务参数。
    ///
    /// ⚠ **`Option` 而不是 `Value`**：`Value` 分不出"没给"和"给了一个 `null`" ✗，
    /// 而界面上**两个动作都不带 `body`**：
    /// * 列表上的「停用/启用」（`toggleEnabled` ✓）—— 它只翻一个布尔位 ✓；
    /// * 编辑保存（表单里没有参数控件 ✓）。
    ///
    /// 用 `Value` 的后果（2026-10-07 用户实测报的就是它）：
    /// 停用一条 `task: "pipeline"` 的定时 ⇒ `body` 变成 `Null` ⇒ 下面那句校验
    /// 「这条定时要跑编排，但 body 里没有 pipelineId」⇒ **连停用都做不到** ✗；
    /// 编辑一条 `live-room` 的定时 ⇒ 它那串口令**被清空** ✗（静默的数据丢失，更坏 ✗）。
    ///
    /// 现在：**没给 = 沿用旧值** ✓（新建时才是 `Null` ✓）。
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    /// 缺省为 **true**：默认停用会让用户第二天来问"为什么没跑"，
    /// 而那是个**静默**的失败。
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// **绑定哪些推送规则**（用户口径 2026-10-07：「推送规则可用于定时任务」✓；
    /// 2026-10-08 更正：「**一个行为可以选择多个规则**」✓ ⇒ 单条变多条 ✓）。
    ///
    /// ⚠ 与上面 `body` **同一个坑**：不带这一格的请求（列表上的「停用/启用」✓、
    /// 老前端 ✓）必须**沿用旧值**，而不是把它清空 ✗。
    ///
    /// ⚠ 语义（照 `double_option` ✓）：**缺席 = 保持原样** ✓、
    /// **给了 `[]` = 清空** ✓（`null` 与 `[]` 同义 ✓ —— 新模型里"空"就是"不推" ✓，
    /// 没有"回到老路"那种第三态 ✗）。
    #[serde(default, deserialize_with = "absent_or_list")]
    pub push_rules: Option<Vec<PushRuleId>>,
    /// **只在哪些运行结局时推**（**空 = 跑完就推** ✓）。
    ///
    /// 同款语义：**缺席 = 保持原样** ✓、**给了 `[]` = 清空** ✓。
    /// ⚠ 这一格只对**运行类**事件有意义 ✓（面板类那四个落在 `/api/push/panel` ✓）。
    #[serde(default, deserialize_with = "absent_or_list")]
    pub push_events: Option<Vec<PushEvent>>,
}

fn default_true() -> bool {
    true
}

pub async fn upsert(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(input): Json<ScheduleInput>,
) -> ApiResult<SchedulePayload> {
    if input.name.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config("名称不能为空".into())));
    }
    // 任务必须在注册表里 —— 存下一个跑不起来的定时，
    // 表现就是"任务天天绿灯空转"。
    // 用 `Config`（→ 400）：错的是请求体里的字段，不是 URL 资源
    // （与 `runs::create` 及 engine 的 `unknown_task_is_config_error` 保持一致）。
    //
    // ⚠ **例外**：`task == PIPELINE_TASK`（"跑编排"）✓ —— 编排**不是**注册表里的任务 ✗
    // （它是一条链，不是 `Task` ✓），调度器对它有**单独的分支**
    // （`Scheduler::fire_pipeline`）。不放行的话，界面上加的那个「定时」按钮
    // 一点就会被这里拒掉 ✗。
    // ⚠ `id` 要在下面取 `existing` **之前**就变成 `ScheduleId`（那两句按 id 查 ✓）。
    let id = ScheduleId::new(id);

    // 保留水位线与记账字段：全量覆盖会把"上次跑成没成"抹掉，
    // 而且会把已经处理过的触发点重放一遍（签到不是幂等的）
    // 归属：取「自己的」那一行（别人的 id 与不存在一样，回 404 不泄露存在性）。
    let existing = state.handle.store.schedule_for(&cur.0, &id).await?;
    if existing.is_none() && state.handle.store.schedule(&id).await?.is_some() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "定时任务不存在：{id}"
        ))));
    }

    // ⚠ **先定出"这次要写的 body"**（缺省沿用旧值 ✓），后面的校验与落库都用它 ——
    // 校验看的是 `input.body` 而落库写另一个的话，就会出现"校验过了、存下去的是空"✗。
    let body = input.body.clone().unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|s| s.body.clone())
            .unwrap_or(serde_json::Value::Null)
    });

    if input.task == mcloud_engine::scheduler::PIPELINE_TASK {
        // ⚠ 建的时候就校验到"这条编排确实存在、而且属于你" ✓：
        // 只在触发时才发现的话，用户要等到那个时间点才知道配错了 ✗
        // （同上面 cron 那条取向：错误要当场说）。
        let Some(pid) = body.get("pipelineId").and_then(|v| v.as_str()) else {
            return Err(ApiError(mcloud_core::Error::Config(
                "这条定时要跑编排，但 body 里没有 pipelineId".into(),
            )));
        };
        let pid = mcloud_core::PipelineId::new(pid);
        if state
            .handle
            .store
            .pipeline_for(&cur.0, &pid)
            .await?
            .is_none()
        {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "编排不存在或不属于你：{pid}"
            ))));
        }
    } else if !state.handle.registry.contains(&input.task) {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "未知任务：{}",
            input.task
        ))));
    }
    // cron 在**创建时**校验：触发时才报错的话，用户要等到那个时间点
    // 才知道自己写错了，而错误信息还淹没在无人看的日志里
    Schedule::validate_expr(&input.expr).map_err(mcloud_core::Error::Config)?;

    // 目标账号必须都是自己的。写进别人的 id 其实不会越权执行（调度器
    // 只按定时的主人过滤账号），但会让用户以为配上了 —— 明确拒绝
    // 比静默丢更不容易误判。
    for account in &input.accounts {
        if state
            .handle
            .store
            .account_for(&cur.0, &AccountId::new(account.clone()))
            .await?
            .is_none()
        {
            return Err(ApiError(mcloud_core::Error::Config(format!(
                "账号不存在或不属于你：{account}"
            ))));
        }
    }

    let timezone = input.timezone.unwrap_or_else(|| "Asia/Shanghai".into());
    mcloud_engine::scheduler::parse_timezone(&timezone).map_err(mcloud_core::Error::Config)?;

    // 老行沿用它的 owner（本来就等于当前用户；单用户下可能是刚回填的 None，
    // 这时补成当前用户，顺手把归属钉死）；全新的一行归当前用户。
    let owner = existing
        .as_ref()
        .and_then(|s| s.owner.clone())
        .or_else(|| Some(cur.0.clone()));
    // ⚠ **在 `Schedule` 字面量之前**算好（与 `body` 同一个口径）：入参没给就沿用老行 ✓。
    // 写在字面量里也行，但那样两个 `.as_ref()` 混在一堆记账字段里，很容易被
    // 后来的改动顺手换成按值消费 ⇒ use-after-move ✗。
    // ⚠ `double_option` 的语义（见 [`double_option`] ✓）：**入参给了**（`[]` 或 `null` ✓）
    // ⇒ 用入参（空清单 = 不推 ✓）；**没给** ⇒ 沿用老行 ✓。
    let push_rules = input.push_rules.clone().unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|s| s.push_rules.clone())
            .unwrap_or_default()
    });
    let push_events = input.push_events.clone().unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|s| s.push_events.clone())
            .unwrap_or_default()
    });
    // **系统维护标记**：从**老行**继承，入参里根本没有这一格 ✓
    //（`ScheduleInput` 刻意不接它 ⇒ 用户伪造不出"我是系统维护的"✗，
    //  也抹不掉它 ⇒ 编辑保存之后系统照旧会维护这条 ✓，见 `Schedule::managed_by` ✓）。
    let managed_by = existing.as_ref().and_then(|s| s.managed_by.clone());
    // **用户意愿**（只对系统维护的那条有意义 ✓）：
    //
    // ⚠ 对系统维护的条目，这一格**就是**界面上那个开关 —— 定时页的
    // 「停用/启用」（`toggleEnabled` ✓）与设置页那张直播卡上的开关，
    // 落到的必须是**同一个**判据 ✓，否则两处开关会各说各话：
    // 在定时页关掉它、系统下一次重算又按"用户想要"把它打开 ✗
    //（那正是用户会报的那种"我关了它自己又开了"✗）。
    //
    // 手建的定时恒为 `None` ✓ —— 它的 `enabled` 就是它自己的全部含义，
    // 没有第二个读者 ✓。
    let managed_wanted = match existing.as_ref() {
        // 系统维护的条目：界面上的开关**就是**用户意愿 ✓
        Some(s) if s.managed_by.is_some() => Some(input.enabled),
        // 手建的定时：这一格恒为 `None` ✓（它没有第二个读者 ✓）
        _ => None,
    };
    let schedule = Schedule {
        id,
        name: input.name,
        task: input.task,
        accounts: input.accounts,
        expr: input.expr,
        timezone,
        body,
        enabled: input.enabled,
        managed_by,
        managed_wanted,
        owner,
        push_rules,
        push_events,
        last_fire_ms: existing.as_ref().and_then(|s| s.last_fire_ms),
        last_run_ms: existing.as_ref().and_then(|s| s.last_run_ms),
        last_status: existing.as_ref().and_then(|s| s.last_status.clone()),
        // ⚠ 记账字段**全都**要留（2026-10-03 review §1.4）：`last_error` 与 `run_count`
        // 原先在这里被重置成 `None` / `0`，与上面那句注释（"保留水位线与记账字段"）
        // **直接相反** ✗ ⇒ 在界面上改个名字就把「累计触发 N 次」清零
        // （`web/src/pages/schedules-page.tsx:384` 确实在展示它），而 `last_status`
        // 还留着 `error`、`last_error` 却是空 ⇒ 写着失败却答不出原因 ——
        // 正是那段注释自己说要避免的形态。
        last_error: existing.as_ref().and_then(|s| s.last_error.clone()),
        run_count: existing.as_ref().map_or(0, |s| s.run_count),
    };
    // ⚠ 只在**新建**时判上限（审计 SEC-16）：更新已有条目不该被"满了"挡住
    if existing.is_none() && state.handle.store.schedules().await?.len() >= super::MAX_SCHEDULES {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "定时条目数已达上限（{}）",
            super::MAX_SCHEDULES
        ))));
    }
    state.handle.store.save_schedule(&schedule).await?;
    let now = state.handle.clock.now_ms();
    Ok(Json(SchedulePayload::build(schedule, now)))
}

pub async fn remove(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let id = ScheduleId::new(id);
    if state
        .handle
        .store
        .schedule_for(&cur.0, &id)
        .await?
        .is_some()
    {
        state.handle.store.remove_schedule(&id).await?;
        return Ok(Json(serde_json::json!({"ok": true})));
    }
    // 存在但不是自己的 ⇒ 404；本来就不存在 ⇒ 幂等 200。
    if state.handle.store.schedule(&id).await?.is_some() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "定时任务不存在：{id}"
        ))));
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

/// 「这一格没给」与「给了（`[]` / `null` / 一串 id）」**分开**。
///
/// # 为什么非要分开（2026-10-07，A 线自报的第 2 条；2026-10-08 换到 `Vec` 上 ✓）
///
/// `Option<Vec<PushRuleId>>` 里，`{"pushRules": []}`、`{"pushRules": null}` 与
/// **根本没有这一格**都反序列化成 `None` ✗ ⇒ 于是只有一种可能：沿用老行 ✓
/// —— **清空不了** ✗。而界面上的「不绑」正是发 `[]`（或 `null` ✓）⇒ 用户选了
/// "不推"、保存后却还绑着，而且**不报错** ✗（最坏的一类）。
///
/// 现在：**没给** ⇒ `#[serde(default)]` 给 `None` ⇒ 沿用 ✓；
/// **给了**（`[]` / `null` / 一串 id ✓）⇒ `Some(..)` ⇒ 用入参 ✓
/// （空清单 = 清空 ✓）。判据就是"反序列化函数**有没有被调用**" ✓
/// —— `null` 也会被调用（它不是"缺席" ✓），所以 `null` 与 `[]` 同义 ✓。
///
/// ⚠ 换到 `Vec` 之后**没有"第三态"**了 ✓：旧模型里"不绑 = 回到账号那份配置"，
/// 新模型里"空 = 不推" ✓ ⇒ `null` 与 `[]` 都清空 ✓。
/// ⚠ `pub(crate)`：设置页那张「直播自动定时」的卡（`routes/live_room.rs` ✓）
/// 用的是**同一套**语义（缺席 = 保持原样、给了 `[]` = 清空 ✓）——
/// 两边各写一份的话，"清空账号列表"这类边界迟早只有一边对 ✓。
pub(crate) fn absent_or_list<'de, T, D>(d: D) -> Result<Option<Vec<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    // `null` → `None` → 折成空清单（= 清空 ✓）；缺席根本不会走到这里 ✓。
    Option::<Vec<T>>::deserialize(d).map(|v| Some(v.unwrap_or_default()))
}
