// 玩法 **1028「AI 工作台」**那一族 —— 四枪、跨三台主机，而它的凭据是**从邮箱会话派生出来的**。
//
//	① POST html5.mail.10086.cn/mw2/file/disk?func=disk:queryUserToken                    **只读**
//	                                                                    → DES 密文的 `account` + 一枚 `token`
//	② POST m.mcloud.139.com/ycloud/signin/public/getUserTaskAndCloudStatusExternal       **只读**
//	                                                                    body 里那枚 `token` 是 **RSA 密文**（明文 = 手机号）
//	③ POST ai.yun.139.com/api/outer/assistant/chat/v2/add                               **写**：一次对话 → 一段 SSE
//	④ 再打一次 ② —— 旧实现拿它打一行 debug，**不改变任何判定**（本层因此把它做成"纯观察"）
//
// 编排（门槛、播报、那一格配置的默认值）在
// [`internal/app/task_matrix_workbench.go`](../app/task_matrix_workbench.go)；
// 本文件只管**形状与读法**。
//
// # ⚠ ③ 与 585 那一发是**同一个 URL、另一套头与体**，本文件刻意不复用 [`app_play_ai_chat_sse.go`](app_play_ai_chat_sse.go)
//
//	585   头：原生设备那一套（`x-DeviceInfo` / `x-yun-uni` / `x-yun-tid` / `Origin`）+ `Accept: text/event-stream`
//	      体：`applicationType/sessionId/sourceChannel/userId/dialogueInput`，`dialogueInput.attachment` **带一个文件**
//	1028  头：**只有** `x-yun-api-version` / `x-yun-client-info` / `x-yun-app-channel` / `Authorization` /
//	      `Content-Type` / `Accept` / 一枚**桌面** UA（`ai-workbench.ts:196-207` 逐字）—— 没有 `Origin`、
//	      没有 `x-yun-tid`、没有设备串
//	      体：外面那一层多 `continuationInfo:null`，`dialogueInput` 是**另一组键**
//	      （`commands` / `command` / `resourceType` / `dialogueType` / `commandType` / `aiWritingSetting`…），
//	      `attachment` 是**空对象**
//
// ⇒ 这正是 AGENTS.md §1.12 事实②那句"URL 相同而参数不同"的极端形态：**URL 对上完全不等于同一条请求**。
// 两套的 `inputTime` 也**不同形**（585 是 UTC + 字面量 `Z`，见
// [PlayAIChatCameraInputTime]；这一套是**本地挂钟分量**、无偏移后缀，见 [AIWorkbenchInputTime]）
// ⇒ 谁也不能顶替谁，"顺手合并成一个 body 构造器"会把两条都打断。
//
// # ⚠⚠ 这一族的 SSE **累加器与 585 的三档读法不是一回事**，别搬结论
//
// 585 那边实拍过：内容在**逐行累加**的 `data.flowResult.outContent`，而 `data.outContent`
// 在两条实拍里**逐行皆空**。那个读数**只属于 585 的那一次会话**，不能拿来解释 1028：
//
//	585  只读 `data:` 前缀的行；倒序取**最后一条**判成败；不解的行**不**忽略（有三档回退）
//	1028 逐行**正序**走；**不带 `data:` 前缀的行照样 `JSON.parse`**（`ai-workbench.ts:207-210`）；
//	     `[DONE]` 与空串只是 `continue`（**不是终止**）；解不开的行 `catch{}` **忽略**；
//	     `flowResult.outContent` 与 `data.outContent` **两条路径都累加进同一个串**（`:233-234`）；
//	     不读 `finishReason`；末尾那半行（无换行收尾）**丢弃**（它的 buffer 从不 flush）
//
// ⇒ 本文件的 [ParseAIWorkbenchSSE] 按上面这一套逐字落，测试里专门钉"它与 [ParsePlayAIChatSSE]
// 在同一段样本上给出**不同**读数"。
//
// # 证据面：**四枪都已有真实读数**（2026-09-26 活体探针三趟）
//
// 落码那天（同日凌晨）按 `queryUserToken` 与 `getUserTaskAndCloudStatusExternal` 两个串
// 逐份扫过 `har/`：**零命中** ⇒ 四枪的形状**全部**取自旧 TS
// `packages/core/src/apis/ai-workbench.ts` 与 `tasks/app/runners.ts:345-448`。
// 随后按 AGENTS.md §1.13"缺口的默认处置是去跑"打了三趟探针（通路 D，
// `internal/app/live_probe_workbench_test.go`），逐枪读数与复跑命令登记在
// [`docs/capture/app-ai-workbench-samples.txt`](../../docs/capture/app-ai-workbench-samples.txt)。
// 对本文件要紧的是这四格：
//
//   - **① 的 `var.data.data` 里确实有 `account` / `token` / `userDomainId`** 那三格
//     （长度 24 / 201 / 19）⇒ [ParseAIWorkbenchUserToken] 那份"按旧 TS 文字形状"写的
//     路径**与真实体对得上**；解出来是 11 位、形如手机号。
//   - **② 不带鉴权、身份全靠 body 里那枚 RSA ⇒ 上游认**：同一口用 585 / 106 / 1020 三个
//     `taskId` 都回 `code:"0" / msg:"success" / result{…}` ⇒ 这一口的自证身份是**可用形状**，
//     不是"我们猜的"。而 1028 自己与一枚造出来的 `999999` 都回 `code:"501" / msg:"任务不存在"`
//     ⇒ 那句"任务不存在"按 **taskId** 给，路由与身份都活着。
//   - **⚠ `code` / `taskStatus` / `cloudStatus` 真实回的是字符串**（`"0"` / `"1"`）
//     ⇒ [ParseAIWorkbenchTaskStatus] 的 `EnvelopeOK` 与 `GatePass` 那两格按**严格数字**判，
//     在真实读数下**永不为真**。这不是我们的解析缺陷：旧实现那两句也是 JS 严格相等
//     （`cs?.code === 0` / `taskStatus === 1`），在同一份体上**同样**不成立
//     ⇒ 本层不放宽（放宽等于把"少发一次 ③ 这个写"交给我们自己猜），
//     取舍登记在 `docs/pending-decisions.md` 本族那一格。
//   - **③ 的 SSE 本层读得出内容**：三趟分别 69 / 66 / 52 行，[ParseAIWorkbenchSSE] 累出的
//     那句三趟**互不相同**（模型原话）⇒ "正序 + 两格 `outContent` 同累一个串"这套读法
//     **至少不是空转**。⚠ 但它**不等于**"这一套比 585 那三档更准"：探针打的是**我们自己的
//     读数**（事件数 / 末条 code / 内容），**原始 SSE 文本没有留存**（③ 是写，本趟只发一次，
//     没有再发一档"直发看体"）⇒ 两套读法在**同一份真实体**上的对照仍是缺口，
//     怎么补成文在 `docs/pending-decisions.md`。
//
// ⚠ 探针**没有**留存原始响应体（值级脱敏 + 不落盘），可复现的是那条复跑命令与本文件的形状表。
//
// # 凭据边界：这一族的**请求侧**一个值都不许出文本
//
// ① 带邮箱会话（`sid` 在 URL、`rmkey` 在 `Cookie`）；② 的 body 装 RSA 密文、明文是手机号；
// ③ 带 `Authorization: Basic base64(mobile:<手机号>:<token>)` **整串**。
// ⇒ DES 密文 `account`、解出来的**手机号**、RSA 密文、Basic 整串、`token`
// **一律不进**错误文本 / 日志 / 夹具真值；三口的错误全部过 [redactError]，
// 手机号与 token 也当秘密传进去（[core.Error] 的文本可能来自上游 echo）。
// 可截断后进日志的只有 ③ 的**响应体**（那是模型输出的那一句，不含凭据），口径与 585 同族。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// 这一族自己的字面量（逐字出处 `apis/ai-workbench.ts`）。
const (
	// AIWorkbenchDESKey 是 ① 那一枚 `account` 的 DES 密钥（`runners.ts:437` 逐字）。
	//
	// ⚠ 它是**硬编码在参考实现里**的一枚固定 key，不是任何凭据：值本身进成文没有泄露面
	// （任何人读得到旧 TS 就读得到它），但**它解出来的明文**是手机号 ⇒ 明文才是边界。
	// 为什么不做成参数：本仓只有这一个消费者，给自由口子等于让人去猜"还有别的 key 吗"。
	AIWorkbenchDESKey = "139_mail"

	// AIWorkbenchSourceChannel 是 ②③ 共同的 `sourceChannel`（`runners.ts:351` 逐字 `"10175"`）。
	//
	// ⚠ 它与 585 那一族的 [PlayAIChatSourceChannel] **不同值**（那一枚是 AI 相机自己的渠道号）
	// ⇒ 不许互搬（AGENTS.md §1.12：跨端跨发起方的常量各按各的出处）。
	AIWorkbenchSourceChannel = "10175"

	// AIWorkbenchTaskID 是 ② 那一发的 `taskId`，也就是本玩法自己的码（`runners.ts:364` 传 `1028`）。
	AIWorkbenchTaskID int64 = 1028

	// AIWorkbenchH5Version / AIWorkbenchPCVersion 是 ③ 的 `dialogueInput.versionInfo`
	// 那两枚（`ai-workbench.ts` 的 `buildDialogueInput` 逐字 `h5:"2.5.0"` / `pc:"1.7.0"`）。
	AIWorkbenchH5Version = "2.5.0"
	AIWorkbenchPCVersion = "1.7.0"

	// AIWorkbenchDesktopUserAgent 是 ②③ 两发共用的 UA（`ai-workbench.ts` 里那两处于字面量
	// 逐字相同的一枚 Windows Chrome 120）。
	//
	// ⚠ 它**不是** [mailAndroidWebViewUserAgent]（① 用那一枚），也不是本仓 App 那套 iOS UA：
	// 同一族里三发用三枚 UA 是旧实现的**原样**，照抄，不"统一"。
	AIWorkbenchDesktopUserAgent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) " +
		"AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

	// AIWorkbenchStatusPath 是 ② 的路径（挂在 [BaseMcloudSignin] 末尾：`/public/…`）。
	AIWorkbenchStatusPath = "/public/getUserTaskAndCloudStatusExternal"

	// AIWorkbenchChatTimeoutMS 是 ③ 的整发上限（`runners.ts:352` 的 `timeoutSec = 60`）。
	//
	// ⚠ 一处**与出处的差别**要知道：旧实现那枚 `AbortController` 掐的是**流**
	// （60s 到就把已收到的部分算数），本仓 [core.HTTPRequest.TimeoutMS] 走的是
	// `context.WithTimeout`（`internal/io/client.go:177`），掐的是**整发请求**⇒ 超时会**丢掉**
	// 已经流过来的内容并交回一句 Timeout。后果只有一格：一段回答超过 60 秒时旧实现有读数、
	// 本仓没有。要改成"边读边累加 + 到点截断"得动传输层 ⇒ **未做**，登记在
	// `docs/pending-decisions.md` 本族那一格。
	AIWorkbenchChatTimeoutMS uint64 = 60_000
)

