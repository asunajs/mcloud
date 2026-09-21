// 运行生命周期状态机 —— 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/stage.rs。
//
// # 为什么需要一个显式状态机
//
// 旧项目里"这次运行处于什么状态"是**由调用方各自维护**的：
// `runner.ts` 往 Map 里塞一个 promise、能力自己往 `state.json` 里写字段、
// WebUI 又从历史里推断。于是出现了三类典型的错：
//
//  1. **假成功**：能力内部 `logger.info("需手动完成: …")` 之后 `return true`，
//     外层据此写 `status = "success"`。运行记录上看起来一切正常。
//  2. **终态被覆盖**：跑完写了 `Done`，随后某条清理路径又写回 `Running`，
//     于是前端一直转圈。
//  3. **终态没有结果**：`Done` 但没有任何成果字段，前端只能显示"成功"两个字。
//
// 这三类错的共同点是**没有一个地方规定"什么状态才算跑完、跑完必须带什么"**。
// 本文件就是那个地方。
//
// # 与 [RunOutcome] 的关系
//
// [RunStage]（本文件）是**存储生命周期**，只有三个阶段：`Running` → `{Done, Failed}`，
// 用来回答"这一行还在跑吗"（前端转不转圈）。
//
// [RunOutcome] 是**业务终局判定**，四态。两者不是一对一：
//
//	Running ──finish(outcome)──┬── Done   （outcome ∈ {Success, PartialSuccess, ManualRequired}）
//	                           └── Failed （outcome == Failed）
//
// ⚠ `Done` **不等于成功** —— 这正是旧项目栽的地方：把 `Done` 读成"成功"，
// 于是"需人工"被吞掉。
package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// ─────────────────────────────────────────────────────────────────────────────
// RunStage
// ─────────────────────────────────────────────────────────────────────────────

// RunStageKind 是运行生命周期的三阶段。
type RunStageKind int

const (
	// StageRunning 表示正在跑。
	StageRunning RunStageKind = iota
	// StageDone 表示跑完了（**不是"成功"**，成败看 Outcome）。
	StageDone
	// StageFailed 表示挂了：执行器自己的失败，或能力的 [OutcomeFailed]。
	StageFailed
)

// String 返回阶段名 —— 它同时是 JSON 里的 `stage` 标签与库里的 `stage` 列。
//
// ⚠ 未知值返回空串（枚举漏态守卫的判据，见 [CapabilityKind.String] 那条注释）。
func (k RunStageKind) String() string {
	switch k {
	case StageRunning:
		return "running"
	case StageDone:
		return "done"
	case StageFailed:
		return "failed"
	}
	return ""
}

// ParseRunStageKind 从存储里读回。
//
// ⚠ 未知值归 [StageRunning]：**"还在跑"是最保守的读法** ——
// 它不会把一条真实在跑的记录误判成已结束（那会让前端提前停掉转圈，
// 而用户以为还在跑）。
func ParseRunStageKind(raw string) RunStageKind {
	switch raw {
	case "running":
		return StageRunning
	case "done":
		return StageDone
	case "failed":
		return StageFailed
	}
	return StageRunning
}

