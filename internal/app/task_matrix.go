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
// 本层有**两个**读者，各自只取旧实现那一半：
//   - [taskMatrixSkipsClick]（写侧 default 分支那一发盲点）—— 命中就**不发**打卡；
//   - `task_matrix_report.go` 的 [TaskVerifyLines] —— 命中就**不出播报行**。
//
// ⚠ 播报那一层只挡**行**、不挡**计数**（旧实现是连 `results.success` 一起 return 掉，
// 于是那一档在界面上既看不见也不算数）。差异与理由写在那份文件的"刻意不同③"里，
// 改这个常量的时候两边都要回看。
//
// ⚠ 它**不**挡专属 runner —— 那张表在 `task_matrix_dispatch.go` 里只用于
// "命中就不发这一发打卡"，而"runner 照跑"那一半是旧实现的实情。
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
// ⚠ 本结构体**不进 JSON**（未登记 json tag 白名单 ⇒ 门禁会红）：它的读数由
// `task_matrix_run.go` 那段翻成 [TaskMatrixRunResult] 后才进结论，
// 直接把这一份交给界面会把"有几组没读到"那一格丢掉。
type TaskMatrix struct {
	// Items 是聚合 + 去 HTML + 排序 + 去重之后的条目，**未过"要不要做"那道门**。
	Items []upstream.TaskV2Item
	// Raw 是**去重之前**的条目数（16 次拉取加起来的全部条目）。
	//
	// 它只为播报那一行存在（旧实现那句 `获取到 32 个任务，去重后 24 个`，
	// `index.ts` 里 `logger.debug` 那一句），但这一格本身就是有用事实：
	// `Raw` 与 [TaskMatrix.Items] 的差就是"上游在不同组里重复给了几条同名任务"，
	// 而重复条目留下的那条由 id 决定（见 [normalizeTaskMatrix] 的顺序讲究）。
	Raw int
	// Fetches 是实际发出的请求数（恒等于 [TaskMatrixFetches]，除非中途整体失败）。
	Fetches int
	// Failures 是**没拿到**的那些组（顺序与请求顺序一致）。
	//
	// ⚠ 它不是"错误"，是"这一组今天没读到"。全部读到时是空切片。
	Failures []TaskMatrixFailure
	// Markets 是**按市场名分开的**那一半读数，顺序与 [TaskMatrixMarketNames] 一致。
	//
	// 为什么要单独有一格（而不是只看 [TaskMatrix.Raw]）：这一族要两个市场名各拉八次，
	// 而聚合把两边摊成一张表 ⇒ 只报总数时，"邮箱那一支一条都没回"与"两支各回一半"
	// 在小结里长得**一模一样**。缺的正是这一格（本文 R14）。
	//
	// ⚠ 这一格只**记录**、不参与任何判定 ⇒ 它不会改哪一条任务做不做。
	// 加它不改判据，只让"跑一趟"能把 R13 / R14 那两问答出来（AGENTS.md §1.13）。
	Markets []TaskMatrixMarketCount
	// Groups 是**逐次拉取**的组级读数，顺序与请求顺序一致（2 × 8 = 16 段，
	// 中途整体失败时可能少几段）。
	//
	// 为什么单独有一格：`groupState.<组>.taskFinishState` 与条目 `process` 这两格**此前**
	// 只存进结构体、一个字都不念（`process` 另有一个读者：522 停表），
	// 而 R15 要的就是"它随不随任务完成而动、它跟组内是否全 `FINISH` 对不对得上"。
	// 这两问光看代码答不出，而跑一趟如果没人念出来，跑十趟也答不出 ⇒ 与 [TaskMatrix.Markets]
	// 同一条理由（AGENTS.md §1.13：缺口的默认处置是去跑，先得看得见）。
	// 念出来之后今天已经答出两格：`taskFinishState` 与"组内是否全 `FINISH`"同日 18 次读数
	// 零例外同向（`0`/`1`/`2` 三档），`process` **不是恒 0**、同一趟内会涨。
	// ⚠ 两者都是**相关性观察** ⇒ 仍不改判据（结论与还缺哪一格见 R15）。
	//
	// ⚠ 只**记录**、不参与任何判定：这两格该不该升格成判据是 R15 的**结论**，不是前提。
	Groups []TaskMatrixGroupCount
}

