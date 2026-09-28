//! 签到族协议测试。
//!
//! 判据来源优先级（本项目一贯口径）：
//! **实拍黄金向量 > H5 源码逐字 > 旧 TS**。
//! 本文件的响应全部取自 `test/golden/app-signin-vectors.json`
//! （2026-09-19 真账号抓包），不是编的。

use super::*;

fn vectors() -> Value {
    // 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是
    // crate 根，相对路径会让这条测试在不同调用方式下时灵时不灵。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/app-signin-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

/// 把向量里那段 `raw`（本来就是对象）序列化成响应文本。
fn raw_body(v: &Value, key: &str) -> String {
    serde_json::to_string(&v[key]["raw"]).expect("向量应可序列化")
}

// ══════════════════════════ 请求形状 ══════════════════════════

/// 百分比编码：`+` 必须变 `%2B`。
///
/// `deviceId` 是 base64，含 `+` `/` `=`。**`+` 在查询串里表示空格** ——
/// 不编码就会被服务端解成另一个值，而且看起来"没报错"，只是行为不对。
#[test]
fn percent_encoding_covers_base64_characters() {
    assert_eq!(percent_encode("a+b/c="), "a%2Bb%2Fc%3D");
    // 字母数字与 `-_.~` 原样保留（与抓包一致）
    assert_eq!(percent_encode("Az09-_.~"), "Az09-_.~");
    // 空格、中文都要编码
    assert_eq!(percent_encode(" "), "%20");
    assert!(!percent_encode("中文").contains('中'));
}

/// 这一族共用的头必须带齐。
#[test]
fn requests_carry_the_h5_header_set() {
    let req = info_v3_request(None);
    let get = |k: &str| {
        req.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("缺头 {k}"))
    };
    assert_eq!(get("activityid"), MARKET_NAME);
    assert_eq!(get("appversion"), H5_APP_VERSION);
    assert_eq!(get("showloading"), "true");
    assert_eq!(get("Accept"), "*/*");
}

/// ⭐ 构造器**不许**自己带 `jwttoken` / `Referer` / `authorization`。
///
/// # 为什么这条值得钉
///
/// 那两个头是**会话的两半**，由 `H5Client` 从同一次建会话里取。
/// 让任务构造器各拼一遍的后果是"票是对的、referer 差一个参数"，
/// 而表现是 **90001 未登录** —— 那个码通常被读成"票过期了"，
/// 于是排查方向整个跑偏。
///
/// 而且 `H5Client` 会**丢掉**调用方给的 referer、用会话那一条 ——
/// 所以传了也不生效，只是让人以为生效了。
#[test]
fn builders_do_not_set_session_bound_headers() {
    for req in [
        info_v3_request(None),
        start_sign_in_request(None),
        multiple_request(),
        receive_v3_request("D", "CID", None),
        receive_revival_reward_request(None),
        receive_task_expansion_request(None),
        prize_record_request(MARKET_NAME, 1, 1, 10),
        cloud_record_v2_request(1, 0, 1, 100),
    ] {
        for bound in ["jwttoken", "referer", "authorization"] {
            assert!(
                !req.headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case(bound)),
                "{bound} 该由 H5Client 注入，构造器不该带：{}",
                req.url
            );
        }
    }
}

/// `appversion` 与原生 App 的版本**不是一个值**（多第 4 段）。
#[test]
fn h5_app_version_differs_from_native() {
    assert_eq!(H5_APP_VERSION, "12.6.0.0");
    assert_ne!(H5_APP_VERSION, "12.6.0", "别把它与原生版本混成一个常量");
}

/// `infoV3` / `startSignIn` 的端点前缀是 `/ycloud/`，不是旧 TS 的 `/market/`。
#[test]
fn endpoints_use_the_measured_prefix() {
    for url in [
        INFO_V3,
        START_SIGN_IN,
        RECEIVE_V3,
        RECEIVE_REVIVAL_REWARD,
        MULTIPLE,
        RECEIVE_TASK_EXPANSION,
        PRIZE_RECORD,
        CLOUD_RECORD_V2,
    ] {
        assert!(
            url.starts_with("https://m.mcloud.139.com/ycloud/"),
            "端点前缀不对（旧 TS 写的是 /market/）：{url}"
        );
    }
}

