//! 黄金向量对拍。
//!
//! 向量来自权威来源（TS 原实现生成 + 真实抓包交叉验证），
//! 本测试确保 Rust 实现与上游**字节级一致**。任何不一致都是 bug，
//! 不允许「调整期望值」来让测试变绿。

use std::path::PathBuf;

use mcloud_crypto::{aes, des, generate_signature, hash, SALT_MAP};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Vectors {
    constants: Constants,
    md5: Vec<Md5Case>,
    sha256: Vec<Sha256Case>,
    #[serde(rename = "aes128CbcFixedIvHex")]
    aes128_fixed_iv: Vec<AesFixedCase>,
    #[serde(rename = "signHeader")]
    sign_header: Vec<SignCase>,
    #[serde(rename = "aesDynamicIv")]
    aes_dynamic_iv: Vec<AesDynamicCase>,
    /// 设备指纹上报：DES-ECB/NoPadding，base64 输出。
    #[serde(rename = "desEcbNoPaddingB64")]
    des_no_padding: Vec<DesEncryptCase>,
    /// 旧接口响应：DES-ECB/PKCS7，base64 进、utf8 出。
    #[serde(rename = "desEcbPkcs7Decrypt")]
    des_pkcs7_decrypt: Vec<DesDecryptCase>,
}

#[derive(Deserialize)]
struct DesEncryptCase {
    name: String,
    key: String,
    message: String,
    expect: String,
}

#[derive(Deserialize)]
struct DesDecryptCase {
    name: String,
    key: String,
    #[serde(rename = "cipherB64")]
    cipher_b64: String,
    expect: String,
}

#[derive(Deserialize)]
struct AesDynamicCase {
    name: String,
    key: String,
    #[serde(rename = "ivHex")]
    iv_hex: String,
    plaintext: String,
    #[serde(rename = "expectB64")]
    expect_b64: String,
    #[serde(rename = "byteExact", default)]
    byte_exact: bool,
}

#[derive(Deserialize)]
struct Constants {
    #[serde(rename = "SIGN_SALT_MAP")]
    sign_salt_map: std::collections::HashMap<String, String>,
    #[serde(rename = "REFRESH_TOKEN_KEY")]
    refresh_token_key: String,
    #[serde(rename = "AES_FIXED_IV")]
    aes_fixed_iv: String,
}

#[derive(Deserialize)]
struct Md5Case {
    name: String,
    input: String,
    expect: String,
}

#[derive(Deserialize)]
struct Sha256Case {
    name: String,
    input: String,
    expect: String,
}

#[derive(Deserialize)]
struct AesFixedCase {
    name: String,
    /// 向量里叫 message
    message: String,
    /// 向量里叫 expect（hex）
    expect: String,
    key: String,
}

#[derive(Deserialize)]
struct SignCase {
    name: String,
    id: String,
    timestamp: Value,
    nonce: String,
    #[serde(rename = "saltKey")]
    salt_key: u8,
    #[serde(default)]
    other: String,
    #[serde(rename = "expectMd5")]
    expect_md5: String,
}

fn load() -> Vectors {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("test/golden/crypto-vectors.json");
    let raw = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("读取黄金向量失败 {}：{e}", p.display()));
    serde_json::from_str(&raw).expect("黄金向量格式不符")
}

fn ts_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => panic!("意外的 timestamp 类型：{other}"),
    }
}

#[test]
fn constants_match_golden() {
    let v = load();
    assert_eq!(
        SALT_MAP[0].1, v.constants.sign_salt_map["1"],
        "盐值 1 必须与 TS 一致"
    );
    assert_eq!(
        SALT_MAP[1].1, v.constants.sign_salt_map["2"],
        "盐值 2 必须与 TS 一致"
    );
    assert_eq!(
        aes::REFRESH_TOKEN_KEY,
        v.constants.refresh_token_key,
        "刷新令牌密钥必须与 TS 一致"
    );
    assert_eq!(
        std::str::from_utf8(aes::AES_FIXED_IV).unwrap(),
        v.constants.aes_fixed_iv,
        "固定 IV 必须与 TS 一致"
    );
}

#[test]
fn md5_matches_all_vectors() {
    let v = load();
    assert!(!v.md5.is_empty(), "向量不应为空");
    for c in &v.md5 {
        assert_eq!(hash::md5_hex(&c.input), c.expect, "MD5 用例 {}", c.name);
    }
}

#[test]
fn sha256_matches_all_vectors() {
    let v = load();
    assert!(!v.sha256.is_empty(), "向量不应为空");
    for c in &v.sha256 {
        assert_eq!(
            hash::sha256_hex(&c.input),
            c.expect,
            "SHA256 用例 {}",
            c.name
        );
    }
}

