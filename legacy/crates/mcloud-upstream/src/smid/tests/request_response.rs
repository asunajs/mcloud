// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

// ==================== 请求与响应 ====================

#[test]
fn request_targets_cmpassport_and_carries_no_cloud_credentials() {
    let payload = build_sm_data(&fp(), &upload(), &mut SeqRng(1)).unwrap();
    let request = device_profile_request(&payload, &fp()).unwrap();

    assert_eq!(
        request.url,
        "https://slw.h5cmpassport.com:9090/deviceprofile/v4"
    );
    assert_eq!(request.method, mcloud_core::HttpMethod::Post);
    // 这是**第三方风控**：云盘的凭据一个都不能漏出去
    for forbidden in [
        "authorization",
        "jwttoken",
        "x-yun-uni",
        "cookie",
        "x-DeviceInfo",
    ] {
        assert!(
            !request
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case(forbidden)),
            "{forbidden} 不该发给 cmpassport"
        );
    }
    for required in ["origin", "referer", "user-agent", "content-type"] {
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
fn the_signin_page_fingerprint_sdk_is_this_same_client() {
    // 这条是"**签到链路的 deviceId 不需要浏览器**"这个结论的地基。
    //
    // 签到页加载的 `fp.min.js` 是数美设备指纹，它自己往 `window._smConf` 里
    // 写一份配置（见签到页 HTML）。那份配置如果与本模块的常量逐字相同，
    // 两边打的就是**同一个 deviceprofile/v4**，换回来的 `"B"+detail.deviceId`
    // 也就是同一个东西 —— 于是"拿官方 App 打开一次签到页、再从 WebView 的
    // localStorage 里把 deviceId 读出来"这条路可以彻底不要。
    //
    // 常量哪天被谁改了，这里先炸，而不是"领取忽然回 615"（那个码看起来
    // 像"没带 deviceId"，实际是"带了一个另一个命名空间的 id"）。
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../../test/golden/app-signin-vectors.json"
    ))
    .unwrap();
    let c = &vectors["constants"];

    assert_eq!(c["SM_ORGANIZATION"].as_str().unwrap(), ORGANIZATION);
    assert_eq!(c["SM_APP_ID"].as_str().unwrap(), APP_ID);
    assert_eq!(
        c["SM_API_HOST"].as_str().unwrap(),
        API_HOST,
        "向量里带 scheme，常量里也带 —— 两边都得是完整 origin"
    );
    assert_eq!(c["SM_API_PATH"].as_str().unwrap(), API_PATH);
    assert_eq!(
        c["SM_PUBLIC_KEY"].as_str().unwrap(),
        mcloud_core::SKLAND_PUBLIC_KEY_B64
    );
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
