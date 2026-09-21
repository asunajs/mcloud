package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"sync/atomic"
	"testing"
)

// 本文件对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/stage.rs 的 12 条测试，
// 外加 Go 侧新长出来的（`errors.As` 那一条、阶段真值表）。
//
// ⚠ `RunStage` 的**整串**形状由 serde 产出、在 enum_test.go 里对拍
// （`TestTaggedEnumJSONMatchesSerdeVectors`）—— 这里不重复造期望值。

// stageNextSeed 是 Rust 侧 `stage.rs` 的 `rng()` 里那个**进程内计数器**。
//
// ⚠⚠ 为什么要它：直觉写法是 `func stageRng() *SeededRng { return NewSeededRng(7) }`
// —— 每次调用都从固定种子重建，于是**两次调用产出完全一样的序列**，
// `NewRunID(stageRng())` 连写两次拿到的是**同一个 id**。
// 那样"不同 run 的 id 不一样"这类断言会以最误导人的方式失败：
// 看起来像业务逻辑错了，其实是测试夹具错了。
//
// ⚠⚠ 还有一个更隐蔽的版本：我最初把 `stageRng` 直接别名成 `run_test.go` 的
// `newRunRng()`（那个 LCG 是 `s*31+7` 的 u8，**周期恰好是 16** —— 而 UUID
// 正是 16 字节）。同一个 rng 连取两次也照样撞，因为第二次正好绕回原点。
// ⇒ **测试夹具的 RNG 必须逐个模块对齐 Rust 侧各自的那一份，不能就近复用**。
// 这里 stage.rs 用的是 splitmix64（与 crypto_test.go 的 [SeededRng] 同算法），
// 与 run.rs 那份**不是同一个东西**。
var stageNextSeed uint64 = 1

func stageRng() *SeededRng {
	seed := atomic.LoadUint64(&stageNextSeed)
	atomic.StoreUint64(&stageNextSeed, seed+0x9E3779B97F4A7C15)
	return NewSeededRng(seed)
}

// stageStep 是 rust 侧 `fn step()` 的对照：一条"去 App 开消息通知"的待办。
func stageStep() ManualStep {
	return NewManualStep("msg-push", nil, "请前往 App 手动打开消息通知", TargetMobileApp)
}

// ─────────────────────────────────────────────────────────────────────────────
// 状态机
// ─────────────────────────────────────────────────────────────────────────────

func TestStartsRunningWithoutOutcome(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	if exec.Stage().Kind != StageRunning {
		t.Fatalf("实得 %v，期望 running", exec.Stage().Kind)
	}
	if _, ok := exec.Outcome(); ok {
		t.Fatal("还在跑就没有判定")
	}
	if exec.IsTerminal() {
		t.Fatal("running 不是终态")
	}
	if got := exec.Stage().ToLegacyStatus(); got != StatusRunning {
		t.Fatalf("实得 %v，期望 running", got)
	}
}

func TestSuccessSettlesToDone(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	stage, err := exec.Finish(NewRunReportSuccess())
	if err != nil {
		t.Fatalf("应能结算: %v", err)
	}
	if !exec.IsTerminal() {
		t.Fatal("结算后应是终态")
	}
	if got, _ := exec.Outcome(); got != OutcomeSuccess {
		t.Fatalf("实得 %v，期望 success", got)
	}
	// 关键：**Done 不等于成功** —— 阶段是 Done，判定才是 Success
	if stage.Kind != StageDone {
		t.Fatalf("实得阶段 %v，期望 done", stage.Kind)
	}
	if got := stage.ToLegacyStatus(); got != StatusDone {
		t.Fatalf("实得 %v，期望 done", got)
	}
}

// TestManualRequiredIsDoneButNotSuccess 是本模块存在的理由：
// **需人工必须能表达，且不能算成功**。
func TestManualRequiredIsDoneButNotSuccess(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "msg-push")
	if _, err := exec.Finish(NewRunReportManualRequired([]ManualStep{stageStep()}, "有一件待办")); err != nil {
		t.Fatalf("应能结算: %v", err)
	}

	got, _ := exec.Outcome()
	if got != OutcomeManualRequired {
		t.Fatalf("实得 %v，期望 manual_required", got)
	}
	// 存储层看起来是 Done（前端**不**显示红色报错）
	if s := exec.Stage().ToLegacyStatus(); s != StatusDone {
		t.Fatalf("实得 %v，期望 done", s)
	}
	// 但业务上**不是**成功
	if got.IsSuccessLike() {
		t.Fatal("manual_required 不该算成功")
	}
	// 而且必须能推送
	if !got.NeedsNotification() {
		t.Fatal("manual_required 必须能推送")
	}
	if n := len(exec.Stage().StageManualSteps()); n != 1 {
		t.Fatalf("待办应有 1 条，实得 %d", n)
	}
}

