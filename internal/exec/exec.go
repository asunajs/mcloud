// 执行器：并发闸、互斥键、run 准入、事件总线。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-exec/src/lib.rs。
//
// # 已落地的是**纯函数与纯内存部分**
//
// 并发语义、互斥键、run 准入、事件总线都不需要 IO，因此现在就能被测住，
// 而不是等到 P5 靠真机试跑验证。真正需要 IO 的部分（跑任务、落库、SSE 的
// HTTP 侧）在 P5 接 —— 本层只保证"给它事件，它按序号存好并交给订阅者"。
//
// # 这些类型是**进程级共享状态**，所以用指针接收者 + 内部一把锁
//
// 事件总线与运行登记表会被两条**并发**的路径同时碰到：跑任务的那条
// （发布事件 / 受理与释放）与看的那条（订阅 SSE、查"谁在跑"）。
// 若要求调用方持有独占引用，跑一次 20 个账号的签到期间，
// 服务层连"现在有几个在跑"都问不了。
// 所以每个方法短暂加锁；这不改变任何语义，因为这两个表的操作本来就是串行的。
package mcloudexec

import (
	"sort"
	"strconv"
	"strings"
	"sync"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 并发闸
// ─────────────────────────────────────────────────────────────────────────────

// Concurrency 是并发闸配置。
type Concurrency struct {
	// GlobalLimit 是全局同时在跑的任务上限。默认 2 —— 原项目要求默认串行防风控。
	GlobalLimit int
}

// DefaultConcurrency 返回默认配置（全局上限 2）。
//
// Go 没有 `Default` trait，所以显式给一个构造函数 —— 别在调用方写
// `Concurrency{GlobalLimit: 2}`，那样默认值就散落在各处了。
func DefaultConcurrency() Concurrency { return Concurrency{GlobalLimit: 2} }

// EffectiveParallelism 给定账号数与执行方式，算出实际并行度。
//
// 规则：
//   - [core.ModeSequential] → 恒为 1（逐个跑），这正是"默认串行防风控"的落点；
//   - [core.ModeParallel] → 取 `min(账号数, 全局上限)`，**永远不超过上限**；
//   - 账号数为 0 → 0（空集合不占用槽位）。
//
// ⚠ 未知的执行方式返回 **0**（一个都不跑）：这里**不能**退化成"按串行跑"，
// 否则新增一种 mode 而这里忘了加分支时，表现是"照常跑了"而不是"没跑" ——
// 而"没跑"会立刻被发现，"照常跑了"会一直错下去。
func (c Concurrency) EffectiveParallelism(accountCount int, mode core.AccountMode) int {
	switch mode {
	case core.ModeSequential:
		if accountCount > 0 {
			return 1
		}
		return 0
	case core.ModeParallel:
		limit := c.GlobalLimit
		if limit < 1 {
			limit = 1
		}
		if accountCount < limit {
			return accountCount
		}
		return limit
	}
	return 0
}

// CanStart 报告该账号是否可以立刻开始（对齐"同能力+账号互斥"的简化形式）。
//
// 与 [Concurrency.EffectiveParallelism] 保持一致地下限取 1：
// `GlobalLimit` 被写错成 0 时应当"只跑一个"而不是**彻底锁死**。
// 静默死锁比跑慢更难排查，所以这里选择可推进。
func (c Concurrency) CanStart(running int) bool {
	limit := c.GlobalLimit
	if limit == 0 {
		limit = 1
	}
	return running < limit
}

// ─────────────────────────────────────────────────────────────────────────────
// 互斥键
// ─────────────────────────────────────────────────────────────────────────────

// MutexKey 是互斥键：同一「能力 + 账号 + **参数摘要**」不允许并发。
//
// # 为什么参数维度不能省（这是全套代码里最危险的语义）
//
// 旧项目 `runner.ts` 的 key 是 `${task.name}:${accountLabel}` —— **不含参数**。
// 它的 join 语义是"已存在 promise 就直接返回同一个结果"，于是：
//
// > 同一账号带不同 `prizeId` 的第二次请求，**拿回的是第一次的结果**。
//
// 表现是"换了个奖品去抢，界面显示抢到了上一个奖品" —— 没有任何报错，
// 因为对旧代码而言这就是"同一个任务已经在跑，复用结果"。
//
// 所以新实现把参数进 key，让"不同参数"成为**不同的锁**而不是同一次运行。
// 这比"复用结果"更符合直觉，也不会静默给错答案。
//
// 用 `string` 的具名类型而不是结构体：它要能直接当 map 的键，
// 而结构体的可比较性要靠每个字段都可比较 —— 多一层没有收益。
type MutexKey string

// NewMutexKey 组装互斥键。
//
// `paramsDigest` 是调用方对**本次运行的参数**算出的稳定摘要（见 [ParamsDigest]）。
// 传空串表示"该能力确实没有参数维度"，此时行为退化为旧的「能力 + 账号」——
// 这是**显式**声明的，不是默认地漏掉。
func NewMutexKey(capability, account, paramsDigest string) MutexKey {
	if paramsDigest == "" {
		return MutexKey(capability + "#" + account)
	}
	return MutexKey(capability + "#" + account + "#" + paramsDigest)
}

// String 返回键的原文（日志、诊断信息用）。
func (k MutexKey) String() string { return string(k) }

// ParamsDigest 把一组参数压成一个短摘要，供 [MutexKey] 使用。
//
// # 为什么先排序再拼
//
// 参数来自 `map[string]any` 这类**无序**容器时，直接遍历会因顺序不同而
// 产出不同字符串 —— 那么"同一组参数"会被算成两个不同的锁，互斥就失效了
// （同一账号能并发跑两次）。
//
// ⚠ 这一条在 **Go 侧比 Rust 侧更危险**：Go 的 map 遍历顺序是**故意随机化**的，
// 所以"忘了排序"不是"换个编译器才出错"，而是**同一进程里两次调用就不一样**。
// Rust 侧是 `HashMap` 的遍历顺序"碰巧"稳定了一段时间才掩盖住这类问题，
// Go 侧连这个缓冲都没有。
//
// 摘要取哈希而不是原文，是为了让 [MutexKey] 能安全地进日志与 URL ——
// 参数里可能有奖品名、口令这类不该外泄的内容。
//
// `nil` 与空集合都返回空串（= 该能力没有参数维度）。**必须显式处理**：
// 否则 sha256("") 会返回一个 64 位十六进制串，"没有参数维度"就变成了
// "有一个固定参数"，与 [NewMutexKey] 里 `paramsDigest == ""` 的约定对不上。
func ParamsDigest(params map[string]string) string {
	if len(params) == 0 {
		return ""
	}

	keys := make([]string, 0, len(params))
	for k := range params {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	// 长度前缀分隔，避免 ("ab","c") 与 ("a","bc") 拼出同一个串
	var buf strings.Builder
	for _, k := range keys {
		v := params[k]
		buf.WriteString(strconv.Itoa(len(k)))
		buf.WriteByte(':')
		buf.WriteString(k)
		buf.WriteString(strconv.Itoa(len(v)))
		buf.WriteByte(':')
		buf.WriteString(v)
		buf.WriteByte('|')
	}

	return core.SHA256Hex([]byte(buf.String()))
}

// ─────────────────────────────────────────────────────────────────────────────
// 准入
// ─────────────────────────────────────────────────────────────────────────────

// AdmissionKind 是准入判定的三种结论。
//
// ⚠ `String()` 一律 switch + 末尾返回空串（枚举漏态守卫）：
// 写成 `if k == AdmissionBlocked { … }; return "admitted"` 会让**任何**越界值
// 都有名字，"已知态有几个"就再也数不出来了。
type AdmissionKind int

const (
	// AdmissionAdmitted 表示这个 runId 是新的，可以开跑。
	AdmissionAdmitted AdmissionKind = iota
	// AdmissionDuplicateRunID 表示同一 runId 已经跑过（或正在跑）——
	// 直接复用既有结果，不重复执行。
	AdmissionDuplicateRunID
	// AdmissionBlocked 表示另一个 runId 正握着同一把锁 ——
	// 这次**不能开跑，也不能假装成功**。
	AdmissionBlocked
)

// String 返回诊断用文本。未知值返回空串。
func (k AdmissionKind) String() string {
	switch k {
	case AdmissionAdmitted:
		return "admitted"
	case AdmissionDuplicateRunID:
		return "duplicate_run_id"
	case AdmissionBlocked:
		return "blocked"
	}
	return ""
}

// Admission 是一次运行的准入判定结果。
//
// 只保留"判断要不要真的开跑"所需的最小信息，**不含结果** ——
// 有了它就能回答这三选一，而结果由存储层回答。
//
// # 为什么是"结构体 + Kind"而不是三个类型
//
// Go 没有 sum type。用接口 + 三个实现能保住穷尽性的形状，但调用方每次都得
// 类型断言，而断言漏一个分支**不报错**。这里选择结构体，并把"哪些字段在哪种
// Kind 下有效"写在字段注释上 —— `Holder` 是**诊断信息**，不参与判定。
type Admission struct {
	Kind AdmissionKind
	// RunID 在 [AdmissionDuplicateRunID] 下有效：重复的那个 runId。
	RunID core.RunID
	// Key 在 [AdmissionBlocked] 下有效：被谁挡住的键。
	Key MutexKey
	// Holder 在 [AdmissionBlocked] 下有效：占用者的 runId。
	//
	// 排查时"被谁挡了"比"被挡了"有用得多。它是**诊断信息**，不参与判定。
	Holder core.RunID
}

// ─────────────────────────────────────────────────────────────────────────────
// 运行登记表
// ─────────────────────────────────────────────────────────────────────────────

// RunRegistry 把"runId 幂等"与"参数互斥"两件事放在一处判定。
//
// 这两件事必须**一起**判：只查 runId 会让不同参数被误判成重复，
// 只查锁会让同一 runId 重放跑两次。旧项目两个都没做。
//
// 这是纯内存实现（P5 会加落库的那一半：跨进程重放靠 `runs` 表）。
type RunRegistry struct {
	mu sync.Mutex
	// accepted 记录已受理的 runId 及其占用的锁。
	accepted map[core.RunID]MutexKey
	// held 记录锁 → 当前持有的 runId。
	held map[MutexKey]core.RunID
}

// NewRunRegistry 建一个空的登记表。
//
// ⚠ 零值 [RunRegistry] **不可用**（两个 map 都是 nil，写会 panic）：
// 必须走这个构造函数。这与 Rust 的 `Default` 不同，是 Go 侧要显式记住的一点。
func NewRunRegistry() *RunRegistry {
	return &RunRegistry{
		accepted: map[core.RunID]MutexKey{},
		held:     map[MutexKey]core.RunID{},
	}
}

// Admit 申请开跑。**这是唯一入口** —— 不要在外面自己判断后直接执行。
//
// 顺序很重要：**先查 runId、再查锁**。
// 反过来的话，重放同一个 runId 会先撞上自己持有的锁、
// 被判成 `Blocked` 而不是 `DuplicateRunId`，调用方就会以为"有人占着，等会儿再来"，
// 于是永远等不到 —— 因为占着它的正是它自己。
//
// # 为什么这里**不**校验两次传入的 key 相同
//
// 试过在重放分支上断言 `existingKey == key`，结论是**不能这么写**：
// 调用方若真把 runId 复用到别的参数上，它**必然**要在登记表已经记下这个 runId
// （也就是还锁着）时来问 —— 而那个场景正是这里要支持的：
// 先 `DuplicateRunId` 被拒 ⇒ 等下把拿到的结果丢掉、带着新参数重开一次。
// 断言会把这条路直接 panic 掉。所以锁键冲突只能在**有意的检查**里暴露，
// 例如 `Blocked` 里带上占用者，排查时能看出"被谁挡了"。
func (r *RunRegistry) Admit(runID core.RunID, key MutexKey) Admission {
	r.mu.Lock()
	defer r.mu.Unlock()

	if _, ok := r.accepted[runID]; ok {
		return Admission{Kind: AdmissionDuplicateRunID, RunID: runID}
	}

	if holder, ok := r.held[key]; ok {
		return Admission{Kind: AdmissionBlocked, Key: key, Holder: holder}
	}

	r.held[key] = runID
	r.accepted[runID] = key
	return Admission{Kind: AdmissionAdmitted}
}

// Release 运行结束，释放锁。
//
// **只释放锁、保留 runId 记录** —— 这是幂等的前提：
// 跑完之后拿同一个 runId 再来，应当得到 [AdmissionDuplicateRunID]（复用结果），
// 而不是又跑一遍。把 `accepted` 也清掉就等于没有幂等。
func (r *RunRegistry) Release(runID core.RunID) bool {
	r.mu.Lock()
	defer r.mu.Unlock()

	key, ok := r.accepted[runID]
	if !ok {
		return false
	}
	// 只有仍由它持有才移除，避免把别人的锁误删
	if r.held[key] == runID {
		delete(r.held, key)
	}
	return true
}

// Running 返回当前正在跑的数量。
func (r *RunRegistry) Running() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.held)
}

