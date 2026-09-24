package server

// 调度循环（`scheduler.go`）的用例。
//
// 这一层只拥有四件事，测试也就只钉这四件：
//
//  1. **什么时候算到点**：水位取哪一列、两列都没有时从哪儿起算、迟到多久算错过；
//  2. **错过之后补不补**：能力声明的默认 + 那一行的显式覆盖，两条都要走到；
//  3. **水位先于起跑**：抬不动就不打上游，抬得动就一个触发点只处理一次；
//  4. **跑完之后那一行显示什么**：`last_status` / `last_error` / `last_run_at`
//     三列各自的写入时机（尤其"`last_run_at` 只在真跑完一趟之后写"）。
//
// ⚠ 链本身**不在这里测**（判据在 `internal/app`）。这里刻意用默认的
// [refusingClient]：第一枪就失败，于是每一趟都会跑到终局并回调记账 ——
// 本层要证的是"结论会落到那一行"，不是它跑对了。
//
// ⚠ 时间全部显式传给 [Server.runScheduledTick]，不依赖真实时钟：循环那一条
// （ticker、开关位）单独由 [TestSchedulerRunningFlagTracksTheLoop] 管。

import (
	"context"
	"encoding/json"
	"errors"
	"maps"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// beijing 把东八区的墙上时间换成时刻。
//
// cron 一律按 [core.TZName]（东八区）判，而 `fixedNow` 是 UTC 的 06:00 ——
// 测试里直接写 UTC 的话，每个触发点都要心算一次 +8，那种用例改一次就得重算一遍。
func beijing(year int, month time.Month, day, hour, minute, second int) time.Time {
	return time.Date(year, month, day, hour, minute, second, 0, core.LocalTZ())
}

// ptrTime 给调度那三列可空的时刻取指针（模型里它们全是 `*time.Time`）。
func ptrTime(at time.Time) *time.Time { return &at }

// dailyNoon 是"每天东八区 14:00"那一条 cron。
//
// 选它是因为 `fixedNow`（UTC 06:00）正好是东八区的 14:00 整 —— 于是"准点触发"
// 那一条用例可以拿它当基准，再靠 `now` 加减秒数来造"迟到 30 秒"与"迟到几小时"。
const dailyNoon = "0 14 * * *"

// newSchedulerHarness 起一台已登录、带一个**能跑到终局**的账号的服务端。
func newSchedulerHarness(t *testing.T) (*harness, string) {
	t.Helper()
	h := newHarness(t).initialized(t)
	return h, seedLoginAccount(t, h, fullPhone)
}

// seedSchedule 直接落一行定时任务。
//
// 走存储层而不是 `PUT /api/schedules`：那个接口有自己的用例（`server_test.go`），
// 而这里要的是"库里**已经存在**的那一行长什么样"—— 包括 PUT 造不出来的那几种
// （无主行、没有 created_at 的行、目标是一个不存在的组合）。
func seedSchedule(t *testing.T, h *harness, schedule core.Schedule, scope core.Scope) {
	t.Helper()
	if schedule.ID == "" {
		schedule.ID = "job"
	}
	if schedule.Name == "" {
		schedule.Name = "夹具任务"
	}
	if schedule.Cron == "" {
		schedule.Cron = dailyNoon
	}
	if schedule.Timezone == "" {
		schedule.Timezone = core.TZName
	}
	schedule.Enabled = true
	// 存储层在**用户**范围下拒绝写无主行（也不能替别人建），所以归属跟着 scope 走；
	// scope 是 [core.ScopeLocal] 时保持 nil —— 那正是"无主行"这一种数据。
	if schedule.Owner == nil {
		schedule.Owner = scope.Owner()
	}
	if err := h.store.UpsertSchedule(context.Background(), scope, &schedule); err != nil {
		t.Fatalf("塞定时任务失败: %v", err)
	}
}

// scheduledAt 拼一行"目标 = 某个能力、上次处理到 watermark"的任务定义。
func capabilitySchedule(id, code string, watermark *time.Time) core.Schedule {
	schedule := core.NewSchedule(id, "夹具任务", dailyNoon, core.TZName)
	target := core.NewScheduleTargetCapability(code)
	schedule.Target = &target
	schedule.LastFireAt = watermark
	return schedule
}

// tickAt 走一轮调度（时钟不外露：`now` 直接给）。
func tickAt(t *testing.T, h *harness, now time.Time) (int, []string) {
	t.Helper()
	fired, problems, err := h.server.runScheduledTick(context.Background(), now)
	if err != nil {
		t.Fatalf("这一轮调度本身失败了: %v", err)
	}
	return fired, problems
}

// getSchedule 读回那一行。
func getSchedule(t *testing.T, h *harness, id string) core.Schedule {
	t.Helper()
	schedule, err := h.store.GetSchedule(
		context.Background(), core.ScopeUser(userIDOf(t, h)), id)
	if err != nil || schedule == nil {
		t.Fatalf("读不回定时任务 %s：%v", id, err)
	}
	return *schedule
}

// waitForScheduleRecorded 等**跑完之后**那一笔记账落库。
//
// 记账在 [launchRun] 那个后台 goroutine 的回调里，所以判"到点就触发"的用例
// 不能同步断言 —— 不等的话断言读到的还是旧值，而这个红法会让人去改判点逻辑。
//
// ⚠ 先等 `last_status` 变成**非 nil**再比对：一次 `ok` 的结论与"还没写"
// 在轮询里长得一模一样（零值就是 ok）。
func waitForScheduleRecorded(t *testing.T, h *harness, id string) core.Schedule {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		schedule := getSchedule(t, h, id)
		if schedule.LastStatus != nil {
			return schedule
		}
		if time.Now().After(deadline) {
			t.Fatalf("等不到调度把结论写回 %s（水位=%v 状态=%v 错误=%v）",
				id, schedule.LastFireAt, schedule.LastStatus, schedule.LastError)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// formatPtrTime 把可空水位打成一行人能读的文本（断言失败时 `*time.Time` 打印的是地址）。
func formatPtrTime(at *time.Time) string {
	if at == nil {
		return "nil"
	}
	return at.UTC().Format(time.RFC3339)
}

// assertWatermark 断言水位落在哪个时刻。
func assertWatermark(t *testing.T, id string, got *time.Time, want time.Time) {
	t.Helper()
	if got == nil {
		t.Fatalf("%s 的水位没写：期望 %s", id, want.UTC().Format(time.RFC3339))
	}
	if !got.Equal(want) {
		t.Errorf("%s 的水位应是 %s，实得 %s",
			id, want.UTC().Format(time.RFC3339), got.UTC().Format(time.RFC3339))
	}
}

// statusOf 读那一行的状态（nil 表示"还没人写过"）。
func statusOf(t *testing.T, schedule core.Schedule) core.ScheduleStatus {
	t.Helper()
	if schedule.LastStatus == nil {
		t.Fatalf("这一行没有执行状态（记账没落到库里）")
	}
	return *schedule.LastStatus
}

func errorText(schedule core.Schedule) string {
	if schedule.LastError == nil {
		return ""
	}
	return *schedule.LastError
}

// ─────────────────────────────────────────────────────────────────────────────
// 判点：到没到、迟到算不算错过
// ─────────────────────────────────────────────────────────────────────────────

// TestSchedulerFiresOnTimeAndWatermarkLandsOnThePoint：迟到 30 秒仍算"准点"。
//
// 水位要落在**那个触发点**而不是 now —— 这是 [core.Schedule.LastFireAt] 那句
// "已处理过的触发时刻"的字面含义，也是下一轮判点的起点。
func TestSchedulerFiresOnTimeAndWatermarkLandsOnThePoint(t *testing.T) {
	h, _ := newSchedulerHarness(t)
	point := beijing(2026, time.September, 20, 14, 0, 0) // == fixedNow
	seedSchedule(t, h, capabilitySchedule("noon", "sign", ptrTime(point.AddDate(0, 0, -1))),
		core.ScopeUser(userIDOf(t, h)))

	fired, _ := tickAt(t, h, point.Add(30*time.Second))
	if fired != 1 {
		t.Fatalf("准点到点应触发 1 条，实得 %d", fired)
	}
	schedule := waitForScheduleRecorded(t, h, "noon")
	assertWatermark(t, "noon", schedule.LastFireAt, point)
	// 这一趟的链按夹具设计**第一枪就失败** ⇒ 结论是 error，而不是 ok。
	if status := statusOf(t, schedule); status != core.ScheduleError {
		t.Errorf("链失败应记 error，实得 %s", status)
	}
	// ⚠ `last_run_at` 写的是**触发那一刻**，不是跑完那一刻：界面上那句
	// "上次执行"回答的是"我设的点它去没去"。
	if schedule.LastRunAt == nil || !schedule.LastRunAt.Equal(point.Add(30*time.Second)) {
		t.Errorf("last_run_at 应是触发时刻，实得 %v", schedule.LastRunAt)
	}
}

// TestSchedulerLeavesAFuturePointAlone：还没到点什么都不写。
//
// 这条挡的是"把水位当 last_run_at 用"那一类改法：判点错了的表现是
// 界面上每次刷新都在跑，而库里那一行被反复改写。
func TestSchedulerLeavesAFuturePointAlone(t *testing.T) {
	h, _ := newSchedulerHarness(t)
	point := beijing(2026, time.September, 20, 14, 0, 0)
	seedSchedule(t, h, capabilitySchedule("noon", "sign", ptrTime(point)),
		core.ScopeUser(userIDOf(t, h)))

	fired, problems := tickAt(t, h, point.Add(time.Hour))
	if fired != 0 || len(problems) != 0 {
		t.Fatalf("下一个点在两小时后，不该触发也不该报问题：%d / %v", fired, problems)
	}
	schedule := getSchedule(t, h, "noon")
	assertWatermark(t, "noon", schedule.LastFireAt, point) // 没动
	if schedule.LastStatus != nil {
		t.Errorf("没跑就不该有执行状态，实得 %s", statusOf(t, schedule))
	}
}

// TestSchedulerWithoutWatermarkDoesNotInventHistory：两列水位都没有 ⇒ 不补跑。
//
// 表里凭空多一行不认识的历史（导入的坏数据），不足以授权循环去打一枪真上游 ——
// 签到与换凭据都是不可逆动作。
func TestSchedulerWithoutWatermarkDoesNotInventHistory(t *testing.T) {
	h, _ := newSchedulerHarness(t)
	schedule := capabilitySchedule("ghost", "sign", nil)
	schedule.Cron = "0 3 * * *" // 东八区凌晨三点，与下面的 now 隔了 11 小时
	schedule.CreatedAt = nil    // 两列都空
	seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

	// now 是东八区 14:00，而今天的 03:00 那个点早过了：不补。
	fired, problems := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0))
	if fired != 0 {
		t.Fatalf("没有水位的行不该补跑，实得 %d（%v）", fired, problems)
	}
	if got := getSchedule(t, h, "ghost").LastFireAt; got != nil {
		t.Errorf("没跑就不该写水位，实得 %v", got)
	}
}

// TestSchedulerWithoutWatermarkStillFiresTheNextPoint：但下一个点照跑。
//
// 上一条只挡"补历史"，不能把这一行判成死行：起算点是 `now - 一个宽限窗口`，
// 所以刚过去的那一个点仍然抓得到（这里取迟到 29 秒，正好在宽限之内）。
func TestSchedulerWithoutWatermarkStillFiresTheNextPoint(t *testing.T) {
	h, _ := newSchedulerHarness(t)
	schedule := core.NewSchedule("early", "夹具任务", "0 3 * * *", core.TZName)
	target := core.NewScheduleTargetCapability("sign")
	schedule.Target = &target
	schedule.CreatedAt = nil
	seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

	fired, _ := tickAt(t, h, beijing(2026, time.September, 20, 3, 0, 29))
	if fired != 1 {
		t.Fatalf("刚过去的触发点该被抓到，实得 %d", fired)
	}
	assertWatermark(t, "early", waitForScheduleRecorded(t, h, "early").LastFireAt,
		beijing(2026, time.September, 20, 3, 0, 0))
}

// ─────────────────────────────────────────────────────────────────────────────
// 错过之后：补一次（日型）/ 弃掉（准点型）/ 那一行自己说了算
// ─────────────────────────────────────────────────────────────────────────────

// TestSchedulerCatchesUpExactlyOnceForADailyTask：停机两天的日型任务只补**一次**。
//
// 两个判据一起看才成立：
//   - 触发了（[core.MissCatchUp] 那一档）；
//   - 水位抬到 `now` 而不是那个错过的点 —— 这就是"错过的整串一次弃掉"，
//     停八小时不会变成"重启后连打八趟上游"。
//
// 反向改坏它的方式：把水位写成 `point` ⇒ 第二轮还会再捡一个点 ⇒ 本用例红。
func TestSchedulerCatchesUpExactlyOnceForADailyTask(t *testing.T) {
	h, accountID := newSchedulerHarness(t)
	missed := beijing(2026, time.September, 18, 14, 0, 0) // 两天前那个点
	seedSchedule(t, h, capabilitySchedule("noon", "sign", ptrTime(
		missed.AddDate(0, 0, -1))), core.ScopeUser(userIDOf(t, h)))

	now := beijing(2026, time.September, 20, 14, 0, 0)
	if fired, _ := tickAt(t, h, now); fired != 1 {
		t.Fatalf("日型任务错过该补一次，实得 %d", fired)
	}
	schedule := waitForScheduleRecorded(t, h, "noon")
	assertWatermark(t, "noon", schedule.LastFireAt, now)

	// 再走一轮（同一个 now 之后 30 秒）：不能又补一次。
	if fired, _ := tickAt(t, h, now.Add(schedulerTick)); fired != 0 {
		t.Fatalf("补过一次的这一轮不该再补，实得 %d", fired)
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 1 {
		t.Errorf("两轮调度应只留一条运行记录，实得 %d 条：%v", len(ids), ids)
	}
}

// TestSchedulerAbandonsAMissedOnTimeTask：准点型（直播口令）错过了就弃掉。
//
// 它绑的是**那个时刻本身** —— 主播那一场已经结束，补跑只是白打上游。
// 弃掉也要留痕：水位抬到 `now`、状态记 `skipped`、并说一句为什么，
// 而 `last_run_at` **不动**（这一趟什么都没跑，抹掉上一次真跑的时刻
// 会让界面显示"这个任务从来不执行"）。
func TestSchedulerAbandonsAMissedOnTimeTask(t *testing.T) {
	h, accountID := newSchedulerHarness(t)
	missed := beijing(2026, time.September, 18, 14, 0, 0)
	schedule := capabilitySchedule("live", "live-room", ptrTime(missed))
	schedule.Body = json.RawMessage(`{"codes":"口令甲"}`)
	scope := core.ScopeUser(userIDOf(t, h))
	seedSchedule(t, h, schedule, scope)
	// 先留一笔"上一次真跑过"的记账，用来验它**没被**这次弃掉的错过抹掉。
	ranEarlier := beijing(2026, time.September, 10, 14, 0, 0)
	if _, err := h.store.RecordScheduleRun(context.Background(), scope, "live",
		ptrTime(ranEarlier), core.ScheduleOK, nil); err != nil {
		t.Fatalf("预置记账失败: %v", err)
	}

	fired, problems := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0))
	if fired != 0 {
		t.Fatalf("准点型错过了不该补跑，实得 %d（%v）", fired, problems)
	}
	got := getSchedule(t, h, "live")
	assertWatermark(t, "live", got.LastFireAt, beijing(2026, time.September, 20, 14, 0, 0))
	if status := statusOf(t, got); status != core.ScheduleSkipped {
		t.Errorf("弃掉的错过应记 skipped，实得 %s", status)
	}
	if !strings.Contains(errorText(got), "不补跑") {
		t.Errorf("界面上那句话该说清为什么不跑，实得 %q", errorText(got))
	}
	if got.LastRunAt == nil || !got.LastRunAt.Equal(ranEarlier) {
		t.Errorf("last_run_at 应保持上一次真跑的时刻，实得 %v", got.LastRunAt)
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
		t.Errorf("弃掉的这一次不该留下运行记录：%v", ids)
	}
}

