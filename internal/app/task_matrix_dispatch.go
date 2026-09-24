package app

// #9「App 任务矩阵」的**分派**那一半：给一条任务，决定"这一趟对它做什么"。
//
// 本文件是**纯函数 + 表**，一个请求都不发 —— 发请求的部分在 `task_matrix_run.go`。
// 分开写的理由不是洁癖：这里每一格都是"判据"，而判据要能在没有网络的情况下
// 被逐条钉住（含"反过来会怎样"）。
//
// # 对应旧实现哪几行
//
// `packages/core/src/tasks/app/tasks.ts` 的 `dispatchTask`（`switch (task.groupid)`）
// 与 `handleTaskClick` / `clickTaskWithStep`。逐条对照：
//
//| 旧实现 | 本文件 |
//| --- | --- |
//| `case "beiyong1"`：先 `handleTaskClick`，再按任务名 / `button.app.link` 追加上传 | `stepForBeiyong1`（上传那一半见 [StepBlockedToolGap]） |
//| `case "month"`：**没有 break** ⇒ 落到 default，但先过"未开备份"那道门 | `monthGate` + 继续走 default |
//| `default` 里有 runner ⇒ `handleTaskClick`（打卡受 `currstep` 门约束） | [StepRunner]，runner 表里逐条记它"先打卡吗" |
//| `default` 里没 runner、且不在 `SKIP_TASK_LIST` ⇒ 盲点一次 | [StepClick]（`key=task`，**不看** `currstep`） |
//| `clickTaskWithStep`：`currstep != 0` 就不发（434 在 22 这一格例外） | [taskClickGateStep0] 与 434 那条例外 |
//
// # ⚠ 与旧实现**刻意不同**的一处（这一处是行为差异，不是风格）
//
// 旧实现里"有专属 runner 但那条 runner 依赖的端点我们没接"这种情况**不存在**
// （它的 runner 全都在）。我们这边它**存在**，而且有两种处置：
//
//   - 落成 [StepClick]（"反正打卡是通用的"）⇒ 上游会把这条记成**已做**，
//     而实际一件都没做（还可能当场发云朵）。这是**谎报**，不是降级。
//   - 落成 [StepBlockedToolGap] ⇒ 一条枪都不发，结论里写明缺哪一族。
//
// 这里选第二种。代价是"这一趟少做几条"，换来的是界面上那句"待做 N 条"仍然可信。

import (
	"regexp"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/upstream"
)

// TaskStepKind 是对一条任务的**处置决定**。
type TaskStepKind int

const (
	// StepClick 是"发一发通用盲点打卡"（旧实现 default 分支那一枪）。
	StepClick TaskStepKind = iota
	// StepRunner 是"走专属 runner"（runner 自己决定要不要先打卡、打几次）。
	StepRunner
	// StepBlockedToolGap 是"这条的玩法本仓还没接" ⇒ **不发**任何请求。
	//
	// 它必须出现在结论里：静默跳过 = 界面上"待做 8 条"跑完还是 8 条，
	// 而没人知道为什么。
	StepBlockedToolGap
	// StepBlockedByRule 是"按旧实现的规则本来就不该发"（`SKIP_TASK_LIST`、
	// `currstep != 0` 那一类）。它是**正常**结果，不是缺陷。
	StepBlockedByRule
	// StepNeedsHuman 是"这件事要人去 App 里操作，程序代劳不了"（本月没开自动备份）。
	StepNeedsHuman
)

// String 用于结论句与日志（不用于 JSON：本枚举今天不进线格式）。
func (k TaskStepKind) String() string {
	switch k {
	case StepClick:
		return "打卡"
	case StepRunner:
		return "专属玩法"
	case StepBlockedToolGap:
		return "本工具未接"
	case StepBlockedByRule:
		return "按规则跳过"
	case StepNeedsHuman:
		return "需人工"
	}
	return "未知处置"
}

