//! 认证面的限速（2026-10-04 复审 `sv-5`）。
//!
//! # 要挡的是什么
//!
//! `POST /api/session`（登录）与 `POST /api/setup`（初始化）都在
//! [`crate::auth_gate::PUBLIC`] 里 ⇒ **不需要任何凭据**就能打，而每一次都要跑一遍
//! argon2id（19 MiB × 2 轮，是整个链路里最贵的一步）。于是：
//!
//! ```text
//! for i in $(seq 100); do curl -s -X POST …/api/session & done
//! ```
//!
//! 就能把这台机器的单核占满、把正在跑的定时任务挤慢 —— 而 `MCLOUD_ADDR` 是
//! **可配**的（`main.rs`），一旦按文档绑到 `0.0.0.0` / LAN，这就是一个**免认证**的
//! DoS 面 ✗。
//!
//! # 为什么是自己写的滑动窗口，而不是拉 `tower_governor`
//!
//! 那个依赖的价值在"按 IP 限速"，而按 IP 需要 `serve` 处接上
//! `into_make_service_with_connect_info`（本版没接）。在**拿不到更细的键**之前，
//! 一个依赖换不来更多真实保护，所以这里用一个几十行的滑动窗口：
//!
//! * **滑动窗口**（不是固定窗口）：`max` 次散布在任意 `window_ms` 里都算满，
//!   避免"窗口边界前后各打一发"这类绕过 ✓；
//! * 只存**窗口内**的时刻 ⇒ 内存上界就是 `max` 条 ✓。
//!
//! # ⚠ 诚实的边界
//!
//! 它是**全局**的（不是按 IP/按用户名）：一个攻击者因此也能把管理员本人挡在门外
//! 一小会儿。要把这条做细需要两件事 —— `serve` 接 `ConnectInfo`、键换成 peer IP，
//! **本版没做** ✗。在"本机 + 单管理员"这个部署形态下，先把"无限并发"变成
//! "每分钟最多 N 次"已经是这一步要的效果 ✓。

use std::collections::VecDeque;
use std::sync::Mutex;

/// 一个滑动窗口限速器。**所有方法都不跨 `await`**（锁只在同步段里持有）。
#[derive(Debug)]
pub struct RateLimiter {
    window_ms: i64,
    max: usize,
    /// 窗口内每次尝试的时刻（毫秒），升序。
    hits: Mutex<VecDeque<i64>>,
}

impl RateLimiter {
    /// `max` 次 / `window_ms` 毫秒。
    ///
    /// `max == 0` 会被抬到 1：0 意味着"永远拒绝"，那不是限速、是把路由关掉 ✗
    ///（要关就用路由本身，别用限速表达）。
    pub fn new(window_ms: i64, max: usize) -> Self {
        Self {
            window_ms: window_ms.max(1),
            max: max.max(1),
            hits: Mutex::new(VecDeque::new()),
        }
    }

    /// 记一次尝试。
    ///
    /// * `Ok(())` —— 这一次在配额内（**且已经计入**）；
    /// * `Err(还要等多少秒)` —— 超限，调用方应当回 429 + `Retry-After`。
    ///
    /// ⚠ 计数发生在**判断之后**：被拒的那几次不占配额，否则攻击者只要一直打，
    /// 窗口就永远清不空，管理员会被**永久**挡在外面 ✗。
    pub fn check(&self, now_ms: i64) -> Result<(), i64> {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());

        // 先丢掉滑出窗口的
        while let Some(front) = hits.front() {
            if now_ms.saturating_sub(*front) >= self.window_ms {
                hits.pop_front();
            } else {
                break;
            }
        }

        if hits.len() >= self.max {
            let oldest = *hits.front().expect("非空才有 len >= max >= 1");
            let wait_ms = self.window_ms - now_ms.saturating_sub(oldest);
            // 向上取整到秒：Retry-After 是秒，报少了客户端会立刻再撞一次
            return Err((wait_ms.max(1) + 999) / 1000);
        }

        hits.push_back(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ 复审 `sv-5`：配额内的放行、超出即拒、**窗口滑过去之后自己恢复**。
    ///
    /// 红测：把 `check` 里 `hits.len() >= self.max` 那句删掉（等于不限速）⇒ 红；
    /// 或把"被拒时不计数"改成"先 push 再判"⇒ 最后一条"窗口过后能恢复"会红
    ///（被拒的那些把窗口塞满，永远恢复不了）。
    #[test]
    fn a_sliding_window_lets_quota_through_and_recovers() {
        let rl = RateLimiter::new(60_000, 3);
        let t0 = 1_700_000_000_000;

        for i in 0..3 {
            assert!(rl.check(t0 + i).is_ok(), "第 {i} 次在配额内");
        }

        // 第 4 次：超限，且要等**最老那次**滑出窗口（t0 + 60s - t0-3ms ≈ 60s）
        let wait = rl.check(t0 + 3).expect_err("第 4 次必须被拒");
        assert!(
            (59..=60).contains(&wait),
            "Retry-After 应当在 ~60 秒：{wait}"
        );

        // 被拒的这次**不占配额** ⇒ 窗口一过立刻恢复（不是"还得再等一轮"）
        assert!(rl.check(t0 + 60_000).is_ok(), "窗口过后应当放行");
        // 但紧接着的下一发又被新窗口的配额挡住
        assert!(rl.check(t0 + 60_001).is_ok(), "新窗口的第 2 个");
        assert!(rl.check(t0 + 60_002).is_ok(), "新窗口的第 3 个");
        assert!(
            rl.check(t0 + 60_003).is_err(),
            "新窗口的第 4 个又超了（窗口是滑动的，不是一过 60s 就重置成无限）"
        );
    }

    /// `max = 0` 抬到 1：限速不该被当成"把路由关掉"的开关。
    #[test]
    fn zero_max_is_clamped_to_one() {
        let rl = RateLimiter::new(1_000, 0);
        assert!(rl.check(0).is_ok());
        assert!(rl.check(1).is_err());
    }
}
