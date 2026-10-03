//! 移动云盘 PC 客户端登录协议（扫码 / 短信 / 账密 / 退出）。
//!
//! # 规格来源
//!
//! 全部依据实测与客户端产物，不含猜测性字段：
//!
//! - `../mcloud-capture/docs/upstream/login-protocol.md`：PC 客户端扫码（本地拼装二维码 +
//!   `thirdlogin` 轮询 + 状态机），采集环境 Windows 11 + 客户端 `8.8.6.20260829`；
//! - `../mcloud-capture/docs/upstream/logout-sms-login.md`：短信登录全链路（含滑块降级）；
//! - `../mcloud/docs/capture/apiServer.js`（从 `app.asar` 提取的 renderer 源码）：
//!   `isMacPc`、`clientType` / `cpid`、设备串兜底模板、三个密码学原语。
//!
//! # 三条"反直觉"的实测结论（照直觉写就会错）
//!
//! 1. **不存在"取码接口"**：二维码 URL 由客户端**本地拼装**，零网络请求
//!    （客户端源码 `createLoginQRcode()` 就是四行字符串拼接）。
//! 2. **扫码与短信、账密共用同一个接口** `POST /user/thirdlogin`，靠 `pintype` 区分。
//! 3. **登录成功的 `data` 是 AES 密文**，不是明文 JSON —— 见 [`LoginPayloadDecryptor`]。
//!
//! # 本模块的边界
//!
//! 只做**请求构造**与**响应语义**，不含轮询循环与持久化：
//!
//! - 轮询节奏由上层用 `Clock` 驱动 —— 循环一旦写在这里，"成功后必须停轮询"
//!   就没法被单测钉住；
//! - 因此所有 body 构造函数与状态判定都是**纯函数**，可只靠
//!   `cargo test -p mcloud-upstream` 验证。
//!
//! # 平台形态（Windows / macOS）
//!
//! 上游把两者当成**两个不同的客户端**：`clientType`、`cpid`、版本号、渠道号、
//! HTTP UA 全都不同（客户端源码用 `navigator.userAgent` 含不含 `mac` 二选一）。
//!
//! ⚠ 这里**只消费** [`PcPlatform`]，不定义它：该枚举要随账号落盘，
//! 而 `mcloud-store` 与 `mcloud-upstream` 是同层兄弟，让 store 依赖 upstream
//! 会违反分层铁律（见 [`PcPlatform`] 自身的说明）。

use mcloud_core::{Error, HttpRequest, PcPlatform, Result};
use mcloud_crypto::{
    aes_ecb_pkcs7_decrypt_hex, decrypt_with_dynamic_iv, encrypt_with_dynamic_iv, sha1_hex_upper,
    Random,
};
use serde_json::{json, Value};

use crate::endpoints;

// ══════════════════════════════ 平台档案 ══════════════════════════════

/// 一个"冒充成 PC 客户端"的档案：平台 + 设备指纹。
///
/// # 与 [`crate::ClientProfile`] 的分工
///
/// [`crate::ClientProfile`] 管的是**凭证**（basic / jwt）与传输参数；
/// 这个类型管的是**身份形态**（我是什么客户端）。分开是因为登录时
/// 还没有凭证，但已经有形态了 —— 合成一个类型会逼着登录路径传一堆
/// 它没有的 `None`。
///
/// # 为什么把 `device_id` 也放进来
///
/// 设备串（`x-yun-client-info`）的第 6 段就是设备指纹，两者是**同一个值**。
/// 让它们在类型上绑定，`device_info()` 就不可能在"没有设备号"时被调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcClient {
    platform: PcPlatform,
    device_id: String,
}

impl PcClient {
    pub fn new(platform: PcPlatform, device_id: impl Into<String>) -> Self {
        Self {
            platform,
            device_id: device_id.into(),
        }
    }

    pub fn platform(&self) -> PcPlatform {
        self.platform
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// `clientType` 业务参数。
    pub fn client_type(&self) -> &'static str {
        match self.platform {
            PcPlatform::Windows => "656",
            PcPlatform::MacOs => "710",
        }
    }

