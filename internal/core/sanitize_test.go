package core

import (
	"encoding/json"
	"strings"
	"testing"
)

// 顺序占位值（已在 scripts/check-identifiers.cjs 的 ALLOWED 表登记）：
// 它们与真实值同为 11 / 19 位且以 1 开头，因此能真正走到重映射分支。
const (
	fakePhone     = "12345678901"
	fakeDomainID  = "1000000000000000001"
	fakeDomainID2 = "1234567890123456789"
)

// 上面三个值的重映射结果，**独立复算**得到（shell 里用 shasum -a 256 取十六进制、
// 逐字符 mod 10），不是从被测实现里打印出来再贴回去的。
// 这条区别很重要：从实现反推的期望值会让「实现改坏了」与「期望值跟着改」同时发生。
const (
	wantPhoneRemap    = "19925400248"
	wantDomainIDRemap = "1998404849973370133"
	wantDomainID2     = "1992208500929123705"
)

func mustParseJSON(t *testing.T, text string) any {
	t.Helper()
	value, ok := parseJSON(text)
	if !ok {
		t.Fatalf("测试自身的 JSON 写错了: %s", text)
	}
	return value
}

func mustMarshal(t *testing.T, value any) string {
	t.Helper()
	out, err := marshalJSON(value)
	if err != nil {
		t.Fatalf("marshalJSON: %v", err)
	}
	return string(out)
}

// dig 从 map[string]any 里按路径取值。名字不能叫 obj —— crypto_test.go 里已有一个同名的强制转换辅助。
func dig(t *testing.T, root any, path ...string) any {
	t.Helper()
	cur := root
	for _, key := range path {
		m, ok := cur.(map[string]any)
		if !ok {
			t.Fatalf("在 %q 处不是对象，实际 %T", key, cur)
		}
		cur, ok = m[key]
		if !ok {
			t.Fatalf("键 %q 不存在", key)
		}
	}
	return cur
}

// at 从 []any 里取下标。
func at(t *testing.T, root any, index int) any {
	t.Helper()
	items, ok := root.([]any)
	if !ok {
		t.Fatalf("不是数组，实际 %T", root)
	}
	if index >= len(items) {
		t.Fatalf("下标 %d 越界（长度 %d）", index, len(items))
	}
	return items[index]
}

// ─────────────────────────────────────────────────────────────────────────────
// fields —— 对应 Rust sanitize/tests/fields.rs
// ─────────────────────────────────────────────────────────────────────────────

func TestSanitizeSensitiveFieldMatchingIsCaseAndSeparatorAgnostic(t *testing.T) {
	for _, key := range []string{
		"auth", "AUTH", "Auth",
		"userDomainId", "user_domain_id",
		"accessToken", "access-token", "AccessToken",
	} {
		if !IsSensitiveField(key) {
			t.Errorf("%s 应被判定为敏感字段", key)
		}
	}
	for _, key := range []string{"prizeId", "count", "date", "code", "nickname"} {
		if IsSensitiveField(key) {
			t.Errorf("%s 不应被判定为敏感字段", key)
		}
	}
}

func TestSanitizeHeadersGetRedactedButHarmlessOnesSurvive(t *testing.T) {
	headers := [][2]string{
		{"auth", "eyJhbGciOiJIUzI1NiJ9.SECRET"},
		{"Cookie", "sid=abc123"},
		{"user-agent", "mcloud/12.6.0"},
		{"content-type", "application/json"},
	}
	out := SanitizeHeaders(headers)
	if out[0][1] != Placeholder {
		t.Errorf("auth 应当被打码，实际 %q", out[0][1])
	}
	if out[1][1] != Placeholder {
		t.Errorf("Cookie 大小写不敏感，应当被打码，实际 %q", out[1][1])
	}
	if out[2][1] != "mcloud/12.6.0" {
		t.Errorf("普通 header 应保留，实际 %q", out[2][1])
	}
	if out[3][1] != "application/json" {
		t.Errorf("content-type 应保留，实际 %q", out[3][1])
	}
}

