//! 事件中枢：序号分配 + 内存重放 + 日志落盘 + SSE 广播。
//!
//! # 为什么不能只用 `broadcast`
//!
//! `tokio::sync::broadcast` 是"只有在线才收得到"的通道：
//!
//! 1. **迟到的订阅者拿不到历史** —— 而"运行已经开始、用户才打开页面"
//!    正是最常见的场景，更别说"运行早就结束了，用户来看日志"。
//! 2. **订阅者之间互相影响** —— 环形缓冲按**最慢**的订阅者推进，
//!    一个卡住的客户端会让其他人丢事件。而卡住的 SSE 客户端是常态
//!    （手机切后台、网络抖动）。
//! 3. **序号语义得自己维护** —— 而序号正是断线续传的锚点。
//!
//! 所以这里是"**每个 run 一份内存历史 + 每订阅者一条独立队列**"，
//! 而不是一个全局广播。
//!
//! # 双源
//!
//! 事件同时进内存历史与存储层：
//! - 内存历史 = **在线**那一半，刚发布的事件不必等落盘（落盘是攒批的）
//! - 存储层 = **持久**那一半，重启后、迟到者、断线重连都靠它
//!
//! 两者缺一不可：只发内存则刷新页面就丢历史，只落库则前端得轮询。
//!
//! # 顺序：先订阅，再读库
//!
//! 这是**必须**的顺序。订阅会把 `since` 之后的内存历史补进队列，
//! 于是"读库"与"订阅"之间那个窗口里发布的事件不会两头都漏掉。
//! 反过来的话，那个窗口里的事件既不在已读的库里、也不在补发的队列里。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcloud_core::{
    log_line_of, AppendOutcome, EventSink, LogLine, RunEvent, RunId, Seq, SeqAllocator, Sequenced,
    Store,
};
use tokio::sync::broadcast;

use crate::run_labels::{trace_event, RunLabels};

/// 每个 run 在内存里保留的事件条数。
///
/// 与日志上限一致：内存里留的和磁盘上存的是同一批东西，
/// 让"迟到的订阅者看到的"和"重新加载后看到的"不会不一样。
pub const HISTORY_PER_RUN: usize = mcloud_core::LOG_LIMIT_PER_RUN;

/// 每个订阅者的队列容量。
///
/// 满了**丢最新的**（见 [`EventHub::publish`]）。
pub const SUBSCRIBER_CAPACITY: usize = 1024;

/// 终态 run 的内存历史**最多留几份**。
///
/// 为什么必须有上限（2026-10-03 review §1.5）：`Inner::runs` 这张表原先
/// **只进不出** —— 一次运行的内存历史最多 [`HISTORY_PER_RUN`]（= 2000）条事件，
/// 而它**永远不释放**；`forget` 与 `unsubscribe` 在生产路径上**一处都没接**
/// （`grep -rn "hub\.unsubscribe\|hub\.forget" crates` 当时只剩测试）。
/// 服务跑得越久，堆着的终态历史越多（粗算 0.5 MB / 次运行）。
///
/// 留 8 份而不是"终态就立刻回收"：终局那一刻前端可能正在重连（页面刷新、
/// 断线续传），那些请求要能从内存历史里补上"订阅之前发生的事"。
/// 8 份够覆盖这个窗口，同时把常驻量钉在"8 次运行的历史"这个量级。
///
/// 更老的终态 run 在 [`EventHub::flush`] 收尾时回收 —— 那一刻日志已落盘，
/// 前端看历史走 `/api/runs/{id}`（读存储层），不再依赖内存。
pub const MAX_TERMINAL_RUNS_KEPT: usize = 8;

/// 同一条 run 最多允许多少个订阅者（2026-10-06 安全审计 `SEC-17`）。
///
/// # 为什么要有上限
///
/// 一条 SSE 订阅 = 一个常驻任务 + 一份队列，而**它不受 HTTP 层并发上限约束**
///（`ConcurrencyLimitLayer` 的名额在 handler 返回响应那一刻就还回去了，而事件流
/// 在那之后还要活很久）。不设上限的话，"开一堆连接订阅同一条 run"就是一条
/// 免认证？不 —— 订阅要登录；但**一个已登录用户**照样能把进程的内存吃光 ✗。
///
/// 32 的依据：正常使用是"一个页面一条流"（外加断线重连的短暂重叠），
/// 32 条覆盖"多标签页 + 多设备 + 重连"绰绰有余，又远小于"能把内存吃光"的量级 ✓。
/// 与 [`MAX_TERMINAL_RUNS_KEPT`] 一起，把订阅者总数钉在
/// `32 × (在跑的 + 8)` 这个量级 ✓。
pub const MAX_SUBSCRIBERS_PER_RUN: usize = 32;

