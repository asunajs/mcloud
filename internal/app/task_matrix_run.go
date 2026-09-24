package app

// #9「App 任务矩阵」的**执行**那一半：把 `task_matrix_dispatch.go` 的决定变成请求，
// 再把"做完之后上游认不认"复查一遍。
//
// # 这一条链**会写**用户的账
//
// 每一发 `task/click` 都在告诉上游"这条我做完了"，云朵可能当场入账且**不可回滚**
// （没有撤销那一口）。所以：
//
//   - 它挂不挂进调度由注册表那一条的 `RunInBatch` 决定（见 `internal/capabilities/registry.go`）；
//   - "本仓没接玩法"的任务一律**不**补一发打卡 —— 那是拿用户的账谎报进度，
//     见 `task_matrix_dispatch.go` 文件头那条"刻意不同"。
//
// # 两趟读取
//
// 旧实现跑完第一趟会**重新拉一次**那张表（`index.ts:44`），用上游回的状态
// 判"这趟做的几条认了没有"。这一格不是装饰：`task/click` 回的是 `success`，
// 它**不报**这条任务的最终状态 —— 四发实拍没有一条带回 `result`。
// 代价是读取请求数翻倍（16 → 32，再加每做一条一枪打卡）。
// 所以有 [TaskMatrixRun.SkipVerify] 这一格，但它在"真发过枪"时**不省**（理由写在那条注释里）。
//
// # 与旧实现不同的三处（各自注释里都有）
//
//  1. `done`（"这趟做了哪些"）只记**真的发过枪**的 id；
//  2. 缺玩法的任务报 [StepBlockedToolGap] 而不是随手打卡；
//  3. 未开备份那道门要的那个状态，本层**自己去读一次** `infoV3`
//     （旧实现读的是 `ctx.state.curMonthBackup`，在那条链上常常没人填 ⇒ 门形同不存在）。

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// appTaskCapabilityCode 是这一族的能力名。
//
// ⚠ 它指回 [capabilities.CodeAppTask] —— code 的唯一成文处在注册表（理由见
// `internal/capabilities/registry.go` 文件头）。
const appTaskCapabilityCode = capabilities.CodeAppTask

// taskMatrixPace 是"发过一枪之后等一会"的那一点间隔（旧实现 `sleep(500)`）。
//
// 它**不是**一个验证过的节流值：旧实现为什么是 500ms 没有出处。
// 保留它的理由是"逐条串行打卡 + 完全不间隔"这件事真实客户端不会做（手点必有间隔），
// 而一旦哪天被判成脚本，最难查的正是这种"看起来最不像原因"的节奏差异。
const taskMatrixPace = 500 * time.Millisecond

// TaskMatrixRun 是这一条链的入口。
type TaskMatrixRun struct {
	SignInWorld

	// Wait 是"等一会"的注入点；nil ⇒ [RealWaiter]（**默认必须真的等**，理由同 [AiAvatarRun.Wait]）。
	Wait Waiter

	// SkipVerify 关掉第二趟复查。
	//
	// ⚠ 只有在**这一趟一枪都没发出去**时关它才是无损的（没东西可复查）。
	// 真发了枪还关复查，结论就退化成"我们发了 N 枪、上游认没认不知道" ——
	// 那是所有未实测写法里最没信息量的一种，所以 [RunAccount] 在发过枪时**无视**这一格。
	SkipVerify bool
}

// TaskStepRecord 是对**一条任务**做过什么的记录（进结论、界面与排障日志）。
type TaskStepRecord struct {
	// TaskID 是任务 id；上游没给 id 的条目不会有记录（分派层直接挡）。
	TaskID int64
	// Name 是去掉 HTML 标签后的任务名。
	Name string
	// Group 是 `groupid`（`day` / `month` / `beiyong1` …）。
	Group string
	// Kind 是处置类别。
	Kind TaskStepKind
	// Runner 是走专属玩法时**哪一条** runner 的名字（来自 [TaskStepPlan.Runner]）；
	// 不发玩法时为空。结论与界面上"这一条是谁做的"只有这一格能回答。
	Runner string
	// Reason 是"为什么是这个类别"。[StepBlockedToolGap] 与 [StepBlockedByRule] 必填 ——
	// 那两格的全部意义就是**说清楚**。
	Reason string
	// Err 是发出去了但失败时的原文（多条用 `；` 串起来）；全成功时为空。
	Err string
}

