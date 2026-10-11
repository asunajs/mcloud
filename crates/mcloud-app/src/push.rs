//! 多渠道运行推送 —— 照 mcloud-sign 的 `packages/shared/src/push` 移植。
//! # 这一半在 app，配置那一半在 core
//!
//! 纯配置与日志模型（[`PushConfig`] 及各渠道配置、[`PushLogLevel`]、日志收集器、
//! 过滤/格式化）已下沉到 `mcloud_core::push` —— 因为 `account.rs` 的
//! `AccountSettings::notify` 要引用 `PushConfig`，而 core 不能反向依赖 app。
//! 本文件保留**投递侧**：构造各渠道请求、HMAC 签名、HTTP 投递、脱敏；
//! 并把 core 的那些名字**原样再导出**，既有调用方与测试的路径不变。
//!
//! # 谁在调用
//!
//! `notify.rs` 在每次运行 `Finished` 时按**运行的那个账号**读
//! `AccountSettings::notify`，用 [`PushConfig::push_text`] 决定推不推、推什么，
//! 再调 [`send_notify`] 扇出所有已配置渠道。**没有**进程级兜底：账号没配就
//! 一个包都不发（与 TS 的差异见 `mcloud_core::push` 模块头）。
//!//! # 依据（能跑的 TS 实现，行为以它为准）
//!
//! | 本文件 | mcloud-sign |
//! |---|---|
//! | [`PushConfig`] / 各渠道配置 / [`PushLogEntry`] | `push/types.ts` |
//! | 各 `build_*` / `send_*` / [`send_request`] / [`send_notify`] / [`format_logs_for_push`] | `push/index.ts` |
//! | [`PushLogCollector`] / [`level_of_type`] | `push/transport.ts` |
//! | `title` / `onlyError` / `minLevel` 的默认值 | `config/schema.ts` 的 `MessageConfigSchema` |
//!
//! # 铁律
//!
//! 1. **任何渠道失败都只记一条日志**：不 panic、不冒泡、不影响其它渠道。
//!    `index.ts` 的 `sendRequest` 把整段包在 try/catch 里就是这个意思。
//! 2. **构造请求是纯函数**（各 `build_*`），IO 只发生在 `send_*` 里 ——
//!    可测的逻辑不许埋在 `await` 后面。
//! 3. **日志里不许出现凭证**：错误串里的 URL 一律抹掉，见 [`strip_urls`]。

//! # 与 TS 的差异（逐条都是刻意的）
//!
//! - **超时**：TS 的 `sendRequest` 传了 `timeout: 10000`，但它的
//!   `createHttp().request()` 没有把 `timeout` 转发给 ofetch
//!   （`http/base.ts` 里只传了 method/body/headers）⇒ 那 10s 在 TS 侧
//!   是被**静默丢掉**的（实际生效的是 ofetch 默认 30s）。本模块按任务要求真的用
//!   10s（[`REQUEST_TIMEOUT_MS`]），并且把它写进 [`PushRequest`]，让传输层没法再丢。
//! - **非 2xx**：TS 的 ofetch 对非 2xx 抛异常 ⇒ 落进 catch 分支。本模块等价地把
//!   非 2xx 记为失败。
//! - **时刻格式**：TS 用 `date.toLocaleTimeString()`，结果随运行环境的 locale 与
//!   时区变（不可复现）。本模块固定成**东八区 `HH:MM:SS`**，见 [`local_time_hms`]。
//! - **`email` 未实现**：本版只留配置结构与文档位，发送函数明确回
//!   [`ChannelOutcome::NotImplemented`]，见 [`send_email`]。
//! - **`title` 默认值**是任务指定的 `mcloud 运行推送`（TS schema 里是
//!   `mcloud-v2 运行推送`）。
//! - **`customPost` 的 `body` 字段**：zod schema 只声明了 `data`（且 zod 默认 strip
//!   未知键 ⇒ `body` 实际进不来配置），但 `types.ts` / `index.ts` 里 `body` 是
//!   `data` 的备选。本实现两者都留，取值次序照 `index.ts` 的 `data || body`。

use mcloud_core::event::redact;
use mcloud_core::Clock;
use serde_json::{json, Map, Value};

pub use mcloud_core::push::{
    format_logs_for_push, level_of_type, local_time_hms, BarkConfig, CustomPost, DingTalkConfig,
    EmailConfig, ExtraFields, PushApiData, PushConfig, PushLogCollector, PushLogEntry,
    PushLogLevel, TgBotConfig, TokenConfig, TwoImConfig, TwoImQuery, WorkWeixinBotConfig,
    WorkWeixinConfig, DEFAULT_TITLE,
};

/// 统一投递的超时：10s（任务口径；TS 侧这个值实际被丢掉，见模块头）。
pub const REQUEST_TIMEOUT_MS: u64 = 10_000;

/// 进日志的细节串最长留多少个字符。
///
/// 上游回一个巨大的 HTML 错误页时，原样塞进日志会把整条运行日志冲掉。
const MAX_LOG_DETAIL: usize = 200;

// ==================== 传输层 ====================

/// 一次待投递的推送请求。
///
/// `timeout_ms` 是**请求的一部分**：仓库既有的 [`mcloud_core::HttpClient`] 的
/// `request` 没有超时参数，拿它做适配就只能把 10s 丢掉（TS 那边正是这么丢的）。
#[derive(Debug, Clone, PartialEq)]
pub struct PushRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    pub timeout_ms: u64,
}

impl PushRequest {
    /// 请求体的线上字节（JSON）；没有体就是 `None`。
    pub fn body_bytes(&self) -> Option<Vec<u8>> {
        self.body.as_ref().map(|body| body.to_string().into_bytes())
    }
}

/// 一次投递的响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushResponse {
    pub status: u16,
    pub body: String,
}

impl PushResponse {
    /// 响应体当 JSON 读；读不出来给 `Value::Null`。
    ///
    /// TS 那边 `ctx.fetch` 直接回解析好的对象，解析失败在 ofetch 里已经抛过了；
    /// 这里「解析失败」按 `Null` 处理 —— 它上面没有 `errcode`/`code`/`err`，
    /// 与 TS 解构一个非对象的结果一致（判成功）。
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// 推送的 HTTP 传输。
///
/// ⚠ 不复用 [`mcloud_core::HttpClient`]：它的 `request` 不收超时参数，
/// 适配层只能把 [`PushRequest::timeout_ms`] 丢掉 —— 那正是 TS 侧
/// `timeout: 10000` 被静默丢弃的同一个坑。要真的 10s，超时就得在签名里。
/// 生产实现（reqwest）与测试替身都实现这个 trait。
#[async_trait::async_trait]
pub trait PushTransport: Send + Sync {
    async fn send(&self, req: PushRequest) -> anyhow::Result<PushResponse>;
}

/// 投递上下文：TS `PushContext { logger, fetch }` 的对应物。
///
/// - `http` 对应 `fetch`；
/// - `clock` 给失败日志打时刻（仓库约定：时间只从 `Clock` 来）；
/// - `logs` 对应挂了 `pushTransport` 的 `logger`：失败与跳过都落进同一份日志。
pub struct PushContext<'a> {
    pub http: &'a dyn PushTransport,
    pub clock: &'a dyn Clock,
    pub logs: &'a PushLogCollector,
}