// RunStage 是运行的存储生命周期。
//
// # ⚠ Go 侧的形状取舍
//
// Rust 是 `#[serde(tag = "stage")]` 的枚举，三个变体的字段**各不相同**：
//
//	{"stage":"running"}
//	{"stage":"done","outcome":…,"manual_steps":[…],"summary":…}
//	{"stage":"failed","summary":…,"manual_steps":[…]}
//
// ⚠ 注意后两条的**字段顺序不同**（`Done` 是 outcome→manual_steps→summary，
// `Failed` 是 summary→manual_steps）。这不是随便排的，整串比对才看得出来，
// 逐字段比对会把这个差异整个放过。
//
// Go 没有 sum type，这里用"结构体 + Kind"近似，靠 [RunStage.MarshalJSON]
// 按 Kind 分派 —— 对外形状**仍然**是 tag 式枚举。
//
// # 两个"说明"字段为什么分开
//
// `Done.summary` 是 `Option<String>`（可为 null），`Failed.summary` 是 `String`
// （**必非空**，空说明会被 [RunExecution.Finish] 拒掉）。
// 合并成一个字段就表达不出"失败必须有说明"这条不变式了。
type RunStage struct {
	Kind RunStageKind `json:"-"`
	// Outcome 是终局的业务判定。仅 [StageDone] 用。
	Outcome RunOutcome `json:"-"`
	// ManualSteps 是需人工的待办。[StageDone] 与 [StageFailed] 都可能有 ——
	// 失败也可能伴随"你得去 App 里做点什么"。
	//
	// ⚠ 空的时候是 `[]` 不是 `null`（Rust 的 `Vec::new()`），见包注释。
	ManualSteps []ManualStep `json:"-"`
	// Summary 是 [StageDone] 的给使用者看的小结，可为 nil。
	Summary *string `json:"-"`
	// Reason 是 [StageFailed] 的说明，**必非空**。
	//
	// 面向使用者（**不含**内部细节，见 [ErrorKind.PublicMessage]）。
	Reason string `json:"-"`
}

// NewRunStageRunning 是"正在跑"。
func NewRunStageRunning() RunStage {
	return RunStage{Kind: StageRunning, ManualSteps: []ManualStep{}}
}

// NewRunStageDone 是"跑完了"。
//
// ⚠ 它**不表示成功** —— 成败看 `outcome`。刻意没有 `NewRunStageSuccess`
// 这种构造函数，就是为了不给人"Done == 成功"的错觉。
func NewRunStageDone(outcome RunOutcome, steps []ManualStep, summary *string) RunStage {
	if steps == nil {
		steps = []ManualStep{}
	}
	return RunStage{Kind: StageDone, Outcome: outcome, ManualSteps: steps, Summary: summary}
}

// NewRunStageFailed 是"挂了"。
func NewRunStageFailed(reason string, steps []ManualStep) RunStage {
	if steps == nil {
		steps = []ManualStep{}
	}
	return RunStage{Kind: StageFailed, ManualSteps: steps, Reason: reason}
}

// IsTerminal 报告是否终态。**终态不可再迁出** —— 否则前端会一直转圈。
func (s RunStage) IsTerminal() bool { return s.Kind != StageRunning }

// StageOutcome 返回终局的业务判定。还在跑时返回 ok=false。
//
// ⚠ [StageFailed] 固定返回 [OutcomeFailed]：执行器崩了的时候
// 能力根本没跑到能判定的地方，硬塞一个别的判定等于假装我们知道得比实际多。
func (s RunStage) StageOutcome() (RunOutcome, bool) {
	switch s.Kind {
	case StageDone:
		return s.Outcome, true
	case StageFailed:
		return OutcomeFailed, true
	}
	return OutcomeSuccess, false
}

// ToLegacyStatus 映射到存储用的三态（`runs.status` 列）。
//
// # 为什么要有这个映射
//
// `runs.status` 是既有的列，只有三个取值，同时被前端（"在跑吗"）与历史列表复用。
// 改列的取值范围会动到已落库的数据与前端契约，所以在**存储层**维持三态、
// 在**业务层**用四态。两者之间的翻译**只此一处**，避免各处自己 switch
// （那样必然会有人写出 `Done => Success` 从而丢掉 [OutcomeManualRequired]）。
func (s RunStage) ToLegacyStatus() RunStatus {
	switch s.Kind {
	case StageDone:
		return StatusDone
	case StageFailed:
		return StatusFailed
	}
	return StatusRunning
}

// StageManualSteps 返回待办列表（两种终态都可能有）。
func (s RunStage) StageManualSteps() []ManualStep { return s.ManualSteps }

