package core

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/golden"
)

// 本文件是 internal/core/traits.go 的消费者。
//
// 期望值分两路：
//   - **行为**逐条平移 Rust 侧 traits.rs 的那 5 条测试；
//   - **形状**全部来自第十份黄金向量 test/golden/traits-shape-vectors.json，
//     那份由 **path 依赖真实 mcloud-core 后让 serde 产出**
//     （生成器 = scripts/gen-traits-shape-vectors.rs）。
//
// ⚠ 形状这条路是**跨语言对拍**：生成侧（serde）与消费侧（本文件）必须是两份实现。
// 这里尤其必要 —— `HTTPRequest` 的注释写着"golden request 对拍依赖它逐字段可比"，
// 而 Rust 侧对它的形状一个断言都没有（只有一条 HttpMethod 的单变体断言）。
//
// ⚠ 取向量函数的文件名必须写成**字面量**：门禁靠扫源码里的字符串判断
// "这份向量有没有人读"，藏进变量会让门禁静默失配、把活向量标成死向量。

type traitsVectors struct {
	EnumText []struct {
		Enum    string `json:"enum"`
		Variant string `json:"variant"`
		Text    string `json:"text"`
		AsStr   string `json:"asStr"`
		IsRead  bool   `json:"isRead"`
	} `json:"$enumText"`
	StructJSON []struct {
		Kind string `json:"kind"`
		JSON string `json:"json"`
	} `json:"$structJson"`
	Decode []struct {
		Type string `json:"type"`
		JSON string `json:"json"`
		OK   bool   `json:"ok"`
	} `json:"$decode"`
	RoundTrip []struct {
		Input string `json:"input"`
		JSON  string `json:"json"`
		Same  bool   `json:"same"`
	} `json:"$roundTrip"`
}

func loadTraitsVectors(t *testing.T) traitsVectors {
	t.Helper()
	raw, err := golden.Load("traits-shape-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	var v traitsVectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	if len(v.EnumText) == 0 || len(v.StructJSON) == 0 || len(v.Decode) == 0 || len(v.RoundTrip) == 0 {
		t.Fatalf("黄金向量缺段：enumText=%d struct=%d decode=%d roundTrip=%d",
			len(v.EnumText), len(v.StructJSON), len(v.Decode), len(v.RoundTrip))
	}
	return v
}

// ── 形状对拍 ─────────────────────────────────────────────────────────────────

// TestHTTPMethodShapeMatchesSerdeVectors 逐个变体比对文本、as_str 与 is_read。
func TestHTTPMethodShapeMatchesSerdeVectors(t *testing.T) {
	v := loadTraitsVectors(t)
	seen := 0
	for _, c := range v.EnumText {
		// ⚠ 用 Rust 侧的类型名（向量由 serde 产出），Go 侧叫 `HTTPMethod`。
		if c.Enum != "HttpMethod" {
			t.Fatalf("向量里有没人认领的枚举 %q —— 加一条就得多写一个断言，别让它静默跳过", c.Enum)
		}
		seen++
		m := enumByVariant(t, "HttpMethod", c.Variant).(HTTPMethod)
		raw, err := marshalJSON(m)
		if err != nil {
			t.Fatalf("%s 序列化失败: %v", c.Variant, err)
		}
		if got := strings.Trim(string(raw), `"`); got != c.Text {
			t.Fatalf("%s 的文本与 serde 不一致：实得 %q，期望 %q", c.Variant, got, c.Text)
		}
		if m.String() != c.AsStr {
			t.Fatalf("%s 的 String() 与 Rust 的 as_str 不一致：实得 %q，期望 %q",
				c.Variant, m.String(), c.AsStr)
		}
		if m.IsRead() != c.IsRead {
			t.Fatalf("%s 的 IsRead 与 Rust 的 is_read 不一致：实得 %v，期望 %v",
				c.Variant, m.IsRead(), c.IsRead)
		}
	}
	if seen != len(v.EnumText) || seen == 0 {
		t.Fatalf("认领了 %d 条、向量有 %d 条 —— 两边必须一一对应", seen, len(v.EnumText))
	}
}

