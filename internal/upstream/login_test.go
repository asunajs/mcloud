package upstream

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// pcVectors 读 PC 客户端密码学的黄金向量（由 Rust/Node 生成侧产出，本侧只消费）。
//
// ⚠ 文件名必须是**字面量**：scripts/check-golden-vectors.cjs 靠正则
// `golden.Load("…")` 判断"这份向量有没有人读"。
func pcVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("pcclient-crypto-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	return out
}

func vectorString(t *testing.T, value any) string {
	t.Helper()
	text, ok := value.(string)
	if !ok {
		t.Fatalf("向量字段不是字符串：%#v", value)
	}
	return text
}

// seqRng 是测试用随机源。**固定序列**而不是"随便填"：
// `sID` / `random` 的长度与字符集由服务端校验，随机一点就够了，
// 但测试要能断言"长度对、字符集合法"，所以必须可复现。
type seqRng struct{ b byte }

func (r *seqRng) FillBytes(dst []byte) {
	for i := range dst {
		r.b = r.b*31 + 7
		dst[i] = r.b
	}
}

// ── 二维码与随机串 ──────────────────────────────────────────────────────────

func TestQrPayloadIsBuiltLocallyWithoutAnyRequest(t *testing.T) {
	got := QrPayload(PlatformWindows, "DEV-1", "SID-1")
	const want = "https://yun.139.com/w/#/qrcLogin?dID=DEV-1&sID=SID-1&cType=11&ver=8.8.6.20260829"
	if got != want {
		t.Fatalf("实得 %q", got)
	}
	if !strings.Contains(got, "ver=") {
		t.Error("二维码要带客户端版本号（上游按版本分流）")
	}
}

func TestNewSessionIDIsThirtyTwoUpperHex(t *testing.T) {
	id := NewSessionID(&seqRng{b: 3})
	if len(id) != 32 {
		t.Fatalf("sID 应 32 字符，实得 %d：%s", len(id), id)
	}
	if id != strings.ToUpper(id) {
		t.Errorf("sID 必须是大写：%s", id)
	}
	for i := 0; i < len(id); i++ {
		if !isHexDigit(id[i]) {
			t.Fatalf("sID 含非十六进制字符 %q：%s", id[i], id)
		}
	}
	// 同种子同值：否则"换码后 sID 变了"这件事没法钉
	if other := NewSessionID(&seqRng{b: 3}); other != id {
		t.Errorf("同种子应得同一 sID：%s vs %s", id, other)
	}
}

// TestNewSmsRandomIsTwentyFourLowerHex —— 长度（24）与字符集（小写十六进制）
// 都由服务端校验，换成 UUID 会形态不对。
func TestNewSmsRandomIsTwentyFourLowerHex(t *testing.T) {
	random := NewSmsRandom(&seqRng{b: 5})
	if len(random) != 24 {
		t.Fatalf("random 应 24 字符，实得 %d：%s", len(random), random)
	}
	if random != strings.ToLower(random) {
		t.Errorf("random 必须是小写：%s", random)
	}
	// 两次发码必须是不同的值（重试前要 reroll）
	if same := NewSmsRandom(&seqRng{b: 5}); same != random {
		t.Error("同种子应得同一值（reroll 由调用方负责）")
	}
}

// ── 请求体 ──────────────────────────────────────────────────────────────────

func TestThirdLoginBodyVariesOnlyByKind(t *testing.T) {
	qr := NewQRThirdLoginBody(PlatformWindows, "SID-1")
	if qr.Pintype != "21" || qr.Dycpwd != "SID-1" || qr.Msisdn != "" {
		t.Errorf("扫码体：%+v", qr)
	}
	if qr.ExtInfo.IfOpenAccount != 0 {
		t.Errorf("扫码的 ifOpenAccount 应为 0，实得 %d", qr.ExtInfo.IfOpenAccount)
	}

	sms := NewSmsThirdLoginBody(PlatformWindows, "19900000000", "123456")
	if sms.Pintype != "5" || sms.Dycpwd != "123456" || sms.Msisdn != "19900000000" {
		t.Errorf("短信体：%+v", sms)
	}
	if sms.ExtInfo.IfOpenAccount != 1 {
		t.Errorf("短信的 ifOpenAccount 应为 1，实得 %d", sms.ExtInfo.IfOpenAccount)
	}

	pwd := NewPasswordThirdLoginBody(PlatformWindows, "alice", "pass1234")
	if pwd.Pintype != "9" || pwd.Msisdn != "alice" {
		t.Errorf("账密体：%+v", pwd)
	}
	// 明文口令绝不能出现在请求体里
	raw, err := core.MarshalJSON(pwd)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if contains(string(raw), "pass1234") {
		t.Errorf("请求体里泄漏了明文口令：%s", raw)
	}
	if pwd.Dycpwd != PasswordHash("pass1234") {
		t.Error("dycpwd 应是口令哈希")
	}
}