// TaskStepPlan 是对一条任务的决定。
type TaskStepPlan struct {
	Kind TaskStepKind
	// ClickKey 是 [StepClick] 与"runner 先打卡"那一发要带的 `key`。空 ⇒ 这一趟不发打卡。
	ClickKey string
	// Runner 是 [StepRunner] 的 runner 名（进日志与结论，便于"这条是谁做的"）。
	Runner string
	// Reason 是**为什么**是这个决定（人话）。非阻塞态也可能带（比如 434 的进度）。
	Reason string
	// ToolGap 是"这一条的另一半缺哪一族端点"。只有 [StepClick] 也可能带它：
	// beiyong1 的"上传图/照"那一条旧实现是**先打卡、再上传** ⇒ 打卡照发，
	// 上传那一半缺，得在结论里说出来。
	ToolGap string
	// Paced 为真 ⇒ 真的发过枪之后等一点间隔（旧实现只在 `handleTaskClick` 里 `sleep(500)`，
	// default 分支那次盲点**不等**。把两边混成一个的表现：
	// 一天十几条任务全变成"零间隔连发"，那是真实客户端不会有的节奏。
	Paced bool
}

// taskStepInput 是分派要的外部事实（**由调用方读好交进来**，本文件不去取）。
type taskStepInput struct {
	// BackupKnown 是"这个号本月开没开自动备份，我们读到了吗"。
	//
	// ⚠ 它与 [BackupOn] 必须分两格：旧实现读的是 `ctx.state.curMonthBackup`，
	// 那一格在 `appTask` 这条链上常常是 `undefined`，而它的判据写的是
	// `=== false`（**严格等于**）⇒ "没读到"与"读到 false"是两回事，
	// 前者不挡任务。照抄这一点，不"顺手把 undefined 当 false"。
	BackupKnown bool
	BackupOn    bool
	// DayOfMonth 是"今天这个月的第几天"（旧实现用 `< 20` 当门槛）。
	DayOfMonth int
}

// backupGateBlocks 是那道"未开备份"门的**唯一**读法。
//
// 门槛那句出自旧实现 `tasks.ts:84-89`：
// `marketname === "sign_in_3" && curMonthBackup === false && new Date().getDate() < 20`。
// 三个条件逐字照抄，其中两格要说清楚：
//
//   - `curMonthBackup === false` 是**严格等于** ⇒ "没读到"不挡任务（见 [taskStepInput.BackupKnown]）；
//   - `marketname === "sign_in_3"` ⇒ 这道门**只管云盘那侧**，邮箱那侧的每月任务照做。
//     为什么上游这么区分**未验证**（备份开关看起来是账号级的），但它是旧实现的
//     实际行为 ⇒ 照抄，不"顺手改成两侧都挡"。
//   - ⚠ **20 这个数没有任何成文解释**（旧 TS 里它就是个字面量）。判错方向的代价
//     不对称：把"该跳过"读成"不跳过" ⇒ 对一个没开备份的号发一整月都完不成的任务；
//     反过来只是少做几条。
func (in taskStepInput) backupGateBlocks(marketname string) bool {
	if marketname != upstream.MarketName {
		return false
	}
	return in.BackupKnown && !in.BackupOn && in.DayOfMonth > 0 && in.DayOfMonth < 20
}

// taskClickKeyDefault 是通用盲点打卡那一带的 key（实拍值，见 `app_task_click.go` 上表）。
const taskClickKeyDefault = upstream.TaskClickKeyDefault

// taskIDOf 取任务 id；没有 id 的条目交回 ok=false（旧实现读的是 `task.id`，
// 上游哪天给一条没有 id 的，它会把 `undefined` 拼进 URL ⇒ 我们不发这一枪）。
func taskIDOf(item upstream.TaskV2Item) (int64, bool) {
	if item.TaskID == nil {
		return 0, false
	}
	return *item.TaskID, true
}

