package app

// #9「App 任务矩阵」的**执行**那一半：把 `task_matrix_dispatch.go` 的决定变成请求，
// 再把"做完之后上游认不认"复查一遍。
//
// # 这一条链**会写**用户的账
//
// 每一发 `task/click` 都在告诉上游"这条我做完了"，云朵可能当场入账且**不可回滚**
// （没有撤销那一口）。所以：
//
//   - 它挂不挂进调度由注册表那一条的 `RunInBatch` 决定（见 `internal/capabilities/registry.go`）；
//   - "本仓没接玩法"的任务一律**不**补一发打卡 —— 那是拿用户的账谎报进度，
//     见 `task_matrix_dispatch.go` 文件头那条"刻意不同"。
//
// # 两趟读取
//
// 旧实现跑完第一趟会**重新拉一次**那张表（`index.ts:44`），用上游回的状态
// 判"这趟做的几条认了没有"。这一格不是装饰：`task/click` 回的是 `success`，
// 它**不报**这条任务的最终状态 —— 四发实拍没有一条带回 `result`。
// 代价是读取请求数翻倍（16 → 32，再加每做一条一枪打卡）。
// 所以有 [TaskMatrixRun.SkipVerify] 这一格，但它在"真发过枪"时**不省**（理由写在那条注释里）。
//
// # 这一条链还会**往用户的网盘写文件**
//
// 2026-09-24 起，四条玩法里有两条（106 / 522）与 beiyong1 那一支的"名字写着上传"
// 会真的打上传那三枪（`file/create` → `PUT` → `file/complete`），表现是网盘根目录
// 多出 `asign-*.png`。所以这一族比打卡更要紧的两件事：
//
//   - 那三枪**零实拍**（`docs/pending-decisions.md` R27）⇒ 这里只保证"顺序与判据
//     照旧实现"，**不**保证跑得通；跑不通的原文进 [TaskStepRecord.Err]。
//   - 造出来的文件 id 一律交回 [TaskMatrixRunResult.CloudFileIDs]，**包括失败路径上
//     已经占下坑的那一个**（编排层刻意多交，见 `cloud_upload.go` 文件头）。
//     2026-09-25 起链尾**自己删**：那一份名单一次交给 `batchTrash`
//     （[TaskMatrixRun.cleanupCloudFiles]，播报见 [TaskCleanupLines]），结局进
//     [TaskMatrixRunResult.Cleanup] 与小结那一句括号。想留着不看表现（调试 / 要看
//     文件名长什么样）设 [TaskMatrixRun.KeepUploadedFiles]。
//     ⚠ 删除那一枪**同样零实拍**（四枪里最后一枪），而且它是**写**：跑一次真上游就
//     会把这一趟的文件移进回收站（不是彻底删除，去回收站还能捞）。
//
// # 前置那一发挂了 ⇒ **整条玩法都不跑**（照旧实现，但这一格在接上传之前无关紧要）
//
// 旧实现 `handleTaskClick` 的形状是 `if (await clickTaskWithStep(...)) { runner(); push; sleep }`
// ⇒ 前置那一发**返回 false**（发出去了但上游回了失败，或传输层挂了）时，runner 根本不跑。
// 在只有 409 / 1005 那两条时这一格被混掉了也没人疼（少一发入口上报而已），
// 接上上传之后它决定的是"**上游都没接这条任务，还要不要往用户网盘写一个文件**"⇒ 照抄。
//
// ⚠ 两格别混：`currstep != 0` 那一档 `clickTaskWithStep` **返回 true**（它只是不点），
// 所以玩法照跑 —— 那是"门只管打卡、不管玩法"，见 `task_matrix_dispatch.go` 的 [stepForBeiyong1]。
//
// 一处**已知不忠实**且刻意保留的差异：前置挂了时旧实现连 `handleTaskClick` 末尾
// 那一拍 sleep 也不等，我们仍等（[apply] 末尾）。方向是"多等不少发"，不会多打任何一枪。
//
// # 与旧实现不同的三处（各自注释里都有）
//
//  1. `done`（"这趟做了哪些"）只记**真的发过枪**的 id；
//  2. 缺玩法的任务报 [StepBlockedToolGap] 而不是随手打卡；
//  3. 未开备份那道门要的那个状态，本层**自己去读一次** `infoV3`
//     （旧实现读的是 `ctx.state.curMonthBackup`，在那条链上常常没人填 ⇒ 门形同不存在）。

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// appTaskCapabilityCode 是这一族的能力名。
//
// ⚠ 它指回 [capabilities.CodeAppTask] —— code 的唯一成文处在注册表（理由见
// `internal/capabilities/registry.go` 文件头）。
const appTaskCapabilityCode = capabilities.CodeAppTask

// taskMatrixPace 是"发过一枪之后等一会"的那一点间隔（旧实现 `sleep(500)`）。
//
// 它**不是**一个验证过的节流值：旧实现为什么是 500ms 没有出处。
// 保留它的理由是"逐条串行打卡 + 完全不间隔"这件事真实客户端不会做（手点必有间隔），
// 而一旦哪天被判成脚本，最难查的正是这种"看起来最不像原因"的节奏差异。
const taskMatrixPace = 500 * time.Millisecond

// TaskMatrixRun 是这一条链的入口。
type TaskMatrixRun struct {
	SignInWorld

	// Wait 是"等一会"的注入点；nil ⇒ [RealWaiter]（**默认必须真的等**，理由同 [AiAvatarRun.Wait]）。
	Wait Waiter

	// SkipVerify 关掉第二趟复查。
	//
	// ⚠ 只有在**这一趟一枪都没发出去**时关它才是无损的（没东西可复查）。
	// 真发了枪还关复查，结论就退化成"我们发了 N 枪、上游认没认不知道" ——
	// 那是所有未实测写法里最没信息量的一种，所以 [RunAccount] 在发过枪时**无视**这一格。
	SkipVerify bool

	// KeepUploadedFiles 设真 ⇒ 这一趟传上网盘的临时文件**不删**，跑完留在用户网盘里。
	//
	// **零值 = 删**（跑完就移进回收站），这是刻意的反向命名：旧实现那一份
	// （`after.ts:131-137` + `utils/file.ts:113` 的 `deleteFiles`）本来就是默认执行，
	// 而"忘了一个开关"的后果两边不对称 —— 忘记"删"会在**用户的**网盘里留下
	// `asign-*.png`，忘记"保留"只是测试夹具多打一发请求。所以默认删，
	// 这一格只给"我要看着这些文件长什么样"与测试用。
	//
	// ⚠ 它只影响**这一趟自己造出来**的 id（[TaskMatrixRunResult.CloudFileIDs]）：
	// 上游说"同名已存在"（`exist`）的那一枚从来不进这份名单，见 `cloud_upload.go` 文件头。
	KeepUploadedFiles bool
}

// TaskStepRecord 是对**一条任务**做过什么的记录（进结论、界面与排障日志）。
type TaskStepRecord struct {
	// TaskID 是任务 id；上游没给 id 的条目不会有记录（分派层直接挡）。
	TaskID int64
	// Name 是去掉 HTML 标签后的任务名。
	Name string
	// Group 是 `groupid`（`day` / `month` / `beiyong1` …）。
	Group string
	// Kind 是处置类别。
	Kind TaskStepKind
	// Runner 是走专属玩法时**哪一条** runner 的名字（来自 [TaskStepPlan.Runner]）；
	// 不发玩法时为空。结论与界面上"这一条是谁做的"只有这一格能回答。
	Runner string
	// Reason 是"为什么是这个类别"。[StepBlockedToolGap] 与 [StepBlockedByRule] 必填 ——
	// 那两格的全部意义就是**说清楚**。
	Reason string
	// Err 是发出去了但失败时的原文（多条用 `；` 串起来）；全成功时为空。
	//
	// ⚠ 它也收"因为缺前置所以**没**发"的那几条（取不到 deviceId、随机源没接）——
	// 口径与 [runRunner] 里入口上报那一格一致：这里说的是"这一条没做成"，
	// 至于有没有出网由 [TaskStepRecord.Kind] 与计数那几格说。
	Err string
	// Uploads 是这一条**传出去**的文件数（秒传也算，`exist` 不算）。
	// 0 ⇒ 这一条没有上传那一半（它不参与那四格复查，只回答"这一条写了几发文件"）。
	Uploads int
	// UploadNote 是上传那一半里**不算失败**的补充：按 `process` 提前停了、
	// 上游说"已存在"所以没传。真失败的原文进 [TaskStepRecord.Err]。
	UploadNote string
	// Fired 是这一条**有没有真的往上游发过枪**（与 [TaskStepRecord.Err] 是一对：
	// 两条合起来才说得清"失败"是哪一种）。
	//
	// 为什么要有这一格（2026-09-26 第廿四趟整趟真跑跑出来的）：小结那句原本无条件写
	// "N 条上报失败"，而当天两号各念了一句 —— 号一那句底下**一条都没有上报**：
	// 434 的"没有可分享的文件"是**缺前置**（本仓刻意不发），1020 才是真发了一发 ⑧ 被打回
	// `HTTP 404`。两种在用户面前长得一样，而**下一步动作完全不同**（前者要么等前面有上传、
	// 要么拍 Q41；后者要去查这一族的入口）。口径与 [TaskRunCounts.FailedNoShot]、
	// [TaskMatrixRunResult.SummaryLine] 那三档同源。
	//
	// ⚠ 它的含义是"出过网"，**不是**"上游认了"：打卡挂了也算出过网（与复查那四格里
	// "做了但上游未认"用同一个 `fired` 口径，见 [apply] 的第二个返回值）。
	Fired bool
}

// TaskVerifyRecord 是复查那一趟对一条任务的读数。
type TaskVerifyRecord struct {
	TaskID int64
	Name   string
	// Group 是 `groupid`（`day` / `month` / `beiyong1` …）。
	//
	// 它只为**播报那一层**存在（`task_matrix_report.go` 要按组分节），
	// 但它是结构体上的一格真事实：复查读到的那条任务属于哪一组，与第一趟
	// [TaskStepRecord.Group] 同一个来源（[taskGroupOf]）。
	Group string
	Class TaskVerifyClass
	// Label 是"该去哪个 App 做"（只有 [VerifyStillOpen] 带）。
	Label string
	// Note 是补打卡那一发的说明（481 / 478 那两条，key 零实拍）。
	Note string
}

