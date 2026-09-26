package app

// 139 邮箱那一半（玩法 **472 / 473「体验 139 邮箱」**）：四枪一串，跨两台主机。
//
//	① POST user-njs.yun.139.com/user/querySpecTokenV2   {toSourceId:"001003"}   → SSO 票
//	② POST mail.10086.cn/login/inlogin.action?cguid=…                            → sid + rmkey
//	③ GET  mail.10086.cn/login/autologin.action?sid=…&rmkey=…                    → 不判响应
//	④ POST caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2            → code=0
//
// 四枪的请求面与判定全在 `internal/upstream/app_mail139.go`（那里成文了哪两枪有实拍、
// 哪两枪零样本）。这一层只管**编排**：什么时候不发、发失败时念什么、错误的先后。
//
// # 为什么这一条与 1004 / 1020 / 1014 / 1028 只有"前三枪"是共用的
//
// 旧实现那四条各自在拿到会话之后打**自己那一口**（`mbox:compose` 真发一封信、
// `mailsms/sms/backup`、PDF 转换、AI 工作台），而那些口本仓一个都没有 ⇒ 它们仍留在
// [taskMatrixToolGaps]。本文件只兑现"体验邮箱"这一条：它后面那一发就是 ④，
// 而 ④ 是签到族自己的 click 的 V2 变体 —— 这一类枪本仓已经会打。
//
// ⚠ 本文件**没有** PE 活跃上报。真流量里 inlogin 之后确实紧跟一发 PE，但那一发的
// 发起方是"打开了某个邮箱页面"而不是任务 472；旧实现里 `triggerMailPEvent` 也只被
// 1004 那一族用。⇒ "472 要不要发 PE"登记成问句（`docs/pending-decisions.md`），不在此落码。
//
// # 播报为什么按"哪一枪挂了"分档
//
// ①②③ 挂的是邮箱那两侧的会话（④ 压根没发出去，任务自然没被认）；③ 挂了而 ④ 成功时，
// 这一条是"活跃没做成、但任务记上了"。两档的处置不同：前者去看邮箱侧的响应，
// 后者回签到族看 `58999`（坑表 #149）。混成一句"邮箱任务失败"等于两问都不答。
//
// # 这一族**不往用户账号留产物**
//
// 不传文件、不建笔记、不发信 ⇒ [TaskMatrixRunResult.CloudFileIDs] 那一条清理链与它无关，
// 链尾也不该为它发 `batchTrash`。它唯一的"产物"是 ④ 记上的那条任务进度，
// 而那一格**不可回滚**（没有撤销打卡那一口）。

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// mailSessionFor 取这一趟的邮箱会话（②的产物），**同一趟只换一次**（①②各一发）。
//
// 第二格 fired 是"这一句里有没有枪真的出过网"（口径见 [apply]）：① 已经发出去了，
// 哪怕 ② 没换到会话也算动过 —— 它真的把登录态发给了邮箱那台主机。
//
// ⚠ **只在成功时缓存**（照旧实现 `ctx.state.mail139`：它读的是
// `cached?.sid && cached?.rmkey`，而写入在 `loginEmail` 返回非 null 之后）。
// 失败不缓存的后果照它写清楚：下一任务（473）会**再走一遍** ①②。
// 不跟着改成"失败也记一笔、整趟放弃"的理由：票很短命（`app_mail139.go` 文件头引的实拍
// —— 换到 3 分 15 秒后重放就被拒 `200050409`），而失败原因里"被风控"与"这一次网络抖了"
// 在响应上分不开；一次失败拖垮整趟，等于把瞬时故障放大成"这个号今天没有邮箱任务"。
// 这一格最坏的表现是**多打两枪**，不是多写一份用户的账。
//
// ⚠ 下面两格"缺接线"的判断**未被断言**（AGENTS.md §1.3）：这一条链在**拉表**那一步就要
// 用随机源与时钟（`AppTaskMatrix` → [do] → 建会话），两者为 nil 时整趟早就 panic 在
// 到不了这里的地方 ⇒ 造不出一份"跑到这一句而 Random 是 nil"的夹具。留着它们的理由是
// [UserTimestamp] 与 [core.UUIDv4] 都是 nil 接口直取（**panic，不是回零值**），
// 而这一族将来可能被别的入口复用（不经过拉表）。**不要**据此以为"缺接线时会安全停下"
// —— 今天真实的表现在到达这里之前就有了。
func (e *taskExecutor) mailSessionFor(
	ctx context.Context,
) (upstream.MailSession, bool, []string) {
	if e.mailSession != nil && e.mailSession.OK() {
		return *e.mailSession, false, nil
	}
	if e.run.Random == nil {
		return upstream.MailSession{}, false, []string{
			"邮箱会话没换：随机源没接，造不出 ② 那一发的 cguid（本工具侧缺接线）",
		}
	}
	if e.run.Clock == nil {
		// ⚠ 必须先判 nil 再用它：[UserTimestamp] 是 `clock.Now()` 直取，接口为 nil 时
		// **panic**（不是回 0）—— 与 [nowMillis] 那里同一处置，理由也在那儿：
		// 一个零值签名时间戳会照常发出去，而上游看到的是 1970 年，比"这条没做成"难查得多。
		return upstream.MailSession{}, false, []string{
			"邮箱会话没换：时钟没接，① 的签名时间戳取不到（本工具侧缺接线）",
		}
	}
	timestamp := UserTimestamp(e.run.Clock)
	ssoToken, perr := e.src.up.AppQuerySpecToken(ctx, e.opening.Profile,
		timestamp, e.opening.Credential.Authorization(), upstream.Mail139SourceID)
	if perr != nil {
		return upstream.MailSession{}, true, []string{"换邮箱用的 SSO 票（①）失败：" + perr.Error() +
			" ⇒ ②③④ 不发，这一条没往邮箱那边写任何东西"}
	}
	session, perr := e.src.up.AppMail139Login(ctx, ssoToken, core.UUIDv4(e.run.Random))
	if perr != nil {
		// ⚠ 这一句里的错误文本已由协议层脱敏（`ParseMailSession` 那次 `redactSecrets`）；
		// 本层**不再** echo `ssoToken`，也不自己拼响应体（那正是"顺手 marshal 一下"的入口）。
		return upstream.MailSession{}, true, []string{"邮箱会话（②）没换到：" + perr.Error() +
			" ⇒ ③④ 不发，不会打这一条任务的卡"}
	}
	e.mailSession = &session // 只有成功才缓存（见函数头）
	return session, true, nil
}

