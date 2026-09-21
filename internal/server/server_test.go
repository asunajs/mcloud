package server

// 这一组测试用 **httptest** 打真实的 HTTP 栈，不走任何 mock 框架。
//
// 理由：本包的价值几乎全在"领域语义 → HTTP 形状"这段映射上 ——
// 状态码、cookie 属性、SSE 帧、错误体形状。用假的 ResponseWriter 测这些，
// 等于测"我调用了 WriteHeader"，而真正会坏的（忘了 Flush、忘了 HttpOnly、
// 5xx 把 SQL 原文带出去）一条都测不到。

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// fixedRandom 是确定性随机源。
//
// ⚠ 它不是"弱随机"：测试里的随机只用于**会话 token 与口令盐**，
// 而这两个值在断言里从来不比字面量（比的是"能登录/登不出"），
// 所以确定性不会把任何一条断言变成自证。
type fixedRandom struct{ next byte }

func (r *fixedRandom) FillBytes(dst []byte) {
	for i := range dst {
		r.next = r.next*31 + 7
		dst[i] = r.next
	}
}

func newRandom() *fixedRandom { return &fixedRandom{next: 3} }

// fixedNow 是整组测试用的"当前时刻"。
//
// 固定在**白天**：东八区的日快照窗口按 [core.LocalTZ] 切分，
// 取一个 UTC 深夜的时刻会让"今天"在两个时区里落在不同日期，
// 那样写出来的断言谁看了都觉得是协议层的锅。
var fixedNow = time.Date(2026, 9, 20, 6, 0, 0, 0, time.UTC)

func newClock() core.Clock { return core.NewFixedClock(fixedNow) }

// harness 是一台配置好的服务端 + 一个已登录的 cookie。
type harness struct {
	server *Server
	store  *mcloudio.SqliteStore
	cookie *http.Cookie
	rng    *fixedRandom
}

