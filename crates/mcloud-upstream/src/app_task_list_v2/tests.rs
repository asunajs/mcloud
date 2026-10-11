//! `taskListV2` 协议层的测试。
//!
//! 钉的都是"改错了也不报错"的那些：body 少一个键、`activityid` 落了**两枚**、
//! 把 `groupid` 写成 `groupId`、把 `result` 当数组解（那是 V3 的形状）。

use serde_json::json;

use super::*;

/// 夹具第一条（group `new`）：这一组今天没有任务，`result` 是**空对象**。
const CAPTURED_NEW: &str = r#"{"code":0,"groupState":{},"msg":"success","result":{}}"#;

/// 夹具第二条（group `beiyong1`）：三条、全 `FINISH`，`result` 按组名分组。
///
/// ⚠ **裁剪过**：实拍一条有 **25 个键**，这里每条只留本层读的十个 ——
/// 其余是 `button` / `buttonProperty` 七份端形态、`steps` / `stepTypeSet`、
/// 素材与投放字段。裁剪口径与参考实现一致（原始体里还有已脱敏的
/// `caiyun.feixin.10086.cn:<REDACTED>?…&token=<REDACTED>` 跳转链接，那些不进仓库）。
const CAPTURED_BEIYONG1: &str = r#"{"code":0,"msg":"success",
 "groupState":{"beiyong1":{"taskFinishState":2}},
 "result":{"beiyong1":[
  {"id":596,"name":"珍藏童真时刻","groupid":"beiyong1","state":"FINISH","limitType":"EVER",
   "enable":1,"currstep":4,"process":0,"marketname":"sign_in_3","sort":1},
  {"id":597,"name":"制作童趣3D玩偶","description":"永葆童心 领38云朵","groupid":"beiyong1",
   "state":"FINISH","limitType":"EVER","enable":1,"currstep":6,"process":0,
   "marketname":"sign_in_3","sort":2},
  {"id":598,"name":"九宫格趣晒萌娃","description":"赢至高88云朵","groupid":"beiyong1",
   "state":"FINISH","limitType":"EVER","enable":1,"currstep":8,"process":0,
   "marketname":"sign_in_3","sort":3}]}}"#;

fn header_values<'a>(req: &'a HttpRequest, key: &str) -> Vec<&'a str> {
    req.headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
        .collect()
}

// ══════════════════════════ 请求形状 ══════════════════════════

/// body 三键**逐字**，键序与实拍一致（`serde_json` 默认按键名排序）。
#[test]
fn the_body_is_exactly_the_captured_one() {
    let req = task_list_v2_request("sign_in_3", "new");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/signin/task/taskListV2"
    );
    let body = req.body_str().expect("这一口要带 body");
    assert_eq!(
        body, r#"{"clientVersion":"12.6.0","group":"new","marketname":"sign_in_3"}"#,
        "键序也要与实拍一致"
    );
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v.as_object().map(|o| o.len()), Some(3));
    assert!(
        v.get("client").is_none(),
        "`client` 是 V3 那一口的键，这一口**没有**"
    );
}

/// ⚠⚠ `activityid` **只有一枚**，且**跟着本次的 marketname 走**。
///
/// `h5()` 会固定写 `sign_in_3`，而 [`HttpRequest::header`] 是**追加**（`push`）——
/// 只 `.header("activityid", marketname)` 会得到**两枚同名头**，
/// 于是"哪枚生效"变成传输层的事，而实拍只有一枚。
#[test]
fn activity_id_follows_the_market_and_appears_exactly_once() {
    let req = task_list_v2_request(TASK_LIST_V2_EMAIL_MARKET, "new");
    let values = header_values(&req, "activityid");
    assert_eq!(
        values.len(),
        1,
        "必须只有一枚 activityid（追加会产生两枚）：{values:?}"
    );
    assert_eq!(
        values[0], TASK_LIST_V2_EMAIL_MARKET,
        "它跟着本次的 marketname 走，不是恒等于签到页那个 sign_in_3"
    );

    // 云盘那一支同样只有一枚
    let req = task_list_v2_request("sign_in_3", "new");
    assert_eq!(header_values(&req, "activityid"), vec!["sign_in_3"]);
}

