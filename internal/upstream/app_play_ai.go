// 拍拍系列（旧 TS 能力 `play-ai-special`，功能盘点表 #16）—— **上游协议层**。
//
// # 本文件只管五件事
//
//	① GET  …/ycloud/newyear/user/info        活动侧状态（只读）
//	② GET  …/ycloud/newyear/task/list        任务表（只读）
//	③ POST …/ycloud/newyear/task/step/click  上报一步任务（**写**）
//	④ POST …/ycloud/newyear/blindbox/lottery 抽一次盲盒（**写**）
//	⑤ POST https://ai.yun.139.com/api/image/aiRecognize 图片识别（写云盘 + 出识别结果）
//
// **不做** app 层编排、不登记能力、不接线、不碰前端：该不该发 ③④、发几次，
// 由调用方按 ①② 的读数决定（与 `app_funai.go` 同一分工口径）。
//
// # 证据只有一份，而它**缺了链路的前半段**（不是"与文档矛盾"）
//
// `test/fixtures/http/夏日拍拍大作战.json` —— 13 条，全部 200，含真实响应体
// （归档时间戳 2026-06-29 12:10–12:12，是**一次连续的操作窗口**）。
// 盘点文档（`docs/business-feature-inventory.md` §2.4 `play-ai-special` 那一条）写的是
// 「活动站会话（`querySpecToken` → `tyrzLogin(marketName shake_shake, sourceId 1314)`）
// + 专用 HTTP 客户端」。
//
// ⚠ 2026-09-24 复核旧 TS 后订正：**文档这一条不是"另一套"，它与旧实现一致**
// （`packages/core/src/tasks/play-ai-special/task.ts:50-70` 就是那两步），
// 缺的只是**归档没拍到换票那一段**。⇒ 本层按夹具实现 ①…⑤ 这件事不变，
// 但"为什么没接活动站会话"的理由从"文档无出处所以不信"换成下面那段**有出处、缺实拍**。
//
// ⇒ **按夹具实现**（本项目冲突口径）。理由要说清：
//   - 夹具里 ①②③④ 四条带的鉴权头**就是** `jwttoken`（与签到族同一机制），
//     头名逐条核对过；
//   - 整份夹具里**没有** `querySpecToken` / `tyrzLogin` 的任何样本，
//     也没有 `marketName: shake_shake`、没有 `sourceId: 1314` ——
//     凭零样本凭空造一条建会话链路，就是拿零样本去猜新接口（本项目明令禁止的那类）。
//   - ⚠ 于是这一页的 `jwttoken` **使用公共来源**（仓库现成的建会话入口，
//     接线点在 app 层）。按 AGENTS.md §1.12，这是**临时态不是终判**：
//     后续抓到本活动页的特有取法 ⇒ 直接改用特有取法并**改写这一句**，
//     不留"注释说公共、代码走特有"，也不叠"补记"。
//
// # 特有取法：**有出处，但没有实拍**（2026-09-24 复核旧 TS 后订正本文件头）
//
// 旧项目 `packages/core/src/tasks/play-ai-special/task.ts:50-70` 里这一页**确实**有自己的一条换票链，
// 而且本仓库**已经**有同一条 `tyrzLogin` 端点（[TyrzLoginRequest] / [TyrzLoginBody]）：
//
//	① GET  …/ycloud/api/cloud/userdomain/v2/querySpecToken?targetSourceId=001005&hideLoading=true
//	   （带四个 H5 签名头；⚠ 与本仓已实现的**POST** `querySpecTokenV2` 是**两个不同端点**）
//	② POST …/ycloud/auth-service/auth/tyrzLogin
//	   body `{token, openAccount:false, marketName:"shake_shake", sourceId:"1314",
//	         type:"signupPlatform", channel:null}` + 头 `type: signupPlatform`
//	   ⇒ 响应的 `result.token` 才是这一页的 `jwttoken`
//
// 与签到那一份的差别全在**参数与头**上（`openAccount` 旧 TS 是 `false` 而这里是数字 `0`；
// 多 `type` / `channel` 两键；多一个 `type` 头）—— 正是 §1.12 事实②说的"URL 相同而参数不同"。
//
// ⚠ **为什么仍然走公共来源**：这两步在本仓**一条实拍都没有**（夹具里没有换票那一步）。
// ⇒ 本层（①…⑤）**继续收调用方注入的 jwt**，接线时用**公共来源**（`OpenSignInSession` 那一份票）——
// 这是 §1.12 的临时态，不是终判。
//
// **但"实现还没有"这一步已经不成立了**（2026-09-24 就地订正本段）：上面那两步已在
// [`app_play_ai_session.go`](app_play_ai_session.go) 里按旧 TS 落成
// [PlayAIQuerySpecTokenRequest] + [PlayAITyrzLoginRequest]（→ [PlayAISession]），
// 请求形状逐字钉住、32 条测试。**剩下的不是"要不要写"，而是"真打之前要不要改用它"** ——
// 而那句要一次实拍才能定论（特有链认不认、`sourceId 1314` 那一步回什么），
// 已记进 `docs/pending-decisions.md` 的 **P5**。**缺口**照实保留：
// 这一页特有取法的**实拍**仍未拍到（活动窗外的页补不了，见 `docs/capture/app-play-ai-samples.txt` §5）。
//
// # 同一份归档里**没有实现**的几类（登记，不是漏掉）
//
//   - `ycloud/user-service/mobile/query`、`ycloud/api/activity/{labels,collect/list,view}`
//     （活动门户的通用请求，不属于本能力要回答的问题）、
//     `open.yun.139.com/miniapp/api/v1/mop/runtime/data-report/config`（前端埋点 SDK 配置）：
//     与 ①…⑤ 同窗口出现，但**不在本族** ⇒ 不实现。
//   - `personal-kd-njs.yun.139.com/hcy/file/get`：归属"上传/清理"那一族（主线处理）。
//   - `chat/v2/add`（**SSE**）与 `visitlog/journaling`（埋点）：夹具里**一条样本都没有**。
//     ⚠ 但"要不要发它们"**不是没有出处**：旧 TS `tasks/play-ai-special/task.ts` 的
//     `aiCameraHandler` 把它们当**门槛**用 —— `:89-99` 与 `:115-125` 两处
//     `if (!journalOk) return false`，也就是**上报失败 ⇒ 整件任务判失败**（连 `step/click` 都不发）。
//     ⇒ 于是"不接埋点"在这条链上不是"少发两条无关请求"，而是**旧实现认为会掉整件任务**。
//     ⚠ 反过来"上游到底认不认这些埋点"**仍然零实拍**（旧实现的门槛是**它自己**的判断，
//     不是上游告诉我们缺了埋点就不给记账）⇒ 两句话都得留着，别合成一句。
//     请求形状已按旧 TS 落在 `app_play_ai_session.go`（**只构造请求**：SSE 的响应形状零样本 ⇒
//     那边刻意**没有**解析器）；接不接进编排是**待拍板 P6**。
//
// # ⚠ 这一族的请求头**比仓库其余 H5 接口少三个**（实拍裁出来的）
//
// 四条 `newyear/*` 带的头是：`accept`、`sec-fetch-site`、`jwttoken`、`showloading`、
// `accept-language`、`accept-encoding`、`sec-fetch-mode`、`user-agent`、`referer`、
// `sec-fetch-dest`、`cookie`（两条 POST 另有 `content-type` 与 `origin`）。
//
// **没有** `appversion`、**没有** `activityid`、**没有** `deviceid`。
//
// 这不是脱敏抹掉的：同一份归档里 `user-service/mobile/query` 与
// `api/activity/{labels,collect/list,view}` 四条**都带** `appversion`、`activityid`、
// `deviceid`（夹具顺序 0/1/2/3，逐条数过），说明归档器保留这些头名。⇒ 是这一族真的没发。
//
// 所以这里**不复用** [WithH5Headers]：它会多加 `activityid` 与 `appversion` 两个头
// （而它**也不发** `deviceid` —— 那一个是 [FunaiModuleRequest] 一类调用点自己补的），
// 所以这里用 [playAIH5Headers] 逐字按夹具发。代价：这一族的头集合单独维护一处。
// 反过来"顺手复用"的表现是发出一条**夹具里不存在的请求**（多那两个头、
// 并且两条 POST 会少一个 `origin`），而上游对多余头的反应**没有样本**。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（逐条来自夹具的字面量，不是猜的）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// PlayAIMarketName 是这一期的市场名。实拍三处都是它：两条 GET 的 `marketName`
	// 查询参数、两条 POST 的 body、以及 `button.link` 里那个活动落地页。
	//
	// ⚠ 它是**期次**的一部分：春日/夏日/秋日拍拍是同一实现的不同期次
	// （盘点文档 §2.4 注），换一期它就换。写死在这里是为了让"换期"这件事
	// **必须**改代码 + 过一次测试，而不是在运行时静默带上过期市场名。
	PlayAIMarketName = "National_PlayAISpecial"

	// PlayAISource 是这一族 body / 查询参数里的 `source`，实拍恒 `app`。
	PlayAISource = "app"

	// PlayAIStepKeyClick 是 `task/step/click` body 里的 `key`。
	//
	// ⚠ 实拍**只有 `click` 这一档**（它与该任务 `button.app.ext` 的值同字）。
	// 其它 step key 有没有、长什么样 ⇒ 零样本。要接别的 step 就先补一条抓包，
	// 不要照 path 上的 `step` 这个词去编一个 key：这一族的 path 写着
	// `/task/step/<something>`，而 `<something>` 与 `key` 是否必须同字**也未验证**。
	PlayAIStepKeyClick = "click"

	// PlayAIHostAI 是 AI 服务的主机（⑤）。
	//
	// ⚠ [envelope.go](envelope.go) 的主机表里**没有**这一项（`ai.yun.139.com`
	// 在全仓 grep 不到第二个出处），而本任务不许改那个文件 ⇒ 先落在本文件。
	// 要收口时由主线把它搬进公共表。
	//
	// ①…④ 那四条**不另立常量**：它们的主机就是 [BaseMcloud]。同一主机取两个名字
	// 会诱导将来有人"顺手统一"成另一个端的地址 —— 而这一族的地址错了不报错，
	// 只是打到别的服务上。
	PlayAIHostAI = "https://ai.yun.139.com"

	// PlayAIRecognizeChannelID 是 ⑤ body 的 `channelId`（实拍 `"101"`，**字符串**）。
	PlayAIRecognizeChannelID = "101"
	// PlayAIRecognizeType 是 ⑤ body 的 `recognizeType`（实拍 `"1"`，**字符串**）。
	PlayAIRecognizeType = "1"
	// PlayAIRecognizeSendType 是 ⑤ body 的 `sendType`（实拍 `"2"`，**字符串**）。
	//
	// ⚠ 三个字段的"为什么是这个值"**没有成文**（夹具只给了值，没给语义）。
	// 语义未验证 ⇒ 一律做成常量而不是参数：给调用方一个自由填的口子，
	// 表现是发出一条没人对拍过的请求。
	PlayAIRecognizeSendType = "2"
	// PlayAIRecognizeTimeoutMS 是 ⑤ body 的 `timeout`（实拍 `30000`，**数字**）。
	//
	// 它是 body 里唯一的数字字段 ⇒ 做成 `int` 让"顺手写成字符串"在测试里就红
	// （那条断言见 `app_play_ai_test.go`）。
	PlayAIRecognizeTimeoutMS = 30000
	// PlayAIRecognizeUploadToCloud 是 ⑤ body 的 `uploadToCloud`。
	//
	// ⚠ 实拍**只有 `true` 一档**：`false` 时上游回什么（还回不回 `fileId`？）零样本
	// ⇒ 不参数化。要"识别但不落云盘"这一档，先补抓包。
	PlayAIRecognizeUploadToCloud = true

	// PlayAILocalOrigin 是原生 WebView 侧发出的 `Origin`（⑤ 实拍逐字）。
	//
	// 它与 ①…④ 用的 `origin: https://m.mcloud.139.com` 是**两个不同的值**：
	// 前者是 App 原生侧发起的请求，后者是同站 H5。混用的后果**未测试**，
	// 但它已经不是夹具里那一条了。
	PlayAILocalOrigin = "mcloudlocal://yun.139.com"

	// PlayAIPlainJSONContentType 是 ⑤ 的 `Content-Type`：`application/json`，
	// **不带 charset**。
	//
	// ⚠ 与 [H5ContentType]（带 `;charset=UTF-8`）**不是**同一个值：
	// ①…④ 实拍带 charset，⑤ 不带。所以 ⑤ 这条要**先剥后补**（见 [dropHeader] 那条
	// "两个同名头会被上游按第一个解析"的说明）。
	PlayAIPlainJSONContentType = "application/json"

	// PlayAIRecognizeCodeOK 是 ⑤ 的成功码：**字符串** `"0000"`（不是数字 0）。
	PlayAIRecognizeCodeOK = "0000"

	// PlayAIShowLoading 是 ①…④ 那个 `showloading` 头**在我们这一侧**的取值。
	//
	// ⚠ 它**不是**抄回来的值：夹具里这一位已被脱敏
	// （[`docs/capture/app-play-ai-samples.txt`](../../docs/capture/app-play-ai-samples.txt) §1
	// 与 §6 各记了一次），能确定的只有"这一族四条都带这个头"。
	// 取值取的是仓库里其余 H5 读接口在调用点上一直钉的那一个（`"true"`，
	// 见 [WithH5Headers] 的各调用点），理由是"别为这一族单独发明一个"。
	// ⚠ "上游认不认别的值""它是否参与判据"**未测试** —— 签到族那句"纯 UI 提示"
	// 搬不过来就是断言（§1.7）。做成常量而不是参数：要试别的取值时改这一处，
	// 而不是让每个调用点各写一个字面量。
	PlayAIShowLoading = "true"
)

