package upstream

// 红包派对族 · 两枚"曾被判为造不出来"的头（`x-DeviceInfo` / `x-origin`）。
//
// ⚠ **零实拍**：夹具十条里这两枚**头名在、值全被遮**。所以本文件钉的是两件事：
//  1. 与**旧 TS `cloud-phone-redpack/task.ts:57-71` 的字面量**一致（那是眼下最高一级出处）；
//  2. 我们自己的纪律：**没给值就不发**（opt-in），并且**不**把另一族那套设备形状搬进来。
//
// 这里**不**出现任何"上游认这一串 / 不认这一串"的句子（`AGENTS.md` §1.7）。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// fixedRand 是只喂固定字节的随机源（长度与字符集要能断言，但**不**依赖真实熵）。
type fixedRand struct{ b byte }

func (f *fixedRand) FillBytes(buf []byte) {
	for i := range buf {
		buf[i] = f.b
	}
}

func TestCloudPhoneDeviceInfoMatchesLegacyTSTemplate(t *testing.T) {
	got := CloudPhoneDeviceInfo("1991234567890", "abcdefghijklmnop")
	// 旧 TS `task.ts:66`：`|h5|1.0.0|v1.0.0||<UA>|${timestamp}${randomStr}|||375X667|zh||`
	want := "|h5|1.0.0|v1.0.0||" + CloudPhoneDeviceInfoUA +
		"|1991234567890abcdefghijklmnop|||375X667|zh||"
	if got != want {
		t.Errorf("整串与旧 TS 模板不一致：\n实得 %q\n Want %q", got, want)
	}
	if n := strings.Count(got, "|") + 1; n != CloudPhoneDeviceInfoSegments {
		t.Errorf("段数实得 %d，成文 %d（空段被删或多塞都会改这一格）", n, CloudPhoneDeviceInfoSegments)
	}
	if !strings.HasPrefix(got, "|") {
		t.Error("首段必须是空串（串以 `|` 开头）")
	}
	segments := strings.Split(got, "|")
	if segments[4] != "" {
		t.Errorf("`v1.0.0` 之后那一格实拍写法是空段，实得 %q", segments[4])
	}
	// ⚠ 第 7 段（下标 6）里 timestamp 与随机尾巴**相邻无分隔**：中间多一个 `|` 就变成两截。
	if segments[6] != "1991234567890abcdefghijklmnop" {
		t.Errorf("timestamp+尾巴应直接拼成一格，实得 %q", segments[6])
	}
	if segments[9] != "375X667" {
		t.Errorf("分辨率那一格实得 %q（注意大小写：这里是 `X`，另一族是 ` * `）", segments[9])
	}
}

func TestCloudPhoneDeviceInfoUAIsNotTheRequestUA(t *testing.T) {
	// 反向改坏它：有人"顺手统一"把两处 UA 抽成一个变量 ⇒ 这条红。
	// 真实客户端那两份字面量**就是不同**（少一段 `like Mac OS X`）。
	if CloudPhoneDeviceInfoUA == CloudPhoneUserAgent {
		t.Fatal("两枚 UA 变成同一串了")
	}
	if !strings.Contains(CloudPhoneUserAgent, "like Mac OS X") {
		t.Error("请求头那枚 UA 应含 `like Mac OS X`")
	}
	if strings.Contains(CloudPhoneDeviceInfoUA, "like Mac OS X") {
		t.Error("设备串里那枚 UA **不该**含 `like Mac OS X`（旧 TS 两份不同字面量）")
	}
}

func TestCloudPhoneXOriginAndRefererAreDifferentStrings(t *testing.T) {
	if CloudPhoneXOriginHomeURL == CloudPhoneRefererHomeURL {
		t.Fatal("两枚头被统一成同一串了：真实客户端 referer 是根、x-origin 是带路由的完整地址")
	}
	want := "https://cpactiv.buy.139.com/#/redEnvelopeParty/home" +
		"?loginType=0&enableShare=1&channelSrc=red-mcloudapp-001"
	if CloudPhoneXOriginHomeURL != want {
		t.Errorf("x-origin 实得 %q，want %q（三个查询键的**顺序**也是断言面）", CloudPhoneXOriginHomeURL, want)
	}
	if CloudPhoneRefererHomeURL != "https://cpactiv.buy.139.com/" {
		t.Errorf("referer 实得 %q", CloudPhoneRefererHomeURL)
	}
	// 不带 token：这一族**不许**把凭据拼进成文的常量。
	if strings.Contains(CloudPhoneXOriginHomeURL, "token=") {
		t.Error("x-origin 里出现了 token 键（实拍那一版带，但成文与常量里不许）")
	}
}

