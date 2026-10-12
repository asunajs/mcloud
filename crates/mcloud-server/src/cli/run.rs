//! CLI 子命令的**分发**（`mcloud <命令>` 那条路）。
//!
//! # 为什么在这里，而不是 `src/bin/`
//!
//! 2026-10-03 用户口径：「**合并为一个，以后就叫 mcloud**」——
//! 原来 `mcloud-server` 与 `mcloud-cli` 是两个 bin（同一个 crate、同一张依赖表），
//! 实测合并只多 **116 KB**，却省掉一整个 3.12 MB 的二进制，而且交付形态变成
//! "拷一个二进制就能跑" ✓。于是入口只剩一个：`src/main.rs` 按 argv 分发，
//! **不带子命令 = 起服务、带子命令 = 走这里**。
//!
//! 分发逻辑放在库里（`src/cli/`）而不是入口里，理由与原来 `src/cli.rs` 文件头写的一样：
//! 库里才能被 `cargo test` 直接测到，而且 `src/bin/` 下每个 `.rs` 都会变成一个独立二进制。
//!
//! ⚠ 日志**不在这里**初始化：默认过滤值两条路不一样（服务要 `mcloud=debug`、
//! CLI 只要 `info`，否则 debug 日志会把命令输出淹掉）⇒ 由 `src/main.rs` 按模式选。

use anyhow::Context;

use crate::cli::push as cli_push;
use crate::cli::{
    auth as cli_auth, config as cli_config, cron as cli_cron, daily as cli_daily,
    live_room as cli_live_room, login as cli_login, pipeline as cli_pipeline,
};

/// 起服务那条路的**显式写法**：`mcloud serve` **等价于**不带参数的 `mcloud` ✓。
///
/// 2026-10-11 用户口径：「**`mcloud serve` 为 webui 命令，简写为 `mcloud`**」✓ ——
/// 两条是**同一个命令**，不是"另一条路"：`serve` 只是那个词的显式版本 ✓。
pub const SERVE: &str = "serve";

/// 入口（`src/main.rs`）那条分流的**唯一**一份判据 ✓。
///
/// # 为什么放在库里
///
/// 分流的形状是"行为契约"（`serve` 与不带参数必须**永远**走同一条路 ✓），
/// 而放在 `src/main.rs` 里就没法被 `cargo test` 直接钉住（bin 的测试要另起进程 ✗）。
/// 这里出一份纯函数 ⇒ 单测能逐条断言"谁走哪条路"✓。
#[derive(Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// 起 HTTP 服务 —— **不带参数**，或显式写 `serve`（两者同一条路 ✓）。
    Serve,
    /// 跑 CLI 命令（`serve` 那个词若写了，这里已经摘掉 ✓）。
    Cli(Vec<String>),
    /// `serve` 后面跟了不认识的东西（且不是帮助开关）⇒ 报错退 2 ✓。
    BadServeArg(String),
}

/// 把 argv（**去掉程序名之后** ✓）判成"起服务 / 跑 CLI"。
///
/// ⚠ 只在**这一处**判：`src/main.rs` 拿它选日志默认值与入口函数，
/// **不再自己写一遍** `args.is_empty()` ✗ —— 两份判据迟早分叉 ✓。
pub fn dispatch(args: Vec<String>) -> Dispatch {
    // **不带参数 = 起服务**（2026-10-03 起的老口径，一个字没改 ✓）
    if args.is_empty() {
        return Dispatch::Serve;
    }
    // ⚠ `run` 也认帮助开关（2026-10-11，与 `serve --help` **同一款写法** ✓）：
    //
    // `mcloud run` 光一个词 = **立即跑默认每日任务**（见 `cli/daily.rs` ✓），
    // 而 `run --help` 问的是"这条命令怎么用" ⇒ 收敛成**看帮助** ✓
    //（= `mcloud --help` 那一份文本、打 **stdout**、退 **0** ✓）。
    //
    // 改前它落进**带参数那条老路**：`--help` 被当成任务名 ⇒ 报"用法错"退 1 ✗
    // —— 问一句"怎么用"却被训一顿，正是用户点名的「用法简化」要抹平的那种小坑 ✓。
    //
    // ⚠ 只在 `args.len() == 2` 时认：`mcloud run <任务名> <账号>` 里那两个位置
    // 一个都不能被吃掉 ✓（万一将来真有个任务叫 `help`，三参数那种写法照旧走老路 ✓）。
    if args[0] == "run" && args.len() == 2 {
        if let Some(flag @ ("help" | "--help" | "-h")) = args.get(1).map(String::as_str) {
            return Dispatch::Cli(vec![flag.to_string()]);
        }
    }
    // `mcloud config --help` 同理（2026-10-12，与上面 `run --help` 逐格同款）：
    // `config` 光一个词 = 打用法，而 `config --help` 问的也是"这条命令怎么用"
    // ⇒ 收敛成**看帮助**（= `mcloud --help` 那一份文本、打 stdout、退 0）。
    // 不收的话 `--help` 会落进子命令解析里被当成"不认识的参数"而报用法错。
    // 只认 `args.len() == 2`：`config export --out x` 那些位置一个都不能被吃掉。
    if args[0] == "config" && args.len() == 2 {
        if let Some(flag @ ("help" | "--help" | "-h")) = args.get(1).map(String::as_str) {
            return Dispatch::Cli(vec![flag.to_string()]);
        }
    }
    if args[0] != SERVE {
        return Dispatch::Cli(args);
    }
    match args.get(1).map(String::as_str) {
        // 光一个 `serve` ⇒ 就是"不带参数"那条路 ✓（默认行为一个字没改 ✓）
        None => Dispatch::Serve,
        // `serve --help` / `serve help` ⇒ 看帮助：与 `mcloud --help` 是**同一份**文本 ✓
        Some(flag @ ("help" | "--help" | "-h")) => Dispatch::Cli(vec![flag.to_string()]),
        // 其余一律**报错**，不静默忽略 ✗（口径与 `cli/push.rs` 的
        // `reject_user_selection` 同款：静默忽略比报错更糟 ✓）
        Some(other) => Dispatch::BadServeArg(other.to_string()),
    }
}

