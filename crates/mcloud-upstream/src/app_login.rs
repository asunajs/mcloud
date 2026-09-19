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
fn with_app_headers(request: HttpRequest, profile: &AppProfile, timestamp: i64) -> HttpRequest {
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
mod tests {
    use super::*;
    use mcloud_core::Random;

    /// 确定性随机源：把给定字节循环填出去。
    ///
    /// 用它而不是真实 `OsRng`，是为了让 [`new_login_random`] 的断言可复现 ——
    /// "随机的形状"（长度、字符集）才是被测对象，具体取值不是。
    struct SeqRandom {
        bytes: Vec<u8>,
        at: usize,
    }

    impl SeqRandom {
        fn new(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                at: 0,
            }
        }
    }

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for slot in dst.iter_mut() {
                *slot = self.bytes[self.at % self.bytes.len()];
                self.at += 1;
            }
        }
    }

    /// 抓包里那台设备的脱敏形态（与 `docs/capture/app-sms-login-samples.txt` 一致）。
    fn profile() -> AppProfile {
        AppProfile::new(AppDevice::ipad(
            "AAA00000000000000000000000000000",
            "AAA000000000",
        ))
    }

    const PHONE: &str = "19900000000";
    const TS: i64 = 1_789_743_362_439_040;

    /// 抓包里那条明文的脱敏形态（186 字节）。
    fn captured_plaintext() -> String {
        sms_thirdlogin_body(PHONE, "429344", "hOjseB").to_string()
    }

    fn header<'a>(request: &'a HttpRequest, name: &str) -> &'a str {
        request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("缺少头 {name}"))
    }

    // ── 请求体：逐字段断言 ──

    #[test]
    fn send_sms_code_body_is_all_strings_and_has_the_typo_field() {
        let body = send_sms_code_body(PHONE, "");
        assert_eq!(
            body,
            json!({
                "mode": "0",
                "phoneNumber": "19900000000",
                "reqType": "3",
                "clientType": "412",
                "random": "qwert",
                "puzzleVerfycode": "",
            })
        );
        // 逐字段再钉一遍类型：PC 端这两个是**数字**，抄过来会得到 9103
        assert!(
            body["mode"].is_string(),
            "App 端 mode 是字符串，PC 端才是数字"
        );
        assert!(body["reqType"].is_string());
        assert!(body["clientType"].is_string());
        // 首次请求是**空串**，不是 null、也不是省略字段
        assert_eq!(body["puzzleVerfycode"], json!(""));
        // 字段名是 verfy 不是 verify（上游拼错了，照抄）
        assert!(body.get("puzzleVerifycode").is_none());
        assert!(body.get("puzzleVerfycode").is_some());
        // `random` 是**硬编码常量**，不是每请求随机：
        // 跨一个月的 4 次抓包都是同一个值（见常量文档）。
        assert_eq!(SEND_SMS_RANDOM, "qwert");
        assert_eq!(body["random"], json!(SEND_SMS_RANDOM));
    }

    #[test]
    fn thirdlogin_body_matches_the_captured_field_table() {
        let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
        assert_eq!(
            body,
            json!({
                "cpid": "55",
                "extInfo": { "ifOpenAccount": "1" },
                "dycpwd": "429344",
                "pintype": "8",
                "loginMode": "0",
                "verType": "1",
                "version": "12.6.0",
                "msisdn": "19900000000",
                "random": "hOjseB",
                "clienttype": "412",
            })
        );

        // 四处最容易被 PC 端带偏的地方，单独钉：
        assert_eq!(
            body["pintype"],
            json!("8"),
            "App 端短信登录也是 8，不是 PC 的 5"
        );
        assert_eq!(body["cpid"], json!("55"), "不是 PC 的 277/332");
        assert!(
            body["extInfo"]["ifOpenAccount"].is_string(),
            "App 端 ifOpenAccount 是字符串；PC 端是数字 1"
        );
        // 同一个字段名大小写不同：这里是小写 t 的 clienttype
        assert!(body.get("clienttype").is_some());
        assert!(
            body.get("clientType").is_none(),
            "getSmsCode 才叫 clientType"
        );
        // dycpwd 是验证码明文（账密登录才哈希）
        assert_eq!(body["dycpwd"], json!("429344"));
    }

    #[test]
    fn captured_plaintext_keeps_the_measured_length() {
        // 186 字节 —— 与抓包一致。密文长度是 384 字符，两者是同一个约束的两种表述。
        assert_eq!(captured_plaintext().len(), 186);
    }

    // ── 加解密 ──

    #[test]
    fn encrypt_body_produces_the_measured_wire_length() {
        let crypto = AppLoginCrypto::production();
        let wire = crypto.encrypt_body(&captured_plaintext()).unwrap();
        assert_eq!(wire.len(), 384, "186 字节明文 → 192 字节密文 → 384 hex");
        assert_eq!(wire, wire.to_ascii_lowercase(), "请求体是**小写** hex");
        assert!(wire.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn implementation_plaintext_is_byte_identical_to_the_live_probe() {
        // 这条把"活体探针"和"Rust 实现"接起来，是加密这一层能收工的依据：
        //
        //   探针送出下面这串（同样的键序、同样的值）→ 上游回 `9441 验证码不正确`
        //   → 说明**服务端成功解密**了它；
        //   而 Rust 的 `sms_thirdlogin_body(...).to_string()` 逐字节等于它
        //   → 所以 Rust 产出的请求体也一定被服务端解得开。
        //
        // 一旦有人改了字段名/字段类型/键序策略，这条会先炸 ——
        // 而不是要等一次真实登录才发现。
        assert_eq!(
            captured_plaintext(),
            r#"{"clienttype":"412","cpid":"55","dycpwd":"429344","extInfo":{"ifOpenAccount":"1"},"loginMode":"0","msisdn":"19900000000","pintype":"8","random":"hOjseB","verType":"1","version":"12.6.0"}"#
        );
        // 与抓包明文同长（186），但键序不同 —— 键序不参与服务端校验
        assert_eq!(captured_plaintext().len(), 186);
    }

    #[test]
    fn encrypt_body_matches_the_golden_vector() {
        // 这条密文由抓包侧的独立实现算出，钉的是"上游层有没有多加/少加一次错位"。
        //
        // 注意它的明文是 **serde_json 的键序**（按字母排序），与抓包里客户端的
        // 字典序不同 —— 所以这条密文**不等于**抓包里那条。
        // "与抓包逐字节相同"的那条断言在 core（`crypto.rs` 的
        // `app_login_body_is_ecb_hex_with_the_tail_ten_moved_to_the_front`），
        // 那里用的是抓包原始键序的明文。
        // 键序不参与服务端校验：活体探针两种键序都回 `9441`。
        let wire = AppLoginCrypto::production()
            .encrypt_body(&captured_plaintext())
            .unwrap();
        assert_eq!(
            wire,
            "17e30bcd98e6e4bdccc6b6e0c034688a3d2db656484a7de56ee67b2580af3dde4075b72cd78a11415da4b19ce641dbf7fad4a096f842da2556008fbbc3521d6ebf0499470b3dcf08532c25cf5ce6ac9052c20db6035c039af766e0206a772f94c3933802caaa302cc5e6d3c7160711a3d966cfc770568650de3a503048bb660f9e57597dc82cf15674c8f622823bc517ef2f7f51483fdff8b8191c6f1e6335d9c48aaba2d076b067ff091c65f3a107a60bd61352d9718a0466512bce9a4397b6"
        );
    }

    #[test]
    fn key_order_does_not_change_the_decryptable_payload() {
        // 抓包键序（客户端字典序）与实现键序（serde_json 字母序）都能被解回同样的字段集。
        // 这条是"键序不重要"的离线表述；联网版是活体探针（两种键序都回 9441）。
        let captured_order = concat!(
            r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","#,
            r#""pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","#,
            r#""msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#,
        );
        let crypto = AppLoginCrypto::production();
        let a: Value = serde_json::from_str(
            &crypto
                .decrypt_body(&crypto.encrypt_body(captured_order).unwrap())
                .unwrap(),
        )
        .unwrap();
        let b: Value = serde_json::from_str(
            &crypto
                .decrypt_body(&crypto.encrypt_body(&captured_plaintext()).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(a, b, "两种键序必须解出同一份字段");
        // 键序确实不同（否则这条测试就没意义了）
        assert_ne!(captured_order, captured_plaintext());
    }

    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let crypto = AppLoginCrypto::production();
        let plain = captured_plaintext();
        let wire = crypto.encrypt_body(&plain).unwrap();
        assert_eq!(crypto.decrypt_body(&wire).unwrap(), plain);
    }

    #[test]
    fn decrypt_body_accepts_the_response_side_shape() {
        // 响应 `data` 与请求体同一套算法 —— 抓包里响应是 13632 字符（6816 字节），
        // 这里用一个小载荷验证同一函数能解，不需要把 13 KB 的密文塞进测试。
        let crypto = AppLoginCrypto::production();
        let session = r#"{"account":"19900000000","belongsTo":2}"#;
        let wire = crypto.encrypt_body(session).unwrap();
        assert_eq!(crypto.decrypt_body(&wire).unwrap(), session);
        // 顺带钉住：`decrypt_user_info` 走的是同一个解密路径
        let v = crypto.decrypt_user_info(&wire).unwrap();
        assert_eq!(v["account"], json!("19900000000"));
    }

    #[test]
    fn decrypt_body_trims_whitespace_but_does_not_tolerate_the_wrong_direction() {
        let crypto = AppLoginCrypto::production();
        let plain = captured_plaintext();
        let wire = crypto.encrypt_body(&plain).unwrap();

        // 上游有时会把 JSON 字符串里的 hex 带上换行/空格
        assert_eq!(
            crypto.decrypt_body(&format!("  {wire}\n")).unwrap(),
            plain,
            "两侧空白应当被容忍"
        );

        // 但**不能**容忍"没反错位"：直接喂 ECB 密文必须失败。
        // 这条很重要 —— 错位不改变字符集，漏做反错位会解出乱码而不是报错，
        // 真正兜底的是 PKCS7 校验。
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_LOGIN_AES_KEY, &plain).unwrap();
        assert!(
            crypto.decrypt_body(&ecb).is_err(),
            "喂未反错位的 ECB 密文必须报错，而不是解出乱码"
        );
    }

    #[test]
    fn alt_key_does_not_decrypt_production_ciphertext() {
        // 用错密钥（备用那把）的表现就是上游的 `9103`。在单测层面先撞一次，
        // 免得要等一次真实请求才发现密钥抄错了。
        let prod = AppLoginCrypto::production();
        let alt = AppLoginCrypto::alt_env();
        assert_ne!(prod.key(), alt.key());

        let wire = prod.encrypt_body(&captured_plaintext()).unwrap();
        assert!(alt.decrypt_body(&wire).is_err());
    }

    #[test]
    fn production_key_is_the_environment_yes_branch() {
        // 活体探针结论：`lYzvvBMUjucw+h^R` 才是生产流量用的那把，
        // 用备用那把上游回 `9103`。这两行是那次探针的代码化表述。
        assert_eq!(AppLoginCrypto::production().key(), "lYzvvBMUjucw+h^R");
        assert_eq!(AppLoginCrypto::alt_env().key(), "+09T5lHhO+smS7x@");
        // 长度都是 16 → AES-128（不是 24/32）
        assert_eq!(APP_LOGIN_AES_KEY.len(), 16);
        assert_eq!(APP_LOGIN_AES_KEY_ALT.len(), 16);
    }

    // ── 随机 ──

    #[test]
    fn login_random_has_the_measured_shape() {
        let mut rng = SeqRandom::new(&[0, 1, 2, 3, 4, 5, 60, 61, 62, 63, 100, 200]);
        let r = new_login_random(&mut rng);
        assert_eq!(r.len(), 6, "实测 `hOjseB` 是 6 位");
        assert!(
            r.chars().all(|c| c.is_ascii_alphanumeric()),
            "字符集是大小写字母 + 数字，实际 {r}"
        );
    }

    // ── 请求构造 ──

    #[test]
    fn thirdlogin_request_is_encrypted_and_keeps_json_content_type() {
        let crypto = AppLoginCrypto::production();
        let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
        let request = thirdlogin_request(&profile(), TS, &crypto, &body).unwrap();

        assert_eq!(request.url, "https://user-njs.yun.139.com/user/thirdlogin");
        // 内容是 hex 文本，但 Content-Type 仍然是 json —— 抓包原样如此
        assert_eq!(
            header(&request, "Content-Type"),
            "application/json;charset=UTF-8"
        );

        let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
        // 关键：发出去的**不是明文**
        assert!(!sent.contains("19900000000"), "请求体里不能出现手机号明文");
        assert!(!sent.contains("dycpwd"));
        // 而是可以解回原样的密文
        assert_eq!(crypto.decrypt_body(&sent).unwrap(), body.to_string());
    }

    #[test]
    fn send_sms_code_request_is_plaintext_and_omits_the_cool_flag() {
        let body = send_sms_code_body(PHONE, "");
        let request = send_sms_code_request(&profile(), TS, &body).unwrap();

        assert_eq!(
            request.url,
            "https://user-njs.yun.139.com/user/sms/getSmsCode"
        );
        // 这一条**不加密**：体就是明文 JSON
        let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
        assert!(sent.contains("19900000000"));
        assert_eq!(
            header(&request, "Content-Type"),
            "application/json;charset=UTF-8"
        );
        // 抓包里 App 带着 `hcy-cool-flag`，但实测它会让服务端改回加密响应
        // （详见 `send_sms_code_request` 的说明）。这里钉住"我们不发它"，
        // 免得有人为了"照抄抓包"把它加回来 —— 那会让响应变成解不开的密文。
        assert!(
            !request.headers.iter().any(|(k, _)| k == "hcy-cool-flag"),
            "不能发 hcy-cool-flag：服务端会据此返回本客户端解不开的加密响应"
        );
        // 也没有 yun-encrypt-*（那是别的链路）
        assert!(
            !request
                .headers
                .iter()
                .any(|(k, _)| k.starts_with("yun-encrypt"))
        );
    }

    /// App 端的滑块取图：**与 `getSmsCode` 同一台主机**、明文、且带全套 App 头。
    ///
    /// 这条断言的意义是钉住"这不是跨端借用接口"——`/user/verfycode` 就在
    /// `base::USER_YUN` 上，和发码、登录是同一台。实测裸请求也能用，
    /// 这里仍然叠 App 头是因为**取题与提交必须同一套头**才稳妥。
    #[test]
    fn slider_puzzle_request_hits_the_same_host_as_get_sms_code() {
        let request = slider_puzzle_request(&profile(), TS, PHONE).unwrap();

        assert_eq!(request.url, "https://user-njs.yun.139.com/user/verfycode");
        let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
        assert_eq!(sent, format!("{{\"account\":\"{PHONE}\",\"type\":2}}"));
        // 与发码同一个主机，不是别的域
        assert_eq!(
            request.url.split("/user/").next(),
            send_sms_code_request(&profile(), TS, &send_sms_code_body(PHONE, ""))
                .unwrap()
                .url
                .split("/user/")
                .next()
        );
        // 设备头与发码一致（取题/提交用同一套）
        assert_eq!(
            header(&request, "service_identifier_key"),
            "service_identifier_userDomain"
        );
        assert!(!header(&request, "x-DeviceInfo").is_empty());
    }

    /// 带滑块的发码体：`puzzleVerfycode` 必须是**加密后**的坐标，且能解回原样。
    #[test]
    fn send_sms_code_body_with_slider_encodes_the_answer() {
        let body = send_sms_code_body_with_slider(PHONE, "370").unwrap();

        // 其余字段与空滑块版本完全一致（同一个函数构造）
        let plain = send_sms_code_body(PHONE, "");
        for key in ["mode", "phoneNumber", "reqType", "clientType", "random"] {
            assert_eq!(body[key], plain[key], "字段 {key} 不该变");
        }

        let encoded = body["puzzleVerfycode"].as_str().unwrap();
        assert_ne!(encoded, "370", "不能把坐标明文发上去");
        // 解回来必须是 370 —— 用的是客户端公开常量那套（与 PC 端同一个）
        let decoded =
            mcloud_core::aes_dynamic_iv_decrypt(crate::login::PUZZLE_AES_KEY, encoded).unwrap();
        assert_eq!(decoded.trim(), "370");
    }

    #[test]
    fn app_requests_carry_the_full_device_header_set() {
        let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
        let request =
            thirdlogin_request(&profile(), TS, &AppLoginCrypto::production(), &body).unwrap();

        // 少了这些头上游回 `9999 登录失败！`（PC 端同款实测）
        assert_eq!(
            header(&request, "service_identifier_key"),
            "service_identifier_userDomain"
        );
        assert_eq!(header(&request, "x-yun-app-channel"), "10000020");
        assert_eq!(header(&request, "x-DeviceName"), "aVBhZA==");
        assert!(
            header(&request, "x-yun-client-info")
                .starts_with("1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|")
        );
        assert!(header(&request, "x-DeviceInfo").ends_with("|0.0000|0.0000|"));
        assert_eq!(
            header(&request, "x-yun-user-agent"),
            "iPad|iPad|ios 26.6|mCloud12.6.0-200"
        );
        assert_eq!(
            header(&request, "User-Agent"),
            "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)"
        );
    }

    // ── 滑块 H5 ──

    #[test]
    fn captcha_h5_url_decodes_to_the_captured_payload() {
        let url = captcha_h5_url(&profile(), PHONE);
        assert!(url.starts_with("https://yun.139.com/fe/captcha/?data="));

        let b64 = url.split("?data=").nth(1).unwrap();
        // `mcloud-core` 只导出了编码器，测试里自带一个小解码器
        let raw = decode_b64(b64);
        let v: Value = serde_json::from_slice(&raw).unwrap();

        assert_eq!(v["captchaType"], json!(2));
        assert_eq!(v["params"]["clientType"], json!("412"));
        assert_eq!(v["params"]["reqType"], json!("3"));
        assert_eq!(v["params"]["nationCode"], json!("+86"));
        assert_eq!(v["params"]["phoneNumber"], json!("19900000000"));

        // headers 里**只有两个头**，不是请求头全集
        let headers = v["headers"].as_object().unwrap();
        assert_eq!(headers.len(), 2, "H5 的 data 里只带两个头");
        assert_eq!(v["headers"]["x-yun-app-channel"], json!("10000020"));
        // 这个 client-info **没有**末尾的 `|0|;`（与请求头不同）
        assert_eq!(
            v["headers"]["x-yun-client-info"],
            json!(
                "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|aVBhZA==|"
            )
        );
    }

    /// 测试用的标准 base64 解码（`mcloud-core` 只导出了编码器）。
    fn decode_b64(s: &str) -> Vec<u8> {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut acc: u32 = 0;
        let mut bits = 0;
        for b in s.bytes().filter(|b| *b != b'=') {
            let v = TABLE
                .iter()
                .position(|c| *c == b)
                .expect("非法 base64 字符") as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    // ── 响应语义 ──

    #[test]
    fn classify_send_sms_maps_the_captcha_gate() {
        // 抓包里那条真实响应
        let env = Envelope::new(json!({
            "success": false,
            "code": "200059554",
            "message": "请先获取滑块拼图验证码",
            "data": { "captchaType": 2 }
        }));
        assert_eq!(
            classify_send_sms(&env),
            AppSendSmsOutcome::NeedCaptcha {
                captcha_type: Some(2)
            }
        );

        // `200059553` 的语义是"**这一个挑战作废了**"（补测时亲手撞到），
        // 必须与 `200059554`（首次要滑块）分开：前者要**重取图**，
        // 后者是**首次取图**。归成同一个分支会让调用方拿旧答案重试，
        // 既必然再失败一次，又白记一次失败次数。
        let voided = Envelope::new(json!({
            "success": false,
            "code": "200059553",
            "message": "同一滑动拼图验证码在单位时间内校验失败次数超限",
            "data": { "captchaType": 1 }
        }));
        assert_eq!(
            classify_send_sms(&voided),
            AppSendSmsOutcome::SliderExhausted {
                captcha_type: Some(1)
            }
        );
    }

    #[test]
    fn classify_send_sms_reads_quota_on_success() {
        let env = Envelope::new(json!({
            "success": true, "code": "0000", "message": "请求成功",
            "data": { "remainGetTimes": 4 }
        }));
        assert_eq!(
            classify_send_sms(&env),
            AppSendSmsOutcome::Sent {
                remain_get_times: Some(4)
            }
        );
    }

    #[test]
    fn classify_login_separates_the_two_probe_codes() {
        // 这两个码是整个加密实现能不能收工的判据，必须分开报
        let wrong_code = Envelope::new(json!({
            "success": false, "code": "9441", "message": "验证码不正确", "data": null
        }));
        assert_eq!(classify_login(&wrong_code), AppLoginOutcome::SmsCodeWrong);

        let undecryptable = Envelope::new(json!({
            "success": false, "code": "9103", "message": "输入参数不合法", "data": null
        }));
        assert_eq!(
            classify_login(&undecryptable),
            AppLoginOutcome::Undecryptable
        );

        let login_failed = Envelope::new(json!({
            "success": false, "code": "9999", "message": "登录失败！", "data": null
        }));
        assert_eq!(classify_login(&login_failed), AppLoginOutcome::LoginFailed);

        // `9441` 与 `9103` 不能折叠成同一个变体 —— 折了就没法用它们定位问题
        assert_ne!(classify_login(&wrong_code), classify_login(&undecryptable));
    }

    #[test]
    fn classify_login_returns_the_ciphertext_on_success() {
        let env = Envelope::new(json!({
            "success": true, "code": "0000", "message": "请求成功",
            "data": "deadbeef"
        }));
        assert_eq!(
            classify_login(&env),
            AppLoginOutcome::Success {
                encrypted: "deadbeef".to_string()
            }
        );
    }

    #[test]
    fn classify_login_reports_bad_shape_instead_of_faking_credentials() {
        // 成功码但 data 不是字符串：如实报错，不能伪造一个空凭据
        let env = Envelope::new(json!({"success": true, "code": "0000", "data": null}));
        match classify_login(&env) {
            AppLoginOutcome::Other { message, .. } => assert!(message.contains("不是密文字符串")),
            other => panic!("应落到 Other，实际 {other:?}"),
        }
    }
}
