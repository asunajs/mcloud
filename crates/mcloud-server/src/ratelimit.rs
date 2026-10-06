//! 认证面的限速（2026-10-04 复审 `sv-5`；2026-10-06 安全审计 `SEC-02` 改成**按 IP**）。
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
//! # ⚠⚠ 2026-10-06：从「全局一个计数器」改成「按 peer IP 分桶」
//!
//! 第一版（`sv-5`）是**全局单键**，两个方向都不对 ✗：
//!
//! 1. **挡不住任何分布式的东西** —— 攻击者换台机器照样打；
//! 2. 更糟的是**它自己变成了一件武器**：被拒的请求**不占配额**（下面那条注释
//!    是有意的），于是攻击者只要持续打，窗口里永远有 `max` 条 ⇒ **管理员每次
//!    登录都被 429 挡回来**，而 WebUI 是这个产品**唯一**的配置入口 ✗。
//!
//! 现在键是 peer IP（`serve` 侧接了 `ConnectInfo`，见 `main.rs`）：
//! 同一个 IP 打满只锁它自己，别人不受影响 ✓。拿不到 peer IP 时（例如测试直接
//! 打 Router、或将来有人换掉 `serve` 的装配）**退化成 `"unknown"` 这一个桶** ——
//! 也就是退回旧行为：仍然限速，但会互相影响。**这个退化是刻意的**：
//! 宁可限速得太粗，也不要"拿不到键 ⇒ 干脆不限" ✗。
//!
//! # 为什么是自己写的滑动窗口，而不是拉 `tower_governor`
//!
//! * **滑动窗口**（不是固定窗口）：`max` 次散布在任意 `window_ms` 里都算满，
//!   避免"窗口边界前后各打一发"这类绕过 ✓；
//! * 只存**窗口内**的时刻 ⇒ 单个键的内存上界就是 `max` 条 ✓；
//! * 键的数量也有上界（[`DEFAULT_MAX_KEYS`]）—— 否则"伪造一堆源 IP"就能把
//!   这张表撑爆，那是**用限速器自己**做 DoS ✗（`check` 里那段淘汰就是为它写的）。
//!
//! # ⚠ 诚实的边界
//!
//! 按 IP 限速在**反代**后面会退化成"所有客户端共用一个桶"（`X-Forwarded-For`
//! 本仓**完全没读**，因为读它就等于信任一个可伪造的头）。要按真实客户端限速，
//! 得先定义那个头的信任边界（见审计报告 SEC-02 的修复建议）。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// 同时最多记多少个键。
///
/// 4096 × `max` 条 `i64` ≈ 最坏几 MB，而正常部署（本机 / 一个 LAN）根本到不了
/// 这个量级 —— 它是"被灌爆"时的护栏，不是容量规划 ✓。
pub const DEFAULT_MAX_KEYS: usize = 4096;

