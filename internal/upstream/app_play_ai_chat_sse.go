// 585「AI 相机」那一发 `POST …/assistant/chat/v2/add` 的**响应侧**，
// 以及**它自己的那一条请求**（与 #16 拍拍那一族的 chat **不是同一套 schema**）。
//
// # 本文件为什么单独存在
//
// [`app_play_ai_session.go`](app_play_ai_session.go) 的文件头原来写着两句：
// "这一条的响应体**零样本** ⇒ 不猜事件名、不写解析器，也**不提供** `(u *Upstream).PlayAIChat*`
// 那个发送方法"，并且用一条反射测试扫住这个口子。
// ⇒ 现在这两句**已经改了**（就地改，不留"注释说 A 代码做 B"，AGENTS.md §2.6）：
// 本文件给了 chat 的读法、判据与发送口，但**只给 585 这一套**，拍拍那一套仍然不发送。
//
// 改口的理由不是"攒够样本了"，而是用户 2026-09-25 定的口径
// （AGENTS.md §1.13/§1.14）：**缺抓包不是停工理由，而这一族的样本可以自己造** ——
// AI 相机是一条可点的玩法，本程序真打一次就会吐出第一段 SSE 原文。
// 所以处置从"等抓包才写解析器"换成"**先按旧 TS 的字面读法落码 → 真跑一次 → 用程序自己的读数更正**"。
// 那条反射测试没有删，改成钉"拍拍那一套仍不发送 + 585 这一套发送且只此一家"。
//
// ⚠ 本文件写完之后又挖到两条**真机 ② 的 SSE 实拍**（`har/2026-09-25-playai-*.har`：
// 一组读数在四条 har 里逐字重复 ⇒ 只算**一次**观测，另一组两条 ⇒ 一共**两次**观测；
// 已按"只记键与长度"脱敏登记进 `docs/capture/app-play-ai-samples.txt`）。
// 它和下面 B 档那套读法**对不上的一格**是：模型那句话在**逐行累加**的
// `data.flowResult.outContent` 里（末行那一条**是空的**），而 `message` 是"请求成功"
// 这一类信封文案 ⇒ 只读 `message` 的播报会把"请求成功"当成模型回答念。
// 本层的处置是 [PlayAIChatSSERead.Answer]：**不动** B 档那套判据（`Success` 仍按旧 TS 读末条），
// 只多带一个内容读数。
//
// # ⚠ 三套 `chat/v2/add`，同一 URL、三套头与体
//
//	① 拍拍（#16）  `packages/core/src/tasks/play-ai-special/task.ts:170-208`
//	              ⇒ 已落码：[PlayAIChatBody] / [PlayAIChatRequest]（**仍无发送口**）
//	② AI 相机（585）`packages/core/src/tasks/ai-tasks.ts:105-150`
//	              ⇒ 本文件的 [PlayAIChatCameraBody] / [PlayAIChatCameraRequest] + 发送口
//	③ AI 工作台（1028）`packages/core/src/apis/ai-workbench.ts:161-285`
//	              ⇒ 本仓**另有一份**：[`app_ai_workbench.go`](app_ai_workbench.go)（那里连 SSE
//	                累加器也是本族自己的一套，因为它的行**不带** `data:` 前缀也要 parse）。
//	                ③的头里连 `x-yun-tid`、`Origin` 都没有，UA 是桌面 Chrome，
//	                body 多 `continuationInfo` / `commands` / `aiWritingSetting` 三格、
//	                `attachment` 是**空对象** —— **不要**复用本文件（差别见 `docs/pending-decisions.md` 里 1028 那一格）。
//
// 三套的差别不是风格：同一主机同一 path，**头集合与 body 键集都不同**，
// 而 `Authorization` 三套**来源各异**（①：这一页自己的换票链；②：账号的 Basic 整串；③：邮箱侧 DES 派生出来的 Basic）。
// ⇒ 这正是 §1.12 事实②那句"URL 相同而参数不同"的极端形态：**URL 对上完全不等于同一条请求**。
// 本文件的测试里有一条专门把①②两套的键集差列出来钉住，防止将来有人"顺手合并成一个 body 构造器"。
//
// # 证据等级（沿用 [`app_play_ai_session.go`](app_play_ai_session.go) 的 A/B/C 三档口径）
//
//   - **A 档（本仓实拍）**：② 的**请求与响应**各有一条（同一次会话）—— 请求侧
//     `Authorization` 与 ⑤ 逐字同一枚、`inputTime` 是本地挂钟 + `+08:00`、`sessionId` 非空；
//     响应侧**两次观测**：47 行（`index` 0→46、累加 70 码点）与 386 行（0→385、2129 码点），
//     两条的末行都是 `finishReason:"stop"` 且 `outContent` 为空。⑤ `aiRecognize` 那一条见
//     `docs/capture/app-play-ai-samples.txt` §4。**仍为 A 档空白**的是：⑤ 的失败响应形状、
//     以及"同一格换一种写法上游认不认"（样图前缀那一问已经有本程序自己的对照读数，
//     见 `internal/app/task_matrix_play_ai.go` 与 `docs/status.md`）。
//   - **B 档（旧 TS 字面量）**：本文件的**判据与三档回退读法**逐字出处 `ai-tasks.ts:151-169`
//     与 `:46-51`。⚠ 它与 A 档在"`message` 是不是模型那句话"上**不一致**（见文件头那条）：
//     这一格的处置是**分工**而不是二选一 —— `Success`/`Code`/`Message` 完全按 B 档读末条
//     （那是参考实现的判据，改它要动黄金判据），"内容"另走 A 档支撑的 [PlayAIChatSSERead.Answer]。
//   - **C 档（本仓已实跑通的同族）**：头集合复用 [playAIAppHeaders]（⑤ 那一族实拍裁出来的那一套），
//     以及 [Envelope] 那条 `UseNumber` 口径（`docs/capture/app-signin-samples.txt` 端到端复现过）。
//
// ⇒ 于是本文件的句子只允许三种："我们发出的这一组恰好是这个形状"（测试钉）、
// "这个形状出自旧 TS 的哪一行 / 哪一条实拍"（注释钉）、"这一格**未测试**"。
// **不允许**"上游会回 `data:` 事件"或"上游不校验 command 那一格"这种话 ——
// 前者现在有实拍撑着，但那是**同一次会话里的两次**观测，推不出"总是这样"；
// 后者两个方向都没样本（§1.7）。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 585 的请求侧：它自己的那几格字面量（B 档：`ai-tasks.ts:105-130` 逐字段）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// PlayAIChatCameraH5Version 是 585 这一套的 `dialogueInput.versionInfo.h5Version`
	//（`ai-tasks.ts:121` 逐字 `"2.7.6"`）。
	//
	// ⚠ 它**不等于**① 拍拍那一套的 [PlayAIChatH5Version]（`"1.0.0"`，出处 `task.ts:189`）。
	// 两值不同 ⇒ 这不是"抄错了一处"，是两条请求本来就带两个版本号。
	// 为什么不做成一个参数：调用方没有"该填哪个版本"的出处，给自由口子只会发明第三档。
	PlayAIChatCameraH5Version = "2.7.6"

	// PlayAIChatCameraDialogue 是 585 那一套里 `dialogue` 的**固定值**（`ai-tasks.ts:112` 逐字
	// 一个全角问号 `"？"`）。
	//
	// ⚠ 它**不是兜底串**：① 那一套的 `dialogue` 由调用方给（旧 TS 取 `recommendList[0]`，
	// 见 [PlayAIChatParams.Dialogue]），而 585 这一套在旧实现里**从来就是**这一个字符。
	// 做成常量而不是参数，是为了让"这一页问的是哪一句"在成文里可核对；
	// [PlayAIChatParams.Dialogue] 仍然必填 ⇒ 调用方要问别的句子时**看得见**自己在偏离出处。
	PlayAIChatCameraDialogue = "？"
)

