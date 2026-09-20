package upstream

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// appLoginPlaintext 是抓包里那条 `thirdlogin` 明文的**脱敏**形态（真号码换成
// `199` 号段）。长度 186 与实测一致 —— 这是"形状没变"的硬约束。
//
// ⚠ 它与 Rust 侧 `crypto/tests/app.rs` 里那份**逐字相同**，不是另编一份：
// 两边的期望值必须来自同一条抓包，否则对拍就变成各自自证。
const appLoginPlaintext = `{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344",` +
	`"pintype":"8","loginMode":"0","verType":"1","version":"12.6.0",` +
	`"msisdn":"19900000000","random":"hOjseB","clienttype":"412"}`

// appLoginWire 是上面那条明文经 **App 那套**（AES-ECB + 末 10 位错位）之后
// 应该得到的线上请求体。
//
// # 它是一次真正的跨实现对拍
//
// 这个值**不是 Go 侧算出来的**：它由本仓库之外、独立实现的 AES-ECB
// （抓包侧的对照脚本）算出，最初记录在 Rust 侧 `crypto/tests/app.rs` 的断言里。
// Go 侧复算出同一串才算过 —— 它同时钉住了 ECB 模式、PKCS7 填充、密钥、
// hex 小写，以及**错位方向**（末 10 位到最前，不是反过来）。
const appLoginWire = "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db6" +
	"23ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff56008" +
	"44ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b1" +
	"6c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52" +
	"959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0"