// HasSeen 报告该 runId 是否已被受理过（含已完成）。
func (r *RunRegistry) HasSeen(runID core.RunID) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	_, ok := r.accepted[runID]
	return ok
}

// ─────────────────────────────────────────────────────────────────────────────
// 事件总线
// ─────────────────────────────────────────────────────────────────────────────

// DefaultSubscriberCapacity 是每个订阅者的默认队列上限。
//
// 取值理由：一个 run 的事件通常几十到几百条；留 1024 足够让一个**正常**的
// SSE 消费者（按序读、偶尔慢一拍）不丢东西，同时把"彻底卡死"的连接限制在
// 有限内存内。真丢的时候 [Delivery.Dropped] 会说出来，不会静默。
const DefaultSubscriberCapacity = 1024

// Delivery 是单条订阅者的投递结果。
//
// 慢订阅者**不阻塞**发布者 —— 这是刻意的：
// 一个卡住的 SSE 连接不该让别人的任务停在半路。
type Delivery struct {
	// Delivered 是这次真的投出去几条。
	Delivered int
	// Dropped 是因为订阅者队列满而**丢弃**几条。
	//
	// 丢的不是"最老的"而是"最新的"（见 [EventBus.Publish]）——
	// 保留最老的，是为了让订阅者至少知道"这个 run 从哪儿开始、发生了什么"；
	// 丢掉最老的会让前端从中间开始，看到的进度是断的。
	Dropped int
}