// PlayAIChatCameraSampleBase64 是旧 TS 给 585 用的那张样图（`ai-tasks.ts:13` 逐字，
// 注释自陈"100x100 白色 PNG"）。
//
// ⚠⚠ 它是**裸 base64**（`iVBOR…` 起头，**没有** `data:image/jpg;base64,` 前缀），
// 而本仓 ⑤ 的**唯一定拍**里 `base64` 那一格装的是**整条 data URL**
// （`data:image/jpg;base64,…`，`docs/capture/app-play-ai-samples.txt` §4，
// 也写进了 [PlayAIRecognizeParams.DataURL]）。
// ⇒ 两档**只有一种实拍**（data URL），旧 TS 那一档**没有实拍支撑**，
// 而上游是否比对前缀 ⇒ **未测试**（一条样本说不出"少前缀会不会被拒"）。
// 所以本层**两个都提供**、一个都不内置：
//   - [PlayAIChatCameraSampleBase64]：照旧 TS 的裸串（`imageExt` 仍写 `jpg`，见下一条的矛盾）；
//   - [PlayAIChatCameraSampleDataURL]：照实拍那一条的形态。
//
// 选哪一档由 app 层在**调用点**写明理由（当前选实拍那一档，理由见
// `internal/app/task_matrix_play_ai.go` 的 [runAICamera]），并登记成一条待验证：
// 要收口只需在同一天各打一次 ⑤，比较两次 `code`/`success`（§1.14：以本程序的运行反馈为准）。
const PlayAIChatCameraSampleBase64 = "iVBORw0KGgoAAAANSUhEUgAAAGQAAABkCAIAAAD/gAID" +
	"AAAACklEQVR42u3BAQEAAACCIP+vbkhAAQAAAO8GECAAASFhMcYAAAAASUVORK5CYII="

