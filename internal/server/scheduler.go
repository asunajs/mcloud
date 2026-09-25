package server

// 调度循环：服务进程内那一条"到点自己跑"的回路。
//
// # 为什么住在服务进程里
//
// 旧实现（Node + croner）跑在**前端那个产物**里，所以这一版 Go 服务最初写的是
// "本后端不装调度循环，定时由外部触发"（`mcloud run --capability …` 加系统 crontab）。
// 那句话在 2026-09-22 被推翻：定时这件事要能配在界面上（`schedules` 表就是为它存在），
// 而"外部触发"要求每台装了这个工具的机器自己去学 crontab —— 界面上那枚"启用"开关
// 于是变成一个纯装饰。所以循环搬进 `mcloud-server`，随进程起停。
//
// ⚠ 一条随之而来的约束：**一份库只跑一个服务端进程**。水位是"读到 → 判 → 写"三步，
// 不是一个跨进程的原子认领，两个进程共用同一个库文件时同一个触发点会被各跑一次
// （表现是"同一批口令投两遍"）。同机的第二个实例（比如 smoke 版）要么换库要么停掉。
//
// # 三件不成文但会咬人的事
//
//   - **水位先于起跑**：抬 `last_fire_at` 在打上游**之前**。反过来做的表现是
//     "进程在跑任务时被人杀掉 ⇒ 重启后同一个点再跑一次"，而签到、投口令这些动作
//     在上游那侧不是幂等的。宁可漏跑一次，不可重跑一次。
//   - **错过的整串一次弃掉**：真错过（进程停过）时水位直接抬到 `now`，而不是
//     每个 tick 追补一个点 —— 后者等于"停机八小时，重启后连打八趟上游"。
//     补不补由 [core.MissPolicy] 那一档决定（准点型不补、日型补最近一次）。
//   - **坏数据每个触发点只报一次**：参数解不出、能力没接上执行，都先把水位抬上去
//     再记账。不抬的话每个 tick 都会重演一遍，`last_error` 会被刷成噪声，
//     而界面上那句"上次执行"就不是任何一个真实时刻了。
//
// # 跑完之后那一步为什么是回调
//
// `launchRun` 在 202 之后还要占着调用方的栈等一分钟弹幕是不可接受的（见
// [run_dispatch.go] 那条注释），所以结论只能从后台 goroutine 里递回来：
// 见 [scheduleCompletion]。它同时也是"schedules.last_status 的第一个写入方" ——
// 这三列自 schema v2 落下起一直没人写。

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

const (
	// schedulerTick 是轮询间隔。
	//
	// 30 秒这个量是从"cron 最细到分钟"推出来的：再密也不会更早触发，
	// 只会多打库；再稀则一条整点任务的迟到时间会长到用户看得见。
	schedulerTick = 30 * time.Second
	// schedulerGrace 是"迟到多久才算真错过"。
	//
	// ⚠ 取**两个** tick 而不是一个：一轮被 SQLite 的写锁顶住是常态（跑任务的那条路
	// 在批量写 `run_events`），那一次迟到不该把一条准点任务的触发点判成"错过"并弃掉。
	// 真·停机迟到的是分钟到小时级，一定越过这一条。
	schedulerGrace = 2 * schedulerTick
)

// ─────────────────────────────────────────────────────────────────────────────
// 循环
// ─────────────────────────────────────────────────────────────────────────────

// startScheduler 起调度循环，阻塞到 `ctx` 结束。
//
// `report` 收的是**循环自己**那一份问题（读库失败那一类）。它不能像别处那样
// 只返回给调用方：这里没有调用方在等，而读库失败时一行 `schedules` 都没读到，
// 错误也就无处可写 —— 界面上会表现成"什么都没发生"，只能打到进程的输出上。
// 传 [io.Discard] 就是"闭嘴跑"。
func (s *Server) startScheduler(ctx context.Context, report func(string)) {
	s.schedulerOn.Store(true)
	defer s.schedulerOn.Store(false)

	// 先跑一轮再起 ticker：`time.Ticker` 的第一发在**一个间隔之后**才来，
	// 于是每次重启都白等 30 秒 —— 而重启恰恰是"错过今天那一趟签到"最常见的原因。
	s.runTick(ctx, s.clock.Now(), report)

	ticker := time.NewTicker(schedulerTick)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			s.runTick(ctx, s.clock.Now(), report)
		}
	}
}

