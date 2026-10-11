//! 运行日志模型。
//!
//! 日志**按 run 归档**而不是一条全局流：前端要"看这次运行的日志"，
//! 全局流还得再按 run_id 过滤一遍；而断线重连时按 run 取也更直接。
//!
//! 保留策略见 [`LOG_LIMIT_PER_RUN`]：一次运行的日志条数有上限。
//! 不设上限的话，一个死循环打日志的任务能把磁盘写满 ——
//! 而这属于"任务写错了"，不该让整个服务跟着倒下。
//!
//! ⚠ 2026-10-09 起那个上限**是可配的**（`AppSettings::log_limit_per_run` ✓，
//! 见 `mcloud_core::settings`），[`LOG_LIMIT_PER_RUN`] 只是**默认值** ✓；
//! 文件总量另有 `AppSettings::log_total_mb` 管 ✓。
//!
//! 读取走**分页**（[`LogPageQuery`] / [`slice_page`] ✓）：
//! 一次运行上万行时，界面要的是"最近这一屏"而不是"全部" ✓。

use serde::{Deserialize, Serialize};

use crate::event::{LogKind, LogLevel, RunEvent};

/// 单次运行保留的日志条数上限的**默认值**（真正生效的是设置里那一格 ✓）。
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
    /// 分类（界面按它上徽章与颜色 ✓）。
    ///
    /// ⚠ 2026-10-09 才加的这一格。以前只有 `RunRecord.logs` 那份**内联**日志
    /// 带分类，而**落盘**那份（`data/logs/<id>.json`）只有级别 ⇒ 回看时
    /// 「领到了 6 颗」与「这一笔没领到」长得一样 ✗（正是用户 2026-10-04
    /// 「info warn error success 等等标记都没有，更别说颜色」要治的那件事 ✓）。
    /// 两条路合一之后，分类就存在这里 ✓。
    ///
    /// `default` 是给**老文件**留的：加这一格之前落盘的行没有它 ⇒ 读回来是
    /// `Info` ✓（与 `RunLogLine::kind` 同一条口径 ✓）。
    #[serde(default)]
    pub kind: LogKind,
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
    /// 造一行（分类默认 [`LogKind::Info`] ✓ —— 要别的用 [`LogLine::with_kind`] ✓）。
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
            kind: LogKind::default(),
            message: message.into(),
            at_ms,
            seq,
        }
    }

    /// 补上分类（链式，读起来就是"这一行是什么" ✓）。
    pub fn with_kind(mut self, kind: LogKind) -> Self {
        self.kind = kind;
        self
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

/// 把一个运行事件变成**要落盘的日志行**（`None` = 这个事件不进日志 ✓）。
///
/// # 为什么要单独一个函数
///
/// "什么进日志、进去长什么样"这条判据有**两个**调用点：
/// * `mcloud_app::EventHub` 攒批落盘时（**写**那一路 ✓）；
/// * `GET /api/runs/{id}/logs` 从内存事件补最新那几行时（**读**那一路 ✓）。
///
/// 两边各写一份必然分叉 —— 症状是"刚跑完时看到的行"与"重启后看到的行"
/// 不一样 ✗（进度行在一边有、在另一边没有 ✓）。所以判据只有这一份 ✓。
///
/// 进度行写成 `[2/3] 领取AI豆` —— 与实时面板**同一份**格式 ✓（回看时"做到第几步"
/// 与"结果如何"同等重要 ✓）。
pub fn log_line_of(event: &RunEvent, seq: Seq) -> Option<LogLine> {
    match event {
        RunEvent::Log {
            run_id,
            level,
            kind,
            message,
            at_ms,
        } => {
            Some(LogLine::new(run_id.get(), *level, message.clone(), *at_ms, seq).with_kind(*kind))
        }
        // ⚠ 进度行**也要进日志**（2026-10-09 ✓）。以前它只进引擎那份内联日志
        // （`RunRecord.logs`），而落盘这一路只认 `RunEvent::Log` ⇒ 两条路合一之后
        // 不补这一条，回看里就再也看不到"做到第几步" ✗。
        RunEvent::Progress {
            run_id,
            step,
            total,
            label,
            at_ms,
        } => Some(
            LogLine::new(
                run_id.get(),
                LogLevel::Info,
                format!("[{step}/{total}] {label}"),
                *at_ms,
                seq,
            )
            .with_kind(LogKind::Start),
        ),
        // `Started` / `Finished` / `Details` 不进日志：前两个是生命周期 ✓，
        // 最后一个进 `details`（前端直接渲染的那一格 ✓），各归各的 ✓。
        _ => None,
    }
}

/// 取日志的**一页**：游标 + 条数上限。
///
/// # 为什么要有它（用户口径 2026-10-09）
///
/// 「大日志做虚拟加载或者分页，避免一次加载太多」✓ —— 一次运行的日志可以有
/// 上万行（上限由设置决定 ✓），一次性全给前端既慢又白给（用户一屏只看十几行 ✓）。
///
/// ⚠ 两个游标**语义不同、方向相反**，别合并：
/// * [`LogPageQuery::after`]：**往下**接（断线续传 —— "我已经看到 n 了，接着给我" ✓），
///   取的是 `seq > after` 里**最早**的那些 ✓；
/// * [`LogPageQuery::before`]：**往上**翻（"我要看更早的" ✓），
///   取的是 `seq < before` 里**最晚**的那些 ✓。
///
/// 两个都给时按 `after` 走（续传优先）—— 调用方本来就不该同时给 ✓。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogPageQuery {
    /// 只要 `seq > after` 的（"已经看到 n" ✓，见 [`Seq::seen`]）。
    pub after: Option<Seq>,
    /// 只要 `seq < before` 的（往上翻页 ✓）。
    pub before: Option<Seq>,
    /// 最多回几条。`0` 按 1 处理（"给我零条"没有调用方会要 ✓）。
    pub limit: usize,
}