// newHarness 起一台**还没初始化**的服务端。
func newHarness(t *testing.T) *harness {
	t.Helper()
	store, err := mcloudio.OpenInMemory()
	if err != nil {
		t.Fatalf("开内存库失败: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	rng := newRandom()
	return &harness{
		server: New(store, newClock(), rng, Config{}),
		store:  store,
		rng:    rng,
	}
}

// initialized 走完初始化，并把 cookie 记下来。
func (h *harness) initialized(t *testing.T) *harness {
	t.Helper()
	rec := h.do(t, "POST", "/api/setup",
		`{"name":"admin","password":"correct-horse"}`, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("初始化应 201，实得 %d：%s", rec.Code, rec.Body.String())
	}
	cookies := rec.Result().Cookies()
	if len(cookies) == 0 {
		t.Fatal("初始化应顺手把会话发出去 —— 否则用户刚设完口令又被要求输一遍")
	}
	h.cookie = cookies[0]
	return h
}

// do 打一个请求，返回 recorder。
func (h *harness) do(t *testing.T, method, path, body string, headers map[string]string) *httptest.ResponseRecorder {
	t.Helper()
	var reader *strings.Reader
	if body == "" {
		reader = strings.NewReader("")
	} else {
		reader = strings.NewReader(body)
	}
	req := httptest.NewRequest(method, path, reader)
	for name, value := range headers {
		req.Header.Set(name, value)
	}
	if h.cookie != nil {
		req.AddCookie(h.cookie)
	}
	rec := httptest.NewRecorder()
	h.server.Handler().ServeHTTP(rec, req)
	return rec
}

// decode 把响应体解成 map。
func decode(t *testing.T, rec *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	value, ok := core.ParseJSON(rec.Body.String())
	if !ok {
		t.Fatalf("响应体不是合法 JSON: %q", rec.Body.String())
	}
	obj, ok := value.(map[string]any)
	if !ok {
		t.Fatalf("响应体不是 JSON 对象: %q", rec.Body.String())
	}
	return obj
}

// ─────────────────────────────────────────────────────────────────────────────
// 认证与初始化闸门
// ─────────────────────────────────────────────────────────────────────────────

// TestHandlersRefuseARequestWithoutAnIdentity：
// 处理器自己那一层（[Server.authed]）必须能在中间件没放行时拒绝。
//
// 它是**第二道**闸，留它的理由不是不信任中间件，而是"哪条路径免登录"
// 这张表将来会被改动 —— 缺了这一层，漏登记一条就等于把那个接口公开了，
// 且不会有任何测试变红。这条就是那道保险本身。
func TestHandlersRefuseARequestWithoutAnIdentity(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := httptest.NewRecorder()
	h.server.authed(func(http.ResponseWriter, *http.Request, *AuthUser) {
		t.Error("没有身份时不该进到处理器里")
	}).ServeHTTP(rec, httptest.NewRequest("GET", "/api/version", nil))

	if rec.Code != http.StatusUnauthorized {
		t.Errorf("应 401，实得 %d", rec.Code)
	}
}

func TestHealthIsTheOnlyDoorOpenWithoutASession(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := h.do(t, "GET", "/api/health", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("/api/health 应一直开着（探活用），实得 %d", rec.Code)
	}
	body := decode(t, rec)
	if body["ok"] != true {
		t.Errorf("health 应带 ok=true，实得 %v", body["ok"])
	}

	// 明确的 cookie 置空：harness 会自动带上，这里要一条"没带 cookie"的请求
	h.cookie = nil
	rec = h.do(t, "GET", "/api/version", "", nil)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("没带 cookie 的业务接口应 401，实得 %d", rec.Code)
	}
}

// TestSetupGateClosesEverythingElseUntilTheFirstAdminExists：
// 没初始化时，除初始化接口外**一律不响应**（503 而不是 401）。
//
// ⚠ 状态码必须是 503 不是 401：401 会诱导前端去弹登录框，
// 而此刻**根本没有账号可登** —— 正确动作是去走初始化向导。
func TestSetupGateClosesEverythingElseUntilTheFirstAdminExists(t *testing.T) {
	h := newHarness(t)

	rec := h.do(t, "GET", "/api/setup", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("初始化状态应一直可查，实得 %d", rec.Code)
	}
	body := decode(t, rec)
	if body["needsSetup"] != true {
		t.Errorf("空库应 needsSetup=true，实得 %v", body["needsSetup"])
	}
	// 口令下限要发给前端：那是 core 定的策略，前端硬编一份必然与它漂移
	if got := body["minPasswordLen"]; got == nil {
		t.Error("应把口令下限发给前端 —— 前端硬编一份会与后端漂移")
	}

	rec = h.do(t, "GET", "/api/version", "", nil)
	if rec.Code != http.StatusServiceUnavailable {
		t.Errorf("未初始化时应 503，实得 %d（401 会让前端弹登录框，而此刻没有账号可登）", rec.Code)
	}
	if got := decode(t, rec)["code"]; got != "setup_required" {
		t.Errorf("code 应是 setup_required，实得 %v", got)
	}
}

func TestSetupIsRejectedOnceTheFirstAdminExists(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := h.do(t, "POST", "/api/setup",
		`{"name":"second","password":"another-horse"}`, nil)
	// 它走的是与 CLI `mcloud user init` 同一个用例，那里"已经有用户"是 BadRequest
	if rec.Code != http.StatusBadRequest {
		t.Errorf("已有管理员时初始化应 400，实得 %d", rec.Code)
	}
}

// TestLogoutRevokesTheSessionImmediately：
// 登出是**删掉会话那一行**，所以下一次带同一个 cookie 的请求必须 401。
func TestLogoutRevokesTheSessionImmediately(t *testing.T) {
	h := newHarness(t).initialized(t)

	if rec := h.do(t, "GET", "/api/session", "", nil); rec.Code != http.StatusOK {
		t.Fatalf("登录态应可用，实得 %d", rec.Code)
	}

	rec := h.do(t, "DELETE", "/api/session", "", nil)
	// 204 是对的：登出没有响应体。断言写成"必须是 200"会把这个改成 200 + 空体，
	// 那才是错的方向 —— 前端按 `res.ok` 判断，多一个空 JSON 反而要它多判一层。
	if rec.Code != http.StatusNoContent {
		t.Fatalf("登出应 204，实得 %d：%s", rec.Code, rec.Body.String())
	}

	rec = h.do(t, "GET", "/api/session", "", nil)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("登出后同一个 cookie 应立刻失效，实得 %d", rec.Code)
	}
}