// PlayAIChatCameraSampleDataURL 把那张样图摊成**实拍里那种形态**的 `base64` 格：
// `data:image/jpg;base64,` + [PlayAIChatCameraSampleBase64]。
//
// ⚠ 前缀里写的是 **`image/jpg`** 而不是 `image/png` —— 而那串字节解出来是一张 **PNG**
// （PNG 的魔数 `89 50 4E 47` 就在 `iVBORw0KGgo` 这一段里）。
// 这不是我们的笔误：旧 TS 的 `imageExt` 也写 `jpg`（`ai-tasks.ts:77`），实拍那条同样写 `jpg`。
// ⇒ **"MIME 与真实格式不一致会不会被拒"未测试**（零对照样本）。不"修正"成 png 的理由与
// [PlayAIRecognizeParams.ImageExt] 同一条：修正等于替上游宣布它比对两者，而本层没资格。
//
// 为什么前缀做成函数而不是常量：它是两格拼出来的（前缀 + 那张图），
// 常量会把"图换了前缀没换"这一类错误变成可能 —— 函数保证两半**同源自同一张图**。
func PlayAIChatCameraSampleDataURL() string {
	return "data:image/jpg;base64," + PlayAIChatCameraSampleBase64
}

// PlayAIChatCameraInputTime 按 585 那一套的 `inputTime` 把一个时刻摊成文本
// （`ai-tasks.ts:103` 是 `new Date().toISOString()`）：
//
//	YYYY-MM-DDTHH:mm:ss.SSSZ     （UTC 挂钟 + 字面量 Z）
//
// ⚠⚠ 它与① 那一套的 [PlayAIChatInputTime] **不是同一个格式**：那一条取**本地挂钟分量**
// 再拼一个**写死的** `+08:00`（旧 JS 的语义，出处 `task.ts:72-76`），这一条是真正的 UTC + `Z`。
// 两者对同一个时刻会给出**不同**的串。
// ⇒ 各按各的出处实现，不统一（统一 = 让其中一条偏离它唯一有出处的写法）。
// 上游对这两种写法是否都认 ⇒ 未测试（本文件零样本）。
func PlayAIChatCameraInputTime(at time.Time) string {
	// `Z` 在这里是**字面量**（Go 的参照布局只在 `Z07:00` / `Z0700` 里认它），
	// 而时刻已经 `.UTC()` ⇒ 与 JS 的 `toISOString()` 同形：固定三位小数、固定 `Z`。
	return at.UTC().Format("2006-01-02T15:04:05.000Z")
}

// PlayAIChatCameraBody 拼 585 那一套的 `chat/v2/add` 请求体
// （逐字段照 `ai-tasks.ts:105-130`）。
//
// 入参复用① 拍拍那一套的 [PlayAIChatParams]（五格**完全同名同义**：`userId` / `dialogue` /
// `inputTime` / `fileId` / `name`），差异全在**固定格**上：
//   - **没有** `dialogueInput.command` 那一格（① 有 `command:{command,subCommand}`，出处 `task.ts:192`）；
//   - `versionInfo.h5Version` 是 [PlayAIChatCameraH5Version]（`2.7.6`），① 是 `1.0.0`；
//   - 余下七枚开关与 `extInfo` / `sortInfo` / `toolSetting` / `attachment` 两格**逐字相同**
//     （两页在旧 TS 里就是同一组布尔抄了两遍）。
//
// ⚠ 键序与①（拍拍那一套）同为"由 Go map marshal 出来的字典序"，那**不是**对拍面（§1.4：int 宽度、
// 参数写法、文件布局不算对拍面；这里的冻结基准是"键集与每格取值"）。
func PlayAIChatCameraBody(params PlayAIChatParams) map[string]any {
	return map[string]any{
		"userId":          params.UserID,
		"sessionId":       "",
		"applicationType": PlayAIChatApplicationType,
		"applicationId":   "",
		"sourceChannel":   PlayAIChatSourceChannel,
		"dialogueInput": map[string]any{
			"dialogue":                        params.Dialogue,
			"prompt":                          "",
			"inputTime":                       params.InputTime,
			"enableForceLlm":                  PlayAIChatEnableForceLlm,
			"enableForceNetworkSearch":        PlayAIChatEnableForceNetworkSearch,
			"enableModelThinking":             PlayAIChatEnableModelThinking,
			"enableAllNetworkSearch":          PlayAIChatEnableAllNetworkSearch,
			"enableKnowledgeAndNetworkSearch": PlayAIChatEnableKnowledgeAndNetSearch,
			"enableRegenerate":                PlayAIChatEnableRegenerate,
			"versionInfo":                     map[string]any{"h5Version": PlayAIChatCameraH5Version},
			"extInfo":                         PlayAIChatExtInfo,
			"sortInfo":                        map[string]any{},
			"toolSetting": map[string]any{
				"imageToolSetting": map[string]any{"enableLlmDescribe": PlayAIChatEnableLlmDescribe},
			},
			"attachment": map[string]any{
				"attachmentTypeList": []int64{PlayAIChatAttachmentTypeImage},
				"fileList": []map[string]any{{
					"fileId": params.FileID,
					"name":   params.FileName,
				}},
			},
		},
	}
}