func TestSanitizeNestedJSONIsRedactedIncludingArrays(t *testing.T) {
	value := mustParseJSON(t, `{
		"data": {
			"auth": "SECRET_AUTH",
			"profile": { "phone": "19900000001", "nickname": "账号2" },
			"accounts": [
				{ "id": "a1", "accessToken": "SECRET_TOKEN" },
				{ "id": "a2", "jwt": "SECRET_JWT" }
			]
		},
		"code": 0
	}`)

	if _, changed := SanitizeJSON(value); !changed {
		t.Fatal("这份 JSON 里有敏感字段，应当报告发生了修改")
	}

	data := dig(t, value, "data")
	if got := dig(t, data, "auth"); got != Placeholder {
		t.Errorf("data.auth = %v，want %s", got, Placeholder)
	}
	profile := dig(t, data, "profile")
	if got := dig(t, profile, "phone"); got != Placeholder {
		t.Errorf("data.profile.phone = %v，want %s", got, Placeholder)
	}
	if got := dig(t, profile, "nickname"); got != "账号2" {
		t.Errorf("昵称不该被改，实际 %v", got)
	}
	accounts := dig(t, data, "accounts")
	if got := dig(t, at(t, accounts, 0), "accessToken"); got != Placeholder {
		t.Errorf("accounts[0].accessToken = %v，want %s", got, Placeholder)
	}
	if got := dig(t, at(t, accounts, 1), "jwt"); got != Placeholder {
		t.Errorf("accounts[1].jwt = %v，want %s", got, Placeholder)
	}
	// 业务码不应被改（它是 json.Number，脱敏只碰字符串与标识形状的数字）
	if got := dig(t, value, "code").(json.Number).String(); got != "0" {
		t.Errorf("code 不该被改，实际 %s", got)
	}

	if leaks := ScanForLeaks(mustMarshal(t, value)); len(leaks) != 0 {
		t.Errorf("脱敏后仍有残留: %v", leaks)
	}
}

func TestSanitizeNonJSONBodyGetsTokenRunsRedacted(t *testing.T) {
	const body = "session=abcdefghijklmnopqrstuvwxyz0123456789ABCDEF&x=1"
	out := string(SanitizeBody([]byte(body)))
	if !strings.Contains(out, Placeholder) {
		t.Errorf("长 token 串应被遮蔽: %s", out)
	}
	if !strings.Contains(out, "&x=1") {
		t.Errorf("短参数应保留: %s", out)
	}
}

