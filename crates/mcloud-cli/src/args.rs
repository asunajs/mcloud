// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::cli_args::*;
use clap::ValueEnum;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub(crate) struct QrArgs {
    #[arg(long, default_value = "data/mcloud.db")]
    pub(crate) db: PathBuf,
    #[arg(long, value_enum, default_value_t = PlatformArg::Windows)]
    pub(crate) platform: PlatformArg,
    /// 设备指纹文件。**换掉它等于换一台设备**：服务端按 `dID + sID` 定位会话，
    /// 漂移的表现是"二维码扫了、手机也确认了，这边毫无反应"。
    #[arg(long, default_value = "data/device.txt")]
    pub(crate) device_file: PathBuf,
    /// 二维码有效期（秒）。默认 120；把码发给别人扫、或先把码贴出来再拿手机时调大。
    #[arg(long, default_value_t = mcloud_capabilities::login::QR_TTL_SECONDS)]
    pub(crate) ttl_seconds: i64,
    /// 额外把二维码写成图片（SVG）。终端字体等宽不保证，字符画经常会变形扫不出来。
    #[arg(long)]
    pub(crate) qr_out: Option<PathBuf>,
    /// 不等用户扫（只出码就退出，供调试）
    #[arg(long)]
    pub(crate) no_wait: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct SmsArgs {
    /// 手机号
    #[arg(long)]
    pub(crate) phone: String,
    #[arg(long, default_value = "data/mcloud.db")]
    pub(crate) db: PathBuf,
    /// 冒充哪个客户端。**两端不是"同一件事的两种参数"**，见 `--client` 的说明。
    #[arg(long, value_enum, default_value_t = ClientArg::Pc)]
    pub(crate) client: ClientArg,
    /// 仅 PC：`clientType` / `cpid` / 版本号
    #[arg(long, value_enum, default_value_t = PlatformArg::Windows)]
    pub(crate) platform: PlatformArg,
    /// 设备指纹文件。**换掉它等于换一台设备**：服务端按 `dID + sID` 定位会话，
    /// 漂移的表现是"二维码扫了、手机也确认了，这边毫无反应"。
    ///
    /// App 端用 `--app-device-file`（形状不同：无 `-ENDIN` 后缀）。
    #[arg(long, default_value = "data/device.txt")]
    pub(crate) device_file: PathBuf,
    /// 仅 App：`deviceId` 的存放位置（32 位大写 HEX，无后缀）。
    #[arg(long, default_value = "data/device-app.txt")]
    pub(crate) app_device_file: PathBuf,
    /// 只发码、不进入输入验证码这一步，然后退出。
    ///
    /// 发码与登录本来就是两次独立调用，中间隔着"用户去手机上读那条短信"。
    /// 有了它就能把两半分开跑（配合 `--device-short` 钉住设备形态）。
    ///
    /// 与 `--code` 互斥：码已经到手时，没有"只发码"这回事。
    #[arg(long, conflicts_with = "code")]
    pub(crate) send_only: bool,
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
    pub(crate) code: Option<String>,
    /// 仅 App：固定设备串第 7 段（12 位大写 HEX）。
    ///
    /// **不传时每次运行现生成**（实测该段不是每安装稳定值，见
    /// `mcloud_app::device::new_app_device_short`）。传它是为了把
    /// "发码 → 登录"两半**钉在同一台设备形态**上：
    /// `--send-only` 会把当次生成的值打出来，原样传回来即可。
    #[arg(long)]
    pub(crate) device_short: Option<String>,
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
pub(crate) struct SigninArgs {
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
    pub(crate) account: Option<usize>,
    /// 打印账号清单（含下标），然后退出 —— 用它查 `--account` 该填几
    #[arg(long, conflicts_with_all = ["account", "phone"])]
    pub(crate) list: bool,
    /// 完整手机号（11 位），不是掩码 id。**只给 v6 之前登录、库里没有整串凭据的老账号用。**
    ///
    /// 用一次就够：跑完会**把号码记进库里**，下次直接 `--account <下标>`。
    /// ⚠ 只记号码，**不记**这一次现拼出来的整串 —— 那一列的口径是"服务端给的原文"，
    /// 写进一个本地合成物会绕过前缀校验（详见 `mcloud_app::signin::SignInCredential::from_phone`）。
    #[arg(long)]
    pub(crate) phone: Option<String>,
    #[arg(long, default_value = "data/mcloud.db")]
    pub(crate) db: PathBuf,
    /// 仅 App：`deviceId` 的存放位置（32 位大写 HEX，无后缀）。
    ///
    /// 必须与登录时用的是**同一个**：服务端按设备定位会话，漂移的表现是
    /// "凭据都在、却回 90001 未登录"。
    #[arg(long, default_value = "data/device-app.txt")]
    pub(crate) app_device_file: PathBuf,
    /// 仅 App：固定设备串第 7 段（12 位大写 HEX）。不传则每次现生成。
    #[arg(long)]
    pub(crate) device_short: Option<String>,
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
    pub(crate) device_id: Option<String>,
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
    pub(crate) receive: bool,
    /// 领取的 `cloudId`（`infoV3` 的 `receiveList[].recordId`）。默认空串。
    ///
    /// 实测抓包里那个数组只有 `cloudType` / `cloudNum`、**没有 `recordId`**，
    /// 所以 H5 的 `e.recordId ?? ""` 实际给的就是空串。照抄，别自己编一个。
    ///
    /// **只在显式模式（给了 `--cloud-type`）下有用。**
    #[arg(long, default_value = "")]
    pub(crate) cloud_id: String,
    /// 显式指定 `cloudType`（`receiveList[].cloudType`）—— **自检用，日常不要给**。
    ///
    /// 不给 = 按账目自动分流（推荐）。给了 = 只发这一笔，见 `--receive` 的说明。
    ///
    /// ⚠ 别拿 `2` 去试：那是**下月可领取**，发出去只会拿到 `+0`。
    #[arg(long)]
    pub(crate) cloud_type: Option<i64>,
    /// 用掉一张**复活卡 / 回馈卡**（`receiveRevivalReward`）。
    ///
    /// ⚠⚠ **这是写请求，而且不可逆** —— H5 的原话是「关闭弹窗视为放弃机会，
    /// **无法补发**」。卡用掉就没了，所以这个开关**默认不开**。
    ///
    /// 与 `--receive` 的区别：那一条领的是**当期可领的云朵**，这一条消耗的是**卡**。
    /// 它**不需要** deviceId（H5 定义里没有 `isDeviceId`），所以**默认一个头都不带**；
    /// 给了 `--device-id` 才会补一个 `deviceid` 头 —— 两个证据不一致，见上游层的说明。
    #[arg(long, conflicts_with = "status_only")]
    pub(crate) revive: bool,
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
    pub(crate) multiple: bool,
    /// 只查状态，不签到
    #[arg(long)]
    pub(crate) status_only: bool,
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
    pub(crate) raw: bool,
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
    pub(crate) records: bool,
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
    pub(crate) records_all: bool,
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
    pub(crate) prizes: bool,
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
    pub(crate) backup_gift: bool,
    /// 今天已签也发一次 `startSignIn`。**用途只有一个：协议自检** ——
    /// 确认我们拼的请求被上游接受（而不是回 `401` / `90001`）。
    ///
    /// 服务端对同一天的重复签到是幂等的（实拍：已签时回 `success` + `todaySignIn:true`），
    /// 所以它不会把今天签两次。但日常签到别开：那会让"今天已经签过"这条捷径失效。
    #[arg(long)]
    pub(crate) force: bool,
    /// 以 JSON 输出（脚本 / 定时任务消费）
    #[arg(long)]
    pub(crate) json: bool,
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
pub(crate) enum ClientArg {
    /// PC 客户端（Windows / macOS）
    Pc,
    /// iOS App
    App,
}

#[derive(clap::Args, Debug)]
pub(crate) struct MigrateArgs {
    /// 旧项目仓库根（据此推导 `asign.json` 与 `packages/api/data/`）
    #[arg(long, default_value = ".")]
    pub(crate) repo_root: PathBuf,
    /// 旧 `asign.json` 路径（覆盖 `--repo-root` 的推导结果）
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
    /// 旧数据目录（覆盖 `--repo-root` 的推导结果）
    #[arg(long)]
    pub(crate) data_dir: Option<PathBuf>,
    /// 目标 SQLite 文件
    #[arg(long, default_value = "data/mcloud.db")]
    pub(crate) db: PathBuf,
    /// 只预演：跑通身份映射与全部引用诊断并报出问题，**不写任何文件**
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// 旧数据原样归档到这个目录（预演时不归档）
    #[arg(long, default_value = "data/legacy-source")]
    pub(crate) backup_dir: PathBuf,
    /// 以 JSON 输出报告
    #[arg(long)]
    pub(crate) json: bool,
}
