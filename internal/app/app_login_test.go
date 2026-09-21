package app

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 本文件对应 Rust 侧 legacy/crates/mcloud-app/src/app_login.rs 的 `mod tests`。
//
// 设备指纹与号码都是**合成值**（`AAA…` / `199` 号段），不含真实标识。

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// appProfile 是抓包里那台设备的脱敏形态（与 app_client 的单测同一份）。
func appProfile() *upstream.AppProfile {
	return upstream.NewAppProfile(
		upstream.NewIPadAppDevice("AAA00000000000000000000000000000", "AAA000000000"))
}

// appSessionPayload 是抓包里那个会话载荷的**形状**（脱敏值），
// 字段名与嵌套层级照抄实测。
func appSessionPayload() map[string]any {
	return map[string]any{
		"account":      "19900000000",
		"areaCode":     "023",
		"atExpiretime": 2592000,
		"expiretime":   "2592000",
		"authToken":    "SYNTH001|1|RCS|1893456000000|AAAA",
		"deviceid":     "AAA00000000000000000000000000000",
		"nationCode":   "+86",
		"userDomainId": "1990000000000000001",
		"userid":       "199000000000",
		"userExtInfo":  []any{map[string]any{"passID": "1990000000"}},
	}
}

// appEncryptedSession 用真实实现把一个会话载荷封成上游会回的 `data` 密文。
func appEncryptedSession(t *testing.T, crypto *upstream.AppLoginCrypto) string {
	t.Helper()
	plain, err := core.MarshalJSON(appSessionPayload())
	if err != nil {
		t.Fatalf("序列化会话载荷失败: %v", err)
	}
	wire, perr := crypto.EncryptBody(string(plain))
	if perr != nil {
		t.Fatalf("加密是纯计算，不该失败: %v", perr)
	}
	return wire
}

// appPuzzleEnvelope 是真实滑块挑战（2026-09-18 实测响应原文）：
// 尺寸是**字符串**，`puzzleLeft` 用公开常量解出来是 `370`。
func appPuzzleEnvelope() map[string]any {
	return map[string]any{
		"success": true, "code": "0000",
		"data": map[string]any{
			"puzzle":      "iVBORw0KGgoAAAANSUhEUgAA",
			"puzzleWidth": "96",
			"picture":     "/9j/4AAQSkZJRgABAgAAAQAB",
			"picWidth":    "680",
			"picHeight":   "400",
			"picType":     "png",
			"puzzleLeft":  "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg=",
		},
	}
}

func appLoginContext(code string) AppLoginContext {
	return AppLoginContext{
		Scope:      loginScope(),
		Profile:    appProfile(),
		Credential: SmsCredential{Phone: "19900000000", Code: code},
	}
}

func headerOf(request core.HTTPRequest, name string) (string, bool) {
	for _, h := range request.Headers {
		if strings.EqualFold(h[0], name) {
			return h[1], true
		}
	}
	return "", false
}

// ─────────────────────────────────────────────────────────────────────────────
// 发码
// ─────────────────────────────────────────────────────────────────────────────

