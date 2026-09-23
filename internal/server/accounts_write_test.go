package server

// 账号写面的三条：粘贴 auth 添加、启用开关、删除。
//
// # 这一组只钉"只有在 HTTP 面上才成立"的事
//
// 拆串与落库的字段口径在 capabilities（那边逐字段断言），窄写与连带删除在存储层。
// 这里管的是三件出网的事：
//
//   - **响应体不含凭据**：`accountPayload` 是逐字段列白的，添加与开关都走它；
//   - **错误串不回吐输入**：用户贴进来的那串是凭据本体，错误体会进浏览器历史、
//     代理日志与前端的错误缓存（`docs/` 里那条"4xx 用原文"的规矩在这里有例外：
//     原文是我们写的，但**入参**不是）；
//   - **归属错了给 404 不给 403**：403 等于向对方确认"这个 id 真实存在"。
//
// 号码一律 199 号段合成值，token 是 `SYNTH…` 假串（见 login_test.go 的同名夹具）。

import (
	"context"
	"encoding/base64"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// pastedAuth 是一串"用户从浏览器控制台里抄出来"的 auth。
//
// 连 `Basic ` 一起给：客户端本地存的就是这一整串，而 [core.ParseAuth] 会折掉
// 第一处 `Basic ` —— 带与不带必须落库落成同一份（那条由 capabilities 钉，
// 这里只保证 HTTP 面收到的形状是真实形状）。
func pastedAuth(t *testing.T) string {
	t.Helper()
	return pastedAuthFor(t, "pc", fullPhone, loginAuthToken)
}

func pastedAuthFor(t *testing.T, platform, phone, token string) string {
	t.Helper()
	raw := platform + ":" + phone + ":" + token
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(raw))
}

// maskedID 是这串凭据应当落成的账号 id（掩码手机号）。
//
// ⚠ 写成字面量而不是调 [core.MaskPhone]：这条的作用正是"id 的口径漂了要变红"，
// 拿产生它的那个函数当期望值等于什么都没测。
const maskedID = "19****00"

// addAccount 打一次"粘贴 auth 添加"，返回 recorder。
func addAccount(t *testing.T, h *harness, auth string) *httptest.ResponseRecorder {
	t.Helper()
	return h.do(t, "POST", "/api/accounts", `{"auth":`+quoteJSON(auth)+`}`, nil)
}

