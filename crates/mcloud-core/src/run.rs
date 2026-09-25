//! 运行记录与任务结果。

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOutcome {
    Success { summary: String },
    Skipped { reason: String },
    Failed { reason: String },
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

    pub fn is_success(&self) -> bool {
        matches!(self, TaskOutcome::Success { .. })
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, TaskOutcome::Failed { .. })
    }

    /// 一句话摘要，用于列表展示与推送。
    pub fn summary(&self) -> &str {
        match self {
            TaskOutcome::Success { summary } => summary,
            TaskOutcome::Skipped { reason } => reason,
            TaskOutcome::Failed { reason } => reason,
        }
    }
}

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
        };
        assert!(!base.needs_attention());
        let failed = RunRecord {
            outcome: TaskOutcome::failed("boom"),
            ..base
        };
        assert!(failed.needs_attention());
    }
}