/// 一个**按 key 分桶**的滑动窗口限速器。
///
/// **所有方法都不跨 `await`**（锁只在同步段里持有）。
#[derive(Debug)]
pub struct RateLimiter {
    window_ms: i64,
    max: usize,
    /// 键的数量上界（见 [`DEFAULT_MAX_KEYS`]）。
    max_keys: usize,
    /// key（生产里是 peer IP 的字符串）→ 窗口内每次尝试的时刻（毫秒），升序。
    hits: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl RateLimiter {
    /// `max` 次 / `window_ms` 毫秒，**每个 key 各算一份**。
    ///
    /// `max == 0` 会被抬到 1：0 意味着"永远拒绝"，那不是限速、是把路由关掉 ✗
    ///（要关就用路由本身，别用限速表达）。
    pub fn new(window_ms: i64, max: usize) -> Self {
        Self::with_max_keys(window_ms, max, DEFAULT_MAX_KEYS)
    }

    /// 同上，但显式给键的数量上界（测试用：用小值就能验淘汰逻辑）。
    pub fn with_max_keys(window_ms: i64, max: usize, max_keys: usize) -> Self {
        Self {
            window_ms: window_ms.max(1),
            max: max.max(1),
            max_keys: max_keys.max(1),
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// 记一次 `key` 的尝试。
    ///
    /// * `Ok(())` —— 这一次在配额内（**且已经计入**）；
    /// * `Err(还要等多少秒)` —— 超限，调用方应当回 429 + `Retry-After`。
    ///
    /// ⚠ 计数发生在**判断之后**：被拒的那几次不占配额，否则攻击者只要一直打，
    /// 窗口就永远清不空，管理员会被**永久**挡在外面 ✗。
    /// （"被拒的不计数"与"按 IP 分桶"是**两条独立的修复**：前者保证窗口能自己
    /// 恢复，后者保证恢复的是**攻击者自己那一桶**，不是所有人共用的那一桶 ✓。）
    pub fn check(&self, key: &str, now_ms: i64) -> Result<(), i64> {
        let mut buckets = self.hits.lock().unwrap_or_else(|e| e.into_inner());

        // 键数到顶才做全量清扫：正常情况下每次请求只碰自己那一桶（O(1)），
        // 不给"每次请求都遍历整张表"留一条 CPU 放大的路 ✓。
        if buckets.len() >= self.max_keys && !buckets.contains_key(key) {
            buckets.retain(|_, hits| {
                prune(hits, now_ms, self.window_ms);
                !hits.is_empty()
            });
            // 清完还是满（窗口内的活键真的比上界多）⇒ 丢掉"最久没动过"的那一桶。
            // 它是最不可能还在窗口里的，代价最小；而**被丢的是别人的桶**这件事
            // 只有在"键数被灌到上界"时才会发生 —— 那已经是攻击形态了 ✓。
            if buckets.len() >= self.max_keys {
                if let Some(oldest) = buckets
                    .iter()
                    .min_by_key(|(_, hits)| hits.back().copied().unwrap_or(i64::MIN))
                    .map(|(k, _)| k.clone())
                {
                    buckets.remove(&oldest);
                }
            }
        }

        let hits = buckets.entry(key.to_string()).or_default();
        prune(hits, now_ms, self.window_ms);

        if hits.len() >= self.max {
            let oldest = *hits.front().expect("非空才有 len >= max >= 1");
            let wait_ms = self.window_ms - now_ms.saturating_sub(oldest);
            // 向上取整到秒：Retry-After 是秒，报少了客户端会立刻再撞一次
            return Err((wait_ms.max(1) + 999) / 1000);
        }

        hits.push_back(now_ms);
        Ok(())
    }

    /// 当前记着多少个键（测试用；也是"内存有没有上界"的读数）。
    pub fn tracked_keys(&self) -> usize {
        self.hits.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// 把某个键的计数**清零**（2026-10-06 安全审计 `SEC-12`）。
    ///
    /// # 为什么需要它
    ///
    /// "登录失败计数"要的是**失败**次数，而限速器数的是**调用**次数 ——
    /// 于是"数失败"只能由调用方决定：失败时 `check`（计入），成功时 `reset`（清零）。
    ///
    /// ⚠⚠ 清零挂在**成功**上，是"不会把管理员锁在门外"的关键：
    /// 口令对的那一次**永远不会被计数**，所以配额只限制"猜错的速度"，
    /// 不限制"能不能进来" ✓（这正是 `SEC-02` 那个坑的教训 —— 限速器自己
    /// 绝不能变成一件把合法用户挡在外面的武器 ✗）。
    pub fn reset(&self, key: &str) {
        self.hits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

/// 丢掉滑出窗口的时刻。
fn prune(hits: &mut VecDeque<i64>, now_ms: i64, window_ms: i64) {
    while let Some(front) = hits.front() {
        if now_ms.saturating_sub(*front) >= window_ms {
            hits.pop_front();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个固定的测试 IP；用它当键，读起来比 `"k"` 清楚。
    const IP: &str = "203.0.113.7";

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
            assert!(rl.check(IP, t0 + i).is_ok(), "第 {i} 次在配额内");
        }

        // 第 4 次：超限，且要等**最老那次**滑出窗口（t0 + 60s - t0-3ms ≈ 60s）
        let wait = rl.check(IP, t0 + 3).expect_err("第 4 次必须被拒");
        assert!(
            (59..=60).contains(&wait),
            "Retry-After 应当在 ~60 秒：{wait}"
        );

        // 被拒的这次**不占配额** ⇒ 窗口一过立刻恢复（不是"还得再等一轮"）
        assert!(rl.check(IP, t0 + 60_000).is_ok(), "窗口过后应当放行");
        // 但紧接着的下一发又被新窗口的配额挡住
        assert!(rl.check(IP, t0 + 60_001).is_ok(), "新窗口的第 2 个");
        assert!(rl.check(IP, t0 + 60_002).is_ok(), "新窗口的第 3 个");
        assert!(
            rl.check(IP, t0 + 60_003).is_err(),
            "新窗口的第 4 个又超了（窗口是滑动的，不是一过 60s 就重置成无限）"
        );
    }

    /// `max = 0` 抬到 1：限速不该被当成"把路由关掉"的开关。
    #[test]
    fn zero_max_is_clamped_to_one() {
        let rl = RateLimiter::new(1_000, 0);
        assert!(rl.check(IP, 0).is_ok());
        assert!(rl.check(IP, 1).is_err());
    }

    /// ⭐⭐ 审计 `SEC-02` 的正题：**一个 IP 打满，不许影响另一个 IP**。
    ///
    /// 这条是"全局单键"那版最致命的地方 —— 攻击者打满配额，管理员就登不上了。
    ///
    /// 红测：把 `check` 里的 `key` 忽略掉（用一个固定串当键）⇒ 本条的第二个断言红。
    #[test]
    fn one_flooded_ip_does_not_lock_out_another() {
        let rl = RateLimiter::new(60_000, 2);
        let attacker = "198.51.100.9";
        let admin = "192.0.2.1";

        assert!(rl.check(attacker, 0).is_ok());
        assert!(rl.check(attacker, 1).is_ok());
        assert!(rl.check(attacker, 2).is_err(), "攻击者自己该被挡住");

        // 管理员（另一个 IP）不受影响 —— 这正是改成按 IP 分桶要的效果
        assert!(rl.check(admin, 3).is_ok(), "别人打满不该把管理员也挡住");
        assert!(rl.check(admin, 4).is_ok());
        assert!(
            rl.check(admin, 5).is_err(),
            "管理员自己的配额也会满，但只挡他自己"
        );
    }

    /// 键的数量必须有上界：否则"伪造一堆源 IP"就是用限速器自己做 DoS。
    ///
    /// 红测：把 `check` 里那段 `max_keys` 淘汰删掉 ⇒ `tracked_keys()` 会等于灌进去的
    /// 键数（10_000），本条红。
    #[test]
    fn the_number_of_tracked_keys_is_bounded() {
        let rl = RateLimiter::with_max_keys(60_000, 3, 8);
        for i in 0..10_000 {
            let _ = rl.check(&format!("10.0.{}.{}", i / 256, i % 256), 0);
        }
        assert!(
            rl.tracked_keys() <= 8,
            "键数必须被钉在上界：{}",
            rl.tracked_keys()
        );
    }

    /// 窗口滑过之后，那一桶**不该继续占着键位**（键位是稀缺资源）。
    #[test]
    fn expired_keys_are_reclaimed_when_the_table_is_full() {
        let rl = RateLimiter::with_max_keys(1_000, 3, 4);
        let t0 = 1_000_000;
        for i in 0..4 {
            assert!(rl.check(&format!("old-{i}"), t0).is_ok());
        }
        assert_eq!(rl.tracked_keys(), 4);

        // 一整个窗口之后，旧键全部过期 ⇒ 新键进来时应当把它们清掉，而不是挤掉
        let _ = rl.check("new", t0 + 1_000);
        assert!(
            rl.tracked_keys() < 4,
            "过期的桶该被回收，实际还留着 {} 个",
            rl.tracked_keys()
        );
    }

    /// ⭐ 审计 `SEC-12`：**成功登录要能把失败计数清零**。
    ///
    /// 不清的话，一个"记错两次、第三次对了"的用户会一直背着那两次失败；
    /// 更要命的是**口令正确的那一次也会被算进配额** ⇒ 连着失败几次之后，
    /// 即使输入正确的口令也进不来 —— 那是"限速器变成锁死管理员"的同一类错 ✗。
    ///
    /// 红测：把 `reset` 改成空函数 ⇒ 本测试第二条断言红。
    #[test]
    fn reset_clears_the_counter_so_a_correct_login_is_never_penalised() {
        let rl = RateLimiter::new(300_000, 3);
        let key = "admin@127.0.0.1";

        // 两次失败（调用方在失败时才计入）
        assert!(rl.check(key, 0).is_ok());
        assert!(rl.check(key, 1).is_ok());

        // 成功 ⇒ 清零
        rl.reset(key);
        assert_eq!(rl.tracked_keys(), 0, "清零后这个键不该还占着");

        // 于是窗口内又是完整的配额，而不是"只剩 1 次"
        for i in 0..3 {
            assert!(rl.check(key, 10 + i).is_ok(), "清零后第 {} 次该放行", i + 1);
        }
        assert!(rl.check(key, 20).is_err(), "配额本身还在");
    }
}
