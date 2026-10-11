//! mcloud 入口：**一个二进制，两种用法**。
//!
//! * **不带参数** ⇒ 起 HTTP 服务（原 `mcloud-server`）
//! * **带子命令** ⇒ 命令行（原 `mcloud-cli`，逻辑在 `mcloud_server::cli::run`）
//!
//! # 为什么合并（2026-10-03 用户口径：「合并为一个，以后就叫 mcloud」）
//!
//! 两者本来就在**同一个 crate**、共用同一张依赖表 ⇒ 实测（同源码、同 release profile）
//! `mcloud-server` 4.05 MB + `mcloud-cli` 3.12 MB = 7.17 MB，合并成一个只有 **4.16 MB**
//! （只比 server 多 **116 KB**，那就是 CLI 独有的代码）✓。省下的一整个二进制是次要的，
//! 主要是交付形态变成 `Cargo.toml` 里写的那句"拷一个二进制就能跑" ✓。
//!
//! ⚠ 分发规则刻意定成「**无参数 = 起服务**」：托管（launchd）那份就是不带参数跑的，
//! 而 CLI 的用法文本本来就写着 `mcloud <命令>` ⇒ 两条路都自然 ✓。
//! 反过来（要求 `mcloud serve`）会让托管方那条命令行也得改，且与既有文档不一致 ✗。

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // ⚠ 两条路的**日志默认值保持各自原样**（合并前 server 与 cli 各有一份）：
    //   服务要 `mcloud=debug`（调度/任务那些才看得见），CLI 只要 `info`
    //   —— 后者是命令输出，塞 debug 日志进去会把结果淹掉 ✗。
    // `RUST_LOG` 仍然优先（两条路都是 `try_from_default_env`）。
    let default_filter = if args.is_empty() {
        "info,mcloud=debug"
    } else {
        "info"
    };
    // ⚠ 从 `fmt().init()` 换成"registry + 两个 layer"：**stdout 那份输出一个字不变** ✓
    // （`fmt::layer()` 就是原来的默认格式化 ✓），只是**多挂一层**把事件抄进
    // 环形缓冲 ✓ —— 给界面上那个「日志」窗口用 ✓（见 `logbuf.rs` 的模块头）。
    //
    // 顺序：filter 在前 ✓（被过滤掉的事件**不会**进环 ✓ —— 否则开了 trace 的
    // 进程会把环冲爆 ✗，而环本来只该装"按当前档位真的会打出来"的东西 ✓）。
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .with(tracing_subscriber::fmt::layer())
        .with(mcloud_server::logbuf::RingLayer)
        .init();

    if args.is_empty() {
        serve().await
    } else {
        mcloud_server::cli::run::run(&args).await
    }
}

