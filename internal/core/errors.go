// 错误分类 —— 对应 Rust 侧 legacy/crates/mcloud-core/src/error.rs（前 197 行）。
//
// 分类的唯一目的是回答两个问题：**能不能重试**、**能不能把原文给用户看**。
//
// 对齐参考项目 caiyun 的两条约定：
//   - `exchange_request_control.go`：错误分类决定重试策略；
//   - `exchange_errors.go`：对外**绝不回传** err.Error()，走文案白名单。
//
// ⚠ 本文件只覆盖 error.rs 的**前半**（ErrorKind / Error / classify_status /
// classify_business_code）。后半的 Classified / classify_body 是一台独立的
// "响应体分类机"，等迁到用到它的层时再补 —— 现在写它没有消费者，
// 而"写下来就没人再看"的代码正是这个项目反复吃亏的地方。
package core

import (
	"errors"
	"fmt"
)

// ─────────────────────────────────────────────────────────────────────────────
// ErrorKind
// ─────────────────────────────────────────────────────────────────────────────

// ErrorKind 是错误类别。
type ErrorKind int

const (
	// KindAuth 凭据失效 —— 不重试，标记该账号需要重新登录/刷新。
	KindAuth ErrorKind = iota
	// KindRateLimited 触发风控或限流 —— 退避后重试（并发过高时最常出现）。
	KindRateLimited
	// KindSoldOut 奖品已兑完 —— 不重试。
	KindSoldOut
	// KindInsufficientCloud 云朵不足 —— 不重试。
	KindInsufficientCloud
	// KindTimeout 超时 —— 可重试。
	KindTimeout
	// KindNetwork 网络层错误 —— 可重试。
	KindNetwork
	// KindUpstream 上游 5xx —— 可重试。
	KindUpstream
	// KindBadRequest 请求参数错误 —— 不重试。
	KindBadRequest
	// KindNotFound 资源不存在**或不属于当前用户** —— 不重试。
	//
	// 两者刻意共用一个类别：若"不属于你"给出与"不存在"不同的错误，
	// 响应差异本身就泄漏了资源是否存在，把 id 空间变成可枚举的。
	KindNotFound
	// KindParse 响应解析失败 —— 不重试（通常是上游改动，重试无意义）。
	KindParse
	// KindInternal 内部错误 —— 不重试。
	KindInternal
)

// String 返回类别的标识名（调试用）。
//
// ⚠ 这**不是**对外文案 —— 对外只能用 [ErrorKind.PublicMessage]。
func (k ErrorKind) String() string {
	switch k {
	case KindAuth:
		return "auth"
	case KindRateLimited:
		return "rate_limited"
	case KindSoldOut:
		return "sold_out"
	case KindInsufficientCloud:
		return "insufficient_cloud"
	case KindTimeout:
		return "timeout"
	case KindNetwork:
		return "network"
	case KindUpstream:
		return "upstream"
	case KindBadRequest:
		return "bad_request"
	case KindNotFound:
		return "not_found"
	case KindParse:
		return "parse"
	case KindInternal:
		return "internal"
	}
	return fmt.Sprintf("ErrorKind(%d)", int(k))
}

// IsRetryable 判断是否可安全重试。
//
// 注意：**写请求**即使返回 true，也必须带幂等键才能重试。
func (k ErrorKind) IsRetryable() bool {
	switch k {
	case KindRateLimited, KindTimeout, KindNetwork, KindUpstream:
		return true
	}
	return false
}

// MarksAccountUnhealthy 判断是否可以立即判定"该账号有硬问题"（用于健康度与告警）。
func (k ErrorKind) MarksAccountUnhealthy() bool {
	return k == KindAuth || k == KindRateLimited
}

// PublicMessage 返回对外可见文案。原文只进日志，**不进 API 响应**。
func (k ErrorKind) PublicMessage() string {
	switch k {
	case KindAuth:
		return "账号凭据已失效，请重新登录"
	case KindRateLimited:
		return "触发限流，请降低并发后重试"
	case KindSoldOut:
		return "奖品已兑完"
	case KindInsufficientCloud:
		return "云朵不足"
	case KindTimeout:
		return "请求超时"
	case KindNetwork:
		return "网络异常"
	case KindUpstream:
		return "上游服务异常"
	case KindBadRequest:
		return "请求参数不合法"
	case KindNotFound:
		return "资源不存在"
	case KindParse:
		return "响应解析失败"
	case KindInternal:
		return "内部错误"
	}
	return "内部错误"
}

