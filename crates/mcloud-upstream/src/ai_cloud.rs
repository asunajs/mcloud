//! AI 云（`ai.yun.139.com`）与云盘任务状态那两口 —— 玩法 **585（AI 相机）**
//! 与 **1028（AI 工作台）** 的协议层。
//!
//! 依据：旧 TS `packages/core/src/tasks/ai-tasks.ts:56-188`（585）、
//! `packages/core/src/tasks/app/runners.ts:345-448`（1028 的编排）、
//! `packages/core/src/apis/ai-workbench.ts`（全部请求形状）。
//! URL / 头 / 体逐字照抄。
//!
//! # ⚠ 两个玩法共用一个"AI 域鉴权"，但**来源不同**
//!
//! * **585** 用的是**云盘凭据**：`Authorization: <authToken>`（裸 token，
//!   **不带** `Basic `）+ `x-yun-uni: <userDomainId>`。`userDomainId` 从
//!   `jwttoken` 里解出来（旧 TS `parseJwtUserDomainId`）。
//! * **1028** 用的是**邮箱域派生的**凭据：`disk:queryUserToken` →
//!   DES-ECB 解出手机号（key `139_mail`）→
//!   `Authorization: Basic base64("mobile:<手机号>:<token>")`。
//!
//! 两者**不能混用**（一个打 `ai.yun.139.com` 时带云盘 token、另一个带邮箱派生的
//! Basic 串）—— 混了的回话是业务码，不像"头拼错了"。
//!
//! # ⚠ 1028 那口是 SSE，而本仓的 HTTP 客户端有 **30 秒总 deadline**
//!
//! 旧 TS 给这一枪单独设了 **60 秒**（`timeoutSec = 60`）。本仓的
//! `ReqwestClient` 用的是组装层那份 profile（缺省 30 秒），任务层改不了
//! ⇒ 实测这一趟约 **12 秒**（2026-10-07 10:42:58→10:43:10）在窗口内，
//! 但**链路慢的时候会被自己的 deadline 掐断**，表现是 `Network` 错误而不是
//! "AI 没回话"。这是**已知的结构差**，记在这里，别当成上游问题去查。

use serde_json::{json, Value};

use mcloud_core::{Error, HttpRequest, Result};
use mcloud_crypto::rsa_pkcs1v15_encrypt_base64_with_b64_key;

/// 585 的识图口。
pub const AI_RECOGNIZE_URL: &str = "https://ai.yun.139.com/api/image/aiRecognize";
/// 两个玩法共用的对话口（SSE）。
pub const AI_CHAT_V2_URL: &str = "https://ai.yun.139.com/api/outer/assistant/chat/v2/add";
/// 1028 的前置/复查状态口。
pub const CLOUD_TASK_STATUS_URL: &str =
    "https://m.mcloud.139.com/ycloud/signin/public/getUserTaskAndCloudStatusExternal";
/// 1028 的 `sourceChannel`（旧 TS `runners.ts:351` 逐字）。
pub const AI_WORKBENCH_SOURCE_CHANNEL: &str = "10175";
/// 585 的 `sourceChannel` / `x-yun-app-channel`（旧 TS `ai-tasks.ts` 的 `"101"`）。
pub const AI_CAMERA_SOURCE_CHANNEL: &str = "101";
/// 585 那几枚 AI 头共用的 UA。
pub const AI_UA: &str = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) \
AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36";
/// 1028 的 SSE 头里那枚浏览器 UA（旧 TS `ai-workbench.ts:172-173` 逐字）。
pub const AI_CHAT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// 585 的 AI 头里那枚固定值（旧 TS `buildAiHeaders` 的 `x-yun-client-info`）。
pub const AI_CAMERA_CLIENT_INFO: &str = "||8||||||||||||";

/// `getCloudTaskStatus` 的 RSA 公钥（旧 TS `ai-workbench.ts:7-16` 逐字 PEM）。
///
/// 用它的**去头去尾去换行**那一份（[`cloud_task_status_pubkey_b64`]）去加密。
pub const CLOUD_TASK_STATUS_PUBKEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA1PWBlhoygGhj2uqniCjI\n\
PXbyufSSeST1yojxn1w1G6E/Uc9OpOFcXqcZYb1uc111zYdBAupYOxG+PXsQoZnZ\n\
TXiZfYO6oTBC623OzEPnE3afYGa62E/8g8C4Zrzaqng4Sw+YvERoroHRM29SRvCB\n\
QK7qcszK2kGLuPVh02WhHuprvR6GQPWcOOXLYOUbg2qVhKu+J1+bJ2wS4camxNF2\n\
W96YSTdWPysHSw0o3fPIOfkDylCmj4odAVIln5vBP/rYtKeFskco+Tpyd5UM+zpw\n\
Rf+VOJ/c/7DIC2iUjsENdb2pasLlhp5mTDeARDK4lzQR+MkJaGzYis3NqSu2PAPJ\n\
TwIDAQAB\n\
-----END PUBLIC KEY-----";

