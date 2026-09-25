// 139 邮箱会话那一族 —— 玩法 **472 / 473「体验 139 邮箱」**要的四枪。
//
//	① POST user-njs.yun.139.com/user/querySpecTokenV2  {"toSourceId":"001003"}  → `data.token`（SSO 票）
//	② POST mail.10086.cn/login/inlogin.action?cguid=<客户端随机串>              → `var.sid` / `var.rmkey`
//	③ GET  mail.10086.cn/login/autologin.action?sid=<sid>&rmkey=<rmkey>          → "真的打开过一次邮箱"那一下
//	④ POST caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2            → 把这条任务记成"做了"
//
// # 四枪各自的**证据面**（2026-09-26 活体探针之后）
//
//   - ① 有实拍（`docs/capture/app-signin-samples.txt` §2.1：同一条接口的 WebKit 发起方回
//     `{"success":true,"code":"0000","data":{"token":"YZsidssolg<32 位小写 hex>"}}`）；
//     探针两次真跑也都换到了票（长度 42，值不落档）。
//   - ② 请求面有实拍（`har/signin-2026-09-19-full.har` 第 160 条，14:38:59，
//     六键单行 XML + `?cguid=<uuid>`，`content-type: application/xml; charset=utf-8`、
//     `origin: mcloudlocal://yun.139.com`，**不带 authorization、不带 cookie**；
//     响应头拍到过 `content-type: text/javascript;charset=UTF-8` + gzip，但**响应体未捕获**）；
//     **响应面现在也有实测了**（来源是探针，不是抓包）：`HTTP 200` + `code="S_OK"`，
//     `var.sid`(38) 与 `var.rmkey`(16) 都在，且**标准 JSON 严解即成**
//     ⇒ 本节此前那句"成功形状零样本"作废，逐键形状与脱敏读数在
//     `docs/capture/app-mail139-samples.txt` §2。
//     ⚠ 它**名子上是 `text/javascript`，内容却是能直接 `JSON.parse` 的那一份** ——
//     别按 content-type 去写解析分支（本层的 `parseMailJSONText` 走的正是"先当 JSON 解"）。
//     链是闭合的：那一条体里的 `token` 与第 39 条（`toSourceId=001003`）换到的那串**逐字相同**
//     ⇒ "①→②"不是推断，是拍到的。
//   - ③④ 在 `har/` 与夹具里**一条都没有**（2026-09-25 按 `autologin.action` 与
//     `taskClickV2` 逐份扫过，零命中），形状来自旧 TS `runners.ts:190-196`（③）与
//     `:218-232`（④）—— 而它两条都被 `tryRequest` 包着 ⇒ 旧实现从来没判过响应。
//     现在探针把两条各打了一遍（走的就是本文件这一层构造的请求）：**两条都回 404**，
//     而归因排除了"我们这一侧写错"——
//     ③ 是 `mail.10086.cn` **自己的 HTML 404 页**（带 / 不带 `authorization`、
//     裸拼 / 百分号编码，三档**逐字节同形**）；④ 是**应用给的 JSON**
//     （`{code:"404", msg:"访问资源不存在"}`，而同一入口 `:7071/ycloud/` 上另一条只读路由
//     回 `200` ⇒ 挂的是那一条路由，不是整片入口）。
//     并且我们的 URL 与体三枚键跟旧 TS **逐字一致** ⇒ 这不是迁移抄错地址；
//     上游今天对两份实现说的是同一句"没有这个资源"。
//     ⚠ 这句话**不等于**"这一族已下线"：厂商 App 自己现在打哪一发，探针答不了
//     （要一次真机取证，问句已挂在 `docs/pending-decisions.md`，
//     取证方案见 `docs/capture/app-mail139-samples.txt` §6）。
//
// ⇒ ②的**请求面**取实拍那一份，③④只能取旧实现那一份 —— 而 ③④ 现在多了一档判据：
// 探针的 `404` 读数（`docs/capture/app-mail139-samples.txt` §1）。
//
// # ② 为什么**不带** authorization，而旧 TS 带
//
// 旧 TS 的 `loginMail` 走主 http 实例，而 `AUTH_DOMAINS`（`packages/shared/src/http/base.ts:11`）
// 里**有** `mail.10086.cn` ⇒ 那一发在旧实现里是带 `authorization`（外加 `deviceid` 与安卓
// WebView UA）的。实拍那一条是 **iPad WebView 直接发**的，一个认证头都没有。
// 本层照**实拍**：只带那五枚应用层头。这一格是"两家请求面不一致、取拍到过的那一份"，
// 不是"authorization 一定多余"—— 上游收过的那一份里没有它，仅此而已。
// ⚠ 2026-09-26 探针按这一份（不带）真打过：`HTTP 200` + `code="S_OK"` ⇒ **这一档是跑通过的**，
// 但它只证明"不带能换到会话"，仍不证明"带了会坏"（旧 TS 带了也换到过，那是另一家的事实）。
//
// ⚠ 反过来，③（旧实现同样经主实例 ⇒ 带 authorization）本层**也带上**：
// 那口零实拍，唯一的出处就是它带去的，把它剥掉等于发一份谁都没发过的请求。
// 探针把"带"与"不带"各打了一遍：**两条都回 404，且响应体逐字节同形**
// ⇒ 这一格没有被答掉，只是被**挪开**了：404 的原因不是 authorization，
// 而"这一口到底要不要那枚头"仍未知（要它先有一个非 404 的响应才谈得上）。
//
// # 凭据边界（这一族有**三枚**新凭据，别漏）
//
// `ssoToken`（①的票，实测几十秒就失效）、`sid`、`rmkey` 全是会话凭据：
//   - ②把它放进**请求体**、③把它放进**查询串**；
//   - [core.SecretValue] 只挡打印，不挡 `encoding/json` ⇒ [MailSession] 刻意**不打 json tag**、
//     也不出现在任何被 marshal 的结构里；
//   - ⚠ ③ 的 URL 里含两枚凭据，而 [internal/io] 的传输层错误**把 URL 写进错误文本** ⇒
//     那一发的错误必须过 [redactError] 才交回，否则日志里就是一次凭据外泄。
package upstream

