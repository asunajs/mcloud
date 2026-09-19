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

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use mcloud_app::{app_login, device, login, migration, payload, user};
use mcloud_core::{Clock, Error as CoreError, UserId, UserStore};
use mcloud_io::{ReqwestClient, SqliteStore, SystemClock, SystemRandom};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::{AppDevice, AppProfile};
use mcloud_upstream::app_login::AppLoginCrypto;
use mcloud_upstream::login::{LoginDataDecryptor, PcPlatform};
use std::path::{Path, PathBuf};
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
    /// 系统用户管理（登录**本系统**的账号，与云盘账号是两回事）
    User {
        #[command(subcommand)]
        action: UserAction,
    },
    /// 登录移动云盘账号（扫码 / 短信）
    Login {
        #[command(subcommand)]
        action: LoginAction,
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

/// 系统用户管理。
///
/// # CLI **刻意不做登录**
///
/// 数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，
/// 给他加一道"你是谁"的关卡既不增加安全性，又把本地运维变别扭。
/// 所以 CLI 一律用 [`mcloud_core::Scope::Local`]（不过滤）。
///
/// 隔离是给**网络上的用户**准备的（WebUI：A 看不到 B 的账号）——
/// 那里用户拿不到库文件，边界才有意义。
///
/// 这里只保留两件事：`init`（建首个管理员，网页端要用，也是账号认领的触发点）
/// 与 `list`（看看有哪些人）。
#[derive(Subcommand, Debug)]
enum UserAction {
    /// 初始化首个管理员（顺手认领从旧库迁移过来、还没归属的账号）
    Init {
        /// 用户名
        #[arg(long, default_value = "admin")]
        name: String,
        /// SQLite 文件
        #[arg(long, default_value = "data/mcloud.db")]
        db: PathBuf,
    },
    /// 列出已有用户
    List {
        #[arg(long, default_value = "data/mcloud.db")]
        db: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum LoginAction {
    /// 扫码登录：终端里画出二维码，用手机扫
    Qr(QrArgs),
    /// 短信验证码登录
    Sms(SmsArgs),
}

/// 冒充哪个客户端。**必须显式选**：不同端的 `clientType` / `cpid` / 版本号都不同，
/// 混搭（mac 的 clientType 配 Windows 的版本号）是上游一眼可辨的矛盾组合。
#[derive(ValueEnum, Clone, Copy, Debug)]
enum PlatformArg {
    Windows,
    Macos,
}

impl From<PlatformArg> for PcPlatform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Windows => Self::Windows,
            PlatformArg::Macos => Self::MacOs,
        }
    }
}

#[derive(clap::Args, Debug)]
struct QrArgs {
    #[arg(long, default_value = "data/mcloud.db")]
    db: PathBuf,
    #[arg(long, value_enum, default_value_t = PlatformArg::Windows)]
    platform: PlatformArg,
    /// 设备指纹文件。**换掉它等于换一台设备**：服务端按 `dID + sID` 定位会话，
    /// 漂移的表现是"二维码扫了、手机也确认了，这边毫无反应"。
    #[arg(long, default_value = "data/device.txt")]
    device_file: PathBuf,
    /// 二维码有效期（秒）。默认 120；把码发给别人扫、或先把码贴出来再拿手机时调大。
    #[arg(long, default_value_t = mcloud_capabilities::login::QR_TTL_SECONDS)]
    ttl_seconds: i64,
    /// 额外把二维码写成图片（SVG）。终端字体等宽不保证，字符画经常会变形扫不出来。
    #[arg(long)]
    qr_out: Option<PathBuf>,
    /// 不等用户扫（只出码就退出，供调试）
    #[arg(long)]
    no_wait: bool,
}

