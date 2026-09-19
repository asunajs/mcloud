//! 移动云盘 **App 端签到（云朵中心）** —— H5 链路的上游接口层。
//!
//! # 规格来源
//!
//! `docs/capture/app-signin-samples.txt`（2026-09-19 实测抓包 + 账号 B 纯协议端到端复现）。
//! 黄金向量：`test/golden/app-signin-vectors.json`。
//!
//! # 为什么这一层看起来"不像一个 App 接口"
//!
//! 因为**签到根本不是一个原生接口，是一个 H5 页面**（`newsignin`）。
//! 原生 App 只做两件事：换一个 SSO token、把 WebView 打开到 `m.mcloud.139.com`。
//! 真正的签到动作全由 H5 发出 —— 所以本模块要同时说两种"客户端语言"：
//!
//! ```text
//! 原生（A 族头，Authorization + 设备头）        H5（自有头，jwttoken）
//! ───────────────────────────────────────────   ─────────────────────────────────────
//! ① POST /user/querySpecTokenV2  ──→ SSO token
//! ② GET  /ycloud/api/timestamp   ──→ 服务端毫秒时间戳（签名要用）
//! ③ POST /ycloud/auth-service/auth/tyrzLogin ──→ jwtToken（≈905 字符，2 小时）
//! ④ GET  /ycloud/signin/page/infoV3?client=app        ← 签到状态，**权威判据**
//! ⑤ GET  /ycloud/signin/page/startSignIn?client=app   ← 执行签到
//! ⑥ POST /ycloud/signin/page/receiveV3                ← 领云朵（**必须带 deviceId**）
//! ⑦ POST /ycloud/signin/page/receiveRevivalReward     ← 用复活卡/回馈卡（**不带 deviceId**）
//! ```
//!
//! ①②③ 是"建会话"，④⑤⑥⑦ 是"签到 + 领取"。**没有 ③ 就没有 ④⑤⑥⑦** ——
//! 少了 `jwttoken` 头，它们一律回 `90001 未登录`。
//!
//! # 三条容易踩的坑（都实测过）
//!
//! 1. **签名不是 B 族那套。** 这里既不是 `signature`（SHA256 + `clientKey`）
//!    也不是 `encodeData`，而是 `md5(盐 + request-id + 时间戳 + nonce + 空串 + 盐)`，
//!    盐是 `sekaMdYYLIZfbCfm` —— 与 PC 端签名**同一个盐**，所以直接复用
//!    [`mcloud_core::generate_signature`]（`salt_key = 2`）。见 [`sign_h5_request`]。
//! 2. **`x-timestamp` 必须取自服务端**（`/ycloud/api/timestamp`），不能用本地时间；
//!    签名里用的是同一个值，两者对不上就是 `401 请求签名校验失败`。
//! 3. **`todaySignIn` 不是成败判据。** 实测 `startSignIn` 的响应是**调用前**的快照，
//!    `todaySignIn:false` 恰恰表示"这次把今天签上了"。判成败一律回查 [`parse_sign_in_status`]。
//!
//! # `deviceId`：**逐接口**判断，绝不按"签到链路"整体外推
//!
//! 签到页里有个 `deviceId`，值是 `"B"` + 88 字符 base64。它**不是"这一族接口都要"**：
//! H5 是**逐接口**用 `isDeviceId: true` 声明的，实测结果与声明一致。
//!
//! ## 它的来源：**就是 [`crate::smid`]，不需要浏览器**
//!
//! 页面里的 `window.SMSdk` 是 `fp.min.js`（数美设备指纹）**自己定义**的
//! （所以 App 二进制里搜不到这个名字），`getDeviceId()` 返回
//! `'B' + localStorage['SMID']`；而 `SMID` 正是它往
//! `slw.h5cmpassport.com:9090/deviceprofile/v4` 上报特征后拿回的 `detail.deviceId`。
//!
//! 那套配置（organization / appId / RSA 公钥 / host）写在签到页 HTML 的
//! `window._smConf` 里，与本项目 [`crate::smid`] 的常量**逐字相同** ——
//! 两边是同一个客户端，所以纯 Rust 上报就能拿到同一个 id。
//! 守卫：`smid::tests::the_signin_page_fingerprint_sdk_is_this_same_client`。
//!
//! ⇒ 曾经那条"先用官方 App 打开一次签到页、再从 WebView 的 `localStorage` 里读"
//! 的路子**已废弃**；`scripts/h5-device-id.py` 只留作取证工具。
//!
//! ## 各接口的签名
//!
//! - [`start_sign_in_request`] 收 `Option<&str>` —— 允许省略（实测不带也能签上），
//!   但调用点必须写出来；
//! - [`do_task_post_request`] 与 [`receive_v3_request`] 收 `&str` —— **必填**，
//!   让"漏发"变成编译期错误，而不是运行时的上游报错（`receiveV3` 不带实测回 `615`）；
//! - [`receive_revival_reward_request`] 收 `Option<&str>` —— H5 定义里**没有**
//!   `isDeviceId`，所以缺省就是不发；给了才补一个 `deviceid` **头**（老抓包如此）。
//!   两个分支都有证据，取哪个都行，见该函数的说明。
//!
//! ⚠ 同一个值在三个接口上落点**各不相同**：`startSignIn` 在 **query**、
//! `receiveV3` 在 **body**、`receiveRevivalReward` 在 **头**。别照着前一个抄。

use mcloud_core::{Error, HttpRequest, Random, generate_signature};
use serde_json::{Value, json};

use crate::Envelope;
use crate::app_client::AppProfile;
use crate::base;

// ─────────────────────────────────────────────────────────────────────────────
// 常量（全部来自抓包，不是猜的）
// ─────────────────────────────────────────────────────────────────────────────

/// 签到活动的市场名。它同时是请求头 `activityid` 的取值。
pub const MARKET_NAME: &str = "sign_in_3";

/// 签到页的来源 id（URL 的 `sourceid` / `tyrzLogin` 体的 `sourceId`）。
pub const SOURCE_ID: &str = "1002";

/// H5 页面的 `appID`（页面 URL 上那个固定串，不是客户端 id）。
pub const H5_APP_ID: &str = "cy62d5a5e8373a272aeca467accad61722c51cfd48";

/// H5 请求头 `appversion`。
///
/// 注意它与 [`crate::app_client::ios::APP_VERSION`]（`12.6.0`）**不是一个值**：
/// 这里带第 4 段（`12.6.0.0`）。
pub const H5_APP_VERSION: &str = "12.6.0.0";

/// `taskListV3` 请求体里的 `clientVersion`（不带第 4 段）。
pub const TASK_CLIENT_VERSION: &str = "12.6.0";

/// `startSignIn` / `infoV3` 的查询参数 `client`。
///
/// 抓包实测是字面量 `app`（**不是数字**）—— 别照 `taskListV3` 的 `client:1` 抄。
pub const CLIENT: &str = "app";

/// `querySpecTokenV2` 请求体里 `toSourceId` 的取值（**本功能选的**，不是"App 端的"）。
///
/// ⚠ **它不是端常量，是调用方按场景选的参数。** 同一个 App 进程里就出现过三个值
/// （`001003` / `001005` / `010001`），按发起方不同而变。本模块取 `001005`，
/// 依据是**签到页 URL 自带的 `targetSourceId=001005`**（App 实拍，见
/// `docs/capture/app-signin-samples.txt` §2.1）。
///
/// 别拿别处的值来换它：
/// - `apiServer.js` 里 `getCloudTicket` 的默认值是 `001003` —— 那是 **PC 客户端**的包；
/// - 签到会话里那条 **WebKit** 发起的 `querySpecTokenV2` 也确实用了 `001003`，
///   但发起方不是签到页本身（同一条接口，原生与 H5 两个发起方的头、参数都不同）。
///
/// "两边实测都能过"只说明**服务端在这条路径上不校验**，不等于可以互换 ——
/// 上游不校验的字段一旦哪天开始校验，错的那个是**静默失效**，不会报错。
/// 跨端（PC ↔ App）、跨发起方（原生 ↔ H5）的常量一律不互搬。
///
/// 反过来说，**同一个文件里出现多个值也不矛盾**：PC 客户端 `apiServer.js` 里
/// 同一条接口三处调用分别传 `001003`（默认）/ `001005` / `001248`。
/// 完整的取值谱、分类判据与"哪些码会整套换头"见 `docs/device-and-source-codes.md`。
pub const TO_SOURCE_ID: &str = "001005";

/// H5 签名用的盐值编号 —— [`mcloud_core::sign_salt`] 里的 `sekaMdYYLIZfbCfm`。
///
/// 单列成常量而不是在调用点写 `2`：`SIGN_SALT_MAP` 哪天被改动，
/// 这里会有一条测试直接失败（见 `h5_salt_is_the_pc_signing_salt`），
/// 而不是变成"签到忽然回 401"。
pub const H5_SIGN_SALT_KEY: u8 = 2;

/// 业务成功码。
pub const CODE_OK: i64 = 0;

/// `90001 未登录` —— `jwttoken` 没带上，或会话没建起来。
pub const CODE_NOT_LOGGED_IN: i64 = 90001;

/// `200050409 token 已失效` —— SSO token 过期。
///
/// 它**很短命**：实测 14:38:54 换到的 token，14:42:09 重放就被拒。
/// 换完立刻用，不要缓存。
pub const CODE_SSO_EXPIRED: i64 = 200050409;

/// `401 请求签名校验失败` —— 签名算式/盐/时间戳任一处不对。
pub const CODE_SIGNATURE_REJECTED: i64 = 401;

/// `11029` —— 上游"活动太火爆"。H5 收到它就 toast「活动太火爆了，请稍后再试」。
///
/// 它**不是**"参数错了"：请求被正常处理了，只是这次没给。可以退避后重试。
pub const CODE_RECEIVE_TOO_BUSY: i64 = 11029;

/// `614` / `615` —— H5 把它们归为同一句「领取失败，请稍后再试」，**但它们是两件事**。
///
/// # 两个码的语义都已实证（2026-09-19，账号 B）
///
/// 同一份 `receiveV3` 请求（`client=app`、`cloudId=""`、`cloudType=2`），只换 `deviceId`：
///
/// | `deviceId` | `code` | 上游 msg |
/// | --- | --- | --- |
/// | 不带 | **`615`** | 活动太火爆啦，**兑换失败**，试试其他奖品吧 |
/// | 长度对、内容错一个字符 | **`614`** | 活动太火爆啦，**锁定失败**，试试其他奖品吧 |
/// | 真实值 | `0` | （`result.receive` 正常返回） |
///
/// ⇒ **`615` = 没带 `deviceId`；`614` = `deviceId` 内容无效。**
/// 两个码的 msg 里那个动词（"兑换" vs "锁定"）就是唯一的区别线索。
///
/// ⚠ **上游的 msg 会把人带偏**：它开口就是"活动太火爆"，读起来像限流、像该退避重试，
/// 而真实原因是**参数缺失或参数无效** —— 重试一万次也没用。
/// 所以别照着 msg 做退避逻辑，先查 `deviceId`。
///
/// ⚠ 这同时说明**服务端校验的是 `deviceId` 的内容，不只是"非空"**：
/// 一个长度合法、只错一个字符的 base64 照样被拒（拿到 `614`）。
/// 所以"随便给个非空值"这条路是堵死的 —— 必须拿到真值。
///
/// 真值来自 iOS `SMSdk.getDeviceId()`，**89 字符 base64 → 65 字节**
/// （实测解码后正好 65 字节，与 `docs/capture/app-signin-samples.txt` §4.1
/// 记的 `1 + 16 + 48` 吻合）。它与 [`crate::smid`] 产出的 `B<uuid>` **不是同一个东西**。
pub const CODE_RECEIVE_FAILED: i64 = 614;
/// 见 [`CODE_RECEIVE_FAILED`]。
pub const CODE_RECEIVE_FAILED_ALT: i64 = 615;