// TestWrongPasswordAndNoSuchUserGiveTheSameAnswer：
// 两者必须是**同一个**结论，否则登录页就是一个用户名枚举器。
func TestWrongPasswordAndNoSuchUserGiveTheSameAnswer(t *testing.T) {
	h := newHarness(t).initialized(t)
	cookie := h.cookie
	h.cookie = nil

	wrong := h.do(t, "POST", "/api/session",
		`{"name":"admin","password":"wrong-password"}`, nil)
	nobody := h.do(t, "POST", "/api/session",
		`{"name":"nobody","password":"wrong-password"}`, nil)

	if wrong.Code != http.StatusUnauthorized || nobody.Code != http.StatusUnauthorized {
		t.Fatalf("两者都应 401，实得 %d / %d", wrong.Code, nobody.Code)
	}
	wrongBody, nobodyBody := decode(t, wrong), decode(t, nobody)
	if wrongBody["code"] != nobodyBody["code"] {
		t.Errorf("code 应相同，实得 %v / %v", wrongBody["code"], nobodyBody["code"])
	}
	if wrongBody["message"] != nobodyBody["message"] {
		t.Errorf("文案应逐字相同，实得 %q / %q", wrongBody["message"], nobodyBody["message"])
	}
	// 请求里的用户名不许出现在响应里 —— 响应体会进前端的错误提示，
	// 而"admin 的口令不对"这类文案正是枚举器想要的输出
	if strings.Contains(wrong.Body.String(), "admin") {
		t.Errorf("失败响应不该回吐请求里的用户名，实得 %s", wrong.Body.String())
	}
	_ = cookie
}

// ─────────────────────────────────────────────────────────────────────────────
// cookie
// ─────────────────────────────────────────────────────────────────────────────

// TestSessionCookieIsHttpOnlyAndSameSiteLax：
// HttpOnly 挡 XSS 取 token；SameSite=Lax 挡 CSRF 里最要命的那一类跨站 POST。
func TestSessionCookieIsHttpOnlyAndSameSiteLax(t *testing.T) {
	h := newHarness(t)
	rec := h.do(t, "POST", "/api/setup",
		`{"name":"admin","password":"correct-horse"}`, nil)
	if rec.Code != http.StatusCreated {
		t.Fatalf("初始化应 201，实得 %d", rec.Code)
	}
	cookies := rec.Result().Cookies()
	cookie := cookies[0]

	if cookie.Name != CookieName {
		t.Errorf("cookie 名应稳定（前端与反向代理都按它配），实得 %q", cookie.Name)
	}
	if !cookie.HttpOnly {
		t.Error("会话 cookie 必须 HttpOnly —— 少了它，一处 XSS 就能把会话拿走")
	}
	if cookie.SameSite != http.SameSiteLaxMode {
		t.Errorf("SameSite 应 Lax，实得 %v", cookie.SameSite)
	}
	if cookie.Path != "/" {
		t.Errorf("Path 应是 /（否则 /api 之外的路径带不上），实得 %q", cookie.Path)
	}
	// ⚠ Secure 默认**关**：本地 http 下打开它等于"登录永远不生效"，
	// 而且不报错 —— 表现只是设完口令又被踢回登录页。
	if cookie.Secure {
		t.Error("默认部署是本地 http，Secure 会让 cookie 根本存不下来")
	}
}

func TestSecureCookieIsHonouredWhenDeployedBehindTLS(t *testing.T) {
	store, err := mcloudio.OpenInMemory()
	if err != nil {
		t.Fatalf("开内存库失败: %v", err)
	}
	defer func() { _ = store.Close() }()
	srv := New(store, newClock(), newRandom(), Config{SecureCookie: true})

	rec := httptest.NewRequest("POST", "/api/setup",
		strings.NewReader(`{"name":"admin","password":"correct-horse"}`))
	rec.Header.Set("Content-Type", "application/json")
	out := httptest.NewRecorder()
	srv.Handler().ServeHTTP(out, rec)
	if out.Code != http.StatusCreated {
		t.Fatalf("初始化应 201，实得 %d", out.Code)
	}
	if !out.Result().Cookies()[0].Secure {
		t.Error("显式开了 SecureCookie 就该打上 Secure —— 配了不生效比没配更坑")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 错误形状
// ─────────────────────────────────────────────────────────────────────────────

// TestUnknownAPIPathIsAJSON404NotAnIndexPage：
// `/api` 下的未知路径必须**明确报 404**，不能落到前端 index.html ——
// 那会让"路径写错"表现成"返回了一段 HTML"，前端解析失败且毫无线索。
func TestUnknownAPIPathIsAJSON404NotAnIndexPage(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := h.do(t, "GET", "/api/nope", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("应 404，实得 %d", rec.Code)
	}
	if got := rec.Header().Get("Content-Type"); !strings.Contains(got, "application/json") {
		t.Errorf("Content-Type 应是 JSON，实得 %q", got)
	}
	if body := rec.Body.String(); strings.Contains(body, "<html") {
		t.Errorf("不该回一段 HTML，实得 %s", body)
	}
	if got := decode(t, rec)["code"]; got != "not_found" {
		t.Errorf("code 应是 not_found，实得 %v", got)
	}
}

// TestEveryErrorIsTheSameShape：
// 前端按 `{code, message}` 写错误处理，拿到别的东西只能显示"HTTP 4xx"。
func TestEveryErrorIsTheSameShape(t *testing.T) {
	h := newHarness(t).initialized(t)

	cases := []struct {
		name   string
		method string
		path   string
		body   string
	}{
		{"请求体不是 JSON", "POST", "/api/session", `not json`},
		{"请求体不是对象", "POST", "/api/session", `[1,2,3]`},
		{"字段类型不对", "POST", "/api/session", `{"name":123,"password":"x"}`},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			rec := h.do(t, c.method, c.path, c.body, nil)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("应 400，实得 %d：%s", rec.Code, rec.Body.String())
			}
			body := decode(t, rec)
			if got := body["code"]; got != "bad_request" {
				t.Errorf("code 应是 bad_request，实得 %v", got)
			}
			// message 必须是**可行动**的说明（缺了哪个字段），不是一句"参数错误"
			message, _ := body["message"].(string)
			if strings.TrimSpace(message) == "" {
				t.Errorf("message 不该是空的，实得 %v", body["message"])
			}
		})
	}
}

