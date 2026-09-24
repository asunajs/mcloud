package app

// 红包派对**会话层**的测试：这张票是**哪一枚**、什么时候建、什么时候**不该**自动重建。
//
// # 这批用例钉的四件事
//
//  1. 建会话是**两枪**（换 SSO token → `tokenValidate`），而且取用本身懒到"第一次真要发请求"才出网；
//  2. 每个业务请求带的 `token` 是**活动站那一张**，不是签到页的 `jwttoken` ——
//     两池共用一份的表现就是这一格会错，而它错的时候**什么都跑得通**（上游回什么未实测）；
//  3. `requestId` / `timestamp` / `sign` **每次重算**（成文 §2.4 第 371 行），
//     而 `x-DeviceInfo` 里那串尾巴的前缀必须等于**同一趟**的 `timestamp`；
//  4. 这一族**没有**失效判据 ⇒ 业务请求失败时**不许**自动重建会话
//     （签到那一条有 `90001` 才允许刷一次重放一次；这里没有，所以一次都不刷）。
//
// ⚠ 与签到那批一样，全部走注入的假上游（[scriptedUpstream] / [liveTaskScript]），不联网。
// 断言优先数**上游请求与请求头上的票**，其次才读 [CloudPhoneSessionSource.Opens] 与
// [CloudPhoneSessionPool.Entries] —— 计数对了而票发错，只有前者抓得住。

import (
	"context"
	"strings"
	"sync"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具：活动站那一族的响应（值全是合成号，键名与实拍一致）
// ─────────────────────────────────────────────────────────────────────────────

// cloudPhoneTicketOK 是 `user/tokenValidate` 的成功响应。
//
// ⚠ `account` 用 199 合成号（仓库口径：成文与测试里的手机号一律掩码且不带尾号）。
func cloudPhoneTicketOK(token string) string {
	return `{"version":"1.0","header":{"appId":"12345681","errMsg":"OK","status":"200"},` +
		`"data":{"account":"199****0001","expireTime":3600,"riskLevel":"PASS",` +
		`"token":"` + token + `"}}`
}

func cloudPhoneBalanceOK(can, total string) string {
	return `{"header":{"errMsg":"OK","status":"200"},"data":{"info":{` +
		`"account":"199****0001","canAmount":"` + can + `","id":"7086772",` +
		`"totalAmount":"` + total + `"}}}`
}

func cloudPhoneToastOK() string {
	return `{"header":{"status":"200"},"data":{"isShowCloud":1}}`
}

// cloudPhoneTaskListOK 是 `configTaskLoginList` 的最小可用形状：三张表各放几条，
// `userStatus` 覆盖 `0`/`1`/`2`/缺失 四档（判据在 `cloud_phone_run_test.go` 那条
// "只数 0"的用例里，这里只负责让清单那一枪**回得来**）。
func cloudPhoneTaskListOK() string {
	return `{"header":{"status":"200"},"data":{` +
		`"configTaskNoviceList":[{"id":"1","taskCode":"NOVICE_1","userStatus":0}],` +
		`"configTaskDailyList":[{"id":"2","taskCode":"DAILY_1","prizeAmount":"20","userStatus":1},` +
		`{"id":"3","taskCode":"DAILY_2","userStatus":2},{"id":"4","taskCode":"DAILY_3"}],` +
		`"configTaskMonthlyList":[{"id":"5","taskCode":"MONTHLY_1","userStatus":0}],` +
		`"configTaskBuyList":[],"secondConfirmStatus":0}}`
}

// cloudPhoneStatusBad 是这一族**失败**那一侧的形状（`status` 非 `"200"` + 一句文案）。
//
// 它的用途不是"演一个业务错误"，而是钉住第 4 条：这一枪失败之后**不得**再打换票那一枪。
const cloudPhoneStatusBad = `{"header":{"errMsg":"活动已结束","status":"500"},"data":null}`

// cloudPhoneHeader 按名字取请求头（大小写不敏感，取第一条）。
func cloudPhoneHeader(request core.HTTPRequest, name string) string {
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1]
		}
	}
	return ""
}

// cloudPhoneFixedDevice 是一条**不联网**的 deviceId 取法（换票那一枪要它，
// 而"取它"本身是另一条链的事，见 [DeviceID]）。
func cloudPhoneFixedDevice() func(context.Context) (string, *core.Error) {
	return func(context.Context) (string, *core.Error) {
		return "SYNTH-DEVICE-199", nil
	}
}