impl<'a> PushContext<'a> {
    pub fn new(
        http: &'a dyn PushTransport,
        clock: &'a dyn Clock,
        logs: &'a PushLogCollector,
    ) -> Self {
        Self { http, clock, logs }
    }

    /// 记一条运行日志：进收集器 + 进 `tracing`。
    ///
    /// 文本先过 [`redact`]（仓库安全红线 §7.3），再落两处。
    fn record(&self, level: u8, r#type: &str, msg: String) {
        let msg = redact(&msg);
        self.logs.write(r#type, msg.clone(), self.clock.now_ms());
        match level {
            0 => tracing::error!(target: "mcloud_app::push", "{}", msg),
            1 => tracing::warn!(target: "mcloud_app::push", "{}", msg),
            _ => tracing::info!(target: "mcloud_app::push", "{}", msg),
        }
    }
}

/// 渠道标识（日志里用的名字逐字照 TS）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Pushplus,
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

impl Channel {
    /// TS 各 `send*` 里传给 `sendRequest` 的 `name`。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pushplus => "pushplus",
            Self::ServerChan => "Server酱",
            Self::WorkWeixin => "企业微信推送",
            Self::WorkWeixinBot => "企业微信Bot推送",
            Self::TgBot => "Telegram Bot 推送",
            Self::Bark => "Bark ios 推送",
            Self::TwoIm => "回逍推送",
            Self::DingTalk => "钉钉推送",
            Self::Email => "邮件消息",
            Self::CustomPost => "自定义消息",
        }
    }
}

/// 单个渠道这一次的结果。
///
/// ⚠ [`ChannelOutcome::Attempted`] **不等于发成功**：成败由统一投递写进日志，
/// 这里只回答「有没有把这一枪交给传输层」。把它与「成功」分开，是 `email` 那条
/// 「不许假装成功」要求能成立的前提。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelOutcome {
    /// 已交给统一投递；成败看日志。
    Attempted,
    /// 按 TS 的跳过条件没发（缺 token / chat_id / key），或没有可发的模板。
    Skipped(&'static str),
    /// 本版未实现（目前只有 [`send_email`]）。
    NotImplemented,
}

/// 统一投递（`sendRequest` 的对应物）。
///
/// 成败判定逐字照 TS `index.ts`：
/// `errcode || err || ![0, 200, undefined].some(c => code === c)` ⇒ 失败。
/// 失败只记日志（errcode 类记 `error`、非 2xx 与传输异常记 `warn`），
/// **绝不 panic、绝不冒泡**。
pub async fn send_request(
    ctx: &PushContext<'_>,
    channel: Channel,
    req: PushRequest,
) -> ChannelOutcome {
    let label = channel.label();
    match ctx.http.send(req).await {
        Ok(resp) if !(200..300).contains(&resp.status) => {
            ctx.record(1, "warn", format!("{label}发送失败: HTTP {}", resp.status));
        }
        Ok(resp) => {
            let data = resp.json();
            if response_is_failure(&data) {
                ctx.record(
                    0,
                    "error",
                    format!("{label}发送失败 {}", safe_reason(&data.to_string())),
                );
            } else {
                ctx.record(3, "info", format!("{label}已发送！"));
            }
        }
        Err(error) => ctx.record(
            1,
            "warn",
            format!("{label}发送失败: {}", safe_reason(&error.to_string())),
        ),
    }
    ChannelOutcome::Attempted
}

/// TS 的成败判据：`errcode || err || code 不在 [0, 200, undefined]`。
///
/// 几处必须逐字对齐 JS 的地方：
/// - `errcode || err` 是 **JS 真值**：`0` / 空串 / `null` / `false` 都算「没失败」，
///   而字符串 `"0"` 算失败；
/// - `code === c` 是 **严格相等**：只有数字 `0` / `200` 以及「没有这个键」放行，
///   字符串 `"0"`、布尔都不放行；
/// - 响应不是 JSON 对象时（TS 解构不出东西）判**成功**。
pub fn response_is_failure(data: &Value) -> bool {
    let Some(object) = data.as_object() else {
        return false;
    };
    let errcode = object.get("errcode").is_some_and(js_truthy);
    let err = object.get("err").is_some_and(js_truthy);
    let code_is_bad = match object.get("code") {
        None | Some(Value::Null) => false,
        Some(Value::Number(number)) => !matches!(number.as_i64(), Some(0 | 200)),
        Some(_) => true,
    };
    errcode || err || code_is_bad
}

/// JS 真值语义。
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        // JS 里对象与数组**一律**真值（空对象、空数组也是）。
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// JS `String(v)`：`twoIm` 的 query 值要照它的样子转。
fn js_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

// ==================== 各渠道：纯构造 ====================

/// 带 JSON 头、POST、10s 的请求骨架。
fn json_request(
    method: &str,
    url: String,
    body: Option<Value>,
    headers: &[(String, String)],
    timeout_ms: u64,
) -> PushRequest {
    let mut merged = vec![("Content-Type".to_string(), "application/json".to_string())];
    for (key, value) in headers {
        // TS 的对象展开会留下**两个大小写不同**的同名头（`Content-Type` 与
        // `content-type` 在 JS 里是两个键），实际生效的是后者。HTTP 语义上按后者
        // 覆盖才是对的，所以这里大小写不敏感地去重，值取渠道给的那个。
        match merged
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
        {
            Some(slot) => *slot = (key.clone(), value.clone()),
            None => merged.push((key.clone(), value.clone())),
        }
    }
    PushRequest {
        method: method.to_string(),
        url,
        headers: merged,
        body,
        timeout_ms,
    }
}

fn build_post(url: String, body: Value) -> PushRequest {
    json_request("POST", url, Some(body), &[], REQUEST_TIMEOUT_MS)
}

/// pushplus：`http://www.pushplus.plus/send`（TS 用的就是 **http**）。
pub fn build_pushplus(config: &TokenConfig, title: &str, text: &str) -> PushRequest {
    let mut body = Map::new();
    body.insert("token".into(), Value::String(config.token.clone()));
    body.insert("title".into(), Value::String(title.to_string()));
    body.insert("content".into(), Value::String(text.to_string()));
    body.extend(config.extra.clone());
    build_post(
        "http://www.pushplus.plus/send".to_string(),
        Value::Object(body),
    )
}

/// Server 酱：`desp` 里每个换行**翻倍**（TS 的 `text.replace(/\n/g, "\n\n")`）。
pub fn build_server_chan(config: &TokenConfig, title: &str, text: &str) -> PushRequest {
    let mut body = Map::new();
    body.insert("text".into(), Value::String(title.to_string()));
    body.insert("title".into(), Value::String(title.to_string()));
    body.insert("desp".into(), Value::String(text.replace('\n', "\n\n")));
    body.extend(config.extra.clone());
    let headers = [("content-type".to_string(), "application/json".to_string())];
    json_request(
        "POST",
        format!("https://sctapi.ftqq.com/{}.send", config.token),
        Some(Value::Object(body)),
        &headers,
        REQUEST_TIMEOUT_MS,
    )
}

