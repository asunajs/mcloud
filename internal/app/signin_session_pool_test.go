package app

// [SessionPool] 的测试：**一张票属于哪一个作用域**，以及**它什么时候被换掉**。
//
// 这批用例钉的四件事，逐条对应用户 2026-09-23 说的那个使用场景
// （原话口径见 `signin_session_pool.go` 文件头）：
//
//  1. 同一账号、同一页面 ⇒ 跨轮只有一条会话（`tyrzLogin` 一次），连点三次签到不重复取票；
//  2. 两个页面各持一条，**互不挤掉**（"去点小红花、再回头领云朵，带的还是签到页那张"）；
//  3. 某一页撞 `90001` ⇒ 只刷它那一条，别页那张票不许被连带重建；
//  4. 外圈登录凭据轮换 ⇒ 名下旧条目整体作废（它们再也刷不出票）。
//
// ⚠ 全部走注入的假上游（前一节用顺序回放的 [scriptedUpstream]，
// 末尾"接线"那一节用按端点分派的 [liveTaskScript]），不联网；
// 断言优先数**上游请求与请求头上的票**，
// 其次才读 [SessionPool.Entries] 与 [SignInSessionSource.Opens] ——
// 计数对了而票发错，只有前者抓得住。
//
// # 为什么这里要造"第二个配方"
//
// 生产代码今天只有 [RecipeSignInPage] 一个取值（小红花那条链借用它，见
// `AGENTS.md` §1.12 与 `live_room_task.go` 的"使用公共来源"标注）。
// 下面用 [secondRecipe] 是把"将来抓到特有取法之后"的结构**先钉住**：
// 那时要改的是"多一个常量 + `OpenSignInSession` 按配方取参"，
// 而池的分键、互不挤掉、按配方各自刷新这三条必须**已经**成立 —— 否则那次改动会顺手改到池。

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// secondRecipe 是"另一个页面按自己的参数换票"的占位配方（理由见文件头那一段）。
//
// 刻意**不**取名叫 `RecipeRedFlowerPage`：那个取值要等抓包证明小红花页确实有特有取法
// 才该出现，测试里先占掉这个名字会让它看起来"已经有了"。
const secondRecipe SessionRecipe = "recipe-under-test"