/// 这一族的头与拍拍那一族**正好相反**：它**带** `appversion` / `showloading` / `accept`。
#[test]
fn this_family_does_carry_the_shared_h5_headers() {
    let req = task_list_v2_request("sign_in_3", "new");
    assert_eq!(header_values(&req, "Accept"), vec!["*/*"]);
    assert_eq!(header_values(&req, "showloading"), vec!["true"]);
    assert_eq!(
        header_values(&req, "appversion"),
        vec![crate::app_signin::H5_APP_VERSION],
        "这一族带 appversion（与拍拍那族相反）"
    );
    assert!(header_values(&req, "isdeviceid").is_empty());
}

/// `clientVersion` 是**三段**，而请求头 `appversion` 是**四段** —— 别混用。
#[test]
fn client_version_is_three_segments() {
    assert_eq!(TASK_CLIENT_VERSION, "12.6.0");
    assert_eq!(TASK_CLIENT_VERSION.split('.').count(), 3);
    assert_eq!(
        crate::app_signin::H5_APP_VERSION.split('.').count(),
        4,
        "头里那个是四段，所以两个常量不能互换"
    );
    assert_ne!(TASK_CLIENT_VERSION, crate::app_signin::H5_APP_VERSION);
}

/// 八个 group 的字面量与**顺序**（顺序是行为的一部分）。
#[test]
fn the_eight_groups_are_pinned_in_order() {
    assert_eq!(
        TASK_LIST_V2_GROUPS,
        [
            "new",
            "beiyong1",
            "beiyong2",
            "cloudEmail",
            "time",
            "day",
            "month",
            "hidden"
        ]
    );
    assert_eq!(TASK_LIST_V2_EMAIL_MARKET, "newsign_139mail");
}

/// group 是**参数**：换一个就换一个 body。
#[test]
fn the_group_flows_into_the_body() {
    let req = task_list_v2_request("sign_in_3", "beiyong2");
    let v: Value = serde_json::from_str(req.body_str().unwrap()).unwrap();
    assert_eq!(v["group"], "beiyong2");
    assert_eq!(v["marketname"], "sign_in_3");
}

// ══════════════════════════ 解析 ══════════════════════════

/// 空组：`result: {}` + `groupState: {}` ⇒ 0 条，且**不报错**。
///
/// 把"上游没给这一组"判成解析失败的代价是**整条聚合链**被一次空响应打断。
#[test]
fn an_empty_group_parses_to_no_items() {
    let parsed = parse_task_list_v2(CAPTURED_NEW, "new").unwrap();
    assert_eq!(parsed.group, "new", "请求的那名要留下来对账");
    assert!(parsed.items.is_empty());
    assert!(parsed.group_finish_states.is_empty());
}

