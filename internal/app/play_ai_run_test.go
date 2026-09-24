package app

// [PlayAIRun] 的用例：只读两枪、票借签到页、referer 用这一页自己的那一份。
//
// 这一层要钉住的四件"抄一遍就会漂移"的事：
//
//  1. **只打读的那两口** —— 这一条能力的边界就是"看"。同一族那三个写端点
//     （`task/step/click` 上报一步、`blindbox/lottery` 抽一次、`aiRecognize` 往网盘写图）
//     一旦被顺手带上，表现不是报错而是**账号状态被改了 / 网盘多了文件**，
//     而界面文案还写着"只读"。
//  2. **两枪都致命**，且**不打第二枪**：缺 ② 会让用户以为"这个号没事做"，
//     那是会改变行为的误读 ⇒ 不学播报那一格的宽容档。
//  3. **票从会话来、referer 从这一页来**（文件头"Referer 不借"那一段）。
//     这一条最容易写错，而且错了**看起来完全正常**：拿 `s.referer` 顶上的话，
//     发出去的是"签到页的 referer + 拍拍的 URL"，只有上游会告诉我们行不行。
//     ⚠ 这个组合本身**零实拍**（见 `docs/capture/app-play-ai-samples.txt` §5），
//     所以这里钉的是"我们发出去的是哪一份"，**不是**"上游认这一份"。
//  4. **接票池**：漏接不报错，只让建会话那三枪按轮次翻倍（用户 2026-09-23 点名不许）。
//     与红包派对那一条**相反**：那一条有自己的票、必须另起一份池；这一条借签到页那张，
//     所以**必须**共用同一份池 —— 两条各写一遍的代价就是这两句早晚有一句会漂移。
//
// 判据（`totalLotteryCount` 到底是什么意思）**不在这里测**：它未验证，
// 本层只保证"没解释它"（见 [TestPlayAISummaryLineHedgesTheLotteryCount]）。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 两枪的端点子串（写成常量是因为断言两头都要用："该打的打了一次"与"不该打的一次没打"）。
const (
	playAIStatusEndpoint = "/ycloud/newyear/user/info"
	playAITaskEndpoint   = "/ycloud/newyear/task/list"
)

// 同一族那三个**写**端点。它们出现在只读链上不是"多一次请求"，而是改了用户的东西。
const (
	playAIStepClickEndpoint = "/ycloud/newyear/task/step/click"
	playAILotteryEndpoint   = "/ycloud/newyear/blindbox/lottery"
	playAIRecognizeEndpoint = "aiRecognize"
)

// playAIStatusBody 是 ① 的响应。`count` 原样拼进 `totalLotteryCount`，
// 于是"0""1""null""这一格没了"四种形态共用一个构造点。
func playAIStatusBody(count string) string {
	if count == "" {
		return `{"code":0,"msg":"success","result":{"showData":{"titleImage":"t.png"}}}`
	}
	return `{"code":0,"msg":"success","result":{"totalLotteryCount":` + count + `}}`
}

// playAITaskBody 是 ② 的响应：四条任务，四档各一（WAIT / FINISH / 没见过的值 / 没有 state）。
//
// 条目取自实拍夹具（`app_play_ai_test.go` 那一份），刻意**不**只留两档：
// "未知那一档存在"正是这一层要单独计数的东西，夹具里没有它就测不到。
const playAITaskBody = `{"code":0,"msg":"success","result":[` +
	`{"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0},` +
	`{"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",` +
	`"groupId":"popup","state":"FINISH","limitType":"EVER","recordid":"20466372","currstep":2,"process":0},` +
	`{"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"DOING","limitType":"DAY","recordid":null,"currstep":1,"process":0},` +
	`{"id":7,"name":"缺状态的那条","flag":"?","groupId":"normal","limitType":"DAY"}]}`

// playAIReadFailBody 是"这一枪没成"的响应（业务码非 0，且**不是**会话过期）。
const playAIReadFailBody = `{"code":1,"msg":"活动太火爆，请稍后重试"}`