// TestScheduleRowOverridesTheCapabilityDefault：那一行显式选过的，比能力声明大。
//
// ⚠ 顺序判据在这里：同一份 cron、同一个错过的点，`live-room` 默认不补，
// 而这一行勾了"补跑" ⇒ 必须跑。反过来的话，用户那句"我要它补"会被能力表盖掉，
// 表现是"界面上勾了没用"。
func TestScheduleRowOverridesTheCapabilityDefault(t *testing.T) {
	h, _ := newSchedulerHarness(t)
	missed := beijing(2026, time.September, 18, 14, 0, 0)
	schedule := capabilitySchedule("live", "live-room", ptrTime(missed))
	schedule.Body = json.RawMessage(`{"codes":"口令甲"}`)
	policy := core.MissCatchUp
	schedule.MissPolicy = &policy
	seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

	fired, problems := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0))
	if fired != 1 {
		t.Fatalf("这一行勾了补跑就该补，实得 %d（%v）", fired, problems)
	}
	waitForScheduleRecorded(t, h, "live")
}

// ─────────────────────────────────────────────────────────────────────────────
// 不归本轮管 / 到点也跑不了的行
// ─────────────────────────────────────────────────────────────────────────────

// TestSchedulerIgnoresDisabledAndUnownedRows：停用的与无主的一律不动。
//
// 无主那一行挡的是**越权**：没有 owner 就没有 scope，也就无法回答
// "这一趟看得见哪些账号"。那种行的正确处置是 `mcloud user init` 去认领。
func TestSchedulerIgnoresDisabledAndUnownedRows(t *testing.T) {
	h, accountID := newSchedulerHarness(t)
	missed := beijing(2026, time.September, 19, 14, 0, 0)
	scope := core.ScopeUser(userIDOf(t, h))

	off := capabilitySchedule("off", "sign", ptrTime(missed))
	off.Enabled = false
	// [seedSchedule] 会替用例强制启用（漏填 Enabled 是常态），这一条要显式关掉。
	off.Owner = scope.Owner() // 用户范围下写不进无主行，归属得跟着 scope
	if err := h.store.UpsertSchedule(context.Background(), scope, &off); err != nil {
		t.Fatalf("塞停用任务失败: %v", err)
	}

	// 无主行只能用本地范围写进去（用户范围下"把任务写成无主"是被存储层挡住的）。
	ghost := capabilitySchedule("ghost", "sign", ptrTime(missed))
	ghost.Owner = nil
	seedSchedule(t, h, ghost, core.ScopeLocal())

	fired, problems := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0))
	if fired != 0 {
		t.Fatalf("停用与无主的行都不该跑，实得 %d（%v）", fired, problems)
	}
	if got := getSchedule(t, h, "off").LastFireAt; got == nil || !got.Equal(missed) {
		t.Errorf("停用的行水位该原地不动（仍是预置的 %s），实得 %v",
			missed.UTC().Format(time.RFC3339), formatPtrTime(got))
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
		t.Errorf("这两行都不该留下运行记录：%v", ids)
	}
}

