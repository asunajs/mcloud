package mcloudio

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// `run_events` 这一组的分量集中在**两件事**上，其它都是围绕它们的边角：
//
//  1. **续传**（[SqliteStore.RunEventsAfter] 的 `after` 语义）—— SSE 断线重连靠它。
//     序号是唯一的锚点，所以下面每条断言都在盯"边界那一号"到底含不含。
//  2. **归属**（`validateRunInScope`）—— 事件行上没有 owner，
//     只顺着 run 查。这条如果漏了，任何登录用户都能往别人的 run 里塞日志。
//
// 幂等那几条（重复写同一批）不是"以防万一"：进程崩在"写完库、还没告诉总线"
// 之间是**正常路径**，重启后重放同一批必须无害。

// putRun 造一个已归属的 run（带账号）。
func putRun(t *testing.T, s *SqliteStore, runID, accountID string) {
	t.Helper()
	run := runningRun(runID, accountID)
	if err := s.InsertRun(context.Background(), core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("建 run %s: %v", runID, err)
	}
}

// runWithAccount 造一个 run + 一个账号，返回 run id。
func runWithAccount(t *testing.T, s *SqliteStore, runID string) core.RunID {
	t.Helper()
	putAccount(t, s, "acc-1")
	putRun(t, s, runID, "acc-1")
	id, ok := core.ParseRunID(runID)
	if !ok {
		t.Fatalf("测试用的 id 必须是合法 uuid: %s", runID)
	}
	return id
}

// logs 造一批从 1 起、连续的 `count` 条 `log` 事件。
func logs(runID core.RunID, count int) []core.Sequenced {
	out := make([]core.Sequenced, 0, count)
	for i := 1; i <= count; i++ {
		out = append(out, core.NewSequenced(
			core.SeqSeen(uint64(i)),
			core.NewEventLog(runID, core.LevelInfo, fmt.Sprintf("第 %d 条", i)),
		))
	}
	return out
}

// seqs 抽出这批事件的序号，便于按切片直接比。
func seqs(events []core.Sequenced) []uint64 {
	out := make([]uint64, 0, len(events))
	for _, e := range events {
		out = append(out, e.Seq.Value())
	}
	return out
}

func eqSeqs(t *testing.T, got []core.Sequenced, want []uint64) {
	t.Helper()
	actual := seqs(got)
	if len(actual) != len(want) {
		t.Fatalf("序号个数不符：实得 %v 期望 %v", actual, want)
	}
	for i := range want {
		if actual[i] != want[i] {
			t.Fatalf("序号不符：实得 %v 期望 %v", actual, want)
		}
	}
}

func TestEventsRoundTripInSequenceOrder(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440000")

	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 3)); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("应有 3 条，实得 %d", len(got))
	}
	// 顺序必须是 seq 升序 —— 前端靠它拼出"发生了什么"的时间线
	eqSeqs(t, got, []uint64{1, 2, 3})
	if got[0].Event.Kind != core.EventLog {
		t.Fatalf("第一条应当是日志事件，实得 %s", got[0].Event.Kind)
	}
}

// TestResumingAfterASeqExcludesThatSeq 是整个文件的核心：**续传的边界**。
//
// `after = Some(n)` 的语义是"我已经看到 n 了"，所以返回的必须**不含 n**。
// 含了会让前端重复渲染一条；不含 n+1 会让它永远缺一条 —— 两者都静默。
func TestResumingAfterASeqExcludesThatSeq(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440001")
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 5)); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	after2 := core.SeqSeen(2)
	rest, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, &after2, 100)
	if err != nil {
		t.Fatalf("续传: %v", err)
	}
	eqSeqs(t, rest, []uint64{3, 4, 5}) // 必须从 3 开始：2 已经收到过了

	// `after = nil` 与 `after = Some(0)` 必须等价（0 号从不发出，见 Seq 的说明）
	fromNone, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("从头读: %v", err)
	}
	zero := core.SeqStart()
	fromZero, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, &zero, 100)
	if err != nil {
		t.Fatalf("从 0 读: %v", err)
	}
	if len(fromNone) != 5 {
		t.Fatalf("从头应读到 5 条，实得 %d", len(fromNone))
	}
	eqSeqs(t, fromNone, seqs(fromZero))

	// 已经追平（`after` = 最后一条）→ 空，而不是报错
	after5 := core.SeqSeen(5)
	nothing, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, &after5, 100)
	if err != nil {
		t.Fatalf("追平后读: %v", err)
	}
	if len(nothing) != 0 {
		t.Fatalf("追平之后应当返回空，方便 SSE 保持连接等新事件，实得 %v", seqs(nothing))
	}
}

