package app

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 本文件对应 Rust 侧 ../mcloud-rust/crates/mcloud-app/src/login.rs 的 `mod tests`。
//
// 测试用的号码一律是 **199 开头的合成号**（脱敏号段），不含任何真实标识。

// ─────────────────────────────────────────────────────────────────────────────
// 假上游
// ─────────────────────────────────────────────────────────────────────────────

// loginHTTP 逐次返回脚本响应，并记下发出去的请求与次数。
//
// 队列**空了就报错**而不是再回一个"看起来正常"的响应：那代表"这次不该打上游"，
// 让"超时后不得再打扰上游"这类断言真能变红。
type loginHTTP struct {
	responses []any
	requests  []core.HTTPRequest
}

func (c *loginHTTP) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	if len(c.responses) == 0 {
		return core.HTTPResponse{}, core.NewInternal("脚本已用完：这次不该再打上游")
	}
	body, err := core.MarshalJSON(c.responses[0])
	if err != nil {
		return core.HTTPResponse{}, err
	}
	c.responses = c.responses[1:]
	return core.NewResponse(200, body), nil
}

func (c *loginHTTP) sent() int { return len(c.requests) }

func (c *loginHTTP) urls() []string {
	out := make([]string, 0, len(c.requests))
	for _, r := range c.requests {
		out = append(out, r.URL)
	}
	return out
}

func (c *loginHTTP) bodyOf(t *testing.T, index int) map[string]any {
	t.Helper()
	if index >= len(c.requests) {
		t.Fatalf("只发出 %d 个请求，取不到第 %d 个", len(c.requests), index)
	}
	value, ok := core.ParseJSON(string(c.requests[index].Body))
	if !ok {
		t.Fatalf("第 %d 个请求的体不是合法 JSON: %q", index, string(c.requests[index].Body))
	}
	obj, isObj := value.(map[string]any)
	if !isObj {
		t.Fatalf("第 %d 个请求的体不是对象: %q", index, string(c.requests[index].Body))
	}
	return obj
}

// ─────────────────────────────────────────────────────────────────────────────
// 假存储 / 时钟 / 随机 / 等待
// ─────────────────────────────────────────────────────────────────────────────

// loginStore 是 [core.StateStore] 的**部分假实现**。
//
// 没重写的方法一被调到就会 panic —— 那正是要的：
// "本层碰了它不该碰的存储入口"立刻响，而不是静默返回零值（§6.1 #35）。
type loginStore struct {
	core.StateStore
	accounts []core.Account
	states   []core.AccountState
	// owners 记下每次写库时的归属。
	//
	// 它是为了让**隔离**这条契约在假实现里也成立：读的时候对不上归属就当没有，
	// 于是"别人的账号读不出来"这条断言在这个 stub 上也是真的 ——
	// 否则那条用例只是在验证"我写进去的东西能读出来"，什么也没守住。
	owners []*core.UserID
}

func (s *loginStore) UpsertAccount(_ context.Context, _ core.Scope, account *core.Account) error {
	s.accounts = append(s.accounts, *account)
	return nil
}

func (s *loginStore) UpsertAccountState(
	_ context.Context, scope core.Scope, state *core.AccountState,
) error {
	s.states = append(s.states, *state)
	s.owners = append(s.owners, scope.Owner())
	return nil
}

// GetAccountState 按"账号 id + 归属"读登录态，对不上归属就当没有。
func (s *loginStore) GetAccountState(
	_ context.Context, scope core.Scope, id core.AccountID,
) (*core.AccountState, error) {
	for i, state := range s.states {
		if state.AccountID != id {
			continue
		}
		owner := s.owners[i]
		if scope.IsLocal() || owner == nil {
			found := state
			return &found, nil
		}
		if scope.Owner() != nil && *owner == *scope.Owner() {
			found := state
			return &found, nil
		}
	}
	return nil, nil
}

// loginClock 是逐次读表都往前跳一小时的时钟：用来造"已经超时"，
// **不依赖真的等一小时**。
type loginClock struct {
	base  time.Time
	reads int
}

func (c *loginClock) Now() time.Time {
	c.reads++
	return c.base.Add(time.Duration(c.reads) * time.Hour)
}

