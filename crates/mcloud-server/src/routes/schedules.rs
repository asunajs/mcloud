//! 定时任务路由。

use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use mcloud_core::{Schedule, ScheduleId};

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

pub async fn list(State(state): State<ServerState>) -> ApiResult<Vec<SchedulePayload>> {
    let now = state.handle.clock.now_ms();
    let all = state.handle.store.schedules().await?;
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
    #[serde(default)]
    pub body: serde_json::Value,
    /// 缺省为 **true**：默认停用会让用户第二天来问"为什么没跑"，
    /// 而那是个**静默**的失败。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub async fn upsert(
    State(state): State<ServerState>,
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
    if !state.handle.registry.contains(&input.task) {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "未知任务：{}",
            input.task
        ))));
    }
    // cron 在**创建时**校验：触发时才报错的话，用户要等到那个时间点
    // 才知道自己写错了，而错误信息还淹没在无人看的日志里
    Schedule::validate_expr(&input.expr).map_err(mcloud_core::Error::Config)?;

    let id = ScheduleId::new(id);
    let timezone = input.timezone.unwrap_or_else(|| "Asia/Shanghai".into());
    mcloud_engine::scheduler::parse_timezone(&timezone).map_err(mcloud_core::Error::Config)?;

    // 保留水位线与记账字段：全量覆盖会把"上次跑成没成"抹掉，
    // 而且会把已经处理过的触发点重放一遍（签到不是幂等的）
    let existing = state.handle.store.schedule(&id).await?;
    let schedule = Schedule {
        id,
        name: input.name,
        task: input.task,
        accounts: input.accounts,
        expr: input.expr,
        timezone,
        body: input.body,
        enabled: input.enabled,
        last_fire_ms: existing.as_ref().and_then(|s| s.last_fire_ms),
        last_run_ms: existing.as_ref().and_then(|s| s.last_run_ms),
        last_status: existing.and_then(|s| s.last_status),
        last_error: None,
        run_count: 0,
    };
    state.handle.store.save_schedule(&schedule).await?;
    let now = state.handle.clock.now_ms();
    Ok(Json(SchedulePayload::build(schedule, now)))
}

pub async fn remove(
    State(state): State<ServerState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    state
        .handle
        .store
        .remove_schedule(&ScheduleId::new(id))
        .await?;
    Ok(Json(serde_json::json!({"ok": true})))
}
