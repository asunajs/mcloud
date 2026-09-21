//! 领域模型。
//!
//! 多账号是**一等公民**：`Account` 覆盖余额、凭据到期、健康度、独立调度，
//! 而不是把账号当成一个附属字段。

use crate::user::UserId;
use serde::{Deserialize, Serialize};

/// 账号标识。
///
/// 用脱敏手机号或昵称派生的稳定字符串，**不使用凭据**作为标识。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 账号快照。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    /// 归属的**系统用户**（[`UserId`](crate::UserId)）。
    ///
    /// # 为什么是 `Option` 而不是文档里写的 `UserId`
    ///
    /// v2 及更早的库里已经有账号数据，而 schema 迁移是**纯 SQL**、
    /// 执行时"首个管理员"还不存在（管理员要靠 `mcloud user init` 之后才有）。
    /// 迁移期硬编一个 owner 只能靠猜，所以历史数据落到 `None`（未归属），
    /// 由初始化首个管理员时**认领**（见 `StateStore::claim_unowned_accounts`）。
    ///
    /// 语义上 `None` 只能出现在"刚迁移完还没初始化管理员"这一个窗口里；
    /// 隔离判据（[`crate::is_visible_to`]）对它的处理是**谁都看不见**，
    /// 而不是"谁都看得见" —— 漏判的代价不对称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
    /// 昵称（优先展示）。
    pub nickname: Option<String>,
    /// 脱敏手机号，形如 `19****01`（前 2 + `****` + 后 2，见 [`crate::pure::mask_phone`]）。
    pub phone_masked: Option<String>,
    pub platform: Option<String>,
    pub enabled: bool,
    pub sort_order: i64,

    // ---- 云朵余额：原项目 `UserState.totalCloud` 从未被采集（写入被注释），这里是一等字段 ----
    /// 当前云朵余额。
    pub cloud_current: Option<i64>,
    pub cloud_updated_at: Option<chrono::DateTime<chrono::Utc>>,

    // ---- 健康度 ----
    /// 凭据到期时间（**epoch 毫秒**，单位见 [`crate::auth`] 模块头）。**只暴露到期时间，不暴露 token 本体。**
    pub auth_expire: Option<i64>,
    pub last_ok_at: Option<chrono::DateTime<chrono::Utc>>,
    pub fail_streak: i64,

    pub note: Option<String>,
}

impl Account {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: AccountId::new(id),
            enabled: true,
            ..Default::default()
        }
    }

    /// 展示名：优先昵称，否则脱敏手机号，否则 ID。
    pub fn display_name(&self) -> &str {
        self.nickname
            .as_deref()
            .or(self.phone_masked.as_deref())
            .unwrap_or_else(|| self.id.as_str())
    }

    /// 凭据是否已过期（`now_millis` 为 **epoch 毫秒**）。
    ///
    /// `None` = 未知，按未过期处理以免误报。
    pub fn is_auth_expired_at(&self, now_millis: i64) -> bool {
        matches!(self.auth_expire, Some(exp) if exp <= now_millis)
    }

    /// 这个云盘账号对该系统用户是否可见。
    ///
    /// **未归属（`owner == None`）对谁都不可见** —— 迁移完还没初始化管理员的窗口里，
    /// 宁可是"账号暂时不出现"，也不要出现"谁都拿得到"。
    pub fn is_visible_to(&self, viewer: &UserId) -> bool {
        match &self.owner {
            Some(owner) => crate::user::is_visible_to(owner, viewer),
            None => false,
        }
    }
}

/// 云朵日快照。
///
/// 语义照搬参考项目 caiyun 的 `CloudStats{Date,CloudCount,CloudDiff,CloudDiffWeek}`，
/// 唯一键为 `account + date`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudStats {
    pub account_id: AccountId,
    /// `YYYY-MM-DD`（`Asia/Shanghai`）。
    pub date: String,
    pub cloud_count: i64,
    /// 与上一个**有记录**之日之差。
    pub cloud_diff: i64,
    /// 与 7 天前（或最早记录）之差。
    pub cloud_diff_week: i64,
}

/// 运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Done,
    Failed,
}

