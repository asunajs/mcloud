// 建会话：换 SSO token → 取服务端时间戳 → `tyrzLogin` 换 jwtToken。
//
// 对应 Rust 侧的 app_signin/session.rs。
//
// # 为什么这三条要放在一起
//
// 它们不是三个独立接口，是**一条链**：没有 ③ 的 `jwttoken`，
// 后面所有签到请求一律回 `90001 未登录`。拆成三个"想到谁调谁"的函数，
// 漏掉中间一步的表现是"每个签到接口都报未登录" —— 看上去像鉴权坏了，
// 实际是会话没建。所以三条并排写在这里，调用方看得见整条链。
package upstream

import (
	"encoding/json"
	"fmt"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 第 1 段：换 SSO token（原生，A 族头）
// ─────────────────────────────────────────────────────────────────────────────

// SigninPageURL 是签到页的 H5 地址（WebView 打开的落地页，也是后续请求的 `referer`）。
//
// 真实客户端把 SSO token 直接拼在查询串里交给 WebView —— H5 自己拿它去
// `tyrzLogin` 建会话。纯协议实现里这个 URL 只用来当 `referer`。
func SigninPageURL(ssoToken string) string {
	return fmt.Sprintf("%s/portal/mobilecloud/index.html?path=newsignin&sourceid=%s"+
		"&enableShare=1&appID=%s&token=%s&targetSourceId=%s",
		BaseMcloud, SourceID, H5AppID, ssoToken, ToSourceID)
}

// QuerySpecTokenBody 是 `querySpecTokenV2` 的请求体。
func QuerySpecTokenBody(toSourceID string) map[string]any {
	return map[string]any{"toSourceId": toSourceID}
}

// QuerySpecTokenRequest 构造 `POST /user/querySpecTokenV2`（**明文**）。
//
// # 头集合：用登录链路那一套，不是抓包里那一套
//
// 抓包原样是**更精简**的一组头（有 `origin: mcloudlocal://yun.139.com`、
// `x-mm-source`、`x-yun-tid`、`x-deviceinfo`，且**没有** `x-yun-uni`）。
// 实测两套都能过（2026-09-19 账号 B 用这里这套跑通），所以直接复用
// [AppProfile.Headers] —— 一套头只维护一处。
//
// `timestamp` 是**微秒**级（`x-yun-user-timestamp` 是 16 位）。
func QuerySpecTokenRequest(
	profile *AppProfile, timestamp int64, authorization string, body any,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/user/querySpecTokenV2", BaseUserYun)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithAppHeaders(request, profile, timestamp).
		WithHeader("Authorization", authorization), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 2 段：服务端时间戳 + 签名
// ─────────────────────────────────────────────────────────────────────────────

// H5TimestampRequest 构造 `GET /ycloud/api/timestamp`（**无鉴权**）。
//
// 它给的是 13 位毫秒。签名里的 `x-timestamp` 必须与它逐字相同，
// 所以**不能**用本地时钟替代 —— 本地时钟偏一点就是 `401`。
func H5TimestampRequest() core.HTTPRequest {
	return core.NewRequestGet(fmt.Sprintf("%s/ycloud/api/timestamp", BaseMcloud)).
		WithHeader("Accept", "*/*")
}

// H5Signature 是一次 H5 请求的签名头（四个字段一一对应四个请求头）。
type H5Signature struct {
	// RequestID 是 `x-request-id`：UUID v4。
	RequestID string
	// Timestamp 是 `x-timestamp`：取自 [ParseServerTimestamp]，13 位毫秒的字符串形式。
	Timestamp string
	// Nonce 是 `x-nonce`：UUID v4。
	Nonce string
	// Signature 是 `x-signature`：小写 hex 的 MD5。
	Signature string
}

// SignH5RequestWith 用**给定的** requestID / nonce 算签名（纯函数，黄金向量走这条）。
//
// 算式：`md5(盐 + requestID + timestamp + nonce + "" + 盐)`，
// 其中盐 = `sekaMdYYLIZfbCfm`、最后那段 `""` 是 H5 留的预留位（实测恒为空串）。
//
// ⚠ **它不是 B 族那套签名**（不是 `signature` / `encodeData`），
// 但与 PC 端签名**同一个盐**，所以直接复用 [core.GenerateSignature]。
func SignH5RequestWith(requestID, timestamp, nonce string) H5Signature {
	return H5Signature{
		RequestID: requestID,
		Timestamp: timestamp,
		Nonce:     nonce,
		Signature: core.GenerateSignature(requestID, timestamp, nonce, H5SignSaltKey, ""),
	}
}

// SignH5Request 现生成 requestID / nonce 并签名。
//
// `rng` 注入而不是内部取全局随机数：这样整条链路在测试里可复现。
func SignH5Request(rng core.Random, serverTimestamp string) H5Signature {
	return SignH5RequestWith(core.UUIDv4(rng), serverTimestamp, core.UUIDv4(rng))
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 3 段：建会话（tyrzLogin）
// ─────────────────────────────────────────────────────────────────────────────

// TyrzLoginBody 是 `tyrzLogin` 的请求体。
//
// ⚠ 字段**按 JSON 键的字典序**声明（marketName → openAccount → sourceId → token），
// 与 serde_json 的默认输出顺序一致 —— 理由见 [ThirdLoginBody] 那处说明。
func TyrzLoginBody(ssoToken string) map[string]any {
	return map[string]any{
		"marketName":  MarketName,
		"openAccount": 0,
		"sourceId":    SourceID,
		"token":       ssoToken,
	}
}

// TyrzLoginRequest 构造 `POST /ycloud/auth-service/auth/tyrzLogin`（**带签名**）。
//
// 成功时响应 `result.token` 就是后面要带的 `jwttoken`（同时服务端也会
// `Set-Cookie` 下发 `jwtToken` / `userDomainId` —— 纯协议实现不需要 cookie jar）。
func TyrzLoginRequest(
	sign H5Signature, referer string, body any,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/auth-service/auth/tyrzLogin", BaseMcloud)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	// `jwt` 传 nil：这一步正是**为了**拿 jwt，所以还没有。
	request = WithH5Headers(request, referer, nil, "true")
	return request.
		WithHeader("Origin", BaseMcloud).
		WithHeader("x-request-id", sign.RequestID).
		WithHeader("x-timestamp", sign.Timestamp).
		WithHeader("x-nonce", sign.Nonce).
		WithHeader("x-signature", sign.Signature), nil
}

// H5Login 是 `tyrzLogin` 成功后的会话凭据。
type H5Login struct {
	// JWTToken 是后面所有签到请求要带的 `jwttoken`。
	JWTToken string
	// Mobile 是响应里回显的手机号（用于确认签的是哪个账号）。可能缺失。
	Mobile *string
}

// ParseSSOToken 解析 `querySpecTokenV2` 的响应，取出 SSO token。
func ParseSSOToken(envelope *Envelope) (string, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return "", err
	}
	if payload == nil {
		return "", core.NewParse("querySpecTokenV2 响应没有 data")
	}
	return RequireStr(payload, "token")
}

// ParseServerTimestamp 解析 `GET /ycloud/api/timestamp`，取出**字符串形式**的 13 位毫秒。
//
// 转成字符串是因为它下一步要原样进 `x-timestamp` 头与签名 ——
// 中途过一遍数字类型只会多一次"格式化可能不一致"的机会。
func ParseServerTimestamp(envelope *Envelope) (string, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return "", err
	}
	switch v := payload.(type) {
	case json.Number:
		return v.String(), nil
	case string:
		return v, nil
	case int64:
		return strconv.FormatInt(v, 10), nil
	case float64:
		return strconv.FormatInt(int64(v), 10), nil
	}
	return "", core.NewParse("api/timestamp 的 result 不是整数")
}

// ParseJWTToken 解析 `tyrzLogin` 的响应。
func ParseJWTToken(envelope *Envelope) (H5Login, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return H5Login{}, err
	}
	if payload == nil {
		return H5Login{}, core.NewParse("tyrzLogin 响应没有 result")
	}
	token, err := RequireStr(payload, "token")
	if err != nil {
		return H5Login{}, err
	}
	login := H5Login{JWTToken: token}
	// `mobile` 可能缺失 —— 它是"确认签的是哪个账号"用的回显，不是凭据的一部分，
	// 缺了不该让整次登录失败。
	if text, ok := payload.(map[string]any)["mobile"].(string); ok {
		login.Mobile = &text
	}
	return login, nil
}