type subscriber struct {
	queue []core.Sequenced
	// capacity 是队列上限 —— 满了就丢新来的，并计入 dropped。
	capacity int
	dropped  int
}

// runStream 是某个 run 的事件流。
//
// 同时承担两个职责：**广播**（给在线的订阅者）与**回放**（给刚连上的订阅者）。
// 这两件事必须在一起，否则会出现旧项目那种症状：
// 前端在 run 开始后才打开页面，于是**先前的日志全丢了**。
type runStream struct {
	// history 保留全部已发布事件，供回放与续传。
	//
	// P5 落库之后这里可以改成"只保留内存里的尾部 + 从 `run_events` 表读更早的"；
	// 现在先全留，因为一个 run 的事件量在几百条量级。
	history []core.Sequenced
	// subscribers 是 id → 待投递队列。
	subscribers map[uint64]*subscriber
	// seq 是序号游标。
	seq core.Seq
}

// EventBus 是 run 事件总线：发布、回放、续传。
//
// # 为什么不用"一个全局广播通道"
//
// 最接近的现成答案（无缓冲广播）有三个不合用的地方：
//
//  1. **它只在在线时有效**：订阅者迟到了就收不到之前的事件。
//     而主用例恰恰是"用户打开页面看一个正在跑的 run"，
//     甚至"run 已经跑完了才来看历史"。
//  2. **它不保证跨订阅者一致**：环形缓冲是按"最慢的订阅者"推进的，
//     一个卡住的订阅者会让别人丢事件 —— 而 SSE 客户端卡住是常态（手机切后台）。
//  3. **它把序号语义留给了调用方**，而序号正是 SSE 续传的锚点。
//
// 这里换成"每个订阅者一条队列 + 全量历史"：迟到的订阅者能从历史补齐，
// 卡住的订阅者只影响自己（丢的事件数会被报出来），序号由总线统一分配。
//
// # 没有通知机制
//
// 订阅方取事件是**拉**（[EventBus.Drain] 非阻塞）而不是被推送：
// 本层是纯层，不引入运行时/调度器。调用方自己决定拉取节奏 ——
// HTTP 侧是每 50ms 拉一次内存队列（不查库），见 `internal/server` 的 runs 路由。
type EventBus struct {
	mu               sync.Mutex
	streams          map[core.RunID]*runStream
	nextSubscriberID uint64
}

