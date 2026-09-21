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
//! 已落地的是**纯函数与纯内存部分**：并发语义、互斥键、run 准入、
//! 运行生命周期、事件总线。这些都不需要 IO，
//! 因此现在就能被测住，而不是等到 P5 靠真机试跑验证。
//! 真正需要 IO 的部分（跑任务、落库、SSE 的 HTTP 侧）在 P5 接 ——
//! 本层只保证"给它事件，它按序号存好并交给订阅者"。
//!
//! # 这些类型是**进程级共享状态**，所以按 `&self` 暴露
//!
//! 事件总线与运行登记表会被两条**并发**的路径同时碰到：跑任务的那条
//! （发布事件 / 受理与释放）与看那条（订阅 SSE、查"谁在跑"）。
//! 收 `&mut self` 就要求调用方把独占借用握到整趟运行结束 ——
//! 跑一次 20 个账号的签到期间，服务层连"现在有几个在跑"都问不了。
//! 所以内部各有一把锁、方法收 `&self`；每次操作短暂加锁不改变任何语义，
//! 因为这两个表的操作本来就是串行的。这也意味着本 crate **不引入 tokio**：
//! 锁是 `std::sync::Mutex`，拉取是调用方的事（SSE 侧见 `mcloud-server`）。

#![forbid(unsafe_code)]

use mcloud_core::{AccountMode, LogLevel, RunEvent, RunId, Seq, Sequenced};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

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
///
/// 方法收 `&self`（内部一把锁），理由见 crate 头。
#[derive(Debug, Default)]
pub struct RunRegistry {
    inner: Mutex<RegistryState>,
}

/// [`RunRegistry`] 的全部可变状态。
///
/// 单独一个结构体而不是直接 `Mutex<RunRegistry>`：后者会让 `&self` 的方法
/// 里再借一次 `self`，绕不开。
#[derive(Debug, Default)]
struct RegistryState {
    /// 已受理的 runId 及其占用的锁。
    accepted: HashMap<RunId, MutexKey>,
    /// 锁 → 当前持有的 runId。
    held: HashMap<MutexKey, RunId>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取内部状态。
    ///
    /// 中毒（持锁线程 panic）时**按原值继续**而不是跟着 panic：这两个 map
    /// 的每次操作都在锁内一次做完，没有"写了一半"的中间态可被读到，
    /// 所以中毒在这里不构成不可用的理由 —— 而跟着 panic 会让整个服务停摆。
    fn state(&self) -> MutexGuard<'_, RegistryState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    pub fn admit(&self, run_id: RunId, key: MutexKey) -> Admission {
        let mut state = self.state();
        if let Some(existing_key) = state.accepted.get(&run_id) {
            // 这里只做诊断，不参与判定（详见上面文档注释）
            let _ = existing_key;
            return Admission::DuplicateRunId(run_id);
        }

        if let Some(holder) = state.held.get(&key) {
            return Admission::Blocked {
                key: key.clone(),
                holder: holder.clone(),
            };
        }

