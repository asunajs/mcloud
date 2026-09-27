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
    },
    Log {
        run_id: RunId,
        level: LogLevel,
        message: String,
        at_ms: i64,
    },
    Progress {
        run_id: RunId,
        step: u32,
        total: u32,
        label: String,
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
            message: message.into(),
            at_ms,
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
pub fn redact(input: &str) -> String {
    const KEYS: [&str; 6] = [
        "authorization",
        "jwttoken",
        "token",
        "ssoToken",
        "auth",
        "cookie",
    ];
    let mut out = input.to_string();
    for key in KEYS {
        let mut search_from = 0usize;
        // 每轮都可能因替换而改变后续偏移，故重新扫描
        while let Some(key_start) = find_ascii_ci(&out, key, search_from) {
            let key_end = key_start + key.len();

            // key 与分隔符之间只允许空白或引号，避免 "authorization" 被 "auth" 命中
            let Some(sep_rel) = out[key_end..].find(['=', ':']) else {
                break;
            };
            let between = &out[key_end..key_end + sep_rel];
            if !between
                .chars()
                .all(|c| c.is_whitespace() || c == '"' || c == '\'')
            {
                search_from = key_end;
                if search_from >= out.len() {
                    break;
                }
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

            search_from = val_start + 3;
            if search_from >= out.len() {
                break;
            }
        }
    }
    out
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
}
