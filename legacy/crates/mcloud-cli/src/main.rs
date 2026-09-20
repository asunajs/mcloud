//! mcloud CLI。
//!
//! # 与原 CLI 的关系
//!
//! 这是原 `packages/cli` 的替代入口，共享同一套能力注册表（`mcloud-capabilities`），
//! 因此 WebUI 与 CLI 不会出现"两边能力不一致"的问题。
//!
//! # 依赖边界（`scripts/check-deps.sh` 会断言）
//!
//! 本产物**不依赖 `mcloud-server`**：后者是 HTTP 产物，拖进来等于让命令行工具
//! 连带拿到 axum / 路由 / 静态资源那一整套。两个产物共用的是 `mcloud-app`（组装层）。
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
//!
//! # 凭据纪律
//!
//! 口令**只从标准输入读**（且不回显），不接受 `--password` 参数 ——
//! 命令行参数会进入 shell 历史与 `ps` 输出，那是凭据泄漏最常见的一条路。

mod fixture_extract;
//
// ── 模块划分 ────────────────────────────────────────────────
// 本文件只保留 CLI 的入口与分发；参数定义与各子命令的实现按职责分文件。
mod account;
mod args;
mod cli_args;
mod login_cmd;
mod signin_one;
mod signin_output;
mod signin_run;
mod smid_cmd;
mod support;

use anyhow::Result;
use clap::Parser;
use mcloud_app::{migration, payload};
use std::process::ExitCode;

use crate::account::*;
use crate::cli_args::*;
use crate::login_cmd::{login_qr, login_sms};
use crate::signin_run::signin_run;
use crate::smid_cmd::{smid_refresh, smid_reset, smid_show};
use crate::support::*;

#[tokio::main]
async fn main() -> ExitCode {
    // 不带子命令 = 直接跑签到；补子命令这件事见 [`Cli::into_command`]。
    let command = Cli::parse().into_command();
    match run(command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
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

        Command::User { action } => match action {
            UserAction::Init { name, db } => user_init(&db, &name).await,
            UserAction::List { db } => user_list(&db).await,
        },

        Command::Login { action } => match action {
            LoginAction::Qr(args) => login_qr(args).await,
            LoginAction::Sms(args) => login_sms(args).await,
        },

        Command::Smid { action } => match action {
            SmidAction::Show { db } => smid_show(&db).await,
            SmidAction::Reset {
                db,
                account,
                report,
            } => smid_reset(&db, &account, report).await,
            SmidAction::Refresh { db, account } => smid_refresh(&db, &account).await,
        },

        Command::Signin(args) => signin_run(args).await,

        Command::Migrate(args) => {
            let mut plan = migration::MigrationPlan::read_from_repo_root(
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

            // 导入的账号归"库里的首个用户"；库里还没有用户就保持无主，
            // 等 `mcloud user init` 时认领 —— 所以两种顺序都能得到正确结果。
            let owner = if args.dry_run {
                None
            } else {
                preferred_owner(&open_local(&args.db)?)
                    .await
                    .map_err(core_err)?
            };
            if !args.dry_run && owner.is_none() {
                // 无主是**正常中间态**（不是失败），但得说一声 —— 否则用户接下来
                // 在网页端登录会发现"账号一个都没有"，而那时已经想不起来是这一步的事。
                eprintln!(
                    "ℹ 库里还没有用户，导入的账号先保持无主。\
                     跑一次 `mcloud user init` 建管理员时会把它们一并认领。"
                );
            }
            let outcome = migration::run(&plan, args.dry_run, owner.as_ref()).await?;

            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&migration::outcome_to_json(&outcome))?
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
            let payload = payload::capabilities_payload();
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
            let payload = payload::version_payload();
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

// ═══════════════════════════════════════════════════════════════════════════
// 用户
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod unit_tests;
