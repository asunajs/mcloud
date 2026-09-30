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
    // 推送渠道的 webhook。**没配就完全没有任何行为** —— 空串 / 纯空白也算没配
    // （判据在 mcloud_app::notify，见其模块头）。这是这个功能的可撤性。
    let notify_webhook = std::env::var("MCLOUD_NOTIFY_WEBHOOK").ok();

    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .notify_webhook(notify_webhook)
        .build()
        .context("组装应用失败")?;

    let tasks = app.registry().len();
    tracing::info!(data_dir, tasks, "mcloud-gost 启动");

    // 先取走后面还要用的两个句柄，再把 `AppHandle` **整体**交出去。
    //
    // 这里刻意**不**逐字段重建 `AppHandle`：那样每加一个字段都要回来补一行，
    // 而漏补的表现是"编译不过"（还算好）或"新功能静默不生效"（更糟）。
    // 直接用 `App` 已经组装好的那一个，字段增减对它零影响。
    let scheduler = Arc::clone(app.scheduler());
    let hub = Arc::clone(app.hub());
    let stop = scheduler.stop_handle();

    let state = mcloud_server::ServerState::new(Arc::new(app.handle));

    // 启动调度器。
    //
    // 它自己会**立刻跑第一个 tick**，不等一个间隔 —— 定时器的第一次触发
    // 是在一个间隔之后，那样每次重启都要白等 30 秒，而重启恰恰是
    // "错过今天那一趟"最常见的原因。
    tokio::spawn(async move { scheduler.run_loop(stop).await });

    // 日志攒批落盘：与调度同频，保证崩溃时最多丢掉几秒的日志。
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
