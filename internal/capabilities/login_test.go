package capabilities

// 本文件的用例按主题分成四组，对应 Rust 侧 login/tests/ 下的 qr / sms / session：
//
//	扫码状态机（推进、终态、TTL、码的映射）
//	短信链路（滑块降级、配额、登录）
//	登录结果 → auth（逐字对齐客户端那行 btoa）
//	登录结果 → 领域对象（账号 / 登录态）
//
// helper 集中在文件开头。

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// seqRandom 是确定性随机源：每次取字节都从当前值开始递增。
//
// 用它而不是真随机，是因为 `sID` / `random` 都要进请求体 ——
// 只有字节确定，请求体才能被逐字段断言。
type seqRandom struct{ b byte }

func (r *seqRandom) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.b
		r.b++
	}
}

// scriptedClient 是"按脚本逐次回放"的假客户端，并把收到的请求留下来供断言。
//
// ⚠ `Send` 在脚本用尽时**返回错误而不是返回空信封**：
// "多打了一次上游"和"上游回了空"是两件不同的事，混在一起会放过前者。
type scriptedClient struct {
	responses []string
	requests  []core.HTTPRequest
}

func (c *scriptedClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	if len(c.responses) == 0 {
		return core.HTTPResponse{}, core.NewInternal("脚本已用完（多打了一次上游）")
	}
	body := c.responses[0]
	c.responses = c.responses[1:]
	return core.NewResponse(200, []byte(body)), nil
}

func (c *scriptedClient) sent() int { return len(c.requests) }

// bodyOf 把第 i 个请求的体解成通用 map。
func (c *scriptedClient) bodyOf(t *testing.T, i int) map[string]any {
	t.Helper()
	if i >= len(c.requests) {
		t.Fatalf("只发了 %d 个请求，取不到第 %d 个", len(c.requests), i)
	}
	obj, ok := decodeObject(string(c.requests[i].Body))
	if !ok {
		t.Fatalf("请求体不是 JSON 对象: %s", c.requests[i].Body)
	}
	return obj
}

func (c *scriptedClient) header(t *testing.T, i int, name string) string {
	t.Helper()
	for _, h := range c.requests[i].Headers {
		if strings.EqualFold(h[0], name) {
			return h[1]
		}
	}
	return ""
}

// decodeObject 把一段 JSON 解成对象。
//
// 走 [core.ParseJSON] 而不是另引一份 JSON 实现：本项目的 JSON 出入口只有一处，
// 测试里也一样（"19 位标识不丢精度"那条保证在断言请求体时同样要成立）。
func decodeObject(text string) (map[string]any, bool) {
	value, ok := core.ParseJSON(text)
	if !ok {
		return nil, false
	}
	obj, ok := value.(map[string]any)
	return obj, ok
}

// mustJSON 把值序列化成 JSON 文本。
func mustJSON(value any) string {
	raw, err := core.MarshalJSON(value)
	if err != nil {
		panic(err)
	}
	return string(raw)
}

// quotedJSON 先序列化成 JSON 文本，再把这一段文本**当成字符串**序列化一次。
//
// 上游登录成功的 `data` 是**字符串**（真实链路里是 AES 密文，不是嵌套对象）——
// 夹具必须还原这个形状，否则"解密 `data`"那一步会被整个跳过，
// 而那正是本层要覆盖的一段。
func quotedJSON(value any) string { return mustJSON(mustJSON(value)) }

// plainDecryptor 是假解密器：测试里登录成功的 `data` 直接就是明文 JSON。
//
// 真实解密（AES-128-ECB + 客户端常量密钥）由 upstream 那层的黄金向量覆盖，
// 这里不重复测它 —— 本层要测的是"拿到 userInfo 之后怎么办"。
type plainDecryptor struct{}

func (plainDecryptor) DecryptUserInfo(ciphertext string) (any, *core.Error) {
	value, ok := core.ParseJSON(ciphertext)
	if !ok {
		return nil, core.NewParse("不是合法 JSON")
	}
	return value, nil
}

// recordingStore 只实现"登录落库"会用到的两个写入口，并记录调用顺序。
//
// # 为什么靠内嵌 [core.StateStore] 而不把 27 个方法都写一遍
//
// 全写一遍会把"这次改了什么"淹在 25 行 `panic("未使用")` 里。
// 内嵌接口之后，**没被重写的方法一旦被调用就会 panic** ——
// 这恰好是想要的效果：登录这条路径**不该**碰别的存储入口，碰了就该当场响，
// 而不是静默返回一个零值。
type recordingStore struct {
	core.StateStore
	order []string
}

