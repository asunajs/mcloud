// 上游响应信封与接口基址 —— 对应 Rust 侧 ../mcloud-rust/crates/mcloud-upstream/src/lib.rs。
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

// envelopeDetailLimit 是 `detail` 进错误文本时的长度上限（按**字**数，不是字节）。
//
// 为什么要有上限而不是"照抄整句"：这一格的形状本仓没有约束力（见 [Envelope.Detail]），
// 而失败文本会同时进日志与逐条播报。今天唯一的样本是五个字（"任务不存在"），
// 上限只是防"上游哪天把整段 HTML 塞进来"，不针对任何已知样本。
const envelopeDetailLimit = 160

// Detail 返回 `detail` —— 上游把**真正的原因**写在这一格的那一句。
//
// 为什么单独有一个动作（`Message` 不是已经给人看的话了吗）：2026-09-25 用户提供的一次
// 线上采集里，`task/click` 对 22 个任务 id 全部回
// `{"code":58999,"message":"当前用户信息读取异常，请尝试重新登录或重启手机","detail":"任务不存在",…}`
// —— 两格说的**不是同一件事**（一个把人往登录态引、一个说"任务不存在"），
// 而从前只有 `message` 进错误文本，于是日志里只剩那一句误导人的话（坑表 **#149**、
// `docs/pending-decisions.md` Q40）。
//
// ⚠ **只收字符串**。`detail` 这一格在别处是**载荷容器**：`smid` 那一口回的是
// `{"code":1100,"detail":{"deviceId":…}}`（见 `smid.go` 自己的解析），
// 把对象整块 `fmt` 进错误文本等于把标识类字段打进日志 ⇒ 非字符串一律读成"没给"。
//
// ⚠ 也**不解释**：本层只多交一句原话，不判它与 `message` 谁对（判据仍只有 `code`）。
// 与 [Envelope.Message] 同一条边界：**只进日志**，不进对外文案（`core.Error.PublicMessage`
// 是白名单，不会带出这一句）。
func (e *Envelope) Detail() (string, bool) {
	value, ok := e.field("detail")
	if !ok {
		return "", false
	}
	text, ok := value.(string)
	if !ok {
		return "", false
	}
	if strings.TrimSpace(text) == "" {
		return "", false
	}
	return text, true
}

// withDetail 把 `detail` 拼在既有的失败文本后面；没有那一格时**原样交回**。
//
// 分隔符用 `｜` 而不是括号：`EnsureOKWithHint` 已经在失败文本尾巴上拼过一枚
// `（<本仓的诊断>）`，两种东西必须看得出谁说的（上游原话 vs 本仓档位表）。
//
// 与 `message` 逐字相同时不拼：那一格只是重复，拼上去会让"上游给了两格"看起来成立。
//
// ⭐ "没有那一格时**原样交回**"这一支今天有过一次真账号实测：同日第八趟单跑 `revival`
// 撞到 `code=500 本月已领取复活卡奖励`，事件流里那一句既没多出 `｜上游 detail：`、
// 也没多出空冒号或空尾巴 ⇒ 旧文本逐字不变（正是 `TestEnvelopeWithoutDetailKeepsTheOldText`
// 钉的那个形状，从此它不只由替身支撑）。
// ⚠ 但这一发**分不清**"上游没带这个键"还是"带了却不是字符串"：本仓不留响应体原文
// （上游错误体常回显凭据，见 [errorFromStatus] 那条注释）。而"真带上来的 `detail`
// 拼出去长什么样"仍只有进程内替身 —— 别把它读成已线上验证（Q40 后半边）。
func (e *Envelope) withDetail(base string) string {
	detail, ok := e.Detail()
	if !ok {
		return base
	}
	if msg, hasMsg := e.Message(); hasMsg && strings.TrimSpace(msg) == detail {
		return base
	}
	runes := []rune(detail)
	if len(runes) > envelopeDetailLimit {
		detail = string(runes[:envelopeDetailLimit]) + "…"
	} else {
		detail = string(runes)
	}
	return base + "｜上游 detail：" + detail
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
			return nil, core.NewBadRequest(e.withDetail(fmt.Sprintf("上游返回 success=false: %s", msg)))
		}
		return nil, core.NewBadRequest(e.withDetail("上游返回 success=false"))
	}

	code, hasCode := e.Code()
	if !hasCode || code == 0 {
		payload, _ := e.Payload()
		return payload, nil
	}
	if msg, ok := e.Message(); ok {
		return nil, core.NewBadRequest(e.withDetail(fmt.Sprintf("上游返回 code=%d: %s", code, msg)))
	}
	return nil, core.NewBadRequest(e.withDetail(fmt.Sprintf("上游返回 code=%d", code)))
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
//
// ⚠ 与 `internal/io.statusError` 是**两处**做同一件事（一处是传输层出口、
// 一处是信封解析出口），改分类时两边都要改。2026-09-25 之前这两处的标签
// 就不一致：io 侧 404 报"参数错误"、这一侧 404 走 `default` 报"上游服务异常"**并且可重试**。
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
	case core.KindNotFound:
		// ⚠ 这一条**必须显式写**：`default` 那支是 `NewUpstream`，而 upstream 是
		// 可重试类别 —— 漏了它，404 会变成"重试到死"，把一个"端点不存在"
		// 洗成一次超时。2026-09-25 真账号跑 ai-avatar 整族撞见 404/405 时，
		// 最先把这条分支逼出来的就是这个默认值。
		return core.NewNotFound(detail)
	default:
		return core.NewUpstream(status, detail)
	}
}
