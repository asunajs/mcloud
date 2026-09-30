//! 健康检查、版本与后端自述。

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::ServerState;

#[derive(Serialize)]
pub struct VersionInfo {
    pub name: &'static str,
    pub version: &'static str,
}

pub async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        name: "mcloud",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// 后端自述。
///
/// 侧栏底部那一行只显示**后端确实知道**的事实：产品名+版本、已注册任务数、
/// 最近一次执行。刻意**不**给账号数 —— 那是账号页的内容，搬进常驻侧栏
/// 等于把同一个数字出现在两个地方。
///
/// `task_count` 用 `usize`：Rust 的 `Vec`/数字永远序列化成 `[]`/`0`，
/// 不会像 Go 的 nil slice 那样变成 `null`（前端 `null.includes(...)` 会抛
/// TypeError 并整页白屏）。这条差异对我们有利，但仍有测试锁住。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub name: &'static str,
    pub version: &'static str,
    pub task_count: usize,
    /// 最近一次运行；没有则为 `null` —— 前端按 falsy 处理，不会去调方法。
    pub last_run: Option<mcloud_core::RunRecord>,
    /// 本进程**开始服务**的时刻（epoch 毫秒）—— 界面据此算「运行时间」。
    ///
    /// 给的是**时刻**而不是"已经跑了多少秒"：一份读数能自己往前长，
    /// 而一个标量在下一次取数之前只会越来越假（页面不轮询）。
    pub started_at_ms: i64,
    /// 调度循环是否真的在跑。
    ///
    /// 这是**运行时实况**，不是配置项：配置写着 enabled 而循环其实没跑，
    /// 正是最难发现的失真。
    pub scheduler_running: bool,
    /// 当前正在运行的任务数。
    pub running_count: usize,
    /// 定时任务总数。
    pub schedule_count: usize,
    /// 编排总数。
    pub pipeline_count: usize,
}

pub async fn status(State(state): State<ServerState>) -> Json<ServiceStatus> {
    // 历史倒序，取第一条即最近一次；读失败按"没有记录"处理，
    // 不该因为历史文件损坏就让整个自述接口 500。
    let last_run = state.handle.store.runs(1).await.ok().and_then(|mut r| {
        if r.is_empty() {
            None
        } else {
            Some(r.remove(0))
        }
    });

    // 计数读失败一律按 0 处理：自述接口不该因为某个文件损坏就 500
    let schedule_count = state
        .handle
        .store
        .schedules()
        .await
        .map(|v| v.len())
        .unwrap_or(0);
    let pipeline_count = state
        .handle
        .store
        .pipelines()
        .await
        .map(|v| v.len())
        .unwrap_or(0);

    Json(ServiceStatus {
        name: "mcloud",
        version: env!("CARGO_PKG_VERSION"),
        task_count: state.handle.registry.len(),
        last_run,
        started_at_ms: state.started_at_ms,
        scheduler_running: state.handle.scheduler.state() == mcloud_engine::SchedulerState::Running,
        running_count: state.handle.cancel.running_count(),
        schedule_count,
        pipeline_count,
    })
}
