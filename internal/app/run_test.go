package app

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// seqRandom 是递增字节的确定性随机源。
//
// 它存在的理由：runId 由 UUIDv4 生成，若用真随机则"同一个账号两趟的 runId"
// 无从比较，而"被谁挡了"（`blocked_by`）这条断言正需要能预知 runId。
type seqRandom struct{ next byte }

func (r *seqRandom) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

func runNow() time.Time { return time.Date(2026, 9, 20, 0, 0, 0, 0, time.UTC) }

func runAccount(suffix string) core.AccountID { return core.NewAccountID("19****" + suffix) }

// storeWithAccounts 造一个用户 + 两个归他的账号。
//
// 账号必须真的存在：`runs` 的读写是按归属过滤的
// （`account_id IN (SELECT … owner = ?)`），拿一个不存在的账号 id 去写，
// 得到的不是"写进去了"，而是"看不见"。
func storeWithAccounts(t *testing.T) *mcloudio.SqliteStore {
	t.Helper()
	store, err := mcloudio.OpenInMemory()
	if err != nil {
		t.Fatalf("打开内存库失败: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })

	ctx := context.Background()
	admin := core.NewUser("alice", "alice", "!", core.RoleAdmin, runNow())
	if err := store.UpsertUser(ctx, &admin); err != nil {
		t.Fatalf("写入用户失败: %v", err)
	}
	scope := core.ScopeUser(&admin.ID)
	for _, suffix := range []string{"01", "02"} {
		account := core.NewAccount(runAccount(suffix).String())
		account.Owner = &admin.ID
		if err := store.UpsertAccount(ctx, scope, &account); err != nil {
			t.Fatalf("写入账号失败: %v", err)
		}
	}
	return store
}

func runScope() core.Scope {
	uid := core.UserID("alice")
	return core.ScopeUser(&uid)
}

func okReport() core.RunReport { return core.NewRunReportSuccess() }

func manualReport() core.RunReport {
	summary := "要去 App 打开消息通知"
	return core.RunReport{
		Outcome:     core.OutcomeManualRequired,
		ManualSteps: []core.ManualStep{},
		Summary:     &summary,
	}
}

func newRunner(t *testing.T, store *mcloudio.SqliteStore) (
	*mcloudexec.RunRegistry, *mcloudexec.EventBus, *Runner,
) {
	t.Helper()
	registry := mcloudexec.NewRunRegistry()
	bus := mcloudexec.NewEventBus()
	return registry, bus, NewRunner(store, runScope(), fixedClock{at: runNow()}, registry, bus)
}

func TestRunsEveryAccountAndRecordsRunsAndEvents(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Outcome != core.OutcomeSuccess {
		t.Errorf("应全部成功，实得 %v", summary.Outcome)
	}
	if len(summary.Ran()) != 2 || summary.Skipped() != 0 {
		t.Errorf("两个都该真跑，实得 ran=%d skipped=%d", len(summary.Ran()), summary.Skipped())
	}
	if got := summary.SummaryLine(); !strings.Contains(got, "全部成功") {
		t.Errorf("摘要应说明全部成功，实得 %q", got)
	}

	// 历史：每账号一行，且都已终结
	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	if len(runs) != 2 {
		t.Fatalf("每账号一行，实得 %d", len(runs))
	}
	for _, run := range runs {
		if run.Capability != "signin" {
			t.Errorf("能力应是 signin，实得 %q", run.Capability)
		}
		if !run.Status.IsTerminal() {
			t.Errorf("跑完不能还是 Running: %v", run.Status)
		}
		if run.FinishedAt == nil {
			t.Error("跑完要有结束时刻")
		}
	}

	// 事件：每个账号一条流，且能按 seq 续传
	for index, entry := range summary.Accounts {
		if entry.Kind != AccountRunRan {
			t.Fatal("两个都该真跑")
		}
		events, err := store.RunEventsAfter(ctx, runScope(), entry.RunID, nil, 100)
		if err != nil {
			t.Fatalf("读事件失败: %v", err)
		}
		if len(events) < 3 {
			t.Errorf("started + finished + progress 至少 3 条，实得 %d", len(events))
			continue
		}
		var prev uint64
		for i, event := range events {
			if i > 0 && uint64(event.Seq) <= prev {
				t.Errorf("序号必须递增，实得 %v", events)
				break
			}
			prev = uint64(event.Seq)
		}
		if events[0].Event.Kind != core.EventStarted {
			t.Errorf("第一条应是 started，实得 %v", events[0].Event.Kind)
		}
		last := events[len(events)-1]
		if last.Event.Kind != core.EventProgress {
			t.Errorf("最后一条应是 progress，实得 %v", last.Event.Kind)
		}
		// ⭐ 进度必须**真的在走**：`Done` 不递增的写法（忘了 `done++`）一样能
		// 发出 progress 事件，而前端那条进度条从此永远停在 0%。
		// ⚠ 进度是**整批**的计数（每个账号的流里都写"这是第几个"），
		// 所以第 index 个账号的流里是 `index+1 / 2`。
		if last.Event.Done != uint32(index+1) || last.Event.Total != 2 {
			t.Errorf("第 %d 个账号的 progress 应是 %d/2，实得 %d/%d",
				index, index+1, last.Event.Done, last.Event.Total)
		}
	}
}

func TestOneFailingAccountDoesNotStopTheOthers(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()
	bad := runAccount("02")

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), bad},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, target core.AccountID, _ RunProgress) (core.RunReport, error) {
		if target == bad {
			return core.RunReport{}, core.NewBadRequest("模拟上游失败")
		}
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Outcome != core.OutcomePartialSuccess {
		t.Errorf("应是部分成功，实得 %v", summary.Outcome)
	}
	if len(summary.Ran()) != 2 {
		t.Errorf("失败的账号也要有结果，只是结果是失败，实得 %d", len(summary.Ran()))
	}

	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	var failed *core.RunRecord
	for i := range runs {
		if runs[i].AccountID != nil && *runs[i].AccountID == bad {
			failed = &runs[i]
			break
		}
	}
	if failed == nil {
		t.Fatal("失败账号应有历史行")
	}
	if failed.Status != core.StatusFailed {
		t.Errorf("失败账号的状态应是 Failed，实得 %v", failed.Status)
	}
	// 失败原因要留下（否则历史里只有"失败"两个字）
	if failed.Message == nil || !strings.Contains(*failed.Message, "模拟上游失败") {
		t.Errorf("失败原因要留下，实得 %v", failed.Message)
	}

	// 失败的那个账号也有 errored 事件
	events, err := store.RunEventsAfter(ctx, runScope(), core.RunID(failed.ID), nil, 100)
	if err != nil {
		t.Fatalf("读事件失败: %v", err)
	}
	hasErrored := false
	for _, event := range events {
		if event.Event.Kind == core.EventErrored {
			hasErrored = true
		}
	}
	if !hasErrored {
		t.Error("出错要有 errored 事件")
	}
}