        state.held.insert(key.clone(), run_id.clone());
        state.accepted.insert(run_id, key);
        Admission::Admitted
    }

    /// 运行结束，释放锁。
    ///
    /// **只释放锁、保留 runId 记录** —— 这是幂等的前提：
    /// 跑完之后拿同一个 runId 再来，应当得到 `DuplicateRunId`（复用结果），
    /// 而不是又跑一遍。把 `accepted` 也清掉就等于没有幂等。
    pub fn release(&self, run_id: &RunId) -> bool {
        let mut state = self.state();
        // 先把键拷出来再动 `held`：不这样的话"读 accepted、改 held"
        // 会同时借这个结构体的两处，虽然编译器允许（字段级拆分），
        // 但读的人得先想一遍才确认它没问题。
        let Some(key) = state.accepted.get(run_id).cloned() else {
            return false;
        };
        // 只有仍由它持有才移除，避免把别人的锁误删
        if state.held.get(&key) == Some(run_id) {
            state.held.remove(&key);
        }
        true
    }

    /// 当前正在跑的数量。
    pub fn running(&self) -> usize {
        self.state().held.len()
    }

    /// 该 runId 是否已被受理过（含已完成）。
    pub fn has_seen(&self, run_id: &RunId) -> bool {
        self.state().accepted.contains_key(run_id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 事件总线
// ─────────────────────────────────────────────────────────────────────────────

/// 单条订阅者的投递结果。
///
/// 慢订阅者**不阻塞**发布者 —— 这是刻意的：
/// 一个卡住的 SSE 连接不该让别人的任务停在半路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    /// 这次真的投出去几条。
    pub delivered: usize,
    /// 因为订阅者队列满而**丢弃**几条。
    ///
    /// 丢的不是"最老的"而是"最新的"（见 [`EventBus::publish`]）——
    /// 保留最老的，是为了让订阅者至少知道"这个 run 从哪儿开始、发生了什么"；
    /// 丢掉最老的会让前端从中间开始，看到的进度是断的。
    pub dropped: usize,
}

/// 某个 run 的事件流。
///
/// 同时承担两个职责：**广播**（给在线的订阅者）与**回放**（给刚连上的订阅者）。
/// 这两件事必须在一起，否则会出现旧项目那种症状：
/// 前端在 run 开始后才打开页面，于是**先前的日志全丢了**。
#[derive(Debug, Default)]
struct RunStream {
    /// 已发布的事件（保留全部，供回放与续传）。
    ///
    /// P5 落库之后这里可以改成"只保留内存里的尾部 + 从 `run_events` 表读更早的"；
    /// 现在先全留，因为一个 run 的事件量在几百条量级。
    history: Vec<Sequenced>,
    /// 订阅者：id → 待投递队列。
    subscribers: HashMap<u64, Subscriber>,
    /// 序号游标。
    seq: Seq,
}

#[derive(Debug)]
struct Subscriber {
    /// 还没被取走的队列。
    queue: Vec<Sequenced>,
    /// 队列上限 —— 满了就丢新来的，并计入 `dropped`。
    capacity: usize,
    dropped: usize,
}

impl Subscriber {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Vec::new(),
            capacity,
            dropped: 0,
        }
    }
}

/// 每个订阅者的默认队列上限。
///
/// 取值理由：一个 run 的事件通常几十到几百条；留 1024 足够让一个**正常**的
/// SSE 消费者（按序读、偶尔慢一拍）不丢东西，同时把"彻底卡死"的连接限制在
/// 有限内存内。真丢的时候 [`Delivery::dropped`] 会说出来，不会静默。
const DEFAULT_SUBSCRIBER_CAPACITY: usize = 1024;

/// run 事件总线：发布、回放、续传。
///
/// # 为什么不用 `tokio::sync::broadcast`
///
/// 那个类型是最接近的现成答案，但它有三个不合用的地方：
///
/// 1. **它只在在线时有效**：订阅者迟到了就收不到之前的事件。
///    而我们的主用例恰恰是"用户打开页面看一个正在跑的 run"，
///    甚至"run 已经跑完了才来看历史"。
/// 2. **它不保证跨订阅者一致**：`broadcast` 的环形缓冲是按"最慢的订阅者"推进的，
///    一个卡住的订阅者会让别人丢事件 —— 而 SSE 客户端卡住是常态（手机切后台）。
/// 3. **它把序号语义留给了调用方**，而序号正是 SSE 续传的锚点。
///
/// 这里换成"每个订阅者一条队列 + 全量历史"：迟到的订阅者能从历史补齐，
/// 卡住的订阅者只影响自己（丢的事件数会被报出来），序号由总线统一分配。
///
/// # 方法收 `&self`（内部一把锁），且**没有通知机制**
///
/// 发布方与订阅方是两条并发的路径（跑任务的那条发，SSE 那条收），
/// 所以它必须是可共享的：内部一把锁，每次操作短暂加锁。理由见 crate 头。
///
/// 订阅方取事件是**拉**（[`EventBus::drain`] 非阻塞）而不是被推送：
/// 本 crate 不依赖 tokio，没有 `Notify` 可用。调用方自己决定拉取节奏 ——
/// HTTP 侧是每 50ms 拉一次内存队列（不查库），见 `mcloud-server` 的 `routes/runs.rs`。
#[derive(Debug, Default)]
pub struct EventBus {
    inner: Mutex<BusState>,
}

