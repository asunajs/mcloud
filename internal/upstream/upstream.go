// 接口调用上下文与请求构造 —— 对应 Rust 侧
// ../mcloud-rust/crates/mcloud-upstream/src/lib.rs 的 `Upstream` 与 login.rs 的请求构造段。
//
// # 为什么"能发请求"是**注入**进来的
//
// 本层只携带一个 [core.HTTPClient]，不携带存储/时间。
// 这样写的好处是 —— 上游层永远无法偷偷读文件或写数据库，
// 它的可测性由类型保证而不是靠约定。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// Upstream 是接口调用上下文：只携带"能发请求"的能力。
type Upstream struct {
	http core.HTTPClient
}

// NewUpstream 用注入的客户端构造调用上下文。
func NewUpstream(client core.HTTPClient) *Upstream { return &Upstream{http: client} }

// Call 发请求并解析成信封。
//
// **业务失败不算错误**：非 2xx 是错误（这时响应体通常不是业务 JSON），
// 2xx 一律解析出信封交回调用方 —— 原实现也是这个语义，
// 调用方需要自己看 `code`（用 [Envelope.EnsureOK] 或本包的状态判定函数）。
func (u *Upstream) Call(ctx context.Context, request core.HTTPRequest) (*Envelope, *core.Error) {
	response, err := u.http.Send(ctx, request)
	if err != nil {
		return nil, asCoreError(err, "发送请求失败")
	}
	if !response.IsSuccess() {
		return nil, errorFromStatus(response.Status)
	}
	return ParseEnvelope(response.Body)
}

// Raw 发请求并返回**原始响应**（不解析信封）。
//
// # 为什么需要它
//
// [Call] 一定走 [ParseEnvelope]，而那是给 JSON 用的。目前唯一的调用方是
// **小红书主页**（HTML）—— 喂给它 `ParseEnvelope` 只会得到一句
// "响应不是 JSON"，而那句错误会掩盖真正的问题（页面被风控挡了、或者
// 拿到了一个空壳页）。
//
// 非 2xx 仍然是错误，与 [Call] 同一口径：那时响应体通常也不是我们要的东西。
func (u *Upstream) Raw(ctx context.Context, request core.HTTPRequest) (core.HTTPResponse, *core.Error) {
	response, err := u.http.Send(ctx, request)
	if err != nil {
		return core.HTTPResponse{}, asCoreError(err, "发送请求失败")
	}
	if !response.IsSuccess() {
		return core.HTTPResponse{}, errorFromStatus(response.Status)
	}
	return response, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求构造
// ─────────────────────────────────────────────────────────────────────────────

// WithClientHeaders 给请求补上"端"头与每次新的 `x-yun-tid`（UUID，幂等 / 链路追踪）。
//
// # 为什么必须有"端"头
//
// 实测：只带 `x-yun-tid` 调 `/user/thirdlogin` 会被上游回 `9999 登录失败！`，
// 带上 [ClientProfile] 那几个设备头之后才是正常的 `200059541`。
// 所以这不是可选的装饰，少了它连第一步都进不去。
//
// `tid` 由调用方传入而不是在这里生成：生成随机数需要 [core.Random]，
// 而请求构造函数保持纯函数才能被逐字段断言。
func WithClientHeaders(request core.HTTPRequest, profile *ClientProfile, tid string) core.HTTPRequest {
	for _, h := range profile.Headers() {
		request = request.WithHeader(h[0], h[1])
	}
	return request.WithHeader("x-yun-tid", tid)
}

func postJSON(url string, body any) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(url).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return request, nil
}

// ThirdLoginRequest 构造 `POST /user/thirdlogin`（扫码 / 短信 / 账密共用）。
func ThirdLoginRequest(body any) (core.HTTPRequest, *core.Error) {
	return postJSON(fmt.Sprintf("%s/user/thirdlogin", BaseUserYun), body)
}

// SendSmsCodeRequest 构造 `POST /user/sms/getSmsCode`。
func SendSmsCodeRequest(body any) (core.HTTPRequest, *core.Error) {
	return postJSON(fmt.Sprintf("%s/user/sms/getSmsCode", BaseUserYun), body)
}

// SliderPuzzleRequest 构造 `POST /user/verfycode`（取滑块图）。
//
// ⚠ 路径**没有 `i`**：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的。
func SliderPuzzleRequest(body any) (core.HTTPRequest, *core.Error) {
	return postJSON(fmt.Sprintf("%s/user/verfycode", BaseUserYun), body)
}

// LogoutRequest 构造 `POST /user/logout`。
func LogoutRequest(body any) (core.HTTPRequest, *core.Error) {
	return postJSON(fmt.Sprintf("%s/user/logout", BaseUserYun), body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录链路的发送
// ─────────────────────────────────────────────────────────────────────────────

// ThirdLogin 是登录总入口（扫码 / 短信 / 账密共用）。
//
// **业务失败不算错误**：轮询期间的"未扫码""已扫码待确认"都是正常中间态，
// 由调用方用 [ClassifyPoll] 判定，所以这里只返回信封。
func (u *Upstream) ThirdLogin(
	ctx context.Context, profile *ClientProfile, tid string, body any,
) (*Envelope, *core.Error) {
	request, err := ThirdLoginRequest(body)
	if err != nil {
		return nil, err
	}
	return u.Call(ctx, WithClientHeaders(request, profile, tid))
}

// SendSmsCode 发送短信验证码。
func (u *Upstream) SendSmsCode(
	ctx context.Context, profile *ClientProfile, tid string, body any,
) (*Envelope, *core.Error) {
	request, err := SendSmsCodeRequest(body)
	if err != nil {
		return nil, err
	}
	return u.Call(ctx, WithClientHeaders(request, profile, tid))
}

// SliderPuzzle 获取滑块拼图（返回信封，调用方用 [ParseSliderPuzzle] 解）。
func (u *Upstream) SliderPuzzle(
	ctx context.Context, profile *ClientProfile, tid string, body any,
) (*Envelope, *core.Error) {
	request, err := SliderPuzzleRequest(body)
	if err != nil {
		return nil, err
	}
	return u.Call(ctx, WithClientHeaders(request, profile, tid))
}

// FetchSliderPuzzle 取滑块图（一次调用把"发请求 + 解出两张图"走完）。
func (u *Upstream) FetchSliderPuzzle(
	ctx context.Context, profile *ClientProfile, tid, phone string,
) (*SliderPuzzle, *core.Error) {
	envelope, err := u.SliderPuzzle(ctx, profile, tid, NewSliderPuzzleBody(phone))
	if err != nil {
		return nil, err
	}
	return ParseSliderPuzzle(envelope)
}

// Logout 退出登录。
//
// **尽力而为**：实测客户端是"先清本地 token，再发 logout"，服务端因此常回
// `01000001 用户token不能为空` —— 这不是失败，是正常顺序的结果。
// 所以调用方**不要**因为它的错误码而中断退出流程（该语义在 capabilities 层落实）。
func (u *Upstream) Logout(
	ctx context.Context, profile *ClientProfile, tid string, body any,
) (*Envelope, *core.Error) {
	request, err := LogoutRequest(body)
	if err != nil {
		return nil, err
	}
	return u.Call(ctx, WithClientHeaders(request, profile, tid))
}
