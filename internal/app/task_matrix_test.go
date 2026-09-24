package app

// [AppTaskMatrix] 那十六次拉取的用例。
//
// 这一层要钉住的四件"抄一遍就会漂移"的事：
//
//  1. **恰好 16 次、按那个顺序**（2 市场名 × 8 group）。多打 = 白打上游（这一族
//     没有幂等键，虽然都是读），少打 = 表不全而界面上看不出来。
//  2. **一次失败不中断、全部失败才报错**。反过来（一次就中断）的表现是：某个 group
//     上游哪天改形状，整张表就没了；反过来（全挂也回空表）的表现是：凭据坏掉的号
//     被念成"今天没任务"。两个方向都要有用例压着。
//  3. **合并语义**：去 HTML ⇒ 按 id 升序 ⇒ 按 name 去重保留 id 小的那条。
//     顺序反了（先去重再排序）留下的就是请求顺序里第一条，与 id 无关。
//  4. **一次跑只用一张票**（接池），撞 90001 只刷那一条会话、重放当前那一枪。
//     十六次各建一条会话是用户 2026-09-23 点名不许的那件事的放大版。
//
// ⚠ 这里**不**判"上游认不认 `activityid` 跟着市场名变"—— 一次真打都没有，
// 判据面见 `app_task_list_v2.go` 文件头；本文件钉的全是"我们发出去/我们怎么合"。

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// taskListV2Endpoint 是这一族的端点子串（断言"该打的打了、不该打的一次没打"两头都用）。
const taskListV2Endpoint = "/ycloud/signin/task/taskListV2"

// taskMatrixEmptyBody 是"这一组今天没有任务"的响应（实拍 group `new` 那一条的形状：
// `result` 是**空对象**，不是空数组，也没有 `groupState` 之外的键）。
const taskMatrixEmptyBody = `{"code":0,"msg":"success","result":{},"groupState":{}}`

// taskMatrixItem 拼一条任务。`state` / `enable` 传空串 ⇒ **不给那个键**（上游有时不给）。
type taskMatrixItem struct {
	id    int64
	name  string
	state string
	step  int64
	// noEnable 为真 ⇒ 不给 `enable`。
	noEnable bool
	enable   int64
}

func (i taskMatrixItem) json() string {
	parts := make([]string, 0, 6)
	if i.name != "" {
		parts = append(parts, `"name":`+quoteJSONString(i.name))
	}
	if i.id != 0 {
		parts = append(parts, `"id":`+itoa64(i.id))
	}
	if i.state != "" {
		parts = append(parts, `"state":`+quoteJSONString(i.state))
	}
	if !i.noEnable {
		parts = append(parts, `"enable":`+itoa64(i.enable))
	}
	if i.step != 0 {
		parts = append(parts, `"currstep":`+itoa64(i.step))
	}
	parts = append(parts, `"process":0`, `"groupid":"day"`, `"marketname":"sign_in_3"`)
	return "{" + strings.Join(parts, ",") + "}"
}

