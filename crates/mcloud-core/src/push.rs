//! 推送的**纯配置与日志模型**。
//!
//! # 为什么这半截在 core
//!
//! core 不能反向依赖 `mcloud-app`，所以「配置长什么样」必须与「怎么发出去」分家：
//!
//! - **本文件**：serde 定义、日志条目/收集器、纯过滤/格式化/节流判定函数 —— 一行 IO
//!   都没有，符合 core 的铁律（见 lib.rs 模块头）。
//! - **`mcloud-app::push`**：构造各渠道请求、HTTP 投递、HMAC 签名、脱敏；
//!   它从本模块**原样再导出**这些类型，既有调用方与测试的路径不变。
//!
//! # 2026-10-08 推翻重做：**规则自带凭据，行为选择规则**
//!
//! 用户原话（这一版的设计就是它，别再往下加层 ✓）：
//!
//! > 「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个平台**」
//! > 「**不是规则去命中行为，而是行为选择规则，一个行为可以选择多个规则**」
//!
//! ⇒ 两条分界，本模块的每一格都按它摆：
//!
//! | 问题 | 归谁 | 落在哪 |
//! |---|---|---|
//! | **怎么发**（发到哪几个平台、token、标题、日志等级、汇总方式） | **规则** | [`PushRule`]（自带一份 [`PushConfig`] ✓） |
//! | **什么时候发**（哪个定时/编排、哪些运行结局、面板行为的节流） | **行为** | `Schedule::push_rules` / `Pipeline::push_rules` + [`PanelBindings`] ✓ |
//!
//! ⚠ **没有**独立的"渠道配置"那一层了 ✗：原先 `UserStore::push_config`
//! （用户级一份渠道配置，落盘 `data/push-config.json`）与规则里的
//! `events` / `channels` / `onlyError` / `minLevel` / `slowFactor` / `failStreak`
//! 全部删掉 —— 它们要么是"什么时候发"（归行为 ✓）、要么被规则自带的
//! `config` 顶掉（归规则 ✓）。⚠ 不做兼容：旧形状的落盘文件**直接删** ✓
//! （见 `mcloud-store` 的 `purge_legacy_push_files` ✓）。
//!
//! # 与 TS 的差异
//!
//! `mcloud-sign` 是**单用户**工具：它只有一份全局推送配置，那是为了 **DRY**。
//! 本仓是**多用户服务** ⇒ 每一条规则属于某个人（`PushRule::owner` ✓），
//! 同一进程里 A、B 的规则各是各的，绝不能把 A 的 token 用到 B 的运行上 ✓。
//! **没有进程级兜底**（「一份 env 管全进程」那种写法已整个删掉）。
//!
//! # 其余与 TS 的差异
//!
//! 超时、非 2xx 判定、时刻格式、email 未实现等说明留在 `mcloud-app::push`
//! 的模块头，因为它们描述的是**投递**那一侧。

use crate::user::UserId;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::Error;
use serde_json::{Map, Value};
/// 推送标题的默认值。
pub const DEFAULT_TITLE: &str = "mcloud 运行推送";

// ==================== 日志侧 ====================

/// 推送日志级别。
///
/// 语义照 TS 的 `PushLogLevel`：`error` 只留 error/fail/fatal，`warn` 再带 warn，
/// `info`（默认）再带 info/success/start，`debug` 全留。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushLogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl PushLogLevel {
    /// TS `PUSH_LEVEL_MAX`：级别越小越严重，过滤规则是 `level <= max`。
    pub const fn max_level(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 3,
            Self::Debug => 4,
        }
    }
}

/// 日志类型 → 数值级别。
///
/// 逐字照 `transport.ts` 的 `levelMap`：`error`/`fail`/`fatal` = 0、`warn` = 1、
/// `info`/`success`/`start` = 3、`debug` = 4，**未知类型按 3**（TS 里那个 `?? 3`）。
pub fn level_of_type(r#type: &str) -> u8 {
    match r#type {
        "error" | "fail" | "fatal" => 0,
        "warn" => 1,
        "debug" => 4,
        _ => 3,
    }
}

/// 一条待推送的日志。
///
/// 字段名对齐 TS 的 `PushLogEntry`；`date` 在 TS 里是 `Date`，这里落成
/// **epoch 毫秒**（`i64`），由 [`format_logs_for_push`] 渲染成东八区墙钟。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushLogEntry {
    /// 数值级别，见 [`level_of_type`]。
    pub level: u8,
    /// 日志类型（`error` / `info` / …），进推送文本的方括号里。
    pub r#type: String,
    pub msg: String,
    /// epoch 毫秒（TS 的 `Date`）。
    pub date: i64,
}

/// 推送日志收集器 —— `pushTransport()` 的对应物。
///
/// # 为什么锁是 `Mutex` 且**永不 panic**
///
/// TS 的收集器是一个闭包捕获的数组，生产里被日志器多路复用；Rust 侧要跨
/// `Send + Sync` 用它。这一整个模块的铁律是「失败只记日志、不 panic」，
/// 所以连锁中毒也**不**把它变成崩溃：中毒的锁用 `into_inner()` 继续用。
/// 丢一条日志可以接受，让一次任务的收尾因为日志而炸掉不可以。
#[derive(Debug, Default)]
pub struct PushLogCollector {
    logs: Mutex<Vec<PushLogEntry>>,
}