func taskGroupOf(item upstream.TaskV2Item) string {
	if item.GroupID == nil {
		return ""
	}
	return *item.GroupID
}

func taskNameOf(item upstream.TaskV2Item) string {
	if item.Name == nil {
		return ""
	}
	return *item.Name
}

// taskMarketOf 取条目上的 `marketname`（旧实现那道 month 门读的就是这一格，
// **不是**我们请求时用的那个市场名 —— 两者在实拍里一致，但上游给什么就读什么）。
// 缺席时交回空串：那不等于 `sign_in_3` ⇒ 门**不挡**（与旧实现 `undefined` 同向）。
func taskMarketOf(item upstream.TaskV2Item) string {
	if item.MarketName == nil {
		return ""
	}
	return *item.MarketName
}

func taskCurrStepOf(item upstream.TaskV2Item) int64 {
	if item.CurrStep == nil {
		return 0
	}
	return *item.CurrStep
}

// taskRunnerSpec 是一条专属 runner 的**外形**：它要不要先打卡、用什么 key。
//
// 为什么要把它做成数据而不是闭包：判据（"409 要先打卡、用 task2"）要能在
// 没有网络的情况下被钉住；真正的动作在 `task_matrix_run.go` 按名字分派。
type taskRunnerSpec struct {
	// Name 是 runner 名（进结论；也是执行层查表的键）。
	Name string
	// OpensEntry 为真 ⇒ 先打一发 `mrp …/page/doTask`（**零实拍**，见 `app_task_click.go` 文件头）。
	OpensEntry bool
	// FinalKey 是 runner 自己那一发打卡的 `key`；空串 ⇒ 不打这一发。
	FinalKey string
}

// taskMatrixRunners 是**本仓已经接上**的专属玩法表。
//
// ⚠ 表里只有两条，而旧实现有十四条 —— 其余十二条在 [taskMatrixToolGaps]。
// 这两条是"整条链今天真的能跑完"的：入口上报（**零实拍**，见 `app_task_click.go` 文件头）
// + 一发打卡（本族那一发只有**两发**实拍，都在 `m.mcloud.139.com`；另两发同名端点在
// `:7071`，属 notice 与分享码那两族 —— 别把它们算成这一发的样本）。
//
// ⚠ 旧实现在跑 runner **之前**还有一发通用打卡（`handleTaskClick` 里那句
// `clickTaskWithStep`，key 恒 `task`、受 `currstep == 0` 门约束）。
// 那一发不在本结构里 —— 它是"所有 runner 共有的前置"，由 [PlanTaskStep] 填进
// [TaskStepPlan.ClickKey]。
var taskMatrixRunners = map[int64]taskRunnerSpec{
	// 旧实现 `runners.ts:104-115`：`click(task)` → `mrp doTask` → 等 500ms → `click(task2)`。
	409: {Name: "云朵中心入口", OpensEntry: true, FinalKey: upstream.TaskClickKeySecond},
	// 旧实现 `runners.ts:124-135`：同一形状，只是最后一发仍是 `key=task`
	// ⇒ 同一条任务在一趟里被打卡**两次**（前置那一发 + 这一发）。照抄：
	// 改成一次是另一种行为，而我们读不到上游对"重复打卡"的处置。
	1005: {Name: "云朵中心入口(新)", OpensEntry: true, FinalKey: taskClickKeyDefault},
}

