//! 任务能力清单。

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::ServerState;

#[derive(Serialize)]
pub struct Capabilities {
    pub tasks: Vec<mcloud_engine::TaskMeta>,
}

/// 列出全部任务（含隐藏任务，前端自行决定是否展示）。
pub async fn list(State(state): State<ServerState>) -> Json<Capabilities> {
    Json(Capabilities {
        tasks: state.handle.registry.list(),
    })
}
