package app

// 活体探针（**只读**）：今天这张任务表里，玩法 **1014** 到底在不在"待做"那一档。
//
// # 为什么先问这一格，而不是直接跑整趟
//
// 第廿二趟收在"新链路（`collectMailPDFProduct` + 链尾那一发覆盖两枚的 `batchTrash`）只有
// 单测与探针背书，没走过生产入口"。而 app-task 的**生产入口**是 [TaskMatrixRun.RunAccount]
// （WebUI 那条 `POST /api/runs` 最终就是调它，见 `internal/server/run_dispatch_signin.go`），
// 它的形状是"拉表 → 只把 [TaskMatrix.Pending] 交给分派"⇒ **一条玩法只有当天在表里且读成待做时
// 才会被跑到**。本仓已经在这一格上撞过两次（`live_probe_test.go` 文件头）：107 与 472/473
// 都有过"当天不在表里 ⇒ 一枪没发"的现场。
//
// 所以带写那一发之前先把这一问问清楚：
//   - 1014 在待做里 ⇒ 下一发跑整趟 [RunAccount]（生产入口，念出事件流那几行）；
//   - 1014 在表里但读成非待做 ⇒ 整趟跑到它那里**不会动手**，跑了也是空读数；
//   - 1014 压根不在表里 ⇒ 同上，且这一格今天补不了（要等明天表变）。
//
// 顺带把**整张 runner 表**在这一号上的实况念一遍：哪些玩法今天可跑、哪些不在表里。
// 这一份读数本身就是判据（R36／R37 那一族"接了但今天跑不到"的档位，界面上没人看得出来）。
//
// # 这一发**不打任何写枪**
//
// 它只走两口读请求：`taskListV2`（[AppTaskMatrix]，16 次）与 `infoV3`（[taskExecutor.backupGate]，
// 只为把 `month` 那一组的门算准 —— 它是分派的输入，不读它就会把"该跑的"念成"不该跑"）。
// 分派本身用的是 [PlanTaskStep]，那是**纯函数**（不发请求）。
// ⇒ 因此它不受"带写那一发只跑一次"的约束（`docs/capture-guidelines.md` 的停止条件只管写枪），
// 可以两号各跑、也可以当天复跑。
//
// 边界：任务名与组名是**上游自己的字符串** ⇒ 可以落；账号只念下标与 id 长度；
// 凭据一律进 [liveProbeSink.see] 登记，`Authorization` 与手机号都不例外。

import (
	"context"
	"fmt"
	"sort"
	"testing"
)

