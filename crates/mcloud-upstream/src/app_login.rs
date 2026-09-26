//! App 端（手机客户端）验证码登录。
//!
//! # 为什么单独一个模块，而不是给 `login` 加个平台开关
//!
//! App 与 PC 走的是**同一个接口** `POST /user/thirdlogin`，但
//! **没有一处是可以复用的**：
//!
//! | | PC 端 | App 端 |
//! |---|---|---|
//! | `pintype` | `"21"` / `"5"` / `"9"` | **`"8"`**（短信也是 8，不是 5）|
//! | `clienttype` | `"656"` / `"710"` | **`"412"`** |
//! | `cpid` | `"277"` / `"332"` | **`"55"`** |
//! | 请求体 | 明文 JSON | **AES-128-ECB + 末 10 位错位的 hex** |
//! | 响应 `data` | AES-ECB hex | AES-ECB hex **+ 错位** |
//! | AES 密钥 | `qPqDw263XgFgL3u8` | **`lYzvvBMUjucw+h^R`** |
//! | `extInfo.ifOpenAccount` | 数字 `1` | **字符串 `"1"`** |
//! | `getSmsCode` 的 `mode`/`reqType` | 数字 | **字符串** |
//! | `Authorization` 首段 | `pc` | **`mobile`** |
//!
//! 所以本模块**刻意不参数化平台**：把两端塞进一个泛型构造器，只会让
//! "传错端"变成一个能静默通过的选项 —— 而这正好是最容易犯的错
//! （把 PC 的密钥搬到 App 路径上，上游回 `9103`）。
//!
//! # 三层封装，第三层是 App 独有的
//!
//! ```text
//! 第 1 层  明文 JSON
//! 第 2 层  AES-128-ECB/PKCS7  →  384 个 hex 字符
//! 第 3 层  末 10 个字符移到最前（错位）   ← App 独有，PC 没有
//! ```
//!
//! 第 3 层**不是加密**，只是一次字符串旋转；但少做这一步服务端会回
//! `9103 输入参数不合法`，因为它要先反旋转再解密。
//!
//! # 实拍证据（本模块写完后实测的三个探针）
//!
//! `9103` / `9441` 构成一个**二值判别器**，调加密参数时不需要真的登录成功：
//!
//! | 送出的体 | 期望响应 | 实测 |
//! |---|---|---|
//! | 加密体 + 生产密钥 + 末 10 位错位 | `9441 验证码不正确` | ✅ `9441` |
//! | 同一个密文但**不旋转** | `9103 输入参数不合法` | ✅ `9103` |
//! | 明文 JSON（没加密） | `9103 输入参数不合法` | ✅ `9103` |
//!
//! 第一行回 `9441` 就说明：密钥、模式、填充、错位方向、hex 大小写**全部正确**
//! —— 因为服务端是"先反旋转、再解密、再解析"才可能走到校验验证码那一步。
//!
//! # 验证码链路：`puzzleVerfycode` 是纯协议入口
//!
//! 真机客户端在 `200059554` 之后会**开 WebView**（短信由那个 H5 自己发），
//! 但那是**客户端行为**，不是服务端的限制：实测把 `puzzleVerfycode`
//! 填成非空，服务端就不再回 `200059554`。
//!
//! 而第一方滑块取图接口就在**同一台主机**上：`POST /user/verfycode`
//! `{"account":…,"type":2}`，裸请求即可。编码与 PC 端完全一样
//! （见 [`crate::login::puzzle_verfycode_encrypt`]），所以这不是新协议，
//! 只是 `clientType` 与请求头换成 App 的。

use serde_json::{json, Value};

use mcloud_core::{Error, HttpRequest, Result};
use mcloud_crypto::Random;

use crate::endpoints::{SEND_SMS_CODE, SLIDER_PUZZLE, THIRD_LOGIN};
use crate::login::{codes, LoginEnvelope, LoginPayloadDecryptor};

// ══════════════════════════════ 常量 ══════════════════════════════