impl RunStatus {
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// 运行记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub account_id: Option<AccountId>,
    /// 能力 code（如 `sign`）。
    pub capability: String,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 本次净增云朵（可负）。
    pub cloud_gained: Option<i64>,
    /// 运行后余额。
    pub cloud_after: Option<i64>,
    pub message: Option<String>,
}

// ============================ 能力注册表 ============================

/// 能力分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Daily,
    Activity,
    Query,
    Exchange,
    Maintenance,
}

/// 能力性质。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// 有副作用（签到、领取、兑换）。
    Action,
    /// 无副作用、可安全反复执行（查余额、查待领取奖品）。
    ///
    /// `Query` 不写运行历史、不产生日志噪音，输出以结构化卡片呈现。
    Query,
}

/// 风险等级。影响默认并发与前端二次确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    /// 消耗资源但可恢复（如领取）。
    Medium,
    /// 不可逆消耗（如兑换扣云朵）—— 前端必须二次确认并显示影响面。
    High,
}

/// 多账号执行方式。默认串行，防风控。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountMode {
    #[default]
    Sequential,
    Parallel,
}

/// 参数控件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Number,
    Bool,
    Select,
    /// 多选（账号选择走这一类）。
    MultiSelect,
    /// 兜底：复杂入参用原始 JSON。
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// 声明式字段描述。
///
/// 前端表单与结果卡片**全部由它生成**，新增能力不需要改前端。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<SelectOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl FieldSpec {
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Text,
            required: false,
            default: None,
            options: None,
            help: None,
        }
    }

    pub fn bool(key: impl Into<String>, label: impl Into<String>, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Bool,
            required: false,
            default: Some(serde_json::Value::Bool(default)),
            options: None,
            help: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_default(mut self, value: serde_json::Value) -> Self {
        self.default = Some(value);
        self
    }
}

// ============================ 账号登录态 ============================

