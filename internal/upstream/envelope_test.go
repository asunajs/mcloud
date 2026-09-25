package upstream

import (
	"fmt"
	"strings"
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

// 58999 那一份**逐字**取自 2026-09-25 用户提供的线上采集（iOS，客户端 13.1.1）：
// `task/click` 对 22 个任务 id 全部回这一句。它之所以是夹具而不是编出来的样例，
// 是因为要验的正是"`message` 与 `detail` 说的不是同一件事"这一格 —— 两句话都得是真的。
const envelopeSample58999 = `{"code":58999,` +
	`"message":"当前用户信息读取异常，请尝试重新登录或重启手机",` +
	`"detail":"任务不存在","tipMsg":null}`

// TestEnvelopeDetailReachesFailureText ⭐ 坑表 #149 / Q40 的落点判据。
//
// 反向改坏它的四处，各自会红：
//   - 把 [Envelope.Detail] 写成"什么类型都收"（`fmt.Sprint(value)`）⇒ 第二条断言红
//     （`smid` 那一口的 `detail` 是**载荷容器**，整块拼进文本等于把 deviceId 打进日志）；
//   - 把"与 `message` 逐字相同就不拼"那一句删掉 ⇒ 第三条断言红（同一句出现两遍）；
//   - 把截断删掉 ⇒ 第四条断言红（超长那一整段会原样进日志与逐条播报）；
//   - 把 `EnsureOK` 里的 `withDetail` 调用整个删掉 ⇒ 第一条断言红（回到"只剩误导的那一句"）。
func TestEnvelopeDetailReachesFailureText(t *testing.T) {
	env := envelopeOf(t, envelopeSample58999)
	_, err := env.EnsureOK()
	if err == nil {
		t.Fatal("58999 该判失败")
	}
	got := err.Error()
	if !contains(got, "上游返回 code=58999") || !contains(got, "任务不存在") {
		t.Errorf("失败文本要同时给出码与 `detail` 那一句，实得 %q", got)
	}
	if !contains(got, "请尝试重新登录或重启手机") {
		t.Errorf("⚠ 不得**替换**掉 `message`：本层不判哪一句是对的，实得 %q", got)
	}
	// 对外文案仍然是白名单那一句：`detail` 与 `message` 一样**只进日志**。
	if public := err.PublicMessage(); public != "请求参数不合法" || contains(public, "任务不存在") {
		t.Errorf("对外文案不该带上游原文，实得 %q", public)
	}

	// `detail` 是对象（`smid` 那一口的形状）⇒ 读成"没给"，一个字都不拼。
	objectDetail := envelopeOf(t, `{"code":1100,"message":"取到了","detail":{"deviceId":"SENTINEL-xyz"}}`)
	_, err = objectDetail.EnsureOK()
	if err == nil {
		t.Fatal("code=1100 该判失败")
	}
	if contains(err.Error(), "SENTINEL") || contains(err.Error(), "detail") {
		t.Errorf("`detail` 是载荷容器时不许拼进文本，实得 %q", err.Error())
	}

	// 与 `message` 逐字相同 ⇒ 不重复拼。（这里**不能**用"参数错误"当面值 ——
	// 它是 [core.Error.Error] 给 BadRequest 加的类别前缀，会把计数读成 2。）
	same := envelopeOf(t, `{"code":400,"message":"活动已结束","detail":"活动已结束"}`)
	_, err = same.EnsureOK()
	if err == nil {
		t.Fatal("该判失败")
	}
	if n := strings.Count(err.Error(), "活动已结束"); n != 1 {
		t.Errorf("同一句只该出现一次，实得 %d 次：%q", n, err.Error())
	}

	// 超长 ⇒ 截到上限并留省略号。
	long := envelopeOf(t, `{"code":400,"message":"m","detail":"`+strings.Repeat("啊", envelopeDetailLimit+10)+`"}`)
	_, err = long.EnsureOK()
	if err == nil {
		t.Fatal("该判失败")
	}
	if !strings.HasSuffix(err.Error(), "…") ||
		strings.Count(err.Error(), "啊") != envelopeDetailLimit {
		t.Errorf("超长要截到上限并带省略号，实得 %q", err.Error())
	}

	// 空串与纯空白 ⇒ 与"没有那一格"同档（不拼出个空尾巴）。
	for _, body := range []string{`{"code":400,"message":"m","detail":""}`,
		`{"code":400,"message":"m","detail":"  "}`} {
		_, err = envelopeOf(t, body).EnsureOK()
		if err == nil {
			t.Fatalf("该判失败：%s", body)
		}
		if contains(err.Error(), "detail") {
			t.Errorf("空白不该拼出尾巴，实得 %q", err.Error())
		}
	}

	// `success=false` 那一支也走同一处（它同样可能带 `detail`）。
	_, err = envelopeOf(t, `{"success":false,"detail":"活动已结束"}`).EnsureOK()
	if err == nil || !contains(err.Error(), "活动已结束") {
		t.Errorf("success=false 那一支也要拼上 detail，实得 %v", err)
	}
}

// TestEnvelopeWithoutDetailKeepsTheOldText 钉住**没有**那一格时的形状不变。
//
// 反向改坏它：把 `withDetail` 写成"无论如何都拼个尾巴"⇒ 两条断言都红
// （而全仓那些逐字断言失败文本的用例会成片红 —— 这条是它们的**前置**，不是冗余）。
func TestEnvelopeWithoutDetailKeepsTheOldText(t *testing.T) {
	_, err := envelopeOf(t, `{"code":514,"msg":"验证失败"}`).EnsureOK()
	if err == nil || err.Error() != "参数错误: 上游返回 code=514: 验证失败" {
		t.Errorf("没有 `detail` 时文本该逐字照旧，实得 %v", err)
	}
	_, err = envelopeOf(t, `{"code":514}`).EnsureOK()
	if err == nil || err.Error() != "参数错误: 上游返回 code=514" {
		t.Errorf("没消息时也不该多出冒号或尾巴，实得 %v", err)
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
		// ⚠ 404 必须**显式**在表里：这一侧的 `default` 是 NewUpstream（可重试），
		// 漏了那条 case 的表现不是"标签不好看"，是"为一个不存在的端点重试到死"。
		{404, core.KindNotFound},
		{405, core.KindBadRequest},
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
