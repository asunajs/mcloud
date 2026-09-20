// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;
use mcloud_core::Error;
use serde_json::Value;

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
    let envelope = Envelope::new(serde_json::json!({"code": 0, "msg": "success", "result": {}}));
    let err = parse_receive_v3(&envelope).unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "应是解析错误：{err}");
    assert!(format!("{err}").contains("receive"));
}
