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
	// Upload 非 nil ⇒ 这一条除了（可能有的）打卡之外，还要**往网盘传文件**（**写**）。
	//
	// 两个来源：① [StepRunner] 从 [taskRunnerSpec.Upload] 抄过来（106 / 522）；
	// ② beiyong1 那一支按**任务名**决定（旧实现 `tasks.ts:73-77`）。
	//
	// ⚠ 为什么由分派层**抄一份**而不是让执行层自己去查 runner 表：
	// 那样"这一条要做什么"就有两个真相源，而 beiyong1 那一支**根本没有 runner**
	// （它是按名字判的），执行层必须能只凭 plan 就把两路都跑对。
	Upload *taskUploadShape
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

// taskProcessOf 读条目自己的 `process`；缺席当 0。
//
// ⚠ "当 0"不是偷懒，是照抄旧实现那句 `task.process >= 100`：JS 里 `undefined >= 100`
// 为 **false** ⇒ 上游没给那一格时旧实现**不会**停。反向（没给就当满 100）的表现是
// 一条都不传，而界面上说的是"上传 0 个"。
//
// ⚠ 这一格是**语义未验证**的读数（R15：实拍三条恒 0 而同批条目全 `FINISH`），
// 拿它当停表之所以安全，是因为真正兜底的是 [taskUploadShape.Rounds] 那个上限。
func taskProcessOf(item upstream.TaskV2Item) int64 {
	if item.Process == nil {
		return 0
	}
	return *item.Process
}

// taskUploadShape 是"这一条的第二半是往网盘传文件"的**参数集**。
//
// 做成数据而不是闭包，与 [taskRunnerSpec] 同一条理由：判据要能在没有网络的情况下被钉住。
// 零值不成一档 —— 所以"要不要传"用 `*taskUploadShape` 是否为 nil 表达，这里不设 `None`。
type taskUploadShape struct {
	// Rounds 是**最多**传几个（旧 522 那句 `每月上传任务单日数量 ?? 5`）。
	//
	// ⚠ 本仓没有"每月上传任务单日数量"这一格配置（全仓 grep 零命中）⇒ 取旧实现的
	// **默认值 5**，钉成 [taskMatrixUploadRoundsPerRun]。要开成参数面得连
	// `docs/cli-usage.md` 与 WebUI 一起改，属 Q32 那一档，不在这里单方面决定。
	Rounds int
	// StopAtProcess > 0 ⇒ 每传一个之前先看条目自己的 `process`，达到就停（旧 522 的 100）。
	//
	// ⚠ 这一格读的是**语义未验证**的 `process`（R15：实拍三条恒 0 而同批条目全 `FINISH`）。
	// 用它的理由不是"它靠谱"，是"旧实现读它"；而**上限 [Rounds] 挡住了最坏情况** ——
	// 就算 `process` 永远读到 0，这一趟也最多传 5 个，不会变成"连传 100 个"。
	StopAtProcess int64
	// Channel 空 ⇒ 安卓档（旧 `uploadRandomFile` 不带 channelSrc 时的默认）。
	Channel upstream.CloudChannel
	// Op 空 ⇒ upload。⚠ 只有安卓档读它（`x-yun-op-type` 的 2 只在备份档出现）。
	Op upstream.CloudOpType
	// Ext 空 ⇒ `.png`（旧实现那句 `options.ext || ".png"`）。
	Ext string
	// PaceEachRound 为真 ⇒ **每传完一个**等一拍（旧 522 循环体末尾那句 `sleep(500)`，
	// 包括最后一轮 ⇒ 与 [TaskStepPlan.Paced] 那一拍是**两次**等待，照抄）。
	//
	// 106 与 beiyong1 那一支的 runner 体里**没有**这一句 ⇒ 假。把它们混成一个
	// 的表现：要么 522 变成"连传 5 个零间隔"，要么 106 白等一拍 —— 都不会报错，
	// 只在"哪天被判成脚本"时最难查的那一类差异上。
	PaceEachRound bool
}

