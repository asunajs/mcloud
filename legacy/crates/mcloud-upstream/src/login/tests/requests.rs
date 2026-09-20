// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

#[test]
fn qr_payload_matches_captured_format() {
    // 协议 §2：`QRcodeUrl?dID=&sID=&cType=11&ver=` —— 参数顺序也是实测的
    let payload = qr_payload(
        PcPlatform::Windows,
        "62AA2BDA29F99744536E142A83CA5B03-ENDIN",
        "A1B2",
    );
    assert_eq!(
        payload,
        "https://yun.139.com/w/#/qrcLogin?dID=62AA2BDA29F99744536E142A83CA5B03-ENDIN&sID=A1B2&cType=11&ver=8.8.6.20260829"
    );
}

#[test]
fn session_id_is_32_uppercase_hex() {
    let mut rng = SeqRandom(0);
    let sid = new_session_id(&mut rng);
    assert_eq!(sid.len(), 32, "sID 必须是 32 位：{sid}");
    assert!(
        sid.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
        "sID 必须是大写十六进制：{sid}"
    );
    assert_eq!(sid, "000102030405060708090A0B0C0D0E0F");
}

#[test]
fn sms_random_is_24_hex_not_uuid() {
    let mut rng = SeqRandom(0);
    let random = new_sms_random(&mut rng);
    assert_eq!(
        random.len(),
        24,
        "实测是 24 位，不是 UUID 的 36 位：{random}"
    );
    assert!(random.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!random.contains('-'));
}

#[test]
fn qr_thirdlogin_body_matches_captured_request() {
    // 逐字段对齐协议 §3 的实测请求体
    let body = qr_thirdlogin_body(PcPlatform::Windows, "A1B2C3");
    assert_eq!(
        body,
        json!({
            "clientType": "656", "cpid": "277", "dycpwd": "A1B2C3",
            "extInfo": { "ifOpenAccount": 0 }, "loginMode": "0", "msisdn": "",
            "pintype": "21", "verType": "2", "version": "8.8.6.20260829"
        })
    );
}

#[test]
fn sms_thirdlogin_body_matches_captured_request() {
    // 协议 §2.4：pintype=5、dycpwd 是验证码明文、msisdn 是手机号、ifOpenAccount=1
    let body = sms_thirdlogin_body(PcPlatform::Windows, "19900000000", "119301");
    assert_eq!(
        body,
        json!({
            "clientType": "656", "cpid": "277", "dycpwd": "119301",
            "extInfo": { "ifOpenAccount": 1 }, "loginMode": "0", "msisdn": "19900000000",
            "pintype": "5", "verType": "2", "version": "8.8.6.20260829"
        })
    );
}

#[test]
fn macos_platform_uses_its_own_client_type_cpid_and_version() {
    // 源码 L11-12：Windows 656/277，macOS 710/332。传错平台 = 冒充另一个客户端。
    assert_eq!(PcPlatform::MacOs.client_type(), "710");
    assert_eq!(PcPlatform::MacOs.cpid(), "332");
    // 版本号也必须跟着平台走：mac 的 clientType 配 Windows 的版本号是自相矛盾
    assert_eq!(PcPlatform::Windows.app_version(), "8.8.6.20260829");
    assert_eq!(PcPlatform::MacOs.app_version(), "4.2.1");

    let body = qr_thirdlogin_body(PcPlatform::MacOs, "S");
    assert_eq!(body["clientType"], "710");
    assert_eq!(body["cpid"], "332");
    assert_eq!(body["version"], "4.2.1");
    assert!(
        qr_payload(PcPlatform::MacOs, "d", "s").ends_with("ver=4.2.1"),
        "二维码里的 ver 也要跟平台一致"
    );
}

#[test]
fn send_sms_code_body_matches_captured_request() {
    let body = send_sms_code_body(
        PcPlatform::Windows,
        "19900000000",
        "0aebc7fd57048e7b92fbd22efe",
    );
    assert_eq!(
        body,
        json!({
            "phoneNumber": "19900000000",
            "random": "0aebc7fd57048e7b92fbd22efe",
            "mode": 0, "reqType": 3, "clientType": "656"
        })
    );
}

#[test]
fn slider_and_logout_bodies_match_captured_requests() {
    assert_eq!(
        slider_puzzle_body("19900000000"),
        json!({"account": "19900000000", "type": 2})
    );
    assert_eq!(
        logout_body("19900000000"),
        json!({"phoneNumber": "19900000000"})
    );
}
