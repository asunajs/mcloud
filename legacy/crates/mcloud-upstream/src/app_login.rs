//! 移动云盘 **App 端（手机客户端）验证码登录**。
//!
//! # 规格来源
//!
//! 全部依据实测，不含猜测性字段：
//!
//! - `docs/capture/app-sms-login-samples.txt`：iOS App 12.6.0 的完整登录抓包
//!   （`getSmsCode` → 滑块 H5 → `thirdlogin` → 会话载荷）
//! - 反汇编 `MCLoginCryptoUtil` / `MCLoginService`（`mCloud_iPhone` 99.99% 未加密，
//!   可直接 `otool -tV`）—— 加解密算法的唯一依据
//! - 活体探针：合成号码打真实上游，`9441` / `9103` 构成二值判别器（见 §证据）
//!
//! # 与 PC 端（[`crate::login`]）的关系：**同一个接口，两套形状**
//!
//! `POST /user/thirdlogin` 两端都调，但**没有一处是可以复用的**：
//!
//! | | PC 端 | App 端 |
//! | --- | --- | --- |
//! | `pintype` | `21` / `5` / `9` | **`8`**（短信也是 8，不是 5） |
//! | `clienttype` | `656` / `710` | **`412`** |
//! | `cpid` | `277` / `332` | **`55`** |
//! | 请求体 | 明文 JSON | **AES-128-ECB + 末 10 位错位 的 hex** |
//! | 响应 `data` | AES-ECB hex | AES-ECB hex **+ 错位** |
//! | AES 密钥 | `qPqDw263XgFgL3u8` | **`lYzvvBMUjucw+h^R`** |
//! | `extInfo.ifOpenAccount` | **数字** `1` | **字符串** `"1"` |
//! | `getSmsCode` 的 `mode`/`reqType` | **数字** | **字符串** |
//!
//! 所以本模块**刻意不参数化平台**：把两端塞进一个泛型构造器，
//! 只会让"传错端"变成一个能静默通过的选项（而这正好是最容易犯的错 ——
//! 把 PC 的密钥搬到 App 路径上，上游会回 `9103`）。
//!
//! # 三层封装，第三层是 App 独有的
//!
//! ```text
//! 第 1 层  明文 JSON
//! 第 2 层  AES-128-ECB/PKCS7  →  384 个 hex 字符
//! 第 3 层  末 10 个字符移到最前（错位）        ← App 独有，PC 没有
//! ```
//!
//! 第 3 层是 `MCLoginCryptoUtil +privateEncrypt:` 里那两行
//! `substringFromIndex:` / `substringToIndex:` 拼出来的。它**不是加密**，
//! 只是一次字符串旋转 —— 但少做这一步，服务端会回 `9103 输入参数不合法`，
//! 因为服务端要先反旋转再解密。
//!
//! # 验证码这条链路：`puzzleVerfycode` **就是纯协议入口**（2026-09-19 补测更正）
//!
//! 早先抓包看到三条 `getSmsCode` 的 `puzzleVerfycode` **全是空串**，
//! 于是本模块一度只把它做成"可传入的字符串参数、不做任何编码"，并留下一条
//! "App 端没有协议面、只能开 H5" 的结论。**那条结论是错的**，错在把
//! **客户端行为**当成了**协议能力**：
//!
//! - App 客户端确实**选择**了开 WebView（`200059554` 之后不重发 `getSmsCode`，
//!   短信由那个 H5 自己发），但那是它的实现选择，不是服务端的限制；
//! - 实测：把这个字段填成非空，服务端**就不再回 `200059554`** —— 它确实会处理它；
//! - 而**第一方滑块取图接口就在同一台主机上**（`base::USER_YUN`）：
//!   `POST /user/verfycode {"account":…,"type":2}`，**裸请求即可**
//!   （不需要 `Authorization`、不需要设备头、不需要 `x-yun-tid`）。
//!
//! 所以完整通路是（实测跑通，返回 `success:true` + `remainGetTimes` 递减）：
//!
//! ```text
//! getSmsCode(空) → 200059554
//!   → POST /user/verfycode 取图
//!   → puzzleLeft 用公开常量 CREATPUZZLE03F9A 解开 = 拖动坐标
//!   → getSmsCode(puzzleVerfycode = PC 那套编码的坐标) → 0000，短信已发
//! ```
//!
//! **编码与 PC 端完全一样**（[`crate::login::puzzle_verfycode_encrypt`]），
//! 所以这不是新协议，是同一个第一方滑块，只是 `clientType` 与请求头换成 App 的。
//!
//! ⚠ 正因如此，别把这一节读成"App 端的滑块已实现自动化"：过掉的是**借来的** PC 那套
//! 取题与编码。App 自己那条路（原生开 WebView、短信由 H5 发）没有做自动化，
//! 真机走的仍是它 —— 上面被推翻的只是"**协议**上不允许"那一层，
//! "真机在开 H5"这个观察从来没错过。
//!
//! H5（[`captcha_h5_url`]）退化为**兜底**：协议路径被限流时才交还给用户。
//!
//! ## 两个 IV 不是一回事（最容易搞错的地方）
//!
//! - **取题**响应的 `puzzleLeft`：**动态 IV**，就藏在它自己的前 16 字节里；
//! - **提交**答案的 `puzzleVerfycode`：**固定 IV** `E526E1F3F9765444`。
//!
//! 实测把提交侧也换成随机 IV 会失败。
//!
//! ## 这个接口**计失败次数**
//!
//! `200059553`（[`codes::SLIDER_EXHAUSTED`]）就是"该挑战作废、重新取图"的信号。
//! 所以**不能拿伪造答案反复试探** —— 补测时踩到过一次。
//! 每次都必须取**新题**并用**解出来的真坐标**。
//!
//! 依据：`docs/capture/app-sms-login-samples.txt` §2.1。

