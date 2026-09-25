package app

// app-task 那一趟的**逐条播报**：把"读了多少条、对哪几条发了枪、上游认没认"
// 一行一行写进事件流，而不是只留末尾那一句小结。
//
// # 为什么这一层不是装饰
//
// 在这之前，一条 app-task 要发四十多枪、跑三十来秒，而界面上只多出**一行**
// [TaskMatrixRunResult.SummaryLine]。用户看不到"这一趟在我的账号上动了哪几条"，
// 而这一条链恰恰是全仓**唯一会写用户的账**的那一条（每一发 `task/click` 都告诉
// 上游"这条我做完了"，且不可回滚）⇒ 逐条播报在这里是**可审计**，不是好看。
//
// # 文案基准（逐字抄自旧实现）
//
// 来自 `packages/core/src/tasks/app/results.ts`：
//
//	分节   ------【每日任务】------
//	成功   成功 <名称> <id>
//	已完成  已完成 <名称>
//	失败   失败 <名称> <id>
//	仍待办  需前往{云盘|邮箱}完成 <名称> <id>
//	上传   上传文件成功
//	清理   ------【清理临时文件】------ / 删除文件 <id,…> / 删除文件成功（旧 CLI 活体输出）
//
// 组名表与组序规则也照它，理由见下面两处各自的注释。
// "清理"那一节另有两处小差异，写在 [TaskCleanupLines] 旁边（不进下面这个"四处"的编号，
// 那四处都是**复查报告**那一块的取舍）。
//
// # 与旧实现**刻意不同**的四处
//
//  1. **档位只有三档**。旧实现有 debug/info/warn/error/fatal 五档，另有
//     start/success/fail 三种"着色类型"（[core.LogLevel] 旁边写清了本仓为什么只有
//     info/warn/error —— 这是给用户看的那一路，事件种类与 JSON 形状是冻结契约）。
//     所以这里把"分节标题 / 成功 / 已完成"落 info，"仍待办"落 warn
//     （[core.LevelWarn] 的声明正是"要人去做点什么"），"失败"落 error。
//  2. **不照抄那个双空格**。旧实现那句是
//     `logger.success("成功", name, message || "", id)`，而全仓没有任何地方给
//     `TaskResults.success` 填 `message`（`types/app.ts:69` 那一格是可选且恒空）⇒
//     实拍里 `成功 手动上传一个文件  106` 那两个空格是**未填的可选字段**留下的痕迹，
//     不是文案。这里用单空格。
//  3. **SKIP 表只挡行、不挡计数**。旧实现那张 11 个 id 的表同时用于
//     "不发盲点打卡"与"不进报告"（`results.ts:33` 在 `collectTaskResult` 最前面 return）。
//     这里只让它挡**播报行**：计数照记。否则界面上的"N 条"与行内条数对不上，
//     而那种不一致比多几行噪声难查得多。
//  4. **空组名给一句人话**。旧实现未知组回落成 `groupid` 本身，于是 `groupid` 缺席的
//     条目会印出 `------【】------`。这里回落成"未分组任务"。
//     ⚠ 其余未知组名**仍**回落成原值（宁可显示英文，也不静默丢一组）。

