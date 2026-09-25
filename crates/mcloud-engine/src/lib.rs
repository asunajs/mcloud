//! mcloud-engine —— 任务注册表、运行器、并发闸与事件总线。
//!
//! **铁律：本 crate 不做 IO**。所有外部交互经 `mcloud-core` 的 trait。

pub mod registry;
pub mod runner;

pub use registry::{Registry, Task, TaskMeta};
pub use runner::{HttpClientFactory, Runner};

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