/// `deviceId` 只在**调用方给了**的时候才拼进查询串。
///
/// ⚠ 这条防的是"永远拼一个空的 `deviceId=`"—— 那是**另一个请求**。
#[test]
fn device_id_is_only_appended_when_present() {
    assert!(!info_v3_request(None).url.contains("deviceId"));
    let with = info_v3_request(Some("a+b"));
    assert!(with.url.contains("deviceId=a%2Bb"), "实得：{}", with.url);
}

/// ⭐ `receiveV3` 的 `deviceId` 是**必填**（不是 `Option`）—— 漏发编译不过。
#[test]
fn receive_v3_requires_device_id_and_sends_it_in_the_body() {
    let req = receive_v3_request("DEV+1", "CID", Some(0));
    let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();

    assert_eq!(body["deviceId"], "DEV+1");
    assert_eq!(body["cloudId"], "CID");
    assert_eq!(body["cloudType"], 0);
    assert_eq!(body["client"], "app");
    // 这一族里唯一标了 isDeviceId 的接口
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "isdeviceid" && v == "true"));
}

/// ⭐ `cloudType` 缺省是**空串**，不是 `0`。
///
/// H5 逐字是 `e.cloudType ?? ""`。`0` 是**合法取值**（当前可领取），
/// "顺手改成 0"会让"没给"与"给 0"变成同一个请求。
#[test]
fn receive_v3_defaults_cloud_type_to_empty_string_not_zero() {
    let req = receive_v3_request("D", "CID", None);
    let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["cloudType"], "", "缺省必须是空串");
    assert_ne!(body["cloudType"], 0, "0 是合法取值，不能拿来当缺省");
}

/// `receiveRevivalReward`：**空 body `{}`**（不是没有 body）、`showloading=false`、
/// 且**不许**带 `isdeviceid`。
#[test]
fn revival_request_shape_matches_the_capture() {
    let req = receive_revival_reward_request(None);
    assert_eq!(req.body.as_deref(), Some("{}"), "body 必须是空对象");
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "showloading" && v == "false"));
    assert!(
        !req.headers.iter().any(|(k, _)| k == "isdeviceid"),
        "这一条 H5 没标 isDeviceId，别照 receiveV3 抄"
    );
    assert!(!req.headers.iter().any(|(k, _)| k == "deviceid"));
}

/// 给了 `deviceId` 才补那个**头**（老抓包那一档）。
#[test]
fn revival_device_header_is_opt_in() {
    let req = receive_revival_reward_request(Some("DEV-1"));
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "deviceid" && v == "DEV-1"));
}

/// ⭐ `multiple` / `receiveTaskExpansion` 发的是**裸 GET**，不是 `?`。
///
/// H5 传的是空对象，axios 对 `undefined` 是整键丢掉 ⇒ 真实客户端一个
/// 查询参数都不带。写成 `?acceptDate=` 是**另一个请求**。
#[test]
fn bare_get_has_no_query_string() {
    assert_eq!(multiple_request().url, MULTIPLE);
    assert!(!multiple_request().url.ends_with('?'));

    assert_eq!(
        receive_task_expansion_request(None).url,
        RECEIVE_TASK_EXPANSION,
        "None 必须是裸 GET"
    );
    // 给了才拼，且要编码
    let with = receive_task_expansion_request(Some("2026-09-30"));
    assert!(with.url.contains("acceptDate=2026-09-30"));
    let encoded = receive_task_expansion_request(Some("a+b"));
    assert!(
        encoded.url.contains("acceptDate=a%2Bb"),
        "实得：{}",
        encoded.url
    );
}

/// 奖品记录与流水的查询串形状。
#[test]
fn query_endpoints_build_expected_query_strings() {
    let p = prize_record_request("sign_in_3_ex", 1, 1, 10);
    assert!(p.url.contains("marketname=sign_in_3_ex"), "实得：{}", p.url);
    assert!(p.url.contains("type=1&pageNumber=1&pageSize=10"));

    let c = cloud_record_v2_request(1, 0, 1, 100);
    assert!(c.url.contains("aiDou=1"), "实得：{}", c.url);
    assert!(c.url.contains("type=0&pageNumber=1&pageSize=100"));
}

/// 市场名里的特殊字符要编码（不能直接拼）。
#[test]
fn prize_market_name_is_encoded() {
    let p = prize_record_request("a b&c", 1, 1, 10);
    assert!(p.url.contains("marketname=a%20b%26c"), "实得：{}", p.url);
}

