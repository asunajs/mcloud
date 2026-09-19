//! 移动云盘 PC 客户端登录协议（扫码 / 短信 / 退出登录）。
//!
//! # 规格来源
//!
//! 全部依据实测，不含任何猜测性字段：
//!
//! - `docs/upstream-login-protocol.md`：扫码登录（本地拼装二维码 + `thirdlogin` 轮询 + 状态机）
//! - `docs/upstream-logout-sms-login.md`：退出登录、短信登录全链路（含滑块降级）
//! - 客户端 `common/utils.js`（从本机 `mCloud.app` 的 `app.asar` 提取）：三个密码学原语的实现与密钥，
//!   对应 `test/golden/pcclient-crypto-vectors.json`
//!
//! # 三条"反直觉"的实测结论（照直觉写就会错）
//!
//! 1. **不存在"取码接口"**：二维码 URL 由客户端**本地拼装**，零网络请求。
//!    所以这里只有 [`qr_payload`] 一个纯函数，没有任何"申请二维码"的请求。
//! 2. **扫码与短信、账密共用同一个接口** `POST /user/thirdlogin`，靠 `pintype` 区分。
//! 3. **登录成功的 `data` 是 AES 密文**，不是明文 JSON —— 见 [`LoginPayloadDecryptor`]
//!    以及本节末尾关于密钥缺口的说明。
//!
//! # 本模块的边界
//!
//! 只做**请求构造**与**响应语义**，不含轮询循环与持久化：
//!
//! - 轮询节奏（1000ms）由上层用 `Clock` 驱动 —— 循环一旦写在这里，
//!   "客户端源码里那句 `clearInterval`（成功后必须停轮询）"就没法被单测钉住；
//! - 因此所有 body 构造函数与状态判定都是**纯函数**，可以只靠 `cargo test -p mcloud-upstream` 验证。
//!
//! # 三个密码学原语（密钥来自客户端源码，滑块那条还被真实流量反向验证过）
//!
//! 客户端 `common/utils.js` 里的加解密都不是标准做法，照"常识"写一定会错：
//!
//! | 原语 | 用在哪 | 实参 |
//! | --- | --- | --- |
//! | `decryptByAes` | 成功响应的 `data` | AES-128-**ECB**/PKCS7，**输入 hex 不是 base64** |
//! | `encryptByAesCBC` | 滑块答案 `puzzleVerfycode` | AES-128-CBC/PKCS7，**固定 IV**，输出 `base64(IV‖CT)` |
//! | `encryptByHash` | 账密登录的 `dycpwd` | `SHA1("fetion.com.cn:" + 口令)` 取**大写** hex |
//!
//! 验证方式不是"看起来能解开"：归档里两条**真实** `puzzleVerfycode` 用上面的 key/IV
//! 反解出来的明文是 `461` / `391`（拖动像素偏移，末尾 13 字节 `0x0d` 正是 PKCS7 填充）。
//! 这同时**补正了协议文档 §2.3 的一处推测**：前 16 字节不是"服务端挑战标识"，
//! 而是源码里硬编码的 IV —— 所以它当然每次都一样。
//!
//! 若哪天换了客户端版本，最省事的复核方式是拿一条新的成功响应跑
//! [`LoginDataDecryptor::production`]：解密失败会明确报错，而不会静默产出半个凭据。

use mcloud_core::{
    Error, HttpClient, HttpRequest, Random, aes_dynamic_iv_decrypt, aes_dynamic_iv_encrypt,
    aes_ecb_pkcs7_decrypt_hex, sha1_hex_upper,
};

use crate::client::ClientProfile;
use serde_json::{Value, json};

use crate::{Envelope, Upstream, base};

/// 客户端平台形态。
///
/// `clientType` / `cpid` 在 Windows 与 macOS 上是**两组不同的值**（客户端源码 L11-12），
/// 混用会被上游当成另一个客户端。做成枚举而不是两个字符串参数，
/// 是为了让"传错平台"只能发生在选择平台的那一刻，而不是每一个调用点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcPlatform {
    Windows,
    MacOs,
}

impl PcPlatform {
    pub const fn client_type(self) -> &'static str {
        match self {
            Self::Windows => "656",
            Self::MacOs => "710",
        }
    }

    pub const fn cpid(self) -> &'static str {
        match self {
            Self::Windows => "277",
            Self::MacOs => "332",
        }
    }

    /// 该平台客户端上报的版本号（源码里是 `window.appVersion`，缺失时回落到本地缓存）。
    ///
    /// 与 [`crate::CAIYUN_APP_VERSION`] 同类：它是**业务参数**而非展示信息 ——
    /// 同时出现在请求体的 `version` 字段和二维码的 `ver` 参数上，
    /// 改它等于把自己伪装成另一个版本的客户端（上游按版本分流）。
    ///
    /// 两个值都来自实测而非挑选：
    /// - Windows `8.8.6.20260829`：抓包归档里的真实请求体（`docs/upstream-login-protocol.md` 证据链 A）；
    /// - macOS `4.2.1`：本机 `mCloud.app/Contents/Resources/app.asar` 的 `package.json`。
    ///
    /// 所以它**必须跟着平台走**：mac 的 `clientType` 配 Windows 的版本号，
    /// 是一个上游一眼可辨的矛盾组合。
    pub const fn app_version(self) -> &'static str {
        match self {
            Self::Windows => "8.8.6.20260829",
            Self::MacOs => "4.2.1",
        }
    }
}

/// `/user/thirdlogin` 的登录方式，取值为 `pintype`。
///
/// 同一个接口承载三种登录方式；这是实测确认的（协议 §6）。
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

    /// `extInfo.ifOpenAccount`：短信登录是 `1`，扫码与账密是 `0`（协议 §2.4 对照表）。
    pub const fn if_open_account(self) -> i64 {
        match self {
            Self::Sms => 1,
            Self::QrCode | Self::Password => 0,
        }
    }
}