func cloudPhoneSource(client core.HTTPClient, cred *SignInCredential) *CloudPhoneSessionSource {
	return NewCloudPhoneSessionSource(upstream.NewUpstream(client), testProfile(),
		cred, testClock(), testRng(), cloudPhoneFixedDevice())
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 1：建会话是两枪，而且取用不出网
// ─────────────────────────────────────────────────────────────────────────────

func TestOpenCloudPhoneSessionIsTwoShots(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
	}}
	session, perr := OpenCloudPhoneSession(context.Background(),
		upstream.NewUpstream(client), testProfile(), cloudPhoneTestCredential(),
		"SYNTH-DEVICE-199", testClock(), testRng())
	if perr != nil {
		t.Fatalf("建会话失败：%s", perr.Message())
	}
	if len(client.requests) != 2 {
		t.Fatalf("建会话该恰好两枪，实得 %d：%v", len(client.requests), client.urls())
	}
	if !strings.Contains(client.requests[0].URL, "querySpecToken") {
		t.Errorf("第一枪该是换 SSO token，实得 %s", client.requests[0].URL)
	}
	if !strings.Contains(client.requests[1].URL, "user/tokenValidate") {
		t.Errorf("第二枪该是 tokenValidate，实得 %s", client.requests[1].URL)
	}
	// 第二枪的 body 里必须同时有 deviceId 与**第一枪换来的那枚** SSO token ——
	// 少任何一个都是一条确定不像客户端的请求。
	body := string(client.requests[1].Body)
	if !strings.Contains(body, "SYNTH-DEVICE-199") || !strings.Contains(body, "SSO-1") {
		t.Errorf("换票那一枪的 body 缺 deviceId 或 SSO token：%s", body)
	}
	// referer / x-origin 是**成文过的地址字面量**，不是从响应里读回来的。
	if session.referer != upstream.CloudPhoneRefererHomeURL ||
		session.xOrigin != upstream.CloudPhoneXOriginHomeURL {
		t.Errorf("会话上的两个地址不是成文那两条")
	}
}

// TestCloudPhoneSessionSourceIsLazyAndBuildsOnce 钉住"取一次没用上 = 零代价"。
//
// 反向改坏点：把 [CloudPhoneSessionPool.Source] 写成"顺手建一条会话" ⇒ 第一条断言红。
func TestCloudPhoneSessionSourceIsLazyAndBuildsOnce(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
	}}
	src := cloudPhoneSource(client, cloudPhoneTestCredential())
	if len(client.requests) != 0 {
		t.Fatalf("只是拿到会话源就该零请求，实得 %d", len(client.requests))
	}
	for i := 0; i < 2; i++ {
		if _, perr := src.Session(context.Background()); perr != nil {
			t.Fatalf("第 %d 次取会话失败：%s", i+1, perr.Message())
		}
	}
	if src.Opens() != 1 {
		t.Errorf("两次取用只该建一条会话，实得 %d", src.Opens())
	}
	if n := client.count("tokenValidate"); n != 1 {
		t.Errorf("换票只该打一次，实得 %d", n)
	}
}

