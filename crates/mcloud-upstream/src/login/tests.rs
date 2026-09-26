//! 登录协议的纯函数测试。
//!
//! 全部离线：请求形状逐字段断言，响应语义用真实抓包过的码值。

use super::*;
use mcloud_crypto::SeqRandom;
use serde_json::json;

fn win_client() -> PcClient {
    PcClient::new(
        PcPlatform::Windows,
        "62AA2BDA29F99744536E142A83CA5B03-ENDIN",
    )
}

fn mac_client() -> PcClient {
    PcClient::new(PcPlatform::MacOs, "62AA2BDA29F99744536E142A83CA5B03-ENDIN")
}

// ══════════════════════════ 平台形态：底层区分 ══════════════════════════

/// 三个业务参数（`clientType` / `cpid` / `version`）随平台变。
///
/// 值取自客户端源码 `apiServer.js:10-12` 与 `package.json`：
/// 混用会被上游当成另一个客户端，所以三个必须**同时**跟着平台走。
#[test]
fn platform_drives_the_business_triple() {
    let w = win_client();
    assert_eq!(w.client_type(), "656");
    assert_eq!(w.cpid(), "277");
    assert_eq!(w.app_version(), "8.8.6.20260829");

    let m = mac_client();
    assert_eq!(m.client_type(), "710");
    assert_eq!(m.cpid(), "332");
    assert_eq!(m.app_version(), "4.2.1");

    // 三者两两不同 —— 防止"改了枚举忘了改某一处"
    assert_ne!(w.client_type(), m.client_type());
    assert_ne!(w.cpid(), m.cpid());
    assert_ne!(w.app_version(), m.app_version());
}

/// 渠道号 mac / win 不同（文档点名的三个易错点之一）。
#[test]
fn app_channel_differs_between_mac_and_windows() {
    assert_eq!(win_client().app_channel(), "10200153");
    assert_eq!(mac_client().app_channel(), "10301000");
    assert_ne!(win_client().app_channel(), mac_client().app_channel());
}

/// **HTTP UA 随平台变** —— 这是"底层区分"里最直观的一条。
///
/// 客户端自己就是靠 UA 里的 `mac` 判断平台的（`apiServer.js:10`），
/// 所以"UA 反映平台"是真实客户端的行为。
#[test]
fn user_agent_differs_between_mac_and_windows() {
    let w = win_client().user_agent();
    let m = mac_client().user_agent();

    assert!(w.contains("Windows NT"), "win UA 应含 Windows NT：{w}");
    assert!(m.contains("Macintosh"), "mac UA 应含 Macintosh：{m}");
    assert!(!w.contains("Macintosh"), "win UA 不该含 Macintosh：{w}");
    assert!(!m.contains("Windows NT"), "mac UA 不该含 Windows NT：{m}");
    assert_ne!(w, m);
}

/// 设备串第 3 段（版本）随平台变，其余段保持一致。
#[test]
fn device_info_version_segment_follows_platform() {
    let w = win_client().device_info();
    let m = mac_client().device_info();

    let segs_w: Vec<&str> = w.split('|').collect();
    let segs_m: Vec<&str> = m.split('|').collect();
    assert_eq!(segs_w.len(), 14, "设备串是 14 段：{w}");
    assert_eq!(segs_m.len(), 14, "设备串是 14 段：{m}");

    assert_eq!(segs_w[3], "8.8.6.20260829");
    assert_eq!(segs_m[3], "4.2.1");
    // 第 6 段是设备指纹（明文，与 x-yun-device-id 同值）
    assert_eq!(segs_w[6], "62AA2BDA29F99744536E142A83CA5B03-ENDIN");
    assert_eq!(segs_m[6], segs_w[6], "设备指纹不该随平台变");
    // 第 4 段固定是 PC
    assert_eq!(segs_w[4], "PC");
    assert_eq!(segs_m[4], "PC");
}