// TestInternalErrorsDoNotLeakDetails：
// 5xx 一律换成分类的公开文案 —— 那些原文里实测带过 SQL 片段与上游响应体。
func TestInternalErrorsDoNotLeakDetails(t *testing.T) {
	secret := "SQLITE_CONSTRAINT_something_internal"
	err := ErrDomain(core.NewInternal(secret))
	if err.Status() != http.StatusInternalServerError {
		t.Fatalf("internal 应 500，实得 %d", err.Status())
	}
	if strings.Contains(err.Message(), secret) {
		t.Errorf("5xx 不该带内部原文，实得 %q", err.Message())
	}
	if err.Message() != core.KindInternal.PublicMessage() {
		t.Errorf("5xx 应换成分类文案，实得 %q", err.Message())
	}
	// 4xx 反过来：那时要给**可行动**的原因
	client := ErrDomain(core.NewBadRequest("缺 deviceId"))
	if client.Status() != http.StatusBadRequest {
		t.Fatalf("bad_request 应 400，实得 %d", client.Status())
	}
	if client.Message() != "缺 deviceId" {
		t.Errorf("4xx 应原样给出原因，实得 %q", client.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 只读业务接口
// ─────────────────────────────────────────────────────────────────────────────

// TestVersionAndCapabilitiesComeFromTheSharedPayload：
// 版本与能力清单的 payload 住在 app 层（CLI 也打同一张表），
// 这里只验证"HTTP 那一层递出去的就是它"。
func TestVersionAndCapabilitiesComeFromTheSharedPayload(t *testing.T) {
	h := newHarness(t).initialized(t)

	rec := h.do(t, "GET", "/api/version", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	version := decode(t, rec)
	if version["name"] != core.ProductName {
		t.Errorf("应带产品名，实得 %v", version["name"])
	}
	if version["version"] != core.Version {
		t.Errorf("version 应来自 core.Version，实得 %v", version["version"])
	}

	rec = h.do(t, "GET", "/api/capabilities", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	body := decode(t, rec)
	list, ok := body["all"].([]any)
	if !ok || len(list) == 0 {
		t.Fatalf("能力清单不该是空的，实得 %v", body)
	}
	// 三个派生列表也得出网：前端的"可批量"勾选框按 batchable 渲染，
	// 缺了它的表现是勾选框全灰，而原因看起来像"这批账号不支持"
	for _, key := range []string{"visible", "batchable", "groups"} {
		if _, found := body[key]; !found {
			t.Errorf("缺 %q，实得 %v", key, body)
		}
	}
}

// TestAccountPayloadOmitsTheOwnerAndAnyCredential：
// 出网形状是**逐字段列出**的，新增字段默认不出网。
// 这里从响应体反查：任何带 owner / 凭据味道的键都不许出现。
func TestAccountPayloadOmitsTheOwnerAndAnyCredential(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, "A1")

	rec := h.do(t, "GET", "/api/accounts", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	body := decode(t, rec)
	list, ok := body["accounts"].([]any)
	if !ok || len(list) != 1 {
		t.Fatalf("应有 1 个账号，实得 %v", body["accounts"])
	}
	account, _ := list[0].(map[string]any)
	for _, forbidden := range []string{"owner", "authorization", "auth", "token", "password"} {
		if _, found := account[forbidden]; found {
			t.Errorf("账号的对外形状不该含 %q —— 出网字段是逐字段列白的，实得 %v",
				forbidden, account)
		}
	}
	// 反过来：该有的展示字段一个都不能少（前端直接读它渲染那张表）
	for _, required := range []string{"id", "displayName", "enabled", "authExpired"} {
		if _, found := account[required]; !found {
			t.Errorf("缺 %q，实得 %v", required, account)
		}
	}
}

// TestAccountsAreScopedToTheCaller：
// 网络路径上 A 不能看到 B 的账号。这是 [core.ScopeUser] 存在的全部理由。
func TestAccountsAreScopedToTheCaller(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, "A1")

	other := secondUser(t, h, "bob")
	h.cookie = other
	rec := h.do(t, "GET", "/api/accounts", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	list, _ := decode(t, rec)["accounts"].([]any)
	if len(list) != 0 {
		t.Errorf("别的用户不该看到 admin 的账号，实得 %d 个", len(list))
	}
}

// TestRunLimitIsClampedNotRejected：
// `?limit=0` 与 `?limit=99999` 都是使用者的手滑，为了一个手滑把整份列表
// 变成 400，代价不匹配。
func TestRunLimitIsClampedNotRejected(t *testing.T) {
	h := newHarness(t).initialized(t)

	zero := h.do(t, "GET", "/api/runs?limit=0", "", nil)
	if zero.Code != http.StatusOK {
		t.Fatalf("limit=0 应夹紧而不是拒绝，实得 %d", zero.Code)
	}
	if got := decode(t, zero)["limit"]; got == nil || asText(t, got) != "1" {
		t.Errorf("limit=0 应夹成 1，实得 %v", got)
	}

	huge := h.do(t, "GET", "/api/runs?limit=99999", "", nil)
	if got := decode(t, huge)["limit"]; asText(t, got) != "200" {
		t.Errorf("limit 应有上限，实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// SSE 事件流
// ─────────────────────────────────────────────────────────────────────────────

// sseServer 起一个**真的** HTTP 服务器（httptest.NewServer），
// 因为 [http.ResponseRecorder] 不实现 [http.Flusher]，而 SSE 全靠它。
func sseServer(t *testing.T, h *harness) *httptest.Server {
	t.Helper()
	ts := httptest.NewServer(h.server.Handler())
	t.Cleanup(ts.Close)
	return ts
}

// sseGet 打一次 SSE 请求，返回**全部**响应体。
//
// 它只在流自然结束时才返回 —— 所以夹具里的事件必须有终局事件，
// 否则这条会一直挂到 ctx 超时（那本身也是个信号：没有终局事件 = 连接不关）。
func sseGet(t *testing.T, h *harness, ts *httptest.Server, path string) string {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", ts.URL+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	if h.cookie != nil {
		req.AddCookie(h.cookie)
	}
	resp, err := ts.Client().Do(req)
	if err != nil {
		t.Fatalf("请求失败: %v", err)
	}
	defer func() { _ = resp.Body.Close() }()
	return readAll(t, resp.Body)
}

func readAll(t *testing.T, r interface{ Read([]byte) (int, error) }) string {
	t.Helper()
	var out strings.Builder
	buf := make([]byte, 4096)
	for {
		n, err := r.Read(buf)
		out.Write(buf[:n])
		if err != nil {
			break
		}
		if out.Len() > 1<<20 {
			t.Fatal("响应体过大，怀疑流没有终止")
		}
	}
	return out.String()
}

// TestRunEventsReplaysFromTheDatabaseAndEndsAtTheTerminalEvent：
// 进程重启后总线是空的，历史只能来自库；且**终局事件之后响应就结束** ——
// 不结束的话每个打开的页面都会挂着一个永久任务。
func TestRunEventsReplaysFromTheDatabaseAndEndsAtTheTerminalEvent(t *testing.T) {
	h := newHarness(t).initialized(t)
	runID := seedRun(t, h, true)

	ts := sseServer(t, h)
	body := sseGet(t, h, ts, "/api/runs/"+string(runID)+"/events")

	if !strings.HasPrefix(body, "id: 1\n") {
		t.Errorf("第一条应是 seq=1，实得 %q", body)
	}
	if !strings.Contains(body, "data: ") {
		t.Errorf("SSE 应有 data 行，实得 %q", body)
	}
	// 终局事件必须出现，且它之后没有别的事件
	if !strings.Contains(body, `"finished"`) {
		t.Errorf("应含终局事件，实得 %q", body)
	}
	if !strings.HasSuffix(body, "\n\n") {
		t.Errorf("流应以一个完整的帧结束，实得 %q", body)
	}
}

// TestRunEventsHonoursTheAfterCursor：
// `after` 是"客户端**已经看到**的那一条"，不是"下一条要取的值"。
// 语义搞反会让每次重连都重发一条（或漏一条）。
func TestRunEventsHonoursTheAfterCursor(t *testing.T) {
	h := newHarness(t).initialized(t)
	runID := seedRun(t, h, true)
	ts := sseServer(t, h)

	body := sseGet(t, h, ts, "/api/runs/"+string(runID)+"/events?after=1")
	if strings.Contains(body, "id: 1\n") {
		t.Errorf("after=1 表示第 1 条已经看过，不该再发，实得 %q", body)
	}
	if !strings.Contains(body, "id: 2\n") {
		t.Errorf("第 2 条应发出来，实得 %q", body)
	}
}

// TestEventsOfSomeoneElsesRunAreNotFoundNotForbidden：
// 归属错误**不能**报成 403 —— 那等于确认了"这个 id 确实存在"。
func TestEventsOfSomeoneElsesRunAreNotFoundNotForbidden(t *testing.T) {
	h := newHarness(t).initialized(t)
	runID := seedRun(t, h, true)
	h.cookie = secondUser(t, h, "bob")

	ts := sseServer(t, h)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET",
		ts.URL+"/api/runs/"+string(runID)+"/events", nil)
	req.AddCookie(h.cookie)
	resp, err := ts.Client().Do(req)
	if err != nil {
		t.Fatalf("请求失败: %v", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusNotFound {
		t.Errorf("别人的 run 应 404（403 会泄露 id 存在），实得 %d", resp.StatusCode)
	}
}

// TestAnUnparseableRunIDIsNotFoundWithoutTouchingTheStore：
// 形状不对的字符串不可能是我们生成的，所以"不存在"是最贴切的结论。
func TestAnUnparseableRunIDIsNotFoundWithoutTouchingTheStore(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "GET", "/api/runs/not-a-uuid/events", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("应 404，实得 %d", rec.Code)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务
// ─────────────────────────────────────────────────────────────────────────────

// TestScheduleRejectsACronThatNeverFires：
// "2 月 30 日"这种 cron 能解析但永远不触发 —— 存下去不会报任何错，
// 只会每天安静地什么都不做。
func TestScheduleRejectsACronThatNeverFires(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "PUT", "/api/schedules/never",
		`{"name":"永不触发","cron":"0 0 30 2 *","target":{"kind":"capability","code":"sign"}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if got := decode(t, rec)["code"]; got != "bad_request" {
		t.Errorf("code 应是 bad_request，实得 %v", got)
	}
}

func TestScheduleRejectsAnUnparseableCron(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "PUT", "/api/schedules/broken",
		`{"name":"坏的","cron":"not a cron","target":{"kind":"capability","code":"sign"}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("应 400，实得 %d", rec.Code)
	}
}

// TestScheduleRoundTripsAndIsScopedToItsOwner：
// 存下去要能读回来，且 owner 记的是**当前登录的人**而不是请求体里给的。
func TestScheduleRoundTripsAndIsScopedToItsOwner(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "PUT", "/api/schedules/daily",
		`{"name":"每日签到","cron":"0 9 * * *","target":{"kind":"capability","code":"sign"}}`, nil)
	if rec.Code != http.StatusOK && rec.Code != http.StatusCreated {
		t.Fatalf("保存应成功，实得 %d：%s", rec.Code, rec.Body.String())
	}

	rec = h.do(t, "GET", "/api/schedules", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	list, _ := decode(t, rec)["schedules"].([]any)
	if len(list) != 1 {
		t.Fatalf("应有 1 条，实得 %v", list)
	}
	first, _ := list[0].(map[string]any)
	if first["id"] != "daily" {
		t.Errorf("id 应原样是 daily，实得 %v", first["id"])
	}
	// 缺省启用：会走到这一步的人都是"特意建了一条定时任务"
	if first["enabled"] != true {
		t.Errorf("缺省应启用，实得 %v", first["enabled"])
	}

	// 别人的列表里不该有它
	h.cookie = secondUser(t, h, "bob")
	rec = h.do(t, "GET", "/api/schedules", "", nil)
	list, _ = decode(t, rec)["schedules"].([]any)
	if len(list) != 0 {
		t.Errorf("定时任务属于创建者，实得 %d 条", len(list))
	}
}

func TestScheduleIsRejectedWhenTheTargetDoesNotExist(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "PUT", "/api/schedules/ghost",
		`{"name":"指向不存在的能力","cron":"0 9 * * *",`+
			`"target":{"kind":"capability","code":"no-such-capability"}}`, nil)
	if rec.Code != http.StatusBadRequest && rec.Code != http.StatusNotFound {
		t.Errorf("目标不存在应拒绝，实得 %d：%s", rec.Code, rec.Body.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// seedAccount 往**当前登录者**的名下塞一个账号。
func seedAccount(t *testing.T, h *harness, id string) {
	t.Helper()
	account := core.NewAccount(id)
	// ⚠ 必须设 Owner：用户 scope 下不许插无主账号（那条守卫在存储层，是对的 ——
	// 无主账号只有 schema 迁移与本地运维才该产生）。
	account.Owner = userIDOf(t, h)
	if err := h.store.UpsertAccount(context.Background(),
		core.ScopeUser(account.Owner), &account); err != nil {
		t.Fatalf("塞账号失败: %v", err)
	}
}

// userIDOf 取当前 cookie 对应的用户 id。
func userIDOf(t *testing.T, h *harness) *core.UserID {
	t.Helper()
	if h.cookie == nil {
		t.Fatal("这条 helper 需要一个已登录的 cookie")
	}
	rec := h.do(t, "GET", "/api/session", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("取当前身份失败: %d", rec.Code)
	}
	body := decode(t, rec)
	user, _ := body["user"].(map[string]any)
	raw, _ := user["id"].(string)
	if raw == "" {
		t.Fatalf("身份里没有 id，实得 %v", body)
	}
	id := core.NewUserID(raw)
	return &id
}

// secondUser 建第二个用户并登录，返回它的会话 cookie。
func secondUser(t *testing.T, h *harness, name string) *http.Cookie {
	t.Helper()
	// ⚠ 随机源要**按名字错开**：每次都从同一个种子起会让会话 token 撞车
	// （sessions.token_hash 是唯一的），而报错是"UNIQUE constraint failed"，
	// 看上去像存储层的锅 —— 真实原因是夹具自己退化成了常量。
	rng := &fixedRandom{next: byte(len(name)*17 + 5)}
	// ⚠⚠ §6.1 #38 的第二个病例：这里**不能**让两个调用共用一个 `perr`。
	// `HashPassword` 返回 `error`（接口），`SignIn` 返回 `*core.Error`；
	// 同一个作用域里后者只是**赋值**，于是成功的 nil `*core.Error`
	// 被塞进 `error` 接口 ⇒ `perr != nil` **恒真**，而 `%v` 打印 `<nil>`。
	// 我第一次就踩了它，报错是"登录失败: <nil>" —— 一个线索都没有。
	hash, herr := core.HashPassword("another-horse", rng)
	if herr != nil {
		t.Fatalf("哈希失败: %v", herr)
	}
	user := core.NewUser(name, name, hash, core.RoleUser, fixedNow)
	if err := h.store.UpsertUser(context.Background(), &user); err != nil {
		t.Fatalf("建用户失败: %v", err)
	}
	_, token, _, sperr := app.SignIn(context.Background(), h.store,
		name, "another-horse", rng, fixedNow)
	if sperr != nil {
		t.Fatalf("登录失败: %v", sperr)
	}
	return &http.Cookie{Name: CookieName, Value: token}
}

// seedRun 往**当前登录者**名下塞一个 run 加三条事件；terminal 控制最后一条
// 是不是终局事件（不是终局的流不会自己结束，测试里别这么造）。
func seedRun(t *testing.T, h *harness, terminal bool) core.RunID {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	runID := core.NewRunID(newRandom())
	accountID := core.NewAccountID("A1")
	now := fixedNow
	account := core.NewAccount("A1")
	account.Owner = scope.Owner()
	if err := h.store.UpsertAccount(ctx, scope, &account); err != nil {
		t.Fatalf("塞账号失败: %v", err)
	}
	record := core.RunRecord{
		ID:         string(runID),
		AccountID:  &accountID,
		Capability: "sign",
		Status:     core.StatusRunning,
		StartedAt:  now,
	}
	if err := h.store.InsertRun(ctx, scope, &record); err != nil {
		t.Fatalf("塞 run 失败: %v", err)
	}
	events := []core.Sequenced{
		core.NewSequenced(1, core.NewEventStarted(runID, "sign")),
		core.NewSequenced(2, core.NewEventLog(runID, core.LevelInfo, "正在签到")),
	}
	if terminal {
		events = append(events, core.NewSequenced(3,
			core.NewEventFinished(runID, core.OutcomeSuccess, nil, nil)))
	}
	if err := h.store.AppendRunEvents(ctx, scope, runID, events); err != nil {
		t.Fatalf("塞事件失败: %v", err)
	}
	return runID
}

// asText 把 JSON 里的值转成文本再比。
//
// ⚠ 这是 §6.1 #34 的老规矩：[core.ParseJSON] 为"19 位标识不丢精度"返回
// `json.Number`，直接跟 `float64(1)` 比会得到"实得 1，期望 1"这种
// 看不出哪里不同的报错。
func asText(t *testing.T, value any) string {
	t.Helper()
	return fmt.Sprint(value)
}

// TestUnknownTimestampsAreNullNotTheEmptyString：
// 「没有这个时间」必须是 JSON 的 `null`，不能是空串。
//
// ⭐ 反向测试先暴露了这里是个缺口：把 [rfc3339Of] 的 nil 分支改成返回 `""`，
// **一条测试都没变红** —— 因为夹具里的账号都是有跑过记录的，两个时间列都有值。
// 而真实场景里「刚加进来还没跑过」的账号占了绝大多数，那两列正是 nil。
//
// 空串在前端是「有一个时间，只是它是空」，`null` 才是「没有这个时间」——
// 在「上次跑成没成」那张表上这两者意思相反。
func TestUnknownTimestampsAreNullNotTheEmptyString(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, "A1") // 刚加进来，从没跑过

	rec := h.do(t, "GET", "/api/accounts", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	list, ok := decode(t, rec)["accounts"].([]any)
	if !ok || len(list) != 1 {
		t.Fatalf("应有 1 个账号，实得 %v", decode(t, rec)["accounts"])
	}
	account, _ := list[0].(map[string]any)

	// `nil` 解出来是 nil，`""` 解出来是空串 —— 两者在 map 里**都存在**，
	// 所以必须判值本身，不能用「键在不在」这种写法。
	for _, key := range []string{"cloudUpdatedAt", "lastOkAt"} {
		value, found := account[key]
		if !found {
			t.Errorf("缺 %q —— 键本身就该在（前端按它判空），实得 %v", key, account)
			continue
		}
		if value != nil {
			t.Errorf("%q 应是 JSON 的 null，实得 %#v（类型 %T）", key, value, value)
		}
	}
}

// TestAnEmptyCookieIsNotASession：
// `mcloud_session=` 这种**空值** cookie 不能被当成一次登录。
//
// ⚠ 这条是**双保险**：[app.CurrentByToken] 自己也会拦空 token（那条由 app 包的
// 用例钉住），所以 HTTP 层这一层删掉**不会让本包的任何测试变红**（反向测试已确认）。
// 留着的理由是「纵深防御」而不是「这里才是判据」—— 别把它当成唯一防线。
func TestAnEmptyCookieIsNotASession(t *testing.T) {
	h := newHarness(t).initialized(t)
	h.cookie = &http.Cookie{Name: "mcloud_session", Value: ""}
	rec := h.do(t, "GET", "/api/session", "", nil)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("空值 cookie 应 401，实得 %d：%s", rec.Code, rec.Body.String())
	}
}

// TestTheStreamClosesPromptlyAtTheTerminalEvent：
// 终局事件之后，连接要**立刻**关掉 —— 不能挂着等超时。
//
// ⭐ 反向测试先暴露了这里是个缺口：把 `pumpEvents` 里那个 `if terminal { return }`
// 删掉，其余测试**一条都没变红** —— 它们只比内容，而内容在超时前后是一样的。
// 于是「流不关」这个 bug 的全部表现就是：每次看完一个已完成的 run，
// 浏览器那个 EventSource 都还要再转五秒才显示"连接已关闭"。
//
// ⇒ 判据只能是**耗时**：流关闭的时刻，不是流的内容。
func TestTheStreamClosesPromptlyAtTheTerminalEvent(t *testing.T) {
	h := newHarness(t).initialized(t)
	runID := seedRun(t, h, true)
	ts := sseServer(t, h)

	// ctx 给 5 秒：它只用来兜底（真出问题时测试会超时而不是永远挂住），
	// 正常路径根本用不到它。
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET",
		ts.URL+"/api/runs/"+string(runID)+"/events", nil)
	if err != nil {
		t.Fatal(err)
	}
	req.AddCookie(h.cookie)

	start := time.Now()
	resp, err := ts.Client().Do(req)
	if err != nil {
		t.Fatalf("请求失败: %v", err)
	}
	defer func() { _ = resp.Body.Close() }()
	body := readAll(t, resp.Body)
	elapsed := time.Since(start)

	if !strings.Contains(body, `"finished"`) {
		t.Fatalf("夹具应有终局事件，实得 %q", body)
	}
	// 阈值取 2 秒：正常路径是毫秒级，挂着不关则是整整 5 秒 —— 差三个数量级。
	if elapsed > 2*time.Second {
		t.Errorf("终局事件之后流应立刻关闭，实得 %v\n"+
			"不关的表现是浏览器那个 EventSource 一直显示「连接中」，而日志其实早就完了",
			elapsed)
	}
}