// TestHTTPStructShapesMatchSerdeVectors 逐条**整串**比对。
//
// ⚠ 整串而不是逐字段：字段顺序、`[]` vs `null`、body 是数字数组还是 base64，
// 全在串里。逐字段比对会把它们全部放过。
func TestHTTPStructShapesMatchSerdeVectors(t *testing.T) {
	v := loadTraitsVectors(t)

	want := map[string]HTTPish{}
	add := func(kind string, value HTTPish) { want[kind] = value }

	add("request-get-minimal", requestish(NewRequestGet("https://example.com/a")))
	add("request-post-headers", requestish(NewRequestPost("https://example.com/b").
		WithHeader("x-a", "1").WithHeader("x-b", "2")))

	body1, err := NewRequestPost("https://example.com/c").
		WithJSONBody(map[string]any{"phone": "19900001234", "n": 1})
	if err != nil {
		t.Fatalf("构造请求体失败: %v", err)
	}
	add("request-json-body", requestish(body1))

	body2, err := NewRequestPost("https://example.com/d").
		WithHeader("Content-Type", "application/x-www-form-urlencoded").
		WithJSONBody(map[string]any{"a": 1})
	if err != nil {
		t.Fatalf("构造请求体失败: %v", err)
	}
	add("request-content-type-kept", requestish(body2))

	body3, err := NewRequestPost("https://example.com/e").
		WithHeader("x-trace", "t-1").
		WithJSONBody(map[string]any{"a": 1})
	if err != nil {
		t.Fatalf("构造请求体失败: %v", err)
	}
	add("request-full", requestish(body3.WithTimeoutMS(3000).WithIdempotencyKey("run-1")))

	add("response-200", responseish(NewResponse(200, []byte(`{"a":1}`))))
	add("response-empty-body", responseish(HTTPResponse{
		Status:  404,
		Headers: []HeaderPair{{"content-type", "text/plain"}},
		Body:    Body{},
	}))

	for _, c := range v.StructJSON {
		w, ok := want[c.Kind]
		if !ok {
			t.Fatalf("向量里有一条没人认领的 kind %q —— 加一条就得多写一个断言，别让它静默跳过", c.Kind)
		}
		got, err := marshalJSON(w.Value)
		if err != nil {
			t.Fatalf("%s 序列化失败: %v", c.Kind, err)
		}
		if string(got) != c.JSON {
			t.Fatalf("%s 与 serde 的输出不一致:\n实得 %s\n期望 %s", c.Kind, got, c.JSON)
		}
	}
	if len(want) != len(v.StructJSON) {
		t.Fatalf("向量有 %d 条、Go 侧认领了 %d 条 —— 两边必须一一对应",
			len(v.StructJSON), len(want))
	}
}

// TestHTTPDecodeMatchesSerdeVectors 比对"哪些输入 serde 收、哪些拒"。
//
// ⚠ 这一节的价值集中在几条反直觉的实测结果上：
//   - `"get"` / `"Get"` **拒**（UPPERCASE 也约束解码）；
//   - `headers: null` **拒**，而 `body: null` **收**
//     （`#[serde(default)]` 只管缺席、不管显式 null；`Option` 则相反）；
//   - `body` 给 base64 串 **拒**（它是 `Vec<u8>`，只认数字数组）。
func TestHTTPDecodeMatchesSerdeVectors(t *testing.T) {
	v := loadTraitsVectors(t)
	for _, c := range v.Decode {
		var err error
		switch c.Type {
		case "HttpMethod":
			var m HTTPMethod
			err = json.Unmarshal([]byte(c.JSON), &m)
		case "HttpRequest":
			var r HTTPRequest
			err = json.Unmarshal([]byte(c.JSON), &r)
		case "HttpResponse":
			var r HTTPResponse
			err = json.Unmarshal([]byte(c.JSON), &r)
		default:
			t.Fatalf("向量里有没人认领的类型 %q", c.Type)
		}
		if c.OK && err != nil {
			t.Fatalf("%s: serde 收，Go 侧却拒了（%v）: %s", c.Type, err, c.JSON)
		}
		if !c.OK && err == nil {
			t.Fatalf("%s: serde 拒，Go 侧却收了: %s", c.Type, c.JSON)
		}
	}
}

