package server

// 「App 任务矩阵」（`app-task`）**写侧**的接线用例。
//
// 判据在 `internal/app/task_matrix_run_test.go`（打哪个域、缺口零枪、门挡不挡、
// 复查那四格各是什么、上传三枪的顺序与三态结局）；本文件只钉这一层独有的四件事：
//
//  1. **受理并跑到终局** —— 分派表里少一行不会编译报错，界面上的表现是"点了开始
//     之后进度条停在那儿"，与"还在跑"一模一样。
//  2. **打的是自己这一族的端点** —— `taskListV2` 32 次 + `task/click` 四发 +
//     `mrp doTask` 一发 + 网盘那一族三枪各一发（`hcy/file/create` → `PUT` →
//     `hcy/file/complete`，2026-09-24 #36 接上的那一半）。另两支**同名**的
//     `task/click`（`:7071/ycloud/signin/task/click` 属 notice 链、
//     `:7071/market/task/task/click` 属分享码族）与 `taskListV3`
//     一枪都不该打：打在别的域上，本族的任务表不会动，复查永远"没认"，
//     而界面上只看得见"没认"。上传那三枪走的是**另一个基域**（`personal-kd-njs`），
//     所以这一条顺带钉住"要传文件的那条玩法真的接到了网盘那一族"——接错的表现为
//     "打卡成功、网盘一枪没打"，界面上只看得见一条"没做成"。
//  3. **一趟只用一张票**（AGENTS §1.12）：32 读 + N 写共用同一条会话。
//     这条链是全部能力里单趟请求数最多的一条，漏接那份票池的代价在这里最大。
//     ⚠ 网盘那三枪**不吃这张票**：它带的是 `Authorization` 整串 + `deviceid` 头，
//     与 jwttoken 是两套画像（成文见 `internal/upstream/app_cloud_upload.go` 文件头，
//     该族该用哪一套仍登记为待判定）。
//  4. **节奏真的走 server 那份注入点** —— [buildAppTaskRun] 把 `s.runs.waiter` 交给
//     编排层；接错（写成 `app.RealWaiter{}`）的表现不是变红而是**真睡 500ms**，
//     所以这里既数次数也认时长。
//
// ⚠ 这里的响应**全是合成夹具**。本族那一发 `task/click`（`m.mcloud.139.com`）只有**两发**
// 实拍（形状见 `internal/upstream/app_task_click.go` 文件头那张表，且**四条实拍都不带
// `result`** ⇒ 下面的成功响应也照那样写），`mrp doTask` 那一发**零实拍**，
// 网盘那三枪（文件型 `create` / `PUT` / `complete`）**同样零实拍**，
// 而"打完之后上游会不会把 `state` 改成 FINISH"一次真打都没测过 ⇒
// 本条钉的是**接线**，不是上游行为。它是这一族第一次跑到终局，
// 历史里此前没有 `app-task` 的成行（登记见 `docs/status.md`）。

import (
	"net/http"
	"strconv"
	"testing"
	"time"
)

// 这一条链自己的端点。路径写全，是为了让"打在别的域上"这件事一眼看得见。
const (
	appTaskListEndpoint = "/ycloud/signin/task/taskListV2"
	appTaskClickRoute   = "/ycloud/signin/task/click"
	appTaskEntryRoute   = "/market/signin/page/doTask"
	appTaskStatusRoute  = "/ycloud/signin/page/infoV3"

	appTaskClickBody = `{"code":0,"msg":"success"}`
	appTaskEntryBody = `{"code":0,"msg":"success"}`

	// 上传那一族的端点（`personal-kd-njs.yun.139.com`，与上面四条**不同基域**）。
	// PUT 那一枪没有固定路径（它打的是 `create` 回的那个预签名地址），所以按**主机**编路由。
	appTaskCreateRoute   = "/hcy/file/create"
	appTaskCompleteRoute = "/hcy/file/complete"
	appTaskPutHost       = "synth-oss.example.invalid"

	// ⚠ 信封与签到那族不同：`code` 是**字符串** `"0000"`，载荷在 `data`。
	appTaskCreateBody = `{"code":"0000","success":true,"data":{"exist":false,"rapidUpload":false,` +
		`"fileId":"SYNTH-FID-1","uploadId":"SYNTH-UID",` +
		`"partInfos":[{"uploadUrl":"https://synth-oss.example.invalid/put/SYNTH-TOKEN"}]}}`
	appTaskCompleteBody = `{"code":"0000","success":true}`
)