/// [`EventBus`] 的全部可变状态（单独成结构体的理由同 `RegistryState`）。
#[derive(Debug, Default)]
struct BusState {
    streams: HashMap<RunId, RunStream>,
    next_subscriber_id: u64,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取内部状态，中毒时按原值继续（理由同 [`RunRegistry::state`]：
    /// 每次操作都在锁内一次做完，没有中间态可读，而跟着 panic 代价更大）。
    fn state(&self) -> MutexGuard<'_, BusState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 开始跟踪一个 run。重复调用是安全的（幂等）。
    ///
    /// 返回该 run 当前的序号游标，供调用方把它交给 [`mcloud_core::stage::RunExecution`]
    /// 之外的发布逻辑使用。
    pub fn open(&self, run_id: &RunId) -> Seq {
        self.state().streams.entry(run_id.clone()).or_default().seq
    }

    /// 发布一条事件，返回被分配的序号。
    ///
    /// 序号由总线统一分配 —— 调用方**不要**自己算，
    /// 否则并发发布时会撞号，而撞号的后果是客户端永久跳过某个事件。
    ///
    /// # 投递策略：队列满时丢**最新**的
    ///
    /// 不是丢最老的。理由：前端至少要能从这个 run 的开头看起。
    /// 丢最老的会让它从中间开始，进度条与日志都对不上；
    /// 丢最新的则只是"后面几条没看到"，且下次续传时能从序号接上。
    pub fn publish(&self, event: RunEvent) -> (Seq, Vec<Delivery>) {
        let run_id = event.run_id().clone();
        let mut state = self.state();
        let stream = state.streams.entry(run_id).or_default();
        let seq = stream.seq.next();
        let item = Sequenced::new(seq, event);
        stream.history.push(item.clone());

        let mut deliveries = Vec::with_capacity(stream.subscribers.len());
        for sub in stream.subscribers.values_mut() {
            if sub.queue.len() < sub.capacity {
                sub.queue.push(item.clone());
                deliveries.push(Delivery {
                    delivered: 1,
                    dropped: 0,
                });
            } else {
                sub.dropped += 1;
                deliveries.push(Delivery {
                    delivered: 0,
                    dropped: 1,
                });
            }
        }

        (seq, deliveries)
    }

    /// 订阅一个 run 的事件。
    ///
    /// `since` = 客户端已收到的最后一条序号（SSE 的 `Last-Event-ID`），
    /// 传 `None` 或 `Seq::start()` 表示"从头"。
    ///
    /// ⚠ **`since` 是"已看到的值"，不是"下一个要取的值"**：传 `Seq::seen(3)`
    /// 会补发 4、5、…；传 `Seq::resume_after(3)`（内部值 4）会从 **5** 开始 ——
    /// 那会静默跳过 4 号。两个构造器的名字就是为区分这件事而存在的。
    pub fn subscribe(&self, run_id: &RunId, since: Option<Seq>) -> u64 {
        self.subscribe_with_capacity(run_id, since, DEFAULT_SUBSCRIBER_CAPACITY)
    }

    /// 同上，但显式指定队列上限（测试"丢事件"时用得上）。
    pub fn subscribe_with_capacity(
        &self,
        run_id: &RunId,
        since: Option<Seq>,
        capacity: usize,
    ) -> u64 {
        let from = since.map_or(0, |s| s.value());
        let mut state = self.state();

        // 先取 id 再借 `streams`：`entry()` 拿的是 `state` 的可变借用，
        // 而计数器也在 `state` 上 —— 先读后借，两者才不打架（行为完全一样）。
        let id = state.next_subscriber_id;
        state.next_subscriber_id += 1;

        let stream = state.streams.entry(run_id.clone()).or_default();

        let mut sub = Subscriber::new(capacity);
        // 回放：把 since 之后的历史直接灌进队列
        for item in stream.history.iter().filter(|i| i.seq.value() > from) {
            if sub.queue.len() < sub.capacity {
                sub.queue.push(item.clone());
            } else {
                // 连回放都装不下：这条订阅者从一开始就跟不上
                sub.dropped += 1;
            }
        }

        stream.subscribers.insert(id, sub);
        id
    }

