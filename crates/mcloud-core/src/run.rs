//! 运行记录与任务结果。

use serde::{Deserialize, Serialize};

// 日志行的级别来自事件那一层（同一套 `LogLevel` ✓，别在两边各定义一个 ✗）。
use crate::event::LogLevel;

/// 运行标识。进程内单调递增，重启后从历史最大値 +1 继续。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RunId(pub u64);

impl RunId {
    pub fn new(n: u64) -> Self {
        Self(n)
    }
    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 运行的**生命周期**状态。
///
/// 与 [`TaskOutcome`] 是两回事，**刻意分开**：
///
/// - `RunStatus` 说"这次运行结束了没有"（存储层事实）
/// - `TaskOutcome` 说"业务上成没成"（业务结论）
///
/// 混成一个字段会出事：`done`（已结束）里可能包含"部分成功""需人工处理"，
/// 拿它去染红绿就是在替后端编一个它没给过的结论。
/// 参考实现里把这条记为踩过的坑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// 正在运行。
    Running,
    /// 正常结束（业务结果看 `outcome`）。
    Done,
    /// 被主动中止。
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, RunStatus::Running)
    }

    /// 中文标签。**只是文字**，不表示颜色语义。
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Running => "进行中",
            RunStatus::Done => "已结束",
            RunStatus::Cancelled => "已中止",
        }
    }
}

/// 任务结果。
///
/// 刻意区分 `Skipped`：活动未开启或条件不满足**不是失败**，
/// 不应触发告警，也不应计入错误率。
///
/// 也刻意区分 `Partial`：一次运行里常常有**几笔子操作**（例如"领取 AI 豆"
/// 有三笔可领），成功的与失败的可能同时存在 ✓。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOutcome {
    Success {
        summary: String,
    },
    Skipped {
        reason: String,
    },
    /// **部分成功**：有成功的、也有失败的。
    ///
    /// # 为什么非要单开一档（2026-10-03 用户实测口径）
    ///
    /// 原话：「下面的 614 失败还直接说成功」✗ —— `beans` 那条领取本来就把
    /// 「至少有一笔成功、也有失败」注释成**部分成功** ✓，但枚举里没有这一档，
    /// 于是它落到 `Success` ⇒ 界面写「成功」、正文写「失败 1 笔」✗，自相矛盾。
    ///
    /// 两个偷懒的替代都不行 ✗：
    /// - 都算 `Success`：把失败盖住（正是用户踩到的那个）；
    /// - 都算 `Failed`：把"其实领到了几笔"说成全败 ✗，还会让错误率与告警虚高。
    ///
    /// ⚠ 语义上它**不是成功**也**不是失败**：`is_success()` 与 `is_failure()`
    /// 都为 `false` ✓（谁要靠它分流，得显式认 `is_partial()`）。
    Partial {
        summary: String,
    },
    Failed {
        reason: String,
    },
}

impl TaskOutcome {
    pub fn success(summary: impl Into<String>) -> Self {
        TaskOutcome::Success {
            summary: summary.into(),
        }
    }
    pub fn skipped(reason: impl Into<String>) -> Self {
        TaskOutcome::Skipped {
            reason: reason.into(),
        }
    }
    pub fn failed(reason: impl Into<String>) -> Self {
        TaskOutcome::Failed {
            reason: reason.into(),
        }
    }