// TestSchedulerRecordsWhyADueRowCouldNotRun：到点了但跑不了的三种情形。
//
// 三条走的是同一段代码（[abandon]），所以共用一条用例：都记 `error`、都抬水位。
// 抬水位这一条是**故意**的：坏数据每个触发周期只报一次，而不是每个 tick 刷一遍，
// 否则界面上那句"上次执行"就不是任何一个真实时刻。
//
// ⚠ 每条自带 `at` 与预置水位：`exchange` 默认不补跑，拿"错过的点"喂它只会走到
// "按策略弃掉"那一条分支（记 `skipped`），永远到不了"能力还没接上执行"。所以那一条
// 必须是**准点迟到**的 —— 顺序判据在这里：先问"这个点还要不要跑"，再问"跑得起来吗"。
//
// ⚠ "能力还没接上"那一格靠**临时摘掉分派表里的一行**来造（`unwire`）：
// 2026-09-23 之前可以直接用 `exchange`（那时它是唯一没接的），接上之后
// 真实配置下这一支已经不可达 —— 但它是 [launchScheduled] 里那条顺序判据唯一的判据，
// 删掉用例等于把"先查 builder、后查参数"这个顺序放成无判据状态。
func TestSchedulerRecordsWhyADueRowCouldNotRun(t *testing.T) {
	// 两个"现在"：`atLate` 距离那个点已经一天（算错过），`atOnTime` 只迟到 30 秒
	// （在 [schedulerGrace] 之内，不算错过）。
	atLate := beijing(2026, time.September, 20, 14, 0, 0)
	atOnTime := beijing(2026, time.September, 20, 14, 0, 30)
	cases := []struct {
		name string
		at   time.Time
		// 预置的水位：与 `at` 一起决定"到点没有"和"算不算错过"。
		watermark time.Time
		// schedule 已经判到点，只差"跑不了"那一半原因。
		schedule func(watermark time.Time) core.Schedule
		want     string
		// 起跑前从 [runBuilders] 里临时摘掉的 code，用例结束放回（本包无 `t.Parallel()`，
		// 且这一条走到 `abandon` 就返回、不会起 goroutine，所以摘表没有并发风险）。
		unwire string
	}{
		{
			name:      "组合还没接上执行",
			at:        atLate,
			watermark: missedNoon(),
			schedule:  pipelineSchedule,
			want:      "任务组合还没接上执行",
		},
		{
			name:      "能力登记了但没接上",
			at:        atOnTime,
			watermark: beijing(2026, time.September, 19, 14, 0, 0),
			schedule: func(watermark time.Time) core.Schedule {
				return capabilitySchedule("job", "exchange", ptrTime(watermark))
			},
			want:   "还没有接上执行",
			unwire: "exchange",
		},
		{
			name:      "参数里有声明不认识的键",
			at:        atLate,
			watermark: missedNoon(),
			schedule:  badKeySchedule,
			want:      "不认识的参数",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			h, accountID := newSchedulerHarness(t)
			if tc.unwire != "" {
				builder, wired := runBuilders[tc.unwire]
				if !wired {
					t.Fatalf("这条用例的前提是 %q 已经接上（否则不需要摘表），实得分派表 %v",
						tc.unwire, slices.Sorted(maps.Keys(runBuilders)))
				}
				delete(runBuilders, tc.unwire)
				defer func() {
					runBuilders[tc.unwire] = builder
					if _, ok := runBuilders[tc.unwire]; !ok {
						t.Errorf("临时摘掉的 %q 没放回去", tc.unwire)
					}
				}()
			}
			schedule := tc.schedule(tc.watermark)
			schedule.ID = "job"
			seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

			fired, _ := tickAt(t, h, tc.at)
			if fired != 0 {
				t.Fatalf("这一条不该起跑，实得 %d", fired)
			}
			got := getSchedule(t, h, "job")
			if status := statusOf(t, got); status != core.ScheduleError {
				t.Errorf("跑不了应记 error，实得 %s", status)
			}
			if !strings.Contains(errorText(got), tc.want) {
				t.Errorf("那句话里该有 %q，实得 %q", tc.want, errorText(got))
			}
			assertWatermark(t, "job", got.LastFireAt, tc.at)
			if got.LastRunAt != nil {
				t.Errorf("没起跑就不该写 last_run_at，实得 %v", got.LastRunAt)
			}
			if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
				t.Errorf("跑不了却留下了运行记录：%v", ids)
			}
		})
	}
}

