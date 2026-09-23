package server

// 刷新凭据那条写接口的用例：这一层只管**只有在 HTTP 面上才成立**的事。
//
// # 不重复断言的部分
//
// 三档判据、新串怎么拼、两端的头怎么备，在 capabilities 与 app 各自钉过。
// 这里只钉四件出网的事：
//
//   - **响应体与 URL 里都不出现凭据**（连新 token 一起查 —— 它是这次刚换出来的，
//     `assertNoPastedAuth` 那类只查"进来的那串"的守卫看不见它）；
//   - **没到点不发请求**（界面上点一下就把真凭据轮换掉，是最贵的一种误触）；
//   - **归属错了 404 且零上游调用**；
//   - **失败时库里那套原样未动**（刷新一半成功的形状比不刷更糟）。
//
// 号码与 token 一律合成值（199 号段 + `SYNTH…`，见 login_test.go 的夹具说明）。

import (
	"context"
	"encoding/base64"
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// refreshSeed 是"库里有这么个账号"的最小描述（服务端版，与 CLI 那份同形）。
//
// `since` 是 token 第 4 段相对**本包固定时钟** [fixedNow] 的偏移：判据是
// "离现在还有几天"，所以偏移必须相对一个能被测试拨动的现在，而不是绝对日期。
type refreshSeed struct {
	phone   string
	prefix  string
	since   time.Duration
	domain  bool
	device  bool
	enabled bool
}

// appDue 是"App 端、还剩 2 天、该刷了、字段齐"的那个账号。
func appDue() refreshSeed {
	return refreshSeed{
		phone: "19900000001", prefix: upstream.IOSAuthPrefix,
		since: 2 * 24 * time.Hour, domain: true, device: true, enabled: true,
	}
}

// pcNotDue 是"PC 端、还剩 30 天"的那个账号。
//
// 端别写字面量而不是引 `capabilities.PCAuthPrefix`：这条测的是"没到点不打上游"，
// 与那个常量没有依赖关系，少一根线就少一处"改了常量把测试也带改"的假绿。
func pcNotDue() refreshSeed {
	return refreshSeed{
		phone: "19900000002", prefix: "pc",
		since: 30 * 24 * time.Hour, domain: true, device: true, enabled: true,
	}
}

// seedRefreshable 把 seed 落成账号 + 登录态，返回界面上那一行的 id（掩码手机号）。
func seedRefreshable(t *testing.T, h *harness, seed refreshSeed) string {
	t.Helper()
	scope := core.ScopeUser(userIDOf(t, h))
	ctx := context.Background()

	id := core.AccountIDFor(seed.phone, nil)
	account := core.Account{ID: id, Enabled: seed.enabled, Platform: strPtr(seed.prefix)}
	// ⚠ Owner 必须给：用户 scope 下不许插无主账号（同 [seedAccount] 那句警告）。
	account.Owner = userIDOf(t, h)
	expire := fixedNow.Add(seed.since).UnixMilli()
	account.AuthExpire = &expire
	if err := h.store.UpsertAccount(ctx, scope, &account); err != nil {
		t.Fatalf("写入账号失败: %v", err)
	}

	token := refreshTokenAt(seed.since)
	authorization := core.SecretValue(authStringOfTest(seed.prefix, seed.phone, token))
	jwt := core.SecretValue(token)
	state := core.AccountState{AccountID: id, Authorization: &authorization, JWTToken: &jwt}
	if seed.domain {
		domain := core.UserDomainID("1990000000000000001")
		state.UserDomainID = &domain
	}
	if seed.device {
		device := core.SecretValue("67F4C56A00000000000000000000315C")
		state.DeviceID = &device
	}
	if err := h.store.UpsertAccountState(ctx, scope, &state); err != nil {
		t.Fatalf("写入登录态失败: %v", err)
	}
	return id.String()
}

func strPtr(s string) *string { return &s }

// refreshTokenAt 造一个第 4 段是"fixedNow + since"（**毫秒**）的合成 token。
func refreshTokenAt(since time.Duration) string {
	return "SYNTH" + strings.Repeat("A", 3) + "|1|RCS|" +
		strconv.FormatInt(fixedNow.Add(since).UnixMilli(), 10) + "|AAAA"
}

func authStringOfTest(prefix, phone, token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(prefix+":"+phone+":"+token))
}

// refreshSuccessBody 是上游那次刷新的成功响应（`data.token` 是新 token）。
//
// `expireTime` 是**秒**且是"还能用多久"，与落库的 `auth_expire`（epoch 毫秒）
// 不是一回事 —— 这里给 30 天，而新串里真正的过期时刻看 `newToken` 的第 4 段。
func refreshSuccessBody(newSince time.Duration) map[string]any {
	return map[string]any{
		"success": true, "code": "0000", "message": "请求成功",
		"data": map[string]any{
			"token": refreshTokenAt(newSince), "expireTime": 2592000,
		},
	}
}

