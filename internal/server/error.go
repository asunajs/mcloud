package server

// 领域错误 → HTTP 响应。
//
// # 为什么要有这一层
//
// 存储层返回的是 [core.Error]（带 [core.ErrorKind]），而 HTTP 要的是
// **状态码 + 稳定的响应体形状**。把映射写在一个地方，前端才可能只认一套形状；
// 散在每个处理器里，就会出现"同一个错误这类接口返 400、那类接口返 500"。
//
// # 文案的两条规矩（沿用 [core.ErrorKind.PublicMessage] 的约定）
//
//   - **4xx 用原文**：这些文案是我们自己写给使用者的（"口令至少 8 位"、
//     "系统里已经有 1 个用户了"），删掉限定词只剩"请求参数不合法"，使用者没法据此改。
//   - **5xx 用类别文案**：原文常常带着路径、SQL 片段或上游返回体 ——
//     它只进日志，响应里给 [core.ErrorKind.PublicMessage] 的类别文案。
//
// 这条分界不是洁癖：4xx 的文案是我们**故意**写给对方看的，
// 5xx 的文案是出了意外时顺手拼出来的。

import (
	"io"
	"net/http"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// maxBodyBytes 是请求体上限。
//
// 不加它，一个不带 Content-Length 的大 body 就能把内存钉住。
// 取 1 MiB 而不是更大：这个服务的请求体只有登录口令、定时任务定义这些，
// 1 MiB 已经是两个数量级的余量。
const maxBodyBytes = 1 << 20

// writeJSON 写一个 JSON 响应。
//
// ⚠ 一律走 [core.MarshalJSON] 而不是 [json.Marshal]：转义规则（HTML 字符不转义）
// 与空数组的形状（`[]` 不是 `null`）在别处已经钉住，这里再走一份就会分叉。
func writeJSON(w http.ResponseWriter, status int, value any) {
	raw, err := core.MarshalJSON(value)
	if err != nil {
		// 序列化失败理论上不可能（字段全是字符串与数字）。真发生了也不能退化成
		// "先写半个响应再说" —— 那会让前端拿到一段截断的 JSON 并报一个看不懂的错。
		w.Header().Set("Content-Type", "application/json; charset=utf-8")
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = w.Write([]byte(`{"code":"internal","message":"` + core.KindInternal.PublicMessage() + `"}`))
		return
	}
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_, _ = w.Write(raw)
}

// decodeJSON 解请求体；任何失败都写成 [ErrBadBody]。
//
// # 为什么不能让调用方自己 `json.NewDecoder(r.Body).Decode(&v)`
//
//   - 那样"缺字段"与"不是合法 JSON"会各自散在处理器里，错误形状也就跟着散了 ——
//     前端是按 `{code, message}` 写的错误处理，拿到别的东西只能显示"HTTP 4xx"。
//   - `json.Unmarshal` 还绕开了 [core.ParseJSONTo] 那条"19 位标识不丢精度"
//     的保证（`UseNumber` 是解码器上的开关），而那种绕开不报错。
//
// # 为什么是泛型而不是 `out any`
//
// [core.ParseJSONTo] 是泛型函数；参数写成 `any` 就只能自己再拼一次解码器，
// 于是上面两条又会被绕开一次。泛型让"解成什么类型"这件事留在调用点，
// 而解码口径仍然只有这一处。
//
// # 分两次解析
//
// 先按 `any` 判"是不是合法 JSON"（报错能说清是**哪一种**不合格：
// 不是合法 JSON / 不是数组 / 缺字段），再按 `T` 填字段。
// 只判一次的话，`{"name":123}` 会拿到与 `[1,2,3]` 同一句"请求体不合契约"，
// 而前者使用者能改、后者不能。
func decodeJSON[T any](w http.ResponseWriter, r *http.Request) (T, bool) {
	var zero T
	raw, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBodyBytes))
	if err != nil {
		ErrBadBody("请求体读不出来，或超过 " + strconv.Itoa(maxBodyBytes) + " 字节").Write(w)
		return zero, false
	}
	if _, ok := core.ParseJSON(string(raw)); !ok {
		ErrBadBody("请求体不是合法 JSON").Write(w)
		return zero, false
	}
	value, ok := core.ParseJSONTo[T](string(raw))
	if !ok {
		ErrBadBody("请求体不合契约").Write(w)
		return zero, false
	}
	return value, true
}

// asDomain 把存储层返回的裸 `error` 收成 [core.Error]。
//
// 存储层按 Go 的 `database/sql` 约定返回 `error`；这里统一收口，
// 免得每个处理器各写一遍"这是不是 *core.Error"。
func asDomain(err error) *core.Error {
	if perr, ok := err.(*core.Error); ok {
		return perr
	}
	return core.NewInternal(err.Error())
}

// apiKind 是错误的大类：不是领域错误的那些（它们没有 [core.ErrorKind]）。
type apiKind int

