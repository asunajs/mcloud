// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::args::*;
use clap::{Parser, Subcommand, ValueEnum};
use mcloud_upstream::login::PcPlatform;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mcloud",
    version,
    about = "移动云盘自动化工具（Rust 重写版）",
    long_about = "不带子命令时直接对**全部账号**跑一遍签到 —— 对齐旧项目\
                  「直接运行、一个参数都不用给」的用法。\
                  顶层与 `mcloud signin` 收**同一套参数**，所以 `mcloud --list`、\
                  `mcloud --status-only`、`mcloud --db <路径>` 都成立。"
)]
pub(crate) struct Cli {
    /// 不传 = 直接跑签到（等价于 `mcloud signin`）
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
    /// 不带子命令时用的就是这一份 —— 与 `mcloud signin` 是**同一个结构体**。
    ///
    /// # 为什么是平铺（`flatten`）而不是"再解析一遍 `["mcloud","signin"]`"
    ///
    /// 先前那种写法有个直接的后果：顶层只认子命令，于是 `mcloud --list` /
    /// `mcloud --status-only` / `mcloud --db X` 会被 clap 判成"未知参数"，
    /// 报 `Usage: mcloud [COMMAND]` —— 一个主打"直接运行"的命令行，
    /// 却要用户先知道子命令叫 `signin` 才能查编号，这是自相矛盾的。
    ///
    /// 平铺之后**只有一份定义**：默认值、互斥关系、帮助文本都不会两处漂移，
    /// 而"两处漂移"恰恰是原来那种写法最容易出的错（只在"直接运行"时表现为
    /// "某个默认值没生效"，最难被发现）。
    #[command(flatten)]
    pub(crate) signin: SigninArgs,
}

impl Cli {
    /// 把"不带子命令"补成显式的 `signin`。
    ///
    /// 两种写法（`mcloud` 与 `mcloud signin`）在这里合流，往下只有一个入口。
    pub(crate) fn into_command(self) -> Command {
        self.command.unwrap_or(Command::Signin(self.signin))
    }
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
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
    /// 查看 / 更换账号绑定的设备特征（smid），以及它换来的 `deviceId`
    Smid {
        #[command(subcommand)]
        action: SmidAction,
    },
    /// 签到（云朵中心）。需要先有一次 App 端登录
    Signin(SigninArgs),
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
pub(crate) enum FixtureAction {
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
pub(crate) enum UserAction {
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
pub(crate) enum LoginAction {
    /// 扫码登录：终端里画出二维码，用手机扫
    Qr(QrArgs),
    /// 短信验证码登录
    Sms(SmsArgs),
}

/// 设备特征（smid）与它换来的 `deviceId`。
///
/// # 为什么这件事需要一个命令
///
/// 旧实现每次上报都现掷一份新特征，同一个账号在风控眼里每次都换一台设备。
/// 新项目把特征**绑到账号**上，于是"我这台设备长什么样"成了一个要能看、
/// 也要能换的状态 —— 没有出口的话，一次误判就变成永久状态。
///
/// 输出**只给摘要**：`deviceId` 是第三方风控服务给这台设备的标识，
/// canvas 那段是特征的识别位，都不该整份打到终端（详见 `SmidState` 的脱敏口径）。
#[derive(Subcommand, Debug)]
pub(crate) enum SmidAction {
    /// 列出每个账号绑定的设备特征与最近换到的 `deviceId`
    Show {
        #[arg(long, default_value = "data/mcloud.db")]
        db: PathBuf,
    },
    /// **更换**某个账号的设备特征：新特征落库，旧的 `deviceId` 当场作废
    Reset {
        /// 账号 id（`mcloud smid show` 里那一列，形如 `19****01`）
        #[arg(long)]
        account: String,
        #[arg(long, default_value = "data/mcloud.db")]
        db: PathBuf,
        /// 换完立刻上报一次，把新的 `deviceId` 也拿到手
        ///
        /// 不传就等到下一次真正用到它时再上报（少一次无谓的第三方请求）。
        #[arg(long)]
        report: bool,
    },
    /// 不换特征，只**再上报一次**换一个新的 `deviceId`
    Refresh {
        #[arg(long)]
        account: String,
        #[arg(long, default_value = "data/mcloud.db")]
        db: PathBuf,
    },
}

/// 冒充哪个客户端。**必须显式选**：不同端的 `clientType` / `cpid` / 版本号都不同，
/// 混搭（mac 的 clientType 配 Windows 的版本号）是上游一眼可辨的矛盾组合。
#[derive(ValueEnum, Clone, Copy, Debug)]
pub(crate) enum PlatformArg {
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
