//! 兑换协议层的测试。
//!
//! 请求/响应逐字来自真机抓包（`mcloud/docs/capture/app-exchange-samples.txt` §5.1~5.3）。
//!
//! 重点在三处**极易写错且不报警**的地方：
//! 1. 字段类型（数字 vs 字符串）；
//! 2. 价格有 `pOrder` / `POrder` **两种拼法**；
//! 3. `probability2` 里 `limitMonthQuantity` 是数字而 `monthGroupLimit` 是**字符串数组**。

use super::*;
use crate::app_signin::SignInEnvelope;

// ── 抓包原文 ────────────────────────────────────────────────────────────

/// §5.1 那条完整商品对象（28 个键里保留判据相关的那些，值逐字）。
const CAPTURED_PRIZE: &str = r#"{
  "POrder":50,"allowInside":0,"areaCode":"0","canEdit":0,"count":399291,
  "dailyCount":5000,"dailyRemainderCount":4998,"endTime":"2026-12-31 23-59-59",
  "groupId":0,"imageUrl":"https://img.mcloud.139.com/x.png",
  "marketName":"sign_in_3_ex",
  "memo":"{\"saleType\":103}",
  "oid":260630012,"onLine":1,"prizeId":260630012,
  "prizeName":"移动云盘100万tokens叠加包",
  "prizeSectionStatus":1,
  "probability2":"{\"multi\":true,\"limitMonthQuantity\":3}",
  "receiveQuantity":0,"sort":1,"startTime":"2026-07-01 09-30-00",
  "totalCount":400000,"type":1057,"updateTime":"2026-09-22 09-46-31",
  "limit":0,"minRemainderCount":4998,"pOrder":50,"nextTimeStatus":0
}"#;

/// §5.6 那条 group 2 的商品（`limitMonthQuantity: 1` + 数组形态的 `monthGroupLimit`）。
const CAPTURED_GROUP_PRIZE: &str = r##"{
  "prizeId":251230053,"prizeName":"网易云音乐VIP会员月卡","groupId":2,
  "type":1035,"pOrder":1800,"POrder":1800,"limit":2,"count":100,
  "totalCount":1000,"dailyCount":10,"dailyRemainderCount":49,
  "minRemainderCount":9,"onLine":1,"nextTimeStatus":0,"prizeSectionStatus":1,
  "startTime":"2026-09-01 00-00-00","endTime":"2026-09-30 23-59-59",
  "marketName":"sign_in_3_ex",
  "probability2":"{\"multi\":true,\"btnBg\":\"#e6c117\",\"limitMonthQuantity\":1,\"limitBeforeSendChinaMobile\":true,\"limitOfficial\":true,\"itemBg\":\"#f5f2e4\",\"monthGroupLimit\":[\"251230051\",\"251230052\",\"251230053\",\"251230057\"]}"
}"##;

/// §5.2 成功响应（记录 13071，逐字）。
const CAPTURED_SUCCESS: &str = r#"{"code":0,"msg":"success","result":{"oid":1272221744,"userDomainId":"x","prizeId":251230053,"prizeName":"网易云音乐VIP会员月卡","flag":1,"insertTime":"2026-09-22T04:16:25.453+00:00","linkUrl":"251230053","type":1035,"uorder":1800,"reasons":"251230053","memo":"","updateTime":"2026-09-22T04:16:25.453+00:00","marketName":"sign_in_3_ex","sourceId":"","orderType":0,"expireTime":"2026-09-30T15:59:59.453+00:00","source":0,"device":0,"belonger":0,"groupId":2,"prizeSectionStatus":1}}"#;

/// §5.2 失败响应（记录 13021/13047/13064，三条逐字相同）。
const CAPTURED_FAILURE: &str = r#"{"code":514,"msg":"验证失败"}"#;

fn env(body: &str) -> SignInEnvelope {
    SignInEnvelope::parse(body).unwrap()
}

fn list_of(prizes: &[&str]) -> ExchangeList {
    let body = format!(
        r#"{{"code":0,"msg":"success","result":{{"0":[{}]}}}}"#,
        prizes.join(",")
    );
    parse_exchange_list(&env(&body)).unwrap()
}

