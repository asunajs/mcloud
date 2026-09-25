//! mcloud-gost 服务入口。

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,mcloud=debug")),
        )
        .init();

    let data_dir = std::env::var("MCLOUD_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let addr = std::env::var("MCLOUD_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .build()
        .context("组装应用失败")?;

    let tasks = app.registry().len();
    tracing::info!(data_dir, tasks, "mcloud-gost 启动");

    let state = mcloud_server::ServerState::new(Arc::new(mcloud_app::AppHandle {
        runner: Arc::clone(app.runner()),
        registry: Arc::clone(app.registry()),
        store: Arc::clone(&app.handle.store),
        clock: Arc::clone(&app.handle.clock),
        sink: Arc::clone(app.sink()),
    }));

    let router = mcloud_server::app(state);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;

    tracing::info!("监听 http://{addr}");
    axum::serve(listener, router)
        .await
        .context("服务异常退出")?;
    Ok(())
}
