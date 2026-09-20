// 上游响应信封与接口基址 —— 对应 Rust 侧 legacy/crates/mcloud-upstream/src/lib.rs。
//
// # 状态
//
// `login` 已落（见 login.go）；其余云盘业务接口等重新抓包后再写，
// **不搬旧 TS**（旧 TS 与线上请求有多处不一致，见 docs/status.md）。
//
// # ⚠ 本层是**纯层**：不发起真实请求，也不持有 HTTP 客户端
//
// 发请求的能力由调用方注入（等 core 的 traits 落地后接上）。
// 于是上游层永远无法偷偷读文件或写数据库 —— 它的可测性不靠约定。
package upstream

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// 上游接口基址（自旧实现整理，供重写时对照；本层不发起真实请求）。
const (
	// BaseMcloudSignin 是主站：签到、兑换、用户域、认证。
	BaseMcloudSignin = "https://m.mcloud.139.com/ycloud/signin"
	// BaseMcloud 是主站根。
	BaseMcloud = "https://m.mcloud.139.com"
	// BaseCaiyun 是彩云主入口（消息推送、备份礼物、老签到入口）。
	BaseCaiyun = "https://caiyun.feixin.10086.cn"
	// BaseCaiyunYcloud 是彩云 `ycloud/*` 入口。
	//
	// **端口不能省**：真实流量里 `ycloud/signin/...` 走的是 `:7071`，
	// 而 TS 里同一批接口写的是不带端口的 `/market/signin/...`。两者是不同入口，
	// 混用会打到错误的地址上。
	BaseCaiyunYcloud = "https://caiyun.feixin.10086.cn:7071"
	// BaseMrpMcloud 是活动资源（任务扩展）。
	BaseMrpMcloud = "https://mrp.mcloud.139.com"
	// BaseUserYun 是用户域（token 刷新、spec token、登录）。
	BaseUserYun = "https://user-njs.yun.139.com"
	// BaseNoteMcloud 是笔记服务。
	BaseNoteMcloud = "https://note.mcloud.139.com"
	// BaseAasCaiyun 是认证服务。
	BaseAasCaiyun = "https://aas.caiyun.feixin.10086.cn"
	// BaseMail139 是 139 邮箱。
	BaseMail139 = "https://mail.10086.cn"
)

// CaiyunAppVersion 是客户端版本号，对齐 `@asunajs/shared/constants.ts` 的
// `CAIYUN_APP_VERSION`。
//
// 它是**业务参数**而不是展示信息：`exchangeList` / `exchangeV2` 都会带上它，
// 改这个值等于把自己伪装成另一个版本的客户端（上游的开关与灰度常常按它分流）。
const CaiyunAppVersion = "12.6.0"

// Envelope 是上游统一响应信封。
//
// # 为什么不做成"万能结构"
//
// 各接口的外层字段名并不统一：`{code, msg, result}`、`{code, msg, data}`、
// `{success, code, message, data}`（`auth.ts` 的 `RefreshTokenResponse`）都有，
// 有些接口（`taskListV2`）还带回 `groupState` 这样的平级业务字段。
// 硬造一个统一结构只会让"实际上没有的字段"看起来存在，
// 所以这里只收敛**两个**跨接口成立的动作：取业务码、取载荷。
//
// # ⚠ 内部存的是**解码后的值**，且数字保持字面量
//
// 解析走 [core.ParseJSON]（`UseNumber`）：19 位的 `userDomainId`
// 若被解成 float64 会**静默丢精度**（末几位归零），
// 而丢掉的恰好是最不该丢的那几位。
type Envelope struct {
	raw any
}

// NewEnvelope 用一个**已解码**的 JSON 值构造信封（测试与调用方已有值时用）。
func NewEnvelope(raw any) *Envelope { return &Envelope{raw: raw} }

// ParseEnvelope 从响应体原文解析出信封。
//
// 与 [NewEnvelope] 的区别只是"要不要先解一次 JSON"，判定逻辑**只有一份**。
func ParseEnvelope(body []byte) (*Envelope, *core.Error) {
	value, ok := core.ParseJSON(string(body))
	if !ok {
		return nil, core.NewParse("响应体不是合法 JSON")
	}
	return &Envelope{raw: value}, nil
}

// Raw 返回原始响应体。接口自己有额外字段（`groupState` 之类）时从这里取。
func (e *Envelope) Raw() any { return e.raw }

// field 取对象字段；响应体不是对象（例如直接回数组）时返回 false。
func (e *Envelope) field(name string) (any, bool) {
	obj, ok := e.raw.(map[string]any)
	if !ok {
		return nil, false
	}
	value, ok := obj[name]
	return value, ok
}

