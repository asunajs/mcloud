// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

#[test]
fn match_key_drops_query_entirely() {
    assert_eq!(
        match_key(
            "post",
            "https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0&_t=1699999999"
        ),
        "POST m.mcloud.139.com/ycloud/signin/page/exchangeList"
    );
    assert_eq!(
        match_key("GET", "https://a.example.com/x"),
        "GET a.example.com/x"
    );
}

#[test]
fn redact_url_strips_volatile_and_masks_sensitive() {
    let url = "https://a.example.com/api?clientVersion=12.6.0&_t=1699999999&nonce=abc&auth=SECRET";
    let out = redact_url(url);
    assert!(out.contains("clientVersion=12.6.0"), "业务参数保留: {out}");
    assert!(!out.contains("_t="), "时间戳应剔除: {out}");
    assert!(!out.contains("nonce="), "随机数应剔除: {out}");
    assert!(!out.contains("SECRET"), "凭据不得残留: {out}");
    assert!(out.contains(&format!("auth={PLACEHOLDER}")));
}
