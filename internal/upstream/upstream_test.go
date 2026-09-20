package upstream

import (
	"context"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// scriptedClient 是"按脚本回放"的假客户端：不联网，且把收到的请求留下来供断言。
//
// Rust 侧的 `NullClient` / `FixtureHttpClient` 同款：上游层只要求一个
// [core.HTTPClient] 实现，不要求任何真实网络对象 ——
// 这个前提在类型层面就成立，所以离线对拍不需要 mock 框架。
type scriptedClient struct {
	response core.HTTPResponse
	requests []core.HTTPRequest
}

func (c *scriptedClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	return c.response, nil
}

func TestUpstreamCanBeConstructedWithoutIO(t *testing.T) {
	// 这个断言的意义在于：`Upstream` 只要求一个 HTTPClient 实现，
	// 不要求任何真实网络对象 —— 测试注入的前提在类型层面就成立了。
	client := &scriptedClient{}
	_ = NewUpstream(client)
	if !hasPrefix(BaseMcloudSignin, "https://") {
		t.Error("基址应是 https")
	}
}

func TestRequestURLsAndTIDHeader(t *testing.T) {
	cases := []struct {
		name    string
		build   func() (core.HTTPRequest, *core.Error)
		wantURL string
	}{
		{"thirdlogin", func() (core.HTTPRequest, *core.Error) {
			return ThirdLoginRequest(map[string]any{})
		}, "https://user-njs.yun.139.com/user/thirdlogin"},
		{"getSmsCode", func() (core.HTTPRequest, *core.Error) {
			return SendSmsCodeRequest(map[string]any{})
		}, "https://user-njs.yun.139.com/user/sms/getSmsCode"},
		{"verfycode", func() (core.HTTPRequest, *core.Error) {
			return SliderPuzzleRequest(map[string]any{})
		}, "https://user-njs.yun.139.com/user/verfycode"},
		{"logout", func() (core.HTTPRequest, *core.Error) {
			return LogoutRequest(map[string]any{})
		}, "https://user-njs.yun.139.com/user/logout"},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			request, err := c.build()
			if err != nil {
				t.Fatalf("构造失败: %v", err)
			}
			if request.URL != c.wantURL {
				t.Errorf("URL 实得 %q", request.URL)
			}
			if request.Method != core.MethodPost {
				t.Errorf("应为 POST，实得 %v", request.Method)
			}
		})
	}

	// 每个请求都带一个新的 x-yun-tid
	profile := NewClientProfile(PlatformWindows, "DEV-1")
	base, err := ThirdLoginRequest(map[string]any{})
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	with := WithClientHeaders(base, profile, "tid-1")

	found := false
	for _, h := range with.Headers {
		if h[0] == "x-yun-tid" && h[1] == "tid-1" {
			found = true
		}
	}
	if !found {
		t.Errorf("缺少 x-yun-tid: %v", with.Headers)
	}
	// 端头也得在（少了它们上游回 9999）
	if len(with.Headers) < 6 {
		t.Errorf("端头不全：%v", with.Headers)
	}
}

func TestCallReturnsEnvelopeOnSuccess(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(`{"code":0,"data":"CIPHER"}`))}
	up := NewUpstream(client)

	env, err := up.ThirdLogin(context.Background(), NewClientProfile(PlatformWindows, "DEV-1"),
		"tid-1", NewQRThirdLoginBody(PlatformWindows, "SID-1"))
	if err != nil {
		t.Fatalf("失败: %v", err)
	}
	if payload, _ := env.Payload(); payload != "CIPHER" {
		t.Errorf("载荷实得 %v", payload)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应发出 1 个请求，实得 %d", len(client.requests))
	}
	// 业务失败不算错误，但**请求本身**要带上端头与 tid
	if len(client.requests[0].Headers) < 6 {
		t.Errorf("请求头不全：%v", client.requests[0].Headers)
	}
}

func TestCallMapsNonSuccessStatusToError(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(429, []byte(`{"msg":"too many"}`))}
	up := NewUpstream(client)

	_, err := up.Call(context.Background(), core.NewRequestGet(BaseUserYun))
	if err == nil {
		t.Fatal("非 2xx 应当是错误")
	}
	if err.Kind() != core.KindRateLimited {
		t.Errorf("实得 %v", err.Kind())
	}
	// **不带响应体原文**：上游错误体里常常直接回显凭据
	if err.Message() != "HTTP 429" {
		t.Errorf("详情应只有状态码，实得 %q", err.Message())
	}
}

func TestFetchSliderPuzzleRoundTripsThroughTheEnvelope(t *testing.T) {
	body := `{"code":0,"data":{"picture":"BG","puzzle":"PZ","picWidth":"680","puzzleLeft":"` +
		livePuzzleLeftCipher(t) + `"}}`
	client := &scriptedClient{response: core.NewResponse(200, []byte(body))}
	up := NewUpstream(client)

	puzzle, cerr := up.FetchSliderPuzzle(context.Background(),
		NewClientProfile(PlatformWindows, "DEV-1"), "tid-1", "19900000000")
	if cerr != nil {
		t.Fatalf("失败: %v", cerr)
	}
	if puzzle.BackgroundB64 != "BG" || puzzle.PuzzleB64 != "PZ" {
		t.Errorf("图片实得 %+v", puzzle)
	}
	if puzzle.PicWidth == nil || *puzzle.PicWidth != 680 {
		t.Errorf("图宽上游给的是**字符串**，应被认成数字：%v", puzzle.PicWidth)
	}
	// 服务端把答案直接给了出来：解出来就是拖动坐标
	answer, cerr := puzzle.Answer()
	if cerr != nil {
		t.Fatalf("解缺口坐标失败: %v", cerr)
	}
	if answer != "370" {
		t.Errorf("实得 %q", answer)
	}
}

// livePuzzleLeftCipher 取黄金向量里那条真实 `puzzleLeft`。
func livePuzzleLeftCipher(t *testing.T) string {
	t.Helper()
	v := pcVectors(t)
	c := v["puzzleLeftDecrypt"].([]any)[0].(map[string]any)
	return vectorString(t, c["cipherB64"])
}

func hasPrefix(s, prefix string) bool {
	return len(s) >= len(prefix) && s[:len(prefix)] == prefix
}