// loginRng 是每次填充都递增的确定性随机源（对齐 Rust 侧 `SeqRandom`）。
type loginRng struct{ next byte }

func (r *loginRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

// recordingWaiter 记下"被要求等了多久"，一次都不真等。
//
// 它是 Go 侧**新长出来的**测试替身（方案 §6.1 #72）：Rust 靠 tokio 的
// `start_paused` 虚拟化定时器，Go 没有对应物，于是"退避了多久"必须靠注入才看得见。
type recordingWaiter struct{ durations []time.Duration }

func (w *recordingWaiter) Wait(_ context.Context, d time.Duration) *core.Error {
	w.durations = append(w.durations, d)
	return nil
}

// plainDecryptor 把 `data` 当明文 JSON 解（真实现是 AES-128-ECB）。
//
// 判据不是"能解"，而是"调用方**只**依赖 [upstream.LoginPayloadDecryptor] 这个
// 接口" —— 上游哪天换了加密，这里换成另一把钥匙即可，驱动层一行不用改。
type plainDecryptor struct{}

func (plainDecryptor) DecryptUserInfo(ciphertext string) (any, *core.Error) {
	value, ok := core.ParseJSON(ciphertext)
	if !ok {
		return nil, core.NewParse("userInfo 不是合法 JSON")
	}
	return value, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

func loginNow() time.Time { return time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC) }

func loginFixedClock() core.Clock { return core.NewFixedClock(loginNow()) }

func loginScope() core.Scope {
	user := core.NewUserID("u-login")
	return core.ScopeUser(&user)
}

func loginUserInfo() map[string]any {
	return map[string]any{
		"account":   "19900000000",
		"authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
		"userId":    "1990000000000000001",
	}
}

// loginSuccessEnvelope 是登录成功的响应信封。
//
// ⚠⚠ `data` 必须是**字符串**（序列化两次）—— 真实链路里它是 AES 密文，
// 本来就是字符串。写成嵌套对象会让解密器那一步被**整个跳过**
// （它收不到 string，判定降级成别的分支），而测试照样是绿的（§6.1 #36）。
func loginSuccessEnvelope() map[string]any {
	inner, err := core.MarshalJSON(loginUserInfo())
	if err != nil {
		panic(err)
	}
	return map[string]any{"success": true, "code": "0000", "data": string(inner)}
}

// loginPuzzleEnvelope 是取滑块图的响应（`puzzleLeft` 解出来是 370）。
func loginPuzzleEnvelope() map[string]any {
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

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

func TestQrLoginWalksTheLoopAndPersistsOnSuccess(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059541"},
		map[string]any{"success": false, "code": "200059548"},
		loginSuccessEnvelope(),
	}}
	store := &loginStore{}
	observer := &recordingObserver{}

	outcome, perr := QRLogin(
		context.Background(), store, upstream.NewUpstream(client),
		NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1"),
		plainDecryptor{}, loginFixedClock(), &loginRng{}, observer, NoWait{})
	if perr != nil {
		t.Fatalf("扫码登录应成功，实得错误: %v", perr)
	}

	if outcome.AccountID.String() != "19****00" {
		t.Errorf("账号 id 应是掩码，实得 %q", outcome.AccountID.String())
	}
	if outcome.Summary() != "19****00" {
		t.Errorf("摘要应是掩码，实得 %q", outcome.Summary())
	}
	if strings.Contains(outcome.Summary(), "19900000000") {
		t.Errorf("摘要不得带出完整手机号: %q", outcome.Summary())
	}

	// 二维码只给了一次，且带着会话 id。
	if len(observer.qr) != 1 {
		t.Fatalf("二维码只应给一次，实得 %d 次", len(observer.qr))
	}
	if !strings.Contains(observer.qr[0], "sID=") {
		t.Errorf("二维码载荷应带 sID: %q", observer.qr[0])
	}

	// 进展按实测的顺序：未扫 → 已扫 → 成功
	if len(observer.progress) != 3 {
		t.Fatalf("应有 3 次进展，实得 %d", len(observer.progress))
	}
	if observer.progress[0].Kind != capabilities.QrNotScanned {
		t.Errorf("第一次应是未扫码，实得 %v", observer.progress[0].Kind)
	}
	if observer.progress[1].Kind != capabilities.QrScanned {
		t.Errorf("第二次应是已扫码，实得 %v", observer.progress[1].Kind)
	}
	if observer.progress[2].Kind != capabilities.QrLoggedIn {
		t.Errorf("第三次应是成功，实得 %v", observer.progress[2].Kind)
	}

	// 三次请求，一条不多
	if client.sent() != 3 {
		t.Errorf("应发 3 次请求，实得 %d", client.sent())
	}

	// 落库：账号归属于登录者，登录态带上凭据
	if len(store.accounts) != 1 || len(store.states) != 1 {
		t.Fatalf("账号与登录态各应写一条，实得 %d / %d", len(store.accounts), len(store.states))
	}
	if store.accounts[0].Owner == nil || *store.accounts[0].Owner != core.NewUserID("u-login") {
		t.Errorf("账号应归属于登录者，实得 %v", store.accounts[0].Owner)
	}
	if store.states[0].JWTToken == nil {
		t.Error("登录态应带上凭据")
	}
	if store.states[0].DeviceID == nil || string(*store.states[0].DeviceID) != "DEV-1" {
		t.Errorf("登录态应记下设备指纹 DEV-1，实得 %v", store.states[0].DeviceID)
	}
}

