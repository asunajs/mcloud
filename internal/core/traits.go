// IO trait —— 全部外部世界从这里进入。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/traits.rs（681 行）。
//
// 具体实现在 `internal/io`（ReqwestClient / SqliteStore / SystemClock），
// 测试与调试台用 fixture client / 内存 store / [FixedClock] 替换。
//
// ⚠ 本文件的形状由第十份黄金向量 `traits-shape-vectors.json` 逐条对拍
// （生成器 `scripts/gen-traits-shape-vectors.rs`，**path 依赖真实的
// mcloud-core** 后由 serde 产出）。理由见 [Body] 那条注释 ——
// `HTTPRequest` 是"golden request 对拍"的载体，而 Rust 侧对它的形状
// 一个断言都没有。
package core

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

// ─────────────────────────────────────────────────────────────────────────────
// HTTP
// ─────────────────────────────────────────────────────────────────────────────

// HeaderPair 是一对请求/响应头。
//
// ⚠ 刻意是**数组**而不是带字段名的结构体：Rust 侧是 `Vec<(String, String)>`，
// serde 把它序列化成**二元组数组** `[["x-a","1"]]`，而 Go 的
// `struct{ Key, Value string }` 序列化成**对象数组** `[{"key":"x-a",...}]`。
// 两侧写出的 golden request 文件会因此不是同一份东西，而且**不报错**。
//
// 用 `[2]string` 让形状天然对齐（实测见向量的 `$structJson`）。
type HeaderPair = [2]string

// Body 是请求/响应体的原始字节。
//
// # ⚠⚠ serde 的 `Vec<u8>` 是**数字数组**，Go 的 `[]byte` 是 **base64 串**
//
//	Rust: {"body":[123,34,97,34,58,49,125]}
//	Go  : {"body":"eyJhIjox"}
//
// 这是同一族差异里最容易被漏掉的一条：`encoding/json` 对 `[]byte` 有
// **专门的** base64 处理，于是"两侧都跑得通、都解析得回来"，
// 只是**写出来的文件不是同一份**。而 `HTTPRequest` 的注释里写着
// "golden request 对拍依赖它逐字段可比" —— 对拍时两边各写一份文件，
// 逐字节 diff 才知道不一样，单侧跑永远是绿的。
//
// 所以这里显式实现编解码：**只接受数字数组**，base64 串一律拒
// （实测 serde 也拒，见向量的 `$decode`）。
type Body []byte

// MarshalJSON 输出数字数组，与 serde 的 `Vec<u8>` 同形。
func (b Body) MarshalJSON() ([]byte, error) {
	if len(b) == 0 {
		return []byte("[]"), nil
	}
	var sb strings.Builder
	sb.WriteByte('[')
	for i, v := range b {
		if i > 0 {
			sb.WriteByte(',')
		}
		sb.WriteString(strconv.Itoa(int(v)))
	}
	sb.WriteByte(']')
	return []byte(sb.String()), nil
}

// UnmarshalJSON 只接受 `null`（→ nil）与数字数组。
//
// ⚠ 刻意**不**直接 `json.Unmarshal(data, &b)`：那样 base64 串会被
// **静默接受**（`encoding/json` 对 `[]byte` 的特例），正是要对拍的两侧分歧点。
func (b *Body) UnmarshalJSON(data []byte) error {
	if string(data) == "null" {
		*b = nil
		return nil
	}
	var nums []int
	// ⚠ 刻意解进 `[]int` 而不是 `[]byte`：`encoding/json` 对 `[]byte` 有
	// **专门的** base64 分支，直接解会**静默接受**字符串形态 ——
	// 那正是两侧会分歧、却都"跑得通"的那一支。
	if err := json.Unmarshal(data, &nums); err != nil {
		return NewParse(fmt.Sprintf("请求/响应体应当是数字数组（Rust 侧 Vec<u8> 的形状），得到 %s",
			strings.TrimSpace(string(data))))
	}
	out := make([]byte, len(nums))
	for i, n := range nums {
		if n < 0 || n > 255 {
			return NewParse(fmt.Sprintf("请求/响应体的第 %d 个字节是 %d，超出 0..255", i, n))
		}
		out[i] = byte(n)
	}
	*b = out
	return nil
}

// HTTPMethod 是 HTTP 方法。
//
// ⚠ 第三族命名规则：Rust 侧是 `#[serde(rename_all = "UPPERCASE")]`
// （此前两族是 `snake_case` 与 `lowercase`）。而且它**同样约束解码** ——
// 实测 `"get"` / `"Get"` 都被 serde 拒掉，只有 `"GET"` 收。
type HTTPMethod int

const (
	MethodGet HTTPMethod = iota
	MethodPost
	MethodPut
	MethodDelete
)

// httpMethodJSON 对应 Rust 侧的 `#[serde(rename_all = "UPPERCASE")]`。
var httpMethodJSON = enumJSON{
	typeName: "HTTPMethod",
	names:    []string{"GET", "POST", "PUT", "DELETE"},
}

// MarshalJSON 输出 `GET` / `POST` / `PUT` / `DELETE`。
func (m HTTPMethod) MarshalJSON() ([]byte, error) { return httpMethodJSON.marshal(int(m)) }

// UnmarshalJSON 是 [HTTPMethod.MarshalJSON] 的逆。
//
// ⚠ 大小写敏感：`"get"` 与 `"Get"` **都要拒** —— serde 的 `rename_all`
// 在解码侧是等值匹配，不是"忽略大小写"。
func (m *HTTPMethod) UnmarshalJSON(data []byte) error {
	v, err := httpMethodJSON.unmarshal(data)
	if err != nil {
		return err
	}
	*m = HTTPMethod(v)
	return nil
}