/// 已知业务码 → 一句话排查提示。返回 `None` 表示"没有已知提示"。
pub const fn diagnose(code: i64) -> Option<&'static str> {
    match code {
        CODE_NOT_LOGGED_IN => Some("jwttoken 没带上，或 tyrzLogin 还没跑（会话没建起来）"),
        CODE_SSO_EXPIRED => Some("SSO token 已失效 —— 它很短命，换完必须立刻用"),
        CODE_SIGNATURE_REJECTED => {
            Some("签名不对：盐 / 算式 / x-timestamp（必须取自 /ycloud/api/timestamp）")
        }
        CODE_RECEIVE_TOO_BUSY => Some("上游「活动太火爆」—— 不是参数错，退避后重试"),
        // 614 / 615 的 msg 与 11029 长得几乎一样（都在说"火爆"），但成因完全不同：
        // 前两者是**参数问题**（重试无用），后者才是真的忙。别按 msg 归类。
        CODE_RECEIVE_FAILED_ALT => Some(
            "领取失败 —— 实测这个码就是「没带 H5 的 deviceId」；上游 msg 说的「兑换失败」是误导",
        ),
        CODE_RECEIVE_FAILED => Some(
            "领取失败 —— 实测这个码就是「H5 的 deviceId 内容无效」；上游 msg 说的「锁定失败」是误导",
        ),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 1 段：换 SSO token（原生，A 族头）
// ─────────────────────────────────────────────────────────────────────────────

/// 签到页的 H5 地址（WebView 打开的落地页，也是后续请求的 `referer`）。
///
/// 真实客户端把 SSO token 直接拼在查询串里交给 WebView —— H5 自己拿它去
/// `tyrzLogin` 建会话。纯协议实现里这个 URL 只用来当 `referer`。
pub fn signin_page_url(sso_token: &str) -> String {
    format!(
        "{}/portal/mobilecloud/index.html?path=newsignin&sourceid={SOURCE_ID}\
         &enableShare=1&appID={H5_APP_ID}&token={sso_token}&targetSourceId={TO_SOURCE_ID}",
        base::MCLOUD
    )
}

/// `querySpecTokenV2` 的请求体。
pub fn query_spec_token_body(to_source_id: &str) -> Value {
    json!({ "toSourceId": to_source_id })
}

/// `POST /user/querySpecTokenV2`（**明文**）。
///
/// # 头集合：用登录链路那一套，不是抓包里那一套
///
/// 抓包原样是**更精简**的一组头（有 `origin: mcloudlocal://yun.139.com`、
/// `x-mm-source`、`x-yun-tid`、`x-deviceinfo`，且**没有** `x-yun-uni`）。
/// 实测两套都能过（2026-09-19 账号 B 用本函数这套跑通），所以这里复用
/// [`crate::app_login`] 的 [`AppProfile::headers`] —— 一套头只维护一处。
///
/// `timestamp` 是**微秒**级（`x-yun-user-timestamp` 是 16 位）。
pub fn query_spec_token_request(
    profile: &AppProfile,
    timestamp: i64,
    authorization: &str,
    body: &Value,
) -> Result<HttpRequest, Error> {
    let request =
        HttpRequest::post(format!("{}/user/querySpecTokenV2", base::USER_YUN)).json_body(body)?;
    Ok(
        crate::app_login::with_app_headers(request, profile, timestamp)
            .header("Authorization", authorization),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 2 段：服务端时间戳 + 签名
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /ycloud/api/timestamp`（**无鉴权**）。
///
/// 它给的是 13 位毫秒。签名里的 `x-timestamp` 必须与它逐字相同，
/// 所以**不能**用本地时钟替代 —— 本地时钟偏一点就是 `401`。
pub fn h5_timestamp_request() -> HttpRequest {
    HttpRequest::get(format!("{}/ycloud/api/timestamp", base::MCLOUD)).header("Accept", "*/*")
}

/// 一次 H5 请求的签名头（四个字段一一对应四个请求头）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H5Signature {
    /// `x-request-id`：UUID v4。
    pub request_id: String,
    /// `x-timestamp`：取自 [`parse_server_timestamp`]，13 位毫秒的字符串形式。
    pub timestamp: String,
    /// `x-nonce`：UUID v4。
    pub nonce: String,
    /// `x-signature`：小写 hex 的 MD5。
    pub signature: String,
}

/// 用**给定的** `request_id` / `nonce` 算签名（纯函数，黄金向量走这条）。
///
/// 算式：`md5(盐 + request_id + timestamp + nonce + "" + 盐)`，
/// 其中盐 = `sekaMdYYLIZfbCfm`、最后那段 `""` 是 H5 留的预留位（实测恒为空串）。
///
/// 与 [`mcloud_core::generate_signature`] 完全同构，所以直接复用 ——
/// 那一个函数同时服务 PC 端签名与 App 签到页签名，因为**盐值本来就相同**。
pub fn sign_h5_request_with(request_id: &str, timestamp: &str, nonce: &str) -> H5Signature {
    H5Signature {
        request_id: request_id.to_string(),
        timestamp: timestamp.to_string(),
        nonce: nonce.to_string(),
        signature: generate_signature(request_id, timestamp, nonce, H5_SIGN_SALT_KEY, ""),
    }
}

/// 现生成 `request_id` / `nonce` 并签名。
///
/// `rng` 注入而不是内部取全局随机数：这样整条链路在测试里可复现
/// （与 [`crate::app_login::new_login_random`] 同一个理由）。
pub fn sign_h5_request(rng: &mut impl Random, server_timestamp: &str) -> H5Signature {
    let request_id = rng.uuid_v4();
    let nonce = rng.uuid_v4();
    sign_h5_request_with(&request_id, server_timestamp, &nonce)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 3 段：建会话（tyrzLogin）
// ─────────────────────────────────────────────────────────────────────────────

/// `tyrzLogin` 的请求体。
pub fn tyrz_login_body(sso_token: &str) -> Value {
    json!({
        "token": sso_token,
        "openAccount": 0,
        "marketName": MARKET_NAME,
        "sourceId": SOURCE_ID,
    })
}

/// `POST /ycloud/auth-service/auth/tyrzLogin`（**带签名**）。
///
/// 成功时响应 `result.token` 就是后面要带的 `jwttoken`（同时服务端也会
/// `Set-Cookie` 下发 `jwtToken` / `userDomainId` —— 纯协议实现不需要 cookie jar）。
pub fn tyrz_login_request(
    sign: &H5Signature,
    referer: &str,
    body: &Value,
) -> Result<HttpRequest, Error> {
    let request = HttpRequest::post(format!(
        "{}/ycloud/auth-service/auth/tyrzLogin",
        base::MCLOUD
    ))
    .json_body(body)?;
    Ok(with_h5_headers(request, referer, None, "true")
        .header("Origin", base::MCLOUD)
        .header("x-request-id", &sign.request_id)
        .header("x-timestamp", &sign.timestamp)
        .header("x-nonce", &sign.nonce)
        .header("x-signature", &sign.signature))
}

// ─────────────────────────────────────────────────────────────────────────────
// 签到主体（全部需要 jwttoken 头）
// ─────────────────────────────────────────────────────────────────────────────

/// H5 请求的公共头。
///
/// `jwt` 为 `None` 表示"还没建会话"（只有 `tyrzLogin` 那一条是这样）。
/// `show_loading` 是纯 UI 提示（H5 的 loading 蒙层），照抄是为了与真实客户端
/// 逐字一致，服务端不据此做任何判断。
fn with_h5_headers(
    request: HttpRequest,
    referer: &str,
    jwt: Option<&str>,
    show_loading: &str,
) -> HttpRequest {
    let mut request = request;
    // `json_body` 会自己塞一个 `content-type: application/json`；H5 发的是带 charset
    // 的那个，且**只发一个**。理由与 `app_login::with_app_headers` 同（同名头会重复）。
    //
    // ⚠ 剥掉之后**必须按 body 补回来**：剥了不补 = 所有 H5 POST 裸发。
    // 之前这里就漏了这一步（`h5_json` 在剥离之前调用，补的又被剥掉），
    // 是 `task_list_v3_body_matches_capture` 这条断言抓出来的。
    // 现在 content-type 只在这一个地方产生，不再有"谁先谁后"的问题。
    request
        .headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
    if request.body.is_some() {
        request = request.header("Content-Type", "application/json;charset=UTF-8");
    }
    request = request
        .header("Accept", "*/*")
        .header("activityid", MARKET_NAME)
        .header("appversion", H5_APP_VERSION)
        .header("Referer", referer)
        .header("showloading", show_loading);
    if let Some(jwt) = jwt {
        // 头名是小写 `jwttoken`（不是 `jwtToken`）—— H5 拦截器原样如此。
        request = request.header("jwttoken", jwt);
    }
    request
}

/// 查询串值的百分号编码。
///
/// `deviceId` 是 base64，含 `+` `/` `=`。**`+` 在查询串里表示空格** ——
/// 不编码就会被服务端解成另一个值（而且看起来"没报错"，只是行为不对）。
/// 抓包里这三个字符正是 `%2B` / `%2F` / `%3D`。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET /ycloud/signin/page/infoV3?client=app` —— **签到状态，权威判据**。
pub fn info_v3_request(jwt: &str, referer: &str) -> HttpRequest {
    with_h5_headers(
        HttpRequest::get(format!(
            "{}/ycloud/signin/page/infoV3?client={CLIENT}",
            base::MCLOUD
        )),
        referer,
        Some(jwt),
        "true",
    )
}

/// `GET /ycloud/signin/page/startSignIn?client=app` —— 执行签到。
///
/// # `device_id`
///
/// 真实客户端会带 `deviceId` 查询参数（来自 iOS `SMSdk` bridge）+ `isdeviceid: true` 头。
/// **实测不带也能签上**（账号 B，`{"code":0,"msg":"success"}`），所以它是 `Option`。
/// 但调用点必须显式写 `None` 或 `Some(..)` —— 见模块头的说明。
///
/// 传 `Some` 时会**同时**补上 `isdeviceid: true` 头：抓包里两者总是成对出现，
/// 只发一个等于自相矛盾（H5 的拦截器就是按那个头决定要不要注入参数）。
pub fn start_sign_in_request(jwt: &str, referer: &str, device_id: Option<&str>) -> HttpRequest {
    let url = match device_id {
        Some(id) => format!(
            "{}/ycloud/signin/page/startSignIn?client={CLIENT}&deviceId={}",
            base::MCLOUD,
            percent_encode(id)
        ),
        None => format!(
            "{}/ycloud/signin/page/startSignIn?client={CLIENT}",
            base::MCLOUD
        ),
    };
    let request = with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "false");
    match device_id {
        Some(_) => request.header("isdeviceid", "true"),
        None => request,
    }
}

/// `POST /ycloud/signin/task/taskListV3` —— 任务列表。
///
/// `client`：`1` = App/H5 已登录（真实客户端），`0` = 匿名。两者都能返回数据，
/// 但字段取值会变（如 `button.canReceive`），所以做成参数而不是常量。
pub fn task_list_v3_request(jwt: &str, referer: &str, client: u8) -> Result<HttpRequest, Error> {
    let body = json!({
        "marketname": MARKET_NAME,
        "client": client,
        "clientVersion": TASK_CLIENT_VERSION,
    });
    let request = HttpRequest::post(format!("{}/ycloud/signin/task/taskListV3", base::MCLOUD))
        .json_body(&body)?;
    Ok(with_h5_headers(request, referer, Some(jwt), "true"))
}

/// `POST /ycloud/signin/page/doTaskPost` —— 上报任务状态。
///
/// `device_id` 是**必填 `&str`**（不是 `Option`）：这个接口在抓包里标了
/// `isDeviceId: true`，且同族的领取接口实测**不带就报错**。
/// 让漏发变成编译期错误，而不是运行时的上游报错。
pub fn do_task_post_request(
    jwt: &str,
    referer: &str,
    device_id: &str,
) -> Result<HttpRequest, Error> {
    let body = json!({ "client": CLIENT, "deviceId": device_id });
    let request = HttpRequest::post(format!("{}/ycloud/signin/page/doTaskPost", base::MCLOUD))
        .json_body(&body)?;
    Ok(with_h5_headers(request, referer, Some(jwt), "true").header("isdeviceid", "true"))
}

/// `POST /ycloud/signin/page/receiveV3` —— **领云朵**。
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// receiveCloudData: function(e){
///   return c["a"].post("/ycloud/signin/page/receiveV3", JSON.stringify(e),
///     {headers:{showLoading:true, isDeviceId:true, "Content-Type":"application/json;charset=UTF-8"}})
/// }
/// ```
///
/// 调用点组体是 `{client, cloudId, cloudType}`：
/// `client` 在 App 内是 `"app"`（小程序 `"mini"`、其他 `"out"`）；
/// `cloudId` 来自 `e.recordId ?? ""`、`cloudType` 来自 `e.cloudType ?? ""`。
///
/// # 为什么 `device_id` 是必填 `&str`
///
/// 它是本模块里**已知唯一一个"不带就失败"**的接口（H5 标了 `isDeviceId: true`，
/// 同族接口实测不带会报错）。所以让它凑不出空值 —— 漏发在编译期就暴露。
///
/// ⚠ 这里要的是 **H5 那个 65 字节 blob**（iOS `SMSdk.getDeviceId()`），
/// **不是** [`crate::smid`] 产出的 `B<uuid>`。两者同名不同物，不能互换。
pub fn receive_v3_request(
    jwt: &str,
    referer: &str,
    cloud_id: &str,
    cloud_type: Option<i64>,
    device_id: &str,
) -> Result<HttpRequest, Error> {
    // H5 是 `e.cloudType ?? ""` —— 缺省给的是**空串**，不是 `0`。
    // 照抄，别"顺手改成 0"：`0` 是个合法取值（可能代表另一种可领类型）。
    let cloud_type = match cloud_type {
        Some(value) => json!(value),
        None => json!(""),
    };
    let body = json!({
        "client": CLIENT,
        "cloudId": cloud_id,
        "cloudType": cloud_type,
        "deviceId": device_id,
    });
    let request = HttpRequest::post(format!("{}/ycloud/signin/page/receiveV3", base::MCLOUD))
        .json_body(&body)?;
    Ok(with_h5_headers(request, referer, Some(jwt), "true").header("isdeviceid", "true"))
}

/// `POST /ycloud/signin/page/receiveRevivalReward` —— **用复活卡 / 回馈卡**。
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// receiveRevivalReward: function(e){
///   return c["a"].post("/ycloud/signin/page/receiveRevivalReward", JSON.stringify(e),
///     {headers:{showLoading:false, "Content-Type":"application/json;charset=UTF-8"}})
/// }
/// ```
///
/// 调用点组体是 `{}`（`z["a"].receiveRevivalReward({})`）—— 所以 body 是 `"{}"`，
/// 这也是老抓包里的样子（`content-length: 2`）。**没有查询参数、没有别的字段。**
///
/// 注意 `showLoading: false`：同页其余接口几乎都是 `true`，只有它是 `false`
/// （H5 要自己画那张卡片的弹窗，不想要全局 loading 遮住）。
///
/// # `device_id`：两个证据不一致，两个分支都留着
///
/// | 证据 | 结论 |
/// | --- | --- |
/// | H5 源码（iOS 12.6.0）| 头里**没有** `isDeviceId` ⇒ 拦截器不注入 |
/// | 老抓包（Android WebView）| 带一个 `deviceid` **头**（值被脱敏，看不出是不是同一个 blob）|
///
/// 所以做成 `Option<&str>`：`None` = 照 H5 定义（缺省，推荐）；
/// `Some(v)` = 照老抓包补一个 `deviceid` 头。**哪个是服务端真正要求的，未验证** ——
/// 这也正是留成可选参数的意义：真出问题时能一次 A/B 试出来（`receiveV3` 就是这么定的）。
///
/// ⚠ 这里**不加** `isdeviceid` 头（老抓包里也没有）—— 别照 `receive_v3_request` 抄。
pub fn receive_revival_reward_request(
    jwt: &str,
    referer: &str,
    device_id: Option<&str>,
) -> Result<HttpRequest, Error> {
    // 空对象体：不是 `None`。`json_body` 会给出 `"{}"`，与抓包的 `content-length: 2` 对齐。
    let request = HttpRequest::post(format!(
        "{}/ycloud/signin/page/receiveRevivalReward",
        base::MCLOUD
    ))
    .json_body(&json!({}))?;
    let request = with_h5_headers(request, referer, Some(jwt), "false");
    Ok(match device_id {
        Some(id) => request.header("deviceid", id),
        None => request,
    })
}

/// `GET /ycloud/signin/page/receiveTaskExpansion?acceptDate=<acceptDate>` —— **延期领取**。
///
/// # 它在"领取"里的位置（这是 H5 的逻辑，不是可有可无的一支）
///
/// `infoV3` 的 `receiveList[]` 按 `cloudType` 分流，逐字见 [`ReceiveItem::cloud_type`]：
///
/// | `cloudType` | 含义 | 动作 |
/// | --- | --- | --- |
/// | `1` | 延期领取 | **本函数**（要 `acceptDate`） |
/// | `2` | 下月可领取 | 什么都不发 |
/// | 其它 | 可领取 | [`receive_v3_request`] |
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// receiveTaskExpansion: function(e){
///   return c["a"].get("/ycloud/signin/page/receiveTaskExpansion",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：A("2", t.acceptDate, e) → z["a"].receiveTaskExpansion({acceptDate: t})
/// ```
///
/// 即**查询参数只有一个 `acceptDate`**、**没有 body**、**没有 `isDeviceId`**
/// （H5 没标 ⇒ 拦截器不注入）。别照 [`receive_v3_request`] 抄那套。
///
/// # 错误码（H5 的处理分支，留着备查）
///
/// `0` 成功 / `1302` 「开启自动备份按钮才可领取」/ `500` 「AI豆已过领取有效期」。
///
/// ⚠ `acceptDate` 来自 `receiveList[]` 里**同一条**的 `acceptDate` —— 不是自己编的日期。
///
/// # 为什么是 `Option<&str>`（而不是必填 `&str`，也不是空串）
///
/// 这条是**被实拍推翻过两次**的地方，写清楚免得再改回去：
///
/// - H5 的调用点逐字是 `A("2", t.acceptDate, e)` → `receiveTaskExpansion({acceptDate: t})`，
///   参数取自**同一条条目**。
/// - 而**实测两条账号的 `receiveList` 里都没有 `acceptDate` 这个字段**（`--raw` 的原始
///   响应里只有 `cloudNum` / `cloudType`）⇒ 那个值是 `undefined`。
/// - axios 的默认 `paramsSerializer` 对 `null` / `undefined` 是**整个键丢掉**
///   （不是发 `acceptDate=`）⇒ 真实客户端发出去的是**裸 GET，一个查询参数都没有**。
///
/// 所以：
/// `None` → 不带查询串（**与真实客户端一致，是缺省路径**）；
/// `Some(v)` → `?acceptDate=<百分号编码后的 v>`。
///
/// ⚠ 别把 `None` 写成 `Some("")` —— 那会发出 `?acceptDate=`，是**另一个请求**
/// （多一个空参数）。服务端怎么处理没验过，没必要去踩。
pub fn receive_task_expansion_request(
    jwt: &str,
    referer: &str,
    accept_date: Option<&str>,
) -> HttpRequest {
    let url = match accept_date {
        Some(value) => format!(
            "{}/ycloud/signin/page/receiveTaskExpansion?acceptDate={}",
            base::MCLOUD,
            percent_encode(value)
        ),
        None => format!("{}/ycloud/signin/page/receiveTaskExpansion", base::MCLOUD),
    };
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `GET /ycloud/signin/page/multiple` —— **签到翻倍**（旧项目叫 `singInMultiple`）。
///
/// # 它领什么
///
/// 「自动备份」每月给**一次**翻倍机会：本月开着备份、且这次机会还没用掉时，
/// 领一次能把**今日签到奖励**翻 2–50 倍。H5 原话逐字：「备份成功，获得1次AI豆
/// 翻倍机会今日签到奖励可翻2-50倍！」，领到手后弹「额外获得 {cloudCount} 豆，
/// 自动备份保持开启，任务奖励翻倍!」。
///
/// # 形状（H5 源码逐字，`newsignin.336f18c3.js`）
///
/// ```javascript
/// cloudMultiple: function(e){
///   return c["a"].get("/ycloud/signin/page/multiple",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：z["a"].cloudMultiple({})        ← **空对象**
/// ```
///
/// ⇒ 与 [`receive_task_expansion_request`] 同一个坑：`params` 是空对象时，
/// axios 的默认序列化器**一个查询参数都不产生**。所以这里发的是**裸 GET**，
/// 不是 `?`。
///
/// # 门槛**必须自己判**，别指望上游替你把关
///
/// 旧项目 `mcloud-sign` 的判据逐字（`tasks/sign/task.ts`）：
///
/// ```javascript
/// if (curMonthBackup && !curMonthBackupSignAccept) { await executeSignInMultiple(ctx); }
/// ```
///
/// 两个值都在 `infoV3` 里（[`SignInStatus::cur_month_backup`] /
/// [`SignInStatus::cur_month_backup_sign_accept`]）。**两个都成立才去领** ——
/// 每月只有一次机会，条件不成立还发，是**白白消耗**。
/// 判据的可执行版本在 `mcloud_app::signin::plan_multiple`。
pub fn multiple_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/ycloud/signin/page/multiple", base::MCLOUD);
    // `showLoading: true`（H5 逐字）—— 与 `receiveTaskExpansion` 同档。
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `cloudRecordV2` 的 `aiDou` —— **账本**。两个值，不是两种货币。
///
/// ⚠ 云朵与「云盘专属 AI豆」是**同一个东西的两个名字**（2026-07-31 按 1:1 改名）。
/// 这两档分开只是因为**记录跨了那次改名**：旧记录留在旧账本里，新记录进新账本。
/// 别把它理解成"两个池子"或"两种货币" —— 那种理解会让人以为可以互相换算。
pub const RECORD_BOOK_CLOUD: i64 = 0;
/// 见 [`RECORD_BOOK_CLOUD`]。`1` = 改名之后的账本。
pub const RECORD_BOOK_BEAN: i64 = 1;

/// `cloudRecordV2` 的 `type` —— H5 那三个筛选页签。
pub const RECORD_FILTER_ALL: i64 = 0;
/// 「获得记录」。
pub const RECORD_FILTER_EARNED: i64 = 1;
/// 「使用记录」。
pub const RECORD_FILTER_SPENT: i64 = -1;

/// `GET /ycloud/signin/public/cloudRecordV2` —— **豆的收支流水**（审计线）。
///
/// # 它解决什么问题
///
/// `infoV3.total` 给的是**当前余额**，但它不告诉你这个数是怎么来的 ——
/// 于是"余额对不上"这类问题只能靠猜。这条给的是**流水**。
///
/// # 形状（H5 源码逐字，`139newsignin.cc383f1b.js`）
///
/// ```javascript
/// getAIBeanRecordData: function(e){
///   return c["a"].get("/ycloud/signin/public/cloudRecordV2",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：{aiDou: 0|1, type: p.value, pageNumber: F.value, pageSize: 10}
/// ```
///
/// 即 **GET + 四个查询参数**、**没有 body**、**没有 `isDeviceId`**（H5 没标 ⇒ 拦截器不注入）。
/// `showLoading: true`。
///
/// ⚠ 函数名里那个 `AIBean` 是**误导**：同一个函数被两个页面共用，
/// 靠 `aiDou` 切换账本（`pointRecord` 传 `0`、`AIBeanRecord` 传 `1`）。
pub fn cloud_record_v2_request(
    jwt: &str,
    referer: &str,
    ai_dou: i64,
    filter: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{}/ycloud/signin/public/cloudRecordV2?aiDou={ai_dou}&type={filter}\
         &pageNumber={page_number}&pageSize={page_size}",
        base::MCLOUD
    );
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `record` 的 `marketname` —— H5 两个奖品记录组件**都传这个值**（逐字核对过两处调用点）。
///
/// ⚠ 名字里的 `_ex` **不是"兑换"**：这两个组件的页签是「待领取 / 已领取 / 已过期」，
/// 是**奖品记录**不是兑换流水（兑换走的是 `exchangeV3` / `exchangeList`）。
/// 别看到 `_ex` 就往"exchange"上想。
pub const PRIZE_MARKET_NAME: &str = "sign_in_3_ex";

/// `record` 的 `type` —— 页签。见 [`PRIZE_MARKET_NAME`] 上那句提醒。
///
/// 这三个值是 H5 里**写死的字面量**（`[{id:1,btnText:"待领取"},{id:2,btnText:"已领取"},
/// {id:10,btnText:"已过期"}]`），不是从响应里读出来的 —— 所以它们是常量不是枚举探测。
pub const PRIZE_STATUS_PENDING: i64 = 1;
/// 「已领取」。
pub const PRIZE_STATUS_RECEIVED: i64 = 2;
/// 「已过期」。
pub const PRIZE_STATUS_EXPIRED: i64 = 10;

/// `GET /ycloud/signin/public/record` —— **奖品记录**（默认页签是「待领取」）。
///
/// 这是旧项目 `printPendingPrizes`（`getUserPrizeLogPageV2`）在新接口上的对应物。
///
/// # 形状（H5 源码逐字，两处调用点）
///
/// ```javascript
/// getPrizeRecordData: function(e){
///   return c["a"].get("/ycloud/signin/public/record",
///     {params: e, headers: {showLoading: true, "content-Type": "application/json"}})
/// }
/// // 调用点：{marketname: "sign_in_3_ex", type: <页签 id>, pageNumber: F.value, pageSize: 10}
/// ```
///
/// 即 **GET + 四个查询参数**、**没有 body**、**没有 `isDeviceId`**（H5 没标）。
/// 分页判据是 H5 逐字写的：**`records.length < pageSize` ⇒ 到底了**
/// （`t.length<10&&(X.value=!0)`）—— 它**不读 `total`**。
///
/// # 与旧接口的关系
///
/// 旧项目打的 `.../checkPrize/getUserPrizeLogPageV2?currPage=&pageSize=` 在
/// `m.mcloud.139.com` 上**仍然存在**，但**裸请求回 `501 参数校验异常`**
/// （换 `pageNumber` 直接 `404`）—— 参数口径已经变了。新接口是这条。
pub fn prize_record_request(
    jwt: &str,
    referer: &str,
    marketname: &str,
    status: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{}/ycloud/signin/public/record?marketname={}&type={status}\
         &pageNumber={page_number}&pageSize={page_size}",
        base::MCLOUD,
        percent_encode(marketname)
    );
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `GET /market/backupgift/info` —— **备份好礼**的状态。
///
/// # 为什么主机是 `caiyun.feixin.10086.cn` 而不是 `m.mcloud.139.com`
///
/// 这个接口**不在新 H5 的签到页里** —— 新 H5 里 `backupGift` 只出现一次，
/// 是一个指向**独立活动页**的 `popLink`
/// （`/portal/caiyunEventPage/index.html?path=backupGifts`）。
/// 所以它没有"新接口"可换，沿用的是旧项目那条。
///
/// ⚠ 换主机是**有实测依据**的，不是照抄旧项目：裸请求
/// `https://caiyun.feixin.10086.cn/market/backupgift/info` 回
/// `{"code":90001,"msg":"未登录"}` —— 与 `m.mcloud.139.com` 上的
/// `record` / `liveRoomFeedback` **回同一个信封**，说明两者在**同一道鉴权网关**后面。
/// （若哪天它回 `404` 而不是 `90001`，就是换了主机，别以为"接口下线了"。）
pub fn backup_gift_info_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/market/backupgift/info", base::CAIYUN);
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

/// `GET /market/backupgift/receive` —— **领备份好礼**。写请求。
///
/// 门槛**不在这里判**：旧项目先读 [`backup_gift_info_request`] 的 `state`，
/// 只有 `state == 0`（可领）才发这条。可执行版本在
/// `mcloud_app::signin::plan_backup_gift`。
pub fn backup_gift_receive_request(jwt: &str, referer: &str) -> HttpRequest {
    let url = format!("{}/market/backupgift/receive", base::CAIYUN);
    with_h5_headers(HttpRequest::get(url), referer, Some(jwt), "true")
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应解析
// ─────────────────────────────────────────────────────────────────────────────
/// 从信封里取一个必填字符串字段，缺了就报 [`Error::Parse`]。
///
/// 不用 `serde` 反序列化整包：这些响应里有十几处"服务端有时给、有时不给"的字段，
/// 用 `#[serde(default)]` 会把"其实没给"静默成默认值 —— 而这里恰恰要靠字段判断成败。
fn require_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse(format!("响应缺少字符串字段 `{key}`")))
}

fn require_i64(value: &Value, key: &str) -> Result<i64, Error> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Parse(format!("响应缺少整数字段 `{key}`")))
}

