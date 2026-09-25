//! 健康检查与版本。

use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct VersionInfo {
    pub name: &'static str,
    pub version: &'static str,
}

pub async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        name: "mcloud-gost",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}
