package server

// 组合执行：一条**有序清单**逐步跑完，也就是"一键执行几乎全部"的那一条路。
//
// # 为什么要有"组合"这一层，而不是给每条能力各加一枚按钮
//
// 旧实现的默认入口就是这个形状：`packages/cli/src/index.ts:127-152` 那一串
// `await signIn(ctx)` → … → `await afterTask(ctx)`，一次跑完全部任务。
// 本仓把它落成 [core.Pipeline]（有序步骤 + 每步的参数 + 失败后停不停），因为
// "全跑"与"跑其中五条、带上我自己的参数"是同一件事的两种取值 —— 为前者另写一套
// 执行器，等于同一份"逐步跑"的逻辑有两个地方会漏步骤。
//
// # 逐步跑复用 [runBuilders]，不新写一份
//
// 每一步就是一个能力 code。跑法与界面上"开始"那枚按钮、与调度到点那一路**完全相同**：
// [Server.launchRun] → 那个 builder → [app.Runner.Run]。组合层只多做三件事：
// 等这一步跑完、按 [core.Pipeline.ContinuesAfterFailure] 判要不要继续、把结论
// 汇总成 [core.PipelineStatus]。与 `run_dispatch.go` 抬头那条纪律同源：
// 同一件事有两份实现时，"界面上跑对了、组合里少一步"不会有任何测试会红。
//
// # 与旧实现的两处**刻意不同**（都不是照抄不动的地方）
//
//   - **步骤在外、账号在内**（旧实现是账号在外、任务在内）。
//     差别只在"上游看到的请求交错方式"，而每步内部仍是逐账号串行（[app.Runner.Run]），
//     所以防风控那条节奏没有放宽。选这个方向的硬理由：`runs` 表的形状是
//     "一行 = 一次能力 × 一个账号"，反过来跑要先把计划切成"单账号 × 单能力"再跑
//     十七八次，多出十几条没人读的中间行，而换来的是同一个终局。
//   - **收尾只推一条**（旧实现也是整链跑完推一条）。
//     单步若不压住，一次一键全跑会推十七八条（每步各一条），而用户手机上那个
//     表现与"推送坏了"难以区分。所以组合里的每一步都带 [launchOptions] 的
//     `suppressNotify`，跑完由 [runPipeline] 自己发一次汇总。
//
// # 跑完之后账记在哪
//
// `pipelines` 表那四列（`last_run_at` / `last_status` / `last_error` /
// `last_duration_ms`）自 schema 落下起没人写，本层是第一个写入方，走
// [core.StateStore.RecordPipelineRun] 的**窄写**（只碰这四列）：一次一键全跑要跑
// 几分钟，用全量 `Upsert` 会把这段时间里用户改过的名字与步骤一起抹掉。
//
// ⚠ 内置那一版"一键全跑"（[PipelineRunAllID]）**没有行**，所以它没有这四列可写。
// 那不是遗漏：给一个"所有人共用的默认清单"造一行数据，等于让第一个改它的人替
// 所有人改了默认值。它的结局只看历史页与推送。

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// PipelineRunAllID 是内置"一键全跑"的保留 id。
//
// 保留而不是"取第一个组合"：界面上那枚按钮与调度目标都要能指到一个**不随数据变**
// 的东西。用户删光自己建过的组合，不该让那枚按钮变成 404。
//
// ⚠ 它是 id 空间里的一个**reserved**值，不是行：[putPipeline] 拒掉这个 id，
// 免得有人建出一行同名组合之后，界面上那枚按钮忽然跑的是他的清单。
const PipelineRunAllID = "all"

// ─────────────────────────────────────────────────────────────────────────────
// 内置清单：一键执行几乎全部
// ─────────────────────────────────────────────────────────────────────────────

