//! 消息推送族的协议层测试。
//!
//! 这一组钉的是**实拍逐字**的那些细节 —— 它们全是"改错了也不报错"的类型：
//! 多一个查询参数、少一个字节的 body、键名"顺手改对"，都只会让某个字段恒为 `None`。

use serde_json::json;

use super::*;

/// 2026-09-25 实拍的那份 `task/status` 响应，逐字。
const CAPTURED_STATUS: &str = r#"{"code":0,"msg":"success","result":{
    "pushOn":1,"firstTaskStatus":3,"secondTaskStatus":1,
    "pushTaskStatus":0,"onDuaration":16,"total":31}}"#;

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

/// `status` 是**裸 GET** —— 实拍 URL 里连 `?` 都没有。
///
/// 多加一个查询参数不会有任何报错，只会让上游按另一套参数理解这一枪。
#[test]
fn status_request_is_a_bare_get_with_no_query_string() {
    let req = msg_push_status_request();
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/msgPushOn/task/status"
    );
    assert_eq!(req.method, "GET");
    assert!(
        !req.url.contains('?'),
        "实拍这一条一个查询参数都不带，实得：{}",
        req.url
    );
    assert!(req.body.is_none(), "裸 GET 不该带 body");
}

/// 路径里 `msgPushOn` 是**驼峰**，主机**不带端口**。
#[test]
fn the_path_casing_and_host_are_exact() {
    let req = msg_push_status_request();
    assert!(
        req.url.contains("/ycloud/msgPushOn/task/"),
        "实得：{}",
        req.url
    );
    assert!(!req.url.contains(":7071"), "这一页不在带端口的老域上");
    assert!(
        !req.url.contains("/market/"),
        "实拍走的是 /ycloud/，不是 /market/"
    );
}

/// 这一族的四个头由 `h5` 统一补，且**不带** `isdeviceid`。
///
/// ⚠ 实拍只记到 `jwttoken` 与 `referer` 两片（那两片由会话层注入，
/// 不在请求构造里），其余没记 —— 所以这里断言的是"我们发了哪四片"，
/// 不是"实拍发了哪四片"。
#[test]
fn the_h5_headers_are_the_shared_four_and_no_isdeviceid() {
    let req = msg_push_status_request();
    assert_eq!(header(&req, "Accept"), Some("*/*"));
    assert_eq!(header(&req, "activityid"), Some("sign_in_3"));
    assert_eq!(header(&req, "appversion"), Some("12.6.0.0"));
    assert_eq!(header(&req, "showloading"), Some("true"));
    assert!(
        header(&req, "isdeviceid").is_none(),
        "这一族没有 isdeviceid 的实拍依据，不该凭空带上"
    );
    // `jwttoken` / `referer` 是**会话的两半**，由 H5Client 注入，不在这里。
    assert!(header(&req, "jwttoken").is_none());
    assert!(header(&req, "Referer").is_none());
}

/// `list` 的 body 是**字面量 `null`**（四个字节），不是 `{}`、也不是"没有 body"。
#[test]
fn list_request_sends_the_literal_null_body() {
    let req = msg_push_list_request();
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/msgPushOn/task/list"
    );
    let body = req.body.as_deref().expect("这一枪要带 body");
    assert_eq!(body, "null");
    assert_eq!(body.len(), 4, "实拍 content-length 是 4");
}

/// `list` 的 `content-type` 是**不带 charset** 的那个。
///
/// 客户端在调用方没声明时会补 `application/json;charset=UTF-8` ——
/// 两个多半都被接受，但实拍这一条没有 charset，多出来的参数是我们自己加的。
#[test]
fn list_content_type_has_no_charset() {
    let req = msg_push_list_request();
    assert_eq!(header(&req, "content-type"), Some("application/json"));
    let count = req
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .count();
    assert_eq!(count, 1, "不能出现两个 content-type");
}

/// `obtain` 的 body 是 `{"type":2}`，方法跟随那个**唯一**的常量。
#[test]
fn obtain_request_carries_type_two() {
    let req = msg_push_obtain_request();
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/msgPushOn/task/obtain"
    );
    assert_eq!(
        req.method,
        if MSG_PUSH_OBTAIN_IS_POST {
            "POST"
        } else {
            "GET"
        },
        "方法只由 MSG_PUSH_OBTAIN_IS_POST 决定"
    );
    let body = req.body.as_deref().expect("这一枪要带 body");
    let v: Value = serde_json::from_str(body).expect("body 应是 JSON");
    assert_eq!(v, json!({ "type": 2 }));
}

/// 发出去的 `type` 与触发发送的判据**必须是同一个数**。
///
/// 抄成两个不同的数字，这条链就**永远不发**，而且不会有任何报错。
#[test]
fn the_sent_type_is_the_same_number_as_the_claimable_status() {
    assert_eq!(MSG_PUSH_OBTAIN_BODY_TYPE, 2);
}

// ══════════════════════════ 解析 ══════════════════════════

/// 实拍那一条读数的六格都要解析出来。
#[test]
fn parses_the_captured_status_verbatim() {
    let s = parse_msg_push_status(&envelope(CAPTURED_STATUS)).unwrap();
    assert_eq!(s.push_on, Some(1));
    assert_eq!(s.on_duaration, Some(16));
    assert_eq!(s.first_task_status, Some(3));
    assert_eq!(s.second_task_status, Some(1));
    assert_eq!(s.total, Some(31));
    assert_eq!(s.push_task_status, Some(0));
}