// String 返回大写方法名，用于日志与对拍。
//
// ⚠ 它是 [HTTPMethod.MarshalJSON] 的**同一张表**，不是另写一份 ——
// 两处文本分叉的话，日志里看着对、写进文件却不对。
func (m HTTPMethod) String() string {
	switch m {
	case MethodGet:
		return "GET"
	case MethodPost:
		return "POST"
	case MethodPut:
		return "PUT"
	case MethodDelete:
		return "DELETE"
	}
	return ""
}

// IsRead 报告这是否是个读请求（决定是否默认可重试）。
func (m HTTPMethod) IsRead() bool { return m == MethodGet }

// HTTPRequest 是出站请求。
//
// # 三个字段的"缺席"语义各不相同，别统一处理
//
//   - `headers` 没有 `skip_serializing_if` ⇒ **永远出现**（空也是 `[]`）；
//   - `body` / `timeout_ms` / `idempotency_key` 是 `Option` + `skip_serializing_if`
//     ⇒ 缺席时**字段消失**（实测见 `$structJson` 的 `request-get-minimal`）；
//   - 解码侧反过来：`body: null` **收**（`Option` 的 None），
//     而 `headers: null` **拒**（`Vec` 的 `#[serde(default)]` 只管**缺席**，
//     不管显式 null）。同一个 `null` 在两种字段上结果相反 ——
//     这是实测出来的，不是从命名规则推的。
type HTTPRequest struct {
	Method         HTTPMethod   `json:"method"`
	URL            string       `json:"url"`
	Headers        []HeaderPair `json:"headers"`
	Body           Body         `json:"body,omitempty"`
	TimeoutMS      *uint64      `json:"timeout_ms,omitempty"`
	IdempotencyKey *string      `json:"idempotency_key,omitempty"`
}

// NewRequestGet 构造一个 GET 请求。
func NewRequestGet(url string) HTTPRequest {
	return HTTPRequest{Method: MethodGet, URL: url, Headers: []HeaderPair{}}
}

// NewRequestPost 构造一个 POST 请求。
func NewRequestPost(url string) HTTPRequest {
	return HTTPRequest{Method: MethodPost, URL: url, Headers: []HeaderPair{}}
}

// Header 追加一个头（链式）。
func (r HTTPRequest) WithHeader(key, value string) HTTPRequest {
	return r.appendHeader(key, value)
}

// appendHeader 返回**带着新 Headers 副本**的 r。
//
// ⚠ 必须拷贝而不是直接 append：接收者是值，但底层数组仍与调用方共享 ——
// cap>len 时一次 append 会写穿到原请求（或另一个同源副本）的 Headers 尾部。
// Rust 的 `Vec` 移动语义没有这个洞，翻译过来风险是 Go 新引入的。
func (r HTTPRequest) appendHeader(key, value string) HTTPRequest {
	headers := make([]HeaderPair, len(r.Headers)+1)
	copy(headers, r.Headers)
	headers[len(r.Headers)] = HeaderPair{key, value}
	r.Headers = headers
	return r
}

// JSONBody 序列化请求体，并在**尚无** content-type 时补一个。
//
// "尚无"是**大小写不敏感**地判断（实测：已有 `Content-Type` 时不补，
// 且**保留原样**，见 `$structJson` 的 `request-content-type-kept`）。
// 重复头由调用方自己负责 —— 上游表单接口本来就要发自己的 content-type。
func (r HTTPRequest) WithJSONBody(value any) (HTTPRequest, error) {
	raw, err := marshalJSON(value)
	if err != nil {
		return r, NewInternal(fmt.Sprintf("序列化请求体失败: %v", err))
	}
	r.Body = Body(raw)
	for _, h := range r.Headers {
		if strings.EqualFold(h[0], "content-type") {
			return r, nil
		}
	}
	return r.appendHeader("content-type", "application/json"), nil
}

// WithTimeoutMS 设置超时（链式）。
func (r HTTPRequest) WithTimeoutMS(ms uint64) HTTPRequest { r.TimeoutMS = &ms; return r }

// WithIdempotencyKey 设置幂等键（链式）。
//
// 它是**写请求可重试的前提**，见 [HTTPRequest.IsRetryable]。
func (r HTTPRequest) WithIdempotencyKey(key string) HTTPRequest {
	r.IdempotencyKey = &key
	return r
}

// IsRetryable 报告该请求当前是否允许重试。
//
// 读请求天然可重试；写请求必须携带幂等键 —— 否则重试等于重复下单。
func (r HTTPRequest) IsRetryable() bool {
	return r.Method.IsRead() || r.IdempotencyKey != nil
}

// MarshalJSON 按 Rust 侧的形状输出。
//
// # 为什么必须手写，不能靠 tag
//
// `body` 的取舍是"**nil 缺席、非 nil 出现（哪怕为空）**" ——
// 对应 Rust 的 `Option<Vec<u8>>`：None 缺席、`Some(vec![])` 输出 `[]`。
// 而 Go 的 `omitempty` 对 nil 与空切片**一视同仁**都会省略，
// 于是 `Some(vec![])` 这一支就消失了。只能显式用指针表达。
//
// ⚠ 走 [marshalJSON] 而不是 `json.Marshal`：URL 里带 `&` 很常见，
// 默认转义会写成 `\u0026`。
func (r HTTPRequest) MarshalJSON() ([]byte, error) {
	type wire struct {
		Method         HTTPMethod   `json:"method"`
		URL            string       `json:"url"`
		Headers        []HeaderPair `json:"headers"`
		Body           *Body        `json:"body,omitempty"`
		TimeoutMS      *uint64      `json:"timeout_ms,omitempty"`
		IdempotencyKey *string      `json:"idempotency_key,omitempty"`
	}
	w := wire{
		Method:         r.Method,
		URL:            r.URL,
		Headers:        r.Headers,
		TimeoutMS:      r.TimeoutMS,
		IdempotencyKey: r.IdempotencyKey,
	}
	if w.Headers == nil {
		// 缺席的 Vec 在 Rust 侧是 vec![]，再序列化一定带 `[]`。
		w.Headers = []HeaderPair{}
	}
	if r.Body != nil {
		w.Body = &r.Body
	}
	return marshalJSON(w)
}