// NewEventBus 建一个空总线。
//
// ⚠ 零值 [EventBus] **不可用**（map 是 nil），必须走这个构造函数。
func NewEventBus() *EventBus {
	return &EventBus{streams: map[core.RunID]*runStream{}}
}

func (b *EventBus) stream(runID core.RunID) *runStream {
	s, ok := b.streams[runID]
	if !ok {
		s = &runStream{subscribers: map[uint64]*subscriber{}}
		b.streams[runID] = s
	}
	return s
}

// Open 开始跟踪一个 run。重复调用是安全的（幂等）。
//
// 返回该 run 当前的序号游标。
func (b *EventBus) Open(runID core.RunID) core.Seq {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.stream(runID).seq
}

// Publish 发布一条事件，返回被分配的序号与每个订阅者的投递结果。
//
// 序号由总线统一分配 —— 调用方**不要**自己算，
// 否则并发发布时会撞号，而撞号的后果是客户端永久跳过某个事件。
//
// # 投递策略：队列满时丢**最新**的
//
// 不是丢最老的。理由：前端至少要能从这个 run 的开头看起。
// 丢最老的会让它从中间开始，进度条与日志都对不上；
// 丢最新的则只是"后面几条没看到"，且下次续传时能从序号接上。
func (b *EventBus) Publish(event core.RunEvent) (core.Seq, []Delivery) {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream := b.stream(event.RunID)
	seq := stream.seq.Next()
	item := core.NewSequenced(seq, event)
	stream.history = append(stream.history, item)

	deliveries := make([]Delivery, 0, len(stream.subscribers))
	for _, sub := range stream.subscribers {
		if len(sub.queue) < sub.capacity {
			sub.queue = append(sub.queue, item)
			deliveries = append(deliveries, Delivery{Delivered: 1})
		} else {
			sub.dropped++
			deliveries = append(deliveries, Delivery{Dropped: 1})
		}
	}
	return seq, deliveries
}

