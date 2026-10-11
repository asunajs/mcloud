//! 事件模型：任务运行过程中向外推送的结构化事件。
//!
//! 前端通过 SSE 消费这些事件，因此**必须可序列化且有稳定的 `type` 标签**。

use serde::{Deserialize, Serialize};

use crate::log::Seq;
use crate::run::{RunId, TaskOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// 日志的**分类** —— 照老项目 `mcloud-sign` 的 consola `type` 体系 ✓
/// （`info / success / fail / warn / error / start / debug / fatal` ✓）。
///
/// # 为什么它与 [`LogLevel`] 是两件事，不能合并
///
/// - [`LogLevel`] 是**严重级别**：筛选、`onlyError`、告警都看它 ✓；
/// - `LogKind` 是**"这条日志是什么"**：界面按它给徽章与颜色 ✓。
///
/// 只有级别的话，「领到了 6 颗」和「这一笔没领到」**都是 info** ✗ ——
/// 用户 2026-10-04 的原话就是「info warn error **success** 等等标记都没有，
/// 更别说颜色」✓。老项目那套 type 正是干这个的 ✓。
///
/// ⚠ `Fail` 与 `Error` 的分工（别混 ✗）：
/// `Fail` = **某一笔**没做成（级别仍是 warn ✓，不该触发 `onlyError` ✓）；
/// `Error` = 整次运行/整条链出错 ✓。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    Info,
    Success,
    Fail,
    Warn,
    Error,
    Start,
    Debug,
}

impl Default for LogKind {
    /// 缺省的分类：`Info` ✓ —— 老记录没有这一格，回落到它最不刺眼 ✓。
    fn default() -> Self {
        LogKind::Info
    }
}

impl LogLevel {
    /// 是否达到给定的最低级别。
    pub fn gte(self, min: LogLevel) -> bool {
        self.rank() >= min.rank()
    }

    fn rank(self) -> u8 {
        match self {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        }
    }
}

/// 运行事件。`tag` 决定 SSE 的 `event:` 名称。
///
/// # ⚠ 为什么这里**没有** `Eq`（而只有 `PartialEq`）
///
/// [`RunEvent::Details`] 带的是 `serde_json::Value`，而 `Value` 只实现
/// `PartialEq`（浮点与数字比较不是等价关系）。原来这一族用的是 `Eq`，
/// 加 `Details` 之后只能退到 `PartialEq` —— 传播到的
/// [`Sequenced`] 与 `mcloud_app::Delivery` 一并退。
///
/// 退这一档的实际代价是零：本仓没有任何地方**依赖** `Eq`
/// （没有把它塞进 `HashMap` 的键、没有 `dedup`、没有 `assert_eq` 的 `Eq` 约束），
/// 而 `PartialEq` 足够撑住"事件相等"这件事的全部现有用法。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    Started {
        run_id: RunId,
        task: String,
        account: String,
        at_ms: i64,
        /// **谁发起的**这次运行：`schedule:<id>` / `pipeline:<id>` / `None`（手点、API、CLI）。
        ///
        /// # 为什么要有这一格（2026-10-07，推送规则那件事带出来的）
        ///
        /// 用户口径：「**用户可将推送规则用于定时任务**」✓ —— 而"这条运行是哪个定时发起的"
        /// 在事件里**根本没有** ✗（`Started` 只有 task/account ✓，`run_labels` 那张表也只记账号 ✓）。
        /// 没有它，投递那一层就只能按账号挑配置 ✗ ⇒ **规则绑在定时上等于绑不上** ✗。
        ///
        /// # 为什么不去翻调度器的状态
        ///
        /// "现在正在跑的是哪条定时"那种反查是**猜** ✗：同一账号可能同时被手点和定时跑，
        /// 而投递发生在运行**结束**时（`notify.rs` 的 spawn ✓）—— 那时调度器早就不在那一发了 ✓。
        /// 发起方在**开跑之前**就知道答案 ⇒ 让它带下来才是准的 ✓。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },
    Log {
        run_id: RunId,
        level: LogLevel,
        /// **分类**（界面按它上徽章与颜色 ✓）。老记录/老前端没有这一格 ⇒ 默认 [`LogKind::Info`] ✓。
        #[serde(default)]
        kind: LogKind,
        message: String,
        at_ms: i64,
    },
    Progress {
        run_id: RunId,
        step: u32,
        total: u32,
        label: String,
        /// 上报时刻（毫秒）。
        ///
        /// ⚠ 2026-10-09 才加的这一格：**落盘的日志行要用它**（回看时"这一步什么时候
        /// 做的"正是要看的东西 ✓）。以前只有 `Log` 带时刻，而进度行落盘时只能拿
        /// "落盘那一刻"的钟 ✗ —— 攒批是 2 秒一次，偏差最多 2 秒 ✓；
        /// 而"从内存回放"那条路连钟都没有（会写成 0 ✗，界面上时间永远是空的 ✓）。
        ///
        /// `default` 是给老前端 / 老测试留的：缺这一格按 0 读 ✓（不是解析失败 ✓）。
        #[serde(default)]
        at_ms: i64,
    },
    Finished {
        run_id: RunId,
        outcome: TaskOutcome,
        duration_ms: u64,
    },
    /// 任务自报的一条**结构化**明细。
    ///
    /// # 与 [`RunEvent::Log`] 的分工
    ///
    /// `Log` 是**给人读**的一句话（进日志、进 `runs.message`）；
    /// 本变体是**给程序读**的形状，由 `runner` 累积并在运行结束时
    /// 并进 `RunRecord.details` —— 那是运行历史里唯一"前端可直接渲染"
    /// 的一格（见 `RunRecord::details` 的说明）。
    ///
    /// # 为什么要走事件，而不是让任务直接写 `details`
    ///
    /// `details` 是**运行结束后**由 runner 组装的（它还要并进 `artifacts`
    /// 那一格，见 `runner.rs` 的收尾段）。任务在跑的过程中拿不到那份记录，
    /// 让它自己去写等于开一条绕过 runner 的旁路。走 `EventSink` 就不一样：
    /// runner 全程通过 sink 观察任务，这条明细与日志、进度**同一条路**进来。
    ///
    /// # `payload` 的形状由**任务自己**定
    ///
    /// 这里刻意不定义结构：`details` 本来就是"每个任务说自己那一格"的地方
    /// （`artifacts` 是云盘产物的那一格，兑换是 `exchange` 那一格）。
    /// 收口在任务的公开类型上（例如 `ExchangeOutcome`），不在这里。
    Details {
        run_id: RunId,
        payload: serde_json::Value,
        at_ms: i64,
    },
}

