use super::*;
use crate::Envelope;
use mcloud_core::{Error, HttpRequest};
use serde_json::Value;

/// H5 请求的公共头。
///
/// `jwt` 为 `None` 表示"还没建会话"（只有 `tyrzLogin` 那一条是这样）。
/// `show_loading` 是纯 UI 提示（H5 的 loading 蒙层），照抄是为了与真实客户端
/// 逐字一致，服务端不据此做任何判断。
pub(crate) fn with_h5_headers(
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
pub(crate) fn percent_encode(value: &str) -> String {
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

// ─────────────────────────────────────────────────────────────────────────────
// 响应解析
// ─────────────────────────────────────────────────────────────────────────────
/// 从信封里取一个必填字符串字段，缺了就报 [`Error::Parse`]。
///
/// 不用 `serde` 反序列化整包：这些响应里有十几处"服务端有时给、有时不给"的字段，
/// 用 `#[serde(default)]` 会把"其实没给"静默成默认值 —— 而这里恰恰要靠字段判断成败。
pub(crate) fn require_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse(format!("响应缺少字符串字段 `{key}`")))
}

pub(crate) fn require_i64(value: &Value, key: &str) -> Result<i64, Error> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Parse(format!("响应缺少整数字段 `{key}`")))
}

/// 业务失败 → [`Error::BadRequest`]，并把 [`diagnose`] 的提示附在 detail 里。
///
/// **不**把上游原文放进对外文案：`Error::public_message()` 走白名单，
/// detail 只进日志。理由见 `crate::error_from_status`。
pub(crate) fn ensure_ok(envelope: &Envelope) -> Result<Option<&Value>, Error> {
    match envelope.ensure_ok() {
        Ok(payload) => Ok(payload),
        Err(err) => match envelope.code().and_then(diagnose) {
            Some(hint) => Err(Error::BadRequest(format!("{err}（{hint}）"))),
            None => Err(err),
        },
    }
}