// UnmarshalJSON 是 [HTTPRequest.MarshalJSON] 的逆。
//
// 两处**反直觉**，都由实测确定（见向量的 `$decode`），别从命名规则推：
//
//   - `headers` 缺席 → 空切片（Rust 的 `#[serde(default)] Vec` 缺席时是 `vec![]`），
//     否则再序列化会输出 `null` 而不是 `[]`；
//   - 但 `headers: null` **要拒** —— `default` 只管**缺席**，不管显式 null。
//     而 `body: null` 反而**要收**（它是 `Option`，null 就是 None）。
//     同一个 null 在两种字段上结果相反。
func (r *HTTPRequest) UnmarshalJSON(data []byte) error {
	// ⚠ `method` / `url` 用**指针**：Rust 侧这两个字段**没有** `#[serde(default)]`，
	// 缺席与显式 null 都是失败；而 Go 的零值会**静默通过**
	// （`method` 缺席 = MethodGet，一个看起来完全合理的请求）。
	// 指针把"缺席"与"null"合并成同一个可判的空，这里两者都要拒。
	type wire struct {
		Method         *HTTPMethod     `json:"method"`
		URL            *string         `json:"url"`
		Headers        json.RawMessage `json:"headers"`
		Body           Body            `json:"body"`
		TimeoutMS      *uint64         `json:"timeout_ms"`
		IdempotencyKey *string         `json:"idempotency_key"`
	}
	var w wire
	if err := json.Unmarshal(data, &w); err != nil {
		return err
	}
	if w.Method == nil {
		return NewParse("method 缺失或为 null：Rust 侧它没有 #[serde(default)]，不给就是失败")
	}
	if w.URL == nil {
		return NewParse("url 缺失或为 null：Rust 侧它没有 #[serde(default)]，不给就是失败")
	}
	if err := rejectNullVec(w.Headers, "headers"); err != nil {
		return err
	}
	headers, err := decodeHeaders(w.Headers, "headers")
	if err != nil {
		return err
	}
	*r = HTTPRequest{
		Method:         *w.Method,
		URL:            *w.URL,
		Headers:        headers,
		Body:           w.Body,
		TimeoutMS:      w.TimeoutMS,
		IdempotencyKey: w.IdempotencyKey,
	}
	return nil
}

// HTTPResponse 是入站响应。
//
// ⚠ `body` **没有** `Option`：Rust 侧是 `#[serde(default)] Vec<u8>`，
// 所以字段**永远出现**（空体是 `[]`，不是 null）。
type HTTPResponse struct {
	Status  uint16       `json:"status"`
	Headers []HeaderPair `json:"headers"`
	Body    Body         `json:"body"`
}

// NewResponse 构造一个响应。
func NewResponse(status uint16, body []byte) HTTPResponse {
	return HTTPResponse{Status: status, Headers: []HeaderPair{}, Body: Body(body)}
}

// Text 把响应体解成 UTF-8 文本。
//
// ⚠ 必须**显式校验**：Go 的 `string(bytes)` 不做 UTF-8 校验
// （非法字节变成 U+FFFD），而 Rust 的 `String::from_utf8` 是**失败**。
// 不校验的话，一个被截断或解密错的响应体会看起来"解析成功了"，
// 而它其实是从密文垃圾里硬读出来的一串替换字符。
func (r HTTPResponse) Text() (string, error) {
	if !utf8.Valid(r.Body) {
		return "", NewParse("响应体不是合法 UTF-8")
	}
	return string(r.Body), nil
}

// IsSuccess 报告状态码是否 2xx。
func (r HTTPResponse) IsSuccess() bool { return r.Status >= 200 && r.Status < 300 }

// rejectNullVec 挡住 `"字段": null`。
//
// ⚠⚠ `encoding/json` 把 null 解进切片是**静默成功**（置 nil、不报错），
// 而 serde 对 `Vec<T>` 的显式 null 是**失败** —— `#[serde(default)]`
// 只在字段**缺席**时补默认值，管不到显式 null。
//
// 这是"null 在 Go 侧静默成功"的**第三个**实例
// （前两个：解进 string 留空串 —— 见 [enumJSON.unmarshal]；
// 解进指针留 nil —— 见 [HTTPRequest.UnmarshalJSON] 的 body）。
// 三个都不是靠想出来的，都是实测撞上的。
// decodeHeaders 解 `Vec<(String, String)>` 形状的二维数组。
//
// ⚠ 刻意**不**直接解进 `[][2]string`：`encoding/json` 对**数组**是
// "元素不足补零值、多余则丢弃"（对切片是"长度不对就报错"，对数组不是），
// 于是 `[["x-a"]]` 会被静默解成 `["x-a", ""]` —— 一个**凭空多出来的空头值**。
// 而 serde 的元组长度是**类型的一部分**，1 元组与 3 元组都拒。
func decodeHeaders(raw json.RawMessage, field string) ([]HeaderPair, error) {
	if len(raw) == 0 {
		return []HeaderPair{}, nil
	}
	var items []json.RawMessage
	if err := json.Unmarshal(raw, &items); err != nil {
		return nil, NewParse(fmt.Sprintf("%s 应当是二维数组（Rust 侧 Vec<(String, String)> 的形状）", field))
	}
	out := make([]HeaderPair, 0, len(items))
	for i, item := range items {
		var pair []string
		if err := json.Unmarshal(item, &pair); err != nil {
			return nil, NewParse(fmt.Sprintf("%s[%d] 应当是两个字符串的数组: %v", field, i, err))
		}
		if len(pair) != 2 {
			return nil, NewParse(fmt.Sprintf("%s[%d] 是 %d 元组，Rust 侧的 (String, String) 必须是 2 元组",
				field, i, len(pair)))
		}
		out = append(out, HeaderPair{pair[0], pair[1]})
	}
	return out, nil
}