use mcloud_core::{
    Error, HttpClient, HttpRequest, Random, aes_ecb_pkcs7_decrypt_hex, aes_ecb_pkcs7_encrypt_hex,
    b64_encode, shift_head10_to_back, shift_tail10_to_front,
};
use serde_json::{Value, json};

use crate::app_client::{AppDevice, AppProfile};
use crate::login::{LoginPayloadDecryptor, codes};
use crate::{Envelope, Upstream, base};

// ============================ 密码学 ============================

/// App 端登录体的 AES 密钥（`isCurrentEnvironment = YES` 分支）。
///
/// 反汇编实证：`+[MCLoginCryptoUtil AESEncrypt:isCurrentEnvironment:]` 里是
/// `csel x20, keyA, keyB, ne` 选出来的**字面量**，不是从 `SecretKeyConfig` 取的
/// —— 所以它在 App 包里改名字也不影响我们，改的是这个常量本身。
pub const APP_LOGIN_AES_KEY: &str = "lYzvvBMUjucw+h^R";

/// 备用密钥（`isCurrentEnvironment = NO` 分支）。
///
/// **它不是"测试环境密钥"**：客户端只是按一个布尔位二选一，
/// 实测生产流量走的是 [`APP_LOGIN_AES_KEY`]。留它是因为
/// "用错这把会得到 9103"是排查密钥问题最快的判据。
pub const APP_LOGIN_AES_KEY_ALT: &str = "+09T5lHhO+smS7x@";

/// App 端登录体的加解密器：AES-128-ECB/PKCS7 + 末 10 位错位。
///
/// 与 PC 端的 [`crate::login::LoginDataDecryptor`] **不通用** ——
/// 它多了错位那一层，且密钥不同。刻意做成两个类型，
/// 就是为了让"用错端的解密器"在编译期就不可能发生。
#[derive(Debug, Clone)]
pub struct AppLoginCrypto {
    key: String,
}

impl AppLoginCrypto {
    /// 生产密钥（实测流量走这把）。
    pub fn production() -> Self {
        Self {
            key: APP_LOGIN_AES_KEY.to_string(),
        }
    }

    /// 备用密钥分支。
    pub fn alt_env() -> Self {
        Self {
            key: APP_LOGIN_AES_KEY_ALT.to_string(),
        }
    }

