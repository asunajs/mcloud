// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

use mcloud_core::b64_decode;

// ==================== smid 串 ====================

#[test]
fn smid_string_is_time_plus_two_md5_segments() {
    let uid = "0f5a1c00-0000-4000-8000-000000000000";
    let smid = smid_string("20260919153000", uid);
    let inner = format!("{}{}00", "20260919153000", md5_hex(uid.as_bytes()));

    assert_eq!(smid.len(), 63, "14 + 32 + 2 + 14 + 1");
    assert!(smid.starts_with("20260919153000"), "{smid}");
    assert!(smid.ends_with('0'));
    // 第三段是 `smsk_web_` 加上前 48 位再取 md5 的前 14 位
    let expect_tail = format!(
        "{}0",
        &md5_hex(format!("smsk_web_{inner}").as_bytes())[..14]
    );
    assert_eq!(smid[48..], expect_tail);
}

#[test]
fn private_id_is_the_first_16_hex_of_uid_md5() {
    assert_eq!(private_id("u").len(), 16);
    assert_eq!(private_id("u"), &md5_hex(b"u")[..16]);
}

#[test]
fn ep_is_128_bytes_of_rsa_output() {
    let ep = ep("some-uid", &mut SeqRng(7)).unwrap();
    let raw = b64_decode(&ep).unwrap();
    // 1024 位公钥 → PKCS#1 v1.5 密文固定 128 字节
    assert_eq!(raw.len(), 128);
}
