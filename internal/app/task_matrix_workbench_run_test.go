package app

// ─────────────────────────────────────────────────────────────────────────────
// 玩法 1028「AI 工作台」的**编排层**
// （邮箱会话 → ①′ 派生凭据 → ②′ 查前置 → ③′ 问一句 → 等 5 秒 → ④′ 再看一眼）
// ─────────────────────────────────────────────────────────────────────────────
//
// 请求面、DES/RSA 两处派生与 SSE 读法在 `internal/upstream/app_ai_workbench.go`，
// 那里各有按值钉的用例。这一层只判**编排**：
//   - **每枪恰好几次**：①′ 一次、②′④′ 同一个口**两次**、③′ 一次；
//   - **②′ 的三档念法**（本文件最贵的一格）：门成立才跳过对话，
//     "查询失败"与"上游回了但没给出 result"都**仍要问** ——
//     把它们混成"跳过"，这一条今天一个字都没发出去而播报说它做完了；
//   - **③′ 的凭据与 `userId` 都只来自 ①′**（库里那串 Basic 与库里那份 userDomainId
//     都**不是**这一族的取值）—— 这一格只有本层钉得住：协议层两处各自只知道
//     "传进来的那枚要原样用"。而它**看着像**顺手复用（585 那一族用的正是库里那份），
//     所以夹具把两枚值刻意做成**不同**，接错就红在按值比对上；
//   - **③′ 判失败 ⇒ ④′ 不发**（与邮箱那一族"③ 挂了仍发 ④"**相反**）；
//   - **④′ 失败不进账**（它是纯观察，记进 Err 会让小结念出"这一条失败"）；
//   - **派生出来的秘密一律不进日志**：DES 密文、解出的手机号、① 给的 token、
//     拼出来的 Basic 整串、邮箱会话那两枚。
//
// ⚠ 夹具**全部是合成**的，但它的**形状**在 2026-09-26 已经过一次真跑对齐
// （探针三趟，逐枪读数见 `docs/capture/app-ai-workbench-samples.txt`）：
// ①′ 那三格（`account` 24 / `token` 201 / `userDomainId` 19）真实存在，③′ 真实回了几十行
// SSE 且我们累得出内容。仍然**没有**的是"上游在某一日会把 1028 摆在表里"——
// 那一格的真实读数是 `code:"501" / msg:"任务不存在"`，本文件下面把它**按真实形状**钉住了。
// DES 那一枚密文**不是**自产自销：它由旧实现用的那个库（crypto-js）算出来，
// 四个向量与逐字对拍见 `app_ai_workbench_test.go`。
//
// ⚠ 三格夹具上的坑，写在这里是因为它们决定"红起来像什么"：
//   - ①′ 与邮箱资产侧那两口（⑤⑥）**共用** `/mw2/file/disk` 那条路径 ⇒ 计数按
//     `func=disk:queryUserToken` 分，不按 URL（同一口径成文在 [mxMailDiskEndpoint]）；
//   - ③′ 与 585 的 ②**同 URL**（`chat/v2/add`）⇒ 同一趟里只有这一族会打它，
//     所以按路径计数仍然等于"③′ 那一发"。**将来**若把两条玩法放进同一份夹具，
//     必须改成按 body 里的 `sourceChannel` 分（与 [mxSpecTokenEndpoint] 同一条坑）；
//   - [liveTaskScript] 取**第一条**匹配的路由，而队列**按弹出顺序**给（用尽后才重复末条）
//     ⇒ 演"②′ 与 ④′ 两种不同读数"靠的是**给两条**响应，而不是换路由。

import (
	"fmt"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	// mxWorkbenchTokenFunc 是 ①′ 在共用路径上的身份（见本文件抬头第三条）。
	mxWorkbenchTokenFunc = "disk:queryUserToken"
	// mxWorkbenchStatusEndpoint 是 ②′④′。它与通用打卡那条（[mxClickEndpoint]）
	// 同主机、同前缀，但互不为子串 ⇒ 两处计数不会互相污染。
	mxWorkbenchStatusEndpoint = "/ycloud/signin/public/getUserTaskAndCloudStatusExternal"

	// mxWorkbenchPhoneCipher 是 DES("19900001111", key="139_mail") 的 base64。
	// 出处不是本仓自己加密：它是旧实现那个 crypto-js 算出来的（同一枚值也在上游层用例里，
	// 两处对得上才算"两边解的是同一份东西"）。
	mxWorkbenchPhoneCipher = "KlRGp7lbaaIYgR3idlxaQw=="
	// mxWorkbenchPhone 是它解出来的那个**合成**号码（199 段，不是真号）。
	mxWorkbenchPhone = "19900001111"
	// mxWorkbenchToken 是 ①′ 回的那枚裸 authToken（合成值）。
	mxWorkbenchToken = "SYNTH-AI-TOKEN"
	// mxWorkbenchUpstreamDomainID 是 ①′ 回的那枚 `userDomainId`。
	//
	// ⚠ 它与夹具库里那一份（[mxCameraUserDomainID]）**刻意不同**：这一族要的是
	// ①′ 那一枚，而 585 那一族要的是库里那一份。做成同一个值，接错了也看不出。
	mxWorkbenchUpstreamDomainID = "1990000000000000009"
	// mxWorkbenchBasic 是 ③′ 该带的那一枚 Authorization：`Basic base64("mobile:<手机号>:<token>")`
	// （字面量，不是测试里现算 —— 现算等于用被测的那同一个函数验它自己）。
	mxWorkbenchBasic = "Basic bW9iaWxlOjE5OTAwMDAxMTExOlNZTlRILUFJLVRPS0VO"
)

