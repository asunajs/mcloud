//! `ai_cloud` 协议层的测试。
//!
//! 三条最要紧：**两套鉴权不能混**（585 用裸 token、1028 用邮箱派生的 Basic 串）、
//! **公钥剥壳只去头尾行**、**SSE 的两种读法各自逐字**。

use super::*;
use mcloud_crypto::SeqRandom;

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// 585 识图那一枪：Authorization 是**裸 token**，且带 `x-yun-uni`。
#[test]
fn ai_recognize_uses_the_raw_token_not_basic() {
    let req = ai_recognize_request(
        "RAWTOKEN",
        "1234567890123456789",
        "AAAA",
        "tid1234",
        "DEVID",
    );
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, AI_RECOGNIZE_URL);
    assert_eq!(header(&req, "Authorization"), Some("RAWTOKEN"));
    assert_eq!(header(&req, "x-yun-uni"), Some("1234567890123456789"));
    assert_eq!(header(&req, "x-yun-tid"), Some("tid1234"));
    // ⚠ `deviceid` 必须带上（旧 TS 的 http 层按域名注入；少了它上游回「鉴权失败」）
    assert_eq!(header(&req, "deviceid"), Some("DEVID"));
    assert_eq!(header(&req, "accept"), Some("application/json"));
    // ⚠⚠ 这一枪**不带** `x-yun-client-info`（旧 TS `useClientInfo` 缺省 false）——
    // 2026-10-07 真机踩过：多带它 ⇒ 上游回 `鉴权失败`（那句话会把人带去查 token ✗）。
    assert_eq!(
        header(&req, "x-yun-client-info"),
        None,
        "识图那一枪不带 client-info（只有对话那一枪带）"
    );
    let body = req.body_str().unwrap();
    assert!(body.contains("\"uploadToCloud\":true"), "{body}");
    assert!(body.contains("\"recognizeType\":\"1\""), "{body}");
    assert!(body.contains("\"timeout\":30000"), "{body}");
}

/// 识图响应解析：成功 + fileId + taskId（数字或字符串都收）。
#[test]
fn parse_ai_recognize_reads_the_three_fields() {
    let r = parse_ai_recognize("{\"success\":true,\"data\":{\"fileId\":\"F1\",\"taskId\":123}}")
        .unwrap();
    assert!(r.success);
    assert_eq!(r.file_id.as_deref(), Some("F1"));
    assert_eq!(r.task_id.as_deref(), Some("123"));

    let r = parse_ai_recognize("{\"success\":false,\"message\":\"no\"}").unwrap();
    assert!(!r.success);
    assert_eq!(r.message.as_deref(), Some("no"));
    assert!(parse_ai_recognize("not json").is_err());
}

/// 585 的对话头用的是**完整 auth**（带 `Basic ` 那一串）—— 与识图那支刻意不同。
#[test]
fn ai_camera_chat_uses_the_full_auth_string() {
    let payload = ai_camera_chat_payload("UID", "F1", "124.jpeg", "2026-10-07T02:45:30.000Z");
    let req = ai_camera_chat_request("Basic cGM6...", &payload, "tid");
    assert_eq!(header(&req, "Authorization"), Some("Basic cGM6..."));
    assert_eq!(header(&req, "Accept"), Some("text/event-stream"));
    let body = req.body_str().unwrap();
    assert!(body.contains("\"name\":\"124.jpeg\""), "{body}");
    assert!(body.contains("\"dialogue\":\"？\""), "{body}");
}

/// 585 的成败判据（旧 TS `isAiChatSuccess` 逐字三支）。
#[test]
fn is_ai_chat_success_matches_the_legacy_three_branches() {
    assert!(is_ai_chat_success(&serde_json::json!("ok")));
    assert!(!is_ai_chat_success(&serde_json::json!("   ")));
    assert!(is_ai_chat_success(&serde_json::json!({"success": true})));
    assert!(is_ai_chat_success(&serde_json::json!({"code": "0000"})));
    assert!(is_ai_chat_success(&serde_json::json!({"code": 0})));
    assert!(!is_ai_chat_success(&serde_json::json!({"code": 500})));
    assert!(!is_ai_chat_success(&serde_json::json!(null)));
}

/// 585 取**最后一条** `data:` 行；没有 `data:` 行时退化成整份 JSON 解析。
#[test]
fn parse_last_sse_data_takes_the_last_line_then_falls_back() {
    let body = "data: {\"code\":1}\n\ndata: {\"code\":0,\"success\":true}\n";
    assert_eq!(parse_last_sse_data(body)["code"], serde_json::json!(0));

    let plain = "{\"success\":true}";
    assert_eq!(
        parse_last_sse_data(plain)["success"],
        serde_json::json!(true)
    );

    let junk = "whatever";
    assert_eq!(
        parse_last_sse_data(junk)["raw"],
        serde_json::json!("whatever")
    );
}