    /// 部分成功：有成的、也有没成的（两边的账都要在小结里说清 ✓）。
    pub fn partial(summary: impl Into<String>) -> Self {
        TaskOutcome::Partial {
            summary: summary.into(),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, TaskOutcome::Success { .. })
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, TaskOutcome::Failed { .. })
    }

    /// 部分成功。⚠ 它与上面两个**都为假**的关系是刻意的 ✓（见变体的说明）。
    pub fn is_partial(&self) -> bool {
        matches!(self, TaskOutcome::Partial { .. })
    }

    /// 脱敏后的副本。
    ///
    /// 每个变体的文本都来自**任务自己**（`summary`/`reason`），而失败原因里
    /// 最典型的就是"上游回显 + 原因" —— 上游把请求体或头原样吐回来时，
    /// 那一串里就带着凭证 ✓。它同时进两条路：`RunEvent::Finished`（SSE / 推送）
    /// 与 `RunRecord.outcome`（落盘）⇒ 两处都得过闸 ✓。
    pub fn scrubbed(&self) -> Self {
        match self {
            TaskOutcome::Success { summary } => TaskOutcome::Success {
                summary: crate::event::redact(summary),
            },
            TaskOutcome::Skipped { reason } => TaskOutcome::Skipped {
                reason: crate::event::redact(reason),
            },
            TaskOutcome::Partial { summary } => TaskOutcome::Partial {
                summary: crate::event::redact(summary),
            },
            TaskOutcome::Failed { reason } => TaskOutcome::Failed {
                reason: crate::event::redact(reason),
            },
        }
    }

    /// 一句话摘要，用于列表展示与推送。
    pub fn summary(&self) -> &str {
        match self {
            TaskOutcome::Success { summary } => summary,
            TaskOutcome::Skipped { reason } => reason,
            TaskOutcome::Partial { summary } => summary,
            TaskOutcome::Failed { reason } => reason,
        }
    }
}

/// 运行日志的**一行**（回看用）。
///
/// # 为什么要有它（2026-10-03 用户口径）
///
/// 任务的 `ctx.info/warn/progress` 以前只作为 SSE 事件**实时播出去** ⇒
/// 页面一刷新、或者运行一结束，就再也看不到那次运行到底做了什么 ✗。
/// 用户原话：「运行结束后无法查看详细日志（任务记录不算），需要补上；
/// 参考 mcloud 的日志分类和颜色，一眼看出」✓ —— 所以把行攒下来、
/// 跟记录一起落盘 ✓，界面按 `level` 着色 ✓（与实时面板同一套配色 ✓）。
///
/// ⚠ 文本在**入队前**已经脱敏（引擎那层的 sink 负责 ✓）。这里不再脱一遍，
/// 但也**不许**有人绕过 sink 往里塞原始文本 ✗。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLogLine {
    pub level: LogLevel,
    /// 分类（界面按它上徽章与颜色 ✓）。老记录没有这一格 ⇒ `Info` ✓。
    #[serde(default)]
    pub kind: crate::event::LogKind,
    pub message: String,
    pub at_ms: i64,
}

/// 每个运行最多留多少行日志。
///
/// 落盘是**整库 JSON 重写**（`JsonStore`）⇒ 不封顶的话，一次刷屏式运行就会把
/// 数据文件撑大 ✗。200 行 ≈ 十几 KB，足够看清一次运行做了什么 ✓。
///
/// ⚠ 与前端实时面板的 `MAX_LOG_LINES = 500` **不是同一个数**，也别合并：
/// 那个只占内存、随页面一起丢 ✓；这个是写盘的、要留很久 ✓。
pub const RUN_LOG_LIMIT: usize = 200;

/// 一次运行的完整记录，落盘到 `history.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: RunId,
    pub task: String,
    pub account: String,
    pub started_at_ms: i64,
    pub duration_ms: u64,
    pub outcome: TaskOutcome,
    /// 生命周期状态。与 `outcome` 分工见 [`RunStatus`]。
    #[serde(default = "default_done")]
    pub status: RunStatus,
    /// 结构化明细，前端可直接渲染。
    #[serde(default)]
    pub details: serde_json::Value,
    /// 这次运行的日志行（最多 [`RUN_LOG_LIMIT`] 行 ✓）。
    ///
    /// `default` + `skip_serializing_if` 两个属性都是给**老数据**留的：
    /// 加这一格之前落盘的记录没有它 ⇒ 读回来是空表 ✓，写回去也不会凭空长出
    /// `"logs": []` ✗。
    ///
    /// ⚠ 列表接口（`/api/runs`）会把它**剥掉**再回 ✗ → 见
    /// `routes/runs.rs::list`：100 条记录各带 200 行，那个响应就不是
    /// 列表该有的体量了 ✗。要日志就按 id 取详情 ✓。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<RunLogLine>,
}

fn default_done() -> RunStatus {
    // 老数据没有这个字段：它们都是跑完才落盘的，一律按"已结束"回填
    RunStatus::Done
}