func poolCredential(account, authorization string) *SignInCredential {
	return &SignInCredential{
		authorization: authorization,
		accountID:     core.NewAccountID(account),
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 1：同账号同页面 ⇒ 一条会话，连点三次不重复取票
// ─────────────────────────────────────────────────────────────────────────────

func TestPoolHandsOutOneSourcePerAccountAndRecipe(t *testing.T) {
	client := &scriptedUpstream{responses: []string{ssoOK("SSO-1"), tsOK, jwtOK("JWT-1")}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	cred := poolCredential("19****01", "Basic x")

	var first *SignInSessionSource
	for i := 0; i < 3; i++ {
		src := pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
		if i == 0 {
			first = src
			continue
		}
		// 反向改坏点：`Source` 里去掉 `p.items[key]` 那道命中 ⇒ 这里拿到三个不同实例。
		if src != first {
			t.Fatalf("第 %d 次取到的是另一条会话源（票池没命中键）", i+1)
		}
	}
	if pool.Entries() != 1 {
		t.Errorf("同一 (账号,配方,凭据) 只该存 1 条，实得 %d", pool.Entries())
	}
	// ⚠ 取票**不许出网**：建会话是懒的。这条是整个池能省枪的前提 ——
	// 若 `Source` 自己去建会话，下面那条零请求会变成"每次点开这一屏都打三枪"。
	if n := len(client.requests); n != 0 {
		t.Errorf("只是取会话源就该零请求，实得 %d 条", n)
	}
}

// TestTwoRoundsOnOnePageShareOneToken ⭐ 就是"WebUI 连点两次签到"那一格：
// 两轮各自从池里取源、各自跑一次真能力（[CloudSignIn]），
// 而整场只打一次 `tyrzLogin`、两条业务请求带同一张票。
//
// 与 `signin_session_test.go` 那批"一轮内共享"的用例的区别在这一句：
// 那里跨的是**同一轮里的多个能力**，这里跨的是**两轮** —— 后者才是这层新增的意义，
// 也是漏接池时唯一会响的那一格（漏接的表现是 `tyrzLogin` 变 2 次，界面上毫无异常）。
func TestTwoRoundsOnOnePageShareOneToken(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		infoV3(false), startSignInOK, infoV3(true), // 第一轮：查 → 签 → 回查
		infoV3(true), // 第二轮：查一句"今天已签"就完事（不发写请求）
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	cred := poolCredential("19****01", "Basic x")
	ctx := context.Background()

	for round := 1; round <= 2; round++ {
		src := pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
		if _, perr := CloudSignIn(ctx, src, SignInOptions{}); perr != nil {
			t.Fatalf("第 %d 轮签到失败：%v", round, perr)
		}
	}
	if n := client.count("tyrzLogin"); n != 1 {
		t.Errorf("两轮该共用 1 条会话，tyrzLogin 实得 %d 次", n)
	}
	// 4 = 第一轮 3 条（查 → 签 → 回查）+ 第二轮 1 条（只查，已签就不再写）。
	tokens := h5Tokens(client, "")
	if len(tokens) != 4 {
		t.Fatalf("应有 4 条带 jwttoken 的业务请求，实得 %d 条：%v", len(tokens), tokens)
	}
	for i, token := range tokens {
		if token != "JWT-1" {
			t.Errorf("第 %d 条业务请求带的不是那一张票：%s", i+1, token)
		}
	}
	if pool.Entries() != 1 {
		t.Errorf("两轮过后池里仍该只有 1 条，实得 %d", pool.Entries())
	}
}

// TestSessionSourceFallsBackToRoundLevelWithoutPool 钉住"没接池"那一支仍然是**轮级**：
// 两次取用各自一条。CLI 一次调用对每个账号只跑一轮，靠的就是这个回落
// （接线处的注释见 `internal/cli/run_signin.go`）。
func TestSessionSourceFallsBackToRoundLevelWithoutPool(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
	}}
	up := upstream.NewUpstream(client)
	cred := poolCredential("19****01", "Basic x")

	first := sessionSource(nil, up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
	second := sessionSource(nil, up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
	if first == second {
		t.Fatal("回落那支该每次新装一条（否则 CLI 的两轮会共用一条，与成文口径不符）")
	}
	if _, perr := first.Session(context.Background()); perr != nil {
		t.Fatalf("第一条建会话失败：%v", perr)
	}
	if _, perr := second.Session(context.Background()); perr != nil {
		t.Fatalf("第二条建会话失败：%v", perr)
	}
	// 反向改坏点：把 `sessionSource` 的 `pool != nil` 判反 ⇒ 这里只会有 1 次 tyrzLogin。
	if n := client.count("tyrzLogin"); n != 2 {
		t.Errorf("无池时两轮各建一条，tyrzLogin 实得 %d 次", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 2：两个配方互不挤掉
// ─────────────────────────────────────────────────────────────────────────────

// TestSecondRecipeDoesNotEvictTheFirstToken 钉住用户那句"去点小红花、再回头领云朵，
// 带的还是签到页那张"：另一条会话的建立**不许**把这一页的票顶掉或换掉。
//
// 这一条是防"把池写成一条槽位"（`map[账号]源` 或干脆一个字段）的回归测试 ——
// 那种写法在"今天只有一个配方"下**完全跑得通**，连点三次签到也只建一条，
// 于是判据 1 那两条会一起绿着；等抓包新增配方那天，它表现为
// "签到与小红花互相把对方的票踢掉"，每次切换页面多打三枪。
func TestSecondRecipeDoesNotEvictTheFirstToken(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		// ⚠ 脚本按**实际发出顺序**排：建会话是懒的，所以每一页那三枪紧挨在它自己
		// 那一枪业务请求前面，不是"先把两条会话都建好再各打一枪"。
		ssoOK("SSO-A1"), tsOK, jwtOK("JWT-A"), infoV3(true), // 签到页第一枪
		ssoOK("SSO-B1"), tsOK, jwtOK("JWT-B"), infoV3(true), // 另一页第一枪
		infoV3(true), // 回头那一枪：池命中 ⇒ 不再建会话，直接带 JWT-A
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	cred := poolCredential("19****01", "Basic x")
	ctx := context.Background()

	first := pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
	if err := first.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("签到页那一枪失败：%v", err)
	}
	other := pool.Source(up, testProfile(), cred, testClock(), testRng(), secondRecipe)
	if other == first {
		t.Fatal("不同配方拿到了同一条会话源 ⇒ 两页会共用一张票，与「两个配方各持一条」矛盾")
	}
	if err := other.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("另一页那一枪失败：%v", err)
	}
	// ⭐ 回头再取签到页：必须是**同一个实例**、带着**原来那张票**。
	again := pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
	if again != first {
		t.Fatal("另一页建过会话之后，签到页这条被换掉了")
	}
	if err := again.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("回头那一枪失败：%v", err)
	}
	if got := h5Tokens(client, "/infoV3"); len(got) != 3 ||
		got[0] != "JWT-A" || got[1] != "JWT-B" || got[2] != "JWT-A" {
		t.Errorf("三枪带的票应是 JWT-A → JWT-B → JWT-A，实得 %v", got)
	}
	if n := client.count("tyrzLogin"); n != 2 {
		t.Errorf("两条会话各建一次即够，tyrzLogin 实得 %d 次", n)
	}
	if pool.Entries() != 2 {
		t.Errorf("池里该有 2 条（两配方各一），实得 %d", pool.Entries())
	}
	if again.Opens() != 1 {
		t.Errorf("签到页那条只该建 1 次会话，实得 %d", again.Opens())
	}
}

// TestPoolIsolatesAccounts 钉住键里那一位账号：两个号不该共用一条会话。
//
// 后果不是省不省枪：共用一条意味着**甲的写请求带着乙的票**，
// 而那条票是拿乙的登录凭据换的（签到会记到乙头上）。
func TestPoolIsolatesAccounts(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()

	a := pool.Source(up, testProfile(), poolCredential("19****01", "Basic x"),
		testClock(), testRng(), RecipeSignInPage)
	b := pool.Source(up, testProfile(), poolCredential("19****02", "Basic y"),
		testClock(), testRng(), RecipeSignInPage)
	if a == b {
		t.Fatal("两个账号共用了同一条会话源")
	}
	sessA, perr := a.Session(context.Background())
	if perr != nil {
		t.Fatalf("甲建会话失败：%v", perr)
	}
	sessB, perr := b.Session(context.Background())
	if perr != nil {
		t.Fatalf("乙建会话失败：%v", perr)
	}
	// referer 里带着本次换来的 SSO token（两条各用各的），所以它是"这两条真是各建各的"
	// 最直接的读数；票也一样 —— 甲的写请求带着乙的票会记到乙头上。
	if sessA.referer == sessB.referer || sessA.jwt == sessB.jwt {
		t.Errorf("两个账号拿到了同一条会话：referer/jwt 有相同项")
	}
	if pool.Entries() != 2 {
		t.Errorf("两个账号该各存 1 条，实得 %d", pool.Entries())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 3：撞 `90001` 只刷它那一条
// ─────────────────────────────────────────────────────────────────────────────

// TestExpiredTokenRebuildsOnlyItsOwnRecipe 钉住"刷新按配方各自发生"：
// 签到页那条过期 ⇒ 只重建签到页 ⇒ 另一页那张票**不动**，也不得多打一次 `tyrzLogin`。
//
// ⚠ 这里必须由池持有那两条，而不是让两条源各自独立存在：刷新走的是
// [SignInSessionSource] 自己的 `session` 字段，池不参与票的更换 ——
// 所以这条测的是"池不会把两页的票并成一条"，也就是判据 2 的另一面。
func TestExpiredTokenRebuildsOnlyItsOwnRecipe(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		// 顺序同上一条：每一页那三枪紧挨在它自己的业务请求前面。
		ssoOK("SSO-A1"), tsOK, jwtOK("JWT-A"), infoV3(true), // 甲第一枪
		ssoOK("SSO-B1"), tsOK, jwtOK("JWT-B"), infoV3(true), // 乙第一枪
		notLoggedIn,                            // 甲第二枪：票过期了
		ssoOK("SSO-A2"), tsOK, jwtOK("JWT-A2"), // 只有甲这一条重建
		infoV3(true), // 甲的重放
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	cred := poolCredential("19****01", "Basic x")
	ctx := context.Background()

	pageA := pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
	pageB := pool.Source(up, testProfile(), cred, testClock(), testRng(), secondRecipe)
	if err := pageA.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("甲第一枪失败：%v", err)
	}
	if err := pageB.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("乙第一枪失败：%v", err)
	}
	if err := pageA.Call(ctx, queryStatus(up)); err != nil {
		t.Fatalf("甲过期后应刷新并重放成功：%v", err)
	}

	if got := h5Tokens(client, "/infoV3"); len(got) != 4 ||
		got[0] != "JWT-A" || got[1] != "JWT-B" || got[2] != "JWT-A" || got[3] != "JWT-A2" {
		t.Errorf("四条业务请求的票应是 A、B、A（过期）、A2（重放），实得 %v", got)
	}
	// ⭐ 乙没被连带刷新。
	if pageB.Opens() != 1 {
		t.Errorf("另一页不该因为甲过期而重建，实得建了 %d 条", pageB.Opens())
	}
	if pool.Entries() != 2 {
		t.Errorf("刷新不该往池里加条目，实得 %d 条", pool.Entries())
	}
	// 4 次 tyrzLogin 里甲占 2（初次 + 过期重建）、乙占 1 ⇒ 合计 3。
	if n := client.count("tyrzLogin"); n != 3 {
		t.Errorf("tyrzLogin 应恰好 3 次，实得 %d", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 4：外圈登录凭据轮换 ⇒ 名下旧条目整体作废
// ─────────────────────────────────────────────────────────────────────────────

func TestRotatedCredentialDropsTheOldEntries(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
		ssoOK("SSO-3"), tsOK, jwtOK("JWT-3"),
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	oldCred := poolCredential("19****01", "Basic old")
	newCred := poolCredential("19****01", "Basic new")

	oldSignIn := pool.Source(up, testProfile(), oldCred, testClock(), testRng(), RecipeSignInPage)
	oldOther := pool.Source(up, testProfile(), oldCred, testClock(), testRng(), secondRecipe)
	if pool.Entries() != 2 {
		t.Fatalf("夹具没到位：换凭据前该有 2 条，实得 %d", pool.Entries())
	}

	// 换新凭据之后，**同一个账号**名下任何一条旧会话都刷不出票（外圈 `Authorization`
	// 已经变了），所以连配方不同的那条也要清掉。
	fresh := pool.Source(up, testProfile(), newCred, testClock(), testRng(), RecipeSignInPage)
	if fresh == oldSignIn {
		t.Fatal("换了登录凭据还复用旧会话源 ⇒ 它揣着的那份凭据再也刷不出票")
	}
	if pool.Entries() != 1 {
		t.Errorf("旧凭据名下那 2 条都该作废，实得剩 %d 条", pool.Entries())
	}
	if pool.Source(up, testProfile(), oldCred, testClock(), testRng(), secondRecipe) == oldOther {
		t.Error("旧凭据 + 另一配方也不该被复用（票池里那位已经不存在了）")
	}
}

// TestPoolKeysOnCredentialFingerprintNotItsText 钉住键里存的是指纹：
// 这个结构会被 `%v` 打进测试失败输出与日志，凭据原文进键就等于进日志。
func TestPoolKeysOnCredentialFingerprintNotItsText(t *testing.T) {
	pool := NewSessionPool()
	cred := poolCredential("19****01", wholeAuth)
	pool.Source(upstream.NewUpstream(&scriptedUpstream{}), testProfile(), cred,
		testClock(), testRng(), RecipeSignInPage)

	// 直接读键（同一个包内），而不是格式化整个池：要钉的就是"键里没有原文"。
	if pool.Entries() != 1 {
		t.Fatalf("夹具没到位：池里应有一条，实得 %d", pool.Entries())
	}
	for key := range pool.items {
		if key.authHash != authFingerprint(wholeAuth) {
			t.Errorf("键里那位不是凭据指纹：%q", key.authHash)
		}
		if strings.Contains(key.authHash, wholeAuth) {
			t.Errorf("键里漏出了登录凭据原文：%s", key.authHash)
		}
	}
	// 空凭据不是"没登录"，而是"这一路压根没带凭据"（测试夹具常见）。
	// 不特判的话两个各自没带凭据的账号会共用一条会话 —— 上面那条账号隔离测试
	// 只在**都带了**凭据时才成立，所以这里单独钉一格。
	if authFingerprint("") != "<empty>" {
		t.Errorf("空凭据该有独立指纹，实得 %q", authFingerprint(""))
	}
	if authFingerprint(wholeAuth) == authFingerprint(wholeAuth+"x") {
		t.Error("指纹撞了：换凭据会被读成同一条会话")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 并发：同键一起进场只装一条
// ─────────────────────────────────────────────────────────────────────────────

// TestConcurrentSourceReturnsOneInstance 是"服务端两个请求同时到"那一格：
// 调度那一趟与用户手点那一趟共用 [RunWorld] 里那一份池（见
// `internal/server/run_dispatch.go`），所以同一键可能被并发取用。
//
// ⚠ 这里断的是**指针相等**而不只是"票相同"：两条源各自缓存同一张票也能过后者，
// 而那样一来任一侧刷票都不会被另一侧看到。
func TestConcurrentSourceReturnsOneInstance(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), tsOK, jwtOK("JWT-1"),
		ssoOK("SSO-2"), tsOK, jwtOK("JWT-2"),
	}}
	up := upstream.NewUpstream(client)
	pool := NewSessionPool()
	cred := poolCredential("19****01", "Basic x")

	const n = 8
	var wg sync.WaitGroup
	sources := make([]*SignInSessionSource, n)
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			sources[i] = pool.Source(up, testProfile(), cred, testClock(), testRng(), RecipeSignInPage)
		}(i)
	}
	wg.Wait()
	for i, src := range sources {
		if src != sources[0] {
			t.Fatalf("第 %d 个 goroutine 拿到了另一条会话源", i+1)
		}
	}
	if pool.Entries() != 1 {
		t.Errorf("并发取用只该存 1 条，实得 %d", pool.Entries())
	}
	// 取用本身零请求（懒建那条约束在并发下也要成立）。
	if len(client.requests) != 0 {
		t.Errorf("并发取票不该出网，实得 %d 条请求", len(client.requests))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 接线：三条入口真的把池用上了
// ─────────────────────────────────────────────────────────────────────────────
//
// 上面那批只证明"池本身是对的"。这一节证明**调用点接上了** ——
// 那才是这次改动唯一会静默失败的地方：漏接 `Sessions` 的表现在界面上完全正常
// （票照样有效），只有请求数会涨。所以每条都带一个"不接池"的对照：
// 对照不成立 ⇒ 说明这条用例压根没走到那个分岔。

// poolSignInStore 只补 [SignInRun.RunAccount] 会碰的那四个存储入口，
// 其余仍然走 [fakeStore] 的"没重写就 panic"。
//
// `GetSmidState` 回错误是**故意的**：那样 [DeviceID] 会失败，
// 而 [SignInRun.RunAccount] 对它那条支路是"降级继续"（成文见那边的 ⚠ 注释），
// 于是本用例不必为 H5 `deviceId` 那一整条上报链编响应。
// ⚠ 别改成回一份空绑定 —— 那会走到"真去上报换一个新 id"，多出好几枪。
type poolSignInStore struct{ fakeStore }

func (s *poolSignInStore) GetSmidState(context.Context, core.Scope, core.AccountID) (*core.SmidState, error) {
	return nil, errors.New("本用例不带 deviceId")
}

func (s *poolSignInStore) CloudStatsSince(
	context.Context, core.Scope, core.AccountID, string,
) ([]core.CloudStats, error) {
	return nil, nil
}

func (s *poolSignInStore) UpsertCloudStats(context.Context, core.Scope, *core.CloudStats) error {
	return nil
}

func (s *poolSignInStore) SetAccountCloud(
	context.Context, core.Scope, core.AccountID, int64, time.Time,
) error {
	return nil
}

// poolTokens 按发出顺序取每条请求带的 `jwttoken`（[liveTaskScript] 版，
// 与 `signin_test.go` 里那个接 [scriptedUpstream] 的同名函数一回事）。
func poolTokens(script *liveTaskScript) []string {
	var out []string
	for _, request := range script.snapshot() {
		for _, pair := range request.Headers {
			if strings.EqualFold(pair[0], "jwttoken") {
				out = append(out, pair[1])
			}
		}
	}
	return out
}

// TestSignInRunTwoRoundsShareOneToken ⭐ 就是用户点名的那一格：
// 界面上连着点两次签到 ⇒ 两轮共用一张 `jwttoken`，`tyrzLogin` 只打一次。
//
// 脚本用**按端点分派**的 [liveTaskScript] 而不是顺序回放的 [scriptedUpstream]，
// 为的是让"接没接池"两档共用同一份夹具：顺序夹具在两档下消费的条数不同，
// 漏接池时先响的是"脚本越界"那句，而不是本条真正要说的话
// （`tyrzLogin` 次数）。这一层要判的就是那一个数。
func TestSignInRunTwoRoundsShareOneToken(t *testing.T) {
	// signTwice 跑两轮，返回 `tyrzLogin` 次数与所有带票的业务请求。
	signTwice := func(withPool bool) (int, []string) {
		script := liveTaskScriptWith(
			// 建会话那三枪。`tyrzLogin` 给两条不同响应：第二轮若真的又建一条，
			// 它会拿到 JWT-2 ⇒ 下面那句"票没换过"才有读数。
			liveTaskRoute{"/user/querySpecTokenV2", []string{ssoOK("SSO-1")}},
			liveTaskRoute{"/ycloud/api/timestamp", []string{tsOK}},
			liveTaskRoute{"tyrzLogin", []string{jwtOK("JWT-1"), jwtOK("JWT-2")}},
			// 第一轮：查（未签）→ 签 → 回查（已签）；第二轮：查一句已签就完事。
			liveTaskRoute{"infoV3", []string{infoV3(false), infoV3(true), infoV3(true)}},
			liveTaskRoute{"startSignIn", []string{startSignInOK}},
		)
		store := poolSignInStore{*liveTaskStore()} // 凭据 = [wholeAuth]，与直播间那批同一份
		run := SignInRun{
			Store:    &store,
			Scope:    core.ScopeLocal(),
			Upstream: upstream.NewUpstream(script),
			Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
			Clock:    testClock(),
			Random:   testRng(),
			// ⚠ 关领取：本条只数"建了几条会话"，开着它要多编一批响应。
			// 领取与签到共用同一条会话这件事测在 `signin_session_test.go`。
			Receive: false,
		}
		var pool *SessionPool
		if withPool {
			pool = NewSessionPool()
			run.Sessions = pool
		}
		for round := 1; round <= 2; round++ {
			if _, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
				t.Fatalf("第 %d 轮签到失败：%s", round, perr.Message())
			}
		}
		if withPool && pool.Entries() != 1 {
			t.Errorf("两轮过后池里该只有 1 条，实得 %d", pool.Entries())
		}
		return script.count("tyrzLogin"), poolTokens(script)
	}

	built, tokens := signTwice(true)
	// 反向改坏点：把 `SignInRun.RunAccount` 里那句换回 `NewSignInSessionSource` ⇒ 这里变 2。
	if built != 1 {
		t.Errorf("接了池 ⇒ 两轮只该建 1 条会话，tyrzLogin 实得 %d 次", built)
	}
	for i, token := range tokens {
		if token != "JWT-1" {
			t.Fatalf("接了池 ⇒ 每条业务请求都该带同一张票，第 %d 条带的是 %s", i+1, token)
		}
	}

	// 对照：不接池 ⇒ 每轮一条、两轮的票不一样。这一支不成立就说明上面那个开关
	// 根本没接进 `RunAccount`（那时代码走的是同一个分支，两档读数会一模一样）。
	built, tokens = signTwice(false)
	if built != 2 {
		t.Errorf("没接池时每轮该各建一条（这是回落那一支的定义），实得 %d 次", built)
	}
	if len(tokens) < 4 || tokens[0] == tokens[3] {
		t.Errorf("没接池时第二轮该换一张票，实得 %v", tokens)
	}
}

// TestLiveRoomTwoRoundsShareOneToken 是同一件事在小红花那条链上的接线判据。
//
// ⚠ 它今天**不该**被读成"小红花页有自己那张票"：那条链仍借用签到页配方
// （调用点标了"jwttoken 使用公共来源"）。这一条只钉"接了池 ⇒ 两轮只建一条会话"，
// 将来抓包换成特有配方时，本条的 `tyrzLogin` 次数判据不变，只是键里那位变了。
func TestLiveRoomTwoRoundsShareOneToken(t *testing.T) {
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235), liveUserInfoBody(240)},
		draws:    []string{liveDrawOKBody(5)},
	})...)
	run := liveTaskRun(script, LiveRoomParams{Codes: []string{"口令甲"}}, nil)
	run.Sessions = NewSessionPool()

	for round := 1; round <= 2; round++ {
		if _, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
			t.Fatalf("第 %d 轮不该失败：%s", round, perr.Message())
		}
	}
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("两轮该共用 1 条云盘会话，tyrzLogin 实得 %d 次", n)
	}
	// ⭐ 省下的只有"建会话"，**没有**把每一轮该看的也省掉：
	// 门是每轮各查一次（两个页签 × 两轮）。漏了这一句的话，
	// "把整趟结果缓存起来"这种改法也能让上面那条继续绿。
	if n := script.count("tabContentList"); n != 4 {
		t.Errorf("两轮各该查两个页签（合计 4），实得 %d 次", n)
	}
	if run.Sessions.Entries() != 1 {
		t.Errorf("池里该只有 1 条，实得 %d", run.Sessions.Entries())
	}
}