// AIWorkbenchStatusPublicKeyB64 是 ② 那枚 RSA 公钥（SPKI DER 的 base64，2048 位）。
//
// 它是旧实现 `ai-workbench.ts:9-19` 那块 PEM **剥掉 armored 行与换行**之后的同一串
// （那边 `runners.ts:361-362` 就是用两个 `replace` 现场剥的），**公开值** ⇒ 可进代码，
// 成文里不整块搬运（`docs/sanitize-guide.md`）。与 [core.SklandPublicKeyB64] 同一取向：
// 存 base64 主体、由 [core.RSAPKCS1v15EncryptBase64WithB64Key] 自己解 DER。
const AIWorkbenchStatusPublicKeyB64 = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA1PWBlhoygGhj2uqniCjI" +
	"PXbyufSSeST1yojxn1w1G6E/Uc9OpOFcXqcZYb1uc111zYdBAupYOxG+PXsQoZnZ" +
	"TXiZfYO6oTBC623OzEPnE3afYGa62E/8g8C4Zrzaqng4Sw+YvERoroHRM29SRvCB" +
	"QK7qcszK2kGLuPVh02WhHuprvR6GQPWcOOXLYOUbg2qVhKu+J1+bJ2wS4camxNF2" +
	"W96YSTdWPysHSw0o3fPIOfkDylCmj4odAVIln5vBP/rYtKeFskco+Tpyd5UM+zpw" +
	"Rf+VOJ/c/7DIC2iUjsENdb2pasLlhp5mTDeARDK4lzQR+MkJaGzYis3NqSu2PAPJ" +
	"TwIDAQAB"