// 被要求滑块 → **自动解出答案**并重发。**全程不开浏览器**。
//
// 这是这一层的核心断言：App 端不再把 H5 地址甩给用户。
func TestAppSendSmsCodeSolvesTheSliderAutomatically(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059554", "message": "请先获取滑块拼图验证码",
			"data": map[string]any{"captchaType": 2}},
		appPuzzleEnvelope(),
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 7}},
	}}

	outcome, perr := AppSendSmsCode(
		context.Background(), upstream.NewUpstream(client), appProfile(),
		"19900000000", loginFixedClock())
	if perr != nil {
		t.Fatalf("走一次滑块后应发码成功: %v", perr)
	}
	if outcome.Kind != AppCodeSent {
		t.Fatalf("第二次就该发出去，实得 %v", outcome.Kind)
	}
	if outcome.RemainGetTimes == nil || *outcome.RemainGetTimes != 7 {
		t.Errorf("配额余量应是 7，实得 %v", outcome.RemainGetTimes)
	}
	if !outcome.SolvedSlider {
		t.Error("应当走了一次滑块")
	}

	// 三次请求，顺序必须是：发码 → 取图 → 带答案重发
	urls := client.urls()
	if len(urls) != 3 {
		t.Fatalf("应是 发码 → 取图 → 带答案重发，实得 %d 次: %v", len(urls), urls)
	}
	if !strings.HasSuffix(urls[0], "/user/sms/getSmsCode") ||
		!strings.HasSuffix(urls[1], "/user/verfycode") ||
		!strings.HasSuffix(urls[2], "/user/sms/getSmsCode") {
		t.Errorf("请求顺序不对: %v", urls)
	}

	// 取图是**明文**请求，且**不带 Authorization**（实测裸请求即可用）
	puzzleBody := string(client.requests[1].Body)
	if !strings.Contains(puzzleBody, "19900000000") {
		t.Errorf("取图请求体应带手机号: %q", puzzleBody)
	}
	if !strings.Contains(puzzleBody, "\"type\":2") {
		t.Errorf("取图请求体应带 type:2: %q", puzzleBody)
	}
	if _, has := headerOf(client.requests[1], "authorization"); has {
		t.Error("取图请求不该带 Authorization（实测裸请求即可用）")
	}

	// 最强的一条：把我们**真的发出去**的密文解回来，必须是 370
	body := client.bodyOf(t, 2)
	encoded, _ := body["puzzleVerfycode"].(string)
	plain, decErr := core.AESDynamicIVDecrypt(upstream.PuzzleAESKey, encoded)
	if decErr != nil {
		t.Fatalf("解不开自己发出去的答案: %v", decErr)
	}
	if strings.TrimSpace(plain) != "370" {
		t.Errorf("发出去的坐标应是 370，实得 %q", plain)
	}

	// 首次那一条仍是空串（照抄实测，不是省略字段）
	first := client.bodyOf(t, 0)
	if value, _ := first["puzzleVerfycode"].(string); value != "" {
		t.Errorf("首次的 puzzleVerfycode 应是空串，实得 %q", value)
	}

	// 不发 hcy-cool-flag：发了响应会变成解不开的密文
	if _, has := headerOf(client.requests[0], "hcy-cool-flag"); has {
		t.Error("发了 hcy-cool-flag 会让响应变成解不开的密文")
	}
	// 时间戳是**微秒**（16 位）
	ts, has := headerOf(client.requests[0], "x-yun-user-timestamp")
	if !has {
		t.Fatal("缺 x-yun-user-timestamp")
	}
	if len(ts) != 16 {
		t.Errorf("x-yun-user-timestamp 应是微秒（16 位），实得 %q", ts)
	}
}

// `200059553`（挑战作废）→ **必须重新取图**，不能复用旧答案。
func TestAppSendSmsCodeRefetchesThePuzzleWhenTheChallengeIsVoided(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059554", "data": map[string]any{"captchaType": 2}},
		appPuzzleEnvelope(),
		map[string]any{"success": false, "code": "200059553", "data": map[string]any{"captchaType": 1}},
		appPuzzleEnvelope(), // 重新取图：挑战已作废，旧答案无效
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 6}},
	}}

	outcome, perr := AppSendSmsCode(
		context.Background(), upstream.NewUpstream(client), appProfile(),
		"19900000000", loginFixedClock())
	if perr != nil {
		t.Fatalf("挑战作废后重取图应成功: %v", perr)
	}
	if outcome.Kind != AppCodeSent || outcome.RemainGetTimes == nil || *outcome.RemainGetTimes != 6 {
		t.Fatalf("应发码成功且余量 6，实得 %+v", outcome)
	}

	verfy := 0
	for _, u := range client.urls() {
		if strings.HasSuffix(u, "/user/verfycode") {
			verfy++
		}
	}
	if verfy != 2 {
		t.Errorf("挑战作废后必须重新取图，实得取图 %d 次", verfy)
	}
}

// 自动过滑块也用尽 → **才**把 H5 地址交给用户（兜底，不是首选）。
func TestAppSendSmsCodeFallsBackToTheH5URLAfterTheAttemptLimit(t *testing.T) {
	// 每次都要滑块、每次都作废 → 到上限就停，别把配额与失败次数刷光。
	//
	// 请求序列（首次尝试**不取图** —— 它是"先空手试一次"）：
	//   发码 → [取图 → 发码] × (上限-1)
	script := []any{map[string]any{"code": "200059554", "data": map[string]any{"captchaType": 2}}}
	for i := 1; i < AppSmsMaxAttempts; i++ {
		script = append(script, appPuzzleEnvelope())
		script = append(script, map[string]any{"code": "200059553", "data": map[string]any{"captchaType": 1}})
	}
	expectedCalls := 1 + (AppSmsMaxAttempts-1)*2
	if len(script) != expectedCalls {
		t.Fatalf("脚本应有 %d 条，实得 %d", expectedCalls, len(script))
	}

	client := &loginHTTP{responses: script}
	outcome, perr := AppSendSmsCode(
		context.Background(), upstream.NewUpstream(client), appProfile(),
		"19900000000", loginFixedClock())
	if perr != nil {
		t.Fatalf("到上限应降级成 H5 兜底，而不是报错: %v", perr)
	}
	if outcome.Kind != AppCodeNeedCaptcha {
		t.Fatalf("到上限应降级成 H5 兜底，实得 %v", outcome.Kind)
	}
	if outcome.Attempts != AppSmsMaxAttempts {
		t.Errorf("不能无限试，实得 %d 次", outcome.Attempts)
	}
	if !strings.HasPrefix(outcome.URL, "https://yun.139.com/fe/captcha/?data=") {
		t.Errorf("兜底应给出 H5 地址，实得 %q", outcome.URL)
	}
	// 坐标要带出来：万一服务端要的答案口径变了，这是唯一线索
	if outcome.LastAnswer != "370" {
		t.Errorf("应带上最后发上去的坐标，实得 %q", outcome.LastAnswer)
	}
	if client.sent() != expectedCalls {
		t.Errorf("上限是硬约束，应发 %d 次，实得 %d", expectedCalls, client.sent())
	}
}

