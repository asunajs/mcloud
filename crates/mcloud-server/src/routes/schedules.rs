//! 定时任务：列表、保存、删除。
//!
//! # 为什么这一组能提前落地，而"跑任务"不能
//!
//! 定时任务的**定义**住在 `config` 那半边（`ConfigStore`），与能力是否实现无关：
//! 它可以先存下来，等 P4 的能力齐了再由调度引擎去执行。
//! 而 `POST /api/runs` 必须真的执行一个能力，所以那里等得起。
//!
//! # 保存时校验什么，不校验什么
//!
//! **校验**（存进去就一定是能执行的）：
//!
//! - `cron` 能解析，且**真的会触发** —— `0 0 0 30 2 *`（2 月 30 日）解析得动、
//!   但永远不会命中。存下它，表现就是"任务天天绿灯空转"，正是旧项目最要命的
//!   那类失真（见 `docs/business-feature-inventory.md` §1.4），所以在入口就挡住；
//! - `timezone` 是合法时区名；
//! - `target` 指向的东西**存在**：能力要在注册表里，组合要在本用户可见范围内。
//!   不校验的话，"保存成功"等于"永远不会执行"，而用户看不出区别；
//! - `account_ids` **每一个都属于当前用户**。这一条不只是数据整洁：
//!   定时任务会拿着这些 id 去执行，混进别人的账号等于让 A 的任务去打 B 的账号。
//!
//! **不校验**：能力是否"已实现"。注册表里就 3 项，那就是当前的全部真相 ——
//! 拿 P4 的完成度当校验条件，会让"注册了但还没实现"的能力既存不下也报不清。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use mcloud_core::cron::next_fire;
use mcloud_core::{
    AccountId, ConfigStore, Error, Schedule, ScheduleTarget, Scope, StateStore, TZ_NAME,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AppState, AuthUser};
use crate::error::{ApiError, ApiJson};

/// 任务 id 的长度上限。
///
/// 它不是路径安全的问题（id 只当数据库主键用），而是"别让一个 id 撑爆列表响应"：
/// 这个字段会原样回给前端并在 URL 里出现。
const MAX_ID_LEN: usize = 64;
/// 名称长度上限（同上，纯展示字段）。
const MAX_NAME_LEN: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/schedules", get(list_schedules))
        .route(
            "/api/schedules/{id}",
            put(put_schedule).delete(delete_schedule),
        )
}

/// `GET /api/schedules`
async fn list_schedules(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let schedules = state
        .store()
        .list_schedules(Scope::user(&auth.user.id))
        .await?;
    let now = state.now();

    Ok(Json(json!({
        "schedules": schedules
            .iter()
            .map(|schedule| schedule_payload(schedule, now))
            .collect::<Vec<_>>(),
    })))
}