/// App 端登录体的 AES 密钥（`isCurrentEnvironment = YES` 分支）。
///
/// 反汇编实证：`+[MCLoginCryptoUtil AESEncrypt:isCurrentEnvironment:]` 里
/// 它是 `csel` 选出来的**字面量**，不是从配置里取的 —— 所以客户端改个变量名
/// 不影响我们，改的是这个常量本身。
pub const APP_LOGIN_AES_KEY: &str = "lYzvvBMUjucw+h^R";

/// 备用密钥（`isCurrentEnvironment = NO` 分支）。
///
/// **它不是"测试环境密钥"**：客户端只是按一个布尔位二选一，
/// 实测生产流量走的是 [`APP_LOGIN_AES_KEY`]。留它是因为
/// "用错这把会得到 `9103`"是排查密钥问题最快的判据。
pub const APP_LOGIN_AES_KEY_ALT: &str = "+09T5lHhO+smS7x@";

/// `getSmsCode` 请求体里的 `random`：**客户端硬编码的常量**。
///
/// 这不是"一个随机值的示例"，而是实测确认的固定字面量：跨一个月的
/// 全部 4 次 `getSmsCode` 请求，它都是同一个 `qwert`。
/// 一个真正随机的值不可能每次相同。
///
/// 所以它**不做成参数** —— 参数会暗示"这里可以也应该变化"。
pub const SEND_SMS_RANDOM: &str = "qwert";

/// App 端 `clienttype` / `clientType`（两者同值，**大小写不同**）。
///
/// 复用 [`crate::auth_refresh::APP_CLIENT_TYPE`] 而不是再写一个 `"412"`：
/// 同一个事实有两个常量就会漂移，而漂移的表现是"刷新能用、登录不能用"
/// （或反过来），排查时很难联想到是两处常量不一致。
pub use crate::auth_refresh::APP_CLIENT_TYPE;

/// App 端 `cpid`。
pub const APP_CPID: &str = "55";

/// App 端上报的版本号（也是设备串第 4 段）。
pub const APP_VERSION: &str = "12.6.0";

/// App 端渠道号。
pub const APP_CHANNEL: &str = "10000020";

/// App 端 `service_identifier_key` 请求头的值。
pub const APP_SERVICE_IDENTIFIER: &str = "service_identifier_userDomain";

/// App 端 `User-Agent`（括号里是 `iOS` 大写，与设备串的 `ipados` 大小写不同）。
pub const APP_USER_AGENT: &str = "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)";

/// App 端 `x-UserAgent`（低版本号形态）。
pub const APP_X_USER_AGENT: &str = "412|iPad|ios 26.6|12.6.0-200";

/// App 端 `x-yun-user-agent`。
pub const APP_X_YUN_USER_AGENT: &str = "iPad|iPad|ios 26.6|mCloud12.6.0-200";

/// `x-DeviceName` 的 base64（`aVBhZA==` = `iPad`）。
pub const APP_DEVICE_NAME_B64: &str = "aVBhZA==";

/// App 端设备串里的局域网地址（实测值，非真实用户网络）。
pub const APP_DEFAULT_IP: &str = "192.168.3.179";

/// App 端设备型号。
pub const APP_DEVICE_MODEL: &str = "iPad8,6";

// ══════════════════════════════ 加解密 ══════════════════════════════

