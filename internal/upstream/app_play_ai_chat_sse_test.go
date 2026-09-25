package upstream

// 585「AI 相机」那一发 `chat/v2/add` 的用例（[`app_play_ai_chat_sse.go`](app_play_ai_chat_sse.go)）。
//
// # 本层是**纯**测试：只钉两件事
//
// ① 我们**发出**的这一组恰好是这个形状（URL / 方法 / 头集合 / body 字节）；
// ② 给一段**声明里那个形状**的响应文本，本层怎么处置它（三档回退 + 那张真值表）。
// 不联网、不起 server、不碰 data 目录，也不打任何真实上游。
//
// # ⚠ 断言分两类，措辞受限的是第二类（AGENTS.md §1.7）
//
// **判据类**断言钉的是"我们怎么处置这一段字节"，不是"上游会回什么"。
// 尤其 `Success` 与 `Source` 的组合：一段 HTML 报错页会被旧 TS 的读法判成**成功**
// （旧 TS 第三档是"原文非空即真"），本层照它实现并把这一格**钉住**而不是悄悄修掉 ——
// 修掉 = 替上游宣布它只在回 JSON 时才算成功，而这句话**没有样本**。
//
// **形状类**断言（[TestParsePlayAIChatSSEAnswerAccumulatesEveryLineButTheLast] 那三条）
// 现在有实拍撑着了：② 的真机 SSE 有**两条**观测（47 行 / 386 行，登记在
// `docs/capture/app-play-ai-samples.txt`），"逐行累加、末条为空、`index` 从 0 严格 +1"
// 都是从那里读来的。⚠ 但两条仍只说明**这两条**：不据此写"上游总是这么分片"。
// 夹具本身（[sseCameraEvent]）是**合成**的，只借路径与形状、不抄真原文。
//
// # 隐私
//
// `userId` 一律 `199…` 合成号；`Authorization`、`x-yun-tid`、样图 base64 都是合成或公开素材。

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// playAIChatCameraTestWhen 是一个**写死的**时刻：`inputTime` 两套房取的都是挂钟分量，
// 只有可预期的入参才能钉住那串文本。
func playAIChatCameraTestWhen() time.Time {
	return time.Date(2026, 6, 29, 12, 10, 43, 123_000_000, time.UTC)
}

// playAIChatCameraTestParams 给一份五格齐全的入参（与甲那一套的用例同一组值，
// 这样"两套的差别只在固定格上"这一句可以被逐字节比对）。
func playAIChatCameraTestParams() PlayAIChatParams {
	return PlayAIChatParams{
		UserID:    "1990000000000000001",
		Dialogue:  PlayAIChatCameraDialogue,
		InputTime: PlayAIChatCameraInputTime(playAIChatCameraTestWhen()),
		FileID:    "SYNTHETIC-FILE-ID",
		FileName:  "1990000000000000005.jpeg",
	}
}

// sseScriptClient 是"按脚本回放、且**允许同时交回响应与错误**"的假客户端。
//
// 为什么不复用 [scriptedClient]：那一份永远回 `nil` 错误，而坑表 #165 记的正是这一格 ——
// `internal/io` 的 `Send` 在**非 2xx** 时同时交回响应与错误（`return response, perr`）。
// 本层的发送口要兑现"先读体、再判错误"，就必须有一个能演出那一档的仪器。
// ⚠ 反过来（按 #168 的教训）：把脱敏/带体的反向用例打在**不会 echo** 的层上是假绿，
// 所以这里刻意让响应体带上可辨认的内容，断言才落在实处。
type sseScriptClient struct {
	status  uint16
	body    string
	sendErr error
	// echoErr 决定"非 2xx 时是否同时交回响应"（io 的真实行为）。
	echoErr  bool
	requests []core.HTTPRequest
}

func (c *sseScriptClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	response := core.NewResponse(c.status, []byte(c.body))
	if c.sendErr != nil {
		if c.echoErr {
			return response, c.sendErr
		}
		return core.HTTPResponse{}, c.sendErr
	}
	return response, nil
}

