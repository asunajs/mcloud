//! 旧项目（mcloud-sign）数据文档的读取与迁移映射 —— **纯逻辑，无 IO**。
//!
//! # 为什么单独一层
//!
//! 迁移的失败方式是**静默的**：把历史记录挂到错误的账号上不会报错，只会让人看到
//! 一份"看起来正常但串了号"的数据；把「旧版其实从未生效」的定时任务照搬过来，
//! 更会让人以为它一直在跑。所以这里不碰文件，只做两件可被穷尽测试的事：
//!
//! 1. **原样还原旧文档的形状**（字段名/可选性），并用真实数据生成的脱敏夹具钉住；
//! 2. **逐条复刻旧版的规整规则与查找规则**，把"旧版会怎么做"和"新版应该怎么做"分开表达，
//!    差异点显式暴露成 [`ReferenceAdvice`]，交给迁移报告而不是静默决定。
//!
//! 夹具：`test/fixtures/legacy/`，由 `scripts/gen-legacy-fixtures.cjs` 从真实数据脱敏生成。
//!
//! # 旧文档一览
//!
//! | 文件 | 内容 | 谁在读它 |
//! |---|---|---|
//! | `asign.json` | 账号（`auth` + `nickname`） | 账号来源，唯一的"真配置" |
//! | `data/state.json` | 各账号 `jwtToken`/`deviceId`/… | 键是**手机号原文**（旧 `accountKey`） |
//! | `data/history.json` | 运行历史（最多 200 条） | `account` 存的是**展示名**，不是标识 |
//! | `data/pipelines.json` | 任务组合 | 有已废弃字段需要迁移 |
//! | `data/schedules.json` | 定时任务 | |
//! | `data/exchange.json` | 定时抢兑 | 窗口 `HH:mm:ss` → 6 段 cron |
//! | `data/webui.json` | 界面偏好 | |
//!
//! # 三个必须知道的旧版怪癖（**不要"顺手修好"**）
//!
//! 1. **`state.json` 的键是手机号原文**，而 `history.json` 的 `account` 是展示名 ——
//!    两者指向同一个账号却用了两套标识，迁移必须分别映射（见 [`LegacyAccountView`]）。
//! 2. **纯数字字符串一律被当作"序号"**：旧 `findAccount` 里 `/^\d+$/` 分支在昵称匹配之前，
//!    于是把手机号当字符串传进去**永远匹配不到**，定时任务会每次记 `账号 x 不存在` 后什么都不做。
//! 3. `webui` 的两个布尔字段默认值方向相反：`rememberLastPage` 只要不是 `false` 就是 `true`，
//!    而 `logExpanded` 必须严格等于 `true`。见 [`normalize_webui`]。

use crate::auth::{account_id, account_label, parse_auth, resolve_phone};
use crate::model::{AccountId, WebuiConfig};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 旧版历史文件的上限（`runner.ts`：超过就 `slice(-200)`）。
pub const LEGACY_HISTORY_CAP: usize = 200;

/// 旧版允许记忆的页面标识（`webui-config.ts` 的 `WEBUI_PAGES`）。
///
/// 白名单本身要和前端菜单共用同一份，所以它住在 [`crate::model`]；
/// 这里只转出去，免得 legacy 的使用者为了一个常量多写一次 import。
pub use crate::model::WEBUI_PAGES;

/// 旧版抢兑窗口的默认值（`exchange-schedule.ts` 的 `DEFAULT_WINDOWS`）。
///
/// 同 [`WEBUI_PAGES`]：默认值属于新领域（服务端也要用），所以住在 [`crate::model`]。
pub use crate::model::DEFAULT_EXCHANGE_WINDOWS;

// ═══════════════════════════════════════════════════════════════════════════
// 1. 旧文档类型
// ═══════════════════════════════════════════════════════════════════════════

/// 旧 `asign.json`。
///
/// 账号字段**刻意全为可选**：`nickname`/`phone` 在真实配置里经常缺席，
/// 缺席时要从 `auth` 现场解析 —— 这正是 [`resolve_phone`] 存在的理由。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LegacyConfig {
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub caiyun: Vec<LegacyAccountConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LegacyAccountConfig {
    /// `Basic base64("平台:手机号:token")`。
    #[serde(default)]
    pub auth: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

impl LegacyAccountConfig {
    /// 该账号的昵称（空串按缺席处理，对齐 JS 的 falsy）。
    pub fn nickname_or_none(&self) -> Option<&str> {
        self.nickname.as_deref().filter(|n| !n.is_empty())
    }
}

/// 旧 `data/state.json`：`{ accounts: { <手机号原文>: UserState } }`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyStateFile {
    #[serde(default)]
    pub accounts: BTreeMap<String, LegacyAccountState>,
}

/// 旧 `UserState`。
///
/// `unknown` 用 `flatten` 兜住**旧版本写过、现在不再产生**的字段（例如 `totalCloud`：
/// 旧代码把它的写入注释掉了，所以真实数据里没有，但别人机器上可能有）。
/// 迁移时这些字段会被原样保留进"未识别字段"清单并报告，而不是静默丢弃。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyAccountState {
    #[serde(default)]
    pub jwt_token: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub user_domain_id: Option<String>,
    #[serde(default)]
    pub cloud_phone_redpack_token: Option<String>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl LegacyAccountState {
    /// 是否一个凭据都没有（全空状态没有迁移价值）。
    pub fn is_empty(&self) -> bool {
        self.jwt_token.as_deref().unwrap_or("").is_empty()
            && self.device_id.as_deref().unwrap_or("").is_empty()
            && self.user_domain_id.as_deref().unwrap_or("").is_empty()
            && self
                .cloud_phone_redpack_token
                .as_deref()
                .unwrap_or("")
                .is_empty()
    }

    /// 出现过的未识别字段名（迁移报告用）。
    pub fn unknown_fields(&self) -> Vec<&str> {
        self.unknown.keys().map(String::as_str).collect()
    }
}

/// 旧 `data/history.json`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyHistoryFile {
    #[serde(default)]
    pub runs: Vec<LegacyRun>,
}