// taskMatrixToolGaps 是"旧实现有专属玩法、本仓还接不上"的那批，**逐条写明缺哪一族**。
//
// ⚠ 这张表的价值就在**理由文本**上：它直接进结论与界面，
// 用户看一眼就知道"下次要补的是上传还是邮箱"。空话（"未实现"）等于没写。
var taskMatrixToolGaps = map[int64]string{
	106:  "上传文件要网盘那三枪（file/create → PUT → file/complete），本仓未实现且 PUT/complete 零实拍（待拍板 P1）",
	113:  "PC 客户端上传 = 同一族三枪 + PC 端 channelSrc，三枪未实现（P1）",
	522:  "循环上传到 100 个 = 同一族三枪，且进度字段 `process` 的口径未验证，三枪未实现（P1）",
	107:  "创建云笔记要笔记服务那一族（note 鉴权 + 建 + 删），本仓未实现",
	434:  "分享文件要外链那一族（getOutLink / delOutLink），本仓未实现",
	472:  "体验 139 邮箱要邮箱会话那一族（autologin + PE 活跃上报 + 邮箱市场 click），本仓未实现",
	473:  "同 472：邮箱会话那一族未实现",
	1004: "给好友发邮件要邮箱会话 + 写信那一族，本仓未实现",
	1014: "PDF 转换 = 上传那一族 + 邮箱侧转换接口，两者都未实现（P1）",
	1020: "邮箱资产备份要邮箱会话那一族，本仓未实现",
	1028: "AI 工作台要邮箱会话 + DES/RSA 派生 + SSE 对话那一族，本仓未实现",
	585:  "AI 相机的玩法链本仓未实现",
}

// PlanTaskStep 决定对一条任务做什么。
//
// ⚠ 它**不判**"这条要不要做"（`state`/`enable` 那道门在 [TaskMatrix.Pending]），
// 进来的应当已经是"待做"的条目。
func PlanTaskStep(item upstream.TaskV2Item, in taskStepInput) TaskStepPlan {
	id, ok := taskIDOf(item)
	if !ok {
		return TaskStepPlan{Kind: StepBlockedByRule, Reason: "上游没给 id，无法上报"}
	}
	group := taskGroupOf(item)

	// 旧实现的 `month` 分支只有这一道门，**没有 break** ⇒ 过完门继续走 default。
	if group == "month" && in.backupGateBlocks(taskMarketOf(item)) {
		return TaskStepPlan{Kind: StepNeedsHuman, Reason: "每月任务要本月已开启自动备份，而这个号本月没开（旧实现在 20 号前直接跳过这一组）"}
	}

	if spec, has := taskMatrixRunners[id]; has {
		plan := TaskStepPlan{Kind: StepRunner, Runner: spec.Name, Paced: true}
		key, click := taskClickGate(id, item, taskClickKeyDefault)
		if !click {
			// 旧实现这一格**不打卡**，但 runner 照样跑（`clickTaskWithStep` 直接回 true）。
			plan.Reason = "currstep 不为 0 ⇒ 按旧实现的门不发前置那一发，只跑玩法"
		} else {
			plan.ClickKey = key
		}
		return plan
	}

	if reason, gap := taskMatrixToolGaps[id]; gap {
		return TaskStepPlan{Kind: StepBlockedToolGap, Reason: reason}
	}

	if group == "beiyong1" {
		return stepForBeiyong1(id, item)
	}

	// default 分支：没有专属玩法 ⇒ 盲点一发，除非命中那张"不发这一枪"的表。
	if taskMatrixSkipsClick(id) {
		return TaskStepPlan{Kind: StepBlockedByRule,
			Reason: "在旧实现的 SKIP_TASK_LIST 里 ⇒ 那一发盲点打卡不发"}
	}
	return TaskStepPlan{Kind: StepClick, ClickKey: taskClickKeyDefault}
}

