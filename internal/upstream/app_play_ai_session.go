// 拍拍系列（#16）的**三样前置** —— 换票链、`visitlog/journaling` 埋点、`chat/v2/add` 请求。
//
// # 本文件补的是"五口跑不通"那一段
//
// [`app_play_ai.go`](app_play_ai.go) 把 ①…⑤ 落齐并测绿，但旧 TS 的编排把三样东西当**前置**，
// 本仓此前一行都没有：
//
//	甲 换票链（这一页自己的 `jwttoken` 从哪来）    `apis/auth.ts:61-97` + `task.ts:50-70`
//	乙 POST /ycloud/visitlog/journaling（埋点）   `apis/play-ai-special.ts:13-15` + `task.ts` 十余处
//	丙 POST …/assistant/chat/v2/add（SSE）        `apis/play-ai-special.ts:36-38` + `task.ts:170-208`
//
// # ⚠⚠ 三样的证据等级**不一样**，全文件按这三档写注释与测试
//
//	**A 档 · 本仓实拍**：只有**一条**，而且不属于这一页 ——
//	  `POST https://m.mcloud.139.com/ycloud/visitlog/journaling` 在**直播小红花那一族**
//	  实拍过（`docs/capture/app-live-redflower-samples.txt` §5.1 第 6 条，2026-09-22 09:05:09）：
//	  `content-type: application/x-www-form-urlencoded;charset=UTF-8`、body 是
//	  `module=uservisit&optkeyword=…&sourceid=1007&marketName=…` 这一串、响应
//	  `{"code":0,"msg":"success"}`。**同 URL、同 host、同门户**，只差 `optkeyword`/`sourceid`/`marketName` 三格。
//	**B 档 · 旧 TS 的字面量**：甲乙丙的**全部**其余事实（端点、参数、键名、键值、顺序、头集合）。
//	**C 档 · 本仓已实跑通的同族实现**：H5 签名的盐与算式（[SignH5RequestWith]，
//	  `docs/capture/app-signin-samples.txt` 端到端复现过）、[TyrzLoginRequest] 的头组装、
//	  以及 ①…④ 那一族的头纪律（[playAIH5Headers]）。
//
// 这一页**自己**对甲/乙/丙**零实拍**（夹具 `夏日拍拍大作战.json` 里换票那一步、埋点、chat 一条都没有）。
// 于是本层的成文与测试只允许两种句子：
//   - "我们发出的这一组恰好是这个形状"（测试钉这个）；
//   - "这个形状出自旧 TS 的哪一行"（注释钉这个）。
//
// **不允许**出现"上游要求 X"或"上游不校验 X"—— 两个方向都没样本（AGENTS.md §1.7）。
//
// 为什么这一轮允许落地而不是"抓到才写"：用户口径是"先按参考项目把业务逻辑迁进来，
// 之后再慢慢抓包分析更新"（`docs/business-feature-inventory.md` §2.4 那条 Go 侧现状、
// `docs/pending-decisions.md` P5）⇒ **接口层冻结、业务层继续**。本文件因此是"可核验的形状登记"，
// 不是"已验证的上游契约"。要把它升级成后者：抓一条本页的换票 + 一条本页的埋点（活动窗外补不了 chat）。
//
// # 与既有实现的关系（**不复制第二份**）
//
//   - 端点 `…/auth-service/auth/tyrzLogin` 本仓**已有** [TyrzLoginRequest]（签到页那一条，实测能过）。
//     这里的 [PlayAITyrzLoginRequest] 是**它的复用 + 一处差别**：body 换成本页那六键、
//     另加一枚 `type: signupPlatform` 头。**URL / 方法 / 头集合 / 签名四件套完全相同**，
//     差异全在参数上 —— 正是 §1.12 事实②那句"URL 可能相同而参数不同"。
//   - 端点 `querySpecToken` 本仓**已有** [QuerySpecTokenRequest]，但那是**另一个 URL**
//     （`user-njs.yun.139.com/user/querySpecTokenV2`，**POST** + body `{toSourceId}`，原生 A 族头）。
//     这一页要的是 `m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/querySpecToken`
//     （**GET** + 查询串 + H5 签名头）。两条**不是**同一接口，测试里各有一条对照专门防止"顺手统一"。
//   - 签名：复用 [H5Signature] / [SignH5RequestWith]，本层**不自己算**（见下面盐值比对）。
//   - 埋点：与 [LiveRoomJournalingRequest] 同一端点、同一 body 语法，只差三格取值。
//
// # 签名盐与拼法的**逐字比对结论**（写代码前做的，结论是"同"）
//
//	旧 TS  `packages/core/src/auth/sign.ts:3-6`   SALT_MAP = {1: seedMdYYLIZfbCxg, 2: sekaMdYYLIZfbCfm}
//	旧 TS  `sign.ts:19`                           `${salt}${id}${timestamp}${nonce}${other}${salt}`
//	旧 TS  `sign.ts:29-42`                        `getSignHeader(ts?, saltKey=1, other?)` ⇒ other 缺席时是 `""`
//	旧 TS  `task.ts:54` `:62`                      两步都是 `getSignHeader(undefined, 2)`
//	本仓   `internal/core/crypto.go:65-68`         signSaltMap = {1: seedMdYYLIZfbCxg, 2: sekaMdYYLIZfbCfm}
//	本仓   `internal/core/crypto.go:675-689`       `md5(salt+id+timestamp+nonce+other+salt)`
//	本仓   `internal/upstream/app_signin_session.go:87-100`  `SignH5RequestWith` = 上式，盐号 [H5SignSaltKey] = 2
//
// ⇒ **盐号相同（2）、盐值同字、拼接顺序同字、`other` 那段同为空串** ⇒ 直接复用 [SignH5RequestWith]。
// ⚠ **一处真实差别**不在算式而在**时间戳来源**：旧 TS `sign.ts:36` 是 `timestamp ?? Date.now()`
// （本地时钟），本仓这条链的 `x-timestamp` 取自服务端 [H5TimestampRequest]（理由写在
// [SignH5Request] 那段："本地时钟偏一点就是 401"）。这里**沿用仓库那一份**（C 档：实测能过），
// 并登记：上游对这两个来源是否都认 ⇒ 未测试。要核它：同一条换票请求分别用服务端与本地时间戳各发一次。
//
// # `X-Tingyun` 那枚头：**本层不发**（三处都同样处理）
//
// 旧 TS `task.ts:29-36` 的 `makeHeaders()` 给埋点与 chat 都带
// `X-Tingyun: c=…;x=…;u=base64#…` —— 那是**听云 APM** 的采样标识串，在本仓其余 H5 族
// 一个都没有（`grep -i tingyun internal/` 为空）。不发的三条理由：
//  1. 那串是旧项目里**写死**的（含一个 `u=` 段，疑似某次会话/设备的标识），照抄等于把
//     别人的标识长期挂在自己的请求上；
//  2. 本仓这一族的头集合是**按夹具**裁出来的（[playAIH5Headers] 多一个头都要说明为什么）；
//  3. 没有任何证据说上游据此判什么 ⇒ **反向**断言"不需要"同样不合法。
//
// ⇒ 处置：不发 + 测试钉"恰好没有这一枚"（钉的是我们的选择），并在真打不认账时把它列进
// 第一个要复核的方向（与 `app_play_ai.go` 文件头那条"③④ 回非 0 先复核埋点"并列）。
//
// # chat/v2/add：**拍拍（#16）这一套只构造请求，不解析响应**
//
// 响应是 **SSE**（`Accept: text/event-stream`）。本仓 [Upstream.Call] 一律 [ParseEnvelope]（JSON），
// 而这一条的响应体**零样本** —— 于是这一族**不猜事件名、不写解析器**，
// 也**不提供** `(u *Upstream).PlayAIChat` 那个发送方法（拍拍这一套至今仍是这样）。
//
// ⚠ **585 AI 相机那一套已经开了这个口子**：读法、判据与发送口在
// [`app_play_ai_chat_sse.go`](app_play_ai_chat_sse.go)。开它的理由与"为什么拍拍这一套仍不开"
// 都写在那一份的文件头（一句话：AI 相机是一条可点的玩法，第一段读数可以由本程序真跑造出来；
// 拍拍那一页要等活动窗口）。**不要**把两套的 body / 头集合合并 —— 三处硬差别列在那一份里。
//
// 反射扫方法名那条测试（`app_play_ai_session_test.go` 的最后一条）因此**改向**了：
// 它现在钉的是"拍拍那一套没有发送口、585 那一套恰好有一个"。
package upstream

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（**B 档：逐条抄旧 TS**，不是猜的；本仓对这一页零实拍）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// PlayAISpecTokenTargetSourceID 是换票第 ① 步的 `targetSourceId`。
	//
	// 出处 `apis/auth.ts:64-73`（`sourceId = "001005"` 是**默认实参**，`task.ts` 调用时没覆盖它）。
	//
	// ⚠ 它与 [ToSourceID] **同值**，但那是**两个参数**：签到那一个的出处是"签到页 URL 自带的
	// `targetSourceId=001005`"（实拍），这一个的出处是旧 TS 的默认实参（零实拍）。
	// [ToSourceID] 自己的注释就写着"它不是端常量，是调用方按场景选的参数"（同一接口出现过
	// `001003`/`001005`/`010001` 三个值）⇒ 合并成一个常量迟早会在某一期只动一个值时静默错。
	PlayAISpecTokenTargetSourceID = "001005"

	// PlayAISpecTokenHideLoading 是第 ① 步查询串的第二个键（旧 TS 把它**写死在 URL 模板里**，
	// `apis/auth.ts:70`）。它看起来是 H5 的 loading 蒙层开关（与 `showloading` 头同类），
	// 但**语义没有成文** ⇒ 做成常量并原样带出，不参数化、不解释。
	PlayAISpecTokenHideLoading = "true"

	// PlayAITyrzLoginMarketName 是第 ② 步 body 的 `marketName`（旧 TS 默认值 `apis/auth.ts:90`）。
	//
	// ⚠⚠ 它**不是** [PlayAIMarketName]（`National_PlayAISpecial`，那是活动接口那一份）。
	// 同一件事里有**两个**叫 marketName 的参数、取两个不同的值，且**同时**出现在
	// `tyrzLogin` 的 body 与 `newyear/*` 的查询串里 —— 这是这一族最容易抄错的一格。
	// 两者的关系（换票那一个是不是**期次无关**的固定入口名）**没有样本**，不解释。
	PlayAITyrzLoginMarketName = "shake_shake"

	// PlayAITyrzLoginSourceID 是第 ② 步 body 的 `sourceId`（旧 TS 默认值 `apis/auth.ts:91`）。
	//
	// ⚠ 与 [SourceID]（签到那一份 `1002`）不同值 ⇒ 也别合并。
	PlayAITyrzLoginSourceID = "1314"

	// PlayAIAuthType 是第 ② 步 body 的 `type` **与**同名请求头 `type` 的取值
	// （旧 TS `apis/auth.ts:92-95`：同一个字面量在**两处**都出现）。
	//
	// ⚠ "两处必须同字"这句**没有证据**，只是旧 TS 恰好复用了同一个串。
	// 本层用**同一个常量**填两处，因此这一格若要分开，改的是这个常量而不是散落的字面量。
	PlayAIAuthType = "signupPlatform"

	// PlayAIPageSourceID 是这一页落地 URL 的 `sourceid`（旧 TS `task.ts:33-34` 的 Referer）。
	//
	// 它与埋点那三档 `sourceid` 中的 `2019` 同值 —— 同值不等于同一格：一个是页面入口标识、
	// 一个是埋点事件分组（见下面三个 `PlayAIJournalingSourceID*`）。
	PlayAIPageSourceID = "2019"
)