// mxWorkbenchUserTokenBody 造 ①′ 的成功响应。`domainID` 传空串 ⇒ **不给**那一格，
// 演"①′ 说成功却没给 userDomainId ⇒ ③④ 都不发"那一档就靠它。
func mxWorkbenchUserTokenBody(domainID string) string {
	// ⚠ 这里用 `fmt.Sprintf` 而不是字符串拼接：identifier 门禁按**文件字面文本**认
	// `"token":"<20 字符以上>"` 这个形状，拼接式写法（`"token":"` + 常量 + `"`）会让
	// 中间那截变量名本身被当成"值"而报成疑似凭据明文（本轮真踩到）。
	parts := fmt.Sprintf(`"account":"%s","token":"%s"`, mxWorkbenchPhoneCipher, mxWorkbenchToken)
	if domainID != "" {
		parts += `,"userDomainId":"` + domainID + `"`
	}
	return `{"code":"S_OK","var":{"data":{"data":{` + parts + `}}}}`
}

// mxWorkbenchStatusBody 造 ②′④′ 的读数。两格传空串 ⇒ **不给**那一格（演"缺读数"）。
//
// ⚠ 这里的数字**不带引号**：门要的是 JSON 数字（旧实现那句是严格 `=== 1`），
// 夹具写成 `"1"` 会让红出现在解析层，而排查目标是编排层（对照用例见上游层那张十行表）。
func mxWorkbenchStatusBody(taskStatus, cloudStatus string) string {
	var result []string
	if taskStatus != "" {
		result = append(result, `"taskStatus":`+taskStatus)
	}
	if cloudStatus != "" {
		result = append(result, `"cloudStatus":`+cloudStatus)
	}
	if len(result) == 0 {
		return `{"code":0,"result":{}}`
	}
	return `{"code":0,"result":{` + strings.Join(result, ",") + `}}`
}

const (
	// mxWorkbenchChatAnswer 两条路径各给一片（`flowResult.outContent` 与直挂 `outContent`）
	// ⇒ 累加成一句话。两格**都要**演：旧实现两处各加一次，只写一处会漏一半内容。
	//
	// ⚠ `sessionId` 用一枚**长**合成值（15 字符）而不是短串：播报那一行按真跑口径只报**长度**，
	// 值不落文本 ⇒ 这条用例要能抓出"把会话标识打进日志"。短到 `S-1` 那种长度，
	// 掩码与断言都抓不住（真跑那三趟它回的是 19 位数字）。
	// mxWorkbenchSessionID 是一枚**长**合成会话标识（真跑那三趟回的是 19 位数字）。
	//
	// ⚠ 刻意不写成 `S-1` 那种短串：播报那一行按真跑口径只念**长度**、不念值
	// （它会进 `runs` 日志与推送正文），而短值既看不出"漏了长度这一格"、
	// 也抓不住"把标识打进日志"那种改坏。
	mxWorkbenchSessionID = "SYNTH-WB-SESSION-IDENTIFIER"
	// mxWorkbenchSessionIDJSON 是它在夹具里带引号的那一段。
	mxWorkbenchSessionIDJSON = `"` + mxWorkbenchSessionID + `"`

	mxWorkbenchChatAnswer = "data: {\"code\":\"0000\",\"message\":\"请求成功\"," +
		"\"data\":{\"sessionId\":" + mxWorkbenchSessionIDJSON + ",\"flowResult\":{\"outContent\":\"你\"}}}\n" +
		"data: {\"data\":{\"outContent\":\"好呀\"}}\n"
	// mxWorkbenchChatNoEvents 演第一格失败：一个事件都没有（`[DONE]` 与非 JSON 都不计）。
	mxWorkbenchChatNoEvents = "data: [DONE]\n这一段不是 JSON\n"
	// mxWorkbenchChatEmptyContent 演**第二格**失败：有事件、但内容累出来是空串。
	// 少了这一档的用例，"只判 Events"那种写法永远不会红。
	mxWorkbenchChatEmptyContent = "data: {\"data\":{\"sessionId\":\"S-2\"}}\n"
)

// workbenchFixture 是一趟的三格读数（空串 / nil ⇒ **不给**那一枪编路由，
// 落到夹具的 404 分支 —— 这就是"被拒"那一档，见本文件抬头第三条）。
type workbenchFixture struct {
	userToken string
	statuses  []string
	chat      string
}