// missedNoon 是"两天前那个已经过去的点"（配合 [dailyNoon] 用）。
func missedNoon() time.Time {
	return beijing(2026, time.September, 18, 14, 0, 0)
}

func pipelineSchedule(watermark time.Time) core.Schedule {
	schedule := core.NewSchedule("job", "夹具任务", dailyNoon, core.TZName)
	target := core.NewScheduleTargetPipeline("does-not-exist")
	schedule.Target = &target
	schedule.LastFireAt = ptrTime(watermark)
	return schedule
}

func badKeySchedule(watermark time.Time) core.Schedule {
	schedule := capabilitySchedule("job", "sign", ptrTime(watermark))
	schedule.Body = json.RawMessage(`{"receive":true}`) // `sign` 没有这一栏
	return schedule
}

// ─────────────────────────────────────────────────────────────────────────────
// 目标账号与并发
// ─────────────────────────────────────────────────────────────────────────────

// TestScheduleAccountColumnWinsOverTheBody：库里那一列才是"跑谁"的权威。
//
// ⚠ 这条挡的是越权路径：`account_ids` 是 `PUT` 逐个验过归属的那一列，
// 而 `body` 是透传参数。让 body 里的 `accounts` 赢，等于"界面上选了 A、
// 实际跑的是 body 里那个"，而它可能指向别人的账号（那一道归属校验就白做了）。
func TestScheduleAccountColumnWinsOverTheBody(t *testing.T) {
	h, pickedID := newSchedulerHarness(t)
	otherID := seedLoginAccount(t, h, "19900000001")

	schedule := capabilitySchedule("noon", "sign", ptrTime(
		beijing(2026, time.September, 19, 14, 0, 0)))
	schedule.AccountIDs = []core.AccountID{core.AccountIDFor(fullPhone, nil)}
	schedule.Body = json.RawMessage(`{"accounts":["` + otherID + `"]}`)
	seedSchedule(t, h, schedule, core.ScopeUser(userIDOf(t, h)))

	if fired, _ := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0)); fired != 1 {
		t.Fatalf("到点了该跑一条，实得 %d", fired)
	}
	waitForScheduleRecorded(t, h, "noon")
	ran := runAccountsOf(t, h)
	if !slices.Equal(ran, []string{pickedID}) {
		t.Errorf("只该跑界面上选中的那个账号，实得 %v（选中 %s / body 里 %s）",
			ran, pickedID, otherID)
	}
}

