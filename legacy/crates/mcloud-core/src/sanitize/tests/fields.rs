// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

#[test]
fn sensitive_field_matching_is_case_and_separator_agnostic() {
    for key in [
        "auth",
        "AUTH",
        "Auth",
        "userDomainId",
        "user_domain_id",
        "accessToken",
        "access-token",
        "AccessToken",
    ] {
        assert!(is_sensitive_field(key), "{key} 应被判定为敏感字段");
    }
    for key in ["prizeId", "count", "date", "code", "nickname"] {
        assert!(!is_sensitive_field(key), "{key} 不应被判定为敏感字段");
    }
}

#[test]
fn headers_get_redacted_but_harmless_ones_survive() {
    let headers = vec![
        h("auth", "eyJhbGciOiJIUzI1NiJ9.SECRET"),
        h("Cookie", "sid=abc123"),
        h("user-agent", "mcloud/12.6.0"),
        h("content-type", "application/json"),
    ];
    let out = sanitize_headers(&headers);
    assert_eq!(out[0].1, PLACEHOLDER);
    assert_eq!(out[1].1, PLACEHOLDER, "Cookie 大小写不敏感");
    assert_eq!(out[2].1, "mcloud/12.6.0", "普通 header 应保留");
    assert_eq!(out[3].1, "application/json");
}

#[test]
fn nested_json_is_redacted_including_arrays() {
    let mut value = json!({
        "data": {
            "auth": "SECRET_AUTH",
            "profile": { "phone": "19900000001", "nickname": "账号2" },
            "accounts": [
                { "id": "a1", "accessToken": "SECRET_TOKEN" },
                { "id": "a2", "jwt": "SECRET_JWT" }
            ]
        },
        "code": 0
    });
    assert!(sanitize_json(&mut value));
    assert_eq!(value["data"]["auth"], PLACEHOLDER);
    assert_eq!(value["data"]["profile"]["phone"], PLACEHOLDER);
    assert_eq!(value["data"]["profile"]["nickname"], "账号2");
    assert_eq!(value["data"]["accounts"][0]["accessToken"], PLACEHOLDER);
    assert_eq!(value["data"]["accounts"][1]["jwt"], PLACEHOLDER);
    assert_eq!(value["code"], 0, "业务码不应被改");
    assert!(scan_for_leaks(&value.to_string()).is_empty());
}

#[test]
fn non_json_body_gets_token_runs_redacted() {
    let body = "session=abcdefghijklmnopqrstuvwxyz0123456789ABCDEF&x=1";
    let out = String::from_utf8(sanitize_body(body.as_bytes())).unwrap();
    assert!(out.contains(PLACEHOLDER), "长 token 串应被遮蔽: {out}");
    assert!(out.contains("&x=1"), "短参数应保留: {out}");
}

#[test]
fn binary_body_is_not_stored_verbatim() {
    let bytes = vec![0x00u8, 0xff, 0xfe, 0x01, 0x02];
    let out = String::from_utf8(sanitize_body(&bytes)).unwrap();
    assert_eq!(out, "<BINARY 5 bytes>");
}