// TestReplayingTheSameBatchIsIdempotent：重放同一批必须无害 ——
// 进程崩在"写完库、还没告诉总线"之间是正常路径。
func TestReplayingTheSameBatchIsIdempotent(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440002")
	batch := logs(runID, 3)

	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, batch); err != nil {
		t.Fatalf("首批: %v", err)
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, batch); err != nil {
		t.Fatalf("重放不该报错: %v", err)
	}

	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("重放不该产生重复行，实得 %d 条", len(got))
	}
}

// TestASecondBatchAppendsAfterTheFirst：第二批可以接在第一批后面（seq 连续），
// 这是"边跑边落库"的正常形态。
func TestASecondBatchAppendsAfterTheFirst(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440003")

	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 2)); err != nil {
		t.Fatalf("首批: %v", err)
	}
	// 从 3 开始的下一批
	next := []core.Sequenced{
		core.NewSequenced(core.SeqSeen(3), core.NewEventLog(runID, core.LevelWarn, "第二批 3")),
		core.NewSequenced(core.SeqSeen(4), core.NewEventLog(runID, core.LevelWarn, "第二批 4")),
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, next); err != nil {
		t.Fatalf("次批: %v", err)
	}

	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	eqSeqs(t, got, []uint64{1, 2, 3, 4})
}

// TestAnotherUsersRunEventsAreInvisibleAndUnwritable：归属 ——
// 别人的事件读写不到，也写不进去。
//
// 三条断言必须给**同一个**结论（NotFound）—— "报什么错"本身不能泄漏
// "这条 run 存在"。这与 `TestUpdatingAMissingOrForeignRunIsAnError` 是同一条理由。
func TestAnotherUsersRunEventsAreInvisibleAndUnwritable(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440004")
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 2)); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	other := newUser(t, s, "other-user")

	_, err := s.RunEventsAfter(ctx, core.ScopeUser(&other), runID, nil, 100)
	assertKind(t, mustErr(t, err), core.KindNotFound) // 别人的 run 读不到
	assertKind(t, mustErr(t, s.AppendRunEvents(ctx, core.ScopeUser(&other), runID, logs(runID, 1))),
		core.KindNotFound) // 别人的 run 也写不进
	_, err = s.DeleteRunEvents(ctx, core.ScopeUser(&other), runID)
	assertKind(t, mustErr(t, err), core.KindNotFound) // 更不该能删别人的事件

	// 而主人自己一切正常
	if _, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100); err != nil {
		t.Fatalf("主人读自己的事件: %v", err)
	}
}

// TestEventsForAMissingRunAreRejected：不存在的 run 不接受事件写入 ——
// 否则会留下"事件表有行、runs 表没行"的孤儿数据。
//
// **读也一样报 NotFound**，而不是返回空切片。这一点值得说明：
// "这个 run 不存在"与"它存在但没有事件"是两件不同的事，而返回空把两者混成一件事，
// 调用方（SSE 端点）就无法区分"该 404"和"该挂着一个空流等新事件"。
// 一致地报错还顺带保住了"报什么错不泄漏它存不存在"。
func TestEventsForAMissingRunAreRejected(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	ghost, ok := core.ParseRunID("550e8400-e29b-41d4-a716-4466554400ff")
	if !ok {
		t.Fatal("夹具 id 必须合法")
	}

	assertKind(t, mustErr(t, s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), ghost, logs(ghost, 1))),
		core.KindNotFound) // run 不存在就不该有事件
	_, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), ghost, nil, 100)
	assertKind(t, mustErr(t, err), core.KindNotFound) // 读一个不存在的 run 也要 404，不能返回空
}

