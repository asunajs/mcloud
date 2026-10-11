//! `auth refresh` —— 续期凭据（命令行入口）；
//! `auth auto-schedule` —— 「凭据自动刷新定时」的开关与预设时间（命令行入口）。
//!
//! # 这一层只做"说给人听"与"读人输进来的东西"
//!
//! 判据、上游调用、窄写落库都在 [`mcloud_app::auth_refresh::AuthRefreshService`]。
//! 本文件**不含业务判定** —— 出现 `expire - now < ...` 就是放错了层。
//! 四个触发面（CLI 单账号 / CLI 批量 / WebUI 按钮 / 服务内调度）
//! 共用同一个 service，于是不会各自漂移。
//!
//! ⚠ 自动定时那一条同理：**算 cron 与 upsert 那条定时**都在
//! `mcloud_app::auth_refresh_auto`（唯一入口 ✓），本文件只解析参数、调它、打印 ✓
//!（与 `cli/live_room.rs` 同一个形状 ✓）。
//!
//! # 退出码
//!
//! - `0` —— 全部成功（含"还没到点，没打上游"）
//! - `1` —— 有账号刷新失败
//! - `2` —— 用法错（`--lead-days` 给了 0 或负数、账号不存在、参数矛盾、
//!   预设时间不是 `HH:MM`）

use anyhow::Context;
use mcloud_app::auth_refresh::RefreshOutcome;
use mcloud_core::AccountId;

use mcloud_app::App;

const EXIT_CONFIG: i32 = 2;

const USAGE: &str = "\
用法：
  mcloud auth refresh <--account <id> | --all> [--lead-days N] [--force]
  mcloud auth auto-schedule                    看当前状态（只读，不写盘 ✓）
  mcloud auth auto-schedule on|off             开/关（会立刻重算一次 ✓）
  mcloud auth auto-schedule at HH:MM           设刷新时间（24 小时制，东八区 ✓）

⚠ 刷新时间是**每个登录用户各一份**（`data/auth-refresh.json` 按 owner 存 ✓）——
但**命令行只作用于本机那一个用户**（= 创建最早的那个 ✓，`mcloud user list` 里
标着「管理员」的那个 ✓），**没有** `--user` / `--owner` ✗：
「选谁」是 **WebUI** 的事（让他登录，在**账号配置页**那张「凭据自动刷新」卡上改 ✓）——
CLI 存在的理由是「无头且无公网的设备」，那种机器上本来就只有一个本机用户 ✓。
⚠ 命令每次都会把**动的是谁**打出来 ✓，所以不会默默落到别人/所有人身上 ✗。
⚠ 到点之后**先走既有判据**（账号设置里的「剩余多少天刷新token」✓）——
不需要刷就跳过，并把原因写进日志 ✓。";

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