// TaskRunCounts 是一次运行的计数。
type TaskRunCounts struct {
	// Read 是第一趟读到的条目数（聚合 + 去重后）。
	Read int
	// Pending 是过了"要不要做"那道门的条数。
	Pending int
	// Clicked 是**成功发出**的打卡条数（按任务计，一条任务打两发算一条）。
	Clicked int
	// Ran 是跑完的专属玩法条数。
	Ran int
	// Failed 是有枪发失败的条数。
	Failed int
	// FailedNoShot 是 [TaskRunCounts.Failed] 里**一枪都没发**的那几条（缺前置）。
	//
	// 为什么要单开：2026-09-26 整趟真跑，号二的小结念"2 条上报失败"，而这两条**根本不是一回事** ——
	// 434 那条一枪都没出网（这一趟前面没有上传任务 ⇒ 没有可分享的文件），
	// 1020 那条是真发了一发 ⑧、被打回 `HTTP 404`。
	// 用户在两种"失败"面前的下一步动作完全不同，而原来那一句分不出来。
	// 判据是 [TaskStepRecord.Fired]，出处见那一格的注释。
	FailedNoShot int
	// ToolGap 是"本仓没接玩法"的条数。
	ToolGap int
	// RuleSkipped 是"按规则本来就不发"的条数（SKIP 表 / currstep 门 / 无 id）。
	RuleSkipped int
	// NeedsHuman 是"要人去 App 操作"的条数。
	NeedsHuman int
	// Uploaded 是这一趟**往网盘传出去**的文件数（秒传也算，`exist` 不算）。
	//
	// ⚠ 它**不按任务计**（与其余各格不同）：522 一条就能传 5 个。所以它不进
	// 复查那四格，只在小结里回答"这一趟在用户网盘里写了几个文件"。
	Uploaded int
	// DoneNow / NotAcknowledged / AlreadyWas / StillOpen 是复查那四格。
	DoneNow         int
	NotAcknowledged int
	AlreadyWas      int
	StillOpen       int
	// Hidden 是复查趟按旧实现**不写进报告**的那两组条数（计数留着，别当"没读到"）。
	Hidden int
}

// TaskMatrixRunResult 是一个账号这一趟的产出。
type TaskMatrixRunResult struct {
	Account core.AccountID
	// Steps 是第一趟的逐条处置（顺序 = 执行顺序，界面上要能按它讲"我做了什么"）。
	Steps []TaskStepRecord
	// Verified 是复查趟的逐条读数（跳过复查时为空）。
	Verified []TaskVerifyRecord
	// Counts 是上面两段的计数。
	Counts TaskRunCounts
	// ReadFailures 是第一趟**没读到**的组数（来自 [TaskMatrix.Failures]）。
	//
	// ⚠ 它必须一路带到结论：读到 3 条与"只读到 3 组、其余 13 组没读到"是两回事，
	// 后者会让"待做 N 条"凭空变小，而界面上看不出来。
	ReadFailures int
	// Fetches 是第一趟实际发出的读取请求数。
	Fetches int
	// VerifiedAgain 是复查趟实际发出的读取请求数（跳过复查时为 0）。
	VerifiedAgain int
	// Notes 是给人看的零散结论（434 的分享进度、补打卡那两枪、复查没读成）。
	Notes []string
	// CloudFileIDs 是这一趟在用户网盘里**造出来**的文件 id（含失败路径上已占坑的）。
	//
	// 它是**清理那一族的输入**，消费点就在同一次 [RunAccount] 的末尾（那一句
	// `batchTrash` 走 [CloudUploader.DeleteCloudFiles]，播报见 [TaskCleanupLines]）。
	// 2026-09-25 之前这一格只是"带出来给人看"，小结因此必带"（未清理）"那一句；
	// 现在那句话按实际结局改写（[TaskMatrixRunResult.Cleanup]）。
	//
	// ⚠ 这里**只放 id**：文件名删除那一口用不上；而 `create` 回的预签名地址里
	// 就是一次性 token ⇒ 绝不外带（凭据边界，见各 `docs/capture/` 样本的抬头）。
	CloudFileIDs []string
	// CloudProductFiles 是上面那份名单里**不是我们上传的**那一半的枚数（今天唯一
	// 来源：玩法 1014 的 ⑥ 在网盘里另存的转换产物，见 [collectMailPDFProduct]）。
	//
	// 为什么结果里要专门有一格：清理只看 id，两族一样删，[CloudFileIDs] 已经够了；
	// 而**小结**那句是按 `len(CloudFileIDs) - Counts.Uploaded` 做减法解释"网盘里为什么
	// 多了 N 个"，产物那一半被减法读成了"占位没传完的坑"。2026-09-26 第廿三趟真跑
	// 念出来的就是这一版：跑成一次 1014 之后小结是
	// `网盘新增 1 个文件（已移进回收站）；另有 1 个占位未传完` —— 那"另有 1 个"其实是
	// 传完了、也删掉了的产物。它不另开一份名单（那会让清理链要拼两段，多一处漏），
	// 只是一个**播报用的枚数** ⇒ 与 [CloudFileIDs] 的一致性由写入点（`runMailPDF`）保证。
	//
	// ⚠ 它**不参与**任何判定：删不删、删几个都只看 [CloudFileIDs]。
	CloudProductFiles int
	// Cleanup 是清理那一节的结局（三档 + 一句原因）。零值 [TaskCleanupNotRun]
	// 的含义就是字面"这趟没有要清理的东西"，所以它**不是**"没跑成"。
	Cleanup TaskCleanupOutcome
	// CleanupReason 是"没删成 / 刻意不删"的那句原因（成功与不跑时为空）。
	CleanupReason string
}

