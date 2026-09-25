//! IO 边界：所有外部能力以 trait 形式暴露。
//!
//! 上层（engine / tasks）只依赖这些 trait；具体实现（reqwest、JSON 文件、
//! 系统时钟）由 `mcloud-app` 注入。这样：
//!
//! - 单元测试可用内存实现，无需网络与磁盘；
//! - 未来把 JSON 存储换成 SQLite 不需要改动业务代码。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::account::{Account, AccountId};
use crate::error::Result;
use crate::event::RunEvent;
use crate::run::{RunId, RunRecord};

// ── HTTP ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// 请求体。JSON 或 XML 文本。
    #[serde(default)]
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn json_body(mut self, v: &serde_json::Value) -> Self {
        self.body = Some(v.to_string());
        self
    }

    pub fn text_body(mut self, s: impl Into<String>) -> Self {
        self.body = Some(s.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 解析为 JSON。失败时给出截断后的原文，便于定位。
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body).map_err(|e| {
            let head: String = self.body.chars().take(200).collect();
            crate::Error::Parse(format!("{e}；原文前 200 字符：{head}"))
        })
    }
}

/// HTTP 客户端抽象。
///
/// 实现方负责注入域名相关的凭证头（authorization / jwttoken / deviceid），
/// 上层无需关心。
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse>;

    /// 便捷方法。
    async fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<HttpResponse> {
        self.request(HttpRequest::post(url).json_body(body)).await
    }
}

// ── 存储 ────────────────────────────────────────────────────────────────

/// 持久化抽象。
#[async_trait]
pub trait Store: Send + Sync {
    async fn accounts(&self) -> Result<Vec<Account>>;
    async fn account(&self, id: &AccountId) -> Result<Option<Account>>;
    /// 保存账号。实现方必须**原子写**并保留备份。
    async fn save_account(&self, account: &Account) -> Result<()>;
    async fn remove_account(&self, id: &AccountId) -> Result<()>;

    async fn append_run(&self, record: &RunRecord) -> Result<()>;
    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>>;
    async fn run(&self, id: RunId) -> Result<Option<RunRecord>>;

    /// 读取账号运行期状态（deviceId、jwtToken 缓存等）。
    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value>;
    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()>;
}

// ── 时钟 ────────────────────────────────────────────────────────────────

/// 时钟抽象。测试用固定时钟，签名向量才能稳定。
pub trait Clock: Send + Sync {
    /// 当前时间，毫秒级 unix 时间戳。
    fn now_ms(&self) -> i64;

    fn now_secs(&self) -> i64 {
        self.now_ms() / 1000
    }
}

// ── 事件 ────────────────────────────────────────────────────────────────

/// 事件接收端。
///
/// 实现方负责扇出：写日志、推 SSE、按级别过滤后推送通知。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: RunEvent);
}
