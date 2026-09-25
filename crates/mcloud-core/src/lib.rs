//! mcloud-core —— 领域类型与 IO trait 定义。
//!
//! **铁律：本 crate 不得引入任何 IO 依赖**（无 reqwest / sqlx / tokio::fs）。
//! 所有外部能力通过 trait 暴露，由上层注入实现，便于离线测试与 fixture 回放。

pub mod account;
pub mod error;
pub mod event;
pub mod run;
pub mod traits;

pub use account::{Account, AccountId, AccountSettings, AuthInfo, DeviceId};
pub use error::{Error, Result};
pub use event::{LogLevel, RunEvent};
pub use run::{RunId, RunRecord, TaskOutcome};
pub use traits::{Clock, EventSink, HttpClient, HttpRequest, HttpResponse, Store};