func TestNeedingAHumanOutranksAPlainFailure(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	manual := runAccount("01")

	summary, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{manual, runAccount("02")},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, target core.AccountID, _ RunProgress) (core.RunReport, error) {
		if target == manual {
			return manualReport(), nil
		}
		return core.RunReport{}, core.NewBadRequest("另一个失败了")
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Outcome != core.OutcomeManualRequired {
		t.Errorf("需人工优先于失败，实得 %v", summary.Outcome)
	}
	if !summary.Outcome.NeedsHumanAction() {
		t.Error("需人工必须能被 needs_human_action 认出")
	}
	if got := summary.SummaryLine(); !strings.Contains(got, "客户端操作") {
		t.Errorf("摘要应提示去客户端操作，实得 %q", got)
	}

	// ⭐ 需人工那一趟在历史里是 **Done 不是 Failed** —— 它确实跑完了，
	// 只是目标没达成。写成 Failed 会让历史里出现一堆"失败"，
	// 而它们其实只是在等用户动一下手。
	ctx := context.Background()
	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	for _, run := range runs {
		if run.AccountID != nil && *run.AccountID == manual {
			if run.Status != core.StatusDone {
				t.Errorf("需人工的历史状态应是 Done，实得 %v", run.Status)
			}
			return
		}
	}
	t.Error("需人工的账号该有历史行")
}