impl PushLogCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收一条日志。
    ///
    /// `at_ms` 必须来自注入的 `Clock`（仓库约定 §7.2：不许直接取系统时间）。
    pub fn write(&self, r#type: impl Into<String>, msg: impl Into<String>, at_ms: i64) {
        let r#type = r#type.into();
        let entry = PushLogEntry {
            level: level_of_type(&r#type),
            r#type,
            msg: msg.into(),
            date: at_ms,
        };
        self.with_logs(|logs| logs.push(entry));
    }

    /// 取一份快照（顺序即写入顺序）。
    pub fn entries(&self) -> Vec<PushLogEntry> {
        self.with_logs(|logs| logs.clone())
    }

    pub fn len(&self) -> usize {
        self.with_logs(|logs| logs.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空。TS 的 `clear()` 对应物（一次运行结束后复用同一个收集器时用）。
    pub fn clear(&self) {
        self.with_logs(Vec::clear);
    }

    /// 本次运行有没有**最严重那一档**的日志。
    ///
    /// ⚠ 判据是**严重级别**（`level_of_type` 给出 [`PushLogLevel::Error`] 那一档，
    /// 即 `0`），**不是** `type` 恰好等于 `"error"` —— `level_of_type` 把
    /// `error` / `fail` / `fatal` 都映射成 0（**同等严重** ✓），所以这三者都算
    /// "有错误" ✓。
    ///
    /// # 为什么偏离 TS（2026-10-08 用户口径）
    ///
    /// TS 的 `sendPushNotification` 判的是 `logs.some(el => el.type === "error")`
    /// （它自己的 `test-push-filter.ts` 还有一条「onlyError: fail 不算 error」
    /// 钉着这个口径）—— 本仓**故意更宽**：照 TS 那条口径，开着"仅错误时推"时
    /// `fail` / `fatal` 这两条**更严重**的反而不推 ✗，用户最该收到的告警被吞了 ✓。
    /// 所以这里判级别、不判类型相等；`type` 那一列只用来渲染 `[类型 时刻]` ✓。
    ///
    /// ⚠ 别在下面重写一遍 `"error" | "fail" | "fatal"` 字面量：哪些 type 属于
    /// 这一档**只有** [`level_of_type`] 一处清单，两处写迟早漂开 ✗。
    pub fn has_error(&self) -> bool {
        let error_level = PushLogLevel::Error.max_level();
        self.with_logs(|logs| {
            logs.iter()
                .any(|entry| level_of_type(&entry.r#type) == error_level)
        })
    }

    /// 锁中毒也照常取用，见类型说明。
    fn with_logs<T>(&self, f: impl FnOnce(&mut Vec<PushLogEntry>) -> T) -> T {
        let mut guard = match self.logs.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&mut guard)
    }
}

/// epoch 毫秒 → 东八区墙钟 `HH:MM:SS`（TS `toLocaleTimeString()` 的确定性替代）。
///
/// 东八区是**固定偏移、无夏令时**，「减 8 小时再取当天秒数」与走 `chrono` 的
/// [`crate::beijing_offset`] 等价，而这里只是纯算术、不牵时区库。
///
/// 这个等价关系有测试钉着（在 `mcloud-app::push` 的测试里）：
/// `local_time_hms_agrees_with_beijing_digits` 拿 [`crate::beijing_time_digits`]
/// 逐段对拍 —— 偏移口径一旦漂移，那条先炸。
pub fn local_time_hms(at_ms: i64) -> String {
    let seconds_since_utc_epoch = at_ms.div_euclid(1_000) + 8 * 3_600;
    let second_of_day = seconds_since_utc_epoch.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60
    )
}

/// 按 `minLevel` 过滤并拼成推送正文（`formatLogsForPush` 的对应物）。
///
/// 每行形如 `[类型 时刻]消息`；被过滤掉的行**不产生空行**。
pub fn format_logs_for_push(logs: &[PushLogEntry], min_level: PushLogLevel) -> String {
    let max = min_level.max_level();
    logs.iter()
        .filter(|entry| entry.level <= max)
        .map(|entry| {
            format!(
                "[{} {}]{}",
                entry.r#type,
                local_time_hms(entry.date),
                entry.msg
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ==================== 配置 ====================

/// 渠道配置里除具名字段外剩下的键（TS 到处用的 `...config` / `...option`）。
///
/// TS 把多余的键**原样展开进请求体**（甚至可以覆盖前面算好的同名值），
/// 所以这里保留一个 `flatten` 的映射，展开顺序也照 TS：先放算出来的字段，
/// 再 `extend(extra)`。
pub type ExtraFields = Map<String, Value>;

/// pushplus / Server 酱这类「只有一个 token + 任意附加字段」的配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenConfig {
    pub token: String,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 企业微信**应用**配置（要两步：先换 `access_token`，再发消息）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkWeixinConfig {
    #[serde(default)]
    pub msgtype: Option<String>,
    #[serde(default)]
    pub touser: Option<String>,
    #[serde(default)]
    pub agentid: Option<i64>,
    #[serde(default)]
    pub corpsecret: Option<String>,
    #[serde(default)]
    pub corpid: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 企业微信**机器人**配置（一个 webhook URL 打到底）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkWeixinBotConfig {
    pub url: String,
    #[serde(default)]
    pub msgtype: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// Telegram Bot 配置。
///
/// ⚠ `chat_id` / `proxy` 也是**请求体**的字段：TS 没把它们解构掉，
/// 于是它们随 `...options` 一起进了 body。这里具名持有、再显式插回 body，
/// 结果形状与 TS 一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TgBotConfig {
    #[serde(default)]
    pub api_host: Option<String>,
    pub token: String,
    // ⚠ TS 这两个字段是 **snake_case**（`chat_id` / `disable_web_page_preview`），
    // 不能跟着结构体的 rename_all 走成 camelCase —— 那样它们会被 flatten 收进
    // extra，具名字段永远是 None，于是"缺 chat_id"的跳过分支被误触发。
    #[serde(default, rename = "chat_id")]
    pub chat_id: Option<Value>,
    #[serde(default, rename = "disable_web_page_preview")]
    pub disable_web_page_preview: Option<bool>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// Bark（iOS）配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarkConfig {
    pub key: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 回逍（2Im）的可选 query。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TwoImQuery {
    #[serde(default)]
    pub plat: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub icon: Option<Value>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 回逍（2Im）配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwoImConfig {
    pub key: String,
    pub sid: String,
    #[serde(default)]
    pub query: Option<TwoImQuery>,
    #[serde(default)]
    pub msgtype: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 钉钉机器人配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DingTalkConfig {
    pub token: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 邮箱配置。
///
/// ⚠ **本版不会发送**：TS 的 email 走 Node 专属 `email.node.ts`（nodemailer/SMTP），
/// Rust 侧要拉 SMTP 客户端依赖，属独立增量。结构留在这里只为「配置能解析、
/// 能看出配了什么」，发送见 `mcloud-app::push::send_email`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailConfig {
    pub pass: String,
    pub from: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub host: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(flatten, default)]
    pub extra: ExtraFields,
}

/// 自定义 POST 模板（`PushApiData`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PushApiData {
    /// 请求 URL，支持 `{title}` / `{text}` 占位符（URL 里按 `encodeURIComponent` 转义）。
    pub url: String,
    /// 默认 `POST`；TS 会 `toUpperCase()`。
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub timeout: Option<u64>,
    /// 模板体：里面的 `{title}` / `{text}` 也会被替换（**不转义**，照 TS）。
    #[serde(default)]
    pub data: Option<ExtraFields>,
    /// `data` 的备选（`index.ts` 的 `data || body`）。
    #[serde(default)]
    pub body: Option<ExtraFields>,
}

/// `customPost` 允许**单个模板或模板数组**（TS 的 `PushApiData | PushApiData[]`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomPost {
    Single(PushApiData),
    Many(Vec<PushApiData>),
}

impl CustomPost {
    /// 摊平成模板列表（单模板就是长度 1 的切片）。
    pub fn items(&self) -> &[PushApiData] {
        match self {
            Self::Single(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}

fn default_title() -> String {
    DEFAULT_TITLE.to_string()
}

/// 推送配置（`PushConfig`）。
///
/// 所有渠道字段都是 `Option`：**没配 = 一枪都不发**（见 `mcloud-app::push::send_notify`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushConfig {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default)]
    pub only_error: bool,
    #[serde(default)]
    pub min_level: PushLogLevel,
    #[serde(default)]
    pub pushplus: Option<TokenConfig>,
    #[serde(default)]
    pub server_chan: Option<TokenConfig>,
    #[serde(default)]
    pub work_weixin: Option<WorkWeixinConfig>,
    #[serde(default)]
    pub work_weixin_bot: Option<WorkWeixinBotConfig>,
    #[serde(default)]
    pub tg_bot: Option<TgBotConfig>,
    #[serde(default)]
    pub bark: Option<BarkConfig>,
    #[serde(default)]
    pub two_im: Option<TwoImConfig>,
    #[serde(default)]
    pub ding_talk: Option<DingTalkConfig>,
    #[serde(default)]
    pub email: Option<EmailConfig>,
    #[serde(default)]
    pub custom_post: Option<CustomPost>,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            only_error: false,
            min_level: PushLogLevel::default(),
            pushplus: None,
            server_chan: None,
            work_weixin: None,
            work_weixin_bot: None,
            tg_bot: None,
            bark: None,
            two_im: None,
            ding_talk: None,
            email: None,
            custom_post: None,
        }
    }
}

impl PushConfig {
    /// 本次运行要不要推。
    ///
    /// 照 TS `cli/src/index.ts` 的 `sendPushNotification`：没有日志不推；
    /// `onlyError` 且本次**没有最严重那一档**日志时不推 —— 那一档是
    /// `error` / `fail` / `fatal` 三条（判级别、不判类型相等，见
    /// [`PushLogCollector::has_error`]，那里写了为什么偏离 TS ✓）。
    pub fn should_push(&self, logs: &PushLogCollector) -> bool {
        if logs.is_empty() {
            return false;
        }
        !self.only_error || logs.has_error()
    }

    /// 本次运行要推的正文；`None` 表示**不推**。
    ///
    /// 顺序照 TS：`should_push` → [`format_logs_for_push`]（带 `minLevel`）→
    /// 过滤完是空串也不推。把它收成一个函数，是为了让接线方不会把
    /// 「onlyError」和「minLevel」两步的顺序写反或漏掉。
    pub fn push_text(&self, logs: &PushLogCollector) -> Option<String> {
        if !self.should_push(logs) {
            return None;
        }
        let text = format_logs_for_push(&logs.entries(), self.min_level);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// **一个渠道都没开**吗（`title`/`onlyError`/`minLevel` 这些旋钮不算渠道 ✓）。
    ///
    /// ⚠ 读侧要拿它把"空配置"折回 `null` ✓：`PUT /api/push/config` 传 `null`
    /// （= 关掉推送 ✓）落盘的是 [`PushConfig::default`] ✓，而 `validate()` 又
    /// **不允许**空配置被显式存进来 ✓ ⇒ 读到"空"就只可能是"关掉" ✓，
    /// 折回 `None` 才不会让 `GET` 与 `PUT` 的话对不上 ✓。
    pub fn has_any_channel(&self) -> bool {
        self.pushplus.is_some()
            || self.server_chan.is_some()
            || self.work_weixin.is_some()
            || self.work_weixin_bot.is_some()
            || self.tg_bot.is_some()
            || self.bark.is_some()
            || self.two_im.is_some()
            || self.ding_talk.is_some()
            || self.email.is_some()
            || self.custom_post.is_some()
    }

    /// 存之前**必须**过的一关：渠道里少 token / 少 chat_id 这类错。
    ///
    /// # 为什么校验住在这里（2026-10-08 从 `account.rs::validate_notify` 搬来）
    ///
    /// 这份配置原先挂在**云盘账号**上（`AccountSettings::notify` ✗），校验函数也就
    /// 跟着住在 `account.rs` 里。现在它收成**用户级**（用户口径：「**很显然我说的和
    /// 云盘账户没有关系**」＋「**丢弃现有情况，重写**」✓）⇒ 校验跟着**类型**走 ✓，
    /// 谁存它谁调 ✓（`PUT /api/push/config` ✓）。
    ///
    /// # 判据一条都没放宽
    ///
    /// 每条渠道的**必填项**逐个非空（应用消息那种两步换 token 的，corpid / corpsecret /
    /// agentid 都要 ✓）；最后一条更关键：**一个渠道都没有就报错** ✗ ——
    /// 那样配一条都发不出去，而用户以为"我配好了" ✗。要关掉推送就整份写 `null` ✓。
    ///
    /// ⚠ 错误信息只说**路径**、绝不带值 ✓（里面有 token / secret ✓）。
    pub fn validate(&self) -> crate::Result<()> {
        let mut any = false;
        if let Some(c) = &self.pushplus {
            any = true;
            require_non_empty("pushConfig.pushplus.token", &c.token)?;
        }
        if let Some(c) = &self.server_chan {
            any = true;
            require_non_empty("pushConfig.serverChan.token", &c.token)?;
        }
        if let Some(c) = &self.work_weixin {
            any = true;
            // 应用消息是两步：先 corpid+corpsecret 换 access_token，再带 agentid 发。
            // 缺任何一个都到不了"发出去了"那一步。
            match c.corpid.as_deref() {
                Some(v) if !v.trim().is_empty() => {}
                _ => {
                    return Err(Error::Config(
                        "pushConfig.workWeixin.corpid 缺失：取 access_token 要用它".into(),
                    ))
                }
            }
            match c.corpsecret.as_deref() {
                Some(v) if !v.trim().is_empty() => {}
                _ => {
                    return Err(Error::Config(
                        "pushConfig.workWeixin.corpsecret 缺失：取 access_token 要用它".into(),
                    ))
                }
            }
            if c.agentid.is_none() {
                return Err(Error::Config(
                    "pushConfig.workWeixin.agentid 缺失：发应用消息要用它".into(),
                ));
            }
        }
        if let Some(c) = &self.work_weixin_bot {
            any = true;
            require_non_empty("pushConfig.workWeixinBot.url", &c.url)?;
        }
        if let Some(c) = &self.tg_bot {
            any = true;
            require_non_empty("pushConfig.tgBot.token", &c.token)?;
            // 判据照发送侧（`build_tg_bot` 的 JS 真值口径）：`0` / `""` / `null` 都算缺，
            // 缺了整条跳过 —— 那是"配了不发"的典型长相。
            match c.chat_id.as_ref() {
                Some(v) if js_truthy(v) => {}
                _ => {
                    return Err(Error::Config(
                        "pushConfig.tgBot.chat_id 缺失或为空：没有它发送侧会整条跳过".into(),
                    ))
                }
            }
        }
        if let Some(c) = &self.bark {
            any = true;
            require_non_empty("pushConfig.bark.key", &c.key)?;
        }
        if let Some(c) = &self.two_im {
            any = true;
            require_non_empty("pushConfig.twoIm.key", &c.key)?;
            require_non_empty("pushConfig.twoIm.sid", &c.sid)?;
        }
        if let Some(c) = &self.ding_talk {
            any = true;
            require_non_empty("pushConfig.dingTalk.token", &c.token)?;
        }
        if let Some(c) = &self.email {
            any = true;
            // 本版不发邮件（见 `mcloud-app::push::EMAIL_NOT_IMPLEMENTED`），
            // 但配了就要配全 —— 一份缺字段的配置只会让人以为它生效了。
            require_non_empty("pushConfig.email.pass", &c.pass)?;
            require_non_empty("pushConfig.email.from", &c.from)?;
            require_non_empty("pushConfig.email.host", &c.host)?;
        }
        if let Some(c) = &self.custom_post {
            any = true;
            let items = c.items();
            if items.is_empty() {
                return Err(Error::Config(
                    "pushConfig.customPost 一个模板都没有：空数组等于没配".into(),
                ));
            }
            for (idx, item) in items.iter().enumerate() {
                require_non_empty(&format!("pushConfig.customPost[{idx}].url"), &item.url)?;
            }
        }
        if !any {
            return Err(Error::Config(
                "推送配置里没有任何渠道（全为 null）：这样配一条都发不出去；要关掉推送请把整份设为 null"
                    .into(),
            ));
        }
        Ok(())
    }
}

// ⚠ 这里原先有一个窄 trait `PushConfigStore`（`Notifier` 靠它读**用户级**渠道
// 配置 ✓）—— 2026-10-08 推翻重做时**整个删掉** ✗：用户口径是「规则配置推送的
// token，包括推送哪几个平台」⇒ "发不发、发给谁"全部来自**规则自己的 `config`**
// ✓，投递层再没有"用户级渠道配置"可读 ✓，那一格 trait 也就没有存在理由了 ✓。
// （它删掉之后 `Notifier` 直接拿 `Arc<dyn Store>`：规则表、面板绑定、节流状态
// 都在 `Store` 上 ✓。）

/// 必填字段的非空校验。**只报路径，不报值**。
fn require_non_empty(path: &str, value: &str) -> crate::Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Config(format!(
            "{path} 不能为空：配了这个渠道却发不出去"
        )));
    }
    Ok(())
}

/// JS 真值口径（与发送侧 `build_tg_bot` 一致 ✓）：`0` / `""` / `null` 都算缺 ✓。
fn js_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
    }
}

// ── 推送规则与面板行为（2026-10-08 冻结口径）──────────────────────────────
//
// 用户原话（**设计就这两句**，别再往下加层 ✓）：
//
// > 「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个平台**」
// > 「**不是规则去命中行为，而是行为选择规则，一个行为可以选择多个规则**」
//
// ⇒ 两条分界：
//
// | 层 | 回答什么 | 落在哪 |
// |---|---|---|
// | ① **规则** | **怎么发**：发到哪几个平台、token、标题、日志等级、汇总方式 | [`PushRule`]（**自带一份 [`PushConfig`]** ✓） |
// | ② **行为** | **什么时候发**：哪个定时 / 哪条编排、哪些运行结局、面板行为的节流 | `Schedule::push_rules` / `Pipeline::push_rules` / [`PanelBindings`] ✓ |
//
// # ⚠ 这一版删掉了什么（都是"看着像少了一格"但故意的 ✓）
//
// * 规则里的 `events` / `channels` / `onlyError` / `minLevel` ✗ ——
//   **前两个是"什么时候发"**（归行为 ✓：`Schedule::push_events` 与
//   `PanelBindings` 各自表达 ✓），**后两个被规则自带的 `config` 顶掉** ✓
//   （`PushConfig` 本来就有这两格 ✓，一份配置一个口径 ✓）。
// * 规则里的 `slowFactor` / `failStreak` ✗ —— 那是"什么时候算异常"，
//   与"怎么发"无关 ✓。它们的判据落在投递侧的判定函数上（见 `mcloud-app::notify` ✓）。
// * **独立的"渠道配置"那一层**（`UserStore::push_config` +
//   `data/push-config.json`）✗ —— 规则自带凭据之后，那一层就是"两份 token
//   可能对不上"的来源 ✓。
// * 旧的 `Schedule::push_rule` / `Pipeline::push_rule`（单条 `Option`）✗ ——
//   用户要的是「一个行为可以选择**多个**规则」✓。
//
// ⚠ 不做兼容：旧形状的 `data/push-rules.json` / `data/push-config.json`
// **启动时直接删掉** ✓（`mcloud-store` 的 `purge_legacy_push_files` ✓），
// 旧字段一个都不保留、不迁移、不读 ✗（用户口径：「目前一切都可以重写，
// 并没有分发 rc4 版本」✓）。

/// 推送规则的 id。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PushRuleId(pub String);

impl PushRuleId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PushRuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// **触发事件** —— 现在只用来表达「一次运行以什么结局收场」✓。
///
/// # 为什么是枚举而不是字符串
///
/// 事件名要落盘（`Schedule::push_events` 里存一串 ✓）、要出现在界面上（多选 ✓）、
/// 要在代码里 `match`（判据 ✓）。用字符串的话，拼错一个（`"run.fail"` /
/// `"runFail"` / `"run_failed"`）不会报错，只会**静默地永不触发** ✗ ——
/// 那是最难查的一类问题 ✓。枚举 + `serde` 的 camelCase 名字：加事件时编译器
/// 会把每一处该改的地方点名 ✓。
///
/// # ⚠ 后四个变体（登录 / 性能）今天**不进** `push_events`
///
/// 用户口径是「**不是规则去命中行为，而是行为选择规则**」✓：登录/性能这类
/// **面板行为**没有定时可绑，它们的选择落在 [`PanelBindings`] 的四个具名格里
/// （`login_new` / `login_failed` / `perf_slow` / `perf_fail_streak` ✓），
/// 而 `push_events` 只对**运行类**事件有意义 ✓（见 `Schedule::push_events` 的说明 ✓）。
///
/// 那四个变体**先留着**：`PushEvent::ALL` / `label()` / `default_selected()` 是
/// 界面那一侧的枚举源 ✓，而"面板行为"与它们是同一批语义 ✓ ——
/// 现在就删会让另一条线（路由/界面）少一份能对上的名字 ✓。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushEvent {
    /// 一次运行以**失败**收场。
    RunFailed,
    /// 一次运行**部分成功**（编排里有的步骤成了、有的没成）。
    RunPartial,
    /// 一次运行**成功**。⚠ 默认**不选** —— 每天准点推一条"一切正常"，用户会去关通知 ✗。
    RunOk,
    /// 一次运行被**主动中止**。
    RunCancelled,
    /// **WebUI 有新登录**（谁、从哪个 IP、什么时候）。
    ///
    /// ⚠ 它的"选择规则 + 节流"落在 [`PanelBindings::login_new`] ✓，不在 `push_events` ✓。
    LoginNew,
    /// **连续登录失败** —— 可能是有人在爆破；这是安全告警。
    LoginFailed,
    /// **性能异常：这次明显慢于它自己的历史**（中位数的若干倍，阈值见
    /// `mcloud-app::notify` 的 `SLOW_FACTOR` ✓）。
    PerfSlow,
    /// **性能异常：同一任务连续失败若干次**（阈值见 `mcloud-app::notify` 的
    /// `FAIL_STREAK` ✓）。
    PerfFailStreak,
}

impl PushEvent {
    /// 全部事件（界面按这个顺序列 ✓，也是"全选"的集合 ✓）。
    pub const ALL: [PushEvent; 8] = [
        PushEvent::RunFailed,
        PushEvent::RunPartial,
        PushEvent::RunOk,
        PushEvent::RunCancelled,
        PushEvent::LoginNew,
        PushEvent::LoginFailed,
        PushEvent::PerfSlow,
        PushEvent::PerfFailStreak,
    ];

    /// 人话名字（界面直接用 ✓）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::RunFailed => "运行失败",
            Self::RunPartial => "部分成功",
            Self::RunOk => "运行成功",
            Self::RunCancelled => "运行被中止",
            // ⚠ 措辞**以用户说法为准**（2026-10-08 用户口径：「登录新设备 / 登录失败」✓）——
            // 原先这两句是「WebUI 新登录 / 登录连续失败」✗，与用户在设置页上看到的
            // 行为名（`BehaviorKey::label()` ✓）对不上：同一件事在界面上有两个名字 ✓。
            // ⇒ 面板类这四句现在与 `BehaviorKey::label()` 的四个名字**逐字一致** ✓
            //（下面 `BehaviorKey::label()` 有一句哨兵用例钉着这条 ✓）。
            // ⚠ 运行类那四句**不动**（它们不是行为，是运行结局 ✓）。
            Self::LoginNew => "登录新设备",
            Self::LoginFailed => "登录失败",
            Self::PerfSlow => "耗时异常",
            Self::PerfFailStreak => "连续失败",
        }
    }

    /// 默认选哪些（**新建定时/编排时给 `push_events` 那份** ✓）。
    ///
    /// 判据：**只在"需要人看一眼"的时候响** ✓ —— 成功/中止不选（每天都响等于没响 ✗），
    /// 安全告警（登录失败）选上 ✓。
    ///
    /// ⚠ 后三个（登录/性能）在这里**只是"界面上默认勾上"**的意思 ✓：它们不进
    /// `push_events`（那是运行类专用 ✓），真正的落点是 [`PanelBindings`] 的四个具名格 ✓。
    pub const fn default_selected() -> [PushEvent; 5] {
        [
            PushEvent::RunFailed,
            PushEvent::RunPartial,
            PushEvent::LoginFailed,
            PushEvent::PerfSlow,
            PushEvent::PerfFailStreak,
        ]
    }
}

