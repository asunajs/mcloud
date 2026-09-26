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
use crate::log::{AppendOutcome, LogLine, Seq};
use crate::pipeline::{Pipeline, PipelineId};
use crate::run::{RunId, RunRecord};
use crate::schedule::{Schedule, ScheduleId};

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
///
/// 拆成几个聚焦的子 trait（而不是一个 20 方法的大接口）：
///
/// - 任务只需要 `LogStore`，不该被迫依赖账号读写；
/// - 换后端时（JSON ↔ SQLite）可以逐个替换；
/// - 测试里可以只造假的那一个。
///
/// `Store` 是它们的组合，凡是"能注入一个后端"的地方都收 `Store`。
#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn accounts(&self) -> Result<Vec<Account>>;
    async fn account(&self, id: &AccountId) -> Result<Option<Account>>;
    /// 保存账号。实现方必须**原子写**并保留备份。
    async fn save_account(&self, account: &Account) -> Result<()>;
    async fn remove_account(&self, id: &AccountId) -> Result<()>;

    /// 读取账号运行期状态（deviceId、jwtToken 缓存等）。
    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value>;
    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()>;
}

/// 运行记录。
#[async_trait]
pub trait RunStore: Send + Sync {
    /// 落盘一条运行记录。**同 `run_id` 已存在时是更新**（运行开始时先写
    /// `running`，结束时回填结果）。
    async fn upsert_run(&self, record: &RunRecord) -> Result<()>;
    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>>;
    async fn run(&self, id: RunId) -> Result<Option<RunRecord>>;
    /// 列出仍在运行中的记录 —— 进程重启后据此把孤儿标记为已中止。
    async fn running_runs(&self) -> Result<Vec<RunRecord>>;
}

/// 运行日志。
#[async_trait]
pub trait LogStore: Send + Sync {
    /// 追加一批日志。超出上限时丢弃最旧的并返回 `Trimmed`。
    async fn append_logs(&self, run_id: RunId, lines: &[LogLine]) -> Result<AppendOutcome>;
    /// 取某次运行的日志。`after_seq` 用于断线重连续传。
    async fn logs(&self, run_id: RunId, after_seq: Option<Seq>) -> Result<Vec<LogLine>>;
    /// 删除某次运行的日志（运行记录被清理时一并删）。
    async fn clear_logs(&self, run_id: RunId) -> Result<()>;
}

/// 定时任务。
#[async_trait]
pub trait ScheduleStore: Send + Sync {
    async fn schedules(&self) -> Result<Vec<Schedule>>;
    async fn schedule(&self, id: &ScheduleId) -> Result<Option<Schedule>>;
    async fn save_schedule(&self, schedule: &Schedule) -> Result<()>;
    async fn remove_schedule(&self, id: &ScheduleId) -> Result<()>;

    /// **窄写**：只推进水位线。
    ///
    /// 为什么不能复用 `save_schedule`：调度器在触发那一刻只持有
    /// "几秒前读到的行"，整行覆盖会把用户在这期间通过 WebUI 改过的
    /// 名字/参数一起抹掉。而水位线必须在打上游**之前**落盘，
    /// 所以这个写既紧急又必须最小。
    async fn mark_schedule_fired(&self, id: &ScheduleId, fire_ms: i64) -> Result<()>;

    /// **窄写**：只记录本次执行结果。
    ///
    /// `run_ms` 为 `None` ⇒ **不动**这一列（错过的触发点被弃掉时不产生运行，
    /// 抹掉真实的上次运行时间会显示成"从未执行"）。
    /// `error` 为 `None` ⇒ **写入空**（清掉旧错误本身就是结论，
    /// 否则"成功"旁边挂着一条旧错误，正是查不出原因的形态）。
    async fn record_schedule_run(
        &self,
        id: &ScheduleId,
        run_ms: Option<i64>,
        status: &str,
        error: Option<&str>,
    ) -> Result<()>;
}

/// 任务编排。
#[async_trait]
pub trait PipelineStore: Send + Sync {
    async fn pipelines(&self) -> Result<Vec<Pipeline>>;
    async fn pipeline(&self, id: &PipelineId) -> Result<Option<Pipeline>>;
    async fn save_pipeline(&self, pipeline: &Pipeline) -> Result<()>;
    async fn remove_pipeline(&self, id: &PipelineId) -> Result<()>;

    /// **窄写**：只记上次执行时间。
    ///
    /// 一次编排要跑几分钟；用整行 `save_pipeline` 会把这段时间里
    /// 用户改过的名字与步骤一起抹掉。
    async fn mark_pipeline_ran(&self, id: &PipelineId, run_ms: i64) -> Result<()>;
}

/// 全部持久化能力的组合。
///
/// 用 trait 继承而不是把方法摊平：这样 `Arc<dyn Store>` 仍然是
/// **一个**对象，注入时不用带五个 `Arc`。
pub trait Store:
    AccountStore + RunStore + LogStore + ScheduleStore + PipelineStore + Send + Sync
{
}

/// 任何同时实现了全部子 trait 的类型自动成为 `Store`。
impl<T> Store for T where
    T: AccountStore + RunStore + LogStore + ScheduleStore + PipelineStore + Send + Sync
{
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
