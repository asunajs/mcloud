//! 刷新凭据：`POST /user/auth/refreshToken`。
//!
//! # 用哪条接口 —— 证据说话
//!
//! 仓库里曾有两条候选：
//!
//! | 接口 | 证据 | 结论 |
//! |---|---|---|
//! | `user-njs.yun.139.com/user/auth/refreshToken` | **实拍闭环**：刷新响应的 `token` 编进 `Authorization` 后，在同一次会话的后续请求头里逐字命中 | **实现它** |
//! | `aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do` | 旧 TS 走的是它（XML 体、content-type 却写 json）；**本仓零抓包**，App 流量里一次都没出现 | **不实现** |
//!
//! 两条都未实测时选有实测的那条；一条已实测时**只**走它。
//! 常量 [`crate::endpoints::AUTH_TOKEN_REFRESH`] 保留着（不删），
//! 因为"删掉后要用时得重新查一遍它是什么"。
//!
//! # 三个单位不是一回事 ⚠
//!
//! 同屏时最容易看错的就是这里：
//!
//! | 名字 | 单位 | 含义 |
//! |---|---|---|
//! | `token` 第 4 段 | 13 位 epoch **毫秒** | **何时**过期（绝对时刻） |
//! | 响应里的 `expireTime` | **秒**（实测 `2592000` = 30 天） | 还**能**用多久（相对时长） |
//! | 落库的 `auth.expire` | 毫秒 | 与 token 第 4 段同源 |
//!
//! `expireTime` **不落库**：token 自己就带过期段，而两个来源迟早会分叉。
//! 它只用来排障（"上游说还能用 30 天"）。
//!
//! # `clientType` 是**第五处**跨端常量分歧
//!
//! App 发 `412`，旧 TS 项目里 `refreshToken` 默认 `414`。
//! 已经犯过同类错误的地方：`Authorization` 前缀、签名密钥、`version`、`toSourceId`。
//! **拿码之前先问："它出现在哪个端？由谁发出？"**

use serde::{Deserialize, Serialize};

use mcloud_core::{Account, Error, HttpRequest, HttpResponse, Result};

/// 刷新接口路径。两端同一个 host、同一个路径。
pub const REFRESH_TOKEN_PATH: &str = "/user/auth/refreshToken";

/// 主机（原生 A 族）。
pub const BASE_USER_YUN: &str = "https://user-njs.yun.139.com";

/// App 端的 `clientType`。
///
/// ⚠ **`412` 是 App 的码**，旧 TS 项目里那个 `414` 不是。
/// 实测出处：`docs/capture/app-rights-token-samples.txt` §4。
pub const APP_CLIENT_TYPE: &str = "412";

/// PC 端的 `clientType`。
///
/// ⚠ 这个值**没有实测**（PC 走不走这条接口本身都还是"按 H5 形状拼出来的"）。
/// 留着它是因为 PC 账号也需要续期，而这个值来自 PC 客户端源码；
/// 一旦实测出别的值，改这里一处。
pub const PC_CLIENT_TYPE: &str = "414";

/// 刷新请求体。
///
/// ⚠ 两个键都**必须有**：
///
/// - `userDomainId` —— "刷哪个用户域里的那个 token"。**不能猜**：
///   编一个会把刷新打到别人的用户域上（上游参考实现的原话）。
/// - `clientType` —— "我是哪个端"。
///
/// 字段顺序按**实拍原串**（`userDomainId` 在前）。用 `serde_json` 的 Map
/// 保序特性输出，与抓包一致。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshTokenBody {
    #[serde(rename = "userDomainId")]
    pub user_domain_id: String,
    #[serde(rename = "clientType")]
    pub client_type: String,
}

/// 刷新响应里我们要的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshResult {
    /// 新的 authToken。
    ///
    /// 实测它就是 `Authorization` 整串解码后的**第三段** —— 所以拿到它之后
    /// 要重新拼整串（见 [`rebuild_authorization`]），而不是替换整串。
    pub token: String,
    /// 上游说"还能用多久"（**秒**）。只供排障，不落库。
    pub expire_seconds: Option<i64>,
}

/// 构造刷新请求。
///
/// `authorization` 是**当前**那串（`Basic base64(...)`）—— 刷新要证明
/// "我是这个 token 的持有者"。
pub fn refresh_request(account: &Account) -> Result<HttpRequest> {
    let domain = account.user_domain_id.as_deref().ok_or_else(|| {
        Error::Config(format!(
            "账号 {} 缺少用户域（userDomainId），无法刷新凭据。\
             该账号是在本字段存在之前登录的，请重新登录一次。",
            account.id
        ))
    })?;

    let client_type = client_type_for(account);

    // ⚠ 这里**不能**用 `serde_json::json!`。
    //
    // `json!` 建的是 `Map<String, Value>`，而本项目启用的 serde_json
    // **默认按 BTreeMap 排序输出键**（除非开 `preserve_order` 特性）——
    // 于是 `{"userDomainId":…,"clientType":…}` 会变成
    // `{"clientType":"412","userDomainId":…}`，**与实拍顺序相反**。
    //
    // 顺序在签名类接口上不是小事，而这个接口虽然不签名，也没有理由
    // 与抓包不一致 —— 一旦上游将来开始按体签名，"顺序对不上"就会
    // 表现成一个含义模糊的鉴权错，而没人会想到是键序。
    //
    // 手拼是刻意的：两个字段都经过 `escape_json_string`，
    // 不是裸 format（token 里可能有需要转义的字符）。
    let body = format!(
        r#"{{"userDomainId":"{}","clientType":"{}"}}"#,
        escape_json_string(domain),
        escape_json_string(client_type)
    );

    Ok(
        HttpRequest::post(format!("{BASE_USER_YUN}{REFRESH_TOKEN_PATH}"))
        // ⚠ content-type 带 `;charset=UTF-8`（实拍如此）
        .header("Content-Type", "application/json;charset=UTF-8")
        // ⚠ `auth.raw` 是**裸 base64**（`AuthInfo::raw` 的契约：写回配置时
        // 保持原形态，不含 `Basic ` 前缀）。这里补前缀；补两次就是
        // `Basic Basic xxx` —— 上游只会回一个含义模糊的鉴权错。
        .header("Authorization", format!("Basic {}", account.auth.raw))
        .text_body(body),
    )
}