impl RunRecord {
    /// 是否应当在推送中被突出显示。
    ///
    /// 中止**不算**需要关注：那是用户自己点的，报红等于制造噪音。
    pub fn needs_attention(&self) -> bool {
        self.status != RunStatus::Cancelled && self.outcome.is_failure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_is_not_failure() {
        let o = TaskOutcome::skipped("活动未开启");
        assert!(!o.is_failure());
        assert!(!o.is_success());
        assert_eq!(o.summary(), "活动未开启");
    }

    #[test]
    fn outcome_json_tag_is_stable() {
        let j = serde_json::to_string(&TaskOutcome::success("ok")).unwrap();
        assert_eq!(j, r#"{"status":"success","summary":"ok"}"#);
    }

    #[test]
    fn run_status_is_separate_from_outcome() {
        // 生命周期与业务结论是两回事：中止的运行也可以有 failed 之外的 outcome
        assert!(RunStatus::Done.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
        assert_eq!(RunStatus::Running.label(), "进行中");
    }

    #[test]
    fn old_records_without_status_default_to_done() {
        let j = r#"{"run_id":1,"task":"sign","account":"138","started_at_ms":0,"duration_ms":1,"outcome":{"status":"success","summary":"ok"},"details":null}"#;
        let r: RunRecord = serde_json::from_str(j).expect("缺 status 的老记录应能解析");
        assert_eq!(r.status, RunStatus::Done);
        // ⚠ 同一份老记录也**没有 `logs`**（那一格是 2026-10-04 才加的 ✓）：
        // 读回来必须是空表 ✓，而且写回去**不能凭空长出** `"logs":[]`
        // —— 历史库里存着成百上千条老记录，凭空长字段就是白白膨胀 ✗。
        assert!(r.logs.is_empty(), "老记录没有 logs ⇒ 空表");
        let back = serde_json::to_string(&r).expect("应能序列化");
        assert!(!back.contains("\"logs\""), "空日志不该被写进 JSON：{back}");
    }

    /// 日志行的形状：级别 + 文本 + 时刻（界面按级别着色 ✓）。
    #[test]
    fn log_lines_round_trip() {
        let line = RunLogLine {
            level: LogLevel::Warn,
            // 分类与级别**不是一回事**：`level` 管配色档位、`kind` 管徽章语义 ✓
            //（`ctx.fail()` 就是 kind=Fail + level=Warn ✓）。这一条是普通 warn ⇒ 同值 ✓。
            kind: crate::event::LogKind::Warn,
            message: "领取AI豆：上游错误 [614]".into(),
            at_ms: 1_700_000_000_123,
        };
        let j = serde_json::to_string(&line).expect("应能序列化");
        // ⚠ 字段顺序就是**结构体里的声明顺序**（level → kind → message → at_ms ✓）——
        // 这一条是逐字比对，`kind` 后来才加进结构体 ⇒ 期望串要跟着带上它 ✗。
        assert_eq!(
            j,
            r#"{"level":"warn","kind":"warn","message":"领取AI豆：上游错误 [614]","at_ms":1700000000123}"#
        );
        let back: RunLogLine = serde_json::from_str(&j).expect("应能解析");
        assert_eq!(back, line);
    }

    #[test]
    fn cancelled_run_does_not_need_attention() {
        let r = RunRecord {
            run_id: RunId::new(1),
            task: "sign".into(),
            account: "138".into(),
            started_at_ms: 0,
            duration_ms: 1,
            outcome: TaskOutcome::failed("被中止"),
            status: RunStatus::Cancelled,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        };
        assert!(
            !r.needs_attention(),
            "用户自己点的中止不该报红 —— 那只是噪音"
        );
    }

    #[test]
    fn only_failures_need_attention() {
        let base = RunRecord {
            run_id: RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            started_at_ms: 0,
            duration_ms: 1,
            outcome: TaskOutcome::success("ok"),
            status: RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        };
        assert!(!base.needs_attention());
        let failed = RunRecord {
            outcome: TaskOutcome::failed("boom"),
            ..base
        };
        assert!(failed.needs_attention());
    }
}
