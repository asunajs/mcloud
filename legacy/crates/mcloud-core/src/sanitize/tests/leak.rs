// 由 sanitize.rs 的 `mod tests` 按主题拆出。
use super::*;

#[test]
fn leak_scanner_actually_detects_a_leak() {
    // 这个用例是"检查器自身"的测试：如果扫描器失灵，
    // 后面所有"无泄漏"断言就都成了空话。
    let dirty = json!({ "auth": "REAL_SECRET_TOKEN" }).to_string();
    assert!(
        !scan_for_leaks(&dirty).is_empty(),
        "扫描器必须能发现未脱敏的敏感字段"
    );

    let dirty_text = "here is a very long secret token: AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ";
    assert!(
        !scan_for_leaks(dirty_text).is_empty(),
        "扫描器必须能发现非结构化的长 token"
    );
}

/// 扫描器不能把"故意保留的 URL"误报成泄漏 —— 否则门禁永远红，比漏报更有害。
#[test]
fn leak_scanner_ignores_urls_but_keeps_tokens() {
    // URL 形状：不该报
    assert!(
        scan_for_leaks("https://m.mcloud.139.com/ycloud/prize/exchange/apply/v3").is_empty(),
        "带 scheme 的 URL 不应误报"
    );
    assert!(
        scan_for_leaks("//cdn.example.com/a/very/long/path/that/definitely/exceeds/40").is_empty(),
        "被截断的 URL 不应误报"
    );
    assert!(
        scan_for_leaks("m.mcloud.139.com/ycloud/prize/exchange/apply/v2").is_empty(),
        "host/path 形状不应误报"
    );

    // 真正的 token：必须报
    assert!(
        !scan_for_leaks("eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATUREPAD123456")
            .is_empty(),
        "JWT 必须被识别"
    );
    assert!(
        !scan_for_leaks("AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ").is_empty(),
        "纯大写长串必须被识别"
    );
}