/// 渠道键 —— 与 [`PushConfig`] 的十个渠道字段**一一对应**（同一批渠道 ✓）。
///
/// # ⚠ 它今天**不参与投递**
///
/// 2026-10-08 用户口径：「规则配置推送的 token，**包括推送哪几个平台**」✓ ⇒
/// "推哪几个平台"由**规则自带的 `config` 里哪几个渠道字段非空**表达 ✓，
/// 规则上那个 `channels: Vec<PushChannelKey>` 已经**删掉** ✗
/// （它和 `config` 是同一件事的两种说法，留着就一定会对不上 ✓）。
///
/// 这个枚举保留下来只做**人话名字与界面枚举**（`ALL` / `label()` ✓）——
/// 渠道清单本来就要有一个地方按固定顺序列出来 ✓，而不是让界面去猜字段名 ✓。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushChannelKey {
    PushPlus,
    ServerChan,
    WorkWeixin,
    WorkWeixinBot,
    TgBot,
    Bark,
    TwoIm,
    DingTalk,
    Email,
    CustomPost,
}

impl PushChannelKey {
    pub const ALL: [PushChannelKey; 10] = [
        Self::PushPlus,
        Self::ServerChan,
        Self::WorkWeixin,
        Self::WorkWeixinBot,
        Self::TgBot,
        Self::Bark,
        Self::TwoIm,
        Self::DingTalk,
        Self::Email,
        Self::CustomPost,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PushPlus => "PushPlus",
            Self::ServerChan => "Server 酱",
            Self::WorkWeixin => "企业微信应用",
            Self::WorkWeixinBot => "企业微信机器人",
            Self::TgBot => "Telegram Bot",
            Self::Bark => "Bark",
            Self::TwoIm => "双卡短信",
            Self::DingTalk => "钉钉机器人",
            Self::Email => "邮件",
            Self::CustomPost => "自定义 POST",
        }
    }
}