func (s *recordingStore) UpsertAccount(_ context.Context, _ core.Scope, _ *core.Account) error {
	s.order = append(s.order, "account")
	return nil
}

func (s *recordingStore) UpsertAccountState(_ context.Context, _ core.Scope, _ *core.AccountState) error {
	s.order = append(s.order, "state")
	return nil
}

func now() time.Time { return time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC) }

// userInfo 是一份登录结果。
//
// ⚠ `authToken` 的第四段是 **epoch 毫秒**（1893456000000 = 2030-01-01），
// 不是随机串 —— 凭据到期判断全靠它，所以夹具里必须带一个真实形状的值。
func userInfo() map[string]any {
	return map[string]any{
		"account":   "19900000000",
		"authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
		"userId":    "1990000000000000001",
	}
}

func b64(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

// numText 把一个解码出来的数字取成文本。
//
// [core.ParseJSON] 用 `json.Number` 而不是 float64（19 位标识不能丢精度），
// 所以断言数字时比文本，别按 float64 比。
func numText(value any) string {
	switch v := value.(type) {
	case json.Number:
		return v.String()
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64)
	case nil:
		return ""
	}
	return fmt.Sprint(value)
}

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

func TestQrLoginWalksNotScannedScannedSuccess(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": false, "code": "200059541", "message": "未找到设备的二维码信息"}`,
		`{"success": false, "code": "200059548", "message": "二维码信息未授权"}`,
		`{"success": true, "code": "0000", "data": ` + quotedJSON(userInfo()) + `}`,
	}}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows,
		"62AA2BDA29F99744536E142A83CA5B03-ENDIN", &seqRandom{}, now())

	var kinds []QrProgressKind
	for qr.NextDelayMS(now()) != 0 {
		progress, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
		if err != nil {
			t.Fatalf("轮询失败: %v", err)
		}
		kinds = append(kinds, progress.Kind)
		if progress.IsTerminal() {
			break
		}
	}

	want := []QrProgressKind{QrNotScanned, QrScanned, QrLoggedIn}
	if len(kinds) != len(want) {
		t.Fatalf("推进序列长度不对: %v", kinds)
	}
	for i := range want {
		if kinds[i] != want[i] {
			t.Errorf("第 %d 次: 期望 %s 实得 %s", i+1, want[i], kinds[i])
		}
	}
	if client.sent() != 3 {
		t.Errorf("应恰好打三次上游，实得 %d", client.sent())
	}
}

// 客户端源码那句"登录成功后必须 clearInterval"：终态之后**不能再发请求**。
//
// 写成断言而不是注释，是因为"忘了停轮询"正是原实现专门标出来提醒自己的坑。
func TestQrLoginStopsPollingOnceTerminal(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": true, "code": "0000", "data": ` + quotedJSON(userInfo()) + `}`,
	}}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now())

	first, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
	if err != nil {
		t.Fatalf("轮询失败: %v", err)
	}
	if first.Kind != QrLoggedIn {
		t.Fatalf("应已登录，实得 %s", first.Kind)
	}
	if client.sent() != 1 {
		t.Fatalf("首次应只打一次，实得 %d", client.sent())
	}

	// 再问十次：结论不变，而且**一次请求都没发**
	for i := 0; i < 10; i++ {
		again, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
		if err != nil {
			t.Fatalf("第 %d 次重复轮询失败: %v", i, err)
		}
		if again.Kind != first.Kind {
			t.Errorf("第 %d 次重复轮询结论变了: %s", i, again.Kind)
		}
	}
	if client.sent() != 1 {
		t.Errorf("终态之后不得再发请求，实得 %d", client.sent())
	}
	if qr.NextDelayMS(now()) != 0 {
		t.Error("终态必须让调用方停下来")
	}
}

