//! `task/click` 的测试。
//!
//! 最要紧的两条：**`id` 非正时一枪不发**（那是拿用户的账去试探上游容错），
//! 以及**它是一发写请求**（URL 与判据都必须逐字对上旧实现）。

use super::*;

fn env(body: &str) -> SignInEnvelope {
    SignInEnvelope::parse(body).expect("夹具应是合法 JSON")
}

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// URL 逐字对上旧实现：`?key=<key>&id=<n>`。
#[test]
fn the_url_is_the_legacy_shape() {
    let req = task_click_request("day", 614).unwrap();
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/signin/task/click?key=day&id=614"
    );
}

/// 空 `key` 落旧实现那个兜底值（`${taskStr ?? "task"}`）。
#[test]
fn an_empty_key_falls_back_to_task() {
    let req = task_click_request("", 1).unwrap();
    assert!(req.url.ends_with("?key=task&id=1"), "实得：{}", req.url);
}

/// ⭐ `id` 非正 ⇒ **构造期就失败**，绝不发出去。
///
/// 旧实现没有"id 为 0 也要发"的调用点；而发一发 `id=0` 的打卡是拿用户的账
/// 去试探上游的容错 —— 即使回 `code:0` 也说明不了任何事。
#[test]
fn a_non_positive_id_is_refused_before_any_request() {
    for bad in [0, -1, i64::MIN] {
        let e = task_click_request("day", bad).expect_err("必须拒绝");
        let msg = e.to_string();
        assert!(msg.contains("正整数"), "错误要说清判据：{msg}");
        assert!(msg.contains(&bad.to_string()), "要带上实得值：{msg}");
    }
}

/// `key` 是查询串里的一格 ⇒ 要编码。
#[test]
fn the_key_is_percent_encoded() {
    let req = task_click_request("a b&c=d", 7).unwrap();
    assert!(
        req.url.contains("key=a%20b%26c%3Dd") || req.url.contains("key=a+b%26c%3Dd"),
        "实得：{}",
        req.url
    );
    assert!(req.url.ends_with("&id=7"));
}

/// 它走 H5 那一套头（与同族 GET 一致），**不带** `isdeviceid`。
///
/// ⚠ 这一条钉的是"别顺手改成 `true`"：多一枚 `isdeviceid` 头上游认不认**没有样本**。
#[test]
fn it_carries_the_h5_headers_and_no_device_flag() {
    let req = task_click_request("day", 1).unwrap();
    assert_eq!(header(&req, "Accept"), Some("*/*"));
    assert_eq!(
        header(&req, "appversion"),
        Some(crate::app_signin::H5_APP_VERSION)
    );
    assert_eq!(header(&req, "showloading"), Some("true"));
    assert!(header(&req, "activityid").is_some(), "同族都带这一枚");
    assert_eq!(header(&req, "isdeviceid"), None, "这一发不该带它");
}

/// ⚠ 四发实拍**都没有** `result` ⇒ 这一格恒 `None`，而不是空串。
#[test]
fn a_missing_result_leaves_notice_none() {
    let got = parse_task_click(&env(r#"{"code":0,"msg":"success"}"#)).unwrap();
    assert_eq!(got.notice, None);
    assert_eq!(got.message, "success");
}

/// 上游真给了 `result.msg` 时照收（今天零样本，但形状由旧实现读出来）。
#[test]
fn result_msg_becomes_the_notice() {
    let got = parse_task_click(&env(
        r#"{"code":0,"msg":"success","result":{"msg":"明天再来"}}"#,
    ))
    .unwrap();
    assert_eq!(got.notice.as_deref(), Some("明天再来"));
}

/// `result` 在但不是对象 ⇒ 不当成结论，也不报错（同族既有口径）。
#[test]
fn a_non_object_result_is_tolerated() {
    for body in [
        r#"{"code":0,"msg":"success","result":null}"#,
        r#"{"code":0,"msg":"success","result":"x"}"#,
        r#"{"code":0,"msg":"success","result":[]}"#,
    ] {
        let got = parse_task_click(&env(body)).unwrap();
        assert_eq!(got.notice, None, "实得：{body}");
    }
}

/// 非 0 业务码 ⇒ 报错（成败由信封判，不由 `notice` 判）。
#[test]
fn a_non_zero_code_is_an_error() {
    let e = parse_task_click(&env(r#"{"code":90001,"msg":"登录超时"}"#)).expect_err("必须报错");
    assert!(e.to_string().contains("90001"), "实得：{e}");
}

/// 非 JSON ⇒ 报错，不静默当成功。
#[test]
fn non_json_is_an_error() {
    assert!(SignInEnvelope::parse("<html>502</html>").is_err());
}
