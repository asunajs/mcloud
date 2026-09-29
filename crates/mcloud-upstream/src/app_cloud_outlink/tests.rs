//! 外链族的测试。
//!
//! ⚠ 这一族**零实拍** —— 响应形状只有 TS 的类型声明，没有一个真实响应体。
//! 所以下面的夹具是**照类型声明写的**，它们钉的是**我们的判读**，不是上游的样子。

use super::*;

fn env(body: &str) -> OutLinkEnvelope {
    OutLinkEnvelope::parse(body).expect("夹具应是合法 JSON")
}

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// ⚠ **空文件表 ⇒ 一个字节都不发**。
///
/// 发一条没有内容的外链等于拿用户的账号建一个指向空集的东西 ——
/// 而那是一次**真的写**（会多一条外链记录）。
#[test]
fn an_empty_file_list_is_refused_before_any_request() {
    let e = get_out_link_request("13800000000", &[], "云盘分享").expect_err("必须拒绝");
    assert!(e.to_string().contains("至少一个文件 id"), "实得：{e}");
}

/// ⚠ **空 linkID 表 ⇒ 一个字节都不发**（没有东西可删就别发写请求）。
#[test]
fn an_empty_link_list_is_refused_before_any_request() {
    let e = del_out_link_request("13800000000", &[]).expect_err("必须拒绝");
    assert!(e.to_string().contains("至少一个 linkID"), "实得：{e}");
}

/// URL 与主机逐字（**不是**网盘文件操作那一台）。
#[test]
fn the_urls_are_the_share_family_ones() {
    let get = get_out_link_request("13800000000", &["F1".into()], "n").unwrap();
    assert_eq!(get.method, "POST");
    assert!(
        get.url
            .ends_with("/orchestration/personalCloud-rebuild/outlink/v1.0/getOutLink"),
        "实得：{}",
        get.url
    );
    let del = del_out_link_request("13800000000", &["L1".into()]).unwrap();
    assert!(
        del.url
            .ends_with("/orchestration/personalCloud-rebuild/outlink/v1.0/delOutLink"),
        "实得：{}",
        del.url
    );
}

/// 这一族的头**照 TS 逐字**，尤其那串带竖线的设备串。
#[test]
fn the_headers_are_the_share_family_ones() {
    let req = get_out_link_request("13800000000", &["F1".into()], "n").unwrap();
    assert_eq!(header(&req, "caller"), Some("web"));
    assert_eq!(header(&req, "cms-device"), Some("default"));
    assert_eq!(header(&req, "mcloud-version"), Some("7.14.4"));
    assert_eq!(header(&req, "x-svctype"), Some("1"));
    assert_eq!(
        header(&req, "x-deviceinfo"),
        Some("||9|7.14.4|edge||||linux unknow||zh-CN|||"),
        "竖线数量也算内容"
    );
    // ⚠ 别把网盘文件操作那一族的头混进来
    assert_eq!(header(&req, "x-yun-op-type"), None, "那是文件操作族的头");
    assert_eq!(header(&req, "x-yun-market-source"), None, "那是安卓档的头");
}

/// body 逐字：`getOutLinkReq` 那几格照 TS。
#[test]
fn the_body_is_the_share_family_shape() {
    let req = get_out_link_request("13800000000", &["F1".into(), "F2".into()], "云盘分享").unwrap();
    let b: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
    let r = &b["getOutLinkReq"];
    assert_eq!(r["subLinkType"], 0);
    assert_eq!(r["encrypt"], 1);
    assert_eq!(r["coIDLst"], serde_json::json!(["F1", "F2"]));
    assert_eq!(r["caIDLst"], serde_json::json!([]));
    assert_eq!(r["pubType"], 1);
    assert_eq!(r["dedicatedName"], "云盘分享");
    assert_eq!(r["period"], 1);
    assert_eq!(r["periodUnit"], 1);
    assert_eq!(r["viewerLst"], serde_json::json!([]));
    assert_eq!(r["extInfo"]["isWatermark"], 0);
    assert_eq!(r["extInfo"]["shareChannel"], "3001");
    assert_eq!(r["commonAccountInfo"]["account"], "13800000000");
    assert_eq!(r["commonAccountInfo"]["accountType"], 1);
}