/// `serve` 后面跟了别的东西：**报错 + 用法 + 退 2** ✓。
pub fn bad_serve_arg(arg: &str) -> anyhow::Result<()> {
    eprintln!("`serve` 不接受参数：{arg}");
    eprintln!("它等价于**不带参数**跑 `mcloud`（= 起 WebUI 服务 ✓）。");
    eprintln!("要跑 CLI 命令就直接写 `mcloud <命令>` ✓（`mcloud --help` 看清单 ✓）。");
    print_usage();
    std::process::exit(2)
}

/// 跑一条 CLI 命令。`args` 是**去掉程序名之后**的参数（`src/main.rs` 负责切）。
///
/// 空 `args` 不会走到这里 —— 那是"起服务"，由 `src/main.rs` 按 [`dispatch`] 分流
/// （这里仍然保留 `None => print_usage()` 那一支：它防的是"将来有人从别处
/// 调进来"这种形状，不是给 `mcloud` 无参数用的）。
pub async fn run(args: &[String]) -> anyhow::Result<()> {
    // ⚠ **在组装应用之前**处理帮助：`mcloud --help` 不该去建/读数据目录 ✗
    // （`AppBuilder::build` + `seed_run_ids` 会碰盘，而"看帮助"是一次纯输出 ✓）。
    // `mcloud help` / `-h` / `serve --help` 都由 [`dispatch`] 收敛到这里 ✓。
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        print!("{USAGE}");
        return Ok(());
    }

    let data_dir = std::env::var("MCLOUD_DATA_DIR").unwrap_or_else(|_| "data".to_string());

    let app = mcloud_app::AppBuilder::new()
        .data_dir(&data_dir)
        .build()
        .context("组装应用失败")?;

    // ⚠⚠ **必须在跑任何任务之前**：把 run id 续到历史最大值之后。
    //
    // 2026-10-02 实测踩到：这里原先没有这一句 ✗ ⇒ CLI 的 run id 从 **1** 重来 ✗
    // ⇒ 一次 `mcloud run live-room` 就把历史里 `run_id=1` 那条**签到记录覆盖掉了** ✗
    // （`json_store` 的警告原话：「运行编号被复用：这次要覆盖的是**另一次**运行…
    //   run=1 old_task=sign new_task=live-room」✓）。而且 CLI 与服务**同时在跑**时，
    // 两边各持一份内存状态 ⇒ 覆盖方向还取决于谁后写盘 ✓。
    //
    // 服务那条路早就有这句（`mcloud-server/src/main.rs`，注释同一条理由：
    // "不做的话重启后编号从 1 重来…症状：历史越重启越少"）✓ —— CLI 漏了 ✓。
    app.handle.seed_run_ids().await?;

    match args.first().map(String::as_str) {
        // 不给参数时**打印用法**，而不是列任务。
        //
        // 列任务的写法在只有 `tasks`/`run` 两个命令时还算合理；
        // 现在有 7 个命令（tasks/pending/login/cron/pipeline/auth/run —— 现算：
        // `grep -n '^        Some("' crates/mcloud-server/src/bin/cli.rs`），
        // "不带参数"多半是**不知道有什么**，这时该给的是命令清单。
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
        // ⚠ 2026-10-11 用户裁定：**`run-all` 已删** ✗ —— 它原先在这里。
        //
        // 删它的理由（用户原话）：它**是多余的** ✗ —— 与 `mcloud run`（**不带参数**）
        // **逐格相同**（同一条链、同一份判据 ✓、都只跑**本机管理员名下的全部账号** ✓），
        // 而 `AGENTS.md` 的「补充裁定 2」写着 **CLI 要简单** ✓。
        // ⇒ 一键跑「系统默认每日任务」只剩上面 `Some("run")` 那一支 ✓
        //（光一个 `run` = 不带参数那条 ✓，见 `cli/daily.rs::run_default` ✓）。
        // 现在它落进下面 `Some(other)` 那一支：**未知子命令 + 用法 + 退 2** ✓。
        //
        // 推送规则（2026-10-07 用户口径「想想 cli 上该怎么做，记得写的时候兼容」✓）：
        // **新加的一条子命令** ⇒ 老命令/老脚本一个字都不用改 ✓（见 `cli/push.rs` 头部的兼容清单 ✓）。
        Some("push") => {
            cli_push::run(&app, &args[1..]).await?;
        }
        // 直播（目前只有「自动定时」那一支 ✓）—— 与 `push` 同一条形状：
        // **新加的一条子命令** ⇒ 老命令/老脚本一个字都不用改 ✓。
        // ⚠ 它是**本机管理员工具**（`legacy_owner` ✓）：那张卡是"每个登录用户一条"，
        // 命令行上只能落到管理员那一份 ✓（理由写在 `cli/live_room.rs` 头部 ✓）。
        Some("live-room") => {
            cli_live_room::run(&app, &args[1..]).await?;
        }
        // 配置的**离线搬运**（2026-10-12 用户裁定：加 `config export` / `config import`）：
        // 推送规则、面板绑定、兑换订阅、定时、编排这些"列表型"配置，界面上的入口
        // 全在后端 API 上（WebUI 只是壳）⇒ 没有浏览器 / 只有 SSH / 界面某页点不动的
        // 机器上用户配不了它们。这一对命令给的是一条离线通路：有界面的机器上配好、
        // 导出成一份 JSON、拷到无头机器上导入。
        //
        // 为什么是**一对通用命令**而不是"每个配置项各一条子命令"：`AGENTS.md` 的
        // 「补充裁定 2」写着 CLI 要简单（用户原话「你把 webui 的功能都搬过去，
        // cli 还是人用的吗」）—— 导出/导入不新增任何"配置面板"形状的东西。
        // 细节（导出哪几类、实例级怎么处置、备份与回滚）见 `cli/config.rs` 的模块头。
        Some("config") => {
            cli_config::run(&app, &args[1..]).await?;
        }
        Some("auth") => {
            let rest = &args[1..];
            match rest.first().map(String::as_str) {
                Some("refresh") => cli_auth::refresh(&app, &rest[1..]).await?,
                // 「凭据自动刷新定时」的开关与刷新时间（用户口径 2026-10-11 ✓）。
                // ⚠ 与 `live-room auto-schedule` 同一条形状：**不带选项 = 只读** ✓。
                // ⚠ 刷新时间自 2026-10-11 起是**每用户一个**（数据层 ✓），但这条命令
                // **只作用于本机那一个用户**、**没有** `--user` ✗ —— 理由（用户口径
                // 「cli 仅用于本地用户」✓）写在 `cli/auth.rs` 那段与 `AGENTS.md` ✓。
                Some("auto-schedule") => cli_auth::auto_schedule(&app, &rest[1..]).await?,
                Some(other) => {
                    eprintln!("未知 auth 子命令：{other}");
                    eprintln!("可用：auth refresh <--account <id> | --all>");
                    eprintln!("      auth auto-schedule [on|off|at HH:MM]");
                    std::process::exit(2);
                }
                None => {
                    eprintln!(
                        "用法：auth refresh <--account <id> | --all> [--lead-days 7] [--force]"
                    );
                    eprintln!("      auth auto-schedule [on|off|at HH:MM]");
                    std::process::exit(2);
                }
            }
        }
        Some("run") => {
            // ⚠⚠ **不带参数**是 2026-10-11 **新加**的一条路 ✓。
            //
            // 用户口径原话：「**`mcloud run` 为简写，主要功能是立即执行所有账号的
            // 系统的默认每日任务**」✓ ⇒ `mcloud run`（光一个词）= **立即**把
            // 「系统默认每日任务」跑在**本机管理员名下的全部账号**上 ✓。
            //
            // 判据（哪条链 = "系统默认每日任务"、哪些账号 = "所有账号"）**不在本地发明** ✗：
            // 全在 `cli/daily.rs` 里、且复用既有那一处（`default_pipeline_id(管理员)` +
            // `accounts_for(管理员)` ✓）—— 与那条 `daily-<owner>` 定时跑的是同一条链 ✓。
            //
            // ⚠ 下面那条**带参数**的路（`run <任务名> <账号>`）**一个字都没改** ✓
            //（含它的用法错文案 ✓）—— 这里只是**先**判"光一个 `run`"。
            if args.len() == 1 {
                let outcome = cli_daily::run_default(&app).await?;
                if outcome.code != 0 {
                    std::process::exit(outcome.code);
                }
                return Ok(());
            }
            let task = args
                .get(1)
                .context("用法：mcloud run <任务名> <账号> [--body <JSON>]")?;
            let account = args
                .get(2)
                .context("用法：mcloud run <任务名> <账号> [--body <JSON>]")?;
            let id = mcloud_core::AccountId::new(account);
            let acct = app
                .handle
                .store
                .account(&id)
                .await?
                .with_context(|| format!("账号不存在：{account}"))?;
            // 任务参数：默认空对象；`--body <JSON>` 显式给。
            //
            // ⚠ 这里原先**写死** `{}`，于是所有"要参数才有意义"的任务在 CLI 上
            // 根本没法用 —— 最典型的是 `live-room`：它的 `codes`（手工口令）
            // 是**唯一**的入口（小红书那条口令没法自动获取），传不进参数
            // 就等于这个任务在 CLI 上不存在。WebUI 那条路一直能传（它发 JSON 体），
            // 所以这个缺口在单测里看不出来 —— 只有"真的从 CLI 跑一次"才会暴露。
            let body = match args.iter().position(|a| a == "--body") {
                Some(i) => {
                    let raw = args.get(i + 1).context("--body 后面要给一段 JSON")?;
                    serde_json::from_str(raw)
                        .with_context(|| format!("--body 不是合法 JSON：{raw}"))?
                }
                None => serde_json::json!({}),
            };
            let record = app.runner().run(task, &acct, &body).await?;
            // ⚠ **先把在飞的推送等落地，再决定退出码**（2026-10-07）。
            // 投递是 spawn 出去的 ✓（服务长驻无所谓 ✓），而 **CLI 跑完就退** ✗ ⇒
            // 进程一没，在飞的那条 HTTP 就被掐了 ✗ —— 而"这次失败了"恰恰是最该推的一条 ✓。
            // TS 那份是 `await sendNotify(...)` ✓，这里补回同一个语义 ✓。
            app.handle.pushes.flush().await;
            println!("[{}] {}", record.outcome.summary(), record.task);
            if record.outcome.is_failure() {
                std::process::exit(1);
            }
        }
        // ── 用户管理（本地命令行，**不经过 HTTP**）──────────────────────────
        //
        // # 为什么必须有它（2026-10-06 用户口径：「我需要一个用于测试的账号…该怎么做」）
        //
        // 这个仓此前**没有任何用户管理入口**：`POST /api/setup` 只在 0 用户时可用，
        // 之后既没有"加用户"的路由、CLI 也没有命令 ⇒ 想加第二个用户只能手改
        // `users.json`，而口令那格是 argon2 的 PHC 串（手算不出来）✗。
        //
        // ⚠ 为什么放在**命令行**而不是 HTTP：
        // 它要写 `data/`（口令哈希、归属）—— 那已经意味着"能碰到这台机器的文件系统"了
        // ⇒ 命令行不引入任何**新的远程**攻击面 ✓；而放到 HTTP 上就多一条"谁能建用户"
        // 的鉴权链要设计（"没有这条路"才是今天安全的默认 ✓）。
        //
        // ⚠⚠ 管理员是**算出来的**、不是设出来的：`legacy_owner` 取"创建最早的那个"
        // （`mcloud_core::owner` ✓）⇒ **空实例上建的第一个就是管理员**（等于绕开
        // WebUI 的初始化向导 ✓），之后再加的**永远是普通用户** ✓。
        // 要换管理员得改 `created_at_ms` —— 那是另一件事，不在这儿做 ✓。
        //
        // ⚠⚠ 加完**必须重启服务**：`JsonStore` 是启动时把 `users.json` 全量读进内存的
        // （`load_all` + `RwLock<Db>` ✓），服务那份快照里没有这个用户 ⇒
        // ① 用新用户名登录会 401 ✗；② 更要命的是服务下次写 `users.json`
        // （改口令、老档口令升级）会**拿它那份陈旧快照整份覆盖** ⇒ 刚加的用户消失 ✗。
        // 所以下面把这句话打给用户看 ✓。
        Some("user") => {
            match args.get(1).map(String::as_str) {
                Some("list") => {
                    let users = app.handle.store.users().await?;
                    let admin = mcloud_core::legacy_owner(&users);
                    println!("用户（{} 个）：", users.len());
                    for u in &users {
                        let tag = if Some(&u.id) == admin.as_ref() {
                            "管理员"
                        } else {
                            "普通"
                        };
                        println!("  {:<20} {:<4} id={}", u.name, tag, u.id);
                    }
                }
                Some("add") => {
                    let (Some(name), Some(password)) = (args.get(2), args.get(3)) else {
                        print_user_usage();
                        std::process::exit(2);
                    };
                    // 与 `/api/setup` **同一套**校验（用户名形状 + 口令长度下限）：
                    // 命令行不是"绕过闸"的后门 ✓，两条路建出来的用户必须一样规矩 ✓。
                    mcloud_core::validate_username(name).map_err(anyhow::Error::msg)?;
                    let users = app.handle.store.users().await?;
                    if users.iter().any(|u| &u.name == name) {
                        anyhow::bail!("用户名已存在：{name}（要改口令用 `mcloud user passwd`）");
                    }
                    // ⚠ 长度下限只对**明文**判得了：摘要恒 64 位（与 `/api/setup` 同一条口径 ✓）
                    if !mcloud_crypto::is_client_digest(password) {
                        let settings = app.handle.store.settings().await?;
                        if password.chars().count() < settings.min_password_len {
                            anyhow::bail!("口令太短：至少 {} 位", settings.min_password_len);
                        }
                    }
                    let user = mcloud_core::User {
                        id: mcloud_core::UserId::new(uuid::Uuid::new_v4().to_string()),
                        name: name.clone(),
                        password_hash: mcloud_core::PasswordHash(
                            mcloud_crypto::mark_digest_scheme(&hash_credential(password)?),
                        ),
                        created_at_ms: app.handle.clock.now_ms(),
                    };
                    // ⚠⚠ **空实例上第一个用户也走这条路**（2026-10-06 用户口径：
                    // 「能不能不使用 api 建首个用户，直接用 cli 建立」）⇒
                    // 全新机器可以不碰浏览器、不开服务，先把管理员建出来 ✓。
                    //
                    // 第一个走 `create_first_user`（**原子**："只在空实例上成功" ✓）：
                    // 万一服务正跑着、恰好有人同时点了初始化向导，两条路不会都成功 ✓。
                    let is_first = users.is_empty();
                    if is_first {
                        if !app.handle.store.create_first_user(&user).await? {
                            anyhow::bail!(
                                "实例已经有用户了（刚被别的进程建掉）—— 跑 `mcloud user list` 看一眼"
                            );
                        }
                    } else {
                        app.handle.store.save_user(&user).await?;
                    }
                    println!("✓ 已建用户 {}（id={}）", user.name, user.id);
                    if is_first {
                        println!(
                            "  这是**第一个**用户 ⇒ 按 `legacy_owner` 的口径他就是**管理员** ✓"
                        );
                        println!(
                            "  ⚠ 也就是说：**不用再走 WebUI 的初始化向导**了 —— 服务起来直接登录 ✓"
                        );
                    } else {
                        println!("  这是**普通用户**（管理员 = 创建最早的那个）⇒ 只能看到归自己的账号与日志。");
                    }
                    println!();
                    println!("⚠⚠ 现在**必须重启服务**它才认这个用户：");
                    println!("     scripts/dev-reload.sh --no-build");
                    println!("   不重启的后果有两层：① 用它登录会 401 ✗；");
                    println!(
                        "   ② 更要命 —— 服务下次写 users.json（改口令、老档口令升级）会拿它那份"
                    );
                    println!("      **陈旧快照整份覆盖**，刚加的这个用户就没了 ✗。");
                }
                Some("passwd") => {
                    let (Some(name), Some(password)) = (args.get(2), args.get(3)) else {
                        print_user_usage();
                        std::process::exit(2);
                    };
                    if !mcloud_crypto::is_client_digest(password) {
                        let settings = app.handle.store.settings().await?;
                        if password.chars().count() < settings.min_password_len {
                            anyhow::bail!("口令太短：至少 {} 位", settings.min_password_len);
                        }
                    }
                    let mut users = app.handle.store.users().await?;
                    let Some(user) = users.iter_mut().find(|u| &u.name == name) else {
                        anyhow::bail!("没有这个用户：{name}（用 `mcloud user list` 看有哪些）");
                    };
                    user.password_hash = mcloud_core::PasswordHash(
                        mcloud_crypto::mark_digest_scheme(&hash_credential(password)?),
                    );
                    let user = user.clone();
                    app.handle.store.save_user(&user).await?;
                    println!("✓ 已改 {} 的口令（id={}）", user.name, user.id);
                    println!("⚠ 同样要重启服务才生效（理由见 `mcloud user add` 的输出）");
                    println!("⚠ 已登录的会话**不会**因此失效 —— 要踢掉就重启服务。");
                }
                _ => {
                    print_user_usage();
                    std::process::exit(2);
                }
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

/// 算"存进 `users.json` 的那个哈希"：**与 `/api/setup` 逐字节同一条路** ✓。
///
/// 先取前端那一次的摘要（`SHA-256(明文)` 的小写 hex ✓），再对**摘要**做 argon2，
/// 最后打上档位前缀（`sha256$` ✓）—— 三步都不能换顺序、也不能省中间那步，
/// 否则登录时 `verify_password` 拿到的候选值对不上 ✗。
fn hash_credential(password: &str) -> anyhow::Result<String> {
    let digest = mcloud_crypto::to_digest_credential(password);
    mcloud_crypto::hash_password(&digest).map_err(|e| anyhow::anyhow!("算口令哈希失败：{e}"))
}

fn print_user_usage() {
    eprintln!("用法：");
    eprintln!("  user list                        列出用户（标出谁是管理员）");
    eprintln!("  user add <名字> <口令>            加一个用户（**空实例上第一个 = 管理员**）");
    eprintln!("  user passwd <名字> <新口令>       改某个用户的口令");
    eprintln!();
    eprintln!("⚠ 管理员是**算出来的**（创建最早的那个），不能用命令指定。");
    eprintln!("⚠ 空实例上第一条 `user add` 建的就是管理员 ⇒ **不必再走 WebUI 的初始化向导** ✓");
    eprintln!("  （与 `POST /api/setup` 同一套校验、同一种口令档位 ✓）");
    eprintln!("⚠ 三条都会写 `data/users.json` ⇒ **跑完必须重启服务**它才认：");
    eprintln!("     scripts/dev-reload.sh --no-build");
    eprintln!("  （服务启动时把 users.json 全量读进内存 ⇒ 不重启就看不到新用户，");
    eprintln!("    而且服务下次写这个文件会把刚改的整份覆盖掉 ✗）");
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

/// 顶层用法文本 —— **只有这一份** ✓。
///
/// * `mcloud help` / `mcloud --help` / `-h`（含 `mcloud serve --help`）⇒ 打到 **stdout**、退 0 ✓
///   （见 [`run`] 开头那一支：**在组装应用之前**就返回，不碰数据目录 ✓）；
/// * 不认识的子命令 ⇒ 走 [`print_usage`] 打到 **stderr**、退 2 ✓。
///
/// ⚠ 两处共用这一份：抄成两份迟早分叉，而"帮助在说谎"是最难发现的那种错 ✗。
///
/// # 为什么分成两组（2026-10-11 用户口径）
///
/// 用户原话：「**`mcloud serve` 为 webui 命令，简写为 `mcloud`；`mcloud user` 为 webui 命令**」✓，
/// 而 `AGENTS.md` 的「补充裁定 2」把两个视角写死了：**WebUI = 多用户共享一份后端** ✓、
/// **CLI = 个人 · 本地 · 简单** ✗（不许把 WebUI 的功能整批搬进 CLI ✗）。
/// ⇒ 用法文本里把这两组**分开列** ✓（只分组与标注，**不改**任何子命令的行为/参数 ✓）。
const USAGE: &str = r#"用法：mcloud [<命令> [参数]]

── 服务端 / WebUI 命令（一个实例、多个登录用户各看各的 ✓）────────────────
  serve                           起 WebUI 服务（默认 http://127.0.0.1:3000）
                                  ⚠ **不带参数跑 `mcloud` 就是它** —— 两者等价 ✓
  user list|add|passwd            给实例建账号（空实例上第一条 add = 管理员 ✓；
                                  之后加的都是普通用户 ✓。见 `mcloud user`）

── 个人 CLI（**只作用于本机那一个用户** = 管理员 ✓；没有 `--user`/`--owner` ✓）──
  tasks                          列出可用任务
  run                             不带参数 = **立即**把「系统默认每日任务」跑在
                                  **本机管理员名下的全部账号**上（不是库里全部 ✗）
                                  ⚠ `run --help` / `run help` / `run -h` 打这份清单（退 0 ✓）
  run <任务名> <账号> [--body <JSON>]  带参数 = 只跑一次那个任务（照旧 ✓）
                                 要参数的任务必须用 --body，例如：
                                 run live-room <账号> --body '{"codes":"口令甲 口令乙","listenSeconds":45}'
  pending                         查看本进程里还没清理的网盘产物
  cron <定时id>                   跑一条已配置的定时（给系统 crontab 调用）
  pipeline run <编排id>           跑一条已配置的编排
  push [list|show|add|rm|events|bind]  推送规则（每账号的渠道之上那一层 ✓）
  live-room auto-schedule [on|off|accounts <id,…>]
                                 直播自动定时（**不带参数 = 只看状态** ✓；
                                 写那几条会去上游现取下一场，约 10 秒）
  auth refresh <--account|--all>  刷新凭据
  auth auto-schedule [on|off|at HH:MM]  凭据自动刷新的开关与刷新时间
                                 （刷新时间**每用户一个** ✓；CLI **只作用于本机那个用户** ✓，
                                  选谁请在 WebUI 上改 —— 见 `mcloud auth auto-schedule help`）
  config export [--out <路径>]    导出本机用户的配置 —— **含账号凭据与设备指纹**
                                 （推送规则 / 面板绑定 / 兑换订阅 / 定时 / 编排 / 刷新钟点 /
                                  账号 / 账号状态 / 账号设备指纹 / 本机设备指纹）；
                                  省略 --out 就打 stdout。⚠ 这份文件等于你的登录态：
                                  别乱发、别提交进版本库
                                 （有界面的话，WebUI「设置 → 服务」里点「导出配置」同效）
  config import <路径> [--with-instance]  把那份 JSON **按 id 合并**到本机用户 ——
                                 同 id 覆盖（逐字段以文件为准）、本机没有的新增、
                                 **本机多出来的保留**；文件里没有的类别一个字节都不动
                                 （`--with-instance` 是**另一回事**：实例级那几格是标量，
                                  没有「本机多出来的一条」这回事 ⇒ 加了它才整格覆盖，
                                  不加就一格都不动）
                                 导入后**不用重新登录**就能跑任务
                                 （导入前自动备份成 `<名字>.bak-<时间戳>`，失败回滚；
                                  导入完要 `scripts/dev-reload.sh --no-build` 重启才生效）
  login <方式>                    登录账号（qr / sms / app / password / device）
  help, --help, -h                打印这份清单

定时与编排的**创建**不在命令行：改配置文件，或用 WebUI。
完整的 WebUI（含登录闸、账号管理、日志）在服务进程里：默认 http://127.0.0.1:3000
"#;

fn print_usage() {
    eprint!("{USAGE}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Dispatch {
        Dispatch::Cli(args.iter().map(|s| s.to_string()).collect())
    }

    /// 门：**`serve` 与"不带参数"是同一条路** ✓（2026-10-11 用户口径：
    /// 「`mcloud serve` 为 webui 命令，简写为 `mcloud`」✓）。
    ///
    /// ⚠ 这条断言是**行为契约**：谁要是把 `serve` 改成另起一条路（复制一份启动逻辑），
    /// 这里就红 ✓。
    #[test]
    fn serve_and_no_args_take_the_same_path() {
        assert_eq!(dispatch(Vec::new()), Dispatch::Serve, "不带参数 = 起服务 ✓");
        assert_eq!(
            dispatch(vec!["serve".to_string()]),
            Dispatch::Serve,
            "`serve` = 同一条路 ✓"
        );
        // `serve --help` / `serve help` ⇒ 看帮助（与 `mcloud --help` 同一份文本 ✓）
        assert_eq!(
            dispatch(vec!["serve".to_string(), "--help".to_string()]),
            cli(&["--help"])
        );
        assert_eq!(
            dispatch(vec!["serve".to_string(), "help".to_string()]),
            cli(&["help"])
        );
        // `serve` 后面跟别的东西 ⇒ **报错**，不静默忽略 ✗
        assert_eq!(
            dispatch(vec!["serve".to_string(), "x".to_string()]),
            Dispatch::BadServeArg("x".to_string())
        );
        // 既有子命令一个字都不变 ✓（`serve` 只是**新加**的一格 ✓）
        assert_eq!(dispatch(vec!["tasks".to_string()]), cli(&["tasks"]));
        assert_eq!(
            dispatch(vec!["run".to_string(), "sign".to_string()]),
            cli(&["run", "sign"])
        );
        assert_eq!(dispatch(vec!["--help".to_string()]), cli(&["--help"]));
        assert_eq!(dispatch(vec!["user".to_string()]), cli(&["user"]));
    }

    /// 门：**`mcloud run --help` 是"看帮助"，不是"用法错"** ✓（2026-10-11）。
    ///
    /// 改前它落进**带参数那条老路**（`--help` 被当任务名 ⇒ 报用法错、退 1 ✗）；
    /// 现在与 `serve --help` **同款**收敛成 [`cli`] 的 `--help` 那一格 ⇒
    /// 与 `mcloud --help` **同一份文本**、打 stdout、退 0 ✓（见 [`run`] 开头那一支 ✓）。
    ///
    /// ⚠ 这条断言是**行为契约**：谁要是把 `run --help` 再拉回老路，这里就红 ✓。
    #[test]
    fn run_help_is_help_not_a_usage_error() {
        for flag in ["help", "--help", "-h"] {
            assert_eq!(
                dispatch(vec!["run".to_string(), flag.to_string()]),
                cli(&[flag]),
                "`run {flag}` 要与 `mcloud {flag}` 走**同一条**路 ✓"
            );
        }
        // ⚠ 只认"光一个 `run` + 一个帮助开关"：带参数那条老路**一个字都没被吃掉** ✓
        assert_eq!(dispatch(vec!["run".to_string()]), cli(&["run"]));
        assert_eq!(
            dispatch(vec![
                "run".to_string(),
                "sign".to_string(),
                "13900000000".to_string()
            ]),
            cli(&["run", "sign", "13900000000"]),
            "`run <任务名> <账号>` 照旧走老路 ✓"
        );
        // 三参数时哪怕第二个词是 `help` 也不当帮助（那个位置是**任务名** ✓）
        assert_eq!(
            dispatch(vec![
                "run".to_string(),
                "help".to_string(),
                "13900000000".to_string()
            ]),
            cli(&["run", "help", "13900000000"]),
            "别把 `run <任务名> <账号>` 里那两个位置吃掉 ✓"
        );
    }

    /// 门：帮助文本里**两组都在**，且 `serve` 与 `user` 归在**服务端 / WebUI** 那一组 ✓。
    #[test]
    fn usage_lists_both_groups() {
        let server = USAGE
            .find("服务端 / WebUI 命令")
            .expect("帮助里要有「服务端 / WebUI 命令」那一组 ✓");
        let personal = USAGE
            .find("个人 CLI")
            .expect("帮助里要有「个人 CLI」那一组 ✓");
        assert!(server < personal, "服务端那组要在前面 ✓");

        // 这两条是用户点名的"webui 命令"⇒ 必须落在**服务端 / WebUI** 那一段里 ✓
        for line in ["  serve", "  user list|add|passwd"] {
            let at = USAGE
                .find(line)
                .unwrap_or_else(|| panic!("帮助里少了 `{line}`"));
            assert!(
                server < at && at < personal,
                "`{line}` 要归在「服务端 / WebUI」那组里 ✓"
            );
        }

        // 个人 CLI 那几条要在**它自己**那一段里 ✓
        for line in [
            "  tasks",
            "  run <任务名>",
            "  login <方式>",
            "  help, --help, -h",
        ] {
            let at = USAGE
                .find(line)
                .unwrap_or_else(|| panic!("帮助里少了 `{line}`"));
            assert!(at > personal, "`{line}` 要归在「个人 CLI」那组里 ✓");
        }

        // ⚠ 默认行为**没被改**：不带参数仍然起服务，这句话必须还在 ✓
        assert!(
            USAGE.contains("不带参数跑 `mcloud` 就是它"),
            "「不带参数 = 起服务」这条口径不能丢 ✓"
        );
    }

    /// 门：`mcloud run` 的**两种形式**都要在帮助里说清 ✓（2026-10-11 用户口径：
    /// 「`mcloud run` 为简写…两种形式都留」✓）。
    ///
    /// 两条断言各对应一种形式：不带参数 = **全部账号跑系统默认每日任务** ✓；
    /// 带参数 = **只跑一次那个任务**（照旧 ✓）。
    #[test]
    fn usage_documents_both_forms_of_run() {
        assert!(
            USAGE.contains("不带参数 = **立即**把「系统默认每日任务」跑在"),
            "帮助里要写清「不带参数 = 全部账号跑系统默认每日任务」✓"
        );
        assert!(
            USAGE.contains("**本机管理员名下的全部账号**"),
            "「所有账号」= 管理员名下的全部账号，这句要说出来 ✓"
        );
        assert!(
            USAGE.contains("带参数 = 只跑一次那个任务"),
            "带参数那条（照旧）也要写清 ✓"
        );
        // 带参数那条的**老用法行**一个字没改 ✓（既有测试也钉着它 ✓）
        assert!(
            USAGE.contains("  run <任务名> <账号> [--body <JSON>]"),
            "带参数那条的用法行不能被改动 ✓"
        );
    }

    /// 门：**`mcloud config --help` 是"看帮助"，不是"用法错"**（2026-10-12）。
    ///
    /// 与 `run --help` / `serve --help` 逐格同款：收敛成 [`cli`] 的 `--help` 那一格
    /// ⇒ 与 `mcloud --help` 同一份文本、打 stdout、退 0。
    ///
    /// 这条断言是行为契约：谁要是把 `config --help` 拉回子命令解析，
    /// 它就会变成"不认识的参数"报用法错，这里就红。
    #[test]
    fn config_help_is_help_not_a_usage_error() {
        for flag in ["help", "--help", "-h"] {
            assert_eq!(
                dispatch(vec!["config".to_string(), flag.to_string()]),
                cli(&[flag]),
                "`config {flag}` 要与 `mcloud {flag}` 走同一条路"
            );
        }
        // 只认"光一个 `config` + 一个帮助开关"：带参数那些写法一个都不能被吃掉
        assert_eq!(dispatch(vec!["config".to_string()]), cli(&["config"]));
        assert_eq!(
            dispatch(vec![
                "config".to_string(),
                "export".to_string(),
                "--out".to_string(),
                "x.json".to_string()
            ]),
            cli(&["config", "export", "--out", "x.json"]),
            "`config export --out <路径>` 照旧走子命令解析"
        );
        assert_eq!(
            dispatch(vec![
                "config".to_string(),
                "import".to_string(),
                "f.json".to_string()
            ]),
            cli(&["config", "import", "f.json"])
        );
    }

    /// 门：`config` 那两条用法行归在**个人 CLI** 那一组（不是"服务端 / WebUI"组）。
    ///
    /// 它与 `push` / `live-room` / `auth` 同一条立场：CLI 只作用于本机那一个用户，
    /// 是"个人 CLI"那一档；而 `serve` / `user` 是服务端那一档。
    #[test]
    fn usage_puts_config_in_the_personal_group() {
        let server = USAGE
            .find("服务端 / WebUI 命令")
            .expect("帮助里要有「服务端 / WebUI 命令」那一组");
        let personal = USAGE
            .find("个人 CLI")
            .expect("帮助里要有「个人 CLI」那一组");
        for line in [
            "  config export [--out <路径>]",
            "  config import <路径> [--with-instance]",
        ] {
            let at = USAGE
                .find(line)
                .unwrap_or_else(|| panic!("帮助里少了 `{line}`"));
            assert!(at > personal, "`{line}` 要归在「个人 CLI」那组里");
            assert!(at > server, "`{line}` 不该落在「服务端 / WebUI」那组里");
        }
        // 实例级那个开关与"导入完要重启才生效"都要在用法里说清
        assert!(USAGE.contains("--with-instance"));
        assert!(USAGE.contains("scripts/dev-reload.sh --no-build"));
    }
}