// readAuth 从库里读回那一整串（判断"换没换"与"有没有换错人"要用它）。
func readAuth(t *testing.T, h *harness, id string) string {
	t.Helper()
	state, err := h.store.GetAccountState(
		context.Background(), core.ScopeUser(userIDOf(t, h)), core.NewAccountID(id))
	if err != nil || state == nil || state.Authorization == nil {
		t.Fatalf("读登录态失败: %v %v", state, err)
	}
	return string(*state.Authorization)
}

// ─────────────────────────────────────────────────────────────────────────────
// 成功那一支
// ─────────────────────────────────────────────────────────────────────────────

// TestRefreshAuthRotatesTheCredentialAndKeepsItOutOfTheResponse：端到端形状。
//
// 三条一起成立才算通过：库里换成了新串、响应说的是这次的结果、两边都不出凭据。
// 只断前两条的话，"顺手把新 token 也回给前端"这种写法照样全绿 ——
// 而它是这条链路上唯一一处能把**刚换出来的**凭据递出网的地方。
func TestRefreshAuthRotatesTheCredentialAndKeepsItOutOfTheResponse(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())
	oldAuth := readAuth(t, h, id)

	const newToken = "SYNTHAAA|1|RCS|9999999999999|BBBB"
	client := &scriptClient{responses: []any{map[string]any{
		"success": true, "code": "0000", "message": "请求成功",
		"data": map[string]any{"token": newToken, "expireTime": 2592000},
	}}}
	h.replaceRefresher(t, client)

	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("刷新应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 1 {
		t.Errorf("该刷的那一支应正好打一次上游，实得 %d 次", client.sent())
	}

	body := rec.Body.String()
	assertNoPastedAuth(t, "刷新的响应", body, oldAuth)
	for _, forbidden := range []string{newToken, base64.StdEncoding.EncodeToString([]byte(newToken))} {
		if strings.Contains(body, forbidden) {
			t.Errorf("响应里带着刚换出来的新 token：%s", brief(body))
		}
	}

	payload := decode(t, rec)
	refresh, _ := payload["refresh"].(map[string]any)
	if refresh == nil {
		t.Fatalf("响应里没有 refresh 这一节：%s", brief(body))
	}
	if refresh["decision"] != "needed" {
		t.Errorf("decision 应是 needed，实得 %v", refresh["decision"])
	}
	if refresh["rotated"] != true {
		t.Errorf("rotated 应是 true，实得 %v", refresh["rotated"])
	}
	if refresh["accountId"] != id {
		t.Errorf("accountId 应是掩码 id %q，实得 %v", id, refresh["accountId"])
	}
	if refresh["end"] != upstream.IOSAuthPrefix {
		t.Errorf("end 应是库里那一串的端别 %q，实得 %v", upstream.IOSAuthPrefix, refresh["end"])
	}
	// afterExpire 断成字面量：它是新 token 第 4 段那个**毫秒**值。
	// 单位一旦漂成秒，这里会红，而界面上显示的是 1970 年。
	if got := numField(refresh["afterExpire"]); got != "9999999999999" {
		t.Errorf("afterExpire 应是新 token 的过期毫秒值，实得 %v", refresh["afterExpire"])
	}

	// 回读：库里那一串真的换了，而且换的是**这个端 + 这个号**（不是拼出来的别家）
	newAuth := readAuth(t, h, id)
	if newAuth == oldAuth {
		t.Fatal("库里那串没变 —— 响应说 rotated=true 而写没落下去")
	}
	parsed, ok := core.ParseAuth(newAuth)
	if !ok {
		t.Fatalf("库里新串解不出来：%v", newAuth)
	}
	if parsed.Token == nil || *parsed.Token != newToken {
		t.Errorf("库里新串的 token 不是上游给的那一个")
	}
	if parsed.Platform == nil || *parsed.Platform != upstream.IOSAuthPrefix {
		t.Errorf("新串的端别被换了（实得 %v）—— App 串刷成 PC 串是静默故障", parsed.Platform)
	}

	// 账号那一行也跟着更新了：界面靠它显示"到哪天"，不必自己再解析一次 token
	account, _ := payload["account"].(map[string]any)
	if account == nil {
		t.Fatal("响应里没有回读后的账号那一行")
	}
	if got := numField(account["authExpire"]); got != "9999999999999" {
		t.Errorf("账号行的 authExpire 应跟着挪到新值，实得 %v", account["authExpire"])
	}
	if account["authExpired"] != false {
		t.Errorf("刷完应显示未过期，实得 %v", account["authExpired"])
	}
}

