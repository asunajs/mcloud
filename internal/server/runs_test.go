package server

// `POST /api/runs` 的用例。
//
// 这一层只拥有四件事，测试也就只钉这四件：
//
//  1. **受理先于结果**：202 必须在任务跑完之前回来（跑一条链要听满一分钟弹幕）；
//  2. **账号↔runId 的配对就是库里那一条**：前端拿它去订阅事件流，
//     配对错了的表现是"进度永远不动"，而不是任何一处报错；
//  3. **参数按能力声明翻**：未知键、类型不对、组合冲突，以及缺省取声明里的默认值；
//  4. **账号的归属与可用性**：别人的 404、停用的 400、留空 = 全部启用、重复只跑一次。
//
// ⚠ 那条链本身（开门 / 收口令 / 投 / 重听）**不在这里测** —— 判据在
// [app.LiveRoomRun]，用例在 `internal/app/live_room_task_test.go`。
// 这里刻意让**第一枪就失败**：这一层要证明的是"任务里的错误会变成事件流里的一条"，
// 把六步链在本包再演一遍就是抄第二份同源副本（而副本会漂移）。

import (
	"context"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// newTestRunWorld 装配执行面的测试版本。
//
// 三样外部世界全按"不许出门"配：HTTP 走调用方给的（默认 [refusingClient]）、
// WS 走 [refusingDialer]、设备指纹落在**临时目录**（不许写进仓库的 `data/`，
// 那是别人正在用的库旁边）。
func newTestRunWorld(t *testing.T, client core.HTTPClient) *RunWorld {
	t.Helper()
	world := newRunWorld(client, refusingDialer{}, newRandom(), t.TempDir()+"/server-device-app.txt")
	// 测试里**不真睡**：AI 头像那条链的节奏是"提交后 8 次 × 3 秒查询"，
	// 真等会让一条接线用例花掉两分钟（口径见 [RunWorld.waiter] 那条注释）。
	world.waiter = app.NoWait{}
	return world
}

// replaceRunWorld 换掉执行面（换成带脚本客户端的那一份）。
//
// 与 [harness.replaceLoginHub] 分开，理由同那边那条注释：合并成"换掉出站客户端"
// 会让一次误挂到执行面上的用例安静地消费掉本来给登录准备的脚本响应。
func (h *harness) replaceRunWorld(t *testing.T, client core.HTTPClient) {
	t.Helper()
	h.server.runs = newTestRunWorld(t, client)
}

// useDistinctRunIDs 把服务端的随机源换成**逐次进一**的那一份。
//
// ⚠⚠ 一条要跑**多个账号**的用例必须换：[fixedRandom] 的状态只有一个字节，
// 而 `x → 31x+7 (mod 256)` 的环长整除 32 —— 一次 UUID 要 16 字节，于是**连着两次**
// [core.NewRunID] 会给出**同一个** id（`server_test.go` 那条"第二次登录拿到同一个
// token"的补记是同一件事的另一面）。
// 不换的话"两个账号两个 runId"这条根本测不了 —— 而它是预分配那条路的**全部意义**。
type useDistinctRunIDs struct{ n byte }

func (r *useDistinctRunIDs) FillBytes(dst []byte) {
	for i := range dst {
		r.n++
		dst[i] = r.n
	}
}

// distinctRunIDs 装上那份随机源并返回它（调用方可能要自己再取一个 id 当占位）。
func distinctRunIDs(h *harness) *useDistinctRunIDs {
	rng := &useDistinctRunIDs{}
	h.server.rng = rng
	return rng
}

// refusingDialer 对任何拨号都直接报错。
//
// 它必须是**默认**拨号器：收口令那一步开的是 WS，不是 HTTP，
// 顶掉了 [Deps.HTTP] 却漏了它，表现就是"测试里真连了一次上游的 SockJS 端点"，
// 而连不上的形态是**卡到监听超时**（默认 60 秒），不是变红。
type refusingDialer struct{}

var _ core.WebSocketDialer = refusingDialer{}

func (refusingDialer) Dial(context.Context, string, []core.HeaderPair) (core.WebSocketConn, error) {
	return nil, core.NewInternal("测试拨号器拒绝开 WS：这条用例不该收弹幕")
}

// blockingClient 把**第一枪**卡住，直到测试放开。
//
// 它是"202 必须先于结果"那条判据的载体：不放行时任务永远跑不完，
// 于是任何"等任务结束才回响应"的写法都会让用例**挂住**（而不是变绿）。
// 放开之后它给的是错误 —— 这一层不需要链跑成功，只需要它**结束**。
type blockingClient struct {
	mu      sync.Mutex
	entered chan struct{}
	release chan struct{}
	once    sync.Once
	calls   int
}

func newBlockingClient() *blockingClient {
	return &blockingClient{
		entered: make(chan struct{}),
		release: make(chan struct{}),
	}
}

var _ core.HTTPClient = (*blockingClient)(nil)

func (c *blockingClient) Send(context.Context, core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	c.calls++
	first := c.calls == 1
	c.mu.Unlock()
	if first {
		c.once.Do(func() { close(c.entered) })
		<-c.release
	}
	return core.HTTPResponse{}, core.NewInternal("夹具：这一枪本来就要失败")
}

// liveRoomBody 拼一份请求体（`params` 由调用方给裸 JSON）。
func liveRoomBody(params string) string {
	return `{"capability":"live-room","params":` + params + `}`
}

// codesBody 是"手工投一个口令"那一份：它**不**碰弹幕，所以默认的
// [refusingDialer] 不会被用到，而第一枪就失败 ⇒ 一次也不会拨到 WS。
func codesBody(codes string) string {
	return liveRoomBody(`{"codes":` + quoteJSON(codes) + `,"skipKouling":true}`)
}

// runsPairing 解 202 里那一份配对。
func runsPairing(t *testing.T, rec *httptest.ResponseRecorder) []map[string]any {
	t.Helper()
	body := decode(t, rec)
	list, ok := body["runs"].([]any)
	if !ok {
		t.Fatalf("响应里没有 runs 数组，实得 %v", body)
	}
	rows := make([]map[string]any, 0, len(list))
	for _, item := range list {
		row, ok := item.(map[string]any)
		if !ok {
			t.Fatalf("runs 里有一项不是对象：%v", item)
		}
		rows = append(rows, row)
	}
	return rows
}

// runIDsOf 取配对里的 runId（顺序就是响应里的顺序）。
func runIDsOf(rows []map[string]any) []core.RunID {
	out := make([]core.RunID, 0, len(rows))
	for _, row := range rows {
		raw, _ := row["runId"].(string)
		id, _ := core.ParseRunID(raw)
		out = append(out, id)
	}
	return out
}

// waitForRunFinished 等那条 run 走完（事件流里出现终局事件）。
//
// 轮询而不是同步等：起跑是在一个 goroutine 里，测试**不该**知道它跑到哪一步 ——
// 这里等的只是"它已经结束"，而那是那一条流自己的结论。
//
// ⚠ "查不到这条 run"要**继续等**，不是立刻失败：`runs` 那一行是那个 goroutine 里
// 写的（[app.Runner.runOne] 的 `InsertRun`），所以 202 之后天然有一小段"配对已经给出去、
// 库里还没有这一行"的窗口。把它当失败的话，本包那批用例就变成**看调度心情**的 flaky
// （实测同一份代码三次跑出三组不同的红）。
// 真·不存在的那一种（runId 打错）由超时那句话负责，它至少会说"已收 0 条"。
//
// ⚠ scope 在**进圈之前**取一次：`userIDOf` 是打一次 `/api/session` 拿的，
// 放在循环里会让"等不到事件"表现为几百次 HTTP 调用，而那句超时
// 本来是该指向"任务没跑完"的。
func waitForRunFinished(t *testing.T, h *harness, runID core.RunID) []core.Sequenced {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	deadline := time.Now().Add(10 * time.Second)
	var events []core.Sequenced
	for {
		found, err := h.store.RunEventsAfter(ctx, scope, runID, nil, 500)
		if err == nil {
			events = found
			for _, item := range events {
				if item.Event.IsTerminal() {
					return events
				}
			}
		}
		if time.Now().After(deadline) {
			t.Fatalf("等不到终局事件（已收 %d 条，最后一次读数 err=%v）", len(events), err)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// storedRunIDs 列出当前登录者名下的所有 run id。
func storedRunIDs(t *testing.T, h *harness) []string {
	t.Helper()
	runs, err := h.store.ListRuns(context.Background(),
		core.ScopeUser(userIDOf(t, h)), 200)
	if err != nil {
		t.Fatalf("列 run 失败: %v", err)
	}
	out := make([]string, 0, len(runs))
	for _, run := range runs {
		out = append(out, run.ID)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 闸门
// ─────────────────────────────────────────────────────────────────────────────

// TestRunTriggerRefusesAnonymousRequests：不登录不能借这台服务跑任务。
//
// 少了这道闸，任何扫到端口的人都能拿**库里所有人的凭据**去打上游 ——
// 而且是以"跑了 N 趟"的形态出现，事后连是谁干的都查不到。
// 断言里带"什么都没落库"：只判状态码测不出"处理器先干活再回 401"。
func TestRunTriggerRefusesAnonymousRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	// 这里用 [seedAccount] 就够了：请求在**进门**时被拒，走不到读登录态那一步。
	seedAccount(t, h, maskedID)

	admin := h.cookie
	h.cookie = nil
	rec := h.do(t, "POST", "/api/runs", codesBody("甲"), nil)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("未登录应 401，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	h.cookie = admin
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("未登录的请求留下了运行记录：%v", ids)
	}
}

// TestRunTriggerIsRejectingBeforeSetup：没初始化时这条也在"只暴露初始化"里面。
//
// 它是 [Server.requireSetup] 那一条对新路由的覆盖 —— 中间件是整片生效的，
// 但"新加一条路由时忘了想它在哪道闸后面"正是这类测试唯一能拦住的东西。
func TestRunTriggerIsRejectedBeforeSetup(t *testing.T) {
	h := newHarness(t)
	rec := h.do(t, "POST", "/api/runs", codesBody("甲"), nil)
	if rec.Code != http.StatusServiceUnavailable {
		t.Errorf("未初始化应 503 setup_required，实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 能力：不存在 / 存在但没接上执行
// ─────────────────────────────────────────────────────────────────────────────

func TestUnknownCapabilityIsNotFound(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, maskedID)

	rec := h.do(t, "POST", "/api/runs",
		`{"capability":"no-such-thing","params":{}}`, nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("不存在的能力应 404，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}

// TestBlankCapabilityIsABadRequestNotAMissingResource：空 code 是**请求**写错了。
//
// 与"没有这个能力"分开报是对的：前者前端该挡住（表单没填完），
// 后者才该显示"这台服务不认识这个任务"。
func TestBlankCapabilityIsABadRequestNotAMissingResource(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "POST", "/api/runs", `{"capability":"  ","params":{}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Errorf("空 capability 应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}

// TestRegisteredButUnwiredCapabilityIsNotImplemented：注册表里有、执行面上没有，
// 必须回 **501**，不能回 202。
//
// 回 202 的后果不是"错一个码"：前端会拿着那对 runId 去订阅事件流，
// 而那条流永远不会有任何事件 —— 界面上的表现是"进度条停在那儿"，
// 与"任务还在跑"一模一样。这一条把"受理"与"会做"钉死成同一件事。
//
// ⚠ **2026-09-23 起这一支在真实配置下已经不可达**：注册表十七条全部接上了执行
// （见 [TestEveryRegisteredCapabilityIsWired]）。所以这一条改成"临时摘掉一条再打"，
// 逐条摘一遍。为什么不只留一条：那样测的是"我摘掉的那一条会 501"，
// 而这一条的意义是**任何**一条没接上时都不会假装受理 —— 十七条共用同一个分支，
// 逐条走一遍的成本就是十六个 501（一个请求都不往上游发，因为 builder 查不到就返回了）。
//
// ⚠ 这里动的是包级变量 [runBuilders]。本包没有 `t.Parallel()`，而那十七条摘掉之后
// 走不到任何 goroutine（分派表查不到就直接回 501），所以不存在与后台读写同一张 map
// 的窗口。将来若给本包加并行，这一条必须改成注入表而不是改全局。
func TestRegisteredButUnwiredCapabilityIsNotImplemented(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	for _, capability := range capabilities.All() {
		code := capability.Code
		builder, wired := runBuilders[code]
		if !wired {
			t.Errorf("%s 本来就没接上 —— 这条用例假设十七条全在表里，请一并回看 %s",
				code, "TestEveryRegisteredCapabilityIsWired")
			continue
		}
		delete(runBuilders, code)
		rec := h.do(t, "POST", "/api/runs",
			`{"capability":`+quoteJSON(code)+`,"params":{}}`, nil)
		runBuilders[code] = builder // 立刻放回：这张表是**进程级共享**的
		if rec.Code != http.StatusNotImplemented {
			t.Errorf("%s 摘掉 builder 后应 501，实得 %d：%s",
				code, rec.Code, brief(rec.Body.String()))
			continue
		}
		body := decode(t, rec)
		if body["code"] != "not_implemented" {
			t.Errorf("%s 的错误码应是 not_implemented，实得 %v", code, body["code"])
		}
		// 5xx 里这一条**刻意**给原文（见 [apiNotImplemented]），但只给我们自己写的话。
		if message, _ := body["message"].(string); !strings.Contains(message, "还没有接上执行") {
			t.Errorf("%s 的 501 该说清是哪个能力没接上，实得 %q", code, message)
		}
		// 放回之后必须重新认得这一条：漏了恢复会让后面每一条都"看起来 501 是对的"。
		if _, back := runBuilders[code]; !back {
			t.Fatalf("%s 没放回分派表 —— 后面的用例全在测一份残缺的表", code)
		}
	}
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("没受理的请求不该留下运行记录：%v", ids)
	}
	// 收尾自检：这张表恢复原样了（十七条），否则污染的是整个测试二进制。
	if got := len(runBuilders); got != len(capabilities.All()) {
		t.Errorf("分派表被改坏了：现有 %d 条，注册表 %d 条", got, len(capabilities.All()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 参数：按能力声明翻
// ─────────────────────────────────────────────────────────────────────────────

// TestUnknownParameterKeyIsRefused：拼错的键**不能**被静默丢掉。
//
// 判据的键名权威只有能力声明那一处；`skipKouling` 写成 `skip_kouling` 时，
// 宽容的 JSON 解码会让那一趟照常跑，只是把用户明确关掉了的那一步又做一遍。
func TestUnknownParameterKeyIsRefused(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs",
		`{"capability":"live-room","params":{"codes":"甲","skip_kouling":true}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("未知参数键应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	message, _ := decode(t, rec)["message"].(string)
	if !strings.Contains(message, "skip_kouling") {
		t.Errorf("400 该点出是哪一栏拼错了，实得 %q", message)
	}
	if !strings.Contains(message, "skipKouling") {
		t.Errorf("400 该把能填的键一并列出来（否则人只能去翻代码），实得 %q", message)
	}
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("被拒的请求留下了运行记录：%v", ids)
	}
}

// TestWrongParameterTypeSaysWhichField：类型不对要说**哪一栏**。
//
// ⚠ 断言里同时要求"不带用户给的内容"：那一栏可能是一大段粘贴进来的东西，
// 而错误体会进浏览器历史与代理日志（与账号写面同一条纪律）。
func TestWrongParameterTypeSaysWhichField(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, maskedID)

	cases := []struct{ name, params, want string }{
		{"口令给成数组", `{"codes":["甲"]}`, "口令"},
		{"听多久给成字符串", `{"listenSeconds":"60"}`, "听弹幕时长"},
		{"只听不投给成文本", `{"koulingOnly":"yes"}`, "只听不投"},
		{"账号给成逗号文本", `{"accounts":"19****00,19****02"}`, "账号"},
	}
	for _, c := range cases {
		rec := h.do(t, "POST", "/api/runs", liveRoomBody(c.params), nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("%s 应 400，实得 %d：%s", c.name, rec.Code, brief(rec.Body.String()))
			continue
		}
		message, _ := decode(t, rec)["message"].(string)
		if !strings.Contains(message, c.want) {
			t.Errorf("%s 的 400 该指出是哪一栏，实得 %q", c.name, message)
		}
		if strings.Contains(message, "19****00,19****02") || strings.Contains(message, `"yes"`) {
			t.Errorf("%s 的 400 把用户给的内容回吐了：%q", c.name, message)
		}
	}
}

// TestParameterConflictsGetTheirOwnWords：三条组合冲突的**判据**在领域层
// （[app.LiveRoomParams.Check]，与 CLI 共用），这里只钉措辞与状态码。
//
// 三条各自单独判：合并成一句"参数组合不合法"等于把用户送去读源码。
func TestParameterConflictsGetTheirOwnWords(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, maskedID)

	cases := []struct{ name, params, want string }{
		{
			name:   "两个开关同时开",
			params: `{"codes":"甲","koulingOnly":true,"skipKouling":true}`,
			want:   "互斥",
		},
		{
			name:   "只用给定口令却没给口令",
			params: `{"skipKouling":true}`,
			want:   "至少填一个口令",
		},
		{
			name:   "听 0 秒",
			params: `{"listenSeconds":0}`,
			want:   "必须为正",
		},
	}
	for _, c := range cases {
		rec := h.do(t, "POST", "/api/runs", liveRoomBody(c.params), nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("%s 应 400，实得 %d：%s", c.name, rec.Code, brief(rec.Body.String()))
			continue
		}
		message, _ := decode(t, rec)["message"].(string)
		if !strings.Contains(message, c.want) {
			t.Errorf("%s 的 400 该说清冲突在哪，期望含 %q，实得 %q", c.name, c.want, message)
		}
	}
}

// TestListenSecondsDefaultComesFromTheDeclaration：省略那一栏 = 用声明里的默认值。
//
// ⭐ 这一条同时钉住**两个**方向：缺省必须是**正数**（否则"省略"会变成一个 400），
// 而且它得来自声明 —— 服务端自己另记一份数字的话，界面上显示 60、
// 后端用别的数跑完，而两者都不会报错。
//
// 判据是"没报错 + 受理了"：真正的 60 在事件流里看不见（那是链内部的事，
// 由 app 层那批用例钉），所以这里不假装能断到它。
func TestListenSecondsDefaultComesFromTheDeclaration(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true}`), nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("省略听多久应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	waitRuns(t, h, runsPairing(t, rec))
}

// TestParallelAccountModeIsRefusedNotIgnored：界面上那个开关后端做不到就得说做不到。
//
// 静默按串行跑的后果不是错，是**慢**：用户以为并行完了才去查历史，
// 而它其实还在排第一个账号。
func TestParallelAccountModeIsRefusedNotIgnored(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true,"accountMode":"parallel"}`), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("并行应 400（本服务只有串行），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("被拒的并行请求留下了运行记录：%v", ids)
	}
	// 显式串行与"根本没给这一栏"必须走同一条路（默认值取声明里的 `"sequential"`）。
	// 两趟的参数**不一样**（第二趟没给 `accountMode`），但互斥键算出来是同一个，
	// 所以它们必须**依次**跑完 —— 不等第一条就重跑第二条会慢，但不会错；
	// 反过来若两趟并行，"同一账号同一批参数跑两遍"就漏出去了。
	for _, params := range []string{
		`{"codes":"甲","skipKouling":true,"accountMode":"sequential"}`,
		`{"codes":"甲","skipKouling":true}`,
	} {
		rec := h.do(t, "POST", "/api/runs", liveRoomBody(params), nil)
		if rec.Code != http.StatusAccepted {
			t.Errorf("%s 应受理，实得 %d：%s", params, rec.Code, brief(rec.Body.String()))
			continue
		}
		waitRuns(t, h, runsPairing(t, rec))
	}
}

// TestRequestBodyMustBeAnObjectShapeWeUnderstand：`params` 给成别的东西不该 500。
func TestMalformedRequestBodyShapeIsABadRequest(t *testing.T) {
	h := newHarness(t).initialized(t)

	cases := []struct{ name, body string }{
		{"不是 JSON", `{"capability":`},
		{"params 是字符串", `{"capability":"live-room","params":"codes=甲"}`},
		{"params 是数组", `{"capability":"live-room","params":["codes"]}`},
		{"整个体是数组", `[{"capability":"live-room"}]`},
	}
	for _, c := range cases {
		rec := h.do(t, "POST", "/api/runs", c.body, nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("%s 应 400，实得 %d：%s", c.name, rec.Code, brief(rec.Body.String()))
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号
// ─────────────────────────────────────────────────────────────────────────────

// TestEmptyAccountListMeansEveryEnabledAccount：留空 = 全部**启用中**的账号。
//
// ⚠ 停用的那个必须不进来：这一趟是**写操作**（投口令会动这个账号的兑换状态），
// 而"停用的账号不碰"是本项目对写操作的一条统一纪律（CLI 的 `--account 0` 同口径）。
func TestEmptyAccountListMeansEveryEnabledAccount(t *testing.T) {
	h := newHarness(t).initialized(t)
	enabled := []string{"19900000002", "19900000003"}
	for _, phone := range enabled {
		seedLoginAccount(t, h, phone)
	}
	// 停用的那个与"别人名下的那一个"留空时都**不该**被算进来
	//（后者由存储层那一道 scope 过滤保证）。
	seedDisabledAccount(t, h, "19****04")
	seedForeignAccount(t, h, "19****05")

	rec := h.do(t, "POST", "/api/runs", liveRoomBody(`{"codes":"甲","skipKouling":true}`), nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != len(enabled) {
		t.Fatalf("应给出 %d 个账号的配对，实得 %d：%v", len(enabled), len(rows), rows)
	}
	waitRuns(t, h, rows)
	got := make([]string, 0, len(rows))
	for _, row := range rows {
		got = append(got, asText(t, row["accountId"]))
	}
	for _, want := range []string{"19****02", "19****03"} {
		if !slices.Contains(got, want) {
			t.Errorf("缺了启用中的账号 %s，实得 %v", want, got)
		}
	}
	for _, unwanted := range []string{"19****04", "19****05"} {
		if slices.Contains(got, unwanted) {
			t.Errorf("账号 %s 不该出现在这一趟里（停用或不属于本用户）：%v", unwanted, got)
		}
	}
}

// TestSomeoneElsesAccountIsNotFoundOnTheTriggerPath：别人的账号 → 404，不是 403。
//
// 与账号写面、SSE 那两条同一口径（403 等于确认"这台服务上有这么个人"，
// 而账号 id 是掩码手机号，猜中它的成本极低）。
func TestSomeoneElsesAccountIsNotFoundOnTheTriggerPath(t *testing.T) {
	h := newHarness(t).initialized(t)
	foreign := seedForeignAccount(t, h, "19****06")

	rec := h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true,"accounts":["`+foreign+`"]}`), nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("别人的账号应 404（403 会泄露 id 存在），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("探测别人的账号却留下了运行记录：%v", ids)
	}
}

// TestDisabledAccountIsRefusedWithAnActionableWord：显式勾选了停用的账号 → 400。
//
// ⚠ 这里**不**用 404：那一栏是用户自己勾出来的，404 会让人去查
// "账号是不是被删了"，而这里要做的是去账号页把它启用。
func TestDisabledAccountIsRefusedWithAnActionableWord(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedDisabledAccount(t, h, "19****07")

	rec := h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true,"accounts":["`+id+`"]}`), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("停用的账号应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	message, _ := decode(t, rec)["message"].(string)
	if !strings.Contains(message, "停用") {
		t.Errorf("400 该说清是停用、并指出下一步，实得 %q", message)
	}
}

// TestTriggerWithNoUsableAccountIsRefused：一个账号都没有时别回 202。
//
// 202 + 空数组的表现是界面上"开始了"却什么都没有；400 才是可行动的那一句。
func TestTriggerWithNoUsableAccountIsRefused(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := h.do(t, "POST", "/api/runs", liveRoomBody(`{"codes":"甲","skipKouling":true}`), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("没有可用账号应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	// 显式给了空数组与"根本没给这一栏"是同一件事：都落到"全部启用账号"这条缺省上。
	rec = h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true,"accounts":[]}`), nil)
	if rec.Code != http.StatusBadRequest {
		t.Errorf("显式空数组应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}

// TestRepeatedAccountIDsRunOnce：重复的 id 只跑一次。
//
// ⭐ 这条不是洁癖：编排是**串行**的，同一账号跑两遍不会被自己的互斥键挡住
// （第一趟跑完才轮到第二趟），于是"选中的账号"会变成"同一批口令投两次"——
// 而口令是不可逆的。
func TestRepeatedAccountIDsRunOnce(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, maskedID)

	rec := h.do(t, "POST", "/api/runs",
		liveRoomBody(`{"codes":"甲","skipKouling":true,"accounts":["`+maskedID+`","`+maskedID+`"]}`), nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 1 {
		t.Errorf("重复的账号 id 应只留一个，实得 %d 项：%v", len(rows), rows)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 受理与事件流
// ─────────────────────────────────────────────────────────────────────────────

// TestTriggerAnswersBeforeTheRunFinishes：⭐ 本接口的**核心**判据。
//
// 三条一起成立才算通过：
//  1. 上游那一枪**还卡着**的时候 202 就回来了（写成同步等待的用例会挂在这里）；
//  2. 响应给出的 runId 就是**库里那一条**（前端拿它订阅事件流，配错了进度永远不动）；
//  3. 放开之后事件流里出现终局事件，且 SSE 那条路读得到。
//
// 链本身故意让它**失败**（第一枪就报错）：这一层要钉的是"错误怎么变成事件、
// 怎么落进历史"，而那条链的正确性由 app 层那批用例负责。
func TestTriggerAnswersBeforeTheRunFinishes(t *testing.T) {
	h := newHarness(t).initialized(t)
	// ⚠ 必须是**带登录态**的账号：`RunAccount` 在读到凭据之前不会出网，
	// 用无凭据的 [seedAccount] 时下面那个"等第一枪"的 `<-client.entered` 永远等不到。
	seedLoginAccount(t, h, fullPhone)
	client := newBlockingClient()
	h.replaceRunWorld(t, client)

	done := make(chan *httptest.ResponseRecorder, 1)
	go func() { done <- h.do(t, "POST", "/api/runs", codesBody("甲"), nil) }()

	select {
	case <-client.entered:
		// 任务确实开跑了 —— 此刻响应**必须**已经在路上。
	case <-time.After(5 * time.Second):
		t.Fatal("夹具没等到任务打第一枪")
	}

	var rec *httptest.ResponseRecorder
	select {
	case rec = <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("202 没在任务结束前回来：这条接口被写成了同步等待")
	}
	if rec.Code != http.StatusAccepted {
		t.Fatalf("应 202，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 1 {
		t.Fatalf("一个账号应给一项配对，实得 %v", rows)
	}
	runID, ok := core.ParseRunID(asText(t, rows[0]["runId"]))
	if !ok {
		t.Fatalf("响应里的 runId 不是合法形状：%v", rows[0]["runId"])
	}
	if asText(t, rows[0]["accountId"]) != maskedID {
		t.Errorf("配对里的账号错了，实得 %v", rows[0]["accountId"])
	}
	// 202 的响应体里不许有凭据（它与 `runs` 列表共用同一套展示口径）。
	assertNoSecrets(t, "受理响应", rec.Body.String())

	close(client.release)
	events := waitForRunFinished(t, h, runID)

	// ⚠ 这三条断的是**这一层**拥有的事：受理时给的那个 id 走完了整条流。
	kinds := make([]string, 0, len(events))
	for _, item := range events {
		kinds = append(kinds, item.Event.Kind.String())
	}
	joined := strings.Join(kinds, ",")
	if !strings.HasPrefix(joined, "started,") {
		t.Errorf("事件流应以 started 开头，实得 %v", kinds)
	}
	if !strings.Contains(joined, "errored") || !strings.Contains(joined, "finished") {
		t.Errorf("失败的运行既要有 errored 也要有 finished（前端靠它收流），实得 %v", kinds)
	}

	// 那条 SSE 读得到同一批事件（订阅的是 runId，不是"最新一趟"）。
	stream := h.do(t, "GET", "/api/runs/"+runID.String()+"/events?after=0", "", nil)
	if stream.Code != http.StatusOK {
		t.Fatalf("SSE 应 200，实得 %d：%s", stream.Code, brief(stream.Body.String()))
	}
	if !strings.Contains(stream.Body.String(), `"type":"finished"`) {
		t.Errorf("SSE 里没有终局事件：%s", brief(stream.Body.String()))
	}
}

// TestPreallocatedRunIDsForEveryAccountAreAllUsed：多账号时**每个** id 都会成为一条记录。
//
// ⭐ 这条钉的是预分配那条路最坏的一种坏法：只跑第一个账号时"响应里给了两个 id、
// 库里只有一条"完全看不出来 —— 第二个的订阅方会拿到 404，而界面上那句是
// "这一趟没有执行"，与"被锁挡住"长得一模一样。
func TestPreallocatedRunIDsForEveryAccountAreAllUsed(t *testing.T) {
	h := newHarness(t).initialized(t)
	distinctRunIDs(h)
	seedLoginAccount(t, h, "19900000002")
	seedLoginAccount(t, h, "19900000003")
	// 两个账号的互斥键**不同**（键里有账号），所以它们会**依次**各打上游一枪；
	// 默认执行面那份 [refusingClient] 每枪都立刻失败，正好让两条都跑到终局。

	rec := h.do(t, "POST", "/api/runs", liveRoomBody(`{"codes":"甲","skipKouling":true}`), nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("应 202，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 2 {
		t.Fatalf("两个账号应给两项配对，实得 %v", rows)
	}
	seen := map[string]struct{}{}
	for _, row := range rows {
		id, ok := core.ParseRunID(asText(t, row["runId"]))
		if !ok {
			t.Fatalf("响应里的 runId 不是合法形状：%v", row["runId"])
		}
		if _, dup := seen[id.String()]; dup {
			t.Fatalf("两个账号拿到了同一个 runId：%v", rows)
		}
		seen[id.String()] = struct{}{}
	}
	waitRuns(t, h, rows)

	stored := storedRunIDs(t, h)
	if len(stored) != 2 {
		t.Errorf("两项配对应对应两条记录，实得 %v", stored)
	}
	for _, row := range rows {
		if !slices.Contains(stored, asText(t, row["runId"])) {
			t.Errorf("配对里的 %v 在库里没有对应记录：%v", row["runId"], stored)
		}
	}
}

// TestTheSecondTriggerOfTheSameAccountIsNotRecorded：
// 同一账号的第二趟在**跑的那一刻**被挡下，于是它的 runId 没有记录。
//
// ⭐ 这条是文档里那三种"订阅拿到 404"的窗口之一（见本文件开头）。它必须**测**：
// 界面上那句"这一趟没有执行"能不能出现，取决于被挡下的账号到底会不会写一行。
//
// # 为什么不靠"先起一趟真的运行"来造出被占住的锁
//
// 那样要么赌时序（第二趟的 goroutine 可能等到锁放开才走到准入，于是它**真的跑了一遍**
// —— 同一批口令投两次，而这条用例本来要测的就是"不会"），
// 要么在测试结束时留一个还卡在上游的 goroutine（库连接已由 `t.Cleanup` 关掉，
// 表现是 `panic: database is closed`，实测踩过）。
// 所以这里**直接在跨请求那份登记表上占住这把锁**：准入判据看到的是同一把键，
// 而这一趟的 goroutine 会在准入那一步就返回，不写库、不出网。
func TestTheSecondTriggerOfTheSameAccountIsNotRecorded(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedLoginAccount(t, h, fullPhone)

	params := app.LiveRoomParams{Codes: []string{"甲"}, ListenSeconds: 60, SkipKouling: true}
	holder := core.NewRunID(newRandom())
	admission := h.server.runs.registry.Admit(holder,
		mcloudexec.NewMutexKey(capabilities.CodeLiveRoom, id, liveRoomDigest(params)))
	if admission.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("夹具没能占住这把锁：%v", admission.Kind)
	}
	t.Cleanup(func() { h.server.runs.registry.Release(holder) })

	rec := h.do(t, "POST", "/api/runs", codesBody("甲"), nil)
	// ⚠ 这一趟**仍然**是 202：准入发生在跑的那一刻，202 时刻无从预知（见 runs.go 开头）。
	if rec.Code != http.StatusAccepted {
		t.Fatalf("被占住时也应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	blockedID, ok := core.ParseRunID(asText(t, rows[0]["runId"]))
	if !ok {
		t.Fatalf("配对的 runId 不合法：%v", rows[0])
	}

	// 被挡下的那一趟**没有**运行记录，所以订阅它的 runId 是 404。
	//
	// ⚠ 这里等一小段再判，而不是"打完就查"：起跑在 goroutine 里，
	// 判得太早等于什么都没判。它走到准入就返回（不写库、不出网），
	// 所以这段等待是**上界**而不是"再赌一次时序" —— 记录要么在准入之前写，
	// 而准入之前什么都在没发生。
	deadline := time.Now().Add(time.Second)
	for {
		if ids := storedRunIDs(t, h); slices.Contains(ids, blockedID.String()) {
			t.Fatalf("被挡下的那一趟写下了运行记录：%v —— 界面上『这一趟没有执行』那句就再也不会出现", ids)
		}
		if time.Now().After(deadline) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	stream := h.do(t, "GET", "/api/runs/"+blockedID.String()+"/events", "", nil)
	if stream.Code != http.StatusNotFound {
		t.Errorf("被挡下的那一趟订阅时应 404，实得 %d：%s",
			stream.Code, brief(stream.Body.String()))
	}
}

// TestRunWorldIsSharedAcrossRequests：执行面是**进程级**那一份。
//
// 与上一条是一对：那条测"挡下"，这条测"为什么挡得住" ——
// 处理器每次现拼一份 [RunWorld] 的话，登记表就退回成"每次请求一份"，
// 上一条会红，而这一条会**依然绿**（它测的是装配本身，不是行为）。
// 所以这条断的是身份而不是行为：两次请求拿到的是同一个指针。
func TestRunWorldIsSharedAcrossRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)
	before := h.server.runs

	for _, code := range []string{"甲", "乙"} {
		rec := h.do(t, "POST", "/api/runs", codesBody(code), nil)
		if rec.Code != http.StatusAccepted {
			t.Fatalf("%s 应受理，实得 %d：%s", code, rec.Code, brief(rec.Body.String()))
		}
		waitRuns(t, h, runsPairing(t, rec))
	}
	if h.server.runs != before {
		t.Error("执行面被换掉了：每请求一份的话，同一账号的互斥登记就一句都不成立")
	}
}

// TestServerWithoutRunWorldSaysSoInsteadOfAccepting：装配漏了执行面时不许回 202。
//
// 这一条的存在理由：`runs` 是 [New] 的必填参数，但"必填"挡不住传 nil，
// 而传了 nil 之后如果不判，表现是**空指针 panic**（整个进程跟着 handler 崩）；
// 如果判了却回 202，表现是界面永远等不到事件。
func TestServerWithoutRunWorldSaysSoInsteadOfAccepting(t *testing.T) {
	store, hub := newStoreWithHub(t, refusingClient{})
	srv := New(store, newClock(), newRandom(), Config{}, hub,
		newTestRefresher(t, store, refusingClient{}), nil)
	h := &harness{server: srv, store: store, rng: newRandom()}
	h.initialized(t)

	rec := h.do(t, "POST", "/api/runs", codesBody("甲"), nil)
	if rec.Code != http.StatusInternalServerError {
		t.Errorf("未装配执行面应 500，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if strings.Contains(rec.Body.String(), "runId") {
		t.Errorf("500 的响应里出现了 runId（假装受理了）：%s", brief(rec.Body.String()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具（账号与脚本）
// ─────────────────────────────────────────────────────────────────────────────

// seedLoginAccount 塞一个**带登录态**的账号，返回界面上那一行的 id（掩码手机号）。
//
// ⚠ 本包这条链的用例**必须**用它而不是 [seedAccount]：`RunAccount` 在打上游之前
// 先要 `CredentialFromAccount`，没有登录态时它在那里就返回错误了 ——
// 于是"出站客户端"根本没被走到，夹具里那个"卡住第一枪"的客户端**永远等不到**，
// 用例的表现是超时而不是变红。
//
// 手机号取 `199` 合成号段（与 `login_test.go` 的 [fullPhone] 同一族）：
// 成文的地方不许出现真实号码。
func seedLoginAccount(t *testing.T, h *harness, phone string) string {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	id := core.AccountIDFor(phone, nil)

	account := core.Account{ID: id, Enabled: true, Platform: strPtr(upstream.IOSAuthPrefix)}
	account.Owner = scope.Owner()
	if err := h.store.UpsertAccount(ctx, scope, &account); err != nil {
		t.Fatalf("写入账号失败: %v", err)
	}
	authorization := core.SecretValue(
		authStringOfTest(upstream.IOSAuthPrefix, phone, loginAuthToken))
	state := core.AccountState{AccountID: id, Authorization: &authorization}
	if err := h.store.UpsertAccountState(ctx, scope, &state); err != nil {
		t.Fatalf("写入登录态失败: %v", err)
	}
	return id.String()
}

// waitRuns 等这一趟的每个账号都跑到终局，并返回按响应顺序排的事件。
//
// ⚠ 每个拿到 202 的用例**都必须**等它：起跑是在一个 goroutine 里，而库连接
// 由 `t.Cleanup` 关闭 —— 不等的话用例结束与后台写入之间有竞态，
// 报错会是"database is closed"这一类与判据毫无关系的东西。
func waitRuns(t *testing.T, h *harness, rows []map[string]any) {
	t.Helper()
	for _, row := range rows {
		id, ok := core.ParseRunID(asText(t, row["runId"]))
		if !ok {
			t.Fatalf("响应里的 runId 不是合法形状：%v", row["runId"])
		}
		waitForRunFinished(t, h, id)
	}
}

// seedDisabledAccount 塞一个**停用中**的账号，返回它的 id。
//
// 它不需要登录态：停用的账号在**受理之前**就被拦下了（走不到读凭据那一步）。
func seedDisabledAccount(t *testing.T, h *harness, id string) string {
	t.Helper()
	account := core.NewAccount(id)
	account.Enabled = false
	account.Owner = userIDOf(t, h)
	if err := h.store.UpsertAccount(context.Background(),
		core.ScopeUser(account.Owner), &account); err != nil {
		t.Fatalf("塞停用账号失败: %v", err)
	}
	return id
}

// seedForeignAccount 在**另一个用户**名下塞一个账号，返回它的 id。
//
// 走"切到 bob 的 cookie 再 [seedAccount]"而不是自己算 scope：`userIDOf` 是
// 按当前 cookie 打 `/api/session` 拿的，自己造一份等于把"归属"这件事的第二套
// 口径引进夹具。
func seedForeignAccount(t *testing.T, h *harness, id string) string {
	t.Helper()
	mine := h.cookie
	defer func() { h.cookie = mine }()
	h.cookie = secondUser(t, h, "bob-"+id)
	seedAccount(t, h, id)
	return id
}