// ─────────────────────────────────────────────────────────────────────────────
// 第 ① 枪：queryUserToken（**只读**）+ DES 派生
// ─────────────────────────────────────────────────────────────────────────────

// AIWorkbenchDomainAuth 是 ① 派生出来的那一枚"AI 域凭据"。
//
// 四格里三格是秘密：[AIWorkbenchDomainAuth.Phone] 是 DES 解出来的手机号，
// [AIWorkbenchDomainAuth.Token] 是 ① 回的那枚裸 token，
// [AIWorkbenchDomainAuth.Authorization] 是 `"Basic " + base64("mobile:<手机号>:<token>")` 整串
// —— 它由前一族的 [AppAuthorization] 构造，与 App 登录那两枚**同形不同源**
// （那两枚来自登录响应，这一枚来自 ①），所以别把两边的值互相顶替。
//
// ⚠ [AIWorkbenchDomainAuth.UserDomainID] **不是**秘密（它是 ③ body 里的 `userId`，
// 与库里那份 `userDomainId` 同族），但它的**来源不同**：这一格是 ① 回的，
// 而 585 那一族用的是库里那份 ⇒ 见 `docs/pending-decisions.md` 本族那一格（"两份是否恒等"未测试）。
type AIWorkbenchDomainAuth struct {
	Phone         core.SecretValue
	Token         core.SecretValue
	Authorization core.SecretValue
	UserDomainID  string
}

// OK 报告"这一份能不能拿去发 ②"（= 有手机号）。
//
// ⚠ 它**不**要求 `UserDomainID` 非空：旧实现的顺序就是先拿手机号打 ②（查前置状态），
// 到需要对话时才检查 `userDomainId`（`runners.ts:390-393`）。把两格揉成一个判据
// 会让"② 打得出去、③ 缺 userId"这一档在界面上变成"什么都没做"。
func (a AIWorkbenchDomainAuth) OK() bool { return a.Phone.Value() != "" }

// AIWorkbenchQueryUserTokenRequest 构造 `POST …/mw2/file/disk?func=disk:queryUserToken&…`。
//
// URL 与头集合与 ⑤ `surplusNum`（[MailSurplusNumRequest]）**逐字相同**，只有 `func` 与体不同：
// 体是**空的** `<object></object>`（旧实现那一句模板字符串里就是这四个字节，
// ⚠ 而**不是** ⑤ 那一份带缩进的三键体 ⇒ 别复用 [mailDiskAppFields]）。
// 之所以两处相同还要各写一个构造函数：`func` 不同就是**另一口接口**，
// 把两口的"恰好相同"编码成一个共享构造器，将来任一侧要改都会牵连另一侧。
func AIWorkbenchQueryUserTokenRequest(session MailSession, cguid, rnd string) (core.HTTPRequest, *core.Error) {
	query, perr := mailDiskQuery("disk:queryUserToken", session, cguid, rnd)
	if perr != nil {
		return core.HTTPRequest{}, perr
	}
	request := core.NewRequestPost(query).
		WithHeader("content-type", mailXMLContentTypeTight).
		WithHeader("accept", "*/*").
		WithHeader("cookie", mailDiskSessionCookie(session)).
		WithHeader("origin", mailHTML5Host).
		WithHeader("referer", mailHTML5Host+"/").
		WithHeader("user-agent", mailAndroidWebViewUserAgent).
		WithHeader("x-requested-with", mailAppMark)
	request.Body = core.Body(mailObjectOpen + mailObjectClose)
	return request, nil
}