    /// `cpid` 业务参数。
    pub fn cpid(&self) -> &'static str {
        match self.platform {
            PcPlatform::Windows => "277",
            PcPlatform::MacOs => "332",
        }
    }

    /// 该平台客户端上报的版本号。
    ///
    /// # 为什么它必须跟着平台走
    ///
    /// 它是**业务参数**而非展示信息：同时进请求体的 `version` 字段与二维码的
    /// `ver` 参数，上游按版本分流。mac 的 `clientType` 配 Windows 的版本号，
    /// 是一个上游一眼可辨的矛盾组合。
    ///
    /// 两个值都有出处：Windows `8.8.6.20260829` 来自抓包归档里的真实请求体；
    /// macOS `4.2.1` 来自客户端 `app.asar` 的 `package.json`。
    pub fn app_version(&self) -> &'static str {
        match self.platform {
            PcPlatform::Windows => "8.8.6.20260829",
            PcPlatform::MacOs => "4.2.1",
        }
    }

    /// 渠道号（`x-yun-app-channel`）。
    ///
    /// # 证据等级
    ///
    /// 来自 `mcloud/docs/client-profiles.md` §4 的**客户端源码**逐行引用：
    /// `x-yun-app-channel: macOS ? 10301000 : 10200153` —— 这是该文档明确点出的
    /// 三个易错点之一（"`x-yun-app-channel` **mac 与 win 不同值**"）。
    ///
    /// ⚠ **这个头不在登录链路的实测头集合里**：登录走的是客户端
    /// `sendRequest` 的 **pre-login 分支**（那时还没有 `UserInfo`），
    /// 而 `x-yun-app-channel` 属于 `cutoverStatus == 1` 那一套**登录后**的头。
    ///
    /// 仍然带上它的理由：它是客户端源码里**唯一**按 mac/win 取值的渠道位，
    /// 而这正是本模块要如实复刻的平台判别位；且 §3.0 记过一条同款教训 ——
    /// 头部不一致的失败长得像"登录被拒"（回 `9999 登录失败！`），
    /// 而排查方向是"把该端的头补齐"，不是"少发一个"。
    ///
    /// 若哪天上游因为莫名其妙的业务码拒登，**第一个该试的就是去掉它**。
    ///
    /// ⚠ 注意它与 [`Self::huawei_channel_src`] **不是**同一个头、也**不是**
    /// 同一个取值规则：那个在客户端里是硬编码常量，不随平台变。
    pub fn app_channel(&self) -> &'static str {
        match self.platform {
            PcPlatform::Windows => "10200153",
            PcPlatform::MacOs => "10301000",
        }
    }

    /// `x-huawei-channelSrc` 的值。
    ///
    /// # ⚠ 它是**常量**，不随平台变
    ///
    /// 这一度写错过：曾让它与 [`Self::app_channel`] 取同一个值，
    /// 于是 mac 会发出 `x-huawei-channelSrc: 10301000`。
    ///
    /// 证据是客户端源码里那一行**字面量**（`mcloud/docs/client-profiles.md` §4
    /// 的 pre-login 分支、`../mcloud/docs/capture/apiServer.js`）：
    ///
    /// ```js
    /// xhr.setRequestHeader("x-huawei-channelSrc","10200153");
    /// ```
    ///
    /// 同一段里 `x-yun-app-channel` 才是三元表达式 —— **只有它随平台变**。
    /// 两个参考实现（Go 的 `huaweiChannelSrc`、Rust 的 `HUAWEI_CHANNEL_SRC`）
    /// 也都把它钉成常量。
    ///
    /// 为什么这个错误要紧：这个头是**登录必需**的那 5 个之一
    /// （见 [`Self::headers`]），发错值等于把一个必需头填错，
    /// 而上游对此的回应是 `9999 登录失败！` —— 看不出是头的问题。
    pub const fn huawei_channel_src(&self) -> &'static str {
        "10200153"
    }

    /// HTTP `User-Agent`。
    ///
    /// # 证据等级（比其它字段低，如实标注）
    ///
    /// 两个参考实现都**没有**给 PC 端设过 HTTP UA（只发自定义的
    /// `x-UserAgent: PC`），所以"原生 PC 客户端到底发什么 UA"**没有实测**。
    ///
    /// 这里用的是**同平台浏览器外壳的实测串**（都带 `Edg/`，说明 Electron
    /// 外壳就是 Edge）：
    ///
    /// - Windows：`../mcloud/docs/capture/web-qr-login-samples.txt` §1 的实测头；
    /// - macOS：`mcloud/test/fixtures/http/登录pc.json` 里 21 条同类请求的实测头。
    ///
    /// 为什么仍然要区分它：客户端**自己**就是靠 UA 里的 `mac` 判断平台的
    /// （`apiServer.js:10`），所以"UA 反映平台"是真实客户端的行为，不是我们的发明。
    ///
    /// ⚠ 两串的浏览器大版本号（153 / 149）只是各自采集时的版本，**不要**
    /// 把它当成"平台 → 版本"的映射去解读；有意义的只是 OS 段。
    pub fn user_agent(&self) -> &'static str {
        match self.platform {
            PcPlatform::Windows => {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36 Edg/153.0.0.0"
            }
            PcPlatform::MacOs => {
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36 Edg/149.0.0.0"
            }
        }
    }

    /// 设备信息串（`x-yun-client-info` / `x-DeviceInfo`，14 段，`|` 分隔）。
    ///
    /// # 字段表（下标 0 起）
    ///
    /// | 下标 | 内容 |
    /// |---|---|
    /// | 0,1 | 空串（前导 `\|\|`） |
    /// | 2 | `11`（固定） |
    /// | 3 | `appVersion` ← **这就是平台差异所在** |
    /// | 4 | `PC` |
    /// | 5 | base64(设备型号码)，兜底是固定常量 |
    /// | 6 | 设备指纹（与 `x-yun-device-id` 同值） |
    /// | 7 | 空串 |
    /// | 8 | OS 描述 |
    /// | 9 | 分辨率 |
    /// | 10 | base64(语言) |
    /// | 11-13 | 空串 |
    ///
    /// ⚠ 第 5 / 10 段是 base64 包一层、第 6 段是明文 —— 同一串里两种编码混用。
    ///
    /// # 已知的保真缺口（**不猜**）
    ///
    /// 这段模板来自客户端源码里**原生接口失败时的兜底**，而那份兜底是
    /// **Windows 专用**的（` Windows 10 (10.0)`、`1920X1040`）。
    /// macOS 真实客户端会走原生接口拿到自己的 OS 串与分辨率，
    /// 但**两个参考实现都没有 mac 的实测样本**（Go 与 Rust 版都复用这份模板
    /// 并同样标注为未知）。
    ///
    /// 所以这里也复用同一份兜底，**只让第 3 段（版本）随平台变**：
    /// 与其编一个看起来合理的 macOS OS 串，不如留一个有据可查的缺口。
    pub fn device_info(&self) -> String {
        format!(
            "||11|{}|PC|QkYtMjAyMDAzMTAxNjQ3|{}|| Windows 10 (10.0)|1920X1040|\
Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||",
            self.app_version(),
            self.device_id,
        )
    }

    /// 登录链路要带的设备头。
    ///
    /// # 这 5 个头是**实测**出来的最小集合，不是装饰
    ///
    /// 该文档 §3.0 记了原始排查过程：只带 `x-yun-tid` 调 `/user/thirdlogin`
    /// 会被回 `{"code":"9999","message":"登录失败！"}`，补上这几个设备头之后
    /// 才是正常的 `200059541 未找到设备的二维码信息`。
    /// 也就是说**"头不对"的失败长得像"登录被拒"**，很容易误判成账号问题。
    ///
    /// 其中 `x-yun-client-info` 与 `x-DeviceInfo` 是**两个头、同一个值**
    /// （大小写不同）—— 不是笔误，客户端源码里就是两行 `setRequestHeader`。
    ///
    /// `device_info` 的第 3 段是版本号，因此这两个头**本身就随平台变化**。
    ///
    /// # 本方法只发登录分支的那 5 个头
    ///
    /// 客户端在**登录时**走 `sendRequest` 的 pre-login 分支，那里只有 5 个头。
    /// `x-yun-app-channel` 属于 `cutoverStatus == 1` 的**登录后**头集合，
    /// 所以**不在这里** —— 它由 [`Self::app_channel`] 提供给别的请求构造点，
    /// 而不是塞进登录请求。
    ///
    /// ⚠ 上一版这里多发了一个 `x-yun-app-channel`：那是把"两个分支的头"
    /// 合并成了一套。合并的后果不是报错，而是登录请求多带一个上游没预期的头。
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let info = self.device_info();
        vec![
            ("x-yun-client-info", info.clone()),
            ("x-DeviceInfo", info),
            ("x-MM-Source", MM_SOURCE.to_string()),
            ("x-huawei-channelSrc", self.huawei_channel_src().to_string()),
            ("x-UserAgent", "PC".to_string()),
        ]
    }

    /// **登录后**（`cutoverStatus == 1`）那一套头里按平台变的渠道位。
    ///
    /// 与 [`Self::headers`] 分开：登录前/后是两套头，混成一套会让
    /// "登录请求多带一个头"这种错误无法被发现（它不报错）。
    pub fn post_login_headers(&self) -> Vec<(&'static str, String)> {
        vec![("x-yun-app-channel", self.app_channel().to_string())]
    }

    /// 已登录的业务请求要带的全部 PC 设备头。
    ///
    /// = [`Self::headers`]（登录那 5 个） + [`Self::post_login_headers`]（登录后追加）。
    ///
    /// 之所以把"相加"这件事放在这里而不是让调用方自己拼两遍：
    /// 两处拼写就有两处漏掉一个头的机会，而漏一个头在上游那里
    /// 表现为一个含义模糊的业务码（`9999 登录失败！`）。
    pub fn business_headers(&self) -> Vec<(&'static str, String)> {
        let mut all = self.headers();
        all.extend(self.post_login_headers());
        all
    }
}