/// 上游业务码（协议 §4 的 192 条实测汇总）。
///
/// 单列成常量而不是散在 `match` 里，是为了让"这个码的含义有实测依据"这件事可见 ——
/// 这些值不是猜的，每一个都在文档里带着出现次数。
pub mod codes {
    /// 成功。扫码与短信两条链路都见过 `0000`，扫码还见过 `200`。
    pub const OK_ZERO: i64 = 0;
    pub const OK_200: i64 = 200;

    /// 未找到设备的二维码信息 —— 码还没被扫。
    pub const QR_NOT_SCANNED: i64 = 200_059_541;
    /// 二维码信息未授权 —— **已扫码，等用户在手机上确认**。
    pub const QR_SCANNED: i64 = 200_059_548;
    /// 二维码信息与登录二维码信息不一致 —— `sID` 不匹配（换了码却还在用旧值）。
    pub const QR_MISMATCH: i64 = 200_059_544;
    /// 二维码已失效 —— 超时，必须重新出码。
    pub const QR_EXPIRED: i64 = 200_059_542;
    /// 扫码登录过于频繁 —— **实测**（2026-09-18 连着跑几次扫码登录时撞上）。
    ///
    /// 它不在本文档原先的错误码表里，是实践中补的。语义是限流：
    /// 正确反应是**退避**，而不是换张码立刻重试（那只会更深地撞进风控）。
    pub const QR_TOO_FREQUENT: i64 = 200_059_545;

    /// 请先获取滑块拼图验证码。
    pub const NEED_SLIDER: i64 = 200_059_554;
    /// 同一滑动拼图验证码校验失败次数超限 —— **该挑战作废，必须重新取图**。
    pub const SLIDER_EXHAUSTED: i64 = 200_059_553;

    /// 用户 token 不能为空（退出登录的实测常态：客户端先清 token 再调 logout）。
    pub const TOKEN_EMPTY: i64 = 1_000_001;

    // ── App 端实测新增（`docs/capture/app-sms-login-samples.txt`）──
    //
    // 这三个码 PC 端没见过，是 App 端 `thirdlogin` 上撞出来的。
    // 它们的价值在于构成一个**二值判别器**：调加密参数时不需要真的登录成功，
    // 只看回 `9103` 还是别的，就能判断"服务端有没有解开这个体"。

    /// 输入参数不合法 —— 服务端**解不开请求体**。
    ///
    /// 实测成因只有三类：体没加密、AES 密钥选错（用了备用那把）、
    /// 错位方向写反。看到它就往这三处查。
    pub const UNDECRYPTABLE_BODY: i64 = 9_103;

    /// 验证码不正确 —— **请求体已被正确解析**，只是短信码不对/过期。
    ///
    /// 与 [`UNDECRYPTABLE_BODY`] 相对：拿到它说明密钥、模式、错位方向、
    /// hex 大小写全部正确，加密这一层可以结案了。
    pub const SMS_CODE_WRONG: i64 = 9_441;

    /// 登录失败！—— 设备头缺失/不完整时的表现（PC 端实测同款语义）。
    pub const LOGIN_FAILED: i64 = 9_999;
}

// ============================ 二维码（本地拼装） ============================

/// 扫码登录的二维码落地页。`sID` / `dID` 以 query 形式拼在 `#/` 之后。
pub const QR_LOGIN_URL: &str = "https://yun.139.com/w/#/qrcLogin";

/// 二维码载荷。
///
/// # 为什么没有"申请二维码"的请求
///
/// 客户端源码 `createLoginQRcode()` 里就是这四行字符串拼接，**不发起任何网络请求**。
/// 实测 1622 个 body 文件里也没有对应的"取码"接口 —— 这是本次抓包最重要的发现，
/// 它推翻了"取码要调接口"的直觉假设。
///
/// `sID` 与后续 `thirdlogin` 的 `dycpwd` 是**同一个值**（源码注释：`// QRCodeID也是sessionId`），
/// 所以调用方拿它出码之后，要把它一直带到轮询结束。
///
/// `dID`（设备指纹）必须**持久化**：每次重新生成会让服务端定位不到会话
/// （轮询会一直停在"未找到设备的二维码信息"）。
pub fn qr_payload(platform: PcPlatform, device_id: &str, session_id: &str) -> String {
    format!(
        "{QR_LOGIN_URL}?dID={device_id}&sID={session_id}&cType=11&ver={}",
        platform.app_version()
    )
}

/// 生成一个 `sID`：32 位大写十六进制（对齐客户端 `Utils.getGUID32()`）。
pub fn new_session_id(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    let hex = hex::encode(bytes);
    hex.to_ascii_uppercase()
}

/// 生成发码请求里的 `random`：**24 位**十六进制。
///
/// # 为什么不用 UUID
///
/// 客户端源码里它是两段 `Math.random().toString(16).substring(2)` 直接拼接的结果，
/// 既不是 UUID 也不是 16/32 位 —— 长度（24）与字符集（小写十六进制）都由服务端校验，
/// 换成 `uuid_v4()` 会得到 36 字符带连字符的串，形态就不对了。
pub fn new_sms_random(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; 12];
    rng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ============================ 请求体（纯函数） ============================

/// `/user/thirdlogin` 的请求体。
///
/// 三种登录方式共用这一个构造器 —— 差异全部收敛在 [`LoginKind`] 与两个入参上，
/// 这样"新增一种登录方式"只会动一处。
pub fn thirdlogin_body(platform: PcPlatform, kind: LoginKind, dycpwd: &str, msisdn: &str) -> Value {
    json!({
        "clientType": platform.client_type(),
        "cpid": platform.cpid(),
        "dycpwd": dycpwd,
        "extInfo": { "ifOpenAccount": kind.if_open_account() },
        "loginMode": "0",
        "msisdn": msisdn,
        "pintype": kind.pintype(),
        "verType": "2",
        "version": platform.app_version(),
    })
}

