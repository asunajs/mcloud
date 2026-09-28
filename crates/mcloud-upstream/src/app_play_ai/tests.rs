//! 拍拍协议层的测试。
//!
//! 钉的都是"改错了也不报错"的那些：多/少一个查询参数、多带两个头、
//! 把 `groupId` 写成 `groupid`、把字符串 `code` 当数字解。

use serde_json::json;

use super::*;

// ══════════════════════════ 夹具（与参考实现的裁剪口径一致）══════════════════════════

/// 夹具第 4 条（`user/info` 第一次，12:10 那一带），逐字。
const STATUS_BEFORE: &str = r#"{"code":0,"msg":"success","result":{
    "showData":{"prizeBg":"https://img.mcloud.139.com//uop_prod/x.png","prizeImages":["a","b"],
    "titleImage":"https://img.mcloud.139.com//uop_prod/y.png"},
    "totalLotteryCount":0}}"#;

/// 夹具第 11 条 —— **同一条请求**的第二次，`totalLotteryCount` 从 `0` 变 `1`。
const STATUS_AFTER: &str = r#"{"code":0,"msg":"success","result":{
    "showData":{"prizeBg":"https://img.mcloud.139.com//uop_prod/x.png","prizeImages":["a","b"],
    "titleImage":"https://img.mcloud.139.com//uop_prod/y.png"},
    "totalLotteryCount":1}}"#;

/// 任务表第一次（夹具第 5 条）。
///
/// ⚠ **裁剪过**：实拍一条有 **21 个键**，这里每条只留本层读的十个 + `button`
/// （压成一键）+ `sort` + `updateTime` —— 后三个是"读了也没用、但**不许**因为
/// 它在就报错"的那一类。裁剪口径与参考实现一致（原始 body 里还有素材 URL
/// 与已脱敏的跳转凭据，那些不进仓库）。
const TASKS_BEFORE: &str = r#"{"code":0,"msg":"success","result":[
 {"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",
  "groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,
  "button":{"app":{"ext":"click"}},"sort":1,"updateTime":"2026-06-29 20:10:43"},
 {"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",
  "groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,
  "button":{"app":{"ext":"click"}},"sort":2,"updateTime":"2026-06-29 20:10:43"},
 {"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",
  "groupId":"popup","state":"FINISH","limitType":"EVER","recordid":"20466372","currstep":2,"process":0,
  "button":{"app":{"ext":"click"}},"sort":3,"updateTime":"2026-05-30 23:15:58"}]}"#;

/// 任务表第二次（夹具第 10 条）。
///
/// 与上面那份的**全部**差别只在 `id:1` 那条：`WAIT`→`FINISH`、`recordid` 由 `null`
/// 变**字符串** `"20674897"`、`currstep` 0→3。
/// ⚠ `process` 两条都是 `0`（连已完成的那条也是）⇒ "进度"读不出。
const TASKS_AFTER: &str = r#"{"code":0,"msg":"success","result":[
 {"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",
  "groupId":"normal","state":"FINISH","limitType":"DAY","recordid":"20674897","currstep":3,"process":0,
  "button":{"app":{"ext":"click"}},"sort":1,"updateTime":"2026-06-29 20:12:26"},
 {"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",
  "groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,
  "button":{"app":{"ext":"click"}},"sort":2,"updateTime":"2026-06-29 20:10:43"},
 {"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",
  "groupId":"popup","state":"FINISH","limitType":"EVER","recordid":"20466372","currstep":2,"process":0,
  "button":{"app":{"ext":"click"}},"sort":3,"updateTime":"2026-05-30 23:15:58"}]}"#;

fn header<'a>(req: &'a HttpRequest, key: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

// ══════════════════════════ 请求形状 ══════════════════════════

/// ① 的 URL **逐字**：只带 `marketName`，**不带 `source`**（与 ② 差一个键）。
#[test]
fn status_url_carries_only_the_market_name() {
    let req = play_ai_status_request();
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/newyear/user/info?marketName=National_PlayAISpecial"
    );
    assert!(!req.url.contains("source="), "① 不带 source：{}", req.url);
    assert!(req.body.is_none(), "实拍请求体是空的");
}

