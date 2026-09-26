package app

// 「任务拓展」（旧实现 `taskExpansion`）：读一次状态 →（本月没开备份就**传一个文件凑**）
// → 等一会儿 → 重读一次 →（这个月攒下记录就）领掉 → 顺带念一句"下月可领多少"。
//
// 对应旧实现 `packages/core/src/tasks/task-expansion.ts`（那份只有 64 行，
// 三条判据全在里面，本文逐条对齐；下面的"刻意差别"就是它与旧实现的**全部**不同）。
//
// # 这一条为什么要做成独立能力
//
// 它是旧项目里**唯一**会"为了改变上游状态而写用户网盘"的任务：
// `curMonthBackup === false` 时它不是提示用户去开备份，而是**自己传一个文件**
// 把这一格刷成 true（`task-expansion.ts:44` 调 `backFile` →
// `uploadRandomFile({opType:"backup", parentFileId:"/"})`）。
// 本仓其余写网盘的链（`app-task` 的 106/522、`ai-avatar`）传文件是**任务要求**，
// 而这里的传文件是**冒充任务要求** —— 所以这一条 `RunInBatch: false`，
// 只有界面上点那一下才会真的写。
//
// # 证据面（写清楚哪一格有实拍、哪一格没有）
//
//   - **读口有实拍**：`test/fixtures/http/3d.json` 两条，请求与响应都在，
//     那一条读到的是 `curMonthBackup:true` + `curMonthTaskRecordCount:0`
//     ⇒ 走的是"**跳过凑备份**"与"**不去领**"那两支。
//   - **凑备份那一支零实拍**：夹具那条压根没进这一支。所以"传上去之后 `curMonthBackup`
//     会不会变 true""20 秒够不够"**都没有样本** —— 唯一的定法是真跑一次
//     （`AGENTS.md` §1.13：缺口的默认处置是去跑，不是去等抓包）。
//   - **领取口零实拍**：`receiveTaskExpansion` 的响应本仓一条都没有
//     （见 `upstream.ReceiveTaskExpansion` 头注），`cloudCount` 给不给都不知道。
//
// # 与旧实现的三处**刻意**差别（其余逐字对齐）
//
//  1. **上传一枪都没发出去时不空等**。旧实现的 `backFile(ctx)` 返回值在调用点被丢掉
//     （`task-expansion.ts:44`），所以"文件备份失败"之后**照样 sleep 20 秒再重读**。
//     那一等是"给服务端时间把这次上传记进当月备份"，而**没有上传就没有东西要记** ⇒
//     这里只在"真的发出过 `file/create`"之后等（含秒传与撞同名两档：上游确实收到了
//     一次备份类事件）。少等只省 20 秒，多等一次只花 20 秒 —— 不对称的地方在
//     "等错了没人知道"，所以宁可少做一件没依据的事。
//     ⚠ 少等的那一支**仍然重读**（"一枪没发"时也一样）：重读是只读、无害，
//     而结论要用最新那一份 ⇒ 这一条偏差只压在"等"上，没有压"读"。
//  2. **领取成功但上游没报数，仍然报"领了"**。旧实现读的是
//     `!receiveErr && receiveData?.result?.cloudCount`（`task-expansion.ts:56`）——
//     这是个 truthy 判据，于是"`code:0` 但 `result` 空"那一趟它**一句都不念**，
//     看起来像什么都没发生。写请求发出去且上游认了，这是实况，就得念出来
//     （与 `notice` 那条 `reward` 不报数时"报已领、括号里说拿不到数"同一口径）。
//  3. **重读失败时把"已经传上去了"交回**。旧实现那句是
//     `logger.error("获取任务扩展失败") + return`，于是用户的网盘里多了一个文件，
//     而日志上这一趟看起来什么都没做。这里结论照报失败，`Upload` 那一格写明传过、
//     并把文件名念出来（与 `cloud_upload.go` 文件头"complete 失败仍交回 fileId"同族）。
//
// # ⚠ 这一条链**不删**自己写的文件
//
// 三点合起来决定这件事：
//   - 旧实现不删（它传完就走）；
//   - "把那个文件移进回收站之后，本月的备份读数会不会回滚"**零样本** ——
//     按 §1.7，两个方向都不许断言，所以也不能拿"删掉更干净"当依据去动它；
//   - 清理那一口（`recyclebin/batchTrash`）今天只在 `app-task` 那条链的链尾有调用点。
//
// ⇒ 处置：文件名与 id **念进事件流**，让人自己去网盘里处置；
// "要不要接自动清理"留成待拍板（`docs/pending-decisions.md`），不是这里默默选掉。
//
// # jwttoken 的来源：这一页用**公共来源**
//
// 夹具那两条带的 `jwttoken` 是**脱敏**的，而换到那张票的链路没被抓到
// ⇒ 按 `AGENTS.md` §1.12 复用既有建会话入口（签到页配方 `RecipeSignInPage`，
// 即 [SignInWorld.Source]），在此声明"**jwttoken 使用公共来源**"。这是**临时态**：
// 哪天抓到「任务拓展」页自己的取票链路，就改用特有取法、把这段改掉，
// 并把新链路补进 `docs/capture/`。
// ⚠ 连带未验证：这一口要不要 `referer` 指向自己那一页。实拍两条的 `referer` 值被脱敏，
// 看不出来 —— 今天带的是签到页那一份（与同页其余接口一样）。

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// taskExpansionCapabilityCode 是这条能力在注册表里的 code（一处定义，
// [core.ManualStep] 与分派表都指回 [capabilities.CodeTaskExpansion]）。
const taskExpansionCapabilityCode = capabilities.CodeTaskExpansion

