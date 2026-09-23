// 换 token：`POST /user/auth/refreshToken`（用户域，与登录、`querySpecTokenV2` 同一台主机）。
//
// # 证据分两档，本文件里**不混着写**
//
// | 面 | 证据 | 落点 |
// | --- | --- | --- |
// | App 的请求/响应形状 | **实拍闭环**（`docs/capture/app-rights-token-samples.txt` §4：一次窗口里 24 次调用，两条完整往返） | [AppRefreshTokenRequest] |
// | "新 token 就是 `Authorization` 第三段" | **本轮新证**（同一份 App 日志：刷新响应里的 `token` 编成 `Basic base64("mobile:<手机号>:<token>")` 之后，在同一次会话的后续请求头里逐字命中；次日窗口同样命中） | [ParseRefreshToken] 的说明 |
// | PC 端走不走这个接口 | **零抓包**。PC 桌面客户端的 API 源码里根本没有 refresh 这条（`har/pc-api.js` / `pc-utils.js` 全文无 `refreshToken`）；调用这个接口的是 **H5**（`docs/capture/apiServer.js` 的 `auth.refreshToken(data)`） | [PCRefreshTokenRequest] 显式标注"未实测" |
//
// ⇒ 也就是说 PC 这一支是**按 H5 的形状 + PC 的档案头**拼出来的，
// 它是"最可能的形状"，不是"实测过的形状"，在第一次打通之前 [PCRefreshTokenRequest] 按未验证对待。
// （旧项目把刷新放在每轮任务收尾自动做，见 `docs/auth-refresh.md` §1 ⇒ 这条不是"要不要打"的闸门，
// 只是"PC 那一支的形状还没对上"的提示。）
//
// # 为什么**不**做旧实现那条 `tellin/authTokenRefresh.do`
//
// 旧 TS 的 `after` 任务刷的是 `aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`
// （XML 请求体、content-type 却写 json、响应正则取 `<token>`；见
// `docs/business-feature-inventory.md` §2.2）。本仓库对它**零抓包**，
// 而 App 的流量里也一次都没出现 `authTokenRefresh`。
// 现在既然实测确认 `user/auth/refreshToken` 换的就是 `Authorization` 里那个 authToken，
// 再实现一条"只有二手描述、且属于另一个服务"的路径没有收益：
// 两条都未实测时选有实测的那条，一条已实测时**只**走它。
//
// # 为什么这里不发 `hcy-cool-flag` 信封
//
// 实拍的那次请求**是**加密的（该接口在 192 那把钥匙的 396 条命中列表里）。但本仓库
// 既有的策略是"已登录的业务接口不带信封、走明文"（[QuerySpecTokenRequest] 同一个
// 主机、同一族钥匙，实测明文过：2026-09-19 账号 B 跑通）。
// 所以这里默认明文，并且**记下这是待核验项**：一旦上游回 `9103` 一类"解不开请求体"，
// 正确反应是按 `docs/upstream/app-business-api.md` §8 那三把钥匙补上 AES-192 信封
// （[core.RefreshTokenKey] 就是那把），而不是改判据。
package upstream

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// RefreshTokenPath 是刷新接口的路径（两端同一个， host 也同一个）。
const RefreshTokenPath = "/user/auth/refreshToken"

// RefreshTokenBody 是 `/user/auth/refreshToken` 的请求体。
//
// ⚠ 两个键都**必须有**：`userDomainId` 是"刷哪个用户域里的那个 token"，
// `clientType` 是"我是哪个端"。缺 `userDomainId` 时本端不猜（见 capabilities 层：
// 库里没有用户域就直接判"这个账号刷不了"，编一个会把刷新打到别人的用户域上）。
//
// 字段顺序按**实拍原串**（`userDomainId` 在前）声明；`encoding/json` 按声明顺序输出，
// 与 [ThirdLoginBody] 那处说明同理。
type RefreshTokenBody struct {
	UserDomainID string `json:"userDomainId"`
	ClientType   string `json:"clientType"`
}

// RefreshTokenResult 是响应 `data` 里我们要的两项。
type RefreshTokenResult struct {
	// Token 是新的 authToken —— 实测它就是 `Authorization` 整串解码后的第三段。
	// 形状与登录给的那个一致：`|` 分 5 段 `[8,1,3,13,172]`，第 4 段是 13 位**毫秒**。
	Token string
	// ExpireSeconds 是**相对时长**（实测 `2592000` = 30 天），不是绝对时间戳。
	//
	// ⚠ 单位与 [Token] 里那 13 位**不是一回事**：那一个是"何时过期"，这一个
	// 是"还能用多久"。落库的 `auth_expire` 用前者（与登录路径同一个来源，
	// 都由 [core.ParseAuth] 从整串里取），这里只是**带着**它供排障与断言。
	// 缺失不算失败：token 自己就带过期段。
	ExpireSeconds *int64
}

// RefreshTokenRequest 构造 `POST /user/auth/refreshToken` 的**裸请求**（不含端头）。
func RefreshTokenRequest(body any) (core.HTTPRequest, *core.Error) {
	return postJSON(BaseUserYun+RefreshTokenPath, body)
}