/// ② 的 URL **逐字**：两个键、顺序就是 `marketName` 在前 `source` 在后。
#[test]
fn task_list_url_carries_market_name_then_source() {
    let req = play_ai_task_list_request();
    assert_eq!(req.method, "GET");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/newyear/task/list?marketName=National_PlayAISpecial&source=app"
    );
    // 顺序也是钉住的一部分：交给库排序的话新增键会悄悄变
    let q = req.url.split_once('?').unwrap().1;
    assert!(q.starts_with("marketName="), "实得：{q}");
    assert!(q.ends_with("&source=app"), "实得：{q}");
}

/// ⚠⚠ 这一族**不发** `activityid` / `appversion` / `isdeviceid` 这三个头。
///
/// 它们不是被脱敏抹掉的：同一份归档里活动门户那四条**都带**这三个头，
/// 说明归档器保留头名 ⇒ 是这一族真的没发。
/// ⇒ **不许**改用 `app_signin::h5()`（它会多加 `activityid` 与 `appversion`）。
#[test]
fn this_family_does_not_send_the_three_extra_headers() {
    for req in [play_ai_status_request(), play_ai_task_list_request()] {
        for absent in ["activityid", "appversion", "isdeviceid"] {
            assert!(
                header(&req, absent).is_none(),
                "这一族不发 `{absent}`（复用 h5() 就会多出来）：{req:?}"
            );
        }
    }
}

/// 这一族实际发的三片头，`jwttoken` 由会话层注入。
#[test]
fn this_family_sends_exactly_three_headers() {
    for req in [play_ai_status_request(), play_ai_task_list_request()] {
        assert_eq!(header(&req, "Accept"), Some("*/*"));
        assert_eq!(header(&req, "showloading"), Some("true"));
        assert_eq!(
            header(&req, "Referer"),
            Some(play_ai_page_referer().as_str())
        );
        assert!(
            header(&req, "jwttoken").is_none(),
            "票由 H5Client 注入，不在这里"
        );
    }
}

/// 页面 Referer **不借**会话里签到页那一份（票借、页面身份不借）。
#[test]
fn the_referer_is_this_activity_page_not_the_signin_one() {
    let referer = play_ai_page_referer();
    assert_eq!(
        referer,
        "https://m.mcloud.139.com/portal/yunpanpage/index.html\
         ?path=National_PlayAISpecial&sourceid=2019&enableShare=1"
    );
    assert!(referer.contains("yunpanpage"), "不是签到页那个落地 URL");
    assert!(referer.contains("sourceid=2019"));
}

// ══════════════════════════ ① 解析 ══════════════════════════

/// 实拍两条：`0`（做任务之前）→ `1`。
///
/// ⚠ `0` 是**合法取值**，所以这一格必须能与"上游没给"分得开（是 `Option`）。
#[test]
fn parses_the_captured_status_before_and_after() {
    assert_eq!(
        parse_play_ai_status(STATUS_BEFORE)
            .unwrap()
            .total_lottery_count,
        Some(0),
        "0 是实拍值，不是「没给」"
    );
    assert_eq!(
        parse_play_ai_status(STATUS_AFTER)
            .unwrap()
            .total_lottery_count,
        Some(1)
    );
}

/// 上游没给那一格 ⇒ `None`（**不伪造成 0**）。
///
/// 语义未验证的那一格，把"没给"读成"0 次"正好会得出"不能抽"的相反结论。
#[test]
fn a_missing_lottery_count_stays_none() {
    let body = r#"{"code":0,"msg":"success","result":{"showData":{}}}"#;
    assert_eq!(
        parse_play_ai_status(body).unwrap().total_lottery_count,
        None
    );
}

