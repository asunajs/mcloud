package server

// 调度循环 × 任务组合（`scheduler.go` 的 `launchScheduledPipeline`）的用例。
//
// `scheduler_test.go` 那一份钉的是"判点、水位、跑完之后那一行显示什么"这四件事，
// 它过去对组合只有一条结论：**跑不了**。本文件补上接上执行之后的三条，
// 也就是这一层独有的三个判点：
//
//  1. **到点就跑，而且跑的是同一个执行器** —— 与界面上那枚按钮共用
//     [Server.runPipeline]。两份实现的表现是"手工点是对的、夜里自动跑的那一版少三步"，
//     而那种偏差没有任何一处会红（判据与 `run_dispatch.go` 抬头那条一致）。
//  2. **两条账都要落**：`schedules.last_*`（这一行去没去）与
//     `pipelines.last_*`（这一份清单跑成什么样）。少任何一条，界面上都只看得见
//     一半，而"半夜跑砸了"这件事通常正是从另一半看出来的。
//  3. **错过之后补不补**：组合**没有**能力级默认（一条链里既有"错过就别补"的签到、
//     也有"错过仍然值得领"的提醒，替它们 collectively 声明默认等于猜），
//     所以只认用户自己选过的那一次 —— 没选过就是 [core.MissSkip]。
//
// ⚠ 时间全部显式传给 [Server.runScheduledTick]（与 `scheduler_test.go` 同一口径），
// 上游一律用脚本夹具：这里判的是"到点之后有没有真的起跑、结论有没有落回那两行"，
// 不是链跑对了。

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// pipelineSchedulerHarness 起一台"有一个能跑成的组合、账号凭据齐全"的服务端。
//
// 装的是脚本客户端（不是调度那组默认 [refusingClient]）：本文件第一条要判的是
// **两行都记成 ok**，用"第一枪就失败"的夹具会得到两条 error —— 那照样能证明"跑了"，
// 但把"跑成了才算 ok"那一格留成了空白。
func pipelineSchedulerHarness(t *testing.T) (*harness, string) {
	t.Helper()
	h := newHarness(t).initialized(t)
	accountID := seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(),
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}})...)
	h.replaceRunWorld(t, client)
	distinctRunIDs(h)
	return h, accountID
}

// seedStoredPipeline 往库里存一行组合（一步 `prizes`，能跑成）。
//
// 走存储层而不是 `PUT /api/pipelines`：接口那一路的校验另有 `pipelines_test.go`，
// 而这里要的是"库里**已经存在**的那一行"，包括 PUT 造不出来的形态
// （选过补跑策略的定时任务、无主行）。
func seedStoredPipeline(t *testing.T, h *harness, id string) {
	t.Helper()
	pipeline := pPipeline(pStep(capabilities.CodePrizes, ""))
	pipeline.ID = id
	pipeline.Owner = userIDOf(t, h)
	if err := h.store.UpsertPipeline(context.Background(),
		core.ScopeUser(userIDOf(t, h)), &pipeline); err != nil {
		t.Fatalf("存组合失败: %v", err)
	}
}

// pipelineTargetSchedule 造一行"目标 = 某个组合"的定时任务。
func pipelineTargetSchedule(pipelineID string, watermark *time.Time) core.Schedule {
	schedule := core.NewSchedule("job", "夹具组合任务", dailyNoon, core.TZName)
	target := core.NewScheduleTargetPipeline(pipelineID)
	schedule.Target = &target
	schedule.LastFireAt = watermark
	return schedule
}