// 埋点 body 的固定段与三档 `sourceid`（B 档：`task.ts` 逐处抄下）。
const (
	// PlayAIJournalingModule 是埋点 body 第一格（实拍同端点在另一族也是这个值，A 档）。
	PlayAIJournalingModule = "uservisit"

	// PlayAIJournalingSourceIDPage 是**页面级**那几条埋点的 `sourceid`：
	// 入口三条、任务前三条、抽奖前两条（`task.ts:339-341`、`:376-389`、`:405-411`），
	// 以及 `aiCamera` 的**第二条**（`:118`）。
	PlayAIJournalingSourceIDPage = "2019"
	// PlayAIJournalingSourceIDCamera 是 `aiCamera` **第一条**埋点的 `sourceid`（`task.ts:92`）。
	//
	// ⚠ 同一个 handler 里两条埋点用两个不同的 `sourceid`（2008 / 2019）—— 旧 TS 就这么写的，
	// 差别为什么存在**没有解释**。别"顺手统一"：统一后就再也对不上那份出处。
	PlayAIJournalingSourceIDCamera = "2008"
	// PlayAIJournalingSourceIDRedbook 是小红书那件任务四条埋点的 `sourceid`（`task.ts:226-229`）。
	PlayAIJournalingSourceIDRedbook = "2014"

	// PlayAIJournalingContentType 是埋点这条的 `content-type`。
	//
	// # 这是**我们的选择**，不是这一页的实拍结论
	//
	// 支持它的是 A 档那一条：**同一个 URL** 在直播小红花族实拍就是 form-urlencoded
	// （`LiveRoomJournalingRequest` 用的字面量与此逐字相同）。
	// 但旧 TS 这一页发的是**它那个专用客户端的静态头** `content-type: application/json;charset=UTF-8`
	// 配一个**字符串** body（`play-ai-special/http.ts:22` + `apis/play-ai-special.ts:14`），
	// 也就是"头说 JSON、体是 form 串"。两种形态**都没有本页实拍**。
	//
	// ⇒ 选与 body 形态自洽的那一个（form-urlencoded），理由只有一条：同端点的唯一实拍长这样。
	// ⚠ 不许据此断言"上游认 form"或"上游不认 JSON"。要核它：抓本页一条埋点，或真打时两种各发一次
	// 比对响应（这是**写**性质的埋点请求，比对时留意别把两次访问算成两条活动足迹）。
	PlayAIJournalingContentType = "application/x-www-form-urlencoded;charset=UTF-8"
)