// stepForBeiyong1 是 `case "beiyong1"` 那一支。
//
// 它与 default 的**关键差别**：这一支的打卡要过 `currstep == 0` 那道门
// （因为它走的是 `handleTaskClick`），而 default 的盲点**不过门**。
// 把两边混成一个的表现：给实拍那三条 `currstep` 为 4/6/8 的条目各发一发打卡
// —— 上游记成"做了"，而我们一件都没做。
func stepForBeiyong1(id int64, item upstream.TaskV2Item) TaskStepPlan {
	plan := TaskStepPlan{}
	if reason, gap := taskMatrixToolGaps[id]; gap {
		plan.ToolGap = reason
	}
	if name := taskNameOf(item); uploadNamedTask(name) {
		plan.ToolGap = "任务名写着上传（" + name + "）：" +
			"旧实现打卡后还要真往网盘传一个文件，那一族三枪本仓未实现（P1）"
	}
	key, click := taskClickGate(id, item, taskClickKeyDefault)
	if !click {
		plan.Kind = StepBlockedByRule
		plan.Reason = "beiyong1 这一组的打卡门是 currstep == 0，本条是 " +
			strconv.FormatInt(taskCurrStepOf(item), 10)
		if plan.ToolGap != "" {
			plan.Reason += "；" + plan.ToolGap
		}
		return plan
	}
	plan.Kind = StepClick
	plan.ClickKey = key
	plan.Paced = true // 这一支走的是 `handleTaskClick`，旧实现在它末尾 sleep
	return plan
}

// uploadNamedTask 对应旧实现那句 `name.includes("上传") && (name.includes("图") || name.includes("照"))`。
//
// ⚠ 同一支里旧实现还有一句"看 `button.app.link` 含 `uploadView`"。本仓的解析器
// **不读** `button`（它是渲染字段，见 `app_task_list.go` 里那六个字段的取舍）⇒
// 那一句今天**不可能触发**。登记在此而不是"顺手补个解析"：要补得先确认
// 上游给不给这一格、给的是什么形状（一次实拍都没有）。
func uploadNamedTask(name string) bool {
	return strings.Contains(name, "上传") &&
		(strings.Contains(name, "图") || strings.Contains(name, "照"))
}

// taskClickGate 是 `clickTaskWithStep`：默认只在 `currstep == 0` 时打卡，
// 唯一的例外是 434 在 `currstep == 22` 这一格。
//
// ⚠ 这两个数（`22` 与"434"）**没有任何成文解释**，是旧实现的字面量
// （`specialMap: Record<number, number> = { 434: 22 }`）。
// 而 434 在 [taskMatrixToolGaps] 里 ⇒ 今天走不到这一格；它留在这里是为了
// 接上分享那一族时不再翻旧源码，并被下面的测试钉住"它确实是个例外"。
func taskClickGate(id int64, item upstream.TaskV2Item, key string) (string, bool) {
	step := taskCurrStepOf(item)
	if step == 0 {
		return key, true
	}
	if id == 434 && step == 22 {
		return key, true
	}
	return key, false
}

