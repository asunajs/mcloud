package upstream

// 红包派对（云机）族 · 基座测试：入口常量、鉴权头、签名算式、`requestId` 形状、
// 信封判据，以及**有实拍**的三条口（`user/tokenValidate`、`redpacket/userAccountInfo`、
// `redpacket/userToastInfo`）。
//
// # 这些用例的取材纪律（与夹具一一对应）
//
// 夹具 `test/fixtures/http/红包派对.json` 是本族**唯一**的事实来源，但它的
// 敏感头值全被换成了占位串 ⇒ 头**名**可信、头**值**不可信。所以：
//
//   - 请求体 / 响应体 / URL / 头名 —— 逐字取自夹具（本文件里的字面量就是夹具里那一条）；
//   - token / sign / deviceId / cookie 一类**值** —— 一律用 `SYNTH…199` 这种合成串，
//     断言只对**算式与位置**成立，绝不把任何看起来像凭据的东西写进仓库；
//   - 手机号 —— 夹具的 `data.account` 是掩码形态（前三位 + 星号 + 后四位，带真尾号），
//     本文件按仓库口径换成合成尾号 `199****0001`，形状一致而值不可复用。
//
// # 反向改坏它，哪一条会红
//
//   - 把 [EnsureCloudPhoneOK] 换成 [Envelope.EnsureOK]（去找 `code`）⇒
//     [TestEnsureCloudPhoneOKRefusesToAssumeSuccess] 的"没有 code 那一档"当场变红 ——
//     这一族**根本没有 code 字段**，用通用判据会把 `status:"500"` 判成成功。
//   - 把签名算式的任意两段调换顺序 ⇒
//     [TestCloudPhoneSignUsesTheDocumentedSegmentOrder] 变红（它对拼的是**段序**，
//     不是"某个 md5"）。
//   - 把 `status` 的比较改成数字 200 或 `String()` 化兼容 ⇒
//     同一条用例里"数字 200 那一档"变红。
//   - 把余额缺省折成 0 ⇒ [TestParseCloudPhoneAccountInfoMissingIsNilNotZero] 变红
//     （这一族产出的是钱，"不知道"写成"没有"是给用户看错的余额）。

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具原文（只放本族那十条里用到的部分）
// ─────────────────────────────────────────────────────────────────────────────

// cloudPhoneCapturedUserAgent / Origin / Accept 逐字抄自第 29–38 条的**未脱敏**头。
const (
	cloudPhoneCapturedUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) " +
		"AppleWebKit/605.1.15 (KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"
	cloudPhoneCapturedOrigin = "https://cpactiv.buy.139.com"
	cloudPhoneCapturedAccept = "application/json, text/plain, */*"
)

// 合成素材：形状照实拍（`requestId` 那一串、时间戳那一串），值全部自造。
const (
	cloudPhoneSyntheticRequestId = "202606301200001991234567890abcdefgh"
	cloudPhoneSyntheticTimestamp = "1991234567890"
	cloudPhoneSyntheticToken     = "SYNTH-ACTIVITY-TOKEN-199"
	cloudPhoneSyntheticSSO       = "SYNTH-SSO-TOKEN-199"
	cloudPhoneSyntheticDeviceID  = "SYNTH-DEVICE-199"
	cloudPhoneSyntheticReferer   = "https://cpactiv.buy.139.com/#/redEnvelopeParty/home"
)