// TestCloudPhoneSessionSourceWithoutDeviceGetterFailsBeforeIO 钉住 [CloudPhoneSessionSource]
// 里 `deviceID == nil` 那一支：**报错而不是回落成空串**，且**一枪都不发**。
//
// 空 `deviceId` 发出去的是一条确定不像客户端的请求；而"先打两枪再发现没 deviceId"
// 会把两枪的失败当成"票刷不动"（这一族没有失效判据，见文件头第 4 条）。
func TestCloudPhoneSessionSourceWithoutDeviceGetterFailsBeforeIO(t *testing.T) {
	client := &scriptedUpstream{responses: []string{ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1")}}
	src := NewCloudPhoneSessionSource(upstream.NewUpstream(client), testProfile(),
		cloudPhoneTestCredential(), testClock(), testRng(), nil)
	_, perr := src.Session(context.Background())
	if perr == nil {
		t.Fatal("没接 deviceId 取法时应报错，实得 nil")
	}
	if !strings.Contains(perr.Message(), "deviceId") {
		t.Errorf("错误串该点名 deviceId（漏接线的人只看这一句），实得 %q", perr.Message())
	}
	if len(client.requests) != 0 {
		t.Errorf("这一支该在出网之前失败，实得 %d 条请求", len(client.requests))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 2：带出去的是活动站那张票
// ─────────────────────────────────────────────────────────────────────────────

func TestCloudPhoneBusinessRequestCarriesTheActivityTicket(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
		cloudPhoneBalanceOK("210", "370"), cloudPhoneTaskListOK(), cloudPhoneToastOK(),
	}}
	src := cloudPhoneSource(client, cloudPhoneTestCredential())
	if _, perr := CollectCloudPhoneAssets(context.Background(), src); perr != nil {
		t.Fatalf("三枪都该成功：%s", perr.Message())
	}
	if len(client.requests) != 5 {
		t.Fatalf("两枪建会话 + 三枪只读 = 5，实得 %d：%v", len(client.requests), client.urls())
	}
	business := client.requests[2]
	if !strings.Contains(business.URL, "redpacket/userAccountInfo") {
		t.Fatalf("第 3 枪该是余额，实得 %s", business.URL)
	}
	if got := cloudPhoneHeader(business, "token"); got != "ACT-1" {
		t.Errorf("业务请求带的 token 不是活动站那一张：%q", got)
	}
	// ⭐ 这一句是"两池不共用"的全部意义：签到的票串绝不该出现在这一族的请求头上。
	if n := countHeaderNamed(business, "jwttoken"); n != 0 {
		t.Errorf("红包派对请求不该带签到页的 jwttoken，实得 %d 处", n)
	}
	if got := cloudPhoneHeader(business, "referer"); got != upstream.CloudPhoneRefererHomeURL {
		t.Errorf("referer 实得 %q", got)
	}
	if got := cloudPhoneHeader(business, "x-origin"); got != upstream.CloudPhoneXOriginHomeURL {
		t.Errorf("x-origin 实得 %q（会话里带着它，调用点不该有机会漏传）", got)
	}
	// 三枪带的**必须**是同一张票（票不换，换的只是每趟的 requestId/timestamp/sign）。
	for i, request := range client.requests[2:] {
		if got := cloudPhoneHeader(request, "token"); got != "ACT-1" {
			t.Errorf("第 %d 枪带的不是那张票：%q", i+1, got)
		}
	}
}

// countHeaderNamed 数某个头名出现几次（本包自带的版本）。
func countHeaderNamed(request core.HTTPRequest, name string) int {
	n := 0
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			n++
		}
	}
	return n
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 3：每次重算，且设备串与头同源同一趟
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneAuthForRecomputesPerCall ⭐ 反向改坏点最直白的一条：
// 把 `AuthFor` 改成"第一次算完存起来" ⇒ 这里三条断言全红，而链照样跑得通（本地看不出问题），
// 只有上游会看见同一个 `requestId` 被重复提交。
func TestCloudPhoneAuthForRecomputesPerCall(t *testing.T) {
	session := CloudPhoneSession{
		token: "ACT-1", referer: upstream.CloudPhoneRefererHomeURL,
		xOrigin: upstream.CloudPhoneXOriginHomeURL,
	}
	// ⚠ 必须是**同一个** rng 实例：[testRng] 每次调用都新造一条脚本，
	// 两次各造一份会得到同一个随机后缀，于是这一条"该不一样"的断言永远红。
	rng := testRng()
	first := session.AuthFor(testClock(), rng)
	second := session.AuthFor(testClock(), rng)
	if first.RequestID == "" || first.Sign() == "" {
		t.Fatal("AuthFor 没把 requestId / sign 填出来")
	}
	if first.RequestID == second.RequestID {
		t.Errorf("requestId 两次相同 ⇒ 被缓存了：%s", first.RequestID)
	}
	if first.Sign() == second.Sign() {
		t.Error("sign 两次相同 ⇒ 复用了一组头素材")
	}
	// 票不变：变的只是这一趟的三个数。
	if first.Token != "ACT-1" || second.Token != "ACT-1" {
		t.Error("AuthFor 把票换了")
	}
	// ⭐ 设备串尾巴的**前缀**就是同一趟的 timestamp（旧 TS `task.ts:57-71` 那一串）。
	// 分开取时钟会出现"头说一个时刻、设备串说另一个时刻"。
	if !strings.Contains(first.DeviceInfo, first.Timestamp) {
		t.Errorf("x-DeviceInfo 里没带上这一趟的 timestamp：%q / %q", first.DeviceInfo, first.Timestamp)
	}
}

// TestCloudPhoneAuthForUsesFixedMillisecondTimestamp 钉住 `timestamp` 是**毫秒十进制串**，
// 而这一族的时钟读数与签到那条的**微秒**不是同一个东西（[UserTimestamp]）。
func TestCloudPhoneAuthForUsesFixedMillisecondTimestamp(t *testing.T) {
	session := CloudPhoneSession{token: "ACT-1"}
	auth := session.AuthFor(testClock(), testRng())
	// testClock 的读数：2026-09-20 12:00:00.123456 UTC → 毫秒位是 123。
	if !strings.HasSuffix(auth.Timestamp, "123") {
		t.Errorf("timestamp 该是毫秒十进制串，实得 %q", auth.Timestamp)
	}
	if strings.Contains(auth.Timestamp, ".") || len(auth.Timestamp) != 13 {
		t.Errorf("timestamp 形状不对（13 位整数）：%q", auth.Timestamp)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 4：失败**不**自动重建
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneFailureDoesNotReopenSession ⭐ 这一条是"这一族与签到那条最大的不同"。
//
// 签到撞 `90001` 允许"刷一次 + 重放一次"，因为那个码**实测过**；
// 本族连"票过期回什么"都零样本 ⇒ 自动重建等于拿猜测替换服务端语义，
// 而且会把"活动下线"读成"票坏了"再白打一轮。
// 反向改坏点：在 [cloudPhoneCall] 里加一句"失败就清掉会话重试" ⇒ 这里换票变 2 次。
func TestCloudPhoneFailureDoesNotReopenSession(t *testing.T) {
	script := liveTaskScriptWith(
		liveTaskRoute{"/user/querySpecToken", []string{ssoOK("SSO-1")}},
		liveTaskRoute{"user/tokenValidate", []string{cloudPhoneTicketOK("ACT-1")}},
		liveTaskRoute{"redpacket/userAccountInfo", []string{cloudPhoneStatusBad}},
	)
	src := cloudPhoneSource(script, cloudPhoneTestCredential())
	_, perr := CollectCloudPhoneAssets(context.Background(), src)
	if perr == nil {
		t.Fatal("余额那一枪回非 200，整趟该失败")
	}
	if !strings.Contains(perr.Message(), "活动已结束") {
		t.Errorf("该把上游那句文案带出去，实得 %q", perr.Message())
	}
	if n := script.count("tokenValidate"); n != 1 {
		t.Errorf("业务失败之后不得重建会话，换票实得 %d 次", n)
	}
	if src.Opens() != 1 {
		t.Errorf("会话该只建过 1 条，实得 %d", src.Opens())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 票池：作用域与凭据轮换
// ─────────────────────────────────────────────────────────────────────────────

func TestCloudPhonePoolHandsOutOneSourcePerAccount(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
	}}
	up := upstream.NewUpstream(client)
	pool := NewCloudPhoneSessionPool()
	cred := poolCredential("19****01", "Basic x")

	first := pool.Source(up, testProfile(), cred, testClock(), testRng(), cloudPhoneFixedDevice())
	for i := 1; i < 3; i++ {
		if again := pool.Source(up, testProfile(), cred, testClock(), testRng(),
			cloudPhoneFixedDevice()); again != first {
			t.Fatalf("第 %d 次取到的是另一条会话源（票池没命中键）", i+1)
		}
	}
	if pool.Entries() != 1 {
		t.Errorf("同一账号只该存 1 条，实得 %d", pool.Entries())
	}
	// 取用零请求（懒建那条约束在池这一层同样成立）。
	if len(client.requests) != 0 {
		t.Errorf("只是取会话源就该零请求，实得 %d 条", len(client.requests))
	}
}

func TestCloudPhonePoolIsolatesAccounts(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
	}}
	up := upstream.NewUpstream(client)
	pool := NewCloudPhoneSessionPool()
	a := pool.Source(up, testProfile(), poolCredential("19****01", "Basic x"),
		testClock(), testRng(), cloudPhoneFixedDevice())
	b := pool.Source(up, testProfile(), poolCredential("19****02", "Basic y"),
		testClock(), testRng(), cloudPhoneFixedDevice())
	if a == b {
		t.Fatal("两个账号共用了同一条会话源 ⇒ 甲的请求会带着乙的票")
	}
	if pool.Entries() != 2 {
		t.Errorf("两个账号该各存 1 条，实得 %d", pool.Entries())
	}
}

// TestCloudPhonePoolDropsRotatedCredential 与签到那侧 [TestRotatedCredentialDropsTheOldEntries]
// 同一判据：外圈凭据换过之后，旧会话源再也刷不出票，留着只会把失败读成"票刷不动"。
func TestCloudPhonePoolDropsRotatedCredential(t *testing.T) {
	up := upstream.NewUpstream(&scriptedUpstream{})
	pool := NewCloudPhoneSessionPool()
	old := pool.Source(up, testProfile(), poolCredential("19****01", "Basic old"),
		testClock(), testRng(), cloudPhoneFixedDevice())
	if pool.Entries() != 1 {
		t.Fatalf("夹具没到位，实得 %d 条", pool.Entries())
	}
	fresh := pool.Source(up, testProfile(), poolCredential("19****01", "Basic new"),
		testClock(), testRng(), cloudPhoneFixedDevice())
	if fresh == old {
		t.Fatal("换了登录凭据还复用旧会话源")
	}
	if pool.Entries() != 1 {
		t.Errorf("旧凭据那条该作废，实得剩 %d 条", pool.Entries())
	}
}

// TestCloudPhonePoolFallbackWithoutPool 钉住 nil 池回落成"每一次那一条"
// （CLI 单次调用与测试夹具走的就是这一支）。
func TestCloudPhonePoolFallbackWithoutPool(t *testing.T) {
	up := upstream.NewUpstream(&scriptedUpstream{})
	cred := poolCredential("19****01", "Basic x")
	first := cloudPhoneSessionSource(nil, up, testProfile(), cred, testClock(), testRng(),
		cloudPhoneFixedDevice())
	second := cloudPhoneSessionSource(nil, up, testProfile(), cred, testClock(), testRng(),
		cloudPhoneFixedDevice())
	if first == second {
		t.Error("无池时两次该各拿一条（否则 CLI 的两轮会共用一条）")
	}
	if got := cloudPhoneSessionSource(NewCloudPhoneSessionPool(), up, testProfile(), cred,
		testClock(), testRng(), cloudPhoneFixedDevice()); got == nil {
		t.Error("接了池却拿到 nil")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 并发
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneConcurrentSessionBuildsOneTicket 是"调度那一趟与手点那一趟同时到"那一格：
// 只建一条会话（两枪），而**不是**八个账号各建一条。
func TestCloudPhoneConcurrentSessionBuildsOneTicket(t *testing.T) {
	script := liveTaskScriptWith(
		liveTaskRoute{"/user/querySpecToken", []string{ssoOK("SSO-1")}},
		liveTaskRoute{"user/tokenValidate", []string{cloudPhoneTicketOK("ACT-1")}},
	)
	src := cloudPhoneSource(script, cloudPhoneTestCredential())

	const n = 8
	var wg sync.WaitGroup
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, perr := src.Session(context.Background()); perr != nil {
				t.Errorf("取会话失败：%s", perr.Message())
			}
		}()
	}
	wg.Wait()
	if n2 := script.count("tokenValidate"); n2 != 1 {
		t.Errorf("并发取用只该建一条会话，换票实得 %d 次", n2)
	}
	if src.Opens() != 1 {
		t.Errorf("Opens 该是 1，实得 %d", src.Opens())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 凭据与描述
// ─────────────────────────────────────────────────────────────────────────────

func cloudPhoneTestCredential() *SignInCredential {
	return poolCredential("19****01", wholeAuth)
}

// TestCloudPhoneSessionDescribeHidesTheTicket 钉住"可打印的那一份里没有票串"。
//
// 这一族比签到那条更严：`Describe` 里连 referer 都给了**完整值**，
// 因为成文那两条地址字面量不带 token（构造常量时就断言过；见 upstream 那侧的用例）。
// ⚠ 别把这一条读成"活动站票可以打印"。
func TestCloudPhoneSessionDescribeHidesTheTicket(t *testing.T) {
	session := CloudPhoneSession{
		token: "SYNTH-ACTIVITY-TOKEN-199", referer: upstream.CloudPhoneRefererHomeURL,
		xOrigin: upstream.CloudPhoneXOriginHomeURL,
	}
	described := session.Describe()
	if strings.Contains(described, "SYNTH-ACTIVITY-TOKEN-199") {
		t.Errorf("Describe 漏出了票串：%s", described)
	}
	// 给的是**长度**：排障时要能回答"这两次拿到的是不是同一张"，而答案不能是票串本身。
	if !strings.Contains(described, "24 字符") {
		t.Errorf("Describe 该给票串长度读数，实得 %s", described)
	}
	if !session.same(CloudPhoneSession{
		token: "SYNTH-ACTIVITY-TOKEN-199", referer: upstream.CloudPhoneRefererHomeURL,
		xOrigin: upstream.CloudPhoneXOriginHomeURL,
	}) {
		t.Error("same 该按内容判定（测试要靠它钉「复用的是同一条」）")
	}
	if session.same(CloudPhoneSession{token: "别的票"}) {
		t.Error("same 把不同的票读成了同一条")
	}
}