/// `referer` 由 `session` 那一份提供（含 `targetSourceId`）。
#[test]
fn referer_comes_from_the_session_recipe() {
    let r = signin_referer("SSO-TOKEN");
    assert!(r.contains("token=SSO-TOKEN"), "实得：{r}");
    assert!(
        r.contains("targetSourceId=001005"),
        "漏了 targetSourceId 会回 90001（参考实现踩过）：{r}"
    );
    assert!(r.contains("sourceid=1002"));
    assert!(r.contains("path=newsignin"));
}

// ══════════════════════════ 实拍响应解析 ══════════════════════════

/// ⭐ 用**真账号抓包**逐字段对拍 `infoV3`。
#[test]
fn parses_the_real_info_v3_capture() {
    let v = vectors();
    let env = SignInEnvelope::parse(&raw_body(&v, "infoV3")).unwrap();
    assert_eq!(env.code, 0);
    assert_eq!(env.message, "success");

    let st = parse_status(&env).unwrap();
    // 这些数字全都来自实拍，不是编的
    assert_eq!(st.total, 10496, "total 才是现行余额（不是 beforeTotal）");
    assert_eq!(st.sign_count, 1);
    assert_eq!(st.month_days, 7);
    assert_eq!(st.sign_in_points, 3);
    assert_eq!(st.cur_month_backup, Some(true));
    assert_eq!(st.cur_month_backup_sign_accept, Some(true));

    // 日历：35 格 —— 上月补位 + 本月 + 下月补位
    assert_eq!(st.days.len(), 35);
    let padding = st.days.iter().filter(|d| !d.current_month).count();
    assert_eq!(padding, 5, "两端各有补位格");
    let current = st.days.iter().filter(|d| d.current_month).count();
    assert_eq!(current, 30);
}

/// ⭐ `beforeTotal` 是**结转额快照**，不是余额 —— 别拿它当 total。
///
/// 曾把它当余额记进趋势图，结果是一条直线（它永远不变）。
#[test]
fn total_is_not_before_total() {
    let v = vectors();
    let env = SignInEnvelope::parse(&raw_body(&v, "infoV3")).unwrap();
    let st = parse_status(&env).unwrap();
    let before = v["infoV3"]["raw"]["result"]["beforeTotal"]
        .as_i64()
        .unwrap();

    // ⭐ 实拍里这两个数**不相等** —— 这正是不能混用的证据
    assert_eq!(st.total, 10496, "total 是现行余额");
    assert_eq!(before, 8027, "beforeTotal 是改名那一刻的结转额");
    assert_ne!(st.total, before, "两个字段是两个数，混用会记出一条直线");

    // 再钉一次"读的是哪个键"：改掉 total，解析结果必须跟着变。
    let mut tampered = v["infoV3"]["raw"].clone();
    tampered["result"]["total"] = serde_json::json!(999);
    let env2 = SignInEnvelope::parse(&serde_json::to_string(&tampered).unwrap()).unwrap();
    assert_eq!(
        parse_status(&env2).unwrap().total,
        999,
        "total 必须读 total 键，而不是 beforeTotal"
    );
}

/// 今日签到判据只在**本月**的格子里找 `t`。
#[test]
fn signed_today_only_looks_at_current_month() {
    let mk = |days: Value| {
        let body = serde_json::json!({
            "code": 0, "msg": "success",
            "result": { "total": 1, "cal": days }
        });
        parse_status(&SignInEnvelope::parse(&body.to_string()).unwrap()).unwrap()
    };

    // 本月那格 t 且 s ⇒ 已签
    let st = mk(serde_json::json!([
        {"d":1,"s":true,"t":true,"currentMonth":1},
        {"d":31,"s":false,"t":false,"currentMonth":0}
    ]));
    assert!(st.signed_today());

    // 非本月那格带 t 且 s ⇒ **不算今天**
    let st = mk(serde_json::json!([
        {"d":31,"s":true,"t":true,"currentMonth":0}
    ]));
    assert!(!st.signed_today(), "上下月补位格的 t 不该被当成今天");

    // 本月那格 t 但没签 ⇒ 未签
    let st = mk(serde_json::json!([
        {"d":5,"s":false,"t":true,"currentMonth":1}
    ]));
    assert!(!st.signed_today());
}

