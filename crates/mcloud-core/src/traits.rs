//! IO trait 定义 —— 全部外部世界从这里进入。
//!
//! 具体实现在 `mcloud-io`（`ReqwestClient` / `SqliteStore` / `SystemClock`），
//! 测试与调试台用 `FixtureHttpClient` / 内存 store / `FixedClock` 替换。
//!
//! 这里用 `async fn in trait`（Rust 1.75+ 原生支持），**刻意不做 `dyn` 兼容** ——
//! 调用方用泛型参数静态分发，测试注入零成本抽象。
#![allow(async_fn_in_trait)]

use crate::error::Error;
use crate::model::{
    Account, AccountId, AccountState, CloudStats, ExchangeConfig, Pipeline, RunRecord, Schedule,
    WebuiConfig,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================ HTTP ============================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }

    /// 读请求默认可重试；写请求必须携带幂等键才允许重试。
    pub const fn is_read(self) -> bool {
        matches!(self, Self::Get)
    }
}

/// 出站请求。
///
/// 结构刻意保持"可序列化且可对拍"：golden request 对拍依赖它逐字段可比。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// 幂等键。写请求重试的前提。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
            idempotency_key: None,
        }
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
            idempotency_key: None,
        }
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    pub fn json_body<T: Serialize>(mut self, value: &T) -> Result<Self, Error> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| Error::Internal(format!("序列化请求体失败: {e}")))?;
        self.body = Some(bytes);
        if !self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        {
            self.headers
                .push(("content-type".into(), "application/json".into()));
        }
        Ok(self)
    }

    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms);
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// 该请求当前是否允许重试。
    pub fn is_retryable(&self) -> bool {
        self.method.is_read() || self.idempotency_key.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    pub fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body).map_err(|e| Error::Parse(format!("JSON 解析失败: {e}")))
    }

    pub fn text(&self) -> Result<String, Error> {
        String::from_utf8(self.body.clone())
            .map_err(|e| Error::Parse(format!("UTF-8 解码失败: {e}")))
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait HttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error>;
}

// ============================ 时间 ============================

/// 时间来源。
///
/// 业务代码**禁止**直接 `SystemTime::now()`：日快照归属、开抢倒计时、
/// 凭据到期判断全都依赖时间，必须能在测试里被钉死。
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
}

/// 固定时钟 —— 纯实现，用于单测与调试台回放。
#[derive(Debug, Clone)]
pub struct FixedClock {
    fixed: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(fixed: DateTime<Utc>) -> Self {
        Self { fixed }
    }

    /// 用 RFC 3339 字符串构造，失败即 panic（测试场景下应当立即暴露）。
    pub fn parse(rfc3339: &str) -> Self {
        Self::new(
            DateTime::parse_from_rfc3339(rfc3339)
                .expect("时间字面量必须是合法 RFC 3339")
                .with_timezone(&Utc),
        )
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.fixed
    }
}

// ============================ 存储 ============================

/// 状态存储。SQLite 实现在 `mcloud-io`。
pub trait StateStore {
    async fn upsert_account(&self, account: &Account) -> Result<(), Error>;
    async fn get_account(&self, id: &AccountId) -> Result<Option<Account>, Error>;
    async fn list_accounts(&self) -> Result<Vec<Account>, Error>;

    /// 读取账号登录态（含凭据）。
    ///
    /// 能力层要用它拿 `jwtToken` / `deviceId` 去调上游 —— 所以它属于**能力层契约**。
    /// 但注意：拿到的东西**不得**出现在日志、API 响应或错误信息里。
    async fn get_account_state(&self, id: &AccountId) -> Result<Option<AccountState>, Error>;
    /// 覆盖写入账号登录态。
    async fn upsert_account_state(&self, state: &AccountState) -> Result<(), Error>;

    /// 按 `account + date` UPSERT 日快照。
    async fn upsert_cloud_stats(&self, stats: &CloudStats) -> Result<(), Error>;
    /// 取 `date >= since` 的快照，按日期升序。
    async fn cloud_stats_since(
        &self,
        id: &AccountId,
        since: &str,
    ) -> Result<Vec<CloudStats>, Error>;

    async fn insert_run(&self, run: &RunRecord) -> Result<(), Error>;
    async fn update_run(&self, run: &RunRecord) -> Result<(), Error>;
    async fn list_runs(&self, limit: usize) -> Result<Vec<RunRecord>, Error>;
}

/// 配置与调度类持久化。
///
/// # 为什么与 [`StateStore`] 分开
///
/// 两者的**使用者不同**：`StateStore` 服务能力层（跑任务时要读凭据、写余额、记运行），
/// `ConfigStore` 服务服务层（WebUI 读写定时任务、组合、抢兑与界面偏好）。
/// 合成一个胖 trait 会让"只想给能力写个内存假实现"的人被迫实现十几个用不到的方法，
/// 而假实现正是本项目保证函数级可测的手段 —— 不能让它的成本反过来逼人不写测试。
///
/// 单行配置（抢兑、界面偏好）**读不到就返回默认值**，不返回 `Option`：
/// 旧版就是这个语义（文件不存在返回默认值、且不写盘），
/// 让每个调用点都去 `unwrap_or_default()` 只会把这段约定抄散。
pub trait ConfigStore {
    async fn upsert_schedule(&self, schedule: &Schedule) -> Result<(), Error>;
    async fn get_schedule(&self, id: &str) -> Result<Option<Schedule>, Error>;
    /// 按创建时间/名称排序的稳定列表。
    async fn list_schedules(&self) -> Result<Vec<Schedule>, Error>;
    /// 删除；返回是否真的删掉了一行。
    async fn delete_schedule(&self, id: &str) -> Result<bool, Error>;

    async fn upsert_pipeline(&self, pipeline: &Pipeline) -> Result<(), Error>;
    async fn get_pipeline(&self, id: &str) -> Result<Option<Pipeline>, Error>;
    async fn list_pipelines(&self) -> Result<Vec<Pipeline>, Error>;

    async fn exchange_config(&self) -> Result<ExchangeConfig, Error>;
    async fn put_exchange_config(&self, config: &ExchangeConfig) -> Result<(), Error>;

    async fn webui_config(&self) -> Result<WebuiConfig, Error>;
    async fn put_webui_config(&self, config: &WebuiConfig) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_requests_are_retryable_without_key() {
        assert!(HttpRequest::get("https://example.com/a").is_retryable());
    }

    #[test]
    fn write_requests_need_idempotency_key_to_retry() {
        let plain = HttpRequest::post("https://example.com/a");
        assert!(!plain.is_retryable(), "无幂等键的写请求不得重试");

        let keyed = HttpRequest::post("https://example.com/a").idempotency_key("run-1");
        assert!(keyed.is_retryable());
    }

    #[test]
    fn json_body_sets_content_type_once() {
        let req = HttpRequest::post("https://example.com/a")
            .json_body(&serde_json::json!({"a": 1}))
            .unwrap()
            .header("content-type", "application/json");
        let count = req
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .count();
        assert_eq!(
            count, 2,
            "显式追加的重复 header 由调用方负责，这里只验证不重复自动添加"
        );
    }

    #[test]
    fn fixed_clock_is_deterministic() {
        let clock = FixedClock::parse("2026-09-16T12:00:00Z");
        assert_eq!(clock.now().to_rfc3339(), "2026-09-16T12:00:00+00:00");
        assert_eq!(clock.now(), clock.now());
    }

    #[test]
    fn http_method_serializes_uppercase() {
        let json = serde_json::to_value(HttpMethod::Post).unwrap();
        assert_eq!(json, "POST");
    }
}