// runMailExperience 跑"体验 139 邮箱"这一条玩法，交回（发过枪没有，失败原因）。
//
// 472 与 473 共用这一个函数：它们在任务表里是同名同组的两条，差别只在 ④ 里那个
// `taskId`（与 106 / 522 共用上传那一族同理）。
//
// ⚠ 顺序照旧实现：会话（①②）→ 活跃（③）→ 打卡（④），而 **③ 失败不挡 ④**。
// 旧实现那里是 `tryRequest` 吞掉错误、④ 无条件照发 ⇒ 我们跟着发，但把 ③ 的失败
// 念出来（本仓反复批评旧实现"上报失败却静默"，见 `app_mail139.go` 文件头）。
// 反过来说，③ 失败而 ④ 成功时这一条仍算"做了、上游认了"—— 认没认由**复查那趟**说话，
// 不由这一层猜。
func (e *taskExecutor) runMailExperience(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	session, fired, problems := e.mailSessionFor(ctx)
	if len(problems) > 0 {
		// 到这里必然是"①② 那一档没过去"：[mailSessionFor] 只有成功才交回可用会话，
		// 所以**不需要**再写一句"会话不可用就跳过"的兜底 —— 那一支没有可达路径，
		// 留着只会让人以为"发过 ③ 却没换到会话"是一种真实状态（AGENTS.md §1.3）。
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problems[0]))
		return fired, problems
	}
	// ③ 活跃那一发：它**不判响应**（为什么写不判，协议层头注成文了），
	// 所以这里唯一的失败是"没发出去 / 非 2xx"。
	fired = true
	if aerr := e.src.up.AppMailAutoLogin(ctx, session,
		e.opening.Credential.Authorization()); aerr != nil {
		problem := "邮箱活跃（③）失败：" + aerr.Error() + " ⇒ 仍照旧实现往下发 ④"
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problem))
		problems = append(problems, problem)
	} else {
		e.run.progress(core.LevelInfo, fmt.Sprintf("邮箱已活跃 %s %d", name, id))
	}
	// ④ 打卡：走签到族那套"拿票 → 发 → 撞 90001 刷一次重放一次"（[do]）。
	// ⚠ 这一发的 jwttoken 是**公共来源**（依据与"将来拍到特有取法要怎么改"写在
	// `upstream.MailMarketTaskClickRequest` 的头注，AGENTS.md §1.12）。
	_, cerr := do(ctx, e.src, func(s SignInSession) (struct{}, *core.Error) {
		return struct{}{}, e.src.up.AppMailMarketTaskClick(
			ctx, s.jwt, s.referer, upstream.MarketName, id)
	})
	if cerr != nil {
		problem := "邮箱市场打卡（④）失败：" + cerr.Error() +
			" ⇒ 前三枪做了、这一条任务上游没记上（" +
			"「活跃过」与「任务被认」是两件事，别把这一句读成邮箱没打开）"
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱任务失败 %s %d：%s", name, id, problem))
		return true, append(problems, problem)
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("邮箱市场已打卡 %s %d", name, id))
	return true, problems
}

