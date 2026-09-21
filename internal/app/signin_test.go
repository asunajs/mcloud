package app

// 签到业务层的测试。
//
// 这一层**不联网**：HTTP 走注入的 [scriptedUpstream]，存储走嵌入接口的部分假实现。
// 所有断言都在回答"这一步该不该发、凭什么算成功"，而不是"请求长什么样"
// —— 后者归 `internal/upstream` 那侧管。

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 测试替身
// ─────────────────────────────────────────────────────────────────────────────

// scriptedUpstream 按脚本**顺序**回放响应，并留下收到的每一个请求供断言。
//
// 顺序回放而不是"每个 URL 配一条"，是因为签到链路的判据本来就包含"第几个请求"：
// 「已签就不发 startSignIn」这条断言只能靠数请求个数来验证。
type scriptedUpstream struct {
	responses []string
	requests  []core.HTTPRequest
	cursor    int
}

func (c *scriptedUpstream) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	if c.cursor >= len(c.responses) {
		return core.HTTPResponse{}, errors.New("脚本里没有更多响应了（多发了请求）")
	}
	body := c.responses[c.cursor]
	c.cursor++
	return core.NewResponse(200, []byte(body)), nil
}

func (c *scriptedUpstream) urls() []string {
	out := make([]string, 0, len(c.requests))
	for _, r := range c.requests {
		out = append(out, r.URL)
	}
	return out
}

func (c *scriptedUpstream) count(substr string) int {
	n := 0
	for _, u := range c.urls() {
		if strings.Contains(u, substr) {
			n++
		}
	}
	return n
}

// fixedClock 固定时钟：微秒那条断言没有它钉不住。
type fixedClock struct{ at time.Time }

func (c fixedClock) Now() time.Time { return c.at }

// seqRng 按脚本吐字节，用满后回绕。
type seqRng struct {
	script []byte
	at     int
}

func (r *seqRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.script[r.at%len(r.script)]
		r.at++
	}
}

// fakeStore 用**内嵌接口**做部分假实现：没重写的方法一调用就 panic，
// 于是"这一层碰了它不该碰的存储入口"会当场响，而不是静默返回零值。
type fakeStore struct {
	core.StateStore
	state   *core.AccountState
	account *core.Account
}

func (s *fakeStore) GetAccountState(_ context.Context, _ core.Scope, _ core.AccountID) (*core.AccountState, error) {
	return s.state, nil
}

func (s *fakeStore) GetAccount(_ context.Context, _ core.Scope, _ core.AccountID) (*core.Account, error) {
	return s.account, nil
}

func testRng() *seqRng { return &seqRng{script: []byte{0x11, 0x22, 0x33, 0x44, 0x55}} }

func testClock() fixedClock {
	return fixedClock{at: time.Date(2026, 9, 20, 12, 0, 0, 123456000, time.UTC)}
}

