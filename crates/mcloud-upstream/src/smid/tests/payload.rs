// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

use mcloud_core::{b64_decode, gunzip};

// ==================== 载荷编码 ====================

/// 把 `data` 一路解回特征体：AES(固定 IV, priId) → hex → base64 → gzip → JSON。
fn decode_data(data_hex: &str, uid: &str) -> Value {
    let b64 = mcloud_core::aes_cbc_fixed_iv_decrypt_hex(&private_id(uid), data_hex).unwrap();
    let gz = b64_decode(&b64).unwrap();
    let json = gunzip(&gz).unwrap();
    serde_json::from_slice(&json).unwrap()
}

#[test]
fn payload_decodes_back_to_the_bound_fingerprint() {
    // 这条测试是"绑定账号"这件事的**唯一**硬证据：
    // 不解开看，就只能断言"算出了一个 hex 串"，那等于没测。
    let uid = "11111111-2222-4333-8444-555555555555";
    let upload = upload();
    let target = fingerprint_target(&fp(), &upload, &mut SeqRng(1));
    let data = encode_target(&target, uid).unwrap();
    let decoded = decode_data(&data, uid);

    assert_eq!(decoded["lz"], des("j862vr1f", "web")); // os
    assert_eq!(decoded["um"], des("a9kbown8", APP_ID)); // appId
    assert_eq!(decoded["nl"], des("14e30pp1", ORGANIZATION)); // organization
    assert_eq!(decoded["pu"], des("bewicpxu", &fp().ua)); // ua
    assert_eq!(decoded["lm"], des("pgutfbo7", &fp().platform)); // platform
    assert_eq!(decoded["na"], des("7s9eixjh", &fp().client_size())); // clientSize
    assert_eq!(decoded["ut"], des("ht2n6dwf", &fp().res())); // res
    // 不加密的三条：改了名，值原样
    assert_eq!(decoded["protocol"], 4);
    assert_eq!(decoded["version"], "3.0.0");
    assert_eq!(decoded["aw"], "");
    // 没有规则的：原名原值
    assert_eq!(decoded["cdp"], 0);
    assert_eq!(decoded["battery"]["charging"], 0);
    assert_eq!(decoded["smid"].as_str().unwrap().len(), 63);
    // tn：摘要是在 `tn` 还是空串时算的
    assert_eq!(decoded["as"], des("crnw0x7e", &get_tn_md5(&target)));
}

#[test]
fn tn_must_be_digest_of_the_target_while_tn_is_still_empty() {
    // 回归护栏：先写回 tn 再算摘要，data 会整串不同（而线上不会报错，只会认不出一台设备）
    let uid = "11111111-2222-4333-8444-555555555555";
    let target = fingerprint_target(&fp(), &upload(), &mut SeqRng(1));
    let good = encode_target(&target, uid).unwrap();

    let mut obj = target.as_object().cloned().unwrap();
    obj.insert("tn".into(), Value::String(get_tn_md5(&target)));
    let bad = encode_target(&Value::Object(obj), uid).unwrap();
    assert_ne!(good, bad, "顺序错了必须能被测出来");
}

#[test]
fn build_sm_data_produces_the_legacy_envelope() {
    let payload = build_sm_data(&fp(), &upload(), &mut SeqRng(1)).unwrap();
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        keys.contains(&"ep")
            && keys.contains(&"data")
            && keys.contains(&"organization")
            && keys.contains(&"appId"),
        "{keys:?}"
    );
    assert_eq!(payload["os"], "web");
    assert_eq!(payload["encode"], 5);
    assert_eq!(payload["compress"], 2);
    assert_eq!(payload["appId"], APP_ID);
    assert_eq!(payload["organization"], ORGANIZATION);
    let data = payload["data"].as_str().unwrap();
    assert!(
        data.len().is_multiple_of(2) && data.chars().all(|c| c.is_ascii_hexdigit()),
        "data 是 AES 的 hex 文本：{}",
        &data[..40.min(data.len())]
    );
}

#[test]
fn the_same_fingerprint_and_seed_give_the_same_payload() {
    let a = build_sm_data(&fp(), &upload(), &mut SeqRng(5)).unwrap();
    let b = build_sm_data(&fp(), &upload(), &mut SeqRng(5)).unwrap();
    assert_eq!(a, b, "注入同一个随机源就要字节级复现（fixture 回放的前提）");
}