func TestSanitizeBinaryBodyIsNotStoredVerbatim(t *testing.T) {
	out := string(SanitizeBody([]byte{0x00, 0xff, 0xfe, 0x01, 0x02}))
	if out != "<BINARY 5 bytes>" {
		t.Errorf("二进制体应整体打码，实际 %q", out)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// identifiers —— 对应 Rust sanitize/tests/identifiers.rs
// ─────────────────────────────────────────────────────────────────────────────

// 标识形状的数字必须被改成合成号段 —— 判据与仓库守卫逐条对齐。
func TestSanitizeIdentifierShapedNumbersAreRemappedToSyntheticRange(t *testing.T) {
	// 11 位：手机号形状。改写后长度与 199 前缀都要保持
	phoneInput := "phone=" + fakePhone
	phone := SynthesizeIdentifiers(phoneInput)
	if len(phone) != len(phoneInput) {
		t.Errorf("长度应当保持: %d vs %d", len(phone), len(phoneInput))
	}
	if phone != "phone="+wantPhoneRemap {
		t.Errorf("phone 重映射 = %q，want %q", phone, "phone="+wantPhoneRemap)
	}

	// 19 位：userDomainId 形状。实测 taskId 也是这个形状，
	// 所以「反正不是标识」这条退路并不存在
	taskIDInput := `"taskId":"` + fakeDomainID + `"`
	remapped := SynthesizeIdentifiers(taskIDInput)
	if len(remapped) != len(taskIDInput) {
		t.Errorf("长度要保持，否则解析会变形: %d vs %d", len(remapped), len(taskIDInput))
	}
	if strings.Contains(remapped, fakeDomainID) {
		t.Errorf("原值不该残留: %s", remapped)
	}
	if remapped != `"taskId":"`+wantDomainIDRemap+`"` {
		t.Errorf("重映射 = %q，want %q", remapped, `"taskId":"`+wantDomainIDRemap+`"`)
	}

	// 确定性：同输入同输出，否则夹具每次重新生成 diff 全是噪声
	if SynthesizeIdentifiers(fakeDomainID) != SynthesizeIdentifiers(fakeDomainID) {
		t.Error("同输入必须同输出")
	}
	if SynthesizeIdentifiers(fakeDomainID) == SynthesizeIdentifiers(fakeDomainID2) {
		t.Error("不同原值应映射到不同结果")
	}
	if got := SynthesizeIdentifiers(fakeDomainID2); got != wantDomainID2 {
		t.Errorf("第二个 19 位值的重映射 = %q，want %q", got, wantDomainID2)
	}

	// 形状不符的一律不动：位数不对、或两侧挨着字母数字
	for _, untouched := range []string{
		"1699999999",
		"1234567890",
		"a12345678901",
		"12345678901a",
		"123",
	} {
		if got := SynthesizeIdentifiers(untouched); got != untouched {
			t.Errorf("%s 不该被改，实际 %s", untouched, got)
		}
	}
}

// 标识重映射必须覆盖所有文本入口，而不是只在某一条分支里生效。
//
// 这一路踩到的坑几乎都是「某条分支只做了一半」（URL 分支、非 JSON 分支、
// header 分支、裸数字…），所以这里逐个入口点名。
func TestSanitizeIdentifierRemapReachesEveryTextPath(t *testing.T) {
	entry := BuildFixtureEntry(
		"POST",
		// URL query 里的标识
		"https://m.mcloud.139.com/a?taskId="+fakeDomainID,
		// 白名单 header 的值
		[][2]string{{"user-agent", "mcloud " + fakeDomainID}},
		// 非 JSON 文本体
		[]byte("taskId="+fakeDomainID),
		200,
		nil,
		// JSON 体，且写成裸数字
		[]byte(`{"taskId":`+fakeDomainID+`}`),
	)

	text := mustMarshal(t, entry)
	if strings.Contains(text, fakeDomainID) {
		t.Fatalf("标识在某个入口漏了出去: %s", text)
	}
	// 逐个入口点名，免得「整体不含」靠的是某一处顺手遮住了
	if got := dig(t, entry, "request", "url"); !strings.Contains(got.(string), wantDomainIDRemap) {
		t.Errorf("URL query 入口没走到: %v", got)
	}
	if got := at(t, dig(t, entry, "request", "headers"), 0); !strings.Contains(at(t, got, 1).(string), wantDomainIDRemap) {
		t.Errorf("header 入口没走到: %v", got)
	}
	if got := dig(t, entry, "request", "body"); !strings.Contains(got.(string), wantDomainIDRemap) {
		t.Errorf("非 JSON 文本体入口没走到: %v", got)
	}
	// 裸数字入口：重映射后仍然是**数字**，不是被引号包起来的字符串
	raw := dig(t, entry, "response", "body_json", "taskId")
	number, ok := raw.(json.Number)
	if !ok {
		t.Fatalf("裸数字应当仍是 json.Number（掩码会让它失去形状），实际 %T", raw)
	}
	if number.String() != wantDomainIDRemap {
		t.Errorf("裸数字入口没走到: %s，want %s", number.String(), wantDomainIDRemap)
	}
}

func TestSanitizeEndToEndHARLikeEntryIsClean(t *testing.T) {
	reqHeaders := [][2]string{
		{"auth", "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIG"},
		{"Cookie", "jsessionid=ABCDEF123456"},
		{"user-agent", "mcloud/12.6.0"},
	}
	reqBody := []byte(`{"prizeId":123,"phone":"19900000001"}`)
	// 注意：响应体里带中文昵称，用于确认 CJK 不会被误伤。
	respBody := []byte(`{"code":0,"data":{"nickname":"账号2","token":"SECRET_IN_RESPONSE","cloudCount":1286}}`)

	entry := BuildFixtureEntry(
		"POST",
		"https://m.mcloud.139.com/ycloud/prize/exchange",
		reqHeaders,
		reqBody,
		200,
		[][2]string{{"content-type", "application/json"}},
		respBody,
	)

	text := mustMarshal(t, entry)
	if leaks := ScanForLeaks(text); len(leaks) != 0 {
		t.Errorf("脱敏后仍有残留: %v", leaks)
	}

	for _, secret := range []string{
		"PAYLOAD.SIG",
		"ABCDEF123456",
		"SECRET_IN_RESPONSE",
		"19900000001",
	} {
		if strings.Contains(text, secret) {
			t.Errorf("%s 不得出现在 fixture 中", secret)
		}
	}

	// 结构必须保留，否则 fixture 无法用于回放解析
	bodyJSON := dig(t, entry, "response", "body_json")
	if got := dig(t, bodyJSON, "code").(json.Number).String(); got != "0" {
		t.Errorf("code = %s，want 0", got)
	}
	data := dig(t, bodyJSON, "data")
	if got := dig(t, data, "cloudCount").(json.Number).String(); got != "1286" {
		t.Errorf("cloudCount = %s，want 1286", got)
	}
	if got := dig(t, data, "nickname"); got != "账号2" {
		t.Errorf("nickname = %v，want 账号2", got)
	}
	if got := dig(t, entry, "match", "key"); got != "POST m.mcloud.139.com/ycloud/prize/exchange" {
		t.Errorf("match.key = %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// shapes —— 对应 Rust sanitize/tests/shapes.rs
// ─────────────────────────────────────────────────────────────────────────────

// 真实 HAR 上暴露出来的四类形态，逐条钉住。
//
// 这些不是假想用例，是把 36 份真实 HAR 跑一遍之后自检报出来的形状：
// 它们都在「键名黑名单」之外，靠按名判断一条都拦不住。
func TestSanitizeRealWorldShapesAreCleanedEndToEnd(t *testing.T) {
	// ① 内嵌 JSON 里的 userdomain_id（下划线形态，靠归一化命中黑名单）
	reqBody := []byte(`{"header":{"custom":{"userdomain_id":"1990000000000000001"}}}`)
	entry := BuildFixtureEntry(
		"POST",
		"https://data.example.com:8080/newapp/action",
		[][2]string{
			// ② 名字不在白名单里的 cookie 类头：实测最常见的漏点
			{"cookieToken", "eyJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATURE"},
			{"hecaiyundata2021jssdkcross", "YZsidssolgbmPq1234567890abcdefghij"},
			{"user-agent", "mcloud/12.6.0"},
		},
		reqBody,
		200,
		[][2]string{{"content-type", "application/json"}},
		// ③ 无害键名 + 长 base64 值（实测形态：base64 的 mobile:手机号:token）
		// ④ 响应里的 CDN 长路径段
		[]byte(`{"data":{"blob":"bW9iaWxlOjE5OTAwMDAwMDAxOmFiY2RlZmdoaWprbG1ub3A=","records":[{"contentImageUrl":"https://cdn3.cmread.com/a/b/cccccccccccccccccccccccccccccccccccccccccccccccc.png"}]}}`),
	)

	text := mustMarshal(t, entry)
	if leaks := ScanForLeaks(text); len(leaks) != 0 {
		t.Errorf("仍有残留: %v", leaks)
	}
	for _, secret := range []string{"1990000000000000001", "YZsidssolgbmPq", "PAYLOAD.SIGNATURE"} {
		if strings.Contains(text, secret) {
			t.Errorf("%s 不得出现在 fixture 中", secret)
		}
	}
	// 白名单内的 header 与业务结构必须留下，否则夹具没法用
	if !strings.Contains(text, "mcloud/12.6.0") {
		t.Error("user-agent 应保留")
	}
	if !strings.Contains(text, "contentImageUrl") {
		t.Error("业务键名应保留")
	}
	if got := dig(t, entry, "match", "key"); got != "POST data.example.com:8080/newapp/action" {
		t.Errorf("match.key = %v", got)
	}
}

// URL 形状的值：路径留下，但里面没名分的长串仍要打码。
//
// 这条钉的是「按 key 判断不够」——实测有接口把整段 base64 塞进 query 参数 e，
// 参数名毫无提示性。
func TestSanitizeURLValuesKeepPathButLoseUnnamedPayloads(t *testing.T) {
	url := "https://data.example.com/newapp/action?e=" + strings.Repeat("a", 300) +
		"&m=1&clientVersion=12.6.0"
	entry := BuildFixtureEntry("POST", url, nil, nil, 200, nil, []byte(`{}`))

	text := mustMarshal(t, entry)
	if leaks := ScanForLeaks(text); len(leaks) != 0 {
		t.Errorf("仍有残留: %v", leaks)
	}
	if !strings.Contains(text, "m=1") {
		t.Error("短参数应保留")
	}
	if !strings.Contains(text, "clientVersion=12.6.0") {
		t.Errorf("业务参数应保留: %s", text)
	}
	if !strings.Contains(text, "newapp/action") {
		t.Error("路径应保留")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// leak —— 对应 Rust sanitize/tests/leak.rs
// ─────────────────────────────────────────────────────────────────────────────

func TestSanitizeLeakScannerActuallyDetectsALeak(t *testing.T) {
	// 这个用例是「检查器自身」的测试：如果扫描器失灵，
	// 后面所有「无泄漏」断言就都成了空话。
	dirty := mustMarshal(t, map[string]any{"auth": "REAL_SECRET_TOKEN"})
	if len(ScanForLeaks(dirty)) == 0 {
		t.Error("扫描器必须能发现未脱敏的敏感字段")
	}

	dirtyText := "here is a very long secret token: AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ"
	if len(ScanForLeaks(dirtyText)) == 0 {
		t.Error("扫描器必须能发现非结构化的长 token")
	}
}

// 扫描器不能把「故意保留的 URL」误报成泄漏 —— 否则门禁永远红，比漏报更有害。
func TestSanitizeLeakScannerIgnoresURLsButKeepsTokens(t *testing.T) {
	// URL 形状：不该报
	for _, clean := range []string{
		"https://m.mcloud.139.com/ycloud/prize/exchange/apply/v3",
		"//cdn.example.com/a/very/long/path/that/definitely/exceeds/40",
		"m.mcloud.139.com/ycloud/prize/exchange/apply/v2",
	} {
		if leaks := ScanForLeaks(clean); len(leaks) != 0 {
			t.Errorf("%s 不应误报，实际: %v", clean, leaks)
		}
	}

	// 真正的 token：必须报
	for _, dirty := range []string{
		"eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.PAYLOAD.SIGNATUREPAD123456",
		"AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ",
	} {
		if leaks := ScanForLeaks(dirty); len(leaks) == 0 {
			t.Errorf("%s 必须被识别为泄漏", dirty)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// url —— 对应 Rust sanitize/tests/url.rs
// ─────────────────────────────────────────────────────────────────────────────

func TestSanitizeMatchKeyDropsQueryEntirely(t *testing.T) {
	got := MatchKey("post",
		"https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0&_t=1699999999")
	if want := "POST m.mcloud.139.com/ycloud/signin/page/exchangeList"; got != want {
		t.Errorf("match_key = %q，want %q", got, want)
	}
	if got, want := MatchKey("GET", "https://a.example.com/x"), "GET a.example.com/x"; got != want {
		t.Errorf("match_key = %q，want %q", got, want)
	}
}

func TestSanitizeRedactURLStripsVolatileAndMasksSensitive(t *testing.T) {
	url := "https://a.example.com/api?clientVersion=12.6.0&_t=1699999999&nonce=abc&auth=SECRET"
	out := RedactURL(url)
	if !strings.Contains(out, "clientVersion=12.6.0") {
		t.Errorf("业务参数保留: %s", out)
	}
	if strings.Contains(out, "_t=") {
		t.Errorf("时间戳应剔除: %s", out)
	}
	if strings.Contains(out, "nonce=") {
		t.Errorf("随机数应剔除: %s", out)
	}
	if strings.Contains(out, "SECRET") {
		t.Errorf("凭据不得残留: %s", out)
	}
	if !strings.Contains(out, "auth="+Placeholder) {
		t.Errorf("auth 应被打码: %s", out)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Go 侧新增：三条 Rust 原测试覆盖不到、换语言后才长出来的风险
// ─────────────────────────────────────────────────────────────────────────────

// ⭐ HTML 转义：Go 的 json.Marshal 默认把 `<` `>` `&` 转义成 `\u003c` 等，serde_json 不转义。
//
// 这条错误的**隐蔽性**在于：夹具解析回来完全正确，回放也不受影响 ——
// 只有「人眼读夹具」与「按文本扫描的守卫」会看到 `\u003cREDACTED\u003e`。
// 而仓库守卫扫的正是文本，`scan_for_leaks` 扫的也是文本。
func TestSanitizeOutputDoesNotEscapeHTML(t *testing.T) {
	headers := [][2]string{{"auth", "SECRET"}}
	value := map[string]any{"headers": headersToAny(SanitizeHeaders(headers))}
	text := mustMarshal(t, value)

	if !strings.Contains(text, Placeholder) {
		t.Errorf("占位符应以原文出现，实际 %s", text)
	}
	if strings.Contains(text, `\u003c`) || strings.Contains(text, `\u003e`) {
		t.Errorf("不得转义尖括号（json.Marshal 的默认行为）: %s", text)
	}
}

// ⭐ 数字精度：Go 默认把 JSON 数字解成 float64，19 位标识会静默丢精度。
//
// 丢精度这件事的隐蔽性在于：它**不报错**，只是末几位归零，
// 于是重映射出来的合成值也跟着变 —— 而它同样「看起来是随机的」。
// 判据只能是「重映射结果必须等于由**原始字面量**独立复算出的那个值」。
func TestSanitizeNineteenDigitNumbersKeepFullPrecision(t *testing.T) {
	value := mustParseJSON(t, `{"taskId":`+fakeDomainID+`}`)

	raw := dig(t, value, "taskId")
	number, ok := raw.(json.Number)
	if !ok {
		t.Fatalf("必须解析成 json.Number（float64 会丢精度），实际 %T", raw)
	}
	if number.String() != fakeDomainID {
		t.Fatalf("字面量必须原样保留，实际 %s", number.String())
	}

	sanitized, changed := SanitizeJSON(value)
	if !changed {
		t.Fatal("标识形状的裸数字应当被重映射")
	}
	got := dig(t, sanitized, "taskId").(json.Number).String()
	if got != wantDomainIDRemap {
		t.Fatalf("重映射 = %s，want %s（float64 路径会得到另一个值）", got, wantDomainIDRemap)
	}
}

// ⭐ Encoder 的尾换行：json.Encoder.Encode 会补一个 `\n`，serde_json::to_vec 不会。
//
// 尾换行看起来无害，但它会让 fixture 文件每次生成的 diff 都多一行，
// 更麻烦的是 `scan_for_leaks` 收到的文本与 Rust 侧不是同一份。
func TestSanitizeMarshalHasNoTrailingNewline(t *testing.T) {
	out, err := marshalJSON(map[string]any{"a": 1})
	if err != nil {
		t.Fatalf("marshalJSON: %v", err)
	}
	if strings.HasSuffix(string(out), "\n") {
		t.Fatalf("不应有尾换行: %q", out)
	}
	if string(out) != `{"a":1}` {
		t.Fatalf("紧凑输出应当是 %s，实际 %s", `{"a":1}`, out)
	}
}

// SanitizeJSON 的根是字符串时改不了原值（Go 的 any 装的是值），只能靠返回值传出来。
//
// Rust 那边是 `&mut Value`，所以照搬签名会踩空：调用方不接返回值，
// 脱敏就**静默什么都没做**。
func TestSanitizeJSONReturnsTheNewValueWhenTheRootIsAString(t *testing.T) {
	const secret = "session=abcdefghijklmnopqrstuvwxyz0123456789ABCDEF"
	got, changed := SanitizeJSON(secret)
	if !changed {
		t.Fatal("根是长 token 串时应当报告发生了修改")
	}
	if got == secret {
		t.Fatal("必须通过返回值把新值传出来")
	}
	if !strings.Contains(got.(string), Placeholder) {
		t.Fatalf("应当被打码，实际 %v", got)
	}
}

// 科学计数法字面量：Go 侧按字面量原样保留，Rust 侧经 f64 往返会变成 `100.0`。
//
// 这条是**刻意保留的差异**，不是遗漏（见 sanitize.go 包注释）：
// 保留字面量更准（Rust 那条路会在 20 位以上整数上丢精度），
// 两侧解析出来的**值**相同，且本模块的输出没有任何门禁做逐字节对拍。
// 写在这里是为了让下一个人改到这里时知道它是想过的。
func TestSanitizeNumberLiteralsArePreservedNotRoundTripped(t *testing.T) {
	value := mustParseJSON(t, `{"ratio":1e2,"plain":1.5,"big":1000000000000000001}`)
	text := mustMarshal(t, value)
	if !strings.Contains(text, `"ratio":1e2`) {
		t.Errorf("科学计数法字面量应原样保留: %s", text)
	}
	if !strings.Contains(text, `"plain":1.5`) {
		t.Errorf("小数字面量应原样保留: %s", text)
	}
	if !strings.Contains(text, `"big":1000000000000000001`) {
		t.Errorf("19 位整数不得经 f64 丢精度: %s", text)
	}
}

// 自检报告里不许出现值本身 —— 它只该给位置、形状分类与长度。
//
// 否则这份「给修复者定位用」的报告会反过来成为一条新的泄漏通道。
func TestSanitizeLeakReportNeverContainsTheValue(t *testing.T) {
	const secret = "AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIIIJJJJ"
	leaks := ScanForLeaks(`{"data":{"blob":"` + secret + `"}}`)
	if len(leaks) == 0 {
		t.Fatal("应当报出残留")
	}
	for _, line := range leaks {
		if strings.Contains(line, secret) {
			t.Errorf("报告里不得出现值本身: %s", line)
		}
		// 但要能定位：必须带 $ 路径与长度
		if !strings.Contains(line, "$.data.blob") {
			t.Errorf("报告必须能定位到具体叶子: %s", line)
		}
		if !strings.Contains(line, "长度 40") {
			t.Errorf("报告必须带长度: %s", line)
		}
	}
}

// 内嵌 JSON 的递归深度有上界（4 层）。
//
// 这条上界不是性能考虑：**纯数字/纯文本串按 JSON 解析也会成功**（`123` 是合法 JSON），
// 若不加限制，一条 40 位纯数字的串会被当成「已解析、无残留」而静默放过。
func TestSanitizeLeakScannerDescendsIntoEmbeddedJSONButStopsAtDepth(t *testing.T) {
	// 一层内嵌：脱敏漏掉的敏感字段必须能被扫出来
	inner := `{"auth":"REAL_SECRET_TOKEN"}`
	outer := mustMarshal(t, map[string]any{"body": inner})
	if leaks := ScanForLeaks(outer); len(leaks) == 0 {
		t.Error("一层内嵌 JSON 里的敏感字段应当被扫出来")
	}

	// 纯数字串不能被当成「合法的内嵌 JSON」而跳过
	const digits = "1000000000000000001" // 19 位，长度不到 40，先确认它本来就不报
	if leaks := ScanForLeaks(`{"taskId":"` + digits + `"}`); len(leaks) != 0 {
		t.Errorf("19 位数字串不足 40 字符，不该报: %v", leaks)
	}
	// 而 40 位以上的纯数字串必须报 —— 它若被当成「已解析、无残留」就会静默放过
	const longDigits = "1234567890123456789012345678901234567890123" // 43 位
	if leaks := ScanForLeaks(`{"taskId":"` + longDigits + `"}`); len(leaks) == 0 {
		t.Error("40 位以上的纯数字串必须被报出来")
	}
}