// ════════════ `auth auto-schedule`（「凭据自动刷新定时」的命令行入口 ✓）════════════
//
// # 为什么命令行上也要有它（本仓口径：WebUI 能做的 CLI 要能 ✓）
//
// 与 `cli/live_room.rs` 同一条理由：那张卡先在 WebUI 上落了地，
// 而"服务器上没有浏览器"这种场景就只剩手改 `data/schedules.json` 一条路 ✗。
//
// ⚠ **不带选项 = 只读** ✓（走 `read_auth_refresh_state` ✓，与 `GET` 同一口径 ✓）——
// 一条"什么参数都不给"的命令把用户配好的时间/开关重算一遍，是那种
// "手滑一下、状态就变了"的形态 ✗（与 `mcloud push panel` 不带参数 = 看一眼 同口径 ✓）。
//
// # ⚠⚠ 钟点是**每用户一个**，但 CLI **不加 `--user`** ✗
//
// 两条口径叠在一起，别搞混：
//
// 1. **数据层是 per-owner 的** ✓（2026-10-11 第一轮：钟点按 owner 存在
//    `data/auth-refresh.json` ✓）—— 这里**只动那一个人**的钟点与他自己那条定时 ✓；
// 2. **CLI 的视角是"本机那一个用户"** ✓（2026-10-11 第二轮，用户口径：
//    「**多用户是面向 webui 给不同设备的用户使用**，**cli 仅用于本地用户**，
//    在无头且无公网的设备没条件使用 webui 或者仅想使用命令行的情况」✓，
//    见 `AGENTS.md` 的「CLI 与 WebUI 的视角分工」✓）⇒
//    **命令行上不加 `--owner` / `--user`** ✗，「选谁」是 **WebUI** 的事 ✓。
//
// ⇒ 这条命令**一律作用于本机那个用户**（`cli_owner` = `legacy_owner`
//   = 创建最早的那个 ✓，与 `push` / `live-room auto-schedule` 逐字同一条口径 ✓），
//   并把"动的是谁"**每次都打出来** ✓ —— 于是它既不会默默作用到**所有人** ✗、
//   也不会默默作用到**别的用户** ✗。
//
// ⚠ 顺带说明"为什么不加 `--user` 也行"：多用户实例里想改**某个人的**钟点，
// 让他**登录 WebUI 在账号配置页那张卡上改** ✓（那才是"给不同设备的用户使用"的入口 ✓）；
// 而 CLI 存在的理由是"无头且无公网的设备" ✓ —— 那种机器上本来就只有一个本机用户 ✓。

/// `auth auto-schedule [on|off|at <HH:MM>]`
///
/// ⚠ **没有** `--user` / `--owner` ✓（理由见上面那段与 `AGENTS.md` ✓）。
pub async fn auto_schedule(app: &App, args: &[String]) -> anyhow::Result<()> {
    // 与 `mcloud push` / `mcloud live-room` 同一条口径：CLI 是本机管理员工具 ✓，
    // 而这条定时是**每个登录用户一条** ✓ ⇒ 命令行上动的是**本机那个用户**那一份 ✓。
    let owner = crate::cli::push::cli_owner(app).await?;
    // ⚠ 把**名字**查出来（`cli_owner` 只回 id ✓）：下面每一行都要打"动的是谁" ✓ ——
    // 不打的话，用户无从确认这一下落在谁身上 ✓（那正是"默默作用"的形态 ✗）。
    let name = local_user_name(app, &owner).await?;
    let store = &app.handle.store;

    // ⚠⚠ **整条 `argv` 都扫一遍**（不是只看第一个位置 ✓）：`--user` 要是写在 `on`
    // 后面，只看首位就会把它**静默忽略** ✗ —— 用户口径点名了这种形态：
    // 「**静默忽略比报错更糟**」✗（判据在 `cli/push.rs::reject_user_selection` ✓，
    // 与 `push` / `live-room` 共用同一份 ✓）。
    crate::cli::push::reject_user_selection(args)?;

    match args.first().map(String::as_str) {
        // ── 不带选项 = **只读** ✓（= `GET /api/auth/refresh-schedule` 的口径 ✓）
        None => {
            let state = mcloud_app::read_auth_refresh_state(store, &owner).await?;
            print_schedule(&state, &name);
            Ok(())
        }
        // ── 开 / 关（= `PUT {enabled}` ✓，预设时间那一格不动 ✓）
        Some("on") | Some("off") => {
            // ⚠ 多给参数**当场拒** ✗（`on extra` 不能"静默按 on 走" ✓）——
            // 静默吃掉一个用户以为生效了的参数，比报错难查得多 ✗。
            if args.len() > 1 {
                anyhow::bail!(
                    "`on` / `off` 不带别的参数（多给了 {} 个）✗\n\n{USAGE}",
                    args.len() - 1
                );
            }
            let wanted = args[0] == "on";
            let state = mcloud_app::reschedule_auth_refresh(
                store,
                &owner,
                Some(mcloud_app::AuthRefreshPatch {
                    wanted: Some(wanted),
                    at: None,
                }),
            )
            .await?;
            print_schedule(&state, &name);
            Ok(())
        }
        // ── 设预设刷新时间（= `PUT {at}` ✓，开关那一格不动 ✓）
        Some("at") => {
            // ⚠ 同上：`at 03:00 extra` 不能"静默按 03:00 走" ✗（多给一律报错 ✓）。
            if args.len() > 2 {
                anyhow::bail!(
                    "`at` 只跟一个 HH:MM（多给了 {} 个）✗\n\n{USAGE}",
                    args.len() - 2
                );
            }
            let raw = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("要给一个 24 小时制的 HH:MM：\n\n{USAGE}"))?;
            // ⚠ 非法值**当场拒**（不静默兜默认值 ✗）—— 判据只有一处
            //（`mcloud_core::auth_refresh::parse_clock` ✓，路由与存储那边用的是同一个 ✓）。
            if mcloud_core::auth_refresh::parse_clock(raw).is_none() {
                eprintln!("预设刷新时间要是 24 小时制的 HH:MM（例如 03:00），收到的是「{raw}」");
                std::process::exit(EXIT_CONFIG);
            }
            let state = mcloud_app::reschedule_auth_refresh(
                store,
                &owner,
                Some(mcloud_app::AuthRefreshPatch {
                    wanted: None,
                    at: Some(raw.clone()),
                }),
            )
            .await?;
            print_schedule(&state, &name);
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        // ⚠ `--user` / `--owner` / `-u` **不在这里认** —— 整条 `argv` 在上面
        // `reject_user_selection` 里已经扫过并报错 ✓（写在 `on` / `at` 后面也躲不过去 ✓）。
        // 留一格在这里反而会退化成"只认第一个位置"的半格 ✗（见 `cli/push.rs` ✓）。
        Some(other) => anyhow::bail!("未知的 auto-schedule 参数：{other}\n\n{USAGE}"),
    }
}