// quoteJSONString 是最小化的 JSON 字符串引号（这些夹具里没有需要转义的字符，
// 真出现引号/反斜杠时**立刻响**，免得悄悄拼出一份不合法的 JSON 再怪解析器）。
func quoteJSONString(text string) string {
	if strings.ContainsAny(text, `"\`) {
		panic("任务矩阵夹具不支持名字里带引号或反斜杠：" + text)
	}
	return `"` + text + `"`
}

// taskMatrixBody 给一个 group 若干条目。
func taskMatrixBody(group string, items ...taskMatrixItem) string {
	if len(items) == 0 {
		return taskMatrixEmptyBody
	}
	parts := make([]string, 0, len(items))
	for _, item := range items {
		parts = append(parts, item.json())
	}
	return `{"code":0,"msg":"success","result":{"` + group + `":[` +
		strings.Join(parts, ",") + `]},"groupState":{"` + group + `":{"taskFinishState":2}}}`
}

// taskMatrixSource 装配世界并取出**这一 (账号, 签到页)** 的那条会话源。
//
// 走 [SignInWorld.Source] 而不是直接 `NewSignInSessionSource`：这一条链的判据
// 恰好包含"接没接池"，手工造源会把那一格绕过去。
func taskMatrixSource(
	t *testing.T, script *liveTaskScript, pool *SessionPool,
) *SignInSessionSource {
	t.Helper()
	world := SignInWorld{
		Store:    &poolSignInStore{*liveTaskStore()},
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(script),
		Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
		Clock:    testClock(),
		Random:   testRng(),
		Sessions: pool,
	}
	src, perr := world.Source(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("取会话源失败: %v", perr)
	}
	return src
}

// taskMatrixScript 是"建会话按序换到这些票"+ 调用方那几条路由。
//
// ⚠ 与 `play_ai_run_test.go` 里那条同名的坑一样：[liveTaskScript] 取**第一条**匹配的路由，
// 所以 `tyrzLogin` 那份只能在构造时给，不能事后追加（追加的那支永远轮不到 ⇒ 假绿）。
func taskMatrixScript(tickets []string, routes ...liveTaskRoute) *liveTaskScript {
	jwts := make([]string, 0, len(tickets))
	for _, ticket := range tickets {
		jwts = append(jwts, jwtOK(ticket))
	}
	session := []liveTaskRoute{
		{"/user/querySpecTokenV2", []string{ssoOK("SSO-1")}},
		{"/ycloud/api/timestamp", []string{tsOK}},
		{"tyrzLogin", jwts},
	}
	return liveTaskScriptWith(append(session, routes...)...)
}

// taskMatrixRequests 取出打向这一族的那些请求（顺序即发出顺序）。
func taskMatrixRequests(script *liveTaskScript) []core.HTTPRequest {
	return requestsTo(script, taskListV2Endpoint)
}

// taskMatrixBodies 把那些请求的 body 解成 (marketname, group) 序列。
func taskMatrixBodies(t *testing.T, script *liveTaskScript) [][2]string {
	t.Helper()
	requests := taskMatrixRequests(script)
	out := make([][2]string, 0, len(requests))
	for _, request := range requests {
		var body map[string]any
		if err := json.Unmarshal(request.Body, &body); err != nil {
			t.Fatalf("body 不是合法 JSON：%s（%v）", request.Body, err)
		}
		market, _ := body["marketname"].(string)
		group, _ := body["group"].(string)
		out = append(out, [2]string{market, group})
	}
	return out
}

// TestAppTaskMatrixFetchesSixteenInOrder ⭐ 次数与顺序。
//
// 顺序按旧实现：先 `sign_in_3` 的八个 group，再 `newsign_139mail` 的八个 group，
// group 内部按 [upstream.TaskListV2Groups] 的字面顺序。
func TestAppTaskMatrixFetchesSixteenInOrder(t *testing.T) {
	script := taskMatrixScript([]string{"JWT-1"},
		liveTaskRoute{taskListV2Endpoint, []string{taskMatrixEmptyBody}})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(taskListV2Endpoint); n != 16 {
		t.Errorf("应恰好 16 次，实得 %d", n)
	}
	if matrix.Fetches != 16 || matrix.Fetches != TaskMatrixFetches() {
		t.Errorf("Fetches 应与 TaskMatrixFetches() 同源，实得 %d vs %d",
			matrix.Fetches, TaskMatrixFetches())
	}
	got := taskMatrixBodies(t, script)
	var want [][2]string
	for _, market := range TaskMatrixMarketNames {
		for _, group := range upstream.TaskListV2Groups {
			want = append(want, [2]string{market, group})
		}
	}
	if len(got) != len(want) {
		t.Fatalf("请求数与序列对不上：%d vs %d", len(got), len(want))
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("第 %d 次应是 %v，实得 %v", i, want[i], got[i])
		}
	}
	// 十六次只建**一条**会话（票 = 账号 × 页面，接了池就不该反复换）。
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("十六次应共用一张票（建会话 1 次），实得 %d 次", n)
	}
	if len(matrix.Failures) != 0 {
		t.Errorf("全成功时 Failures 应是空，实得 %v", matrix.Failures)
	}
}

// TestAppTaskMatrixDoesNotTouchTheWriteSide：这一条只读，别的端点一次都不打。
//
// 三个"做任务"的写口（`doTask` / `task/click` / `receiveTask`）与 `taskListV3`
// 出现在这条链上，后果不是报错而是**用户的东西被改了**（或读到另一张表）。
func TestAppTaskMatrixDoesNotTouchTheWriteSide(t *testing.T) {
	script := taskMatrixScript([]string{"JWT-1"},
		liveTaskRoute{taskListV2Endpoint, []string{taskMatrixEmptyBody}})
	if _, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool())); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	for _, forbidden := range []string{
		"/signin/page/doTask", "/signin/task/click", "/signin/task/taskClickV2",
		"/market/signin/page/receiveTask", "/ycloud/signin/task/taskListV3",
	} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("只读链不该打 %s，实得 %d 次", forbidden, n)
		}
	}
}

// TestAppTaskMatrixSingleGroupFailureDoesNotAbort：一次失败只登记，不打断其余。
func TestAppTaskMatrixSingleGroupFailureDoesNotAbort(t *testing.T) {
	bodies := make([]string, 0, 16)
	// 第 3 次（sign_in_3 的 beiyong2）回业务码非 0。
	for i := 0; i < 16; i++ {
		if i == 2 {
			bodies = append(bodies, `{"code":1,"msg":"系统繁忙"}`)
			continue
		}
		bodies = append(bodies, taskMatrixBody("day", taskMatrixItem{id: int64(100 + i), name: "任务"}))
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("一组失败不该中断整趟，实得 %v", perr)
	}
	if len(matrix.Failures) != 1 {
		t.Fatalf("应恰好登记 1 组失败，实得 %d", len(matrix.Failures))
	}
	if matrix.Failures[0].MarketName != upstream.MarketName || matrix.Failures[0].Group != "beiyong2" {
		t.Errorf("失败记录要点名是哪一组，实得 %+v", matrix.Failures[0])
	}
	if matrix.Failures[0].Err == nil || !strings.Contains(matrix.Failures[0].Err.Error(), "系统繁忙") {
		t.Errorf("原文要留在失败记录里便于排障，实得 %v", matrix.Failures[0].Err)
	}
	if len(matrix.Items) == 0 {
		t.Error("其余十五组的结果该照常带出来")
	}
	if line := matrix.SummaryLine(); !strings.Contains(line, "1/16") {
		t.Errorf("结论句该写明有几组没读到（否则界面把残表当全表），实得 %q", line)
	}
}

// TestAppTaskMatrixFailsOnlyWhenEveryFetchFails：十六次全挂 ⇒ 整体报错。
//
// 与上一条是一对。判反的表现不在测试里，在界面上：空表会被念成"这个号今天没任务"。
func TestAppTaskMatrixFailsOnlyWhenEveryFetchFails(t *testing.T) {
	bodies := make([]string, 0, 16)
	for i := 0; i < 16; i++ {
		bodies = append(bodies, `{"code":1,"msg":"系统繁忙"}`)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr == nil {
		t.Fatal("全挂应报错，实得一份空表（这正是刻意不同②要挡的那件事）")
	}
	if !strings.Contains(perr.Error(), "16 次拉取全部失败") {
		t.Errorf("错误该写明是全挂，实得 %q", perr.Error())
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("上游原文该带出来，实得 %q", perr.Error())
	}
	if len(matrix.Items) != 0 || matrix.Fetches != 0 {
		t.Errorf("整体失败时不该交出半份结论，实得 %+v", matrix)
	}
}

// TestAppTaskMatrixMergesThenSortsThenDedupsByNameKeepingLowestID ⭐ 合并语义。
//
// 两条同名任务分别落在**先请求的组**（id 大）与**后请求的组**（id 小）：
// 只按请求顺序去重会留下 id 大的那条，反过来（先排序再去重）留下 id 小的。
// 留下哪条不是口味问题 —— 将来接写侧时要按 `currstep` 决定点击带第几步。
func TestAppTaskMatrixMergesThenSortsThenDedupsByNameKeepingLowestID(t *testing.T) {
	bodies := []string{
		// 第 1 次（sign_in_3 / new）：id 大的一条。
		taskMatrixBody("new", taskMatrixItem{id: 900, name: "去上传照片", state: "WAIT", step: 9}),
		// 第 2 次（sign_in_3 / beiyong1）：id 小的同名一条。
		taskMatrixBody("beiyong1", taskMatrixItem{id: 12, name: "去上传照片", state: "WAIT", step: 1}),
	}
	for i := 2; i < 16; i++ {
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(matrix.Items) != 1 {
		t.Fatalf("同名两条应去重成一条，实得 %d", len(matrix.Items))
	}
	kept := matrix.Items[0]
	if kept.TaskID == nil || *kept.TaskID != 12 {
		t.Errorf("应留下 id 小的那条（先排序后去重），实得 %v", kept.TaskID)
	}
	if kept.CurrStep == nil || *kept.CurrStep != 1 {
		t.Errorf("随条目带出的 currstep 也应是留下那条的，实得 %v", kept.CurrStep)
	}
}

// TestAppTaskMatrixStripsHTMLFromNames：任务名带标签（旧 TS `stripHtml`，`helpers.ts:88-91`）。
func TestAppTaskMatrixStripsHTMLFromNames(t *testing.T) {
	bodies := []string{
		taskMatrixBody("new", taskMatrixItem{id: 1, name: "<b>去</b>上传照片<font color='red'>（+10）</font>", state: "WAIT"}),
	}
	for i := 1; i < 16; i++ {
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(matrix.Items) != 1 {
		t.Fatalf("应 1 条，实得 %d", len(matrix.Items))
	}
	if got := *matrix.Items[0].Name; got != "去上传照片（+10）" {
		t.Errorf("标签应被剥掉，实得 %q", got)
	}
}

// TestAppTaskMatrixDedupUsesTheStrippedName：去重的键是**剥完标签之后**的名字。
//
// 反例（先按原名去重再剥）会让 `<b>去上传</b>` 与 `去上传` 各留一条 ——
// 界面上就是同一个任务出现两行。
func TestAppTaskMatrixDedupUsesTheStrippedName(t *testing.T) {
	bodies := []string{
		taskMatrixBody("new", taskMatrixItem{id: 1, name: "去上传", state: "WAIT"}),
		taskMatrixBody("beiyong1", taskMatrixItem{id: 2, name: "<em>去上传</em>", state: "WAIT"}),
	}
	for i := 2; i < 16; i++ {
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(matrix.Items) != 1 {
		t.Fatalf("剥完标签同名的两条应合成一条，实得 %d：%v",
			len(matrix.Items), matrix.Items)
	}
	if *matrix.Items[0].TaskID != 1 {
		t.Errorf("应留 id 小的那条，实得 %v", matrix.Items[0].TaskID)
	}
}

// TestAppTaskMatrixKeepsNamelessAndNilIDItems：两处"缺字段"的取向。
//
//   - `name` 缺席 ⇒ **各留各的**（刻意不同③；旧 TS 会全撞成一个 undefined 键只剩一条）；
//   - `id` 缺席 ⇒ 排在**最后**、且不被当成 0（当成 0 会跑到最前面，把同名条目挤掉）。
func TestAppTaskMatrixKeepsNamelessAndNilIDItems(t *testing.T) {
	const noID = `{"state":"WAIT","enable":1,"process":0,"groupid":"day"}`
	const noName = `{"id":55,"state":"WAIT","enable":1}`
	bodies := []string{
		`{"code":0,"msg":"success","result":{"new":[` + noID + `,` + noID + `]}}`,
		`{"code":0,"msg":"success","result":{"beiyong1":[` + noName + `,` + noName + `]}}`,
		taskMatrixBody("beiyong2", taskMatrixItem{id: 7, name: "有名字的那条", state: "WAIT"}),
	}
	for i := 3; i < 16; i++ {
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	// 无 `name` 的条目**各留各的**（刻意不同③）⇒ 2 条无 id + 2 条 id 55 + 1 条 id 7 = 5。
	// 反过来（按旧 TS 的 `uniqBy(name)`）会留下 1 + 1 + 1 = 3 条，即**静默少两条任务**。
	if len(matrix.Items) != 5 {
		t.Fatalf("应 5 条（无 name 的一律不去重），实得 %d", len(matrix.Items))
	}
	last := matrix.Items[len(matrix.Items)-1]
	if last.TaskID != nil {
		t.Errorf("没有 id 的条目应排在最后，实得末位 id=%v", *last.TaskID)
	}
	if last.Name != nil {
		t.Errorf("无 name 的条目应原样保留 nil，实得 %q", *last.Name)
	}
	if got := matrix.SummaryLine(); !strings.Contains(got, "5 条") {
		t.Errorf("结论句里的条数应与 Items 同源，实得 %q", got)
	}
}

// TestAppTaskMatrixPendingGate："要不要做"那道门，四档各一次。
func TestAppTaskMatrixPendingGate(t *testing.T) {
	bodies := []string{
		taskMatrixBody("new",
			taskMatrixItem{id: 1, name: "待做", state: "WAIT", enable: 1},
			taskMatrixItem{id: 2, name: "已完成", state: "FINISH", enable: 1},
			taskMatrixItem{id: 3, name: "没给 enable", state: "WAIT", noEnable: true},
			taskMatrixItem{id: 4, name: "enable 是 0", state: "WAIT", enable: 0},
			taskMatrixItem{id: 5, name: "没给 state", enable: 1},
		),
	}
	for i := 1; i < 16; i++ {
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1"}, liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	pending := matrix.Pending()
	if len(pending) != 2 {
		t.Fatalf("应恰好 2 条待做（id 1 与 5），实得 %d：%v", len(pending), namesOf(pending))
	}
	if *pending[0].TaskID != 1 || *pending[1].TaskID != 5 {
		t.Errorf("待做的那两条应是 id 1 与 5，实得 %v %v", pending[0].TaskID, pending[1].TaskID)
	}
}

func namesOf(items []upstream.TaskV2Item) []string {
	out := make([]string, 0, len(items))
	for _, item := range items {
		if item.TaskID == nil {
			out = append(out, "<无 id>")
			continue
		}
		out = append(out, itoa64(*item.TaskID))
	}
	return out
}

// TestAppTaskMatrixRefreshesOnceOnExpiredSession：第 5 次撞 90001 ⇒ 刷一次、重放那一枪。
//
// 十六次共用同一条会话，所以"刷一次"之后**余下的那些次要用新票** ——
// 只数 `tyrzLogin` 次数的话，"刷了但没换出去"这件事是绿的。
func TestAppTaskMatrixRefreshesOnceOnExpiredSession(t *testing.T) {
	bodies := make([]string, 0, 17)
	for i := 0; i < 16; i++ {
		if i == 4 {
			bodies = append(bodies, notLoggedIn)
			continue
		}
		bodies = append(bodies, taskMatrixEmptyBody)
	}
	script := taskMatrixScript([]string{"JWT-1", "JWT-2"},
		liveTaskRoute{taskListV2Endpoint, bodies})
	matrix, perr := AppTaskMatrix(context.Background(),
		taskMatrixSource(t, script, NewSessionPool()))
	if perr != nil {
		t.Fatalf("刷票后应继续，实得 %v", perr)
	}
	if n := script.count("tyrzLogin"); n != 2 {
		t.Errorf("应恰好刷一次（建会话合计 2 次），实得 %d", n)
	}
	if n := script.count(taskListV2Endpoint); n != 17 {
		t.Errorf("十六次 + 重放一次 = 17，实得 %d", n)
	}
	if len(matrix.Failures) != 0 {
		t.Errorf("自救成功就不该留下失败记录，实得 %v", matrix.Failures)
	}
	requests := taskMatrixRequests(script)
	// 第 5 枪（下标 4）带旧票、重放那一枪（下标 5）带新票。
	if got, _ := headerOf(requests[4], "jwttoken"); got != "JWT-1" {
		t.Errorf("原枪该带当时那张票，实得 %q", got)
	}
	if got, _ := headerOf(requests[5], "jwttoken"); got != "JWT-2" {
		t.Errorf("重放那一枪该带刷新后的票，实得 %q", got)
	}
	for i, request := range requests[6:] {
		if got, _ := headerOf(request, "jwttoken"); got != "JWT-2" {
			t.Errorf("刷新之后第 %d 枪仍带着旧票（池没更新），实得 %q", i+7, got)
		}
	}
}

// TestAppTaskMatrixTwoRoundsShareOneTicket：接池 ⇒ 第二轮不重建会话。
//
// 与不接池那份对照（同一份脚本、`nil` 池）⇒ 差值就是"漏接池"的代价：
// 每轮多打三枪，其中一枪还是真换票。
func TestAppTaskMatrixTwoRoundsShareOneTicket(t *testing.T) {
	for _, tc := range []struct {
		name       string
		pool       func() *SessionPool
		wantOpens  int
		wantTicket string
	}{
		// 接池：第二轮仍用**第一张**票（建会话合计 1 次）。
		{"接池", func() *SessionPool { return NewSessionPool() }, 1, "JWT-1"},
		// 不接池：每轮各建一条 ⇒ 第二轮换到脚本里的第二张票。
		{"不接池", func() *SessionPool { return nil }, 2, "JWT-2"},
	} {
		script := taskMatrixScript([]string{"JWT-1", "JWT-2"},
			liveTaskRoute{taskListV2Endpoint, []string{taskMatrixEmptyBody}})
		pool := tc.pool()
		for round := 0; round < 2; round++ {
			if _, perr := AppTaskMatrix(context.Background(),
				taskMatrixSource(t, script, pool)); perr != nil {
				t.Fatalf("%s 第 %d 轮失败: %v", tc.name, round+1, perr)
			}
		}
		if n := script.count("tyrzLogin"); n != tc.wantOpens {
			t.Errorf("%s：两轮合计应建会话 %d 次，实得 %d", tc.name, tc.wantOpens, n)
		}
		if n := script.count(taskListV2Endpoint); n != 32 {
			t.Errorf("%s：两轮应各打 16 次，实得 %d", tc.name, n)
		}
		requests := taskMatrixRequests(script)
		if got, _ := headerOf(requests[len(requests)-1], "jwttoken"); got != tc.wantTicket {
			t.Errorf("%s：最后一轮用的票应是 %s，实得 %q", tc.name, tc.wantTicket, got)
		}
	}
}

// TestTaskMatrixNeverClickIsTheOldList：那份 `SKIP_TASK_LIST` 逐字登记。
//
// 写侧的调用点是 [taskMatrixSkipsClick]（default 分支那次盲点），**行为**在
// `task_matrix_run_test.go` 那侧钉；这里只钉"表本身没被改错"——
// 尤其 [TaskMatrixNeverClick] 注释里那句"它挡的不是做不做"。
func TestTaskMatrixNeverClickIsTheOldList(t *testing.T) {
	want := []int64{117, 437, 991, 406, 385, 480, 1022, 1018, 603, 1021, 120}
	if len(TaskMatrixNeverClick) != len(want) {
		t.Fatalf("应恰好 %d 个 id，实得 %d", len(want), len(TaskMatrixNeverClick))
	}
	for i := range want {
		if TaskMatrixNeverClick[i] != want[i] {
			t.Errorf("第 %d 个应是 %d，实得 %d", i, want[i], TaskMatrixNeverClick[i])
		}
	}
}

// TestTaskMatrixFetchesMatchesTheLoop：那个"16"与循环体真的是同一个来源。
//
// 反向改坏它的样子：把 [TaskMatrixFetches] 写成常量 16 —— 将来谁往
// [upstream.TaskListV2Groups] 里加一个 group，"全挂才失败"那条判据就会**永远不成立**
// （失败数追不上常量），而这条用例会红。
func TestTaskMatrixFetchesMatchesTheLoop(t *testing.T) {
	if got, want := TaskMatrixFetches(),
		len(TaskMatrixMarketNames)*len(upstream.TaskListV2Groups); got != want {
		t.Errorf("Fetches 应等于市场名数 × group 数，实得 %d vs %d", got, want)
	}
	if got := TaskMatrixFetches(); got != 16 {
		t.Errorf("今天这一族是 16 次，实得 %d（改了数组要一并回看文件头那三处口径）", got)
	}
}