impl RunEvent {
    /// SSE `event:` 名称。
    ///
    /// ⚠ [`RunEvent::Details`] **刻意与日志共用 `log`**：它是一条结构化明细，
    /// 不是给用户看的一次状态变化，为它单开一个 SSE `event:` 名会让
    /// 「哪些事件要订阅」这件事多一档 —— 而前端不订阅它就会**静默漏掉**
    /// （`EventSource` 只派发注册过的名字）。它随 `log` 一起到达，
    /// 由 `runs.rs` 的日志路径忽略（`details` 走运行记录，不走日志）。
    pub fn event_name(&self) -> &'static str {
        match self {
            RunEvent::Started { .. } => "started",
            RunEvent::Log { .. } | RunEvent::Details { .. } => "log",
            RunEvent::Progress { .. } => "progress",
            RunEvent::Finished { .. } => "finished",
        }
    }

    pub fn run_id(&self) -> RunId {
        match self {
            RunEvent::Started { run_id, .. }
            | RunEvent::Log { run_id, .. }
            | RunEvent::Details { run_id, .. }
            | RunEvent::Progress { run_id, .. }
            | RunEvent::Finished { run_id, .. } => *run_id,
        }
    }

    /// 便捷构造：信息级日志。
    pub fn info(run_id: RunId, message: impl Into<String>, at_ms: i64) -> Self {
        RunEvent::Log {
            run_id,
            level: LogLevel::Info,
            kind: LogKind::Info,
            message: message.into(),
            at_ms,
        }
    }

    /// 脱敏后的副本 —— **事件离开本项目之前的最后一道闸** ✓。
    ///
    /// # 为什么要有它（2026-10-03）
    ///
    /// 复核意见 §1.14 指出：`redact()` 当时**只在 API 错误消息那一处**用过 ✗，
    /// 而任务日志（`Log.message`）、进度文案（`Progress.label`）、结构化明细
    /// （`Details.payload`）与运行结论（`Finished.outcome` 里的 `summary`/`reason`）
    /// 这一整族**没有任何代码侧的闸** ⇒ 上游回显一枚凭证就会顺着
    /// 「SSE 直播流 / 进程日志 / webhook 推送 / 运行记录落盘」四条路出去 ✗✗。
    ///
    /// 交给每个 sink 自己记得脱敏是不行的 ✗（将来多一个 sink 就多一个漏点）。
    /// 所以形状在这里给死：sink 只管 `event.scrubbed()`，一行就够 ✓。
    ///
    /// ⚠ `Started` 的 `task`/`account` **不脱敏**：那是任务名与账号 id，
    /// 界面上本来就要显示 ✓ —— 脱敏的目的是"别把凭证带出去"，不是"什么都藏起来" ✗。
    pub fn scrubbed(&self) -> RunEvent {
        match self {
            RunEvent::Log {
                run_id,
                level,
                kind,
                message,
                at_ms,
            } => RunEvent::Log {
                run_id: *run_id,
                level: *level,
                // ⚠ `kind` 必须**原样带过去** ✗→✓：漏了它，回看时所有分类
                // 都会退化成 `Info`（颜色全丢 ✓），而且不报错 ✗。
                kind: *kind,
                message: redact(message),
                at_ms: *at_ms,
            },
            RunEvent::Progress {
                run_id,
                step,
                total,
                label,
                at_ms,
            } => RunEvent::Progress {
                run_id: *run_id,
                step: *step,
                total: *total,
                label: redact(label),
                at_ms: *at_ms,
            },
            RunEvent::Details {
                run_id,
                payload,
                at_ms,
            } => RunEvent::Details {
                run_id: *run_id,
                payload: redact_value(payload),
                at_ms: *at_ms,
            },
            RunEvent::Finished {
                run_id,
                outcome,
                duration_ms,
            } => RunEvent::Finished {
                run_id: *run_id,
                outcome: outcome.scrubbed(),
                duration_ms: *duration_ms,
            },
            // 没有自由文本可脱（见上面那条说明）
            other @ RunEvent::Started { .. } => other.clone(),
        }
    }
}

