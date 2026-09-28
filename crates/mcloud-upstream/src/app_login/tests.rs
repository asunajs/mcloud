//! App 端登录协议层的测试。
//!
//! 重点在三处**极易写错且不报错**的地方：
//!
//! 1. 字段类型（字符串 vs 数字）—— 写错了服务端解不开，回 `9103`；
//! 2. `clienttype` 与 `clientType` 的大小写；
//! 3. 请求体是否真的被加密 + 旋转。

use super::*;
use mcloud_crypto::SeqRandom;

// ══════════════════════════════ 加解密 ══════════════════════════════

/// 抓包里那条 `thirdlogin` 明文（186 字节，**抓包原样键序**）。
const CAPTURED_PLAINTEXT: &str = r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#;

/// 上面那串的线上形态（抓包侧独立实现算出）。
const GOLDEN_WIRE: &str = "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db623ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff5600844ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b16c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0";

#[test]
fn production_key_is_the_16_byte_literal() {
    // 密钥必须是 16 字节（→ AES-128）。它不是从配置里读的，是 App 包里的字面量。
    assert_eq!(AppLoginCrypto::production().key(), "lYzvvBMUjucw+h^R");
    assert_eq!(AppLoginCrypto::production().key().len(), 16);
    assert_eq!(AppLoginCrypto::alt_env().key(), "+09T5lHhO+smS7x@");
    assert_eq!(AppLoginCrypto::alt_env().key().len(), 16);
}

#[test]
fn encrypt_body_matches_the_golden_wire() {
    // 这条钉"上游层有没有多加/少加/反做一次错位"。
    let wire = AppLoginCrypto::production()
        .encrypt_body(CAPTURED_PLAINTEXT)
        .unwrap();
    assert_eq!(wire.len(), 384, "186 字节明文 → 192 字节密文 → 384 hex");
    assert_eq!(wire, GOLDEN_WIRE);
    assert_eq!(wire, wire.to_ascii_lowercase(), "请求体是小写 hex");
}

#[test]
fn decrypt_body_reverses_encrypt_body() {
    let c = AppLoginCrypto::production();
    let wire = c.encrypt_body(CAPTURED_PLAINTEXT).unwrap();
    assert_eq!(c.decrypt_body(&wire).unwrap(), CAPTURED_PLAINTEXT);
}

#[test]
fn decrypt_body_trims_surrounding_whitespace() {
    // 上游响应体里偶尔带换行；两个 trim 都要在。
    let c = AppLoginCrypto::production();
    let wire = c.encrypt_body(CAPTURED_PLAINTEXT).unwrap();
    assert_eq!(
        c.decrypt_body(&format!("  {wire}\n")).unwrap(),
        CAPTURED_PLAINTEXT
    );
}

#[test]
fn wrong_rotation_direction_is_rejected_not_silently_garbage() {
    // 最容易犯的错：把请求方向用到响应上。
    // 旋转不改变字符集，所以"忘了反旋转"只会解出乱码或填充非法 ——
    // 必须报错，而不是安静地给出一个看起来正常的字符串。
    let c = AppLoginCrypto::production();
    let ecb =
        mcloud_crypto::aes_ecb_pkcs7_encrypt_hex(APP_LOGIN_AES_KEY, CAPTURED_PLAINTEXT).unwrap();
    assert!(
        c.decrypt_body(&ecb).is_err(),
        "未反旋转的密文必须报错，不能静默解出垃圾"
    );
}

#[test]
fn alt_key_cannot_decrypt_production_ciphertext() {
    // 离线复现 `9103` 那条判据的成因之一：用错密钥。
    let wire = AppLoginCrypto::production()
        .encrypt_body(CAPTURED_PLAINTEXT)
        .unwrap();
    assert!(AppLoginCrypto::alt_env().decrypt_body(&wire).is_err());
}

#[test]
fn decrypt_user_info_parses_the_session_payload() {
    // 成功响应 `data` 是密文，解开后是会话 JSON。
    let c = AppLoginCrypto::production();
    let session = r#"{"authToken":"T|1|RCS|1|x","userDomainId":"1990000000000000001"}"#;
    let wire = c.encrypt_body(session).unwrap();
    let v = c.decrypt_user_info(&wire).unwrap();
    assert_eq!(v["userDomainId"], "1990000000000000001");
}

