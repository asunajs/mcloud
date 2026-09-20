// 由上游登录模块的 `mod tests` 按主题拆出；确定性随机源等 helper 见同目录 mod.rs。
use super::*;

#[test]
fn request_urls_and_tid_header() {
    // 四个接口都在用户域，且每个请求都带一个新的 x-yun-tid
    for (req, path) in [
        (thirdlogin_request(&json!({})).unwrap(), "/user/thirdlogin"),
        (
            send_sms_code_request(&json!({})).unwrap(),
            "/user/sms/getSmsCode",
        ),
        (
            slider_puzzle_request(&json!({})).unwrap(),
            "/user/verfycode",
        ),
        (logout_request(&json!({})).unwrap(), "/user/logout"),
    ] {
        assert_eq!(req.url, format!("https://user-njs.yun.139.com{path}"));
        assert_eq!(req.method.as_str(), "POST");
    }

    let profile = ClientProfile::new(PcPlatform::Windows, "DEV-1");
    let with = with_client_headers(thirdlogin_request(&json!({})).unwrap(), &profile, "tid-1");
    assert!(
        with.headers
            .contains(&("x-yun-tid".to_string(), "tid-1".to_string()))
    );
}

#[test]
fn poll_classification_covers_every_documented_code() {
    let poll = |v: Value| classify_poll(&Envelope::new(v));

    assert_eq!(
        poll(json!({"code": "200059541", "message": "未找到设备的二维码信息"})),
        PollOutcome::NotScanned
    );
    assert_eq!(
        poll(json!({"code": "200059548", "message": "二维码信息未授权"})),
        PollOutcome::Scanned
    );
    assert_eq!(
        poll(json!({"code": "200059544"})),
        PollOutcome::SessionMismatch
    );
    assert_eq!(poll(json!({"code": "200059542"})), PollOutcome::Expired);
    // 实测补的码：连着跑几次扫码登录会被限频。它不是"参数错误"——
    // 区别很重要：参数错误该改请求，限流该等。
    assert_eq!(
        poll(json!({"code": "200059545", "message": "扫码登录过于频繁"})),
        PollOutcome::TooFrequent
    );

    // 成功两种码都见过：`0000`（字符串）与 `200`（数字）
    assert_eq!(
        poll(json!({"code": "0000", "data": "CIPHER"})),
        PollOutcome::Success {
            encrypted: "CIPHER".to_string()
        }
    );
    assert_eq!(
        poll(json!({"code": 200, "data": "CIPHER"})),
        PollOutcome::Success {
            encrypted: "CIPHER".to_string()
        }
    );
}

#[test]
fn success_code_without_ciphertext_is_reported_not_faked() {
    // 成功码但 data 形状不对：必须如实报告，不能伪造一个空凭据往下走
    match classify_poll(&Envelope::new(json!({"code": "0000", "data": {"a": 1}}))) {
        PollOutcome::Other { message, .. } => assert!(message.contains("data")),
        other => panic!("应当归为 Other，实际 {other:?}"),
    }
}

#[test]
fn unknown_poll_code_is_preserved_for_caller() {
    // 上游在活动期会加新码，硬编码成"错误"会把新码误判
    match classify_poll(&Envelope::new(json!({"code": 999999, "message": "新码"}))) {
        PollOutcome::Other { code, message } => {
            assert_eq!(code, Some(999_999));
            assert_eq!(message, "新码");
        }
        other => panic!("应当原样带回，实际 {other:?}"),
    }
}

#[test]
fn send_sms_classification_handles_slider_downgrade() {
    let sent = classify_send_sms(&Envelope::new(json!({
        "success": true, "code": "0000",
        "data": { "remainGetTimes": 8 }
    })));
    assert_eq!(
        sent,
        SendSmsOutcome::Sent {
            remain_get_times: Some(8)
        }
    );

    // 需要滑块
    assert_eq!(
        classify_send_sms(&Envelope::new(json!({
            "success": false, "code": "200059554", "data": {"captchaType": 2}
        }))),
        SendSmsOutcome::NeedSlider {
            captcha_type: Some(2)
        }
    );

    // 挑战作废 —— 必须重新取图（协议 §3 的降级路径）
    assert_eq!(
        classify_send_sms(&Envelope::new(json!({
            "success": false, "code": "200059553", "data": {"captchaType": 1}
        }))),
        SendSmsOutcome::SliderExhausted {
            captcha_type: Some(1)
        }
    );
}

#[test]
fn decryptor_failure_is_loud_not_a_silent_empty_credential() {
    // 解密失败必须是明确的错误，而不是一个看起来成功的空结果：
    // 后者会表现为"登录成功了但 auth 是空的"，是最难查的一类故障。
    let err = LoginDataDecryptor::production()
        .decrypt_user_info("not-a-cipher")
        .unwrap_err();
    assert_eq!(err.kind(), mcloud_core::ErrorKind::Parse, "{err}");
    assert!(!err.public_message().is_empty());
}