/// 缺 `cal` 要报错（它是判据的来源），缺 `total` 也要报错（余额是我们要记的数）。
#[test]
fn missing_required_fields_are_rejected() {
    let no_cal = r#"{"code":0,"msg":"success","result":{"total":1}}"#;
    assert!(parse_status(&SignInEnvelope::parse(no_cal).unwrap()).is_err());

    let no_total = r#"{"code":0,"msg":"success","result":{"cal":[]}}"#;
    assert!(parse_status(&SignInEnvelope::parse(no_total).unwrap()).is_err());
}

/// `receiveList` 缺了**不报错** —— 它是附属字段，不该拖垮"今天签没签"。
#[test]
fn missing_receive_list_is_not_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"total":5,"cal":[]}}"#;
    let st = parse_status(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert!(st.receive_list.is_empty());
}

/// `receiveList` 的三种 `cloudType` 与可选字段。
#[test]
fn receive_items_carry_cloud_type_semantics() {
    let body = serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"total": 1, "cal": [], "receiveList": [
            {"cloudType": 0, "cloudNum": 6},
            {"cloudType": 1, "cloudNum": 20, "acceptDate": "2026-09-30"},
            {"cloudType": 2, "cloudNum": 240}
        ]}
    });
    let st = parse_status(&SignInEnvelope::parse(&body.to_string()).unwrap()).unwrap();
    assert_eq!(st.receive_list.len(), 3);
    assert_eq!(st.receive_list[0].cloud_type, cloud_type::NOW);
    assert_eq!(st.receive_list[0].cloud_num, 6);
    // 实测抓到的条目里**没有 recordId** ⇒ `None`，不替上游编默认值
    assert_eq!(st.receive_list[0].record_id, None);
    assert_eq!(st.receive_list[1].cloud_type, cloud_type::EXPANSION);
    assert_eq!(
        st.receive_list[1].accept_date.as_deref(),
        Some("2026-09-30")
    );
    assert_eq!(st.receive_list[2].cloud_type, cloud_type::NEXT_MONTH);
}

/// ⭐ `multiple` 的门槛：**两个条件都成立才去领**。
///
/// 每月只有一次机会，条件不成立还发是**白白消耗**。
#[test]
fn multiple_gate_requires_both_conditions() {
    let mk = |backup: bool, accept: bool| SignInStatus {
        total: 1,
        sign_in_points: 0,
        sign_count: 0,
        month_days: 0,
        days: vec![],
        receive_list: vec![],
        to_receive: None,
        cur_month_backup: Some(backup),
        cur_month_backup_sign_accept: Some(accept),
    };

    assert!(
        mk(true, false).should_claim_multiple(),
        "开备份且没用掉 ⇒ 领"
    );
    assert!(!mk(true, true).should_claim_multiple(), "已经用掉了 ⇒ 不领");
    assert!(!mk(false, false).should_claim_multiple(), "没开备份 ⇒ 不领");
    assert!(!mk(false, true).should_claim_multiple());

    // 上游没给这两个值时**也不领**（宁可少领一次，也不白耗机会）
    let mut unknown = mk(true, false);
    unknown.cur_month_backup = None;
    assert!(!unknown.should_claim_multiple(), "缺字段时不该领");
    unknown.cur_month_backup = Some(true);
    unknown.cur_month_backup_sign_accept = None;
    assert!(!unknown.should_claim_multiple(), "缺字段时不该领");
}