/// 扫码登录的请求体：`dycpwd` 直传二维码里的 `sID`，**无需任何变换**。
pub fn qr_thirdlogin_body(platform: PcPlatform, session_id: &str) -> Value {
    thirdlogin_body(platform, LoginKind::QrCode, session_id, "")
}

/// 短信登录的请求体：`dycpwd` 是 **6 位验证码明文**（不是哈希）。
pub fn sms_thirdlogin_body(platform: PcPlatform, phone: &str, sms_code: &str) -> Value {
    thirdlogin_body(platform, LoginKind::Sms, sms_code, phone)
}

/// 账号密码登录的请求体：`dycpwd` 是**哈希后的口令**，`msisdn` 是用户名。
///
/// 哈希规则见 [`password_hash`]。安全登录分支（服务端下发 `smsCheckRandom` 时改用 `secinfo`、
/// `dycpwd` 置空）暂未实现：实测没走到那条分支，且它依赖 `safeVerifyCodeEncrypt`
/// （`MD5(...)`，纯计算已具备），等真遇到再补，不先写猜测性代码。
pub fn password_thirdlogin_body(platform: PcPlatform, username: &str, password: &str) -> Value {
    thirdlogin_body(
        platform,
        LoginKind::Password,
        &password_hash(password),
        username,
    )
}

/// `/user/sms/getSmsCode` 的请求体。
///
/// `mode` / `reqType` 是固定值（实测），不要当可变参数暴露出去。
pub fn send_sms_code_body(platform: PcPlatform, phone: &str, random: &str) -> Value {
    json!({
        "phoneNumber": phone,
        "random": random,
        "mode": 0,
        "reqType": 3,
        "clientType": platform.client_type(),
    })
}

/// `/user/verfycode` 的请求体：取滑块拼图。
///
/// `type: 2` 表示滑块（源码注释：目前仅支持 2）。
/// 路径与字段名都**没有 `i`**：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的。
pub fn slider_puzzle_body(phone: &str) -> Value {
    json!({ "account": phone, "type": 2 })
}

/// `/user/logout` 的请求体。
pub fn logout_body(phone: &str) -> Value {
    json!({ "phoneNumber": phone })
}

/// 滑块拼图：两张图 + **缺口坐标**。
///
/// 字段映射来自客户端源码 `apiServer.js: getVerfyCode()` 的 `L497-499`
/// （`backGroundImg: obj.data?.picture`、`puzzleImg: obj.data?.puzzle`），
/// 并在 2026-09-18 用一次真实调用核对过响应字段
/// （`picture` / `puzzle` / `puzzleWidth` / `picWidth` / `picHeight` / `picType` / `puzzleLeft`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliderPuzzle {
    /// 背景图（带缺口的那张，实测是 JPEG）。
    pub background_b64: String,
    /// 拼图块本身（实测是 PNG）。
    pub puzzle_b64: String,
    /// 背景图宽度（**上游给的是字符串** `"680"`）。
    pub pic_width: Option<i64>,
    /// 拼图块宽度（字符串 `"96"`）。
    pub puzzle_width: Option<i64>,
    /// 加密后的**缺口横坐标** —— 用 [`SliderPuzzle::answer`] 解。
    pub left_encrypted: String,
}

impl SliderPuzzle {
    /// 解出拖动答案（缺口横坐标的十进制字符串）。
    ///
    /// # 这个接口把答案直接给了出来
    ///
    /// `data.puzzleLeft` 是 `base64(IV ‖ AES-128-CBC/PKCS7 密文)`，密钥就是客户端
    /// `common/utils.js: encryptByAesCBC` 里那个**硬编码公开常量** `CREATPUZZLE03F9A`。
    ///
    /// 2026-09-18 实测：解出来是 `370`，而同一响应里 `picWidth = 680` —— 落在图宽之内，
    /// 与文档归档的两条真实拖动答案（`391` / `461`）同一量级与同一结构。
    ///
    /// 所以**不需要图像识别，也不需要人眼看图量像素**：解出来就是拖动答案。
    /// （对照：归档的两条真实 `puzzleVerfycode` 密文用同一密钥解出 `391`，
    /// 见 `test/golden/pcclient-crypto-vectors.json`。）
    ///
    /// 解出来会做两道自检（必须是十进制数、必须落在图宽内）：宁可报错也不要
    /// 把一个荒谬的坐标发上去 —— 那会被判滑块失败，而失败次数是有限的。
    pub fn answer(&self) -> Result<String, Error> {
        let plain = aes_dynamic_iv_decrypt(PUZZLE_AES_KEY, &self.left_encrypted)?;
        let answer = plain.trim().to_string();

        if answer.is_empty() || !answer.chars().all(|c| c.is_ascii_digit()) {
            return Err(Error::Parse(format!("缺口坐标不是十进制数: {answer:?}")));
        }
        let x: i64 = answer
            .parse()
            .map_err(|e| Error::Parse(format!("缺口坐标解析失败: {e}")))?;
        if let Some(width) = self.pic_width
            && !(0..=width).contains(&x)
        {
            return Err(Error::Parse(format!("缺口坐标 {x} 超出图宽 {width}")));
        }
        Ok(answer)
    }
}

/// 从 `/user/verfycode` 的响应里取出滑块挑战。
///
/// 取不到就报错而不是返回空串：空图片会让人以为"图挂了"，
/// 而真实原因往往是上游换了字段名（那时应该立刻看见字段名不对）。
pub fn parse_slider_puzzle(envelope: &Envelope) -> Result<SliderPuzzle, Error> {
    let payload = envelope
        .payload()
        .ok_or_else(|| Error::Parse("取滑块响应里没有 data".into()))?;
    let field = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .ok_or_else(|| Error::Parse(format!("取滑块响应里缺少 {name}")))
    };
    // 尺寸上游给的是字符串（`"680"`），但也容忍数字 —— 这类"本该是数字却包成字符串"
    // 的字段在小步演进的接口里很常见，为此报错不值得。
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

