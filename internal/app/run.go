package app

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// ─────────────────────────────────────────────────────────────────────────────
// 计划与结果
// ─────────────────────────────────────────────────────────────────────────────

// RunPlan 是一次跨账号运行要跑的东西。
type RunPlan struct {
	// Capability 是能力 code（写进 `runs` 行与事件里，前端按它分组）。
	Capability string
	// Accounts 是目标账号（调用方已按 [core.Scope] 过滤过 —— 这里不再查一遍库）。
	Accounts []core.AccountID
	// ParamsDigest 是参数指纹（[mcloudexec.ParamsDigest] 的产物）。
	//
	// 参与互斥键：**同一个账号、同一个能力、不同参数**不该互相挡 ——
	// 手动签到与定时签到用不同参数时是两件事。
	ParamsDigest string
}

// AccountRunKind 是单个账号在本次运行里的两种结局。
type AccountRunKind int

const (
	// AccountRunRan 表示真的跑了。
	AccountRunRan AccountRunKind = iota
	// AccountRunSkipped 表示同一把锁被别的 run 占着：**不排队、也不假装成功**。
	AccountRunSkipped
)

// AccountRun 是单个账号的结果。
//
// Rust 侧是 `enum AccountRun { Ran { … }, Skipped { … } }`；Go 没有载荷枚举，
// 用"种类 + 全部字段"的写法（与 [ReceiveOutcome] 同一套约定）。
// 代价是"该读哪个字段"靠注释而不是靠类型；换来的是调用方不必写 type switch。
type AccountRun struct {
	Kind AccountRunKind
	// Account 是目标账号，两种结局都有。
	Account core.AccountID
	// RunID 仅在 [AccountRunRan] 下有效。
	RunID core.RunID
	// Report 仅在 [AccountRunRan] 下有效。
	Report core.RunReport
	// BlockedBy 仅在 [AccountRunSkipped] 下有效：占用者的 runId。
	//
	// 记下占用者，是因为"被谁挡了"比"被挡了"有用得多 ——
	// 排查时能直接找到那一趟。
	BlockedBy core.RunID
}

// RunSummary 是一次跨账号运行的汇总。
type RunSummary struct {
	Capability string
	// Outcome 由**真的跑了**的账号合成；被挡下的那些不参与（它们没有结果可合）。
	Outcome  core.RunOutcome
	Accounts []AccountRun
	// Events 是落库的事件条数（观测用：SSE 断线续传读的就是它们）。
	Events int
}

// Ran 是真的跑了的账号及其结果。
func (s RunSummary) Ran() []AccountRun {
	out := make([]AccountRun, 0, len(s.Accounts))
	for _, entry := range s.Accounts {
		if entry.Kind == AccountRunRan {
			out = append(out, entry)
		}
	}
	return out
}

// Skipped 是被锁挡下的账号数。
func (s RunSummary) Skipped() int {
	n := 0
	for _, entry := range s.Accounts {
		if entry.Kind == AccountRunSkipped {
			n++
		}
	}
	return n
}

// SummaryLine 是给人看的一句话。
//
// 被挡下的账号**必须**出现在这句话里：否则"这次只跑了一个账号"看起来
// 跟"另一个账号跑失败了"一模一样，而两者的处置完全不同。
func (s RunSummary) SummaryLine() string {
	line := fmt.Sprintf("%s：跑了 %d 个账号，%s",
		s.Capability, len(s.Ran()), outcomeWord(s.Outcome))
	if skipped := s.Skipped(); skipped > 0 {
		line += fmt.Sprintf("，%d 个因正在运行被跳过", skipped)
	}
	return line
}

func outcomeWord(outcome core.RunOutcome) string {
	switch outcome {
	case core.OutcomeSuccess:
		return "全部成功"
	case core.OutcomePartialSuccess:
		return "部分成功"
	case core.OutcomeManualRequired:
		return "需要你去客户端操作"
	case core.OutcomeFailed:
		return "失败"
	}
	return ""
}

// ─────────────────────────────────────────────────────────────────────────────
// 运行器
// ─────────────────────────────────────────────────────────────────────────────

// RunTask 是"针对单个账号做一件事"：成功给 [core.RunReport]、失败给 error。
type RunTask func(ctx context.Context, account core.AccountID) (core.RunReport, error)

// Runner 是跨账号运行器。
//
// 持有运行级的共享状态（准入表与事件总线），因为它们的生命周期**比一次 run 长**：
// 服务端进程活着的时候，"谁正在跑"与"SSE 订阅者"都要跨 run 延续。
//
// 两者都是**指针**（而不是各自持有一份）：调用方必须让同一个实例被跑任务与
// 看事件两条路共用 —— 各持一份的话，SSE 那边永远看不到这里发布的事件。
// 它们的内部各自带锁（见 `mcloudexec` 的包注释），所以一次运行期间订阅者照样能读。
type Runner struct {
	store    core.StateStore
	scope    core.Scope
	clock    core.Clock
	registry *mcloudexec.RunRegistry
	bus      *mcloudexec.EventBus
}

// NewRunner 组装一次编排（`registry` / `bus` 由调用方持有并复用）。
func NewRunner(
	store core.StateStore,
	scope core.Scope,
	clock core.Clock,
	registry *mcloudexec.RunRegistry,
	bus *mcloudexec.EventBus,
) *Runner {
	return &Runner{store: store, scope: scope, clock: clock, registry: registry, bus: bus}
}