func TestQrLoginGivesUpAtItsOwnTTLWithoutAskingUpstream(t *testing.T) {
	client := &scriptedClient{}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now())

	later := now().Add(time.Duration(QRTTLSeconds) * time.Second)
	if !qr.IsExpired(later) {
		t.Error("过了 TTL 应判为过期")
	}
	if qr.NextDelayMS(later) != 0 {
		t.Error("过期后不应再要求轮询")
	}

	progress, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, later)
	if err != nil {
		t.Fatalf("轮询失败: %v", err)
	}
	if progress.Kind != QrFailed || progress.Reason.Kind != FailureTTLElapsed {
		t.Fatalf("应因超时失败，实得 %s / %s", progress.Kind, progress.Reason.Kind)
	}
	if progress.Reason.TTLSeconds != QRTTLSeconds {
		t.Errorf("失败原因应带回 TTL，实得 %d", progress.Reason.TTLSeconds)
	}
	if client.sent() != 0 {
		t.Errorf("超时就不该再打扰上游，实得 %d", client.sent())
	}
}

func TestQrLoginMapsTheDocumentedTerminalCodes(t *testing.T) {
	cases := []struct {
		code string
		want QrFailureKind
	}{
		{"200059542", FailureExpiredUpstream},
		{"200059544", FailureSessionMismatch},
	}
	for _, c := range cases {
		t.Run(c.code, func(t *testing.T) {
			client := &scriptedClient{responses: []string{
				`{"success": false, "code": "` + c.code + `"}`,
			}}
			up := upstream.NewUpstream(client)
			qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now())

			progress, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
			if err != nil {
				t.Fatalf("轮询失败: %v", err)
			}
			if progress.Kind != QrFailed || progress.Reason.Kind != c.want {
				t.Fatalf("期望失败 %s，实得 %s / %s", c.want, progress.Kind, progress.Reason.Kind)
			}
			if qr.NextDelayMS(now()) != 0 {
				t.Error("失败也是终态，必须让调用方停下来")
			}
		})
	}
}

// 限流**不是**终态：会话仍然有效，把它当终态会逼用户重开一张码，
// 而重开只会更深地撞进风控。
func TestQrLoginKeepsPollingThroughRateLimit(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": false, "code": "200059545", "message": "扫码登录过于频繁"}`,
		`{"success": false, "code": "200059541"}`,
	}}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now())

	first, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
	if err != nil {
		t.Fatalf("轮询失败: %v", err)
	}
	if first.Kind != QrFailed || first.Reason.Kind != FailureTooFrequent {
		t.Fatalf("应判为限流，实得 %s / %s", first.Kind, first.Reason.Kind)
	}
	if first.IsTerminal() {
		t.Error("限流不得是终态 —— 否则调用方会去换码，反而更深地撞风控")
	}
	if qr.NextDelayMS(now()) != QRPolllIntervalMS {
		t.Error("限流后应继续按原节奏轮询（总时长仍受 TTL 约束）")
	}

	// 而且它**不该被当成终态缓存**：下一次仍要真的发请求
	second, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
	if err != nil {
		t.Fatalf("第二次轮询失败: %v", err)
	}
	if second.Kind != QrNotScanned {
		t.Errorf("限流后应继续推进，实得 %s", second.Kind)
	}
	if client.sent() != 2 {
		t.Errorf("限流不得被缓存，应继续发请求，实得 %d", client.sent())
	}
}

func TestQrLoginKeepsUnknownCodesVerbatim(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": false, "code": 200999999, "message": "新码"}`,
	}}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now())

	progress, err := qr.Poll(context.Background(), up, "tid", plainDecryptor{}, now())
	if err != nil {
		t.Fatalf("轮询失败: %v", err)
	}
	if progress.Reason.Kind != FailureRejected {
		t.Fatalf("应原样带回，实得 %s", progress.Reason.Kind)
	}
	if progress.Reason.Code == nil || *progress.Reason.Code != 200999999 {
		t.Errorf("码应原样带回: %v", progress.Reason.Code)
	}
	if progress.Reason.Message != "新码" {
		t.Errorf("文案应原样带回: %q", progress.Reason.Message)
	}
}

