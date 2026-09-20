//! 运行生命周期状态机。
//!
//! # 为什么需要一个显式状态机
//!
//! 旧项目里"这次运行处于什么状态"是**由调用方各自维护**的：
//! `runner.ts` 往 Map 里塞一个 promise、能力自己往 `state.json` 里写字段、
//! WebUI 又从历史里推断。于是出现了三类典型的错：
//!
//! 1. **假成功**：能力内部 `logger.info("需手动完成: …")` 之后 `return true`，
//!    外层据此写 `status = "success"`。运行记录上看起来一切正常
//!    （`docs/business-feature-inventory.md` §1.4）。
//! 2. **终态被覆盖**：跑完写了 `Done`，随后某条清理路径又写回 `Running`，
//!    于是前端一直转圈。
//! 3. **终态没有结果**：`Done` 但没有任何成果字段，前端只能显示"成功"两个字。
//!
//! 这三类错的共同点是**没有一个地方规定"什么状态才算跑完、跑完必须带什么"**。
//! 这个模块就是那个地方。
//!
//! # 与 `RunOutcome` 的关系
//!
//! [`RunStage`]（本模块）是**存储生命周期**，只有三个阶段：
//! `Running` → `{Done, Failed}`，用来回答"这一行还在跑吗"（前端转不转圈）。
//!
//! [`RunOutcome`] 是**业务终局判定**，四态。两者不是一对一：
//! `Done` 可以对应 `Success` / `PartialSuccess` / `ManualRequired` 三种。
//!
//! ```text
//! Running ──finish(outcome)──┬── Done   （outcome ∈ {Success, PartialSuccess, ManualRequired}）
//!                            └── Failed （outcome == Failed）
//! ```
//!
//! 注意 `Failed` 这个阶段对应**唯一的** `RunOutcome::Failed`，
//! 而 `Done` 对应其余三态 —— 也就是说 **`Done` 不等于成功**。
//! 这正是旧项目栽的地方：把 `Done` 读成"成功"，于是"需人工"被吞掉。

use serde::{Deserialize, Serialize};

use crate::run::{ManualStep, RunId, RunOutcome};

/// 运行的存储生命周期。
///
/// 只有三个阶段，且**终态不可再迁出** —— 见 [`RunStage::is_terminal`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum RunStage {
    /// 正在跑。
    Running,
    /// 跑完了（**不是"成功"**，成败看 `outcome`）。
    Done {
        outcome: RunOutcome,
        /// 需人工的待办。`outcome == ManualRequired` 时**必须非空**（见下）。
        manual_steps: Vec<ManualStep>,
        summary: Option<String>,
    },
    /// 挂了。执行器自己的失败，或能力的 `RunOutcome::Failed`。
    Failed {
        /// 面向使用者的说明（**不含**内部细节，见 `Error::public_message`）。
        summary: String,
        /// 需人工的待办 —— 失败也可能伴随"你得去 App 里做点什么"。
        manual_steps: Vec<ManualStep>,
    },
}

impl RunStage {
    pub const fn running() -> Self {
        Self::Running
    }

    /// 是不是终态。
    ///
    /// 执行器用它守住"终态之后不再迁移"这条不变式。
    pub const fn is_terminal(&self) -> bool {
        !matches!(self, Self::Running)
    }

    /// 终局的业务判定。还在跑时返回 `None`。
    pub const fn outcome(&self) -> Option<RunOutcome> {
        match self {
            Self::Running => None,
            Self::Done { outcome, .. } => Some(*outcome),
            Self::Failed { .. } => Some(RunOutcome::Failed),
        }
    }

    /// 映射到存储用的三态（`runs.status` 列）。
    ///
    /// # 为什么要有这个映射
    ///
    /// `runs.status` 是既有的列，只有三个取值，而它同时被前端
    /// （"在跑吗"）与历史列表复用。改列的取值范围会动到已落库的数据与前端契约，
    /// 所以在**存储层**维持三态、在**业务层**用四态。
    /// 两者之间的翻译只此一处，避免各处自己 `match`（那样必然会有人写出
    /// `Done => "success"` 从而丢掉 `ManualRequired`）。
    pub const fn to_legacy_status(&self) -> crate::model::RunStatus {
        match self {
            Self::Running => crate::model::RunStatus::Running,
            Self::Done { .. } => crate::model::RunStatus::Done,
            Self::Failed { .. } => crate::model::RunStatus::Failed,
        }
    }