// 任务表 `state` / `limitType` 的观测值**不在这里重复声明**：
// 实拍这一族出现的是 `WAIT` / `FINISH` 与 `DAY` / `EVER`，
// 与签到任务表那族（[TaskStateWait]、[TaskLimitDay] 等）字面相同。
//
// ⚠ 字面相同**不等于**同一份枚举：两族的取值集各自只由各自的样本支撑，
// 所以解析结果里存的是 `*string` 原文，由调用方按需要对照那些常量 ——
// 不在此处造一个"跨族枚举"。

// ─────────────────────────────────────────────────────────────────────────────
// 请求头与查询串
// ─────────────────────────────────────────────────────────────────────────────

// playAIH5Headers 给 ①…④ 补头，逐字按夹具。
//
// 与 [WithH5Headers] 的**三处**差别（前两处是"夹具说不发"，第三处是"夹具说要发"）：
//  1. 不发 `activityid`（[WithH5Headers] 会钉成 [MarketName] `sign_in_3`）；
//  2. 不发 `appversion`（`deviceid` 它本来也不发，这里是按夹具再确认一次不发）；
//  3. `Origin` **只给带 body 的那两条**（实拍 GET 没有 `origin`、POST 有 `https://m.mcloud.139.com`）。
//
// `jwt` 是**必填**而不是 `*string`：这四条在夹具里条条带 `jwttoken`，
// 不带票去发是另一种请求（而且上游回什么没有样本）。让"票从哪来"这件事
// 在函数签名上就无法省略 —— 这是"特有取法仍未拍到"这一缺口唯一能做的护栏。
//
// `showLoading` 的值在夹具里**已被脱敏** ⇒ 由调用方给。它是否参与判据 **未测试**
// （在签到族它是纯 UI 提示；把那句结论搬过来就是断言）。
func playAIH5Headers(request core.HTTPRequest, referer, jwt, showLoading string) core.HTTPRequest {
	request.Headers = dropHeader(request.Headers, "content-type")
	if len(request.Body) > 0 {
		request = request.
			WithHeader("Content-Type", H5ContentType).
			WithHeader("Origin", BaseMcloud)
	}
	return request.
		WithHeader("Accept", "*/*").
		WithHeader("Referer", referer).
		WithHeader("showloading", showLoading).
		WithHeader("jwttoken", jwt)
}