// 埋点 `optkeyword` 的**后缀**表（前缀恒为 [PlayAIMarketName] + `_`）。
//
// 逐条来自 `task.ts`，一条不多一条不少；每个常量的注释标它那一串出现在**哪一步**、
// 以及那一步用的 `sourceid` 是哪一档 —— 后缀与 `sourceid` 的配对关系是这族埋点唯一的
// "看得出出处"的信息，丢了它就只剩一串没有来源的字符串。
//
// ⚠ 常量名里的 `App` 后缀**不改变字面值**：`click_totask0` 与 `click_totask0_app` 是
// 两条**不同**的事件（旧 TS 连着发两条），不是"同一条的两种写法"。
const (
	PlayAIJournalingSuffixPV              = "pv"                // task.ts:339 入口第 1 条（sourceid 2019）
	PlayAIJournalingSuffixClient          = "client"            // task.ts:340 入口第 2 条（2019）**与** aiCamera 第 1 条（2008，task.ts:92）—— 同字两条，`sourceid` 不同
	PlayAIJournalingSuffixClientIsApp     = "client_isApp"      // task.ts:341 入口第 3 条（2019）
	PlayAIJournalingSuffixClickToTask0    = "click_totask0"     // task.ts:376 任务前第 1 条（2019）
	PlayAIJournalingSuffixClickToTask0App = "click_totask0_app" // task.ts:382 任务前第 2 条（2019）
	PlayAIJournalingSuffixClickLottery    = "click_lottery"     // task.ts:405 抽奖前第 1 条（2019）
	PlayAIJournalingSuffixClickLotteryApp = "click_lottery_app" // task.ts:411 抽奖前第 2 条（2019）
)

// `chat/v2/add` body 的固定字面量（B 档：`task.ts:170-208` 逐字段）。
const (
	// PlayAIChatApplicationType 是 body 的 `applicationType`（`task.ts:176`）。
	PlayAIChatApplicationType = "chat"
	// PlayAIChatSourceChannel 是 body 的 `sourceChannel`（`task.ts:178`）。
	//
	// ⚠ 它与 ⑤ 的 [PlayAIRecognizeChannelID] 同值（都是 `"101"`），但是**两个请求的两格**：
	// ⑤ 那一条**有实拍**，这一条没有 ⇒ 不合并成一个常量（合并后换期只改得动一处）。
	PlayAIChatSourceChannel = "101"
	// PlayAIChatCommand 是 `dialogueInput.command.command`（`task.ts:192`）。
	PlayAIChatCommand = "036"
	// PlayAIChatSubCommand 是 `dialogueInput.command.subCommand`（`task.ts:192`）。
	PlayAIChatSubCommand = "036006"
	// PlayAIChatH5Version 是 `dialogueInput.versionInfo.h5Version`（`task.ts:189`）。
	//
	// ⚠ 它是**旧 TS 写死的**"H5 版本号"，与 [H5AppVersion]/[IOSAppVersion] 都不同值。
	PlayAIChatH5Version = "1.0.0"
	// PlayAIChatExtInfo 是 `dialogueInput.extInfo`：**一个装着空对象的字符串**
	// （`task.ts:190` `extInfo: "{}"`）。不是对象、不是空串 —— 三个形态差一个字节。
	PlayAIChatExtInfo = "{}"

	// 七枚开关逐字照 `task.ts:183-188` 与 `:193`。**为什么做成常量**：五枚 `false` + 两枚 `true`
	// 这种组合读起来像"随手填的"，而它是旧实现唯一跑过的那一档；有人"顺手把 enableForceLlm 打开"
	// 时，常量测试先红，逼他先去看这一格有没有样本（答案是没有：这一条**整段**零实拍）。
	PlayAIChatEnableForceLlm                    = false // task.ts:183
	PlayAIChatEnableForceNetworkSearch          = true  // task.ts:184
	PlayAIChatEnableModelThinking               = false // task.ts:185
	PlayAIChatEnableAllNetworkSearch            = false // task.ts:186
	PlayAIChatEnableKnowledgeAndNetSearch       = false // task.ts:187
	PlayAIChatEnableRegenerate                  = false // task.ts:188
	PlayAIChatEnableLlmDescribe                 = true  // task.ts:193 `toolSetting.imageToolSetting`
	PlayAIChatAttachmentTypeImage         int64 = 3     // task.ts:195 `attachment.attachmentTypeList[0]`
)