    /// 待办列表（两种终态都可能有）。
    pub fn manual_steps(&self) -> &[ManualStep] {
        match self {
            Self::Running => &[],
            Self::Done { manual_steps, .. } | Self::Failed { manual_steps, .. } => manual_steps,
        }
    }

    /// 结算是成功还是失败 —— 按**阶段**而非 `outcome` 的口径。
    ///
    /// 注意这里与 [`RunOutcome::is_success_like`] **不同**：
    /// `ManualRequired` 在这个口径下算"没失败"（阶段是 `Done`），
    /// 但在 `RunOutcome` 口径下**不算成功**。
    ///
    /// 两个口径都需要、且不能混用：
    /// - 前端"红不红"用这个（`ManualRequired` 不该显示成报错）；
    /// - "这一趟目标达成了吗"用 `RunOutcome::is_success_like`。
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// 生命周期迁移的错误。
///
/// 每次都**显式报错**而不是静默忽略：
/// 静默忽略会让"重复结算"看起来像正常操作，
/// 而重复结算往往意味着同一个 runId 被执行了两次（幂等闸失效）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error("运行已在终态（{current}），不能再迁移到 {attempted}")]
    AlreadyTerminal {
        current: &'static str,
        attempted: &'static str,
    },

    #[error("需人工（ManualRequired）必须带至少一条待办说明，否则与旧项目那句没人看的日志等价")]
    ManualRequiredWithoutSteps,

    #[error("失败必须带说明，空说明等于把错误吞掉")]
    FailedWithoutSummary,

    #[error("runId 不匹配：期望 {expected}，实得 {actual}")]
    RunIdMismatch { expected: RunId, actual: RunId },
}

/// 一次运行的可变状态。
///
/// # 用法
///
/// ```ignore
/// let mut exec = RunExecution::start(run_id, "sign");
/// exec.record(&RunEvent::progress(run_id.clone(), 1, 3))?;
/// let stage = exec.finish(RunReport::success())?;
/// assert!(stage.is_terminal());
/// ```
#[derive(Debug, Clone)]
pub struct RunExecution {
    run_id: RunId,
    capability: String,
    stage: RunStage,
    /// 已发事件数 —— 用来守住"终局之后不再发布"。
    emitted_terminal: bool,
}