// Subscribe 订阅一个 run 的事件。
//
// `since` = 客户端已收到的最后一条序号（SSE 的 `Last-Event-ID`），
// 零值表示"从头"。
//
// ⚠ **`since` 是"已看到的值"，不是"下一个要取的值"**：传 [core.SeqSeen](3)
// 会补发 4、5、…；传 [core.SeqResumeAfter](3)（内部值 4）会从 **5** 开始 ——
// 那会静默跳过 4 号。两个构造器的名字就是为区分这件事而存在的。
func (b *EventBus) Subscribe(runID core.RunID, since core.Seq) uint64 {
	return b.SubscribeWithCapacity(runID, since, DefaultSubscriberCapacity)
}

// SubscribeWithCapacity 同 [EventBus.Subscribe]，但显式指定队列上限
// （测试"丢事件"时用得上）。
func (b *EventBus) SubscribeWithCapacity(runID core.RunID, since core.Seq, capacity int) uint64 {
	if capacity < 0 {
		capacity = 0
	}
	b.mu.Lock()
	defer b.mu.Unlock()

	id := b.nextSubscriberID
	b.nextSubscriberID++

	stream := b.stream(runID)
	sub := &subscriber{capacity: capacity}
	// 回放：把 since 之后的历史直接灌进队列
	for _, item := range stream.history {
		if item.Seq.Value() <= since.Value() {
			continue
		}
		if len(sub.queue) < sub.capacity {
			sub.queue = append(sub.queue, item)
		} else {
			// 连回放都装不下：这条订阅者从一开始就跟不上
			sub.dropped++
		}
	}
	stream.subscribers[id] = sub
	return id
}

