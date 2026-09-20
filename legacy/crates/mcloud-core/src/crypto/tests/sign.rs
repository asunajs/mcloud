// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use serde_json::Value;

// ── 签名头 ──────────────────────────────────────────────────────────────

#[test]
fn sign_header_matches_ts_vectors() {
    let v = vectors();
    for c in &v.sign_header {
        let ts = match &c.timestamp {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            other => panic!("意外的 timestamp 类型: {other}"),
        };
        assert_eq!(
            generate_signature(&c.id, &ts, &c.nonce, c.salt_key, &c.other),
            c.expect_md5,
            "签名用例 {}",
            c.name
        );
    }
}

#[test]
fn unknown_salt_falls_back_to_first() {
    assert_eq!(sign_salt(99), SIGN_SALT_MAP[0].1);
    assert_eq!(sign_salt(1), SIGN_SALT_MAP[0].1);
    assert_eq!(sign_salt(2), SIGN_SALT_MAP[1].1);
}

// ── getTn ───────────────────────────────────────────────────────────────

#[test]
fn get_tn_matches_ts_vectors() {
    let v = vectors();
    for c in &v.get_tn {
        assert_eq!(get_tn(&c.input), c.expect, "getTn 用例 {}", c.name);
    }
}

/// `null` 必须变成字符串 `"null"`，不能变空串 —— 这是 getTn 的隐蔽分支。
#[test]
fn get_tn_null_becomes_literal_null() {
    let obj = serde_json::json!({ "a": null });
    assert_eq!(get_tn(&obj), "null");
}

#[test]
fn get_tn_scales_numbers_by_10000() {
    assert_eq!(get_tn(&serde_json::json!({ "t": 7 })), "70000");
    assert_eq!(get_tn(&serde_json::json!({ "tz": -480 })), "-4800000");
    // 非整数走 floor
    assert_eq!(get_tn(&serde_json::json!({ "a": 1.00009 })), "10000");
}