// TestTwoProbesShareOneSession 钉住"界面上连着看这一屏"那一格：
// 探针每次都重新问门（那是它唯一要回答的"今晚有没有"），但票复用。
func TestTwoProbesShareOneSession(t *testing.T) {
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235)},
	})...)
	up := upstream.NewUpstream(script)
	pool := NewSessionPool()

	for i := 0; i < 2; i++ {
		probe, perr := ProbeLiveRoom(context.Background(), up, probeProfile(),
			probeCredential(), testClock(), testRng(), pool)
		if perr != nil {
			t.Fatalf("第 %d 次探针不该失败：%s", i+1, perr.Message())
		}
		if probe.Balance == nil || probe.Balance.FlowerCnt != 235 {
			t.Fatalf("第 %d 次没读到余额：%+v", i+1, probe)
		}
	}
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("两次探针该共用 1 条会话，tyrzLogin 实得 %d 次", n)
	}
	// 对照：不接池 ⇒ 每次看一眼各建一条（CLI 与测试夹具走的就是这一支）。
	solo := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235)},
	})...)
	for i := 0; i < 2; i++ {
		if _, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(solo),
			probeProfile(), probeCredential(), testClock(), testRng(), nil); perr != nil {
			t.Fatalf("对照那一支第 %d 次不该失败：%s", i+1, perr.Message())
		}
	}
	if n := solo.count("tyrzLogin"); n != 2 {
		t.Errorf("没接池时每次探针各建一条，tyrzLogin 实得 %d 次", n)
	}
}