/// `receiveV3` 的实拍形状（`result.receive`）。
#[test]
fn parses_receive_v3() {
    let body = r#"{"code":0,"msg":"success","result":{"receive":6}}"#;
    let r = parse_receive_v3(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(r.receive, 6);

    // 缺 receive ⇒ 报错（这是我们唯一要读的数）
    let bad = r#"{"code":0,"msg":"success","result":{}}"#;
    assert!(parse_receive_v3(&SignInEnvelope::parse(bad).unwrap()).is_err());
}

/// ⭐ 用**真账号抓包**对拍 `receiveRevivalReward`。
#[test]
fn parses_the_real_revival_capture() {
    let v = vectors();
    let env = SignInEnvelope::parse(&raw_body(&v, "receiveRevivalReward")).unwrap();
    let r = parse_revival_reward(&env).unwrap();

    assert_eq!(r.card_type, 1, "1 = 复活卡（实拍）");
    assert_eq!(r.reward_clouds, 114);
    assert_eq!(r.total_clouds, 571);
    assert_eq!(r.little_first_time, 0);
}

/// `littleFirstTime` 是**附属字段**：缺了不判失败（它只换一句文案）。
#[test]
fn little_first_time_is_optional() {
    let body =
        r#"{"code":0,"msg":"success","result":{"cardType":2,"rewardClouds":5,"totalClouds":9}}"#;
    let r = parse_revival_reward(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(r.card_type, 2, "2 = 回馈卡");
    assert_eq!(r.little_first_time, 0, "缺了给 0，不报错");
}

/// ⭐ "本月已领取"是**正常的没得领**，不是失败。
///
/// 真账号实拍：`HTTP 200，{"code":500,"msg":"本月已领取复活卡奖励"}`。
/// 判成失败会让历史页每天一条红、并触发推送，而它下个月之前都不会好。
#[test]
fn already_claimed_is_recognised() {
    let body = r#"{"code":500,"msg":"本月已领取复活卡奖励"}"#;
    let env = SignInEnvelope::parse(body).unwrap();
    assert!(revival_already_claimed(&env), "实拍那条文案必须认出来");
    assert!(revival_message_already_claimed("本月已领取回馈卡"));
    assert!(revival_message_already_claimed("已领过奖励"));
}

/// ⚠ 认不出的一律**仍按失败处理**（宁可漏认，也不误判一片）。
#[test]
fn unrecognised_revival_messages_stay_failures() {
    // 裸一句"本月已领取"——没有卡型也没有"奖励"⇒ 刻意不在匹配面里
    assert!(!revival_message_already_claimed("本月已领取"));
    // 别的故障
    assert!(!revival_message_already_claimed("系统繁忙"));
    assert!(!revival_message_already_claimed(""));
    assert!(!revival_message_already_claimed("已领取"));
}

/// `multiple` 没有 `result` **不判失败**（缺了不影响"领没领到"这个结论）。
#[test]
fn multiple_without_result_is_still_ok() {
    let body = r#"{"code":0,"msg":"success"}"#;
    let r = parse_multiple(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(r.cloud_count, None);
    assert_eq!(r.multiple, None);
}

/// `multiple`：`multiple` 是**倍数**、`cloudCount` 是**合计**，两个都要读。
#[test]
fn multiple_reads_both_the_factor_and_the_total() {
    let body = r#"{"code":0,"msg":"success","result":{"cloudCount":42,"multiple":14}}"#;
    let r = parse_multiple(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(r.multiple, Some(14), "倍数是 14 倍");
    assert_eq!(r.cloud_count, Some(42), "合计是 42 豆");
    assert_ne!(r.multiple, r.cloud_count, "两者不是一个量");
}

/// ⭐ 用**真账号抓包**对拍奖品记录。
#[test]
fn parses_the_real_prize_record_capture() {
    let v = vectors();
    let env = SignInEnvelope::parse(&raw_body(&v, "prizeRecords")).unwrap();
    let recs = parse_prize_records(&env).unwrap();

    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(r.prize_name, "移动云盘1000万tokens叠加包");
    assert_eq!(r.flag, 1);
    assert!(r.pending(), "flag==1 ⇒ 待领取");
    assert_eq!(r.market, "sign_in_3_ex");
    // `oid` 实拍是**数字**，实现侧统一成字符串
    assert_eq!(r.oid, "1270932714");
    assert_eq!(r.prize_id, 260630014);
    assert_eq!(
        r.expire_time.as_deref(),
        Some("2026-09-30T15:59:59.000+00:00")
    );
}

/// `flag != 1` 不是待领取。
#[test]
fn only_flag_one_is_pending() {
    let body = r#"{"code":0,"msg":"success","result":{"records":[
      {"prizeName":"a","flag":0},{"prizeName":"b","flag":1},{"prizeName":"c","flag":2}]}}"#;
    let recs = parse_prize_records(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert!(!recs[0].pending());
    assert!(recs[1].pending());
    assert!(!recs[2].pending());
}

/// `oid` 两种形态都收（数字与字符串）。
#[test]
fn oid_accepts_number_and_string() {
    let body = r#"{"code":0,"msg":"success","result":{"records":[
      {"oid":1270932714},{"oid":"1270932714"}]}}"#;
    let recs = parse_prize_records(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(recs[0].oid, "1270932714");
    assert_eq!(recs[1].oid, "1270932714");
}

