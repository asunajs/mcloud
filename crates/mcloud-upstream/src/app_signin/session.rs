use super::*;
use crate::Envelope;
use crate::app_client::AppProfile;
use crate::base;
use mcloud_core::{Error, HttpRequest, Random, generate_signature};
use serde_json::{Value, json};

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