// TaskMatrixGroupCount 是**一次**拉取（一个市场名 × 一个请求组名）的组级读数。
//
// 三段各对应 R15 的一种读法：条数与 `Finished` 是"组内是否全完成"，
// `States` 是上游给的"完成度"那一格的原样取值，`ProcessValues` 是条目 `process`
// 在这一组里出现过的面值。把三者**并排**念出来，才有分辨"是不是同一件事"的样本。
type TaskMatrixGroupCount struct {
	// MarketName 与 [TaskMatrixMarketCount.MarketName] 同义（我们发出去的那个）。
	MarketName string
	// Group 是**请求**带的那一个组名 —— 不取响应键名，理由见 `TaskListV2.Group` 那条注释。
	Group string
	// Items 是这一次回来的条目数（**去重之前**，只看这一组）。
	Items int
	// Finished 是其中 `state == "FINISH"` 的条数（逐字比对，口径同 [taskFinished]）。
	Finished int
	// ProcessValues 是这一组条目里 `process` 出现过的面值（去重、升序；缺失的不算一档）。
	ProcessValues []int64
	// States 是响应顶层 `groupState` 里的**全部**键值（按组名升序）。
	//
	// ⚠ 不按"请求那一个组"筛选：实拍里 `groupState` 只带本次那一个键，但那是**一次观测**
	// 不是契约（条目那一侧同理是全摊平）。它给了别的键名，恰恰是 R15 要看见的事实。
	States []TaskMatrixStateEntry
}

// TaskMatrixStateEntry 是 `groupState` 里的一个键。
type TaskMatrixStateEntry struct {
	// Group 是那一枚键名。
	Group string
	// Value 是 `taskFinishState` 的原样取值。
	//
	// ⚠ `nil` 只表示"键在、值不是整数或缺失"，而**今天这一档走不到**：上游层那份
	// [parseGroupState] 只收整数档，非整数的键**整个被丢掉** ⇒ 界面上"没给"与"给了但
	// 不是整数"是同一档（`TestAppTaskMatrixRecordsGroupReadings` 把这一格钉成了断言）。
	// 保留 nil 分支是防"将来解析器放宽"时静默读成 0 —— ⚠ 它今天**未被断言**。
	Value *int64
}

// TaskMatrixMarketCount 是**一个市场名**那一半边这一次拉到了什么。
type TaskMatrixMarketCount struct {
	// MarketName 是**我们发出去**的那个市场名（请求 body 的 `marketname`，
	// 同时也是 `activityid` 头的取值 —— 见 [TaskListV2Request]）。
	MarketName string
	// Fetches 是这一半边发出的请求数（恒等于 [upstream.TaskListV2Groups] 的长度）。
	Fetches int
	// Failures 是这一半边**没读到**的次数。
	Failures int
	// Raw 是这一半边回来的条目数（**去重之前**，且只看这一边自己的响应）。
	Raw int
	// FieldValues 是这些响应条目里 `marketname` **字段**出现过的取值（去重、升序）。
	//
	// ⭐ 这一格就是 R13 的判据：请求带的市场名与响应里的字段值**对不对得上**，
	// 只有真打才看得到 —— 若 `newsign_139mail` 那八次回来的条目仍写着 `sign_in_3`，
	// 说明上游把 `activityid` 当成常量、我们读到的是**同一张表**（两批数据叠成一批）。
	// 空的条目数组它自己就表达了（`Raw == 0` 时这一格必空）。
	FieldValues []string
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
	matrix := TaskMatrix{Failures: []TaskMatrixFailure{}, Markets: []TaskMatrixMarketCount{}}
	for _, marketName := range TaskMatrixMarketNames {
		market := TaskMatrixMarketCount{MarketName: marketName, FieldValues: []string{}}
		seen := map[string]bool{}
		for _, group := range upstream.TaskListV2Groups {
			list, perr := do(ctx, src, func(s SignInSession) (upstream.TaskListV2, *core.Error) {
				return src.up.AppTaskListV2(ctx, s.jwt, s.referer, marketName, group)
			})
			matrix.Fetches++
			market.Fetches++
			if perr != nil {
				matrix.Failures = append(matrix.Failures, TaskMatrixFailure{
					MarketName: marketName, Group: group, Err: perr,
				})
				market.Failures++
				continue
			}
			market.Raw += len(list.Items)
			// ⚠ 变量名用 `reading`，不复用 `group`：外层循环变量 `group` 是**请求那枚组名**，
			// 而这一格是它的读数（同名会让 `Group: group` 那一句读起来像自引用）。
			reading := TaskMatrixGroupCount{
				MarketName: marketName, Group: group,
				Items:         len(list.Items),
				ProcessValues: []int64{}, States: []TaskMatrixStateEntry{},
			}
			seenProcess := map[int64]bool{}
			for _, item := range list.Items {
				if item.MarketName != nil && !seen[*item.MarketName] {
					seen[*item.MarketName] = true
					market.FieldValues = append(market.FieldValues, *item.MarketName)
				}
				if taskFinished(item) {
					reading.Finished++
				}
				if item.Process != nil && !seenProcess[*item.Process] {
					seenProcess[*item.Process] = true
					reading.ProcessValues = append(reading.ProcessValues, *item.Process)
				}
			}
			sort.Slice(reading.ProcessValues, func(i, j int) bool {
				return reading.ProcessValues[i] < reading.ProcessValues[j]
			})
			for name, value := range list.GroupFinishStates {
				reading.States = append(reading.States, TaskMatrixStateEntry{Group: name, Value: value})
			}
			sort.Slice(reading.States, func(i, j int) bool {
				return reading.States[i].Group < reading.States[j].Group
			})
			matrix.Groups = append(matrix.Groups, reading)
			matrix.Items = append(matrix.Items, list.Items...)
		}
		sort.Strings(market.FieldValues)
		matrix.Markets = append(matrix.Markets, market)
	}
	if len(matrix.Failures) == matrix.Fetches && matrix.Fetches > 0 {
		// 一条错误就够排障了：带上前两次失败的市场名/组名与第一个错误原文。
		first := matrix.Failures[0]
		return TaskMatrix{}, first.Err.WithPrefix(fmt.Sprintf(
			"任务表 %d 次拉取全部失败（首个：%s/%s）：", matrix.Fetches, first.MarketName, first.Group))
	}
	matrix.Raw = len(matrix.Items)
	matrix.Items = normalizeTaskMatrix(matrix.Items)
	return matrix, nil
}