    /// 指定密钥（测试用）。
    pub fn with_key(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// 明文 → 线上请求体（hex 字符串）。
    pub fn encrypt_body(&self, plaintext: &str) -> Result<String, Error> {
        let ecb = aes_ecb_pkcs7_encrypt_hex(&self.key, plaintext)?;
        Ok(shift_tail10_to_front(&ecb))
    }

    /// 线上响应 `data` → 明文。
    ///
    /// 两步都**必须**成功：错位不改变字符集，所以"只反错位不解密"也能得到一个
    /// 看起来正常的 hex 串 —— 那种情况下会解出乱码而不是报错，
    /// 所以这里靠 AES 的 PKCS7 校验来兜底（解错必然填充非法）。
    pub fn decrypt_body(&self, wire_hex: &str) -> Result<String, Error> {
        let ecb = shift_head10_to_back(wire_hex.trim());
        aes_ecb_pkcs7_decrypt_hex(&self.key, &ecb)
    }
}

impl LoginPayloadDecryptor for AppLoginCrypto {
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
        let plain = self.decrypt_body(ciphertext)?;
        serde_json::from_str(&plain).map_err(|e| {
            Error::Parse(format!(
                "App 端登录载荷不是合法 JSON（密钥不对或漏了错位都会落到这里）: {e}"
            ))
        })
    }
}

// ============================ 请求体（纯函数） ============================

/// `getSmsCode` 请求体里的 `random`：**客户端硬编码的常量**。
///
/// 这不是"一个随机值的示例"，而是实测确认的**固定字面量**：
/// `FXTrace` 日志里跨 2026-08-12 ~ 09-18 的全部 4 次 `getSmsCode` 请求，
/// `random` 都是同一个 `qwert`（见 `docs/capture/app-sms-login-samples.txt` §1）。
/// 一个真正随机的值不可能跨一个月每次相同。
///
/// 所以它**不做成参数** —— 参数会暗示"这里可以也应该变化"，
/// 而实际上客户端从不变化它。要改这个值，应当先有新的抓包证据。
pub const SEND_SMS_RANDOM: &str = "qwert";

/// `/user/sms/getSmsCode` 的请求体。
///
/// 三个**易错点**，全都在抓包里能对上：
///
/// 1. **所有值都是字符串**（PC 端这里 `mode` / `reqType` 是数字）——
///    `"mode":"0"` 不是 `"mode":0`；
/// 2. `puzzleVerfycode` 字段名是 `verfy` 不是 `verify`（与 PC 端同款拼写错误）；
/// 3. 首次请求它是**空串**，不是 `null`、也不是省略字段。
///
/// `random` 固定为 [`SEND_SMS_RANDOM`]（理由见该常量的文档）。
/// **注意它与 `thirdlogin` 的 `random` 不是同一个值** —— 后者是每请求随机的 6 字符。
pub fn send_sms_code_body(phone: &str, puzzle_verfycode: &str) -> Value {
    json!({
        "mode": "0",
        "phoneNumber": phone,
        "reqType": "3",
        "clientType": crate::app_client::ios::CLIENT_TYPE,
        "random": SEND_SMS_RANDOM,
        "puzzleVerfycode": puzzle_verfycode,
    })
}

/// 带滑块的发码请求体：在基础 body 上填上**已编码**的滑块答案。
///
/// # 为什么直接复用 PC 的 [`crate::login::puzzle_verfycode_encrypt`]
///
/// 因为实测证明这就是**同一个第一方滑块**：取图接口是同一个 `/user/verfycode`、
/// 同一台主机，编码出来也是同样的 `base64(固定IV ‖ AES-128-CBC/PKCS7)`。
/// 2026-09-19 实测：取新题解出坐标 `466`、按这套编码填进去，
/// 上游回 `{"success":true,"code":"0000","data":{"remainGetTimes":8,...}}`，短信真的发出去了。
///
/// 单独成函数而不是给 [`send_sms_code_body`] 加参数，理由与 PC 端同款：
/// 走滑块是**异常路径**，让它在调用点显式可见比埋一个 `Option` 更难忽略。
pub fn send_sms_code_body_with_slider(phone: &str, slider_answer: &str) -> Result<Value, Error> {
    let encoded = crate::login::puzzle_verfycode_encrypt(slider_answer)?;
    Ok(send_sms_code_body(phone, &encoded))
}