/// 起 HTTP 服务（原 `mcloud-server` 的 main 本体，逐行照搬）。
async fn serve() -> anyhow::Result<()> {
    let data_dir = std::env::var("MCLOUD_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let addr = std::env::var("MCLOUD_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    // 推送渠道是**每账号**的（`AccountSettings::notify`），**没有**进程级兜底：
    // 不能有「一份 env 管全进程」的推送配置 —— 那是串号（A 的渠道套到 B 的
    // 运行上）。账号没配 ⇒ 一个包都不发。
    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .build()
        .context("组装应用失败")?;

    // ⚠ **必须在起服务之前**：把 run id 续到历史最大值之后。
    // 不做的话重启后编号从 1 重来 —— 而运行记录按编号 upsert，
    // 新记录会把上次启动留下的同编号记录覆盖掉（症状：历史"越重启越少"）。
    app.handle.seed_run_ids().await?;

    // ⚠ **必须在调度器读盘之前**：那条默认定时是种进库里的，种晚了这一趟启动就看不见它
    // （`scheduler.rs` 在 `run_loop` 里读一次 `store.schedules()` ✓）⇒ 表现成"第一次启动
    // 不跑，重启一次才跑" ✗。
    // 用户口径 2026-10-07：「不自动新建怎么算默认」✓ —— 默认编排 + 默认定时开箱就有 ✓。
    app.handle.seed_defaults().await?;

    // ⚠ 服务端日志**落盘**（2026-10-09 用户口径：「使用本地记录」✓）：
    // 在这之前它只在内存环里（2000 行封顶、重启即清空 ✗）。挂上目录之后，
    // 每条 `tracing` 事件都会攒批写进 `data/server-logs/<日期>.jsonl` ✓，
    // 内存里只留窗口（行数由设置 `serverLogMemLines` 管 ✓），翻页时按需读盘 ✓。
    // 顺带把两格设置推给它（内存窗口 + 磁盘上限 ✓）。
    let settings = app.handle.store.settings().await?;
    mcloud_server::logbuf::init(std::path::Path::new(&data_dir), &settings);

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

    // 「重启服务」那条路要用的信号：先取出来，再把 handle 交给 state
    let restart = Arc::clone(&app.handle.shutdown);
    // ⚠ 顺手清过期会话要用到 store 与 clock（审计 `SEC-14`），而下一行就把
    // `app.handle` 移进 `Arc::new` 了 ⇒ **必须在这之前**各拿一份 Arc ✓
    // （放后面就是 `borrow of moved value`，编译期就会拦下来）。
    let store_for_purge = Arc::clone(&app.handle.store);
    let clock_for_purge = Arc::clone(&app.handle.clock);
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
        // 每 30 个 tick（≈60 秒）清一次过期会话（2026-10-06 安全审计 `SEC-14`）。
        //
        // # 为什么挂在这个循环上、而不是另起一个
        //
        // 这个循环本来就是"每 2 秒干一件小事"（日志落盘），再加一件同量级的事
        // 不增加任何并发面 ✓。而另起一个定时器意味着又多一处要跟着 shutdown 收尾。
        //
        // # 为什么必须做
        //
        // `purge_expired_sessions` 在此之前**没有任何生产调用者** ⇒ 会话表只进不出：
        // 每登录一次就多一行，永远不消失（`SEC-14`）。登录/登出会删自己那一行，
        // 但"关了浏览器不再回来"的那些只能靠清扫 ✓。
        const PURGE_EVERY_TICKS: u32 = 30;
        let mut since_purge: u32 = 0;
        loop {
            tick.tick().await;
            if let Err(e) = hub_for_flush.flush().await {
                tracing::error!(error = %e, "日志落盘失败");
            }
            since_purge += 1;
            if since_purge >= PURGE_EVERY_TICKS {
                since_purge = 0;
                let now = clock_for_purge.now_ms();
                match store_for_purge.purge_expired_sessions(now).await {
                    // 清不到是常态（本工具就一个管理员），别每 60 秒刷一行日志
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "已清理过期会话"),
                    Err(e) => tracing::warn!(error = %e, "清理过期会话失败"),
                }
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
    //
    // ⚠⚠ **必须是 `into_make_service_with_connect_info`，不能是裸 `router`**
    // （2026-10-06 安全审计 `SEC-02`）：认证面的限速按 **peer IP** 分桶，
    // 而 peer IP 只能从连接信息里拿。用裸 `router` 的话 handler 里取不到
    // `ConnectInfo` ⇒ 所有请求挤进同一个桶 ⇒ 静默退回"全局单键"：
    // 既挡不住分布式，又能被未认证者反过来把管理员锁在登录页外 ✗。
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(restart))
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
async fn shutdown_signal(restart: Arc<tokio::sync::Notify>) {
    // WebUI 点「重启服务」走这条：与 SIGTERM 同一条优雅收尾路径。
    // （托管方是 launchd/supervisor 时进程会被重拉；手动前台跑时它就停在这儿。）
    let requested = async move {
        restart.notified().await;
    };

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
        _ = requested => tracing::info!("WebUI 请求重启"),
    }
}