/// ⭐ 回成功但**没给 linkID** ⇒ 当失败。
///
/// 这一族最要紧的判据：外链是写出来的，"`code:0` 但没给 linkID"意味着
/// **有一条我们建了却删不掉的外链**。读成成功，它就永远留在用户的分享记录里。
#[test]
fn an_ok_envelope_without_a_link_id_is_a_failure() {
    let got = parse_get_out_link(&env(
        r#"{"code":"0","data":{"getOutLinkRes":{"getOutLinkResSet":[]}}}"#,
    ))
    .unwrap();
    assert!(got.link_ids.is_empty());
    let e = got.ensure_found().expect_err("必须当失败");
    assert!(e.to_string().contains("删不掉"), "实得：{e}");
}

/// 正常一发：拿到 linkID 与地址（`linkID` 是删除那一口的输入）。
#[test]
fn link_ids_are_taken_from_the_set() {
    let got = parse_get_out_link(&env(
        r#"{"code":"0","data":{"getOutLinkRes":{"getOutLinkResSet":[
            {"linkID":"L1","linkUrl":"https://caiyun.139.com/w/#/a1"},
            {"linkID":"L2","linkUrl":"https://caiyun.139.com/w/#/a2"}]}}}"#,
    ))
    .unwrap();
    assert_eq!(got.link_ids, vec!["L1", "L2"]);
    assert_eq!(got.urls.len(), 2);
    got.ensure_found().unwrap();
}

/// `code` 数字/字符串两种都收（TS 类型写 string，抓到过数字）。
#[test]
fn both_code_shapes_are_accepted() {
    assert!(env(r#"{"code":"0"}"#).is_ok());
    assert!(env(r#"{"code":0}"#).is_ok());
    assert!(!env(r#"{"code":"1"}"#).is_ok());
}

/// 非 0 码 ⇒ 报错，带上原码。
#[test]
fn a_non_zero_code_surfaces_the_code() {
    let e = parse_get_out_link(&env(r#"{"code":"1001","data":null}"#)).expect_err("必须报错");
    assert!(e.to_string().contains("1001"), "实得：{e}");
}

/// ⚠ 缺 `code` 这一格 ⇒ 报错**并把原文带出来**（这一族零实拍，
/// 我们不知道"成功"长什么样，宁可说得出来）。
#[test]
fn a_missing_code_is_reported_with_the_body() {
    let e = OutLinkEnvelope::parse(r#"{"message":"nope"}"#).expect_err("必须报错");
    let msg = e.to_string();
    assert!(msg.contains("没有 `code`"), "实得：{msg}");
    assert!(msg.contains("nope"), "原文要带上：{msg}");
}

/// 删除那一口：`linkIDs` 与账号信息逐字。
#[test]
fn the_delete_body_carries_the_link_ids() {
    let req = del_out_link_request("13800000000", &["L1".into(), "L2".into()]).unwrap();
    let b: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
    assert_eq!(
        b["delOutLinkReq"]["linkIDs"],
        serde_json::json!(["L1", "L2"])
    );
    assert_eq!(
        b["delOutLinkReq"]["commonAccountInfo"]["account"],
        "13800000000"
    );
    assert_eq!(b["delOutLinkReq"]["commonAccountInfo"]["accountType"], 1);
    assert_eq!(
        header(&req, "caller"),
        Some("web"),
        "删除那一口也走同一套头"
    );
    parse_del_out_link(&env(r#"{"code":"0","data":null}"#)).unwrap();
    assert!(parse_del_out_link(&env(r#"{"code":"9","data":null}"#)).is_err());
}
