package upstream

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// appSigninVectors 是 test/golden/app-signin-vectors.json 的形状。
//
// ⚠ 这份向量的 `$source` 是 **Reqable 抓包**（iOS App 签到页 H5）+ 账号 B 纯协议复现，
// 里面的签名是**实拍值**而不是算出来的 ⇒ 它是一次真正的跨端对拍：
// Go 侧算出来的 MD5 必须与抓包里那一次请求的 `x-signature` 逐字相同。
type appSigninVectors struct {
	Constants   map[string]any `json:"constants"`
	H5Signature []struct {
		Name      string `json:"name"`
		RequestID string `json:"requestId"`
		Timestamp string `json:"timestamp"`
		Nonce     string `json:"nonce"`
		SaltKey   uint8  `json:"saltKey"`
		Other     string `json:"other"`
		ExpectMD5 string `json:"expectMd5"`
	} `json:"h5Signature"`
	Envelopes []struct {
		Name        string `json:"name"`
		Raw         any    `json:"raw"`
		Code        int64  `json:"code"`
		ExpectToken string `json:"expectToken"`
	} `json:"envelopes"`
	InfoV3 struct {
		Name string `json:"name"`
		Raw  any    `json:"raw"`
	} `json:"infoV3"`
	StartSignIn struct {
		Name string `json:"name"`
		Raw  any    `json:"raw"`
	} `json:"startSignIn"`
	PrizeRecords struct {
		Name string `json:"name"`
		Raw  any    `json:"raw"`
	} `json:"prizeRecords"`
	ReceiveRevivalReward struct {
		Name string `json:"name"`
		Raw  any    `json:"raw"`
	} `json:"receiveRevivalReward"`
}

func loadAppSigninVectors(t *testing.T) appSigninVectors {
	t.Helper()
	raw, err := golden.Load("app-signin-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	var out appSigninVectors
	decoded, ok := core.ParseJSON(string(raw))
	if !ok {
		t.Fatal("黄金向量不是合法 JSON")
	}
	// 走一遍 marshal → parse 拿到统一的 `json.Number` 语义（见 §6.1 #34）
	buf, err := core.MarshalJSON(decoded)
	if err != nil {
		t.Fatalf("重新序列化失败: %v", err)
	}
	if err := json.Unmarshal(buf, &out); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	return out
}

// vecText 把向量里的一个标量取成文本（数字与字符串都能取）。
func vecText(value any) string {
	switch v := value.(type) {
	case json.Number:
		return v.String()
	case string:
		return v
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64)
	}
	return fmt.Sprint(value)
}

// ─────────────────────────────────────────────────────────────────────────────
// 常量：由抓包钉住，改错一个不会报错
// ─────────────────────────────────────────────────────────────────────────────

