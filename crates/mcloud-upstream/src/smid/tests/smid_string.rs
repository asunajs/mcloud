//! `smid` 串、`priId`、`ep` 的形状。

use super::*;
use mcloud_crypto::b64_decode;

#[test]
fn smid_string_is_time_plus_two_md5_segments() {
    let uid = "0f5a1c00-0000-4000-8000-000000000000";
    let smid = smid_string("20260919153000", uid);
    let inner = format!("{}{}00", "20260919153000", mcloud_crypto::md5_hex(uid));

    assert_eq!(smid.len(), 63, "14 + 32 + 2 + 14 + 1");
    assert!(smid.starts_with("20260919153000"), "{smid}");
    assert!(smid.ends_with('0'));
    // 第三段是 `smsk_web_` 加上前 48 位再取 md5 的前 14 位
    let expect_tail = format!(
        "{}0",
        &mcloud_crypto::md5_hex(&format!("smsk_web_{inner}"))[..14]
    );
    assert_eq!(smid[48..], expect_tail);
}

#[test]
fn private_id_is_the_first_16_hex_of_uid_md5() {
    assert_eq!(private_id("u").len(), 16);
    assert_eq!(private_id("u"), &mcloud_crypto::md5_hex("u")[..16]);
}

#[test]
fn ep_is_128_bytes_of_rsa_output() {
    let ep = ep("some-uid", &mut seq_rng(7)).unwrap();
    let raw = b64_decode(&ep).unwrap();
    // 1024 位公钥 → PKCS#1 v1.5 密文固定 128 字节
    assert_eq!(raw.len(), 128);
}

#[test]
fn the_two_uids_are_drawn_separately() {
    // 旧实现里 `getSmId()` 内部**自己又** randomUUID 了一次，
    // 与 `getSmData` 外层那个 uid 不是同一个。这条钉住"没有顺手统一"。
    let mut rng = seq_rng(1);
    let target = fingerprint_target(&fp(), &upload(), &mut rng);
    // 特征体里的 smid 用的是第 1 个 uuid；换一个随机源起点就该不同
    let other = fingerprint_target(&fp(), &upload(), &mut seq_rng(200));
    assert_ne!(target["smid"], other["smid"]);
}