func TestAHeldLockSkipsOnlyThatAccount(t *testing.T) {
	store := storeWithAccounts(t)
	registry, _, runner := newRunner(t, store)
	ctx := context.Background()

	// 先让别人占住 01 那把锁（同一个能力 + 账号 + 参数指纹）
	rng := &seqRandom{next: 1}
	holder := core.NewRunID(rng)
	key := mcloudexec.NewMutexKey("signin", runAccount("01").String(), "d0")
	if got := registry.Admit(holder, key); got.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("占用者应被受理，实得 %v", got.Kind)
	}

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d0",
	}, rng, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Skipped() != 1 || len(summary.Ran()) != 1 {
		t.Errorf("被占的那个跳过、另一个照跑，实得 skipped=%d ran=%d",
			summary.Skipped(), len(summary.Ran()))
	}
	// 跳过不是失败：总判定只看真的跑了的那个
	if summary.Outcome != core.OutcomeSuccess {
		t.Errorf("跳过不该影响总判定，实得 %v", summary.Outcome)
	}
	// 而且必须能看出"被谁挡了"
	var blockedBy *core.RunID
	for _, entry := range summary.Accounts {
		if entry.Kind == AccountRunSkipped {
			blockedBy = &entry.BlockedBy
		}
	}
	if blockedBy == nil || *blockedBy != holder {
		t.Errorf("应记下占用者 %v，实得 %v", holder, blockedBy)
	}
	// 那句给人看的话里也要有它，否则"只跑了一个"看起来像"另一个失败了"
	if got := summary.SummaryLine(); !strings.Contains(got, "跳过") {
		t.Errorf("摘要应提到跳过，实得 %q", got)
	}

	// 被挡下的账号不该产生 runs 行（它没跑）
	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	if len(runs) != 1 {
		t.Errorf("只该有一行历史，实得 %d", len(runs))
	}
}

func TestLocksAreReleasedEvenWhenTheTaskFails(t *testing.T) {
	store := storeWithAccounts(t)
	registry, _, runner := newRunner(t, store)

	if _, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01")},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return core.RunReport{}, core.NewInternal("炸了")
	}); err != nil {
		t.Fatalf("单账号失败不该让整批报错: %v", err)
	}
	if got := registry.Running(); got != 0 {
		t.Errorf("跑完必须把锁放掉，否则这个账号再也跑不起来，实得 %d 个还在跑", got)
	}

	// 再跑同一个账号：不该被判成"有人在跑"
	summary, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01")},
		ParamsDigest: "d0",
	}, &seqRandom{next: 9}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Skipped() != 0 || summary.Outcome != core.OutcomeSuccess {
		t.Errorf("锁应已释放，实得 skipped=%d outcome=%v", summary.Skipped(), summary.Outcome)
	}
}

func TestDifferentParamsDoNotBlockEachOther(t *testing.T) {
	store := storeWithAccounts(t)
	registry, _, runner := newRunner(t, store)

	rng := &seqRandom{next: 1}
	// 同一个账号、同一个能力，但**参数不同**
	holder := core.NewRunID(rng)
	key := mcloudexec.NewMutexKey("signin", runAccount("01").String(), "别的参数")
	if got := registry.Admit(holder, key); got.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("占用者应被受理，实得 %v", got.Kind)
	}

	summary, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01")},
		ParamsDigest: "d0",
	}, rng, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Skipped() != 0 {
		t.Errorf("参数不同是两件事，不该互相挡，实得 skipped=%d", summary.Skipped())
	}
}

func TestEmptyTargetsAreReportedAsFailed(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     nil,
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if summary.Outcome != core.OutcomeFailed {
		t.Errorf("一个账号都没跑不算成功，实得 %v", summary.Outcome)
	}
	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	if len(runs) != 0 {
		t.Errorf("没跑就不该有历史，实得 %d 行", len(runs))
	}
}

// TestTheTaskSeesTheAccountItWasGiven：
// 任务闭包拿到的是**计划里那个**账号，不是别的什么 ——
// 一旦传错，逐账号的互斥与历史归属会一起错位，而表面上仍然"跑成功了"。
func TestTheTaskSeesTheAccountItWasGiven(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	want := []core.AccountID{runAccount("01"), runAccount("02")}

	var seen []core.AccountID
	if _, err := runner.Run(context.Background(), RunPlan{
		Capability:   "signin",
		Accounts:     want,
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, account core.AccountID, _ RunProgress) (core.RunReport, error) {
		seen = append(seen, account)
		return okReport(), nil
	}); err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	if len(seen) != len(want) {
		t.Fatalf("应逐个账号各调一次，实得 %v", seen)
	}
	for i := range want {
		if seen[i] != want[i] {
			t.Errorf("第 %d 个账号应是 %v，实得 %v", i, want[i], seen[i])
		}
	}
}