// TestDeletingEventsReportsTheRealCount：删除返回真实行数 ——
// 调用方要靠它确认"确实清干净了"。
func TestDeletingEventsReportsTheRealCount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440005")
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 4)); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	n, err := s.DeleteRunEvents(ctx, core.ScopeUser(uidPtr()), runID)
	if err != nil {
		t.Fatalf("删除: %v", err)
	}
	if n != 4 {
		t.Fatalf("应删 4 行，实得 %d", n)
	}
	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("删完应当为空，实得 %d 条", len(got))
	}
	// 重复删 → 0 行，而不是报错（"已经没了"不是失败）
	n, err = s.DeleteRunEvents(ctx, core.ScopeUser(uidPtr()), runID)
	if err != nil {
		t.Fatalf("重复删除不该报错: %v", err)
	}
	if n != 0 {
		t.Fatalf("重复删应为 0 行，实得 %d", n)
	}
}

// TestDeletingOneRunsEventsLeavesTheOtherAlone：删除一个 run 的事件不影响别的 run
// —— 序号是**每个 run 各自**的计数。
func TestDeletingOneRunsEventsLeavesTheOtherAlone(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440006")
	putRun(t, s, "550e8400-e29b-41d4-a716-446655440007", "acc-1")
	b, ok := core.ParseRunID("550e8400-e29b-41d4-a716-446655440007")
	if !ok {
		t.Fatal("夹具 id 必须合法")
	}

	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), a, logs(a, 3)); err != nil {
		t.Fatalf("写 A: %v", err)
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), b, logs(b, 2)); err != nil {
		t.Fatalf("写 B: %v", err)
	}

	if _, err := s.DeleteRunEvents(ctx, core.ScopeUser(uidPtr()), a); err != nil {
		t.Fatalf("删 A: %v", err)
	}

	gotA, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), a, nil, 100)
	if err != nil {
		t.Fatalf("读 A: %v", err)
	}
	if len(gotA) != 0 {
		t.Fatalf("A 应已清空，实得 %d", len(gotA))
	}
	gotB, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), b, nil, 100)
	if err != nil {
		t.Fatalf("读 B: %v", err)
	}
	if len(gotB) != 2 {
		t.Fatalf("删 A 不该动 B，实得 %d 条", len(gotB))
	}
}

// TestTheLimitKeepsTheOldestEvents：`limit` 真的生效 ——
// **且截断的是尾部**（保留最老的几条）。
//
// 方向很重要：如果实现写成 `ORDER BY seq DESC LIMIT n`，拿到的是"最新的 n 条"，
// 前端第一次连上就会看到 run 的结尾、然后发现前面全没了。
func TestTheLimitKeepsTheOldestEvents(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440008")
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, logs(runID, 6)); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	page, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 2)
	if err != nil {
		t.Fatalf("第一页: %v", err)
	}
	eqSeqs(t, page, []uint64{1, 2}) // 截断必须从尾部截，保留最老的几条

	// 拿最后一条的 seq 再问一次 —— 这就是调用方翻页的方式
	last := page[len(page)-1].Seq
	next, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, &last, 2)
	if err != nil {
		t.Fatalf("第二页: %v", err)
	}
	eqSeqs(t, next, []uint64{3, 4})
}

