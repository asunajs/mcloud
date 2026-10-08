//! `mcloud live-room` —— **直播**相关的命令行入口（今天只有「自动定时」那一支 ✓）。
//!
//! # 为什么要有它（2026-10-08 用户口径）
//!
//! 那张卡先在 WebUI 上落了地（`GET/PUT /api/live-room/auto-schedule` ✓），
//! 但命令行上**没有对应入口** ✗ —— 实测 `mcloud live-room auto-schedule` 只打一句
//! usage（= 这个子命令不存在 ✓）。于是"服务器上没有浏览器"这种场景
//!（用户口径：「没做的看能不能做」✓）就只剩手改 `data/schedules.json` 一条路 ✗。
//!
//! # ⚠ 重算只有一份实现（别在这儿另写一套 ✗）
//!
//! 时刻 = 下一场 + 1 小时、兜底链、upsert 那条定时 —— 全部在
//! [`mcloud_app::live_auto::reschedule_live_auto`] ✓。本模块只做三件事：
//! 解析参数 → 调它 → 打印结果 ✓（与 `cli/push.rs` 同一个形状 ✓）。
//! 在命令行里再拼一遍 cron 表达式的话，"算出来的时刻"就会有两套写法，
//! 迟早分叉 ✓（用户已经见过一次那样的后果：「16:31 还显示 15:00」✗）。
//!
//! # ⚠ 它会**真打上游**（最坏约 10 秒）
//!
//! 重算要按账号去上游现取「下一场」（用户口径：「下一场应该从上游获取啊」✓），
//! 每个账号最多 4 枪、总超时 10 秒（[`mcloud_app::live_auto::UPSTREAM_FETCH_TIMEOUT`] ✓）。
//! ⇒ 写那两条命令**必须先打一行"正在向上游取下一场…"** ✓ ——
//! 不打的话用户看到的是"光标停住十秒"，会以为命令卡死 ✗。
//!
//! # 用法
//!
//! ```text
//! mcloud live-room auto-schedule                     看当前状态（只读，不写 ✓）
//! mcloud live-room auto-schedule on|off              开/关（会重算一次 ✓）
//! mcloud live-room auto-schedule accounts <id,…>     设作用账号（会重算一次 ✓）
//! mcloud live-room auto-schedule accounts -|all      清空 = 全部已启用账号
//! ```
//!
//! ⚠ **不带选项 = 只读** ✓（走 `read_auto_state` ✓，与 `GET` 同一口径 ✓）——
//! 一条"什么参数都不给"的命令把用户配好的账号/开关重算一遍，是那种
//! "手滑一下、状态就变了"的形态 ✗（与 `mcloud push panel` 不带参数 = 看一眼
//! 同一条口径 ✓）。

use mcloud_app::live_auto::{read_auto_state, reschedule_live_auto, AutoLivePatch, AutoLiveState};
use mcloud_app::App;
use mcloud_core::AccountId;

use crate::cli::push::cli_owner;

const USAGE: &str = "\
用法：
  mcloud live-room auto-schedule                   看当前状态（只读，不写盘 ✓）
  mcloud live-room auto-schedule on                开（并**立刻重算**一次）
  mcloud live-room auto-schedule off               关（并**立刻重算**一次）
  mcloud live-room auto-schedule accounts <id,…>   设作用账号（并**立刻重算**一次）
  mcloud live-room auto-schedule accounts -|all    清空 = 全部已启用账号

⚠ 重算会**去上游现取**「下一场」（每个账号最多约 10 秒，并发）⇒ 写那几条会等一会儿。
⚠ 算不出下一场 ⇒ 那条定时**停用但保留**（账号多选与你的开关都留着，下次算出来自动启用 ✓）。";

/// 跑一条 `live-room` 命令。`args` 是**去掉 `live-room` 之后**的参数 ✓。
pub async fn run(app: &App, args: &[String]) -> anyhow::Result<()> {
    let outcome = match args.first().map(String::as_str) {
        Some("auto-schedule") => auto_schedule(app, &args[1..]).await,
        Some("help" | "--help" | "-h") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => anyhow::bail!("未知的 live-room 子命令：{other}\n\n{USAGE}"),
    };
    // ⚠ **收尾 flush**（与 `cli/push.rs` / `cli/run.rs` 同一条口径 ✓）：
    // 本命令族今天不触发投递 ✓，但重算是 spawn 出去的（服务那条路 ✓）——
    // 将来这里若加"顺手跑一次直播任务"，漏掉这句就是"点了没反应、也不报错" ✗。
    app.handle.pushes.flush().await;
    outcome
}