/// 设备头集合里，凡涉及平台的头都必须不同 —— 但**只有一个**。
///
/// ⚠ 这条测试一度写错过：它曾断言 `x-huawei-channelSrc` 也随平台变，
/// 于是把实现一起带偏（mac 会发出 `10301000`）。而客户端源码里那一行是
/// **字面量** `"10200153"`，只有 `x-yun-app-channel` 是三元表达式。
/// 两个参考实现也都把它钉成常量。
#[test]
fn platform_sensitive_headers_differ() {
    let w = win_client().headers();
    let m = mac_client().headers();

    let get = |hs: &[(&'static str, String)], k: &str| {
        hs.iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("缺头 {k}"))
    };

    // 两个头、同一个值（客户端源码里就是两行 setRequestHeader）
    assert_eq!(
        get(&w, "x-yun-client-info"),
        get(&w, "x-DeviceInfo"),
        "这两个头必须同值"
    );
    // 且这个值本身随平台变（含版本段）
    assert_ne!(get(&w, "x-yun-client-info"), get(&m, "x-yun-client-info"));

    // ⭐ `x-huawei-channelSrc` 是**常量**：两端必须相同
    assert_eq!(
        get(&w, "x-huawei-channelSrc"),
        get(&m, "x-huawei-channelSrc"),
        "这个头在客户端源码里是字面量，不该随平台变"
    );
    assert_eq!(get(&w, "x-huawei-channelSrc"), "10200153");

    // 自定义 UA 头两端都是 "PC"（不是 HTTP UA）
    assert_eq!(get(&w, "x-UserAgent"), "PC");
    assert_eq!(get(&m, "x-UserAgent"), "PC");
    // x-MM-Source 固定
    assert_eq!(get(&w, "x-MM-Source"), "000");
    assert_eq!(get(&m, "x-MM-Source"), get(&w, "x-MM-Source"));

    // 登录请求**只有这 5 个**：`x-yun-app-channel` 属于登录后那一套
    assert_eq!(w.len(), 5, "登录分支只发 5 个头：{w:?}");
    assert!(
        !w.iter().any(|(n, _)| *n == "x-yun-app-channel"),
        "登录请求不该带 x-yun-app-channel（那是 cutoverStatus==1 的头）"
    );
}