/// 公钥剥壳：只剩 base64 那一行，长度对得上。
#[test]
fn pubkey_is_stripped_to_bare_base64() {
    let b64 = cloud_task_status_pubkey_b64();
    assert!(!b64.contains("-----"));
    assert!(!b64.contains('\n'));
    assert!(b64.starts_with("MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A"));
    assert!(b64.ends_with("TwIDAQAB"));
    // 它是能用的：加密出来是一串 base64（RSA-2048 ⇒ 256 字节 ⇒ 344 字符）
    let ct = encrypt_phone_for_task_status("13900000000", &mut SeqRandom::new(7)).unwrap();
    assert_eq!(ct.len(), 344, "实得 {}：{ct}", ct.len());
}

/// 1028 的对话体：`commands` 是 `"000"`、`command` 是 null（旧 TS 逐字）。
#[test]
fn dialogue_input_keeps_the_legacy_null_and_string_fields() {
    let d = build_dialogue_input("你好", "2026-10-07 10:45:30");
    assert_eq!(d["dialogue"], serde_json::json!("你好"));
    assert_eq!(d["commands"], serde_json::json!("000"));
    assert!(d["command"].is_null());
    assert_eq!(d["versionInfo"]["pcVersion"], serde_json::json!("1.7.0"));
}

/// 1028 的对话请求头：Authorization 是**派生出来的 Basic 串**、sourceChannel 两处一致。
#[test]
fn workbench_chat_headers_use_the_derived_basic_auth() {
    let d = build_dialogue_input("你好", "2026-10-07 10:45:30");
    let req = workbench_chat_request("Basic bW9iaWxl...", "UID", &d, AI_WORKBENCH_SOURCE_CHANNEL);
    assert_eq!(header(&req, "Authorization"), Some("Basic bW9iaWxl..."));
    assert_eq!(header(&req, "x-yun-app-channel"), Some("10175"));
    assert_eq!(
        header(&req, "x-yun-client-info"),
        Some("4g||30|||||||1440/900|zh-CN||||")
    );
    let body = req.body_str().unwrap();
    assert!(body.contains("\"sourceChannel\":\"10175\""), "{body}");
    assert!(body.contains("\"userId\":\"UID\""), "{body}");
    assert!(body.contains("\"continuationInfo\":null"), "{body}");
}

/// SSE 流解析：事件计数、拼文本、会话 id、最后一条 code/message。
#[test]
fn parse_sse_stream_collects_text_and_ids() {
    let body = concat!(
        "data: {\"code\":\"0\",\"data\":{\"sessionId\":\"S1\",\"dialogueId\":\"D1\",",
        "\"flowResult\":{\"outContent\":\"你好\"}}}\n\n",
        "data: {\"code\":\"0\",\"data\":{\"outContent\":\"，世界\"}}\n\n",
        "data: [DONE]\n\n",
        "not-json\n"
    );
    let r = parse_sse_stream(body);
    assert_eq!(r.event_count, 2);
    assert_eq!(r.full_text, "你好，世界");
    assert_eq!(r.session_id, "S1");
    assert_eq!(r.dialogue_id, "D1");
    assert_eq!(r.last_code, "0");
}

/// 状态口：`result.{taskStatus,cloudStatus}` 逐字读。
#[test]
fn parse_cloud_task_status_reads_the_two_fields() {
    let s = parse_cloud_task_status("{\"code\":0,\"result\":{\"taskStatus\":1,\"cloudStatus\":0}}")
        .unwrap();
    assert_eq!(s.code, 0);
    assert_eq!(s.task_status, Some(1));
    assert_eq!(s.cloud_status, Some(0));

    let s = parse_cloud_task_status("{\"code\":500}").unwrap();
    assert_eq!(s.task_status, None);
    assert!(parse_cloud_task_status("<html>").is_err());
}

/// 状态口的请求体三格逐字。
#[test]
fn cloud_task_status_body_is_the_legacy_shape() {
    let req = cloud_task_status_request("RSA==", 1028, AI_WORKBENCH_SOURCE_CHANNEL);
    let body = req.body_str().unwrap();
    assert!(body.contains("\"token\":\"RSA==\""), "{body}");
    assert!(body.contains("\"taskId\":1028"), "{body}");
    assert!(body.contains("\"sourceChannel\":\"10175\""), "{body}");
}
