// 139 邮箱**资产侧**那三口 —— 玩法 **1004 / 1014 / 1020** 要的那一发发请求。
//
//	⑤ POST html5.mail.10086.cn/mw2/file/disk?func=disk:surplusNum            → 转换权益（**只读**）
//	⑥ POST html5.mail.10086.cn/mw2/file/disk?func=disk:convertPdfFromYunFile → 把网盘里一个文件转成 PDF（**写**）
//	⑦ POST appmail.mail.10086.cn/RmWeb/mail?func=mbox:getComposeId           → 取一封新邮件的 composeId
//	⑧ POST mail.10086.cn/mailsms/sms/backup?sid=<sid>                        → 「邮箱备份」那一条任务的动作本身
//
// # 这一族与 [`app_mail139.go`](app_mail139.go) 那四枪的关系
//
// 那四枪负责**换到一张会话**（`sid` + `rmkey`），本文件负责**花那张会话**。四枪里 ③④
// 在 2026-09-26 探针真跑时回的是 `404`（`docs/capture/app-mail139-samples.txt` §1），
// 而 ③ 打的正是 `mail.10086.cn` 这一台 —— 所以本文件 ⑧ 那一口（同一台、同一个 `sid`）
// 今天大概率也是 404：旧实现自己那份运行日志里就写着它 404
// （`mcloud-sign/TASKS.md:79`：账号 2 的 `邮箱备份请求失败 [POST] https://mail.10086.cn/mailsms/sms/backup?sid=…: 404`）。
// ⚠ 那一句是**旧实现的一侧事实**，不是我们的判据；它的作用只是说明"这一口 404 并非本仓独有"。
//
// 反过来，⑤⑥⑦ 打在**另外两台**（`html5.mail.10086.cn` 与 `appmail.mail.10086.cn`）。
// 这一格是本轮真正想读的东西：`sid` 与 `rmkey` 是 ② 换到的、② 已实测能换到（`S_OK`），
// 若 ⑤ 在 `html5` 那台回 `S_OK` ⇒ "③④ 的 404 是**那一条路由**不在，不是这张会话无效"，
// R35 剩下的那一半就被这一发收窄（问句挂在 `docs/pending-decisions.md` R35）。
// 反过来若 ⑤ 也 404 / 回非 `S_OK`，那也是读数 —— 它会把范围从"两条路由"扩到"整片 web 入口"。
//
// # 证据面：**四口全部零实拍**
//
// `har/` 与夹具里一条都没有（2026-09-26 按 `mailsms/sms/backup`、`convertPdfFromYunFile`、
// `getComposeId`、`surplusNum` 四个串逐份扫过，零命中）。形状**全部**取自旧 TS
// `packages/core/src/apis/mail139.ts:73-160`，而旧实现那一族在跑（它的 1020 那一发
// 至少真打出去过 —— 打成 404 也算"发出去过"）。⚠ 所以本文件落的是**形状**，
// 不是"上游认这一套"：AGENTS.md §1.13 的处置是"接口层冻结、业务层继续 + 去跑一次"，
// 跑的结果由探针那一趟交回（`internal/app/live_probe_test.go`），不写进本文件的注释。
//
// # 凭据边界：四口**每一口**都带凭据
//
// `sid` 进查询串（⑤⑥⑦⑧ 全都是），`rmkey` 进 `Cookie`（⑤⑥⑦）。而 `internal/io` 的
// 传输层错误**把 URL 原样嵌进错误文本**（实测形状 `Post "https://…?sid=…": dial tcp …`）
// ⇒ 四口的错误一律过 [redactError]，且调用方不得把 [MailSession] 整块 marshal。
// ⚠ ⑤⑥ 的 `Cookie` 里那枚 `rmkey` 同样要防：本层不打印请求头，但 fixture 落盘那条路
// 会记头，所以头值只在这一句里出现一次，不进任何被返回的结构。
package upstream

