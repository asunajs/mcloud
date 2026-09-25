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
        hub: Arc::clone(app.hub()),
        scheduler: Arc::clone(app.scheduler()),
        pipelines: Arc::clone(&app.handle.pipelines),
        cancel: Arc::clone(&app.handle.cancel),
    }));

    // 启动调度器。
    //
    // 它自己会**立刻跑第一个 tick**，不等一个间隔 —— 定时器的第一次触发
    // 是在一个间隔之后，那样每次重启都要白等 30 秒，而重启恰恰是
    // "错过今天那一趟"最常见的原因。
    let scheduler = Arc::clone(app.scheduler());
    let stop = scheduler.stop_handle();
    tokio::spawn(async move { scheduler.run_loop(stop).await });

    // 日志攒批落盘：与调度同频，保证崩溃时最多丢掉几秒的日志。
    let hub = Arc::clone(app.hub());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            tick.tick().await;
            if let Err(e) = hub.flush().await {
                tracing::error!(error = %e, "日志落盘失败");
            }
        }
    });

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
