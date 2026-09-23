package server

// 登录两条路的 HTTP 面。
//
// # 这一组测的是"形状 + 归属 + 凭据不外流"，不是协议
//
// 轮询状态机在 capabilities，驱动与会话表在 app（那边各有用例）。
// 这里只钉三件**只有在 HTTP 面上才成立**的事：
//
//   - 未登录的浏览器**用不了**这两条路（否则这台服务成了公开的发码代理）；
//   - 会话 id 是横向越权的入口 ⇒ 别人的会话读不到、图也取不到（404，与存储层同一口径）；
//   - 响应体里不许出现凭据与完整手机号 —— 错误串也不许插值用户的输入。
//
// # 每个用例只起**一个**扫码会话
//
// 后台轮询协程与脚本客户端是**争用**关系：两个会话会抢同一串响应，
// "谁拿到成功那一条"由调度决定，用例就变成了掷硬币。
// 号码一律 199 号段合成值，凭据是 SYNTH… 假串。

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 假上游与假等待器
// ─────────────────────────────────────────────────────────────────────────────

// refusingClient 对任何请求都直接报错。
//
// 本包的默认客户端必须是它：登录接口接上之后，"某条与上游无关的用例意外走到网络"
// 不再是变红而是**卡住**（先等 DNS、再等超时，扫码那条循环一轮询两分钟）。
type refusingClient struct{}

var _ core.HTTPClient = refusingClient{}

func (refusingClient) Send(context.Context, core.HTTPRequest) (core.HTTPResponse, error) {
	return core.HTTPResponse{}, core.NewInternal("测试客户端拒绝联网：这条用例不该打上游")
}

// scriptClient 按脚本依次给响应，并记下请求 URL。
//
// 队列用完了就**报错而不是重复最后一个**：那样"多打了一次上游"会表现成会话失败（响），
// 而不是"看起来一切正常"（假绿）。
type scriptClient struct {
	mu        sync.Mutex
	responses []any
	urls      []string
}

var _ core.HTTPClient = (*scriptClient)(nil)

func (c *scriptClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.urls = append(c.urls, request.URL)
	if len(c.responses) == 0 {
		return core.HTTPResponse{}, core.NewInternal("脚本已用完：这次不该再打上游")
	}
	raw, err := core.MarshalJSON(c.responses[0])
	if err != nil {
		return core.HTTPResponse{}, err
	}
	c.responses = c.responses[1:]
	return core.NewResponse(200, raw), nil
}

func (c *scriptClient) sent() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.urls)
}

func (c *scriptClient) requestURLs() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]string(nil), c.urls...)
}

// pollCappedWaiter 让轮询之间不真睡，但**次数超限就报错**。
//
// 光有"不等"不够：轮询跑在后台协程里，脚本一旦不再给终态，"不等"就退化成
// 一个把假上游打爆的死循环 —— 而那会把**整包**测试拖成一团糊。上限把它变回一行失败信息。
type pollCappedWaiter struct {
	mu    sync.Mutex
	calls int
}

// maxTestPolls 是一次登录在测试里最多被要求等几次。
//
// 真实链路最多十几次（TTL 120 秒 / 间隔 1 秒），20 留了余量：
// 正常用例碰不到它，坏用例一定被它拦住。
const maxTestPolls = 20

var _ app.Waiter = (*pollCappedWaiter)(nil)

