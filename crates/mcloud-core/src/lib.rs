//! mcloud-core —— 领域类型与 IO trait 定义。
//!
//! **铁律：本 crate 不得引入任何 IO 依赖**（无 reqwest / sqlx / tokio::fs）。
//! 所有外部能力通过 trait 暴露，由上层注入实现，便于离线测试与 fixture 回放。

pub mod account;
pub mod cancel;
pub mod error;
pub mod event;
pub mod log;
pub mod pc_device;
pub mod pipeline;
pub mod run;
pub mod schedule;
pub mod smid;
pub mod traits;
pub mod user;

/// 测试替身（固定时钟等）。
///
/// 仅在 `testkit` feature 下编译，供其他 crate 的测试复用；
/// 见 docs/conventions.md §8.3——不要在各自 crate 里复制一份。
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use account::{Account, AccountId, AccountSettings, AuthInfo, DeviceId, PcPlatform};
pub use cancel::CancelToken;
pub use error::{Error, Result};
pub use event::{LogLevel, RunEvent, SeqAllocator, Sequenced};
pub use log::{AppendOutcome, LogLine, Seq, LOG_LIMIT_PER_RUN};
pub use pc_device::{PcDeviceStore, PC_DEVICE_SUFFIX};
pub use pipeline::{AccountMode, Pipeline, PipelineId, PipelineStep};
pub use run::{RunId, RunRecord, RunStatus, TaskOutcome};
pub use schedule::{Schedule, ScheduleId};
pub use smid::{SmidState, SmidStore};
pub use traits::{
    beijing_offset, beijing_time_digits, AccountStore, Clock, EventSink, HttpClient, HttpRequest,
    HttpResponse, LogStore, PipelineStore, RunStore, ScheduleStore, Store, UserStore,
};
pub use user::{
    validate_username, PasswordHash, Session, SessionKind, User, UserId, MIN_PASSWORD_LEN,
    WEB_SESSION_TTL_MS,
};