/// `result` 缺失或不是对象 ⇒ **报错**（实拍两条都是对象）。
#[test]
fn a_bad_status_result_is_an_error() {
    assert!(parse_play_ai_status(r#"{"code":0,"msg":"success"}"#).is_err());
    assert!(
        parse_play_ai_status(r#"{"code":0,"msg":"success","result":[]}"#).is_err(),
        "result 是数组要响"
    );
}

// ══════════════════════════ ② 解析 ══════════════════════════

/// 两次实拍都要解得出来，且 `id:1` 那条从 `WAIT`/`null`/0 走到 `FINISH`/字符串/3。
#[test]
fn parses_both_captured_task_lists() {
    let before = parse_play_ai_task_list(TASKS_BEFORE).unwrap();
    assert_eq!(before.len(), 3);
    let first = &before[0];
    assert_eq!(first.task_id, Some(1));
    assert_eq!(first.name.as_deref(), Some("体验拍照问AI"));
    assert_eq!(first.flag.as_deref(), Some("拍照问AI"));
    assert_eq!(first.description.as_deref(), Some("抽奖次数+1"));
    assert_eq!(first.group_id.as_deref(), Some("normal"));
    assert_eq!(first.state.as_deref(), Some(PLAY_AI_TASK_STATE_WAITING));
    assert_eq!(first.limit_type.as_deref(), Some("DAY"));
    assert_eq!(first.record_id, None, "没做过时是 null");
    assert_eq!(first.curr_step, Some(0));

    let after = parse_play_ai_task_list(TASKS_AFTER).unwrap();
    let first = &after[0];
    assert_eq!(first.state.as_deref(), Some(PLAY_AI_TASK_STATE_DONE));
    assert_eq!(
        first.record_id.as_deref(),
        Some("20674897"),
        "做过时是字符串"
    );
    assert_eq!(first.curr_step, Some(3));
}

/// ⚠⚠ `groupId` 是**驼峰** —— 这一族与签到任务表那族的**真实差异**。
///
/// 写成 `groupid` 会**静默读不到**（读到 `None`，而 `None` 是合法的"上游没给"）。
#[test]
fn group_id_is_camel_case_and_lowercase_is_not_read() {
    let tasks = parse_play_ai_task_list(TASKS_BEFORE).unwrap();
    assert_eq!(tasks[0].group_id.as_deref(), Some("normal"));
    assert_eq!(tasks[2].group_id.as_deref(), Some("popup"));

    // 反证：给一份**全小写** `groupid` 的体，这一格读不出来（说明确实在按驼峰取）
    let lowercase = r#"{"code":0,"msg":"success","result":[
        {"id":1,"groupid":"normal","state":"WAIT"}]}"#;
    let t = parse_play_ai_task_list(lowercase).unwrap();
    assert_eq!(
        t[0].group_id, None,
        "全小写 groupid 不该被读到 —— 那会掩盖「两族键名不同」这件事"
    );
}

/// ⚠ `process` 实拍**连已完成的那条也是 0** —— 所以"进度"这个口径在它上面说不通。
///
/// 两个都存、**不加解释**：该读 `process` 还是 `curr_step` 未验证。
#[test]
fn process_is_zero_even_for_a_finished_task() {
    let tasks = parse_play_ai_task_list(TASKS_AFTER).unwrap();
    for t in &tasks {
        assert_eq!(t.process, Some(0), "实拍三条全是 0：{t:?}");
    }
    let finished = tasks
        .iter()
        .find(|t| t.state.as_deref() == Some(PLAY_AI_TASK_STATE_DONE))
        .expect("有一条第 FINISH");
    assert_eq!(
        finished.process,
        Some(0),
        "已完成的条目 process 仍是 0 ⇒ 拿它当「进度」会读错"
    );
    assert!(
        finished.curr_step.unwrap_or(0) > 0,
        "真正的推进在这两格上是 curr_step"
    );
}

/// `result` 不是数组 ⇒ **报错**，不"顺手兼容两种形状"。
#[test]
fn a_non_array_task_result_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":{"id":1}}"#;
    let err = parse_play_ai_task_list(body).expect_err("对象形态要响");
    assert!(err.to_string().contains("不是数组"), "实得：{err}");
}

/// `result` 里有一条不是对象 ⇒ **报错**（不是跳过）。
///
/// 任务表是本能力的判据来源：一条读不出来的记录比"少一条"更危险
/// （调用方会以为那一步没做，然后再上报一次）。
#[test]
fn a_non_object_task_entry_is_an_error() {
    let body = r#"{"code":0,"msg":"success","result":[{"id":1},{"id":2},123]}"#;
    assert!(parse_play_ai_task_list(body).is_err());
}