/// 本机那个用户**叫什么**（`cli_owner` 只回 id ✓）—— 打给用户看"这次动的是谁" ✓。
///
/// ⚠ 查不到就回 id 那串（**不报错**）：能查到用户就一定查得到名字，查不到说明
/// 库里那一刻正在被改写 —— 为一句"显示用的话"把命令打挂不划算 ✓。
async fn local_user_name(app: &App, owner: &mcloud_core::UserId) -> anyhow::Result<String> {
    Ok(app
        .handle
        .store
        .users()
        .await?
        .into_iter()
        .find(|u| &u.id == owner)
        .map(|u| u.name)
        .unwrap_or_else(|| owner.to_string()))
}

/// 把状态打成几行（`GET` / `PUT` 两条路共用一份 ✓ —— 两边各写一遍的话，
/// 命令行看到的与界面看到的迟早不一样 ✓）。
///
/// ⚠ `who` 是**这次动的是谁**（= 本机那个用户 ✓），必须打出来 ——
/// 不打的话，"这一下落到谁身上"对用户就是不可见的 ✗
///（而"默默作用到别人/所有人"正是用户口径里点名不许的 ✓）。
fn print_schedule(state: &mcloud_app::AuthRefreshState, who: &str) {
    println!("用户：{who}（本机用户；只影响他一个人 ✓）");
    println!(
        "凭据自动刷新：{}",
        if state.enabled {
            "已开启"
        } else {
            "已关闭"
        }
    );
    println!("刷新时间：{}（东八区）", state.at);
    match state.next_at.as_deref() {
        Some(at) => println!("下一次触发：{at}"),
        None => println!("下一次触发：算不出来（见下面的说明）"),
    }
    if let Some(note) = &state.note {
        println!();
        println!("⚠ {note}");
    }
    println!();
    println!("（那条定时是 `auto-auth-refresh-<用户id>`，在「定时」页也看得到；");
    println!("  别的用户各有一份自己的刷新时间 —— 改他们那几份请让他在 WebUI 上改。）");
    println!("  到点之后先走既有判据：不需要刷就跳过，并把原因写进日志。）");
}
