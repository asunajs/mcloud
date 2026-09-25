//! mcloud-engine —— 任务注册表、运行器、并发闸与事件总线。
//!
//! **铁律：本 crate 不做 IO**。所有外部交互经 `mcloud-core` 的 trait。

pub mod cancel_registry;
pub mod pipeline;
pub mod registry;
pub mod runner;
pub mod scheduler;

pub use cancel_registry::{CancelRegistry, SharedCancelRegistry};
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
}

/// 任务执行结果类型别名。
pub type TaskResult = mcloud_core::Result<TaskOutcome>;