/// 业务失败 → [`Error::BadRequest`]，并把 [`diagnose`] 的提示附在 detail 里。
///
/// **不**把上游原文放进对外文案：`Error::public_message()` 走白名单，
/// detail 只进日志。理由见 `crate::error_from_status`。
fn ensure_ok(envelope: &Envelope) -> Result<Option<&Value>, Error> {
    match envelope.ensure_ok() {
        Ok(payload) => Ok(payload),
        Err(err) => match envelope.code().and_then(diagnose) {
            Some(hint) => Err(Error::BadRequest(format!("{err}（{hint}）"))),
            None => Err(err),
        },
    }
}

/// 解析 `querySpecTokenV2` 的响应，取出 SSO token。
pub fn parse_sso_token(envelope: &Envelope) -> Result<String, Error> {
    let payload = ensure_ok(envelope)?
        .ok_or_else(|| Error::Parse("querySpecTokenV2 响应没有 data".to_string()))?;
    Ok(require_str(payload, "token")?.to_string())
}

/// 解析 `GET /ycloud/api/timestamp` 的响应，取出**字符串形式**的 13 位毫秒。
///
/// 转成字符串是因为它下一步要原样进 `x-timestamp` 头与签名 ——
/// 中途过一遍数字类型只会多一次"格式化可能不一致"的机会。
pub fn parse_server_timestamp(envelope: &Envelope) -> Result<String, Error> {
    let payload = ensure_ok(envelope)?;
    let ts = payload
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Parse("api/timestamp 的 result 不是整数".to_string()))?;
    Ok(ts.to_string())
}

/// `tyrzLogin` 成功后的会话凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H5Login {
    /// 后面所有签到请求要带的 `jwttoken`。
    pub jwt_token: String,
    /// 响应里回显的手机号（用于确认签的是哪个账号）。可能缺失。
    pub mobile: Option<String>,
}

/// 解析 `tyrzLogin` 的响应。
pub fn parse_jwt_token(envelope: &Envelope) -> Result<H5Login, Error> {
    let payload = ensure_ok(envelope)?
        .ok_or_else(|| Error::Parse("tyrzLogin 响应没有 result".to_string()))?;
    Ok(H5Login {
        jwt_token: require_str(payload, "token")?.to_string(),
        mobile: payload
            .get("mobile")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// `infoV3` 的 `cal[]` 里的一天。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignInDay {
    /// 日（1..=31）。
    pub day: u8,
    /// 该日是否已签（`s`）。
    pub signed: bool,
    /// 是否属于本月（`currentMonth == 1`）。`false` 的是上/下月的补位格。
    pub current_month: bool,
    /// 该日可获得云朵数（`n`）。
    pub points: i64,
    /// 服务端是否把该日标为"今天"（`t`）。
    pub today: bool,
}

/// `infoV3` 的 `receiveList[]` 里的一条 —— "有一笔云朵挂在账上"。
///
/// 它同时是 [`receive_v3_request`] 的**输入来源**（`cloudId` / `cloudType` 都取自这里）。
///
/// ⚠ **名字里的 "receive" 不等于"现在能领"** —— 见 [`Self::cloud_type`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveItem {
    /// `cloudType` —— **决定这一笔现在能不能领**。
    ///
    /// 判据是当前版 H5 源码 `newsignin.336f18c3.js`（本仓库 `har/h5-signin/`）里的
    /// 那个点击分流，逐字如下：
    ///
    /// ```js
    /// 1 === t.cloudType ? A("2", t.acceptDate, e)          // receiveTaskExpansion：延期领取
    /// : 2 === t.cloudType ? s.$toast("下月可领取")           // ← 纯提示，**不发任何请求**
    /// : n("receivePoints", t, "", …)                        // ← 只有这一支走 receiveV3
    /// ```
    ///
    /// 同一份源码里还有两处独立旁证：模板上 `"is-next-month": 2 === g[0]?.cloudType`，
    /// 以及标签 `2 === g[0]?.cloudType ? "" : "可领取"` —— 即 `2` **不显示"可领取"**。
    ///
    /// ⇒ 所以 `2` 是**「下月可领取」**，不是"待领取"。给它发 `receiveV3` 是
    /// **官方客户端不会做的事**，实测回 `code=0` 但 `result.receive = 0`（什么都没领到，
    /// 见 `docs/capture/app-signin-samples.txt`）。
    ///
    /// ⚠ 这里曾经写着"新版 H5 则一律走 `receiveV3`"——**没有依据，已删**。
    /// 抓包里那条账号的 `receiveList` 只有一条 `{cloudType:2, cloudNum:234}`，
    /// 而整个 HAR 里 `receiveV3` 的请求数是 **0**：官方客户端正因为 `cloudType==2`
    /// 而**从未调用过它**。这才是"2 不可领"最直接的证据。
    ///
    /// # `0` 是**当前可领取**（2026-09-19 实测）
    ///
    /// 同一天稍晚，账号 159 的账目变成两条：
    ///
    /// ```json
    /// [{cloudType:2, cloudNum:240}, {cloudType:0, cloudNum:6}], "toReceive": 6
    /// ```
    ///
    /// 走 `receiveV3(cloudType=0)` 之后：`toReceive` 变 `0`、`cloudType:0` 那条
    /// **从账目里消失**，而 `cloudType:2` 的 240 **一动没动**。
    ///
    /// ⇒ 两个结论：① `0` 是**合法取值**（"当前可领取"），不是"上游没给"——
    /// 这正是 [`receive_v3_request`] 把缺字段写成空串、而不是写成 `0` 的原因；
    /// ② `toReceive` 是**当前可领取的合计**（`6`），不是废字段。
    pub cloud_type: i64,
    /// 这笔有多少云朵（`cloudNum`）。
    pub cloud_num: i64,
    /// `recordId` —— [`receive_v3_request`] 的 `cloud_id` 来源。
    ///
    /// ⚠ 实测抓到的条目里**没有这个字段**（只有 `cloudType` / `cloudNum`），
    /// 于是 H5 的 `e.recordId ?? ""` 实际给的就是空串。这里如实做成 `Option`，
    /// **不替上游编一个默认值**。
    pub record_id: Option<String>,
    /// `acceptDate` —— **只有 `cloudType == 1` 才用得上**。
    ///
    /// H5 里 `1 === t.cloudType ? A("2", t.acceptDate, e)`，那个 `A` 就是
    /// `receiveTaskExpansion({acceptDate})`（见 [`receive_task_expansion_request`]）。
    /// 少了它，`cloudType == 1` 那条路就**根本发不出去** —— 只能干瞪眼。
    ///
    /// 实测抓到的条目（`cloudType == 2`）里没有这个字段，所以是 `Option`。
    pub accept_date: Option<String>,
}

/// `infoV3` 的解析结果 —— **判断"今天签没签"的唯一权威依据**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInStatus {
    /// **现行余额（豆）** —— 上游字段 `total`。
    ///
    /// # 为什么是 `total` 而不是 `beforeTotal`（这里记错过一次）
    ///
    /// H5 把两个数都显示出来，原话逐字：
    ///
    /// ```javascript
    /// total >= 600 && 1 == isCloudEnough
    ///   ? "恭喜你，现拥有" + total + "豆，足够兑换" + …
    ///   : total >= 600 && 0 == isCloudEnough
    ///     ? "您只需要再攒" + lackCloudNum + "豆就能兑换" + …
    /// ```
    ///
    /// 即 **`total` 是「现拥有」，`lackCloudNum` 是「还差多少」**。
    /// 实测自洽：兑换门槛 `canExchangeText[0].pOrder = 12000`，
    /// 而 `12000 - 10502(total) = 1498 = lackCloudNum` ✓。
    ///
    /// 两个数的**来源也分得清**（Vuex store `newsignin`）：
    /// `beforeTotal → setBeforeCloudTotal`、`total → setCloudTotal`；
    /// 前者只有**云朵明细（历史）页** `pointRecord` 读，后者是 **AI豆明细页**
    /// `AIBeanRecord` 读、而且**只有它进 `persist`**。
    ///
    /// ⚠ `beforeTotal` 我们**记错过**（当成余额记进 `cloud_stats`，趋势图因此是一条直线）。
    /// 它现在仍然解析出来（[`Self::cloud_total`]），但**不再当作余额用**。
    ///
    /// # 第三个独立证据（2026-09-19）：它就是账本的合计
    ///
    /// `GET cloudRecordV2?aiDou=1` 那一档的**全部条目求和，逐字等于 `total`**：
    /// 账号 1 = 7431、账号 2（159）= 10502，两个都命中。
    /// ⇒ 这个数不是"某个接口报的另一个值"，而是**那本流水从头到尾的累加**。
    ///
    /// ⚠ 云朵与 AI豆是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
    /// 所以别把这两个字段说成"两种货币" —— 它们只是**两个口径的数**。
    pub bean_total: i64,
    /// 上游字段 `beforeTotal` —— **改名那一刻的结转额**，不是余额。
    ///
    /// # 它到底是什么（2026-09-19 从流水里定案）
    ///
    /// 它就是改名交接的那一条：`GET cloudRecordV2?aiDou=1` 里
    /// `mark: "shouTiaoAiDou"` / `summary: "云朵转换AI豆"` 那笔的 `num`。
    ///
    /// | 账号 | `shouTiaoAiDou` | `beforeTotal` | AI豆账本合计 | `total` |
    /// | --- | --- | --- | --- | --- |
    /// | 1 | 5392 | **5392** ✓ | 7431 | **7431** ✓ |
    /// | 2（159）| 8027 | **8027** ✓ | 10502 | **10502** ✓ |
    ///
    /// ⇒ 两件事同时坐实：
    /// 1. `beforeTotal` = **改名时结转进来的那笔**，是一张**冻结的历史快照**，
    ///    所以它**永远不变**（先前"签到前后都不动"的实测，原因在这里）。
    /// 2. `bean_total`（= `total`）= **AI豆账本从头到尾的合计**，逐字相等 ——
    ///    这也说明那个账本自 2026-07-30 起是**完整**的。
    ///
    /// 两者差的那 2475 = **改名后新增的全部**（8 月 1556 + 9 月 904 + 7 月 15）。
    /// 先前"差 2475、差在哪没查清"的悬案到此收口。
    ///
    /// ⚠ 还有一处**没查清**、别当成已知：云朵账本（`aiDou=0`）的合计比
    /// `beforeTotal` **恰好多 100**，两个账号都是，且那 100 都是 2024 年那笔
    /// `mark:"1003"`「开启APP通知领云朵」；账本里**没有任何 -100 的扣减**。
    /// 推测是**云朵过期不入账**（那样云朵账本的合计就只能当"毛收入"、不能当余额），
    /// 但**没有证据**，故只记现象。
    ///
    /// # 为什么它是 `Option`（而 `bean_total` 不是）
    ///
    /// 判据是"缺了它还能不能回答本结构的问题"：
    /// `bean_total` 是**余额**，是我们要记、要报的那个数 —— 缺了就等于没答；
    /// 而这个是**我们不用的一个旁证数**，缺了只是少一条线索。
    /// 让一个不用的字段有能力把整条状态查询打挂，是没必要付的代价。
    pub cloud_total: Option<i64>,
    /// 本次签到可得数（`signInPoints`）。H5 渲染成 `+"{signInPoints}豆"`。
    pub sign_in_points: i64,
    /// 连续签到天数。
    pub sign_count: i64,
    /// 服务端报的本月已签天数（`monthDays`）。
    pub month_days: i64,
    /// 日历格。
    pub days: Vec<SignInDay>,
    /// 可领取的条目（`receiveList`）。
    ///
    /// 上游不给这个字段时是**空数组**，不是错误 —— 它只影响"要不要去领"，
    /// 不影响"今天签没签"这个主判据。所以这里不做 [`require_str`] 那种硬要求。
    pub receive_list: Vec<ReceiveItem>,
    /// **现在就能领**的合计（`toReceive`）。上游有时不给，所以是 `Option`。
    ///
    /// 语义是实测定的：账目 `[{2,240},{0,6}]` 时它是 `6`，领掉 `0` 那条之后变 `0`，
    /// 而 `2` 那条**不计入**。⇒ 它只统计「立即领取」那一类，不含月度线。
    pub to_receive: Option<i64>,
    /// 本月**是否已开启自动备份**（`curMonthBackup`）。
    ///
    /// 它是**两条翻倍线的总门槛**：
    /// - 月度任务奖励（`cloudType=1`）领不到会回 `1302`「开启自动备份按钮才可领取哦～」；
    /// - 签到翻倍（[`multiple_request`]）的前提条件之一。
    ///
    /// `None` = 上游没给这个字段（**不是 `false`**）。判据在
    /// [`crate::app_signin::multiple_request`] 的文档里 —— 别把 `None` 当 `false` 处理，
    /// 那会让"上游没报"变成"这个账号没开备份"。
    pub cur_month_backup: Option<bool>,
    /// 本月的**签到翻倍**是否**已经领过**（`curMonthBackupSignAccept`）。
    ///
    /// 每月只有一次机会，所以它是 [`multiple_request`] 的另一个前提条件。
    /// 旧项目 `mcloud-sign` 的判据逐字是
    /// `if (curMonthBackup && !curMonthBackupSignAccept)` —— 两个都要成立才去领。
    pub cur_month_backup_sign_accept: Option<bool>,
}