func (w *pollCappedWaiter) Wait(context.Context, time.Duration) *core.Error {
	w.mu.Lock()
	w.calls++
	calls := w.calls
	w.mu.Unlock()
	if calls > maxTestPolls {
		return core.NewTimeout("测试里的轮询次数超过上限，停掉这一轮")
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// loginUserInfo 是登录成功时上游回的那份会话（合成值）。
func loginUserInfo() map[string]any {
	return map[string]any{
		"account":   "19900000000",
		"authToken": loginAuthToken,
		"userId":    "1990000000000000001",
	}
}

// loginAuthToken 是那份会话里**绝不能出网**的那一段。
const loginAuthToken = "SYNTH001|1|RCS|1893456000000|AAAA"

// fullPhone 是夹具里的完整手机号：它同样**不许**出现在任何响应里
// （界面上能出现的只有掩码形状，见 [core.MaskPhone]）。
const fullPhone = "19900000000"

// pcLoginEnvelope 用**生产密钥**把会话封成 `data` 密文。
//
// ⚠ 必须走真实加密：会话表用的是生产解密器（那是它的产品形态，没有"换个解密器"的缝），
// 夹具给明文会让"成功"永远到不了。
func pcLoginEnvelope(t *testing.T) map[string]any {
	t.Helper()
	wire, err := core.AESECBPKCS7EncryptHex(upstream.LoginAESKey, loginPlainText(t))
	if err != nil {
		t.Fatalf("加密会话失败: %v", err)
	}
	return loginEnvelope(wire)
}

// appLoginEnvelope 同上，走 App 端那把钥匙（含末 10 位错位）。
func appLoginEnvelope(t *testing.T) map[string]any {
	t.Helper()
	wire, perr := upstream.NewAppLoginCryptoProduction().EncryptBody(loginPlainText(t))
	if perr != nil {
		t.Fatalf("加密会话失败: %v", perr)
	}
	return loginEnvelope(wire)
}

// loginPlainText 是待加密的 `userInfo` 文本。
func loginPlainText(t *testing.T) string {
	t.Helper()
	raw, err := core.MarshalJSON(loginUserInfo())
	if err != nil {
		t.Fatalf("序列化会话失败: %v", err)
	}
	return string(raw)
}

// loginEnvelope 把密文包成上游那个形状：`data` 是**字符串**，不是嵌套对象。
func loginEnvelope(ciphertext string) map[string]any {
	return map[string]any{"success": true, "code": "0000", "data": ciphertext}
}

// qrNotScanned / qrScanned 是轮询的两个中间态响应。
func qrNotScanned() map[string]any {
	return map[string]any{"success": false, "code": "200059541", "message": "未找到设备的二维码信息"}
}

func qrScanned() map[string]any {
	return map[string]any{"success": false, "code": "200059548", "message": "二维码信息未授权"}
}

// codeSent 是 App 端发码受理的响应。
func codeSent() map[string]any {
	return map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 5}}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func stringField(t *testing.T, body map[string]any, key string) string {
	t.Helper()
	text, ok := body[key].(string)
	if !ok {
		t.Fatalf("字段 %s 不是字符串：%v（整个响应 %+v）", key, body[key], body)
	}
	return text
}

// numField 把 JSON 数字取成文本。`core.ParseJSON` 给的是 json.Number（19 位标识不丢精度）。
func numField(value any) string {
	switch v := value.(type) {
	case json.Number:
		return v.String()
	case nil:
		return ""
	}
	return "非数字"
}

// startQR 走出码那一步，返回会话 id 与二维码载荷。
func startQR(t *testing.T, h *harness) (string, string) {
	t.Helper()
	rec := h.do(t, "POST", "/api/login/qr", `{}`, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("出码应 201，实得 %d：%s", rec.Code, rec.Body.String())
	}
	body := decode(t, rec)
	return stringField(t, body, "sessionId"), stringField(t, body, "qrUrl")
}

// awaitPhase 读会话直到目标阶段。
//
// 必须**等条件而不是数次数**：驱动跑在后台协程里，"第几次轮询到第几个态"由调度决定。
func awaitPhase(t *testing.T, h *harness, path string, want app.LoginPhase) map[string]any {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	var last map[string]any
	for {
		rec := h.do(t, "GET", path, "", nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("读会话应 200，实得 %d：%s", rec.Code, rec.Body.String())
		}
		last = decode(t, rec)
		switch phase := stringField(t, last, "phase"); phase {
		case string(want):
			return last
		case string(app.LoginFailed):
			// 失败是终态，等下去只会等到同样的失败 —— 直接把它摊开
			t.Fatalf("会话进入了失败态：%+v", last)
		}
		if time.Now().After(deadline) {
			t.Fatalf("等不到 %s 态，最后看到 %+v", want, last)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

// brief 把响应体截成一行能看的片段。
//
// 二维码那一条响应是一整张 SVG（几十 KB 的 path 数据），失败信息里原样贴出来
// 会把有用的一行埋掉 —— 判据是"看了能接着查"，不是"信息全"。
func brief(body string) string {
	const keep = 200
	body = strings.ReplaceAll(body, "\n", " ")
	if len(body) <= keep {
		return body
	}
	return body[:keep] + "…（截断）"
}

// assertNoSecrets 把"响应里没有凭据、没有完整手机号"这条纪律做成一句话。
//
// `Basic ` 与两个键名是**形状**判据：凭据在库里就是这一串前缀 + 这两个字段，
// 它们出现在响应里意味着出网白名单被改坏了（见 [accountPayload]）。
func assertNoSecrets(t *testing.T, where, body string) {
	t.Helper()
	for _, forbidden := range []string{loginAuthToken, fullPhone, "Basic ", "jwtToken", `"authorization"`} {
		if strings.Contains(body, forbidden) {
			t.Errorf("%s 里出现了不该出网的东西 %q：%s", where, forbidden, brief(body))
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 闸门
// ─────────────────────────────────────────────────────────────────────────────

// TestLoginEndpointsRefuseAnonymousRequests：登录接口**不是**免登录接口。
//
// 这里的"登录"指登录移动云盘账号。若它免登录，任何一个扫到端口的人都能拿这台服务
// 发短信（连带把手机号打进上游），也能拿别人的设备指纹去出码。
func TestLoginEndpointsRefuseAnonymousRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &scriptClient{responses: []any{codeSent(), appLoginEnvelope(t)}}
	h.replaceLoginHub(t, client)
	h.cookie = nil // [harness.do] 只在 cookie 非空时带上

	for _, request := range []struct{ method, path, body string }{
		{"POST", "/api/login/qr", `{}`},
		{"GET", "/api/login/qr/whatever", ""},
		{"GET", "/api/login/qr/whatever/image.svg", ""},
		{"POST", "/api/login/sms/app", `{"phone":"` + fullPhone + `"}`},
		{"POST", "/api/login/sms/app/whatever/verify", `{"code":"119301"}`},
		{"POST", "/api/login/sms/pc", `{"phone":"` + fullPhone + `"}`},
		{"POST", "/api/login/sms/pc/whatever/verify", `{"code":"119301"}`},
	} {
		rec := h.do(t, request.method, request.path, request.body, nil)
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("%s %s 应 401，实得 %d：%s",
				request.method, request.path, rec.Code, rec.Body.String())
		}
	}
	if client.sent() != 0 {
		t.Errorf("未登录的请求一个上游都不该发，实得 %d: %v", client.sent(), client.requestURLs())
	}
}

// 中间件那道闸也要单独钉：`isPublic` 与 `isSetupAllowed` 都是**精确路径**比较，
// 以后有人为了省事把 `/api/login` 写成前缀，上面那条（走真 handler）照样 401，测不出来。
func TestLoginRoutesStayBehindTheSessionGate(t *testing.T) {
	for _, path := range []string{
		"/api/login/qr", "/api/login/qr/1", "/api/login/qr/1/image.svg",
		"/api/login/sms/app", "/api/login/sms/app/1/verify",
		"/api/login/sms/pc", "/api/login/sms/pc/1/verify",
	} {
		if isPublic(http.MethodPost, path) || isPublic(http.MethodGet, path) {
			t.Errorf("%s 被当成免登录端点了", path)
		}
		if isSetupAllowed(path) {
			t.Errorf("%s 在未初始化时该被挡住 —— 建管理员之前谁都不该能发码", path)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

func TestQRLoginServesAScannableSVGAndWalksToSuccess(t *testing.T) {
	client := &scriptClient{responses: []any{qrNotScanned(), qrScanned(), pcLoginEnvelope(t)}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	id, payload := startQR(t, h)
	// 载荷里带着会话标识与设备指纹 —— 服务端靠这两样定位这一次扫码
	if !strings.Contains(payload, "sID=") {
		t.Errorf("二维码载荷应带会话标识: %q", payload)
	}

	// 图：必须是**能直接 <img src> 的 SVG**。字符画在浏览器里不成立（等宽字体不保证），
	// 而"码扫不出来"会被用户理解成"这软件坏了"。
	image := h.do(t, "GET", "/api/login/qr/"+id+"/image.svg", "", nil)
	if image.Code != http.StatusOK {
		t.Fatalf("二维码图应 200，实得 %d：%s", image.Code, brief(image.Body.String()))
	}
	if got := image.Header().Get("Content-Type"); !strings.HasPrefix(got, "image/svg+xml") {
		t.Errorf("Content-Type 应是 image/svg+xml，实得 %q", got)
	}
	if !strings.HasPrefix(image.Body.String(), "<svg") {
		t.Errorf("响应体应是 SVG 文档：%s", brief(image.Body.String()))
	}
	assertNoSecrets(t, "二维码图", image.Body.String())

	view := awaitPhase(t, h, "/api/login/qr/"+id, app.LoginSucceeded)
	// 成功只给**掩码**形状的账号 id
	if accountID := stringField(t, view, "accountId"); !strings.HasPrefix(accountID, "19****") {
		t.Errorf("accountId 应是掩码形状，实得 %q", accountID)
	}
	raw, err := core.MarshalJSON(view)
	if err != nil {
		t.Fatalf("序列化会话结论失败: %v", err)
	}
	assertNoSecrets(t, "会话结论", string(raw))

	// 落库：账号真的出现在**这个用户**的列表里，而列表响应里没有凭据
	accounts := h.do(t, "GET", "/api/accounts", "", nil)
	if accounts.Code != http.StatusOK {
		t.Fatalf("读账号列表失败: %d", accounts.Code)
	}
	assertNoSecrets(t, "账号列表", accounts.Body.String())
	if !strings.Contains(accounts.Body.String(), "19****") {
		t.Errorf("登录成功应落一个账号，列表里没有: %s", accounts.Body.String())
	}
}

// 别的用户手里有会话 id 也读不到结论、取不到图。
//
// 会话 id 是 uuid、猜不到 —— 但"猜不到"不是判据，越权测试要**真的带着别人的 id 再来一次**。
func TestAnotherUserCannotReadSomeoneElsesLoginSession(t *testing.T) {
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, &scriptClient{responses: []any{qrNotScanned(), pcLoginEnvelope(t)}})
	id, _ := startQR(t, h)
	awaitPhase(t, h, "/api/login/qr/"+id, app.LoginSucceeded)

	mine := h.cookie
	h.cookie = secondUser(t, h, "bob")
	defer func() { h.cookie = mine }()

	if rec := h.do(t, "GET", "/api/login/qr/"+id, "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("别人的会话结论应 404，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if rec := h.do(t, "GET", "/api/login/qr/"+id+"/image.svg", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("别人的二维码图应 404，实得 %d：%s", rec.Code, rec.Body.String())
	}
	// 别人的账号也不许出现在自己的列表里
	if strings.Contains(h.do(t, "GET", "/api/accounts", "", nil).Body.String(), "19****") {
		t.Error("别人的账号进了我的列表")
	}
}

// 未知会话 / 未知端 / 空输入：三类手滑都要给 4xx，且都不许把输入带回去。
func TestLoginRequestsReportBadInputWithoutEchoingIt(t *testing.T) {
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, &scriptClient{})

	cases := []struct{ method, path, body, why string }{
		{"POST", "/api/login/qr", `{"platform":"android"}`, "端名不认识"},
		{"GET", "/api/login/qr/no-such-session", "", "会话不存在"},
		{"GET", "/api/login/qr/no-such-session/image.svg", "", "图也取不到"},
		{"POST", "/api/login/sms/app", `{"phone":"  "}`, "手机号为空"},
		{"POST", "/api/login/sms/app/no-such/verify", `{"code":"119301"}`, "会话不存在"},
		{"POST", "/api/login/sms/app/whatever/verify", `{"code":""}`, "验证码为空"},
		{"POST", "/api/login/sms/pc", `{"phone":"` + fullPhone + `","platform":"android"}`, "PC 端名不认识"},
		{"POST", "/api/login/sms/pc", `{"phone":"  "}`, "PC 手机号为空"},
		{"POST", "/api/login/sms/pc/no-such/verify", `{"code":"119301"}`, "PC 会话不存在"},
		{"POST", "/api/login/sms/pc/whatever/verify", `{"code":""}`, "PC 验证码为空"},
	}
	for _, c := range cases {
		rec := h.do(t, c.method, c.path, c.body, nil)
		if rec.Code < 400 || rec.Code > 499 {
			t.Errorf("%s %s（%s）应给 4xx，实得 %d：%s",
				c.method, c.path, c.why, rec.Code, rec.Body.String())
		}
		assertNoSecrets(t, c.why, rec.Body.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// App 端验证码
// ─────────────────────────────────────────────────────────────────────────────

func TestAppSmsLoginOverHTTPWalksToSuccess(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), appLoginEnvelope(t)}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	rec := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("发码应 201，实得 %d：%s", rec.Code, rec.Body.String())
	}
	sent := decode(t, rec)
	id := stringField(t, sent, "sessionId")
	// 上游给的配额余量要原样带出来：用户 next 一步就是"还要不要再发一次"
	if got := numField(sent["remainGetTimes"]); got != "5" {
		t.Errorf("应把配额余量带出来，实得 %v", sent["remainGetTimes"])
	}
	// 自动过滑块成了就不该有兜底地址（有的话界面会多一个没用的链接）
	if text := stringField(t, sent, "captchaUrl"); text != "" {
		t.Errorf("不该给 H5 兜底地址: %q", text)
	}
	assertNoSecrets(t, "发码响应", rec.Body.String())

	ver := h.do(t, "POST", "/api/login/sms/app/"+id+"/verify", `{"code":"119301"}`, nil)
	if ver.Code != http.StatusOK {
		t.Fatalf("填码应 200，实得 %d：%s", ver.Code, ver.Body.String())
	}
	view := decode(t, ver)
	if stringField(t, view, "phase") != string(app.LoginSucceeded) {
		t.Fatalf("应成功：%+v", view)
	}
	if accountID := stringField(t, view, "accountId"); !strings.HasPrefix(accountID, "19****") {
		t.Errorf("accountId 应是掩码形状: %q", accountID)
	}
	assertNoSecrets(t, "填码响应", ver.Body.String())

	// 发码 + 登录各一次，且第二段打的是 thirdlogin
	if client.sent() != 2 {
		t.Fatalf("应恰好两次上游，实得 %d: %v", client.sent(), client.requestURLs())
	}
	if !strings.HasSuffix(client.requestURLs()[1], "/user/thirdlogin") {
		t.Errorf("第二段应打 thirdlogin: %v", client.requestURLs())
	}
	if !strings.Contains(h.do(t, "GET", "/api/accounts", "", nil).Body.String(), "19****") {
		t.Error("登录成功应落一个账号")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// PC 端验证码
// ─────────────────────────────────────────────────────────────────────────────

// PC 那条与 App 那条**共用会话表但走两套端档案**：这里钉的是 HTTP 面上才成立的三件事
// —— 端名进得去、滑块是自动解的（所以不给兜底地址）、跨端拿会话拿不到。
func TestPcSmsLoginOverHTTPWalksToSuccess(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), pcLoginEnvelope(t)}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	rec := h.do(t, "POST", "/api/login/sms/pc",
		`{"phone":"`+fullPhone+`","platform":"macos"}`, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("PC 发码应 201，实得 %d：%s", rec.Code, rec.Body.String())
	}
	sent := decode(t, rec)
	id := stringField(t, sent, "sessionId")
	if got := numField(sent["remainGetTimes"]); got != "5" {
		t.Errorf("应把配额余量带出来，实得 %v", sent["remainGetTimes"])
	}
	// PC 端的滑块由驱动自动解掉，没有"去这个地址拖一下"那一支：
	// 给了反而会让界面摆一个没用的链接。
	if text := stringField(t, sent, "captchaUrl"); text != "" {
		t.Errorf("PC 这条路不该给 H5 兜底地址: %q", text)
	}
	assertNoSecrets(t, "PC 发码响应", rec.Body.String())

	ver := h.do(t, "POST", "/api/login/sms/pc/"+id+"/verify", `{"code":"119301"}`, nil)
	if ver.Code != http.StatusOK {
		t.Fatalf("PC 填码应 200，实得 %d：%s", ver.Code, ver.Body.String())
	}
	view := decode(t, ver)
	if stringField(t, view, "phase") != string(app.LoginSucceeded) {
		t.Fatalf("应成功：%+v", view)
	}
	if accountID := stringField(t, view, "accountId"); !strings.HasPrefix(accountID, "19****") {
		t.Errorf("accountId 应是掩码形状: %q", accountID)
	}
	assertNoSecrets(t, "PC 填码响应", ver.Body.String())

	if client.sent() != 2 {
		t.Fatalf("应恰好两次上游，实得 %d: %v", client.sent(), client.requestURLs())
	}
	if !strings.HasSuffix(client.requestURLs()[1], "/user/thirdlogin") {
		t.Errorf("第二段应打 thirdlogin: %v", client.requestURLs())
	}
	if !strings.Contains(h.do(t, "GET", "/api/accounts", "", nil).Body.String(), "19****") {
		t.Error("PC 登录成功应落一个账号")
	}
}

// 跨端拿会话必须拿不到：App 发的码不能从 PC 的填码口换出去（反之亦然）。
//
// 两条路的端档案、加密器与落库口径都不同，若这里"顺手认了"，
// 表现是"用 iPad 的会话以 Windows 客户端登录成功"—— 上游侧看是一次凭空冒出的登录。
func TestSmsSessionsDoNotCrossEnds(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), codeSent()}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	appID := stringField(t, decode(t,
		h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)), "sessionId")
	pcID := stringField(t, decode(t,
		h.do(t, "POST", "/api/login/sms/pc", `{"phone":"`+fullPhone+`"}`, nil)), "sessionId")

	before := client.sent()
	for _, c := range []struct{ path, why string }{
		{"/api/login/sms/pc/" + appID + "/verify", "拿 App 会话走 PC 填码口"},
		{"/api/login/sms/app/" + pcID + "/verify", "拿 PC 会话走 App 填码口"},
	} {
		rec := h.do(t, "POST", c.path, `{"code":"119301"}`, nil)
		if rec.Code != http.StatusNotFound {
			t.Errorf("%s 应 404，实得 %d：%s", c.why, rec.Code, rec.Body.String())
		}
		assertNoSecrets(t, c.why, rec.Body.String())
	}
	if client.sent() != before {
		t.Errorf("跨端提交打了上游（%d → %d）：会话不该被另一端换出去", before, client.sent())
	}
}

// 同一个会话重复提交必须幂等：第二次**不再打上游、不再覆盖凭据**。
//
// 前端的按钮禁用不是保证（超时重发、双击都会到第二次），
// 而第二次的代价是"再要一次验证码 + 再写一次凭据"，用户看到的却早已是成功。
func TestVerifyingTheSameSessionTwiceDoesNotSendAgain(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), appLoginEnvelope(t)}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	started := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)
	id := stringField(t, decode(t, started), "sessionId")
	if code := h.do(t, "POST", "/api/login/sms/app/"+id+"/verify", `{"code":"119301"}`, nil).Code; code != http.StatusOK {
		t.Fatalf("第一次填码应 200，实得 %d", code)
	}
	before := client.sent()

	second := h.do(t, "POST", "/api/login/sms/app/"+id+"/verify", `{"code":"119301"}`, nil)
	if second.Code != http.StatusOK {
		t.Fatalf("重复提交应仍是 200（幂等），实得 %d：%s", second.Code, second.Body.String())
	}
	if stringField(t, decode(t, second), "phase") != string(app.LoginSucceeded) {
		t.Errorf("重复提交的结论不该变：%s", second.Body.String())
	}
	if client.sent() != before {
		t.Errorf("重复提交又打了上游（%d → %d）—— 会再刷一次凭据", before, client.sent())
	}
}

// 未完成会话的配额挡在**打上游之前**。
//
// 这条测的是**顺序**而不是数值：配额若挡在发码之后，用户连点四次就真发出去四条短信
// （发码还连带一次滑块），风控照吃 —— 那正是配额存在的理由。
func TestLiveLoginSessionQuotaBlocksBeforeSending(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), codeSent(), codeSent()}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	for i := 0; i < 3; i++ {
		rec := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)
		if rec.Code != http.StatusCreated {
			t.Fatalf("第 %d 次发码应 201，实得 %d：%s", i+1, rec.Code, rec.Body.String())
		}
	}
	block := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)
	if block.Code != http.StatusBadRequest {
		t.Fatalf("超过配额应 400，实得 %d：%s", block.Code, block.Body.String())
	}
	if client.sent() != 3 {
		t.Errorf("第 4 次不该再发码，实得 %d 次上游: %v", client.sent(), client.requestURLs())
	}
	// 得给一句能行动的话：只说"太多"用户不知道要等什么
	if !strings.Contains(block.Body.String(), "未完成") {
		t.Errorf("响应要说明在等什么，实得 %s", block.Body.String())
	}
}

// 配额是**按人**算的：别人的三条未完成不该把我挡住。
func TestLoginSessionQuotaIsPerUser(t *testing.T) {
	client := &scriptClient{responses: []any{codeSent(), codeSent(), codeSent(), codeSent()}}
	h := newHarness(t).initialized(t)
	h.replaceLoginHub(t, client)

	for i := 0; i < 3; i++ {
		if rec := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil); rec.Code != http.StatusCreated {
			t.Fatalf("第 %d 次发码应 201，实得 %d：%s", i+1, rec.Code, rec.Body.String())
		}
	}
	mine := h.cookie
	defer func() { h.cookie = mine }()
	h.cookie = secondUser(t, h, "bob")

	rec := h.do(t, "POST", "/api/login/sms/app", `{"phone":"`+fullPhone+`"}`, nil)
	if rec.Code != http.StatusCreated {
		t.Errorf("别人的未完成会话不该挡住我，实得 %d：%s", rec.Code, rec.Body.String())
	}
}
