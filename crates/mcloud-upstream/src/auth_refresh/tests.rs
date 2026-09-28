//! 刷新凭据测试。
//!
//! 重点在**单位**与**拼串**：这两处错了都不报错，只会静默算错
//! （"永远不需要刷新" / 库里落一个用不了的串）。

use super::*;
use mcloud_core::{AccountId, AccountSettings, AuthInfo, DeviceId};

/// 一份贴近实拍的账号。
fn account(platform: &str, domain: Option<&str>) -> Account {
    Account {
        id: AccountId::new("199****01"),
        nickname: None,
        auth: AuthInfo {
            raw: "bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAx".into(),
            platform: platform.into(),
            phone: "19900000000".into(),
            token: "SYNTH001|1|RCS|1792391642025|payload".into(),
            expire: 1_792_391_642_025,
        },
        device_id: DeviceId("DEV".into()),
        pc_platform: None,
        user_domain_id: domain.map(str::to_string),
        settings: AccountSettings::default(),
    }
}

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ── 请求形状 ────────────────────────────────────────────────────────────

#[test]
fn request_uses_the_captured_endpoint_and_shape() {
    let req = refresh_request(&account("mobile", Some("1990000000000000001"))).unwrap();
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url, "https://user-njs.yun.139.com/user/auth/refreshToken",
        "端点必须来自实拍，不是 authTokenRefresh.do（那条零抓包）"
    );
    assert_eq!(
        header(&req, "Content-Type"),
        Some("application/json;charset=UTF-8")
    );
    assert!(header(&req, "Authorization").is_some(), "要带当前凭据");
    let body = req.body_str().unwrap();
    assert!(
        body.contains(r#""userDomainId":"1990000000000000001""#),
        "{body}"
    );
    assert!(body.contains(r#""clientType":"412""#), "App 是 412：{body}");
}

/// ⭐ `clientType` 按**凭据里的端别**选，不按参数。
#[test]
fn client_type_follows_the_credential_not_a_parameter() {
    assert_eq!(
        client_type_for(&account("mobile", None)),
        "412",
        "App 是 412"
    );
    assert_eq!(client_type_for(&account("pc", None)), "414", "PC 是 414");
}

/// ⭐ 请求体的**键顺序**与实拍一致（`userDomainId` 在前）。
#[test]
fn body_key_order_matches_the_capture() {
    let req = refresh_request(&account("mobile", Some("1990000000000000001"))).unwrap();
    let body = req.body_str().unwrap();
    let a = body.find("userDomainId").unwrap();
    let b = body.find("clientType").unwrap();
    assert!(a < b, "userDomainId 应在前：{body}");
}

/// ⭐ 缺用户域时**明确报错**，不编一个。
///
/// 编一个会把刷新打到别人的用户域上。
#[test]
fn missing_domain_is_an_explicit_error() {
    let e = refresh_request(&account("mobile", None))
        .unwrap_err()
        .to_string();
    assert!(e.contains("用户域") || e.contains("userDomainId"), "{e}");
    assert!(e.contains("重新登录"), "要告诉用户怎么办：{e}");
}

// ── 响应解析 ────────────────────────────────────────────────────────────

#[test]
fn parses_the_real_envelope_shape() {
    // 信封：success + code（**字符串** "0000"）+ message + data
    let body = r#"{"success":true,"code":"0000","message":"请求成功",
      "data":{"token":"SYNTH001|1|RCS|1792245335331|AAAA","expireTime":2592000}}"#;
    let r = parse_refresh(body).unwrap();
    assert_eq!(r.token, "SYNTH001|1|RCS|1792245335331|AAAA");
    assert_eq!(r.expire_seconds, Some(2592000), "expireTime 是秒");
}

#[test]
fn expire_seconds_may_be_absent() {
    let body = r#"{"success":true,"code":"0000","data":{"token":"T"}}"#;
    let r = parse_refresh(body).unwrap();
    assert_eq!(r.token, "T");
    assert_eq!(r.expire_seconds, None, "缺了不算失败 —— token 自带过期段");
}

#[test]
fn rejects_failure_envelopes() {
    for bad in [
        r#"{"success":false,"code":"1001","message":"参数错误"}"#,
        r#"{"success":true,"code":"9103","message":"解不开请求体"}"#,
        r#"{"code":"1001","message":"失败"}"#,
    ] {
        assert!(parse_refresh(bad).is_err(), "应报错：{bad}");
    }
}

/// 缺 `data.token` 要报错 —— 否则会拿一个空串去拼 Authorization。
#[test]
fn rejects_missing_token() {
    assert!(parse_refresh(r#"{"success":true,"code":"0000","data":{}}"#).is_err());
    assert!(parse_refresh(r#"{"success":true,"code":"0000","data":{"token":""}}"#).is_err());
}

#[test]
fn non_json_is_an_error_not_a_panic() {
    assert!(parse_refresh("<html>502</html>").is_err());
}

// ── 拼回整串 ────────────────────────────────────────────────────────────

/// ⭐ 新 token 是 `Authorization` 的**第三段**，不是整串。
///
/// 直接把整串替换成新 token 的写法，落库后是一个**用不了的串**。
#[test]
fn rebuilds_the_full_authorization_not_just_the_token() {
    let acc = account("mobile", Some("1990000000000000001"));
    let rebuilt = rebuild_authorization(&acc, "NEWTOKEN").unwrap();
    let decoded = mcloud_crypto::b64_decode(&rebuilt).unwrap();
    let plain = String::from_utf8(decoded).unwrap();
    assert_eq!(
        plain, "mobile:19900000000:NEWTOKEN",
        "形状是 `端别:手机号:token`"
    );
}

/// ⭐ 返回的是**裸 base64**，不带 `Basic ` 前缀。
///
/// 带了的话 `basic_authorization()` 会再补一次，变成 `Basic Basic …`。
#[test]
fn rebuilt_value_is_bare_base64_without_the_basic_prefix() {
    let acc = account("mobile", None);
    let rebuilt = rebuild_authorization(&acc, "T").unwrap();
    assert!(!rebuilt.starts_with("Basic "), "不该带前缀：{rebuilt}");
    assert!(!rebuilt.contains(' '), "裸 base64 不含空格：{rebuilt}");
    // 补前缀之后才是请求头要的形态（`basic_authorization` 的契约）
    let mut with_new = acc.clone();
    with_new.auth.raw = rebuilt;
    assert!(crate::basic_authorization(&with_new).starts_with("Basic "));
}

/// 端别从凭据里取，不重新猜。
#[test]
fn rebuild_keeps_the_platform_from_the_credential() {
    for p in ["mobile", "pc"] {
        let acc = account(p, None);
        let rebuilt = rebuild_authorization(&acc, "T").unwrap();
        let plain = String::from_utf8(mcloud_crypto::b64_decode(&rebuilt).unwrap()).unwrap();
        assert!(plain.starts_with(&format!("{p}:")), "{plain}");
    }
}

#[test]
fn rebuild_rejects_empty_token_and_missing_phone() {
    let acc = account("mobile", None);
    assert!(rebuild_authorization(&acc, "").is_err());
    let mut no_phone = account("mobile", None);
    no_phone.auth.phone = String::new();
    assert!(rebuild_authorization(&no_phone, "T").is_err());
}

// ── 一步到位 ────────────────────────────────────────────────────────────

#[test]
fn apply_refresh_returns_both_the_result_and_the_new_auth() {
    let acc = account("mobile", Some("1990000000000000001"));
    let resp = HttpResponse {
        status: 200,
        body: r#"{"success":true,"code":"0000","data":{"token":"NEW","expireTime":2592000}}"#
            .into(),
        headers: vec![],
    };
    let (r, auth) = apply_refresh(&acc, &resp).unwrap();
    assert_eq!(r.token, "NEW");
    let plain = String::from_utf8(mcloud_crypto::b64_decode(&auth).unwrap()).unwrap();
    assert!(plain.ends_with(":NEW"), "{plain}");
}

#[test]
fn apply_refresh_maps_http_errors() {
    let acc = account("mobile", None);
    let resp = HttpResponse {
        status: 500,
        body: "boom".into(),
        headers: vec![],
    };
    let e = apply_refresh(&acc, &resp).unwrap_err().to_string();
    assert!(e.contains("500"), "{e}");
}

// ── 常量 ────────────────────────────────────────────────────────────────

/// ⭐ `412` 是 App 的码。旧 TS 项目的 `414` **不是** —— 这是第五处跨端分歧。
#[test]
fn app_client_type_is_412_not_the_legacy_414() {
    assert_eq!(APP_CLIENT_TYPE, "412");
    assert_ne!(
        APP_CLIENT_TYPE, "414",
        "414 是旧 TS 项目的默认值，不是 App 的"
    );
}
