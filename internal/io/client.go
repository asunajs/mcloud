package mcloudio

import (
	"compress/gzip"
	"compress/zlib"
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// Client 是真实 HTTP 客户端（net/http + 系统根证书）。
//
// 对应 Rust 侧 legacy/crates/mcloud-io/src/reqwest_client.rs。
//
// ⚠ 它**必须是本包的类型**。scripts/check-go-deps.sh 断言「组装根
// （app / server）必须依赖本包」，那条断言挡的是"没人接上 IO 实现"这个
// **静默死配置**：编译能过、跑起来什么都没发生。若组装层直接持
// *http.Client，那条断言就失去了意义。
//
// # 与 Rust 侧的一处**已知**差异：不声明 brotli
//
// Rust 侧 reqwest 开了 `gzip` / `brotli` / `deflate` 三个 feature，
// Go 标准库**只有** gzip 与 deflate（[compress/zlib]），没有 brotli。
// ⇒ 本实现只声明 `gzip, deflate`：
//
//	声明了却解不开  → 响应变成一堆乱码，且错误指不到源头；
//	不声明          → 只是多花一点带宽。
//
// 两条路里只有后者是安全的。真要接 brotli 就是引入一个第三方解码器，
// 那是一个选型决定，不是顺手能补的。
type Client struct {
	http   *http.Client
	policy RetryPolicy
}

// ─────────────────────────────────────────────────────────────────────────────
// 重试策略
// ─────────────────────────────────────────────────────────────────────────────

// RetryPolicy 是重试策略。
type RetryPolicy struct {
	// MaxAttempts 是最大尝试次数（**含首次**）。1 = 不重试。
	MaxAttempts uint32
	// BaseDelay 是首次退避基数。
	BaseDelay time.Duration
	// MaxDelay 是退避上限。
	MaxDelay time.Duration
}

// DefaultRetryPolicy 是默认策略，逐项对齐 Rust 侧的 `impl Default`。
func DefaultRetryPolicy() RetryPolicy {
	return RetryPolicy{
		MaxAttempts: 3,
		BaseDelay:   300 * time.Millisecond,
		MaxDelay:    5 * time.Second,
	}
}

// DelayFor 是第 attempt 次失败后应等待多久（attempt 从 1 开始）。
//
// 指数退避 + 抖动，抑制多账号同时重试造成的同步冲击 ——
// 那种同步冲击**本身就是风控诱因**，不是"重试太频繁"这么简单。
//
// 抖动落在 [raw/2, raw)：Rust 侧用系统纳秒做扰动源（避免额外引入 rand 依赖），
// 这里同口径用 [time.Time.Nanosecond]。
//
// ⚠ 上限是**硬**的：`attempt` 极大时（含 `math.MaxUint32`）也必须 ≤ MaxDelay。
// 那条由 `TestBackoffNeverExceedsMaxDelayAtExtremeAttempts` 钉住 ——
// 指数退避一旦溢出，"退避"就变成"睡到明年"。
func (p RetryPolicy) DelayFor(attempt uint32) time.Duration {
	// Rust 侧是 `attempt.saturating_sub(1).min(16)`：第 1 次失败后就是 base_delay。
	// ⚠ 别写成 `if attempt > 1 { attempt-- }` —— 那会让第 1 次的指数变成 1，
	// 首退避直接翻倍（实测 175ms 而不是该有的 [50,100)ms）。
	exponent := attempt - 1
	if attempt == 0 {
		exponent = 0
	}
	if exponent > 16 {
		exponent = 16
	}
	raw := p.BaseDelay * time.Duration(uint64(1)<<exponent)
	if raw > p.MaxDelay || raw < 0 {
		raw = p.MaxDelay
	}
	half := raw / 2
	if half <= 0 {
		return raw
	}
	jitter := time.Duration(time.Now().Nanosecond()) % half
	return half + jitter
}

// ─────────────────────────────────────────────────────────────────────────────
// 构造
// ─────────────────────────────────────────────────────────────────────────────

// NewClient 返回一个默认配置的客户端。
func NewClient() *Client { return NewClientWithPolicy(DefaultRetryPolicy()) }

// NewClientWithPolicy 按给定策略构造。
//
// `policy.MaxAttempts = 1` 即等价于"不重试"，调试时很有用。
//
// ⚠ Transport 上刻意关了 [net/http] 的自动解压
// （`DisableCompression: true`）：自动解压只在"Go 自己加了 Accept-Encoding"
// 时才发生，而我们这里**自己管**这个头（要能只声明 gzip 与 deflate）。
// 两套机制同时开着会出现"解了两次"或"一次都没解"，两种都是乱码。
func NewClientWithPolicy(policy RetryPolicy) *Client {
	transport := &http.Transport{
		DisableCompression: true,
		// 与 Rust 侧 reqwest 的默认连接池同量级；这里显式写是为了让
		// "每个账号一个连接"这种用法不会悄悄退化成每次握手。
		MaxIdleConns:        64,
		MaxIdleConnsPerHost: 8,
		IdleConnTimeout:     90 * time.Second,
	}
	return &Client{
		http: &http.Client{
			Transport: transport,
			Timeout:   20 * time.Second,
		},
		policy: policy,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送
// ─────────────────────────────────────────────────────────────────────────────

// Send 发一个请求，**带重试**。
//
// # 重试的两个条件必须同时成立
//
//   - 错误类别本身可重试（[core.ErrorKind.IsRetryable]）；
//   - 请求可重试（[core.HTTPRequest.IsRetryable]）—— 写请求必须带幂等键。
//
// 缺第二个条件会把"写请求重放了"变成一种正常现象，而它在账目上很难追。
func (c *Client) Send(ctx context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	retryableRequest := request.IsRetryable()
	attempts := c.policy.MaxAttempts
	if attempts == 0 {
		attempts = 1
	}

	var last *core.Error
	for attempt := uint32(1); attempt <= attempts; attempt++ {
		response, perr := c.sendOnce(ctx, request)
		if perr == nil && response.IsSuccess() {
			return response, nil
		}
		if perr == nil {
			perr = statusError(response)
		}
		if !(perr.Kind().IsRetryable() && retryableRequest) {
			return response, perr
		}
		last = perr
		if attempt < attempts {
			time.Sleep(c.policy.DelayFor(attempt))
		}
	}
	if last == nil {
		last = core.NewInternal("重试耗尽但无错误记录")
	}
	return core.HTTPResponse{}, last
}

// sendOnce 发一次，不做重试也不判状态码。
func (c *Client) sendOnce(ctx context.Context, request core.HTTPRequest) (core.HTTPResponse, *core.Error) {
	if request.TimeoutMS != nil && *request.TimeoutMS > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, time.Duration(*request.TimeoutMS)*time.Millisecond)
		defer cancel()
	}

	var body io.Reader
	if len(request.Body) > 0 {
		body = strings.NewReader(string(request.Body))
	}
	httpRequest, err := http.NewRequestWithContext(ctx, httpMethodOf(request.Method), request.URL, body)
	if err != nil {
		return core.HTTPResponse{}, core.NewBadRequest("构造请求失败: " + err.Error())
	}
	for _, pair := range request.Headers {
		httpRequest.Header.Add(pair[0], pair[1])
	}
	if httpRequest.Header.Get("Accept-Encoding") == "" {
		httpRequest.Header.Set("Accept-Encoding", "gzip, deflate")
	}

	response, err := c.http.Do(httpRequest)
	if err != nil {
		return core.HTTPResponse{}, mapTransportError(err)
	}
	defer func() { _ = response.Body.Close() }()

	raw, perr := decodeBody(response)
	if perr != nil {
		return core.HTTPResponse{}, perr
	}
	return core.HTTPResponse{
		Status:  uint16(response.StatusCode),
		Headers: flattenHeaders(response.Header),
		Body:    raw,
	}, nil
}

// statusError 把非 2xx 的响应映射成错误。
//
// ⚠ 5xx 那条把**响应体**放进内部消息（与 Rust 侧一致）：
// 上游的 5xx 常常带着 traceId，那是排障唯一能拿到的线索。
// 它只进 [core.Error.Message]（内部日志），[core.Error.PublicMessage]
// 是固定的"上游服务异常"，所以不会外泄到 API 响应里。
func statusError(response core.HTTPResponse) *core.Error {
	status := strconv.Itoa(int(response.Status))
	switch core.ClassifyStatus(response.Status) {
	case core.KindAuth:
		return core.NewAuth("HTTP " + status)
	case core.KindRateLimited:
		return core.NewRateLimited("HTTP " + status)
	case core.KindTimeout:
		return core.NewTimeout("HTTP " + status)
	case core.KindUpstream:
		return core.NewUpstream(response.Status, string(response.Body))
	}
	return core.NewBadRequest("HTTP " + status)
}

// mapTransportError 把传输层错误归类。
//
// 只有超时值得单独分类（它是"可能已到达上游"的语义，退避策略不同）；
// 连接失败、请求构造失败、TLS 握手失败对调用方而言都是"网络异常"，
// 因此统一归到 Network，不再细分 —— 分成同一处理结果的两个分支没有意义。
func mapTransportError(err error) *core.Error {
	var netErr net.Error
	if errors.As(err, &netErr) && netErr.Timeout() {
		return core.NewTimeout(err.Error())
	}
	// ⚠ **双保险，不是必需的那一层**（这句话是实测出来的，别想当然）：
	// 在 Go 1.26 上 `errors.As(context.DeadlineExceeded, &netErr)` **已经为真**，
	// 且它的 `Timeout()` 返回 true —— 也就是上面那条分支已经接住了 ctx 超时。
	// ⇒ 删掉下面这几行**不会让任何测试变红**（反向测试已确认）。
	// 留着的理由是"超时"与"网络异常"的退避策略不同、判错代价不小，
	// 而这个行为**依赖具体 Go 版本**（早年它并不满足 net.Error）。
	if errors.Is(err, context.DeadlineExceeded) {
		return core.NewTimeout(err.Error())
	}
	return core.NewNetwork(err.Error())
}

func httpMethodOf(method core.HTTPMethod) string {
	switch method {
	case core.MethodGet:
		return http.MethodGet
	case core.MethodPost:
		return http.MethodPost
	case core.MethodPut:
		return http.MethodPut
	case core.MethodDelete:
		return http.MethodDelete
	}
	return http.MethodGet
}

// decodeBody 按 Content-Encoding 解压。
//
// 只认 gzip 与 deflate（理由见 [Client] 的说明）。未知或缺失的编码**原样返回** ——
// 那本来就是绝大多数响应的情况。
func decodeBody(response *http.Response) ([]byte, *core.Error) {
	encoding := strings.ToLower(strings.TrimSpace(response.Header.Get("Content-Encoding")))
	raw, err := io.ReadAll(response.Body)
	if err != nil {
		return nil, core.NewNetwork("读响应体失败: " + err.Error())
	}
	switch encoding {
	case "", "identity":
		return raw, nil
	case "gzip":
		reader, err := gzip.NewReader(strings.NewReader(string(raw)))
		if err != nil {
			return nil, core.NewParse("gzip 解压失败: " + err.Error())
		}
		defer func() { _ = reader.Close() }()
		return readAllDecoded(reader, "gzip")
	case "deflate":
		reader, err := zlib.NewReader(strings.NewReader(string(raw)))
		if err != nil {
			return nil, core.NewParse("deflate 解压失败: " + err.Error())
		}
		defer func() { _ = reader.Close() }()
		return readAllDecoded(reader, "deflate")
	}
	return nil, core.NewParse("不支持的 Content-Encoding: " + encoding)
}

func readAllDecoded(reader io.Reader, what string) ([]byte, *core.Error) {
	out, err := io.ReadAll(reader)
	if err != nil {
		return nil, core.NewParse(what + " 解压失败: " + err.Error())
	}
	return out, nil
}

// flattenHeaders 把响应头摊平。
//
// ⚠ 同名头**全部**保留（Rust 侧的 `headers.iter()` 也是这样）：
// `Set-Cookie` 会重复出现，只留一个等于丢掉会话。
func flattenHeaders(header http.Header) []core.HeaderPair {
	count := 0
	for _, values := range header {
		count += len(values)
	}
	out := make([]core.HeaderPair, 0, count)
	for name, values := range header {
		for _, value := range values {
			out = append(out, core.HeaderPair{name, value})
		}
	}
	return out
}
