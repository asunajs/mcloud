//! 运行日志模型。
//!
//! 日志**按 run 归档**而不是一条全局流：前端要"看这次运行的日志"，
//! 全局流还得再按 run_id 过滤一遍；而断线重连时按 run 取也更直接。
//!
//! 保留策略见 [`LOG_LIMIT_PER_RUN`]：一次运行的日志条数有上限。
//! 不设上限的话，一个死循环打日志的任务能把磁盘写满 ——
//! 而这属于"任务写错了"，不该让整个服务跟着倒下。

use serde::{Deserialize, Serialize};

use crate::event::LogLevel;

/// 单次运行保留的日志条数上限。
///
/// 取 2000：足够覆盖正常任务的完整过程（实测多在几十到几百条），
/// 又能在任务失控时把内存与磁盘占用钉住。
pub const LOG_LIMIT_PER_RUN: usize = 2000;

// 用编译期断言而不是 `#[test]` 里的 `assert!`：后者会被 clippy 的
// `assertions_on_constants` 判为"会被优化掉"，而且它本来就不该等到
// 跑测试才发现——常量写坏了应该在编译时就炸。
const _: () = assert!(LOG_LIMIT_PER_RUN >= 500, "上限太小会把正常任务的日志截断");
const _: () = assert!(LOG_LIMIT_PER_RUN <= 10_000, "上限太大则失去保护意义");

/// 一条日志。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// 所属运行。
    pub run_id: u64,
    pub level: LogLevel,
    pub message: String,
    /// 毫秒时间戳。
    pub at_ms: i64,
    /// 该运行内的递增序号。
    ///
    /// 前端断线重连后可以带上已收到的最大 `seq` 续传；
    /// 只靠时间戳不行 —— 同一毫秒内的多条日志会乱序或漏掉。
    pub seq: Seq,
}

impl LogLine {
    pub fn new(
        run_id: u64,
        level: LogLevel,
        message: impl Into<String>,
        at_ms: i64,
        seq: Seq,
    ) -> Self {
        Self {
            run_id,
            level,
            message: message.into(),
            at_ms,
            seq,
        }
    }
}

/// 事件/日志的**流位置**。
///
/// # 为什么要有这个 newtype
///
/// "已看到的序号"与"下一个要取的序号"都是整数、只差 1。把后者当成前者
/// 传给"从某处开始取"的接口，会**静默跳过一条** —— 不报错，只是少一条日志。
///
/// 所以用两个命名构造器把意图写进代码：
/// - [`Seq::seen`]：客户端**已经看到** n（下次要 n+1）
/// - [`Seq::resume_after`]：要从 n **之后**继续（内部存 n，语义是"大于 n"）
///
/// 两者数值相同，语义不同 —— 调用点必须显式选一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);

impl Seq {
    /// 起点：还没收到任何事件。
    ///
    /// 用 0 而不是 1：0 表示"没有下界"，缺失的 `Last-Event-ID`
    /// 与"从头开始"于是是同一条路径，不需要分支。
    pub const ZERO: Seq = Seq(0);

    /// 客户端**已经看到** `n`，下次应收到 `n + 1`。
    pub const fn seen(n: u64) -> Self {
        Seq(n)
    }

    /// 客户端要从 `n` **之后**继续（即"我已经看到 n"）。
    ///
    /// 与 [`Seq::seen`] 数值相同、意图不同：这个名字用在
    /// "把它交给从某处开始取的接口"那一侧，读代码时能立刻看出
    /// 传的是"已看到的值"而不是"下一个要取的值"。
    pub const fn resume_after(n: u64) -> Self {
        Seq(n)
    }

    /// 下一条。
    ///
    /// **饱和而不是回绕**：回绕会让序号变小，于是"大于 after"这个
    /// 续传判据永远为假 —— 那次运行从此再也发不出任何事件，
    /// 而且是一个到不了、却让整条流彻底静默的分支。
    pub const fn next(self) -> Self {
        Seq(self.0.saturating_add(1))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Seq> for u64 {
    fn from(s: Seq) -> u64 {
        s.0
    }
}

/// 追加日志时的返回：是否发生了截断。
///
/// 调用方据此发一条"日志已截断"的提示 —— 静默丢弃会让人以为
/// 任务只跑到一半就停了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// 正常追加。
    Appended,
    /// 超出上限，**丢弃了最旧的一条**（保留最新的更有用）。
    Trimmed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_line_roundtrips_with_camel_case() {
        let l = LogLine::new(7, LogLevel::Warn, "注意", 1_700_000_000_000, Seq(3));
        let j = serde_json::to_string(&l).unwrap();
        assert!(j.contains("\"runId\":7"), "字段应为 camelCase：{j}");
        assert!(j.contains("\"atMs\""), "字段应为 camelCase：{j}");
        let back: LogLine = serde_json::from_str(&j).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn seq_is_monotonic_by_construction() {
        let a = LogLine::new(1, LogLevel::Info, "a", 0, Seq(1));
        let b = LogLine::new(1, LogLevel::Info, "b", 0, Seq(2));
        assert!(b.seq > a.seq, "seq 必须递增，前端才能续传");
    }

    #[test]
    fn seq_zero_is_the_starting_point() {
        assert_eq!(Seq::ZERO.get(), 0);
        assert_eq!(Seq::ZERO.next(), Seq(1));
    }

    #[test]
    fn seen_and_resume_after_are_equal_in_value_but_distinct_in_intent() {
        // 数值相同 —— 正因如此才需要两个名字
        assert_eq!(Seq::seen(3), Seq::resume_after(3));
        assert_eq!(Seq::seen(3).get(), 3);
    }

    /// 饱和而不是回绕：回绕会让"序号 > after"永远为假，整条流静默。
    #[test]
    fn seq_next_saturates_instead_of_wrapping() {
        let max = Seq(u64::MAX);
        assert_eq!(max.next(), max, "必须是饱和，不能回绕到 0");
    }

    #[test]
    fn seq_orders_monotonically() {
        assert!(Seq(2) > Seq(1));
        assert!(Seq::ZERO < Seq(1));
    }
}