// appTaskItem 造一条任务条目（`state` 空串 ⇒ 不给那一格）。
//
// 形状与 `internal/app/task_matrix_test.go` 那一份同源 —— 两处各写一份是刻意的：
// 接线用例不该因为编排层的夹具改了而跟着红。
func appTaskItem(id int64, group, name, state string, currStep int64) string {
	stateField := ""
	if state != "" {
		stateField = `,"state":` + quoteJSON(state)
	}
	return `{"id":` + strconv.FormatInt(id, 10) +
		`,"name":` + quoteJSON(name) +
		`,"groupid":` + quoteJSON(group) +
		`,"marketname":` + quoteJSON("sign_in_3") +
		`,"enable":1,"process":0,"currstep":` + strconv.FormatInt(currStep, 10) +
		stateField + `}`
}

// appTaskBody 把条目放进**一次** `taskListV2` 响应（十六次读共用同一份 ⇒ 每组都返回它，
// 读侧按 id 去重后正好一份表）。
func appTaskBody(items ...string) string {
	joined := ""
	for i, item := range items {
		if i > 0 {
			joined += ","
		}
		joined += item
	}
	return `{"code":0,"msg":"success","result":{"new":[` + joined +
		`]},"groupState":{}}`
}

func TestAppTaskDispatchHitsItsOwnChain(t *testing.T) {
	// 四条任务，各代表一种处置：409 走专属玩法（前置打卡 + 入口上报 + 第二发打卡）、
	// 777 走 default 盲点、107 缺笔记那一族玩法 ⇒ **一枪不发**（缺口小结靠它），
	// 106 走"打卡 + 往网盘传一个文件"（#36 接上的那一半）。
	table := appTaskBody(
		appTaskItem(409, "day", "进入云朵中心", "WAIT", 0),
		appTaskItem(777, "day", "看个广告", "WAIT", 0),
		appTaskItem(107, "day", "创建一篇云笔记", "WAIT", 0),
		appTaskItem(106, "day", "上传一个文件", "WAIT", 0),
	)
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	// 409 那一发入口上报要 `deviceId`；库里没有绑定时它会先去换（本用例没给那一枪
	// 编路由 ⇒ 404 ⇒ 记一条失败），红成的样子是"doTask 应打 1 次实得 0 次"、指错方向。
	seedBoundDevice(t, h, id)

	client := newProbeScript(append(signInSessionRoutes(), []probeRoute{
		{match: appTaskListEndpoint, bodies: []string{table}},
		{match: appTaskStatusRoute, bodies: []string{signInStatusClaimable}},
		{match: appTaskClickRoute, bodies: []string{appTaskClickBody}},
		{match: appTaskEntryRoute, bodies: []string{appTaskEntryBody}},
		// 上传那一族三枪（106 那一条的第二半）。⚠ 三枪**全部零实拍**（R27）⇒ 这三份
		// 响应只是"让链路走到终局"的夹具，不是上游行为证据；`uploadUrl` 按凭据同类
		// 处理，用合成域，任何断言都比**主机**而不是整串 URL。
		{match: appTaskCreateRoute, bodies: []string{appTaskCreateBody}},
		{match: appTaskPutHost, bodies: []string{""}},
		{match: appTaskCompleteRoute, bodies: []string{appTaskCompleteBody}},
	}...)...)
	h.replaceRunWorld(t, client)
	waits := &recordingWaits{}
	h.server.runs.waiter = waits

	rec := h.do(t, "POST", "/api/runs", `{"capability":"app-task","params":{}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("app-task 应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	events := waitForRunFinished(t, h, firstRunID(t, rows))

	for _, want := range []struct {
		endpoint string
		times    int
	}{
		// 两趟 × 2 市场名 × 8 group。
		{appTaskListEndpoint, 32},
		// 409 两发（前置 + 玩法那一发 `task2`）+ 777 一发 + 106 的前置那一发；
		// 107 走缺口 ⇒ 零发。
		{appTaskClickRoute, 4},
		{appTaskEntryRoute, 1},
		// 备份门只在第一趟读一次（复查不需要它：门判的是"要不要开始做"）。
		{appTaskStatusRoute, 1},
		// ⭐ 106 那一条的第二半：三枪各一发，且**只有**它打这一族（409 / 777 / 107 都不该）。
		{appTaskCreateRoute, 1},
		{appTaskPutHost, 1},
		{appTaskCompleteRoute, 1},
	} {
		if got := client.count(want.endpoint); got != want.times {
			t.Errorf("%s 应打 %d 次，实得 %d 次（全部 URL：%v）",
				want.endpoint, want.times, got, urlsOf(client))
		}
	}
	// ⭐ 另外两支同名 `task/click` 与这一族**挨着**的那几个写口，一发都不该打。
	for _, forbidden := range []string{
		":7071", "/market/task/task/click", "taskListV3",
		"/ycloud/signin/page/doTaskPost", "/market/signin/page/receiveTask",
		"startSignIn", "/ycloud/signin/page/multiple", "receiveV3",
		"backupgift", "receiveRevivalReward", "/ycloud/signin/public/record",
		"exchangeV3", "/market/rabactive/",
	} {
		if got := client.count(forbidden); got != 0 {
			t.Errorf("app-task 不该打 %s，实得 %d 次 —— 分派把这一条接成了别的能力（URL：%v）",
				forbidden, got, urlsOf(client))
		}
	}
	if got := client.count("/ycloud/auth-service/auth/tyrzLogin"); got != 1 {
		t.Errorf("32 读 + 4 打卡 + 1 入口 + 1 门 + 3 网盘应共用一张票，建会话应 1 次，实得 %d 次（URL：%v）",
			got, urlsOf(client))
	}
	// ⭐ 节奏按条记：409 那条玩法等一拍，106 那条在"打卡成功之后"等一拍
	// （旧实现 `handleTaskClick` 末尾那句 `sleep(500)`；上传那一族自己每轮也等，
	// 但 106 的形状是"只传一个"，循环体那一拍只在 522 才有），777 是 default 盲点、不等。
	// 混成一个节奏的表现是"四发零间隔连打"，而那是真实客户端不会有的样子
	// （成文见 `task_matrix_dispatch.go` 的 `Paced` 与 `task_matrix_run.go` 的 `apply`）。
	if got := waits.count(500 * time.Millisecond); got != 2 {
		t.Errorf("该等 2 次 500ms（409 玩法 + 106 那一趟），实得 %d 次（全部：%v）",
			got, waits.all())
	}
	// 小结要能在事件流里看见 —— 两句各钉一件事：
	// ① "缺玩法的那些**明说不做**"（107）= 这一条与旧实现最大的行为差别；
	// ② "网盘新增 N 个（未清理）" = 接线之后这一趟**真的会写用户网盘**，
	//    界面上看不见这一句就等于把代价替用户藏起来了（清理那一半挂在 #37）。
	if !anyMessageContains(events, "本工具还没接上玩法") {
		t.Errorf("事件流里没有那句玩法缺口的小结：%v", messagesOf(events))
	}
	if !anyMessageContains(events, "未清理") {
		t.Errorf("事件流里没有「这一趟写了网盘、还没清理」那一句：%v", messagesOf(events))
	}
	// ⭐ **逐条**播报要能一路走到事件流（app 层那几行经 `RunProgress` → `run_events` → SSE）。
	//
	// 为什么在 server 层再钉一遍、不在 app 层测完就算：这一条链是本仓**唯一会写用户的账**
	// 的那一条，而"跑了四十多枪只有一行小结"那种状态一旦回退，界面上没人看得出来
	// —— 只有事件流里的行能说清"这一趟动了哪几条"。
	//
	// ⚠ 夹具那一份表两趟共用（state 恒 `WAIT`）⇒ 三条做过的都落"失败"那一档（做了没认），
	// 107 落"需前往"。这不是笔误，正是这一条要的读数。
	for _, want := range []string{
		"------【任务列表】------",
		// 这一枚钉的是**两个数都在**：夹具把同一份表塞进 16 次响应 ⇒ 去重前是 64 条，
		// 只报"4 条"会让人以为上游就给了四条（"去重后 4 个"那半句才是真相）。
		"获取到 64 个任务，去重后 4 个",
		"尝试完成 上传一个文件 106",
		"上传文件成功 上传一个文件 106",
		"跳过 创建一篇云笔记 107",
		"------【每日任务】------",
		"失败 上传一个文件 106",
		"需前往云盘完成 创建一篇云笔记 107",
	} {
		if !anyMessageContains(events, want) {
			t.Errorf("事件流里没有逐条播报的那一行 %q：%v", want, messagesOf(events))
		}
	}
	assertRunCapability(t, h, rows, "app-task")
}

// TestAppTaskIsAcceptedAndReachesTerminal：上游一枪都不回时也要跑到终局。
//
// 与 [TestSignInFamilyIsAcceptedAndReachesTerminal] 同一目的，单列是因为这一条不在
// 那十一行的名单里（它走专用外壳）—— 漏了这一条，"接上但分派表没登记"就没人钉。
func TestAppTaskIsAcceptedAndReachesTerminal(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs", `{"capability":"app-task","params":{}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("app-task 应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	events := waitForRunFinished(t, h, firstRunID(t, rows))
	if !anyMessageContains(events, "拒绝联网") {
		t.Errorf("事件流里没有上游失败的原因：%v", messagesOf(events))
	}
	assertRunCapability(t, h, rows, "app-task")
}