// TestEmptyManualRequiredViaConstructorSettlesAsPartialSuccess 构造函数路径：
// 空手说"需人工" → 上游已经把它降级成部分成功，所以这条**不该**报错。
//
// 降级不是错误，是设计。真正会被拒的是**字面量构造**那条路，见下一条测试。
// 两条合起来才说明"空待办的需人工进不了库"：一条管构造函数路径（降级），
// 一条管绕过构造函数路径（报错）。
func TestEmptyManualRequiredViaConstructorSettlesAsPartialSuccess(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "msg-push")
	stage, err := exec.Finish(NewRunReportManualRequired(nil, "没了"))
	if err != nil {
		t.Fatalf("构造函数已经降级过了，此处应当顺利结算: %v", err)
	}
	if got, _ := exec.Outcome(); got != OutcomePartialSuccess {
		t.Fatalf("实得 %v，期望 partial_success", got)
	}
	if !exec.IsTerminal() {
		t.Fatal("结算之后就是终态")
	}
	if s := stage.ToLegacyStatus(); s != StatusDone {
		t.Fatalf("实得 %v，期望 done", s)
	}
	if len(stage.StageManualSteps()) != 0 {
		t.Fatal("不该有待办")
	}
	// 关键：**不是**需人工 —— 否则前端会弹一个"请去做某事"却说不出是什么的提示
	if got, _ := exec.Outcome(); got == OutcomeManualRequired {
		t.Fatal("不该被判成需人工")
	}
}

// TestHandBuiltManualRequiredWithoutStepsIsRejected 字面量绕过构造函数
// （字段是导出的）—— `Normalize` 只升级不降级，所以这种"空待办的需人工"
// 会一路穿到 `Finish`，必须在这里被拒。
//
// 这条是 [StageErrManualRequiredWithoutSteps] **唯一**的可达路径。
// 删掉那条校验就等于：只要不用构造函数，旧项目那句没人看的日志就能进库。
func TestHandBuiltManualRequiredWithoutStepsIsRejected(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "msg-push")
	handBuilt := RunReport{
		Outcome:     OutcomeManualRequired,
		ManualSteps: []ManualStep{}, // 空手
		Summary:     strPtr("我也说不清要做什么"),
	}
	if _, err := exec.Finish(handBuilt); !StageErrorIs(err, StageErrManualRequiredWithoutSteps) {
		t.Fatalf("手写的空待办需人工必须被拒，实得 %v", err)
	}
	// 被拒之后**不能**变成终态 —— 调用方还能补上待办重试
	if exec.IsTerminal() {
		t.Fatal("被拒之后不该变成终态")
	}
	if exec.Stage().Kind != StageRunning {
		t.Fatalf("实得 %v，期望仍在 running", exec.Stage().Kind)
	}
}

func TestFailedRequiresASummary(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	if _, err := exec.Finish(NewRunReportFailed("")); !StageErrorIs(err, StageErrFailedWithoutSummary) {
		t.Fatalf("空说明应被拒，实得 %v", err)
	}
	if exec.IsTerminal() {
		t.Fatal("被拒之后不该变成终态")
	}

	// 只有空白也算空
	if _, err := exec.Finish(NewRunReportFailed("   ")); !StageErrorIs(err, StageErrFailedWithoutSummary) {
		t.Fatalf("空白也算空，实得 %v", err)
	}

	if _, err := exec.Finish(NewRunReportFailed("上游 502")); err != nil {
		t.Fatalf("有说明就能过: %v", err)
	}
	if got, _ := exec.Outcome(); got != OutcomeFailed {
		t.Fatalf("实得 %v，期望 failed", got)
	}
	if s := exec.Stage().ToLegacyStatus(); s != StatusFailed {
		t.Fatalf("实得 %v，期望 failed", s)
	}
}