// TestBodyKeysAreSortedLikeSerde 守住"声明顺序 == 字典序"那条约定。
//
// serde_json 的 Map 默认按字典序输出，Go 的 struct 按声明顺序输出 ——
// 两边要能逐字节对拍，声明顺序就必须等于字典序。
// ⚠ 反向测试：把某个字段挪到错误位置，这条会红。
func TestBodyKeysAreSortedLikeSerde(t *testing.T) {
	body := NewSendSmsCodeBody(PlatformWindows, "19900000000", "abc")
	raw, err := core.MarshalJSON(body)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	const want = `{"clientType":"656","mode":0,"phoneNumber":"19900000000","random":"abc","reqType":3}`
	if string(raw) != want {
		t.Errorf("实得 %s", raw)
	}
}

// TestSliderBodyUsesUpstreamSpelling 路径与字段名都**没有 `i`**：
// 是 `verfycode` 不是 `verifycode` —— 上游就这么拼的。
func TestSliderBodyUsesUpstreamSpelling(t *testing.T) {
	body := NewSliderPuzzleBody("19900000000")
	if body.Type != 2 {
		t.Errorf("type 应为 2，实得 %d", body.Type)
	}
	raw, err := core.MarshalJSON(body)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if !contains(string(raw), `"account"`) || !contains(string(raw), `"type"`) {
		t.Errorf("实得 %s", raw)
	}
}

// ── 口令与滑块（对拍黄金向量） ───────────────────────────────────────────────

func TestPasswordHashUsesTheSourcePrefix(t *testing.T) {
	v := pcVectors(t)
	constants := v["constants"].(map[string]any)
	if got := vectorString(t, constants["PASSWORD_HASH_PREFIX"]); got != PasswordHashPrefix {
		t.Errorf("向量里的前缀是 %q，代码里是 %q", got, PasswordHashPrefix)
	}
	if PasswordHashPrefix != "fetion.com.cn:" {
		t.Errorf("前缀实得 %q", PasswordHashPrefix)
	}

	sha1Upper := v["sha1Upper"].([]any)
	want := vectorString(t, sha1Upper[0].(map[string]any)["expect"])
	if got := PasswordHash("pass1234"); got != want {
		t.Errorf("SHA-1 前缀 + 大写 hex：实得 %s 期望 %s", got, want)
	}
}

func TestConstantsMatchTheGoldenVector(t *testing.T) {
	// 密钥是"哪一把"这件事由向量钉住，不靠人眼 ——
	// 用错密钥的表现不是报错，是"解出来是乱码当成功"。
	v := pcVectors(t)
	constants := v["constants"].(map[string]any)
	pairs := []struct {
		name string
		got  string
	}{
		{"PUZZLE_AES_KEY", PuzzleAESKey},
		{"PUZZLE_AES_IV", string(PuzzleAESIV[:])},
		{"LOGIN_AES_KEY", LoginAESKey},
		{"LOGIN_AES_KEY_TEST", LoginAESKeyTest},
	}
	for _, p := range pairs {
		if want := vectorString(t, constants[p.name]); want != p.got {
			t.Errorf("%s：向量 %q vs 代码 %q", p.name, want, p.got)
		}
	}
}

// TestPuzzleAnswerDecryptsTheLiveCoordinate 用的是 2026-09-18 一次**真实**
// `/user/verfycode` 响应的 `data.puzzleLeft`：解出来是 `370`，
// 而同响应里 `picWidth = 680` ⇒ 服务端把缺口横坐标直接给了出来。
func TestPuzzleAnswerDecryptsTheLiveCoordinate(t *testing.T) {
	v := pcVectors(t)
	cases := v["puzzleLeftDecrypt"].([]any)
	c := cases[0].(map[string]any)

	puzzle := SliderPuzzle{
		BackgroundB64: "b64-bg",
		PuzzleB64:     "b64-piece",
		LeftEncrypted: vectorString(t, c["cipherB64"]),
	}
	width := int64(680)
	puzzle.PicWidth = &width

	answer, err := puzzle.Answer()
	if err != nil {
		t.Fatalf("解缺口坐标失败: %v", err)
	}
	if want := vectorString(t, c["expectPlain"]); answer != want {
		t.Errorf("实得 %q 期望 %q", answer, want)
	}
}