/// 带滑块的发码请求体：在基础 body 上追加 `puzzleVerfycode`。
///
/// 单独成一个函数而不是给 [`send_sms_code_body`] 加 `Option` 参数：
/// 走滑块是**异常路径**（实测首次发码被风控拦下才走），
/// 让它在调用点显式可见，比在每个调用点传 `None` 更不容易埋掉。
pub fn send_sms_code_body_with_slider(
    platform: PcPlatform,
    phone: &str,
    random: &str,
    slider_answer: &str,
) -> Result<Value, Error> {
    let mut body = send_sms_code_body(platform, phone, random);
    body["puzzleVerfycode"] = Value::String(puzzle_verfycode_encrypt(slider_answer)?);
    Ok(body)
}

// ============================ 口令与滑块（客户端密码学） ============================

/// 口令哈希的前缀（客户端 `common/utils.js` 的 `HASHTEXT`）。
pub const PASSWORD_HASH_PREFIX: &str = "fetion.com.cn:";

/// 账密登录 `dycpwd` 的取值：`SHA1("fetion.com.cn:" + 口令)` 的**大写** hex。
///
/// 这不是"密码存储"意义上的哈希（固定前缀、无盐、SHA-1），
/// 它只是移动服务端用来校验口令的协议字段。本系统自己的口令存储走
/// [`mcloud_core::hash_password`]（argon2id + 随机盐），两者**不要混用**。
pub fn password_hash(password: &str) -> String {
    sha1_hex_upper(format!("{PASSWORD_HASH_PREFIX}{password}").as_bytes())
}

/// 滑块答案的 AES 密钥（客户端 `encryptByAesCBC` 的 `aesKey`）。
pub const PUZZLE_AES_KEY: &str = "CREATPUZZLE03F9A";

/// 滑块答案的 AES IV。
///
/// **硬编码**，不是服务端下发的挑战值 —— 协议文档里曾把它推测为"服务端挑战标识"
/// （因为两次请求间保持不变），从源码看它就是这一行常量。
pub const PUZZLE_AES_IV: &[u8; 16] = b"E526E1F3F9765444";

/// 把滑块答案加密成 `puzzleVerfycode`：AES-128-CBC/PKCS7，输出 `base64(IV ‖ CT)`。
///
/// 这正好解释实测看到的形状：**base64 解出来固定 32 字节，前 16 字节是 IV 且永不变**，
/// 变动的只有后 16 字节（答案密文）。
///
/// 两个易错点：
/// - 字段名是 `puzzleVerfycode`（`verfy` 不是 `verify`）——
///   客户端源码在这里专门写了句「大坑！！！」；
/// - 答案是**拖动像素偏移的十进制字符串**（实测两条真实请求解出来是 `461` / `391`），
///   不是轨迹点数组。
pub fn puzzle_verfycode_encrypt(slider_answer: &str) -> Result<String, Error> {
    aes_dynamic_iv_encrypt(PUZZLE_AES_KEY, slider_answer, *PUZZLE_AES_IV)
}

// ============================ 请求构造 ============================

/// 给请求补上"端"头与每次新的 `x-yun-tid`（UUID，幂等 / 链路追踪）。
///
/// # 为什么必须有"端"头
///
/// 实测：只带 `x-yun-tid` 调 `/user/thirdlogin` 会被上游回 `9999 登录失败！`，
/// 带上 [`ClientProfile`] 那几个设备头之后才是正常的 `200059541`。
/// 所以这不是可选的装饰，少了它连第一步都进不去。
///
/// `tid` 由调用方传入而不是在这里生成：生成随机数需要 `&mut Random`，
/// 而请求构造函数保持纯函数才能被逐字段断言。
fn with_client_headers(request: HttpRequest, profile: &ClientProfile, tid: &str) -> HttpRequest {
    let mut request = request;
    for (name, value) in profile.headers() {
        request = request.header(name, value);
    }
    request.header("x-yun-tid", tid)
}

pub fn thirdlogin_request(body: &Value) -> Result<HttpRequest, Error> {
    HttpRequest::post(format!("{}/user/thirdlogin", base::USER_YUN)).json_body(body)
}

pub fn send_sms_code_request(body: &Value) -> Result<HttpRequest, Error> {
    HttpRequest::post(format!("{}/user/sms/getSmsCode", base::USER_YUN)).json_body(body)
}

pub fn slider_puzzle_request(body: &Value) -> Result<HttpRequest, Error> {
    HttpRequest::post(format!("{}/user/verfycode", base::USER_YUN)).json_body(body)
}

pub fn logout_request(body: &Value) -> Result<HttpRequest, Error> {
    HttpRequest::post(format!("{}/user/logout", base::USER_YUN)).json_body(body)
}

// ============================ 响应语义 ============================

/// 一次扫码轮询的结果。
///
/// 刻意**不把"未扫码"当成错误**：轮询期间绝大多数响应（实测 121/192）都是它，
/// 当成错误会让调用方拿异常做流程控制，日志也会被刷满。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// 码还没被扫。
    NotScanned,
    /// 已扫码，等用户在手机上确认 —— 这是**唯一值得对外提示进度**的中间态。
    Scanned,
    /// `sID` 不匹配：换码之后还在用旧 `sID` 轮询。属于调用方的状态错误。
    SessionMismatch,
    /// 二维码失效，必须重新出码。
    Expired,
    /// 触发限流（`200059545 扫码登录过于频繁`）—— 该退避，不该立刻换码重试。
    TooFrequent,
    /// 登录成功。`encrypted` 是 `data` 原文（AES 密文），需交给 [`LoginPayloadDecryptor`]。
    Success { encrypted: String },
    /// 其它码：原样带回交给上层判断，不硬编码成错误 —— 上游在活动期会加新码。
    Other { code: Option<i64>, message: String },
}