// TestTerminalCannotTransitionAgain 终态不可再迁出 ——
// 否则前端会一直转圈（旧项目真出现过）。
func TestTerminalCannotTransitionAgain(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	if _, err := exec.Finish(NewRunReportSuccess()); err != nil {
		t.Fatalf("第一次应成功: %v", err)
	}

	_, err := exec.Finish(NewRunReportFailed("改判"))
	if !StageErrorIs(err, StageErrAlreadyTerminal) {
		t.Fatalf("终态后不能再 finish，实得 %v", err)
	}
	var se *StageError
	if !asStageError(err, &se) || se.Current != "done" || se.Attempted != "finish" {
		t.Fatalf("current/attempted 应带上细节，实得 %v", err)
	}
	if got, _ := exec.Outcome(); got != OutcomeSuccess {
		t.Fatalf("判定不该被改掉，实得 %v", got)
	}

	if _, err := exec.Fail("又挂了"); !StageErrorIs(err, StageErrAlreadyTerminal) {
		t.Fatalf("终态后不能再 fail，实得 %v", err)
	}
}

func TestExecutorFailureNeedsOnlyASummary(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	if _, err := exec.Fail("执行器内部错误"); err != nil {
		t.Fatalf("应能失败: %v", err)
	}
	if got, _ := exec.Outcome(); got != OutcomeFailed {
		t.Fatalf("实得 %v，期望 failed", got)
	}
	if s := exec.Stage().ToLegacyStatus(); s != StatusFailed {
		t.Fatalf("实得 %v，期望 failed", s)
	}
	// 执行器失败时还没有能力判定，所以没有待办
	if n := len(exec.Stage().StageManualSteps()); n != 0 {
		t.Fatalf("执行器失败不该带待办，实得 %d", n)
	}
}

// TestTerminalEventIsEmittedAtMostOnce 终局事件**至多且必须最后**一条。
func TestTerminalEventIsEmittedAtMostOnce(t *testing.T) {
	id := NewRunID(stageRng())
	exec := NewRunExecution(id, "sign")

	if err := exec.Record(NewEventStarted(id, "sign")); err != nil {
		t.Fatalf("开始事件可以记: %v", err)
	}
	if err := exec.Record(NewEventProgress(id, 1, 3)); err != nil {
		t.Fatalf("进度可以记: %v", err)
	}
	if exec.HasEmittedTerminal() {
		t.Fatal("还没发终局")
	}

	if err := exec.Record(NewEventFinished(id, OutcomeSuccess, nil, nil)); err != nil {
		t.Fatalf("终局事件可以记一次: %v", err)
	}
	if !exec.HasEmittedTerminal() {
		t.Fatal("已发终局")
	}

	// 第二次终局事件必须被拒
	if err := exec.Record(NewEventErrored(id, "又挂了")); !StageErrorIs(err, StageErrAlreadyTerminal) {
		t.Fatalf("终局事件不得发第二次，实得 %v", err)
	}
	// 终局之后的普通日志也不行 —— 否则前端会在收到 result 后又冒出日志
	if err := exec.Record(NewEventLog(id, LevelInfo, "迟到的日志")); !StageErrorIs(err, StageErrAlreadyTerminal) {
		t.Fatalf("终局后不该再有事件，实得 %v", err)
	}
}

func TestEventsFromAnotherRunAreRejected(t *testing.T) {
	// ⚠ 刻意两次**各自**取一个新的 rng（与 Rust 侧逐字一致），而不是
	// "同一个 rng 连取两次"。后者也能产出不同的 id，但它**绕开**了上面那条
	// 陷阱而不是守住它 —— 于是有人把计数器改回固定种子时，这条测试照样绿。
	//
	// （这是我第一版的写法：改成单 rng 后反向测试就不再变红，
	//   即断言**没有承重**而我不知道。反向测试是唯一能发现这件事的手段。）
	mine := NewRunID(stageRng())
	theirs := NewRunID(stageRng())
	if mine == theirs {
		t.Fatal("测试 RNG 要能产出不同 id")
	}
	exec := NewRunExecution(mine, "sign")
	if err := exec.Record(NewEventStarted(theirs, "sign")); !StageErrorIs(err, StageErrRunIDMismatch) {
		t.Fatalf("别的 run 的事件不该记到这条执行上，实得 %v", err)
	}
}

