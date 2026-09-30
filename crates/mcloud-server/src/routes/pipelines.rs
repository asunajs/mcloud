//! 任务编排路由。

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use mcloud_core::{Account, AccountId, Pipeline, PipelineId, PipelineStep, UserId};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

pub fn routes() -> Router<ServerState> {
    Router::new()
        .route("/api/pipelines", get(list))
        .route("/api/pipelines/{id}", put(upsert).delete(remove))
        .route("/api/pipelines/{id}/run", post(run))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineInput {
    pub name: String,
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub account_mode: mcloud_core::AccountMode,
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
    pub steps: Vec<PipelineStep>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<Vec<Pipeline>> {
    // 只列自己的编排。
    Ok(Json(state.handle.store.pipelines_for(&cur.0).await?))
}

pub async fn upsert(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(input): Json<PipelineInput>,
) -> ApiResult<Pipeline> {
    if input.name.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config("名称不能为空".into())));
    }
    let id = PipelineId::new(id);
    // 归属：只认「自己的」那一行；别人的 id 与不存在一样回 404。
    let existing = state.handle.store.pipeline_for(&cur.0, &id).await?;
    if existing.is_none() && state.handle.store.pipeline(&id).await?.is_some() {
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "编排不存在：{id}"
        ))));
    }
    let owner = existing
        .as_ref()
        .and_then(|p| p.owner.clone())
        .or_else(|| Some(cur.0.clone()));
    let pipeline = Pipeline {
        id,
        name: input.name,
        accounts: input.accounts,
        account_mode: input.account_mode,
        stop_on_error: input.stop_on_error,
        steps: input.steps,
        enabled: input.enabled,
        owner,
        last_run_ms: existing.and_then(|p| p.last_run_ms),
    };

    // 校验在保存**之前**：空 steps、空任务名都必须在创建时拒绝，
    // 而不是运行时静默"点了没反应"
    pipeline.validate().map_err(mcloud_core::Error::Config)?;
    // 并行模式当前不支持 —— 明确报错，不静默按串行跑
    mcloud_engine::pipeline::ensure_supported_account_mode(pipeline.account_mode)?;
    // 整条链预检：否则第 9 步任务名拼错时，前 8 步已经把真上游打了一遍
    state.handle.pipelines.preflight(&pipeline)?;

    state.handle.store.save_pipeline(&pipeline).await?;
    Ok(Json(pipeline))
}

pub async fn remove(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let id = PipelineId::new(id);
    // 先过归属：别人的编排既不能删、也不该从「被定时引用」这句报错里
    // 泄露它的存在。真不存在 ⇒ 幂等 200。
    if state
        .handle
        .store
        .pipeline_for(&cur.0, &id)
        .await?
        .is_none()
    {
        if state.handle.store.pipeline(&id).await?.is_some() {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "编排不存在：{id}"
            ))));
        }
        return Ok(Json(serde_json::json!({"ok": true})));
    }
    // 被定时引用时**拒绝**并列出引用者，而不是级联删除：
    // 级联会静默把用户另一处配好的"每天 08:10 跑它"一起带走
    //（只看自己的定时：别人的定时不构成引用）。
    let refs: Vec<String> = state
        .handle
        .store
        .schedules_for(&cur.0)
        .await?
        .into_iter()
        .filter(|s| s.task == format!("pipeline:{}", id.as_str()))
        .map(|s| s.name)
        .collect();
    if !refs.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "该编排正被定时任务引用，请先删除它们：{}",
            refs.join("、")
        ))));
    }
    state.handle.store.remove_pipeline(&id).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// 触发结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAccepted {
    pub pipeline: String,
    /// 后台执行。前端通过运行历史与 SSE 观察进度。
    pub started: bool,
}

/// 执行一条编排。
///
/// 返回 **202 语义**：立即返回，链在后台跑完。
/// 一条链可能要跑几分钟，不能占着这个请求不放。
pub async fn run(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> ApiResult<RunAccepted> {
    let id = PipelineId::new(id);
    let pipeline = state
        .handle
        .store
        .pipeline_for(&cur.0, &id)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::NotFound(format!("编排不存在：{id}"))))?;

    // 预检同步做：坏清单应当是 400，而不是"已接受但一步都没跑"——
    // 后者在界面上的表现与"还在跑"一模一样
    state.handle.pipelines.preflight(&pipeline)?;

    let accounts = target_accounts(&state, &cur.0, &pipeline.accounts).await?;
    if accounts.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "没有可执行的账号".into(),
        )));
    }

    // 后台跑。这里必须**脱离请求上下文** —— 请求一返回，
    // 它的上下文就被取消了，只脱离"记账"那一步的话，
    // 表现是"用户点了运行、拿到 202，然后每一步都记成'没等完'"。
    let handle = state.handle.clone();
    let started_ms = state.handle.clock.now_ms();
    tokio::spawn(async move {
        let cancel = mcloud_core::CancelToken::new();
        let outcome = handle
            .pipelines
            .run(&pipeline, &accounts, Some(cancel))
            .await;
        match outcome {
            Ok(o) => tracing::info!(pipeline = %pipeline.id, "编排结束：{}", o.summary_line()),
            Err(e) => tracing::error!(pipeline = %pipeline.id, error = %e, "编排失败"),
        }
        // 记账用**窄写**：一次编排要跑几分钟，整行覆盖会把用户在这期间
        // 改过的名字与步骤一起抹掉。
        // 记的是**开始时刻**而不是结束时刻 —— 界面回答的问题是
        // "我配的那条链什么时候跑的"。
        if let Err(e) = handle
            .store
            .mark_pipeline_ran(&pipeline.id, started_ms)
            .await
        {
            tracing::warn!(pipeline = %pipeline.id, error = %e, "编排执行时间记账失败");
        }
    });

    Ok(Json(RunAccepted {
        pipeline: id.as_str().to_string(),
        started: true,
    }))
}

/// 解析目标账号：空 = 全部。
///
/// 返回的是内部用的 `Result`，不是 `ApiResult` —— 它不直接当响应体，
/// 用 `ApiResult` 会让返回值被多包一层 `Json`。
async fn target_accounts(
    state: &ServerState,
    viewer: &UserId,
    wanted: &[String],
) -> Result<Vec<Account>, ApiError> {
    // 空清单 = 「全部」对**当前用户**的全部 —— 不能是库里的全部，
    // 否则一条 `accounts: []` 的编排会把别人的账号也跑一遍。
    let all = state.handle.store.accounts_for(viewer).await?;
    if wanted.is_empty() {
        return Ok(all);
    }
    let ids: Vec<AccountId> = wanted.iter().map(AccountId::new).collect();
    Ok(all.into_iter().filter(|a| ids.contains(&a.id)).collect())
}

// 让 `Arc<AppHandle>` 能被 spawn 里的闭包捕获
#[allow(dead_code)]
fn _assert_send_sync(_: Arc<crate::ServerState>) {}
