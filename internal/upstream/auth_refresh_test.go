package upstream

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// 换 token 这一条的测试分两块，**判据强度不一样**：
//   - 请求形状（URL / 体 / 头）有 App 实拍可对拍 ⇒ 断到字面量；
//   - PC 那一支没有抓包，只能断"**档案给的码，而不是调用方给的码**"这一条
//     （它是我们唯一能自己保证的事；上游收不收，要等真跑一次才知道）。
//
// 响应解析那条信封是 `success` + 字符串 `code` 的家族 —— 两半都要各钉一次，
// 缺一条的表现是"业务失败被当成功，拿着空 token 覆盖库里那串还能用的"。

// 合成值（`199` 号段，见 docs/credential-boundary.md）。
const (
	refreshDomain = "1990000000000000001"
	refreshPhone  = "19900000001"
)

// refreshTokenNew 是"刷新回来的那一个"合成 authToken：
// 保持实拍的形状（`|` 分 5 段、段长 [8,1,3,13,172]，总长 201），值全假。
var refreshTokenNew = "AbCdEfGh|1|RCS|1792245335331|" +
	strings.Repeat("A", 170) + "0-"

func TestRefreshTokenBodyKeepsTheCapturedKeyOrder(t *testing.T) {
	// 实拍原串：`{"userDomainId":"…","clientType":"412"}`（两个键、这个顺序）。
	// 顺序不是审美问题：[ThirdLoginBody] 那处说明过，"发出去的字节"是唯一
	// 能对拍的东西；这里以实拍为准（Rust 侧没有这条接口，不必向字典序靠）。
	body := &RefreshTokenBody{UserDomainID: refreshDomain, ClientType: IOSClientType}
	got, err := core.MarshalJSON(body)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	want := `{"userDomainId":"1990000000000000001","clientType":"412"}`
	if string(got) != want {
		t.Errorf("请求体实得 %s，期望 %s", got, want)
	}
}

func TestAppRefreshRequestShape(t *testing.T) {
	authorization := AppAuthorization(refreshPhone, "旧token")
	request, perr := AppRefreshTokenRequest(
		testAppProfile(), 1727000000000000, authorization,
		&RefreshTokenBody{UserDomainID: refreshDomain, ClientType: IOSClientType})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.URL != "https://user-njs.yun.139.com/user/auth/refreshToken" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	if got := headerValue(request.Headers, "Authorization"); got != authorization {
		t.Errorf("Authorization 实得 %q", got)
	}
	// 这个接口比 A 族其余接口多的两个头，**本来就在** [AppProfile.Headers] 里；
	// 这里钉住它，是为了让"哪天有人给 Headers 动了刀"能在这里响。
	if got := headerValue(request.Headers, "x-yun-api-version"); got != "v1" {
		t.Errorf("x-yun-api-version 实得 %q", got)
	}
	if got := headerValue(request.Headers, "x-UserAgent"); !strings.HasPrefix(got, "412|") {
		t.Errorf("x-UserAgent 第 1 段应是 clientType，实得 %q", got)
	}
	// 与 [TestAppRequestsNeverSendHcyCoolFlag] 同一条判据：这条接口也走明文。
	if countHeader(request.Headers, "hcy-cool-flag") != 0 {
		t.Error("发 hcy-cool-flag 会让响应变成我们解不开的密文")
	}
	if !strings.Contains(string(request.Body), `"clientType":"412"`) {
		t.Errorf("体里应带 App 的 clientType: %s", request.Body)
	}
}

// TestPCRefreshRequestTakesItsClientTypeFromTheProfile 钉的是**跨端常量不许互搬**。
//
// PC 那一支没有抓包，所以这里不断言"上游会接受"，只断言"我们不会发错端"：
// 码只能来自档案的平台，调用点拿不到它 —— 一旦哪天有人加一个 `clientType` 入参，
// "PC 档案配 App 的 412"这种上游一眼可辨的矛盾组合就能静默通过（本仓库记过五次）。
func TestPCRefreshRequestTakesItsClientTypeFromTheProfile(t *testing.T) {
	for _, c := range []struct {
		name     string
		platform PcPlatform
		want     string
	}{
		{"windows", PlatformWindows, "656"},
		{"macos", PlatformMacOS, "710"},
	} {
		t.Run(c.name, func(t *testing.T) {
			profile := NewClientProfile(c.platform, "DEV-1")
			request, perr := PCRefreshTokenRequest(
				profile, "tid-1", "Basic cGM6",
				&RefreshTokenBody{UserDomainID: refreshDomain, ClientType: profile.Platform().ClientType()})
			if perr != nil {
				t.Fatalf("构造失败: %v", perr)
			}
			if !strings.Contains(string(request.Body), `"clientType":"`+c.want+`"`) {
				t.Errorf("应带 %s 的码: %s", c.name, request.Body)
			}
			if strings.Contains(string(request.Body), `"412"`) {
				t.Errorf("PC 的体里出现了 App 的 412: %s", request.Body)
			}
			if got := headerValue(request.Headers, "x-yun-tid"); got != "tid-1" {
				t.Errorf("PC 的端头集合里应有 x-yun-tid，实得 %q", got)
			}
		})
	}
}