func TestCloudPhoneDeviceInfoSuffixShape(t *testing.T) {
	got := CloudPhoneDeviceInfoSuffix(&fixedRand{b: 7})
	if len(got) != CloudPhoneDeviceInfoRandomLen {
		t.Fatalf("长度实得 %d，want %d", len(got), CloudPhoneDeviceInfoRandomLen)
	}
	for _, r := range got {
		if !strings.ContainsRune("0123456789abcdefghijklmnopqrstuvwxyz", r) {
			t.Fatalf("字符集越界：%q", r)
		}
	}
	// 与 `requestId` 那枚尾巴**长度不同**，互相顶替要红。
	if CloudPhoneRequestIDSuffixLen == CloudPhoneDeviceInfoRandomLen {
		t.Fatal("两枚随机尾巴被统一成长度相同了")
	}
}

func TestCloudPhoneTimestampMilliIsPlainDecimal(t *testing.T) {
	got := CloudPhoneTimestampMilli(1991234567890)
	if got != "1991234567890" {
		t.Errorf("实得 %q（旧 TS 那一格是 `Date.now().toString()` ⇒ 纯十进制毫秒）", got)
	}
	for _, bad := range []string{"-", ":", ".", " "} {
		if strings.Contains(got, bad) {
			t.Errorf("串里出现了 %q：格式化多了分隔符", bad)
		}
	}
}

func TestWithCloudPhoneAuthSendsTheTwoOptionalHeadersOnlyWhenGiven(t *testing.T) {
	base := cloudPhoneSyntheticAuth()

	// ① 都不给 ⇒ 两枚都不发（默认态不许变：现在的十条口都按这个走）。
	quiet := WithCloudPhoneAuth(mustCloudPhoneTestRequest(t), base)
	for _, name := range []string{"x-DeviceInfo", "x-origin"} {
		if n := countHeader(quiet.Headers, name); n != 0 {
			t.Errorf("没给值却发了 %s，共 %d 枚", name, n)
		}
	}

	// ② 都给出 ⇒ 逐字发出去，且**各只一枚**（重复头会让服务端读到哪一枚都不确定）。
	full := base
	full.DeviceInfo = CloudPhoneDeviceInfo(full.Timestamp, "abcdefghijklmnop")
	full.XOrigin = CloudPhoneXOriginHomeURL
	loud := WithCloudPhoneAuth(mustCloudPhoneTestRequest(t), full)
	if got := headerValue(loud.Headers, "x-origin"); got != CloudPhoneXOriginHomeURL {
		t.Errorf("x-origin 实得 %q", got)
	}
	if got := headerValue(loud.Headers, "x-DeviceInfo"); got != full.DeviceInfo {
		t.Errorf("x-DeviceInfo 实得 %q", got)
	}
	for _, name := range []string{"x-DeviceInfo", "x-origin"} {
		if n := countHeader(loud.Headers, name); n != 1 {
			t.Errorf("%s 发了 %d 枚，want 1", name, n)
		}
	}

	// ③ 只给一枚 ⇒ 另一枚仍不发（"顺手补一个默认值"要红在这里）。
	one := base
	one.XOrigin = CloudPhoneXOriginHomeURL
	half := WithCloudPhoneAuth(mustCloudPhoneTestRequest(t), one)
	if n := countHeader(half.Headers, "x-DeviceInfo"); n != 0 {
		t.Errorf("没给 DeviceInfo 却发了 x-DeviceInfo，共 %d 枚", n)
	}
}

// mustCloudPhoneTestRequest 取一条**没贴过鉴权头**的裸请求当底板
// （用 [CloudPhoneRequest] 会先把鉴权头贴一遍，再贴一次就测不出"重复头"这一档了）。
func mustCloudPhoneTestRequest(t *testing.T) core.HTTPRequest {
	t.Helper()
	return core.NewRequestPost(CloudPhoneBaseURL + "/redpacket/userToastInfo")
}
