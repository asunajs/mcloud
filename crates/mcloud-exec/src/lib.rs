//! mcloud-exec —— 执行器层。
//!
//! 职责（P5 实现）：
//! - `runId`（uuid v4）作为幂等单元，同 runId 重放不重复执行；
//! - **并发闸**：默认串行（防风控）、全局并发上限可配、同「能力 + 账号」互斥、
//!   长等待型任务（抢兑预热）在等待期释放并发槽；
//! - **事件总线**：`log` / `progress` / `result` / `error` → `broadcast` → SSE，
//!   同时落 `runs` / `run_events` 表（替代原 TS 版仅存内存的 `AsyncRunState`）。
//!
//! # 状态
//!
//! P0 只定义并发语义这一处最容易做错的东西 —— 它是**纯函数**，
//! 因此现在就能被测住，而不是等到 P5 靠真机试跑验证。

#![forbid(unsafe_code)]

use mcloud_core::AccountMode;

/// 并发闸配置。
#[derive(Debug, Clone, Copy)]
pub struct Concurrency {
    /// 全局同时在跑的任务上限。默认 2 —— 原项目要求默认串行防风控。
    pub global_limit: usize,
}

impl Default for Concurrency {
    fn default() -> Self {
        Self { global_limit: 2 }
    }
}

impl Concurrency {
    /// 给定账号数与执行方式，算出实际并行度。
    ///
    /// 规则：
    /// - `Sequential` → 恒为 1（逐个跑），这正是"默认串行防风控"的落点；
    /// - `Parallel` → 取 `min(账号数, 全局上限)`，**永远不超过上限**；
    /// - 账号数为 0 → 0（空集合不占用槽位）。
    pub fn effective_parallelism(&self, account_count: usize, mode: AccountMode) -> usize {
        match mode {
            AccountMode::Sequential => usize::from(account_count > 0),
            AccountMode::Parallel => account_count.min(self.global_limit.max(1)),
        }
    }

    /// 该账号是否可以立刻开始（对齐"同能力+账号互斥"的简化形式）。
    ///
    /// 与 [`Self::effective_parallelism`] 保持一致地下限取 1：
    /// `global_limit` 被写错成 0 时应当"只跑一个"而不是**彻底锁死**。
    /// 静默死锁比跑慢更难排查，所以这里选择可推进。
    ///
    /// 这里手写三元而不用 `self.global_limit.max(1)`：`Ord::max` 在 Rust 1.91 还不是
    /// `const`，用了就无法保持 `const fn`。
    pub const fn can_start(&self, running: usize) -> bool {
        let limit = if self.global_limit == 0 {
            1
        } else {
            self.global_limit
        };
        running < limit
    }
}

/// 互斥键：同一「能力 + 账号」不允许并发。
///
/// 用途：抢兑这类高竞争动作，同一账号并发请求既容易触发风控、
/// 也可能造成重复兑换。
pub fn mutex_key(capability: &str, account: &str) -> String {
    format!("{capability}#{account}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_never_parallelizes() {
        let concurrency = Concurrency::default();
        for count in [1, 2, 5, 100] {
            assert_eq!(
                concurrency.effective_parallelism(count, AccountMode::Sequential),
                1,
                "串行模式下 {count} 个账号也必须逐个跑"
            );
        }
    }

    #[test]
    fn parallel_is_capped_by_global_limit() {
        let concurrency = Concurrency { global_limit: 2 };
        assert_eq!(
            concurrency.effective_parallelism(1, AccountMode::Parallel),
            1
        );
        assert_eq!(
            concurrency.effective_parallelism(2, AccountMode::Parallel),
            2
        );
        assert_eq!(
            concurrency.effective_parallelism(10, AccountMode::Parallel),
            2,
            "并行也不得突破全局上限"
        );
    }

    #[test]
    fn empty_account_set_uses_no_slot() {
        let concurrency = Concurrency::default();
        assert_eq!(
            concurrency.effective_parallelism(0, AccountMode::Sequential),
            0
        );
        assert_eq!(
            concurrency.effective_parallelism(0, AccountMode::Parallel),
            0
        );
    }

    #[test]
    fn zero_global_limit_still_allows_one() {
        // 配置写错成 0 时不应把系统锁死
        let concurrency = Concurrency { global_limit: 0 };
        assert_eq!(
            concurrency.effective_parallelism(3, AccountMode::Parallel),
            1
        );
        assert!(concurrency.can_start(0));
    }

    #[test]
    fn mutex_key_separates_capability_and_account() {
        assert_eq!(mutex_key("exchange", "acc-1"), "exchange#acc-1");
        assert_ne!(
            mutex_key("exchange", "acc-1"),
            mutex_key("exchange", "acc-2")
        );
        assert_ne!(mutex_key("exchange", "acc-1"), mutex_key("sign", "acc-1"));
    }
}
