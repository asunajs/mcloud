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
    /// 结构化明细，前端可直接渲染。
    #[serde(default)]
    pub details: serde_json::Value,
}

impl RunRecord {
    /// 是否应当在推送中被突出显示。
    pub fn needs_attention(&self) -> bool {
        self.outcome.is_failure()
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
    fn only_failures_need_attention() {
        let base = RunRecord {
            run_id: RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            started_at_ms: 0,
            duration_ms: 1,
            outcome: TaskOutcome::success("ok"),
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
