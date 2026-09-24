package app

// 「App 任务矩阵」的**读**那一半：把 `taskListV2` 按 2 × 8 = 16 次拉全，聚合成一张表。
//
// # 证据
//
// 对应 `docs/business-feature-inventory.md` §2.3 那句"`app` 不该是一个能力，应拆成
// `task-list`（查询）+ 每种玩法一个能力"里的**查询**那一半 —— 但它是**另一张表**：
// 现成的 `task-list` 能力打的是 `taskListV3`（签到页那张），这一条打的是
// 旧实现 `packages/core/src/tasks/app/helpers.ts:40-71` 那一族（App 任务矩阵）。
// 两口同形不同请求（`app_task_list_v2.go` 文件头记了差异）。
//
// 这一族的响应形状有**本仓夹具的两条实拍**（`test/fixtures/http/3d.json`：
// group `new` 空、group `beiyong1` 三条），所以聚合层的输入形状不是推的。
//
// # 旧实现怎么读那张表（本层照抄的部分）
//
//   - 两个市场名 × 八个 group：`helpers.ts:44` 的数组顺序 + 调用顺序，逐条照抄；
//   - **一次失败不算失败**：旧 TS `if (!err && data?.result)` 直接跳过那一组（`helpers.ts:65`），
//     整个函数不会因为某个 group 挂掉而中断 ⇒ 这里同样逐次容错，把失败的组**登记**下来交出去；
//   - 摊平：`Object.values(result).flat()`，不看键名；
//   - `name` 去 HTML 标签（`helpers.ts:88-91`，正则 `/<[^>]+>/g`）；
//   - 先按 `id` 升序排，再按 `name` 去重**保留先来的那条**（`tasks.ts:12-14` 的 `uniqBy`）
//     ⇒ 同名的两条任务，id 小的赢。
//
// # 与旧实现**刻意不同**的三处（都在下面各自的注释里）
//
//  1. 十六次**串行**跑，不并发（旧 TS 是两个市场名并发、各自内部串行）。
//  2. **十六次全挂 ⇒ 整体报错**（旧 TS 回一张空表）。
//  3. `name` 缺席的条目**各自保留**（旧 TS 会把它们全去重成一条）。
//
// 这三处都是"实现层面更安全"，不改任何**判据**；反过来"上游认不认我们这一组的
// `activityid`"仍**未测试**（一次真打都没打过）。