const (
	// apiDomain 是领域层错误，按 [core.ErrorKind] 映射状态码。
	apiDomain apiKind = iota
	// apiUnauthorized 是未登录 / 会话无效。
	//
	// **不区分**"没带 cookie"与"cookie 无效/已撤销"：两者该导出同一个动作 —— 去登录。
	apiUnauthorized
	// apiSetupRequired 是还没初始化首个管理员：此时除初始化接口外一律不响应。
	apiSetupRequired
	// apiBadBody 是请求体不是合法 JSON，或字段不符合契约。
	apiBadBody
)

// APIError 是接口错误。
//
// ⚠ 它不是 [core.Error]：那是**领域**错误，进不了 HTTP 这层的判断
// （"没有会话"在领域层根本没有对应物）。两边各留一个类型，
// 才不会出现"为了发一个 401 而去造一个 KindAuth"。
type APIError struct {
	kind   apiKind
	domain *core.Error
	detail string
}

// ErrDomain 包一个领域错误。
func ErrDomain(err *core.Error) APIError { return APIError{kind: apiDomain, domain: err} }

// ErrUnauthorized 未登录。
func ErrUnauthorized() APIError { return APIError{kind: apiUnauthorized} }

// ErrSetupRequired 还没初始化。
func ErrSetupRequired() APIError { return APIError{kind: apiSetupRequired} }

// ErrBadBody 请求体不合契约。detail 是对使用者**可行动**的说明（缺了哪个字段）。
func ErrBadBody(detail string) APIError {
	return APIError{kind: apiBadBody, detail: detail}
}

// statusAndCode 给出状态码与响应体里的 `code`。
//
// `code` 是给前端 `switch` 用的稳定标识，**不是** [core.ErrorKind.String] ——
// 后者是给日志看的，两者改动节奏不同（类别会细分，外面的 `code` 不该跟着变）。
func (e APIError) statusAndCode() (int, string) {
	switch e.kind {
	case apiUnauthorized:
		return http.StatusUnauthorized, "unauthorized"
	case apiSetupRequired:
		return http.StatusServiceUnavailable, "setup_required"
	case apiBadBody:
		return http.StatusBadRequest, "bad_request"
	}

	switch e.domain.Kind() {
	case core.KindAuth:
		return http.StatusUnauthorized, "unauthorized"
	case core.KindNotFound:
		return http.StatusNotFound, "not_found"
	case core.KindBadRequest:
		return http.StatusBadRequest, "bad_request"
	case core.KindRateLimited:
		return http.StatusTooManyRequests, "rate_limited"
	// 业务冲突：不是"参数写错了"，而是"当前状态下做不了" ——
	// 用 409 让前端能把它与 400 区分开（前者可以重试/换时间，后者不能）。
	case core.KindSoldOut, core.KindInsufficientCloud:
		return http.StatusConflict, "conflict"
	// 错不在这个请求，也不在我们内部：上游/网络问题一律 502。
	case core.KindTimeout, core.KindNetwork, core.KindUpstream:
		return http.StatusBadGateway, "upstream"
	}
	// Parse / Internal：真正的 5xx。
	//
	// ⚠ 用**兜底**而不是逐个列：新增一种 ErrorKind 时**默认落 500**，
	// 而不是让维护者给它挑一个"看起来合适"的 4xx —— 4xx 意味着"使用者的错"，
	// 而一个新类别的语义还没人验证过。
	return http.StatusInternalServerError, "internal"
}

// Message 是响应体里的 `message`。
//
// 5xx 走类别文案这条由 `TestInternalErrorsDoNotLeakDetails` 钉住：
// 那些原文里实测带过 SQL 片段与上游响应体。
func (e APIError) Message() string {
	switch e.kind {
	case apiUnauthorized:
		return "请先登录"
	case apiSetupRequired:
		return "还没初始化首个管理员，请先完成初始化"
	case apiBadBody:
		return e.detail
	}

	status, _ := e.statusAndCode()
	if isClientError(status) {
		// 用 `Message()` 而不是 `Error()`：后者带"凭据失效:"这类类别前缀，
		// 那是给日志看的，端给使用者就成了"凭据失效: 用户名或口令不正确"。
		return e.domain.Message()
	}
	return e.domain.Kind().PublicMessage()
}

// Status 是 HTTP 状态码。
func (e APIError) Status() int {
	status, _ := e.statusAndCode()
	return status
}

// Code 是响应体里的 `code`。
func (e APIError) Code() string {
	_, code := e.statusAndCode()
	return code
}

// Write 把错误写成响应。
//
// 单独给一个方法而不是让调用方拼 JSON：响应体形状必须**只有一处**定义，
// 否则"某个处理器忘了包一层"就会让前端的错误分支收到别的东西。
func (e APIError) Write(w http.ResponseWriter) {
	writeJSON(w, e.Status(), map[string]any{
		"code":    e.Code(),
		"message": e.Message(),
	})
}

func isClientError(status int) bool { return status >= 400 && status < 500 }