impl SignInStatus {
    /// 本月某天是否已签。传 `0` 或 `>31` 返回 `None`。
    ///
    /// 只看 `current_month == true` 的格子 —— `cal` 里还混着上月最后一天
    /// 与下月开头几天，按 `d` 裸查会命中错的月份（实测抓包里就有 `d:31` 的上月格）。
    pub fn signed_on(&self, day: u8) -> Option<bool> {
        if day == 0 || day > 31 {
            return None;
        }
        self.days
            .iter()
            .find(|d| d.current_month && d.day == day)
            .map(|d| d.signed)
    }

    /// 服务端标为"今天"的那格是否已签。
    ///
    /// 用它**不需要本地时钟** —— 这也是它比"自己算今天几号"更稳的地方
    /// （服务端与时区才是判"今天"的那一方）。`cal` 里没有 `t:true` 的格子时返回 `None`。
    pub fn signed_today(&self) -> Option<bool> {
        self.days.iter().find(|d| d.today).map(|d| d.signed)
    }

    /// 按 `cal` 数出来的本月已签天数。
    ///
    /// 与 [`SignInStatus::month_days`] 是两条独立路径（一个来自日历格、一个来自服务端字段），
    /// 相等才说明解析没走样。测试里用它对拍。
    pub fn counted_month_days(&self) -> usize {
        self.days
            .iter()
            .filter(|d| d.current_month && d.signed)
            .count()
    }
}

/// 解析 `infoV3` 的响应。
pub fn parse_sign_in_status(envelope: &Envelope) -> Result<SignInStatus, Error> {
    let payload = ensure_ok(envelope)?;
    let payload = payload.ok_or_else(|| Error::Parse("infoV3 响应没有 result".to_string()))?;

    let cal = payload
        .get("cal")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("infoV3 响应缺少 result.cal 数组".to_string()))?;

    let mut days = Vec::with_capacity(cal.len());
    for cell in cal {
        let day = require_i64(cell, "d")?;
        let day = u8::try_from(day)
            .map_err(|_| Error::Parse(format!("infoV3 的 cal[].d 越界: {day}")))?;
        days.push(SignInDay {
            day,
            signed: cell.get("s").and_then(Value::as_bool).unwrap_or(false),
            current_month: cell.get("currentMonth").and_then(Value::as_i64) == Some(1),
            points: cell.get("n").and_then(Value::as_i64).unwrap_or(0),
            today: cell.get("t").and_then(Value::as_bool).unwrap_or(false),
        });
    }

    // `receiveList` 与主判据无关：上游不给时给空数组，而不是报错 ——
    // 那会让"今天签没签"这件确定的事被一个可选的附属字段拖垮。
    let receive_list = payload
        .get("receiveList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ReceiveItem {
                        cloud_type: item.get("cloudType").and_then(Value::as_i64)?,
                        cloud_num: item.get("cloudNum").and_then(Value::as_i64)?,
                        record_id: item
                            .get("recordId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        accept_date: item
                            .get("acceptDate")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(SignInStatus {
        bean_total: require_i64(payload, "total")?,
        cloud_total: payload.get("beforeTotal").and_then(Value::as_i64),
        sign_in_points: require_i64(payload, "signInPoints")?,
        sign_count: require_i64(payload, "signCount")?,
        month_days: require_i64(payload, "monthDays")?,
        days,
        receive_list,
        to_receive: payload.get("toReceive").and_then(Value::as_i64),
        // 两个门槛字段都是 `Option<bool>`：**只认真正的布尔**。
        // 实测上游给的就是 `true` / `false`；万一哪天变成 `1` / `0`，这里会得到 `None`，
        // 于是 `plan_multiple` 判 `Unknown` 而**不去发那个写请求** —— 这是个安全的失败方向。
        cur_month_backup: payload.get("curMonthBackup").and_then(Value::as_bool),
        cur_month_backup_sign_accept: payload
            .get("curMonthBackupSignAccept")
            .and_then(Value::as_bool),
    })
}

/// `startSignIn` 的解析结果。
///
/// ⚠ [`SignInResult::today_sign_in`] 是**调用前**的状态，不是成败判据。
/// 实测（账号 B）：响应里 `todaySignIn:false`、`cal` 里 19 号还是 `s:false`，
/// 但紧接着的 `infoV3` 已经是 `s:true` —— 那一次确实签上了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInResult {
    /// 调用**前**今天是否已签。
    pub today_sign_in: bool,
    /// 本次签到可得云朵数。
    pub sign_in_points: i64,
    /// 连续签到天数。
    pub sign_count: i64,
    /// 服务端时间（毫秒），可能缺失。
    pub server_time_ms: Option<i64>,
}

/// 解析 `startSignIn` 的响应。
pub fn parse_start_sign_in(envelope: &Envelope) -> Result<SignInResult, Error> {
    let payload = ensure_ok(envelope)?;
    let payload = payload.ok_or_else(|| Error::Parse("startSignIn 响应没有 result".to_string()))?;
    Ok(SignInResult {
        today_sign_in: payload
            .get("todaySignIn")
            .and_then(Value::as_bool)
            .ok_or_else(|| Error::Parse("startSignIn 响应缺少 todaySignIn".to_string()))?,
        sign_in_points: require_i64(payload, "signInPoints")?,
        sign_count: require_i64(payload, "signCount")?,
        server_time_ms: payload.get("time").and_then(Value::as_i64),
    })
}

/// `receiveV3` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveResult {
    /// 本次领到的云朵数（`result.receive`）。
    ///
    /// H5 拿它做弹窗动画（`ee.receivePoints = n.receive`），所以它就是"这次进账多少"。
    /// 注意与 [`SignInStatus::sign_in_points`] 不是一回事：那个是"签到能拿多少"（还没到手）。
    pub receive: i64,
}

/// 解析 `receiveV3` 的响应。
///
/// 失败分支不走这里 —— `11029` / `614` / `615` 都由 [`ensure_ok`] 转成
/// [`Error::BadRequest`] 并附上 [`diagnose`] 的提示。
pub fn parse_receive_v3(envelope: &Envelope) -> Result<ReceiveResult, Error> {
    let payload = ensure_ok(envelope)?;
    let payload = payload.ok_or_else(|| Error::Parse("receiveV3 响应没有 result".to_string()))?;
    Ok(ReceiveResult {
        receive: require_i64(payload, "receive")?,
    })
}

/// `receiveRevivalReward` 的解析结果 —— **用掉一张卡，换回一批云朵**。
///
/// 实拍响应（`test/fixtures/http/复活.json`）：
///
/// ```json
/// {"code":0,"msg":"success",
///  "result":{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevivalReward {
    /// `1` = **复活卡**（随机复活上一年失效的云朵），`2` = **回馈卡**。
    ///
    /// H5 用 `1 == cardType ? "复活卡弹窗" : "回馈卡弹窗"` 分流，
    /// 两张卡连累计数的**文案单位都不同**（复活卡叫"累计复活AI豆"，回馈卡叫"累计获得奖励"）。
    /// 所以它是判据不是装饰 —— 缺了就没法说清"这次用掉的是什么卡"。
    pub card_type: i64,
    /// 本次随机复活到的云朵数。**这就是"这次进账多少"**（与 [`ReceiveResult::receive`] 同义）。
    pub reward_clouds: i64,
    /// 累计数（复活卡口径下是"累计复活AI豆"）。
    pub total_clouds: i64,
    /// 只影响一句文案（"下月再来可获得AI豆回馈卡！" vs "下月再来可继续获得AI豆复活卡！"）。
    ///
    /// 所以它是**附属字段**：服务端不给就给 `0`，不因此判整条响应失败 ——
    /// 判据是"缺了它还能不能回答本函数的问题"，它答不了任何问题，只是换个措辞。
    /// （与"主判据缺字段必须报错"成对，见 [`parse_sign_in_status`] 里 `receiveList` 的处理。）
    pub little_first_time: i64,
}

/// 解析 `receiveRevivalReward` 的响应。
///
/// ⚠ 这里**不**把 `total_clouds` 接进 `record_cloud` —— 它到底是不是"云朵余额"口径
/// **没有验证**：H5 文案说的是"复活卡把失效云朵**转换成 AI豆**"，
/// 累计文案也是"累计复活**AI豆**"。规划 §3.4 的来源③当初假设它是云朵余额，
/// 两者可能不是同一个池子。**等有 `hc1t` / `cloudStatus` 的实拍再定**，
/// 现在接进去只会把两个口径混进同一条曲线。
pub fn parse_revival_reward(envelope: &Envelope) -> Result<RevivalReward, Error> {
    let payload = ensure_ok(envelope)?;
    let payload =
        payload.ok_or_else(|| Error::Parse("receiveRevivalReward 响应没有 result".to_string()))?;
    Ok(RevivalReward {
        card_type: require_i64(payload, "cardType")?,
        reward_clouds: require_i64(payload, "rewardClouds")?,
        total_clouds: require_i64(payload, "totalClouds")?,
        // 附属字段：缺了给 0（见字段文档）。
        little_first_time: payload
            .get("littleFirstTime")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

/// `receiveTaskExpansion` 的解析结果 —— **月度任务奖励**（`cloudType == 1` 那条线）。
///
/// # 这条线是什么（两个来源对上之后的说法）
///
/// 「本月签到/做任务攒记录 → **下个月**结算 → 下个月来领」，且**要开着自动备份**。
/// 所以它在 `infoV3` 的 `receiveList[]` 里有两个状态，同一个特性的两个时点：
///
/// | `cloudType` | 时点 | 老路径的字段 | 动作 |
/// | --- | --- | --- | --- |
/// | `1` | **已到期、现在能领**（上月攒的那批）| `curMonthTaskRecordCount > 0` | **本接口** |
/// | `2` | 还没到期（本月攒的，下月才能领）| `nextMonthTaskRecordCount` | 只提示，不发请求 |
///
/// 旧项目 `mcloud-sign` 的 `_taskExpansion` 逐字是这么写的：
/// `if (curMonthTaskRecordCount > 0) { … receiveTaskExpansion(acceptDate) }`，
/// 然后 `if (nextMonthTaskRecordCount) { logger.info("下月可领取…") }` —— **一领一报**，
/// 与新 H5 的 `1 → 领` / `2 → toast("下月可领取")` 完全同构。
///
/// # 为什么字段是 `Option`
///
/// ⚠ **本仓库没有任何 `receiveTaskExpansion` 的实拍响应。** 唯一的字段线索来自旧项目
/// `mcloud-sign` 的调用点：`receiveData?.result?.cloudCount`，以及 H5 的错误分支
/// （`1302` 未开启备份、`500` 已过领取有效期）。
///
/// 所以这里**不**用 [`require_i64`]：那个约定是"缺了主判据就判整条响应失败"，
/// 而这条接口的主判据是**成败**（由 `code` 决定，`ensure_ok` 已经管了），
/// `cloudCount` 只回答"领到多少"。没有实拍还硬要求字段，等于把猜测变成断言 ——
/// 第一次真跑就会以一个看不懂的解析错误告终。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveTaskExpansion {
    /// `result.cloudCount` —— 本次领到多少云朵（豆）。
    ///
    /// `None` = 上游没给（**不是 0**）：0 是"领到 0 朵"，`None` 是"没说"。
    pub cloud_count: Option<i64>,
}

/// 解析 `receiveTaskExpansion` 的响应。
///
/// ⚠ **没有 `result` 不算失败。** 两个客户端都只拿 `code` 判成败：
/// H5 的 `.then` 里只比 `e.data.code`（`0` / `1302` / `500`），
/// 旧项目 `mcloud-sign` 写的是 `receiveData?.result?.cloudCount` —— `?.` 就是"没有也无所谓"。
///
/// 这里曾经把"没有 result"报成 [`Error::Parse`]，那是**比客户端更严**：
/// 真跑起来会以一句解析错误盖掉上游真正想说的话（比如 `1302` 未开备份）。
/// 现在只在拿不到 `cloudCount` 时给 `None`。
pub fn parse_receive_task_expansion(envelope: &Envelope) -> Result<ReceiveTaskExpansion, Error> {
    let payload = ensure_ok(envelope)?;
    Ok(ReceiveTaskExpansion {
        cloud_count: payload
            .as_ref()
            .and_then(|payload| payload.get("cloudCount"))
            .and_then(Value::as_i64),
    })
}

/// [`multiple_request`] 的解析结果 —— **签到翻倍**这一次额外领到多少豆。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleResult {
    /// `result.cloudCount` —— 本次额外获得的豆数。
    ///
    /// H5 逐字读它：`e.data.result.cloudCount` → 「额外获得 {cloudCount} 豆」。
    ///
    /// # 为什么是 `Option`（这条**没有实拍响应**）
    ///
    /// 字段名来自**当前版 H5 的调用点**（比旧项目的转述强），但**没有实拍**：
    /// 两个账号的 `curMonthBackupSignAccept` 都已经是 `true`（本月已领过），
    /// 复现不出来。所以按 [`ReceiveTaskExpansion::cloud_count`] 的同一约定 ——
    /// **缺了不判整条失败**。领没领到由 `code` 决定（`ensure_ok` 已经管了），
    /// 这个数只回答"领到多少"；硬要求它会把一次**成功**的领取变成解析错误。
    pub cloud_count: Option<i64>,
    /// `result.multiple` —— **倍数**（`N` 倍）。
    ///
    /// 旧项目逐字读了它并拼成一句日志：
    /// `ctx.logger.success("成功获得", multiple, "倍云朵，共计", cloudCount)`
    /// —— 即 **`multiple` 是倍数、`cloudCount` 是这次的合计**，两者不是一个量。
    /// 只报 `cloudCount` 会让"翻了几倍"这个信息丢掉。
    ///
    /// 与 [`Self::cloud_count`] 同档：**没有实拍**、**缺了不判失败**。
    pub multiple: Option<i64>,
}

/// 解析 `multiple` 的响应。
///
/// `result` 缺失**不判失败** —— 理由同 [`parse_receive_task_expansion`]：
/// 两个客户端都只拿 `code` 判成败，`result` 只是"领到多少"的补充。
pub fn parse_multiple(envelope: &Envelope) -> Result<MultipleResult, Error> {
    let payload = ensure_ok(envelope)?;
    let field = |key: &str| {
        payload
            .as_ref()
            .and_then(|payload| payload.get(key))
            .and_then(Value::as_i64)
    };
    Ok(MultipleResult {
        cloud_count: field("cloudCount"),
        multiple: field("multiple"),
    })
}

/// `cloudRecordV2` 的解析结果 —— 一页流水。
///
/// # 为什么条目是**原样的 `Value`**
///
/// H5 拿到 `records` 之后**直接丢给模板渲染**，从没读过条目里的任何具体字段名。
/// ⇒ 本仓库**没有任何 `records[]` 的实拍**（也没人抓过）。
///
/// 所以这里**不声明字段**：编一个 `amount` / `createTime` 出来，等于把猜测变成断言 ——
/// 第一次真跑就会以一个看不懂的解析错误收场，而原因只是"名字猜错了"。
/// 原样交出去，让调用方看着办。
///
/// 真正**有依据**的只有下面两个字段（H5 逐字读了它们）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRecords {
    /// `result.records` —— 本页流水条目，**原样**。
    pub records: Vec<Value>,
    /// `result.total` —— ⚠ **条目总数（分页用），不是金额**。
    ///
    /// 判据是 H5 的翻页逻辑：`records.length + … >= total` 就判"加载完了" ——
    /// 能这么比的只有条数。
    pub total: Option<i64>,
}

