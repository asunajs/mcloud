//! mcloud-engine —— 任务注册表、运行器、并发闸与事件总线。
//!
//! **铁律：本 crate 不做 IO**。所有外部交互经 `mcloud-core` 的 trait。

pub mod artifacts;
pub mod cancel_registry;
pub mod details_ledger;
pub mod pipeline;
pub mod registry;
pub mod runner;
pub mod scheduler;

pub use artifacts::{Artifact, ArtifactKind, ArtifactLedger, ArtifactRecorder, SharedLedger};
pub use cancel_registry::{CancelRegistry, SharedCancelRegistry};
pub use details_ledger::{DetailsLedger, DetailsRecorder, SharedDetails};
pub use pipeline::{PipelineOutcome, PipelineRunner, StepStatus, StepSummary};
pub use registry::{Registry, Task, TaskMeta};
pub use runner::{HttpClientFactory, Runner};
pub use scheduler::{next_fire_at, Decision, FireOutcome, Scheduler, SchedulerState};

use mcloud_core::{Account, TaskOutcome};
use std::sync::Arc;

/// 任务执行上下文。
pub struct TaskContext {
    pub account: Account,
    pub http: Arc<dyn mcloud_core::HttpClient>,
    pub store: Arc<dyn mcloud_core::Store>,
    pub clock: Arc<dyn mcloud_core::Clock>,
    pub sink: Arc<dyn mcloud_core::EventSink>,
    pub run_id: mcloud_core::RunId,
    /// **本次运行**的取消令牌（用户在界面上点"中止这次运行"）。
    pub cancel: mcloud_core::CancelToken,
    /// **整条编排链**的取消令牌（用户中止整个 pipeline）。
    ///
    /// 与 `cancel` 是两个来源：单次运行的中止只停这一步，
    /// 编排的中止要停掉整条链。任务必须**两个都看** ——
    /// 只看一个的话，另一种中止方式对它就是无效的。
    pub chain_cancel: Option<mcloud_core::CancelToken>,
    /// 产物登记句柄：本任务往用户网盘写了东西就**必须**在这里上报。
    ///
    /// 它只能登记（`run_id` 与 `account` 已由执行器填好），
    /// 销账与遗忘只属于清理路径 —— 见 [`crate::ArtifactRecorder`]。
    ///
    /// 不上报的后果**不报错**：那文件永久留在用户网盘上，
    /// 而界面上没有任何一行字说明"这里留了东西"。
    pub artifacts: crate::ArtifactRecorder,
    /// 结构化明细句柄：本任务要把**形状**交回（而不只是一句话）就写这里。
    ///
    /// 写进去的东西会并进 `RunRecord.details` —— 运行历史里唯一
    /// "前端可直接渲染"的一格。`TaskOutcome` 那三档加一句 `summary`
    /// 装不下的东西（兑换的 `status / plan / code / offset / order`）
    /// 就是走这条。
    ///
    /// 与日志的分工：`ctx.info()` 是**给人读**的一句话，
    /// `ctx.details()` 是**给程序读**的形状。
    pub details: crate::DetailsRecorder,
}

impl TaskContext {
    /// 在**安全点**检查是否应中止。
    ///
    /// 长循环的任务（对一批文件逐个操作、逐个账号轮询）必须每轮调一次，
    /// 否则"中止"对它等于无效。已中止时返回 `Err(Error::Cancelled)`，
    /// 任务直接 `?` 上去即可。
    pub fn check_cancelled(&self) -> mcloud_core::Result<()> {
        self.cancel.check()?;
        if let Some(c) = &self.chain_cancel {
            c.check()?;
        }
        Ok(())
    }

    /// 是否已请求中止（不返回错误的版本，供 `if` 判断）。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.chain_cancel.as_ref().is_some_and(|c| c.is_cancelled())
    }
}

impl TaskContext {
    /// 发一条信息级日志。
    pub fn info(&self, msg: impl Into<String>) {
        use mcloud_core::{LogLevel, RunEvent};
        self.sink.emit(RunEvent::Log {
            run_id: self.run_id,
            level: LogLevel::Info,
            message: msg.into(),
            at_ms: self.clock.now_ms(),
        });
    }

    /// 发一条警告。
    pub fn warn(&self, msg: impl Into<String>) {
        use mcloud_core::{LogLevel, RunEvent};
        self.sink.emit(RunEvent::Log {
            run_id: self.run_id,
            level: LogLevel::Warn,
            message: msg.into(),
            at_ms: self.clock.now_ms(),
        });
    }

    /// 上报一条**本 run 造出来的**网盘文件（防漏记的入口）。
    ///
    /// 传 `pre_existing` 是**故意**的：那条判断是防误删的唯一防线，
    /// 不该让每个调用方自己记。详见 [`crate::ArtifactLedger::register_created`]。
    pub fn register_cloud_file(
        &self,
        file_id: impl Into<String>,
        pre_existing: bool,
    ) -> mcloud_core::Result<bool> {
        self.artifacts.register_cloud_file(file_id, pre_existing)
    }

    /// 上报进度。
    pub fn progress(&self, step: u32, total: u32, label: impl Into<String>) {
        use mcloud_core::RunEvent;
        self.sink.emit(RunEvent::Progress {
            run_id: self.run_id,
            step,
            total,
            label: label.into(),
        });
    }

    /// 自报一条**结构化**明细（给程序读，不是给人读）。
    ///
    /// 落点：运行结束时并进 `RunRecord.details`，前端从那里按字段渲染。
    ///
    /// # 两件事一起做，缺一不可
    ///
    /// ① 往账本里记 —— 这是**落历史**的那一份，`Runner` 收尾时取走；
    /// ② 往事件流里发 —— 这样订阅了这次运行的界面**当场**就能收到，
    ///    不必等运行结束再回头查历史。
    ///
    /// ⚠ 只做 ① 会让"跑的时候看不见、跑完才出现"；只做 ② 会让
    /// 没订阅（或用轮询）的调用方**永远拿不到**，因为事件流是不重放的。
    pub fn details(&self, payload: serde_json::Value) {
        self.details.record(payload.clone());
        self.sink.emit(mcloud_core::RunEvent::Details {
            run_id: self.run_id,
            payload,
            at_ms: self.clock.now_ms(),
        });
    }
}

/// 任务执行结果类型别名。
pub type TaskResult = mcloud_core::Result<TaskOutcome>;