import (
	"context"
	"fmt"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// 这一族自己的主机、路径与体里的字面量。
//
// 与上一族同一取向：**素材字面量整串照抄**，不按仓库里其它族的常量重拼。
// ⚠ `html5` 与 `appmail` 两枚**都是** `mail.10086.cn` 的子域，因此它们都命中
// [yun139AuthHostSuffixes] 那条"这台主机要带 authorization"的规则 —— 但旧 TS 这三口
// 走的是**原生 `fetch`**（文件头那句"避免 HTTP 客户端干扰"就是它自己的注释），
// 根本没过主实例 ⇒ 上游收到过的那一份**不带** authorization。这里照它。
const (
	mailHTML5Host   = "https://html5.mail.10086.cn"
	mailAppMailHost = "https://appmail.mail.10086.cn"
	// mailDiskPath 是 ⑤⑥ 那两口的路径（`mw2/file/disk`，两口同一路径、只换 `func`）。
	mailDiskPath = "/mw2/file/disk"
	// mailRmWebPath 是 ⑦ 的路径；⑧ 走 [BaseMail139] 上的 `mailsms/sms/backup`。
	mailRmWebPath  = "/RmWeb/mail"
	mailBackupPath = "/mailsms/sms/backup"
	// mailDiskAppType / mailDiskAppVer / mailDiskChannel 出现在 ⑤⑥ 两口的体里，
	// 且 ⑤ 的体**只有**这三键（旧 TS `mail139.ts:96-101`）。照抄。
	mailDiskAppType = "1"
	mailDiskAppVer  = "10.0.3"
	mailDiskChannel = "10242000"
	// mailDiskComeFrom 是查询串末尾那枚 `comefrom=166`。
	//
	// ⚠ 它**不是** [Mail139SourceID]（`001003`）：两枚数字不同族不同含义，
	// 这里没有互搬的余地（AGENTS.md §1.12 那条"跨端跨发起方常量不许互搬"同一取向）。
	mailDiskComeFrom = "166"
	// mailAppMark 是 ⑤⑥ 的 `X-Requested-With`（旧 TS 写死 `cn.cj.pe` = 邮箱原生 App 的包名族）。
	//
	// ⚠ 这一枚与 ② 那一条（iPad WebView、没有 `X-Requested-With`）是**两份不同的发起方**，
	// 别拿 ② 的头集合套这一族。
	mailAppMark = "cn.cj.pe"
	// mailAndroidWebViewUserAgent 是旧 TS `OKHTTP_UA`：小米 22041216C / Android 14 / Chrome 128 的
	// WebView 串。它与本仓 [AppProfile] 那套原生头、与 ② 实拍那枚 iPad UA **都不同**。
	// ⚠ 别"顺手统一"成仓库里现成的 UA：这一族上游收到过的就是这一枚（旧实现在跑），
	// 而它到底认不认 UA **未测试**（零样本 ⇒ 只有真跑一次并回读上游说的话才答得出）。
	mailAndroidWebViewUserAgent = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) " +
		"AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36"
	// mailXMLContentTypeTight 是旧 TS `buildHeaders` 与 `getComposeId` 那两处的字面量。
	//
	// ⚠ 刻意**不**复用 [mailXMLContentTypeSpaced]：两枚都是"上游收到过的那一份"，
	// 但来自不同接口，谁也没被证实过"另一枚也行"（AGENTS.md §1.4：线格式属于冻结面，
	// 不在无判据时统一）。
	mailXMLContentTypeTight = "application/xml;charset=UTF-8"
)

// mailDiskFuncConvertPDF 与 mailDiskFuncSurplusNum 是 ⑤⑥ 的 `func` 值。
const (
	mailDiskFuncSurplusNum = "disk:surplusNum"
	mailDiskFuncConvertPDF = "disk:convertPdfFromYunFile"
	mailRmWebFuncComposeID = "mbox:getComposeId"
)

// ─────────────────────────────────────────────────────────────────────────────
// 共同的查询串与会话头
// ─────────────────────────────────────────────────────────────────────────────