import (
	"context"
	"fmt"
	"sort"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// Mail139SourceID 是邮箱那一族自己的 `toSourceId`。
//
// ⚠ 它与签到族的 [ToSourceID]（`001005`）**是两个值，不是同义写法**：
// 实拍里 `001003` 挂在 WebKit 发起的那一条（权益 / 云票 / 邮箱），`001005` 挂在签到页 URL
// 自带的 `targetSourceId`。成文口径见 `docs/capture/app-signin-samples.txt` §2.1 末尾那条纠错：
// **跨端、跨发起方的常量一律不许互搬**。这里取 `001003`，出处是"②那条 inlogin 的体里的 token
// 来自一次 `{"toSourceId":"001003"}`"—— 拍到过，不是选了一个看起来对的。
const Mail139SourceID = "001003"

// 这一族自己的路径与体里的字面量。
//
// 它们和 [noteMobileUserAgent] 同一角色：**素材字面量**（对任何账号都发同一串）⇒ 整串照抄，
// 不按仓库里其它族的常量重拼。
const (
	mailInLoginPath   = "/login/inlogin.action"
	mailAutoLoginPath = "/login/autologin.action"
	// mailInLoginClientID 是体里的 `clientId`（实拍 `10804`；旧 TS 同值）。
	mailInLoginClientID = "10804"
	// mailInLoginVersion 是体里的 `version`（`9`）。
	//
	// ⚠ 它**不是** App 版本号（[CaiyunAppVersion] 那个 `12.6.0`）：同一个体里两枚不同族的数，
	// 实拍 `10804 / 9`，照抄。
	mailInLoginVersion = "9"
	// mailInLoginType 是体里的 `loginType`（`7` = SSO 票登录；旧 TS 同值）。
	mailInLoginType = "7"
	// mailInLoginEMode 是体里的 `eMode`（`1`）。
	//
	// ⚠ **实拍有、旧 TS 没有**的那一格（旧 TS 只发四键）。取实拍那份 ⇒ 这一族发六键。
	mailInLoginEMode = "1"
	// mailAppOrigin 是 ② 的 `origin` —— App 自定义 scheme，服务端 CORS 按它放行
	// （实拍 ② 的 OPTIONS 回 `access-control-allow-origin: mcloudlocal://yun.139.com`）。
	mailAppOrigin = "mcloudlocal://yun.139.com"
	// mailWebViewUserAgent 是 ② 实拍那一条的 UA（iPad WebKit 发起方）。
	//
	// ⚠ 它与 [AppProfile] 那套原生头里的 `User-Agent`（`mCloud_iPhone/… (iPad; iOS …)`）
	// **不是同一枚**：拍到的这一发是 WebView 发的。照抄拍到那份，理由同
	// [noteMobileUserAgent] —— 上游收到过的就是它。
	// ⚠ 也别读成"这一族要求 iPad UA"：上游查不查 UA **未验证**（零样本），
	// 要核它得让 ② 真跑一次并回读上游说的话。
	mailWebViewUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_7 like Mac OS X) " +
		"AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"
	// mailXMLContentType 是 ② 的 `content-type`。
	//
	// ⚠ 字面量取自实拍：小写头名、`charset` 前有一个**空格**、`utf-8` **小写**。
	// 旧 TS 写的是 `application/xml;charset=UTF-8`（无空格、大写）。两者都算"XML"，
	// 但上游按串匹配的可能性不为零 ⇒ 取拍到那份。
	mailXMLContentType = "application/xml; charset=utf-8"
)