/// 解析 `cloudRecordV2` 的响应。
///
/// ⚠ 没有 `records` 时给**空数组**而不是报错：这是审计接口，
/// "这一档没有记录"是正常答案（例如改名前的账本本来就只到 2026-07-30）。
pub fn parse_cloud_records(envelope: &Envelope) -> Result<CloudRecords, Error> {
    let payload = ensure_ok(envelope)?;
    let records = payload
        .as_ref()
        .and_then(|payload| payload.get("records"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(CloudRecords {
        records,
        total: payload
            .as_ref()
            .and_then(|payload| payload.get("total"))
            .and_then(Value::as_i64),
    })
}

/// `record` 的解析结果 —— 一页**奖品记录**（见 [`prize_record_request`]）。
///
/// # 字段依据（**有实拍**，2026-09-19，账号 159）
///
/// 「待领取」页签回了一条。实拍条目长这样（**已去掉账号标识字段**）：
///
/// ```json
/// {"flag":1,"groupId":0,"type":1057,"source":0,"belonger":0,"device":0,
///  "prizeId":260630014,"prizeName":"移动云盘1000万tokens叠加包",
///  "insertTime":"2026-09-16T11:12:54.000+00:00",
///  "expireTime":"2026-09-30T15:59:59.000+00:00",
///  "updateTime":"2026-09-16T11:12:54.000+00:00",
///  "market":"sign_in_3_ex","marketName":"sign_in_3_ex",
///  "oid":1270932714,"orderType":0,"prizeSectionStatus":1,
///  "reasons":"260630014","uorder":400,
///  "linkUrl":"{\"saleType\":103,\"days\":\"0\",\"serviceId\":\"…\"}"}
/// ```
///
/// ⇒ 所以这里**声明**这几个字段。但仍保持 `Option`：
/// 缺一个就少一行字，不该把整条查询打成解析错误
/// （**空页才是最常见的答案** —— 三个页签里总有两个是空的）。
///
/// ⚠ 条目里**带账号标识**（`userDomainId`，那个 19 位值）。所以本仓库
/// **不留原样条目**：黄金向量只保留上面这几个字段，见
/// `test/golden/app-signin-vectors.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrizeRecordItem {
    /// `prizeName` —— 奖品名。列表主文案。
    pub prize_name: Option<String>,
    /// `insertTime` —— **中奖时间**。H5 用 dayjs 格式化成 `YYYY-MM-DD`。
    pub insert_time: Option<String>,
    /// `expireTime` —— **过期时间**。
    ///
    /// ⚠ 这里曾经写着"当前版 H5 没有这个字段"——**错了，已按实拍更正**。
    /// H5 的**列表项模板**确实只显示「中奖时间」（`insertTime`），
    /// 但响应里 `expireTime` **一直在**：实拍是 `2026-09-30T15:59:59.000+00:00`
    /// （中奖后 14 天）。「模板没渲染它」**不等于**「响应没有它」——
    /// 旧项目 `printPendingPrizes` 读这个字段是对的，当时是我把两件事混成了一件。
    pub expire_time: Option<String>,
    /// `flag` —— **`1` = 待领取**。旧项目 `printPendingPrizes` 就是按它过滤的。
    ///
    /// 页签（请求参数 `type`）已经筛过一遍，所以它多半恒为 `1`；
    /// 留着是因为它**是旧项目那条过滤的判据**，丢了就没人能解释"为什么这么筛"。
    pub flag: Option<i64>,
    /// `prizeId` —— 奖品 id。点「去领取」时要回传给奖品详情接口。
    pub prize_id: Option<i64>,
    /// `oid` —— 中奖记录 id（H5 里叫 `drawRecode`，是**同一个值换了个名字**）。
    ///
    /// 实拍是**数字**（`1270932714`），而 H5 把它拼进 URL 查询串 ——
    /// 两种形态都能用，所以这里统一成字符串。
    pub oid: Option<String>,
    /// `marketName` —— 该奖品属于哪个活动（H5 里叫 `marketId`）。实拍 `sign_in_3_ex`。
    pub market_name: Option<String>,
    /// `type` —— 奖品类型号（实拍 `1057`）。
    ///
    /// ⚠ 与请求参数那个"页签 `type`"（`1/2/10`）**同名不同物**，别混。
    pub prize_type: Option<i64>,
    /// 本条**原样**保留 —— 上面那几个字段是"能认出来的"，其余不替上游编名字。
    ///
    /// ⚠ 它含账号标识，**别直接落进文档或黄金向量**。
    pub raw: Value,
}

/// 一页奖品记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrizeRecords {
    /// `result.records` —— 本页条目。
    pub records: Vec<PrizeRecordItem>,
}

/// 解析 `record` 的响应。
///
/// ⚠ 没有 `records` 时给**空数组**而不是报错：三个页签里总有两个是空的
/// （待领取/已领取/已过期），"这一档没有记录"正是最常见的**正常**答案。
pub fn parse_prize_records(envelope: &Envelope) -> Result<PrizeRecords, Error> {
    let payload = ensure_ok(envelope)?;
    let items = payload
        .as_ref()
        .and_then(|payload| payload.get("records"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let str_field =
        |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
    let records = items
        .into_iter()
        .map(|raw| PrizeRecordItem {
            prize_name: str_field(&raw, "prizeName"),
            insert_time: str_field(&raw, "insertTime"),
            expire_time: str_field(&raw, "expireTime"),
            flag: raw.get("flag").and_then(Value::as_i64),
            prize_id: raw.get("prizeId").and_then(Value::as_i64),
            // `oid` 可能是数字也可能是字符串 —— H5 直接把它塞进 URL 参数，
            // 两种形态都能用，所以这里两种都收，统一成字符串。
            oid: raw.get("oid").and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            }),
            market_name: str_field(&raw, "marketName"),
            prize_type: raw.get("type").and_then(Value::as_i64),
            raw,
        })
        .collect();
    Ok(PrizeRecords { records })
}

/// `backupgift/info` 的解析结果 —— **备份好礼**。
///
/// # 三态（旧项目 `backupGiftTask` 的 `switch (state)` 逐字）
///
/// | `state` | 含义 | 动作 |
/// | --- | --- | --- |
/// | `-1` | **未开启备份** | 不领，提示去 App 开 |
/// | `1` | 本月**已领过** | 不领 |
/// | `0` | **可领** | 发 [`backup_gift_receive_request`] |
///
/// ⚠ `state` 是**必填**（用 [`require_i64`]）：它就是这条接口的主判据 ——
/// 缺了它就无法回答"该不该领"，而这正是本结构存在的唯一理由。
/// （与 [`MultipleResult::cloud_count`] 那种"只回答领到多少"的附属字段成对，
/// 见 [`parse_sign_in_status`] 里对两者的说明。）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGift {
    /// `result.state` —— 见上表。
    pub state: i64,
    /// `result.curMonth` —— 本月**有没有**备份好礼。
    ///
    /// 旧项目先判它：`if (!curMonth) { "无备份礼物可领"; return; }` ——
    /// 即它比 `state` **更靠前**，`false` 时连 `state` 都不用看。
    pub cur_month: bool,
}

/// 解析 `backupgift/info` 的响应。
pub fn parse_backup_gift(envelope: &Envelope) -> Result<BackupGift, Error> {
    let payload = ensure_ok(envelope)?;
    let payload =
        payload.ok_or_else(|| Error::Parse("backupgift/info 响应没有 result".to_string()))?;
    Ok(BackupGift {
        state: require_i64(payload, "state")?,
        // `curMonth` 旧项目当布尔用（`if (!curMonth)`），所以这里按"真值"收：
        // 布尔、数字、字符串三种形态都认，缺了当 `false`。
        cur_month: match payload.get("curMonth") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_i64().is_some_and(|value| value != 0),
            Some(Value::String(text)) => !text.is_empty() && text != "0" && text != "false",
            _ => false,
        },
    })
}

/// `backupgift/receive` 的解析结果 —— 这次领到多少豆。
///
/// # 为什么 `result` 直接是个数字
///
/// 旧项目的类型就写着 `result: number`，调用点读 `typeof receiveData?.result === "number"`
/// 再把它当豆数打印。⇒ 这条的 `result` **不是对象**，别按对象去取字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGiftReceive {
    /// `result` —— 领到的豆数。上游没给数字时是 `None`（≠ 0）。
    pub received: Option<i64>,
}

/// 解析 `backupgift/receive` 的响应。
///
/// `result` 缺失**不判失败** —— 同 [`parse_multiple`]：成败由 `code` 决定。
pub fn parse_backup_gift_receive(envelope: &Envelope) -> Result<BackupGiftReceive, Error> {
    let payload = ensure_ok(envelope)?;
    Ok(BackupGiftReceive {
        received: payload.and_then(|value| value.as_i64()),
    })
}