/// [`EventHub::subscribe`] 失败的原因。
///
/// ⚠ 两个原因是**不同的事**，调用方要分开处理：前者（内存里没有）应当从存储层
/// 回放终局，后者（人太多）应当回一个明确的拒绝 —— 混成一个 `None`
/// 会让"订阅一条已结束的 run"和"订阅者满了"变成同一种表现 ✗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeError {
    /// 内存里**没有**这条 run：它要么已结束且历史被回收，要么压根没发布过事件。
    /// 调用方应当改走存储层（发一条终局事件然后收尾）。
    NotTracked,
    /// 这条 run 的订阅者已达 [`MAX_SUBSCRIBERS_PER_RUN`]。
    TooManySubscribers,
}

/// 一个 run 的内存态。
#[derive(Debug, Default)]
struct RunState {
    /// 序号分配器。
    seq: SeqAllocator,
    /// 已发布的事件（含序号），按序。
    history: Vec<Sequenced>,
    /// 订阅者：id → 队列。
    subscribers: HashMap<u64, Subscriber>,
    /// 是否已结束。结束后可被 [`EventHub::forget`] 回收。
    terminal: bool,
    /// 变终态的**先后**（进程内单调递增）。
    ///
    /// ⚠ 不能用 `seq` 排序：每个 run 的序号**各从 1 开始**，跨 run 不可比。
    /// 也不能用时钟：测试里是 `FixedClock`，同一毫秒全相等 ⇒ 排序结果随机、
    /// 用例会闪。所以单独一个计数器。
    terminal_order: Option<u64>,
}

#[derive(Debug)]
struct Subscriber {
    queue: Vec<Sequenced>,
    /// 因队列满而丢弃的条数。**累计且不自动清零**。
    dropped: usize,
}

/// 一次拉取的结果。
///
/// ⚠ `PartialEq` 而**不是** `Eq`：它裹着 [`Sequenced`]，后者带
/// [`RunEvent::Details`]（`serde_json::Value` 只实现 `PartialEq`）。
/// 理由与代价见 `RunEvent` 的说明。
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub events: Vec<Sequenced>,
    /// 本次（以及此前累计）丢了多少条。
    pub dropped: usize,
}

/// 事件中枢。
pub struct EventHub {
    inner: Mutex<Inner>,
    store: Arc<dyn Store>,
    /// 用于唤醒正在等待的 SSE 任务，避免忙轮询。
    notify: broadcast::Sender<RunId>,
    /// `run_id → 账号`：让**每一行**日志都认得出主人（见 [`crate::run_labels`]）。
    labels: RunLabels,
}

#[derive(Default)]
struct Inner {
    runs: HashMap<RunId, RunState>,
    next_subscriber: u64,
    /// 待落盘的日志缓冲：`run_id → lines`。
    ///
    /// 攒批而不是每条一次落盘：一次运行会产生几十到几百条日志，
    /// 每条都整体重写文件是不可接受的。
    pending: HashMap<RunId, Vec<LogLine>>,
    /// 终态顺序号的分配器（见 [`RunState::terminal_order`]）。
    terminal_counter: u64,
}

impl EventHub {
    /// ⚠ 2026-10-09 起不再要 `Clock`：落盘那一行的时刻取**事件自己的 `at_ms`**
    /// （见 `mcloud_core::log_line_of` ✓）—— 以前用"中枢收到事件那一刻"的钟，
    /// 而攒批是 2 秒一次、`Details` 之类的构造点又不带时刻 ⇒ 只能写 0 ✗。
    /// 事件本来就带着准确的时刻（`TaskContext::log` 打的 ✓），用它更准 ✓。
    pub fn new(store: Arc<dyn Store>) -> Self {
        let (notify, _) = broadcast::channel(256);
        Self {
            inner: Mutex::new(Inner::default()),
            store,
            notify,
            labels: RunLabels::default(),
        }
    }