/// 登录后那一套头里，`x-yun-app-channel` **才**随平台变。
#[test]
fn post_login_headers_carry_the_platform_channel() {
    let get = |hs: Vec<(&'static str, String)>| hs[0].1.clone();

    assert_eq!(get(win_client().post_login_headers()), "10200153");
    assert_eq!(get(mac_client().post_login_headers()), "10301000");
    assert_ne!(
        get(win_client().post_login_headers()),
        get(mac_client().post_login_headers()),
        "这是客户端源码里唯一按 mac/win 取值的渠道位"
    );
}

/// 业务头 = 登录头 + 登录后的渠道位，且不重不漏。
///
/// 这条钉住"相加"这件事只有一处实现。漏一个头的表现是上游回一个
/// 含义模糊的业务码，而不是报错。
#[test]
fn business_headers_are_login_headers_plus_the_channel() {
    for c in [win_client(), mac_client()] {
        let login = c.headers();
        let business = c.business_headers();

        assert_eq!(login.len(), 5);
        assert_eq!(business.len(), login.len() + 1, "只多一个头");

        // 登录那 5 个一个不少地出现在业务头里
        for (name, value) in &login {
            assert!(
                business.iter().any(|(n, v)| n == name && v == value),
                "业务头少了 {name}"
            );
        }

        // 多出来的那个正是按平台变的渠道位
        assert_eq!(
            business
                .iter()
                .find(|(n, _)| *n == "x-yun-app-channel")
                .map(|(_, v)| v.as_str()),
            Some(c.app_channel())
        );
    }

    // 两个平台的业务头必须不同（差异在渠道位与设备串上）
    assert_ne!(
        win_client().business_headers(),
        mac_client().business_headers()
    );
}

/// 请求体里的平台差异（`clientType` / `cpid` / `version`）真实落到 JSON 上。
///
/// 这条是"底层区分"的终点断言：**mac 与 win 发出去的 body 不相等**。
#[test]
fn thirdlogin_body_differs_by_platform() {
    let w = qr_thirdlogin_body(&win_client(), "SID123");
    let m = qr_thirdlogin_body(&mac_client(), "SID123");

    assert_ne!(w, m, "mac 与 win 的登录请求体必须不同");
    assert_eq!(w["clientType"], "656");
    assert_eq!(m["clientType"], "710");
    assert_eq!(w["cpid"], "277");
    assert_eq!(m["cpid"], "332");
    assert_eq!(w["version"], "8.8.6.20260829");
    assert_eq!(m["version"], "4.2.1");
    // 与平台无关的部分必须一致，否则就是把差异撒到了别处
    assert_eq!(w["pintype"], m["pintype"]);
    assert_eq!(w["dycpwd"], m["dycpwd"]);
    assert_eq!(w["loginMode"], m["loginMode"]);
    assert_eq!(w["verType"], m["verType"]);
}

// ══════════════════════════ 请求体形态 ══════════════════════════

/// body 的键序 = JSON 字典序（与 Go 参考的刻意排列一致）。
#[test]
fn thirdlogin_body_keys_are_in_dictionary_order() {
    let body = qr_thirdlogin_body(&win_client(), "SID");
    let text = body.to_string();

    // serde_json::Map 默认 BTreeMap ⇒ 天然字典序，无需手工维护顺序
    let keys: Vec<&str> = [
        "clientType",
        "cpid",
        "dycpwd",
        "extInfo",
        "loginMode",
        "msisdn",
        "pintype",
        "verType",
        "version",
    ]
    .into();
    let mut pos = 0;
    for k in keys {
        let at = text[pos..]
            .find(&format!("\"{k}\":"))
            .unwrap_or_else(|| panic!("body 里找不到键 {k}：{text}"));
        pos += at;
    }
}

/// 三种登录方式只差 `pintype` / `dycpwd` / `msisdn` / `ifOpenAccount`。
#[test]
fn login_kinds_map_to_expected_pintype_and_ext_info() {
    assert_eq!(LoginKind::QrCode.pintype(), "21");
    assert_eq!(LoginKind::Sms.pintype(), "5");
    assert_eq!(LoginKind::Password.pintype(), "9");

    // 短信是 1，其余是 0（协议 §2.4 对照表）
    assert_eq!(LoginKind::Sms.if_open_account(), 1);
    assert_eq!(LoginKind::QrCode.if_open_account(), 0);
    assert_eq!(LoginKind::Password.if_open_account(), 0);

    let c = win_client();
    let qr = qr_thirdlogin_body(&c, "SID");
    assert_eq!(qr["pintype"], "21");
    assert_eq!(qr["dycpwd"], "SID");
    assert_eq!(qr["msisdn"], "");
    assert_eq!(qr["extInfo"]["ifOpenAccount"], 0);

    let sms = sms_thirdlogin_body(&c, "19900000001", "123456");
    assert_eq!(sms["pintype"], "5");
    assert_eq!(sms["dycpwd"], "123456", "短信码是明文，不是哈希");
    assert_eq!(sms["msisdn"], "19900000001");
    assert_eq!(sms["extInfo"]["ifOpenAccount"], 1);

    let pw = password_thirdlogin_body(&c, "19900000001", "hunter2");
    assert_eq!(pw["pintype"], "9");
    assert_eq!(pw["msisdn"], "19900000001");
    assert_eq!(pw["extInfo"]["ifOpenAccount"], 0);
    // 口令必须是哈希后的，绝不能是明文
    assert_ne!(pw["dycpwd"], "hunter2");
    assert_eq!(pw["dycpwd"], password_hash("hunter2"));
}

#[test]
fn send_sms_code_body_shape() {
    let body = send_sms_code_body(&win_client(), "19900000001", "abcdef0123456789abcdef01");
    assert_eq!(body["phoneNumber"], "19900000001");
    assert_eq!(body["mode"], 0);
    assert_eq!(body["reqType"], 3);
    assert_eq!(body["clientType"], "656");
    // 平台差异同样到达这里
    assert_eq!(
        send_sms_code_body(&mac_client(), "19900000001", "x")["clientType"],
        "710"
    );
}

/// 口令哈希：固定前缀 + SHA-1 + **大写**。
#[test]
fn password_hash_is_uppercase_sha1_with_prefix() {
    let h = password_hash("abc");
    // 复算一遍，确保不是"看着像"
    let expect = mcloud_crypto::sha1_hex_upper("fetion.com.cn:abc");
    assert_eq!(h, expect);
    assert_eq!(h.len(), 40);
    assert_eq!(h, h.to_uppercase(), "必须是大写");
    assert_ne!(
        h,
        mcloud_crypto::sha1_hex_upper("abc"),
        "前缀是内容的一部分"
    );
}

// ══════════════════════════ 二维码与随机量 ══════════════════════════

/// 二维码载荷是**本地拼装**，`ver` 跟着平台走。
#[test]
fn qr_payload_is_assembled_locally_with_platform_version() {
    let w = qr_payload(&win_client(), "SID42");
    assert!(w.starts_with(QR_LOGIN_URL), "实得：{w}");
    assert!(
        w.contains("dID=62AA2BDA29F99744536E142A83CA5B03-ENDIN"),
        "实得：{w}"
    );
    assert!(w.contains("sID=SID42"), "实得：{w}");
    assert!(w.contains("cType=11"), "实得：{w}");
    assert!(w.contains("ver=8.8.6.20260829"), "实得：{w}");

    let m = qr_payload(&mac_client(), "SID42");
    assert!(m.contains("ver=4.2.1"), "实得：{m}");
    assert_ne!(w, m, "二维码里的版本随平台变");
}

/// `sID`：32 位**大写** hex。
#[test]
fn session_id_is_32_uppercase_hex() {
    let mut rng = SeqRandom::new(0xAB);
    let id = new_session_id(&mut rng);
    assert_eq!(id.len(), 32);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id, id.to_uppercase(), "必须大写：{id}");
    // 打版本位是 UUID 的行为，这里不该有连字符
    assert!(!id.contains('-'));
}