/// PEM → 裸 base64（旧 TS `runners.ts:361` 那个 `replace(/-----[A-Z ]+-----/g, "")`）。
///
/// 照抄：**只去头尾行与换行**，不做别的加工。
pub fn cloud_task_status_pubkey_b64() -> String {
    CLOUD_TASK_STATUS_PUBKEY
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<String>()
}

/// 用上面那把公钥加密手机号（旧 TS `rt.crypto.rsaEncrypt(auth.phone, pubkeyB64)`）。
pub fn encrypt_phone_for_task_status(
    phone: &str,
    rng: &mut impl mcloud_crypto::Random,
) -> Result<String> {
    rsa_pkcs1v15_encrypt_base64_with_b64_key(&cloud_task_status_pubkey_b64(), phone.as_bytes(), rng)
        .map_err(|e| Error::Other(format!("RSA 加密手机号失败：{e}")))
}

// ── ① 585：AI 相机 ─────────────────────────────────────────────────────

/// `POST {AI_RECOGNIZE_URL}`（旧 TS `ai-tasks.ts:82-87` 的 payload 逐字）。
///
/// `base64` 是一张 100×100 白色 PNG 的 base64（**不带** data URL 前缀，
/// 旧 TS `getAiCameraSampleBase64()` 给的就是裸 base64）。
///
/// ⚠⚠ **这一枪不带 `x-yun-client-info`** —— 旧 TS 的 `buildAiHeaders(ctx, ctx)`
/// 那个 `useClientInfo` 参数**缺省是 `false`**，只有对话那一枪（自己内联写头）
/// 才带它 ✓。2026-10-07 真机踩过：多带这一枚头 ⇒ 上游回
/// `{"success":false,"message":"鉴权失败"}`，而那句话听起来像"票过期了"，
/// 会把人带去查 token 有效期（那儿一切正常）✗。
///
/// ⚠⚠ **`auth_token` 要传完整 auth（`"Basic …"` 那一串），不是裸 token** ——
/// 旧 TS 那一句写的是 `Authorization: ctx.config.token`，但它的 `ctx.http`
/// 会按域名（`AUTH_DOMAINS` 命中 `yun.139.com`）**再注入一次** authorization，
/// 实发头是完整串 ✓。2026-10-07 真机对拍：裸 token ⇒
/// `{"code":"01000002","message":"鉴权失败"}`，完整 auth ⇒ `success:true` ✓。
///
/// ⚠⚠ **`deviceid` 是必须的**（2026-10-07 抓的是 TS 自己那一枪的实发头）：
/// 旧 TS 的 `ctx.http` 会按域名注入 `accept: application/json` 与
/// `deviceid: <smid>`（`TARGET_HOSTNAMES` 命中 `yun.139.com` ✓）。
/// 少了 `deviceid` 上游同样回 `鉴权失败` —— 与"票过期"是**同一句话**，
/// 所以这一格必须在协议层就带上，别指望调用方记得 ✓。
pub fn ai_recognize_request(
    auth_token: &str,
    user_domain_id: &str,
    image_base64: &str,
    x_yun_tid: &str,
    device_id: &str,
) -> HttpRequest {
    let body = json!({
        "channelId": AI_CAMERA_SOURCE_CHANNEL,
        "userId": user_domain_id,
        "recognizeType": "1",
        "base64": image_base64,
        "sendType": "2",
        "imageExt": "jpg",
        "uploadToCloud": true,
        "timeout": 30000,
    });
    HttpRequest::post(AI_RECOGNIZE_URL)
        .header("accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", AI_UA)
        .header("deviceid", device_id)
        .header("x-yun-tid", x_yun_tid)
        .header("Authorization", auth_token)
        .header("x-yun-api-version", "v1")
        .header("x-yun-module-type", "100")
        .header("x-yun-op-type", "1")
        .header("x-yun-app-channel", "10214200")
        .header("hcy-cool-flag", "1")
        .header("x-yun-uni", user_domain_id)
        .json_body(&body)
}

/// 585 的识图结果（旧 TS 只看 `success` / `data.fileId` / `data.taskId` / `message`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiRecognize {
    pub success: bool,
    pub file_id: Option<String>,
    pub task_id: Option<String>,
    pub message: Option<String>,
}

