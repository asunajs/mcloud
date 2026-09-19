// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

// ── 随机 ──

#[test]
fn login_random_has_the_measured_shape() {
    let mut rng = SeqRandom::new(&[0, 1, 2, 3, 4, 5, 60, 61, 62, 63, 100, 200]);
    let r = new_login_random(&mut rng);
    assert_eq!(r.len(), 6, "实测 `hOjseB` 是 6 位");
    assert!(
        r.chars().all(|c| c.is_ascii_alphanumeric()),
        "字符集是大小写字母 + 数字，实际 {r}"
    );
}

// ── 请求构造 ──

#[test]
fn thirdlogin_request_is_encrypted_and_keeps_json_content_type() {
    let crypto = AppLoginCrypto::production();
    let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
    let request = thirdlogin_request(&profile(), TS, &crypto, &body).unwrap();

    assert_eq!(request.url, "https://user-njs.yun.139.com/user/thirdlogin");
    // 内容是 hex 文本，但 Content-Type 仍然是 json —— 抓包原样如此
    assert_eq!(
        header(&request, "Content-Type"),
        "application/json;charset=UTF-8"
    );

    let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
    // 关键：发出去的**不是明文**
    assert!(!sent.contains("19900000000"), "请求体里不能出现手机号明文");
    assert!(!sent.contains("dycpwd"));
    // 而是可以解回原样的密文
    assert_eq!(crypto.decrypt_body(&sent).unwrap(), body.to_string());
}

#[test]
fn send_sms_code_request_is_plaintext_and_omits_the_cool_flag() {
    let body = send_sms_code_body(PHONE, "");
    let request = send_sms_code_request(&profile(), TS, &body).unwrap();

    assert_eq!(
        request.url,
        "https://user-njs.yun.139.com/user/sms/getSmsCode"
    );
    // 这一条**不加密**：体就是明文 JSON
    let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
    assert!(sent.contains("19900000000"));
    assert_eq!(
        header(&request, "Content-Type"),
        "application/json;charset=UTF-8"
    );
    // 抓包里 App 带着 `hcy-cool-flag`，但实测它会让服务端改回加密响应
    // （详见 `send_sms_code_request` 的说明）。这里钉住"我们不发它"，
    // 免得有人为了"照抄抓包"把它加回来 —— 那会让响应变成解不开的密文。
    assert!(
        !request.headers.iter().any(|(k, _)| k == "hcy-cool-flag"),
        "不能发 hcy-cool-flag：服务端会据此返回本客户端解不开的加密响应"
    );
    // 也没有 yun-encrypt-*（那是别的链路）
    assert!(
        !request
            .headers
            .iter()
            .any(|(k, _)| k.starts_with("yun-encrypt"))
    );
}

/// App 端的滑块取图：**与 `getSmsCode` 同一台主机**、明文、且带全套 App 头。
///
/// 这条断言的意义是钉住"这不是跨端借用接口"——`/user/verfycode` 就在
/// `base::USER_YUN` 上，和发码、登录是同一台。实测裸请求也能用，
/// 这里仍然叠 App 头是因为**取题与提交必须同一套头**才稳妥。
#[test]
fn slider_puzzle_request_hits_the_same_host_as_get_sms_code() {
    let request = slider_puzzle_request(&profile(), TS, PHONE).unwrap();

    assert_eq!(request.url, "https://user-njs.yun.139.com/user/verfycode");
    let sent = String::from_utf8(request.body.clone().unwrap()).unwrap();
    assert_eq!(sent, format!("{{\"account\":\"{PHONE}\",\"type\":2}}"));
    // 与发码同一个主机，不是别的域
    assert_eq!(
        request.url.split("/user/").next(),
        send_sms_code_request(&profile(), TS, &send_sms_code_body(PHONE, ""))
            .unwrap()
            .url
            .split("/user/")
            .next()
    );
    // 设备头与发码一致（取题/提交用同一套）
    assert_eq!(
        header(&request, "service_identifier_key"),
        "service_identifier_userDomain"
    );
    assert!(!header(&request, "x-DeviceInfo").is_empty());
}

/// 带滑块的发码体：`puzzleVerfycode` 必须是**加密后**的坐标，且能解回原样。
#[test]
fn send_sms_code_body_with_slider_encodes_the_answer() {
    let body = send_sms_code_body_with_slider(PHONE, "370").unwrap();

    // 其余字段与空滑块版本完全一致（同一个函数构造）
    let plain = send_sms_code_body(PHONE, "");
    for key in ["mode", "phoneNumber", "reqType", "clientType", "random"] {
        assert_eq!(body[key], plain[key], "字段 {key} 不该变");
    }

    let encoded = body["puzzleVerfycode"].as_str().unwrap();
    assert_ne!(encoded, "370", "不能把坐标明文发上去");
    // 解回来必须是 370 —— 用的是客户端公开常量那套（与 PC 端同一个）
    let decoded =
        mcloud_core::aes_dynamic_iv_decrypt(crate::login::PUZZLE_AES_KEY, encoded).unwrap();
    assert_eq!(decoded.trim(), "370");
}

#[test]
fn app_requests_carry_the_full_device_header_set() {
    let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
    let request = thirdlogin_request(&profile(), TS, &AppLoginCrypto::production(), &body).unwrap();

    // 少了这些头上游回 `9999 登录失败！`（PC 端同款实测）
    assert_eq!(
        header(&request, "service_identifier_key"),
        "service_identifier_userDomain"
    );
    assert_eq!(header(&request, "x-yun-app-channel"), "10000020");
    assert_eq!(header(&request, "x-DeviceName"), "aVBhZA==");
    assert!(
        header(&request, "x-yun-client-info")
            .starts_with("1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|")
    );
    assert!(header(&request, "x-DeviceInfo").ends_with("|0.0000|0.0000|"));
    assert_eq!(
        header(&request, "x-yun-user-agent"),
        "iPad|iPad|ios 26.6|mCloud12.6.0-200"
    );
    assert_eq!(
        header(&request, "User-Agent"),
        "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)"
    );
}