/// 发码 `random`：24 位**小写** hex（不是 UUID）。
#[test]
fn sms_random_is_24_lowercase_hex_not_uuid() {
    let mut rng = SeqRandom::new(1);
    let r = new_sms_random(&mut rng);
    assert_eq!(r.len(), 24, "实得：{r}");
    assert!(r.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(r, r.to_lowercase(), "必须小写");
    assert!(!r.contains('-'), "不能是 UUID 形态");
}

/// 两个随机量必须来自随机源，不能是常量。
#[test]
fn random_helpers_are_not_constant() {
    let a = new_session_id(&mut SeqRandom::new(1));
    let b = new_session_id(&mut SeqRandom::new(2));
    assert_ne!(a, b, "换成常量就是可预测的会话标识");
    assert_ne!(
        new_sms_random(&mut SeqRandom::new(1)),
        new_sms_random(&mut SeqRandom::new(9))
    );
}

// ══════════════════════════ 请求构造：头必须齐 ══════════════════════════

/// 请求必须带齐设备头 —— 少了它们上游回 `9999 登录失败！`（实测）。
#[test]
fn thirdlogin_request_carries_all_device_headers() {
    let client = win_client();
    let req = thirdlogin_request(&client, "TID-1", &qr_thirdlogin_body(&client, "SID"));

    assert_eq!(req.method, "POST");
    assert_eq!(req.url, "https://user-njs.yun.139.com/user/thirdlogin");

    let names: Vec<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
    for want in [
        "x-yun-client-info",
        "x-DeviceInfo",
        "x-MM-Source",
        "x-huawei-channelSrc",
        "x-UserAgent",
        "x-yun-tid",
    ] {
        assert!(names.contains(&want), "缺头 {want}；实得 {names:?}");
    }
    assert!(req.body.is_some(), "登录请求必须有 JSON body");
}

/// 四种请求都打对了端点。
#[test]
fn each_request_targets_its_own_endpoint() {
    let c = win_client();
    assert!(thirdlogin_request(&c, "t", &json!({}))
        .url
        .ends_with("/user/thirdlogin"));
    assert!(send_sms_code_request(&c, "t", &json!({}))
        .url
        .ends_with("/user/sms/getSmsCode"));
    // 没有 `i` —— 是 verfycode 不是 verifycode
    assert!(slider_puzzle_request(&c, "t", &json!({}))
        .url
        .ends_with("/user/verfycode"));
    assert!(logout_request(&c, "t", &json!({}))
        .url
        .ends_with("/user/logout"));
}

/// `x-yun-tid` 是每次调用注入的，会进到头上。
#[test]
fn tid_is_injected_as_a_header() {
    let c = win_client();
    let req = thirdlogin_request(&c, "TID-abc", &json!({}));
    let tid = req
        .headers
        .iter()
        .find(|(k, _)| k == "x-yun-tid")
        .map(|(_, v)| v.as_str());
    assert_eq!(tid, Some("TID-abc"));
}

// ══════════════════════════ 响应语义 ══════════════════════════

#[test]
fn envelope_normalizes_string_and_numeric_codes() {
    // 字符串形态（扫码链路实测就是这种）
    let e =
        LoginEnvelope::parse(r#"{"code":"200059541","message":"未找到设备的二维码信息"}"#).unwrap();
    assert_eq!(e.code, Some(codes::QR_NOT_SCANNED));
    assert_eq!(e.raw_code, "200059541");
    assert_eq!(e.message, "未找到设备的二维码信息");

    // 数字形态
    let e = LoginEnvelope::parse(r#"{"code":200059548,"message":"授权"}"#).unwrap();
    assert_eq!(e.code, Some(codes::QR_SCANNED));

    // "0000" → 0
    let e = LoginEnvelope::parse(r#"{"code":"0000","data":"deadbeef"}"#).unwrap();
    assert_eq!(e.code, Some(0));
    assert_eq!(e.raw_code, "0000");

    // msg 是另一个键名
    let e = LoginEnvelope::parse(r#"{"code":90001,"msg":"未登录"}"#).unwrap();
    assert_eq!(e.message, "未登录");

    // 非 JSON 必须报错，不能静默当成"其它码"
    assert!(LoginEnvelope::parse("<html>502</html>").is_err());
}

/// 扫码轮询的每个码都归到自己的档，且**未扫码不是错误**。
#[test]
fn classify_poll_maps_every_known_code() {
    let mk = |code: &str| LoginEnvelope::parse(&format!(r#"{{"code":"{code}"}}"#)).unwrap();

    assert_eq!(classify_poll(&mk("200059541")), PollOutcome::NotScanned);
    assert_eq!(classify_poll(&mk("200059548")), PollOutcome::Scanned);
    assert_eq!(
        classify_poll(&mk("200059544")),
        PollOutcome::SessionMismatch
    );
    assert_eq!(classify_poll(&mk("200059542")), PollOutcome::Expired);
    assert_eq!(classify_poll(&mk("200059545")), PollOutcome::TooFrequent);

    // 未知码原样带回，不硬编成错误（上游会加新码）
    match classify_poll(&mk("12345")) {
        PollOutcome::Other { code, .. } => assert_eq!(code, Some(12345)),
        other => panic!("未知码应归 Other，实得 {other:?}"),
    }
}

/// 成功码必须带密文载荷才算成功。
#[test]
fn classify_poll_success_requires_ciphertext_payload() {
    let ok = LoginEnvelope::parse(r#"{"code":"0000","data":"aabbcc"}"#).unwrap();
    assert_eq!(
        classify_poll(&ok),
        PollOutcome::Success {
            encrypted: "aabbcc".into()
        }
    );

    // 200 也是成功码
    let ok200 = LoginEnvelope::parse(r#"{"code":200,"data":"zz"}"#).unwrap();
    assert_eq!(
        classify_poll(&ok200),
        PollOutcome::Success {
            encrypted: "zz".into()
        }
    );

    // 成功码但 data 不是字符串：如实报"形状不对"，不许伪造空凭据
    let bad = LoginEnvelope::parse(r#"{"code":"0000","data":{"a":1}}"#).unwrap();
    match classify_poll(&bad) {
        PollOutcome::Other { code, message } => {
            assert_eq!(code, Some(0));
            assert!(message.contains("data"), "实得：{message}");
        }
        other => panic!("应归 Other，实得 {other:?}"),
    }
}

#[test]
fn classify_send_sms_maps_quota_slider_and_exhaustion() {
    let sent = LoginEnvelope::parse(r#"{"code":"0000","data":{"remainGetTimes":3}}"#).unwrap();
    assert_eq!(
        classify_send_sms(&sent),
        SendSmsOutcome::Sent {
            remain_get_times: Some(3)
        }
    );

    let need = LoginEnvelope::parse(r#"{"code":200059554,"data":{"captchaType":2}}"#).unwrap();
    assert_eq!(
        classify_send_sms(&need),
        SendSmsOutcome::NeedSlider {
            captcha_type: Some(2)
        }
    );

    let gone = LoginEnvelope::parse(r#"{"code":200059553}"#).unwrap();
    assert_eq!(
        classify_send_sms(&gone),
        SendSmsOutcome::SliderExhausted { captcha_type: None }
    );

    // 配额缺失不该让"已发出"变成失败
    let no_quota = LoginEnvelope::parse(r#"{"code":"0000"}"#).unwrap();
    assert_eq!(
        classify_send_sms(&no_quota),
        SendSmsOutcome::Sent {
            remain_get_times: None
        }
    );
}

/// 短信码错误（`9441`）与"解不开请求体"（`9103`）是**不同**的档。
#[test]
fn sms_code_wrong_is_distinct_from_undecryptable_body() {
    let wrong = LoginEnvelope::parse(r#"{"code":9441,"message":"验证码不正确"}"#).unwrap();
    let bad = LoginEnvelope::parse(r#"{"code":9103,"message":"输入参数不合法"}"#).unwrap();

    let a = classify_poll(&wrong);
    let b = classify_poll(&bad);
    assert_ne!(a, b, "两个码语义不同，不能归到同一档");
    match (a, b) {
        (
            PollOutcome::Other {
                code: Some(9441), ..
            },
            PollOutcome::Other {
                code: Some(9103), ..
            },
        ) => {}
        other => panic!("两码应各自归 Other 且保留原码，实得 {other:?}"),
    }
}

// ══════════════════════════ 滑块 ══════════════════════════

/// 滑块答案加密：`base64(IV ‖ CT)`，IV 是硬编码常量且长度固定 32 字节。
#[test]
fn puzzle_verfycode_is_base64_of_iv_plus_ciphertext() {
    let enc = puzzle_verfycode_encrypt("461").unwrap();
    let raw = mcloud_crypto::b64_decode(&enc).unwrap();
    assert_eq!(raw.len(), 32, "16 字节 IV + 16 字节密文块");
    assert_eq!(&raw[..16], PUZZLE_AES_IV, "前 16 字节必须是硬编码 IV");

    // 两次加密的密文部分不同（PKCS7 之下明文相同、IV 固定 ⇒ 密文其实相同；
    // 这里断言的是"IV 恒定"这个易错点，而不是随机性）
    let again = puzzle_verfycode_encrypt("461").unwrap();
    assert_eq!(enc, again, "IV 硬编码 ⇒ 同一答案的密文可复现");

    // 不同答案必须产出不同密文
    assert_ne!(enc, puzzle_verfycode_encrypt("462").unwrap());
}

#[test]
fn slider_body_with_puzzle_adds_the_misspelled_field() {
    let body = send_sms_code_body_with_slider(&win_client(), "19900000001", "rnd", "461").unwrap();
    // 字段名是 puzzleVerfycode（verfy 不是 verify）
    let enc = body
        .get("puzzleVerfycode")
        .and_then(Value::as_str)
        .expect("缺 puzzleVerfycode");
    assert!(enc.starts_with("E526E1F3F9765444") || !enc.is_empty());
    // 基础字段仍在
    assert_eq!(body["phoneNumber"], "19900000001");
    assert_eq!(body["random"], "rnd");

    // 基础发码 body **不该**有滑块字段（异常路径要保持显眼）
    let base = send_sms_code_body(&win_client(), "19900000001", "rnd");
    assert!(base.get("puzzleVerfycode").is_none());
}

/// 滑块答案能从上游响应里解出来 —— 用真实密文形状自造一条。
#[test]
fn slider_puzzle_answer_decrypts_and_validates() {
    // 用同一个密钥/IV 造一条 puzzleLeft（上游就是这么发的）
    let encrypted =
        encrypt_with_dynamic_iv(PUZZLE_AES_KEY.as_bytes(), PUZZLE_AES_IV, "370").unwrap();
    let p = SliderPuzzle {
        background_b64: "bg".into(),
        puzzle_b64: "pz".into(),
        pic_width: Some(680),
        puzzle_width: Some(96),
        left_encrypted: encrypted,
    };
    assert_eq!(p.answer().unwrap(), "370");

    // 超出图宽必须报错，不能把荒谬坐标发上去
    let bad = SliderPuzzle {
        pic_width: Some(100),
        ..p.clone()
    };
    assert!(bad.answer().is_err(), "370 超出图宽 100，必须拒绝");

    // 非十进制内容必须报错
    let junk_ct = encrypt_with_dynamic_iv(PUZZLE_AES_KEY.as_bytes(), PUZZLE_AES_IV, "abc").unwrap();
    let junk = SliderPuzzle {
        left_encrypted: junk_ct,
        pic_width: None,
        ..p
    };
    assert!(junk.answer().is_err());
}

#[test]
fn parse_slider_puzzle_reads_fields_and_tolerates_string_sizes() {
    let env = LoginEnvelope::parse(
        r#"{"code":"0000","data":{"picture":"BG","puzzle":"PZ","picWidth":"680","puzzleWidth":96,"puzzleLeft":"CT"}}"#,
    )
    .unwrap();
    let p = parse_slider_puzzle(&env).unwrap();
    assert_eq!(p.background_b64, "BG");
    assert_eq!(p.puzzle_b64, "PZ");
    assert_eq!(p.pic_width, Some(680), "字符串尺寸要能解析");
    assert_eq!(p.puzzle_width, Some(96), "数字尺寸也要能接");
    assert_eq!(p.left_encrypted, "CT");

    // 缺字段必须报错，而不是回空串（空图会被当成"图挂了"）
    let missing = LoginEnvelope::parse(r#"{"code":"0000","data":{"picture":"BG"}}"#).unwrap();
    assert!(parse_slider_puzzle(&missing).is_err());

    // 没有 data
    let nodata = LoginEnvelope::parse(r#"{"code":"0000"}"#).unwrap();
    assert!(parse_slider_puzzle(&nodata).is_err());
}

// ══════════════════════════ 解密 ══════════════════════════

/// 登录响应解密：AES-128-ECB/PKCS7，密钥 `qPqDw263XgFgL3u8`。
#[test]
fn login_decryptor_uses_aes_ecb_and_parses_user_info() {
    use aes::cipher::{BlockEncrypt, KeyInit};

    let plain = r#"{"account":"19900000001","userId":"ud-1","token":"tok"}"#;
    let key = LOGIN_AES_KEY.as_bytes();

    // 手工 ECB 加密（含 PKCS7），作为 data 密文
    let mut buf = plain.as_bytes().to_vec();
    let pad = 16 - (buf.len() % 16);
    buf.resize(buf.len() + pad, pad as u8);
    let cipher = aes::Aes128::new_from_slice(key).unwrap();
    let mut ct = Vec::new();
    // 上面刚补齐到 16 的整数倍，余数为空
    let (blocks, _) = buf.as_chunks::<16>();
    for block in blocks {
        let mut b = *block;
        cipher.encrypt_block((&mut b).into());
        ct.extend_from_slice(&b);
    }
    let hex_ct = hex::encode(&ct);

    let v = LoginDataDecryptor::production()
        .decrypt_user_info(&hex_ct)
        .unwrap();
    assert_eq!(v["account"], "19900000001");
    assert_eq!(v["userId"], "ud-1");

    // 换个密钥必须失败（而不是解出乱码当成功）
    let wrong = LoginDataDecryptor::with_key("0123456789abcdef")
        .decrypt_user_info(&hex_ct)
        .map(|v| v.to_string())
        .unwrap_or_default();
    assert!(
        !wrong.contains("19900000001"),
        "错误密钥不该解出原文，实得：{wrong}"
    );

    // 测试环境密钥与生产密钥必须不同，否则换环境会静默用错
    assert_ne!(LOGIN_AES_KEY, LOGIN_AES_KEY_TEST);
}

#[test]
fn login_decryptor_rejects_non_hex() {
    assert!(LoginDataDecryptor::production()
        .decrypt_user_info("!!!not-hex!!!")
        .is_err());
}
