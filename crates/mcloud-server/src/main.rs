//! mcloud 服务入口。

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
    // 推送渠道是**每账号**的（`AccountSettings::notify`），**没有**进程级兜底：
    // 不能有「一份 env 管全进程」的推送配置 —— 那是串号（A 的渠道套到 B 的
    // 运行上）。账号没配 ⇒ 一个包都不发。
    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .build()
        .context("组装应用失败")?;

    let tasks = app.registry().len();
    tracing::info!(data_dir, tasks, "mcloud 启动");

    // 先取走后面还要用的两个句柄，再把 `AppHandle` **整体**交出去。
    //
    // 这里刻意**不**逐字段重建 `AppHandle`：那样每加一个字段都要回来补一行，
    // 而漏补的表现是"编译不过"（还算好）或"新功能静默不生效"（更糟）。
    // 直接用 `App` 已经组装好的那一个，字段增减对它零影响。
    let scheduler = Arc::clone(app.scheduler());
    let hub = Arc::clone(app.hub());
    let stop = scheduler.stop_handle();
    // 收尾时要拿它去喊停，所以**在 `scheduler` 被 move 进 spawned 任务之前**另留一份。
    let stop_scheduler = Arc::clone(app.scheduler());

    let state = mcloud_server::ServerState::new(Arc::new(app.handle));

    // 启动调度器。
    //
    // 它自己会**立刻跑第一个 tick**，不等一个间隔 —— 定时器的第一次触发
    // 是在一个间隔之后，那样每次重启都要白等 30 秒，而重启恰恰是
    // "错过今天那一趟"最常见的原因。
    //
    // ⚠ 句柄要留着（`ticker`）：退出前得 `await` 它，确认循环真的停了，
    // 否则"收到信号 → 进程退出"会在调度器跑到一半时把日志/状态截断。
    let ticker = tokio::spawn(async move { scheduler.run_loop(stop).await });

    // 日志攒批落盘：与调度同频，保证崩溃时最多丢掉几秒的日志。
    let hub_for_flush = Arc::clone(&hub);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            tick.tick().await;
            if let Err(e) = hub_for_flush.flush().await {
                tracing::error!(error = %e, "日志落盘失败");
            }
        }
    });

    let router = mcloud_server::app(state);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("监听 {addr} 失败"))?;

    tracing::info!("监听 http://{addr}");
    // `with_graceful_shutdown`：收到信号后**先停止接收新连接**，把在途请求做完再返回。
    // 直接 `std::process::exit` 或让 main 掉出去，正在写的响应会被拦腰截断，
    // 前端看到的是"随机一个请求网络错误"—— 而重启是常事（改完代码就要重启一次）。
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("服务异常退出")?;

    // 收尾顺序：**先停调度器**（它可能正跑在 tick 里，得让它自己收干净），
    // 再把攒着的日志落盘。反过来的话，调度器最后那几条日志会丢。
    tracing::info!("正在停止");
    stop_scheduler.stop();
    let _ = ticker.await;
    if let Err(e) = hub.flush().await {
        tracing::error!(error = %e, "退出前日志落盘失败");
    }
    tracing::info!("已停止");
    Ok(())
}

/// 等一个退出信号：`Ctrl-C`（SIGINT）或 `SIGTERM`。
///
/// 两个都要等：`Ctrl-C` 是手动跑（`scripts/run-local.sh`）时的路径，
/// 而 launchd / systemd 停服务发的是 **SIGTERM** —— 只接 `ctrl_c()` 的话，
/// 托管场景下永远是"被 SIGKILL 打死"，优雅收尾那一段等于没写。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            // 装不上 SIGTERM 处理器（极罕见）时不能把整个退出路径挂住：
            // 返回即视为"收到信号"，剩下的交给默认处置。
            Err(e) => tracing::warn!(error = %e, "注册 SIGTERM 处理器失败"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("收到 Ctrl-C"),
        _ = terminate => tracing::info!("收到 SIGTERM"),
    }
}