/// 账号登录态 —— 旧 `data/state.json` 里 `accounts[<手机号>]` 的内容。
///
/// # 为什么单独一张表、单独一个类型
///
/// 这里面是**可直接复用的凭据**（`jwtToken` 能直接冒充登录用户）。
/// 把它和 [`Account`]（昵称/余额/健康度，会被 `/api/accounts` 直接吐给前端）
/// 放在一起，等于每次列表查询都把凭据读进内存、随时可能被顺手序列化出去。
/// 分表之后，"导出账号列表"这类操作天然拿不到凭据。
///
/// **`Debug` 是手写的**：和 [`crate::auth::AuthInfo`] 同样的理由 ——
/// 否则任何一次 `tracing::debug!(?state)` 都会把凭据写进日志。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountState {
    pub account_id: AccountId,
    pub jwt_token: Option<String>,
    pub device_id: Option<String>,
    /// 上游的用户域标识（纯标识，不是凭据，保留以便排障）。
    pub user_domain_id: Option<String>,
    /// 话费红包活动专用 token。
    pub cloud_phone_redpack_token: Option<String>,
    /// **完整手机号**（v6 起）。
    ///
    /// # 它现在的角色：标识，不是"拼凭据的原料"
    ///
    /// `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`，
    /// 中间那段确实是完整手机号。但**不要拿它去拼** —— 整串 auth 由
    /// [`Self::authorization`] 原样存着，见那里的说明。
    ///
    /// 这一列留下来是因为它有两个别处替代不了的用途：
    /// 日志里认出"签的是谁"（掩码不够）、以及排障时确认端与号对得上。
    ///
    /// 补上它之前，任何要用 `Authorization` 的能力都只能让调用方在命令行上
    /// 重新输一遍手机号（`mcloud signin --phone`），而那是个**缺口不是设计**。
    ///
    /// # 为什么放这张表
    ///
    /// 它与 `user_domain_id` 同类：**是标识不是凭据**，但**不该被 `/api/accounts`
    /// 吐给前端**（那张表里只放掩码）。所以留在凭据表里，与"列出账号"那条最常走的
    /// 查询天然隔开。
    ///
    /// `v6` 之前登录的账号这一列是 `NULL` —— 那是**如实记录"当时没存"**，
    /// 不要拿掩码去填。
    pub phone: Option<String>,
    /// **原样存下的 `Authorization` 头值**（`Basic …`，v7 起）。
    ///
    /// # 为什么存整串，而不是拆成"前缀 + 手机号 + token"再拼回去
    ///
    /// 拆开再拼是个**绕远路**，而且是错的：
    ///
    /// | | 拆开再拼（v6 的做法） | 整串存（现在的做法） |
    /// |---|---|---|
    /// | 依赖 | 必须凑齐三段，缺一段就用不了 | 一段都不用凑 |
    /// | 前缀 | 由调用点**硬编码**（App 写死 `mobile`）—— 猜错就是静默发错头 | 前缀本来就在串里 |
    /// | 旧数据 | 导入时只留下第三段，手机号与前缀一起丢了 | 旧项目的 `auth` 本来就是这个形状，直接搬 |
    ///
    /// 旧项目 `mcloud-sign` 的 `asign.json` 里存的 `auth` 就是**这个值本身**
    /// （`Basic base64("mobile:<手机号>:<authToken>")`），它一直用得好好的。
    /// 我们在重写时把它解码、拆成三份、只留 token，然后要求用户每次在命令行上
    /// 把手机号补回来 —— 那是自找的。
    ///
    /// # 与 [`Self::phone`] 的关系
    ///
    /// 两者**冗余但不同源**：`phone` 是给人看的标识，`authorization` 是给上游用的
    /// 凭据。写入侧应当同时给（登录时两者都有）；读取侧**优先用整串**，
    /// 只有它是 `NULL` 的老行才退回"用 `phone` + `jwt_token` 拼"那条路。
    ///
    /// # ⚠ 这一列只装「**拿到的**」，不装「拼出来的」
    ///
    /// 合法的来源只有两个，都是"服务端或旧配置原样给的"：
    ///   1. 登录响应 —— `mcloud_capabilities::login` 里那个 `auth` 整串；
    ///   2. 旧项目 `asign.json` 的 `auth` 字段（迁移回填）。
    ///
    /// **不要**把本地合成的串写进来。读取侧会**优先采信**它，而整串路径
    /// **不校验前缀**（前缀就在串里，没有可校验的对象）—— 于是一串拼错的凭据
    /// 一旦落库，就会被永久当成原文使用，现象只是"忽然回 90001 未登录"。
    /// 典型反例：`--phone` 那条老路拼出来的串前缀固定是 `mobile`，
    /// 拿它去补一个 `pc:` 端账号，存下去就是**永久错的**。
    ///
    /// 补录手机号时只写 [`AccountState::phone`] 即可 —— 下次照旧现拼，前缀闸照旧生效。
    pub authorization: Option<String>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl std::fmt::Debug for AccountState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /// 只暴露"有没有"与长度：足以判断"状态是否被清空"，又不泄漏可复用的内容。
        fn describe(value: &Option<String>) -> Option<String> {
            value
                .as_ref()
                .map(|v| format!("<redacted {} 字符>", v.chars().count()))
        }
        f.debug_struct("AccountState")
            .field("account_id", &self.account_id)
            .field("jwt_token", &describe(&self.jwt_token))
            .field("device_id", &describe(&self.device_id))
            // 纯标识，保留
            .field("user_domain_id", &self.user_domain_id)
            .field(
                "cloud_phone_redpack_token",
                &describe(&self.cloud_phone_redpack_token),
            )
            // 手机号不是凭据，但打日志时没有任何理由用完整的 —— 与
            // `mcloud_app::signin::SignInCredential` 的 `Debug` 同一个口径：
            // **掩码**（`19****16`），而不是 `<redacted>`。这样排障时还看得出是哪个账号。
            .field("phone", &self.phone.as_deref().map(crate::pure::mask_phone))
            // 整串 auth 是**凭据**（解码后就是 `mobile:<手机号>:<token>`），
            // 所以只暴露"有没有 + 多长"，与 `jwt_token` 同一个口径。
            .field("authorization", &describe(&self.authorization))
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}

impl AccountState {
    pub fn new(account_id: impl Into<String>) -> Self {
        Self {
            account_id: AccountId::new(account_id),
            ..Default::default()
        }
    }

