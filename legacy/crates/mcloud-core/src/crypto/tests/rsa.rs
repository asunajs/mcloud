// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── RSA ─────────────────────────────────────────────────────────────────

/// **核心验证**：RSA PKCS#1 v1.5 加密不可字节级对拍，所以这样验：
/// Node 用公钥加密 → Rust 用私钥解回 → 必须等于原明文。
#[test]
fn rsa_decrypts_node_produced_ciphertext() {
    let v = vectors();
    let priv_der = b64_decode(&v.rsa_self.private_key_pkcs8_b64).unwrap();
    assert!(!v.rsa_self.cases.is_empty());
    for c in &v.rsa_self.cases {
        let pt = rsa_pkcs1v15_decrypt(&priv_der, &c.cipher_b64).unwrap();
        assert_eq!(
            String::from_utf8(pt).unwrap(),
            c.message,
            "RSA 解密用例 {}",
            c.name
        );
    }
}

/// 反方向：Rust 加密 → Rust 自解。这条证明我们的**加密**产出是合法 PKCS#1 v1.5。
#[test]
fn rsa_encrypt_then_self_decrypt() {
    let v = vectors();
    let priv_der = b64_decode(&v.rsa_self.private_key_pkcs8_b64).unwrap();
    let mut rng = SeededRng::new(42);
    for c in &v.rsa_self.cases {
        let ct = rsa_pkcs1v15_encrypt_base64_with_b64_key(
            &v.rsa_self.public_key_b64,
            c.message.as_bytes(),
            &mut rng,
        )
        .unwrap();
        let pt = rsa_pkcs1v15_decrypt(&priv_der, &ct).unwrap();
        assert_eq!(String::from_utf8(pt).unwrap(), c.message, "用例 {}", c.name);
    }
}

/// 同一明文两次加密必须**不同** —— 把"RSA 不可对拍"这件事本身钉成测试，
/// 避免以后有人误加字节级断言。
#[test]
fn rsa_encryption_is_intentionally_non_deterministic() {
    let v = vectors();
    let mut rng = SeededRng::new(7);
    let a = rsa_pkcs1v15_encrypt_base64_with_b64_key(&v.rsa_self.public_key_b64, b"v", &mut rng)
        .unwrap();
    let b = rsa_pkcs1v15_encrypt_base64_with_b64_key(&v.rsa_self.public_key_b64, b"v", &mut rng)
        .unwrap();
    assert_ne!(a, b, "PKCS#1 v1.5 加密填充含随机字节，两次结果不应相同");
}

/// skland 真实公钥：只断言结构（1024 位 → 128 字节密文）。
#[test]
fn rsa_skland_ciphertext_has_expected_size() {
    let v = vectors();
    let c = &v.rsa_skland[0];
    assert_eq!(
        c.public_key_b64, SKLAND_PUBLIC_KEY_B64,
        "公钥常量应与源码一致"
    );
    let mut rng = SeededRng::new(9);
    for sample in &c.samples {
        assert_eq!(
            b64_decode(sample).unwrap().len(),
            c.modulus_bytes,
            "已知样本长度"
        );
    }
    let ct =
        rsa_pkcs1v15_encrypt_base64_with_b64_key(SKLAND_PUBLIC_KEY_B64, b"v", &mut rng).unwrap();
    assert_eq!(b64_decode(&ct).unwrap().len(), c.modulus_bytes);
}
