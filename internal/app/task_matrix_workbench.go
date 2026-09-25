package app

// 玩法 **1028「AI 工作台」**的**编排**：四枪一串，跨三台主机，前两枪借邮箱那一族的会话。
//
//	① POST user-njs.yun.139.com/mail/disk!…queryUserToken      （邮箱会话 sid+rmkey）→ DES 手机号 + token + userDomainId
//	② POST m.mcloud.139.com/ycloud/signin/public/getUserTaskAndCloudStatusExternal   （RSA(手机号)）→ 前置状态（**只读**）
//	③ POST ai.yun.139.com/api/outer/assistant/chat/v2/add      （Basic 整串）→ 一段 SSE
//	④ 与 ② 同一口、同一构造，排在 ③ 之后等 5 秒 ⇒ **纯观察**
//
// 而 ① 之前还有邮箱会话那两枪（`querySpecToken` + `inlogin`），由 [mailSessionFor] 供 ——
// 旧实现这一条也是 `getMail139Session(ctx)` 开头（`runners.ts:175-180`）。
//
// 请求面、DES/RSA 两处派生、SSE 读法与全部判据的出处全在
// [`internal/upstream/app_ai_workbench.go`](../upstream/app_ai_workbench.go)。
// 本文件只管**编排**：什么时候不发、发失败时念什么、错误的先后。
//
// # 证据面（2026-09-26 探针三趟，逐枪读数见 [`docs/capture/app-ai-workbench-samples.txt`](../../docs/capture/app-ai-workbench-samples.txt)）
//
//   - **①′ 通**：上游回 `code:"S_OK"`，`var.data.data` 里 `account`(24，DES) / `token`(201) /
//     `userDomainId`(19) 三格都在 ⇒ 我们那份按旧 TS 文字形状写的解析器**跟真实响应对得上**
//     （解出来是 11 位手机号形状）；
//   - **②′ 认这枚 RSA 自证身份**（不带任何鉴权头）：同一口换 `taskId=585/106/1020` 都回
//     `code:"0"` + `result`，换 `999999` 与 **1028 自己**都回 `code:"501" / msg:"任务不存在"`
//     ⇒ "任务不存在"是**按 taskId** 给的，不是路由死了、也不是身份不行；
//   - **③′ 通**：SSE 三趟各 69 / 66 / 52 行，累加出来的**确实是模型回的那一句**（三句不同）
//     ⇒ 本层的"两格都要"判据在真实读数上成立（`Events>0` 且 `Answer≠""`）；
//   - **④′** 与 ②′ 逐字同形（`501`）。
//
// ⚠⚠ 由这些读数得出一格**以前没写成文的事实**：上游把 `code`、`taskStatus`、`cloudStatus`
// 全部回成**字符串**（`"0"` / `"1"`），而旧实现那句门是 JS 严格相等 `=== 0` / `=== 1`
// ⇒ **在真实读数下这道门永远不成立**，旧实现那条 runner 今天实际走的就是"永远问一句"。
// 本层跟着它**不放宽**（放宽等于把"少发一次写"的决定权交给我们自己猜），
// 于是念法固定落在 [runWorkbench] 的 `default` 那一档（"读不出门 ⇒ 仍发对话"）。
// 取舍、后果与"哪天要改"登记在 `docs/pending-decisions.md` 本族那一格。
//
// # ⚠ ② 那一格有两种"不聊"，只有一种是真跳过
//
// 旧实现把 `needChat` 的初值设成 **true**，只有 `code===0 && result && taskStatus===1 &&
// cloudStatus===0` 成立时才置 false（`runners.ts:363-376`），而整个查询包在 `try/catch` 里
// ⇒ **查询失败也是"照样问一句"**。两档混成一个的表现：② 因为风控或网络挂了，播报念成
// "AI 工作台前置已满足，跳过对话"，而这一条今天**一个字都没往 AI 那边发** ——
// 那是本族最贵的一种误读。所以 [upstream.AIWorkbenchTaskStatus] 交回 `EnvelopeOK` 与
// `GatePass` **两格**，本层按三档分别念（下面那段）。
//
// # ③ 的成败判据在**这一层**，不在协议层
//
// 旧实现的判据是 runner 里那一句 `eventCount === 0 || !fullText` ⇒ 失败（`runners.ts:404-408`，
// 它自己的猜测是"可能内容触发审核被断流"）。协议层因此**不带** `Success` 格
// （`app_ai_workbench.go` 头注第二条），本层直接按那一句判。
//
// ⚠ 两格**都要**：只判 `Events` 会把"上游回了十条空分片"念成成功；只判 `Answer` 会把
// "非 SSE 的一段文本被当成零事件"念成"没内容"而丢掉那条更好的说明。
//
// # 这一条**不往用户网盘写东西**，但会往 AI 那边留一次对话
//
// 它不传文件、不建笔记 ⇒ [TaskMatrixRunResult.CloudFileIDs] 那条清理链与它无关
// （与 472 / 473 / 1020 同族，见 `task_matrix_mail.go` 文件头最后一条）。
// 它的"产物"有两格，都**不可回滚**：③ 在 AI 工作台那边留下一次会话记录（本仓零实现
// "删会话"那一口，也没有它的响应样本），④ 之后上游那条任务的进度。
//
// # 播报分档：为什么 ② 的失败记账、④ 的失败不记
//
// [runMailExperience] 那一条的口径是"前面那一枪挂了也要念出来"，本层跟着它，但把两枪
// 分开处理，因为**它们对这一条的影响不同**：
//   - **② 失败会影响这一条怎么走**（门不成立 ⇒ 我们多发了一发 ③，那是对用户账号的一次写），
//     所以它进 [TaskStepRecord.Err]（口径同邮箱那一族的 ③）；
//   - **④ 失败不影响任何一步**：它是链尾的纯观察（旧实现那句 `catch { /* ignore */ }`）。
//     记进 Err 会让小结出现"这一条失败"，而 ①②③ 其实都成了 —— 那才是误导。
//     所以它**只进事件流**（[TaskMatrixRun.progress]），不进账。
//
// ⚠ 与"④ 的读数说明这一条到底成没成"是两件事：成没成由**复查那一趟**（拉表第二遍）说话，
// ④ 只是旧实现留在链尾的那一眼，本层不把它当判据。

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// workbenchPostChatDelay 是 ③ 之后、④ 之前那一点等待（旧实现 `postChatDelaySec = 5`，
// `runners.ts:352`）。
//
// 为什么等：旧实现注释的那一句是"确认任务推进"—— 打完对话立刻查状态，上游那一条进度
// 可能还没落。⚠ "多久才算落"**零样本**（本族四枪全部没有实拍），这里保持出处那一个值，
// 不缩短也不加长。
const workbenchPostChatDelay = 5 * time.Second