// 限频（`200059545`）**不是**终态：退避后继续问。
//
// 这条是实测换来的：连着跑几次扫码登录会被限频，而当时的实现把它当终态 → 直接失败。
// 用户看到"登录失败"就会重开一张码，于是更深地撞进风控。
func TestQrLoginBacksOffOnRateLimitInsteadOfGivingUp(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059545", "message": "扫码登录过于频繁"},
		map[string]any{"success": false, "code": "200059545", "message": "扫码登录过于频繁"},
		loginSuccessEnvelope(),
	}}
	store := &loginStore{}
	observer := &recordingObserver{}
	waiter := &recordingWaiter{}

	outcome, perr := QRLogin(
		context.Background(), store, upstream.NewUpstream(client),
		NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1"),
		plainDecryptor{}, loginFixedClock(), &loginRng{}, observer, waiter)
	if perr != nil {
		t.Fatalf("限频之后必须继续尝试，而不是放弃，实得: %v", perr)
	}
	if outcome.AccountID.String() != "19****00" {
		t.Errorf("账号 id 应是掩码，实得 %q", outcome.AccountID.String())
	}
	if client.sent() != 3 {
		t.Errorf("限频两次之后应继续问到成功，实得 %d 次", client.sent())
	}

	// 退避是真退避：两次限频各等 30 秒，且**没有**被当成"下一次轮询间隔"
	if len(waiter.durations) != 2 {
		t.Fatalf("应退避 2 次，实得 %d 次", len(waiter.durations))
	}
	for i, d := range waiter.durations {
		if d != TooFrequentBackoff {
			t.Errorf("第 %d 次退避应是 %v，实得 %v", i+1, TooFrequentBackoff, d)
		}
	}

	// 观察者要能看到限频事实，前端才能提示"等一会儿"
	sawLimit := false
	for _, p := range observer.progress {
		if p.Kind == capabilities.QrFailed && p.Reason.Kind == capabilities.FailureTooFrequent {
			sawLimit = true
		}
	}
	if !sawLimit {
		t.Error("观察者应看到限频事实")
	}
}

func TestQrLoginStopsWhenTheCodeExpires(t *testing.T) {
	// 空脚本：一旦真发请求就会报"脚本已用完"
	client := &loginHTTP{responses: nil}

	_, perr := QRLogin(
		context.Background(), &loginStore{}, upstream.NewUpstream(client),
		NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1"),
		plainDecryptor{}, &loginClock{base: loginNow()}, &loginRng{}, SilentObserver{}, NoWait{})
	if perr == nil {
		t.Fatal("超时必须结束循环")
	}
	if perr.Kind() != core.KindTimeout {
		t.Errorf("应是 Timeout，实得 %v (%v)", perr.Kind(), perr)
	}
	if client.sent() != 0 {
		t.Errorf("超时不该再打扰上游，实得 %d 次", client.sent())
	}
}