/// 企业微信第一步：用 corpid + corpsecret 换 `access_token`。
pub fn build_work_weixin_token(config: &WorkWeixinConfig) -> PushRequest {
    let mut body = Map::new();
    if let Some(corpid) = &config.corpid {
        body.insert("corpid".into(), Value::String(corpid.clone()));
    }
    if let Some(corpsecret) = &config.corpsecret {
        body.insert("corpsecret".into(), Value::String(corpsecret.clone()));
    }
    build_post(
        "https://qyapi.weixin.qq.com/cgi-bin/gettoken".to_string(),
        Value::Object(body),
    )
}

/// 企业微信第二步：`agentid` + 动态类型键（`text` / `markdown` …）。
pub fn build_work_weixin_message(
    config: &WorkWeixinConfig,
    access_token: &str,
    title: &str,
    text: &str,
) -> PushRequest {
    let msgtype = config.msgtype.clone().unwrap_or_else(|| "text".to_string());
    let touser = config.touser.clone().unwrap_or_else(|| "@all".to_string());
    let mut body = Map::new();
    body.insert("touser".into(), Value::String(touser));
    body.insert("msgtype".into(), Value::String(msgtype.clone()));
    if let Some(agentid) = config.agentid {
        body.insert("agentid".into(), Value::Number(agentid.into()));
    }
    body.insert(msgtype, json!({ "content": format!("{title}\n\n{text}") }));
    body.extend(config.extra.clone());
    build_post(
        format!("https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={access_token}"),
        Value::Object(body),
    )
}

/// 企业微信机器人：一个 webhook URL，动态类型键。
pub fn build_work_weixin_bot(config: &WorkWeixinBotConfig, title: &str, text: &str) -> PushRequest {
    let msgtype = config.msgtype.clone().unwrap_or_else(|| "text".to_string());
    let mut body = Map::new();
    body.insert("msgtype".into(), Value::String(msgtype.clone()));
    body.insert(msgtype, json!({ "content": format!("{title}\n\n{text}") }));
    body.extend(config.extra.clone());
    build_post(config.url.clone(), Value::Object(body))
}

/// Telegram Bot。
///
/// 缺 `token` 或 `chat_id`（JS 真值口径：空串 / 0 / null 都算缺）时返回 `None`
/// —— TS 也是先跳过，不是在请求里塞个 `null`。
pub fn build_tg_bot(config: &TgBotConfig, title: &str, text: &str) -> Option<PushRequest> {
    if config.token.is_empty() || !config.chat_id.as_ref().is_some_and(js_truthy) {
        return None;
    }
    let host = config
        .api_host
        .clone()
        .unwrap_or_else(|| "api.telegram.org".to_string());
    let mut body = Map::new();
    body.insert(
        "disable_web_page_preview".into(),
        Value::Bool(config.disable_web_page_preview.unwrap_or(true)),
    );
    if let Some(chat_id) = &config.chat_id {
        body.insert("chat_id".into(), chat_id.clone());
    }
    if let Some(proxy) = &config.proxy {
        body.insert("proxy".into(), Value::String(proxy.clone()));
    }
    body.extend(config.extra.clone());
    // `text` 在 TS 里排在展开之后 ⇒ 永远由它收尾。
    body.insert("text".into(), Value::String(format!("{title}\n\n{text}")));
    Some(build_post(
        format!("https://{host}/bot{}/sendMessage", config.token),
        Value::Object(body),
    ))
}

/// Bark（iOS）。缺 `key` 时 `None`。
pub fn build_bark(config: &BarkConfig, title: &str, text: &str) -> Option<PushRequest> {
    if config.key.is_empty() {
        return None;
    }
    let level = config
        .level
        .clone()
        .unwrap_or_else(|| "passive".to_string());
    let mut body = Map::new();
    body.insert("body".into(), Value::String(text.to_string()));
    body.insert("title".into(), Value::String(title.to_string()));
    body.insert("level".into(), Value::String(level));
    body.extend(config.extra.clone());
    Some(build_post(
        format!("https://api.day.app/{}", config.key),
        Value::Object(body),
    ))
}

/// 回逍（2Im）。`key` / `sid` 走查询串，其余走请求体。
pub fn build_two_im(config: &TwoImConfig, title: &str, text: &str) -> PushRequest {
    let msgtype = config.msgtype.clone().unwrap_or_else(|| "text".to_string());
    let mut pairs: Vec<(String, String)> = vec![
        ("key".to_string(), config.key.clone()),
        ("sid".to_string(), config.sid.clone()),
    ];
    if let Some(query) = &config.query {
        if let Some(plat) = &query.plat {
            pairs.push(("plat".to_string(), plat.clone()));
        }
        if let Some(group) = &query.group {
            pairs.push(("group".to_string(), group.clone()));
        }
        if let Some(icon) = &query.icon {
            pairs.push(("icon".to_string(), js_to_string(icon)));
        }
        for (key, value) in &query.extra {
            pairs.push((key.clone(), js_to_string(value)));
        }
    }
    let url = format!("https://api.2im.cn/push?{}", form_urlencode_pairs(&pairs));
    let mut body = Map::new();
    body.insert("msgtype".into(), Value::String(msgtype));
    body.insert("content".into(), Value::String(text.to_string()));
    body.insert("title".into(), Value::String(title.to_string()));
    body.extend(config.extra.clone());
    build_post(url, Value::Object(body))
}

/// 钉钉机器人：有 `secret` 就加签。
///
/// 签名逐字照 TS：以 secret 为密钥、以「毫秒时间戳 + 换行 + secret」为消息做
/// HmacSHA256，结果 base64 后按 `encodeURIComponent` 转义进查询串。
/// `timestamp_ms` 由调用方从 `Clock` 取，这样纯构造函数可复现（测试用固定向量）。
pub fn build_ding_talk(
    config: &DingTalkConfig,
    title: &str,
    text: &str,
    timestamp_ms: i64,
) -> PushRequest {
    let query = match &config.secret {
        Some(secret) if !secret.is_empty() => {
            let message = format!("{timestamp_ms}\n{secret}");
            let sign = hmac_sha256_base64(secret.as_bytes(), message.as_bytes());
            format!(
                "&timestamp={timestamp_ms}&sign={}",
                encode_uri_component(&sign)
            )
        }
        _ => String::new(),
    };
    let mut body = Map::new();
    body.insert("msgtype".into(), Value::String("text".to_string()));
    // ⚠ 正文前面那个**前导空格**是 TS 原文，别顺手删掉。
    body.insert(
        "text".into(),
        json!({ "content": format!(" {title}\n\n{text}") }),
    );
    body.extend(config.extra.clone());
    let headers = [("content-type".to_string(), "application/json".to_string())];
    json_request(
        "POST",
        format!(
            "https://oapi.dingtalk.com/robot/send?access_token={}{query}",
            config.token
        ),
        Some(Value::Object(body)),
        &headers,
        REQUEST_TIMEOUT_MS,
    )
}