func TestQrPollRequestCarriesSessionIDAsDycpwd(t *testing.T) {
	client := &scriptedClient{responses: []string{`{"code": "200059541"}`}}
	up := upstream.NewUpstream(client)
	qr := StartQrLogin(upstream.PlatformWindows, "dev-1", &seqRandom{}, now())

	if _, err := qr.Poll(context.Background(), up, "tid-9", plainDecryptor{}, now()); err != nil {
		t.Fatalf("轮询失败: %v", err)
	}

	if got := client.requests[0].URL; got != "https://user-njs.yun.139.com/user/thirdlogin" {
		t.Errorf("URL 不对: %s", got)
	}
	if got := client.header(t, 0, "x-yun-tid"); got != "tid-9" {
		t.Errorf("每个请求都要带新的 x-yun-tid: %q", got)
	}
	body := client.bodyOf(t, 0)
	// sID 与 dycpwd 是同一个值：弄错的话轮询会一直停在"未找到设备"
	if body["dycpwd"] != qr.SessionID() {
		t.Errorf("dycpwd 应为 sID: %v", body["dycpwd"])
	}
	if body["pintype"] != "21" {
		t.Errorf("扫码的 pintype 应为 21: %v", body["pintype"])
	}
	if body["clientType"] != "656" {
		t.Errorf("Windows 的 clientType 应为 656: %v", body["clientType"])
	}
}

func TestQrPayloadIsBuiltLocallyWithTheSessionID(t *testing.T) {
	qr := StartQrLogin(upstream.PlatformMacOS, "DEV-ID", &seqRandom{}, now())
	payload := qr.QrPayload()

	if !strings.HasPrefix(payload, "https://yun.139.com/w/#/qrcLogin?dID=DEV-ID&sID=") {
		t.Errorf("载荷前缀不对: %s", payload)
	}
	if !strings.Contains(payload, qr.SessionID()) {
		t.Errorf("载荷应含 sID: %s", payload)
	}
	if !strings.HasSuffix(payload, "cType=11&ver=4.2.1") {
		t.Errorf("macOS 的 ver 应是 4.2.1: %s", payload)
	}
	if len(qr.SessionID()) != 32 {
		t.Errorf("sID 应是 32 位十六进制: %q", qr.SessionID())
	}
	// 出码是**零网络请求**（客户端源码里就是字符串拼接），
	// 所以这份断言不需要任何假客户端 —— 它本身就证明了这一点。
}

func TestQrLoginWithTTLOverridesTheDeadline(t *testing.T) {
	qr := StartQrLogin(upstream.PlatformWindows, "dev", &seqRandom{}, now()).WithTTL(600)
	if qr.IsExpired(now().Add(2 * time.Minute)) {
		t.Error("覆盖成 600 秒后，2 分钟不该过期")
	}
	if !qr.IsExpired(now().Add(11 * time.Minute)) {
		t.Error("过了 600 秒应过期")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信
// ─────────────────────────────────────────────────────────────────────────────

func TestSmsLoginReportsSliderDowngradeAndTracksQuota(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": false, "code": "200059554", "data": {"captchaType": 2}}`,
		`{"success": false, "code": "200059553", "data": {"captchaType": 1}}`,
		`{"success": true, "code": "0000", "data": {"remainGetTimes": 8}}`,
	}}
	up := upstream.NewUpstream(client)
	sms := StartSmsLogin(upstream.PlatformWindows, "DEV-1", "19900000000", &seqRandom{})

	// ① 首次发码被风控要求滑块
	first, err := sms.SendCode(context.Background(), up, "tid")
	if err != nil {
		t.Fatalf("发码失败: %v", err)
	}
	if first.Kind != upstream.SmsNeedSlider || first.CaptchaType == nil || *first.CaptchaType != 2 {
		t.Fatalf("应要求滑块: %+v", first)
	}

	// ② 带滑块重试，但挑战已被判废 —— 调用方**必须重新取图**
	second, err := sms.SendCodeWithSlider(context.Background(), up, "tid", "461")
	if err != nil {
		t.Fatalf("发码失败: %v", err)
	}
	if second.Kind != upstream.SmsSliderExhausted {
		t.Fatalf("应报告挑战作废: %+v", second)
	}

	// ③ 换新挑战后成功，配额被记下来
	sms.Reroll(&seqRandom{9})
	third, err := sms.SendCodeWithSlider(context.Background(), up, "tid", "391")
	if err != nil {
		t.Fatalf("发码失败: %v", err)
	}
	if third.Kind != upstream.SmsSent {
		t.Fatalf("应已发出: %+v", third)
	}
	if sms.RemainGetTimes() == nil || *sms.RemainGetTimes() != 8 {
		t.Errorf("配额应被记下来: %v", sms.RemainGetTimes())
	}

	// 第三次请求体里必须同时有 24 位 random 与加密后的滑块答案
	body := client.bodyOf(t, 2)
	if got, _ := body["random"].(string); len(got) != 24 {
		t.Errorf("random 应是 24 位十六进制: %q", got)
	}
	if body["puzzleVerfycode"] != "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs=" {
		t.Errorf("滑块答案的密文与实测不符: %v", body["puzzleVerfycode"])
	}
}