// TaskVerifyRecord 是复查那一趟对一条任务的读数。
type TaskVerifyRecord struct {
	TaskID int64
	Name   string
	Class  TaskVerifyClass
	// Label 是"该去哪个 App 做"（只有 [VerifyStillOpen] 带）。
	Label string
	// Note 是补打卡那一发的说明（481 / 478 那两条，key 零实拍）。
	Note string
}

// TaskRunCounts 是一次运行的计数。
type TaskRunCounts struct {
	// Read 是第一趟读到的条目数（聚合 + 去重后）。
	Read int
	// Pending 是过了"要不要做"那道门的条数。
	Pending int
	// Clicked 是**成功发出**的打卡条数（按任务计，一条任务打两发算一条）。
	Clicked int
	// Ran 是跑完的专属玩法条数。
	Ran int
	// Failed 是有枪发失败的条数。
	Failed int
	// ToolGap 是"本仓没接玩法"的条数。
	ToolGap int
	// RuleSkipped 是"按规则本来就不发"的条数（SKIP 表 / currstep 门 / 无 id）。
	RuleSkipped int
	// NeedsHuman 是"要人去 App 操作"的条数。
	NeedsHuman int
	// DoneNow / NotAcknowledged / AlreadyWas / StillOpen 是复查那四格。
	DoneNow         int
	NotAcknowledged int
	AlreadyWas      int
	StillOpen       int
	// Hidden 是复查趟按旧实现**不写进报告**的那两组条数（计数留着，别当"没读到"）。
	Hidden int
}

// TaskMatrixRunResult 是一个账号这一趟的产出。
type TaskMatrixRunResult struct {
	Account core.AccountID
	// Steps 是第一趟的逐条处置（顺序 = 执行顺序，界面上要能按它讲"我做了什么"）。
	Steps []TaskStepRecord
	// Verified 是复查趟的逐条读数（跳过复查时为空）。
	Verified []TaskVerifyRecord
	// Counts 是上面两段的计数。
	Counts TaskRunCounts
	// ReadFailures 是第一趟**没读到**的组数（来自 [TaskMatrix.Failures]）。
	//
	// ⚠ 它必须一路带到结论：读到 3 条与"只读到 3 组、其余 13 组没读到"是两回事，
	// 后者会让"待做 N 条"凭空变小，而界面上看不出来。
	ReadFailures int
	// Fetches 是第一趟实际发出的读取请求数。
	Fetches int
	// VerifiedAgain 是复查趟实际发出的读取请求数（跳过复查时为 0）。
	VerifiedAgain int
	// Notes 是给人看的零散结论（434 的分享进度、补打卡那两枪、复查没读成）。
	Notes []string
}