/// `PUT /api/schedules/:id` —— 新建或覆盖。
///
/// 用 `PUT`（而不是 `POST`）是因为 id 由调用方给在路径里：同一条路径重复提交
/// 是**同一个结果**（幂等），这正是 PUT 的语义。id 由前端生成（沿用旧数据的字符串），
/// 服务端不发明 id —— 那样"改一个任务"与"建一个任务"就分不出来了。
async fn put_schedule(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ScheduleBody>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope::user(&auth.user.id);
    let now = state.now();

    validate_id(&id)?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::Domain(Error::BadRequest("任务名不能为空".into())));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::Domain(Error::BadRequest(format!(
            "任务名最多 {MAX_NAME_LEN} 个字"
        ))));
    }

    let timezone = body.timezone.unwrap_or_else(|| TZ_NAME.to_string());
    // 解析失败与"永远不触发"都要拒：前者是写错了，后者是**存下去也不会跑** ——
    // 而它不会报任何错，只会每天安静地什么都不做。
    match next_fire(&body.cron, &timezone, now) {
        Err(err) => return Err(ApiError::Domain(Error::BadRequest(err.message().into()))),
        Ok(None) => {
            return Err(ApiError::Domain(Error::BadRequest(format!(
                "这个 cron 永远不会触发: {:?}",
                body.cron
            ))));
        }
        Ok(Some(_)) => {}
    }

    // 目标必须存在（见模块头）。两种目标各自查一遍，用的是**同一个 scope** ——
    // 于是"别人的组合"在这里也查不到，不必额外写一条越权判断。
    match &body.target {
        ScheduleTarget::Capability { code } => {
            if mcloud_capabilities::get(code).is_none() {
                return Err(ApiError::Domain(Error::BadRequest(format!(
                    "没有这个能力: {code}"
                ))));
            }
        }
        ScheduleTarget::Pipeline { id: pipeline_id } => {
            if state
                .store()
                .get_pipeline(scope, pipeline_id)
                .await?
                .is_none()
            {
                return Err(ApiError::Domain(Error::BadRequest(format!(
                    "没有这个组合: {pipeline_id}"
                ))));
            }
        }
    }

    // 账号逐个校验归属。空表示"全部账号"（模型里定的语义），所以不报错。
    for account_id in &body.account_ids {
        if state
            .store()
            .get_account(scope, account_id)
            .await?
            .is_none()
        {
            return Err(ApiError::Domain(Error::BadRequest(format!(
                "云盘账号不存在或不属于你: {account_id}"
            ))));
        }
    }

    // 已有的那一行：`created_at` 与调度器写的**记账字段**要留着。
    // 全量覆盖的话，用户改一次启用状态就会把"上次跑成没成"抹掉 ——
    // 而那正是他打开这个页面想看的东西。
    let existing = state.store().get_schedule(scope, &id).await?;

    let schedule = Schedule {
        id: id.clone(),
        owner: Some(auth.user.id.clone()),
        name,
        target: Some(body.target),
        cron: body.cron,
        timezone,
        account_ids: body.account_ids,
        body: body.body,
        enabled: body.enabled,
        created_at: existing.as_ref().and_then(|s| s.created_at).or(Some(now)),
        last_run_at: existing.as_ref().and_then(|s| s.last_run_at),
        last_status: existing.as_ref().and_then(|s| s.last_status),
        last_error: existing.as_ref().and_then(|s| s.last_error.clone()),
    };

    state.store().upsert_schedule(scope, &schedule).await?;

    Ok(Json(
        json!({ "schedule": schedule_payload(&schedule, now) }),
    ))
}

/// `DELETE /api/schedules/:id` —— 删掉。不存在（或不属于你）→ 404。
async fn delete_schedule(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let deleted = state
        .store()
        .delete_schedule(Scope::user(&auth.user.id), &id)
        .await?;

    if !deleted {
        // "不属于你"与"不存在"同一结论（存储层已经这么实现了，不泄漏存在性）
        return Err(ApiError::Domain(Error::NotFound(format!("定时任务 {id}"))));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ScheduleBody {
    name: String,
    /// `{"kind": "capability", "code": "sign"}` 或 `{"kind": "pipeline", "id": "..."}`
    target: ScheduleTarget,
    cron: String,
    /// 缺省 `Asia/Shanghai`（与旧数据、`TZ_NAME` 一致）。
    #[serde(default)]
    timezone: Option<String>,
    /// **空表示全部账号**，不是"没有账号"。
    #[serde(default)]
    account_ids: Vec<AccountId>,
    /// 透传给能力的参数。
    #[serde(default)]
    body: Option<Value>,
    /// 缺省启用：会走到这一步的人都是"特意建了一条定时任务"。
    /// 默认停用会让他第二天来问"为什么没跑"，而那是个**静默**的失败。
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

const fn enabled_by_default() -> bool {
    true
}

fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() {
        return Err(ApiError::Domain(Error::BadRequest(
            "任务 id 不能为空".into(),
        )));
    }
    if id.chars().count() > MAX_ID_LEN {
        return Err(ApiError::Domain(Error::BadRequest(format!(
            "任务 id 最多 {MAX_ID_LEN} 个字符"
        ))));
    }
    // 允许的字符集与"kebab-case 文件名"同一套：id 会被拼进 URL 与日志，
    // 放开到任意字符只会让"看起来像两个任务"的东西在别处变成同一个。
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::Domain(Error::BadRequest(format!(
            "任务 id 只能用字母、数字与 - _ . ：{id}"
        ))));
    }
    Ok(())
}

