// 签到主体：`infoV3`（权威判据）/ `startSignIn`（执行签到）/ `doTaskPost`（上报任务状态）。
//
// 对应 Rust 侧的 app_signin/sign_in.rs。
//
// ⚠ 这几条接口**全部需要 `jwttoken` 头** —— 没有建会话（[TyrzLoginRequest]）
// 就调它们，一律回 `90001 未登录`。
//
// 任务清单（`taskListV3`）在 `app_task_list.go` —— 它现在是一条独立能力，
// 字段表与证据都归到那一页去维护了。
package upstream

import (
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// InfoV3Request 构造 `GET /ycloud/signin/page/infoV3?client=app` —— **签到状态，权威判据**。
func InfoV3Request(jwt, referer string) core.HTTPRequest {
	return WithH5Headers(
		core.NewRequestGet(fmt.Sprintf("%s/ycloud/signin/page/infoV3?client=%s", BaseMcloud, Client)),
		referer, &jwt, "true")
}

// StartSignInRequest 构造 `GET /ycloud/signin/page/startSignIn?client=app` —— 执行签到。
//
// # `deviceID`
//
// 真实客户端会带 `deviceId` 查询参数（来自 iOS `SMSdk` bridge）+ `isdeviceid: true` 头。
// **实测不带也能签上**（账号 B，`{"code":0,"msg":"success"}`），所以它是可选的。
//
// 但传了就会**同时**补上 `isdeviceid: true` 头：抓包里两者总是成对出现，
// 只发一个等于自相矛盾（H5 的拦截器就是按那个头决定要不要注入参数）。
//
// ⚠ 查询串里的 `deviceId` 必须**百分号编码**：它是 base64，含 `+` / `/` `=`，
// 而 `+` 在查询串里表示空格（见 [PercentEncode]）。
func StartSignInRequest(jwt, referer string, deviceID *string) core.HTTPRequest {
	url := fmt.Sprintf("%s/ycloud/signin/page/startSignIn?client=%s", BaseMcloud, Client)
	if deviceID != nil {
		url = fmt.Sprintf("%s&deviceId=%s", url, PercentEncode(*deviceID))
	}
	request := WithH5Headers(core.NewRequestGet(url), referer, &jwt, "false")
	if deviceID != nil {
		request = request.WithHeader("isdeviceid", "true")
	}
	return request
}

// DoTaskPostRequest 构造 `POST /ycloud/signin/page/doTaskPost` —— 上报任务状态。
//
// `deviceID` 是**必填**（不是可选）：这个接口在抓包里标了 `isDeviceId: true`，
// 且同族的领取接口实测**不带就报错**。让漏发变成编译期错误，
// 而不是运行时的上游报错。
func DoTaskPostRequest(jwt, referer, deviceID string) (core.HTTPRequest, *core.Error) {
	body := map[string]any{"client": Client, "deviceId": deviceID}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/page/doTaskPost", BaseMcloud)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithH5Headers(request, referer, &jwt, "true").
		WithHeader("isdeviceid", "true"), nil
}

// SignInResult 是 `startSignIn` 的解析结果。
//
// ⚠ [SignInResult.TodaySignIn] 是**调用前**的状态，不是成败判据。
// 实测（账号 B）：响应里 `todaySignIn:false`、`cal` 里 19 号还是 `s:false`，
// 但紧接着的 `infoV3` 已经是 `s:true` —— 那一次确实签上了。
type SignInResult struct {
	// TodaySignIn 是调用**前**今天是否已签。
	//
	// ⚠ 它是**快照**：`false` 恰恰表示"这次把今天签上了"。
	// 判成败一律回查 [ParseSignInStatus]（见方案与项目备忘里的那条铁律）。
	TodaySignIn bool
	// SignInPoints 是本次签到可得云朵数。
	SignInPoints int64
	// SignCount 是连续签到天数。
	SignCount int64
	// ServerTimeMS 是服务端时间（毫秒），可能缺失。
	ServerTimeMS *int64
}

// ParseStartSignIn 解析 `startSignIn` 的响应。
func ParseStartSignIn(envelope *Envelope) (SignInResult, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return SignInResult{}, err
	}
	if payload == nil {
		return SignInResult{}, core.NewParse("startSignIn 响应没有 result")
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return SignInResult{}, core.NewParse("startSignIn 的 result 不是对象")
	}
	// `todaySignIn` 只认真正的布尔：上游把它变成 `1` / `0` 时要立刻响，
	// 而不是"顺手当成 true" —— 那会让"今天到底签没签"这个判断悄悄反掉。
	today, ok := obj["todaySignIn"].(bool)
	if !ok {
		return SignInResult{}, core.NewParse("startSignIn 响应缺少布尔字段 `todaySignIn`")
	}
	out := SignInResult{TodaySignIn: today}
	for _, spec := range []struct {
		key string
		dst *int64
	}{
		{"signInPoints", &out.SignInPoints},
		{"signCount", &out.SignCount},
	} {
		v, err := RequireInt64(payload, spec.key)
		if err != nil {
			return SignInResult{}, err
		}
		*spec.dst = v
	}
	if v, ok := intAtOK(obj, "time"); ok {
		out.ServerTimeMS = &v
	}
	return out, nil
}

// 任务清单的解析（`result` 是数组那一族）在 `app_task_list.go` 的 [ParseTaskList]。
