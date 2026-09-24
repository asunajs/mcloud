package app

// 「AI 头像」跑一个账号那一段：生成 N 张 → 抽完当天次数 → 找回写进网盘的临时文件。
//
// 对应旧实现 `tasks/ai-avatar/task.ts`（`docs/business-feature-inventory.md` §2.3 `ai-avatar`）。
// 协议层在 `internal/upstream/app_ai_avatar.go`（八口，其中 `hcy/file/list` 有实拍、
// rabactive 那六口**零实拍**）；本文件只管"按什么顺序发、发不出去时算哪一档"。
//
// # 这条链是三族里唯一会**往用户网盘写东西**的
//
// `image/upload` 一发出去，用户网盘里就多两个文件（噪点原图 + 合成图）。
// 用户 2026-09 的授权口径是"其他都实测，包括上传网盘文件"，所以这一条**照写**；
// 但"写完谁来清"今天没有答案 —— 清理要用的 `recyclebin/batchTrash` 零实拍（P1），
// 所以本链只负责把找回的 `fileId` 报出来（`SummaryLine` 里有那一句），**不删任何东西**。
//
// # `jwttoken` 使用公共来源（临时态，不是终判）
//
// 这一页的票按 [SignInWorld.Open] 走签到页那一条建会话配方（`RecipeSignInPage`）。
// 归档里**没有**换出该页 jwttoken 的那一步 ⇒ 按退路走（口径见 `AGENTS.md` §1.12）。
// 抓到特有取法时**主动更正**这里取票那一句并改掉这段说明，不留"注释说公共、代码走特有"。
//
// # 节奏照旧实现：三处 sleep 都是**判据的一部分**
//
// 轮询"先睡再查"（第一次查之前先等 3 秒）、轮间 2 秒、抽奖间隔 1 秒。
// 旧实现那三个数是它跑通过的那一份，本仓没有第二个证据；
// 而"上游 3 秒内到底合没合成好"只能真跑才看得见（未测试）。
// "等"走 [Waiter] 注入点而不是 `time.Sleep`，理由与 [LoginOutcome] 那条一样：
// 否则这些时长的语义要么测不了，要么让测试真睡 24 秒。

