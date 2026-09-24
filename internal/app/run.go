package app

import (
	"context"
	"fmt"
	"strconv"

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

	// RunIDs 与 Accounts **一一对应**，由调用方预分配；nil 表示"跑到哪个才分配哪个"。
	//
	// 存在的理由只有异步触发那一条路：`POST /api/runs` 必须**在动手之前**就把
	// runId 回给前端（前端拿它去订阅 `GET /api/runs/{id}/events`），而账号是串行跑的、
	// 第二个要等第一个跑完才轮到 —— 现分配的话响应里只能给出第一个。
	// 留空是同步调用方（CLI、测试）的默认。
	//
	// ⚠ 长度不等时不猜、不补齐：直接报错返回。一个空 runId 会落进 `runs` 主键，
	// 那比一次失败糟得多。
	RunIDs []core.RunID
}

// RunProgress 是"往**当前这个账号**的事件流里写一条人话"。
//
// 它作为参数交给 [RunTask] 而不是让任务自己去拿总线：任务手上没有 runId
// （每个账号一个），而序号必须由总线统一发 —— 否则 SSE 的断线续传（`?after=`）
// 与 `run_events` 的 `(run_id, seq)` 主键会对不上。
//
// ⚠ 这条通道只给**过程话**。结构化结果走 [core.RunReport]（终局事件与 `runs.message`），
// 事件种类与 JSON 形状是**冻结契约**（与 Rust 侧逐字一致），不为了传数据新增一类。
type RunProgress func(level core.LogLevel, message string)

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
	// Notify 是收尾那条推送的结论；`nil` = **这台运行器没接推送**。
	//
	// 为什么是指针而不是零值：三态里"没接"与"接了但门槛没过"是两句不同的话 ——
	// 前者是部署形态（CLI 就不接），后者是配置问题（界面上要指得出来）。
	// 压成零值会让"没接"读成"没发"，而排查方向完全相反。
	//
	// ⚠ 它**不**进 `run_events`：那张表按**单账号那一趟**组织（主键 `(run_id, seq)`），
	// 而一次跨账号推送不属于其中任何一条 run —— 写进任意一条都会让那一趟的事件流
	// 含进别人的事。现在的落点是调用方（调度记账、CLI 输出）；要持久化得另找位置，
	// 已登记成待拍板（`docs/pending-decisions.md` Q23）。
	Notify *RunNotifyResult
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
//
// `progress` 由 [Runner] 给，写进去的每一条都会成为**这个账号那一次 run** 的
// `log` 事件（既进总线也落库）。它不是可选的装饰：一条要跑几十秒的任务
// 如果没有过程话，界面上的表现就是"进度条停在那里不动"，
// 而用户分不清是卡住了还是在等上游。
type RunTask func(
	ctx context.Context, account core.AccountID, progress RunProgress,
) (core.RunReport, error)

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
	// notify 是收尾那条推送的编排；`nil` = 不推（CLI 与多数测试的默认形态）。
	//
	// 为什么挂在运行器而不是调用方：门槛要的"本次运行的全部过程话"只有这里攒得齐
	// （过程话是从 [RunProgress] 那一路流出去的），让每个调用方自己抄一份就会
	// 出现"服务端推、CLI 不推"这种同一件事两套形状的扩散。
	// 接不接仍是调用方的事 —— 见 [WithRunNotify]。
	notify *RunNotify
}

// RunnerOption 是运行器的可选项（用变参而不是把构造函数撑成七个参数：
// 现有调用方一次都不用改，新选项也不会把签名越撑越长）。
type RunnerOption func(*Runner)

// WithRunNotify 给这台运行器接上"跑完一趟推一次"。
//
// 传 `nil` 等于不接（与不传同一个效果）：让"没接"只有一种表示法，
// 免得 `WithRunNotify(nil)` 与"不调这个方法"将来分叉出两种行为。
func WithRunNotify(notify *RunNotify) RunnerOption {
	return func(r *Runner) {
		if notify != nil {
			r.notify = notify
		}
	}
}

