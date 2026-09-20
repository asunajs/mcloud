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
//! 目前落地的是**纯函数部分**：并发语义与互斥键。这两块不需要 IO，
//! 因此现在就能被测住，而不是等到 P5 靠真机试跑验证。
//! 真正需要 IO 的部分（跑任务、落库、SSE）在 P5 接。

#![forbid(unsafe_code)]

use mcloud_core::{AccountMode, RunId};

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

/// 互斥键：同一「能力 + 账号 + **参数摘要**」不允许并发。
///
/// # 为什么参数维度不能省（这是全套代码里最危险的语义）
///
/// 旧项目 `runner.ts` 的 key 是 `${task.name}:${accountLabel}` —— **不含参数**。
/// 它的 join 语义是"已存在 promise 就直接返回同一个结果"，于是：
///
/// > 同一账号带不同 `prizeId` 的第二次请求，**拿回的是第一次的结果**。
///
/// 表现是"换了个奖品去抢，界面显示抢到了上一个奖品" —— 没有任何报错，
/// 因为对旧代码而言这就是"同一个任务已经在跑，复用结果"。
///
/// 所以新实现把参数进 key，让"不同参数"成为**不同的锁**而不是同一次运行。
/// 这比"复用结果"更符合直觉，也不会静默给错答案。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutexKey(String);