// TestTerminalEventOutcomeMatchesStage 事件与阶段两条线要能对上：
// 终局事件里的 outcome 就是阶段判定。
func TestTerminalEventOutcomeMatchesStage(t *testing.T) {
	id := NewRunID(stageRng())
	exec := NewRunExecution(id, "msg-push")
	if err := exec.Record(NewEventStarted(id, "msg-push")); err != nil {
		t.Fatalf("ok: %v", err)
	}

	report := NewRunReportManualRequired([]ManualStep{stageStep()}, "有待办")
	if _, err := exec.Finish(report); err != nil {
		t.Fatalf("应能结算: %v", err)
	}
	ev := NewEventFinished(id, report.Outcome, report.ManualSteps, report.Summary)
	if err := exec.Record(ev); err != nil {
		t.Fatalf("终局事件应能记: %v", err)
	}

	evOutcome, ok := ev.EventOutcome()
	if !ok {
		t.Fatal("终局事件应有判定")
	}
	stageOutcome, _ := exec.Outcome()
	if evOutcome != stageOutcome {
		t.Fatalf("事件与阶段必须一致：事件 %v，阶段 %v", evOutcome, stageOutcome)
	}
	if evOutcome != OutcomeManualRequired {
		t.Fatalf("实得 %v，期望 manual_required", evOutcome)
	}
}