// RunAccount 跑一个账号：读表 → 逐条分派 → 复查。
//
// ⚠ 整表读不下来（十六次全挂）在这里**整体报错**，与 [AppTaskMatrix] 同口径；
// 单条打卡失败不整体失败（旧实现也是继续跑完剩下的），失败原文留在
// [TaskStepRecord.Err] 里进结论。
func (r TaskMatrixRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (TaskMatrixRunResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return TaskMatrixRunResult{Account: account, Notes: []string{}}, perr
	}
	result := TaskMatrixRunResult{Account: account, Notes: []string{}}

	matrix, perr := AppTaskMatrix(ctx, src)
	if perr != nil {
		return result, perr
	}
	result.Fetches = matrix.Fetches
	result.ReadFailures = len(matrix.Failures)
	result.Counts.Read = len(matrix.Items)
	pending := matrix.Pending()
	result.Counts.Pending = len(pending)

	exec := newTaskExecutor(r, src, account)
	gate := exec.backupGate(ctx)
	for _, item := range pending {
		plan := PlanTaskStep(item, gate)
		record, fired := exec.apply(ctx, item, plan)
		result.Steps = append(result.Steps, record)
		result.Counts.absorb(record)
		if id, ok := taskIDOf(item); ok && fired {
			exec.done[id] = true
		}
		// 434 是"分享第几天"的多日任务：跑之前先记下名字里那个数，
		// 复查时只有**涨了**才报好消息（旧实现 `index.ts:47-54`）。
		if id, ok := taskIDOf(item); ok && id == 434 {
			if n, found := ShareProgress(taskNameOf(item)); found {
				value := n
				exec.shareBefore = &value
			}
		}
	}

	// 一枪都没发时跳过复查是**无损**的；发过枪就不给跳（见 SkipVerify 那条注释）。
	if r.SkipVerify && len(exec.done) == 0 {
		r.progress(core.LevelInfo, result.SummaryLine())
		return result, nil
	}
	r.verify(ctx, src, &result, &exec)
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// backupGate 读一次 `infoV3` 拿"本月开没开自动备份"，交回分派要的外部事实。
//
// 读失败 ⇒ 交回 `BackupKnown=false`（门**不挡**任务）。这不是偷懒：旧实现在
// 那一格没值时同样不挡，而"读不到就全跳过"会让一次网络抖动变成"这个号今天没事可做"。
func (e *taskExecutor) backupGate(ctx context.Context) taskStepInput {
	in := taskStepInput{DayOfMonth: e.dayOfMonth()}
	status, perr := SignInStatus(ctx, e.src)
	if perr != nil || status.CurMonthBackup == nil {
		return in
	}
	in.BackupKnown = true
	in.BackupOn = *status.CurMonthBackup
	return in
}

// dayOfMonth 给"这个月的第几天"，按**东八区**算。
//
// ⚠ 不能用 UTC 的日：[core.Clock] 交回的是 UTC，而那道门（旧实现 `getDate() < 20`）
// 是本地日历上的 20 号。UTC 读数会让每月 19 日 08:00–23:59 这一窗口的判定
// 与真实客户端差一天。
//
// ⚠ 时钟没接（`Clock == nil`，CLI 单次调用与部分测试夹具）时交回 0 ⇒
// [taskStepInput.backupGateBlocks] 恒 false ⇒ 门不挡。这是"读不到就当不知道"
// 的同一取向，不是漏判。
func (e *taskExecutor) dayOfMonth() int {
	if e.run.Clock == nil {
		return 0
	}
	return e.run.Clock.Now().In(appTaskLocation()).Day()
}

// verify 打第二趟：重新拉表 → 补该补的那一发 → 按状态分四格。
func (r TaskMatrixRun) verify(
	ctx context.Context, src *SignInSessionSource,
	result *TaskMatrixRunResult, exec *taskExecutor,
) {
	second, perr := AppTaskMatrix(ctx, src)
	if perr != nil {
		// 复查失败**不**推翻第一趟：枪确实发出去了。它只让"认没认"这一格变成未知。
		result.Notes = append(result.Notes, "复查没读到，所以完成数无法核对："+perr.Error())
		return
	}
	result.VerifiedAgain = second.Fetches
	for _, item := range second.Items {
		id, ok := taskIDOf(item)
		if !ok {
			continue
		}
		if id == 434 {
			// 多日任务：旧实现只报"进度涨了没有"，不参与那四格。
			if n, found := ShareProgress(taskNameOf(item)); found {
				if exec.shareBefore != nil && n > *exec.shareBefore {
					result.Notes = append(result.Notes, fmt.Sprintf(
						"分享文件有好礼：已第 %d 天（跑之前是 %d 天）", n, *exec.shareBefore))
				}
			}
			continue
		}
		note := ""
		if key, again := PlanVerifyClick(item); again {
			if _, cerr := r.clickOnce(ctx, src, key, id); cerr != nil {
				note = "补打卡失败：" + cerr.Error()
			} else {
				note = "补打卡已发出（" + key + "，这一枚 key 零实拍）"
			}
			// 口径同 [apply]：**发出过**就算"这一条今天动过"。
			// 于是补打失败的那一条落在"做了没认"，而不是"仍待办"（后者是说给用户的
			// "这一条我们没碰，你去 App 里做"，而我们明明碰了）。
			exec.done[id] = true
		}
		class := ClassifyVerifiedTask(item, exec.done)
		record := TaskVerifyRecord{TaskID: id, Name: taskNameOf(item), Class: class, Note: note}
		if class == VerifyStillOpen {
			record.Label = VerifyMarketLabel(item)
		}
		result.Verified = append(result.Verified, record)
		result.Counts.absorbVerify(class)
	}
}

// clickOnce 发一发打卡（过 [do]：撞 90001 刷一次票、重放一次）。
func (r TaskMatrixRun) clickOnce(
	ctx context.Context, src *SignInSessionSource, key string, id int64,
) (upstream.TaskClick, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.TaskClick, *core.Error) {
		return src.up.AppTaskClick(ctx, s.jwt, s.referer, key, id)
	})
}