/// `/user/thirdlogin` 的请求体（**明文**，加密由 [`AppLoginCrypto::encrypt_body`] 负责）。
///
/// 字段表来自抓包（`docs/capture/app-sms-login-samples.txt` §3.1），逐字段：
///
/// | 字段 | 值 | 备注 |
/// | --- | --- | --- |
/// | `cpid` | `"55"` | App 端固定 |
/// | `clienttype` | `"412"` | **全小写 `t`**，与 `getSmsCode` 的 `clientType` 大小写不同 |
/// | `pintype` | `"8"` | **短信登录也是 8**，不是 PC 的 5 |
/// | `dycpwd` | 6 位验证码**明文** | 不做哈希（账密登录才哈希） |
/// | `loginMode` / `verType` | `"0"` / `"1"` | App 端独有 |
/// | `extInfo.ifOpenAccount` | `"1"` | **字符串**，PC 端这里是数字 |
/// | `version` | `"12.6.0"` | 与设备串第 4 段一致 |
/// | `random` | 6 字符 | 与 `getSmsCode` 的 `random` **不是同一个值** |
///
/// 键序与客户端字典序不同（`serde_json` 默认按键排序）—— 实测这不影响服务端：
/// 服务端是"先解密再解析 JSON"，键序不参与任何校验（活体探针验证过两种键序都回 `9441`）。
pub fn sms_thirdlogin_body(phone: &str, sms_code: &str, random: &str) -> Value {
    json!({
        "cpid": crate::app_client::ios::CPID,
        "extInfo": { "ifOpenAccount": "1" },
        "dycpwd": sms_code,
        "pintype": "8",
        "loginMode": "0",
        "verType": "1",
        "version": crate::app_client::ios::APP_VERSION,
        "msisdn": phone,
        "random": random,
        "clienttype": crate::app_client::ios::CLIENT_TYPE,
    })
}

/// 生成登录体里的 `random`：6 位大小写字母 + 数字。
///
/// 形状取自实测（`hOjseB`）。它**不参与任何可观测的校验** ——
/// 依据是：`getSmsCode` 那个同名字段是硬编码常量 [`SEND_SMS_RANDOM`]
/// （`qwert`），服务端照样正常受理，说明这个字段只被串联/回显。
/// 所以这里不追求"某个精确长度"，只保证字符集与量级一致。
///
/// 但要诚实记一笔：**`thirdlogin` 的 `random` 只有 1 个样本**，
/// "每次不同"是从"客户端为什么要随机"推的，不是实测到的。
/// 它是否为定值，需要第二次登录抓包才能确认。
pub fn new_login_random(rng: &mut impl Random) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    rng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

// ============================ 滑块验证码 H5 ============================

/// 滑块验证码 H5 的落地页。
///
/// # 它为什么是个纯函数
///
/// 与 PC 端的二维码一样（见 [`crate::login::qr_payload`]），这个 URL 是
/// **客户端本地拼装**的，零网络请求：`200059554` 之后 App 直接开 WebView 加载它。
///
/// # `data` 参数里有什么
///
/// `base64(JSON)`，JSON 是三段：
///
/// ```json
/// {
///   "headers": { "x-yun-app-channel": "10000020",
///                "x-yun-client-info": "1|192.168.3.179|2|12.6.0|…|" },
///   "captchaType": 2,
///   "params": { "clientType": "412", "reqType": "3",
///               "nationCode": "+86", "phoneNumber": "19900000000" }
/// }
/// ```
///
/// 关键结论：**H5 页自己带着完整请求头和手机号**，也就是说
/// "过滑块"与"发短信"是 H5 会话内完成的，原生侧只等回调。
/// 这就是为什么抓包里 `200059554` 之后**没有**第二次 `getSmsCode`。
///
/// 注意 `headers` 里**只有两个头**（`x-yun-app-channel` 与 `x-yun-client-info`），
/// 不是 [`AppProfile::headers`] 的全集 —— 别顺手把整个头集合塞进去。
pub fn captcha_h5_url(profile: &AppProfile, phone: &str) -> String {
    let payload = json!({
        "headers": {
            "x-yun-app-channel": crate::app_client::ios::CHANNEL,
            // 第 14 段（`x-DeviceName` 的 base64）之后的 `|0|;` 在这里**没有**，
            // 与 x-yun-client-info 请求头不同 —— 抓包原样如此。
            "x-yun-client-info": client_info_for_captcha(profile.device()),
        },
        "captchaType": 2,
        "params": {
            "clientType": crate::app_client::ios::CLIENT_TYPE,
            "reqType": "3",
            "nationCode": "+86",
            "phoneNumber": phone,
        },
    });
    let data = b64_encode(payload.to_string().as_bytes());
    format!("{CAPTCHA_H5_BASE}?data={data}")
}

