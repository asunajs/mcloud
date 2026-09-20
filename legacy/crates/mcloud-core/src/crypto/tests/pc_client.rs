// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

#[test]
fn pc_client_constants_match_the_source_file() {
    // 常量单独钉一次：它们是"从客户端源码抄来的事实"，
    // 改了它们等于换了密钥，必须有人主动改这里
    let v = pc_vectors();
    assert_eq!(v["constants"]["LOGIN_AES_KEY"], "qPqDw263XgFgL3u8");
    assert_eq!(v["constants"]["LOGIN_AES_KEY_TEST"], "2ErfJus1Ofr@2o24");
    assert_eq!(v["constants"]["PASSWORD_HASH_PREFIX"], "fetion.com.cn:");
}

#[test]
fn decrypts_pc_client_login_data_with_source_key() {
    let v = pc_vectors();
    let key = v["constants"]["LOGIN_AES_KEY"].as_str().unwrap();

    for case in v["loginDataDecrypt"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let cipher = case["cipherHex"].as_str().unwrap();
        let got = aes_ecb_pkcs7_decrypt_hex(key, cipher);

        if case["expectError"].as_bool() == Some(true) {
            assert!(got.is_err(), "{name}: 非法填充必须报错，实际解出 {got:?}");
            continue;
        }
        assert_eq!(
            got.unwrap_or_else(|e| panic!("{name}: {e}")),
            case["expectPlain"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn aes_ecb_rejects_malformed_ciphertext() {
    let key = "qPqDw263XgFgL3u8";
    // 输入是 hex，不是 base64 —— 传 base64 必须失败而不是解出乱码
    assert!(aes_ecb_pkcs7_decrypt_hex(key, "RTUyNkUxRjNGOTc2NTQ0NA==").is_err());
    // 长度不是 16 的倍数
    assert!(aes_ecb_pkcs7_decrypt_hex(key, "0011223344556677").is_err());
    // 空密文
    assert!(aes_ecb_pkcs7_decrypt_hex(key, "").is_err());
}