// taskExecutor 是"把决定变成请求"那一层带的**跨条状态**。
type taskExecutor struct {
	run     TaskMatrixRun
	src     *SignInSessionSource
	account core.AccountID
	// done 是这趟**真的发过枪**的 id 集合（复查那四格的输入）。
	done map[int64]bool

	// deviceID 是惰性取的（只有那两条入口类 runner 需要它）。
	// 取失败时 [deviceErr] 记下原因：那两条各自记失败、其余任务照跑 ——
	// 一个活动侧接口挂了不该让整趟变成"这个号没跑成"。
	deviceID  string
	deviceErr *core.Error
	deviceGot bool

	// shareBefore 是跑之前从 434 的名字里读到的天数。
	shareBefore *int
}

func newTaskExecutor(
	r TaskMatrixRun, src *SignInSessionSource, account core.AccountID,
) taskExecutor {
	return taskExecutor{run: r, src: src, account: account, done: map[int64]bool{}}
}

// apply 执行一个决定，交回那条任务的记录与"**这一条今天动过没有**"。
//
// ⚠ 第二个返回值是"**打那一枪了**"，**不是**"上游认了"。两格混成一个的表现：
// 上游回一句 `任务已过期` 时这条今天**没被记成动过**，于是复查那趟把它念成
// "仍待办"（VerifyStillOpen）—— 而那正是"我们没做，用户你自己去做"的那一档。
// 用户会看到一条他其实已经打过、只是打失败的枪挂在"仍待办"里；真正该说的是
// "做了、上游没认"（VerifyFailed / `notAcknowledgedCount`）。
// 所以这里只在**真的发出过请求**时回 true；上游怎么回应由 [TaskStepRecord.Err] 说。
func (e *taskExecutor) apply(
	ctx context.Context, item upstream.TaskV2Item, plan TaskStepPlan,
) (TaskStepRecord, bool) {
	record := TaskStepRecord{
		TaskID: derefTaskID(item), Name: taskNameOf(item),
		Group: taskGroupOf(item), Kind: plan.Kind, Runner: plan.Runner, Reason: plan.Reason,
	}
	var fired bool
	switch plan.Kind {
	case StepClick:
		_, cerr := e.run.clickOnce(ctx, e.src, plan.ClickKey, record.TaskID)
		fired = true // 请求发出去了就算动过 —— 成没成看 Err，见函数头
		record.Err = errText(cerr)
	case StepRunner:
		fired = e.runRunner(ctx, record.TaskID, plan, &record)
	case StepBlockedToolGap, StepBlockedByRule, StepNeedsHuman:
		// 什么都不发。这正是这三态的意义。
	}
	if record.Reason == "" && plan.ToolGap != "" {
		record.Reason = plan.ToolGap
	}
	if fired && plan.Paced {
		e.pace(ctx)
	}
	return record, fired
}

