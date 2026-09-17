//! 领域模型。
//!
//! 多账号是**一等公民**：`Account` 覆盖余额、凭据到期、健康度、独立调度，
//! 而不是把账号当成一个附属字段。

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

    #[test]
    fn account_state_emptiness_is_about_all_fields() {
        let mut state = AccountState::new("19****01");
        assert!(state.is_empty());

        state.user_domain_id = Some("1".into());
        assert!(!state.is_empty(), "只有标识也算有内容");
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