#[derive(clap::Args, Debug)]
struct SmsArgs {
    /// 手机号
    #[arg(long)]
    phone: String,
    #[arg(long, default_value = "data/mcloud.db")]
    db: PathBuf,
    /// 冒充哪个客户端。**两端不是"同一件事的两种参数"**，见 `--client` 的说明。
    #[arg(long, value_enum, default_value_t = ClientArg::Pc)]
    client: ClientArg,
    /// 仅 PC：`clientType` / `cpid` / 版本号
    #[arg(long, value_enum, default_value_t = PlatformArg::Windows)]
    platform: PlatformArg,
    /// 设备指纹文件。**换掉它等于换一台设备**：服务端按 `dID + sID` 定位会话，
    /// 漂移的表现是"二维码扫了、手机也确认了，这边毫无反应"。
    ///
    /// App 端用 `--app-device-file`（形状不同：无 `-ENDIN` 后缀）。
    #[arg(long, default_value = "data/device.txt")]
    device_file: PathBuf,
    /// 仅 App：`deviceId` 的存放位置（32 位大写 HEX，无后缀）。
    #[arg(long, default_value = "data/device-app.txt")]
    app_device_file: PathBuf,
    /// 只发码、不进入输入验证码这一步，然后退出。
    ///
    /// 发码与登录本来就是两次独立调用，中间隔着"用户去手机上读那条短信"。
    /// 有了它就能把两半分开跑（配合 `--device-short` 钉住设备形态）。
    ///
    /// 与 `--code` 互斥：码已经到手时，没有"只发码"这回事。
    #[arg(long, conflicts_with = "code")]
    send_only: bool,
    /// 直接给出验证码，跳过交互式输入。**同时跳过发码**。
    ///
    /// 码已经到手就不该再发一次：那会白刷一次今日配额，还要连带过一次滑块。
    /// 发码与登录本来就是两次独立调用，有码时只跑后一半。
    ///
    /// # 为什么这里可以接受"从命令行传"，而口令不行
    ///
    /// 口令（`--password`）**永远不允许**：它长期有效、可复用，会留在 shell 历史与 `ps` 里。
    /// 短信验证码不同 —— 它**单次、短时效**（分钟级、用过即废），
    /// 且自动化测试与远程协助必须能非交互地提交它。
    /// 真要更严，用标准输入即可（不给这个参数就走交互读取）。
    #[arg(long)]
    code: Option<String>,
    /// 仅 App：固定设备串第 7 段（12 位大写 HEX）。
    ///
    /// **不传时每次运行现生成**（实测该段不是每安装稳定值，见
    /// `mcloud_app::device::new_app_device_short`）。传它是为了把
    /// "发码 → 登录"两半**钉在同一台设备形态**上：
    /// `--send-only` 会把当次生成的值打出来，原样传回来即可。
    #[arg(long)]
    device_short: Option<String>,
}