// TestAManualRequiredFinishSurvivesTheRoundTrip：终局事件（含 `ManualRequired`
// 与它的待办）必须原样往返 —— 这是回看历史的价值所在。
func TestAManualRequiredFinishSurvivesTheRoundTrip(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-446655440009")

	step := core.NewManualStep("msg-push", nil, "请前往 App 手动打开消息通知", core.TargetMobileApp)
	summary := "3 个账号已签到，1 个需手动"
	batch := []core.Sequenced{
		core.NewSequenced(core.SeqSeen(1), core.NewEventStarted(runID, "msg-push")),
		core.NewSequenced(core.SeqSeen(2), core.NewEventProgress(runID, 3, 10)),
		core.NewSequenced(core.SeqSeen(3), core.NewEventFinished(
			runID, core.OutcomeManualRequired, []core.ManualStep{step}, &summary)),
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, batch); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("应有 3 条，实得 %d", len(got))
	}

	// 终局那条的待办必须一字不差地回来 —— 丢掉它，回看历史就只剩一句"需人工"，
	// 而"要人去做什么"恰恰是这条结论的全部价值
	last := got[2].Event
	if last.Kind != core.EventFinished {
		t.Fatalf("最后一条应当是 Finished，实得 %s", last.Kind)
	}
	if last.Outcome != core.OutcomeManualRequired {
		t.Fatalf("结论应为 ManualRequired，实得 %s", last.Outcome)
	}
	if len(last.ManualSteps) != 1 {
		t.Fatalf("待办应有 1 条，实得 %d", len(last.ManualSteps))
	}
	if last.ManualSteps[0] != step {
		t.Fatalf("待办必须一字不差地回来：\n实得 %+v\n期望 %+v", last.ManualSteps[0], step)
	}
	if last.Summary == nil || *last.Summary != summary {
		t.Fatalf("结论说明应当原样回来，实得 %v", last.Summary)
	}
	if !last.IsTerminal() {
		t.Fatal("Finished 应当是终局事件")
	}

	// 进度那条的百分比也要能还原（前端直接拿它画进度条）
	pct, ok := got[1].Event.Percent()
	if !ok || pct != 30 {
		t.Fatalf("进度应为 30%%，实得 %d ok=%v", pct, ok)
	}
}

// TestAnEmptyBatchIsAccepted：空批不报错 ——
// 调用方不该为了"这次没有日志"写一个 if。
func TestAnEmptyBatchIsAccepted(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-44665544000a")
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, nil); err != nil {
		t.Fatalf("空批不该报错: %v", err)
	}
	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("应当是空的，实得 %d", len(got))
	}
}

// TestEventPayloadRoundTripsEveryKind 逐类核对事件在**库里那一列**的形状。
//
// 这条不是重复：上面几条只看 `Kind` / 几个字段，而 payload 是**独立的一份
// 序列化**（`kind` 列只是它的一个冗余索引）。真正落库的是 payload，
// 前端拿到的是它 —— 所以它必须自己单独被钉住。
func TestEventPayloadRoundTripsEveryKind(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-44665544000b")

	summary := "结算说明"
	step := core.NewManualStep("sign", nil, "去 App 点一下", core.TargetMobileApp)
	batch := []core.Sequenced{
		core.NewSequenced(core.SeqSeen(1), core.NewEventStarted(runID, "sign")),
		core.NewSequenced(core.SeqSeen(2), core.NewEventLog(runID, core.LevelError, "带 <尖括号> 与 & 的正文")),
		core.NewSequenced(core.SeqSeen(3), core.NewEventProgress(runID, 7, 20)),
		core.NewSequenced(core.SeqSeen(4), core.NewEventFinished(
			runID, core.OutcomePartialSuccess, []core.ManualStep{step}, &summary)),
		core.NewSequenced(core.SeqSeen(5), core.NewEventErrored(runID, "上游 500")),
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, batch); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	got, err := s.RunEventsAfter(ctx, core.ScopeUser(uidPtr()), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件: %v", err)
	}
	if len(got) != 5 {
		t.Fatalf("应有 5 条，实得 %d", len(got))
	}

	kinds := []core.RunEventKind{core.EventStarted, core.EventLog, core.EventProgress,
		core.EventFinished, core.EventErrored}
	for i, want := range kinds {
		if got[i].Event.Kind != want {
			t.Fatalf("第 %d 条的类别应为 %s，实得 %s", i+1, want, got[i].Event.Kind)
		}
		if got[i].Event.RunID != runID {
			t.Fatalf("第 %d 条的 runId 丢了：%s", i+1, got[i].Event.RunID)
		}
	}
	// ⚠ 尖括号与 & **不被转义** —— payload 走的是 [core.RunEvent.MarshalJSON]，
	// 而不是 `json.Marshal`（后者默认 escapeHTML，会把 `<` 变成 `\u003c`）。
	// 这条断言是"库里存的形状与 serde_json 一致"唯一被守的地方。
	if got[1].Event.Message != "带 <尖括号> 与 & 的正文" {
		t.Fatalf("日志正文应原样回来（含尖括号与 &），实得 %q", got[1].Event.Message)
	}
	if got[4].Event.Kind != core.EventErrored || got[4].Event.Message != "上游 500" {
		t.Fatalf("errored 的说明丢了: %+v", got[4].Event)
	}
}