    /// 是否一个字段都没有 —— 空状态没有迁移价值，导入时应被跳过。
    pub fn is_empty(&self) -> bool {
        self.jwt_token.is_none()
            && self.device_id.is_none()
            && self.user_domain_id.is_none()
            && self.cloud_phone_redpack_token.is_none()
            // `phone` 也算 —— 否则一个"只存了手机号"的行会被判成空状态，
            // 而它对 `mcloud signin --account` 是**唯一**可用来源。
            && self.phone.is_none()
            // 整串 auth 同理：v7 之后它才是真正拿来用的那个凭据。
            && self.authorization.is_none()
    }
}

/// 账号的 **smid 状态**：那份绑定账号的设备特征，与最近一次上报换回的 `deviceId`。
///
/// # 为什么不并进 [`AccountState`]
///
/// 同一个主体（这个账号在这台设备上的状态）、同一张表，但**写入契约相反**：
/// `AccountState` 是"整体覆盖的凭据快照"（重新登录要把失效的 token 冲掉），
/// 而这里的两列必须活过重新登录 —— 换凭据不该换掉设备长相，换长相也不该动凭据。
/// 放进同一个结构里，"覆盖"就会顺手把特征清掉（登录路径根本不知道它的存在）。
/// 所以存储层给它们配了独立的方法（见 `StateStore` 的 smid 那三条）。
///
/// # 这一层只搬文本
///
/// `fingerprint_json` 是 `mcloud-upstream::smid::SmFingerprint` 的序列化结果。
/// 类型放在 core 会让 core 反向依赖协议层，所以这里存 JSON 串、由调用方负责解析 ——
/// 代价是"库里那串读不出来"必须由调用方处理（那里的规矩是**报错，不是悄悄重掷一份**）。
///
/// # 刻意不派生 `Serialize`
///
/// 这两样都不是能吐给前端的东西：`device_id` 是第三方风控服务给这台设备的标识。
/// 少一个 `Serialize`，就少一条"顺手塞进响应"的路。
#[derive(Clone, PartialEq, Eq)]
pub struct SmidState {
    pub account_id: AccountId,
    /// 绑定账号的设备特征（JSON 文本）。`None` = 还没绑过。
    pub fingerprint_json: Option<String>,
    /// 最近一次上报换回的 `deviceId`（含 `B` 前缀）。**会变**，只用作下一次上报的 `box`。
    pub device_id: Option<String>,
}

impl std::fmt::Debug for SmidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 与 AccountState 同一个口径：只暴露"有没有"与长度，足以判断"有没有被清掉"
        fn describe(value: &Option<String>) -> Option<String> {
            value
                .as_ref()
                .map(|v| format!("<redacted {} 字符>", v.chars().count()))
        }
        f.debug_struct("SmidState")
            .field("account_id", &self.account_id)
            .field("fingerprint_json", &describe(&self.fingerprint_json))
            .field("device_id", &describe(&self.device_id))
            .finish()
    }
}

impl SmidState {
    pub fn new(account_id: impl Into<String>) -> Self {
        Self {
            account_id: AccountId::new(account_id),
            fingerprint_json: None,
            device_id: None,
        }
    }

    /// 两样都没有 —— 这个账号还从没上报过指纹。
    pub fn is_empty(&self) -> bool {
        self.fingerprint_json.is_none() && self.device_id.is_none()
    }
}

// ============================ 定时任务 ============================

/// 定时任务的执行目标（旧版的 `task?` 与 `pipelineId?` 二选一）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTarget {
    /// 单个能力，值是能力 `code`（旧版的"任务子项目名"）。
    Capability { code: String },
    /// 任务组合。
    Pipeline { id: String },
}

/// 定时任务的上次执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    Ok,
    Error,
    /// 到点触发时同「任务+账号」已在运行，本次跳过（不排队）。
    Skipped,
}