// allRunPipeline 是内置那一版"全跑"。
//
// # 顺序照旧实现，逐条对应
//
// `signIn → revivalRewardTask → taskExpansionTask → [aiAvatar] → playAiSpecial
// → playAiLottery → [cloudPhoneRedpack] → [liveRoomKouling] → msgPushOn
// → notice → backupGift → appTask → [hc1t] → printPendingPrizes → receive → after`
// （方括号 = 旧实现里由账号配置开关决定，其余无条件）。
//
// # 为什么默认清单里**没有** `exchange` / `live-room` / `ai-avatar` / `task-list`
//
//   - `exchange`（兑换）：旧实现也不在默认链里，而且它**不可逆扣云朵**、要人当场选品
//     —— 一键跑不该替人做这个决定（本仓另有一条独立页面与二次确认）。
//   - `live-room`（直播口令）、`ai-avatar`（AI 头像）：旧实现默认**关**
//     （`config.直播口令?.开启` / `config.AI新头像?.开启`，默认值见
//     `packages/shared/src/config/schema.ts`）。前者还要每个账号各听 60 秒弹幕，
//     后者那一条链**不清理**自己写进网盘的文件（见 `registry.go` 那一条的代价段）。
//     放进"谁都会按的一枚按钮"里，等于把这两笔代价隐到默认路径上。
//   - `task-list` / `cloud-phone-redpack` / `play-ai-special` 里的**只读那一半**：
//     红包派对与拍拍旧实现默认开、且我们只有只读实现（盘点，不代做），所以它们在；
//     `task-list` 旧实现没有这一条（它只是本仓的"看任务表"接口），不塞进链里凑数。
//   - `auth-refresh` 与 `cloud-snapshot` 排在末尾，替的是旧实现最后那一步
//     `afterTask`（统一清理 + 打印今日云朵 + auth 维护）：清理今天由各条链自己在
//     收尾做（见 `app/task_matrix_run.go` 的 `cleanupCloudFiles`），剩下两半各接一条：
//     快照 = 今日云朵，刷新 = auth 维护（它自己判"到没到提前量"，没到就一发请求都不发）。
//
// ⚠ `StopOnError` 取 **false**（一步失败继续跑后面的）：这与旧实现同一取向 ——
// 那串 `await` 外面是一个 `try/catch` 包住**整条链**，任何一步抛错都会让后面的
// 全部不跑，而签到坏了不该连累领取。这里让失败**记账**而不是**截断**。
func allRunPipeline() core.Pipeline {
	code := func(codes ...string) []core.PipelineStep {
		steps := make([]core.PipelineStep, 0, len(codes))
		for _, one := range codes {
			steps = append(steps, core.PipelineStep{Task: one})
		}
		return steps
	}
	pipeline := core.NewPipeline(PipelineRunAllID, "一键全跑（除兑换）")
	pipeline.Description = ptrString("按旧实现的默认顺序跑完全部任务；兑换与直播、AI 头像不在里面，理由见代码注释")
	pipeline.Steps = code(
		capabilities.CodeSign,
		capabilities.CodeRevival,
		capabilities.CodeMultiple,
		capabilities.CodePlayAISpecial,
		capabilities.CodeFunAI,
		capabilities.CodeCloudPhoneRedpack,
		capabilities.CodeMsgPush,
		capabilities.CodeNotice,
		capabilities.CodeBackupGift,
		capabilities.CodeAppTask,
		capabilities.CodePrizes,
		capabilities.CodeReceive,
		capabilities.CodeCloudSnapshot,
		capabilities.CodeAuthRefresh,
	)
	return pipeline
}

func ptrString(text string) *string { return &text }

// ─────────────────────────────────────────────────────────────────────────────
// 结论形状
// ─────────────────────────────────────────────────────────────────────────────

// PipelineStepOutcome 是组合里**一步**的结局。
//
// 刻意带 `Runs`（这一步实际起跑的账号↔runId 配对）：组合本身没有事件流，
// 界面要跟着看进度只能拿这些 runId 去订阅单趟那一条 —— 少了这一格，前端就只能
// 靠"隔一会儿刷一次历史"猜现在跑到哪了。
type PipelineStepOutcome struct {
	Index  int                 `json:"index"`
	Task   string              `json:"task"`
	Title  string              `json:"title,omitempty"`
	Status core.PipelineStatus `json:"status"`
	Note   string              `json:"note,omitempty"`
	// Ran / Blocked 是这一步**真的跑了**与**被另一趟挡下**的账号数。
	//
	// 为什么不只留 `Runs`：整条链的"一个账号都没跑成"要看**每一步**这两格，
	// 而从 `Runs` 反推要求调用方知道"目标账号一共几个"（每一步可能不同）。
	// 调度那一档 `skipped` 就落在这两格上（见 [scheduledOutcomeForPipeline]）。
	Ran     int              `json:"ran"`
	Blocked int              `json:"blocked"`
	Runs    []map[string]any `json:"runs,omitempty"`
}