/// 自定义模板。`Ok(None)` = 这个模板没有 URL，跳过（TS 的 `if (!apiTemplate?.url) return;`）。
///
/// 替换规则逐字照 TS，两处**不一样**：
/// - **URL**：`{title}` / `{text}` 各自只替换**第一次**，值按 `encodeURIComponent` 转义；
/// - **请求体**：`{title}` / `{text}` 全局替换且**不转义**，替换完再把真实的
///   换行/回车转成字面量再 `JSON.parse` 回去 —— 所以正文里的换行能活，
///   而标题里带引号会让这一步解析失败（TS 落进 catch），这里相应地回 `Err`。
pub fn build_custom_post(
    api: &PushApiData,
    title: &str,
    text: &str,
) -> anyhow::Result<Option<PushRequest>> {
    if api.url.is_empty() {
        return Ok(None);
    }
    let method = api
        .method
        .as_deref()
        .map(str::to_uppercase)
        .unwrap_or_else(|| "POST".to_string());
    let url = api
        .url
        .replacen("{title}", &encode_uri_component(title), 1)
        .replacen("{text}", &encode_uri_component(text), 1);
    // JS `data || body`：`data` 只要**存在**（哪怕空对象，空对象在 JS 里是真值）就取胜。
    let payload = api.data.clone().or_else(|| api.body.clone());
    let body = match payload {
        Some(fields) if !fields.is_empty() => {
            let raw = serde_json::to_string(&Value::Object(fields))?;
            let patched = raw
                .replace("{title}", title)
                .replace("{text}", text)
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            Some(serde_json::from_str::<Value>(&patched)?)
        }
        _ => None,
    };
    let headers: Vec<(String, String)> = api
        .headers
        .iter()
        .flat_map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect();
    Ok(Some(json_request(
        &method,
        url,
        body,
        &headers,
        api.timeout.unwrap_or(REQUEST_TIMEOUT_MS),
    )))
}

// ==================== HMAC / 编码 ====================

/// `base64(HmacSHA256(key, message))`。
///
/// # 为什么不是 `hmac` crate
///
/// 依赖表里没有 `hmac`（Cargo.lock 里查无此包），而 `sha2` / `base64` 已经有了
/// —— 通过 `mcloud-crypto` 的 [`mcloud_crypto::sha256_hex_bytes`] 与
/// [`mcloud_crypto::b64_encode`]。为一次钉钉签名往依赖表里加一个包不划算，
/// 所以按 RFC 2104 手写这十几行，并用固定向量钉住。
pub fn hmac_sha256_base64(key: &[u8], message: &[u8]) -> String {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&sha256_bytes(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend_from_slice(&inner_pad);
    inner.extend_from_slice(message);
    let inner_digest = sha256_bytes(&inner);
    let mut outer = Vec::with_capacity(96);
    outer.extend_from_slice(&outer_pad);
    outer.extend_from_slice(&inner_digest);
    mcloud_crypto::b64_encode(&sha256_bytes(&outer))
}

/// SHA-256 的**原始摘要**。
///
/// `mcloud-crypto` 只导出 hex 形态（`sha256_hex_bytes`），而 HMAC 要拿 32 字节
/// 原始摘要当下轮输入，所以这里把 64 位小写 hex 解回字节。`expect` 兜的是
/// `sha256_hex_bytes` 的输出契约（64 位 hex），不是外部输入 —— 这个分支不可达。
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let hex_digest = mcloud_crypto::sha256_hex_bytes(data);
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let pair = hex_digest
            .get(index * 2..index * 2 + 2)
            .expect("sha256_hex_bytes 必定回 64 位 hex");
        *byte = u8::from_str_radix(pair, 16).expect("sha256 的 hex 输出必定可解");
    }
    digest
}

/// `encodeURIComponent` 的对应物：字母数字与 `-_.!~*'()` 不转义。
fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if is_uri_component_unreserved(byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn is_uri_component_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
        )
}

/// `application/x-www-form-urlencoded` 序列化（`URLSearchParams.toString()` 的对应物）。
///
/// ⚠ 与 [`encode_uri_component`] **不是**同一套：这里空格编成 `+`，而
/// `~ ! ' ( )` 要转义成 `%7E` / `%21` / `%27` / `%28` / `%29`。twoIm 的
/// URL 要逐字对齐 TS，就靠把两者分开。
fn form_urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b' ' => out.push('+'),
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'*') => {
                out.push(char::from(byte));
            }
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_urlencode_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_urlencode(key), form_urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

// ==================== 日志脱敏 ====================

/// 把错误/响应原文压成一行可进日志的文本：脱敏 → 抹 URL → 截断。
fn safe_reason(raw: &str) -> String {
    let cleaned = strip_urls(&redact(raw));
    cleaned.chars().take(MAX_LOG_DETAIL).collect()
}

