use super::*;
use crate::Envelope;
use crate::base;
use mcloud_core::{Error, HttpRequest};
use serde_json::json;

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