// runRunner 跑一条专属玩法，交回"**有没有真的发出过请求**"（口径同 [apply]）。
//
// 今天的两条形状一样（`前置打卡(受门约束) → mrp 入口上报 → 自己的那一发打卡`），
// 差别只在最后一发的 key，所以按 spec 走，不为两条各写一份。
//
// ⚠ "取不到 deviceId"那一档**不算发过**：入口上报压根没出网（旧实现在那里会带一发
// 空 `deviceId`，我们不跟着做，成文见 `app_task_click.go` 文件头）。
func (e *taskExecutor) runRunner(
	ctx context.Context, id int64, plan TaskStepPlan, record *TaskStepRecord,
) bool {
	spec, ok := taskMatrixRunners[id]
	if !ok {
		record.Kind = StepBlockedToolGap
		record.Reason = "分派表说走 runner，但 runner 表里没有这一条（内部不一致）"
		return false
	}
	fired := false
	var problems []string
	if plan.ClickKey != "" {
		if _, cerr := e.run.clickOnce(ctx, e.src, plan.ClickKey, id); cerr != nil {
			problems = append(problems, "前置打卡失败："+cerr.Error())
		}
		fired = true
	}
	if spec.OpensEntry {
		deviceID, derr := e.entryDevice(ctx)
		switch {
		case derr != nil:
			problems = append(problems, "入口上报没做成（取不到 deviceId）："+derr.Error())
		default:
			if err := e.entryReport(ctx, deviceID); err != nil {
				problems = append(problems, "入口上报失败")
			}
			fired = true
		}
	}
	if spec.FinalKey != "" {
		if _, cerr := e.run.clickOnce(ctx, e.src, spec.FinalKey, id); cerr != nil {
			problems = append(problems, "玩法打卡失败："+cerr.Error())
		}
		fired = true
	}
	record.Err = strings.Join(problems, "；")
	return fired
}

// entryDevice 惰性取 H5 `deviceId`（同一趟只取一次，失败也只试一次）。
func (e *taskExecutor) entryDevice(ctx context.Context) (string, *core.Error) {
	if e.deviceGot {
		return e.deviceID, e.deviceErr
	}
	e.deviceGot = true
	deviceID, perr := e.run.DeviceIDFor(ctx, e.account)
	e.deviceID, e.deviceErr = deviceID, perr
	return deviceID, perr
}

// entryReport 打"云朵中心入口"那一发（**零实拍**，见 `app_task_click.go` 文件头）。
func (e *taskExecutor) entryReport(ctx context.Context, deviceID string) *core.Error {
	_, perr := do(ctx, e.src, func(s SignInSession) (struct{}, *core.Error) {
		return struct{}{}, e.src.up.AppMrpDoTask(ctx, s.jwt, s.referer, deviceID)
	})
	return perr
}

// pace 等那一点间隔。等不住（ctx 取消）时**静默返回**：
// ctx 已经取消，后面每一枪都会自己失败，没必要在这里再造一条错误文本。
func (e *taskExecutor) pace(ctx context.Context) {
	wait := e.run.Wait
	if wait == nil {
		wait = RealWaiter{}
	}
	_ = wait.Wait(ctx, taskMatrixPace)
}

// ─────────────────────────────────────────────────────────────────────────────
// 计数与结论
// ─────────────────────────────────────────────────────────────────────────────

// absorb 把一条记录并进计数。
//
// ⚠ 类别与"成没成"是两格：[StepClick] 也可能带着 `Err`（发出去了但上游说失败）。
// 把它们混成"成功/失败"两档的表现是：`Clicked` 里混进失败的枪，而复查那趟
// "做了却没认"的数不上涨 ⇒ 两头的数互相对不上，谁也不知道该信哪一个。
func (c *TaskRunCounts) absorb(record TaskStepRecord) {
	failed := record.Err != ""
	switch record.Kind {
	case StepClick:
		if failed {
			c.Failed++
			return
		}
		c.Clicked++
	case StepRunner:
		if failed {
			c.Failed++
			return
		}
		c.Ran++
	case StepBlockedToolGap:
		c.ToolGap++
	case StepBlockedByRule:
		c.RuleSkipped++
	case StepNeedsHuman:
		c.NeedsHuman++
	}
}

// absorbVerify 并复查那一格。
func (c *TaskRunCounts) absorbVerify(class TaskVerifyClass) {
	switch class {
	case VerifyDoneNow:
		c.DoneNow++
	case VerifyFailed:
		c.NotAcknowledged++
	case VerifyAlreadyWas:
		c.AlreadyWas++
	case VerifyStillOpen:
		c.StillOpen++
	case VerifyHidden:
		c.Hidden++
	}
}