// NewRunner 组装一次编排（`registry` / `bus` 由调用方持有并复用）。
func NewRunner(
	store core.StateStore,
	scope core.Scope,
	clock core.Clock,
	registry *mcloudexec.RunRegistry,
	bus *mcloudexec.EventBus,
	opts ...RunnerOption,
) *Runner {
	r := &Runner{store: store, scope: scope, clock: clock, registry: registry, bus: bus}
	for _, opt := range opts {
		opt(r)
	}
	return r
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

	// 只有接了推送才攒过程话（见 [runLogSink] 的头注：一条直播链路能写几百行）。
	// `nil` 是合法值 —— [runLogSink.add] 对 nil 接收者直接返回，
	// 于是攒取那一步在没接推送时**一次分配都没有**，也不需要在调用点判空。
	var sink *runLogSink
	if r.notify != nil {
		sink = &runLogSink{}
	}

	// 预分配的 runId 必须与账号**一一对应**。长度不等不报错的话，多出来的账号
	// 会拿到一个空 runId 去落 `runs` 主键 —— 那是一条谁也查不到的记录。
	if plan.RunIDs != nil && len(plan.RunIDs) != len(plan.Accounts) {
		return RunSummary{}, core.NewInternal(fmt.Sprintf(
			"预分配的 runId 数量与账号不等：%d 对 %d", len(plan.RunIDs), len(plan.Accounts)))
	}

	for i, account := range plan.Accounts {
		key := mcloudexec.NewMutexKey(plan.Capability, account.String(), plan.ParamsDigest)
		runID := core.NewRunID(rng)
		if plan.RunIDs != nil {
			runID = plan.RunIDs[i]
		}
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

		// 攒给收尾那条推送的一份"记录器"。三件事都收在这一个闭包里：
		// 时刻（只从注入的时钟取）、账号前缀、以及"没接推送时什么都不做"。
		rec := r.logRecorder(sink, i, len(plan.Accounts))
		entry, written, doneNow, perr := r.runOne(ctx, plan, account, runID, task, done, total, rec)
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
	summary := RunSummary{
		Capability: plan.Capability,
		Outcome:    core.CombineOutcomes(reports),
		Accounts:   entries,
		Events:     eventsWritten,
	}

	// 收尾推送。**放在返回之前、落库之后**：正文要含"这趟成没成"那句小结，
	// 而那句只有等汇总出来了才有。
	//
	// 用 `ctx` 而不是另起一个背景上下文：这一趟如果是被"关掉页面"或"进程收尾"
	// 取消掉的，那一条推送也不该硬发（发出去的话，用户收到一条说不清结局的消息）。
	// 代价是"跑完了但推送没发出去"这一格会记成一次失败，而不是静默 —— 查得出来。
	if r.notify != nil {
		result := r.notify.Dispatch(ctx, NotifyInput{
			Summary: summary.SummaryLine(),
			Lines:   sink.collect(r.clock.Now()),
		})
		summary.Notify = &result
	}
	return summary, nil
}

// logRecorder 造一个"把过程话攒进收尾推送"的记录器；不推送时返回 nil（nil 安全）。
//
// 账号前缀只在**多账号**那一趟才加：单账号加它是纯噪声，而多账号不加就会
// 分不清"这一句是谁的"（旧实现是每账号收尾打一句 `账号 ${index+1}`，同一个取向）。
//
// ⚠ 前缀用**序号**，不用 [core.AccountID]：后者由手机号派生（见 `core.AccountID` 头注），
// 而推送正文要经过第三方服务器。序号是"本次运行内的第几个"，
// 与界面上那张账号表的顺序同源（调用方传进来的 `plan.Accounts` 就是那个顺序）。
func (r *Runner) logRecorder(sink *runLogSink, index, accounts int) runLogRecorder {
	if sink == nil {
		return nil
	}
	prefix := ""
	if accounts > 1 {
		prefix = "账号 " + strconv.Itoa(index+1) + "："
	}
	return func(level core.LogLevel, message string) {
		sink.add(level, r.clock.Now(), prefix+message)
	}
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
	rec runLogRecorder,
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
	// 过程话与开场那条**同源**（同一个 runID、同一条总线），所以进同一个 batch 落库：
	// 序号是发布时分配的，分两条路写会让"库里的 seq 顺序"与"写入顺序"脱节，
	// 而断线续传读的正是 seq 单调。
	progress := func(level core.LogLevel, message string) {
		batch = append(batch, r.publish(core.NewEventLog(runID, level, message)))
		// 同一条消息进第二个去处：收尾那条推送的正文。
		//
		// 刻意**不**从 `run_events` 回读：那要等落库、且跨账号时得按多个 runId 拼，
		// 而"界面上看到的"与"推送里看到的"同源这件事，靠的是同一个 `progress` 入口，
		// 不是靠两处读同一张表对上 —— 表里还有 started/finished 那几类不该进正文的行。
		rec.record(level, message)
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

	report, taskErr := task(ctx, account, progress)
	if taskErr != nil {
		batch = append(batch, r.publish(core.NewEventErrored(runID, taskErr.Error())))
		report = core.NewRunReportFailed(taskErr.Error())
		// 这一条是 `onlyError` 那道门槛**看得见**的唯一入口：
		// 旧实现走的是 `logger.error(...)`，所以出错一定进正文；
		// 本仓的错误经 [core.NewEventErrored] 走事件流，不攒进来的话
		// "只推错误"的推送会在真的失败那一趟什么都没发。
		rec.record(core.LevelError, "这一趟出错："+taskErr.Error())
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
