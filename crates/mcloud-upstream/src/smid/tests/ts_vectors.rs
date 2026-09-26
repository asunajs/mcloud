//! TS 黄金向量对拍：smid 四段 + 整份特征体的逐字段复现。

use super::*;

// ==================== TS 黄金向量对拍 ====================

#[test]
fn smid_string_matches_the_ts_vectors() {
    let v = smid_vectors();
    assert!(!v.smid_string.is_empty(), "smidString 段不能为空");
    for c in &v.smid_string {
        let got = smid_string(&c.time_digits, &c.uid);
        assert_eq!(got, c.expect, "用例 {}", c.name);
        assert_eq!(got.len(), 63, "用例 {}", c.name);
    }
}

#[test]
fn private_id_matches_the_ts_vectors() {
    let v = smid_vectors();
    assert!(!v.smid_pri_id.is_empty(), "smidPriId 段不能为空");
    for c in &v.smid_pri_id {
        let got = private_id(&c.uid);
        assert_eq!(got, c.expect, "用例 {}", c.name);
        assert_eq!(got.len(), 16, "AES 密钥必须是 16 个字符：用例 {}", c.name);
    }
}

#[test]
fn tn_matches_the_ts_vectors() {
    let v = smid_vectors();
    assert!(!v.smid_tn.is_empty(), "smidTn 段不能为空");
    for c in &v.smid_tn {
        // 向量给的 target 里 `tn` 就是空串：这条同时锁住"摘要先算、再写回"的顺序
        assert_eq!(
            c.target["tn"],
            Value::String(String::new()),
            "用例 {}",
            c.name
        );
        assert_eq!(get_tn_md5(&c.target), c.expect, "用例 {}", c.name);
    }
}

#[test]
fn the_whole_des_rule_table_matches_the_ts_vectors() {
    // 整份比对而不是抽查：25 条规则里键名或密钥写歪一条，这里就断
    let v = smid_vectors();
    assert!(!v.smid_des_rules.is_empty(), "smidDesRules 段不能为空");
    for c in &v.smid_des_rules {
        assert_eq!(
            encrypt_by_des_rules(&c.target).unwrap(),
            c.expect,
            "用例 {}",
            c.name
        );
    }
    // 向量必须真的覆盖到每一条规则
    // （否则"整份通过"可能只是恰好没测到某条）
    let covered: std::collections::BTreeSet<&str> = v
        .smid_des_rules
        .iter()
        .flat_map(|c| c.expect.as_object().unwrap().keys().map(String::as_str))
        .collect();
    for (field, rule) in RULES {
        assert!(
            covered.contains(rule.wire_name),
            "规则 {field}→{} 没被任何向量覆盖",
            rule.wire_name
        );
    }
}

/// 按向量给的特征体反推出那份"稳定特征"。
///
/// 故意**不**在测试里再抄一遍常量：这样下面那条断言才等于证明
/// 「整份特征体只由 [`SmFingerprint`] + 时间 + 旧 deviceId 决定」，
/// 而不是"两边都抄了同一份字面量"。
fn fingerprint_of(target: &Value) -> SmFingerprint {
    let text = |key: &str| target[key].as_str().unwrap_or_default().to_string();
    let int = |key: &str| target[key].as_i64().unwrap_or_default();
    // 屏幕与 dpr 在特征体里只以 clientSize / res 的形式出现
    let res = text("res");
    let res: Vec<&str> = res.split('_').collect();
    SmFingerprint {
        ua: text("ua"),
        page_url: text("url"),
        platform: text("platform"),
        screen_width: res[0].parse().unwrap(),
        screen_height: res[1].parse().unwrap(),
        dpr: res[2].parse().unwrap(),
        max_touch_points: int("maxTouchPoints"),
        connection_rtt: int("connectionRtt"),
        cpucount: int("cpucount"),
        canvas: text("canvas"),
    }
}

/// 按脚本吐 uuid / 字节的随机源：用来把特征体**逐字段**复现成向量那一份。
struct Scripted {
    uuids: std::vec::IntoIter<String>,
    bytes: std::collections::VecDeque<u8>,
}

impl Random for Scripted {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst {
            *byte = self.bytes.pop_front().unwrap_or(0);
        }
    }

    fn uuid_v4(&mut self) -> String {
        self.uuids.next().expect("脚本里的 uuid 用完了")
    }
}

/// 造 `Scripted`：`level` 要落到 0.5，得让那两字节 = 32768
/// （`32768/65535*100` 四舍五入 = 50）。
fn scripted(smid_uid: &str, target: &Value) -> Scripted {
    Scripted {
        uuids: vec![
            smid_uid.to_string(),
            target["vpw"].as_str().unwrap().to_string(),
            target["trees"].as_str().unwrap().to_string(),
        ]
        .into_iter(),
        bytes: [0u8, 0, 128, 0].into_iter().collect(),
    }
}

#[test]
fn every_field_of_the_feature_body_traces_back_to_the_ts_vector() {
    // 这是"绑定账号"最硬的一条：从向量反推指纹，再正向拼回去，逐字节相等。
    // 少一个字段、多一个字段、或者某个字段其实来自 rng，都会在这里断掉。
    let v = smid_vectors();
    for (c, smid_uid) in v.smid_tn.iter().zip([
        "6b4a3c2d-1111-4222-8333-444444444444",
        "9a2b3c4d-1111-4222-8333-444444444444",
    ]) {
        let fingerprint = fingerprint_of(&c.target);
        let upload = SmUpload {
            time_digits: &c.target["smid"].as_str().unwrap()[..14],
            now_millis: c.target["svm"].as_i64().unwrap(),
            old_device_id: c.target["box"].as_str().unwrap(),
        };
        let built = fingerprint_target(&fingerprint, &upload, &mut scripted(smid_uid, &c.target));
        assert_eq!(built, c.target, "用例 {}", c.name);

        // 顺带钉住那几个"写死但不是指纹"的字段：值来自旧代码，改一个就是换协议
        assert_eq!(built["protocol"], 4);
        assert_eq!(built["version"], "3.0.0");
        assert_eq!(built["sdkver"], "3.0.0");
        assert_eq!(built["subVersion"], "1.0.0");
        assert_eq!(built["rtype"], "all");
        assert_eq!(built["status"], "0010");
        assert_eq!(built["timezone"], -480);
        assert_eq!(built["time"], 7);
        assert_eq!(built["cdp"], 0);
        assert_eq!(built["os"], "web");
        assert_eq!(built["organization"], ORGANIZATION);
        assert_eq!(built["appId"], APP_ID);
        assert_eq!(built["plugins"], PLUGINS);
        assert_eq!(built["referer"], "");
    }
}

#[test]
fn the_bound_fingerprint_round_trips_through_the_feature_body() {
    // 反推出来的指纹再序列化一次，必须还在"稳定字段"的范围内 ——
    // 这条防的是"向量悄悄改了，而测试只比对了拼接结果"
    let v = smid_vectors();
    let fingerprint = fingerprint_of(&v.smid_tn[0].target);
    let text = fingerprint.to_json_string().unwrap();
    let back = SmFingerprint::from_json_str(&text).unwrap();
    assert_eq!(back, fingerprint);
    assert_eq!(
        Value::String(back.client_size()),
        v.smid_tn[0].target["clientSize"]
    );
}