/// `x-MM-Source` 的兜底值（客户端源码：`appSource || "000"`）。
pub const MM_SOURCE: &str = "000";

// ══════════════════════════════ 登录方式 ══════════════════════════════

/// `/user/thirdlogin` 的登录方式，取值为 `pintype`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
    /// 扫码：`dycpwd` = 二维码里的 `sID`，`msisdn` 为空。
    QrCode,
    /// 短信验证码：`dycpwd` = 6 位验证码明文，`msisdn` = 手机号。
    Sms,
    /// 账号密码：`dycpwd` = 口令哈希，`msisdn` = 用户名。
    Password,
}

impl LoginKind {
    pub const fn pintype(self) -> &'static str {
        match self {
            Self::QrCode => "21",
            Self::Sms => "5",
            Self::Password => "9",
        }
    }

    /// `extInfo.ifOpenAccount`：短信是 `1`，扫码与账密是 `0`。
    pub const fn if_open_account(self) -> i64 {
        match self {
            Self::Sms => 1,
            Self::QrCode | Self::Password => 0,
        }
    }
}

/// 上游业务码（实测汇总）。
///
/// 单列成常量而不是散在 `match` 里，是为了让"这个码的含义有实测依据"可见。
pub mod codes {
    /// 成功。扫码与短信两条链路都见过，扫码还见过 `200`。
    pub const OK_ZERO: i64 = 0;
    /// 同上（`200` 那条）。
    pub const OK_200: i64 = 200;