func TestAppSendSmsCodeReportsTheQuotaWhenAccepted(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 4}},
	}}

	outcome, perr := AppSendSmsCode(
		context.Background(), upstream.NewUpstream(client), appProfile(),
		"19900000000", loginFixedClock())
	if perr != nil {
		t.Fatalf("0000 应归为 Sent: %v", perr)
	}
	if outcome.RemainGetTimes == nil || *outcome.RemainGetTimes != 4 {
		t.Errorf("配额余量应是 4，实得 %v", outcome.RemainGetTimes)
	}
	if outcome.SolvedSlider {
		t.Error("一次就过，没走滑块")
	}
	if client.sent() != 1 {
		t.Errorf("没要滑块就不该多打取图接口，实得 %d 次", client.sent())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录
// ─────────────────────────────────────────────────────────────────────────────

func TestAppSmsLoginEncryptsTheBodyAndPersistsTheAppSession(t *testing.T) {
	crypto := upstream.NewAppLoginCryptoProduction()
	client := &loginHTTP{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": appEncryptedSession(t, crypto)},
	}}
	store := &loginStore{}

	outcome, perr := AppSmsLogin(
		context.Background(), store, upstream.NewUpstream(client),
		appLoginContext("119301"), crypto, loginFixedClock(), &loginRng{next: 7})
	if perr != nil {
		t.Fatalf("App 登录应成功: %v", perr)
	}

	// ① 发出去的体是**密文**：手机号与验证码都不该以明文出现
	sent := string(client.requests[0].Body)
	if strings.Contains(sent, "19900000000") {
		t.Error("请求体里不能有手机号明文")
	}
	if strings.Contains(sent, "119301") {
		t.Error("请求体里不能有验证码明文")
	}
	// 但能解回原样（证明发的是本端自己的密文，不是别的什么）
	plain, perr := crypto.DecryptBody(sent)
	if perr != nil {
		t.Fatalf("解不回自己发出去的体: %v", perr)
	}
	if !strings.Contains(plain, "119301") || !strings.Contains(plain, "19900000000") {
		t.Errorf("解回的明文应含手机号与验证码: %q", plain)
	}

	// ② 解出的会话被如实带回
	obj, _ := outcome.UserInfo.(map[string]any)
	if obj["userid"] != "199000000000" {
		t.Errorf("userInfo 应带回会话载荷，实得 %v", obj["userid"])
	}

	// ③ 落库：platform 必须是 `mobile` 而不是 PC 的 `pc`
	if len(store.accounts) != 1 {
		t.Fatalf("应写一条账号，实得 %d", len(store.accounts))
	}
	if store.accounts[0].Platform == nil || *store.accounts[0].Platform != "mobile" {
		t.Errorf("App 登录的 Authorization 前缀是 mobile，实得 %v", store.accounts[0].Platform)
	}
	if store.accounts[0].PhoneMasked == nil || *store.accounts[0].PhoneMasked != "19****00" {
		t.Errorf("掩码口径应与 core 一致，实得 %v", store.accounts[0].PhoneMasked)
	}

	if len(store.states) != 1 {
		t.Fatalf("应写一条登录态，实得 %d", len(store.states))
	}
	if store.states[0].UserDomainID == nil ||
		string(*store.states[0].UserDomainID) != "1990000000000000001" {
		t.Errorf("App 的 userDomainId 要能落进 user_domain_id，实得 %v", store.states[0].UserDomainID)
	}
	if store.states[0].JWTToken == nil ||
		string(*store.states[0].JWTToken) != "SYNTH001|1|RCS|1893456000000|AAAA" {
		t.Errorf("凭据应落库，实得 %v", store.states[0].JWTToken)
	}
	if store.states[0].DeviceID == nil ||
		string(*store.states[0].DeviceID) != "AAA00000000000000000000000000000" {
		t.Errorf("设备指纹应落库，实得 %v", store.states[0].DeviceID)
	}
}