/// 定时任务定义（旧 `data/schedules.json` → `schedules` 表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    /// 归属的**系统用户**；`None` = 未归属（v3 及以前的历史数据），**谁都看不见**。
    ///
    /// 与 [`Account::owner`] 同一套语义、同一个认领时机（`mcloud user init`）——
    /// 理由见那里的说明：迁移是纯 SQL，执行时管理员还不存在，硬编一个只能是猜。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
    pub name: String,
    /// 缺失即无法执行 —— 导入时会作为问题报出，而不是猜一个目标。
    pub target: Option<ScheduleTarget>,
    /// 5 段或 6 段（含秒）cron。
    pub cron: String,
    /// 归一化后一定非空；旧版缺省 `Asia/Shanghai`。
    pub timezone: String,
    /// 目标账号。**空表示全部账号**（不是"没有账号"）。
    pub account_ids: Vec<AccountId>,
    /// 透传给能力的参数。
    pub body: Option<serde_json::Value>,
    pub enabled: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<ScheduleStatus>,
    pub last_error: Option<String>,
}

// ============================ 任务组合 ============================

/// 组合的终态。
///
/// **与 [`ScheduleStatus`] 刻意不同**：旧版的组合没有 `skipped`，
/// 却多一个 `partial`（部分步骤失败但没中断）。别把两者合并成一个枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStatus {
    Ok,
    Error,
    /// 有步骤失败但未中断后续步骤。
    Partial,
}

/// 组合中的单个步骤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineStep {
    /// 能力 `code`。
    pub task: String,
    pub body: Option<serde_json::Value>,
    /// 该步失败后是否继续；`None` 表示取组合级 `stop_on_error` 的反值。
    pub continue_on_error: Option<bool>,
}

/// 任务组合定义（旧 `data/pipelines.json` → `pipelines` 表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: String,
    /// 归属的**系统用户**；语义与 [`Schedule::owner`] 完全一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
    pub name: String,
    pub description: Option<String>,
    /// 有序步骤，执行顺序即数组顺序。
    pub steps: Vec<PipelineStep>,
    /// 目标账号。**空表示全部账号。**
    pub account_ids: Vec<AccountId>,
    pub account_mode: AccountMode,
    pub stop_on_error: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_status: Option<PipelineStatus>,
    pub last_error: Option<String>,
    pub last_duration_ms: Option<i64>,
}

impl Pipeline {
    /// 某一步失败后是否继续执行后续步骤（步骤级优先，其次取组合级的反值）。
    pub fn continues_after_failure(&self, step: &PipelineStep) -> bool {
        step.continue_on_error.unwrap_or(!self.stop_on_error)
    }
}

// ============================ 定时抢兑 ============================

/// 「账号 × 奖品」绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeBinding {
    pub account_id: AccountId,
    pub prize_id: i64,
    /// 仅展示用。
    pub prize_name: Option<String>,
}

/// 抢兑窗口的默认值（旧 `exchange-schedule.ts` 的 `DEFAULT_WINDOWS`）。
///
/// 这两个时刻是 10 点与 12 点开抢前的 20 秒 —— 提前量由旧版实测得出，不是随手写的。
pub const DEFAULT_EXCHANGE_WINDOWS: [&str; 2] = ["09:59:40", "11:59:40"];

/// 定时抢兑配置（旧 `data/exchange.json`，单行表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangeConfig {
    pub enabled: bool,
    /// 抢兑窗口时刻（`HH:mm:ss`，本地时区），已排序去重。
    pub windows: Vec<String>,
    pub bindings: Vec<ExchangeBinding>,
    /// 由本模块创建并维护的定时任务 id。
    ///
    /// 必须存下来：保存配置时要整体重建这些任务，没有这份清单就删不掉上一次留下的。
    pub schedule_ids: Vec<String>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            windows: DEFAULT_EXCHANGE_WINDOWS
                .iter()
                .map(|w| (*w).to_string())
                .collect(),
            bindings: Vec::new(),
            schedule_ids: Vec::new(),
            updated_at: None,
        }
    }
}

// ============================ 界面偏好 ============================

/// 允许记忆的页面标识（与前端左侧菜单一致）。
pub const WEBUI_PAGES: [&str; 6] = [
    "tasks",
    "pipelines",
    "schedules",
    "accounts",
    "history",
    "config",
];

/// 界面偏好默认页。
pub const DEFAULT_WEBUI_PAGE: &str = "tasks";

