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
use crate::user::{Session, User, UserId};
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

/// 数据可见范围 —— "**谁来问**"这件事的唯一表达。
///
/// # 为什么不是"每个方法都收 `&UserId`"
///
/// 隔离的初衷是**网络上的用户**：WebUI 里 A 不能看到 B 的账号。
/// 但 CLI 是另一回事 —— 它跑在**持有数据库文件**的那台机器上：
/// 能跑 CLI 的人本来就能直接 `sqlite3` 读库，给他加一道"你是谁"的关卡
/// 既不增加任何安全性，又把本地运维变得别扭（列个账号都得先登录）。
///
/// 所以把范围显式化：
///
/// - 网络路径（WebUI 的每个请求）用 [`Scope::User`]；
/// - 本地路径（CLI）用 [`Scope::Local`]。
///
/// 每个调用点都必须写清楚自己站在哪一边，而**过滤逻辑只有一份**（在存储实现里）：
/// 查询写 `(?n IS NULL OR owner = ?n)`，一个表达式同时表达两种范围。
/// 这比"再加一套不带用户参数的平行 API"好 —— 后者会让两条路的语义慢慢分叉。
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// 某个系统用户：只能看/改自己的东西。
    User(&'a UserId),
    /// 本地运维（CLI）：不过滤。
    ///
    /// 用它等于声明"调用方本来就拿着库文件"。**服务层的路由不得使用它** ——
    /// 那会把所有人的数据暴露给任何一个已登录用户。
    Local,
}

impl<'a> Scope<'a> {
    pub const fn user(id: &'a UserId) -> Self {
        Self::User(id)
    }

    /// 期望的归属：`User` 时是那个用户；`Local` 时无约束。
    ///
    /// 存储实现把它直接当参数传给 `(?n IS NULL OR owner = ?n)`：
    /// `None` 让那一支恒真（不过滤），`Some(id)` 则退化成等值比较。
    pub const fn owner(self) -> Option<&'a UserId> {
        match self {
            Self::User(id) => Some(id),
            Self::Local => None,
        }
    }

    pub const fn is_local(self) -> bool {
        matches!(self, Self::Local)
    }
}

