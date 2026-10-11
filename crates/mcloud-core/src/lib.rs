//! mcloud-core —— 领域类型与 IO trait 定义。
//!
//! **铁律：本 crate 不得引入任何 IO 依赖**（无 reqwest / sqlx / tokio::fs）。
//! 所有外部能力通过 trait 暴露，由上层注入实现，便于离线测试与 fixture 回放。

pub mod account;
/// 「凭据自动刷新定时」的纯逻辑（id / 预设时间 / cron 三条算法）。
/// ⚠ 凭据续期的**判据与上游调用**不在这里 —— 那些在 `mcloud_app::auth_refresh` ✓。
pub mod auth_refresh;
pub mod cancel;
pub mod error;
pub mod event;
pub mod exchange;
pub mod kouling;
/// 「直播自动定时」的纯逻辑（id / 时刻 / cron 三条算法）。
/// ⚠ 「下一场」这个值**不在这里**算 —— 它来自共享口令池，见模块头 ✓。
pub mod live_auto;
pub mod log;
pub mod owner;
pub mod pc_device;
pub mod pipeline;
pub mod push;
pub mod run;
pub mod schedule;
pub mod settings;
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
pub use auth_refresh::{
    auth_refresh_schedule_id, daily_expr, parse_clock, AUTH_REFRESH_TASK, MANAGED_BY_AUTH_REFRESH,
};
pub use cancel::CancelToken;
pub use error::{Error, Result};
pub use event::{LogKind, LogLevel, RunEvent, SeqAllocator, Sequenced};
pub use exchange::ExchangeSubscription;
pub use kouling::{
    ClaimVerdict, KoulingPool, KoulingSource, KoulingState, LiveSession, PoolCode, PoolSummary,
    ProbeRoom,
};
pub use live_auto::{
    beijing_now, beijing_stamp, earliest_plus_one_hour, live_auto_schedule_id, one_shot_expr,
    LIVE_ROOM_TASK, MANAGED_BY_LIVE_AUTO,
};
pub use log::{
    log_line_of, slice_page, AppendOutcome, LogLine, LogPage, LogPageQuery, LogStats, Seq,
    LOG_LIMIT_PER_RUN,
};
pub use owner::{legacy_owner, visible_to};
pub use pc_device::{PcDeviceStore, PC_DEVICE_SUFFIX};
pub use pipeline::{AccountMode, Pipeline, PipelineId, PipelineStep};
pub use push::{
    BehaviorBinding, BehaviorKey, BehaviorState, BehaviorStatus, PanelBindings, PushChannelKey,
    PushConfig, PushEvent, PushRule, PushRuleId, Throttle, ThrottleDecision, ThrottleMode,
    ThrottleReason, ThrottleState,
};
pub use run::{RunId, RunLogLine, RunRecord, RunStatus, TaskOutcome, RUN_LOG_LIMIT};
pub use schedule::{Schedule, ScheduleId};
pub use settings::{
    AppSettings, DEFAULT_HISTORY_LIMIT, DEFAULT_LOG_LIMIT_PER_RUN, DEFAULT_LOG_TOTAL_MB,
    DEFAULT_MIN_PASSWORD_LEN, DEFAULT_SERVER_LOG_MEM_LINES, DEFAULT_SERVER_LOG_TOTAL_MB,
    DEFAULT_SESSION_TTL_HOURS, HISTORY_LIMIT_RANGE, LOG_LIMIT_PER_RUN_RANGE, LOG_TOTAL_MB_RANGE,
    SERVER_LOG_MEM_LINES_RANGE, SERVER_LOG_TOTAL_MB_RANGE,
};
pub use smid::{SmidState, SmidStore};
pub use traits::{
    beijing_format, beijing_offset, beijing_time_digits, format_with_offset, parse_beijing_time,
    utc_format, AccountStore, Clock, EventSink, HttpClient, HttpRequest, HttpResponse, LogStore,
    PipelineStore, RunStore, ScheduleStore, Store, UserStore,
};
pub use user::{
    validate_username, PasswordHash, Session, SessionKind, User, UserId, MIN_PASSWORD_LEN,
    WEB_SESSION_TTL_MS,
};