// ─────────────────────────────────────────────────────────────────────────────
// 资产侧那两口（玩法 **1020 邮箱备份** / **1014 PDF 转换**）
// ─────────────────────────────────────────────────────────────────────────────

// mailDiskRandoms 造 ⑤⑥ 那两枚查询参数（形状按旧 TS：`cguid = "0" +
// Math.floor(Math.random()*1e8)`、`rnd = Math.random().toFixed(15)`）。
//
// 为什么在编排层而不是协议层：[upstream] 是纯层，不许自取随机数（AGENTS.md 那一族
// 口径成文在 `app_mail139_assets.go` 的 [mailDiskQuery] 头注），而这两枚**必须**每次不同
// —— 它们是旧实现里"看着像参数、其实是防缓存"的那一类（零样本说上游比过它们的值）。
//
// ⚠ 这里用 `% 10` 取十进制位，**不**做拒绝采样：本仓对取模偏差的要求在**设备指纹**
// 那一条（`core.Below` 的头注），这两枚值今天没有任何已知判据读它的分布。
// 将来若拍到"上游认 cguid 的取值范围"，改的是这一处，并顺带删掉这句话。
func mailDiskRandoms(r core.Random) (string, string) {
	var b [15]byte
	r.FillBytes(b[:])
	digits := make([]byte, 0, len(b))
	for _, v := range b {
		digits = append(digits, byte('0'+v%10))
	}
	return "0" + string(digits[:8]), "0." + string(digits)
}

// mailPDFAsset 是玩法 1014 要上传的那一份**最小 PDF**（名字 + 字节），逐字按旧 TS
// `runners.ts:285-311` 里 `pdfConvertTask` 拼的那一串（含那张 xref 表里的偏移量 ——
// 它是**手写死的**，与真实对象位置并不严格对应；照抄，不"顺手修正"：
// 上游收到过的就是这一份，而它读不读 xref **未测试**）。
//
// `ts` 由调用方给（同一个毫秒戳同时进文件名与正文 ⇒ 一趟里两份读数对得上）。
func mailPDFAsset(ts int64) (string, []byte) {
	uniqueText := "asign auto pdf " + strconv.FormatInt(ts, 10)
	name := "asign-pdf-" + strconv.FormatInt(ts, 10) + ".pdf"
	content := "" +
		"%PDF-1.4\n" +
		"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n" +
		"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n" +
		"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R " +
		"/Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n" +
		"4 0 obj\n<< /Length " + strconv.Itoa(len(uniqueText)+50) + " >>\nstream\n" +
		"BT\n/F1 18 Tf\n72 720 Td\n(" + uniqueText + ") Tj\nET\n" +
		"endstream\nendobj\n" +
		"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n" +
		"xref\n0 6\n0000000000 65535 f\n0000000009 00000 n\n0000000058 00000 n\n" +
		"0000000115 00000 n\n0000000223 00000 n\n0000000378 00000 n\n" +
		"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n444\n%%EOF\n"
	return name, []byte(content)
}