// IsFailed 报告结算是成功还是失败 —— 按**阶段**而非 `outcome` 的口径。
//
// ⚠ 与 [RunOutcome.IsSuccessLike] **不同**：[OutcomeManualRequired] 在这个口径下
// 算"没失败"（阶段是 `Done`），但在 `RunOutcome` 口径下**不算成功**。
// 两个口径都需要、且不能混用：
//   - 前端"红不红"用这个（`ManualRequired` 不该显示成报错）；
//   - "这一趟目标达成了吗"用 [RunOutcome.IsSuccessLike]。
func (s RunStage) IsFailed() bool { return s.Kind == StageFailed }

// MarshalJSON 按 Kind 分派，产出与 Rust 的 `tag = "stage"` **逐字相同**的 JSON。
//
// ⚠ 走 [marshalJSON] 而不是 `json.Marshal`（后者默认转义 HTML）。
func (s RunStage) MarshalJSON() ([]byte, error) {
	switch s.Kind {
	case StageRunning:
		return marshalJSON(struct {
			Stage string `json:"stage"`
		}{s.Kind.String()})
	case StageDone:
		return marshalJSON(struct {
			Stage       string       `json:"stage"`
			Outcome     RunOutcome   `json:"outcome"`
			ManualSteps []ManualStep `json:"manual_steps"`
			Summary     *string      `json:"summary"`
		}{s.Kind.String(), s.Outcome, s.ManualSteps, s.Summary})
	case StageFailed:
		// ⚠ 顺序是 summary 在**前** —— 与 `Done` 不同，见类型注释。
		return marshalJSON(struct {
			Stage       string       `json:"stage"`
			Summary     string       `json:"summary"`
			ManualSteps []ManualStep `json:"manual_steps"`
		}{s.Kind.String(), s.Reason, s.ManualSteps})
	}
	return nil, fmt.Errorf("未知的阶段 %d", int(s.Kind))
}