// workbenchScript 把"一条 1028 + 这一族的响应"编成一份脚本。
func workbenchScript(t *testing.T, f workbenchFixture) *liveTaskScript {
	t.Helper()
	waiting := mxItems(mxItem(1028, "day", "AI 工作台", "WAIT", "", 0))
	routes := []liveTaskRoute{{mxMailInLoginEndpoint, []string{mailSessionBody()}}}
	if f.userToken != "" {
		routes = append(routes, liveTaskRoute{mxMailDiskEndpoint, []string{f.userToken}})
	}
	if len(f.statuses) > 0 {
		routes = append(routes, liveTaskRoute{mxWorkbenchStatusEndpoint, f.statuses})
	}
	if f.chat != "" {
		routes = append(routes, liveTaskRoute{mxCameraChatEndpoint, []string{f.chat}})
	}
	routes = append(routes, mxHappy()...)
	return mxScript([]string{mxCameraTicket}, waiting, nil, routes...)
}

// workbenchGood 是"整条链跑通"那一档：①′ 给全、②′ 说还没到（所以问一句）、④′ 给第二份读数、
// ③′ 回两行分片。
func workbenchGood() workbenchFixture {
	return workbenchFixture{
		userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
		statuses: []string{
			mxWorkbenchStatusBody("0", "0"), // ②′：门不成立 ⇒ 要问
			mxWorkbenchStatusBody("1", "0"), // ④′：链尾那一眼
		},
		chat: mxWorkbenchChatAnswer,
	}
}

// workbenchRun 装配一条链（夹具store 带**库里那份** userDomainId，与 ①′ 那枚刻意不同）。
func workbenchRun(
	t *testing.T, script *liveTaskScript, progress RunProgress,
) (TaskMatrixRun, *recordingWaiter) {
	t.Helper()
	waiter := &recordingWaiter{}
	run := mxRun(t, mxCameraStore(true), script, NewSessionPool(), testClock(), waiter)
	run.Progress = progress
	return run, waiter
}

// workbenchWaitedSeconds 把 waiter 记到的时长摊成便于比对的一串。
func workbenchWaitedSeconds(waiter *recordingWaiter) string {
	parts := make([]string, 0, len(waiter.durations))
	for _, d := range waiter.durations {
		parts = append(parts, d.String())
	}
	return strings.Join(parts, " / ")
}

// workbenchFuncCount 只数 ①′ 那一发：按 `func`，不按 URL（理由见本文件抬头第三条）。
func workbenchFuncCount(script *liveTaskScript) int {
	n := 0
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxWorkbenchTokenFunc) {
			n++
		}
	}
	return n
}