// MarketSummaryLine 把 [TaskMatrix.Markets] 念成**一行**给人看的读数。
//
// 只念读数、不加解释（口径与 `noticeSlotReadings`、`FunaiReadings.Line` 一致）：
// "邮箱那一支回了 0 条"究竟是"这个号没开邮箱任务"还是"上游不认我们这个 `activityid`"，
// 这一句分辨不了，也不该在这一句里分辨（AGENTS.md §1.7）。
//
// 空表回空串 ⇒ 调用方**不给**这一行（没有读数可念时不播一句"没读数"）。
func (m TaskMatrix) MarketSummaryLine() string {
	if len(m.Markets) == 0 {
		return ""
	}
	parts := make([]string, 0, len(m.Markets))
	for _, market := range m.Markets {
		text := fmt.Sprintf("%s：读到 %d/%d 次、%d 条",
			market.MarketName, market.Fetches-market.Failures, market.Fetches, market.Raw)
		if len(market.FieldValues) == 0 {
			text += "、响应里没带 marketname"
		} else {
			text += "、响应里的 marketname：" + strings.Join(market.FieldValues, "、")
		}
		parts = append(parts, text)
	}
	return "拉取明细 " + strings.Join(parts, "；")
}

// GroupReadingLines 把 [TaskMatrix.Groups] 念成**每个市场名一行**读数。
//
// `label` 只出现在行头（"拉表前" / "复查后"），用来把**同一条链上的两次读数**对上：
// R15 问的两件事里，"它随不随任务完成而动"只有把两次并排放进同一份事件流才看得见。
//
// 口径与 [TaskMatrix.MarketSummaryLine] 同一条：**照抄面值、不加解释**。
// 这一句不判"taskFinishState=2 是不是这一组做完了"，也不判"process=0 是不是没进度"
// —— 那正是 R15 还没有答案的地方（AGENTS.md §1.7）。
//
// 没有读数时回 nil ⇒ 调用方不给行（与 [MarketSummaryLine] 空表回空串同一取舍）。
func (m TaskMatrix) GroupReadingLines(label string) []string {
	if len(m.Groups) == 0 {
		return nil
	}
	head := "组读数"
	if label != "" {
		head = fmt.Sprintf("组读数（%s）", label)
	}
	lines := make([]string, 0, len(TaskMatrixMarketNames))
	for _, marketName := range TaskMatrixMarketNames {
		parts := make([]string, 0, len(upstream.TaskListV2Groups))
		for _, reading := range m.Groups {
			if reading.MarketName == marketName {
				parts = append(parts, reading.line())
			}
		}
		if len(parts) == 0 {
			continue
		}
		lines = append(lines, fmt.Sprintf("%s %s：%s", head, marketName, strings.Join(parts, "；")))
	}
	return lines
}

// line 是**一组**那一段读数。
//
// ⚠ 三截的顺序按"R15 的两问怎么排"来：先看上游给的 `taskFinishState`，
// 再看组内条目的 `state` 是不是全 `FINISH`，最后看 `process` 的面值 —— 前两截并排
// 才看得出"完成度那一格跟实际完成状态对不对得上"。
func (g TaskMatrixGroupCount) line() string {
	states := "taskFinishState 没给"
	if len(g.States) > 0 {
		pairs := make([]string, 0, len(g.States))
		for _, entry := range g.States {
			if entry.Value == nil {
				pairs = append(pairs, entry.Group+"=值不是整数")
				continue
			}
			pairs = append(pairs, fmt.Sprintf("%s=%d", entry.Group, *entry.Value))
		}
		states = "taskFinishState{" + strings.Join(pairs, "、") + "}"
	}
	if g.Items == 0 {
		return fmt.Sprintf("%s：%s、回 0 条", g.Group, states)
	}
	process := "process 全没给"
	if len(g.ProcessValues) > 0 {
		values := make([]string, 0, len(g.ProcessValues))
		for _, value := range g.ProcessValues {
			values = append(values, fmt.Sprintf("%d", value))
		}
		process = "process{" + strings.Join(values, "、") + "}"
	}
	return fmt.Sprintf("%s：%s、%d 条（FINISH %d）、%s", g.Group, states, g.Items, g.Finished, process)
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
