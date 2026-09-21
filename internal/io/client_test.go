package mcloudio

import (
	"compress/gzip"
	"compress/zlib"
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 退避
// ─────────────────────────────────────────────────────────────────────────────

// TestBackoffGrowsAndIsCapped：指数增长，且有硬上限。
func TestBackoffGrowsAndIsCapped(t *testing.T) {
	policy := RetryPolicy{
		MaxAttempts: 10,
		BaseDelay:   100 * time.Millisecond,
		MaxDelay:    time.Second,
	}
	d1 := policy.DelayFor(1)
	d3 := policy.DelayFor(3)
	d9 := policy.DelayFor(9)

	// 抖动落在 [raw/2, raw)
	if d1 < 50*time.Millisecond || d1 >= 100*time.Millisecond {
		t.Errorf("第 1 次应落在 [50ms,100ms)，实得 %v", d1)
	}
	if d3 < 200*time.Millisecond || d3 >= 400*time.Millisecond {
		t.Errorf("第 3 次应落在 [200ms,400ms)，实得 %v", d3)
	}
	if d9 > time.Second {
		t.Errorf("退避必须有上限，实得 %v", d9)
	}
}

// TestBackoffNeverExceedsMaxDelayAtExtremeAttempts：
// `attempt` 极大时（含 MaxUint32）也必须 ≤ MaxDelay ——
// 指数退避一旦溢出，"退避"就变成"睡到明年"。
func TestBackoffNeverExceedsMaxDelayAtExtremeAttempts(t *testing.T) {
	policy := DefaultRetryPolicy()
	for _, attempt := range []uint32{1, 5, 17, 64, ^uint32(0)} {
		if got := policy.DelayFor(attempt); got > policy.MaxDelay {
			t.Errorf("attempt=%d 超过上限: %v > %v", attempt, got, policy.MaxDelay)
		}
	}
}

// TestNoRetryPolicyEffectivelyDisablesRetries：`MaxAttempts = 1` 即不重试。
func TestNoRetryPolicyEffectivelyDisablesRetries(t *testing.T) {
	policy := RetryPolicy{MaxAttempts: 1, BaseDelay: time.Millisecond, MaxDelay: time.Millisecond}
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls++
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := NewClientWithPolicy(policy)
	_, err := client.Send(context.Background(), core.NewRequestGet(server.URL))
	if err == nil {
		t.Fatal("500 应报错")
	}
	if calls != 1 {
		t.Errorf("不重试就只该发一次，实得 %d", calls)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 重试的两个条件
// ─────────────────────────────────────────────────────────────────────────────

// TestReadRequestsRetryButWritesNeedAnIdempotencyKey：
// 写请求**不带幂等键就不许重试** —— 重放一个写请求在账目上很难追。
func TestReadRequestsRetryButWritesNeedAnIdempotencyKey(t *testing.T) {
	fast := RetryPolicy{MaxAttempts: 3, BaseDelay: time.Millisecond, MaxDelay: 2 * time.Millisecond}

	t.Run("读请求重试", func(t *testing.T) {
		calls := 0
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			calls++
			w.WriteHeader(http.StatusInternalServerError)
		}))
		defer server.Close()
		_, _ = NewClientWithPolicy(fast).Send(context.Background(), core.NewRequestGet(server.URL))
		if calls != 3 {
			t.Errorf("读请求（可重试类别）应重试到上限，实得 %d 次", calls)
		}
	})

	t.Run("写请求无幂等键不重试", func(t *testing.T) {
		calls := 0
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			calls++
			w.WriteHeader(http.StatusInternalServerError)
		}))
		defer server.Close()
		_, _ = NewClientWithPolicy(fast).Send(context.Background(), core.NewRequestPost(server.URL))
		if calls != 1 {
			t.Errorf("写请求不带幂等键就一次都不该重放，实得 %d 次", calls)
		}
	})

	t.Run("写请求带幂等键才重试", func(t *testing.T) {
		calls := 0
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			calls++
			w.WriteHeader(http.StatusInternalServerError)
		}))
		defer server.Close()
		request := core.NewRequestPost(server.URL).WithIdempotencyKey("run-1")
		_, _ = NewClientWithPolicy(fast).Send(context.Background(), request)
		if calls != 3 {
			t.Errorf("带幂等键的写请求应重试，实得 %d 次", calls)
		}
	})
}