func TestSmsLoginDecryptsUserInfoOnSuccess(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": true, "code": "0000", "data": ` + quotedJSON(userInfo()) + `}`,
	}}
	up := upstream.NewUpstream(client)
	sms := StartSmsLogin(upstream.PlatformWindows, "DEV-1", "19900000000", &seqRandom{})

	info, err := sms.Login(context.Background(), up, "tid", "119301", plainDecryptor{})
	if err != nil {
		t.Fatalf("登录失败: %v", err)
	}
	fields, ok := info.(map[string]any)
	if !ok || fields["account"] != "19900000000" {
		t.Fatalf("应解出 userInfo: %v", info)
	}

	body := client.bodyOf(t, 0)
	if body["pintype"] != "5" {
		t.Errorf("短信的 pintype 应为 5: %v", body["pintype"])
	}
	if body["dycpwd"] != "119301" {
		t.Errorf("短信方式下 dycpwd 是明文验证码: %v", body["dycpwd"])
	}
	extInfo, _ := body["extInfo"].(map[string]any)
	// ⚠ core.ParseJSON 为了"19 位标识不丢精度"用的是 `json.Number` 而不是 float64，
	// 所以数字**不能**按 float64 比 —— 比它的文本。
	if numText(extInfo["ifOpenAccount"]) != "1" {
		t.Errorf("短信登录的 ifOpenAccount 应为 1: %v", extInfo["ifOpenAccount"])
	}
}

