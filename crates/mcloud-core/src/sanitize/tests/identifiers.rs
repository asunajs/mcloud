// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

/// 标识形状的数字必须被改成合成号段 —— 判据与仓库守卫逐条对齐。
#[test]
fn identifier_shaped_numbers_are_remapped_to_synthetic_range() {
    // 两个"一眼假"的顺序占位值（已在 scripts/check-identifiers.cjs 的 ALLOWED 表登记）：
    // 它们与真实值同为 11 / 19 位且以 1 开头，因此能真正走到重映射分支。
    const FAKE_PHONE: &str = "12345678901";
    const FAKE_DOMAIN_ID: &str = "1000000000000000001";
    const FAKE_DOMAIN_ID_2: &str = "1234567890123456789";

    // 11 位：手机号形状。改写后长度与 199 前缀都要保持
    let phone = synthesize_identifiers(&format!("phone={FAKE_PHONE}"));
    assert_eq!(phone.len(), format!("phone={FAKE_PHONE}").len());
    assert!(phone.starts_with("phone=199"), "{phone}");

    // 19 位：userDomainId 形状。实测 `taskId` 也是这个形状，
    // 所以"反正不是标识"这条退路并不存在
    let task_id = format!("\"taskId\":\"{FAKE_DOMAIN_ID}\"");
    let remapped = synthesize_identifiers(&task_id);
    assert_eq!(remapped.len(), task_id.len(), "长度要保持，否则解析会变形");
    assert!(!remapped.contains(FAKE_DOMAIN_ID));
    assert!(remapped.contains("199"), "{remapped}");

    // 确定性：同输入同输出，否则夹具每次重新生成 diff 全是噪声
    assert_eq!(
        synthesize_identifiers(FAKE_DOMAIN_ID),
        synthesize_identifiers(FAKE_DOMAIN_ID)
    );
    assert_ne!(
        synthesize_identifiers(FAKE_DOMAIN_ID),
        synthesize_identifiers(FAKE_DOMAIN_ID_2),
        "不同原值应映射到不同结果"
    );

    // 形状不符的一律不动：位数不对、或两侧挨着字母数字
    for untouched in [
        "1699999999",
        "1234567890",
        "a12345678901",
        "12345678901a",
        "123",
    ] {
        assert_eq!(
            synthesize_identifiers(untouched),
            untouched,
            "{untouched} 不该被改"
        );
    }
}

/// 标识重映射必须覆盖所有文本入口，而不是只在某一条分支里生效。
///
/// 这一路踩到的坑几乎都是"某条分支只做了一半"（URL 分支、非 JSON 分支、
/// header 分支、裸数字…），所以这里逐个入口点名。
#[test]
fn identifier_remap_reaches_every_text_path() {
    // 顺序占位值（已登记），与真实值同形状，所以能真正走到重映射分支
    const REAL_ID: &str = "1000000000000000001";

    let entry = build_fixture_entry(
        "POST",
        // URL query 里的标识
        &format!("https://m.mcloud.139.com/a?taskId={REAL_ID}"),
        // 白名单 header 的值
        &[h("user-agent", &format!("mcloud {REAL_ID}"))],
        // 非 JSON 文本体
        format!("taskId={REAL_ID}").as_bytes(),
        200,
        &[],
        // JSON 体，且写成裸数字
        format!("{{\"taskId\":{REAL_ID}}}").as_bytes(),
    );

    let text = entry.to_string();
    assert!(!text.contains(REAL_ID), "标识在某个入口漏了出去: {text}");
}

#[test]
fn end_to_end_har_like_entry_is_clean() {
    let req_headers = vec![
        h("auth", "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIG"),
        h("Cookie", "jsessionid=ABCDEF123456"),
        h("user-agent", "mcloud/12.6.0"),
    ];
    let req_body = r#"{"prizeId":123,"phone":"19900000001"}"#.as_bytes();
    // 注意：响应体里带中文昵称。`br#"..."#` 是字节串字面量，**不允许非 ASCII 字符**，
    // 所以含 CJK 的样本必须写成 `r#"..."#.as_bytes()`。
    let resp_body =
        r#"{"code":0,"data":{"nickname":"账号2","token":"SECRET_IN_RESPONSE","cloudCount":1286}}"#
            .as_bytes();

    let entry = build_fixture_entry(
        "POST",
        "https://m.mcloud.139.com/ycloud/prize/exchange",
        &req_headers,
        req_body,
        200,
        &[h("content-type", "application/json")],
        resp_body,
    );

    let text = entry.to_string();
    let leaks = scan_for_leaks(&text);
    assert!(leaks.is_empty(), "脱敏后仍有残留: {leaks:?}");

    for secret in [
        "PAYLOAD.SIG",
        "ABCDEF123456",
        "SECRET_IN_RESPONSE",
        "19900000001",
    ] {
        assert!(!text.contains(secret), "{secret} 不得出现在 fixture 中");
    }

    // 结构必须保留，否则 fixture 无法用于回放解析
    assert_eq!(entry["response"]["body_json"]["code"], 0);
    assert_eq!(entry["response"]["body_json"]["data"]["cloudCount"], 1286);
    assert_eq!(entry["response"]["body_json"]["data"]["nickname"], "账号2");
    assert_eq!(
        entry["match"]["key"],
        "POST m.mcloud.139.com/ycloud/prize/exchange"
    );
}