// AppRefreshTokenRequest 构造 App 端的刷新请求。
//
// 头集合直接复用 [AppProfile.Headers]：这个接口比 A 族其余接口多的那两个头
// （`x-yun-api-version: v1`、`x-UserAgent` 第 1 段是 `clientType`）
// **本来就在那一套里**（实测 `412|iPad|ios 26.6|12.6.0-200`），
// 所以不需要为它单开一份 —— 少一处"两份头迟早漂移"。
func AppRefreshTokenRequest(
	profile *AppProfile, timestamp int64, authorization string, body any,
) (core.HTTPRequest, *core.Error) {
	request, err := RefreshTokenRequest(body)
	if err != nil {
		return request, err
	}
	return WithAppHeaders(request, profile, timestamp).
		WithHeader("Authorization", authorization), nil
}

// PCRefreshTokenRequest 构造 PC 端的刷新请求。
//
// ⚠ **未实测**（理由见文件头那张表）。它按 [ClientProfile.Headers] 那套已登录头
// 加 `Authorization` 拼装，`clientType` 由档案的平台给出 ——
// 调用点**不给**这个值，免得"PC 档案配 App 的 412"这种矛盾组合能静默通过
// （那是本仓库记过五次的同类坑）。
func PCRefreshTokenRequest(
	profile *ClientProfile, tid, authorization string, body any,
) (core.HTTPRequest, *core.Error) {
	request, err := RefreshTokenRequest(body)
	if err != nil {
		return request, err
	}
	return WithClientHeaders(request, profile, tid).
		WithHeader("Authorization", authorization), nil
}

// ParseRefreshToken 解析刷新响应，取出新 token。
//
// 成功判据用 [Envelope.EnsureOK]：这个信封是 `success` + `code`（字符串 `"0000"`）
// 并列的那一族 —— `success:false` 优先判失败，`code` 是字符串也要认得
// （[Envelope.Code] 就是为此收字符串）。
//
// # 为什么这里**不**判"新 token 与旧 token 不相等"
//
// 上游在同一次刷新里回同一个 token 是可能的（提前刷新、服务端认为还不需要换）。
// 那是**成功**，不是异常：调用方拿到的仍然是一个可用的凭据。
// 若在这里判"变了才算成功"，"不需要刷"会被翻成一次失败，进而被记成 `fail_streak`。
func ParseRefreshToken(envelope *Envelope) (RefreshTokenResult, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return RefreshTokenResult{}, err
	}
	if payload == nil {
		return RefreshTokenResult{}, core.NewParse("refreshToken 响应没有 data")
	}
	token, err := RequireStr(payload, "token")
	if err != nil {
		return RefreshTokenResult{}, err
	}
	// ⚠ 空串**不是**"上游没给"，而是"给了一个用不了的凭据"。
	// 不在这里拦，下游会把它拼成 `Basic base64("mobile:<手机号>:")` 并**覆盖库里那串还能用的**，
	// 症状是"刷新一次，账号当场掉线"，而且事后看不出来是这次刷新干的。
	if token == "" {
		return RefreshTokenResult{}, core.NewParse("refreshToken 响应里的 token 是空串")
	}
	// 只警告式缺失（不报错）：见 [RefreshTokenResult.ExpireSeconds] 的说明。
	// 走 `OptionalInt64` 而不是 `RequireInt64`，是为了让"上游哪天改成不回了"
	// 表现为一次可诊断的降级，而不是整条刷新链失败。
	result := RefreshTokenResult{Token: token}
	if seconds, ok := optionalInt64(payload, "expireTime"); ok {
		result.ExpireSeconds = &seconds
	}
	return result, nil
}

// AppRefreshToken 是 App 端刷新的"发出去 + 解回来"。
func (u *Upstream) AppRefreshToken(
	ctx context.Context,
	profile *AppProfile,
	timestamp int64,
	authorization, userDomainID string,
) (RefreshTokenResult, *core.Error) {
	request, perr := AppRefreshTokenRequest(
		profile, timestamp, authorization,
		&RefreshTokenBody{UserDomainID: userDomainID, ClientType: IOSClientType})
	if perr != nil {
		return RefreshTokenResult{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return RefreshTokenResult{}, perr
	}
	return ParseRefreshToken(envelope)
}

// PCRefreshToken 是 PC 端刷新的"发出去 + 解回来"。
//
// ⚠ 未实测（见 [PCRefreshTokenRequest]）。
func (u *Upstream) PCRefreshToken(
	ctx context.Context,
	profile *ClientProfile,
	tid, authorization, userDomainID string,
) (RefreshTokenResult, *core.Error) {
	request, perr := PCRefreshTokenRequest(
		profile, tid, authorization,
		&RefreshTokenBody{
			UserDomainID: userDomainID,
			ClientType:   profile.Platform().ClientType(),
		})
	if perr != nil {
		return RefreshTokenResult{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return RefreshTokenResult{}, perr
	}
	return ParseRefreshToken(envelope)
}

// optionalInt64 取可选整数字段：键不存在、或值不是整数都返回 false。
//
// 与 [RequireInt64] 分开写，是因为"缺失"在这条接口上是**允许**的，
// 用 Require 再吞掉错误会让"上游把字段改名了"和"上游这次没回"长得一样。
func optionalInt64(payload any, key string) (int64, bool) {
	obj, ok := payload.(map[string]any)
	if !ok {
		return 0, false
	}
	value, exists := obj[key]
	if !exists {
		return 0, false
	}
	n, ok := asInt64(value)
	return n, ok
}