// RunAccount 跑一个账号：读表 → 逐条分派 → 复查。
//
// ⚠ 整表读不下来（十六次全挂）在这里**整体报错**，与 [AppTaskMatrix] 同口径；
// 单条打卡失败不整体失败（旧实现也是继续跑完剩下的），失败原文留在
// [TaskStepRecord.Err] 里进结论。
func (r TaskMatrixRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (TaskMatrixRunResult, *core.Error) {
	// 走 [SignInWorld.Open] 而不是 [SignInWorld.Source]：上传那三枪要 `Authorization`
	// 整串（另一套鉴权，不是会话票），而档案与凭据只能从同一份装配里拿
	// （理由见 `signin_world.go` 里 [SignInOpening] 那条注释）。
	opening, perr := r.Open(ctx, account)
	if perr != nil {
		return TaskMatrixRunResult{Account: account, Notes: []string{},
			CloudFileIDs: []string{}}, perr
	}
	src := opening.Source
	result := TaskMatrixRunResult{Account: account, Notes: []string{}, CloudFileIDs: []string{}}

	matrix, perr := AppTaskMatrix(ctx, src)
	if perr != nil {
		return result, perr
	}
	result.Fetches = matrix.Fetches
	result.ReadFailures = len(matrix.Failures)
	result.Counts.Read = len(matrix.Items)
	pending := matrix.Pending()
	result.Counts.Pending = len(pending)

	// 播报的**入口那两行**（旧实现 `index.ts` 里 `logger.start("------【任务列表】------")`
	// 与那句 `获取到 32 个任务，去重后 24 个`）。
	//
	// ⚠ "去重后"这一格必须是 `Raw` 与 `Read` 两个数：16 次拉取会给出重复的同名任务
	// （实拍那两轮就是 32→24 与 35→27），只报后者会让人以为"这个号只有 24 条任务"。
	// 少掉的那些**不必**是同一条：去重键只有 `name` ⇒ 第九趟真跑念出来的是每趟丢掉 8 份、
	// 其中 5 份**与留下的那份 `id` 不同**（`开启备份领好礼` 548 留下 / 549 输掉，两边同一市场名）。
	// 那一格连同它的 `state` / `enable` 由 [DedupLossLines] 逐份念出来。
	r.progress(core.LevelInfo, "------【任务列表】------")
	r.progress(core.LevelInfo, fmt.Sprintf("获取到 %d 个任务，去重后 %d 个", matrix.Raw, len(matrix.Items)))
	// 按市场名再念一遍**同一批**读数（本文 R13 / R14 要的就是这一行）。
	//
	// 为什么上面那句不够：`获取到 46 个` 是两个市场名摊平之后的数，于是"邮箱那一支
	// 一条都没回"与"两支各回一半"长得一模一样 —— 而这两档对"我们带的 `activityid`
	// 上游认不认"是完全不同的证据。跑一趟只有一次机会留下这个读数（回去就要重跑整条链）。
	if line := matrix.MarketSummaryLine(); line != "" {
		r.progress(core.LevelInfo, line)
	}
	// 组级读数（`taskFinishState` 与 `process`）逐组念一遍，**复查后**还会念第二遍。
	//
	// 为什么值得占两行日志：这两格今天既进结构体又**一个字都不念**，于是 R15 那两问
	// （"它是不是组内完成度""`process` 是不是进度"）跑十趟也答不出 —— 而答出它只需要
	// 把两次读数并排看一眼。
	for _, line := range matrix.GroupReadingLines("拉表前") {
		r.progress(core.LevelInfo, line)
	}
	// 去重**输掉的那一份**读了什么（紧贴在组读数后面 —— 它俩是同一格读数的两半）。
	for _, line := range DedupLossLines("拉表前", matrix) {
		r.progress(core.LevelInfo, line)
	}
	if result.ReadFailures > 0 {
		// 这一句要紧贴在条数后面：`24 个`读起来像"全表就这些"，而它可能只是 13/16 组。
		// 小结里也有同一格（[TaskMatrixRunResult.SummaryLine]），但那是三十行之后才出现，
		// 追实时日志的人看不到。
		r.progress(core.LevelWarn, fmt.Sprintf("有 %d/%d 组没读到 ⇒ 这不是全表",
			result.ReadFailures, matrix.Fetches))
	}

	exec := newTaskExecutor(r, opening, account)
	gate := exec.backupGate(ctx)
	for _, item := range pending {
		plan := PlanTaskStep(item, gate)
		// 动手**之前**先播一行（口径与"这一条会不会出网"见 [taskStepLiveLine]）。
		if line, ok := taskStepLiveLine(item, plan); ok {
			r.progress(line.Level, line.Text)
		}
		record, fired := exec.apply(ctx, item, plan)
		result.Steps = append(result.Steps, record)
		// ⭐ 打卡挂了的**原文**当场播一行（其余岔路不播 —— 它们自己已经念过了）。
		//
		// 为什么只补 `StepClick` 这一档：2026-09-25 那趟真跑把事件流整个读下来，
		// 上传（[taskExecutor.runUploads]）、入口上报、玩法那几条岔路**各自**都
		// `progress` 了一句带原文的话，唯独打卡那一枪挂掉时**一个字都没有** ——
		// 它的原文只进 [TaskStepRecord.Err]，而那份记录今天出不了 app 层
		// （不序列化进 `finished`，末尾报告用的是旧实现那句不带原因的逐字文案 `失败 <名称> <id>`）。
		// 反过来，若对所有 `Err != ""` 都播，434 那种"一枪没发而跳过"会连念两遍
		// （runner 自己那句 warn + 这一句），而同一天实测正是这么撞出来的。
		//
		// ⭐ 第八趟真跑（同日，两条 `app-task`）把这条规则的**前提**又确认了一遍：
		// 两条 run 的小结都念"1 条上报失败"，事件流里都没有 `打卡失败 …` 那一行 ——
		// 而那一条失败**都是 434 分享那一档**（`StepRunner`），它的实时行是 warn 那句
		// `分享文件跳过 … 没有可分享的文件 …` ⇒ "小结计数有它、却没有带原因的行"**不是缺口**。
		// 反过来读也不成立：不能因为"这一趟没出现打卡失败行"就断定播报漏了。
		//
		// 反向改坏它：删掉这一句 ⇒ `TestTaskMatrixRunBroadcastsTheFailureReason` 红。
		if plan.Kind == StepClick && record.Err != "" {
			r.progress(core.LevelError, fmt.Sprintf("打卡失败 %s %d：%s",
				record.Name, record.TaskID, record.Err))
		}
		result.Counts.absorb(record)
		if id, ok := taskIDOf(item); ok && fired {
			exec.done[id] = true
		}
		// 434 是"分享第几天"的多日任务：跑之前先记下名字里那个数，
		// 复查时只有**涨了**才报好消息（旧实现 `index.ts:47-54`）。
		if id, ok := taskIDOf(item); ok && id == 434 {
			if n, found := ShareProgress(taskNameOf(item)); found {
				value := n
				exec.shareBefore = &value
			}
		}
	}
	result.CloudFileIDs = append(result.CloudFileIDs, exec.cloudFileIDs...)
	result.CloudProductFiles = exec.cloudProductFiles

	// 一枪都没发时跳过复查是**无损**的；发过枪就不给跳（见 SkipVerify 那条注释）。
	if r.SkipVerify && len(exec.done) == 0 {
		r.cleanupCloudFiles(ctx, &exec, &result)
		r.progress(core.LevelInfo, result.SummaryLine())
		return result, nil
	}
	r.verify(ctx, src, matrix, &result, &exec)
	// **逐条**播报复查读数（分节 + 组内四档），然后才是那句小结。
	//
	// ⚠ 顺序刻意：人是**从上往下**读日志的。把小结放前面会让"上游没认这几条"
	// 埋在一堆计数之后，而那句小结之下的每一行才是"具体哪一条"。
	//
	// ⚠ [TaskMatrixRunResult.Notes] 里的三格**不**各自占一行：434 的分享进度与
	// "复查没读到"拼在小结尾巴上（[SummaryLine] 末尾），补打卡那一则挂在它所属
	// 那一行的尾巴（见 [TaskVerifyLines]）⇒ 同一句话不会在事件流里出现两次。
	for _, line := range TaskVerifyLines(result.Verified) {
		r.progress(line.Level, line.Text)
	}
	// 清理排在**复查播报之后、小结之前**：旧实现那一节属于 `afterTask`，整条链跑完才做
	// （`after.ts:131`），而小结必须带上清理的结局 —— 反过来写会让"网盘新增 N 个"
	// 那一句先落地、再补一句"其实删了"，读的人要在两句之间自己回滚一次。
	r.cleanupCloudFiles(ctx, &exec, &result)
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// cleanupCloudFiles 把这一趟造出来的网盘文件移进回收站，并播那一节。
//
// 三格判据都出自旧实现（`after.ts:131-137` + `utils/file.ts:113-125`）：
//
//   - **一次删多个**：旧实现把跨任务累积的整份 `ctx.state.files` 一次交给 `batchTrash`，
//     所以活体日志里是"一行 id 逗号相连 + 一句成功"。这里同样一发打完，
//     不做"逐条删"（那是多发请求、多几个失败点，旧实现没有这个形状）。
//   - **删的只有我们自己记下的 id**：名单只有 [taskExecutor.cloudFileIDs] 一个来源，
//     而 `exist`（撞上用户已有同名文件）那一档刻意**不**记 id（`cloud_upload.go` 文件头）。
//     这一格是"会不会删错东西"的唯一防线，所以它不接受任何外部传入的 id。
//   - **失败不重跑**：旧实现失败只打一句 error。这里也一枪不打第二遍 —— 但要把
//     结局记进结果并写进小结，因为"没删掉"意味着**用户网盘里留着东西**。
//
// ⚠ 本函数原来那句"`batchTrash` 自身零实拍"已被 2026-09-26 的活体探针**推翻并就地改掉**：
// 这一口今天打过十几枪（第廿六~卅趟），请求侧被上游认、回执三格也有逐字读数
// （`code:"0000"` / `success:true` / 没有 `msg` 键，见
// [`docs/capture/app-cloud-recyclebin-samples.txt`](../../docs/capture/app-cloud-recyclebin-samples.txt) §13），
// 而"删一枚非空目录只留一条""重复点名不新增条目"两格也都有了判据。
// 仍**没有**读数的两格：上游把 `batchTrash` 的**失败**回成什么形状（今天只见过成功那一档），
// 以及"整份名单里有一枚 id 是别人家的 / 已经不在了"时那一发怎么判 —— 本函数不替它们做保证。
func (r TaskMatrixRun) cleanupCloudFiles(
	ctx context.Context, exec *taskExecutor, result *TaskMatrixRunResult,
) {
	if len(result.CloudFileIDs) == 0 {
		result.Cleanup = TaskCleanupNotRun
		return
	}
	if r.KeepUploadedFiles {
		result.Cleanup = TaskCleanupKept
		result.CleanupReason = "这一趟设了 KeepUploadedFiles"
		for _, line := range TaskCleanupLines(result.CloudFileIDs, result.Cleanup, result.CleanupReason) {
			r.progress(line.Level, line.Text)
		}
		return
	}
	uploader, uerr := exec.cloudUploader(ctx)
	if uerr != nil {
		result.Cleanup = TaskCleanupFailed
		result.CleanupReason = uerr.Error()
	} else if _, derr := uploader.DeleteCloudFiles(ctx, result.CloudFileIDs); derr != nil {
		result.Cleanup = TaskCleanupFailed
		result.CleanupReason = derr.Error()
	} else {
		result.Cleanup = TaskCleanupDone
		result.CleanupReason = ""
	}
	for _, line := range TaskCleanupLines(result.CloudFileIDs, result.Cleanup, result.CleanupReason) {
		r.progress(line.Level, line.Text)
	}
}

// backupGate 读一次 `infoV3` 拿"本月开没开自动备份"，交回分派要的外部事实。
//
// 读失败 ⇒ 交回 `BackupKnown=false`（门**不挡**任务）。这不是偷懒：旧实现在
// 那一格没值时同样不挡，而"读不到就全跳过"会让一次网络抖动变成"这个号今天没事可做"。
func (e *taskExecutor) backupGate(ctx context.Context) taskStepInput {
	in := taskStepInput{DayOfMonth: e.dayOfMonth()}
	status, perr := SignInStatus(ctx, e.src)
	if perr != nil || status.CurMonthBackup == nil {
		return in
	}
	in.BackupKnown = true
	in.BackupOn = *status.CurMonthBackup
	return in
}

// dayOfMonth 给"这个月的第几天"，按**东八区**算。
//
// ⚠ 不能用 UTC 的日：[core.Clock] 交回的是 UTC，而那道门（旧实现 `getDate() < 20`）
// 是本地日历上的 20 号。UTC 读数会让每月 19 日 08:00–23:59 这一窗口的判定
// 与真实客户端差一天。
//
// ⚠ 时钟没接（`Clock == nil`，CLI 单次调用与部分测试夹具）时交回 0 ⇒
// [taskStepInput.backupGateBlocks] 恒 false ⇒ 门不挡。这是"读不到就当不知道"
// 的同一取向，不是漏判。
func (e *taskExecutor) dayOfMonth() int {
	if e.run.Clock == nil {
		return 0
	}
	return e.run.Clock.Now().In(appTaskLocation()).Day()
}

// verify 打第二趟：重新拉表 → 补该补的那一发 → 按状态分四格。
//
// `first` 是**第一趟**那份表。传进来的唯一用途是逐条 `process` 的对照播报
// （[ProcessChangeLines]）—— 那一格要的是"同一趟的前后两遍"，隔一趟再比就混进
// 上游自己按日重置那一档，分辨不了（与组读数为什么要念两遍同一条理由，见 R15）。
func (r TaskMatrixRun) verify(
	ctx context.Context, src *SignInSessionSource, first TaskMatrix,
	result *TaskMatrixRunResult, exec *taskExecutor,
) {
	second, perr := AppTaskMatrix(ctx, src)
	if perr != nil {
		// 复查失败**不**推翻第一趟：枪确实发出去了。它只让"认没认"这一格变成未知。
		result.Notes = append(result.Notes, "复查没读到，所以完成数无法核对："+perr.Error())
		return
	}
	result.VerifiedAgain = second.Fetches
	// **第二次**组级读数（与入口那几行同格式，只换行头标签）。
	//
	// 这一行为什么值一次拉取（32 个请求）：R15 的第一问是"`taskFinishState` 随不随
	// 任务完成而动"，而它只能靠**同一趟的前后两次**并排回答 —— 隔一天再跑就混进了
	// "上游自己按日重置"这一档，分辨不了。两行都在，日志里一眼可见"有没有哪一组动过"。
	for _, line := range second.GroupReadingLines("复查后") {
		r.progress(core.LevelInfo, line)
	}
	// **逐条**对照两遍的 `process`（R15 归因那一格：组读数只给面值集合，指不出是哪一条动的）。
	//
	// 反向改坏它：删掉这一段 ⇒ `TestTaskMatrixRunBroadcastsProcessDiff` 红
	// （它同时钉"变了的那一条要点名"与"总结那行要在明细之前"）。
	for _, line := range ProcessChangeLines("复查后", first, second) {
		r.progress(core.LevelInfo, line)
	}
	// 复查那一遍的去重读数也要念：两遍是**两批读数**，只念拉表前那一遍就等于只留着一半证据。
	// 第九趟真跑正是这个形状 —— 拉表前两边同读 `79`、复查后同读 `84`，而"输掉的那一份
	// 与留下的那份 id 不同"那五行在两遍里各出现一次（少了后一遍就无从和 `process` 对照行对齐）。
	for _, line := range DedupLossLines("复查后", second) {
		r.progress(core.LevelInfo, line)
	}
	for _, item := range second.Items {
		id, ok := taskIDOf(item)
		if !ok {
			continue
		}
		if id == 434 {
			// 多日任务：旧实现只报"进度涨了没有"，不参与那四格。
			if n, found := ShareProgress(taskNameOf(item)); found {
				if exec.shareBefore != nil && n > *exec.shareBefore {
					result.Notes = append(result.Notes, fmt.Sprintf(
						"分享文件有好礼：已第 %d 天（跑之前是 %d 天）", n, *exec.shareBefore))
				}
			}
			continue
		}
		note := ""
		if key, again := PlanVerifyClick(item); again {
			if _, cerr := r.clickOnce(ctx, src, key, id); cerr != nil {
				note = "补打卡失败：" + cerr.Error()
			} else {
				note = "补打卡已发出（" + key + "，这一枚 key 零实拍）"
			}
			// 口径同 [apply]：**发出过**就算"这一条今天动过"。
			// 于是补打失败的那一条落在"做了没认"，而不是"仍待办"（后者是说给用户的
			// "这一条我们没碰，你去 App 里做"，而我们明明碰了）。
			exec.done[id] = true
		}
		class := ClassifyVerifiedTask(item, exec.done)
		record := TaskVerifyRecord{TaskID: id, Name: taskNameOf(item),
			Group: taskGroupOf(item), Class: class, Note: note}
		if class == VerifyStillOpen {
			record.Label = VerifyMarketLabel(item)
		}
		result.Verified = append(result.Verified, record)
		result.Counts.absorbVerify(class)
	}
}

// clickOnce 发一发打卡（过 [do]：撞 90001 刷一次票、重放一次）。
func (r TaskMatrixRun) clickOnce(
	ctx context.Context, src *SignInSessionSource, key string, id int64,
) (upstream.TaskClick, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.TaskClick, *core.Error) {
		return src.up.AppTaskClick(ctx, s.jwt, s.referer, key, id)
	})
}