// TestNonRetryableStatusesAreNotRetried：4xx（除 408/429）不该退避重试。
func TestNonRetryableStatusesAreNotRetried(t *testing.T) {
	fast := RetryPolicy{MaxAttempts: 3, BaseDelay: time.Millisecond, MaxDelay: 2 * time.Millisecond}
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		calls++
		w.WriteHeader(http.StatusUnauthorized)
	}))
	defer server.Close()

	_, err := NewClientWithPolicy(fast).Send(context.Background(), core.NewRequestGet(server.URL))
	if err == nil {
		t.Fatal("401 应报错")
	}
	if calls != 1 {
		t.Errorf("401 不该重试（该去重新登录，不是多试几次），实得 %d 次", calls)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 错误分类
// ─────────────────────────────────────────────────────────────────────────────

// TestStatusIsClassifiedTheSameWayCoreDoes：
// 客户端不许自己重造一套状态码分类 —— 重试策略完全由 core 的那个函数决定。
func TestStatusIsClassifiedTheSameWayCoreDoes(t *testing.T) {
	cases := []struct {
		status uint16
		kind   core.ErrorKind
	}{
		{401, core.KindAuth},
		{403, core.KindAuth},
		{408, core.KindTimeout},
		{429, core.KindRateLimited},
		{422, core.KindBadRequest},
		{500, core.KindUpstream},
		{503, core.KindUpstream},
	}
	for _, c := range cases {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(int(c.status))
		}))
		client := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1})
		_, err := client.Send(context.Background(), core.NewRequestGet(server.URL))
		server.Close()
		if err == nil {
			t.Errorf("%d 应报错", c.status)
			continue
		}
		perr, ok := err.(*core.Error)
		if !ok {
			t.Errorf("%d 应返回 *core.Error，实得 %T", c.status, err)
			continue
		}
		if perr.Kind() != c.kind {
			t.Errorf("%d 应归为 %v，实得 %v", c.status, c.kind, perr.Kind())
		}
		// ⚠ 对外文案必须是固定那句，不能把上游原文漏出去
		if strings.Contains(perr.PublicMessage(), "HTTP") {
			t.Errorf("%d 的对外文案不该带内部细节，实得 %q", c.status, perr.PublicMessage())
		}
	}
}

// TestUpstreamBodyIsKeptForDiagnosticsButNeverPublished：
// 5xx 的响应体进**内部消息**（traceId 是排障唯一线索），但**不进**对外文案。
func TestUpstreamBodyIsKeptForDiagnosticsButNeverPublished(t *testing.T) {
	const trace = `{"traceId":"abc-123","msg":"internal"}`
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = w.Write([]byte(trace))
	}))
	defer server.Close()

	_, err := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).
		Send(context.Background(), core.NewRequestGet(server.URL))
	perr, ok := err.(*core.Error)
	if !ok {
		t.Fatalf("应是 *core.Error，实得 %T", err)
	}
	if !strings.Contains(perr.Message(), "abc-123") {
		t.Errorf("内部消息应留住 traceId，实得 %q", perr.Message())
	}
	if strings.Contains(perr.PublicMessage(), "abc-123") {
		t.Errorf("对外文案不该带上游原文，实得 %q", perr.PublicMessage())
	}
	if perr.Status() != 500 {
		t.Errorf("Status 应保留，实得 %d", perr.Status())
	}
}

// TestTransportErrorClassification 把"什么算超时"钉在**函数**上，
// 而不是钉在"走了哪条分支"上 ——
// 后者会随 Go 版本变（`context.DeadlineExceeded` 是否满足 `net.Error`
// 是版本相关的，见 mapTransportError 里的注释）。
func TestTransportErrorClassification(t *testing.T) {
	if got := mapTransportError(context.DeadlineExceeded); got.Kind() != core.KindTimeout {
		t.Errorf("ctx 超时必须归为 Timeout（退避策略不同），实得 %v", got.Kind())
	}
	if got := mapTransportError(errors.New("boom")); got.Kind() != core.KindNetwork {
		t.Errorf("普通传输错误应归为 Network，实得 %v", got.Kind())
	}
}

// TestRequestTimeoutIsATimeoutNotANetworkError：
// 端到端再走一次：请求级超时走 context，最终仍要归成 Timeout。
func TestRequestTimeoutIsATimeoutNotANetworkError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		time.Sleep(200 * time.Millisecond)
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	request := core.NewRequestGet(server.URL).WithTimeoutMS(10)
	_, err := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).Send(context.Background(), request)
	perr, ok := err.(*core.Error)
	if !ok {
		t.Fatalf("应是 *core.Error，实得 %T", err)
	}
	if perr.Kind() != core.KindTimeout {
		t.Errorf("请求级超时应归为 Timeout，实得 %v（%s）", perr.Kind(), perr.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解压
// ─────────────────────────────────────────────────────────────────────────────

// TestGzipAndDeflateAreDecoded：Go 不会"自动"帮我们解（我们自己管 Accept-Encoding）。
func TestGzipAndDeflateAreDecoded(t *testing.T) {
	const want = `{"code":0,"msg":"success"}`

	t.Run("gzip", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Encoding", "gzip")
			writer := gzip.NewWriter(w)
			_, _ = writer.Write([]byte(want))
			_ = writer.Close()
		}))
		defer server.Close()
		got := sendAndRead(t, server.URL)
		if got != want {
			t.Errorf("gzip 应解开，实得 %q", got)
		}
	})

	t.Run("deflate", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Encoding", "deflate")
			writer := zlib.NewWriter(w)
			_, _ = writer.Write([]byte(want))
			_ = writer.Close()
		}))
		defer server.Close()
		got := sendAndRead(t, server.URL)
		if got != want {
			t.Errorf("deflate 应解开，实得 %q", got)
		}
	})

	t.Run("无编码原样返回", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			_, _ = w.Write([]byte(want))
		}))
		defer server.Close()
		if got := sendAndRead(t, server.URL); got != want {
			t.Errorf("无编码应原样返回，实得 %q", got)
		}
	})
}

