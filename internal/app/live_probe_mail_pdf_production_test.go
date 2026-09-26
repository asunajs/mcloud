package app

// 活体探针（**带写**）：让 1014 那条新清理链走一遍**生产代码的入口**，念出事件流那几行。
//
// # 这一发补的是哪一格
//
// 第廿二趟把 `collectMailPDFProduct`（⑥ 之后按 `var.directoryId` 找回产物）接进了
// [taskExecutor.runMailPDF]，并让产物与素材**同一发** `batchTrash`。当时留下的边界是：
// "新链路只有单测与夹具背书，没走过生产入口"。生产入口 = [TaskMatrixRun.RunAccount]
// （WebUI 的 `POST /api/runs` 最终就调它），而它的形状是"拉表 → 只把待做那档交给
// [taskExecutor.apply]"⇒ 一条玩法**当天不在待做里就跑到不了它**。
//
// ⚠ 同日那发只读探针（`live_probe_app_task_table_test.go`）念出来的是：两枚测试号里
// 1014 **都在表中、但 `state` 读成 `FINISH`**（`currstep=3`）⇒ 今天跑整趟 [RunAccount]
// 也**不会**动它，跑了只是一串空读数。所以这一发取的是"生产路径的**上一条**"：
//
//	真表里那一枚 1014 → 只把 `state` 改成 `WAIT` → [PlanTaskStep]（纯函数）
//	→ [taskExecutor.apply]（生产那一处：分派 → [runRunner] → `spec.MailPDF` → [runMailPDF]）
//	→ [TaskMatrixRun.cleanupCloudFiles]（生产末尾那一句：一发 `batchTrash` 覆盖整份账）
//	→ [TaskMatrixRunResult.SummaryLine]（小结那句）
//
// 也就是**除"拉表 + 复查"以外**的整条生产链。播报走的是 [SignInWorld.Progress] 那一路
// （探针把它接到 [liveProbeSink.progress]）⇒ 这里念出的每一行，与界面上事件流里的每一行
// **是同一句文本**，不是另写的。
//
// # 这一发**只改一格**：`state`
//
// 名字、组、`currstep`、`enable`、`marketname` 全部沿用上游这一趟真回的那一枚条目。
// 为什么这点差别要说清：`currstep=3` 决定分派**不发前置那一发打卡**（旧实现的门），
// 所以这一发不会把一条已完成的任务又打一次卡（那是不可回滚的写）；它只发
// "传素材 + ⑥ 转换 + 链尾清理"这三件本来就要验的事。改的那一格在输出里**念出前后两值**。
//
// # 边界
//
//   - 四个 env 齐 + 确认串逐字对（同其它带写探针）；表里没有 1014 ⇒ **一枪都不发**，直接停。
//   - 真实 `fileId` / 票 / 手机号一律只进 [liveProbeSink.see] 与掩码表；文件名只有
//     `asign-*`（我们自己造的那一族）落全名。
//   - 链尾之后**再等一档**才做独立回读（昨天的读数：`batchTrash` 之后当场那次回读
//     **还在**，约 56 秒后不再列出 ⇒ 当场回读会把"列目录延迟"读成"删不动"，见
//     `docs/capture/app-cloud-residue-samples.txt` §4）。等的是**读数时机**，不是重试。
//   - ⚠ **小结那一句也在读数里**，所以探针必须**照抄** [TaskMatrixRun.RunAccount] 末尾对
//     `result` 的两处填充（`Counts.absorb(record)` 与 `CloudProductFiles`）。第一发没抄 ⇒
//     小结念出 `另有 2 个占位未传完`，而那一趟一枚占位都没有：`Counts` 全零时
//     [TaskMatrixRunResult.SummaryLine] 的减法把**素材与产物都**算成"没传完的坑"。
//     那一条是**探针工件**不是生产缺陷（生产那两处一直在填），但它把生产的小结**念错了**，
//     而小结正是界面上那一行 ⇒ 登记成坑表 **#187**（"探针替生产填的那一格要逐字对齐"）。
//   - ⚠ 本探针**没有断言**（它是仪器不是回归测试）：判据落在 `docs/status.md` 与
//     `docs/pending-decisions.md`，写成断言会让上游改口时红一格既不是缺陷也不是判据。

