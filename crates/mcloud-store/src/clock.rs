//! 系统时钟。

use mcloud_core::Clock;

/// 基于系统时间的时钟。
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

/// 固定时钟，测试用。
#[cfg(test)]
pub struct FixedClock(pub i64);

#[cfg(test)]
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