func TestParseRefreshTokenReadsTheSuccessPlusStringCodeFamily(t *testing.T) {
	// 实拍响应（token 脱敏成 `199` 段的合成值，形状保持 201 字符 / 5 段）。
	raw := `{"success":true,"code":"0000","message":"请求成功",` +
		`"data":{"token":"` + refreshTokenNew + `","expireTime":2592000}}`
	envelope, perr := ParseEnvelope([]byte(raw))
	if perr != nil {
		t.Fatalf("解析信封: %v", perr)
	}
	got, perr := ParseRefreshToken(envelope)
	if perr != nil {
		t.Fatalf("解析: %v", perr)
	}
	if got.Token != refreshTokenNew {
		t.Errorf("token 实得 %q", got.Token)
	}
	// ⚠ 单位是**秒**、而且是"还能用多久"不是"何时过期"（见 RefreshTokenResult）
	if got.ExpireSeconds == nil || *got.ExpireSeconds != 2_592_000 {
		t.Errorf("expireTime 实得 %v", got.ExpireSeconds)
	}
}

func TestParseRefreshTokenToleratesMissingExpireTime(t *testing.T) {
	// 上游这次没回 expireTime：token 自己带 13 位毫秒过期段，所以**不算失败**。
	// 反向改坏的方式：把 optionalInt64 换成 RequireInt64 —— 这条会红。
	envelope := NewEnvelope(map[string]any{
		"success": true, "code": json.Number("0000"),
		"data": map[string]any{"token": refreshTokenNew},
	})
	got, perr := ParseRefreshToken(envelope)
	if perr != nil {
		t.Fatalf("缺 expireTime 不该失败: %v", perr)
	}
	if got.ExpireSeconds != nil {
		t.Errorf("应缺席而不是补一个 0: %v", *got.ExpireSeconds)
	}
}

func TestParseRefreshTokenFailsOnBusinessErrors(t *testing.T) {
	cases := []struct {
		name  string
		raw   string
		what  string
		equal bool
	}{
		{"success=false", `{"success":false,"code":"0000","data":{"token":"x"}}`, "success=false", false},
		{"非零码", `{"success":true,"code":"9999","message":"失败","data":{}}`, "9999", false},
		{"没有 data", `{"success":true,"code":"0000"}`, "没有 data", false},
		{"空 token", `{"success":true,"code":"0000","data":{"token":""}}`, "空串", false},
		{"token 不是字符串", `{"success":true,"code":"0000","data":{"token":123}}`, "token", false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			envelope, perr := ParseEnvelope([]byte(c.raw))
			if perr != nil {
				t.Fatalf("信封应能解析: %v", perr)
			}
			_, perr = ParseRefreshToken(envelope)
			if perr == nil {
				t.Fatal("应判失败，实得成功")
			}
			if !strings.Contains(perr.Error(), c.what) {
				t.Errorf("错误里应看得出没的是哪一样（%s），实得 %v", c.what, perr)
			}
		})
	}
}

// TestEmptyTokenNeverReachesTheCallerAsASuccess 单独把"空 token"这条拎出来：
// 它的后果是**把库里那串还能用的凭据覆盖成不可用的**，
// 而其他几条失败只是"这次没刷成"。
func TestEmptyTokenNeverReachesTheCallerAsASuccess(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"success":true,"code":"0000","data":{"token":"","expireTime":2592000}}`))}
	up := NewUpstream(client)

	_, perr := up.AppRefreshToken(context.Background(), testAppProfile(), 1, "Basic x", refreshDomain)
	if perr == nil {
		t.Fatal("空 token 必须判失败")
	}
	if kind, _ := core.KindOf(perr); kind != core.KindParse {
		t.Errorf("这是上游给的东西不合形状，应归 Parse，实得 %v", kind)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应只发一次: %d", len(client.requests))
	}
}

func TestRefreshTokenSendersReachTheScriptedClient(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(
		`{"success":true,"code":"0000","message":"请求成功","data":`+
			`{"token":"`+refreshTokenNew+`","expireTime":2592000}}`))}
	up := NewUpstream(client)

	if _, perr := up.AppRefreshToken(
		context.Background(), testAppProfile(), 1727000000000000,
		AppAuthorization(refreshPhone, "旧token"), refreshDomain); perr != nil {
		t.Fatalf("App: %v", perr)
	}
	appRequest := client.requests[0]

	if _, perr := up.PCRefreshToken(
		context.Background(), NewClientProfile(PlatformWindows, "DEV-1"), "tid-1",
		"Basic cGM6MTk5MDAwMDAwMDAxOng=", refreshDomain); perr != nil {
		t.Fatalf("PC: %v", perr)
	}
	pcRequest := client.requests[1]

	// 两端同一个 URL —— 差别全在头与体的 clientType 上（那是"端"的全部含义）
	if appRequest.URL != pcRequest.URL {
		t.Errorf("两端应打同一个接口: %q vs %q", appRequest.URL, pcRequest.URL)
	}
	if !strings.Contains(string(appRequest.Body), refreshDomain) ||
		!strings.Contains(string(pcRequest.Body), refreshDomain) {
		t.Error("两端的体里都该带 userDomainId")
	}
}

// TestRefreshTokenFixtureKeepsTheCapturedShape 钉住**夹具自己**：
// 实拍 token 是 201 字符、`|` 分 5 段、段长 [8,1,3,13,172]。
// 夹具哪天被改得不成形，用它做的断言就不再代表线上（那是假绿的一种）。
func TestRefreshTokenFixtureKeepsTheCapturedShape(t *testing.T) {
	if len(refreshTokenNew) != 201 {
		t.Fatalf("合成 token 应保持实拍的 201 字符，实得 %d", len(refreshTokenNew))
	}
	segs := strings.Split(refreshTokenNew, "|")
	want := []int{8, 1, 3, 13, 172}
	if len(segs) != len(want) {
		t.Fatalf("应 5 段，实得 %d", len(segs))
	}
	for i, n := range want {
		if len(segs[i]) != n {
			t.Errorf("第 %d 段应长 %d，实得 %d", i+1, n, len(segs[i]))
		}
	}
}