// playAIRun 装配这一条链要的世界。
//
// ⚠ 刻意**不**复用 [signInWorldFor]：那份夹具不带 `Sessions` 与 `Progress`
// （它服务的那几条链各有自己的接法），而这一条链的两个判据恰好就是"接没接池"
// 与"说了什么话"。用它的夹具会让下面两条用例挂在"永远读到 nil"那一侧。
func playAIRun(script *liveTaskScript, pool *SessionPool, progress RunProgress) PlayAIRun {
	return PlayAIRun{SignInWorld: SignInWorld{
		Store:    &poolSignInStore{*liveTaskStore()},
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(script),
		Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
		Clock:    testClock(),
		Random:   testRng(),
		Sessions: pool,
		Progress: progress,
	}}
}

// playAIRunStore 换一份库夹具（"库里没凭据"那一档用）。
func playAIRunStore(store core.StateStore, script *liveTaskScript) PlayAIRun {
	return PlayAIRun{SignInWorld: signInWorldFor(store, script)}
}

// playAIRunScript 是"建会话一次成功"的那份夹具。
func playAIRunScript(routes ...liveTaskRoute) *liveTaskScript {
	return playAIScriptWithTickets([]string{"JWT-1"}, routes...)
}

// playAIScriptWithTickets 让调用方指定**每次建会话换到哪张票**。
//
// ⚠ 不能图省事往 [playAIRunScript] 上再追加一条 `tyrzLogin` 路由：
// [liveTaskScript] 取**第一条**匹配的路由，追加那份永远轮不到，
// 于是"第二轮有没有换票"这种判据会读成"两轮同一张"（假绿）。
func playAIScriptWithTickets(tickets []string, routes ...liveTaskRoute) *liveTaskScript {
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

// ─────────────────────────────────────────────────────────────────────────────
// 打哪些枪、按什么顺序、带什么头
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIRunReadsExactlyTwo：⭐ 只读两枪各一次，三个写端点一次都不打。
func TestPlayAIRunReadsExactlyTwo(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("0")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	res, perr := playAIRun(script, nil, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(playAIStatusEndpoint); n != 1 {
		t.Errorf("user/info 应被打 1 次，实得 %d", n)
	}
	if n := script.count(playAITaskEndpoint); n != 1 {
		t.Errorf("task/list 应被打 1 次，实得 %d", n)
	}
	// 这三个是"做掉任务 / 抽一次 / 往网盘写图"。它们出现在只读链上，
	// 后果不是报错而是**用户的东西被改了**，所以逐点数一遍。
	for _, forbidden := range []string{playAIStepClickEndpoint, playAILotteryEndpoint, playAIRecognizeEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("只读链不该打 %s，实得 %d 次", forbidden, n)
		}
	}
	// 顺序：实拍序列就是"先状态后任务表"（夹具第 4、5 条）。
	// 反过来打不会报错，但"②之后再看①"意味着 ① 读到的次数含本次动作的影响，
	// 那一格本来就没解释 ⇒ 别再给它多一层歧义。
	if first, second := requestIndex(script, playAIStatusEndpoint), requestIndex(script, playAITaskEndpoint); first > second {
		t.Errorf("两枪顺序反了：user/info 在第 %d 条请求，task/list 在第 %d 条", first, second)
	}
	// 读数原样带出（不是"这一趟没白跑"的证据，是"下一层拿得到什么"的证据）。
	if len(res.Board.Tasks) != 4 {
		t.Fatalf("应带出 4 条任务，实得 %d", len(res.Board.Tasks))
	}
	if got := res.Board.Counts(); got != (PlayAITaskCounts{Total: 4, Done: 1, Waiting: 1, Unknown: 2}) {
		t.Errorf("四档计数漂移：实得 %+v", got)
	}
	if res.Board.Status.TotalLotteryCount == nil || *res.Board.Status.TotalLotteryCount != 0 {
		t.Errorf("totalLotteryCount 实拍 0 ⇒ 应是非 nil 的 0，实得 %v", res.Board.Status.TotalLotteryCount)
	}
	if res.Status != PlayAIReadDone {
		t.Errorf("档位应是 PlayAIReadDone，实得 %v", res.Status)
	}
}

// TestPlayAIRunBorrowsTheSignInTicketButNotItsPage ⭐ 文件头第 3 条那一格。
//
// 三个读数一起看才算钉住：
//   - `jwttoken` = 签到页那条会话换到的**那一张**（票借、不另起池）；
//   - `Referer` = **这一页**的落地串（含 `National_PlayAISpecial`，**不含**签到页的 `newsignin`）；
//   - `showloading` = 我们这一侧钉的那个值。
//
// ⚠ 这一条**不**主张"上游认这份 referer"——那是零实拍（见文件头）。
func TestPlayAIRunBorrowsTheSignInTicketButNotItsPage(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("1")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	if _, perr := playAIRun(script, nil, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	business := requestsTo(script, playAIStatusEndpoint, playAITaskEndpoint)
	if len(business) != 2 {
		t.Fatalf("应恰好两条业务请求，实得 %d", len(business))
	}
	for i, request := range business {
		if got, _ := headerOf(request, "jwttoken"); got != "JWT-1" {
			t.Errorf("第 %d 条业务请求带的不是签到页那张票：%q", i+1, got)
		}
		referer, _ := headerOf(request, "referer")
		if referer != upstream.PlayAIPageReferer() {
			t.Errorf("第 %d 条的 referer 不是这一页那一份：\n实得 %q\n期望 %q",
				i+1, referer, upstream.PlayAIPageReferer())
		}
		if !strings.Contains(referer, upstream.PlayAIMarketName) {
			t.Errorf("referer 里该有这一页的市场名 %q，实得 %q", upstream.PlayAIMarketName, referer)
		}
		// 反向那一半同样重要：借了签到页那份的话，这一句读起来和上面两句一样"正常"。
		if strings.Contains(referer, "newsignin") {
			t.Errorf("第 %d 条借用了签到页 referer：%q", i+1, referer)
		}
		if got, _ := headerOf(request, "showloading"); got != upstream.PlayAIShowLoading {
			t.Errorf("第 %d 条的 showloading 实得 %q，期望 %q", i+1, got, upstream.PlayAIShowLoading)
		}
	}
}

// TestPlayAIRunUsesSharedSession：这条链**复用**签到页那一条会话，不自建第二套建会话流程。
func TestPlayAIRunUsesSharedSession(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("0")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	if _, perr := playAIRun(script, nil, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("建会话应恰好一次，实得 %d", n)
	}
}

// TestPlayAIRunTwoRoundsShareOneTicket ⭐ 界面上连着点两次"拍拍盘点"那一格。
//
// 与 `cloud_phone_run_test.go` 同名那一条**同一个判据、相反的结论**：
// 这一族**该**共用签到页那份池（借的就是那张票），红包派对那族**不该**
// （它有自己的票，共用会互相顶）。两条都留着，是因为"接个池"这个动作
// 在两边长得一模一样，而接错方向的池不会有任何测试变红。
func TestPlayAIRunTwoRoundsShareOneTicket(t *testing.T) {
	rounds := func(withPool bool) (int, []string) {
		// 两条不同响应：第二轮若真的又建一条会话，它会拿到 JWT-2
		// ⇒ 下面那句"票没换过"才有读数。
		script := playAIScriptWithTickets([]string{"JWT-1", "JWT-2"},
			liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("0")}},
			liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
		)
		var pool *SessionPool
		if withPool {
			pool = NewSessionPool()
		}
		run := playAIRun(script, pool, nil)
		for round := 1; round <= 2; round++ {
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("第 %d 轮盘点失败：%s", round, perr.Message())
			}
			if res.Status != PlayAIReadDone {
				t.Fatalf("第 %d 轮结论不对：%s", round, res.SummaryLine())
			}
		}
		if withPool && pool.Entries() != 1 {
			t.Errorf("两轮过后池里该只有 1 条，实得 %d", pool.Entries())
		}
		return script.count("tyrzLogin"), poolTokens(script)
	}

	built, tokens := rounds(true)
	// 反向改坏点：把 `signInWorldFor` 里那句 `Sessions` 去掉 ⇒ built 变 2。
	if built != 1 {
		t.Errorf("接了池 ⇒ 两轮只该建 1 条会话，tyrzLogin 实得 %d 次", built)
	}
	// 两轮 × 两枪 = 四条业务请求，全部带**同一张**票。
	if len(tokens) != 4 {
		t.Fatalf("四条业务请求都该带票，实得 %d 条：%v", len(tokens), tokens)
	}
	for i, token := range tokens {
		if token != "JWT-1" {
			t.Fatalf("第 %d 条业务请求换了票：%s（接了池就不该有第二张）", i+1, token)
		}
	}

	built, tokens = rounds(false)
	if built != 2 {
		t.Errorf("没接池时每轮该各建一条（对照不成立说明上面那个开关没走进分岔），实得 %d", built)
	}
	if len(tokens) != 4 || tokens[0] == tokens[3] {
		t.Errorf("没接池时第二轮该换一张票，实得 %v", tokens)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 失败面：两枪都致命、会话过期只刷一次
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIRunFirstReadIsFatal：① 挂 ⇒ **不打 ②**、交回错误、结论是零值。
//
// 反向改坏它（把 ① 的错误折成"零值继续"）会让界面上出现一句
// "拍拍任务 4 件（未完成 …）"而次数那一格读成"上游没给" ——
// 那是把"没查成"念成"查过了"。
func TestPlayAIRunFirstReadIsFatal(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIReadFailBody}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	res, perr := playAIRun(script, nil, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("① 失败必须交回错误")
	}
	if !strings.Contains(perr.Error(), "活动太火爆") {
		t.Errorf("上游原文要一路带到错误里，实得 %q", perr.Error())
	}
	if n := script.count(playAITaskEndpoint); n != 0 {
		t.Errorf("① 已经挂了，不该再打 ②，实得 %d 次", n)
	}
	if res.Status != PlayAIReadFailed || len(res.Board.Tasks) != 0 ||
		res.Board.Status.TotalLotteryCount != nil {
		t.Errorf("失败时结论必须是零值，不能带上一次尝试的残值：实得 %+v", res)
	}
}