func TestSigninConstantsMatchTheCapturedVector(t *testing.T) {
	v := loadAppSigninVectors(t)
	cases := []struct {
		key string
		got string
	}{
		{"MARKET_NAME", MarketName},
		{"SOURCE_ID", SourceID},
		{"TO_SOURCE_ID", ToSourceID},
		{"H5_APP_ID", H5AppID},
		{"H5_APP_VERSION", H5AppVersion},
		{"TASK_CLIENT_VERSION", TaskClientVersion},
		{"H5_SIGN_SALT_KEY", strconv.Itoa(int(H5SignSaltKey))},
	}
	for _, c := range cases {
		if want := vecText(v.Constants[c.key]); want != c.got {
			t.Errorf("%s: 向量 %s，代码 %s", c.key, want, c.got)
		}
	}

	// 盐是 **PC 端签名那个盐**（`sekaMdYYLIZfbCfm`），不是这一族自造的
	if want := vecText(v.Constants["H5_SIGN_SALT"]); want != core.SignSalt(H5SignSaltKey) {
		t.Errorf("H5 签名盐应与 core 的盐表一致: 向量 %s，core %s", want, core.SignSalt(H5SignSaltKey))
	}

	// 反证：H5 的 appversion 与原生 App 的 version **不是一个值**
	if H5AppVersion == IOSAppVersion {
		t.Error("H5 的 appversion 带第 4 段（12.6.0.0），原生的不带 —— 别合并成一个常量")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 签名
// ─────────────────────────────────────────────────────────────────────────────

func TestH5SignatureReproducesTheCapturedRequest(t *testing.T) {
	v := loadAppSigninVectors(t)
	if len(v.H5Signature) == 0 {
		t.Fatal("向量里没有 h5Signature 用例")
	}
	for _, c := range v.H5Signature {
		t.Run(c.Name, func(t *testing.T) {
			if c.SaltKey != H5SignSaltKey {
				t.Errorf("向量给的 saltKey=%d，代码里是 %d", c.SaltKey, H5SignSaltKey)
			}
			sign := SignH5RequestWith(c.RequestID, c.Timestamp, c.Nonce)
			if sign.Signature != c.ExpectMD5 {
				t.Errorf("签名与实拍不符:\n 期望 %s\n 实得 %s", c.ExpectMD5, sign.Signature)
			}
			// 四个头要一一对应：签名里用的 timestamp 必须就是发出去的那个
			if sign.Timestamp != c.Timestamp {
				t.Errorf("x-timestamp 必须原样带回（它就是签名的一部分）: %s", sign.Timestamp)
			}
			if sign.RequestID != c.RequestID || sign.Nonce != c.Nonce {
				t.Error("request-id / nonce 应原样带回")
			}
		})
	}
}

// 反证：换一个盐就得到另一串 —— 这条防止有人把 H5 签名"顺手"换成 B 族那套。
func TestH5SignatureIsNotTheBFamilySignature(t *testing.T) {
	v := loadAppSigninVectors(t)
	c := v.H5Signature[0]
	other := core.GenerateSignature(c.RequestID, c.Timestamp, c.Nonce, 1, c.Other)
	if other == c.ExpectMD5 {
		t.Error("换一个盐值不应得到同一个签名 —— 说明测试没真正取值")
	}
}

// seqRandomUpstream 是确定性随机源（递增字节）。
//
// 用它而不是真随机，是因为 `x-request-id` / `x-nonce` 都要进签名 ——
// 只有字节确定，"同一份输入该得到同一份签名"才断言得了。
type seqRandomUpstream struct{ b byte }

func (r *seqRandomUpstream) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.b
		r.b++
	}
}