// TestSchedulerBlockedAccountsRecordSkipped：同一账号正被另一趟占着 ⇒ skipped。
//
// 这就是旧实现 `protect:true` 的那一条：**不排队**，跳过了事。
// 而 `last_run_at` 不写（这一趟没跑成任何账号）。
func TestSchedulerBlockedAccountsRecordSkipped(t *testing.T) {
	h, accountID := newSchedulerHarness(t)
	scope := core.ScopeUser(userIDOf(t, h))
	seedSchedule(t, h, capabilitySchedule("noon", "sign", ptrTime(
		beijing(2026, time.September, 19, 14, 0, 0))), scope)
	ranEarlier := beijing(2026, time.September, 12, 14, 0, 0)
	if _, err := h.store.RecordScheduleRun(context.Background(), scope, "noon",
		ptrTime(ranEarlier), core.ScheduleOK, nil); err != nil {
		t.Fatalf("预置记账失败: %v", err)
	}

	// 占住这一账号的这一把锁：与手动那一路同时点"开始"是同一形态。
	holder := core.NewRunID(newRandom())
	key := mcloudexec.NewMutexKey("sign", accountID, signInDigest())
	if admission := h.server.runs.registry.Admit(holder, key); admission.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("占锁失败：%v", admission.Kind)
	}
	defer h.server.runs.registry.Release(holder)

	if fired, _ := tickAt(t, h, beijing(2026, time.September, 20, 14, 0, 0)); fired != 1 {
		t.Fatalf("起跑是受理了的（挡在准入那一步），应报 1，实得 %d", fired)
	}
	got := waitForScheduleRecorded(t, h, "noon")
	if status := statusOf(t, got); status != core.ScheduleSkipped {
		t.Errorf("全被挡下应记 skipped，实得 %s", status)
	}
	if got.LastRunAt == nil || !got.LastRunAt.Equal(ranEarlier) {
		t.Errorf("一个账号都没跑成，last_run_at 不该动，实得 %v", got.LastRunAt)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 记账口径（纯映射，不碰库）
// ─────────────────────────────────────────────────────────────────────────────

// TestScheduledOutcomeMappingIsTheDocumentedTable：四态 + 两种特殊情况。
//
// ⚠ "部分成功也记 error"是**沿用旧口径**：定时任务没有人当场看着，
// 把"一半没成"记成 ok 等于把它藏起来。改这一格要先想清楚界面上谁来接这句话。
func TestScheduledOutcomeMappingIsTheDocumentedTable(t *testing.T) {
	cases := []struct {
		name     string
		summary  app.RunSummary
		err      error
		want     core.ScheduleStatus
		wantText string
	}{
		{"全部成功", summaryOf(core.OutcomeSuccess, 2, 0), nil, core.ScheduleOK, ""},
		// 三档非 ok 的都必须带一句话：`last_status = error` 配一个空的
		// `last_error`，在界面上就是"失败了但不知道为什么"。
		{"部分成功", summaryOf(core.OutcomePartialSuccess, 2, 0), nil, core.ScheduleError, "部分成功"},
		{"要人操作", summaryOf(core.OutcomeManualRequired, 1, 0), nil, core.ScheduleError, "需要你去客户端操作"},
		{"彻底失败", summaryOf(core.OutcomeFailed, 1, 0), nil, core.ScheduleError, "失败"},
		{
			"编排自身报错", summaryOf(core.OutcomeSuccess, 0, 0),
			errors.New("写库失败"), core.ScheduleError, "写库失败",
		},
		{"全被挡下", summaryOf(core.OutcomeSuccess, 0, 2), nil, core.ScheduleSkipped, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			status, text := scheduledOutcome(tc.summary, tc.err)
			if status != tc.want {
				t.Errorf("状态应是 %s，实得 %s", tc.want, status)
			}
			if tc.wantText != "" && !strings.Contains(text, tc.wantText) {
				t.Errorf("那句话该含 %q，实得 %q", tc.wantText, text)
			}
		})
	}
}