// TestTheRunningRowIsVisibleWhileTheTaskIsStillGoing：
// 跑起来就该在历史里看得见，**而不是等它结束才突然出现**（长任务尤其如此）。
//
// ⚠ 这条是"先记 Running"唯一能被测到的地方：跑完之后那行必然是终态，
// 只看结果的话，"先插入 Running"与"最后直接写终态"长得一模一样
// —— 反向测试已确认：把 `StatusRunning` 改成 `StatusDone`，其余七条**一条都不变红**。
func TestTheRunningRowIsVisibleWhileTheTaskIsStillGoing(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()
	target := runAccount("01")

	var seen []core.RunRecord
	if _, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{target},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, account core.AccountID, _ RunProgress) (core.RunReport, error) {
		runs, err := store.ListRuns(ctx, runScope(), 10)
		if err != nil {
			return core.RunReport{}, err
		}
		for _, run := range runs {
			if run.AccountID != nil && *run.AccountID == account {
				seen = append(seen, run)
			}
		}
		return okReport(), nil
	}); err != nil {
		t.Fatalf("运行失败: %v", err)
	}

	if len(seen) != 1 {
		t.Fatalf("任务进行中就该看得见自己那一行，实得 %d 行", len(seen))
	}
	if seen[0].Status != core.StatusRunning {
		t.Errorf("进行中那一行应是 Running，实得 %v", seen[0].Status)
	}
	if seen[0].FinishedAt != nil {
		t.Errorf("还没跑完不该有结束时刻，实得 %v", seen[0].FinishedAt)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 异步触发面：预分配的 runId 与过程话
//
// 这两件都是 `POST /api/runs` 带来的，而它带来的**只有**这两件：
// 响应必须在动手之前就把 runId 交出去（前端拿它订阅事件流），
// 而一条要跑几十秒的任务必须往那个流里写过程话。
// ─────────────────────────────────────────────────────────────────────────────

// TestPreallocatedRunIDsAreUsedInOrder 钉住"预分配就是最终 id"。
//
// ⚠ 若这里回退成"跑到哪个才分配哪个"，异步那条路会坏得很难查：
// 响应里给出的第二个 id 与库里那一行的 id **不是同一个**，
// 于是前端订阅第一个账号能出日志、订阅第二个永远空白，
// 而后端看起来什么都没做错（它确实跑完了）。
func TestPreallocatedRunIDsAreUsedInOrder(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()

	// 分配用的随机源与运行用的**不是同一个**：这样"实得 id 等于预分配 id"
	// 才不是因为两边恰好抽到了同一串字节。
	alloc := &seqRandom{next: 200}
	want := []core.RunID{core.NewRunID(alloc), core.NewRunID(alloc)}

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d0",
		RunIDs:       want,
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	ran := summary.Ran()
	if len(ran) != 2 {
		t.Fatalf("两个都该跑，实得 %d", len(ran))
	}
	for i := range want {
		if ran[i].RunID != want[i] {
			t.Errorf("第 %d 个账号的 runId 应是预分配那个 %v，实得 %v", i, want[i], ran[i].RunID)
		}
	}

	// 库里的行也必须是这两个 id（历史页与事件流靠同一个 id 对上）。
	runs, err := store.ListRuns(ctx, runScope(), 10)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	byID := map[core.RunID]bool{}
	for _, run := range runs {
		byID[core.RunID(run.ID)] = true
	}
	for _, id := range want {
		if !byID[id] {
			t.Errorf("库里没有 runId %v 那一行", id)
		}
	}
}

// TestMismatchedPreallocatedRunIDsFailBeforeAnything 钉住"数量不等不猜、不补齐"。
//
// 少给一个的话最容易"顺手"用随机源补上，而那会让**响应里给出的 id**
// 与**实际用的 id**在最后一个账号上不一致 —— 表现是"最后一个账号永远没日志"。
// 空 runId 更糟：它会落进 `runs` 主键，成为一条谁也查不到的记录。
func TestMismatchedPreallocatedRunIDsFailBeforeAnything(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()

	_, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d0",
		RunIDs:       []core.RunID{core.NewRunID(&seqRandom{next: 7})},
	}, &seqRandom{next: 1}, func(context.Context, core.AccountID, RunProgress) (core.RunReport, error) {
		t.Fatal("参数不合法时一个账号都不该开跑")
		return core.RunReport{}, nil
	})
	if err == nil {
		t.Fatal("数量不等必须报错")
	}
	if !strings.Contains(err.Error(), "runId") {
		t.Errorf("报错要点名 runId，实得 %v", err)
	}
	runs, lerr := store.ListRuns(ctx, runScope(), 10)
	if lerr != nil {
		t.Fatalf("列举历史失败: %v", lerr)
	}
	if len(runs) != 0 {
		t.Errorf("失败要在动手之前，库里不该留下一行，实得 %d", len(runs))
	}
}