/// 汇总方式："一次运行推一条"还是"一步推一条"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DigestMode {
    /// 一次运行**一条**（默认 ✓ —— 编排十几步的话，逐步推会刷屏 ✗）。
    #[default]
    PerRun,
    /// 每一步**各一条**（想盯细节时用 ✓）。
    PerStep,
}

// ⚠ 这里原先有 `QuietHours`（静默时段：`from` / `to` 两个 `HH:MM` + `contains`
// 跨零点判据）与只服务它的 `parse_hhmm` —— **整块删了** ✗。
//
// 2026-10-08 用户口径：「**不要静默时间**」✓ ⇒ 这个功能整体不存在了：
// 规则上不再有 `quiet` 那一格、投递层不再有"落在静默时段就不发"那一句、
// 界面与 CLI 也不再画/打它 ✓。**不要**按"以后可能还要"把它留在注释或
// 隐藏开关里 ✗ —— 留着就是一条永远不生效的配置，下一个人照着它去查 ✗。

/// 一条推送规则 —— 只回答「**怎么发**」✓。
///
/// ⚠ 里面**没有** `events` / `channels` / `slowFactor` / `failStreak` ✗ ——
/// 那些都是"什么时候发"（归行为：`Schedule::push_events` / `PanelBindings` ✓），
/// 或者已经由 `config` 自己表达（推哪几个平台 = 哪几个渠道字段非空 ✓）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRule {
    pub id: PushRuleId,
    pub name: String,
    /// ⭐ 这条规则自己的**平台与凭据**（复用现成的 [`PushConfig`] ✓ ——
    /// 它已经有 10 个渠道 + `title` / `onlyError` / `minLevel` ✓）。
    ///
    /// # 为什么凭据在**规则**里，而不是单开一层"渠道配置"
    ///
    /// 用户原话：「我设置一个推送规则，**这个规则配置推送的 token，包括推送
    /// 哪几个平台**」✓ ⇒ 一条规则 = 一份完整的"往哪发、怎么发" ✓；
    /// 「推送哪几个平台」就是 `config` 里**哪几个渠道字段非空** ✓ ——
    /// 于是原先那个 `channels: Vec<PushChannelKey>` **没有存在理由** ✗
    /// （同一件事的两种说法，留着就一定会对不上 ✓）。
    ///
    /// ⚠ 老形状（规则里只有 `events`/`channels`，渠道配置另存一份
    /// `data/push-config.json`）**不兼容、不迁移** ✓：用户口径是
    /// 「目前一切都可以重写，并没有分发 rc4 版本」✓ ——
    /// 旧文件由 `mcloud-store` 启动时**直接删掉** ✓。
    pub config: PushConfig,
    /// 汇总方式：一次运行一条 / 一步一条 ✓。
    #[serde(default)]
    pub digest: DigestMode,
    // ⚠ 这里原先有 `quiet: Option<QuietHours>`（静默时段）—— 2026-10-08 用户口径
    // 「**不要静默时间**」⇒ **整块删掉** ✗（字段、类型、跨零点判据、界面两格时间与
    // 投递层那一句一起删 ✓）。留一个 `#[serde(default)]` 的空壳只会让老文件里的
    // `quiet` 键被静默忽略、而界面再也改不动它 ✗。
    #[serde(default = "default_true_push")]
    pub enabled: bool,
    /// **归属**：这条规则属于哪个系统用户（与账号/编排/定时同一条口径 ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,
}

fn default_true_push() -> bool {
    true
}

impl PushRule {
    /// 存之前**必须**过的一关 —— 直接交给 `config` 自己那一条判据 ✓。
    ///
    /// # 为什么这里只是转发（一行都不多写）
    ///
    /// `PushConfig::validate()` 已经把该拦的都拦了（每个渠道的必填项逐个非空 ✓，
    /// 而且**一个渠道都没有就报错** ✓ —— 那条判据正是"规则自带凭据"最要紧的一关 ✓）。
    /// 规则这一层没有自己的字段需要校验 ⇒ 抄一份等于把同一套判据维护两遍 ✗
    /// （改一处忘一处 = 一条规则能存进去但发不出去 ✓）。
    pub fn validate(&self) -> crate::Result<()> {
        self.config.validate()
    }

    /// 新建时的默认形状（界面上点「新建规则」看到的就是它 ✓）。
    ///
    /// ⚠ 渠道**一个都不开**：默认开哪个都是替用户做主（把消息发到一个他没点过的
    /// 平台 ✗）⇒ 空配置 + [`PushRule::validate`] 拦着，逼他显式选 ✓。
    pub fn new_default(id: impl Into<String>, owner: Option<UserId>) -> Self {
        Self {
            id: PushRuleId::new(id),
            name: "新规则".into(),
            config: PushConfig::default(),
            digest: DigestMode::default(),
            enabled: true,
            owner,
        }
    }
}

// ⚠ 这里原先有 `narrow()`（按 `channels` 收窄一份共享配置 ✓）与 `apply_filters()`
// （把规则上的 `onlyError` / `minLevel` 盖到共享配置上 ✓）—— **两个都删了** ✗。
// 用户口径是「规则配置推送的 token，包括推送哪几个平台」⇒ 规则**自己就有一份完整
// 配置** ✓，"收窄"与"覆盖"这两个动作没有对象了（没有共享配置可收窄 ✓）。
// 留着的坏处很实在：它们暗示"还存在一份被多条规则共用的配置"，而下一个人会照着
// 这个暗示去找那份配置 ✗ —— 找不到，然后自己造一个 ✗。

// ── 面板行为与节流（2026-10-08 新增）──────────────────────────────────────
//
// 用户原话：「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓
// 登录、性能这类事件**没有定时/编排可绑** ✗ ⇒ 它们不走 `Schedule::push_rules`，
// 而是走本节的 [`PanelBindings`]（按 owner 分表，落盘 `data/push-throttle.json`
// 与 `data/push-panel.json` ✓）。
//
// ⚠ 用户另一句：「登录，性能，这些可能**频繁触发**，注意」✓ ⇒ 每个面板行为都带
// 节流（[`Throttle`] ✓），且**保守默认**见 [`BehaviorKey::default_throttle`] ✓。

/// 面板行为的名字（没有定时/编排可绑的那几类 ✓）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BehaviorKey {
    /// **WebUI 有新登录**（新设备/新地点）。
    LoginNew,
    /// **登录失败**。
    LoginFailed,
    /// **耗时异常**（这次明显慢于它自己的历史）。
    PerfSlow,
    /// **连续失败**（同一任务连着栽了几回）。
    PerfFailStreak,
}

impl BehaviorKey {
    /// 全部面板行为（界面按这个顺序列 ✓）。
    pub const ALL: [BehaviorKey; 4] = [
        BehaviorKey::LoginNew,
        BehaviorKey::LoginFailed,
        BehaviorKey::PerfSlow,
        BehaviorKey::PerfFailStreak,
    ];

    /// 人话名字（界面直接用 ✓）。
    ///
    /// ⚠ 这四句必须与 [`PushEvent::label()`] 里**面板类那四句**逐字一致
    /// （2026-10-08 用户口径：「登录新设备 / 登录失败」✓）—— 同一个行为在
    /// 「设置页那行标题」（读事件清单的 label ✓）与「推送通知的标题」
    /// （`routes/auth.rs` / `notify.rs` 读本函数的 label ✓）上出现两个名字，
    /// 用户会以为是两件事 ✗。这条由下面 `panel_labels_match_event_labels` 钉住 ✓。
    pub const fn label(self) -> &'static str {
        match self {
            Self::LoginNew => "登录新设备",
            Self::LoginFailed => "登录失败",
            Self::PerfSlow => "耗时异常",
            Self::PerfFailStreak => "连续失败",
        }
    }

    /// **保守默认节流**（用户定的口径 ✓，每个行为都能改 ✓）。
    ///
    /// | 行为 | mode | cooldown | dailyCap |
    /// |---|---|---|---|
    /// | 登录新设备 / 登录失败 | `every`（每次触发都推） | 1800s | 10 |
    /// | 耗时异常 / 连续失败 | `onChange`（只在状态变化时推） | 1800s | 10 |
    ///
    /// # 为什么这两组的 mode 不一样（不是随手写的）
    ///
    /// 登录是**离散事件**（"有人从新设备登进来了"就是一条事实 ✓）⇒ 每次触发都值得看一眼 ✓，
    /// 靠冷却压频次 ✓。性能是**持续状态**（一个任务慢下来会连续几十次都慢 ✓）⇒
    /// 每次触发都推就是刷屏 ✗，所以只在**进入异常**与**恢复正常**那两个拐点上推 ✓
    /// （`onChange` ✓）。两组都给了 1800s 冷却与每日 10 条上限：用户说的是
    /// 「可能**频繁触发**，注意」✓ —— 保守优先，宁可少叫醒一次 ✓。
    pub const fn default_throttle(self) -> Throttle {
        let mode = match self {
            Self::LoginNew | Self::LoginFailed => ThrottleMode::Every,
            Self::PerfSlow | Self::PerfFailStreak => ThrottleMode::OnChange,
        };
        Throttle {
            mode,
            cooldown_secs: 1800,
            daily_cap: 10,
        }
    }
}