// TestRefreshAuthWithNoRequestBodyUsesTheDefaults：`curl -XPOST` 不送体也要能刷。
//
// 两个字段都可省是这条接口的**参数面**，而"整个体都不送"是它最常见的试法；
// 判据是"行为与送 {} 完全一致"，不是"别报错就行"。
func TestRefreshAuthWithNoRequestBodyUsesTheDefaults(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())

	client := &scriptClient{responses: []any{refreshSuccessBody(20 * 24 * time.Hour)}}
	h.replaceRefresher(t, client)

	// 空体：[harness.do] 连 Content-Length 都不给，正是 curl 不送体的形状
	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("不送请求体应 200（两个字段都可省），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 1 {
		t.Errorf("缺省提前量 7 天，剩 2 天的账号该刷，实得打了 %d 次", client.sent())
	}
	if refresh, _ := decode(t, rec)["refresh"].(map[string]any); refresh["rotated"] != true {
		t.Errorf("缺省参数应刷成，实得 %v", refresh)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 这一条路由的口径：**点一下就换**，提前量不参与
// ─────────────────────────────────────────────────────────────────────────────

// TestRefreshAuthRotatesEvenBeforeTheLeadWindow：还剩 30 天（远超默认 7 天提前量）
// 点一下，也必须真打上游并换成。
//
// 这就是 2026-09-22 用户报的那条障："那个 45 天没到就不刷" —— 提前量是给
// 批量扫（CLI `--all`、外部定时复用的同一条命令）省请求用的，把它摆在一枚
// 带确认弹窗的手动按钮前面，表现就是"我点了，它说还早，什么都没发生"。
//
// `decision` 仍要如实报 `not-yet`（那两个字段合起来 = "未到点但按点击换了"），
// 而真正要钉的是 `sent()>=1` 与库里那一串**变了**：只把响应改成"已换"而不动库，
// 在界面上与正解长得一模一样。
func TestRefreshAuthRotatesEvenBeforeTheLeadWindow(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, pcNotDue())
	before := readAuth(t, h, id)

	client := &scriptClient{responses: []any{refreshSuccessBody(20 * 24 * time.Hour)}}
	h.replaceRefresher(t, client)

	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("点一下应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 1 {
		t.Errorf("这一条要直接换 token，上游请求 %d 次，期望 1 次", client.sent())
	}
	payload := decode(t, rec)
	refresh, _ := payload["refresh"].(map[string]any)
	if refresh["decision"] != "not-yet" || refresh["rotated"] != true {
		t.Errorf("结论应是 not-yet + 已换（如实报档位，但确实换了），实得 %v", refresh)
	}
	if after := readAuth(t, h, id); after == before {
		t.Error("响应说换了，库里那一串却没动")
	}
	// 账号那一行也要在：前端拿到 200 会顺手更新那一行，缺了它就是显示成空白
	if _, ok := payload["account"].(map[string]any); !ok {
		t.Error("响应里少了 account 那一节")
	}
}

