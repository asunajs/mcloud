//! 推送的**纯配置与日志模型**。
//!
//! # 为什么这半截在 core
//!
//! 每账号的推送渠道是 `AccountSettings::notify: Option<PushConfig>` 的一格
//! （见 [`crate::account`]），而 `AccountSettings` 属于 `mcloud-core`。core 不能
//! 反向依赖 `mcloud-app`，所以「配置长什么样」必须与「怎么发出去」分家：
//!
//! - **本文件**：serde 定义、日志条目/收集器、纯过滤与格式化函数 —— 一行 IO
//!   都没有，符合 core 的铁律（见 lib.rs 模块头）。
//! - **`mcloud-app::push`**：构造各渠道请求、HTTP 投递、HMAC 签名、脱敏；
//!   它从本模块**原样再导出**这些类型，既有调用方与测试的路径不变。
//!
//! # 与 TS 的差异：全局一份 vs 每账号一份
//!
//! `mcloud-sign` 是**单用户**工具：它只有一份全局推送配置，那是为了 **DRY**
//! ——同一个用户的多个账号不想把同一份配置抄很多遍。
//! 本仓是**多用户服务** ⇒ 收件地址是**账号**的属性：同一进程里 A、B 两个账号
//! 必须能各配各的渠道，绝不能把 A 的渠道套到 B 的运行上。所以配置挂在账号上，
//! 且**没有进程级兜底**（「一份 env 管全进程」那种写法已整个删掉）。
//!
//! ⚠ **未做项（刻意留白，别偷偷补成 fallback）**：TS 那种「同一份配置套多个
//! 账号」的正确形态是**用户级模板**（一个用户一份，账号可选引用）。本版没做。
//! 要做就在用户维度加模板 —— 在进程维度加兜底恰好是串号的来源。
//!
//! # 其余与 TS 的差异
//!
//! 超时、非 2xx 判定、时刻格式、email 未实现等说明留在 `mcloud-app::push`
//! 的模块头，因为它们描述的是**投递**那一侧。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
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

    /// 本次运行有没有 **`error` 类型**的日志。
    ///
    /// ⚠ 判据是 **type**，不是 `level`：TS 的 `sendPushNotification` 写的是
    /// `logs.some(el => el.type === "error")`，所以 `fail` / `fatal`（level 同样是 0）
    /// **不算**。它自己的 `test-push-filter.ts` 有一条
    /// 「onlyError: fail 不算 error」钉着这个行为。
    pub fn has_error(&self) -> bool {
        self.with_logs(|logs| logs.iter().any(|entry| entry.r#type == "error"))
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
    /// `onlyError` 且本次**没有 error 类型**日志时不推（`fail` 不算，见
    /// [`PushLogCollector::has_error`]）。
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
}