impl RunExecution {
    pub fn start(run_id: RunId, capability: impl Into<String>) -> Self {
        Self {
            run_id,
            capability: capability.into(),
            stage: RunStage::Running,
            emitted_terminal: false,
        }
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn capability(&self) -> &str {
        &self.capability
    }

    pub const fn stage(&self) -> &RunStage {
        &self.stage
    }

    pub const fn outcome(&self) -> Option<RunOutcome> {
        self.stage.outcome()
    }

    pub const fn is_terminal(&self) -> bool {
        self.stage.is_terminal()
    }

    /// 是否已经发布过终局事件。
    pub const fn has_emitted_terminal(&self) -> bool {
        self.emitted_terminal
    }

    /// 记一次事件发布 —— 调用方在**真正**把事件推进总线之前调它。
    ///
    /// 终局之后再记会被拒：这是"终局事件至多一次、且必须是最后一条"的守门人。
    pub fn record(&mut self, event: &crate::run::RunEvent) -> Result<(), StageError> {
        if event.run_id() != &self.run_id {
            return Err(StageError::RunIdMismatch {
                expected: self.run_id.clone(),
                actual: event.run_id().clone(),
            });
        }
        if self.emitted_terminal {
            return Err(StageError::AlreadyTerminal {
                current: "terminal-emitted",
                attempted: "record",
            });
        }
        if event.is_terminal() {
            self.emitted_terminal = true;
        }
        Ok(())
    }

    /// 正常结算。
    ///
    /// 两条硬校验，都是为了让"旧项目那种失真"无法表达：
    /// - `ManualRequired` **必须**带待办（空手说需人工 = 那句没人看的日志）；
    /// - `Failed` **必须**带说明（空说明 = 把错误吞掉）。
    ///
    /// # 第一条校验什么时候才真的会触发
    ///
    /// [`crate::run::RunReport::manual_required`] 在构造函数里就把"空待办"降级成了
    /// `PartialSuccess`，所以走构造函数**到不了**这里。能到这里的只有
    /// **用字面量直接构造** `RunReport` 的调用方（字段是 `pub`）。
    ///
    /// 那为什么不干脆把这条删掉？因为 `normalize` 只做**升级**
    /// （`Success` + 有待办 → `ManualRequired`），**不做降级** ——
    /// 手写的 `ManualRequired` + 空待办会一路穿到这里。这一层是那唯一一条路的守卫，
    /// 删了就等于"只要不用构造函数，旧项目那句没人看的日志就能进库"。
    /// 见 `hand_built_manual_required_without_steps_is_rejected`。
    ///
    /// 返回结算后的阶段**快照**（而不是 `&self` 的借用）：
    /// 借用会在调用方留下一个活着的 `&self`，于是紧接着的
    /// `exec.is_terminal()` / `exec.outcome()` 会当成可变借用冲突而编译不过 ——
    /// 而那两句恰恰是结算后最自然的下一步。`RunStage` 很小（`Vec` + `Option<String>`），
    /// `clone` 一份比让调用方绕着借用走更划算。
    pub fn finish(&mut self, report: crate::run::RunReport) -> Result<RunStage, StageError> {
        if self.stage.is_terminal() {
            return Err(StageError::AlreadyTerminal {
                current: self.stage_name(),
                attempted: "finish",
            });
        }

        // 复核不变式：字段是 `pub` 的，`RunReport` 可以被字面量绕过构造函数
        let report = report.normalize();

        match report.outcome {
            // 空待办说"需人工" = 旧项目那句没人看的日志。
            // 只在**字面量绕过构造函数**时可达（构造函数会先降级），见本方法的文档。
            RunOutcome::ManualRequired if report.manual_steps.is_empty() => {
                Err(StageError::ManualRequiredWithoutSteps)
            }
            RunOutcome::Failed => {
                let summary = report.summary.unwrap_or_default();
                if summary.trim().is_empty() {
                    return Err(StageError::FailedWithoutSummary);
                }
                let stage = RunStage::Failed {
                    summary,
                    manual_steps: report.manual_steps,
                };
                self.stage = stage.clone();
                Ok(stage)
            }
            outcome => {
                let stage = RunStage::Done {
                    outcome,
                    manual_steps: report.manual_steps,
                    summary: report.summary,
                };
                self.stage = stage.clone();
                Ok(stage)
            }
        }
    }

    /// 执行器自身失败（panic、内部错误、断线）。
    ///
    /// 与 [`Self::finish`] 的区别：这条**不需要**能力给出业务判定，
    /// 因为能力可能根本没跑到能判定的地方。
    pub fn fail(&mut self, summary: impl Into<String>) -> Result<RunStage, StageError> {
        if self.stage.is_terminal() {
            return Err(StageError::AlreadyTerminal {
                current: self.stage_name(),
                attempted: "fail",
            });
        }
        let summary = summary.into();
        if summary.trim().is_empty() {
            return Err(StageError::FailedWithoutSummary);
        }
        let stage = RunStage::Failed {
            summary,
            manual_steps: Vec::new(),
        };
        self.stage = stage.clone();
        Ok(stage)
    }

    const fn stage_name(&self) -> &'static str {
        match self.stage {
            RunStage::Running => "running",
            RunStage::Done { .. } => "done",
            RunStage::Failed { .. } => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Random;
    use crate::run::{LogLevel, RunEvent, RunReport, Seq};

    struct SeededRng(u64);

    impl SeededRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
    }

    impl Random for SeededRng {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for chunk in dst.chunks_mut(8) {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                let bytes = z.to_be_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
    }

    /// 测试用随机源。
    ///
    /// # 为什么不是每次都从固定种子重建
    ///
    /// 直觉写法是 `fn rng() -> SeededRng { SeededRng::new(7) }` —— 每次调用都从种子 7
    /// 重建。那样**两次调用会产出完全一样的序列**，于是 `RunId::new(&mut rng())`
    /// 连着写两次拿到的是同一个 id。这会让"不同 run 的 id 不一样"这类断言
    /// 以最误导人的方式失败：看起来像业务逻辑错了，其实是测试夹具错了。
    ///
    /// 这里用一个进程内的计数器把种子岔开，并把上一次的游标留下来 ——
    /// 既保证可复现（同一个测试跑两次结果相同），又保证同一个测试里
    /// 多次取号不会撞。
    fn rng() -> SeededRng {
        static NEXT_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        SeededRng::new(
            NEXT_SEED.fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn step() -> ManualStep {
        ManualStep {
            capability: "msg-push".into(),
            account: None,
            instruction: "请前往 App 手动打开消息通知".into(),
            target: crate::run::ManualTarget::MobileApp,
        }
    }

    #[test]
    fn starts_running_without_outcome() {
        let exec = RunExecution::start(RunId::new(&mut rng()), "sign");
        assert_eq!(exec.stage(), &RunStage::Running);
        assert!(exec.outcome().is_none(), "还在跑就没有判定");
        assert!(!exec.is_terminal());
        assert_eq!(
            exec.stage().to_legacy_status(),
            crate::model::RunStatus::Running
        );
    }

    #[test]
    fn success_settles_to_done() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "sign");
        let stage = exec.finish(RunReport::success()).expect("应能结算");
        assert!(exec.is_terminal());
        assert_eq!(exec.outcome(), Some(RunOutcome::Success));
        // 关键：Done 不等于成功 —— 阶段是 Done，判定才是 Success
        assert_eq!(stage.to_legacy_status(), crate::model::RunStatus::Done);
    }

    /// 这是本模块存在的理由：**需人工必须能表达，且不能算成功**。
    #[test]
    fn manual_required_is_done_but_not_success() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "msg-push");
        exec.finish(RunReport::manual_required(vec![step()], "有一件待办"))
            .expect("应能结算");

        assert_eq!(exec.outcome(), Some(RunOutcome::ManualRequired));
        // 存储层看起来是 Done（前端不显示红色报错）
        assert_eq!(
            exec.stage().to_legacy_status(),
            crate::model::RunStatus::Done
        );
        // 但业务上**不是**成功
        assert!(!exec.outcome().expect("有判定").is_success_like());
        // 而且必须能推送
        assert!(exec.outcome().expect("有判定").needs_notification());
        assert_eq!(exec.stage().manual_steps().len(), 1);
    }

