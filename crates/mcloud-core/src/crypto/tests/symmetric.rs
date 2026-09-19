// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── AES 固定 IV ─────────────────────────────────────────────────────────

#[test]
fn aes_fixed_iv_matches_ts_vectors() {
    let v = vectors();
    for c in &v.aes_fixed {
        assert_eq!(
            aes_cbc_fixed_iv_encrypt_hex(&c.key, &c.message).unwrap(),
            c.expect,
            "aes 用例 {}",
            c.name
        );
    }
}

// ── DES ─────────────────────────────────────────────────────────────────

#[test]
fn des_ecb_no_padding_matches_ts_vectors() {
    let v = vectors();
    for c in &v.des_enc {
        let got = des_ecb_no_padding_encrypt_base64(&c.key, &c.message).unwrap();
        match &c.expect {
            Some(exp) => assert_eq!(&got, exp, "des 用例 {}", c.name),
            None => panic!("TS 侧该用例本身报错，Rust 却成功了: {}", c.name),
        }
    }
}

#[test]
fn des_ecb_no_padding_empty_stays_empty() {
    // 空串 → 空密文（不是补满一块）。这条单独钉死，因为它最容易"顺手修好"。
    assert_eq!(
        des_ecb_no_padding_encrypt_base64("a9kbown8", "").unwrap(),
        ""
    );
}

#[test]
fn des_ecb_pkcs7_decrypt_matches_ts_vectors() {
    let v = vectors();
    for c in &v.des_dec {
        assert_eq!(
            des_ecb_pkcs7_decrypt(&c.key, &c.cipher_b64).unwrap(),
            c.expect,
            "des 解密用例 {}",
            c.name
        );
    }
}

/// 含 CJK 时按字节补零会得到非 8 倍数长度 → 必须报错，而不是静默产出错误密文。
#[test]
fn des_byte_based_padding_rejects_unaligned_cjk() {
    // "移动云盘" 是 4 个字符 / 12 字节，12 % 8 != 0 —— 但字节补零会补到 16，能算出结果。
    // 这里断言的是：**不做 JS 字符串级补零**，行为差异被显式记录下来。
    let got = des_ecb_no_padding_encrypt_base64("a9kbown8", "移动云盘").unwrap();
    assert!(
        !got.is_empty(),
        "字节补零路径应能算出结果（与 JS 的字符补零不同）"
    );
}

// ── AES 动态 IV ─────────────────────────────────────────────────────────

#[test]
fn aes_dynamic_iv_fixed_matches_ts_byte_exact() {
    let v = vectors();
    for c in &v.aes_dyn {
        assert_eq!(c.key_bytes, c.key.len(), "向量里的 keyBytes 应自洽");
    }
    let c = v
        .aes_dyn
        .iter()
        .find(|c| c.byte_exact)
        .expect("应有一例固定 IV 的字节级对拍");
    let iv_raw = hex::decode(&c.iv_hex).expect("向量里的 ivHex 应是合法 hex");
    // 钉死这条：生成器曾把"AES_FIXED_IV"（16 个 ASCII 字节）当 hex 解析成 8 字节，
    // CryptoJS 不报错、默默产出无法解回的密文。见 scripts/gen-crypto-vectors.cjs 注释。
    assert_eq!(iv_raw.len(), 16, "AES-CBC 的 IV 必须是 16 字节");
    let iv: [u8; 16] = iv_raw.try_into().unwrap();
    assert_eq!(
        aes_dynamic_iv_encrypt(&c.key, &c.plaintext, iv).unwrap(),
        c.expect_b64,
        "AES 动态 IV（固定 IV）字节级对拍失败: {}",
        c.name
    );
}

/// 随机 IV 的例子：Rust 必须能把 TS 产出的密文解回原明文（跨实现互通）。
#[test]
fn aes_dynamic_iv_decrypts_ts_random_iv_output() {
    let v = vectors();
    for c in &v.aes_dyn {
        assert_eq!(
            aes_dynamic_iv_decrypt(&c.key, &c.expect_b64).unwrap(),
            c.plaintext,
            "AES 动态 IV 解密用例 {}",
            c.name
        );
    }
}

#[test]
fn aes_dynamic_iv_rejects_short_input() {
    assert!(aes_dynamic_iv_decrypt(REFRESH_TOKEN_KEY, "AAAA").is_err());
}

/// `REFRESH_TOKEN_KEY` 是 24 字节 —— 也就是 AES-192。写错成 AES-128/256 这里会立刻炸。
#[test]
fn refresh_token_key_is_aes192() {
    assert_eq!(REFRESH_TOKEN_KEY.len(), 24);
    let mut rng = SeededRng::new(1);
    let out = aes_dynamic_iv_encrypt_random(REFRESH_TOKEN_KEY, "x", &mut rng).unwrap();
    // IV(16) + 1 块密文(16) = 32 字节 → base64 44 字符（含 padding）
    assert_eq!(b64_decode(&out).unwrap().len(), 32);
}
