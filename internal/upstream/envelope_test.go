package upstream

import (
	"fmt"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

func envelopeOf(t *testing.T, text string) *Envelope {
	t.Helper()
	env, err := ParseEnvelope([]byte(text))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	return env
}

func TestEnvelopeReadsNumericCodeAndPayload(t *testing.T) {
	env := envelopeOf(t, `{"code": 0, "msg": "success", "result": {"a": 1}}`)
	if code, ok := env.Code(); !ok || code != 0 {
		t.Errorf("code 实得 %d / %v", code, ok)
	}
	if msg, ok := env.Message(); !ok || msg != "success" {
		t.Errorf("message 实得 %q / %v", msg, ok)
	}
	payload, ok := env.Payload()
	if !ok {
		t.Fatal("应当有载荷")
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		t.Fatalf("载荷不是对象：%#v", payload)
	}
	if n, ok := asInt64(obj["a"]); !ok || n != 1 {
		t.Errorf("payload.a 实得 %v / %v", obj["a"], ok)
	}
	if _, err := env.EnsureOK(); err != nil {
		t.Errorf("ensure_ok 应成功，实得 %v", err)
	}
}

// TestEnvelopeReadsStringCodeAndSuccessFlag —— `RefreshTokenResponse` 的 code
// 是字符串，只认数字会把它当成"没有信封"。
func TestEnvelopeReadsStringCodeAndSuccessFlag(t *testing.T) {
	okEnv := envelopeOf(t, `{"success": true, "code": "0", "message": "ok"}`)
	if code, ok := okEnv.Code(); !ok || code != 0 {
		t.Errorf("字符串 code 应被认成数字，实得 %d / %v", code, ok)
	}
	if success, ok := okEnv.Success(); !ok || !success {
		t.Errorf("success 实得 %v / %v", success, ok)
	}
	if _, err := okEnv.EnsureOK(); err != nil {
		t.Errorf("应成功，实得 %v", err)
	}

	// `success: false` 必须判失败，且**优先于** code。
	// 折进 code 会得到 0，于是失败被当成成功 —— 这是踩过的坑，用这条钉住。
	failed := envelopeOf(t, `{"success": false, "code": "0", "message": "凭据失效"}`)
	if _, err := failed.EnsureOK(); err == nil {
		t.Error("success=false 不能被判成功")
	}
}

func TestEnvelopeWithoutAnyCodeIsTreatedAsOK(t *testing.T) {
	// 有些接口直接回数组（如 `taskListV2` 的 result），没有信封字段
	bare := envelopeOf(t, `[{"a": 1}]`)
	if _, ok := bare.Code(); ok {
		t.Error("裸数组不该解出 code")
	}
	if _, err := bare.EnsureOK(); err != nil {
		t.Errorf("没有 code 应算成功，实得 %v", err)
	}
}

func TestBusinessFailureCarriesCodeAndMessage(t *testing.T) {
	env := envelopeOf(t, `{"code": 514, "msg": "验证失败"}`)
	_, err := env.EnsureOK()
	if err == nil {
		t.Fatal("应当失败")
	}
	if got := err.Error(); !contains(got, "514") || !contains(got, "验证失败") {
		t.Errorf("详情应含码与消息：%s", got)
	}
	// 对外文案走白名单，不夹带上游原文
	if got := err.PublicMessage(); got != "请求参数不合法" {
		t.Errorf("对外文案实得 %q", got)
	}
}

// TestEnvelopeKeepsNineteenDigitIdentifiersExact 守住"数字不许解成 float64"。
//
// 19 位的 `userDomainId` 一旦被解成 float64，末几位会**静默归零** ——
// 而它恰好是登录结果里用来定位用户域的那一个值。
func TestEnvelopeKeepsNineteenDigitIdentifiersExact(t *testing.T) {
	const want = "1990000000000000001"
	env := envelopeOf(t, `{"code": 0, "data": {"userDomainId": "`+want+`"}}`)
	payload, _ := env.Payload()
	obj := payload.(map[string]any)
	got, ok := obj["userDomainId"].(string)
	if !ok || got != want {
		t.Fatalf("实得 %#v（应是原样字符串）", obj["userDomainId"])
	}
}

func TestParseEnvelopeRejectsBrokenJSON(t *testing.T) {
	if _, err := ParseEnvelope([]byte(`{"code":`)); err == nil {
		t.Error("坏 JSON 应报错")
	}
}

func TestHTTPStatusMapsToErrorKind(t *testing.T) {
	cases := []struct {
		status uint16
		want   core.ErrorKind
	}{
		{401, core.KindAuth},
		{429, core.KindRateLimited},
		{408, core.KindTimeout},
		{400, core.KindBadRequest},
		{503, core.KindUpstream},
	}
	for _, c := range cases {
		err := errorFromStatus(c.status)
		if got := err.Kind(); got != c.want {
			t.Errorf("HTTP %d 实得 %v 期望 %v", c.status, got, c.want)
		}
		// **不带响应体原文**：上游错误体里常常直接回显凭据。
		// 用 [core.Error.Message] 而不是 `Error()` —— 后者带类别前缀（给日志看）。
		if got := err.Message(); got != fmt.Sprintf("HTTP %d", c.status) {
			t.Errorf("详情应只有状态码，实得 %q", got)
		}
	}
}

func contains(haystack, needle string) bool {
	for i := 0; i+len(needle) <= len(haystack); i++ {
		if haystack[i:i+len(needle)] == needle {
			return true
		}
	}
	return false
}