/// 旧 `RunRecord`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRun {
    /// 任务子项目名（`sign` / `prizes` / `exchange` …）。
    #[serde(default)]
    pub task: String,
    /// **展示名**（昵称优先，否则脱敏手机号）—— 不是账号标识。见模块头怪癖 1。
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub logs: Vec<LegacyLogEntry>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// 旧日志条目（`PushLogEntry`）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyLogEntry {
    /// 数值严重级。真实数据里只出现过 `0`（error）/ `3`（info）/ `4`（debug）。
    #[serde(default)]
    pub level: Option<i64>,
    /// 语义类型：`info` / `start` / `success` / `debug` / `error`。
    ///
    /// **同一个 `level` 会对应多个 `type`**（`3` 同时对应 info/start/success），
    /// 所以语义必须看 `type`，不能只看 `level`。
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
}

/// 旧 `data/pipelines.json`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyPipelinesFile {
    #[serde(default)]
    pub pipelines: Vec<LegacyPipeline>,
}

/// 旧 `PipelineDef`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyPipeline {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub steps: Vec<LegacyStep>,
    /// **已废弃**：单账号。读取时迁移为 `accounts`（见 [`normalize_pipeline_accounts`]）。
    #[serde(default, rename = "account")]
    pub legacy_account: Option<Value>,
    #[serde(default)]
    pub accounts: Option<Vec<Value>>,
    /// `sequential`（默认，防风控）/ `parallel`。
    #[serde(default)]
    pub account_mode: Option<String>,
    #[serde(default)]
    pub stop_on_error: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    /// `ok` / `error` / `partial`。
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_duration_ms: Option<i64>,
}

/// 旧 `PipelineStep`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyStep {
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub body: Option<Value>,
    /// 该步失败后是否继续；缺省取组合级 `stopOnError` 的反值。
    #[serde(default)]
    pub continue_on_error: Option<bool>,
    /// **已废弃**：步骤级账号。旧 `normalizeStored` 会显式丢掉它，这里也丢。
    #[serde(default, rename = "account")]
    pub legacy_account: Option<Value>,
}

/// 旧 `data/schedules.json`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacySchedulesFile {
    #[serde(default)]
    pub schedules: Vec<LegacySchedule>,
}

/// 旧 `ScheduleDef`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySchedule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// 任务子项目名（与 `pipelineId` 二选一）。
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub pipeline_id: Option<String>,
    /// 5 段或 6 段（含秒）cron。
    #[serde(default)]
    pub cron: String,
    /// 缺省 `Asia/Shanghai`。
    #[serde(default)]
    pub timezone: Option<String>,
    /// 目标账号：序号（数字或纯数字字符串）/ 昵称。见模块头怪癖 2。
    #[serde(default)]
    pub account: Option<Value>,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    /// `ok` / `error` / `skipped`。
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// 旧 `data/exchange.json`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyExchangeFile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<LegacyBinding>,
    /// 由该模块创建并维护的定时任务 id（迁移时用来识别"自动生成"的调度）。
    #[serde(default)]
    pub schedule_ids: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// 旧 `ExchangeBinding`。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyBinding {
    #[serde(default)]
    pub account: Option<Value>,
    #[serde(default)]
    pub prize_id: Option<Value>,
    #[serde(default)]
    pub prize_name: Option<String>,
}

/// 旧 `data/webui.json`。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyWebuiFile {
    #[serde(default)]
    pub remember_last_page: Option<bool>,
    #[serde(default)]
    pub last_page: Option<String>,
    #[serde(default)]
    pub log_expanded: Option<bool>,
    #[serde(default)]
    pub background_run: Option<bool>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. 旧版规整规则（逐条复刻）
// ═══════════════════════════════════════════════════════════════════════════

/// 迁移期的错误。**每一条都必须能指向一个具体的、可人工修复的问题。**
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyError {
    #[error("时刻格式应为 HH:mm:ss，收到「{0}」")]
    BadExchangeWindow(String),
    #[error("至少需要一个抢兑时刻")]
    NoExchangeWindow,
    #[error("奖品 ID 无效：{0}")]
    BadPrizeId(String),
    #[error(
        "账号掩码冲突：{account_id} 同时来自 {keys:?}（两个手机号的前 2 位与后 2 位相同）。\
         继续迁移会把两个账号的数据混在一起，因此中止；请为其中一个账号显式指定不同的 id"
    )]
    MaskCollision {
        account_id: String,
        keys: Vec<String>,
    },
}

/// 复刻 `windowToCron`：`HH:mm:ss` → 6 段 cron（含秒）。
///
/// 注意 `Number()` 会**去掉前导零**：`"09:59:40"` → `"40 59 9 * * *"`。
/// 这不是笔误 —— croner 与 Rust 侧的 cron 解析器都接受无前导零的字段。
pub fn exchange_window_to_cron(window: &str) -> Result<String, LegacyError> {
    let parts: Vec<&str> = window.trim().split(':').collect();
    if parts.len() != 3 {
        return Err(LegacyError::BadExchangeWindow(window.to_string()));
    }
    let (hh, mm, ss) = (parts[0], parts[1], parts[2]);
    let ok2 = |s: &str, max: u32| -> Option<u32> {
        if s.len() != 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let n: u32 = s.parse().ok()?;
        (n <= max).then_some(n)
    };
    // 小时 00-23，分秒 00-59，且都必须写满两位
    let (Some(h), Some(m), Some(s)) = (ok2(hh, 23), ok2(mm, 59), ok2(ss, 59)) else {
        return Err(LegacyError::BadExchangeWindow(window.to_string()));
    };
    Ok(format!("{s} {m} {h} * * *"))
}

/// 复刻 `normalizeWindows`：`trim` → 5 位补 `:00` → 逐个校验 → **去重并排序**。
///
/// 排序是字符串序，对定长 `HH:mm:ss` 等价于时间序。
pub fn normalize_exchange_windows(raw: &[String]) -> Result<Vec<String>, LegacyError> {
    let mut cleaned: Vec<String> = raw
        .iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .map(|w| if w.len() == 5 { format!("{w}:00") } else { w })
        .collect();

    for w in &cleaned {
        exchange_window_to_cron(w)?;
    }
    if cleaned.is_empty() {
        return Err(LegacyError::NoExchangeWindow);
    }
    cleaned.sort();
    cleaned.dedup();
    Ok(cleaned)
}

