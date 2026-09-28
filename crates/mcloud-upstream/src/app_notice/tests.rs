//! 公告任务族的协议层测试。
//!
//! 这一组钉的是**实拍逐字**的那些细节 —— 它们全是"改错了也不报错"的类型：
//! 主机少一个 `:7071`、路径大小写"顺手改对"、裸数字的 `result` 按对象去取、
//! 缺字段被伪造成 0。

use super::*;

/// 实拍那份 `getTaskInfo`（`开启通知领云朵.json` 第 3 / 6 条，两遍逐字相同）。
const CAPTURED_INFO: &str = r#"{"code":0,"msg":"success","result":{
    "taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},
    "taskTwoInfo":{"cloudStatus":1,"progress":31,"sum":100,"taskStatus":3}}}"#;

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

/// `getTaskInfo` 是**裸 GET**（实拍 `match.key` 里没有 `?`），且主机**带 `:7071`**。
///
/// ⚠ 少一个端口、多一个查询参数都不会报错，只会让这一枪打到别处或按另一套参数理解。
#[test]
fn task_info_request_is_a_bare_get_on_the_captured_host() {
    let req = notice_task_info_request();
    assert_eq!(
        req.url,
        "https://caiyun.feixin.10086.cn:7071/ycloud/openemailsms-service/openEmailsms/getTaskInfo"
    );
    assert_eq!(req.method, "GET");
    assert!(!req.url.contains('?'), "实得：{}", req.url);
    assert!(req.body.is_none(), "裸 GET 不该带 body");
}

/// 两处大小写**逐字**：`openemailsms-service` 全小写、`openEmailsms` 驼峰。
///
/// 改对任何一处的表现都是 `404`，而 404 看起来像"接口下线了"。
#[test]
fn the_ported_host_and_the_two_casings_are_exact() {
    let req = notice_task_info_request();
    assert!(
        req.url.contains("openemailsms-service/openEmailsms/"),
        "实得：{}",
        req.url
    );
    assert!(
        !req.url.contains("openEmailsms-service"),
        "主机段是小写的，实得：{}",
        req.url
    );
    assert!(
        !req.url.contains("openemailsms/openemailsms"),
        "末段是驼峰，实得：{}",
        req.url
    );
    assert!(!req.url.contains("/market/"), "实拍走的是 /ycloud/");
}

/// `reward` 打在同一个前缀的 `/reward` 上，且是 **GET**（实拍如此 —— 尽管它是写）。
#[test]
fn reward_request_targets_the_reward_path() {
    let req = notice_reward_request();
    assert_eq!(
        req.url,
        "https://caiyun.feixin.10086.cn:7071/ycloud/openemailsms-service/openEmailsms/reward"
    );
    assert_eq!(req.method, "GET");
    assert!(req.body.is_none());
}

/// 这一族由 `h5` 统一补四个头，且**不带** `isdeviceid`。
///
/// ⚠ 实拍那五条**都带** `deviceid`，而本仓不带（只有 WebView 流量原文、
/// 没有 H5 调用点）⇒ 这里断言的是"我们发了哪四片"，不是"实拍发了哪四片"。
/// `jwttoken` / `referer` 是**会话的两半**，由 `H5Client` 注入，不在这里。
#[test]
fn the_shared_four_headers_are_present_and_no_isdeviceid() {
    let req = notice_task_info_request();
    assert_eq!(header(&req, "Accept"), Some("*/*"));
    assert_eq!(header(&req, "activityid"), Some("sign_in_3"));
    assert_eq!(header(&req, "appversion"), Some("12.6.0.0"));
    assert_eq!(header(&req, "showloading"), Some("true"));
    assert!(
        header(&req, "isdeviceid").is_none(),
        "这一族没有 isdeviceid 的实拍依据，不该凭空带上"
    );
    assert!(header(&req, "deviceid").is_none(), "也不自己补 deviceid 头");
    assert!(header(&req, "jwttoken").is_none());
    assert!(header(&req, "Referer").is_none());
}

/// `receiveTask` 把 id 放在**查询串**里（`?taskId=1021`），走 `signin/page`。
#[test]
fn receive_task_request_puts_the_id_in_the_query() {
    let req = receive_task_request(1021);
    assert_eq!(
        req.url,
        "https://caiyun.feixin.10086.cn:7071/ycloud/signin/page/receiveTask?taskId=1021"
    );
    assert_eq!(req.method, "GET");
    // 同一个函数换个 id 就换个 id，不粘住实拍那一个
    assert!(receive_task_request(7).url.ends_with("?taskId=7"));
}

/// `reportAppNoticeStatus` 的 `status` 是**调用方给的**，实拍那一条是 `1`。
#[test]
fn report_status_request_carries_the_status_it_is_given() {
    assert_eq!(
        report_app_notice_status_request(APP_NOTICE_STATUS_ON).url,
        "https://caiyun.feixin.10086.cn:7071/ycloud/signin/page/reportAppNoticeStatus?status=1"
    );
    assert!(report_app_notice_status_request(0)
        .url
        .ends_with("?status=0"));
}

