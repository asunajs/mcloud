//! 系统时钟。

use mcloud_core::Clock;

/// 基于系统时间的时钟。
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

// 固定时钟已移到 `mcloud_core::testkit::FixedClock`：
// 它之前是这里的 `#[cfg(test)]` 私有项，别处用不到，engine 层要测时间就得
// 各写一份。统一放在 core（配合 `testkit` feature）后，所有 crate 共用同一个
// 实现。见 docs/conventions.md §8.3。