// taskExecutor 是"把决定变成请求"那一层带的**跨条状态**。
type taskExecutor struct {
	run     TaskMatrixRun
	opening SignInOpening
	account core.AccountID
	// done 是这趟**真的发过枪**的 id 集合（复查那四格的输入）。
	done map[int64]bool
	// src 是 [opening] 里那条会话源的投影（读表、打卡与复查共用一张票）。
	src *SignInSessionSource

	// deviceID 是惰性取的：那两条入口类 runner 与上传那三枪都要它
	// （`deviceid` 是旧实现主 http 实例的静态头，每一枪都带）。
	// 取失败时 [deviceErr] 记下原因：需要它的每一条各自记失败、其余任务照跑 ——
	// 一个活动侧接口挂了不该让整趟变成"这个号没跑成"。
	deviceID  string
	deviceErr *core.Error
	deviceGot bool

	// cloudFileIDs 是这一趟在网盘里造出来的文件 id（进结果，等清理那一族消费）。
	cloudFileIDs []string

	// cloudProductFiles 是这些 id 里**不是我们上传的那一半** —— 上游自己另存出来的
	// （今天只有一个来源：玩法 1014 的 ⑥ 转换产物，见 [collectMailPDFProduct]）。
	//
	// 为什么单独有一格而不是并进 [cloudFileIDs] 就完事：清理只看 id，两族一样删；
	// 而**小结**要说清"网盘里为什么多了 N 个"，那一格用的是减法
	// （`len(CloudFileIDs) - Counts.Uploaded`，见下面那句"占位未传完"）。
	// 2026-09-26 第廿三趟真跑念出来的就是**算错的那一版**：跑成一次 1014 之后小结是
	// `网盘新增 1 个文件（已移进回收站）；另有 1 个占位未传完` —— 那"另有 1 个"其实是
	// ⑥ 另存的产物，它传完了、也删了，被减法读成了"传了一半的坑"。
	cloudProductFiles int

	// shareBefore 是跑之前从 434 的名字里读到的天数。
	shareBefore *int

	// mailSession 是这一趟换到的 139 邮箱会话（② 的产物：`sid` + `rmkey`）。
	//
	// ⚠ **只在成功时缓存**，且这一趟之内有效（不跨轮、不进票池）—— 照旧实现的
	// `ctx.state.mail139`，理由与"失败时下一条会重走 ①②"的取舍写在
	// `task_matrix_mail.go` 的 [mailSessionFor] 头注。
	//
	// 为什么**不**接进 [CloudPhoneSessionPool] 那一类跨轮票池：那一族的键是
	// (账号, 凭据指纹)，而邮箱会话的有效期**零样本**（`docs/pending-decisions.md`
	// 里"sid/rmkey 能活多久"那一格）⇒ 跨轮复用等于自造一个 TTL，
	// 而 AGENTS.md §1.12 明写不自造。
	mailSession *upstream.MailSession
}

func newTaskExecutor(
	r TaskMatrixRun, opening SignInOpening, account core.AccountID,
) taskExecutor {
	return taskExecutor{
		run: r, opening: opening, src: opening.Source, account: account,
		done: map[int64]bool{}, cloudFileIDs: []string{},
	}
}

// apply 执行一个决定，交回那条任务的记录与"**这一条今天动过没有**"。
//
// ⚠ 第二个返回值是"**打那一枪了**"，**不是**"上游认了"。两格混成一个的表现：
// 上游回一句 `任务已过期` 时这条今天**没被记成动过**，于是复查那趟把它念成
// "仍待办"（VerifyStillOpen）—— 而那正是"我们没做，用户你自己去做"的那一档。
// 用户会看到一条他其实已经打过、只是打失败的枪挂在"仍待办"里；真正该说的是
// "做了、上游没认"（VerifyFailed / `notAcknowledgedCount`）。
// 所以这里只在**真的发出过请求**时回 true；上游怎么回应由 [TaskStepRecord.Err] 说。
//
// ⚠ 这一条口径同样覆盖**上传那三枪**：一条"只传了文件、没打卡"的（beiyong1 那支
// 被 `currstep` 门挡住的那些）也算动过 —— 它真的出网了，还在网盘里留下了东西。
//
// `paced` 只为不把两处时机叠成两次等待：beiyong1 那一支旧实现在**上传之前**等
// （那一拍在 `handleTaskClick` 里，上传在它外面），runner 那一支在**玩法之后**等
// （`handleTaskClick` 末尾）。两边各等一次，合起来才是旧实现的节奏。
func (e *taskExecutor) apply(
	ctx context.Context, item upstream.TaskV2Item, plan TaskStepPlan,
) (TaskStepRecord, bool) {
	record := TaskStepRecord{
		TaskID: derefTaskID(item), Name: taskNameOf(item),
		Group: taskGroupOf(item), Kind: plan.Kind, Runner: plan.Runner, Reason: plan.Reason,
	}
	var (
		fired    bool
		paced    bool
		problems []string
	)
	switch plan.Kind {
	case StepClick:
		_, cerr := e.run.clickOnce(ctx, e.src, plan.ClickKey, record.TaskID)
		fired = true // 请求发出去了就算动过 —— 成没成看 Err，见函数头
		problems = append(problems, errText(cerr))
		if plan.Upload != nil {
			// 打卡挂了就没有那一拍：旧实现那句 `sleep(500)` 在
			// `if (await clickTaskWithStep(...))` **里面**，而打卡失败时它是 false。
			if cerr == nil {
				e.pace(ctx)
				paced = true
			}
			sent, uploadProblems := e.runUploads(ctx, item, plan.Upload, &record)
			fired = fired || sent
			problems = append(problems, uploadProblems...)
		}
	case StepRunner:
		// 玩法自己那段（含它的上传）把原因直接写进 record.Err，见 [runRunner]。
		fired = e.runRunner(ctx, item, plan, &record)
	case StepBlockedByRule:
		// beiyong1 那一支的门**只挡打卡**：旧实现 `clickTaskWithStep` 在 `currstep != 0`
		// 时返回 true（只是不点），于是那一拍与后面的"名字写着上传"都照做。
		if plan.Upload != nil {
			e.pace(ctx)
			paced = true
			sent, uploadProblems := e.runUploads(ctx, item, plan.Upload, &record)
			fired = sent
			problems = append(problems, uploadProblems...)
		}
	case StepBlockedToolGap, StepNeedsHuman:
		// 什么都不发。这正是这两态的意义。
	}
	if text := joinProblems(problems); text != "" {
		record.Err = text
	}
	record.Fired = fired
	if record.Reason == "" && plan.ToolGap != "" {
		record.Reason = plan.ToolGap
	}
	if fired && plan.Paced && !paced {
		e.pace(ctx)
	}
	return record, fired
}

