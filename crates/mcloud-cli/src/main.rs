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
use mcloud_app::{app_login, cloud, device, login, migration, payload, signin, smid, user};
use mcloud_core::{
    Account, AccountId, Clock, Error as CoreError, Random, Scope, StateStore, UserId, UserStore,
    account_label, md5_hex,
};
use mcloud_io::{ReqwestClient, SqliteStore, SystemClock, SystemRandom};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::{AppDevice, AppProfile};
use mcloud_upstream::app_login::AppLoginCrypto;
use mcloud_upstream::app_signin::SignInStatus;
use mcloud_upstream::login::{LoginDataDecryptor, PcPlatform};
use mcloud_upstream::smid::SmFingerprint;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
struct Cli {
    /// 不传 = 直接跑签到（等价于 `mcloud signin`）
    #[command(subcommand)]
    command: Option<Command>,
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
    signin: SigninArgs,
}

impl Cli {
    /// 把"不带子命令"补成显式的 `signin`。
    ///
    /// 两种写法（`mcloud` 与 `mcloud signin`）在这里合流，往下只有一个入口。
    fn into_command(self) -> Command {
        self.command.unwrap_or(Command::Signin(self.signin))
    }
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
enum SmidAction {
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

/// 签到（云朵中心）的参数。
///
/// # 账号怎么给（**缺口已收口，2026-09-19**）
///
/// 首选 `--account <掩码id>`：完整手机号在 **v6 起随登录落进 `account_state.phone`**，
/// 所以签到不需要再让用户输一遍号码。
///
/// `--phone <完整手机号>` 保留给**老账号**：v6 之前登录的行 `phone` 列是 `NULL`，
/// 那是"当时没存"的如实记录，重登一次就补上。
///
/// # 为什么不能拿掩码去凑
///
/// 第一段 `querySpecTokenV2` 要 `Authorization: Basic base64("mobile:<手机号>:<authToken>")`，
/// 中间那段是**完整**手机号。拿掩码（`19****16`）去拼，服务端认不认是未知数 ——
/// 而这类"能过但不该过"的东西一旦过了一次，就会变成没人敢动的隐性依赖。
#[derive(clap::Args, Debug)]
struct SigninArgs {
    /// 账号**下标**（从 1 开始，就是 `mcloud signin --list` 打出来的那个编号）。
    ///
    /// # 为什么是下标而不是掩码 id
    ///
    /// 旧项目 `mcloud-sign` 一直用 `账号 1` / `账号 2` 指代账号，日志与配置里都是这个
    /// 语义。掩码 id（`19****16`）要用户先去别处查一次、再原样抄过来 —— 那是把
    /// "机器的主键"当成"人的输入"用。
    ///
    /// **不传就是全部账号**（只跑启用的）—— 这也是不带子命令直接 `mcloud` 时的行为。
    ///
    /// ⚠ 下标是**位置**，不是身份：在中间插一个账号会让后面所有编号平移。
    /// 定时任务里请用 `--phone`（老账号）或干脆不传（跑全部）。
    #[arg(long, value_name = "N")]
    account: Option<usize>,
    /// 打印账号清单（含下标），然后退出 —— 用它查 `--account` 该填几
    #[arg(long, conflicts_with_all = ["account", "phone"])]
    list: bool,
    /// 完整手机号（11 位），不是掩码 id。**只给 v6 之前登录、库里没有整串凭据的老账号用。**
    ///
    /// 用一次就够：跑完会**把号码记进库里**，下次直接 `--account <下标>`。
    /// ⚠ 只记号码，**不记**这一次现拼出来的整串 —— 那一列的口径是"服务端给的原文"，
    /// 写进一个本地合成物会绕过前缀校验（详见 `mcloud_app::signin::SignInCredential::from_phone`）。
    #[arg(long)]
    phone: Option<String>,
    #[arg(long, default_value = "data/mcloud.db")]
    db: PathBuf,
    /// 仅 App：`deviceId` 的存放位置（32 位大写 HEX，无后缀）。
    ///
    /// 必须与登录时用的是**同一个**：服务端按设备定位会话，漂移的表现是
    /// "凭据都在、却回 90001 未登录"。
    #[arg(long, default_value = "data/device-app.txt")]
    app_device_file: PathBuf,
    /// 仅 App：固定设备串第 7 段（12 位大写 HEX）。不传则每次现生成。
    #[arg(long)]
    device_short: Option<String>,
    /// H5 那个 `deviceId`（89 字符 base64、`B` 前缀）。**一般不用传。**
    ///
    /// # 不传时怎么来的（纯 Rust，不需要浏览器）
    ///
    /// 从库里这个账号的 `smid_device_id` 取；库里还没有就**现上报一次**换一个。
    /// 走的是 `mcloud_app::smid` —— 它打的
    /// `POST https://slw.h5cmpassport.com:9090/deviceprofile/v4` 与签到页
    /// `fp.min.js` 是**同一个接口、同一份 `organization` / `appId` / RSA 公钥**
    /// （逐字比对过），拿回来的是**同一个 `"B"+deviceId`**。
    ///
    /// ⇒ 以前那条"用官方 App 打开一次签到页、再用 `scripts/h5-device-id.py`
    /// 从 WebView 的 localStorage 里读"的路子**不再需要**（脚本留着只当取证工具）。
    ///
    /// # 什么时候才需要显式传
    ///
    /// 只想拿"别的设备 id"做对照实验时。日常 `--receive` 会自动解析，不用管。
    ///
    /// ⚠ 别拿空串去试：`receiveV3` 不带 `deviceId` 会回 `615`（上游**校验它的内容**，
    /// 所以"随便编一个"也走不通，实测编错一个字符回 `614`）。
    #[arg(long = "device-id", visible_alias = "h5-device-id")]
    device_id: Option<String>,
    /// 签到之后**领云朵**：默认按 `infoV3` 的账目**自动分流**。
    ///
    /// ⚠ **这是写请求** —— 上游认为有可领的就会真的领走。
    ///
    /// # 默认行为：自己判断该领哪个（不用给任何参数）
    ///
    /// 走 [`mcloud_app::signin::receive_all`]：读一次 `infoV3` 的 `receiveList`，
    /// 按 `cloudType` 逐条分流 —— 判据见 [`cloud_type_label`]。**两条线不一样**：
    ///
    /// | `cloudType` | 是什么 | 动作 | 生效 |
    /// | --- | --- | --- | --- |
    /// | `1` | **月度任务奖励**（上个月攒下的）| `receiveTaskExpansion(acceptDate)` | 按结算日，**不是立即** |
    /// | `2` | **下月**才可领 | **什么都不发** | — |
    /// | 其它 | **立即领取** | `receiveV3` | **直接生效** |
    ///
    /// 所以「账目里有个数」**推不出**「现在能领到」：我们两个账号的账目都是
    /// `cloudType=2`，那是**下月**的数 —— 实测跑完前后余额一字不变
    /// （`5392 → 5392`），同一天的官方抓包里 `receiveV3` 请求数也是 `0`。
    ///
    /// # 显式模式（自检用）
    ///
    /// 给了 `--cloud-type` 就**不再分流**，只发你指定的那一笔 —— 用来复现
    /// "不带 deviceId 回 `615`" 这类结论。日常别用它：`cloudType` 填错的表现是
    /// "领到 0 朵"，看起来像上游没发。
    ///
    /// # 它**不包含**签到翻倍
    ///
    /// 「自动备份」每月送的那一次翻倍机会（`multiple`）是**另一条线**，
    /// 有独立的门槛（`curMonthBackup && !curMonthBackupSignAccept`），
    /// 所以单独一个 `--multiple`。两个可以一起给（`--receive --multiple`），
    /// 与旧项目把它们拆成 `receiveTask` / `executeSignInMultiple` 是一致的。
    #[arg(long, conflicts_with = "status_only")]
    receive: bool,
    /// 领取的 `cloudId`（`infoV3` 的 `receiveList[].recordId`）。默认空串。
    ///
    /// 实测抓包里那个数组只有 `cloudType` / `cloudNum`、**没有 `recordId`**，
    /// 所以 H5 的 `e.recordId ?? ""` 实际给的就是空串。照抄，别自己编一个。
    ///
    /// **只在显式模式（给了 `--cloud-type`）下有用。**
    #[arg(long, default_value = "")]
    cloud_id: String,
    /// 显式指定 `cloudType`（`receiveList[].cloudType`）—— **自检用，日常不要给**。
    ///
    /// 不给 = 按账目自动分流（推荐）。给了 = 只发这一笔，见 `--receive` 的说明。
    ///
    /// ⚠ 别拿 `2` 去试：那是**下月可领取**，发出去只会拿到 `+0`。
    #[arg(long)]
    cloud_type: Option<i64>,
    /// 用掉一张**复活卡 / 回馈卡**（`receiveRevivalReward`）。
    ///
    /// ⚠⚠ **这是写请求，而且不可逆** —— H5 的原话是「关闭弹窗视为放弃机会，
    /// **无法补发**」。卡用掉就没了，所以这个开关**默认不开**。
    ///
    /// 与 `--receive` 的区别：那一条领的是**当期可领的云朵**，这一条消耗的是**卡**。
    /// 它**不需要** deviceId（H5 定义里没有 `isDeviceId`），所以**默认一个头都不带**；
    /// 给了 `--device-id` 才会补一个 `deviceid` 头 —— 两个证据不一致，见上游层的说明。
    #[arg(long, conflicts_with = "status_only")]
    revive: bool,
    /// 领**签到翻倍**（`multiple`）—— 「自动备份」每月送的那一次机会。
    ///
    /// # 它领什么
    ///
    /// 本月开着自动备份、且这次机会还没用掉时，领一次能把**今日签到奖励**
    /// 翻 2–50 倍。H5 原话：「备份成功，获得1次AI豆翻倍机会今日签到奖励可翻2-50倍！」，
    /// 领到手后弹「额外获得 {cloudCount} 豆，自动备份保持开启，任务奖励翻倍!」。
    ///
    /// 这是旧项目 `mcloud-sign` 里 `signIn` 顺手做掉的一步（`executeSignInMultiple`），
    /// 很容易被忽略 —— 而它是一条**独立的、每月一次的**收入。
    ///
    /// # 门槛**自己判**，不成立就一个请求都不发
    ///
    /// 判据逐字来自旧项目：`if (curMonthBackup && !curMonthBackupSignAccept)`，
    /// 两个值都取自 `infoV3`。**每月只有一次机会，发错是白白消耗。**
    ///
    /// ⚠ 没开备份时旧项目会**上传一个文件把备份打开再重试** —— 我们**没有网盘上传
    /// 能力**，所以只会如实报告"本月未开启备份"，**不假装做过**。
    #[arg(long, conflicts_with = "status_only")]
    multiple: bool,
    /// 只查状态，不签到
    #[arg(long)]
    status_only: bool,
    /// 把 `infoV3` 的**原始响应信封**原样打出来，不做任何解析。
    ///
    /// # 为什么需要它
    ///
    /// 我们的结构体只声明了**自己用得到的**字段。于是"某个数到底在哪"这类问题
    /// 会被自己的解析挡住 —— 你看到的是"我们读出来的"，不是"上游给的"。
    /// 这个开关把后者原样交出来（`--status-only` 的读请求，换一种打印）。
    ///
    /// ⚠ 原始响应里带**账号标识**（`x-yun-uni` 那个 19 位值等）。
    /// 别把输出直接贴到公开的地方 —— 要贴就先用 `--account` 之外的字段手工删一遍。
    #[arg(long, conflicts_with_all = ["receive", "revive", "multiple", "backup_gift", "prizes"])]
    raw: bool,
    /// 查**豆的收支流水**（`cloudRecordV2`）—— 两个账本各打一页。
    ///
    /// # 它跟 `--status-only` 的分工
    ///
    /// `--status-only` 回答"**现在有多少**"；这一条回答"**这些是怎么来的**"。
    /// 余额对不上时，只有流水能指出差在哪一笔。
    ///
    /// # 两个账本，不是两种货币
    ///
    /// `aiDou=0` = 改名前的账本（云朵）、`aiDou=1` = 改名后的（AI豆）。
    /// 云朵与 AI豆是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
    /// 分开只是因为记录跨了那次改名。
    ///
    /// ⚠ 流水条目的**字段名没有实拍**（H5 只把它丢给模板渲染，从没读过具体字段名），
    /// 所以这里**原样打出来**，不假装知道哪个是金额、哪个是时间。
    #[arg(long, conflicts_with_all = ["receive", "revive", "multiple", "backup_gift", "raw", "prizes"])]
    records: bool,
    /// `--records` 把两个账本**翻完**（默认只取最近 50 条）。
    ///
    /// 为什么需要它：**按月结算的规律一眼就能看出来、只看 50 条就看不出来。**
    /// 实测这条流水有 460 + 70 条、跨 5 个月 —— 默认那 50 条正好卡在
    /// 「8 月 1 日入账 340、9 月 1 日入账 240」两笔**备份翻倍奖励**之间，
    /// 于是"下月可领取"这个数从哪来的**完全看不出来**。
    ///
    /// ⚠ 它是**只读**的（`GET cloudRecordV2`），但会把两个账本整个拉下来：
    /// 人看的输出有几百行。要看全量就开，日常对账别开。
    #[arg(long, requires = "records")]
    records_all: bool,
    /// 查**待领取奖品**（`record` 的「待领取」页签）。**只读。**
    ///
    /// # 它解决什么
    ///
    /// 旧项目 `mcloud-sign` 的 `printPendingPrizes`：把"中了但还没领"的奖品列出来，
    /// 免得奖品躺着过期。我们之前完全没有这条 —— 只有豆的流水，没有奖品。
    ///
    /// # 与 `--records` 的区别
    ///
    /// `--records` 是**豆的收支**（`cloudRecordV2`），这一条是**奖品的状态**
    /// （`record`）。两个不同的东西，别混：奖品领到手之后才变成豆入账。
    ///
    /// ⚠ 列表项模板只显示「**中奖时间**」（`insertTime`），但**过期时间
    /// （`expireTime`）在响应里是有的**（实拍：中奖后 14 天）——
    /// 这里两个都打：要盯的是过期时间，不是中奖时间。
    ///
    /// ⚠ 末尾会把**原始条目**原样列一遍（我们只认得出上面那几个字段，
    /// 其余不替上游编名字）。条目里**带账号标识**（`userDomainId`）——
    /// 别把输出直接贴到公开的地方，与 `--raw` 同一个注意。
    #[arg(long, conflicts_with_all = ["receive", "revive", "multiple", "backup_gift"])]
    prizes: bool,
    /// 领**备份好礼**（`backupgift/receive`）—— **写请求，每月一次**。
    ///
    /// # 它领什么
    ///
    /// 「备份好礼」本身（不是 [`Self::multiple`] 那个签到翻倍）。两者都以
    /// 「开了自动备份」为前提，所以没开备份时**两条会一起拿不到** ——
    /// 但它们是**两条独立的线**：接口、周期、门槛字段都不是同一个，别合并。
    ///
    /// # 门槛**自己判**，不成立就一个请求都不发
    ///
    /// 判据逐字来自旧项目 `backupGiftTask`：
    ///
    /// | `curMonth` | `state` | 动作 |
    /// | --- | --- | --- |
    /// | 假 | — | 本月无礼（**连 `state` 都不看**）|
    /// | 真 | `0` | **领** |
    /// | 真 | `1` | 本月已领过 |
    /// | 真 | 其他（实测 `-1`）| 未开启备份 |
    ///
    /// **每月只有一次，发错是白白消耗。**
    ///
    /// ⚠ 未开备份时旧项目会提示"前往 APP 手动开启"，**不会**替你去开 ——
    /// 我们同样只如实报告。
    #[arg(long, conflicts_with = "status_only")]
    backup_gift: bool,
    /// 今天已签也发一次 `startSignIn`。**用途只有一个：协议自检** ——
    /// 确认我们拼的请求被上游接受（而不是回 `401` / `90001`）。
    ///
    /// 服务端对同一天的重复签到是幂等的（实拍：已签时回 `success` + `todaySignIn:true`），
    /// 所以它不会把今天签两次。但日常签到别开：那会让"今天已经签过"这条捷径失效。
    #[arg(long)]
    force: bool,
    /// 以 JSON 输出（脚本 / 定时任务消费）
    #[arg(long)]
    json: bool,
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
    // 这里从手机号推 id：与下面 `app_sms_login` 落库时用的是同一个 `account_label`，
    // 所以"读回填"与"写落库"指的是同一行。
    let profile = app_login::profile_with_stored_domain_id(
        &store,
        scope,
        profile,
        &AccountId::new(account_label(None, &args.phone)),
    )
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
async fn signin_run(args: SigninArgs) -> Result<()> {
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
struct SigninTarget {
    /// 展示用下标（从 1 开始，与 [`list_accounts_with_index`] 打出来的一致）。
    ///
    /// `--phone` 定位的账号**不在**启用列表里也能跑，那时下标没有意义，记 0。
    index: usize,
    account_id: AccountId,
    /// `--phone` 给的那个完整号码（老账号专用）。`Some` 时**强制**走现拼那条路。
    phone_override: Option<String>,
}

/// 打印账号清单（含下标）。
///
/// 存在的理由只有一个：`--account` 收的是**下标**，而"下标 3 是哪个账号"必须
/// 有个地方能查到。顺序与 [`mcloud_core::StateStore::list_accounts`] 一致
/// （`sort_order ASC, id ASC`），所以它是稳定的。
async fn list_accounts_with_index(store: &SqliteStore, scope: Scope<'_>) -> Result<()> {
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
async fn select_signin_targets(
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

/// 跑**一个**账号的一次签到（含 `--receive` / `--revive` 两个写开关）。
///
/// 返回 `Ok(None)` = 人看的输出已经打完了；`Ok(Some(v))` = `--json` 模式的结果对象，
/// 交给调用方决定怎么合并（单账号直接打、多账号合成数组）。
///
/// `device_id` / `device_short` 由调用方给：它们代表**这台安装**，一次运行里不该变。
#[allow(clippy::too_many_arguments)]
async fn signin_one(
    store: &SqliteStore,
    scope: Scope<'_>,
    target: &SigninTarget,
    device_id: &str,
    device_short: &str,
    args: &SigninArgs,
    rng: &mut impl Random,
) -> Result<Option<serde_json::Value>> {
    let account_id = &target.account_id;
    let phone_override = target.phone_override.as_deref();

    // 设备形态必须与登录时一致（同一个文件 → 同一个 `deviceId`）。
    // 漂移的表现是"凭据都在、却回 90001 未登录"，看起来像凭据坏了。
    let profile = AppProfile::new(AppDevice::ipad(device_id, device_short));
    // 回填库里存的 `userDomainId`（= `x-yun-uni` 头）。读的范围必须与写的一致，
    // 否则会出现"读到某个账号的用户域、却把它发给另一个账号"。
    let profile = app_login::profile_with_stored_domain_id(store, scope, profile, account_id)
        .await
        .map_err(core_err)?;

    let credential = match phone_override {
        Some(phone) => signin::SignInCredential::from_phone(store, scope, phone).await,
        None => signin::SignInCredential::from_account(store, scope, account_id).await,
    }
    .map_err(core_err)?;

    // `--phone` 只该输一次：把号码记下来，下次 `--account <下标>` 就行。
    //
    // # 为什么**只记手机号**，不记上面那个现拼出来的整串
    //
    // `authorization` 列的口径是"**服务端给的原文**"——登录响应里拿到的、或旧配置里
    // 存着的那一串。而这里手上的整串是**我们拼的**：前缀由 `AppProfile` 硬编码成
    // `mobile`（见 `mcloud_app::signin` 里那道前缀闸）。
    //
    // 把拼出来的东西写进那一列，等于**给它伪造一个"这是拿到的"的身份**：
    //   ① 它会被整串优先路径直接采信，而那条路**绕过前缀闸** —— 此后这个账号
    //      发出去的每个头都带着一个我们自己安上去的前缀，没人再会去核对；
    //   ② 一旦它是 `pc:` 端的账号（`platform` 列为空的老行拦不住），存下去的就是
    //      一串**永久错的**凭据，而现象只是"忽然回 90001 未登录"。
    //
    // 只记手机号就够了：下次走退路照旧现拼，前缀闸照旧生效。用户不再需要输第二遍，
    // 而"这串是从哪来的"这个问题永远不会被答错。
    //
    // 用 `fill_account_credentials`（**只填空缺**）而不是整体覆盖：库里已有整串的账号
    // 不该被这个动作碰到。
    //
    // 失败不致命（这次签到用的凭据已经拼好了），但**要说一声**：否则用户下次还得
    // 再输一遍，而他不知道为什么。
    if let Some(phone) = phone_override
        && let Err(err) = store
            .fill_account_credentials(scope, account_id, None, Some(phone))
            .await
    {
        eprintln!("⚠ 手机号没能记进库里（下次还得再输一遍）：{err}");
    }

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    // H5 那个 `deviceId`。**显式给的优先**；否则从库里取（没有就现上报一次换一个）——
    // 两条路都是纯 Rust，见 [`resolve_device_id`]。
    //
    // 解析失败**不当致命错误**：签到本身不带它也能签上（实测 2026-09-19 账号 B），
    // 所以降级成"不带"，只把原因打到 stderr。真正需要它的 `--receive` 会从上游
    // 拿到 `615` —— 那时这条提示就是"为什么是 615"的唯一线索。
    //
    // `--status-only` / `--raw` / `--records` 直接跳过：查个状态不该顺手去打一次第三方风控。
    let read_only = args.status_only || args.raw || args.records;
    let h5_device_id = if read_only {
        None
    } else {
        match resolve_device_id(
            &upstream,
            store,
            scope,
            account_id,
            args.device_id.as_deref(),
            rng,
        )
        .await
        {
            Ok(id) => Some(id),
            Err(err) => {
                eprintln!("⚠ 取 H5 deviceId 失败，这次不带它：{err}");
                None
            }
        }
    };

    // 下标只有"按列表跑"时才有意义；`--phone` 定位的账号记 0，那时不打下标 ——
    // 打一个 0 出来只会让人以为它是第一个账号。
    let who = if target.index == 0 {
        format!("账号 {}", credential.account_id())
    } else {
        format!("账号 {} ({})", target.index, credential.account_id())
    };
    let progress = format!(
        "{who}：以 iOS App 身份{}",
        if args.raw {
            "取 infoV3 原始响应"
        } else if args.records {
            "取豆的收支流水"
        } else if args.prizes {
            "查待领取奖品"
        } else if args.status_only {
            "查询签到状态"
        } else {
            "签到"
        }
    );
    // ⚠ `--json` 时这行走 **stderr**。
    //
    // stdout 上只能有 JSON —— 否则 `mcloud signin --json | jq` 会因为这一行直接报
    // 语法错，而"机器读的输出里混进一句人话"正是最难被发现的破坏方式：
    // 人肉看一眼觉得没问题，脚本那边已经全挂了。
    if args.json {
        eprintln!("{progress}");
    } else {
        println!("{progress}");
    }

    if args.records {
        // 两个账本各取一页。顺序固定（改名前的在前），这样人看的输出与 JSON 的键序一致。
        let books = [
            (signin::RECORD_BOOK_CLOUD, "云朵账本（改名前）", "cloud"),
            (signin::RECORD_BOOK_BEAN, "AI豆账本（改名后）", "bean"),
        ];
        let mut json = serde_json::Map::new();
        for (book, label, key) in books {
            // `--records-all` 就一页页翻到底；默认只取第一页。
            //
            // 退出条件三个都要：`--records-all` 没开、上游给了 `total` 且已收满、
            // 或者**这一页是空的**（`total` 缺失时唯一的终止依据）。
            let mut collected = Vec::new();
            let mut total = None;
            for page_number in 1..=RECORD_MAX_PAGES {
                let page = signin::cloud_records(
                    &upstream,
                    &profile,
                    &credential,
                    &signin::RecordsQuery {
                        ai_dou: book,
                        filter: signin::RECORD_FILTER_ALL,
                        page_number,
                        page_size: RECORD_PAGE_SIZE,
                    },
                    &SystemClock,
                    rng,
                )
                .await
                .map_err(core_err)?;
                let got = page.records.len() as i64;
                total = page.total.or(total);
                collected.extend(page.records);
                if !args.records_all || got == 0 {
                    break;
                }
                if total.is_some_and(|t| collected.len() as i64 >= t) {
                    break;
                }
            }
            let page = signin::CloudRecords {
                records: collected,
                total,
            };
            if !args.json {
                print_cloud_records(label, book, &page);
            }
            json.insert(
                key.to_string(),
                serde_json::json!({
                    "aiDou": book,
                    "total": page.total,
                    "records": page.records,
                }),
            );
        }
        if args.json {
            return Ok(Some(serde_json::Value::Object(json)));
        }
        return Ok(None);
    }

    if args.prizes {
        // **只读**，而且**不落库** —— 与 `--raw` 同策：奖品列表不是余额，记进
        // `cloud_stats` 只会往趋势图里塞噪声。
        //
        // 只取一页（`PRIZE_PAGE_SIZE`）：这条的用途是"看有没有待领的"，
        // 不是把历史全拉下来。要看更多页得自己翻 —— 现在没有那个开关。
        let page = signin::pending_prizes(
            &upstream,
            &profile,
            &credential,
            &SystemClock,
            rng,
            PRIZE_PAGE_SIZE,
        )
        .await
        .map_err(core_err)?;
        if args.json {
            return Ok(Some(serde_json::json!({
                "marketName": signin::PRIZE_MARKET_NAME,
                "status": signin::PRIZE_STATUS_PENDING,
                "pageSize": PRIZE_PAGE_SIZE,
                // 条目原样带出去 —— 我们只认得出几个字段，其余不替上游编名字。
                "records": page.records.iter().map(|item| item.raw.clone()).collect::<Vec<_>>(),
            })));
        }
        print_pending_prizes(&page);
        return Ok(None);
    }

    if args.raw {
        // 排障路径：**不解析、不落库**。
        //
        // 不落库是有意的 —— 原始响应没被解析成 `cloudTotal`，记一条"余额未知"
        // 的历史只会往趋势图里塞噪声。`--raw` 是拿来看的，不是拿来攒数据的。
        let raw = signin::status_raw(&upstream, &profile, &credential, &SystemClock, rng)
            .await
            .map_err(core_err)?;
        if args.json {
            return Ok(Some(raw));
        }
        // 人看的时候给缩进版；`to_string_pretty` 对 `Value` 不会失败，
        // 但真失败了也不该让整个命令挂掉 —— 退回紧凑形态，信息一点不少。
        println!(
            "{}",
            serde_json::to_string_pretty(&raw).unwrap_or_else(|_| raw.to_string())
        );
        return Ok(None);
    }

    if args.status_only {
        let status = signin::status(&upstream, &profile, &credential, &SystemClock, rng)
            .await
            .map_err(core_err)?;
        // 采集放在 `--json` 判断**之前**：落库与"这次怎么打印"无关，
        // json 模式同样该攒下余额历史，否则脚本调用的那些天在趋势图上是空的。
        // ⚠ 记的是 **`bean_total`（上游 `total`）**，不是 `cloud_total`（`beforeTotal`）——
        // 后者不随领取变化，记它等于让趋势图永远是一条直线。
        // 这一对数字有实拍对照，见 `mcloud_app::cloud::record_cloud` 的说明。
        let cloud_note = record_cloud_note(store, scope, account_id, status.bean_total).await;
        if args.json {
            return Ok(Some(signin_status_json(&status, None)));
        }
        print_signin_status(&status);
        if let Some(note) = cloud_note {
            println!("{note}");
        }
        return Ok(None);
    }

    let report = signin::sign_in(
        &upstream,
        &profile,
        &credential,
        &signin::SignInOptions {
            device_id: h5_device_id.as_deref(),
            force: args.force,
        },
        &SystemClock,
        rng,
    )
    .await
    .map_err(core_err)?;

    // 记 `after` 而不是 `before`：两者在余额上应当相等
    // （签到只是打卡，豆要另外领 —— 实测签到前后 `total` 不变），
    // 取 `after` 是"更晚读到的那次"。
    let cloud_note = record_cloud_note(store, scope, account_id, report.after.bean_total).await;

    // ⚠ 两个写开关（`--receive` / `--revive`）**必须跑在 `--json` 判断之前**。
    //
    // 放在后面的话，`--json --receive` 会**静默什么都不做** —— 而静默空操作比报错
    // 难查得多：定时任务会以为领过了，日志里却一个字都没有。结果先收进 `extras`，
    // 两种输出各自消费（人看的输出**仍然排在签到结论之后**，见下面的循环）。
    let mut extras: Vec<(&'static str, WriteOutcome)> = Vec::new();

    if args.receive {
        // 领取会**再建一次会话**（`open_session` 在 `sign_in` 内部，没暴露出来）。
        // 自检场景下多两次请求无所谓 —— 见 `signin::receive_all` 的说明。
        //
        // 失败**不用 `?` 抛出去**：这个开关的用途恰恰是"看上游到底回什么码"，
        // 抛出去就只剩一句错误信息，反而看不到原始 code。
        let device = h5_device_id.as_deref().unwrap_or("");
        let device_note = match (device.is_empty(), args.device_id.is_some()) {
            (true, _) => "为空".to_string(),
            (false, true) => "用 --device-id 给的".to_string(),
            (false, false) => format!(
                "用库里/新上报的（{}…）",
                device.chars().take(8).collect::<String>()
            ),
        };
        let (head, json, tail) = if args.cloud_type.is_some() {
            // ── 显式模式（自检用）：我说领哪一笔就领哪一笔 ──────────────────
            let head = format!(
                "指定领取：再发一次 receiveV3（cloudId={:?}，cloudType={:?}，deviceId {}）",
                args.cloud_id, args.cloud_type, device_note
            );
            match signin::receive_cloud(
                &upstream,
                &profile,
                &credential,
                &signin::ReceiveTarget {
                    cloud_id: &args.cloud_id,
                    cloud_type: args.cloud_type,
                    device_id: device,
                },
                &SystemClock,
                rng,
            )
            .await
            {
                Ok(result) => (
                    head,
                    serde_json::json!({ "ok": true, "mode": "explicit", "received": result.receive }),
                    format!("  上游回的 result.receive = {} 豆", result.receive),
                ),
                Err(err) => (
                    head,
                    serde_json::json!({ "ok": false, "mode": "explicit", "error": err.to_string() }),
                    format!("  领取失败：{err}"),
                ),
            }
        } else {
            // ── 默认：按上游的账目**自己判断**该领哪些 ────────────────────────
            //
            // 这是日常该走的路。把"该领哪个"交给用户，等于让用户去猜 `cloudType`
            // 的含义，而猜错的表现是"领到 0 朵"——看起来像上游没发。
            let head = format!("按 infoV3 的账目自动分流领取（deviceId {device_note}）");
            match signin::receive_all(&upstream, &profile, &credential, device, &SystemClock, rng)
                .await
            {
                Ok(outcomes) => {
                    let json = serde_json::json!({
                        "ok": true,
                        "mode": "auto",
                        "items": outcomes.iter().map(receive_outcome_json).collect::<Vec<_>>(),
                    });
                    let lines: Vec<String> = outcomes.iter().map(receive_outcome_line).collect();
                    let tail = if lines.is_empty() {
                        "  账目里没有可处理的条目（`receiveList` 为空）".to_string()
                    } else {
                        lines.join("\n")
                    };
                    (head, json, tail)
                }
                Err(err) => (
                    head,
                    serde_json::json!({ "ok": false, "mode": "auto", "error": err.to_string() }),
                    format!("  领取失败：{err}"),
                ),
            }
        };
        extras.push((
            "receive",
            WriteOutcome {
                human: format!("{head}\n{tail}"),
                json,
            },
        ));
    }

    if args.revive {
        // ⚠⚠ **不可逆** —— H5 原话「关闭弹窗视为放弃机会，无法补发」。
        // 所以它单独一个开关，绝不跟着签到顺手跑。
        //
        // 与 `--receive` 同样的理由，失败**不用 `?` 抛出去**：
        // 这个开关的用途就是"看上游到底回什么码"。
        //
        // 注意 `device_id` 给的是 `Option`（不 `unwrap_or("")`）——
        // `None` = 照 H5 定义**不带**这个头，与"给了个空串"是两件事。
        // 注意这里**故意不回落**到库里的 `smid_device_id`：H5 的
        // `receiveRevivalReward` 定义里没有 `isDeviceId`，默认就该一个头都不带。
        // 只有显式 `--device-id` 时才补这个头（两个证据不一致，见上游层说明）。
        let device = args.device_id.as_deref();
        let head = format!(
            "再发一次 receiveRevivalReward（用复活卡/回馈卡，deviceid 头{}）",
            if device.is_some() { "已给" } else { "不带" }
        );
        let (json, tail) = match signin::receive_revival_reward(
            &upstream,
            &profile,
            &credential,
            device,
            &SystemClock,
            rng,
        )
        .await
        {
            Ok(reward) => (
                serde_json::json!({
                    "ok": true,
                    "cardType": reward.card_type,
                    "rewardClouds": reward.reward_clouds,
                    "totalClouds": reward.total_clouds,
                    "littleFirstTime": reward.little_first_time,
                }),
                format!(
                    "  用掉一张{}：本次 +{} 豆，累计 {}（littleFirstTime={}）",
                    if reward.card_type == 1 {
                        "复活卡"
                    } else {
                        "回馈卡"
                    },
                    reward.reward_clouds,
                    reward.total_clouds,
                    reward.little_first_time
                ),
            ),
            Err(err) => (
                serde_json::json!({ "ok": false, "error": err.to_string() }),
                format!("  失败：{err}"),
            ),
        };
        extras.push((
            "revive",
            WriteOutcome {
                human: format!("{head}\n{tail}"),
                json,
            },
        ));
    }

    if args.multiple {
        // **写请求，每月只有一次机会。** 门槛由 `claim_multiple` 自己判
        // （`curMonthBackup && !curMonthBackupSignAccept`）—— 不成立就**一个请求都不发**。
        //
        // 失败**不用 `?` 抛出去**：与 `--receive` / `--revive` 同一个理由 ——
        // 这个开关的用途就是"看上游到底回什么码"。
        let (json, tail) =
            match signin::claim_multiple(&upstream, &profile, &credential, &SystemClock, rng).await
            {
                Ok(outcome) => (
                    serde_json::json!({
                        // `ok` = "这次调用没出错"，**不等于"领到了"** ——
                        // 三种"按逻辑就没领"（已领过 / 没开备份 / 判不了）也是 `ok: true`。
                        // 领没领到看 `claimed`。
                        "ok": true,
                        "claimed": matches!(outcome, signin::MultipleOutcome::Claimed { .. }),
                        "outcome": multiple_outcome_json(&outcome),
                    }),
                    multiple_outcome_line(&outcome),
                ),
                Err(err) => (
                    serde_json::json!({ "ok": false, "claimed": false, "error": err.to_string() }),
                    format!("  失败：{err}"),
                ),
            };
        extras.push((
            "multiple",
            WriteOutcome {
                human: format!("领签到翻倍（`multiple`，每月一次）\n{tail}"),
                json,
            },
        ));
    }

    if args.backup_gift {
        // **写请求，每月只有一次机会。** 门槛由 `claim_backup_gift` 自己判
        // （`curMonth` + `state`）—— 不成立就**一个请求都不发**。
        //
        // 失败**不用 `?` 抛出去**：与 `--receive` / `--revive` / `--multiple` 同一个理由 ——
        // 这个开关的用途就是"看上游到底回什么码"。
        let (json, tail) =
            match signin::claim_backup_gift(&upstream, &profile, &credential, &SystemClock, rng)
                .await
            {
                Ok(outcome) => (
                    serde_json::json!({
                        // 与 `--multiple` 同一套约定：`ok` = "这次调用没出错"，
                        // **不等于"领到了"** —— 三种"按逻辑就没领"也是 `ok: true`。
                        "ok": true,
                        "claimed": matches!(outcome, signin::BackupGiftOutcome::Claimed { .. }),
                        "outcome": backup_gift_outcome_json(&outcome),
                    }),
                    backup_gift_outcome_line(&outcome),
                ),
                Err(err) => (
                    serde_json::json!({ "ok": false, "claimed": false, "error": err.to_string() }),
                    format!("  失败：{err}"),
                ),
            };
        extras.push((
            "backupGift",
            WriteOutcome {
                human: format!("领备份好礼（`backupgift`，每月一次）\n{tail}"),
                json,
            },
        ));
    }

    if args.json {
        let mut value = signin_status_json(&report.after, Some(&report));
        if !extras.is_empty() {
            let object = value
                .as_object_mut()
                .expect("signin_status_json 一定返回对象");
            for (key, extra) in &extras {
                object.insert((*key).to_string(), extra.json.clone());
            }
        }
        // 打不打由调用方定：多账号时它要把每个账号的结果合成数组再输出。
        return Ok(Some(value));
    }

    // 三种结局必须**分开报** —— 它们在日志里长得一样，但含义完全不同：
    // 定时任务连跑一个月，只有分开才知道它到底有没有在干活。
    if report.signed_now() {
        match report.sign_in_points() {
            Some(points) => println!("签到成功：+{points} 豆"),
            None => println!("签到成功"),
        }
    } else if report.was_already_signed() {
        if report.result.is_some() {
            println!(
                "今天本来就已签；按 --force 重发了一次 startSignIn，上游接受（协议自检通过）。"
            );
        } else {
            println!("今天已经签过了，本次没有重复提交。");
        }
    } else {
        // 既不是"刚签上"、也不是"本来就签过"，却还能走到这里 —— 不该发生。
        // 宁可打一句明确的异常，也不要静默按"成功"糊过去。
        println!("⚠ 本次既未签上、也没有「今天已签」的凭据，请检查 infoV3 的返回值。");
    }
    print_signin_status(&report.after);
    if let Some(note) = cloud_note {
        println!("{note}");
    }
    // 写开关的输出排在签到结论**之后** —— 它们是在上面已经发生过的动作，
    // 但"这次签到怎么样"才是这一屏的主角。
    for (_, extra) in &extras {
        println!("{}", extra.human);
    }
    Ok(None)
}

/// `--receive` / `--revive` 这类**写开关**的结果。
///
/// 两处都要用：人看的输出是一段文本，`--json` 要的是结构化字段。
/// 打包在一起是为了让"结果在哪个分支被消费"变得显式 —— 见 `signin_one` 里的说明。
struct WriteOutcome {
    /// 给人看的那几行（含"正在做什么"的前缀）。
    human: String,
    /// 塞进 `--json` 输出的那个对象。
    json: serde_json::Value,
}

/// 把这次读到的余额落库，返回要打印的那一行（`None` = 没成功，已经警告过了）。
///
/// **失败不影响签到结论**：余额是顺带采集的副产品，它写不进去不该把一次成功的签到
/// 变成失败（规划 §3.4："多源取可信值，失败降级，不阻塞主流程"）。
/// 但也**不能静默** —— 悄悄不采集的后果是趋势图上少一段，而那时没人会想到
/// 是几个月前某次写库失败。所以退化成一句 `stderr` 警告。
///
/// 用 `eprintln` 而不是 `println`：这行不是签到结果的一部分，
/// 有人把 stdout 重定向成日志时不该混进去。
async fn record_cloud_note(
    store: &SqliteStore,
    scope: Scope<'_>,
    account_id: &AccountId,
    count: i64,
) -> Option<String> {
    match cloud::record_cloud(store, scope, account_id, count, &SystemClock).await {
        Ok(stats) => Some(format!(
            "  余额已记录：{} 豆（日增 {:+}，周增 {:+}）",
            stats.cloud_count, stats.cloud_diff, stats.cloud_diff_week
        )),
        Err(err) => {
            eprintln!("⚠ 余额没能落库（不影响本次签到）：{err}");
            None
        }
    }
}

/// `--records` 每档取多少条。
///
/// 50 是**只用来对账**的量级：H5 自己一页 10 条、滚动加载，
/// 而这里是要"一眼看完整段近期流水"，翻页反而碍事。
const RECORD_PAGE_SIZE: i64 = 50;

/// `--records-all` 最多翻多少页 —— 纯粹是防"上游 `total` 恒不回落到 0"那种死循环。
///
/// 按 `RECORD_PAGE_SIZE` 算，20 页 = 1000 条，比实测最大的账本（460 条）宽裕一倍。
const RECORD_MAX_PAGES: i64 = 20;

/// 一档账本的一页流水 → 人看的几行。
///
/// ⚠ 条目**原样打**（一条一行 compact JSON）：流水里的字段名**没有实拍**
/// （H5 只把它丢给模板渲染，从没读过具体字段名）。假装知道哪个是金额、
/// 哪个是时间，只会把猜测写进输出里 —— 而那正是"看起来很像真的"的错误。
fn print_cloud_records(label: &str, book: i64, page: &signin::CloudRecords) {
    let total = match page.total {
        Some(total) => total.to_string(),
        // `None` ≠ 0：上游没给，不是"一条都没有"。
        None => "上游没给".to_string(),
    };
    println!(
        "  {label}（aiDou={book}）：共 {total} 条，本页 {} 条",
        page.records.len()
    );
    if page.records.is_empty() {
        // 空**不是错**：改名前的账本本来就只到 2026-07-30。
        println!("    （本页没有记录）");
        return;
    }
    for record in &page.records {
        println!("    {record}");
    }
    // 单独把「备份翻倍奖励」挑出来再列一遍。
    //
    // 理由：这一条是**唯一能解释 `receiveList[cloudType=2]` 那个数从哪来的**凭证 ——
    // 它是**按月结算**的（次月 1 日入账），而整页流水是几百条按时间倒序的杂项，
    // 那几笔混在里面根本找不着。见 `docs/capture/app-signin-samples.txt` §3.7。
    let backup: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.get("type").and_then(serde_json::Value::as_str) == Some("backup_1"))
        .collect();
    if !backup.is_empty() {
        println!("    ── 其中「备份翻倍奖励」（type=backup_1，按月结算）──");
        for record in backup {
            println!("    {record}");
        }
    }
}

/// `--prizes` 取多少条。
///
/// 与 H5 一致（它写死 `pageSize: 10`）。这条的用途是"看有没有待领的"，
/// 不是把历史全拉下来 —— 一页足够，而且**正好与 H5 的分页判据对齐**
/// （`records.length < pageSize` ⇒ 到底了）。
const PRIZE_PAGE_SIZE: i64 = 10;

/// 待领取奖品的一页 → 人看的几行。
///
/// # 只打认得出的字段
///
/// `prizeName` / `insertTime` / `expireTime` 都有**实拍**（2026-09-19 账号 159），
/// 所以打它们。**过期时间才是要盯的那个** —— 奖品躺着会过期，而 H5 的列表
/// 只显示中奖时间，看不出还剩几天。
fn print_pending_prizes(page: &signin::PrizeRecords) {
    if page.records.is_empty() {
        // 空**不是错**：待领取本来就常常是空的（账号 1 实测就是空的）。
        println!("  没有待领取的奖品。");
        return;
    }
    println!("  待领取奖品 {} 个：", page.records.len());
    for item in &page.records {
        let name = item.prize_name.as_deref().unwrap_or("（上游没给奖品名）");
        let insert = item
            .insert_time
            .as_deref()
            .map(|time| format!("中奖 {time}"))
            .unwrap_or_else(|| "中奖时间未给".to_string());
        let expire = item
            .expire_time
            .as_deref()
            .map(|time| format!("过期 {time}"))
            .unwrap_or_else(|| "过期时间未给".to_string());
        println!("    {name}（{insert}，{expire}）");
    }
    // 与 `--records` 同一策：认不出的字段**原样带出来**，不替上游编名字。
    println!("    ── 原始条目（我们只认得出上面那几个字段）──");
    for item in &page.records {
        println!("    {}", item.raw);
    }
}

/// 打印签到状态。
///
/// **不把整份 `cal` 倒出来**：35 个格子对排障没有帮助，而"今天签没签"
/// 与"本月已签几天"才是要看的两件事。
///
/// # 单位叫「豆」，不叫「朵」
///
/// 云朵与「云盘专属 AI豆」是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
/// 所以这里不是"换算"，是**同一个量的两种叫法**。界面（H5）上写的是「豆」，
/// 输出跟着界面走 —— 用户对着 App 核数时才不会以为差了什么东西。
fn print_signin_status(status: &SignInStatus) {
    let today = match status.signed_today() {
        Some(true) => "已签",
        Some(false) => "未签",
        // 服务端没把任何一格标成 `t` 时才会走到这里。这时**不能**猜，
        // 因为"按日期裸查"会命中上/下月的补位格（`cal` 里真的混着它们）。
        None => "未知（服务端没标出今天是哪一格）",
    };
    println!("  今天：{today}");
    println!("  豆余额：{}", status.bean_total);
    // 两个口径不一致时**把另一个也说出来** —— 不说的话，看到 `--raw` 的人
    // 会以为我们漏读了一个字段。
    //
    // 说清它是什么：`beforeTotal` 是**2026-07-31 改名时结转进来的那笔**
    // （流水里 `mark: "shouTiaoAiDou"`「云朵转换AI豆」），是一张**冻结的快照**，
    // 所以它永远不变、也永远不等于余额。见 `SignInStatus::cloud_total`。
    //
    // 上游没给这个字段（`None`）就**不提**：那是"没说"，不是"是 0"。
    if let Some(cloud_total) = status.cloud_total
        && cloud_total != status.bean_total
    {
        println!(
            "  （另有一个数 {cloud_total}：改名时结转进来的那笔，不是余额 —— 见 `--raw` 的 beforeTotal）"
        );
    }
    println!("  连续签到：{} 天", status.sign_count);
    println!(
        "  本月已签：{} 天（服务端报 {}）",
        status.counted_month_days(),
        status.month_days
    );
    println!("  本次可得：{} 豆", status.sign_in_points);
    // 有才打这一行 —— 天天打一句"没有"是纯噪音。
    //
    // ⚠ **"在 `receiveList` 里" ≠ "现在能领"**：能不能领由 `cloudType` 决定，
    // 判据见 [`cloud_type_label`]。这里曾经一律写成"可领取：cloudType=2 × 154 朵"，
    // 而 `2` 恰恰是**下月可领取** —— 一句话把"领不了"说成了"能领 154 朵"。
    if !status.receive_list.is_empty() {
        let items: Vec<String> = status
            .receive_list
            .iter()
            .map(|item| {
                format!(
                    "{} {} 豆（cloudType={}）",
                    cloud_type_label(item.cloud_type),
                    item.cloud_num,
                    item.cloud_type
                )
            })
            .collect();
        println!("  可领取账目：{}", items.join("，"));
    }
    // 「自动备份」的门槛 —— `--multiple`（签到翻倍）与月度奖励都靠它。
    //
    // 三态要分得开：开了 / 没开 / **上游没说**。混成一个 bool 的话，
    // "判不了"会被读成"没开"，然后有人去追一个不存在的备份问题。
    let backup = match (status.cur_month_backup, status.cur_month_backup_sign_accept) {
        (Some(true), Some(false)) => "本月已开启（翻倍机会还没领，可 `--multiple`）".to_string(),
        (Some(true), Some(true)) => "本月已开启（翻倍机会本月已领过）".to_string(),
        (Some(true), None) => "本月已开启（翻倍机会领没领，上游没说）".to_string(),
        (Some(false), _) => "本月未开启 —— 翻倍奖励与月度奖励都拿不到".to_string(),
        (None, _) => "上游没给 `curMonthBackup`，判不了".to_string(),
    };
    println!("  自动备份：{backup}");
}

/// `receiveList` 里那条 `cloudType` 的人话 —— **"现在能不能领、怎么领"由它决定**。
///
/// 判据是当前版 H5 源码 `newsignin.336f18c3.js` 的点击分流（逐字见
/// `mcloud_upstream::app_signin::ReceiveItem::cloud_type`）：
///
/// | 值 | 含义 | 动作 | 生效 |
/// | --- | --- | --- | --- |
/// | `0` | **当前可领取**（`toReceive` 就是它的合计）| `receiveV3` | **直接生效** |
/// | `1` | **月度任务奖励**（上月攒下的），**需开启自动备份** | `receiveTaskExpansion(acceptDate)` | 按结算日 |
/// | `2` | **下月**才可领 | 什么都不发 | — |
/// | 其它 | 当前可领取 | `receiveV3` | 直接生效 |
///
/// `0` 与"其它"走同一条路（H5 的 `else` 分支就是 `receiveV3`）——
/// **`0` 是合法取值**，不是"上游没给"。写成 `?? ""` 的是**缺字段**那种情形。
///
/// ⚠ `1` 与 `2` 是**同一个特性的两个时点**，不是两种奖励：
/// `2` 是"本月攒的、下月才能领"的预告；等它到期就变成 `1`（那时才带 `acceptDate`）。
/// 旧项目 `mcloud-sign` 走的**老路径**把这两个数分别叫
/// `curMonthTaskRecordCount`（`>0` 就领）与 `nextMonthTaskRecordCount`（只打印）——
/// 与这里 `1 → 领` / `2 → 不发` 一一对应。
///
/// # 实测（2026-09-19，账号 159）
///
/// 账目 `[{cloudType:2, cloudNum:240}, {cloudType:0, cloudNum:6}]`、`toReceive: 6`：
/// 自动分流跳过了 240（下月），领掉了 6（当前）——
/// 领完 `toReceive` 变 `0`、`cloudType:0` 那条**从账目里消失**，而 `cloudType:2` 的 240 一动没动。
///
/// ⇒ 由此也定死了 `toReceive` 的语义：**"现在就能领"的合计**（`0` 那类之和），
/// 不含 `1` / `2` 那条线。`--raw` 打出来的原始响应里 `total` 是**现行余额**（豆），
/// 与 `toReceive` 不是一回事。
fn cloud_type_label(cloud_type: i64) -> &'static str {
    match cloud_type {
        0 => "可领取（立即生效）",
        1 => "月度可领取（需开启备份）",
        2 => "下月可领取",
        _ => "可领取（立即生效）",
    }
}

/// 一条领取实况 → 人看的一行。
///
/// 三种"没领到"要分得开，否则全都长得像"上游没发"：
/// - [`signin::ReceiveOutcome::NextMonth`]：**按逻辑就不该领**（下月才作数）；
/// - [`signin::ReceiveOutcome::MonthlyExpansion`] 里 `accept_date: None`：
///   **发了裸请求**（上游没给结算日，与真实客户端一致）；
/// - [`signin::ReceiveOutcome::Failed`]：**发了但失败**（例如月度那条回 `1302` 未开备份）。
fn receive_outcome_line(outcome: &signin::ReceiveOutcome) -> String {
    use signin::ReceiveOutcome as R;
    match outcome {
        R::NextMonth { cloud_num } => format!(
            "  跳过 {} 豆：下月才可领（`cloudType=2`，官方 H5 也只弹提示、不发请求）",
            cloud_num
        ),
        R::MonthlyExpansion {
            accept_date,
            cloud_count,
        } => {
            // 没带结算日要说出来：结果可能对不上，但**请求确实发了**。
            // 这里曾经写成"跳过不发"，那是与真实客户端分叉的说法。
            let date = match accept_date {
                Some(date) => format!("结算日 {date}"),
                None => "**没带结算日**（上游没给，照 H5 发的是裸请求）".to_string(),
            };
            match cloud_count {
                Some(count) => format!("  月度奖励领取成功（{date}）：+{count} 豆"),
                // `None` ≠ 0：上游没报数，不是"领到 0 朵"。
                None => format!("  月度奖励已提交（{date}）：上游没报领到多少"),
            }
        }
        R::Immediate {
            cloud_type,
            received,
        } => format!("  立即领取（cloudType={cloud_type}）：+{received} 豆"),
        R::Failed { cloud_type, error } => {
            format!("  领取失败（cloudType={cloud_type}）：{error}")
        }
    }
}

/// 同上，给 `--json` 用的结构化形状。
fn receive_outcome_json(outcome: &signin::ReceiveOutcome) -> serde_json::Value {
    use signin::ReceiveOutcome as R;
    match outcome {
        R::NextMonth { cloud_num } => {
            serde_json::json!({ "action": "nextMonth", "cloudNum": cloud_num })
        }
        R::MonthlyExpansion {
            accept_date,
            cloud_count,
        } => serde_json::json!({
            "action": "monthlyExpansion",
            "acceptDate": accept_date,
            "cloudCount": cloud_count,
        }),
        R::Immediate {
            cloud_type,
            received,
        } => serde_json::json!({
            "action": "immediate",
            "cloudType": cloud_type,
            "received": received,
        }),
        R::Failed { cloud_type, error } => serde_json::json!({
            "action": "failed",
            "cloudType": cloud_type,
            "error": error,
        }),
    }
}

/// 一条签到翻倍结果 → 人看的一行。
///
/// 四种"没领到"要分得开，否则全都长得像"上游没发"：
/// - [`signin::MultipleOutcome::AlreadyAccepted`]：**本月已经领过了**（每月一次，正常）；
/// - [`signin::MultipleOutcome::NoBackup`]：**本月没开自动备份**，机会根本没产生；
/// - [`signin::MultipleOutcome::Unknown`]：**上游没给门槛字段**，判不了，所以没发；
/// - [`signin::MultipleOutcome::Failed`]：**发了但失败**（例如回 `1302`）。
fn multiple_outcome_line(outcome: &signin::MultipleOutcome) -> String {
    use signin::MultipleOutcome as M;
    match outcome {
        // `multiple` 是**倍数**、`cloud_count` 是**合计** —— 旧项目那句话就是
        // `"成功获得", multiple, "倍云朵，共计", cloudCount`。两者都可能缺，
        // 所以分开拼：缺倍数就只说合计，缺合计就只说倍数。
        M::Claimed {
            cloud_count,
            multiple,
        } => match (multiple, cloud_count) {
            (Some(multiple), Some(count)) => {
                format!("  签到翻倍领取成功：{multiple} 倍，共计 +{count} 豆")
            }
            (None, Some(count)) => format!("  签到翻倍领取成功：+{count} 豆（上游没报倍数）"),
            (Some(multiple), None) => {
                format!("  签到翻倍已领取：{multiple} 倍（上游没报合计）")
            }
            // `None` ≠ 0：上游什么都没报，不是"领到 0 豆"。
            (None, None) => "  签到翻倍已领取：上游没报倍数也没报合计".to_string(),
        },
        M::AlreadyAccepted => "  跳过：本月已经领过翻倍机会（每月只有一次）".to_string(),
        M::NoBackup => "  跳过：本月未开启自动备份，翻倍机会没产生。\n  \
                        ⚠ 旧项目会上传一个文件把备份打开再重试，我们还没有上传能力 ——\n  \
                        去 App 里随便备份一个文件，下个月起就有了。"
            .to_string(),
        M::Unknown => "  跳过：`infoV3` 没给 `curMonthBackup` / `curMonthBackupSignAccept`，\
                       判不了，故一个请求都没发"
            .to_string(),
        M::Failed { error } => format!("  领取失败：{error}"),
    }
}

/// 同上，给 `--json` 用的结构化形状。
fn multiple_outcome_json(outcome: &signin::MultipleOutcome) -> serde_json::Value {
    use signin::MultipleOutcome as M;
    match outcome {
        M::Claimed {
            cloud_count,
            multiple,
        } => serde_json::json!({
            "action": "claimed",
            "multiple": multiple,
            "cloudCount": cloud_count,
        }),
        M::AlreadyAccepted => serde_json::json!({ "action": "alreadyAccepted" }),
        M::NoBackup => serde_json::json!({ "action": "noBackup" }),
        M::Unknown => serde_json::json!({ "action": "unknownGate" }),
        M::Failed { error } => serde_json::json!({ "action": "failed", "error": error }),
    }
}

/// 领备份好礼的一条结果 → 人看的一行。
///
/// 四档"没领"必须分得开 —— 它们在日志里都长得像"没动静"，但下一步该做什么完全不同：
/// - [`signin::BackupGiftOutcome::AlreadyReceived`]：**本月已领过**，等下次；
/// - [`signin::BackupGiftOutcome::NoBackup`]：**未开备份**，去 App 开；
/// - [`signin::BackupGiftOutcome::NothingThisMonth`]：**本月没有这个活动**，什么都不用做；
/// - [`signin::BackupGiftOutcome::Failed`]：**发了但失败**，要看错误码。
fn backup_gift_outcome_line(outcome: &signin::BackupGiftOutcome) -> String {
    use signin::BackupGiftOutcome as B;
    match outcome {
        B::Claimed { received } => match received {
            Some(received) => format!("  备份好礼领取成功：+{received} 豆"),
            // `None` ≠ 0：上游没说领到多少。
            None => "  备份好礼已领取：上游没报领到多少".to_string(),
        },
        B::AlreadyReceived => "  跳过：本月已经领过备份好礼（每月只有一次）".to_string(),
        B::NoBackup => "  跳过：未开启自动备份（`state=-1`）——\n  \
                        去 App 里开启自动备份，之后每月才有这份礼。"
            .to_string(),
        B::NothingThisMonth => {
            "  跳过：本月没有备份好礼可领（`curMonth` 为假）—— 这**不是**未开备份".to_string()
        }
        B::Failed { error } => format!("  领取失败：{error}"),
    }
}

/// 同上，给 `--json` 用的结构化形状。
fn backup_gift_outcome_json(outcome: &signin::BackupGiftOutcome) -> serde_json::Value {
    use signin::BackupGiftOutcome as B;
    match outcome {
        B::Claimed { received } => {
            serde_json::json!({ "action": "claimed", "received": received })
        }
        B::AlreadyReceived => serde_json::json!({ "action": "alreadyReceived" }),
        B::NoBackup => serde_json::json!({ "action": "noBackup" }),
        B::NothingThisMonth => serde_json::json!({ "action": "nothingThisMonth" }),
        B::Failed { error } => serde_json::json!({ "action": "failed", "error": error }),
    }
}

/// 状态 → JSON。
///
/// `signedDays` 与 `monthDays` 是**两条独立路径**算出来的同一个数：
/// 前者自己数 `cal`，后者是服务端报的。两者不一致就是上游数据有问题，
/// 摆在同一个 JSON 里比塞进一句自然语言里更容易被脚本发现。
fn signin_status_json(
    status: &SignInStatus,
    report: Option<&signin::SignInReport>,
) -> serde_json::Value {
    let signed_days: Vec<u8> = status
        .days
        .iter()
        .filter(|day| day.current_month && day.signed)
        .map(|day| day.day)
        .collect();

    let mut out = serde_json::json!({
        "signedToday": status.signed_today(),
        // ⚠ `beanTotal` 才是**余额**（上游 `total`）。`cloudTotal` 保留是因为它一直在这儿，
        // 但它装的是上游 `beforeTotal` —— 云朵明细（历史）页口径的另一个数，
        // **不随领取变化**。别拿它画趋势，见 `mcloud_app::cloud::record_cloud`。
        "beanTotal": status.bean_total,
        "cloudTotal": status.cloud_total,
        "signCount": status.sign_count,
        "signInPoints": status.sign_in_points,
        "monthDays": status.month_days,
        "countedMonthDays": status.counted_month_days(),
        "signedDays": signed_days,
        // `receiveList` 是 `--receive` 自动分流的取值来源；
        // `recordId` 实拍里没有，所以这里如实给 `null` 而不是编一个空串。
        //
        // `acceptDate` 只有 `cloudType=1`（月度那条线）才用得上 —— 但**要打出来**：
        // 少了它就只能干瞪眼（请求发不出去），而"为什么发不出去"必须看得见。
        "receiveList": status.receive_list.iter().map(|item| serde_json::json!({
            "cloudType": item.cloud_type,
            "cloudNum": item.cloud_num,
            "recordId": item.record_id,
            "acceptDate": item.accept_date,
        })).collect::<Vec<_>>(),
        "toReceive": status.to_receive,
        // 「自动备份」的两条门槛 —— 它们是 `--multiple` 与月度奖励（`cloudType=1`）
        // 能不能领的前提。**不给的话，"为什么没领到"就只能靠猜。**
        //
        // `null` = 上游没给这个字段，**不是 `false`** —— 见
        // `SignInStatus::cur_month_backup`。两者对 `--multiple` 的处置完全不同。
        "curMonthBackup": status.cur_month_backup,
        "curMonthBackupSignAccept": status.cur_month_backup_sign_accept,
    });
    if let Some(report) = report {
        out["signedNow"] = serde_json::json!(report.signed_now());
        // 与 `signedNow` 成对：只有分开给，"定时任务到底有没有在干活"才判得出来。
        out["alreadySigned"] = serde_json::json!(report.was_already_signed());
        // 签到前的余额。**叫 `beanBefore` 而不是 `cloudBefore`** —— 改名是有意的：
        // 这个名字以前装的是上游 `beforeTotal`（那个不随领取变动的数），
        // 叫"before"会让人以为它是"签到前的余额"。改名比留一个静默错的键好。
        out["beanBefore"] = serde_json::json!(report.before.bean_total);
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// 设备特征（smid）
// ═══════════════════════════════════════════════════════════════════════════

async fn smid_show(db: &Path) -> Result<()> {
    let store = open_local(db)?;
    let accounts = store.list_accounts(Scope::Local).await.map_err(core_err)?;
    if accounts.is_empty() {
        println!("还没有云盘账号。先跑：mcloud login qr");
        return Ok(());
    }

    let mut bound = 0;
    for account in &accounts {
        let title = match account.nickname.as_deref() {
            Some(nick) => format!("{}（{nick}）", account.id.as_str()),
            None => account.id.as_str().to_string(),
        };
        println!("{title}");

        let state = store
            .get_smid_state(Scope::Local, &account.id)
            .await
            .map_err(core_err)?;
        match state.as_ref().and_then(|s| s.fingerprint_json.as_deref()) {
            Some(raw) => {
                bound += 1;
                println!("  设备特征  {}", fingerprint_line(raw));
            }
            None => println!(
                "  设备特征  未绑定（下一次 `mcloud signin` 会自动绑一个并上报，\
                 也可以现在就做：`mcloud smid refresh --account …`）"
            ),
        }
        match state.and_then(|s| s.device_id) {
            Some(id) => println!("  deviceId  {}", mask_device_id(&id)),
            None => println!("  deviceId  还没有（上报后由风控服务给）"),
        }
    }
    println!("\n共 {} 个账号，已绑定设备特征 {bound} 个", accounts.len());
    Ok(())
}

async fn smid_reset(db: &Path, account: &str, report: bool) -> Result<()> {
    let store = open_local(db)?;
    let id = require_account(&store, account).await?;

    let mut rng = SystemRandom;
    let binding = smid::rotate_fingerprint(&store, Scope::Local, &id, &mut rng)
        .await
        .map_err(core_err)?;
    let json = binding.fingerprint.to_json_string().map_err(core_err)?;
    println!("已换上新的设备特征：{}", fingerprint_line(&json));
    println!("旧的 deviceId 同时作废（新长相配旧 id 会被认成同一台设备在撒谎）");

    if report {
        let device_id = report_device_id(&store, &id).await?;
        println!("已上报，新的 deviceId：{}", mask_device_id(&device_id));
    } else {
        println!("没有上报：下一次真正用到 deviceId 时会自动换一个新的");
    }
    Ok(())
}

async fn smid_refresh(db: &Path, account: &str) -> Result<()> {
    let store = open_local(db)?;
    let id = require_account(&store, account).await?;
    let device_id = report_device_id(&store, &id).await?;
    println!("已上报，新的 deviceId：{}", mask_device_id(&device_id));
    Ok(())
}

/// 上报换一个新的 `deviceId`（特征不变 —— 换特征走 `smid_reset`）。
async fn report_device_id(store: &SqliteStore, id: &AccountId) -> Result<String> {
    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);
    let mut rng = SystemRandom;
    smid::report_device_id(&upstream, store, Scope::Local, id, &SystemClock, &mut rng)
        .await
        .map_err(core_err)
}

/// 解析签到链路要用的那个 H5 `deviceId`（89 字符 base64、`B` 前缀）。
///
/// 顺序：**显式给的** → 库里存的 `smid_device_id`（没有就上报一次换一个）。
///
/// # 为什么两条路都不需要浏览器
///
/// 第二条走 [`mcloud_app::smid`]，它打的
/// `POST https://slw.h5cmpassport.com:9090/deviceprofile/v4` 与签到页
/// `fp.min.js` 是**同一个接口**：host、`organization`、`appId`、RSA 公钥
/// 逐字相同（签到页 HTML 里的 `window._smConf` 就是这份配置）。
/// 两边拿回来的都是 `"B" + detail.deviceId`，**同一个东西**。
///
/// ⇒ 以前那条"先用官方 App 打开一次签到页、再拿 `scripts/h5-device-id.py`
/// 从 WebView 的 `localStorage` 里读"的路子**不再需要**。
async fn resolve_device_id<C: mcloud_core::HttpClient>(
    upstream: &Upstream<'_, C>,
    store: &SqliteStore,
    scope: Scope<'_>,
    account_id: &AccountId,
    explicit: Option<&str>,
    rng: &mut impl Random,
) -> Result<String, CoreError> {
    match explicit {
        Some(id) => Ok(id.to_string()),
        None => smid::device_id(upstream, store, scope, account_id, &SystemClock, rng).await,
    }
}

/// `--account` 必须指向一个已有的账号。
///
/// 打错一个字就会凭空给一个不存在的账号绑上特征 —— 那一行永远没人读得到，
/// 只会留在库里让人困惑。所以查不到就直接报错，并把现有账号列出来。
async fn require_account(store: &SqliteStore, account: &str) -> Result<AccountId> {
    let id = AccountId::new(account);
    if store
        .get_account(Scope::Local, &id)
        .await
        .map_err(core_err)?
        .is_some()
    {
        return Ok(id);
    }
    let known: Vec<String> = store
        .list_accounts(Scope::Local)
        .await
        .map_err(core_err)?
        .iter()
        .map(|a: &Account| a.id.as_str().to_string())
        .collect();
    bail!(
        "没有账号 {account}（库里现有：{}）",
        if known.is_empty() {
            "还没有账号".to_string()
        } else {
            known.join(", ")
        }
    )
}

/// 设备特征的**摘要**行：能认出"是哪台设备"，但不把识别位整份抄进终端。
///
/// `canvas` 那一段就是这份特征的识别位（真机上由渲染结果决定、稳定不变），
/// 所以只进 `fp#` 那个摘要里。UA 不打：本项目只用一个公开常量 UA，
/// 打出来不带任何区分信息（`mcloud-upstream::smid::CAIYUN_ANDROID_WEB_UA`）。
fn fingerprint_line(raw: &str) -> String {
    let digest = &md5_hex(raw.as_bytes())[..8];
    match SmFingerprint::from_json_str(raw) {
        Ok(fp) => format!(
            "fp#{digest}  {} · {}×{} · dpr={} · 触屏 {} 点 · rtt {} · cpu {}",
            fp.platform,
            fp.screen_width,
            fp.screen_height,
            fp.dpr,
            fp.max_touch_points,
            fp.connection_rtt,
            fp.cpucount,
        ),
        Err(_) => {
            format!("fp#{digest}  （库里那串读不出来：`mcloud smid reset --account …` 重新绑定）")
        }
    }
}

/// `deviceId` 的展示形态：只留首尾。
///
/// 它是第三方风控服务给这台设备的标识 —— 完整值留在库里即可，
/// 终端上那一眼通常是被截图/被贴进 issue 的，所以按卡片号的规矩来。
fn mask_device_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    let keep = chars.len().min(4);
    let head = chars.first().copied().unwrap_or('-');
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("{head}…{tail}（{} 字符）", chars.len())
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

    // ── 账号选择（下标语义）─────────────────────────────────────────────
    //
    // 这几条盯的是**用户直接感受到的那一层**：旧项目里账号就是"账号 1 / 账号 2"，
    // 所以"下标几指哪个号"必须稳、错了必须说清楚。用 clap 真解析而不是手搓
    // `SigninArgs`，是为了连"参数之间的约束"一起验掉。

    /// 造一个只有账号、没有用户的临时库。
    ///
    /// 用 [`Scope::Local`] 是刻意的：这里要验的是**下标选择**，与归属无关；
    /// 归属那套（首个管理员认领无主数据）在 io 层有自己的用例。
    async fn store_with_accounts(accounts: &[(&str, bool)]) -> (tempfile::TempDir, SqliteStore) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = SqliteStore::open(dir.path().join("t.db")).expect("建库");
        for (position, (id, enabled)) in accounts.iter().enumerate() {
            let account = Account {
                id: AccountId::new(*id),
                enabled: *enabled,
                sort_order: position as i64,
                ..Account::default()
            };
            store
                .upsert_account(Scope::Local, &account)
                .await
                .expect("写入账号");
        }
        (dir, store)
    }

    fn signin_args(argv: &[&str]) -> SigninArgs {
        // 走**完整的**命令行解析与 [`Cli::into_command`] 合流，而不是只喂给
        // `SigninArgs`：这样子命令名、`conflicts_with_all`、"不带子命令"这条路
        // 全都在这一个入口上被验到。
        //
        // 这里刻意用**顶层写法**（`mcloud --account 2`），因为那正是用户
        // "直接运行"时会敲的形态；`mcloud signin --account 2` 由下面那条
        // 等价性用例覆盖。
        let command =
            Cli::parse_from(std::iter::once("mcloud").chain(argv.iter().copied())).into_command();
        let Command::Signin(args) = command else {
            panic!("解析出来必须是签到");
        };
        args
    }

    /// 顶层写法与 `mcloud signin` 写法**必须等价** ——
    /// 它们共用一个 `SigninArgs`，所以任何差异都说明有人把参数抄成了两份。
    #[test]
    fn top_level_and_subcommand_spellings_agree() {
        let bare =
            Cli::parse_from(["mcloud", "--account", "3", "--status-only", "--json"]).into_command();
        let named = Cli::parse_from([
            "mcloud",
            "signin",
            "--account",
            "3",
            "--status-only",
            "--json",
        ])
        .into_command();
        let (Command::Signin(bare), Command::Signin(named)) = (bare, named) else {
            panic!("两边都该是签到");
        };
        assert_eq!(bare.account, named.account);
        assert_eq!(bare.status_only, named.status_only);
        assert_eq!(bare.json, named.json);
        assert_eq!(bare.db, named.db, "默认库也要一致");
    }

    /// 不带 `--account` 就是全部**启用**的账号，且下标用的是**全列表位置**。
    ///
    /// 停用的那个占掉 2 号位、但不参与执行 —— 这两件事必须同时成立：
    /// 一旦执行时重新编号，同一个数字在不同时刻就会指到不同账号上，
    /// 而 `--account 2` 又走的是全列表位置，两套编号会打架。
    #[tokio::test]
    async fn signin_without_account_runs_every_enabled_account() {
        let (_dir, store) =
            store_with_accounts(&[("19****01", true), ("19****02", false), ("19****03", true)])
                .await;

        let targets = select_signin_targets(&store, Scope::Local, &signin_args(&[]))
            .await
            .expect("空参数应当能选出账号");
        let picked: Vec<(usize, &str)> = targets
            .iter()
            .map(|t| (t.index, t.account_id.as_str()))
            .collect();
        assert_eq!(
            picked,
            vec![(1, "19****01"), (3, "19****03")],
            "停用的 2 号位既不执行、也不被顶替"
        );
    }

    /// `--account N` 用的是**同一个**全列表位置，停用的账号也数进去 ——
    /// 显式指定就照做，否则"下标 = 清单里的行号"这条规则会有例外。
    #[tokio::test]
    async fn signin_account_index_counts_disabled_accounts_too() {
        let (_dir, store) =
            store_with_accounts(&[("19****01", true), ("19****02", false), ("19****03", true)])
                .await;

        let targets =
            select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "2"]))
                .await
                .expect("显式指定停用账号也应当照做");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].account_id.as_str(), "19****02");
        assert_eq!(targets[0].index, 2);
    }

    /// 下标从 1 开始。`--account 0` 一定是搞错了，且要**明确说是从 1 开始** ——
    /// 只回一句"没有这个账号"会让人以为库里是空的。
    #[tokio::test]
    async fn signin_account_index_is_one_based() {
        let (_dir, store) = store_with_accounts(&[("19****01", true)]).await;

        let err = select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "0"]))
            .await
            .expect_err("0 不是合法下标");
        let text = format!("{err}");
        assert!(text.contains("从 1 开始"), "{text}");
    }

    /// 越界时要说清"库里一共几个"，否则用户只能靠猜。
    #[tokio::test]
    async fn signin_account_index_out_of_range_reports_the_total() {
        let (_dir, store) = store_with_accounts(&[("19****01", true), ("19****02", true)]).await;

        let err = select_signin_targets(&store, Scope::Local, &signin_args(&["--account", "9"]))
            .await
            .expect_err("越界");
        let text = format!("{err}");
        assert!(text.contains("超出范围"), "{text}");
        assert!(text.contains('2'), "要报出总数: {text}");
        assert!(text.contains("--list"), "要指出查清单的办法: {text}");
    }

    /// `--phone` 是**逃生口**：它按号码推 id，不查清单，所以空库也能选中 ——
    /// 那正是它的用途（老账号可能压根没进过清单）。下标记 0 表示"没有下标语义"。
    #[tokio::test]
    async fn signin_phone_needs_no_account_row() {
        let (_dir, store) = store_with_accounts(&[]).await;

        let targets = select_signin_targets(
            &store,
            Scope::Local,
            &signin_args(&["--phone", "19900000001"]),
        )
        .await
        .expect("按号码定位不需要库里有账号");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].index, 0, "没有下标语义就别编一个");
        assert_eq!(targets[0].phone_override.as_deref(), Some("19900000001"));
        assert_eq!(
            targets[0].account_id.as_str(),
            account_label(None, "19900000001")
        );
    }

    /// `--list` 是"查了就走"，与"指定跑谁"互斥 —— 让 clap 在解析期就拒掉，
    /// 而不是进到函数里再判一次（那种判法迟早会漏掉一条组合）。
    /// 两种写法都要拒，否则"顶层能用、子命令不能用"这种不一致会留一条口子。
    #[test]
    fn signin_list_conflicts_with_account_and_phone() {
        /// 拼一条 `… --list <extra>` 的命令行（`head` 是"顶层写法"或"子命令写法"）。
        fn list_argv(head: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
            let mut v = head.to_vec();
            v.push("--list");
            v.extend_from_slice(extra);
            v
        }

        for head in [&["mcloud"][..], &["mcloud", "signin"][..]] {
            assert!(Cli::try_parse_from(list_argv(head, &["--account", "1"])).is_err());
            assert!(Cli::try_parse_from(list_argv(head, &["--phone", "19900000001"])).is_err());
            assert!(Cli::try_parse_from(list_argv(head, &[])).is_ok());
        }
    }

    /// 不带子命令直接 `mcloud` 时走的就是签到那条路，且**顶层收全套签到参数**。
    ///
    /// 后者是这条用例真正的价值：早先顶层只认子命令，`mcloud --list` /
    /// `mcloud --status-only` / `mcloud --db X` 会被 clap 判成"未知参数"并报
    /// `Usage: mcloud [COMMAND]` —— 一个主打"直接运行"的命令行却要用户先知道
    /// 子命令叫 `signin` 才能查编号。
    #[test]
    fn bare_invocation_falls_through_to_signin() {
        let cli = Cli::try_parse_from(["mcloud"]).expect("不带子命令也要能解析");
        assert!(cli.command.is_none(), "顶层不该有默认子命令");

        let Command::Signin(args) = Cli::parse_from(["mcloud"]).into_command() else {
            panic!("不带子命令必须落到签到");
        };
        assert_eq!(args.db, PathBuf::from("data/mcloud.db"));
        assert!(
            args.account.is_none() && args.phone.is_none(),
            "零参数 = 全部启用账号"
        );
        assert!(!args.receive && !args.revive, "两个写开关默认都得关");
        // 新增的两个同理 —— 尤其 `--backup-gift` 是**每月一次**的写请求，
        // 默认打开等于每次跑签到都去消耗一次名额。
        assert!(
            !args.multiple && !args.backup_gift && !args.prizes,
            "`--multiple` / `--backup-gift` / `--prizes` 默认都得关"
        );

        // 顶层直接收参数（这一条以前是红的）
        for argv in [
            vec!["mcloud", "--list"],
            vec!["mcloud", "--status-only"],
            vec!["mcloud", "--json"],
            vec!["mcloud", "--db", "/tmp/other.db"],
        ] {
            let cli = Cli::try_parse_from(argv.clone())
                .unwrap_or_else(|err| panic!("{argv:?} 该被接受，却报：{err}"));
            assert!(cli.command.is_none(), "{argv:?} 不该被当成子命令");
        }
    }

    /// 写开关之间**可以叠加**，但它们与只读开关**必须互斥**。
    ///
    /// 为什么这条值得一个测试：`--prizes` 是只读、`--backup-gift` 是写，
    /// 两者都在同一次会话里发请求 —— 叠加起来不会报错，只会让"这次到底干了什么"
    /// 变得说不清。互斥关系是**声明式**的（`conflicts_with_all`），
    /// 写错一个名字 clap 不会报错，只会静默不互斥 —— 所以钉一条。
    #[test]
    fn write_and_read_switches_are_mutually_exclusive() {
        // 写开关之间叠加：允许（与旧项目把它们拆成不同任务是同一回事）。
        for argv in [
            vec!["mcloud", "--receive", "--multiple"],
            vec!["mcloud", "--receive", "--backup-gift"],
            vec!["mcloud", "--multiple", "--backup-gift"],
            vec![
                "mcloud",
                "--receive",
                "--revive",
                "--multiple",
                "--backup-gift",
            ],
        ] {
            Cli::try_parse_from(argv.clone())
                .unwrap_or_else(|err| panic!("{argv:?} 该被接受，却报：{err}"));
        }

        // 只读开关之间、以及只读与写之间：都不许叠。
        for argv in [
            vec!["mcloud", "--prizes", "--receive"],
            vec!["mcloud", "--prizes", "--backup-gift"],
            vec!["mcloud", "--prizes", "--raw"],
            vec!["mcloud", "--prizes", "--records"],
            vec!["mcloud", "--raw", "--backup-gift"],
            vec!["mcloud", "--records", "--backup-gift"],
            vec!["mcloud", "--status-only", "--backup-gift"],
        ] {
            assert!(
                Cli::try_parse_from(argv.clone()).is_err(),
                "{argv:?} 该被拒绝，却通过了"
            );
        }
    }
}
