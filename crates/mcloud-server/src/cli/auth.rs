//! `auth refresh` —— 续期凭据（命令行入口）。
//!
//! # 这一层只做"说给人听"与"读人输进来的东西"
//!
//! 判据、上游调用、窄写落库都在 [`mcloud_app::auth_refresh::AuthRefreshService`]。
//! 本文件**不含业务判定** —— 出现 `expire - now < ...` 就是放错了层。
//! 四个触发面（CLI 单账号 / CLI 批量 / WebUI 按钮 / 服务内调度）
//! 共用同一个 service，于是不会各自漂移。
//!
//! # 退出码
//!
//! - `0` —— 全部成功（含"还没到点，没打上游"）
//! - `1` —— 有账号刷新失败
//! - `2` —— 用法错（`--lead-days` 给了 0 或负数、账号不存在、参数矛盾）

use anyhow::Context;
use mcloud_app::auth_refresh::RefreshOutcome;
use mcloud_core::AccountId;

use mcloud_app::App;

const EXIT_CONFIG: i32 = 2;

struct Args {
    account: Option<String>,
    all: bool,
    lead_days: Option<f64>,
    force: bool,
}

fn parse_args(argv: &[String]) -> anyhow::Result<Args> {
    let mut out = Args {
        account: None,
        all: false,
        lead_days: None,
        force: false,
    };
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--all" => out.all = true,
            "--force" => out.force = true,
            "--account" => {
                i += 1;
                out.account = Some(argv.get(i).context("--account 后面要跟账号 id")?.clone());
            }
            "--lead-days" => {
                i += 1;
                let raw = argv.get(i).context("--lead-days 后面要跟天数")?;
                let v: f64 = raw
                    .parse()
                    .with_context(|| format!("--lead-days 不是数字：{raw}"))?;
                // ⚠ 0 或负数 ⇒ **用法错**，不静默当默认值。
                //
                // 想「到期当天才刷」的人写 0，静默变账号配置的天数恰好是他想避开的含义。
                if v <= 0.0 {
                    eprintln!("--lead-days 必须为正数（给的是 {v}）");
                    eprintln!("想「到期当天才刷」请给一个很小的正数，比如 0.001；");
                    eprintln!("想无条件刷新请用 --force。");
                    std::process::exit(EXIT_CONFIG);
                }
                out.lead_days = Some(v);
            }
            other => {
                eprintln!("未知参数：{other}");
                print_usage();
                std::process::exit(EXIT_CONFIG);
            }
        }
        i += 1;
    }
    Ok(out)
}

fn print_usage() {
    eprintln!();
    eprintln!("用法：auth refresh <--account <id> | --all> [--lead-days N] [--force]");
    eprintln!();
    eprintln!("  --account <id>   只刷这一个账号");
    eprintln!("  --all            刷所有账号");
    eprintln!(
        "  --lead-days N    剩余不足 N 天时刷新（不给就用账号配置「剩余多少天刷新token」，账号默认 10 天；必须为正）"
    );
    eprintln!("  --force          到不到点都刷（越过 --lead-days，越不过「需人工」那一档）");
    eprintln!();
    eprintln!("（本项目暂无账号级启停，所以 --all 就是全部账号。）");
}

/// `auth refresh ...`
pub async fn refresh(app: &App, argv: &[String]) -> anyhow::Result<()> {
    let args = parse_args(argv)?;

    // 两个都给等于没说清刷谁。不默认「全部」—— 那是个会改凭据的动作。
    if args.account.is_none() && !args.all {
        eprintln!("要给 --account <id> 或 --all（不默认「全部」——那是个会改凭据的动作）");
        print_usage();
        std::process::exit(EXIT_CONFIG);
    }
    if args.account.is_some() && args.all {
        eprintln!("--account 与 --all 不能同时给");
        std::process::exit(EXIT_CONFIG);
    }

    let all = app.handle.store.accounts().await?;

    let targets: Vec<mcloud_core::Account> = if let Some(id) = &args.account {
        let want = AccountId::new(id.clone());
        match all.iter().find(|a| a.id == want) {
            Some(a) => vec![a.clone()],
            None => {
                eprintln!("账号不存在：{id}");
                print_accounts(&all);
                std::process::exit(EXIT_CONFIG);
            }
        }
    } else {
        // ⚠ 本项目**没有**账号级启用/停用（见 `AuthRefreshService::refresh_all`
        // 的说明）。上游参考实现会在这里过滤掉停用账号；我们没有那个概念，
        // 所以不过滤 —— 而不是编一个恒为 `true` 的字段来"看起来在过滤"。
        all.clone()
    };

    if targets.is_empty() {
        println!("没有需要刷新的账号");
        return Ok(());
    }

    let mut failed = 0usize;
    let mut rotated = 0usize;

    for acct in &targets {
        match app
            .handle
            .auth_refresh
            .refresh(acct, args.lead_days, args.force)
            .await
        {
            Ok(RefreshOutcome::Rotated {
                new_expire_ms,
                forced,
            }) => {
                rotated += 1;
                if forced {
                    // 同一句「已换新凭据」不区分这两件事，批量跑完人会
                    // 以为提前量生效过 —— 所以强制那一支单独说。
                    println!(
                        "[{}] 已按 --force 换新凭据（新过期 {}）",
                        acct.id,
                        fmt_ms(new_expire_ms)
                    );
                } else {
                    println!(
                        "[{}] 已换新凭据（新过期 {}）",
                        acct.id,
                        fmt_ms(new_expire_ms)
                    );
                }
            }
            Ok(RefreshOutcome::Skipped { remaining_days }) => {
                println!(
                    "[{}] 未到提前量（还剩 {remaining_days:.1} 天），未打上游",
                    acct.id
                );
            }
            Ok(RefreshOutcome::ManualOnly { expire_ms }) => {
                println!(
                    "[{}] 需人工：库里没有有效的过期时刻（expire={expire_ms}）。请重新登录该账号。",
                    acct.id
                );
                failed += 1;
            }
            Err(e) => {
                // 上游失败时库里**原样不动**（窄写只在拿到新串之后跑）
                failed += 1;
                eprintln!("[{}] 刷新失败：{e}", acct.id);
            }
        }
    }

    println!();
    println!(
        "完成：{rotated} 个已换，{failed} 个失败（共 {} 个）",
        targets.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn fmt_ms(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => ms.to_string(),
    }
}

fn print_accounts(all: &[mcloud_core::Account]) {
    if all.is_empty() {
        eprintln!();
        eprintln!("实例里还没有账号。在 WebUI 的「账号」页添加。");
        return;
    }
    eprintln!();
    eprintln!("现有账号（{} 个）：", all.len());
    for a in all {
        let name = a.nickname.as_deref().unwrap_or("");
        eprintln!("  {:<20} {:<12}", a.id, name);
    }
}
