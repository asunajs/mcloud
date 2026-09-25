package app

// 585「AI 相机」（旧实现 `packages/core/src/tasks/ai-tasks.ts` 的 `completeAiCameraTask`）
// 的**编排**：两枪一串，同一台主机，**同一枚凭据** —— 而它**不是 `jwttoken`**。
//
//	① POST ai.yun.139.com/api/image/aiRecognize                Authorization = 库里那串 `Basic …` **整串**
//	                                                            → `data.fileId` + `data.taskId`
//	② POST ai.yun.139.com/api/outer/assistant/chat/v2/add      Authorization = **同一枚整串**
//	                                                            → 一段 SSE 文本
//
// 请求面、三档回退读法与成功判据全在
// [`internal/upstream/app_play_ai_chat_sse.go`](../upstream/app_play_ai_chat_sse.go)
// （那里成文了三套 `chat/v2/add` 的分别）。
// 本文件只管**编排**：什么时候不发、发失败时念什么、产物记在哪本账上。
//
// # ⚠⚠ 两枪的凭据是**同一枚**：库里那串 `Basic …` 整串（两次真跑 + 一条实拍换出来的）
//
// 这一格连着被否过两次，顺序别记反（它决定"下一次真跑该怀疑哪里"）：
//   - **第一次**：把旧实现的 `ctx.config.token` 读成"H5 那张票" ⇒ ① 带票出网
//     ⇒ `code:"01000002"` / `鉴权失败`（2026-09-26 真跑。同趟里 data URL 与裸 base64
//     两档样图同样挂 ⇒ "样图比不比前缀"那一问**没收到答案**）。
//   - **第二次**：回去逐字读出处 —— `config` 是 `utils/init-account.ts:52` 用
//     `parseAuth(auth)` 摊平的，`auth/utils.ts` 的 `doParseAuth` = 去 `Basic ` 头 →
//     base64 解码 → 按 `:` 拆三段 ⇒ `ctx.config.token` 是那串 Basic 的**第三段裸 authToken**
//     （本仓云笔记 [runNote] 取的是同一枚）。换成裸 authToken 再真跑 ⇒ **同样**
//     `code:"01000002"` / `鉴权失败` ⇒ 这一版也被否。
//   - **然后不再猜**：去读 `har/2026-09-25-playai-*.har` 里真机那两条 —— ① 与 ② 的
//     `Authorization` **逐字同一枚**（同长 302、同指纹 `2c72db5d0783`），且真机 ① 回
//     `code=0000` 并给了 `fileId` / `taskId`。
//
// ⇒ **参考实现这一处与真实客户端不一致**（它把 ① 取成了 Basic 的第三段），本仓按**实拍**落。
// ✅ 这一版（整串）**已经真跑通过**（2026-09-26，`docs/status.md` 那一趟）：
// ①② 都成功（`发过枪=true 失败 0 条`，② 的读数 `档=data 行、code=0000`），
// 且同一趟里 ① 的**对照档**（裸 base64 样图）也成功 ⇒ "样图前缀比不比"那一问同时收口。
// 下面那三格与实拍的偏离**不再是阻塞项**（它们都在"两枪已被接受"的那一趟里带着），
// 但"上游读不读它们"仍是**未测试**：
//   - `x-yun-tid`：实拍是 **36 字符**（UUID 形态），我们按旧实现 `randomHex(8)` 发 16 位十六进制；
//   - ① 实拍**不带** `x-yun-uni`，而 [WithAppHeaders] 那一套公共设备头会补上它；
//   - `userId` 的来源（见下面那一节）。
//
// 两枪都不带票 ⇒ **本族一枪都不需要建会话**。所以这里**不**走 [do]：走 [do] 等于给它套
// 一层"撞 `90001` 刷票重放"，而它压根不带票，那一层只会把"这次失败"变成"多两枪"。
// ② 同理（旧实现那一发也是直发）。
//
// ⚠ 反过来，**不要**为了"给这一页换一张特有的票"去接
// [`upstream.PlayAIQuerySpecTokenRequest` → `PlayAITyrzLoginRequest`](../upstream/app_play_ai_session.go)：
// 那两条是为 `newyear/*` 与埋点那一族（带 `jwttoken` 头）准备的，585 这两口不吃票，
// 接上只会让每次真跑多两枪没人验过的请求。那一族的"公共 / 特有票"仍归
// `play_ai_run.go` 文件头 **P5** 管，与本文件无关。
//
// # 本族**不带 `jwttoken`** ⇒ AGENTS.md §1.12 那一问在这里不适用（留痕）
//
// §1.12 的"特有优先 / 公共兜底 / 来源必须记录"针对的是请求头里那串 `jwttoken`。
// 实拍与旧实现两边都在这里都对得上：① 那条实拍**没有** `jwttoken` 头
// （`docs/capture/app-play-ai-samples.txt` §4），② 那一条同样只带原生侧的 `Authorization`
// ⇒ 这一族用的是登录凭据本身，不是任何一张页面票。
// 因此这里既不写"使用公共来源"，也**不许**将来有人补一句"改用本页的票"——
// 那会凭空多出两枪，而两枪都不解决问题（上面那段就是照这个顺序踩过来的）。
//
// # `userId` 从哪儿来（与旧实现**不同源**的一格，真跑失败先怀疑这里）
//
// 旧实现这一格是**从票里解出来的**：`ai-tasks.ts:59-64` 先把 `parseJwtUserDomainId(jwtToken)`
// 写回 `ctx.state.userDomainId` 再用（`jwtToken` 取 `ctx.http.jwtToken || ctx.state.jwtToken`）。
// 本仓没有"解 JWT 载荷"那一口（全仓零实现），用的是库里那份 `account_state.user_domain_id`
// —— 它的出处是 `thirdlogin` **响应**里的 `userDomainId`（成文见 [ProfileWithStoredDomainID]）。
// ⚠ 2026-09-26 那次真跑顺手看了这张票的载荷一眼：**顶层只有 `exp` / `iat` / `iss` / `sub`
// 四格，没有 `userDomainId`** ⇒ 旧实现那句若真取到过东西，走的是它的第二档回退
// （把 `sub` 再当 JSON 解一遍，`auth/utils.ts:64-67`），那一档本层**没读**。
// ⇒ "票里那一格"与"响应里那一格"**是否恒等仍未测试**。后果值得写清楚，因为它不触发警觉：
// 两值不同时，① 会把这张图识别到**另一个用户域**下（或直接被拒）。要核它：同一趟里把
// `sub` 解出来与库里那份**比对字符串** —— **待验证**登记（`docs/pending-decisions.md` R37），
// 不因此停手。
//
// # ⚠ 这一族**会往用户网盘写一张图**
//
// ① 的 `uploadToCloud` 实拍恒 `true`（[upstream.PlayAIRecognizeUploadToCloud]）⇒
// 一次真跑就在网盘里落一张 100×100 的白图。它**必须**进
// [TaskMatrixRunResult.CloudFileIDs] 那条清理链（链尾 `batchTrash`），口径与 106 / 522 /
// 1014 那几条一致。⚠ 一处**删不掉**的形态要说明：① 说成功却没给 `fileId` 时，
// 这一层拿不到可删的标识 ⇒ 那张图留在网盘里而账上没有（下面的 problem 文本会念出来，
// 但念出来不等于删掉了）。
//
// # 样图为什么选 data URL 那一档
//
// 上游层的 [upstream.PlayAIChatCameraSampleBase64]（旧 TS 的裸 base64）与
// [upstream.PlayAIChatCameraSampleDataURL]（实拍那种带 `data:image/jpg;base64,` 前缀的形态）
// **两档都提供、一个都不内置**，选型落在本调用点：这里取 **data URL** 那一档。
// 理由：本层对 ① 只有**一条**实拍，而它装的就是整条 data URL
// （`docs/capture/app-play-ai-samples.txt` §4）；旧 TS 那一档没有实拍支撑。
// ⚠ "上游比不比这个前缀"**未测试** —— 要收口只需同一天各打一次 ① 比对两次的 `code`
// 与 `success`（AGENTS.md §1.14：以本程序自己的运行反馈为准），登记在 R37。
//
// # `name` / `inputTime` / `sessionId` 三格：本层与实拍的**已知偏离**（都登记在 R37）
//
// 这三格都在 ② 的请求体上。实拍取自 `har/2026-09-25-playai-*.har`（四条里是**同一次会话**
// 的读数 ⇒ 只算一次观测），本层按参考实现落，差别逐格写清：
//   - **`attachment.fileList[0].name`**：真机给的是 `taskId` 的**精确十进制加一** + `.jpeg`
//     （19 位一位不掉）。旧实现 `ai-tasks.ts:99-100` 那句 `parseInt(taskId, 10) + 1`
//     方向与真机一致，但 `parseInt` 得的是 float64 ⇒ 19 位会掉末几位，**复刻它等于复刻一个
//     已经丢精度的读数**。本层用 `taskId` **原值** + `.jpeg`，① 没给 `taskId` 时退到
//     "这一趟的毫秒戳 + `.jpeg`"（旧实现同一处也是退到时间戳）。
//   - **`inputTime`**：真机那一档是**本地挂钟 + 偏移**（实拍形如 `2026-09-25T20:15:19.24+08:00`），
//     本层按旧实现那套给 **UTC + 字面量 `Z`**（[upstream.PlayAIChatCameraInputTime]，值来自
//     注入时钟）。所以"这一套是 UTC+Z"只是**参考实现**的口径，不是真实客户端的口径。
//   - **`sessionId`**：真机那一趟两条共用同一枚非空值，本层传**空串**。
//
// ⚠ 这三格上游**读不读** ⇒ **未测试**（一个方向都没样本，§1.7）。要收口：拿同一枚 `fileId`
// 各发一次 ②、只换其中一格，比对 `code` 与那一条 `data:` 行；或者先看"② 换成整串凭据后
// 会不会通"—— 凭据那一格没通之前，这三格都在它后面。