/// 滑块验证码 H5 的基址（实测）。
pub const CAPTCHA_H5_BASE: &str = "https://yun.139.com/fe/captcha/";

/// H5 的 `data` 里那个 `x-yun-client-info`。
///
/// 与 [`AppDevice::client_info`] 的唯一差别是**末尾的 `0|;` 没有了**（`|` 留着）——
/// 抓包原值是 `…|aVBhZA==|`，而请求头里是 `…|aVBhZA==|0|;`。
/// 这个差异很小但真实存在，所以单列一个函数而不是加个布尔开关
/// （开关会在调用点被传错，而这种差异一旦传错就是"滑块过不去"且看不出原因）。
fn client_info_for_captcha(device: &AppDevice) -> String {
    let full = device.client_info();
    full.strip_suffix("0|;").unwrap_or(&full).to_string()
}

// ============================ 请求构造 ============================

/// 给请求补上 App 端档案的头。
///
/// `timestamp` 由调用方注入（微秒级 Unix 时间戳），而不是在这里取系统时间 ——
/// 请求构造函数保持纯函数才能被逐字段断言。
///
/// # 为什么先删一遍 `content-type`
///
/// [`HttpRequest::json_body`] 会**自己**塞一个 `content-type: application/json`。
/// 实测上游收的是带 charset 的 `application/json;charset=UTF-8`，
/// 所以这里要覆盖它 —— 但 HTTP 头是**同名可重复**的列表，
/// 直接 push 会发出两个 `Content-Type`（实测过的坑：reqwest 会拼成
/// `application/json, application/json;charset=UTF-8`，上游按前者解析）。
/// 因此这里先把同名头删干净，再加唯一一个。
pub(crate) fn with_app_headers(
    request: HttpRequest,
    profile: &AppProfile,
    timestamp: i64,
) -> HttpRequest {
    let mut request = request;
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
    for (name, value) in profile.headers(timestamp) {
        request = request.header(name, value);
    }
    request.header("Content-Type", "application/json;charset=UTF-8")
}

/// `getSmsCode` 请求（**明文 JSON**，这一条不加密）。
///
/// # 为什么**不发** `hcy-cool-flag`
///
/// 抓包里 App 是发的（`hcy-cool-flag: 1`，三条请求都有）。但实测发现：
/// **这个头只要出现，服务端就把响应换成加密体** ——
///
/// | 请求 | 响应 |
/// | --- | --- |
/// | 不带 `hcy-cool-flag` | `{"success":false,"code":"200059554","message":"请先获取滑块拼图验证码","data":{"captchaType":2}}` |
/// | `hcy-cool-flag: 1` | `"<192 个 base64 字符>"`（裸 JSON 字符串，144 字节密文） |
/// | `hcy-cool-flag: 0` | 同上 —— 只要**出现**就切，与取值无关 |
///
/// 那 144 字节既不是 `MCLoginCryptoUtil` 那套（试过错位前后）、也不是
/// `MCStateCryptoHelper` 的 SM4-ECB（带上我们自己的 `yun-encrypt-*` 会话密钥试过，
/// 解不出来），响应头里也没有任何密钥 —— 密钥来源未知。
///
/// 所以这里**主动不发**这个头：发一个自己兑现不了的能力声明，换来的是
/// "服务端回了一个本客户端解不开的响应"，而它在**不发**时的语义与抓包完全一致
/// （同一个 `200059554`）。这是"照抄抓包"与"能跑起来"冲突时唯一说得通的选择。
///
/// 待办：等拿到 App 解这类响应时的密钥来源（大概率与 `MCStateCryptoHelper`
/// 的 `keyStatus`/`hadCache` 有关），再把开关打开。
pub fn send_sms_code_request(
    profile: &AppProfile,
    timestamp: i64,
    body: &Value,
) -> Result<HttpRequest, Error> {
    let request = with_app_headers(
        HttpRequest::post(format!("{}/user/sms/getSmsCode", base::USER_YUN)).json_body(body)?,
        profile,
        timestamp,
    );
    Ok(request)
}