// ─────────────────────────────────────────────────────────────────────────────
// 这一页的 Referer
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIPageReferer 是旧 TS 给埋点与 chat 用的那个 `Referer`（`task.ts:33-34` 逐字）：
//
//	https://m.mcloud.139.com/portal/yunpanpage/index.html?path=National_PlayAISpecial&sourceid=2019&enableShare=1
//
// ⚠ **它是 B 档**：①…④ 那六条实拍的 `referer` 头**已脱敏**，所以"本页真实发出的 referer 长这样"
// 没有实拍支撑 —— 这里给的是"旧 TS 用的那一串"。
//
// 另一处刻意不同：签到族那份页面 URL（[SigninPageURL]）会把 SSO token 拼进查询串，
// 这一份**不含任何凭据**（旧 TS 那一份也不含）。凭据只走 `jwttoken` 头 ——
// 一个会过期的凭据拼进 URL，只会让日志与错误里多一处凭据（同 [LiveRoomPageReferer] 的处置）。
func PlayAIPageReferer() string {
	return fmt.Sprintf("%s/portal/yunpanpage/index.html?path=%s&sourceid=%s&enableShare=1",
		BaseMcloud, PlayAIMarketName, PlayAIPageSourceID)
}

// ─────────────────────────────────────────────────────────────────────────────
// 甲·换票 ①：GET …/ycloud/api/cloud/userdomain/v2/querySpecToken
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIQuerySpecTokenRequest 构造换票第①步（**GET + 签名四件套，不带票**）。
//
//	GET https://m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/querySpecToken
//	    ?targetSourceId=001005&hideLoading=true
//
// 两个查询键的**先后**照旧 TS 的 URL 模板（`apis/auth.ts:70`）：查询串顺序在 HTTP 上通常无所谓，
// 但这一条零实拍 ⇒ 按**字节**可追出处的写法优先（对拍时能直接 diff）。
//
// # 头集合：取 C 档那一份（[TyrzLoginRequest] 同一套组装）
//
// [playAIH5Headers]（①…④ 那一族按夹具裁的）在这里**不适用**：它把 `jwt` 做成必填字符串，
// 而这一步的存在**就是为了**拿票。于是这里走 [WithH5Headers] + `Origin` + 签名四件套 ——
// 与签到族那条已实跑通的建会话请求逐字同一套（含 `activityid: sign_in_3` 与 `appversion`）。
//
// ⚠ 这一处**偏离**本族"少三个头"的取向，理由与风险都写清：
//   - 那六条夹具实拍证明的是 `newyear/*` 那四条**不发** `activityid`/`appversion`；
//     它没说 `auth-service` 与 `userdomain` 这两个服务怎么要求（**换票两条零实拍**）。
//   - 反过来，同一个 `tyrzLogin` 端点**带**这两个头是实测能过的（C 档）。
//     ⇒ 在两个都没有证据的方向里，取"同端点有实证的那一套"。
//   - 代价：这一条带的头比 ①…④ 多两个。**删掉它们会不会也过 ⇒ 未测试**；
//     真打时若第②步回 `401 请求签名校验失败`，第一个查 `x-timestamp` 的来源
//     （[H5TimestampRequest]），第二个才是这两个多出来的头。
func PlayAIQuerySpecTokenRequest(sign H5Signature, referer, showLoading string) core.HTTPRequest {
	request := playAIQuerySpecTokenURL()
	request = WithH5Headers(request, referer, nil, showLoading).
		WithHeader("Origin", BaseMcloud)
	return playAIAttachSignature(request, sign)
}

// playAIQuerySpecTokenURL 拼第①步的 URL（**GET、无 body**）。
func playAIQuerySpecTokenURL() core.HTTPRequest {
	return core.NewRequestGet(playAIQuery(
		fmt.Sprintf("%s/ycloud/api/cloud/userdomain/v2/querySpecToken", BaseMcloud),
		[2]string{"targetSourceId", PlayAISpecTokenTargetSourceID},
		[2]string{"hideLoading", PlayAISpecTokenHideLoading},
	))
}

// playAIAttachSignature 把 [H5Signature] 的四枚头挂上去。
//
// 为什么本文件要有这个函数而 [TyrzLoginRequest] 里已经写了一遍：第①步是 GET，
// 复用不了那个 POST 构造函数，而四枚头的**名字与顺序**必须与它逐字一致
// （两条是同一条链的两步，对拍时要能看出"只差 body 与一枚 type 头"）。
// ⇒ [PlayAITyrzLoginRequest] 直接调 [TyrzLoginRequest]，这里调本函数，两处共用同一份头名表。
func playAIAttachSignature(request core.HTTPRequest, sign H5Signature) core.HTTPRequest {
	return request.
		WithHeader("x-request-id", sign.RequestID).
		WithHeader("x-timestamp", sign.Timestamp).
		WithHeader("x-nonce", sign.Nonce).
		WithHeader("x-signature", sign.Signature)
}

