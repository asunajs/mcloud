// 由同目录 mod.rs 提供确定性随机源等 helper；本文件是按主题拆出的用例。
use super::*;

// ── 请求体：逐字段断言 ──

#[test]
fn send_sms_code_body_is_all_strings_and_has_the_typo_field() {
    let body = send_sms_code_body(PHONE, "");
    assert_eq!(
        body,
        json!({
            "mode": "0",
            "phoneNumber": "19900000000",
            "reqType": "3",
            "clientType": "412",
            "random": "qwert",
            "puzzleVerfycode": "",
        })
    );
    // 逐字段再钉一遍类型：PC 端这两个是**数字**，抄过来会得到 9103
    assert!(
        body["mode"].is_string(),
        "App 端 mode 是字符串，PC 端才是数字"
    );
    assert!(body["reqType"].is_string());
    assert!(body["clientType"].is_string());
    // 首次请求是**空串**，不是 null、也不是省略字段
    assert_eq!(body["puzzleVerfycode"], json!(""));
    // 字段名是 verfy 不是 verify（上游拼错了，照抄）
    assert!(body.get("puzzleVerifycode").is_none());
    assert!(body.get("puzzleVerfycode").is_some());
    // `random` 是**硬编码常量**，不是每请求随机：
    // 跨一个月的 4 次抓包都是同一个值（见常量文档）。
    assert_eq!(SEND_SMS_RANDOM, "qwert");
    assert_eq!(body["random"], json!(SEND_SMS_RANDOM));
}

#[test]
fn thirdlogin_body_matches_the_captured_field_table() {
    let body = sms_thirdlogin_body(PHONE, "429344", "hOjseB");
    assert_eq!(
        body,
        json!({
            "cpid": "55",
            "extInfo": { "ifOpenAccount": "1" },
            "dycpwd": "429344",
            "pintype": "8",
            "loginMode": "0",
            "verType": "1",
            "version": "12.6.0",
            "msisdn": "19900000000",
            "random": "hOjseB",
            "clienttype": "412",
        })
    );

    // 四处最容易被 PC 端带偏的地方，单独钉：
    assert_eq!(
        body["pintype"],
        json!("8"),
        "App 端短信登录也是 8，不是 PC 的 5"
    );
    assert_eq!(body["cpid"], json!("55"), "不是 PC 的 277/332");
    assert!(
        body["extInfo"]["ifOpenAccount"].is_string(),
        "App 端 ifOpenAccount 是字符串；PC 端是数字 1"
    );
    // 同一个字段名大小写不同：这里是小写 t 的 clienttype
    assert!(body.get("clienttype").is_some());
    assert!(
        body.get("clientType").is_none(),
        "getSmsCode 才叫 clientType"
    );
    // dycpwd 是验证码明文（账密登录才哈希）
    assert_eq!(body["dycpwd"], json!("429344"));
}

#[test]
fn captured_plaintext_keeps_the_measured_length() {
    // 186 字节 —— 与抓包一致。密文长度是 384 字符，两者是同一个约束的两种表述。
    assert_eq!(captured_plaintext().len(), 186);
}
