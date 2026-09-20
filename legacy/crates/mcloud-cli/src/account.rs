// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::support::*;
use anyhow::{Context, Result, bail};
use mcloud_app::user;
use mcloud_core::{Clock, Error as CoreError, UserId, UserStore};
use mcloud_io::{SqliteStore, SystemClock, SystemRandom};
use std::path::Path;

pub(crate) async fn user_init(db: &Path, name: &str) -> Result<()> {
    let store = SqliteStore::open(db).map_err(core_err)?;

    let password = read_password_twice()?;
    let mut rng = SystemRandom;
    let outcome = user::init_first_admin(&store, name, &password, &mut rng, SystemClock.now())
        .await
        .map_err(core_err)?;

    println!(
        "已创建管理员 {}（角色 {}）",
        outcome.user.name,
        outcome.user.role.as_str()
    );
    if outcome.claimed.total() > 0 {
        // 三笔分开报：升级上来的用户最想知道的是"我的东西还在不在"，
        // 只回一个总数等于把这件事糊成一句话。
        println!(
            "已认领旧数据里原本没有归属的数据：账号 {} 个、定时任务 {} 个、组合 {} 个",
            outcome.claimed.accounts, outcome.claimed.schedules, outcome.claimed.pipelines
        );
    }

    println!("\n接下来：mcloud login qr    # 用手机扫码添加第一个云盘账号");
    Ok(())
}

pub(crate) async fn user_list(db: &Path) -> Result<()> {
    // 只读、且 CLI 持有库文件，所以不需要任何身份（见 UserAction 的说明）
    let store = SqliteStore::open(db).map_err(core_err)?;

    let users = store.list_users().await.map_err(core_err)?;
    if users.is_empty() {
        println!("还没有用户。先跑：mcloud user init");
        return Ok(());
    }
    println!("用户 {} 个：", users.len());
    for user in users {
        println!(
            "  {:<20} {:<6} 建于 {}",
            user.name,
            user.role.as_str(),
            user.created_at.to_rfc3339()
        );
    }
    Ok(())
}

/// 读一行敏感输入（口令 / 验证码 / 滑块答案）。
///
/// # 两条输入路径
///
/// - **有终端**：交给 `rpassword` 从 `/dev/tty` 读，输入不回显；
/// - **没有终端**（管道、CI、脚本）：退回读标准输入 —— 否则自动化就没法初始化了
///   （实测：`echo 'pw' | mcloud user init` 在只有 `prompt_password` 时会报
///   `Device not configured`，因为它在找不需要存在的终端）。
///
/// 两条路都**不接受命令行参数**：参数会进 shell 历史与 `ps` 输出，
/// 那是凭据泄漏最常见的一条路。
pub(crate) fn read_secret(prompt: &str) -> Result<String> {
    use std::io::{BufRead as _, IsTerminal as _};

    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password(prompt).context("读取输入失败");
    }

    eprint!("{prompt}");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("读取标准输入失败")?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// 读两次口令并要求一致。
pub(crate) fn read_password_twice() -> Result<String> {
    let first = read_secret("设置口令: ")?;
    let again = read_secret("再输一次: ")?;
    if first != again {
        bail!("两次输入的口令不一致");
    }
    Ok(first)
}

// ═══════════════════════════════════════════════════════════════════════════
// 登录
// ═══════════════════════════════════════════════════════════════════════════

/// 打开库（本地运维视角，**不要求任何身份**）。
pub(crate) fn open_local(db: &Path) -> Result<SqliteStore> {
    SqliteStore::open(db).map_err(core_err)
}

/// 新建云盘账号时的默认归属：**库里的首个用户**（通常就是那个管理员）。
///
/// # 为什么不干脆不归属
///
/// CLI 不需要"你是谁"，但账号**必须有主人** —— 网页端按隔离规则看不到无主账号，
/// 否则会出现"扫码成功了、网页里却没有这个账号"这种最难解释的现象。
///
/// 所以这里只是**给新账号挑一个默认主人**，不是权限检查：
/// 库里还没有用户时返回 `None`，账号保持无主，等 `mcloud user init` 时被认领
/// （见 `StateStore::claim_unowned_accounts`）。
pub(crate) async fn preferred_owner(store: &SqliteStore) -> Result<Option<UserId>, CoreError> {
    Ok(store
        .list_users()
        .await?
        .into_iter()
        .next()
        .map(|user| user.id))
}

/// 把"默认归属"变成一个范围：有主可选就归它，否则本地范围（无主）。
pub(crate) fn scope_for_new_accounts(owner: &Option<UserId>) -> mcloud_core::Scope<'_> {
    match owner {
        Some(id) => mcloud_core::Scope::user(id),
        None => mcloud_core::Scope::Local,
    }
}