    /// 未找到设备的二维码信息 —— 码还没被扫。
    pub const QR_NOT_SCANNED: i64 = 200_059_541;
    /// 二维码信息未授权 —— **已扫码，等用户在手机上确认**。
    pub const QR_SCANNED: i64 = 200_059_548;
    /// 二维码信息与登录二维码信息不一致 —— `sID` 不匹配。
    pub const QR_MISMATCH: i64 = 200_059_544;
    /// 二维码已失效 —— 超时，必须重新出码。
    pub const QR_EXPIRED: i64 = 200_059_542;
    /// 扫码登录过于频繁 —— 语义是限流：该**退避**，不该换张码立刻重试。
    pub const QR_TOO_FREQUENT: i64 = 200_059_545;

    /// 请先获取滑块拼图验证码。
    pub const NEED_SLIDER: i64 = 200_059_554;
    /// 同一滑块验证码校验失败次数超限 —— 挑战作废，必须重新取图。
    pub const SLIDER_EXHAUSTED: i64 = 200_059_553;

    /// 用户 token 不能为空（退出登录的实测常态：客户端先清 token 再调 logout）。
    pub const TOKEN_EMPTY: i64 = 1_000_001;

    /// 输入参数不合法 —— 服务端**解不开请求体**。
    pub const UNDECRYPTABLE_BODY: i64 = 9_103;
    /// 验证码不正确 —— 请求体已被正确解析，只是短信码不对/过期。
    pub const SMS_CODE_WRONG: i64 = 9_441;
    /// 登录失败！—— 设备头缺失/不完整时的表现。
    pub const LOGIN_FAILED: i64 = 9_999;
}

/// 成功码判定：`0`（含 `"0000"`）与 `200`。
pub fn is_success_code(code: i64) -> bool {
    code == codes::OK_ZERO || code == codes::OK_200
}

// ══════════════════════════════ 二维码（本地拼装） ══════════════════════════════

/// 扫码登录的二维码落地页。`sID` / `dID` 以 query 形式拼在 `#/` 之后。
pub const QR_LOGIN_URL: &str = "https://yun.139.com/w/#/qrcLogin";

/// 拼出二维码载荷。
///
/// # 没有"申请二维码"这个请求
///
/// 客户端源码里就是四行字符串拼接，**不发起任何网络请求**；实测 1622 个
/// body 文件里也没有对应的取码接口。它推翻了"取码要调接口"的直觉假设。
///
/// `sID` 与后续 `thirdlogin` 的 `dycpwd` 是**同一个值**（源码注释：
/// `// QRCodeID也是sessionId`），所以要一直带到轮询结束。
///
/// `dID`（设备指纹）必须**持久化**：每次重新生成会让服务端定位不到会话，
/// 轮询会一直停在"未找到设备的二维码信息"。
pub fn qr_payload(client: &PcClient, session_id: &str) -> String {
    format!(
        "{QR_LOGIN_URL}?dID={}&sID={session_id}&cType=11&ver={}",
        client.device_id(),
        client.app_version(),
    )
}

/// 生成 PC 设备指纹（`windowMachine`）：32 位大写 hex + `-ENDIN`。
///
/// # 为什么必须**持久化**
///
/// 它是服务端定位"这次扫码会话是哪台设备"的唯一凭据（二维码的 `dID`）。
/// 每次重新生成的表现是轮询**永远停在** `200059541 未找到设备的二维码信息`
/// —— 服务端按 `dID` 找不到那张码。而这看起来像"码过期了"，
/// 极难往"设备号变了"上想。
///
/// 落盘由 [`mcloud_core::PcDeviceStore`] 负责（core 不能依赖本层的随机源与 hex）。
pub fn new_pc_device_id(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    format!(
        "{}{}",
        hex::encode_upper(bytes),
        mcloud_core::PC_DEVICE_SUFFIX
    )
}