// TestLiveProbeAppTaskTablePeek 拉一次表，逐条念"今天会怎么处置它"，并单独回答 1014 那一问。
func TestLiveProbeAppTaskTablePeek(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())
	matrix, perr := AppTaskMatrix(ctx, opening.Source)
	if perr != nil {
		t.Fatalf("拉表失败（16 次里全挂才算整体失败）：%s", sink.mask(perr.Error()))
	}
	pending := matrix.Pending()
	sink.say("拉表：%d 枪 / 去重前 %d 条 / 去重后 %d 条 / 待做 %d 条",
		matrix.Fetches, matrix.Raw, len(matrix.Items), len(pending))
	if line := matrix.MarketSummaryLine(); line != "" {
		sink.say("按市场名：%s", line)
	}
	for _, line := range matrix.GroupReadingLines("本次只读拉表") {
		sink.say("%s", line)
	}
	if len(matrix.Failures) > 0 {
		// 这一句必须念：`待做 8 条`读起来像全表，而它可能只是 13/16 组。
		sink.say("⚠ 有 %d/%d 组没读到 ⇒ 上面那些条数**不是全表**", len(matrix.Failures), matrix.Fetches)
	}

	// 分派要的外部事实（`month` 那一组的门 + 今天是几号）。只读：一发 `infoV3`。
	executor := newTaskExecutor(run, opening, account)
	gate := executor.backupGate(ctx)
	sink.say("分派门：本月自动备份 known=%v（读不到时门**不挡**，与生产同口径）", gate.BackupKnown)

	// ── 逐条待做：念"生产入口会怎么处置它"（纯函数，不发枪）──
	for _, item := range pending {
		plan := PlanTaskStep(item, gate)
		sink.say("待做 #%d [%s] %s ⇒ 处置=%s runner=%q 先打卡=%v 要传网盘=%v%s%s",
			derefTaskID(item), taskGroupOf(item), taskNameOf(item),
			plan.Kind, plan.Runner, plan.ClickKey != "", plan.Upload != nil,
			liveProbePlanSuffix("原因", plan.Reason), liveProbePlanSuffix("缺口", plan.ToolGap))
	}

	// ── 那一问：1014 在整张表里的哪一档（不在表里 / 在但非待做 / 在且待做）──
	sink.say("=== 1014 那一问 ===")
	id := int64(1014)
	inTable := false
	for _, item := range matrix.Items {
		if derefTaskID(item) != id {
			continue
		}
		inTable = true
		isPending := false
		for _, p := range pending {
			if derefTaskID(p) == id {
				isPending = true
			}
		}
		plan := PlanTaskStep(item, gate)
		sink.say("表里有这一条：%s ⇒ 读成待做=%v，处置=%s runner=%q，`state`=%s `currstep`=%s `process`=%s `enable`=%s",
			taskNameOf(item), isPending, plan.Kind, plan.Runner,
			liveProbeFieldWord(item.State), liveProbeFieldWord(item.CurrStep),
			liveProbeFieldWord(item.Process), liveProbeFieldWord(item.Enable))
		if isPending {
			sink.say("⇒ **今天可以走生产入口**：跑 [RunAccount] 会动到这一条（下一发是带写的整趟）")
		} else {
			sink.say("⇒ 今天**不会**动它：整趟 [RunAccount] 只吃待做那一档，跑了也是空读数")
		}
	}
	if !inTable {
		sink.say("表里**没有** 1014 这一条 ⇒ 整趟 [RunAccount] 不会碰它；这一格今天补不了（等表变）")
	}

	// ── 顺带：runner 表里每一条玩法，今天在这一号上是什么档 ──
	sink.say("=== runner 表逐条在这一号上的实况 ===")
	for _, rid := range liveProbeRunnerIDs() {
		status := "不在表里"
		for _, item := range matrix.Items {
			if derefTaskID(item) != rid {
				continue
			}
			status = "在表里·非待做"
			for _, p := range pending {
				if derefTaskID(p) == rid {
					status = "在表里·待做"
				}
			}
		}
		spec := taskMatrixRunners[rid]
		sink.say("#%d %-22s ⇒ %s%s", rid, spec.Name, status,
			liveProbeWriteWord(spec))
	}
	sink.say("读数怎么用：只有读成「在表里·待做」的那些玩法，跑整趟 [RunAccount] 才会真的动它们；" +
		"1014 那一行决定第廿三趟走哪条路（走整趟 / 今天补不了要登记）。")
}

// liveProbeRunnerIDs 交回 runner 表里的任务 id（升序，保证读数可比对）。
func liveProbeRunnerIDs() []int64 {
	ids := make([]int64, 0, len(taskMatrixRunners))
	for id := range taskMatrixRunners {
		ids = append(ids, id)
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return ids
}

// liveProbePlanSuffix 把一句"原因 / 缺口"接成尾巴（空串时整段不留，免得读数里出现空括号）。
func liveProbePlanSuffix(label, text string) string {
	if text == "" {
		return ""
	}
	return fmt.Sprintf(" %s=%q", label, text)
}

// liveProbeWriteWord 标出这条玩法**会不会写网盘**（跑整趟前得知道这一发要往用户网盘写几份）。
func liveProbeWriteWord(spec taskRunnerSpec) string {
	switch {
	case spec.MailPDF:
		return "（**写**：传一份 PDF + ⑥ 另存一份产物）"
	case spec.AICamera:
		return "（**写**：① 识图会写网盘）"
	case spec.Upload != nil:
		return "（**写**：带上传那一半）"
	case spec.Notes:
		return "（写：建一条笔记再移进回收站）"
	case spec.MailBackup:
		return "（写：⑧ 备份那一发）"
	}
	return ""
}

// liveProbeFieldWord 念一条任务里那一格的**面值**（`state` / `currstep` / `process` / `enable`
// 都是上游自己的业务读数，不是凭据 ⇒ 可以落）。nil 念成"缺"，不念成 0 ——
// 那一格在 R25 / R15 里是"上游没给"与"给了 0"的分别，混了就读不出来了。
func liveProbeFieldWord[T any](value *T) string {
	if value == nil {
		return "缺"
	}
	return fmt.Sprintf("%v", *value)
}