/// 界面偏好（旧 `data/webui.json`，单行表）。
///
/// 存在服务端而不是浏览器 `localStorage`：这是旧版的既有选择，
/// 好处是换浏览器/换设备后偏好还在。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebuiConfig {
    /// 记住上次所在页面。
    pub remember_last_page: bool,
    /// 上次所在页面标识。**必须落在 [`WEBUI_PAGES`] 内。**
    pub last_page: String,
    /// 组合运行结果里的日志默认展开。
    pub log_expanded: bool,
    /// 任务/组合默认异步执行（立即返回，前端轮询结果）。
    pub background_run: bool,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for WebuiConfig {
    fn default() -> Self {
        Self {
            remember_last_page: true,
            last_page: DEFAULT_WEBUI_PAGE.to_string(),
            log_expanded: false,
            background_run: false,
            updated_at: None,
        }
    }
}

impl WebuiConfig {
    /// 把 `last_page` 收拢到白名单内（非法值回落默认）。
    pub fn normalize_page(&mut self) {
        if !WEBUI_PAGES.contains(&self.last_page.as_str()) {
            self.last_page = DEFAULT_WEBUI_PAGE.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_prefers_nickname_then_phone_then_id() {
        let mut acc = Account::new("acc-1");
        assert_eq!(acc.display_name(), "acc-1");

        acc.phone_masked = Some("19****01".into());
        assert_eq!(acc.display_name(), "19****01");

        acc.nickname = Some("账号2".into());
        assert_eq!(acc.display_name(), "账号2");
    }

    #[test]
    fn auth_expiry_unknown_is_treated_as_valid() {
        let mut acc = Account::new("acc-1");
        assert!(!acc.is_auth_expired_at(1_789_000_000_000));

        // 用真实量级的毫秒值，避免测试数据教出错误的单位印象
        acc.auth_expire = Some(1_788_999_999_999);
        assert!(
            acc.is_auth_expired_at(1_789_000_000_000),
            "到期时间早于现在应判为过期"
        );

        acc.auth_expire = Some(1_789_000_000_001);
        assert!(!acc.is_auth_expired_at(1_789_000_000_000));
    }

    #[test]
    fn account_mode_defaults_to_sequential() {
        assert_eq!(AccountMode::default(), AccountMode::Sequential);
    }

    #[test]
    fn field_spec_serializes_for_frontend() {
        let spec = FieldSpec::bool("skipWait", "跳过等待", false)
            .with_help("关闭后等到整点开抢")
            .with_default(serde_json::json!(true));
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["kind"], "bool");
        assert_eq!(json["key"], "skipWait");
        assert_eq!(json["default"], true);
        assert_eq!(json["required"], false);
    }

    #[test]
    fn run_status_terminality() {
        assert!(!RunStatus::Running.is_terminal());
        assert!(RunStatus::Done.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
    }

    /// 凭据不得出现在 `Debug` 里，但纯标识（`user_domain_id`）要保留 —— 它有排障价值。
    ///
    /// 注意这条只管**运行期输出**。同一个值若出现在要进版本库的夹具/字面量里，
    /// 就必须是合成值：界线是"能不能进 GitHub"，不是"能不能进日志"。
    /// 见 `crate::test_support::assert_only_synthetic_identifiers`。
    #[test]
    fn account_state_debug_redacts_credentials() {
        let state = AccountState {
            jwt_token: Some("SECRETJWT".into()),
            device_id: Some("SECRETDEVICE".into()),
            user_domain_id: Some("1990000000000000001".into()),
            cloud_phone_redpack_token: Some("SECRETREDPACK".into()),
            ..AccountState::new("19****01")
        };

        let shown = format!("{state:?}");
        assert!(!shown.contains("SECRETJWT"), "jwt 不得泄漏: {shown}");
        assert!(
            !shown.contains("SECRETDEVICE"),
            "deviceId 不得泄漏: {shown}"
        );
        assert!(
            !shown.contains("SECRETREDPACK"),
            "红包 token 不得泄漏: {shown}"
        );
        assert!(shown.contains("redacted"), "应标明被脱敏: {shown}");
        assert!(
            shown.contains("1990000000000000001"),
            "纯标识应保留以便排障: {shown}"
        );
    }

    /// 手机号既不是凭据也不是"纯标识"：它**要掩码**，不能原样打出来，也不能整段抹掉。
    ///
    /// 掩码而不是 `<redacted>` 的理由是排障：多账号时"这是哪个账号"必须看得出来，
    /// 而 `19****16` 已经足够。口径与 `mcloud_app::signin::SignInCredential` 的 `Debug` 一致。
    #[test]
    fn account_state_debug_masks_the_phone() {
        let state = AccountState {
            phone: Some("19900000001".into()),
            ..AccountState::new("19****01")
        };

        let shown = format!("{state:?}");
        assert!(!shown.contains("19900000001"), "完整号码不得出现: {shown}");
        assert!(shown.contains("19****01"), "应掩码而非抹掉: {shown}");
    }

    #[test]
    fn account_state_emptiness_is_about_all_fields() {
        let mut state = AccountState::new("19****01");
        assert!(state.is_empty());

        state.user_domain_id = Some("1".into());
        assert!(!state.is_empty(), "只有标识也算有内容");

        // 只存了手机号的行**不是**空状态：它是 `mcloud signin --account` 的唯一来源，
        // 判成空就会被导入流程当"没有内容"跳过。
        let only_phone = AccountState {
            phone: Some("19900000001".into()),
            ..AccountState::new("19****01")
        };
        assert!(!only_phone.is_empty());
    }

    /// `ScheduleTarget` 用内部标签序列化，便于直接落库/出给前端而不产生二义。
    #[test]
    fn schedule_target_serializes_with_kind_tag() {
        let capability = serde_json::to_value(ScheduleTarget::Capability {
            code: "sign".into(),
        })
        .unwrap();
        assert_eq!(capability["kind"], "capability");
        assert_eq!(capability["code"], "sign");

        let pipeline = serde_json::to_value(ScheduleTarget::Pipeline { id: "p-1".into() }).unwrap();
        assert_eq!(pipeline["kind"], "pipeline");
        assert_eq!(pipeline["id"], "p-1");

        let back: ScheduleTarget = serde_json::from_value(capability).unwrap();
        assert_eq!(
            back,
            ScheduleTarget::Capability {
                code: "sign".into()
            }
        );
    }

    /// 步骤级 `continue_on_error` 优先，否则取组合级 `stop_on_error` 的**反值**。
    #[test]
    fn pipeline_step_continuation_falls_back_to_pipeline_flag() {
        fn pipeline(stop_on_error: bool) -> Pipeline {
            Pipeline {
                id: "p-1".into(),
                owner: None,
                name: "轻量巡检".into(),
                description: None,
                steps: Vec::new(),
                account_ids: Vec::new(),
                account_mode: AccountMode::Sequential,
                stop_on_error,
                created_at: None,
                updated_at: None,
                last_run_at: None,
                last_status: None,
                last_error: None,
                last_duration_ms: None,
            }
        }
        let step = |coe: Option<bool>| PipelineStep {
            task: "sign".into(),
            body: None,
            continue_on_error: coe,
        };

        let strict = pipeline(true);
        assert!(
            !strict.continues_after_failure(&step(None)),
            "stopOnError=true 时默认不继续"
        );
        assert!(
            strict.continues_after_failure(&step(Some(true))),
            "步骤级可以覆盖组合级"
        );
        assert!(!strict.continues_after_failure(&step(Some(false))));

        let lenient = pipeline(false);
        assert!(lenient.continues_after_failure(&step(None)));
    }

    #[test]
    fn webui_config_defaults_and_page_whitelist() {
        let defaults = WebuiConfig::default();
        assert!(defaults.remember_last_page);
        assert_eq!(defaults.last_page, DEFAULT_WEBUI_PAGE);
        assert!(!defaults.log_expanded);

        let mut bogus = WebuiConfig {
            last_page: "../../etc/passwd".into(),
            ..Default::default()
        };
        bogus.normalize_page();
        assert_eq!(bogus.last_page, DEFAULT_WEBUI_PAGE);

        let mut ok = WebuiConfig {
            last_page: "history".into(),
            ..Default::default()
        };
        ok.normalize_page();
        assert_eq!(ok.last_page, "history");
    }
}