/// 读这台机器的 PC 设备指纹；没有就生成一个并落盘。
///
/// # 为什么"读不到就生成"必须在这里而不是 store 里
///
/// 生成要随机源（在 `mcloud-crypto`），而 `mcloud-core` 不能依赖它
/// （反向依赖，见 docs/conventions.md §1）。所以 store 只提供
/// "读 / 写"，这条"读不到就生成"的策略放在有随机源的这一层。
///
/// # 为什么不能每次都生成
///
/// 它是服务端定位扫码会话的唯一凭据（二维码的 `dID`）。
/// 每次重新生成的表现是轮询**永远停在** `200059541 未找到设备的二维码信息`
/// —— 服务端按 `dID` 找不到那张码。而这看起来像"码过期了"，
/// 极难往"设备号变了"上想。
pub async fn load_or_create_pc_device<S: mcloud_core::PcDeviceStore + ?Sized>(
    store: &S,
    rng: &mut impl Random,
) -> Result<String> {
    if let Some(existing) = store.pc_device_id().await? {
        return Ok(existing);
    }
    let fresh = new_pc_device_id(rng);
    store.set_pc_device_id(&fresh).await?;
    Ok(fresh)
}

/// 生成 `sID`：32 位**大写**十六进制（对齐客户端 `Utils.getGUID32()`）。
pub fn new_session_id(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    hex::encode_upper(bytes)
}

/// 生成发码请求里的 `random`：**24 位小写**十六进制。
///
/// # 为什么不用 UUID
///
/// 客户端源码里它是两段 `Math.random().toString(16).substring(2)` 拼出来的，
/// 既不是 UUID 也不是 16/32 位 —— 长度（24）与字符集（小写十六进制）都由
/// 服务端校验。换成 `uuid_v4()` 会得到 36 字符带连字符的串，形态就不对了。
pub fn new_sms_random(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 12];
    rng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ══════════════════════════════ 请求体（纯函数） ══════════════════════════════

/// `/user/thirdlogin` 的请求体。
///
/// 三种登录方式共用这一个构造器 —— 差异全部收敛在 [`LoginKind`] 与两个入参上。
///
/// 字段顺序不是随意的：`mcloud` 的实现刻意按 **JSON key 字典序**排列。
/// 本项目不用手工维护顺序：`serde_json::Map` 默认是 `BTreeMap`，
/// 序列化出来天然就是字典序（没开 `preserve_order` 特性）。
pub fn thirdlogin_body(client: &PcClient, kind: LoginKind, dycpwd: &str, msisdn: &str) -> Value {
    json!({
        "clientType": client.client_type(),
        "cpid": client.cpid(),
        "dycpwd": dycpwd,
        "extInfo": { "ifOpenAccount": kind.if_open_account() },
        "loginMode": "0",
        "msisdn": msisdn,
        "pintype": kind.pintype(),
        "verType": "2",
        "version": client.app_version(),
    })
}

/// 扫码登录：`dycpwd` 直传二维码里的 `sID`，**无需任何变换**。
pub fn qr_thirdlogin_body(client: &PcClient, session_id: &str) -> Value {
    thirdlogin_body(client, LoginKind::QrCode, session_id, "")
}

/// 短信登录：`dycpwd` 是 **6 位验证码明文**（不是哈希）。
pub fn sms_thirdlogin_body(client: &PcClient, phone: &str, sms_code: &str) -> Value {
    thirdlogin_body(client, LoginKind::Sms, sms_code, phone)
}

/// 账密登录：`dycpwd` 是**哈希后的口令**，`msisdn` 是用户名。
pub fn password_thirdlogin_body(client: &PcClient, username: &str, password: &str) -> Value {
    thirdlogin_body(
        client,
        LoginKind::Password,
        &password_hash(password),
        username,
    )
}

/// `/user/sms/getSmsCode` 的请求体。
///
/// `mode` / `reqType` 是固定值（实测），不要当可变参数暴露出去。
pub fn send_sms_code_body(client: &PcClient, phone: &str, random: &str) -> Value {
    json!({
        "phoneNumber": phone,
        "random": random,
        "mode": 0,
        "reqType": 3,
        "clientType": client.client_type(),
    })
}

/// `/user/verfycode` 的请求体：取滑块拼图。
///
/// 路径与字段名都**没有 `i`**：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的。
pub fn slider_puzzle_body(phone: &str) -> Value {
    json!({ "account": phone, "type": 2 })
}

/// `/user/logout` 的请求体。
pub fn logout_body(phone: &str) -> Value {
    json!({ "phoneNumber": phone })
}

// ══════════════════════════════ 口令与滑块 ══════════════════════════════

/// 口令哈希的前缀（客户端 `common/utils.js` 的 `HASHTEXT`）。
pub const PASSWORD_HASH_PREFIX: &str = "fetion.com.cn:";

