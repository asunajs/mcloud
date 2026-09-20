use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::{Value, json};

// ─────────────────────────────────────────────────────────────────────────────
// 签到主体（全部需要 jwttoken 头）
// ─────────────────────────────────────────────────────────────────────────────

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