import (
	"context"
	"fmt"
	"regexp"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// TaskMatrixMarketNames 是这一族请求的两个市场名，**顺序即合并顺序**（先云盘后邮箱）。
//
// 旧 TS 是 `Promise.all([getAppTaskList(ctx,"sign_in_3"), getAppTaskList(ctx,"newsign_139mail")])`
// 再 `[...cloudTasks, ...emailTasks]` ⇒ 有效顺序就是这一个。
var TaskMatrixMarketNames = []string{
	upstream.MarketName, upstream.TaskListV2EmailMarketName,
}

// TaskMatrixNeverClick 是旧实现那份 `SKIP_TASK_LIST`（`tasks.ts:111`，逐字）。
//
// ⚠ **它在旧实现里挡的不是"做不做"**，而是两件事：
// ① 通用那次"盲点" `task/click`（`tasks.ts:101-103`，命中就**不发**这一枪）；
// ② 结果播报（`results.ts:33`，命中就**不出现在报告里**）。
// 任务列表照样拉、专属 runner 照样跑 ⇒ 把它读成"这 11 个 id 永远不做"是**错**的。
//
// 本层今天只有读的那一半，所以这个常量**没有调用点** —— 登记在此是为了接写侧时
// 那一次盲点点击有出处，不是因为"已经用上了"。
var TaskMatrixNeverClick = []int64{117, 437, 991, 406, 385, 480, 1022, 1018, 603, 1021, 120}

// TaskMatrixFetches 是这一次聚合**注定要发**的请求数（2 × 8）。
//
// 它是算出来的、不是抄来的：将来谁加一个 group 或市场名，这里跟着变，
// 而 `AppTaskMatrix` 那条"全挂才失败"的判据用的是同一个数 ⇒ 两边不会各说各话。
func TaskMatrixFetches() int {
	return len(TaskMatrixMarketNames) * len(upstream.TaskListV2Groups)
}

// TaskMatrixFailure 是**一次**拉取的失败记录。
type TaskMatrixFailure struct {
	MarketName string
	Group      string
	Err        *core.Error
}

// TaskMatrix 是聚合后的那张表。
//
// ⚠ 本结构体**不进 JSON**（未登记 json tag 白名单 ⇒ 门禁会红）：
// 今天它没有调用点，界面上出现它之前要先决定"这是一条能力还是 `task-list` 的第二页"。
type TaskMatrix struct {
	// Items 是聚合 + 去 HTML + 排序 + 去重之后的条目，**未过"要不要做"那道门**。
	Items []upstream.TaskV2Item
	// Fetches 是实际发出的请求数（恒等于 [TaskMatrixFetches]，除非中途整体失败）。
	Fetches int
	// Failures 是**没拿到**的那些组（顺序与请求顺序一致）。
	//
	// ⚠ 它不是"错误"，是"这一组今天没读到"。全部读到时是空切片。
	Failures []TaskMatrixFailure
}

// taskNameTagPattern 对应旧 TS 的 `/<[^>]+>/g`。
var taskNameTagPattern = regexp.MustCompile(`<[^>]+>`)

// AppTaskMatrix 打那 16 次并聚合成一张表。**只读，一件任务都不做**。
//
// 串行（刻意不同①）：旧 TS 两个市场名并发。改成串行的理由是这一族**共用同一条会话**，
// 并发会让"撞 90001 刷一次票、重放一次"这条自救支路在两个 goroutine 里同时判定同一张票，
// 而那条支路的正确性只在串行上被测过（`signin_session.go` 的 [SignInSessionSource.Call]）。
// 顺序与合并结果不受影响 ⇒ 这是实现层面的选择，不是业务口径。
//
// 全挂才失败（刻意不同②）：旧 TS 十六次全失败仍然回**一张空表**，
// 而空表在界面上的读法是"这个号今天没有任务"—— 与"一次都没读到"是两回事。
// 判反的表现是：凭据失效的号被念成"没事可做"，于是没人去看它。
func AppTaskMatrix(ctx context.Context, src *SignInSessionSource) (TaskMatrix, *core.Error) {
	matrix := TaskMatrix{Failures: []TaskMatrixFailure{}}
	for _, marketName := range TaskMatrixMarketNames {
		for _, group := range upstream.TaskListV2Groups {
			list, perr := do(ctx, src, func(s SignInSession) (upstream.TaskListV2, *core.Error) {
				return src.up.AppTaskListV2(ctx, s.jwt, s.referer, marketName, group)
			})
			matrix.Fetches++
			if perr != nil {
				matrix.Failures = append(matrix.Failures, TaskMatrixFailure{
					MarketName: marketName, Group: group, Err: perr,
				})
				continue
			}
			matrix.Items = append(matrix.Items, list.Items...)
		}
	}
	if len(matrix.Failures) == matrix.Fetches && matrix.Fetches > 0 {
		// 一条错误就够排障了：带上前两次失败的市场名/组名与第一个错误原文。
		first := matrix.Failures[0]
		return TaskMatrix{}, first.Err.WithPrefix(fmt.Sprintf(
			"任务表 %d 次拉取全部失败（首个：%s/%s）：", matrix.Fetches, first.MarketName, first.Group))
	}
	matrix.Items = normalizeTaskMatrix(matrix.Items)
	return matrix, nil
}

// normalizeTaskMatrix 做那三步：去 HTML ⇒ 按 id 升序 ⇒ 按 name 去重保留先来。
//
// 顺序**有讲究**：旧实现是先 sort 再 dedup（`index.ts:16-17`），所以"id 小的赢"
// 是靠这个次序成立的。反过来做（先去重再排序）留下的就是**请求顺序**里第一条，
// 而请求顺序由 group 数组决定、与 id 无关 ⇒ 会留下一条 id 更大的，
// 将来接写侧时"这条任务做到第几步"（`currstep`）就跟着错。
func normalizeTaskMatrix(items []upstream.TaskV2Item) []upstream.TaskV2Item {
	out := make([]upstream.TaskV2Item, 0, len(items))
	for _, item := range items {
		if item.Name != nil {
			name := strings.TrimSpace(taskNameTagPattern.ReplaceAllString(*item.Name, ""))
			value := name
			item.Name = &value
		}
		out = append(out, item)
	}
	// 稳定排序：没有 id 的条目排在**最后**、且彼此保持原顺序。
	// ⚠ 不这么做的话（比如把 nil 当 0），无 id 条目会跑到最前面，把同名条目挤掉。
	sort.SliceStable(out, func(i, j int) bool {
		left, leftOK := out[i].TaskID, out[i].TaskID != nil
		right, rightOK := out[j].TaskID, out[j].TaskID != nil
		if !leftOK && !rightOK {
			return false
		}
		if !leftOK {
			return false
		}
		if !rightOK {
			return true
		}
		return *left < *right
	})
	// 按 name 去重，保留先来的那条（排序之后 = id 小的那条）。
	// 刻意不同③：`name` 缺席（nil）的条目**不参与去重**，各留各的。
	// 旧 TS 是 `uniqBy(t => t.name)`，`undefined` 会全撞成一个键 ⇒ 只剩第一条。
	// 那是"少一条任务"的方向 errs，而任务表少一行的表现是**漏做**，不是报错。
	seen := make(map[string]bool, len(out))
	deduped := make([]upstream.TaskV2Item, 0, len(out))
	for _, item := range out {
		if item.Name == nil {
			deduped = append(deduped, item)
			continue
		}
		if seen[*item.Name] {
			continue
		}
		seen[*item.Name] = true
		deduped = append(deduped, item)
	}
	return deduped
}

// Pending 是"这条任务还要不要做"那道门，逐条对应旧实现的主循环（`index.ts:34`）：
// `state === "FINISH" || enable !== 1` ⇒ 跳过。
//
// 两格的**缺省取向**都按旧 TS 的字面语义落：
//   - `enable` 缺席 ⇒ 旧 TS `undefined !== 1` 成立 ⇒ 跳过；
//   - `state` 缺席 ⇒ 旧 TS `undefined === "FINISH"` 不成立 ⇒ **保留**。
//
// ⚠ 两格都是"照抄字面量"，不是"实测过上游真的这么给"：实拍三条**全是** `FINISH` 且
// `enable=1`，即"未完成"那一档在夹具里**一条都没有**（未测试）。
// 因此这一格判反的后果（该做的没做 / 做完的又做一遍）要等第一次真打才能核。
func (m TaskMatrix) Pending() []upstream.TaskV2Item {
	out := make([]upstream.TaskV2Item, 0, len(m.Items))
	for _, item := range m.Items {
		if item.Enable == nil || *item.Enable != 1 {
			continue
		}
		if item.State != nil && *item.State == upstream.TaskStateFinish {
			continue
		}
		out = append(out, item)
	}
	return out
}

// SummaryLine 是一句人话结论（进 run 事件流与界面）。
//
// ⚠ 它**不报**"还剩几次可做"这类额度，也不把 `groupState` 的读数当"完成度"——
// 两个口径都没验过（见 `app_task_list_v2.go` 里 [upstream.TaskListV2.GroupFinishStates] 那条）。
func (m TaskMatrix) SummaryLine() string {
	if len(m.Failures) == 0 {
		return fmt.Sprintf("任务表已读全：%d 条，其中待做 %d 条", len(m.Items), len(m.Pending()))
	}
	return fmt.Sprintf(
		"任务表读到 %d 条（待做 %d 条）；有 %d/%d 组没读到，界面显示的不是全表",
		len(m.Items), len(m.Pending()), len(m.Failures), m.Fetches)
}