    /// 锁中毒时取回内部值继续用。
    ///
    /// 与"直接 panic"相反：这里每次操作都在锁内一次做完，没有
    /// "写了一半"的中间态可被读到，所以中毒不构成不可用的理由 ——
    /// 而跟着 panic 会让整个服务停摆。
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 订阅一个 run 的事件。
    ///
    /// `since` 是"客户端**已经看到**的序号"（用 [`Seq::seen`] 构造）。
    /// 返回订阅者 id，调用方随后用 [`EventHub::drain`] 拉取。
    ///
    /// # ⚠⚠ 2026-10-06 安全审计 `SEC-17`：这里**不再**凭空建条目
    ///
    /// 旧实现是 `inner.runs.entry(run_id).or_default()` —— 订阅一条**内存里没有**
    /// 的 run（已结束 + 历史被 `evict_terminal_runs` 回收）会建出一个
    /// **非终态**空条目 ⇒ 之后 `drain` 永远空、通知永远等不到 ⇒ 那条 SSE
    /// **永不结束**（客户端只能一直收 keep-alive）✗；而且这个空条目
    /// **永远不会被回收**（`evict_terminal_runs` 只收 `terminal` 的）⇒
    /// 反复订阅不存在的 run 就是一条内存泄漏 ✗。
    ///
    /// 现在返回 [`SubscribeError::NotTracked`]，由调用方决定"从存储层回放终局" ✓。
    /// 判存在与建订阅**在同一把锁内**完成，所以不存在"查完就被回收"的窗口 ✓。
    pub fn subscribe(&self, run_id: RunId, since: Seq) -> Result<u64, SubscribeError> {
        self.subscribe_inner(run_id, since, false)
    }

    /// 订阅一个**已经确认在跑、但还没发布第一条事件**的 run（审计 `SEC-17` 的配套）。
    ///
    /// # 为什么必须有这条路
    ///
    /// `POST /api/runs` 在 **spawn 运行任务之前**就把归属登记进 `run_owners`
    ///（`runs.rs` 的 `note_run_owner`）⇒ 客户端可能在"归属已经可判"到
    /// "第一条 `started` 事件到达"之间订阅。那个窗口里 [`EventHub::subscribe`]
    /// 会说 `NotTracked`，而**存储层也还没有记录**（运行记录是**结束时**才写的）
    /// ⇒ 调用方若据此"回放终局"就会得到 404，把一条正在跑的流判成不存在 ✗。
    ///
    /// ⚠⚠ 它**只该**在调用方已经确认"这条 run 真的在跑"时用（`run_owners` 有它、
    /// 且存储层还没有记录）。拿它去订一条不存在的 run 就退回了旧实现那个
    /// "永不结束 + 永不回收"的坑 ✗ —— 所以两条路是两个名字，不是一个 `bool` 参数
    /// 藏在默认值里 ✓。
    pub fn subscribe_pending(&self, run_id: RunId, since: Seq) -> Result<u64, SubscribeError> {
        self.subscribe_inner(run_id, since, true)
    }

    fn subscribe_inner(
        &self,
        run_id: RunId,
        since: Seq,
        create: bool,
    ) -> Result<u64, SubscribeError> {
        let mut inner = self.lock();

        // ⚠ 顺序有讲究：`id` 要在**拿到 `state` 之前**取。
        // `state` 是对 `inner.runs` 的可变借用，而 `inner.next_subscriber` 是
        // 另一个字段 —— 直接写两个字段本来允许（不相交借用），但中间夹一个
        // `if/else` + `?` 之后借用区间的推断就变紧了（实测 E0502/E0499）。
        // 先取 id 就绕开了这件事，语义完全一样 ✓。
        // 代价：`TooManySubscribers` 那条早退会白耗一个 id —— id 只是个
        // 单调递增的编号，跳号无害 ✓。
        let id = inner.next_subscriber;
        inner.next_subscriber = inner.next_subscriber.wrapping_add(1);

        let state = if create {
            inner.runs.entry(run_id).or_default()
        } else {
            inner
                .runs
                .get_mut(&run_id)
                .ok_or(SubscribeError::NotTracked)?
        };
        if state.subscribers.len() >= MAX_SUBSCRIBERS_PER_RUN {
            return Err(SubscribeError::TooManySubscribers);
        }

        // 把 since 之后的历史补进队列 —— 这样"订阅之前就已发布的事件"
        // 不会因为订阅动作本身而丢掉
        let backlog: Vec<Sequenced> = state
            .history
            .iter()
            .filter(|s| s.seq > since)
            .cloned()
            .collect();
        state.subscribers.insert(
            id,
            Subscriber {
                queue: backlog,
                dropped: 0,
            },
        );
        Ok(id)
    }

    pub fn unsubscribe(&self, run_id: RunId, subscriber: u64) {
        let mut inner = self.lock();
        if let Some(state) = inner.runs.get_mut(&run_id) {
            state.subscribers.remove(&subscriber);
        }
    }

    /// 非阻塞地取走某订阅者队列里的全部事件。
    pub fn drain(&self, run_id: RunId, subscriber: u64) -> Delivery {
        let mut inner = self.lock();
        match inner
            .runs
            .get_mut(&run_id)
            .and_then(|s| s.subscribers.get_mut(&subscriber))
        {
            Some(sub) => Delivery {
                events: std::mem::take(&mut sub.queue),
                dropped: sub.dropped,
            },
            None => Delivery {
                events: Vec::new(),
                dropped: 0,
            },
        }
    }