/// 把串里的 URL 换成 `<url>`。
///
/// # 为什么必须做
///
/// 好几个渠道的凭证就在 **URL 的路径或查询**里：Bark 的 key、Telegram 的
/// token、twoIm 的 key/sid、钉钉与 Server 酱的 access_token。而 [`redact`]
/// 只认 `k=v` / `k: v` 形态的 `token` 键 —— 路径里的那几种它一个都盖不住。
/// 仓库的安全红线（§7.3）是「日志里绝不能出现完整凭证」，所以宁少一句可读信息，
/// 也不把 URL 写进日志。
fn strip_urls(input: &str) -> String {
    const SCHEMES: [&str; 2] = ["http://", "https://"];
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let hit = SCHEMES.iter().filter_map(|scheme| rest.find(scheme)).min();
        match hit {
            Some(index) => {
                out.push_str(&rest[..index]);
                out.push_str("<url>");
                let after = &rest[index..];
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                rest = &after[end..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

// ==================== 各渠道：投递 ====================

pub async fn send_pushplus(
    ctx: &PushContext<'_>,
    config: &TokenConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    send_request(ctx, Channel::Pushplus, build_pushplus(config, title, text)).await
}

pub async fn send_server_chan(
    ctx: &PushContext<'_>,
    config: &TokenConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    send_request(
        ctx,
        Channel::ServerChan,
        build_server_chan(config, title, text),
    )
    .await
}

/// 企业微信应用：先换 token 再发消息。
///
/// ⚠ TS 拿到 `access_token` 是 `undefined` 时会把 `undefined` 拼进 URL 再打一枪；
/// 这里改为**跳过并记一条 error 日志** —— 拿一个假 token 去打上游只会在上游侧
/// 留一条失败记录，不如在自己这边说清楚。
pub async fn send_work_weixin(
    ctx: &PushContext<'_>,
    config: &WorkWeixinConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    let label = Channel::WorkWeixin.label();
    let access_token = match ctx.http.send(build_work_weixin_token(config)).await {
        Ok(resp) if (200..300).contains(&resp.status) => resp
            .json()
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        Ok(resp) => {
            ctx.record(
                0,
                "error",
                format!("{label}失败: 取 access_token 时 HTTP {}", resp.status),
            );
            return ChannelOutcome::Skipped("取 access_token 失败");
        }
        Err(error) => {
            ctx.record(
                0,
                "error",
                format!("{label}失败: {}", safe_reason(&error.to_string())),
            );
            return ChannelOutcome::Skipped("取 access_token 失败");
        }
    };
    let Some(access_token) = access_token else {
        // 200 但没有 access_token（上游回了 errcode）：也要留痕，不许静默跳过。
        ctx.record(
            0,
            "error",
            format!("{label}失败: gettoken 响应里没有 access_token"),
        );
        return ChannelOutcome::Skipped("缺少 access_token");
    };
    send_request(
        ctx,
        Channel::WorkWeixin,
        build_work_weixin_message(config, &access_token, title, text),
    )
    .await
}

pub async fn send_work_weixin_bot(
    ctx: &PushContext<'_>,
    config: &WorkWeixinBotConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    send_request(
        ctx,
        Channel::WorkWeixinBot,
        build_work_weixin_bot(config, title, text),
    )
    .await
}

pub async fn send_tg_bot(
    ctx: &PushContext<'_>,
    config: &TgBotConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    match build_tg_bot(config, title, text) {
        Some(request) => send_request(ctx, Channel::TgBot, request).await,
        None => {
            ctx.record(
                3,
                "info",
                format!("{}跳过: 缺少 token 或 chat_id", Channel::TgBot.label()),
            );
            ChannelOutcome::Skipped("缺少 token 或 chat_id")
        }
    }
}

pub async fn send_bark(
    ctx: &PushContext<'_>,
    config: &BarkConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    match build_bark(config, title, text) {
        Some(request) => send_request(ctx, Channel::Bark, request).await,
        None => {
            // 这条文案是 TS 的原文（它没走 name 模板，写的不是 "Bark ios 推送"）。
            ctx.record(3, "info", "Bark 推送跳过: 缺少 key".to_string());
            ChannelOutcome::Skipped("缺少 key")
        }
    }
}

pub async fn send_two_im(
    ctx: &PushContext<'_>,
    config: &TwoImConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    send_request(ctx, Channel::TwoIm, build_two_im(config, title, text)).await
}

pub async fn send_ding_talk(
    ctx: &PushContext<'_>,
    config: &DingTalkConfig,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    let request = build_ding_talk(config, title, text, ctx.clock.now_ms());
    send_request(ctx, Channel::DingTalk, request).await
}

/// 自定义模板：**不走** [`send_request`] 的 `errcode` 判定。
///
/// TS 的 `sendCustomPost` 直接拿 `ctx.fetch` 打这一枪，既不判 `errcode`、也不用
/// `POST`（模板自带 method），所以这里单独一条投递路径，只保留共同的铁律：
/// 失败只记日志。模板体 JSON 替换坏掉（TS 的 catch）也算失败，记 `warn`。
pub async fn send_custom_post(
    ctx: &PushContext<'_>,
    post: &CustomPost,
    title: &str,
    text: &str,
) -> ChannelOutcome {
    let label = Channel::CustomPost.label();
    let mut attempted = false;
    for item in post.items() {
        match build_custom_post(item, title, text) {
            Ok(None) => {}
            Ok(Some(request)) => {
                attempted = true;
                match ctx.http.send(request).await {
                    Ok(resp) if !(200..300).contains(&resp.status) => {
                        ctx.record(1, "warn", format!("{label}发送失败: HTTP {}", resp.status))
                    }
                    Ok(_) => ctx.record(3, "info", format!("{label}已发送！")),
                    Err(error) => ctx.record(
                        1,
                        "warn",
                        format!("{label}发送失败: {}", safe_reason(&error.to_string())),
                    ),
                }
            }
            Err(error) => ctx.record(
                1,
                "warn",
                format!("{label}发送失败: {}", safe_reason(&error.to_string())),
            ),
        }
    }
    if attempted {
        ChannelOutcome::Attempted
    } else {
        ChannelOutcome::Skipped("没有可用的自定义请求")
    }
}

/// email 渠道的「未实现」说明（进日志，也供测试与接线方引用）。
pub const EMAIL_NOT_IMPLEMENTED: &str = "email 渠道本版未实现：TS 走 Node 专属 email.node.ts（nodemailer/SMTP），Rust 侧要拉 SMTP 客户端依赖，属独立增量；已配 email 也不会发送。";

/// **未实现**的 email 渠道。
///
/// # 为什么留空
///
/// TS 的 `sendEmail` 依赖 Node 专属的 `email.node.ts`（nodemailer + SMTP）。
/// Rust 侧要复刻就得引入 SMTP 客户端依赖，而本任务允许改动的文件里**不含**
/// `crates/mcloud-app/Cargo.toml`（也不许自作主张加依赖）⇒ 本版不发邮件。
///
/// # 为什么不是「静默不发」
///
/// 静默不发 = 配置看起来生效了、实际一条都没出去，属于本仓最忌讳的
/// 「不发错、不报错，只是东西不对」。所以这里**往日志收一条 warn**，
/// 并明确返回 [`ChannelOutcome::NotImplemented`] —— 配了 email 是**可观测的未实现**，
/// 不是成功。
pub async fn send_email(
    ctx: &PushContext<'_>,
    _config: &EmailConfig,
    _title: &str,
    _text: &str,
) -> ChannelOutcome {
    ctx.record(1, "warn", EMAIL_NOT_IMPLEMENTED.to_string());
    ChannelOutcome::NotImplemented
}

/// 遍历**所有已配置**的渠道逐个发（`sendNotify` 的对应物）。
///
/// - 没配的渠道**一枪都不发**（TS 的 `if (!channelConfig) continue`）；
/// - 一个渠道失败不影响其余（每个 `send_*` 自己把失败吃掉，见 [`send_request`]）；
/// - 顺序是 [`PushConfig`] 的字段顺序，与 TS 的 `PushConfig` 声明顺序一致。
pub async fn send_notify(ctx: &PushContext<'_>, config: &PushConfig, title: &str, text: &str) {
    if let Some(channel) = &config.pushplus {
        send_pushplus(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.server_chan {
        send_server_chan(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.work_weixin {
        send_work_weixin(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.work_weixin_bot {
        send_work_weixin_bot(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.tg_bot {
        send_tg_bot(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.bark {
        send_bark(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.two_im {
        send_two_im(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.ding_talk {
        send_ding_talk(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.email {
        send_email(ctx, channel, title, text).await;
    }
    if let Some(channel) = &config.custom_post {
        send_custom_post(ctx, channel, title, text).await;
    }
}

// ── 推送地址的闸（2026-10-04 复审 sv-8）──────────────────────────────────

/// 推送地址的**安全检查**。不合规 ⇒ `Err(原因)`，调用方**不许发**。
///
/// # 为什么必须有这条闸
///
/// 推送地址是**用户配置里的整条 URL**（`customPost.url`、企业微信机器人的
/// `workWeixinBot.url`、TG 的 `apiHost` …），而发请求的是**服务进程**。不设闸的话，
/// 有权改某个账号设置的人就能让服务向任意地址发任意方法、带任意头的请求
///（`http://127.0.0.1:3000/api/...`、云元数据 `169.254.169.254`、内网段扫描），
/// 而响应体不回给调用方 ⇒ 这是一条**盲** SSRF，状态码还会进推送日志 ✗。
/// 单用户本机部署下这只是"管理员自伤"，但本仓的归属模型是多用户、`MCLOUD_ADDR`
/// 还可以绑到 LAN ⇒ 那时第二个用户就能拿它当内网探针 ✗。
///
/// # 闸放行什么、挡住什么
///
/// * 只允许 `http` / `https`（挡掉 `file:` / `gopher:` 这类 scheme）；
/// * 拒**字面量**的回环 / 私网 / 链路本地（含云元数据 `169.254.169.254`）/
///   未指定 / 组播 / 文档段 / CGNAT（`100.64/10`）地址；
/// * 拒 `localhost` 与 `.localhost` / `.local` / `.internal` 这类**名字**。
///
/// # ⚠⚠ 2026-10-06 安全审计 `SEC-04`：判据必须交给 `url` crate，不能自己切字符串
///
/// 旧实现是"按 `://` 切、再按 `@` / `:` / `[]` 切出主机，最后 `host.parse::<IpAddr>()`"。
/// 而真正发请求的 reqwest 用的是 **WHATWG** 的 URL 解析 —— 两把尺子不一样，
/// 差额就是一条绕过（**已实测**，见下面 `validate_push_url` 的注释）。
/// 现在两边都交给同一份实现：`url::Url::parse` → `Url::host()` ✓。
///
/// # ⚠ 闸分两层（2026-10-09 加固：第二层看**真实解析出来的 IP**）
///
/// * **第一层**（本函数，字面量、不联网）—— 挡"直接写 IP / 本机名字"；
/// * **第二层**（[`validate_push_url_resolved`]，要 DNS）—— 挡"域名解析到本机/内网"，
///   也就是 `http://127.0.0.1.nip.io:3998/` 这一类（**已实测绕过**过第一层 ✗）。
///
/// ⚠ 真正发请求的那条路（`notify.rs` 的 `HttpPushTransport`）走的是**第二层** ✓；
/// 本函数留着有两条理由：① 它是第二层的**第一跳**（两者共用同一份字面量判据，
/// 见 [`check_push_url_literal`] ✓）；② 它是**同步**的、**不联网** ⇒ 以后要在
/// "不想为一次校验去碰 DNS"的地方（设置页即时校验那类）先用它挡一遍，直接用 ✓。
///
/// # 诚实的边界（第二层也解决不了的）
///
/// 第二层解析完到真正连上之间，DNS 还能变（**DNS rebinding / TOCTOU**）⇒
/// 本轮**没有**把连接钉在已核过的 IP 上（做不到的理由与残留写在
/// [`validate_push_url_resolved`] 上 ✓，别当成已解决 ✗）。
pub fn validate_push_url(url: &str) -> Result<(), String> {
    match check_push_url_literal(url)? {
        LiteralCheck::Pass { .. } => Ok(()),
        LiteralCheck::Local { reason } => Err(reason),
    }
}

/// 字面量那一层的结论（[`check_push_url_literal`] 的返回值）。
enum LiteralCheck {
    /// 字面量判据过了。`domain` 非空 ⇒ **还需要 DNS**（名字 + 端口）；
    /// 为空 ⇒ 字面量本身就是已核过的公网 IP，判完了 ✓。
    Pass { domain: Option<(String, u16)> },
    /// 指向本机 / 内网（字面量 IP 或本机名字）。`reason` 是给用户看的原因。
    Local { reason: String },
}

/// **字面量那一层**的实现：解析 URL、判 scheme、判字面量 IP、判本机/内网名字。
///
/// ⚠ 单独抽出来是为了让 [`validate_push_url`]（同步）与
/// [`validate_push_url_resolved`]（要 DNS）**共用同一份判据** ——
/// 写两份迟早分叉，而分叉的那一半就是绕过 ✗。
///
/// ⚠ `Err` 只用于"**与内网无关**"的错（不是合法 URL / scheme 不对 / 没有主机名）：
/// 这类错**任何开关都不放行**（`MCLOUD_ALLOW_LOCAL_PUSH` 管的是"地址指向哪"，
/// 不是"用什么协议"）✓。指向本机/内网一律走 `Ok(LiteralCheck::Local { .. })` ✓。
fn check_push_url_literal(url: &str) -> Result<LiteralCheck, String> {
    use url::Host;

    // ⚠⚠ 为什么是 `Url::parse` 而不是自己切字符串（2026-10-06 审计 `SEC-04`，**实测**）：
    //
    // 下面这些写法在 WHATWG 里**都是** `127.0.0.1`（`url-2.5.8/src/host.rs:297-361`
    // 的 `parse_ipv4number` / `parse_ipv4addr` 认十六进制、前导零八进制、单段十进制，
    // 并会去掉末尾空段），而"自己切出来再 `parse::<IpAddr>()`"看到的分别是
    // `2130706433` / `0x7f000001` / `017700000001` / `127.0.0.1.` —— **都不是合法
    // IP 字面量** ⇒ 被当成普通域名放行 ✗：
    //
    // ```text
    // http://2130706433/        http://0x7f000001/     http://017700000001/
    // http://127.0.0.1./        http://[::ffff:127.0.0.1]/
    // ```
    //
    // 实测方式（独立小程序，只读链接工作区的 `liburl`，不写 `target/`）：
    // `$HOME/.cargo/bin/rustc -O /tmp/ssrf_check.rs -o /tmp/ssrf_check \
    //    -L dependency=target/debug/deps --extern url=target/debug/deps/liburl-*.rlib`
    let parsed = url::Url::parse(url).map_err(|e| format!("推送地址不是合法 URL（{e}）：{url}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("推送地址只允许 http/https，实得 {other}：{url}")),
    }

    let Some(host) = parsed.host() else {
        return Err(format!("推送地址没有主机名：{url}"));
    };

    match host {
        // `url` 已经把十进制/十六进制/八进制/末尾点这些写法归一化成真正的 IP ⇒
        // 这里拿到的就是"reqwest 会去连的那个地址" ✓
        Host::Ipv4(v4) => {
            if !is_public_ip(&std::net::IpAddr::V4(v4)) {
                return Ok(LiteralCheck::Local {
                    reason: format!("推送地址指向本机/内网（{v4}），拒绝发送：{url}"),
                });
            }
            Ok(LiteralCheck::Pass { domain: None })
        }
        Host::Ipv6(v6) => {
            if !is_public_ip(&std::net::IpAddr::V6(v6)) {
                return Ok(LiteralCheck::Local {
                    reason: format!("推送地址指向本机/内网（{v6}），拒绝发送：{url}"),
                });
            }
            Ok(LiteralCheck::Pass { domain: None })
        }
        Host::Domain(name) => {
            // ⚠ 末尾点在 DNS 里与不带点**等价**（`localhost.` = `localhost`）⇒
            // 先削掉再比，否则下面那几条判据一个字符就被绕过去了 ✗
            let name = name.trim_end_matches('.');
            if name.is_empty() {
                return Err(format!("推送地址没有主机名：{url}"));
            }
            if name == "localhost"
                || name.ends_with(".localhost")
                || name.ends_with(".local")
                || name.ends_with(".internal")
            {
                return Ok(LiteralCheck::Local {
                    reason: format!("推送地址指向本机/内网名字（{name}），拒绝发送：{url}"),
                });
            }
            // ⚠ 交给第二层去解析。端口只影响 `lookup_host` 的入参形状（DNS 不关心
            // 端口 ✓），取不到就用 0（reqwest 的 `resolve_to_addrs` 也是这个约定 ✓）。
            Ok(LiteralCheck::Pass {
                domain: Some((
                    name.to_string(),
                    parsed.port_or_known_default().unwrap_or(0),
                )),
            })
        }
    }
}

// ── 第二层：看**真实解析出来的 IP**（2026-10-09）────────────────────────────

/// 用户**显式**放行本机/内网推送地址的环境变量。
///
/// ⚠⚠ **管理员级 / 服务级**，**不是**每个用户自己能勾的开关 ——
/// 后者等于把闸拆了（普通用户勾一下就能拿服务当内网探针 ✗）。
/// 命名与既有环境变量（`MCLOUD_ADDR` / `MCLOUD_DATA_DIR` / `MCLOUD_COOKIE_SECURE` ✓）
/// 对齐 ✓。
pub const ALLOW_LOCAL_PUSH_ENV: &str = "MCLOUD_ALLOW_LOCAL_PUSH";

/// DNS 解析的**超时**：一条坏 DNS 不许把推送卡住（推送整体本来就只有 10s ✓）。
///
/// 3s 的取法：公网 DNS 正常在 100ms 内回；3s 已经远超正常值，又明显小于推送的
/// 10s 总预算 ⇒ 解析卡住时**这一条推送**失败（日志留原因 ✓），不影响别的渠道 ✓。
///
/// ⚠ 它是**加在**推送那 10s **之外**的（闸在 `HttpPushTransport::send` 里跑在
/// `tokio::time::timeout(req.timeout_ms, …)` **之前** ✓）⇒ 最坏情况是
/// 3s（解析卡住）+ 10s（请求超时）= 13s，比加固前多 3s ✗。
/// 为什么这样摆而不是把解析算进那 10s：`PushRequest::timeout_ms` 今天就是
/// "**这一枪 HTTP 请求**的预算" ✓（`notify.rs` 的 `HttpPushTransport::send` 拿它
/// `tokio::time::timeout` 包住 `self.http.request(..)` ✓），改它要动推送别的语义 ✗
/// ⇒ 不改，只把这条代价写在这里 ✓。
/// 实际影响很小：只有 DNS 坏掉时才会走到 3s，而那时这条推送本来也要失败 ✓。
pub const DNS_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// `MCLOUD_ALLOW_LOCAL_PUSH` 的**解析规则**（单独一个函数是为了可测，与
/// `mcloud-server` 的 `cookie_secure_from_env` 同一形状 ✓）。
///
/// ⚠ **只有真值才放行**：`1` / `true` / `yes`。
/// **读不到（`None`）= 拒** —— 别写成"读不到就当开" ✗；`0` / `false` / 别的串也一律是拒 ✓。
pub fn parse_allow_local_push(raw: Option<&str>) -> bool {
    matches!(raw, Some("1") | Some("true") | Some("yes"))
}

/// 从环境变量读 [`ALLOW_LOCAL_PUSH_ENV`]（缺省 = **关** ✓）。
pub fn allow_local_push_from_env() -> bool {
    parse_allow_local_push(std::env::var(ALLOW_LOCAL_PUSH_ENV).ok().as_deref())
}

/// DNS 解析的**可替换接口**。
///
/// ⚠⚠ 抽成 trait 的**唯一**理由是测试：单测里给一个假解析器就能把
/// "名字解析到 127.0.0.1"这类场景逐条钉死，而**不用真去打公网 DNS** ✗
/// （真打 DNS 的测试会在离网机器上红、在别人机器上绿，还会变慢 ✓）。
#[async_trait::async_trait]
pub trait PushHostResolver: Send + Sync {
    /// 解析 `host`。失败 ⇒ `Err(原因)`（调用方**不许**把失败当放行 ✗）。
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<std::net::IpAddr>, String>;
}

/// 生产用的解析器：交给 OS（`getaddrinfo`，异步封装在 tokio 的阻塞池里 ✓）。
pub struct SystemResolver;

#[async_trait::async_trait]
impl PushHostResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<std::net::IpAddr>, String> {
        let addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| format!("DNS 解析失败（{host}）：{e}"))?;
        Ok(addrs.map(|a| a.ip()).collect())
    }
}

/// 闸的**结论**（放行时还要告诉调用方"是不是靠开关放的"）。
///
/// ⚠ 为什么要有这个返回值而不是只回 `Ok(())`：单测里抓 `tracing` 日志**不稳**
/// （全局订阅者、并发、`RUST_LOG` 都会影响 ✗）⇒ 把"是否因开关放行"做成
/// **可断言的结构** ✓。日志照发（见 [`local_verdict`] ✓），但它不是唯一证据 ✓。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushUrlVerdict {
    /// 地址指向公网 ⇒ 正常放行，没动开关。
    Public,
    /// 本该拒，但 `MCLOUD_ALLOW_LOCAL_PUSH` 开着 ⇒ 放行。
    /// `reason` 就是"本来要拒的原因"（同时已经 `warn` 过一条 ✓）。
    AllowedByLocalSwitch { reason: String },
}

impl PushUrlVerdict {
    /// 这一次放行**是不是**靠 [`ALLOW_LOCAL_PUSH_ENV`] 换来的。
    pub fn allowed_by_switch(&self) -> bool {
        matches!(self, Self::AllowedByLocalSwitch { .. })
    }
}

/// **第二层**：在字面量判据之上，把域名**真正解析出来**，逐个 IP 判公网。
///
/// # 判据
///
/// 1. 先跑 [`check_push_url_literal`]（快、不联网 ✓）；它判 `Local` ⇒ 走下面的开关；
/// 2. 是域名就解析（**带超时** [`DNS_LOOKUP_TIMEOUT`] ✓）：解析失败 / 超时 /
///    一个地址都没有 ⇒ **拒**（⚠ 不许"解析失败就放过" ✗）；
/// 3. 解析出来的**每一个** IP 都必须是公网（[`is_public_ip`]）—— **只要有一个
///    不合格就拒** ✗（只挑第一个看 = 绕过 ✓）；
/// 4. 全过 ⇒ [`PushUrlVerdict::Public`] ✓。
///
/// # ⚠⚠ TOCTOU / DNS rebinding：这条**没有**解决（诚实记录）
///
/// 本函数解析完到 reqwest 真正连上之间，DNS **还能变** —— 攻击者控制的权威 DNS
/// 可以在两次查询里给不同答案（第一次公网、第二次 `127.0.0.1`）✗。
///
/// 真正的堵法是"把连接**钉在已核过的那个 IP** 上"，而本仓今天**做不到**，理由具体：
///
/// * reqwest 0.12.28 的 `ClientBuilder::resolve` / `resolve_to_addrs`
///   （`reqwest-0.12.28/src/async_impl/client.rs:2270` / `:2278` ✓ **已核过源码**）
///   是 **builder 级**的 —— 只作用于**新建的 `Client`**，**没有** per-request 版本 ✗；
/// * 而本仓的 HTTP 出口是 `mcloud_core::HttpClient` 这个抽象，底下
///   `ReqwestClient`（`mcloud-upstream/src/client.rs:108`）**共用两个预建的
///   `reqwest::Client`**（`inner` / `legacy` ✓），且 `HttpRequest`
///   （`mcloud-core/src/traits.rs:26`）里**没有**"这次请求钉哪个 IP"的位置 ✗；
/// * ⇒ 要钉就得给 `HttpClient` 加一条新方法 + 让 `ReqwestClient` **每次推送现建
///   一个 `Client`**（要复制 profile / cookie / gzip / 超时一整套设置，重复就是
///   分叉的来源 ✗），这是**跨 crate 改动共享层**，超出"推送 SSRF 闸"这一件事的范围，
///   也和"别顺手改推送别的语义"冲突 ⇒ 本轮**不做**，只把残留写在这里 ✓。
///
/// 残留的**实际**大小：窗口是从本函数返回（已核过）到 reqwest 自己再解析那一次，
/// 中间只隔一个函数调用 ⇒ 要打中得**精确踩点**这个毫秒级窗口，比第一层的洞难得多；
/// 但它**不是 0** ✓。要真正关掉这条，走上面那三步（下一轮单独做 ✓）。
///
/// ⚠ 另外：**重定向**那条路已经堵着（推送客户端 `no_redirects` ✓）——
/// 否则"先答公网、再 302 到 127.0.0.1"根本不用抢窗口 ✗。
pub async fn validate_push_url_resolved(
    url: &str,
    resolver: &dyn PushHostResolver,
    allow_local: bool,
) -> Result<PushUrlVerdict, String> {
    validate_push_url_resolved_within(url, resolver, allow_local, DNS_LOOKUP_TIMEOUT).await
}

/// [`validate_push_url_resolved`] 的实现（超时是入参 ⇒ 单测能用毫秒级超时钉住
/// "解析卡住 ⇒ 拒"那条 ✓，不必真等 3 秒）。
async fn validate_push_url_resolved_within(
    url: &str,
    resolver: &dyn PushHostResolver,
    allow_local: bool,
    dns_timeout: std::time::Duration,
) -> Result<PushUrlVerdict, String> {
    let (host, port) = match check_push_url_literal(url)? {
        LiteralCheck::Pass { domain: None } => return Ok(PushUrlVerdict::Public),
        LiteralCheck::Local { reason } => return local_verdict(reason, url, allow_local),
        LiteralCheck::Pass {
            domain: Some(host_port),
        } => host_port,
    };

    // ⚠⚠ 解析必须**带超时**：一条坏 DNS 不许把推送卡住（`tokio::time::timeout`
    // 包住 —— 与 `notify.rs` 里给推送本身加超时是同一手法 ✓）。
    let ips = match tokio::time::timeout(dns_timeout, resolver.resolve(&host, port)).await {
        Ok(Ok(ips)) if !ips.is_empty() => ips,
        Ok(Ok(_)) => {
            return local_verdict(
                format!("推送地址的名字 {host} 解析不出任何地址，拒绝发送：{url}"),
                url,
                allow_local,
            )
        }
        Ok(Err(e)) => {
            return local_verdict(
                format!("推送地址的名字 {host} 解析失败（{e}），拒绝发送：{url}"),
                url,
                allow_local,
            )
        }
        Err(_) => {
            return local_verdict(
                format!(
                    "推送地址的名字 {host} 解析超时（{}ms），拒绝发送：{url}",
                    dns_timeout.as_millis()
                ),
                url,
                allow_local,
            )
        }
    };

    // ⚠ **每一个** IP 都要是公网：一个公网 + 一个 `127.0.0.1` 的应答也必须拒 ✗
    // （只判第一个/任一个是常见的写错法，等于给绕过留门 ✓）。
    let bad: Vec<std::net::IpAddr> = ips.iter().copied().filter(|ip| !is_public_ip(ip)).collect();
    if !bad.is_empty() {
        let list = bad
            .iter()
            .map(|ip| ip.to_string())
            .collect::<Vec<_>>()
            .join("、");
        return local_verdict(
            format!("推送地址的名字 {host} 解析到本机/内网地址（{list}），拒绝发送：{url}"),
            url,
            allow_local,
        );
    }
    Ok(PushUrlVerdict::Public)
}

/// 一条**指向本机/内网**（或解析不出来）的判定该走哪边：
/// 开关关着 ⇒ `Err`（拒，**这一条推送**失败 ✓）；开关开着 ⇒ 放行 **+ 一条 warn** ✓。
///
/// ⚠ 放行**必须留痕**，不许静默 ✗ —— 用户口径是"我显式允许了，但我要在日志里看见它" ✓。
/// ⚠ 日志在这里发（而不是让调用方自己记得发）：调用方有 4 处以上，靠"记得"迟早漏一处 ✗。
fn local_verdict(reason: String, url: &str, allow_local: bool) -> Result<PushUrlVerdict, String> {
    if !allow_local {
        return Err(reason);
    }
    // ⚠ 留痕的**正文**由 [`local_switch_warn_message`] 单独产出（可断言 ✓），
    // 这里只负责真的把它打出去 —— 结构化字段（url / reason / env）一并带上 ✓。
    let message = local_switch_warn_message(url, &reason);
    tracing::warn!(
        url = %url,
        reason = %reason,
        env = ALLOW_LOCAL_PUSH_ENV,
        "{message}"
    );
    Ok(PushUrlVerdict::AllowedByLocalSwitch { reason })
}

/// 开关放行时那条 warn 的**正文**（单独抽出来是为了**可断言** ✓）。
///
/// ⚠ 为什么不是"直接写在 `tracing::warn!` 里"：单测抓 `tracing` **不稳**
/// （`with_default` 是线程局部的，而 `tracing` 的 interest 缓存是**进程全局**的 ——
/// 并行跑的另一个用例先在没有 subscriber 时碰过同一个 callsite，事件就再也出不来了 ✗，
/// 实测过：同一份代码全量跑绿、`--exact` 单独跑红）。⇒ 把正文做成**纯函数**，
/// 逐字断言；"有没有真的打出去"由 `local_verdict` 里那一处 `warn!` 保证（只有一处 ✓）。
fn local_switch_warn_message(url: &str, reason: &str) -> String {
    format!(
        "用户显式允许了本机/内网推送地址（{ALLOW_LOCAL_PUSH_ENV}=1），照发：{url}（本来要拒的原因：{reason}）"
    )
}

/// 这个 IP 能不能当推送目标（能 ⇒ `true`）。
///
/// 判据是"**公网**才放行"：私网 / 回环 / 链路本地 / 未指定 / 组播 / 广播 /
/// 文档段 / CGNAT 一律拒。用白名单式的写法而不是"列几个已知内网段"，
/// 免得漏掉 `0.0.0.0`、`224.0.0.0/4` 这类不常见但同样不该打的地址 ✓。
fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_public_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // ⚠⚠ IPv4-mapped（`::ffff:127.0.0.1`）：下面那几条 IPv6 判据**一条都不认它**
            // （它的 segments 不是 `[0,…,1]`，也不是 fc00::/7、fe80::/10），
            // 而 OS 连它时走的就是 IPv4 ⇒ 不拆出来按 IPv4 判就等于放行了一条打本机的路 ✗
            //（2026-10-06 审计 `SEC-04` 实测：`http://[::ffff:127.0.0.1]/` 能过旧闸）。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(&v4);
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()          // fc00::/7
                || v6.is_unicast_link_local()) // fe80::/10
        }
    }
}

/// IPv4 那半边的判据（单独抽出来，是因为 IPv4-mapped 的 IPv6 也要走它）。
fn is_public_v4(v4: &std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        // CGNAT `100.64.0.0/10`（运营商级 NAT —— 不是公网）
        || (o[0] == 100 && (64..=127).contains(&o[1])))
}

#[cfg(test)]
mod tests;