// ParseAIWorkbenchUserToken 读 ① 并**当场**做 DES 派生，交回那一枚 AI 域凭据。
//
// 判据逐字按旧实现 `runners.ts:430-443` 的 `deriveAiDomainAuth`：
//   - `code !== "S_OK"` ⇒ 失败（与资产侧同一枚字面量，复用 [mailAssetOKCode]）；
//   - `var.data.data` 里 `account` 或 `token` 缺任一 ⇒ 失败；
//   - DES 解密抛错 ⇒ 失败（旧实现整个函数裹在 `try{}catch{}` 里，**一句日志都不打**；
//     ⚠ 本仓不跟这一格：静默中止正是本轮反复批评的那类行为，三种失败各给一句人话）。
//
// 响应走 [parseMailAssetEnvelope] ⇒ 单引号那套宽松解法在这里是**有出处**的
// （旧实现 `ai-workbench.ts` 的 `parseLooseJson` 与资产侧同一族、同一写法）。
//
// ⚠ 失败文本**不 echo 响应体**：这一口成功时回的就是加密的手机号与 token，
// 失败时回什么零样本 ⇒ 只念 `code` 与截断后的 `summary`（与 [ParseMailSurplusNum] 同一处置）。
func ParseAIWorkbenchUserToken(response core.HTTPResponse) (AIWorkbenchDomainAuth, *core.Error) {
	value, perr := parseMailAssetEnvelope(response, "AI 工作台凭据派生")
	if perr != nil {
		return AIWorkbenchDomainAuth{}, perr
	}
	root, _ := value.(map[string]any)
	code, _ := root["code"].(string)
	summary, _ := root["summary"].(string)
	if code != mailAssetOKCode {
		return AIWorkbenchDomainAuth{}, core.NewBadRequest(fmt.Sprintf(
			"AI 工作台凭据派生（①）上游没认：code=%s%s", codeOrUnknown(code), assetSummaryHint(summary)))
	}
	// `var.data.data` 那三层（旧实现 `r?.var?.data?.data`）：任一层不是对象 ⇒ 当成"没回"，
	// 不 panic 也不猜形状。
	first, _ := root["var"].(map[string]any)
	second, _ := first["data"].(map[string]any)
	inner, _ := second["data"].(map[string]any)
	account, _ := inner["account"].(string)
	token, _ := inner["token"].(string)
	userDomainID, _ := inner["userDomainId"].(string)
	if account == "" || token == "" {
		return AIWorkbenchDomainAuth{}, core.NewParse(fmt.Sprintf(
			"AI 工作台凭据派生（①）说成功却没回 account 或 token（缺 account=%v 缺 token=%v）"+
				"⇒ 不猜形状，②③④ 都不发",
			account == "", token == ""))
	}
	phone, derr := core.DESECBPKCS7Decrypt(AIWorkbenchDESKey, account)
	if derr != nil {
		// ⚠ 不 echo `account`：它是 DES 密文，解开的明文是手机号 ⇒ 密文也不进日志。
		return AIWorkbenchDomainAuth{}, core.NewParse(
			"AI 工作台凭据派生（①）的 account 解不开（DES-ECB/PKCS7）：" + derr.Error() +
				" ⇒ ②③④ 都不发")
	}
	if phone == "" {
		return AIWorkbenchDomainAuth{}, core.NewParse(
			"AI 工作台凭据派生（①）的 account 解出来是空串 ⇒ ②③④ 都不发")
	}
	return AIWorkbenchDomainAuth{
		Phone:         core.NewSecretValue(phone),
		Token:         core.NewSecretValue(token),
		Authorization: core.NewSecretValue(AppAuthorization(phone, token)),
		UserDomainID:  userDomainID,
	}, nil
}

