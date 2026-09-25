package app

// ─────────────────────────────────────────────────────────────────────────────
// 玩法 585「AI 相机」的**编排层**（① 识图 → ② 拿 fileId 问一句）
// ─────────────────────────────────────────────────────────────────────────────
//
// 请求面与 SSE 三档读法在 `internal/upstream/app_play_ai.go` /
// `app_play_ai_chat_sse.go`，那里各有按值钉的用例。这一层只判**编排**：
//   - 两枪各恰好一发、顺序对（② 的 fileId 只能是 ① 刚回的那一枚）；
//   - **两枪用同一枚凭据，而它不是票**（都是库里那串 `Basic …` **整串**）—— 这一格只有
//     本层钉得住：协议层两处各自只知道"传进来的那枚要原样进头"，把调用点接成票、
//     或接成 Basic 的第三段裸 authToken，它都看不出来。而这两格都是**真跑换来的**：
//     带票出网 ⇒ `code:"01000002"` / `鉴权失败`；带裸 authToken 出网 ⇒ **同样** `01000002`；
//     真机那两条实拍里 ①② 的 `Authorization` 逐字同一枚（纠正全程成文在
//     `task_matrix_play_ai.go` 文件头第一条）；
//   - 什么时候**一枪都不发**：库里没有 userDomainId（填错会打到别人账号上）、
//     库里没有 Authorization 整串、缺时钟 / 缺随机源；
//   - 写了网盘就记账，哪怕后面那一发挂了（口径同 1014）；
//   - 播报**必须说命中哪一档**（旧判据的第三档会把一段报错页念成成功）。
//
// ⚠ 夹具是**合成**的，但它不是凭空造的：② 有真机 SSE 实拍（`har/2026-09-25-playai-*.har`
// 里那一条：47 行 `data:`、末条 `finishReason:"stop"`），① 有成功定拍（`code=0000` +
// `fileId` / `taskId`），两处的**值都已脱敏** ⇒ 本文件每一条钉的仍是"我们怎么处置这一种读数"。
// ⚠ 四条 har 里是**同一次会话**的同一组读数 ⇒ 成文时只算一次观测（登记见
// `docs/capture/app-play-ai-samples.txt`）。
//
// ⚠ 两处夹具上的坑，写在这里是因为它们会决定"红起来像什么"：
//   - [mxStoreBound] **不带** `UserDomainID` ⇒ 直接用它，585 会停在"宁可不发"那一档，
//     红成"① 应发 1 次实得 0 次"，看着像编排漏了一枪。所以本文件另有 [mxCameraStore]。
//   - [liveTaskScript] 永远回 **HTTP 200** ⇒ 演"② 被拒"只能用**不给路由**那一档
//     （落到夹具的 404 分支），不是给一份失败响应体。

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	// mxCameraRecognizeEndpoint 是 ①，mxCameraChatEndpoint 是 ②。两条路径互不为子串
	// ⇒ 各数各的不会互相污染（与 [mxMailClickEndpoint] 同一考量）。
	mxCameraRecognizeEndpoint = "/api/image/aiRecognize"
	mxCameraChatEndpoint      = "/api/outer/assistant/chat/v2/add"
	// mxCameraTicket 是**签到页那张票**的夹具值。本族两枪**都不带它**（成文在
	// `task_matrix_play_ai.go` 文件头第一条），它只服务两件事：夹具的建会话那三枪，
	// 以及下面那条"请求侧的值不进日志"的用例需要一个"看着像凭据、且代码不该打印"的串。
	// 刻意与 [wholeAuth] 长得完全不一样：万一有人把票接进 ①，按值比对的用例要一眼看出。
	mxCameraTicket = "SYNTHCAMERAJWT.HEADER.PAYLOAD.SIG"
	// 凭据那一枚不另造常量：①② 都该带 [wholeAuth]（库里那串 `Basic …` 整串）。
	// 曾经这里钉的是"Basic 的第三段裸 authToken"（`ak|bk|ck|…`），那一版真跑被上游
	// 以 `01000002 鉴权失败` 否掉 ⇒ 按实拍改成整串（全程见 `task_matrix_play_ai.go` 文件头）。
	mxCameraUserDomainID = "1990000000000000001"
	mxCameraFileID       = "SYNTH-CAMERA-FID-1"
	mxCameraTaskID       = "1991452320654084604"
)

// mxCameraTIDShape 是 `x-yun-tid` 的形状（我们按旧实现 `randomHex(8)` ⇒ 16 个十六进制字符）。
// 只比形状不比值：值由注入的随机源造，钉字面量等于把"每一发换一枚"写成"永远那一枚"。
//
// ⚠ 这一格与实拍**已知不同**：真机那两条的 `x-yun-tid` 是 36 字符（UUID 形态）。
// 上游比不比这一格 ⇒ **未测试**；① 换成整串凭据后若仍被拒，这是第一个要逐格试的偏离
// （成文在 `task_matrix_play_ai.go` 文件头）。
var mxCameraTIDShape = regexp.MustCompile(`^[0-9a-f]{16}$`)

// mxCameraMillisecondNameShape 是"① 没回 taskId"时那个文件名的形状（13 位毫秒戳 + .jpeg）。
var mxCameraMillisecondNameShape = regexp.MustCompile(`"name":"[0-9]{13}\.jpeg"`)