// ClassifyStatus 按 HTTP 状态码分类。
//
// 这是纯函数，重试策略完全由它决定，因此单测直接覆盖边界值。
func ClassifyStatus(status uint16) ErrorKind {
	switch {
	case status >= 200 && status <= 299:
		// 成功不该走错误路径；落到 Internal 说明调用方判断有误
		return KindInternal
	case status == 401 || status == 403:
		return KindAuth
	case status == 408:
		return KindTimeout
	case status == 429:
		return KindRateLimited
	case status == 400 || status == 404 || status == 422:
		return KindBadRequest
	case status >= 500 && status <= 599:
		return KindUpstream
	}
	return KindInternal
}

// ClassifyBusinessCode 从上游响应的业务码推断错误类别。
//
// 上游用 200 + 业务码表达失败很常见，所以不能只看 HTTP 状态。
//
// 第二个返回值 ok 为 false 表示"按已知映射算成功"。**注意它不知道信封长什么样** ——
// 判断"这次响应到底属于哪一类失败"要用 error.rs 后半的 classify_body。
func ClassifyBusinessCode(code int64) (kind ErrorKind, ok bool) {
	if code == 0 {
		return 0, false
	}
	// 各期活动/接口的"已抢完"码不统一，这里只收敛出语义明确的几类，
	// 其余归 BadRequest，交由调用方按业务文案细化。
	return KindBadRequest, true
}

// ─────────────────────────────────────────────────────────────────────────────
// Error
// ─────────────────────────────────────────────────────────────────────────────

// errVariant 对应 Rust enum Error 的**变体**。
//
// ⚠⚠ 它**不是** [ErrorKind] 的同义词，虽然绝大多数情况一一对应：
// FixtureMiss 与 Internal 是**两个变体、同一个 kind**。
// 把两者合并会丢掉"fixture 未命中"这条对外文案，而它恰恰是用来
// 让"偷偷打真网"立刻暴露的那一条 —— 见 error.rs 的注释。
type errVariant int

const (
	variantAuth errVariant = iota
	variantRateLimited
	variantSoldOut
	variantInsufficientCloud
	variantTimeout
	variantNetwork
	variantUpstream
	variantBadRequest
	variantNotFound
	variantParse
	variantInternal
	variantFixtureMiss
)

// Error 对应 Rust 的 enum Error。
//
// 变体不导出 ⇒ 只能经构造函数产出，与 Rust 的 enum 构造同强度。
type Error struct {
	variant errVariant
	msg     string
	status  uint16 // 仅 variantUpstream 有效
}

// NewAuth 构造"凭据失效"。
func NewAuth(msg string) *Error { return &Error{variant: variantAuth, msg: msg} }

// NewRateLimited 构造"限流/风控"。
func NewRateLimited(msg string) *Error { return &Error{variant: variantRateLimited, msg: msg} }

// NewSoldOut 构造"已兑完"。
func NewSoldOut(msg string) *Error { return &Error{variant: variantSoldOut, msg: msg} }

// NewInsufficientCloud 构造"云朵不足"。
func NewInsufficientCloud(msg string) *Error {
	return &Error{variant: variantInsufficientCloud, msg: msg}
}

// NewTimeout 构造"超时"。
func NewTimeout(msg string) *Error { return &Error{variant: variantTimeout, msg: msg} }

// NewNetwork 构造"网络错误"。
func NewNetwork(msg string) *Error { return &Error{variant: variantNetwork, msg: msg} }

// NewUpstream 构造"上游错误"。
func NewUpstream(status uint16, msg string) *Error {
	return &Error{variant: variantUpstream, msg: msg, status: status}
}

// NewBadRequest 构造"参数错误"。
func NewBadRequest(msg string) *Error { return &Error{variant: variantBadRequest, msg: msg} }