// TestRefreshAuthIgnoresALeadDaysField：`leadDays` 已经不是这条路由的入参了，
// 但**带了不报错**，也不参与判据（`0`、`-1`、`7` 三种都直接换）。
//
// 为什么不复用那条 400：这个字段是今天早些时候上过的形状，老前端（浏览器里缓存的
// 那版 JS）还会带它。把它判成 400 的表现是"点了刷新说参数不合法"，
// 而它对这次操作本来就没有影响。真正要挡的是**没传错**的 `platform`：
// `clientType` 写错上游会直接 401，而 401 在界面上像"凭据坏了"。
func TestRefreshAuthIgnoresALeadDaysField(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())

	for _, body := range []string{`{"leadDays":0}`, `{"leadDays":-1}`, `{"leadDays":7}`} {
		client := &scriptClient{responses: []any{refreshSuccessBody(20 * 24 * time.Hour)}}
		h.replaceRefresher(t, client)
		rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", body, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("%s 不该被拒（这一条只认直接换），实得 %d：%s",
				body, rec.Code, brief(rec.Body.String()))
		}
		if client.sent() != 1 {
			t.Errorf("%s：上游请求 %d 次，期望 1 次", body, client.sent())
		}
	}

	h.replaceRefresher(t, &scriptClient{responses: []any{refreshSuccessBody(time.Hour)}})
	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{"platform":"android"}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Errorf("不认识的 platform 应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}

// TestRefreshAuthNeedsAnAccountInTheCallersScope：别人的与从未存在的都是 404。
//
// 403 等于确认"这个掩码 id 真实存在"，而它只有 4 位可猜。
// 两条一起测，是因为分开报就成了探测器的读数。
func TestRefreshAuthNeedsAnAccountInTheCallersScope(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())
	before := readAuth(t, h, id)
	admin := h.cookie

	// 打之前先把刷新装配换成"会数次数"的那个：越权的请求连上游都不该摸到
	client := &scriptClient{responses: []any{refreshSuccessBody(20 * 24 * time.Hour)}}
	h.replaceRefresher(t, client)

	h.cookie = secondUser(t, h, "bob")
	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("别人的账号应 404（403 会泄露 id 存在），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}

	h.cookie = admin
	rec = h.do(t, "POST", "/api/accounts/19****99/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("从未存在的 id 应 404，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	if client.sent() != 0 {
		t.Errorf("越权与不存在的 id 一次上游都不该打，实得 %d 次", client.sent())
	}
	if after := readAuth(t, h, id); after != before {
		t.Error("别人的请求改动了 admin 名下账号的凭据")
	}
}

// TestRefreshAuthRefusesAnonymousRequests：这条接口不是免登录接口。
//
// 少了这道闸，任何扫到端口的人都能把这台服务上每个账号的凭据逐个轮换掉 ——
// 而刷新成功会让旧凭据**当场作废**，那比删账号更接近拒绝服务。
func TestRefreshAuthRefusesAnonymousRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())
	before := readAuth(t, h, id)

	admin := h.cookie
	h.cookie = nil
	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	h.cookie = admin

	if rec.Code != http.StatusUnauthorized {
		t.Errorf("未登录应 401，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if after := readAuth(t, h, id); after != before {
		t.Error("未登录的刷新请求改动了凭据")
	}
}

// TestRefreshAuthKeepsTheCredentialWhenTheUpstreamFails：上游失败 ⇒ 库里原样。
//
// 夹具**不**换客户端：默认那份 [refusingClient] 就是"打不出去"。
// 5xx 走类别文案（原文里实测带过上游响应体与 SQL 片段），所以这里断的是
// "message 里既没有原因原文、也没有凭据"。
func TestRefreshAuthKeepsTheCredentialWhenTheUpstreamFails(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())
	before := readAuth(t, h, id)

	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("打不出去应 500，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	assertNoPastedAuth(t, "刷新失败的响应", rec.Body.String(), before)
	if message, _ := decode(t, rec)["message"].(string); strings.Contains(message, "拒绝联网") {
		t.Errorf("5xx 把内部原文端给了使用者：%q", message)
	}
	if after := readAuth(t, h, id); after != before {
		t.Error("上游失败却动了库 —— 一半成功的刷新比不刷更糟")
	}
}

// TestRefreshAuthWithoutAUserDomainFailsBeforeDialing：缺用户域是 400，不是 502。
//
// 请求体里 `userDomainId` 是必填项，缺了它打上游只会被拒或被定位到**别人的**
// 用户域，两种都比"这次不刷"糟（判据在 [capabilities.UserDomainForRefresh]）。
// 这一条钉的是 HTTP 面那两件：状态码是可行动的原因（400 + 一句人话）、且零请求。
func TestRefreshAuthWithoutAUserDomainFailsBeforeDialing(t *testing.T) {
	h := newHarness(t).initialized(t)
	seed := appDue()
	seed.domain = false
	id := seedRefreshable(t, h, seed)

	client := &scriptClient{responses: []any{refreshSuccessBody(20 * 24 * time.Hour)}}
	h.replaceRefresher(t, client)

	rec := h.do(t, "POST", "/api/accounts/"+id+"/refresh-auth", `{}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("缺用户域应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 0 {
		t.Errorf("缺用户域时一次上游都不该打，实得 %d 次", client.sent())
	}
	message, _ := decode(t, rec)["message"].(string)
	if !strings.Contains(message, "重新登录") {
		t.Errorf("这句 4xx 要给得出可行动的下一步，实得 %q", message)
	}
}

// TestRefreshAuthIsNotReachableByGet：GET 同一路径必须落到兜底 404。
//
// 这条接口的**方法**是它语义的一部分：POST 意味着"这会改动状态"。
// 挂成 GET 的话，浏览器的预取与图片嗅探就能替用户点那个按钮。
func TestRefreshAuthIsNotReachableByGet(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedRefreshable(t, h, appDue())

	rec := h.do(t, "GET", "/api/accounts/"+id+"/refresh-auth", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("GET 这条路径应 404（写操作只认 POST），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
}