func TestSmsLoginSurfacesBusinessFailureWithoutInventingAReason(t *testing.T) {
	client := &scriptedClient{responses: []string{
		`{"success": false, "code": "200059526", "message": "登录保护"}`,
	}}
	up := upstream.NewUpstream(client)
	sms := StartSmsLogin(upstream.PlatformWindows, "DEV-1", "19900000000", &seqRandom{})

	// 上游的码与文案**原样带回**：不臆造"验证码错误"这种我们并不知道的原因
	_, err := sms.Login(context.Background(), up, "tid", "000000", plainDecryptor{})
	if err == nil {
		t.Fatal("业务失败必须报错")
	}
	detail := err.Error()
	if !strings.Contains(detail, "200059526") || !strings.Contains(detail, "登录保护") {
		t.Errorf("应原样带回上游的码与文案: %s", detail)
	}
	if strings.Contains(detail, "验证码错误") {
		t.Errorf("不得臆造原因: %s", detail)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录结果 → auth
// ─────────────────────────────────────────────────────────────────────────────

func TestAuthIsRebuiltExactlyLikeTheClientDoes(t *testing.T) {
	// utils.js L230: "Basic " + btoa("pc:" + account + ":" + authToken)
	auth, err := AuthFromUserInfo(userInfo())
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	want := "Basic " + b64("pc:19900000000:SYNTH001|1|RCS|1893456000000|AAAA")
	if auth != want {
		t.Errorf("与客户端不一致:\n 期望 %s\n 实得 %s", want, auth)
	}

	// 而且它必须是既有解析器认得的形状
	parsed, ok := core.ParseAuth(auth)
	if !ok {
		t.Fatal("拼出来的 auth 应能被 core.ParseAuth 认得")
	}
	if parsed.Platform == nil || *parsed.Platform != "pc" {
		t.Errorf("平台段应是 pc: %v", parsed.Platform)
	}
	if parsed.Phone == nil || *parsed.Phone != "19900000000" {
		t.Errorf("手机号段不对: %v", parsed.Phone)
	}
	if got := parsed.ExpireMillis(); got != 1893456000000 {
		t.Errorf("到期毫秒不对: %d", got)
	}
}

func TestMissingFieldsAreReportedByName(t *testing.T) {
	// 上游改了字段名时，必须立刻说出缺哪个 ——
	// 而不是拼出一个空 auth 往下跑（那会带着一个没有凭据的"已登录"账号继续走）
	for _, missing := range []string{"account", "authToken"} {
		t.Run(missing, func(t *testing.T) {
			info := userInfo()
			info[missing] = ""
			_, err := AuthFromUserInfo(info)
			if err == nil {
				t.Fatalf("缺 %s 应报错", missing)
			}
			if got := err.Error(); !strings.Contains(got, missing) {
				t.Errorf("报错应点名字段 %s: %s", missing, got)
			}
		})
	}
}

// 只认字符串：上游把 account 改成数字时，必须立刻响，
// 而不是"顺手转成人能读的字符串"让它静默通过。
func TestNonStringFieldsAreRejectedNotCoerced(t *testing.T) {
	info := userInfo()
	info["account"] = float64(19900000000)
	if _, err := AuthFromUserInfo(info); err == nil {
		t.Error("数字类型的 account 应被拒，而不是被转成字符串")
	}
}

func TestAppPrefixProducesMobileNotPC(t *testing.T) {
	// App 端实测：Basic base64("mobile:<手机号>:<authToken>")
	auth, err := AuthFromUserInfoWithPrefix(userInfo(), "mobile")
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	want := "Basic " + b64("mobile:19900000000:SYNTH001|1|RCS|1893456000000|AAAA")
	if auth != want {
		t.Errorf("App 的前缀应是 mobile:\n 期望 %s\n 实得 %s", want, auth)
	}

	// 前缀进得了既有解析器，所以 App 的登录结果不需要第二套模型
	parsed, ok := core.ParseAuth(auth)
	if !ok {
		t.Fatal("App 的 auth 也应被 core.ParseAuth 认得")
	}
	if parsed.Platform == nil || *parsed.Platform != "mobile" {
		t.Errorf("平台段应是 mobile: %v", parsed.Platform)
	}

	// 反证：不能和 PC 的那个撞成一样
	pc, err := AuthFromUserInfo(userInfo())
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	if auth == pc {
		t.Error("两端的 auth 不得相同 —— 前缀搞混会让凭据对不上端")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录结果 → 领域对象
// ─────────────────────────────────────────────────────────────────────────────

func TestAppSessionPayloadIsUnderstoodWithoutFakingUserID(t *testing.T) {
	// App 的会话载荷用 `userDomainId`，PC 用 `userId` —— 是同一个东西。
	// 这里钉住"两种名字都认"，免得有人为了复用而在 App 侧伪造一个 `userId`。
	info := userInfo()
	info["userDomainId"] = info["userId"]
	delete(info, "userId")

	_, state, err := AccountAndStateFromLoginWithPrefix(
		core.ScopeLocal(), "DEV", info, "mobile", now())
	if err != nil {
		t.Fatalf("映射失败: %v", err)
	}
	if state.UserDomainID == nil || string(*state.UserDomainID) != "1990000000000000001" {
		t.Errorf("用户域标识应认 userDomainId: %v", state.UserDomainID)
	}
	if state.JWTToken == nil || string(*state.JWTToken) != "SYNTH001|1|RCS|1893456000000|AAAA" {
		t.Errorf("token 不对: %v", state.JWTToken)
	}
}

func TestLoginResultMapsToAnOwnedAccountAndState(t *testing.T) {
	owner := core.NewUserID("u1")
	account, state, err := AccountAndStateFromLogin(
		core.ScopeUser(&owner), "62AA2BDA29F99744536E142A83CA5B03-ENDIN", userInfo(), now())
	if err != nil {
		t.Fatalf("映射失败: %v", err)
	}

	if account.Owner == nil || *account.Owner != owner {
		t.Errorf("归属不对: %v", account.Owner)
	}
	if account.ID.String() != "19****00" {
		t.Errorf("账号 id 应是手机号掩码: %s", account.ID)
	}
	if account.PhoneMasked == nil || *account.PhoneMasked != "19****00" {
		t.Errorf("掩码不对: %v", account.PhoneMasked)
	}
	if account.Platform == nil || *account.Platform != "pc" {
		t.Errorf("平台不对: %v", account.Platform)
	}
	if account.AuthExpire == nil || *account.AuthExpire != 1893456000000 {
		t.Errorf("到期毫秒不对: %v", account.AuthExpire)
	}
	if !account.IsVisibleTo(owner) {
		t.Error("本人应看得见")
	}
	if account.IsVisibleTo(core.NewUserID("别人")) {
		t.Error("别人不应看得见")
	}

	if state.AccountID != account.ID {
		t.Errorf("登录态应挂在同一个账号上: %s vs %s", state.AccountID, account.ID)
	}
	if state.JWTToken == nil || string(*state.JWTToken) != "SYNTH001|1|RCS|1893456000000|AAAA" {
		t.Errorf("token 不对: %v", state.JWTToken)
	}
	if state.DeviceID == nil || string(*state.DeviceID) != "62AA2BDA29F99744536E142A83CA5B03-ENDIN" {
		t.Errorf("deviceId 不对: %v", state.DeviceID)
	}
	if state.UserDomainID == nil || string(*state.UserDomainID) != "1990000000000000001" {
		t.Errorf("用户域标识不对: %v", state.UserDomainID)
	}
	if state.UpdatedAt == nil || !state.UpdatedAt.Equal(now()) {
		t.Errorf("更新时间不对: %v", state.UpdatedAt)
	}
}

// 凭据**整串**落库：不是"拆开存、用的时候拼回去"。
//
// 合成的串不可检测（读侧优先采信整串，而整串路径没有前缀可校验），
// 所以这里断言的是"库里那一列 == 拼出来的那一整串"，而不是三段分别相等。
func TestPersistedAuthorizationIsTheWholeString(t *testing.T) {
	_, state, err := AccountAndStateFromLogin(core.ScopeLocal(), "DEV", userInfo(), now())
	if err != nil {
		t.Fatalf("映射失败: %v", err)
	}
	if state.Authorization == nil {
		t.Fatal("应落整串 auth")
	}
	want, err := AuthFromUserInfo(userInfo())
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	if string(*state.Authorization) != want {
		t.Errorf("库里应是拼出来的那一整串:\n 期望 %s\n 实得 %s", want, *state.Authorization)
	}
	// 完整手机号也落库：它是给人看的标识，不是拼凭据的原料
	if state.Phone == nil || string(*state.Phone) != "19900000000" {
		t.Errorf("完整手机号应落库: %v", state.Phone)
	}
}

// 两条路（旧数据导入 / 登录）必须得出同一个 AccountID，
// 否则同一个云盘账号会被当成两个。
func TestAccountIDDerivesFromTheSameMaskingPathAsLegacyImport(t *testing.T) {
	account, _, err := AccountAndStateFromLogin(
		core.ScopeLocal(), "dev",
		map[string]any{"account": "19900000000", "authToken": "T|1|R|1|A"},
		now())
	if err != nil {
		t.Fatalf("映射失败: %v", err)
	}
	want := core.NewAccountID(core.AccountLabel(nil, "19900000000"))
	if account.ID != want {
		t.Errorf("账号 id 应与导入路径一致: 期望 %s 实得 %s", want, account.ID)
	}
}

func TestPersistLoginWritesAccountBeforeState(t *testing.T) {
	store := &recordingStore{}
	owner := core.NewUserID("u1")
	id, err := PersistLogin(
		context.Background(), store, core.ScopeUser(&owner), "DEV", userInfo(), now())
	if err != nil {
		t.Fatalf("落库失败: %v", err)
	}
	if id.String() != "19****00" {
		t.Errorf("返回的账号 id 不对: %s", id)
	}
	// 顺序是**先账号、后登录态**：`account_state` 的写入会校验账号归属，
	// 账号不在就先写凭据会被正确地拒掉
	if len(store.order) != 2 || store.order[0] != "account" || store.order[1] != "state" {
		t.Errorf("写入顺序应为 account → state，实得 %v", store.order)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 枚举的漏态守卫
// ─────────────────────────────────────────────────────────────────────────────

// Go 的枚举是「int 的别名」，`switch` 漏一个分支不报错。
// 所以每个枚举都要有一条"取值全覆盖"的断言。
func TestEnumStringsCoverEveryCase(t *testing.T) {
	for k := QrNotScanned; k <= QrFailed; k++ {
		if k.String() == "" {
			t.Errorf("QrProgressKind(%d) 没有名字", int(k))
		}
	}
	for k := FailureTTLElapsed; k <= FailureRejected; k++ {
		if k.String() == "" {
			t.Errorf("QrFailureKind(%d) 没有名字", int(k))
		}
	}
	// 越界值必须给出空串（而不是撞进某个分支）
	if (QrProgressKind(99)).String() != "" || (QrFailureKind(99)).String() != "" {
		t.Error("越界枚举值应返回空串")
	}
}