// mxCameraRecognizeBody 造 ① 的响应。`fileID` / `taskID` 传空串 ⇒ **不给**那一格 ——
// 演"说成功了却没给 id"那一档要靠它，所以两格分开可选。
//
// ⚠ `code` 是**字符串** `"0000"`：这一族的解析器对数字形态直接报 parse 错
// （`app_play_ai.go` 里那句"实得 %T"），夹具写错会让红出现在解析层，
// 而排查目标是编排层。
func mxCameraRecognizeBody(fileID, taskID string) string {
	var data []string
	if fileID != "" {
		data = append(data, `"fileId":"`+fileID+`"`)
	}
	if taskID != "" {
		data = append(data, `"taskId":"`+taskID+`"`)
	}
	return `{"code":"0000","message":"请求成功","success":true,"data":{` +
		strings.Join(data, ",") + `}}`
}

const (
	// mxCameraChatDataLine 命中第一档（倒序扫到的第一条可解析 `data:` 行），且判成成功。
	//
	// ⚠ 它**故意不带** `data.flowResult` ⇒ 这一档的 `Answer` 是空的，播报因此念 `原文`。
	// 之所以留这一档：真实上游回的 `message` 是"请求成功"这类**信封**文案、不是模型那一句
	// （实拍两条都是这样，见 `docs/capture/app-play-ai-samples.txt` §8），
	// 而"没读到回答时要念原文"正是 [playAICameraReadLine] 的那一条分支 —— 这里把它演出来。
	// 念回答那一档在 [mxCameraChatStreamedAnswer]。
	mxCameraChatDataLine = "event: message\ndata: {\"code\":\"0000\",\"success\":true," +
		"\"message\":\"请求成功\"}\n\n"
	// mxCameraChatRawText 命中第三档：整段不是 JSON ⇒ "原文当一个字符串、非空即真" ⇒
	// 旧判据把它念成**成功**。这一档存在的意义就是下面那条 Warn 用例。
	mxCameraChatRawText = "<html><body>502 Bad Gateway</body></html>"
	// mxCameraChatRejected 命中第一档但判成**失败**（`success:false` 且 `code` 既不是
	// `"0000"` 也不是数字 0 —— 字符串 `"0"` 刻意不判成成功，那是 JS 严格相等那一格）。
	mxCameraChatRejected = "data: {\"code\":\"0\",\"success\":false,\"message\":\"额度不足\"}"
)

// mxCameraChatStreamedAnswer 是**实拍那两条**的形状：逐条 `data:` 各带一个分片，
// **末条**（`finishReason:"stop"`）那一片是**空串** ⇒ 只有"逐行累加"才念得出模型那句话。
// 三条里前两条合起来是回答，第三条空 —— 与实拍同构（分片 1–3 码点、`index` 从 0 严格 +1）。
var mxCameraChatStreamedAnswer = mxChatAnswerLines("这是", "一张白图。", "")

// mxChatAnswerLines 按实拍那两条的键路径造若干条 `data:` 行（最后一条置 `stop`）。
//
// 写成函数而不是抄真原文：实拍的每一条事件带十几格（`dialogueId`、`sessionId`、
// `recommend`、`responseStatement`…），本层只读四格 ⇒ 抄全文等于把"本层认得的形状"
// 伪装成"上游回的形状"。
func mxChatAnswerLines(chunks ...string) string {
	lines := make([]string, 0, len(chunks))
	for i, chunk := range chunks {
		reason := "processing"
		if i == len(chunks)-1 {
			reason = "stop"
		}
		lines = append(lines, fmt.Sprintf(
			`data: {"code":"0000","message":"请求成功","success":true,"data":{`+
				`"finishReason":%q,"flowResult":{"index":%d,"outContent":%q,"finishReason":%q}}}`,
			reason, i, chunk, reason))
	}
	return strings.Join(lines, "\n") + "\n\n"
}

// mxCameraStore 给一份**带用户域**的绑定态。
//
// `withDomain=false` 那一档演的是"老库里没这一格 / 登录响应没给"：这一层此时
// **一枪都不发**，而不是拿空串去问上游。
func mxCameraStore(withDomain bool) *mxStore {
	store := mxStoreBound()
	if withDomain {
		domain := core.UserDomainID(mxCameraUserDomainID)
		store.state.UserDomainID = &domain
	}
	return store
}

// cameraRun 装配一条链（与 [mailRun] 同一份接法，只是把夹具换成带用户域那一份）。
func cameraRun(t *testing.T, script *liveTaskScript, progress RunProgress) TaskMatrixRun {
	t.Helper()
	run := mxRun(t, mxCameraStore(true), script, NewSessionPool(), testClock(), &recordingWaiter{})
	run.Progress = progress
	return run
}

// cameraScript 把"一条 585 + 两枪的响应"编成一份脚本。
//
// `chatBody` 传空串 ⇒ **不给** ② 编路由（夹具的 404 分支就是"被拒"那一档，
// 见本文件抬头第二条）。
func cameraScript(t *testing.T, recognizeBody, chatBody string) *liveTaskScript {
	t.Helper()
	waiting := mxItems(mxItem(585, "day", "AI 相机：拍照问一句", "WAIT", "", 0))
	routes := []liveTaskRoute{{mxCameraRecognizeEndpoint, []string{recognizeBody}}}
	if chatBody != "" {
		routes = append(routes, liveTaskRoute{mxCameraChatEndpoint, []string{chatBody}})
	}
	routes = append(routes, mxHappy()...)
	return mxScript([]string{mxCameraTicket}, waiting, nil, routes...)
}