// ParsePlayAIQuerySpecToken 读第①步的 `result` —— 换票链的**中间凭据**（SSO token）。
//
// 旧 TS 的类型声明是 `QuerySpecTokenNewResponse{code:number, msg:string, result:string}`
// （`apis/auth.ts:6-10`）：`result` 是**裸字符串**，不是对象。
// 这与 [ParseSSOToken]（`querySpecTokenV2` 那条）不同 —— 那一条的 payload 是 `{token: …}` 对象。
// 两个端点两个形态，**别拿一个解析器去解两条**。
//
// 信封判据复用 [playAINewyearPayload]（`code` 必须是**数字字面量**）：这一条的声明形态与
// ①…④ 同构。⚠ 那是"按 B 档的声明收"，不是"这一条实拍过"；把 ⑤ 那族（`code` 是字符串）
// 喂进来会像 ①…④ 一样当场响 —— 这条反向用例在下面。
func ParsePlayAIQuerySpecToken(envelope *Envelope) (string, *core.Error) {
	result, hasResult, perr := playAINewyearPayload(envelope)
	if perr != nil {
		return "", perr
	}
	if !hasResult {
		return "", core.NewParse("querySpecToken 响应没有 result（旧 TS 声明这一族必带）")
	}
	token, ok := result.(string)
	if !ok {
		// 不"顺手兼容 object 形态"：兼容就等于替上游宣布"它哪天改成对象我们也能读"，
		// 而这一条本仓连一条实拍都没有。
		return "", core.NewParse(fmt.Sprintf(
			"querySpecToken 的 result 不是字符串，实得 %T（旧 TS 声明是 string，不是对象）", result))
	}
	if token == "" {
		return "", core.NewParse("querySpecToken 的 result 是空串（拿它去换票只会得到一次更难懂的失败）")
	}
	return token, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 甲·换票 ②：POST …/ycloud/auth-service/auth/tyrzLogin（这一页的参数变体）
// ─────────────────────────────────────────────────────────────────────────────

// PlayAITyrzLoginBody 拼第②步的 body（六个键）。
//
// 与 [TyrzLoginBody]（签到那一份，四个键）的**逐键差别** —— 这就是 §1.12 事实②的现场：
//
//	marketName   shake_shake        vs  [MarketName] sign_in_3
//	sourceId     1314                vs  [SourceID] 1002
//	openAccount  **false**（布尔）    vs  **0**（数字）        ← 旧 TS 那份是布尔，本仓那份是数字
//	type         signupPlatform      vs  （没有这一键）
//	channel      null                vs  （没有这一键）
//	token        同一个含义（第①步换来的 SSO token）
//
// ⚠ `openAccount` 那一处值得单独说：`false` 与 `0` 在**这条请求上是否等价**没有样本
// （本仓那一份的 `0` 是**实拍**来的，旧 TS 这一份的 `false` 是它自己代码里的）。
// 这里按"这一页的出处"逐字写 `false`，不折成 `0`：把两种形态统一起来
// 就是替上游宣布"它不区分"，而它区分不区分我们不知道。
func PlayAITyrzLoginBody(ssoToken string) map[string]any {
	return map[string]any{
		"token":       ssoToken,
		"openAccount": false,
		"marketName":  PlayAITyrzLoginMarketName,
		"sourceId":    PlayAITyrzLoginSourceID,
		"type":        PlayAIAuthType,
		"channel":     nil,
	}
}

// PlayAITyrzLoginRequest 构造换票第②步 —— **复用** [TyrzLoginRequest] 再加一枚头。
//
// 与签到那一条的差别**只有两处**：body（见 [PlayAITyrzLoginBody]）与多出来的
// `type: signupPlatform` 请求头（旧 TS `apis/auth.ts:95` 把它拼进 headers）。
// URL / 方法 / 签名四件套 / `Origin` 全部继承 —— 同一端点，不该有两份实现。
//
// ⚠ `type` 这枚头**没有实拍**（本仓签到族那条实拍的请求头里没有它，因为那一步不需要传）。
// 为什么仍然照发：这一页的 body 与头**成对**出现在同一份出处里，只发一半是**我们自己**造的新形态，
// 比"两处都按出处发"更难对拍。上游认不认这枚头 ⇒ 未测试。
func PlayAITyrzLoginRequest(
	sign H5Signature, referer, ssoToken, showLoading string,
) (core.HTTPRequest, *core.Error) {
	request, perr := TyrzLoginRequest(sign, referer, PlayAITyrzLoginBody(ssoToken))
	if perr != nil {
		return request, perr
	}
	// `WithH5Headers` 里 `showloading` 被 TyrzLoginRequest 钉成了 "true"。这一页的
	// `hideLoading=true` 说明它想**关**蒙层 ⇒ 先剥后补成调用方给的值（不剥会发出两个同名头，
	// 上游按第一个解析 —— 见 [dropHeader]）。
	if showLoading != "" {
		request.Headers = dropHeader(request.Headers, "showloading")
		request = request.WithHeader("showloading", showLoading)
	}
	return request.WithHeader("type", PlayAIAuthType), nil
}

// PlayAISession 是第②步的产物：这一页自己的会话凭据。
//
// ⚠ 结构与 [H5Login] 有意不同 —— 这里多带 `UserDomainID`：⑤ `aiRecognize` 的 body 要一个
// `userId`（旧 TS 从 `ctx.state.userDomainId` 拿，`task.ts:327-328`），而旧 TS 声明的
// `TyrzLoginNewResponse.result` 里就有 `userDomainId`（`apis/auth.ts:41-47`）。
// 那三个额外键（`provCode`/`areaCode`/`userDomainId`）本仓**只读这一个**，
// 且它是**指针**：这一页零实拍，"缺了就是坏响应"这句话我们没资格说。
type PlayAISession struct {
	// JWTToken 是这一步的**唯一目的**：后面 ①…④ 与埋点要带的 `jwttoken`。
	JWTToken string
	// Mobile 是响应回显的手机号（确认这张票签给谁）。可能缺失 ⇒ 指针。
	Mobile *string
	// UserDomainID 是 ⑤ 那一条 `userId` 的候选来源（见结构体上那段）。可能缺失 ⇒ 指针。
	UserDomainID *string
}

// ParsePlayAITyrzLogin 解析第②步的响应。
//
// 判据用 [EnsureOKWithHint]（与 [ParseJWTToken] 同一个）：**不是** [playAINewyearPayload]。
// 理由是这一条的**失败档**要能被上层类型化认出来：`90001` 会落成
// [core.NewSessionExpired]（换票这一步报"未登录"听起来怪，但那条判据来自公共表，
// 与签到族同端点同处置）。`result.token` 缺失/非字符串 ⇒ 报错。
//
// ⚠ 这一条**整段**都是 B 档形状（旧 TS 的 interface），本仓对这一页零实拍。
func ParsePlayAITyrzLogin(envelope *Envelope) (PlayAISession, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return PlayAISession{}, perr
	}
	if payload == nil {
		return PlayAISession{}, core.NewParse("tyrzLogin 响应没有 result")
	}
	token, perr := RequireStr(payload, "token")
	if perr != nil {
		return PlayAISession{}, perr
	}
	session := PlayAISession{JWTToken: token}
	// 两格可选：只认字符串形态，别的形态一律 nil（不猜、不转换）。
	if text, ok := payload.(map[string]any)["mobile"].(string); ok {
		session.Mobile = &text
	}
	if text, ok := payload.(map[string]any)["userDomainId"].(string); ok {
		session.UserDomainID = &text
	}
	return session, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 乙·埋点：POST /ycloud/visitlog/journaling
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIJournalingKeyword 把后缀拼成一条 `optkeyword`。
//
// 规则只有一条，而它是**从旧 TS 那十余条埋点串里逐条数出来的**（每一条都是
// `National_PlayAISpecial_<后缀>`）：前缀取 [PlayAIMarketName]，中间一个下划线。
//
// ⚠ 这条规则是 B 档。而且注意它**不**适用于 `marketName` 那一格 —— 那一格用的是
// [PlayAIMarketName] 全名，不带后缀。两格同字面量但不是一回事。
func PlayAIJournalingKeyword(suffix string) string {
	return PlayAIMarketName + "_" + suffix
}

// PlayAIJournalingClickTaskKeywords 返回 `aiCamera` 那条**第二枚**埋点的 optkeyword
// （`task.ts:118`：`click_task_<id>_app`，`sourceid` 是 2019）。
func PlayAIJournalingClickTaskAppKeyword(taskID int64) string {
	return PlayAIJournalingKeyword(fmt.Sprintf("click_task_%d_app", taskID))
}

// PlayAIJournalingClickTaskKeyword 返回**任务前**第三条的 optkeyword
// （`task.ts:388`：`click_task_<id>`，**没有** `_app` 尾，`sourceid` 2019）。
//
// ⚠ 它与上面那个只差一个后缀，分属**两个不同的调用点**（任务前 / aiCamera 里），
// 别合并成一个函数。
func PlayAIJournalingClickTaskKeyword(taskID int64) string {
	return PlayAIJournalingKeyword(fmt.Sprintf("click_task_%d", taskID))
}

// PlayAIJournalingCompleteTaskKeywords 返回旧 TS 为小红书那件任务发的**四条**。
//
//	`complete_task<id>`、`complete_task<id>_app`、`complete_task_<id>`、`complete_task_<id>_app`
//	（`task.ts:226-229`，`sourceid` 全是 2014）
//
// ⚠⚠ 这四条**不是**"上游要四条"，而是**旧实现在猜格式**：`complete_task<id>` 与
// `complete_task_<id>` 只差一个下划线，同一件事不可能有两个真名 —— 作者当时也不知道
// 是哪一种，于是四种全发（循环里连结果都不看，`task.ts:231-233`）。
//
// ⇒ 本层的处置：**把四串原样交出去，并标明它们是四种猜法**。发几条、发哪条是 app 层的事
// （等一条实拍就能收成一条）。这里不"顺手只留一种"，那等于替旧实现宣布它猜对了哪一种。
func PlayAIJournalingCompleteTaskKeywords(taskID int64) []string {
	return []string{
		PlayAIJournalingKeyword(fmt.Sprintf("complete_task%d", taskID)),
		PlayAIJournalingKeyword(fmt.Sprintf("complete_task%d_app", taskID)),
		PlayAIJournalingKeyword(fmt.Sprintf("complete_task_%d", taskID)),
		PlayAIJournalingKeyword(fmt.Sprintf("complete_task_%d_app", taskID)),
	}
}

// PlayAIJournalingBody 拼一条埋点的**请求体**（不是 JSON！）。
//
// 四个键、顺序固定、值是**原样**（不做百分号编码）：
//
//	module=uservisit&optkeyword=<…>&sourceid=<…>&marketName=National_PlayAISpecial
//
// 三处理由：
//   - 顺序与键名照 A 档那条实拍（`LiveRoomJournalingRequest` 拼的是同一个模板）；
//   - **不编码**：实拍的 `optkeyword` 与 `marketName` 全是 `[A-Za-z0-9_]`，编码后逐字节不变；
//     而 [PercentEncode] 会把任何意外进来的字符吃掉，"看不出被改过"比"当场变样"更糟。
//     ⇒ 这里刻意**不**调用 PercentEncode，改由测试钉"含空格/`&` 的入参会原样出现"，
//     把"要不要编码"这件事留到有实拍可对拍时再定（现在两种都没有证据）。
func PlayAIJournalingBody(optkeyword, sourceID string) string {
	return fmt.Sprintf("module=%s&optkeyword=%s&sourceid=%s&marketName=%s",
		PlayAIJournalingModule, optkeyword, sourceID, PlayAIMarketName)
}

// PlayAIJournalingRequest 构造 `POST https://m.mcloud.139.com/ycloud/visitlog/journaling`。
//
// 与 ①…④ 同一套头（[playAIH5Headers]：不带 `activityid`/`appversion`/`deviceid`），
// 只是 `content-type` **先剥后补**成 [PlayAIJournalingContentType]
// （[playAIH5Headers] 见 body 非空会补 JSON 那一个，那是给 ①…④ 的 body 定的）。
//
// `jwt` 必填：埋点也在 `/ycloud` 域内，A 档那条实拍**带票**（`WithLiveRoomHeaders` 那一族同样带）。
// 不带票会不会被判成匿名访问 ⇒ 未测试。
func PlayAIJournalingRequest(optkeyword, sourceID, jwt, referer, showLoading string) core.HTTPRequest {
	request := core.NewRequestPost(fmt.Sprintf("%s/ycloud/visitlog/journaling", BaseMcloud))
	request.Body = core.Body(PlayAIJournalingBody(optkeyword, sourceID))
	request = playAIH5Headers(request, referer, jwt, showLoading)
	request.Headers = dropHeader(request.Headers, "content-type")
	return request.WithHeader("Content-Type", PlayAIJournalingContentType)
}

// ParsePlayAIJournaling 判一条埋点回没回"认账"。
//
// # 它的判据**不是这一页的**
//
// A 档：同端点在直播小红花族实拍到 `{"code":0,"msg":"success"}`。这一页零样本。
// 于是这里只钉得住两件事：`code` 是数字且为 0 ⇒ 交回 nil；非 0 ⇒ 把码与上游原文带出来
// （**不**折成成功）。
//
// ⚠ 旧 TS 把这一步当**门槛**（`task.ts:89-99`、`:115-125` 两处 `if (!journalOk) return false`，
// 上报失败 ⇒ 整件任务放弃，连 `step/click` 都不发）。**要不要同样当门槛是 app 层的决定**，
// 本层只交结论；两种选择的代价都不为零（当门槛：埋点一挂整件任务不做；不当：可能白做后面的写请求）。
func ParsePlayAIJournaling(envelope *Envelope) *core.Error {
	_, _, perr := playAINewyearPayload(envelope)
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 丙·chat/v2/add：**只构造请求**（响应是 SSE，零样本 ⇒ 不猜形状）
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIChatParams 是 `chat/v2/add` 里**必须由调用方给**的五格。
//
// 其余每一格都是常量（见上面的 `PlayAIChat*`）。为什么把入参收得这么紧：
// 这一条**整段**都是 B 档，可自由填的口子越多，能发出"出处里不存在的那一条请求"的面就越大。
type PlayAIChatParams struct {
	// UserID 是 body 的 `userId`（旧 TS 取 `ctx.state.userDomainId`，`task.ts:174`）。
	//
	// 与 ⑤ 同一口径：它不是凭据，**是**账号标识，填错等于以别人的身份问一句
	// （[PlayAIRecognizeParams.UserID] 那条）。测试里一律用 `199…` 合成号。
	UserID string
	// Dialogue 是 `dialogueInput.dialogue`（问出去的那句话）。
	//
	// 旧 TS 的取法是 `recommendList[0]`，为空则 `"这是什么"`（`task.ts:169`）。
	// ⇒ 那是 app 层的**选择**，本层不内置兜底串（内置就是替这一族发明一条没人对拍过的默认值）。
	Dialogue string
	// InputTime 是 `dialogueInput.inputTime`（**字符串**，形如
	// `2026-06-29T12:10:43.123+08:00`；见 [PlayAIChatInputTime]）。
	//
	// 由调用方给而不是内部取时钟：本层所有构造函数都得是**纯函数**才能逐字段断言。
	InputTime string
	// FileID 是 `attachment.fileList[0].fileId` —— ⑤ 的 `data.fileId`。
	//
	// 也就是"刚被 `uploadToCloud` 落进云盘的那张图"。两口的耦合就在这一个字段上：
	// ⑤ 的注释说它要交给"上传/清理"那一族，这里则是它的另一个消费者。
	FileID string
	// FileName 是 `attachment.fileList[0].name`。
	//
	// ⚠ 旧 TS 那一格是 `parseInt(String(taskId), 10) + 1` 再拼 `.jpeg`（`task.ts:196`）。
	// 这是 JS 的数值语义：19 位的 `taskId` 过一遍 double 会**丢末几位**
	// （末三位会被抹成 `…000` 那一类，`…005` 变 `…000`）。
	// ⇒ 本层**不复刻**那段算术，也不提供"从 taskId 推导文件名"的函数：
	//   - 复刻 = 把一次精度丢失写成"我们的实现"，而这一格连实拍都没有；
	//   - 它只是**文件名**（不参与任何判据的证据链），所以正确做法是调用方给什么就发什么。
	// 要判断它有没有影响：抓到一条本页 chat 的实拍，比对 `name` 与 ⑤ 的 `taskId`。
	FileName string
}

// PlayAIChatInputTime 按旧 TS 的 `formatTime`（`task.ts:72-76`）把一个时刻摊成那串文本：
//
//	YYYY-MM-DDTHH:mm:ss.SSS+08:00
//
// ⚠⚠ 它取的是**挂钟分量**，而 `+08:00` 是**字面量后缀** —— 这正是旧 JS 的语义
// （`getFullYear()/getHours()` 用的是运行环境的本地分量，后缀却是硬编码的 `+08:00`）。
// 所以传一个 UTC 时刻进来，得到的是"那个 UTC 挂钟值 + 假装在东八区"。
//
// 为什么不"修正"成 `t.In(+08:00)`：那是替上游宣布"它比对时区"，而这一格**零实拍**
// （旧 TS 在本地时区就是东八区的环境里跑，两种写法**在它的运行环境里逐字节相同**）。
// ⇒ 由调用方决定给哪个挂钟；本层的义务只有一条：**同样的入参出同样的串**（可逐字段断言）。
func PlayAIChatInputTime(at time.Time) string {
	return fmt.Sprintf("%04d-%02d-%02dT%02d:%02d:%02d.%03d+08:00",
		at.Year(), int(at.Month()), at.Day(), at.Hour(), at.Minute(), at.Second(),
		at.Nanosecond()/int(time.Millisecond))
}

// PlayAIChatBody 拼 `chat/v2/add` 的请求体（**逐字段照旧 TS `task.ts:170-208`**）。
//
// 三处形状容易被"顺手整理"掉，都在测试里按字节钉着：
//   - `sessionId` 与 `applicationId` 是**空串**（不是缺席、不是 null）：旧 TS 每次都给空串；
//   - `extInfo` 是字符串 `"{}"`，`sortInfo` 是**对象** `{}` —— 两格长得不一样不是笔误；
//   - `attachmentTypeList` 是 `[3]`（数组里一个数字），`fileList` 是**一个元素**的数组。
func PlayAIChatBody(params PlayAIChatParams) map[string]any {
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
			"versionInfo":                     map[string]any{"h5Version": PlayAIChatH5Version},
			"extInfo":                         PlayAIChatExtInfo,
			"sortInfo":                        map[string]any{},
			"command": map[string]any{
				"command":    PlayAIChatCommand,
				"subCommand": PlayAIChatSubCommand,
			},
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

// PlayAIChatRequest 构造 `POST https://ai.yun.139.com/api/outer/assistant/chat/v2/add`。
//
// 头集合**沿用 ⑤ 那一份**（[playAIAppHeaders]：[WithAppHeaders] 的公共设备头 +
// 不带 charset 的 `application/json` + `Origin: mcloudlocal://yun.139.com` + `x-yun-tid` +
// `Authorization`），只把 `Accept` 换成 `text/event-stream`（先剥后补）。
// 与 ⑤ 一致的理由：两条都是**同一个原生侧 AI 服务**的接口，旧 TS 给它们的两组头
// 本来就**只差 `Accept` 与 `x-yun-client-info`/`x-yun-app-channel` 三枚**。
//
// 三处已知偏离，都登记在这里（都在测试里点名，都不是本层的判据）：
//  1. `x-yun-client-info` / `x-DeviceInfo`：旧 TS 写死了一串 **iPhone** 形态
//     （`task.ts:41-42`、`:202`），而本仓 ⑤ 已经决定"三处设备画像互相矛盾 ⇒ 沿用仓库那**一套**、
//     不在这一页混第三种"（见 [PlayAIRecognizeRequest] 第 2 条与盘点文档 §5.6，**未实测**）。
//     这一条跟着 ⑤ 走同一个 [AppProfile]：两条 AI 请求的设备串**互相一致**，
//     但**与旧 TS 那两串不一致**。要核它：抓一条未脱敏的同接口请求比对设备串。
//  2. `x-yun-app-channel`：旧 TS 这一条给 `"101"`（与 body 的 `sourceChannel` 同值），
//     而 [AppProfile] 给的是 [IOSChannel]（`10000020`）。这里**同样按仓库那一套**（同一处理由：
//     画像与渠道号在旧 TS 里是**一组**互相矛盾的值，拆开只会有第四种组合）。
//     ⇒ 这一处是本文件里我认为**最可能在真打时先出问题**的一格，复核顺序排第一。
//  3. `content-type`：旧 TS 没写（ofetch 对对象 body 会补一个 `application/json`），
//     本层与 ⑤ 同选 [PlayAIPlainJSONContentType]。**这一条零实拍**，是**我们的选择**。
func PlayAIChatRequest(
	profile *AppProfile, timestamp int64, authorization, tid string, params PlayAIChatParams,
) (core.HTTPRequest, *core.Error) {
	// 五格必填。护栏与 585 那一套**共用一个函数**（见 [playAIChatParamGuard]）：
	// 两页的这五格同名同义，两处各写一遍只会让"改一处忘一处"变成可能。
	if perr := playAIChatParamGuard(params); perr != nil {
		return core.HTTPRequest{}, perr
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/api/outer/assistant/chat/v2/add", PlayAIHostAI)).
		WithJSONBody(PlayAIChatBody(params))
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = playAIAppHeaders(request, profile, timestamp, authorization, tid)
	request.Headers = dropHeader(request.Headers, "accept")
	return request.WithHeader("Accept", PlayAIChatSSEAccept), nil
}

// playAIChatParamGuard 钉住 chat 两套（拍拍 / 585）共用的那五格必填。
//
// 为什么单独一个函数：五格在两页**同名同义**（`userId` / `dialogue` / `inputTime` /
// `fileId` / `name`），两处各写一遍 switch 的下一次演变一定是"只改了一处"——
// 而这类偏差的表现是一条空值请求安静地发出去。
// 护栏的理由同 [PlayAIRecognizeRequest]：少一格就是发出一条出处里不存在
// 的请求，而 `userId` 那一条的后果是"以别人的账号去问"。
func playAIChatParamGuard(params PlayAIChatParams) *core.Error {
	switch {
	case params.UserID == "":
		return core.NewBadRequest("chat/v2/add 的 userId 不能为空（填错会以别人的账号提问）")
	case params.Dialogue == "":
		return core.NewBadRequest("chat/v2/add 的 dialogue 不能为空（兜底串属 app 层，本层不内置）")
	case params.InputTime == "":
		return core.NewBadRequest("chat/v2/add 的 inputTime 不能为空（两套房两种格式：" +
			"见 PlayAIChatInputTime 与 PlayAIChatCameraInputTime）")
	case params.FileID == "":
		return core.NewBadRequest("chat/v2/add 的 fileId 不能为空（它来自 aiRecognize 的 data.fileId）")
	case params.FileName == "":
		return core.NewBadRequest("chat/v2/add 的 name 不能为空（旧 TS 那一格永不为空）")
	}
	return nil
}

// PlayAIChatSSEAccept 是 `chat/v2/add` 的 `Accept`（旧 TS `task.ts:204`）。
//
// ⚠ 它同时是本层"响应是流式文本、不是一段 JSON"的**唯一**成文标记：
// 看到 [ParseEnvelope] 出现在这条链上就是错的（本文件因此**不解析**这一条，
// 也不给**拍拍这一套**提供发送方法）。
// 585 那一套的读法、判据与发送口在 [`app_play_ai_chat_sse.go`](app_play_ai_chat_sse.go)
// —— 那里解释了"为什么这一族可以从零样本起步、由真跑补第一段读数"。
const PlayAIChatSSEAccept = "text/event-stream"

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
//
// 三条**各自只发一条**：换票的两步**不串成一条链**（先换票再建会话是 app 层的编排，
// 理由与 `app_play_ai.go` 文件头那句"该不该发 ③④ 由调用方决定"同一条），
// chat 那一条**不在这里**（响应 SSE，零样本 ⇒ 见文件头）。
// ─────────────────────────────────────────────────────────────────────────────

// PlayAIQuerySpecToken 发换票第①步，给出 SSO token。只读（GET）。
func (u *Upstream) PlayAIQuerySpecToken(
	ctx context.Context, sign H5Signature, referer, showLoading string,
) (string, *core.Error) {
	envelope, perr := u.Call(ctx, PlayAIQuerySpecTokenRequest(sign, referer, showLoading))
	if perr != nil {
		return "", perr
	}
	return ParsePlayAIQuerySpecToken(envelope)
}

// PlayAITyrzLogin 发换票第②步，给出这一页自己的会话。
//
// ⚠ 这一步**写**在服务端建了一条会话（同时下发 cookie 与票），但它可重放：
// 每次调用换一张新票，不会消耗额度（这一句的**依据**是签到族同端点的既有行为，
// 这一页零实拍 ⇒ 仍属未测试）。仍按"传输层不自动重试"处理。
func (u *Upstream) PlayAITyrzLogin(
	ctx context.Context, sign H5Signature, referer, ssoToken, showLoading string,
) (PlayAISession, *core.Error) {
	request, perr := PlayAITyrzLoginRequest(sign, referer, ssoToken, showLoading)
	if perr != nil {
		return PlayAISession{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return PlayAISession{}, perr
	}
	return ParsePlayAITyrzLogin(envelope)
}

// PlayAIJournaling 发一条埋点。
//
// 一次调用 = 一条埋点事件（旧 TS 那几处循环是**多条事件**，循环归调用方）。
func (u *Upstream) PlayAIJournaling(
	ctx context.Context, optkeyword, sourceID, jwt, referer, showLoading string,
) *core.Error {
	envelope, perr := u.Call(ctx,
		PlayAIJournalingRequest(optkeyword, sourceID, jwt, referer, showLoading))
	if perr != nil {
		return perr
	}
	return ParsePlayAIJournaling(envelope)
}