func testProfile() *upstream.AppProfile {
	return upstream.NewAppProfile(upstream.NewIPadAppDevice(
		"67F4C56A00000000000000000000315C", "8A708867DBC3"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 建会话
// ─────────────────────────────────────────────────────────────────────────────

func TestUserTimestampIsMicrosecondsNotMilliseconds(t *testing.T) {
	got := UserTimestamp(testClock())
	// 16 位是微秒；13 位才是毫秒
	if n := len(itoa64(got)); n != 16 {
		t.Errorf("x-yun-user-timestamp 应是微秒（16 位），实得 %d 位：%d", n, got)
	}
	if want := testClock().Now().UnixMicro(); got != want {
		t.Errorf("应等于时钟的微秒，实得 %d 期望 %d", got, want)
	}
}

// TestSessionWalksTheThreeStepsInOrder：建会话的顺序本身就是判据 ——
// 换 token 与用它之间**不能**夹别的 IO（那个 token 几十秒就废）。
func TestSessionWalksTheThreeStepsInOrder(t *testing.T) {
	up := upstream.NewUpstream(&scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT","mobile":"199****0001"}}`,
	}})
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	referer, jwt, perr := OpenSignInSession(context.Background(), up, testProfile(),
		&cred, testClock(), testRng())
	if perr != nil {
		t.Fatalf("建会话失败: %v", perr)
	}
	if jwt != "JWT" {
		t.Errorf("应拿到 jwtToken，实得 %q", jwt)
	}
	if !strings.Contains(referer, "SSO-TOKEN") {
		t.Errorf("referer 应带上本次的 SSO token，实得 %q", referer)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 签到：成败判据
// ─────────────────────────────────────────────────────────────────────────────

// infoV3 造一份 `infoV3` 响应。
//
// ⚠ 四个整数字段（`total` / `signInPoints` / `signCount` / `monthDays`）**一个都不能少**：
// 它们在协议层是 `RequireInt64` 硬要求的（见 `ParseSignInStatus`），缺任一个整份解析就失败。
// 照抓包写"我关心的那几个"会让测试以"协议层报错"的形式挂掉，而真正想测的门槛判定根本没跑到。
func infoV3(todaySigned bool) string {
	cell := `{"d":20,"s":%s,"currentMonth":1,"n":50,"t":true}`
	if todaySigned {
		return statusPayload(`"cal":[` + strings.Replace(cell, "%s", "true", 1) + `]`)
	}
	return statusPayload(`"cal":[` + strings.Replace(cell, "%s", "false", 1) + `]` +
		`,"monthDays":6`)
}

// statusPayload 补上协议层硬要求的那四个整数字段，再拼上本用例关心的字段。
func statusPayload(extra string) string {
	return `{"code":0,"msg":"success","result":{` + extra +
		`,"signInPoints":3,"signCount":7,"total":10496,"monthDays":7}}`
}

const startSignInOK = `{"code":0,"msg":"success","result":{"todaySignIn":false,"signInPoints":50,"signCount":7}}`

// TestSignInSkipsStartSignInWhenAlreadySigned：
// 今天已签就**不发**写请求 —— 少一次写，也少一次风控记数。
func TestSignInSkipsStartSignInWhenAlreadySigned(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		infoV3(true),
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	report, perr := CloudSignIn(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, SignInOptions{}, testClock(), testRng())
	if perr != nil {
		t.Fatalf("签到失败: %v", perr)
	}
	if client.count("/startSignIn") != 0 {
		t.Errorf("今天已签就不该发 startSignIn，实得 %d 次", client.count("/startSignIn"))
	}
	if report.Result != nil {
		t.Error("没发请求时 Result 应为 nil —— 它是「这次有没有发」的事实，不是默认值")
	}
	if !report.WasAlreadySigned() || !report.SignedToday() {
		t.Error("来之前已签、现在也已签")
	}
	if report.SignedNow() {
		t.Error("这次**没有**把今天从「未签」变成「已签」，SignedNow 应为 false")
	}
}

// TestSignInTrustsTheRecheckNotTheStartSignInField：
// `startSignIn` 回的 `todaySignIn` 是**调用前**的快照 —— 实测 `false` 的那次恰恰签上了。
// 所以唯一可信的判据是回查 `infoV3`。
func TestSignInTrustsTheRecheckNotTheStartSignInField(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		infoV3(false), // 调用前：未签
		startSignInOK, // startSignIn 回 todaySignIn:false（**快照**，不是结论）
		infoV3(true),  // 回查：签上了
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	report, perr := CloudSignIn(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, SignInOptions{}, testClock(), testRng())
	if perr != nil {
		t.Fatalf("应判为成功（回查已签），实得错误: %v", perr)
	}
	if !report.SignedNow() {
		t.Error("这次真的把今天签上了，SignedNow 应为 true")
	}
	if client.count("/infoV3") != 2 {
		t.Errorf("应回查一次 infoV3，实得 %d 次", client.count("/infoV3"))
	}
	if got := report.SignInPoints(); got == nil || *got != 50 {
		t.Errorf("本次可得云朵应为 50，实得 %v", got)
	}
}

// TestSignInFailsWhenTheRecheckSaysNotSigned：
// `code == 0` 也不算成功 —— 上游对"今天已经签过"同样回 success。
func TestSignInFailsWhenTheRecheckSaysNotSigned(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		infoV3(false),
		startSignInOK,
		infoV3(false), // 回查仍未签
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	_, perr := CloudSignIn(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, SignInOptions{}, testClock(), testRng())
	if perr == nil {
		t.Fatal("回查显示未签就应当失败 —— 只看 code == 0 会把「已签过」误报成成功")
	}
	if !strings.Contains(perr.Message(), "infoV3") {
		t.Errorf("报错要指名判据是 infoV3，实得 %q", perr.Message())
	}
}

// TestForceSendsStartSignInButDoesNotCountAsSignedNow：
// `--force` 是协议自检用的，它会多打一次写请求，但**不算**"这次签上的"。
func TestForceSendsStartSignInButDoesNotCountAsSignedNow(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		infoV3(true),
		startSignInOK,
		infoV3(true),
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	report, perr := CloudSignIn(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, SignInOptions{Force: true}, testClock(), testRng())
	if perr != nil {
		t.Fatalf("自检模式下已签也发，应成功: %v", perr)
	}
	if client.count("/startSignIn") != 1 {
		t.Errorf("开了 force 就应发 startSignIn，实得 %d 次", client.count("/startSignIn"))
	}
	if report.SignedNow() {
		t.Error("本来就已签，force 那次**不算**「这次签上的」—— 定时任务连跑一个月时，只有分开报才看得出它到底有没有在干活")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 凭据
// ─────────────────────────────────────────────────────────────────────────────

const wholeAuth = "Basic MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr"

// TestCredentialPrefersTheWholeAuthorizationString：
// v7 起库里存的是整串，一段都不用凑 —— 也不需要 `phone` 列。
func TestCredentialPrefersTheWholeAuthorizationString(t *testing.T) {
	auth := core.SecretValue(wholeAuth)
	store := &fakeStore{state: &core.AccountState{Authorization: &auth}}

	cred, perr := CredentialFromAccount(context.Background(), store,
		core.ScopeLocal(), core.NewAccountID("19****01"))
	if perr != nil {
		t.Fatalf("取整串失败: %v", perr)
	}
	if cred.Authorization() != wholeAuth {
		t.Errorf("应**原样**取到整串，实得 %q", cred.Authorization())
	}
	// 手机号只用于打印：库里没给也能从整串里解出来，不该因此报错
	if cred.Phone() != "19900000001" {
		t.Errorf("手机号应顺手从整串里解出来，实得 %q", cred.Phone())
	}
}

// TestCredentialFallsBackToPhoneAndTokenForLegacyRows：
// 老行（v7 之前）没有整串，退回 `phone` + `jwt_token` 现拼，前缀是 `mobile`。
func TestCredentialFallsBackToPhoneAndTokenForLegacyRows(t *testing.T) {
	token := core.SecretValue("AUTH-TOKEN")
	phone := core.FullPhone("19900000001")
	store := &fakeStore{state: &core.AccountState{JWTToken: &token, Phone: &phone}}

	cred, perr := CredentialFromAccount(context.Background(), store,
		core.ScopeLocal(), core.NewAccountID("19****01"))
	if perr != nil {
		t.Fatalf("现拼失败: %v", perr)
	}
	want := upstream.AppAuthorization("19900000001", "AUTH-TOKEN")
	if cred.Authorization() != want {
		t.Errorf("老行应现拼，实得 %q 期望 %q", cred.Authorization(), want)
	}
	if !strings.HasPrefix(cred.Authorization(), "Basic bW9iaWxl") {
		t.Errorf("现拼的前缀固定是 mobile（base64 后以 bW9iaWxl 开头），实得 %q", cred.Authorization())
	}
}

// TestCredentialRejectsAnAccountLoggedInOnTheOtherClient：
// 前缀闸只会**新增拒绝**：只有"能证明是另一个端登录的"才拦。
func TestCredentialRejectsAnAccountLoggedInOnTheOtherClient(t *testing.T) {
	token := core.SecretValue("TOKEN")
	phone := core.FullPhone("19900000001")
	platform := "pc"
	store := &fakeStore{
		state:   &core.AccountState{JWTToken: &token, Phone: &phone},
		account: &core.Account{Platform: &platform},
	}
	_, perr := CredentialFromAccount(context.Background(), store,
		core.ScopeLocal(), core.NewAccountID("19****01"))
	if perr == nil {
		t.Fatal("PC 端登录的账号不该被拿去跑 App 端签到 —— 那会拿一个「PC 发的 token 配 App 的前缀」去试探上游，失败长得像「未登录」")
	}
	if !strings.Contains(perr.Message(), "--client app") {
		t.Errorf("报错要给出出路（重登一次），实得 %q", perr.Message())
	}
}

// TestCredentialNeverInventsTheHeaderFromTheMask：
// 既没有整串、也没有完整手机号时**报错**，绝不退化成拿掩码去凑。
func TestCredentialNeverInventsTheHeaderFromTheMask(t *testing.T) {
	token := core.SecretValue("TOKEN")
	store := &fakeStore{state: &core.AccountState{JWTToken: &token}}
	id := core.NewAccountID("19****01")

	_, perr := CredentialFromAccount(context.Background(), store, core.ScopeLocal(), id)
	if perr == nil {
		t.Fatal("拼不出凭据就该报错 —— 拿掩码去凑的话，服务端认不认是未知数，而这类「能过但不该过」一旦过了一次就会变成没人敢动的隐性依赖")
	}
	if strings.Contains(perr.Message(), id.String()) && !strings.Contains(perr.Message(), "--phone") {
		t.Errorf("报错要给出两条出路，实得 %q", perr.Message())
	}
}

// TestCredentialStringHidesTheSecret：`%v` 是最容易在排障时顺手打出来的东西。
func TestCredentialStringHidesTheSecret(t *testing.T) {
	auth := core.SecretValue(wholeAuth)
	cred := SignInCredential{
		authorization: string(auth),
		phone:         "19900000001",
		accountID:     core.NewAccountID("19****01"),
	}
	text := cred.String()
	if strings.Contains(text, wholeAuth) || strings.Contains(text, "MjoxOTkw") {
		t.Errorf("打印凭据时**整串**不能出现，实得 %q", text)
	}
	if strings.Contains(text, "19900000001") {
		t.Errorf("手机号也该掩码 —— 打日志没有任何理由用完整的，实得 %q", text)
	}
	if !strings.Contains(text, "19****01") {
		t.Errorf("账号 id 该留着（它是定位用的），实得 %q", text)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 领取的分流
// ─────────────────────────────────────────────────────────────────────────────

func TestPlanReceiveSplitsTheThreeWays(t *testing.T) {
	date := "2026-08"
	id := "REC-1"
	got := PlanReceive([]upstream.ReceiveItem{
		{CloudType: 1, CloudNum: 100, AcceptDate: &date},
		{CloudType: 2, CloudNum: 200},
		{CloudType: 0, CloudNum: 300, RecordID: &id},
		{CloudType: 7, CloudNum: 400},
	})
	if len(got) != 4 {
		t.Fatalf("应有 4 条，实得 %d", len(got))
	}
	if got[0].Kind != ReceiveActionMonthlyExpansion || got[0].AcceptDate != date {
		t.Errorf("cloudType=1 应走延期领取并带 acceptDate，实得 %+v", got[0])
	}
	if got[1].Kind != ReceiveActionNextMonth {
		t.Errorf("cloudType=2 是下月才可领，实得 %+v", got[1])
	}
	if got[2].Kind != ReceiveActionImmediate || got[2].CloudID != id {
		t.Errorf("其余走立即领取，实得 %+v", got[2])
	}
	// `recordId` 缺失时照 H5 的 `?? ""`：给空串，不是跳过
	if got[3].CloudID != "" || got[3].CloudType != 7 {
		t.Errorf("recordId 缺失应给空串（H5 的 e.recordId ?? \"\"），实得 %+v", got[3])
	}
}

// TestReceiveAllSendsNoQueryParamWhenAcceptDateIsEmpty：
// 空串必须落成"不带查询参数"（裸 GET），不是 `?acceptDate=` ——
// 后者与真实客户端分叉（axios 会把 `undefined` 整个键丢掉）。
func TestReceiveAllSendsNoQueryParamWhenAcceptDateIsEmpty(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		statusPayload(`"cal":[],"receiveList":[{"cloudType":1,"cloudNum":100}]`),
		`{"code":0,"msg":"success","result":{"cloudCount":100}}`,
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	outcomes, perr := ReceiveAll(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, "DEV", testClock(), testRng())
	if perr != nil {
		t.Fatalf("领取失败: %v", perr)
	}
	if len(outcomes) != 1 || outcomes[0].Kind != ReceiveOutcomeMonthlyExpansion {
		t.Fatalf("应领到月度那条，实得 %+v", outcomes)
	}
	if outcomes[0].AcceptDate != "" {
		t.Errorf("发出去的 acceptDate 应记为空（没带查询参数），实得 %q", outcomes[0].AcceptDate)
	}
	// ⭐ 真正的判据在**请求行**上：结果里记的那个空串只是"我记得我打算不带"，
	// 而唯一能证明"真的没带"的是最后一个请求的 URL 里没有 `acceptDate`。
	// 只断言前者，把 `nil` 换成 `&""` 也不会变红（已实测）。
	last := client.requests[len(client.requests)-1]
	if strings.Contains(last.URL, "acceptDate") {
		t.Errorf("裸 GET 才对 —— 多一个空的 `acceptDate=` 就与真实客户端分叉，实得 %q", last.URL)
	}
}

// TestReceiveAllKeepsGoingWhenOneItemFails：
// 一条失败不影响其余 —— 它们本来就是互不相干的账。
func TestReceiveAllKeepsGoingWhenOneItemFails(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		statusPayload(`"cal":[],"receiveList":[` +
			`{"cloudType":1,"cloudNum":100},{"cloudType":0,"cloudNum":50}]`),
		`{"code":1302,"msg":"未开启自动备份"}`,
		`{"code":0,"msg":"success","result":{"receive":50}}`,
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	outcomes, perr := ReceiveAll(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, "DEV", testClock(), testRng())
	if perr != nil {
		t.Fatalf("单条失败不该让整批失败: %v", perr)
	}
	if len(outcomes) != 2 {
		t.Fatalf("应有 2 条结果，实得 %d", len(outcomes))
	}
	if outcomes[0].Kind != ReceiveOutcomeFailed || outcomes[0].CloudType != 1 {
		t.Errorf("第一条应记失败，实得 %+v", outcomes[0])
	}
	if outcomes[1].Kind != ReceiveOutcomeImmediate || outcomes[1].Received != 50 {
		t.Errorf("第二条应照常领到，实得 %+v", outcomes[1])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 翻倍与备份好礼：门槛判定
// ─────────────────────────────────────────────────────────────────────────────

// comma 给"要追加的裸字段"补一个逗号：空串时不补（否则 `{"cal":[],}` 不是合法 JSON）。
func comma(extra string) string {
	if extra == "" {
		return ""
	}
	return ","
}

func statusWith(backup, accept *bool) upstream.SignInStatus {
	return upstream.SignInStatus{CurMonthBackup: backup, CurMonthBackupSignAccept: accept}
}

func TestPlanMultipleFourWays(t *testing.T) {
	yes, no := true, false
	cases := []struct {
		name   string
		status upstream.SignInStatus
		want   MultiplePlan
	}{
		// ⚠ 顺序有意义：两个都"不领"，但"已领过"是更具体的说法
		{"已领过", statusWith(&yes, &yes), MultiplePlanAlreadyAccepted},
		{"可领", statusWith(&yes, &no), MultiplePlanClaim},
		{"没开备份", statusWith(&no, &no), MultiplePlanNoBackup},
		{"上游没给", statusWith(nil, nil), MultiplePlanUnknown},
		{"只给了一半", statusWith(&yes, nil), MultiplePlanUnknown},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanMultiple(c.status); got != c.want {
				t.Errorf("实得 %v，期望 %v", got, c.want)
			}
		})
	}
}

// TestClaimMultipleSendsNoWriteRequestWhenTheGateIsClosed：
// 每月只有一次机会 —— 条件不成立还发，是白白消耗掉这个月的名额。
func TestClaimMultipleSendsNoWriteRequestWhenTheGateIsClosed(t *testing.T) {
	yes, no := true, false
	cases := []struct {
		name   string
		status string
		want   MultipleOutcomeKind
	}{
		{"已领过", `"curMonthBackup":true,"curMonthBackupSignAccept":true`,
			MultipleOutcomeAlreadyAccepted},
		{"没开备份", `"curMonthBackup":false`, MultipleOutcomeNoBackup},
		{"上游没给", ``, MultipleOutcomeUnknown},
	}
	_ = yes
	_ = no
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			client := &scriptedUpstream{responses: []string{
				`{"success":true,"code":"0000","message":"请求成功",` +
					`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
				`{"code":0,"msg":"success","result":1789800128813}`,
				`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
				statusPayload(`"cal":[]` + comma(c.status) + c.status),
			}}
			cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}
			got, perr := ClaimMultiple(context.Background(), upstream.NewUpstream(client),
				testProfile(), &cred, testClock(), testRng())
			if perr != nil {
				t.Fatalf("门槛不成立不该报错: %v", perr)
			}
			if got.Kind != c.want {
				t.Errorf("实得 %v，期望 %v", got.Kind, c.want)
			}
			if client.count("/multiple") != 0 {
				t.Errorf("门槛不成立就一个请求都不该发，实得 %d 次", client.count("/multiple"))
			}
		})
	}
}

// TestPlanBackupGiftChecksCurMonthFirst：
// `curMonth` 是前置开关，`state` 是它的细分类 —— 顺序不能颠倒。
func TestPlanBackupGiftChecksCurMonthFirst(t *testing.T) {
	cases := []struct {
		name string
		gift upstream.BackupGift
		want BackupGiftPlan
	}{
		// ⭐ 这两条才是"顺序"的判据：`state` 单独看也有结论（1 = 已领），
		// 只有 `curMonth` 为假时先判它，才能把"本月压根没这个礼"与"这个月领过了"分开。
		// 少了这两条，把判序颠倒**不会变红**（已实测）。
		{"本月没礼（state 即便为 0）", upstream.BackupGift{CurMonth: false, State: 0},
			BackupGiftPlanNothingThisMonth},
		{"本月没礼（state 为 1 也不能说已领）", upstream.BackupGift{CurMonth: false, State: 1},
			BackupGiftPlanNothingThisMonth},
		{"可领", upstream.BackupGift{CurMonth: true, State: 0}, BackupGiftPlanClaim},
		{"已领", upstream.BackupGift{CurMonth: true, State: 1}, BackupGiftPlanAlreadyReceived},
		{"未开备份", upstream.BackupGift{CurMonth: true, State: -1}, BackupGiftPlanNoBackup},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanBackupGift(c.gift); got != c.want {
				t.Errorf("实得 %v，期望 %v", got, c.want)
			}
		})
	}
}

// TestClaimBackupGiftReportsFailureInsteadOfFailing：
// 这个开关的用途恰恰是"看上游到底回什么码"，抛出去就只剩一句错误信息。
func TestClaimBackupGiftReportsFailureInsteadOfFailing(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"success":true,"code":"0000","message":"请求成功",` +
			`"data":{"token":"SSO-TOKEN","resultCode":"104000"}}`,
		`{"code":0,"msg":"success","result":1789800128813}`,
		`{"code":0,"msg":"success","result":{"token":"JWT"}}`,
		`{"code":0,"msg":"success","result":{"state":0,"curMonth":true}}`,
		`{"code":1302,"msg":"未开启自动备份"}`,
	}}
	cred := SignInCredential{authorization: "Basic x", accountID: core.NewAccountID("19****01")}

	got, perr := ClaimBackupGift(context.Background(), upstream.NewUpstream(client),
		testProfile(), &cred, testClock(), testRng())
	if perr != nil {
		t.Fatalf("失败应进结果而不是返回错误: %v", perr)
	}
	if got.Kind != BackupGiftOutcomeFailed {
		t.Errorf("实得 %v，期望 Failed", got.Kind)
	}
	if !strings.Contains(got.Error, "1302") {
		t.Errorf("失败原因要带上那个码（它是唯一的线索），实得 %q", got.Error)
	}
}

// itoa64 是 [strconv.FormatInt] 之外的一个小工具 —— 本文件只用它数位数，
// 不引入依赖是为了让"16 位"这条断言读起来只跟数字本身有关。
func itoa64(v int64) string {
	if v == 0 {
		return "0"
	}
	negative := v < 0
	if negative {
		v = -v
	}
	var buf []byte
	for v > 0 {
		buf = append([]byte{byte('0' + v%10)}, buf...)
		v /= 10
	}
	if negative {
		buf = append([]byte{'-'}, buf...)
	}
	return string(buf)
}