// MailSession 是一次 139 邮箱会话的两枚凭据。
//
// 两个字段都是 [core.SecretValue]：② 的响应体一旦能被 marshal，就等于把凭据写进了日志。
// ⚠ 所以这一结构体**永不**参与 `json.Marshal`（也不得进 fixture 真值）。
type MailSession struct {
	// SID 是 `var.sid`，进 ③ 的查询串与（将来那几口）的 `Cookie: Os_SSo_Sid=`。
	SID core.SecretValue
	// RMKey 是 `var.rmkey`，进 ③ 的查询串与 `Cookie: RMKEY=`。
	RMKey core.SecretValue
}

// OK 报告这一张会话是否凑齐了两枚凭据。
//
// 存在是因为调用方要把"没凑齐"播成一句人话，而不是把一个 [core.SecretValue] 摊开判空。
func (s MailSession) OK() bool { return s.SID.Value() != "" && s.RMKey.Value() != "" }

// escapeXMLText 转义 XML 文本节点里的五个字符。
//
// 抄旧 TS `apis/auth.ts:4-9` 的 `escapeXml`（`& < > " '`）。今天 `ssoToken` 的形状是
// `YZsidssolg` + 32 位小写 hex ⇒ 里面**没有**需要转义的字符，这一句看起来是死代码。
// 但它不是：票的字符集是上游定的，而我们**没有**成文说它恒为 hex
// （§2.1 只记录了见过的两例）。少了这一步，一张带 `&` 的票会把体拼成一份**结构不同**的 XML，
// 而表现是"②一直失败"—— 那正是最难归因的一类故障。
func escapeXMLText(value string) string {
	replacer := strings.NewReplacer(
		"&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;", "'", "&apos;")
	return replacer.Replace(value)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ② 枪：换邮箱会话（inlogin）
// ─────────────────────────────────────────────────────────────────────────────

// MailInLoginRequest 构造 `POST mail.10086.cn/login/inlogin.action?cguid=<cguid>`。
//
// 体的**六键与顺序**逐字取自实拍（单行、无换行、`clientIp` 是空串）：
// `clientId` → `version` → `loginType` → `eMode` → `token` → `clientIp`。
// ⚠ 与旧 TS 差三处，都取实拍那一份：旧 TS 只发四键（没有 `eMode` / `clientIp`）、
// 体里带换行与缩进、URL **不带** `?cguid=`。
//
// `cguid` 由**调用方**生成并传进来（本层是纯层，不许自己取随机数）：
// 实拍那一枚是 UUID v4 形态 ⇒ 编排层用 `core.UUIDv4(rng)`。
// ⚠ 别与旧 TS 同族另两口的 cguid 拼法混用（`apis/mail139.ts` 里另有
// 8 位 hex 与 `0`+随机数两种）—— 那是**别的接口**的，本口只见过 UUID。
//
// 头只有实拍那五枚应用层头（`accept` / `content-type` / `origin` / `user-agent` /
// `accept-language`）：⚠ **刻意不带 `authorization`、不带 `cookie`**（文件头那一节），
// 也**不带** `sec-fetch-*` 与 `priority` —— 那几枚是浏览器自动加的，本仓没有任何一发
// 请求手写它们。⚠ 2026-09-26 探针按这一份头集合真跑过：**上游照收**
// （`HTTP 200` + `code="S_OK"`）⇒ 这一格从"未验证"转成"实测通过"（读数见
// `docs/capture/app-mail139-samples.txt` §1 ②）。
// ⚠ 别把这一句读成"CORS 测过了"：探针是 Go 程序直连，浏览器的同源策略**根本不参与**，
// 这里证的只是"少发那几枚头，上游不拒"。
func MailInLoginRequest(ssoToken, cguid string) (core.HTTPRequest, *core.Error) {
	if ssoToken == "" {
		return core.HTTPRequest{}, core.NewInternal("邮箱登录缺 ssoToken：它是这一个体的唯一凭据键（错误文本刻意不带它的值）")
	}
	if cguid == "" {
		return core.HTTPRequest{}, core.NewInternal("邮箱登录缺 cguid：实拍那条的 URL 带着它，少发等于换了一条接口")
	}
	body := fmt.Sprintf(
		"<?xml version=\"1.0\" encoding=\"utf-8\"?><object>"+
			"<string name=\"clientId\">%s</string>"+
			"<string name=\"version\">%s</string>"+
			"<string name=\"loginType\">%s</string>"+
			"<string name=\"eMode\">%s</string>"+
			"<string name=\"token\">%s</string>"+
			"<string name=\"clientIp\"></string>"+
			"</object>",
		mailInLoginClientID, mailInLoginVersion, mailInLoginType, mailInLoginEMode,
		escapeXMLText(ssoToken))
	// `core.Body` 直接赋值：这一口发的是 XML，[core.HTTPRequest.WithJSONBody] 会补
	// `application/json`，而那正是实拍里**没有**的那一枚 content-type。
	request := core.NewRequestPost(BaseMail139+mailInLoginPath+"?cguid="+PercentEncode(cguid)).
		WithHeader("content-type", mailXMLContentType).
		WithHeader("origin", mailAppOrigin).
		WithHeader("user-agent", mailWebViewUserAgent).
		WithHeader("accept-language", "zh-CN,zh-Hans;q=0.9").
		WithHeader("accept", "*/*")
	request.Body = core.Body(body)
	return request, nil
}

// parseMailJSONText 把响应文本解成 JSON，**先严后宽**。
//
// 严的那一遍是标准 JSON；宽的那一遍是 `RmWeb` 那一族的既有事实：`apis/mail139.ts:66-70`
// 的 `parseLooseJson` 把**单引号**换成双引号再解，因为那一族回的是单引号 JSON。
// ⚠ 但 inlogin（`mail.10086.cn`）与 `RmWeb`（`appmail.mail.10086.cn`）是**两台**，
// 而旧 TS 对 inlogin 用的是标准 `JSON.parse` ⇒ 宽那一遍在这里是**同族惯例的外推，未被证实**。
// 它只在严解**失败**之后才起作用，所以不会把一份本来好好的 JSON 解坏；
// 代价是"真解析失败"时错误文本要说两遍都试过了（见 [ParseMailSession]）。
func parseMailJSONText(text string) (any, bool) {
	if value, ok := core.ParseJSON(text); ok {
		return value, true
	}
	loose := strings.ReplaceAll(text, "'", `"`)
	value, ok := core.ParseJSON(loose)
	return value, ok
}

// ParseMailSession 从 ② 的响应里取 `var.sid` 与 `var.rmkey`。
//
// # 判据只有一条：两枚都在
//
// 旧 TS `tasks/auth-service.ts:26-40` 的判据就是 `v?.sid && v?.rmkey`，**不看 `code`**。
// 这里照它（R27 同族：判据只在有出处的时候才落码）。⚠ 这一族的响应体形状**零样本**
// —— 实拍那条连体都没捕到（gzip、`text/javascript`），所以"成功时长什么样"
// 只由旧实现那句支撑，第一次真跑要回读的就是它。
//
// # 失败文本为什么**绝不 echo 响应体**
//
// 旧实现在这里 `logger.error("获取 sid 失败", body)` —— 把**整个响应**摊进日志。
// 那一口返回的正是会话凭据，而失败响应里带什么没有任何样本 ⇒ 这里只说：
// HTTP 状态、缺哪几枚键、以及（若解得开）顶层 `code` 与 `summary` 两枚**字符串**，
// 并额外把 `ssoToken` 从成品文本里抹一遍（上游有把提交回来的票 echo 进错误消息的先例：
// `200050409 token 已失效`）。两枚凭据都拿到了却仍走到这里的情况不存在，
// 所以缺的那枚必然不在文本里 —— 但 `code` / `summary` 里可能有的东西我们无法预期，
// 于是**先抹再拼**而不是指望它干净。
func ParseMailSession(response core.HTTPResponse, ssoToken string) (MailSession, *core.Error) {
	value, ok := parseMailJSONText(string(response.Body))
	if !ok {
		return MailSession{}, core.NewParse(redactSecrets(fmt.Sprintf(
			"邮箱登录响应解不开（标准 JSON 与同族那套单引号两种都试过，%d 字节，状态 %d）"+
				"⇒ 这一族的响应体形状零样本，第一趟真跑要先把它读出来",
			len(response.Body), response.Status), core.NewSecretValue(ssoToken)))
	}
	var sid, rmKey string
	if obj, isObj := value.(map[string]any); isObj {
		if inner, isVar := obj["var"].(map[string]any); isVar {
			sid, _ = inner["sid"].(string)
			rmKey, _ = inner["rmkey"].(string)
		}
	}
	missing := make([]string, 0, 2)
	if sid == "" {
		missing = append(missing, "var.sid")
	}
	if rmKey == "" {
		missing = append(missing, "var.rmkey")
	}
	if len(missing) > 0 {
		return MailSession{}, core.NewParse(redactSecrets(
			"邮箱登录没给 "+strings.Join(missing, " 与 ")+"（状态 "+
				strconv.FormatUint(uint64(response.Status), 10)+"，顶层键："+topLevelKeys(value)+
				mailCodeHint(value)+"）⇒ 后三枪不发，这张会话作废",
			core.NewSecretValue(ssoToken)))
	}
	return MailSession{SID: core.NewSecretValue(sid), RMKey: core.NewSecretValue(rmKey)}, nil
}

// topLevelKeys 交出响应对象的键名（**只有键名**）。
//
// 为什么这一格值得留：② 的形状零样本，第一次真跑如果是"解开了却没有 var"，
// 唯一的线索就是"上游到底给了什么键"。值一概不交，因为值的形状未知、
// 而这一族失败响应里最可能出现的就是票。
//
// ⚠ **按键名升序**交回，不是随手 join：Go 的 map 遍历顺序每次都可能不同，
// 而这一串是**进日志与错误文本**的 ⇒ 不排序会让同一份响应产出两种文本，
// 用例也只能写成"两个键各自在不在"而不是"这一串是什么"。（第一次随机红就在
// 这里：`{"msg":…,"data":…}` 交出过 `msg,data` 也交出过 `data,msg`，坑表 **#166**。）
func topLevelKeys(value any) string {
	obj, ok := value.(map[string]any)
	if !ok {
		return "响应不是对象"
	}
	names := make([]string, 0, len(obj))
	for key := range obj {
		names = append(names, key)
	}
	if len(names) == 0 {
		return "空对象"
	}
	sort.Strings(names)
	return strings.Join(names, ",")
}

// mailCodeHint 补一句这一族的业务码（只读 `code` / `summary` 两枚**字符串**）。
//
// 139 邮箱那一族成功码是 `S_OK`（`apis/mail139.ts` 多处与 `code: "PARSE_ERR"` 相对），
// 所以这两枚取到的是"上游自己说的话"，不是我们从状态码推的结论。
// ⚠ 取不到或不是字符串 ⇒ 交回空串：宁可不多说，也不把载荷整块摊开。
// 截断到 120 个字符：`summary` 在上游那边可以是任意长度。
func mailCodeHint(value any) string {
	obj, ok := value.(map[string]any)
	if !ok {
		return ""
	}
	parts := make([]string, 0, 2)
	for _, key := range []string{"code", "summary"} {
		if text, isStr := obj[key].(string); isStr && text != "" {
			parts = append(parts, key+"="+clipRunes(text, 120))
		}
	}
	if len(parts) == 0 {
		return ""
	}
	return "｜上游响应：" + strings.Join(parts, ":")
}

// AppMail139Login 用一张 SSO 票换邮箱会话（② 那一发）。
//
// ⚠ 票**很短命**（§2.1.1 记着实测：换到 3 分 15 秒后重放已被拒 `200050409 token 已失效`）
// ⇒ 调用方必须"换完立刻用"，不要缓存 `ssoToken`；要缓存的是**换到的会话**（见 app 层
// 那一趟内的缓存），因为 `sid`/`rmkey` 的有效期是拍到的另一回事。
//
// 走 [Upstream.Raw] 而不是 [Upstream.Call]：实拍那条响应的 `content-type` 是
// `text/javascript`，喂给 `ParseEnvelope` 只会得到一句"响应不是 JSON"，
// 而它会掩盖真正的问题（票失效 / 被风控 / 给了一个 HTML 壳）。
func (u *Upstream) AppMail139Login(
	ctx context.Context, ssoToken, cguid string,
) (MailSession, *core.Error) {
	request, perr := MailInLoginRequest(ssoToken, cguid)
	if perr != nil {
		return MailSession{}, perr
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return MailSession{}, perr
	}
	return ParseMailSession(response, ssoToken)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ③ 枪：autologin（"真的打开过一次邮箱"）
// ─────────────────────────────────────────────────────────────────────────────

// MailAutoLoginRequest 构造 `GET mail.10086.cn/login/autologin.action?sid=<sid>&rmkey=<rmkey>`。
//
// ⚠ **两枚凭据在查询串里**，而传输层错误文本会带上 URL ⇒ 这一发的错误必须过
// [redactError]（见 [Upstream.AppMailAutoLogin]）。值本身先 [PercentEncode]：
// `sid` 实拍形状是 hex 串，`rmkey` 此前**零样本**（2026-09-26 探针读到长度 16，
// 而 `sid` 是 38 —— 值不落档，见 `docs/capture/app-mail139-samples.txt` §2），
// 旧 TS 同族的另一口又把 `sid` **裸拼**进 URL（`apis/mail139.ts:73`）——
// 那两种在这一口都对不对**仍无判据**（探针两次都回 404，见下），
// 取"编码"是因为不编码时一个带 `+` 的值会**静默**变成另一个值（见 [PercentEncode]）。
// ⚠ 探针做过"裸拼 vs 编码"的对照档：两档响应**逐字节同形**（都是 HTML 404 页）
// ⇒ 这一格既没被证明对、也没被证明错，只是"不是 404 的原因"。
//
// `authorization` 传空 ⇒ 不带那一枚。默认**要带**：这一口零实拍，唯一的出处是旧 TS
// 经主实例发它，而 `mail.10086.cn` 在 `AUTH_DOMAINS` 里 ⇒ "上游收到过的"就是带的那一份。
// ⚠ 这与 ② 相反（② 有实拍、实拍不带）—— 两口的证据类型不同，处置就不同，不是不一致。
// ⚠ 2026-09-26 探针按"带 / 不带"各打一次：两条都回 `HTTP 404` + 同一份 HTML 404 页
// ⇒ 这一口今天**在厂商侧是什么状态**未知（这一族要不要它，得等一次真机取证才能答）。
func MailAutoLoginRequest(session MailSession, authorization string) (core.HTTPRequest, *core.Error) {
	if !session.OK() {
		return core.HTTPRequest{}, core.NewInternal(
			"邮箱活跃没发：这张会话缺 sid 或 rmkey（错误文本刻意不带它们的值）")
	}
	request := core.NewRequestGet(BaseMail139+mailAutoLoginPath+
		"?sid="+PercentEncode(session.SID.Value())+
		"&rmkey="+PercentEncode(session.RMKey.Value())).
		WithHeader("accept", "*/*").
		WithHeader("origin", mailAppOrigin).
		WithHeader("user-agent", mailWebViewUserAgent)
	if authorization != "" {
		request = request.WithHeader("authorization", authorization)
	}
	return request, nil
}

// AppMailAutoLogin 发第 ③ 枪。
//
// **响应不判**：旧 TS `runners.ts:190-196` 用 `tryRequest` 整发吞掉，本仓**零样本**说
// 它成功时长什么样（连是不是 JSON 都不知道）。所以这里只把"没发出去 / 非 2xx"交回，
// 2xx 一律算"发过了"。⚠ 别读成"这一枪算成功了" —— 它算不算 472 的判据要看 ④ 之后
// 复查那趟的读数。
//
// ⚠ 2026-09-26 探针真跑过这一发：`HTTP 404`（三档同形，见 [MailAutoLoginRequest] 头注）
// ⇒ "成功形状长什么样"这一格**仍然空着**，"③④ 上游认不认"这一问也**还没有正面答案**：
// 现在的读数是"两发都 404"，而不是"打了不算"。404 时这一口会经 [redactError] 交回
// `不存在: HTTP 404`，编排层照旧实现往下发 ④（见 `task_matrix_mail.go`）。
//
// ⚠ 错误文本必须过 [redactError]：URL 里两枚凭据，而 `internal/io` 的传输层错误
// 原样嵌 URL（实测形状 `Get "https://…?sid=…&rmkey=…": dial tcp …`）。
func (u *Upstream) AppMailAutoLogin(
	ctx context.Context, session MailSession, authorization string,
) *core.Error {
	request, perr := MailAutoLoginRequest(session, authorization)
	if perr != nil {
		return perr
	}
	_, perr = u.Raw(ctx, request)
	return redactError(perr, session.SID, session.RMKey)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ④ 枪：taskClickV2（把这条任务记成"做了"）
// ─────────────────────────────────────────────────────────────────────────────

// MailMarketTaskClickRequest 构造
// `POST caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2`。
//
// 体三键逐字（旧 TS `runners.ts:221-225`）：`marketname`、`clientVersion`、`taskId`。
// `marketname` 由调用方给：**472 / 473 用 [MarketName]（`sign_in_3`）**，
// 而旧实现在 1004 那一发用的是 [TaskListV2EmailMarketName]（`newsign_139mail`）⇒
// 它不是端常量，别在这一层钉死。
// `clientVersion` 取 [CaiyunAppVersion]（`12.6.0`）—— 旧 TS 那一发带的就是同名常量
// `CAIYUN_APP_VERSION`，与 [TaskClientVersion] 同值但**出处不同**，这里按旧 TS 的字面量族走。
//
// 头沿用 [WithH5Headers]（与 `app_notice.go` 里 `:7071` 那两口同一先例），
// ⚠ **jwttoken 使用公共来源**：这一发拿的是签到页那条会话的票与 referer
// （`internal/app/signin.go` 的 [OpenSignInSession] 那一套）。依据是旧 TS 的主实例
// 对 `caiyun.feixin.10086.cn`（`TARGET_HOSTNAMES` 里有它）注入的就是**同一枚** jwttoken。
// 这一族**特有**的取法至今没有拍到过 ⇒ 按 AGENTS.md §1.12 走兜底；
// 将来抓到"邮箱市场这一口认自己那张票"，改的是调用点的票来源，并**同时**改掉这句话。
// `activityid` 跟着本次的 `marketname` 走（同 [TaskListV2Request] 那一处的处置）——
// ⚠ 这一格是**类比**，不是实拍：零实拍说"这一口看 activityid"。
//
// ⚠ 2026-09-26 探针真跑过这一发（同一个 `taskId=472`、直发与生产路径各一次）：
// `HTTP 404` + 体是 JSON `{code:"404", msg:"访问资源不存在"}`。这条读数的用处与**边界**：
//   - 用处：证明请求**到了应用**（同一入口另一条只读路由 `getTaskInfo` 回 `200`），
//     所以 404 不是"基址写错 / 端口不通 / 网关挡下"；而我们的 URL 与体三键跟旧 TS 逐字一致。
//   - 边界：它**没有**回答"这一口认不认这次打卡"，也**没有**回答"`activityid` 要不要"——
//     路由都不在，头与体怎么配都测不出来。上面那句"类比"仍是类比。
func MailMarketTaskClickRequest(
	jwt, referer, marketName string, taskID int64,
) (core.HTTPRequest, *core.Error) {
	if taskID <= 0 {
		return core.HTTPRequest{}, core.NewParse(fmt.Sprintf(
			"邮箱市场打卡需要正整数任务 id，实得 %d（不发 id=0 的试探请求）", taskID))
	}
	if marketName == "" {
		return core.HTTPRequest{}, core.NewInternal("邮箱市场打卡缺 marketname：旧实现两处调用点用的是两个不同值，没有默认")
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/task/taskClickV2", BaseCaiyunYcloud)).WithJSONBody(map[string]any{
		"marketname":    marketName,
		"clientVersion": CaiyunAppVersion,
		"taskId":        taskID,
	})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = WithH5Headers(request, referer, &jwt, "true")
	request.Headers = dropHeader(request.Headers, "activityid")
	return request.WithHeader("activityid", marketName), nil
}

// AppMailMarketTaskClick 发第 ④ 枪。**写请求**（不可回滚：没有"撤销打卡"这一口）。
//
// 判据用 [EnsureOKWithHint]（`code != 0` 即失败），理由与 mrp `doTask` 那一口**不同**：
// 那一条零实拍且响应形状未知 ⇒ 刻意不判（见 [`app_task_click.go`](app_task_click.go) 文件头）；
// 这一条虽然也零实拍，但它打的**就是签到族那个 click 的 V2**，同族四发打卡实拍
// 全是 `{"code":0,"msg":"success"}` ⇒ "有一个 0 码信封"是有同族依据的，
// 而"上报失败却静默"是本仓反复批评旧实现的那一处。
// ⚠ 判到**非 0** 时按失败交回，但别把它读成"上游说这一条没做完"：
// 同一路径的兄弟口今天对 22 个 id 全回 `58999 任务不存在`（坑表 #149），
// 非 0 码的语义在这一族仍然**未验证**。
func (u *Upstream) AppMailMarketTaskClick(
	ctx context.Context, jwt, referer, marketName string, taskID int64,
) *core.Error {
	request, perr := MailMarketTaskClickRequest(jwt, referer, marketName, taskID)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = EnsureOKWithHint(envelope)
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 错误文本脱敏（这一族需要**保住类别**，与 notify 那一族不同）
// ─────────────────────────────────────────────────────────────────────────────

// redactError 在**保住变体语义**的前提下重写一条错误的正文。
//
// # 为什么不能像 notify 那样只交回一句文本
//
// `notify_two_step.go:308` 那里做的是 `Reason`（给人看的字符串），丢弃类别没有后果；
// 这一族交回的是 [core.Error]，调用链上要按 [ErrorKind.IsRetryable] 决定重不重试、
// 按 [Error.IsSessionExpired] 决定刷不刷票。用 [core.NewNetwork] 一盖 ⇒ 一次 `90001`
// 会变成"网络错误"，那既是错的文案也是错的处置。
//
// # 重建为什么够用
//
// [core.Error] 的变体是**不导出**的，只能靠 [Error.Kind] + [Error.Status] +
// [Error.IsSessionExpired] + [Error.IsFixtureMiss] 这四个出口还原。四个都点名了 ⇒
// 全部 13 个变体都能落回同一类别与同一对外文案（`Internal` 与 `FixtureMiss` 同类、
// `BadRequest` 与 `SessionExpired` 同类，正是那两个出口存在的理由）。
//
// ⚠ 快路径：文本里**没有**任何一枚凭据 ⇒ 原样交回同一个指针，不做一次无谓的重建。
func redactError(err *core.Error, secrets ...core.SecretValue) *core.Error {
	if err == nil {
		return nil
	}
	redacted := redactSecrets(err.Message(), secrets...)
	if redacted == err.Message() {
		return err
	}
	switch {
	case err.IsSessionExpired():
		return core.NewSessionExpired(redacted)
	case err.IsFixtureMiss():
		return core.NewFixtureMiss(redacted)
	case err.Kind() == core.KindUpstream:
		return core.NewUpstream(err.Status(), redacted)
	default:
		return newByKind(err.Kind(), redacted)
	}
}

// newByKind 按类别构造一条错误（[redactError] 的重建表）。
//
// 表必须**全**：漏一档会让"未知类别"悄悄变成 `Internal`，那正好把
// "凭据失效"洗成"内部错误"。所以这里逐个 `case` 写完，default 只在
// 将来 `ErrorKind` 加了新值时才会命中 —— 那一格靠 review 而不是靠运行时。
func newByKind(kind core.ErrorKind, msg string) *core.Error {
	switch kind {
	case core.KindAuth:
		return core.NewAuth(msg)
	case core.KindRateLimited:
		return core.NewRateLimited(msg)
	case core.KindSoldOut:
		return core.NewSoldOut(msg)
	case core.KindInsufficientCloud:
		return core.NewInsufficientCloud(msg)
	case core.KindTimeout:
		return core.NewTimeout(msg)
	case core.KindNetwork:
		return core.NewNetwork(msg)
	case core.KindUpstream:
		return core.NewUpstream(0, msg)
	case core.KindBadRequest:
		return core.NewBadRequest(msg)
	case core.KindNotFound:
		return core.NewNotFound(msg)
	case core.KindParse:
		return core.NewParse(msg)
	}
	return core.NewInternal(msg)
}