/// ⚠⚠ **不许"顺手改对"**：上游拼错成 `onDuaration`，就必须按错的收。
///
/// 这一条用"两个拼法都给"来钉：正确拼法那个**读不到**。
/// 改成正确拼法的表现是"这个字段永远是 0"（其实是 `None`），而**不报错**。
#[test]
fn the_misspelled_duration_key_is_the_one_that_is_read() {
    assert_eq!(MSG_PUSH_DURATION_KEY, "onDuaration");

    let body = r#"{"code":0,"msg":"success","result":{
        "onDuration":99,"onDuaration":16}}"#;
    let s = parse_msg_push_status(&envelope(body)).unwrap();
    assert_eq!(
        s.on_duaration,
        Some(16),
        "读的必须是上游那个**拼错**的键，不是正确拼法"
    );
}

/// 缺 `result` **不报错** —— 判据读不出来要落成"什么都不发"那一档，
/// 而不是落成一次接口故障。
#[test]
fn a_missing_result_is_not_an_error() {
    let body = r#"{"code":0,"msg":"success"}"#;
    let s = parse_msg_push_status(&envelope(body)).unwrap();
    assert_eq!(s, MsgPushStatus::default(), "六格全缺，且不报错");
}

/// `result` 不是对象时同样不报错，六格全缺。
///
/// 缺了要落 `Unknown`（一个写请求都不发），**不能**缺省成 0 ——
/// `pushOn` 缺省成 0 等于凭空造一条"通知已关闭"的待办。
#[test]
fn a_non_object_result_yields_all_none() {
    let body = r#"{"code":0,"msg":"success","result":[]}"#;
    let s = parse_msg_push_status(&envelope(body)).unwrap();
    assert_eq!(s.push_on, None);
    assert_eq!(s.second_task_status, None);
}

/// 字符串形式的数字也收（上游有的字段是字符串）。
#[test]
fn string_form_numbers_are_accepted() {
    let body = r#"{"code":0,"msg":"success","result":{
        "pushOn":"1","secondTaskStatus":"2"}}"#;
    let s = parse_msg_push_status(&envelope(body)).unwrap();
    assert_eq!(s.push_on, Some(1));
    assert_eq!(s.second_task_status, Some(2));
}

/// 外层 `code` 非 0 ⇒ 报错（这一枪没打上）。**判据层的 Unknown 不是这个。**
#[test]
fn a_non_zero_code_is_an_error() {
    let body = r#"{"code":90001,"msg":"token 失效"}"#;
    assert!(
        parse_msg_push_status(&envelope(body)).is_err(),
        "外层非 0 是接口故障，不能落成 Unknown"
    );
}

/// `obtainCode` 在位时读出来。
#[test]
fn parses_the_obtain_code() {
    let body = r#"{"code":0,"msg":"success","result":{"obtainCode":0}}"#;
    let o = parse_msg_push_obtain(&envelope(body)).unwrap();
    assert_eq!(o.obtain_code, Some(0));
}

/// 缺 `obtainCode` **不报错**，但也不能伪造成 `Some(0)`。
///
/// 写请求已经发出去了 —— 把"判不了"伪造成"成功"，用户就再也不会去查这一笔。
#[test]
fn a_missing_obtain_code_is_none_not_zero() {
    let body = r#"{"code":0,"msg":"success","result":{}}"#;
    let o = parse_msg_push_obtain(&envelope(body)).unwrap();
    assert_eq!(o.obtain_code, None, "缺 ≠ 0；成败由判定层判");
}

/// 外层非 0 仍然是错（这一枪没打上），与"领没领到"是两件事。
#[test]
fn obtain_outer_code_is_still_an_error() {
    let body = r#"{"code":500,"msg":"系统繁忙"}"#;
    assert!(parse_msg_push_obtain(&envelope(body)).is_err());
}

/// `list` 按实拍形状解析；`name` 是**字符串**而 `num` 是数字。
#[test]
fn parses_the_captured_record_list() {
    let body = r#"{"code":0,"msg":"success","result":[
        {"name":"200","num":200,"type":2,"obtainStatus":1,
         "obtainTime":"2026年09月10日",
         "createTime":"2026-09-10T16:32:33.000+08:00"}]}"#;
    let rows = parse_msg_push_list(&envelope(body)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name.as_deref(), Some("200"));
    assert_eq!(rows[0].num, Some(200));
    assert_eq!(rows[0].type_, Some(2));
    assert_eq!(rows[0].obtain_status, Some(1));
    assert_eq!(rows[0].obtain_time.as_deref(), Some("2026年09月10日"));
}

/// `result` 不是数组 ⇒ **空列表**，不是解析失败。
///
/// 这一口是只读、且没有任何判定依赖它 —— 为它判失败只会让一次真跑白跑。
#[test]
fn a_non_array_list_result_is_empty() {
    let body = r#"{"code":0,"msg":"success","result":{}}"#;
    assert!(parse_msg_push_list(&envelope(body)).unwrap().is_empty());
}