/// 带流位置的事件。
///
/// 序号与事件**必须是同一个批次里定下来的**：序号由发布方分配。
/// 若在写库时才重新计算，崩溃重放会给同一批事件算出新序号，
/// 客户端就会看到重复的进度。
///
/// ⚠ 这里是 `PartialEq` 而**不是** `Eq`：它裹着 [`RunEvent`]，而后者带
/// [`RunEvent::Details`]（`serde_json::Value`，只实现 `PartialEq`）。
/// 理由与代价见 `RunEvent` 的说明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequenced {
    pub seq: Seq,
    pub event: RunEvent,
}

impl Sequenced {
    pub fn new(seq: Seq, event: RunEvent) -> Self {
        Self { seq, event }
    }
}

/// 按 run 递增的序号分配器。
///
/// 每条运行一个实例。放在这里而不是让每个发布方自己维护计数器，
/// 是因为"序号连续且单调"是前端续传正确性的前提。
#[derive(Debug, Default)]
pub struct SeqAllocator {
    next: std::sync::atomic::AtomicU64,
}

impl SeqAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取下一个序号。从 1 开始（0 是"还没有事件"的哨兵）。
    pub fn next(&self) -> Seq {
        Seq(self
            .next
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1))
    }

    /// 当前已分配到哪儿（不含未分配的）。
    pub fn current(&self) -> Seq {
        Seq(self.next.load(std::sync::atomic::Ordering::SeqCst))
    }
}

/// 脱敏：把可能出现的凭证替换掉。
///
/// 事件会被写入磁盘并展示在浏览器里，绝不能带出完整凭证。
///
/// # 为什么有"两种形状"（2026-10-03 补）
///
/// 原来这里只认 `key=value`/`key: value` 这一种形状 ✗，于是有个**大洞**：
/// 真实泄漏里最常见的是**光秃秃一枚凭证**（没有键名跟着），例如上游把整段
/// 回显塞进报错 —— `调用失败：eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.…`
/// 或者 `curl -H "Authorization: Bearer eyJ…"`。`key=value` 那套一条都拦不住 ✗。
///
/// ⇒ 现在两层：**键名路标**（6 → 20 个键名）+ **裸形状兜底**
/// （JWT 三段、`Bearer <串>`）。第二层不需要路标，自己就够醒目 ✓。
///
/// ⚠ 它是**字符串**级、不懂结构：`redact_value` 负责把它递归地应用到 JSON 的
/// 每一个字符串上（`RunEvent::Details` / `RunRecord.details` 那一格就是这么处理的 ✓）。
pub fn redact(input: &str) -> String {
    // ⚠ 顺序有讲究，别顺手调换（2026-10-03 被钉子抓过一次）：
    // `Authorization: Bearer <token>` 里，键名层会把**关键词 `Bearer` 自己**吃成
    // `***` ✗，于是方案层再也看不到它 ⇒ 那枚真 token 逃掉 ✗✗。
    // 所以方案层必须**先**跑：先把 `Bearer <值>` 整段处理掉，再让键名层收尾 ✓。
    redact_keyed(&redact_auth_schemes(&redact_jwts(input)))
}