// taskMatrixSkipsClick 查那张"不发盲点打卡"的表（旧实现 `SKIP_TASK_LIST`）。
func taskMatrixSkipsClick(id int64) bool {
	for _, blocked := range TaskMatrixNeverClick {
		if blocked == id {
			return true
		}
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// 复查那一趟（旧实现 `index.ts:44-67`）
// ─────────────────────────────────────────────────────────────────────────────

// PlanVerifyClick 是复查趟对个别任务**补**的那一发打卡。
//
// 旧实现只有两条：481 在 `currstep == 2` 时、478 在状态不是 FINISH 时，
// 各用 `randomCloudTask` 这一枚 key 补一发。
// ⚠ 这一枚 key **零实拍**，而且这两个 id 既不在 runner 表也不在 SKIP 表 ⇒
// 它们在**第一趟**就已经被盲点过一次了。这里照旧实现补第二发，
// 并在结论里把它标成"补打卡（零实拍）"，是为了让对账时能一眼看见这一枪。
func PlanVerifyClick(item upstream.TaskV2Item) (string, bool) {
	id, ok := taskIDOf(item)
	if !ok {
		return "", false
	}
	switch id {
	case 481:
		if taskCurrStepOf(item) == 2 {
			return upstream.TaskClickKeyRandomCloud, true
		}
	case 478:
		if !taskFinished(item) {
			return upstream.TaskClickKeyRandomCloud, true
		}
	}
	return "", false
}

// TaskVerifyClass 是"复查那一眼看到的东西"。
type TaskVerifyClass int

const (
	// VerifyDoneNow 是"这趟做了、复查也显示完成" —— 唯一能算成功的一档。
	VerifyDoneNow TaskVerifyClass = iota
	// VerifyFailed 是"这趟做了、复查仍未完成" —— 旧实现那句"失败"。
	VerifyFailed
	// VerifyAlreadyWas 是"这趟没做、但已经完成了"（往日或别处做的）⇒ 只报告，不算成绩。
	VerifyAlreadyWas
	// VerifyStillOpen 是"这趟没做、也没完成" ⇒ 旧实现那句"需前往云盘/邮箱完成"。
	VerifyStillOpen
	// VerifyHidden 是 `hidden` / `hiddenabc` 两组：旧实现**不写进报告**，这里也照它。
	VerifyHidden
)

// String 进结论句。
func (c TaskVerifyClass) String() string {
	switch c {
	case VerifyDoneNow:
		return "本次做完"
	case VerifyFailed:
		return "做了但上游未认"
	case VerifyAlreadyWas:
		return "此前已完成"
	case VerifyStillOpen:
		return "仍未完成"
	case VerifyHidden:
		return "隐藏组"
	}
	return "未知"
}

// ClassifyVerifiedTask 是旧实现 `collectTaskResult` 那四格。
//
// `done` 是这趟**真的发过动作**的 id 集合。
//
// ⚠ 与旧实现有一处**刻意不同**：它对 `clickTaskWithStep` 回了 true 却什么都没发
// 的情况（`currstep != 0`）也把 id 记进 `doingList` ⇒ 复查时会被念成"本次成功"。
// 本仓的 `done` 只记真发过枪的（见 `task_matrix_run.go`），所以那一档在这里落到
// [VerifyStillOpen]。这不影响上游行为，只影响"界面说这趟做了几条"的可信度。
func ClassifyVerifiedTask(item upstream.TaskV2Item, done map[int64]bool) TaskVerifyClass {
	if group := taskGroupOf(item); group == "hidden" || group == "hiddenabc" {
		return VerifyHidden
	}
	id, ok := taskIDOf(item)
	finished := taskFinished(item)
	if !ok {
		if finished {
			return VerifyAlreadyWas
		}
		return VerifyStillOpen
	}
	if done[id] {
		if finished {
			return VerifyDoneNow
		}
		return VerifyFailed
	}
	if finished {
		return VerifyAlreadyWas
	}
	return VerifyStillOpen
}

// taskFinished 读 `state == FINISH`（逐字，实拍那两档之一）。
func taskFinished(item upstream.TaskV2Item) bool {
	return item.State != nil && *item.State == upstream.TaskStateFinish
}

// VerifyMarketLabel 给出"该去哪个 App 做"（旧实现按 `marketname` 分"邮箱 / 云盘"）。
func VerifyMarketLabel(item upstream.TaskV2Item) string {
	if item.MarketName != nil && *item.MarketName == upstream.TaskListV2EmailMarketName {
		return "邮箱"
	}
	return "云盘"
}

// shareProgressRe 对应旧实现那句 `/(\d+)\/7/`。
var shareProgressRe = regexp.MustCompile(`(\d+)/7`)

// ShareProgress 从任务名里读"分享第几天"（434 那一族是多日任务，旧实现靠它报进度）。
//
// ⚠ 分母 `7` 是**字面量**，没有任何成文说明它跟奖品档数有没有关系。
func ShareProgress(name string) (int, bool) {
	groups := shareProgressRe.FindStringSubmatch(name)
	if groups == nil {
		return 0, false
	}
	n, err := strconv.Atoi(groups[1])
	if err != nil {
		return 0, false
	}
	return n, true
}