/// 某个面板行为选中的规则（**可多选** ✓）+ 它的节流 ✓。
///
/// ⚠ `Default` 给的是"不绑规则 + [`Throttle::default`]（`every` 那档 ✓）+ 两格阈值
/// 留空 ✓"—— 想要**按行为分**的保守默认，走 [`BehaviorKey::default_throttle`] 或
/// [`PanelBindings::default`] ✓（单个 binding 不知道自己属于哪个行为 ✓）。
///
/// ⚠ 这里**不能**再派生 `Eq`：`slow_factor: Option<f64>` 里是浮点 ✓
/// （`PartialEq` 够用：这些绑定只做比较与展示，不当 map 键 ✓）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorBinding {
    /// 这个行为要用的推送规则（**空 = 不推** ✓ —— 与"没配"同义，不是"全推" ✗）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PushRuleId>,
    #[serde(default)]
    pub throttle: Throttle,
    /// 「耗时异常」的倍数判据：超过该任务历史**中位数**的这么多倍算异常。
    /// ⚠ 只有 `perf_slow` 这一格有意义 ✓（别的行为留 `None` ✓）。
    ///
    /// # 为什么是 `Option` 而不是给个默认值
    ///
    /// `None` = **用户没动过** ⇒ 跟随内置默认（3.0 ✓）；`Some(3.0)` = 用户**明确设成 3** ✓。
    /// 两者今天的判定结果一样，但界面/接口分得清"没配过"与"配成了 3" ✓ ——
    /// 与 [`PushConfig`] 的渠道字段
    /// （`None` = 没配 ✓）同一条口径：用 `Option` 表达"用户没设"，
    /// 而不是拿一个默认值冒充用户的选择 ✗。兜底常量只留一处
    /// （`mcloud-app::notify` 的 `SLOW_FACTOR` ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_factor: Option<f64>,
    /// 「连续失败」的条数判据：连着栽这么多回算异常。
    /// ⚠ 只有 `perf_fail_streak` 这一格有意义 ✓。
    ///
    /// `None` = 用户没动过 ⇒ 跟随内置默认（3 ✓）；`Some(n)` = 用户明确设成 n ✓
    /// （理由同上 ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_streak: Option<u32>,
}

impl BehaviorBinding {
    /// 按**这个行为自己的**保守默认造一份（`PanelBindings::default()` 用它 ✓）。
    ///
    /// ⚠ 两格阈值一律 `None` ✓（= 跟随内置默认 3.0 / 3 ✓，见上面 `slow_factor` 的说明 ✓）。
    pub fn with_default_throttle(key: BehaviorKey) -> Self {
        Self {
            rules: Vec::new(),
            throttle: key.default_throttle(),
            slow_factor: None,
            fail_streak: None,
        }
    }
}

/// **面板行为**的绑定（按 owner 一份 ✓，落盘 `data/push-panel.json` ✓）。
///
/// 用户口径：「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓
/// —— 这四个具名格就是那个入口的落点 ✓。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelBindings {
    #[serde(default = "login_new_binding")]
    pub login_new: BehaviorBinding,
    #[serde(default = "login_failed_binding")]
    pub login_failed: BehaviorBinding,
    #[serde(default = "perf_slow_binding")]
    pub perf_slow: BehaviorBinding,
    #[serde(default = "perf_fail_streak_binding")]
    pub perf_fail_streak: BehaviorBinding,
}

fn login_new_binding() -> BehaviorBinding {
    BehaviorBinding::with_default_throttle(BehaviorKey::LoginNew)
}
fn login_failed_binding() -> BehaviorBinding {
    BehaviorBinding::with_default_throttle(BehaviorKey::LoginFailed)
}
fn perf_slow_binding() -> BehaviorBinding {
    BehaviorBinding::with_default_throttle(BehaviorKey::PerfSlow)
}
fn perf_fail_streak_binding() -> BehaviorBinding {
    BehaviorBinding::with_default_throttle(BehaviorKey::PerfFailStreak)
}

impl Default for PanelBindings {
    fn default() -> Self {
        Self {
            login_new: login_new_binding(),
            login_failed: login_failed_binding(),
            perf_slow: perf_slow_binding(),
            perf_fail_streak: perf_fail_streak_binding(),
        }
    }
}

impl PanelBindings {
    /// 按行为取那一格（投递侧只走这一条路 ✓，不再 `match` 一遍字段名 ✓）。
    pub fn get(&self, key: BehaviorKey) -> &BehaviorBinding {
        match key {
            BehaviorKey::LoginNew => &self.login_new,
            BehaviorKey::LoginFailed => &self.login_failed,
            BehaviorKey::PerfSlow => &self.perf_slow,
            BehaviorKey::PerfFailStreak => &self.perf_fail_streak,
        }
    }

    /// 按行为取那一格（可变，设置页改节流用 ✓）。
    pub fn get_mut(&mut self, key: BehaviorKey) -> &mut BehaviorBinding {
        match key {
            BehaviorKey::LoginNew => &mut self.login_new,
            BehaviorKey::LoginFailed => &mut self.login_failed,
            BehaviorKey::PerfSlow => &mut self.perf_slow,
            BehaviorKey::PerfFailStreak => &mut self.perf_fail_streak,
        }
    }

    /// **存盘前的校验**（唯一一份 ✓）——`PUT /api/push/panel` 与
    /// `mcloud push panel` 都走它 ✓。
    ///
    /// # 为什么只校验 `slow_factor`
    ///
    /// `Some(0.0)` 会让"耗时异常"的判据**永远成立**（`耗时 > 中位数 × 0` 恒真 ✓，
    /// 见 `mcloud-app::notify` 的 `perf_verdict` ✓）⇒ 用户设一次 0，
    /// 从此每一次运行都推一条"耗时异常" ✗ —— 那是**静默的刷屏**，
    /// 而界面上的格子看起来完全正常 ✓。非有限数（`NaN` / `inf`）同理：
    /// 比较恒为假或恒为真，判据失去意义 ✓。
    ///
    /// ⚠ `fail_streak: Some(0)` **不在这里拦**：0 是**合法**的语义
    ///（"关掉连判" ✓，见 `perf_verdict` 里那个 `> 0` 守卫 ✓）——
    /// 把它当错误报掉，等于把用户明确表达的一个意愿判成非法 ✗。
    pub fn validate(&self) -> crate::Result<()> {
        for key in BehaviorKey::ALL {
            if let Some(factor) = self.get(key).slow_factor {
                // ⚠ 判据写成"非（有限且 > 0）"而不是"<= 0"：`NaN` 与任何数比较
                // 都是假 ⇒ 用 `<= 0` 会把它放过去 ✗。
                if !(factor.is_finite() && factor > 0.0) {
                    return Err(Error::Config(format!(
                        "{}的倍数必须大于 0（实得 {factor}）—— 0 或负数会让「耗时异常」永远成立",
                        key.label()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// 节流方式（见 [`BehaviorKey::default_throttle`] 那张表 ✓）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThrottleMode {
    /// 每次触发都推（再受冷却与每日上限约束 ✓）。
    #[default]
    Every,
    /// **只在状态变化时推**：进入异常一条、恢复正常一条 ✓。
    OnChange,
}

/// 一个面板行为的节流参数 ✓。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Throttle {
    #[serde(default)]
    pub mode: ThrottleMode,
    /// 同一行为两次推送的**最小间隔**（秒 ✓）。
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// **每日上限**（条 ✓）；超了**记日志**、不静默 ✗。
    ///
    /// ⚠ `0` = **不限**（不封顶 ✓）—— 这是设置页/CLI 对用户的**可见承诺** ✓
    /// （2026-10-08 真机验证发现旧语义反了 ✗，见 [`Throttle::decide`]）。
    #[serde(default = "default_daily_cap")]
    pub daily_cap: u32,
}

fn default_cooldown_secs() -> u64 {
    1800
}

fn default_daily_cap() -> u32 {
    10
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            mode: ThrottleMode::default(),
            cooldown_secs: default_cooldown_secs(),
            daily_cap: default_daily_cap(),
        }
    }
}

/// 面板行为的**状态**（`onChange` 要判"变没变"就靠它 ✓）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BehaviorStatus {
    /// 一切正常（对"恢复正常"那一条有意义 ✓）。
    Normal,
    /// 处于异常（登录失败 / 耗时异常 / 连续失败 ✓）。
    Abnormal,
}

/// 一个行为**最近一次**的发送状态 —— **要落盘** ✓（`data/push-throttle.json` ✓）。
///
/// # 为什么非得落盘（不是内存里攒着就行）
///
/// 用户说的是「登录，性能，这些可能**频繁触发**，注意」✓ —— 而节流正是靠
/// "上次什么时候发的""今天发了几条"这两个读数 ✓。只放内存里 ⇒ **进程一重启，
/// 计数与冷却全归零** ⇒ 重启之后同一批事件会再来一轮 ✗（重启本来就可能紧接着
/// 一串触发：服务刚起来，登录/性能的判定一起跑 ✓）。那正是要防的"风暴" ✓。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorState {
    /// 上次**真的发出去**的时刻（epoch 毫秒 ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sent_ms: Option<i64>,
    /// 计数属于哪一天（东八区天数，见 [`local_day`] ✓）—— 跨天要把计数归零 ✓。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<i64>,
    /// 今天发了多少条 ✓。
    #[serde(default)]
    pub sent_today: u32,
    /// 今天**被节流拦下**多少次 —— 「不许静默」那份留痕 ✓（重启也还在 ✓）。
    #[serde(default)]
    pub throttled_today: u32,
    /// 上次看到的状态（`onChange` 判"变没变" ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<BehaviorStatus>,
}

/// 某个 owner 的**全部面板行为**的节流状态（`data/push-throttle.json` 的一格 ✓）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThrottleState {
    /// ⚠ 用 `HashMap<BehaviorKey, …>` 而不是四个具名字段：这一格是**运行时状态**
    /// （没有"界面要按固定顺序列"的需求 ✗），而 `BehaviorKey` 是现成的键 ✓。
    /// 落盘形状是 `{"behaviors":{"loginNew":{…}}}` ✓。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub behaviors: HashMap<BehaviorKey, BehaviorState>,
}

impl ThrottleState {
    /// 没记过 ⇒ 全默认（`last_sent_ms: None` ⇒ 第一次一定放行 ✓）。
    pub fn get(&self, key: BehaviorKey) -> BehaviorState {
        self.behaviors.get(&key).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, key: BehaviorKey, state: BehaviorState) {
        self.behaviors.insert(key, state);
    }
}

/// 一次节流判定的结果 ✓。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrottleDecision {
    /// 放行，并给出**该写回去的新状态**（`last_sent_ms` / 今日计数都已算好 ✓）。
    Send(BehaviorState),
    /// 拦下，并给出**该写回去的新状态**（只把"今天已节流"加一 ✓）。
    Suppressed {
        reason: ThrottleReason,
        next: BehaviorState,
    },
}