// runMailBackup 跑玩法 **1020「邮箱资产备份」**：会话（①②）→ ⑧ `mailsms/sms/backup`。
//
// ⚠ 它与 [runMailExperience] 的**形状差别**（这是判据，不是风格）：旧实现这一条
// 只取会话，**不打 ③、也不打 ④**（`runners.ts:160-165`），而它后面那一发就是任务本体
// ⇒ 通用前置那一发仍由 [PlanTaskStep] 按 `currstep` 门决定。把两条混成一个的表现：
// 要么给这一条多发一发 404 的市场打卡，要么该打的会话少一环。
//
// ⚠ ⑧ 是**写**：它请求的是"把这个号的短信/邮箱备份跑一次"。本仓**不判**它的响应
// （协议层成文：只把"没发出去 / 非 2xx"交回），所以念出来的是"已提交"，
// 而"这一条任务上游认没认"由复查那一趟说话。
func (e *taskExecutor) runMailBackup(
	ctx context.Context, item upstream.TaskV2Item,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	session, fired, problems := e.mailSessionFor(ctx)
	if len(problems) > 0 {
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱备份失败 %s %d：%s", name, id, problems[0]))
		return fired, problems
	}
	fired = true
	if berr := e.src.up.AppMailBackup(
		ctx, session, e.opening.Credential.Authorization(),
	); berr != nil {
		problem := "邮箱备份（⑧）失败：" + berr.Error()
		e.run.progress(core.LevelError, fmt.Sprintf("邮箱备份失败 %s %d：%s", name, id, berr.Error()))
		return true, append(problems, problem)
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("邮箱备份已提交 %s %d", name, id))
	return true, problems
}