// defaultTaskExpansionBackupWait 是"传完等到重读"的那一格时长。
//
// 旧实现读的是 `ctx.config.backupWaitTime || 20`（秒）—— 一格**配置项**。
// 这里做成注入点（[TaskExpansionRun.BackupWait]）而不是配置项：
// 差别记在这儿，不伪装成等价。真实时长够不够**零样本**（夹具那条没走这一支）。
const defaultTaskExpansionBackupWait = 20 * time.Second

// taskExpansionRetries 是"凑完备份再读一遍"的**上限**。
//
// 逐字对齐旧实现那个 `timer`（`_taskExpansion(ctx, 1)`，重入时 `timer-1 == 0` ⇒ 不再凑）：
// 传了两次还没刷上去，说明这一支不是"再传一个"能解决的，继续传就是往用户网盘里堆文件。
//
// ⚠ 这一格是**那条上限的名字**，代码里没有读它的分支：本文件走的是"一遍 + 至多一次重读"
// 的直写流程，"至多传一个文件"是由流程结构（只有**第一遍**那一次能进
// [taskExpansionShouldUpload] 那一格）保证的，并由
// `TestTaskExpansionUploadsAtMostOneFile` 钉住。谁把这里改成循环，必须让那条上限**真的**
// 参与判断（否则那条测试会红，而这一格仍只是个装饰）。
const taskExpansionRetries = 1

// ─────────────────────────────────────────────────────────────────────────────
// 判定（**纯函数**，不碰网络）
// ─────────────────────────────────────────────────────────────────────────────

// taskExpansionShouldUpload 报告"这一份读数要求去凑备份"。
//
// ⚠ 判据是 `curMonthBackup` **读到 false**，不是"没读到 true"：
// 旧实现那句 `if (curMonthBackup === false)` 是严格判，nil（上游没给这一格）
// 走的是"不凑"。这一条不是抠字 —— 凑备份会**往用户网盘写一个文件**，
// 把"上游没说"读成"上游说了没开"就是拿一次不可回滚的写去补一个空洞。
func taskExpansionShouldUpload(read upstream.TaskExpansion) bool {
	return read.CurMonthBackup != nil && !*read.CurMonthBackup
}

// taskExpansionShouldClaim 报告"这一份读数要求去领"。
//
// 逐字对齐 `curMonthTaskRecordCount > 0`。⚠ 没读到（nil）也**不领**：
// 领是要发写请求的，而"记录数为 0"与"上游没说记录数"在念出来时是两句话
// （见 [taskExpansionNothingReason]），在发不发这一枪上却是同一个处置。
func taskExpansionShouldClaim(read upstream.TaskExpansion) bool {
	return read.CurMonthTaskRecordCount != nil && *read.CurMonthTaskRecordCount > 0
}