// mailDiskQuery 拼 ⑤⑥ 那一条 URL，逐字按旧 TS `buildQuery`（`mail139.ts:73-77`）：
// `…/mw2/file/disk?func=<func>&sid=<sid>&behaviorData=&rnd=<rnd>&cguid=<cguid>&k=&comefrom=166`。
//
// 两个随机值由**调用方**给（本层是纯层，不许自取随机数）：
//   - `cguid` 旧实现是 `"0" + Math.floor(Math.random()*1e8)`（**不是** UUID；② 那一发才是 UUID 形态）；
//   - `rnd` 旧实现是 `Math.random().toFixed(15)`，即 `0.xxxxxxxxxxxxxxx`。
//
// ⚠ 空值不当"没有这一枚参数"处理，而是**直接报错不发**：`behaviorData=` 与 `k=` 那两枚
// 本来就是空值参数，把 `rnd` / `cguid` 也拼成空会发出一份**形状不同**的 URL，
// 而表现是"这一口一直失败" —— 最难归因的那一类（同 [MailInLoginRequest] 里 `cguid` 的处置）。
//
// `sid` 走 [PercentEncode] 而不是旧实现那样裸拼：实拍 ② 换到的那枚是 hex 形态 ⇒
// 编码是恒等变换，而它挡的是"值里带 `+` 或 `&` 时**静默**变成另一份请求"
// （与 ③ 同一处置，理由见 [`app_mail139.go`](app_mail139.go) 的 [MailAutoLoginRequest] 头注）。
func mailDiskQuery(funcName string, session MailSession, cguid, rnd string) (string, *core.Error) {
	if !session.OK() {
		return "", core.NewInternal("邮箱资产侧那两口缺会话：这张票没有 sid 或 rmkey（错误文本刻意不带它们的值）")
	}
	if cguid == "" || rnd == "" {
		return "", core.NewInternal(fmt.Sprintf(
			"邮箱资产侧缺随机参数：cguid 与 rnd 都要有（实得 cguid 非空=%v、rnd 非空=%v）⇒ 少一枚就是换了一条接口",
			cguid != "", rnd != ""))
	}
	return fmt.Sprintf("%s%s?func=%s&sid=%s&behaviorData=&rnd=%s&cguid=%s&k=&comefrom=%s",
		mailHTML5Host, mailDiskPath, funcName,
		PercentEncode(session.SID.Value()), PercentEncode(rnd), PercentEncode(cguid), mailDiskComeFrom), nil
}

// mailDiskSessionCookie 拼 ⑤⑥ 那枚 `Cookie`（旧 TS `buildHeaders` 里那一串）。
//
// 顺序照旧实现：`RMKEY=<rmkey>; Os_SSo_Sid=<sid>`。⚠ 这一串含凭据 ⇒ 只准直接进头，
// 不得进错误文本、日志或夹具真值（[redactError] 保不住**头**，所以头从来不出现在出口值里）。
func mailDiskSessionCookie(session MailSession) string {
	return "RMKEY=" + session.RMKey.Value() + "; Os_SSo_Sid=" + session.SID.Value()
}

// mailObjectOpen 与 mailObjectClose 是这一族 XML 体的外壳。
const (
	mailObjectOpen  = "<object>"
	mailObjectClose = "</object>"
)