// SchedulerRunning 报告调度循环在不在跑（`GET /api/status` 的那个字段）。
func (s *Server) SchedulerRunning() bool { return s.schedulerOn.Load() }

// runTick 跑一轮，并把循环级别的错误交给 `report`。
func (s *Server) runTick(ctx context.Context, now time.Time, report func(string)) {
	_, problems, err := s.runScheduledTick(ctx, now)
	if err != nil {
		report("调度轮询失败（这一轮什么都没看）：" + core.ScrubText(err.Error()))
	}
	for _, problem := range problems {
		report("定时任务 " + core.ScrubText(problem))
	}
}

// runScheduledTick 走一遍**所有人**的定时任务，返回"起跑了几条"与一份问题清单。
//
// # 为什么用 [core.ScopeLocal] 读、却按行回到各自 owner
//
// 这一条读是非路由的（没有任何请求拿得到这份列表），而"谁的行归谁管"这件事
// 在后续每一步都要成立：水位与记账都写回**那一行自己的** owner scope。
// 所以 ScopeLocal 在这里只用于"找出每一行的 owner 是谁"，不用于读写。
//
// ⚠ 无主行（`owner IS NULL`，导入或迁移留下的）**一律不触发**：
// 没有 owner 就没有 scope，也就无法回答"这一趟看得见哪些账号"。
// 那种行的正确处置是 `mcloud user init` 去认领，不是让循环替它猜一个身份。
func (s *Server) runScheduledTick(
	ctx context.Context, now time.Time,
) (int, []string, error) {
	if s.runs == nil {
		return 0, nil, core.NewInternal("这个服务端没有装配执行面，调度循环无任务可跑")
	}
	schedules, err := s.store.ListSchedules(ctx, core.ScopeLocal())
	if err != nil {
		return 0, nil, err
	}
	fired, problems := 0, []string{}
	for i := range schedules {
		started, problem := s.fireScheduleIfDue(ctx, &schedules[i], now)
		if problem != "" {
			problems = append(problems, problem)
		}
		if started {
			fired++
		}
	}
	return fired, problems, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 一行：判到点 → 推水位 → 起跑
// ─────────────────────────────────────────────────────────────────────────────

// fireScheduleIfDue 处理一行定时任务，返回"这一行起跑了没"与一句问题。
//
// 单行的问题**不会**中断整轮：一条坏数据不该让别人的定时任务停摆。
func (s *Server) fireScheduleIfDue(
	ctx context.Context, schedule *core.Schedule, now time.Time,
) (bool, string) {
	// 三道"这一行不归本轮管"的闸，全都在打库之前。
	// 无主行不触发（没有 owner 就没有 scope，见 [runScheduledTick] 的说明）；
	// 没有目标的行连"该在哪些点跑"都无从判断，也不动。
	if !schedule.Enabled || schedule.Owner == nil || schedule.Target == nil {
		return false, ""
	}

	point, due, perr := nextDuePoint(schedule, now)
	if perr != nil {
		// cron 或时区解不出来：**不推水位**（推了等于把一条能修好的数据判死），
		// 只报出来。界面上那条任务下次刷新还是原样。
		return false, schedule.ID + "：判触发点失败 —— " + perr.Message()
	}
	if !due {
		return false, ""
	}
	missed := now.Sub(point) > schedulerGrace

	// 下面两条"到点了但跑不了"的分支都**在判到点之后**，这是为了让它们
	// 一个触发周期只报一次：放在判点之前会让每个 tick 都重写一遍水位与记账，
	// 界面上那句"上次执行"就不是任何一个真实时刻了。
	if schedule.Target.Kind == core.TargetPipeline {
		return s.launchScheduledPipeline(ctx, schedule, now, point, missed)
	}
	capability, found := capabilities.Get(schedule.Target.Code)
	if !found {
		// PUT 那一路验过目标存在，走到这里只剩"能力 code 被改名而库里还是旧的"。
		return s.abandon(ctx, schedule, now, core.ScheduleError,
			"没有这个能力："+schedule.Target.Code)
	}
	if missed && capabilities.ResolveMissPolicy(capability, schedule) == core.MissSkip {
		return s.abandon(ctx, schedule, now, core.ScheduleSkipped,
			fmt.Sprintf("错过的触发点（%s）按「不补跑」弃掉，等下一个点",
				point.In(core.LocalTZ()).Format(time.RFC3339)))
	}
	return s.launchScheduled(ctx, schedule, now, point, missed, capability)
}

// nextDuePoint 找"水位之后、且不晚于 now 的那一个触发点"。
//
// # 水位从哪一列来
//
// [core.Schedule.LastFireAt] 是"已处理过的触发时刻"，它是唯一能当水位用的列：
// 一次按 [core.MissSkip] 弃掉的错过**不会留下任何运行**，拿 `LastRunAt` 代替的话，
// 下一个 tick 又把同一次错过捡起来 —— 表现成"每次刷新页面多一条 skipped"。
//
// ⚠ 两列都是 nil（导入的坏数据）时以 `now` 往前推一个宽限窗口起算，也就是**不补历史**：
// 表里凭空多一行不认识的历史，不足以授权循环去打一枪真上游。
// 但也**不能直接以 `now` 起算** —— [core.NextFire] 返回严格晚于 `after` 的点，
// 那样这一行在每个 tick 都只能"等到下一个整点之后的第一个点"，
// 表现成一行永远不触发的死任务。留一个宽限窗口意味着：现在这一秒之前错过的整串翻篇，
// 而恰好落在窗口内的那一个点照样会补上。
func nextDuePoint(schedule *core.Schedule, now time.Time) (time.Time, bool, *core.Error) {
	watermark := now.Add(-schedulerGrace)
	if schedule.LastFireAt != nil {
		watermark = *schedule.LastFireAt
	} else if schedule.CreatedAt != nil {
		watermark = *schedule.CreatedAt
	}
	next, fires, err := core.NextFire(schedule.Cron, schedule.Timezone, watermark)
	if err != nil {
		return time.Time{}, false, asDomain(err)
	}
	if !fires || next.After(now) {
		return next, false, nil
	}
	return next, true, nil
}

// abandon 处理"到点了但不跑"的三种情形（组合没接上、能力没接上、按策略弃掉）：
// 推水位 + 记账，然后返回"没起跑"。
//
// 水位一律抬到 `now`：这三条都不是"这一次特殊"，而抬到 now 意味着
// 已经过去的触发点整串翻篇，下一个 tick 不会替它们再报一遍。
// `last_run_at` **不写**（nil 那一支），因为这一趟什么都没跑。
func (s *Server) abandon(
	ctx context.Context, schedule *core.Schedule, now time.Time,
	status core.ScheduleStatus, note string,
) (bool, string) {
	if perr := s.markFired(ctx, schedule, now); perr != nil {
		return false, schedule.ID + "：" + perr.Message()
	}
	if rerr := s.recordScheduledRun(ctx, schedule, nil, status, note); rerr != nil {
		return false, schedule.ID + "：" + note + "（而且这句没写进库：" + rerr.Message() + "）"
	}
	return false, ""
}

// launchScheduled 是"到点、并且要跑"那一条路：推水位 → 翻参数 → 交给分派表。
//
// 三条链（直播口令 / 签到 / 到期换凭据）在这里走的都是 [runBuilders] 那一份，
// 也就是界面上"开始"按钮那一条路的同一个 builder —— 判据见 `run_dispatch.go` 的
// 文件头：两边各写一遍的代价是"界面上跑对了、定时那一版漏了一步"。
func (s *Server) launchScheduled(
	ctx context.Context, schedule *core.Schedule, now, point time.Time, missed bool,
	capability capabilities.Capability,
) (bool, string) {
	builder, wired := builderFor(capability.Code)
	if !wired {
		// 注册表里有、分派表里没有 —— 界面上回 501，这里记一句同样的话。
		return s.abandon(ctx, schedule, now, core.ScheduleError,
			"能力「"+capability.Title+"」还没有接上执行")
	}
	watermark := point
	if missed {
		watermark = now // 错过的整串一次弃掉：只跑最近这一次，不逐次追补
	}
	if perr := s.markFired(ctx, schedule, watermark); perr != nil {
		return false, schedule.ID + "：" + perr.Message()
	}

	raw, perr := scheduleParams(schedule)
	if perr == nil {
		perr = checkParamKeys(capability, raw)
	}
	if perr == nil {
		// `accountMode` 不用在这里另判：没给就落到声明默认（`"sequential"`），
		// 显式给了并行则由 builder 里那一条 [sequentialOnly] 拒掉 ——
		// 界面与调度共用同一句拒绝，不另写一份措辞。
		var spec runSpec
		spec, perr = builder(ctx, s, scopeOfSchedule(schedule), capability, raw)
		if perr == nil {
			_, perr = s.launchRun(ctx, spec, notifyPerRun,
				s.scheduleCompletion(ctx, schedule, now))
		}
	}
	if perr != nil {
		// `last_run_at` 不写：这一趟没起跑，只是"到点了但没能开始"。
		s.recordScheduledRun(ctx, schedule, nil, core.ScheduleError, perr.Message())
		return false, schedule.ID + "：" + perr.Message()
	}
	return true, ""
}

// launchScheduledPipeline 是"到点、目标是组合"那一条路。
//
// 它与 [Server.launchScheduled] 做的是同一件事，只是目标不是一条能力而是一串：
// 判到点 → 推水位 → 交给**同一个** [Server.runPipeline]。走同一个执行器是硬要求，
// 判据与 `run_dispatch.go` 抬头那条一致 —— 界面上那枚按钮与定时各跑一套的话，
// "手工点是对的、夜里自动跑的那一版少三步"不会有任何测试会红。
//
// # 补跑策略：没有能力声明可跟随，所以只有两档
//
// [capabilities.ResolveMissPolicy] 的第一档是"这一行自己选过的"，第二档是
// "该能力的默认"。组合**没有那第二档**：一条链里既有"错过就别补"的签到、
// 也有"错过仍然值得领"的提醒，替它们 collectively 声明一个默认等于猜。
// 所以这里只认用户自己选过的，没选过 ⇒ [core.MissSkip]（与 ResolveMissPolicy
// 的兜底同一方向：补跑是替人授权一次额外执行，没人说过就不做）。
func (s *Server) launchScheduledPipeline(
	ctx context.Context, schedule *core.Schedule, now, point time.Time, missed bool,
) (bool, string) {
	id := schedule.Target.ID
	pipeline, perr := s.pipelineToRun(ctx, scopeOfSchedule(schedule), id)
	if perr != nil {
		// 组合被删了、或这条链一步都跑不了（能力被改名）：都是"到点了但没能开始"。
		return s.abandon(ctx, schedule, now, core.ScheduleError, perr.Message())
	}
	policy := core.MissSkip
	if schedule.MissPolicy != nil {
		policy = *schedule.MissPolicy
	}
	if missed && policy == core.MissSkip {
		return s.abandon(ctx, schedule, now, core.ScheduleSkipped,
			fmt.Sprintf("错过的触发点（%s）按「不补跑」弃掉，等下一个点",
				point.In(core.LocalTZ()).Format(time.RFC3339)))
	}
	if missed {
		// 选过"补跑"才走到这里。补**只补最近这一次**（水位抬到 now）：整条链一跑
		// 就是几分钟，逐点追补等于把同一批任务连打几遍。与 [Server.launchScheduled]
		// 里同一格的处理一模一样。
		point = now
	}
	if perr := s.markFired(ctx, schedule, point); perr != nil {
		return false, schedule.ID + "：" + perr.Message()
	}
	s.runPipelineAsync(ctx, scopeOfSchedule(schedule), pipeline,
		s.pipelineScheduleCompletion(ctx, schedule, now))
	return true, ""
}

// pipelineScheduleCompletion 是组合跑完之后那一步的记账（两条账，不是一条）：
//
//   - `schedules.last_*`：这一行定时任务"到点去没去、跑成什么样"；
//   - `pipelines.last_*` 由 [Server.runPipelineAsync] 自己写，这里不重复。
//
// 上下文再脱开一次的理由与 [Server.scheduleCompletion] 完全相同。
func (s *Server) pipelineScheduleCompletion(
	ctx context.Context, schedule *core.Schedule, firedAt time.Time,
) func(PipelineResult, *core.Error) {
	return func(result PipelineResult, perr *core.Error) {
		status, note := scheduledOutcomeForPipeline(result, perr)
		detached := context.WithoutCancel(ctx)
		if status == core.ScheduleSkipped {
			s.recordScheduledRun(detached, schedule, nil, status, note)
			return
		}
		s.recordScheduledRun(detached, schedule, &firedAt, status, note)
	}
}

// scheduledOutcomeForPipeline 把一次组合执行的结论翻成 `schedules.last_status`。
//
// `ScheduleStatus` 只有 ok / error / skipped 三档，**没有** partial，所以组合那一档
// 的 partial 在这里只能落进 error —— 这与 [scheduledOutcome] 对单条能力做的事一致
// （"部分失败也记 error"：没有人当场看着时，把"一半没成"记成 ok 等于把它藏起来）。
func scheduledOutcomeForPipeline(
	result PipelineResult, perr *core.Error,
) (core.ScheduleStatus, string) {
	if perr != nil {
		return core.ScheduleError, perr.Message()
	}
	if pipelineAllBlocked(result) {
		return core.ScheduleSkipped, "所有账号都正被另一趟占着，本次不排队"
	}
	if result.OK() {
		return core.ScheduleOK, ""
	}
	return core.ScheduleError, result.SummaryLine()
}

// scheduleCompletion 是跑完之后的那一步：把结论写回那一行。
//
// ⚠ 它跑在 [launchRun] 那个**脱离调用方**的 goroutine 里，所以上下文在这里
// 再脱开一次（调度那一轮的 ctx 早就结束了），而库里那一行可能已被删 ——
// `RecordScheduleRun` 返回 false 就是这种情况，此时什么都不做。
//
// `firedAt` 用的是**触发那一刻**而不是跑完的时刻：界面上那句"上次执行"
// 回答的是"我设的那个点它去没去"。
func (s *Server) scheduleCompletion(
	ctx context.Context, schedule *core.Schedule, firedAt time.Time,
) func(app.RunSummary, error) {
	return func(summary app.RunSummary, err error) {
		status, note := scheduledOutcome(summary, err)
		detached := context.WithoutCancel(ctx)
		if status == core.ScheduleSkipped {
			// 一个账号都没跑成（全被另一趟挡下）⇒ `last_run_at` 不写：
			// 那一列说的是"跑过"，抹掉上一次真跑的时刻会让界面显示"从来不执行"。
			s.recordScheduledRun(detached, schedule, nil, status, note)
			return
		}
		s.recordScheduledRun(detached, schedule, &firedAt, status, note)
	}
}

// scheduledOutcome 把一次运行的结论翻成调度那一列。
//
// 映射表（与旧实现的口径逐条对齐，除注了的）：
//
//   - 编排自身报错（写库那一类）⇒ error，文本是那句错误；
//   - **一个账号都没跑**且全被挡 ⇒ skipped（对应旧 croner `protect:true` 的
//     "撞上在跑就跳过、不排队"）；
//   - [core.OutcomeSuccess] ⇒ ok；
//   - 其余（部分成功 / 要人操作 / 失败）⇒ **error**。
//
// ⚠ 最后一条是**刻意沿用**旧口径："部分失败也记 error"看着像夸大，但定时任务
// 没有人当场看着，把"一半没成"记成 ok 等于把它藏起来 —— 而 ok 配一句旧错误
// 在界面上并排出现，正是查不出原因的形态。
func scheduledOutcome(summary app.RunSummary, err error) (core.ScheduleStatus, string) {
	if err != nil {
		return core.ScheduleError, err.Error()
	}
	if len(summary.Ran()) == 0 && summary.Skipped() > 0 {
		return core.ScheduleSkipped, fmt.Sprintf(
			"%d 个账号正被另一趟占着，本次不排队", summary.Skipped())
	}
	if summary.Outcome == core.OutcomeSuccess {
		return core.ScheduleOK, ""
	}
	return core.ScheduleError, summary.SummaryLine()
}

// ─────────────────────────────────────────────────────────────────────────────
// 存储侧的两笔窄写
// ─────────────────────────────────────────────────────────────────────────────

// markFired 推水位，并把"抬不动"当成"这一行不归本进程管"处理。
//
// ⚠ 返回错误时调用方**不许**继续打上游：库里说没处理过、上游已经被真打了一枪，
// 那比漏跑一次糟得多。`written == false`（已被删或归属被改）同样报出来。
func (s *Server) markFired(
	ctx context.Context, schedule *core.Schedule, watermark time.Time,
) *core.Error {
	written, err := s.store.MarkScheduleFire(ctx, scopeOfSchedule(schedule), schedule.ID, watermark)
	if err != nil {
		return asDomain(err)
	}
	if !written {
		return core.NewInternal("水位没抬上去（这一行已被删除或换了归属）")
	}
	return nil
}

// recordScheduledRun 写执行记账。`ranAt` 为 nil 表示"这一列别动"（见
// [core.StateStore.RecordScheduleRun] 那两个 nil 的不对称语义）。
//
// 返回的错误只用于"喊一声"：记账失败**不该**让已经跑完的任务再跑一遍，
// 那份失败会以"界面上还是旧值"的形态露出来，这里把它报给调用方就够，
// 不重试、也不因此改动已经抬上去的水位。
//
// ⚠ 文本一律过 [core.ScrubText] 再落库：`last_error` 装的是上游或编排的
// 原话，而那一列会被 `GET /api/schedules` 原样带出去 —— 库里、界面上、
// 用户截图里都不该出现真凭据或真手机号。
func (s *Server) recordScheduledRun(
	ctx context.Context, schedule *core.Schedule, ranAt *time.Time,
	status core.ScheduleStatus, note string,
) *core.Error {
	var lastError *string
	if text := strings.TrimSpace(core.ScrubText(note)); text != "" && status != core.ScheduleOK {
		lastError = &text
	}
	// `written == false` 不报：最常见的是"跑的这一分钟里任务被人删了"，
	// 那不是故障，而也没有任何地方该为它显示错误。
	_, err := s.store.RecordScheduleRun(
		ctx, scopeOfSchedule(schedule), schedule.ID, ranAt, status, lastError)
	if err != nil {
		return asDomain(err)
	}
	return nil
}

// scopeOfSchedule 从那一行的 owner 反解数据可见范围。
//
// ⚠ 与 HTTP 那一路的 [scopeOf] 是**两个函数**，虽然返回的都是 [core.ScopeUser]：
// 前者的身份来自会话（可能为 nil 的那一半在 [Server.runScheduledTick] 已经挡掉），
// 后者来自登录态。把它们合成一个"给我 scope"的函数会让"无主行"那条守卫
// 变成 `ScopeUser(nil)`，而那是 panic（有意为之，见 [core.ScopeUser]）。
func scopeOfSchedule(schedule *core.Schedule) core.Scope {
	return core.ScopeUser(schedule.Owner)
}

// ─────────────────────────────────────────────────────────────────────────────
// 参数：库里的 body → 能力声明的那几栏
// ─────────────────────────────────────────────────────────────────────────────

// scheduleParams 把那一行翻成分派表要的裸参数。
//
// # `account_ids` 那一列**覆盖** body 里的 `accounts`
//
// 库里"目标账号"是独立一列（界面上那个多选框写的是它，`PUT` 校验归属也校验它），
// 而 `body` 是给能力的透传参数。同一件事有两个地方可写时，以**那一列**为准：
// 让 body 赢等于"在界面上选了账号 A、实际跑的是 body 里那个"，
// 而它指向的可能是别人的账号（那一列的归属校验就白做了）。
//
// # 空数组 = 全部账号
//
// [core.Schedule.AccountIDs] 的语义是"空表示全部"（不是"没有账号"），所以这里
// 留空时**不写** `accounts` 这一栏 —— 能力声明那一栏的缺省正是"全部启用账号"
// （见 [resolveTargets]）。写一个 `"[]"` 进去反而会走通同一条路，但把语义
// 变成了"这里显式给了零个"，读代码的人得分辨两种"零"。
func scheduleParams(schedule *core.Schedule) (map[string]json.RawMessage, *core.Error) {
	raw := map[string]json.RawMessage{}
	body := bytes.TrimSpace(schedule.Body)
	if len(body) > 0 && !bytes.Equal(body, []byte("null")) {
		// 只认对象：数组或字符串都可能是"参数"，而能力声明的键名要按键取，
		// 解不进对象就是数据写错了，报出来比兜一个空参数诚实。
		if err := json.Unmarshal(body, &raw); err != nil {
			return nil, core.NewBadRequest("这条定时任务的参数不是一个 JSON 对象")
		}
	}
	if len(schedule.AccountIDs) > 0 {
		ids := make([]string, 0, len(schedule.AccountIDs))
		for _, id := range schedule.AccountIDs {
			ids = append(ids, id.String())
		}
		encoded, err := json.Marshal(ids)
		if err != nil {
			return nil, core.NewInternal("账号列表翻不成 JSON：" + err.Error())
		}
		raw["accounts"] = encoded
	}
	return raw, nil
}