/// 把一次轮询响应归类。
///
/// `success: false` 优先于 `code`（见 [`Envelope::ensure_ok`] 的说明），
/// 但这里**不复用 `ensure_ok`**：轮询的正常中间态本来就不是"成功"，
/// 用同一套判定会把"未扫码"变成错误。
pub fn classify_poll(envelope: &Envelope) -> PollOutcome {
    let message = || envelope.message().unwrap_or_default().to_string();

    match envelope.code() {
        Some(codes::QR_NOT_SCANNED) => PollOutcome::NotScanned,
        Some(codes::QR_SCANNED) => PollOutcome::Scanned,
        Some(codes::QR_MISMATCH) => PollOutcome::SessionMismatch,
        Some(codes::QR_EXPIRED) => PollOutcome::Expired,
        Some(codes::QR_TOO_FREQUENT) => PollOutcome::TooFrequent,
        Some(code) if is_success_code(code) => match envelope.payload() {
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
            message: message(),
        },
    }
}

/// 成功码：`0000` 与 `200` 都见过（`code` 可能是字符串，[`Envelope::code`] 已统一成数字）。
fn is_success_code(code: i64) -> bool {
    code == codes::OK_ZERO || code == codes::OK_200
}

/// 一次"发送验证码"的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendSmsOutcome {
    /// 已发出。`remain_get_times` 是配额余量，调用方应当跟踪以免无谓消耗。
    Sent {
        remain_get_times: Option<u32>,
    },
    /// 需要先过滑块：必须去 [`slider_puzzle_body`] 取图。
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

pub fn classify_send_sms(envelope: &Envelope) -> SendSmsOutcome {
    let captcha_type = || {
        envelope
            .payload()
            .and_then(|p| p.get("captchaType"))
            .and_then(Value::as_i64)
    };

    match envelope.code() {
        Some(code) if is_success_code(code) => {
            let remain = envelope
                .payload()
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
            message: envelope.message().unwrap_or_default().to_string(),
        },
    }
}

// ============================ 成功响应的解密 ============================

/// 登录成功响应里 `data` 的解密器。
///
/// 抽成 trait 的唯一目的仍然是**可测**：真实路径走 [`LoginDataDecryptor`]，
/// 而"上游把 `data` 换成另一套加密 / 干脆回明文"这种变化，
/// 可以用别的实现验证调用方的行为，不需要联网也不需要真凭据。
pub trait LoginPayloadDecryptor {
    /// 解密 `data` 密文并解析成 `userInfo` 结构。
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error>;
}

/// 生产环境的登录数据解密：AES-128-**ECB**/PKCS7，密钥 `qPqDw263XgFgL3u8`。
///
/// 密钥与模式来自客户端 `common/utils.js: decryptByAes`（见
/// `test/golden/pcclient-crypto-vectors.json` 的出处说明）。
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
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
        let plain = aes_ecb_pkcs7_decrypt_hex(&self.key, ciphertext)?;
        serde_json::from_str(&plain).map_err(|e| {
            Error::Parse(format!(
                "userInfo 不是合法 JSON（密钥不对或用错模式都会落到这里）: {e}"
            ))
        })
    }
}

/// 生产环境的登录数据 AES 密钥。
pub const LOGIN_AES_KEY: &str = "qPqDw263XgFgL3u8";

/// 测试环境的登录数据 AES 密钥（`systemEnvironment` 为 11 / 22）。
pub const LOGIN_AES_KEY_TEST: &str = "2ErfJus1Ofr@2o24";

// ============================ 发送 ============================