func rejectNullVec(raw json.RawMessage, field string) error {
	if len(raw) > 0 && string(raw) == "null" {
		return NewParse(fmt.Sprintf("%s 是 null：Rust 侧的 Vec 对显式 null 是失败（只有 Option 才收 null）", field))
	}
	return nil
}

// DecodeJSON 把响应体解成 T —— 对应 Rust 的 `HttpResponse::json::<T>()`。
//
// # 为什么是包级泛型函数而不是方法
//
// Go 的方法**不能**带自己的类型参数，而 Rust 的那个是
// `pub fn json<T: Deserialize>(&self) -> Result<T, Error>`。
// 只能提到包级：`DecodeJSON[Envelope](resp)`。
//
// ⚠ 解码失败一律归 [NewParse]（不可重试）：通常是上游改了形状，
// 重试只会拿到同一份解不开的东西。
func DecodeJSON[T any](r HTTPResponse) (T, error) {
	var v T
	if err := json.Unmarshal(r.Body, &v); err != nil {
		return v, NewParse(fmt.Sprintf("JSON 解析失败: %v", err))
	}
	return v, nil
}

// UnmarshalJSON 把缺席的 `headers` / `body` 归一化成空切片。
//
// 理由与 [HTTPRequest.UnmarshalJSON] 相同：Rust 的 `#[serde(default)] Vec`
// 缺席时是 `vec![]`，Go 是 nil，再序列化就输出 `null` 而不是 `[]`。
func (r *HTTPResponse) UnmarshalJSON(data []byte) error {
	// ⚠ `status` 用指针，理由与 [HTTPRequest.UnmarshalJSON] 的 method / url 相同：
	// 它**没有** `#[serde(default)]`，缺席就是失败，而 Go 的零值（0）会静默通过。
	type wire struct {
		Status  *uint16         `json:"status"`
		Headers json.RawMessage `json:"headers"`
		Body    json.RawMessage `json:"body"`
	}
	var w wire
	if err := json.Unmarshal(data, &w); err != nil {
		return err
	}
	if w.Status == nil {
		return NewParse("status 缺失或为 null：Rust 侧它没有 #[serde(default)]，不给就是失败")
	}
	// ⚠ 这里与 [HTTPRequest] **相反**：`body` 是 `Vec<u8>` 而不是 `Option`，
	// 所以 null 也要拒（实测见向量的 `$decode`）。别顺手照抄请求那条。
	if err := rejectNullVec(w.Headers, "headers"); err != nil {
		return err
	}
	if err := rejectNullVec(w.Body, "body"); err != nil {
		return err
	}
	headers, err := decodeHeaders(w.Headers, "headers")
	if err != nil {
		return err
	}
	body := Body{}
	if len(w.Body) > 0 {
		if err := json.Unmarshal(w.Body, &body); err != nil {
			return err
		}
	}
	*r = HTTPResponse{Status: *w.Status, Headers: headers, Body: body}
	return nil
}

// MarshalJSON 按 Rust 侧的形状输出（两个切片都要出现）。
func (r HTTPResponse) MarshalJSON() ([]byte, error) {
	type wire struct {
		Status  uint16       `json:"status"`
		Headers []HeaderPair `json:"headers"`
		Body    Body         `json:"body"`
	}
	w := wire{Status: r.Status, Headers: r.Headers, Body: r.Body}
	if w.Headers == nil {
		w.Headers = []HeaderPair{}
	}
	if w.Body == nil {
		w.Body = Body{}
	}
	return marshalJSON(w)
}

// HTTPClient 是出站 HTTP 的唯一出口。
//
// # ⚠ 与 Rust 的一处**刻意**不同：多一个 ctx
//
// Rust 侧是 `async fn send(&self, request: HTTPRequest)` —— `async fn in trait`
// 没有取消机制，也没有地方放它。Go 的 `net/http` 处处要 ctx，
// 少一个就等于让实现内部去猜（通常是 `context.Background()`，于是
// "取消一个跑了 20 个账号的签到"这件事根本没有入口）。
// 这是**设计选择**而不是翻译误差，所以写在这里而不是 §6.1。
type HTTPClient interface {
	Send(ctx context.Context, request HTTPRequest) (HTTPResponse, error)
}

// ─────────────────────────────────────────────────────────────────────────────
// 时间
// ─────────────────────────────────────────────────────────────────────────────

// Clock 是时间来源。
//
// 业务代码**禁止**直接 `time.Now()`：日快照归属、开抢倒计时、
// 凭据到期判断全都依赖时间，必须能在测试里被钉死。
type Clock interface {
	// Now 返回当前时刻。实现**必须**返回 UTC（Rust 侧是 `DateTime<Utc>`）。
	Now() time.Time
}