// taskExpansionJudged 报告"这一份读数至少给出一格判据"。
//
// 两格都没有 ⇒ [TaskExpansionStatusUnreadable]：一个请求都不发，
// 并把"上游回了 code:0 但什么都没读出来"说清楚 —— 与 [PlanNotice] 里
// `Unknown` 那一档同一条理由（当成"没得领"是一句没根据的肯定话）。
func taskExpansionJudged(read upstream.TaskExpansion) bool {
	return read.CurMonthBackup != nil || read.CurMonthTaskRecordCount != nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 档位
// ─────────────────────────────────────────────────────────────────────────────

// TaskExpansionStatus 是这一趟的结论档位。
type TaskExpansionStatus int

const (
	// TaskExpansionStatusNone 是"**没走到能报结论的那一步**"（读第一遍就挂了）。
	//
	// ⚠ 刻意占零值：[TaskExpansionRun.RunAccount] 有好几条提前 return 错误的路径，
	// 那些路径上结构体是零值。零值若是"领到了"，一句"领到 N 朵"就会在
	// **一个请求都没发**的那趟上被念出来（与 [NoticeOutcomeNone] 同一条理由）。
	TaskExpansionStatusNone TaskExpansionStatus = iota
	// TaskExpansionStatusClaimed 是领取那一发**发出去且上游认了**。
	// 朵数在 [TaskExpansionResult.CloudCount]，可能为 nil（上游没报，见"刻意差别"第 2 条）。
	TaskExpansionStatusClaimed
	// TaskExpansionStatusNothing 是**没得领**（结论那次读到的记录数是 0 或没读到）。
	// 算**成功**：这一趟是"该领的时候领掉"，来一趟确认"现在不该领"就是达成了。
	TaskExpansionStatusNothing
	// TaskExpansionStatusBackupManual 是**凑过一次备份之后仍未开**，且这一趟没领到东西
	// ⇒ 要人去 App 里开自动备份。
	//
	// 与 [NoticeManual] 那一档不同，这一条**今天可达**：判据就是重读那一次的
	// `curMonthBackup === false`，旧实现也是这么判的（它那句日志是
	// "本月未开启备份，将无法获取翻倍奖励"）。
	TaskExpansionStatusBackupManual
	// TaskExpansionStatusUnreadable 是**判不了**：两格判据都没读到 ⇒ 一枪不发。
	TaskExpansionStatusUnreadable
	// TaskExpansionStatusClaimFailed 是领取那一发**发了但失败**。
	//
	// 不返回错误：请求发过、没成，这是实况；报成错误会把"发过"这件事抹掉
	// （与 [NoticeOutcomeFailed] 同一条处置）。
	TaskExpansionStatusClaimFailed
)

// TaskExpansionUpload 是"凑备份那一发上传"的结局档位。
//
// 它与 [TaskExpansionStatus] 是**两个轴**：上传成没成 ≠ 这一趟领没领到
// （旧实现里这两件事在同一条链上但互不门控 —— 上传失败它仍会重读、仍会去领）。
type TaskExpansionUpload int

const (
	// TaskExpansionUploadNotNeeded 是**没凑**：备份本来就开着，或那一格没读到。
	TaskExpansionUploadNotNeeded TaskExpansionUpload = iota
	// TaskExpansionUploadDone 是四枪走完、网盘里多了一个我们造的文件。
	TaskExpansionUploadDone
	// TaskExpansionUploadRapid 是**秒传**（`rapidUpload:true`）：后两枪没发，
	// 但上游收到了这一次备份类事件。⚠ 这一档本仓**零样本**（见 `cloud_upload.go` 文件头）。
	TaskExpansionUploadRapid
	// TaskExpansionUploadExists 是**撞上用户已有同名文件**（`exist:true`）。
	// ⚠ 那一个不是我们造的 ⇒ id **绝不**进任何清理名单（删它就是删用户的东西）。
	TaskExpansionUploadExists
	// TaskExpansionUploadBlocked 是**一枪都没发**：前置缺（取不到 deviceId，或随机源没接）。
	//
	// 单独一档是因为它与"发了但失败"要分开：前者是本工具侧缺接线，
	// 后者是上游侧的事，处置完全不同。
	TaskExpansionUploadBlocked
	// TaskExpansionUploadFailed 是上传**发过、没成**。
	TaskExpansionUploadFailed
)

// needsWait 报告"这一档上传之后要不要等一会儿再重读"（"刻意差别"第 1 条的落点）。
//
// 等的是"服务端把刚刚这一次备份事件记进当月" ⇒ 前提是**事件真的发出去了**。
// [TaskExpansionUploadFailed] 里也含"create 发出去了但 PUT / complete 挂了"那种
// （见 [CloudUploadReceipt] 那个"错误路径也可能带 FileID"的说明）—— 那种情况
// 网盘里已经有坑了，所以仍等。只有"一枪没发"（Blocked / NotNeeded）不等。
func (u TaskExpansionUpload) needsWait() bool {
	return u != TaskExpansionUploadNotNeeded && u != TaskExpansionUploadBlocked
}

// taskExpansionUploadWord 是档位的人话名（播报与小结共用一份，免得两处漂移）。
func (u TaskExpansionUpload) word() string {
	switch u {
	case TaskExpansionUploadDone:
		return "传了一个文件，没删"
	case TaskExpansionUploadRapid:
		return "秒传，没删"
	case TaskExpansionUploadExists:
		return "撞上同名（那个文件不是我们造的）"
	case TaskExpansionUploadBlocked:
		return "一枪没发"
	case TaskExpansionUploadFailed:
		return "没传成"
	}
	return "没凑"
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// TaskExpansionRun 跑一次任务拓展。
type TaskExpansionRun struct {
	SignInWorld

	// Wait 是"等一会"的注入点；nil ⇒ [RealWaiter]（**默认必须真的等**，
	// 理由同 [AiAvatarRun.Wait]：这一等等的就是"服务端有没有记上这次备份"，
	// 不等就重读等于把这一支测成"传完立刻读"，而那**不是**这条链的语义）。
	Wait Waiter

	// BackupWait 是凑过备份之后那一等；零 ⇒ [defaultTaskExpansionBackupWait]。
	//
	// 做成字段而不是配置项：测试要能把它压成极小值，才能钉"等了几次、等了多久"
	// 而不真睡 20 秒（同一手法见 `ai_avatar_run.go` 那三处等待）。
	BackupWait time.Duration
}

// TaskExpansionResult 是一个账号这一趟的产出。
type TaskExpansionResult struct {
	Account core.AccountID
	Status  TaskExpansionStatus

	// Upload / UploadFileID / UploadName / UploadError 是"凑备份那一发"的四个读数。
	// FileID 与 Name **只在真的发过上传时**非空（含失败路径上已占坑的那一枚 ——
	// 那是 `cloud_upload.go` 文件头多交回来的那一个，这里念出来是为了让人能自己去删）。
	Upload       TaskExpansionUpload
	UploadFileID string
	UploadName   string
	UploadError  string
	// WaitedTime 是这一趟**实际**等了多久（零 = 没等，见"刻意差别"第 1 条）。
	WaitedTime time.Duration
	// BackupRetries 是凑过几次备份（0 或 1，上限见 [taskExpansionRetries]）。
	BackupRetries int

	// First / Last 是第一遍与**结论依据那一次**的读数。
	// 没走凑备份那一支时两者指向同一份内容。
	First *upstream.TaskExpansion
	Last  *upstream.TaskExpansion
	// Reads 是读了几遍（1 或 2）—— 排障时要能看出这一趟**有没有**走凑备份那一支。
	Reads int

	// CloudCount 是领取那一发报回的朵数；nil = 上游没报（**不是** 0）。
	CloudCount *int64

	// BackupStillOff 是"凑过一次之后 `curMonthBackup` 仍读成 false"。
	// ⚠ 它是**独立的一位**，因为 [TaskExpansionStatusClaimed] 与它可以同时成立
	// （本月的记录照领，下月翻倍的前置仍没开）⇒ 档位的取舍见 [TaskExpansionRun.RunReport]。
	BackupStillOff bool

	// Reason 是"没领"那几档的人话依据。
	Reason string
	// Error 是失败原因（[TaskExpansionStatusClaimFailed] / 读失败那句原话）。
	Error string
	// Notes 是这一趟里**不改变处置**但该让人看见的事（例：这一发没带 deviceid）。
	Notes []string
}

// RunAccount 跑**一个账号**的这一笔。
//
// 三条链路上只有"读第一遍""重读"失败会把这一趟交回错误；
// 上传与领取各自的失败都**折成档位**（发过了就是实况，见上面那两条"刻意差别"）。
func (r TaskExpansionRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (TaskExpansionResult, *core.Error) {
	result := TaskExpansionResult{Account: account, Notes: []string{}}
	opening, perr := r.Open(ctx, account)
	if perr != nil {
		return TaskExpansionResult{Account: account}, perr
	}
	src := opening.Source

	// deviceid：实拍那两条**都带**，所以取一次；取不到**不拦这一口**（只读，
	// 少一个头最多被上游拒），但要把"这一发没带"念出来 —— 它决定了将来
	// 这一条真跑失败时该往哪查（与 [ReceiveV3Request] 那种"不带就回 615"不同族）。
	deviceID, derr := r.DeviceIDFor(ctx, account)
	var deviceArg *string
	if derr == nil && deviceID != "" {
		deviceArg = &deviceID
	} else {
		note := "这一发没带 deviceid（换不到 H5 deviceId）"
		if derr != nil {
			note += "：" + derr.Error()
		}
		result.Notes = append(result.Notes, note)
		r.progress(core.LevelWarn, note)
	}

	first, perr := r.read(ctx, src, deviceArg)
	if perr != nil {
		// 读第一遍就挂：这一趟没有任何结论可报（零值档位，见 [TaskExpansionStatusNone]）。
		// 账号仍要带上 —— 编排层那句失败事件认的就是这一格。
		return TaskExpansionResult{Account: account}, perr
	}
	result.First = &first
	result.Last = &first
	result.Reads = 1
	r.progress(core.LevelInfo, "任务拓展读数："+taskExpansionReadings(first))

	// ⚠ 只有**第一遍**要求凑备份（上限见 [taskExpansionRetries]）：
	// 第二遍再读到 false 就只报"仍未开"，不再传第二个文件。
	reading := first
	if taskExpansionShouldUpload(first) {
		upload, fileID, name, uploadErr := r.uploadBackup(ctx, opening, deviceArg)
		result.Upload, result.UploadFileID, result.UploadName, result.UploadError =
			upload, fileID, name, uploadErr
		result.BackupRetries = 1
		r.progress(core.LevelInfo, "凑备份："+upload.word()+
			taskExpansionNameTail(name, fileID, uploadErr))

		if upload.needsWait() {
			wait := r.backupWait()
			if werr := r.waiter().Wait(ctx, wait); werr != nil {
				// 等不住（ctx 取消）⇒ **不发**重读那一枪：后面的枪也一样会挂，
				// 而这里交回的是取消原因，比"重读失败"有用。
				result.Error = werr.Error()
				return result, werr
			}
			result.WaitedTime = wait
			r.progress(core.LevelInfo,
				fmt.Sprintf("等 %s 让服务端记下这次备份，然后重读", wait))
		}

		second, perr := r.read(ctx, src, deviceArg)
		if perr != nil {
			// "刻意差别"第 3 条：上传那一格的实况**已经**记在 result 里了，
			// 交回错误的同时把它留着（旧实现这里直接 return，网盘里多一个文件而日志上没事发生）。
			result.Error = perr.Error()
			return result, perr
		}
		result.Last = &second
		result.Reads = 2
		reading = second
		r.progress(core.LevelInfo, "重读任务拓展："+taskExpansionReadings(second))
		result.BackupStillOff = taskExpansionShouldUpload(second)
	}

	if !taskExpansionJudged(reading) {
		result.Status = TaskExpansionStatusUnreadable
		result.Reason = "上游回了 code:0，但 curMonthBackup 与 curMonthTaskRecordCount 两格都没读到"
		return result, nil
	}

	if !taskExpansionShouldClaim(reading) {
		if result.BackupStillOff {
			result.Status = TaskExpansionStatusBackupManual
			result.Reason = "凑过一次备份，重读仍读到 curMonthBackup=false"
		} else {
			result.Status = TaskExpansionStatusNothing
			result.Reason = taskExpansionNothingReason(reading)
		}
		r.progress(core.LevelInfo, result.SummaryLine())
		return result, nil
	}

	// 领取那一发用的是**同一条响应里的** acceptDate（旧实现逐字如此：
	// `receiveTaskExpansion(acceptDate)`，值取自解构出来的那一份）。
	claimed, perr := do(ctx, src, func(s SignInSession) (upstream.ReceiveTaskExpansion, *core.Error) {
		return src.up.AppReceiveTaskExpansion(ctx, s.jwt, s.referer, reading.AcceptDate)
	})
	if perr != nil {
		result.Status = TaskExpansionStatusClaimFailed
		result.Error = perr.Error()
		r.progress(core.LevelError, result.SummaryLine())
		return result, nil
	}
	result.CloudCount = claimed.CloudCount
	result.Status = TaskExpansionStatusClaimed
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// read 读一次任务拓展（只读，可重复调）。
func (r TaskExpansionRun) read(
	ctx context.Context, src *SignInSessionSource, deviceID *string,
) (upstream.TaskExpansion, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.TaskExpansion, *core.Error) {
		return src.up.AppTaskExpansion(ctx, s.jwt, s.referer, deviceID)
	})
}

// uploadBackup 往网盘传一个 `backup` 档的文件去凑备份（**写**，且**不删**）。
//
// 三样前置各有各的"缺了会怎样"，全部折成 [TaskExpansionUploadBlocked] 而**不是**报错：
// 这一趟的主目标是"该领的时候领掉"，凑备份只是给下一趟铺路，
// 凑不了要把原因念出来，但不该因此不发领取那一枪（旧实现也是继续往下走的）。
//
// 装配与 `task_matrix_run.go` 的 `cloudUploader` 同一套口径（`opType:"backup"`
// 只有 android 档读，所以 Channel 留零值走默认档）。
//
// ⚠ 上面那三句守卫在**链路用例里到不了**（随机源为 nil 时建会话那一枪先 panic，
// 凭据为 nil 时 [SignInWorld.Open] 先报错），它们由
// `TestTaskExpansionUploadBlockedGuards` **直接调这一层**钉住 —— 不是"没测的分支"。
func (r TaskExpansionRun) uploadBackup(
	ctx context.Context, opening SignInOpening, deviceID *string,
) (TaskExpansionUpload, string, string, string) {
	if deviceID == nil {
		// 旧实现那一族每一枪都带 `deviceid` ⇒ 缺它就不发（不赌"少一个头上游理不理"，
		// 这一族零样本）。
		return TaskExpansionUploadBlocked, "", "",
			"换不到 H5 deviceId，而这一族的每一枪都带 deviceid 头 ⇒ 不发"
	}
	if r.Random == nil {
		return TaskExpansionUploadBlocked, "", "", "随机源没接：造不出文件名与内容（本工具侧缺接线）"
	}
	if opening.Credential == nil {
		return TaskExpansionUploadBlocked, "", "", "取不到档案里的凭据（authorization）"
	}
	uploader := CloudUploader{
		Up:       r.Upstream,
		Rng:      r.Random,
		Auth:     opening.Credential.Authorization(),
		DeviceID: *deviceID,
	}
	receipt, perr := uploader.UploadRandom(ctx, CloudUploadOptions{
		Op: upstream.CloudOpBackup,
		// 旧实现逐字 `parentFileId: "/"` —— 传进**根目录**。
		ParentFileID: "/",
	})
	if perr != nil {
		// ⚠ FileID 照收：错误路径上占下的坑也得念出来（见 `cloud_upload.go` 文件头）。
		return TaskExpansionUploadFailed, receipt.FileID, receipt.Name, perr.Error()
	}
	switch receipt.Outcome {
	case CloudUploadDone:
		return TaskExpansionUploadDone, receipt.FileID, receipt.Name, ""
	case CloudUploadRapid:
		return TaskExpansionUploadRapid, receipt.FileID, receipt.Name, ""
	case CloudUploadExists:
		// ⚠ 这个 id **不是**我们造的（用户网盘里本来就有同名文件）⇒ 只念名字、不交 id，
		// 将来谁接自动清理，绝不能把它放进清理名单（那是删用户的东西）。
		return TaskExpansionUploadExists, "", receipt.Name, ""
	default:
		// 上游没报错却给了零值档位 —— 今天构造不出来（`UploadContent` 的成功路径
		// 必定把 Outcome 设成三档之一）。不静默当成功：宁可念一句"档位没认出来"。
		return TaskExpansionUploadFailed, receipt.FileID, receipt.Name,
			"上传返回的结局档位是" + receipt.Outcome.String()
	}
}

// taskExpansionNameTail 把"文件名 / fileId / 原因"拼成播报的括号那一段。
//
// ⚠ 只有文件名与我们的 fileId 进这一句 —— 它是**出网**的事件流，
// `authorization` 只在 [CloudUploader] 里过一次，不出这扇门。
func taskExpansionNameTail(name, fileID, reason string) string {
	parts := []string{}
	if name != "" {
		parts = append(parts, name)
	}
	if fileID != "" {
		parts = append(parts, "fileId "+fileID)
	}
	if reason != "" {
		parts = append(parts, reason)
	}
	if len(parts) == 0 {
		return ""
	}
	return "（" + strings.Join(parts, "，") + "）"
}

// backupWait 取这一趟要等的那一格时长。
func (r TaskExpansionRun) backupWait() time.Duration {
	if r.BackupWait != 0 {
		return r.BackupWait
	}
	return defaultTaskExpansionBackupWait
}

func (r TaskExpansionRun) waiter() Waiter {
	if r.Wait == nil {
		return RealWaiter{}
	}
	return r.Wait
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 三句纪律：
//   - 朵数为 nil 时念"**上游没报数**"而不是 0；
//   - 读数只**照抄**（`curMonthBackup=false`），不解释成"这个号没开自动备份"
//     —— 那一格的语义来自旧实现的名字与那句日志，本仓没有正面样本；
//   - "下月可领"那一格照旧只在 >0 时念（旧实现 `if (nextMonthTaskRecordCount)`
//     是 truthy 判，0 与 nil 都不念）。
func (res TaskExpansionResult) SummaryLine() string {
	head := "任务拓展"
	switch res.Status {
	case TaskExpansionStatusClaimed:
		text := head + "已领"
		if res.CloudCount != nil {
			text = fmt.Sprintf("%s领到 %d 朵", head, *res.CloudCount)
		} else {
			text += "（上游没报朵数）"
		}
		if res.BackupStillOff {
			text += "；本月备份仍读成 false，翻倍的前置没开"
		}
		return text + res.nextMonthTail() + res.uploadTail()
	case TaskExpansionStatusNothing:
		return head + "没领：" + res.Reason + res.readingsTail() +
			res.nextMonthTail() + res.uploadTail()
	case TaskExpansionStatusBackupManual:
		return head + "没领：" + res.Reason + res.readingsTail() +
			res.nextMonthTail() + res.uploadTail()
	case TaskExpansionStatusUnreadable:
		// 判不了那一档**也要带读数**：这一档的全部意义就是"上游回了 code:0 而我们
		// 读不出判据"，此时最有信息量的东西就是它到底给了什么（例：只有
		// `nextMonthTaskRecordCount` 在 ⇒ 形状变了，而不是这个号恰好空的）。
		// ⚠ 不带上句"下月可领 N 朵"：结论都没落地时念附送信息，会把注意力带走。
		return head + "判不了：" + res.Reason + res.readingsTail()
	case TaskExpansionStatusClaimFailed:
		return head + "失败：领取那一发没成 —— " + res.Error + res.uploadTail()
	}
	// 零值那一档（没走到结论）：错误文本由调用方交回编排层，这里只念已知的部分。
	return head + "没办成" + res.uploadTail()
}

// nextMonthTail 是"下月可领 N 朵"那一句（旧实现 `if (nextMonthTaskRecordCount)`）。
//
// 只在 **>0** 时念：nil 与 0 在这一句里都无话可说，念出来会得到
// "下月可领 0 朵"这种看起来像结论的空话。
func (res TaskExpansionResult) nextMonthTail() string {
	if res.Last == nil {
		return ""
	}
	if res.Last.NextMonthTaskRecordCount != nil && *res.Last.NextMonthTaskRecordCount > 0 {
		return fmt.Sprintf("；下月可领 %d 朵", *res.Last.NextMonthTaskRecordCount)
	}
	return ""
}

// readingsTail 把**结论那一次**的读数附在后面（"没领"那一档也要看得见依据，
// 与 `noticeSlotReadings` 同一条理由：真跑一趟要能回答"到底是哪一格挡住的"）。
func (res TaskExpansionResult) readingsTail() string {
	if res.Last == nil {
		return ""
	}
	return "（读数：" + taskExpansionReadings(*res.Last) + "）"
}

// uploadTail 把上传那一格附在后面（"没办成"那几档也要看得见网盘里多没多东西）。
//
// ⚠ 这里**只念文件名，不念 fileId**：`runs.message` 是要长期存下来、进历史页与推送的
// 那一句，而 fileId 是排障用的对象标识 —— 事件流那一行已经带它了（见 [RunAccount] 里
// 那句"凑备份："）。与 `aiAvatarOutput` 那格"个数而不是那一串 id"是同一条取舍。
func (res TaskExpansionResult) uploadTail() string {
	if res.Upload == TaskExpansionUploadNotNeeded {
		return ""
	}
	reason := ""
	if res.Upload == TaskExpansionUploadBlocked || res.Upload == TaskExpansionUploadFailed {
		// 只有"没做成"那两档才把原因带进小结：档位的字（"传了一个文件，没删"）
		// 后面跟一句上游原文，是这一行唯一还能加信息量的地方。
		reason = res.UploadError
	}
	return "；凑备份" + res.Upload.word() + taskExpansionNameTail(res.UploadName, "", reason)
}

// taskExpansionNothingReason 给"没领"那一档一句**有根据的**依据。
//
// ⚠ 分开两句："记录数读到 0"与"记录数没读到"是不同的话，前者是有根据的"现在不该领"，
// 后者只是"我们没根据"（同一个处置：不发写请求）。
func taskExpansionNothingReason(read upstream.TaskExpansion) string {
	if read.CurMonthTaskRecordCount == nil {
		return "上游没给 curMonthTaskRecordCount（没读到 ≠ 0，所以这一枪不发）"
	}
	return fmt.Sprintf("本月记录数读到 %d，不是 >0", *read.CurMonthTaskRecordCount)
}

// taskExpansionReadings 把一份读数念成一行文本。**缺的键念成"缺"**，不念成 0 / false ——
// "上游没给这一格"与"上游给了 false"是两个处置（见 [taskExpansionShouldUpload]）。
//
// `acceptDate` 只念"有 / 缺"，**不念值**：它是上游给的一个日期串，念出来对人没用，
// 而这一句是出网的事件流（口径见 `docs/sanitize-guide.md`）。
func taskExpansionReadings(read upstream.TaskExpansion) string {
	parts := make([]string, 0, 4)
	if read.CurMonthBackup != nil {
		parts = append(parts, fmt.Sprintf("curMonthBackup=%v", *read.CurMonthBackup))
	} else {
		parts = append(parts, "curMonthBackup=缺")
	}
	if read.CurMonthTaskRecordCount != nil {
		parts = append(parts, fmt.Sprintf("curMonthTaskRecordCount=%d", *read.CurMonthTaskRecordCount))
	} else {
		parts = append(parts, "curMonthTaskRecordCount=缺")
	}
	if read.NextMonthTaskRecordCount != nil {
		parts = append(parts, fmt.Sprintf("nextMonthTaskRecordCount=%d", *read.NextMonthTaskRecordCount))
	} else {
		parts = append(parts, "nextMonthTaskRecordCount=缺")
	}
	if read.AcceptDate != nil {
		parts = append(parts, "acceptDate=有")
	} else {
		parts = append(parts, "acceptDate=缺")
	}
	return strings.Join(parts, " ")
}

// RunReport 把结论映射到 [core.RunReport]。
//
// # 为什么 `BackupStillOff` 与"领到了"同时成立时**不**报需人工
//
// [core.OutcomeManualRequired] 的读法是"目标尚未达成，程序无法代劳"
// （`internal/core/run.go` 那条注释）。这一趟的目标是"把本月攒下的记录领掉"，
// 领掉了就是达成了；"翻倍的前置没开"影响的是**下个月那一趟**，
// 把已经办成的这一趟降成黄灯，表现是历史页每天一条"要人动手"、
// 而人去了下个月还是一条。所以这一格**念进 Summary**，不改成档。
//
// ⚠ 这一条取态是 **agent 侧默认（非用户要求）**：要改成"只要备份没开就报需人工"，
// 改的是下面那一句档位判断（[TaskExpansionStatusBackupManual] 只在没领到时才落），
// 不用动结论层 —— `BackupStillOff` 已经是独立的一位。
func (res TaskExpansionResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	if res.Status == TaskExpansionStatusBackupManual {
		account := res.Account
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			taskExpansionCapabilityCode, &account,
			"在移动云盘 App 里开启自动备份：本工具传一个文件去凑，当月备份那一格仍读成未开，"+
				"而下月的翻倍奖励要靠它（这一步程序代劳不了）",
			core.TargetMobileApp,
		)}, summary)
	}
	// ⚠ 零值那一档（[TaskExpansionStatusNone]）与"发了但失败"走同一侧：
	// 它出现在"读挂了 / 等不住 / 重读挂了"那几条提前交回错误的路径上，而那些路径里
	// `Status` 恰恰就是零值。让零值落到 default 分支 ⇒ 表现是"网盘里多了一个文件、
	// 历史页那一格却是绿的"（与 [TaskExpansionStatusNone] 占零值同一条理由的另一半）。
	if res.Status == TaskExpansionStatusClaimFailed || res.Status == TaskExpansionStatusNone {
		return core.NewRunReportFailed(summary)
	}
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