// playAIQuery 按**实拍里键的出现顺序**拼查询串。
//
// 为什么不让 `url.Values.Encode()` 来排：它按**字典序**，会把 `marketName&source`
// 排成同一条（这里恰好一致），但把"参数顺序"这件事交给库 ⇒ 哪天新增一个键，
// 顺序会**悄悄变**。实拍两条 GET 的串很短，逐字钉住更省心。
// 值仍走 [PercentEncode]（当前两个值都是纯 ASCII，编码后逐字不变）。
func playAIQuery(path string, params ...[2]string) string {
	if len(params) == 0 {
		return path
	}
	var out strings.Builder
	out.WriteString(path)
	for i, p := range params {
		if i == 0 {
			out.WriteByte('?')
		} else {
			out.WriteByte('&')
		}
		out.WriteString(p[0])
		out.WriteByte('=')
		out.WriteString(PercentEncode(p[1]))
	}
	return out.String()
}

// ─────────────────────────────────────────────────────────────────────────────
// 信封：**两族不同构，各自判**（见 docs/capture/app-api-families.txt §4）
//
// ⚠ 不抽公共泛型解析器，也不改 [Envelope] 的公共判据：
//   - `newyear/*`：`{"code":<数字 0>,"msg":<字符串>,"result":…}`
//   - `aiRecognize`：`{"code":"0000","message":"请求成功","data":{…},"success":true}`
//
// 共用一个判据（例如 [Envelope.EnsureOK]）会把两族**都**判成成功 ——
// 因为它的 `code` 走 [asInt64]，而那里**数字与字符串都收**：
// `{"code":0,…}`（H5 那族）与 `{"code":"0000",…}`（AI 那族）都能过，
// 于是"把 AI 的响应喂给 newyear 的解析器"是一条静默通路。
// 两条判据分别只认自己那一族的 `code` **类型**，就是为了在这种情况下立刻响。
//
// # 非零码怎么处理
//
// 两族各自**只有一条成功样本**（`code=0` / `code="0000"`），**非零码一个样本都没有**
// ⇒ 这里不枚举错误码、也不写"某码意味着 X"，只把非零码 + 上游 `msg`/`message`
// **原文**透成一条错误交回调用方（`core.Error` 的 `Message` 只进日志，
// 对外文案走白名单，见 [errorFromStatus] 那条"不带响应体原文"的理由）。
// 要补错误码语义：抓到一条非零响应，把码与原文一起登记，再回来加分支。
// ─────────────────────────────────────────────────────────────────────────────

// playAINewyearPayload 判 ①…④ 的信封，给出 `result` 原值与"它在不在"。
//
// `result` 允许缺席：实拍 ③（`task/step/click`）成功响应就是
// `{"code":0,"msg":"success"}` —— **连 result 这个键都没有**。
// 把"没有 result"一律判成解析错误的表现是：一次真成功的上报被读成失败，
// 而它已经是**写**操作（重跑等于再上报一次）。
func playAINewyearPayload(envelope *Envelope) (any, bool, *core.Error) {
	root, ok := envelope.Raw().(map[string]any)
	if !ok {
		return nil, false, core.NewParse("拍拍活动响应不是 JSON 对象")
	}
	rawCode, hasCode := root["code"]
	if !hasCode {
		return nil, false, core.NewParse("拍拍活动响应没有 code 字段（这一族必带）")
	}
	// 只认**数字字面量**：这一族实拍是数字 0。哪天变成字符串，
	// 说明手里这份**不是**这一族的响应（AI 那族就是字符串 "0000"）⇒ 立刻响。
	number, ok := rawCode.(json.Number)
	if !ok {
		return nil, false, core.NewParse(
			fmt.Sprintf("拍拍活动的 code 不是数字字面量，实得 %T（两族信封不同构，别混解）", rawCode))
	}
	code, convErr := number.Int64()
	if convErr != nil {
		return nil, false, core.NewParse("拍拍活动的 code 解不成整数: " + number.String())
	}
	if code != CodeOK {
		prefix := fmt.Sprintf("拍拍活动上游返回 code=%d", code)
		if code == CodeNotLoggedIn {
			// ⚠ **唯一**一个在这里被单独立起来的非零码，理由不是"这一族抓到了这一码"
			// （**没有**：newyear 那六条实拍全是 `code=0`），而是它是这一族**唯一**
			// 一个"换个动作就能自救"的形态：`90001` 在签到族已实测 = **jwttoken 没了或过期**
			// （`docs/capture/app-signin-samples.txt`），而 app 层那条自救支路
			// （[core.Error.IsSessionExpired] → 刷一次票、重放一次）只认这个变体。
			// 判成 BadRequest 的表现是**自救代码永远不触发**：票池里那张过期的票会一直被打，
			// 这一条能力从此每趟都失败，而界面上只看得到"上游回 90001"。
			//
			// ⚠ 借来的判据要标清边界：这一族**撞 90001 时刷了票再重放能不能过 ⇒ 未测试**。
			// 怎么补这一格：真跑一次这一条只读链，中途把票放过期（或手工把池里那条改脏），
			// 看它是"刷一次就成"还是"重放仍 90001"。
			hint, known := Diagnose(code)
			base := playAIBusinessError(prefix, root["msg"])
			if known {
				return nil, false, core.NewSessionExpired(base.Message() + "（" + hint + "）")
			}
			return nil, false, core.NewSessionExpired(base.Message())
		}
		return nil, false, playAIBusinessError(prefix, root["msg"])
	}
	result, hasResult := root["result"]
	return result, hasResult, nil
}

