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
use crate::user::{Session, User, UserId};

// ── HTTP ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// 请求体的**原始字节**。
    ///
    /// # ⚠ 为什么是字节而不是 `String`
    ///
    /// 网盘上传那一枪（`PUT <uploadUrl>`）发的是**图片的原始字节** ——
    /// 用 `String` 装不下（那不是 UTF-8 文本，`from_utf8` 会失败或损坏内容）。
    /// 而"损坏内容"在那个位置**不会报错**：上游只会回一个校验失败，
    /// 排查方向会整个跑偏。所以类型上就让它装得下。
    ///
    /// 文本请求照旧用 [`HttpRequest::json_body`] / [`HttpRequest::text_body`]，
    /// 它们只是把文本转成字节；读回文本用 [`HttpRequest::body_str`]。
    #[serde(default)]
    pub body: Option<Vec<u8>>,
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

    /// `PUT` —— 上传那一枪用的方法。
    pub fn put(url: impl Into<String>) -> Self {
        Self {
            method: "PUT".into(),
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
        self.body = Some(v.to_string().into_bytes());
        self
    }

    pub fn text_body(mut self, s: impl Into<String>) -> Self {
        self.body = Some(s.into().into_bytes());
        self
    }

    /// 二进制体（上传图片用）。
    pub fn bytes_body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    /// 把体读成文本（测试与少数要读回原文的调用点用）。
    ///
    /// ⚠ 体不是合法 UTF-8 时回 `None` —— 那说明这一枪带的是二进制，
    /// **不是**"没有体"（要判有没有体用 `body.is_some()`）。
    pub fn body_str(&self) -> Option<&str> {
        match self.body.as_ref() {
            Some(bytes) => std::str::from_utf8(bytes).ok(),
            None => None,
        }
    }

    /// 请求是否**带体**。
    ///
    /// ⚠ **空体也算带体**：`Some(vec![])` 与 `None` 语义不同 ——
    /// 直播那两条 `give` / `draw` 靠 `None` 表达"无体"，
    /// 而"显式给了个空体"是要发出一个 `content-length: 0` 的。
    /// ⇒ 判"有没有体"只看 `is_some()`，**不要**顺手加 `!is_empty()`：
    /// 那会把显式空体变成无体，表现是少一个头、少一段长度声明。
    pub fn has_body(&self) -> bool {
        self.body.is_some()
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

/// 系统用户与会话（WebUI 登录闸）。
///
/// 与 `AccountStore` 分开是刻意的：那是"要操作的对象"，这是"谁能操作"。
/// 合成一个 trait 会让"只想给任务写个内存假实现"的人被迫实现用户管理，
/// 而假实现正是保证函数级可测的手段。
#[async_trait]
pub trait UserStore: Send + Sync {
    /// 用户数。**首次部署判定用它**：0 个用户 ⇒ 需要走 `/api/setup`。
    async fn user_count(&self) -> Result<usize>;
    async fn users(&self) -> Result<Vec<User>>;
    async fn user_by_name(&self, name: &str) -> Result<Option<User>>;
    async fn user(&self, id: &UserId) -> Result<Option<User>>;
    async fn save_user(&self, user: &User) -> Result<()>;

    /// 建会话（存的是 `sha256(token)`，不是原文）。
    async fn save_session(&self, session: &Session) -> Result<()>;
    /// 按令牌哈希查会话。
    async fn session(&self, token_hash: &str) -> Result<Option<Session>>;
    async fn remove_session(&self, token_hash: &str) -> Result<()>;
    /// 清掉某用户的所有会话（改口令时用 —— 旧会话必须失效）。
    ///
    /// 返回被清掉的条数：调用方可以据此提示"已登出 N 个设备"。
    async fn remove_user_sessions(&self, user_id: &UserId) -> Result<usize>;
    /// 清掉已过期的会话。顺手回收，避免表无限增长。
    async fn purge_expired_sessions(&self, now_ms: i64) -> Result<usize>;
}

/// 全部持久化能力的组合。
///
/// 用 trait 继承而不是把方法摊平：这样 `Arc<dyn Store>` 仍然是
/// **一个**对象，注入时不用带五个 `Arc`。
pub trait Store:
    AccountStore + RunStore + LogStore + ScheduleStore + PipelineStore + UserStore + Send + Sync
{
}

/// 任何同时实现了全部子 trait 的类型自动成为 `Store`。
impl<T> Store for T where
    T: AccountStore + RunStore + LogStore + ScheduleStore + PipelineStore + UserStore + Send + Sync
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

/// 东八区固定偏移 —— `timezone: -480` 就是 UTC+8 的分钟数取负。
///
/// # 为什么写死在代码里，不用系统本地时区
///
/// smid 特征体里 `timezone: -480` 是**写死的**东八区，而 `smid` 字段里的
/// `yyyyMMddHHmmss` 必须与它同源。如果时间按运行机器的本地时区算，
/// 一台设在 UTC 的服务器就会自报"我在东八区，但我的时间戳是 UTC"——
/// 这是风控侧一眼可见的不一致。
///
/// 所以"东八区"这个事实在本项目里**只有这一处定义**，需要它的地方都引用这里。
pub fn beijing_offset() -> chrono::FixedOffset {
    // UTC+8 是合法偏移，构造不会失败；真失败说明常量被改错了，那属于代码缺陷。
    chrono::FixedOffset::east_opt(8 * 3600).unwrap_or_else(|| {
        debug_assert!(false, "东八区偏移必须合法");
        // 退化到 UTC：时间偏 8 小时总好过崩溃，而 debug 断言会先把问题暴露出来。
        chrono::FixedOffset::east_opt(0).expect("UTC 偏移必然合法")
    })
}

/// 东八区墙钟的 14 位 `yyyyMMddHHmmss`。
///
/// 见 [`beijing_offset`] 说明为什么固定用东八区。
/// 时间**只能**来自注入的 [`Clock`]，不许直接 `SystemTime::now()`——
/// 否则测试无法钉住上报内容（见 docs/conventions.md §7.2）。
pub fn beijing_time_digits(clock: &dyn Clock) -> String {
    use chrono::TimeZone as _;
    let offset = beijing_offset();
    match offset.timestamp_millis_opt(clock.now_ms()) {
        chrono::LocalResult::Single(t) => t.format("%Y%m%d%H%M%S").to_string(),
        // 东八区**没有夏令时**，所以这两个分支对本偏移不可达。
        // 真出现说明偏移常量被改成了有 DST 的地方。
        _ => String::new(),
    }
}

/// 把一个**不带时区**的本地时刻串按**东八区**解成 epoch 毫秒。
///
/// 解不出来回 `None`（`None` ≠ "没有边界"，调用方自己按三态处置）。
///
/// # 为什么在这里，而不是各任务层各写一份
///
/// 上游那些活动页给的是 `"2025-05-28T00:00:00"` 这种**本地墙钟**串，
/// 判"活动在不在期"必须按东八区解释 —— 按 UTC 解会把边界**整体推后 8 小时**
/// （快到 `endTime` 时多发一次写请求，快到 `startTime` 时少发一次）。
///
/// [`beijing_offset`] 已经是"东八区"在本项目的**唯一**定义，而 `chrono`
/// 只有本 crate 引得到（`mcloud-tasks` / `mcloud-upstream` 的依赖里都没有它）
/// ⇒ 日期算术与偏移一起放这里，别处就不会再出现第二份"自己拆年月日"的实现。
///
/// ⚠ `mcloud-tasks::exchange` 里那份**手写**的解析器（`parse_exchange_time`）
/// 早于本函数，形态也不同（它那边时分秒之间是 `-`）。它**仍然有效**，
/// 且已用一个"与 [`beijing_offset`] 对拍"的用例钉住偏移来源一致 ——
/// 不要因为本函数的出现就顺手去改它（改了是白担一次回归风险）。
///
/// # `fmt` 用 chrono 的格式串
///
/// 例：`"%Y-%m-%dT%H:%M:%S"`。上游形态变了就把 `fmt` 改对，
/// **不要**在这里"兼容多种形态" —— 那会让"上游换了格式"这件事静默消失。
pub fn parse_beijing_time(raw: &str, fmt: &str) -> Option<i64> {
    use chrono::TimeZone as _;
    let naive = chrono::NaiveDateTime::parse_from_str(raw.trim(), fmt).ok()?;
    // 固定偏移下不存在歧义，`single()` 就够了（`ambiguous` 只会出现在有 DST 的时区）。
    beijing_offset()
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp_millis())
}

// ── 事件 ────────────────────────────────────────────────────────────────

/// 事件接收端。
///
/// 实现方负责扇出：写日志、推 SSE、按级别过滤后推送通知。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: RunEvent);
}