// FixedClock 是固定时钟 —— 纯实现，用于单测与调试台回放。
type FixedClock struct {
	fixed time.Time
}

// NewFixedClock 构造固定时钟，时刻统一归一化到 UTC。
func NewFixedClock(t time.Time) FixedClock { return FixedClock{fixed: t.UTC()} }

// ParseFixedClock 用 RFC 3339 字符串构造，失败即 panic。
//
// ⚠ 与 Rust 侧同口径（`FixedClock::parse` 也是 panic）：
// 它是**测试夹具**，字面量写错就该立刻炸，而不是悄悄用零值 ——
// 零值时间是 0001-01-01，落在任何"是否已过期"的判断里都是一个荒唐的答案。
func ParseFixedClock(rfc3339 string) FixedClock {
	t, err := time.Parse(time.RFC3339, rfc3339)
	if err != nil {
		panic(fmt.Sprintf("时间字面量必须是合法 RFC 3339: %q (%v)", rfc3339, err))
	}
	return NewFixedClock(t)
}

// Now 实现 [Clock]。
func (c FixedClock) Now() time.Time { return c.fixed }

// ─────────────────────────────────────────────────────────────────────────────
// 数据可见范围
// ─────────────────────────────────────────────────────────────────────────────

// Scope 是数据可见范围 —— "**谁来问**"这件事的唯一表达。
//
// # 为什么不是"每个方法都收一个 UserID"
//
// 隔离的初衷是**网络上的用户**：WebUI 里 A 不能看到 B 的账号。
// 但 CLI 是另一回事 —— 它跑在**持有数据库文件**的那台机器上：
// 能跑 CLI 的人本来就能直接 `sqlite3` 读库，给他加一道"你是谁"的关卡
// 既不增加任何安全性，又把本地运维变得别扭（列个账号都得先登录）。
//
// 所以把范围显式化：网络路径（WebUI 的每个请求）用 [ScopeUser]，
// 本地路径（CLI）用 [ScopeLocal]。每个调用点都必须写清楚自己站在哪一边，
// 而**过滤逻辑只有一份**（在存储实现里）：查询写
// `(?n IS NULL OR owner = ?n)`，一个表达式同时表达两种范围。
// 这比"再加一套不带用户参数的平行 API"好 —— 后者会让两条路的语义慢慢分叉。
type Scope struct {
	// user 为 nil 表示 [ScopeLocal]（不过滤）。
	//
	// ⚠⚠ 这个"nil = 不过滤"是**危险的默认值**，它被选中的唯一理由是
	// Rust 侧的 `Scope::Local` 也是"无约束"那一支。为了不让它被误取到，
	// 字段不导出，且 [ScopeUser] 对 nil 入参 **panic** —— 见那里的说明。
	user *UserID
}

// ScopeUser 声明"站在某个系统用户那边"。
//
// ⚠⚠ **nil 入参直接 panic**，这是刻意的一处不优雅。
//
// Rust 侧是 `Scope::User(&'a UserId)` —— `&T` 非空是**类型保证**，
// 编译器不允许构造出"User 但没有用户"这种态。Go 的 `*UserID` 可空，
// 于是 `ScopeUser(someLookupThatReturnedNil)` 会**静默变成 Local**，
// 而 Local 的语义是"不过滤" —— 一次失败的查用户，直接变成
// "把所有人的数据都列出来"。这是本项目里唯一一处
// "指针可空"会放大成越权的地方，所以让它炸，不要让它静默。
func ScopeUser(id *UserID) Scope {
	if id == nil {
		panic("ScopeUser: 用户标识为 nil（那会静默退化成不过滤的 ScopeLocal）")
	}
	return Scope{user: id}
}

// ScopeLocal 声明"站在本地运维那边"（不过滤）。
//
// 用它等于声明"调用方本来就拿着库文件"。**服务层的路由不得使用它** ——
// 那会把所有人的数据暴露给任何一个已登录用户。
func ScopeLocal() Scope { return Scope{} }

// Owner 返回期望的归属：[ScopeUser] 时是那个用户；[ScopeLocal] 时是 nil。
//
// 存储实现把它直接当参数传给 `(?n IS NULL OR owner = ?n)`：
// nil 让那一支恒真（不过滤），非 nil 则退化成等值比较。
func (s Scope) Owner() *UserID { return s.user }

// IsLocal 报告这是否是本地范围。
func (s Scope) IsLocal() bool { return s.user == nil }

// ─────────────────────────────────────────────────────────────────────────────
// 认领结果
// ─────────────────────────────────────────────────────────────────────────────

// ClaimedRows 是一次认领各表影响了多少行（见 [StateStore.ClaimUnowned]）。
//
// 不是"随便返回个总数"：`mcloud user init` 要把三笔分别打印出来
// （"认领了 3 个账号、2 个定时任务、0 个组合"）。只回一个总数等于把
// "历史数据到底有没有被接住"这件事糊成一句话 —— 而升级上来的用户正是靠它
// 判断"我的账号还在不在"。
type ClaimedRows struct {
	Accounts  uint64 `json:"accounts"`
	Schedules uint64 `json:"schedules"`
	Pipelines uint64 `json:"pipelines"`
}

// Total 返回三笔之和。
func (c ClaimedRows) Total() uint64 { return c.Accounts + c.Schedules + c.Pipelines }

// ─────────────────────────────────────────────────────────────────────────────
// StateStore
// ─────────────────────────────────────────────────────────────────────────────