impl MutexKey {
    /// 组装互斥键。
    ///
    /// `params_digest` 是调用方对**本次运行的参数**算出的稳定摘要
    /// （见 [`params_digest`]）。传空串表示"该能力确实没有参数维度"，
    /// 此时行为退化为旧的「能力 + 账号」—— 这是**显式**声明的，不是默认地漏掉。
    pub fn new(capability: &str, account: &str, params_digest: &str) -> Self {
        if params_digest.is_empty() {
            return Self(format!("{capability}#{account}"));
        }
        Self(format!("{capability}#{account}#{params_digest}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MutexKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 把一组参数压成一个短摘要，供 [`MutexKey`] 使用。
///
/// # 为什么用"排序后的键值对 + 哈希"而不是 `serde_json::to_string`
///
/// 参数来自 `HashMap` / `serde_json::Value` 这类**无序**容器时，
/// 直接序列化会因遍历顺序不同而产出不同字符串 ——
/// 那么"同一组参数"会被算成两个不同的锁，互斥就失效了（同一账号能并发跑两次）。
/// 所以先按 key 排序再拼，保证同一集合**恒定**产出同一个摘要。
///
/// 摘要取哈希而不是原文，是为了让 `MutexKey` 能安全地进日志与 URL ——
/// 参数里可能有奖品名、口令这类不该外泄的内容。
///
/// 传入 `None` 返回空串（= 该能力没有参数维度）。
pub fn params_digest(params: Option<&[(String, String)]>) -> String {
    let Some(pairs) = params else {
        return String::new();
    };

    // 空集合也返回空串，与 `None` 同义 —— **必须显式处理**：
    // 否则 sha256("") 会返回一个 64 位十六进制串，"没有参数维度"就变成了
    // "有一个固定参数"，与 `MutexKey::new` 里 `is_empty()` 的约定对不上。
    if pairs.is_empty() {
        return String::new();
    }

    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    // 长度前缀分隔，避免 ("ab","c") 与 ("a","bc") 拼出同一个串
    let mut buf = String::new();
    for (k, v) in sorted {
        buf.push_str(&k.len().to_string());
        buf.push(':');
        buf.push_str(k);
        buf.push_str(&v.len().to_string());
        buf.push(':');
        buf.push_str(v);
        buf.push('|');
    }

    // 复用 core 的 sha256，避免在 exec 里再引一个哈希实现
    mcloud_core::sha256_hex(buf.as_bytes())
}

/// 一次运行的准入判定结果。
///
/// 只保留"判断要不要真的开跑"所需的最小信息，**不含结果** ——
/// 有了它就能回答这三选一，而结果由存储层回答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// 这个 runId 是新的，可以开跑。
    Admitted,
    /// 同一 `runId` 已经跑过（或正在跑）—— 直接复用既有结果，不重复执行。
    DuplicateRunId(RunId),
    /// 另一个 runId 正握着同一把锁 —— 这次**不能开跑，也不能假装成功**。
    ///
    /// `holder` 是占用者的 runId：排查时"被谁挡了"比"被挡了"有用得多。
    /// 它是**诊断信息**，不参与判定，所以不影响 `PartialEq` 的语义直觉。
    Blocked { key: MutexKey, holder: RunId },
}

/// 运行登记表：把"runId 幂等"与"参数互斥"两件事放在一处判定。
///
/// 这两件事必须**一起**判：只查 runId 会让不同参数被误判成重复，
/// 只查锁会让同一 runId 重放跑两次。旧项目两个都没做。
///
/// 这是纯内存实现（P5 会加落库的那一半：跨进程重放靠 `runs` 表）。
#[derive(Debug, Default)]
pub struct RunRegistry {
    /// 已受理的 runId 及其占用的锁。
    accepted: std::collections::HashMap<RunId, MutexKey>,
    /// 锁 → 当前持有的 runId。
    held: std::collections::HashMap<MutexKey, RunId>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 申请开跑。**这是唯一入口** —— 不要在外面自己判断后直接执行。
    ///
    /// 顺序很重要：**先查 runId、再查锁**。
    /// 反过来的话，重放同一个 runId 会先撞上自己持有的锁、
    /// 被判成 `Blocked` 而不是 `DuplicateRunId`，调用方就会以为"有人占着，等会儿再来"，
    /// 于是永远等不到 —— 因为占着它的正是它自己。
    ///
    /// # 为什么这里**不**断言两次传入的 key 相同
    ///
    /// 试过在重放分支上 `debug_assert_eq!(existing_key, &key)`，结论是**不能这么写**：
    /// 调用方若真把 runId 复用到别的参数上，它**必然**要在登记表已经记下这个 runId
    /// （也就是还锁着）时来问 —— 而那个场景正是这里要支持的：
    /// 先 `DuplicateRunId` 被拒 ⇒ 等下把拿到的结果丢掉、带着新参数重开一次。
    /// 断言会把这条路直接 panic 掉。所以锁键冲突只能在**有意的检查**里暴露，
    /// 例如 `Blocked { holder }` 里带上占用者，排查时能看出"被谁挡了"。
    pub fn admit(&mut self, run_id: RunId, key: MutexKey) -> Admission {
        if let Some(existing_key) = self.accepted.get(&run_id) {
            // 这里只做诊断，不参与判定（详见上面文档注释）
            let _ = existing_key;
            return Admission::DuplicateRunId(run_id);
        }

        if let Some(holder) = self.held.get(&key) {
            return Admission::Blocked {
                key: key.clone(),
                holder: holder.clone(),
            };
        }

        self.held.insert(key.clone(), run_id.clone());
        self.accepted.insert(run_id, key);
        Admission::Admitted
    }

    /// 运行结束，释放锁。
    ///
    /// **只释放锁、保留 runId 记录** —— 这是幂等的前提：
    /// 跑完之后拿同一个 runId 再来，应当得到 `DuplicateRunId`（复用结果），
    /// 而不是又跑一遍。把 `accepted` 也清掉就等于没有幂等。
    pub fn release(&mut self, run_id: &RunId) -> bool {
        let Some(key) = self.accepted.get(run_id) else {
            return false;
        };
        // 只有仍由它持有才移除，避免把别人的锁误删
        if self.held.get(key) == Some(run_id) {
            self.held.remove(key);
        }
        true
    }

    /// 当前正在跑的数量。
    pub fn running(&self) -> usize {
        self.held.len()
    }

    /// 该 runId 是否已被受理过（含已完成）。
    pub fn has_seen(&self, run_id: &RunId) -> bool {
        self.accepted.contains_key(run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::Random;

    /// 测试用 RNG。
    ///
    /// **不能用 `wrapping_mul/wrapping_add` 那类线性同余** —— 试过，会坏掉两个测试，
    /// 而且坏法很有迷惑性：`uuid_v4` 只取前 16 字节，线性的低位序列在头 16 字节里
    /// 几乎不动，两个"不同"的 runId 只在版本位那一字节上相差 1，
    /// 于是两次 `MutexKey::new` 算出了**同一个**锁键，报出来的是
    /// "同一 runId 两次用了不同的锁键"这种指向错误方向的失败。
    /// 这里改用 core 里那个 splitmix64 同款实现：8 字节一组混匀，
    /// 每个 runId 的 16 字节都被打散。
    struct SeededRng(u64);

    impl SeededRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
    }

    impl Random for SeededRng {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for chunk in dst.chunks_mut(8) {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                let bytes = z.to_be_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
    }

    fn rng() -> SeededRng {
        SeededRng::new(11)
    }

    /// 两个**互不相同**的 runId —— 顺手断言一下，免得将来换了 RNG 又踩回上面那个坑。
    ///
    /// 注意必须**共用一个 RNG 实例**：每次 `rng()` 都从同一个种子重建，
    /// 那样连续取两次会得到**同一个** runId（第一版的 `two_run_ids` 就是这么错的，
    /// 被这里的 `assert_ne!` 抓了出来）。
    fn two_run_ids() -> (RunId, RunId) {
        let mut r = rng();
        let a = RunId::new(&mut r);
        let b = RunId::new(&mut r);
        assert_ne!(a, b, "测试 RNG 必须能产出不同的 runId");
        (a, b)
    }

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

    // ── 互斥键 ──────────────────────────────────────────────────────────

    #[test]
    fn mutex_key_separates_capability_and_account() {
        let key = |cap: &str, acc: &str| MutexKey::new(cap, acc, "");
        assert_eq!(key("exchange", "acc-1").as_str(), "exchange#acc-1");
        assert_ne!(key("exchange", "acc-1"), key("exchange", "acc-2"));
        assert_ne!(key("exchange", "acc-1"), key("sign", "acc-1"));
    }

    /// **这条是本次修复的核心**：不同参数必须是不同的锁。
    ///
    /// 旧项目 key 不含参数，导致"换个 prizeId 再抢"拿回上一次的结果。
    #[test]
    fn different_params_are_different_locks() {
        let a = MutexKey::new("exchange", "acc-1", "digest-a");
        let b = MutexKey::new("exchange", "acc-1", "digest-b");
        assert_ne!(a, b, "不同参数不得共用一个锁（旧项目的 bug 就在这）");
    }

    #[test]
    fn empty_digest_degrades_explicitly() {
        // 显式声明"无参数维度"时才退化成两段式
        assert_eq!(
            MutexKey::new("sign", "acc-1", "").as_str(),
            "sign#acc-1",
            "空摘要应退化为 能力#账号"
        );
        assert_eq!(
            MutexKey::new("sign", "acc-1", "abc").as_str(),
            "sign#acc-1#abc"
        );
    }

    // ── 参数摘要 ────────────────────────────────────────────────────────

    #[test]
    fn params_digest_ignores_order() {
        // 这是"必须排序"的理由：HashMap 遍历顺序不保证，不排序会让同一组参数
        // 产出两个摘要，互斥直接失效
        let a = vec![
            ("prizeId".to_string(), "1001".to_string()),
            ("skipWait".to_string(), "true".to_string()),
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(
            params_digest(Some(&a)),
            params_digest(Some(&b)),
            "同一组参数换个顺序必须是同一个摘要"
        );
    }

    #[test]
    fn params_digest_distinguishes_values() {
        let a = vec![("prizeId".to_string(), "1001".to_string())];
        let b = vec![("prizeId".to_string(), "1002".to_string())];
        assert_ne!(params_digest(Some(&a)), params_digest(Some(&b)));
    }

    #[test]
    fn params_digest_is_not_ambiguous_across_boundaries() {
        // 不做长度前缀的话 ("ab","c") 与 ("a","bc") 会拼成同一个串
        let a = vec![("ab".to_string(), "c".to_string())];
        let b = vec![("a".to_string(), "bc".to_string())];
        assert_ne!(params_digest(Some(&a)), params_digest(Some(&b)));
    }

    #[test]
    fn params_digest_none_is_empty() {
        assert_eq!(params_digest(None), "");
        assert_eq!(params_digest(Some(&[])), "");
    }

    #[test]
    fn params_digest_hides_raw_values() {
        // 摘要要能安全进日志/URL —— 原文（可能是口令或奖品名）不得出现
        let secret = "0a1b2c3d4e5f".to_string();
        let d = params_digest(Some(&[("kouling".to_string(), secret.clone())]));
        assert!(!d.contains(&secret), "摘要里泄漏了参数原文");
        assert_eq!(d.len(), 64, "sha256 十六进制");
    }

    // ── 登记表（幂等 + 互斥一起判） ──────────────────────────────────────

    #[test]
    fn fresh_run_is_admitted_and_holds_lock() {
        let mut reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        let key = MutexKey::new("sign", "acc-1", "");

        assert_eq!(reg.admit(id.clone(), key), Admission::Admitted);
        assert_eq!(reg.running(), 1);
        assert!(reg.has_seen(&id));
    }

    #[test]
    fn same_run_id_replay_does_not_run_twice() {
        let mut reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        let key = MutexKey::new("sign", "acc-1", "");

        assert_eq!(reg.admit(id.clone(), key.clone()), Admission::Admitted);
        // 重放同一个 runId：即便锁已被自己持有，也必须报 Duplicate 而不是 Blocked
        assert_eq!(
            reg.admit(id.clone(), key.clone()),
            Admission::DuplicateRunId(id.clone())
        );
        assert_eq!(reg.running(), 1, "重放不得多占一个槽");
    }

    /// 关键：跑完释放锁之后，同 runId 重放**仍**要判重复。
    ///
    /// 否则"网络重试"会把已经做完的事再做一遍（而写操作没有幂等键）。
    #[test]
    fn replay_after_release_is_still_duplicate() {
        let mut reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        let key = MutexKey::new("sign", "acc-1", "");

        assert_eq!(reg.admit(id.clone(), key.clone()), Admission::Admitted);
        assert!(reg.release(&id));
        assert_eq!(reg.running(), 0, "锁已释放");

        assert_eq!(
            reg.admit(id.clone(), key),
            Admission::DuplicateRunId(id),
            "跑完之后同 runId 仍应判重复（幂等的前提）"
        );
    }

    #[test]
    fn second_run_id_same_params_is_blocked() {
        let mut reg = RunRegistry::new();
        let (id1, id2) = two_run_ids();
        let key = MutexKey::new("exchange", "acc-1", "same-params");

        assert_eq!(reg.admit(id1, key.clone()), Admission::Admitted);
        // 不同 runId、同一把锁 ⇒ 挡住（同一账号同一参数不能并发）
        assert!(matches!(reg.admit(id2, key), Admission::Blocked { .. }));
        assert_eq!(reg.running(), 1);
    }

    /// 不同参数可以并发 —— 这正是修复掉的那个 bug。
    #[test]
    fn different_params_may_run_concurrently() {
        let mut reg = RunRegistry::new();
        let (id1, id2) = two_run_ids();

        let key_a = MutexKey::new(
            "exchange",
            "acc-1",
            &params_digest(Some(&[("prizeId".into(), "1001".into())])),
        );
        let key_b = MutexKey::new(
            "exchange",
            "acc-1",
            &params_digest(Some(&[("prizeId".into(), "1002".into())])),
        );
        assert_ne!(key_a, key_b, "不同参数必须得到不同的锁键");

        assert_eq!(reg.admit(id1, key_a), Admission::Admitted);
        assert_eq!(
            reg.admit(id2, key_b),
            Admission::Admitted,
            "同一账号抢不同奖品不得互相挡 —— 旧项目就是在这里拿回了错的结果"
        );
        assert_eq!(reg.running(), 2, "两个不同参数的运行各占一把锁");
    }

    #[test]
    fn release_is_scoped_to_its_own_run() {
        let mut reg = RunRegistry::new();
        let (id1, id2) = two_run_ids();
        let key = MutexKey::new("sign", "acc-1", "");

        reg.admit(id1.clone(), key);
        // id2 没被受理过，release 应当什么都不做
        assert!(!reg.release(&id2));
        assert_eq!(reg.running(), 1, "不该误释放别人的锁");
    }

    #[test]
    fn release_twice_is_idempotent() {
        let mut reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        reg.admit(id.clone(), MutexKey::new("sign", "acc-1", ""));

        assert!(reg.release(&id));
        assert!(reg.release(&id), "重复释放不该报错");
        assert_eq!(reg.running(), 0);
    }
}