/// 流水解析（正负号就是进出）。
#[test]
fn parses_cloud_records() {
    let body = r#"{"code":0,"msg":"success","result":{"records":[
      {"num":3,"summary":"签到","mark":"sign","inserttime":"2026-09-19T01:00:00.000+00:00"},
      {"num":-100,"summary":"兑换","mark":"1003"}]}}"#;
    let recs = parse_cloud_records(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].num, 3);
    assert_eq!(recs[0].summary, "签到");
    assert_eq!(recs[1].num, -100, "支出是负数");
    assert_eq!(recs[1].insert_time, None);
}

// ══════════════════════════ 信封语义 ══════════════════════════

/// ⭐ 签到族的 code 是**数字**，与登录族的 `"0000"` 不是同一个信封。
#[test]
fn signin_envelope_is_numeric_not_string_zero() {
    let env = SignInEnvelope::parse(r#"{"code":0,"msg":"success"}"#).unwrap();
    assert_eq!(env.code, 0);
    assert!(env.ensure_ok("测试").is_ok());

    // 登录族那种 "0000" 也收（数字字符串），但语义要按签到族判
    let env = SignInEnvelope::parse(r#"{"code":"0"}"#).unwrap();
    assert_eq!(env.code, 0);
}

/// `msg` 与 `message` 两个键名都收。
#[test]
fn message_falls_back_to_the_other_key() {
    assert_eq!(
        SignInEnvelope::parse(r#"{"code":1,"message":"用 message"}"#)
            .unwrap()
            .message,
        "用 message"
    );
    assert_eq!(
        SignInEnvelope::parse(r#"{"code":1,"msg":"用 msg"}"#)
            .unwrap()
            .message,
        "用 msg"
    );
}

/// 非 0 的 code 必须变成**错误**，且把上游原文带上。
#[test]
fn non_zero_code_becomes_an_error_naming_the_upstream_text() {
    let env = SignInEnvelope::parse(r#"{"code":615,"msg":"设备号不能为空"}"#).unwrap();
    let err = env.ensure_ok("领AI豆").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("615"), "要带业务码：{text}");
    assert!(text.contains("设备号不能为空"), "要带上游原文：{text}");
    assert!(text.contains("领AI豆"), "要带是哪一步：{text}");
}

/// 不是 JSON ⇒ 解析错误（不是 panic，也不是当成成功）。
#[test]
fn non_json_body_is_a_parse_error() {
    let err = SignInEnvelope::parse("<html>网关错误</html>").unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "实得：{err:?}");
}

/// ⚠ `result` 是**裸数字**时 `ok_result` **照样返回它** ——
/// 形状校验交给各自的解析函数，不在这里做。
///
/// # 为什么不在信封层判"必须是对象"
///
/// 这一族里有接口的 `result` **本来就是数字**
/// （`receiveTask` 回 `result: 100`，实拍）。在信封层要求对象，
/// 会把这些接口全部判成解析失败 —— 而"成功但形状不同"被报成失败，
/// 用户看到的是"天天报错"，实际什么事都没有。
#[test]
fn bare_numeric_result_passes_the_envelope() {
    let env = SignInEnvelope::parse(r#"{"code":0,"msg":"success","result":100}"#).unwrap();
    assert!(env.ensure_ok("领任务奖励").is_ok());
    let r = env.ok_result("领任务奖励").unwrap();
    assert_eq!(*r, serde_json::json!(100), "信封只搬 result，不判形状");

    // 而真正需要对象的解析函数会自己拒绝它
    assert!(parse_status(&env).is_err(), "infoV3 要对象，数字得报错");
}

// ══════════════════════════ backup-gift（独立主机）══════════════════════════

/// ⚠ 它走的是**另一个主机**，不是 `m.mcloud.139.com`。
#[test]
fn backup_gift_endpoints_live_on_caiyun() {
    for url in [BACKUP_GIFT_INFO, BACKUP_GIFT_RECEIVE] {
        assert!(
            url.starts_with("https://caiyun.feixin.10086.cn/market/backupgift/"),
            "备份好礼不在 ycloud 那一族里：{url}"
        );
    }
    // 与这一族其余端点**必须**不同主机
    assert!(INFO_V3.starts_with("https://m.mcloud.139.com/ycloud/"));
    assert_ne!(
        BACKUP_GIFT_INFO.split('/').nth(2),
        INFO_V3.split('/').nth(2),
        "两者主机不同是刻意的，别顺手统一掉"
    );
}

#[test]
fn backup_gift_requests_shape() {
    let info = backup_gift_info_request();
    assert_eq!(info.url, BACKUP_GIFT_INFO);
    assert!(info.body.is_none(), "状态查询是裸 GET");
    // 与这一族一样带 activityid / appversion
    assert!(info
        .headers
        .iter()
        .any(|(k, v)| k == "activityid" && v == MARKET_NAME));

    let recv = backup_gift_receive_request();
    assert_eq!(recv.url, BACKUP_GIFT_RECEIVE);
    // ⚠ GET 但是写 —— 这条断言防的是"哪天有人把它改成 POST"
    assert_eq!(recv.method, "GET", "上游就是 GET（却会记账）");
}

/// 三态常量与旧项目 `switch (state)` 逐字一致。
#[test]
fn backup_gift_states_match_the_reference() {
    assert_eq!(backup_gift_state::OFF, -1);
    assert_eq!(backup_gift_state::CLAIMABLE, 0);
    assert_eq!(backup_gift_state::CLAIMED, 1);
}

#[test]
fn parses_backup_gift_info() {
    let body = r#"{"code":0,"msg":"success","result":{"state":0,"curMonth":true}}"#;
    let g = parse_backup_gift(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(g.state, 0);
    assert!(g.cur_month);
    assert!(g.claimable());
    assert!(!g.no_gift_this_month());
}

/// `curMonth == false` 比 `state` **更靠前**：它一票否决。
#[test]
fn backup_gift_cur_month_vetoes_claimability() {
    let body = r#"{"code":0,"msg":"success","result":{"state":0,"curMonth":false}}"#;
    let g = parse_backup_gift(&SignInEnvelope::parse(body).unwrap()).unwrap();
    assert!(g.no_gift_this_month());
    assert!(!g.claimable(), "本月没有这份礼时不该可领");
}

/// 三态各自的可领性。
#[test]
fn backup_gift_claimability_by_state() {
    let mk = |state: i64| {
        let body =
            format!(r#"{{"code":0,"msg":"success","result":{{"state":{state},"curMonth":true}}}}"#);
        parse_backup_gift(&SignInEnvelope::parse(&body).unwrap()).unwrap()
    };
    assert!(mk(backup_gift_state::CLAIMABLE).claimable(), "0 = 可领");
    assert!(!mk(backup_gift_state::OFF).claimable(), "-1 = 未开备份");
    assert!(
        !mk(backup_gift_state::CLAIMED).claimable(),
        "1 = 本月已领过"
    );
}

/// `state` 是**必填**（主判据），缺了要报错。
#[test]
fn backup_gift_requires_state() {
    let body = r#"{"code":0,"msg":"success","result":{"curMonth":true}}"#;
    let err = parse_backup_gift(&SignInEnvelope::parse(body).unwrap()).unwrap_err();
    assert!(err.to_string().contains("state"), "实得：{err}");
}

/// `receive` 的 `result` 是**裸数字**，缺了也给 `None`（不判失败）。
#[test]
fn parses_backup_gift_receive_forms() {
    // 裸数字
    let a = parse_backup_gift_receive(
        &SignInEnvelope::parse(r#"{"code":0,"msg":"success","result":1}"#).unwrap(),
    )
    .unwrap();
    assert_eq!(a.received, Some(1));

    // 数字字符串
    let b = parse_backup_gift_receive(
        &SignInEnvelope::parse(r#"{"code":0,"msg":"success","result":"2"}"#).unwrap(),
    )
    .unwrap();
    assert_eq!(b.received, Some(2));

    // 整个 result 缺失 ⇒ 不是失败（领取这个动作本身已经成功了）
    let c =
        parse_backup_gift_receive(&SignInEnvelope::parse(r#"{"code":0,"msg":"success"}"#).unwrap())
            .unwrap();
    assert_eq!(c.received, None);

    // 非 0 码仍然是失败
    assert!(parse_backup_gift_receive(
        &SignInEnvelope::parse(r#"{"code":500,"msg":"系统繁忙"}"#).unwrap()
    )
    .is_err());
}