/// App 端登录体的加解密器：AES-128-ECB/PKCS7 + 末 10 位错位。
///
/// 与 PC 端的 [`crate::login::LoginDataDecryptor`] **不通用** —— 它多了
/// 错位那一层，且密钥不同。刻意做成两个类型，就是为了让"用错端的解密器"
/// 在**编译期**就不可能发生。
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

    /// 当前密钥。
    pub fn key(&self) -> &str {
        &self.key
    }

    /// 明文 → 线上请求体（hex 字符串）。
    pub fn encrypt_body(&self, plaintext: &str) -> Result<String> {
        let ecb = mcloud_crypto::aes_ecb_pkcs7_encrypt_hex(&self.key, plaintext)
            .map_err(|e| Error::Other(format!("App 登录体加密失败：{e}")))?;
        Ok(mcloud_crypto::shift_tail10_to_front(&ecb))
    }

    /// 线上响应 `data` → 明文。
    ///
    /// 两步都**必须**成功：错位不改变字符集，所以"只反错位不解密"也能得到一个
    /// 看起来正常的 hex 串 —— 那种情况下会解出乱码而不是报错，
    /// 所以这里靠 AES 的 PKCS7 校验来兜底（解错必然填充非法）。
    pub fn decrypt_body(&self, wire_hex: &str) -> Result<String> {
        let ecb = mcloud_crypto::shift_head10_to_back(wire_hex.trim());
        mcloud_crypto::aes_ecb_pkcs7_decrypt_hex(&self.key, &ecb)
            .map_err(|e| Error::Parse(format!("App 登录体解密失败（密钥、错位或填充不对）：{e}")))
    }
}

impl LoginPayloadDecryptor for AppLoginCrypto {
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value> {
        let plain = self.decrypt_body(ciphertext)?;
        serde_json::from_str(&plain).map_err(|e| {
            Error::Parse(format!(
                "App 端登录载荷不是合法 JSON（密钥不对或漏了错位都会落到这里）：{e}"
            ))
        })
    }
}

// ══════════════════════════ 请求体（纯函数） ══════════════════════════

/// `/user/sms/getSmsCode` 的请求体。
///
/// 三个**易错点**，全都在抓包里能对上：
///
/// 1. **所有值都是字符串**（PC 端这里 `mode` / `reqType` 是数字）
///    —— `"mode":"0"` 不是 `"mode":0`；
/// 2. 字段名是 `puzzleVerfycode` —— 上游自己就把 `verify` 拼成了 `verfy`；
/// 3. 首次请求它是**空串**，不是 `null`、也不是省略字段。
///
/// `random` 固定为 [`SEND_SMS_RANDOM`]，**不是** `thirdlogin` 那个 6 字符随机值。
pub fn send_sms_code_body(phone: &str, puzzle_verfycode: &str) -> Value {
    json!({
        "mode": "0",
        "phoneNumber": phone,
        "reqType": "3",
        "clientType": APP_CLIENT_TYPE,
        "random": SEND_SMS_RANDOM,
        "puzzleVerfycode": puzzle_verfycode,
    })
}

/// 带滑块的发码请求体：在基础 body 上填**已编码**的滑块答案。
///
/// 直接复用 PC 的 [`crate::login::puzzle_verfycode_encrypt`]，因为实测证明
/// 这就是**同一个第一方滑块**：同一个 `/user/verfycode`、同一台主机，
/// 编码出来也是同样的 `base64(固定IV ‖ AES-128-CBC/PKCS7)`。
///
/// 单独成函数而不是给上个函数加参数：走滑块是**异常路径**，
/// 让它在调用点显式可见比埋一个 `Option` 更难忽略。
pub fn send_sms_code_body_with_slider(phone: &str, slider_answer: &str) -> Result<Value> {
    let encoded = crate::login::puzzle_verfycode_encrypt(slider_answer)?;
    Ok(send_sms_code_body(phone, &encoded))
}