func TestSignH5RequestIsDeterministicGivenTheRNG(t *testing.T) {
	first := SignH5Request(&seqRandomUpstream{}, "1789800128813")
	second := SignH5Request(&seqRandomUpstream{}, "1789800128813")
	if first != second {
		t.Errorf("同一个 rng 序列应得到同一份签名: %+v vs %+v", first, second)
	}
	if first.Timestamp != "1789800128813" {
		t.Errorf("时间戳应原样带进签名: %s", first.Timestamp)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 编码：deviceId 的 `+` 在查询串里是空格
// ─────────────────────────────────────────────────────────────────────────────

func TestPercentEncodeEscapesBase64Specials(t *testing.T) {
	cases := []struct{ in, want string }{
		{"+", "%2B"},
		{"/", "%2F"},
		{"=", "%3D"},
		{"abcXYZ019-._~", "abcXYZ019-._~"},
		{"Babc+/=", "Babc%2B%2F%3D"},
	}
	for _, c := range cases {
		if got := PercentEncode(c.in); got != c.want {
			t.Errorf("PercentEncode(%q) = %q，期望 %q", c.in, got, c.want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 头：content-type 只能有一个
// ─────────────────────────────────────────────────────────────────────────────

func countHeader(headers []core.HeaderPair, name string) int {
	n := 0
	for _, h := range headers {
		if strings.EqualFold(h[0], name) {
			n++
		}
	}
	return n
}

// headerValue 取一个头的值；没有则返回空串（"有没有"另用 countHeader 断言）。
func headerValue(headers []core.HeaderPair, name string) string {
	for _, h := range headers {
		if strings.EqualFold(h[0], name) {
			return h[1]
		}
	}
	return ""
}

func TestH5HeadersCarryExactlyOneContentType(t *testing.T) {
	request, err := core.NewRequestPost(BaseMcloud).WithJSONBody(map[string]any{"a": 1})
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	jwt := "jwt-905"
	out := WithH5Headers(request, "https://referer", &jwt, "true")

	if n := countHeader(out.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应恰好一个，实得 %d", n)
	}
	if got := headerValue(out.Headers, "Content-Type"); got != H5ContentType {
		t.Errorf("content-type 应是带 charset 的那个: %q", got)
	}
	// 头名是**小写** `jwttoken`（H5 拦截器原样如此）
	if got := headerValue(out.Headers, "jwttoken"); got != "jwt-905" {
		t.Errorf("jwttoken 头不对: %q", got)
	}
	if got := headerValue(out.Headers, "activityid"); got != MarketName {
		t.Errorf("activityid 应是市场名: %q", got)
	}
	if got := headerValue(out.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 应原样带回: %q", got)
	}
}

// 未建会话时（`tyrzLogin` 那一步）**不能**带 `jwttoken`。
func TestH5HeadersOmitJWTTokenBeforeTheSessionExists(t *testing.T) {
	out := WithH5Headers(core.NewRequestGet(BaseMcloud), "https://referer", nil, "false")
	if countHeader(out.Headers, "jwttoken") != 0 {
		t.Error("还没建会话时不该带 jwttoken")
	}
	// GET 无 body ⇒ 也不该有 content-type
	if countHeader(out.Headers, "content-type") != 0 {
		t.Error("没有 body 时不该发 content-type")
	}
}

func TestAppHeadersReplaceContentTypeInsteadOfDuplicatingIt(t *testing.T) {
	request, err := core.NewRequestPost(BaseUserYun).WithJSONBody(map[string]any{"toSourceId": ToSourceID})
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	out := WithAppHeaders(request, appProfile(), 1789743362439040)
	if n := countHeader(out.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应恰好一个，实得 %d（两个会被上游按第一个解析）", n)
	}
	if got := headerValue(out.Headers, "Content-Type"); got != H5ContentType {
		t.Errorf("content-type 应是带 charset 的那个: %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 业务码诊断：别按 msg 归类
// ─────────────────────────────────────────────────────────────────────────────

func TestDiagnoseKeepsTheTwoReceiveFailuresApart(t *testing.T) {
	// 614 / 615 的 msg 都说"活动太火爆"，但成因完全不同：
	// 一个内容无效、一个压根没带 —— 提示必须把这两件事分开说
	code614, ok614 := Diagnose(CodeReceiveFailed)
	code615, ok615 := Diagnose(CodeReceiveFailedAlt)
	if !ok614 || !ok615 {
		t.Fatal("两个领取失败码都应有已知提示")
	}
	if code614 == code615 {
		t.Error("614 与 615 的提示不得相同 —— 它们的成因不同（内容无效 vs 没带）")
	}
	if !strings.Contains(code615, "没带") || !strings.Contains(code614, "无效") {
		t.Errorf("提示应点明成因:\n 614: %s\n 615: %s", code614, code615)
	}
	// 而且两者都**不该**建议退避重试 —— 那是 11029 才有的语义
	for _, hint := range []string{code614, code615} {
		if strings.Contains(hint, "退避") || strings.Contains(hint, "重试") {
			t.Errorf("参数错误不该建议重试: %s", hint)
		}
	}
	busy, _ := Diagnose(CodeReceiveTooBusy)
	if !strings.Contains(busy, "退避") {
		t.Errorf("11029 才该说退避: %s", busy)
	}
	if unknown, ok := Diagnose(500); ok {
		t.Errorf("未知码不该给提示: %s", unknown)
	}
}

// `90001` 必须是**类型化**的"会话过期"。
//
// 反向改坏各有一处会红：① 把 [EnsureOKWithHint] 里那一支改回 `NewBadRequest`
// ⇒ `IsSessionExpired` 那一条红；② 顺手把别的码也判成过期 ⇒ 循环那一条红；
// ③ 把这个变体的 kind 分开（判成 `KindAuth`）⇒ kind 与 `Error()` 渲染两条红。
func TestNotLoggedInIsTypedAsSessionExpired(t *testing.T) {
	_, err := EnsureOKWithHint(envelopeFrom(t, map[string]any{
		"code": CodeNotLoggedIn, "msg": "未登录",
	}))
	if err == nil {
		t.Fatal("90001 应判失败")
	}
	if !err.IsSessionExpired() {
		t.Errorf("90001 必须带'会话过期'这个变体 —— app 层靠它决定刷新重放: %v", err)
	}
	// 加了变体**不等于**改了分类：对外文案与健康判定必须逐字不变。
	if err.Kind() != core.KindBadRequest {
		t.Errorf("kind 应仍是 BadRequest（否则 90001 的对外文案会跟着变）: %v", err.Kind())
	}
	if !strings.HasPrefix(err.Error(), "参数错误: ") {
		t.Errorf("Error() 渲染须与加变体前一致: %q", err.Error())
	}
	if !strings.Contains(err.Message(), "会话没建起来") {
		t.Errorf("Diagnose 的提示仍应在正文里: %q", err.Message())
	}
	// 包一层前缀是这一族的常态（同一接口在一次运行里被调多次），
	// 变体不能被洗掉 —— 否则"该重放"在最常见的那条路径上失效。
	if !err.WithPrefix("infoV3: ").IsSessionExpired() {
		t.Error("WithPrefix 之后仍应是会话过期")
	}
	if !core.IsSessionExpired(err) {
		t.Error("任意 error 版本也应认这个变体")
	}
	if core.IsSessionExpired(nil) || core.IsSessionExpired(core.NewBadRequest("别的错")) {
		t.Error("nil 与其他错误不得判成会话过期")
	}
	// 其余已知码**不是**"刷新会话就能自救"：614/615 是参数问题、11029 是真忙。
	for _, code := range []int64{CodeReceiveFailed, CodeReceiveFailedAlt, CodeReceiveTooBusy} {
		_, other := EnsureOKWithHint(envelopeFrom(t, map[string]any{"code": code, "msg": "活动太火爆"}))
		if other == nil {
			t.Fatalf("%d 应判失败", code)
		}
		if other.IsSessionExpired() {
			t.Errorf("%d 不是会话过期 —— 重建会话救不了它", code)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 建会话三段
// ─────────────────────────────────────────────────────────────────────────────

func TestSigninPageURLCarriesTheSourceIDs(t *testing.T) {
	url := SigninPageURL("SSO-TOKEN")
	for _, want := range []string{
		"https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin",
		"sourceid=" + SourceID,
		"appID=" + H5AppID,
		"token=SSO-TOKEN",
		"targetSourceId=" + ToSourceID,
	} {
		if !strings.Contains(url, want) {
			t.Errorf("URL 应含 %s: %s", want, url)
		}
	}
}

func TestQuerySpecTokenRequestUsesTheNativeHeaders(t *testing.T) {
	body := QuerySpecTokenBody(ToSourceID)
	request, err := QuerySpecTokenRequest(appProfile(), 1789743362439040, "Basic Zm9v", body)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if request.URL != "https://user-njs.yun.139.com/user/querySpecTokenV2" {
		t.Errorf("URL 不对: %s", request.URL)
	}
	if got := headerValue(request.Headers, "Authorization"); got != "Basic Zm9v" {
		t.Errorf("Authorization 应原样带上: %q", got)
	}
	if got := headerValue(request.Headers, "x-yun-user-timestamp"); got != "1789743362439040" {
		t.Errorf("时间戳应是微秒级: %q", got)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个，实得 %d", n)
	}
	// 未登录态不该带 x-yun-uni
	if countHeader(request.Headers, "x-yun-uni") != 0 {
		t.Error("换 SSO token 这一步还没有 userDomainId")
	}
}

func TestTyrzLoginRequestCarriesTheFourSignatureHeaders(t *testing.T) {
	sign := SignH5RequestWith("rid", "1789800128813", "nonce")
	request, err := TyrzLoginRequest(sign, "https://referer", TyrzLoginBody("SSO-TOKEN"))
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if request.URL != "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin" {
		t.Errorf("URL 不对: %s", request.URL)
	}
	cases := map[string]string{
		"x-request-id": sign.RequestID,
		"x-timestamp":  sign.Timestamp,
		"x-nonce":      sign.Nonce,
		"x-signature":  sign.Signature,
		"Origin":       BaseMcloud,
		"showloading":  "true",
	}
	for name, want := range cases {
		if got := headerValue(request.Headers, name); got != want {
			t.Errorf("%s: 期望 %q 实得 %q", name, want, got)
		}
	}
	// 这一步正是为了拿 jwt，所以**没有** jwttoken 头
	if countHeader(request.Headers, "jwttoken") != 0 {
		t.Error("tyrzLogin 不该带 jwttoken —— 它就是来换 jwt 的")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析：逐条走黄金向量里的真实响应
// ─────────────────────────────────────────────────────────────────────────────

// envelopeFrom 把向量里的 `raw` 还原成一个信封。
func envelopeFrom(t *testing.T, raw any) *Envelope {
	t.Helper()
	buf, err := core.MarshalJSON(raw)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	// ⚠ **别**写成 `envelope, err := ParseEnvelope(buf)`：
	// `err` 上面已经由 `core.MarshalJSON` 声明成 `error`（接口）了，
	// 同一个 `:=` 只是**赋值**不重新声明类型，于是 `ParseEnvelope` 返回的
	// **nil `*core.Error`** 被塞进接口 ⇒ `err != nil` 恒为真，而 `%v` 打印 `<nil>`。
	// 换一个变量名是最省事的修法（见方案 §6.1 #38）。
	envelope, perr := ParseEnvelope(buf)
	if perr != nil {
		t.Fatalf("解析信封失败: %v", perr)
	}
	return envelope
}

func TestSigninEnvelopesAreClassifiedAsCaptured(t *testing.T) {
	v := loadAppSigninVectors(t)
	if len(v.Envelopes) == 0 {
		t.Fatal("向量里没有 envelopes 用例")
	}
	for _, c := range v.Envelopes {
		t.Run(c.Name, func(t *testing.T) {
			envelope := envelopeFrom(t, c.Raw)
			code, ok := envelope.Code()
			if !ok {
				t.Fatalf("取不到业务码")
			}
			if code != c.Code {
				t.Errorf("业务码: 期望 %d 实得 %d", c.Code, code)
			}

			// 已知码必须给出提示（这正是"别照 msg 归类"的落点）
			if _, known := Diagnose(code); known {
				_, err := EnsureOKWithHint(envelope)
				if err == nil {
					t.Fatalf("code=%d 应判为失败", code)
				}
				hint, _ := Diagnose(code)
				if !strings.Contains(err.Message(), hint) {
					t.Errorf("detail 里应带上排查提示:\n 期望含 %s\n 实得 %s", hint, err.Message())
				}
			}
		})
	}
}

func TestParseSSOTokenAndServerTimestampFromTheVector(t *testing.T) {
	v := loadAppSigninVectors(t)
	for _, c := range v.Envelopes {
		switch c.Name {
		case "querySpecTokenV2-success":
			token, err := ParseSSOToken(envelopeFrom(t, c.Raw))
			if err != nil {
				t.Fatalf("解析 SSO token 失败: %v", err)
			}
			if token != c.ExpectToken {
				t.Errorf("SSO token: 期望 %s 实得 %s", c.ExpectToken, token)
			}
		case "h5-timestamp-success":
			ts, err := ParseServerTimestamp(envelopeFrom(t, c.Raw))
			if err != nil {
				t.Fatalf("解析服务端时间戳失败: %v", err)
			}
			// 必须是**字符串形式**的 13 位毫秒 —— 它下一步要原样进签名
			if ts != "1789800128813" || len(ts) != 13 {
				t.Errorf("时间戳应是字符串形式的 13 位毫秒: %q", ts)
			}
		}
	}
}

func TestParseJWTTokenReadsTheOptionalMobile(t *testing.T) {
	withMobile := envelopeFrom(t, map[string]any{
		"success": true, "code": "0000",
		"result": map[string]any{"token": "jwt-905", "mobile": "19900000000"},
	})
	login, err := ParseJWTToken(withMobile)
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if login.JWTToken != "jwt-905" {
		t.Errorf("jwt 不对: %s", login.JWTToken)
	}
	if login.Mobile == nil || *login.Mobile != "19900000000" {
		t.Errorf("mobile 应原样带回: %v", login.Mobile)
	}

	// `mobile` 缺失不该让整次登录失败 —— 它只是回显，不是凭据
	without := envelopeFrom(t, map[string]any{
		"success": true, "code": "0000", "result": map[string]any{"token": "jwt-905"},
	})
	login2, err := ParseJWTToken(without)
	if err != nil {
		t.Fatalf("缺 mobile 不该失败: %v", err)
	}
	if login2.Mobile != nil {
		t.Errorf("缺 mobile 时应为 nil: %v", login2.Mobile)
	}

	// 缺 `token` 必须报错（那才是凭据）
	if _, err := ParseJWTToken(envelopeFrom(t, map[string]any{
		"success": true, "code": "0000", "result": map[string]any{"mobile": "19900000000"},
	})); err == nil {
		t.Error("缺 token 应报错")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 取值 helper：只认字符串 / 整数，不顺手做类型转换
// ─────────────────────────────────────────────────────────────────────────────

func TestRequireStrRefusesNumbersButRequireInt64AcceptsThem(t *testing.T) {
	payload := any(map[string]any{"n": json.Number("614"), "s": "text"})
	if _, err := RequireStr(payload, "n"); err == nil {
		t.Error("数字不该被当成字符串取出来")
	}
	n, err := RequireInt64(payload, "n")
	if err != nil || n != 614 {
		t.Errorf("整数应取得到: %d / %v", n, err)
	}
	if _, err := RequireInt64(payload, "s"); err == nil {
		t.Error("字符串不该被当成整数取出来")
	}
	if _, err := RequireStr(payload, "missing"); err == nil {
		t.Error("缺字段应报错")
	}
}