// summaryOf 拼一份编排真正会给出的那几种 [app.RunSummary] 形状。
//
// "被挡下"那一格刻意**不**带 Outcome 之外的信息：`Skipped()` 数的就是
// `Kind == AccountRunSkipped` 的条目，而结论合成只看真跑了的那些。
func summaryOf(outcome core.RunOutcome, ran, skipped int) app.RunSummary {
	entries := make([]app.AccountRun, 0, ran+skipped)
	for range ran {
		entries = append(entries, app.AccountRun{Kind: app.AccountRunRan})
	}
	for range skipped {
		entries = append(entries, app.AccountRun{Kind: app.AccountRunSkipped})
	}
	return app.RunSummary{Capability: "sign", Outcome: outcome, Accounts: entries}
}

// ─────────────────────────────────────────────────────────────────────────────
// 那一条循环本身
// ─────────────────────────────────────────────────────────────────────────────

// TestSchedulerRunningFlagTracksTheLoop：`schedulerRunning` 说的是真话。
//
// 这个开关位是 [GET /api/status] 那个字段的唯一来源。侧栏底部那句"调度在跑"
// 之前恒为 false（后端确实没有循环），现在有了循环，它就**必须**跟着起停。
func TestSchedulerRunningFlagTracksTheLoop(t *testing.T) {
	h := newHarness(t).initialized(t)
	if h.server.SchedulerRunning() {
		t.Fatal("还没起循环就报在跑")
	}
	ctx, cancel := context.WithCancel(context.Background())
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		h.server.startScheduler(ctx, func(string) {})
	}()
	if !waitSchedulerFlag(t, h, true) {
		t.Fatal("起了循环之后 schedulerRunning 应变真（等 5 秒没等到）")
	}
	cancel()
	select {
	case <-stopped:
	case <-time.After(5 * time.Second):
		t.Fatal("ctx 结束了循环还没退")
	}
	if h.server.SchedulerRunning() {
		t.Error("循环退了还报在跑")
	}
}