    /// 取走订阅者当前所有待投递事件（非阻塞）。
    pub fn drain(&self, run_id: &RunId, subscriber: u64) -> Vec<Sequenced> {
        let mut state = self.state();
        let Some(stream) = state.streams.get_mut(run_id) else {
            return Vec::new();
        };
        let Some(sub) = stream.subscribers.get_mut(&subscriber) else {
            return Vec::new();
        };
        std::mem::take(&mut sub.queue)
    }

    /// 该订阅者累计被丢了多少条。
    ///
    /// 暴露它是为了**让丢事件可见** —— 旧项目所有投递失败都吞成日志，
    /// 于是"前端少了半截日志"从来没人发现。
    ///
    /// ⚠ 这是**累计值，不自动清零**：调用方看到它大于 0 之后要处理一次，
    /// 而"清零"的办法是重新订阅（`unsubscribe` + `subscribe`）——
    /// 重订阅会把 `since` 之后的历史再灌一遍，丢掉的那些因此补齐。
    pub fn dropped(&self, run_id: &RunId, subscriber: u64) -> usize {
        self.state()
            .streams
            .get(run_id)
            .and_then(|s| s.subscribers.get(&subscriber))
            .map_or(0, |s| s.dropped)
    }

    /// 取消订阅。
    pub fn unsubscribe(&self, run_id: &RunId, subscriber: u64) -> bool {
        self.state()
            .streams
            .get_mut(run_id)
            .is_some_and(|s| s.subscribers.remove(&subscriber).is_some())
    }

    /// 该 run 已发布的事件条数。
    pub fn len(&self, run_id: &RunId) -> usize {
        self.state()
            .streams
            .get(run_id)
            .map_or(0, |s| s.history.len())
    }

    pub fn is_empty(&self, run_id: &RunId) -> bool {
        self.len(run_id) == 0
    }