// ─────────────────────────────────────────────────────────────────────────────
// 分派：1028 该被认成玩法，而不是缺口
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanTaskStepSelectsWorkbenchTask ⭐ 1028 走 runner，且形状只有 Workbench 那一格。
//
// 反向改坏它：把 1028 挪回 [taskMatrixToolGaps] ⇒ 这一条红；给 spec 顺手挂上
// `FinalKey` ⇒ 也红（那会多打一发通用 `task/click`，而旧实现那条 runner 后面
// 不再打本族那一发）。
func TestPlanTaskStepSelectsWorkbenchTask(t *testing.T) {
	item := dispatchItem(1028, "day", "AI 工作台", "WAIT", 0)
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepRunner {
		t.Errorf("1028 该走专属玩法，实得 %v（%s）", plan.Kind, plan.Reason)
	}
	if plan.Runner == "" {
		t.Errorf("结论要能说出是哪一条玩法做的，实得空")
	}
	if plan.ClickKey != taskClickKeyDefault {
		t.Errorf("1028 的前置那一发仍按通用门走（key=task），实得 %q", plan.ClickKey)
	}
	spec, ok := taskMatrixRunners[1028]
	if !ok {
		t.Fatal("1028 不在 runner 表里")
	}
	if !spec.Workbench || spec.Upload != nil || spec.Shares || spec.Notes || spec.Mail ||
		spec.MailBackup || spec.MailPDF || spec.AICamera ||
		spec.FinalKey != "" || spec.OpensEntry {
		t.Errorf("1028 的形状只该有 Workbench 那一格，实得 %+v", spec)
	}
	if _, gap := taskMatrixToolGaps[1028]; gap {
		t.Errorf("1028 接上后没从缺口表删掉 ⇒ 界面上会永远显示『没接玩法』")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 正常那一趟
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunWorkbenchAsksOnce ⭐ 整条链跑通那一档（六格判据）。
//
//   - **计数**：①′ 一次、②′④′ 同一个口**两次**、③′ 一次。④′ 那次最容易丢
//     （"跳过对话"与"问完就结束"两种写法都能让 ③′ 那一格看起来没问题）。
//   - **时序**：换会话 → ①′ → ②′ → ③′ → ④′（①′ 之前必须已有会话，②′ 之前必须已有手机号）。
//   - **③′ 的 Authorization 是 ①′ 派生出来的那一枚 Basic**，不是库里那串（[wholeAuth]）。
//     库里那一份在本族压根不该出网 —— 这一格按值比才看得出来，接错时请求形状完全正常。
//   - **`userId` 用 ①′ 回的那一枚**（夹具里它与库里那份**不同**）；同理 body 里
//     不该出现库里那枚值。
//   - **`inputTime` 由注入时钟给出**，格式是本族那一套（`2026-09-20 12:00:00`，
//     无 `T` 无 `Z`）⇒ 与 585 那一族不同形（对照见 [mxCameraChatEndpoint] 那一条用例）。
//   - **问出去的那一句是出处那个兜底常量**（本仓没有 `aiChatMessage` 那一格配置，
//     成文与"要不要做成配置项"见 [workbenchDialogue]）。
//   - **它不写网盘** ⇒ 清理链一次都不该发（与 585 / 1014 相反的那一格）。
func TestTaskMatrixRunWorkbenchAsksOnce(t *testing.T) {
	script := workbenchScript(t, workbenchGood())
	log := &mxLog{}
	run, waiter := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := workbenchFuncCount(script); n != 1 {
		t.Errorf("①′ 该恰好一发（按 func 数），实得 %d", n)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("邮箱会话该只换一次，实得 %d", n)
	}
	if n := script.count(mxWorkbenchStatusEndpoint); n != 2 {
		t.Errorf("②′ 与 ④′ 是同一个口的两遍，应共两发，实得 %d", n)
	}
	if n := script.count(mxCameraChatEndpoint); n != 1 {
		t.Errorf("③′ 该恰好一发，实得 %d", n)
	}
	session, token, status, chat := requestIndex(script, mxMailInLoginEndpoint),
		requestIndex(script, mxWorkbenchTokenFunc),
		requestIndex(script, mxWorkbenchStatusEndpoint),
		requestIndex(script, mxCameraChatEndpoint)
	secondStatus := -1
	if all := requestsTo(script, mxWorkbenchStatusEndpoint); len(all) == 2 {
		// 第二遍的**下标**要从头算：requestsTo 只给请求，不给它在总序列里的位置。
		seen := 0
		for i, request := range script.snapshot() {
			if strings.Contains(request.URL, mxWorkbenchStatusEndpoint) {
				seen++
				if seen == 2 {
					secondStatus = i
				}
			}
		}
	}
	if !(session >= 0 && token > session && status > token && chat > status && secondStatus > chat) {
		t.Errorf("时序应为 换会话>派生>前置>对话>复查，实得 %d/%d/%d/%d/%d（全部：%v）",
			session, token, status, chat, secondStatus, mxURLs(script))
	}
	request, has := mailRequest(script, mxCameraChatEndpoint)
	if !has {
		t.Fatal("没看到 ③′ 那一枪")
	}
	if got := headerValue(request, "authorization"); got != mxWorkbenchBasic {
		t.Errorf("③′ 的凭据该是 ①′ 派生那枚 Basic（库里那串 Basic 在本族不该出网），实得 %q", got)
	}
	if got := headerValue(request, "authorization"); got == wholeAuth {
		t.Errorf("③′ 竟带了库里那串 —— 这一族的凭据只能从 ①′ 现换")
	}
	body := string(request.Body)
	if !strings.Contains(body, `"userId":"`+mxWorkbenchUpstreamDomainID+`"`) {
		t.Errorf("userId 该用 ①′ 回的那一枚，实得 %s", playAICameraLogText(body))
	}
	if strings.Contains(body, mxCameraUserDomainID) {
		t.Errorf("body 里出现了库里那枚 userDomainId ⇒ 接成了 585 那一族的取值：%s",
			playAICameraLogText(body))
	}
	if !strings.Contains(body, `"inputTime":"2026-09-20 12:00:00"`) {
		t.Errorf("inputTime 该由注入时钟给出本族那一套格式，实得 %s", playAICameraLogText(body))
	}
	if !strings.Contains(body, `"dialogue":"`+workbenchDialogue+`"`) {
		t.Errorf("问的那一句是出处那个兜底常量，实得 %s", playAICameraLogText(body))
	}
	if !strings.Contains(body, `"sourceChannel":"`+"10175"+`"`) {
		t.Errorf("③′ 的 sourceChannel 要带（它决定上游记哪一家的账），实得 %s",
			playAICameraLogText(body))
	}
	// ②′④′ 两遍都**不带鉴权头**（身份全在 body 那枚 RSA 密文里）—— 协议层头注成文，
	// 这里钉的是"编排层没有顺手把 ①′ 的 Basic 复用过去"。
	for i, statusRequest := range requestsTo(script, mxWorkbenchStatusEndpoint) {
		if got := headerValue(statusRequest, "authorization"); got != "" {
			t.Errorf("第 %d 遍前置查询不该带 authorization，实得非空", i+1)
		}
		if b := string(statusRequest.Body); strings.Contains(b, mxWorkbenchPhone) {
			t.Errorf("第 %d 遍把手机号**明文**发出去了（出处只发 RSA 密文）", i+1)
		}
	}
	// 等那 5 秒（链尾复查的前置）+ 通用那一拍 500ms。
	if !strings.Contains(workbenchWaitedSeconds(waiter), "5s") {
		t.Errorf("③′ 之后要等那 5 秒再复查，实得：%s", workbenchWaitedSeconds(waiter))
	}
	// 不写网盘 ⇒ 清理链一次都不发。
	if n := mxTrashCount(script); n != 0 || len(res.CloudFileIDs) != 0 {
		t.Errorf("这一条不该往网盘写任何东西，实得清理 %d 次 / 账本 %v", n, res.CloudFileIDs)
	}
	// 播报：前置一行（未满足 ⇒ 要问）+ 对话一行 + 复查一行。
	if !log.hasAt(core.LevelInfo, "AI 工作台前置未满足") {
		t.Errorf("②′ 说没到 ⇒ 要念「前置未满足」，实得：\n%s", log.joined())
	}
	line := log.lineWith("AI 工作台对话已回")
	if line == "" || !strings.Contains(line, `模型回话="你好呀"`) {
		t.Errorf("两格 outContent 路径要**都**累加后念出来，实得：\n%s", log.joined())
	}
	if !strings.Contains(line, "data 行数=2") {
		t.Errorf("要念出事件数（它和空内容一起才是判据），实得 %q", line)
	}
	recheck := log.lineWith("AI 工作台链尾复查")
	if recheck == "" || !strings.Contains(recheck, "门=过") {
		t.Errorf("链尾那一眼也要念（④′ 的读数才是「这一趟之后上游怎么说」），实得 %q", recheck)
	}
	// ⚠ 会话标识**不落文本**（真跑三趟它回 19 位数字，而这一行会进 `runs` 日志与推送正文）：
	// 只许念长度。成文在 [workbenchReadLine] 头注第四条。
	if strings.Contains(line, mxWorkbenchSessionID) ||
		!strings.Contains(line, "sessionId 长度="+fmt.Sprint(len(mxWorkbenchSessionID))) {
		t.Errorf("③′ 那一行要念 sessionId 的**长度**而不是值，实得 %q", line)
	}
	if strings.Contains(log.joined(), "AI 工作台失败") {
		t.Errorf("全成功的一趟不许念失败：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
	if res.Steps[0].Err != "" {
		t.Errorf("全成功时结论里不该有原因，实得 %q", res.Steps[0].Err)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ②′ 的三档：只有"门成立"那一档才跳过对话
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunWorkbenchSkipsChatWhenGatePasses ⭐ 上游**说了**不用问 ⇒ ③′ 零发。
//
// 这一档是四档里唯一"③′ 一次都不该发"的。反向改坏它：把门判据从"两枚都是数字"
// 放宽成"字符串也算" ⇒ 本条红（那会少问一句，而"少问"的表现是这一条今天没进度）。
//
// ⚠ 与下面那条 [TestTaskMatrixRunWorkbenchStatusDoesNotGateChat] 一起读才是完整的：
// 那一档钉的是"上游没说 ⇒ 必须问"。两档合起来才挡住"把查询失败念成前置已满足"。
func TestTaskMatrixRunWorkbenchSkipsChatWhenGatePasses(t *testing.T) {
	fixture := workbenchFixture{
		userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
		// 队列用尽后重复末条 ⇒ ②′ 与 ④′ 拿到同一份"门成立"。
		statuses: []string{mxWorkbenchStatusBody("1", "0")},
		chat:     mxWorkbenchChatAnswer,
	}
	script := workbenchScript(t, fixture)
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxCameraChatEndpoint); n != 0 {
		t.Errorf("门成立时不许问那一句，实得 %d 次", n)
	}
	// ⚠ 链尾那一眼**照发**：旧实现那句 ④′ 在 `if (needChat)` 外面（`runners.ts:414`）。
	if n := script.count(mxWorkbenchStatusEndpoint); n != 2 {
		t.Errorf("跳过对话也要走链尾复查，实得 %d 次", n)
	}
	if !log.hasAt(core.LevelInfo, "AI 工作台前置已满足") {
		t.Errorf("实得：\n%s", log.joined())
	}
	line := log.lineWith("AI 工作台前置已满足")
	if !strings.Contains(line, "门=过") || !strings.Contains(line, "taskStatus=1") {
		t.Errorf("跳过那一行要带上游的两格读数（否则与「查询失败」同形），实得 %q", line)
	}
	if res.Counts.Failed != 0 || res.Counts.Ran != 1 {
		t.Errorf("跳过对话不算失败（上游说这一条不用做），实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunWorkbenchStatusDoesNotGateChat ⭐ 另外三档**都要问**那一句。
//
// 三档的区分是这一族最贵的一格：它们的上游读数完全不同，而对这一条的处置相同
// （照旧实现那句 `needChat` 初值 true）。混成一档的表现：②′ 被风控挡下时，
// 播报念"前置已满足、跳过对话"，而这一条今天一个字都没发出去。
func TestTaskMatrixRunWorkbenchStatusDoesNotGateChat(t *testing.T) {
	for _, tier := range []struct {
		name     string
		fixture  workbenchFixture
		wantIn   string // 那一行里要有的字样
		wantMark string // Err 里要有的字样
	}{
		{
			// 无路由 ⇒ 404 ⇒ 查询失败。
			name:     "查询本身失败",
			fixture:  workbenchFixture{userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID), chat: mxWorkbenchChatAnswer},
			wantIn:   "AI 工作台前置状态（②′）没查到",
			wantMark: "上游没说",
		},
		{
			name: "回了但没给 result",
			fixture: workbenchFixture{
				userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
				statuses:  []string{`{"code":0}`},
				chat:      mxWorkbenchChatAnswer,
			},
			wantIn: "AI 工作台前置状态读不出门", wantMark: "仍发对话",
		},
		{
			name: "result 有了但门不成立",
			fixture: workbenchFixture{
				userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
				statuses:  []string{mxWorkbenchStatusBody("2", "1")},
				chat:      mxWorkbenchChatAnswer,
			},
			wantIn: "AI 工作台前置未满足", wantMark: "",
		},
		{
			// ⭐ **真实读数那一档**（2026-09-26 探针：同一口换 `taskId=585/106/1020` 都回这一型）。
			// 三格全是**带引号的字符串** ⇒ `EnvelopeOK` 不成立（旧实现那句 `cs?.code === 0`
			// 在同一份体上**同样**不成立）⇒ 走"读不出门"那一档、**仍发对话**。
			// 这一条存在是为了钉住"不放宽"这个决定：哪天有人为了让门能成立而去做隐式转换，
			// 本条立刻红 ⇒ 逼他先回答"少发一次 ③′（写）这个决定该由谁做"。
			// 逐字读数见 `docs/capture/app-ai-workbench-samples.txt` §2。
			name: "上游真实那一型（三格都是字符串）",
			fixture: workbenchFixture{
				userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
				statuses: []string{`{"code":"0","msg":"success","result":` +
					`{"cloudNum":"6","cloudStatus":"0","taskStatus":"1"}}`},
				chat: mxWorkbenchChatAnswer,
			},
			wantIn: "AI 工作台前置状态读不出门", wantMark: "仍发对话",
		},
		{
			// ⭐ 1028 **自己**今天的读数：`501 / 任务不存在`（三趟一致，也与造出来的
			// `taskId=999999` 逐字同形）⇒ 同一档处置。少了这一条，"本族今天实际只会走
			// 哪一档"在测试里是**没有影子**的。
			name: "1028 今天真回的那一句（501 任务不存在）",
			fixture: workbenchFixture{
				userToken: mxWorkbenchUserTokenBody(mxWorkbenchUpstreamDomainID),
				statuses:  []string{`{"code":"501","msg":"任务不存在"}`},
				chat:      mxWorkbenchChatAnswer,
			},
			wantIn: "AI 工作台前置状态读不出门", wantMark: "仍发对话",
		},
	} {
		t.Run(tier.name, func(t *testing.T) {
			script := workbenchScript(t, tier.fixture)
			log := &mxLog{}
			run, _ := workbenchRun(t, script, log.handle())
			res, perr := mxResult(t, run)
			if perr != nil {
				t.Fatalf("跑一趟失败: %v", perr)
			}
			if n := script.count(mxCameraChatEndpoint); n != 1 {
				t.Errorf("这一档仍要问那一句，实得 %d 次", n)
			}
			// ⭐ 三档都要问，而**记账不同**：只有"查询本身失败"那一档进结论
			//（它是本工具侧要怀疑的一格），另两档是上游的读数、按成功走。
			if strings.Contains(tier.wantIn, "没查到") != strings.Contains(res.Steps[0].Err, "没查到") {
				t.Errorf("「上游没说」那一档要留在结论里，其余两档不必；实得 %q", res.Steps[0].Err)
			}
			if !strings.Contains(log.joined(), tier.wantIn) {
				t.Errorf("要念出「%s」这一档，实得：\n%s", tier.wantIn, log.joined())
			}
			if tier.wantMark != "" && !strings.Contains(log.joined(), tier.wantMark) {
				t.Errorf("念出来要说清后果（%q），实得：\n%s", tier.wantMark, log.joined())
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 一枪都不发 / 只发半串那几种形态
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunWorkbenchTokenFailureSendsNothingAfter ⭐ ①′ 挂 ⇒ 后面三枪都不发。
//
// 反向改坏它：把"拿库里的 Basic 顶一下"写进来 ⇒ ③′ 计数变 1（那是**打到别处**的一发，
// 而且它带着本族没有的那一格 sourceChannel 语义）。
func TestTaskMatrixRunWorkbenchTokenFailureSendsNothingAfter(t *testing.T) {
	script := workbenchScript(t, workbenchFixture{chat: mxWorkbenchChatAnswer})
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	for _, forbidden := range []string{mxWorkbenchStatusEndpoint, mxCameraChatEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("①′ 没成就该一枪不发：%s 实得 %d 次", forbidden, n)
		}
	}
	if !strings.Contains(res.Steps[0].Err, "派生凭据（①′）失败") {
		t.Errorf("要说清挂在哪一枪，实得 %q", res.Steps[0].Err)
	}
	if !log.hasAt(core.LevelError, "AI 工作台失败") {
		t.Errorf("实得：\n%s", log.joined())
	}
	if res.Counts.Ran != 0 || res.Counts.Failed != 1 {
		t.Errorf("没做成要落失败那一档，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunWorkbenchWithoutUserDomainIDSkipsChatAndRecheck ⭐ ①′ 没回 userDomainId。
//
// 这一格挡的是"拿库里那份顶一下"：本仓**有**另一枚 userDomainId（585 那一族用的就是它），
// 而这一族的出处只认 ①′ 那一枚 —— 两者同不同源**未测试**，所以宁可不问
// （成文在 `task_matrix_workbench.go` 函数头第二条）。
//
// ⚠ ②′ 仍会发一次（门在 userDomainId 之前），所以断言单位是"③′④′ 各 0 次、②′ 只 1 次"。
func TestTaskMatrixRunWorkbenchWithoutUserDomainIDSkipsChatAndRecheck(t *testing.T) {
	script := workbenchScript(t, workbenchFixture{
		userToken: mxWorkbenchUserTokenBody(""),
		statuses:  []string{mxWorkbenchStatusBody("0", "0")},
		chat:      mxWorkbenchChatAnswer,
	})
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	if n := script.count(mxCameraChatEndpoint); n != 0 {
		t.Errorf("没有 userDomainId 就不该问那一句，实得 %d", n)
	}
	if n := script.count(mxWorkbenchStatusEndpoint); n != 1 {
		t.Errorf("旧实现这一档直接 return ⇒ 链尾复查也不发，实得 %d 次", n)
	}
	if !strings.Contains(res.Steps[0].Err, "userDomainId") {
		t.Errorf("要说清缺哪一格，实得 %q", res.Steps[0].Err)
	}
	if !log.hasAt(core.LevelError, "AI 工作台跳过") {
		t.Errorf("实得：\n%s", log.joined())
	}
}

// TestTaskMatrixRunWorkbenchChatFailureSkipsRecheck ⭐ ③′ 的三种失败都**不发** ④′。
//
// 这一条与邮箱那一族（"③ 挂了仍发 ④"）**刻意相反**：出处那三处都是 `return`。
// 把它"对齐"成邮箱那一族的表现：④′ 会多一发，而那一发**什么都没写**、只是白敲上游一下。
//
// 三档各挡一种写法：只判 `Events`（第二档会漏）、只判内容（第一档会漏）、
// ③′ 报错还继续（第三档会漏）。
func TestTaskMatrixRunWorkbenchChatFailureSkipsRecheck(t *testing.T) {
	for _, tier := range []struct {
		name    string
		chat    string
		wantIn  string
		wantErr string
	}{
		{"一个事件都没有", mxWorkbenchChatNoEvents, "AI 工作台对话（③′）上游没回任何内容", "data 行数=0"},
		{"有事件但内容为空", mxWorkbenchChatEmptyContent, "AI 工作台对话（③′）上游没回任何内容", "data 行数=1"},
		{"那一发被拒", "", "AI 工作台对话（③′）失败", ""},
	} {
		t.Run(tier.name, func(t *testing.T) {
			fixture := workbenchGood()
			fixture.chat = tier.chat
			script := workbenchScript(t, fixture)
			log := &mxLog{}
			run, _ := workbenchRun(t, script, log.handle())
			res, perr := mxResult(t, run)
			if perr != nil {
				t.Fatalf("这一档不该让整趟报错: %v", perr)
			}
			if n := script.count(mxWorkbenchStatusEndpoint); n != 1 {
				t.Errorf("③′ 失败时链尾复查不发，实得 %d 次", n)
			}
			if !log.hasAt(core.LevelError, "AI 工作台失败") {
				t.Errorf("实得：\n%s", log.joined())
			}
			if !strings.Contains(log.joined(), tier.wantIn) {
				t.Errorf("要说清「%s」，实得：\n%s", tier.wantIn, log.joined())
			}
			if tier.wantErr != "" && !strings.Contains(res.Steps[0].Err, tier.wantErr) {
				t.Errorf("结论里要带上同一份读数（%q），实得 %q", tier.wantErr, res.Steps[0].Err)
			}
			if res.Counts.Failed != 1 {
				t.Errorf("没做成要落失败那一档，实得 %+v", res.Counts)
			}
		})
	}
}

// TestTaskMatrixRunWorkbenchRecheckFailureIsNotChalkedUp ⭐ ④′ 挂了**不进账**。
//
// 这是本族与邮箱那一族（③ 失败仍记账）**相反**的一格，所以单独钉。
// 记进 Err 的表现：小结念"这一条失败"，而 ①′②′③′ 其实都成了 ——
// 用户看到的是"要重跑"，而重跑会**多问一句**（③′ 是写）。
//
// ⚠ 这一档靠**队列的第二条**演：②′ 与 ④′ 是同一个口，[liveTaskScript] 按弹出顺序给响应
// （用尽后才重复末条）⇒ 第一份给"门不成立"、第二份给"根本解不开"，才只让 ④′ 挂。
// （原先这里写成"两份都成功、只断言不记账"，那演的是**成功档**、不是失败档 —— 已按队列
// 的真实语义改掉；"④′ 成功要单独念一行"那一格在 [TestTaskMatrixRunWorkbenchAsksOnce]。）
func TestTaskMatrixRunWorkbenchRecheckFailureIsNotChalkedUp(t *testing.T) {
	fixture := workbenchGood()
	fixture.statuses = []string{
		mxWorkbenchStatusBody("0", "0"), // ②′：门不成立 ⇒ 问一句（这一趟要跑到 ④′）
		"这不是 JSON",                      // ④′：解不开 ⇒ 协议层交回 parse 错
	}
	script := workbenchScript(t, fixture)
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	// 前提：③′ 真的问成了、④′ 真的发了（否则"没记账"可能来自一条压根没走到链尾的趟）。
	if n := script.count(mxCameraChatEndpoint); n != 1 {
		t.Fatalf("这一条要在 ③′ 问成之后才有意义，实得 %d 次", n)
	}
	if n := script.count(mxWorkbenchStatusEndpoint); n != 2 {
		t.Fatalf("②′④′ 两发都要有，实得 %d 次", n)
	}
	if !log.hasAt(core.LevelWarn, "AI 工作台链尾复查") {
		t.Errorf("④′ 挂了要念一行 warn（它不进账，事件流是它唯一的出口），实得：\n%s", log.joined())
	}
	if !log.has("纯观察") {
		t.Errorf("这一行要说清「它为什么不进账」，实得：\n%s", log.joined())
	}
	if res.Steps[0].Err != "" {
		t.Errorf("①′②′③′ 都成了 ⇒ 结论里不该有原因，实得 %q", res.Steps[0].Err)
	}
	if res.Counts.Failed != 0 || res.Counts.Ran != 1 {
		t.Errorf("实得 %+v", res.Counts)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 凭据边界
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunWorkbenchKeepsDerivedSecretsOutOfLog ⭐ 派生链上任何一格都不进日志。
//
// 这一族比 585 多两格要防的：DES 密文与**解出来的手机号**（它在库里根本没有，
// 是这一趟现换的），以及由手机号拼出来的 Basic 整串。跑的是**全失败**那一趟 ——
// 失败文本会把上游原话与错误串带进来，那正是最容易顺手把请求侧的值拼进去的形状。
// 口径见 `docs/sanitize-guide.md`。
func TestTaskMatrixRunWorkbenchKeepsDerivedSecretsOutOfLog(t *testing.T) {
	fixture := workbenchGood()
	fixture.chat = "" // ③′ 无路由 ⇒ 404 ⇒ 走失败那一趟
	script := workbenchScript(t, fixture)
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	for _, secret := range []string{
		mxWorkbenchPhoneCipher, mxWorkbenchPhone, mxWorkbenchToken, mxWorkbenchBasic,
		mxMailSID, mxMailRMKey, mxCameraTicket, wholeAuth,
	} {
		if strings.Contains(log.joined(), secret) || strings.Contains(res.Steps[0].Err, secret) {
			t.Errorf("事件流或失败文本里带了派生链上的值：%q", playAICameraLogText(secret))
		}
	}
	// 反向对照：这一趟**确实**跑到了 ③′（否则上面那圈"没泄露"可能是空跑）。
	if n := script.count(mxCameraChatEndpoint); n != 1 {
		t.Fatalf("这一条要在③′ 真的发过一枪才有意义，实得 %d", n)
	}
}

// TestTaskMatrixRunWorkbenchChatBodyHasNoUpstreamTextLeak ⭐ 播报里念的是**内容**，不是整段原文。
//
// 与 585 那一族同一条理由：③′ 的 `message` 是信封文案（"请求成功"），不是模型那一句。
// 这一条只钉"两格分开念、回答那一格来自累加"，形状细节由上游层用例钉。
func TestTaskMatrixRunWorkbenchChatBodyHasNoUpstreamTextLeak(t *testing.T) {
	fixture := workbenchGood()
	fixture.chat = "data: {\"code\":\"0000\",\"message\":\"请求成功\"," +
		"\"data\":{\"flowResult\":{\"outContent\":\"只有一句\"}}}\n"
	script := workbenchScript(t, fixture)
	log := &mxLog{}
	run, _ := workbenchRun(t, script, log.handle())
	if _, perr := mxResult(t, run); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	line := log.lineWith("AI 工作台对话已回")
	if !strings.Contains(line, `模型回话="只有一句"`) || !strings.Contains(line, `message="请求成功"`) {
		t.Errorf("内容格与信封格要分开念，实得 %q", line)
	}
	if strings.Contains(line, "data: {") {
		t.Errorf("成功时不该把整行原文挤进这一行，实得 %q", line)
	}
}