import (
	"context"
	"path"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

// liveProbePDFProductWait 是链尾清理之后到独立回读之间的等待。下限按昨天那一发
// 实测到的"列目录延迟 ≈56 秒"取整并放宽一档；它**不**改变任何请求，只改变读的时间点。
const liveProbePDFProductWait = 90 * time.Second

// TestLiveProbeMailPDFProductionChain 走上面那条"生产 apply + 生产清理"的链，
// 并在链尾之后做一次**独立**只读回读（根目录 + 产物那个目录），念"这一趟留没留下东西"。
func TestLiveProbeMailPDFProductionChain(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	// ── 真表里的那一枚 1014（只读那一发：16 枪 taskListV2）──
	matrix, perr := AppTaskMatrix(ctx, opening.Source)
	if perr != nil {
		t.Fatalf("拉表失败：%s", sink.mask(perr.Error()))
	}
	real, found := liveProbeFindTaskItem(matrix.Items, 1014)
	if !found {
		sink.say("这一号的表里**没有** 1014（去重后 %d 条）⇒ 一枪不发，停。", len(matrix.Items))
		sink.say("⇒ 这一格今天补不了：要补得换一个表里有它的号，或等这张表变。")
		return
	}
	sink.say("真表那一枚：#%d %s [组 %s] state=%s currstep=%s enable=%s marketname=%s",
		derefTaskID(real), taskNameOf(real), taskGroupOf(real),
		stateWords(real), liveProbeFieldWord(real.CurrStep), enableWords(real), marketWords(real))

	item := real
	stateWait := upstream.TaskStateWait
	item.State = &stateWait
	sink.say("⚠ **本探针只改了 `state` 这一格**：%s → %s（其余字段逐字沿用上游那一份）",
		stateWords(real), stateWords(item))
	if !taskPending(item) {
		sink.say("改完仍读不成待做 ⇒ 与生产入口的口径不一致，停（不发写枪）")
		return
	}

	executor := newTaskExecutor(run, opening, account)
	gate := executor.backupGate(ctx)
	plan := PlanTaskStep(item, gate)
	sink.say("分派（纯函数，零枪）：处置=%s runner=%q 前置打卡=%v 要传网盘=%v 原因=%q 缺口=%q",
		plan.Kind, plan.Runner, plan.ClickKey != "", plan.Upload != nil, plan.Reason, plan.ToolGap)
	if plan.Kind != StepRunner || plan.Runner != taskMatrixRunners[1014].Name {
		sink.say("⚠ 分派**没有**把它交给 1014 那条 runner ⇒ 下面这一发仍走生产 apply，"+
			"但读数只到「这一条今天被处置成 %s」，不回答清理链", plan.Kind)
	}
	// 生产入口在动手**之前**先播的那一行（RunAccount 循环里同一句）。
	if line, ok := taskStepLiveLine(item, plan); ok {
		sink.say("生产播报[%d] %s", int(line.Level), line.Text)
	}

	// ── 前后快照（都只读）：根目录看素材、产物那个目录看⑥另存的那一枚 ──
	rootBefore, rootOK, _ := liveProbeListing("动手前", run, opening, sink)
	dir, dirOK := liveProbeProductFolder(run, opening, sink)
	dirBefore := liveProbeNameSet(run, opening, sink, "产物目录·动手前", dir, dirOK)

	// ── 生产那两处 ──
	started := time.Now()
	record, fired := executor.apply(ctx, item, plan)
	sink.say("apply 交回：发过枪=%v 处置=%s runner=%q 本次上传计数=%d",
		fired, record.Kind, plan.Runner, record.Uploads)
	if record.Reason != "" {
		sink.say("record.Reason：%s", sink.mask(record.Reason))
	}
	if record.Err != "" {
		sink.say("record.Err（进小结与界面的就是这句）：%s", sink.mask(record.Err))
	}
	result := TaskMatrixRunResult{
		Account:      account,
		Notes:        []string{},
		CloudFileIDs: append([]string{}, executor.cloudFileIDs...),
	}
	// ⭐ 这两行是**照抄 [TaskMatrixRun.RunAccount] 末尾那两处**（`Counts.absorb(record)` 与
	// `CloudProductFiles = exec.cloudProductFiles`）。第一版探针**没有**抄，于是小结念出
	// `另有 2 个占位未传完` —— 那一趟里一枚占位都没有，那一句是**探针自己造的**
	// （`Counts` 全零 ⇒ `SummaryLine` 的减法 `2-0-0` 把素材也算成了没传完的坑）。
	// 抄了之后这一发的小结与生产那一趟**同源**，它念什么界面上就念什么。
	result.Counts.absorb(record)
	result.CloudProductFiles = executor.cloudProductFiles
	sink.say("这一趟记账（网盘里我们造出来的枚数）：%d，其中上游另存的产物 %d 枚，"+
		"上传计数 %d —— 链尾那一发 batchTrash 的输入是那份账",
		len(result.CloudFileIDs), result.CloudProductFiles, result.Counts.Uploaded)
	// 生产末尾那一句（RunAccount 里同一处），播报由它自己念。
	run.cleanupCloudFiles(ctx, &executor, &result)
	sink.say("小结：%s", result.SummaryLine())
	sink.say("清理结局：%s 原因=%q", liveProbeCleanupWord(result), result.CleanupReason)

	// ── 独立回读：等过"列目录延迟"那一档再看差集 ──
	sink.say("等 %v 后做独立只读回读（昨天的读数：batchTrash 当场那次还在列，约 56 秒后不再列出）",
		liveProbePDFProductWait)
	time.Sleep(liveProbePDFProductWait)
	rootAfter, rootOK2, _ := liveProbeListing("清理后回读", run, opening, sink)
	liveProbeSayAdded("清理后 vs 动手前（根目录）", rootBefore, rootAfter, sink)
	dirAfter := liveProbeNameSet(run, opening, sink, "产物目录·清理后回读", dir, dirOK)
	liveProbeSayDiff("产物目录", dirBefore, dirAfter, sink)
	if !rootOK || !rootOK2 || !dirOK {
		sink.say("⚠ 上面三遍里有一遍没读出来 ⇒ 差集不成立，这一格仍空（**不等于**「没留下东西」）")
	}
	sink.say("耗时 %v ⇒ 三条读数怎么用：① 事件流里那一句是「产物已对上」还是「产物没对上」，"+
		"它就是生产播报的原文；② 记账枚数 = 2 才说明产物真的挂上了清理链；"+
		"③ 独立回读的两份差集**都空**，才算「这一趟在用户网盘里没留下东西」。",
		time.Since(started).Round(time.Second))
}

// liveProbeCleanupWord 念清理那一节的四档（与小结里 `cleanupPhrase` 同一分档，
// 这里只要一个能进读数的短词）。
func liveProbeCleanupWord(result TaskMatrixRunResult) string {
	switch result.Cleanup {
	case TaskCleanupNotRun:
		return "没有要清理的"
	case TaskCleanupDone:
		return "已移进回收站"
	case TaskCleanupKept:
		return "刻意保留"
	case TaskCleanupFailed:
		return "清理失败"
	}
	return "未知档"
}

// liveProbeFindTaskItem 按 id 在一份条目里找第一条。
func liveProbeFindTaskItem(items []upstream.TaskV2Item, id int64) (upstream.TaskV2Item, bool) {
	for _, item := range items {
		if derefTaskID(item) == id {
			return item, true
		}
	}
	return upstream.TaskV2Item{}, false
}

// liveProbeNameSet 列一个目录（或根目录 "/"）一页，交回**名字集合**与"这一遍读没读出来"。
//
// 读不出来时交回 nil，调用方据此念"这一格空白"—— 不拿空集冒充"目录里没有东西"
// （那是本仓在这一族上已经犯过一次的那类错，见坑表 #185）。
func liveProbeNameSet(
	run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink, what, parentFileID string, ok bool,
) map[string]bool {
	if !ok || parentFileID == "" {
		sink.say("%s：**没有可列的目录**（上面那行导航说了为什么）⇒ 这一格空白", what)
		return nil
	}
	items, listed := liveProbeRawListing(run, opening, sink, parentFileID)
	if !listed {
		sink.say("%s：那一页**没读出来** ⇒ 这一格空白", what)
		return nil
	}
	names := make(map[string]bool, len(items))
	for _, item := range items {
		names[item.name] = true
	}
	ours := 0
	for name := range names {
		if strings.HasPrefix(name, CloudUploadNamePrefix) {
			ours++
		}
	}
	sink.say("%s：%d 项（其中前缀 %q 的 %d 枚）", what, len(names), CloudUploadNamePrefix, ours)
	return names
}

// liveProbeSayDiff 念两份名字集合的**双向**差集：多出来的那几枚才是"这一趟留下的"，
// 少掉的那几枚只数个数（它们多半是用户自己移走的，与本次无关）。
func liveProbeSayDiff(what string, before, after map[string]bool, sink *liveProbeSink) {
	if before == nil || after == nil {
		sink.say("%s 差集：**两遍里有一遍空白** ⇒ 这一格仍是空白（不读成「没留下东西」）", what)
		return
	}
	var added []string
	removed := 0
	for name := range after {
		if !before[name] {
			added = append(added, name)
		}
	}
	for name := range before {
		if !after[name] {
			removed++
		}
	}
	sort.Strings(added)
	sink.say("%s 净差 = 多 %d 枚 / 少 %d 枚", what, len(added), removed)
	for _, name := range added {
		if strings.HasPrefix(name, CloudUploadNamePrefix) {
			sink.say("%s 多出来（**我方造的那一枚**，它没被链尾删掉）：%s", what, name)
			continue
		}
		sink.say("%s 多出来（不是我方前缀 ⇒ 名字不念）：扩展名 %q，名字长度 %d",
			what, strings.ToLower(path.Ext(name)), len([]rune(name)))
	}
}
