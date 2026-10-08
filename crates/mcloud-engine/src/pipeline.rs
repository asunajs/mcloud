//! 任务编排执行。
//!
//! # 执行顺序：**步骤在外，账号在内**
//!
//! 一个 pipeline 是一串有序步骤，每步对一组账号跑同一个任务。
//! 循环顺序是 `for 步骤 { for 账号 }`，不是反过来。
//!
//! 差别在于"上游看到的请求交错方式"：步骤在外时，签到会先把所有账号跑完
//! 再开始领AI豆；账号在外时，会先把一个账号的所有步骤跑完。
//! 后者更贴近"一个账号一轮做完"的直觉，但对**共享状态**的步骤不友好
//! （例如某步依赖"所有账号都已签到"）。
//! 选步骤在外，是因为它让每一步的产出都是完整的一批，便于记账与展示。
//! 每步内部仍是**逐账号串行** —— 防风控的节奏没有被放宽。
//!
//! # 一次性预检
//!
//! 在发出**任何**真实请求之前，先校验整条链的步骤是否都存在。
//! 否则"第 3 步的任务名拼错了"的表现是：前两步已经把真上游打了一遍
//! （签到是写操作），然后整条链返回一个错误。代价是参数构造两遍，很便宜。
//!
//! # 失败语义是两层
//!
//! - **步骤级** `continue_on_error`：这一步失败算不算数
//! - **编排级** `stop_on_error`：整个流程还要不要走
//!
//! 两层都有才够用：只有一层时，要么无法跳过非关键步骤，
//! 要么无法在关键步骤失败时止损。

use std::sync::Arc;

use mcloud_core::{
    AccountMode, CancelToken, Error, Pipeline, PipelineStep, Result, RunStatus, TaskOutcome,
};

use crate::runner::Runner;

/// 单个步骤在一个账号上的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct StepRun {
    pub step_index: usize,
    pub task: String,
    pub account: String,
    /// 该次运行的 id。可能为 0（被并发闸跳过）。
    pub run_id: u64,
    pub outcome: TaskOutcome,
    /// 是否被中止。
    pub cancelled: bool,
}

impl StepRun {
    fn failed(&self) -> bool {
        self.outcome.is_failure()
    }
    fn blocked(&self) -> bool {
        // 被闸跳过时 run_id 为 0 且 outcome 是 skipped
        self.run_id == 0
    }
}

/// 一个步骤的汇总状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// 全部成功。
    Ok,
    /// 部分成功。
    Partial,
    /// 失败。
    Error,
    /// 该步没跑（被闸挡住，或前序步骤中止了整条链）。
    Skipped,
    /// 被中止。
    Cancelled,
}

/// 单个步骤的汇总。
#[derive(Debug, Clone, PartialEq)]
pub struct StepSummary {
    pub index: usize,
    pub task: String,
    pub status: StepStatus,
    pub runs: Vec<StepRun>,
}

/// 整条链的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineOutcome {
    pub steps: Vec<StepSummary>,
    /// 在第几步中止的。`None` = 跑完了。
    ///
    /// 必须记下来：否则"第 2 步失败、按设定继续"与
    /// "第 2 步失败、后面 8 步没跑"在界面上长得一模一样。
    pub stopped_after: Option<usize>,
    pub cancelled: bool,
}

impl PipelineOutcome {
    /// 整体状态。**单调**：error > partial > ok，后面成功不能把前面的失败翻绿。
    pub fn status(&self) -> StepStatus {
        if self.cancelled {
            return StepStatus::Cancelled;
        }
        let mut worst = StepStatus::Ok;
        for s in &self.steps {
            worst = worse(worst, s.status);
        }
        worst
    }

    /// 一句话摘要，用于推送与列表展示。
    pub fn summary_line(&self) -> String {
        let ran = self
            .steps
            .iter()
            .filter(|s| s.status != StepStatus::Skipped)
            .count();
        let total = self.steps.len();
        let tail = match self.stopped_after {
            Some(i) => format!("，在第 {} 步中止", i + 1),
            None => String::new(),
        };
        format!(
            "{ran}/{total} 步已执行 · {}{tail}",
            match self.status() {
                StepStatus::Ok => "全部成功",
                StepStatus::Partial => "部分成功",
                StepStatus::Error => "有失败",
                StepStatus::Skipped => "未执行",
                StepStatus::Cancelled => "已中止",
            }
        )
    }
}