/// `result` 整个**缺席**也按空组处理（同样不报错）。
#[test]
fn a_missing_result_is_treated_as_an_empty_group() {
    let parsed = parse_task_list_v2(r#"{"code":0,"msg":"success"}"#, "time").unwrap();
    assert!(parsed.items.is_empty());
}

/// 三条实拍要逐字解出来。
#[test]
fn parses_the_captured_beiyong1_group() {
    let parsed = parse_task_list_v2(CAPTURED_BEIYONG1, "beiyong1").unwrap();
    assert_eq!(parsed.items.len(), 3);
    assert_eq!(parsed.group, "beiyong1");

    let first = &parsed.items[0];
    assert_eq!(first.task_id, Some(596));
    assert_eq!(first.name.as_deref(), Some("珍藏童真时刻"));
    assert_eq!(first.description, None, "第一条实拍没有 description");
    assert_eq!(first.group_id.as_deref(), Some("beiyong1"));
    assert_eq!(first.state.as_deref(), Some("FINISH"));
    assert_eq!(first.limit_type.as_deref(), Some("EVER"));
    assert_eq!(first.enable, Some(1));
    assert_eq!(first.curr_step, Some(4));
    assert_eq!(first.process, Some(0));
    assert_eq!(first.market_name.as_deref(), Some("sign_in_3"));

    assert_eq!(
        parsed.items[1].description.as_deref(),
        Some("永葆童心 领38云朵")
    );
    // currstep 三条是 4/6/8
    let steps: Vec<Option<i64>> = parsed.items.iter().map(|t| t.curr_step).collect();
    assert_eq!(steps, vec![Some(4), Some(6), Some(8)]);
}

/// ⭐ `button.app.link` 也要读出来 —— 玩法 615 那一族**唯一的门**就在这一格 ✓。
///
/// 形状取自抓包仓 `har/2026-09-25-通知有礼.har`（同族条目 `id=614` 的 `button`）：
/// `{"text":"去完成","action":"jump","link":"mcloud://main/uploadView"}` ✓。
/// ⚠ 这里把**坏形状**也一起钉住：读成 `Some("")` 之类假值会让那一族误判成"该传 10 张" ✗。
#[test]
fn button_app_link_is_read_and_bad_shapes_stay_none() {
    let body = |b: &str| {
        format!(
            r#"{{"code":0,"msg":"success","result":{{"beiyong1":[{{"id":615,"name":"记录国庆节欢乐瞬间","groupid":"beiyong1","state":"WAIT","enable":1,"process":0,"marketname":"sign_in_3","button":{b}}}]}}}}"#
        )
    };
    let ok = parse_task_list_v2(
        &body(r#"{"app":{"text":"去完成","action":"jump","link":"mcloud://main/uploadView"}}"#),
        "beiyong1",
    )
    .unwrap();
    assert_eq!(
        ok.items[0].button_app_link.as_deref(),
        Some("mcloud://main/uploadView")
    );

    for bad in [
        "{}",                       // 没有 app 这一层
        r#"{"app":{}}"#,            // 有 app、没有 link
        r#"{"app":{"link":null}}"#, // null
        r#"{"app":{"link":123}}"#,  // 不是字符串
    ] {
        let p = parse_task_list_v2(&body(bad), "beiyong1").unwrap();
        assert_eq!(p.items[0].button_app_link, None, "坏形状 ⇒ None：{bad}");
    }
    // 连 `button` 整格都没有 ⇒ 同样 `None`（不报错 ✓）
    let none = parse_task_list_v2(
        r#"{"code":0,"msg":"success","result":{"beiyong1":[{"id":615,"groupid":"beiyong1"}]}}"#,
        "beiyong1",
    )
    .unwrap();
    assert_eq!(none.items[0].button_app_link, None);
}

/// ⚠⚠ 这一口的 `groupid` 是**全小写** —— 与拍拍那一族的驼峰 `groupId` **相反**。
///
/// 写成 `groupId` 会**静默读不到**（读到 `None`，而 `None` 是合法的"上游没给"）。
#[test]
fn group_id_is_lowercase_here_unlike_the_play_ai_family() {
    let parsed = parse_task_list_v2(CAPTURED_BEIYONG1, "beiyong1").unwrap();
    assert_eq!(parsed.items[0].group_id.as_deref(), Some("beiyong1"));

    // 反证：给一份**驼峰** `groupId` 的体，这一格读不出来
    let camel = r#"{"code":0,"msg":"success","result":{"g":[
        {"id":1,"groupId":"beiyong1","state":"FINISH"}]}}"#;
    let parsed = parse_task_list_v2(camel, "g").unwrap();
    assert_eq!(
        parsed.items[0].group_id, None,
        "驼峰 groupId 不该被读到 —— 那会掩盖「两族键名不同」这件事"
    );
}

/// ⚠ `state` 已是 `FINISH` 而 `process` 仍是 `0`，`currstep` 却是 4/6/8。
///
/// ⇒ "进度到底读哪个字段"未验证，两格都存、不加解释。
#[test]
fn process_is_zero_while_currstep_moved() {
    let parsed = parse_task_list_v2(CAPTURED_BEIYONG1, "beiyong1").unwrap();
    for t in &parsed.items {
        assert_eq!(t.process, Some(0), "实拍三条全是 0");
        assert_eq!(t.state.as_deref(), Some("FINISH"));
    }
    assert!(
        parsed.items.iter().all(|t| t.curr_step.unwrap_or(0) > 0),
        "真正的推进在这两格上是 currstep"
    );
}

/// `groupState.<group>.taskFinishState` 要按组交出来（**只存不判**）。
#[test]
fn group_finish_states_are_carried_out() {
    let parsed = parse_task_list_v2(CAPTURED_BEIYONG1, "beiyong1").unwrap();
    assert_eq!(
        parsed.group_finish_states,
        vec![("beiyong1".to_string(), Some(2))]
    );
    let empty = parse_task_list_v2(CAPTURED_NEW, "new").unwrap();
    assert!(empty.group_finish_states.is_empty(), "实拍那次是空对象");
}

/// ⚠ `result` 是**数组** ⇒ 报错。
///
/// 数组是 **V3** 那一口的形状。兼容代码会"顺手"两种都吃，
/// 于是这条链在形状变化上**不会响**。
#[test]
fn an_array_result_is_rejected_because_that_is_the_other_endpoint() {
    let body = r#"{"code":0,"msg":"success","result":[{"id":1}]}"#;
    let err = parse_task_list_v2(body, "new").expect_err("数组形态要响");
    assert!(
        err.to_string().contains("别混解"),
        "要说清这是与 V3 不同形：{err}"
    );
}