/// `/user/verfycode` 请求（**明文**）：取**第一方**滑块拼图。
///
/// # 为什么 App 端也用它（而不是去开那个 H5）
///
/// 实测：这个接口就在 **App 自己的主机**上（`base::USER_YUN` 与 `getSmsCode` 是同一台），
/// 而且**裸请求就能用** —— 不需要 `Authorization`、不需要设备头、不需要 `x-yun-tid`。
/// 这里仍然叠上 App 的头，是因为**取题与提交答案必须用同一套头**才稳妥，
/// 而提交那一步（`getSmsCode`）本来就要带全套 App 头。
///
/// 路径与字段名都没有 `i`：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的
/// （与 PC 端同一个接口，所以直接复用 [`crate::login::slider_puzzle_body`]）。
pub fn slider_puzzle_request(
    profile: &AppProfile,
    timestamp: i64,
    phone: &str,
) -> Result<HttpRequest, Error> {
    let request = with_app_headers(
        HttpRequest::post(format!("{}/user/verfycode", base::USER_YUN))
            .json_body(&crate::login::slider_puzzle_body(phone))?,
        profile,
        timestamp,
    );
    Ok(request)
}

/// `thirdlogin` 请求（**加密体**）。
///
/// 传进来的是明文 [`Value`]，加密在这里完成 —— 这样"忘了加密"在类型层面
/// 就不可能发生（PC 端那个 `thirdlogin_request` 收的是已经构造好的 body，
/// 因为它不需要加密）。
///
/// 注意请求体的 Content-Type 仍然是 `application/json;charset=UTF-8`，
/// 尽管内容是 hex 文本 —— 抓包原样如此，别"顺手改成 text/plain"。
pub fn thirdlogin_request(
    profile: &AppProfile,
    timestamp: i64,
    crypto: &AppLoginCrypto,
    body: &Value,
) -> Result<HttpRequest, Error> {
    let plaintext = body.to_string();
    let wire = crypto.encrypt_body(&plaintext)?;

    let mut request = with_app_headers(
        HttpRequest::post(format!("{}/user/thirdlogin", base::USER_YUN)),
        profile,
        timestamp,
    );
    request.body = Some(wire.into_bytes());
    Ok(request)
}

// ============================ 响应语义 ============================

/// 一次"发送验证码"的结果。
///
/// **刻意不复用 PC 端的 [`crate::login::SendSmsOutcome`]**：两端的
/// `NeedSlider` 名字一样，**下一步动作完全不同** ——
///
/// - PC 端：去调 `POST /user/verfycode` 取图，本地识别后把答案
///   AES-CBC 加密成 `puzzleVerfycode` 再重发 `getSmsCode`；
/// - App 端：**开 WebView** 加载 [`captcha_h5_url`]，然后等一个回调
///   （发短信是 H5 干的，原生侧不会重发 `getSmsCode`）。
///
/// 共用一个枚举会让调用方写错分支却仍然编译通过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSendSmsOutcome {
    /// 已发出。`remain_get_times` 是配额余量。
    Sent {
        remain_get_times: Option<u32>,
    },
    /// 需要先过滑块拼图。
    ///
    /// **这是首次发码的常态**，不是错误：客户端应当接着去
    /// [`Upstream::app_fetch_slider_puzzle`] 取图、解出坐标、带答案重发
    /// （[`send_sms_code_body_with_slider`]）。只有当那条路也走不通
    /// （见 [`AppSendSmsOutcome::SliderExhausted`]）时，才把 H5 地址交给用户。
    NeedCaptcha {
        captcha_type: Option<i64>,
    },
    /// `200059553 同一滑动拼图验证码在单位时间内校验失败次数超限`
    /// —— **这个挑战作废了，必须重新取图**。
    ///
    /// 实测语义（补测时亲手撞到）：它**不是**"你这个账号被限流了"，
    /// 而是"这一道题的尝试次数用完了"。所以正确的反应是重取一道新题，
    /// 而不是放弃、也不是拿旧答案再试一次。
    SliderExhausted {
        captcha_type: Option<i64>,
    },
    Other {
        code: Option<i64>,
        message: String,
    },
}