// NewNotFound 构造"不存在"。
func NewNotFound(msg string) *Error { return &Error{variant: variantNotFound, msg: msg} }

// NewParse 构造"解析失败"。
func NewParse(msg string) *Error { return &Error{variant: variantParse, msg: msg} }

// NewInternal 构造"内部错误"。
func NewInternal(msg string) *Error { return &Error{variant: variantInternal, msg: msg} }

// NewFixtureMiss 构造"fixture 未命中"。
func NewFixtureMiss(msg string) *Error { return &Error{variant: variantFixtureMiss, msg: msg} }

// Error 实现 error 接口 —— **带**类别前缀，对应 Rust 的 Display。
//
// 前缀在**日志**里很有用（一眼看出类别），但直接端给使用者就很怪 ——
// 登录页上写"凭据失效: 用户名或口令不正确"，前半句是给运维看的。
// 所以要不要前缀是调用方的事：[Error.Message] 给不带前缀的正文，
// [Error.PublicMessage] 给白名单文案。**不要**用 split(": ") 去前缀。
func (e *Error) Error() string {
	switch e.variant {
	case variantAuth:
		return "凭据失效: " + e.msg
	case variantRateLimited:
		return "限流/风控: " + e.msg
	case variantSoldOut:
		return "已兑完: " + e.msg
	case variantInsufficientCloud:
		return "云朵不足: " + e.msg
	case variantTimeout:
		return "超时: " + e.msg
	case variantNetwork:
		return "网络错误: " + e.msg
	case variantUpstream:
		return fmt.Sprintf("上游错误 %d: %s", e.status, e.msg)
	case variantBadRequest:
		return "参数错误: " + e.msg
	case variantNotFound:
		return "不存在: " + e.msg
	case variantParse:
		return "解析失败: " + e.msg
	case variantInternal:
		return "内部错误: " + e.msg
	case variantFixtureMiss:
		return "fixture 未命中: " + e.msg
	}
	return e.msg
}

// Message 返回错误正文 —— **不含**类别前缀。
func (e *Error) Message() string { return e.msg }

// Kind 返回错误类别。
func (e *Error) Kind() ErrorKind {
	switch e.variant {
	case variantAuth:
		return KindAuth
	case variantRateLimited:
		return KindRateLimited
	case variantSoldOut:
		return KindSoldOut
	case variantInsufficientCloud:
		return KindInsufficientCloud
	case variantTimeout:
		return KindTimeout
	case variantNetwork:
		return KindNetwork
	case variantUpstream:
		return KindUpstream
	case variantBadRequest:
		return KindBadRequest
	case variantNotFound:
		return KindNotFound
	case variantParse:
		return KindParse
	}
	// Internal 与 FixtureMiss 同归 Internal
	return KindInternal
}

// Status 返回上游 HTTP 状态码；非 Upstream 变体时为 0。
func (e *Error) Status() uint16 { return e.status }

// IsFixtureMiss 报告这是否是"fixture 未命中"。
//
// 单独给出是因为它**在 kind 上看不出来**（与 Internal 同类），
// 而测试与调试台恰恰只关心这一个变体。
func (e *Error) IsFixtureMiss() bool { return e.variant == variantFixtureMiss }

// PublicMessage 返回可对外暴露的文案（不含任何原文细节）。
func (e *Error) PublicMessage() string { return e.Kind().PublicMessage() }

// KindOf 从任意 error 里取出 [Error] 的类别。
//
// ⚠ 用 errors.As 而**不是**裸类型断言：调用方几乎一定会用
// `fmt.Errorf("...: %w", err)` 包一层上下文（Rust 的 `?` 不会，Go 的惯例会），
// 裸断言在那种情况下**静默返回 ok=false** —— 于是"分类"这件事
// 在最需要它的地方失效，而且不报错。
//
// 非本包的 error 返回 ok=false —— 调用方**必须**处理这个分支，
// 不能把"未知错误"默认成 Internal 就当分类做完了。
func KindOf(err error) (kind ErrorKind, ok bool) {
	var e *Error
	if errors.As(err, &e) {
		return e.Kind(), true
	}
	return 0, false
}
