//! 签到任务测试。
//!
//! 重点：**判据是 `infoV3` 而不是 `startSignIn`** —— 这是 mcloud 实测出来的坑，
//! 抄错的实现在"签到成功"时会报失败，用户又点一次就是重复请求。

use super::*;

/// 一份贴近实拍的 `infoV3` 响应。
fn info_body(signed_today: bool, total: i64) -> String {
    format!(
        r#"{{"code":0,"msg":"success","result":{{
          "total":{total},"beforeTotal":9999,"signInPoints":3,"signCount":1,
          "monthDays":7,"curMonthBackup":true,"toReceive":0,
          "cal":[{{"d":31,"s":false,"n":0,"p":0,"t":false,"currentMonth":0}},
                 {{"d":18,"s":true,"n":0,"p":0,"t":false,"currentMonth":1}},
                 {{"d":19,"s":{signed_today},"n":3,"p":0,"t":true,"currentMonth":1}}]}}}}"#
    )
}

// ── 状态解析 ────────────────────────────────────────────────────────────

#[test]
fn parses_status_from_capture_shape() {
    let s = parse_status(&info_body(true, 8027)).unwrap();
    assert_eq!(s.total, 8027, "total 才是现行余额");
    assert_eq!(s.month_days, 7);
    assert!(s.signed_today);
}

#[test]
fn unsigned_today_is_detected() {
    let s = parse_status(&info_body(false, 8024)).unwrap();
    assert!(!s.signed_today);
    assert_eq!(s.total, 8024);
}

/// ⭐ `total` 与 `beforeTotal` **不是**同一个数。
///
/// `beforeTotal` 是"云朵明细页"口径的另一个数，**领取后不动**。
/// 抄错这一个字段的表现是"签到成功但余额没变"。
#[test]
fn does_not_confuse_total_with_before_total() {
    let s = parse_status(&info_body(true, 8027)).unwrap();
    assert_eq!(s.total, 8027);
    assert_ne!(s.total, 9999, "beforeTotal 不是余额");
}

/// `cal` 里 `currentMonth == 0` 的是上月/下月的补位格，不该被当成今天。
#[test]
fn ignores_padding_days_from_other_months() {
    // 把"今天"标记放在一个 currentMonth=0 的格子上 ⇒ 不该算已签
    let body = r#"{"code":0,"result":{"total":1,"cal":[
      {"d":19,"s":true,"t":true,"currentMonth":0}]}}"#;
    let s = parse_status(body).unwrap();
    assert!(!s.signed_today, "非本月的格子不算今天");
}

#[test]
fn missing_cal_means_not_signed() {
    let body = r#"{"code":0,"result":{"total":5}}"#;
    let s = parse_status(body).unwrap();
    assert!(!s.signed_today, "拿不到日历时不能判成已签 —— 那会漏签");
}

#[test]
fn accepts_string_numbers() {
    let body = r#"{"code":0,"result":{"total":"8027","monthDays":"7","cal":[]}}"#;
    let s = parse_status(body).unwrap();
    assert_eq!(s.total, 8027);
    assert_eq!(s.month_days, 7);
}

#[test]
fn non_zero_code_is_an_error() {
    let body = r#"{"code":500,"msg":"服务异常","result":null}"#;
    let e = parse_status(body).unwrap_err().to_string();
    assert!(e.contains("500"), "{e}");
    assert!(e.contains("服务异常"), "{e}");
}

#[test]
fn string_code_zero_is_accepted() {
    let body = r#"{"code":"0000","result":{"total":1,"cal":[]}}"#;
    assert!(parse_status(body).is_ok());
}

#[test]
fn non_json_is_an_error_not_a_panic() {
    assert!(parse_status("<html>502 Bad Gateway</html>").is_err());
}

/// 会话失效要**说清楚是票的问题**，别混进"任务失败"。
#[test]
fn session_expiry_error_mentions_the_ticket() {
    let body = r#"{"code":90001,"msg":"未登录"}"#;
    let e = parse_status(body).unwrap_err().to_string();
    assert!(e.contains("会话"), "应说清是会话问题：{e}");
    assert!(e.contains("票"), "应引向票而不是账号：{e}");
}

// ── 百分号编码 ──────────────────────────────────────────────────────────

/// ⭐ `deviceId` 是 base64，`+` 在查询串里表示空格。
///
/// 不编码就会被服务端解成另一个值，而且**看起来没报错**，只是行为不对。
/// 抓包里三个字符正是 `%2B` / `%2F` / `%3D`。
#[test]
fn percent_encoding_escapes_base64_specials() {
    assert_eq!(percent_encode("a+b/c="), "a%2Bb%2Fc%3D");
    assert_eq!(percent_encode("AAECAwQ="), "AAECAwQ%3D");
}

#[test]
fn percent_encoding_keeps_unreserved_characters() {
    // 抓包里字母数字都没被编码 —— 编码它们虽然也能过，但就不是抓包的形状了
    assert_eq!(percent_encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
}

#[test]
fn percent_encoding_is_uppercase_hex() {
    // RFC 3986 与抓包都用大写
    let e = percent_encode(" ");
    assert_eq!(e, "%20");
}

// ── 端点 ────────────────────────────────────────────────────────────────

/// 端点必须是 `/ycloud/`，不是旧 TS 的 `/market/`。
#[test]
fn endpoints_use_the_captured_ycloud_prefix() {
    assert!(INFO_V3.contains("/ycloud/signin/page/infoV3"), "{INFO_V3}");
    assert!(
        START_SIGN_IN.contains("/ycloud/signin/page/startSignIn"),
        "{START_SIGN_IN}"
    );
    assert!(!INFO_V3.contains("/market/"), "旧前缀是错的：{INFO_V3}");
    assert!(
        !START_SIGN_IN.contains("/market/"),
        "旧前缀是错的：{START_SIGN_IN}"
    );
}