// workbenchDialogue 是 ③ 问出去的那一句。
//
// 旧实现有两档：`ctx.config.mail139?.aiChatMessage`（配置项）|| `"你好"`（兜底），
// 见 `runners.ts:178`。本仓**没有**这一格配置（全仓零实现 `aiChatMessage`）⇒ 今天能用的
// 只有兜底那一档，于是这里落的是与出处**逐字相同**的常量。
//
// ⚠ **agent 侧默认（非用户要求）**（AGENTS.md §1.11）：不做成配置项是本仓当前状态，
// 不是用户拍过的口径。要不要给界面加这一格（每个号问不同的话）是设计决定
// ⇒ 登记成问句（`docs/pending-decisions.md` 本族那一格），不因此停手。
const workbenchDialogue = "你好"

// runWorkbench 跑玩法 1028 的本体，交回（发过枪没有，失败原因）。
//
// 顺序与三处停止条件逐条对到旧实现 `runners.ts:345-427`，各自"改了会怎样"：
//   - **① 拿不到派生凭据 ⇒ ②③④ 都不发**（旧实现 `if (!auth) return`）：
//     ② 的 RSA 输入只有那一枚手机号，③ 的 Authorization 是它拼出来的 Basic 整串，
//     两枚都没有 ⇒ 后面一枪都构造不出来。
//   - **缺 `userDomainId` ⇒ ③④ 都不发**（旧实现同一处也是 `return`，**不发链尾那一眼**）。
//     这一格是整个 runner 里最容易被"顺手补个兜底"改坏的地方：本仓**另有**一份
//     `Profile.UserDomainID()`（585 那一族用的就是它，见
//     [`task_matrix_play_ai.go`](task_matrix_play_ai.go) 的 `playAICameraUserID`），
//     而这一族的出处取的是 **① 响应里那一枚** —— 两者同不同源**未测试**，
//     所以这里**不**跟着换成库里那份（拿别人的读数填这一格会打到别人的会话上）。
//   - **③ 判失败 ⇒ ④ 不发**（旧实现 `return`）。
//   - **只有"问了"或"门说不用问"之后才走到 ④**，而门成立那条路**也要走** ④
//     （旧实现 ④ 在 `if (needChat)` **外面**）⇒ 别把它写成"问完才复查"。
func (e *taskExecutor) runWorkbench(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	line := fmt.Sprintf("%s %d", name, id)
	session, fired, problems := e.mailSessionFor(ctx)
	if len(problems) > 0 {
		e.run.progress(core.LevelError, fmt.Sprintf("AI 工作台失败 %s：%s", line, problems[0]))
		return fired, problems
	}
	fired = true
	// ① 派生 AI 域凭据。**读**：不改用户任何东西，只是把手机号解给本层看。
	//
	// 两枚查询参数（`cguid` / `rnd`）与邮箱资产侧那两口同源 ⇒ 复用 [mailDiskRandoms]
	// （出处口径：协议层不许自取随机数，见 `app_ai_workbench.go` 的构造函数头注）。
	//
	// ⚠ 这里**不**再判 `e.run.Random` / `e.run.Clock` 为 nil：走到这一句必然已经过
	// [mailSessionFor]，而它在两者任一为 nil 时直接交回 problems（一枪不发）⇒ 再判一次
	// 是死分支（AGENTS.md §1.3：不留看着像保护、其实到不了的支）。
	cguid, rnd := mailDiskRandoms(e.run.Random)
	auth, aerr := e.src.up.AppAIWorkbenchUserToken(ctx, session, cguid, rnd)
	if aerr != nil {
		problem := "AI 工作台派生凭据（①′）失败：" + aerr.Error() +
			" ⇒ ②′③′④′ 都不发（②′ 的 RSA 与 ③′ 的 Authorization 只能从这一枪来）"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 工作台失败 %s：%s", line, problem))
		return fired, append(problems, problem)
	}
	// ①′ 成功 ⇒ `Phone` 必非空（协议层那句"解出来是空串"另挡一档，见
	// `ParseAIWorkbenchUserToken`）⇒ 这里**不**再判一次 `auth.OK()`，理由同上那条死分支。

	// ② 前置状态（**只读**）。三档念法见文件头第一条。
	status, serr := e.src.up.AppAIWorkbenchTaskStatus(ctx, auth,
		upstream.AIWorkbenchTaskID, upstream.AIWorkbenchSourceChannel, e.run.Random)
	needChat := true
	switch {
	case serr != nil:
		needChat = true
		problem := "AI 工作台前置状态（②′）没查到：" + serr.Error() +
			" ⇒ 照旧实现的门仍发对话（这一档是「上游没说」，不是「上游说不用问」）"
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 工作台存疑 %s：%s", line, problem))
		problems = append(problems, problem)
	case status.GatePass:
		needChat = false
		e.run.progress(core.LevelInfo, fmt.Sprintf("AI 工作台前置已满足 %s：%s ⇒ 不问这一句（链尾仍看一眼）",
			line, workbenchStatusLine(status)))
	case status.EnvelopeOK:
		e.run.progress(core.LevelInfo, fmt.Sprintf("AI 工作台前置未满足 %s：%s ⇒ 发对话",
			line, workbenchStatusLine(status)))
	default:
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 工作台前置状态读不出门 %s：%s ⇒ 照旧实现仍发对话",
			line, workbenchStatusLine(status)))
	}
	if needChat {
		asked, chatProblems := e.runWorkbenchChat(ctx, line, auth)
		fired = fired || asked
		problems = append(problems, chatProblems...)
		if len(chatProblems) > 0 {
			// ③ 那一档没过去（缺 userDomainId / 发失败 / 上游没回内容）⇒ 链尾那一眼**不发**，
			// 与旧实现那三处 `return` 同一形状（见函数头第二条）。
			return fired, problems
		}
	}
	// ④ 链尾那一眼：等不住（ctx 取消）时也**不发**，且不改这一条的结论（它不影响任何一步）。
	if werr := e.wait(ctx, workbenchPostChatDelay); werr != nil {
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 工作台链尾复查没发 %s：等不住（%s）"+
			" ⇒ 那一发是纯观察，不发不改变这一条的任何结论", line, werr.Error()))
		return fired, problems
	}
	after, aerr := e.src.up.AppAIWorkbenchTaskStatus(ctx, auth,
		upstream.AIWorkbenchTaskID, upstream.AIWorkbenchSourceChannel, e.run.Random)
	if aerr != nil {
		// ⚠ 只念不记账：理由成文在文件头最后一条。
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 工作台链尾复查（④′）失败 %s：%s"+
			" ⇒ 不进这一条的结论（它是纯观察）", line, aerr.Error()))
		return fired, problems
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("AI 工作台链尾复查 %s：%s", line, workbenchStatusLine(after)))
	return fired, problems
}

