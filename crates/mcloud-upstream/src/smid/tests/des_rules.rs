// 由同目录 mod.rs 提供确定性随机源等 helper；本文件是按主题拆出的用例。
use super::*;

// ==================== DES 规则表 ====================

#[test]
fn rule_table_matches_the_legacy_field_count() {
    assert_eq!(
        RULES.len(),
        25,
        "旧 DES_RULE 是 25 条，多一条少一条都是事故"
    );
    // 三条"改名但不加密"
    for (field, name) in [
        ("box", "aw"),
        ("protocol", "protocol"),
        ("version", "version"),
    ] {
        let rule = rule_for(field).unwrap();
        assert_eq!(rule.cipher_key, None, "{field} 在旧代码里 is_encrypt: 0");
        assert_eq!(rule.wire_name, name);
    }
    // 抽查几条加密的改名（写错一个字母，服务端就看不到那个字段）
    assert_eq!(rule_for("appId").unwrap().wire_name, "um");
    assert_eq!(rule_for("ua").unwrap().wire_name, "pu");
    assert_eq!(rule_for("tn").unwrap().wire_name, "as");
    assert_eq!(rule_for("organization").unwrap().wire_name, "nl");
}

#[test]
fn fields_without_a_rule_keep_their_own_name() {
    for field in [
        "smid",
        "cdp",
        "battery",
        "maxTouchPoints",
        "connectionRtt",
        "cpucount",
    ] {
        assert!(rule_for(field).is_none(), "{field} 不该有规则");
    }
}

#[test]
fn numbers_are_stringified_the_js_way_before_encryption() {
    // `String(-480)` 是 "-480"：按 Rust 的 Debug 或带小数点的写法加密，服务端解出来就是另一个值
    let target = json!({ "timezone": -480, "time": 7, "svm": 1_789_743_328_974i64 });
    let out = encrypt_by_des_rules(&target).unwrap();
    assert_eq!(
        out["bl"],
        Value::String(des_ecb_no_padding_encrypt_base64("l94h8clc", "-480").unwrap())
    );
    assert_eq!(
        out["jt"],
        Value::String(des_ecb_no_padding_encrypt_base64("btb17oag", "7").unwrap())
    );
    assert_eq!(
        out["ee"],
        Value::String(des_ecb_no_padding_encrypt_base64("yf780e9w", "1789743328974").unwrap())
    );
}

#[test]
fn empty_strings_stay_empty_after_encryption() {
    // 旧 encryptDes("") → 空 base64（补零逻辑 rem==0 时不补，分块也就没有）
    let out = encrypt_by_des_rules(&json!({ "referer": "" })).unwrap();
    assert_eq!(out["mf"], Value::String(String::new()));
}

#[test]
fn encrypted_fields_do_not_leak_plaintext() {
    let out = encrypt_by_des_rules(&json!({ "os": "web" })).unwrap();
    let wire = out["lz"].as_str().unwrap();
    assert_ne!(wire, "web");
    assert!(!wire.contains("web"));
}
