//! 云笔记（`mnote`）族协议层 —— 玩法 **107「创建云笔记」**。
//!
//! 依据：旧 TS `packages/core/src/tasks/app/runners.ts:46-66`（编排）与
//! `packages/core/src/apis/note.ts`（三枪的形状）。URL / 头 / 体逐字照抄。
//!
//! # 链路（旧 TS 逐字）
//!
//! ```text
//! ① POST https://note.mcloud.139.com/noteServer/api/authTokenRefresh.do
//!      body {authToken, userPhone}   ← authToken 就是云盘那份裸 token
//!      ⇒ 响应里要有 headers.app_auth，否则整条链**直接放弃**（旧 TS `return`）
//! ② POST https://mnote.caiyun.feixin.10086.cn/noteServer/api/createNote.do
//! ③ sleep 2000ms
//! ④ POST …/noteServer/api/moveToRecycleBin.do
//! ```
//!
//! # ⚠ ① 今天**回不到** `headers.app_auth`（2026-10-07 实测）
//!
//! 探针读数：`获取笔记鉴权失败，跳过云笔记任务` ⇒ 第 ②③④ 枪**一枪都没发**。
//! 旧 TS 对这个失败的处理是**静默跳过**（只 `logger.info` 一句）⇒ 本仓照抄
//! "跳过"，但把原因写进小结（"跳过"与"成功"在界面上必须分得开）。
//!
//! # ⚠ 建笔记那支的体很大，且**逐字照抄**
//!
//! 那些字段（`expands.noteType` / `landMark` / `topmost` 字符串 `"0"` …）看着
//! 像可以精简，但**没有一样是我们可以删的**：上游认不认缺字段没有样本，
//! 而删字段的表现是"笔记建了但任务没算" —— 那种失败最难查。照抄。

use serde_json::{json, Value};

use mcloud_core::{Error, HttpRequest, Result};

/// 笔记鉴权口。
pub const NOTE_AUTH_URL: &str = "https://note.mcloud.139.com/noteServer/api/authTokenRefresh.do";
/// 笔记服务主机。
pub const MNOTE_HOST: &str = "https://mnote.caiyun.feixin.10086.cn";
/// 建笔记。
pub const NOTE_CREATE_PATH: &str = "/noteServer/api/createNote.do";
/// 移进回收站。
pub const NOTE_TRASH_PATH: &str = "/noteServer/api/moveToRecycleBin.do";

/// `POST {NOTE_AUTH_URL}`（旧 TS `note.ts:9-31` 逐字）。
pub fn note_auth_request(token: &str, phone: &str) -> HttpRequest {
    let body = json!({ "authToken": token, "userPhone": phone });
    HttpRequest::post(NOTE_AUTH_URL)
        .header("Content-Type", "application/json")
        .header("APP_CP", "android")
        .header("APP_NUMBER", phone)
        .header("CP_VERSION", "3.2.0")
        .header("x-huawei-channelsrc", "10001400")
        .header("User-Agent", "mobile")
        .json_body(&body)
}

/// 从鉴权响应里取 `headers` 那一格（旧 TS `resp.headers`）。
///
/// ⚠ 返回的是**整个 headers 对象**：旧 TS 把它整份当 `noteAuth.headers`
/// 再并进后两枪的头里（`...headers`）⇒ 这里照抄，不挑字段。
pub fn parse_note_headers(body: &str) -> Result<Option<Value>> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        Error::Parse(format!(
            "笔记鉴权响应不是 JSON：{e}；原文前 200 字：{}",
            body.chars().take(200).collect::<String>()
        ))
    })?;
    Ok(v.get("headers").cloned())
}

/// `headers.app_auth` 是不是在（旧 TS 那句 `if (!noteAuth?.headers?.app_auth) return`）。
pub fn note_app_auth(headers: &Value) -> Option<&str> {
    headers.get("app_auth").and_then(Value::as_str)
}

/// `POST {MNOTE_HOST}{NOTE_CREATE_PATH}`（旧 TS `note.ts:38-81` 逐字）。
///
/// `note_headers` 是 ① 拿回来的那一份（旧 TS 的 `...headers` 展开）。
pub fn create_note_request(
    note_id: &str,
    title: &str,
    phone: &str,
    note_headers: &Value,
    now_ms: i64,
) -> HttpRequest {
    let ts = now_ms.to_string();
    let body = json!({
        "archived": 0,
        "attachmentdir": "",
        "attachmentdirid": "",
        "attachments": [],
        "contentid": "",
        "contents": [{
            "data": "<span></span>",
            "noteId": note_id,
            "sortOrder": 0,
            "type": "TEXT",
        }],
        "cp": "",
        "createtime": ts,
        "description": "",
        "expands": { "noteType": 0 },
        "landMark": [],
        "latlng": "",
        "location": "",
        "noteid": note_id,
        "remindtime": "",
        "remindtype": 0,
        "revision": "1",
        "system": "",
        "tags": [],
        "title": title,
        "topmost": "0",
        "updatetime": ts,
        "userphone": phone,
        "version": "",
        "visitTime": ts,
    });
    let mut req = HttpRequest::post(format!("{MNOTE_HOST}{NOTE_CREATE_PATH}"))
        .header("Content-Type", "application/json")
        .header("APP_CP", "pc")
        .header("APP_NUMBER", phone)
        .header("CP_VERSION", "7.7.1.20240115");
    req = with_note_headers(req, note_headers);
    req.json_body(&body)
}

/// `POST {MNOTE_HOST}{NOTE_TRASH_PATH}`（旧 TS `note.ts:86-106` 逐字）。
pub fn delete_note_request(note_id: &str, note_headers: &Value) -> HttpRequest {
    let body = json!({ "noteids": [{ "noteid": note_id }] });
    let mut req = HttpRequest::post(format!("{MNOTE_HOST}{NOTE_TRASH_PATH}"))
        .header("Content-Type", "application/json")
        .header("APP_CP", "pc")
        .header("CP_VERSION", "7.7.1.20240115");
    req = with_note_headers(req, note_headers);
    req.json_body(&body)
}

/// 把 ① 拿回来的 `headers` **整份**并进后两枪（旧 TS 的 `...headers` 展开）。
///
/// ⚠ 只收字符串值：那一格是上游给的 JSON，值不是字符串的成员直接跳过
///（`HttpRequest` 的头值是字符串，塞不进去 —— 而"塞不进去"在这里
/// 应当表现为**少一枚头**，不是 panic）。
fn with_note_headers(mut req: HttpRequest, headers: &Value) -> HttpRequest {
    if let Some(obj) = headers.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                req = req.header(k.clone(), s.to_string());
            }
        }
    }
    req
}

#[cfg(test)]
mod tests;
