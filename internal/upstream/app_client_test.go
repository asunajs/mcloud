package upstream

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// 抓包里那台设备的脱敏形态，全部断言都以它为准。
func appDevice() AppDevice {
	return NewIPadAppDevice("AAA00000000000000000000000000000", "AAA000000000")
}

func appProfile() *AppProfile { return NewAppProfile(appDevice()) }

// headerOf 取一个头；**缺了直接失败**（而不是返回空串让后面的比较自己去解释）。
func headerOf(t *testing.T, headers []core.HeaderPair, name string) string {
	t.Helper()
	for _, h := range headers {
		if h[0] == name {
			return h[1]
		}
	}
	t.Fatalf("缺少头 %s", name)
	return ""
}

func hasHeader(headers []core.HeaderPair, name string) bool {
	for _, h := range headers {
		if h[0] == name {
			return true
		}
	}
	return false
}

func TestDeviceInfoHas14SegmentsAndEndsWithABar(t *testing.T) {
	info := appDevice().DeviceInfo()
	want := "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|"
	if info != want {
		t.Errorf("设备串与实测不符:\n 期望 %s\n 实得 %s", want, info)
	}

	segments := strings.Split(info, "|")
	if len(segments) != 14 {
		t.Fatalf("设备串是 13 个字段 + 一个空的尾段 = 14，实得 %d（段数变了说明抄错了模板）", len(segments))
	}
	if segments[4] != "APPLE" {
		t.Errorf("第 5 段应是 APPLE: %s", segments[4])
	}
	if segments[5] != "iPad8,6" {
		t.Errorf("机型是明文，不是 base64: %s", segments[5])
	}
	if segments[8] != "ipados 26.6" {
		t.Errorf("第 9 段不对: %s", segments[8])
	}
	if segments[9] != "375 * 667" {
		t.Errorf("分辨率段里的空格是原样: %s", segments[9])
	}
	if segments[len(segments)-1] != "" {
		t.Errorf("原串以 | 结尾，末段应为空: %q", segments[len(segments)-1])
	}
}

func TestClientInfoIs16SegmentsAndDiffersFromDeviceInfo(t *testing.T) {
	info := appDevice().ClientInfo()
	want := "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|aVBhZA==|0|;"
	if info != want {
		t.Errorf("client-info 与实测不符:\n 期望 %s\n 实得 %s", want, info)
	}

	segments := strings.Split(info, "|")
	if len(segments) != 16 {
		t.Fatalf("client-info 应是 16 段，实得 %d", len(segments))
	}
	if segments[13] != "aVBhZA==" {
		t.Errorf("第 14 段是 x-DeviceName 的 base64: %s", segments[13])
	}
	if segments[14] != "0" || segments[15] != ";" {
		t.Errorf("末两段应是 0|; : %s|%s", segments[14], segments[15])
	}

	// 两端最容易抄错的一处：PC 端 `x-yun-client-info` 与 `x-DeviceInfo` 同值，
	// App 端**不同值**。这里钉住"不同"这件事本身。
	if appDevice().ClientInfo() == appDevice().DeviceInfo() {
		t.Error("App 端这两个头不是同一个值（PC 端才是）")
	}
}

func TestThreeOSVersionSpellingsAreAllPresent(t *testing.T) {
	d := appDevice()
	// 同一个系统版本在三处三种写法 —— 统一成一种会让上游的正则失配
	if !strings.Contains(d.DeviceInfo(), "|ipados 26.6|") {
		t.Errorf("设备串里应是 ipados 小写: %s", d.DeviceInfo())
	}
	if got := d.XUserAgent(); got != "412|iPad|ios 26.6|12.6.0-200" {
		t.Errorf("x-UserAgent 不对: %s", got)
	}
	if got := d.YunUserAgent(); got != "iPad|iPad|ios 26.6|mCloud12.6.0-200" {
		t.Errorf("x-yun-user-agent 不对: %s", got)
	}
	if got := d.UserAgent(); got != "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)" {
		t.Errorf("User-Agent 不对: %s", got)
	}
}

func TestOSVersionOverrideReachesAllThreeSpellings(t *testing.T) {
	d := appDevice().WithOSVersion("18.2")
	if !strings.Contains(d.DeviceInfo(), "|ipados 18.2|") {
		t.Errorf("设备串没跟着改: %s", d.DeviceInfo())
	}
	if !strings.Contains(d.XUserAgent(), "|ios 18.2|") {
		t.Errorf("x-UserAgent 没跟着改: %s", d.XUserAgent())
	}
	if !strings.Contains(d.UserAgent(), "(iPad; iOS 18.2;") {
		t.Errorf("User-Agent 没跟着改（这一处是大写 iOS）: %s", d.UserAgent())
	}
}