// `9441` 是**用户**的问题（验证码不对）→ BadRequest，可重试、可提示。
func TestAppSmsLoginReportsWrongCodeAsBadRequest(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "9441", "message": "验证码不正确"},
	}}

	_, perr := AppSmsLogin(
		context.Background(), &loginStore{}, upstream.NewUpstream(client),
		appLoginContext("000000"), upstream.NewAppLoginCryptoProduction(),
		loginFixedClock(), &loginRng{next: 1})
	if perr == nil {
		t.Fatal("验证码不对必须报错")
	}
	if perr.Kind() != core.KindBadRequest {
		t.Errorf("9441 是用户的问题 → BadRequest，实得 %v (%v)", perr.Kind(), perr)
	}
	if !strings.Contains(perr.Error(), "9441") {
		t.Errorf("要带上上游的码: %v", perr)
	}
}

// `9103` 是**我们**的加密参数不对 → Internal。
//
// 归到 BadRequest 会让前端把它渲染成"请检查你输入的参数"，
// 而用户根本无从改起 —— 那正是这条断言要挡的。
func TestAppSmsLoginReportsUndecryptableAsInternal(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "9103", "message": "输入参数不合法"},
	}}

	_, perr := AppSmsLogin(
		context.Background(), &loginStore{}, upstream.NewUpstream(client),
		appLoginContext("119301"), upstream.NewAppLoginCryptoProduction(),
		loginFixedClock(), &loginRng{next: 1})
	if perr == nil {
		t.Fatal("9103 必须报错")
	}
	if perr.Kind() != core.KindInternal {
		t.Errorf("9103 是我们的问题 → Internal，实得 %v (%v)", perr.Kind(), perr)
	}
	if !strings.Contains(perr.Error(), "9103") {
		t.Errorf("要带上上游的码: %v", perr)
	}
}