// runWorkbenchChat 跑 ③ 那一发，交回（发过枪没有，失败原因）。
//
// 单独成函数为一件事：**"③ 失败 ⇒ ④ 不发"**是这一族与邮箱那一族（③ 失败仍发 ④）
// **相反**的一格，它必须集中在一处、可被一条用例逐字钉住。
func (e *taskExecutor) runWorkbenchChat(
	ctx context.Context, line string, auth upstream.AIWorkbenchDomainAuth,
) (bool, []string) {
	if auth.UserDomainID == "" {
		// ①′ 说成功却没给这一格（协议层不挡它：旧实现把缺失归到 `|| ""`，
		// `runners.ts:445` 那句 `inner.userDomainId || ""`）。
		problem := "AI 工作台没问：①′ 没回 userDomainId ⇒ ③′④′ 都不发" +
			"（这一格出处**只有** ① 那一个，不拿库里那份顶 —— 两者同不同源未测试）"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 工作台跳过 %s：%s", line, problem))
		return false, []string{problem}
	}
	// `inputTime` 由**注入时钟**造（本层不读 time.Now()，否则夹具不可复现）。
	// 格式与 585 那一族**不同形**（那一套是 UTC + 字面量 `Z`），取舍成文在
	// `upstream.AIWorkbenchInputTime` 头注。
	read, cerr := e.src.up.AppAIWorkbenchChat(ctx, auth, upstream.AIWorkbenchChatParams{
		UserID:    auth.UserDomainID,
		Dialogue:  workbenchDialogue,
		InputTime: upstream.AIWorkbenchInputTime(e.run.Clock.Now()),
	})
	if cerr != nil {
		problem := "AI 工作台对话（③′）失败：" + cerr.Error() + " ⇒ 链尾复查不发"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 工作台失败 %s：%s", line, problem))
		return true, []string{problem}
	}
	// 判据逐字按旧实现那一句（文件头第二条）：两格**都要**。
	if read.Events == 0 || read.Answer == "" {
		problem := "AI 工作台对话（③′）上游没回任何内容：" + workbenchReadLine(read) +
			"（旧实现同一处的猜测是「内容触发审核被断流」—— ⚠ 那只是它的猜测，未验证）⇒ 链尾复查不发"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 工作台失败 %s：%s", line, problem))
		return true, []string{problem}
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("AI 工作台对话已回 %s：%s", line, workbenchReadLine(read)))
	return true, nil
}