    /// 构造函数路径：空手说"需人工" → 上下游已经把它降级成部分成功。
    ///
    /// 所以这条**不该**报错 —— `manual_required(vec![], ..)` 返回的本来就是
    /// 一个 `PartialSuccess`，`finish` 只是照实结算。降级不是错误，是设计。
    ///
    /// 真正会被拒的是**字面量构造**那条路，见下一条测试。
    /// 两条测试合起来才说明"空待办的需人工进不了库"：一条管构造函数路径（降级），
    /// 一条管绕过构造函数路径（报错）。
    #[test]
    fn empty_manual_required_via_constructor_settles_as_partial_success() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "msg-push");
        let stage = exec
            .finish(RunReport::manual_required(Vec::new(), "没了"))
            .expect("构造函数已经降级过了，此处应当顺利结算");

        assert_eq!(exec.outcome(), Some(RunOutcome::PartialSuccess));
        assert!(exec.is_terminal(), "结算之后就是终态");
        assert_eq!(stage.to_legacy_status(), crate::model::RunStatus::Done);
        assert!(stage.manual_steps().is_empty());
        // 关键：**不是**需人工 —— 否则前端会弹一个"请去做某事"却说不出是什么的提示
        assert_ne!(exec.outcome(), Some(RunOutcome::ManualRequired));
    }

    /// 字面量绕过构造函数（字段是 `pub`）—— `normalize` 只升级不降级，
    /// 所以这种"空待办的需人工"会一路穿到 `finish`，必须在这里被拒。
    ///
    /// 这条是 `StageError::ManualRequiredWithoutSteps` 唯一的可达路径。
    /// 删掉那条校验就等于：只要不用构造函数，旧项目那句没人看的日志就能进库。
    #[test]
    fn hand_built_manual_required_without_steps_is_rejected() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "msg-push");
        let hand_built = RunReport {
            outcome: RunOutcome::ManualRequired,
            manual_steps: Vec::new(), // 空手
            summary: Some("我也说不清要做什么".into()),
        };
        let err = exec
            .finish(hand_built)
            .expect_err("手写的空待办需人工必须被拒");

        assert_eq!(err, StageError::ManualRequiredWithoutSteps);
        // 被拒之后**不能**变成终态 —— 调用方还能补上待办重试
        assert!(!exec.is_terminal(), "被拒之后不该变成终态");
        assert_eq!(exec.stage(), &RunStage::Running);
    }

    #[test]
    fn failed_requires_a_summary() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "sign");
        let err = exec
            .finish(RunReport::failed(""))
            .expect_err("空说明应被拒");
        assert_eq!(err, StageError::FailedWithoutSummary);
        assert!(!exec.is_terminal());

        // 只有空白也算空
        let err = exec
            .finish(RunReport::failed("   "))
            .expect_err("空白也算空");
        assert_eq!(err, StageError::FailedWithoutSummary);

        exec.finish(RunReport::failed("上游 502"))
            .expect("有说明就能过");
        assert_eq!(exec.outcome(), Some(RunOutcome::Failed));
        assert_eq!(
            exec.stage().to_legacy_status(),
            crate::model::RunStatus::Failed
        );
    }

    /// 终态不可再迁出 —— 否则前端会一直转圈（旧项目真出现过）。
    #[test]
    fn terminal_cannot_transition_again() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "sign");
        exec.finish(RunReport::success()).expect("第一次应成功");

        let err = exec
            .finish(RunReport::failed("改判"))
            .expect_err("终态后不能再 finish");
        assert_eq!(
            err,
            StageError::AlreadyTerminal {
                current: "done",
                attempted: "finish"
            }
        );
        assert_eq!(exec.outcome(), Some(RunOutcome::Success), "判定不该被改掉");

        let err = exec.fail("又挂了").expect_err("终态后不能再 fail");
        assert!(matches!(err, StageError::AlreadyTerminal { .. }));
    }

    #[test]
    fn executor_failure_needs_only_a_summary() {
        let mut exec = RunExecution::start(RunId::new(&mut rng()), "sign");
        exec.fail("执行器内部错误").expect("应能失败");
        assert_eq!(exec.outcome(), Some(RunOutcome::Failed));
        assert_eq!(
            exec.stage().to_legacy_status(),
            crate::model::RunStatus::Failed
        );
        // 执行器失败时还没有能力判定，所以没有待办
        assert!(exec.stage().manual_steps().is_empty());
    }

    /// 终局事件**至多且必须最后**一条。
    #[test]
    fn terminal_event_is_emitted_at_most_once() {
        let id = RunId::new(&mut rng());
        let mut exec = RunExecution::start(id.clone(), "sign");

        exec.record(&RunEvent::started(id.clone(), "sign"))
            .expect("开始事件可以记");
        exec.record(&RunEvent::progress(id.clone(), 1, 3))
            .expect("进度可以记");
        assert!(!exec.has_emitted_terminal());

        exec.record(&RunEvent::finished(
            id.clone(),
            RunOutcome::Success,
            &[],
            None,
        ))
        .expect("终局事件可以记一次");
        assert!(exec.has_emitted_terminal());

        // 第二次终局事件必须被拒
        let err = exec
            .record(&RunEvent::errored(id.clone(), "又挂了"))
            .expect_err("终局事件不得发第二次");
        assert!(matches!(err, StageError::AlreadyTerminal { .. }));

        // 终局之后的普通日志也不行 —— 否则前端会在收到 result 后又冒出日志
        let err = exec
            .record(&RunEvent::log(id.clone(), LogLevel::Info, "迟到的日志"))
            .expect_err("终局后不该再有事件");
        assert!(matches!(err, StageError::AlreadyTerminal { .. }));
    }

    #[test]
    fn events_from_another_run_are_rejected() {
        let mine = RunId::new(&mut rng());
        let theirs = RunId::new(&mut rng());
        assert_ne!(mine, theirs, "测试 RNG 要能产出不同 id");

        let mut exec = RunExecution::start(mine.clone(), "sign");
        let err = exec
            .record(&RunEvent::started(theirs, "sign"))
            .expect_err("别的 run 的事件不该记到这条执行上");
        assert!(matches!(err, StageError::RunIdMismatch { .. }));
    }

    /// 事件与阶段两条线要能对上：终局事件里的 outcome 就是阶段判定。
    #[test]
    fn terminal_event_outcome_matches_stage() {
        let id = RunId::new(&mut rng());
        let mut exec = RunExecution::start(id.clone(), "msg-push");
        exec.record(&RunEvent::started(id.clone(), "msg-push"))
            .expect("ok");

        let report = RunReport::manual_required(vec![step()], "有待办");
        exec.finish(report.clone()).expect("应能结算");
        let ev = RunEvent::finished(id, report.outcome, &report.manual_steps, report.summary);
        exec.record(&ev).expect("终局事件应能记");

        assert_eq!(ev.outcome(), exec.outcome(), "事件与阶段必须一致");
        assert_eq!(ev.outcome(), Some(RunOutcome::ManualRequired));
    }

    #[test]
    fn seq_walks_with_events() {
        // 序号分配与事件发布是两件事，这里演示它们配合起来的样子
        let id = RunId::new(&mut rng());
        let mut seq = Seq::start();
        let mut exec = RunExecution::start(id.clone(), "sign");

        let mut emitted = Vec::new();
        for ev in [
            RunEvent::started(id.clone(), "sign"),
            RunEvent::progress(id.clone(), 1, 2),
            RunEvent::finished(id.clone(), RunOutcome::Success, &[], None),
        ] {
            exec.record(&ev).expect("应能记");
            emitted.push(seq.next());
        }
        assert_eq!(
            emitted.iter().map(|s| s.value()).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
