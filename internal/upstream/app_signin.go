// 移动云盘 **App 端签到（云朵中心）** —— H5 链路的上游接口层。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-upstream/src/app_signin/。
//
// # 为什么这一层看起来"不像一个 App 接口"
//
// 因为**签到根本不是一个原生接口，是一个 H5 页面**（`newsignin`）。
// 原生 App 只做两件事：换一个 SSO token、把 WebView 打开到 `m.mcloud.139.com`。
// 真正的签到动作全由 H5 发出 —— 所以本文件要同时说两种"客户端语言"：
//
//	原生（A 族头，Authorization + 设备头）            H5（自有头，jwttoken）
//	① POST /user/querySpecTokenV2  ──→ SSO token
//	② GET  /ycloud/api/timestamp   ──→ 服务端毫秒时间戳（签名要用）
//	③ POST /ycloud/auth-service/auth/tyrzLogin ──→ jwtToken（≈905 字符，2 小时）
//	④ GET  /ycloud/signin/page/infoV3?client=app        ← 签到状态，**权威判据**
//	⑤ GET  /ycloud/signin/page/startSignIn?client=app   ← 执行签到
//	⑥ POST /ycloud/signin/page/receiveV3                ← 领云朵（**必须带 deviceId**）
//	⑦ POST /ycloud/signin/page/receiveRevivalReward     ← 用复活卡/回馈卡（**不带 deviceId**）
//
// ①②③ 是"建会话"，④⑤⑥⑦ 是"签到 + 领取"。**没有 ③ 就没有 ④⑤⑥⑦** ——
// 少了 `jwttoken` 头，它们一律回 `90001 未登录`。
//
// 本文件只落地 ①②③（建会话）与这一族共用的常量 / 头 / 解析helper；
// ④⑤⑥⑦ 各成一族，等建会话这层接上之后再来。
//
// # 规格来源
//
// docs/capture/app-signin-samples.txt（2026-09-19 实测抓包 + 账号 B 纯协议端到端复现）。
// 黄金向量：test/golden/app-signin-vectors.json。
package upstream

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（全部来自抓包，不是猜的）
//
// ⚠ 这些取值由 test/golden/app-signin-vectors.json 的 `constants` 段钉住：
// 改错一个不会报错，只会让整条链路变成"另一个客户端"。
// ─────────────────────────────────────────────────────────────────────────────

// MarketName 是签到活动的市场名，同时是请求头 `activityid` 的取值。
const MarketName = "sign_in_3"

// SourceID 是签到页的来源 id（URL 的 `sourceid` / `tyrzLogin` 体的 `sourceId`）。
const SourceID = "1002"

// H5AppID 是 H5 页面的 `appID`（页面 URL 上那个固定串，不是客户端 id）。
const H5AppID = "cy62d5a5e8373a272aeca467accad61722c51cfd48"

// H5AppVersion 是 H5 请求头 `appversion`。
//
// 注意它与 [IOSAppVersion]（`12.6.0`）**不是一个值**：这里带第 4 段（`12.6.0.0`）。
const H5AppVersion = "12.6.0.0"

// TaskClientVersion 是 `taskListV3` 请求体里的 `clientVersion`（不带第 4 段）。
const TaskClientVersion = "12.6.0"

// Client 是 `startSignIn` / `infoV3` 的查询参数 `client`。
//
// 抓包实测是字面量 `app`（**不是数字**）—— 别照 `taskListV3` 的 `client:1` 抄。
const Client = "app"