func waitSchedulerFlag(t *testing.T, h *harness, want bool) bool {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if h.server.SchedulerRunning() == want {
			return true
		}
		time.Sleep(2 * time.Millisecond)
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// runIDsForAccount 列出某个账号名下的运行记录 id。
func runIDsForAccount(t *testing.T, h *harness, accountID string) []string {
	t.Helper()
	runs, err := h.store.ListRuns(context.Background(), core.ScopeUser(userIDOf(t, h)), 200)
	if err != nil {
		t.Fatalf("列 run 失败: %v", err)
	}
	out := make([]string, 0, len(runs))
	for _, run := range runs {
		if run.AccountID != nil && run.AccountID.String() == accountID {
			out = append(out, run.ID)
		}
	}
	return out
}

// runAccountsOf 列出所有运行记录的账号（按 id 排序，便于比对）。
func runAccountsOf(t *testing.T, h *harness) []string {
	t.Helper()
	runs, err := h.store.ListRuns(context.Background(), core.ScopeUser(userIDOf(t, h)), 200)
	if err != nil {
		t.Fatalf("列 run 失败: %v", err)
	}
	out := make([]string, 0, len(runs))
	for _, run := range runs {
		if run.AccountID != nil {
			out = append(out, run.AccountID.String())
		}
	}
	slices.Sort(out)
	return out
}