/// 复刻 `normalizeAccounts`（组合的账号选择）。
///
/// 规则与怪癖：
/// - 只有**真数组**才算数：`accounts` 是字符串时会被当作"没给"，转而去看废弃的单值字段；
/// - 单值 `account` → `[account]`；
/// - 剔除 `""` / `null`（JS 会连 `undefined` 一起剔，但 `undefined` 在 JSON 里不存在）；
/// - 去重，保持首次出现顺序；
/// - **结果为空一律返回 `None`，语义是"全部账号"**（而不是"没有账号"）。
pub fn normalize_pipeline_accounts(
    accounts: Option<&[Value]>,
    legacy_account: Option<&Value>,
) -> Option<Vec<Value>> {
    let raw: Vec<Value> = match accounts {
        Some(list) => list.to_vec(),
        None => legacy_account.cloned().into_iter().collect(),
    };

    let mut out: Vec<Value> = Vec::with_capacity(raw.len());
    for value in raw {
        let drop = match &value {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            _ => false,
        };
        if !drop && !out.contains(&value) {
            out.push(value);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 复刻 `normalize`（界面偏好）。
///
/// 两个布尔的默认方向**刻意相反**，这是旧版的真实行为：
/// - `remember_last_page`：`raw !== false` → **缺省就是 `true`**；
/// - `log_expanded` / `background_run`：`raw === true` → **缺省就是 `false`**。
///
/// `last_page` 不在 [`WEBUI_PAGES`] 白名单内则回落到 [`crate::model::DEFAULT_WEBUI_PAGE`]。
///
/// 返回的是**新领域类型** [`WebuiConfig`]，不另设一个"规整后"的孪生类型 ——
/// 两个只差字段类型的结构体，迟早会有一边忘记同步。
pub fn normalize_webui(raw: &LegacyWebuiFile) -> WebuiConfig {
    let page = raw
        .last_page
        .as_deref()
        .filter(|p| WEBUI_PAGES.contains(p))
        .unwrap_or(crate::model::DEFAULT_WEBUI_PAGE);
    WebuiConfig {
        remember_last_page: raw.remember_last_page != Some(false),
        last_page: page.to_string(),
        log_expanded: raw.log_expanded == Some(true),
        background_run: raw.background_run == Some(true),
        // 只用于展示，解析不出来的直接丢掉，不为一条时间戳保留原文
        updated_at: raw.updated_at.as_deref().and_then(parse_rfc3339),
    }
}

/// 宽松解析 RFC 3339 时间戳：解不出返回 `None`。
///
/// 旧文件里的时间戳是 `toISOString()` 产物（如 `2026-09-16T12:03:33.972Z`），
/// 但**手改过的文件里什么都可能有** —— 而迁移不该因为一个时间戳格式不对就失败。
pub fn parse_rfc3339(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// 复刻 `appendHistory` 的截断：只保留**最后** `cap` 条。
pub fn cap_history<T>(runs: &mut Vec<T>, cap: usize) {
    if runs.len() > cap {
        let cut = runs.len() - cap;
        runs.drain(..cut);
    }
}

/// 逐条解析，坏掉的条目只被记录下来而不会拖垮整份文件。
///
/// 迁移的底线是"能救的都要救"：旧文件里一条被手改坏的记录，
/// 不该导致另外 95 条历史全部丢失。返回 `(成功的条目, [(下标, 错误信息)])`。
pub fn parse_each<T: DeserializeOwned>(items: &[Value]) -> (Vec<T>, Vec<(usize, String)>) {
    let mut ok = Vec::with_capacity(items.len());
    let mut bad = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match serde_json::from_value::<T>(item.clone()) {
            Ok(value) => ok.push(value),
            Err(err) => bad.push((index, err.to_string())),
        }
    }
    (ok, bad)
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. 账号视图与"旧键 → 新标识"映射
// ═══════════════════════════════════════════════════════════════════════════

/// 一个旧账号被解析后的全部派生信息。
///
/// 迁移期间要反复用到（旧 `state.json` 的键、历史里的展示名、组合/定时任务里的账号引用），
/// 而每项都要解析一次 `auth` —— 所以解析一次、到处复用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAccountView {
    /// 从 1 开始的序号，对应旧 `findAccount` 的序号语义。
    pub index: usize,
    pub nickname: Option<String>,
    /// [`resolve_phone`] 的结果，可能为空串（既没配 `phone` 也解不出）。
    pub phone: String,
    /// 展示名。**这就是 `history.json` 里 `runs[].account` 存的东西。**
    pub label: String,
    /// 旧 `state.json` 的键（手机号原文，否则昵称，否则 `"account"`）。
    pub legacy_key: String,
    /// 新标识：手机号走掩码，避免把完整号码当主键四处引用。
    pub account_id: AccountId,
    /// 凭据到期时间（**epoch 毫秒**，`0` = 无过期信息）。
    ///
    /// 单位是毫秒而非秒：旧版拿它和 `Date.now()` 直接相减。见 [`crate::auth`] 模块头。
    pub auth_expire: i64,
    /// 平台号（`auth` 的第 1 段，如 `"2"`）；解不出为 `None`。
    pub platform: Option<String>,
    /// 旧配置里显式写出的 `phone`（与从 `auth` 解出来的是两回事）。
    pub config_phone: Option<String>,
}

impl LegacyAccountView {
    pub fn from_config(index: usize, config: &LegacyAccountConfig) -> Self {
        let nickname = config.nickname_or_none();
        let config_phone = config.phone.as_deref().filter(|p| !p.is_empty());
        let phone = resolve_phone(config_phone, &config.auth);
        let parsed = parse_auth(&config.auth);
        let auth_expire = parsed.as_ref().map_or(0, |info| info.expire_millis());
        let platform = parsed.as_ref().and_then(|info| info.platform.clone());

        Self {
            index,
            nickname: nickname.map(str::to_string),
            label: account_label(nickname, &phone),
            legacy_key: crate::auth::legacy_account_key(&phone, nickname),
            account_id: account_id(&phone, nickname),
            auth_expire,
            platform,
            config_phone: config_phone.map(str::to_string),
            phone,
        }
    }

    /// 昵称与手机号是否都能匹配到这个字符串（用于识别"展示名/手机号"引用）。
    pub fn matches_name(&self, who: &str) -> bool {
        self.nickname.as_deref() == Some(who) || (!self.phone.is_empty() && self.phone == who)
    }
}

/// 把整份旧配置解析成账号视图（序号从 1 开始，与旧版一致）。
pub fn describe_legacy_accounts(configs: &[LegacyAccountConfig]) -> Vec<LegacyAccountView> {
    configs
        .iter()
        .enumerate()
        .map(|(i, cfg)| LegacyAccountView::from_config(i + 1, cfg))
        .collect()
}

/// 旧键（`state.json` 的键）→ 新 `AccountId`。
///
/// **这是历史数据能对上号的唯一根据**，所以撞车必须硬失败：两个手机号的掩码相同
/// （前 2 后 2 相同）时，两个账号会塌缩成同一个 `AccountId`，
/// 迁移的结果是一份"云朵余额与凭据混在一起"的数据 —— 而且不会报错。
pub fn build_identity_map(
    views: &[LegacyAccountView],
) -> Result<BTreeMap<String, AccountId>, LegacyError> {
    let mut map: BTreeMap<String, AccountId> = BTreeMap::new();
    let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for view in views {
        map.insert(view.legacy_key.clone(), view.account_id.clone());
        let entry = sources.entry(view.account_id.0.clone()).or_default();
        if !entry.contains(&view.legacy_key) {
            entry.push(view.legacy_key.clone());
        }
    }

    if let Some((account_id, keys)) = sources.iter().find(|(_, keys)| keys.len() > 1) {
        return Err(LegacyError::MaskCollision {
            account_id: account_id.clone(),
            keys: keys.clone(),
        });
    }
    Ok(map)
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. 账号引用解析（旧语义 vs 迁移建议）
// ═══════════════════════════════════════════════════════════════════════════

/// **旧版** `findAccount` 的解析结果 —— 只描述旧版会怎么做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyTarget {
    /// 未指定 → 全部账号。
    All,
    /// 命中第 n 个账号（1 起）。
    Index(usize),
    /// 按昵称或手机号命中第 n 个账号。
    Named(usize),
    /// 解析不到。旧版此时记 `账号 x 不存在`，**并且一个账号都不跑**。
    Unresolved(String),
}

/// 复刻旧 `findAccount` 的查找顺序。
///
/// **`/^\d+$/` 分支在昵称匹配之前**，所以纯数字字符串一律按序号解释，
/// 手机号字符串永远匹配不到。这条怪癖原样保留 —— 迁移要识别出"过去从未生效的引用"，
/// 而不是假装它一直有效。
pub fn resolve_legacy_target(
    views: &[LegacyAccountView],
    reference: Option<&Value>,
) -> LegacyTarget {
    let Some(reference) = reference else {
        return LegacyTarget::All;
    };

    let lookup_index = |n: f64| -> Option<usize> {
        if n < 1.0 || n.fract() != 0.0 {
            return None;
        }
        let idx = n as usize;
        (idx <= views.len()).then_some(idx)
    };

    match reference {
        Value::Number(num) => match num.as_f64().and_then(lookup_index) {
            Some(idx) => LegacyTarget::Index(idx),
            None => LegacyTarget::Unresolved(num.to_string()),
        },
        Value::String(text) => {
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                // 纯数字串 = 序号（**不是手机号**）
                return match text.parse::<f64>().ok().and_then(lookup_index) {
                    Some(idx) => LegacyTarget::Index(idx),
                    None => LegacyTarget::Unresolved(text.clone()),
                };
            }
            views.iter().find(|v| v.matches_name(text)).map_or_else(
                || LegacyTarget::Unresolved(text.clone()),
                |v| LegacyTarget::Named(v.index),
            )
        }
        other => LegacyTarget::Unresolved(other.to_string()),
    }
}

/// 迁移诊断：这条旧引用**新版**该怎么解释，以及它过去是否真的生效过。
///
/// 与 [`LegacyTarget`] 分开，是因为两者回答的是不同的问题：
/// 前者是"旧版怎么理解它"，后者是"该不该照搬"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceAdvice {
    /// 未指定 → 全部账号，照搬。
    AllAccounts,
    /// 旧版能解析，照搬到同一个账号。
    Same { index: usize },
    /// 旧版把它当序号且**越界**（于是从未生效），但按手机号能唯一匹配到一个账号。
    ///
    /// 建议改指到该账号；这是**行为变更**，必须让用户确认。
    RecoveredFromPhone { digits: String, index: usize },
    /// 旧版解析不到，按新规则也匹配不到 → 必须人工确认，不要猜。
    WasNeverEffective { raw: String },
}

/// 给出一条旧引用的迁移建议。
pub fn advise_reference(views: &[LegacyAccountView], reference: Option<&Value>) -> ReferenceAdvice {
    match resolve_legacy_target(views, reference) {
        LegacyTarget::All => ReferenceAdvice::AllAccounts,
        LegacyTarget::Index(index) | LegacyTarget::Named(index) => ReferenceAdvice::Same { index },
        LegacyTarget::Unresolved(raw) => {
            // 越界的纯数字串：旧版当成序号 → 落空；但它很可能本来就是手机号。
            let recovered = views
                .iter()
                .find(|v| !v.phone.is_empty() && v.phone == raw)
                // 手机号可能写在配置的 phone 字段里（与 auth 解出的不一定相同）
                .or_else(|| {
                    views
                        .iter()
                        .find(|v| v.config_phone.as_deref() == Some(raw.as_str()))
                });
            match recovered {
                Some(view) => ReferenceAdvice::RecoveredFromPhone {
                    digits: raw,
                    index: view.index,
                },
                None => ReferenceAdvice::WasNeverEffective { raw },
            }
        }
    }
}

/// 把旧引用解析成新 `AccountId` 列表（`None` 语义为"全部账号"，由调用方决定怎么展开）。
///
/// 只走 [`ReferenceAdvice`] 里**明确**的分支：拿不准的一律返回 `Err`，
/// 迫使调用方把问题摆到迁移报告里，而不是替用户做决定。
pub fn resolve_reference_strict(
    views: &[LegacyAccountView],
    reference: Option<&Value>,
) -> Result<Option<AccountId>, ReferenceAdvice> {
    match advise_reference(views, reference) {
        ReferenceAdvice::AllAccounts => Ok(None),
        ReferenceAdvice::Same { index } | ReferenceAdvice::RecoveredFromPhone { index, .. } => {
            Ok(views.get(index - 1).map(|v| v.account_id.clone()))
        }
        advice @ ReferenceAdvice::WasNeverEffective { .. } => Err(advice),
    }
}

/// 按**展示名**找账号 —— 迁移 `history.json` 的 `runs[].account` 用。
///
/// 展示名可能撞车（两个账号同名，或两个账号的掩码手机号相同），
/// 所以返回全部候选，由调用方判断是否为 1 个。
pub fn find_by_label<'a>(
    views: &'a [LegacyAccountView],
    label: &str,
) -> Vec<&'a LegacyAccountView> {
    views.iter().filter(|v| v.label == label).collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// 测试：一半钉规整规则，一半钉"真实夹具能端到端走通"
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    /// 夹具由 `scripts/gen-legacy-fixtures.cjs` 从旧项目真实数据脱敏生成。
    /// 用 `include_str!` 而不是运行时读盘：夹具缺失就是编译失败，不会静默跳过断言。
    mod fixture {
        pub const CONFIG: &str = include_str!("../../../test/fixtures/legacy/asign.json");
        pub const STATE: &str = include_str!("../../../test/fixtures/legacy/state.json");
        pub const HISTORY: &str = include_str!("../../../test/fixtures/legacy/history.json");
        pub const PIPELINES: &str = include_str!("../../../test/fixtures/legacy/pipelines.json");
        pub const SCHEDULES: &str = include_str!("../../../test/fixtures/legacy/schedules.json");
        pub const EXCHANGE: &str = include_str!("../../../test/fixtures/legacy/exchange.json");
        pub const WEBUI: &str = include_str!("../../../test/fixtures/legacy/webui.json");
        pub const META: &str = include_str!("../../../test/fixtures/legacy/$meta.json");
    }

    fn load<T: DeserializeOwned>(text: &str) -> T {
        serde_json::from_str(text).expect("夹具应是合法 JSON 且字段与新定义一致")
    }

    /// 造一个能通过 `parse_auth` 的 auth，用来在测试里构造任意手机号的账号。
    fn auth_for(phone: &str) -> String {
        let payload = format!("2:{phone}:ak|bk|ck|1893456000000|fk");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(payload)
        )
    }

    fn account(phone: &str, nickname: Option<&str>) -> LegacyAccountConfig {
        LegacyAccountConfig {
            auth: auth_for(phone),
            nickname: nickname.map(str::to_string),
            phone: None,
        }
    }

    fn fixture_views() -> Vec<LegacyAccountView> {
        describe_legacy_accounts(&load::<LegacyConfig>(fixture::CONFIG).caiyun)
    }

    // ───────────────────────── 夹具：端到端 ─────────────────────────

    /// 账号配置 → 视图：手机号来自 auth 现场解析，标识走掩码。
    #[test]
    fn fixture_config_derives_identity_from_auth() {
        let config: LegacyConfig = load(fixture::CONFIG);
        assert_eq!(config.version, Some(2), "旧配置是 version 2");
        assert_eq!(config.caiyun.len(), 2);

        let views = describe_legacy_accounts(&config.caiyun);
        // 真实配置里**没有** phone 字段，号码只能从 auth 里解 —— 这正是 resolve_phone 的价值
        assert!(views.iter().all(|v| v.config_phone.is_none()));

        assert_eq!(views[0].nickname.as_deref(), Some("账号2"));
        assert_eq!(views[0].phone, "19900000001");
        assert_eq!(views[0].label, "账号2", "有昵称时展示名就是昵称");
        assert_eq!(views[0].legacy_key, "19900000001");
        assert_eq!(views[0].account_id.as_str(), "19****01");

        assert_eq!(views[1].nickname.as_deref(), Some("账号1"));
        assert_eq!(views[1].phone, "19900000002");
        assert_eq!(views[1].account_id.as_str(), "19****02");

        assert_eq!(
            views[0].platform.as_deref(),
            Some("2"),
            "平台号来自 auth 第 1 段"
        );

        // 序号从 1 起，与旧 findAccount 一致
        assert_eq!((views[0].index, views[1].index), (1, 2));

        // 夹具的 expire 刻意放在未来，免得这份测试随时间变成"凭据过期"。
        // 这里同时钉住**量级**：真实数据是 13 位毫秒，写成 10 位秒值会让下游整条链路的
        // 单位假设悄悄错掉（见 auth.rs 模块头）。
        assert!(
            views.iter().all(|v| v.auth_expire > 1_700_000_000_000),
            "夹具 auth 的 expire 应是毫秒量级且在未来"
        );
        assert!(
            views.iter().all(|v| v.auth_expire.to_string().len() == 13),
            "夹具应当长得像真实数据：13 位毫秒"
        );
    }

    /// **跨文件**：`state.json` 的键是手机号原文，必须能映射到与 `asign.json` 相同的 `AccountId`。
    /// 这条挂了就意味着历史凭据会挂到错误的账号上。
    #[test]
    fn fixture_state_keys_map_onto_the_same_account_ids() {
        let state: LegacyStateFile = load(fixture::STATE);
        let views = fixture_views();
        let map = build_identity_map(&views).expect("夹具内不应有掩码冲突");

        assert_eq!(state.accounts.len(), 2);
        assert_eq!(map.len(), 2);

        let mut derived: Vec<&str> = views.iter().map(|v| v.account_id.as_str()).collect();
        let mut mapped: Vec<&str> = state.accounts.keys().map(|k| map[k].as_str()).collect();
        derived.sort_unstable();
        mapped.sort_unstable();
        assert_eq!(derived, mapped, "state.json 的键应恰好覆盖全部账号");

        // 四个状态字段都要能读出来；未识别字段应为空（夹具里不该有）
        for (key, account_state) in &state.accounts {
            assert!(!account_state.is_empty(), "{key} 的状态不该是空的");
            assert!(
                account_state.unknown_fields().is_empty(),
                "{key} 出现了未识别字段 {:?}",
                account_state.unknown_fields()
            );
            assert!(account_state.jwt_token.as_deref().unwrap_or("").len() > 500);
            assert!(account_state.device_id.as_deref().unwrap_or("").len() > 80);
            assert!(account_state.user_domain_id.is_some());
        }
    }

    /// **跨文件**：历史里的展示名必须唯一对应一个账号。
    /// 展示名撞车（两个账号同名）会让历史无法归属 —— 这里是发现它的地方。
    #[test]
    fn fixture_history_labels_each_resolve_to_one_account() {
        let history: LegacyHistoryFile = load(fixture::HISTORY);
        assert_eq!(history.runs.len(), 12, "夹具只保留最后 12 条");

        let views = fixture_views();
        for run in &history.runs {
            assert!(!run.task.is_empty(), "每条历史都应记下任务名");
            let hits = find_by_label(&views, &run.account);
            assert_eq!(
                hits.len(),
                1,
                "展示名 {:?} 应唯一对应一个账号，实际命中 {}",
                run.account,
                hits.len()
            );
        }

        let tasks: Vec<&str> = history.runs.iter().map(|r| r.task.as_str()).collect();
        assert!(
            tasks.contains(&"auth-refresh"),
            "夹具里应有 auth-refresh 记录，实际 {tasks:?}"
        );
    }

    /// 日志条目同时保留数值 `level` 与语义 `type`，且**同一个 level 对应多个 type**。
    #[test]
    fn fixture_log_entries_keep_level_and_type_separately() {
        let history: LegacyHistoryFile = load(fixture::HISTORY);
        let mut combos = std::collections::BTreeSet::new();

        for run in &history.runs {
            for entry in &run.logs {
                assert!(entry.msg.is_some(), "日志条目应有 msg");
                assert!(entry.date.is_some(), "日志条目应有 date");
                combos.insert((entry.level, entry.kind.clone()));
            }
        }

        assert!(combos.contains(&(Some(3), Some("info".into()))));
        assert!(combos.contains(&(Some(0), Some("error".into()))));
        // 这是本测试存在的理由：level 3 之下不止一种语义，只看 level 会丢掉 start/success
        let level3: Vec<&Option<String>> = combos
            .iter()
            .filter(|(level, _)| *level == Some(3))
            .map(|(_, kind)| kind)
            .collect();
        assert!(
            level3.len() >= 2,
            "level=3 应对应多个 type，实际 {level3:?}"
        );
    }

    /// 组合：夹具里没给账号字段 → 语义是"全部账号"，而不是"没有账号"。
    #[test]
    fn fixture_pipeline_migrates_legacy_account_field() {
        let file: LegacyPipelinesFile = load(fixture::PIPELINES);
        assert_eq!(file.pipelines.len(), 1);
        let pipeline = &file.pipelines[0];

        assert_eq!(pipeline.name, "轻量巡检");
        assert_eq!(pipeline.steps.len(), 2);
        assert_eq!(
            pipeline
                .steps
                .iter()
                .map(|s| s.task.as_str())
                .collect::<Vec<_>>(),
            vec!["prizes", "receive"]
        );
        assert_eq!(pipeline.stop_on_error, Some(true));
        assert_eq!(pipeline.last_status.as_deref(), Some("ok"));
        assert_eq!(pipeline.last_duration_ms, Some(2481));

        assert!(pipeline.accounts.is_none() && pipeline.legacy_account.is_none());
        assert_eq!(
            normalize_pipeline_accounts(
                pipeline.accounts.as_deref(),
                pipeline.legacy_account.as_ref()
            ),
            None,
            "没给账号 = 全部账号"
        );
    }

    /// 抢兑：窗口按旧规则转成 6 段 cron；夹具里 `bindings` 为空但字段必须都在。
    #[test]
    fn fixture_exchange_windows_become_six_field_cron() {
        let exchange: LegacyExchangeFile = load(fixture::EXCHANGE);
        assert!(!exchange.enabled);
        assert!(exchange.bindings.is_empty());
        assert!(exchange.schedule_ids.is_empty());
        assert!(
            exchange.updated_at.is_some(),
            "字段应被识别（不是塞进 ignored）"
        );

        let windows = normalize_exchange_windows(&exchange.windows).expect("夹具窗口应合法");
        assert_eq!(windows, vec!["09:59:40", "11:59:40"]);
        assert_eq!(
            windows
                .iter()
                .map(|w| exchange_window_to_cron(w).unwrap())
                .collect::<Vec<_>>(),
            vec!["40 59 9 * * *", "40 59 11 * * *"]
        );
    }

    #[test]
    fn fixture_schedules_file_parses_even_when_empty() {
        let file: LegacySchedulesFile = load(fixture::SCHEDULES);
        assert!(file.schedules.is_empty(), "真实数据里定时任务列表为空");
    }

    #[test]
    fn fixture_webui_normalizes_with_legacy_defaults() {
        let prefs = normalize_webui(&load::<LegacyWebuiFile>(fixture::WEBUI));
        assert!(prefs.remember_last_page);
        assert_eq!(prefs.last_page, "tasks");
        assert!(!prefs.log_expanded);
        assert!(!prefs.background_run);
        assert!(prefs.updated_at.is_some());
    }

    /// 夹具声明的"分配了哪些脱敏号码"必须与我们从 `asign.json` 推出来的号码一致。
    ///
    /// 否则说明夹具与它的元信息脱节了 —— 那是最糟的情况：测试还在过，但依据已经错了。
    ///
    /// 注意元信息里**只有脱敏号**：真实号码是 PII，不落盘（早先这里存的是
    /// `{原号码: 脱敏号}`，等于把真实手机号写进了版本库）。
    #[test]
    fn fixture_phone_assignment_agrees_with_derived_phones() {
        let meta: Value = load(fixture::META);
        let mut declared: Vec<&str> = meta["$phoneAssignment"]["$assigned"]
            .as_array()
            .expect("$meta.json 应有 $phoneAssignment.$assigned")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let views = fixture_views();
        let mut derived: Vec<&str> = views.iter().map(|v| v.phone.as_str()).collect();
        declared.sort_unstable();
        derived.sort_unstable();
        assert_eq!(declared, derived);

        // 元信息里应记下各源文件的 sha256，避免有人以为夹具就是全部历史
        let sums = meta["$sourceSha256"].as_object().unwrap();
        assert!(sums.contains_key("history.json"));
    }

    /// 夹具里**不得**出现真实标识：11 位手机号与 19 位 `userDomainId`
    /// 都必须是 `199` 开头的合成值。
    ///
    /// 这条是给"元信息/注释里顺手写下原值"这类泄漏兜底的 —— 夹具是脱敏产物，
    /// 出现真实值就说明脱敏被绕过了。生成器自己也有一道自检，
    /// 这里再来一道是因为它会**编译进测试**：夹具被手改也能被发现，
    /// 并且它会把 base64（`auth` 字段）里的载荷解码后再查一遍。
    ///
    /// 判定规则、误报原因与剩余盲区见
    /// [`crate::test_support::assert_only_synthetic_identifiers`]。
    #[test]
    fn fixtures_contain_no_real_identifiers() {
        for (name, text) in [
            ("$meta.json", fixture::META),
            ("asign.json", fixture::CONFIG),
            ("state.json", fixture::STATE),
            ("history.json", fixture::HISTORY),
            ("pipelines.json", fixture::PIPELINES),
            ("schedules.json", fixture::SCHEDULES),
            ("exchange.json", fixture::EXCHANGE),
            ("webui.json", fixture::WEBUI),
        ] {
            crate::test_support::assert_only_synthetic_identifiers(name, text);
        }
    }

    // ───────────────────────── 规整规则 ─────────────────────────

    /// `windowToCron`：**前导零会被 `Number()` 去掉**（`09` → `9`）。
    #[test]
    fn exchange_window_conversion_follows_old_rules() {
        assert_eq!(
            exchange_window_to_cron("09:59:40").unwrap(),
            "40 59 9 * * *"
        );
        assert_eq!(exchange_window_to_cron("00:00:00").unwrap(), "0 0 0 * * *");
        assert_eq!(
            exchange_window_to_cron("23:59:59").unwrap(),
            "59 59 23 * * *"
        );
        // 旧版在正则前先 trim
        assert_eq!(
            exchange_window_to_cron(" 09:59:40 ").unwrap(),
            "40 59 9 * * *"
        );

        for bad in [
            "9:59:40",
            "09:59",
            "09:59:40:00",
            "24:00:00",
            "09:60:00",
            "09:59:60",
            "",
            "abc",
        ] {
            assert!(
                exchange_window_to_cron(bad).is_err(),
                "{bad:?} 应被判为非法时刻"
            );
        }
    }

    #[test]
    fn normalize_windows_pads_dedupes_and_sorts() {
        let windows = |list: &[&str]| {
            normalize_exchange_windows(&list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
        };

        assert_eq!(
            windows(&["11:59:40", "09:59:40", "11:59:40"]).unwrap(),
            vec!["09:59:40", "11:59:40"],
            "去重并排序"
        );
        assert_eq!(windows(&["09:59"]).unwrap(), vec!["09:59:00"], "5 位补 :00");
        assert_eq!(
            windows(&[" 09:59:40 ", ""]).unwrap(),
            vec!["09:59:40"],
            "空串被剔除"
        );
        assert_eq!(windows(&[]), Err(LegacyError::NoExchangeWindow));
        assert_eq!(windows(&[""]), Err(LegacyError::NoExchangeWindow));
    }

    /// 组合的账号选择：只有真数组才算数，空结果表示"全部账号"。
    #[test]
    fn pipeline_accounts_follow_legacy_normalization() {
        let norm = normalize_pipeline_accounts;

        // 废弃的单值字段 → 数组
        assert_eq!(
            norm(None, Some(&json!("账号1"))),
            Some(vec![json!("账号1")])
        );
        // 去空、去重，保持首次出现顺序
        assert_eq!(
            norm(
                Some(&[json!("a"), json!(""), json!("a"), Value::Null]),
                None
            ),
            Some(vec![json!("a")])
        );
        // **空数组会盖掉废弃字段**（旧版先判 Array.isArray）
        assert_eq!(norm(Some(&[]), Some(&json!("账号1"))), None);
        // 都没有 → 全部账号
        assert_eq!(norm(None, None), None);
        // 非数组的 accounts 被当作"没给"
        assert_eq!(
            norm(Some(&[json!(1), json!(2)]), None),
            Some(vec![json!(1), json!(2)])
        );
    }

    /// 界面偏好：两个布尔的默认方向相反 —— 这是旧版的真实行为。
    #[test]
    fn webui_boolean_defaults_point_in_opposite_directions() {
        let prefs = normalize_webui(&LegacyWebuiFile::default());
        assert!(prefs.remember_last_page, "缺省即 true（!== false）");
        assert!(!prefs.log_expanded, "缺省即 false（=== true）");
        assert!(!prefs.background_run);
        assert_eq!(prefs.last_page, "tasks");

        let explicit = normalize_webui(&LegacyWebuiFile {
            remember_last_page: Some(false),
            log_expanded: Some(true),
            last_page: Some("history".into()),
            ..Default::default()
        });
        assert!(!explicit.remember_last_page);
        assert!(explicit.log_expanded);
        assert_eq!(explicit.last_page, "history");

        // 白名单外的页面回落到默认
        let bogus = normalize_webui(&LegacyWebuiFile {
            last_page: Some("../../etc/passwd".into()),
            ..Default::default()
        });
        assert_eq!(bogus.last_page, "tasks", "白名单外的值必须回落");
    }

    #[test]
    fn cap_history_keeps_the_tail() {
        let mut runs: Vec<usize> = (0..250).collect();
        cap_history(&mut runs, LEGACY_HISTORY_CAP);
        assert_eq!(runs.len(), 200);
        assert_eq!(runs.first(), Some(&50));
        assert_eq!(runs.last(), Some(&249));

        let mut small: Vec<usize> = vec![1, 2, 3];
        cap_history(&mut small, LEGACY_HISTORY_CAP);
        assert_eq!(small, vec![1, 2, 3], "不足上限时不动");
    }

    /// 一条坏记录不该拖垮整份文件。
    #[test]
    fn parse_each_isolates_broken_items() {
        let items = vec![
            json!({"task": "sign", "account": "账号1", "start": "t", "end": "t"}),
            json!(42),
            json!({"task": "prize", "account": "账号2"}),
        ];
        let (ok, bad) = parse_each::<LegacyRun>(&items);
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].task, "sign");
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0, 1, "坏条目的下标要能定位");
    }

    #[test]
    fn legacy_state_reports_unknown_fields_instead_of_dropping_them() {
        let state: LegacyAccountState =
            serde_json::from_value(json!({"jwtToken": "j", "totalCloud": 123})).unwrap();
        assert_eq!(state.jwt_token.as_deref(), Some("j"));
        assert_eq!(
            state.unknown_fields(),
            vec!["totalCloud"],
            "旧版本写过的字段应被报告，而不是静默丢弃"
        );
        assert!(!state.is_empty());
        assert!(LegacyAccountState::default().is_empty());
    }

    // ───────────────────────── 账号引用 ─────────────────────────

    /// 旧 `findAccount`：**纯数字字符串按序号解释**，手机号字符串永远匹配不到。
    #[test]
    fn legacy_target_resolution_reproduces_find_account() {
        let views = fixture_views();
        let resolve = |value: Value| resolve_legacy_target(&views, Some(&value));

        assert_eq!(resolve_legacy_target(&views, None), LegacyTarget::All);

        assert_eq!(resolve(json!(1)), LegacyTarget::Index(1));
        assert_eq!(resolve(json!(2)), LegacyTarget::Index(2));
        assert_eq!(resolve(json!("1")), LegacyTarget::Index(1), "纯数字串=序号");
        assert_eq!(resolve(json!("账号1")), LegacyTarget::Named(2));
        assert_eq!(resolve(json!("账号2")), LegacyTarget::Named(1));

        // 越界序号
        assert_eq!(resolve(json!(0)), LegacyTarget::Unresolved("0".into()));
        assert_eq!(resolve(json!(3)), LegacyTarget::Unresolved("3".into()));
        assert_eq!(resolve(json!(-1)), LegacyTarget::Unresolved("-1".into()));

        // ★ 手机号字符串：被当成序号（19 亿），必然越界 —— 这就是那个陷阱
        assert_eq!(
            resolve(json!("19900000002")),
            LegacyTarget::Unresolved("19900000002".into()),
            "手机号字符串会被当序号：这正是旧版静默不执行的原因"
        );
        assert_eq!(resolve(json!("")), LegacyTarget::Unresolved(String::new()));
        assert_eq!(
            resolve(json!("不存在")),
            LegacyTarget::Unresolved("不存在".into())
        );
    }

    /// 迁移建议与旧语义分开表达：能对应上的会**明确标出这是一次行为变更**。
    #[test]
    fn advice_flags_references_that_never_took_effect() {
        let views = fixture_views();
        let advise = |value: Value| advise_reference(&views, Some(&value));

        assert_eq!(advise_reference(&views, None), ReferenceAdvice::AllAccounts);
        assert_eq!(advise(json!("账号1")), ReferenceAdvice::Same { index: 2 });
        assert_eq!(advise(json!(1)), ReferenceAdvice::Same { index: 1 });

        assert_eq!(
            advise(json!("19900000002")),
            ReferenceAdvice::RecoveredFromPhone {
                digits: "19900000002".into(),
                index: 2
            },
            "越界的数字串若是已知手机号，应给出可恢复的建议"
        );
        assert_eq!(
            advise(json!("12345678901")),
            ReferenceAdvice::WasNeverEffective {
                raw: "12345678901".into()
            },
            "不认识的数字串不能猜"
        );

        // 明确的分支才给结果；拿不准的一律让调用方去报告
        assert_eq!(
            resolve_reference_strict(&views, Some(&json!("19900000002"))),
            Ok(Some(AccountId::new("19****02")))
        );
        assert!(resolve_reference_strict(&views, Some(&json!("12345678901"))).is_err());
        assert_eq!(resolve_reference_strict(&views, None), Ok(None));
    }

    /// 掩码撞车必须**中止迁移**：两个账号会塌缩成一个 `AccountId`，且不会报错。
    #[test]
    fn mask_collision_aborts_migration() {
        let views = describe_legacy_accounts(&[
            account("19000000001", Some("甲")),
            account("19100000001", Some("乙")),
        ]);
        // 两个号码前 2 后 2 都是 19 / 01
        assert_eq!(views[0].account_id.as_str(), "19****01");
        assert_eq!(views[1].account_id.as_str(), "19****01");

        match build_identity_map(&views) {
            Err(LegacyError::MaskCollision { account_id, keys }) => {
                assert_eq!(account_id, "19****01");
                assert_eq!(keys, vec!["19000000001", "19100000001"]);
            }
            other => panic!("应报掩码冲突，实际 {other:?}"),
        }
    }

    #[test]
    fn identity_map_is_built_for_distinct_masks() {
        let views = describe_legacy_accounts(&[
            account("19000000001", Some("甲")),
            account("19100000002", Some("乙")),
        ]);
        let map = build_identity_map(&views).expect("掩码不同应能建立映射");
        assert_eq!(map["19000000001"].as_str(), "19****01");
        assert_eq!(map["19100000002"].as_str(), "19****02");
    }

    /// 没有手机号的账号退化为昵称（旧 `accountKey` 的兜底链）。
    #[test]
    fn accounts_without_phone_fall_back_to_nickname() {
        let views = describe_legacy_accounts(&[
            LegacyAccountConfig {
                auth: "!!!not-base64!!!".into(),
                nickname: Some("备用账号".into()),
                phone: None,
            },
            LegacyAccountConfig {
                auth: String::new(),
                nickname: None,
                phone: None,
            },
        ]);
        assert_eq!(views[0].phone, "");
        assert_eq!(views[0].legacy_key, "备用账号");
        assert_eq!(views[0].account_id.as_str(), "备用账号");
        assert_eq!(views[0].label, "备用账号");

        assert_eq!(views[1].legacy_key, "account");
        assert_eq!(views[1].account_id.as_str(), "account");
        assert_eq!(views[1].label, "账号");
    }
}