/// 空数组是合法的（**不**等于"这个号没事做"——上游两种情形同一种回法）。
#[test]
fn an_empty_task_list_parses_to_nothing() {
    let body = r#"{"code":0,"msg":"success","result":[]}"#;
    assert!(parse_play_ai_task_list(body).unwrap().is_empty());
}

/// 缺 `id` / `state` 等键 ⇒ 那些格子是 `None`，**不报错**（上游可能就不给）。
#[test]
fn missing_task_fields_stay_none() {
    let body = r#"{"code":0,"msg":"success","result":[{"name":"只有名字"}]}"#;
    let t = parse_play_ai_task_list(body).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].task_id, None);
    assert_eq!(t[0].name.as_deref(), Some("只有名字"));
    assert_eq!(t[0].state, None, "读不出状态就是读不出，别折成「未完成」");
}

// ══════════════════════════ 信封：这一族只认数字 code ══════════════════════════

/// ⚠⚠ `code` 是**字符串** ⇒ 报错。
///
/// 这一族实拍是**数字** `0`；而 AI 那族的 `code` 是字符串 `"0000"`。
/// 宽松解析会把 `"0000"` 落成 `0`、也就是**判成成功** ——
/// 这道检查就是为了在那一刻立刻响，而不是拿错族的响应当成这一族的读数。
#[test]
fn a_string_code_is_rejected_because_the_other_family_uses_that_shape() {
    let body = r#"{"success":true,"code":"0000","message":"ok","data":{}}"#;
    let err = parse_play_ai_status(body).expect_err("字符串 code 要响");
    assert!(
        err.to_string().contains("不是数字字面量"),
        "要说清是形态不对（两族信封不同构）：{err}"
    );
}

/// 没有 `code` 字段 ⇒ 报错（这一族必带）。
#[test]
fn a_missing_code_is_an_error() {
    let body = r#"{"msg":"success","result":{}}"#;
    let err = parse_play_ai_status(body).expect_err("缺 code 要响");
    assert!(err.to_string().contains("code"), "实得：{err}");
}

/// 非 0 业务码 ⇒ 报错，且把上游那句带出来。
#[test]
fn a_non_zero_code_is_an_error() {
    let body = r#"{"code":500,"msg":"系统繁忙","result":{}}"#;
    let err = parse_play_ai_status(body).expect_err("非 0 要报错");
    assert!(err.to_string().contains("500"), "实得：{err}");
}

/// `90001` 要落成"票的问题"那句人话，而不是干巴巴的"失败 code=90001"。
#[test]
fn a_session_expired_code_is_explained_as_a_session_problem() {
    let body = r#"{"code":90001,"msg":"未登录","result":{}}"#;
    let err = parse_play_ai_status(body).expect_err("90001 是失败");
    let text = err.to_string();
    assert!(!text.is_empty());
    assert!(
        !text.contains("code=90001") || text.contains("会话") || text.contains("登录"),
        "90001 该被解释成人话（票/会话），实得：{text}"
    );
}

/// 响应不是 JSON ⇒ 报错。
#[test]
fn a_non_json_body_is_an_error() {
    assert!(parse_play_ai_status("not json").is_err());
}

/// 常量与实拍一致（改它们等于让请求打到别处）。
#[test]
fn the_constants_match_the_capture() {
    assert_eq!(PLAY_AI_MARKET_NAME, "National_PlayAISpecial");
    assert_eq!(PLAY_AI_SOURCE, "app");
    assert_eq!(PLAY_AI_SHOW_LOADING, "true");
    assert_eq!(PLAY_AI_PAGE_SOURCE_ID, "2019");
    assert_eq!(PLAY_AI_TASK_STATE_WAITING, "WAIT");
    assert_eq!(PLAY_AI_TASK_STATE_DONE, "FINISH");
    // 转义一下，免得被 lint 当成没用到
    let _ = json!({});
}
