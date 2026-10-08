//! push 模块的测试。
//!
//! 分层：`build_*`（纯构造）拿 URL / body 做 golden 断言；
//! `send_*` / `send_notify` 用假传输层断言「发了几枪、发到哪、
//! 一个渠道失败有没有影响别的渠道」。

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use mcloud_core::testkit::FixedClock;
use serde_json::json;

/// 假传输层排队的一条应答。
#[derive(Debug, Clone)]
enum Reply {
    /// 正常回一个 HTTP 状态与响应体。
    Http(u16, &'static str),
    /// 传输层直接报错（超时 / 连不上那类）。
    Failed(&'static str),
}

/// 记录每一枪的假传输层。
#[derive(Debug, Default)]
struct Fake {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<PushRequest>>,
}

impl Fake {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// 已发出的请求（按顺序）。
    fn seen(&self) -> Vec<PushRequest> {
        self.seen.lock().expect("测试里不该中毒").clone()
    }

    /// 发了几枪。
    fn calls(&self) -> usize {
        self.seen.lock().expect("测试里不该中毒").len()
    }
}

#[async_trait::async_trait]
impl PushTransport for Fake {
    async fn send(&self, req: PushRequest) -> anyhow::Result<PushResponse> {
        self.seen.lock().expect("测试里不该中毒").push(req);
        let reply = self
            .replies
            .lock()
            .expect("测试里不该中毒")
            .pop_front()
            .unwrap_or(Reply::Http(200, "{}"));
        match reply {
            Reply::Http(status, body) => Ok(PushResponse {
                status,
                body: body.to_string(),
            }),
            Reply::Failed(message) => Err(anyhow::anyhow!(message)),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(raw: &str) -> T {
    serde_json::from_str(raw).expect("测试配置必须能解析")
}

fn context<'a>(
    http: &'a Fake,
    clock: &'a FixedClock,
    logs: &'a PushLogCollector,
) -> PushContext<'a> {
    PushContext::new(http, clock, logs)
}

/// 2023-11-15 06:13:20（东八区），做钉钉签名的固定时间戳。
const VECTOR_TS: i64 = 1_700_000_000_000;

// ==================== a) 各渠道的纯构造 ====================

#[test]
fn pushplus_url_body_and_timeout_match_ts() {
    let config: TokenConfig = parse(r#"{"token":"tk"}"#);
    let request = build_pushplus(&config, "标题", "正文");
    assert_eq!(
        request.url, "http://www.pushplus.plus/send",
        "TS 用的就是 http，不是 https"
    );
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.timeout_ms, REQUEST_TIMEOUT_MS,
        "统一投递必须真的带 10s"
    );
    assert_eq!(
        request.body,
        Some(json!({"token": "tk", "title": "标题", "content": "正文"}))
    );
    assert_eq!(
        request.headers,
        vec![("Content-Type".to_string(), "application/json".to_string())]
    );
}

#[test]
fn pushplus_extra_fields_spread_last_like_ts() {
    // TS 是 { token, title, content, ...config }：config 里的同名键在最后，能盖掉前面的。
    let config: TokenConfig = parse(r#"{"token":"tk","title":"被配置盖掉的标题"}"#);
    let request = build_pushplus(&config, "标题", "正文");
    assert_eq!(
        request.body,
        Some(json!({"token": "tk", "title": "被配置盖掉的标题", "content": "正文"})),
        "展开必须发生在算出来的字段之后"
    );
}

#[test]
fn server_chan_doubles_newlines_like_ts() {
    let config: TokenConfig = parse(r#"{"token":"SCT9"}"#);
    let request = build_server_chan(&config, "T", "a\nb");
    assert_eq!(request.url, "https://sctapi.ftqq.com/SCT9.send");
    assert_eq!(
        request.body,
        Some(json!({"text": "T", "title": "T", "desp": "a\n\nb"})),
        "desp 里每个换行要翻倍"
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("content-type")
                && value == "application/json"),
        "渠道自带的 content-type 头必须在：{:?}",
        request.headers
    );
}

#[test]
fn work_weixin_token_and_message_match_ts() {
    let config: WorkWeixinConfig = parse(r#"{"corpid":"corp","corpsecret":"sec","agentid":1001}"#);
    let token_request = build_work_weixin_token(&config);
    assert_eq!(
        token_request.url,
        "https://qyapi.weixin.qq.com/cgi-bin/gettoken"
    );
    assert_eq!(
        token_request.body,
        Some(json!({"corpid": "corp", "corpsecret": "sec"}))
    );

    let message = build_work_weixin_message(&config, "AT", "T", "B");
    assert_eq!(
        message.url,
        "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token=AT"
    );
    assert_eq!(
        message.body,
        Some(json!({
            "touser": "@all",
            "msgtype": "text",
            "agentid": 1001,
            "text": {"content": "T\n\nB"},
        })),
        "touser 默认 @all、动态类型键是 text"
    );
}

#[test]
fn work_weixin_uses_msgtype_as_dynamic_key() {
    let config: WorkWeixinConfig = parse(r#"{"msgtype":"markdown","touser":"u1"}"#);
    let message = build_work_weixin_message(&config, "AT", "T", "B");
    assert_eq!(
        message.body,
        Some(json!({
            "touser": "u1",
            "msgtype": "markdown",
            "markdown": {"content": "T\n\nB"},
        }))
    );
}

#[test]
fn work_weixin_bot_posts_to_configured_url() {
    let config: WorkWeixinBotConfig =
        parse(r#"{"url":"https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=k"}"#);
    let request = build_work_weixin_bot(&config, "T", "B");
    assert_eq!(
        request.url,
        "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=k"
    );
    assert_eq!(
        request.body,
        Some(json!({"msgtype": "text", "text": {"content": "T\n\nB"}}))
    );
}

#[test]
fn tg_bot_url_and_body_match_ts() {
    let config: TgBotConfig = parse(r#"{"token":"123:abc","chat_id":42}"#);
    let request = build_tg_bot(&config, "T", "B").expect("token 与 chat_id 都在，不该跳过");
    assert_eq!(
        request.url,
        "https://api.telegram.org/bot123:abc/sendMessage"
    );
    assert_eq!(
        request.body,
        Some(json!({
            "disable_web_page_preview": true,
            "chat_id": 42,
            "text": "T\n\nB",
        })),
        "chat_id 是请求体字段，text 收尾"
    );
}

#[test]
fn tg_bot_skips_without_token_or_chat_id() {
    let no_chat: TgBotConfig = parse(r#"{"token":"123:abc"}"#);
    assert!(
        build_tg_bot(&no_chat, "T", "B").is_none(),
        "缺 chat_id 跳过"
    );
    let zero_chat: TgBotConfig = parse(r#"{"token":"123:abc","chat_id":0}"#);
    assert!(
        build_tg_bot(&zero_chat, "T", "B").is_none(),
        "chat_id=0 在 JS 里是假值，TS 会跳过"
    );
    let empty_token: TgBotConfig = parse(r#"{"token":"","chat_id":1}"#);
    assert!(
        build_tg_bot(&empty_token, "T", "B").is_none(),
        "空 token 跳过"
    );
}

#[test]
fn bark_url_and_body_match_ts() {
    let config: BarkConfig = parse(r#"{"key":"bk1"}"#);
    let request = build_bark(&config, "T", "B").expect("key 在，不该跳过");
    assert_eq!(request.url, "https://api.day.app/bk1");
    assert_eq!(
        request.body,
        Some(json!({"body": "B", "title": "T", "level": "passive"}))
    );
    let empty: BarkConfig = parse(r#"{"key":""}"#);
    assert!(build_bark(&empty, "T", "B").is_none(), "缺 key 必须跳过");
}

#[test]
fn two_im_url_and_body_match_ts() {
    let config: TwoImConfig =
        parse(r#"{"key":"a b","sid":"c&d","query":{"plat":"2","group":"g"}}"#);
    let request = build_two_im(&config, "T", "B");
    assert_eq!(
        request.url, "https://api.2im.cn/push?key=a+b&sid=c%26d&plat=2&group=g",
        "URLSearchParams 口径：空格是 +，& 要转义"
    );
    assert_eq!(
        request.body,
        Some(json!({"msgtype": "text", "content": "B", "title": "T"})),
        "key / sid / query 不进请求体"
    );
}

#[test]
fn uri_encoding_matches_the_two_js_dialects() {
    assert_eq!(
        encode_uri_component("a b+c/d=e~f!g"),
        "a%20b%2Bc%2Fd%3De~f!g",
        "encodeURIComponent 保留 ~ !，空格是 %20"
    );
    assert_eq!(
        form_urlencode("a b+c/d=e~f!g"),
        "a+b%2Bc%2Fd%3De%7Ef%21g",
        "URLSearchParams 把空格编成 +，~ ! 要转义"
    );
}

#[test]
fn ding_talk_signature_matches_fixed_vector() {
    // 期望值来自独立的 Python hmac + base64（密钥 mcloud-secret，消息
    // "1700000000000 换行 mcloud-secret"），不是本实现自证。
    assert_eq!(
        hmac_sha256_base64(
            b"mcloud-secret",
            format!("{VECTOR_TS}\nmcloud-secret").as_bytes()
        ),
        "JLu4GHUbl3ks/Mql/8bdwtNvrq6VIWJcqV987VhOV1M="
    );
    let config: DingTalkConfig = parse(r#"{"token":"tk","secret":"mcloud-secret"}"#);
    let request = build_ding_talk(&config, "T", "B", VECTOR_TS);
    assert_eq!(
        request.url,
        "https://oapi.dingtalk.com/robot/send?access_token=tk&timestamp=1700000000000&sign=JLu4GHUbl3ks%2FMql%2F8bdwtNvrq6VIWJcqV987VhOV1M%3D",
        "签名按 encodeURIComponent 转义后拼在查询串末尾"
    );
    assert_eq!(
        request.body,
        Some(json!({"msgtype": "text", "text": {"content": " T\n\nB"}})),
        "content 前的那个前导空格是 TS 原文"
    );
}

#[test]
fn ding_talk_without_secret_has_no_signature() {
    let config: DingTalkConfig = parse(r#"{"token":"tk"}"#);
    assert_eq!(
        build_ding_talk(&config, "T", "B", VECTOR_TS).url,
        "https://oapi.dingtalk.com/robot/send?access_token=tk"
    );
}

#[test]
fn custom_post_replaces_placeholders_like_ts() {
    let api: PushApiData = parse(
        r#"{"url":"https://x.test/h?t={title}","method":"post","data":{"title":"{title}","text":"{text}"}}"#,
    );
    let request = build_custom_post(&api, "T X", "B\nC")
        .expect("模板体替换后仍是合法 JSON")
        .expect("URL 非空，不该跳过");
    assert_eq!(request.method, "POST", "method 要 toUpperCase");
    assert_eq!(request.url, "https://x.test/h?t=T%20X");
    assert_eq!(
        request.body,
        Some(json!({"title": "T X", "text": "B\nC"})),
        "请求体里的占位符不转义，真实换行靠转义保住"
    );
}

#[test]
fn custom_post_url_replaces_only_the_first_placeholder() {
    // TS 的 String.replace 不带 /g：URL 里出现两次 {title} 时只换第一次。
    let api: PushApiData = parse(r#"{"url":"https://x.test/{title}/{title}"}"#);
    let request = build_custom_post(&api, "A", "B")
        .expect("没有体，不会解析失败")
        .expect("URL 非空");
    assert_eq!(
        request.url, "https://x.test/A/{title}",
        "第二个占位符保持原样"
    );
}

#[test]
fn custom_post_data_wins_over_body_and_empty_data_means_no_body() {
    let both: PushApiData = parse(r#"{"url":"https://x.test","data":{},"body":{"a":1}}"#);
    let request = build_custom_post(&both, "T", "B")
        .expect("空对象好解析")
        .expect("URL 非空");
    assert_eq!(
        request.body, None,
        "空 data 在 JS 里也是真值，会盖掉 body，然后因为键数为 0 而不带体"
    );

    let only_body: PushApiData = parse(r#"{"url":"https://x.test","body":{"a":1}}"#);
    assert_eq!(
        build_custom_post(&only_body, "T", "B")
            .expect("好解析")
            .expect("URL 非空")
            .body,
        Some(json!({"a": 1})),
        "data 缺席时 body 顶上"
    );

    let no_url: PushApiData = parse(r#"{"url":""}"#);
    assert!(
        build_custom_post(&no_url, "T", "B")
            .expect("好解析")
            .is_none(),
        "没有 URL 的模板跳过"
    );
}

#[test]
fn custom_post_honours_its_own_method_headers_and_timeout() {
    let api: PushApiData =
        parse(r#"{"url":"https://x.test","method":"get","headers":{"X-A":"1"},"timeout":3000}"#);
    let request = build_custom_post(&api, "T", "B")
        .expect("好解析")
        .expect("URL 非空");
    assert_eq!(request.method, "GET");
    assert_eq!(request.timeout_ms, 3000, "模板自己的 timeout 要生效");
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "X-A" && value == "1"));
    assert!(
        request
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("content-type")),
        "没有指定 content-type 时补默认值：{:?}",
        request.headers
    );
}

// ==================== b) minLevel 过滤 ====================

#[test]
fn level_map_matches_ts_transport() {
    assert_eq!(level_of_type("error"), 0);
    assert_eq!(level_of_type("fail"), 0);
    assert_eq!(level_of_type("fatal"), 0);
    assert_eq!(level_of_type("warn"), 1);
    assert_eq!(level_of_type("info"), 3);
    assert_eq!(level_of_type("success"), 3);
    assert_eq!(level_of_type("start"), 3);
    assert_eq!(level_of_type("debug"), 4);
    assert_eq!(level_of_type("别的"), 3, "未知类型照 TS 的兜底值 3");
}

#[test]
fn collector_records_level_type_message_and_date() {
    let logs = PushLogCollector::new();
    logs.write("info", "test", 0);
    logs.write("debug", "debug", 0);
    let entries = logs.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].level, 3, "info level=3");
    assert_eq!(entries[1].level, 4, "debug level=4");
    assert_eq!(entries[0].r#type, "info");
    assert_eq!(entries[0].msg, "test");
    assert_eq!(entries[0].date, 0);
    logs.clear();
    assert!(logs.is_empty(), "clear 后为空");
}

#[test]
fn min_level_filters_debug_but_keeps_error() {
    let logs = PushLogCollector::new();
    logs.write("debug", "d", 0);
    logs.write("info", "i", 0);
    logs.write("warn", "w", 0);
    logs.write("error", "e", 0);
    let entries = logs.entries();

    let info = format_logs_for_push(&entries, PushLogLevel::Info);
    assert_eq!(
        info, "[info 08:00:00]i\n[warn 08:00:00]w\n[error 08:00:00]e",
        "info 级不推 debug 行"
    );
    assert!(info.contains("error"), "error 行必须在");
    assert_eq!(
        format_logs_for_push(&entries, PushLogLevel::Error),
        "[error 08:00:00]e",
        "error 级只剩最严重的一行"
    );
    assert_eq!(
        format_logs_for_push(&entries, PushLogLevel::Debug)
            .lines()
            .count(),
        4,
        "debug 级全留"
    );

    let only_info = vec![PushLogEntry {
        level: 3,
        r#type: "info".to_string(),
        msg: "i".to_string(),
        date: 0,
    }];
    assert_eq!(
        format_logs_for_push(&only_info, PushLogLevel::Error),
        "",
        "过滤完没有行就是空串（不产生空行）"
    );
}

#[test]
fn local_time_hms_agrees_with_beijing_digits() {
    for at_ms in [0_i64, VECTOR_TS, VECTOR_TS + 3_999, -1_000] {
        let digits = mcloud_core::beijing_time_digits(&FixedClock(at_ms));
        let want = format!("{}:{}:{}", &digits[8..10], &digits[10..12], &digits[12..14]);
        assert_eq!(local_time_hms(at_ms), want, "at_ms={at_ms}");
    }
    assert_eq!(local_time_hms(0), "08:00:00", "epoch 0 在东八区是早上 8 点");
}

// ==================== c) onlyError ====================

#[test]
fn only_error_skips_until_a_log_reaches_the_error_level() {
    let config: PushConfig = parse(r#"{"onlyError":true}"#);
    let logs = PushLogCollector::new();
    assert!(config.push_text(&logs).is_none(), "没有日志不推");

    logs.write("info", "签到成功", 0);
    assert!(
        config.push_text(&logs).is_none(),
        "只有 info 时 onlyError 不推"
    );

    // ⚠ 2026-10-08 用户口径：判据是**严重级别**，不是 `type` 恰好等于 `"error"` ——
    // `fail` / `fatal` 与 `error` 同为 level 0 ⇒ 都算 ✓、都要推 ✓。
    // （TS 的 `test-push-filter.ts` 有一条「onlyError: fail 不算 error」钉着旧口径，
    //   那条会把最严重的两条吞掉 ✗ —— 本仓**故意不照抄** ✓。）
    logs.write("fail", "任务失败", 0);
    assert!(
        config.push_text(&logs).is_some(),
        "fail 也是最严重那一档 ⇒ onlyError 要推"
    );

    logs.write("error", "网络错误", 0);
    assert!(config.push_text(&logs).is_some(), "有 error 当然推");
}

#[test]
fn without_only_error_info_logs_are_pushed() {
    let config: PushConfig = parse(r#"{"minLevel":"info"}"#);
    let logs = PushLogCollector::new();
    logs.write("info", "i", 0);
    assert_eq!(config.push_text(&logs).as_deref(), Some("[info 08:00:00]i"));
}

#[tokio::test]
async fn only_error_gates_send_notify_end_to_end() {
    let config: PushConfig = parse(r#"{"onlyError":true,"pushplus":{"token":"tk"}}"#);
    let logs = PushLogCollector::new();
    logs.write("info", "签到成功", 0);
    let fake = Fake::new(vec![]);
    let ctx = context(&fake, &FixedClock(0), &logs);

    if let Some(text) = config.push_text(&logs) {
        send_notify(&ctx, &config, &config.title, &text).await;
    }
    assert_eq!(fake.calls(), 0, "全是 info 时一条都不发");

    logs.write("error", "炸了", 0);
    if let Some(text) = config.push_text(&logs) {
        send_notify(&ctx, &config, &config.title, &text).await;
    }
    assert_eq!(fake.calls(), 1, "出现 error 后要发");
    assert_eq!(
        fake.seen()[0].body,
        Some(json!({
            "token": "tk",
            "title": "mcloud 运行推送",
            "content": "[info 08:00:00]签到成功\n[error 08:00:00]炸了",
        })),
        "正文按 minLevel=info 过滤后逐行拼接"
    );
}

// ==================== d) 失败不冒泡 ====================

#[test]
fn response_judgement_matches_ts() {
    assert!(!response_is_failure(&json!({})));
    assert!(!response_is_failure(&json!({"errcode": 0})));
    assert!(!response_is_failure(&json!({"code": 0})));
    assert!(!response_is_failure(&json!({"code": 200})));
    assert!(!response_is_failure(&json!({"code": null})));
    assert!(!response_is_failure(&json!([])), "不是对象就当没这回事");
    assert!(!response_is_failure(&json!("ok")));

    assert!(response_is_failure(&json!({"errcode": 40013})));
    assert!(response_is_failure(&json!({"err": "boom"})));
    assert!(response_is_failure(&json!({"code": 1})));
    assert!(
        response_is_failure(&json!({"code": "0"})),
        "JS 是严格相等：字符串形态的 0 不放行"
    );
    assert!(response_is_failure(&json!({"code": "200"})));
    assert!(
        response_is_failure(&json!({"err": "0"})),
        "非空字符串在 JS 里是真值"
    );
}

#[tokio::test]
async fn one_channel_failure_does_not_stop_the_others() {
    let fake = Fake::new(vec![Reply::Http(500, "boom"), Reply::Failed("timed out")]);
    let config: PushConfig =
        parse(r#"{"pushplus":{"token":"a"},"serverChan":{"token":"b"},"bark":{"key":"c"}}"#);
    let logs = PushLogCollector::new();
    send_notify(&context(&fake, &FixedClock(0), &logs), &config, "T", "B").await;

    assert_eq!(fake.calls(), 3, "前两个渠道失败，第三个照发");
    assert_eq!(fake.seen()[0].url, "http://www.pushplus.plus/send");
    assert_eq!(fake.seen()[2].url, "https://api.day.app/c");

    let entries = logs.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry.level == 1 && entry.r#type == "warn"),
        "500 与超时都要留下 warn 日志：{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.msg.contains("Bark ios 推送已发送")),
        "成功的那个渠道仍要留一条 info：{entries:?}"
    );
}

#[tokio::test]
async fn errcode_response_is_logged_as_error_and_does_not_bubble() {
    let fake = Fake::new(vec![Reply::Http(
        200,
        r#"{"errcode":40013,"errmsg":"invalid appid"}"#,
    )]);
    let config: PushConfig = parse(r#"{"pushplus":{"token":"a"}}"#);
    let logs = PushLogCollector::new();
    send_notify(&context(&fake, &FixedClock(0), &logs), &config, "T", "B").await;

    let entries = logs.entries();
    assert!(
        entries
            .iter()
            .any(|entry| entry.r#type == "error" && entry.msg.contains("pushplus发送失败")),
        "errcode 类失败记 error：{entries:?}"
    );
}

#[tokio::test]
async fn work_weixin_skips_when_token_exchange_yields_no_token() {
    let fake = Fake::new(vec![Reply::Http(200, r#"{"errcode":40001}"#)]);
    let config: PushConfig = parse(r#"{"workWeixin":{"corpid":"c","corpsecret":"s"}}"#);
    let logs = PushLogCollector::new();
    send_notify(&context(&fake, &FixedClock(0), &logs), &config, "T", "B").await;

    assert_eq!(
        fake.calls(),
        1,
        "换 token 一枪，缺 access_token 就不再打第二枪"
    );
    assert!(
        logs.has_error(),
        "缺 access_token 必须可观测：{:?}",
        logs.entries()
    );
}

#[tokio::test]
async fn failure_logs_never_leak_credential_urls() {
    let fake = Fake::new(vec![Reply::Failed(
        "error sending request for url (https://api.day.app/SECRETKEY)",
    )]);
    let config: PushConfig = parse(r#"{"bark":{"key":"SECRETKEY"}}"#);
    let logs = PushLogCollector::new();
    send_notify(&context(&fake, &FixedClock(0), &logs), &config, "T", "B").await;

    let all: String = logs
        .entries()
        .iter()
        .map(|entry| entry.msg.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !all.contains("SECRETKEY"),
        "Bark 的 key 在路径里，redact 盖不住，必须靠抹 URL：{all}"
    );
    assert!(!all.contains("https"), "日志里不该出现完整 URL：{all}");
    assert!(all.contains("<url>"), "URL 应被替换成占位符：{all}");
}

// ==================== e) 只遍历已配置渠道 ====================

#[tokio::test]
async fn send_notify_only_touches_configured_channels() {
    let fake = Fake::new(vec![]);
    let logs = PushLogCollector::new();
    let ctx = context(&fake, &FixedClock(0), &logs);

    send_notify(&ctx, &parse::<PushConfig>("{}"), "T", "B").await;
    assert_eq!(fake.calls(), 0, "什么都没配 = 一枪都不发");

    let only_pushplus: PushConfig = parse(r#"{"pushplus":{"token":"tk"}}"#);
    send_notify(&ctx, &only_pushplus, "T", "B").await;
    assert_eq!(fake.calls(), 1, "只配了 pushplus 就只发一枪");
    assert_eq!(fake.seen()[0].url, "http://www.pushplus.plus/send");
}

#[tokio::test]
async fn custom_post_array_sends_each_template() {
    let config: PushConfig =
        parse(r#"{"customPost":[{"url":"https://a.test"},{"url":"https://b.test"}]}"#);
    let fake = Fake::new(vec![]);
    let logs = PushLogCollector::new();
    send_notify(&context(&fake, &FixedClock(0), &logs), &config, "T", "B").await;

    assert_eq!(fake.calls(), 2, "数组里每个模板都要打一枪");
    assert_eq!(fake.seen()[0].url, "https://a.test");
    assert_eq!(fake.seen()[1].url, "https://b.test");
}

// ==================== email：配了也不许假装成功 ====================

#[tokio::test]
async fn email_is_not_implemented_and_never_pretends_success() {
    let fake = Fake::new(vec![]);
    let logs = PushLogCollector::new();
    let config: PushConfig =
        parse(r#"{"email":{"pass":"p","from":"a@b.c","host":"smtp.example","to":"x@y.z"}}"#);
    let ctx = context(&fake, &FixedClock(0), &logs);

    let outcome = send_email(&ctx, config.email.as_ref().expect("配了 email"), "T", "B").await;
    assert_eq!(
        outcome,
        ChannelOutcome::NotImplemented,
        "email 不许假装成功"
    );
    assert_eq!(fake.calls(), 0, "email 一枪都不发（没有 SMTP 客户端）");

    let entries = logs.entries();
    assert!(
        entries.iter().any(|entry| entry.msg.contains("未实现")),
        "未实现必须可观测：{entries:?}"
    );
    assert!(
        entries.iter().all(|entry| entry.level == 1),
        "记的是 warn：{entries:?}"
    );

    logs.clear();
    send_notify(&ctx, &config, "T", "B").await;
    assert_eq!(fake.calls(), 0, "走 send_notify 也一样不发");
    assert!(
        logs.entries()
            .iter()
            .any(|entry| entry.r#type == "warn" && entry.msg.contains("未实现")),
        "走 send_notify 也要留痕：{:?}",
        logs.entries()
    );
}

// ==================== 配置形态 ====================

#[test]
fn push_config_uses_camel_case_and_ts_field_names() {
    let config: PushConfig = parse(
        r#"{"title":"自定义","onlyError":true,"minLevel":"warn",
            "pushplus":{"token":"p"},"serverChan":{"token":"s"},
            "workWeixin":{"corpid":"c","corpsecret":"sec"},
            "workWeixinBot":{"url":"https://x"},"tgBot":{"token":"t","chat_id":"1"},
            "bark":{"key":"k"},"twoIm":{"key":"k","sid":"s"},"dingTalk":{"token":"d"},
            "email":{"pass":"p","from":"f@x","host":"h"},
            "customPost":{"url":"https://y"}}"#,
    );
    assert_eq!(config.title, "自定义");
    assert!(config.only_error);
    assert_eq!(config.min_level, PushLogLevel::Warn);
    assert!(matches!(config.custom_post, Some(CustomPost::Single(_))));
    assert!(config.pushplus.is_some());
    assert!(config.server_chan.is_some());
    assert!(config.work_weixin.is_some());
    assert!(config.work_weixin_bot.is_some());
    assert!(config.tg_bot.is_some());
    assert!(config.bark.is_some());
    assert!(config.two_im.is_some());
    assert!(config.ding_talk.is_some());
    assert!(config.email.is_some());

    let default: PushConfig = parse("{}");
    assert_eq!(default.title, DEFAULT_TITLE, "默认标题");
    assert!(!default.only_error);
    assert_eq!(
        default.min_level,
        PushLogLevel::Info,
        "默认 minLevel 是 info"
    );
    assert!(default.pushplus.is_none() && default.custom_post.is_none());
}

#[test]
fn email_config_carries_the_ts_fields() {
    let config: EmailConfig =
        parse(r#"{"pass":"p","from":"a@b.c","host":"smtp.example","port":465}"#);
    assert_eq!(config.port, Some(465));
    assert!(config.to.is_none());
}

// ══════════════ 推送地址的闸（复审 sv-8）══════════════

/// ⚠⚠ 复审 `sv-8`：**公网 http(s) 放行，本机/内网一律拒**。
///
/// 这条闸挡的是"有权改账号设置的人把服务当内网探针"（盲 SSRF）：推送地址整条来自
/// 用户配置，而发请求的是服务进程。
///
/// 红测：把 `validate_push_url` 改回"只判空"⇒ 拒的那一组全红。
#[test]
fn push_urls_that_point_inside_are_refused() {
    // 该放行的：真渠道的地址（企业微信机器人 / TG / serverchan 这些形状）
    for ok in [
        "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc",
        "https://api.telegram.org/bot123:ABC/sendMessage",
        "https://sctapi.ftqq.com/SCT123.send?title=x",
        "http://8.8.8.8/hook", // 公网 IP 直连也是合法的
        "https://push.example.com:8443/x",
        "http://172.32.0.1/x", // ⚠ 172.16/12 **之外**，别把整段 172 都拒了
    ] {
        assert!(
            validate_push_url(ok).is_ok(),
            "这条本该放行：{ok} —— {:?}",
            validate_push_url(ok)
        );
    }

    // 该拒的：本机 / 私网 / 链路本地（含云元数据）/ 别的 scheme
    for bad in [
        "file:///etc/passwd",
        "gopher://127.0.0.1:6379/_x",
        "ftp://example.com/x",
        "http://127.0.0.1:3000/api/system/restart",
        "http://127.1.2.3/x",
        "http://localhost:8080/hook",
        "http://nas.local/hook",
        "http://foo.internal/hook",
        "http://169.254.169.254/latest/meta-data/", // 云元数据
        "http://10.1.2.3/x",
        "http://192.168.1.10/hook",
        "http://172.16.0.1/x",
        "http://172.31.255.254/x",
        "http://0.0.0.0/x",
        "http://100.64.0.1/x", // CGNAT
        "http://224.0.0.1/x",
        "http://[::1]/x",
        "http://[fd00::1]/x",
        "http://[fe80::1]/x",
        "//example.com/x", // 没有 scheme
        "example.com/hook",
    ] {
        assert!(validate_push_url(bad).is_err(), "这条本该被拒：{bad}");
    }
}

/// ⚠⚠ 2026-10-06 安全审计 `SEC-04`：**同一把尺子**才能挡住这几条。
///
/// 旧闸是"自己切字符串 + `host.parse::<IpAddr>()`"，而 reqwest 走 WHATWG 解析
/// （`url` crate）—— 下面这些写法在 WHATWG 里**全是 `127.0.0.1` / 内网地址**，
/// 但在"自己切字符串"眼里只是几个**不像 IP 的域名** ⇒ 旧闸放行 ✗（实测过）。
///
/// 红测：把 `validate_push_url` 改回按 `://` 切字符串 + `parse::<IpAddr>()`
/// ⇒ 这一整组全红（尤其前四条与两条 `::ffff:`）。
#[test]
fn whatwg_ipv4_spellings_cannot_smuggle_a_private_host_past_the_gate() {
    for bad in [
        // WHATWG 的 IPv4 数字解析：十进制 / 十六进制 / 八进制 / 末尾点
        //（`url-2.5.8/src/host.rs:297-361`）—— 都会被归一化成 127.0.0.1
        "http://2130706433/",
        "http://0x7f000001/",
        "http://017700000001/",
        "http://127.0.0.1./",
        "http://2130706433:3000/api/system/restart",
        // 云元数据 / 私网的同一批写法
        "http://2852039166/", // 169.254.169.254 的十进制
        "http://0xa9fea9fe/", // 同上，十六进制
        "http://3232235777/", // 192.168.1.1 的十进制
        // IPv4-mapped IPv6：`is_loopback()` 这些判据一条都不认它
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:169.254.169.254]/",
        "http://[::ffff:10.0.0.1]/",
        "http://[::ffff:192.168.1.1]/",
        // 名字的末尾点（DNS 里与不带点等价）
        "http://localhost./hook",
        "http://nas.local./hook",
        "http://foo.internal./hook",
    ] {
        assert!(
            validate_push_url(bad).is_err(),
            "这条本该被拒（WHATWG 归一化后指向本机/内网）：{bad}"
        );
    }

    // 反向：公网地址不能被这一轮收紧误伤（尤其"末尾点"不该把正常域名一起拒掉）
    for ok in [
        "https://push.example.com./hook",
        "http://8.8.8.8/hook",
        "http://172.32.0.1/x", // 172.16/12 之外
    ] {
        assert!(
            validate_push_url(ok).is_ok(),
            "这条本该放行：{ok} —— {:?}",
            validate_push_url(ok)
        );
    }
}
