//! mcloud-io —— IO 实现层。
//!
//! 这是**唯一**允许依赖 `reqwest` / `rusqlite` 的 crate（由 `scripts/check-deps.sh`
//! 断言，门禁第 4 条）。
//! 上游层与能力层只看到 `mcloud_core` 里的 trait，因此可以被替换成
//! fixture 客户端 / 内存 store 进行离线验证。

pub mod fixture;
pub mod legacy_import;
pub mod random;
pub mod reqwest_client;
pub mod store;

use chrono::{DateTime, Utc};
use mcloud_core::Clock;

pub use fixture::{FixtureEntry, FixtureHttpClient};
pub use legacy_import::{
    LEGACY_DATA_FILES, LegacyImportError, LegacySource, MigrationIssue, MigrationReport,
    archive_legacy_source, diagnose_legacy, import_legacy,
};
pub use random::SystemRandom;
pub use reqwest_client::{ReqwestClient, RetryPolicy};
pub use store::SqliteStore;

/// 真实时钟。
///
/// 放在 io 层而不是 core：`SystemTime::now()` 是 IO，
/// core 的纯净性靠这条边界维持。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::Clock;

    #[test]
    fn system_clock_is_monotonic_enough_for_ordering() {
        let clock = SystemClock;
        let a = clock.now();
        let b = clock.now();
        assert!(b >= a, "时钟不应倒退");
    }
}
