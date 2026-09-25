//! 测试替身。
//!
//! 仅在 `testkit` feature（或本 crate 自身的测试）下编译。
//!
//! 其他 crate 的测试通过
//! `mcloud-core = { workspace = true, features = ["testkit"] }` 取用，
//! 而不是各写一份固定时钟——那样迟早会出现「几个行为不一致的假实现」，
//! 让测试结果依赖于你碰巧用了哪一个。

use crate::traits::Clock;

/// 固定时钟：永远返回构造时给定的毫秒时间戳。
///
/// 时长与到期时间断言依赖时间的确定性；换成真实时钟，这些测试会变成
/// 偶发失败，而且失败信息毫无价值（「期望 0，实际 1」）。
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_never_advances() {
        let c = FixedClock(1_700_000_000_000);
        assert_eq!(c.now_ms(), 1_700_000_000_000);
        assert_eq!(c.now_ms(), 1_700_000_000_000, "重复读取必须得到同一个值");
        // 默认实现基于 now_ms，也要跟着确定
        assert_eq!(c.now_secs(), 1_700_000_000);
    }
}