// ToSourceID 是 `querySpecTokenV2` 请求体里 `toSourceId` 的取值
// （**本功能选的**，不是"App 端的"）。
//
// ⚠⚠ **它不是端常量，是调用方按场景选的参数。** 同一个 App 进程里就出现过三个值
// （`001003` / `001005` / `010001`），按发起方不同而变。这里取 `001005`，
// 依据是**签到页 URL 自带的 `targetSourceId=001005`**（App 实拍）。
//
// 别拿别处的值来换它：
//   - `apiServer.js` 里 `getCloudTicket` 的默认值是 `001003` —— 那是 **PC 客户端**的包；
//   - 签到会话里那条 **WebKit** 发起的 `querySpecTokenV2` 也确实用了 `001003`，
//     但发起方不是签到页本身（同一条接口，原生与 H5 两个发起方的头、参数都不同）。
//
// "两边实测都能过"只说明**服务端在这条路径上不校验**，不等于可以互换 ——
// 上游不校验的字段一旦哪天开始校验，错的那个是**静默失效**，不会报错。
const ToSourceID = "001005"

// H5SignSaltKey 是 H5 签名用的盐值编号 —— core 的 signSaltMap 里的 `sekaMdYYLIZfbCfm`。
//
// 单列成常量而不是在调用点写 `2`：签名盐表哪天被改动，
// 这里会有一条测试直接失败，而不是变成"签到忽然回 401"。
const H5SignSaltKey uint8 = 2

// 业务码。
const (
	// CodeOK 是业务成功码。
	CodeOK int64 = 0
	// CodeNotLoggedIn 是 `90001 未登录` —— `jwttoken` 没带上，或会话没建起来。
	CodeNotLoggedIn int64 = 90001
	// CodeSSOExpired 是 `200050409 token 已失效` —— SSO token 过期。
	//
	// 它**很短命**：实测 14:38:54 换到的 token，14:42:09 重放就被拒。
	// 换完立刻用，不要缓存。
	CodeSSOExpired int64 = 200050409
	// CodeSignatureRejected 是 `401 请求签名校验失败` ——
	// 签名算式 / 盐 / 时间戳任一处不对。
	CodeSignatureRejected int64 = 401
	// CodeReceiveTooBusy 是 `11029` —— 上游"活动太火爆"。
	//
	// 它**不是**"参数错了"：请求被正常处理了，只是这次没给。可以退避后重试。
	CodeReceiveTooBusy int64 = 11029

	// CodeReceiveFailedAlt 是 `615`：实测这个码就是**没带 H5 的 deviceId**。
	//
	// # 614 / 615 的语义都已实证（2026-09-19，账号 B）
	//
	// 同一份 `receiveV3` 请求，只换 `deviceId`：
	//
	//	不带                       → 615，msg「…兑换失败…」
	//	长度对、内容错一个字符     → 614，msg「…锁定失败…」
	//	真实值                     → 0
	//
	// ⚠ **上游的 msg 会把人带偏**：它开口就是"活动太火爆"，读起来像限流、
	// 像该退避重试，而真实原因是**参数缺失或参数无效** —— 重试一万次也没用。
	// 所以别照着 msg 做退避逻辑，先查 `deviceId`。
	//
	// ⚠ 这同时说明**服务端校验的是 `deviceId` 的内容，不只是"非空"**：
	// 一个长度合法、只错一个字符的 base64 照样被拒。所以"随便给个非空值"
	// 这条路是堵死的 —— 必须拿到真值。
	CodeReceiveFailedAlt int64 = 615
	// CodeReceiveFailed 是 `614`：实测这个码就是**H5 的 deviceId 内容无效**。
	CodeReceiveFailed int64 = 614
)