// taskMatrixUploadRoundsPerRun 是 522 那一趟最多传几个（旧实现的 `?? 5` 那一档）。
const taskMatrixUploadRoundsPerRun = 5

// taskUploadOne 是"就传一个"那一档（106 与 beiyong1 的名字门共用它）。
//
// 单独一个函数是为了让"106 与 beiyong1 走的是同一档形状"这件事可断言
// （旧实现两处都是裸的 `uploadRandomFile(ctx)`，没有任何参数差别）。
func taskUploadOne() *taskUploadShape {
	return &taskUploadShape{Rounds: 1}
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
	// Upload 非 nil ⇒ 这条 runner 的**最后一件事**是往网盘传文件（**写**：会在用户
	// 网盘里留下真文件）。旧实现 `106: uploadRandomFile(ctx)` /
	// `522: 循环 uploadRandomFile` 那两条的形状。
	Upload *taskUploadShape
	// Shares 为真 ⇒ 这条 runner 的**本体**是"把这一趟传上去的第一个文件分享出去、
	// 再立刻取消分享"（**两次写**：建一条公开外链 + 删掉它）。
	//
	// 它与 [Upload] 的正相反：那一格**生产**文件，这一格**消费**别人生产的文件
	// （旧实现 `runners.ts:69` 读的就是 `ctx.state.files`）。所以它有一个
	// **旧实现自带的脆弱点**：同一趟里排在它前面的那些上传任务必须真的传上来过
	// 东西，否则它没有可分享的文件（今天的处置见执行层 `runShare`）。
	// ⚠ 2026-09-25 真跑两趟：两趟**都正好落在这一个脆弱点上**（待做表按 id 升序，
	// 当天能上传的那几条 id 都比 434 大）⇒ 两口仍零真打。要不要给它造一次可分享的
	// 文件是设计决定，未拍板 ⇒ `docs/pending-decisions.md` **Q41**。
	Shares bool
	// Notes 为真 ⇒ 这条 runner 的**本体**是"在云笔记里建一条随机笔记、再把它移进回收站"
	// （**两次写**，形状与 [Shares] 同：建了又删）。
	//
	// 它是这一族**唯一**的清理手段：本仓没有"物删回收站笔记"那一口（零实现、零实拍）
	// ⇒ 产物会留在用户的笔记回收站里，**进不了** [TaskMatrixRunResult.CloudFileIDs]
	// 那条网盘清理链。这一格是已知残留，登记在 `docs/pending-decisions.md` **R32**。
	Notes bool
	// Mail 为真 ⇒ 这条 runner 的**本体**是"换一张 139 邮箱会话票（`toSourceId=001003`）
	// → 用它活跃一次 → 再打一发**邮箱市场**的 `taskClickV2`"，四枪逐处取舍写在
	// `internal/upstream/app_mail139.go` 文件头。
	//
	// 它与上面几格的分别：**这一条不往用户账号里留产物**（不传文件、不建笔记、不建外链），
	// 但它是这一张表里**唯一一条跨两台服务**的：前三枪打的是 139 邮箱自己的
	// （`user-njs.yun.139.com` 换票 + `mail.10086.cn` 两发），只有最后一发回到彩云 `:7071`。
	// ⇒ "邮箱那三枪失败"与"任务没被上游认"是两种不同的失败，执行层分开念。
	Mail bool
	// MailBackup 为真 ⇒ 这条 runner 的**本体**是"换一张邮箱会话（①②）→ ⑧ 备份那一发"
	// （玩法 1020）。它与 [Mail] 的分别是**这一条不打 ③、也不打 ④**：旧实现那一条
	// runner 只有那两步（`runners.ts:160-165`），而 1020 的产物本身就是那一发请求。
	// ⚠ ⑧ 是**写**（请求"把这个号的备份跑一次"），且本仓**不判**它的响应 ⇒
	// 播报念"已提交"，不念"已完成"。
	MailBackup bool
	// MailPDF 为真 ⇒ 这条 runner 的**本体**是"换一张邮箱会话（①②）→ **往网盘传一份
	// 最小 PDF** → ⑥ 把那一份转成 PDF"（玩法 1014）。
	//
	// 它是这一张表里**唯一一条两服务接力**的：写盘那一半用网盘那三枪（与 106 / 522 同一族，
	// 产物进 [taskExecutor.cloudFileIDs] 那条清理链），转换那一半打的是邮箱侧的 ⑥。
	// ⚠ **转换产物**不在清理链上：⑥ 零实拍、它给不给新 fileId 与落在哪个目录都不知道
	// ⇒ 这一格登记为残留问句（`docs/pending-decisions.md` R36），别读成"跑完没留东西"。
	MailPDF bool
	// AICamera 为真 ⇒ 这条 runner 的**本体**是"① 识图（`aiRecognize`）→ ② 拿它给的
	// fileId 问一句（`chat/v2/add`，回的是 SSE）"（玩法 585，旧实现 `ai-tasks.ts`）。
	//
	// 三个"与上面几格不同"都在这里，不在执行层：
	//   - **两枪同一主机、同一枚凭据**：①② 都带库里那串 Basic 整串（实拍逐字相同），
	//     而它**不是票**。旧实现两处各取 `ctx.config.token` / `ctx.config.auth`，
	//     它自己那两枚就不相同 ⇒ 这是本条最容易接错的一格，逐处取舍写在
	//     `internal/app/task_matrix_play_ai.go` 文件头。
	//   - **它是本表两条"响应不是一段 JSON"里的第一条**（另一条是下面的 [Workbench]）：
	//     ② 回 SSE，判据是旧实现那三档回退，
	//     所以它的"成功"必须连**命中哪一档**一起念（协议层
	//     `internal/upstream/app_play_ai_chat_sse.go` 文件头那条"一段报错页也算成功"）。
	//   - **它写盘、但产物不是我们传上去的**：① 的 `uploadToCloud` 实拍恒 true ⇒
	//     一张白图落在用户网盘里，`data.fileId` 进 [taskExecutor.cloudFileIDs] 那条清理链。
	//     ⚠ 与 [MailPDF] 同一类残留：① 说成功却没回 fileId 时**删不掉**（登记 R37）。
	AICamera bool
	// Workbench 为真 ⇒ 这条 runner 的**本体**是"换一张邮箱会话（①②）→ ①′ 派生 AI 域
	// 凭据（DES）→ ②′ 查前置状态（RSA，**只读**）→ ③′ 问一句（SSE）→ 等 5 秒 →
	// ④′ 再看一眼同一口"（玩法 1028，旧实现 `runners.ts:345-448`）。
	//
	// 三个"与上面几格不同"都在这里，不在执行层：
	//   - **它是本表唯一一条要自己解密上游返回值的**：①′ 把上游给的 DES 密文解成手机号，
	//     再用它拼 Basic 与 RSA 输入 ⇒ 凭据链不是"库里那一枚"，而是**这一趟现换的**。
	//     逐处口径与"手机号/密文一律不进日志"写在 `internal/app/task_matrix_workbench.go`
	//     与 `internal/upstream/app_ai_workbench.go` 文件头。
	//   - **它的 `userId` 只来自 ①′ 那一枪**（旧实现那句 `|| ""` ⇒ 取不到就直接放弃对话），
	//     而 585 那一族用的是库里那份 `x-yun-uni` —— ⚠ 两者同不同源**未测试**，
	//     所以两条各自取自己出处那一枚，不许合并（见 `task_matrix_workbench.go` 函数头）。
	//   - **它不写网盘**（⇒ 不在清理链上），但会在 AI 工作台那边留下一次会话记录，
	//     而本仓没有"删会话"那一口。
	//
	// ⚠ 它**没有** `FinalKey` 也没有 `Mail` 那一发市场打卡：旧实现这一条runner 后面
	// 不再打本族 `task/click` ⇒ 通用前置那一发仍由 [PlanTaskStep] 按 `currstep` 门决定。
	Workbench bool
}