async fn auto_schedule(app: &App, args: &[String]) -> anyhow::Result<()> {
    // 与 `mcloud push` 同一条口径：CLI 是本机管理员工具 ✓，
    // 而「自动定时」是**每个登录用户一条** ✓ ⇒ 命令行上动的是管理员那一份 ✓。
    let owner = cli_owner(app).await?;
    let store = &app.handle.store;
    let clock = &app.handle.clock;

    match args.first().map(String::as_str) {
        // ── 不带选项 = **只读** ✓（= `GET /api/live-room/auto-schedule` 的口径 ✓）
        None => {
            let state = read_auto_state(store, clock, &owner).await?;
            print_state(&state);
            Ok(())
        }
        // ── 开 / 关（= `PUT {enabled}` ✓，账号那一格不动 ✓）
        Some("on") | Some("off") => {
            let wanted = args[0] == "on";
            let state = recompute(
                app,
                &owner,
                AutoLivePatch {
                    wanted: Some(wanted),
                    accounts: None,
                },
            )
            .await?;
            print_state(&state);
            Ok(())
        }
        // ── 设作用账号（= `PUT {accounts}` ✓，开关那一格不动 ✓）
        Some("accounts") => {
            let raw = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("要给账号 id 清单（`-` 或 `all` = 清空）：\n\n{USAGE}")
            })?;
            // `-` / `all` = 清空 ⇒ 回到"全部已启用账号"（`Schedule::accounts` 的既有语义 ✓）。
            let accounts = parse_accounts(raw);
            // ⚠ 与 `PUT` 路由**同一条校验**：点名的账号必须是**自己的** ✓。
            // 不校验的话命令行能存下一个跑不起来的账号 id（调度器只按定时的主人过滤 ✓）
            // ⇒ 用户以为配上了 ✗（路由那边就是为了这个才 400 ✓）。
            for account in &accounts {
                if store
                    .account_for(&owner, &AccountId::new(account.clone()))
                    .await?
                    .is_none()
                {
                    anyhow::bail!(
                        "账号不存在或不属于你：{account}（先 `mcloud user list` 看看 ✓）"
                    );
                }
            }
            let state = recompute(
                app,
                &owner,
                AutoLivePatch {
                    wanted: None,
                    accounts: Some(accounts),
                },
            )
            .await?;
            print_state(&state);
            Ok(())
        }
        Some(other) => anyhow::bail!("未知的 auto-schedule 参数：{other}\n\n{USAGE}"),
    }
}

/// 调**唯一那份**重算实现 ✓，并在等它之前把"会等一会儿"说出来 ✓。
async fn recompute(
    app: &App,
    owner: &mcloud_core::UserId,
    patch: AutoLivePatch,
) -> anyhow::Result<AutoLiveState> {
    // ⚠ 这行必须在 await **之前**打：重算会真打上游（最坏约 10 秒 ✓），
    // 不打的话用户看到的是"命令卡死了" ✗（与"点了没反应"是同一类体验 ✓）。
    println!("正在向上游取下一场…（每个账号最多约 10 秒，请稍等）");
    // 用**这个进程**按账号造客户端的工厂（`AppHandle::http_for` ✓）——
    // 与 `PUT` 路由那条路是同一份 ✓（拿的是每个账号自己的票 ✓）。
    Ok(reschedule_live_auto(
        &app.handle.store,
        &app.handle.clock,
        Some(&app.handle.http_for),
        owner,
        Some(patch),
    )
    .await?)
}

/// 把状态打成几行（`GET` / `PUT` 两条路共用一份 ✓ —— 两边各写一遍的话，
/// 同一件事在"看"与"改完"之后会有两种说法 ✗）。
fn print_state(state: &AutoLiveState) {
    println!("开关      {}", if state.enabled { "开" } else { "关" });
    match &state.at {
        // 时刻是**东八区墙钟串**（与上游同形 ✓），原样打 ✓。
        Some(at) => println!("下次触发  {at}（= 下一场 + 1 小时）"),
        // ⚠ **不许留空** ✗：空着的话用户分不清"没算出来"与"命令坏了" ✓
        //（`at` 为 `null` 就是"现在算不出下一场 ⇒ 已停用"这一件事 ✓）。
        None => println!("没有下一场，已停用"),
    }
    println!(
        "作用账号  {}",
        if state.accounts.is_empty() {
            "（全部已启用账号）".to_string()
        } else {
            state.accounts.join("、")
        }
    );
    // 说明只在**说不清**的时候才有（正常有下一场时是 `None` ✓，见 `AutoLiveState` ✓）。
    if let Some(note) = &state.note {
        println!("说明      {note}");
    }
}

/// `accounts` 那个参数 → 账号 id 清单 ✓。
///
/// * `-` / `all` ⇒ **空清单**（= 清空 ⇒ 回到"全部已启用账号" ✓，
///   与 `Schedule::accounts` 的既有语义一致 ✓）；
/// * 逗号分隔、**顺手去掉空白与空项**（`a, b,` 不该多出一个空 id ✓）。
///
/// ⚠ 单独抽出来只为一件事：它是本模块**唯一**的纯函数 ⇒ 能被单测钉住 ✓
///（"清空"那两种写法与"多个 id"是最容易手滑写错的两处 ✓）。
fn parse_accounts(raw: &str) -> Vec<String> {
    if raw == "-" || raw == "all" {
        return Vec::new();
    }
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_accounts;

    /// `-` / `all` = 清空 ✓（这是**唯一**能让用户回到"全部已启用账号"的写法 ✓）。
    #[test]
    fn clear_markers_mean_empty_list() {
        assert!(parse_accounts("-").is_empty());
        assert!(parse_accounts("all").is_empty());
    }

    /// 逗号分隔、去空白、丢空项 ✓ —— `a, b,` 只有两个 id（不是三个 ✓）。
    #[test]
    fn comma_separated_ids_are_trimmed() {
        assert_eq!(parse_accounts("a1"), vec!["a1".to_string()]);
        assert_eq!(
            parse_accounts(" a1 , b2 ,"),
            vec!["a1".to_string(), "b2".to_string()]
        );
        // ⚠ 空串 ⇒ 空清单 ⇒ 会**清空**账号多选（那是 `accounts ""` 的语义 ✓）——
        // 钉住它是为了说明这条不是"没给参数"（没给参数在调用方就被拒了 ✓）。
        assert!(parse_accounts("").is_empty());
    }

    /// `all` 只在**整串**相等时才算清空 ✓（某个账号真叫 `all` 时不至于误清 ✓，
    /// 而 `all,b2` 是"一个叫 all 的账号 + b2" ✓）。
    #[test]
    fn clear_marker_must_be_the_whole_argument() {
        assert_eq!(
            parse_accounts("all,b2"),
            vec!["all".to_string(), "b2".to_string()]
        );
    }
}