func TestQrLoginReportsTerminalFailureInsteadOfSpinning(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059542"},
		// 若循环没停，就会用到这里
		map[string]any{"success": false, "code": "200059541"},
	}}

	_, perr := QRLogin(
		context.Background(), &loginStore{}, upstream.NewUpstream(client),
		NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1"),
		plainDecryptor{}, loginFixedClock(), &loginRng{}, SilentObserver{}, NoWait{})
	if perr == nil {
		t.Fatal("码失效必须结束")
	}
	if !strings.Contains(perr.Error(), "expired_upstream") {
		t.Errorf("失败原因要能看出是上游判失效（便于事后归因）: %v", perr)
	}
	if client.sent() != 1 {
		t.Errorf("终态之后不得继续轮询，实得 %d 次", client.sent())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信
// ─────────────────────────────────────────────────────────────────────────────

// 发码成功、不需要滑块：一次请求搞定。
func TestSmsSendCodeSendsDirectlyWhenNoSliderIsRequired(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 8}},
	}}

	sent, perr := SmsSendCode(
		context.Background(), upstream.NewUpstream(client),
		upstream.PlatformWindows, "DEV-1", "19900000000", &loginRng{})
	if perr != nil {
		t.Fatalf("发码应成功: %v", perr)
	}
	if sent.RemainGetTimes == nil || *sent.RemainGetTimes != 8 {
		t.Errorf("配额余量应是 8，实得 %v", sent.RemainGetTimes)
	}
	if sent.SolvedSlider {
		t.Error("首次发码不该走滑块")
	}
	if client.sent() != 1 {
		t.Fatalf("应发 1 次请求，实得 %d", client.sent())
	}

	body := client.bodyOf(t, 0)
	// 实测 `mode`/`reqType` 是固定值，`random` 是 24 位十六进制（不是 UUID）
	//
	// ⚠ 取数用 [loginInt] 而不是比 `float64`：core.ParseJSON 走 `UseNumber`
	// （19 位的 `userDomainId` 解成 float64 会静默丢精度），
	// 于是这些值是 `json.Number`，直接比数字会**恒假** —— 而恒假的断言不会报错。
	if loginInt(t, body["mode"]) != 0 {
		t.Errorf("mode 应是 0，实得 %v", body["mode"])
	}
	if loginInt(t, body["reqType"]) != 3 {
		t.Errorf("reqType 应是 3，实得 %v", body["reqType"])
	}
	text, _ := body["random"].(string)
	if len(text) != 24 {
		t.Errorf("random 应是 24 位十六进制，实得 %q", text)
	}
	if _, has := body["puzzleVerfycode"]; has {
		t.Error("首次不该带滑块答案")
	}
}

// 被要求滑块 → **自动解出答案**并重发，且发出去的答案必须能解回正确坐标。
func TestSmsSendCodeSolvesTheSliderAutomatically(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059554", "message": "请先获取滑块拼图验证码",
			"data": map[string]any{"captchaType": 2}},
		loginPuzzleEnvelope(),
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 7}},
	}}

	sent, perr := SmsSendCode(
		context.Background(), upstream.NewUpstream(client),
		upstream.PlatformWindows, "DEV-1", "19900000000", &loginRng{})
	if perr != nil {
		t.Fatalf("走一次滑块后应发码成功: %v", perr)
	}
	if sent.RemainGetTimes == nil || *sent.RemainGetTimes != 7 {
		t.Errorf("配额余量应是 7，实得 %v", sent.RemainGetTimes)
	}
	if !sent.SolvedSlider {
		t.Error("应当走了一次滑块")
	}
	if client.sent() != 3 {
		t.Fatalf("应是 发码 → 取图 → 带答案重发，实得 %d 次", client.sent())
	}

	urls := client.urls()
	if !strings.HasSuffix(urls[0], "/user/sms/getSmsCode") {
		t.Errorf("第一次应是发码，实得 %q", urls[0])
	}
	if !strings.HasSuffix(urls[1], "/user/verfycode") {
		t.Errorf("第二次应是取图（verfycode 没有 i），实得 %q", urls[1])
	}
	if !strings.HasSuffix(urls[2], "/user/sms/getSmsCode") {
		t.Errorf("第三次应是带答案重发，实得 %q", urls[2])
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
}