/// 解析识图响应。
pub fn parse_ai_recognize(body: &str) -> Result<AiRecognize> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        Error::Parse(format!(
            "AI 识图响应不是 JSON：{e}；原文前 200 字：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;
    Ok(AiRecognize {
        success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
        file_id: v
            .get("data")
            .and_then(|d| d.get("fileId"))
            .and_then(Value::as_str)
            .map(str::to_string),
        task_id: v
            .get("data")
            .and_then(|d| d.get("taskId"))
            .map(|t| match t {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
        message: v.get("message").and_then(Value::as_str).map(str::to_string),
    })
}

/// 585 的对话体（旧 TS `ai-tasks.ts:105-130` 的 `chatPayload` 逐字）。
///
/// `file_name` = `${parseInt(taskId) + 1}.jpeg`（旧 TS 那一行**故意**用
/// 任务号 +1 当文件名 —— 照抄，别"修"成别的）。
pub fn ai_camera_chat_payload(
    user_id: &str,
    file_id: &str,
    file_name: &str,
    input_time: &str,
) -> Value {
    json!({
        "userId": user_id,
        "sessionId": "",
        "applicationType": "chat",
        "applicationId": "",
        "sourceChannel": AI_CAMERA_SOURCE_CHANNEL,
        "dialogueInput": {
            "dialogue": "？",
            "prompt": "",
            "inputTime": input_time,
            "enableForceLlm": false,
            "enableForceNetworkSearch": true,
            "enableModelThinking": false,
            "enableAllNetworkSearch": false,
            "enableKnowledgeAndNetworkSearch": false,
            "enableRegenerate": false,
            "versionInfo": { "h5Version": "2.7.6" },
            "extInfo": "{}",
            "sortInfo": {},
            "toolSetting": { "imageToolSetting": { "enableLlmDescribe": true } },
            "attachment": {
                "attachmentTypeList": [3],
                "fileList": [{ "fileId": file_id, "name": file_name }],
            },
        },
    })
}

/// 585 的对话头（旧 TS `ai-tasks.ts:132-149` 那个 `fetch` 的头逐字）。
///
/// ⚠ 这一支的 `Authorization` 是**完整的 auth**（带 `Basic ` 前缀那一串），
/// 与识图那一支的**裸 token** 不同 —— 旧 TS 就是这么写的，别统一。
pub fn ai_camera_chat_request(auth: &str, payload: &Value, x_yun_tid: &str) -> HttpRequest {
    HttpRequest::post(AI_CHAT_V2_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", AI_UA)
        .header("x-yun-tid", x_yun_tid)
        .header("Authorization", auth)
        .header("x-yun-api-version", "v1")
        .header("x-yun-module-type", "100")
        .header("x-yun-op-type", "1")
        .header("x-yun-app-channel", "10214200")
        .header("hcy-cool-flag", "1")
        .header("x-yun-client-info", AI_CAMERA_CLIENT_INFO)
        .header("Accept", "text/event-stream")
        .json_body(payload)
}

/// 585 的成败判据（旧 TS `isAiChatSuccess` 逐字）：
/// 响应是字符串且非空、或 `success` 为真、或 `code` 是 `"0000"` / `0`。
pub fn is_ai_chat_success(v: &Value) -> bool {
    match v {
        Value::String(s) => !s.trim().is_empty(),
        Value::Object(o) => {
            o.get("success").and_then(Value::as_bool).unwrap_or(false)
                || matches!(o.get("code"), Some(Value::String(s)) if s == "0000")
                || matches!(o.get("code"), Some(Value::Number(n)) if n.as_i64() == Some(0))
        }
        _ => false,
    }
}

/// 585 取**最后一条 `data:` 行**当结果（旧 TS `ai-tasks.ts:151-169` 逐字）。
///
/// 找不到 `data:` 行时退化成"整个响应体当 JSON 解析"，
/// 再失败则回 `{"raw": <原文>}`（旧 TS 就是这个兜底）。
pub fn parse_last_sse_data(body: &str) -> Value {
    for line in body.lines().rev() {
        if let Some(rest) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(rest.trim()) {
                return v;
            }
        }
    }
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return v;
    }
    json!({ "raw": body })
}

// ── ② 1028：AI 工作台 ──────────────────────────────────────────────────

/// 1028 的对话体（旧 TS `buildDialogueInput` 逐字）。
pub fn build_dialogue_input(message: &str, input_time: &str) -> Value {
    json!({
        "dialogue": message,
        "prompt": "",
        "inputTime": input_time,
        "commands": "000",
        "command": Value::Null,
        "resourceType": "0",
        "resourceId": "",
        "dialogueType": "0",
        "commandType": 1,
        "enableForceNetworkSearch": false,
        "enableAllNetworkSearch": false,
        "enableAiSearch": false,
        "extInfo": "{}",
        "versionInfo": { "pcVersion": "1.7.0", "h5Version": "2.5.0" },
        "toolSetting": { "imageToolSetting": { "enableLlmDescribe": false } },
        "attachment": {},
        "aiWritingSetting": {},
    })
}

/// 1028 的对话请求（旧 TS `fireWorkbenchChat` 的体与头逐字）。
pub fn workbench_chat_request(
    authorization: &str,
    user_id: &str,
    dialogue_input: &Value,
    source_channel: &str,
) -> HttpRequest {
    let body = json!({
        "applicationType": "chat",
        "sessionId": "",
        "dialogueInput": dialogue_input,
        "sourceChannel": source_channel,
        "userId": user_id,
        "continuationInfo": Value::Null,
    });
    HttpRequest::post(AI_CHAT_V2_URL)
        .header("x-yun-api-version", "v1")
        .header("x-yun-client-info", "4g||30|||||||1440/900|zh-CN||||")
        .header("x-yun-app-channel", source_channel)
        .header("Authorization", authorization)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("User-Agent", AI_CHAT_UA)
        .json_body(&body)
}

/// SSE 消费结果（旧 TS `SseChatResult` 逐字）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseChat {
    pub full_text: String,
    pub session_id: String,
    pub dialogue_id: String,
    pub event_count: usize,
    pub last_code: String,
    pub last_message: String,
}

/// 解析 SSE 正文（旧 TS `consumeSseStream` 逐字）。
///
/// ⚠ 本仓拿到的是**整份响应体**（客户端不是流式的）⇒ 与 TS 逐行读流
/// 的结果**等价**，前提是服务端会自己收流。收不了流的那天表现是
/// "请求挂到客户端 deadline"（见模块头那条已知结构差）。
pub fn parse_sse_stream(body: &str) -> SseChat {
    let mut out = SseChat::default();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        out.event_count += 1;
        if let Some(c) = ev.get("code") {
            out.last_code = match c {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
        }
        if let Some(m) = ev.get("message").and_then(Value::as_str) {
            out.last_message = m.to_string();
        }
        if let Some(d) = ev.get("data") {
            if let Some(s) = d.get("sessionId").and_then(Value::as_str) {
                out.session_id = s.to_string();
            }
            if let Some(s) = d.get("dialogueId").and_then(Value::as_str) {
                out.dialogue_id = s.to_string();
            }
            if let Some(s) = d
                .get("flowResult")
                .and_then(|f| f.get("outContent"))
                .and_then(Value::as_str)
            {
                out.full_text.push_str(s);
            }
            if let Some(s) = d.get("outContent").and_then(Value::as_str) {
                out.full_text.push_str(s);
            }
        }
    }
    out
}

/// `POST {CLOUD_TASK_STATUS_URL}`（旧 TS `getCloudTaskStatus` 逐字）。
pub fn cloud_task_status_request(
    rsa_token: &str,
    task_id: i64,
    source_channel: &str,
) -> HttpRequest {
    let body = json!({
        "token": rsa_token,
        "taskId": task_id,
        "sourceChannel": source_channel,
    });
    HttpRequest::post(CLOUD_TASK_STATUS_URL)
        .header("Content-Type", "application/json")
        .header("Origin", "https://appmail.mail.10086.cn")
        .header("Referer", "https://appmail.mail.10086.cn/")
        .header("User-Agent", AI_CHAT_UA)
        .json_body(&body)
}

/// 1028 的前置/复查读数（旧 TS 只读 `result.{taskStatus,cloudStatus}`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudTaskStatus {
    pub code: i64,
    pub task_status: Option<i64>,
    pub cloud_status: Option<i64>,
}

/// 解析状态响应。
pub fn parse_cloud_task_status(body: &str) -> Result<CloudTaskStatus> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        Error::Parse(format!(
            "cloudTaskStatus 响应不是 JSON：{e}；原文前 200 字：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let result = v.get("result");
    Ok(CloudTaskStatus {
        code,
        task_status: result
            .and_then(|r| r.get("taskStatus"))
            .and_then(Value::as_i64),
        cloud_status: result
            .and_then(|r| r.get("cloudStatus"))
            .and_then(Value::as_i64),
    })
}

#[cfg(test)]
mod tests;
