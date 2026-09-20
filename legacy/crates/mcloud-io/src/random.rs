//! 生产随机源：OS CSPRNG。
//!
//! 放在 io 层而不是 core：取系统熵是 IO，core 的纯净性靠这条边界维持。
//! core 只定义 [`Random`] trait，并在测试里换成固定种子的实现 ——
//! 这正是"设备指纹能被 fixture 复现"的前提。

use mcloud_core::Random;

/// 基于操作系统熵池的密码学安全随机源。
///
/// 用 `OsRng` 而不是 `ThreadRng`：前者直接读 OS 熵源，
/// 不需要额外的用户态状态，且在所有支持的目标平台上都可用。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRandom;

impl Random for SystemRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        use rand::RngCore as _;
        // 取不到熵时**必须失败**而不是退化成弱随机：
        // 弱随机会让设备指纹/smid 可预测，进而影响风控判定。
        rand::rngs::OsRng.fill_bytes(dst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_random_produces_distinct_values() {
        let mut rng = SystemRandom;
        let a = rng.uuid_v4();
        let b = rng.uuid_v4();
        assert_ne!(a, b, "两次 UUID 不应相同");
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4', "版本位应为 4");
    }

    #[test]
    fn system_random_fills_buffer() {
        let mut rng = SystemRandom;
        let mut buf = [0u8; 32];
        rng.fill_bytes(&mut buf);
        assert_ne!(buf, [0u8; 32], "全零说明随机源没有真正工作");
    }

    #[test]
    fn below_stays_in_range() {
        let mut rng = SystemRandom;
        for _ in 0..200 {
            assert!(rng.below(6) < 6);
        }
    }
}