#[test]
fn decrypt_user_info_reports_bad_json_instead_of_panicking() {
    let c = AppLoginCrypto::production();
    let wire = c.encrypt_body("not json at all").unwrap();
    assert!(c.decrypt_user_info(&wire).is_err());
}

// ══════════════════════════ 请求体形状 ══════════════════════════

#[test]
fn send_sms_code_body_uses_strings_everywhere() {
    // PC 端这里 mode/reqType 是数字 —— App 端是字符串。
    // 这条是"照 PC 抄过去"最直接的拦网。
    let body = send_sms_code_body("19900000000", "");
    assert_eq!(body["mode"], "0");
    assert_eq!(body["reqType"], "3");
    assert_eq!(body["clientType"], "412");
    assert_eq!(body["phoneNumber"], "19900000000");
    assert_eq!(body["random"], "qwert", "固定常量，不是随机值");
    // 首次请求是**空串**，不是 null、也不能省字段
    assert_eq!(body["puzzleVerfycode"], "");
    assert!(body.get("puzzleVerfycode").is_some());

    for key in ["mode", "reqType", "clientType"] {
        assert!(body[key].is_string(), "{key} 必须是字符串（PC 端才是数字）");
    }
    // 字段名是上游自己的拼写错误：verfy 而非 verify
    assert!(body.get("puzzleVerifycode").is_none(), "上游拼的是 verfy");
}

#[test]
fn send_sms_code_body_length_matches_the_capture() {
    // 抓包里那条请求体是 111 字节。
    let body = send_sms_code_body("19900000000", "");
    assert_eq!(body.to_string().len(), 111);
}

#[test]
fn slider_variant_encodes_the_answer() {
    let plain = send_sms_code_body("19900000000", "");
    let with_slider = send_sms_code_body_with_slider("19900000000", "466").unwrap();

    // 除 puzzleVerfycode 外，其余五个字段逐字节相同
    for key in ["mode", "reqType", "clientType", "phoneNumber", "random"] {
        assert_eq!(with_slider[key], plain[key], "{key} 不该被滑块改动");
    }
    let encoded = with_slider["puzzleVerfycode"].as_str().unwrap();
    assert!(!encoded.is_empty(), "带滑块时该字段非空");
    assert_ne!(encoded, "466", "必须是编码后的值，不是明文坐标");
    assert!(
        mcloud_crypto::b64_decode(encoded).unwrap().len() > 16,
        "是 base64(固定IV ‖ 密文)"
    );
}

#[test]
fn thirdlogin_body_shape_and_types() {
    let body = sms_thirdlogin_body("19900000000", "429344", "hOjseB");

    assert_eq!(body["cpid"], "55");
    assert_eq!(body["pintype"], "8", "短信登录也是 8，不是 PC 的 5");
    // dycpwd 是**明文**验证码，不做哈希（账密登录才哈希）
    assert_eq!(body["dycpwd"], "429344");
    assert_eq!(body["loginMode"], "0");
    assert_eq!(body["verType"], "1", "PC 端是 2");
    assert_eq!(body["version"], "12.6.0");
    assert_eq!(body["msisdn"], "19900000000");
    assert_eq!(body["random"], "hOjseB");
    // extInfo.ifOpenAccount 是**字符串**（PC 端是数字 1）
    assert_eq!(body["extInfo"]["ifOpenAccount"], "1");
    assert!(body["extInfo"]["ifOpenAccount"].is_string());
}

#[test]
fn thirdlogin_uses_lowercase_clienttype_and_has_no_camelcase_twin() {
    // 大小写是这条链路最尖锐的坑：getSmsCode 用 clientType，
    // thirdlogin 用 clienttype，且**没有**大写那个键。
    let body = sms_thirdlogin_body("19900000000", "429344", "hOjseB");
    assert!(body.get("clienttype").is_some(), "thirdlogin 用小写 t");
    assert_eq!(body["clienttype"], "412");
    assert!(
        body.get("clientType").is_none(),
        "thirdlogin 不能出现 clientType"
    );
}

#[test]
fn thirdlogin_plaintext_serialization_is_186_bytes() {
    // 键序由 serde_json 决定（字母序），长度与抓包一致。
    let body = sms_thirdlogin_body("19900000000", "429344", "hOjseB");
    let s = body.to_string();
    assert_eq!(s.len(), 186, "长度是形状哨兵：{s}");
    // ⚠ 长度只是哨兵，不是判据 —— 键序不同但长度相同的两份明文
    // 密文完全不同，所以真正的判据是下面这条整串比对。
    assert_ne!(s, CAPTURED_PLAINTEXT, "键序不同（这正是两条黄金值的由来）");
    assert_eq!(s.len(), CAPTURED_PLAINTEXT.len());
}

