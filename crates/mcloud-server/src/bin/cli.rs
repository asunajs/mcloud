//! mcloud-gost 命令行入口。

use mcloud_server::cli::{
    auth as cli_auth, cron as cli_cron, login as cli_login, pipeline as cli_pipeline,
};

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
        // 不给参数时**打印用法**，而不是列任务。
        //
        // 列任务的写法在只有 `tasks`/`run` 两个命令时还算合理；
        // 现在有 6 个命令，"不带参数"多半是**不知道有什么**，
        // 这时该给的是命令清单。
        None => print_usage(),
        Some("tasks") => {
            println!("可用任务（{} 个）：", app.registry().len());
            for m in app.registry().list() {
                let flag = if m.hidden { " [隐藏]" } else { "" };
                println!("  {:<20} {}{}", m.name, m.title, flag);
            }
        }
        // 看"有没有东西留在用户网盘上"。
        //
        // 这一条**只读内存里的账本**：进程重启后账本就没了
        // （留痕在运行记录的 `details.artifacts` 里，见 `mcloud-engine` 的收尾）。
        // 所以它的用处是"刚跑完想看还剩什么"，而不是"查历史"。
        Some("pending") => {
            let ledger = app.runner().artifacts();
            let runs = ledger.runs();
            let total = ledger.total_pending();
            if runs.is_empty() {
                println!("没有待清理的产物。");
                println!("（进程重启后这里总是空的 —— 历史留痕在运行记录的 details.artifacts）");
            } else {
                println!("待清理产物共 {total} 条，分布在 {} 次运行里：", runs.len());
                for run_id in runs {
                    let items = ledger.pending(run_id);
                    println!("\n  运行 {run_id}（{} 条）：", items.len());
                    for a in items {
                        // 逐条给出「账号 / 类型 / ID」—— 这就是旧项目
                        // 只报个数却说不清是哪几个的那个缺口。
                        println!("    {}  账号={}  ID={}", a.kind, a.account, a.id);
                    }
                }
            }
        }
        Some("login") => {
            // `login` 的子命令自己解析参数（它们要读 stdin，做成"纯 argv"反而绕）
            let rest = &args[1..];
            match rest.first().map(String::as_str) {
                Some("qr") => cli_login::login_qr(&app, &rest[1..]).await?,
                Some("sms") => cli_login::login_sms(&app, &rest[1..]).await?,
                // App 端是**另一套协议**，不是 `sms` 加个平台参数
                Some("app") => cli_login::login_app(&app, &rest[1..]).await?,
                Some("password") => cli_login::login_password(&app, &rest[1..]).await?,
                Some("device") => cli_login::login_device(&app, &rest[1..]).await?,
                Some(other) => {
                    eprintln!("未知登录方式：{other}");
                    print_login_usage();
                    std::process::exit(2);
                }
                None => {
                    print_login_usage();
                    std::process::exit(2);
                }
            }
        }
        Some("cron") => {
            cli_cron::run_schedule(&app, &args[1..]).await?;
        }
        Some("pipeline") => {
            // `pipeline run <id>` 与 `pipeline <id>` 都走这里（形状与 `auth refresh` 对齐）
            cli_pipeline::run_pipeline(&app, &args[1..]).await?;
        }
        Some("auth") => {
            let rest = &args[1..];
            match rest.first().map(String::as_str) {
                Some("refresh") => cli_auth::refresh(&app, &rest[1..]).await?,
                Some(other) => {
                    eprintln!("未知 auth 子命令：{other}");
                    eprintln!("可用：auth refresh <--account <id> | --all>");
                    std::process::exit(2);
                }
                None => {
                    eprintln!(
                        "用法：auth refresh <--account <id> | --all> [--lead-days 7] [--force]"
                    );
                    std::process::exit(2);
                }
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
            print_usage();
            std::process::exit(2);
        }
    }
    Ok(())
}

fn print_login_usage() {
    eprintln!("用法：");
    eprintln!("  login qr [windows|macos]              扫码登录（终端出码）");
    eprintln!("  login sms <手机号> [windows|macos]    短信验证码登录（PC 端）");
    eprintln!("  login app <手机号>                    短信验证码登录（App/手机端）");
    eprintln!("  login password <用户名> [windows|macos] 账号密码登录");
    eprintln!("  login device show|reset               查看/更换本机 PC 设备指纹");
    eprintln!();
    eprintln!("平台默认 windows —— mac 与 win 在请求头、UA 与落盘数据上都不一样，");
    eprintln!("登录后该平台会随账号存下来，后续请求一直沿用它。");
    eprintln!();
    eprintln!("`login app` 与 `login sms` **不是同一条链路**：App 走另一套密钥、");
    eprintln!("另一套设备头，请求体还要加密；落盘的 auth 前缀也不同（mobile vs pc）。");
}

fn print_usage() {
    eprintln!("用法：mcloud-gost <命令> [参数]");
    eprintln!();
    eprintln!("  tasks                          列出可用任务");
    eprintln!("  run <任务名> <账号>             跑一次任务");
    eprintln!("  pending                         查看本进程里还没清理的网盘产物");
    eprintln!("  cron <定时id>                   跑一条已配置的定时（给系统 crontab 调用）");
    eprintln!("  pipeline run <编排id>           跑一条已配置的编排");
    eprintln!("  auth refresh <--account|--all>  刷新凭据");
    eprintln!("  login <方式>                    登录账号（qr / sms / app / password）");
    eprintln!();
    eprintln!("定时与编排的**创建**不在命令行：改配置文件，或用 WebUI。");
    eprintln!("完整的 WebUI（含登录闸、账号管理、日志）请直接运行 `mcloud-gost serve`。");
}