// taskMatrixRunners 是**本仓已经接上**的专属玩法表。
//
// ⚠ 表里**十二条**，而旧实现有十四条 —— 其余两条在 [taskMatrixToolGaps]
// （113 / 1004）。第十二条（1028）于 2026-09-26 随 `app_ai_workbench.go` 接上，
// ⭐ **同日四枪都拿到了真实读数**（活体探针三趟，`docs/capture/app-ai-workbench-samples.txt`）
// ⇒ 这一条不再是"只到形状"；它剩下的那一格是"上游三格回字符串 ⇒ 那道门永不成立，
// 要不要放宽"（**R38**ⓐ），是本表**唯一一条由用户拍板**的判据取舍。
// 前两条（409 / 1005）是"整条链今天真的能跑完"的：入口上报（**零实拍**，见
// `app_task_click.go` 文件头）+ 一发打卡（本族那一发只有**两发**实拍，都在
// `m.mcloud.139.com`；另两发同名端点在 `:7071`，属 notice 与分享码那两族 ——
// 别把它们算成这一发的样本）。
//
// 中间两条（106 / 522）是 2026-09-24 随上传族接上的，⚠ 它们**会往用户网盘写文件**，
// 而那三枪**零实拍**（R27）+ 同一台主机的鉴权画像未定（R28）⇒ 今天能不能真跑通**没有结论**，
// 只是"形状与顺序按旧实现落好了"。这一族挂在 `app-task` 那条能力上，而它
// `RunInBatch:false`（调度与批量都不替它跑，只有界面上点那一下才会真发）。
//
// 最后一条（434）是 2026-09-25 随外链那一族接上的，形状与上面几条都不同：它**不生产**
// 文件、只消费这一趟前面那些上传任务产出的文件（见 [taskRunnerSpec.Shares]）。
// 它的两口同样**零 REST 实拍**（成文见 `internal/upstream/app_cloud_outlink.go` 文件头）。
//
// 107（云笔记）与 434 同族（建了又删），但它是这一张表里**唯一一条"旧实现从没跑到过"**
// 的玩法：旧 TS 那个判据恒不成立 ⇒ 后两枪一枪都没发过（`docs/business-feature-inventory.md:383`
// 那句"实测 bug"）。所以它的形状取自**在跑的** Go 参考实现，而不是旧 TS ——
// 三口逐字与两处取舍写在 `internal/upstream/app_note.go` 文件头，⚠ 也是**零 REST 实拍**。
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
	// 旧实现 `runners.ts:41-44`：runner 只做一件事 —— `uploadRandomFile(ctx)`，
	// **不带任何参数** ⇒ 根目录 + 安卓档 + `.png` + 一个。它**没有** `FinalKey`：
	// 玩法自己不再打一发（前置那一发由 [PlanTaskStep] 按门决定）。
	106: {Name: "上传一个随机文件", Upload: taskUploadOne()},
	// 旧实现 `runners.ts:117-125`：`for i < (config.tasks?.每月上传任务单日数量 ?? 5)`，
	// 每轮先看 `task.process >= 100` 就 break，传一个、等 500ms。
	// ⚠ 旧实现那个循环**没有 break 之外的停止条件**：`process` 不涨它就固定传满 5 个
	// （上限就是那格配置的默认值）。这里保持同一形状（[taskUploadShape.Rounds] 是上限，
	// [taskUploadShape.StopAtProcess] 是那一格读数）。
	// ⚠ `PaceEachRound` 是它**与 106 唯一的形状差别**（旧实现循环体里多那句 sleep）。
	522: {
		Name: "循环上传凑满 100 个",
		Upload: &taskUploadShape{
			Rounds: taskMatrixUploadRoundsPerRun, StopAtProcess: 100, PaceEachRound: true,
		},
	},
	// 旧实现 `runners.ts:68-88`：取 `ctx.state.files` 的**第一个**建一条外链，
	// 上游给了 `linkID` 就**立刻**再发一口把它删掉。
	//
	// 它**没有** `FinalKey`（runner 自己不再打一发）、也**没有** `Upload`（它不传文件）。
	// 前置那一发仍由 [PlanTaskStep] 按门决定，而 434 恰好是那道门**唯一有例外**的一条
	// （`currstep == 22` 也打，见 [taskClickGate]）⇒ 它是那张例外表今天唯一能走到的入口。
	434: {Name: "分享一个文件并立刻取消", Shares: true},
	// 旧实现 `runners.ts:47-66` + `apis/note.ts`：换笔记凭据 → 建一条随机笔记 →
	// 等 2s → 把它移进回收站。形状与 434 同族（建了又删），差别在**删不到底**：
	// 这一族只进回收站，本仓没有物删那一口（R32）。
	//
	// ⚠ 这条**不是**"照旧实现落好了"那么简单：旧 TS 那一条 runner **从来没有真的发过
	// 后两枪** —— 它读 `http.post` 返回值的 `.headers`，而 ofetch 直接返回解析后的响应体
	// ⇒ 那个判据恒不成立，每次都走"跳过云笔记任务"那一支（盘点里的"实测 bug"就是这句，
	// `docs/business-feature-inventory.md:383`）。所以这一条的**判据源是在跑的 Go 参考实现**
	// （caiyun `caiyun_note.go` + `handleNoteTask`），不是旧 TS；逐处差异写在
	// `internal/upstream/app_note.go` 文件头。
	//
	// ⚠ 它同样**没有** `FinalKey`（旧 TS 那一条 runner 自己不再打一发；它在恒跳过的
	// 那段代码之后，从未执行过 ⇒ 那一发到底存不存在**没有证据**，故不发）。
	107: {Name: "创建并删除一个云笔记", Notes: true},
	// 旧实现 `runners.ts:132-151`：472 与 473 **共用同一个 runner**，只在最后一发
	// 市场 click 的 `taskId` 上分开（472 打 472、473 打 473）⇒ 这里是两格同样的开关。
	//
	// ⚠ 它并**不**做 PE 活跃上报：旧实现那句 `triggerMailPEvent` 只被 1004 用到
	// （发邮件成功之后那一档），本仓早前的 gap 理由把 PE 算在 472 头上是**读错了出处**。
	//
	// ⚠ 这一条**没有** `FinalKey`：runner 自己那一发是**邮箱市场**的 `taskClickV2`
	// （另一台主机、另一个 `marketname` 语义），由 [taskRunnerSpec.Mail] 那一支发，
	// 不是本族 `task/click` 那一发。通用前置那一发仍由 [PlanTaskStep] 按 `currstep` 门决定。
	472: {Name: "体验 139 邮箱", Mail: true},
	473: {Name: "体验 139 邮箱", Mail: true},
	// 旧实现 `runners.ts:167-172`：会话（①②）→ 传一份最小 PDF → ⑥ 转换。
	// ⚠ 它**没有** `Upload` 那一格：1014 的上传是**转换的前置**（要的是那一个 fileId，
	// 不是"有过一次上传动作"），内容与文件名都按旧 TS 那一份固定形状 ⇒ 走 [MailPDF]
	// 自己那一条链（见 `runMailPDF`），不与 106 / 522 共档。
	1014: {Name: "PDF 转换（邮箱侧）", MailPDF: true},
	// 旧实现 `runners.ts:160-165`：会话（①②）→ ⑧ 备份那一发。
	// ⚠ 与 472 / 473 同一取向：`FinalKey` 留空 ⇒ 通用打卡那一支一个字都不念，
	// 成败由 [MailBackup] 那一支按"发没发出去 / 非 2xx"分档播报。
	1020: {Name: "邮箱资产备份", MailBackup: true},
	// 旧实现 `packages/core/src/tasks/app/runners.ts:182-185`：runner 本体只有一句
	// `completeAiCameraTask(ctx)` ⇒ 没有 `Upload`、没有 `FinalKey`、也不换任何会话。
	//
	// ⚠ 它**不建会话、不换票**：两枪带的是**同一枚**凭据 —— 库里那串 Basic 整串
	// （不是票）。这一格被两次真跑 + 一条实拍换出来，顺序别记反，全过程成文在
	// `internal/app/task_matrix_play_ai.go` 文件头第一节。
	// 它与下面 1028 是本表**仅有的两条**"响应不是一段 JSON"（② 回 SSE）。
	//
	// ⚠ 2026-09-26 接上：请求侧与读法已于本日落码（`internal/upstream/app_play_ai_chat_sse.go`），
	// 但 ② 的响应**零实拍** ⇒ "接上"仍只到**形状**那一层。真正的缺口（`userId` 与旧实现
	// 不同源、样图前缀比不比、`name` 那一格上游读不读）另有登记处：
	// `docs/pending-decisions.md` **R37** —— 别再把它放回 [taskMatrixToolGaps]。
	585: {Name: "AI 相机：识图后问一句", AICamera: true},
	// 旧实现 `runners.ts:174-180`：先 `getMail139Session(ctx)`（与 472 / 1014 / 1020 共用
	// [mailSessionFor]），再把 `aiWorkbenchTask` 那四步跑一遍 ⇒ 本条**有**邮箱会话那一半。
	//
	// ⚠ `FinalKey` 留空、也没有 `Mail` 那一发市场打卡：旧实现这一条 runner 后面
	// 不再打本族 `task/click`（它"确认任务推进"靠的是链尾那一眼 ④，而那一发**不写**任何东西）。
	//
	// ⚠ 它是本表**凭据链最长**的一条（邮箱会话 → DES 解出的手机号 → Basic + RSA），
	// 也是唯一一条 `userId` **不取自库里**的 —— 两处取舍与"为什么不许和 585 合并"成文在
	// `internal/app/task_matrix_workbench.go` 文件头。
	//
	// ⚠ 2026-09-26 接上，**同日拿到四枪的真实读数**（活体探针三趟，落档
	// `docs/capture/app-ai-workbench-samples.txt`）：①′ 派生凭据通、②′ 的 RSA 自证身份被认
	// （对照枪 585 / 106 / 1020 都回 `result`，1028 自己回 `501 任务不存在`）、③′ 的 SSE 累得出模型原话。
	// ⚠⚠ 最要紧的一格：**上游三格全回字符串 ⇒ 旧实现那道门在真实读数下永不成立**，
	// 本层跟着**不放宽**（放宽等于替用户决定少发一次写）。
	// 缺的那几格（要不要放宽、③′ 原始 SSE 未留存、问话要不要做成配置项、
	// `userDomainId` 与库里那枚同不同源）登记在 `docs/pending-decisions.md` **R38**
	// —— 别再把它放回 [taskMatrixToolGaps]。
	1028: {Name: "AI 工作台：查前置后问一句", Workbench: true},
}