// SummaryLine 是一句人话结论。
//
// 顺序是刻意的：先说"这趟做了什么"，再说"什么没做、为什么"，最后才是复查读数。
// 反过来写会把"上游没认"这件最该看见的事埋在一堆计数后面。
func (res TaskMatrixRunResult) SummaryLine() string {
	c := res.Counts
	parts := []string{fmt.Sprintf("任务表 %d 条（待做 %d 条）", c.Read, c.Pending)}
	if c.Clicked+c.Ran > 0 {
		parts = append(parts, fmt.Sprintf("本次上报 %d 条（打卡 %d、玩法 %d）",
			c.Clicked+c.Ran, c.Clicked, c.Ran))
	}
	if c.Failed > 0 {
		parts = append(parts, fmt.Sprintf("%d 条上报失败", c.Failed))
	}
	if c.ToolGap > 0 {
		parts = append(parts, fmt.Sprintf("%d 条本工具还没接上玩法", c.ToolGap))
	}
	if c.NeedsHuman > 0 {
		parts = append(parts, fmt.Sprintf("%d 条要人去 App 里开一下", c.NeedsHuman))
	}
	if res.VerifiedAgain > 0 {
		parts = append(parts, fmt.Sprintf("复查：认下 %d、没认 %d、此前已完成 %d、仍待办 %d",
			c.DoneNow, c.NotAcknowledged, c.AlreadyWas, c.StillOpen))
	} else {
		parts = append(parts, "未复查")
	}
	if res.ReadFailures > 0 {
		parts = append(parts, fmt.Sprintf("有 %d/%d 组没读到（这不是全表）",
			res.ReadFailures, res.Fetches))
	}
	line := strings.Join(parts, "；")
	if len(res.Notes) > 0 {
		line += "。" + strings.Join(res.Notes, "。")
	}
	return line
}

// RunReport 把这一趟映射到四态结论。
//
// | 情形（自上而下第一个命中） | 结论 |
// | --- | --- |
// | 有"要人去 App 做"的（未开备份） | [core.OutcomeManualRequired] |
// | 复查说"做了但没认" / 有枪发失败 / 有玩法缺口 | [core.OutcomePartialSuccess] |
// | 其余（含"今天没有待做的任务"） | [core.OutcomeSuccess] |
//
// ⚠ "一条枪都没发"**不是**失败：一个当天没有待做任务的号跑出来就是零条，
// 判成失败会让每天的历史里全是噪声（与 [TaskListResult.RunReport] 同一条理由）。
//
// ⚠ [StepBlockedToolGap] 落 PartialSuccess 而不是 ManualRequired：那是**本工具的**缺口，
// 要人去 App 点什么并不能补上它 —— 把它报成"需人工"会把工单派错人。
func (res TaskMatrixRunResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	c := res.Counts
	manual := []core.ManualStep{}
	if c.NeedsHuman > 0 {
		account := res.Account
		manual = append(manual, core.NewManualStep(appTaskCapabilityCode, &account,
			"有几条每月任务要先在 App 里开启自动备份才能做（本工具不代开）",
			core.TargetMobileApp))
	}
	outcome := core.OutcomeSuccess
	if c.NotAcknowledged > 0 || c.Failed > 0 || c.ToolGap > 0 {
		outcome = core.OutcomePartialSuccess
	}
	if len(manual) > 0 {
		outcome = core.OutcomeManualRequired
	}
	return core.RunReport{Outcome: outcome, ManualSteps: manual, Summary: &summary}
}

// derefTaskID 取 id（没有 id 的条目交回 0 —— 那种条目在分派层就被挡了，不会走到执行）。
func derefTaskID(item upstream.TaskV2Item) int64 {
	if item.TaskID == nil {
		return 0
	}
	return *item.TaskID
}

// errText 把错误摊成一句文本（nil ⇒ 空串）。
func errText(err *core.Error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// appTaskLocation 是这一族"本地日历"的口径（东八区）。
//
// 与 [upstream.ExchangeTimeLocation] / `FunaiActivityLocation` 同值不同名，
// 那两处是各自族自己的常量；这里单列一份，是为了让"哪一族的门用哪份日历"
// 这件事在各自文件里可读，而不是三族共享一个来路不明的全局时区。
func appTaskLocation() *time.Location { return time.FixedZone("UTC+8", 8*3600) }
