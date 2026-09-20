// 执行器的测试。
//
// 对应 Rust 侧 legacy/crates/mcloud-exec/src/lib.rs 的 `#[cfg(test)]` 模块，
// **外加**两条 Go 侧才成立的断言（map 遍历顺序随机化、并发安全），见文件末尾。
package mcloudexec

import (
	"strings"
	"sync"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// seededRng 是 exec 侧测试用的随机源：splitmix64，种子 11。
//
// ⚠ **不能用 `s*31+7` 那类线性同余** —— 试过（core 的 run 测试用的就是它），
// 会坏掉两个测试，而且坏法很有迷惑性：`UUIDv4` 只取前 16 字节，
// 线性的低位序列在头 16 字节里几乎不动，两个"不同"的 runId 只在版本位那一字节上
// 相差 1，于是两次 `NewMutexKey` 算出了**同一个**锁键，报出来的是
// "同一 runId 两次用了不同的锁键"这种指向错误方向的失败。
//
// 这里用 splitmix64：8 字节一组混匀，每个 runId 的 16 字节都被打散。
//
// ⚠ 别就近复用别的包那份 RNG：Rust 侧**每个模块各自定义** `SeededRng`，
// 算法并不相同（见 go-migration-plan.md §6.1 #19）。
type seededRng struct{ s uint64 }

func newSeededRng() *seededRng { return &seededRng{s: 11} }

func (r *seededRng) FillBytes(dst []byte) {
	for chunkStart := 0; chunkStart < len(dst); chunkStart += 8 {
		r.s += 0x9E3779B97F4A7C15
		z := r.s
		z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
		z = (z ^ (z >> 27)) * 0x94D049BB133111EB
		z ^= z >> 31
		bytes := [8]byte{
			byte(z >> 56), byte(z >> 48), byte(z >> 40), byte(z >> 32),
			byte(z >> 24), byte(z >> 16), byte(z >> 8), byte(z),
		}
		end := chunkStart + 8
		if end > len(dst) {
			end = len(dst)
		}
		copy(dst[chunkStart:end], bytes[:])
	}
}

// twoRunIDs 给出两个**互不相同**的 runId —— 顺手断言一下，
// 免得将来换了 RNG 又踩回上面那个坑。
//
// 注意必须**共用一个 RNG 实例**：每次 `newSeededRng()` 都从同一个种子重建，
// 那样连续取两次会得到**同一个** runId（第一版就是这么错的）。
func twoRunIDs(t *testing.T) (core.RunID, core.RunID) {
	t.Helper()
	r := newSeededRng()
	a := core.NewRunID(r)
	b := core.NewRunID(r)
	if a == b {
		t.Fatal("测试 RNG 必须能产出不同的 runId")
	}
	return a, b
}

// ── 并发闸 ──────────────────────────────────────────────────────────────────

func TestSequentialNeverParallelizes(t *testing.T) {
	c := DefaultConcurrency()
	for _, count := range []int{1, 2, 5, 100} {
		if got := c.EffectiveParallelism(count, core.ModeSequential); got != 1 {
			t.Errorf("串行模式下 %d 个账号也必须逐个跑，实得 %d", count, got)
		}
	}
}

func TestParallelIsCappedByGlobalLimit(t *testing.T) {
	c := Concurrency{GlobalLimit: 2}
	if got := c.EffectiveParallelism(1, core.ModeParallel); got != 1 {
		t.Errorf("1 个账号：实得 %d", got)
	}
	if got := c.EffectiveParallelism(2, core.ModeParallel); got != 2 {
		t.Errorf("2 个账号：实得 %d", got)
	}
	if got := c.EffectiveParallelism(10, core.ModeParallel); got != 2 {
		t.Errorf("并行也不得突破全局上限，实得 %d", got)
	}
}

func TestEmptyAccountSetUsesNoSlot(t *testing.T) {
	c := DefaultConcurrency()
	if got := c.EffectiveParallelism(0, core.ModeSequential); got != 0 {
		t.Errorf("串行 + 0 账号：实得 %d", got)
	}
	if got := c.EffectiveParallelism(0, core.ModeParallel); got != 0 {
		t.Errorf("并行 + 0 账号：实得 %d", got)
	}
}

func TestZeroGlobalLimitStillAllowsOne(t *testing.T) {
	// 配置写错成 0 时不应把系统锁死
	c := Concurrency{GlobalLimit: 0}
	if got := c.EffectiveParallelism(3, core.ModeParallel); got != 1 {
		t.Errorf("实得 %d", got)
	}
	if !c.CanStart(0) {
		t.Error("上限为 0 时也应当允许跑一个")
	}
}

func TestUnknownAccountModeRunsNothing(t *testing.T) {
	// Go 没有穷尽性检查：新增一种 mode 而这里忘了加分支时，
	// 表现必须是"没跑"（立刻被发现），不能是"照常跑了"。
	c := DefaultConcurrency()
	if got := c.EffectiveParallelism(3, core.AccountMode(99)); got != 0 {
		t.Errorf("未知执行方式应返回 0，实得 %d", got)
	}
}

func TestAdmissionKindTruthTable(t *testing.T) {
	// 枚举漏态守卫：已知态有几个，由"String() 是否返回空串"数出来。
	known := 0
	for k := 0; k < 8; k++ {
		if AdmissionKind(k).String() != "" {
			known++
		}
	}
	if known != 3 {
		t.Errorf("已知准入结论应为 3 个，实得 %d —— 新增一种结论就要同步 String()", known)
	}
}

// ── 互斥键 ──────────────────────────────────────────────────────────────────

func TestMutexKeySeparatesCapabilityAndAccount(t *testing.T) {
	key := func(cap, acc string) MutexKey { return NewMutexKey(cap, acc, "") }
	if got := key("exchange", "acc-1").String(); got != "exchange#acc-1" {
		t.Errorf("实得 %q", got)
	}
	if key("exchange", "acc-1") == key("exchange", "acc-2") {
		t.Error("不同账号必须是不同的锁")
	}
	if key("exchange", "acc-1") == key("sign", "acc-1") {
		t.Error("不同能力必须是不同的锁")
	}
}

// TestDifferentParamsAreDifferentLocks 是本次修复的核心：
// 旧项目 key 不含参数，导致"换个 prizeId 再抢"拿回上一次的结果。
func TestDifferentParamsAreDifferentLocks(t *testing.T) {
	a := NewMutexKey("exchange", "acc-1", "digest-a")
	b := NewMutexKey("exchange", "acc-1", "digest-b")
	if a == b {
		t.Error("不同参数不得共用一个锁（旧项目的 bug 就在这）")
	}
}

func TestEmptyDigestDegradesExplicitly(t *testing.T) {
	if got := NewMutexKey("sign", "acc-1", "").String(); got != "sign#acc-1" {
		t.Errorf("空摘要应退化为 能力#账号，实得 %q", got)
	}
	if got := NewMutexKey("sign", "acc-1", "abc").String(); got != "sign#acc-1#abc" {
		t.Errorf("实得 %q", got)
	}
}

// ── 参数摘要 ────────────────────────────────────────────────────────────────

func TestParamsDigestIgnoresOrder(t *testing.T) {
	a := ParamsDigest(map[string]string{"prizeId": "1001", "skipWait": "true"})
	b := ParamsDigest(map[string]string{"skipWait": "true", "prizeId": "1001"})
	if a != b {
		t.Errorf("同一组参数换个顺序必须是同一个摘要：%s vs %s", a, b)
	}
}

// TestParamsDigestIsStableAcrossCalls 是 **Go 侧专属**的那条。
//
// Rust 侧"必须排序"的理由是 `HashMap` 遍历顺序不保证；而 **Go 的 map 遍历顺序
// 是故意随机化的** —— 忘了排序不是"换个编译器才出错"，
// 而是**同一进程里两次调用就不一样**。所以这里连算 50 次。
//
// ⚠ 反向测试：把 `sort.Strings` 去掉之后这条**必须变红**（已实测）。
func TestParamsDigestIsStableAcrossCalls(t *testing.T) {
	params := map[string]string{"prizeId": "1001", "skipWait": "true", "mode": "fast"}
	want := ParamsDigest(params)
	for i := 0; i < 50; i++ {
		if got := ParamsDigest(params); got != want {
			t.Fatalf("第 %d 次算出了不同的摘要：%s vs %s（多半是忘了排序）", i, got, want)
		}
	}
}

func TestParamsDigestDistinguishesValues(t *testing.T) {
	a := ParamsDigest(map[string]string{"prizeId": "1001"})
	b := ParamsDigest(map[string]string{"prizeId": "1002"})
	if a == b {
		t.Error("不同取值必须是不同的摘要")
	}
}

func TestParamsDigestIsNotAmbiguousAcrossBoundaries(t *testing.T) {
	// 不做长度前缀的话 ("ab","c") 与 ("a","bc") 会拼成同一个串
	a := ParamsDigest(map[string]string{"ab": "c"})
	b := ParamsDigest(map[string]string{"a": "bc"})
	if a == b {
		t.Error("缺长度前缀会让两组不同参数拼成同一个串")
	}
}

func TestParamsDigestEmptyIsEmptyString(t *testing.T) {
	if got := ParamsDigest(nil); got != "" {
		t.Errorf("nil 应为空串，实得 %q", got)
	}
	if got := ParamsDigest(map[string]string{}); got != "" {
		t.Errorf("空集合应为空串，实得 %q", got)
	}
}

func TestParamsDigestHidesRawValues(t *testing.T) {
	// 摘要要能安全进日志/URL —— 原文（可能是口令或奖品名）不得出现
	secret := "0a1b2c3d4e5f"
	d := ParamsDigest(map[string]string{"kouling": secret})
	if strings.Contains(d, secret) {
		t.Error("摘要里泄漏了参数原文")
	}
	if len(d) != 64 {
		t.Errorf("sha256 十六进制应为 64 字符，实得 %d", len(d))
	}
}

// ── 登记表（幂等 + 互斥一起判） ──────────────────────────────────────────────

func TestFreshRunIsAdmittedAndHoldsLock(t *testing.T) {
	reg := NewRunRegistry()
	id := core.NewRunID(newSeededRng())
	key := NewMutexKey("sign", "acc-1", "")

	if got := reg.Admit(id, key); got.Kind != AdmissionAdmitted {
		t.Fatalf("实得 %v", got.Kind)
	}
	if reg.Running() != 1 {
		t.Errorf("实得 %d", reg.Running())
	}
	if !reg.HasSeen(id) {
		t.Error("受理过的 runId 应能被查到")
	}
}

func TestSameRunIDReplayDoesNotRunTwice(t *testing.T) {
	reg := NewRunRegistry()
	id := core.NewRunID(newSeededRng())
	key := NewMutexKey("sign", "acc-1", "")

	if got := reg.Admit(id, key); got.Kind != AdmissionAdmitted {
		t.Fatalf("实得 %v", got.Kind)
	}
	// 重放同一个 runId：即便锁已被自己持有，也必须报 Duplicate 而不是 Blocked
	got := reg.Admit(id, key)
	if got.Kind != AdmissionDuplicateRunID || got.RunID != id {
		t.Fatalf("实得 %v / %s", got.Kind, got.RunID)
	}
	if reg.Running() != 1 {
		t.Errorf("重放不得多占一个槽，实得 %d", reg.Running())
	}
}

// TestReplayAfterReleaseIsStillDuplicate 关键：跑完释放锁之后，
// 同 runId 重放**仍**要判重复。
//
// 否则"网络重试"会把已经做完的事再做一遍（而写操作没有幂等键）。
func TestReplayAfterReleaseIsStillDuplicate(t *testing.T) {
	reg := NewRunRegistry()
	id := core.NewRunID(newSeededRng())
	key := NewMutexKey("sign", "acc-1", "")

	reg.Admit(id, key)
	if !reg.Release(id) {
		t.Fatal("释放应成功")
	}
	if reg.Running() != 0 {
		t.Fatalf("锁已释放，实得 %d", reg.Running())
	}
	if got := reg.Admit(id, key); got.Kind != AdmissionDuplicateRunID {
		t.Errorf("跑完之后同 runId 仍应判重复（幂等的前提），实得 %v", got.Kind)
	}
}

func TestSecondRunIDSameParamsIsBlocked(t *testing.T) {
	reg := NewRunRegistry()
	id1, id2 := twoRunIDs(t)
	key := NewMutexKey("exchange", "acc-1", "same-params")

	if got := reg.Admit(id1, key); got.Kind != AdmissionAdmitted {
		t.Fatalf("实得 %v", got.Kind)
	}
	got := reg.Admit(id2, key)
	if got.Kind != AdmissionBlocked {
		t.Fatalf("不同 runId、同一把锁 ⇒ 应挡住，实得 %v", got.Kind)
	}
	if got.Holder != id1 {
		t.Errorf("占用者应是 id1，实得 %s", got.Holder)
	}
	if reg.Running() != 1 {
		t.Errorf("实得 %d", reg.Running())
	}
}

// TestDifferentParamsMayRunConcurrently 是修复掉的那个 bug 的另一半。
func TestDifferentParamsMayRunConcurrently(t *testing.T) {
	reg := NewRunRegistry()
	id1, id2 := twoRunIDs(t)

	keyA := NewMutexKey("exchange", "acc-1", ParamsDigest(map[string]string{"prizeId": "1001"}))
	keyB := NewMutexKey("exchange", "acc-1", ParamsDigest(map[string]string{"prizeId": "1002"}))
	if keyA == keyB {
		t.Fatal("不同参数必须得到不同的锁键")
	}

	if got := reg.Admit(id1, keyA); got.Kind != AdmissionAdmitted {
		t.Fatalf("实得 %v", got.Kind)
	}
	if got := reg.Admit(id2, keyB); got.Kind != AdmissionAdmitted {
		t.Fatalf("同一账号抢不同奖品不得互相挡 —— 旧项目就是在这里拿回了错的结果，实得 %v", got.Kind)
	}
	if reg.Running() != 2 {
		t.Errorf("两个不同参数的运行各占一把锁，实得 %d", reg.Running())
	}
}

func TestReleaseIsScopedToItsOwnRun(t *testing.T) {
	reg := NewRunRegistry()
	id1, id2 := twoRunIDs(t)
	key := NewMutexKey("sign", "acc-1", "")

	reg.Admit(id1, key)
	if reg.Release(id2) {
		t.Error("没被受理过的 runId 释放应当报 false")
	}
	if reg.Running() != 1 {
		t.Errorf("不该误释放别人的锁，实得 %d", reg.Running())
	}
}

func TestReleaseTwiceIsIdempotent(t *testing.T) {
	reg := NewRunRegistry()
	id := core.NewRunID(newSeededRng())
	reg.Admit(id, NewMutexKey("sign", "acc-1", ""))

	if !reg.Release(id) {
		t.Error("第一次释放应成功")
	}
	if !reg.Release(id) {
		t.Error("重复释放不该报错")
	}
	if reg.Running() != 0 {
		t.Errorf("实得 %d", reg.Running())
	}
}

// ── 事件总线 ────────────────────────────────────────────────────────────────

func TestSeqIsAssignedByTheBus(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	s1, _ := bus.Publish(core.NewEventStarted(id, "sign"))
	s2, _ := bus.Publish(core.NewEventProgress(id, 1, 3))
	s3, _ := bus.Publish(core.NewEventLog(id, core.LevelInfo, "hi"))
	if s1.Value() != 1 || s2.Value() != 2 || s3.Value() != 3 {
		t.Errorf("实得 %d %d %d", s1.Value(), s2.Value(), s3.Value())
	}
	if bus.Len(id) != 3 {
		t.Errorf("实得 %d", bus.Len(id))
	}
}

// TestLateSubscriberReceivesEarlierEvents 迟到的订阅者必须能看全 ——
// 旧项目这里会丢事件。
func TestLateSubscriberReceivesEarlierEvents(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	bus.Publish(core.NewEventStarted(id, "sign"))
	bus.Publish(core.NewEventProgress(id, 1, 2))

	// 事件已经发完了才订阅（用户打开页面时 run 早就开始了）
	sub := bus.Subscribe(id, core.SeqStart())
	got := bus.Drain(id, sub)
	if len(got) != 2 {
		t.Fatalf("迟到的订阅者应补齐历史，实得 %d", len(got))
	}
	if !got[0].Event.IsStarted() {
		t.Error("第一条应是 started")
	}
	if got[1].Seq.Value() != 2 {
		t.Errorf("实得 %d", got[1].Seq.Value())
	}
}

// TestReconnectResumesAfterLastSeen 断线重连：
// 从 `Last-Event-ID` 之后续传，不重复也不跳号。
func TestReconnectResumesAfterLastSeen(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	for i := 0; i < 5; i++ {
		bus.Publish(core.NewEventProgress(id, uint32(i), 5))
	}
	// 客户端说"我收到 3 号了" —— `since` 是**已看到的值**（SeqSeen），
	// 所以 3 号不重发，从 4 号接着来
	sub := bus.Subscribe(id, core.SeqSeen(3))
	got := bus.Drain(id, sub)
	if len(got) != 2 {
		t.Fatalf("只该补 4、5 两条，实得 %d", len(got))
	}
	if got[0].Seq.Value() != 4 || got[1].Seq.Value() != 5 {
		t.Errorf("实得 %d %d", got[0].Seq.Value(), got[1].Seq.Value())
	}
}

func TestLiveSubscriberReceivesInOrder(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	sub := bus.Subscribe(id, core.SeqStart())

	bus.Publish(core.NewEventStarted(id, "sign"))
	bus.Publish(core.NewEventProgress(id, 1, 2))
	bus.Publish(core.NewEventFinished(id, core.OutcomeSuccess, nil, nil))

	got := bus.Drain(id, sub)
	if len(got) != 3 {
		t.Fatalf("实得 %d", len(got))
	}
	for i, want := range []uint64{1, 2, 3} {
		if got[i].Seq.Value() != want {
			t.Errorf("第 %d 条序号实得 %d", i, got[i].Seq.Value())
		}
	}
	if !got[2].Event.IsTerminal() {
		t.Error("最后一条应是终态")
	}
}

// TestSlowSubscriberDoesNotBlockThePublisher 慢订阅者**不能阻塞**发布者 ——
// 卡住的 SSE 连接不该拖住别人的任务。
func TestSlowSubscriberDoesNotBlockThePublisher(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	slow := bus.SubscribeWithCapacity(id, core.SeqStart(), 2)
	healthy := bus.Subscribe(id, core.SeqStart())

	for i := 0; i < 5; i++ {
		_, deliveries := bus.Publish(core.NewEventProgress(id, uint32(i), 5))
		// 发布永远成功，不因某人满而失败
		if len(deliveries) != 2 {
			t.Fatalf("两个订阅者都该收到一次投递结果，实得 %d", len(deliveries))
		}
	}

	// 慢的那位丢了 3 条（5 发、容 2），且**被记下来**而不是静默
	if got := bus.Dropped(id, slow); got != 3 {
		t.Errorf("实得 %d", got)
	}
	slowGot := bus.Drain(id, slow)
	if len(slowGot) != 2 {
		t.Fatalf("满队列只留得下 2 条，实得 %d", len(slowGot))
	}
	// 丢的是**最新**的：留下的应是 1、2 号，不是 4、5 号。
	if slowGot[0].Seq.Value() != 1 || slowGot[1].Seq.Value() != 2 {
		t.Errorf("实得 %d %d", slowGot[0].Seq.Value(), slowGot[1].Seq.Value())
	}

	// 正常的订阅者一条不丢
	if got := bus.Dropped(id, healthy); got != 0 {
		t.Errorf("实得 %d", got)
	}
	if got := len(bus.Drain(id, healthy)); got != 5 {
		t.Errorf("实得 %d", got)
	}
}

func TestEndedRunCanStillBeReplayed(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	bus.Publish(core.NewEventStarted(id, "sign"))
	summary := "要去 App 打开消息通知"
	bus.Publish(core.NewEventFinished(id, core.OutcomeManualRequired, nil, &summary))

	// 没有订阅者，直接回放（"用户来看历史"这条路径）
	all := bus.Replay(id, core.SeqStart())
	if len(all) != 2 {
		t.Fatalf("实得 %d", len(all))
	}
	outcome, ok := all[1].Event.EventOutcome()
	if !ok || outcome != core.OutcomeManualRequired {
		t.Errorf("回放要保住四态判定，不能压成成功：%v / %v", outcome, ok)
	}
}

func TestUnsubscribeStopsDelivery(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	sub := bus.Subscribe(id, core.SeqStart())
	if bus.SubscriberCount(id) != 1 {
		t.Fatalf("实得 %d", bus.SubscriberCount(id))
	}

	if !bus.Unsubscribe(id, sub) {
		t.Error("第一次退订应报 true")
	}
	if bus.SubscriberCount(id) != 0 {
		t.Errorf("实得 %d", bus.SubscriberCount(id))
	}
	if bus.Unsubscribe(id, sub) {
		t.Error("重复退订应报 false")
	}

	// 退订后再发布不该 panic，也不该给已退订的人投递
	_, deliveries := bus.Publish(core.NewEventStarted(id, "sign"))
	if len(deliveries) != 0 {
		t.Errorf("实得 %d", len(deliveries))
	}
}

func TestStreamsAreIsolatedPerRun(t *testing.T) {
	bus := NewEventBus()
	a, b := twoRunIDs(t)
	bus.Publish(core.NewEventStarted(a, "sign"))
	bus.Publish(core.NewEventStarted(b, "exchange"))
	bus.Publish(core.NewEventProgress(b, 1, 2))

	if bus.Len(a) != 1 || bus.Len(b) != 2 {
		t.Errorf("实得 %d / %d", bus.Len(a), bus.Len(b))
	}
	if len(bus.Replay(a, core.SeqStart())) != 1 {
		t.Error("a 的回放应只有 1 条")
	}
}

func TestUnknownRunYieldsEmptyEverything(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	if !bus.IsEmpty(id) {
		t.Error("未知 run 应为空")
	}
	if len(bus.Replay(id, core.SeqStart())) != 0 {
		t.Error("未知 run 的回放应为空")
	}
	if bus.SubscriberCount(id) != 0 {
		t.Error("未知 run 的订阅者数应为 0")
	}
	if len(bus.Drain(id, 42)) != 0 {
		t.Error("对不存在的 run drain 不该 panic，也不该有结果")
	}
	if bus.Forget(id) {
		t.Error("忘掉一个不存在的 run 应报 false")
	}
}

// TestSeqIsStrictlyIncreasingAndMatchesHistory 序号在同一个 run 内**严格递增**，
// 且与历史里的排列一致 —— 这是续传能成立的前提。
func TestSeqIsStrictlyIncreasingAndMatchesHistory(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	for i := 0; i < 20; i++ {
		bus.Publish(core.NewEventProgress(id, uint32(i), 20))
	}
	history := bus.Replay(id, core.SeqStart())
	for i := 1; i < len(history); i++ {
		if history[i-1].Seq.Value() >= history[i].Seq.Value() {
			t.Fatalf("序号不递增：%d → %d", history[i-1].Seq.Value(), history[i].Seq.Value())
		}
	}

	// 从任意位置续传，拿到的应恰好是它之后那些，且不重不漏。
	//
	// 用 SeqSeen(last)（"我已经看到 last 了"）而不是 SeqResumeAfter(last)
	// （"下一个要取 last+1"）：`since` 参数的语义是**前者**。
	// 传后者会让过滤条件 `> last+1` 把 `last+1` 自己也跳过 —— 静默少一条。
	for _, last := range []uint64{1, 7, 19} {
		after := bus.Replay(id, core.SeqSeen(last))
		if len(after) != 20-int(last) {
			t.Errorf("seen(%d) 条数不对：实得 %d", last, len(after))
		}
		if after[0].Seq.Value() != last+1 {
			t.Errorf("续传应从 %d 开始，实得 %d", last+1, after[0].Seq.Value())
		}
	}

	// 边界：`last = 20`（已追平）→ 空，而不是报错或重发
	if len(bus.Replay(id, core.SeqSeen(20))) != 0 {
		t.Error("追平之后没有可补的")
	}
	// 边界：零值与 seen(0) 等价（0 号从不发出）
	if len(bus.Replay(id, core.SeqStart())) != 20 {
		t.Error("零值应等价于从头")
	}
	if len(bus.Replay(id, core.SeqSeen(0))) != 20 {
		t.Error("seen(0) 应等价于从头")
	}
}

func TestLogHelperAssignsSeq(t *testing.T) {
	bus := NewEventBus()
	id := core.NewRunID(newSeededRng())
	if seq := bus.Log(id, core.LevelWarn, "注意"); seq.Value() != 1 {
		t.Errorf("实得 %d", seq.Value())
	}
	if bus.Len(id) != 1 {
		t.Errorf("实得 %d", bus.Len(id))
	}
}

// TestBusAndRegistryAreSafeUnderConcurrency 是 **Go 侧专属**的那条。
//
// Rust 用 `&mut R rng` 保证同一随机源不被并发共享 —— 那是**编译期保证**。
// 这里的锁是运行期的，所以只有 `-race` 能证明它成立（见方案 §5.2）。
// 缺了这条，data race 不一定让测试变红，可能只是偶发地产生错误数据。
func TestBusAndRegistryAreSafeUnderConcurrency(t *testing.T) {
	bus := NewEventBus()
	reg := NewRunRegistry()
	id := core.NewRunID(newSeededRng())

	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			bus.Publish(core.NewEventProgress(id, uint32(i), 8))
			bus.Subscribe(id, core.SeqStart())
			reg.Admit(core.NewRunID(newSeededRng()), NewMutexKey("sign", "acc-1", ""))
			_ = bus.Len(id)
			_ = reg.Running()
		}(i)
	}
	wg.Wait()

	if got := bus.Len(id); got != 8 {
		t.Errorf("8 次并发发布应留下 8 条，实得 %d", got)
	}
	history := bus.Replay(id, core.SeqStart())
	seen := map[uint64]bool{}
	for _, item := range history {
		if seen[item.Seq.Value()] {
			t.Errorf("序号 %d 重复 —— 并发分配撞号", item.Seq.Value())
		}
		seen[item.Seq.Value()] = true
	}
}

// TestZeroValueTypesAreUnusable 守住"必须走构造函数"这条。
//
// Rust 的 `Default` 让 `RunRegistry::default()` 天然可用；而 Go 的零值
// 是 **nil map**，写会 panic。这里显式把"零值可用吗"这件事问出来：
// 读操作可以（nil map 可读），写操作不行 —— 所以断言的是"读不 panic"，
// 而不是"零值也能用"。
func TestZeroValueTypesAreUnusable(t *testing.T) {
	var reg RunRegistry
	var bus EventBus
	id := core.NewRunID(newSeededRng())

	if reg.HasSeen(id) {
		t.Error("零值登记表不该认得任何 runId")
	}
	if bus.Len(id) != 0 {
		t.Error("零值总线不该有事件")
	}
}