#[test]
fn aes_fixed_iv_matches_all_vectors() {
    let v = load();
    for c in &v.aes128_fixed_iv {
        // 固定 IV 路径对应 rt.crypto.aesEncrypt：密钥截断/补零到 16 字节
        let got = aes::aes_encrypt_compat(&c.key, &c.message)
            .unwrap_or_else(|e| panic!("加密失败（用例 {}）：{e}", c.name));
        assert_eq!(got, c.expect, "AES 固定 IV 用例 {}", c.name);
    }
}

#[test]
fn sign_header_matches_all_vectors() {
    let v = load();
    assert!(!v.sign_header.is_empty(), "签名向量不应为空");
    for c in &v.sign_header {
        let ts = ts_string(&c.timestamp);
        let got = generate_signature(&c.id, &ts, &c.nonce, c.salt_key, &c.other);
        assert_eq!(got, c.expect_md5, "签名用例 {}", c.name);
    }
}

/// 未知 saltKey 必须回落到 1，这是原实现里 `|| SALT_MAP[1]` 的行为。
#[test]
fn unknown_salt_key_matches_golden_fallback() {
    let v = load();
    for c in v.sign_header.iter().filter(|c| c.salt_key > 2) {
        let ts = ts_string(&c.timestamp);
        let got = generate_signature(&c.id, &ts, &c.nonce, c.salt_key, &c.other);
        assert_eq!(got, c.expect_md5, "回落用例 {}", c.name);
        // 同时确认它确实等价于 saltKey=1
        let as_one = generate_signature(&c.id, &ts, &c.nonce, 1, &c.other);
        assert_eq!(got, as_one, "未知 saltKey 应等价于 1");
    }
}

/// 动态 IV 路径：`byteExact` 的向量必须字节级相等。
///
/// 这是证明「密钥不被截断」（真 AES-192）的关键证据。
#[test]
fn dynamic_iv_matches_byte_exact_vectors() {
    let v = load();
    let mut checked = 0;
    for c in &v.aes_dynamic_iv {
        if !c.byte_exact {
            continue;
        }
        let key = c.key.as_bytes();
        let raw_iv = hex::decode(&c.iv_hex).expect("ivHex 应为合法 hex");
        let iv: [u8; 16] = raw_iv.as_slice().try_into().expect("IV 应为 16 字节");
        let got = aes::encrypt_with_dynamic_iv(key, &iv, &c.plaintext)
            .unwrap_or_else(|e| panic!("加密失败（用例 {}）：{e}", c.name));
        assert_eq!(got, c.expect_b64, "动态 IV 用例 {}", c.name);
        checked += 1;
    }
    assert!(checked > 0, "应至少有一条 byteExact 动态 IV 向量");
}

/// 动态 IV 路径：随机 IV 的向量靠解密断言（用同一 key 解出原文）。
#[test]
fn dynamic_iv_decrypts_directional_vectors() {
    let v = load();
    for c in &v.aes_dynamic_iv {
        let key = c.key.as_bytes();
        let got = aes::decrypt_with_dynamic_iv(key, &c.expect_b64)
            .unwrap_or_else(|e| panic!("解密失败（用例 {}）：{e}", c.name));
        assert_eq!(got, c.plaintext, "动态 IV 解密用例 {}", c.name);
    }
}

/// 往返自检。
#[test]
fn dynamic_iv_roundtrip_with_golden_key() {
    let v = load();
    let key = v.constants.refresh_token_key.as_bytes();
    let iv = [0x5Au8; 16];
    let enc = aes::encrypt_with_dynamic_iv(key, &iv, "向量往返测试").unwrap();
    assert_eq!(
        aes::decrypt_with_dynamic_iv(key, &enc).unwrap(),
        "向量往返测试"
    );
}

/// DES-ECB/NoPadding 加密（设备指纹上报路径）。
///
/// 18 条向量覆盖三种密钥写法（正好 8 字节 / 短了补零 / 长了截断）
/// × 六种消息长度（1、7、8、9、16、0），边界是刻意排的：
/// 8 与 16 是「已对齐」，必须**不补**整块。
#[test]
fn des_ecb_no_padding_matches_all_vectors() {
    let v = load();
    assert!(!v.des_no_padding.is_empty(), "向量不应为空");
    for c in &v.des_no_padding {
        let got = des::des_encrypt_b64(&c.message, &c.key)
            .unwrap_or_else(|e| panic!("加密失败（用例 {}）：{e}", c.name));
        assert_eq!(got, c.expect, "DES 无填充用例 {}", c.name);
    }
}

/// DES-ECB/PKCS7 解密（旧接口响应路径）。
#[test]
fn des_ecb_pkcs7_decrypt_matches_all_vectors() {
    let v = load();
    assert!(!v.des_pkcs7_decrypt.is_empty(), "向量不应为空");
    for c in &v.des_pkcs7_decrypt {
        let got = des::des_decrypt_b64(&c.cipher_b64, &c.key)
            .unwrap_or_else(|e| panic!("解密失败（用例 {}）：{e}", c.name));
        assert_eq!(got, c.expect, "DES PKCS7 解密用例 {}", c.name);
    }
}