/// 账密登录 `dycpwd` 的取值：`SHA1("fetion.com.cn:" + 口令)` 的**大写** hex。
///
/// 这不是"密码存储"意义上的哈希（固定前缀、无盐、SHA-1），只是移动服务端
/// 用来校验口令的协议字段。本系统自己的口令存储走 `mcloud_crypto::hash_password`
/// （argon2id + 随机盐），两者**不要混用**。
pub fn password_hash(password: &str) -> String {
    sha1_hex_upper(&format!("{PASSWORD_HASH_PREFIX}{password}"))
}

/// 滑块答案的 AES 密钥（客户端 `encryptByAesCBC` 的 `aesKey`）。
pub const PUZZLE_AES_KEY: &str = "CREATPUZZLE03F9A";

/// 滑块答案的 AES IV。
///
/// **硬编码**，不是服务端下发的挑战值 —— 文档里曾把它推测为"服务端挑战标识"
/// （因为两次请求间保持不变），从源码看它就是这一行常量。
pub const PUZZLE_AES_IV: &[u8; 16] = b"E526E1F3F9765444";

/// 把滑块答案加密成 `puzzleVerfycode`：AES-128-CBC/PKCS7，输出 `base64(IV ‖ CT)`。
///
/// 两个易错点：
/// - 字段名是 `puzzleVerfycode`（`verfy` 不是 `verify`）——
///   客户端源码在这里专门写了句「大坑！！！」；
/// - 答案是**拖动像素偏移的十进制字符串**（实测两条真实请求解出来是 `461` / `391`），
///   不是轨迹点数组。
pub fn puzzle_verfycode_encrypt(slider_answer: &str) -> Result<String> {
    encrypt_with_dynamic_iv(PUZZLE_AES_KEY.as_bytes(), PUZZLE_AES_IV, slider_answer)
        .map_err(|e| Error::Other(format!("滑块答案加密失败：{e}")))
}

/// 带滑块的发码请求体：在基础 body 上追加 `puzzleVerfycode`。
///
/// 单独成一个函数而不是给 [`send_sms_code_body`] 加 `Option` 参数：
/// 走滑块是**异常路径**（首次发码被风控拦下才走），让它在调用点显式可见，
/// 比在每个调用点传 `None` 更不容易埋掉。
pub fn send_sms_code_body_with_slider(
    client: &PcClient,
    phone: &str,
    random: &str,
    slider_answer: &str,
) -> Result<Value> {
    let mut body = send_sms_code_body(client, phone, random);
    body["puzzleVerfycode"] = Value::String(puzzle_verfycode_encrypt(slider_answer)?);
    Ok(body)
}

/// 滑块拼图：两张图 + **缺口坐标**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliderPuzzle {
    /// 背景图（带缺口的那张，实测是 JPEG）。
    pub background_b64: String,
    /// 拼图块本身（实测是 PNG）。
    pub puzzle_b64: String,
    /// 背景图宽度（上游给的是**字符串** `"680"`）。
    pub pic_width: Option<i64>,
    /// 拼图块宽度（字符串 `"96"`）。
    pub puzzle_width: Option<i64>,
    /// 加密后的**缺口横坐标** —— 用 [`SliderPuzzle::answer`] 解。
    pub left_encrypted: String,
}

impl SliderPuzzle {
    /// 解出拖动答案（缺口横坐标的十进制字符串）。
    ///
    /// # 上游把答案直接给了出来
    ///
    /// `data.puzzleLeft` 是 `base64(IV ‖ AES-128-CBC/PKCS7 密文)`，密钥就是客户端
    /// 里那个**硬编码公开常量** [`PUZZLE_AES_KEY`]。所以**不需要图像识别**，
    /// 也不需要人眼量像素。
    ///
    /// 解出来会做两道自检（必须是十进制数、必须落在图宽内）：宁可报错也不要
    /// 把一个荒谬的坐标发上去 —— 那会被判滑块失败，而失败次数是有限的。
    pub fn answer(&self) -> Result<String> {
        let plain = decrypt_with_dynamic_iv(PUZZLE_AES_KEY.as_bytes(), &self.left_encrypted)
            .map_err(|e| Error::Parse(format!("滑块缺口坐标解密失败：{e}")))?;
        let answer = plain.trim().to_string();

        if answer.is_empty() || !answer.chars().all(|c| c.is_ascii_digit()) {
            return Err(Error::Parse(format!("缺口坐标不是十进制数：{answer:?}")));
        }
        let x: i64 = answer
            .parse()
            .map_err(|e| Error::Parse(format!("缺口坐标解析失败：{e}")))?;
        if let Some(width) = self.pic_width {
            if !(0..=width).contains(&x) {
                return Err(Error::Parse(format!("缺口坐标 {x} 超出图宽 {width}")));
            }
        }
        Ok(answer)
    }
}