func TestSeqWalksWithEvents(t *testing.T) {
	// 序号分配与事件发布是两件事，这里演示它们配合起来的样子
	id := NewRunID(stageRng())
	seq := SeqStart()
	exec := NewRunExecution(id, "sign")

	var emitted []uint64
	for _, ev := range []RunEvent{
		NewEventStarted(id, "sign"),
		NewEventProgress(id, 1, 2),
		NewEventFinished(id, OutcomeSuccess, nil, nil),
	} {
		if err := exec.Record(ev); err != nil {
			t.Fatalf("应能记: %v", err)
		}
		emitted = append(emitted, seq.Next().Value())
	}
	want := []uint64{1, 2, 3}
	for i := range want {
		if emitted[i] != want[i] {
			t.Fatalf("序号实得 %v，期望 %v", emitted, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Go 侧新长出来的守卫
// ─────────────────────────────────────────────────────────────────────────────

// TestStageErrorSurvivesWrapping 钉住 [StageErrorIs] 走 `errors.As`。
//
// Go 的惯例是 `fmt.Errorf("…: %w", err)` 包一层，裸类型断言在那种情况下
// **静默返回 false**（§6.1 #6）—— 于是"终态之后又结算了一次"这种
// 恰恰最该报警的事，会被当成普通错误处理掉。
func TestStageErrorSurvivesWrapping(t *testing.T) {
	exec := NewRunExecution(NewRunID(stageRng()), "sign")
	if _, err := exec.Finish(NewRunReportSuccess()); err != nil {
		t.Fatalf("第一次应成功: %v", err)
	}
	_, raw := exec.Finish(NewRunReportFailed("改判"))

	wrapped := fmt.Errorf("结算 sign 时出错: %w", raw)
	if !StageErrorIs(wrapped, StageErrAlreadyTerminal) {
		t.Fatal("包一层之后仍应认得出类别")
	}
	if StageErrorIs(wrapped, StageErrRunIDMismatch) {
		t.Fatal("不该误判成别的类别")
	}
}

// TestRunStageKindTruthTable 是 [RunStageKind] 三态的真值表。
//
// ⚠ `String()` 同时是 JSON 的 `stage` 标签与库里的 `stage` 列 ——
// 改这里等于改外部契约。
func TestRunStageKindTruthTable(t *testing.T) {
	table := []struct {
		kind   RunStageKind
		text   string
		status RunStatus
		term   bool
	}{
		{StageRunning, "running", StatusRunning, false},
		{StageDone, "done", StatusDone, true},
		{StageFailed, "failed", StatusFailed, true},
	}
	known := countKnownStates(t, "RunStageKind", func(i int) string { return RunStageKind(i).String() })
	if known != len(table) {
		t.Fatalf("RunStageKind 有 %d 个已知态，真值表只有 %d 行", known, len(table))
	}
	for _, row := range table {
		if got := row.kind.String(); got != row.text {
			t.Fatalf("RunStageKind(%d).String() = %q，期望 %q", int(row.kind), got, row.text)
		}
		if got := ParseRunStageKind(row.text); got != row.kind {
			t.Fatalf("Parse(%q) = %v，期望 %v", row.text, got, row.kind)
		}
		stage := RunStage{Kind: row.kind}
		if got := stage.ToLegacyStatus(); got != row.status {
			t.Fatalf("%s 的 status = %v，期望 %v", row.text, got, row.status)
		}
		if got := stage.IsTerminal(); got != row.term {
			t.Fatalf("%s 的 IsTerminal = %v，期望 %v", row.text, got, row.term)
		}
	}
	// ⚠ 未知值归 **running**：把一条真实在跑的记录误判成已结束，
	// 会让前端提前停掉转圈 —— 那比"一直转"更糟。
	if got := ParseRunStageKind("nonsense"); got != StageRunning {
		t.Fatalf("未知值实得 %v，期望 running", got)
	}
}

// TestFailedStageOutcomeIsAlwaysFailed 钉住"执行器崩了也只报 failed"。
//
// 能力可能根本没跑到能判定的地方，硬塞一个别的判定等于
// 假装我们知道得比实际多。
func TestFailedStageOutcomeIsAlwaysFailed(t *testing.T) {
	stage := NewRunStageFailed("崩了", nil)
	got, ok := stage.StageOutcome()
	if !ok || got != OutcomeFailed {
		t.Fatalf("实得 (%v, %v)，期望 (failed, true)", got, ok)
	}
	if !stage.IsFailed() {
		t.Fatal("failed 阶段应 IsFailed")
	}
	// ⚠ 但 `Done` + `ManualRequired` 在这个口径下**不算失败** ——
	// 前端"红不红"看这个，需人工不该显示成报错。
	done := NewRunStageDone(OutcomeManualRequired, []ManualStep{stageStep()}, nil)
	if done.IsFailed() {
		t.Fatal("需人工不该显示成失败")
	}
}

// TestRunStageRoundTripsThroughJSON 是编解码成对的自洽检查。
//
// 与 enum_test.go 里那条整串对拍互补：那条保证"和 serde 一样"，
// 这条保证"自己解得回来"。
func TestRunStageRoundTripsThroughJSON(t *testing.T) {
	steps := []ManualStep{stageStep()}
	cases := []RunStage{
		NewRunStageRunning(),
		NewRunStageDone(OutcomeManualRequired, steps, strPtr("有一件待办")),
		NewRunStageDone(OutcomeSuccess, nil, nil),
		NewRunStageFailed("上游 502", nil),
	}
	for _, want := range cases {
		raw, err := marshalJSON(want)
		if err != nil {
			t.Fatalf("%v 序列化失败: %v", want.Kind, err)
		}
		var back RunStage
		if err := json.Unmarshal(raw, &back); err != nil {
			t.Fatalf("%v 反序列化失败: %v", want.Kind, err)
		}
		if back.Kind != want.Kind {
			t.Fatalf("kind 实得 %v，期望 %v", back.Kind, want.Kind)
		}
		if len(back.ManualSteps) != len(want.ManualSteps) {
			t.Fatalf("%v 待办数实得 %d，期望 %d", want.Kind, len(back.ManualSteps), len(want.ManualSteps))
		}
		switch want.Kind {
		case StageDone:
			if back.Outcome != want.Outcome {
				t.Fatalf("outcome 实得 %v，期望 %v", back.Outcome, want.Outcome)
			}
			if (back.Summary == nil) != (want.Summary == nil) {
				t.Fatalf("summary 的 nil 性不一致")
			}
			if want.Summary != nil && *back.Summary != *want.Summary {
				t.Fatalf("summary 实得 %q，期望 %q", *back.Summary, *want.Summary)
			}
		case StageFailed:
			if back.Reason != want.Reason {
				t.Fatalf("reason 实得 %q，期望 %q", back.Reason, want.Reason)
			}
		}
	}
}

// asStageError 是 [errors.As] 的薄封装，只为测试里取细节方便。
func asStageError(err error, out **StageError) bool {
	*out = nil
	var se *StageError
	if !errors.As(err, &se) {
		return false
	}
	*out = se
	return true
}
