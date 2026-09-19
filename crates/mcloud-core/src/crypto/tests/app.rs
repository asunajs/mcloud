// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── App 端（MCLoginCryptoUtil）：AES-ECB 加密 + 末 10 位错位 ──

/// App 端登录体的 AES 密钥（`MCLoginCryptoUtil +AESEncrypt:` 的
/// `isCurrentEnvironment = YES` 分支，反汇编实证）。
const APP_KEY: &str = "lYzvvBMUjucw+h^R";

/// 抓包里那条 `thirdlogin` 明文的**脱敏**形态（真号码换成 `199` 号段）。
/// 长度 186 与实测一致 —— 这是"形状没变"的硬约束。
const APP_LOGIN_PLAINTEXT: &str = concat!(
    r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","#,
    r#""pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","#,
    r#""msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#,
);

#[test]
fn app_login_plaintext_keeps_the_captured_length() {
    // 186 字节 → PKCS7 补 6 → 192 字节 → 384 个 hex 字符。
    // 这三个数都直接来自抓包（docs/capture/app-sms-login-samples.txt §6.1），
    // 一旦有人"顺手"改了字段，长度会先炸。
    assert_eq!(APP_LOGIN_PLAINTEXT.len(), 186);
}

#[test]
fn app_login_body_is_ecb_hex_with_the_tail_ten_moved_to_the_front() {
    // 期望值由本仓库之外、独立实现的 AES-ECB 算出（抓包侧的对照脚本），
    // 不是 Rust 侧自产自销：它同时钉住了 ECB 模式、PKCS7 填充、密钥、hex 小写，
    // 以及**错位方向**（末 10 位到最前，不是反过来）。
    let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    assert_eq!(ecb.len(), 384);
    let wire = shift_tail10_to_front(&ecb);
    assert_eq!(
        wire,
        "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db623ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff5600844ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b16c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0"
    );

    // 错位方向反了会得到完全不同的串 —— 单独钉一条，避免"看起来也像随机 hex"。
    assert_ne!(wire, shift_head10_to_back(&ecb));
    assert_eq!(
        &wire[..10],
        &ecb[ecb.len() - 10..],
        "前 10 位来自 ECB 密文的末 10 位"
    );
    assert_eq!(&wire[10..], &ecb[..ecb.len() - 10]);
}

#[test]
fn app_login_shift_is_its_own_inverse_on_real_lengths() {
    let wire =
        shift_tail10_to_front(&aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap());
    assert_eq!(shift_head10_to_back(&wire).len(), 384);

    // 往返：加密 → 错位 → 反错位 → 解密，必须回到原文
    let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    let back = shift_head10_to_back(&shift_tail10_to_front(&ecb));
    assert_eq!(back, ecb);
    assert_eq!(
        aes_ecb_pkcs7_decrypt_hex(APP_KEY, &back).unwrap(),
        APP_LOGIN_PLAINTEXT
    );
}

#[test]
fn app_login_shift_is_identity_for_short_strings() {
    // 客户端是 `p = MAX(len, 10) - 10`：长度 ≤ 10 时两段拼回来等于原串。
    // 这里照抄行为而不是"顺手校验长度"—— 上游真回了个短串时，
    // 抛错会让调用方拿到一个和客户端不一致的结果。
    for s in ["", "a", "0123456789", "short"] {
        assert_eq!(shift_tail10_to_front(s), s, "{s}");
        assert_eq!(shift_head10_to_back(s), s, "{s}");
    }
    // 恰好 11 位：末 10 位（`123456789A`）到最前，首字符 `0` 落到末尾
    assert_eq!(shift_tail10_to_front("0123456789A"), "123456789A0");
    // 反过来：末字符 `A` 提到最前，前 10 位顺次后移
    assert_eq!(shift_head10_to_back("0123456789A"), "A0123456789");
    // 两者互为逆运算 —— 这是"错位方向没写反"的判据。
    // （n = 11 时两个方向在字面上会重合，所以只断言"往返回到原值"才判得准。）
    assert_eq!(
        shift_head10_to_back(&shift_tail10_to_front("0123456789A")),
        "0123456789A"
    );
    assert_eq!(
        shift_tail10_to_front(&shift_head10_to_back("0123456789A")),
        "0123456789A"
    );
}

#[test]
fn app_login_wrong_key_is_what_9103_looks_like() {
    // 活体探针的反面：换一把 key（App 的备用 key）加密，上游会回 `9103 输入参数不合法`。
    // 本测试不需要联网，它只钉住"两把 key 产出的密文确实不同"这件事 ——
    // 这样"密钥选错"在单测层面就能被发现，而不是要等一次真实请求。
    let alt = aes_ecb_pkcs7_encrypt_hex("+09T5lHhO+smS7x@", APP_LOGIN_PLAINTEXT).unwrap();
    let prod = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    assert_ne!(alt, prod);
    // 用错 key 去解会**明确失败**（PKCS7 非法），而不是静默产出半个凭据
    assert!(aes_ecb_pkcs7_decrypt_hex("+09T5lHhO+smS7x@", &prod).is_err());
}

#[test]
fn app_login_encrypt_length_rule_matches_the_capture() {
    // hex_len = 32 * ceil((pt_len + 1) / 16)：抓包里 186 → 384，这条规律写死，
    // 免得有人把 PKCS7 换成"零填充"（那样 186 字节会变成 192 但末块内容不同，
    // 长度看不出来 —— 所以长度只是必要条件，不是充分条件，真正的判据是上面的密文相等）。
    for pt_len in [1usize, 7, 15, 16, 17, 186, 200] {
        let text = "x".repeat(pt_len);
        let hex = aes_ecb_pkcs7_encrypt_hex(APP_KEY, &text).unwrap();
        let expect_blocks = (pt_len + 1).div_ceil(16);
        assert_eq!(
            hex.len(),
            expect_blocks * 32,
            "明文 {pt_len} 字节时密文长度不对"
        );
    }
}

#[test]
fn sha1_upper_matches_pc_client_encrypt_by_hash() {
    let v = pc_vectors();
    for case in v["sha1Upper"].as_array().unwrap() {
        let got = sha1_hex_upper(case["input"].as_str().unwrap().as_bytes());
        assert_eq!(got, case["expect"].as_str().unwrap(), "{}", case["name"]);
    }

    // 大小写不是风格问题：结果直接进请求体的 dycpwd，服务端按十六进制解析
    let hashed = sha1_hex_upper(b"fetion.com.cn:pass1234");
    assert_eq!(hashed, hashed.to_ascii_uppercase());
    assert_eq!(hashed.len(), 40, "SHA-1 是 40 位十六进制");
}