// waitForPipelineRecorded 等组合那一行的记账落库。
//
// 与 [waitForScheduleRecorded] 同一理由：跑完整条链在后台 goroutine 里。
// ⚠ 顺序上 `pipelines` 那一条**先**写（[Server.runPipelineAsync] 里记账在回调之前），
// 所以调度那一行已经有状态时这一行必然也在 —— 但仍要等，因为两次 tick 之间
// 用户可能看到"上次执行"而组合那一列还是空的。
func waitForPipelineRecorded(t *testing.T, h *harness, id string) core.Pipeline {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		pipeline, err := h.store.GetPipeline(
			context.Background(), core.ScopeUser(userIDOf(t, h)), id)
		if err != nil {
			t.Fatalf("读不回组合 %s：%v", id, err)
		}
		if pipeline != nil && pipeline.LastStatus != nil {
			return *pipeline
		}
		if time.Now().After(deadline) {
			t.Fatalf("等不到组合 %s 的执行记账（行=%v）", id, pipeline)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// getPipelineNow 读一行组合（不等）。
func getPipelineNow(t *testing.T, h *harness, id string) *core.Pipeline {
	t.Helper()
	pipeline, err := h.store.GetPipeline(
		context.Background(), core.ScopeUser(userIDOf(t, h)), id)
	if err != nil {
		t.Fatalf("读不回组合 %s：%v", id, err)
	}
	return pipeline
}

// ─────────────────────────────────────────────────────────────────────────────
// 到点、目标是一个组合
// ─────────────────────────────────────────────────────────────────────────────

// TestSchedulerRunsAPipelineAndRecordsBothRows：准点到点 ⇒ 起跑，并把两条账都写完。
func TestSchedulerRunsAPipelineAndRecordsBothRows(t *testing.T) {
	h, accountID := pipelineSchedulerHarness(t)
	point := beijing(2026, time.September, 20, 14, 0, 0) // == fixedNow
	seedStoredPipeline(t, h, "job")
	seedSchedule(t, h, pipelineTargetSchedule("job", ptrTime(point.AddDate(0, 0, -1))),
		core.ScopeUser(userIDOf(t, h)))

	fired, problems := tickAt(t, h, point.Add(30*time.Second))
	if fired != 1 || len(problems) != 0 {
		t.Fatalf("到点的组合该触发 1 条，实得 %d（问题：%v）", fired, problems)
	}

	schedule := waitForScheduleRecorded(t, h, "job")
	// 水位落在**那个触发点**（迟到 30 秒仍在宽限之内，算准点）—— 与能力那一路同一判据。
	assertWatermark(t, "job", schedule.LastFireAt, point)
	if status := statusOf(t, schedule); status != core.ScheduleOK {
		t.Errorf("这一步跑得成，调度那一行该记 ok，实得 %s（%s）", status, errorText(schedule))
	}
	if schedule.LastRunAt == nil || !schedule.LastRunAt.Equal(point.Add(30*time.Second)) {
		t.Errorf("last_run_at 该是触发那一刻，实得 %v", schedule.LastRunAt)
	}

	pipeline := waitForPipelineRecorded(t, h, "job")
	if pipeline.LastStatus == nil || *pipeline.LastStatus != core.PipelineOK {
		t.Errorf("组合那一行也该记 ok，实得 %v", pipeline.LastStatus)
	}
	if pipeline.LastError != nil {
		t.Errorf("跑成之后组合的 last_error 该清空，实得 %q", *pipeline.LastError)
	}
	if pipeline.LastRunAt == nil || pipeline.LastDurationMS == nil {
		t.Errorf("组合的 last_run_at / last_duration_ms 都该写，实得 %v / %v",
			pipeline.LastRunAt, pipeline.LastDurationMS)
	}
	// 这一趟确实是**那条链**跑出来的（而不是只写了两行账）。
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 1 {
		t.Errorf("调度跑组合该留下 1 行运行记录，实得 %d 行：%v", len(ids), ids)
	}
}

// TestSchedulerSkipsAMissedPipelineNobodyChoseToCatchUp：错过 + 没人选过策略 ⇒ 不补。
//
// ⭐ 这一条挡的是"把组合当成有一条默认的能力"：[capabilities.ResolveMissPolicy] 的
// 第二档是"该能力声明的默认"，而一条链**没有**这样一个默认（判据见
// `scheduler.go` 的 `launchScheduledPipeline`）。写成"跟着某个能力的默认"或
// "默认补跑"，表现都是用户半夜开机后发现签到被多打了一遍。
func TestSchedulerSkipsAMissedPipelineNobodyChoseToCatchUp(t *testing.T) {
	h, accountID := pipelineSchedulerHarness(t)
	missed := missedNoon()
	seedStoredPipeline(t, h, "job")
	seedSchedule(t, h, pipelineTargetSchedule("job", ptrTime(missed)),
		core.ScopeUser(userIDOf(t, h)))

	now := beijing(2026, time.September, 20, 14, 0, 0)
	fired, problems := tickAt(t, h, now)
	if fired != 0 || len(problems) != 0 {
		t.Fatalf("错过的点没人选过补跑，不该起跑：%d / %v", fired, problems)
	}

	schedule := waitForScheduleRecorded(t, h, "job")
	if status := statusOf(t, schedule); status != core.ScheduleSkipped {
		t.Errorf("该记 skipped，实得 %s（%s）", status, errorText(schedule))
	}
	assertWatermark(t, "job", schedule.LastFireAt, now) // 弃掉也要抬水位：一个错过只报一次
	if schedule.LastRunAt != nil {
		t.Errorf("没跑就不该写 last_run_at（它会盖掉上一次真跑的时刻），实得 %v", schedule.LastRunAt)
	}
	// 组合那一行**没有被跑过**，所以四列仍是空的。
	if pipeline := getPipelineNow(t, h, "job"); pipeline.LastStatus != nil {
		t.Errorf("弃掉的这一次不该给组合记账，实得 %v（%v）",
			pipeline.LastStatus, pipeline.LastError)
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
		t.Errorf("弃掉的那一次不该留下运行记录：%v", ids)
	}
}

// TestSchedulerCatchesUpAPipelineOnlyWhenTheRowSaysSo：那一行自己选过"补跑"才补。
//
// ⚠ 补的时候水位抬到 **now** 而不是那个错过的点：整条链一跑就是几分钟，
// 逐点追补等于把同一批任务连打几遍（与 [Server.launchScheduled] 同一格处理）。
func TestSchedulerCatchesUpAPipelineOnlyWhenTheRowSaysSo(t *testing.T) {
	h, accountID := pipelineSchedulerHarness(t)
	catchUp := core.MissCatchUp
	schedule := pipelineTargetSchedule("job", ptrTime(missedNoon()))
	schedule.MissPolicy = &catchUp
	seedStoredPipeline(t, h, "job")
	seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

	now := beijing(2026, time.September, 20, 14, 0, 0)
	fired, problems := tickAt(t, h, now)
	if fired != 1 || len(problems) != 0 {
		t.Fatalf("那一行选过补跑就该起跑：%d / %v", fired, problems)
	}
	got := waitForScheduleRecorded(t, h, "job")
	assertWatermark(t, "job", got.LastFireAt, now)
	if status := statusOf(t, got); status != core.ScheduleOK {
		t.Errorf("补的这一趟跑得成，该记 ok，实得 %s（%s）", status, errorText(got))
	}
	waitForPipelineRecorded(t, h, "job")
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 1 {
		t.Errorf("补跑该留下 1 行运行记录，实得 %d 行：%v", len(ids), ids)
	}
}

// TestSchedulerDoesNotQueueTheSamePipelineTwice：同一条链正被占着 ⇒ 那一行记 skipped。
//
// 这是 [scheduledOutcomeForPipeline] 里那一档 `pipelineAllBlocked` 的落点：
// 旧 croner 的 `protect:true` 说的是"撞上在跑就跳过、不排队"。判红（=组合坏了）
// 与判 ok（=什么都没做）都不对，只有 skipped 对得上"下一次再说"。
//
// ⚠ 占锁的写法与 [TestRunPipelineBlockedStepIsPartial] 同源：**问 builder 要那把键**，
// 自己拼会在参数指纹非空时挡不住任何东西。
func TestSchedulerDoesNotQueueTheSamePipelineTwice(t *testing.T) {
	h, accountID := pipelineSchedulerHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	pipeline := pPipeline(pStep(capabilities.CodePrizes, ""))
	pipeline.ID = "job"
	pipeline.Owner = userIDOf(t, h)
	requests, perr := pipelineRequests(pipeline)
	if perr != nil {
		t.Fatalf("清单合法：%s", perr.Message())
	}
	spec, perr := h.server.pipelineSpecFor(ctx, scope, requests[0])
	if perr != nil {
		t.Fatalf("建计划失败：%s", perr.Message())
	}
	holder := core.NewRunID(h.server.rng)
	key := mcloudexec.NewMutexKey(spec.plan.Capability, accountID, spec.plan.ParamsDigest)
	if admission := h.server.runs.registry.Admit(holder, key); admission.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("夹具占锁失败（前提没成立）：%s", admission.Kind)
	}
	defer func() { h.server.runs.registry.Release(holder) }()

	point := beijing(2026, time.September, 20, 14, 0, 0)
	seedStoredPipeline(t, h, "job")
	seedSchedule(t, h, pipelineTargetSchedule("job", ptrTime(point.AddDate(0, 0, -1))), scope)

	fired, _ := tickAt(t, h, point.Add(30*time.Second))
	if fired != 1 {
		t.Fatalf("到点了，该起跑（哪怕随即被挡下）：%d", fired)
	}
	schedule := waitForScheduleRecorded(t, h, "job")
	if status := statusOf(t, schedule); status != core.ScheduleSkipped {
		t.Errorf("账号正被另一趟占着 ⇒ 这一行该记 skipped，实得 %s（%s）",
			status, errorText(schedule))
	}
	if !strings.Contains(errorText(schedule), "另一趟") {
		t.Errorf("那句话该说清是『被占着』而不是别的失败，实得 %q", errorText(schedule))
	}
	if schedule.LastRunAt != nil {
		t.Errorf("一个账号都没跑成 ⇒ 不该写 last_run_at，实得 %v", schedule.LastRunAt)
	}
	// 被挡下的那一步**没有**打上游。
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
		t.Errorf("被挡下不该留下运行记录：%v", ids)
	}
}
