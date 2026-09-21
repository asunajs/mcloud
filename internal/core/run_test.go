// 运行语义的测试。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/run.rs 的 `#[cfg(test)]` 模块，
// **外加**四张枚举真值表（见文件末尾）—— Go 没有 Rust 的 `match` 穷尽性检查，
// "新增一个态、某个 switch 没处理"不再有编译错误，那道保证由表来兜。
package core

import (
	"encoding/json"
	"math"
	"strings"
	"testing"
)

// runRng 是 run.rs 测试里那个随机源：`next = next * 31 + 7`。
//
// ⚠ 它与本包 crypto_test.go 里的 [SeededRng]（splitmix64）**不是同一个东西** ——
// Rust 侧不同模块的测试各自定义了 `SeededRng`，实现并不相同。
// 混用会让 runId 对不上，而现象只是"每次跑出来的 id 不一样"，
// 排查时很容易被当成"UUID 本来就该随机"。
type runRng struct{ s uint8 }

func newRunRng() *runRng { return &runRng{s: 3} }

// FillBytes 实现 [Random]。
//
// ⚠ `* 31 + 7` 在 uint8 上的回绕是 Go 的**定义行为**，
// 与 Rust 的 `wrapping_mul(31).wrapping_add(7)` 一致 —— 那边必须写成 wrapping_*，
// 因为 debug 构建下 Rust 的 `*` 和 `+` 溢出会 panic。
func (r *runRng) FillBytes(dst []byte) {
	for i := range dst {
		r.s = r.s*31 + 7
		dst[i] = r.s
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// RunID
// ─────────────────────────────────────────────────────────────────────────────

func TestRunIDIsUUIDv4Shaped(t *testing.T) {
	id := NewRunID(newRunRng())
	if len(id.String()) != 36 {
		t.Fatalf("runId 应为 36 字符，实得 %d：%s", len(id.String()), id)
	}
	if !isUUIDv4Shape(id.String()) {
		t.Fatalf("自产的 runId 应当过形状校验：%s", id)
	}
	if id.String()[14] != '4' {
		t.Fatalf("版本位必须是 4，实得 %q：%s", id.String()[14], id)
	}
}

func TestSameSeedSameRunID(t *testing.T) {
	// 注入随机源的意义：同种子必须同 runId，否则"幂等重放"这条语义没法钉。
	if a, b := NewRunID(newRunRng()), NewRunID(newRunRng()); a != b {
		t.Fatalf("同种子应得到同一 runId：%s vs %s", a, b)
	}
}

func TestParseAcceptsGeneratedAndRejectsJunk(t *testing.T) {
	id := NewRunID(newRunRng())
	if got, ok := ParseRunID(id.String()); !ok || got != id {
		t.Fatalf("自产的 runId 应能被 parse 回来：ok=%v got=%s want=%s", ok, got, id)
	}

	// 形状不对的一律拒掉 —— 这些串都不可能是我们生成的。
	for _, bad := range []string{
		"",
		"abc",
		"not-a-uuid-at-all-but-right-length!",  // 38 字符
		"00000000-0000-0000-0000-000000000000", // 版本位不是 4
		"550e8400e29b41d4a716446655440000",     // 无连字符
	} {
		if _, ok := ParseRunID(bad); ok {
			t.Fatalf("不该接受: %q", bad)
		}
	}
}

func TestParseNormalizesCase(t *testing.T) {
	const upper = "550E8400-E29B-41D4-A716-446655440000"
	parsed, ok := ParseRunID(upper)
	if !ok {
		t.Fatalf("大写应被接受: %q", upper)
	}
	if parsed.String() != "550e8400-e29b-41d4-a716-446655440000" {
		t.Fatalf("应归一到小写，实得 %s", parsed)
	}
}

func TestRunIDSerializesTransparently(t *testing.T) {
	id := NewRunID(newRunRng())
	raw, err := json.Marshal(id)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	// 透明包装：JSON 里就是裸字符串，不是 {"0": "..."}
	if want := `"` + id.String() + `"`; string(raw) != want {
		t.Fatalf("实得 %s，期望 %s", raw, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// RunReport
// ─────────────────────────────────────────────────────────────────────────────

func TestManualRequiredWithoutStepsDegradesToPartial(t *testing.T) {
	// 不说"要人做什么"的 ManualRequired 等于旧项目那句没人看的日志。
	r := NewRunReportManualRequired(nil, "有事要做")
	if r.Outcome != OutcomePartialSuccess {
		t.Fatalf("空 steps 应降级为 PartialSuccess，实得 %v", r.Outcome)
	}
	if len(r.ManualSteps) != 0 {
		t.Fatalf("降级后不该带 steps，实得 %v", r.ManualSteps)
	}
}

func TestManualRequiredWithStepsKeepsOutcome(t *testing.T) {
	step := NewManualStep("msg-push", nil, "请前往 App 打开消息通知", TargetMobileApp)
	r := NewRunReportManualRequired([]ManualStep{step}, "需要你动一下手")
	if r.Outcome != OutcomeManualRequired {
		t.Fatalf("实得 %v，期望 ManualRequired", r.Outcome)
	}
	if len(r.ManualSteps) != 1 || r.ManualSteps[0] != step {
		t.Fatalf("steps 应当原样带回来，实得 %v", r.ManualSteps)
	}
	// 「需人工」在存储层仍是 Done（跑完了），业务结论另存 —— 别把两者混成一个字段。
	if got := r.Outcome.ToLegacyStatus(); got != StatusDone {
		t.Fatalf("需人工的那一趟确实跑完了，status 应为 done，实得 %v", got)
	}
}

func TestNormalizeCatchesHandBuiltSuccessWithSteps(t *testing.T) {
	// 字段是导出的，字面量能绕过构造函数 —— 这条守住"成功却有活要干"。
	handBuilt := RunReport{
		Outcome:     OutcomeSuccess,
		ManualSteps: []ManualStep{NewManualStep("notice", nil, "短信通知未开启", TargetWeb)},
		Summary:     nil,
	}
	if got := handBuilt.Normalize().Outcome; got != OutcomeManualRequired {
		t.Fatalf("实得 %v，期望被纠回 ManualRequired", got)
	}
}

func TestNormalizeLeavesCleanSuccessAlone(t *testing.T) {
	if got := NewRunReportSuccess().Normalize().Outcome; got != OutcomeSuccess {
		t.Fatalf("干净的成功不该被改动，实得 %v", got)
	}
}

func TestSuccessLikeExcludesManualRequired(t *testing.T) {
	if !OutcomeSuccess.IsSuccessLike() || !OutcomePartialSuccess.IsSuccessLike() {
		t.Fatal("Success 与 PartialSuccess 都该算有产出")
	}
	// 关键：需人工**不算**成功 —— 目标尚未达成。
	if OutcomeManualRequired.IsSuccessLike() {
		t.Fatal("需人工不该算成功（再跑一遍也没用）")
	}
	if OutcomeFailed.IsSuccessLike() {
		t.Fatal("Failed 不该算成功")
	}
}

func TestOnlySuccessSkipsNotification(t *testing.T) {
	if OutcomeSuccess.NeedsNotification() {
		t.Fatal("全成不该打扰人")
	}
	for _, o := range []RunOutcome{OutcomePartialSuccess, OutcomeManualRequired, OutcomeFailed} {
		if !o.NeedsNotification() {
			t.Fatalf("%v 应当通知", o)
		}
	}
}

func TestOnlyManualRequiredNeedsHumanAction(t *testing.T) {
	if !OutcomeManualRequired.NeedsHumanAction() {
		t.Fatal("ManualRequired 应当要求人动手")
	}
	// 部分失败值得通知，但不需要人动手 —— 重跑就行。
	for _, o := range []RunOutcome{OutcomePartialSuccess, OutcomeFailed} {
		if o.NeedsHumanAction() {
			t.Fatalf("%v 不需要人动手，重跑即可", o)
		}
	}
}

func TestOnlyFailedMapsToFailedStatus(t *testing.T) {
	if got := OutcomeFailed.ToLegacyStatus(); got != StatusFailed {
		t.Fatalf("实得 %v，期望 failed", got)
	}
	for _, o := range []RunOutcome{OutcomeSuccess, OutcomePartialSuccess, OutcomeManualRequired} {
		if got := o.ToLegacyStatus(); got != StatusDone {
			t.Fatalf("%v 的 status 应为 done，实得 %v", o, got)
		}
	}
}

func TestLegacyStatusFallbackIsLossyButSafe(t *testing.T) {
	if got := FromLegacyStatus(StatusFailed); got != OutcomeFailed {
		t.Fatalf("实得 %v，期望 failed", got)
	}
	// Done 无法区分三种成功态，退路一律给 Success —— 有损，只用于读旧数据。
	for _, s := range []RunStatus{StatusDone, StatusRunning} {
		if got := FromLegacyStatus(s); got != OutcomeSuccess {
			t.Fatalf("from(%v) 实得 %v，期望 success", s, got)
		}
	}
}

func TestRunReportRoundTripsThroughJSON(t *testing.T) {
	// ⚠ 账号 id 用合成标识（199 号段）而不是 Rust 测试里的 `15****42`：
	// 值本身不参与跨语言对拍，而真实形状的号码会被仓库的脱敏守卫拦下。
	acct := NewAccountID("19900001234")
	step := NewManualStep("backup-gift", &acct, "未开启备份，请前往 App 手动开启", TargetMobileApp)
	r := NewRunReportManualRequired([]ManualStep{step}, "有一件待办")

	raw, err := marshalJSON(r)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	// snake_case 是外部契约（进 JSON 与推送正文）。
	for _, want := range []string{`"manual_required"`, `"mobile_app"`, `"manual_steps"`} {
		if !strings.Contains(string(raw), want) {
			t.Fatalf("%s 里应当有 %s", raw, want)
		}
	}

	var back RunReport
	if err := json.Unmarshal(raw, &back); err != nil {
		t.Fatalf("反序列化失败: %v", err)
	}
	// ⚠ 逐字段比，不用 `back != r`：两个结构体各含一个指针字段
	// （`Summary *string` 与 `ManualStep.Account *AccountID`），
	// 直接用 `==` 比的是**指针**，往返之后必然不等 ——
	// 而这个测试关心的正是"解出来的值一样"，不是"指向同一个地址"。
	if back.Outcome != r.Outcome {
		t.Fatalf("outcome 实得 %v，期望 %v", back.Outcome, r.Outcome)
	}
	if len(back.ManualSteps) != 1 {
		t.Fatalf("manual_steps 应有 1 条，实得 %d", len(back.ManualSteps))
	}
	got := back.ManualSteps[0]
	if got.Capability != step.Capability || got.Instruction != step.Instruction || got.Target != step.Target {
		t.Fatalf("步骤往返不一致：\n实得 %+v\n期望 %+v", got, step)
	}
	if (got.Account == nil) != (step.Account == nil) {
		t.Fatalf("account 的 nil 性不一致：实得 %v，期望 %v", got.Account, step.Account)
	}
	if got.Account != nil && *got.Account != *step.Account {
		t.Fatalf("account 实得 %v，期望 %v", *got.Account, *step.Account)
	}
	if back.Summary == nil || *back.Summary != *r.Summary {
		t.Fatalf("summary 实得 %v，期望 %v", back.Summary, r.Summary)
	}
}

// TestManualStepsIsEmptyArrayNotJSONNull 钉住"空是 `[]` 而不是 `null`"。
//
// Rust 侧的 `Vec::new()` 序列化成 `[]`，而 Go 的 nil slice 是 `null` ——
// 前端 `steps.map(...)` 在 null 上直接炸，而 JSON 层面两条路都不报错。
func TestManualStepsIsEmptyArrayNotJSONNull(t *testing.T) {
	for name, r := range map[string]RunReport{
		"success":        NewRunReportSuccess(),
		"failed":         NewRunReportFailed("全挂了"),
		"degraded-empty": NewRunReportManualRequired(nil, "有事要做"),
	} {
		raw, err := marshalJSON(r)
		if err != nil {
			t.Fatalf("%s 序列化失败: %v", name, err)
		}
		if !strings.Contains(string(raw), `"manual_steps":[]`) {
			t.Fatalf("%s 应产出空数组而不是 null：%s", name, raw)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Seq
// ─────────────────────────────────────────────────────────────────────────────

func TestSeqStartsAtOneNotZero(t *testing.T) {
	seq := SeqStart()
	// 0 是"还没发过"的哨兵，所以第一条事件必须是 1。
	if seq.Value() != 0 {
		t.Fatalf("start 应为 0（哨兵），实得 %d", seq.Value())
	}
	if got := seq.Next().Value(); got != 1 {
		t.Fatalf("第一条事件应为 1，实得 %d", got)
	}
	if got := seq.Next().Value(); got != 2 {
		t.Fatalf("第二条应为 2，实得 %d", got)
	}
	// Next 要推进自身，不是只返回。
	if seq.Value() != 2 {
		t.Fatalf("Next 之后自身应为 2，实得 %d", seq.Value())
	}
}

// TestResumeAfterAndSeenAreOffByOne 存在的唯一理由：SeqSeen(n) 与 SeqResumeAfter(n)
// **差 1**，而它们是同一个参数。传错会静默跳过一条事件，排查时没人会想到这里。
func TestResumeAfterAndSeenAreOffByOne(t *testing.T) {
	// 「我已经看到 3 了」→ 下一个要给它的是 4。
	if got := SeqSeen(3).Value(); got != 3 {
		t.Fatalf("seen(3) 应为 3，实得 %d", got)
	}
	// 「我们发到 3 了」→ 下一个要取的是 4。
	if got := SeqResumeAfter(3).Value(); got != 4 {
		t.Fatalf("resume_after(3) 应为 4，实得 %d", got)
	}
	if SeqSeen(3) == SeqResumeAfter(3) {
		t.Fatal("两者不许相等 —— 相等就说明差 1 这件事被抹掉了")
	}
	if SeqStart() != SeqSeen(0) {
		t.Fatal("从头是 0，与 seen(0) 等价")
	}
}

func TestSeqSaturatesInsteadOfWrapping(t *testing.T) {
	seq := SeqSeen(math.MaxUint64)
	// 绕回 0 会让"seq > after"永远为假，这个 run 从此再也发不出事件。
	if got := seq.Next().Value(); got != math.MaxUint64 {
		t.Fatalf("饱和在上界，实得 %d", got)
	}
	if SeqResumeAfter(math.MaxUint64) != SeqSeen(math.MaxUint64) {
		t.Fatal("resume_after(MAX) 与 seen(MAX) 应同为'已经到头了'")
	}
}

func TestSeqRestoresFromStoredValues(t *testing.T) {
	if SeqFromStored(7) != SeqSeen(7) {
		t.Fatal("存的是序号本身，应原样还原")
	}
	// 脏数据退到"从头来"，而不是 panic 打断整次回放。
	if SeqFromStored(0) != SeqStart() || SeqFromStored(-3) != SeqStart() {
		t.Fatal("0 与负数都应退到 start")
	}
}

func TestSeqSerializesAsABareNumber(t *testing.T) {
	raw, err := json.Marshal(SeqSeen(12))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if string(raw) != "12" {
		t.Fatalf("透明包装，实得 %s，期望 12", raw)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// RunEvent
// ─────────────────────────────────────────────────────────────────────────────

func TestEventJSONShapeIsTheExternalContract(t *testing.T) {
	id := NewRunID(newRunRng())

	raw, err := marshalJSON(NewEventProgress(id, 3, 20))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	for _, want := range []string{`"type":"progress"`, `"runId"`, `"done":3`, `"total":20`} {
		if !strings.Contains(string(raw), want) {
			t.Fatalf("%s 里应当有 %s", raw, want)
		}
	}

	raw, err = marshalJSON(NewEventLog(id, LevelWarn, "风控退避"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	for _, want := range []string{`"type":"log"`, `"level":"warn"`, `"message":"风控退避"`} {
		if !strings.Contains(string(raw), want) {
			t.Fatalf("%s 里应当有 %s", raw, want)
		}
	}

	raw, err = marshalJSON(NewEventErrored(id, "又挂了"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if !strings.Contains(string(raw), `"type":"errored"`) {
		t.Fatalf("%s 里应当有 errored", raw)
	}
}

// TestEventDoesNotHTMLEscape 钉住"事件正文不被引擎转义"。
//
// ⚠ 这条有两个层面，缺一不可：
//  1. [RunEvent.MarshalJSON] 内部必须走 [marshalJSON]（SetEscapeHTML(false)）；
//  2. 调用方必须用 [marshalJSON] 而不是 `json.Marshal` ——
//     后者即便拿到已经不转义的字节，也会在 compact 阶段**再转义一次**。
//     所以"实现了 MarshalJSON"并不等于"输出不转义"，第二步同样要守住。
func TestEventDoesNotHTMLEscape(t *testing.T) {
	const msg = `需要 <b>手动</b> 处理 & 确认`
	raw, err := marshalJSON(NewEventLog(NewRunID(newRunRng()), LevelWarn, msg))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if strings.Contains(string(raw), `\u003c`) {
		t.Fatalf("走 marshalJSON 不该转义 HTML：%s", raw)
	}

	// 反向证据：`json.Marshal` 会把 MarshalJSON 的产出**再**转义一次 ——
	// 这就是"必须用 marshalJSON"的由来，不是风格问题。
	escaped, err := json.Marshal(NewEventLog(NewRunID(newRunRng()), LevelWarn, msg))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if !strings.Contains(string(escaped), `\u003c`) {
		t.Fatalf("预期 json.Marshal 会二次转义（本条用来证明约定是必要的），实得 %s", escaped)
	}
}

func TestEventKindMatchesTheJSONTag(t *testing.T) {
	id := NewRunID(newRunRng())
	events := []RunEvent{
		NewEventStarted(id, "sign"),
		NewEventLog(id, LevelInfo, "hi"),
		NewEventProgress(id, 1, 2),
		NewEventFinished(id, OutcomeSuccess, nil, nil),
		NewEventErrored(id, "boom"),
	}
	for _, ev := range events {
		raw, err := marshalJSON(ev)
		if err != nil {
			t.Fatalf("序列化失败: %v", err)
		}
		// 库里的 kind 列与 JSON 里的 type 必须同源 —— 漂移了就没法按 kind 查。
		if want := `"type":"` + ev.EventKind() + `"`; !strings.Contains(string(raw), want) {
			t.Fatalf("%s 里应当有 %s", raw, want)
		}
	}
}

// TestOnlyFinishedCarriesAnOutcome —— 终局事件至多两类，且 Errored **不带** outcome。
// 它表示"还没跑到能力层判定"，硬塞一个 Failed 是假装知道得比实际多。
func TestOnlyFinishedCarriesAnOutcome(t *testing.T) {
	id := NewRunID(newRunRng())

	fin := NewEventFinished(id, OutcomeManualRequired, nil, nil)
	if got, ok := fin.EventOutcome(); !ok || got != OutcomeManualRequired {
		t.Fatalf("Finished 应带 outcome：ok=%v got=%v", ok, got)
	}
	if !fin.IsTerminal() || fin.IsStarted() {
		t.Fatal("Finished 是终局、不是开场")
	}

	err := NewEventErrored(id, "又挂了")
	if _, ok := err.EventOutcome(); ok {
		t.Fatal("执行器出错时还没做业务判定，不该有 outcome")
	}
	if !err.IsTerminal() {
		t.Fatal("Errored 也算终局：执行器崩了就不会有后续事件")
	}

	started := NewEventStarted(id, "sign")
	if !started.IsStarted() || started.IsTerminal() {
		t.Fatal("Started 是开场、不是终局")
	}
	if _, ok := started.EventOutcome(); ok {
		t.Fatal("Started 不该有 outcome")
	}
}

func TestPercentIsIntegerAndGuardsZeroTotal(t *testing.T) {
	id := NewRunID(newRunRng())

	for _, c := range []struct {
		done, total uint32
		want        uint32
		ok          bool
	}{
		{3, 10, 30, true},
		{0, 10, 0, true},
		{10, 10, 100, true},
		// 除零：给 false 而不是 0/100 —— 一个假数字会让前端画出一条错的进度条。
		{0, 0, 0, false},
		// 三分之一：整数除法，不往 JSON 里塞 33.333333333333336。
		{1, 3, 33, true},
	} {
		got, ok := NewEventProgress(id, c.done, c.total).Percent()
		if ok != c.ok || (ok && got != c.want) {
			t.Fatalf("progress(%d,%d) = (%d,%v)，期望 (%d,%v)", c.done, c.total, got, ok, c.want, c.ok)
		}
	}

	// 非进度事件没有百分比。
	if _, ok := NewEventStarted(id, "sign").Percent(); ok {
		t.Fatal("Started 没有百分比")
	}
}

func TestFinishedEventRoundTripsWithManualSteps(t *testing.T) {
	step := NewManualStep("msg-push", nil, "请前往 App 手动打开消息通知", TargetMobileApp)
	summary := "3 个账号已签到，1 个需手动"
	ev := NewEventFinished(NewRunID(newRunRng()), OutcomeManualRequired, []ManualStep{step}, &summary)

	raw, err := marshalJSON(ev)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	var back RunEvent
	if err := json.Unmarshal(raw, &back); err != nil {
		t.Fatalf("反序列化失败: %v", err)
	}
	if back.Kind != EventFinished {
		t.Fatalf("应当是 Finished，实得 %v", back.Kind)
	}
	// 待办要一字不差地回来 —— 丢掉它，回看历史就只剩一句"需人工"。
	if len(back.ManualSteps) != 1 || back.ManualSteps[0] != step {
		t.Fatalf("待办没回来：%v", back.ManualSteps)
	}
	if back.Summary == nil || *back.Summary != summary {
		t.Fatalf("小结没回来：%v", back.Summary)
	}
}

func TestSequencedBindsSeqAndEventTogether(t *testing.T) {
	item := NewSequenced(SeqSeen(4), NewEventStarted(NewRunID(newRunRng()), "sign"))
	if item.Seq.Value() != 4 {
		t.Fatalf("seq 应为 4，实得 %d", item.Seq.Value())
	}
	if !item.Event.IsStarted() {
		t.Fatal("事件应当原样带着")
	}
	// 序号与事件**同批次确定**，所以能一起落库（(run_id, seq) 做主键）。
	if item.Seq != SeqSeen(4) {
		t.Fatal("seq 不该在取出时被改动")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 合成
// ─────────────────────────────────────────────────────────────────────────────

// TestCombiningReportsPicksTheStateThatDeservesAttention —— 多账号结果的合成：
// 优先级、空集合、以及"没人失败就算成功"都要钉住。
func TestCombiningReportsPicksTheStateThatDeservesAttention(t *testing.T) {
	report := func(o RunOutcome) RunReport {
		return RunReport{Outcome: o, ManualSteps: []ManualStep{}, Summary: nil}
	}
	ok := func() RunReport { return report(OutcomeSuccess) }
	bad := func() RunReport { return report(OutcomeFailed) }

	// 全成功 / 部分成功 / 全失败
	if got := CombineOutcomes([]RunReport{ok(), ok()}); got != OutcomeSuccess {
		t.Fatalf("全成功应合成 Success，实得 %v", got)
	}
	if got := CombineOutcomes([]RunReport{ok(), bad()}); got != OutcomePartialSuccess {
		t.Fatalf("一成一败应合成 PartialSuccess，实得 %v", got)
	}
	if got := CombineOutcomes([]RunReport{bad(), bad()}); got != OutcomeFailed {
		t.Fatalf("全败应合成 Failed，实得 %v", got)
	}

	// 要人动手的那一态**压过**失败：重跑可能好 vs 重跑一定不好。
	if got := CombineOutcomes([]RunReport{ok(), report(OutcomeManualRequired)}); got != OutcomeManualRequired {
		t.Fatalf("实得 %v，期望 ManualRequired", got)
	}
	if got := CombineOutcomes([]RunReport{bad(), report(OutcomeManualRequired)}); got != OutcomeManualRequired {
		t.Fatalf("两者都有时更该突出要人动手的那件事，实得 %v", got)
	}

	// 空集合不是成功 —— "一个账号都没跑成"必须在总判定里露出来。
	if got := CombineOutcomes(nil); got != OutcomeFailed {
		t.Fatalf("空集合应合成 Failed，实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ⭐ 枚举漏态守卫（Go 没有 Rust 的 match 穷尽性检查）
// ─────────────────────────────────────────────────────────────────────────────

// countKnownStates 数出"有多少个已知态"。
//
// 判据是各枚举的 String() 对越界值返回**空串**：从 0 往上走到第一个空串，
// 那个下标就是已知态的个数。
func countKnownStates(t *testing.T, label string, str func(int) string) int {
	t.Helper()
	const max = 64
	for i := 0; i < max; i++ {
		if str(i) == "" {
			return i
		}
	}
	t.Fatalf("%s：连着 %d 个值都有名字，String() 忘了对越界值返回空串", label, max)
	return 0
}

// TestRunOutcomeTruthTable 是 [RunOutcome] 四态的完整真值表。
//
// ⚠ 它补的是 Go 相对 Rust **少掉**的那道编译期保证：Rust 的 `enum + match`
// 让"新增一个态、某个分支没处理"变成编译错误；Go 的 `switch` 漏一个分支
// 不报错、会静默走 default。所以把每个态在每个判据下的取值摊成一张表 ——
// 新增态时表必须补一行，否则"已知态数"与"表长"不等，这条就红。
// （方案 §9 点名这是"最该补装 linter"的一条，在没装之前先用表兜住。）
func TestRunOutcomeTruthTable(t *testing.T) {
	table := []struct {
		outcome    RunOutcome
		name       string
		successish bool
		notify     bool
		human      bool
		status     RunStatus
	}{
		{OutcomeSuccess, "success", true, false, false, StatusDone},
		{OutcomePartialSuccess, "partial_success", true, true, false, StatusDone},
		{OutcomeManualRequired, "manual_required", false, true, true, StatusDone},
		{OutcomeFailed, "failed", false, true, false, StatusFailed},
	}

	known := countKnownStates(t, "RunOutcome", func(i int) string { return RunOutcome(i).String() })
	if known != len(table) {
		t.Fatalf("RunOutcome 有 %d 个已知态，真值表只有 %d 行 —— 新增的态没登记", known, len(table))
	}

	for _, row := range table {
		if got := row.outcome.String(); got != row.name {
			t.Fatalf("RunOutcome(%d).String() = %q，期望 %q", int(row.outcome), got, row.name)
		}
		if got := ParseRunOutcome(row.name); got != row.outcome {
			t.Fatalf("ParseRunOutcome(%q) = %v，期望 %v", row.name, got, row.outcome)
		}
		if got := row.outcome.IsSuccessLike(); got != row.successish {
			t.Fatalf("%s.IsSuccessLike() = %v，期望 %v", row.name, got, row.successish)
		}
		if got := row.outcome.NeedsNotification(); got != row.notify {
			t.Fatalf("%s.NeedsNotification() = %v，期望 %v", row.name, got, row.notify)
		}
		if got := row.outcome.NeedsHumanAction(); got != row.human {
			t.Fatalf("%s.NeedsHumanAction() = %v，期望 %v", row.name, got, row.human)
		}
		if got := row.outcome.ToLegacyStatus(); got != row.status {
			t.Fatalf("%s.ToLegacyStatus() = %v，期望 %v", row.name, got, row.status)
		}
	}
}

func TestManualTargetTruthTable(t *testing.T) {
	table := []struct {
		target ManualTarget
		name   string
	}{
		{TargetMobileApp, "mobile_app"},
		{TargetWeb, "web"},
		{TargetWebui, "webui"},
		{TargetCli, "cli"},
		{TargetUnspecified, "unspecified"},
	}

	known := countKnownStates(t, "ManualTarget", func(i int) string { return ManualTarget(i).String() })
	if known != len(table) {
		t.Fatalf("ManualTarget 有 %d 个已知态，真值表只有 %d 行", known, len(table))
	}
	for _, row := range table {
		if got := row.target.String(); got != row.name {
			t.Fatalf("ManualTarget(%d).String() = %q，期望 %q", int(row.target), got, row.name)
		}
		if got := ParseManualTarget(row.name); got != row.target {
			t.Fatalf("ParseManualTarget(%q) = %v，期望 %v", row.name, got, row.target)
		}
	}
	// 归不进去的情形要有地方放 —— 强行归类会把"我们没搞清"伪装成"我们知道"。
	if got := ParseManualTarget("whatever"); got != TargetUnspecified {
		t.Fatalf("未知名应落到 Unspecified，实得 %v", got)
	}
}

func TestLogLevelTruthTable(t *testing.T) {
	table := []struct {
		level LogLevel
		name  string
	}{
		{LevelInfo, "info"},
		{LevelWarn, "warn"},
		{LevelError, "error"},
	}

	known := countKnownStates(t, "LogLevel", func(i int) string { return LogLevel(i).String() })
	if known != len(table) {
		t.Fatalf("LogLevel 有 %d 个已知态，真值表只有 %d 行", known, len(table))
	}
	for _, row := range table {
		if got := row.level.String(); got != row.name {
			t.Fatalf("LogLevel(%d).String() = %q，期望 %q", int(row.level), got, row.name)
		}
		if got := ParseLogLevel(row.name); got != row.level {
			t.Fatalf("ParseLogLevel(%q) = %v，期望 %v", row.name, got, row.level)
		}
	}
	// 顺序即严重程度 —— Rust 侧有 `Ord`，Go 侧靠 iota 顺序，谁把 Error 挪前面就该挂。
	if !(LevelInfo < LevelWarn && LevelWarn < LevelError) {
		t.Fatal("LogLevel 的声明顺序即严重程度，不许调整")
	}
}

func TestRunStatusTruthTable(t *testing.T) {
	table := []struct {
		status RunStatus
		name   string
	}{
		{StatusRunning, "running"},
		{StatusDone, "done"},
		{StatusFailed, "failed"},
	}

	known := countKnownStates(t, "RunStatus", func(i int) string { return RunStatus(i).String() })
	if known != len(table) {
		t.Fatalf("RunStatus 有 %d 个已知态，真值表只有 %d 行", known, len(table))
	}
	for _, row := range table {
		if got := row.status.String(); got != row.name {
			t.Fatalf("RunStatus(%d).String() = %q，期望 %q", int(row.status), got, row.name)
		}
		if got := ParseRunStatus(row.name); got != row.status {
			t.Fatalf("ParseRunStatus(%q) = %v，期望 %v", row.name, got, row.status)
		}
	}
	// 零值必须是 Running：它回答"这一行还在跑吗"，读不出来时"还在跑"最保守。
	var zero RunStatus
	if zero != StatusRunning {
		t.Fatalf("RunStatus 的零值应是 Running，实得 %v", zero)
	}
}

func TestEventKindTruthTable(t *testing.T) {
	table := []struct {
		kind      RunEventKind
		name      string
		terminal  bool
		started   bool
		hasOutcom bool
	}{
		{EventStarted, "started", false, true, false},
		{EventLog, "log", false, false, false},
		{EventProgress, "progress", false, false, false},
		{EventFinished, "finished", true, false, true},
		{EventErrored, "errored", true, false, false},
	}

	known := countKnownStates(t, "RunEventKind", func(i int) string { return RunEventKind(i).String() })
	if known != len(table) {
		t.Fatalf("RunEventKind 有 %d 个已知态，真值表只有 %d 行", known, len(table))
	}
	for _, row := range table {
		if got := row.kind.String(); got != row.name {
			t.Fatalf("RunEventKind(%d).String() = %q，期望 %q", int(row.kind), got, row.name)
		}
		ev := RunEvent{Kind: row.kind}
		if got := ev.IsTerminal(); got != row.terminal {
			t.Fatalf("%s.IsTerminal() = %v，期望 %v", row.name, got, row.terminal)
		}
		if got := ev.IsStarted(); got != row.started {
			t.Fatalf("%s.IsStarted() = %v，期望 %v", row.name, got, row.started)
		}
		if _, ok := ev.EventOutcome(); ok != row.hasOutcom {
			t.Fatalf("%s.EventOutcome() 的 ok = %v，期望 %v", row.name, ok, row.hasOutcom)
		}
	}
}
