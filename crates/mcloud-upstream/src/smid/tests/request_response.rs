//! 请求与响应。

use super::*;

// ==================== 请求与响应 ====================

#[test]
fn request_targets_cmpassport_and_carries_no_cloud_credentials() {
    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(1)).unwrap();
    let request = device_profile_request(&payload, &fp()).unwrap();

    assert_eq!(
        request.url,
        "https://slw.h5cmpassport.com:9090/deviceprofile/v4"
    );
    assert_eq!(request.method, "POST");
    // 这是**第三方风控**：云盘的凭据一个都不能漏出去
    for forbidden in [
        "authorization",
        "jwttoken",
        "x-yun-uni",
        "cookie",
        "x-deviceinfo",
    ] {
        assert!(
            !request
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case(forbidden)),
            "{forbidden} 不该发给 cmpassport"
        );
    }
    for required in ["origin", "referer", "user-agent"] {
        assert!(
            request
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case(required)),
            "缺请求头 {required}"
        );
    }
}

#[test]
fn the_request_carries_the_payload_as_its_body() {
    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(1)).unwrap();
    let request = device_profile_request(&payload, &fp()).unwrap();
    let body: Value = serde_json::from_str(request.body_str().unwrap()).unwrap();
    assert_eq!(body, payload);
}

#[test]
fn ok_response_gets_the_b_prefix_back() {
    let raw = json!({"code": 1100, "detail": {"deviceId": "1990000000000000000001"}});
    assert_eq!(parse_device_id(&raw).unwrap(), "B1990000000000000000001");
}

#[test]
fn rejected_response_fails_without_echoing_the_body() {
    // 错误串会被打日志：响应体里可能带着别的 deviceId，不能跟着出去
    let raw = json!({"code": 1101, "detail": {"deviceId": "SECRET1990000000000000000000"}});
    let err = parse_device_id(&raw).unwrap_err().to_string();
    assert!(err.contains("1101"), "{err}");
    assert!(!err.contains("SECRET"), "响应体不该被回吐：{err}");

    let no_detail = json!({"code": 1100});
    assert!(parse_device_id(&no_detail).is_err());
    // 空 deviceId 也失败：旧实现会拼出光秃秃一个 "B" 存下去
    let empty = json!({"code": 1100, "detail": {"deviceId": ""}});
    assert!(parse_device_id(&empty).is_err());
}

#[test]
fn a_non_numeric_code_is_rejected() {
    // 云盘那批用字符串码（"0000"），这里用数字码 —— 两者不是一类，别混
    let raw = json!({"code": "1100", "detail": {"deviceId": "x"}});
    assert!(parse_device_id(&raw).is_err(), "字符串码不该被当成成功");
}

#[test]
fn missing_code_is_rejected() {
    assert!(parse_device_id(&json!({"detail": {"deviceId": "x"}})).is_err());
}

/// 假客户端：确认 `smid_device_id` 会真的发请求并解析响应。
#[tokio::test]
async fn smid_device_id_sends_the_request_and_returns_the_prefixed_id() {
    use mcloud_core::{HttpRequest, HttpResponse};
    use std::sync::Mutex;

    struct FakeHttp {
        seen: Mutex<Vec<HttpRequest>>,
        body: String,
        status: u16,
    }

    #[async_trait::async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
                headers: Vec::new(),
            })
        }
    }

    let http = FakeHttp {
        seen: Mutex::new(Vec::new()),
        body: json!({"code": 1100, "detail": {"deviceId": "199abc"}}).to_string(),
        status: 200,
    };

    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(1)).unwrap();
    let id = smid_device_id(&http, &fp(), &payload).await.unwrap();
    assert_eq!(id, "B199abc");

    let seen = http.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].url.contains("deviceprofile/v4"));
}

#[tokio::test]
async fn a_non_2xx_status_is_a_network_error_not_an_empty_id() {
    use mcloud_core::{HttpRequest, HttpResponse};
    use std::sync::Mutex;

    struct FakeHttp {
        seen: Mutex<Vec<HttpRequest>>,
    }

    #[async_trait::async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: 500,
                body: "boom".into(),
                headers: Vec::new(),
            })
        }
    }

    let http = FakeHttp {
        seen: Mutex::new(Vec::new()),
    };
    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(1)).unwrap();
    // 旧实现把异常 catch 成空串；这里必须是 Err ——
    // 空串被当成成功存进库，那个账号就永远绑着一个空设备
    let err = smid_device_id(&http, &fp(), &payload).await.unwrap_err();
    assert!(matches!(err, Error::Network(_)), "{err:?}");
}

/// `deviceid` 必须能穿过会话层那套头改写。
///
/// # 为什么这条值得单独测
///
/// 组装层的顺序是 `SmidClient`（补 `deviceid`）→ `H5Client`（改写成 H5 的头）
/// → `ReqwestClient`。中间那层会重写 `referer`、补 `jwttoken` / `activityid`，
/// 还会做一次 `dedup_headers`（同名头只留最后一个）。
///
/// 也就是说，`SmidClient` 辛苦换来的 id 要活着穿过一层**会改动 headers 的**
/// 装饰器。这条链路一旦断掉，表现是"每个请求都缺 `deviceid`"—— 而它编译通过、
/// 单元测试也全绿（各层自己都对），只有把两层接起来才看得出来。
#[test]
fn deviceid_survives_the_h5_header_rewrite() {
    use crate::session::{h5_headers, SessionRecipe};

    // 模拟 `SmidClient` 已经补好的请求
    let with_device = HttpRequest::get("https://m.mcloud.139.com/ycloud/api/x")
        .header("deviceid", "B1990000000000000000001");

    // 穿过 `H5Client` 实际调用的那个改写函数
    let params = SessionRecipe::SignInPage.params();
    let rewritten = h5_headers(
        with_device,
        params,
        "https://m.mcloud.139.com/portal/mobilecloud/index.html",
        Some("a-jwt-token"),
        "true",
    );

    let deviceid = rewritten
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("deviceid"))
        .map(|(_, v)| v.clone());
    assert_eq!(
        deviceid.as_deref(),
        Some("B1990000000000000000001"),
        "会话层改头不能把 deviceid 吃掉：headers={:?}",
        rewritten.headers
    );
    // 顺带确认会话层该加的东西确实加了（否则上面那条可能只是"没改写"）
    assert!(
        rewritten
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("jwttoken")),
        "会话层必须补上 jwttoken"
    );
}