// TestProgressMessagesBecomeLogEventsInBothChannels 钉住过程话**同时**走两条路。
//
// 只发总线不落库 ⇒ 刷新页面日志就没了；只落库不发总线 ⇒ 前端得轮询数据库。
// 两边都写才叫"在线能看到、事后也查得到"，所以这里两条都要断言。
//
// ⚠ 还有一处容易写错的是**序号**：过程话必须与 `started` 走同一次发布，
// 序号由总线统一发。各自开一条写路径的话，`(run_id, seq)` 会撞或跳号，
// 而 SSE 的 `?after=` 续传读的就是那个 seq。
func TestProgressMessagesBecomeLogEventsInBothChannels(t *testing.T) {
	store := storeWithAccounts(t)
	_, bus, runner := newRunner(t, store)
	ctx := context.Background()
	target := runAccount("01")

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{target},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, _ core.AccountID, progress RunProgress) (core.RunReport, error) {
		progress(core.LevelInfo, "开门中…")
		progress(core.LevelWarn, "该主播有 2 间在播，本次只听第一间")
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	runID := summary.Ran()[0].RunID

	events, err := store.RunEventsAfter(ctx, runScope(), runID, nil, 100)
	if err != nil {
		t.Fatalf("读事件失败: %v", err)
	}

	// 落库那一份：两条 log 都在，级别与消息一字不差，且夹在 started 与 finished 之间。
	var logged []core.RunEvent
	for _, item := range events {
		if item.Event.Kind == core.EventLog {
			logged = append(logged, item.Event)
		}
	}
	if len(logged) != 2 {
		t.Fatalf("应有两条 log 事件，实得 %d（全部事件：%v）", len(logged), events)
	}
	if logged[0].Level != core.LevelInfo || logged[0].Message != "开门中…" {
		t.Errorf("第一条 log 不符：%+v", logged[0])
	}
	if logged[1].Level != core.LevelWarn || !strings.Contains(logged[1].Message, "只听第一间") {
		t.Errorf("第二条 log 不符：%+v", logged[1])
	}
	if logged[0].RunID != runID {
		t.Errorf("log 事件要挂在**这个账号**的 runId 上，实得 %v", logged[0].RunID)
	}
	// 顺序：started → log → log → progress/finished，seq 严格递增。
	var prev uint64
	for i, item := range events {
		if i > 0 && uint64(item.Seq) <= prev {
			t.Fatalf("序号必须递增，第 %d 条起不对：%v", i, events)
		}
		prev = uint64(item.Seq)
	}
	if events[0].Event.Kind != core.EventStarted {
		t.Errorf("第一条仍是 started，实得 %v", events[0].Event.Kind)
	}

	// 总线那一份（在线订阅者走这条路）：内容必须与库里**同源同序**。
	replayed := bus.Replay(runID, core.Seq(0))
	if len(replayed) != len(events) {
		t.Fatalf("总线与库条数不一致（%d 对 %d），说明有一边是另写的",
			len(replayed), len(events))
	}
	for i := range events {
		if replayed[i].Event.Kind != events[i].Event.Kind ||
			replayed[i].Seq != events[i].Seq {
			t.Errorf("第 %d 条两边不同：总线 %+v / 库 %+v", i+1, replayed[i], events[i])
		}
	}
}

// TestProgressOfTheRightAccount：过程话只能进**自己那个**账号的流。
//
// 串行跑两个账号时，如果 progress 闭包抓住了上一个账号的 runId，
// 现象是"日志混在别人的那一行下面"—— 界面上看着完全正常，
// 而只有把两个账号的消息写成不同的时候才会暴露。
func TestProgressOfTheRightAccount(t *testing.T) {
	store := storeWithAccounts(t)
	_, _, runner := newRunner(t, store)
	ctx := context.Background()

	summary, err := runner.Run(ctx, RunPlan{
		Capability:   "signin",
		Accounts:     []core.AccountID{runAccount("01"), runAccount("02")},
		ParamsDigest: "d0",
	}, &seqRandom{next: 1}, func(_ context.Context, account core.AccountID, progress RunProgress) (core.RunReport, error) {
		progress(core.LevelInfo, "只属于"+account.String())
		return okReport(), nil
	})
	if err != nil {
		t.Fatalf("运行失败: %v", err)
	}
	ran := summary.Ran()
	if len(ran) != 2 {
		t.Fatalf("两个都该跑，实得 %d", len(ran))
	}
	for _, entry := range ran {
		events, err := store.RunEventsAfter(ctx, runScope(), entry.RunID, nil, 100)
		if err != nil {
			t.Fatalf("读事件失败: %v", err)
		}
		var messages []string
		for _, item := range events {
			if item.Event.Kind == core.EventLog {
				messages = append(messages, item.Event.Message)
			}
		}
		if len(messages) != 1 || messages[0] != "只属于"+entry.Account.String() {
			t.Errorf("账号 %v 的流里出现了别人的过程话：%v", entry.Account, messages)
		}
	}
}