// UnmarshalJSON 是 [RunStage.MarshalJSON] 的逆。
func (s *RunStage) UnmarshalJSON(data []byte) error {
	var probe struct {
		Stage string `json:"stage"`
	}
	if err := json.Unmarshal(data, &probe); err != nil {
		return err
	}
	switch probe.Stage {
	case "running":
		s.Kind, s.ManualSteps = StageRunning, []ManualStep{}
		s.Outcome, s.Summary, s.Reason = OutcomeSuccess, nil, ""
		return nil
	case "done":
		var v struct {
			Outcome     RunOutcome   `json:"outcome"`
			ManualSteps []ManualStep `json:"manual_steps"`
			Summary     *string      `json:"summary"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		s.Kind, s.Outcome, s.ManualSteps, s.Summary = StageDone, v.Outcome, v.ManualSteps, v.Summary
		s.Reason = ""
		return nil
	case "failed":
		var v struct {
			Summary     string       `json:"summary"`
			ManualSteps []ManualStep `json:"manual_steps"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		s.Kind, s.Reason, s.ManualSteps = StageFailed, v.Summary, v.ManualSteps
		s.Outcome, s.Summary = OutcomeSuccess, nil
		return nil
	}
	return fmt.Errorf("未知的阶段 %q", probe.Stage)
}

// ─────────────────────────────────────────────────────────────────────────────
// StageError
// ─────────────────────────────────────────────────────────────────────────────

// StageErrorKind 是生命周期迁移失败的四类。
type StageErrorKind int

const (
	// StageErrAlreadyTerminal 表示终态之后还想迁移。
	StageErrAlreadyTerminal StageErrorKind = iota
	// StageErrManualRequiredWithoutSteps 表示空手说"需人工"。
	StageErrManualRequiredWithoutSteps
	// StageErrFailedWithoutSummary 表示失败却不带说明。
	StageErrFailedWithoutSummary
	// StageErrRunIDMismatch 表示把别的 run 的事件记到这条执行上。
	StageErrRunIDMismatch
)

// StageError 是生命周期迁移的错误。
//
// # 为什么每次都**显式报错**而不是静默忽略
//
// 静默忽略会让"重复结算"看起来像正常操作，而重复结算往往意味着
// 同一个 runId 被执行了两次（**幂等闸失效**）——
// 那是要立刻知道的事，不是可以忽略的事。
type StageError struct {
	Kind StageErrorKind
	// Current / Attempted 是 [StageErrAlreadyTerminal] 的细节。
	Current   string
	Attempted string
	// Expected / Actual 是 [StageErrRunIDMismatch] 的细节。
	Expected RunID
	Actual   RunID
}

// Error 实现 error。文案与 Rust 侧 `#[error(…)]` 逐字对齐。
func (e *StageError) Error() string {
	switch e.Kind {
	case StageErrAlreadyTerminal:
		return fmt.Sprintf("运行已在终态（%s），不能再迁移到 %s", e.Current, e.Attempted)
	case StageErrManualRequiredWithoutSteps:
		return "需人工（ManualRequired）必须带至少一条待办说明，否则与旧项目那句没人看的日志等价"
	case StageErrFailedWithoutSummary:
		return "失败必须带说明，空说明等于把错误吞掉"
	case StageErrRunIDMismatch:
		return fmt.Sprintf("runId 不匹配：期望 %s，实得 %s", e.Expected, e.Actual)
	}
	return "未知的状态机错误"
}

// StageErrorIs 报告 err 是否是指定类别的 [StageError]。
//
// ⚠ 走 [errors.As] 而不是裸类型断言：Go 的惯例是用 `fmt.Errorf("…: %w", err)`
// 包一层，裸断言在那种情况下**静默返回 false**（§6.1 #6）。
func StageErrorIs(err error, kind StageErrorKind) bool {
	var se *StageError
	if !errors.As(err, &se) {
		return false
	}
	return se.Kind == kind
}

// ─────────────────────────────────────────────────────────────────────────────
// RunExecution
// ─────────────────────────────────────────────────────────────────────────────

// RunExecution 是一次运行的可变状态。
//
// 用法：
//
//	exec := NewRunExecution(NewRunID(rng), "sign")
//	_ = exec.Record(NewEventProgress(id, 1, 3))
//	stage, _ := exec.Finish(NewRunReportSuccess())
type RunExecution struct {
	runID      RunID
	capability string
	stage      RunStage
	// emittedTerminal 记录是否已发过终局事件 —— 用来守住"终局之后不再发布"。
	emittedTerminal bool
}

// NewRunExecution 开始一次运行，初始阶段是 [StageRunning]。
func NewRunExecution(runID RunID, capability string) *RunExecution {
	return &RunExecution{runID: runID, capability: capability, stage: NewRunStageRunning()}
}

// RunID 返回这次运行的标识。
func (e *RunExecution) RunID() RunID { return e.runID }

// Capability 返回能力 code。
func (e *RunExecution) Capability() string { return e.capability }

// Stage 返回当前阶段的**快照**。
//
// ⚠ 返回副本而不是指针：返回指针会在调用方留下一个活着的引用，
// 于是紧接着的 `exec.Finish(...)` 会让人以为它指向的还是旧值。
func (e *RunExecution) Stage() RunStage { return e.stage }

// Outcome 返回终局的业务判定。还在跑时返回 ok=false。
func (e *RunExecution) Outcome() (RunOutcome, bool) { return e.stage.StageOutcome() }

// IsTerminal 报告是否已在终态。
func (e *RunExecution) IsTerminal() bool { return e.stage.IsTerminal() }

// HasEmittedTerminal 报告是否已经发布过终局事件。
func (e *RunExecution) HasEmittedTerminal() bool { return e.emittedTerminal }

// Record 记一次事件发布 —— 调用方在**真正**把事件推进总线之前调它。
//
// 终局之后再记会被拒：这是"终局事件至多一次、且必须是最后一条"的守门人。
func (e *RunExecution) Record(event RunEvent) error {
	if event.RunID != e.runID {
		return &StageError{
			Kind: StageErrRunIDMismatch, Expected: e.runID, Actual: event.RunID,
		}
	}
	if e.emittedTerminal {
		return &StageError{
			Kind: StageErrAlreadyTerminal, Current: "terminal-emitted", Attempted: "record",
		}
	}
	if event.IsTerminal() {
		e.emittedTerminal = true
	}
	return nil
}

// Finish 正常结算。
//
// 两条硬校验，都是为了让"旧项目那种失真"无法表达：
//   - [OutcomeManualRequired] **必须**带待办（空手说需人工 = 那句没人看的日志）；
//   - [OutcomeFailed] **必须**带说明（空说明 = 把错误吞掉）。
//
// # 第一条校验什么时候才真的会触发
//
// [NewRunReportManualRequired] 在构造函数里就把"空待办"降级成了
// [OutcomePartialSuccess]，所以走构造函数**到不了**这里。
// 能到这里的只有**用字面量直接构造** [RunReport] 的调用方（字段是导出的）。
//
// 那为什么不干脆把这条删掉？因为 [RunReport.Normalize] 只做**升级**
// （`Success` + 有待办 → `ManualRequired`），**不做降级** ——
// 手写的 `ManualRequired` + 空待办会一路穿到这里。
// 这一层是那唯一一条路的守卫，删了就等于"只要不用构造函数，
// 旧项目那句没人看的日志就能进库"。
//
// 返回结算后的阶段**快照**（而不是指针）：借用会在调用方留下一个活着的引用，
// 于是紧接着的 `IsTerminal()` / `Outcome()` 会让人以为读到的还是旧值 ——
// 而那两句恰恰是结算后最自然的下一步。`RunStage` 很小，复制一份更划算。
func (e *RunExecution) Finish(report RunReport) (RunStage, error) {
	if e.stage.IsTerminal() {
		return RunStage{}, &StageError{
			Kind: StageErrAlreadyTerminal, Current: e.stageName(), Attempted: "finish",
		}
	}

	// 复核不变式：字段是导出的，RunReport 可以被字面量绕过构造函数。
	report = report.Normalize()

	if report.Outcome == OutcomeManualRequired && len(report.ManualSteps) == 0 {
		return RunStage{}, &StageError{Kind: StageErrManualRequiredWithoutSteps}
	}

	if report.Outcome == OutcomeFailed {
		summary := ""
		if report.Summary != nil {
			summary = *report.Summary
		}
		// ⚠ 只有空白也算空：一个"   "的说明和没有说明是一样的。
		if strings.TrimSpace(summary) == "" {
			return RunStage{}, &StageError{Kind: StageErrFailedWithoutSummary}
		}
		stage := NewRunStageFailed(summary, report.ManualSteps)
		e.stage = stage
		return stage, nil
	}

	stage := NewRunStageDone(report.Outcome, report.ManualSteps, report.Summary)
	e.stage = stage
	return stage, nil
}

// Fail 是执行器自身失败（panic、内部错误、断线）。
//
// 与 [RunExecution.Finish] 的区别：这条**不需要**能力给出业务判定，
// 因为能力可能根本没跑到能判定的地方。
func (e *RunExecution) Fail(summary string) (RunStage, error) {
	if e.stage.IsTerminal() {
		return RunStage{}, &StageError{
			Kind: StageErrAlreadyTerminal, Current: e.stageName(), Attempted: "fail",
		}
	}
	if strings.TrimSpace(summary) == "" {
		return RunStage{}, &StageError{Kind: StageErrFailedWithoutSummary}
	}
	stage := NewRunStageFailed(summary, nil)
	e.stage = stage
	return stage, nil
}

func (e *RunExecution) stageName() string { return e.stage.Kind.String() }