import (
	"context"
	"fmt"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// playAICameraFileSuffix 是 ② 那个附件的文件名后缀（`ai-tasks.ts:100` 逐字 `.jpeg`）。
//
// ⚠ 它与样图**不是一回事**：那一张是 PNG（魔数就在字节里），`imageExt` 与 MIME 前缀
// 都写 `jpg`，而这里连扩展名都换成了 `jpeg`。三处不一致**全按出处抄**，不"修正"：
// 修正等于替上游宣布它比对这几格，而本层一条样本都没有（同
// [upstream.PlayAIRecognizeParams.ImageExt] 那条理由）。
const playAICameraFileSuffix = ".jpeg"

// playAICameraLogTextRunes 是播报里带上游原文时的截断位数。
//
// 为什么要有这一格：② 回的是模型输出，一段 SSE 里最长的可能就是那条 `data:` 行，
// 而本仓事件流**一行一条**、不过滤长度（与协议层那份 300 码点的错误摘录是两个预算：
// 那一处截的是"错误里的响应体"，这一处截的是"成功路径上的播报"）。
const playAICameraLogTextRunes = 200

// runAICamera 跑玩法 585 的本体，交回**有没有真的发出过请求**与逐格的原因。
//
// 三处判据与 [runMailPDF] 同一取向，各自"改了会怎样"：
//   - **① 失败 ⇒ ② 不发**：② 的 `fileId` 只能是 ① 刚造出来的那一个，没有它
//     连请求都构造不出来（协议层那五格必填会直接拒）。
//   - **① 成功就记账**，哪怕后面 ② 挂了：那张图已经在网盘里了，
//     ② 失败不会替上游删掉它（口径同 [runUploads]"文件 id 一律记账"）。
//   - **② 的成败按旧实现的判据念，但一定念出命中哪一档**：第三档"整段原文当一个字符串、
//     非空即真"会让一段 HTML 报错页被判成**成功**（`app_play_ai_chat_sse.go` 文件头那条），
//     所以 `Success == true` 而 `Source == 原文` 时念的是 **Warn** 并带上原文摘录，
//     不念成"已完成"。这一格不这样做，界面上就会出现一条"成功"而没人看得出它没读到事件。
func (e *taskExecutor) runAICamera(
	ctx context.Context, item upstream.TaskV2Item, record *TaskStepRecord,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	userID, problems := e.playAICameraUserID()
	if userID == "" {
		e.reportAICameraBlocked(name, id, problems)
		return false, problems
	}
	if e.run.Random == nil || e.run.Clock == nil {
		// ⚠ 两格各自挡一次的理由与 [mailSessionFor] 那段相同：接口为 nil 时是 **panic**，
		// 不是回零值（`UserTimestamp` / `core.UUIDv4` 都是直取）。这里一次判完，
		// 免得"随机源有、时钟没有"那种半边接线走到一半才炸。
		blocked := []string{
			"AI 相机没做：" + playAICameraWiringMissing(e.run.Random != nil, e.run.Clock != nil) +
				"（本工具侧缺接线）⇒ 两枪都不发",
		}
		e.reportAICameraBlocked(name, id, blocked)
		return false, blocked
	}
	// 两枪**同一枚凭据**：库里那串 `Basic …` **整串**。
	//
	// ⚠ 这一格是**实拍 + 两次真跑**换出来的，顺序别记反（全过程成文在文件头第一条）：
	//   - 带签到页那张票 ⇒ `01000002 鉴权失败`；
	//   - 带 Basic **第三段那枚裸 authToken**（= 旧实现 `ctx.config.token` 的字面取值）⇒ **同样** `01000002`；
	//   - 真机那两条实拍里 ①② 的 `Authorization` **逐字同一枚**（同长 302、同指纹）。
	// ⇒ 参考实现这一处与真实客户端**不一致**，本仓按实拍落。
	//
	// 也因此这里**不**走 [do]：两枪都不带票，套一层"撞 `90001` 刷票重放"只会把
	// "这次失败"变成"多两枪"。
	authorization := e.opening.Credential.Authorization()
	if authorization == "" {
		// ⚠ 这一格在**整条链**上够不到：[SignInWorld.Open] 自己就先拦"这个号没有可用凭据"
		// （实得 `凭据失效: … authToken 为空`）。留在这里是因为 runner 也能被**直接**调
		// （活体探针就是那一档），而它宁可多发一发必被拒的枪也不要拼出空 Authorization。
		// 断言方式见 `task_matrix_play_ai_run_test.go`：手工把 opening 那一格换空。
		//
		// 错误文本里**不带**那串凭据：成文见 `docs/sanitize-guide.md`。
		blocked := []string{
			"AI 相机没做：这个号的登录态里没有 Authorization 整串 ⇒ 两枪都不发" +
				"（①② 全靠它，本族不带票）",
		}
		e.reportAICameraBlocked(name, id, blocked)
		return false, blocked
	}
	// ① 识图。**写**：往用户网盘落一张图。
	recognized, rerr := e.src.up.PlayAIRecognize(
		ctx, e.opening.Profile, UserTimestamp(e.run.Clock), authorization,
		randomHex(e.run.Random, 8),
		upstream.PlayAIRecognizeParams{
			UserID:   userID,
			DataURL:  upstream.PlayAIChatCameraSampleDataURL(),
			ImageExt: upstream.PlayAIRecognizeImageExt,
		})
	fired := true
	if rerr != nil {
		problem := "AI 相机识图（①）失败：" + rerr.Error() +
			" ⇒ ② 不发（没有 fileId 可问），网盘里也不会多一张图（除非上游其实写成了却没说）"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 相机失败 %s %d：%s", name, id, problem))
		return fired, []string{problem}
	}
	if recognized.FileID == nil || *recognized.FileID == "" {
		// ① 说成功了却没给 fileId：这是"清理链收不到东西"那一格（文件头第三条）。
		problem := "AI 相机识图（①）没回 fileId ⇒ ② 不发；" +
			"⚠ 那张图**可能已经在网盘里**而本层拿不到可删的标识 ⇒ 不在清理链上（R37）"
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 相机跳过 %s %d：%s", name, id, problem))
		return fired, []string{problem}
	}
	fileID := *recognized.FileID
	e.cloudFileIDs = append(e.cloudFileIDs, fileID)
	record.Uploads++
	e.run.progress(core.LevelInfo, fmt.Sprintf("AI 相机识图已回 fileId %s %d", name, id))

	// ② 对话。凭据与 ① **同一枚**（实拍逐字相同；旧实现这一发取 `ctx.config.auth`，
	// 与它自己 ① 取的那一枚并不相同 —— 见上面那一段）。同样**不**走 [do]。
	//
	// 一次读数喂两格（`inputTime` 与"① 没给 taskId 时的文件名"）：旧实现是每格各取一次
	// `new Date()`，同一趟里差几毫秒没有可观察后果，而**这里必须用注入的时钟** ——
	// 直接 `time.Now()` 会让这条链在夹具里不可复现（AGENTS.md 那一族口径）。
	chatWhen := e.run.Clock.Now()
	fileName := playAICameraFileName(recognized.TaskID, chatWhen.UnixMilli())
	read, cerr := e.src.up.PlayAIChatCamera(
		ctx, e.opening.Profile, UserTimestamp(e.run.Clock),
		authorization, randomHex(e.run.Random, 8),
		upstream.PlayAIChatParams{
			UserID:    userID,
			Dialogue:  upstream.PlayAIChatCameraDialogue,
			InputTime: upstream.PlayAIChatCameraInputTime(chatWhen),
			FileID:    fileID,
			FileName:  fileName,
		})
	if cerr != nil {
		problem := "AI 相机对话（②）失败：" + cerr.Error() +
			" ⇒ ① 那张图仍会在链尾被移进回收站（账已记）"
		e.run.progress(core.LevelError, fmt.Sprintf("AI 相机失败 %s %d：%s", name, id, problem))
		return fired, []string{problem}
	}
	return fired, e.reportAICameraChat(name, id, read)
}

// reportAICameraBlocked 念"一枪都没发"那一档。
//
// 为什么这一层要自己念：[apply] 只把原因写进 [TaskStepRecord.Err]（那是给小结与
// 界面用的），**不**往事件流里发逐条失败行 —— 本族其他几条（识图失败 / 没 fileId /
// 对话失败）都是各自念各自的。少了这一行，界面上就只剩一句"尝试完成 AI 相机…"
// 后面什么都没有，而"为什么没动"正是用户唯一想知道的那一格。
func (e *taskExecutor) reportAICameraBlocked(name string, id int64, problems []string) {
	e.run.progress(core.LevelError, fmt.Sprintf("AI 相机失败 %s %d：%s",
		name, id, joinProblems(problems)))
}

// reportAICameraChat 把 ② 的读数念成事件，并交回要并进结论的原因（成功时交回空）。
//
// 单独成函数只为一件事：**三档 Source 的措辞**是这一族唯一会"把没读到读成读到了"的地方，
// 它必须集中在一处、可被一条用例逐字钉住（散在 [runAICamera] 里就早晚只钉到两档）。
//
// 为什么"成功"也可能交回一条 problem：第三档（原文非空即真）按旧实现的判据**是**成功，
// 而它对人的意义是"这一发的响应里一条 `data:` 事件都没有"⇒ 记 Warn 并带原因，
// 但不改判据（改判据就不是照抄参考实现了，§1.9）。
func (e *taskExecutor) reportAICameraChat(
	name string, id int64, read upstream.PlayAIChatSSERead,
) []string {
	line := fmt.Sprintf("%s %d", name, id)
	switch {
	case !read.Success:
		problem := "AI 相机对话（②）上游没判成成功：" + playAICameraReadLine(read)
		e.run.progress(core.LevelError, fmt.Sprintf("AI 相机失败 %s：%s", line, problem))
		return []string{problem}
	case read.Source == upstream.PlayAIChatSSEFromRawText:
		problem := "AI 相机对话（②）按旧实现判据算成功，但**一条 data 事件都没读到**" +
			"（整段原文当一个字符串、非空即真 ⇒ 一段报错页也是这一档）：" + playAICameraReadLine(read)
		e.run.progress(core.LevelWarn, fmt.Sprintf("AI 相机存疑 %s：%s", line, problem))
		return []string{problem}
	default:
		e.run.progress(core.LevelInfo,
			fmt.Sprintf("AI 相机对话已回 %s：%s", line, playAICameraReadLine(read)))
		return nil
	}
}

// playAICameraUserID 取两枪 body 里那个 `userId`（19 位标识，**不是**凭据）。
//
// 交回空串 ⇒ 调用方一枪都不发：这一格填错的表现是"打到别人的用户域"（协议层那句护栏
// 的理由，见 [upstream.PlayAIRecognizeRequest]），比"这一条没做成"重得多，
// 所以宁可不发也不猜。来源与"为什么不是票里那一格"写在文件头。
func (e *taskExecutor) playAICameraUserID() (string, []string) {
	domain := e.opening.Profile.UserDomainID()
	if domain == nil || *domain == "" {
		return "", []string{
			"AI 相机没做：这个号还没有 userDomainId（库里那份是登录响应给的）⇒ 两枪都不发" +
				"（这一格填错会打到别人的账号上，宁可不发）",
		}
	}
	return *domain, nil
}

// playAICameraFileName 拼 ② 那个附件的文件名（取舍见文件头第五条）。
//
// ① 没回 `taskId` 时退到**毫秒戳**：旧实现同一处也是退到时间戳（`ai-tasks.ts:99`
// 那句 `|| Date.now()`），两档只差在"有没有 +1"，而那一格本仓不复刻。
func playAICameraFileName(taskID *string, millis int64) string {
	if taskID != nil && *taskID != "" {
		return *taskID + playAICameraFileSuffix
	}
	return strconv.FormatInt(millis, 10) + playAICameraFileSuffix
}

// playAICameraReadLine 把 ② 的读数摊成**一行**人话（进事件流，也进 [TaskStepRecord.Err]）。
//
// 六格逐字各有理由：
//   - `档` = [upstream.PlayAIChatSSESource.String]：没有它，"读到一条事件"与
//     "整段报错页被当成一个字符串"是同一句话（这一族最贵的一种误读）。
//   - `data 行数` = 上游今天到底回没回 SSE 最便宜的读数（见 Events 那条）。
//   - `code` / `message` 只在对象档有值；**原样带出**，不折成"码 0"那种归一。
//     ⚠ 实拍里 `message` 是"请求成功"这一类**信封**文案，不是模型那一句 ——
//     那一句在 `模型回话`。
//   - `模型回话` = [upstream.PlayAIChatSSERead.Answer]（逐行累加的分片）。**只有这一格**
//     念的是内容；没有它时这一条"成功"只是上游没说错，界面上一律仍要念出档位。
//   - `原文` **只在没读到回答时**念（那时它是唯一证据），并过 [playAICameraLogText] 截断。
//
// ⚠ 这里念的是**响应**侧的文本，本族响应里不含凭据（凭据全在请求头）⇒ 可截断后入日志；
// 换成任何**请求**侧的值（票、Basic 整串、`tid`）一律不进 ——
// `docs/sanitize-guide.md`。
func playAICameraReadLine(read upstream.PlayAIChatSSERead) string {
	text := "AI 相机响应：档=" + read.Source.String() +
		"、data 行数=" + strconv.Itoa(read.Events)
	if read.Code != "" {
		text += "、code=" + read.Code
	}
	if read.Message != "" {
		text += "、message=" + playAICameraLogText(read.Message)
	}
	if read.Answer != "" {
		// 这才是模型那一句（逐行累加，见 `PlayAIChatSSERead.Answer`：末行那一条按实拍是空的）。
		text += "、模型回话=" + playAICameraLogText(read.Answer)
	}
	if read.Line != "" && read.Answer == "" {
		// 只在**没读到回答**时念原文：那时它是唯一的证据。读到回答之后再念一份信封原文，
		// 等于用一长串把要紧的那一句挤出这一行。
		text += "、原文=" + playAICameraLogText(read.Line)
	}
	return text
}

// playAICameraLogText 把一段上游文本摊成可进事件流的**一行**：换行/制表换成空格，
// 再按码点截到 [playAICameraLogTextRunes]（按字节截会把中文切成半个）。
func playAICameraLogText(text string) string {
	flat := strings.Map(func(r rune) rune {
		if r == '\n' || r == '\r' || r == '\t' {
			return ' '
		}
		return r
	}, strings.TrimSpace(text))
	runes := []rune(flat)
	if len(runes) > playAICameraLogTextRunes {
		return strconv.Quote(string(runes[:playAICameraLogTextRunes])) +
			"…（截断，共 " + strconv.Itoa(len(runes)) + " 码点）"
	}
	return strconv.Quote(flat)
}

// playAICameraWiringMissing 说清"哪一格没接"（而不是笼统写一句"缺接线"）：
// 两格的修法不同 —— 随机源在 [TaskMatrixRun.Random]，时钟在 [TaskMatrixRun.Clock]。
func playAICameraWiringMissing(hasRandom, hasClock bool) string {
	switch {
	case hasRandom && !hasClock:
		return "时钟没接（① 的签名时间戳与 ② 的 inputTime 都由它造，本层不读 time.Now()）"
	case !hasRandom && hasClock:
		return "随机源没接（两枪各带一枚 x-yun-tid，旧实现是 randomHex(8)）"
	default:
		return "随机源与时钟都没接"
	}
}