/// `result.<name>` 不是数组 ⇒ 报错。
#[test]
fn a_non_array_group_value_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"beiyong1":{"id":1}}}"#;
    let err = parse_task_list_v2(body, "beiyong1").expect_err("对象值要响");
    assert!(err.to_string().contains("不是数组"), "实得：{err}");
}

/// 键名**不参与筛选**：`result` 里给的是别的组名也照样摊平。
///
/// 旧 TS 是把全部值摊平、不看键名；而"只有一个键"是**一次观测、不是契约**。
#[test]
fn items_are_flattened_regardless_of_the_key_name() {
    let body = r#"{"code":0,"msg":"success","result":{"别的组名":[
        {"id":1,"name":"甲"},{"id":2,"name":"乙"}]}}"#;
    let parsed = parse_task_list_v2(body, "beiyong1").unwrap();
    assert_eq!(parsed.items.len(), 2, "全摊平，不按键名筛");
    assert_eq!(parsed.items[0].name.as_deref(), Some("甲"));
}

/// 缺字段 ⇒ 那些格子是 `None`（**不报错**），且非对象条目也不报错。
#[test]
fn missing_fields_and_blank_entries_stay_none() {
    let body = r#"{"code":0,"msg":"success","result":{"g":[{"name":"只有名字"},123]}}"#;
    let parsed = parse_task_list_v2(body, "g").unwrap();
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.items[0].task_id, None);
    assert_eq!(
        parsed.items[0].state, None,
        "读不出状态就是读不出，别折成「未完成」"
    );
    assert_eq!(
        parsed.items[1],
        TaskV2Item::default(),
        "非对象条目落成全 None"
    );
}

/// 非 0 业务码 ⇒ 报错。
#[test]
fn a_non_zero_code_is_an_error() {
    let body = r#"{"code":500,"msg":"系统繁忙"}"#;
    assert!(parse_task_list_v2(body, "new").is_err());
}

/// 响应不是 JSON ⇒ 报错。
#[test]
fn a_non_json_body_is_an_error() {
    assert!(parse_task_list_v2("not json", "new").is_err());
}

/// 转义一下，免得 `json!` 被 lint 当成没用到。
#[test]
fn the_json_helper_is_available() {
    let _ = json!({});
}

/// ⭐ `button.email.link`（玩法 1017 的入口页那一格）—— 2026-10-10 真机原始条目里的形状 ✓。
///
/// 实拍（`newsign_139mail` 组）：
/// `"button":{"email":{"action":"jump","link":"https://m.mcloud.139.com/portal/mobilecloud/index.html?…&token=#ssoToken#","text":"去完成"},"out":{"text":"去完成"}}`
/// ⇒ 两件事一起钉住：① 读得出这一格；② `button.app` 与 `button.email` **互不串台** ✓。
#[test]
fn button_email_link_is_read_and_does_not_leak_into_app_link() {
    let link = "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin&sourceid=1179&enableShare=1&targetSourceId=001005&token=#ssoToken#";
    let body = serde_json::json!({
        "code": 0,
        "msg": "success",
        "result": { "month": [{
            "id": 1017,
            "name": "登录移动云盘云朵中心",
            "groupid": "month",
            "state": "WAIT",
            "currstep": 1,
            "enable": 1,
            "process": 0,
            "marketname": "newsign_139mail",
            "button": {
                "email": { "action": "jump", "link": link, "text": "去完成" },
                "out": { "text": "去完成" }
            }
        }]}
    })
    .to_string();
    let p = parse_task_list_v2(&body, "month").unwrap();
    assert_eq!(p.items[0].button_email_link.as_deref(), Some(link));
    assert_eq!(
        p.items[0].button_app_link, None,
        "只有 email 那一层 ⇒ app 那一格必须是 None（两格不许串台 ✓）"
    );

    // 坏形状一律 `None`（读到假值会让那一档去发一条空链接 ✗）。
    for bad in ["{}", r#"{"email":{}}"#, r#"{"email":{"link":null}}"#] {
        let b = format!(
            r#"{{"code":0,"msg":"success","result":{{"month":[{{"id":1017,"button":{bad}}}]}}}}"#
        );
        let p = parse_task_list_v2(&b, "month").unwrap();
        assert_eq!(p.items[0].button_email_link, None, "坏形状 ⇒ None：{bad}");
    }
}