/// `/user/thirdlogin` 的请求体（**明文**，加密交给 [`AppLoginCrypto::encrypt_body`]）。
///
/// | 字段 | 值 | 备注 |
/// |---|---|---|
/// | `cpid` | `"55"` | App 端固定 |
/// | `clienttype` | `"412"` | **全小写 `t`**，与 `getSmsCode` 的 `clientType` 不同 |
/// | `pintype` | `"8"` | **短信登录也是 8**，不是 PC 的 5 |
/// | `dycpwd` | 6 位验证码**明文** | 不做哈希（账密登录才哈希）|
/// | `loginMode` / `verType` | `"0"` / `"1"` | App 端独有 |
/// | `extInfo.ifOpenAccount` | `"1"` | **字符串**，PC 端这里是数字 |
/// | `version` | `"12.6.0"` | 与设备串第 4 段一致 |
/// | `random` | 6 字符 | 与 `getSmsCode` 的 `random` **不是同一个值** |
pub fn sms_thirdlogin_body(phone: &str, sms_code: &str, random: &str) -> Value {
    json!({
        "cpid": APP_CPID,
        "extInfo": { "ifOpenAccount": "1" },
        "dycpwd": sms_code,
        "pintype": "8",
        "loginMode": "0",
        "verType": "1",
        "version": APP_VERSION,
        "msisdn": phone,
        "random": random,
        "clienttype": APP_CLIENT_TYPE,
    })
}

/// 生成登录体里的 `random`：6 位大小写字母 + 数字。
///
/// 形状取自实测（`hOjseB`）。它**不参与任何可观测的校验** —— 依据是
/// `getSmsCode` 那个同名字段是硬编码常量，服务端照样受理。
///
/// 要诚实记一笔：`thirdlogin` 的 `random` **只有 1 个样本**，
/// "每次不同"是从"客户端为什么要随机"推的，不是实测到的。
pub fn new_login_random(rng: &mut impl Random) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    rng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

// ══════════════════════════ 设备串与请求头 ══════════════════════════

/// App 端设备串（14 段，末尾一段是空的）。
///
/// `x-DeviceInfo` 与 `x-yun-client-info` 在 **App 端不相等**（PC 端相等），
/// 差别是后者多了 `x-DeviceName` 与 `0|;` 两段。
pub fn app_device_info(device_id: &str) -> String {
    format!(
        "1|{ip}|2|{ver}|APPLE|{model}|{device_id}|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|",
        ip = APP_DEFAULT_IP,
        ver = APP_VERSION,
        model = APP_DEVICE_MODEL,
    )
}

/// App 端 `x-yun-client-info`：设备串 + `x-DeviceName` 的 base64 + `0|;`。
pub fn app_client_info(device_id: &str) -> String {
    format!("{}{}|0|;", app_device_info(device_id), APP_DEVICE_NAME_B64)
}

/// 登录链路要带的**全部** App 设备头。
///
/// # 为什么是一整套而不是最小集合
///
/// 实测：缺设备头时上游回 `9999 登录失败！` —— 也就是**"头不对"的失败
/// 长得像"登录被拒"**，很容易误判成账号问题。所以这里照抓包给全套。
///
/// # ⚠ 刻意**不发** `hcy-cool-flag`
///
/// 抓包里 App 是带这个头的，但实测：**只要带上它**（任意值），服务端就会
/// 把响应体换成一段 144 字节的加密串，而我们**没有那把密钥**
/// （试过错位前后、也试过 SM4-ECB 那条路，都解不开）。
///
/// 不发它时语义与抓包完全一致（同样回 `200059554`）。这是
/// "照抄抓包"与"能跑起来"冲突时唯一说得通的选择。
pub fn app_headers(device_id: &str, timestamp: i64) -> Vec<(&'static str, String)> {
    vec![
        ("Pragma", "no-cache".to_string()),
        ("x-yun-user-timestamp", timestamp.to_string()),
        ("x-yun-app-channel", APP_CHANNEL.to_string()),
        ("x-yun-net-type", "1".to_string()),
        ("x-yun-svc-type", "1".to_string()),
        ("x-huawei-channelSrc", APP_CHANNEL.to_string()),
        ("x-yun-api-version", "v1".to_string()),
        ("x-UserAgent", APP_X_USER_AGENT.to_string()),
        ("x-yun-client-info", app_client_info(device_id)),
        ("Accept-Language", "zh-CN".to_string()),
        ("service_identifier_key", APP_SERVICE_IDENTIFIER.to_string()),
        ("User-Agent", APP_USER_AGENT.to_string()),
        ("x-DeviceName", APP_DEVICE_NAME_B64.to_string()),
        ("x-DeviceInfo", app_device_info(device_id)),
        ("x-yun-user-agent", APP_X_YUN_USER_AGENT.to_string()),
        ("x-SvcType", "1".to_string()),
    ]
}