// StateStore 是账号维度的状态存储。SQLite 实现在 `internal/io`。
//
// # 数据隔离是**契约的一部分**，不是调用方的义务
//
// 每个方法都要求传 [Scope]，实现**必须**按它过滤。[ScopeUser] 时三条规则：
//
//  1. **读不到就是 nil / 空列表**："不属于你"与"不存在"返回同一个结果，
//     实现里不要为前者单独造一个错误（理由见 [KindNotFound]）；
//  2. **写之前要校验，而不是先写进去再说**：传入不属于该用户的 [AccountID]
//     时实现应拒绝，而不是径直 UPDATE；
//  3. **`owner` 为空的历史账号对谁都不可见**（读不到、也不能被写）——
//     迁移完还没初始化管理员的那个窗口里，宁可是"账号暂时不出现"，
//     也不要出现"谁都拿得到"。
//
// 把这些写进类型（每个方法都要一个 [Scope]）而不是靠"记得加 WHERE 条件"，
// 是因为漏掉一处就是越权，而**漏掉一处不会有任何编译期提示**。
//
// # 本地范围不是"安全后门"
//
// [ScopeLocal] 只该被 CLI 这类**已经持有库文件**的调用方使用：
// 它不制造新的越权可能（能跑 CLI 的人本来就能直接读库），
// 但服务层用它就等于把所有人的数据交给任何一个已登录用户。
// 所以它是个**必须在调用点写出来的**选择，而不是默认值。
type StateStore interface {
	// UpsertAccount 写入账号。
	//
	// 归属检查随 [Scope] 走：[ScopeUser] 时 `account.Owner` 必须是同一个用户；
	// [ScopeLocal] 时不再约束归属。
	UpsertAccount(ctx context.Context, scope Scope, account *Account) error

	// GetAccount 取账号。不在该范围内时返回 nil，nil 错误。
	GetAccount(ctx context.Context, scope Scope, id AccountID) (*Account, error)

	// SetAccountCloud 记下账号**当前**的云朵余额（**窄写**：只动
	// `cloud_current` / `cloud_updated_at`）。
	//
	// # 为什么不复用 [StateStore.UpsertAccount]
	//
	// 那个方法是**整行覆盖** —— 想改余额就得"读整行 → 改一列 → 写回去"。
	// 而读与写之间别人改了昵称 / 启用状态，整行覆盖会把它**静默写回旧值**：
	// 一个只在列表页显示错名字的 bug，查起来毫无头绪。
	//
	// `at` 由调用方给（[Clock]），实现不自己取时间 —— 测试才能钉住。
	SetAccountCloud(ctx context.Context, scope Scope, id AccountID, count int64, at time.Time) error

	// ListAccounts 该范围内可见的账号，按 `sort_order` 升序。
	ListAccounts(ctx context.Context, scope Scope) ([]Account, error)

	// GetAccountState 读取账号登录态（含凭据）。
	//
	// 能力层要用它拿 `jwtToken` / `deviceId` 去调上游 —— 所以它属于
	// **能力层契约**。但注意：拿到的东西**不得**出现在日志、API 响应或错误信息里。
	GetAccountState(ctx context.Context, scope Scope, id AccountID) (*AccountState, error)

	// UpsertAccountState 覆盖写入账号登录态。
	//
	// 是**覆盖**而不是合并：登录态是一个整体快照，nil 表示"上游这次没给"，
	// 若做合并就永远删不掉已失效的 token。
	UpsertAccountState(ctx context.Context, scope Scope, state *AccountState) error

	// GetSmidState 读这个账号的 smid 状态（绑定的设备特征 + 最近拿到的 `deviceId`）。
	//
	// 与 [StateStore.GetAccountState] 分开：那条是"能冒充这个用户的凭据"，
	// 这条是"这台设备长什么样"。放在一起之后，任何一次取凭据都会顺手把
	// 特征也读进内存，而它跟凭据的泄露面完全不是一回事
	// （它可以大方地打在本地日志里排障）。
	//
	// 返回 nil 表示这个账号**一行状态都没有**（从没登录过也没绑过指纹）。
	GetSmidState(ctx context.Context, scope Scope, id AccountID) (*SmidState, error)

	// SetSmidFingerprint 写入**新的**设备特征，并把 `smid_device_id` 清空。
	//
	// # 为什么清空 deviceId 与写特征是**同一条语句**
	//
	// 换指纹的语义是"这台机器换个长相"。旧的 `deviceId` 是**旧长相**换来的
	// 那一个，留着它，下一次上报就等于自相矛盾：新的屏幕/ua/canvas，
	// 配的却是旧设备的 id —— 那正是风控服务最想看到的"设备在漂移"信号。
	//
	// 所以这两件事不能分成两次调用：中间崩溃会留下"新指纹 + 旧 id"，
	// 而且那种状态**看不出来**（两列各自都合法）。
	//
	// 入参是序列化后的特征文本：存储层不解析它，也就不会替调用方
	// 决定"读不出来时怎么办"。
	SetSmidFingerprint(ctx context.Context, scope Scope, id AccountID, fingerprintJSON string) error

	// SetSmidDeviceID 记下最近一次上报换回的 `deviceId`（含 `B` 前缀）。
	//
	// 只在**上报成功**之后调用：拿到的 id 会被下一次上报当作 `box` 带回去，
	// 一个失败的、或半截的响应值写进去，等于让这台设备从此自称一个来历不明的 id。
	//
	// **不**动 `updated_at`：那一列的语义是"登录态快照的时间"，
	// 一次指纹上报不是新快照。
	SetSmidDeviceID(ctx context.Context, scope Scope, id AccountID, deviceID string) error

	// FillAccountCredentials 把账号**缺**的 `authorization` / `phone` 两列填上，
	// **不动其余任何列**。
	//
	// # 是"填空缺"，不是"覆盖"
	//
	// 已经有值的那一列**原样保留**，入参给什么都不覆盖它。nil 表示
	// "这一列我没有可填的"。
	//
	// # 为什么是窄写而不是 [StateStore.UpsertAccountState]
	//
	// 那个方法按契约是**整体覆盖**。拿它来"补两列"会把 `jwt_token` /
	// `user_domain_id` / `cloud_phone_redpack_token` 一起抹掉 ——
	// 而那正是本方法要修的库所依赖的东西。
	//
	// # 为什么按状态判（"这一列空着就填"）而不是按源文件哈希判
	//
	// 后者对已经导入过的库永远不会再跑，会让同一个版本的程序产出两种库形态。
	// 填空缺的语义让回填天然幂等，于是"每次都跑"是安全的：
	// **重登过、库里存着更新凭据的账号不会被旧配置覆盖回去**。
	//
	// 所以它也**不动 `updated_at`** —— 补一个缺口不是新快照。
	FillAccountCredentials(ctx context.Context, scope Scope, id AccountID, authorization, phone *string) error

	// UpsertCloudStats 按 `account + date` UPSERT 日快照。
	UpsertCloudStats(ctx context.Context, scope Scope, stats *CloudStats) error

	// CloudStatsSince 取 `date >= since` 的快照，按日期升序。
	CloudStatsSince(ctx context.Context, scope Scope, id AccountID, since string) ([]CloudStats, error)

	// InsertRun 写运行记录。带账号时按范围校验归属。
	InsertRun(ctx context.Context, scope Scope, run *RunRecord) error

	// UpdateRun 更新运行记录。只能更新该范围内的记录。
	UpdateRun(ctx context.Context, scope Scope, run *RunRecord) error

	// ListRuns 该范围内可见的运行记录，按开始时间倒序。
	//
	// **`account_id` 为空的记录只在 [ScopeLocal] 下出现**：旧数据导入时
	// "匹配不到账号"的历史会由迁移检查报出来（那才是它该被看到的地方），
	// 而不是塞进某个用户的列表里冒充"他的历史"。
	ListRuns(ctx context.Context, scope Scope, limit int) ([]RunRecord, error)

	// AppendRunEvents 追加一批运行事件（落库，供重启后回看与断线续传）。
	//
	// # 为什么是"一批"而不是"一条"
	//
	// 调用方通常在一次上游交互之后手里已经攒了若干条
	// （一条 Progress + 几条 Log）。按条写的话，每条都是一次事务 + 一次 fsync，
	// 而事件量恰恰是在"20 个账号依次签到"这种场景下最大的。
	// 按批写同时还给了实现一个**天然的顺序边界**：批内的 seq 一定是连续的。
	//
	// # 幂等
	//
	// seq 由运行执行器统一分配，**同一批重复写不会产生重复行**
	// （表上 `(run_id, seq)` 是主键，实现应当用它做 `ON CONFLICT DO NOTHING`）。
	// 进程在"写完库、还没来得及告诉总线"时崩掉，重启后重放同一批是**正常路径**。
	//
	// # 范围
	//
	// 事件本身不带 owner，归属由它所属的 run 决定 ——
	// 所以实现必须**顺着 run 校验这次写入落在范围内**，
	// 否则一个用户能往别人的 run 里塞日志。
	AppendRunEvents(ctx context.Context, scope Scope, runID RunID, events []Sequenced) error

	// RunEventsAfter 读某个 run 的事件，seq 严格大于 after，按 seq 升序。
	//
	// after 为 nil 表示从头开始。
	//
	// # 这是 SSE 续传的落库侧
	//
	// 浏览器把 `Last-Event-ID`（就是上一个 seq）带回来，服务端就调这个方法。
	// 之所以不用"时间戳 + 分页"：同一毫秒内的两个事件无法区分先后，
	// 于是要么重复推、要么漏推，两者都会让前端显示一个错的进度。
	RunEventsAfter(ctx context.Context, scope Scope, runID RunID, after *Seq, limit int) ([]Sequenced, error)

	// DeleteRunEvents 删掉某个 run 的全部事件，返回删掉的行数。
	//
	// `run_events` 上**没有外键**（SQLite 的外键默认不生效，写了也是假的），
	// 所以清理必须显式做，否则事件表只增不减 —— 而它按"一个 run 几百条"增长，
	// 是这张库最先撑起来的一张表。
	//
	// 返回行数是因为调用方要靠它确认"确实清干净了"，而不是"没报错所以大概清了"。
	// 重复删除返回 0，**不是错误**：那就是"我要的状态已经达成了"。
	DeleteRunEvents(ctx context.Context, scope Scope, runID RunID) (int, error)

	// ClaimUnowned 把尚未归属的数据认领给某个用户：**账号、定时任务、组合一起**。
	//
	// # 为什么需要它
	//
	// v3 及更早的库里已经有这些数据，而 schema 迁移是纯 SQL、执行时管理员还不存在，
	// 所以它们的 owner 是空（谁都看不见）。初始化首个管理员之后必须把历史数据
	// 交还给它，否则升级上来的用户会看到"账号全没了"。
	//
	// **只认领 owner 为空的行**：已经属于别人的不会被抢走，重复调用返回 0。
	// 三张表的认领在**同一个事务**里完成 —— 半途而废会留下
	// "账号回来了、定时任务没回来"这种很难解释的状态。
	ClaimUnowned(ctx context.Context, userID UserID) (ClaimedRows, error)
}