impl LogPageQuery {
    pub fn new(limit: usize) -> Self {
        Self {
            after: None,
            before: None,
            limit,
        }
    }
}

/// 一页日志（+ 这一路**一共**有多少行 ✓）。
///
/// `total` 是"这个 run 在盘上总共留了多少行"—— 界面要显示"已显示 500 / 共 1730 行"
/// 就得靠它，而不是拿返回条数猜 ✓。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    /// 这个 run 现有多少行（**不受**游标与 `limit` 影响 ✓）。
    pub total: usize,
}

/// 日志目录的用量读数（给「存储信息」卡与容量上限用 ✓）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStats {
    /// 目录里有几个日志文件（不含 `.bak` ✓ —— 那是覆盖前的副本，不是"日志" ✓）。
    pub files: usize,
    /// 目录合计字节（**含** `.bak`：它照样占磁盘 ✓）。
    pub bytes: u64,
    /// 当前生效的总量上限（字节，来自设置 ✓）。
    pub max_bytes: u64,
}

/// 按 [`LogPageQuery`] 从**已按 `seq` 升序**的全部行里切一页。
///
/// ⚠ 这是**唯一**一份分页判据：默认实现（`LogStore::logs_page`）与存储层
/// （`JsonStore` 的按需读盘那条路）都调它 ✓ —— 两边各写一份必然分叉
/// （"从内存拿"和"从盘上拿"给出不同的页 ✗）。
///
/// 前置条件：`all` 按 `seq` 严格升序（本仓的写入路径保证这一点 ✓，见
/// `EventHub::publish` 的序号分配 ✓）。
pub fn slice_page(all: &[LogLine], q: &LogPageQuery) -> LogPage {
    let limit = q.limit.max(1);
    let lines: Vec<LogLine> = match q.after {
        // 续传：从游标之后**最早**那条开始，往后给 limit 条 ✓
        Some(after) => all
            .iter()
            .filter(|l| l.seq > after)
            .take(limit)
            .cloned()
            .collect(),
        // 看最近的 / 往上翻页：先定位右端，再往前取 limit 条 ✓
        None => {
            let end = match q.before {
                // 升序 ⇒ 二分找第一条 `seq >= before` 的位置就是右端 ✓
                Some(before) => all.partition_point(|l| l.seq < before),
                None => all.len(),
            };
            let start = end.saturating_sub(limit);
            all[start..end].to_vec()
        }
    };
    LogPage {
        lines,
        total: all.len(),
    }
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

    /// 分类要跟着落盘（回看时按它上徽章与颜色 ✓）＋**老文件**没有这一格时按 `Info` ✓。
    #[test]
    fn log_line_keeps_its_kind_and_tolerates_old_files() {
        let l =
            LogLine::new(7, LogLevel::Info, "领到了 6 颗", 0, Seq(1)).with_kind(LogKind::Success);
        let j = serde_json::to_string(&l).unwrap();
        assert!(j.contains("\"kind\":\"success\""), "{j}");
        assert_eq!(serde_json::from_str::<LogLine>(&j).unwrap(), l);

        // 加这一格之前落盘的行：没有 `kind` 键 ⇒ Info（而不是解析失败 ✗）
        let old: LogLine =
            serde_json::from_str(r#"{"runId":1,"level":"info","message":"老行","atMs":5,"seq":2}"#)
                .unwrap();
        assert_eq!(old.kind, LogKind::Info);
        assert_eq!(old.message, "老行");
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

    /// 造一串 `seq = 1..=n` 的行（分页判据要的就是升序 ✓）。
    fn lines(n: u64) -> Vec<LogLine> {
        (1..=n)
            .map(|i| LogLine::new(1, LogLevel::Info, format!("第 {i} 行"), 0, Seq(i)))
            .collect()
    }

    fn seqs(p: &LogPage) -> Vec<u64> {
        p.lines.iter().map(|l| l.seq.get()).collect()
    }

    /// 不带游标 ⇒ **最后** limit 条（"打开就是最近这些" ✓），且 `total` 是全部行数 ✓。
    #[test]
    fn without_a_cursor_you_get_the_tail() {
        let all = lines(10);
        let p = slice_page(&all, &LogPageQuery::new(3));
        assert_eq!(seqs(&p), vec![8, 9, 10]);
        assert_eq!(p.total, 10, "total 不受 limit 影响");
        // 不足一页时给全部（而不是补空行 ✓）
        assert_eq!(slice_page(&all, &LogPageQuery::new(99)).lines.len(), 10);
        // 0 也当 1 处理：不返回空页（空页在界面上与"没有日志"分不开 ✗）
        assert_eq!(seqs(&slice_page(&all, &LogPageQuery::new(0))), vec![10]);
    }

    /// `before` ⇒ 往上翻：`seq < before` 里**最晚**的 limit 条 ✓。
    #[test]
    fn before_walks_upwards() {
        let all = lines(10);
        let q = LogPageQuery {
            before: Some(Seq(8)),
            ..LogPageQuery::new(3)
        };
        assert_eq!(seqs(&slice_page(&all, &q)), vec![5, 6, 7]);
        // 一直翻到头：给能给的（1、2 ✓），调用方据"少于 limit"判断到底了 ✓
        let q = LogPageQuery {
            before: Some(Seq(3)),
            ..LogPageQuery::new(5)
        };
        assert_eq!(seqs(&slice_page(&all, &q)), vec![1, 2]);
        // 游标比最小那条还小 ⇒ 空页（真到底了 ✓）
        let q = LogPageQuery {
            before: Some(Seq(1)),
            ..LogPageQuery::new(5)
        };
        assert!(slice_page(&all, &q).lines.is_empty());
    }

    /// `after` ⇒ 往下接（断线续传）：`seq > after` 里**最早**的 limit 条 ✓。
    #[test]
    fn after_resumes_forwards() {
        let all = lines(10);
        let q = LogPageQuery {
            after: Some(Seq(7)),
            ..LogPageQuery::new(3)
        };
        assert_eq!(seqs(&slice_page(&all, &q)), vec![8, 9, 10]);
        let q = LogPageQuery {
            after: Some(Seq::ZERO),
            ..LogPageQuery::new(2)
        };
        assert_eq!(
            seqs(&slice_page(&all, &q)),
            vec![1, 2],
            "从头接就是最前面几条"
        );
    }

    /// 两个游标都给 ⇒ 按 `after` 走（续传优先 ✓，见 [`LogPageQuery`] 的说明）。
    #[test]
    fn after_wins_when_both_cursors_are_given() {
        let all = lines(10);
        let q = LogPageQuery {
            after: Some(Seq(2)),
            before: Some(Seq(10)),
            limit: 3,
        };
        assert_eq!(seqs(&slice_page(&all, &q)), vec![3, 4, 5]);
    }
}