// TestHTTPRoundTripNormalizesAbsentSlices 比对"解回来再序列化"的形状。
//
// Rust 的 `#[serde(default)] Vec` 缺席时是 `vec![]` ⇒ 再序列化**多出** `[]`；
// Go 的 nil slice 会输出 `null`。两侧不同形，靠 UnmarshalJSON 归一化补齐。
func TestHTTPRoundTripNormalizesAbsentSlices(t *testing.T) {
	v := loadTraitsVectors(t)
	for _, c := range v.RoundTrip {
		var out any
		if strings.Contains(c.Input, "method") {
			var r HTTPRequest
			if err := json.Unmarshal([]byte(c.Input), &r); err != nil {
				t.Fatalf("解 %s 失败: %v", c.Input, err)
			}
			out = r
		} else {
			var r HTTPResponse
			if err := json.Unmarshal([]byte(c.Input), &r); err != nil {
				t.Fatalf("解 %s 失败: %v", c.Input, err)
			}
			out = r
		}
		got, err := marshalJSON(out)
		if err != nil {
			t.Fatalf("再序列化 %s 失败: %v", c.Input, err)
		}
		if string(got) != c.JSON {
			t.Fatalf("%s 解回来再序列化与 serde 不一致:\n实得 %s\n期望 %s", c.Input, got, c.JSON)
		}
	}
}

// ── 逐条平移 Rust 侧 traits.rs 的测试 ────────────────────────────────────────

func TestReadRequestsAreRetryableWithoutKey(t *testing.T) {
	if !NewRequestGet("https://example.com/a").IsRetryable() {
		t.Fatal("读请求在不带幂等键时就该可重试")
	}
}

func TestWriteRequestsNeedIdempotencyKeyToRetry(t *testing.T) {
	if NewRequestPost("https://example.com/a").IsRetryable() {
		t.Fatal("无幂等键的写请求不得重试")
	}
	if !NewRequestPost("https://example.com/a").WithIdempotencyKey("run-1").IsRetryable() {
		t.Fatal("带幂等键的写请求应当可重试")
	}
}

// TestJSONBodySetsContentTypeOnce 与 Rust 侧那条同形：先 json_body，
// 再**显式**追加一个同名头，结果是 **2 个**。
//
// ⚠ 数字 2 不是笔误：自动补的那个与调用方追加的那个都在。
// 这条测的是"自动补**只在没有同名头时**发生" —— 若自动补无条件执行，
// 这里会是 3；若它把调用方的覆盖了，这里会是 1。
func TestJSONBodySetsContentTypeOnce(t *testing.T) {
	req, err := NewRequestPost("https://example.com/a").
		WithJSONBody(map[string]any{"a": 1})
	if err != nil {
		t.Fatalf("构造请求体失败: %v", err)
	}
	req = req.WithHeader("content-type", "application/json")

	count := 0
	for _, h := range req.Headers {
		if strings.EqualFold(h[0], "content-type") {
			count++
		}
	}
	if count != 2 {
		t.Fatalf("content-type 应有 2 个（自动补的 1 个 + 显式追加的 1 个），实得 %d", count)
	}
}

func TestFixedClockIsDeterministic(t *testing.T) {
	clock := ParseFixedClock("2026-09-16T12:00:00Z")
	// ⚠ 与 Rust 那条不同形：chrono 的 `to_rfc3339()` 给 `+00:00`，
	// 而 Go 的 `time.RFC3339` 给 `Z`。两者是同一时刻，只是渲染不同 ——
	// 所以这里比时刻、不比字面量（比字面量会把"渲染差异"错记成"语义差异"）。
	if got := clock.Now().UTC().Format(time.RFC3339); got != "2026-09-16T12:00:00Z" {
		t.Fatalf("固定时钟的时刻不对: %s", got)
	}
	if clock.Now() != clock.Now() {
		t.Fatal("固定时钟必须每次给出同一个值")
	}
}

func TestHTTPMethodSerializesUppercase(t *testing.T) {
	raw, err := marshalJSON(MethodPost)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if string(raw) != `"POST"` {
		t.Fatalf("HttpMethod 应序列化成大写文本，实得 %s", raw)
	}
}

// ── Go 侧新长出来的守卫 ──────────────────────────────────────────────────────