/// 从 `/user/verfycode` 的响应里取出滑块挑战。
///
/// 取不到就报错而不是返回空串：空图片会让人以为"图挂了"，
/// 而真实原因往往是上游换了字段名（那时应该立刻看见字段名不对）。
pub fn parse_slider_puzzle(env: &LoginEnvelope) -> Result<SliderPuzzle> {
    let payload = env
        .data
        .as_ref()
        .ok_or_else(|| Error::Parse("取滑块响应里没有 data".into()))?;
    let field = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .ok_or_else(|| Error::Parse(format!("取滑块响应里缺少 {name}")))
    };
    // 尺寸上游给的是字符串，但也容忍数字 —— 这类字段在小步演进的接口里很常见。
    let size = |name: &str| {
        payload.get(name).and_then(|v| match v {
            Value::String(s) => s.parse::<i64>().ok(),
            Value::Number(n) => n.as_i64(),
            _ => None,
        })
    };
    Ok(SliderPuzzle {
        background_b64: field("picture")?,
        puzzle_b64: field("puzzle")?,
        pic_width: size("picWidth"),
        puzzle_width: size("puzzleWidth"),
        left_encrypted: field("puzzleLeft")?,
    })
}

// ══════════════════════════════ 响应信封 ══════════════════════════════

/// `thirdlogin` 一族的响应信封。
///
/// `code` 可能是**数字**（`200059541`）也可能是**字符串**（`"0000"` / `"9999"`），
/// 两种都在实测里出现过，所以统一归一成数字。
#[derive(Debug, Clone, PartialEq)]
pub struct LoginEnvelope {
    /// 归一后的业务码。既不是数字也不是数字字符串时为 `None`。
    pub code: Option<i64>,
    /// 原始 code 文本（排障用；归一化会丢掉 `"0000"` 的形态）。
    pub raw_code: String,
    /// 消息。上游用过 `message` 与 `msg` 两个键名。
    pub message: String,
    /// `data` 原样保留。
    pub data: Option<Value>,
}

impl LoginEnvelope {
    pub fn parse(body: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("登录响应不是 JSON：{e}")))?;

        let raw_code = match v.get("code") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        // `"0000"` → 0；`"200059541"` → 200059541；非数字文本 → None
        let code = if raw_code.is_empty() {
            None
        } else {
            raw_code.parse::<i64>().ok()
        };

        let message = v
            .get("message")
            .or_else(|| v.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(Self {
            code,
            raw_code,
            message,
            data: v.get("data").cloned(),
        })
    }
}

/// 一次扫码轮询的结果。
///
/// 刻意**不把"未扫码"当成错误**：轮询期间绝大多数响应都是它，
/// 当成错误会让调用方拿异常做流程控制，日志也会被刷满。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// 码还没被扫。
    NotScanned,
    /// 已扫码，等用户在手机上确认 —— 唯一值得对外提示进度的中间态。
    Scanned,
    /// `sID` 不匹配：换了码却还在用旧值轮询。属于调用方的状态错误。
    SessionMismatch,
    /// 二维码失效，必须重新出码。
    Expired,
    /// 触发限流 —— 该退避，不该立刻换码重试。
    TooFrequent,
    /// 登录成功。`encrypted` 是 `data` 原文（AES 密文），交给 [`LoginPayloadDecryptor`]。
    Success { encrypted: String },
    /// 其它码：原样带回交给上层判断，不硬编码成错误 —— 上游在活动期会加新码。
    Other { code: Option<i64>, message: String },
}

/// 把一次轮询响应归类。
pub fn classify_poll(env: &LoginEnvelope) -> PollOutcome {
    match env.code {
        Some(codes::QR_NOT_SCANNED) => PollOutcome::NotScanned,
        Some(codes::QR_SCANNED) => PollOutcome::Scanned,
        Some(codes::QR_MISMATCH) => PollOutcome::SessionMismatch,
        Some(codes::QR_EXPIRED) => PollOutcome::Expired,
        Some(codes::QR_TOO_FREQUENT) => PollOutcome::TooFrequent,
        Some(code) if is_success_code(code) => match env.data.as_ref() {
            Some(Value::String(cipher)) => PollOutcome::Success {
                encrypted: cipher.clone(),
            },
            // 成功码但没有密文载荷：如实报告"形状不对"，不要伪造一个空凭据。
            _ => PollOutcome::Other {
                code: Some(code),
                message: "登录成功但 data 不是密文字符串".to_string(),
            },
        },
        other => PollOutcome::Other {
            code: other,
            message: env.message.clone(),
        },
    }
}

/// 一次"发送验证码"的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendSmsOutcome {
    /// 已发出。`remain_get_times` 是配额余量。
    Sent {
        remain_get_times: Option<u32>,
    },
    /// 需要先过滑块：必须去取图。
    NeedSlider {
        captcha_type: Option<i64>,
    },
    /// 滑块挑战已作废（失败次数超限）：**必须重新取图**，不能复用旧挑战。
    SliderExhausted {
        captcha_type: Option<i64>,
    },
    Other {
        code: Option<i64>,
        message: String,
    },
}