// Drain 取走订阅者当前所有待投递事件（非阻塞）。
func (b *EventBus) Drain(runID core.RunID, subscriberID uint64) []core.Sequenced {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return nil
	}
	sub, ok := stream.subscribers[subscriberID]
	if !ok {
		return nil
	}
	out := sub.queue
	sub.queue = nil
	return out
}

// Dropped 返回该订阅者累计被丢了多少条。
//
// 暴露它是为了**让丢事件可见** —— 旧项目所有投递失败都吞成日志，
// 于是"前端少了半截日志"从来没人发现。
//
// ⚠ 这是**累计值，不自动清零**：调用方看到它大于 0 之后要处理一次，
// 而"清零"的办法是重新订阅（[EventBus.Unsubscribe] + [EventBus.Subscribe]）——
// 重订阅会把 `since` 之后的历史再灌一遍，丢掉的那些因此补齐。
func (b *EventBus) Dropped(runID core.RunID, subscriberID uint64) int {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return 0
	}
	sub, ok := stream.subscribers[subscriberID]
	if !ok {
		return 0
	}
	return sub.dropped
}

// Unsubscribe 取消订阅。
func (b *EventBus) Unsubscribe(runID core.RunID, subscriberID uint64) bool {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return false
	}
	_, existed := stream.subscribers[subscriberID]
	delete(stream.subscribers, subscriberID)
	return existed
}

// Len 返回该 run 已发布的事件条数。
func (b *EventBus) Len(runID core.RunID) int {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return 0
	}
	return len(stream.history)
}

// IsEmpty 报告该 run 是否还没有任何事件。
func (b *EventBus) IsEmpty(runID core.RunID) bool { return b.Len(runID) == 0 }

// Replay 回放某 run 从 `since` 之后的所有事件（不建订阅）。
//
// 给"run 已结束、用户来看历史"这条路径用 —— 那时没有在线订阅者，
// 也不需要订阅，直接读一遍即可。
func (b *EventBus) Replay(runID core.RunID, since core.Seq) []core.Sequenced {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return nil
	}
	out := make([]core.Sequenced, 0, len(stream.history))
	for _, item := range stream.history {
		if item.Seq.Value() > since.Value() {
			out = append(out, item)
		}
	}
	return out
}

// SubscriberCount 返回当前订阅者数量。
func (b *EventBus) SubscriberCount(runID core.RunID) int {
	b.mu.Lock()
	defer b.mu.Unlock()

	stream, ok := b.streams[runID]
	if !ok {
		return 0
	}
	return len(stream.subscribers)
}

// Forget 忘掉一个 run 的全部事件与订阅者。
//
// **只在 run 的事件已落库之后调**（P5）：落库前清掉等于把历史丢了。
func (b *EventBus) Forget(runID core.RunID) bool {
	b.mu.Lock()
	defer b.mu.Unlock()

	_, ok := b.streams[runID]
	delete(b.streams, runID)
	return ok
}

// Log 是日志事件的便捷构造 + 发布。
//
// 单独给一个方法是因为"打日志"是最高频的调用，
// 每次手写 `core.NewEventLog(...)` 会诱使调用方自己去拼字符串。
func (b *EventBus) Log(runID core.RunID, level core.LogLevel, message string) core.Seq {
	seq, _ := b.Publish(core.NewEventLog(runID, level, message))
	return seq
}
