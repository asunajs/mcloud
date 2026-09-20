// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::account::*;
use crate::args::*;
use crate::signin_one::signin_one;
use crate::support::*;
use anyhow::{Result, bail};
use mcloud_app::device;
use mcloud_core::{AccountId, Scope, StateStore, account_label};
use mcloud_io::{SqliteStore, SystemRandom};

// ═══════════════════════════════════════════════════════════════════════════
// 签到（云朵中心）
// ═══════════════════════════════════════════════════════════════════════════

/// 签到。**用 Rust 直连上游** —— 这条链路不经过浏览器、不经过 WebView。
///
/// 链路：换 SSO token → 取服务端时间戳 → 签名建会话 → 查状态 → 签到 → 回查。
/// 判据（`infoV3` 说了算，不看 `todaySignIn`）与三条坑见 `mcloud_app::signin`。
///
/// # 一次跑一个还是全部
///
/// 不给 `--account` / `--phone` 就是**全部启用的账号** —— 这也是不带子命令直接
/// `mcloud` 时的行为（对齐旧项目"直接运行、一个参数都不用给"）。
///
/// # 一个账号失败不影响其余
///
/// 旧项目对每个账号各自 try/catch，一个号挂了不会让其余账号今天签不上。
/// 这里照做：逐个跑、失败记在 stderr、最后用退出码统一报"有几个失败"。
pub(crate) async fn signin_run(args: SigninArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let scope = scope_for_new_accounts(&owner);

    if args.list {
        return list_accounts_with_index(&store, scope).await;
    }

    let targets = select_signin_targets(&store, scope, &args).await?;
    if targets.is_empty() {
        // 空库不是错误（用户可能刚 init），但**必须说清楚下一步做什么** ——
        // 直接"跑完、什么都没打"会让人以为签到成功了。
        println!("库里还没有账号。先登录一个：`mcloud login sms --client app`");
        return Ok(());
    }

    // 设备形态在**整次运行里只解析一次**：同一个安装就该是同一台设备。
    // 放进循环里会让每个账号拿到不同的 `device_short`（不传 `--device-short` 时
    // 现生成），于是"一次运行"对应了好几种设备形态 —— 与登录时的形态也对不上。
    let mut rng = SystemRandom;
    let device_id =
        device::load_or_create_app_device_id(&args.app_device_file, &mut rng).map_err(core_err)?;
    let device_short = match &args.device_short {
        Some(short) => short.to_ascii_uppercase(),
        None => device::new_app_device_short(&mut rng),
    };

    let multi = targets.len() > 1;
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut failed = 0usize;

    for target in &targets {
        if multi {
            // 进度行：`--json` 时走 stderr，理由同 `signin_one` 里那段（stdout 只放结果）。
            let line = format!(
                "------ 账号 {} ({}) ------",
                target.index, target.account_id
            );
            if args.json {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        match signin_one(
            &store,
            scope,
            target,
            &device_id,
            &device_short,
            &args,
            &mut rng,
        )
        .await
        {
            Ok(Some(value)) => results.push(value),
            Ok(None) => {}
            Err(err) => {
                failed += 1;
                eprintln!(
                    "✗ 账号 {} ({}) 失败：{err:#}",
                    target.index, target.account_id
                );
                if args.json {
                    results.push(serde_json::json!({
                        "account": target.account_id.as_str(),
                        "index": target.index,
                        "ok": false,
                        "error": format!("{err:#}"),
                    }));
                }
            }
        }
    }

    if args.json {
        // 单账号给对象、多账号给数组。这条规则是**刻意**的：脚本里最常见的是
        // "盯一个账号"，而给那种用法套一层数组，等于逼所有老脚本改一遍。
        // 多账号时如果不套数组，多个对象的字段会互相盖掉 —— 那才是真危险。
        let value = if results.len() == 1 {
            results.remove(0)
        } else {
            serde_json::Value::Array(results)
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    if failed > 0 {
        // 退出码要非零：定时任务靠它判断"这次到底有没有全部成功"。
        // 只打日志的话，一个静默失败的账号可以连着挂一个月没人知道。
        anyhow::bail!("{failed} 个账号失败（本次共 {} 个）", targets.len());
    }
    Ok(())
}

/// 一次签到要跑的一个账号。
#[derive(Debug)]
pub(crate) struct SigninTarget {
    /// 展示用下标（从 1 开始，与 [`list_accounts_with_index`] 打出来的一致）。
    ///
    /// `--phone` 定位的账号**不在**启用列表里也能跑，那时下标没有意义，记 0。
    pub(crate) index: usize,
    pub(crate) account_id: AccountId,
    /// `--phone` 给的那个完整号码（老账号专用）。`Some` 时**强制**走现拼那条路。
    pub(crate) phone_override: Option<String>,
}

/// 打印账号清单（含下标）。
///
/// 存在的理由只有一个：`--account` 收的是**下标**，而"下标 3 是哪个账号"必须
/// 有个地方能查到。顺序与 [`mcloud_core::StateStore::list_accounts`] 一致
/// （`sort_order ASC, id ASC`），所以它是稳定的。
pub(crate) async fn list_accounts_with_index(store: &SqliteStore, scope: Scope<'_>) -> Result<()> {
    let accounts = store.list_accounts(scope).await.map_err(core_err)?;
    if accounts.is_empty() {
        println!("库里还没有账号。先登录一个：`mcloud login sms --client app`");
        return Ok(());
    }
    println!("下标  账号          昵称            登录端   状态");
    for (position, account) in accounts.iter().enumerate() {
        println!(
            "{:>4}  {:<12}  {:<14}  {:<6}  {}",
            position + 1,
            account.id.as_str(),
            account.nickname.as_deref().unwrap_or("-"),
            account.platform.as_deref().unwrap_or("-"),
            if account.enabled { "启用" } else { "停用" },
        );
    }
    println!("\n不带 `--account` 直接跑，就是对**全部启用**的账号各签一次。");
    Ok(())
}

/// 决定这次跑哪些账号。
///
/// 三条路，优先级从高到低：
/// 1. `--phone`：老账号的逃生口，直接用号码推 id（与登录落库同源）；
/// 2. `--account N`：按下标取**第 N 个账号**（含停用的 —— 显式指定就照做）；
/// 3. 都不给：全部**启用**的账号。
///
/// 第 2 条把停用的账号也算进去，是为了让"下标 = 列表里的位置"这条规则不带例外：
/// 一旦下标随启用状态变动，同一个数字在不同时刻会指到不同账号上。
pub(crate) async fn select_signin_targets(
    store: &SqliteStore,
    scope: Scope<'_>,
    args: &SigninArgs,
) -> Result<Vec<SigninTarget>> {
    // `--phone` 那条路用 `account_label` 推 id，与登录落库**同源**，
    // 所以"登录时存进去的"和"签到时要读的"必然是同一行。
    if let Some(phone) = &args.phone {
        return Ok(vec![SigninTarget {
            index: 0,
            account_id: AccountId::new(account_label(None, phone)),
            phone_override: Some(phone.clone()),
        }]);
    }

    let accounts = store.list_accounts(scope).await.map_err(core_err)?;

    if let Some(wanted) = args.account {
        // 下标从 1 开始：`--account 0` 一定是搞错了，早点说清楚
        if wanted == 0 {
            bail!("`--account` 的下标从 1 开始（`--account 1` 是第一个账号）");
        }
        let account = accounts.get(wanted - 1).ok_or_else(|| {
            anyhow::anyhow!(
                "下标 {wanted} 超出范围：库里一共 {} 个账号（用 `mcloud signin --list` 看清单）",
                accounts.len()
            )
        })?;
        if !account.enabled {
            // 显式指定就照做，但要提醒一句 —— 否则"我明明把它停用了、怎么还在跑"
            // 会变成一次没头没尾的排查。
            eprintln!(
                "⚠ 账号 {wanted} ({}) 是停用状态，本次仍然按你指定的跑。",
                account.id
            );
        }
        return Ok(vec![SigninTarget {
            index: wanted,
            account_id: account.id.clone(),
            phone_override: None,
        }]);
    }

    Ok(accounts
        .into_iter()
        .enumerate()
        .filter(|(_, account)| account.enabled)
        .map(|(position, account)| SigninTarget {
            index: position + 1,
            account_id: account.id,
            phone_override: None,
        })
        .collect())
}