// runMailPDF 跑玩法 **1014「PDF 转换」**：会话（①②）→ 往网盘传一份最小 PDF →
// ⑥ `disk:convertPdfFromYunFile`（`convertType:1`）。
//
// 三处与旧实现一致、各自"改了会怎样"：
//   - **顺序**：先换会话再传文件（旧实现 `pdfConvertTask` 排在 `getMail139Session` 之后）。
//     反过来会多出一种表现：会话都没换到，却已经在用户网盘里写了一个文件。
//   - **fileId 取自这一次上传的收据**，不是旧实现那句 `ctx.state.files` 的末位 ——
//     那是旧实现"共享可变状态"的偷懒（同一趟里前面别的任务也往里塞）。本仓按
//     收据取，表现是"转换的一定是这一条刚传上去的那一份"。⚠ 这是一处**与旧实现不同**
//     的取舍，理由与 434 那一格的教训同源（[taskRunnerSpec.Shares] 那一条脆弱点）。
//   - **传上去的那一份进清理链**（[taskExecutor.cloudFileIDs]）⇒ 链尾一次 `batchTrash`。
//     ⚠ **转换产物**（⑥ 在网盘里另存的那一份 `.docx`）**也**进这一条链，但走的是
//     [collectMailPDFProduct]：⑥ 的响应里**没有**产物的 fileId（`var.fileId` 两发实拍都是空串），
//     只有它交代的那个目录（`var.directoryId`）⇒ 得先列那个目录、按素材干名精确对上。
//     对不上时**念出来**（"产物没对上 ⇒ 它仍会留在网盘里"），不静默、也不假装清干净了。
//     2026-09-26 实拍：一号两发 S_OK 都在等过一档之后读到，且那次 `batchTrash` 之后
//     约两分钟的只读走查里它已不再被列出（当场那次回读**还在** ⇒ 列目录有延迟，见
//     `docs/capture/app-cloud-residue-samples.txt` §4）。
func (e *taskExecutor) runMailPDF(
	ctx context.Context, item upstream.TaskV2Item, record *TaskStepRecord,
) (bool, []string) {
	name, id := taskNameOf(item), derefTaskID(item)
	session, fired, problems := e.mailSessionFor(ctx)
	if len(problems) > 0 {
		e.run.progress(core.LevelError, fmt.Sprintf("PDF 转换失败 %s %d：%s", name, id, problems[0]))
		return fired, problems
	}
	fired = true
	// ⑥ 要一个真实存在的 fileId ⇒ 先传（**写**）。时间戳读不到就不传（AGENTS.md §1.7：
	// 一份 1970 年命名、内容里写着 0 的文件会照常留在用户网盘里）。
	ts, ok := e.nowMillis()
	if !ok {
		problem := "PDF 转换没做：时钟没接，造不出文件名与内容里的时间戳（本工具侧缺接线）"
		e.run.progress(core.LevelError, fmt.Sprintf("PDF 转换失败 %s %d：%s", name, id, problem))
		return true, append(problems, problem)
	}
	uploader, uerr := e.cloudUploader(ctx)
	if uerr != nil {
		problem := "PDF 转换没做：" + uerr.Error()
		e.run.progress(core.LevelError, fmt.Sprintf("PDF 转换失败 %s %d：%s", name, id, problem))
		return true, append(problems, problem)
	}
	fileName, content := mailPDFAsset(ts)
	receipt, perr := uploader.UploadContent(ctx, content, CloudUploadOptions{Name: fileName})
	if receipt.FileID != "" {
		e.cloudFileIDs = append(e.cloudFileIDs, receipt.FileID)
		record.Uploads++
	}
	if perr != nil {
		problem := "PDF 那份素材没传上去：" + perr.Error() + " ⇒ ⑥ 不发（没有 fileId 可转）"
		e.run.progress(core.LevelError, fmt.Sprintf("PDF 转换失败 %s %d：%s", name, id, perr.Error()))
		return true, append(problems, problem)
	}
	if receipt.FileID == "" {
		// 三种"没有 fileId"都落到这里：上游说同名已存在、秒传没回 id、形状变了。
		// 念"已存在"而不念"传好了"：这一档我们**没有**可转的那一枚。
		problem := "PDF 那份素材没有 fileId（结局：" + receipt.Outcome.String() +
			"）⇒ ⑥ 不发，也不去删一个不是我们造的坑"
		e.run.progress(core.LevelWarn, fmt.Sprintf("PDF 转换跳过 %s %d：%s", name, id, problem))
		return true, append(problems, problem)
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf(
		"PDF 素材已上传（%s） %s %d", uploadBranchWords(receipt.Outcome), name, id))
	cguid, rnd := mailDiskRandoms(e.run.Random)
	convert, cerr := e.src.up.AppMailConvertPDF(
		ctx, session, cguid, rnd, receipt.Name, receipt.FileID, int64(len(content)),
		upstream.MailPDFConvertType,
	)
	if cerr != nil {
		problem := "PDF 转换（⑥）失败：" + cerr.Error() +
			" ⇒ 刚传上去那一份仍会在链尾被移进回收站；上游没认的转换不产文件" +
			"（2026-09-26 一号被拒那一发之后，只读走查里没多出这一趟的那一枚）"
		e.run.progress(core.LevelError, fmt.Sprintf("PDF 转换失败 %s %d：%s", name, id, cerr.Error()))
		return true, append(problems, problem)
	}
	e.run.progress(core.LevelInfo, fmt.Sprintf("PDF 转换已提交 %s %d", name, id))
	// ⑥ 认了 ⇒ 上游在**它自己交代的那个目录**里另存一份产物（`var.directoryId` +
	// `<素材干名>.docx`，两发 S_OK 实拍同型）。它不在任何一份现成账上，所以这里把它找回来
	// 挂进同一条清理链；找不回时**念出来**，不假装"跑完什么都没留下"。
	products, why := e.collectMailPDFProduct(ctx, convert.DirectoryID, receipt.Name)
	e.cloudFileIDs = append(e.cloudFileIDs, products...)
	// 产物那一半**不计进 Uploads**（它不是我们传的），但要单独记一格：小结那句减法
	// 靠它把"上游另存的"与"占位没传完的"分开（2026-09-26 真跑就是把它念成了后者）。
	e.cloudProductFiles += len(products)
	if len(products) > 0 {
		e.run.progress(core.LevelInfo, fmt.Sprintf(
			"PDF 转换产物已对上（链尾一并移进回收站） %s %d", name, id))
		return true, problems
	}
	// ⚠ 找不回产物**不**并进 problems（与 `ai_avatar_run.go` 的 [collectTempFiles] 同一口径）：
	// 转换本身已经做成了，把这一条判红等于告诉用户"这一条没成"，而他据此重跑会**再转一次**
	// —— 多一枚残留，还多耗一次权益（权益是有数的：一号两发 S_OK 之后就回
	// `用户权益已经使用完`，成文 `docs/capture/app-cloud-residue-samples.txt` §7）。
	// 代价是这条只活在事件流那一行 warn 里 ⇒ 界面上这一条显示成功，网盘里多一枚没人管的文件。
	e.run.progress(core.LevelWarn, fmt.Sprintf(
		"PDF 转换产物没对上：%s ⇒ 它仍会留在网盘里 %s %d", why, name, id))
	return true, problems
}