// playAIRecognizePayload 判 ⑤ 的信封并给出 `data` 对象。
//
// 两道判据都收：`code` 必须是字符串 `"0000"`，且 `success` 必须是 `true`。
// ⚠ 为什么两个都判而不多余：唯一样本里两者**同时**成立，
// 所以"只 code 成立 / 只 success 成立"这两种响应的含义**都没有样本** ——
// 任一条不成立就交回错误，是把"看不懂的那一档"留给调用方而不是猜。
func playAIRecognizePayload(envelope *Envelope) (map[string]any, *core.Error) {
	root, ok := envelope.Raw().(map[string]any)
	if !ok {
		return nil, core.NewParse("aiRecognize 响应不是 JSON 对象")
	}
	code, ok := root["code"].(string)
	if !ok {
		return nil, core.NewParse(
			fmt.Sprintf("aiRecognize 的 code 不是字符串，实得 %T（这一族是字符串 %q）", root["code"], PlayAIRecognizeCodeOK))
	}
	success, hasSuccess := root["success"].(bool)
	if code != PlayAIRecognizeCodeOK || !hasSuccess || !success {
		return nil, playAIBusinessError(
			fmt.Sprintf("aiRecognize 上游返回 code=%q success=%v", code, success), root["message"])
	}
	data, ok := root["data"].(map[string]any)
	if !ok {
		// 这一族本层要读的东西**全在 data 里**，缺了就是"什么都读不到"，
		// 与 ③ 那种"本来就没有 result"不同 ⇒ 这里判失败。
		return nil, core.NewParse("aiRecognize 响应没有对象形态的 data")
	}
	return data, nil
}

// playAIBusinessError 把"非成功码 + 上游那句原文"拼成一条错误。
//
// 原文**只在 msg/message 是字符串时**才带上：夹具里两族这句话都是字符串，
// 而万一上游哪天改成对象或干脆不给，我们要的是"码 + 一句人话"，
// 不是 `map[string]any` 的 Go 打印形态。
func playAIBusinessError(prefix string, upstreamMsg any) *core.Error {
	if text, ok := upstreamMsg.(string); ok && text != "" {
		return core.NewBadRequest(fmt.Sprintf("%s: %s", prefix, text))
	}
	return core.NewBadRequest(prefix)
}

// ─────────────────────────────────────────────────────────────────────────────
// ① 用户活动状态：GET /ycloud/newyear/user/info
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIUserStatusRequest 构造 `GET …/ycloud/newyear/user/info?marketName=<市场名>`。
//
// ⚠ 这一条**只带 `marketName`**、**不带 `source`** —— 与 ② 差一个键。
// 实拍两次调用这一条的 URL 逐字相同（同一个查询串），所以"少带/多带一个键"
// 是**看得见的**偏离，测试按字节钉住。
func PlayAIUserStatusRequest(jwt, referer, showLoading string) core.HTTPRequest {
	return playAIH5Headers(
		core.NewRequestGet(playAIQuery(
			fmt.Sprintf("%s/ycloud/newyear/user/info", BaseMcloud),
			[2]string{"marketName", PlayAIMarketName},
		)),
		referer, jwt, showLoading)
}

// PlayAIUserStatus 是 ① 里本层要带出去的那一格。
//
// # 为什么只有这一格
//
// 实拍 `result` 有 `showData{prizeBg, prizeImages[6], titleImage}` 与
// `totalLotteryCount` 两项。`showData` 全是**奖池图片素材**（渲染用），
// 不参与任何判据 ⇒ 不声明（声明出来只会多一处会漂移的字段表）。
type PlayAIUserStatus struct {
	// TotalLotteryCount 是 `result.totalLotteryCount`。
	//
	// 实拍两条：`0`（做完任务之前）→ 做完任务并复查时 `1`。
	// ⚠ **语义未验证**："累计已获得几次"还是"当前还剩几次"两说 ——
	// 这一窗口里 `1` 那条**出现在抽奖之前**，所以它至少含"还没花掉"的意思，
	// 但一次观测说不出会不会被花掉后减回去。
	// ⇒ **只存不判**：该不该抽由调用方定，且定之前要先补"抽一次之后再看这一格"
	// 的样本（只能在活动窗口内做，窗外补不了）。
	//
	// 指针：`0` 是**合法取值**（实拍就有），必须与"上游没给"分得开。
	TotalLotteryCount *int64
}