// PlayAIChatCameraRequest 构造 585 那一发的 `POST https://ai.yun.139.com/api/outer/assistant/chat/v2/add`。
//
// 头：复用 [playAIAppHeaders]（⑤ 那一族按实拍裁出来的原生侧那套）+ 把 `Accept` 换成
// [PlayAIChatSSEAccept] —— 与 [PlayAIChatRequest] **同一处理、同一处偏离登记**：
// `x-yun-client-info` / `x-DeviceInfo` 用仓库那**一套**画像，而旧 TS 这一页写死的是
// `||8||||||||||||` 那一串 iPhone 形态（`ai-tasks.ts:37`）；`x-yun-app-channel` 同理。
// ⇒ 三条已知偏离（画像、渠道号、`content-type` 不带 charset 之外的选择）逐字登记在
// [PlayAIChatRequest] 头上，本文件不重复一遍，也不在这里再分叉一次。
//
// ⚠ `Authorization` 由调用方给**整个 Basic 串**，与 ⑤ 那一发是**同一枚**（实拍里两条
// 的 `Authorization` 逐字相同：同长、同指纹 ⇒ 见 `docs/capture/app-play-ai-samples.txt`）。
// 参考实现不是这样：它这里取 `ctx.config.auth`（整串）、⑤ 取 `ctx.config.token`
// （`parseAuth(auth)` 摊出来的**第三段裸 authToken**，`utils/init-account.ts:52` +
// `auth/utils.ts` 的 `doParseAuth`）⇒ **它自己那条链上两发用的凭据并不相同**，
// 而它的 ① 与真实客户端不同。两枚**都不是任何一张 `jwttoken`**。
// 这是 585 最容易接错的一格：三种接法（都传整串 / ① 传裸 authToken / ① 传票）里
// 后两种本仓都**真跑过**，① 都回 `code:"01000002"` / `鉴权失败`（2026-09-26，
// 纠正全程成文在 [`internal/app/task_matrix_play_ai.go`](../app/task_matrix_play_ai.go)
// 文件头第一条）。测试里按值钉住的是"两枪同一枚整串"。
func PlayAIChatCameraRequest(
	profile *AppProfile, timestamp int64, authorization, tid string, params PlayAIChatParams,
) (core.HTTPRequest, *core.Error) {
	if perr := playAIChatParamGuard(params); perr != nil {
		return core.HTTPRequest{}, perr
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/api/outer/assistant/chat/v2/add", PlayAIHostAI)).
		WithJSONBody(PlayAIChatCameraBody(params))
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = playAIAppHeaders(request, profile, timestamp, authorization, tid)
	request.Headers = dropHeader(request.Headers, "accept")
	return request.WithHeader("Accept", PlayAIChatSSEAccept), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应侧：把一段 SSE 文本摊成"旧 TS 会拿去判成败的那一个值"
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIChatSSESource 标记 [PlayAIChatSSERead.Value] 是从哪一档来的。
//
// 为什么要这一格：三档回退的**判据不同**（前两档看对象字段，第三档只看"文本非空"），
// 播报里如果不说清命中了哪一档，一次"整段原文被当成一个字符串而判成功"
// 与一次"读到一条 data: 事件而判成功"看起来一模一样。
type PlayAIChatSSESource int

const (
	// PlayAIChatSSEFromDataLine 命中倒序扫到的第一条可解析 `data:` 行（`ai-tasks.ts:153-162`）。
	PlayAIChatSSEFromDataLine PlayAIChatSSESource = iota + 1
	// PlayAIChatSSEFromWholeBody 没有任何可解析的 `data:` 行，退到整体 parse（`:165-168`）。
	PlayAIChatSSEFromWholeBody
	// PlayAIChatSSEFromRawText 整体也不是 JSON，退到"原文当一个字符串值"（`:169`）。
	//
	// ⚠ 这一档**不是**"上游没回东西"：旧 TS 走到这里之后，
	// 判据是"这串非空就算成功"（`:48`）⇒ 一段 HTML 报错页会被判成**成功**。
	// 本层照它实现（这是"照抄参考实现"，§1.9），并把这个后果写进播报：见 [PlayAIChatSSERead.Source]。
	PlayAIChatSSEFromRawText
)

// String 让这一格能直接进事件流文本（避免每处调用点各写一遍 switch）。
func (s PlayAIChatSSESource) String() string {
	switch s {
	case PlayAIChatSSEFromDataLine:
		return "data 行"
	case PlayAIChatSSEFromWholeBody:
		return "整体 JSON"
	case PlayAIChatSSEFromRawText:
		return "原文"
	default:
		return "未知档"
	}
}

// PlayAIChatSSERead 是 585 那一发响应的读数。
//
// 字段少是有意的：实拍那两条 ② 响应里出现的键比这里多得多（`flowResult.outContent`、
// `finishReason`、`responseStatement`、`middleRecommend`、`title`…），
// 而本层只带出"旧 TS 会读的那几格 + 播报要念的那几格"，
// 不去声明一个"看起来完整"的事件结构（那会把两次观测摊成一份 schema）。
type PlayAIChatSSERead struct {
	// Source 是命中的那一档（见 [PlayAIChatSSESource]）。
	Source PlayAIChatSSESource
	// Line 是命中那一行**剥掉 `data:` 前缀并 trim 之后**的原文；
	// 只有 [PlayAIChatSSEFromDataLine] 那一档非空。
	//
	// 它单独带出来的理由：真跑时第一段 SSE 原文要靠它落地，
	// 而 `Value` 已经是解析后的通用值，形状信息与原文对不上了。
	Line string
	// Value 是解析结果（对象 / 数组 / 数字 / 布尔 / 字符串），第三档是 `string`（整段原文）。
	//
	// ⚠ 数字一律是 [json.Number]（[core.ParseJSON] 开 `UseNumber`）：
	// 过一遍 float64 会把 19 位标识静默丢精度，而 `code === 0` 与 `code === "0"`
	// 在判据里**不是一回事**（见 [playAIChatSSESuccess]）。
	Value any
	// Success 是旧 TS `isAiChatSuccess`（`:46-51`）对 [Value] 的判定。
	Success bool
	// Code 只在 `Value` 是对象且带 `code` 时非空：数字与字符串都摊成**串**原样带出，
	// 只为播报与归因（判据不用它，见 [Success] 那条）。
	Code string
	// Message 同上，取 `message`，再退 `msg`（两族在这一格用不同键名，见 [Envelope].Message）。
	Message string
	// Events 是文本里 `data:` 行的**条数**（不是可解析的条数）。
	//
	// 为什么带它：它是"上游今天到底回没回 SSE"最便宜的一个读数，
	// 而且能分开两种"成功"：一条 `data:` 事件（`Events=1`）与一段 HTML 报错页
	// （`Events=0` 且 `Source=原文`）在 `Success` 上可以是同一个值。
	Events int
	// Answer 是把**每一条** `data:` 行里的 `data.flowResult.outContent` 按出现顺序拼起来。
	//
	// ⚠ 为什么非要"拼"而不能读末行（两条真机 ② 实拍，同一次会话各一条，
	// `docs/capture/app-play-ai-samples.txt` §8）：
	//   - 47 行那条：`index` 从 0 严格 +1 递增到 46、每片 1–3 码点、**最后一条**（`finishReason:"stop"`）
	//     的 `outContent` 是**空串**，拼完 70 码点、以句号收尾；
	//   - 386 行那条：同样的形状（`index` 0→385、每片 1–13 码点、末条空），拼完 2129 码点。
	//
	// 倒序取一条的那套判据（[Success]，旧 TS 原样）拿到的正好是**末条** —— 它判成败够用
	// （两条实拍里每行的 `code` 都是 `"0000"`），拿文本**一定拿不到**（末条那一片是空的）。
	//
	// 所以这一格是**本仓新增**的读数，不是参考实现有的：旧 TS 只把 585 的 chat 返回当布尔用
	// （`isAiChatSuccess`），它压根不念模型说了什么（它的累加器在**另一族** 1028 那边）。
	// 本层加它的理由是播报 ——
	// 一条"成功"却不带任何内容的事件流，与旧实现那条"整段报错页也算成功"是同一类误读。
	//
	// ⚠ 只认 `data.flowResult.outContent` 这一条路径。另一条 `data.outContent`
	// （旧 TS 给 1028 那一族写的累加器读它）在两条实拍里**逐行都是空**（非空的行数 = 0）
	// ⇒ 585 这一族不读它，也**不**因此断言"上游永远不会用它"（那两行只说明这两条样本）。
	Answer string
}

// ParsePlayAIChatSSE 把一段响应文本摊成 [PlayAIChatSSERead]，逐字照 `ai-tasks.ts:151-169`：
//
//	① 按 `\n` 切行、**倒序**扫，第一条以 `data:` 起头且 `slice(5).trim()` 能 parse 的行就是结果；
//	② 一条都没有 ⇒ 整体 parse；
//	③ 还不行 ⇒ 整段原文当一个字符串值。
//
// 两处细节容易被"顺手整理"掉，都在测试里钉着：
//   - **倒序**：取的是**最后**一条可解析的 `data:` 行（流式响应里它是终态那一条），
//     正序实现会读到**首**事件，而首事件的 `code` 与终态那条可以不同 ⇒ 判据会跟着变；
//   - **解不开就跳过**：一条 `data:` 行解不开时**继续往前扫**（旧 TS 的 `catch` 里是空的），
//     不是"命中第一条 `data:` 行然后失败"。
//
// 它**不报错**：三档回退的含义就是"无论上游回什么都给出一个可判的值"。
// ⚠ 这也意味着"上游回了一段垃圾"在这里表现为 `Source=原文`，不是 error ——
// 调用方要把 `Source` 念出来，别只念 `Success`。
func ParsePlayAIChatSSE(text string) PlayAIChatSSERead {
	lines := strings.Split(text, "\n")
	// 先整趟数一遍 `data:` 行的条数，再倒序找那一条要用的。
	// 为什么分两趟：旧 TS 只取一条（它不数），而本层要把"上游今天到底回没回 SSE"
	// 念出来（见 [PlayAIChatSSERead.Events]）。它在倒序循环里数会只数到**命中之后**的那几行 ——
	// 那是个看着合理、每次都小的读数，正是本仓登记过的那一类"失败时什么都没有"（#165 同族）。
	events := 0
	var answer strings.Builder
	for _, line := range lines {
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		events++
		// 同一次遍历里顺手累加回答（理由见 [PlayAIChatSSERead.Answer]：末行那一条是空的）。
		// 解不开的行**跳过**而不是中断：一条流里混一行非 JSON 不该让我们丢掉前后都读得到的内容。
		answer.WriteString(playAIChatSSEChunk(line[len("data:"):]))
	}
	// 倒序：从数组尾部往头部走（`lines.reverse()` 的等价写法，不改调用方传进来的切片）。
	for i := len(lines) - 1; i >= 0; i-- {
		line := lines[i]
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		payload := strings.TrimSpace(line[len("data:"):])
		value, ok := core.ParseJSON(payload)
		if !ok {
			continue
		}
		return playAIChatSSERead(PlayAIChatSSEFromDataLine, payload, value, text, events, answer.String())
	}
	if value, ok := core.ParseJSON(text); ok {
		return playAIChatSSERead(PlayAIChatSSEFromWholeBody, "", value, text, events, answer.String())
	}
	return playAIChatSSERead(PlayAIChatSSEFromRawText, "", text, text, events, answer.String())
}

// playAIChatSSEChunk 从**一条** `data:` 的载荷里取 `data.flowResult.outContent` 那一格。
//
// 取不到就交回空串，**不**报错也不猜别的键：见 [PlayAIChatSSERead.Answer] 那两条
// （"只认这一条路径"与"另一条路径没有样本"）。
func playAIChatSSEChunk(payload string) string {
	value, ok := core.ParseJSON(strings.TrimSpace(payload))
	if !ok {
		return ""
	}
	return playAIChatSSEChunkValue(value)
}

// playAIChatSSEChunkValue 是那一格的取值：只接受字符串，其余形态（数字 / null / 缺席）
// 一律当"这一行没有内容分片"。
func playAIChatSSEChunkValue(value any) string {
	root, ok := value.(map[string]any)
	if !ok {
		return ""
	}
	data, ok := root["data"].(map[string]any)
	if !ok {
		return ""
	}
	flow, ok := data["flowResult"].(map[string]any)
	if !ok {
		return ""
	}
	chunk, _ := flow["outContent"].(string)
	return chunk
}

// playAIChatSSERead 组装读数并把三处计数一次算清（三个回退分支共用，免得各写一遍判据）。
func playAIChatSSERead(
	source PlayAIChatSSESource, line string, value any, text string, events int, answer string,
) PlayAIChatSSERead {
	read := PlayAIChatSSERead{
		Source:  source,
		Line:    line,
		Value:   value,
		Success: playAIChatSSESuccess(value),
		Events:  events,
		Answer:  answer,
	}
	if read.Answer == "" {
		// 第二档（整体 JSON、一条 `data:` 都没有）里同一格路径也读一次：
		// 那不是"猜上游会这么回"，而是"这一份已经解析出来的对象里有没有那一格"。
		read.Answer = playAIChatSSEChunkValue(value)
	}
	if obj, ok := value.(map[string]any); ok {
		read.Code = playAIChatSSEScalar(obj, "code")
		read.Message = playAIChatSSEScalar(obj, "message")
		if read.Message == "" {
			read.Message = playAIChatSSEScalar(obj, "msg")
		}
	}
	return read
}

// playAIChatSSEScalar 把对象里一格摊成**可播报的串**：只有字符串与数字两种形态给值，
// 其余（对象 / 数组 / null / 布尔）给空串。
//
// 为什么不像 [Envelope].Code 那样统一成 int64：这一族的 `code` 在旧 TS 的判据里
// **同时**接受字符串 `"0000"` 与数字 `0`（`:49`），摊成 int64 会把 `"0000"` 变成 `0` ——
// 那正好抹掉"两种形态"这一格信息，而它在本族**零样本**，抹不得。
func playAIChatSSEScalar(obj map[string]any, key string) string {
	switch value := obj[key].(type) {
	case string:
		return value
	case json.Number:
		return value.String()
	default:
		return ""
	}
}

// playAIChatSSESuccess 是旧 TS `isAiChatSuccess`（`ai-tasks.ts:46-51`）的逐行对照实现：
//
//	if (!response) return false;
//	if (typeof response === "string") return response.trim().length > 0;
//	if (response.success || response.code === "0000" || response.code === 0) return true;
//
// 三处必须留心的语义：
//  1. `!response` 是 JS 的 falsy ⇒ `null`、`false`、`0`、`""`、缺席都算"没有响应"。
//     Go 侧要照这句抄，否则 `code: 0` 之外的一格 `null` 会走进下面那条而**判成功**
//     （`null` 在 JS 里被第一条拦住，在 Go 里如果只判 `value == nil` 就漏掉 `false`/`0`/`""`）。
//  2. 字符串那一支在**前两档**都可能命中：`data:"你好"` 解出来是 JSON 字符串，
//     第三档整个原文就是字符串。两支都是"trim 后非空即成功"。
//  3. `code === "0000"` 与 `code === 0` 是**严格相等**：JSON 字符串 `"0"` 两个都不等
//     （JS 里 `"0" === 0` 为 **false**，不做隐式转换）。
//     这一条靠 [json.Number] 与 Go 的类型区分**天然成立**，所以这里不许有人
//     "顺手"把 `code` 归一成数字再比 —— 那会把 `"0"` 也判成成功。
//
// `success` 走 JS 真值口径（见 [playAIChatSSETruthy]），不是 Go 的 `.(bool)`：
// 差别在 `success: 1` / `success: "true"` 这类形态上（**零样本**，只登记不解释）。
func playAIChatSSESuccess(value any) bool {
	switch typed := value.(type) {
	case nil:
		return false
	case bool:
		// `true` 单独一个布尔：JS 里 `response.success` 会去 `true.success`（= undefined）⇒ 假。
		return false
	case string:
		return strings.TrimSpace(typed) != ""
	// 数字**只可能是 [json.Number]**：本层不复刻 JS 的隐式转换（`"0" === 0` 为假那条）。
	// 数字形态的 `response.success` 是 undefined、数字本身没有 `code` 格 ⇒ 一律假。
	// `[]any` 不写分支也落到末尾的 false：JS 里 `[].success` 与 `[].code` 都是 undefined ⇒ 同假。
	case json.Number:
		return false
	case map[string]any:
		if playAIChatSSETruthy(typed["success"]) {
			return true
		}
		switch code := typed["code"].(type) {
		case string:
			return code == "0000"
		case json.Number:
			// JSON 里的 `0` 与 `0.0` 在 JS 里 `=== 0` 都为真；[json.Number] 的文本不同。
			// 用 `Int64()`/`Float64()` 判而不是比串，是为了让 `0.0` 也落在"数字 0"这一档
			// —— 而 `"0"`（字符串）**不会**走到这里（上一支已经返回）。
			if n, err := code.Float64(); err == nil {
				return n == 0
			}
			return false
		}
	}
	return false
}

// playAIChatSSETruthy 是 JS 的 truthiness（只覆盖 JSON 能解出来的那些形态）：
// 除 `false`、`null`、`0`（含 `-0` 与 `0.0`）、`""` 之外都算真。
//
// ⚠ 空数组 `[]` 与空对象 `{}` 在 JS 里是**真** —— 这里刻意跟着，不"修正"。
// 为什么不用 Go 的 `ok && v == true`：那会在 `success: 1` 这一档给出相反答案，
// 而这一档**零样本** ⇒ 按参考实现抄（§1.9），差别登记在此。
func playAIChatSSETruthy(value any) bool {
	switch typed := value.(type) {
	case nil:
		return false
	case bool:
		return typed
	case string:
		return typed != ""
	case json.Number:
		n, err := typed.Float64()
		return err == nil && n != 0
	case map[string]any:
		return true
	case []any:
		return true
	default:
		return true
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIChatCamera 发 585 那一发 chat 并给出 SSE 读数。**写**：一次调用问一句。
//
// # 为什么它不复用 [Upstream.Call] 而直接打 `u.http.Send`
//
// [Call] 一定走 [ParseEnvelope]（那是给 JSON 信封用的），而这一条回的是 SSE；
// [Upstream.Raw] 则把非 2xx 一律折成一句 `HTTP <status>`，**响应体丢掉**。
// 坑表 #165 记的就是这一格：`internal/io` 的 `Send` 在非 2xx 时**同时**交回响应与错误，
// 而"这一发到底被谁拒、回的是什么体"**只有体答得出**。本层的诊断面按那条的处方写：
// 先读体、再判错误（两件事不写成 `else`），并把状态与截断后的体一起放进错误文本。
//
// ⚠ 错误文本里带响应体，是本仓少见的处置，边界要划清：
// 这一族的响应体是**模型输出的那一句**，不含凭据（凭据全在请求头），
// 所以截断后放进错误串是可接受的；换成任何**请求**侧的值（`Authorization`、票、`tid`）
// 仍然一律不进 —— 见 `AGENTS.md` 的凭据边界与 `docs/sanitize-guide.md`。
func (u *Upstream) PlayAIChatCamera(
	ctx context.Context, profile *AppProfile, timestamp int64, authorization, tid string,
	params PlayAIChatParams,
) (PlayAIChatSSERead, *core.Error) {
	request, perr := PlayAIChatCameraRequest(profile, timestamp, authorization, tid, params)
	if perr != nil {
		return PlayAIChatSSERead{}, perr
	}
	response, err := u.http.Send(ctx, request)
	// ⚠ 顺序（#165 的正面兑现）：先问"**有没有一个可用响应**"，再问"有没有错误"。
	// [internal/io] 的 `Send` 在非 2xx 时**同时**交回响应与错误
	// （`return response, perr`），所以"`err != nil` 就说明什么都没拿到"是**错的**：
	// 那样写会把状态码与体一起丢掉，而"这一发是被谁拒的"只有体答得出。
	if response.Status == 0 {
		// 连状态码都没有 ⇒ 这一档**才是**"没有响应"：只报错误类别，不凭空造摘录。
		if err != nil {
			return PlayAIChatSSERead{}, asCoreError(err, "发送 chat/v2/add 失败")
		}
		return PlayAIChatSSERead{}, core.NewParse("chat/v2/add 没有拿到响应（状态码为空）")
	}
	// ⚠ 顺序：**先取体**，再判状态码（#165）。体不是 UTF-8 时不去猜它是什么。
	text, textErr := response.Text()
	if !response.IsSuccess() {
		return PlayAIChatSSERead{}, core.NewBadRequest(
			fmt.Sprintf("chat/v2/add 上游回 HTTP %d%s", response.Status, playAIChatSSEExcerpt(text)))
	}
	if textErr != nil {
		return PlayAIChatSSERead{}, core.NewParse("chat/v2/add 的响应体不是合法 UTF-8，无法按文本读 SSE")
	}
	if err != nil {
		// 2xx 还带错误：本仓的 `Send` 不会走到这里（非 2xx 才造错误），
		// 这一支只是把"以体为准"这句话写死：**有体、且是 2xx ⇒ 读体，不把 err 提成失败**。
		// 未被断言（自定义客户端才演得出来，见 `…Non2xxKeepsTheBodyExcerpt` 那一档）。
		return ParsePlayAIChatSSE(text), nil
	}
	return ParsePlayAIChatSSE(text), nil
}

// playAIChatSSEExcerpt 把响应体摊成一句可进错误文本的截断摘录。
//
// 三条裁剪规则都在这里，因为它们是**这一族的**规矩（别处要截断请自己写一条）：
//   - 换行与制表换成空格：多行体会把日志行打断（本仓事件流一行一条）；
//   - 只取前 [playAIChatSSEExcerptRunes] 个**码点**（按字节截会把中文切成半个）；
//   - 空体写成"（响应体为空）"，与"我们没读到"区分开。
func playAIChatSSEExcerpt(text string) string {
	if strings.TrimSpace(text) == "" {
		return "（响应体为空）"
	}
	flat := strings.Map(func(r rune) rune {
		if r == '\n' || r == '\r' || r == '\t' {
			return ' '
		}
		return r
	}, text)
	runes := []rune(flat)
	if len(runes) > playAIChatSSEExcerptRunes {
		return fmt.Sprintf("；响应摘录：%q…（截断，共 %d 码点）",
			string(runes[:playAIChatSSEExcerptRunes]), len(runes))
	}
	return fmt.Sprintf("；响应摘录：%q", flat)
}

// playAIChatSSEExcerptRunes 是摘录长度上限。
//
// 为什么是 300：旧 TS 在非 2xx 那一支自己截的是 `text.slice(0, 300)`
// （`apis/ai-workbench.ts:278`，见 1028 那一族），本层没有更好的一档可取，
// ⚠ 但**出处不同** ⇒ 它是我们对"日志里该念多少体"的选择，不是上游的成文。
const playAIChatSSEExcerptRunes = 300