/// 定时任务的对外形状（含**下次执行时间**）。
///
/// 下次执行由服务端算：前端要显示它，而它依赖 `cron` + `timezone` 两个字段的
/// 联合语义（5 段还是 6 段、周日写 0 还是 7、日与周同时限定时取 OR），
/// 让每个客户端各实现一遍必然分叉。
fn schedule_payload(schedule: &Schedule, now: chrono::DateTime<chrono::Utc>) -> Value {
    // 停用即不会触发 —— 这时给一个"下次执行时间"就是在骗人。
    // 算不出来（导入进来的坏数据）也退化成 null，**不让一整份列表因此 500**：
    // 一条坏数据不该把用户的整个定时任务页打不开。
    let next_fire = if schedule.enabled {
        match next_fire(&schedule.cron, &schedule.timezone, now) {
            Ok(next) => next.map(|t| t.to_rfc3339()),
            Err(err) => {
                tracing::warn!(
                    schedule = %schedule.id,
                    error = %err,
                    "定时任务的 cron 解析失败，列表里按'无下次执行'处理"
                );
                None
            }
        }
    } else {
        None
    };

    json!({
        "id": schedule.id,
        "name": schedule.name,
        "target": schedule.target,
        "cron": schedule.cron,
        "timezone": schedule.timezone,
        "accountIds": schedule.account_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        "body": schedule.body,
        "enabled": schedule.enabled,
        "nextFireAt": next_fire,
        "createdAt": schedule.created_at.map(|t| t.to_rfc3339()),
        "lastRunAt": schedule.last_run_at.map(|t| t.to_rfc3339()),
        "lastStatus": schedule.last_status,
        "lastError": schedule.last_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Clock, FixedClock};

    fn now() -> chrono::DateTime<chrono::Utc> {
        FixedClock::parse("2026-09-20T00:00:00Z").now()
    }

    fn schedule(cron: &str, enabled: bool) -> Schedule {
        Schedule {
            id: "sched-1".into(),
            owner: None,
            name: "每日签到".into(),
            target: Some(ScheduleTarget::Capability {
                code: "sign".into(),
            }),
            cron: cron.into(),
            timezone: TZ_NAME.into(),
            account_ids: Vec::new(),
            body: None,
            enabled,
            created_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }

    #[test]
    fn payload_carries_the_next_fire_time() {
        // 09:00 +08:00，从 2026-09-20T00:00Z（= 08:00 +08:00）起算就是当天 09:00
        let payload = schedule_payload(&schedule("0 0 9 * * *", true), now());
        assert_eq!(payload["nextFireAt"], json!("2026-09-20T01:00:00+00:00"));
    }

    /// 停用的任务不会触发 —— 这时给"下次执行时间"就是在骗人。
    #[test]
    fn a_disabled_schedule_has_no_next_fire_time() {
        let payload = schedule_payload(&schedule("0 0 9 * * *", false), now());
        assert_eq!(payload["nextFireAt"], Value::Null);
    }

    /// 一条坏数据不该把整份列表打不开（真实来源：旧库导入的表达式）。
    #[test]
    fn a_broken_cron_degrades_to_no_next_fire_instead_of_failing_the_list() {
        let payload = schedule_payload(&schedule("这不是 cron", true), now());
        assert_eq!(payload["nextFireAt"], Value::Null);
    }

    #[test]
    fn id_shape_is_checked_before_it_reaches_the_database() {
        assert!(validate_id("sched-1").is_ok());
        assert!(validate_id("a.b_c-9").is_ok());
        assert!(validate_id("").is_err());
        assert!(validate_id(&"x".repeat(MAX_ID_LEN + 1)).is_err());
        assert!(validate_id("带中文").is_err());
    }
}
