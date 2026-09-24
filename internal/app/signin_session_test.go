package app

// [SignInSessionSource] 的测试：它管的只有两件事 ——
// **这一个作用域里建几条会话**、**`90001` 时刷几条重放几次**。
//
// 作用域由**谁持有它**决定：直接从池里拿是"这一页那一份"（跨轮复用，
// 判据测在 `signin_session_pool_test.go`），现造一条是"这一轮那一份"（本文件的用例）。
//
// 全部走注入的 [scriptedUpstream]（见 `signin_test.go`），不联网。
// 断言优先数**上游请求**（`tyrzLogin` 打了几次、每条 `infoV3` 带的是哪个 `jwttoken`），
// 其次才读 [SignInSessionSource.Opens] —— 计数器和真实请求序列不一致时，
// 响的是前者，而"同一页用同一个 `jwttoken`"这条口径最终是由后者来担保的。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 脚本片段
// ─────────────────────────────────────────────────────────────────────────────

// ssoOK / tsOK / jwtOK 是建会话那三枪的成功响应。
//
// 每次建会话都换一个 SSO token —— 实拍里它就是短命的、一换一个新串，
// referer 里带的也是它。刻意让**两条会话的两个字段都不同**：
// 若只换 jwt 而 referer 相同，"换票不换 referer"这条错误路径就测不出来。
func ssoOK(token string) string {
	return `{"success":true,"code":"0000","message":"请求成功",` +
		`"data":{"token":"` + token + `","resultCode":"104000"}}`
}

const tsOK = `{"code":0,"msg":"success","result":1789800128813}`

func jwtOK(jwt string) string {
	return `{"code":0,"msg":"success","result":{"token":"` + jwt + `","mobile":"199****0001"}}`
}

// notLoggedIn 是缺 `jwttoken` 头或票已过期时上游回的 `90001`（实拍见
// `docs/capture/app-signin-samples.txt`）。它是这一族测试唯一的触发器。
const notLoggedIn = `{"code":90001,"msg":"未登录"}`

// ─────────────────────────────────────────────────────────────────────────────
// 助手
// ─────────────────────────────────────────────────────────────────────────────

func newTestSource(client *scriptedUpstream) (*upstream.Upstream, *SignInSessionSource) {
	up := upstream.NewUpstream(client)
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}
	return up, NewSignInSessionSource(up, testProfile(), &cred, testClock(), testRng())
}

// h5Tokens 按发出顺序返回每条命中 `urlSubstr` 的请求所带的 `jwttoken`。
func h5Tokens(client *scriptedUpstream, urlSubstr string) []string {
	var out []string
	for _, r := range client.requests {
		if !strings.Contains(r.URL, urlSubstr) {
			continue
		}
		for _, h := range r.Headers {
			if strings.EqualFold(h[0], "jwttoken") {
				out = append(out, h[1])
			}
		}
	}
	return out
}

