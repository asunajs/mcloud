//! 服务端二进制：起 HTTP 服务（API + 前端产物）。
//!
//! ```text
//! cargo run -p mcloud-server                       # 默认 127.0.0.1:8787
//! cargo run -p mcloud-server -- --db data/mcloud.db --port 9000
//! ```
//!
//! 前端开发时用 `cd web && npm run dev`（vite 在 5173，已配好到 8787 的代理）；
//! 要看生产形态就 `npm run build`，然后本进程会直接发 `web/dist`。

use anyhow::Context as _;
use clap::Parser;
use mcloud_core::PRODUCT_NAME;
use mcloud_io::{SqliteStore, SystemClock};
use mcloud_server::{AppState, ServerConfig, serve};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "mcloud-server", version, about = "移动云盘助手 · WebUI 后端")]
struct Args {
    /// SQLite 文件（不存在就新建并建表）
    #[arg(long, default_value = "data/mcloud.db")]
    db: PathBuf,
    /// 监听地址。默认**只监听本机** —— 这是个人工具，不该一上来就暴露在局域网上
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// 监听端口
    #[arg(long, default_value_t = 8787)]
    port: u16,
    /// 前端产物目录（`cd web && npm run build` 的输出）
    #[arg(long, default_value = "web/dist")]
    web_dir: PathBuf,
    /// 给会话 cookie 打 `Secure`。**只有放在 HTTPS 反代后面才开** ——
    /// 本地 http 下打开它，浏览器根本不会回传 cookie，
    /// 表现是"登录接口返回成功，但每个请求都是未登录"，很难往 cookie 上想
    #[arg(long)]
    secure_cookie: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mcloud_server=info,tower_http=warn".into()),
        )
        .init();

    let args = Args::parse();
    let store = SqliteStore::open(&args.db)
        .with_context(|| format!("打开数据库失败：{}", args.db.display()))?;

    let state = AppState::new(
        store,
        Arc::new(SystemClock),
        ServerConfig {
            secure_cookie: args.secure_cookie,
        },
    );

    let addr = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("监听 {addr} 失败（端口被占用？）"))?;
    let local = listener.local_addr()?;

    println!("{PRODUCT_NAME} 服务已启动：http://{local}");
    println!("  数据库：{}", args.db.display());
    println!("  前端：{}", args.web_dir.display());
    if !args.web_dir.is_dir() {
        println!("  （前端目录不存在，只提供 API；开发时用 cd web && npm run dev）");
    }

    serve(listener, state, Some(args.web_dir)).await?;
    Ok(())
}