// ─────────────────────────────────────────────────────────────────────────────
// UserStore
// ─────────────────────────────────────────────────────────────────────────────

// UserStore 是系统用户与会话的持久化。
//
// # 为什么与 [StateStore] 分开
//
// 理由比 [ConfigStore] 更强一层：**"能建用户"与"能读云盘凭据"不该长在同一个接口上**。
// HTTP 鉴权中间件的全部需求是"按名字查用户 + 按 token 哈希查会话"，
// 它不该因此顺带获得读 `jwt_token` 的方法 —— 分开之后，**类型系统**就挡住了这件事，
// 不需要靠 code review 的自觉。
//
// # 口令与会话的边界
//
//   - 口令只以 [HashPassword]（argon2id）的编码串形态存储；
//   - 会话只存 `sha256(token)`：明文只在签发那一刻返回一次，**不经过这一层**，
//     这样任何实现都不可能顺手把它写进日志。
type UserStore interface {
	// UpsertUser 新建或覆盖用户。
	//
	// 用户名**唯一且大小写不敏感**（由 schema 的 `COLLATE NOCASE` 保证）：
	// 否则 `Alice` 与 `alice` 能各建一个，登录时"匹配到谁"取决于查询顺序。
	UpsertUser(ctx context.Context, user *User) error

	// GetUserByName 按用户名查（登录路径）。大小写不敏感。
	GetUserByName(ctx context.Context, name string) (*User, error)

	GetUser(ctx context.Context, id UserID) (*User, error)

	// ListUsers 按创建时间排序的稳定列表。
	ListUsers(ctx context.Context) ([]User, error)

	// CountUsers 用户总数。
	//
	// 它是"**是否已完成初始化**"的判据：为 0 时 WebUI 只暴露初始化页、
	// 其余接口一律不响应（避免两套 bootstrap）。
	CountUsers(ctx context.Context) (uint64, error)

	// InsertSession 写入会话。入参含**已算好的哈希**（见 [HashSessionToken]）。
	InsertSession(ctx context.Context, session *Session) error

	// GetSession 按 token 哈希查会话。
	GetSession(ctx context.Context, tokenHash string) (*Session, error)

	// DeleteSession 删除单个会话（登出）。返回是否真的删掉了一行。
	DeleteSession(ctx context.Context, tokenHash string) (bool, error)

	// DeleteExpiredSessions 清理已过期会话，返回删除条数。
	//
	// **时间从外部传入**：会话到期是业务判断，不能由存储实现偷偷决定"现在几点"。
	DeleteExpiredSessions(ctx context.Context, now time.Time) (uint64, error)

	// DeleteUserSessions 删除某用户的全部会话（改口令 / 删用户时用）。
	DeleteUserSessions(ctx context.Context, userID UserID) (uint64, error)
}