/// 给请求叠上 App 头，并把 `Content-Type` 归一成抓包那个形态。
///
/// # 为什么先删再加
///
/// [`HttpRequest::json_body`] 在**没有** `content-type` 时会自己加一个
/// `application/json`。若这里直接 `push`，就会有两个 `Content-Type`；
/// reqwest 会把它们拼成 `application/json, application/json;charset=UTF-8`，
/// 而上游读**第一个**（也就是错的那个）。
///
/// 所以先不分大小写地删掉所有 `content-type`，再补上唯一一个。
fn with_app_headers(mut request: HttpRequest, device_id: &str, timestamp: i64) -> HttpRequest {
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
    for (name, value) in app_headers(device_id, timestamp) {
        request = request.header(name, value);
    }
    request.header("Content-Type", "application/json;charset=UTF-8")
}

// ══════════════════════════ 请求构造 ══════════════════════════

/// `POST /user/sms/getSmsCode`（**明文体**）。
///
/// 注意：**取题**（[`slider_puzzle_request`]）与**提交答案**（本函数）
/// 必须用**同一套头**才稳妥，所以本函数也照常叠全套 App 头。
pub fn send_sms_code_request(device_id: &str, timestamp: i64, body: &Value) -> HttpRequest {
    with_app_headers(
        HttpRequest::post(SEND_SMS_CODE).json_body(body),
        device_id,
        timestamp,
    )
}

/// `POST /user/verfycode`（**明文体**）：取第一方滑块拼图。
///
/// # 为什么 App 端也用它，而不是去开那个 H5
///
/// 实测：这个接口就在 App 自己的主机上，而且**裸请求就能用** ——
/// 不需要 `Authorization`、不需要设备头、不需要 `x-yun-tid`。
///
/// 路径与字段名都没有 `i`：是 `verfycode` 不是 `verifycode`。
///
/// ⚠ 请求体里 `type` 是**数字** `2` —— 这是整条 App 链路里唯一的数字字段。
pub fn slider_puzzle_request(device_id: &str, timestamp: i64, phone: &str) -> HttpRequest {
    with_app_headers(
        HttpRequest::post(SLIDER_PUZZLE).json_body(&crate::login::slider_puzzle_body(phone)),
        device_id,
        timestamp,
    )
}

/// `POST /user/thirdlogin`（**加密体**）。
///
/// 传进来的是明文 [`Value`]，加密在这里完成 —— 这样"忘了加密"在类型层面
/// 就不可能发生。
///
/// ⚠ `Content-Type` 仍然是 `application/json;charset=UTF-8`，尽管内容是
/// hex 文本 —— 抓包原样如此，别"顺手改成 text/plain"。
pub fn thirdlogin_request(
    device_id: &str,
    timestamp: i64,
    crypto: &AppLoginCrypto,
    body: &Value,
) -> Result<HttpRequest> {
    let plaintext = body.to_string();
    let wire = crypto.encrypt_body(&plaintext)?;

    let mut request = with_app_headers(HttpRequest::post(THIRD_LOGIN), device_id, timestamp);
    request.body = Some(wire);
    Ok(request)
}

// ══════════════════════════ 响应语义 ══════════════════════════