func TestAppSmsLoginSendsTheFullAppHeaderSet(t *testing.T) {
	crypto := upstream.NewAppLoginCryptoProduction()
	client := &loginHTTP{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": appEncryptedSession(t, crypto)},
	}}

	if _, perr := AppSmsLogin(
		context.Background(), &loginStore{}, upstream.NewUpstream(client),
		appLoginContext("119301"), crypto, loginFixedClock(), &loginRng{next: 3}); perr != nil {
		t.Fatalf("登录应成功: %v", perr)
	}

	request := client.requests[0]
	if request.URL != "https://user-njs.yun.139.com/user/thirdlogin" {
		t.Errorf("应打 thirdlogin，实得 %q", request.URL)
	}
	// 内容类型只出现一次（`WithAppHeaders` 会先删掉 `json_body` 塞的那个）
	contentTypes := 0
	for _, h := range request.Headers {
		if strings.EqualFold(h[0], "content-type") {
			contentTypes++
			if h[1] != "application/json;charset=UTF-8" {
				t.Errorf("内容类型应是 json;charset=UTF-8，实得 %q", h[1])
			}
		}
	}
	if contentTypes != 1 {
		t.Errorf("不能出现两个 Content-Type，实得 %d 个", contentTypes)
	}
	// 关键的 App 端头都在
	for _, name := range []string{
		"service_identifier_key", "x-DeviceInfo", "x-yun-client-info",
		"x-UserAgent", "x-DeviceName",
	} {
		if _, has := headerOf(request, name); !has {
			t.Errorf("缺头 %s", name)
		}
	}
	// 首次登录没有 userDomainId，因此不该带 x-yun-uni
	if _, has := headerOf(request, "x-yun-uni"); has {
		t.Error("首次登录不该带 x-yun-uni")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 用户域回填
// ─────────────────────────────────────────────────────────────────────────────

// 回填这条线**为什么存在**：`thirdlogin` 之后 `userDomainId` 就入库了，
// 第二次起的 App 请求要带着它（`x-yun-uni`）。没接上之前，已登录的 App
// 请求会静默缺这个头。
func TestAKnownAccountGetsItsDomainIDBack(t *testing.T) {
	store := &loginStore{}
	id := core.NewAccountID("19****00")
	domain := core.UserDomainID("1990000000000000001")
	store.states = append(store.states, core.AccountState{AccountID: id, UserDomainID: &domain})
	store.owners = append(store.owners, func() *core.UserID {
		owner := core.NewUserID("u-login")
		return &owner
	}())

	filled, perr := ProfileWithStoredDomainID(
		context.Background(), store, loginScope(), appProfile(), id)
	if perr != nil {
		t.Fatalf("回填应成功: %v", perr)
	}
	// 档案里有值还不算，要真的落到发出去的那个头上
	found := false
	for _, h := range filled.Headers(UserTimestamp(loginFixedClock())) {
		if h[0] == "x-yun-uni" && h[1] == "1990000000000000001" {
			found = true
		}
	}
	if !found {
		t.Error("回填的值必须进请求头，而不是只存进档案")
	}
}

// 三种"没有"一律原样返回档案：库里查无此账号、那一列是空串、账号属于别人。
//
// 编一个值会让上游把这次请求定位到**别人**的用户域，比缺这个头更糟。
func TestNothingIsInventedWhenTheDomainIDIsAbsent(t *testing.T) {
	store := &loginStore{}
	id := core.NewAccountID("19****00")

	// ① 首次登录：库里还没有这个账号
	bare, perr := ProfileWithStoredDomainID(
		context.Background(), store, loginScope(), appProfile(), id)
	if perr != nil {
		t.Fatalf("查无此账号不该报错: %v", perr)
	}
	if bare.UserDomainID() != nil {
		t.Error("查无此账号时不要带 x-yun-uni")
	}

	// ② 有账号，但那一列是空串（上游没给 / 旧数据）
	empty := core.UserDomainID("")
	store.states = append(store.states, core.AccountState{AccountID: id, UserDomainID: &empty})
	store.owners = append(store.owners, nil)
	got, perr := ProfileWithStoredDomainID(
		context.Background(), store, loginScope(), appProfile(), id)
	if perr != nil {
		t.Fatalf("空串不该报错: %v", perr)
	}
	if got.UserDomainID() != nil {
		t.Error("空串不等于有值")
	}

	// ③ 同一个账号的数据属于别人 —— 按隔离契约读不到，也就不能带出去
	other := core.NewUserID("u-other")
	alien, perr := ProfileWithStoredDomainID(
		context.Background(), store, core.ScopeUser(&other), appProfile(), id)
	if perr != nil {
		t.Fatalf("别人的账号不该报错: %v", perr)
	}
	if alien.UserDomainID() != nil {
		t.Error("别人的账号不能回填进来")
	}
}

// 回填**不得**改调用方手上那份档案。
//
// Go 的 [upstream.AppProfile.WithUserDomainID] 改的是 receiver（指针语义），
// 而 Rust 那版是值语义 + 返回新值。照抄过去会让"查一下用户域"这种只读动作
// 带上副作用：调用方那份档案被改了，而它自己并不知道（§6.1 #73）。
func TestProfileWithStoredDomainIDDoesNotMutateTheCallerProfile(t *testing.T) {
	store := &loginStore{}
	id := core.NewAccountID("19****00")
	domain := core.UserDomainID("1990000000000000001")
	store.states = append(store.states, core.AccountState{AccountID: id, UserDomainID: &domain})
	store.owners = append(store.owners, nil)

	original := appProfile()
	if _, perr := ProfileWithStoredDomainID(
		context.Background(), store, core.ScopeLocal(), original, id); perr != nil {
		t.Fatalf("回填应成功: %v", perr)
	}
	if original.UserDomainID() != nil {
		t.Error("回填不得改调用方手上那份档案 —— 只读动作不该有副作用")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 时间戳与到期
// ─────────────────────────────────────────────────────────────────────────────

// 时间戳是微秒这件事已由 `signin_test.go` 的
// `TestUserTimestampIsMicrosecondsNotMilliseconds` 钉住 —— 两个消费者共用
// **同一个** [UserTimestamp]，所以这里**不**再写一条同名的断言：
// 两条会各自演化，而真正的风险（某处顺手改成毫秒）在两条都在时也照样漏。

func TestAppSessionExpiryIsAnExplicitDate(t *testing.T) {
	now := loginNow()
	if got := AppSessionExpiry(now); !got.After(now) {
		t.Errorf("到期时刻必须在现在之后，实得 %v", got)
	}
}

// AppCodeKind 的 String 要覆盖全部变体：
// 日志里出现空串等于"这个结果没有名字"，事后无从查起。
func TestAppCodeKindStringsAreAllNamed(t *testing.T) {
	for _, kind := range []AppCodeKind{AppCodeSent, AppCodeNeedCaptcha} {
		if kind.String() == "" {
			t.Errorf("%d 没有名字", kind)
		}
	}
}