func testAppProfile() *AppProfile {
	return NewAppProfile(NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"))
}

// ─────────────────────────────────────────────────────────────────────────────
// 密码学
// ─────────────────────────────────────────────────────────────────────────────

// TestAppLoginPlaintextKeepsTheCapturedLength：186 字节 → PKCS7 补 6 →
// 192 字节 → 384 个 hex 字符。这三个数都直接来自抓包，
// 一旦有人"顺手"改了字段，长度会先炸。
func TestAppLoginPlaintextKeepsTheCapturedLength(t *testing.T) {
	if len(appLoginPlaintext) != 186 {
		t.Errorf("明文长度应为 186（抓包原值），实得 %d", len(appLoginPlaintext))
	}
}

// TestAppLoginBodyIsECBHexWithTheTailTenMovedToTheFront —— 本模块**最中心**的一条断言。
//
// ⚠ 反向测试已确认：把 `ShiftTail10ToFront` 换成 `ShiftHead10ToBack` 之后本条变红。
func TestAppLoginBodyIsECBHexWithTheTailTenMovedToTheFront(t *testing.T) {
	crypto := NewAppLoginCryptoProduction()
	ecb, err := core.AESECBPKCS7EncryptHex(crypto.Key(), appLoginPlaintext)
	if err != nil {
		t.Fatalf("ECB 加密失败: %v", err)
	}
	if len(ecb) != 384 {
		t.Errorf("ECB hex 应为 384 字符，实得 %d", len(ecb))
	}
	wire, perr := crypto.EncryptBody(appLoginPlaintext)
	if perr != nil {
		t.Fatalf("加密失败: %v", perr)
	}
	if wire != appLoginWire {
		t.Errorf("应复算出抓包侧那条线上请求体，\n实得 %s", wire)
	}
	// 错位方向反了会得到完全不同的串 —— 单独钉一条，避免"看起来也像随机 hex"。
	if reversed := core.ShiftHead10ToBack(ecb); wire == reversed {
		t.Error("错位方向反了却得到了同一个串 —— 说明这一步根本没生效")
	}
	if wire[:10] != ecb[len(ecb)-10:] {
		t.Errorf("前 10 位应来自 ECB 密文的末 10 位，实得 %q vs %q",
			wire[:10], ecb[len(ecb)-10:])
	}
	if wire[10:] != ecb[:len(ecb)-10] {
		t.Error("其余部分应是 ECB 密文去掉末 10 位后的前缀")
	}
}

// TestAppLoginRoundTrip：加密 → 错位 → 反错位 → 解密，必须回到原文。
func TestAppLoginRoundTrip(t *testing.T) {
	crypto := NewAppLoginCryptoProduction()
	wire, perr := crypto.EncryptBody(appLoginPlaintext)
	if perr != nil {
		t.Fatalf("加密失败: %v", perr)
	}
	back, perr := crypto.DecryptBody(wire)
	if perr != nil {
		t.Fatalf("解密失败: %v", perr)
	}
	if back != appLoginPlaintext {
		t.Errorf("往返应回到原文，实得 %s", back)
	}
}

// TestAppLoginAltKeyProducesADifferentWire：备用密钥那把"用了会得到 9103" ——
// 它是排查密钥问题最快的判据，所以它必须真的不一样。
func TestAppLoginAltKeyProducesADifferentWire(t *testing.T) {
	prod := NewAppLoginCryptoProduction()
	alt := NewAppLoginCryptoAltEnv()
	if prod.Key() == alt.Key() {
		t.Fatal("两把密钥不应相同")
	}
	wireProd, _ := prod.EncryptBody(appLoginPlaintext)
	wireAlt, _ := alt.EncryptBody(appLoginPlaintext)
	if wireProd == wireAlt {
		t.Error("换密钥后线上请求体应不同 —— 相同说明密钥没生效")
	}
	// 用生产密钥去解备用密钥加密的体：必须报错，不能解出乱码
	if _, perr := prod.DecryptBody(wireAlt); perr == nil {
		t.Error("用错密钥时应报错（PKCS7 填充必然非法），静默解出乱码更危险")
	}
}

// TestAppLoginDecryptUserInfoParsesThePayload：解密器要交出**解开的 JSON 值**，
// 而不是明文字符串 —— 调用方拿字符串会自己去 parse 第二次。
func TestAppLoginDecryptUserInfoParsesThePayload(t *testing.T) {
	crypto := NewAppLoginCryptoProduction()
	wire, _ := crypto.EncryptBody(appLoginPlaintext)
	value, perr := crypto.DecryptUserInfo(wire)
	if perr != nil {
		t.Fatalf("解密失败: %v", perr)
	}
	obj, ok := value.(map[string]any)
	if !ok {
		t.Fatalf("应解成对象，实得 %T", value)
	}
	if text, _ := obj["msisdn"].(string); text != "19900000000" {
		t.Errorf("msisdn 应为 19900000000，实得 %v", obj["msisdn"])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求体
// ─────────────────────────────────────────────────────────────────────────────

// TestAppSendSmsBodyValuesAreAllStrings：PC 端这里 `mode` / `reqType` 是数字，
// App 端是**字符串** —— `"mode":"0"` 不是 `"mode":0`。
//
// ⚠ 反向测试已确认：把 `Mode` 改成整数后本条变红。
func TestAppSendSmsBodyValuesAreAllStrings(t *testing.T) {
	buf, err := core.MarshalJSON(NewAppSendSmsCodeBody("19900000000", ""))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	text := string(buf)
	for _, want := range []string{
		`"mode":"0"`,
		`"reqType":"3"`,
		`"clientType":"412"`,
		`"random":"qwert"`,
		`"puzzleVerfycode":""`,
	} {
		if !strings.Contains(text, want) {
			t.Errorf("请求体应含 %s，实得 %s", want, text)
		}
	}
	for _, notWant := range []string{`"mode":0`, `"reqType":3`} {
		if strings.Contains(text, notWant) {
			t.Errorf("App 端这两个值是字符串，不该出现 %s，实得 %s", notWant, text)
		}
	}
}

// TestAppRandomIsTheHardcodedLiteral：`getSmsCode` 的 `random` 是客户端硬编码的
// `qwert`（跨一个月的 4 次抓包全部相同），**不是**每次随机。
// 做成参数会暗示"这里可以也应该变化"。
func TestAppRandomIsTheHardcodedLiteral(t *testing.T) {
	if SendSmsRandom != "qwert" {
		t.Errorf("应为 qwert，实得 %q", SendSmsRandom)
	}
	// 与 thirdlogin 那个 random 不是同一个值：后者 6 位字符集、每请求生成。
	if got := NewLoginRandom(&seqRandomUpstream{}); len(got) != 6 {
		t.Errorf("thirdlogin 的 random 应为 6 位，实得 %q", got)
	}
	for _, ch := range NewLoginRandom(&seqRandomUpstream{}) {
		if !strings.ContainsRune(LoginRandomAlphabet, ch) {
			t.Errorf("random 只应含大小写字母与数字，实得 %q", ch)
		}
	}
}

// TestAppThirdLoginBodyShapeVsPC：两端"同一个接口两套形状"里
// 最容易漏的几处，逐个对照钉住。
//
//   - `extInfo.ifOpenAccount`：App **字符串** `"1"` vs PC **数字** `1`
//   - `clienttype`：App 全小写 `t`；而 `getSmsCode` 里是 `clientType`
//   - `pintype`：短信登录也是 `8`，不是 PC 的 `5`
func TestAppThirdLoginBodyShapeVsPC(t *testing.T) {
	buf, err := core.MarshalJSON(NewAppSmsThirdLoginBody("19900000000", "429344", "hOjseB"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	text := string(buf)
	for _, want := range []string{
		`"ifOpenAccount":"1"`,
		`"clienttype":"412"`,
		`"pintype":"8"`,
		`"cpid":"55"`,
		`"version":"12.6.0"`,
		`"loginMode":"0"`,
		`"verType":"1"`,
	} {
		if !strings.Contains(text, want) {
			t.Errorf("App 端登录体应含 %s，实得 %s", want, text)
		}
	}
	if strings.Contains(text, `"clientType":"412"`) {
		t.Error("thirdlogin 是 **clienttype**（全小写 t），与 getSmsCode 的 clientType 不同")
	}
	// 与 PC 端的对照：同一处字段在 PC 端是数字
	pcBuf, _ := core.MarshalJSON(NewThirdLoginBody(PlatformWindows, LoginSms, "123456", "19900000000"))
	if !strings.Contains(string(pcBuf), `"ifOpenAccount":1`) {
		t.Errorf("PC 端的 ifOpenAccount 是数字 1，实得 %s", pcBuf)
	}
}

// TestAppBodiesDeclareFieldsInJSONKeyOrder：进请求体的结构体必须**按 JSON 键字典序**
// 声明字段 —— Rust 侧 `body.to_string()` 走 `serde_json` 的 Map（BTreeMap，按键排序），
// 而 Go 的 struct 按声明顺序输出。顺手照抄 `json!` 的书写顺序，两边就会发出
// **字节不同**的请求体，而服务端不校验键序 ⇒ 不报错、只是对拍再也比不了整串。
//
// ⚠ 反向测试已确认：把 `AppThirdLoginBody` 按抓包字面量顺序（cpid 打头）重排后，
// 本条变红。而 `len == 186` 那条**不会**变红（两种顺序长度相同）—— 这正是坑的形状。
func TestAppBodiesDeclareFieldsInJSONKeyOrder(t *testing.T) {
	cases := []struct {
		name  string
		build func() ([]byte, error)
	}{
		{"thirdlogin", func() ([]byte, error) {
			return core.MarshalJSON(NewAppSmsThirdLoginBody("19900000000", "429344", "hOjseB"))
		}},
		{"getSmsCode", func() ([]byte, error) {
			return core.MarshalJSON(NewAppSendSmsCodeBody("19900000000", ""))
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			buf, err := c.build()
			if err != nil {
				t.Fatalf("序列化失败: %v", err)
			}
			value, ok := core.ParseJSON(string(buf))
			if !ok {
				t.Fatalf("不是合法 JSON: %s", buf)
			}
			obj, _ := value.(map[string]any)
			// 用"重新序列化一遍 map"得到字典序版本，逐字节比
			sorted, err := core.MarshalJSON(obj)
			if err != nil {
				t.Fatalf("重新序列化失败: %v", err)
			}
			if string(buf) != string(sorted) {
				t.Errorf("字段应按 JSON 键字典序声明（与 serde_json 的 Map 输出对齐）\n实得   %s\n字典序 %s",
					buf, sorted)
			}
		})
	}
}

// TestAppSliderBodyReusesThePCEncoding：滑块答案的编码与 PC 端**完全一样**
// （同一个第一方滑块），只是 `clientType` 与头换成 App 的。
func TestAppSliderBodyReusesThePCEncoding(t *testing.T) {
	body, perr := NewAppSendSmsCodeBodyWithSlider("19900000000", "466")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if body.PuzzleVerfycode == "" {
		t.Fatal("带滑块时 puzzleVerfycode 不应为空")
	}
	// 用公开常量反解回来必须得到原坐标 —— 这同时钉住了 IV 是**固定**那个
	plain, err := core.AESDynamicIVDecrypt(PuzzleAESKey, body.PuzzleVerfycode)
	if err != nil {
		t.Fatalf("反解失败: %v", err)
	}
	if plain != "466" {
		t.Errorf("反解应得到坐标 466，实得 %q", plain)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求构造与头
// ─────────────────────────────────────────────────────────────────────────────

// TestAppRequestsNeverSendHcyCoolFlag：抓包里 App 是发 `hcy-cool-flag: 1` 的，
// 但只要它**出现**，服务端就把响应换成我们解不开的加密体 ⇒ 主动不发。
//
// ⚠ 反向测试已确认：一旦有人"照抄抓包"把它补回去，本条立刻变红。
func TestAppRequestsNeverSendHcyCoolFlag(t *testing.T) {
	profile := testAppProfile()
	requests := map[string]core.HTTPRequest{}
	if r, perr := AppSendSmsCodeRequest(profile, 1, NewAppSendSmsCodeBody("19900000000", "")); perr == nil {
		requests["getSmsCode"] = r
	}
	if r, perr := AppSliderPuzzleRequest(profile, 1, "19900000000"); perr == nil {
		requests["verfycode"] = r
	}
	if r, perr := AppThirdLoginRequest(profile, 1,
		NewAppLoginCryptoProduction(), NewAppSmsThirdLoginBody("19900000000", "429344", "hOjseB")); perr == nil {
		requests["thirdlogin"] = r
	}
	if len(requests) != 3 {
		t.Fatalf("三条请求都应构造成功，实得 %d", len(requests))
	}
	for name, request := range requests {
		if countHeader(request.Headers, "hcy-cool-flag") != 0 {
			t.Errorf("%s **不发** hcy-cool-flag —— 它一出现响应就变成解不开的密文", name)
		}
	}
}

// TestAppThirdLoginSendsTheHexCiphertextButKeepsJSONContentType：
// 内容是 hex 文本，但 Content-Type 仍是 `application/json;charset=UTF-8`
// —— 抓包原样如此，别"顺手改成 text/plain"。
func TestAppThirdLoginSendsTheHexCiphertextButKeepsJSONContentType(t *testing.T) {
	body := NewAppSmsThirdLoginBody("19900000000", "429344", "hOjseB")
	request, perr := AppThirdLoginRequest(testAppProfile(), 1,
		NewAppLoginCryptoProduction(), body)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	// 线上字节 = EncryptBody(实际发出的明文)。
	// ⚠ **不是** appLoginWire —— 那个串对应的是抓包字面量顺序的明文，
	// 而本包的 body 按**字典序**声明字段（与 serde_json 的 Map 输出对齐）。
	plaintext, err := core.MarshalJSON(body)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	want, perr := NewAppLoginCryptoProduction().EncryptBody(string(plaintext))
	if perr != nil {
		t.Fatalf("加密失败: %v", perr)
	}
	if got := string(request.Body); got != want {
		t.Errorf("请求体应是错位后的 hex 密文，实得 %s", got)
	}
	if !strings.Contains(request.URL, "/user/thirdlogin") {
		t.Errorf("应打 /user/thirdlogin，实得 %q", request.URL)
	}
	if got := headerValue(request.Headers, "Content-Type"); got != "application/json;charset=UTF-8" {
		t.Errorf("Content-Type 应带 charset 的 JSON，实得 %q", got)
	}
	if countHeader(request.Headers, "content-type") != 1 {
		t.Errorf("Content-Type 应恰好一个，实得 %d",
			countHeader(request.Headers, "content-type"))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 滑块 H5
// ─────────────────────────────────────────────────────────────────────────────

// TestCaptchaH5DataCarriesOnlyTwoHeaders：H5 的 `data.headers` 里
// **只有两个头**（`x-yun-app-channel` 与 `x-yun-client-info`），
// 不是 [AppProfile.Headers] 的全集 —— 别顺手把整个头集合塞进去。
//
// 并且那个 `x-yun-client-info` 末尾**没有** `|0|;`（请求头里才有）。
func TestCaptchaH5DataCarriesOnlyTwoHeaders(t *testing.T) {
	profile := testAppProfile()
	url := CaptchaH5URL(profile, "19900000000")
	if !strings.HasPrefix(url, CaptchaH5Base+"?data=") {
		t.Fatalf("应是本地拼装的 URL，实得 %q", url)
	}
	encoded := strings.TrimPrefix(url, CaptchaH5Base+"?data=")
	raw, err := core.B64Decode(encoded)
	if err != nil {
		t.Fatalf("data 应是 base64(JSON)，解码失败: %v", err)
	}
	value, ok := core.ParseJSON(string(raw))
	if !ok {
		t.Fatalf("data 不是合法 JSON: %s", raw)
	}
	obj, _ := value.(map[string]any)
	headers, _ := obj["headers"].(map[string]any)
	if len(headers) != 2 {
		t.Errorf("data.headers 应恰好两个头，实得 %d：%v", len(headers), headers)
	}
	if _, ok := headers["x-yun-app-channel"]; !ok {
		t.Error("应含 x-yun-app-channel")
	}
	info, _ := headers["x-yun-client-info"].(string)
	if info == "" {
		t.Fatal("应含 x-yun-client-info")
	}
	if strings.HasSuffix(info, "0|;") {
		t.Error("H5 的 data 里那个 client-info **没有**末尾的 `0|;`（请求头里才有）")
	}
	// 而请求头里那个**有** —— 差异很小但真实存在，两个方向都要钉住。
	if !strings.HasSuffix(profile.Device().ClientInfo(), "0|;") {
		t.Error("请求头里的 client-info 应以 `0|;` 结尾")
	}
	params, _ := obj["params"].(map[string]any)
	if params["phoneNumber"] != "19900000000" {
		t.Errorf("params 应带手机号，实得 %v", params["phoneNumber"])
	}
	if params["nationCode"] != "+86" {
		t.Errorf("nationCode 应为 +86，实得 %v", params["nationCode"])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应语义
// ─────────────────────────────────────────────────────────────────────────────

// TestClassifyAppSendSmsThreeWays：`200059554` 是**常态**不是错误，
// `200059553` 是"这道题作废了"不是"账号被限流"。
//
// ⚠ 反向测试已确认：把 `CodeSliderExhausted` 那档删掉（落进 Other）之后本条变红 ——
// 而那种退化不会有任何报错，只会让人以为"被限流了，等一会儿"。
func TestClassifyAppSendSmsThreeWays(t *testing.T) {
	cases := []struct {
		name string
		raw  map[string]any
		want AppSendSmsKind
	}{
		{"已发出", map[string]any{"code": 0, "msg": "success",
			"data": map[string]any{"remainGetTimes": 8}}, AppSmsSent},
		{"需要先过滑块", map[string]any{"code": 200059554,
			"msg":  "请先获取滑块拼图验证码",
			"data": map[string]any{"captchaType": 2}}, AppSmsNeedCaptcha},
		{"这道题作废了", map[string]any{"code": 200059553,
			"msg":  "同一滑动拼图验证码在单位时间内校验失败次数超限",
			"data": map[string]any{"captchaType": 2}}, AppSmsSliderExhausted},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := ClassifyAppSendSms(envelopeFrom(t, c.raw))
			if got.Kind != c.want {
				t.Fatalf("应判为 %v，实得 %v（message=%q）", c.want, got.Kind, got.Message)
			}
		})
	}
	// 配额余量只在"已发出"那档有意义
	sent := ClassifyAppSendSms(envelopeFrom(t, map[string]any{"code": 0,
		"data": map[string]any{"remainGetTimes": 8}}))
	if sent.RemainGetTimes == nil || *sent.RemainGetTimes != 8 {
		t.Errorf("remainGetTimes 应为 8，实得 %v", sent.RemainGetTimes)
	}
}

// TestClassifyAppLoginFourWays：`9441` 说明加密那套**全对了**只是码错，
// `9103` 说明服务端解不开（体没加密 / 密钥错 / 错位反了），`9999` 是设备头缺失。
func TestClassifyAppLoginFourWays(t *testing.T) {
	cases := []struct {
		name string
		raw  map[string]any
		want AppLoginKind
	}{
		{"成功", map[string]any{"code": 0, "msg": "success", "data": "CIPHER"}, AppLoginSuccess},
		{"验证码不对", map[string]any{"code": 9441, "msg": "验证码不正确"}, AppLoginSmsCodeWrong},
		{"体解不开", map[string]any{"code": 9103, "msg": "输入参数不合法"}, AppLoginUndecryptable},
		{"设备头缺失", map[string]any{"code": 9999, "msg": "登录失败！"}, AppLoginFailed},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got := ClassifyAppLogin(envelopeFrom(t, c.raw))
			if got.Kind != c.want {
				t.Fatalf("应判为 %v，实得 %v", c.want, got.Kind)
			}
		})
	}
}

// TestClassifyAppLoginRejectsANonStringSuccessPayload：成功码但 `data` 不是密文字符串
// 时**如实报告形状不对**，不伪造空凭据 —— 伪造出来的空凭据会让"登录成功"
// 后面跟着一串 nil。
func TestClassifyAppLoginRejectsANonStringSuccessPayload(t *testing.T) {
	got := ClassifyAppLogin(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "data": map[string]any{"a": 1},
	}))
	if got.Kind != AppLoginOther {
		t.Fatalf("应判为 Other，实得 %v", got.Kind)
	}
	if !strings.Contains(got.Message, "不是密文字符串") {
		t.Errorf("报错应说清形状不对，实得 %q", got.Message)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// TestAppThirdLoginEncryptsBeforeSending：发送层拿明文进、密文出 ——
// "忘了加密"在类型层面就不可能发生。
func TestAppThirdLoginEncryptsBeforeSending(t *testing.T) {
	up, client := respondWith(t, map[string]any{"code": 9441, "msg": "验证码不正确"})
	body := NewAppSmsThirdLoginBody("19900000000", "429344", "hOjseB")
	envelope, perr := up.AppThirdLogin(context.Background(),
		testAppProfile(), 1789800128813000,
		NewAppLoginCryptoProduction(), body)
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}
	if got := ClassifyAppLogin(envelope); got.Kind != AppLoginSmsCodeWrong {
		t.Errorf("应判为验证码不对，实得 %v", got.Kind)
	}
	// 发出去的**不可能是明文** —— 这是"忘了加密"唯一的兜底断言
	if strings.Contains(string(client.requests[0].Body), "19900000000") {
		t.Error("发出去的应是密文；能明文搜到手机号说明这一步没加密")
	}
	if !isAllHex(string(client.requests[0].Body)) {
		t.Errorf("发出去的应是 hex，实得 %s", client.requests[0].Body)
	}
}

// isAllHex 判一段文本是不是纯 hex —— 密文与明文 JSON 的分界线。
func isAllHex(text string) bool {
	for _, ch := range text {
		if !strings.ContainsRune("0123456789abcdef", ch) {
			return false
		}
	}
	return len(text) > 0
}

// TestAppSendSmsCodeSendsPlaintext：这一条**不加密**（与 thirdlogin 相反）。
func TestAppSendSmsCodeSendsPlaintext(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 200059554, "msg": "请先获取滑块拼图验证码",
		"data": map[string]any{"captchaType": 2},
	})
	envelope, perr := up.AppSendSmsCode(context.Background(),
		testAppProfile(), 1789800128813000, NewAppSendSmsCodeBody("19900000000", ""))
	if perr != nil {
		t.Fatalf("发码失败: %v", perr)
	}
	if got := ClassifyAppSendSms(envelope); got.Kind != AppSmsNeedCaptcha {
		t.Errorf("应判为需要滑块，实得 %v", got.Kind)
	}
	body := string(client.requests[0].Body)
	if !strings.Contains(body, `"puzzleVerfycode":""`) {
		t.Errorf("首次发码应是明文 JSON 且 puzzleVerfycode 为空串，实得 %s", body)
	}
}

// TestAppFetchSliderPuzzleUsesTheSameHostAsSendSms：取题与提交必须在**同一台主机**
// 上（[BaseUserYun]），否则服务端会把两者当成不同会话。
func TestAppFetchSliderPuzzleUsesTheSameHostAsSendSms(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success",
		"data": map[string]any{
			"picture":     "BG",
			"puzzle":      "PZ",
			"puzzleLeft":  "LEFT",
			"picWidth":    300,
			"puzzleWidth": 40,
		},
	})
	_, perr := up.AppFetchSliderPuzzle(context.Background(), testAppProfile(), 1, "19900000000")
	if perr != nil {
		t.Fatalf("取题失败: %v", perr)
	}
	if !strings.HasPrefix(client.requests[0].URL, BaseUserYun) {
		t.Errorf("取题应与发码同主机（%s），实得 %q", BaseUserYun, client.requests[0].URL)
	}
	if !strings.HasSuffix(client.requests[0].URL, "/user/verfycode") {
		t.Errorf("路径是 verfycode（没有 i），实得 %q", client.requests[0].URL)
	}
}
