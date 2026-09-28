//! 趣玩AI 协议层的测试。
//!
//! 钉的都是"改错了也不报错"的那些：多一个查询参数、少一个键的 body、
//! 把 `null` 当 0、把无时区串按 UTC 解 —— 每一条都只会让某个判据悄悄失效。

use serde_json::json;

use super::*;

/// 实拍那份 `page/getmodule` 的 `result`，逐字（顺序 11 那一带的读数）。
const CAPTURED_MODULE: &str = r#"{"code":0,"msg":"success","result":{
    "moduleId":"1-250730","drawTimes":1,"drawsUsed":0,"todayGetLotteryCount":1,
    "startTime":"2025-05-28T00:00:00","endTime":"2026-12-31T23:59:59"}}"#;

/// 未中奖那一档的第二份实拍（`code:0` + `isWin:0` + `prize*` 全 `null`），逐字。
const CAPTURED_LOSE: &str = r#"{"code":0,"msg":"success","result":{"id":10657867,"type":"1",
    "moduleId":"1-250730","lotteryDate":"20260925","isWin":0,"prizeId":null,"prizeName":null,
    "prizeUrl":null,"prizeType":1,"lotteryTime":"2026-09-25T20:21:43.238687928",
    "marketName":"National_playAI","source":"0"}}"#;

/// 中奖那一档（归档顺序 11），逐字。
const CAPTURED_WIN: &str = r#"{"code":0,"msg":"success","result":{"id":3228298,"isWin":1,
    "lotteryDate":"20260629","lotteryTime":"2026-06-29T20:13:47.890625379",
    "marketName":"National_playAI","moduleId":"1-250730","prizeId":260107009,
    "prizeName":"30云朵","prizeType":2,"prizeUrl":"https://x/a.png","type":"1"}}"#;

fn envelope(body: &str) -> SignInEnvelope {
    SignInEnvelope::parse(body).expect("信封应可解析")
}

fn header<'a>(req: &'a HttpRequest, key: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

// ══════════════════════════ 请求形状 ══════════════════════════

/// `getmodule` 是**裸 GET**：没有查询串、没有 body、没有 `isdeviceid`、没有 `content-type`。
#[test]
fn module_request_is_a_bare_get() {
    let req = funai_module_request(None);
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/funai/api/page/getmodule"
    );
    assert_eq!(req.method, "GET");
    assert!(!req.url.contains('?'), "实拍没有查询串：{}", req.url);
    assert!(req.body.is_none(), "实拍请求体是空的");
    assert!(
        header(&req, "content-type").is_none(),
        "裸 GET 不该带 content-type（带 body 时才由客户端补）"
    );
    assert!(
        header(&req, "isdeviceid").is_none(),
        "实拍这一条没有 isdeviceid"
    );
}

/// `deviceid` 是**可选**的：给了才发（理由见文件头第三条）。
#[test]
fn device_id_is_optional_on_both_requests() {
    assert!(header(&funai_module_request(None), "deviceid").is_none());
    assert_eq!(
        header(&funai_module_request(Some("B-abc")), "deviceid"),
        Some("B-abc")
    );
    assert!(header(&funai_lottery_request("1-250730", None), "deviceid").is_none());
    assert_eq!(
        header(
            &funai_lottery_request("1-250730", Some("B-abc")),
            "deviceid"
        ),
        Some("B-abc")
    );
}

/// `lottery` 的 body 只有 `moduleId` **一个键**，且**恰好 23 字节**（实拍 `content-length: 23`）。
#[test]
fn lottery_body_is_exactly_the_captured_one() {
    let req = funai_lottery_request("1-250730", None);
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/funai/api/page/lottery"
    );
    let body = req.body_str().expect("这一枪要带 body");
    assert_eq!(body, r#"{"moduleId":"1-250730"}"#);
    assert_eq!(body.len(), 23, "实拍 content-length 是 23");
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v, json!({"moduleId": "1-250730"}));
    assert_eq!(
        v.as_object().map(|o| o.len()),
        Some(1),
        "不许自作主张加 marketName / client 之类的键"
    );
}