/// 把一次"发送验证码"响应归类。
pub fn classify_send_sms(env: &LoginEnvelope) -> SendSmsOutcome {
    let captcha_type = || {
        env.data
            .as_ref()
            .and_then(|p| p.get("captchaType"))
            .and_then(Value::as_i64)
    };

    match env.code {
        Some(code) if is_success_code(code) => {
            let remain = env
                .data
                .as_ref()
                .and_then(|p| p.get("remainGetTimes"))
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            SendSmsOutcome::Sent {
                remain_get_times: remain,
            }
        }
        Some(codes::NEED_SLIDER) => SendSmsOutcome::NeedSlider {
            captcha_type: captcha_type(),
        },
        Some(codes::SLIDER_EXHAUSTED) => SendSmsOutcome::SliderExhausted {
            captcha_type: captcha_type(),
        },
        other => SendSmsOutcome::Other {
            code: other,
            message: env.message.clone(),
        },
    }
}

// ══════════════════════════════ 成功响应的解密 ══════════════════════════════

/// 登录成功响应里 `data` 的解密器。
///
/// 抽成 trait 的唯一目的仍然是**可测**：真实路径走 [`LoginDataDecryptor`]，
/// 而"上游把 `data` 换成另一套加密 / 干脆回明文"这种变化，
/// 可以用别的实现验证调用方的行为，不需要联网也不需要真凭据。
pub trait LoginPayloadDecryptor {
    /// 解密 `data` 密文并解析成 `userInfo` 结构。
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value>;
}

/// 生产环境的登录数据解密：AES-128-**ECB**/PKCS7。
///
/// 密钥与模式来自客户端 `common/utils.js: decryptByAes`。
/// 注意是 **ECB 不是 CBC**（见 `mcloud_crypto::aes_ecb_pkcs7_decrypt_hex` 的说明）。
pub struct LoginDataDecryptor {
    key: String,
}

impl LoginDataDecryptor {
    /// 生产密钥。
    pub fn production() -> Self {
        Self {
            key: LOGIN_AES_KEY.to_string(),
        }
    }

    /// 测试环境密钥。客户端在 `systemEnvironment` 为 11 / 22 时会换这把 key，
    /// 拿生产 key 去打测试环境的表现是"解出来是乱码"。
    pub fn test_env() -> Self {
        Self {
            key: LOGIN_AES_KEY_TEST.to_string(),
        }
    }

    /// 指定密钥（测试用）。
    pub fn with_key(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }
}

impl LoginPayloadDecryptor for LoginDataDecryptor {
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value> {
        let plain = aes_ecb_pkcs7_decrypt_hex(&self.key, ciphertext)
            .map_err(|e| Error::Parse(format!("登录响应解密失败（密钥或模式不对）：{e}")))?;
        serde_json::from_str(&plain).map_err(|e| {
            Error::Parse(format!(
                "userInfo 不是合法 JSON（密钥不对或用错模式都会落到这里）：{e}"
            ))
        })
    }
}

/// 生产环境的登录数据 AES 密钥。
pub const LOGIN_AES_KEY: &str = "qPqDw263XgFgL3u8";

/// 测试环境的登录数据 AES 密钥（`systemEnvironment` 为 11 / 22）。
pub const LOGIN_AES_KEY_TEST: &str = "2ErfJus1Ofr@2o24";

// ══════════════════════════════ 请求构造 ══════════════════════════════

/// 给请求补上"端"头与每次新的 `x-yun-tid`（UUID，链路追踪）。
///
/// `tid` 由调用方传入而不是在这里生成：生成随机数需要 `&mut Random`，
/// 而请求构造函数保持纯函数才能被逐字段断言。
fn with_client_headers(mut req: HttpRequest, client: &PcClient, tid: &str) -> HttpRequest {
    for (name, value) in client.headers() {
        req = req.header(name, value);
    }
    req.header("x-yun-tid", tid)
}

/// `POST /user/thirdlogin`（扫码 / 短信 / 账密共用）。
pub fn thirdlogin_request(client: &PcClient, tid: &str, body: &Value) -> HttpRequest {
    with_client_headers(
        HttpRequest::post(endpoints::THIRD_LOGIN).json_body(body),
        client,
        tid,
    )
}

/// `POST /user/sms/getSmsCode`。
pub fn send_sms_code_request(client: &PcClient, tid: &str, body: &Value) -> HttpRequest {
    with_client_headers(
        HttpRequest::post(endpoints::SEND_SMS_CODE).json_body(body),
        client,
        tid,
    )
}

/// `POST /user/verfycode`。
pub fn slider_puzzle_request(client: &PcClient, tid: &str, body: &Value) -> HttpRequest {
    with_client_headers(
        HttpRequest::post(endpoints::SLIDER_PUZZLE).json_body(body),
        client,
        tid,
    )
}

/// `POST /user/logout`。
pub fn logout_request(client: &PcClient, tid: &str, body: &Value) -> HttpRequest {
    with_client_headers(
        HttpRequest::post(endpoints::LOGOUT).json_body(body),
        client,
        tid,
    )
}

#[cfg(test)]
mod tests;