// ── 请求构造 ────────────────────────────────────────────────────────────

#[test]
fn exchange_list_url_has_exactly_two_query_params() {
    let req = exchange_list_request();
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/signin/page/exchangeList?client=app&clientVersion=12.6.0"
    );
    // 只读请求不带 deviceId（实拍如此）
    assert!(!req.url.contains("deviceId"), "清单不带 deviceId");
    assert!(req.body.is_none(), "GET 无 body");
}

#[test]
fn list_and_slide_share_the_h5_header_set() {
    for req in [exchange_list_request(), get_slide_request()] {
        let get = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("Accept"), Some("*/*"));
        assert_eq!(get("activityid"), Some("sign_in_3"));
        // ⚠ appversion 与 clientVersion **不是同一个值**（四段 vs 三段）
        assert_eq!(get("appversion"), Some("12.6.0.0"));
        assert_eq!(get("showloading"), Some("true"));
        // referer / jwttoken 由会话层注入，这里**不该**有
        assert!(get("Referer").is_none(), "referer 由 H5Client 注入");
        assert!(get("jwttoken").is_none(), "票由 H5Client 注入");
    }
}

#[test]
fn get_slide_body_is_exactly_two_bytes() {
    let req = get_slide_request();
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/auth-service/slide/getSlide"
    );
    assert_eq!(req.body_str(), Some("{}"), "实拍 Content-Length: 2");
}

#[test]
fn get_slide_uses_form_content_type_not_json() {
    // ⚠ 探针把这条发成 JSON **也照样回 code 0**，所以不一致这件事
    // 在探针那一轮完全没露头。按实拍构造。
    let req = get_slide_request();
    let cts: Vec<&str> = req
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(cts.len(), 1, "必须恰好一个 content-type：{cts:?}");
    assert_eq!(cts[0], "application/x-www-form-urlencoded;charset=UTF-8");
    // 不能带上 isdeviceid（那是别的接口的）
    assert!(!req
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("isdeviceid")));
}

#[test]
fn exchange_v3_body_matches_the_capture_byte_for_byte() {
    let req = exchange_v3_request("BDEVICE", 251230053, Some(144));
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/signin/page/exchangeV3"
    );
    // ⭐ 键序是契约：prizeId → client → clientVersion → puzzleOffset → smsCode → deviceId
    assert_eq!(
        req.body_str(),
        Some(
            r#"{"prizeId":251230053,"client":"app","clientVersion":"12.6.0","puzzleOffset":144,"smsCode":"","deviceId":"BDEVICE"}"#
        )
    );
}

#[test]
fn exchange_v3_body_length_matches_the_capture() {
    // 实拍 content-length: 196（offset 两位数时 195）。
    let len = |offset: i64, dev: &str| {
        exchange_v3_request(dev, 251230053, Some(offset))
            .body
            .unwrap()
            .len()
    };
    // 用实拍那个 89 字符的 deviceId 长度来核对总量
    let dev = "B".repeat(89);
    assert_eq!(len(144, &dev), 196, "offset 三位数");
    assert_eq!(len(99, &dev), 195, "offset 两位数");
}

#[test]
fn exchange_v3_null_offset_renders_json_null() {
    let req = exchange_v3_request("D", 1, None);
    assert!(
        req.body_str().unwrap().contains(r#""puzzleOffset":null"#),
        "None 要写成 JSON null（空串与 null 在上游那里逐字相同，都回 501）"
    );
}

#[test]
fn isdeviceid_is_per_request_not_blanket() {
    // ⚠ 实拍里 `exchangeV3` **有** `isdeviceid: true`，而 `getSlide`/`exchangeList`
    // **没有**。这是一条逐接口的入参，不是一刀切 —— 一刀切会让取题那条多带一个
    // 上游没预期的头（不报错，只是"另一个客户端"）。
    let has = |req: &HttpRequest| {
        req.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("isdeviceid"))
    };
    assert!(
        has(&exchange_v3_request("D", 1, Some(1))),
        "下单要带 isdeviceid"
    );
    assert!(!has(&get_slide_request()), "取题不带 isdeviceid");
    assert!(!has(&exchange_list_request()), "清单不带 isdeviceid");

    // `origin` 同样只在下单那条上（实拍如此）
    let origin = |req: &HttpRequest| {
        req.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("origin"))
    };
    assert!(origin(&exchange_v3_request("D", 1, Some(1))));
    assert!(!origin(&get_slide_request()));
    assert!(!origin(&exchange_list_request()));
}