/// 模块 id 是**参数**，不是写死的 —— 换期时上游会给另一个 id。
#[test]
fn the_module_id_flows_through_into_the_body() {
    let req = funai_lottery_request("2-260324-1", None);
    assert_eq!(req.body_str(), Some(r#"{"moduleId":"2-260324-1"}"#));
}

/// 两条请求共用那四片 H5 头（`jwttoken` / `referer` 由会话层注入，不在这里）。
#[test]
fn both_requests_carry_the_shared_h5_headers() {
    for req in [
        funai_module_request(None),
        funai_lottery_request("1-250730", None),
    ] {
        assert_eq!(header(&req, "Accept"), Some("*/*"));
        assert_eq!(header(&req, "activityid"), Some("sign_in_3"));
        assert_eq!(header(&req, "appversion"), Some("12.6.0.0"));
        assert_eq!(header(&req, "showloading"), Some("true"));
        assert!(header(&req, "jwttoken").is_none(), "由 H5Client 注入");
    }
}

// ══════════════════════════ 解析 getmodule ══════════════════════════

/// 实拍那一份要逐字解出来，含按**东八区**解的两个活动期。
#[test]
fn parses_the_captured_module_verbatim() {
    let m = parse_funai_module(&envelope(CAPTURED_MODULE)).unwrap();
    assert_eq!(m.module_id, "1-250730");
    assert_eq!(m.draw_times, Some(1));
    assert_eq!(m.draws_used, Some(0));
    assert_eq!(m.today_get_lottery_count, Some(1));
    assert_eq!(m.start_time, Some(1_748_361_600_000));
    assert_eq!(m.end_time, Some(1_798_732_799_000));
}

/// ⚠⚠ 活动期按**东八区**解，不是 UTC。
///
/// 按 UTC 解会把边界整体**推后 8 小时**：快到 `endTime` 时多发一次写请求，
/// 快到 `startTime` 时少发一次。这条把差值**逐字**钉住。
#[test]
fn activity_times_are_interpreted_as_beijing_not_utc() {
    let m = parse_funai_module(&envelope(CAPTURED_MODULE)).unwrap();
    let start = m.start_time.unwrap();
    // 2025-05-28T00:00:00 东八区 == 2025-05-27T16:00:00Z
    assert_eq!(start, 1_748_361_600_000);
    // 若按 UTC 解会得到 1_748_390_400_000（多 8 小时）
    assert_ne!(start, 1_748_390_400_000, "按 UTC 解会让边界整体推后 8 小时");
    assert_eq!(1_748_390_400_000 - start, 8 * 3_600_000);
    // 与 mcloud-core 那份"东八区"的唯一定义对拍
    assert_eq!(
        start,
        parse_beijing_time("2025-05-28T00:00:00", FUNAI_ACTIVITY_TIME_LAYOUT).unwrap()
    );
}

/// 时间**形态不认识** ⇒ `None`，**不报错**。
///
/// 报错会把一次**能抽**的机会变成解析失败；而"上游换了格式"该表现为
/// "判不了 ⇒ 不发请求"（那句结论人能看懂）。
#[test]
fn an_unrecognised_time_shape_is_none_not_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{
        "moduleId":"1-250730","startTime":"2026/01/01 00:00:00","endTime":1735689600}}"#;
    let m = parse_funai_module(&envelope(body)).unwrap();
    assert_eq!(m.start_time, None, "斜杠形态不认识");
    assert_eq!(m.end_time, None, "epoch 数字也算形态不认识");
}

/// `moduleId` 缺失 ⇒ **报错**（它缺了就发不出抽奖请求）。
#[test]
fn a_missing_module_id_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"drawTimes":1}}"#;
    let err = parse_funai_module(&envelope(body)).expect_err("缺 moduleId 要报错");
    assert!(err.to_string().contains("moduleId"), "实得：{err}");
}

/// `moduleId` 是**空串**也报错 —— 带着空 id 发出去的请求**一定**是另一个请求。
#[test]
fn an_empty_module_id_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"moduleId":""}}"#;
    assert!(parse_funai_module(&envelope(body)).is_err());
}

/// 三个计数**全是 `null`** ⇒ 三格全 `None`（**不是 0**）。
///
/// 这正是 `timed/getmodule` 那一族的形状 —— 所以"判不了"那一档**不是假想分支**。
#[test]
fn null_counters_stay_none_and_never_become_zero() {
    let body = r#"{"code":0,"msg":"success","result":{"moduleId":"1-250730",
        "drawTimes":null,"drawsUsed":null,"todayGetLotteryCount":null}}"#;
    let m = parse_funai_module(&envelope(body)).unwrap();
    assert_eq!(m.draw_times, None);
    assert_eq!(m.draws_used, None);
    assert_eq!(m.today_get_lottery_count, None);
    assert_eq!(m.module_id, "1-250730", "id 有值就仍要给出来");
}

