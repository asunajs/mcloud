//! HTTP fixture 回放客户端。
//!
//! 用途有两个，都很关键：
//!
//! 1. **L2 测试**：用真实 HAR 转出的脱敏 fixture 验证上游接口层的请求构造与响应解析，
//!    全程零联网。
//! 2. **调试台离线模式**：WebUI 里对任意接口做参数化执行而不真的打上游。
//!
//! # 设计取舍：未命中必须报错
//!
//! 未命中时返回 [`mcloud_core::Error::FixtureMiss`]，**不静默回退到真实网络**。
//! 否则"零联网"就只是口头约定 —— 测试会因为悄悄打了真网而变绿，掩盖 fixture 缺口。

use mcloud_core::{Error, HttpClient, HttpMethod, HttpRequest, HttpResponse, sanitize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 一条 fixture 记录。字段与 `mcloud_core::sanitize::build_fixture_entry` 的输出一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureEntry {
    pub r#match: MatchSpec,
    pub request: RecordedRequest,
    pub response: RecordedResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchSpec {
    /// `METHOD host/path`，由 `mcloud_core::sanitize::match_key` 生成。
    pub key: String,
    pub method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRequest {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// 原始文本（已脱敏）。
    #[serde(default)]
    pub body: String,
    /// 结构化响应体（已脱敏）。优先用它构造回放响应，保留解析所需的类型信息。
    #[serde(default)]
    pub body_json: serde_json::Value,
}

/// 回放客户端。
pub struct FixtureHttpClient {
    /// `match.key` → 待消费的记录队列（同一接口在流程里出现多次时按顺序取）。
    queue: Mutex<HashMap<String, Vec<FixtureEntry>>>,
    /// 实际收到的请求，供 golden request 对拍使用。
    recorded: Mutex<Vec<HttpRequest>>,
}

impl FixtureHttpClient {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(HashMap::new()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    /// 从单个 JSON 文件加载（文件内容可以是单条，也可以是数组）。
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<usize, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Internal(format!("读取 fixture 失败 {}: {e}", path.display())))?;
        let entries = parse_entries(&text)
            .map_err(|e| Error::Internal(format!("解析 fixture 失败 {}: {e}", path.display())))?;
        Ok(self.insert(entries))
    }

    /// 递归加载目录下全部 `*.json`。
    pub fn load_dir(&self, dir: impl AsRef<Path>) -> Result<usize, Error> {
        let dir = dir.as_ref();
        let mut files = Vec::new();
        collect_json_files(dir, &mut files).map_err(|e| {
            Error::Internal(format!("遍历 fixture 目录失败 {}: {e}", dir.display()))
        })?;
        files.sort();

        let mut total = 0;
        for file in files {
            total += self.load_file(file)?;
        }
        Ok(total)
    }

    fn insert(&self, entries: Vec<FixtureEntry>) -> usize {
        let mut queue = self.queue.lock().expect("fixture 队列锁中毒");
        let count = entries.len();
        for entry in entries {
            queue
                .entry(entry.r#match.key.clone())
                .or_default()
                .push(entry);
        }
        count
    }

    /// 已加载的接口数量。
    pub fn key_count(&self) -> usize {
        self.queue.lock().expect("fixture 队列锁中毒").len()
    }

    /// 实际收到的请求快照（golden 对拍用）。
    pub fn recorded_requests(&self) -> Vec<HttpRequest> {
        self.recorded.lock().expect("请求记录锁中毒").clone()
    }

    /// 自检：把所有已加载 fixture 重新序列化后扫一遍泄漏。
    ///
    /// 这是"脱敏是否真的生效"的最后一道闸 —— 在 CI 里对所有 fixture 跑一次。
    pub fn audit_leaks(&self) -> Vec<String> {
        let queue = self.queue.lock().expect("fixture 队列锁中毒");
        let mut leaks = Vec::new();
        for (key, entries) in queue.iter() {
            for entry in entries {
                let text = serde_json::to_string(entry).unwrap_or_default();
                for leak in sanitize::scan_for_leaks(&text) {
                    leaks.push(format!("[{key}] {leak}"));
                }
            }
        }
        leaks
    }
}

impl Default for FixtureHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for FixtureHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.recorded
            .lock()
            .expect("请求记录锁中毒")
            .push(request.clone());

        let key = sanitize::match_key(request.method.as_str(), &request.url);

        let entry = {
            let mut queue = self.queue.lock().expect("fixture 队列锁中毒");
            match queue.get_mut(&key) {
                Some(list) if !list.is_empty() => {
                    // 同一接口多条记录时按顺序消费，用完保留最后一条（幂等重放）。
                    if list.len() > 1 {
                        list.remove(0)
                    } else {
                        list[0].clone()
                    }
                }
                _ => {
                    return Err(Error::FixtureMiss(format!(
                        "{key}（未命中即报错，绝不回退真实网络）"
                    )));
                }
            }
        };

        let body = response_body_bytes(&entry.response);
        let mut response = HttpResponse::new(entry.response.status, body);
        response.headers = entry.response.headers.clone();
        Ok(response)
    }
}

/// 回放响应体：优先用结构化的 `body_json`，否则退回文本。
fn response_body_bytes(recorded: &RecordedResponse) -> Vec<u8> {
    if !recorded.body_json.is_null() {
        serde_json::to_vec(&recorded.body_json).unwrap_or_default()
    } else {
        recorded.body.clone().into_bytes()
    }
}

fn parse_entries(text: &str) -> Result<Vec<FixtureEntry>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    match value {
        serde_json::Value::Array(_) => serde_json::from_value(value),
        other => Ok(vec![serde_json::from_value(other)?]),
    }
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

/// 便捷构造：直接喂内存里的记录，便于单测。
pub fn entry_from_parts(
    method: HttpMethod,
    url: &str,
    status: u16,
    body_json: serde_json::Value,
) -> FixtureEntry {
    FixtureEntry {
        r#match: MatchSpec {
            key: sanitize::match_key(method.as_str(), url),
            method: method.as_str().to_string(),
        },
        request: RecordedRequest {
            url: sanitize::redact_url(url),
            headers: Vec::new(),
            body: String::new(),
        },
        response: RecordedResponse {
            status,
            headers: Vec::new(),
            body: body_json.to_string(),
            body_json,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_with(entries: Vec<FixtureEntry>) -> FixtureHttpClient {
        let client = FixtureHttpClient::new();
        client.insert(entries);
        client
    }

    #[tokio::test]
    async fn replays_recorded_response() {
        let client = client_with(vec![entry_from_parts(
            HttpMethod::Post,
            "https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0",
            200,
            serde_json::json!({"code": 0, "data": {"count": 54}}),
        )]);

        let resp = client
            .send(HttpRequest::post(
                "https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0&_t=1",
            ))
            .await
            .expect("应命中 fixture");

        assert_eq!(resp.status, 200);
        let json: serde_json::Value = resp.json().unwrap();
        assert_eq!(json["data"]["count"], 54);
    }

    #[tokio::test]
    async fn miss_is_an_error_never_a_real_request() {
        let client = FixtureHttpClient::new();
        let err = client
            .send(HttpRequest::get("https://example.com/nothing"))
            .await
            .expect_err("未命中必须报错");
        assert!(matches!(err, Error::FixtureMiss(_)), "实际: {err:?}");
    }

    #[tokio::test]
    async fn records_every_request_for_golden_comparison() {
        let client = client_with(vec![entry_from_parts(
            HttpMethod::Get,
            "https://a.example.com/x",
            200,
            serde_json::json!({}),
        )]);
        let _ = client
            .send(HttpRequest::get("https://a.example.com/x").header("a", "b"))
            .await;

        let recorded = client.recorded_requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].method, HttpMethod::Get);
        assert_eq!(
            recorded[0].headers,
            vec![("a".to_string(), "b".to_string())]
        );
    }

    #[tokio::test]
    async fn duplicate_entries_are_consumed_in_order() {
        let client = client_with(vec![
            entry_from_parts(
                HttpMethod::Get,
                "https://a.example.com/x",
                200,
                serde_json::json!({"n": 1}),
            ),
            entry_from_parts(
                HttpMethod::Get,
                "https://a.example.com/x",
                200,
                serde_json::json!({"n": 2}),
            ),
        ]);

        let first: serde_json::Value = client
            .send(HttpRequest::get("https://a.example.com/x"))
            .await
            .unwrap()
            .json()
            .unwrap();
        let second: serde_json::Value = client
            .send(HttpRequest::get("https://a.example.com/x"))
            .await
            .unwrap()
            .json()
            .unwrap();
        let third: serde_json::Value = client
            .send(HttpRequest::get("https://a.example.com/x"))
            .await
            .unwrap()
            .json()
            .unwrap();

        assert_eq!(first["n"], 1);
        assert_eq!(second["n"], 2);
        assert_eq!(third["n"], 2, "用尽后保留最后一条，保证幂等重放");
    }

    #[test]
    fn loads_from_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("m.mcloud.139.com");
        std::fs::create_dir_all(&nested).unwrap();

        let entry = entry_from_parts(
            HttpMethod::Get,
            "https://m.mcloud.139.com/a",
            200,
            serde_json::json!({"ok": true}),
        );
        std::fs::write(
            nested.join("exchangeList.json"),
            serde_json::to_string_pretty(&entry).unwrap(),
        )
        .unwrap();
        // 非 JSON 文件应被忽略
        std::fs::write(nested.join("readme.md"), "not a fixture").unwrap();

        let client = FixtureHttpClient::new();
        let loaded = client.load_dir(dir.path()).unwrap();
        assert_eq!(loaded, 1);
        assert_eq!(client.key_count(), 1);
    }

    #[test]
    fn audit_reports_a_leaky_fixture() {
        let mut entry = entry_from_parts(
            HttpMethod::Post,
            "https://a.example.com/login",
            200,
            serde_json::json!({"auth": "REAL_SECRET"}),
        );
        entry.response.body = "{\"auth\":\"REAL_SECRET\"}".into();

        let client = client_with(vec![entry]);
        assert!(
            !client.audit_leaks().is_empty(),
            "自检必须能发现未脱敏的 fixture"
        );
    }

    #[test]
    fn audit_accepts_a_clean_fixture() {
        let entry = entry_from_parts(
            HttpMethod::Post,
            "https://a.example.com/login",
            200,
            serde_json::json!({"auth": sanitize::PLACEHOLDER, "nickname": "账号2"}),
        );
        let client = client_with(vec![entry]);
        assert!(client.audit_leaks().is_empty());
    }
}