/// 归类 `getSmsCode` 的响应。
pub fn classify_send_sms(envelope: &Envelope) -> AppSendSmsOutcome {
    let captcha_type = || {
        envelope
            .payload()
            .and_then(|p| p.get("captchaType"))
            .and_then(Value::as_i64)
    };

    match envelope.code() {
        Some(codes::OK_ZERO) | Some(codes::OK_200) => {
            let remain = envelope
                .payload()
                .and_then(|p| p.get("remainGetTimes"))
                .and_then(Value::as_u64)
                .map(|n| n as u32);
            AppSendSmsOutcome::Sent {
                remain_get_times: remain,
            }
        }
        Some(codes::NEED_SLIDER) => AppSendSmsOutcome::NeedCaptcha {
            captcha_type: captcha_type(),
        },
        Some(codes::SLIDER_EXHAUSTED) => AppSendSmsOutcome::SliderExhausted {
            captcha_type: captcha_type(),
        },
        other => AppSendSmsOutcome::Other {
            code: other,
            message: envelope.message().unwrap_or_default().to_string(),
        },
    }
}

/// 一次 App 端登录的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppLoginOutcome {
    /// 登录成功。`encrypted` 是 `data` 原文（密文），交给 [`AppLoginCrypto`] 解。
    Success {
        encrypted: String,
    },
    /// `9441 验证码不正确` —— **参数已被正确解析**，只是码不对/过期。
    ///
    /// 这个码是排查加密问题最有用的信号：看到它就说明密钥、模式、错位方向、
    /// hex 大小写全都对了。
    SmsCodeWrong,
    /// `9103 输入参数不合法` —— 服务端**解不开**请求体。
    ///
    /// 实测的三种成因（都亲手撞过）：体没加密、密钥用成了备用那把、
    /// 错位方向写反。排查顺序就按这个来。
    Undecryptable,
    /// `9999 登录失败！` —— 设备头缺失时的表现（PC 端实测同款）。
    LoginFailed,
    Other {
        code: Option<i64>,
        message: String,
    },
}

/// 归类 `thirdlogin` 的响应。
pub fn classify_login(envelope: &Envelope) -> AppLoginOutcome {
    match envelope.code() {
        Some(codes::OK_ZERO) | Some(codes::OK_200) => match envelope.payload() {
            Some(Value::String(cipher)) => AppLoginOutcome::Success {
                encrypted: cipher.clone(),
            },
            // 成功码但载荷不是密文字符串：如实报告形状不对，不伪造空凭据
            _ => AppLoginOutcome::Other {
                code: envelope.code(),
                message: "登录成功但 data 不是密文字符串".to_string(),
            },
        },
        Some(codes::SMS_CODE_WRONG) => AppLoginOutcome::SmsCodeWrong,
        Some(codes::UNDECRYPTABLE_BODY) => AppLoginOutcome::Undecryptable,
        Some(codes::LOGIN_FAILED) => AppLoginOutcome::LoginFailed,
        other => AppLoginOutcome::Other {
            code: other,
            message: envelope.message().unwrap_or_default().to_string(),
        },
    }
}

// ============================ 发送 ============================

impl<C: HttpClient> Upstream<'_, C> {
    /// App 端发送短信验证码（明文体）。
    pub async fn app_send_sms_code(
        &self,
        profile: &AppProfile,
        timestamp: i64,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(send_sms_code_request(profile, timestamp, body)?)
            .await
    }

    /// App 端取**第一方**滑块挑战（`/user/verfycode`，明文）。
    ///
    /// 解坐标不需要图像识别：上游把缺口横坐标加密后直接给了出来，
    /// 见 [`crate::login::SliderPuzzle::answer`]。
    pub async fn app_fetch_slider_puzzle(
        &self,
        profile: &AppProfile,
        timestamp: i64,
        phone: &str,
    ) -> Result<crate::login::SliderPuzzle, Error> {
        let envelope = self
            .call(slider_puzzle_request(profile, timestamp, phone)?)
            .await?;
        crate::login::parse_slider_puzzle(&envelope)
    }

    /// App 端登录（`thirdlogin`，加密体）。
    pub async fn app_thirdlogin(
        &self,
        profile: &AppProfile,
        timestamp: i64,
        crypto: &AppLoginCrypto,
        body: &Value,
    ) -> Result<Envelope, Error> {
        self.call(thirdlogin_request(profile, timestamp, crypto, body)?)
            .await
    }
}

#[cfg(test)]
mod tests;