/// 状态取"更坏"的那个。`Skipped` 不参与把结果拉坏。
fn worse(a: StepStatus, b: StepStatus) -> StepStatus {
    fn rank(s: StepStatus) -> u8 {
        match s {
            StepStatus::Cancelled => 4,
            StepStatus::Error => 3,
            StepStatus::Partial => 2,
            StepStatus::Ok => 1,
            StepStatus::Skipped => 0,
        }
    }
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

/// 编排执行器。
pub struct PipelineRunner {
    runner: Arc<Runner>,
}

impl PipelineRunner {
    pub fn new(runner: Arc<Runner>) -> Self {
        Self { runner }
    }

    /// 预检：整条链的任务名是否都在注册表里。
    ///
    /// **必须在发出任何请求之前调用。** 返回第一个找不到的任务名。
    pub fn preflight(&self, pipeline: &Pipeline) -> Result<()> {
        pipeline.validate().map_err(Error::Config)?;
        for (i, step) in pipeline.steps.iter().enumerate() {
            if !self.runner.has_task(&step.task) {
                return Err(Error::Config(format!(
                    "第 {} 步的任务不存在：{}",
                    i + 1,
                    step.task
                )));
            }
        }
        Ok(())
    }

    /// 执行整条链。
    ///
    /// `cancel` 为协作式取消：每个步骤/账号开始前检查一次。
    /// 已经在跑的请求不被打断 —— 见 `mcloud_core::cancel` 的说明。
    pub async fn run(
        &self,
        pipeline: &Pipeline,
        accounts: &[mcloud_core::Account],
        cancel: Option<CancelToken>,
    ) -> Result<PipelineOutcome> {
        self.preflight(pipeline)?;

        let cancelled = |c: &Option<CancelToken>| c.as_ref().is_some_and(|t| t.is_cancelled());
        let mut summaries: Vec<StepSummary> = Vec::new();
        let mut stopped_after = None;
        let mut was_cancelled = false;

        for (idx, step) in pipeline.steps.iter().enumerate() {
            // 中止后不再开始新步骤，但**仍然把剩余步骤记为 Skipped**，
            // 让结果的形状稳定 —— 前端不需要处理"数组长度会变"的情况
            if cancelled(&cancel) || stopped_after.is_some() {
                // 这里必须也置 was_cancelled：否则"一开始就已取消"的情况下
                // 所有步骤都是 Skipped，整体状态会被判成 Ok ——
                // 那等于把一次中止报成成功。
                if cancelled(&cancel) {
                    was_cancelled = true;
                }
                summaries.push(StepSummary {
                    index: idx,
                    task: step.task.clone(),
                    status: StepStatus::Skipped,
                    runs: Vec::new(),
                });
                continue;
            }

            let mut runs = Vec::new();
            for acc in accounts {
                if cancelled(&cancel) {
                    was_cancelled = true;
                    break;
                }
                // 把链令牌传下去：这样"中止整条链"能传到每一步内部，
                // 而不是只在本层不再发起新步骤
                // ⭐ 发起方带下去（`pipeline:<id>` ✓）：推送规则绑在**编排**上时，
                // 每一步的投递都靠它认领（见 `RunEvent::Started.origin` ✓）。
                let record = self
                    .runner
                    .run_full(
                        &step.task,
                        acc,
                        &step.body,
                        cancel.clone(),
                        Some(format!("pipeline:{}", pipeline.id)),
                    )
                    .await?;
                let blocked = record.run_id.get() == 0;
                runs.push(StepRun {
                    step_index: idx,
                    task: step.task.clone(),
                    account: acc.id.to_string(),
                    run_id: record.run_id.get(),
                    outcome: record.outcome.clone(),
                    cancelled: record.status == RunStatus::Cancelled,
                });
                let _ = blocked;
            }

            let status = summarize_step(&runs, cancelled(&cancel));
            let step_failed = matches!(status, StepStatus::Error | StepStatus::Partial);

            // 两层失败语义：步骤说继续就继续，否则看编排级
            if pipeline.should_abort_after(step, step_failed) {
                stopped_after = Some(idx);
            }
            summaries.push(StepSummary {
                index: idx,
                task: step.task.clone(),
                status,
                runs,
            });
        }

        Ok(PipelineOutcome {
            steps: summaries,
            stopped_after,
            cancelled: was_cancelled,
        })
    }
}

/// 汇总一个步骤的状态。
///
/// 判定顺序刻意如此：
/// 1. 有账号在跑但出错 ⇒ error
/// 2. 全被闸挡住 ⇒ skipped（不是 error，也不是 ok —— 见下）
/// 3. 部分成功 ⇒ partial
/// 4. 其余 ⇒ ok
fn summarize_step(runs: &[StepRun], cancelled: bool) -> StepStatus {
    if cancelled && runs.iter().any(|r| r.cancelled) {
        return StepStatus::Cancelled;
    }
    if runs.is_empty() {
        return StepStatus::Skipped;
    }
    if runs.iter().all(StepRun::blocked) {
        // 全被挡住：既不能报 error（"上一轮还在跑"是常态，判红让人以为清单坏了），
        // 也不能报 ok（那等于把"这一趟什么都没做"藏起来）
        return StepStatus::Skipped;
    }
    let attempted: Vec<&StepRun> = runs.iter().filter(|r| !r.blocked()).collect();
    let failed = attempted.iter().filter(|r| r.failed()).count();
    if failed == 0 {
        StepStatus::Ok
    } else if failed == attempted.len() {
        StepStatus::Error
    } else {
        StepStatus::Partial
    }
}

/// 多账号执行方式。
///
/// 当前实现**只支持串行** —— 上游常有限流，并发打过去容易触发风控。
/// `Parallel` 会明确报错而不是静默按串行跑：静默接受等于在界面上留一个
/// 后端做不到的开关，用户以为并发了、实际没有，只会觉得"怎么这么慢"。
pub fn ensure_supported_account_mode(mode: AccountMode) -> Result<()> {
    match mode {
        AccountMode::Sequential => Ok(()),
        AccountMode::Parallel => Err(Error::Config(
            "当前只支持串行执行（上游限流）；并行模式尚未实现".into(),
        )),
    }
}

/// 便捷：从单个步骤构造一次性编排（供"只跑一步"的场景）。
pub fn single_step_pipeline(id: impl Into<String>, step: PipelineStep) -> Pipeline {
    Pipeline {
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: mcloud_core::PipelineId::new(id),
        name: "单步".into(),
        accounts: Vec::new(),
        account_mode: AccountMode::Sequential,
        stop_on_error: true,
        steps: vec![step],
        enabled: true,
        owner: None,
        last_run_ms: None,
    }
}

#[cfg(test)]
pub(crate) mod tests;