// runRunner 跑一条专属玩法，交回"**有没有真的发出过请求**"（口径同 [apply]）。
//
// 表里每一条都按 spec 走，不为每条各写一份 switch。四族各自的形状：
//   - 409 / 1005：`前置打卡(受门约束) → mrp 入口上报 → 自己的那一发打卡`，
//     差别只在最后一发的 key；
//   - 106 / 522：没有 `OpensEntry`、没有 `FinalKey`，玩法本体就是上传那一半；
//   - 434 / 107：既不打卡也不上传，本体是"建一个再删一个"（分别是外链与云笔记）；
//   - 472 / 473 / 1014 / 1020 / 585：自己开一条跨服务的链（见下面那五个分支，
//     以及 585 与 1014 共有的"先写网盘再拿 id 干一件事"那一格）。
//
// ⚠ "取不到 deviceId"那一档**不算发过**：入口上报压根没出网（旧实现在那里会带一发
// 空 `deviceId`，我们不跟着做，成文见 `app_task_click.go` 文件头）。
//
// ⭐ 前置那一发**发了但失败** ⇒ 后面的全部跳过（旧实现 `handleTaskClick` 的 `if`
// 包着 runner 那一句）。这一格在只有 409 / 1005 时是"少一发入口上报"，
// 接上传之后它决定"上游没接这条任务时还要不要往用户网盘写文件"⇒ 必须照。
// 仍交回 `fired=true`：那一发打卡确实出网了（"动过"的口径见 [apply]）。
func (e *taskExecutor) runRunner(
	ctx context.Context, item upstream.TaskV2Item, plan TaskStepPlan, record *TaskStepRecord,
) bool {
	id := derefTaskID(item)
	spec, ok := taskMatrixRunners[id]
	if !ok {
		record.Kind = StepBlockedToolGap
		record.Reason = "分派表说走 runner，但 runner 表里没有这一条（内部不一致）"
		return false
	}
	fired := false
	var problems []string
	if plan.ClickKey != "" {
		_, cerr := e.run.clickOnce(ctx, e.src, plan.ClickKey, id)
		fired = true
		if cerr != nil {
			record.Err = "前置打卡失败：" + cerr.Error() + " ⇒ 按旧实现的门，这一条的玩法整条不跑"
			return fired
		}
	}
	if spec.OpensEntry {
		deviceID, derr := e.accountDeviceID(ctx)
		switch {
		case derr != nil:
			problems = append(problems, "入口上报没做成（取不到 deviceId）："+derr.Error())
		default:
			if err := e.entryReport(ctx, deviceID); err != nil {
				// 原文必须带上（2026-09-26 第廿四趟真跑跑出来的错）：从前这一句只写
				// "入口上报失败"，于是界面上是一条**没有任何原因**的失败 —— 而同一趟
				// 其他岔路（打卡、玩法、邮箱）都带原文。
				problems = append(problems, "入口上报失败："+err.Error())
			}
			fired = true
		}
	}
	if spec.FinalKey != "" {
		if _, cerr := e.run.clickOnce(ctx, e.src, spec.FinalKey, id); cerr != nil {
			problems = append(problems, "玩法打卡失败："+cerr.Error())
		}
		fired = true
	}
	if spec.Upload != nil {
		sent, uploadProblems := e.runUploads(ctx, item, spec.Upload, record)
		fired = fired || sent
		problems = append(problems, uploadProblems...)
	}
	if spec.Shares {
		// 外链那一族排在上传之后：它消费的是**这一趟前面那些任务**传上去的文件
		// （口径与旧实现 `ctx.state.files` 完全一致，见 [runShare]）。
		sent, shareProblems := e.runShare(ctx, item)
		fired = fired || sent
		problems = append(problems, shareProblems...)
	}
	if spec.Notes {
		// 笔记那一族**不消费**上面那些账：它自己开一条链（换票 → 建 → 移进回收站），
		// 与网盘文件无关，所以放在 Shares 之后也不改变任何一条的形状。
		sent, noteProblems := e.runNote(ctx, item)
		fired = fired || sent
		problems = append(problems, noteProblems...)
	}
	if spec.Mail {
		// 邮箱那一族（472 / 473）排在最后，同样**不消费**前面那些账：它自己开一条
		// 四枪的链，跨两台主机，产物只有"④ 记上的那条进度"（见 `task_matrix_mail.go` 文件头）。
		//
		// ⚠ 它与前几格的**关键差别**在 [taskRunnerSpec.Mail] 那条注释上：这条 spec 的
		// `ClickKey` 与 `FinalKey` 都是空 ⇒ 通用打卡那一支一个字都不念，
		// 失败全靠 [runMailExperience] 自己按"哪一枪挂了"分档播报。
		sent, mailProblems := e.runMailExperience(ctx, item)
		fired = fired || sent
		problems = append(problems, mailProblems...)
	}
	if spec.MailBackup {
		// 1020：会话（①②）+ ⑧ 那一发。它排在 [Mail] 之后不影响任何一条 —— 一条任务
		// 只有一个开关会命中（`Mail` 是 472 / 473，`MailBackup` 是 1020）。
		sent, backupProblems := e.runMailBackup(ctx, item)
		fired = fired || sent
		problems = append(problems, backupProblems...)
	}
	if spec.MailPDF {
		// 1014：会话（①②）+ 传一份最小 PDF + ⑥。它是这一张表里**唯一一条要先写网盘**
		// 的邮箱玩法 ⇒ 放在最后，让链尾那一条 `batchTrash` 清理链在它之后收口
		// （口径与 [spec.Upload] 那几条一致：传上去的一定要记账，见 `runMailPDF`）。
		sent, pdfProblems := e.runMailPDF(ctx, item, record)
		fired = fired || sent
		problems = append(problems, pdfProblems...)
	}
	if spec.AICamera {
		// 585：① 识图（**写网盘**）→ ② 拿 fileId 问一句（回 SSE）。
		//
		// 它与 [MailPDF] 挨着放不是巧合：两条都是"自己往网盘造一个文件、再拿它的 id
		// 干一件事"，所以两条都**必须**在链尾那条清理枪之前记账（见 `runMailPDF` 的
		// 同一处口径）。区别在凭据：那一条全程一把（邮箱会话），这一条两枪两枚 ——
		// 逐处取舍写在 `task_matrix_play_ai.go` 文件头。
		sent, cameraProblems := e.runAICamera(ctx, item, record)
		fired = fired || sent
		problems = append(problems, cameraProblems...)
	}
	if spec.Workbench {
		// 1028：邮箱会话（①②）→ ①′ 派生 AI 域凭据 → ②′ 查前置 → ③′ 问一句 →
		// 等 5 秒 → ④′ 再看一眼。它排在 [AICamera] 之后不影响任何一条（一条任务只有
		// 一个开关会命中），但它**自己带一次等待**，所以放在链尾附近：那 5 秒不会
		// 推到别的任务身上。
		//
		// ⚠ 与 [Mail] 那一支同族的一点：它的 `ClickKey` / `FinalKey` 都由 [PlanTaskStep]
		// 的门决定（runner 自己不打本族 `task/click`），成败全靠 [runWorkbench] 分档播报。
		sent, workbenchProblems := e.runWorkbench(ctx, item)
		fired = fired || sent
		problems = append(problems, workbenchProblems...)
	}
	record.Err = joinProblems(problems)
	return fired
}

// accountDeviceID 惰性取 H5 `deviceId`（同一趟只取一次，失败也只试一次）。
//
// 两个消费者：那两条入口类 runner（`mrp …/doTask` 的查询参数）、上传那三枪
// （`deviceid` 头 —— 旧实现是主 http 实例的静态头，每一枪都带）。
// 之所以只取一次：这一趟它不会变，而换不到时每次都重试会把一次活动侧故障
// 放大成"每条任务多一枪"。
func (e *taskExecutor) accountDeviceID(ctx context.Context) (string, *core.Error) {
	if e.deviceGot {
		return e.deviceID, e.deviceErr
	}
	e.deviceGot = true
	deviceID, perr := e.run.DeviceIDFor(ctx, e.account)
	e.deviceID, e.deviceErr = deviceID, perr
	return deviceID, perr
}

// entryReport 打"云朵中心入口"那一发。响应判定（`code` 非 0 ⇒ 失败）已于
// 2026-09-26 第廿四趟接上，出处见 `app_task_click.go` 文件头那一节。
func (e *taskExecutor) entryReport(ctx context.Context, deviceID string) *core.Error {
	_, perr := do(ctx, e.src, func(s SignInSession) (struct{}, *core.Error) {
		return struct{}{}, e.src.up.AppMrpDoTask(ctx, s.jwt, s.referer, deviceID)
	})
	return perr
}

// pace 等那一点间隔。等不住（ctx 取消）时**静默返回**：
// ctx 已经取消，后面每一枪都会自己失败，没必要在这里再造一条错误文本。
func (e *taskExecutor) pace(ctx context.Context) {
	_ = e.wait(ctx, taskMatrixPace)
}

// wait 是"等一会"的注入点（nil ⇒ [RealWaiter]，理由同 [TaskMatrixRun.Wait] 头上那句
// "默认必须真的等"）。与 [pace] 分开：那一格把取消吞了，这一格要把原因交回调用方
// —— 笔记那一族等不住时**要不要照发后一枪**是它自己的决定，见 [runNote]。
func (e *taskExecutor) wait(ctx context.Context, d time.Duration) *core.Error {
	wait := e.run.Wait
	if wait == nil {
		wait = RealWaiter{}
	}
	return wait.Wait(ctx, d)
}

// nowMillis 给"毫秒时间戳只能由调用方注入"那一族口径提供一个读数（上游层的请求构造
// 函数不许读时钟，见 `internal/upstream/app_note.go` 里 `nowMillis` 那段）。
//
// ⚠ 时钟没接时交回 `false` 而不是"给个 0"：一个零值时间戳会**照常发出去**，
// 而上游看到的是一条 1970 年建的笔记 —— 那比"这一条没做成"难查得多。
// 同一格与 [dayOfMonth] 的处理**刻意不同**：那一格读不到 ⇒ 门不挡（保守放行），
// 因为挡错会让用户今天没事做；这一格读不到 ⇒ 不发（保守拒绝），因为发出去的是**写**。
func (e *taskExecutor) nowMillis() (int64, bool) {
	if e.run.Clock == nil {
		return 0, false
	}
	return e.run.Clock.Now().UnixMilli(), true
}

// ─────────────────────────────────────────────────────────────────────────────
// 上传那一半（106 / 522 与 beiyong1 的"名字写着上传"）
// ─────────────────────────────────────────────────────────────────────────────

// cloudUploader 装配"能往网盘传文件"那一层（编排见 `cloud_upload.go`）。
//
// 三样前置各有各的"缺了会怎样"，所以逐条拦、不赌：
//   - `Authorization`：来自 [SignInOpening.Credential]（schema v7 起库里存的就是整串）。
//     取不到时整趟早就在 [SignInWorld.Open] 报错了，不会走到这里。
//   - `deviceid`：旧实现主 http 实例的静态头，**每一枪都带**。"少这一上游理不理"
//     本仓零样本 ⇒ 按旧实现的处置办（它在取不到 deviceId 时根本不开这一趟，
//     见 `ai_avatar_run.go` 里同一格的说法），这里记一条失败、一枪不发。
//   - 随机源：文件名与内容都由它造（旧 `randomHex`）。没接上就是造不出**任何**一枪
//     的请求体，报"本工具侧缺接线"而不是 panic。
func (e *taskExecutor) cloudUploader(ctx context.Context) (CloudUploader, *core.Error) {
	if e.run.Random == nil {
		return CloudUploader{}, core.NewInternal(
			"随机源没接：造不出文件名与内容（本工具侧缺接线）")
	}
	deviceID, derr := e.accountDeviceID(ctx)
	if derr != nil {
		return CloudUploader{}, core.NewInternal(
			"这一族每一枪都带 deviceid 头，取不到 deviceId 就不发（零样本，不赌少一个头）：" +
				derr.Error())
	}
	return CloudUploader{
		Up:       e.src.up,
		Rng:      e.run.Random,
		Auth:     e.opening.Credential.Authorization(),
		DeviceID: deviceID,
	}, nil
}