// Code 返回业务码。
//
// `code` 既有数字也有**字符串**（`RefreshTokenResponse.code` 就是 string），
// 两种都要认；没有 `code` 字段时返回 false
// —— 有些接口直接回数组或裸值，本来就没有信封。
func (e *Envelope) Code() (int64, bool) {
	value, ok := e.field("code")
	if !ok {
		return 0, false
	}
	return asInt64(value)
}

// Success 返回 `success` 布尔位。有些接口（`RefreshTokenResponse`）用它表达成败。
//
// 与 [Envelope.Code] **刻意分开**：把 `success` 折进 `code`
// （`false` → 0）会让 `{"success": false}` 被 [Envelope.EnsureOK] 判成成功 ——
// 见那里的说明。
func (e *Envelope) Success() (bool, bool) {
	value, ok := e.field("success")
	if !ok {
		return false, false
	}
	flag, ok := value.(bool)
	return flag, ok
}

// Message 返回人类可读的上游消息。**只进日志**，不要塞进 API 响应
// （见 core.Error.PublicMessage）。
//
// 两个键都认：`msg` 与 `message`。
func (e *Envelope) Message() (string, bool) {
	for _, key := range []string{"msg", "message"} {
		if value, ok := e.field(key); ok {
			if text, ok := value.(string); ok {
				return text, true
			}
		}
	}
	return "", false
}

// Payload 返回业务载荷：`data` 或 `result`。
func (e *Envelope) Payload() (any, bool) {
	for _, key := range []string{"data", "result"} {
		if value, ok := e.field(key); ok {
			return value, true
		}
	}
	return nil, false
}

// EnsureOK 在业务失败时返回错误，成功时给出载荷（`nil` 表示本来就没有载荷）。
//
// 只做到"非 0 即失败"这一层：各期活动的"已抢完 / 云朵不足"码并不统一，
// 硬编码只会把某一期的码当成通用语义。细分交给调用方
// （见 core.ClassifyBusinessCode 的说明）。
func (e *Envelope) EnsureOK() (any, *core.Error) {
	// `success: false` 必须判失败，而且**优先于** `code`。
	//
	// 这里踩过一次：最初把 `success` 折进 `code`（`false` → `0`），
	// 于是 `{"success": false}` 被判成功。两类错误的代价并不对称 ——
	// 多报一次失败只是噪声，把失败当成功会让调用方拿着一张空 token 继续往下跑。
	if success, ok := e.Success(); ok && !success {
		if msg, ok := e.Message(); ok {
			return nil, core.NewBadRequest(fmt.Sprintf("上游返回 success=false: %s", msg))
		}
		return nil, core.NewBadRequest("上游返回 success=false")
	}

	code, hasCode := e.Code()
	if !hasCode || code == 0 {
		payload, _ := e.Payload()
		return payload, nil
	}
	if msg, ok := e.Message(); ok {
		return nil, core.NewBadRequest(fmt.Sprintf("上游返回 code=%d: %s", code, msg))
	}
	return nil, core.NewBadRequest(fmt.Sprintf("上游返回 code=%d", code))
}

// asInt64 把"可能是数字也可能是字符串"的值统一成 int64。
//
// ⚠ 数字在这里是 [json.Number]（**字符串**形态的字面量），不是 float64 ——
// 见 [Envelope] 的说明。所以两条分支都要走，缺一条就会把 `"0"` 当成"没有信封"。
//
// # 为什么还要收 `int` / `int64`
//
// 同一个 `map[string]any` 在 Go 里有两种来源：从响应体解出来的（数字是
// [json.Number]），以及**我们自己拼出来的**（`BuildSmData` 返回的那个上报体，
// 数字是 `int`）。只认前一种，后一种就会静默返回 `(0, false)` ——
// 表现为"encode 应为 5，实得 5"这种看不懂的错误（§6.1 #34 的第二种形态）。
func asInt64(value any) (int64, bool) {
	switch v := value.(type) {
	case json.Number:
		n, err := v.Int64()
		return n, err == nil
	case string:
		n, err := strconv.ParseInt(strings.TrimSpace(v), 10, 64)
		return n, err == nil
	case float64:
		return int64(v), true
	case int:
		return int64(v), true
	case int64:
		return v, true
	}
	return 0, false
}

// errorFromStatus 把 HTTP 状态码映射成错误。
//
// **不带响应体原文**：上游的错误体里常常直接回显凭据，
// 而 core.Error 的 Message 会进日志、PublicMessage 才是对外文案 ——
// 两头都不该出现上游原文，所以这里只保留状态码。
func errorFromStatus(status uint16) *core.Error {
	detail := fmt.Sprintf("HTTP %d", status)
	switch core.ClassifyStatus(status) {
	case core.KindAuth:
		return core.NewAuth(detail)
	case core.KindRateLimited:
		return core.NewRateLimited(detail)
	case core.KindTimeout:
		return core.NewTimeout(detail)
	case core.KindBadRequest:
		return core.NewBadRequest(detail)
	default:
		return core.NewUpstream(status, detail)
	}
}
