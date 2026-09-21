// 由 crypto.rs 的 `mod tests` 按原语拆出；黄金向量夹具见同目录 mod.rs。
use super::*;

// ── 哈希 ────────────────────────────────────────────────────────────────

#[test]
fn md5_matches_ts_vectors() {
    let v = vectors();
    assert!(!v.md5.is_empty(), "向量不能为空");
    for c in &v.md5 {
        assert_eq!(md5_hex(c.input.as_bytes()), c.expect, "md5 用例 {}", c.name);
    }
}

#[test]
fn sha256_matches_ts_vectors() {
    let v = vectors();
    for c in &v.sha256 {
        assert_eq!(
            sha256_hex(c.input.as_bytes()),
            c.expect,
            "sha256 用例 {}",
            c.name
        );
    }
}

#[test]
fn hmac_matches_ts_vectors() {
    let v = vectors();
    for c in &v.hmac_sha256_b64 {
        assert_eq!(
            hmac_sha256_base64(c.key.as_bytes(), c.message.as_bytes()),
            c.expect,
            "hmac 用例 {}",
            c.name
        );
    }
}

/// 反向确认：base64 与 hex 不能搞混（原实现这里是 base64，很容易写错）。
#[test]
fn hmac_is_base64_not_hex() {
    let v = vectors();
    let c = &v.hmac_sha256_b64[0];
    let got = hmac_sha256_base64(c.key.as_bytes(), c.message.as_bytes());
    assert!(
        got.ends_with('=') || got.len() == 44,
        "base64 应有 padding/长度特征: {got}"
    );
    assert!(!got.chars().all(|ch| ch.is_ascii_hexdigit()), "不应是 hex");
}
