package app

// 「任务清单」跑一个账号那一段 —— **只读**，对应 `docs/business-feature-inventory.md`
// §2.3 里那个"`app` 拆成 `task-list`（查询）+ 每种玩法一个能力"的第一步。
//
// # 为什么先做查询、不做"把任务做掉"
//
// 旧项目那条 `app` 链是**一个读取任务列表 + 按任务码分发**的执行器，
// 分发到的每种玩法（上传、建笔记、发邮件阅读…）各自要另一条链，
// 而那些链**多数尚无抓包样本**（§5.1 那条"前缀 / 端点要按端逐条实测"仍未兑现）。
//
// 先只做查询有两个实际用处：① 界面能回答"这个号今天还有什么没做"；
// ② 它给出的 `id` / `groupid` 正是将来逐条接线时要回传的参数 ——
// 等于把"该做哪些"这一层先钉住，再逐条补"怎么做"。
//
// # 这一条不改任何状态
//
// 只打 `taskListV3` 这一口，**不发** `doTaskPost` / `receiveTask`：那两口是写操作，
// 而"点了就算完成"在上游到底怎么判**未验证**（见下方 SummaryLine 那条注释里的
// 实拍矛盾）。所以界面上这一条承诺的是"看"，不是"做"。

import (
	"context"
	"fmt"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// TaskListClientApp 是 `taskListV3` body 里 `client` 的取值（抓包实测的那个）。
//
// ⚠ `0`（匿名）也返回完整列表，只是 `button.*` 那些字段的取值会变 ——
// 我们**不读**那些字段，所以两个值今天看起来一样，但这不构成"可以传 0"的依据。
// 钉成常量是为了"换一下看看上游反应"这件事变成一处改动，而不是散落各调用点。
const TaskListClientApp uint8 = 1

// TaskList 查一次任务清单。
func TaskList(
	ctx context.Context, src *SignInSessionSource,
) (upstream.TaskList, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.TaskList, *core.Error) {
		return src.up.AppTaskListV3(ctx, s.jwt, s.referer, TaskListClientApp)
	})
}

// TaskListRun 跑一次"只查不做"的任务清单。
type TaskListRun struct {
	SignInWorld
}

// TaskListResult 是一个账号这一趟的产出。
//
// ⚠ 与 [PrizesResult] 同：**没有**"整批失败"那一格 —— 查询失败一律作为
// `*core.Error` 交回编排层（那条路多发一条 `errored` 事件、原文进 `runs.message`）。
type TaskListResult struct {
	Account core.AccountID
	// Items 是任务数组（原样交出去，界面上要按 `groupid` 分组渲染）。
	Items []upstream.TaskItem
	// FinishState 是顶层 `taskFinishState.taskFinishState`。
	//
	// ⚠ **不参与结论判定**（语义未验证，理由见 [upstream.TaskList.FinishState]）。
	// 带着它只是"上游给了就交出去"，别让将来读它的人以为它已经被解释过了。
	FinishState *int64
}

func (r TaskListRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (TaskListResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return TaskListResult{Account: account}, perr
	}
	list, perr := TaskList(ctx, src)
	if perr != nil {
		return TaskListResult{Account: account}, perr
	}
	result := TaskListResult{Account: account, Items: list.Items, FinishState: list.FinishState}
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// TaskListCounts 是把 `state` 数一遍的结果。
type TaskListCounts struct {
	Total    int
	Wait     int
	Finished int
	// Unknown 是 `state` 缺失、或不是实拍那两个取值的条数。
	//
	// 它**必须**单独有一格：把"没给状态"归进"未完成"会让一句"还有 3 条没做"
	// 变成凭空多出来的催促；归进"已完成"更糟 —— 那会让人以为都办完了。
	Unknown int
}

// Counts 数一遍状态。
//
// ⚠ 只认逐字的 `WAIT` / `FINISH`（实拍那两个），其余一律算 [TaskListCounts.Unknown]：
// 大写归一（`strings.ToUpper`）会把一个**新出现的**状态静默读成已知的某一个。
func (res TaskListResult) Counts() TaskListCounts {
	counts := TaskListCounts{Total: len(res.Items)}
	for _, item := range res.Items {
		if item.State == nil {
			counts.Unknown++
			continue
		}
		switch *item.State {
		case upstream.TaskStateWait:
			counts.Wait++
		case upstream.TaskStateFinish:
			counts.Finished++
		default:
			counts.Unknown++
		}
	}
	return counts
}

// SummaryLine 是给人看的一句话小结。
//
// 只报**计数**，不报名目：一张任务表可以有十几条（实拍 18 条），
// 全列会把一条通知变成一段清单；要看得全走界面那条查询。
func (res TaskListResult) SummaryLine() string {
	counts := res.Counts()
	if counts.Total == 0 {
		return "任务清单是空的"
	}
	text := fmt.Sprintf("任务 %d 条：未完成 %d、已完成 %d", counts.Total, counts.Wait, counts.Finished)
	if counts.Unknown > 0 {
		text += fmt.Sprintf("、状态未知 %d", counts.Unknown)
	}
	if groups := res.groupLabels(); groups != "" {
		text += "；分组 " + groups
	}
	return text
}

// groupLabels 给出"分组 × 未完成条数"，最多三组。
//
// 为什么要这一截：`groupid` 是将来接线时唯一能按它分发参数的字段，
// 而"未完成 10 条"里有多少属于 `day`、多少属于 `time` 决定了要不要去碰那些玩法。
//
// 排序排的是**组名的副本**，不是 [TaskListResult.Items] —— 那份是交出去给界面渲染的，
// 统计函数不该改动它的顺序（表现是"界面上的分组顺序跟跑了几次统计有关"）。
func (res TaskListResult) groupLabels() string {
	wait := map[string]int{}
	for _, item := range res.Items {
		if item.State == nil || *item.State != upstream.TaskStateWait {
			continue
		}
		name := "未分组"
		if item.GroupID != nil && *item.GroupID != "" {
			name = *item.GroupID
		}
		wait[name]++
	}
	if len(wait) == 0 {
		return ""
	}
	names := make([]string, 0, len(wait))
	for name := range wait {
		names = append(names, name)
	}
	sort.Strings(names)
	truncated := len(names) > 3
	if truncated {
		names = names[:3]
	}
	parts := make([]string, 0, len(names))
	for _, name := range names {
		parts = append(parts, fmt.Sprintf("%s %d", name, wait[name]))
	}
	text := strings.Join(parts, "、")
	if truncated {
		text += fmt.Sprintf("…（另有 %d 组）", len(wait)-len(names))
	}
	return text
}

// RunReport：**只读，所以只有成功这一态**（与 [PrizesResult.RunReport] 同一条理由）。
//
// 空清单算成功 —— "来看一眼，没有待办的任务"正是这一趟想要的答复。
func (res TaskListResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
