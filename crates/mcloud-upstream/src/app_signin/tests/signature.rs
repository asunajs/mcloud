// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::generate_signature;

// `sign_salt` 只在测试里用（把常量与向量、与 core 三者钉在一起），
// 生产代码走的是 `generate_signature`，所以它不进模块顶部的 import。
use mcloud_core::sign_salt;

// ───────────────────────── 签名 ─────────────────────────

#[test]
fn h5_salt_is_the_pc_signing_salt() {
    // 签到页的盐**就是** PC 端签名用的第二个盐 —— 所以能直接复用
    // `generate_signature`。这条断言把常量与向量、与 core 三者钉在一起：
    // `SIGN_SALT_MAP` 哪天变了，这里先炸，而不是"签到忽然回 401"。
    let v = vectors();
    let expected = v["constants"]["H5_SIGN_SALT"].as_str().unwrap();
    assert_eq!(sign_salt(H5_SIGN_SALT_KEY), expected);
    assert_eq!(expected, "sekaMdYYLIZfbCfm");
}

#[test]
fn real_capture_signature_is_reproduced() {
    // 最有价值的一条：期望值是**抓包里服务端接受的那个签名**，不是算出来的。
    // 它同时锁住"盐 = saltKey 2"、"末段 other 为空串"、"拼串顺序"三件事。
    let v = captured_signature();
    let sign = sign_h5_request_with(
        v["requestId"].as_str().unwrap(),
        v["timestamp"].as_str().unwrap(),
        v["nonce"].as_str().unwrap(),
    );
    assert_eq!(sign.signature, v["expectMd5"].as_str().unwrap());
    assert_eq!(v["saltKey"].as_u64().unwrap() as u8, H5_SIGN_SALT_KEY);
    assert_eq!(v["other"].as_str().unwrap(), "");
}

#[test]
fn signature_uses_uuids_and_the_injected_timestamp() {
    let mut rng = SeqRandom::new(7);
    let sign = sign_h5_request(&mut rng, "1789800128813");

    assert_eq!(sign.timestamp, "1789800128813", "时间戳必须原样透传");
    for (name, value) in [("request_id", &sign.request_id), ("nonce", &sign.nonce)] {
        assert_eq!(value.len(), 36, "{name} 应是 36 字符的 UUID");
        assert_eq!(value.matches('-').count(), 4, "{name} 应有 4 个连字符");
    }
    assert_ne!(
        sign.request_id, sign.nonce,
        "两个 UUID 不能是同一个 —— 它们是两次独立取随机"
    );
    assert_eq!(sign.signature.len(), 32);
    // 签名确实是把那两个 uuid 喂进算式得到的，不是别的东西
    assert_eq!(
        sign.signature,
        generate_signature(
            &sign.request_id,
            "1789800128813",
            &sign.nonce,
            H5_SIGN_SALT_KEY,
            ""
        )
    );
}