// TestKindColumnMatchesThePayloadTag 钉住"冗余但不漂移"那条约定：
// `kind` 列（SQL 可直接过滤）与 payload 里的 `type` 标签必须同源。
//
// 两者不同源时，`WHERE kind = 'log'` 会筛出 payload 自称是 progress 的行 ——
// 而两边各自看都"对"。
func TestKindColumnMatchesThePayloadTag(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	runID := runWithAccount(t, s, "550e8400-e29b-41d4-a716-44665544000c")
	batch := []core.Sequenced{
		core.NewSequenced(core.SeqSeen(1), core.NewEventStarted(runID, "sign")),
		core.NewSequenced(core.SeqSeen(2), core.NewEventLog(runID, core.LevelInfo, "x")),
		core.NewSequenced(core.SeqSeen(3), core.NewEventProgress(runID, 1, 2)),
		core.NewSequenced(core.SeqSeen(4), core.NewEventErrored(runID, "boom")),
	}
	if err := s.AppendRunEvents(ctx, core.ScopeUser(uidPtr()), runID, batch); err != nil {
		t.Fatalf("写事件: %v", err)
	}

	rows, err := s.db.QueryContext(ctx,
		"SELECT seq, kind, payload FROM run_events WHERE run_id = ? ORDER BY seq ASC", runID.String())
	if err != nil {
		t.Fatalf("直查: %v", err)
	}
	defer func() { _ = rows.Close() }()

	seen := 0
	for rows.Next() {
		var (
			seq     int64
			kind    string
			payload string
		)
		if err := rows.Scan(&seq, &kind, &payload); err != nil {
			t.Fatalf("扫描: %v", err)
		}
		seen++
		want := fmt.Sprintf(`"type":"%s"`, kind)
		if !strings.Contains(payload, want) {
			t.Fatalf("第 %d 行的 kind 列是 %q，而 payload 里找不到 %s：%s", seq, kind, want, payload)
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("遍历: %v", err)
	}
	if seen != 4 {
		t.Fatalf("应有 4 行，实得 %d", seen)
	}
}

// TestRunEventsSurviveReopen：事件是"回看历史"的载体，必须落盘。
func TestRunEventsSurviveReopen(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "mcloud.db")

	first, err := Open(path)
	if err != nil {
		t.Fatalf("打开: %v", err)
	}
	admin := core.NewUser(testUser, testUser, core.PasswordHash("!"), core.RoleAdmin, time.Now().UTC())
	if err := first.UpsertUser(ctx, &admin); err != nil {
		t.Fatalf("建用户: %v", err)
	}
	acc := account("acc-1")
	owner := admin.ID
	acc.Owner = &owner
	if err := first.UpsertAccount(ctx, core.ScopeUser(&owner), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}
	runID, ok := core.ParseRunID("550e8400-e29b-41d4-a716-44665544000d")
	if !ok {
		t.Fatal("夹具 id 必须合法")
	}
	run := runningRun(runID.String(), "acc-1")
	if err := first.InsertRun(ctx, core.ScopeUser(&owner), &run); err != nil {
		t.Fatalf("建 run: %v", err)
	}
	if err := first.AppendRunEvents(ctx, core.ScopeUser(&owner), runID, logs(runID, 3)); err != nil {
		t.Fatalf("写事件: %v", err)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("关库: %v", err)
	}

	second, err := Open(path)
	if err != nil {
		t.Fatalf("重开: %v", err)
	}
	t.Cleanup(func() { _ = second.Close() })
	got, err := second.RunEventsAfter(ctx, core.ScopeUser(&owner), runID, nil, 100)
	if err != nil {
		t.Fatalf("重启后读事件: %v", err)
	}
	eqSeqs(t, got, []uint64{1, 2, 3})
}
