//! 任务编排（pipeline）模型。
//!
//! 一个 pipeline 是**有序的步骤列表**，步骤之间共享一组账号。
//! 刻意不做"步骤间传值/依赖图"：那会把编排器变成一个需要调试的语言。
//! 步骤之间唯一的耦合是**失败语义**（继续或中止）。

use serde::{Deserialize, Serialize};

use crate::schedule::ScheduleId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PipelineId(pub String);

impl PipelineId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PipelineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 多账号的执行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountMode {
    /// 逐个账号跑完整个 pipeline。默认 —— 上游常有限流，
    /// 并发打过去容易触发风控，而串行的代价用户可以接受。
    #[default]
    Sequential,
    /// 所有账号并行。
    Parallel,
}

/// 一个步骤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStep {
    /// 任务名。
    pub task: String,
    /// 该步骤的参数。
    #[serde(default)]
    pub body: serde_json::Value,
    /// 该步骤失败时是否继续后续步骤。
    ///
    /// 默认 `false`：**多数步骤是有依赖的**（先签到才能领云朵），
    /// 默认继续会让后面的步骤在错误状态下空跑，产出误导性的"成功"。
    #[serde(default)]
    pub continue_on_error: bool,
}

/// 一个编排。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pipeline {
    pub id: PipelineId,
    pub name: String,
    /// 目标账号。为空表示"所有已启用账号"。
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub account_mode: AccountMode,
    /// 步骤失败时是否中止整个 pipeline。
    ///
    /// 与步骤自己的 `continue_on_error` 是**两层**：
    /// 步骤级说"这一步失败不算数"，编排级说"整个流程还要不要走"。
    /// 两层都有才够用 —— 只有一层时，要么无法跳过非关键步骤，
    /// 要么无法在关键步骤失败时止损。
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
    pub steps: Vec<PipelineStep>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 上次执行时间（毫秒）。
    #[serde(default)]
    pub last_run_ms: Option<i64>,
}

fn default_true() -> bool {
    true
}

impl Pipeline {
    /// 校验：步骤非空、任务名非空。
    ///
    /// 空 steps 的 pipeline 是"点了运行什么也没发生"，
    /// 必须创建时就拒绝，而不是运行时静默成功。
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("编排至少要有一个步骤".into());
        }
        for (i, s) in self.steps.iter().enumerate() {
            if s.task.trim().is_empty() {
                return Err(format!("第 {} 步没有指定任务名", i + 1));
            }
        }
        Ok(())
    }

    /// 某个步骤失败后是否应中止整个编排。
    ///
    /// 判定顺序：步骤说"继续"就继续（它比编排级更具体）；
    /// 否则看编排级的 `stop_on_error`。
    pub fn should_abort_after(&self, step: &PipelineStep, step_failed: bool) -> bool {
        if !step_failed {
            return false;
        }
        if step.continue_on_error {
            return false;
        }
        self.stop_on_error
    }
}

/// 编排与定时的绑定（可选）。
///
/// 单独一张表而不是把 `schedule` 塞进 `Pipeline`：一个编排可以被多条
/// 定时引用（早班/晚班），把定时写进编排就锁死成一对一了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSchedule {
    pub pipeline: PipelineId,
    pub schedule: ScheduleId,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(task: &str, cont: bool) -> PipelineStep {
        PipelineStep {
            task: task.into(),
            body: serde_json::Value::Null,
            continue_on_error: cont,
        }
    }

    fn pipeline(steps: Vec<PipelineStep>, stop_on_error: bool) -> Pipeline {
        Pipeline {
            id: PipelineId::new("p1"),
            name: "测试".into(),
            accounts: vec![],
            account_mode: AccountMode::default(),
            stop_on_error,
            steps,
            enabled: true,
            last_run_ms: None,
        }
    }

    #[test]
    fn empty_steps_is_rejected() {
        let p = pipeline(vec![], true);
        assert!(p.validate().is_err(), "空编排必须被拒绝");
    }

    #[test]
    fn blank_task_name_is_rejected() {
        let p = pipeline(vec![step("  ", false)], true);
        let e = p.validate().unwrap_err();
        assert!(e.contains("第 1 步"), "错误应指出是第几步：{e}");
    }

    #[test]
    fn valid_pipeline_passes() {
        let p = pipeline(vec![step("sign", false), step("receive", false)], true);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn step_continue_overrides_pipeline_stop() {
        let p = pipeline(vec![step("optional", true)], true);
        // 步骤自己说继续 ⇒ 即使编排级 stop_on_error=true 也不中止
        assert!(!p.should_abort_after(&step("optional", true), true));
    }

    #[test]
    fn pipeline_stop_applies_when_step_does_not_continue() {
        let p = pipeline(vec![step("critical", false)], true);
        assert!(p.should_abort_after(&step("critical", false), true));
    }

    #[test]
    fn success_never_aborts() {
        let p = pipeline(vec![step("critical", false)], true);
        assert!(!p.should_abort_after(&step("critical", false), false));
    }

    #[test]
    fn lenient_pipeline_continues_on_any_failure() {
        let p = pipeline(vec![step("a", false)], false);
        assert!(
            !p.should_abort_after(&step("a", false), true),
            "stop_on_error=false 时任何失败都不中止"
        );
    }

    #[test]
    fn account_mode_defaults_to_sequential() {
        assert_eq!(AccountMode::default(), AccountMode::Sequential);
    }
}