// cloudPhoneSyntheticAuth 是测试用的一组鉴权素材。
func cloudPhoneSyntheticAuth() CloudPhoneAuth {
	return CloudPhoneAuth{
		RequestID: cloudPhoneSyntheticRequestId,
		Timestamp: cloudPhoneSyntheticTimestamp,
		Token:     cloudPhoneSyntheticToken,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 入口与常量：逐条钉在夹具原文上
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneEntryConstantsMatchTheCapture ⭐ 入口、Origin、UA、Accept、appId、
// version、渠道串、sourceId、成功位 —— 每一条都能指到某一条样本。
//
// 反向改坏它：把 UA 换成 [LiveUserAgent]（两者今天逐字相同）也会红 ——
// 那不是为了区分两个字符串，是为了让"哪天有人把它们合成一个"这件事可见：
// 同值互搬的表现是**没有任何本地断言会变红**（实现文件头那条注释记的就是这件事）。
func TestCloudPhoneEntryConstantsMatchTheCapture(t *testing.T) {
	if CloudPhoneBaseURL != "https://cpactiv.buy.139.com/cloudphone-market" {
		t.Errorf("入口前缀应逐字为夹具那一条，实得 %q", CloudPhoneBaseURL)
	}
	if CloudPhoneOrigin != cloudPhoneCapturedOrigin {
		t.Errorf("Origin 实得 %q", CloudPhoneOrigin)
	}
	if CloudPhoneUserAgent != cloudPhoneCapturedUserAgent {
		t.Errorf("UA 应与实拍逐字相同，实得 %q", CloudPhoneUserAgent)
	}
	if CloudPhoneAccept != cloudPhoneCapturedAccept {
		t.Errorf("Accept 应与实拍逐字相同（不是 */*），实得 %q", CloudPhoneAccept)
	}
	// 实拍十条的 `header.appId` 全是这个字符串；它同时是签名算式的第二段。
	if CloudPhoneAppID != "12345681" {
		t.Errorf("appId 实得 %q", CloudPhoneAppID)
	}
	// 四条 `redpacket/*` 的 body 里逐字出现。
	if CloudPhoneVersion != "SYS_CONFIG_Y" {
		t.Errorf("version 实得 %q", CloudPhoneVersion)
	}
	// 渠道串的字面值实拍在第 9 条那条**未脱敏**的活动页 URL 上。
	if CloudPhoneChannelSrc != "red-mcloudapp-001" {
		t.Errorf("channelSrc 实得 %q", CloudPhoneChannelSrc)
	}
	if CloudPhoneSpecTokenSourceID != "001216" {
		t.Errorf("本族活动站 sourceId 实得 %q", CloudPhoneSpecTokenSourceID)
	}
	// ⭐ 成功位是**字符串** "200"（夹具十个里逐字带引号）。
	if CloudPhoneSuccessStatus != "200" {
		t.Errorf("header.status 是字符串判据，实得 %q", CloudPhoneSuccessStatus)
	}
	// tokenValidate 那个 `version` 与 `pintype` 是**这一条口独有**的实拍值。
	if CloudPhoneTokenValidateVersion != "1.0" {
		t.Errorf("tokenValidate 的 version 是 1.0（不是 SYS_CONFIG_Y），实得 %q",
			CloudPhoneTokenValidateVersion)
	}
	if CloudPhonePintype != 13 || CloudPhoneLoginConfig != "redpacketLoginConfig" {
		t.Errorf("pintype/loginConfig 实得 %v / %q", CloudPhonePintype, CloudPhoneLoginConfig)
	}
}

// TestCloudPhoneSignSaltOnlyTracesToTheDoc ⭐ 钉住这枚盐的**出处层级**。
//
// 成文说 `SIGN_SALT == md5("123456")`；夹具里算式的四个输入与输出**全被遮**，
// 所以这条断言只证明"这个常量与成文那句仍然一致"，**不**证明上游认它。
//
// 反向改坏它：有人改这串十六进制 ⇒ 本条红，而他看到的是"成文那句对不上了"，
// 不是"上游签名错了"。这两件事必须分开成文，否则下一个人会以为这里验过上游。
func TestCloudPhoneSignSaltOnlyTracesToTheDoc(t *testing.T) {
	if CloudPhoneSignSalt != core.MD5Hex([]byte("123456")) {
		t.Errorf("成文那句是 md5(\"123456\")；常量与之不再一致（**上游侧从未验过**）实得 %q",
			CloudPhoneSignSalt)
	}
	if len(CloudPhoneSignSalt) != 32 {
		t.Errorf("它应是 32 位十六进制 md5 输出，实得长度 %d", len(CloudPhoneSignSalt))
	}
}

// TestCloudPhoneSignUsesTheDocumentedSegmentOrder ⭐ 算式与**段序**。
//
// 全部输入是合成值 ⇒ 断言里没有任何可复用凭据。
//
// 反向改坏它：调换 md5 输入里任意两段 ⇒ 第一条（等于手工拼的那一串）当场红；
// 少拼 APP_ID 或盐 ⇒ 红；把 md5 换成别的摘要 ⇒ 长度那一条红。
func TestCloudPhoneSignUsesTheDocumentedSegmentOrder(t *testing.T) {
	requestID := "20260630120000" + "1991234567890" + "abcdefgh"
	token := "SYNTH-A-TOKEN"
	timestamp := "1991234567890"

	got := CloudPhoneSign(requestID, token, timestamp)
	want := core.MD5Hex([]byte(requestID + CloudPhoneAppID + token + CloudPhoneSignSalt + timestamp))
	if got != want {
		t.Errorf("签名应是 md5(requestId+APP_ID+token+SALT+timestamp)，实得 %q", got)
	}
	if len(got) != 32 || strings.ToLower(got) != got {
		t.Errorf("应是 32 位小写十六进制，实得 %q", got)
	}
	if again := CloudPhoneSign(requestID, token, timestamp); again != got {
		t.Error("同样输入两次不一致")
	}
	// 三个输入各自都进得了摘要（任一段没拼进去 ⇒ 这一档红）。
	for _, caseName := range []string{"requestId", "token", "timestamp"} {
		var changed string
		switch caseName {
		case "requestId":
			changed = CloudPhoneSign(requestID+"x", token, timestamp)
		case "token":
			changed = CloudPhoneSign(requestID, token+"x", timestamp)
		case "timestamp":
			changed = CloudPhoneSign(requestID, token, timestamp+"x")
		}
		if changed == got {
			t.Errorf("%s 的改动没影响签名 ⇒ 那一段没进摘要", caseName)
		}
	}
	// 段序：把首尾两段调换是**另一个** md5，而失败表现只有一句非 200。
	if CloudPhoneSign(timestamp, token, requestID) == got {
		t.Error("签名对段序不敏感 ⇒ 算式被写成了与顺序无关的形式")
	}
	// 方法版与函数版必须同源（各算一遍早晚漂移）。
	if auth := (CloudPhoneAuth{RequestID: requestID, Token: token, Timestamp: timestamp}); auth.Sign() != got {
		t.Error("CloudPhoneAuth.Sign 与 CloudPhoneSign 不一致")
	}
}

// TestCloudPhoneRequestIDShapeFollowsTheDoc 钉 requestId 的拼法（⚠ 形状未实拍，
// 实现文件头列了怎么验）。它不参与任何判据，所以这里只钉"拼出来是什么"。
//
// 反向改坏它：把布局换成带分隔的 RFC3339 ⇒ 长度那一条红；
// 把时区改成 UTC ⇒ 前缀那一条红（用的这个时刻在 UTC 是前一天 16 点）。
func TestCloudPhoneRequestIDShapeFollowsTheDoc(t *testing.T) {
	// 东八区 2026-06-30 00:30:00，同一时刻在 UTC 是 2026-06-29 16:30:00。
	at := time.Date(2026, 6, 29, 16, 30, 0, 0, time.UTC)
	suffix := CloudPhoneRequestIDSuffix(&seqRandomUpstream{})
	if len(suffix) != CloudPhoneRequestIDSuffixLen {
		t.Fatalf("随机尾巴应是 %d 位，实得 %q", CloudPhoneRequestIDSuffixLen, suffix)
	}
	for _, ch := range suffix {
		if !strings.ContainsRune("0123456789abcdefghijklmnopqrstuvwxyz", ch) {
			t.Errorf("尾巴只应含 base36 字符，实得 %q", string(ch))
		}
	}
	id := CloudPhoneRequestID(at, 1991234567890, suffix)
	if !strings.HasPrefix(id, "20260630003000") {
		t.Errorf("前缀应是东八区无分隔的 YYYYMMDDHHmmss，实得 %q", id)
	}
	if strings.ContainsAny(id, "-: TZ") {
		t.Errorf("id 里不该出现任何分隔符（它会进 md5），实得 %q", id)
	}
	if want := "20260630003000" + "1991234567890" + suffix; id != want {
		t.Errorf("三段拼接实得 %q，应为 %q", id, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 鉴权头
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneAuthHeadersMatchCapturedHeaderNames ⭐ 头名逐个对上实拍，
// 并且**没给值就不发**的那两枚（`x-DeviceInfo`、`Cookie`）各钉一次。
// ⚠ `x-origin` 不在这里钉：它现在是 opt-in（值有旧 TS 出处），
// 那三档（都不给 / 都给 / 只给一枚）在 `app_cloud_phone_headers_test.go`。
//
// 反向改坏它：把 `requestId` 写成 `x-request-id`（本仓库另一族的写法）⇒ 红；
// 顺手补一个 Cookie ⇒ 红；`DeviceInfo` 空却发了 `x-DeviceInfo` ⇒ 红。
func TestCloudPhoneAuthHeadersMatchCapturedHeaderNames(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	request, perr := CloudPhoneRequest("redpacket/userToastInfo", nil, auth, cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("本族全是 POST，实得 %s", request.Method)
	}
	if want := CloudPhoneBaseURL + "/redpacket/userToastInfo"; request.URL != want {
		t.Errorf("URL 实得 %q，应为 %q", request.URL, want)
	}
	for _, name := range []string{"User-Agent", "Origin", "Accept", "appId", "x-channelSrc",
		"requestId", "timestamp", "token", "sign"} {
		if n := countHeader(request.Headers, name); n != 1 {
			t.Errorf("头 %s 应恰好一个，实得 %d", name, n)
		}
	}
	if got := headerValue(request.Headers, "requestId"); got != cloudPhoneSyntheticRequestId {
		t.Errorf("requestId 应是素材那一枚，实得 %q", got)
	}
	if got := headerValue(request.Headers, "timestamp"); got != cloudPhoneSyntheticTimestamp {
		t.Errorf("timestamp 应**原样**传（单位未核实 ⇒ 本层不格式化），实得 %q", got)
	}
	if got := headerValue(request.Headers, "sign"); got != auth.Sign() {
		t.Errorf("sign 应是算式的结果，实得 %q", got)
	}
	if got := headerValue(request.Headers, "appId"); got != CloudPhoneAppID {
		t.Errorf("appId 实得 %q", got)
	}
	if got := headerValue(request.Headers, "x-channelSrc"); got != CloudPhoneChannelSrc {
		t.Errorf("空 ChannelSrc 应落回常量，实得 %q", got)
	}
	if got := headerValue(request.Headers, "Referer"); got != cloudPhoneSyntheticReferer {
		t.Errorf("referer 是参数（实拍十条该头全被遮），实得 %q", got)
	}
	// content-type 只能有一个，且是实拍那个带 charset 的写法。
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应恰好一个，实得 %d", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
		t.Errorf("content-type 实得 %q，应为 %q", got, H5ContentType)
	}
	// 这一条用的是**没给值**的默认 auth ⇒ 两枚都不该出现：
	// `x-origin` 是 opt-in（值有旧 TS 出处，见 `app_cloud_phone_headers.go`），
	// `Cookie` 是纯协议实现一律不带。
	for _, name := range []string{"x-origin", "Cookie"} {
		if n := countHeader(request.Headers, name); n != 0 {
			t.Errorf("%s 在没给值/一律不带的情况下被发了，实得 %d", name, n)
		}
	}
	// x-DeviceInfo 只在给了的时候发。
	if n := countHeader(request.Headers, "x-DeviceInfo"); n != 0 {
		t.Errorf("DeviceInfo 为空就不该有这个头，实得 %d", n)
	}
	withDevice, perr := CloudPhoneRawRequest("redpacket/userToastInfo",
		map[string]any{"version": CloudPhoneVersion},
		CloudPhoneAuth{RequestID: cloudPhoneSyntheticRequestId, Timestamp: cloudPhoneSyntheticTimestamp,
			Token: cloudPhoneSyntheticToken, DeviceInfo: "SYNTH-FINGERPRINT-199"}, "")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if got := headerValue(withDevice.Headers, "x-DeviceInfo"); got != "SYNTH-FINGERPRINT-199" {
		t.Errorf("给了就该原样带出，实得 %q", got)
	}
	// referer 不给 ⇒ 不发这个头（不是发一个空值）。
	if n := countHeader(withDevice.Headers, "Referer"); n != 0 {
		t.Errorf("referer 为空串时不该发 Referer 头，实得 %d", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 信封判据
// ─────────────────────────────────────────────────────────────────────────────

// TestEnsureCloudPhoneOKRefusesToAssumeSuccess ⭐ 本层最要紧的一条。
//
// 通用信封判据 [Envelope.EnsureOK] 找的是 `code`，而这一族**没有** `code`：
// "没有 code" 在它那里是通过 ⇒ 上游回 `status:"500"` 也会被当成成功。
// 这一族产出的是钱，所以这一档必须**显式**报错。
//
// 反向改坏它：把 [EnsureCloudPhoneOK] 换成 `envelope.EnsureOK()` ⇒
// "没有 code 只有 header.status" 那一档当场红。
func TestEnsureCloudPhoneOKRefusesToAssumeSuccess(t *testing.T) {
	for name, body := range map[string]string{
		// ⭐ 这一族最常见的形状：没有 code、没有 success，只有 header.status。
		"没有 code 的普通信封":   `{"header":{"appId":"12345681","errMsg":"OK","status":"500"},"data":{"errorMsg":"活动已结束"}}`,
		"没有 header 段":     `{"data":{"anything":1}}`,
		"header 不是对象":     `{"header":"x","data":{}}`,
		"status 缺失":       `{"header":{"appId":"12345681","errMsg":"OK"},"data":{}}`,
		"status 是数字不是字符串": `{"header":{"status":200},"data":{}}`,
		"status 是布尔":      `{"header":{"status":true},"data":{}}`,
		"status 是空串":      `{"header":{"status":""},"data":{}}`,
		"整包是数组":           `[{"header":{"status":"200"}}]`,
	} {
		if _, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t, body)); perr == nil {
			t.Errorf("%s：应报错，绝不能判成成功", name)
		}
	}
	// 成功位：只认字符串 "200"。
	data, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"isShowCloud":1}}`))
	if perr != nil {
		t.Fatalf("成功信封不该报错: %v", perr)
	}
	if got, _ := data["isShowCloud"].(json.Number); got.String() != "1" {
		t.Errorf("data 应原样交出，实得 %#v", data)
	}
	// `data` 缺失或不是对象 ⇒ **不报错**、交 nil（"打上了但没载荷"与"没打上"是两件事）。
	for name, body := range map[string]string{
		"没有 data":     `{"header":{"status":"200"}}`,
		"data 是数组":    `{"header":{"status":"200"},"data":[1,2]}`,
		"data 是 null": `{"header":{"status":"200"},"data":null}`,
	} {
		data, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读判据不该把它打成失败: %v", name, perr)
		}
		if data != nil {
			t.Errorf("%s：应交 nil，实得 %#v", name, data)
		}
	}
}

// TestCloudPhoneFailureTextPriorityMatchesTheDoc ⭐ 文案优先级
// `data.errorMsg → header.errMsg → header.respMsg`（成文 §2.4 第 370 行）。
//
// 反向改坏它：把优先级写反（先 header 再 data）⇒ 第一条红；
// 把"空串算不算给了"改成算 ⇒ 第三条红。
func TestCloudPhoneFailureTextPriorityMatchesTheDoc(t *testing.T) {
	for _, c := range []struct {
		name string
		body string
		want string
	}{
		{"三处都给 ⇒ 取 data.errorMsg",
			`{"header":{"status":"500","errMsg":"OK","respMsg":"兜底"},"data":{"errorMsg":"以数据里那句为准"}}`,
			"以数据里那句为准"},
		{"只给 header.errMsg",
			`{"header":{"status":"500","errMsg":"只有 errMsg"},"data":{}}`, "只有 errMsg"},
		{"data.errorMsg 是空串 ⇒ 继续往下找",
			`{"header":{"status":"500","errMsg":"第二档"},"data":{"errorMsg":""}}`, "第二档"},
		{"只有 respMsg（这一格零样本，留着只为哪天给了别丢）",
			`{"header":{"status":"500","respMsg":"只有 respMsg"},"data":{}}`, "只有 respMsg"},
	} {
		_, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t, c.body))
		if perr == nil {
			t.Errorf("%s：应报错", c.name)
			continue
		}
		if !strings.Contains(perr.Message(), c.want) {
			t.Errorf("%s：文案应带上游那句，实得 %q", c.name, perr.Message())
		}
		if perr.Kind() != core.KindBadRequest {
			t.Errorf("%s：非 200 是业务侧答复，应归 BadRequest，实得 %v", c.name, perr.Kind())
		}
	}
	// 三处全空 ⇒ 不许退化成"成功"，也不许凭空编一句原因。
	_, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"","respMsg":""},"data":{"errorMsg":""}}`))
	if perr == nil {
		t.Fatal("三处文案全空也要报错")
	}
	if !strings.Contains(perr.Message(), "文案位都是空的") {
		t.Errorf("应明说三处都是空的，实得 %q", perr.Message())
	}
	// 判据不过的那两类归 Parse（信封不认识），不与 BadRequest 混。
	for _, body := range []string{`{"data":{}}`, `{"header":{"status":200}}`} {
		if _, perr := EnsureCloudPhoneOK(cloudPhoneEnvelope(t, body)); perr == nil ||
			perr.Kind() != core.KindParse {
			t.Errorf("%s：应报 KindParse，实得 %v", body, perr)
		}
	}
}

// TestParseCloudPhoneHeaderIsReadonly 只读信封的 header 段：**不判定、不报错**。
//
// 分成两个函数是给"只想看上游说了什么"的调用方用的（`AGENTS.md` §1.7）。
func TestParseCloudPhoneHeaderIsReadonly(t *testing.T) {
	head := ParseCloudPhoneHeader(cloudPhoneEnvelope(t,
		`{"header":{"appId":"12345681","errMsg":"OK","requestId":"2071905980258680833","status":"200"},"data":{"ret":1}}`))
	if head.Status == nil || *head.Status != "200" {
		t.Errorf("status 实得 %v", head.Status)
	}
	if head.AppID == nil || *head.AppID != "12345681" {
		t.Errorf("appId 实得 %v", head.AppID)
	}
	if head.ErrMsg == nil || *head.ErrMsg != "OK" {
		t.Errorf("errMsg 实得 %v", head.ErrMsg)
	}
	// 实拍第 31/34 条的 requestId 就是这种 19 位纯数字（**字符串**形态）。
	if head.RequestID == nil || *head.RequestID != "2071905980258680833" {
		t.Errorf("requestId 实得 %v", head.RequestID)
	}
	if head.RespMsg != nil {
		t.Errorf("respMsg 夹具从未出现过 ⇒ 应是 nil，实得 %q", *head.RespMsg)
	}
	// 没有 header 段 ⇒ 全 nil、不报错（判定的活在那一个函数里）。
	empty := ParseCloudPhoneHeader(cloudPhoneEnvelope(t, `{"data":{}}`))
	if empty != (CloudPhoneHeader{}) {
		t.Errorf("应是零值结构，实得 %#v", empty)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 有实拍之一：`user/tokenValidate`（第 29 条）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneTokenValidateBodyMatchesCapture ⭐ 请求体逐字节对第 29 条。
//
// 反向改坏它：把这一条也走统一 body（补上 `version:"SYS_CONFIG_Y"`）⇒ 红；
// 把 pintype 抄成第 1 条那个 `5`（**另一个域**的同一尾段名）⇒ 红。
func TestCloudPhoneTokenValidateBodyMatchesCapture(t *testing.T) {
	request, perr := CloudPhoneTokenValidateRequest(cloudPhoneSyntheticAuth(),
		cloudPhoneSyntheticReferer, cloudPhoneSyntheticDeviceID, cloudPhoneSyntheticSSO)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := CloudPhoneBaseURL + "/user/tokenValidate"; request.URL != want {
		t.Errorf("URL 实得 %q，应为 %q", request.URL, want)
	}
	// 值全换成合成串，**键名、顺序与版本**仍与实拍逐字一致。
	want := `{"deviceId":"SYNTH-DEVICE-199","loginConfig":"redpacketLoginConfig",` +
		`"pintype":13,"token":"SYNTH-SSO-TOKEN-199","version":"1.0"}`
	if got := string(request.Body); got != want {
		t.Errorf("请求体应逐字等于实拍形状\n实得 %s\n应为 %s", got, want)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个，实得 %d", n)
	}
}

// TestParseCloudPhoneTokenValidateReadsTheCapture ⭐ 读数的每一格都指到第 29 条响应。
//
// 反向改坏它：把 `expireTime` 当毫秒时间戳解释（夹具第 1 条**另一个域**那一枚就是毫秒）
// ⇒ 这里只比原始读数 3600，任何换算都会红；把 account 折成空串 ⇒ 红。
func TestParseCloudPhoneTokenValidateReadsTheCapture(t *testing.T) {
	got, perr := ParseCloudPhoneTokenValidate(cloudPhoneEnvelope(t,
		`{"data":{"account":"199****0001","expireTime":3600,"phone":"SYNTH-PHONE-FIELD",`+
			`"riskLevel":"PASS","token":"SYNTH-ACTIVITY-TOKEN-199"},`+
			`"header":{"appId":"12345681","errMsg":"OK","status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Token != "SYNTH-ACTIVITY-TOKEN-199" {
		t.Errorf("token 是整族的入口票，实得 %q", got.Token)
	}
	if got.Account == nil || *got.Account != "199****0001" {
		t.Errorf("account（上游给的掩码）实得 %v", got.Account)
	}
	if got.RiskLevel == nil || *got.RiskLevel != "PASS" {
		t.Errorf("riskLevel 实得 %v", got.RiskLevel)
	}
	// 只交**原始读数**：3600 是秒还是别的，成文与样本都没说。
	if got.ExpireTime == nil || *got.ExpireTime != 3600 {
		t.Errorf("expireTime 实拍是数字 3600，实得 %v", got.ExpireTime)
	}
}

// TestParseCloudPhoneTokenValidateRefusesToAssumeSuccess ⭐ 换不到票就不是登录成功。
//
// 这一条的失败必须**显式**：往后每一枪都带这一枚 token，
// 把它伪造成成功会让整族表现为"活动结束了"。
func TestParseCloudPhoneTokenValidateRefusesToAssumeSuccess(t *testing.T) {
	for name, body := range map[string]string{
		"status 非 200":    `{"header":{"status":"500","errMsg":"登录失败"},"data":{"token":"SYNTH-A"}}`,
		"data 里没有 token":  `{"header":{"status":"200"},"data":{"account":"199****0001"}}`,
		"token 是空串":       `{"header":{"status":"200"},"data":{"token":""}}`,
		"token 是数字（类型变了）": `{"header":{"status":"200"},"data":{"token":199}}`,
		"data 缺失":         `{"header":{"status":"200"}}`,
	} {
		if _, perr := ParseCloudPhoneTokenValidate(cloudPhoneEnvelope(t, body)); perr == nil {
			t.Errorf("%s：应报错，绝不能判成登录成功", name)
		}
	}
	// 反向：其它字段缺了**不**报错（本层只判"有没有换到票"这一格）。
	got, perr := ParseCloudPhoneTokenValidate(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"token":"SYNTH-ACTIVITY-TOKEN-199"}}`))
	if perr != nil {
		t.Fatalf("只缺旁证字段不该打断: %v", perr)
	}
	if got.Account != nil || got.RiskLevel != nil || got.ExpireTime != nil {
		t.Errorf("缺的位应是 nil，实得 %#v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 有实拍之二：`redpacket/userAccountInfo`（第 35 条）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneAccountInfoBodyMatchesCapture ⭐ 逐字节对第 35 条（实拍 43 字节）。
//
// 反向改坏它：给这条少带 `platformType`（它是这一条**独有**的键）⇒ 红。
func TestCloudPhoneAccountInfoBodyMatchesCapture(t *testing.T) {
	request, perr := CloudPhoneAccountInfoRequest(cloudPhoneSyntheticAuth(), cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"platformType":1,"version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
		t.Errorf("请求体应逐字等于实拍\n实得 %s\n应为 %s", request.Body, want)
	}
	if len(request.Body) != 43 {
		t.Errorf("实拍 content-length 是 43，实得 %d", len(request.Body))
	}
	if want := CloudPhoneBaseURL + "/redpacket/userAccountInfo"; request.URL != want {
		t.Errorf("URL 实得 %q", request.URL)
	}
}

// TestParseCloudPhoneAccountInfoReadsTheCapture ⭐ 余额的每一格都指到第 35 条。
//
// 反向改坏它：在本层做 `/100`（成文那句"代码 /100 显示元"是**展示层**的事）⇒
// 210 那一条红；把字符串金额位当"读不出"⇒ 同一条红。
func TestParseCloudPhoneAccountInfoReadsTheCapture(t *testing.T) {
	got, perr := ParseCloudPhoneAccountInfo(cloudPhoneEnvelope(t,
		`{"data":{"info":{"account":"199****0001","canAmount":"210","createTime":"2024-10-17 18:56:33",`+
			`"id":"7086772","totalAmount":"370","updateTime":"2026-06-17 20:02:58"}},`+
			`"header":{"appId":"12345681","errMsg":"OK","status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	// ⭐ 单位是**分**：这里读到的是 210，不是 2.1 也不是 2。
	if got.CanAmount == nil || *got.CanAmount != 210 {
		t.Errorf("canAmount 应是分（210），实得 %v", got.CanAmount)
	}
	if got.TotalAmount == nil || *got.TotalAmount != 370 {
		t.Errorf("totalAmount 实得 %v", got.TotalAmount)
	}
	if got.Account == nil || *got.Account != "199****0001" {
		t.Errorf("account 实得 %v", got.Account)
	}
	if got.AccountID == nil || *got.AccountID != "7086772" {
		t.Errorf("id 实拍是字符串，实得 %v", got.AccountID)
	}
	// 时间位**不解析成 time.Time**：按哪个时区解释没有证据。
	if got.CreateTime == nil || *got.CreateTime != "2024-10-17 18:56:33" {
		t.Errorf("createTime 实得 %v", got.CreateTime)
	}
	if got.UpdateTime == nil || *got.UpdateTime != "2026-06-17 20:02:58" {
		t.Errorf("updateTime 实得 %v", got.UpdateTime)
	}
}

// TestParseCloudPhoneAccountInfoMissingIsNilNotZero ⭐ "不知道" ≠ "没有"。
//
// 这一族产出的是钱：把缺省折成 0，用户看到的是"余额被清零了"。
//
// 反向改坏它：`optInt64Field` 换成"读不到给 0"⇒ 前两档红；
// 给 `info` 缺失那一档加一条报错 ⇒ 第三档红（只读口要能落成"这一格不知道"）。
func TestParseCloudPhoneAccountInfoMissingIsNilNotZero(t *testing.T) {
	// 给了 0 与没给，必须分得开。
	zero, perr := ParseCloudPhoneAccountInfo(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"info":{"canAmount":"0","totalAmount":"0"}}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if zero.CanAmount == nil || *zero.CanAmount != 0 {
		t.Errorf("给了 0 就该读到 0，实得 %v", zero.CanAmount)
	}
	// 只缺 canAmount ⇒ 那一格 nil，其余照读。
	half, perr := ParseCloudPhoneAccountInfo(cloudPhoneEnvelope(t,
		`{"header":{"status":"200"},"data":{"info":{"totalAmount":"370"}}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if half.CanAmount != nil {
		t.Errorf("没给的那一格该是 nil，实得 %d", *half.CanAmount)
	}
	if half.TotalAmount == nil || *half.TotalAmount != 370 {
		t.Errorf("totalAmount 应照读，实得 %v", half.TotalAmount)
	}
	// info 整个缺失 ⇒ 全 nil、**不报错**（"前一次没读到"与"接口挂了"要分得开）。
	for name, body := range map[string]string{
		"没有 data":        `{"header":{"status":"200"}}`,
		"info 不是对象":      `{"header":{"status":"200"},"data":{"info":"x"}}`,
		"canAmount 类型不对": `{"header":{"status":"200"},"data":{"info":{"canAmount":{}}}}`,
	} {
		got, perr := ParseCloudPhoneAccountInfo(cloudPhoneEnvelope(t, body))
		if perr != nil {
			t.Errorf("%s：只读口不该报错: %v", name, perr)
			continue
		}
		if got.CanAmount != nil || got.TotalAmount != nil {
			t.Errorf("%s：该是 nil 而不是 0，实得 %#v", name, got)
		}
	}
	// 信封不过 ⇒ 必须报错（这与"载荷读不出"是两件事）。
	if _, perr := ParseCloudPhoneAccountInfo(cloudPhoneEnvelope(t,
		`{"header":{"status":"500","errMsg":"额度查询失败"},"data":{}}`)); perr == nil {
		t.Error("信封不过时应报错")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 有实拍之三：`redpacket/userToastInfo`（第 37 条）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneToastInfoBodyAndPayload ⭐ body 逐字对第 37 条；载荷只有 `isShowCloud`。
//
// 成文把这条口记成"播报**金额**"，而实拍响应里一个金额都没有 ⇒
// 这条测试同时是那个矛盾的记录点：它只钉"读到 1"，不钉任何金额。
func TestCloudPhoneToastInfoBodyAndPayload(t *testing.T) {
	request, perr := CloudPhoneToastInfoRequest(cloudPhoneSyntheticAuth(), cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := `{"version":"SYS_CONFIG_Y"}`; string(request.Body) != want {
		t.Errorf("请求体应逐字等于实拍\n实得 %s", request.Body)
	}
	if len(request.Body) != 26 {
		t.Errorf("实拍三条（第 32/36/37 条）content-length 都是 26，实得 %d", len(request.Body))
	}
	got, perr := ParseCloudPhoneToastInfo(cloudPhoneEnvelope(t,
		`{"data":{"isShowCloud":1},"header":{"appId":"12345681","errMsg":"OK","status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.IsShowCloud == nil || *got.IsShowCloud != 1 {
		t.Errorf("isShowCloud 实得 %v", got.IsShowCloud)
	}
	// 缺 ⇒ nil（不是 0）。这一族"非 0 是什么"零样本，把 nil 折成 0 就是凭空造一档。
	missing, perr := ParseCloudPhoneToastInfo(cloudPhoneEnvelope(t,
		`{"data":{},"header":{"status":"200"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if missing.IsShowCloud != nil {
		t.Errorf("缺位应是 nil，实得 %d", *missing.IsShowCloud)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务状态位那一档命名（五档 + 未知）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhoneUserStatusKeepsTheFiveBucketsApart ⭐ 特别是 `2`：
// 成文明写它"未定义但旧实现会进执行分支"，所以它**既不**折进已完成、**也不**折进未完成。
//
// 反向改坏它：把 `2` 的 case 删掉（落到 default）⇒ 第一条红；
// 把未知值 7 折成"未完成"（等于自动去做一件状态未知的任务）⇒ 最后一档红。
func TestCloudPhoneUserStatusKeepsTheFiveBucketsApart(t *testing.T) {
	names := map[CloudPhoneUserStatus]string{
		CloudPhoneUserStatusUnfinished:     "未完成",
		CloudPhoneUserStatusCompleted:      "已完成",
		CloudPhoneUserStatusUndefined2:     "未定义(2)",
		CloudPhoneUserStatusPrizeExhausted: "奖品已兑完",
		CloudPhoneUserStatusTomorrow:       "明天再来",
	}
	seen := map[string]CloudPhoneUserStatus{}
	for status, want := range names {
		if got := status.String(); got != want {
			t.Errorf("%d 的档位名应是 %q，实得 %q", int64(status), want, got)
		}
		if !status.Known() {
			t.Errorf("%d 是成文那五档之一，Known() 不该是 false", int64(status))
		}
		if other, dup := seen[want]; dup {
			t.Errorf("%d 与 %d 撞了同一档名 ⇒ 两档被折成一档", int64(status), int64(other))
		}
		seen[want] = status
	}
	// `2` 与 `0`/`1` 必须是三个不同的读数（处置方向相反）。
	if CloudPhoneUserStatusUndefined2.String() == CloudPhoneUserStatusUnfinished.String() ||
		CloudPhoneUserStatusUndefined2.String() == CloudPhoneUserStatusCompleted.String() {
		t.Error("2 那一档被折进了别的档")
	}
	unknown := CloudPhoneUserStatus(7)
	if unknown.Known() {
		t.Error("7 不是成文那五档")
	}
	if !strings.Contains(unknown.String(), "未知") {
		t.Errorf("没见过的取值单独成一档文本，实得 %q", unknown.String())
	}
	if unknown.String() == CloudPhoneUserStatusUnfinished.String() {
		t.Error("未知折成了未完成 ⇒ 会被当成\"还没做，去做\"")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层（假客户端，不联网）
// ─────────────────────────────────────────────────────────────────────────────

// TestAppCloudPhoneAccountInfoUsesTheInjectedClient 走一遍"注入假客户端"这条路，
// 顺带钉住**发送层也判信封**：载荷之外还要有一条非 200 的响应。
func TestAppCloudPhoneAccountInfoUsesTheInjectedClient(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"data":{"info":{"canAmount":"210","totalAmount":"370"}},`+
			`"header":{"appId":"12345681","errMsg":"OK","status":"200"}}`))}
	got, perr := NewUpstream(client).AppCloudPhoneAccountInfo(nil, cloudPhoneSyntheticAuth(),
		cloudPhoneSyntheticReferer)
	if perr != nil {
		t.Fatalf("调用失败: %v", perr)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应恰好发一条，实得 %d", len(client.requests))
	}
	if want := CloudPhoneBaseURL + "/redpacket/userAccountInfo"; client.requests[0].URL != want {
		t.Errorf("URL 实得 %q", client.requests[0].URL)
	}
	if got.CanAmount == nil || *got.CanAmount != 210 {
		t.Errorf("实得 %v", got.CanAmount)
	}
	// HTTP 200 而信封 status 非 200 ⇒ 必须报错（[Upstream.Call] 只看状态码）。
	bad := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"活动未开始"},"data":{}}`))}
	if _, perr := NewUpstream(bad).AppCloudPhoneAccountInfo(nil, cloudPhoneSyntheticAuth(), ""); perr == nil {
		t.Error("status 非 200 时发送层必须报错，不能交一个空读数")
	}
	// 状态码非 2xx 时**不带**响应体原文进错误（那里常常回显凭据）。
	gone := &scriptedClient{response: core.NewResponse(404,
		[]byte(`{"header":{"status":"404","token":"SYNTH-ECHOED-CREDENTIAL"}}`))}
	_, perr = NewUpstream(gone).AppCloudPhoneAccountInfo(nil, cloudPhoneSyntheticAuth(), "")
	if perr == nil {
		t.Fatal("非 2xx 应报错")
	}
	if strings.Contains(perr.Message(), "SYNTH-ECHOED") {
		t.Error("错误文案里不该出现上游响应体原文")
	}
}

// TestAppCloudPhoneTokenValidateDoesNotLeakTheTokenIntoErrors ⭐ 凭据不进文案。
func TestAppCloudPhoneTokenValidateDoesNotLeakTheTokenIntoErrors(t *testing.T) {
	auth := cloudPhoneSyntheticAuth()
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"header":{"status":"500","errMsg":"风控拦截"},"data":{"errorMsg":"请稍后再试"}}`))}
	got, perr := NewUpstream(client).AppCloudPhoneTokenValidate(nil, auth, "",
		cloudPhoneSyntheticDeviceID, cloudPhoneSyntheticSSO)
	if perr == nil {
		t.Fatal("非 200 应报错")
	}
	if got.Token != "" {
		t.Error("失败时不该交出一个 token")
	}
	for _, text := range []string{perr.Message(), client.requests[0].URL} {
		if strings.Contains(text, cloudPhoneSyntheticToken) ||
			strings.Contains(text, cloudPhoneSyntheticSSO) {
			t.Errorf("错误文案/URL 里出现了凭据：%q", text)
		}
	}
	// 但请求头里必须有它（这是它唯一该去的地方）。
	if got := headerValue(client.requests[0].Headers, "token"); got != cloudPhoneSyntheticToken {
		t.Errorf("token 头应是素材那一枚，实得 %q", got)
	}
}

// cloudPhoneEnvelope 把夹具那类原文解成信封。
//
// ⚠ 必须走 [ParseEnvelope]（`UseNumber`）：夹具里 `expireTime` 是 19 位、
// `account` 是掩码串，用默认解码会静默丢精度。
func cloudPhoneEnvelope(t *testing.T, body string) *Envelope {
	t.Helper()
	envelope, perr := ParseEnvelope([]byte(body))
	if perr != nil {
		t.Fatalf("夹具不是合法 JSON: %v", perr)
	}
	return envelope
}
