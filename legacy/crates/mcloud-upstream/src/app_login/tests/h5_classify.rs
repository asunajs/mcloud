// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

// ── 滑块 H5 ──

#[test]
fn captcha_h5_url_decodes_to_the_captured_payload() {
    let url = captcha_h5_url(&profile(), PHONE);
    assert!(url.starts_with("https://yun.139.com/fe/captcha/?data="));

    let b64 = url.split("?data=").nth(1).unwrap();
    // `mcloud-core` 只导出了编码器，测试里自带一个小解码器
    let raw = decode_b64(b64);
    let v: Value = serde_json::from_slice(&raw).unwrap();

    assert_eq!(v["captchaType"], json!(2));
    assert_eq!(v["params"]["clientType"], json!("412"));
    assert_eq!(v["params"]["reqType"], json!("3"));
    assert_eq!(v["params"]["nationCode"], json!("+86"));
    assert_eq!(v["params"]["phoneNumber"], json!("19900000000"));

    // headers 里**只有两个头**，不是请求头全集
    let headers = v["headers"].as_object().unwrap();
    assert_eq!(headers.len(), 2, "H5 的 data 里只带两个头");
    assert_eq!(v["headers"]["x-yun-app-channel"], json!("10000020"));
    // 这个 client-info **没有**末尾的 `|0|;`（与请求头不同）
    assert_eq!(
        v["headers"]["x-yun-client-info"],
        json!(
            "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|aVBhZA==|"
        )
    );
}

/// 测试用的标准 base64 解码（`mcloud-core` 只导出了编码器）。
fn decode_b64(s: &str) -> Vec<u8> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in s.bytes().filter(|b| *b != b'=') {
        let v = TABLE
            .iter()
            .position(|c| *c == b)
            .expect("非法 base64 字符") as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

// ── 响应语义 ──

#[test]
fn classify_send_sms_maps_the_captcha_gate() {
    // 抓包里那条真实响应
    let env = Envelope::new(json!({
        "success": false,
        "code": "200059554",
        "message": "请先获取滑块拼图验证码",
        "data": { "captchaType": 2 }
    }));
    assert_eq!(
        classify_send_sms(&env),
        AppSendSmsOutcome::NeedCaptcha {
            captcha_type: Some(2)
        }
    );

    // `200059553` 的语义是"**这一个挑战作废了**"（补测时亲手撞到），
    // 必须与 `200059554`（首次要滑块）分开：前者要**重取图**，
    // 后者是**首次取图**。归成同一个分支会让调用方拿旧答案重试，
    // 既必然再失败一次，又白记一次失败次数。
    let voided = Envelope::new(json!({
        "success": false,
        "code": "200059553",
        "message": "同一滑动拼图验证码在单位时间内校验失败次数超限",
        "data": { "captchaType": 1 }
    }));
    assert_eq!(
        classify_send_sms(&voided),
        AppSendSmsOutcome::SliderExhausted {
            captcha_type: Some(1)
        }
    );
}

#[test]
fn classify_send_sms_reads_quota_on_success() {
    let env = Envelope::new(json!({
        "success": true, "code": "0000", "message": "请求成功",
        "data": { "remainGetTimes": 4 }
    }));
    assert_eq!(
        classify_send_sms(&env),
        AppSendSmsOutcome::Sent {
            remain_get_times: Some(4)
        }
    );
}

#[test]
fn classify_login_separates_the_two_probe_codes() {
    // 这两个码是整个加密实现能不能收工的判据，必须分开报
    let wrong_code = Envelope::new(json!({
        "success": false, "code": "9441", "message": "验证码不正确", "data": null
    }));
    assert_eq!(classify_login(&wrong_code), AppLoginOutcome::SmsCodeWrong);

    let undecryptable = Envelope::new(json!({
        "success": false, "code": "9103", "message": "输入参数不合法", "data": null
    }));
    assert_eq!(
        classify_login(&undecryptable),
        AppLoginOutcome::Undecryptable
    );

    let login_failed = Envelope::new(json!({
        "success": false, "code": "9999", "message": "登录失败！", "data": null
    }));
    assert_eq!(classify_login(&login_failed), AppLoginOutcome::LoginFailed);

    // `9441` 与 `9103` 不能折叠成同一个变体 —— 折了就没法用它们定位问题
    assert_ne!(classify_login(&wrong_code), classify_login(&undecryptable));
}

#[test]
fn classify_login_returns_the_ciphertext_on_success() {
    let env = Envelope::new(json!({
        "success": true, "code": "0000", "message": "请求成功",
        "data": "deadbeef"
    }));
    assert_eq!(
        classify_login(&env),
        AppLoginOutcome::Success {
            encrypted: "deadbeef".to_string()
        }
    );
}

#[test]
fn classify_login_reports_bad_shape_instead_of_faking_credentials() {
    // 成功码但 data 不是字符串：如实报错，不能伪造一个空凭据
    let env = Envelope::new(json!({"success": true, "code": "0000", "data": null}));
    match classify_login(&env) {
        AppLoginOutcome::Other { message, .. } => assert!(message.contains("不是密文字符串")),
        other => panic!("应落到 Other，实际 {other:?}"),
    }
}
