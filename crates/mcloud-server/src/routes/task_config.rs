//! 任务的参数配置。
//!
//! # 为什么单独一组接口
//!
//! 任务的参数（`dryRun` / `live-room` 的 `codes`、`listenSeconds`…）从前**没有任何
//! 界面入口**：任务页把参数名印成一行文字、运行请求体的 `body` 恒为 `{}`，
//! 想开"试运行"只能 curl。用户口径（2026-10-01）：任务页保持简单，
//! **参数挪到单独的配置页** —— 这一组接口就是那份配置的落点。
//!
//! # 存的是"默认参数"，不是"这一次的参数"
//!
//! 任务页点「运行」时把这份参数带上；定时规则、编排步骤各自带自己的 `body`，
//! **不受这里影响**。理由是"显式优于隐式"：一次运行真正收到什么参数，
//! 只取决于它的发起方，不取决于某个悄悄生效的全局默认值。
//!
//! # 校验在服务端做
//!
//! 前端会照 [`ParamSpec`] 画表单，但**拦不住 curl**。所以类型、取值范围、
//! "有没有这个参数"都在这里判一遍：不认识的键直接拒 ——
//! 默默收下的表现是用户填了却毫无作用，那是最难查的一类。

use std::collections::HashSet;

use axum::extract::{Path, State};
use axum::Json;
use mcloud_core::Error;
use mcloud_engine::{ParamKind, ParamSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskConfigView {
    pub name: String,
    /// 该任务的参数说明（前端照它画表单 + 做本地校验）。
    pub specs: Vec<ParamSpec>,
    /// 已保存的值。
    ///
    /// **没配过的键也会出现**，值是该参数的默认值 —— 前端不必自己算默认，
    /// 也就不会出现"前端默认 60、后端默认 30"这种两处各说一套的情况。
    pub params: Value,
}

#[derive(Deserialize)]
pub struct TaskConfigPatch {
    /// 要保存的参数。没给的键**回落到默认值**（PUT = 整体替换，与策略那条同口径）。
    #[serde(default)]
    pub params: Value,
}

/// 取任务的参数说明；任务不存在 ⇒ 404（不是空列表：那会让界面显示一个空表单）。
fn specs_of(state: &ServerState, name: &str) -> Result<Vec<ParamSpec>, ApiError> {
    let task = state
        .handle
        .registry
        .get(name)
        .ok_or_else(|| ApiError(Error::NotFound(format!("任务不存在：{name}"))))?;
    Ok(task.params())
}

fn kind_name(kind: ParamKind) -> &'static str {
    match kind {
        ParamKind::Bool => "true / false",
        ParamKind::Number => "数字",
        ParamKind::Text => "文本",
    }
}

/// 按说明归一化：补齐缺的键、校验类型与范围、拒绝不认识的键。
fn normalize(specs: &[ParamSpec], given: &Value) -> Result<Value, ApiError> {
    let obj = given
        .as_object()
        .ok_or_else(|| ApiError(Error::Config("params 必须是一个对象".into())))?;

    let known: HashSet<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    if let Some(extra) = obj.keys().find(|k| !known.contains(k.as_str())) {
        let supported = if specs.is_empty() {
            "无".to_string()
        } else {
            specs
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        };
        return Err(ApiError(Error::Config(format!(
            "这个任务没有参数「{extra}」；它认得的参数：{supported}"
        ))));
    }

    let mut out = serde_json::Map::new();
    for spec in specs {
        let raw = obj.get(&spec.name).unwrap_or(&spec.default);
        let value = match spec.kind {
            ParamKind::Bool => raw
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| type_error(spec, "true / false"))?,
            ParamKind::Number => {
                let n = raw.as_f64().ok_or_else(|| type_error(spec, "数字"))?;
                if spec.min.is_some_and(|min| n < min) || spec.max.is_some_and(|max| n > max) {
                    return Err(ApiError(Error::Config(format!(
                        "参数「{}」超出范围：{}~{}",
                        spec.name,
                        spec.min
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".into()),
                        spec.max
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".into())
                    ))));
                }
                serde_json::json!(n)
            }
            ParamKind::Text => raw
                .as_str()
                .map(|t| Value::String(t.to_string()))
                .ok_or_else(|| type_error(spec, "文本"))?,
        };
        out.insert(spec.name.clone(), value);
    }
    Ok(Value::Object(out))
}

fn type_error(spec: &ParamSpec, want: &str) -> ApiError {
    ApiError(Error::Config(format!(
        "参数「{}」（{}）要填{}{}",
        spec.name,
        spec.title,
        want,
        match kind_name(spec.kind) {
            "true / false" => "",
            _ => "",
        }
    )))
}

/// `GET /api/tasks/{name}/config`
pub async fn get_one(
    State(state): State<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<TaskConfigView> {
    let specs = specs_of(&state, &name)?;
    let saved = state.handle.store.task_params(&name).await?;
    // 存过的值与默认值合并后再归一化：老配置里缺了新加的键也能正常读出来。
    let merged = normalize(&specs, &merge(&specs, &saved))?;
    Ok(Json(TaskConfigView {
        name,
        specs,
        params: merged,
    }))
}

/// 把存下来的值与默认值合起来（存过的优先）。非对象的存量一律当空。
fn merge(specs: &[ParamSpec], saved: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for spec in specs {
        let v = saved
            .get(&spec.name)
            .cloned()
            .unwrap_or_else(|| spec.default.clone());
        out.insert(spec.name.clone(), v);
    }
    Value::Object(out)
}

/// `PUT /api/tasks/{name}/config`
pub async fn put_one(
    State(state): State<ServerState>,
    Path(name): Path<String>,
    Json(body): Json<TaskConfigPatch>,
) -> ApiResult<TaskConfigView> {
    let specs = specs_of(&state, &name)?;
    let normalized = normalize(&specs, &body.params)?;
    state
        .handle
        .store
        .save_task_params(&name, &normalized)
        .await?;
    tracing::info!(task = %name, "任务参数已保存");
    Ok(Json(TaskConfigView {
        name,
        specs,
        params: normalized,
    }))
}