// Run 按计划跑一遍：逐账号准入 → 执行 → 发事件 → 落库。
//
// 单账号失败**不会**中断整批（继续跑下一个），因为"一个账号风控了"不该让
// 其余账号今天什么都不做。
//
// # 顺序执行（v1）
//
// **不做并行**，且这是有意的：串行是原项目防风控的要求，
// 并行是**显式选择**而非默认行为。
//
// # 事件同时走两条路
//
// 总线负责**在线的**订阅者（带 seq，断了能从 `?after=` 续上），
// `run_events` 表负责**后来的**人与重启后的续传。两者缺一不可：
// 只发总线则刷新页面就丢历史，只落库则前端得轮询。
//
// 落库按**账号**批量（而不是逐条，也不是整批结束才写）：
// 崩在最坏的情况下丢一个账号的事件，而不是整次运行的。
func (r *Runner) Run(
	ctx context.Context,
	plan RunPlan,
	rng core.Random,
	task RunTask,
) (RunSummary, error) {
	total := uint32(len(plan.Accounts))
	entries := make([]AccountRun, 0, len(plan.Accounts))
	eventsWritten := 0
	done := uint32(0)

	for _, account := range plan.Accounts {
		key := mcloudexec.NewMutexKey(plan.Capability, account.String(), plan.ParamsDigest)
		runID := core.NewRunID(rng)

		admission := r.registry.Admit(runID, key)
		if admission.Kind != mcloudexec.AdmissionAdmitted {
			// 被挡下（或 runId 撞了 —— 刚生成的 runId 不该重复，真重复了说明
			// 随机源被钉住了，测试里会）。按"被挡下"处理而不是 panic：
			// 编排层不该因为随机源退化就炸掉整批。
			blockedBy := admission.Holder
			if admission.Kind == mcloudexec.AdmissionDuplicateRunID {
				blockedBy = admission.RunID
			}
			entries = append(entries, AccountRun{
				Kind:      AccountRunSkipped,
				Account:   account,
				BlockedBy: blockedBy,
			})
			continue
		}

		entry, written, doneNow, perr := r.runOne(ctx, plan, account, runID, task, done, total)
		done = doneNow
		if perr != nil {
			return RunSummary{}, perr
		}
		eventsWritten += written
		entries = append(entries, entry)
	}

	reports := make([]core.RunReport, 0, len(entries))
	for _, entry := range entries {
		if entry.Kind == AccountRunRan {
			reports = append(reports, entry.Report)
		}
	}
	return RunSummary{
		Capability: plan.Capability,
		Outcome:    core.CombineOutcomes(reports),
		Accounts:   entries,
		Events:     eventsWritten,
	}, nil
}

// runOne 跑单个账号。
//
// 单独成函数**不是**为了好看：释放锁必须 `defer`，而 `defer` 在 `for` 里
// 会攒到函数结束才跑 —— 那意味着**第一个账号就锁死后面所有账号**。
// 放进这个函数里，`defer` 就落在"每个账号跑完"这个正确的边界上。
func (r *Runner) runOne(
	ctx context.Context,
	plan RunPlan,
	account core.AccountID,
	runID core.RunID,
	task RunTask,
	done, total uint32,
) (AccountRun, int, uint32, error) {
	// 保证"受理过就一定释放"。
	//
	// 用 defer 而不是"记得在每个 return 前 release"：后者在加了一个提前返回之后
	// 就会静默漏掉，而漏掉的后果是**那把锁永远被占着** ——
	// 同一个账号从此再也跑不起来，且没有任何报错指向这里。
	defer r.registry.Release(runID)

	startedAt := r.clock.Now()
	r.bus.Open(runID)
	batch := []core.Sequenced{
		r.publish(core.NewEventStarted(runID, plan.Capability)),
	}
	accountID := account
	if err := r.store.InsertRun(ctx, r.scope, &core.RunRecord{
		ID:         runID.String(),
		AccountID:  &accountID,
		Capability: plan.Capability,
		// 先记 Running：跑起来就该在历史里看得见，
		// 而不是等它结束才突然出现（长任务尤其如此）
		Status:    core.StatusRunning,
		StartedAt: startedAt,
	}); err != nil {
		return AccountRun{}, 0, done, err
	}

	report, taskErr := task(ctx, account)
	if taskErr != nil {
		batch = append(batch, r.publish(core.NewEventErrored(runID, taskErr.Error())))
		report = core.NewRunReportFailed(taskErr.Error())
	}

	batch = append(batch, r.publish(
		core.NewEventFinished(runID, report.Outcome, report.ManualSteps, report.Summary)))
	done++
	batch = append(batch, r.publish(core.NewEventProgress(runID, done, total)))

	finishedAt := r.clock.Now()
	if err := r.store.UpdateRun(ctx, r.scope, &core.RunRecord{
		ID:         runID.String(),
		AccountID:  &accountID,
		Capability: plan.Capability,
		Status:     report.Outcome.ToLegacyStatus(),
		StartedAt:  startedAt,
		FinishedAt: &finishedAt,
		Message:    report.Summary,
	}); err != nil {
		return AccountRun{}, 0, done, err
	}

	// 落库在逐个账号的边界上：崩了最多丢一个账号的事件，而不是整批
	written := 0
	if len(batch) != 0 {
		if err := r.store.AppendRunEvents(ctx, r.scope, runID, batch); err != nil {
			return AccountRun{}, 0, done, err
		}
		written = len(batch)
	}

	return AccountRun{Kind: AccountRunRan, Account: account, RunID: runID, Report: report},
		written, done, nil
}

// publish 发一条事件到总线，返回**带序号的副本**（落库要用它）。
//
// 序号由总线发，落库的 `(run_id, seq)` 主键因此与在线的 SSE 流一致 ——
// 断线重连时 `?after=<seq>` 才能接得上。
func (r *Runner) publish(event core.RunEvent) core.Sequenced {
	seq, _ := r.bus.Publish(event)
	return core.NewSequenced(seq, event)
}