// workbenchStatusLine 把 ②④ 的读数摊成一行。
//
// 五格全念（`code` / 有没有 `result` / 两枚状态 / 门）：**只念"门过没过"不够**——
// 这一行的用途是"下一次真跑该怀疑哪里"，而 `taskStatus=2` 与"上游压根没回 result"
// 是完全不同的两件事（前者是状态，后者是没读到）。
func workbenchStatusLine(status upstream.AIWorkbenchTaskStatus) string {
	result := "无"
	if status.HasResult {
		result = "有"
	}
	gate := "不过"
	if status.GatePass {
		gate = "过"
	}
	text := "AI 工作台前置：code=" + orNone(status.CodeRaw) + "、result=" + result
	if status.TaskStatus != "" {
		text += "、taskStatus=" + status.TaskStatus
	}
	if status.CloudStatus != "" {
		text += "、cloudStatus=" + status.CloudStatus
	}
	return text + "、门=" + gate
}

// workbenchReadLine 把 ③ 的读数摊成一行：三格各有"少了它会怎样"。
//   - `data 行数`：判据的第一格（0 事件与"有事件但空内容"是两种形状）。
//   - `code` / `message`：只在某些行里有值，原样带出不归一（同 585 那一族的口径）。
//   - `模型回话`：**只有这一格**是内容 ⇒ 过截断（响应侧文本，不含凭据，见协议层头注）。
//   - `sessionId`：**只报长度**。真跑那三趟它回的是 19 位数字（一次会话的标识），而这一行
//     会进事件流 ⇒ 也会进 `runs` 日志与推送正文（本仓的推送把播报行原样发出去）。
//     它与"真实 fileId 不进成文"、"预签名 URL 不进日志"同一档：**标识不落文本**。
func workbenchReadLine(read upstream.AIWorkbenchChatRead) string {
	text := "AI 工作台响应：data 行数=" + fmt.Sprint(read.Events)
	if read.LastCode != "" {
		text += "、code=" + read.LastCode
	}
	if read.LastMessage != "" {
		text += "、message=" + playAICameraLogText(read.LastMessage)
	}
	if read.SessionID != "" {
		text += "、sessionId 长度=" + fmt.Sprint(len(read.SessionID))
	}
	return text + "、模型回话=" + playAICameraLogText(read.Answer)
}

// orNone 给"上游没给这一格"一个可读的读数，而不是空字符串（空串进播报看起来像漏拼）。
//
// 它在本文件用，是因为 ② 的 `code` 在失败档真的可能是空（体压根不是 JSON 时
// 协议层交回零值读数而不是错误）。
func orNone(text string) string {
	if text == "" {
		return "无"
	}
	return text
}