// TestBodyRejectsBase64String 是 [Body] 那条注释的断言。
//
// ⚠ 这条在 Rust 侧**没有对应测试**（serde 天然不给 `Vec<u8>` 走 base64），
// 它是 Go 侧才存在的陷阱：`encoding/json` 对 `[]byte` 有专门的 base64 分支。
// 不写这条的话，"用 json.Unmarshal 直接解"这种写法会静静通过所有对拍。
func TestBodyRejectsBase64String(t *testing.T) {
	var b Body
	if err := json.Unmarshal([]byte(`"eyJhIjox"`), &b); err == nil {
		t.Fatalf("base64 串不该被当成 body 收下（那会与 serde 的 Vec<u8> 分叉），实得 %v", []byte(b))
	}
	if err := json.Unmarshal([]byte(`[256]`), &b); err == nil {
		t.Fatal("超出 0..255 的字节值不该被收下")
	}
	if err := json.Unmarshal([]byte(`null`), &b); err != nil || b != nil {
		t.Fatalf("null 应当解成 nil: err=%v b=%v", err, b)
	}
	var ok Body
	if err := json.Unmarshal([]byte(`[1,2,3]`), &ok); err != nil || len(ok) != 3 || ok[2] != 3 {
		t.Fatalf("数字数组应当被收下: err=%v b=%v", err, ok)
	}
}

// TestTextRejectsInvalidUTF8 对齐 Rust 的 `String::from_utf8`（失败而非替换）。
func TestTextRejectsInvalidUTF8(t *testing.T) {
	bad := HTTPResponse{Body: Body{0xff, 0xfe}}
	if _, err := bad.Text(); err == nil {
		t.Fatal("非法 UTF-8 的响应体应当报错，而不是替换成 U+FFFD 后返回")
	}
	good := HTTPResponse{Body: Body("你好")}
	if s, err := good.Text(); err != nil || s != "你好" {
		t.Fatalf("合法 UTF-8 应当原样返回: %q err=%v", s, err)
	}
}

// TestScopeUserNilPanics 守住"nil 会静默退化成不过滤"这一支。
//
// ⚠ 这条是 Go 侧独有的：Rust 的 `&UserId` 非空是**类型保证**，
// 编译器不给这个机会。Go 的指针可空，所以只能靠运行时 panic 把它挡住。
func TestScopeUserNilPanics(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("ScopeUser(nil) 应当 panic —— 不 panic 就等于静默退化成 ScopeLocal（不过滤）")
		}
	}()
	_ = ScopeUser(nil)
}

func TestScopeOwnerSemantics(t *testing.T) {
	id := UserID("u-1")
	if got := ScopeUser(&id).Owner(); got == nil || *got != id {
		t.Fatalf("ScopeUser 的 Owner 应是那个用户，实得 %v", got)
	}
	if ScopeUser(&id).IsLocal() {
		t.Fatal("ScopeUser 不该是 Local")
	}
	if got := ScopeLocal().Owner(); got != nil {
		t.Fatalf("ScopeLocal 的 Owner 应是 nil（传给 `?n IS NULL OR owner = ?n` 让它恒真），实得 %v", got)
	}
	if !ScopeLocal().IsLocal() {
		t.Fatal("ScopeLocal 应是 Local")
	}
}

func TestClaimedRowsTotal(t *testing.T) {
	rows := ClaimedRows{Accounts: 3, Schedules: 2, Pipelines: 0}
	if rows.Total() != 5 {
		t.Fatalf("Total 应为 5，实得 %d", rows.Total())
	}
}

func TestParseFixedClockPanicsOnBadLiteral(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("非法时间字面量应当 panic —— 静默用零值会让'是否已过期'的判断得到荒唐答案")
		}
	}()
	_ = ParseFixedClock("not-a-time")
}

// ── 小工具 ───────────────────────────────────────────────────────────────────

// HTTPish 让"请求"与"响应"能放进同一个 map（两者都只用来序列化）。
type HTTPish struct{ Value any }

func requestish(r HTTPRequest) HTTPish   { return HTTPish{Value: r} }
func responseish(r HTTPResponse) HTTPish { return HTTPish{Value: r} }