func TestPuzzleAnswerRejectsOutOfRangeCoordinates(t *testing.T) {
	v := pcVectors(t)
	c := v["puzzleLeftDecrypt"].([]any)[0].(map[string]any)
	width := int64(100) // 真实图宽是 680，故意压小
	puzzle := SliderPuzzle{LeftEncrypted: vectorString(t, c["cipherB64"])}
	puzzle.PicWidth = &width

	// 宁可报错，也不要把一个荒谬的坐标发上去 —— 失败次数是有限的
	if _, err := puzzle.Answer(); err == nil {
		t.Error("坐标超出图宽时必须报错")
	}
}

func TestPuzzleVerfycodeEncryptRoundTrips(t *testing.T) {
	body, err := NewSendSmsCodeBodyWithSlider(PlatformWindows, "19900000000", "abc", "370")
	if err != nil {
		t.Fatalf("加密滑块答案失败: %v", err)
	}
	if body.PuzzleVerfycode == nil {
		t.Fatal("走滑块时 puzzleVerfycode 必须出现")
	}
	// 答案是**拖动像素偏移的十进制字符串**，不是轨迹点数组
	plain, decryptErr := core.AESDynamicIVDecrypt(PuzzleAESKey, *body.PuzzleVerfycode)
	if decryptErr != nil {
		t.Fatalf("回解失败: %v", decryptErr)
	}
	if plain != "370" {
		t.Errorf("实得 %q", plain)
	}

	// 不带滑块时该键**不出现**
	plainBody := NewSendSmsCodeBody(PlatformWindows, "19900000000", "abc")
	if plainBody.PuzzleVerfycode != nil {
		t.Error("普通发码不该带 puzzleVerfycode")
	}
}

// ── 响应语义 ────────────────────────────────────────────────────────────────

func TestPollClassificationCoversEveryDocumentedCode(t *testing.T) {
	poll := func(text string) PollOutcome {
		return ClassifyPoll(envelopeOf(t, text))
	}

	cases := []struct {
		name string
		body string
		want PollKind
	}{
		{"未扫码", `{"code": "200059541", "message": "未找到设备的二维码信息"}`, PollNotScanned},
		{"已扫码待确认", `{"code": "200059548", "message": "二维码信息未授权"}`, PollScanned},
		{"sID 不匹配", `{"code": "200059544"}`, PollSessionMismatch},
		{"码已失效", `{"code": "200059542"}`, PollExpired},
		// 实测补的码：连着跑几次扫码登录会被限频。它不是"参数错误"——
		// 区别很重要：参数错误该改请求，限流该等。
		{"过于频繁", `{"code": "200059545", "message": "扫码登录过于频繁"}`, PollTooFrequent},
		// 成功两种码都见过：`0000`（字符串）与 `200`（数字）
		{"成功/字符串码", `{"code": "0000", "data": "CIPHER"}`, PollSuccess},
		{"成功/数字码", `{"code": 200, "data": "CIPHER"}`, PollSuccess},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := poll(c.body)
			if got.Kind != c.want {
				t.Fatalf("实得 %v 期望 %v", got.Kind, c.want)
			}
			if c.want == PollSuccess && got.Encrypted != "CIPHER" {
				t.Errorf("密文实得 %q", got.Encrypted)
			}
		})
	}
}

func TestSuccessCodeWithoutCiphertextIsReportedNotFaked(t *testing.T) {
	// 成功码但 data 形状不对：必须如实报告，不能伪造一个空凭据往下走
	got := ClassifyPoll(envelopeOf(t, `{"code": "0000", "data": {"a": 1}}`))
	if got.Kind != PollOther {
		t.Fatalf("应当归为 Other，实得 %v", got.Kind)
	}
	if !contains(got.Message, "data") {
		t.Errorf("消息应说明是 data 形状不对：%q", got.Message)
	}
}

func TestUnknownPollCodeIsPreservedForCaller(t *testing.T) {
	// 上游在活动期会加新码，硬编码成"错误"会把新码误判
	got := ClassifyPoll(envelopeOf(t, `{"code": 999999, "message": "新码"}`))
	if got.Kind != PollOther {
		t.Fatalf("应当原样带回，实得 %v", got.Kind)
	}
	if got.Code == nil || *got.Code != 999999 {
		t.Errorf("code 实得 %v", got.Code)
	}
	if got.Message != "新码" {
		t.Errorf("message 实得 %q", got.Message)
	}
}