// cameraResult 跑一趟。
func cameraResult(t *testing.T, run TaskMatrixRun) (TaskMatrixRunResult, *core.Error) {
	t.Helper()
	return run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
}

// ─────────────────────────────────────────────────────────────────────────────
// 分派：585 该被认成玩法，而不是缺口
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanTaskStepSelectsAICameraTask ⭐ 585 走 runner，且形状只有 AICamera 那一格。
//
// 反向改坏它：把 585 挪回 [taskMatrixToolGaps] ⇒ 这一条红；给 spec 顺手挂上
// `FinalKey` ⇒ 也红（那会多打一发通用 `task/click`，旧实现那条 runner 只有一句
// `completeAiCameraTask`）。
func TestPlanTaskStepSelectsAICameraTask(t *testing.T) {
	item := dispatchItem(585, "day", "AI 相机：拍照问一句", "WAIT", 0)
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepRunner {
		t.Errorf("585 该走专属玩法，实得 %v（%s）", plan.Kind, plan.Reason)
	}
	if plan.Runner == "" {
		t.Errorf("结论要能说出是哪一条玩法做的，实得空")
	}
	if plan.ClickKey != taskClickKeyDefault {
		t.Errorf("585 的前置那一发仍按通用门走（key=task），实得 %q", plan.ClickKey)
	}
	spec, ok := taskMatrixRunners[585]
	if !ok {
		t.Fatal("585 不在 runner 表里")
	}
	if !spec.AICamera || spec.Upload != nil || spec.Shares || spec.Notes || spec.Mail ||
		spec.MailBackup || spec.MailPDF || spec.FinalKey != "" || spec.OpensEntry {
		t.Errorf("585 的形状只该有 AICamera 那一格，实得 %+v", spec)
	}
	if _, gap := taskMatrixToolGaps[585]; gap {
		t.Errorf("585 接上后没从缺口表删掉 ⇒ 界面上会永远显示『没接玩法』")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 正常那一趟：两枪各一发、两枪同一枚凭据、写盘记账
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunAICameraRecognizesThenChats ⭐ 585 的整条链跑通那一档。
//
// 七格判据各挡一种错：
//   - **两枪各恰好一发**：多一发就是往用户网盘多写一张图 / 多问一句。
//   - **时序**：② 的 fileId 只能来自**这一次** ① 的响应（旧实现读的是共享状态，
//     同 1014 那一格，见 `task_matrix_mail_run_test.go` 里同一处口径）。
//   - **两枪同一枚凭据，而它不是票**（本文件最贵的一格）：①② 都带库里那串 Basic 整串。
//     接错时请求看起来完全正常（头名、头数、体形状全对），只有按值比才看得出来 ——
//     而"① 带票"与"① 带 Basic 的第三段裸 authToken"两种接法都**真跑过**，都回鉴权失败。
//   - **`userId` 用库里那一份**，两枪同一个值。
//   - ① 的样图走 **data URL** 那一档（调用点的选型，协议层两档都给）。
//   - 传上去的图进清理链（链尾一次 `batchTrash`，删的是同一枚 id）。
//   - **`inputTime` 是 UTC + 字面量 `Z`**（旧实现那一套），值来自**注入的时钟**。
//     ⚠ 实拍那一档不是这个形状：真机给的是**本地挂钟 + `+08:00`** ⇒ 这一格是已知偏离，
//     成文与后果见 `task_matrix_play_ai.go` 文件头"`name` / `inputTime` / `sessionId`"。
func TestTaskMatrixRunAICameraRecognizesThenChats(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
		mxCameraChatDataLine)
	log := &mxLog{}
	res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxCameraRecognizeEndpoint); n != 1 {
		t.Errorf("① 该恰好一发，实得 %d", n)
	}
	if n := script.count(mxCameraChatEndpoint); n != 1 {
		t.Errorf("② 该恰好一发，实得 %d", n)
	}
	// 时序：建会话（tyrzLogin）→ ① → ② → 链尾清理。
	// ⚠ 那一发建会话是**通用前置打卡**那条链要的票，与 585 无关（本族两枪都不带票）：
	// 所以这一格只判**先后**，凭据的选型全靠下面那两条按值比。
	session, recognize, chat, trash := requestIndex(script, "tyrzLogin"),
		requestIndex(script, mxCameraRecognizeEndpoint),
		requestIndex(script, mxCameraChatEndpoint), requestIndex(script, mxTrashEndpoint)
	if !(session >= 0 && recognize > session && chat > recognize && trash > chat) {
		t.Errorf("时序应为 建会话>识图>对话>清理，实得 %d / %d / %d / %d（全部：%v）",
			session, recognize, chat, trash, mxURLs(script))
	}
	recognizeRequest, has := mailRequest(script, mxCameraRecognizeEndpoint)
	if !has {
		t.Fatal("没看到 ① 那一枪")
	}
	chatRequest, has := mailRequest(script, mxCameraChatEndpoint)
	if !has {
		t.Fatal("没看到 ② 那一枪")
	}
	// ⭐ 凭据：**两枪同一枚**，就是库里那串 `Basic …` 整串，而**它不是票**。
	// 三种接错各挡一种：① 接成票（"顺手复用会话"最自然的方向）、① 接成 Basic 的第三段
	// 裸 authToken（照抄参考实现）、两枪各接一枚。前两种都**真跑过**，都回 `01000002 鉴权失败`；
	// 实拍里 ①② 的 `Authorization` 逐字同一枚。
	for name, request := range map[string]core.HTTPRequest{"①": recognizeRequest, "②": chatRequest} {
		if got := headerValue(request, "authorization"); got != wholeAuth {
			t.Errorf("%s 的 Authorization 该是库里那串 Basic 整串（实拍 ①② 逐字同一枚；"+
				"旧实现 ① 取 ctx.config.token 是它的第三段，那一枚真跑过、被拒），实得 %q", name, got)
		}
	}
	// 两枪各带一枚 tid，形状对（值由随机源造 ⇒ 只比形状）。
	for name, request := range map[string]core.HTTPRequest{"①": recognizeRequest, "②": chatRequest} {
		if got := headerValue(request, "x-yun-tid"); !mxCameraTIDShape.MatchString(got) {
			t.Errorf("%s 的 x-yun-tid 要形如 16 位十六进制（旧实现 randomHex(8)），实得 %q", name, got)
		}
	}
	// userId：两枪同一个值，且就是库里那一份。
	for name, request := range map[string]core.HTTPRequest{"①": recognizeRequest, "②": chatRequest} {
		if body := string(request.Body); !strings.Contains(body,
			`"userId":"`+mxCameraUserDomainID+`"`) {
			t.Errorf("%s 的 userId 该用库里那份 userDomainId，实得 %s", name,
				playAICameraLogText(body))
		}
	}
	// ① 的选型：样图是 data URL 那一档，`imageExt` 用有出处的那一枚取值。
	if body := string(recognizeRequest.Body); !strings.Contains(body,
		`"base64":"data:image/jpg;base64,`) {
		t.Errorf("① 的样图该是 data URL 那一档（唯一有实拍支撑的形态），实得 %s",
			playAICameraLogText(body))
	}
	if body := string(recognizeRequest.Body); !strings.Contains(body, `"imageExt":"jpg"`) {
		t.Errorf("① 的 imageExt 用两处出处一致的那一枚取值，实得 %s", playAICameraLogText(body))
	}
	// ② 的体：fileId 是**这一次** ① 回的那一枚；文件名用 taskId 原值 + `.jpeg`。
	// ⚠ 真机那一格是 **taskId 的精确十进制加一**（旧实现那句 `parseInt(taskId,10)+1` 方向对、
	// 但过 float64 会掉末几位）。本层用原值是一处**已知偏离**，取舍与"上游读不读"登记在
	// `task_matrix_play_ai.go` 文件头。
	body := string(chatRequest.Body)
	if !strings.Contains(body, `"fileId":"`+mxCameraFileID+`"`) {
		t.Errorf("② 问的必须是这一次识出来的那一份，实得 %s", playAICameraLogText(body))
	}
	if !strings.Contains(body, `"name":"`+mxCameraTaskID+playAICameraFileSuffix+`"`) {
		t.Errorf("② 的文件名该是 taskId 原值 + %s，实得 %s", playAICameraFileSuffix,
			playAICameraLogText(body))
	}
	// 注入时钟那一档：testClock() 是 2026-09-20 12:00:00.123456 UTC ⇒ `.000Z` 形态。
	if !strings.Contains(body, `"inputTime":"2026-09-20T12:00:00.123Z"`) {
		t.Errorf("② 的 inputTime 该由注入时钟给出 UTC+Z 那一套，实得 %s", playAICameraLogText(body))
	}
	if !strings.Contains(body, `"dialogue":"？"`) {
		t.Errorf("② 问的那一句是旧实现写死的全角问号，实得 %s", playAICameraLogText(body))
	}
	// 记账：图进了清理链，链尾删的就是同一枚 id。
	if len(res.CloudFileIDs) != 1 || res.CloudFileIDs[0] != mxCameraFileID {
		t.Errorf("识出来的 fileId 要进清理链，实得 %v", res.CloudFileIDs)
	}
	if n := mxTrashCount(script); n != 1 ||
		!strings.Contains(mxTrashBodies(script)[0], mxCameraFileID) {
		t.Errorf("链尾该把这一份移进回收站，实得 %d 次 / %v", n, mxTrashBodies(script))
	}
	if res.Steps[0].Uploads != 1 || res.Counts.Uploaded != 1 {
		t.Errorf("这一条要按'写了一个文件'记账，实得 %+v / %+v", res.Steps[0], res.Counts)
	}
	// 播报：识图一行、对话一行，且对话那行**说命中哪一档**。
	if !log.hasAt(core.LevelInfo, "AI 相机识图已回 fileId") {
		t.Errorf("① 成功要单独念一行（它说了网盘里多了一张图），实得：\n%s", log.joined())
	}
	line := log.lineWith("AI 相机对话已回")
	if !strings.Contains(line, "档=data 行") {
		t.Errorf("成功那一行必须念出命中哪一档（不念就会与'报错页被当成一个字符串'同形），实得 %q",
			line)
	}
	if strings.Contains(log.joined(), "AI 相机失败") || strings.Contains(log.joined(), "AI 相机跳过") {
		t.Errorf("全成功的一趟不许念失败/跳过：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunAICameraFallsBackToMillisecondFileName ⭐ ① 没回 taskId ⇒ 文件名退到毫秒戳。
//
// 这一格钉的是"退路也要可复现"：退路如果直接写 `time.Now()`，夹具里每次跑都是不同串，
// 而它**看起来一直是绿的**。毫秒戳由注入时钟给出 ⇒ 只比形状（13 位）。
func TestTaskMatrixRunAICameraFallsBackToMillisecondFileName(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, ""), mxCameraChatDataLine)
	if _, perr := cameraResult(t, cameraRun(t, script, (&mxLog{}).handle())); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	chatRequest, has := mailRequest(script, mxCameraChatEndpoint)
	if !has {
		t.Fatal("没看到 ② 那一枪")
	}
	if body := string(chatRequest.Body); !mxCameraMillisecondNameShape.MatchString(body) {
		t.Errorf("没有 taskId 时文件名该是 13 位毫秒戳 + %s，实得 %s",
			playAICameraFileSuffix, playAICameraLogText(body))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 一枪都不发 / 只发一枪那三种形态
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunAICameraWithoutUserDomainIDSendsNothing ⭐ 库里没有用户域 ⇒ 两枪都不发。
//
// 这一格挡的是"拿空 userId 去问上游"：① 的护栏（协议层那句"填错会打到别人的账号上"）
// 会让请求压根构造不出来，而表现是"报一个 parse 错"，看不出是接线缺格。
// ⚠ 通用前置那一发**照发**（它与 585 无关，是这张表所有 runner 共有的前置），
// 所以断言单位是"这两个端点各 0 次"，不是"整趟零请求"。
func TestTaskMatrixRunAICameraWithoutUserDomainIDSendsNothing(t *testing.T) {
	waiting := mxItems(mxItem(585, "day", "AI 相机：拍照问一句", "WAIT", "", 0))
	script := mxScript([]string{mxCameraTicket}, waiting, nil, mxHappy()...)
	log := &mxLog{}
	run := mxRun(t, mxCameraStore(false), script, NewSessionPool(), testClock(), &recordingWaiter{})
	run.Progress = log.handle()
	res, perr := cameraResult(t, run)
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	for _, forbidden := range []string{mxCameraRecognizeEndpoint, mxCameraChatEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("没有用户域就不该发 %s，实得 %d 次", forbidden, n)
		}
	}
	if n := mxTrashCount(script); n != 0 {
		t.Errorf("不是我们造的那一格不许删，实得 %d 次", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "userDomainId") || !strings.Contains(text, "宁可不发") {
		t.Errorf("要说清'为什么不发'与'为什么不能猜'，实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "AI 相机失败") {
		t.Errorf("这一档是 error（本工具侧缺一样输入），实得：\n%s", log.joined())
	}
	if res.Counts.Ran != 0 || res.Counts.Failed != 1 {
		t.Errorf("没做成要落失败那一档（不是'跑过了'），实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunAICameraWithoutAuthorizationSendsNothing ⭐ 登录态里没有 Authorization 整串 ⇒ 两枪都不发。
//
// 这一格挡的是"拿空 Authorization 出网"：协议层不设这一格（`authorization` 是必填参数，
// 但空串照样拼得出发得出去），表现就是一发必被拒的 `01000002`，而错误串里看不出
// "是我们这边少了一格"。
// ⚠ 它**不判**"能不能从整串里拆出第三段"：曾经这里判的是那一格，而 ① 换成拆出来的
// 裸 authToken 之后真跑仍被拒 ⇒ 两枪都用整串，守卫也跟着回到"整串在不在"这一问。
//
// ⚠ 这一组**不走整条链**，两条理由叠在一起：
//   - [SignInWorld.Open] 自己就先拦"库里没有可用凭据"那种账号（实得
//     `凭据失效: … authToken 为空`），所以正常装配下**到不了** runner 的这一格 ——
//     这里是把正常那份 opening 的凭据**手工换空**，测的就是这一层自己的守卫；
//   - 时钟 / 随机源同理（见 [TestTaskMatrixRunAICameraWiringMissing]）。
func TestTaskMatrixRunAICameraWithoutAuthorizationSendsNothing(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
		mxCameraChatDataLine)
	log := &mxLog{}
	run := cameraRun(t, script, log.handle())
	account := core.NewAccountID(liveTestAccountID)
	opening, perr := run.SignInWorld.Open(context.Background(), account)
	if perr != nil {
		t.Fatalf("装配世界失败: %v", perr)
	}
	// 整串换空，账号与其余格子不动（`Credential` 是指针字段，要另建一枚）。
	opening.Credential = &SignInCredential{accountID: account}
	executor := newTaskExecutor(run, opening, account)
	record := &TaskStepRecord{}
	fired, problems := executor.runAICamera(context.Background(),
		dispatchItem(585, "day", "AI 相机：拍照问一句", "WAIT", 0), record)
	if fired {
		t.Errorf("没有凭据时不许交回'发过'（那会把它记成'今天动过'）")
	}
	text := joinProblems(problems)
	if !strings.Contains(text, "Authorization") || !strings.Contains(text, "两枪都不发") {
		t.Errorf("要说清缺哪一格与后果，实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "AI 相机失败") {
		t.Errorf("这一档是本工具侧缺一样输入 ⇒ error，实得：\n%s", log.joined())
	}
	for _, forbidden := range []string{mxCameraRecognizeEndpoint, mxCameraChatEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("没有 Authorization 就不该发 %s，实得 %d 次", forbidden, n)
		}
	}
	if record.Uploads != 0 || len(executor.cloudFileIDs) != 0 {
		t.Errorf("没发就没有账，实得 %+v / %v", record, executor.cloudFileIDs)
	}
}

// TestTaskMatrixRunAICameraRecognizeFailureSkipsChat ⭐ ① 挂 ⇒ ② 不发，网盘里也不该多一张图。
//
// 反向改坏它：把"① 失败还继续"写进去 ⇒ ② 带着空 fileId 发出去（协议层会拦，
// 红成"② 发了 1 次"）；把记账挪到 ① 失败那一支 ⇒ `mxTrashCount` 会多删一枚
// 我们并没有造出来的 id。
func TestTaskMatrixRunAICameraRecognizeFailureSkipsChat(t *testing.T) {
	script := cameraScript(t,
		`{"code":"58999","message":"当前用户信息读取异常","success":false}`, mxCameraChatDataLine)
	log := &mxLog{}
	res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxCameraChatEndpoint); n != 0 {
		t.Errorf("① 失败就不该发 ②，实得 %d", n)
	}
	if n := mxTrashCount(script); n != 0 {
		t.Errorf("没写成就没有东西要清，实得 %d 次", n)
	}
	if len(res.CloudFileIDs) != 0 || res.Counts.Uploaded != 0 {
		t.Errorf("① 失败时两本账都该是空，实得 %v / %+v", res.CloudFileIDs, res.Counts)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "识图（①）失败") || !strings.Contains(text, "58999") {
		t.Errorf("失败文本要带上游那一句的原话（58999 是线上采集里的那一档），实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "AI 相机失败") {
		t.Errorf("实得：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunAICameraWithoutFileIDSkipsChat ⭐ ① 说成功了却没给 fileId。
//
// 这一档**刻意不**"退回用别的 id 问一次"：手上没有可问的那一份，问哪个都是猜。
// 它同时是清理链**收不到东西**的那一格 —— 那张图很可能已经在网盘里了，
// 而本层拿不到可删的标识 ⇒ 播报必须念出来（念出来不等于删掉了，成文在 R37）。
func TestTaskMatrixRunAICameraWithoutFileIDSkipsChat(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody("", mxCameraTaskID), mxCameraChatDataLine)
	log := &mxLog{}
	res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	if n := script.count(mxCameraChatEndpoint); n != 0 {
		t.Errorf("没有 fileId 就不该发 ②，实得 %d", n)
	}
	if len(res.CloudFileIDs) != 0 || res.Steps[0].Uploads != 0 {
		t.Errorf("拿不到 id 就不可能有清理账，实得 %v / %+v", res.CloudFileIDs, res.Steps[0])
	}
	if n := mxTrashCount(script); n != 0 {
		t.Errorf("账上没有东西时不许发清理枪，实得 %d 次", n)
	}
	line := log.lineWith("AI 相机跳过")
	if line == "" || !strings.Contains(line, "可能已经在网盘里") {
		t.Errorf("要念出'图可能留下了而删不掉'，实得：\n%s", log.joined())
	}
	if !log.hasAt(core.LevelWarn, "AI 相机跳过") {
		t.Errorf("这一档是 warn（上游说成功了，但我们没拿到东西），实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "AI 相机识图已回") {
		t.Errorf("没拿到 fileId 不许念「识图已回」：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("没做成要落失败那一档，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunAICameraChatRejectedStillTrashesImage ⭐ ② 被拒 ⇒ ① 那张图仍要删。
//
// 这一格钉的是**副作用收尾**：写已经发生了，问句成没成不改变"要把它清掉"。
// 只在成功时记账的写法会留下一张没人认领的图 —— 那正是本仓给这一族加清理链的全部理由。
// ⚠ 夹具永远回 200 ⇒ "被拒"只能靠**不给路由**那一档演（落到 404）。
func TestTaskMatrixRunAICameraChatRejectedStillTrashesImage(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID), "")
	log := &mxLog{}
	res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("② 被拒不该让整趟报错: %v", perr)
	}
	if len(res.CloudFileIDs) != 1 || res.CloudFileIDs[0] != mxCameraFileID {
		t.Errorf("① 成了就要记账，哪怕 ② 挂，实得 %v", res.CloudFileIDs)
	}
	if n := mxTrashCount(script); n != 1 ||
		!strings.Contains(mxTrashBodies(script)[0], mxCameraFileID) {
		t.Errorf("链尾仍要删这一份，实得 %d 次 / %v", n, mxTrashBodies(script))
	}
	if res.Steps[0].Uploads != 1 || res.Counts.Uploaded != 1 {
		t.Errorf("写了网盘就要按'写了一个文件'记账，实得 %+v / %+v", res.Steps[0], res.Counts)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "对话（②）失败") || !strings.Contains(text, "移进回收站") {
		t.Errorf("要说清'哪一枪挂了'与'图仍会被清掉'，实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "AI 相机失败") {
		t.Errorf("实得：\n%s", log.joined())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 播报三档：SSE 读法命中哪一档，事件流就得说哪一档
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunAICameraChatTiers ⭐ 三档读数各念哪一级、哪一句。
//
// 这一组存在的理由只有一条：**`Success==true` 不等于"读到了模型那句话"**。
// 旧判据第三档是"整段原文当一个字符串、非空即真"，一段 502 报错页走的就是这一档
// ⇒ 如果这里只按 `Success` 分两档念，界面上会出现一条"成功"而没人看得出
// 它压根没读到任何 `data:` 事件。档位必须**逐字**进事件流，所以每条用例连
// `档=` 一起钉。
func TestTaskMatrixRunAICameraChatTiers(t *testing.T) {
	for _, tier := range []struct {
		name      string
		chatBody  string
		wantLevel core.LogLevel
		// want 是那一行的前缀（三档各不同：已回 / 存疑 / 失败）。
		want     string
		wantTail string
		wantFail bool
	}{
		{
			name: "data 行且判成成功", chatBody: mxCameraChatDataLine,
			wantLevel: core.LevelInfo, want: "AI 相机对话已回",
			wantTail: "档=data 行、data 行数=1", wantFail: false,
		},
		{
			// 第三档：**判成成功**却一条事件都没读到 ⇒ Warn + 并进失败那一档。
			name: "整段原文（报错页被当成一个字符串）", chatBody: mxCameraChatRawText,
			wantLevel: core.LevelWarn, want: "AI 相机存疑",
			wantTail: "档=原文、data 行数=0", wantFail: true,
		},
		{
			// `code:"0"` 两个都不等（JS 严格相等那格）⇒ 判成失败。
			name: "data 行但判成失败", chatBody: mxCameraChatRejected,
			wantLevel: core.LevelError, want: "AI 相机失败",
			wantTail: "code=0、message=", wantFail: true,
		},
	} {
		t.Run(tier.name, func(t *testing.T) {
			script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
				tier.chatBody)
			log := &mxLog{}
			res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
			if perr != nil {
				t.Fatalf("跑一趟失败: %v", perr)
			}
			line := log.lineWith(tier.want)
			if line == "" {
				t.Fatalf("没念到「%s」那一行：\n%s", tier.want, log.joined())
			}
			if !log.hasAt(tier.wantLevel, tier.want) {
				t.Errorf("「%s」该是档位 %v，实得：\n%s", tier.want, tier.wantLevel, log.joined())
			}
			if !strings.Contains(line, tier.wantTail) {
				t.Errorf("那一行要带「%s」，实得 %q", tier.wantTail, line)
			}
			// 图是 ① 造的 ⇒ 三档都得清（判据只说"问句成没成"，不说"写没写"）。
			if n := mxTrashCount(script); n != 1 {
				t.Errorf("链尾该删那一张图，实得 %d 次", n)
			}
			failed := res.Counts.Failed == 1
			if failed != tier.wantFail {
				t.Errorf("失败档位该是 %v（判成成功但没读到事件也算失败：它交回一条 problem），实得 %+v",
					tier.wantFail, res.Counts)
			}
			if tier.wantFail && !strings.Contains(res.Steps[0].Err, tier.wantTail) {
				t.Errorf("结论里的原因要带上同一份读数（档位与 data 行数），实得 %q", res.Steps[0].Err)
			}
		})
	}
}

// TestTaskMatrixRunAICameraReadsOutTheModelSentence ⭐ 读到回答时念 `模型回话=`、**不**念 `原文`。
//
// 这一条钉的是本族最贵的一种误读：`Success==true` 且 `message` 有值，**不代表**念出来的是
// 模型那句话。实拍两条 ② 响应的 `message` 都是"请求成功"这一类**信封**文案，
// 而内容在**逐行累加**的 `data.flowResult.outContent` 里（末行那一条是空的）
// ⇒ 只念 `message` 的播报会把"请求成功"当成回答。
// 两条断言各挡一头：回答要拼出来念；拼出来之后**不再**念整条原文（那会挤掉那一句话）。
//
// 反向改坏它谁会红：删掉 [playAICameraReadLine] 里 `read.Answer != ""` 那一支 ⇒ 第一格红；
// 把"只在没读到回答时念原文"那个条件删掉 ⇒ 第二格红。
func TestTaskMatrixRunAICameraReadsOutTheModelSentence(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
		mxCameraChatStreamedAnswer)
	log := &mxLog{}
	if _, perr := cameraResult(t, cameraRun(t, script, log.handle())); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	line := log.lineWith("AI 相机对话已回")
	if line == "" {
		t.Fatalf("没念到对话那一行：\n%s", log.joined())
	}
	if !strings.Contains(line, `模型回话="这是一张白图。"`) {
		t.Errorf("回答应是三条分片顺序拼接后念出来，实得 %q", line)
	}
	if strings.Contains(line, "、原文=") {
		t.Errorf("读到回答后不该再念整条原文，实得 %q", line)
	}
	// 档位与行数与"有没有回答"无关，仍然要念（第一档那一条测试钉的是行数=1 那一档）。
	if !strings.Contains(line, "档=data 行、data 行数=3") {
		t.Errorf("要念出档位与 data 行数，实得 %q", line)
	}
	if !strings.Contains(line, `message="请求成功"`) {
		t.Errorf("信封那一格照旧原样带出（它不是回答，但念出来才能对齐实拍），实得 %q", line)
	}
}

// TestTaskMatrixRunAICameraUpstreamTextIsOneLine ⭐ 上游文本进事件流前要**压平 + 截断**。
//
// 本仓事件流一行一条，而 ② 回的是模型输出（一段 SSE 里最长的那条可能就是它）：
// 不压平会打断行，不截断会把日志撑爆。按**码点**截（按字节截会把中文切成半个）。
func TestTaskMatrixRunAICameraUpstreamTextIsOneLine(t *testing.T) {
	long := "data: {\"code\":\"1\",\"success\":false,\"message\":\"" +
		strings.Repeat("异", playAICameraLogTextRunes+40) + "\"}"
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID), long)
	log := &mxLog{}
	if _, perr := cameraResult(t, cameraRun(t, script, log.handle())); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	joined := log.joined()
	line := log.lineWith("AI 相机失败")
	if !strings.Contains(line, "截断，共") {
		t.Errorf("超长的那一段要标明截断与总码点数，实得：\n%s", joined)
	}
	if strings.Contains(line, "\n") {
		t.Errorf("上游那一段必须压成一行（事件流一行一条），实得 %q", line)
	}
	if got := strings.Count(messageSegment(line), "异"); got > playAICameraLogTextRunes {
		t.Errorf("message 那一段没截到预算内：带了 %d 个「异」，预算只有 %d",
			got, playAICameraLogTextRunes)
	}
}

// messageSegment 从那一行里只取出 `message=` 到 `、原文=` 之间那一段。
//
// 为什么要切：一行里**两处**带上游文本（`message` 与整条 `data:` 行的原文），
// 两处各有各的预算。整行数字数会得出"361 个"这种读数而它并不违反任何一条预算 ——
// 那种断言红了也不知道该修哪一头。
func messageSegment(line string) string {
	_, rest, ok := strings.Cut(line, "message=")
	if !ok {
		return ""
	}
	head, _, _ := strings.Cut(rest, "、原文=")
	return head
}

// ─────────────────────────────────────────────────────────────────────────────
// 凭据边界 + 缺接线
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunAICameraKeepsRequestSideSecretsOutOfLog ⭐ 票 / Basic 整串 / tid 不进日志。
//
// 这一条跑的是**全失败**那一趟：失败文本会把上游原话与错误串带进来，那正是最容易
// 顺手把请求侧的值一起拼进去的形状。**响应**侧文本可以截断后入日志（本族响应不含凭据），
// 请求侧任何值都不行 —— 口径见 `docs/sanitize-guide.md`。
// ⚠ 名单里**票也要留**：它压根不该出现在这一族的请求里，而"错误串里顺手带上刚读过的那张票"
// 是本族最容易踩的一种泄露（Basic 整串则是"该带、但不能打"）。
func TestTaskMatrixRunAICameraKeepsRequestSideSecretsOutOfLog(t *testing.T) {
	script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
		mxCameraChatRejected)
	log := &mxLog{}
	res, perr := cameraResult(t, cameraRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	tid := ""
	if request, has := mailRequest(script, mxCameraChatEndpoint); has {
		tid = headerValue(request, "x-yun-tid")
	}
	if tid == "" {
		t.Fatal("夹具这一趟没带 tid，这一条就没在测东西")
	}
	for _, leak := range []string{mxCameraTicket, wholeAuth, tid} {
		if strings.Contains(log.joined(), leak) || strings.Contains(res.Steps[0].Err, leak) {
			t.Errorf("事件流或失败文本里带了请求侧的值：%q", playAICameraLogText(leak))
		}
	}
}

// TestTaskMatrixRunAICameraWiringMissing ⭐ 缺随机源 / 缺时钟 ⇒ 各说一句缺哪一格，两枪都不发。
//
// ⚠ 这一组**不走整条链**：时钟与随机源在拉表、建会话那两步就已经被读走，
// 把它们置 nil 会让红出现在**那里**（panic），压根走不到 runner 的这一格守卫。
// 所以这里直接对 [taskExecutor] 调 [runAICamera] —— 它测的就是"半接线装配下这一层
// 怎么处置"，不是"整条链能不能跑"。
//
// 三档各挡一种错：两格都缺写成笼统一句 ⇒ 修法看不出（随机源在 [TaskMatrixRun.Random]、
// 时钟在 [TaskMatrixRun.Clock]，是两个字段）；只判其中一格 ⇒ 另一格在**发出去一半**
// 之后才炸（`x-yun-tid` 为空的请求看起来完全正常）。
func TestTaskMatrixRunAICameraWiringMissing(t *testing.T) {
	for _, missing := range []struct {
		name       string
		dropRandom bool
		dropClock  bool
		want       string
	}{
		{"只缺时钟", false, true, "时钟没接"},
		{"只缺随机源", true, false, "随机源没接"},
		{"两格都缺", true, true, "随机源与时钟都没接"},
	} {
		t.Run(missing.name, func(t *testing.T) {
			script := cameraScript(t, mxCameraRecognizeBody(mxCameraFileID, mxCameraTaskID),
				mxCameraChatDataLine)
			run := cameraRun(t, script, (&mxLog{}).handle())
			if missing.dropRandom {
				run.Random = nil
			}
			if missing.dropClock {
				run.Clock = nil
			}
			account := core.NewAccountID(liveTestAccountID)
			opening, perr := run.SignInWorld.Open(context.Background(), account)
			if perr != nil {
				t.Fatalf("装配世界失败: %v", perr)
			}
			executor := newTaskExecutor(run, opening, account)
			record := &TaskStepRecord{}
			fired, problems := executor.runAICamera(context.Background(),
				dispatchItem(585, "day", "AI 相机：拍照问一句", "WAIT", 0), record)
			if fired {
				t.Errorf("缺接线时不许交回'发过'（那会把它记成'今天动过'）")
			}
			text := joinProblems(problems)
			if !strings.Contains(text, missing.want) || !strings.Contains(text, "两枪都不发") {
				t.Errorf("要说清缺哪一格与后果，实得 %q", text)
			}
			if record.Uploads != 0 || len(executor.cloudFileIDs) != 0 {
				t.Errorf("没发就没有账，实得 %+v / %v", record, executor.cloudFileIDs)
			}
			for _, forbidden := range []string{mxCameraRecognizeEndpoint, mxCameraChatEndpoint} {
				if n := script.count(forbidden); n != 0 {
					t.Errorf("缺接线时不该发 %s，实得 %d 次", forbidden, n)
				}
			}
		})
	}
}