// PipelineResult 是一次组合执行的汇总（导出给 CLI / 测试与将来的组合历史用）。
type PipelineResult struct {
	PipelineID string                `json:"pipeline_id"`
	Name       string                `json:"name"`
	Status     core.PipelineStatus   `json:"status"`
	Note       string                `json:"note,omitempty"`
	Steps      []PipelineStepOutcome `json:"steps"`
	StartedAt  time.Time             `json:"started_at"`
	DurationMS int64                 `json:"duration_ms"`
	// StoppedAfter 是"跑完第几步就停了"，`-1` 表示一步都没停（跑到了清单末尾）。
	StoppedAfter int `json:"stopped_after"`
	// Notify 是收尾那一条推送的结论；nil = 这台服务端没接推送。
	Notify *app.RunNotifyResult `json:"-"`
}

// OK 报整体结局是不是"全绿"。
func (r PipelineResult) OK() bool { return r.Status == core.PipelineOK }

// SummaryLine 是给人看的那一句（进 `last_error`、推送正文、日志）。
//
// 三格都要有：跑了几步、结局、以及**没跑完时停在第几步**。少了最后一格，
// "13 步里第 4 步失败、按设定继续"与"第 4 步失败、后面 9 步没跑"在界面上长得一样，
// 而前者的处置是修那一步、后者是查为什么停了。
func (r PipelineResult) SummaryLine() string {
	failed := 0
	for _, step := range r.Steps {
		if step.Status == core.PipelineError {
			failed++
		}
	}
	line := fmt.Sprintf("任务组合「%s」：跑了 %d 步、其中 %d 步失败，%s",
		r.Name, len(r.Steps), failed, pipelineStatusWord(r.Status))
	if r.StoppedAfter >= 0 {
		line += fmt.Sprintf("（按设定停在第 %d 步）", r.StoppedAfter+1)
	}
	if note := strings.TrimSpace(r.Note); note != "" {
		line += "：" + note
	}
	return line
}

func pipelineStatusWord(status core.PipelineStatus) string {
	switch status {
	case core.PipelineOK:
		return "全部跑完且没有失败"
	case core.PipelinePartial:
		return "跑完了，但有步骤没跑成或被挡下"
	case core.PipelineError:
		return "有步骤失败"
	}
	return "结局未知"
}

// ─────────────────────────────────────────────────────────────────────────────
// 预检：整条链能不能开始
// ─────────────────────────────────────────────────────────────────────────────

// pipelineStepRequest 是一步**过了预检**的样子：能力在、执行接上了、参数键名对得上。
type pipelineStepRequest struct {
	step       core.PipelineStep
	capability capabilities.Capability
	params     map[string]json.RawMessage
}

// pipelineRequests 把清单翻成"能跑的一步一步"，任何一步翻不出来就整体报错。
//
// # 为什么要**先全量预检**再动手
//
// 第 9 步的键名拼错，最坏的表现不是"那一步没跑"，而是**前面 8 步已经把真上游打了
// 一遍**（签到、领奖都是写操作），然后整条链回一个 400。预检把这类错误挪到
// 一个请求都没发之前；代价是一份清单要构造两次参数（这里一次、起跑时 builder 里一次），
// 而那是纯内存的 JSON 解码。
func pipelineRequests(pipeline core.Pipeline) ([]pipelineStepRequest, *core.Error) {
	if len(pipeline.Steps) == 0 {
		return nil, core.NewBadRequest("这个组合一步都没有，没什么可跑的")
	}
	// 组合级的账号列表**覆盖**每一步 body 里的 `accounts` —— 与定时任务同一口径，
	// 判据（"界面上选了 A、不能实际跑 body 里那个人"）逐字见 `scheduleParams`。
	// 空 = 全部启用账号，所以这时**不写**这一栏，让它落到能力声明的默认。
	var override json.RawMessage
	if len(pipeline.AccountIDs) > 0 {
		ids := make([]string, 0, len(pipeline.AccountIDs))
		for _, id := range pipeline.AccountIDs {
			ids = append(ids, id.String())
		}
		encoded, err := json.Marshal(ids)
		if err != nil {
			return nil, core.NewInternal("账号列表翻不成 JSON：" + err.Error())
		}
		override = encoded
	}
	if perr := sequentialOnly(pipeline.AccountMode.String()); perr != nil {
		return nil, perr
	}

	requests := make([]pipelineStepRequest, 0, len(pipeline.Steps))
	for i, step := range pipeline.Steps {
		capability, found := capabilities.Get(step.Task)
		if !found {
			return nil, core.NewBadRequest(
				fmt.Sprintf("第 %d 步没有这个能力：%s", i+1, step.Task))
		}
		if _, wired := builderFor(capability.Code); !wired {
			return nil, core.NewBadRequest(fmt.Sprintf(
				"第 %d 步的能力「%s」还没有接上执行", i+1, capability.Title))
		}
		params, perr := stepParams(step, i, override, pipeline.AccountMode)
		if perr != nil {
			return nil, perr
		}
		if perr := checkParamKeys(capability, params); perr != nil {
			return nil, core.NewBadRequest(
				fmt.Sprintf("第 %d 步（%s）：%s", i+1, capability.Title, perr.Message()))
		}
		requests = append(requests, pipelineStepRequest{
			step: step, capability: capability, params: params,
		})
	}
	return requests, nil
}

