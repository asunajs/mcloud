//! mcloud-app —— 组装层。
//!
//! 把具体 IO 实现（reqwest / JSON 存储 / 系统时钟）注入 engine，
//! 供 `mcloud-cli` 与 `mcloud-server` 共用。

pub mod builder;
pub mod sink;

pub use builder::{App, AppBuilder};
pub use sink::BroadcastSink;

use std::sync::Arc;

/// 应用句柄。
pub struct AppHandle {
    pub runner: Arc<mcloud_engine::Runner>,
    pub registry: Arc<mcloud_engine::Registry>,
    pub store: Arc<dyn mcloud_core::Store>,
    pub clock: Arc<dyn mcloud_core::Clock>,
    pub sink: Arc<BroadcastSink>,
}