    /// 回放某 run 从 `since` 之后的所有事件（不建订阅）。
    ///
    /// 给"run 已结束、用户来看历史"这条路径用 —— 那时没有在线订阅者，
    /// 也不需要订阅，直接读一遍即可。
    pub fn replay(&self, run_id: &RunId, since: Option<Seq>) -> Vec<Sequenced> {
        let from = since.map_or(0, |s| s.value());
        self.state()
            .streams
            .get(run_id)
            .map(|s| {
                s.history
                    .iter()
                    .filter(|i| i.seq.value() > from)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 当前订阅者数量。
    pub fn subscriber_count(&self, run_id: &RunId) -> usize {
        self.state()
            .streams
            .get(run_id)
            .map_or(0, |s| s.subscribers.len())
    }

    /// 忘掉一个 run 的全部事件与订阅者。
    ///
    /// **只在 run 的事件已落库之后调**（P5）：
    /// 落库前清掉等于把历史丢了。
    pub fn forget(&self, run_id: &RunId) -> bool {
        self.state().streams.remove(run_id).is_some()
    }

    /// 日志事件的便捷构造 + 发布。
    ///
    /// 单独给一个方法是因为"打日志"是最高频的调用，
    /// 每次手写 `RunEvent::log(...)` 会诱使调用方自己去拼字符串。
    pub fn log(&self, run_id: &RunId, level: LogLevel, message: impl Into<String>) -> Seq {
        let (seq, _) = self.publish(RunEvent::log(run_id.clone(), level, message));
        seq
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
        let reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        let key = MutexKey::new("sign", "acc-1", "");

        assert_eq!(reg.admit(id.clone(), key), Admission::Admitted);
        assert_eq!(reg.running(), 1);
        assert!(reg.has_seen(&id));
    }

    #[test]
    fn same_run_id_replay_does_not_run_twice() {
        let reg = RunRegistry::new();
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
        let reg = RunRegistry::new();
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
        let reg = RunRegistry::new();
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
        let reg = RunRegistry::new();
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
        let reg = RunRegistry::new();
        let (id1, id2) = two_run_ids();
        let key = MutexKey::new("sign", "acc-1", "");

        reg.admit(id1.clone(), key);
        // id2 没被受理过，release 应当什么都不做
        assert!(!reg.release(&id2));
        assert_eq!(reg.running(), 1, "不该误释放别人的锁");
    }

    #[test]
    fn release_twice_is_idempotent() {
        let reg = RunRegistry::new();
        let id = RunId::new(&mut rng());
        reg.admit(id.clone(), MutexKey::new("sign", "acc-1", ""));

        assert!(reg.release(&id));
        assert!(reg.release(&id), "重复释放不该报错");
        assert_eq!(reg.running(), 0);
    }

    // ── 事件总线 ────────────────────────────────────────────────────────────

    #[test]
    fn seq_is_assigned_by_the_bus() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        let (s1, _) = bus.publish(RunEvent::started(id.clone(), "sign"));
        let (s2, _) = bus.publish(RunEvent::progress(id.clone(), 1, 3));
        let (s3, _) = bus.publish(RunEvent::log(id.clone(), LogLevel::Info, "hi"));
        assert_eq!((s1.value(), s2.value(), s3.value()), (1, 2, 3));
        assert_eq!(bus.len(&id), 3);
    }

    /// 迟到的订阅者必须能看全 —— 旧项目这里会丢事件。
    #[test]
    fn late_subscriber_receives_earlier_events() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        bus.publish(RunEvent::started(id.clone(), "sign"));
        bus.publish(RunEvent::progress(id.clone(), 1, 2));

        // 事件已经发完了才订阅（用户打开页面时 run 早就开始了）
        let sub = bus.subscribe(&id, None);
        let got = bus.drain(&id, sub);
        assert_eq!(got.len(), 2, "迟到的订阅者应补齐历史");
        assert!(got[0].event.is_started());
        assert_eq!(got[1].seq.value(), 2);
    }

    /// 断线重连：从 `Last-Event-ID` 之后续传，不重复也不跳号。
    #[test]
    fn reconnect_resumes_after_last_seen() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        for i in 0..5 {
            bus.publish(RunEvent::progress(id.clone(), i, 5));
        }
        // 客户端说"我收到 3 号了" —— `since` 是**已看到的值**（`Seq::seen`），
        // 所以 3 号不重发，从 4 号接着来
        let sub = bus.subscribe(&id, Some(Seq::seen(3)));
        let got = bus.drain(&id, sub);
        assert_eq!(got.len(), 2, "只该补 4、5 两条");
        assert_eq!(got[0].seq.value(), 4);
        assert_eq!(got[1].seq.value(), 5);
    }

    /// 在线订阅者按发布顺序收到。
    #[test]
    fn live_subscriber_receives_in_order() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        let sub = bus.subscribe(&id, None);

        bus.publish(RunEvent::started(id.clone(), "sign"));
        bus.publish(RunEvent::progress(id.clone(), 1, 2));
        bus.publish(RunEvent::finished(
            id.clone(),
            mcloud_core::RunOutcome::Success,
            &[],
            None,
        ));