func TestAppHeadersMatchTheCapturedSet(t *testing.T) {
	p := appProfile()
	h := p.Headers(1789743362439040)

	cases := map[string]string{
		"x-yun-user-timestamp":   "1789743362439040",
		"Pragma":                 "no-cache",
		"x-yun-app-channel":      "10000020",
		"x-huawei-channelSrc":    "10000020",
		"x-yun-net-type":         "1",
		"x-yun-svc-type":         "1",
		"x-SvcType":              "1",
		"x-yun-api-version":      "v1",
		"Accept-Language":        "zh-CN",
		"service_identifier_key": "service_identifier_userDomain",
		"x-DeviceName":           "aVBhZA==",
		"x-yun-client-info":      p.Device().ClientInfo(),
		"x-DeviceInfo":           p.Device().DeviceInfo(),
		"x-UserAgent":            p.Device().XUserAgent(),
		"User-Agent":             p.Device().UserAgent(),
		"x-yun-user-agent":       p.Device().YunUserAgent(),
	}
	for name, want := range cases {
		if got := headerOf(t, h, name); got != want {
			t.Errorf("%s: 期望 %q 实得 %q", name, want, got)
		}
	}

	// `x-yun-uni` 在首次登录时**不该出现**：那时还没有 userDomainId，
	// 编一个值会让上游定位到别人的用户域。
	if hasHeader(h, "x-yun-uni") {
		t.Error("未登录时不应带 x-yun-uni")
	}
}

func TestUserDomainIDIsOptionalAndOnlyAddedWhenKnown(t *testing.T) {
	p := appProfile().WithUserDomainID("1990000000000000001")
	if got := headerOf(t, p.Headers(1), "x-yun-uni"); got != "1990000000000000001" {
		t.Errorf("x-yun-uni 不对: %s", got)
	}
}

func TestAppHeadersDoNotCarryPCOnlyNames(t *testing.T) {
	h := appProfile().Headers(1)
	for _, pcOnly := range []string{"x-MM-Source", "x-yun-tid"} {
		if hasHeader(h, pcOnly) {
			t.Errorf("%s 是 PC 端才有的头，App 端抓包里没有", pcOnly)
		}
	}
}

func TestAppAuthorizationUsesTheMobilePrefixNotPC(t *testing.T) {
	const token = "SYNTH001|1|RCS|1893456000000|AAAA"
	auth := AppAuthorization("19900000000", token)

	// 逐字节钉住整个头。比"解回来看看"更强：前缀写错时 base64 仍然合法，
	// 只有整串比对才挡得住。
	want := "Basic bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAxfDF8UkNTfDE4OTM0NTYwMDAwMDB8QUFBQQ=="
	if auth != want {
		t.Errorf("与实测不符:\n 期望 %s\n 实得 %s", want, auth)
	}

	// 反证：PC 的前缀会得到另一串（这一条防止有人把两端的构造合并成一个）
	pc := "Basic cGM6MTk5MDAwMDAwMDA6U1lOVEgwMDF8MXxSQ1N8MTg5MzQ1NjAwMDAwMHxBQUFB"
	if auth == pc {
		t.Error("pc 前缀是 PC 端的形状，App 不能用")
	}
}

func TestAnonymousAuthorizationMatchesTheCapturedBytes(t *testing.T) {
	// 字面量 `(null)` 而不是空串 —— 照抄实测，别"顺手清理"成 `mobile::`
	if AnonymousAuthorization != "Basic bW9iaWxlOihudWxsKToobnVsbCk=" {
		t.Errorf("占位头与实测不符: %s", AnonymousAuthorization)
	}
	// 与构造函数自洽：用 `(null)` 当凭据应当得到同一个头
	if got := AppAuthorization("(null)", "(null)"); got != AnonymousAuthorization {
		t.Errorf("构造函数与常量不自洽:\n 常量 %s\n 构造 %s", AnonymousAuthorization, got)
	}
}

// App 端的 `Authorization` 与 `capabilities` 那侧拼出来的必须是同一串。
//
// 这条把两端（PC 用 `pc`、App 用 `mobile`）的**前缀来源**钉到一处：
// 哪天有人给 App 端登录结果误用了 PC 的构造函数，这里会立刻响。
func TestAppAuthorizationMatchesCapabilitiesWithMobilePrefix(t *testing.T) {
	// 与 capabilities.AuthFromUserInfoWithPrefix(…, "mobile") 同一条公式
	payload := "mobile:19900000000:SYNTH001|1|RCS|1893456000000|AAAA"
	if got := AppAuthorization("19900000000", "SYNTH001|1|RCS|1893456000000|AAAA"); got != "Basic "+core.B64Encode([]byte(payload)) {
		t.Errorf("App 端构造与通用构造不一致: %s", got)
	}
}