/// 这个账号该用哪个 `clientType`。
///
/// 判据是**凭据串里的端别**（`AuthInfo.platform`），不是参数 ——
/// "库里是 App 的串、参数选 PC"这种矛盾组合没有意义。
///
/// PC 账号再按 `pc_platform` 细分（实拍未覆盖，暂时两平台同码）。
pub fn client_type_for(account: &Account) -> &'static str {
    if account.auth.platform == "pc" {
        PC_CLIENT_TYPE
    } else {
        APP_CLIENT_TYPE
    }
}

/// 解析刷新响应。
///
/// 信封是**又一种**形状：`success` + `code`（**字符串** `"0000"`）+ `message` + `data`。
pub fn parse_refresh(body: &str) -> Result<RefreshResult> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| Error::Parse(format!("refreshToken 响应不是 JSON：{e}")))?;

    // `code` 是字符串 "0000"；同时 `success` 也要为真。
    // 两个都要看：只看一个的话，另一种失败形态会被当成成功。
    let code_ok = match v.get("code") {
        Some(serde_json::Value::String(s)) => s == "0000" || s == "0",
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(0),
        _ => false,
    };
    let success = v
        .get("success")
        .and_then(|b| b.as_bool())
        .unwrap_or(code_ok);

    if !success || !code_ok {
        let msg = v
            .get("message")
            .or_else(|| v.get("msg"))
            .and_then(|m| m.as_str())
            .unwrap_or("(无消息)");
        return Err(Error::Other(format!(
            "refreshToken 失败：code={} message={msg}",
            v.get("code").map(|c| c.to_string()).unwrap_or_default()
        )));
    }

    let data = v
        .get("data")
        .ok_or_else(|| Error::Parse("refreshToken 响应没有 data".into()))?;

    let token = data
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Error::Parse("refreshToken 响应没有 data.token".into()))?
        .to_string();

    // `expireTime` 是**秒**且是**相对时长**。缺失不算失败 ——
    // token 自己就带过期段。
    let expire_seconds = data.get("expireTime").and_then(|e| match e {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    });

    Ok(RefreshResult {
        token,
        expire_seconds,
    })
}

/// 把新 token 拼回完整的 `Authorization` 串。
///
/// # 为什么是"重拼"而不是"替换"
///
/// 实测确认：刷新返回的 `token` 就是 `Authorization` 整串解码后的**第三段**
/// （形状 `mobile:<手机号>:<token>`）。所以正确做法是保留前两段、
/// 只换第三段。
///
/// 直接把整串替换成新 token 的写法，落库后是**一个用不了的串** ——
/// 而它看起来"有值"，排查时很难想到是拼错了。
///
/// 前缀取 `auth.platform`（`mobile` / `pc`），**不重新猜**：
/// 猜错的表现是"用 PC 的身份去续 App 的会话"。
pub fn rebuild_authorization(account: &Account, new_token: &str) -> Result<String> {
    if new_token.is_empty() {
        return Err(Error::Parse("新 token 为空".into()));
    }
    let phone = &account.auth.phone;
    if phone.is_empty() {
        return Err(Error::Parse(
            "账号缺手机号，拼不出 Authorization（它要 `端别:手机号:token`）".into(),
        ));
    }
    // ⚠ 返回的是**裸 base64**，与 `AuthInfo::raw` 同形（不带 `Basic ` 前缀）。
    // 带前缀的话，`basic_authorization()` 会再补一次，变成 `Basic Basic …`。
    let plain = format!("{}:{phone}:{new_token}", account.auth.platform);
    Ok(mcloud_crypto::b64_encode(plain.as_bytes()))
}

/// JSON 字符串转义（只做最小的必要集）。
///
/// `userDomainId` 是 19 位数字、`clientType` 是 3 位数字，实际上永远不需要
/// 转义 —— 但**不因此省掉这一步**：将来有人把别的字段塞进来（或上游的
/// 用户域变成别的形状）时，"忘了转义"会静默产出一个坏 JSON。
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 从响应里取"新凭据"并校验它是可用的形状。
///
/// 单独一层是为了让调用方只处理 `Result`，不必自己串
/// "解析 → 重拼 → 校验"三步（少写一步就是落一个坏串进库）。
pub fn apply_refresh(account: &Account, resp: &HttpResponse) -> Result<(RefreshResult, String)> {
    if !(200..300).contains(&resp.status) {
        return Err(Error::Network(format!(
            "refreshToken 失败，HTTP {}：{}",
            resp.status,
            resp.body.chars().take(200).collect::<String>()
        )));
    }
    let result = parse_refresh(&resp.body)?;
    let auth = rebuild_authorization(account, &result.token)?;
    Ok((result, auth))
}

#[cfg(test)]
mod tests;