#[test]
fn thirdlogin_plaintext_is_byte_identical_to_the_live_probe() {
    // 这条把"我实测跑通的活体探针"和"实现产出的串"接起来：
    //   探针送出这串 → 上游回 9441 → 服务端**成功解密**了它
    //   而实现产出的串逐字节等于它 → 所以实现产出的体也一定解得开
    // 一旦有人改了字段名/类型/键序策略，这条先炸，而不是等一次真实登录。
    let body = sms_thirdlogin_body("19900000000", "429344", "hOjseB");
    assert_eq!(
        body.to_string(),
        r#"{"clienttype":"412","cpid":"55","dycpwd":"429344","extInfo":{"ifOpenAccount":"1"},"loginMode":"0","msisdn":"19900000000","pintype":"8","random":"hOjseB","verType":"1","version":"12.6.0"}"#
    );
}

#[test]
fn new_login_random_is_six_alphanumeric_chars() {
    let mut rng = SeqRandom::new(1);
    let r = new_login_random(&mut rng);
    assert_eq!(r.len(), 6);
    assert!(r.chars().all(|c| c.is_ascii_alphanumeric()), "{r}");
}

#[test]
fn new_login_random_is_not_the_send_sms_constant() {
    // 两个 random 不是同一个值：发码那个是硬编码常量。
    let mut rng = SeqRandom::new(7);
    assert_ne!(new_login_random(&mut rng), SEND_SMS_RANDOM);
}

// ══════════════════════════ 请求构造 ══════════════════════════

fn body_of(req: &HttpRequest) -> String {
    req.body_str().unwrap_or_default().to_string()
}

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[test]
fn requests_go_to_the_right_endpoints() {
    let body = send_sms_code_body("19900000000", "");
    let a = send_sms_code_request("D", 1, &body);
    assert_eq!(a.url, SEND_SMS_CODE);
    assert_eq!(a.method, "POST");

    let b = slider_puzzle_request("D", 1, "19900000000");
    assert_eq!(b.url, SLIDER_PUZZLE);
    assert_eq!(b.method, "POST");

    let c = thirdlogin_request(
        "D",
        1,
        &AppLoginCrypto::production(),
        &sms_thirdlogin_body("19900000000", "429344", "hOjseB"),
    )
    .unwrap();
    assert_eq!(c.url, THIRD_LOGIN);
    assert_eq!(c.method, "POST");
}