/// `status=1` 是**唯一**实拍取值 —— 常量本身就是那条判据。
#[test]
fn the_only_captured_status_value_is_one() {
    assert_eq!(APP_NOTICE_STATUS_ON, 1);
}

/// 打卡那一步走的是 `signin/task/click`（**不是** `signin/page/…`），
/// 且查询串逐字是 `key=task&id=<n>`。
#[test]
fn click_request_uses_the_task_prefix_and_the_captured_key() {
    let req = signin_task_click_request(1022);
    assert_eq!(
        req.url,
        "https://caiyun.feixin.10086.cn:7071/ycloud/signin/task/click?key=task&id=1022"
    );
    assert_eq!(NOTICE_CLICK_KEY, "task");
    assert!(
        !req.url.contains("/signin/page/"),
        "同一条链上两种前缀都出现过，照抄实拍、不统一：{}",
        req.url
    );
}

// ══════════════════════════ `getTaskInfo` 解析 ══════════════════════════

/// 实拍那两条读数的四个字段都要解析出来。
#[test]
fn parses_the_captured_slots_verbatim() {
    let info = parse_notice_task_info(&envelope(CAPTURED_INFO)).unwrap();
    let one = info.task_one_info.expect("槽一应在");
    assert_eq!(one.cloud_status, Some(2));
    assert_eq!(one.sum, Some(200));
    assert_eq!(one.task_status, Some(3));
    assert_eq!(one.progress, None, "槽一实拍没有 progress");

    let two = info.task_two_info.expect("槽二应在");
    assert_eq!(two.cloud_status, Some(1));
    assert_eq!(two.sum, Some(100));
    assert_eq!(two.progress, Some(31));
    assert_eq!(two.task_status, Some(3));
}

/// ⚠ `progress` 缺席读成 `None`，**不是 0**。
///
/// "上游没给"与"上游给了 0"是两件事：混起来会让人觉得任务进度是零。
#[test]
fn a_missing_progress_is_none_not_zero() {
    let body = r#"{"code":0,"msg":"success","result":{
        "taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3}}}"#;
    let info = parse_notice_task_info(&envelope(body)).unwrap();
    assert_eq!(info.task_one_info.unwrap().progress, None);
    assert_eq!(info.task_two_info, None, "槽二整个没给");
}

/// 缺 `result` **不报错** —— 判据读不出来要落"什么都不发"那一档，
/// 而不是落成一次接口故障。
#[test]
fn a_missing_result_is_not_an_error() {
    let info = parse_notice_task_info(&envelope(r#"{"code":0,"msg":"success"}"#)).unwrap();
    assert_eq!(info, NoticeTaskInfo::default(), "两槽全缺，且不报错");
}

/// ⚠ "没给这一槽"（`None`）与"给了但四个字段全缺"（`Some(空槽)`）**不是一回事**。
///
/// 前者是上游没给，后者说明这一槽存在、读数一个都没拿到。合成一件事之后，
/// 判定层就再也说不出"这一槽在哪一步丢了"。
#[test]
fn a_missing_slot_is_none_while_an_empty_slot_is_an_empty_slot() {
    let body = r#"{"code":0,"msg":"success","result":{"taskOneInfo":{}}}"#;
    let info = parse_notice_task_info(&envelope(body)).unwrap();
    let one = info.task_one_info.expect("空对象仍然是「给了这一槽」");
    assert_eq!(one, NoticeTaskSlot::default(), "四个字段全缺");
    assert!(info.task_two_info.is_none());
}

/// 槽不是对象（数组 / 字符串 / `null`）一律按"没给这一槽"处置，不报错。
#[test]
fn a_non_object_slot_is_treated_as_not_given() {
    let body = r#"{"code":0,"msg":"success","result":{
        "taskOneInfo":[],"taskTwoInfo":"x"}}"#;
    let info = parse_notice_task_info(&envelope(body)).unwrap();
    assert!(info.task_one_info.is_none());
    assert!(info.task_two_info.is_none());
}

/// 字符串形式的数字也收 —— 上游有的字段是字符串（参考实现踩过
/// "严格相等在真实读数上永假"那一档）。
#[test]
fn string_form_numbers_are_accepted() {
    let body = r#"{"code":0,"msg":"success","result":{
        "taskOneInfo":{"cloudStatus":"2","sum":"200","taskStatus":"3"}}}"#;
    let one = parse_notice_task_info(&envelope(body))
        .unwrap()
        .task_one_info
        .unwrap();
    assert_eq!(one.cloud_status, Some(2));
    assert_eq!(one.sum, Some(200));
    assert_eq!(one.task_status, Some(3));
}