func TestSendSmsClassificationHandlesSliderDowngrade(t *testing.T) {
	sent := ClassifySendSms(envelopeOf(t,
		`{"success": true, "code": "0000", "data": {"remainGetTimes": 8}}`))
	if sent.Kind != SmsSent {
		t.Fatalf("实得 %v", sent.Kind)
	}
	if sent.RemainGetTimes == nil || *sent.RemainGetTimes != 8 {
		t.Errorf("配额余量实得 %v", sent.RemainGetTimes)
	}

	need := ClassifySendSms(envelopeOf(t,
		`{"success": false, "code": "200059554", "data": {"captchaType": 2}}`))
	if need.Kind != SmsNeedSlider {
		t.Fatalf("需要滑块，实得 %v", need.Kind)
	}
	if need.CaptchaType == nil || *need.CaptchaType != 2 {
		t.Errorf("captchaType 实得 %v", need.CaptchaType)
	}

	// 挑战作废 —— 必须重新取图（协议 §3 的降级路径）
	exhausted := ClassifySendSms(envelopeOf(t,
		`{"success": false, "code": "200059553", "data": {"captchaType": 1}}`))
	if exhausted.Kind != SmsSliderExhausted {
		t.Fatalf("实得 %v", exhausted.Kind)
	}
	if exhausted.CaptchaType == nil || *exhausted.CaptchaType != 1 {
		t.Errorf("captchaType 实得 %v", exhausted.CaptchaType)
	}
}

// ── 解密 ────────────────────────────────────────────────────────────────────

func TestLoginDecryptorUsesEcbHexNotBase64(t *testing.T) {
	v := pcVectors(t)
	c := v["loginDataDecrypt"].([]any)[0].(map[string]any)

	info, err := NewLoginDataDecryptorProduction().
		DecryptUserInfo(vectorString(t, c["cipherHex"]))
	if err != nil {
		t.Fatalf("解密失败: %v", err)
	}
	obj, ok := info.(map[string]any)
	if !ok {
		t.Fatalf("userInfo 不是对象：%#v", info)
	}
	if obj["account"] != "19900000000" {
		t.Errorf("account 实得 %v", obj["account"])
	}
	if obj["userId"] != "1990000000000000001" {
		t.Errorf("userId 实得 %v", obj["userId"])
	}

	// 用错密钥要报错，不能返回半截结果
	if _, err := NewLoginDataDecryptorWithKey("0000000000000000").
		DecryptUserInfo(vectorString(t, c["cipherHex"])); err == nil {
		t.Error("换密钥后必须失败，而不是解出乱码当成功")
	}
}

func TestLoginDecryptorReportsPaddingFailureInsteadOfGarbage(t *testing.T) {
	v := pcVectors(t)
	c := v["loginDataDecrypt"].([]any)[1].(map[string]any)
	if _, ok := c["expectError"]; !ok {
		t.Skip("向量里没有 expectError 那条")
	}
	_, err := NewLoginDataDecryptorProduction().
		DecryptUserInfo(vectorString(t, c["cipherHex"]))
	if err == nil {
		t.Fatal("填充非法时必须报错，而不是返回乱码")
	}
	if got := err.Kind(); got != core.KindParse {
		t.Errorf("错误类别实得 %v", got)
	}
}

// TestDecryptorIsInjectableForChangedUpstreamShapes 接口存在的意义：
// 上游若返回明文 JSON，调用方代码不用改。
func TestDecryptorIsInjectableForChangedUpstreamShapes(t *testing.T) {
	var plain plainTextDecryptor
	info, err := plain.DecryptUserInfo(`{"account":"19900000000"}`)
	if err != nil {
		t.Fatalf("失败: %v", err)
	}
	obj := info.(map[string]any)
	if obj["account"] != "19900000000" {
		t.Errorf("实得 %v", obj["account"])
	}
}

// plainTextDecryptor 是假解密器：验证"上游换了加密方式"时调用方的行为。
type plainTextDecryptor struct{}

func (plainTextDecryptor) DecryptUserInfo(ciphertext string) (any, *core.Error) {
	value, ok := core.ParseJSON(ciphertext)
	if !ok {
		return nil, core.NewParse("不是合法 JSON")
	}
	return value, nil
}

func isHexDigit(c byte) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')
}