#[test]
fn slider_puzzle_body_type_is_the_number_two() {
    // 整条 App 链路里**唯一**的数字字段。写成字符串服务端不认。
    let req = slider_puzzle_request("D", 1, "19900000000");
    let sent = body_of(&req);
    assert_eq!(sent, r#"{"account":"19900000000","type":2}"#);
}

#[test]
fn thirdlogin_body_is_encrypted_on_the_wire() {
    let crypto = AppLoginCrypto::production();
    let req = thirdlogin_request(
        "D",
        1,
        &crypto,
        &sms_thirdlogin_body("19900000000", "429344", "hOjseB"),
    )
    .unwrap();
    let sent = body_of(&req);

    // 明文特征绝不能出现在线上体里
    assert!(!sent.contains("19900000000"), "手机号不该明文上网");
    assert!(!sent.contains("dycpwd"), "字段名不该明文上网");
    assert!(!sent.contains("429344"), "验证码不该明文上网");
    // 而且是可逆的 —— 证明密钥/模式/旋转都对
    assert_eq!(sent.len(), 384);
    assert!(crypto.decrypt_body(&sent).is_ok());
    assert!(crypto.decrypt_body(&sent).unwrap().contains("dycpwd"));
}

#[test]
fn exactly_one_content_type_header_is_sent() {
    // json_body 会自己加一个 content-type；若不先删，就会有两个，
    // 而 reqwest 会把它们拼接起来，上游读第一个（错的那个）。
    let body = send_sms_code_body("19900000000", "");
    for req in [
        send_sms_code_request("D", 1, &body),
        slider_puzzle_request("D", 1, "19900000000"),
        thirdlogin_request(
            "D",
            1,
            &AppLoginCrypto::production(),
            &sms_thirdlogin_body("19900000000", "429344", "hOjseB"),
        )
        .unwrap(),
    ] {
        let n = req
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .count();
        assert_eq!(n, 1, "必须恰好一个 Content-Type：{:?}", req.url);
        assert_eq!(
            header(&req, "content-type").unwrap(),
            "application/json;charset=UTF-8",
            "加密体的 Content-Type 仍是 json（抓包原样）"
        );
    }
}

#[test]
fn app_headers_carry_the_measured_values() {
    let req = slider_puzzle_request("DEV1", 1789743328974257, "19900000000");
    assert_eq!(header(&req, "x-yun-app-channel").unwrap(), "10000020");
    assert_eq!(header(&req, "x-huawei-channelSrc").unwrap(), "10000020");
    assert_eq!(header(&req, "x-yun-net-type").unwrap(), "1");
    assert_eq!(header(&req, "x-yun-svc-type").unwrap(), "1");
    assert_eq!(header(&req, "x-SvcType").unwrap(), "1");
    assert_eq!(header(&req, "x-yun-api-version").unwrap(), "v1");
    assert_eq!(header(&req, "Accept-Language").unwrap(), "zh-CN");
    assert_eq!(
        header(&req, "service_identifier_key").unwrap(),
        "service_identifier_userDomain"
    );
    assert_eq!(header(&req, "x-DeviceName").unwrap(), "aVBhZA==");
    assert_eq!(
        header(&req, "x-yun-user-timestamp").unwrap(),
        "1789743328974257"
    );
    assert_eq!(header(&req, "User-Agent").unwrap(), APP_USER_AGENT);
    assert_eq!(header(&req, "x-UserAgent").unwrap(), APP_X_USER_AGENT);
    assert_eq!(
        header(&req, "x-yun-user-agent").unwrap(),
        APP_X_YUN_USER_AGENT
    );
}

#[test]
fn device_info_and_client_info_differ_on_the_app_side() {
    // PC 端这两个头同值；App 端**不同** —— 后者多两段。
    let info = app_device_info("DEV1");
    let client = app_client_info("DEV1");
    assert_ne!(info, client, "App 端这两个头不相等（PC 端才相等）");
    assert!(client.starts_with(&info));
    assert!(client.ends_with("aVBhZA==|0|;"));
    // 14 段、最后一段是空的 —— 所以分隔符是 13 个而不是 14 个。
    let segs: Vec<&str> = info.split('|').collect();
    assert_eq!(segs.len(), 14, "14 段：{info}");
    assert_eq!(segs[13], "", "末尾是空段");
    assert_eq!(segs[0], "1");
    assert_eq!(segs[1], "192.168.3.179");
    assert_eq!(segs[2], "2");
    assert_eq!(segs[3], "12.6.0");
    assert_eq!(segs[4], "APPLE");
    assert_eq!(segs[5], "iPad8,6");
    assert_eq!(segs[6], "DEV1");
    assert_eq!(segs[8], "ipados 26.6");
    assert_eq!(segs[9], "375 * 667");
    assert_eq!(segs[10], "zh-Hans-CN");
    assert_eq!(segs[11], "0.0000");
    assert_eq!(segs[12], "0.0000");
    // 客户端信息 = 设备串 + x-DeviceName + `0|;`，各占一段
    let csegs: Vec<&str> = client.split('|').collect();
    assert_eq!(csegs.len(), 16, "16 段：{client}");
    assert_eq!(csegs[13], "aVBhZA==");
    assert_eq!(csegs[14], "0");
    assert_eq!(csegs[15], ";");
}

#[test]
fn app_requests_never_send_hcy_cool_flag() {
    // 实测：带这个头会让响应变成一段我们解不开的 144 字节密文。
    // 不发它时语义与抓包一致（同样回 200059554）。
    let body = send_sms_code_body("19900000000", "");
    for req in [
        send_sms_code_request("D", 1, &body),
        slider_puzzle_request("D", 1, "19900000000"),
        thirdlogin_request(
            "D",
            1,
            &AppLoginCrypto::production(),
            &sms_thirdlogin_body("19900000000", "429344", "hOjseB"),
        )
        .unwrap(),
    ] {
        assert!(
            header(&req, "hcy-cool-flag").is_none(),
            "不能发 hcy-cool-flag：{:?}",
            req.url
        );
        assert!(
            !req.headers
                .iter()
                .any(|(k, _)| k.to_ascii_lowercase().starts_with("yun-encrypt")),
            "登录链路不走国密那条路"
        );
    }
}

// ══════════════════════════ 响应归类 ══════════════════════════

fn env(json: &str) -> LoginEnvelope {
    LoginEnvelope::parse(json).unwrap()
}

#[test]
fn classify_send_sms_reads_sent_and_remains() {
    let e =
        env(r#"{"success":true,"code":"0000","message":"请求成功","data":{"remainGetTimes":8}}"#);
    assert_eq!(
        classify_send_sms(&e),
        AppSendSmsOutcome::Sent {
            remain_get_times: Some(8)
        }
    );
}

#[test]
fn classify_send_sms_treats_need_slider_as_normal_not_error() {
    // 首次发码的**常态**，不是错误。
    let e = env(
        r#"{"success":false,"code":"200059554","message":"请先获取滑块拼图验证码","data":{"captchaType":2}}"#,
    );
    assert_eq!(
        classify_send_sms(&e),
        AppSendSmsOutcome::NeedCaptcha {
            captcha_type: Some(2)
        }
    );
}

#[test]
fn classify_send_sms_keeps_exhausted_separate_from_need_slider() {
    // ⚠ 这一支必须与上一支分开：合并会让调用方拿**已经作废的一道题**
    // 反复重试，而这个接口是计失败次数的。
    let e = env(
        r#"{"success":false,"code":"200059553","message":"校验失败次数超限","data":{"captchaType":1}}"#,
    );
    assert_eq!(
        classify_send_sms(&e),
        AppSendSmsOutcome::SliderExhausted {
            captcha_type: Some(1)
        }
    );
    // 与 NeedCaptcha 是两个不同结论
    assert_ne!(
        classify_send_sms(&e),
        AppSendSmsOutcome::NeedCaptcha {
            captcha_type: Some(1)
        }
    );
}

#[test]
fn classify_send_sms_falls_through_for_unknown_codes() {
    let e = env(r#"{"success":false,"code":"12345","message":"别的东西"}"#);
    assert_eq!(
        classify_send_sms(&e),
        AppSendSmsOutcome::Other {
            code: Some(12345),
            message: "别的东西".to_string()
        }
    );
}

#[test]
fn classify_login_maps_the_binary_discriminator() {
    // 9441 = 体解开了、码不对；9103 = 体没解开。这两条是调加密时的判据。
    assert_eq!(
        classify_login(&env(r#"{"code":"9441","message":"验证码不正确"}"#)),
        AppLoginOutcome::SmsCodeWrong
    );
    assert_eq!(
        classify_login(&env(r#"{"code":"9103","message":"输入参数不合法"}"#)),
        AppLoginOutcome::Undecryptable
    );
    assert_eq!(
        classify_login(&env(r#"{"code":"9999","message":"登录失败！"}"#)),
        AppLoginOutcome::LoginFailed
    );
}

#[test]
fn classify_login_success_requires_a_cipher_string() {
    let e = env(r#"{"success":true,"code":"0000","message":"请求成功","data":"deadbeef"}"#);
    assert_eq!(
        classify_login(&e),
        AppLoginOutcome::Success {
            encrypted: "deadbeef".to_string()
        }
    );
}

#[test]
fn classify_login_success_with_non_string_data_is_reported_not_faked() {
    // 成功码但载荷形状不对：如实报告，**不伪造空凭据**。
    let e = env(r#"{"success":true,"code":"0000","message":"请求成功","data":{"a":1}}"#);
    match classify_login(&e) {
        AppLoginOutcome::Other { code, message } => {
            assert_eq!(code, Some(0));
            assert!(message.contains("不是密文字符串"), "{message}");
        }
        other => panic!("不该归成 {other:?}"),
    }
}

#[test]
fn envelope_accepts_numeric_and_string_codes() {
    // 上游两种形态都出现过（`"0000"` 与 `0`）。
    assert_eq!(
        classify_login(&env(r#"{"code":0,"data":"x"}"#)),
        AppLoginOutcome::Success {
            encrypted: "x".to_string()
        }
    );
    assert_eq!(
        classify_login(&env(r#"{"code":"0","data":"x"}"#)),
        AppLoginOutcome::Success {
            encrypted: "x".to_string()
        }
    );
}