    /// 等待新事件。用于替代忙轮询。
    pub fn waiter(&self) -> broadcast::Receiver<RunId> {
        self.notify.subscribe()
    }

    /// 已结束的 run 是否还在内存里。
    pub fn is_tracked(&self, run_id: RunId) -> bool {
        self.lock().runs.contains_key(&run_id)
    }

    /// 重放某个 run 的全部（或 `since` 之后的）事件。
    ///
    /// 给"运行已结束、用户来看历史"这条路径用。
    pub fn replay(&self, run_id: RunId, since: Seq) -> Vec<Sequenced> {
        let inner = self.lock();
        inner
            .runs
            .get(&run_id)
            .map(|s| {
                s.history
                    .iter()
                    .filter(|e| e.seq > since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 回收一个 run 的内存态。
    ///
    /// **只在事件已落盘之后调用** —— 否则内存里没了、盘上也没有，
    /// 那段历史就真的丢了。
    ///
    /// ⚠ 生产路径**不需要**手动调它：终态 run 由 [`EventHub::flush`] 里的
    /// [`EventHub::evict_terminal_runs`] 按上限自动回收（2026-10-03 review §1.5）。
    /// 这个方法留着给"明确知道这条已经没用了"的场合（测试与将来的强制清理）。
    pub fn forget(&self, run_id: RunId) {
        let mut inner = self.lock();
        inner.runs.remove(&run_id);
        inner.pending.remove(&run_id);
        // ⚠ 账号那份映射也要一起收（否则长跑进程里"跑过的账号"只增不减 ✗）。
        // 注意：`Finished` 那条日志**先打、后忘**（见 `run_labels::trace_event` ✓），
        // 所以这里再忘一次是幂等的兜底 ✓。
        self.labels.forget(run_id);
    }

    /// 订阅者数量（诊断用）。
    pub fn subscriber_count(&self, run_id: RunId) -> usize {
        self.lock()
            .runs
            .get(&run_id)
            .map(|s| s.subscribers.len())
            .unwrap_or(0)
    }

    /// 分配序号、记入内存历史、投递给订阅者，并把日志放进待落盘缓冲。
    ///
    /// 返回分配到的序号。
    fn publish(&self, event: RunEvent) -> Seq {
        let run_id = event.run_id();
        let is_terminal = matches!(event, RunEvent::Finished { .. });

        let seq = {
            let mut inner = self.lock();
            // ⚠ 顺序号先取到局部变量：`state` 借的是 `inner.runs`，
            // 之后再碰 `inner` 的其它字段就会 E0502（实测踩过）。
            let order = inner.terminal_counter;
            let state = inner.runs.entry(run_id).or_default();
            let seq = state.seq.next();
            let item = Sequenced::new(seq, event.clone());

            state.history.push(item.clone());
            if state.history.len() > HISTORY_PER_RUN {
                let excess = state.history.len() - HISTORY_PER_RUN;
                state.history.drain(..excess);
            }
            if is_terminal {
                state.terminal = true;
                state.terminal_order = Some(order);
            }

            // 投递给每个订阅者。队列满 ⇒ **丢最新的**：
            // 前端至少要能从这个 run 的开头看起。丢最老的会让它从中间开始，
            // 进度条与日志都对不上；丢最新的只是"后面几条没看到"，
            // 而且下次续传能从序号接上。
            for sub in state.subscribers.values_mut() {
                if sub.queue.len() >= SUBSCRIBER_CAPACITY {
                    sub.dropped += 1;
                    continue;
                }
                sub.queue.push(item.clone());
            }

            // 日志进待落盘缓冲（`Log` 与 `Progress` 都进 —— 判据只有一份，
            // 见 `mcloud_core::log_line_of` ✓）。
            if let Some(line) = log_line_of(&event, seq) {
                inner.pending.entry(run_id).or_default().push(line);
            }

            if is_terminal {
                inner.terminal_counter = inner.terminal_counter.saturating_add(1);
            }

            seq
        };

        // 唤醒等待者。容量满（没有等待者）时返回 Err，属正常。
        let _ = self.notify.send(run_id);
        seq
    }

    /// 回收**多余的终态 run**，只留最近 [`MAX_TERMINAL_RUNS_KEPT`] 份。
    ///
    /// 三个条件全满足才回收，缺一条要么丢数据、要么丢功能：
    /// - `terminal`：**还在跑的绝不回收**（它的事件只有内存里有，盘上还没有）；
    /// - `subscribers.is_empty()`：还有人在看这条流 —— 回收会让它当场断在半路；
    /// - `pending` 里没有它的日志：日志还没落盘就回收 = **真丢**（[`EventHub::forget`]
    ///   连待落盘缓冲一起删，而落盘失败的那批会被放回缓冲等重试）。
    ///
    /// 由 [`EventHub::flush`] 在末尾调用：那个循环每 2 秒跑一次（`main.rs`），
    /// 所以回收既不额外起任务，也总是发生在"日志已经落盘"之后。
    fn evict_terminal_runs(&self) {
        let mut inner = self.lock();
        // 拆开借：`runs` 要遍历、`pending` 要查，而闭包里不能整份借 `inner`
        let Inner {
            runs,
            pending,
            terminal_counter: _,
            next_subscriber: _,
        } = &mut *inner;
        let mut candidates: Vec<(RunId, u64)> = runs
            .iter()
            .filter(|(id, s)| {
                s.terminal
                    && s.subscribers.is_empty()
                    && pending.get(*id).is_none_or(|p| p.is_empty())
            })
            .filter_map(|(id, s)| s.terminal_order.map(|order| (*id, order)))
            .collect();
        if candidates.len() <= MAX_TERMINAL_RUNS_KEPT {
            return;
        }
        // 早变终态的先走
        candidates.sort_unstable_by_key(|(_, order)| *order);
        let excess = candidates.len() - MAX_TERMINAL_RUNS_KEPT;
        for (id, _) in candidates.into_iter().take(excess) {
            runs.remove(&id);
            // ⚠ 只删 `runs`，**不**删 `pending`：走到这里已经保证它是空的
            // （非空的话上面那道 filter 就把它挡掉了）。
        }
    }

    /// 把待落盘的日志刷进存储层。
    ///
    /// 攒批到**每个 run** 的边界，不是每条一次、也不是全部结束才写：
    /// 最坏情况下丢掉的是一个 run 的日志，而不是整次运行的。
    pub async fn flush(&self) -> mcloud_core::Result<usize> {
        let batches: Vec<(RunId, Vec<LogLine>)> = {
            let mut inner = self.lock();
            inner.pending.drain().collect()
        };
        let mut total = 0;
        for (run_id, lines) in batches {
            if lines.is_empty() {
                continue;
            }
            match self.store.append_logs(run_id, &lines).await {
                Ok(AppendOutcome::Appended) => total += lines.len(),
                Ok(AppendOutcome::Trimmed) => {
                    // 截断必须报出来：静默丢弃会让人以为任务只跑到一半就停了
                    tracing::warn!(run = %run_id, "日志超出上限，已丢弃最旧的");
                    total += lines.len();
                }
                Err(e) => {
                    // 落盘失败**不能**影响正在跑的任务：日志是观测手段，
                    // 不是业务结果。放回缓冲等下次重试。
                    tracing::error!(run = %run_id, error = %e, "日志落盘失败，将重试");
                    let mut inner = self.lock();
                    inner.pending.entry(run_id).or_default().splice(0..0, lines);
                }
            }
        }
        // 收尾时顺手回收多余的终态 run（见 `evict_terminal_runs`）。
        // 放在 `flush` 里是因为它的前提正是"日志已经落盘"，而 flush 每 2 秒跑一次。
        self.evict_terminal_runs();
        Ok(total)
    }
}

impl EventSink for EventHub {
    fn emit(&self, event: RunEvent) {
        // 出去之前先过一道脱敏（复核意见 §1.14）：
        // 这一路会把任务的日志/结论原样带出去（进程日志 / SSE / 推送 / 落盘），
        // 而上游回显里可能就有一枚凭证 ✗。形状由 mcloud-core 给死 ⇒ sink 只要一行 ✓。
        let event = event.scrubbed();
        // 顺手打到 tracing，便于无订阅者时也能从进程日志追溯。
        // ⚠ 走共用的 [`trace_event`]（另一个 sink 也用它 ✓）：它会顺手把
        // "这个 run 是哪个账号"盖到**每一行**上 —— 否则服务端日志页上除第一行之外
        // 全是"没主的行"，几个账号轮流跑时根本认不出谁是谁（2026-10-05 用户报 ✓）。
        trace_event(&self.labels, &event);
        self.publish(event);
    }
}

impl std::fmt::Debug for EventHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("EventHub")
            .field("tracked_runs", &inner.runs.len())
            .field("pending_log_runs", &inner.pending.len())
            .finish()
    }
}

#[cfg(test)]
mod tests;