// Diagnose 给出已知业务码的一句话排查提示；`(false)` 表示"没有已知提示"。
//
// ⚠ `614` / `615` 的 msg 与 `11029` 长得几乎一样（都在说"火爆"），
// 但成因完全不同：前两者是**参数问题**（重试无用），后者才是真的忙。
// 别按 msg 归类 —— 这条函数的存在就是为了不让人去读 msg。
func Diagnose(code int64) (string, bool) {
	switch code {
	case CodeNotLoggedIn:
		return "jwttoken 没带上，或 tyrzLogin 还没跑（会话没建起来）", true
	case CodeSSOExpired:
		return "SSO token 已失效 —— 它很短命，换完必须立刻用", true
	case CodeSignatureRejected:
		return "签名不对：盐 / 算式 / x-timestamp（必须取自 /ycloud/api/timestamp）", true
	case CodeReceiveTooBusy:
		return "上游「活动太火爆」—— 不是参数错，退避后重试", true
	case CodeReceiveFailedAlt:
		return "领取失败 —— 实测这个码就是「没带 H5 的 deviceId」；上游 msg 说的「兑换失败」是误导", true
	case CodeReceiveFailed:
		return "领取失败 —— 实测这个码就是「H5 的 deviceId 内容无效」；上游 msg 说的「锁定失败」是误导", true
	}
	return "", false
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求头
// ─────────────────────────────────────────────────────────────────────────────

// H5ContentType 是 H5 请求实际发出的 content-type。
//
// ⚠ 它**带 charset**，而 [core.HTTPRequest.WithJSONBody] 补的是不带的那个。
// 所以凡是要发 body 的 H5 / App 原生请求，都要**先剥后补**（见 [replaceHeader]）——
// 直接再追加一个同名头会发出两个 `Content-Type`，
// 上游按前一个解析（实测过的坑，拼成了 `application/json, application/json;charset=UTF-8`）。
const H5ContentType = "application/json;charset=UTF-8"

// dropHeader 删掉所有同名头（大小写不敏感）。
//
// # 为什么必须有它
//
// HTTP 头是**同名可重复**的列表；Go 侧 [core.HTTPRequest.WithHeader] 只会追加。
// 于是"覆盖一个头"在 Go 里默认做不到，而"发出两个 content-type"的表现是
// **上游按第一个解析** —— 请求看起来完全正常，只有 charset 那部分被丢掉，
// 于是所有带中文的 body 会在某一天开始乱码。
//
// 所以覆盖一个头的写法是：**先 dropHeader，再 WithHeader**。
func dropHeader(headers []core.HeaderPair, name string) []core.HeaderPair {
	kept := make([]core.HeaderPair, 0, len(headers))
	for _, h := range headers {
		if strings.EqualFold(h[0], name) {
			continue
		}
		kept = append(kept, h)
	}
	return kept
}

// WithH5Headers 给请求补上 H5 的公共头。
//
// `jwt` 为 nil 表示"还没建会话"（只有 `tyrzLogin` 那一条是这样）。
// `showLoading` 是纯 UI 提示（H5 的 loading 蒙层），照抄是为了与真实客户端
// 逐字一致，服务端不据此做任何判断。
//
// ⚠ 剥掉 content-type 之后**必须按 body 补回来**：剥了不补 = 所有 H5 POST 裸发。
// Rust 侧这里就漏过一次（`h5_json` 在剥离之前调用，补的又被剥掉），
// 是被"请求体与抓包一致"那条断言抓出来的。现在 content-type 只在本函数里产生，
// 不再有"谁先谁后"的问题。
func WithH5Headers(request core.HTTPRequest, referer string, jwt *string, showLoading string) core.HTTPRequest {
	request.Headers = dropHeader(request.Headers, "content-type")
	if len(request.Body) > 0 {
		request = request.WithHeader("Content-Type", H5ContentType)
	}
	request = request.
		WithHeader("Accept", "*/*").
		WithHeader("activityid", MarketName).
		WithHeader("appversion", H5AppVersion).
		WithHeader("Referer", referer).
		WithHeader("showloading", showLoading)
	if jwt != nil {
		// 头名是小写 `jwttoken`（不是 `jwtToken`）—— H5 拦截器原样如此。
		request = request.WithHeader("jwttoken", *jwt)
	}
	return request
}

// WithAppHeaders 给请求补上 App 原生那套头（供 `querySpecTokenV2` 等原生请求用）。
//
// 与 [WithH5Headers] 同：先把 content-type 删干净再补带 charset 的那个。
func WithAppHeaders(request core.HTTPRequest, profile *AppProfile, timestamp int64) core.HTTPRequest {
	request.Headers = dropHeader(request.Headers, "content-type")
	for _, h := range profile.Headers(timestamp) {
		request = request.WithHeader(h[0], h[1])
	}
	return request.WithHeader("Content-Type", H5ContentType)
}

// ─────────────────────────────────────────────────────────────────────────────
// 编码与解析
// ─────────────────────────────────────────────────────────────────────────────

// PercentEncode 是查询串值的百分号编码。
//
// `deviceId` 是 base64，含 `+` `/` `=`。**`+` 在查询串里表示空格** ——
// 不编码就会被服务端解成另一个值（而且看起来"没报错"，只是行为不对）。
// 抓包里这三个字符正是 `%2B` / `%2F` / `%3D`。
func PercentEncode(value string) string {
	var out strings.Builder
	out.Grow(len(value))
	for i := 0; i < len(value); i++ {
		b := value[i]
		switch {
		case b >= 'A' && b <= 'Z', b >= 'a' && b <= 'z', b >= '0' && b <= '9',
			b == '-', b == '.', b == '_', b == '~':
			out.WriteByte(b)
		default:
			fmt.Fprintf(&out, "%%%02X", b)
		}
	}
	return out.String()
}

// RequireStr 从已解码的 payload 里取一个必填字符串字段。
//
// # 为什么不用"反序列化整包"
//
// 这些响应里有十几处"服务端有时给、有时不给"的字段；
// 用结构体 + 默认值会把"其实没给"静默成默认值 ——
// 而这里恰恰要靠"字段在不在"判断成败。
//
// ⚠ 只认字符串：上游这些字段从来都是字符串，而"顺手把数字也转过来"
// 会让一次类型变化静默通过 —— 那正是要立刻看见的那类变化。
func RequireStr(payload any, key string) (string, *core.Error) {
	obj, ok := payload.(map[string]any)
	if !ok {
		return "", core.NewParse(fmt.Sprintf("响应 payload 不是对象，取不到 `%s`", key))
	}
	text, ok := obj[key].(string)
	if !ok {
		return "", core.NewParse(fmt.Sprintf("响应缺少字符串字段 `%s`", key))
	}
	return text, nil
}

// RequireInt64 同 [RequireStr]，取整数。
//
// ⚠ [core.ParseJSON] 为了"19 位标识不丢精度"用的是 `json.Number`，
// 所以这里必须**同时**认 `json.Number` 与 `int64` —— 只认后者会让
// "从向量文件读进来的响应体"全部解析失败（那条见方案 §6.1 #34）。
func RequireInt64(payload any, key string) (int64, *core.Error) {
	obj, ok := payload.(map[string]any)
	if !ok {
		return 0, core.NewParse(fmt.Sprintf("响应 payload 不是对象，取不到 `%s`", key))
	}
	switch v := obj[key].(type) {
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return 0, core.NewParse(fmt.Sprintf("响应字段 `%s` 不是整数: %s", key, v.String()))
		}
		return n, nil
	case int64:
		return v, nil
	case float64:
		return int64(v), nil
	}
	return 0, core.NewParse(fmt.Sprintf("响应缺少整数字段 `%s`", key))
}

// EnsureOKWithHint 判业务成败，失败时把 [Diagnose] 的提示附在 detail 里。
//
// **不**把上游原文放进对外文案：`core.Error` 的对外文案走白名单，
// detail 只进日志（理由见 [errorFromStatus]）。
func EnsureOKWithHint(envelope *Envelope) (any, *core.Error) {
	payload, err := envelope.EnsureOK()
	if err == nil {
		return payload, nil
	}
	if code, ok := envelope.Code(); ok {
		if hint, known := Diagnose(code); known {
			wrapped := core.NewBadRequest(fmt.Sprintf("%s（%s）", err.Message(), hint))
			return nil, wrapped
		}
	}
	return nil, err
}