// stepParams 解某一步的参数。
//
// `body` 的语义是"这一步要传给能力的参数对象"，与 `schedule.Body` 同一形状，
// 因此也沿用它的判据：**只认对象**。`PipelineStep.Body` 在模型层接受任意 JSON
// （serde 实测，见 [core.PipelineStep.UnmarshalJSON] 第 3 条），所以"存得进去但
// 跑不了"是可能的 —— 这里把它报出来，不静默当空参数跑。
func stepParams(
	step core.PipelineStep, index int, override json.RawMessage,
	mode core.AccountMode,
) (map[string]json.RawMessage, *core.Error) {
	params := map[string]json.RawMessage{}
	body := strings.TrimSpace(string(step.Body))
	if body != "" && body != "null" {
		if err := json.Unmarshal([]byte(body), &params); err != nil {
			return nil, core.NewBadRequest(fmt.Sprintf(
				"第 %d 步（%s）的参数不是一个 JSON 对象", index+1, step.Task))
		}
	}
	if override != nil {
		params["accounts"] = override
	}
	// 执行方式跟着组合走：组合是"串行的一串步骤"，某一步若被 body 写成并行，
	// 它就会与**下一步**抢同一批账号，而下一步的判据（例："本月翻倍要签到在先"）
	// 读的正是上一步写到上游的那个状态。组合级已经在 [pipelineRequests] 里
	// 过 [sequentialOnly]，所以这里给出的永远是 `"sequential"`。
	params["accountMode"] = json.RawMessage(`"` + mode.String() + `"`)
	return params, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 执行
// ─────────────────────────────────────────────────────────────────────────────

// runPipeline 逐步跑一条组合，**阻塞到跑完**。
//
// 同步语义是刻意的：调用方要么自己起 goroutine（HTTP 那一路，见
// [Server.runPipelineAsync]），要么本来就在后台（调度那一路）。
// 本函数不碰 HTTP，也不写库 —— 记账留在调用方，这样"跑"与"记"不会在两条路上分叉。
//
// `perr` 非 nil 只有一种可能：**整条链一步都没开始**（预检没过、拿不到 scope 下的
// 数据）。跑起来之后的失败一律进步骤的 `Status`/`Note`，不往外交 error ——
// 因为第 4 步失败时第 5 步可能还在跑，把它表达成"这个函数失败了"就没人负责
// 收那半截结论了。
func (s *Server) runPipeline(
	ctx context.Context, scope core.Scope, pipeline core.Pipeline,
) (PipelineResult, *core.Error) {
	requests, perr := pipelineRequests(pipeline)
	if perr != nil {
		return PipelineResult{}, perr
	}
	result := PipelineResult{
		PipelineID:   pipeline.ID,
		Name:         pipeline.Name,
		Status:       core.PipelineOK,
		Steps:        make([]PipelineStepOutcome, 0, len(requests)),
		StartedAt:    s.clock.Now(),
		StoppedAfter: -1,
	}
	lines := make([]app.NotifyLogLine, 0, len(requests))

	for i, request := range requests {
		outcome := s.runPipelineStep(ctx, scope, i, request)
		result.Steps = append(result.Steps, outcome)
		lines = append(lines, app.NotifyLogLine{
			Level:   notifyLevelForPipeline(outcome.Status),
			At:      s.clock.Now(),
			Message: pipelineStepLine(outcome),
		})
		// 整体结论：error 盖过 partial，partial 盖过 ok（不会被后面的 ok 翻回绿）。
		if outcome.Status == core.PipelineError {
			result.Status = core.PipelineError
			result.Note = pipelineStepNote(outcome)
		} else if outcome.Status == core.PipelinePartial && result.Status != core.PipelineError {
			result.Status = core.PipelinePartial
			result.Note = pipelineStepNote(outcome)
		}
		if outcome.Status == core.PipelineError && !pipeline.ContinuesAfterFailure(request.step) {
			result.StoppedAfter = i
			result.Note = fmt.Sprintf("第 %d 步失败，按组合的设定停下：%s",
				i+1, pipelineStepNote(outcome))
			break
		}
	}

	// 跑完（或中途停下）之后的收尾：先推一条汇总，再落账。
	result.DurationMS = s.clock.Now().Sub(result.StartedAt).Milliseconds()
	if len(lines) > 0 {
		notify := s.notify.RunNotify().Dispatch(context.WithoutCancel(ctx), app.NotifyInput{
			Summary: result.SummaryLine(),
			Lines:   lines,
		})
		result.Notify = &notify
	}
	return result, nil
}

// runPipelineStep 跑一步并等它结束。
//
// 失败判据与调度的 [scheduledOutcome] 同一族，但**多一档 partial**：定时任务那侧的
// `ScheduleStatus` 只有 ok/error/skipped，没有"部分成功"可表达，所以它把 partial
// 归进 error；组合有这一档，就不做那种夸大（判据与后果见 [pipelineStepStatus]）。
func (s *Server) runPipelineStep(
	ctx context.Context, scope core.Scope, index int, request pipelineStepRequest,
) PipelineStepOutcome {
	outcome := PipelineStepOutcome{
		Index:  index,
		Task:   request.step.Task,
		Title:  request.capability.Title,
		Status: core.PipelineOK,
	}
	spec, perr := s.pipelineSpecFor(ctx, scope, request)
	if perr != nil {
		outcome.Status = core.PipelineError
		outcome.Note = perr.Message()
		return outcome
	}
	summary, perr := s.launchRunWaiting(ctx, spec)
	if perr != nil {
		outcome.Status = core.PipelineError
		outcome.Note = perr.Message()
		return outcome
	}
	outcome.Ran, outcome.Blocked = len(summary.Ran()), summary.Skipped()
	runs := make([]acceptedRun, 0, len(summary.Ran()))
	for _, entry := range summary.Accounts {
		runs = append(runs, acceptedRun{Account: entry.Account, RunID: entry.RunID})
	}
	outcome.Runs = runPairings(runs)
	outcome.Status, outcome.Note = pipelineStepStatus(summary)
	return outcome
}

// pipelineAllBlocked 报"整条链一个账号都没跑成，全是被另一趟占着"。
//
// 判据用**步数**而不是"每一步都 blocked"：只要有一步真跑了账号，就不能记 skipped
// （那一趟确实往上游打了请求，界面上该看到"跑过"）。
func pipelineAllBlocked(result PipelineResult) bool {
	if len(result.Steps) == 0 {
		return false
	}
	ran, blocked := 0, 0
	for _, step := range result.Steps {
		ran += step.Ran
		blocked += step.Blocked
	}
	return ran == 0 && blocked > 0
}

// pipelineStepStatus 把一步的跨账号结论翻成组合那一档。
//
// 映射表（每格都对应一种**界面表现**，不是分类学游戏）：
//   - 一个账号都没起跑、全被另一趟挡下 ⇒ **partial** + "被挡下"那句。
//     不判 error：组合里常见"上一轮还在跑"，判红会让人以为清单坏了；
//     也不判 ok：那等于把"这一趟什么都没做"藏起来（[scheduledOutcome] 里
//     同样有这一格，它那边只能记 skipped，这一档在本层没有对应的第四态）。
//   - [core.OutcomeFailed] ⇒ error。
//   - [core.OutcomeManualRequired] ⇒ partial。这一步没做成**不是故障**，
//     是要人去客户端点一下；判红会把"该你做"和"上游坏了"混成同一句。
//   - [core.OutcomePartialSuccess] ⇒ partial。
//   - [core.OutcomeSuccess] ⇒ ok。
//
// ⚠ `error` 那一档带的是 [app.RunSummary.SummaryLine] 原句：组合没有事件流，
// 界面与推送上唯一能说出"哪一步为什么红"的就是这一句。
func pipelineStepStatus(summary app.RunSummary) (core.PipelineStatus, string) {
	if len(summary.Ran()) == 0 && summary.Skipped() > 0 {
		return core.PipelinePartial, fmt.Sprintf(
			"%d 个账号正被另一趟占着，这一步没跑", summary.Skipped())
	}
	switch summary.Outcome {
	case core.OutcomeSuccess:
		return core.PipelineOK, ""
	case core.OutcomeFailed:
		return core.PipelineError, summary.SummaryLine()
	case core.OutcomeManualRequired:
		return core.PipelinePartial, summary.SummaryLine()
	case core.OutcomePartialSuccess:
		return core.PipelinePartial, summary.SummaryLine()
	}
	return core.PipelinePartial, summary.SummaryLine()
}

// pipelineSpecFor 查分派表并造这一步的计划。
func (s *Server) pipelineSpecFor(
	ctx context.Context, scope core.Scope, request pipelineStepRequest,
) (runSpec, *core.Error) {
	builder, wired := builderFor(request.capability.Code)
	if !wired { // 预检挡过，走到这里是分派表被改坏
		return runSpec{}, core.NewInternal("能力「" + request.capability.Title + "」没有接上执行")
	}
	return builder(ctx, s, scope, request.capability, request.params)
}

// launchRunWaiting 起跑并**等它跑完**。
//
// ⚠ 两条不显然的：
//   - `done` 在 [Server.launchRun] 那个**脱离调用方**的 goroutine 里执行，
//     所以本函数返回时那一趟已经真的写完了库；这里的 select 只是把
//     "回调式"翻成"顺序式"，不改变执行位置。
//   - 上下文被取消（服务在收尾）时**不等**：那一趟会在后台跑完（`WithoutCancel`），
//     但调用方不该在进程退出路上再等它 —— 表现成"Ctrl-C 之后服务停不掉"。
//     此时这一步记 error 并说清是"没等完"，不去猜它后来成没成。
func (s *Server) launchRunWaiting(
	ctx context.Context, spec runSpec,
) (app.RunSummary, *core.Error) {
	type completion struct {
		summary app.RunSummary
		err     error
	}
	finished := make(chan completion, 1)
	_, perr := s.launchRun(ctx, spec, suppressNotify, func(summary app.RunSummary, err error) {
		finished <- completion{summary: summary, err: err}
	})
	if perr != nil {
		return app.RunSummary{}, perr
	}
	select {
	case done := <-finished:
		if done.err != nil {
			// 编排自身的错误（写库失败那一类）：这一步视为失败，后面的按设定办。
			return app.RunSummary{}, asDomain(done.err)
		}
		return done.summary, nil
	case <-ctx.Done():
		return app.RunSummary{}, core.NewInternal("服务正在收尾，这一步没等完")
	}
}

// pipelineStepLine 是一步的过程话（推送正文里那一行）。
func pipelineStepLine(outcome PipelineStepOutcome) string {
	line := fmt.Sprintf("第 %d 步 %s：%s",
		outcome.Index+1, outcome.Title, pipelineStatusWord(outcome.Status))
	if note := pipelineStepNote(outcome); note != "" {
		line += " —— " + note
	}
	return line
}

// pipelineStepNote 取那一步的备注，**先过脱敏**。
//
// 这一句会进 `pipelines.last_error`、推送正文与日志三处，而它装的是编排或上游的
// 原话（`asDomain(err)` 之后是 `core.Error` 的消息）。与 [recordScheduledRun]
// 同一条纪律：库里、界面上、用户截图里都不该出现真凭据或真手机号。
func pipelineStepNote(outcome PipelineStepOutcome) string {
	return scrubNote(outcome.Note)
}

// scrubNote 是"备注出网/落库之前"那一道脱敏，两个调用方共用（一步的备注、整条链的备注）。
func scrubNote(note string) string {
	return strings.TrimSpace(core.ScrubText(note))
}

func notifyLevelForPipeline(status core.PipelineStatus) core.LogLevel {
	switch status {
	case core.PipelineError:
		return core.LevelError
	case core.PipelinePartial:
		return core.LevelWarn
	}
	return core.LevelInfo
}

// ─────────────────────────────────────────────────────────────────────────────
// 记账
// ─────────────────────────────────────────────────────────────────────────────

// recordPipelineRun 把一次组合执行的结论写回那一行。
//
// `stored == false` ⇒ 这个组合**没有行**（内置那一版"全跑"）。此时什么都不写，
// 返回 nil 而不是错误：那不是故障，是这条路的正常形态（判据见本文件抬头）。
//
// ⚠ `last_error` **只在非 ok 时写**，且 ok 时显式清空：
// 与 [recordScheduledRun] 同一格口径 —— `last_status=ok` 配一句上次的错误并排显示，
// 是"查不出原因"最常见的形态。
func (s *Server) recordPipelineRun(
	ctx context.Context, scope core.Scope, pipeline core.Pipeline,
	result PipelineResult,
) *core.Error {
	if pipeline.ID == PipelineRunAllID {
		// 内置那一版（[allRunPipeline]）：库里没有它的行，也就没有四列可写。
		return nil
	}
	status := result.Status
	duration := result.DurationMS
	ranAt := result.StartedAt
	var lastError *string
	if note := scrubNote(result.Note); note != "" && status != core.PipelineOK {
		lastError = &note
	}
	written, err := s.store.RecordPipelineRun(ctx, scope, pipeline.ID,
		&ranAt, status, lastError, &duration)
	if err != nil {
		return asDomain(err)
	}
	if !written {
		// 跑的这一几分钟里组合被人删了（或改了归属）：不是故障，不重跑也不补写。
		return nil
	}
	return nil
}

// runPipelineAsync 起一条后台 goroutine 跑完整条组合，跑完记账。
//
// HTTP 那一路只能这样：一次十七八步的链要跑几分钟，而接口不该挂着
// （[Server.triggerRun] 抬头那条 202 的理由在这里更长）。
//
// ⭐ **整条链都要跑在脱开调用方的上下文上**，不是只有记账那一步。
// 传进来的 ctx 在 HTTP 那一路就是 `r.Context()`，而 net/http 对它的定义是
// "客户端断开、HTTP/2 取消、**或 ServeHTTP 返回**时取消"
// （`Request.Context` 的文档原话：`or when the ServeHTTP method returns`）。
// 202 就是那个处理器返回，于是"只脱开记账"会得到一个极难在现场看清的形态：
// 用户点了那枚按钮 ⇒ 拿到 202 ⇒ 第一步之后**每一步都记成**"服务正在收尾，这一步没等完"
// （[Server.launchRunWaiting] 里那一档），而服务根本没在收尾。
// 更要命的是这个错在 `httptest.ResponseRecorder` 上**测不出来** —— recorder 那条路
// 永远不取消上下文，判据见 [TestPipelineChainSurvivesTheEndOfItsOwnRequest]（它打真服务器）。
//
// 取舍与 [Server.launchRun] 同一份：**一旦受理就让它跑完**，不因"这一次 HTTP 请求结束"
// 或"调度那一次 tick 结束"而腰斩。
//
// ⚠ 随之而来的事实要说清：脱开之后，[Server.launchRunWaiting] 里那一档 `ctx.Done()`
// 在本文件**当前两个**生产调用方（`pipelines.go` 的路由、`scheduler.go` 的到点触发）
// 上都不会再触发 —— 它只剩一个去处：直接把可取消上下文交给 [Server.runPipeline] 的
// 调用方（例如将来 CLI 在前台跑一条链、Ctrl-C 时不干等）。那一档由
// [TestWaitingStepGivesUpWhenTheServerIsClosing] **直接调 runPipeline** 钉着，
// 不是"留着但没人测"的兜底。
func (s *Server) runPipelineAsync(
	ctx context.Context, scope core.Scope, pipeline core.Pipeline,
	done func(PipelineResult, *core.Error),
) {
	detached := context.WithoutCancel(ctx)
	go func() {
		result, perr := s.runPipeline(detached, scope, pipeline)
		if perr == nil {
			perr = s.recordPipelineRun(detached, scope, pipeline, result)
		}
		if done != nil {
			done(result, perr)
		}
	}()
}