/// 第一层：`键名 = 值` / `键名: 值`（引号可有可无）。
///
/// # 为什么是"扫标识符"而不是"拿清单里的字面量去搜"
///
/// ⚠ 2026-10-04 复审 `co-4`：旧写法是 `for key in SENSITIVE_KEYS { find(key) }`，
/// 而清单里写的是**归一化后**的名字（`verifycode` / `apikey`）⇒ 文本里那三种
/// 真实形状**一个都抓不到** ✗：
///
/// ```text
/// verify_code=123456          ← 清单里没有 "verify_code" 这个字面量
/// api_key: sk-…               ← 同上
/// X-Verify-Code: 9999         ← 同上
/// ```
///
/// JSON 那一层（[`is_sensitive_key`]）一直是对的 ✓ —— 两层的尺子不一样，
/// 就是这处漏的根因。改成"先切出标识符、再归一化比对"之后，两层同一把尺子 ✓。
///
/// # 判定用"归一化后**后缀**相等"，不是整串相等
///
/// 文本里的键**带前缀**是常态：`x-auth-token` / `x-yun-jwttoken`。旧写法靠
/// "子串 + 分隔符紧跟在键名之后"隐式实现了**尾匹配**，所以这里必须保留它 ✓ ——
/// 换成整串相等会把这几种前缀键**从"已脱敏"退回"裸奔"** ✗（那是比原问题更糟的回归）。
fn redact_keyed(input: &str) -> String {
    let mut out = input.to_string();
    let mut scan = 0usize;
    while let Some((key_start, key_end)) = next_identifier(&out, scan) {
        if !text_key_is_sensitive(&out[key_start..key_end]) {
            scan = key_end;
            continue;
        }

        // key 与分隔符之间只允许空白或引号，避免 "authorization" 被 "auth" 命中
        let Some(sep_rel) = out[key_end..].find(['=', ':']) else {
            break;
        };
        let between = &out[key_end..key_end + sep_rel];
        if !between
            .chars()
            .all(|c| c.is_whitespace() || c == '"' || c == '\'')
        {
            scan = key_end;
            continue;
        }

        // 跳过分隔符后的空白
        let mut cursor = key_end + sep_rel + 1;
        while cursor < out.len() {
            let c = out[cursor..].chars().next().expect("cursor 在边界内");
            if c.is_whitespace() {
                cursor += c.len_utf8();
            } else {
                break;
            }
        }

        // 带引号的值取到配对引号；裸值取到常见分隔符
        let (val_start, val_end) = if out[cursor..].starts_with('"') {
            let s = cursor + 1;
            let e = out[s..].find('"').map(|i| s + i).unwrap_or(out.len());
            (s, e)
        } else if out[cursor..].starts_with('\'') {
            let s = cursor + 1;
            let e = out[s..].find('\'').map(|i| s + i).unwrap_or(out.len());
            (s, e)
        } else {
            let e = out[cursor..]
                .find(['&', '"', '\'', ' ', ',', ';', '\n', '}', ']'])
                .map(|i| cursor + i)
                .unwrap_or(out.len());
            (cursor, e)
        };

        if val_end > val_start {
            out.replace_range(val_start..val_end, "***");
        }

        scan = val_start + 3;
    }
    out
}

/// 从 `from` 起找下一个"像键名"的标识符：`[A-Za-z0-9_-]+` 的**极大**段。
///
/// 只按 ASCII 字节判界，多字节字符一律跳过 —— 切片点永远落在 ASCII 标识符的
/// 起止处，不会把某个汉字劈开 ✓。
fn next_identifier(s: &str, from: usize) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    let mut i = from;
    while i < b.len() {
        if is_id(b[i]) {
            let start = i;
            while i < b.len() && is_id(b[i]) {
                i += 1;
            }
            return Some((start, i));
        }
        i += 1;
    }
    None
}

/// 文本层的键名判定：归一化之后，**以某个敏感键名结尾**就算。
///
/// ⚠ 与 JSON 层的 [`is_sensitive_key`]（整串相等）**刻意不同**：文本里的键名
/// 带前缀（`x-auth-token`）。两处别合并成一个函数 —— 合并就意味着要么 JSON 层
/// 开始误伤 `oauth` 这类正常键，要么文本层把前缀键放过去 ✗。
fn text_key_is_sensitive(identifier: &str) -> bool {
    let k = normalize_key(identifier);
    SENSITIVE_KEYS
        .iter()
        .any(|s| k.ends_with(&normalize_key(s)))
}

