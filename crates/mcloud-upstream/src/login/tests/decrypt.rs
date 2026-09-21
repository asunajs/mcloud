// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

#[test]
fn password_login_hashes_with_the_source_prefix() {
    let v = pc_vectors();
    assert_eq!(v["constants"]["PASSWORD_HASH_PREFIX"], PASSWORD_HASH_PREFIX);
    assert_eq!(PASSWORD_HASH_PREFIX, "fetion.com.cn:");

    let hash = password_hash("pass1234");
    assert_eq!(
        hash,
        v["sha1Upper"][0]["expect"].as_str().unwrap(),
        "SHA-1 前缀 + 大写 hex"
    );

    let body = password_thirdlogin_body(PcPlatform::Windows, "alice", "pass1234");
    // 协议 §6：账密登录 dycpwd 是口令哈希、msisdn 是用户名、ifOpenAccount=0
    assert_eq!(body["pintype"], "9");
    assert_eq!(body["dycpwd"], hash);
    assert_eq!(body["msisdn"], "alice");
    assert_eq!(body["extInfo"]["ifOpenAccount"], 0);
    // 明文口令绝不能出现在请求体里
    assert!(!body.to_string().contains("pass1234"));
}

#[test]
fn login_decryptor_uses_ecb_hex_not_base64() {
    let v = pc_vectors();
    let case = &v["loginDataDecrypt"][0];
    let info = LoginDataDecryptor::production()
        .decrypt_user_info(case["cipherHex"].as_str().unwrap())
        .unwrap();
    assert_eq!(info["account"], "19900000000");
    assert_eq!(info["userId"], "1990000000000000001");

    // 用错密钥要报错，不能返回半截结果
    let wrong = LoginDataDecryptor::with_key("0000000000000000")
        .decrypt_user_info(case["cipherHex"].as_str().unwrap());
    assert!(wrong.is_err(), "换密钥后必须失败，而不是解出乱码当成功");
}

#[test]
fn login_decryptor_reports_padding_failure_instead_of_garbage() {
    let v = pc_vectors();
    let bad = v["loginDataDecrypt"][1]["cipherHex"].as_str().unwrap();
    assert_eq!(v["loginDataDecrypt"][1]["expectError"], true);
    let err = LoginDataDecryptor::production()
        .decrypt_user_info(bad)
        .unwrap_err();
    assert_eq!(err.kind(), mcloud_core::ErrorKind::Parse, "{err}");
}

/// 假解密器：验证调用方在"上游换了加密方式"时的行为，不需要真密钥。
struct PlainTextDecryptor;

impl LoginPayloadDecryptor for PlainTextDecryptor {
    fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
        serde_json::from_str(ciphertext).map_err(|e| Error::Parse(e.to_string()))
    }
}

#[test]
fn decryptor_is_injectable_for_changed_upstream_shapes() {
    // trait 存在的意义：上游若返回明文 JSON，调用方代码不用改
    let v = PlainTextDecryptor
        .decrypt_user_info(r#"{"account":"19900000000"}"#)
        .unwrap();
    assert_eq!(v["account"], "19900000000");
}