// AppAIWorkbenchUserToken 发第 ① 枪。**只读**。
func (u *Upstream) AppAIWorkbenchUserToken(
	ctx context.Context, session MailSession, cguid, rnd string,
) (AIWorkbenchDomainAuth, *core.Error) {
	request, perr := AIWorkbenchQueryUserTokenRequest(session, cguid, rnd)
	if perr != nil {
		return AIWorkbenchDomainAuth{}, perr
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return AIWorkbenchDomainAuth{}, redactError(perr, session.SID, session.RMKey)
	}
	auth, perr := ParseAIWorkbenchUserToken(response)
	return auth, redactError(perr, session.SID, session.RMKey)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ②④ 枪：getUserTaskAndCloudStatusExternal（**只读**，两遍同一个构造函数）
// ─────────────────────────────────────────────────────────────────────────────

// AIWorkbenchTaskStatus 是 ②④ 的读数。
//
// 五格里三格是**给播报用**的原样读数（`code` 与两枚状态），另两格是判据：
//   - [AIWorkbenchTaskStatus.EnvelopeOK] = 旧实现那句 `cs?.code === 0 && cs.result`
//     （**严格**：`code` 得是数字 `0`，字符串 `"0"` 在它那一层不成立）；
//   - [AIWorkbenchTaskStatus.GatePass] = 再叠上 `taskStatus === 1 && cloudStatus === 0`。
//
// 为什么要 `EnvelopeOK` 与 `GatePass` 两格而不是一个布尔：旧实现是
// "拿不到状态就**照发对话**"（`runners.ts:363-376` 那个 `needChat` 初值是 true，
// 只有门成立才置 false）⇒ 播报要能分清"上游说不用问"与"上游没说，所以问了"。
// 少这一格，一次 ② 失败会被念成"前置已满足、跳过对话"，那是本族最贵的一种误读。
type AIWorkbenchTaskStatus struct {
	CodeRaw    string
	EnvelopeOK bool
	// HasResult = 后一半那句 `cs.result` 真值（⚠ `{}` 在 JS 里也算真）。
	HasResult   bool
	TaskStatus  string
	CloudStatus string
	GatePass    bool
}

// AIWorkbenchStatusRequest 构造 ②（④ 与它**同一个构造函数**）。
//
// body 的 `token` 是 `[手机号](#AIWorkbenchDomainAuth.Phone)` 的 RSA PKCS#1 v1.5 密文
// （base64）。⚠ 随机源**由调用方注入**：PKCS#1 v1.5 的填充含随机字节 ⇒ 同一枚手机号
// 每次密文都不同，这是**特性不是缺陷**（也正因此这一发的 body 不能进黄金向量，
// 夹具测试钉的是"body 里只有那一枚密文、没有手机号明文、两次不同且长 256 字节"
// —— ⚠ **解不回手机号**：那需要私钥，而本仓只有公钥，这一格**测不了**，
// 只能等真跑看上游回什么）。
//
// 这一发**不带任何鉴权头**：旧实现那三条就是 `content-type` / `Origin` / `Referer` / UA
// （`ai-workbench.ts` 的 `getCloudTaskStatus`）—— 它的"身份"全在 body 那一枚密文里。
// ⚠ 所以别"顺手"给它补 `Authorization`：那会变成另一条请求。
func AIWorkbenchStatusRequest(
	phone core.SecretValue, taskID int64, sourceChannel string, rng core.Random,
) (core.HTTPRequest, *core.Error) {
	if phone.Value() == "" {
		return core.HTTPRequest{}, core.NewInternal(
			"AI 工作台状态查询缺手机号：RSA 那一发**只有**这一枚输入，空串等于让上游猜是谁")
	}
	if rng == nil {
		return core.HTTPRequest{}, core.NewInternal(
			"AI 工作台状态查询缺随机源：RSA PKCS#1 v1.5 的填充要随机，本层不自取随机数")
	}
	if taskID <= 0 {
		return core.HTTPRequest{}, core.NewInternal(fmt.Sprintf(
			"AI 工作台状态查询的 taskId 非法（%d）⇒ 不发（这一格是它查的那一条玩法，0 没有意义）", taskID))
	}
	if sourceChannel == "" {
		return core.HTTPRequest{}, core.NewInternal("AI 工作台状态查询缺 sourceChannel")
	}
	cipher, err := core.RSAPKCS1v15EncryptBase64WithB64Key(
		AIWorkbenchStatusPublicKeyB64, []byte(phone.Value()), rng)
	if err != nil {
		return core.HTTPRequest{}, asCoreError(err, "AI 工作台状态查询的 token 加密失败")
	}
	request, jerr := core.NewRequestPost(BaseMcloudSignin + AIWorkbenchStatusPath).
		WithJSONBody(map[string]any{
			"token":         cipher,
			"taskId":        taskID,
			"sourceChannel": sourceChannel,
		})
	if jerr != nil {
		return request, asCoreError(jerr, "序列化 AI 工作台状态查询的请求体失败")
	}
	return request.
		WithHeader("origin", mailAppMailHost).
		WithHeader("referer", mailAppMailHost+"/").
		WithHeader("user-agent", AIWorkbenchDesktopUserAgent), nil
}

// ParseAIWorkbenchTaskStatus 读 ②④ 的响应（**不判 HTTP 状态**，那是发送层的事）。
//
// 解不开体才算失败；`code` 不是 0、没有 `result` 都只是**读数**（旧实现把
// "拿不到状态"处理成"照样问一句"，见 [AIWorkbenchTaskStatus]）。
func ParseAIWorkbenchTaskStatus(response core.HTTPResponse) (AIWorkbenchTaskStatus, *core.Error) {
	value, ok := parseMailJSONText(string(response.Body))
	if !ok {
		return AIWorkbenchTaskStatus{}, core.NewParse(fmt.Sprintf(
			"AI 工作台状态查询响应解不开（%d 字节，状态 %d）⇒ 这一口的失败形状零样本，先看状态码",
			len(response.Body), response.Status))
	}
	root, isObj := value.(map[string]any)
	if !isObj {
		return AIWorkbenchTaskStatus{}, core.NewParse(fmt.Sprintf(
			"AI 工作台状态查询响应不是对象（状态 %d）⇒ 本层不猜它是什么", response.Status))
	}
	read := AIWorkbenchTaskStatus{CodeRaw: aiWorkbenchScalarText(root["code"])}
	codeNum, codeIsNum := root["code"].(json.Number)
	// `cs?.code === 0 && cs.result`：后一半是 JS 真值判断（`{}` 也算真），
	// 所以这里用 [playAIChatSSETruthy] 而不是"是不是个对象"——
	// 两档的差别只在"上游把 result 回成数组/字符串"这一格：那边 `EnvelopeOK` 仍成立、
	// 只是读不出两枚状态（⇒ 照旧实现"拿不到状态就问一句"）。零样本，按参考实现抄。
	_, resultIsMap := root["result"].(map[string]any)
	read.HasResult = playAIChatSSETruthy(root["result"])
	if codeIsNum && read.HasResult {
		if n, err := codeNum.Float64(); err == nil && n == 0 {
			read.EnvelopeOK = true
		}
	}
	if resultIsMap && read.EnvelopeOK {
		result, _ := root["result"].(map[string]any)
		read.TaskStatus = aiWorkbenchScalarText(result["taskStatus"])
		read.CloudStatus = aiWorkbenchScalarText(result["cloudStatus"])
		// 两枚都要**是数字**且严格等于 1 / 0（旧实现 `=== 1` / `=== 0`）。
		// 字符串 `"1"` 在那一句下面不成立 ⇒ 这里也不成立（刻意不做隐式转换）。
		taskNum, taskIsNum := result["taskStatus"].(json.Number)
		cloudNum, cloudIsNum := result["cloudStatus"].(json.Number)
		taskVal, taskErr := taskNum.Float64()
		cloudVal, cloudErr := cloudNum.Float64()
		read.GatePass = taskIsNum && cloudIsNum &&
			taskErr == nil && cloudErr == nil && taskVal == 1 && cloudVal == 0
	}
	return read, nil
}

// AppAIWorkbenchTaskStatus 发 ②（或 ④，同一口）。**只读**。
//
// ⚠ 交回的错误里**不含**手机号：RSA 加密在构造阶段就抛错，而 `core` 那几条错误串
// 只带 `error` 文本；这里仍过一遍 [redactError] 兜"上游把提交的东西 echo 回来"那一格。
func (u *Upstream) AppAIWorkbenchTaskStatus(
	ctx context.Context, auth AIWorkbenchDomainAuth, taskID int64, sourceChannel string, rng core.Random,
) (AIWorkbenchTaskStatus, *core.Error) {
	secrets := []core.SecretValue{auth.Phone, auth.Token, auth.Authorization}
	request, perr := AIWorkbenchStatusRequest(auth.Phone, taskID, sourceChannel, rng)
	if perr != nil {
		return AIWorkbenchTaskStatus{}, redactError(perr, secrets...)
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return AIWorkbenchTaskStatus{}, redactError(perr, secrets...)
	}
	status, perr := ParseAIWorkbenchTaskStatus(response)
	return status, redactError(perr, secrets...)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ③ 枪：chat/v2/add（**写**：问一句）+ 本族自己的 SSE 累加器
// ─────────────────────────────────────────────────────────────────────────────

// AIWorkbenchChatParams 是 ③ 的三格入参。
//
// 为什么**没有** `SourceChannel` 与 `Authorization`：前者在本族是常量（两处共用 `"10175"`，
// 旧实现同一个局部变量喂 ② 与 ③，见 [AIWorkbenchSourceChannel]），后者是凭据 ⇒
// 放在函数参数上，好让"这一发带谁"在签名上就看得见。
type AIWorkbenchChatParams struct {
	// UserID 是 body 顶层那枚 `userId`（= ① 回的 `userDomainId`，**不是**库里那份）。
	UserID string
	// Dialogue 是问出去的那一句（旧实现取配置项 `aiChatMessage`，兜底 `"你好"`）。
	Dialogue string
	// InputTime 是 `dialogueInput.inputTime`，格式见 [AIWorkbenchInputTime]。
	InputTime string
}

// AIWorkbenchInputTime 摊 ③ 的 `inputTime`：`YYYY-MM-DD HH:MM:SS`（**本地挂钟分量**，
// 无时区后缀、无毫秒）。
//
// 逐字出处 `ai-workbench.ts` 的 `buildDialogueInput`：它取 `new Date()` 的
// `getFullYear/getMonth+1/getDate/getHours/getMinutes/getSeconds`，各 `padStart(2,"0")`
// ⇒ 那正是 Go 这边"按**本地**时区格式化"的语义（`time.Time.Format` 不换算时区，
// 打印的就是这个时刻自己的挂钟读数）。
//
// ⚠⚠ 它与 585 那一套的 [PlayAIChatCameraInputTime]（UTC + 字面量 `Z`）**不同形**：
// 同一个时刻两族会给出**不同**的串。两族各按各的出处实现，不统一 ——
// 统一等于让其中一条偏离它唯一有出处的写法。
func AIWorkbenchInputTime(at time.Time) string {
	return at.Format("2006-01-02 15:04:05")
}

// AIWorkbenchDialogueInput 拼 ③ 的 `dialogueInput`（`ai-workbench.ts` 的
// `buildDialogueInput` 那 14 键逐字）。
//
// 三处"看着像抄错"的地方**全部照抄**，各自都是**旧实现写死**的字面量：
//   - `commands` 是**字符串** `"000"`（不是数字），而 `command` 是 `null`；
//   - `commandType` 是**数字** `1`；`resourceType` / `dialogueType` 是**字符串** `"0"`；
//   - `toolSetting.imageToolSetting.enableLlmDescribe` 在 `buildDialogueInput` 里写的是
//     `false`（那个 interface 的注释性默认值不算出处）。
//
// ⚠ 这些键**没有一个是本仓能解释的**（语义零成文、零样本）。本层不解释、不校验、
// 不补默认 —— 与 [MailConvertPDFRequest] 对 `convertType` 四档的处置同一取向。
func AIWorkbenchDialogueInput(params AIWorkbenchChatParams) map[string]any {
	return map[string]any{
		"dialogue":                 params.Dialogue,
		"prompt":                   "",
		"inputTime":                params.InputTime,
		"commands":                 "000",
		"command":                  nil,
		"resourceType":             "0",
		"resourceId":               "",
		"dialogueType":             "0",
		"commandType":              1,
		"enableForceNetworkSearch": false,
		"enableAllNetworkSearch":   false,
		"enableAiSearch":           false,
		"extInfo":                  "{}",
		"versionInfo": map[string]any{
			"pcVersion": AIWorkbenchPCVersion,
			"h5Version": AIWorkbenchH5Version,
		},
		"toolSetting":      map[string]any{"imageToolSetting": map[string]any{"enableLlmDescribe": false}},
		"attachment":       map[string]any{},
		"aiWritingSetting": map[string]any{},
	}
}

// AIWorkbenchChatBody 拼 ③ 的外层体（`fireWorkbenchChat` 里 `JSON.stringify` 那一份）。
//
// 键集与 585 那一套的差别（[PlayAIChatCameraBody]）就两处：**多** `continuationInfo:null`，
// **少** `applicationId`；`dialogueInput` 整块是另一组键（见 [AIWorkbenchDialogueInput]）。
// ⚠ 键序由 Go map marshal 成字典序，与出处那份顺序不同 ⇒ 不是对拍面（§1.4），
// 与 585 那一族同一登记。
func AIWorkbenchChatBody(sourceChannel string, params AIWorkbenchChatParams) map[string]any {
	return map[string]any{
		"applicationType":  "chat",
		"sessionId":        "",
		"dialogueInput":    AIWorkbenchDialogueInput(params),
		"sourceChannel":    sourceChannel,
		"userId":           params.UserID,
		"continuationInfo": nil,
	}
}

// aiWorkbenchChatParamGuard 是 ③ 的三格必填护栏（理由逐条写在函数体里）。
func aiWorkbenchChatParamGuard(params AIWorkbenchChatParams) *core.Error {
	switch {
	case params.UserID == "":
		// 这一格填错的表现是"问到别人的用户域下"，比"这一条没做成"重得多 ⇒ 宁可不发。
		return core.NewBadRequest(
			"AI 工作台对话的 userId 不能为空（它是 ① 回的 userDomainId，填错会问到别人的账号）")
	case params.Dialogue == "":
		// ⚠ 与旧实现的**一处刻意偏离**：那边 `ctx.config.mail139?.aiChatMessage || "你好"`
		// 会在配置为空串时**兜底**。本仓把兜底放在 app 层的调用点，本层拒绝空串 ——
		// 让"协议层不发空问句"这件事可断言（同 585 那一族的 [playAIChatParamGuard]）。
		return core.NewBadRequest(
			"AI 工作台对话的 dialogue 不能为空（兜底串属 app 层，本层不内置）")
	case params.InputTime == "":
		return core.NewBadRequest(
			"AI 工作台对话的 inputTime 不能为空（格式见 AIWorkbenchInputTime，与 585 那一套不同形）")
	}
	return nil
}

// AIWorkbenchChatRequest 构造 `POST https://ai.yun.139.com/api/outer/assistant/chat/v2/add`。
//
// 头集合**逐字**四枚 + 三枚标准头（`buildAiChatV2Headers`）：
// `x-yun-api-version: v1` / `x-yun-client-info: 4g||30|||||||1440/900|zh-CN||||` /
// `x-yun-app-channel: <sourceChannel>` / `Authorization: <Basic 整串>` /
// `Content-Type: application/json` / `Accept: text/event-stream` / 桌面 UA。
//
// ⚠ **不复用** [playAIAppHeaders]，也不补 `Origin` / `Referer` / `x-yun-tid` / 设备头：
// 那一条是 585（AI 相机）按实拍裁出来的原生侧那套，本族出处里**没有**这些头。
// 补上会发出与参考实现不同的一份请求，而"上游在不在意这些头"零样本（§1.7：两个方向都不许断言）。
// `x-yun-client-info` 那一串里的空段（`4g||30|…|1440/900|zh-CN||||`）是**逐字**照抄：
// 段数与空值都是旧实现发出去过的那一份，少一个 `|` 就是另一条。
func AIWorkbenchChatRequest(
	authorization core.SecretValue, sourceChannel string, params AIWorkbenchChatParams,
) (core.HTTPRequest, *core.Error) {
	if perr := aiWorkbenchChatParamGuard(params); perr != nil {
		return core.HTTPRequest{}, perr
	}
	if authorization.Value() == "" {
		return core.HTTPRequest{}, core.NewInternal(
			"AI 工作台对话缺 Authorization：这一发的身份全在它上面（错误文本刻意不带它的值）")
	}
	if sourceChannel == "" {
		return core.HTTPRequest{}, core.NewInternal("AI 工作台对话缺 sourceChannel")
	}
	request, jerr := core.NewRequestPost(PlayAIHostAI + "/api/outer/assistant/chat/v2/add").
		WithJSONBody(AIWorkbenchChatBody(sourceChannel, params))
	if jerr != nil {
		return request, asCoreError(jerr, "序列化 AI 工作台对话的请求体失败")
	}
	// `WithJSONBody` 在**没有** content-type 时会补一枚 `application/json`（大小写不敏感地判）；
	// 这里显式再写一次是为了把**头名的大小写**也钉成出处那一份（fetch 侧原样送出）。
	request.Headers = dropHeader(request.Headers, "content-type")
	return request.
		WithHeader("x-yun-api-version", "v1").
		WithHeader("x-yun-client-info", "4g||30|||||||1440/900|zh-CN||||").
		WithHeader("x-yun-app-channel", sourceChannel).
		WithHeader("Content-Type", "application/json").
		WithHeader("Accept", "text/event-stream").
		WithHeader("User-Agent", AIWorkbenchDesktopUserAgent).
		WithHeader("Authorization", authorization.Value()).
		WithTimeoutMS(AIWorkbenchChatTimeoutMS), nil
}

// AIWorkbenchChatRead 是 ③ 那一本**逐行累加**的读数。
//
// 六格全部是旧实现 `consumeSseStream` 往同一个对象上写的那六格（`SseChatResult`）：
// [AIWorkbenchChatRead.Events] 与 [AIWorkbenchChatRead.Answer] 就是它**唯一**的成败判据
// （`runners.ts:400-402`：`eventCount === 0 || !fullText` ⇒ 失败）。
//
// ⚠ 本族**没有** `Success` 字段：585 那一族把判据放在协议层（因为它的判据写在 api 函数里），
// 这一族的判据写在**runner** 里 ⇒ 放在这里就会有两处"谁说了算"。app 层按上面那一句直判。
// 同理不读 `finishReason`（旧实现压根不读它，`ai-workbench.ts:215-236`）。
type AIWorkbenchChatRead struct {
	Answer      string
	SessionID   string
	DialogueID  string
	Events      int
	LastCode    string
	LastMessage string
}

// ParseAIWorkbenchSSE 把 ③ 的响应文本摊成 [AIWorkbenchChatRead]。
//
// 五格取舍逐条对到 `ai-workbench.ts:189-249`，**每一格都与 585 的读法相反或更宽**：
//  1. 只处理**以换行收尾**的整行：TS 是 `buffer.indexOf("\n")` 边收边切，
//     循环退出时 buffer 里那半行**从不 flush** ⇒ 这里丢掉最后一个无换行的尾段
//     （若文本本来就以 `\n` 收尾，那个空尾段丢不丢都一样）。
//  2. **不带 `data:` 前缀的行也 parse**（`let data = line;` 然后才判前缀）
//     ⇒ 一段裸 JSON 流同样计一次事件。
//  3. 空串与 `[DONE]` 只是 `continue`，**不是终止**（585 那边 `[DONE]` 没有任何特殊地位）。
//  4. 解不开的行 `catch{}` **忽略**，不计 `eventCount`。
//  5. `flowResult.outContent` 与 `data.outContent` **两条路径都累加**进同一个串（`:233-234`），
//     正序、不跳末条；`sessionId` / `dialogueId` **后到的覆盖先到的**。
//
// ⚠ 不许拿"585 的实拍里 `data.outContent` 逐行皆空"来解释这一条：那是**另一族、另一次会话**
// 的读数（文件头第二节成文）。本族两套读法各自的对错要等自己的真跑。
func ParseAIWorkbenchSSE(text string) AIWorkbenchChatRead {
	var out AIWorkbenchChatRead
	lines := strings.Split(text, "\n")
	if len(lines) == 0 {
		return out
	}
	for _, raw := range lines[:len(lines)-1] {
		line := strings.TrimSpace(raw)
		if line == "" {
			continue
		}
		payload := line
		if strings.HasPrefix(payload, "data:") {
			payload = strings.TrimSpace(payload[len("data:"):])
		}
		if payload == "" || payload == "[DONE]" {
			continue
		}
		value, ok := core.ParseJSON(payload)
		if !ok {
			continue // 旧实现：`catch { /* not JSON, ignore */ }`
		}
		out.Events++
		root, isObj := value.(map[string]any)
		if !isObj {
			continue
		}
		if code, has := root["code"]; has && playAIChatSSETruthy(code) {
			out.LastCode = aiWorkbenchScalarText(code)
		}
		if message, has := root["message"]; has && playAIChatSSETruthy(message) {
			out.LastMessage = aiWorkbenchScalarText(message)
		}
		data, isData := root["data"].(map[string]any)
		if !isData {
			continue
		}
		if sid, has := data["sessionId"]; has && playAIChatSSETruthy(sid) {
			out.SessionID = aiWorkbenchScalarText(sid)
		}
		if did, has := data["dialogueId"]; has && playAIChatSSETruthy(did) {
			out.DialogueID = aiWorkbenchScalarText(did)
		}
		// 两条路径**都**加（先 flowResult 再直挂，与出处的两行顺序一致）。
		if flow, isFlow := data["flowResult"].(map[string]any); isFlow {
			if chunk, has := flow["outContent"]; has && playAIChatSSETruthy(chunk) {
				out.Answer += aiWorkbenchScalarText(chunk)
			}
		}
		if chunk, has := data["outContent"]; has && playAIChatSSETruthy(chunk) {
			out.Answer += aiWorkbenchScalarText(chunk)
		}
	}
	return out
}

// aiWorkbenchScalarText 把一个 JSON 标量摊成"旧实现 `String(x)` 会得到的那个串"。
//
// 三档够用：字符串原样、[json.Number] 取字面量文本、布尔给 `true` / `false`。
// ⚠ 两处与 JS 的**已知差别**（都零样本，登记在 `docs/pending-decisions.md` 本族那一格）：
//   - `json.Number` 保留**输入字面量** ⇒ 上游若回 `1.50`，这里念 `1.50`，JS 念 `1.5`；
//     整数形态（最常见那一档）两边一致。
//   - 对象与数组落空串，而 JS 的 `String({})` 是 `[object Object]`。那种形状出现在
//     `outContent` 上本身就是意外，跟着念一串 `[object Object]` 进播报不更有用。
func aiWorkbenchScalarText(value any) string {
	switch typed := value.(type) {
	case string:
		return typed
	case json.Number:
		return typed.String()
	case bool:
		return strconv.FormatBool(typed)
	default:
		return ""
	}
}

// AppAIWorkbenchChat 发第 ③ 枪。**写**：一次调用问一句（上游会记一次会话）。
//
// 为什么它不复用 [Upstream.Raw]：坑表 #165 —— `internal/io` 的 `Send` 在非 2xx 时
// **同时**交回响应与错误，而 `Raw` 会把体丢掉。"这一发被谁拒、回的是什么页"只有体答得出，
// 所以处方与 [Upstream.PlayAIChatCamera] 同一套：**先问有没有可用响应 → 先取体 → 再判状态码**，
// 并把状态与截断后的体一起放进错误文本。
//
// ⚠ 错误文本里带响应体的边界（同 585 那一族）：这一族的体是**模型输出的那一句**，
// 不含凭据 ⇒ 截断后可进；**请求侧**任何值（Basic 整串、DES 密文、手机号、RSA 密文）
// 一律不进 —— 所以出口仍过 [redactError]，兜的是"上游把提交进来的东西 echo 回来"。
func (u *Upstream) AppAIWorkbenchChat(
	ctx context.Context, auth AIWorkbenchDomainAuth, params AIWorkbenchChatParams,
) (AIWorkbenchChatRead, *core.Error) {
	secrets := []core.SecretValue{auth.Phone, auth.Token, auth.Authorization}
	request, perr := AIWorkbenchChatRequest(auth.Authorization, AIWorkbenchSourceChannel, params)
	if perr != nil {
		return AIWorkbenchChatRead{}, redactError(perr, secrets...)
	}
	response, err := u.http.Send(ctx, request)
	if response.Status == 0 {
		// 连状态码都没有 ⇒ 这一档**才是**"没有响应"：只报错误类别，不凭空造摘录。
		if err != nil {
			return AIWorkbenchChatRead{}, redactError(asCoreError(err, "发送 AI 工作台对话失败"), secrets...)
		}
		return AIWorkbenchChatRead{}, redactError(
			core.NewParse("AI 工作台对话没有拿到响应（状态码为空）"), secrets...)
	}
	text, textErr := response.Text()
	if !response.IsSuccess() {
		return AIWorkbenchChatRead{}, redactError(core.NewBadRequest(fmt.Sprintf(
			"AI 工作台对话上游回 HTTP %d%s", response.Status, aiWorkbenchExcerpt(text))), secrets...)
	}
	if textErr != nil {
		return AIWorkbenchChatRead{}, redactError(core.NewParse(
			"AI 工作台对话的响应体不是合法 UTF-8，无法按文本读 SSE"), secrets...)
	}
	// 2xx 还带错误：本仓的 `Send` 走不到这里（非 2xx 才造错误），这一支只是把
	// "**有体、且是 2xx ⇒ 读体**"写死，与 [Upstream.PlayAIChatCamera] 同一形状。
	// ⚠ 未被断言（自定义客户端才演得出来）。
	return ParseAIWorkbenchSSE(text), nil
}

// aiWorkbenchExcerpt 把 ③ 的响应体摊成一句可进错误文本的截断摘录。
//
// 三条裁剪规则（压平换行 / 按**码点**截 / 空体另说）是**这一族的**规矩，
// 所以自己写一条而不是复用 [playAIChatSSEExcerpt]（那一处的头注就写着"别处要截断请自己写一条"）。
// 位数取 300 的出处：旧实现非 2xx 那一支自己截的是 `text.slice(0, 300)`
// （`ai-workbench.ts:278`）—— ⚠ 那是 **UTF-16 单元**数，这里是**码点**数，
// 于是这一格是我们对"日志里该念多少体"的选择，不是上游的成文。
func aiWorkbenchExcerpt(text string) string {
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
	if len(runes) > 300 {
		return fmt.Sprintf("；响应摘录：%q…（截断，共 %d 码点）", string(runes[:300]), len(runes))
	}
	return fmt.Sprintf("；响应摘录：%q", flat)
}