/// 外层 `code` 非 0 ⇒ 报错（这一枪没打上）。
#[test]
fn a_non_zero_code_on_getmodule_is_an_error() {
    let body = r#"{"code":500,"msg":"系统繁忙"}"#;
    assert!(parse_funai_module(&envelope(body)).is_err());
}

// ══════════════════════════ 解析 lottery ══════════════════════════

/// 中奖那一档。
#[test]
fn parses_the_captured_win() {
    let l = parse_funai_lottery(&envelope(CAPTURED_WIN)).unwrap();
    assert_eq!(l.prize_name.as_deref(), Some("30云朵"));
    assert_eq!(l.prize_id, Some(260_107_009));
    assert_eq!(l.prize_type, Some(2));
    assert_eq!(l.is_win, Some(1));
    assert_eq!(l.module_id.as_deref(), Some("1-250730"));
    assert_eq!(l.market_name.as_deref(), Some(FUNAI_MARKET_NAME));
}

/// ⚠ **未中奖也是 `code:0`** —— 它是**成功**不是失败。
///
/// 而且这一档 `prizeType` **仍有值**（`1`）⇒ 它**不是**"有没有奖品"的信号；
/// 判"中没中"只能看 `is_win`。
#[test]
fn parses_the_captured_lose_as_a_success() {
    let l = parse_funai_lottery(&envelope(CAPTURED_LOSE)).unwrap();
    assert_eq!(l.is_win, Some(0), "没中奖读数是 0");
    assert_eq!(l.prize_name, None, "没中奖时 prizeName 是 null");
    assert_eq!(l.prize_id, None);
    assert_eq!(
        l.prize_type,
        Some(1),
        "⚠ 没中奖也带 prizeType —— 它不能当「有没有奖品」的信号"
    );
}

/// 外层 `code` 非 0 ⇒ 报错（本口零样本，所以不猜码值，一律按失败交回）。
#[test]
fn a_non_zero_code_on_lottery_is_an_error() {
    let body = r#"{"code":500,"msg":"系统繁忙"}"#;
    assert!(parse_funai_lottery(&envelope(body)).is_err());
}

/// 没有 `result` **不判失败** —— 成败已由 `code` 说过一次。
#[test]
fn a_missing_lottery_result_is_not_an_error() {
    let body = r#"{"code":0,"msg":"success"}"#;
    let l = parse_funai_lottery(&envelope(body)).unwrap();
    assert_eq!(l, FunaiLottery::default());
    assert_eq!(
        l.prize_name, None,
        "别把它念成「没中」—— 判中没中要看 isWin"
    );
}

/// 布局常量就是实拍那个形态（改它等于让所有时间判据静默失效）。
#[test]
fn the_layout_constant_is_the_captured_shape() {
    assert_eq!(FUNAI_ACTIVITY_TIME_LAYOUT, "%Y-%m-%dT%H:%M:%S");
    // 实拍那两个串都能解出来
    assert!(parse_beijing_time("2025-05-28T00:00:00", FUNAI_ACTIVITY_TIME_LAYOUT).is_some());
    assert!(parse_beijing_time("2026-12-31T23:59:59", FUNAI_ACTIVITY_TIME_LAYOUT).is_some());
    // 而"时分秒之间用 `-`"的那种（另一族的形态）解不出来 ⇒ 两族形态确实不同
    assert!(parse_beijing_time("2026-12-31 23-59-59", FUNAI_ACTIVITY_TIME_LAYOUT).is_none());
}

/// `parse_beijing_time` 对**非法历法**要回 `None`，不许被"规范化"成别的日期。
#[test]
fn impossible_dates_are_rejected() {
    for bad in [
        "2025-13-01T00:00:00", // 13 月
        "2025-02-30T00:00:00", // 2 月没有 30 日
        "2025-05-28T24:00:00", // 24 点
        "2025-05-28 00:00:00", // 分隔符不对
        "不是时间",
    ] {
        assert_eq!(
            parse_beijing_time(bad, FUNAI_ACTIVITY_TIME_LAYOUT),
            None,
            "「{bad}」不该被解出来"
        );
    }
}