import (
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// TaskReportLine 是一行播报：级别 + 文本。
//
// 之所以先做成切片再播出去（而不是各处直接调 `progress`）：这一层的规则
// （组序、组内四档顺序、SKIP 表那格）必须能**单独测**，而把它做成纯函数之后，
// 测试不需要装一条会话、也不需要数请求枪数。
type TaskReportLine struct {
	Level core.LogLevel
	Text  string
}

// taskGroupNameCN 是旧实现 `GROUP_NAMES` 那张表（逐字）。
//
// ⚠ 未登记的组回落成**原组名**而不是空：少一个键的表现必须是"这一组显示成英文"，
// 不能是"这一组凭空消失"（与 web 侧 `GROUP_LABEL` 同一条纪律）。
func taskGroupNameCN(group string) string {
	if group == "" {
		// 刻意不同④：旧实现这里会印出 `------【】------`。
		return "未分组任务"
	}
	switch group {
	case "day":
		return "每日任务"
	case "month":
		return "每月任务"
	case "new":
		return "新用户任务"
	case "time":
		return "热门任务"
	case "hidden", "hiddenabc":
		return "隐藏任务"
	case "beiyong1":
		return "临时任务"
	case "cloudEmail":
		return "云邮箱任务"
	}
	return group
}

// taskVerifyTier 是旧实现那四格（`success` / `completed` / `failed` / `incomplete`）。
type taskVerifyTier int

const (
	tierDoneNow taskVerifyTier = iota
	tierAlreadyWas
	tierFailed
	tierStillOpen
)

// tierOfVerify 把复查类别映射到播报档位。
//
// ⚠ [VerifyHidden] 交回 `false`：旧实现那两组（`hidden` / `hiddenabc`）根本不进报告，
// 这里也照它。计数仍在 [TaskRunCounts.Hidden] 里，别把它读成"没读到"。
func tierOfVerify(class TaskVerifyClass) (taskVerifyTier, bool) {
	switch class {
	case VerifyDoneNow:
		return tierDoneNow, true
	case VerifyAlreadyWas:
		return tierAlreadyWas, true
	case VerifyFailed:
		return tierFailed, true
	case VerifyStillOpen:
		return tierStillOpen, true
	case VerifyHidden:
		return 0, false
	}
	return 0, false
}

// taskVerifyLine 是一条复查读数对应的播报行。
//
// `record.Label` 只有 [tierStillOpen] 用得上（"该去哪个 App 做"，来自
// [VerifyMarketLabel]），`record.Note` 则**哪一档都可能带**（补打卡那一发就挂在
// [tierDoneNow] / [tierFailed] 那两条上）—— 把它拼在同一行尾巴，是为了让
// "这行说的这条任务今天被动过两次"在**这一行**里就能读出来，而不是去小结里找。
func taskVerifyLine(tier taskVerifyTier, record TaskVerifyRecord) TaskReportLine {
	line := TaskReportLine{Level: core.LevelInfo}
	switch tier {
	case tierDoneNow:
		// 刻意不同②：旧实现那句里的 `message` 恒空 ⇒ 不复制那两个空格。
		line.Text = fmt.Sprintf("成功 %s %d", record.Name, record.TaskID)
	case tierAlreadyWas:
		// 旧实现这句**不带 id**（`results.ts:108`），照它：这一档不是我们做的，
		// 报 id 只会让人以为"这趟动了它"。
		line.Text = fmt.Sprintf("已完成 %s", record.Name)
	case tierFailed:
		line.Level = core.LevelError
		line.Text = fmt.Sprintf("失败 %s %d", record.Name, record.TaskID)
	case tierStillOpen:
		// 档位来自 [core.LevelWarn] 的声明："要人去做点什么"。
		line.Level = core.LevelWarn
		// ⚠ 这一档**包含**"本工具没接玩法"的那些：我们没做，而用户去 App 里点一下
		// 确实能做 ⇒ 这句话对两者都成立。第一趟那行 [taskStepLiveLine] 会说清楚
		// 这一条是"工具没接"还是"没到点"。
		line.Text = fmt.Sprintf("需前往%s完成 %s %d", record.Label, record.Name, record.TaskID)
	}
	if record.Note != "" {
		// 用"："而不是再套一层括号：那几则 note 自己就带括号
		// （"补打卡已发出（task2，这一枚 key 零实拍）"），套上去会读成嵌套。
		line.Text += "：" + record.Note
	}
	return line
}

// TaskVerifyLines 把复查读数翻成"按组分节 + 组内四档"的播报行。
//
// 两条顺序规则都照旧实现（`printTaskResultsByGroup` / `printGroupResults`）：
//
//   - **组内**：成功 → 已完成 → 失败 → 仍待办。
//   - **组间**：按"把四档拼成一条长队之后，组名第一次出现的顺序"
//     （旧实现拼的顺序是 `success, failed, incomplete, completed`）。
//
// ⚠ 组间那条不是洁癖：它让"这趟真做了事"的组排在前面。旧实现那两轮实拍正是靠它
// 解释的 —— 账号 1 有 `成功 手动上传一个文件 106`（`day` 组）所以"每日任务"在最前，
// 账号 2 那一组只剩"已完成"所以排到最后。改成固定组序会让两轮的读法不一样，
// 而那个"先说我做了什么"的取向恰恰是这一层想要的。
func TaskVerifyLines(verified []TaskVerifyRecord) []TaskReportLine {
	// 先按档分桶（桶内保持复查读数顺序 = 第二趟表里的 id 升序）。
	byTier := map[taskVerifyTier][]int{}
	for i, record := range verified {
		tier, ok := tierOfVerify(record.Class)
		if !ok {
			continue
		}
		if taskMatrixSkipsClick(record.TaskID) {
			// 刻意不同③：旧实现连计数一起丢，这里只挡行。
			continue
		}
		byTier[tier] = append(byTier[tier], i)
	}

	// 组序：按 success → failed → incomplete → completed 的顺序首次出现。
	var groups []string
	seenGroup := map[string]bool{}
	for _, tier := range []taskVerifyTier{tierDoneNow, tierFailed, tierStillOpen, tierAlreadyWas} {
		for _, idx := range byTier[tier] {
			group := verified[idx].Group
			if !seenGroup[group] {
				seenGroup[group] = true
				groups = append(groups, group)
			}
		}
	}

	out := make([]TaskReportLine, 0, len(verified)+len(groups))
	for _, group := range groups {
		out = append(out, TaskReportLine{Level: core.LevelInfo,
			Text: fmt.Sprintf("------【%s】------", taskGroupNameCN(group))})
		// 组内顺序：成功 → 已完成 → 失败 → 仍待办。
		for _, tier := range []taskVerifyTier{tierDoneNow, tierAlreadyWas, tierFailed, tierStillOpen} {
			for _, idx := range byTier[tier] {
				record := verified[idx]
				if record.Group != group {
					continue
				}
				out = append(out, taskVerifyLine(tier, record))
			}
		}
	}
	return out
}

// taskStepLiveLine 是第一趟**动手之前**那一行播报。
//
// 它是"实时"那一半：一趟三十来秒、四十多枪，用户要能在事情**正在发生**时看到
// 这一条是哪一条，而不是等末尾那一句小结。
//
// 交回 `false` 的两档都是"这一条根本不出网"：按规则被挡又没有上传的那一支
// （beiyong1 的 `currstep` 门）、以及没有 id 的条目（分派层就挡了，走不到这里）。
// 不发这一行是为了让"界面上有几行"与"发了几枪"能对上 —— 那些条数由
// [TaskRunCounts.RuleSkipped] 与小结算。
func taskStepLiveLine(item upstream.TaskV2Item, plan TaskStepPlan) (TaskReportLine, bool) {
	name, id := taskNameOf(item), derefTaskID(item)
	switch plan.Kind {
	case StepClick, StepRunner:
		text := fmt.Sprintf("尝试完成 %s %d", name, id)
		if plan.Runner != "" {
			// runner 名字带出来：四条玩法共用两档 key，界面上只有这一格能答
			// "这一条是谁做的"（与 [TaskStepRecord.Runner] 同一理由）。
			text = fmt.Sprintf("尝试完成 %s %d（玩法 %s）", name, id, plan.Runner)
		}
		return TaskReportLine{Level: core.LevelInfo, Text: text}, true
	case StepBlockedByRule:
		if plan.Upload == nil {
			return TaskReportLine{}, false
		}
		// 那道门**只挡打卡**，上传照做 ⇒ 这一行必须说清"只传文件"，
		// 否则用户看到"尝试完成"会以为我们也上报了进度（那是谎报）。
		return TaskReportLine{Level: core.LevelInfo,
			Text: fmt.Sprintf("只传文件、不发打卡 %s %d：%s", name, id, taskPlanReason(plan))}, true
	case StepBlockedToolGap, StepNeedsHuman:
		// 形状照旧实现那句 `跳过无法自动化的任务: 生成我的校园海报(33)`，
		// 内容带上**原因**：这两档的全部意义就是"说清楚为什么没做"。
		return TaskReportLine{Level: core.LevelWarn,
			Text: fmt.Sprintf("跳过 %s %d：%s", name, id, taskPlanReason(plan))}, true
	}
	return TaskReportLine{}, false
}

// taskPlanReason 取"为什么是这个类别"那一句。
//
// 两格都要看：[TaskStepPlan.Reason] 与 [TaskStepPlan.ToolGap]，且优先前者
// —— [apply] 里也是这个顺序（`record.Reason` 空时用 `plan.ToolGap` 补）。
// 两处口径必须一致，否则界面上的那一行与那条记录会说两句不同的话。
func taskPlanReason(plan TaskStepPlan) string {
	if plan.Reason != "" {
		return plan.Reason
	}
	return plan.ToolGap
}

// TaskCleanupOutcome 是"这一趟造出来的网盘文件删了没有"那一档。
//
// 三档而不是 bool，因为"没删"有**两种完全不同的原因**：这一趟设了保留（故意的），
// 和发了删除那一枪而没成（要人去补删）。把它们合成一档的表现是用户在网盘里
// 看到一堆 `asign-*.png`，而日志里那句"清理失败"分不清是哪种。
type TaskCleanupOutcome int

const (
	// TaskCleanupNotRun 是"这一趟根本没有需要清理的东西"。
	TaskCleanupNotRun TaskCleanupOutcome = iota
	// TaskCleanupDone 是删掉了（上游认了）。
	TaskCleanupDone
	// TaskCleanupKept 是刻意留着：[TaskMatrixRun.KeepUploadedFiles] 设了真。
	TaskCleanupKept
	// TaskCleanupFailed 是发了那一枪而没成 —— ⚠ 文件仍在用户网盘里。
	TaskCleanupFailed
)

// TaskCleanupLines 播"清理临时文件"那一节。
//
// 文案基准是旧 CLI 的**活体输出**（不是注释）：
//
//	------【清理临时文件】------
//	删除文件 <id>,<id>,…            ← 旧实现 `after.ts:134`，一次多个、逗号相连
//	删除文件成功                     ← `utils/file.ts:120`
//	删除文件失败 <原因>              ← 同函数那两行 error
//
// 与旧实现不同的两处，都是"这一条链自己判"带来的：
//
//   - **没有东西要清时整节不播**。旧实现那一节属于 `afterTask`（无论有没有文件都要打
//     "无临时文件需要清理"），而这里是 app-task 链内的一步：一个文件都没写过还播一节
//     "清理"会让人以为这一趟写过东西。
//   - id 那一行落 **info**（旧实现是 debug）。本仓没有 debug 档，而它是这一节里
//     唯一"清理失败之后还能照着手动删"的信息，不该被默认档位滤掉。
func TaskCleanupLines(fileIDs []string, outcome TaskCleanupOutcome, reason string) []TaskReportLine {
	if len(fileIDs) == 0 {
		return nil
	}
	lines := []TaskReportLine{
		{Level: core.LevelInfo, Text: "------【清理临时文件】------"},
		{Level: core.LevelInfo, Text: "删除文件 " + strings.Join(fileIDs, ",")},
	}
	switch outcome {
	case TaskCleanupDone:
		lines = append(lines, TaskReportLine{Level: core.LevelInfo, Text: "删除文件成功"})
	case TaskCleanupKept:
		// warn 而不是 info：这一档的读法是"这些文件现在还在你的网盘里，是你（或这个
		// 客户端的设置）让它留的"。`reason` 说清是谁设的。
		lines = append(lines, TaskReportLine{Level: core.LevelWarn,
			Text: fmt.Sprintf("未清理 %d 个：%s", len(fileIDs), reason)})
	case TaskCleanupFailed:
		lines = append(lines, TaskReportLine{Level: core.LevelError,
			Text: "删除文件失败：" + reason})
	case TaskCleanupNotRun:
		// 有 id 却判成"不用清理"是调用方写错了。这里不静默：宁可少一行结局，
		// 也不编造一句"成功"。
	}
	return lines
}