// PlayAIParseUserStatus 解析 ① 的响应。
func PlayAIParseUserStatus(envelope *Envelope) (PlayAIUserStatus, *core.Error) {
	result, hasResult, perr := playAINewyearPayload(envelope)
	if perr != nil {
		return PlayAIUserStatus{}, perr
	}
	if !hasResult {
		return PlayAIUserStatus{}, core.NewParse("user/info 响应没有 result（实拍两条都有）")
	}
	obj, ok := result.(map[string]any)
	if !ok {
		return PlayAIUserStatus{}, core.NewParse("user/info 的 result 不是对象")
	}
	return PlayAIUserStatus{TotalLotteryCount: playAIOptInt(obj, "totalLotteryCount")}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 任务表：GET /ycloud/newyear/task/list
// ─────────────────────────────────────────────────────────────────────────────

// PlayAITaskListRequest 构造 `GET …/ycloud/newyear/task/list?marketName=…&source=app`。
//
// 实拍这一条出现**两次**，两次的 method / URL / 查询串**逐字相同**（差别只在响应：
// 中间夹了一次 ③ 上报，于是 `id:1` 那条从 `WAIT` 变 `FINISH`）。
// ⇒ "同一端点两次"不是两种参数，别据此造两个变体。
func PlayAITaskListRequest(jwt, referer, showLoading string) core.HTTPRequest {
	return playAIH5Headers(
		core.NewRequestGet(playAIQuery(
			fmt.Sprintf("%s/ycloud/newyear/task/list", BaseMcloud),
			[2]string{"marketName", PlayAIMarketName},
			[2]string{"source", PlayAISource},
		)),
		referer, jwt, showLoading)
}

// PlayAIUserTask 是任务表里的一条。
//
// # 为什么只声明这些
//
// 实拍一条有 21 个键（夹具顺序 5，逐条数过）：另有 `button`（七份端形态
// `app/harmony/leadeon/other/wechat/email/mini`，其中 `email` / `mini` 实拍是 `null`，
// 里面是 `link` / `btnBg` / `itemBg` / `popBtn` / `text` / `tips`）、`showData`（CSS 串）、
// `icon` / `img`（素材）、`rule`（一段 HTML 规则文案）、`hasBak` / `hasNda`、
// `sort` / `source`、`currDay` / `updateTime`。这些是**渲染与投放**用的：本层要回答的是
// "有哪些任务、做没做、做一步要回传哪个 id"，所以只取身份 + 状态 + 计数。
//
// ⚠ `button.*.link` 是 `mcloud://main/aiCamera?<那个值归档时已被脱敏>` 与小红书链接（含 token 参数）：
// 那是**跳转与埋点凭据**，既不声明也不进本文件（脱敏位**不可当样本**，也不许抄进仓库）。
type PlayAIUserTask struct {
	// TaskID 是 `id` —— ③ 的 body 要回传它（实拍 `taskId: 1` 对应 `id: 1` 那条）。
	TaskID *int64
	// Name 是 `name`（列表主文案，实拍「体验拍照问AI」「拍夏日关键词」「去小红书点赞」）。
	Name *string
	// Flag 是 `flag` —— 实拍的短标签（「拍照问AI」「拍夏日关键词」「小红书」）。
	//
	// 声明它而不是只用 `name`：③ 上报时"哪一步"这件事在实拍里靠 `key`+`taskId`
	// 表达，`flag` 是**唯一**能把任务与步对上眼的字面量（`button.*.ext` 就是 `click`，
	// 三条任务全都一样）。要不要用它做匹配由调用方定，本层只负责交出去。
	Flag *string
	// Description 是 `description`（实拍三条都是「抽奖次数+1」，即**奖励**那一格）。
	Description *string
	// GroupID 是 `groupId`。
	//
	// ⚠⚠ **驼峰** `groupId`：签到任务表那族是全小写 `groupid`（见 [PlayAIUserTask] 的
	// 对照 [TaskItem]）。这是两族的真实差异，写成同一个键会有一族**静默读不到**
	// （读到 nil，而 nil 是合法的"上游没给"）。
	GroupID *string
	// State 是 `state`（实拍 `WAIT` / `FINISH`）。不归一：读不出原样就交 nil。
	State *string
	// LimitType 是 `limitType`（实拍 `DAY` / `EVER`）。
	LimitType *string
	// RecordID 是 `recordid`。
	//
	// 实拍两种形态都出现过：`null`（没做过）与**字符串** `"20674897"`（做过）。
	// ⇒ 按字符串收（数字形态零样本，不猜）。nil = "上游给了 null 或没给"，
	// 与"给了空串"是两回事。
	RecordID *string
	// CurrStep 是 `currstep`（**全小写**，实拍 ② 两次之间 `id:1` 那条从 0 变 3）。
	CurrStep *int64
	// Process 是 `process`（实拍三条**全是 0**，包括已完成的两条）。
	//
	// ⚠ 所以"进度"这个口径在 `process` 上**说不通** —— 该读 `process` 还是
	// `currstep` 未验证（同 [TaskItem] 那条教训：已完成的条目 `process` 仍是 0）。
	// 两个都存，不加解释。
	Process *int64
}

// PlayAIParseTaskList 解析 ② 的响应，给出任务数组。
//
// `result` 必须是**数组**：上游改成对象时在这里立刻响，不"顺手兼容两种形状"
// （兼容会把形状变化藏起来，同 [ParseTaskList] 那条口径）。
func PlayAIParseTaskList(envelope *Envelope) ([]PlayAIUserTask, *core.Error) {
	result, hasResult, perr := playAINewyearPayload(envelope)
	if perr != nil {
		return nil, perr
	}
	if !hasResult {
		return nil, core.NewParse("task/list 响应没有 result（实拍两条都有）")
	}
	items, ok := result.([]any)
	if !ok {
		return nil, core.NewParse("task/list 的 result 不是数组")
	}
	out := make([]PlayAIUserTask, 0, len(items))
	for _, raw := range items {
		// 单条不是对象时**跳过**还是**报错**？这里报错：任务表是本能力的判据来源，
		// 一条读不出来的记录比"少一条"更危险（调用方会以为那一步没做）。
		item, ok := raw.(map[string]any)
		if !ok {
			return nil, core.NewParse("task/list 的 result 里有一条不是对象")
		}
		out = append(out, PlayAIUserTask{
			TaskID:      playAIOptInt(item, "id"),
			Name:        playAIOptStr(item, "name"),
			Flag:        playAIOptStr(item, "flag"),
			Description: playAIOptStr(item, "description"),
			GroupID:     playAIOptStr(item, "groupId"),
			State:       playAIOptStr(item, "state"),
			LimitType:   playAIOptStr(item, "limitType"),
			RecordID:    playAIOptStr(item, "recordid"),
			CurrStep:    playAIOptInt(item, "currstep"),
			Process:     playAIOptInt(item, "process"),
		})
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 上报一步任务：POST /ycloud/newyear/task/step/click（**写**）
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIStepClickBody 是 ③ 的请求体。键序**逐字照实拍**（实拍 body 已按字典序，
// 与 Go 的 map marshal 一致，所以两条都能钉住字节）。
func PlayAIStepClickBody(taskID int64) map[string]any {
	return map[string]any{
		"key":        PlayAIStepKeyClick,
		"marketName": PlayAIMarketName,
		"source":     PlayAISource,
		"taskId":     taskID,
	}
}

// PlayAIStepClickRequest 构造 `POST …/ycloud/newyear/task/step/click` —— **写请求**。
//
// 实拍 body 逐字（`content-length: 79`）：
//
//	{"key":"click","marketName":"National_PlayAISpecial","source":"app","taskId":1}
//
// ⚠ `taskId` 是**数字**（实拍无引号）。上游哪天改成字符串时 [PlayAIStepClickBody]
// 不会自动跟上，所以测试按字节钉这一条。
//
// 该不该发由调用方决定（本层不设门槛，与 `app_signin_send.go` 那一族同一分工）。
func PlayAIStepClickRequest(jwt, referer, showLoading string, taskID int64) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/newyear/task/step/click", BaseMcloud)).
		WithJSONBody(PlayAIStepClickBody(taskID))
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return playAIH5Headers(request, referer, jwt, showLoading), nil
}

// PlayAIParseStepClick 判 ③ 的成败。
//
// 实拍成功响应 `{"code":0,"msg":"success"}` —— **没有 result**，所以这一条
// 唯一能交回的就是"上游认了"这个结论（`nil`）。
//
// ⚠ 不去判"到底记上了没有"：那要回查 ②（实拍序列确实是这么做的 —— 上报之后
// 连着查了 ② 与 ①，看到 `state` 从 `WAIT` 变 `FINISH`）。回查是 app 层的事。
func PlayAIParseStepClick(envelope *Envelope) *core.Error {
	_, _, perr := playAINewyearPayload(envelope)
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 抽一次盲盒：POST /ycloud/newyear/blindbox/lottery（**写**）
// ─────────────────────────────────────────────────────────────────────────────

// PlayAILotteryBody 是 ④ 的请求体（实拍**两个键**，`content-length: 54`）。
func PlayAILotteryBody() map[string]any {
	return map[string]any{
		"marketName": PlayAIMarketName,
		"source":     PlayAISource,
	}
}

// PlayAILotteryRequest 构造 `POST …/ycloud/newyear/blindbox/lottery` —— **抽一次**。
//
// ⚠ 一次调用**只抽一次**，循环由调用方管（本层连"还剩几次"都不问 ——
// 那要读 ①，而 ① 的语义本身未验证）。
func PlayAILotteryRequest(jwt, referer, showLoading string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/newyear/blindbox/lottery", BaseMcloud)).
		WithJSONBody(PlayAILotteryBody())
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return playAIH5Headers(request, referer, jwt, showLoading), nil
}

// PlayAIPrize 是 `result.prize` —— 抽中的那一格。
//
// 字段按唯一样本逐字声明。**全是指针**：一条样本说不出"缺哪个就说明响应坏了"，
// 而这是**写**请求 —— 硬要求某个字段的表现是把一次真成功的抽奖报成解析错误，
// 调用方为了拿结论只能再抽一次（同 [FunaiLottery] 那条教训）。
type PlayAIPrize struct {
	// PrizeID 是 `prizeId`（实拍 `260416004`）。
	PrizeID *int64
	// PrizeName 是 `prizeName`（实拍「0.5元支付宝红包」）—— 这一格唯一**给人看**的结论。
	PrizeName *string
	// PrizeType 是 `prizeType`（实拍 `1002`）。
	//
	// ⚠ 这套类型号的取值表**没有成文**，本层不解释它（别拿它分"现金/云朵/实物"）。
	// 要判奖品种类先看 [PlayAIPrize.Type] 与 [PlayAIPrize.PrizeName]。
	PrizeType *int64
	// Type 是 `type`（实拍 `"common"`）。⚠ 取值集**零其它样本**，不解释。
	Type *string
	// Oid 是 `oid`（实拍**数字** `1248582079`），统一成字符串带出。
	//
	// 为什么收两种形态：领奖专区那条链接（`buttonLink`）与本仓奖品记录那一族
	// （见 [optIDField] 的说明）都把 oid 塞进 URL，而上游两个形态都出现过。
	Oid *string
	// ButtonLink 是 `buttonLink` —— 领奖落地页。
	//
	// 实拍它的查询串里带 `token=<那个值归档时已被脱敏>` ⇒ **一次性的凭据型链接**：
	// 原样带出仅供调用方用，不进日志、不进黄金向量（见仓库的脱敏纪律）。
	ButtonLink *string
	// ButtonTxt 是 `buttonTxt`（实拍「前往激活」）。
	ButtonTxt *string
	// Tips 是 `tips`（实拍那句「奖品已下发到你的账号中，请在本月内前往…激活使用，逾期失效」）。
	//
	// ⚠ 它是**上游文案**：原样存，不改写、不据此判"是否已下发"（判据是 `code`）。
	Tips *string
	// Title 是 `title`（实拍「恭喜你获得」）。
	Title *string
	// PrizeImg 是 `prizeImg`（素材 URL）。
	PrizeImg *string
	// FirstMonthGet 是 `firstMonthGet`（实拍 `false`）。⚠ 语义**零其它样本**，不解释。
	FirstMonthGet *bool
	// High 是 `high`（实拍 `false`）。同上：不解释。
	High *bool

	// ⚠ 唯一样本里**只有 null** 的三个键：`description` / `extInfo` / `poster`。
	//
	// 这里刻意**不**声明它们，理由不是"用不上"而是"不知道它们是什么形态"：
	// 给 `description` 写个 `*string` 就是替上游宣布"非 null 时是字符串"，
	// 而 `extInfo` 这个名字更像**容器**。编一个解析函数的下一步，
	// 一定是有人拿 `null` 当 `""` 去做判据（同 [FunaiModule] 对
	// `isDraw`/`drawTime`/`drawResult` 三个 null 字段的处置）。
	// ⇒ 登记成零样本项：抓到非 null 的 prize，再按实拍的形态声明。
}

// PlayAILotteryResult 是 ④ 的解析结果。
type PlayAILotteryResult struct {
	// Prize 是 `result.prize`。
	//
	// `nil` = 上游回了 `code:0` 但没给 `prize` —— **这种响应没有样本**。
	// 不报错是刻意的：成败已由 `code` 说过一次，这里再判一次会把一次
	// **已经抽掉**的抽奖读成"失败、可以再抽一次"。nil 交给调用方折叠成
	// 一句"抽了，但读不出中了什么"。
	Prize *PlayAIPrize
}

// PlayAIParseLottery 解析 ④ 的响应。
func PlayAIParseLottery(envelope *Envelope) (PlayAILotteryResult, *core.Error) {
	result, hasResult, perr := playAINewyearPayload(envelope)
	if perr != nil {
		return PlayAILotteryResult{}, perr
	}
	if !hasResult {
		return PlayAILotteryResult{}, nil
	}
	obj, ok := result.(map[string]any)
	if !ok {
		return PlayAILotteryResult{}, core.NewParse("blindbox/lottery 的 result 不是对象")
	}
	prizeRaw, hasPrize := obj["prize"]
	// 两种空态一起放过：`prize` 这个键**不在**，与给了 `null`（Go 侧解成 `nil`）。
	//
	// 为什么不分成两格：这一族的判据是 `code`，而"code=0 但没给 prize"这种情况
	// **一个样本都没有** —— 为它编两种不同的错误等于编两句瞎话。
	// 调用方拿到的都是 `Prize == nil`，即"抽掉了，但读不出中了什么"。
	if !hasPrize || prizeRaw == nil {
		return PlayAILotteryResult{}, nil
	}
	prizeObj, ok := prizeRaw.(map[string]any)
	if !ok {
		return PlayAILotteryResult{}, core.NewParse("blindbox/lottery 的 result.prize 不是对象")
	}
	return PlayAILotteryResult{Prize: &PlayAIPrize{
		PrizeID:       playAIOptInt(prizeObj, "prizeId"),
		PrizeName:     playAIOptStr(prizeObj, "prizeName"),
		PrizeType:     playAIOptInt(prizeObj, "prizeType"),
		Type:          playAIOptStr(prizeObj, "type"),
		Oid:           playAIOptID(prizeObj, "oid"),
		ButtonLink:    playAIOptStr(prizeObj, "buttonLink"),
		ButtonTxt:     playAIOptStr(prizeObj, "buttonTxt"),
		Tips:          playAIOptStr(prizeObj, "tips"),
		Title:         playAIOptStr(prizeObj, "title"),
		PrizeImg:      playAIOptStr(prizeObj, "prizeImg"),
		FirstMonthGet: playAIOptBool(prizeObj, "firstMonthGet"),
		High:          playAIOptBool(prizeObj, "high"),
	}}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ 图片识别：POST https://ai.yun.139.com/api/image/aiRecognize
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIRecognizeParams 是 ⑤ 的三个入参。
//
// 三个都必填 ⇒ 不做成可选：实拍只有一条，而"少带一个键会长成另一条请求"
// 这件事在这一族已被说过一次（见 [PlayAIStepClickRequest] 的字节断言）。
type PlayAIRecognizeParams struct {
	// UserID 是 body 的 `userId`（实拍是一串 19 位标识 ⇒ 本仓库**不存**真值，
	// 测试里用 `199…` 合成号）。
	//
	// ⚠ 它**不是**凭据、但**是**账号标识：由调用方注入（同 [AppProfile.UserDomainID]
	// 的口径 —— 填错会把请求打到别人的账号上）。
	UserID string
	// DataURL 是 body 的 `base64`。
	//
	// ⚠ 实拍这个键装的**不是裸 base64**，而是整条 **data URL**
	// （`data:image/jpg;base64,…`）。所以这里原样收下、不拼前缀也不剥前缀：
	// 剥/拼任何一次都会改变字节，而"改一个字节"在这条请求上**没有对拍样本**。
	DataURL string
	// ImageExt 是 body 的 `imageExt`（实拍 `jpg`）。
	//
	// ⚠ 它与 [PlayAIRecognizeParams.DataURL] 的前缀**实拍一致**（都是 `jpg`），
	// 但"必须一致"这句**未验证**（一条样本说不出上游是否比对两者）。
	// ⇒ 不自动从 DataURL 推、也不校验一致，只把两格原样发出去：
	// 自动推会替上游宣布一条它没说过的规则，而本层没资格。
	ImageExt string
}

// PlayAIRecognizeBody 拼 ⑤ 的请求体（八个键，逐字照实拍）。
//
// 实拍的键序就是字典序，所以 Go 的 map marshal 直接得到同一串字节
// （`timeout` 是数字、`uploadToCloud` 是裸 `true`，与实拍一致）。
func PlayAIRecognizeBody(params PlayAIRecognizeParams) map[string]any {
	return map[string]any{
		"base64":        params.DataURL,
		"channelId":     PlayAIRecognizeChannelID,
		"imageExt":      params.ImageExt,
		"recognizeType": PlayAIRecognizeType,
		"sendType":      PlayAIRecognizeSendType,
		"timeout":       PlayAIRecognizeTimeoutMS,
		"uploadToCloud": PlayAIRecognizeUploadToCloud,
		"userId":        params.UserID,
	}
}

// PlayAIRecognizeRequest 构造 `POST https://ai.yun.139.com/api/image/aiRecognize`。
//
// # 头用哪一套
//
// 实拍这一条是 **App 原生侧**发出的（`Authorization` + `x-DeviceInfo` +
// `x-yun-api-version` + `x-yun-tid` + `Origin: mcloudlocal://yun.139.com`，
// `Sec-Fetch-Site: cross-site`），与 ①…④ 那套 H5 头**不是一套东西**：
// 它**不带** `jwttoken`。所以这里走 [WithAppHeaders] 那一套公共设备头。
//
// 三处已知偏离（都登记在测试里，都不是本层的判据）：
//  1. `Content-Type`：实拍 `application/json`（**无 charset**），而 [WithAppHeaders]
//     补的是带 charset 的 [H5ContentType] ⇒ 这里先剥后补成 [PlayAIPlainJSONContentType]。
//     这条**必须**改回来 —— 它是"上游按第一个解析"那一类会静默生效的偏离。
//  2. `User-Agent`：夹具这一条带的是 iPad WebKit 形态
//     （`Mozilla/5.0 (iPad; …) MCloudApp/12.6.0 iPad AppLanguage/zh-CN`），
//     而 [AppProfile] 造的是 `mCloud_iPhone/…` 形态。这正是盘点文档 §5.6 说的
//     「三处设备画像互相矛盾，真机上到底哪个生效必须实测」—— **未实测** ⇒
//     沿用仓库那**一套**画像，不在这一页混第三种，免得把矛盾固化成三份。
//     要核它：抓一条未脱敏的同接口请求比对 `User-Agent` 与 `x-DeviceInfo`。
//  3. `sec-fetch-*` / `accept-language` / `accept-encoding`：WebKit 自动加的头，
//     本仓库其余接口都不发（现状如此），上游是否据此判什么 ⇒ 未测试。
func PlayAIRecognizeRequest(
	profile *AppProfile, timestamp int64, authorization, tid string, params PlayAIRecognizeParams,
) (core.HTTPRequest, *core.Error) {
	if params.UserID == "" {
		return core.HTTPRequest{}, core.NewBadRequest("aiRecognize 的 userId 不能为空（填错会打到别人的账号上）")
	}
	if params.DataURL == "" {
		return core.HTTPRequest{}, core.NewBadRequest("aiRecognize 的 base64 不能为空（实拍装的是整条 data URL）")
	}
	if params.ImageExt == "" {
		return core.HTTPRequest{}, core.NewBadRequest("aiRecognize 的 imageExt 不能为空")
	}
	request, err := core.NewRequestPost(fmt.Sprintf("%s/api/image/aiRecognize", PlayAIHostAI)).
		WithJSONBody(PlayAIRecognizeBody(params))
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	// 设备头由 [playAIAppHeaders] 补，`Content-Type` 也在那里按实拍换掉
	//（不在这里先剥：那会变成"两处都在管同一个头"，谁先谁后又成了坑）。
	return playAIAppHeaders(request, profile, timestamp, authorization, tid), nil
}

// playAIAppHeaders 给 ⑤ 补原生侧的头：公共设备头走 [WithAppHeaders]，
// 但 `Content-Type` **换成**实拍那个不带 charset 的 [PlayAIPlainJSONContentType]。
//
// ⚠ 换的方式必须是**先剥后补**（见 [dropHeader] 那条"两个同名头会被上游按
// 第一个解析"）：[WithAppHeaders] 末尾一定会追加带 charset 的 [H5ContentType]，
// 直接再追加一个就是两个 `Content-Type`，而请求看起来完全正常。
// ⇒ 整条链路上 `Content-Type` 只在本函数里产生第二次，测试按"恰好一个"钉住。
func playAIAppHeaders(
	request core.HTTPRequest, profile *AppProfile, timestamp int64, authorization, tid string,
) core.HTTPRequest {
	request = WithAppHeaders(request, profile, timestamp)
	request.Headers = dropHeader(request.Headers, "content-type")
	return request.
		WithHeader("Content-Type", PlayAIPlainJSONContentType).
		WithHeader("Accept", "*/*").
		WithHeader("Origin", PlayAILocalOrigin).
		WithHeader("x-yun-tid", tid).
		WithHeader("Authorization", authorization)
}

// PlayAIRecognizeResult 是 ⑤ 的解析结果（`data` 里本层带出去的那几格）。
//
// # `data` 里**不**声明的键
//
// 唯一样本 `data` 共七个键。`content` / `hasQuestion` / `language` 三个**只有 null 样本**
// ⇒ 下面单列说明为什么**仍然**给了它们一格（与 [PlayAIPrize] 那三个 null 键的处置
// **不同**，差别在这里：这三个有名字可核对语义、而本层的结论要拿它们当"识别结果"的
// 组成部分；prize 那三个连语义都没成文）。
type PlayAIRecognizeResult struct {
	// TaskID 是 `data.taskId` —— 实拍**字符串**形态的 19 位标识 `"1991452320654084604"`。
	//
	// ⚠ 必须按字符串原样带出：过一遍数字类型会**静默丢精度**（见 [Envelope] 那条
	// 19 位 `userDomainId` 的说明），而这一格将来是 `chat/v2/add`（SSE，**本族零样本**）
	// 的入参 —— 丢末几位等于问错了任务。
	TaskID *string
	// FileID 是 `data.fileId`（实拍值已脱敏 ⇒ 只声明键，不存样本）。
	//
	// 它是"这张图落到云盘后的那个文件"：清理那一族（`hcy/file/get`，主线处理）
	// 要的正是它，所以这一格必须交出去。
	FileID *string
	// HasText 是 `data.hasText`（实拍 `false`）。
	//
	// 指针：`false` 是**真实取值**，与"上游没给"分不开的话，
	// "图里有没有字"这个判据就会在缺字段那天开始永远为真/为假。
	HasText *bool
	// Recommend 是 `data.recommendList` —— 实拍三条推荐提问（「人物戴眼镜了吗」…）。
	//
	// ⚠ 元素必须是**字符串**，出现别的形态就报错：这一格是本层唯一会**展开成数组**的
	// 字段，"上游把元素换成对象"必须当场看见。
	// 另：实拍**没有**"缺这个键"或"给了空数组"的样本 ⇒ 本层不区分（`len == 0` 两态合一）。
	Recommend []string
	// Content 是 `data.content` —— 唯一样本里是 `null`。
	//
	// ⚠ 类型是**按名字猜的**（识别文本）：非 null 形态零样本。
	// 用的是"非字符串一律 nil"的收法，所以万一它是对象，
	// 这里表现为 nil（**不是**"上游没给"，而是"形态不是串"）——
	// 这一格**不参与任何判据**，故容忍；要真读它先补非 null 抓包。
	Content *string
	// HasQuestion 是 `data.hasQuestion` —— 唯一样本里是 `null`，形态同样未验证
	//（按布尔收，非布尔给 nil）。
	HasQuestion *bool
	// Language 是 `data.language` —— 唯一样本里是 `null`（按字符串收，同上）。
	Language *string
}

// PlayAIParseRecognize 解析 ⑤ 的响应。
func PlayAIParseRecognize(envelope *Envelope) (PlayAIRecognizeResult, *core.Error) {
	data, perr := playAIRecognizePayload(envelope)
	if perr != nil {
		return PlayAIRecognizeResult{}, perr
	}
	recommend, perr := playAIOptStrList(data, "recommendList")
	if perr != nil {
		return PlayAIRecognizeResult{}, perr
	}
	return PlayAIRecognizeResult{
		TaskID:      playAIOptStr(data, "taskId"),
		FileID:      playAIOptStr(data, "fileId"),
		HasText:     playAIOptBool(data, "hasText"),
		Recommend:   recommend,
		Content:     playAIOptStr(data, "content"),
		HasQuestion: playAIOptBool(data, "hasQuestion"),
		Language:    playAIOptStr(data, "language"),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 取值 helper（本文件专用）
//
// 为什么不再用包内既有的 [optStrField] / [optInt64Field] / [boolAt]：
// 那三个的**语义**与本层需要的一致（"形态不对就当没有"），但包内已经有一位
// 并行作者在写 `CloudPhone*` 那一族，同一包里再引入一份同义 helper 只会撞名。
// 所以这里放三份**逐字同语义**的本地版（`playAI*` 前缀），
// 并在函数上写明与公共版的一致性 —— 收口时由主线合并，不由这一页单方面动公共文件。
// ─────────────────────────────────────────────────────────────────────────────

// playAIOptStr 取可选字符串。非字符串（含数字）一律 nil。同 [optStrField]。
func playAIOptStr(obj map[string]any, key string) *string {
	if obj == nil {
		return nil
	}
	text, ok := obj[key].(string)
	if !ok {
		return nil
	}
	return &text
}

// playAIOptInt 取可选整数。非数字一律 nil。同 [optInt64Field]。
func playAIOptInt(obj map[string]any, key string) *int64 {
	if obj == nil {
		return nil
	}
	value, present := obj[key]
	if !present {
		return nil
	}
	number, ok := asInt64(value)
	if !ok {
		return nil
	}
	return &number
}

// playAIOptBool 取可选布尔。**只认真布尔**：上游哪天改成 `1`/`0` 这里得到 nil，
// 于是调用方读成"没给"而不是替它选一个方向。
func playAIOptBool(obj map[string]any, key string) *bool {
	if obj == nil {
		return nil
	}
	flag, ok := obj[key].(bool)
	if !ok {
		return nil
	}
	return &flag
}

// playAIOptID 取"数字或字符串都收"的标识格，统一成字符串。同 [optIDField]。
//
// ⚠ 与 [playAIOptInt] 的区别就是这个函数**不许**被换掉：`oid` 实拍是数字，
// 而它下一步要进 URL 查询串 —— 收成整数再格式化，19 位那种会先丢精度。
func playAIOptID(obj map[string]any, key string) *string {
	if obj == nil {
		return nil
	}
	switch value := obj[key].(type) {
	case string:
		return &value
	case json.Number:
		text := value.String()
		return &text
	case float64:
		text := strconv.FormatInt(int64(value), 10)
		return &text
	case int64:
		text := strconv.FormatInt(value, 10)
		return &text
	}
	return nil
}

// playAIOptStrList 取字符串数组。键缺席 ⇒ `(nil, nil)`；元素非字符串 ⇒ 报错。
func playAIOptStrList(obj map[string]any, key string) ([]string, *core.Error) {
	if obj == nil {
		return nil, nil
	}
	value, present := obj[key]
	if !present || value == nil {
		return nil, nil
	}
	items, ok := value.([]any)
	if !ok {
		return nil, core.NewParse(fmt.Sprintf("响应字段 `%s` 不是数组", key))
	}
	out := make([]string, 0, len(items))
	for _, raw := range items {
		text, ok := raw.(string)
		if !ok {
			return nil, core.NewParse(fmt.Sprintf("响应字段 `%s` 里有非字符串元素", key))
		}
		out = append(out, text)
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIUserStatus 发 ①。只读。
//
// `jwt` 用**公共来源**的票（见本文件头的 §1.12 那条临时态说明）。
func (u *Upstream) PlayAIUserStatus(
	ctx context.Context, jwt, referer, showLoading string,
) (PlayAIUserStatus, *core.Error) {
	envelope, perr := u.Call(ctx, PlayAIUserStatusRequest(jwt, referer, showLoading))
	if perr != nil {
		return PlayAIUserStatus{}, perr
	}
	return PlayAIParseUserStatus(envelope)
}

// PlayAITaskList 发 ②。只读。
func (u *Upstream) PlayAITaskList(
	ctx context.Context, jwt, referer, showLoading string,
) ([]PlayAIUserTask, *core.Error) {
	envelope, perr := u.Call(ctx, PlayAITaskListRequest(jwt, referer, showLoading))
	if perr != nil {
		return nil, perr
	}
	return PlayAIParseTaskList(envelope)
}

// PlayAIStepClick 发 ③。**写**：一次调用上报一步，是否重复上报由调用方管
// （实拍两条 `limitType`：`DAY` 每天一次、`EVER` 一次性 —— 上限语义未验证）。
func (u *Upstream) PlayAIStepClick(
	ctx context.Context, jwt, referer, showLoading string, taskID int64,
) *core.Error {
	request, perr := PlayAIStepClickRequest(jwt, referer, showLoading, taskID)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	return PlayAIParseStepClick(envelope)
}

// PlayAILottery 发 ④。**写**：一次调用抽一次。
func (u *Upstream) PlayAILottery(
	ctx context.Context, jwt, referer, showLoading string,
) (PlayAILotteryResult, *core.Error) {
	request, perr := PlayAILotteryRequest(jwt, referer, showLoading)
	if perr != nil {
		return PlayAILotteryResult{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return PlayAILotteryResult{}, perr
	}
	return PlayAIParseLottery(envelope)
}

// PlayAIRecognize 发 ⑤。
//
// ⚠ 它**会往云盘写一张图**（`uploadToCloud` 实拍恒 true，见
// [PlayAIRecognizeUploadToCloud]）。清理那一格（`data.fileId`）交回调用方，
// 由"上传/清理"那一族处理 —— 本层不留悬空文件的策略。
func (u *Upstream) PlayAIRecognize(
	ctx context.Context, profile *AppProfile, timestamp int64, authorization, tid string,
	params PlayAIRecognizeParams,
) (PlayAIRecognizeResult, *core.Error) {
	request, perr := PlayAIRecognizeRequest(profile, timestamp, authorization, tid, params)
	if perr != nil {
		return PlayAIRecognizeResult{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return PlayAIRecognizeResult{}, perr
	}
	return PlayAIParseRecognize(envelope)
}