#[test]
fn exchange_v3_carries_the_two_extra_headers() {
    let req = exchange_v3_request("D", 1, Some(1));
    let get = |name: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    // 这两个头少一个就不是"同一个客户端"了
    assert_eq!(get("isdeviceid"), Some("true"));
    assert_eq!(get("origin"), Some("https://m.mcloud.139.com"));
    assert_eq!(
        get("content-type"),
        None,
        "content-type 由 H5Client 补 json"
    );
}

// ── 清单解析 ────────────────────────────────────────────────────────────

#[test]
fn parses_the_captured_prize() {
    let list = list_of(&[CAPTURED_PRIZE]);
    assert_eq!(list.groups.len(), 1);
    let p = list.find(260630012).expect("按 prizeId 找得到");

    assert_eq!(p.name.as_deref(), Some("移动云盘100万tokens叠加包"));
    assert_eq!(p.kind, Some(1057));
    assert_eq!(p.limit, Some(0));
    assert_eq!(p.count, Some(399291));
    assert_eq!(p.daily_remainder_count, Some(4998));
    assert_eq!(p.min_remainder_count, Some(4998));
    assert_eq!(p.on_line, Some(1));
    assert_eq!(p.start_time.as_deref(), Some("2026-07-01 09-30-00"));
    assert_eq!(p.end_time.as_deref(), Some("2026-12-31 23-59-59"));
    assert_eq!(p.market_name.as_deref(), Some("sign_in_3_ex"));
    assert!(p.can_exchange_now(), "limit=0 且余量非 0 ⇒ 可兑");
}