/// 一次"发送验证码"的结果。
///
/// **刻意不复用 PC 端的 [`crate::login::SendSmsOutcome`]**：两端的
/// `NeedSlider` 名字一样，但**下一步动作完全不同** ——
///
/// - PC 端：取图 → 本地识别 → 编码成 `puzzleVerfycode` 重发；
/// - App 端：真机是**开 WebView**，短信由那个 H5 发。
///
/// 共用一个枚举会让调用方写错分支却仍然编译通过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSendSmsOutcome {
    /// 已发出。`remain_get_times` 是配额余量。
    Sent {
        /// 今日还可发送的次数。
        remain_get_times: Option<u32>,
    },
    /// 需要先过滑块拼图。
    ///
    /// **这是首次发码的常态，不是错误**：接着去 [`slider_puzzle_request`]
    /// 取图、解出坐标、带答案重发。
    NeedCaptcha {
        /// 上游给的验证码类型。
        captcha_type: Option<i64>,
    },
    /// `200059553 同一滑动拼图验证码在单位时间内校验失败次数超限`
    /// —— **这个挑战作废了，必须重新取图**。
    ///
    /// 它**不是**"你的账号被限流了"，而是"这一道题的尝试次数用完了"。
    /// 所以正确的反应是重取一道**新题**，而不是拿旧答案再试一次。
    SliderExhausted {
        /// 上游给的验证码类型。
        captcha_type: Option<i64>,
    },
    /// 其它码（含缺码）。
    Other {
        /// 归一后的业务码。
        code: Option<i64>,
        /// 上游消息原文。
        message: String,
    },
}

/// 归类 `getSmsCode` 的响应。
pub fn classify_send_sms(envelope: &LoginEnvelope) -> AppSendSmsOutcome {
    let captcha_type = || {
        envelope
            .data
            .as_ref()
            .and_then(|p| p.get("captchaType"))
            .and_then(Value::as_i64)
    };

    match envelope.code {
        Some(codes::OK_ZERO) | Some(codes::OK_200) => {
            let remain = envelope
                .data
                .as_ref()
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
        // ⚠ 这一支**必须**与上一个分开：合并会让调用方拿着**已经作废的一道题**
        // 反复重试，而这个接口是**计失败次数**的。
        Some(codes::SLIDER_EXHAUSTED) => AppSendSmsOutcome::SliderExhausted {
            captcha_type: captcha_type(),
        },
        other => AppSendSmsOutcome::Other {
            code: other,
            message: envelope.message.clone(),
        },
    }
}

/// 一次 `thirdlogin` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppLoginOutcome {
    /// 登录成功。`encrypted` 是 `data` 原文（AES 密文），交给 [`AppLoginCrypto`]。
    Success {
        /// `data` 字段的密文原文。
        encrypted: String,
    },
    /// `9441` 验证码不正确 —— **参数已解开**，只是码不对或过期。
    SmsCodeWrong,
    /// `9103 输入参数不合法` —— **服务端解不开请求体**。
    ///
    /// 三个已知原因：①体没加密 ②用了备用密钥 ③错位方向写反。
    Undecryptable,
    /// `9999 登录失败！` —— 多半是设备头缺失或不完整。
    LoginFailed,
    /// 其它码（含缺码）。
    Other {
        /// 归一后的业务码。
        code: Option<i64>,
        /// 上游消息原文。
        message: String,
    },
}

/// 归类 `thirdlogin` 的响应。
pub fn classify_login(envelope: &LoginEnvelope) -> AppLoginOutcome {
    match envelope.code {
        Some(codes::OK_ZERO) | Some(codes::OK_200) => match envelope.data.as_ref() {
            Some(Value::String(cipher)) => AppLoginOutcome::Success {
                encrypted: cipher.clone(),
            },
            // 成功码但载荷不是密文字符串：如实报告形状不对，**不伪造空凭据**。
            _ => AppLoginOutcome::Other {
                code: envelope.code,
                message: "登录成功但 data 不是密文字符串".to_string(),
            },
        },
        Some(codes::SMS_CODE_WRONG) => AppLoginOutcome::SmsCodeWrong,
        Some(codes::UNDECRYPTABLE_BODY) => AppLoginOutcome::Undecryptable,
        Some(codes::LOGIN_FAILED) => AppLoginOutcome::LoginFailed,
        other => AppLoginOutcome::Other {
            code: other,
            message: envelope.message.clone(),
        },
    }
}

#[cfg(test)]
mod tests;