// runUploads 跑"这一条的第二半：往网盘传文件"，交回**有没有真的发出过请求**与
// 逐轮的原因（由调用方并进 [TaskStepRecord.Err]）。
//
// 三条判据都出自旧实现，各自"改了会怎样"：
//
//   - **上限就是 `Rounds`**，`process` 只是提前停：旧实现那个循环读的是循环外
//     那个 `task` 对象的 `process`，它在**这一趟里不会变**（没人中途重拉表）⇒
//     "凑满 100 个"靠的其实是那格配置的默认值 5，不是那个读数。这里同样读快照，
//     形状一致：`process` 真会涨就提前停，不涨也不会多传。
//     ⚠ `process` 的语义仍未验证（R15：实拍三条恒 0 而同批条目全 `FINISH`）。
//   - **失败不中断循环**：旧 `uploadRandomFile` 把异常吞成 `false`，循环照转下一轮。
//   - **文件 id 一律记账**，包括失败路径上已经占下坑的那一个（编排层刻意多交，
//     见 `cloud_upload.go` 文件头）。`exist` 那一档**不记** —— 那一个不是我们造的，
//     交给删除那一族就是删用户自己的东西。
func (e *taskExecutor) runUploads(
	ctx context.Context, item upstream.TaskV2Item, shape *taskUploadShape,
	record *TaskStepRecord,
) (bool, []string) {
	uploader, uerr := e.cloudUploader(ctx)
	if uerr != nil {
		return false, []string{"上传没做成：" + uerr.Error()}
	}
	var (
		sent     bool
		problems []string
		stopped  string
		done     int
	)
	for round := 1; round <= shape.Rounds; round++ {
		if shape.StopAtProcess > 0 && taskProcessOf(item) >= shape.StopAtProcess {
			stopped = fmt.Sprintf("第 %d 个之前读到 process=%d 已达 %d ⇒ 停（这一格仍是**相关性**读法，见 R15）",
				round, taskProcessOf(item), shape.StopAtProcess)
			break
		}
		receipt, perr := uploader.UploadRandom(ctx, CloudUploadOptions{
			Channel: shape.Channel, Op: shape.Op, Ext: shape.Ext,
		})
		sent = true
		if receipt.FileID != "" {
			e.cloudFileIDs = append(e.cloudFileIDs, receipt.FileID)
		}
		switch {
		case perr != nil:
			problem := fmt.Sprintf("第 %d 个上传失败：%s", round, perr.Error())
			problems = append(problems, problem)
			// 实时那一行**复用同一段文本**：上传是这一族唯一会往用户网盘写东西的动作，
			// 事件流里的那一行与记录里的那一格**不许**说两句不同的话。
			e.run.progress(core.LevelError,
				fmt.Sprintf("上传文件失败 %s %d：%s", taskNameOf(item), derefTaskID(item), perr.Error()))
		case receipt.Outcome == CloudUploadExists:
			problem := fmt.Sprintf("第 %d 个没传：上游说同名文件已存在（%s）⇒ 不算我们造的，也不去删",
				round, receipt.Name)
			problems = append(problems, problem)
			e.run.progress(core.LevelWarn, problem)
		default:
			done++
			// 旧实现那一句是 `logger.debug("上传文件成功")`（每条一行、不带任务名，
			// 因为它一次只在做一条任务）。这里带上名字与 id：这一族一趟要做若干条，
			// 而"传上去的那些文件后来要删"这件事必须能对上号（清理那一半的输入
			// 就是这些 id，见 #37）。
			//
			// ⭐ 括号里那一格是**这一轮新加的读数面**（P1 剩下的第①件）：从前"秒传"
			// 与"走完三枪"念的是**同一句**，于是跑十趟也答不出"上游今天给没给过
			// `rapidUpload:true`"。用词只说**我们发了几枪**，不解释上游为什么给秒传
			// （那一句语义未验证，AGENTS.md §1.7）。
			// 补上之后 2026-09-25 第六趟真跑的读数是：五发**全部**是"完整上传"那一档 ⇒
			// 秒传至今零样本，而这一格**不是**"再跑几趟就有"（随机名 + 随机内容 ⇒ 撞不上），
			// 要样本得故意重传同名同内容 —— 那是为补样造行为，本仓未做（口径见 R27）。
			e.run.progress(core.LevelInfo, fmt.Sprintf(
				"上传文件成功（%s） %s %d",
				uploadBranchWords(receipt.Outcome), taskNameOf(item), derefTaskID(item)))
		}
		if shape.PaceEachRound {
			// 旧实现那一拍在循环体末尾，**失败也等、末轮也等**（它把异常吞在函数里，
			// 循环照转）。末轮之后 `handleTaskClick` 还有一次 ⇒ 522 一趟共 6 拍。
			// 这里不"顺手省掉重复的那一拍"：省与不省都是节奏，而我们没有出处说哪个对。
			e.pace(ctx)
		}
	}
	record.Uploads = done
	if stopped != "" {
		record.UploadNote = stopped
	} else if done == 0 && len(problems) == 0 {
		record.UploadNote = "形状写着要传，但上限是 0 ⇒ 一个都不传（内部不一致）"
	}
	return sent, problems
}

// uploadBranchWords 把"这一次上传走了哪一档"念成**播报里括号中那一句**。
//
// 为什么不用 [CloudUploadOutcome.String]（"已上传 / 秒传 / 已存在"）：那一档说的是**结局**，
// 而播报要说的是**我们发了几枪** —— 这一格的用途是回答"上游今天给没给过 `rapidUpload`"
// （P1 剩下的第①件），拿"已上传"念它就等于什么都没念。
//
// ⚠ 两个措辞都只描述**本仓这一次的行为**，不解释上游为什么走这一档（语义未验证，§1.7）。
// `CloudUploadExists` 与出错两档**读不到这里**（各自有自己的那句播报），
// 零值那一档今天走不到 —— ⚠ 它**未被断言**，留一句"未知结局"是为了万一读到时
// 界面不显示成一次成功。
func uploadBranchWords(outcome CloudUploadOutcome) string {
	switch outcome {
	case CloudUploadDone:
		return "完整上传：create → PUT → complete 三枪都发了"
	case CloudUploadRapid:
		return "秒传：只发了 create，PUT 与 complete 没发"
	}
	return "未知结局"
}

// ─────────────────────────────────────────────────────────────────────────────
// 分享那一半（434：建一条外链、再立刻取消它）
// ─────────────────────────────────────────────────────────────────────────────