import (
	"context"
	"encoding/base64"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// AiAvatarDailyGenerateCount 是**本工具**每天生成几张（旧实现 `DEFAULT_GENERATE_COUNT`）。
//
// ⚠ 旧实现读的是用户配置 `AI新头像.每日生成次数`，缺省才用 5；本仓今天
// **没有**"每能力带参数"的参数面（除 `sign` 与 `live-room` 外全部只有账号两栏），
// 所以这里钉成常量。要不要把它开成一个参数 = 待拍板（`docs/pending-decisions.md` Q19），
// 因为开一栏等于改这一条能力的参数面。
const AiAvatarDailyGenerateCount = 5

const (
	// aiAvatarAttemptsPerRound 是**一轮**最多试几次合成（旧 `attempt <= 2`）。
	// 一次"失败"包括合成没回任务 id、上游判失败、下载失败 —— 都重打一次。
	aiAvatarAttemptsPerRound = 2
	// aiAvatarMaxPolls 是一次合成的状态查询上限（旧 `MAX_POLL_COUNT`）。
	aiAvatarMaxPolls = 8
	// aiAvatarPollInterval 是**每次查询之前**的等待（旧循环体先 `sleep` 再 `getStatus`）。
	aiAvatarPollInterval = 3 * time.Second
	// aiAvatarRoundInterval 是两张图之间的等待（旧实现在**最后一轮之后不等**）。
	aiAvatarRoundInterval = 2 * time.Second
	// aiAvatarDrawInterval 是两次抽奖之间的等待，同样"最后一次不等"。
	aiAvatarDrawInterval = time.Second
	// aiAvatarDrawFallback 是"读不到剩余次数"时最多试抽几枪（旧 `MAX_DRAW_FALLBACK`）。
	aiAvatarDrawFallback int64 = 5
	// aiAvatarWorkListType 固定 1：旧实现的默认值。这套 type 的取值表没有成文，
	// 本层不枚举（理由见 [upstream.AiAvatarWorkListRequest]）。
	aiAvatarWorkListType int64 = 1
	// aiAvatarFileListPageSize 沿用旧实现的 100（一页取完，不翻页 —— 多了静默漏，见 R16）。
	aiAvatarFileListPageSize int64 = 100
)

// ─────────────────────────────────────────────────────────────────────────────
// 纯判据：抽奖该打几枪
// ─────────────────────────────────────────────────────────────────────────────

// PlanAiAvatarDrawRounds 把 `getHashLeft` 的读数翻成"最多抽几枪"。
//
// `left == nil` 涵盖**两种**上游形态：`result` 是 `null`，以及整个字段没给
// （协议层都交回 `nil`，见 [upstream.ParseAiAvatarHashLeft]）。
// 负数按 0 办（旧实现 `Math.max(0, result)`）。
//
// # 为什么"读不到"这一档仍然要抽（与 [PlanFunai] 相反）
//
// 趣玩AI那一天只有一次额度，判错 ⇒ 花掉一次回不来；这一族的抽奖是**消耗上游给的次数**，
// 而"上游给了几次"读不到时旧实现的做法是照抽、靠 `408` 那一句停（`task.ts:128`）。
// 两种错法的代价不对称的方向**正好相反**：多抽的代价是"上游回一次拒绝"，
// 少抽的代价是"今天的额度作废"。所以这里跟随旧实现取"照抽"。
//
// ⚠ "无额度时上游到底回 `408` 还是别的"本仓**零样本**（`AiAvatarDrawResult.Stopped`
// 那一格登记的正是它）⇒ 上面那句代价比较是**按旧代码的读法**，不是实测。
// 要核它：拿一个当天已抽完的账号再抽一次，把响应逐字记进 `docs/capture/`。
func PlanAiAvatarDrawRounds(left *int64) int64 {
	if left == nil {
		return aiAvatarDrawFallback
	}
	if *left < 0 {
		return 0
	}
	return *left
}

// AiAvatarDrawPhase 是抽奖那一族的**停在哪**（四档互斥，进 `SummaryLine`）。
type AiAvatarDrawPhase int

const (
	// AiAvatarDrawRan 是按判据抽完了（一枪都没被叫停）。
	AiAvatarDrawRan AiAvatarDrawPhase = iota
	// AiAvatarDrawStopped 是上游叫停（`code == 408`）—— 剩下的判据次数**没有**发出去。
	AiAvatarDrawStopped
	// AiAvatarDrawSkipped 是判据说明一次都别抽（剩余 0）⇒ **一个写请求都没发**。
	AiAvatarDrawSkipped
	// AiAvatarDrawBlocked 是抽到某一枪发不出去（网络 / 会话刷不动 / 响应形状变了）
	// ⇒ 后面那些额度今天拿不回来了。
	AiAvatarDrawBlocked
)

// Reason 是这一档说给用户的那半句。
//
// ⚠ [AiAvatarDrawStopped] 那一句**不**解释 408 是什么（"额度用完了吧"）：
// 那一档的语义只来自旧实现的 `break`，没有任何一条实拍说它对应什么。
func (p AiAvatarDrawPhase) Reason() string {
	switch p {
	case AiAvatarDrawStopped:
		return "上游回了停止码（旧实现认 408），剩下的没抽"
	case AiAvatarDrawSkipped:
		return "上游报的剩余次数是 0，本次一个抽奖请求都没发"
	case AiAvatarDrawBlocked:
		return "有一枪没发出去，剩下的次数今天拿不回来"
	}
	return "按判据抽完了"
}

// ─────────────────────────────────────────────────────────────────────────────
// 结局
// ─────────────────────────────────────────────────────────────────────────────

// AiAvatarStatus 是一个账号这一趟的档位。
type AiAvatarStatus int

const (
	// AiAvatarDone 是**两条链都走完了，且有产出**（生成 ≥1 张 或 抽中 ≥1 次）。
	AiAvatarDone AiAvatarStatus = iota
	// AiAvatarNothing 是**走完了但零产出**：一张没生成、一次没抽中（含"没次数"）。
	//
	// 它算**成功**（与 [FunaiNothing] 同一口径）：这一族每一天都可能零产出，
	// 折成失败会让历史列表天天红，而"红"在这里不指向任何一件人要做的事。
	AiAvatarNothing
	// AiAvatarFailed 是**中途被拦住**：等不下去了（ctx 取消）或抽奖那一枪发不出去。
	//
	// ⚠ 与 [AiAvatarDrawBlocked] 是同一件事的两个视角（档位 vs 那一族的细节），
	// 刻意不合并：`Generated`/`Draws` 那两格数字在两种情况下都有意义。
	AiAvatarFailed
)

// AiAvatarResult 是实况。
//
// ⚠ 取不到票 / 取不到凭据 / 换不到 `deviceId` 三类**不**在这一格里 ——
// 它们作为 `*core.Error` 交回编排层（多发一条 `errored` 事件，比折成失败说得更清楚）。
type AiAvatarResult struct {
	Account core.AccountID
	Status  AiAvatarStatus
	// Generated 是成功写进网盘的张数（一轮最多算一次，重试成功也只算一次）。
	Generated int
	// Rounds 是计划轮数（[AiAvatarDailyGenerateCount]）。分开两格是为了"3/5"这种读法
	// 能被界面直接摆出来，而不是等人去数事件。
	Rounds int
	// Draws 是**抽中**的枪数（被叫停与被拒都不计）。
	Draws int
	// DrawPhase 是抽奖那一族停在哪。
	DrawPhase AiAvatarDrawPhase
	// Prizes 是上游报的奖品名（`contentName` 优先，两格都没有时**不**收空串）。
	Prizes []string
	// TempFileIDs 是按文件名找回的网盘文件 id —— **交给将来的清理**，本链不删。
	TempFileIDs []string
	// Note 是"这一趟额外该知道的一件事"（找回临时文件失败那一类）。
	Note string
	// Error 是 [AiAvatarFailed] 的原因。
	Error string
}

// AiAvatarRun 跑一次 AI 头像（生成 + 抽奖 + 找回）。
type AiAvatarRun struct {
	SignInWorld

	// Wait 是"等一会"的注入点。nil ⇒ [RealWaiter]（**默认必须是真的等**：
	// 把 nil 当"不等"会让生产环境对着上游连打八枪查询，而那是本文件头那条节奏的全部意义）。
	Wait Waiter
}

// RunAccount 跑**一个账号**的这一趟。
//
// 三件事的顺序照旧实现（生成 → 抽奖 → 找回），**不能换**：
// 找回要用生成那一族产出的那批上传 id，抽奖放在中间则与旧实现的日志时序一致。
//
// `deviceId` 取不到 ⇒ 整趟失败并回错误：旧实现在这一步直接跳过这个账号
// （`init-account.ts:31-35`），因为那一族的每一枪都带 `deviceid` 头。
// ⚠ "少了这个头上游会不会理"本仓零样本 ⇒ 不赌"少一个头也能跑"，按旧实现的处置办。
func (r AiAvatarRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (AiAvatarResult, *core.Error) {
	opening, perr := r.Open(ctx, account)
	if perr != nil {
		return AiAvatarResult{Account: account}, perr
	}
	// `authorization` 只从这一份凭据来（schema v7 起库里存的就是整串）。
	// 不另加一个导出方法：包内直接读字段，等于"零对外新增面"（见 `signin.go` 那条凭据边界）。
	authorization := opening.Credential.Authorization()
	deviceID, perr := r.DeviceIDFor(ctx, account)
	if perr != nil {
		failed := AiAvatarResult{Account: account, Status: AiAvatarFailed, Error: perr.Error()}
		return failed, perr
	}
	wait := r.Wait
	if wait == nil {
		wait = RealWaiter{}
	}

	result := AiAvatarResult{
		Account: account, Rounds: AiAvatarDailyGenerateCount,
		Prizes: []string{}, TempFileIDs: []string{},
	}

	uploadIDs, perr := r.generateRounds(ctx, opening.Source, authorization, deviceID, wait)
	if perr != nil {
		// 只有"等被取消"会走到这里：那时后面两族都不该发，直接交回错误。
		return AiAvatarResult{Account: account, Status: AiAvatarFailed, Rounds: result.Rounds,
			Generated: len(uploadIDs), Error: perr.Error()}, perr
	}
	result.Generated = len(uploadIDs)

	draws, phase, prizes, perr := r.drawAll(ctx, opening.Source, authorization, deviceID, wait)
	if perr != nil {
		return AiAvatarResult{
			Account: account, Status: AiAvatarFailed, Rounds: result.Rounds,
			Generated: result.Generated, Draws: draws, DrawPhase: phase,
			Prizes: prizes, Error: perr.Error(),
		}, perr
	}
	result.Draws = draws
	result.DrawPhase = phase
	result.Prizes = prizes
	if phase != AiAvatarDrawRan {
		r.progress(core.LevelInfo, "AI头像抽奖："+phase.Reason())
	}

	// 找回只在**真的写过**之后才做：没有上传 id 时这三枪必然什么都找不到，
	// 白打三枪（其中 `file/list` 还是另一套头）不值。
	if len(uploadIDs) > 0 {
		ids, note := r.collectTempFiles(ctx, opening, authorization, deviceID, uploadIDs)
		result.TempFileIDs = ids
		result.Note = note
		if note != "" {
			r.progress(core.LevelWarn, "AI头像临时文件没找全："+note)
		}
	}

	result.Status = AiAvatarNothing
	if result.Generated > 0 || result.Draws > 0 {
		result.Status = AiAvatarDone
	}
	r.progress(core.LevelInfo, "AI头像："+result.SummaryLine())
	return result, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 生成那一族
// ─────────────────────────────────────────────────────────────────────────────

// generateRounds 跑 `Rounds` 轮，每轮最多试 [aiAvatarAttemptsPerRound] 次。
//
// 返回**成功**的那些上传 id（顺序 = 轮次顺序）。失败轮次只发一条过程话，
// 不影响后面的轮次（旧实现就是这个脾气：一轮废了继续下一轮）。
//
// 唯一回错误的情况是"等不下去了"（ctx 取消）—— 那时继续发请求是没授权的。
func (r AiAvatarRun) generateRounds(
	ctx context.Context, src *SignInSessionSource, authorization, deviceID string, wait Waiter,
) ([]string, *core.Error) {
	uploadIDs := []string{}
	for round := 0; round < AiAvatarDailyGenerateCount; round++ {
		var uploadID string
		var reason string
		for attempt := 1; attempt <= aiAvatarAttemptsPerRound && uploadID == ""; attempt++ {
			id, note, perr := r.generateOnce(ctx, src, authorization, deviceID, wait)
			if perr != nil {
				return uploadIDs, perr
			}
			uploadID, reason = id, note
		}
		if uploadID != "" {
			uploadIDs = append(uploadIDs, uploadID)
			r.progress(core.LevelInfo, "AI头像第"+strconv.Itoa(round+1)+"/"+
				strconv.Itoa(AiAvatarDailyGenerateCount)+"次生成成功")
		} else {
			r.progress(core.LevelWarn, "AI头像第"+strconv.Itoa(round+1)+"次生成失败："+reason)
		}
		if round < AiAvatarDailyGenerateCount-1 {
			if perr := wait.Wait(ctx, aiAvatarRoundInterval); perr != nil {
				return uploadIDs, perr
			}
		}
	}
	return uploadIDs, nil
}

// generateOnce 造一张噪点图 → 提交合成 → 轮询到成功 → 下载 → 写进网盘。
//
// 返回的 `reason` 只在 `uploadID == ""` 时有意义（那一轮的失败原因）。
// 除"等被取消"外**一律不回错误**：合成失败、上游判死、轮询超时、下载失败、
// 写盘失败在旧实现里都是"这一次没成"，交给调用方重试下一枪。
//
// # 为什么"先睡再查"
//
// 旧循环体第一行是 `await sleep(3000)`，然后才 `getStatus`（`task.ts:57-59`）——
// 也就是**提交之后不会立刻查一次**。看起来像少查一次，其实是给上游留那 3 秒；
// 换成"先查后睡"的表现是第一枪大概率还在排队、白占一次额度未知的查询。
// 这里保持旧序。⚠ "上游 3 秒内合成得完吗"未测试。
func (r AiAvatarRun) generateOnce(
	ctx context.Context, src *SignInSessionSource, authorization, deviceID string, wait Waiter,
) (uploadID string, reason string, perr *core.Error) {
	noiseBase64 := aiAvatarNoiseBase64(r.Random)

	merged, merr := do(ctx, src, func(s SignInSession) (upstream.AiAvatarTask, *core.Error) {
		return src.up.AppAiAvatarMerge(ctx, authorization, s.jwt, deviceID, noiseBase64)
	})
	if merr != nil {
		return "", "合成请求失败：" + merr.Error(), nil
	}
	if merged.TaskID == nil || *merged.TaskID == "" {
		return "", "上游回了成功但没给任务 id", nil
	}
	taskID := *merged.TaskID

	for poll := 0; poll < aiAvatarMaxPolls; poll++ {
		if werr := wait.Wait(ctx, aiAvatarPollInterval); werr != nil {
			return "", "", werr
		}
		status, serr := do(ctx, src, func(s SignInSession) (upstream.AiAvatarTask, *core.Error) {
			return src.up.AppAiAvatarStatus(ctx, authorization, s.jwt, deviceID, taskID)
		})
		if serr != nil {
			return "", "查合成状态失败：" + serr.Error(), nil
		}
		// 三档：`4` 判死、`3` 成功、其余（含"没给这一格"）继续等。
		// ⚠ "没给"落在继续等这一侧是**旧实现的读法**（`status !== 3`），
		// 而它今天的真实含义**未验证**（[upstream.AiAvatarTask] 那段登记的三档只有
		// 3/4 两个数是旧代码里出现过的）。
		if status.Status != nil && *status.Status == upstream.AiAvatarStatusFailed {
			return "", "上游判这一次合成失败", nil
		}
		if status.Status == nil || *status.Status != upstream.AiAvatarStatusSuccess {
			continue
		}
		if status.ImageURL == nil || *status.ImageURL == "" {
			return "", "合成成功但没回图片地址", nil
		}
		mergedBase64, derr := aiAvatarDownloadImage(ctx, src.up, *status.ImageURL)
		if derr != nil {
			return "", "下载合成图失败：" + derr.Error(), nil
		}
		id, uerr := do(ctx, src, func(s SignInSession) (string, *core.Error) {
			return src.up.AppAiAvatarUpload(ctx, authorization, s.jwt, deviceID,
				noiseBase64, mergedBase64)
		})
		if uerr != nil {
			return "", "写入网盘失败：" + uerr.Error(), nil
		}
		return id, "", nil
	}
	return "", "查了 " + strconv.Itoa(aiAvatarMaxPolls) + " 次还没合成好", nil
}

// aiAvatarDownloadImage 取上游回的那个图片地址，编成**裸 base64**。
//
// 走 [upstream.Upstream.Raw]（非 2xx 即错误），请求**不带任何头** ——
// 旧实现那里就是一句裸 `fetch(url)`。
//
// ⚠ 两件事没验过，都不擅自"改进"：
//   - 那个地址**要不要带凭据**才能取到（未验证；带错了的代价可能是把 token 发给另一个域）；
//   - 上游会不会给一个指向内网的地址（这一族今天没有 URL 白名单，P20 登记）。
//     旧实现同样不设防，本仓今天唯一的缓解是"这个 URL 由上游响应给"。
func aiAvatarDownloadImage(
	ctx context.Context, up *upstream.Upstream, imageURL string,
) (string, *core.Error) {
	response, perr := up.Raw(ctx, core.NewRequestGet(imageURL))
	if perr != nil {
		return "", perr
	}
	return base64.StdEncoding.EncodeToString(response.Body), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 抽奖那一族
// ─────────────────────────────────────────────────────────────────────────────

// drawAll 读一次剩余次数 → 按判据抽，直到抽完 / 被叫停 / 某一枪没发出去。
//
// 与旧实现的**一处刻意不同**：某一枪失败时这里**停手**，旧实现是 `continue` 下一枪。
// 理由是本仓的协议层把"上游回了非 0 码"与"请求压根没发出去"合成了同一个
// [core.Error]（见 [upstream.ParseAiAvatarDraw] 与 [upstream.EnsureOKWithHint]），
// 分不开就按更保守那一档办：宁多少抽一次，不要对着一个坏会话连打五枪。
// 要恢复旧行为，先把那一族的失败分型（HTTP vs 业务码）做进协议层 —— 登记成待办，
// 不在这里靠猜错。
//
// `getHashLeft` 那一枪失败**不**停：读不到判据时按 [aiAvatarDrawFallback] 兜底
// （判据与理由见 [PlanAiAvatarDrawRounds]）。
func (r AiAvatarRun) drawAll(
	ctx context.Context, src *SignInSessionSource, authorization, deviceID string, wait Waiter,
) (drawn int, phase AiAvatarDrawPhase, prizes []string, perr *core.Error) {
	left, herr := do(ctx, src, func(s SignInSession) (*int64, *core.Error) {
		return src.up.AppAiAvatarHashLeft(ctx, authorization, s.jwt, deviceID)
	})
	if herr != nil {
		r.progress(core.LevelWarn, "AI头像读剩余次数失败，按旧实现的兜底次数抽："+herr.Error())
		left = nil
	}
	rounds := PlanAiAvatarDrawRounds(left)
	prizes = []string{}
	if rounds == 0 {
		return 0, AiAvatarDrawSkipped, prizes, nil
	}
	for index := int64(0); index < rounds; index++ {
		outcome, derr := do(ctx, src, func(s SignInSession) (upstream.AiAvatarDrawResult, *core.Error) {
			return src.up.AppAiAvatarDraw(ctx, authorization, s.jwt, deviceID)
		})
		if derr != nil {
			return drawn, AiAvatarDrawBlocked, prizes, derr.WithPrefix(
				"AI头像第" + strconv.Itoa(int(index+1)) + "次抽奖没发出去: ")
		}
		if outcome.Stopped {
			return drawn, AiAvatarDrawStopped, prizes, nil
		}
		drawn++
		if name := aiAvatarPrizeName(outcome.Prize); name != "" {
			prizes = append(prizes, name)
		}
		if index < rounds-1 {
			if werr := wait.Wait(ctx, aiAvatarDrawInterval); werr != nil {
				return drawn, AiAvatarDrawRan, prizes, werr
			}
		}
	}
	return drawn, AiAvatarDrawRan, prizes, nil
}

// aiAvatarPrizeName 按旧实现的优先序取奖品名：`contentName` 优先，退到 `prizeName`。
//
// 两格都是 `*string`（没有任何一档样本能告我们哪一格必填），都拿不到时返回空串 ——
// 调用方**不收**空串进清单：那会在界面上摆出一个空条目，看起来像"中了一个叫空的奖"。
func aiAvatarPrizeName(prize upstream.AiAvatarPrize) string {
	if prize.ContentName != nil && *prize.ContentName != "" {
		return *prize.ContentName
	}
	if prize.PrizeName != nil {
		return *prize.PrizeName
	}
	return ""
}

// ─────────────────────────────────────────────────────────────────────────────
// 找回写进网盘的那两个文件（清理的前置，本链不删）
// ─────────────────────────────────────────────────────────────────────────────

// collectTempFiles 用三枪把上传产物在网盘里的 `fileId` 找回来。
//
// **回不了错误**：这一族全失败也只是"那几个文件留在网盘上没人管"，
// 而生成与抽奖都已经办完了 —— 把整趟判红等于告诉用户"这一趟没成"，
// 而他们据此重跑会**再写五张图**进网盘。旧实现同样只 debug 一句（`task.ts:160`）。
// 拿回来的只有 id 与一句人话。
//
// 两枪读的是 rabactive（签到页那张票 + `deviceid`），第三枪读的是网盘侧
// （`x-yun-*` 那一套头、**不带 `jwttoken`**）—— 所以档案必须用 [SignInOpening.Profile]
// 那一份回填过 `x-yun-uni` 的，理由见它的注释。
func (r AiAvatarRun) collectTempFiles(
	ctx context.Context, opening SignInOpening, authorization, deviceID string,
	uploadIDs []string,
) ([]string, string) {
	parentFileID, perr := do(ctx, opening.Source, func(s SignInSession) (*string, *core.Error) {
		return opening.Source.up.AppAiAvatarCatalogParentFileID(ctx, authorization, s.jwt,
			deviceID, upstream.AiAvatarCatalogMarketName)
	})
	if perr != nil {
		return []string{}, "读活动目录失败：" + perr.Error()
	}
	if parentFileID == nil || *parentFileID == "" {
		return []string{}, "上游没给活动目录的路径，没去找那些文件"
	}
	works, perr := do(ctx, opening.Source, func(s SignInSession) ([]upstream.AiAvatarWork, *core.Error) {
		return opening.Source.up.AppAiAvatarWorkList(ctx, authorization, s.jwt, deviceID,
			aiAvatarWorkListType)
	})
	if perr != nil {
		return []string{}, "读作品列表失败：" + perr.Error()
	}
	fileList, perr := opening.Source.up.AppCloudFileList(ctx, opening.Profile,
		UserTimestamp(r.Clock), authorization, *parentFileID, aiAvatarFileListPageSize)
	if perr != nil {
		return []string{}, "列目录失败：" + perr.Error()
	}
	ids := collectAiAvatarFileIDs(uploadIDs, works, fileList.Items)
	if len(ids) == 0 {
		// 一枪没撞中：要么网盘里没有（上游没落盘），要么名字对不上（同名假设破了），
		// 要么多于 100 个文件被分页截掉。三档在这一格**分不开**（没有样本能分），
		// 所以这句话只说"没找到"，不说"没有"。
		return []string{}, "按文件名一个都没撞上（可能没落盘、可能名字变了、也可能被一页 100 个截掉）"
	}
	return ids, ""
}

// ─────────────────────────────────────────────────────────────────────────────
// 给人看的两句话
// ─────────────────────────────────────────────────────────────────────────────

// SummaryLine 是这一趟的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 里面只有张数、次数、奖品名与**待清理文件的个数**：没有任何凭据形态的东西。
// [AiAvatarResult.TempFileIDs] 那一串 id **不**进这句话 —— 界面上要的是"留了几个"，
// 不是三串没人看得懂的对象 id（它们本身也不是 secret，只是没用）。
func (res AiAvatarResult) SummaryLine() string {
	var builder strings.Builder
	builder.WriteString("AI头像生成 ")
	builder.WriteString(strconv.Itoa(res.Generated))
	builder.WriteString("/")
	builder.WriteString(strconv.Itoa(res.Rounds))
	// "抽奖"那半句的空格**不在**前缀里：Skipped 那一档念起来是"抽奖没抽（剩余 0）"，
	// 拼成"抽奖 没抽"是要露馅的（`TestAiAvatarSummaryLineTellsEveryPhaseApart` 钉住）。
	builder.WriteString(" 张，抽奖")
	switch res.DrawPhase {
	case AiAvatarDrawSkipped:
		builder.WriteString("没抽（剩余 0）")
	case AiAvatarDrawStopped:
		builder.WriteString(" " + strconv.Itoa(res.Draws) + " 次后被上游叫停")
	case AiAvatarDrawBlocked:
		builder.WriteString(" " + strconv.Itoa(res.Draws) + " 次后有一枪没发出去")
	case AiAvatarDrawRan:
		builder.WriteString(" " + strconv.Itoa(res.Draws) + " 次")
	}
	if len(res.Prizes) > 0 {
		builder.WriteString("，抽到「" + strings.Join(res.Prizes, "、") + "」")
	}
	if len(res.TempFileIDs) > 0 {
		builder.WriteString("，网盘里留了 " + strconv.Itoa(len(res.TempFileIDs)) + " 个待清理文件")
	}
	if res.Status == AiAvatarFailed && res.Error != "" {
		builder.WriteString("；失败：" + res.Error)
	} else if res.Note != "" {
		builder.WriteString("；" + res.Note)
	}
	return builder.String()
}

// RunReport 把三档结论映射到 [core.RunReport]。
//
// 只有 [AiAvatarFailed] 落失败 —— 而今天这一档**只由 ctx 取消**触发
// （抽奖那一枪发不出去时 `RunAccount` 已经提前返回错误了，见 `drawAll` 那条注释），
// 所以这条映射实际上是给"编排层拿不到错误、只拿得到结果"的那条路兜底。
//
// 没有 [core.OutcomeManualRequired] 那一档：这一族不需要人去做程序代劳不了的事
// （噪点图不需要人挑，抽奖不需要人点）。**但**"网盘里留了文件没人清"是一件
// 今天程序办不了的事 —— 它挂不挂成人工步骤，等清理那一族（P1）定性再判，
// 现在挂出去只会让人去手动删一堆可能不存在的文件。
func (res AiAvatarResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	if res.Status == AiAvatarFailed {
		return core.NewRunReportFailed(summary)
	}
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
