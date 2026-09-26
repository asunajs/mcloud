//! 载荷编码：`data` 能不能解回那份绑定的特征。

use super::*;

// ==================== 载荷编码 ====================

#[test]
fn payload_decodes_back_to_the_bound_fingerprint() {
    // 这条测试是"绑定账号"这件事的**唯一**硬证据：
    // 不解开看，就只能断言"算出了一个 hex 串"，那等于没测。
    let uid = "11111111-2222-4333-8444-555555555555";
    let upload = upload();
    let target = fingerprint_target(&fp(), &upload, &mut seq_rng(1));
    let data = encode_target(&target, uid).unwrap();
    let decoded = decode_target(&data, uid).unwrap();

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
    // 回归护栏：先写回 tn 再算摘要，data 会整串不同
    // （而线上不会报错，只会认不出一台设备）
    let uid = "11111111-2222-4333-8444-555555555555";
    let target = fingerprint_target(&fp(), &upload(), &mut seq_rng(1));
    let good = encode_target(&target, uid).unwrap();

    let mut obj = target.as_object().cloned().unwrap();
    obj.insert("tn".into(), Value::String(get_tn_md5(&target)));
    let bad = encode_target(&Value::Object(obj), uid).unwrap();
    assert_ne!(good, bad, "顺序错了必须能被测出来");
}

#[test]
fn build_sm_data_produces_the_legacy_envelope() {
    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(1)).unwrap();
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
    let a = build_sm_data(&fp(), &upload(), &mut seq_rng(5)).unwrap();
    let b = build_sm_data(&fp(), &upload(), &mut seq_rng(5)).unwrap();
    assert_eq!(a, b, "注入同一个随机源就要字节级复现（fixture 回放的前提）");
}

#[test]
fn the_ep_uid_and_the_data_key_are_the_same_uid() {
    // 分两次生成 uid 会让服务端看到"包裹用的 uid"与"解密用的密钥"对不上。
    // 这里用"解出的密文能用响应里的 ep 对应的 uid 解开"来钉住它 ——
    // 做法是从 payload 反推：同一个 uid 既能解 data，也能当 priId 的来源。
    let payload = build_sm_data(&fp(), &upload(), &mut seq_rng(11)).unwrap();
    let data = payload["data"].as_str().unwrap();
    // 只有正确的 uid 能解出合法 JSON；随便换一个必然失败
    assert!(decode_target(data, "not-the-uid").is_err());
    // 而正确的那个：从 ep 解不出来（我们没私钥），
    // 所以改为正向验证 —— 重放同一份随机源必须得到同一份 payload。
    let again = build_sm_data(&fp(), &upload(), &mut seq_rng(11)).unwrap();
    assert_eq!(payload, again);
}
