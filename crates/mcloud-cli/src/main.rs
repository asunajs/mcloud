//! mcloud CLI。
//!
//! # 与原 CLI 的关系
//!
//! 这是原 `packages/cli` 的替代入口，共享同一套能力注册表（`mcloud-capabilities`），
//! 因此 WebUI 与 CLI 不会出现"两边能力不一致"的问题。
//!
//! # 版本号
//!
//! 原项目在构建期用 esbuild 的 `define` 注入 `__APP_VERSION__`（读 `package.json`）。
//! Rust 侧不需要这套机制：`env!("CARGO_PKG_VERSION")` 由编译器直接注入，
//! 加上 clap 的 `version` 属性，`--version` 与控制台输出天然一致。
//!
//! # 退出码约定（沿用原项目）
//!
//! 只有**运行错误**才影响退出码；业务层面的 fail 属 info 级，不影响退出码。

mod fixture_extract;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "mcloud",
    version,
    about = "移动云盘自动化工具（Rust 重写版）",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 从 HAR 流量包抽取脱敏 fixture（用于离线回放测试）
    Fixture {
        #[command(subcommand)]
        action: FixtureAction,
    },
    /// 把旧项目（mcloud-sign）的数据迁移到 SQLite
    Migrate(MigrateArgs),
    /// 打印能力注册表（WebUI 表单与 CLI 帮助同源，故此处即契约）
    Capabilities {
        /// 输出原始 JSON（前端/脚本消费用）
        #[arg(long)]
        json: bool,
    },
    /// 打印版本与运行时信息
    Version {
        /// 以 JSON 输出（与 `GET /api/version` 的响应体一致）
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum FixtureAction {
    /// HAR → 脱敏 fixture
    Extract {
        /// 单个 .har 文件或包含 .har 的目录
        #[arg(long, default_value = "har")]
        har: PathBuf,
        /// 输出目录
        #[arg(long, default_value = "test/fixtures/http")]
        out: PathBuf,
        /// 只扫描与自检，不写文件
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::Args, Debug)]
struct MigrateArgs {
    /// 旧项目仓库根（据此推导 `asign.json` 与 `packages/api/data/`）
    #[arg(long, default_value = ".")]
    repo_root: PathBuf,
    /// 旧 `asign.json` 路径（覆盖 `--repo-root` 的推导结果）
    #[arg(long)]
    config: Option<PathBuf>,
    /// 旧数据目录（覆盖 `--repo-root` 的推导结果）
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// 目标 SQLite 文件
    #[arg(long, default_value = "data/mcloud.db")]
    db: PathBuf,
    /// 只预演：跑通身份映射与全部引用诊断并报出问题，**不写任何文件**
    #[arg(long)]
    dry_run: bool,
    /// 旧数据原样归档到这个目录（预演时不归档）
    #[arg(long, default_value = "data/legacy-source")]
    backup_dir: PathBuf,
    /// 以 JSON 输出报告
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Fixture { action } => match action {
            FixtureAction::Extract { har, out, dry_run } => {
                let report = fixture_extract::extract(&har, &out, dry_run)?;
                print!("{}", fixture_extract::render_report(&report, dry_run));

                if !report.leaks.is_empty() {
                    // Fail closed：脱敏有残留就当作运行错误，不静默放过。
                    anyhow::bail!(
                        "脱敏自检未通过，已拒绝写出（{} 处残留）。请检查 sanitize 黑名单。",
                        report.leaks.len()
                    );
                }
                Ok(())
            }
        },

        Command::Migrate(args) => {
            let mut plan = mcloud_server::migration::MigrationPlan::read_from_repo_root(
                &args.repo_root,
                args.db.clone(),
                args.backup_dir.clone(),
            );
            // 显式给的路径覆盖推导结果：有人把 `asign.json` 挪到别处时，
            // 不该被迫把整个仓库根指到一个不对的地方。
            if let Some(config) = args.config {
                plan.source.config_path = config;
            }
            if let Some(data_dir) = args.data_dir {
                plan.source.data_dir = data_dir;
            }

            let outcome = mcloud_server::migration::run(&plan, args.dry_run).await?;

            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&mcloud_server::migration::outcome_to_json(
                        &outcome
                    ))?
                );
            } else {
                for line in outcome.human_lines() {
                    println!("{line}");
                }
            }

            // 迁移本身成功（退出码 0）；需要人工确认的问题只是**提示**，
            // 不能因此判失败 —— 那会让脚本在"有 3 条历史记录找不到账号"时误以为迁移崩了。
            Ok(())
        }

        Command::Capabilities { json } => {
            let payload = mcloud_server::capabilities_payload();
            if json {
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }

            println!(
                "能力注册表（{} 项）\n",
                payload["all"].as_array().map_or(0, Vec::len)
            );
            for cap in payload["all"].as_array().expect("payload 必有 all 数组") {
                let code = cap["code"].as_str().unwrap_or("?");
                let title = cap["title"].as_str().unwrap_or("?");
                let group = cap["group"].as_str().unwrap_or("?");
                let risk = cap["risk"].as_str().unwrap_or("?");
                let hidden = cap["hidden"].as_bool().unwrap_or(false);

                print!("  {code:<16} {title}  [{group} · risk={risk}]");
                if hidden {
                    print!("  (独立页面)");
                }
                println!();
            }

            // 渲染成 `sign, cloud-snapshot`，而不是 Debug 形式的 `[String("sign"), ...]`
            let visible = payload["visible"].as_array().cloned().unwrap_or_default();
            let names: Vec<&str> = visible.iter().filter_map(|v| v.as_str()).collect();
            println!("\n通用任务列表可见: {}", names.join(", "));
            Ok(())
        }

        Command::Version { json } => {
            let payload = mcloud_server::version_payload();
            if json {
                // 与 `GET /api/version` 的响应体逐字一致，脚本可直接复用
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!(
                    "{} {} (时区 {})",
                    payload["name"]
                        .as_str()
                        .unwrap_or(mcloud_core::PRODUCT_NAME),
                    payload["version"].as_str().unwrap_or("?"),
                    payload["timezone"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
    }
}