// runShare 跑玩法 434 的本体，交回**有没有真的发出过请求**与逐格的原因。
//
// 形状逐字照旧实现 `runners.ts:68-88`：取 `ctx.state.files` 的**第一个** →
// `getOutLink(account=手机号, [fileId], "")` → 上游给了 `linkID` 就 `delOutLink([linkID])`。
// 这里的 `ctx.state.files` 就是 [taskExecutor.cloudFileIDs]（同一趟里那些上传任务写的账）。
//
// # 三处与旧实现**刻意不同**，全都在"留一条公开外链"这一侧
//
// 这一族的后果不对称：`getOutLink` 是**写**，它建出来的那条链接在删掉之前
// 任何拿到 URL 的人都能打开。旧实现在三个岔路口都是**静默**的，这里三处都改成说话：
//
//   - 没有可分享的文件：旧实现 `logger.info` 一句就 return，那条任务**照样**被记进
//     `doingList` ⇒ 报告念成"本次成功"。这里交回一条失败原因（[TaskStepRecord.Err]），
//     因为那一发前置打卡很可能已经把这条任务报给上游了 —— "打了卡却没分享"是
//     最该让人看见的一种不一致（门的例外：`currstep == 22` 也打，见 [taskClickGate]）。
//   - 上游没给 `linkID`：旧实现那句 `if (linkId)` ⇒ 静默不删 ⇒ 网盘里留一条公开链接。
//     这里播错误并记失败，文案里直接写"需手动去 App 里取消分享"。
//   - `delOutLink` 的响应：旧实现把 `tryRequest` 的返回值整个丢弃 ⇒ 删失败也是静默。
//     这里判它的码（见 `internal/upstream/app_cloud_outlink.go`），失败就报。
//
// # 两枪都不重试、也不补发
//
// 建外链发一次就够（重放的代价是多一条公开链接）；删失败时**不**在同一趟里补第二发 ——
// 与清理那一族（[TaskMatrixRun.cleanupCloudFiles]）同一条纪律：一次失败的删除
// 该由人决定要不要再试，而不是脚本自己循环写用户的账号。
//
// # 2026-09-25 真跑两趟：两趟都落在**上面第一条那一档**，两口仍零真打
//
// 跑的是 `app-task`（两个测试号各一趟，号码不进成文）。434 当天**在**两个号的待做表里、
// 这个 runner **确实**被调用，而两趟的事件流都是同一句
// `分享文件跳过 分享文件领好礼… 434：没有可分享的文件…` —— 随后 522 / 614 各真的往网盘
// 传上去过文件（分别 5 个与 1 个，链尾也都成功移进回收站）。
//
// 卡住的是**表序**，不是接线：[normalizeTaskMatrix] 按 id 稳定升序排（对齐旧
// `index.ts:16` 的 `sort((a,b)=>a.id-b.id)`），`Pending()` 保持那个次序，于是
// 434 轮到它时排在它前面的那些条**这一趟里没有一条传过东西**（当天能上传的那几条 id
// 都比它大：522 / 614；比它小的 106 在两号当天都是 `FINISH` ⇒ 根本不进待做表）。
// 旧实现同形状，它今天能走到 `getOutLink` 靠的是
// `ctx.state.files` 由更早的别的来源写进来（`utils/file.ts:59/90`、
// `play-ai-special/task.ts:163`、`ai-avatar/task.ts:157`），**不是**靠任务表里
// 恰好有上传排在前面 ⇒ 上面那条"最该让人看见的不一致"就是**旧实现自带的脆弱点**，
// 本仓只是把它如实跑了两次。
//
// ⚠ 同一天接上 434 **之前**那两趟里，这一条念的是"本仓未实现"（tool gap），
// 而 522 的上传同样排在它**后面** ⇒ 表序这一格有四个样本、两种二进制，
// 不是今天偶然。
//
// 后果：`getOutLink` / `delOutLink` 两口今天**依旧零真打**（R30 不闭合）。要不要给这一条
// 造一次可分享的文件（改表序 / 让它自己先传一个 / 从网盘既有文件里取一个）是**设计决定**，
// 本仓未动 ⇒ 立成问句见 [`docs/pending-decisions.md`] **Q41**。
func (e *taskExecutor) runShare(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	if len(e.cloudFileIDs) == 0 {
		// 旧实现那句 `没有可分享的文件，跳过分享任务`（`runners.ts:71`）+ 上面第一条差异。
		problem := "没有可分享的文件：这一趟前面没有任务往网盘传过东西 ⇒ 分享那一半没做"
		e.run.progress(core.LevelWarn, fmt.Sprintf("分享文件跳过 %s %d：%s", name, id, problem))
		return false, []string{problem}
	}
	deviceID, derr := e.accountDeviceID(ctx)
	if derr != nil {
		return false, []string{"分享没做成：" + derr.Error()}
	}
	account := e.opening.Credential.Phone()
	if account == "" {
		// 这一格是 `commonAccountInfo.account`（旧实现传的就是登录手机号）。
		// 错误文本里**不**echo 它：它是完整号码，而这一族的其它三处同样不带。
		return false, []string{"分享没做成：这个号没有可用的手机号（commonAccountInfo 定位不到账号）⇒ 一枪不发"}
	}
	auth := e.opening.Credential.Authorization()
	shared, perr := e.src.up.AppCloudGetOutLink(ctx, auth, deviceID, upstream.CloudOutLinkInput{
		Account: account,
		// 只分享**第一个**（旧实现 `files[0]`）。多文件会回几条 linkID 未测试，
		// 所以这里不"顺手把这一趟传的都分享掉"。
		FileIDs: []string{e.cloudFileIDs[0]},
	})
	if perr != nil {
		problem := "分享失败：" + perr.Error()
		e.run.progress(core.LevelError,
			fmt.Sprintf("分享文件失败 %s %d：%s", name, id, perr.Error()))
		return true, []string{problem}
	}
	// 请求**已经发出去了**：从这里往下每一档都算"这一条今天动过"（口径见 [apply]）。
	if shared.LinkID == nil || *shared.LinkID == "" {
		problem := "已建外链但上游没给 linkID ⇒ 没删：这条链接可能仍在用户的网盘里，" +
			"需手动去 App 里取消分享（旧实现在这里静默跳过）"
		e.run.progress(core.LevelError,
			fmt.Sprintf("分享文件取消失败 %s %d：%s", name, id, problem))
		return true, []string{problem}
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("分享文件成功 %s %d", name, id))
	if derr := e.src.up.AppCloudDelOutLink(
		ctx, auth, deviceID, account, []string{*shared.LinkID},
	); derr != nil {
		problem := "取消分享失败：" + derr.Error() + " ⇒ 那条外链仍在网盘里，需手动去 App 里取消"
		e.run.progress(core.LevelError,
			fmt.Sprintf("分享文件取消失败 %s %d：%s", name, id, derr.Error()))
		return true, []string{problem}
	}
	// 旧实现这一句**没有**（它删完不吭声）。这里补一行 info：这一族是"建了又删"，
	// 少了这一行，日志里只有"分享文件成功"，读的人会以为网盘里留着一条分享。
	e.run.progress(core.LevelInfo, fmt.Sprintf("已取消分享 %s %d", name, id))
	return true, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 云笔记那一半（107：建一条随机笔记、再把它移进回收站）
// ─────────────────────────────────────────────────────────────────────────────

// noteRecycleWait 是"建完等到再删"的那一点间隔（旧实现 `runners.ts:63` 的 `sleep(2000)`）。
//
// 它与 [taskMatrixPace]（500ms）**不是一个值**，也不合并：那 500ms 是"发完一枪打卡之后"
// 的通用节奏，这一格是"等上游把刚建的那条笔记落库"。⚠ 两格都没有出处说明为什么是这个数
// （与 `taskMatrixPace` 头上同一句：旧实现为什么是 2000ms 没有成文）。
const noteRecycleWait = 2 * time.Second

// runNote 跑玩法 107 的本体，交回**有没有真的发出过请求**与逐格的原因。
//
// 形状来自**在跑的** Go 参考实现（caiyun `handleNoteTask` / `createTempNote`）：
// 换 `app_auth` → 建一条随机笔记 → 等 2s → 移进回收站。
// ⚠ **不是**旧 TS：那一条 runner 从没跑到过后两枪（成文见
// [`internal/upstream/app_note.go`](../upstream/app_note.go) 文件头与
// `docs/business-feature-inventory.md:383`）⇒ 这里连"照抄"都算不上，
// 只能算"按一个在跑的实现落形状"。三口的判据缺口一并写在那儿。
//
// # `authToken` 从哪儿来
//
// 两家的调用方都是"配置里那枚 token"，而本仓没有那一格配置（登录态只有库里那串
// `Authorization`）。它就是 `Basic base64("<前缀>:<手机号>:<authToken>")`，
// 所以这里现拆：[core.ParseAuth] 交回的 `Token` 正是 `authToken`。
// ⚠ 这一格是**推断**（"整串第三段 = 登录 token"由 [`internal/core/auth.go`](../core/auth.go)
// 的 `ParseAuth` 成文支撑，不是"笔记服务认这一枚"的实测）⇒ 真跑时如果上游在这一口回
// 鉴权失败，第一个要怀疑的就是它，而不是请求形状。
//
// # ⚠ 这一族的产物**删不到底**
//
// `moveToRecycleBin` 只是移进回收站，本仓没有物删那一口 ⇒ 每次真跑都会在用户的
// 笔记回收站里留一条随机标题的笔记（直到上游自己清）。它**不**记进
// [TaskMatrixRunResult.CloudFileIDs]：那一条链清的是网盘文件，把笔记 id 塞进去
// 等于往另一个服务发删除。这一格登记在 `docs/pending-decisions.md` **R32**。
func (e *taskExecutor) runNote(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	if e.run.Random == nil {
		return false, []string{"云笔记没做成：随机源没接，造不出 noteid 与标题（本工具侧缺接线）"}
	}
	authorization := e.opening.Credential.Authorization()
	phone := e.opening.Credential.Phone()
	authToken := ""
	if info, ok := core.ParseAuth(authorization); ok && info.Token != nil {
		authToken = *info.Token
	}
	if authToken == "" || phone == "" {
		// 错误文本里**不带**任何一项的值：两枚都是凭据 / 号码（成文见 `docs/sanitize-guide.md`）。
		return false, []string{
			"云笔记没做成：这个号的登录态里拆不出 authToken 或手机号 ⇒ 一枪不发" +
				"（笔记那一族的三口全靠它们，缺一个就凑不出发出去的东西）",
		}
	}
	appAuth, perr := e.src.up.AppNoteAuthToken(ctx, authorization, authToken, phone)
	if perr != nil {
		// 与旧 TS 同向：这一口拿不到 `app_auth` ⇒ 后两枪**不发**，整条停在"没做成"。
		// ⚠ 但旧实现在这里是**静默跳过**（`logger.info` 一句、还把那条任务记进"本次成功"），
		// 这里交回失败原因，让它落在"这一条没做成"那一档。
		problem := "换笔记鉴权失败：" + perr.Error() + " ⇒ 后两枪不发，不会在笔记里留下东西"
		e.run.progress(core.LevelError, fmt.Sprintf("云笔记失败 %s %d：%s", name, id, problem))
		return true, []string{problem}
	}
	// 一发出网了：从这里往下都算"这一条今天动过"（口径见 [apply]）。
	now, hasNow := e.nowMillis()
	if !hasNow {
		return false, []string{
			"建笔记没做成：时钟没接，`createtime` / `updatetime` / `visitTime` 三格没有值可填" +
				"（本层不读 `time.Now()`，见 `app_note.go`）⇒ 不再发后两枪",
		}
	}
	noteID := randomHex(e.run.Random, 32)
	title := randomHex(e.run.Random, 3)
	if cerr := e.src.up.AppNoteCreate(ctx, authorization, appAuth, upstream.NoteCreateInput{
		NoteID: noteID, Title: title, Phone: phone,
	}, now); cerr != nil {
		problem := "建笔记失败：" + cerr.Error() + " ⇒ 没删（这一趟没在笔记里留下东西" +
			"，除非上游其实建成功了却没说）"
		e.run.progress(core.LevelError, fmt.Sprintf("云笔记失败 %s %d：%s", name, id, cerr.Error()))
		return true, []string{problem}
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("云笔记已建 %s %d", name, id))
	if werr := e.wait(ctx, noteRecycleWait); werr != nil {
		// 等不住 ⇒ **照发**删除那一枪：跳过等待的代价是"上游可能还没落库、删不掉"，
		// 而不发那一枪的代价是"一定留一条笔记在用户账号里"。前者可重试，后者不可见。
		e.run.progress(core.LevelWarn, fmt.Sprintf(
			"云笔记没等够 2 秒（%s）⇒ 直接发删除，不等了", werr.Error()))
	}
	if derr := e.src.up.AppNoteMoveToRecycleBin(ctx, authorization, appAuth, noteID); derr != nil {
		problem := "移进回收站失败：" + derr.Error() +
			" ⇒ 那条笔记**留在用户的云笔记里**（标题是随机串，需手动去找并删除）"
		e.run.progress(core.LevelError, fmt.Sprintf("云笔记删除失败 %s %d：%s", name, id, derr.Error()))
		return true, []string{problem}
	}
	// 与外链那一族同一条纪律：这一族是"建了又删"，少了这一行，日志里只有
	// "云笔记已建" ⇒ 读的人会以为账号里留着一条笔记。⚠ 也别读成"清干净了"：
	// 它只在**回收站**里（R32），所以这里念的是"已移进回收站"。
	e.run.progress(core.LevelInfo, fmt.Sprintf("云笔记已移进回收站 %s %d", name, id))
	return true, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 计数与结论
// ─────────────────────────────────────────────────────────────────────────────

// absorb 把一条记录并进计数。
//
// ⚠ 类别与"成没成"是两格：[StepClick] 也可能带着 `Err`（发出去了但上游说失败）。
// 把它们混成"成功/失败"两档的表现是：`Clicked` 里混进失败的枪，而复查那趟
// "做了却没认"的数不上涨 ⇒ 两头的数互相对不上，谁也不知道该信哪一个。
func (c *TaskRunCounts) absorb(record TaskStepRecord) {
	c.Uploaded += record.Uploads
	failed := record.Err != ""
	// 一条失败如果是"没出网"那种，两格都要动：`Failed` 是"这一条没做成"的总数，
	// `FailedNoShot` 是它里面"根本没发出去"的那一半（小结按这两格分档措辞）。
	failedNoShot := failed && !record.Fired
	switch record.Kind {
	case StepClick:
		if failed {
			c.Failed++
			if failedNoShot {
				c.FailedNoShot++
			}
			return
		}
		c.Clicked++
	case StepRunner:
		if failed {
			c.Failed++
			if failedNoShot {
				c.FailedNoShot++
			}
			return
		}
		c.Ran++
	case StepBlockedToolGap:
		c.ToolGap++
	case StepBlockedByRule:
		c.RuleSkipped++
		// 这一态也可能真传了文件（beiyong1 的门只挡打卡）。传了却没失败时它既不进
		// `Clicked` 也不进 `Ran`，只体现在 `Uploaded` 那一句里 —— 别把它念成"什么都没做"。
		if failed {
			c.Failed++
			if failedNoShot {
				c.FailedNoShot++
			}
		}
	case StepNeedsHuman:
		c.NeedsHuman++
	}
}

// absorbVerify 并复查那一格。
func (c *TaskRunCounts) absorbVerify(class TaskVerifyClass) {
	switch class {
	case VerifyDoneNow:
		c.DoneNow++
	case VerifyFailed:
		c.NotAcknowledged++
	case VerifyAlreadyWas:
		c.AlreadyWas++
	case VerifyStillOpen:
		c.StillOpen++
	case VerifyHidden:
		c.Hidden++
	}
}

// SummaryLine 是一句人话结论。
//
// 顺序是刻意的：先说"这趟做了什么"，再说"什么没做、为什么"，最后才是复查读数。
// 反过来写会把"上游没认"这件最该看见的事埋在一堆计数后面。
func (res TaskMatrixRunResult) SummaryLine() string {
	c := res.Counts
	parts := []string{fmt.Sprintf("任务表 %d 条（待做 %d 条）", c.Read, c.Pending)}
	if c.Clicked+c.Ran > 0 {
		parts = append(parts, fmt.Sprintf("本次上报 %d 条（打卡 %d、玩法 %d）",
			c.Clicked+c.Ran, c.Clicked, c.Ran))
	}
	if c.Failed > 0 {
		parts = append(parts, res.failurePhrase())
	}
	if c.Uploaded > 0 {
		// 这一句必须带**结局**：这一族会在用户网盘里写文件，而"写了之后删没删"是
		// 跑过之后唯一需要人工跟进的事实。2026-09-25 之前这里恒写"（未清理）"
		// （那时删除那一枪确实没有调用点），现在按 [TaskMatrixRunResult.Cleanup] 分流。
		parts = append(parts, fmt.Sprintf("网盘新增 %d 个文件%s", c.Uploaded, res.cleanupPhrase()))
	}
	if n := res.CloudProductFiles; n > 0 {
		// ⭐ 上游**另存**出来的那一半要单独说（2026-09-26 第廿三趟真跑跑出来的错）：
		// 它既不是"我们传上去的几个"（所以不在 `c.Uploaded` 里），也不是"传了一半的坑"，
		// 而是玩法 1014 的 ⑥ 在网盘里另存的一份产物。混进下面那句减法就会念成
		// "另有 1 个占位未传完"—— 而它传完了、也被删了，用户据此去查"哪个坑没填"是白查。
		parts = append(parts, fmt.Sprintf(
			"另有 %d 个是上游另存的转换产物，也在同一份清理名单里", n))
	}
	if n := len(res.CloudFileIDs) - c.Uploaded - res.CloudProductFiles; n > 0 {
		// 传失败但**已经占下坑**的那些（编排层刻意多交 id，见 `cloud_upload.go` 文件头）。
		// 不与上一句合并：那一格说"传成了几个"，这一格说"还有几个没传完也要删"。
		parts = append(parts, fmt.Sprintf("另有 %d 个占位未传完，也在同一份清理名单里", n))
	}
	if c.ToolGap > 0 {
		parts = append(parts, fmt.Sprintf("%d 条本工具还没接上玩法", c.ToolGap))
	}
	if c.NeedsHuman > 0 {
		parts = append(parts, fmt.Sprintf("%d 条要人去 App 里开一下", c.NeedsHuman))
	}
	if res.VerifiedAgain > 0 {
		parts = append(parts, fmt.Sprintf("复查：认下 %d、没认 %d、此前已完成 %d、仍待办 %d",
			c.DoneNow, c.NotAcknowledged, c.AlreadyWas, c.StillOpen))
	} else {
		parts = append(parts, "未复查")
	}
	if res.ReadFailures > 0 {
		parts = append(parts, fmt.Sprintf("有 %d/%d 组没读到（这不是全表）",
			res.ReadFailures, res.Fetches))
	}
	line := strings.Join(parts, "；")
	if len(res.Notes) > 0 {
		line += "。" + strings.Join(res.Notes, "。")
	}
	return line
}

// failurePhrase 是小结里那句"有几条没做成"，按**有没有出过网**分三档。
//
// 为什么要分（2026-09-26 整趟真跑跑出来的错）：原来这一句无条件写"N 条上报失败"，
// 而当天号二那句底下两条里**只有一条真上报过** ——
// 434 那条是"这一趟前面没有任务往网盘传过东西 ⇒ 一枪没发"，1020 那条才是
// "发了 ⑧、上游回 404"。两种在界面上长得一样，用户下一步要做的完全不同
// （前者是 Q41 那一问，后者是去查这一族的入口）。
//
// 反向改坏它：把三档合回一句 ⇒ `TestSummaryLineSeparatesUnsentFromFailedReports` 红
// （它同时钉"全没出网时不许出现『上报』"与"真发过枪那一半仍要写『上报失败』"）。
func (res TaskMatrixRunResult) failurePhrase() string {
	c := res.Counts
	switch {
	case c.FailedNoShot == 0:
		return fmt.Sprintf("%d 条上报失败", c.Failed)
	case c.FailedNoShot == c.Failed:
		return fmt.Sprintf("%d 条没做成（一枪都没发：缺前置）", c.Failed)
	default:
		return fmt.Sprintf("%d 条没做成（%d 条发出去了、%d 条一枪没发：缺前置）",
			c.Failed, c.Failed-c.FailedNoShot, c.FailedNoShot)
	}
}

// cleanupPhrase 是小结里"网盘新增 N 个文件"后面那个括号。
//
// 用词刻意：**"移进回收站"而不是"已删除"** —— `batchTrash` 那一句的语义就是
// 进回收站（旧实现的函数名也叫 `deleteFiles（移至回收站）`），说"已删除"会让人
// 以为网盘里彻底没了、也就不会去回收站看那一份占位。
func (res TaskMatrixRunResult) cleanupPhrase() string {
	switch res.Cleanup {
	case TaskCleanupDone:
		return "（已移进回收站）"
	case TaskCleanupKept:
		return "（本次保留，未清理）"
	case TaskCleanupFailed:
		return fmt.Sprintf("（清理失败：%s）", res.CleanupReason)
	}
	// 落到这里只有"一个文件都没写过却带 Uploaded>0"这一种自相矛盾，
	// 以及清理没跑成的兜底 ⇒ 保守地说"未清理"。
	return "（未清理）"
}

// RunReport 把这一趟映射到四态结论。
//
// | 情形（自上而下第一个命中） | 结论 |
// | --- | --- |
// | 有"要人去 App 做"的（未开备份） | [core.OutcomeManualRequired] |
// | 复查说"做了但没认" / 有枪发失败 / 有玩法缺口 | [core.OutcomePartialSuccess] |
// | 其余（含"今天没有待做的任务"） | [core.OutcomeSuccess] |
//
// ⚠ "一条枪都没发"**不是**失败：一个当天没有待做任务的号跑出来就是零条，
// 判成失败会让每天的历史里全是噪声（与 [TaskListResult.RunReport] 同一条理由）。
//
// ⚠ [StepBlockedToolGap] 落 PartialSuccess 而不是 ManualRequired：那是**本工具的**缺口，
// 要人去 App 点什么并不能补上它 —— 把它报成"需人工"会把工单派错人。
func (res TaskMatrixRunResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	c := res.Counts
	manual := []core.ManualStep{}
	if c.NeedsHuman > 0 {
		account := res.Account
		manual = append(manual, core.NewManualStep(appTaskCapabilityCode, &account,
			"有几条每月任务要先在 App 里开启自动备份才能做（本工具不代开）",
			core.TargetMobileApp))
	}
	outcome := core.OutcomeSuccess
	if c.NotAcknowledged > 0 || c.Failed > 0 || c.ToolGap > 0 {
		outcome = core.OutcomePartialSuccess
	}
	if len(manual) > 0 {
		outcome = core.OutcomeManualRequired
	}
	return core.RunReport{Outcome: outcome, ManualSteps: manual, Summary: &summary}
}

// derefTaskID 取 id（没有 id 的条目交回 0 —— 那种条目在分派层就被挡了，不会走到执行）。
func derefTaskID(item upstream.TaskV2Item) int64 {
	if item.TaskID == nil {
		return 0
	}
	return *item.TaskID
}

// errText 把错误摊成一句文本（nil ⇒ 空串）。
func errText(err *core.Error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// joinProblems 把若干段"这一条没做成的原因"拼成一句（空段丢掉，段间用 `；`）。
//
// 为什么要有这一枚而不是各处直接 `strings.Join`：三个调用点都会带空串（成功时
// 错误文本本来就是空），拼出来若留着分隔符，界面上就会出现一条以"；"开头的空原因
// —— 而 [TaskStepRecord.Err] 非空**就是**"这一条有失败"的判据（见 [TaskRunCounts.absorb]），
// 多一个假失败比少一句解释严重得多。
func joinProblems(parts []string) string {
	kept := make([]string, 0, len(parts))
	for _, part := range parts {
		if strings.TrimSpace(part) != "" {
			kept = append(kept, part)
		}
	}
	return strings.Join(kept, "；")
}

// appTaskLocation 是这一族"本地日历"的口径（东八区）。
//
// 与 [upstream.ExchangeTimeLocation] / `FunaiActivityLocation` 同值不同名，
// 那两处是各自族自己的常量；这里单列一份，是为了让"哪一族的门用哪份日历"
// 这件事在各自文件里可读，而不是三族共享一个来路不明的全局时区。
func appTaskLocation() *time.Location { return time.FixedZone("UTC+8", 8*3600) }
