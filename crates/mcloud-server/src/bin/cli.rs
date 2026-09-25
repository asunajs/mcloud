//! mcloud-gost 命令行入口。

use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = std::env::var("MCLOUD_DATA_DIR").unwrap_or_else(|_| "data".to_string());

    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .build()
        .context("组装应用失败")?;

    match args.first().map(String::as_str) {
        Some("tasks") | None => {
            println!("可用任务（{} 个）：", app.registry().len());
            for m in app.registry().list() {
                let flag = if m.hidden { " [隐藏]" } else { "" };
                println!("  {:<20} {}{}", m.name, m.title, flag);
            }
        }
        Some("run") => {
            let task = args.get(1).context("用法：cli run <任务名> <账号>")?;
            let account = args.get(2).context("用法：cli run <任务名> <账号>")?;
            let id = mcloud_core::AccountId::new(account);
            let acct = app
                .handle
                .store
                .account(&id)
                .await?
                .with_context(|| format!("账号不存在：{account}"))?;
            let record = app
                .runner()
                .run(task, &acct, &serde_json::json!({}))
                .await?;
            println!("[{}] {}", record.outcome.summary(), record.task);
            if record.outcome.is_failure() {
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("未知子命令：{other}");
            eprintln!("可用：tasks | run <任务名> <账号>");
            std::process::exit(2);
        }
    }
    Ok(())
}