/// 解析 `taskListV3` 的响应，返回任务数组。
///
/// 这一条的 `result` 是**数组**（不是对象）—— 所以走 `ensure_ok` 后拿到的是 `Value::Array`。
pub fn parse_task_list(envelope: &Envelope) -> Result<Vec<Value>, Error> {
    let payload = ensure_ok(envelope)?;
    match payload {
        Some(Value::Array(items)) => Ok(items.clone()),
        _ => Err(Error::Parse(
            "taskListV3 的 result 不是数组（该接口不带信封字段，只有 result）".to_string(),
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送
// ─────────────────────────────────────────────────────────────────────────────

impl<C: mcloud_core::HttpClient> crate::Upstream<'_, C> {
    /// 换 SSO token（`querySpecTokenV2`）。
    pub async fn app_query_spec_token(
        &self,
        profile: &AppProfile,
        timestamp: i64,
        authorization: &str,
        to_source_id: &str,
    ) -> Result<String, Error> {
        let body = query_spec_token_body(to_source_id);
        let envelope = self
            .call(query_spec_token_request(
                profile,
                timestamp,
                authorization,
                &body,
            )?)
            .await?;
        parse_sso_token(&envelope)
    }

    /// 取服务端毫秒时间戳（签名要用）。
    pub async fn app_h5_timestamp(&self) -> Result<String, Error> {
        let envelope = self.call(h5_timestamp_request()).await?;
        parse_server_timestamp(&envelope)
    }

    /// 建会话（`tyrzLogin`，带签名）。
    pub async fn app_h5_login(
        &self,
        sign: &H5Signature,
        referer: &str,
        sso_token: &str,
    ) -> Result<H5Login, Error> {
        let body = tyrz_login_body(sso_token);
        let envelope = self.call(tyrz_login_request(sign, referer, &body)?).await?;
        parse_jwt_token(&envelope)
    }

    /// 查签到状态（`infoV3`）。
    pub async fn app_sign_in_status(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<SignInStatus, Error> {
        let envelope = self.call(info_v3_request(jwt, referer)).await?;
        parse_sign_in_status(&envelope)
    }

    /// `infoV3` 的**原始响应体**，不做任何解析。
    ///
    /// # 为什么留一个不解析的入口
    ///
    /// [`SignInStatus`] 只保留我们**认识的**字段。协议还在摸（`receiveList[]`
    /// 的条目里到底还有哪些字段、顺序有没有含义），"只看得见的字段"会让排查
    /// 变成对着残缺数据猜 —— 这个入口就是为了那种时候。
    ///
    /// ⚠ 它不是给业务用的：调用方拿到的是上游原文，**字段随上游变**，
    /// 别把它接进任何判断里。
    pub async fn app_sign_in_status_raw(&self, jwt: &str, referer: &str) -> Result<Value, Error> {
        let envelope = self.call(info_v3_request(jwt, referer)).await?;
        Ok(envelope.raw().clone())
    }

    /// 执行签到（`startSignIn`）。
    pub async fn app_start_sign_in(
        &self,
        jwt: &str,
        referer: &str,
        device_id: Option<&str>,
    ) -> Result<SignInResult, Error> {
        let envelope = self
            .call(start_sign_in_request(jwt, referer, device_id))
            .await?;
        parse_start_sign_in(&envelope)
    }

    /// 任务列表（`taskListV3`）。
    pub async fn app_task_list_v3(
        &self,
        jwt: &str,
        referer: &str,
        client: u8,
    ) -> Result<Vec<Value>, Error> {
        let envelope = self
            .call(task_list_v3_request(jwt, referer, client)?)
            .await?;
        parse_task_list(&envelope)
    }

    /// 领云朵（`receiveV3`）。
    ///
    /// `device_id` 是必填的 —— 见 [`receive_v3_request`] 的说明。
    pub async fn app_receive_v3(
        &self,
        jwt: &str,
        referer: &str,
        cloud_id: &str,
        cloud_type: Option<i64>,
        device_id: &str,
    ) -> Result<ReceiveResult, Error> {
        let envelope = self
            .call(receive_v3_request(
                jwt, referer, cloud_id, cloud_type, device_id,
            )?)
            .await?;
        parse_receive_v3(&envelope)
    }

    /// 查豆的收支流水（`cloudRecordV2`）。**只读**，用于对账 / 排障。
    ///
    /// `ai_dou` 取 [`RECORD_BOOK_CLOUD`] / [`RECORD_BOOK_BEAN`]，
    /// `filter` 取 [`RECORD_FILTER_ALL`] / [`RECORD_FILTER_EARNED`] / [`RECORD_FILTER_SPENT`]。
    pub async fn app_cloud_records(
        &self,
        jwt: &str,
        referer: &str,
        ai_dou: i64,
        filter: i64,
        page_number: i64,
        page_size: i64,
    ) -> Result<CloudRecords, Error> {
        let envelope = self
            .call(cloud_record_v2_request(
                jwt,
                referer,
                ai_dou,
                filter,
                page_number,
                page_size,
            ))
            .await?;
        parse_cloud_records(&envelope)
    }

    /// 用复活卡 / 回馈卡（`receiveRevivalReward`）。
    ///
    /// `device_id` 可选 —— 两个证据不一致，见 [`receive_revival_reward_request`]。
    pub async fn app_receive_revival_reward(
        &self,
        jwt: &str,
        referer: &str,
        device_id: Option<&str>,
    ) -> Result<RevivalReward, Error> {
        let envelope = self
            .call(receive_revival_reward_request(jwt, referer, device_id)?)
            .await?;
        parse_revival_reward(&envelope)
    }

    /// 延期领取（`receiveTaskExpansion`）—— `cloudType == 1` 那一条路。
    ///
    /// `accept_date` 取自 `receiveList[]` 里**同一条**的 `acceptDate`。
    pub async fn app_receive_task_expansion(
        &self,
        jwt: &str,
        referer: &str,
        accept_date: Option<&str>,
    ) -> Result<ReceiveTaskExpansion, Error> {
        let envelope = self
            .call(receive_task_expansion_request(jwt, referer, accept_date))
            .await?;
        parse_receive_task_expansion(&envelope)
    }

    /// 领**签到翻倍**（`multiple`）。**写请求**，每月只有一次机会。
    ///
    /// ⚠ 门槛**不在这里判** —— 见 [`multiple_request`] 的文档。调用方要先看
    /// `infoV3` 的 `curMonthBackup` / `curMonthBackupSignAccept`
    /// （可执行版本：`mcloud_app::signin::plan_multiple`）。
    pub async fn app_claim_multiple(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<MultipleResult, Error> {
        let envelope = self.call(multiple_request(jwt, referer)).await?;
        parse_multiple(&envelope)
    }

    /// 查**奖品记录**（`record`）。只读。
    pub async fn app_prize_records(
        &self,
        jwt: &str,
        referer: &str,
        marketname: &str,
        status: i64,
        page_number: i64,
        page_size: i64,
    ) -> Result<PrizeRecords, Error> {
        let envelope = self
            .call(prize_record_request(
                jwt,
                referer,
                marketname,
                status,
                page_number,
                page_size,
            ))
            .await?;
        parse_prize_records(&envelope)
    }

    /// 查**备份好礼**状态（`backupgift/info`）。只读。
    pub async fn app_backup_gift(&self, jwt: &str, referer: &str) -> Result<BackupGift, Error> {
        let envelope = self.call(backup_gift_info_request(jwt, referer)).await?;
        parse_backup_gift(&envelope)
    }

    /// 领**备份好礼**（`backupgift/receive`）。**写请求**，每月一次。
    ///
    /// ⚠ 门槛**不在这里判** —— 调用方要先看 [`Self::app_backup_gift`] 的
    /// [`BackupGift::state`] 与 [`BackupGift::cur_month`]。
    /// 可执行版本：`mcloud_app::signin::plan_backup_gift`。
    pub async fn app_receive_backup_gift(
        &self,
        jwt: &str,
        referer: &str,
    ) -> Result<BackupGiftReceive, Error> {
        let envelope = self.call(backup_gift_receive_request(jwt, referer)).await?;
        parse_backup_gift_receive(&envelope)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试：黄金向量 + 逐字段断言
// ─────────────────────────────────────────────────────────────────────────────
//
// 这里**只测纯函数**（请求构造 + 响应解析）—— 它们承载了本模块的全部逻辑。
// `impl Upstream` 那几个 `app_*` 方法是三行包装（建请求 → `call` → 解析），
// 而 `mcloud-upstream` 的依赖树里**没有 async runtime**（这是刻意的：
// 本层禁止 IO，能跑异步的只有 `mcloud-io`），所以包装层由 `mcloud-app`
// 的编排测试覆盖，不在这里硬造一个 runtime。

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::Random;
    // `sign_salt` 只在测试里用（把常量与向量、与 core 三者钉在一起），
    // 生产代码走的是 `generate_signature`，所以它不进模块顶部的 import。
    use mcloud_core::sign_salt;

    /// 黄金向量（真实抓包 + 真实响应）。
    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../test/golden/app-signin-vectors.json"))
            .expect("黄金向量文件必须是合法 JSON")
    }

    /// 确定性随机源：把给定字节循环填出去。
    ///
    /// 用它而不是真实 CSPRNG，是为了让 [`sign_h5_request`] 可复现 ——
    /// 被测的是"两个 uuid + 一次 md5"这个形状，不是具体取值。
    struct SeqRandom {
        bytes: Vec<u8>,
        at: usize,
    }

    impl SeqRandom {
        fn new(seed: u8) -> Self {
            Self {
                bytes: (0..64).map(|i| seed.wrapping_add(i as u8)).collect(),
                at: 0,
            }
        }
    }

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for b in dst.iter_mut() {
                *b = self.bytes[self.at % self.bytes.len()];
                self.at += 1;
            }
        }
    }

    /// 向量里的那个 `x-signature` 实例。
    fn captured_signature() -> Value {
        vectors()["h5Signature"][0].clone()
    }

    fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    // ───────────────────────── 签名 ─────────────────────────

    #[test]
    fn h5_salt_is_the_pc_signing_salt() {
        // 签到页的盐**就是** PC 端签名用的第二个盐 —— 所以能直接复用
        // `generate_signature`。这条断言把常量与向量、与 core 三者钉在一起：
        // `SIGN_SALT_MAP` 哪天变了，这里先炸，而不是"签到忽然回 401"。
        let v = vectors();
        let expected = v["constants"]["H5_SIGN_SALT"].as_str().unwrap();
        assert_eq!(sign_salt(H5_SIGN_SALT_KEY), expected);
        assert_eq!(expected, "sekaMdYYLIZfbCfm");
    }

    #[test]
    fn real_capture_signature_is_reproduced() {
        // 最有价值的一条：期望值是**抓包里服务端接受的那个签名**，不是算出来的。
        // 它同时锁住"盐 = saltKey 2"、"末段 other 为空串"、"拼串顺序"三件事。
        let v = captured_signature();
        let sign = sign_h5_request_with(
            v["requestId"].as_str().unwrap(),
            v["timestamp"].as_str().unwrap(),
            v["nonce"].as_str().unwrap(),
        );
        assert_eq!(sign.signature, v["expectMd5"].as_str().unwrap());
        assert_eq!(v["saltKey"].as_u64().unwrap() as u8, H5_SIGN_SALT_KEY);
        assert_eq!(v["other"].as_str().unwrap(), "");
    }

    #[test]
    fn signature_uses_uuids_and_the_injected_timestamp() {
        let mut rng = SeqRandom::new(7);
        let sign = sign_h5_request(&mut rng, "1789800128813");

        assert_eq!(sign.timestamp, "1789800128813", "时间戳必须原样透传");
        for (name, value) in [("request_id", &sign.request_id), ("nonce", &sign.nonce)] {
            assert_eq!(value.len(), 36, "{name} 应是 36 字符的 UUID");
            assert_eq!(value.matches('-').count(), 4, "{name} 应有 4 个连字符");
        }
        assert_ne!(
            sign.request_id, sign.nonce,
            "两个 UUID 不能是同一个 —— 它们是两次独立取随机"
        );
        assert_eq!(sign.signature.len(), 32);
        // 签名确实是把那两个 uuid 喂进算式得到的，不是别的东西
        assert_eq!(
            sign.signature,
            generate_signature(
                &sign.request_id,
                "1789800128813",
                &sign.nonce,
                H5_SIGN_SALT_KEY,
                ""
            )
        );
    }

    // ───────────────────────── 第 1 段：SSO token ─────────────────────────

    #[test]
    fn query_spec_token_body_is_exactly_to_source_id() {
        assert_eq!(
            query_spec_token_body(TO_SOURCE_ID),
            serde_json::json!({"toSourceId": "001005"})
        );
    }

    #[test]
    fn query_spec_token_request_keeps_app_headers_and_authorization() {
        let profile = AppProfile::new(crate::app_client::AppDevice::ipad(
            "AAA00000000000000000000000000000",
            "AAA000000000",
        ))
        .with_user_domain_id("1990000000000000001");
        let body = query_spec_token_body(TO_SOURCE_ID);
        let request =
            query_spec_token_request(&profile, 1789800128813000, "Basic SYNTH", &body).unwrap();

        assert_eq!(request.method, mcloud_core::HttpMethod::Post);
        assert_eq!(
            request.url,
            "https://user-njs.yun.139.com/user/querySpecTokenV2"
        );
        // 与 App 登录链路同一套头（复用 `AppProfile::headers`），加一个 Authorization
        assert_eq!(header(&request, "Authorization"), Some("Basic SYNTH"));
        assert_eq!(header(&request, "x-yun-uni"), Some("1990000000000000001"));
        assert_eq!(
            header(&request, "x-yun-user-timestamp"),
            Some("1789800128813000")
        );
        assert_eq!(
            header(&request, "Content-Type"),
            Some("application/json;charset=UTF-8"),
            "抓包收的是带 charset 的那个，且只能有一个同名头"
        );
        assert_eq!(
            request
                .headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .count(),
            1,
            "json_body 会自己塞一个 content-type，必须被覆盖掉而不是叠成两个"
        );
    }

    // ───────────────────────── 第 2 段：时间戳 ─────────────────────────

    #[test]
    fn h5_timestamp_request_is_bare_get() {
        let request = h5_timestamp_request();
        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert_eq!(request.url, "https://m.mcloud.139.com/ycloud/api/timestamp");
        assert!(request.body.is_none(), "这条是无鉴权裸 GET");
        assert!(header(&request, "jwttoken").is_none());
    }

    // ───────────────────────── 第 3 段：建会话 ─────────────────────────

    #[test]
    fn tyrz_login_body_matches_capture() {
        assert_eq!(
            tyrz_login_body("YZsidssolgSYNTH000000000000000000000000000"),
            serde_json::json!({
                "token": "YZsidssolgSYNTH000000000000000000000000000",
                "openAccount": 0,
                "marketName": "sign_in_3",
                "sourceId": "1002",
            })
        );
        // `openAccount` 是**数字** 0，不是字符串 —— 抓包原样如此
        assert!(tyrz_login_body("x")["openAccount"].is_i64());
    }

    #[test]
    fn tyrz_login_request_carries_all_four_sign_headers() {
        let sign = sign_h5_request_with("rid-1", "1789800128813", "nonce-1");
        let body = tyrz_login_body("sso");
        let referer = signin_page_url("sso");
        let request = tyrz_login_request(&sign, &referer, &body).unwrap();

        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin"
        );
        assert_eq!(header(&request, "x-request-id"), Some("rid-1"));
        assert_eq!(header(&request, "x-timestamp"), Some("1789800128813"));
        assert_eq!(header(&request, "x-nonce"), Some("nonce-1"));
        assert_eq!(
            header(&request, "x-signature"),
            Some(sign.signature.as_str())
        );
        assert_eq!(header(&request, "Origin"), Some("https://m.mcloud.139.com"));
        assert_eq!(header(&request, "activityid"), Some(MARKET_NAME));
        assert_eq!(header(&request, "appversion"), Some(H5_APP_VERSION));
        // 还没建会话 —— 这条**不带** jwttoken（带了反而是错的）
        assert!(
            header(&request, "jwttoken").is_none(),
            "tyrzLogin 是建会话那一步，此时还没有 jwt"
        );
    }

    #[test]
    fn signin_page_url_is_the_referer_shape_from_capture() {
        // 期望值**逐字抄自抓包**（HAR 里 128 处 `referer` 头都是这个形状），
        // 不是从实现反推的。踩过的坑：初版漏了末尾的 `&targetSourceId=001005`，
        // 而当时的断言正是照着实现写的，于是"实现漏了、测试也跟着漏"。
        let url = signin_page_url("SSO");
        assert_eq!(
            url,
            "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin\
             &sourceid=1002&enableShare=1\
             &appID=cy62d5a5e8373a272aeca467accad61722c51cfd48&token=SSO\
             &targetSourceId=001005"
        );
        // `targetSourceId` 必须排在 `token` **之后**：抓包里就是这个顺序。
        //
        // 本链路里 URL 的 `targetSourceId` 与 `querySpecTokenV2` 的 `toSourceId`
        // 恰好同值（都是 `001005`），所以两处都由 `TO_SOURCE_ID` 供 —— 改一处不会漏另一处。
        // ⚠ 但**别把"同值"当成上游的约束**：PC 客户端 `openAIWebChat` 就是
        // `toSourceId=001248` 而 URL 里写 `targetsourceid=001005`（`docs/capture/apiServer.js` L954/L969）。
        // 这条断言钉的是**抓包这一份实况**，不是一条协议规则。
        assert!(url.ends_with(&format!("&targetSourceId={TO_SOURCE_ID}")));
    }

    // ───────────────────────── 签到主体 ─────────────────────────

    #[test]
    fn info_v3_request_needs_jwt_and_uses_client_app() {
        let request = info_v3_request("JWT", "https://referer/");
        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/infoV3?client=app"
        );
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        assert_eq!(header(&request, "Referer"), Some("https://referer/"));
        assert_eq!(header(&request, "activityid"), Some(MARKET_NAME));
        assert_eq!(header(&request, "appversion"), Some(H5_APP_VERSION));
    }

    #[test]
    fn start_sign_in_without_device_id_is_just_client_app() {
        // 实测形态：不带 deviceId 也能签上（账号 B 验证过），
        // 而且此时**不发** `isdeviceid` 头 —— 发一个空的等于自相矛盾。
        let request = start_sign_in_request("JWT", "r", None);
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app"
        );
        assert_eq!(header(&request, "isdeviceid"), None);
        assert_eq!(header(&request, "showloading"), Some("false"));
    }

    #[test]
    fn start_sign_in_with_device_id_percent_encodes_base64() {
        // `deviceId` 是 base64，含 `+` `/` `=`。`+` 在查询串里表示空格 ——
        // 不编码就会被服务端解成另一个值，而且**不报错**（只是行为不对）。
        let raw = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0A=";
        let request = start_sign_in_request("JWT", "r", Some(raw));
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app\
             &deviceId=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0%2BP0A%3D"
        );
        assert_eq!(header(&request, "isdeviceid"), Some("true"));
        assert!(!request.url.contains('+'), "`+` 必须被编码成 %2B");
    }

    #[test]
    fn percent_encode_leaves_unreserved_alone() {
        assert_eq!(percent_encode("aZ0-._~"), "aZ0-._~");
        assert_eq!(percent_encode("+/="), "%2B%2F%3D");
        assert_eq!(percent_encode("A B"), "A%20B");
    }

    #[test]
    fn task_list_v3_body_matches_capture() {
        let request = task_list_v3_request("JWT", "r", 1).unwrap();
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/task/taskListV3"
        );
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        assert_eq!(
            header(&request, "Content-Type"),
            Some("application/json;charset=UTF-8")
        );
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "marketname": "sign_in_3",
                "client": 1,
                "clientVersion": "12.6.0",
            })
        );
        // `client` 是**数字**（与 startSignIn 的 `client=app` 字符串不是一回事）
        assert!(body["client"].is_i64());
    }

    #[test]
    fn do_task_post_requires_a_device_id() {
        let request = do_task_post_request("JWT", "r", "DEV").unwrap();
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"client": "app", "deviceId": "DEV"})
        );
        assert_eq!(header(&request, "isdeviceid"), Some("true"));
        // 这里 `device_id` 是 `&str`（必填）—— 漏发是编译错误，不是运行时报错
    }

    // ───────────────────────── 领取（`receiveV3`） ─────────────────────────

    #[test]
    fn receive_v3_carries_the_h5_body_and_the_isdeviceid_header() {
        let request = receive_v3_request("JWT", "r", "REC-1", Some(2), "DEVICE-BLOB").unwrap();

        assert_eq!(request.method, mcloud_core::HttpMethod::Post);
        assert!(request.url.ends_with("/ycloud/signin/page/receiveV3"));
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        // 逐字对照 H5 调用点：`{client, cloudId: e.recordId, cloudType: e.cloudType}`
        // 外加拦截器按 `isDeviceId: true` 塞进 body 的 `deviceId`（POST 走 body，不走 query）。
        assert_eq!(
            body,
            serde_json::json!({
                "client": "app",
                "cloudId": "REC-1",
                "cloudType": 2,
                "deviceId": "DEVICE-BLOB",
            })
        );
        assert_eq!(header(&request, "isdeviceid"), Some("true"));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    }

    /// H5 是 `e.cloudType ?? ""` —— 缺省给的是**空串**，不是 `0`。
    ///
    /// 这条不是为了好看：`0` 是个**合法取值**（另一种可领类型），
    /// 把"上游没给"写成 `0` 等于替上游编了一个类型。
    #[test]
    fn receive_v3_omits_a_missing_cloud_type_as_an_empty_string_not_zero() {
        let request = receive_v3_request("JWT", "r", "", None, "DEV").unwrap();
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();

        assert_eq!(body["cloudType"], serde_json::json!(""));
        assert_ne!(body["cloudType"], serde_json::json!(0));
        assert_eq!(body["cloudId"], serde_json::json!(""));
    }

    #[test]
    fn parse_receive_v3_reads_the_received_amount() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"receive": 234}
        }));
        assert_eq!(parse_receive_v3(&envelope).unwrap().receive, 234);
    }

    #[test]
    fn parse_receive_v3_turns_upstream_failures_into_bad_request() {
        // `614` / `615` 是 H5 里那两句「领取失败」；`11029` 是「活动太火爆」。
        // 三者都必须走 `ensure_ok` → `BadRequest`，而不是被当成"领到了 0 朵"。
        for code in [
            CODE_RECEIVE_TOO_BUSY,
            CODE_RECEIVE_FAILED,
            CODE_RECEIVE_FAILED_ALT,
        ] {
            let envelope = Envelope::new(serde_json::json!({"code": code, "msg": "x"}));
            let err = parse_receive_v3(&envelope).unwrap_err();
            assert!(
                matches!(err, Error::BadRequest(_)),
                "码 {code} 应是业务失败：{err}"
            );
            assert!(
                format!("{err}").contains("领取失败") || format!("{err}").contains("活动太火爆"),
                "码 {code} 应带上排查提示：{err}"
            );
        }
    }

    #[test]
    fn missing_receive_amount_is_a_parse_error_not_a_silent_zero() {
        // 与 `beforeTotal` 同一条理由：缺字段静默成 0 会让"没领到"看起来像"领到了 0 朵"。
        let envelope =
            Envelope::new(serde_json::json!({"code": 0, "msg": "success", "result": {}}));
        let err = parse_receive_v3(&envelope).unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "应是解析错误：{err}");
        assert!(format!("{err}").contains("receive"));
    }

    // ────────────────────── 复活卡 / 回馈卡（`receiveRevivalReward`） ──────────────────────

    #[test]
    fn receive_revival_reward_posts_an_empty_object_body() {
        let request = receive_revival_reward_request("JWT", "r", None).unwrap();

        assert_eq!(request.method, mcloud_core::HttpMethod::Post);
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/receiveRevivalReward"
        );
        // H5 是 `receiveRevivalReward({})` ⇒ `JSON.stringify({})` == `"{}"`（2 字节）。
        // 老抓包里的 `content-length: 2` 正是这个。
        assert_eq!(request.body.as_deref(), Some(&b"{}"[..]));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        assert_eq!(
            header(&request, "Content-Type"),
            Some("application/json;charset=UTF-8")
        );
    }

    /// 同页其余接口几乎都是 `showLoading: true`，只有这一条是 `false`
    /// （H5 要自己画卡片弹窗，不想被全局 loading 遮住）。照抄，别"统一成 true"。
    #[test]
    fn receive_revival_reward_asks_for_no_global_loading() {
        let request = receive_revival_reward_request("JWT", "r", None).unwrap();
        assert_eq!(header(&request, "showloading"), Some("false"));

        // 对照组：同一个辅助函数，别的接口拿到的就是 `true`。
        assert_eq!(
            header(&info_v3_request("JWT", "r"), "showloading"),
            Some("true")
        );
    }

    /// H5 定义里**没有** `isDeviceId`（⇒ 不发）；老抓包里有个 `deviceid` 头（⇒ 发）。
    /// 两个分支都得能用，因为它们各自有证据，而"哪个才是服务端要的"未验证。
    #[test]
    fn receive_revival_reward_only_sends_the_device_header_when_asked() {
        let without = receive_revival_reward_request("JWT", "r", None).unwrap();
        assert_eq!(header(&without, "deviceid"), None);

        let with = receive_revival_reward_request("JWT", "r", Some("BLOB")).unwrap();
        assert_eq!(header(&with, "deviceid"), Some("BLOB"));

        // ⚠ 这里**不加** `isdeviceid` 头 —— 老抓包里也没有。
        // 它与 `receiveV3` 不同：那一条是 `isdeviceid: true` + 值进 body，别照抄。
        assert_eq!(header(&with, "isdeviceid"), None);
    }

    /// `receiveTaskExpansion` 是 **GET + 单个查询参数**，不是 POST + body。
    ///
    /// 这条形状来自 H5 源码逐字：
    /// `receiveTaskExpansion: function(e){ return c["a"].get(".../receiveTaskExpansion",
    /// {params: e, headers:{showLoading:!0}})}`，调用点是 `{acceptDate: t.acceptDate}`。
    #[test]
    fn receive_task_expansion_is_a_get_with_only_the_accept_date() {
        let request = receive_task_expansion_request("JWT", "r", Some("2026-09-01"));

        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert!(request.body.is_none(), "GET 没有 body");
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/receiveTaskExpansion?acceptDate=2026-09-01"
        );
        // H5 没标 `isDeviceId` ⇒ 拦截器不注入；`deviceId` 也不在查询串里。
        assert_eq!(header(&request, "isdeviceid"), None);
        assert!(!request.url.contains("deviceId"));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        // H5 这条是 `showLoading: true`（与 `receiveRevivalReward` 的 `false` 相反）。
        assert_eq!(header(&request, "showloading"), Some("true"));
    }

    /// `cloudMultiple({})` —— `params` 是**空对象**，axios 序列化出来**一个参数都没有**。
    #[test]
    fn multiple_is_a_bare_get_with_no_query_params() {
        let request = multiple_request("JWT", "r");

        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert!(request.body.is_none(), "GET 没有 body");
        assert_eq!(
            request.url, "https://m.mcloud.139.com/ycloud/signin/page/multiple",
            "空 params ⇒ 连 `?` 都不该有"
        );
        assert!(!request.url.contains('?'));
        // H5 没标 `isDeviceId` ⇒ 拦截器不注入。
        assert_eq!(header(&request, "isdeviceid"), None);
        assert!(!request.url.contains("deviceId"));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        // H5 逐字 `showLoading: true`。
        assert_eq!(header(&request, "showloading"), Some("true"));
    }

    /// 没有实拍响应，所以 `cloudCount` 缺失**不能**判失败（`None` ≠ `0`）。
    #[test]
    fn parse_multiple_tolerates_a_missing_cloud_count() {
        let with = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"cloudCount": 21}
        }));
        assert_eq!(parse_multiple(&with).unwrap().cloud_count, Some(21));

        let without = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {}
        }));
        assert_eq!(
            parse_multiple(&without).unwrap().cloud_count,
            None,
            "上游没报数 ≠ 领到 0 豆"
        );

        // 整个 `result` 都没有也不行判失败 —— 两个客户端都只拿 `code` 判成败。
        let no_result = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
        assert_eq!(parse_multiple(&no_result).unwrap().cloud_count, None);
    }

    /// 非 `0` 的 `code` 要**当失败抛出去**，不能悄悄给个 `None`。
    #[test]
    fn parse_multiple_rejects_a_non_zero_code() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 1302, "msg": "开启自动备份按钮才可领取哦～"
        }));
        let err = parse_multiple(&envelope).unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "应是上游报错：{err}");
    }

    /// `multiple` 与 `cloudCount` 是**两个量**：前者是倍数、后者是合计。
    ///
    /// 旧项目逐字拼的日志是 `"成功获得", multiple, "倍云朵，共计", cloudCount`
    /// —— 只报 `cloudCount` 会把"翻了几倍"丢掉。
    #[test]
    fn parse_multiple_reads_the_multiplier_as_well() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"multiple": 2, "cloudCount": 6}
        }));
        let result = parse_multiple(&envelope).unwrap();
        assert_eq!(result.multiple, Some(2));
        assert_eq!(result.cloud_count, Some(6));

        // 两个字段各自独立缺失，互不牵连。
        let only_count = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"cloudCount": 6}
        }));
        assert_eq!(parse_multiple(&only_count).unwrap().multiple, None);
    }

    /// 奖品记录是 **GET + 四个查询参数**、没有 body、没有 `isDeviceId`。
    #[test]
    fn prize_record_is_a_get_with_four_query_params() {
        let request =
            prize_record_request("JWT", "r", PRIZE_MARKET_NAME, PRIZE_STATUS_PENDING, 1, 10);

        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert!(request.body.is_none(), "GET 没有 body");
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/public/record\
             ?marketname=sign_in_3_ex&type=1&pageNumber=1&pageSize=10"
        );
        // H5 没标 `isDeviceId` ⇒ 拦截器不注入。
        assert_eq!(header(&request, "isdeviceid"), None);
        assert!(!request.url.contains("deviceId"));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        assert_eq!(header(&request, "showloading"), Some("true"));
    }

    /// `marketname` 里出现保留字符要编码 —— 与 `acceptDate` 同源问题。
    #[test]
    fn prize_record_percent_encodes_the_market_name() {
        let request = prize_record_request("JWT", "r", "a b&c", 1, 1, 10);
        assert!(
            request.url.contains("marketname=a%20b%26c"),
            "{}",
            request.url
        );
    }

    /// 条目字段名有**实拍**（2026-09-19 账号 159 的「待领取」页签），
    /// 但**没有的字段也不判失败** —— 空页才是最常见的答案。
    #[test]
    fn parse_prize_records_reads_the_h5_field_names() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {"records": [{
                "prizeName": "移动云盘1000万tokens叠加包",
                "insertTime": "2026-09-16T11:12:54.000+00:00",
                "expireTime": "2026-09-30T15:59:59.000+00:00",
                "flag": 1,
                "prizeId": 260630014,
                "oid": 1270932714,
                "marketName": "sign_in_3_ex",
                "type": 1057,
                "extraWeDoNotKnow": {"whatever": true}
            }]}
        }));
        let page = parse_prize_records(&envelope).unwrap();
        assert_eq!(page.records.len(), 1);
        let item = &page.records[0];
        assert_eq!(
            item.prize_name.as_deref(),
            Some("移动云盘1000万tokens叠加包")
        );
        assert_eq!(
            item.insert_time.as_deref(),
            Some("2026-09-16T11:12:54.000+00:00")
        );
        // ⚠ 过期时间**在响应里**（虽然 H5 的列表项模板不渲染它）。
        assert_eq!(
            item.expire_time.as_deref(),
            Some("2026-09-30T15:59:59.000+00:00")
        );
        assert_eq!(item.flag, Some(1), "旧项目就是按 flag===1 过滤的");
        assert_eq!(item.prize_id, Some(260630014));
        assert_eq!(item.oid.as_deref(), Some("1270932714"), "实拍里 oid 是数字");
        assert_eq!(item.market_name.as_deref(), Some("sign_in_3_ex"));
        assert_eq!(item.prize_type, Some(1057));
        // 认不出来的字段原样留着，不丢。
        assert_eq!(
            item.raw["extraWeDoNotKnow"]["whatever"],
            serde_json::json!(true)
        );
    }

    /// 黄金向量里那条**实拍**奖品记录，逐字段对一遍。
    ///
    /// 上面那条用的是手写夹具；这一条用的是**线上真回来的**那一份 ——
    /// 手写夹具只能证明"按我们以为的字段名能解析出来"，证明不了
    /// "线上就是这个字段名"。两者都要有。
    #[test]
    fn golden_prize_record_parses_into_the_same_shape() {
        let envelope = Envelope::new(vectors()["prizeRecords"]["raw"].clone());
        let page = parse_prize_records(&envelope).expect("实拍那条必须能解析");
        assert_eq!(page.records.len(), 1);
        let item = &page.records[0];
        assert_eq!(
            item.prize_name.as_deref(),
            Some("移动云盘1000万tokens叠加包")
        );
        assert_eq!(
            item.insert_time.as_deref(),
            Some("2026-09-16T11:12:54.000+00:00")
        );
        assert_eq!(
            item.expire_time.as_deref(),
            Some("2026-09-30T15:59:59.000+00:00")
        );
        assert_eq!(item.flag, Some(1));
        assert_eq!(item.prize_id, Some(260630014));
        assert_eq!(item.oid.as_deref(), Some("1270932714"));
        assert_eq!(item.market_name.as_deref(), Some("sign_in_3_ex"));
        assert_eq!(item.prize_type, Some(1057));

        // ⚠ 脱敏口径：向量里**必须没有** `userDomainId`。
        // 这条不是形式主义 —— 线上那条响应里真的有它（19 位账号标识），
        // 摘录时删掉是**动作**，会有人漏。守卫放在这里，漏了立刻红。
        assert!(
            item.raw.get("userDomainId").is_none(),
            "黄金向量里不许留账号标识字段"
        );
    }

    /// 三个页签里总有两个是空的 ⇒ **没有 `records` 是正常答案**，不是错误。
    #[test]
    fn parse_prize_records_treats_an_empty_page_as_normal() {
        for payload in [
            serde_json::json!({"code": 0, "msg": "success", "result": {"records": []}}),
            serde_json::json!({"code": 0, "msg": "success", "result": {}}),
            serde_json::json!({"code": 0, "msg": "success"}),
        ] {
            let page = parse_prize_records(&Envelope::new(payload)).unwrap();
            assert!(page.records.is_empty());
        }
    }

    /// `oid` 可能是数字也可能是字符串（H5 直接把它塞进 URL 参数），两种都要收。
    #[test]
    fn parse_prize_records_accepts_a_numeric_oid() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"records": [{"oid": 20260919}]}
        }));
        let page = parse_prize_records(&envelope).unwrap();
        assert_eq!(page.records[0].oid.as_deref(), Some("20260919"));
    }

    /// 备份好礼两条都打在 `caiyun.feixin.10086.cn` —— 主机换过，别照抄 `MCLOUD`。
    #[test]
    fn backup_gift_requests_use_the_caiyun_host() {
        let info = backup_gift_info_request("JWT", "r");
        assert_eq!(info.method, mcloud_core::HttpMethod::Get);
        assert!(info.body.is_none());
        assert_eq!(
            info.url,
            "https://caiyun.feixin.10086.cn/market/backupgift/info"
        );
        assert_eq!(header(&info, "jwttoken"), Some("JWT"));

        let receive = backup_gift_receive_request("JWT", "r");
        assert_eq!(receive.method, mcloud_core::HttpMethod::Get);
        assert_eq!(
            receive.url,
            "https://caiyun.feixin.10086.cn/market/backupgift/receive"
        );
    }

    /// `state` 是**主判据**（缺了就必须报错），`curMonth` 是前置开关（缺了当 `false`）。
    #[test]
    fn parse_backup_gift_reads_state_and_cur_month() {
        let ready = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"state": 0, "curMonth": true}
        }));
        let gift = parse_backup_gift(&ready).unwrap();
        assert_eq!(gift.state, 0);
        assert!(gift.cur_month);

        // 旧项目 `if (!curMonth)` —— 数字 0 / 字符串 "0" 都要当假。
        for falsy in [
            serde_json::json!(0),
            serde_json::json!("0"),
            serde_json::json!(false),
        ] {
            let envelope = Envelope::new(serde_json::json!({
                "code": 0, "msg": "success", "result": {"state": -1, "curMonth": falsy}
            }));
            assert!(!parse_backup_gift(&envelope).unwrap().cur_month, "{falsy}");
        }

        // 缺 `state` ⇒ 解析错误：没有它就回答不了"该不该领"。
        let missing = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"curMonth": true}
        }));
        let err = parse_backup_gift(&missing).unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "{err}");
    }

    /// `backupgift/receive` 的 `result` **是裸数字**，不是对象。
    #[test]
    fn parse_backup_gift_receive_reads_a_bare_number() {
        let with = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": 30
        }));
        assert_eq!(parse_backup_gift_receive(&with).unwrap().received, Some(30));

        let without = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
        assert_eq!(
            parse_backup_gift_receive(&without).unwrap().received,
            None,
            "上游没报数 ≠ 领到 0 豆"
        );
    }

    /// `infoV3` 的两个翻倍门槛字段 —— 只认**真布尔**，缺了给 `None` 而不是 `false`。
    #[test]
    fn parse_sign_in_status_reads_the_two_backup_gates() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 1, "signInPoints": 3, "signCount": 1, "monthDays": 0, "cal": [],
                "curMonthBackup": true, "curMonthBackupSignAccept": false
            }
        }));
        let status = parse_sign_in_status(&envelope).unwrap();
        assert_eq!(status.cur_month_backup, Some(true));
        assert_eq!(status.cur_month_backup_sign_accept, Some(false));

        // 缺字段 ⇒ `None`（"没说"），**不是** `Some(false)`（"没开"）。
        // 这个区分是 `plan_multiple` 的安全前提：把"没说"当"没开"会漏领，
        // 当"开了"会白发一个写请求。
        let missing = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 1, "signInPoints": 3, "signCount": 1, "monthDays": 0, "cal": []
            }
        }));
        let status = parse_sign_in_status(&missing).unwrap();
        assert_eq!(status.cur_month_backup, None);
        assert_eq!(status.cur_month_backup_sign_accept, None);
    }

    /// `acceptDate` 缺席时**一个查询参数都不带** —— 这是实拍定的，不是省事。
    ///
    /// H5 的 `receiveTaskExpansion({acceptDate: t.acceptDate})` 里，`t.acceptDate`
    /// 在实拍的 `receiveList[]` 里根本不存在 ⇒ `undefined` ⇒ axios 的默认序列化器
    /// 把整个键丢掉。所以真实客户端发的是**裸 GET**。
    ///
    /// ⚠ 别改成 `?acceptDate=` —— 那是**另一个请求**（多一个空参数），没验过。
    #[test]
    fn receive_task_expansion_drops_the_param_when_accept_date_is_absent() {
        let request = receive_task_expansion_request("JWT", "r", None);
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/page/receiveTaskExpansion"
        );
        assert!(!request.url.contains('?'), "{}", request.url);
    }

    /// `acceptDate` 里出现保留字符时要编码 —— 与 `startSignIn` 的 `deviceId` 同源问题。
    #[test]
    fn receive_task_expansion_percent_encodes_the_accept_date() {
        let request = receive_task_expansion_request("JWT", "r", Some("2026-09-01 12:00+08"));
        assert!(
            request
                .url
                .ends_with("acceptDate=2026-09-01%2012%3A00%2B08"),
            "{}",
            request.url
        );
    }

    // ─────────────────── 豆的流水（`cloudRecordV2`） ───────────────────

    /// 形状来自 H5 逐字：`get("/ycloud/signin/public/cloudRecordV2", {params: e})`，
    /// 调用点是 `{aiDou, type, pageNumber, pageSize}` —— **GET + 四个查询参数**。
    #[test]
    fn cloud_record_v2_is_a_get_with_four_query_params() {
        let request =
            cloud_record_v2_request("JWT", "r", RECORD_BOOK_BEAN, RECORD_FILTER_ALL, 1, 50);

        assert_eq!(request.method, mcloud_core::HttpMethod::Get);
        assert!(request.body.is_none(), "GET 没有 body");
        assert_eq!(
            request.url,
            "https://m.mcloud.139.com/ycloud/signin/public/cloudRecordV2\
             ?aiDou=1&type=0&pageNumber=1&pageSize=50"
        );
        // H5 没标 `isDeviceId` ⇒ 拦截器不注入；查询串里也不该有 `deviceId`。
        assert_eq!(header(&request, "isdeviceid"), None);
        assert!(!request.url.contains("deviceId"));
        assert_eq!(header(&request, "jwttoken"), Some("JWT"));
        // 与 `infoV3` 同族：`showLoading: true`。
        assert_eq!(header(&request, "showloading"), Some("true"));
    }

    /// 两个账本只差 `aiDou` 一个参数 —— 别把它写成两条不同的接口。
    #[test]
    fn cloud_record_v2_books_differ_only_by_the_ai_dou_param() {
        let cloud = cloud_record_v2_request("J", "r", RECORD_BOOK_CLOUD, RECORD_FILTER_ALL, 1, 10);
        let bean = cloud_record_v2_request("J", "r", RECORD_BOOK_BEAN, RECORD_FILTER_ALL, 1, 10);
        assert!(cloud.url.contains("aiDou=0"), "{}", cloud.url);
        assert!(bean.url.contains("aiDou=1"), "{}", bean.url);
        assert_eq!(
            cloud.url.replace("aiDou=0", "aiDou=1"),
            bean.url,
            "除 `aiDou` 外必须逐字相同"
        );
        // `type` 是**负数**（`-1` = 使用记录）也要原样进查询串。
        let spent = cloud_record_v2_request("J", "r", RECORD_BOOK_BEAN, RECORD_FILTER_SPENT, 1, 10);
        assert!(spent.url.contains("type=-1"), "{}", spent.url);
    }

    /// 流水条目的**字段名没有实拍**，所以解析层只保证两个有依据的字段，
    /// 条目本身**原样**交出去 —— 这里断言"原样"这件事真的成立。
    #[test]
    fn parse_cloud_records_keeps_items_verbatim_and_tolerates_an_empty_book() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 137,
                "records": [{"whateverTheServerCallsIt": 6, "another": "2026-09-19"}]
            }
        }));
        let page = parse_cloud_records(&envelope).unwrap();
        assert_eq!(page.total, Some(137), "`total` 是**条数**，不是金额");
        assert_eq!(page.records.len(), 1);
        // 逐字保真：一个键都没改、没加、没删。
        assert_eq!(
            page.records[0],
            serde_json::json!({"whateverTheServerCallsIt": 6, "another": "2026-09-19"})
        );

        // 改名前的账本本来就可能没有记录 —— 空是**答案**，不是错误。
        let empty = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"total": 0, "records": []}
        }));
        let page = parse_cloud_records(&empty).unwrap();
        assert!(page.records.is_empty());
        assert_eq!(page.total, Some(0));

        // 连 `result` 都没有也不判失败（`None` ≠ 0，见 `total` 的说明）。
        let none = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
        let page = parse_cloud_records(&none).unwrap();
        assert!(page.records.is_empty());
        assert_eq!(page.total, None);
    }

    /// 上游失败要**原样透出**，不能变成"这一档没有记录" —— 那会把故障伪装成空账本。
    #[test]
    fn parse_cloud_records_does_not_turn_failures_into_an_empty_book() {
        let envelope =
            Envelope::new(serde_json::json!({"code": CODE_NOT_LOGGED_IN, "msg": "未登录"}));
        assert!(parse_cloud_records(&envelope).is_err());
    }

    /// 没有实拍响应，所以 `cloudCount` 缺失**不能**判失败（`None` ≠ `0`）。
    #[test]
    fn parse_receive_task_expansion_tolerates_a_missing_cloud_count() {
        let with = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"cloudCount": 88}
        }));
        assert_eq!(
            parse_receive_task_expansion(&with).unwrap().cloud_count,
            Some(88)
        );

        let without = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {}
        }));
        assert_eq!(
            parse_receive_task_expansion(&without).unwrap().cloud_count,
            None,
            "上游没报数 ≠ 领到 0 朵"
        );

        // **连 `result` 都没有**也不能判失败 —— 两个客户端都只认 `code`：
        // H5 只比 `e.data.code`，旧项目写的是 `receiveData?.result?.cloudCount`。
        // 这里曾经把这种响应报成 `Error::Parse`，那会让 `code=0` 的成功
        // 以一个看不懂的解析错误收场。
        let no_result = Envelope::new(serde_json::json!({ "code": 0, "msg": "success" }));
        assert_eq!(
            parse_receive_task_expansion(&no_result)
                .unwrap()
                .cloud_count,
            None,
            "没有 result 是「没说领到多少」，不是失败"
        );
    }

    /// H5 的两个业务错误码要**原样透出**，不能变成一句"网络异常"。
    ///
    /// `1302` = 未开启自动备份（这是"月度那条线"的门槛，不是 bug）；
    /// `500` = 已过领取有效期。
    #[test]
    fn parse_receive_task_expansion_surfaces_the_h5_error_codes() {
        for code in [1302, 500] {
            let envelope = Envelope::new(serde_json::json!({
                "code": code, "msg": "nope"
            }));
            let err = parse_receive_task_expansion(&envelope).unwrap_err();
            assert!(
                err.to_string().contains(&code.to_string()),
                "错误码要看得见：{err}"
            );
        }
    }

    #[test]
    fn parse_revival_reward_reads_the_card_and_the_amount() {
        // 逐字取自实拍响应（`test/fixtures/http/复活.json` 的 `receiveRevivalReward`）。
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {"cardType": 1, "littleFirstTime": 0, "rewardClouds": 114, "totalClouds": 571}
        }));
        let reward = parse_revival_reward(&envelope).unwrap();
        assert_eq!(reward.card_type, 1, "1 = 复活卡，2 = 回馈卡");
        assert_eq!(reward.reward_clouds, 114, "本次进账");
        assert_eq!(
            reward.total_clouds, 571,
            "累计（复活卡口径下是「累计复活AI豆」）"
        );
        assert_eq!(reward.little_first_time, 0);
    }

    #[test]
    fn missing_reward_clouds_is_a_parse_error_not_a_silent_zero() {
        // 与 `receive` / `beforeTotal` 同一条理由：`rewardClouds` 就是"这次领到多少"，
        // 静默成 0 会让"没领到"看起来像"领到了 0 朵"。
        for missing in ["cardType", "rewardClouds", "totalClouds"] {
            let mut result = serde_json::json!({
                "cardType": 1, "littleFirstTime": 0, "rewardClouds": 114, "totalClouds": 571
            });
            result.as_object_mut().unwrap().remove(missing);
            let envelope =
                Envelope::new(serde_json::json!({"code": 0, "msg": "success", "result": result}));
            let err = parse_revival_reward(&envelope).unwrap_err();
            assert!(
                matches!(err, Error::Parse(_)),
                "缺 {missing} 应是解析错误：{err}"
            );
            assert!(
                format!("{err}").contains(missing),
                "报错要点名缺的是哪个字段：{err}"
            );
        }
    }

    /// `littleFirstTime` 是**附属字段**：它只影响一句文案，答不了本函数的任何问题。
    ///
    /// 判据与 `receiveList` 那条一致 —— "缺了它还能不能回答本函数的问题"。
    /// 与上面那条"主判据缺了必须报错"成对：**别把两种字段用同一把尺子量**。
    #[test]
    fn little_first_time_is_optional_and_does_not_break_the_verdict() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {"cardType": 2, "rewardClouds": 8, "totalClouds": 20}
        }));
        let reward = parse_revival_reward(&envelope).unwrap();
        assert_eq!(reward.card_type, 2);
        assert_eq!(reward.reward_clouds, 8);
        assert_eq!(reward.little_first_time, 0, "缺了给 0，不因此判整条失败");
    }

    /// 上游的业务失败必须转成 [`Error::BadRequest`]，不能被当成"复活到 0 朵"。
    #[test]
    fn parse_revival_reward_turns_upstream_failures_into_bad_request() {
        for code in [CODE_NOT_LOGGED_IN, CODE_SIGNATURE_REJECTED] {
            let envelope = Envelope::new(serde_json::json!({"code": code, "msg": "x"}));
            let err = parse_revival_reward(&envelope).unwrap_err();
            assert!(matches!(err, Error::BadRequest(_)), "码 {code}：{err}");
        }
    }

    // ───────────────────────── 响应解析（黄金向量） ─────────────────────────

    /// 复活卡的响应从向量里读，**不在测试里重抄一遍数字**。
    ///
    /// 这一段向量的来源与其余几段不同（老项目夹具，不是本次 iOS 抓包），
    /// 出处与保留理由写在向量自己的 `note` 里。
    #[test]
    fn parse_revival_reward_from_the_pinned_capture() {
        let v = vectors();
        let segment = &v["receiveRevivalReward"];
        let envelope = Envelope::new(segment["raw"].clone());

        let reward = parse_revival_reward(&envelope).unwrap();
        let expect = &segment["raw"]["result"];
        assert_eq!(reward.card_type, expect["cardType"].as_i64().unwrap());
        assert_eq!(
            reward.reward_clouds,
            expect["rewardClouds"].as_i64().unwrap()
        );
        assert_eq!(reward.total_clouds, expect["totalClouds"].as_i64().unwrap());
        assert_eq!(
            reward.little_first_time,
            expect["littleFirstTime"].as_i64().unwrap()
        );
        assert_eq!(reward.card_type, 1, "这一段实拍用的是复活卡（回馈卡是 2）");
    }

    #[test]
    fn parse_sso_token_from_real_capture() {
        let v = vectors();
        let entry = v["envelopes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "querySpecTokenV2-success")
            .unwrap();
        let envelope = Envelope::new(entry["raw"].clone());
        let token = parse_sso_token(&envelope).unwrap();
        assert_eq!(token, entry["expectToken"].as_str().unwrap());
        assert_eq!(token.len(), 42, "形状：`YZsidssolg` + 32 位小写 hex");
        assert!(token.starts_with("YZsidssolg"));
        assert_eq!(envelope.code(), Some(0), "`code` 是字符串 \"0000\"，也要认");
    }

    #[test]
    fn parse_server_timestamp_keeps_all_13_digits() {
        let v = vectors();
        let entry = v["envelopes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "h5-timestamp-success")
            .unwrap();
        let envelope = Envelope::new(entry["raw"].clone());
        let ts = parse_server_timestamp(&envelope).unwrap();
        assert_eq!(ts, "1789800128813");
        assert_eq!(ts.len(), 13, "毫秒级 —— 少一位就是秒，签名会不对");
    }

    #[test]
    fn parse_sign_in_status_from_real_capture() {
        let v = vectors();
        let envelope = Envelope::new(v["infoV3"]["raw"].clone());
        let status = parse_sign_in_status(&envelope).unwrap();

        // ── 余额是 `total`，不是 `beforeTotal`（这对数字是实拍的一前一后）──
        //
        // 黄金向量这份是**领取前**的实拍：`total = 10496`、`beforeTotal = 8027`。
        // 领掉那条 `{cloudType:0, cloudNum:6}` 之后重新抓，`total` 变成 **10502**（+6 ✓），
        // 而 `beforeTotal` **一动不动还是 8027**。
        //
        // ⇒ 余额是 `total`。`beforeTotal` 是云朵明细（历史）页口径的另一个数。
        // 这一条曾经记反过（拿 `beforeTotal` 当余额写进 `cloud_stats`）。
        assert_eq!(status.bean_total, 10496);
        assert_eq!(status.cloud_total, Some(8027));
        // 兑换门槛 12000 减去余额，正好是上游自己报的"还差多少"。
        // 这条等式是 `total` = 余额最硬的旁证：两个字段由服务端分别算出，能对上说明口径一致。
        assert_eq!(12000 - status.bean_total, 1504);
        assert_eq!(
            v["infoV3"]["raw"]["result"]["lackCloudNum"].as_i64(),
            Some(1504),
            "`lackCloudNum` 必须等于 12000 - total"
        );
        assert_eq!(status.sign_in_points, 3);
        assert_eq!(status.sign_count, 1);
        assert_eq!(status.month_days, 7);
        assert_eq!(status.days.len(), 35, "抓包里的 cal 有 35 格");

        // 两条独立路径必须一致：一条数日历格，一条读服务端字段
        assert_eq!(
            status.counted_month_days() as i64,
            status.month_days,
            "`monthDays` 应等于 cal 里 `s && currentMonth` 的条数"
        );

        // 权威判据
        assert_eq!(status.signed_today(), Some(true), "抓包那一刻 19 号已签");
        assert_eq!(status.signed_on(19), Some(true));
        assert_eq!(status.signed_on(3), Some(false));
        assert_eq!(status.signed_on(0), None);
        assert_eq!(status.signed_on(32), None);
    }

    #[test]
    fn signed_on_ignores_other_months() {
        // `cal` 里混着上月最后一天与下月开头几天。按 `d` 裸查会命中错的月份 ——
        // 这里构造"上月 31 号签了、本月 31 号没签"，必须只认本月那个。
        let status = SignInStatus {
            bean_total: 0,
            cloud_total: Some(0),
            sign_in_points: 0,
            sign_count: 0,
            month_days: 0,
            days: vec![
                SignInDay {
                    day: 31,
                    signed: true,
                    current_month: false,
                    points: 0,
                    today: false,
                },
                SignInDay {
                    day: 31,
                    signed: false,
                    current_month: true,
                    points: 0,
                    today: false,
                },
            ],
            receive_list: Vec::new(),
            to_receive: None,
            cur_month_backup: None,
            cur_month_backup_sign_accept: None,
        };
        assert_eq!(status.signed_on(31), Some(false));
        assert_eq!(status.signed_today(), None, "没有 t:true 的格子时返回 None");
    }

    /// `receiveList` 是**可选的**：不给就是空数组，不能拖垮"今天签没签"这个主判据。
    ///
    /// 这条与 `missing_required_field_is_a_parse_error_not_a_silent_zero` 是**一对**：
    /// 主判据缺字段必须报错，附属字段缺了必须放过。判据是"缺了它还能不能回答本函数的问题"。
    #[test]
    fn receive_list_is_optional_and_never_breaks_the_main_verdict() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 1, "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
                "cal": [{"d": 1, "s": true, "n": 0, "p": 0, "t": true, "currentMonth": 1}]
            }
        }));
        let status = parse_sign_in_status(&envelope).unwrap();
        assert!(status.receive_list.is_empty());
        assert_eq!(status.to_receive, None);
        assert_eq!(status.signed_today(), Some(true), "主判据照常可用");
    }

    /// 逐字对照 HAR 里的真实 `infoV3` 响应：`[{"cloudType":2,"cloudNum":234}]`。
    #[test]
    fn parse_sign_in_status_reads_the_receive_list() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 7431, "beforeTotal": 5392, "signInPoints": 3, "signCount": 1, "monthDays": 6,
                "toReceive": 0,
                "cal": [],
                "receiveList": [{"cloudType": 2, "cloudNum": 234}]
            }
        }));
        let status = parse_sign_in_status(&envelope).unwrap();

        assert_eq!(status.to_receive, Some(0));
        assert_eq!(status.receive_list.len(), 1);
        assert_eq!(status.receive_list[0].cloud_type, 2);
        assert_eq!(status.receive_list[0].cloud_num, 234);
        assert_eq!(
            status.receive_list[0].record_id, None,
            "实拍里没有 recordId —— 别替上游编一个，H5 的 `?? \"\"` 给的就是空串"
        );
    }

    /// `receiveList` 里缺 `cloudType` / `cloudNum` 的条目被**跳过**，而不是整条报错。
    #[test]
    fn receive_items_without_the_required_numbers_are_skipped() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 1, "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
                "cal": [],
                "receiveList": [{"cloudNum": 234}, {"cloudType": 2}, {"cloudType": 1, "cloudNum": 7}]
            }
        }));
        let status = parse_sign_in_status(&envelope).unwrap();
        assert_eq!(status.receive_list.len(), 1, "只有最后一条是完整的");
        assert_eq!(status.receive_list[0].cloud_num, 7);
    }

    #[test]
    fn parse_start_sign_in_from_real_capture() {
        let v = vectors();
        let envelope = Envelope::new(v["startSignIn"]["raw"].clone());
        let result = parse_start_sign_in(&envelope).unwrap();
        assert!(result.today_sign_in, "抓包那一刻已签，所以是 true");
        assert_eq!(result.sign_in_points, 3);
        assert_eq!(result.sign_count, 1);
        assert_eq!(result.server_time_ms, Some(1789799929696));
    }

    #[test]
    fn parse_task_list_requires_an_array() {
        let ok = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": [{"id": 549, "name": "开启备份领好礼"}]
        }));
        let items = parse_task_list(&ok).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], 549);

        let bad = Envelope::new(serde_json::json!({"code": 0, "result": {"id": 1}}));
        assert!(
            parse_task_list(&bad).is_err(),
            "result 是对象时要报错，不能当空列表"
        );
    }

    // ───────────────────────── 失败路径 ─────────────────────────

    #[test]
    fn every_real_error_envelope_is_classified() {
        let v = vectors();
        for entry in v["envelopes"].as_array().unwrap() {
            let name = entry["name"].as_str().unwrap();
            if entry["code"].as_i64().unwrap_or(0) == 0 {
                continue;
            }
            let envelope = Envelope::new(entry["raw"].clone());
            assert!(envelope.ensure_ok().is_err(), "{name} 应被判失败");
        }
    }

    #[test]
    fn not_logged_in_error_carries_an_actionable_hint() {
        let envelope = Envelope::new(serde_json::json!({"code": 90001, "msg": "未登录"}));
        let err = parse_sign_in_status(&envelope).unwrap_err();
        let detail = format!("{err}");
        assert!(detail.contains("90001"), "{detail}");
        assert!(
            detail.contains("jwttoken"),
            "提示要指向真正的原因：{detail}"
        );
        // 对外文案仍走白名单，不夹带上游原文
        assert_eq!(err.public_message(), "请求参数不合法");
    }

    #[test]
    fn diagnose_covers_the_three_debugging_signals() {
        assert!(diagnose(CODE_NOT_LOGGED_IN).unwrap().contains("jwttoken"));
        assert!(diagnose(CODE_SSO_EXPIRED).unwrap().contains("立刻用"));
        assert!(diagnose(CODE_SIGNATURE_REJECTED).unwrap().contains("签名"));
        assert_eq!(diagnose(0), None);
    }

    /// 领取类的三个码**必须都有提示**，而且提示里要能看出"这是领取失败" ——
    /// 否则它们会掉进 `None` 那一档，被当成"没有已知提示"的陌生错误。
    #[test]
    fn diagnose_covers_the_receive_codes() {
        assert!(diagnose(CODE_RECEIVE_TOO_BUSY).unwrap().contains("火爆"));
        assert!(diagnose(CODE_RECEIVE_FAILED).unwrap().contains("领取失败"));
        assert!(
            diagnose(CODE_RECEIVE_FAILED_ALT)
                .unwrap()
                .contains("领取失败")
        );
    }

    /// `614` 与 `615` 的提示必须**不一样**。
    ///
    /// 上游把它们归成同一句 toast（「领取失败，请稍后再试」），但实测是两件事：
    /// 一个是"没带"、一个是"带了但内容无效"。合并成一条提示，
    /// 等于把"该去查参数有没有传"与"该去查参数对不对"这两条排查路径又混回去。
    #[test]
    fn diagnose_distinguishes_a_missing_device_id_from_an_invalid_one() {
        let missing = diagnose(CODE_RECEIVE_FAILED_ALT).unwrap();
        let invalid = diagnose(CODE_RECEIVE_FAILED).unwrap();

        assert_ne!(missing, invalid);
        assert!(missing.contains("没带"), "615 应指向「没带」：{missing}");
        assert!(
            invalid.contains("无效"),
            "614 应指向「内容无效」：{invalid}"
        );
    }

    #[test]
    fn missing_required_field_is_a_parse_error_not_a_silent_zero() {
        // `total`（余额）缺失时**不能**静默成 0 —— 那会让"余额 0"看起来像真实数据，
        // 而它是要被记进 `cloud_stats` 的。
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {"signInPoints": 3, "signCount": 1, "monthDays": 1, "cal": []}
        }));
        let err = parse_sign_in_status(&envelope).unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "应是解析错误：{err}");
        assert!(
            format!("{err}").contains("total"),
            "要点名缺的是哪个字段：{err}"
        );

        // ⚠ 对照组：`beforeTotal` 是**我们不用的旁证数**，缺了**不该**打挂整条调用。
        // 这两条合起来就是"两种字段用两把尺子"——判据是"缺了它还能不能回答问题"。
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success",
            "result": {
                "total": 7431, "signInPoints": 3, "signCount": 1, "monthDays": 1,
                "cal": [{"d": 1, "s": true, "n": 3, "p": 0, "t": true, "currentMonth": 1}]
            }
        }));
        let status = parse_sign_in_status(&envelope).unwrap();
        assert_eq!(status.bean_total, 7431);
        assert_eq!(status.cloud_total, None, "缺了给 `None`，不是 0");
    }

    #[test]
    fn calendar_day_out_of_range_is_rejected() {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0,
            "result": {
                "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
                "cal": [{"d": 999, "s": false, "n": 0, "p": 0, "t": false, "currentMonth": 1}]
            }
        }));
        assert!(parse_sign_in_status(&envelope).is_err());
    }
}