// playAIChatCameraSend 发一次（测试专用的一行包装：参数在六条用例里逐字相同）。
func playAIChatCameraSend(
	t *testing.T, up *Upstream, params PlayAIChatParams,
) (PlayAIChatSSERead, *core.Error) {
	t.Helper()
	return up.PlayAIChatCamera(context.Background(), playAITestProfile(),
		1750000000000, "Basic U1lOVEhFVElDLUJBU0lD-Q==", "TID-SYNTHETIC", params)
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求侧
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIChatCameraBodyIsTheLegacyShapeByteForByte 钉 585 那一套的 body 字节。
//
// 与甲那一套同形的部分（空串两格、`extInfo` 是串而 `sortInfo` 是对象、`attachmentTypeList:[3]`）
// 在这里再钉一遍**不是重复**：两套房各有自己的字节，而有人"统一成一个构造器"时
// 只会有一条红 —— 那条红就是本条存在的目的。
// 反向改坏它谁会红：给 body 补一个 `command` 格（"和拍拍那份对齐"）⇒ 本条红；
// 把 `h5Version` 写成 `1.0.0` ⇒ 本条红；动 `inputTime` 的格式 ⇒ 本条红。
func TestPlayAIChatCameraBodyIsTheLegacyShapeByteForByte(t *testing.T) {
	request, perr := PlayAIChatCameraRequest(playAITestProfile(), 1750000000000,
		"Basic U1lOVEhFVElDLUJBU0lD-Q==", "TID-SYNTHETIC", playAIChatCameraTestParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	want := `{"applicationId":"","applicationType":"chat","dialogueInput":{` +
		`"attachment":{"attachmentTypeList":[3],"fileList":[{"fileId":"SYNTHETIC-FILE-ID","name":"1990000000000000005.jpeg"}]},` +
		`"dialogue":"？","enableAllNetworkSearch":false,"enableForceLlm":false,"enableForceNetworkSearch":true,` +
		`"enableKnowledgeAndNetworkSearch":false,"enableModelThinking":false,"enableRegenerate":false,` +
		`"extInfo":"{}","inputTime":"2026-06-29T12:10:43.123Z","prompt":"","sortInfo":{},` +
		`"toolSetting":{"imageToolSetting":{"enableLlmDescribe":true}},` +
		`"versionInfo":{"h5Version":"2.7.6"}},` +
		`"sessionId":"","sourceChannel":"101","userId":"1990000000000000001"}`
	if got := playAISessionTestBody(t, "585 chat", request); got != want {
		t.Errorf("body 应逐字为 %q，实得 %q", want, got)
	}
	// 那一枚问号是**全角**的（旧 TS `ai-tasks.ts:112` 逐字），半角 `?` 是偏离出处。
	if !strings.Contains(want, `"dialogue":"？"`) {
		t.Error("dialogue 应钉成全角问号")
	}
}

// TestPlayAIChatCameraBodyDiffersFromThePlayAIRoundOnlyWhereRegistered 把"两套 chat 只有三处差别"
// 这一句钉死，而不是留成注释里的一句主张。
//
// 甲（`PlayAIChatBody`）与乙（本层）在旧 TS 里是**两个文件抄同一组常量**：
// 七枚开关、`extInfo`、`sortInfo`、`toolSetting`、`attachment` 两格全同，
// 只有 `command`（甲有乙无）、`h5Version`（1.0.0 / 2.7.6）、`dialogue` 与 `inputTime` 的格式不同。
// 为什么值得单列一条：将来有人看到"两套房"会想合并，而合并**只能**在这些格上出错；
// 本条把这些格逐一点名，其余一律要求相同 ⇒ 合并的冲动一出现就红。
// 反向改坏它谁会红：把 `PlayAIChatCameraH5Version` 改成 `1.0.0`（"统一"）⇒ 本条红。
func TestPlayAIChatCameraBodyDiffersFromThePlayAIRoundOnlyWhereRegistered(t *testing.T) {
	params := playAIChatCameraTestParams()
	// 甲那一套的 `inputTime` 用**它自己**的格式（`+08:00` 后缀），这是第四处差别，
	// 由 [PlayAIChatInputTime] / [PlayAIChatCameraInputTime] 各钉各的；比对时先固定入参。
	roundOne := PlayAIChatBody(PlayAIChatParams{
		UserID: params.UserID, Dialogue: params.Dialogue,
		InputTime: PlayAIChatInputTime(playAIChatCameraTestWhen()),
		FileID:    params.FileID, FileName: params.FileName,
	})
	roundTwo := PlayAIChatCameraBody(params)

	one, two := mustFlatten(t, roundOne), mustFlatten(t, roundTwo)
	// 键集：甲比乙多**一整棵** `command`（两格），乙不比甲多任何键。
	for key := range one {
		if key == "dialogueInput.command.command" || key == "dialogueInput.command.subCommand" {
			if _, ok := two[key]; ok {
				t.Errorf("乙不该有 %s（旧 TS `ai-tasks.ts:111-129` 这一套没写 command）", key)
			}
			continue
		}
		if _, ok := two[key]; !ok {
			t.Errorf("甲有 %s 而乙没有 —— 两套房的键集只差 command 那一棵，别多差", key)
		}
	}
	for key := range two {
		if _, ok := one[key]; !ok {
			t.Errorf("乙多出 %s（甲没有）", key)
		}
	}
	// 值：只在**登记过的那几格**允许不同。
	registered := map[string]bool{
		"dialogueInput.versionInfo.h5Version": true, // 1.0.0 / 2.7.6
		"dialogueInput.inputTime":             true, // 挂钟+08:00 / UTC+Z
	}
	for key, value := range one {
		if strings.HasPrefix(key, "dialogueInput.command.") {
			continue
		}
		if other, ok := two[key]; ok && other != value && !registered[key] {
			t.Errorf("%s 两套房的值应相同（甲 %q / 乙 %q），不同就必须先登记", key, value, other)
		}
	}
	// 反过来：两格"允许不同"的必须**确实**不同，否则就是有人把两处差别抹平了一处。
	for key := range registered {
		if one[key] == two[key] {
			t.Errorf("%s 两套房的值本该不同（这是登记过的差别），实得同一个 %q", key, one[key])
		}
	}
}

// mustFlatten 把 body 那种嵌套 map 摊成 `a.b.c` → 字符串值，用于逐键比对。
//
// 值摊成串（含类型标记）而不是留 `any`：`any` 之间不能直接 `==`（map 不可比），
// 而"值是 `true` 还是字符串 \"true\""在这一族是**看得见**的差别，所以要带上类型。
func mustFlatten(t *testing.T, body map[string]any) map[string]string {
	t.Helper()
	encoded, err := json.Marshal(body)
	if err != nil {
		t.Fatalf("body 序列化失败: %v", err)
	}
	var generic map[string]any
	if err := json.Unmarshal(encoded, &generic); err != nil {
		t.Fatalf("body 反序列化失败: %v", err)
	}
	out := map[string]string{}
	var walk func(prefix string, value any)
	walk = func(prefix string, value any) {
		switch typed := value.(type) {
		case map[string]any:
			for key, child := range typed {
				next := key
				if prefix != "" {
					next = prefix + "." + key
				}
				walk(next, child)
			}
		default:
			out[prefix] = typedString(typed)
		}
	}
	walk("", generic)
	return out
}

// typedString 给值带上类型，使 `true`（布尔）与 `"true"`（字符串）不相等。
func typedString(value any) string {
	switch typed := value.(type) {
	case string:
		return "str:" + typed
	case bool:
		return "bool:" + strings.ToUpper(typedStringOf(typed))
	case float64:
		return "num:" + typedStringOf(typed)
	case nil:
		return "null"
	default:
		return "other:" + typedStringOf(typed)
	}
}

func typedStringOf(value any) string {
	if s, ok := value.(string); ok {
		return s
	}
	encoded, _ := json.Marshal(value)
	return string(encoded)
}

// TestPlayAIChatCameraRequestHeadersAreTheNativeSetWithSSEAccept 钉 585 这一发的头集合。
//
// 与 ⑤ [PlayAIRecognizeRequest] 同源（[playAIAppHeaders]）+ `Accept` 换成 SSE，
// 三处已知偏离沿用 [PlayAIChatRequest] 那份登记（设备画像 / 渠道号 / content-type）。
// 本条钉的是**我们发的是什么**，其中三件有独立价值：
//   - `Accept` 恰好一枚且是 `text/event-stream`（"先剥后补"没做对就会有两枚，请求看着仍正常）；
//   - `Content-Type` 恰好一枚且**不带 charset**（同上）；
//   - **没有** `jwttoken`、没有 `X-Tingyun`（这一族是原生侧凭据，不是 H5 那一套）。
func TestPlayAIChatCameraRequestHeadersAreTheNativeSetWithSSEAccept(t *testing.T) {
	request, perr := PlayAIChatCameraRequest(playAITestProfile(), 1750000000000,
		"Basic U1lOVEhFVElDLUJBU0lD-Q==", "TID-SYNTHETIC", playAIChatCameraTestParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := "https://ai.yun.139.com/api/outer/assistant/chat/v2/add"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if got, want := countHeader(request.Headers, "accept"), 1; got != want {
		t.Errorf("accept 应恰好 %d 枚（先剥后补），实得 %d", want, got)
	}
	if got := headerValue(request.Headers, "accept"); got != PlayAIChatSSEAccept {
		t.Errorf("Accept 应是 %q，实得 %q", PlayAIChatSSEAccept, got)
	}
	if got, want := countHeader(request.Headers, "content-type"), 1; got != want {
		t.Errorf("content-type 应恰好 %d 枚，实得 %d", want, got)
	}
	if got := headerValue(request.Headers, "content-type"); got != PlayAIPlainJSONContentType {
		t.Errorf("Content-Type 应是**不带 charset** 的 %q，实得 %q", PlayAIPlainJSONContentType, got)
	}
	if got := headerValue(request.Headers, "origin"); got != PlayAILocalOrigin {
		t.Errorf("Origin 应是原生侧那一串 %q，实得 %q", PlayAILocalOrigin, got)
	}
	for _, absent := range []string{"jwttoken", "x-tingyun", "cookie", "authorization-token"} {
		if countHeader(request.Headers, absent) != 0 {
			t.Errorf("这一族不发 %q（夹具与旧 TS 都没有这一枚）", absent)
		}
	}
	if request.IsRetryable() {
		t.Error("chat 是写请求且无幂等键 ⇒ 传输层不许自动重试")
	}
}

// TestPlayAIChatCameraAndRecognizePassTheAuthorizationThroughVerbatim 钉"两枚凭据在协议层是同一张口"。
//
// 实拍里 ①② 的 `Authorization` **逐字同一枚**（库里那串 `Basic …` 整串），而参考实现
// 不是这样：它 chat 用 `ctx.config.auth`（整串，`ai-tasks.ts:139`）、⑤ 用
// `ctx.config.token`（`parseAuth(auth)` 的第三段裸 authToken，`:24` + `utils/init-account.ts:52`）。
// ⇒ 本层**两发都只是把传进来的那一枚原样进头**，不做任何"该是哪一枚"的判断 ——
// 三枚（整串 / 裸 authToken / 那张票）在这里都是不透明串，协议层无从区分，
// 只有 app 层按值钉的调用点用例判得住（`task_matrix_play_ai_run_test.go`）。
// ⚠ 那一格是**实测踩过**的：① 传票、以及 ① 传裸 authToken，都回 `01000002 鉴权失败`
// （2026-09-26 真跑，见 [`internal/app/task_matrix_play_ai.go`](../app/task_matrix_play_ai.go) 文件头）。
// 反向改坏它谁会红：谁在协议层"顺手补一个 `Basic ` 前缀"或"顺手剥掉前缀"⇒ 本条按值比红。
func TestPlayAIChatCameraAndRecognizePassTheAuthorizationThroughVerbatim(t *testing.T) {
	const basic = "Basic U1lOVEhFVElDLUJBU0lD-Q=="
	chat, perr := PlayAIChatCameraRequest(playAITestProfile(), 1750000000000,
		basic, "TID-SYNTHETIC", playAIChatCameraTestParams())
	if perr != nil {
		t.Fatalf("chat 构造失败: %v", perr)
	}
	if got := headerValue(chat.Headers, "authorization"); got != basic {
		t.Errorf("chat 的 Authorization 应是传进去的那一枚（原样、不加不减前缀），实得 %q", got)
	}
	recognize, perr := PlayAIRecognizeRequest(playAITestProfile(), 1750000000000,
		basic, "TID-SYNTHETIC", PlayAIRecognizeParams{
			UserID: "1990000000000000001", DataURL: PlayAIChatCameraSampleDataURL(), ImageExt: "jpg",
		})
	if perr != nil {
		t.Fatalf("⑤ 构造失败: %v", perr)
	}
	if got := headerValue(recognize.Headers, "authorization"); got != basic {
		t.Errorf("⑤ 的 Authorization 应是传进去的那一枚（实拍里它与 chat 那发逐字相同），实得 %q", got)
	}
	// 反向一格：换一个明显不是 Basic 的串进去，⑤ 也**照原样发**（协议层不判形态）。
	// 这一格存在的理由是上面那句"本层无从区分"：不许将来有人在这里补一句
	// "不是 Basic 就剥前缀 / 就拒"，那会把 app 层的选型判据吞进协议层。
	other, perr := PlayAIRecognizeRequest(playAITestProfile(), 1750000000000,
		"SYNTHETIC-NOT-A-BASIC", "TID-SYNTHETIC", PlayAIRecognizeParams{
			UserID: "1990000000000000001", DataURL: PlayAIChatCameraSampleDataURL(), ImageExt: "jpg",
		})
	if perr != nil {
		t.Fatalf("⑤ 构造失败（换一枚不透明串之后）: %v", perr)
	}
	if got := headerValue(other.Headers, "authorization"); got != "SYNTHETIC-NOT-A-BASIC" {
		t.Errorf("⑤ 不该改写传进来的凭据（不加 `Basic ` 前缀也不剥），实得 %q", got)
	}
}

// TestPlayAIChatCameraRequestGuardsTheSameFiveFields 钉"五格护栏两套房共用一个函数"。
//
// 与甲那一套同一组用例、同一句错误文本（见 [playAIChatParamGuard]）：
// 两页的这五格同名同义，各写一遍 switch 的下一次演变一定是"只改了一处"。
// 反向改坏它谁会红：把 guard 换成只判三格 ⇒ 本条在后两格上红。
func TestPlayAIChatCameraRequestGuardsTheSameFiveFields(t *testing.T) {
	cases := []struct {
		name     string
		field    func(params *PlayAIChatParams)
		fragment string
	}{
		{"userId", func(p *PlayAIChatParams) { p.UserID = "" }, "userId"},
		{"dialogue", func(p *PlayAIChatParams) { p.Dialogue = "" }, "dialogue"},
		{"inputTime", func(p *PlayAIChatParams) { p.InputTime = "" }, "inputTime"},
		{"fileId", func(p *PlayAIChatParams) { p.FileID = "" }, "fileId"},
		{"name", func(p *PlayAIChatParams) { p.FileName = "" }, "name"},
	}
	for _, tc := range cases {
		params := playAIChatCameraTestParams()
		tc.field(&params)
		request, perr := PlayAIChatCameraRequest(playAITestProfile(), 1750000000000,
			"Basic U1lOVEhFVElDLUJBU0lD-Q==", "TID-SYNTHETIC", params)
		msg := playAITestErrMessage(t, "585 chat 缺 "+tc.name, perr)
		if !strings.Contains(msg, tc.fragment) {
			t.Errorf("报错应点名 %s，实得 %q", tc.fragment, msg)
		}
		if len(request.Body) != 0 || len(request.Headers) != 0 {
			t.Errorf("拒掉时不该留下半条请求（%s）", tc.name)
		}
		// 同一格在甲那一套里也必须被拒，且**错误文本一致** —— 这才是"共用一个函数"的证据。
		_, oneErr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
			"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
		if oneErr == nil || oneErr.Message() != msg {
			t.Errorf("甲那一套同一格的报错应与乙逐字相同，实得 %v", oneErr)
		}
	}
}

// TestPlayAIChatCameraInputTimeIsUTCWithALiteralZ 钉那串文本与甲那一套**不是同一格式**。
//
// 旧 TS 两处写法不同：`ai-tasks.ts:103` 是 `new Date().toISOString()`（UTC + `Z`），
// `task.ts:72-76` 的 `formatTime` 是本地挂钟分量 + 字面量 `+08:00`。
// 反向改坏它谁会红：把本函数改成复用 [PlayAIChatInputTime]（"两份拼法合一"）⇒ 本条红，
// 而且红得很安静：串仍然形如时间，只有钉字节的断言看得出来。
func TestPlayAIChatCameraInputTimeIsUTCWithALiteralZ(t *testing.T) {
	if got, want := PlayAIChatCameraInputTime(playAIChatCameraTestWhen()),
		"2026-06-29T12:10:43.123Z"; got != want {
		t.Errorf("应逐字为 %q，实得 %q", want, got)
	}
	// 入参带非 UTC 时区时也按**同一时刻**的 UTC 分量摊（`toISOString` 的语义）。
	tokyo := time.FixedZone("JST", 9*60*60)
	same := time.Date(2026, 6, 29, 21, 10, 43, 123_000_000, tokyo)
	if got, want := PlayAIChatCameraInputTime(same), "2026-06-29T12:10:43.123Z"; got != want {
		t.Errorf("同一时刻换时区应得同一个串，实得 %q（期望 %q）", got, want)
	}
	// 与甲那一套对**同一时刻**给出不同的串（这条是"两格式之别"的反向哨兵）。
	if PlayAIChatCameraInputTime(playAIChatCameraTestWhen()) ==
		PlayAIChatInputTime(playAIChatCameraTestWhen()) {
		t.Error("两套房的 inputTime 拼法本该不同")
	}
	// 纯函数：同入参同输出（本层所有构造函数都要能被逐字段断言的前提）。
	if PlayAIChatCameraInputTime(same) != PlayAIChatCameraInputTime(same) {
		t.Error("同一入参两次调用给了不同的串")
	}
}

// TestPlayAIChatCameraSampleHasBothShapesAndDataURLWrapsTheSameBytes 钉"两档都提供、都不内置"。
//
// ⑤ 的唯一定拍里 `base64` 装的是**整条 data URL**，而旧 TS 这一页发的是**裸串**
// （`ai-tasks.ts:13`）。本层不猜上游比不比前缀，所以两个函数都给（见文件头那条待验证）。
// 反向改坏它谁会红：把 [PlayAIChatCameraSampleDataURL] 的前缀改成 `image/png`
// （"字节确实是 PNG，修一下"）⇒ 本条红 —— 那是替上游宣布它比对 MIME 与真实格式。
func TestPlayAIChatCameraSampleHasBothShapesAndDataURLWrapsTheSameBytes(t *testing.T) {
	if !strings.HasPrefix(PlayAIChatCameraSampleDataURL(), "data:image/jpg;base64,") {
		t.Errorf("前缀应是实拍那一档的 data:image/jpg;base64,，实得 %q",
			PlayAIChatCameraSampleDataURL()[:min(len(PlayAIChatCameraSampleDataURL()), 40)])
	}
	if !strings.HasSuffix(PlayAIChatCameraSampleDataURL(), PlayAIChatCameraSampleBase64) {
		t.Error("data URL 应以**同一张图**的裸串结尾（两半必须同源）")
	}
	if strings.Contains(PlayAIChatCameraSampleBase64, ",") ||
		strings.HasPrefix(PlayAIChatCameraSampleBase64, "data:") {
		t.Error("裸串那一档就是裸串：不许有人把前缀挪进来")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应侧：三档回退 + 那张真值表
// ─────────────────────────────────────────────────────────────────────────────

// sseCameraEvent 造**一条** ② 的 `data:` 载荷，形状照实拍那两条的键路径
// （顶层 `code`/`message`/`success` + `data.flowResult.{index,outContent,finishReason}`）。
//
// 为什么写成函数而不是抄一段真原文进来：实拍那两条里每一条事件都带十几格
// （`dialogueId`、`sessionId`、`recommend`、`responseStatement`…），而本层只读其中四格；
// 抄全文等于把"本层认得的形状"伪装成"上游回的形状"。这里刻意只留被读的格 + 判据用的格。
func sseCameraEvent(index int, outContent, finishReason string) string {
	return `{"code":"0000","message":"请求成功","success":true,"data":{` +
		`"finishReason":"` + finishReason + `","flowResult":{` +
		`"index":` + strconv.Itoa(index) +
		`,"outContent":"` + outContent +
		`","finishReason":"` + finishReason + `"}}}`
}

// TestParsePlayAIChatSSETakesTheLastParsableDataLine 钉"倒序"这一格。
//
// 旧 TS 是 `lines.reverse()` 之后取**第一条**能 parse 的 `data:` 行（`ai-tasks.ts:153-162`），
// 也就是流的**最后**一条。正序实现会读到首事件 —— 两档的 `code` 可以不同，判据跟着翻。
// 所以本条把首末两条造成**相反**的读数（首：成功码；末：失败码）。
// 反向改坏它谁会红：把循环从 `0` 走到头 ⇒ `Code` 变成 `"0000"`、`Success` 变真，本条红。
func TestParsePlayAIChatSSETakesTheLastParsableDataLine(t *testing.T) {
	text := "data:{\"code\":\"0000\",\"message\":\"首事件\"}\n" +
		": ping\n" +
		"data:{\"code\":\"9999\",\"message\":\"末事件\"}\n" +
		"\n"
	read := ParsePlayAIChatSSE(text)
	if read.Source != PlayAIChatSSEFromDataLine {
		t.Fatalf("应命中 data 行那一档，实得 %v", read.Source)
	}
	if read.Code != "9999" || read.Message != "末事件" {
		t.Errorf("应取**最后**一条可解析的 data 行，实得 code=%q message=%q", read.Code, read.Message)
	}
	if read.Success {
		t.Error(`code "9999" 且 success 缺席 ⇒ 按旧 TS 判据应为假`)
	}
	// 三条 `data:` 行（中间那行 `data:{` 解不开也要计数）：本例两条。
	if read.Events != 2 {
		t.Errorf("data 行数应数到 2，实得 %d", read.Events)
	}
	// `Line` 带的是**剥掉前缀并 trim 之后**的原文（真跑时要靠它落第一段样本）。
	if read.Line != `{"code":"9999","message":"末事件"}` {
		t.Errorf("Line 应是原文那一行的净串，实得 %q", read.Line)
	}
}

// TestParsePlayAIChatSSESkipsUnparsableDataLinesAndKeepsScanning 钉"解不开就继续往前扫"。
//
// 旧 TS 的 `catch {}` 是**空**的（`:158-160`）：一条解不开的 `data:` 行不算命中。
// 本层若"命中第一条 data 行然后失败"，表现是把流的**尾随碎片**当成响应。
// 反向改坏它谁会红：把 `continue` 改成 `return`（"命中就停"）⇒ 本条红。
func TestParsePlayAIChatSSESkipsUnparsableDataLinesAndKeepsScanning(t *testing.T) {
	text := "data:{\"code\":\"0000\"}\ndata:[DONE]\ndata:not json at all\n"
	read := ParsePlayAIChatSSE(text)
	if read.Source != PlayAIChatSSEFromDataLine || read.Code != "0000" {
		t.Fatalf("应跳过两条解不开的、取第一条能解的，实得 source=%v code=%q", read.Source, read.Code)
	}
	if !read.Success {
		t.Error(`code "0000" 应判成功`)
	}
	if read.Events != 3 {
		t.Errorf("三条 data 行都该数进去，实得 %d", read.Events)
	}
}

// TestParsePlayAIChatSSEFallsBackToWholeBodyThenRawText 钉后两档，并钉住那个**后果**。
//
// 第三档（原文当一个字符串值）在旧 TS 的判据下"非空即成功"（`:48`）⇒
// 一段 HTML 报错页会被判成成功。本层**照它实现**，并用这条测试把这个反直觉读数钉成成文：
// 谁要"修"成失败，先在这里改掉断言，并给出一句有样本支撑的理由。
// 反向改坏它谁会红：给第三档加一句"不是 JSON 就判失败" ⇒ 本条第二例红。
func TestParsePlayAIChatSSEFallsBackToWholeBodyThenRawText(t *testing.T) {
	whole := ParsePlayAIChatSSE(`{"success":true,"data":{"answer":"合成"}}`)
	if whole.Source != PlayAIChatSSEFromWholeBody || !whole.Success {
		t.Errorf("无可解析 data 行时应整体 parse，实得 source=%v success=%v",
			whole.Source, whole.Success)
	}
	if whole.Events != 0 {
		t.Errorf("这一档 data 行数应为 0，实得 %d", whole.Events)
	}
	html := ParsePlayAIChatSSE("<html><body>502 Bad Gateway</body></html>")
	if html.Source != PlayAIChatSSEFromRawText {
		t.Errorf("既无 data 行又非 JSON 时应落到原文档，实得 %v", html.Source)
	}
	if !html.Success {
		t.Error("⚠ 旧 TS 的第三档是'原文非空即成功'（502 页会被判成功）：本层照抄，改动需先给理由与样本")
	}
	if html.Value != "<html><body>502 Bad Gateway</body></html>" {
		t.Errorf("第三档的 Value 应是整段原文，实得 %v", html.Value)
	}
	// 空响应：三档都到不了"非空"，判失败 —— 与"上游回了垃圾"分开。
	empty := ParsePlayAIChatSSE("   \n\n")
	if empty.Success || empty.Source != PlayAIChatSSEFromRawText || empty.Events != 0 {
		t.Errorf("空文本应是原文档且判失败，实得 %+v", empty)
	}
}

// TestPlayAIChatSSESuccessIsTheLegacyTruthTable 把 `isAiChatSuccess`（`ai-tasks.ts:46-51`）
// 的每一支摊成表。
//
// 三条最容易写错的语义各有一例：
//   - `{"code":"0"}` **不是**成功（JS 的 `===` 不做隐式转换，`"0" === 0` 为假）；
//   - `{"success":1}` **是**成功（JS 真值口径，不是 Go 的 `.(bool)`）；
//   - 顶层 `0` / `false` / `null` / `""` 全部为假（`!response` 那一支）。
//
// 反向改坏它谁会红：把 `code` 归一成数字再比（`"0"` 变 0）⇒ 第 4 例红；
// 把 `success` 改成 Go 断言 ⇒ 第 6 例红；漏掉 falsy 那一支 ⇒ 第 9、10 例红。
func TestPlayAIChatSSESuccessIsTheLegacyTruthTable(t *testing.T) {
	cases := []struct {
		name  string
		value any
		want  bool
	}{
		{"空指针", nil, false},
		{"success true", map[string]any{"success": true}, true},
		{"code 字符串 0000", map[string]any{"code": "0000"}, true},
		{"code 字符串零（严格相等应为假）", map[string]any{"code": "0"}, false},
		{"code 数字零", decode(t, `{"code":0}`), true},
		{"success 数字一（JS 真值）", decode(t, `{"success":1}`), true},
		{"success 空串", decode(t, `{"success":""}`), false},
		{"success 空对象（JS 里对象恒真）", decode(t, `{"success":{}}`), true},
		{"success 缺席且 code 非零", decode(t, `{"code":"9999"}`), false},
		{"顶层布尔真", true, false},
		{"顶层布尔假", false, false},
		{"顶层数字", decode(t, `0`), false},
		{"顶层空串", "", false},
		{"顶层非空串", "  合成回答  ", true},
		{"顶层数组", decode(t, `[1,2]`), false},
		{"空对象", map[string]any{}, false},
	}
	for _, tc := range cases {
		if got := playAIChatSSESuccess(tc.value); got != tc.want {
			t.Errorf("%s：判据应得 %v，实得 %v", tc.name, tc.want, got)
		}
	}
	// 数字那一支要确认它真的以 [json.Number] 进来（`UseNumber` 的前提）：
	// 若哪天 [core.ParseJSON] 不再开 `UseNumber`，`{"code":0}` 会以 float64 出现而落到默认支。
	if _, ok := decode(t, `{"code":0}`).(map[string]any); !ok {
		t.Fatal("夹具解不出对象")
	}
}

// TestParsePlayAIChatSSEAnswerAccumulatesEveryLineButTheLast ⭐ 钉"回答要逐行拼、末行是空的"。
//
// 形状照实拍那两条（`docs/capture/app-play-ai-samples.txt` §8）：每一片只有 1–3 码点、
// `index` 从 0 严格 +1，**末条**（`finishReason:"stop"`）的 `outContent` 是**空串**。
// 本层判据读的是末条（旧 TS 原样），所以"成功"与"有没有拿到那句话"是两回事 ——
// 这一条测试存在的意义就是把这两件事分开钉住。
//
// 反向改坏它谁会红：把累加改成"只取倒序命中的那一条"（= 末行）⇒ `Answer` 变成空串、红；
// 改成"只取第一条" ⇒ 少了两码点、红；按 `index` 重排（实拍里出现顺序与 index 同序）
// 在当前夹具里看不出来，但本仓**不**重排，成文写在 [PlayAIChatSSERead.Answer] 那一格。
func TestParsePlayAIChatSSEAnswerAccumulatesEveryLineButTheLast(t *testing.T) {
	text := "data:" + sseCameraEvent(0, "这", "processing") + "\n" +
		"data:" + sseCameraEvent(1, "是一张", "processing") + "\n" +
		"data:" + sseCameraEvent(2, "白图。", "processing") + "\n" +
		"data:" + sseCameraEvent(3, "", "stop") + "\n" +
		"\n"
	read := ParsePlayAIChatSSE(text)
	if read.Answer != "这是一张白图。" {
		t.Errorf("回答应是四条 data 行里非空分片的顺序拼接，实得 %q", read.Answer)
	}
	// 同一份读数里判据那一侧仍按旧 TS 读**末条**：它带的是信封，不是回答。
	if read.Source != PlayAIChatSSEFromDataLine || read.Events != 4 {
		t.Errorf("档位或行数不对：%+v", read)
	}
	if read.Code != "0000" || read.Message != "请求成功" {
		t.Errorf("code/message 应来自末条信封，实得 %q / %q", read.Code, read.Message)
	}
	if !read.Success {
		t.Error("末条 code=0000 ⇒ 判据应为真")
	}
	// `Line` 仍是**末条**原文（真跑时要靠它落第一段 SSE 样本），不是拼出来的回答。
	if !strings.Contains(read.Line, `"finishReason":"stop"`) {
		t.Errorf("Line 应带末条原文，实得 %q", read.Line)
	}
}

// TestParsePlayAIChatSSEAnswerTakesOnlyTheDocumentedPath 钉"只认那一条路径"。
//
// 三例各挡一种"顺手多读一格"：
//   - `data.outContent`（旧 TS 给 1028 那一族累加器读的那条路径）在 585 的两条实拍里
//     **逐行都是空** ⇒ 本层不读它，也不猜它将来会有值；
//   - `flowResult` 不是对象（null / 缺席）时不能完全不懂；
//   - 解不开的一行照旧**跳过**而不是中断累加。
//
// 反向改坏它谁会红：给 [playAIChatSSEChunkValue] 补一句"`data.outContent` 也读" ⇒ 第 2 例红。
func TestParsePlayAIChatSSEAnswerTakesOnlyTheDocumentedPath(t *testing.T) {
	text := "data:" + sseCameraEvent(0, "甲", "processing") + "\n" +
		"data:{\"code\":\"0000\",\"message\":\"请求成功\",\"data\":{\"outContent\":\"这一格本层不读\"}}\n" +
		"data:[DONE]\n" +
		"data:{\"code\":\"0000\",\"data\":{\"flowResult\":null}}\n" +
		"data:" + sseCameraEvent(1, "乙", "stop") + "\n"
	read := ParsePlayAIChatSSE(text)
	if read.Answer != "甲乙" {
		t.Errorf("只该拼 `data.flowResult.outContent`，实得 %q", read.Answer)
	}
	if read.Events != 5 {
		t.Errorf("五条 data 行都该数进去，实得 %d", read.Events)
	}
	// 末条仍可解析 ⇒ 判据档不变（这一格与 Answer 无关，钉它别被"顺手改判据"带走）。
	if read.Source != PlayAIChatSSEFromDataLine || !read.Success {
		t.Errorf("档位/判据不该受累加影响：%+v", read)
	}
}

// TestParsePlayAIChatSSEAnswerFallbacks 钉后两档在"回答"这一格上的表现。
//
// 第二档（整体 JSON、一条 `data:` 都没有）里同一格路径再读一次 —— 那不是猜上游会这么回，
// 而是"这一份已解析的对象里有没有那一格"。第三档（原文）拿不出结构 ⇒ 回答必为空，
// 调用方因此**一定**会看到"没有回答"而不会把一段 HTML 当成模型说过什么。
//
// 反向改坏它谁会红：删掉 [playAIChatSSERead] 里 `read.Answer == ""` 那一段兜底 ⇒ 第一例红。
func TestParsePlayAIChatSSEAnswerFallbacks(t *testing.T) {
	whole := ParsePlayAIChatSSE(`{"code":"0000","data":{"flowResult":{"outContent":"整体档里的那一句"}}}`)
	if whole.Source != PlayAIChatSSEFromWholeBody {
		t.Fatalf("应落到整体档，实得 %v", whole.Source)
	}
	if whole.Answer != "整体档里的那一句" {
		t.Errorf("整体档也该读同一格路径，实得 %q", whole.Answer)
	}
	if whole.Events != 0 {
		t.Errorf("这一档 data 行数为 0，实得 %d", whole.Events)
	}
	html := ParsePlayAIChatSSE("<html><body>502 Bad Gateway</body></html>")
	if html.Answer != "" {
		t.Errorf("原文档不该有回答，实得 %q", html.Answer)
	}
	if !html.Success {
		t.Error("⚠ 旧 TS 第三档'原文非空即成功'：本层照抄（改动需先给理由与样本），这里只钉它没被顺手修掉")
	}
}

// TestPlayAIChatSSESourceStringIsWhatTheRunReads 钉播报用的那一格文本。
//
// `Success` 单独念出来会掩盖"命中的是哪一档"，所以三档都要有人话文本；
// 第四例是防御：有人加了第四档而没给文案时，念出来的是"未知档"而不是空串。
func TestPlayAIChatSSESourceStringIsWhatTheRunReads(t *testing.T) {
	for _, tc := range []struct {
		source PlayAIChatSSESource
		want   string
	}{
		{PlayAIChatSSEFromDataLine, "data 行"},
		{PlayAIChatSSEFromWholeBody, "整体 JSON"},
		{PlayAIChatSSEFromRawText, "原文"},
		{PlayAIChatSSESource(99), "未知档"},
	} {
		if got := tc.source.String(); got != tc.want {
			t.Errorf("档位 %d 的文案应是 %q，实得 %q", tc.source, tc.want, got)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIChatCameraSendsAndReturnsTheReading 端到端（进程内）：发一次、读出 data 终事件。
func TestPlayAIChatCameraSendsAndReturnsTheReading(t *testing.T) {
	client := &sseScriptClient{status: 200,
		body: "data:{\"code\":\"0000\"}\ndata:{\"success\":true,\"outContent\":\"合成回答\"}\n\n"}
	up := NewUpstream(client)
	read, perr := playAIChatCameraSend(t, up, playAIChatCameraTestParams())
	if perr != nil {
		t.Fatalf("发送应成功，实得 %v", perr)
	}
	if read.Source != PlayAIChatSSEFromDataLine || !read.Success {
		t.Errorf("应读出末事件并判成功，实得 %+v", read)
	}
	if len(client.requests) != 1 {
		t.Errorf("应恰好发一条，实得 %d", len(client.requests))
	}
	if got := headerValue(client.requests[0].Headers, "authorization"); got != "Basic U1lOVEhFVElDLUJBU0lD-Q==" {
		t.Errorf("发出去的应是注入的 Basic 整串，实得 %q", got)
	}
}

// TestPlayAIChatCameraNon2xxKeepsTheBodyExcerpt 钉 #165 那一格：**非 2xx 也要把体交出来**。
//
// 坑表 #165 的记录：`internal/io` 的 `Send` 在非 2xx 时同时交回响应与错误，
// 而 [Upstream.Raw] 那条链把错误摆在前面 ⇒ 体被丢掉，"这一发到底被谁拒"就读不出来了。
// 本层的发送口因此**不走** [Upstream.Call]/[Raw]，直接打 `u.http.Send` 并按"先读体再判状态"写。
// 反向改坏它谁会红：把这里换成 `u.Raw(...)` ⇒ 错误文本里没有响应摘录，本条红
// （而真跑时那一次的差别正是"能看到 404 页写了什么"与"只看到一个 404"）。
func TestPlayAIChatCameraNon2xxKeepsTheBodyExcerpt(t *testing.T) {
	client := &sseScriptClient{status: 404, echoErr: true, sendErr: core.NewParse("合成传输错误"),
		body: "<html>no route</html>"}
	up := NewUpstream(client)
	_, perr := playAIChatCameraSend(t, up, playAIChatCameraTestParams())
	msg := playAITestErrMessage(t, "非 2xx", perr)
	if !strings.Contains(msg, "404") {
		t.Errorf("错误文本应带状态码，实得 %q", msg)
	}
	if !strings.Contains(msg, "no route") {
		t.Errorf("错误文本应带响应摘录（#165），实得 %q", msg)
	}
	// ⚠ 凭据边界：摘录只来自**响应体**，请求侧的两枚值一个字都不许出现在这里。
	for _, secret := range []string{"U1lOVEhFVElDLUJBU0lD", "TID-SYNTHETIC", "1990000000000000001"} {
		if strings.Contains(msg, secret) {
			t.Errorf("错误文本里不该出现 %q", secret)
		}
	}
}

// TestPlayAIChatCameraExcerptIsFlattenedAndTruncated 钉截断那三条规矩。
//
// 换行必须被打平（本仓事件流一行一条），超长必须截并且**报出总码点数**
// （不然"被截了"与"上游就回这么多"分不开）。反向：去掉 `strings.Map` ⇒ 第一条红。
func TestPlayAIChatCameraExcerptIsFlattenedAndTruncated(t *testing.T) {
	if got := playAIChatSSEExcerpt("第一行\n第二行\t制表"); strings.ContainsAny(got, "\n\t") {
		t.Errorf("摘录里不该留换行或制表，实得 %q", got)
	}
	long := strings.Repeat("汉", playAIChatSSEExcerptRunes+50)
	got := playAIChatSSEExcerpt(long)
	if !strings.Contains(got, "截断") || !strings.Contains(got, "共 350 码点") {
		t.Errorf("超长应截断并点名总码点，实得 %q", got)
	}
	// 按**码点**截：截成 900 字节会把中文切成半个，而那是"看起来正常"的坏输出。
	if n := strings.Count(got, "汉"); n != playAIChatSSEExcerptRunes {
		t.Errorf("摘录里应恰好 300 个汉字，实得 %d（截断按字节而非码点了？）", n)
	}
	if playAIChatSSEExcerpt("  \n ") != "（响应体为空）" {
		t.Error("空白体应写成'响应体为空'，与'没读到'分开")
	}
}

// TestPlayAIChatCameraRejectsNonUTF8Body 钉"体不是 UTF-8 时不去猜它是什么"。
//
// [core.HTTPResponse.Text] 会**显式校验**（Go 的 `string(bytes)` 不校验，
// 非法字节会变成 U+FFFD 而看起来"解析成功了"）。本层要的是那一处失败，
// 而不是拿一串替换字符走三档回退 —— 那会表现为一次真成功（第三档非空即真）。
// 反向改坏它谁会红：把 `textErr` 那一支删掉 ⇒ 本条红（且红得正是那条"看起来温和"的假成功）。
func TestPlayAIChatCameraRejectsNonUTF8Body(t *testing.T) {
	client := &sseScriptClient{status: 200, body: string([]byte{0xff, 0xfe, 0x00, 0x80})}
	up := NewUpstream(client)
	_, perr := playAIChatCameraSend(t, up, playAIChatCameraTestParams())
	msg := playAITestErrMessage(t, "非 UTF-8 体", perr)
	if !strings.Contains(msg, "UTF-8") {
		t.Errorf("应点名编码问题，实得 %q", msg)
	}
}

// TestPlayAIChatCameraTransportErrorKeepsCategoryOnly 钉"传输层错误那一次没有体可读"。
//
// 与上一条同族的另一档：`Send` 只回错误、响应是零值时，本层不许凭空造出一段摘录，
// 而是交回错误类别（[io] 侧的错误文本已不含 URL 与凭据，见 `docs/sanitize-guide.md`）。
// 反向改坏它谁会红：把 `asCoreError` 换成"顺手把 request.URL 拼进文本" ⇒ 本条不红，
// ⚠ 所以这里额外钉一条：错误文本里不许出现主机名与凭据。
func TestPlayAIChatCameraTransportErrorKeepsCategoryOnly(t *testing.T) {
	client := &sseScriptClient{sendErr: core.NewParse("合成：连接被重置")}
	up := NewUpstream(client)
	_, perr := playAIChatCameraSend(t, up, playAIChatCameraTestParams())
	msg := playAITestErrMessage(t, "传输层错误", perr)
	if !strings.Contains(msg, "连接被重置") {
		t.Errorf("应把错误类别带出来，实得 %q", msg)
	}
	for _, secret := range []string{"ai.yun.139.com", "U1lOVEhFVElD", "TID-SYNTHETIC"} {
		if strings.Contains(msg, secret) {
			t.Errorf("传输层错误的文本里不该出现 %q", secret)
		}
	}
}

// decode 把一段 JSON 摊成通用值（真值表用它构造"上游可能回的那种形态"）。
func decode(t *testing.T, text string) any {
	t.Helper()
	value, ok := core.ParseJSON(text)
	if !ok {
		t.Fatalf("夹具 JSON 解不开: %q", text)
	}
	return value
}