// quoteJSON 把一个字符串包成 JSON 字符串字面量。
//
// 只处理夹具里会出现的两类字符：这一组要往里塞 base64 与中文错误文案，
// 手写转义最容易把请求体写成非法 JSON（那会红在"请求体不合法"上，
// 而不是红在真正要测的那条判据上）。
func quoteJSON(s string) string {
	replacer := strings.NewReplacer(`\`, `\\`, `"`, `\"`)
	return `"` + replacer.Replace(s) + `"`
}

// accountRow 读界面上那一行（按 id 在列表里找）。
func accountRow(t *testing.T, h *harness, id string) (map[string]any, bool) {
	t.Helper()
	rec := h.do(t, "GET", "/api/accounts", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("列账号应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	list, _ := decode(t, rec)["accounts"].([]any)
	for _, item := range list {
		row, _ := item.(map[string]any)
		if row["id"] == id {
			return row, true
		}
	}
	return nil, false
}

// assertNoPastedAuth 是 [assertNoSecrets] 的粘贴版：连 base64 本体一起查。
//
// ⚠ base64 那一段才是"用户贴进来的东西"。`loginAuthToken` 在它里面是**编码过**的，
// 所以 assertNoSecrets 查不到它 —— 少这一条，"把入参原样回显"这种写法照样能全绿。
func assertNoPastedAuth(t *testing.T, where string, body string, auth string) {
	t.Helper()
	assertNoSecrets(t, where, body)
	for _, fragment := range []string{auth, strings.TrimPrefix(auth, "Basic ")} {
		if fragment != "" && strings.Contains(body, fragment) {
			t.Errorf("%s 回吐了用户粘贴的 auth（%q）：%s", where, fragment, brief(body))
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 添加
// ─────────────────────────────────────────────────────────────────────────────

// TestAccountWriteEndpointsRefuseAnonymousRequests：
// 三条写接口都必须先有会话。
//
// 少了这道闸，任何扫到端口的人都能往这台服务里塞账号、把别人的账号关掉或删掉。
// 夹具里预置一个账号，请求打过去之后**它必须还在** —— 只断状态码的话，
// "处理器先干活再返回 401"这种写法测不出来。
func TestAccountWriteEndpointsRefuseAnonymousRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedAccount(t, h, maskedID)

	admin := h.cookie
	h.cookie = nil
	cases := []struct{ method, path, body string }{
		{"POST", "/api/accounts", `{"auth":"x"}`},
		{"PUT", "/api/accounts/" + maskedID + "/enabled", `{"enabled":false}`},
		{"DELETE", "/api/accounts/" + maskedID, ""},
	}
	for _, c := range cases {
		rec := h.do(t, c.method, c.path, c.body, nil)
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("%s %s 应 401，实得 %d：%s", c.method, c.path, rec.Code, brief(rec.Body.String()))
		}
	}

	h.cookie = admin
	if _, found := accountRow(t, h, maskedID); !found {
		t.Error("未登录的写请求不该改动任何数据 —— 夹具里那个账号没了")
	}
}

// TestPastingAnAuthCreatesTheMaskedAccount：添加的端到端形状。
//
// 三条一起成立才算这条通过：**库里**有凭据（否则添加没意义）、
// **响应里**没有凭据（否则出网白名单形同虚设）、界面上那一行只有掩码。
func TestPastingAnAuthCreatesTheMaskedAccount(t *testing.T) {
	h := newHarness(t).initialized(t)
	auth := pastedAuth(t)

	rec := addAccount(t, h, auth)
	if rec.Code != http.StatusCreated {
		t.Fatalf("粘贴 auth 应 201，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	assertNoPastedAuth(t, "添加成功的响应", rec.Body.String(), auth)

	body := decode(t, rec)
	if body["id"] != maskedID {
		t.Errorf("id 应是掩码手机号 %q，实得 %v", maskedID, body["id"])
	}
	if body["displayName"] != maskedID {
		t.Errorf("没有昵称时展示名应落到掩码手机号，实得 %v", body["displayName"])
	}
	// 到期时间是从 token 第 4 段来的：这一条把"粘贴路径也解析到期"钉住，
	// 少了它，界面上"已过期"那一列对粘贴进来的账号永远是空的。
	if got := numField(body["authExpire"]); got != "1893456000000" {
		t.Errorf("authExpire 应是 token 里那个毫秒值，实得 %v", body["authExpire"])
	}
	if body["authExpired"] != false {
		t.Errorf("相对固定时钟应未过期，实得 %v", body["authExpired"])
	}
	if got := asText(t, body["enabled"]); got != "true" {
		t.Errorf("新账号缺省应启用，实得 %v", body["enabled"])
	}

	// 库里那份必须有凭据 —— 响应里没有不等于库里没有。
	scope := core.ScopeUser(userIDOf(t, h))
	id := core.NewAccountID(maskedID)
	// 完整手机号**只**在库里：界面上那一行永远是掩码（见 [accountPayload]）。
	state, err := h.store.GetAccountState(context.Background(), scope, id)
	if err != nil || state == nil {
		t.Fatalf("登录态没落库：%v %v", state, err)
	}
	if state.Authorization == nil || state.JWTToken == nil {
		t.Errorf("凭据没落库（authorization/jwtToken 有空的那个）：%v", state)
	}
	if state.Phone == nil || string(state.Phone.Value()) != fullPhone {
		// ⚠ 这里**不**打印 `state.Phone`：它的 Stringer 是掩码，看着像"断言在查掩码"，
		// 而完整手机号一旦进失败信息就进了测试日志（真号跑这条时会在）。
		t.Error("完整手机号应进**库**（界面上永远只有掩码）")
	}
	if _, found := accountRow(t, h, maskedID); !found {
		t.Error("添加完应在列表里看得见")
	}
}

// TestPastingAnExistingAccountIsRefusedNotMerged：
// 已存在时**拒绝**，而不是整行覆盖。
//
// 覆盖会把昵称、排序、启用状态按这份新快照写回去，而用户只想换个凭据 ——
// 更重的一条是凭据可能被降级（见 capabilities.PersistAuth 的说明）。
// 所以这里预置一个"跑过一阵子"的账号（关停用 + 起昵称），再贴一次同一串，
// 判据是**那两列还在**。
func TestPastingAnExistingAccountIsRefusedNotMerged(t *testing.T) {
	h := newHarness(t).initialized(t)
	auth := pastedAuth(t)
	scope := core.ScopeUser(userIDOf(t, h))

	if rec := addAccount(t, h, auth); rec.Code != http.StatusCreated {
		t.Fatalf("首次添加应 201，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	account, err := h.store.GetAccount(context.Background(), scope, core.NewAccountID(maskedID))
	if err != nil || account == nil {
		t.Fatalf("读账号失败：%v %v", account, err)
	}
	nickname := "主力号"
	disabled := false
	account.Nickname = &nickname
	account.Enabled = disabled
	if err := h.store.UpsertAccount(context.Background(), scope, account); err != nil {
		t.Fatalf("预置账号状态失败：%v", err)
	}

	rec := addAccount(t, h, auth)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("重复粘贴应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	assertNoPastedAuth(t, "重复粘贴的响应", rec.Body.String(), auth)

	after, err := h.store.GetAccount(context.Background(), scope, core.NewAccountID(maskedID))
	if err != nil || after == nil {
		t.Fatalf("读账号失败：%v %v", after, err)
	}
	if after.Nickname == nil || *after.Nickname != nickname {
		t.Errorf("昵称被覆盖掉了（实得 %v）—— 拒绝而不是合并才有这一条", after.Nickname)
	}
	if after.Enabled != disabled {
		t.Errorf("停用状态被覆盖回去了（实得 %v）", after.Enabled)
	}
}

// TestPasteRejectsUnusableAuthWithoutEchoingIt：
// 每一条拒绝都要给得出原因，但**不许**把入参带回去。
func TestPasteRejectsUnusableAuthWithoutEchoingIt(t *testing.T) {
	h := newHarness(t).initialized(t)

	cases := []struct{ name, body, marker string }{
		{"字段缺失", `{}`, ""},
		{"空串", `{"auth":""}`, ""},
		{"只有空白", `{"auth":"   "}`, ""},
		{"不是 base64", `{"auth":"not base64 !!!"}`, "not base64 !!!"},
		{"解出来不是三段", quoteWrap("auth", pastedAuthFor(t, "pc", "", "")), "pc::"},
		{"有手机号没 token", `{"auth":` + quoteJSON(pastedAuthFor(t, "pc", fullPhone, "")) + `}`, ""},
		{"请求体不是对象", `[1,2,3]`, ""},
	}
	for _, c := range cases {
		rec := h.do(t, "POST", "/api/accounts", c.body, nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("%s：应 400，实得 %d：%s", c.name, rec.Code, brief(rec.Body.String()))
			continue
		}
		message, _ := decode(t, rec)["message"].(string)
		if strings.TrimSpace(message) == "" {
			t.Errorf("%s：应给得出可行动的原因，实得 %s", c.name, brief(rec.Body.String()))
		}
		if c.marker != "" && strings.Contains(rec.Body.String(), c.marker) {
			t.Errorf("%s：把用户的输入回吐了（%q）：%s", c.name, c.marker, brief(rec.Body.String()))
		}
	}

	if list, _ := decode(t, h.do(t, "GET", "/api/accounts", "", nil))["accounts"].([]any); len(list) != 0 {
		t.Errorf("被拒的粘贴不该留下账号，实得 %d 个", len(list))
	}
}

func quoteWrap(key, value string) string {
	return `{"` + key + `":` + quoteJSON(value) + `}`
}

// ─────────────────────────────────────────────────────────────────────────────
// 启用开关
// ─────────────────────────────────────────────────────────────────────────────

// TestEnabledToggleOnlyTouchesEnabled：
// 这条窄写**只**动一列。
//
// ⭐ 判据是"别的列还在"而不是"这一列变了"：整行覆盖在"变了"这一面上
// 与窄写**表现完全相同**（都是 false → true），只有旁边的列能把它区分开。
// 那正是当初要新增一条窄写而不是复用 UpsertAccount 的全部理由。
func TestEnabledToggleOnlyTouchesEnabled(t *testing.T) {
	h := newHarness(t).initialized(t)
	if rec := addAccount(t, h, pastedAuth(t)); rec.Code != http.StatusCreated {
		t.Fatalf("夹具添加账号失败：%d %s", rec.Code, brief(rec.Body.String()))
	}
	scope := core.ScopeUser(userIDOf(t, h))
	id := core.NewAccountID(maskedID)

	account, err := h.store.GetAccount(context.Background(), scope, id)
	if err != nil || account == nil {
		t.Fatalf("读账号失败：%v %v", account, err)
	}
	nickname := "主力号"
	account.Nickname = &nickname
	account.SortOrder = 7
	if err := h.store.UpsertAccount(context.Background(), scope, account); err != nil {
		t.Fatalf("预置失败：%v", err)
	}

	for _, want := range []bool{false, true} {
		rec := h.do(t, "PUT", "/api/accounts/"+maskedID+"/enabled",
			`{"enabled":`+boolText(want)+`}`, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("开关应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
		}
		assertNoPastedAuth(t, "开关的响应", rec.Body.String(), pastedAuthFor(t, "pc", fullPhone, loginAuthToken))
		if got := decode(t, rec)["enabled"]; asText(t, got) != boolText(want) {
			t.Errorf("响应里的 enabled 应是 %v，实得 %v", want, got)
		}
	}

	after, err := h.store.GetAccount(context.Background(), scope, id)
	if err != nil || after == nil {
		t.Fatalf("读账号失败：%v %v", after, err)
	}
	if after.Nickname == nil || *after.Nickname != nickname {
		t.Errorf("昵称被写回去了（实得 %v）", after.Nickname)
	}
	if after.SortOrder != 7 {
		t.Errorf("排序被写回去了（实得 %d）—— 那是整行覆盖的特征", after.SortOrder)
	}
	if !after.Enabled {
		t.Error("最后那次是给 true，库里应是启用")
	}
}

func boolText(v bool) string {
	if v {
		return "true"
	}
	return "false"
}

// TestEnabledToggleRejectsAMissingField：
// `{"enabled":...}` 里那个字段**缺了**与给了 `false` 必须是两回事。
//
// 这就是 [enabledBody] 用 `*bool` 的原因：漏传一旦静默变成"停用"，
// 用户看到的是"我明明没改它，它怎么不跑了" —— 那是最难察觉的一类改动。
func TestEnabledToggleRejectsAMissingField(t *testing.T) {
	h := newHarness(t).initialized(t)
	if rec := addAccount(t, h, pastedAuth(t)); rec.Code != http.StatusCreated {
		t.Fatalf("夹具添加账号失败：%d %s", rec.Code, brief(rec.Body.String()))
	}

	for _, body := range []string{`{}`, `{"enabled":null}`, `{"enabled":"yes"}`, `{"on":true}`, `not json`} {
		rec := h.do(t, "PUT", "/api/accounts/"+maskedID+"/enabled", body, nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("请求体 %s 应 400，实得 %d：%s", body, rec.Code, brief(rec.Body.String()))
			continue
		}
		if got := decode(t, rec)["code"]; got != "bad_request" {
			t.Errorf("code 应是 bad_request，实得 %v", got)
		}
	}

	row, found := accountRow(t, h, maskedID)
	if !found {
		t.Fatal("被拒的开关请求不该删掉账号")
	}
	if got := asText(t, row["enabled"]); got != "true" {
		t.Errorf("账号应还是启用状态（漏传不等于停用），实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 删除
// ─────────────────────────────────────────────────────────────────────────────

// TestDeletingAnAccountTakesItsCredentialsWith：删除是**连带**的。
//
// 204 无体（前端已经知道那一行长什么样，再回一份"刚被删掉的东西"只会让它
// 去显示一个不存在的内容）。判据在库里：账号行与登录态行都得消失，
// 而登录态那一行才是"能冒充这台机器上这个云盘账号"的东西。
//
// 最后再贴一次同一串：能成，才算"没有残留行挡路"。
func TestDeletingAnAccountTakesItsCredentialsWith(t *testing.T) {
	h := newHarness(t).initialized(t)
	auth := pastedAuth(t)
	if rec := addAccount(t, h, auth); rec.Code != http.StatusCreated {
		t.Fatalf("夹具添加账号失败：%d %s", rec.Code, brief(rec.Body.String()))
	}
	scope := core.ScopeUser(userIDOf(t, h))
	id := core.NewAccountID(maskedID)

	rec := h.do(t, "DELETE", "/api/accounts/"+maskedID, "", nil)
	if rec.Code != http.StatusNoContent {
		t.Fatalf("删除应 204，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if rec.Body.Len() != 0 {
		t.Errorf("204 不该有响应体，实得 %q", brief(rec.Body.String()))
	}
	if _, err := h.store.GetAccount(context.Background(), scope, id); err != nil {
		t.Errorf("读已删除的账号应给 nil 而不是报错：%v", err)
	}
	if account, err := h.store.GetAccount(context.Background(), scope, id); err == nil && account != nil {
		t.Errorf("账号行还在：%v", account)
	}
	if state, err := h.store.GetAccountState(context.Background(), scope, id); err == nil && state != nil {
		t.Error("凭据行还在 —— 只删账号行等于把凭据留在库里")
	}

	if _, found := accountRow(t, h, maskedID); found {
		t.Error("列表里还能看到它")
	}
	if again := addAccount(t, h, auth); again.Code != http.StatusCreated {
		t.Errorf("删掉后重新粘贴同一串应 201，实得 %d：%s",
			again.Code, brief(again.Body.String()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属
// ─────────────────────────────────────────────────────────────────────────────

// TestSomeoneElsesAccountIsNotFoundOnEveryWritePath：
// 三条写路径上，别人的账号一律 **404**，而且**改动不了**。
//
// ⚠ 为什么不是 403：403 等于确认"这个 id 真实存在"，而账号 id 是掩码手机号
// （`19****00`）—— 猜中它的成本极低，一次探测就能确认"这台服务上有这么个人"。
// 与存储层、SSE 那条同一个口径。
func TestSomeoneElsesAccountIsNotFoundOnEveryWritePath(t *testing.T) {
	h := newHarness(t).initialized(t)
	auth := pastedAuth(t)
	if rec := addAccount(t, h, auth); rec.Code != http.StatusCreated {
		t.Fatalf("夹具添加账号失败：%d %s", rec.Code, brief(rec.Body.String()))
	}
	adminCookie := h.cookie

	h.cookie = secondUser(t, h, "bob")
	cases := []struct{ method, path, body string }{
		{"PUT", "/api/accounts/" + maskedID + "/enabled", `{"enabled":false}`},
		{"DELETE", "/api/accounts/" + maskedID, ""},
	}
	for _, c := range cases {
		rec := h.do(t, c.method, c.path, c.body, nil)
		if rec.Code != http.StatusNotFound {
			t.Errorf("%s %s 应 404（403 会泄露 id 存在），实得 %d：%s",
				c.method, c.path, rec.Code, brief(rec.Body.String()))
		}
	}
	// bob 自己贴一串：落成的必须是**他自己**名下的账号，不能撞上 admin 那一个。
	//
	// 期望是 400 而不是 409：`accounts.id` 目前是全局主键（存储层那条注释把它列成
	// 已知限制，正解是主键改成 `(owner, id)`），所以"同名"在库里根本不存在两行，
	// 报出来的是 [core.StateStore.UpsertAccount] 那句"该账号已被其他用户添加"。
	// 等到主键真改了，这一条会红 —— 那时把期望改成 201，并补一条"两行各自存在"。
	if rec := addAccount(t, h, auth); rec.Code != http.StatusBadRequest {
		t.Errorf("别人的同名账号应报 400（全局唯一 id 被占）而不是静默改动，实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	} else {
		assertNoPastedAuth(t, "同名账号的响应", rec.Body.String(), auth)
	}

	h.cookie = adminCookie
	row, found := accountRow(t, h, maskedID)
	if !found {
		t.Fatal("admin 的账号被别人的请求弄没了")
	}
	if got := asText(t, row["enabled"]); got != "true" {
		t.Errorf("admin 的账号 enabled 被别人改了，实得 %v", got)
	}
}

// TestAWriteAgainstAnUnknownIDIsNotFound：从未存在与被别人占有给**同一个**结论。
//
// 两者分开报（一个 404 一个 403）就成了探测器的读数。
func TestAWriteAgainstAnUnknownIDIsNotFound(t *testing.T) {
	h := newHarness(t).initialized(t)
	unknown := "19****99"

	rec := h.do(t, "PUT", "/api/accounts/"+unknown+"/enabled", `{"enabled":false}`, nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("不存在的 id 应 404，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rec = h.do(t, "DELETE", "/api/accounts/"+unknown, "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("删除不存在的 id 应 404，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}