// TestBrotliIsNotAdvertised：标准库没有 brotli ⇒ 我们**不声明**它。
//
// 声明了却解不开会把响应变成一堆乱码；不声明只是多花一点带宽。
func TestBrotliIsNotAdvertised(t *testing.T) {
	seen := ""
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen = r.Header.Get("Accept-Encoding")
		_, _ = w.Write([]byte("{}"))
	}))
	defer server.Close()
	_ = sendAndRead(t, server.URL)

	if strings.Contains(strings.ToLower(seen), "br") {
		t.Errorf("不该声明 brotli（标准库解不开它），实得 %q", seen)
	}
	if !strings.Contains(seen, "gzip") {
		t.Errorf("应声明 gzip，实得 %q", seen)
	}
}

// TestAnUnsupportedEncodingIsAnErrorNotGarbage：
// 解不开就报错 —— 悄悄把乱码当响应交出去，错误会指到完全无关的下一层。
func TestAnUnsupportedEncodingIsAnErrorNotGarbage(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Encoding", "br")
		_, _ = w.Write([]byte{0x0b, 0x00, 0x00})
	}))
	defer server.Close()

	_, err := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).
		Send(context.Background(), core.NewRequestGet(server.URL))
	if err == nil {
		t.Fatal("不支持的编码应报错，而不是把乱码交上去")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 头
// ─────────────────────────────────────────────────────────────────────────────

// TestRepeatedHeadersAreAllKept：`Set-Cookie` 会重复出现，只留一个等于丢掉会话。
func TestRepeatedHeadersAreAllKept(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Add("Set-Cookie", "a=1; Path=/")
		w.Header().Add("Set-Cookie", "b=2; Path=/")
		_, _ = w.Write([]byte("{}"))
	}))
	defer server.Close()

	response, err := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).
		Send(context.Background(), core.NewRequestGet(server.URL))
	if err != nil {
		t.Fatalf("发送失败: %v", err)
	}
	cookies := 0
	for _, pair := range response.Headers {
		if strings.EqualFold(pair[0], "Set-Cookie") {
			cookies++
		}
	}
	if cookies != 2 {
		t.Errorf("两个 Set-Cookie 都该保留，实得 %d", cookies)
	}
}

// TestRequestHeadersAreForwarded：请求头逐条带出去，包括重复的同名头。
func TestRequestHeadersAreForwarded(t *testing.T) {
	var got []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got = r.Header.Values("x-dup")
		_, _ = w.Write([]byte("{}"))
	}))
	defer server.Close()

	request := core.NewRequestGet(server.URL).
		WithHeader("x-dup", "one").
		WithHeader("x-dup", "two")
	_, _ = NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).Send(context.Background(), request)
	if len(got) != 2 {
		t.Errorf("两个同名头都该发出去，实得 %v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 系统随机源与时钟
// ─────────────────────────────────────────────────────────────────────────────

// TestSystemRandomProducesDistinctValues：两次取值不应相同。
func TestSystemRandomProducesDistinctValues(t *testing.T) {
	var rng core.Random = SystemRandom{}
	a := core.UUIDv4(rng)
	b := core.UUIDv4(rng)
	if a == b {
		t.Errorf("两次 UUID 不应相同: %s", a)
	}
	if len(a) != 36 {
		t.Errorf("UUID 应是 36 位，实得 %d", len(a))
	}
	if a[14] != '4' {
		t.Errorf("版本位应为 4，实得 %q", a[14])
	}
}

// TestSystemRandomFillsBuffer：全零说明随机源没有真正工作。
func TestSystemRandomFillsBuffer(t *testing.T) {
	var buf [32]byte
	SystemRandom{}.FillBytes(buf[:])
	allZero := true
	for _, b := range buf {
		if b != 0 {
			allZero = false
			break
		}
	}
	if allZero {
		t.Error("全零说明随机源没有真正工作")
	}
}

// TestSystemClockIsUTC：[core.Clock] 的契约要求实现返回 UTC。
func TestSystemClockIsUTC(t *testing.T) {
	now := SystemClock{}.Now()
	if now.Location() != time.UTC {
		t.Errorf("应归一化到 UTC，实得 %v", now.Location())
	}
	a := SystemClock{}.Now()
	b := SystemClock{}.Now()
	if b.Before(a) {
		t.Error("时钟不应倒退")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

func sendAndRead(t *testing.T, url string) string {
	t.Helper()
	response, err := NewClientWithPolicy(RetryPolicy{MaxAttempts: 1}).
		Send(context.Background(), core.NewRequestGet(url))
	if err != nil {
		t.Fatalf("发送失败: %v", err)
	}
	return string(response.Body)
}
