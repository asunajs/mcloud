// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

/// 真实 HAR 上暴露出来的四类形态，逐条钉住。
///
/// 这些不是假想用例，是把 36 份真实 HAR 跑一遍之后自检报出来的形状：
/// 它们都在"键名黑名单"之外，靠按名判断一条都拦不住。
#[test]
fn real_world_shapes_are_cleaned_end_to_end() {
    // ① 内嵌 JSON 里的 `userdomain_id`（下划线形态，靠归一化命中黑名单）
    let req_body = r#"{"header":{"custom":{"userdomain_id":"1990000000000000001"}}}"#;
    let entry = build_fixture_entry(
        "POST",
        "https://data.example.com:8080/newapp/action",
        &[
            // ② 名字不在白名单里的 cookie 类头：实测最常见的漏点
            h("cookieToken", "eyJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATURE"),
            h("hecaiyundata2021jssdkcross", "YZsidssolgbmPq1234567890abcdefghij"),
            h("user-agent", "mcloud/12.6.0"),
        ],
        req_body.as_bytes(),
        200,
        &[h("content-type", "application/json")],
        // ③ 无害键名 + 长 base64 值（实测形态：base64 的 `mobile:手机号:token`）
        // ④ 响应里的 CDN 长路径段
        r#"{"data":{"blob":"bW9iaWxlOjE5OTAwMDAwMDAxOmFiY2RlZmdoaWprbG1ub3A=","records":[{"contentImageUrl":"https://cdn3.cmread.com/a/b/cccccccccccccccccccccccccccccccccccccccccccccccc.png"}]}}"#
            .as_bytes(),
    );

    let text = entry.to_string();
    let leaks = scan_for_leaks(&text);
    assert!(leaks.is_empty(), "仍有残留: {leaks:?}");

    for secret in ["1990000000000000001", "YZsidssolgbmPq", "PAYLOAD.SIGNATURE"] {
        assert!(!text.contains(secret), "{secret} 不得出现在 fixture 中");
    }
    // 白名单内的 header 与业务结构必须留下，否则夹具没法用
    assert!(text.contains("mcloud/12.6.0"), "user-agent 应保留");
    assert!(text.contains("contentImageUrl"), "业务键名应保留");
    assert_eq!(
        entry["match"]["key"],
        "POST data.example.com:8080/newapp/action"
    );
}

/// URL 形状的值：路径留下，但里面没名分的长串仍要打码。
///
/// 这条钉的是"按 key 判断不够"——实测有接口把整段 base64 塞进 query 参数 `e`，
/// 参数名毫无提示性。
#[test]
fn url_values_keep_path_but_lose_unnamed_payloads() {
    let url = format!(
        "https://data.example.com/newapp/action?e={}&m=1&clientVersion=12.6.0",
        "a".repeat(300)
    );
    let entry = build_fixture_entry("POST", &url, &[], b"", 200, &[], b"{}");
    let text = entry.to_string();
    assert!(scan_for_leaks(&text).is_empty(), "仍有残留");
    assert!(text.contains("m=1"), "短参数应保留");
    assert!(
        text.contains("clientVersion=12.6.0"),
        "业务参数应保留: {text}"
    );
    assert!(text.contains("newapp/action"), "路径应保留");
}