/// 第二层之一：**JWT**（`eyJ…` + 两个点 + 三段 base64url）。
///
/// 为什么不认键名：这一段本身就是"我是一枚 JWT"的签名 —— `eyJ` 是 `{"` 的
/// base64url（头部必然是 `{"alg":…`）✓。上游回显、报错、`curl` 示例里它常常
/// 单独出现，跟前没有任何键名 ✗。
///
/// ⚠ 判定门槛（`dots == 2 && 长度 >= 24`）是**故意保守**的：文本里恰好出现
/// `eyJ…` 的概率不为零，宁可漏掉短假货，也不要把正常内容吃掉 ✓。
fn redact_jwts(input: &str) -> String {
    const HEAD: &str = "eyJ";
    let b = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        let Some(rel) = input[i..].find(HEAD) else {
            out.push_str(&input[i..]);
            break;
        };
        let start = i + rel;
        out.push_str(&input[i..start]);

        // 从 `eyJ` 起吃一串"JWT 允许的字符"（base64url + `=` + 最多两个点）
        let mut j = start;
        let mut dots = 0usize;
        while j < b.len() {
            let c = b[j];
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'=' {
                j += 1;
            } else if c == b'.' && dots < 2 {
                dots += 1;
                j += 1;
            } else {
                break;
            }
        }
        if dots == 2 && j - start >= 24 {
            out.push_str("***");
            i = j;
        } else {
            // 不是 JWT ⇒ 只吐回 `eyJ` 三个字节，后面的内容一个字都不许吃 ✗
            out.push_str(HEAD);
            i = start + HEAD.len();
        }
    }
    out
}

/// 第二层之二：`Bearer <串>`（HTTP 头最常见的写法）。
///
/// ⚠ **刻意不做 `Basic`**：`basically speaking` 这种正常英文会被它误伤 ✗
/// （"跟着 1~2 个空格 + 8 个以上 token 字符"这个门槛拦不住 ✓），而 `Basic`
/// 那一路通常带着 `authorization:` 键名 ⇒ 第一层已经拦住了 ✓。
fn redact_auth_schemes(input: &str) -> String {
    const KW: &str = "bearer";
    let mut out = input.to_string();
    let mut from = 0usize;
    while let Some(p) = find_ascii_ci(&out, KW, from) {
        let end = p + KW.len();
        // 后面必须是 1~2 个空格（否则只是别的词的一部分）
        let spaces = out[end..].bytes().take_while(|c| *c == b' ').count();
        if spaces == 0 || spaces > 2 {
            from = end;
            continue;
        }
        let val_start = end + spaces;
        // 值只吃"像凭证的 ASCII"：碰到中文、引号、空白就停 ⇒ 不会连正常文本一起吞 ✗
        let val_len = out[val_start..]
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || b"-._~+/=".contains(c))
            .count();
        if val_len >= 8 {
            out.replace_range(val_start..val_start + val_len, "***");
            from = val_start + 3;
        } else {
            from = end;
        }
    }
    out
}

/// 敏感键名清单 —— **两个机制共用这一份** ✓。
///
/// ⚠ 别在别处再抄一份 ✗：`key=value`（文本里）与 `{"key": value}`（JSON 里）
/// 是同一个知识的两处用法，抄开就会漂 ✗。
const SENSITIVE_KEYS: [&str; 20] = [
    // 原来那 6 个
    "authorization",
    "jwttoken",
    "token",
    "ssoToken",
    "auth",
    "cookie",
    // 2026-10-03 补：口令/签名/会话/密钥这一族原来**一个都没有** ✗，
    // 而"登录密码"恰恰是本项目最容易出现在日志里的东西（用户会贴出来问）。
    "password",
    "passwd",
    "pwd",
    "secret",
    "sign",
    "signature",
    "session",
    "sessionid",
    "accesstoken",
    "refreshtoken",
    "apikey",
    "credential",
    "verifycode",
    "captcha",
];

/// 归一化：小写 + 去掉 `_`/`-`/空格。
///
/// 这样 `access_token` / `Access-Token` / `accessToken` 都是 `accesstoken` ✓，
/// 而清单里只需要写一遍 ✓。
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 键名**本身**是否就是敏感名（JSON 那一层用：那里键与值是分开的节点）。
///
/// ⚠ 用"归一化后**相等**"而不是子串包含：`author` 含 `auth` ✗，而
/// `{"author":"张三"}` 是**正常内容**，把它脱敏掉等于制造噪音 ✗。
pub fn is_sensitive_key(key: &str) -> bool {
    let k = normalize_key(key);
    SENSITIVE_KEYS.iter().any(|s| normalize_key(s) == k)
}