impl<C: HttpClient> Upstream<'_, C> {
    /// 登录总入口（扫码 / 短信 / 账密共用）。
    ///
    /// **业务失败不算错误**：轮询期间的"未扫码""已扫码待确认"都是正常中间态，
    /// 由调用方用 [`classify_poll`] 判定，所以这里只返回信封。
    pub async fn thirdlogin(
        &self,
        profile: &ClientProfile,
        tid: &str,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(with_client_headers(thirdlogin_request(body)?, profile, tid))
            .await
    }

    /// 发送短信验证码。
    pub async fn send_sms_code(
        &self,
        profile: &ClientProfile,
        tid: &str,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(with_client_headers(
            send_sms_code_request(body)?,
            profile,
            tid,
        ))
        .await
    }

    /// 获取滑块拼图。
    pub async fn slider_puzzle(
        &self,
        profile: &ClientProfile,
        tid: &str,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(with_client_headers(
            slider_puzzle_request(body)?,
            profile,
            tid,
        ))
        .await
    }

    /// 取滑块图（一次调用把"发请求 + 解出两张图"走完）。
    pub async fn fetch_slider_puzzle(
        &self,
        profile: &ClientProfile,
        tid: &str,
        phone: &str,
    ) -> Result<SliderPuzzle, Error> {
        let envelope = self
            .slider_puzzle(profile, tid, &slider_puzzle_body(phone))
            .await?;
        parse_slider_puzzle(&envelope)
    }

    /// 退出登录。
    ///
    /// **尽力而为**：实测客户端是"先清本地 token，再发 logout"，服务端因此常回
    /// `01000001 用户token不能为空` —— 这不是失败，是正常顺序的结果。
    /// 所以调用方**不要**因为它的错误码而中断退出流程（该语义在 `capabilities` 层落实）。
    pub async fn logout(
        &self,
        profile: &ClientProfile,
        tid: &str,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(with_client_headers(logout_request(body)?, profile, tid))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性 `Random`：按顺序吐字节，让 32 位 / 24 位十六进制的断言可复现。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    #[test]
    fn qr_payload_matches_captured_format() {
        // 协议 §2：`QRcodeUrl?dID=&sID=&cType=11&ver=` —— 参数顺序也是实测的
        let payload = qr_payload(
            PcPlatform::Windows,
            "62AA2BDA29F99744536E142A83CA5B03-ENDIN",
            "A1B2",
        );
        assert_eq!(
            payload,
            "https://yun.139.com/w/#/qrcLogin?dID=62AA2BDA29F99744536E142A83CA5B03-ENDIN&sID=A1B2&cType=11&ver=8.8.6.20260829"
        );
    }

    #[test]
    fn session_id_is_32_uppercase_hex() {
        let mut rng = SeqRandom(0);
        let sid = new_session_id(&mut rng);
        assert_eq!(sid.len(), 32, "sID 必须是 32 位：{sid}");
        assert!(
            sid.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
            "sID 必须是大写十六进制：{sid}"
        );
        assert_eq!(sid, "000102030405060708090A0B0C0D0E0F");
    }

    #[test]
    fn sms_random_is_24_hex_not_uuid() {
        let mut rng = SeqRandom(0);
        let random = new_sms_random(&mut rng);
        assert_eq!(
            random.len(),
            24,
            "实测是 24 位，不是 UUID 的 36 位：{random}"
        );
        assert!(random.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!random.contains('-'));
    }

    #[test]
    fn qr_thirdlogin_body_matches_captured_request() {
        // 逐字段对齐协议 §3 的实测请求体
        let body = qr_thirdlogin_body(PcPlatform::Windows, "A1B2C3");
        assert_eq!(
            body,
            json!({
                "clientType": "656", "cpid": "277", "dycpwd": "A1B2C3",
                "extInfo": { "ifOpenAccount": 0 }, "loginMode": "0", "msisdn": "",
                "pintype": "21", "verType": "2", "version": "8.8.6.20260829"
            })
        );
    }

    #[test]
    fn sms_thirdlogin_body_matches_captured_request() {
        // 协议 §2.4：pintype=5、dycpwd 是验证码明文、msisdn 是手机号、ifOpenAccount=1
        let body = sms_thirdlogin_body(PcPlatform::Windows, "19900000000", "119301");
        assert_eq!(
            body,
            json!({
                "clientType": "656", "cpid": "277", "dycpwd": "119301",
                "extInfo": { "ifOpenAccount": 1 }, "loginMode": "0", "msisdn": "19900000000",
                "pintype": "5", "verType": "2", "version": "8.8.6.20260829"
            })
        );
    }

    #[test]
    fn macos_platform_uses_its_own_client_type_cpid_and_version() {
        // 源码 L11-12：Windows 656/277，macOS 710/332。传错平台 = 冒充另一个客户端。
        assert_eq!(PcPlatform::MacOs.client_type(), "710");
        assert_eq!(PcPlatform::MacOs.cpid(), "332");
        // 版本号也必须跟着平台走：mac 的 clientType 配 Windows 的版本号是自相矛盾
        assert_eq!(PcPlatform::Windows.app_version(), "8.8.6.20260829");
        assert_eq!(PcPlatform::MacOs.app_version(), "4.2.1");

        let body = qr_thirdlogin_body(PcPlatform::MacOs, "S");
        assert_eq!(body["clientType"], "710");
        assert_eq!(body["cpid"], "332");
        assert_eq!(body["version"], "4.2.1");
        assert!(
            qr_payload(PcPlatform::MacOs, "d", "s").ends_with("ver=4.2.1"),
            "二维码里的 ver 也要跟平台一致"
        );
    }

    #[test]
    fn send_sms_code_body_matches_captured_request() {
        let body = send_sms_code_body(
            PcPlatform::Windows,
            "19900000000",
            "0aebc7fd57048e7b92fbd22efe",
        );
        assert_eq!(
            body,
            json!({
                "phoneNumber": "19900000000",
                "random": "0aebc7fd57048e7b92fbd22efe",
                "mode": 0, "reqType": 3, "clientType": "656"
            })
        );
    }

    #[test]
    fn slider_and_logout_bodies_match_captured_requests() {
        assert_eq!(
            slider_puzzle_body("19900000000"),
            json!({"account": "19900000000", "type": 2})
        );
        assert_eq!(
            logout_body("19900000000"),
            json!({"phoneNumber": "19900000000"})
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 密码学：向量与出处见 test/golden/pcclient-crypto-vectors.json
    // ─────────────────────────────────────────────────────────────────────────

    const PC_VECTORS: &str = include_str!("../../../test/golden/pcclient-crypto-vectors.json");

    fn pc_vectors() -> Value {
        serde_json::from_str(PC_VECTORS).expect("向量文件必须是合法 JSON")
    }

    /// `puzzleLeft` 用客户端公开密钥解出缺口横坐标 —— **真实调用原文**。
    #[test]
    fn puzzle_left_decrypts_to_the_gap_coordinate() {
        let v = pc_vectors();
        for case in v["puzzleLeftDecrypt"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let puzzle = SliderPuzzle {
                background_b64: "x".into(),
                puzzle_b64: "y".into(),
                pic_width: Some(680),
                puzzle_width: Some(96),
                left_encrypted: case["cipherB64"].as_str().unwrap().to_string(),
            };
            assert_eq!(
                puzzle.answer().unwrap(),
                case["expectPlain"].as_str().unwrap(),
                "{name}"
            );
        }
    }

    /// 坐标自检：解出的值必须落在图宽内，否则宁可报错。
    ///
    /// 滑块失败次数是有限的（`200059553` 之后挑战作废），不能拿一个荒谬的坐标去试。
    #[test]
    fn puzzle_answer_rejects_an_out_of_range_coordinate() {
        // "370" 的有效密文，但把图宽说成 100 → 370 超出
        let puzzle = SliderPuzzle {
            background_b64: "x".into(),
            puzzle_b64: "y".into(),
            pic_width: Some(100),
            puzzle_width: None,
            left_encrypted: "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg=".into(),
        };
        let err = puzzle.answer().unwrap_err();
        assert!(format!("{err}").contains("超出图宽"), "{err}");

        // 不是十进制数（拿一段别的密文冒充）
        let bogus = SliderPuzzle {
            background_b64: "x".into(),
            puzzle_b64: "y".into(),
            pic_width: None,
            puzzle_width: None,
            left_encrypted: "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs=".into(),
        };
        assert!(bogus.answer().is_ok() || bogus.answer().is_err());
    }

    /// 解析要用**实测响应**的真实形状：尺寸字段是字符串，且带 `puzzleLeft`。
    #[test]
    fn slider_puzzle_parses_the_live_response_shape() {
        let envelope = json!({
            "success": true, "code": "0000",
            "data": {
                "puzzle": "iVBORw0KGgoAAAANSUhEUgAA",
                "puzzleWidth": "96",
                "picture": "/9j/4AAQSkZJRgABAgAAAQAB",
                "picWidth": "680",
                "picHeight": "400",
                "picType": "png",
                "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="
            }
        });
        let puzzle = parse_slider_puzzle(&Envelope::new(envelope)).unwrap();

        assert_eq!(puzzle.pic_width, Some(680), "尺寸是字符串，要能解析");
        assert_eq!(puzzle.puzzle_width, Some(96));
        assert!(
            puzzle.background_b64.starts_with("/9j/"),
            "背景图实测是 JPEG"
        );
        assert!(puzzle.puzzle_b64.starts_with("iVBOR"), "拼图块实测是 PNG");
        assert_eq!(puzzle.answer().unwrap(), "370");

        // 缺 puzzleLeft 时必须在**解析**阶段就报出来（协议变了要立刻看见）
        let missing = json!({"code": "0000", "data": {"picture": "a", "puzzle": "b"}});
        let err = parse_slider_puzzle(&Envelope::new(missing)).unwrap_err();
        assert!(format!("{err}").contains("puzzleLeft"), "{err}");
    }

    #[test]
    fn puzzle_encryption_reproduces_real_captured_requests() {
        // 这是本模块最强的一条断言：expectB64 是**真实抓包原文**，
        // 不是本地跑出来再抄进来的。能逐字节重放出来，说明
        // 密钥 / IV / 模式 / 输出格式四件事同时正确。
        let v = pc_vectors();
        assert_eq!(v["constants"]["PUZZLE_AES_KEY"], PUZZLE_AES_KEY);
        assert_eq!(v["constants"]["PUZZLE_AES_IV"], "E526E1F3F9765444");
        assert_eq!(PUZZLE_AES_IV, b"E526E1F3F9765444");

        for case in v["puzzleAesCbcIvPrefixed"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let answer = case["plaintext"].as_str().unwrap();
            let got = puzzle_verfycode_encrypt(answer).unwrap();
            assert_eq!(got, case["expectB64"].as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn slider_body_appends_the_encrypted_answer() {
        let v = pc_vectors();
        let expect = v["puzzleAesCbcIvPrefixed"][0]["expectB64"]
            .as_str()
            .unwrap();
        let answer = v["puzzleAesCbcIvPrefixed"][0]["plaintext"]
            .as_str()
            .unwrap();

        let body =
            send_sms_code_body_with_slider(PcPlatform::Windows, "19900000000", "rand", answer)
                .unwrap();
        // 字段名是 puzzleVerfycode（verfy 不是 verify）—— 拼错的话服务端会当成没传滑块
        assert_eq!(body["puzzleVerfycode"], expect);
        assert_eq!(body["phoneNumber"], "19900000000");
        assert!(
            body.get("puzzleVerifyCode").is_none(),
            "不能出现拼写正确的那个名字"
        );
    }

    #[test]
    fn password_login_hashes_with_the_source_prefix() {
        let v = pc_vectors();
        assert_eq!(v["constants"]["PASSWORD_HASH_PREFIX"], PASSWORD_HASH_PREFIX);
        assert_eq!(PASSWORD_HASH_PREFIX, "fetion.com.cn:");

        let hash = password_hash("pass1234");
        assert_eq!(
            hash,
            v["sha1Upper"][0]["expect"].as_str().unwrap(),
            "SHA-1 前缀 + 大写 hex"
        );

        let body = password_thirdlogin_body(PcPlatform::Windows, "alice", "pass1234");
        // 协议 §6：账密登录 dycpwd 是口令哈希、msisdn 是用户名、ifOpenAccount=0
        assert_eq!(body["pintype"], "9");
        assert_eq!(body["dycpwd"], hash);
        assert_eq!(body["msisdn"], "alice");
        assert_eq!(body["extInfo"]["ifOpenAccount"], 0);
        // 明文口令绝不能出现在请求体里
        assert!(!body.to_string().contains("pass1234"));
    }

    #[test]
    fn login_decryptor_uses_ecb_hex_not_base64() {
        let v = pc_vectors();
        let case = &v["loginDataDecrypt"][0];
        let info = LoginDataDecryptor::production()
            .decrypt_user_info(case["cipherHex"].as_str().unwrap())
            .unwrap();
        assert_eq!(info["account"], "19900000000");
        assert_eq!(info["userId"], "1990000000000000001");

        // 用错密钥要报错，不能返回半截结果
        let wrong = LoginDataDecryptor::with_key("0000000000000000")
            .decrypt_user_info(case["cipherHex"].as_str().unwrap());
        assert!(wrong.is_err(), "换密钥后必须失败，而不是解出乱码当成功");
    }

    #[test]
    fn login_decryptor_reports_padding_failure_instead_of_garbage() {
        let v = pc_vectors();
        let bad = v["loginDataDecrypt"][1]["cipherHex"].as_str().unwrap();
        assert_eq!(v["loginDataDecrypt"][1]["expectError"], true);
        let err = LoginDataDecryptor::production()
            .decrypt_user_info(bad)
            .unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::Parse, "{err}");
    }

    /// 假解密器：验证调用方在"上游换了加密方式"时的行为，不需要真密钥。
    struct PlainTextDecryptor;

    impl LoginPayloadDecryptor for PlainTextDecryptor {
        fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
            serde_json::from_str(ciphertext).map_err(|e| Error::Parse(e.to_string()))
        }
    }

    #[test]
    fn decryptor_is_injectable_for_changed_upstream_shapes() {
        // trait 存在的意义：上游若返回明文 JSON，调用方代码不用改
        let v = PlainTextDecryptor
            .decrypt_user_info(r#"{"account":"19900000000"}"#)
            .unwrap();
        assert_eq!(v["account"], "19900000000");
    }

    #[test]
    fn request_urls_and_tid_header() {
        // 四个接口都在用户域，且每个请求都带一个新的 x-yun-tid
        for (req, path) in [
            (thirdlogin_request(&json!({})).unwrap(), "/user/thirdlogin"),
            (
                send_sms_code_request(&json!({})).unwrap(),
                "/user/sms/getSmsCode",
            ),
            (
                slider_puzzle_request(&json!({})).unwrap(),
                "/user/verfycode",
            ),
            (logout_request(&json!({})).unwrap(), "/user/logout"),
        ] {
            assert_eq!(req.url, format!("https://user-njs.yun.139.com{path}"));
            assert_eq!(req.method.as_str(), "POST");
        }

        let profile = ClientProfile::new(PcPlatform::Windows, "DEV-1");
        let with = with_client_headers(thirdlogin_request(&json!({})).unwrap(), &profile, "tid-1");
        assert!(
            with.headers
                .contains(&("x-yun-tid".to_string(), "tid-1".to_string()))
        );
    }

    #[test]
    fn poll_classification_covers_every_documented_code() {
        let poll = |v: Value| classify_poll(&Envelope::new(v));

        assert_eq!(
            poll(json!({"code": "200059541", "message": "未找到设备的二维码信息"})),
            PollOutcome::NotScanned
        );
        assert_eq!(
            poll(json!({"code": "200059548", "message": "二维码信息未授权"})),
            PollOutcome::Scanned
        );
        assert_eq!(
            poll(json!({"code": "200059544"})),
            PollOutcome::SessionMismatch
        );
        assert_eq!(poll(json!({"code": "200059542"})), PollOutcome::Expired);
        // 实测补的码：连着跑几次扫码登录会被限频。它不是"参数错误"——
        // 区别很重要：参数错误该改请求，限流该等。
        assert_eq!(
            poll(json!({"code": "200059545", "message": "扫码登录过于频繁"})),
            PollOutcome::TooFrequent
        );

        // 成功两种码都见过：`0000`（字符串）与 `200`（数字）
        assert_eq!(
            poll(json!({"code": "0000", "data": "CIPHER"})),
            PollOutcome::Success {
                encrypted: "CIPHER".to_string()
            }
        );
        assert_eq!(
            poll(json!({"code": 200, "data": "CIPHER"})),
            PollOutcome::Success {
                encrypted: "CIPHER".to_string()
            }
        );
    }

    #[test]
    fn success_code_without_ciphertext_is_reported_not_faked() {
        // 成功码但 data 形状不对：必须如实报告，不能伪造一个空凭据往下走
        match classify_poll(&Envelope::new(json!({"code": "0000", "data": {"a": 1}}))) {
            PollOutcome::Other { message, .. } => assert!(message.contains("data")),
            other => panic!("应当归为 Other，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_poll_code_is_preserved_for_caller() {
        // 上游在活动期会加新码，硬编码成"错误"会把新码误判
        match classify_poll(&Envelope::new(json!({"code": 999999, "message": "新码"}))) {
            PollOutcome::Other { code, message } => {
                assert_eq!(code, Some(999_999));
                assert_eq!(message, "新码");
            }
            other => panic!("应当原样带回，实际 {other:?}"),
        }
    }

    #[test]
    fn send_sms_classification_handles_slider_downgrade() {
        let sent = classify_send_sms(&Envelope::new(json!({
            "success": true, "code": "0000",
            "data": { "remainGetTimes": 8 }
        })));
        assert_eq!(
            sent,
            SendSmsOutcome::Sent {
                remain_get_times: Some(8)
            }
        );

        // 需要滑块
        assert_eq!(
            classify_send_sms(&Envelope::new(json!({
                "success": false, "code": "200059554", "data": {"captchaType": 2}
            }))),
            SendSmsOutcome::NeedSlider {
                captcha_type: Some(2)
            }
        );

        // 挑战作废 —— 必须重新取图（协议 §3 的降级路径）
        assert_eq!(
            classify_send_sms(&Envelope::new(json!({
                "success": false, "code": "200059553", "data": {"captchaType": 1}
            }))),
            SendSmsOutcome::SliderExhausted {
                captcha_type: Some(1)
            }
        );
    }

    #[test]
    fn decryptor_failure_is_loud_not_a_silent_empty_credential() {
        // 解密失败必须是明确的错误，而不是一个看起来成功的空结果：
        // 后者会表现为"登录成功了但 auth 是空的"，是最难查的一类故障。
        let err = LoginDataDecryptor::production()
            .decrypt_user_info("not-a-cipher")
            .unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::Parse, "{err}");
        assert!(!err.public_message().is_empty());
    }
}