/// 冒充 PC 还是手机 App。
///
/// # 为什么必须显式选，而不是"自动探测"
///
/// 两端的差别不止是几个参数：
///
/// | | PC | App |
/// | --- | --- | --- |
/// | 请求体 | 明文 JSON | **加密** |
/// | 滑块 | 可取图自动解 | **H5 页面，只能人工拖** |
/// | 发短信 | 我们发 | **H5 页面自己发** |
/// | `Authorization` 前缀 | `pc` | `mobile` |
///
/// 混搭（App 的 `clientType` 配 PC 的明文请求体）会被上游当成"参数不合法"，
/// 而那个错误码与"密钥用错"长得一模一样。所以让调用方**显式**承担这个选择。
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ClientArg {
    /// PC 客户端（Windows / macOS）
    Pc,
    /// iOS App
    App,
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

        Command::User { action } => match action {
            UserAction::Init { name, db } => user_init(&db, &name).await,
            UserAction::List { db } => user_list(&db).await,
        },

        Command::Login { action } => match action {
            LoginAction::Qr(args) => login_qr(args).await,
            LoginAction::Sms(args) => login_sms(args).await,
        },

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

async fn user_init(db: &Path, name: &str) -> Result<()> {
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

async fn user_list(db: &Path) -> Result<()> {
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
fn read_secret(prompt: &str) -> Result<String> {
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
fn read_password_twice() -> Result<String> {
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
fn open_local(db: &Path) -> Result<SqliteStore> {
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
async fn preferred_owner(store: &SqliteStore) -> Result<Option<UserId>, CoreError> {
    Ok(store
        .list_users()
        .await?
        .into_iter()
        .next()
        .map(|user| user.id))
}

/// 把"默认归属"变成一个范围：有主可选就归它，否则本地范围（无主）。
fn scope_for_new_accounts(owner: &Option<UserId>) -> mcloud_core::Scope<'_> {
    match owner {
        Some(id) => mcloud_core::Scope::user(id),
        None => mcloud_core::Scope::Local,
    }
}

async fn login_qr(args: QrArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let platform: PcPlatform = args.platform.into();
    let mut rng = SystemRandom;
    let device_id = device::load_or_create(&args.device_file, &mut rng).map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);
    let observer = TerminalObserver {
        device_id: device_id.clone(),
        qr_out: args.qr_out.clone(),
    };

    println!(
        "正在以 {} 客户端身份登录（设备 {}）",
        platform_name(platform),
        device_id
    );

    if args.no_wait {
        let qr = mcloud_capabilities::login::QrLogin::start(
            platform,
            device_id.as_str(),
            &mut rng,
            SystemClock.now(),
        );
        println!("\n{}", render_qr(&qr.qr_payload())?);
        println!("{}", qr.qr_payload());
        return Ok(());
    }

    let outcome = login::qr_login(
        &store,
        &upstream,
        login::LoginContext::new(scope_for_new_accounts(&owner), platform, &device_id)
            .with_ttl(args.ttl_seconds),
        &LoginDataDecryptor::production(),
        &SystemClock,
        &mut rng,
        &observer,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    // 打印**字段名**（不打印值）：上游的响应形状与文档示例并不总是一致
    // （实测就少了一个 `userId`），而字段名不是秘密、值才是。
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}

async fn login_sms(args: SmsArgs) -> Result<()> {
    match args.client {
        ClientArg::Pc => login_sms_pc(args).await,
        ClientArg::App => login_sms_app(args).await,
    }
}

async fn login_sms_pc(args: SmsArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let platform: PcPlatform = args.platform.into();
    let mut rng = SystemRandom;
    let device_id = device::load_or_create(&args.device_file, &mut rng).map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    // 发码（被要求滑块时自动解答案重试）。这段循环在 `mcloud_app::login` 里，
    // 与 WebUI 共用同一份 —— 协议语义不该各写一遍。
    //
    // 但 `--code` 已经带着码来了就不再发：那会白刷一次今日配额，
    // 而且 PC 端发码还要连带解一次滑块。发码与登录是两次独立调用，有码时只跑后一半。
    let code = match &args.code {
        Some(code) => code.clone(),
        None => {
            let sent = login::sms_send_code(&upstream, platform, &device_id, &args.phone, &mut rng)
                .await
                .map_err(core_err)?;
            match sent.remain_get_times {
                Some(remain) => println!("验证码已发送（今日剩余 {remain} 次）"),
                None => println!("验证码已发送"),
            }
            if sent.solved_slider {
                println!("（被风控要求滑块，已自动解出缺口位置并重发）");
            }
            read_secret("请输入收到的 6 位验证码: ")?
        }
    };

    let outcome = login::sms_login(
        &store,
        &upstream,
        login::LoginContext::new(scope_for_new_accounts(&owner), platform, &device_id),
        login::SmsCredential {
            phone: &args.phone,
            code: code.trim(),
        },
        &LoginDataDecryptor::production(),
        &SystemClock,
        &mut rng,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}

/// App（iOS）端的验证码登录。
///
/// # 与 PC 那条路最大的不同：**短信是那个 H5 页面发的**
///
/// 被要求滑块时，PC 端可以取图 → 算坐标 → 重发（所以 PC 有个自动重试循环）。
/// App 端没有这条路：原生侧只是把 H5 打开，**短信由 H5 发**，
/// 回调之后直接进 `thirdlogin`。所以这里把地址打给用户，
/// 并明确"不要再重新执行本命令"——否则会再要一次滑块、把配额刷掉。
///
/// # 补充：本函数里那个自动重试环**不是** App 自己的滑块被自动化了
///
/// `app_login::app_send_sms_code` 的环走的是 **PC 端那套**取题与编码
/// （`/user/verfycode` + `puzzle_verfycode_encrypt`，只是拿 App 的头发出去），
/// 属**围魏救赵**。App 端自身的滑块自动化**并未实现**，真机走的仍是上面那条 H5 路。
/// 所以别把"这里过掉了"读成"App 的滑块已攻克"。
async fn login_sms_app(args: SmsArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let mut rng = SystemRandom;

    let device_id =
        device::load_or_create_app_device_id(&args.app_device_file, &mut rng).map_err(core_err)?;
    // `deviceShort` **每次运行现生成**：实测同一 `deviceId` 在不同时间点换过 short，
    // 所以它不是一个可以持久化的"每安装指纹"（见 `mcloud_app::device` 的说明）。
    // 传 `--device-short` 可以把它钉住，用于把"发码 → 登录"两半连成一次会话。
    let device_short = match &args.device_short {
        Some(short) => short.to_ascii_uppercase(),
        None => device::new_app_device_short(&mut rng),
    };
    let profile = AppProfile::new(AppDevice::ipad(&device_id, &device_short));
    // 读回填与写落库**必须同一个范围**：范围不同会出现"读到某个账号的用户域、
    // 却把它发给另一个账号"，那比不带 `x-yun-uni` 更糟。
    let scope = scope_for_new_accounts(&owner);
    // 登录过一次之后 `userDomainId` 就在库里了，第二次起要带上（真机也一样）。
    // 首次登录查不到，档案保持不带 —— 见 `app_login::profile_with_stored_domain_id`。
    let profile = app_login::profile_with_stored_domain_id(&store, scope, profile, &args.phone)
        .await
        .map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    println!("正在以 iOS App 身份登录（deviceId {device_id}）");

    // 有码就不再发码。App 端这一步的代价比 PC 更大：`getSmsCode` 会再要一次滑块，
    // 而白刷一次今日配额；码已经到手时它纯粹是浪费。
    let code = match &args.code {
        Some(code) => code.clone(),
        None => {
            match app_login::app_send_sms_code(&upstream, &profile, &args.phone, &SystemClock)
                .await
                .map_err(core_err)?
            {
                app_login::AppCodeOutcome::Sent {
                    remain_get_times,
                    solved_slider,
                } => {
                    match remain_get_times {
                        Some(remain) => println!("验证码已发送（今日剩余 {remain} 次）"),
                        None => println!("验证码已发送"),
                    }
                    if solved_slider {
                        println!(
                            "（被风控要求滑块，已取图解出缺口坐标并自动重发 —— 全程没有开浏览器）"
                        );
                    }
                }
                // 只有**自动过滑块也失败**时才会走到这里。把坐标一并打出来：
                // 万一是服务端要的答案口径变了，这一行就是唯一线索。
                app_login::AppCodeOutcome::NeedCaptcha {
                    url,
                    captcha_type,
                    attempts,
                    last_answer,
                } => {
                    println!(
                        "自动过滑块失败（已试 {attempts} 次，captchaType={}，最后发上去的坐标 {last_answer:?}）。",
                        captcha_type.map_or("未知".to_string(), |t| t.to_string())
                    );
                    println!("改走兜底路径：请在浏览器打开下面这个地址，拖动拼图完成验证：");
                    println!("\n{url}\n");
                    println!(
                        "（**短信由该页面发出**，完成拖动后回到这里继续。不要再重新执行本命令）"
                    );
                }
            }

            if args.send_only {
                // 把当次的设备短标识打出来：下一步用 `--device-short` 传回来，
                // 两半才会被上游看成同一台设备形态。
                println!("\n本次设备短标识（继续时用 --device-short 传回）：{device_short}");
                println!("发码完成，已按 --send-only 退出。");
                return Ok(());
            }

            read_secret("请输入收到的 6 位验证码: ")?
        }
    };

    let outcome = app_login::app_sms_login(
        &store,
        &upstream,
        app_login::AppLoginContext {
            scope,
            profile: &profile,
            credential: login::SmsCredential {
                phone: &args.phone,
                code: code.trim(),
            },
        },
        &AppLoginCrypto::production(),
        &SystemClock,
        &mut rng,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}

/// 终端观察者：把二维码画出来，把进展印成一行。
struct TerminalObserver {
    device_id: String,
    /// 额外落一张图片（`--qr-out`）。
    qr_out: Option<PathBuf>,
}

impl login::LoginObserver for TerminalObserver {
    fn on_qr(&self, payload: &str) {
        // 图片优先：终端字符画只在等宽字体下成立，实际很容易变形，
        // 而"码扫不出来"会被用户理解成"这软件坏了"。
        if let Some(path) = &self.qr_out {
            match write_qr_svg(payload, path) {
                Ok(()) => println!(
                    "\n二维码已写入图片，**打开它扫描**（比终端字符画可靠）：\n  {}\n",
                    path.display()
                ),
                Err(err) => eprintln!("（写二维码图片失败：{err:#}）"),
            }
        }

        println!("用移动云盘 App 扫描下面的二维码：\n");
        match render_qr(payload) {
            Ok(art) => println!("{art}"),
            Err(err) => println!("（终端画不出二维码：{err}）"),
        }
        // 链接也打出来：有的终端字体/配色会把二维码糊掉，手输链接是兜底
        println!("{payload}");
        println!(
            "设备指纹 {}（换掉它服务端就定位不到本次会话）\n",
            self.device_id
        );
        println!("等待扫码…（Ctrl-C 取消）");
    }

    fn on_progress(&self, progress: &mcloud_capabilities::login::QrProgress) {
        use mcloud_capabilities::login::QrProgress;
        match progress {
            QrProgress::NotScanned => {}
            QrProgress::Scanned => println!("已扫码，请在手机上确认登录"),
            QrProgress::LoggedIn { .. } => {}
            // 限频会反复出现，所以给一句人话 + 说明"码还有效"，
            // 否则用户看到"失败"会以为要重开一张码
            QrProgress::Failed {
                reason: mcloud_capabilities::login::QrFailure::TooFrequent,
            } => println!("被限频（扫码登录过于频繁），等一会儿自动重试；二维码仍然有效，不用重开"),
            QrProgress::Failed { reason } => println!("失败：{reason:?}"),
        }
    }
}

/// 把二维码载荷渲染成终端可打印的字符画。
///
/// 用半高块字符（`Dense1x2`）：同样的信息量只占一半行数，
/// 否则一屏放不下一个足够大的二维码（太小手机扫不出来）。
///
/// **但它只在等宽字体 + 正确行距下成立**，所以同时提供
/// [`write_qr_svg`]：字符画变形的概率远高于"用户以为的偶然"。
fn render_qr(payload: &str) -> Result<String> {
    let code = qrcode::QrCode::new(payload.as_bytes()).context("二维码内容过长或无法编码")?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

/// 把二维码写成 SVG 图片。
///
/// 用 SVG 而不是 PNG：**不用引入图像编码库**（qrcode 自带的渲染器就够了），
/// 而浏览器与 macOS 预览都能直接打开，扫描体验与位图没差别。
fn write_qr_svg(payload: &str, path: &Path) -> Result<()> {
    let code = qrcode::QrCode::new(payload.as_bytes()).context("二维码内容过长或无法编码")?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        // 每模块 8px 左右：太小手机对焦困难，太大一屏放不下
        .min_dimensions(360, 360)
        .quiet_zone(true)
        .build();

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败 {}", parent.display()))?;
    }
    std::fs::write(path, svg).with_context(|| format!("写二维码失败 {}", path.display()))?;
    Ok(())
}

const fn platform_name(platform: PcPlatform) -> &'static str {
    match platform {
        PcPlatform::Windows => "Windows",
        PcPlatform::MacOs => "macOS",
    }
}

/// `mcloud_core::Error` → `anyhow`。
///
/// 只在顶层做这一次转换：内核的错误类型带着"能不能重试、能不能给用户看"的分类，
/// 在这里折成 anyhow 之前，`{err:#}` 打出来的仍然是它的 Display。
fn core_err(err: CoreError) -> anyhow::Error {
    anyhow::Error::new(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::Scope;

    #[test]
    fn new_accounts_go_to_the_first_user_when_one_exists() {
        // CLI 不要求"你是谁"，但新建的账号得有主人 —— 否则网页端看不到它
        let owner = Some(UserId::new("admin"));
        assert!(matches!(
            scope_for_new_accounts(&owner),
            Scope::User(id) if id.as_str() == "admin"
        ));
    }

    #[test]
    fn new_accounts_stay_unowned_when_no_user_exists_yet() {
        // 库里还没有用户时不能凭空造一个主人：账号保持无主，
        // 等 `mcloud user init` 时被认领（那条路径在 io 层有用例）
        assert!(matches!(scope_for_new_accounts(&None), Scope::Local));
    }
}