// TestPlayAIRunSecondReadIsAlsoFatal：② 挂 = 整笔挂（这一条不学"播报失败只留 nil"）。
func TestPlayAIRunSecondReadIsAlsoFatal(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("1")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAIReadFailBody}},
	)
	res, perr := playAIRun(script, nil, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("② 失败必须交回错误：缺任务表会被念成\"这个号没事做\"")
	}
	if n := script.count(playAIStatusEndpoint); n != 1 {
		t.Errorf("② 挂掉之前该恰好打过一次 ①，实得 %d", n)
	}
	if res.Status != PlayAIReadFailed || res.Board.Status.TotalLotteryCount != nil {
		t.Errorf("② 挂了要把 ① 的读数也归零（不然半份结论会进界面），实得 %+v", res)
	}
}

// TestReadPlayAIBoardZeroesTheBoardOnSecondFailure：② 挂 ⇒ **交回的板子是零值**。
//
// ⚠ 这一条必须打在 [ReadPlayAIBoard] 上，不能只靠上面那条 run 级用例：
// [PlayAIRun.RunAccount] 在失败支路上本来就自己构造了一份不含 `Board` 的结论，
// 于是"把 ① 的残值带出去"这种改法在 run 层**看不出来**（实测过：变异 m8 绿）。
// 半份读数（有次数、没任务表）正是界面最容易当成完整结论去念的那一格。
func TestReadPlayAIBoardZeroesTheBoardOnSecondFailure(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("3")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAIReadFailBody}},
	)
	src, perr := playAIRun(script, nil, nil).Source(
		context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("取会话源失败: %v", perr)
	}
	// 反向改坏点：把 ② 失败那一支改成 `return PlayAIBoard{Status: status}, perr` ⇒ 这里红。
	board, perr := ReadPlayAIBoard(context.Background(), src)
	if perr == nil {
		t.Fatal("② 失败必须交回错误")
	}
	if board.Status.TotalLotteryCount != nil || board.Tasks != nil {
		t.Errorf("失败时交回的板子必须是零值，实得 %+v", board)
	}
}

