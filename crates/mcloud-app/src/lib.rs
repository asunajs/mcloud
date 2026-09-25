//! mcloud-app —— 组装层。
//!
//! 把具体 IO 实现（reqwest / JSON 存储 / 系统时钟）注入 engine，
//! 供 `mcloud-cli` 与 `mcloud-server` 共用。

pub mod builder;
pub mod hub;
pub mod sink;

pub use builder::{App, AppBuilder};
pub use hub::{Delivery, EventHub};
pub use sink::BroadcastSink;

use std::sync::Arc;

/// 应用句柄。
pub struct AppHandle {
    pub runner: Arc<mcloud_engine::Runner>,
    pub registry: Arc<mcloud_engine::Registry>,
    pub store: Arc<dyn mcloud_core::Store>,
    pub clock: Arc<dyn mcloud_core::Clock>,
    /// 事件中枢：日志收集、序号分配、SSE 重放都经它。
    pub hub: Arc<EventHub>,
    /// 定时调度器。
    pub scheduler: Arc<mcloud_engine::Scheduler>,
    /// 编排执行器。
    pub pipelines: Arc<mcloud_engine::PipelineRunner>,
    /// 运行中任务的取消登记表。
    pub cancel: Arc<mcloud_engine::CancelRegistry>,
}

impl AppHandle {
    /// 事件中枢也是 `EventSink`，任务通过它发日志。
    pub fn sink(&self) -> Arc<dyn mcloud_core::EventSink> {
        Arc::clone(&self.hub) as Arc<dyn mcloud_core::EventSink>
    }
}