// queryStatus 是"一条真正带 `jwttoken` 的业务请求"的最小代理：
// 用真的 [upstream.Upstream.AppSignInStatus] 而不是手写错误，
// 这样 90001 → [core.Error.IsSessionExpired] 那条**分类链**也在被测范围内 ——
// 换成手工构造错误的话，`EnsureOKWithHint` 里那个码到变体的映射坏了也照样绿。
func queryStatus(up *upstream.Upstream) func(SignInSession) *core.Error {
	return func(s SignInSession) *core.Error {
		_, err := up.AppSignInStatus(context.Background(), s.jwt, s.referer)
		return err
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口径 1：一轮任务用同一个 jwttoken
// ─────────────────────────────────────────────────────────────────────────────

func TestSourceBuildsExactlyOneSessionForTheRound(t *testing.T) {
	client := &scriptedUpstream{responses: []string{ssoOK("SSO-1"), tsOK, jwtOK("JWT-1")}}
	_, src := newTestSource(client)

	for i := 0; i < 3; i++ {
		var got SignInSession
		if err := src.Call(context.Background(), func(s SignInSession) *core.Error {
			got = s
			return nil
		}); err != nil {
			t.Fatalf("第 %d 次调用失败: %v", i+1, err)
		}
		if got.jwt != "JWT-1" || !strings.Contains(got.referer, "SSO-1") {
			t.Fatalf("第 %d 次拿到的不是这一轮那一条会话: %+v", i+1, got.Describe())
		}
	}
	// 反向改坏点：把 `Session` 里的 `cached()` 短路去掉 ⇒ 这两个计数会变 3。
	if src.Opens() != 1 {
		t.Errorf("一轮应只建 1 条会话，实得 %d", src.Opens())
	}
	if n := client.count("tyrzLogin"); n != 1 {
		t.Errorf("tyrzLogin 应只打 1 次，实得 %d 次", n)
	}
}

// TestSessionDescribeCarriesNoCredential：`jwt` 是可直接复用的凭据，
// 而 Go 的 `%v` 会连未导出字段一起打 —— 所以这里既不能出现票文，也不能出现 referer 全文。
func TestSessionDescribeCarriesNoCredential(t *testing.T) {
	s := SignInSession{referer: "https://x", jwt: "JWT-SECRET-VALUE"}
	line := s.Describe()
	if strings.Contains(line, "JWT-SECRET-VALUE") || strings.Contains(line, "https://x") {
		t.Errorf("Describe 不该带出凭据原文: %s", line)
	}
	// 两个长度都钉死：只给一个"字符"就松到看不出 referer 和票会不会打串。
	if !strings.Contains(line, "9 字符") || !strings.Contains(line, "16 字符") {
		t.Errorf("Describe 该按 referer=9 / jwttoken=16 给长度，实得 %s", line)
	}
}

// TestRoundSharesOneSessionAcrossCapabilities 把口径 1 钉在**能力层**：
// 一轮里连着跑"签到 + 领取"（读 + 写、跨两个能力函数），只建一条会话、
// 每条业务请求带同一张票。
//
// 上面那条 [TestSourceBuildsExactlyOneSessionForTheRound] 只喂了一个裸闭包，
// 证明的是 `Session` 的缓存短路；这一条才证明**改接之后**没有哪个能力函数
// 偷偷自己去开一条新会话 —— 那正是这次重构要消灭的东西（12 个各开各的调用点）。
func TestRoundSharesOneSessionAcrossCapabilities(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		infoV3(false), startSignInOK, infoV3(true), // CloudSignIn：查 → 签 → 回查
		statusPayload(`"cal":[],"receiveList":[{"cloudType":1,"cloudNum":100}]`),
		`{"code":0,"msg":"success","result":{"cloudCount":100}}`, // ReceiveAll：查 → 领
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}
	src := signinSource(client, &cred)
	ctx := context.Background()

	if _, perr := CloudSignIn(ctx, src, SignInOptions{}); perr != nil {
		t.Fatalf("签到失败: %v", perr)
	}
	if _, perr := ReceiveAll(ctx, src, "DEV"); perr != nil {
		t.Fatalf("领取失败: %v", perr)
	}
	if src.Opens() != 1 {
		t.Errorf("一轮两能力应只建 1 条会话，实得 %d", src.Opens())
	}
	if n := client.count("tyrzLogin"); n != 1 {
		t.Errorf("tyrzLogin 应只打 1 次，实得 %d 次", n)
	}
	// 反向改坏点：`h5Tokens(client, "")` 数的是**所有**带 `jwttoken` 头的请求
	// （`strings.Contains(url, "")` 恒真）。若把 `SignInOptions{}` 那类改造漏掉一个、
	// 让它自己再开一条会话，这里会多出第二张票；若业务请求改带别的头名，
	// 计数会掉到 0 而"全等于 JWT-1"照样成立 —— 所以**先钉条数**。
	tokens := h5Tokens(client, "")
	if len(tokens) != 5 {
		// 5 = 签到那三次（查 → 签 → 回查）+ 领取那两次（查 → 领）。
		t.Fatalf("这一轮应有 5 条带 jwttoken 的业务请求，实得 %d 条：%v", len(tokens), tokens)
	}
	for i, token := range tokens {
		if token != "JWT-1" {
			t.Errorf("第 %d 条业务请求带的不是这一轮那张票：%v", i+1, tokens)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口径 2：只有 `90001` 才刷新
// ─────────────────────────────────────────────────────────────────────────────

func TestOtherFailuresDoNotTriggerARefresh(t *testing.T) {
	client := &scriptedUpstream{responses: []string{ssoOK("SSO-1"), tsOK, jwtOK("JWT-1")}}
	_, src := newTestSource(client)

	for _, tc := range []struct{ name, msg string }{
		{"参数错（615 那一类）", "响应参数没带（615）"},
		{"限流（11029 那一类）", "活动太火爆，稍后再试（11029）"},
		{"解析失败", "infoV3 响应没有 result"},
	} {
		want := core.NewBadRequest(tc.msg)
		if tc.name == "解析失败" {
			want = core.NewParse(tc.msg)
		}
		err := src.Call(context.Background(), func(SignInSession) *core.Error { return want })
		if err == nil || err.Message() != tc.msg {
			t.Errorf("%s：错误被改写了（多刷了一次就是重放）: %v", tc.name, err)
		}
		if err.IsSessionExpired() {
			t.Errorf("%s：不该判成会话过期 ⇒ %s", tc.name, err)
		}
	}
	// 反向改坏点：`Call` 里去掉 `!first.IsSessionExpired()` 那道门 ⇒ 下面两条变 3 / 3+。
	if src.Opens() != 1 {
		t.Errorf("非会话过期不该刷新，实得建了 %d 条", src.Opens())
	}
	if n := client.count("tyrzLogin"); n != 1 {
		t.Errorf("三次失败请求都不该重开，tyrzLogin 实得 %d 次", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口径 3：`90001` ⇒ 刷一次 ⇒ 用新票重放一次
// ─────────────────────────────────────────────────────────────────────────────

func TestNotLoggedInRefreshesOnceAndReplaysWithTheNewToken(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		notLoggedIn, // 第一条票已过期
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
		infoV3(true), // 重放：换了票就过了
	}}
	up, src := newTestSource(client)

	if err := src.Call(context.Background(), queryStatus(up)); err != nil {
		t.Fatalf("重放应成功，实得: %v", err)
	}
	if src.Opens() != 2 {
		t.Errorf("过期后应重建 1 条（合计 2），实得 %d", src.Opens())
	}
	if n := client.count("tyrzLogin"); n != 2 {
		t.Errorf("tyrzLogin 应打 2 次，实得 %d", n)
	}
	// 关键：重放那条**必须**带新票。两条 referer 也都是新的（同源那条约束）。
	if got := h5Tokens(client, "/infoV3"); len(got) != 2 || got[0] != "JWT-1" || got[1] != "JWT-2" {
		t.Errorf("两条 infoV3 带的票应是 JWT-1 → JWT-2，实得 %v", got)
	}
}

func TestReplayFailureIsReportedAsStillFailing(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		notLoggedIn,
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
		notLoggedIn, // 换了票还是不认 ⇒ 这一轮到此为止
	}}
	up, src := newTestSource(client)

	err := src.Call(context.Background(), queryStatus(up))
	if err == nil {
		t.Fatal("重放仍失败时必须返回错误")
	}
	if !strings.Contains(err.Message(), "jwttoken 刷新后重放仍失败") {
		t.Errorf("错误里要点明「重放仍失败」，否则与「第一次就失败」分不清: %s", err)
	}
	// 反向改坏点：把重放写成循环（`for` 而不是再来一次）⇒ 这条会红。
	if n := client.count("tyrzLogin"); n != 2 {
		t.Errorf("只许刷一次，tyrzLogin 实得 %d 次", n)
	}
	if n := client.count("/infoV3"); n != 2 {
		t.Errorf("只许重放一次，infoV3 实得 %d 次", n)
	}
}

func TestRefreshFailureTellsTheUserTheTokenFailed(t *testing.T) {
	// 建第二条会话时第一枪就没响应（脚本空了）—— 等价于"刷新失败"。
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		notLoggedIn,
	}}
	up, src := newTestSource(client)

	err := src.Call(context.Background(), queryStatus(up))
	if err == nil {
		t.Fatal("刷新失败时必须返回错误")
	}
	if !strings.Contains(err.Message(), "jwttoken 刷新失败") {
		t.Errorf("要按 jwttoken 失败通知用户，实得: %s", err)
	}
	if !err.IsSessionExpired() {
		t.Errorf("刷新失败仍是会话问题（调用方据此判这条账号要重新登录）: %s", err)
	}
	if n := client.count("tyrzLogin"); n != 1 {
		t.Errorf("刷新失败不该重试第二遍，tyrzLogin 实得 %d 次", n)
	}
}

// TestRefreshIsRoundWideNotPerCall 钉住"刷新发生在**源**上"这件事：
// 一轮中途某条请求撞 `90001` ⇒ 换新票 ⇒ **后面那几个能力**也拿新票，而不是各自
// 抱着自己第一次拿到的那张旧票。
//
// 反向改坏点：如果 `Call` 把新会话只用在重放那一条、不写回 `s.session`，
// 后面 `ReceiveAll` 会再撞一次 `90001` ⇒ `tyrzLogin` 变 3 次、`/infoV3` 的票序列变短。
func TestRefreshIsRoundWideNotPerCall(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		infoV3(false),
		notLoggedIn, // 写请求这一枪撞上过期
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
		startSignInOK,
		infoV3(true), // CloudSignIn 的回查：新票
		statusPayload(`"cal":[],"receiveList":[{"cloudType":1,"cloudNum":100}]`),
		`{"code":0,"msg":"success","result":{"cloudCount":100}}`,
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}
	src := signinSource(client, &cred)
	ctx := context.Background()

	if _, perr := CloudSignIn(ctx, src, SignInOptions{}); perr != nil {
		t.Fatalf("过期后刷新重放应让签到成功，实得: %v", perr)
	}
	if _, perr := ReceiveAll(ctx, src, "DEV"); perr != nil {
		t.Fatalf("后半轮应直接用新票跑完，实得: %v", perr)
	}
	if n := client.count("tyrzLogin"); n != 2 {
		t.Errorf("整轮只该因这次过期重建 1 条（合计 2），实得 %d 次", n)
	}
	if got := h5Tokens(client, "/startSignIn"); len(got) != 2 || got[0] != "JWT-1" || got[1] != "JWT-2" {
		t.Errorf("startSignIn 应先旧票后新票，实得 %v", got)
	}
	if got := h5Tokens(client, "/infoV3"); len(got) != 3 ||
		got[0] != "JWT-1" || got[1] != "JWT-2" || got[2] != "JWT-2" {
		t.Errorf("刷新之后各能力的 infoV3 都该带 JWT-2，实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口径 3 的另一半：六步链**中途**过期
// ─────────────────────────────────────────────────────────────────────────────

// TestRedFlowerChainRefreshesAndReplaysTheFailedStep 钉住 [RunRedFlower] 里那层
// [SignInSessionSource.Call] 真的生效：第①步撞上 `90001` ⇒ 刷一次 ⇒ **重放这一步**，
// 链条从那里正常跑完，而不是整条失败。
//
// 为什么值得单列一条（`live_room_test.go` 那批用例都预置了现成的票，测不到这里）：
// 这条链的"可重放"依赖 [RedFlowerSession.Next] **不推进状态** 这个设计。
// 两边各改一半时，表现是"①跑了两次、②③各跑一次"——余额会显示两笔起始读数，
// 而没有任何一条错误说出来。只有把请求序列钉住才抓得住。
func TestRedFlowerChainRefreshesAndReplaysTheFailedStep(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		notLoggedIn,                          // ① 起始余额：这一枪带着旧票
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"), // 刷新：建会话那三枪
		liveUserInfoBody(235), // ① 重放
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody, // ② 埋点 ×5
		liveGiveNoQualificationBody, // ③ 首次参与
		liveDrawOKBody(5),           // ④ 口令
		liveRevivalBody,             // ⑤ 复活花
		liveUserInfoBody(240),       // ⑥ 终值
	}}
	up := upstream.NewUpstream(client)
	src := flowerSource(up, "JWT-OLD") // 预置一张"已经过期"的票

	report, err := RunRedFlower(context.Background(), src, NewRedFlowerSession([]string{"AAA"}))
	if err != nil {
		t.Fatalf("过期后应刷新并重放这一步，实得错误：%s", err.Message())
	}
	if report.Gained() != 5 || report.BalanceAfter.FlowerCnt != 240 {
		t.Errorf("链应正常跑完，实得进账 %d / 终值 %d", report.Gained(), report.BalanceAfter.FlowerCnt)
	}
	if src.Opens() != 1 {
		t.Errorf("一次过期只该重建 1 条，实得 %d", src.Opens())
	}
	// ①⑥ 共用一个端点：靠"第 1 条带旧票、后两条带新票"同时钉住**重放**与**不重复推进**。
	if got := h5Tokens(client, "/liveRoomFeedback/user/info"); len(got) != 3 ||
		got[0] != "JWT-OLD" || got[1] != "JWT-2" || got[2] != "JWT-2" {
		t.Errorf("user/info 三次的票应是 JWT-OLD → JWT-2 → JWT-2，实得 %v", got)
	}
	// 反向改坏点：`Next` 若顺手推进状态，①重放会变成②的活 ⇒ draw 少发或 user/info 少一次。
	if got := h5Tokens(client, "/redFlower/draw"); len(got) != 1 || got[0] != "JWT-2" {
		t.Errorf("口令应只投一次、且带刷新后的新票，实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 并发那一段：后进场的人复用别人建好的那条，而不是再建一条
// ─────────────────────────────────────────────────────────────────────────────

func TestRefreshReusesWhatSomeoneElseAlreadyBuilt(t *testing.T) {
	client := &scriptedUpstream{}
	_, src := newTestSource(client)

	stale := SignInSession{referer: "r-old", jwt: "JWT-OLD"}
	fresh := SignInSession{referer: "r-new", jwt: "JWT-NEW"}
	src.session = &fresh // 模拟"另一个 goroutine 刚刷完"

	got, err := src.Refresh(context.Background(), stale)
	if err != nil {
		t.Fatalf("应复用现成的新会话，实得错误: %v", err)
	}
	if !got.same(fresh) {
		t.Errorf("应拿到别人建好的那条，实得 %s", got.Describe())
	}
	// 反向改坏点：去掉 `!current.same(stale)` 那道判断 ⇒ 这里会多打三枪。
	if src.Opens() != 0 || len(client.requests) != 0 {
		t.Errorf("既然已有新会话就不该再建：opens=%d requests=%d", src.Opens(), len(client.requests))
	}
}

func TestRefreshDropsOnlyTheSessionThatFailed(t *testing.T) {
	s := &SignInSessionSource{}
	current := SignInSession{referer: "r-a", jwt: "JWT-A"}
	s.session = &current

	s.drop(SignInSession{referer: "r-b", jwt: "JWT-B"}) // 不是这一条
	if _, ok := s.cached(); !ok {
		t.Error("drop 只该清掉失败的那一条，别人刚建的不能一起丢")
	}
	s.drop(current)
	if _, ok := s.cached(); ok {
		t.Error("失败的那一条应被清掉")
	}
}