/// 被拦下的原因（要进日志 ✓ —— 「不许静默」✓）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleReason {
    /// `onChange` 且状态没变（持续异常期间 ✓）。
    Unchanged,
    /// 还在冷却期内 ✓。
    Cooldown,
    /// 今天已经到上限 ✓。
    DailyCap,
}

impl ThrottleReason {
    /// 人话原因（日志与测试断言都用它 ✓）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "状态没变化（onChange 只在状态变化时推）",
            Self::Cooldown => "冷却期内",
            Self::DailyCap => "今天已达上限",
        }
    }
}

impl Throttle {
    /// **节流的唯一判据**（纯函数 ✓ —— 时间与"今天"都由调用方给 ✓，于是可测 ✓）。
    ///
    /// 顺序**必须**是下面这样，每一步都有理由：
    ///
    /// 1. **跨天先把计数归零**（`prev.day != day` ✓）—— 不归零的话，昨天用满 10 条
    ///    之后今天一条都发不出来 ✗，而用户看不出哪里错了 ✓。⚠ 只归零计数，
    ///    `last_status` **不动**：状态是跨天的（昨天异常、今天还是异常 ⇒ 不该当成"变化" ✓）。
    /// 2. **"恢复正常"永远放行** ✓ —— 用户口径：恢复是状态变化 ✓，不受冷却与上限影响 ✓。
    ///    否则用户会以为它一直坏着 ✗（这是刻意的：宁可多发一条"好了" ✓）。
    /// 3. `onChange` 且状态没变 ⇒ 拦（**持续异常期间只发一条** ✓）。
    /// 4. 冷却期 ⇒ 拦。5. 每日上限 ⇒ 拦（**只当 `daily_cap > 0`** ✓；`0` = 不限 ⇒ 不拦 ✓）。
    ///
    /// ⚠ "恢复正常"的判据是 **`prev.last_status == Some(Abnormal)` 且这次是 `Normal`** ✓
    /// —— 写成 `status == Normal` 就错了 ✗：第一次看到正常状态（`last_status: None`）
    /// 会平白发一条"已恢复正常"，而用户那边什么都没发生过 ✓。
    pub fn decide(
        &self,
        status: BehaviorStatus,
        now_ms: i64,
        day: i64,
        prev: &BehaviorState,
    ) -> ThrottleDecision {
        let mut next = prev.clone();
        if next.day != Some(day) {
            next.day = Some(day);
            next.sent_today = 0;
            next.throttled_today = 0;
        }

        let recovering =
            prev.last_status == Some(BehaviorStatus::Abnormal) && status == BehaviorStatus::Normal;
        let changed = prev.last_status != Some(status);

        if recovering {
            // ⭐ 永远放行 ✓（见上面第 2 条）。计数**照样加一** —— 它确实是一条发出去的
            // 消息 ✓，不算进去的话"今天发了几条"这个读数就是假的 ✗。
            next.last_status = Some(status);
            next.last_sent_ms = Some(now_ms);
            next.sent_today = next.sent_today.saturating_add(1);
            return ThrottleDecision::Send(next);
        }

        let suppressed = |mut next: BehaviorState, reason: ThrottleReason| {
            next.throttled_today = next.throttled_today.saturating_add(1);
            ThrottleDecision::Suppressed { reason, next }
        };

        if self.mode == ThrottleMode::OnChange && !changed {
            return suppressed(next, ThrottleReason::Unchanged);
        }

        if let Some(last) = next.last_sent_ms {
            let cooldown_ms = self.cooldown_secs.saturating_mul(1_000) as i64;
            if now_ms.saturating_sub(last) < cooldown_ms {
                return suppressed(next, ThrottleReason::Cooldown);
            }
        }

        // ⚠ `>=`（不是 `>`）：`daily_cap = 10` 的意思是"今天最多 10 条" ✓。
        //
        // ⭐ **`daily_cap = 0` = 不限**（不封顶 ✓）—— 这是**用户可见的承诺**：
        // 设置页那四行的提示写的就是「一天最多推这么多条。0 = 不限。」✓，
        // CLI `push panel --daily-cap` 的帮助同样写「0 = 不限」✓。
        // ⇒ 后端**必须**照这个口径做 ✓，不能自己另立一套 ✗。
        //
        // ⚠ 2026-10-08 真机验证发现旧语义是**反的** ✗：那时 `daily_cap = 0` 被当成
        // "一条都不发"（`sent_today >= 0` 恒真 ⇒ 永远拦 ✓），于是按界面语义传 `--daily-cap 0`
        // 的用户**一条推送都收不到** ✗ —— 差点被误判成"推送功能坏了" ✓。
        // ⇒ 只有 `daily_cap > 0` 才因上限拦 ✓；`0` 只是**放开上限**，
        // **冷却与状态变化那两条照旧管** ✓（见上面第 3、4 步，语义未变 ✗）。
        if self.daily_cap > 0 && next.sent_today >= self.daily_cap {
            return suppressed(next, ThrottleReason::DailyCap);
        }

        next.last_status = Some(status);
        next.last_sent_ms = Some(now_ms);
        next.sent_today = next.sent_today.saturating_add(1);
        ThrottleDecision::Send(next)
    }
}