        let got = bus.drain(&id, sub);
        let seqs: Vec<u64> = got.iter().map(|i| i.seq.value()).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        assert!(got[2].event.is_terminal());
    }

    /// 慢订阅者**不能阻塞**发布者 —— 卡住的 SSE 连接不该拖住别人的任务。
    #[test]
    fn slow_subscriber_does_not_block_the_publisher() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        // 队列只容 2 条
        let slow = bus.subscribe_with_capacity(&id, None, 2);
        let healthy = bus.subscribe(&id, None);

        for i in 0..5 {
            let (_, deliveries) = bus.publish(RunEvent::progress(id.clone(), i, 5));
            // 发布永远成功，不因某人满而失败
            assert_eq!(deliveries.len(), 2, "两个订阅者都该收到一次投递结果");
        }

        // 慢的那位丢了 3 条（5 发、容 2），且**被记下来**而不是静默
        assert_eq!(bus.dropped(&id, slow), 3);
        let slow_got = bus.drain(&id, slow);
        assert_eq!(slow_got.len(), 2, "满队列只留得下 2 条");
        // 丢的是**最新**的：留下的应是 1、2 号，不是 4、5 号。
        // 前端至少能从开头看起，进度语义不断。
        assert_eq!(slow_got[0].seq.value(), 1);
        assert_eq!(slow_got[1].seq.value(), 2);

        // 正常的订阅者一条不丢
        assert_eq!(bus.dropped(&id, healthy), 0);
        assert_eq!(bus.drain(&id, healthy).len(), 5);
    }

    #[test]
    fn ended_run_can_still_be_replayed() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        bus.publish(RunEvent::started(id.clone(), "sign"));
        bus.publish(RunEvent::finished(
            id.clone(),
            mcloud_core::RunOutcome::ManualRequired,
            &[],
            None,
        ));

        // 没有订阅者，直接回放（"用户来看历史"这条路径）
        let all = bus.replay(&id, None);
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[1].event.outcome(),
            Some(mcloud_core::RunOutcome::ManualRequired),
            "回放要保住四态判定，不能压成成功"
        );
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        let sub = bus.subscribe(&id, None);
        assert_eq!(bus.subscriber_count(&id), 1);

        assert!(bus.unsubscribe(&id, sub));
        assert_eq!(bus.subscriber_count(&id), 0);
        assert!(!bus.unsubscribe(&id, sub), "重复退订应报 false");

        // 退订后再发布不该 panic，也不该给已退订的人投递
        let (_, deliveries) = bus.publish(RunEvent::started(id.clone(), "sign"));
        assert!(deliveries.is_empty());
    }

    #[test]
    fn streams_are_isolated_per_run() {
        let bus = EventBus::new();
        let (a, b) = two_run_ids();
        bus.publish(RunEvent::started(a.clone(), "sign"));
        bus.publish(RunEvent::started(b.clone(), "exchange"));
        bus.publish(RunEvent::progress(b.clone(), 1, 2));

        assert_eq!(bus.len(&a), 1);
        assert_eq!(bus.len(&b), 2);
        assert_eq!(bus.replay(&a, None).len(), 1);
    }

    #[test]
    fn unknown_run_yields_empty_everything() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        assert!(bus.is_empty(&id));
        assert!(bus.replay(&id, None).is_empty());
        assert_eq!(bus.subscriber_count(&id), 0);
        // 对不存在的 run drain 也不该 panic
        assert!(bus.drain(&id, 42).is_empty());
        assert!(!bus.forget(&id));
    }

    /// 序号在同一个 run 内**严格递增**，且与历史里的排列一致 ——
    /// 这是续传能成立的前提。
    #[test]
    fn seq_is_strictly_increasing_and_matches_history() {
        let bus = EventBus::new();
        let id = RunId::new(&mut rng());
        for i in 0..20 {
            bus.publish(RunEvent::progress(id.clone(), i, 20));
        }
        let history = bus.replay(&id, None);
        let seqs: Vec<u64> = history.iter().map(|i| i.seq.value()).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "序号不递增: {seqs:?}");

        // 从任意位置续传，拿到的应恰好是它之后那些，且不重不漏。
        //
        // 用 `Seq::seen(last)`（"我已经看到 last 了"）而不是 `resume_after(last)`
        // （"下一个要取 last+1"）：`since` 参数的语义是**前者**。
        // 传后者会让过滤条件 `> last+1` 把 `last+1` 自己也跳过 —— 静默少一条。
        for last in [1u64, 7, 19] {
            let after = bus.replay(&id, Some(mcloud_core::Seq::seen(last)));
            let expected = 20usize - last as usize;
            assert_eq!(after.len(), expected, "seen({last}) 条数不对");
            assert_eq!(
                after.first().map(|i| i.seq.value()),
                Some(last + 1),
                "续传应从 last+1 开始"
            );
        }

        // 边界：`last = 20`（已追平）→ 空，而不是报错或重发
        assert!(
            bus.replay(&id, Some(mcloud_core::Seq::seen(20))).is_empty(),
            "追平之后没有可补的"
        );
        // 边界：`None` 与 `seen(0)` 等价（0 号从不发出）
        assert_eq!(bus.replay(&id, None).len(), 20);
        assert_eq!(bus.replay(&id, Some(mcloud_core::Seq::seen(0))).len(), 20);
    }
}