#[test]
fn price_reads_both_spellings() {
    // 实拍里同一份响应**同时**有 `POrder` 与 `pOrder`。
    // 只读一个键会把一部分商品的价格判成"没给"，于是"余额够不够"恒假。
    let both = list_of(&[r#"{"prizeId":1,"pOrder":50,"POrder":99}"#]);
    assert_eq!(
        both.find(1).unwrap().price,
        Some(50),
        "两个键都在时**小写优先**（H5 的消费点是 t.pOrder）"
    );

    let only_upper = list_of(&[r#"{"prizeId":2,"POrder":77}"#]);
    assert_eq!(only_upper.find(2).unwrap().price, Some(77), "大写兜底");

    let neither = list_of(&[r#"{"prizeId":3}"#]);
    assert_eq!(
        neither.find(3).unwrap().price,
        None,
        "都没给就是 None，不猜 0"
    );
}

#[test]
fn probability2_parses_number_and_string_array_separately() {
    // ⚠⚠ 这是本模块最容易写错的一处：`limitMonthQuantity` 是**数字**、
    // `monthGroupLimit` 是**字符串数组**。按"都是数字"读会让后者静默归零。
    let list = list_of(&[(CAPTURED_GROUP_PRIZE)]);
    let p = list.find(251230053).unwrap();
    let q = p.quota.as_ref().expect("probability2 应被解析");

    assert_eq!(q.month_quantity, Some(1), "limitMonthQuantity 是数字");
    assert_eq!(
        q.group_prize_ids.as_deref(),
        Some(
            &[
                "251230051".to_string(),
                "251230052".to_string(),
                "251230053".to_string(),
                "251230057".to_string()
            ][..]
        ),
        "monthGroupLimit 是**字符串**数组（不是数字）"
    );
}

#[test]
fn absent_group_key_differs_from_empty_array() {
    // "没这个键"与"给了空数组"是**两种**答案。
    let absent = list_of(&[r#"{"prizeId":1,"probability2":"{\"limitMonthQuantity\":1}"}"#]);
    assert_eq!(
        absent
            .find(1)
            .unwrap()
            .quota
            .as_ref()
            .unwrap()
            .group_prize_ids,
        None
    );

    let empty = list_of(&[r#"{"prizeId":2,"probability2":"{\"monthGroupLimit\":[]}"}"#]);
    assert_eq!(
        empty
            .find(2)
            .unwrap()
            .quota
            .as_ref()
            .unwrap()
            .group_prize_ids,
        Some(vec![]),
        "空数组是 Some(vec![])，不是 None"
    );
}

#[test]
fn undecodable_probability2_is_an_error_not_silence() {
    // 这一层是 `limit==4` 的字段依据：把"解不出"读成"上游没给"
    // 会让"组名额用完了"看起来可兑。
    let body =
        r#"{"code":0,"msg":"success","result":{"0":[{"prizeId":1,"probability2":"{not json"}]}}"#;
    let e = parse_exchange_list(&env(body)).unwrap_err().to_string();
    assert!(e.contains("probability2"), "{e}");
}

#[test]
fn groups_are_sorted_numerically_not_lexically() {
    // 实拍键不连续（0,1,2,5,7,8,9,11,15），且 "10" 不能排在 "2" 前面。
    let body = r#"{"code":0,"msg":"success","result":{"15":[{"prizeId":15}],"2":[{"prizeId":2}],"10":[{"prizeId":10}],"0":[{"prizeId":0}]}}"#;
    let list = parse_exchange_list(&env(body)).unwrap();
    let keys: Vec<&str> = list.groups.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["0", "2", "10", "15"], "按数值升序，不是字典序");
    let ids: Vec<i64> = list.all().iter().map(|p| p.prize_id).collect();
    assert_eq!(ids, vec![0, 2, 10, 15]);
}

#[test]
fn result_must_be_an_object_grouped_by_group_id() {
    // 上游哪天改成数组要**立刻响** —— 顺手兼容会把形状变化藏起来，
    // 表现是"清单忽然空了但每一枪都成功"。
    let body = r#"{"code":0,"msg":"success","result":[{"prizeId":1}]}"#;
    let e = parse_exchange_list(&env(body)).unwrap_err().to_string();
    assert!(e.contains("不是按 groupId 分组"), "{e}");
}

#[test]
fn a_non_array_group_invalidates_the_whole_list() {
    let body = r#"{"code":0,"msg":"success","result":{"0":{"prizeId":1}}}"#;
    let e = parse_exchange_list(&env(body)).unwrap_err().to_string();
    assert!(e.contains("不是数组"), "{e}");
}

#[test]
fn non_zero_code_is_an_error_for_the_read_path() {
    // 只读查询没有"非 0 但仍是数据"的档。
    let body = r#"{"code":90001,"msg":"未登录"}"#;
    assert!(parse_exchange_list(&env(body)).is_err());
}

#[test]
fn missing_prize_id_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"0":[{"prizeName":"x"}]}}"#;
    assert!(
        parse_exchange_list(&env(body)).is_err(),
        "没有 prizeId 不能默认成 0"
    );
}

// ── 滑块 ────────────────────────────────────────────────────────────────

#[test]
fn parses_the_captured_slide_puzzle() {
    let b64 = mcloud_crypto::b64_encode(b"PNGDATA");
    let body = format!(
        r#"{{"code":0,"msg":"success","result":{{"picWidth":"680","picHeight":"400","picType":"png","puzzleWidth":"96","picture":"{b64}","puzzle":"{b64}","puzzleLeft":null}}}}"#
    );
    let p = parse_slide_puzzle(&env(&body)).unwrap();
    assert_eq!(p.picture_png, b"PNGDATA");
    assert_eq!(p.puzzle_png, b"PNGDATA");
    // ⚠ 三个尺寸在实拍里是**字符串**
    assert_eq!(p.pic_width, Some(680));
    assert_eq!(p.pic_height, Some(400));
    assert_eq!(p.puzzle_width, Some(96));
}

#[test]
fn slide_parser_ignores_puzzle_left() {
    // 实拍 `puzzleLeft` **恒为 null**（4 条一致）—— 服务端不给答案。
    // 解析器连字段都不读：读出一个永远为空的字段会让人以为"有值就能用"。
    let b64 = mcloud_crypto::b64_encode(b"X");
    let body = format!(
        r#"{{"code":0,"msg":"success","result":{{"picture":"{b64}","puzzle":"{b64}","puzzleLeft":"SHOULD-NOT-BE-USED"}}}}"#
    );
    let p = parse_slide_puzzle(&env(&body)).unwrap();
    // 结构体里根本没有这个字段 —— 能编译就说明没读它。
    assert_eq!(p.picture_png, b"X");
}

#[test]
fn slide_non_zero_code_is_an_error() {
    let body = r#"{"code":90001,"msg":"未登录"}"#;
    assert!(
        parse_slide_puzzle(&env(body)).is_err(),
        "取题失败没有可分流的档"
    );
}

#[test]
fn slide_requires_both_images() {
    let b64 = mcloud_crypto::b64_encode(b"X");
    let body = format!(r#"{{"code":0,"result":{{"picture":"{b64}"}}}}"#);
    let e = parse_slide_puzzle(&env(&body)).unwrap_err().to_string();
    assert!(e.contains("puzzle"), "缺 puzzle 要报错：{e}");
}

// ── 下单结果 ────────────────────────────────────────────────────────────

#[test]
fn parses_the_captured_success() {
    let r = parse_exchange_result(&env(CAPTURED_SUCCESS)).unwrap();
    assert!(r.is_ok());
    let o = r.order.expect("成功要有 order");
    assert_eq!(o.order_id, 1272221744);
    assert_eq!(o.prize_id, 251230053);
    assert_eq!(o.prize_name.as_deref(), Some("网易云音乐VIP会员月卡"));
    assert_eq!(o.cost, Some(1800), "uorder 是上游**说**扣了多少");
    assert_eq!(o.market_name.as_deref(), Some("sign_in_3_ex"));
    // ⚠ linkUrl 是 prizeId 的**字符串形态**，不是 URL
    assert_eq!(o.link_url.as_deref(), Some("251230053"));
    // 时间戳是 +00:00 形态（不是 Go 的 Z）—— 原样保留
    assert_eq!(
        o.insert_time.as_deref(),
        Some("2026-09-22T04:16:25.453+00:00")
    );
}

#[test]
fn failure_is_data_not_an_error() {
    // ⭐ 这条钉住"下线那条不走 ensure_ok"：`514` 是可分流的业务响应，
    // 转成 Err 会让调用方没法"重取滑块再来一次"。
    let r = parse_exchange_result(&env(CAPTURED_FAILURE)).unwrap();
    assert_eq!(r.code, CODE_SLIDER_FAILED);
    assert_eq!(r.message, "验证失败");
    assert!(r.order.is_none(), "失败没有订单");
}

#[test]
fn every_documented_code_round_trips() {
    // 码**不全都是三位**（`2301` 是四位）⇒ 别按形状解析。
    for (code, want) in [
        (501, CODE_SLIDER_PARAM_MISSING),
        (514, CODE_SLIDER_FAILED),
        (407, CODE_NOT_TARGET_USER),
        (415, CODE_PRIZE_EXHAUSTED_TODAY),
        (2301, CODE_DUPLICATED),
        (420, CODE_DAILY_QUOTA),
        (421, CODE_MONTHLY_QUOTA),
        (610, CODE_GENERIC),
        (614, CODE_BUSY_A),
        (615, CODE_BUSY_B),
    ] {
        let body = format!(r#"{{"code":{code},"msg":"m"}}"#);
        let r = parse_exchange_result(&env(&body)).unwrap();
        assert_eq!(r.code, want);
        assert!(!r.is_ok());
    }
}

#[test]
fn success_without_oid_is_an_error_not_a_fake_order() {
    // 成功码但缺 oid ⇒ 判不了成交。**不能**伪造一个订单号。
    for bad in [
        r#"{"code":0,"msg":"success","result":{"prizeId":1}}"#,
        r#"{"code":0,"msg":"success","result":{"oid":9}}"#,
        r#"{"code":0,"msg":"success"}"#,
    ] {
        assert!(
            parse_exchange_result(&env(bad)).is_err(),
            "缺关键字段必须报错：{bad}"
        );
    }
}

#[test]
fn success_without_prize_name_keeps_it_none() {
    // 参考实现把"code==0 但缺 prizeName"记为失败。这里**不补空串** ——
    // 补了就把那一档抹平了（调用方仍能看出它是 None）。
    let body = r#"{"code":0,"msg":"success","result":{"oid":1272221744,"prizeId":251230053}}"#;
    let r = parse_exchange_result(&env(body)).unwrap();
    assert!(r.is_ok());
    assert_eq!(r.order.unwrap().prize_name, None);
}

#[test]
fn missing_code_is_an_error() {
    // 没有 code 就判不了成交没成交，而这一步花的是真豆。
    assert!(parse_exchange_result(&env(r#"{"msg":"x"}"#)).is_err());
}

#[test]
fn message_accepts_both_msg_and_message_keys() {
    let a = parse_exchange_result(&env(r#"{"code":514,"msg":"来自 msg"}"#)).unwrap();
    assert_eq!(a.message, "来自 msg");
    let b = parse_exchange_result(&env(r#"{"code":514,"message":"来自 message"}"#)).unwrap();
    assert_eq!(b.message, "来自 message");
}

// ── 组字典（`prizeGroup.json`）──────────────────────────────────────────────

/// 组字典的抓包原文（2026-10-01 20:32，逐字，3,043 B）。
///
/// 来源：`../mcloud-capture/docs/capture/app-exchange-samples.txt` —— 那个仓建仓后的
/// **第一份真样本**。⚠ 这一份里 `groupId` **类型混用**（`"0"`/`"3"` 是字符串、
/// 其余是数字），是实拍形态，别"顺手"统一。
const CAPTURED_PRIZE_GROUPS: &str = r#"[{"title":"限时抢兑","subTitle":"","prizeType":1056,"list":[],"icon":"https://uop.mcloud.139.com/uop/gray/advert_media/1/20220829/ccad73a5eeb040e89740937c5fbe0e27.png","groupId":4,"media":true},{"title":"西藏用户专属","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250808/8d54e06589fb424ba735e35f9386d3ca.png","groupId":12,"media":true},{"title":"限时福利","subTitle":"","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250808/8d54e06589fb424ba735e35f9386d3ca.png","groupId":15,"media":true},{"title":"视频类会员","subTitle":"(月卡每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20260409/05be1db48bdc41b4973cf15f02e448d3.png","groupId":1,"media":true},{"title":"音乐类会员","subTitle":"(月卡每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20260409/1d28d4b2ece643888b13914522a13699.png","groupId":2,"media":true},{"title":"游戏礼包权益","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20251014/d32f836c6bae4466b75d85f45b6ee75f.png","groupId":14,"media":true},{"title":"美食饮品权益","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250714/325a283da91c4eea9a2fb0df83852f6d.png","groupId":11,"media":true},{"title":"咖啡饮品权益","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20251014/46eecd5b934c4f53a472e760d5f9fcb1.png","groupId":13,"media":true},{"title":"交通出行类","subTitle":"（每月多选一限兑）","prizeType":1035,"list":[],"icon":"https://uop.mcloud.139.com/uop/prod/advert_media/1/20221108/05e2fd21b6c941bcbadabc5929cf0240.png","groupId":6,"media":true},{"title":"外卖美食权益","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250808/2c1f2973f8094ba4b2b02079139b11c7.png","groupId":5,"media":true},{"title":"快递寄件券","subTitle":"（每月多选一限兑）","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250808/c8955e0a625d4ebba8498f57fe44850f.png","groupId":7,"media":true},{"title":"云盘转存券","subTitle":"","prizeType":1058,"list":[],"icon":"https://img.mcloud.139.com//uop_gray/20240111/3f42cddb4c8a4f669efe05fa8ba2fe77.jpg","groupId":8,"media":true},{"title":"实用工具类","subTitle":"","prizeType":1035,"list":[],"icon":"https://img.mcloud.139.com//uop_prod/20250808/c2e9d52162ed47cdb9228431765dd373.png","groupId":9,"media":true},{"title":"山西专属流量奖品","subTitle":"(每月多选一限兑)","prizeType":1035,"list":[],"icon":"https://om.mrp.139.com/img/signin/plus.png","groupId":"3","media":true},{"title":"其他权益奖品","subTitle":"","list":[],"icon":"https://img.mcloud.139.com//uop_prod/20260409/cd84351237e84638b2b43aa763d5ed3f.png","groupId":"0","media":false}]"#;

#[test]
fn prize_group_folder_drops_the_ex_suffix() {
    // 实拍这一对：清单里的 `marketName` → H5 静态目录名。
    assert_eq!(
        prize_group_folder("sign_in_3_ex").as_deref(),
        Some("sign_in_3")
    );
    // 没有 `_ex` 后缀时**原样用**（不猜、不硬编）。
    assert_eq!(
        prize_group_folder("sign_in_3").as_deref(),
        Some("sign_in_3")
    );
    // 推不出来 ⇒ `None`：调用方据此**不发那一枪**，而不是发一个 404 当"没组名"。
    assert_eq!(prize_group_folder(""), None);
    assert_eq!(prize_group_folder("   "), None);
    assert_eq!(prize_group_folder("_ex"), None);
    // 连请求也是 `None`，不进网络层。
    assert!(prize_group_request("").is_none());
    assert!(prize_group_request("sign_in_3_ex").is_some());
}

#[test]
fn parse_prize_groups_reads_the_captured_dictionary() {
    let groups = parse_prize_groups(CAPTURED_PRIZE_GROUPS).unwrap();
    assert_eq!(groups.len(), 15);
    let by_id = |id: &str| groups.iter().find(|g| g.group_id == id).unwrap();
    // 组名就是上游原话 —— 界面上直接念，别自己编"音乐类"这种名字。
    assert_eq!(by_id("2").title.as_deref(), Some("音乐类会员"));
    assert_eq!(
        by_id("2").sub_title.as_deref(),
        Some("(月卡每月多选一限兑)")
    );
    // ⚠ 括号**全角半角混用**是上游的：组 6/7 是全角 `（…）`、组 2/13 是半角 `(…)`。
    // 别"顺手规范化" —— 那是它给用户看的文案。
    assert_eq!(by_id("6").sub_title.as_deref(), Some("（每月多选一限兑）"));
    assert_eq!(by_id("13").sub_title.as_deref(), Some("(每月多选一限兑)"));
    // `subTitle: ""` 归 `None`（界面上"没有规则"与"规则是空串"没区别）。
    assert_eq!(by_id("9").title.as_deref(), Some("实用工具类"));
    assert_eq!(by_id("9").sub_title, None);
    // 类型混用归一：源里 `"0"` 是字符串、`1` 是数字，两边都要按同一个键查得到
    // （清单那边的组键是**字符串**，两边得能对上）。
    assert!(groups.iter().any(|g| g.group_id == "0"));
    assert!(groups.iter().any(|g| g.group_id == "1"));
    // `icon` 取到了（界面可以给分区加图标）；`prizeType` 也留着。
    assert!(by_id("2").icon.is_some());
    assert_eq!(by_id("2").prize_type, Some(1035));
}

#[test]
fn an_empty_body_is_an_error_not_an_empty_dictionary() {
    // ⚠ 实拍：同族路径换成 `sign_in_3_ex`，上游回 **200 + 0 字节**。
    // 把空体读成"上游说没有组"会让界面组名全空**且不报警**，所以这里必须响。
    for body in ["", "   ", "\n"] {
        assert!(parse_prize_groups(body).is_err(), "{body:?}");
    }
}

#[test]
fn non_array_and_missing_group_id_are_errors() {
    // 顶层不是数组（实拍是数组）。
    assert!(parse_prize_groups("{}").is_err());
    // 少组键的那一组对界面没用，而且是实拍里没有的形态 ⇒ 响，别静默跳过。
    assert!(parse_prize_groups(r#"[{"title":"没有组键"}]"#).is_err());
}