/// 把 [`redact`] 递归地应用到一份 JSON 的**每一个字符串**上。
///
/// 为什么必须递归：`RunEvent::Details` 与 `RunRecord.details` 是任务自报的形状
/// （`serde_json::Value`），键名由**任务自己**定 ⇒ 顶层那 20 个键名一个都不认识，
/// 而里面很容易有 `{"url": "https://…?token=…"}` 这种 ✓。
///
/// 数字/布尔/null 原样（它们不可能是凭证文本 ✓），对象/数组按原结构重建 ✓。
pub fn redact_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(redact(s)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| {
                    // ⚠⚠ JSON 里**键名与值是两棵不同的节点** ✗ ⇒ 只扫"值"会把
                    // `{"pwd":"s3cr3t3"}` 整条漏掉（那个值字符串旁边根本没有键名）。
                    // 2026-10-03 就是被用例抓出来的：URL 那种 `?token=…` 能拦住，
                    // 恰好掩盖了这一格 ✗。键名敏感 ⇒ **整格**替换，不进去看了 ✓。
                    if is_sensitive_key(k) {
                        (k.clone(), serde_json::Value::String("***".to_string()))
                    } else {
                        (k.clone(), redact_value(v))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// ASCII 大小写不敏感查找，返回 `hay` 中的字节下标。
///
/// 刻意不用 `to_lowercase()`：那会改变非 ASCII 字符的字节长度，
/// 导致下标错位、进而在切片时 panic。
fn find_ascii_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    let last = h.len() - n.len();
    let mut i = from;
    while i <= last {
        if !hay.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Error.gte(LogLevel::Warn));
        assert!(!LogLevel::Debug.gte(LogLevel::Info));
        assert!(LogLevel::Info.gte(LogLevel::Info));
    }

    #[test]
    fn event_name_is_stable() {
        let e = RunEvent::info(RunId::new(1), "hi", 0);
        assert_eq!(e.event_name(), "log");
        assert_eq!(e.run_id(), RunId::new(1));
    }

    #[test]
    fn redacts_token_like_values() {
        let s = redact("authorization=abc123&foo=bar");
        assert!(s.contains("authorization=***"), "got: {s}");
        assert!(s.contains("foo=bar"), "无关字段不应被改动: {s}");
    }

    #[test]
    fn redacts_json_style() {
        let s = redact(r#"{"jwttoken":"eyJhbGciOi","ok":1}"#);
        assert!(s.contains(r#""jwttoken":"***""#), "got: {s}");
        assert!(s.contains(r#""ok":1"#), "无关字段不应被改动: {s}");
    }

    #[test]
    fn redacts_quoted_key_variants() {
        for input in [
            r#""token": "abc123""#,
            r#"'token': 'abc123'"#,
            r#"token=abc123"#,
            r#"token: abc123"#,
            r#"TOKEN=abc123"#,
        ] {
            let out = redact(input);
            assert!(!out.contains("abc123"), "未脱敏：{input} -> {out}");
        }
    }

    /// ⚠⚠ 复审 `co-4`：键名里的分隔符（`_` / `-`）与大小写不该让脱敏漏掉。
    ///
    /// 清单里写的是**归一化后**的名字（`verifycode` / `apikey`），而旧实现是拿这些
    /// **字面量**去文本里搜 ⇒ 下面前三种一个都抓不到 ✗（JSON 那一层却一直是好的
    /// ⇒ 两层两把尺子，这就是漏的根因）。
    ///
    /// 红测：把 `text_key_is_sensitive` 换回字面量搜索、或改成"整串相等"⇒
    /// `verify_code` / `api_key` / `X-Verify-Code` 三种都不会被脱敏，红。
    #[test]
    fn redacts_keys_with_separators_or_case_inside_the_name() {
        for input in [
            "verify_code=123456",
            "verifyCode=123456",
            "X-Verify-Code: 123456",
            "api_key: sk-live-abcdef",
            "apiKey=sk-live-abcdef",
        ] {
            let out = redact(input);
            assert!(
                !out.contains("123456") && !out.contains("sk-live-abcdef"),
                "未脱敏：{input} -> {out}"
            );
        }

        // ⚠ 前缀键是旧实现靠"子串 + 分隔符紧跟在键名之后"隐式抓到的 ⇒
        // 换成"整串相等"会把这几种从"已脱敏"退回**裸奔**（比原问题更糟）。
        for input in [
            "x-auth-token=SECRETVALUE",
            "x-yun-jwttoken: SECRETVALUE",
            "X-Request-Sessionid=SECRETVALUE",
        ] {
            let out = redact(input);
            assert!(!out.contains("SECRETVALUE"), "前缀键漏了：{input} -> {out}");
        }

        // 正常词不该被误伤：`author` 不以 `auth` **结尾**，`oauth2` 也不是。
        assert_eq!(redact("author=张三"), "author=张三");
        assert_eq!(redact("oauth2=public"), "oauth2=public");
    }

    #[test]
    fn handles_non_ascii_without_panicking() {
        // 中文会让字节下标与字符下标错位，旧实现会在此 panic
        let s = redact("账号：张三，token=secret值，备注：测试");
        assert!(!s.contains("secret值"), "got: {s}");
        assert!(s.contains("张三"), "不应破坏无关中文：{s}");
    }

    #[test]
    fn does_not_redact_key_prefix_words() {
        // "author" 以 "auth" 开头，但不是凭证字段
        let s = redact(r#"{"author":"张三","authenticationInfo":"x"}"#);
        assert!(s.contains("张三"), "不应误伤 author 字段：{s}");
    }

    #[test]
    fn empty_and_missing_values_are_safe() {
        assert_eq!(redact(""), "");
        assert_eq!(redact("token="), "token=");
        // 不 panic 即可
        let _ = redact("token");
        let _ = redact("cookie:");
    }

    #[test]
    fn seq_allocator_starts_at_one_and_is_monotonic() {
        let a = SeqAllocator::new();
        assert_eq!(a.next(), Seq(1), "从 1 开始：0 是'还没有事件'的哨兵");
        assert_eq!(a.next(), Seq(2));
        assert_eq!(a.next(), Seq(3));
        assert_eq!(a.current(), Seq(3));
    }

    #[test]
    fn seq_allocator_is_shared_safely() {
        use std::sync::Arc;
        let a = Arc::new(SeqAllocator::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let a = Arc::clone(&a);
            handles.push(std::thread::spawn(move || {
                let mut got = Vec::new();
                for _ in 0..50 {
                    got.push(a.next().get());
                }
                got
            }));
        }
        let mut all: Vec<u64> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        all.sort_unstable();
        assert_eq!(all.len(), 400);
        // 并发分配不得出现重复序号
        all.dedup();
        assert_eq!(all.len(), 400, "并发分配的序号不能重复");
        assert_eq!(all[0], 1);
        assert_eq!(*all.last().unwrap(), 400);
    }

    #[test]
    fn find_ascii_ci_basic() {
        assert_eq!(find_ascii_ci("Hello World", "world", 0), Some(6));
        assert_eq!(find_ascii_ci("Hello", "xyz", 0), None);
        assert_eq!(find_ascii_ci("aaa", "a", 1), Some(1));
    }

    // ── 2026-10-03：复核意见 §1.14 那批钉子 ────────────────────────────────
    //
    // 这一族用例的判据都是"**凭证文本不许出现在输出里**"，
    // 而不是"输出长什么样" —— 后者会被实现细节牵着走 ✗。

    /// 一枚**真的** JWT（三段 base64url，头必然以 `eyJ` 开头）。
    // ⚠ 必须**一行**写完、中间不许有空白：Rust 的续行/多行拼接会把行首缩进
    // 留在字符串里 ✗，那样 JWT 中间夹着空格 ⇒ 脱敏器会（正确地）拒绝它 ——
    // 夹具一坏，钉子就废了 ✓（这条 2026-10-03 真踩过）。
    const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

    #[test]
    fn redacts_bare_jwt_without_any_key() {
        let out = redact(&format!("调用失败：{JWT} 请稍后重试"));
        assert!(!out.contains("eyJhbGciOi"), "裸 JWT 没被脱敏：{out}");
        assert!(out.contains("调用失败：***"), "描述被吃掉了：{out}");
        assert!(out.ends_with(" 请稍后重试"), "后面的中文被吃了：{out}");
    }

    #[test]
    fn redacts_bearer_scheme() {
        let out = redact("curl -H \"Authorization: Bearer abcdefghijklmnop\"");
        assert!(!out.contains("abcdefghijklmnop"), "{out}");
        // ⚠ 中文/引号要停住：不许把后面的正常内容一起吞了
        let out2 = redact("Bearer abcdefghijklmnop 之后的中文");
        assert!(out2.ends_with("之后的中文"), "{out2}");
    }

    #[test]
    fn keeps_eyj_that_is_not_a_jwt() {
        // 文本里恰好出现 `eyJ` 时**一个字节都不许多吃**（否则正常内容会被啃掉）
        assert_eq!(redact("变量 eyJ 只是举例"), "变量 eyJ 只是举例");
        assert_eq!(redact("eyJ太短了.abc.def"), "eyJ太短了.abc.def");
    }

    #[test]
    fn redacts_password_family_keys() {
        let out = redact("password=hunter2&user=张三");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("张三"), "非凭证字段不该被连累：{out}");
        let out2 = redact(r#"{"pwd":"x9f2","verifyCode":"7788","ok":1}"#);
        assert!(!out2.contains("x9f2") && !out2.contains("7788"), "{out2}");
        assert!(out2.contains(r#""ok":1"#), "不该动别的字段：{out2}");
    }

    #[test]
    fn redact_value_walks_every_json_string() {
        let v = serde_json::json!({
            "url": "https://host/api?token=deadbeef",
            "list": [{ "pwd": "s3cr3t3" }],
            "n": 3,
        });
        let out = redact_value(&v);
        let text = out.to_string();
        assert!(!text.contains("deadbeef"), "{text}");
        assert!(!text.contains("s3cr3t3"), "{text}");
        assert_eq!(out["n"], serde_json::json!(3), "非字符串不该被动：{out}");
    }

    #[test]
    fn scrubbed_covers_log_progress_details_and_finished() {
        // 这四条就是复核意见点名的出站路径 —— 少任何一条，凭证就顺着那条出去 ✗
        let cases = [
            RunEvent::Log {
                run_id: RunId::new(1),
                level: LogLevel::Info,
                // 分类（`kind` 是后加的字段 ⇒ 这一处构造当时漏了 ✗，`--all-targets` 才抓到 ✓）
                kind: crate::event::LogKind::Info,
                message: format!("拿到凭证 {JWT}"),
                at_ms: 0,
            },
            RunEvent::Progress {
                run_id: RunId::new(1),
                step: 1,
                total: 2,
                label: "password=hunter2".into(),
                at_ms: 0,
            },
            RunEvent::Details {
                run_id: RunId::new(1),
                payload: serde_json::json!({ "url": "https://h/?token=deadbeef" }),
                at_ms: 0,
            },
            RunEvent::Finished {
                run_id: RunId::new(1),
                outcome: TaskOutcome::failed(format!("上游回显 {JWT}")),
                duration_ms: 5,
            },
        ];
        for event in cases {
            let text = format!("{:?}", event.scrubbed());
            assert!(!text.contains("eyJhbGciOi"), "没脱敏：{text}");
            assert!(!text.contains("hunter2"), "没脱敏：{text}");
            assert!(!text.contains("deadbeef"), "没脱敏：{text}");
        }
        // `Started` 是账号/任务名（界面上本来就要显示）⇒ **不该**被动
        let started = RunEvent::Started {
            run_id: RunId::new(1),
            task: "sign".into(),
            account: "0f6050105ab111f4".into(),
            at_ms: 0,
            origin: None,
        };
        assert_eq!(started.scrubbed(), started);
    }

    #[test]
    fn task_outcome_scrubbed_keeps_the_shape() {
        let out = TaskOutcome::failed("token=abc123 已失效").scrubbed();
        assert!(matches!(out, TaskOutcome::Failed { .. }), "{out:?}");
        assert!(!out.summary().contains("abc123"), "{out:?}");
        assert!(out.summary().contains("已失效"), "{out:?}");
        // 幂等：再脱一次不变（sink 套 sink 时会重复调用 ✓）
        assert_eq!(out.scrubbed(), out);
    }
    #[test]
    fn sensitive_key_uses_equality_not_substring() {
        // 正向：写法归一化后相等（大小写 / 下划线 / 连字符都不影响）
        for k in [
            "pwd",
            "password",
            "access_token",
            "Access-Token",
            "ssoToken",
            "verifyCode",
        ] {
            assert!(is_sensitive_key(k), "{k} 该被认成敏感键名");
        }
        // 反向：**含**敏感词但不是它 ⇒ 不许误伤（`{"author":"张三"}` 是正常内容）
        for k in [
            "author",
            "authenticationInfo",
            "tokenizer",
            "user",
            "summary",
        ] {
            assert!(!is_sensitive_key(k), "{k} 不该被认成敏感键名");
        }
    }

    #[test]
    fn redact_value_keeps_normal_fields_intact() {
        let v = serde_json::json!({ "author": "张三", "ok": 1, "note": "一切正常" });
        assert_eq!(redact_value(&v), v, "正常字段一个字都不该动");
    }
}