/// 外层 `code` 非 0 ⇒ 报错（这一枪没打上）。**"判不了"不是这一档。**
#[test]
fn a_non_zero_code_is_an_error() {
    let body = r#"{"code":90001,"msg":"未登录"}"#;
    assert!(
        parse_notice_task_info(&envelope(body)).is_err(),
        "外层非 0 是接口故障，不能落成「判不了」那一档"
    );
}

// ══════════════════════════ `reward` / `receiveTask` 解析 ══════════════════════════

/// 实拍那条 `reward`（`{"code":0,"msg":"success"}`）**没有 `result`**：
/// 不算失败，但也**不许**伪造成 `Some(0)`。
#[test]
fn reward_without_result_is_none_and_not_an_error() {
    let r = parse_notice_reward(&envelope(r#"{"code":0,"msg":"success"}"#)).unwrap();
    assert_eq!(r.cloud_count, None, "实拍恒 None；到账读数只能靠复查");
    // `result: null` 与"没有 result"同档
    let r = parse_notice_reward(&envelope(r#"{"code":0,"result":null}"#)).unwrap();
    assert_eq!(r.cloud_count, None);
}

/// `reward` 哪天回了**裸数字**就把那个数收下来。
///
/// ⚠ 这一档**零样本**（实拍恒无 `result`）—— 这个用例钉的是"哪天给了别丢"，
/// 不是"上游会这么回"。
#[test]
fn reward_reads_a_bare_number_result() {
    let r = parse_notice_reward(&envelope(r#"{"code":0,"result":100}"#)).unwrap();
    assert_eq!(r.cloud_count, Some(100));
}

/// 外层非 0 仍然是错（这一枪没打上），与"领没领到"是两件事。
#[test]
fn reward_outer_code_is_still_an_error() {
    assert!(parse_notice_reward(&envelope(r#"{"code":500,"msg":"系统繁忙"}"#)).is_err());
}

/// `receiveTask` 的 `result` 是**裸数字**（实拍 `100`）。
#[test]
fn receive_task_reads_a_bare_number_result() {
    let got = parse_receive_task(&envelope(r#"{"code":0,"msg":"success","result":100}"#)).unwrap();
    assert_eq!(got.cloud_count, Some(100));
    // 裸数字写成字符串也收
    let got = parse_receive_task(&envelope(r#"{"code":0,"result":"100"}"#)).unwrap();
    assert_eq!(got.cloud_count, Some(100));
}

/// ⚠ **别按对象去取**：`result` 是对象时读不出数（`None`），**不是 0**。
///
/// 按 `result.cloudCount` 去取会让这条永远报 0 且不报错，
/// 与"上游真的没发"看着一模一样。
#[test]
fn receive_task_does_not_read_a_number_out_of_an_object() {
    let got = parse_receive_task(&envelope(r#"{"code":0,"result":{"cloudCount":100}}"#)).unwrap();
    assert_eq!(got.cloud_count, None, "对象里没有裸数字");
    // 没有 result 同样是 None（不是 0）
    let got = parse_receive_task(&envelope(r#"{"code":0,"msg":"success"}"#)).unwrap();
    assert_eq!(got.cloud_count, None);
}

// ══════════════════════════ 只判 `code` 的两口 ══════════════════════════

/// `reportAppNoticeStatus` 与 `signin/task/click` 的实拍响应**都没有 `result`**
/// ⇒ 只判 `code`：非 0 是错，0 是通过（而"通过"不等于"通知已经开了"）。
#[test]
fn report_status_and_click_only_care_about_the_code() {
    let ok = envelope(r#"{"code":0,"msg":"success"}"#);
    assert!(parse_report_app_notice_status(&ok).is_ok());
    assert!(parse_signin_task_click(&ok).is_ok());

    let bad = envelope(r#"{"code":500,"msg":"系统繁忙"}"#);
    assert!(parse_report_app_notice_status(&bad).is_err());
    assert!(parse_signin_task_click(&bad).is_err());
}

/// 那份实拍读数里的 `sum` 只被**照抄**，没有任何判定依赖它 ——
/// 这个用例钉的是"别哪天顺手拿它当到账数"：`reward` 什么都不报，
/// 所以"领到多少"这句话今天**没有来源**。
#[test]
fn the_slot_sum_is_only_a_reading_not_an_amount() {
    let info = parse_notice_task_info(&envelope(CAPTURED_INFO)).unwrap();
    assert_eq!(info.task_one_info.unwrap().sum, Some(200));
    assert_eq!(info.task_two_info.unwrap().sum, Some(100));

    let reward = parse_notice_reward(&envelope(r#"{"code":0,"msg":"success"}"#)).unwrap();
    assert_eq!(
        reward.cloud_count, None,
        "到账那一口给不出数 ⇒ 不能拿 sum 顶它"
    );
}