// TestPlayAIRunRefreshesOnceOnExpiredSession：`90001` ⇒ 刷一次、重放一次，只此一次。
//
// 走的是 [SignInSessionSource.Call] 那条通用支路，但**调用点也要测**：
// 写成"任何错都刷一次再来"的表现是——上面那条 `code:1`（"活动太火爆"）
// 会被重放一次，而那是一句参数/风控层面的话，刷一万次也没用（见 `signin_session.go` 头注）。
func TestPlayAIRunRefreshesOnceOnExpiredSession(t *testing.T) {
	script := playAIScriptWithTickets([]string{"JWT-1", "JWT-2"},
		// ① 第一枪回未登录，重放那一枪成功。
		liveTaskRoute{playAIStatusEndpoint, []string{notLoggedIn, playAIStatusBody("0")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	pool := NewSessionPool()
	res, perr := playAIRun(script, pool, nil).RunAccount(
		context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("刷新后重放应成功，实得：%v", perr)
	}
	if res.Status != PlayAIReadDone {
		t.Fatalf("档位应是成功，实得 %v", res.Status)
	}
	if n := script.count("tyrzLogin"); n != 2 {
		t.Errorf("应恰好刷一次（建会话合计 2 次），实得 %d", n)
	}
	if n := script.count(playAIStatusEndpoint); n != 2 {
		t.Errorf("① 应被打两次（原枪 + 重放），实得 %d", n)
	}
	if n := script.count(playAITaskEndpoint); n != 1 {
		t.Errorf("② 不该被重放，实得 %d", n)
	}
	// 重放那一枪带的是**新票**：还带着旧票的话，"刷了"这件事只发生在计数上，没发生在请求上。
	statusRequests := requestsTo(script, playAIStatusEndpoint)
	if len(statusRequests) != 2 {
		t.Fatalf("夹具没成立：① 实得 %d 条请求", len(statusRequests))
	}
	if got, _ := headerOf(statusRequests[1], "jwttoken"); got != "JWT-2" {
		t.Errorf("重放那一枪该带刷新后的票，实得 %q", got)
	}
	// 刷新只换**这一页**那一条：池里仍然只有一条（账号 × 签到页配方）。
	if pool.Entries() != 1 {
		t.Errorf("刷新后池里该还是 1 条，实得 %d", pool.Entries())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 念读数：计数分档、小结、报告
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIBoardCounts：三档 + 缺键，全部不并进"未完成"。
func TestPlayAIBoardCounts(t *testing.T) {
	task := func(state string) upstream.PlayAIUserTask {
		out := upstream.PlayAIUserTask{}
		if state != "" {
			value := state
			out.State = &value
		}
		return out
	}
	cases := []struct {
		name  string
		tasks []upstream.PlayAIUserTask
		want  PlayAITaskCounts
	}{
		{"空表不等于没事做", nil, PlayAITaskCounts{}},
		{"两档齐全", []upstream.PlayAIUserTask{task("WAIT"), task("FINISH")},
			PlayAITaskCounts{Total: 2, Done: 1, Waiting: 1}},
		{"没见过的值单独一档", []upstream.PlayAIUserTask{task("DOING"), task("WAIT")},
			PlayAITaskCounts{Total: 2, Waiting: 1, Unknown: 1}},
		{"缺 state 也进未知（nil 折成空串后落 default）",
			[]upstream.PlayAIUserTask{task(""), task("FINISH")},
			PlayAITaskCounts{Total: 2, Done: 1, Unknown: 1}},
		// 大小写敏感是**这一族**的口径（不归一，见 `app_task_list.go` 那段"为什么不归一大小写"）。
		{"小写 finish 算读不出，不算完成", []upstream.PlayAIUserTask{task("finish")},
			PlayAITaskCounts{Total: 1, Unknown: 1}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := (PlayAIBoard{Tasks: c.tasks}).Counts(); got != c.want {
				t.Errorf("实得 %+v，期望 %+v", got, c.want)
			}
		})
	}
}

// TestPlayAISummaryLineHedgesTheLotteryCount：⭐ 那句话不许把未验证的语义念成结论。
//
// ⚠ 这几句会进 `runs.message` 与推送标题，改一个字就算界面漂移。
func TestPlayAISummaryLineHedgesTheLotteryCount(t *testing.T) {
	count := func(value int64) *int64 { return &value }
	cases := []struct {
		name   string
		result PlayAIResult
		want   string
		must   []string
		forbid []string
	}{
		{
			name:   "有次数：念读数、并标明这一格没解释",
			result: PlayAIResult{Status: PlayAIReadDone, Board: PlayAIBoard{Status: upstream.PlayAIUserStatus{TotalLotteryCount: count(1)}}},
			want: "拍拍任务 0 件（未完成 0、已完成 0）；抽奖次数那一格读到 1（是累计获得还是还剩，未验证）；" +
				"本工具只读：不做任务、不抽奖、不写网盘",
			must: []string{"未验证"},
			// "还能抽 N 次""N 次可抽"都是把未验证那一格念成结论。
			// ⚠ 别拿"还剩"当禁词：我们自己的那句 hedge 就写着"是累计获得还是还剩"，
			// 那正是**不许**下判断的写法 —— 禁它会把 hedge 本身禁掉。
			forbid: []string{"还能抽", "可抽", "剩余"},
		},
		{
			name:   "零次：0 是合法取值，不能念成\"没给\"",
			result: PlayAIResult{Status: PlayAIReadDone, Board: PlayAIBoard{Status: upstream.PlayAIUserStatus{TotalLotteryCount: count(0)}}},
			want: "拍拍任务 0 件（未完成 0、已完成 0）；抽奖次数那一格读到 0（是累计获得还是还剩，未验证）；" +
				"本工具只读：不做任务、不抽奖、不写网盘",
			must:   []string{"读到 0"},
			forbid: []string{"上游没给"},
		},
		{
			name:   "上游没给这一格：只报\"没给\"，不猜它等于几",
			result: PlayAIResult{Status: PlayAIReadDone, Board: PlayAIBoard{}},
			want: "拍拍任务 0 件（未完成 0、已完成 0）；抽奖次数那一格上游没给（这一格语义本就未验证，" +
				"不据此判断）；本工具只读：不做任务、不抽奖、不写网盘",
			must:   []string{"上游没给"},
			forbid: []string{"读到"},
		},
		{
			name: "状态读不出那一档要念出来",
			result: PlayAIResult{Status: PlayAIReadDone, Board: PlayAIBoard{
				Tasks: []upstream.PlayAIUserTask{{State: strPtr("WAIT")}, {State: strPtr("DOING")}},
			}},
			want: "拍拍任务 2 件（未完成 1、已完成 0、状态读不出 1）；抽奖次数那一格上游没给" +
				"（这一格语义本就未验证，不据此判断）；本工具只读：不做任务、不抽奖、不写网盘",
			must:   []string{"状态读不出 1"},
			forbid: nil,
		},
		{
			name:   "失败：交回原因，不拼一份看起来完整的读数",
			result: PlayAIResult{Status: PlayAIReadFailed, Error: "上游回 1：活动太火爆"},
			want:   "拍拍没读成：上游回 1：活动太火爆",
			must:   []string{"活动太火爆"},
			forbid: []string{"任务 0 件"},
		},
		{
			name:   "失败但错误串是空的也要成句（不留下\"：\"结尾那种半截话）",
			result: PlayAIResult{Status: PlayAIReadFailed},
			want:   "拍拍没读成",
			must:   nil,
			forbid: []string{"："},
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := c.result.SummaryLine()
			if got != c.want {
				t.Errorf("实得 %q\n期望 %q", got, c.want)
			}
			for _, phrase := range c.must {
				if !strings.Contains(got, phrase) {
					t.Errorf("少了 %q：%q", phrase, got)
				}
			}
			for _, phrase := range c.forbid {
				if strings.Contains(got, phrase) {
					t.Errorf("不得出现 %q：%q", phrase, got)
				}
			}
		})
	}
}

// TestPlayAISummaryLineCarriesNoAccountText：给人看那句不许带上账号侧的串。
//
// 这一族的响应本身**不含**手机号（实拍 `user/info` / `task/list` 只有活动侧字段），
// 但建会话那一枪的回显里有 `mobile`，而这一句会被抄进 `runs.message` 与日志
// （`docs/sanitize-guide.md`：手机号一律掩码且不带尾号）。
func TestPlayAISummaryLineCarriesNoAccountText(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("2")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	var seen []string
	run := playAIRun(script, nil, func(_ core.LogLevel, message string) {
		seen = append(seen, message)
	})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	for _, text := range append(seen, res.SummaryLine(), *res.RunReport().Summary) {
		for _, forbidden := range []string{"mobile", "1990", "19****", "Authorization", "Basic "} {
			if strings.Contains(text, forbidden) {
				t.Errorf("给人看那句话里出现了 %q：%q", forbidden, text)
			}
		}
	}
}

// TestPlayAIRunReportMapsBothStatuses：两档 → 两种 [core.RunReport]。
//
// 只读那一半**不产待办**（`ManualSteps` 必须是空 slice 而不是 nil：那是出网形状契约）。
func TestPlayAIRunReportMapsBothStatuses(t *testing.T) {
	ok := PlayAIResult{Status: PlayAIReadDone, Board: PlayAIBoard{
		Tasks: []upstream.PlayAIUserTask{{State: strPtr("WAIT")}},
	}}
	report := ok.RunReport()
	if report.Outcome != core.OutcomeSuccess {
		t.Errorf("成功档应是 OutcomeSuccess，实得 %v", report.Outcome)
	}
	if report.ManualSteps == nil {
		t.Error("ManualSteps 必须是空 slice 而不是 nil（出网形状契约）")
	}
	if report.Summary == nil || *report.Summary != ok.SummaryLine() {
		t.Errorf("报告里那句该与 SummaryLine 同源，实得 %v", report.Summary)
	}
	failed := PlayAIResult{Status: PlayAIReadFailed, Error: "网络不通"}.RunReport()
	if failed.Outcome != core.OutcomeFailed {
		t.Errorf("失败档应是 OutcomeFailed，实得 %v", failed.Outcome)
	}
	if failed.Summary == nil || !strings.Contains(*failed.Summary, "网络不通") {
		t.Errorf("失败原因要进小结，实得 %v", failed.Summary)
	}
}

// TestPlayAIRunEmitsProgress：成功一句 Info、失败一句 Warn，且与报告同一句结论。
//
// 漏了失败那一句的表现是"界面上这条任务的日志里只有一句成功" —— 因为
// 终局事件与过程话各走一条路，只有这里能对得上。
func TestPlayAIRunEmitsProgress(t *testing.T) {
	okScript := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("1")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	var seen []string
	run := playAIRun(okScript, nil, func(level core.LogLevel, message string) {
		seen = append(seen, level.String()+"|"+message)
	})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if len(seen) != 1 || !strings.HasPrefix(seen[0], core.LevelInfo.String()+"|") {
		t.Fatalf("成功该恰好一句 Info，实得 %v", seen)
	}
	if !strings.Contains(seen[0], res.SummaryLine()) {
		t.Errorf("过程话与结论句该是同一句：\n过程 %q\n结论 %q", seen[0], res.SummaryLine())
	}

	failScript := playAIRunScript(liveTaskRoute{playAIStatusEndpoint, []string{playAIReadFailBody}})
	seen = nil
	run = playAIRun(failScript, nil, func(level core.LogLevel, message string) {
		seen = append(seen, level.String()+"|"+message)
	})
	if _, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("这一档应失败")
	}
	if len(seen) != 1 || !strings.HasPrefix(seen[0], core.LevelWarn.String()+"|") {
		t.Fatalf("失败该恰好一句 Warn，实得 %v", seen)
	}
}

// TestPlayAIRunFailsBeforeAnyRequestWhenCredentialMissing：库里没凭据 ⇒ 一枪都不打。
//
// 这条挡的是"错误被折成一份空读数"：空读数在界面上与"这个号没参加活动"完全一样。
func TestPlayAIRunFailsBeforeAnyRequestWhenCredentialMissing(t *testing.T) {
	script := playAIRunScript(
		liveTaskRoute{playAIStatusEndpoint, []string{playAIStatusBody("0")}},
		liveTaskRoute{playAITaskEndpoint, []string{playAITaskBody}},
	)
	run := playAIRunStore(&poolSignInStore{fakeStore{state: nil}}, script)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("库里没凭据必须失败")
	}
	if res.Status != PlayAIReadFailed {
		t.Errorf("档位应是失败，实得 %v", res.Status)
	}
	if n := script.count("mcloud.139.com"); n != 0 {
		t.Errorf("拿不到凭据就不该发任何请求，实得 %d 条", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具助手
// ─────────────────────────────────────────────────────────────────────────────

// strPtr 只给测试用：任务表那些格子全是指针，字面量取不了地址。
func strPtr(text string) *string { return &text }

// requestIndex 返回第一个命中任一子串的请求序号（没命中给 -1，比较时自然排在最前 ⇒
// 调用方要么两条都命中，要么用例自己先 Fatal）。
func requestIndex(script *liveTaskScript, substrings ...string) int {
	for i, request := range script.snapshot() {
		for _, needle := range substrings {
			if strings.Contains(request.URL, needle) {
				return i
			}
		}
	}
	return -1
}

// requestsTo 取出命中任一子串的全部请求（保持发出顺序），用来逐条查头。
func requestsTo(script *liveTaskScript, substrings ...string) []core.HTTPRequest {
	var out []core.HTTPRequest
	for _, request := range script.snapshot() {
		for _, needle := range substrings {
			if strings.Contains(request.URL, needle) {
				out = append(out, request)
				break
			}
		}
	}
	return out
}