// 产物那一枚的等待窗口。两个数都来自实拍，不是拍的：
// 一号两发 S_OK 都在**第二次轮询**（即等过一档之后）读到产物，而第一次轮询（紧挨着 ⑥）
// 还没有 ⇒ "转换要一点时间才落盘"这一档是真的；窗口取 4 档 × 10 秒 ≈ 半分钟，
// 是"比已见的最慢一档宽出数倍、又不至于把一条玩法拖成一分钟"的折中。
// ⚠ 它**不是**判据：等不到时上面那句"没对上"照样念，不改成"再多等一会"。
const (
	mailPDFProductPolls     = 4
	mailPDFProductPollDelay = 10 * time.Second
	// mailPDFProductPageSize 与生产同档：一个目录只看一页，超页时下面那段"没对上"的原因里
	// 已经写明"可能被一页 100 个截掉"。
	mailPDFProductPageSize = 100
)

// mailPDFProductNames 由素材名推出**候选产物名**。
//
// 实拍两发都是"干名一模一样、扩展名换成 `.docx`"。`.pdf` 那一档留着是**排除项**：
// 万一上游哪天改成"同名同扩展另存"，这一条也认得出来，而它不引入新的误删面
// —— 名字干名是我们自己造的那一串毫秒数（`asign-pdf-<ms>`），撞不上用户自己的文件。
func mailPDFProductNames(sourceName string) []string {
	stem := strings.TrimSuffix(sourceName, ".pdf")
	return []string{stem + ".docx", stem + ".pdf"}
}

// collectMailPDFProduct 在 ⑥ 交代的那个目录里把这一趟的产物找回来，交回 fileId 与一句人话。
//
// 形状照 `ai_avatar_run.go` 的 [collectTempFiles]：**回不了错误** —— 找不到产物只是
// "网盘里多一枚没人管"，而转换本身已经做成了，把整条玩法判红会让人以为"这一条没成"，
// 而他据此重跑会**再转一次**（多一枚残留，还多耗一次权益）。所以这里只交回 id 与原因。
//
// 三处刻意的"不猜"：
//   - `directoryID` 空 ⇒ **不**去按目录名导航（那三枚名是上游的系统目录，改名就失效，
//     而且本仓没有任何"按路径找 fileId"的生产口），只念"⑥ 没交代目录"；
//   - 只按**精确名字**对上（候选来自素材名），不按前缀扫一片 ⇒ 与"不许按名字猜着删"
//     那条纪律同源（`docs/pending-decisions.md` R36）；
//   - 列目录失败 / 一枪都没撞中 ⇒ 念"没对上"并列**分不开的那几档**，不念"没有产物"。
func (e *taskExecutor) collectMailPDFProduct(
	ctx context.Context, directoryID, sourceName string,
) ([]string, string) {
	if directoryID == "" {
		return nil, "⑥ 没交代产物在哪个目录（`var.directoryId` 是空串）⇒ 没去找"
	}
	if e.run.Clock == nil {
		return nil, "时钟没接：列目录那一枪的时间戳头造不出来 ⇒ 没去找产物"
	}
	wait := e.run.Wait
	if wait == nil {
		wait = RealWaiter{}
	}
	names := mailPDFProductNames(sourceName)
	for attempt := 0; attempt < mailPDFProductPolls; attempt++ {
		if attempt > 0 {
			if werr := wait.Wait(ctx, mailPDFProductPollDelay); werr != nil {
				return nil, "等产物落盘那一等被打断：" + werr.Error()
			}
		}
		page, lerr := e.src.up.AppCloudFileList(ctx, e.opening.Profile,
			UserTimestamp(e.run.Clock), e.opening.Credential.Authorization(),
			directoryID, mailPDFProductPageSize)
		if lerr != nil {
			return nil, "列产物那个目录失败：" + lerr.Error()
		}
		for _, file := range page.Items {
			for _, candidate := range names {
				if file.Name == candidate && file.FileID != "" {
					return []string{file.FileID}, ""
				}
			}
		}
	}
	return nil, fmt.Sprintf(
		"在那个目录里轮询到第 %d 次都没读到这一趟那一枚（落得比等待窗口晚、名字变了、"+
			"或被一页 %d 个截掉 —— 这三档分不开）",
		mailPDFProductPolls, mailPDFProductPageSize)
}