// mailDiskAppFields 是 ⑤⑥ 两口体末尾共用的那三键（旧实现把它放在**最后**）。
func mailDiskAppFields() string {
	return "<string name=\"appType\">" + mailDiskAppType + "</string>" +
		"<string name=\"appVer\">" + mailDiskAppVer + "</string>" +
		"<string name=\"channel\">" + mailDiskChannel + "</string>"
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑤ 枪：surplusNum（查 PDF 转换权益，**只读**）
// ─────────────────────────────────────────────────────────────────────────────

// MailSurplusNumRequest 构造 `POST …/mw2/file/disk?func=disk:surplusNum&…`。
//
// 体只有那三枚应用字段（旧 TS `mail139.ts:96-101` 的模板字符串里就是这些，
// 带换行与两空格缩进 ⇒ 这里也**带**：那是上游收到过的那一份字节）。
//
// 这一口是**只读**的，也是这一族里唯一可以随手打一发拿读数的口子 ——
// 探针要验"这张会话在 `html5` 那台认不认"，打它就是最低成本的一枪。
func MailSurplusNumRequest(session MailSession, cguid, rnd string) (core.HTTPRequest, *core.Error) {
	query, perr := mailDiskQuery(mailDiskFuncSurplusNum, session, cguid, rnd)
	if perr != nil {
		return core.HTTPRequest{}, perr
	}
	body := "<object>\n  <string name=\"appType\">" + mailDiskAppType + "</string>\n" +
		"  <string name=\"appVer\">" + mailDiskAppVer + "</string>\n" +
		"  <string name=\"channel\">" + mailDiskChannel + "</string>\n</object>"
	request := core.NewRequestPost(query).
		WithHeader("content-type", mailXMLContentTypeTight).
		WithHeader("accept", "*/*").
		WithHeader("cookie", mailDiskSessionCookie(session)).
		WithHeader("origin", mailHTML5Host).
		WithHeader("referer", mailHTML5Host+"/").
		WithHeader("user-agent", mailAndroidWebViewUserAgent).
		WithHeader("x-requested-with", mailAppMark)
	request.Body = core.Body(body)
	return request, nil
}

// ParseMailSurplusNum 从 ⑤ 的响应里取 `var.benefitSurplusCount` 与 `var.benefitStatus`。
//
// 两枚都按**字符串**读（旧 TS 那个 interface 就是 `string`×2）。取不到不算失败：
// 这一口没有"必须有什么"的判据在手（形状零样本），所以交回空串由调用方决定怎么念。
// 解不开响应体**才**算失败（那一档说明发过去的东西上游没认，见 [parseMailAssetEnvelope]）。
func ParseMailSurplusNum(response core.HTTPResponse) (string, string, *core.Error) {
	value, perr := parseMailAssetEnvelope(response, "邮箱转换权益")
	if perr != nil {
		return "", "", perr
	}
	obj, _ := value.(map[string]any)
	inner, _ := obj["var"].(map[string]any)
	count, _ := inner["benefitSurplusCount"].(string)
	status, _ := inner["benefitStatus"].(string)
	return count, status, nil
}

// AppMailSurplusNum 发第 ⑤ 枪。**只读**。
func (u *Upstream) AppMailSurplusNum(
	ctx context.Context, session MailSession, cguid, rnd string,
) (string, string, *core.Error) {
	request, perr := MailSurplusNumRequest(session, cguid, rnd)
	if perr != nil {
		return "", "", perr
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return "", "", redactError(perr, session.SID, session.RMKey)
	}
	count, status, perr := ParseMailSurplusNum(response)
	return count, status, redactError(perr, session.SID, session.RMKey)
}

// MailPDFConvertType 是玩法 **1014** 那一发用的 `convertType`。
//
// 它是**旧实现两处调用点共同的那个值**（`runners.ts` 的 `pdfConvertTask` 写死 `1`），
// 不是"1 = 转 PDF"这种我们编得出来的语义 —— 上面 [MailConvertPDFRequest] 头注成文了
// "这四档的语义没有任何成文，本层不解释"。之所以在这里给它一个名字而不让调用方写字面量：
// 让"四档语义未知、本仓只用旧实现那一档"这件事在**调用点**可读。
const MailPDFConvertType = 1

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑥ 枪：convertPdfFromYunFile（**写**：转换产物会进网盘）
// ─────────────────────────────────────────────────────────────────────────────

// MailConvertPDFRequest 构造 `POST …/mw2/file/disk?func=disk:convertPdfFromYunFile&…`。
//
// 体的**五键与顺序**逐字取自旧 TS（`mail139.ts:119-140`）：
// `fileName`(string) → `fileRealSize`(**int**，注意标签不是 string) → `fileId`(string)
// → `convertType`(int) → 然后才是那三枚应用字段。
// ⚠ `convertType` 旧实现的默认是 `1`，而类型域是 `1|2|3|4`（"转成什么"四档）。
// **本仓不解释这四档**：没有任何成文说过 2/3/4 分别是什么，探针也没打过 ——
// 传进来的值原样落进体，越界不判（判了就是我们从零样本里编规则）。
//
// `fileName` 与 `fileId` 都过 [escapeXMLText]：前者由编排层生成（今天的形状是 ASCII + 时间戳，
// 转义是死代码），后者**来自上游响应** ⇒ 它不是我们能假定字符集的东西。
func MailConvertPDFRequest(
	session MailSession, cguid, rnd, fileName, fileID string, fileRealSize int64, convertType int,
) (core.HTTPRequest, *core.Error) {
	query, perr := mailDiskQuery(mailDiskFuncConvertPDF, session, cguid, rnd)
	if perr != nil {
		return core.HTTPRequest{}, perr
	}
	if fileID == "" {
		return core.HTTPRequest{}, core.NewInternal(
			"PDF 转换缺 fileId：它是这一个体的主键，空串等于让上游猜转哪一个")
	}
	if fileRealSize < 0 {
		return core.HTTPRequest{}, core.NewInternal(fmt.Sprintf(
			"PDF 转换的 fileRealSize 不能是负数，实得 %d（负数不是“没这个字段”，是一条坏请求）", fileRealSize))
	}
	body := mailObjectOpen + "\n  " +
		"<string name=\"fileName\">" + escapeXMLText(fileName) + "</string>\n  " +
		"<int name=\"fileRealSize\">" + strconv.FormatInt(fileRealSize, 10) + "</int>\n  " +
		"<string name=\"fileId\">" + escapeXMLText(fileID) + "</string>\n  " +
		"<int name=\"convertType\">" + strconv.Itoa(convertType) + "</int>\n  " +
		mailDiskAppFields() + "\n" + mailObjectClose
	request := core.NewRequestPost(query).
		WithHeader("content-type", mailXMLContentTypeTight).
		WithHeader("accept", "*/*").
		WithHeader("cookie", mailDiskSessionCookie(session)).
		WithHeader("origin", mailHTML5Host).
		WithHeader("referer", mailHTML5Host+"/").
		WithHeader("user-agent", mailAndroidWebViewUserAgent).
		WithHeader("x-requested-with", mailAppMark)
	request.Body = core.Body(body)
	return request, nil
}

// AppMailConvertPDF 发第 ⑥ 枪。**写请求**：转换产物会落进用户网盘。
//
// 判据：`code == "S_OK"` ⇒ 认这一发做成了（旧 TS `runners.ts:336-340` 就是这一句，
// 失败时它打 `code` 与 `summary`）。⚠ 它**没有**读 `var` 里任何东西，所以那一格
// 形状仍然零样本 —— 别在这里加"从 var 取产物 fileId"的推断。
//
// 产物进不进 [CloudFileIDs] 那一类清理账本由**调用方**决定：这一口产出的那个 PDF
// 是不是就是传进去的那一个 `fileId`（转换 = 原地换内容，还是另存一份），
// 本仓**未测试**，所以不敢替它登记删除。
func (u *Upstream) AppMailConvertPDF(
	ctx context.Context, session MailSession, cguid, rnd, fileName, fileID string,
	fileRealSize int64, convertType int,
) *core.Error {
	request, perr := MailConvertPDFRequest(session, cguid, rnd, fileName, fileID, fileRealSize, convertType)
	if perr != nil {
		return perr
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return redactError(perr, session.SID, session.RMKey)
	}
	value, perr := parseMailAssetEnvelope(response, "邮箱 PDF 转换")
	if perr != nil {
		return redactError(perr, session.SID, session.RMKey)
	}
	obj, _ := value.(map[string]any)
	code, _ := obj["code"].(string)
	if code == mailAssetOKCode {
		return nil
	}
	summary, _ := obj["summary"].(string)
	return core.NewUpstream(response.Status, "邮箱 PDF 转换没认：上游 code="+
		clipRunes(codeOrUnknown(code), 60)+assetSummaryHint(summary))
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑦ 枪：getComposeId（取一封新邮件的草稿号）
// ─────────────────────────────────────────────────────────────────────────────

// MailComposeIDRequest 构造 `POST appmail.mail.10086.cn/RmWeb/mail?func=mbox:getComposeId&sid=<sid>`。
//
// 头**只有四枚**（旧 TS `mail139.ts:145-157` 那份原生 fetch）：`cookie: RMKEY=<rmkey>`
// （⚠ 这一口**不带** `Os_SSo_Sid`，与 ⑤⑥ 那两口的头集合不同）、`content-type`、`accept`、
// `user-agent`。体是**空串** —— 不是"没有 body"也不是 `{}`：旧实现传的就是 `body: ""`，
// 而 `content-type` 仍写 xml。
//
// 这一口本身**不发邮件**：它只要一个草稿号。真正有副作用的是拿到号之后的
// `mbox:compose`（旧 TS `mail139.ts:163+`，本仓**刻意不落码** —— 那一发是真发一封邮件，
// 收件人还是配置项，已挂在 `docs/pending-decisions.md` 待用户拍板）。
// 所以"⑦ 接上"不等于"1004 可用"：1004 要的是 ⑦ + compose 两发。
func MailComposeIDRequest(session MailSession) (core.HTTPRequest, *core.Error) {
	if !session.OK() {
		return core.HTTPRequest{}, core.NewInternal(
			"取 composeId 缺会话：这一口要 sid（查询串）与 rmkey（Cookie）各一枚")
	}
	request := core.NewRequestPost(mailAppMailHost+mailRmWebPath+
		"?func="+mailRmWebFuncComposeID+"&sid="+PercentEncode(session.SID.Value())).
		WithHeader("cookie", "RMKEY="+session.RMKey.Value()).
		WithHeader("content-type", mailXMLContentTypeTight).
		WithHeader("accept", "*/*").
		WithHeader("user-agent", mailAndroidWebViewUserAgent)
	request.Body = core.Body("")
	return request, nil
}

// ParseMailComposeID 从 ⑦ 的响应里取草稿号，**照旧实现读两种形状**。
//
// 旧 TS `runners.ts:243-249`：`code === "S_OK"` 且 `var` 是**字符串**就直接用；
// `var` 是**对象**时读 `.m`，再退 `.id`，两个都没有就当没取到。
// ⚠ 这两种到底哪一种是上游今天给的，**未测试**（零样本）—— 落两种读法不是因为
// "我们知道它会变"，是因为旧实现两处都写了而它只跑到过其中一种（跑到过哪种也没成文）。
// 取不到 ⇒ 交回 [core.KindParse] 的错误，**不带响应体**（这一族的失败响应里可能塞票）。
func ParseMailComposeID(response core.HTTPResponse) (string, *core.Error) {
	value, perr := parseMailAssetEnvelope(response, "邮箱草稿号")
	if perr != nil {
		return "", perr
	}
	obj, _ := value.(map[string]any)
	code, _ := obj["code"].(string)
	if code != mailAssetOKCode {
		summary, _ := obj["summary"].(string)
		return "", core.NewUpstream(response.Status, "邮箱草稿号没认：上游 code="+
			clipRunes(codeOrUnknown(code), 60)+assetSummaryHint(summary))
	}
	switch v := obj["var"].(type) {
	case string:
		if v != "" {
			return v, nil
		}
	case map[string]any:
		for _, key := range []string{"m", "id"} {
			if text, isStr := v[key].(string); isStr && text != "" {
				return text, nil
			}
		}
	}
	// 上面两条支路都没接住 ⇒ 键名是**唯一不含值的线索**（同 ② 那一格的处置，坑表 #166：
	// [topLevelKeys] 按键名升序交回，所以这一串可以稳定比对）。
	return "", core.NewParse("邮箱草稿号解不出值（状态 " +
		strconv.FormatUint(uint64(response.Status), 10) + "，顶层键：" + topLevelKeys(value) +
		mailCodeHint(value) + "）⇒ 1004 那一发不发，composeId 两档读法都没接住")
}

// AppMailComposeID 发第 ⑦ 枪。它**不**发邮件（见 [MailComposeIDRequest] 头注）。
func (u *Upstream) AppMailComposeID(
	ctx context.Context, session MailSession,
) (string, *core.Error) {
	request, perr := MailComposeIDRequest(session)
	if perr != nil {
		return "", perr
	}
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return "", redactError(perr, session.SID, session.RMKey)
	}
	composeID, perr := ParseMailComposeID(response)
	return composeID, redactError(perr, session.SID, session.RMKey)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ⑧ 枪：mailsms/sms/backup（玩法 1020 的那一发动作）
// ─────────────────────────────────────────────────────────────────────────────

// MailBackupRequest 构造 `POST mail.10086.cn/mailsms/sms/backup?sid=<sid>`，体是 `{}`。
//
// 这一口与 ③ 同一台、同一个 `sid`，也**同样**经旧实现的主实例发（`runners.ts:287`
// 用的是 `ctx.http.post`，而 `mail.10086.cn` 在 `AUTH_DOMAINS` 里）⇒ 上游收到过的那一份
// **带** authorization。所以这里与 ③ 同一处置：默认带上调用方给的那枚，传空则不带。
//
// ⚠ 旧实现对这一口**只判"有没有抛异常"**（`tryRequest` 吞掉响应），连状态码都不看。
// 本仓不照抄这一条：见 [Upstream.AppMailBackup]。
func MailBackupRequest(session MailSession, authorization string) (core.HTTPRequest, *core.Error) {
	if !session.OK() {
		return core.HTTPRequest{}, core.NewInternal(
			"邮箱备份缺会话：这一口的 URL 只有 sid 一枚凭据，但换不到它就没有 sid（错误文本刻意不带值）")
	}
	request := core.NewRequestPost(BaseMail139+mailBackupPath+
		"?sid="+PercentEncode(session.SID.Value())).
		WithHeader("accept", "*/*").
		WithHeader("content-type", "application/json").
		WithHeader("user-agent", mailAndroidWebViewUserAgent)
	if authorization != "" {
		request = request.WithHeader("authorization", authorization)
	}
	request.Body = core.Body("{}")
	return request, nil
}

// AppMailBackup 发第 ⑧ 枪。**是否算完成由复查那一趟读数说**，这一层只报"上游认不认这一发"。
//
// 判据比旧实现**多**一档：非 2xx 直接交回错误。旧实现那句 `tryRequest` 会把 404 吞成
// "已提交"，而那正是本仓反复批评的"失败静默"（同 [`app_mail139.go`](app_mail139.go) 里
// `triggerMailPEvent` 那一格）。
// ⚠ 但**别把 2xx 读成"任务做完了"**：这一口成功响应长什么样零样本（本层不解析体），
// 2xx 只到"上游没拒"。1020 到底认不认，看编排层复查 `taskListV2` 那一趟的读数。
func (u *Upstream) AppMailBackup(
	ctx context.Context, session MailSession, authorization string,
) *core.Error {
	request, perr := MailBackupRequest(session, authorization)
	if perr != nil {
		return perr
	}
	_, perr = u.Raw(ctx, request)
	return redactError(perr, session.SID, session.RMKey)
}

// ─────────────────────────────────────────────────────────────────────────────
// 这一族共同的响应解析出口
// ─────────────────────────────────────────────────────────────────────────────

// mailAssetOKCode 是这一族的成功码字面量。
//
// 它与 [EnvelopeOKCode]（签到族那个数字 `0`）**不是一回事**：这一族回的是字符串 `S_OK`
// （旧 TS 多处与 `code:"PARSE_ERR"` 相对）。别统一。
const mailAssetOKCode = "S_OK"

// parseMailAssetEnvelope 把响应文本解成 JSON 并**只**验"解得开、且是个对象"。
//
// 它**不**判 `code` —— 三口对 `code` 的处置各不相同（⑤ 不判、⑥ 判 `S_OK`、
// ⑦ 判 `S_OK` 且还要读 `var`），在这一层判会把差别抹平。
//
// ⚠ 错误文本一律**不带响应体**：这一族失败时最可能出现的就是票（② 那一族的先例是
// 上游把提交回来的东西 echo 进错误消息），所以只交回状态码、字节数、顶层键名与
// `code` / `summary` 两枚字符串（[topLevelKeys] + [mailCodeHint]）。
// 解析走 [parseMailJSONText]：这一族回**单引号 JSON** 是旧实现写死的事实
// （`parseLooseJson` 先把 `'` 换成 `"`），所以"先严后宽"那两遍在这里是**有出处**的
// —— 与 ② 那一发（同函数被用，但那边是外推）不同。
func parseMailAssetEnvelope(response core.HTTPResponse, what string) (any, *core.Error) {
	value, ok := parseMailJSONText(string(response.Body))
	if !ok {
		return nil, core.NewParse(fmt.Sprintf(
			"%s响应解不开（标准 JSON 与这一族那套单引号两种都试过，%d 字节，状态 %d）"+
				"⇒ 这一族的失败响应形状零样本，先看状态码再看键名",
			what, len(response.Body), response.Status))
	}
	if _, isObj := value.(map[string]any); !isObj {
		return nil, core.NewParse(fmt.Sprintf(
			"%s响应不是对象（状态 %d，%d 字节）⇒ 本层不猜它是什么", what, response.Status, len(response.Body)))
	}
	return value, nil
}

// codeOrUnknown 把"上游没给 code"与"给了空串"都念成同一句人话，且不放大载荷。
func codeOrUnknown(code string) string {
	if code == "" {
		return "没有 code"
	}
	return code
}

// assetSummaryHint 补一句 `summary`（只有它是**字符串**时才补，且截断）。
//
// 旧实现在失败那一条支路上打的是 `logger.fail("PDF转换失败", r?.code, r?.summary)` ⇒
// 这一句是"上游自己说的话"，与 [mailCodeHint] 同一取向；不复用它是因为这里已经有
// `code=` 那一小句，两者拼出来的文本形状不同（用例钉的是这一种）。
func assetSummaryHint(summary string) string {
	if summary == "" {
		return ""
	}
	return "，summary=" + clipRunes(summary, 120)
}
