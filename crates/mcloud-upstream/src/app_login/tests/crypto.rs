// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

// ── 加解密 ──

#[test]
fn encrypt_body_produces_the_measured_wire_length() {
    let crypto = AppLoginCrypto::production();
    let wire = crypto.encrypt_body(&captured_plaintext()).unwrap();
    assert_eq!(wire.len(), 384, "186 字节明文 → 192 字节密文 → 384 hex");
    assert_eq!(wire, wire.to_ascii_lowercase(), "请求体是**小写** hex");
    assert!(wire.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn implementation_plaintext_is_byte_identical_to_the_live_probe() {
    // 这条把"活体探针"和"Rust 实现"接起来，是加密这一层能收工的依据：
    //
    //   探针送出下面这串（同样的键序、同样的值）→ 上游回 `9441 验证码不正确`
    //   → 说明**服务端成功解密**了它；
    //   而 Rust 的 `sms_thirdlogin_body(...).to_string()` 逐字节等于它
    //   → 所以 Rust 产出的请求体也一定被服务端解得开。
    //
    // 一旦有人改了字段名/字段类型/键序策略，这条会先炸 ——
    // 而不是要等一次真实登录才发现。
    assert_eq!(
        captured_plaintext(),
        r#"{"clienttype":"412","cpid":"55","dycpwd":"429344","extInfo":{"ifOpenAccount":"1"},"loginMode":"0","msisdn":"19900000000","pintype":"8","random":"hOjseB","verType":"1","version":"12.6.0"}"#
    );
    // 与抓包明文同长（186），但键序不同 —— 键序不参与服务端校验
    assert_eq!(captured_plaintext().len(), 186);
}

#[test]
fn encrypt_body_matches_the_golden_vector() {
    // 这条密文由抓包侧的独立实现算出，钉的是"上游层有没有多加/少加一次错位"。
    //
    // 注意它的明文是 **serde_json 的键序**（按字母排序），与抓包里客户端的
    // 字典序不同 —— 所以这条密文**不等于**抓包里那条。
    // "与抓包逐字节相同"的那条断言在 core（`crypto.rs` 的
    // `app_login_body_is_ecb_hex_with_the_tail_ten_moved_to_the_front`），
    // 那里用的是抓包原始键序的明文。
    // 键序不参与服务端校验：活体探针两种键序都回 `9441`。
    let wire = AppLoginCrypto::production()
        .encrypt_body(&captured_plaintext())
        .unwrap();
    assert_eq!(
        wire,
        "17e30bcd98e6e4bdccc6b6e0c034688a3d2db656484a7de56ee67b2580af3dde4075b72cd78a11415da4b19ce641dbf7fad4a096f842da2556008fbbc3521d6ebf0499470b3dcf08532c25cf5ce6ac9052c20db6035c039af766e0206a772f94c3933802caaa302cc5e6d3c7160711a3d966cfc770568650de3a503048bb660f9e57597dc82cf15674c8f622823bc517ef2f7f51483fdff8b8191c6f1e6335d9c48aaba2d076b067ff091c65f3a107a60bd61352d9718a0466512bce9a4397b6"
    );
}

#[test]
fn key_order_does_not_change_the_decryptable_payload() {
    // 抓包键序（客户端字典序）与实现键序（serde_json 字母序）都能被解回同样的字段集。
    // 这条是"键序不重要"的离线表述；联网版是活体探针（两种键序都回 9441）。
    let captured_order = concat!(
        r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","#,
        r#""pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","#,
        r#""msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#,
    );
    let crypto = AppLoginCrypto::production();
    let a: Value = serde_json::from_str(
        &crypto
            .decrypt_body(&crypto.encrypt_body(captured_order).unwrap())
            .unwrap(),
    )
    .unwrap();
    let b: Value = serde_json::from_str(
        &crypto
            .decrypt_body(&crypto.encrypt_body(&captured_plaintext()).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(a, b, "两种键序必须解出同一份字段");
    // 键序确实不同（否则这条测试就没意义了）
    assert_ne!(captured_order, captured_plaintext());
}

#[test]
fn encrypt_then_decrypt_round_trips() {
    let crypto = AppLoginCrypto::production();
    let plain = captured_plaintext();
    let wire = crypto.encrypt_body(&plain).unwrap();
    assert_eq!(crypto.decrypt_body(&wire).unwrap(), plain);
}

#[test]
fn decrypt_body_accepts_the_response_side_shape() {
    // 响应 `data` 与请求体同一套算法 —— 抓包里响应是 13632 字符（6816 字节），
    // 这里用一个小载荷验证同一函数能解，不需要把 13 KB 的密文塞进测试。
    let crypto = AppLoginCrypto::production();
    let session = r#"{"account":"19900000000","belongsTo":2}"#;
    let wire = crypto.encrypt_body(session).unwrap();
    assert_eq!(crypto.decrypt_body(&wire).unwrap(), session);
    // 顺带钉住：`decrypt_user_info` 走的是同一个解密路径
    let v = crypto.decrypt_user_info(&wire).unwrap();
    assert_eq!(v["account"], json!("19900000000"));
}

#[test]
fn decrypt_body_trims_whitespace_but_does_not_tolerate_the_wrong_direction() {
    let crypto = AppLoginCrypto::production();
    let plain = captured_plaintext();
    let wire = crypto.encrypt_body(&plain).unwrap();

    // 上游有时会把 JSON 字符串里的 hex 带上换行/空格
    assert_eq!(
        crypto.decrypt_body(&format!("  {wire}\n")).unwrap(),
        plain,
        "两侧空白应当被容忍"
    );

    // 但**不能**容忍"没反错位"：直接喂 ECB 密文必须失败。
    // 这条很重要 —— 错位不改变字符集，漏做反错位会解出乱码而不是报错，
    // 真正兜底的是 PKCS7 校验。
    let ecb = aes_ecb_pkcs7_encrypt_hex(APP_LOGIN_AES_KEY, &plain).unwrap();
    assert!(
        crypto.decrypt_body(&ecb).is_err(),
        "喂未反错位的 ECB 密文必须报错，而不是解出乱码"
    );
}

#[test]
fn alt_key_does_not_decrypt_production_ciphertext() {
    // 用错密钥（备用那把）的表现就是上游的 `9103`。在单测层面先撞一次，
    // 免得要等一次真实请求才发现密钥抄错了。
    let prod = AppLoginCrypto::production();
    let alt = AppLoginCrypto::alt_env();
    assert_ne!(prod.key(), alt.key());

    let wire = prod.encrypt_body(&captured_plaintext()).unwrap();
    assert!(alt.decrypt_body(&wire).is_err());
}

#[test]
fn production_key_is_the_environment_yes_branch() {
    // 活体探针结论：`lYzvvBMUjucw+h^R` 才是生产流量用的那把，
    // 用备用那把上游回 `9103`。这两行是那次探针的代码化表述。
    assert_eq!(AppLoginCrypto::production().key(), "lYzvvBMUjucw+h^R");
    assert_eq!(AppLoginCrypto::alt_env().key(), "+09T5lHhO+smS7x@");
    // 长度都是 16 → AES-128（不是 24/32）
    assert_eq!(APP_LOGIN_AES_KEY.len(), 16);
    assert_eq!(APP_LOGIN_AES_KEY_ALT.len(), 16);
}