// `200059553`（挑战作废）→ **必须重新取图**，不能复用旧答案。
func TestSmsSendCodeRefetchesThePuzzleWhenTheChallengeIsVoided(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{"success": false, "code": "200059554", "data": map[string]any{"captchaType": 2}},
		loginPuzzleEnvelope(),
		map[string]any{"success": false, "code": "200059553", "data": map[string]any{"captchaType": 1}},
		loginPuzzleEnvelope(), // 重新取图：挑战已作废，旧答案无效
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 6}},
	}}

	sent, perr := SmsSendCode(
		context.Background(), upstream.NewUpstream(client),
		upstream.PlatformWindows, "DEV-1", "19900000000", &loginRng{})
	if perr != nil {
		t.Fatalf("挑战作废后重取图应成功: %v", perr)
	}
	if sent.RemainGetTimes == nil || *sent.RemainGetTimes != 6 {
		t.Errorf("配额余量应是 6，实得 %v", sent.RemainGetTimes)
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

func TestSmsSendCodeGivesUpAfterTheAttemptLimit(t *testing.T) {
	// 每次都要滑块、每次都作废 → 到上限就停，别把配额刷光
	client := &loginHTTP{responses: []any{
		map[string]any{"code": "200059554", "data": map[string]any{"captchaType": 2}},
		loginPuzzleEnvelope(),
		map[string]any{"code": "200059553"},
		loginPuzzleEnvelope(),
		map[string]any{"code": "200059553"},
	}}

	_, perr := SmsSendCode(
		context.Background(), upstream.NewUpstream(client),
		upstream.PlatformWindows, "DEV-1", "19900000000", &loginRng{})
	if perr == nil {
		t.Fatal("到上限必须放弃")
	}
	if perr.Kind() != core.KindRateLimited {
		t.Errorf("应是 RateLimited，实得 %v (%v)", perr.Kind(), perr)
	}
	// 要给出替代路径：用户此刻最需要的是"那我还能怎么登"
	if !strings.Contains(perr.Error(), "扫码登录") {
		t.Errorf("要给出替代路径: %v", perr)
	}
	// 失败信息里要能看到"我们最后发了什么"，否则无法判断是坐标算错了还是风控
	if !strings.Contains(perr.Error(), "370") {
		t.Errorf("要带上最后发上去的坐标: %v", perr)
	}
}

// 上游业务失败要**原样带回它的码与文案**：不臆造"手机号错误"这类我们并不知道
// 的结论。
//
// `5010047` 是 2026-09-18 用非法号码真实调用发码接口观察到的
// （见 docs/upstream/logout-sms-login.md §4）。
func TestSmsSendCodeSurfacesUpstreamFailureVerbatim(t *testing.T) {
	client := &loginHTTP{responses: []any{
		map[string]any{
			"success": false,
			"code":    "5010047",
			"message": "国家码和手机号不匹配或手机号格式错误",
		},
	}}

	_, perr := SmsSendCode(
		context.Background(), upstream.NewUpstream(client),
		upstream.PlatformWindows, "DEV-1", "0000", &loginRng{})
	if perr == nil {
		t.Fatal("上游失败必须带回来")
	}
	detail := perr.Error()
	if !strings.Contains(detail, "5010047") {
		t.Errorf("要原样带回上游的码: %v", detail)
	}
	if !strings.Contains(detail, "国家码和手机号不匹配") {
		t.Errorf("要原样带回上游的文案: %v", detail)
	}
}

func TestSmsLoginDecryptsAndPersists(t *testing.T) {
	client := &loginHTTP{responses: []any{loginSuccessEnvelope()}}
	store := &loginStore{}

	outcome, perr := SmsLogin(
		context.Background(), store, upstream.NewUpstream(client),
		NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1"),
		SmsCredential{Phone: "19900000000", Code: "119301"},
		plainDecryptor{}, loginFixedClock(), &loginRng{})
	if perr != nil {
		t.Fatalf("短信登录应成功: %v", perr)
	}
	if outcome.AccountID.String() != "19****00" {
		t.Errorf("账号 id 应是掩码，实得 %q", outcome.AccountID.String())
	}
	if len(store.states) != 1 || store.states[0].JWTToken == nil {
		t.Fatal("登录态应带上凭据")
	}

	body := client.bodyOf(t, 0)
	// 短信登录与扫码共用 /user/thirdlogin，靠 pintype 区分
	if body["pintype"] != "5" {
		t.Errorf("pintype 应是 5，实得 %v", body["pintype"])
	}
	if body["dycpwd"] != "119301" {
		t.Errorf("短信方式下 dycpwd 是明文验证码，实得 %v", body["dycpwd"])
	}
	if body["msisdn"] != "19900000000" {
		t.Errorf("msisdn 应是手机号，实得 %v", body["msisdn"])
	}
	ext, _ := body["extInfo"].(map[string]any)
	if loginInt(t, ext["ifOpenAccount"]) != 1 {
		t.Errorf("extInfo.ifOpenAccount 应是 1，实得 %v", ext["ifOpenAccount"])
	}
}

// loginInt 从解析后的 JSON 里取整数。
//
// 存在的理由见上面那条注释：`UseNumber` 让数字是 `json.Number`，
// 断言写成形如 `body["mode"] != 0` 时**永远为真**（类型不同）——
// 一条恒真的断言比没有断言更危险，因为它是绿的。
func loginInt(t *testing.T, value any) int64 {
	t.Helper()
	switch n := value.(type) {
	case json.Number:
		parsed, err := n.Int64()
		if err != nil {
			t.Fatalf("%v 不是整数: %v", value, err)
		}
		return parsed
	case float64:
		return int64(n)
	case int64:
		return n
	case nil:
		t.Fatalf("字段不存在")
	}
	t.Fatalf("字段不是数字: %v", value)
	return 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Go 侧新增：等待、TTL 来源、摘要退路
// ─────────────────────────────────────────────────────────────────────────────

// 等待必须能被取消：一次扫码最长要等 120 秒，
// 用户 Ctrl-C 了却还在等是没法接受的（Rust 侧由 tokio 的取消保证）。
func TestRealWaiterStopsWaitingWhenTheContextIsCancelled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	perr := RealWaiter{}.Wait(ctx, 10*time.Second)
	if perr == nil {
		t.Fatal("ctx 已取消时必须立刻返回，而不是睡满")
	}
	if perr.Kind() != core.KindTimeout {
		t.Errorf("取消应归到 Timeout，实得 %v", perr.Kind())
	}
}

// TTL 的默认值必须来自能力层的常量，不能在本层再写一份 120。
//
// 判据：改 [capabilities.QRTTLSeconds] 时，这里的期望值**跟着变** ——
// 于是"两处各写一份、后来只改了一处"不会静默通过（同 §6.1 #67 那条教训）。
func TestLoginContextTTLComesFromTheRegistryNotASecondLiteral(t *testing.T) {
	if got := NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1").ttl; got != capabilities.QRTTL {
		t.Errorf("默认 TTL 应取自能力层常量 %v，实得 %v", capabilities.QRTTL, got)
	}
	if got := NewLoginContext(loginScope(), upstream.PlatformWindows, "DEV-1").WithTTL(600 * time.Second).ttl; got != 600*time.Second {
		t.Errorf("WithTTL 应能放宽，实得 %d", got)
	}
}

// 账号字段缺失时退回账号 id —— 那也是掩码，所以不会把完整手机号带出来。
func TestLoginSummaryFallsBackToTheMaskedAccountID(t *testing.T) {
	outcome := LoginOutcome{AccountID: core.NewAccountID("19****00")}
	if outcome.Summary() != "19****00" {
		t.Errorf("缺少 account 时应退回账号 id，实得 %q", outcome.Summary())
	}
}

func TestSessionExpiryIsAnExplicitDateNotInfinity(t *testing.T) {
	now := loginNow()
	expires := SessionExpiry(now, 30)
	if !expires.After(now) {
		t.Errorf("到期时刻必须在现在之后，实得 %v", expires)
	}
	if days := expires.Sub(now).Hours() / 24; days != 30 {
		t.Errorf("应是 30 天后，实得 %v 天", days)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 观察者替身
// ─────────────────────────────────────────────────────────────────────────────

type recordingObserver struct {
	qr       []string
	progress []capabilities.QrProgress
}

func (o *recordingObserver) OnQR(payload string) {
	o.qr = append(o.qr, payload)
}

func (o *recordingObserver) OnProgress(progress capabilities.QrProgress) {
	o.progress = append(o.progress, progress)
}