// taskMatrixToolGaps 是"旧实现有专属玩法、本仓还接不上"的那批，**逐条写明缺哪一族**。
//
// ⚠ 这张表的价值就在**理由文本**上：它直接进结论与界面，
// 用户看一眼就知道"下次要补的是上传还是邮箱"。空话（"未实现"）等于没写。
var taskMatrixToolGaps = map[int64]string{
	// ⚠ 113 缺的**不是**上传那一族（PC 档的三枪形状 `CloudChannelPC` 已落码并已接进 runner 表），
	// 而是旧实现那条前置：`runners.ts:110-115` 先 `refreshAuthToken(ctx)`
	// （= `aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`，XML 体、content-type 写 json），
	// 而本仓**刻意不实现那一口**（理由写在 `internal/upstream/auth_refresh.go` 文件头：
	// 零抓包、App 流量里从未出现，两条都未实测时选有实测的那条）。
	// 于是这一条变成一个**真问题**而不是一个待办：旧实现那一发的返回值**被丢弃**
	// （`refreshAuthToken` 交回新 token 而 `113` 没用它）⇒ 它对业务的全部作用就是
	// "上游看到一次 PC 侧的 token 刷新请求"。要不要用本仓已实测的 `user/auth/refreshToken`
	// 顶它（换掉的是另一个服务），**待拍板 Q32**。
	113: "PC 客户端上传：三枪与 PC 档已落码，缺的是旧实现那一发前置 `authTokenRefresh.do`" +
		"（本仓刻意不实现那一个服务，见 `internal/upstream/auth_refresh.go` 文件头）⇒ 用哪一口顶它待拍板 Q32",
	// 107（云笔记）在 [taskMatrixRunners]：三口已于 2026-09-25 落码
	// （`internal/upstream/app_note.go`）。⚠ 但"接上"只到**形状**那一层：这一族三口
	// **零 REST 实拍**，而旧 TS 那一条 runner 从未跑到过后两枪 ⇒ 它连"照抄"都算不上。
	// 别再把它放回本表（同上面 113 与下面 434 那两格的写法）。
	//
	// 434（分享文件）在 [taskMatrixRunners]：外链那一族两口已于 2026-09-25 落码
	// （`internal/upstream/app_cloud_outlink.go`）。它今天仍**没有一次真打**，
	// 但"本仓缺端点"这一档已经不成立了 —— 别再把它放回本表。
	// 472 / 473（体验 139 邮箱）在 [taskMatrixRunners]：邮箱会话那一族四枪已于
	// 2026-09-25 落码（`internal/upstream/app_mail139.go`）。⚠ "接上"仍只到**形状**那一层：
	// ② 只有请求面实拍（响应体零样本）、③④ 连请求面都只有旧 TS 那一份。
	// 另外这一族**不需要** PE 活跃上报 —— 早前那格理由把它算在 472 头上是读错出处
	// （`triggerMailPEvent` 只在 1004 那一支被调用），别再把它放回本表。
	1004: "给好友发邮件要「写信那一族」的**第二口** `mbox:compose`（本仓零实现、零实拍）" +
		"⇒ 邮箱会话那一族已落码、取草稿号那一口（⑦ `mbox:getComposeId`）也已落码并在 2026-09-26 " +
		"探针里真拿到过一枚号（`docs/capture/app-mail139-samples.txt` §8），挡的既不是会话也不是 composeId；" +
		"而 compose 那一枪是**真发一封邮件**（旧实现默认发给自己，但收件人是配置项）⇒ 要不要发它待拍板",
	// ⚠ 1014 与 1020 在 2026-09-26 从本表挪进 [taskMatrixRunners]：邮箱资产侧那两口
	// （⑥ `convertPdfFromYunFile`、⑧ `mailsms/sms/backup`）已落码（`app_mail139_assets.go`）。
	// 别再放回本表（同上面 107 / 434 / 472 那几格的写法）—— 它们**缺的已经不再是端点**，
	// 留着会让界面上永远显示"没接玩法"，而真正的缺口（那两口零真实反馈）另有登记处：
	// `docs/pending-decisions.md` R35 / R36。
	// ⚠ 1028（AI 工作台）在 2026-09-26 从本表挪进 [taskMatrixRunners]：四枪与那两处派生
	// （DES 解手机号、RSA 加密身份）+ 本族自己的 SSE 累加器都已落码
	// （`internal/upstream/app_ai_workbench.go` + `internal/app/task_matrix_workbench.go`）。
	// 别再放回本表（同上面 107 / 434 / 472 / 1014 / 585 那几格的写法）——
	// 留着会让界面上永远显示"没接玩法"，而它真正的缺口另有登记处：
	// `docs/pending-decisions.md` 本族那一格（四枪已于 2026-09-26 活体探针三趟拿到真实读数，
	// 落档 `docs/capture/app-ai-workbench-samples.txt` ⇒ 剩下的两格是"那道门在真实读数下
	// **永不成立**要不要放宽"与"③′ 的原始 SSE 未留存"；另：`aiChatMessage` 那一格配置
	// 本仓仍没有，今天问的是出处那个兜底常量）。
	// ⚠ 585（AI 相机）在 2026-09-26 从本表挪进 [taskMatrixRunners]：两枪（① `aiRecognize`、
	// ② `chat/v2/add`）与那三档 SSE 读法都已落码（`app_play_ai_chat_sse.go` +
	// `task_matrix_play_ai.go`）。别再放回本表（同上面 107 / 434 / 472 / 1014 那几格的写法）——
	// 留着会让界面上永远显示"没接玩法"，而它真正的缺口（② 响应零实拍、`userId` 与旧实现
	// 不同源）另有登记处：`docs/pending-decisions.md` R37。
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
		plan := TaskStepPlan{Kind: StepRunner, Runner: spec.Name, Paced: true, Upload: spec.Upload}
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
// （因为它走的是 `handleTaskClick`），而 default 盲点**不过门**。
// 把两边混成一个的表现：给实拍那三条 `currstep` 为 4/6/8 的条目各发一发打卡
// —— 上游记成"做了"，而我们一件都没做。
//
// ⚠ 那一支**还有第二半**（旧实现 `tasks.ts:72-83`）：打卡之后，
// "名字里写着上传 + 图/照"的条目要**真往网盘传一个文件**。
// 这里要说清一处**门只管打卡、不管上传**：旧实现的 `clickTaskWithStep` 在
// `currstep != 0` 时返回的是 `true`（它只是**不点**，不是"这条别再往下走"），
// 于是那句 `uploadRandomFile` 照样执行。所以本函数在 `!click` 那一支
// **仍然**把 Upload 交出去（表现成 [StepBlockedByRule] + 一次真上传），
// 而不是"门挡住 ⇒ 整条什么都不做"。把这两格混成一个的表现：
// `currstep` 非 0 的那些上传任务从此一条都不做，而界面上它们仍显示"按规则跳过"。
func stepForBeiyong1(id int64, item upstream.TaskV2Item) TaskStepPlan {
	plan := TaskStepPlan{}
	if reason, gap := taskMatrixToolGaps[id]; gap {
		plan.ToolGap = reason
	}
	if name := taskNameOf(item); uploadNamedTask(name) {
		// 旧实现这一句是裸的 `uploadRandomFile(ctx)` ⇒ 与 106 同一档形状。
		plan.Upload = taskUploadOne()
		plan.Reason = "任务名写着上传（" + name + "）⇒ 打卡之后按旧实现再传一个文件"
	}
	key, click := taskClickGate(id, item, taskClickKeyDefault)
	if !click {
		plan.Kind = StepBlockedByRule
		gate := "beiyong1 这一组的打卡门是 currstep == 0，本条是 " +
			strconv.FormatInt(taskCurrStepOf(item), 10)
		if plan.Upload != nil {
			gate += "（打卡不发，但上传照做 —— 见函数头那句「门只管打卡」）"
		}
		if plan.ToolGap != "" {
			gate += "；" + plan.ToolGap
		}
		plan.Reason = gate
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
//
// ⚠ **将来接它时别照这一支的"传一个"写**：那一句走的是另一个函数
// `upload10PictureTask`（`tasks/ai-tasks.ts:193-225`）—— 目标是**补满 10 张 `.jpg`**
// （`remaining = max(0, 10 - task.process)`，已是 10 就直接算成），循环里每张之间还
// `sleep(300)`，且"成功数 > 0"才算完成。口径与逐字判据成文在
// `docs/business-feature-inventory.md` §2.4 那一条（含"本仓 `process` 目前只有一个消费者
// = 522 停表"那一格，别以为它没人读）。
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