// ─────────────────────────────────────────────────────────────────────────────
// ConfigStore
// ─────────────────────────────────────────────────────────────────────────────

// ConfigStore 是配置与调度类持久化。
//
// # 为什么与 [StateStore] 分开
//
// 两者的**使用者不同**：`StateStore` 服务能力层（跑任务时要读凭据、写余额、记运行），
// `ConfigStore` 服务服务层（WebUI 读写定时任务、组合、抢兑与界面偏好）。
// 合成一个胖接口会让"只想给能力写个内存假实现"的人被迫实现十几个用不到的方法，
// 而假实现正是本项目保证函数级可测的手段 —— 不能让它的成本反过来逼人不写测试。
type ConfigStore interface {
	// UpsertSchedule 新建或覆盖一个定时任务。
	//
	// # 写操作**也要**过归属检查（不只是读）
	//
	// 只过滤读的话，A 仍然能**覆盖** B 的任务：主键是 id，写入是 INSERT OR REPLACE，
	// 一次同 id 的写入就能改掉别人的那一行 —— 越权从"看得到"变成更坏的"改得掉"。
	// 所以 [ScopeUser] 时校验传入的 owner：属于别人 → [NewNotFound]。
	UpsertSchedule(ctx context.Context, scope Scope, schedule *Schedule) error

	GetSchedule(ctx context.Context, scope Scope, id string) (*Schedule, error)

	// ListSchedules 按创建时间/名称排序的稳定列表（只含 [Scope] 可见的行）。
	ListSchedules(ctx context.Context, scope Scope) ([]Schedule, error)

	// DeleteSchedule 删除；返回是否真的删掉了一行（不可见等价于不存在）。
	DeleteSchedule(ctx context.Context, scope Scope, id string) (bool, error)

	UpsertPipeline(ctx context.Context, scope Scope, pipeline *Pipeline) error

	GetPipeline(ctx context.Context, scope Scope, id string) (*Pipeline, error)

	ListPipelines(ctx context.Context, scope Scope) ([]Pipeline, error)

	// ExchangeConfig 取单行抢兑配置：**无归属**，保持全局。
	//
	// 不是漏了：`exchange_config` 被 schema 的 `CHECK (id = 1)` 钉成单行表，
	// 这本身就是"全局唯一一份"的设计。真要按用户分，那是**改语义**
	// （单行表 → 每用户一行），该单独做一次决定，而不是顺手加个 owner 列。
	//
	// **读不到就返回默认值**，不返回 nil：旧版就是这个语义
	// （文件不存在返回默认值、且不写盘），让每个调用点都去兜底只会把约定抄散。
	ExchangeConfig(ctx context.Context) (ExchangeConfig, error)

	PutExchangeConfig(ctx context.Context, config *ExchangeConfig) error

	WebuiConfig(ctx context.Context) (WebuiConfig, error)

	PutWebuiConfig(ctx context.Context, config *WebuiConfig) error
}