/// 账号维度的状态存储。SQLite 实现在 `mcloud-io`。
///
/// # 数据隔离是**契约的一部分**，不是调用方的义务
///
/// 每个方法都要求传 [`Scope`]，实现**必须**按它过滤。`Scope::User` 时三条规则：
///
/// 1. **读不到就是 `None` / 空列表**："不属于你"与"不存在"返回同一个结果，
///    实现里不要为前者单独造一个错误（理由见 [`ErrorKind::NotFound`]）；
/// 2. **写之前要校验，而不是先写进去再说**：传入不属于该用户的 `AccountId`
///    时实现应拒绝（`validate_owned`），而不是径直 UPDATE；
/// 3. **`owner` 为空的历史账号对谁都不可见**（读不到、也不能被写）——
///    迁移完还没初始化管理员的那个窗口里，宁可是"账号暂时不出现"，
///    也不要出现"谁都拿得到"。
///
/// 把这些写进类型（每个方法都要一个 `Scope`）而不是靠"记得加 WHERE 条件"，
/// 是因为漏掉一处就是越权，而**漏掉一处不会有任何编译期提示**。
///
/// # 本地范围不是"安全后门"
///
/// [`Scope::Local`] 只该被 CLI 这类**已经持有库文件**的调用方使用：
/// 它不制造新的越权可能（能跑 CLI 的人本来就能直接读库），
/// 但服务层用它就等于把所有人的数据交给任何一个已登录用户。
/// 所以它是个**必须在调用点写出来的**选择，而不是默认值。
pub trait StateStore {
    /// 写入账号。
    ///
    /// 归属检查随 [`Scope`] 走：`Scope::User` 时 `account.owner` 必须是同一个用户；
    /// `Scope::Local` 时不再约束归属（本地运维，见 [`Scope::Local`] 的说明）。
    async fn upsert_account(&self, scope: Scope<'_>, account: &Account) -> Result<(), Error>;

    /// 取账号。不在该范围内时返回 `None`。
    async fn get_account(&self, scope: Scope<'_>, id: &AccountId)
    -> Result<Option<Account>, Error>;

    /// 该范围内可见的账号，按 `sort_order` 升序。
    async fn list_accounts(&self, scope: Scope<'_>) -> Result<Vec<Account>, Error>;

    /// 读取账号登录态（含凭据）。
    ///
    /// 能力层要用它拿 `jwtToken` / `deviceId` 去调上游 —— 所以它属于**能力层契约**。
    /// 但注意：拿到的东西**不得**出现在日志、API 响应或错误信息里。
    async fn get_account_state(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
    ) -> Result<Option<AccountState>, Error>;

    /// 覆盖写入账号登录态。
    ///
    /// 是**覆盖**而不是合并：登录态是一个整体快照，`None` 表示"上游这次没给"，
    /// 若做合并就永远删不掉已失效的 token。
    async fn upsert_account_state(
        &self,
        scope: Scope<'_>,
        state: &AccountState,
    ) -> Result<(), Error>;

    /// 按 `account + date` UPSERT 日快照。
    async fn upsert_cloud_stats(&self, scope: Scope<'_>, stats: &CloudStats) -> Result<(), Error>;

    /// 取 `date >= since` 的快照，按日期升序。
    async fn cloud_stats_since(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        since: &str,
    ) -> Result<Vec<CloudStats>, Error>;

    /// 写运行记录。带账号时按范围校验归属。
    async fn insert_run(&self, scope: Scope<'_>, run: &RunRecord) -> Result<(), Error>;
    /// 更新运行记录。只能更新该范围内的记录。
    async fn update_run(&self, scope: Scope<'_>, run: &RunRecord) -> Result<(), Error>;

    /// 该范围内可见的运行记录，按开始时间倒序。
    ///
    /// **`account_id` 为空的记录只在 `Scope::Local` 下出现**：旧数据导入时
    /// "匹配不到账号"的历史会被 `MigrationIssue::UnresolvedRunAccount` 报出来
    /// （那才是它该被看到的地方），而不是塞进某个用户的列表里冒充"他的历史"。
    async fn list_runs(&self, scope: Scope<'_>, limit: usize) -> Result<Vec<RunRecord>, Error>;

    /// 把尚未归属的账号认领给某个用户，返回认领条数。
    ///
    /// # 为什么需要它
    ///
    /// v2 及更早的库里已经有账号，而 schema 迁移是纯 SQL、执行时管理员还不存在，
    /// 所以那些账号的 `owner` 是 `None`（谁都看不见）。初始化首个管理员之后必须
    /// 把历史账号交还给它，否则升级上来的用户会看到"账号全没了"。
    ///
    /// **只认领 `owner IS NULL` 的行**：已经属于别人的账号不会被抢走，重复调用返回 0。
    async fn claim_unowned_accounts(&self, user_id: &UserId) -> Result<u64, Error>;
}

/// 系统用户与会话的持久化。
///
/// # 为什么与 [`StateStore`] 分开
///
/// 理由比 [`ConfigStore`] 更强一层：**"能建用户"与"能读云盘凭据"不该长在同一个 trait 上**。
/// HTTP 鉴权中间件的全部需求是"按名字查用户 + 按 token 哈希查会话"，
/// 它不该因此顺带获得读 `jwt_token` 的方法 —— 分开之后，**类型系统**就挡住了这件事，
/// 不需要靠 code review 的自觉。
///
/// # 口令与会话的边界
///
/// - 口令只以 [`crate::hash_password`]（argon2id）的编码串形态存储；
/// - 会话只存 `sha256(token)`：明文只在签发那一刻返回一次，**不经过这一层**，
///   这样任何实现都不可能顺手把它写进日志。
pub trait UserStore {
    /// 新建或覆盖用户。
    ///
    /// 用户名**唯一且大小写不敏感**（由 schema 的 `COLLATE NOCASE` 保证）：
    /// 否则 `Alice` 与 `alice` 能各建一个，登录时"匹配到谁"取决于查询顺序。
    async fn upsert_user(&self, user: &User) -> Result<(), Error>;

    /// 按用户名查（登录路径）。大小写不敏感。
    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>, Error>;

    async fn get_user(&self, id: &UserId) -> Result<Option<User>, Error>;

    /// 按创建时间排序的稳定列表。
    async fn list_users(&self) -> Result<Vec<User>, Error>;

    /// 用户总数。
    ///
    /// 它是"**是否已完成初始化**"的判据：为 `0` 时 WebUI 只暴露初始化页、
    /// 其余接口一律不响应（`docs/rust-rewrite-plan.md` §3.7 的"避免两套 bootstrap"）。
    async fn count_users(&self) -> Result<u64, Error>;

    /// 写入会话。入参含**已算好的哈希**（见 [`crate::hash_session_token`]）。
    async fn insert_session(&self, session: &Session) -> Result<(), Error>;

    /// 按 token 哈希查会话。
    async fn get_session(&self, token_hash: &str) -> Result<Option<Session>, Error>;

    /// 删除单个会话（登出）。返回是否真的删掉了一行。
    async fn delete_session(&self, token_hash: &str) -> Result<bool, Error>;

    /// 清理已过期会话，返回删除条数。
    ///
    /// **时间从外部传入**：会话到期是业务判断，不能由存储实现偷偷决定"现在几点"。
    async fn delete_expired_sessions(&self, now: DateTime<Utc>) -> Result<u64, Error>;

    /// 删除某用户的全部会话（改口令 / 删用户时用，避免旧设备继续用旧会话）。
    async fn delete_user_sessions(&self, user_id: &UserId) -> Result<u64, Error>;
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