/// epoch 毫秒 → **东八区**的天数（`day` 键 ✓）。
///
/// ⚠ 与推送正文里的时刻是**同一套口径**（固定 +8 ✓，见 [`local_time_hms`]）
/// —— 跨天归零若按另一个时区算，用户会在"他以为的今天"里被莫名拦下 ✗。
pub fn local_day(now_ms: i64) -> i64 {
    (now_ms.div_euclid(1_000) + 8 * 3_600).div_euclid(86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ⚠ 这里原先有 `quiet_hours_handles_the_wrap_around_midnight`（静默时段跨零点）
    // —— 随 `QuietHours` 整块删掉 ✗（2026-10-08 用户口径「不要静默时间」✓）。

    // ── 面板行为的节流（2026-10-08 新增 ✓）──────────────────────────────
    //
    // 用户口径：「登录，性能，这些可能**频繁触发**，注意」✓ ⇒ 判据做成**纯函数**
    // （时间与"今天"都由调用方给 ✓）并逐条钉住 —— 这几条写反了**都不报错** ✗：
    // 一种表现是"该响的时候不响"（用户以为功能坏了），另一种是刷屏（用户去关通知）✓。

    /// 测试用的"现在"与"今天"。
    const NOW: i64 = 1_700_000_000_000;
    const DAY: i64 = 19_675;

    fn sent(decision: ThrottleDecision) -> BehaviorState {
        match decision {
            ThrottleDecision::Send(next) => next,
            other => panic!("期望放行，实际 {other:?}"),
        }
    }

    fn blocked(decision: ThrottleDecision) -> (ThrottleReason, BehaviorState) {
        match decision {
            ThrottleDecision::Suppressed { reason, next } => (reason, next),
            other => panic!("期望拦下，实际 {other:?}"),
        }
    }

    /// ⭐ `onChange`：**持续异常期间只发一条**，**恢复正常必发**（不受冷却与上限约束 ✓）。
    ///
    /// 这条是用户口径里最要紧的一条：恢复那一条如果被冷却/上限拦掉，
    /// 用户会以为它**一直坏着** ✗ —— 那比不推还坏 ✓。
    #[test]
    fn on_change_sends_once_on_entry_and_always_on_recovery() {
        let th = BehaviorKey::PerfSlow.default_throttle();
        assert_eq!(th.mode, ThrottleMode::OnChange);

        // 第一次进入异常 ⇒ 放行（`last_status: None` 不算"没变化" ✓）
        let s1 = sent(th.decide(
            BehaviorStatus::Abnormal,
            NOW,
            DAY,
            &BehaviorState::default(),
        ));
        assert_eq!(s1.last_status, Some(BehaviorStatus::Abnormal));
        assert_eq!(s1.sent_today, 1);

        // 持续异常：**哪怕早过了冷却**也只拦，原因是"状态没变化"（不是冷却 ✓）
        let (reason, s2) =
            blocked(th.decide(BehaviorStatus::Abnormal, NOW + 10 * 3_600_000, DAY, &s1));
        assert_eq!(reason, ThrottleReason::Unchanged);
        assert_eq!(s2.throttled_today, 1, "拦下必须留痕（不许静默）");
        assert_eq!(s2.sent_today, 1, "被拦下不该加发送计数");

        // ⭐ 恢复正常：**距上次只过了 1 秒**（冷却期内）也**必须发** ✓
        let s3 = sent(th.decide(BehaviorStatus::Normal, NOW + 1_000, DAY, &s1));
        assert_eq!(s3.last_status, Some(BehaviorStatus::Normal));

        // 恢复之后再次进入异常 ⇒ 又回到"要过冷却"的规矩上（冷却期内被拦 ✓）
        let (reason, _) = blocked(th.decide(BehaviorStatus::Abnormal, NOW + 2_000, DAY, &s3));
        assert_eq!(reason, ThrottleReason::Cooldown);
        // ⚠ 冷却从**恢复那条**起算（`last_sent_ms` 记的是"上次真的发出去" ✓）
        let s4 = sent(th.decide(BehaviorStatus::Abnormal, NOW + 1_000 + 1_800_000, DAY, &s3));
        assert_eq!(s4.sent_today, 3, "1（进入）+ 1（恢复）+ 1（再进入）");
    }

    /// 冷却期内第二次被拦 ✓（`every` 那档也一样受冷却约束 —— 用户要的是"别刷屏" ✓）。
    #[test]
    fn cooldown_blocks_the_second_trigger() {
        let th = BehaviorKey::LoginNew.default_throttle();
        assert_eq!(th.mode, ThrottleMode::Every);

        let s1 = sent(th.decide(
            BehaviorStatus::Abnormal,
            NOW,
            DAY,
            &BehaviorState::default(),
        ));
        let (reason, next) = blocked(th.decide(BehaviorStatus::Abnormal, NOW + 60_000, DAY, &s1));
        assert_eq!(reason, ThrottleReason::Cooldown);
        assert_eq!(next.sent_today, 1);
        assert_eq!(next.throttled_today, 1);

        // 过了冷却就照发（`every` = 每次触发都推 ✓）
        let s2 = sent(th.decide(BehaviorStatus::Abnormal, NOW + 1_800_000, DAY, &s1));
        assert_eq!(s2.sent_today, 2);
    }

    /// 每日上限到了被拦 ✓，而且**拦下时把"今天已节流"加一**（这就是那份留痕 ✓）。
    #[test]
    fn daily_cap_blocks_and_leaves_a_trace() {
        let th = Throttle {
            mode: ThrottleMode::Every,
            cooldown_secs: 0,
            daily_cap: 2,
        };
        let mut s = BehaviorState::default();
        for _ in 0..2 {
            s = sent(th.decide(BehaviorStatus::Abnormal, NOW, DAY, &s));
        }
        assert_eq!(s.sent_today, 2);

        let (reason, next) = blocked(th.decide(BehaviorStatus::Abnormal, NOW, DAY, &s));
        assert_eq!(reason, ThrottleReason::DailyCap);
        assert_eq!(next.sent_today, 2, "被拦下不加发送计数");
        assert_eq!(next.throttled_today, 1, "「今天已节流 N 次」要能读出来");

        // ⭐ 恢复正常**不受上限影响** ✓（上限拦的是"持续异常"的重复告警 ✓）
        let back = sent(th.decide(BehaviorStatus::Normal, NOW, DAY, &next));
        assert_eq!(back.last_status, Some(BehaviorStatus::Normal));
    }

    /// ⭐ **`daily_cap = 0` = 不限**（用户可见的承诺 ✓，设置页/CLI 都这么写 ✓）。
    ///
    /// ⚠ 2026-10-08 真机验证挖出来的真 bug：旧实现把 `0` 当成"一条都不发" ✗，
    /// 于是照界面语义传 `--daily-cap 0` 的用户**一条推送都收不到** ✗ ——
    /// 这条用例把"0 不拦"钉死，别再写反 ✓。
    #[test]
    fn daily_cap_zero_means_unlimited() {
        let th = Throttle {
            mode: ThrottleMode::Every,
            cooldown_secs: 0,
            daily_cap: 0,
        };
        // 远远超过任何"小上限"的条数：一条都不该被 `DailyCap` 拦下 ✓
        let mut s = BehaviorState::default();
        for i in 0..25 {
            let (reason, next) = match th.decide(BehaviorStatus::Abnormal, NOW, DAY, &s) {
                ThrottleDecision::Send(next) => (None, next),
                ThrottleDecision::Suppressed { reason, next } => (Some(reason), next),
            };
            assert_ne!(
                reason,
                Some(ThrottleReason::DailyCap),
                "第 {i} 条被上限拦了"
            );
            assert_eq!(reason, None, "第 {i} 条本就不该被任何理由拦下（冷却 0 ✓）");
            s = next;
        }
        assert_eq!(s.sent_today, 25, "0 = 不限 ⇒ 计数照加、不封顶");
        assert_eq!(s.throttled_today, 0);
    }

    /// ⭐ **`0` 只放开上限，没放开别的** ✓：`daily_cap = 0` 时**冷却照旧**拦 ✓。
    ///
    /// 这条和上一条是一对 —— 少了它，"不限"很容易被顺手写成"什么都不管" ✗。
    #[test]
    fn daily_cap_zero_still_respects_cooldown() {
        let th = Throttle {
            mode: ThrottleMode::Every,
            cooldown_secs: 1800,
            daily_cap: 0,
        };
        let s1 = sent(th.decide(
            BehaviorStatus::Abnormal,
            NOW,
            DAY,
            &BehaviorState::default(),
        ));
        assert_eq!(s1.sent_today, 1);

        // 冷却期内 ⇒ 仍然被 `Cooldown` 拦（**不是** `DailyCap` ✓）
        let (reason, next) = blocked(th.decide(BehaviorStatus::Abnormal, NOW + 60_000, DAY, &s1));
        assert_eq!(reason, ThrottleReason::Cooldown, "0 只放开上限、没放开冷却");
        assert_eq!(next.sent_today, 1, "被拦下不加发送计数");
        assert_eq!(next.throttled_today, 1, "拦下照旧留痕");

        // 过了冷却 ⇒ 照发 ✓
        let s2 = sent(th.decide(BehaviorStatus::Abnormal, NOW + 1_800_000, DAY, &s1));
        assert_eq!(s2.sent_today, 2);
    }

    /// 跨天：**计数归零**（否则昨天用满之后今天一条都发不出来 ✗），
    /// 但 `last_status` **不归零**（否则每天都会把"持续异常"当成新变化 ✗）。
    #[test]
    fn day_rollover_resets_counts_but_not_status() {
        let yesterday = BehaviorState {
            last_sent_ms: Some(NOW - 2_000_000),
            day: Some(DAY - 1),
            sent_today: 10,
            throttled_today: 7,
            last_status: Some(BehaviorStatus::Abnormal),
        };

        // `every`：昨天满了 10 条，今天照样能发第一条 ✓
        let every = BehaviorKey::LoginNew.default_throttle();
        let s = sent(every.decide(BehaviorStatus::Abnormal, NOW, DAY, &yesterday));
        assert_eq!(s.day, Some(DAY));
        assert_eq!(s.sent_today, 1, "跨天要把今天的计数归零");
        assert_eq!(s.throttled_today, 0, "「今天已节流」也是今天的事");

        // `onChange`：状态没变 ⇒ 拦；但计数已经归零、状态仍是 Abnormal ✓
        let on_change = BehaviorKey::PerfSlow.default_throttle();
        let (reason, next) =
            blocked(on_change.decide(BehaviorStatus::Abnormal, NOW, DAY, &yesterday));
        assert_eq!(reason, ThrottleReason::Unchanged);
        assert_eq!(next.sent_today, 0);
        assert_eq!(next.throttled_today, 1);
        assert_eq!(
            next.last_status,
            Some(BehaviorStatus::Abnormal),
            "状态是跨天的，不能跟着计数一起归零"
        );
    }

    /// 新建面板绑定的**保守默认**（用户定死的那张表 ✓，每个行为都能改 ✓）。
    #[test]
    fn panel_defaults_are_conservative() {
        let panel = PanelBindings::default();
        for key in BehaviorKey::ALL {
            let b = panel.get(key);
            assert!(b.rules.is_empty(), "默认不绑规则 ⇒ 这个行为一条都不发");
            assert_eq!(b.throttle.cooldown_secs, 1800, "{key:?} 的冷却");
            assert_eq!(b.throttle.daily_cap, 10, "{key:?} 的每日上限");
            // 两格阈值默认留空 = **用户没动过** ⇒ 跟随内置默认（3.0 / 3 ✓）。
            // ⚠ 不许在这里填 3.0 / 3 ✗ —— 那就分不清"没配过"与"配成了 3"了 ✓。
            assert!(b.slow_factor.is_none(), "{key:?} 的倍数阈值默认留空");
            assert!(b.fail_streak.is_none(), "{key:?} 的连败阈值默认留空");
        }
        // 登录是离散事件 ⇒ every；性能是持续状态 ⇒ onChange
        assert_eq!(
            panel.get(BehaviorKey::LoginNew).throttle.mode,
            ThrottleMode::Every
        );
        assert_eq!(
            panel.get(BehaviorKey::LoginFailed).throttle.mode,
            ThrottleMode::Every
        );
        assert_eq!(
            panel.get(BehaviorKey::PerfSlow).throttle.mode,
            ThrottleMode::OnChange
        );
        assert_eq!(
            panel.get(BehaviorKey::PerfFailStreak).throttle.mode,
            ThrottleMode::OnChange
        );

        // `get` / `get_mut` 必须指向**同一格**（设置页改完要真的生效 ✓）
        let mut edited = PanelBindings::default();
        edited.get_mut(BehaviorKey::PerfSlow).rules = vec![PushRuleId::new("r1")];
        assert_eq!(edited.perf_slow.rules.len(), 1);
        assert_eq!(edited.get(BehaviorKey::PerfSlow).rules.len(), 1);
        assert!(edited.get(BehaviorKey::LoginNew).rules.is_empty());
    }

    /// ⭐ 面板类那四句 label 在**两个枚举**上必须逐字一致（用户口径
    /// 「登录新设备 / 登录失败」✓）。
    ///
    /// # 为什么要有这条
    ///
    /// 同一个行为有两处会显示名字，而它们读的是**两个不同的枚举**：
    /// * 「设置 → 事件推送」那四行的行标题 ⇒ `GET /api/push/events` 的
    ///   [`PushEvent::label()`] ✓；
    /// * 推送通知自己的标题（`mcloud 登录告警 · …` / `mcloud 性能告警 · …`）⇒
    ///   [`BehaviorKey::label()`] ✓。
    ///
    /// 两边写岔了**不会报错**，只会让用户在两个地方看到两个名字 ✗
    ///（2026-10-08 就是这么岔的：「WebUI 新登录」vs「登录新设备」✓）。
    #[test]
    fn panel_labels_match_event_labels() {
        let pairs = [
            (BehaviorKey::LoginNew, PushEvent::LoginNew),
            (BehaviorKey::LoginFailed, PushEvent::LoginFailed),
            (BehaviorKey::PerfSlow, PushEvent::PerfSlow),
            (BehaviorKey::PerfFailStreak, PushEvent::PerfFailStreak),
        ];
        for (key, event) in pairs {
            assert_eq!(
                key.label(),
                event.label(),
                "{key:?} 在「设置页行标题」与「推送通知标题」上必须是同一个名字"
            );
        }
        // 顺带钉住用户说的那两句（别的措辞都是回归 ✓）
        assert_eq!(BehaviorKey::LoginNew.label(), "登录新设备");
        assert_eq!(BehaviorKey::LoginFailed.label(), "登录失败");
    }

    /// ⭐ `slow_factor` 必须 **> 0 且有限**（`Some(0.0)` 会让"耗时异常"永远成立 ✗）。
    ///
    /// ⚠ 只有**存盘那条路**调它（`PUT /api/push/panel` 与 CLI ✓）——
    /// 读回来的老数据不做二次校验（校验在入口，不在出口 ✓）。
    #[test]
    fn panel_validate_rejects_non_positive_slow_factor() {
        let mut panel = PanelBindings::default();
        assert!(panel.validate().is_ok(), "默认（两格阈值都留空）必须过 ✓");

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            panel.perf_slow.slow_factor = Some(bad);
            let err = panel
                .validate()
                .expect_err(&format!("slowFactor = {bad} 必须被拒"))
                .to_string();
            assert!(err.contains("倍数必须大于 0"), "{bad} 的错误原话：{err}");
        }

        // 正数照旧放行 ✓（1.0 与 2.5 都合法 —— 阈值是"多少倍"，不是"至少多少"）
        for ok in [1.0, 2.5, 100.0] {
            panel.perf_slow.slow_factor = Some(ok);
            assert!(panel.validate().is_ok(), "{ok} 应当放行");
        }

        // ⚠ `fail_streak: Some(0)` **不是**错：0 = 关掉连判 ✓（语义见 `perf_verdict` ✓）
        panel.perf_slow.slow_factor = None;
        panel.perf_fail_streak.fail_streak = Some(0);
        assert!(panel.validate().is_ok(), "0 是合法的连败阈值（= 关掉连判）");
    }

    /// `PushRule::validate()` 就是 `config` 那一条判据（一个渠道都没开 ⇒ 拒 ✓）。
    #[test]
    fn rule_validate_delegates_to_config() {
        let rule = PushRule::new_default("r", None);
        assert!(
            rule.validate().is_err(),
            "新建规则一个渠道都没开 ⇒ 存不进去（否则用户以为配好了）"
        );
        let mut ok = rule.clone();
        ok.config = cfg_from(r#"{"serverChan":{"token":"SCT"}}"#);
        assert!(ok.validate().is_ok());
    }

    /// `local_day` 是**东八区**的天数（与推送正文的时刻同一套口径 ✓）。
    #[test]
    fn local_day_is_beijing_and_rolls_at_midnight() {
        // epoch 那一刻 = 北京 1970-01-01 08:00 ⇒ 第 0 天
        assert_eq!(local_day(0), 0);
        // 北京 1970-01-01 00:30（epoch 前 7.5 小时）⇒ 还是**第 0 天**（东八区的一天 ✓）
        assert_eq!(local_day(-8 * 3_600 * 1000 + 30 * 60 * 1000), 0);
        // 北京 1969-12-31 23:30（epoch 前 8.5 小时）⇒ 才是**前一天**
        assert_eq!(local_day(-8 * 3_600 * 1000 - 30 * 60 * 1000), -1);
        // 2026-10-07T13:57:43Z = 北京 21:57 ⇒ 当天；+2h = 23:57 仍是当天；+3h 才是次日
        let d = 1_791_381_463_000;
        assert_eq!(local_day(d), local_day(d + 2 * 3_600 * 1000));
        assert_ne!(local_day(d), local_day(d + 3 * 3_600 * 1000));
    }

    // ── PushConfig::validate / has_any_channel（2026-10-08 从 account.rs 搬来 ✓）──
    //
    // ⚠ 这几条是**搬家时补的**：原先那些断言住在 `account.rs`（`validate_notify` 的
    // 调用方 ✓），字段一收成用户级就跟着搬走了 ✓ —— 判据不能跟着函数一起消失 ✗。
    //
    // ⚠ 一律用 **JSON** 造（不写结构体字面量 ✓）：既省掉各渠道类型的 `Default` ✗，
    // 又顺带钉住 **wire 名**（camelCase ✓）—— 那正是前端与 CLI 要写的形状 ✓。

    fn cfg_from(json: &str) -> PushConfig {
        serde_json::from_str(json).expect("测试用的 JSON 得是合法配置")
    }

    /// 空配置必须被拒 ✓：一个渠道都没有 = 配了也发不出去 ✓，
    /// 而用户会以为"我配好了" ✗。要关掉推送请整份写 `null` ✓。
    #[test]
    fn empty_config_is_rejected() {
        let err = PushConfig::default().validate().unwrap_err().to_string();
        assert!(err.contains("没有任何渠道"), "{err}");
        assert!(!PushConfig::default().has_any_channel());
    }

    /// 配了渠道但 token 是空白 ⇒ 拒 ✓（存进去之后只会静默发不出去 ✗）。
    #[test]
    fn blank_token_is_rejected() {
        let err = cfg_from(r#"{"pushplus":{"token":"   "}}"#)
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("pushConfig.pushplus.token"), "{err}");
        assert!(err.contains("不能为空"), "{err}");
        // ⚠ **只报路径、不报值** ✓（里面有 token ✓）—— 查的是**那个值本身**
        // （三个空格 ✓），不是"有没有空格" ✗（中文文案里本来就有空格 ✓）。
        assert!(!err.contains("   "), "不许把值带进错误信息：{err}");
    }

    /// 一份真配置 ⇒ 过 ✓，且 `has_any_channel` 认得出来 ✓（读侧靠它把"空"折回 `null` ✓）。
    #[test]
    fn a_real_config_passes_and_counts_as_configured() {
        let cfg = cfg_from(r#"{"title":"我的推送","serverChan":{"token":"SCT"}}"#);
        assert!(cfg.validate().is_ok());
        assert!(cfg.has_any_channel());
    }

    /// ⚠ 那三个**非渠道**旋钮不算"配了渠道" ✓：只有 `title` / `onlyError` / `minLevel`
    /// 时仍是空配置 ✓（否则 `GET /api/push/config` 会把"关掉"回成一份假对象 ✗）。
    #[test]
    fn knobs_alone_are_not_channels() {
        let cfg = cfg_from(r#"{"title":"标题","onlyError":true,"minLevel":"error"}"#);
        assert!(!cfg.has_any_channel());
        assert!(cfg.validate().is_err());
    }

    // ── 「只在出错时推」判的是**严重级别**，不是 type（2026-10-08 用户口径 ✓）──
    //
    // ⚠ 这里**故意偏离 TS**：TS 的 `sendPushNotification` 判 `el.type === "error"`
    // （`test-push-filter.ts` 还有一条「onlyError: fail 不算 error」钉着），照抄会把
    // `fail` / `fatal` 这两条**更严重**的吞掉 ✗ —— 开着"仅错误时推"时，用户最该
    // 收到的告警反而不推 ✓。判据收在 `level_of_type` 一处（error/fail/fatal ⇒ 0 ✓）。

    /// 只有 `fail` ⇒ 也要推（TS 那条"fail 不算"的口径在本仓不成立 ✓）。
    #[test]
    fn only_error_pushes_when_the_only_log_is_fail() {
        let config = cfg_from(r#"{"onlyError":true}"#);
        let logs = PushLogCollector::new();
        logs.write("fail", "这一笔没成", 0);
        assert!(logs.has_error(), "fail 的 level 也是 0（最严重那一档）");
        assert!(config.should_push(&logs), "只有 fail 时 onlyError 也要推");
    }

    /// 只有 `fatal` ⇒ 也要推。
    #[test]
    fn only_error_pushes_when_the_only_log_is_fatal() {
        let config = cfg_from(r#"{"onlyError":true}"#);
        let logs = PushLogCollector::new();
        logs.write("fatal", "整条链挂了", 0);
        assert!(logs.has_error(), "fatal 的 level 也是 0");
        assert!(config.should_push(&logs), "只有 fatal 时 onlyError 也要推");
    }

    /// 只有 `warn` / `info` / `debug` ⇒ 不推（它们没到 0 那一档 ✓）。
    #[test]
    fn only_error_skips_until_something_reaches_the_error_level() {
        let config = cfg_from(r#"{"onlyError":true}"#);
        for ty in ["warn", "info", "debug"] {
            let logs = PushLogCollector::new();
            logs.write(ty, "没事", 0);
            assert!(!logs.has_error(), "{ty} 不该算错误");
            assert!(!config.should_push(&logs), "只有 {ty} 时 onlyError 不推");
        }
        assert!(
            !config.should_push(&PushLogCollector::new()),
            "一条日志都没有时不推"
        );
    }
}
