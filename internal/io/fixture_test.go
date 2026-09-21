package mcloudio

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

func clientWith(t *testing.T, entries ...FixtureEntry) *FixtureClient {
	t.Helper()
	client := NewFixtureClient()
	client.Insert(entries)
	return client
}

// ─────────────────────────────────────────────────────────────────────────────
// 回放
// ─────────────────────────────────────────────────────────────────────────────

// TestReplayIgnoresTheQueryWhenMatching：
// 匹配键丢弃整个 query —— 否则每次抓包都生成一批命不中的 fixture。
func TestReplayIgnoresTheQueryWhenMatching(t *testing.T) {
	client := clientWith(t, EntryFromParts(core.MethodPost,
		"https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0",
		200,
		map[string]any{"code": 0, "data": map[string]any{"count": 54}}))

	// 多带一个 `_t=1`（真实链路每次都不同），仍应命中
	response, err := client.Send(context.Background(), core.NewRequestPost(
		"https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0&_t=1"))
	if err != nil {
		t.Fatalf("应命中 fixture，实得 %v", err)
	}
	if response.Status != 200 {
		t.Errorf("状态码应回放 200，实得 %d", response.Status)
	}
	value, ok := core.ParseJSON(string(response.Body))
	if !ok {
		t.Fatalf("回放的响应体不是合法 JSON: %s", response.Body)
	}
	root, _ := value.(map[string]any)
	data, _ := root["data"].(map[string]any)
	// ⚠ 数字是 json.Number（§6.1 #34）：比文本，别按 float64 比
	if got := data["count"]; got != nil && got.(interface{ String() string }).String() != "54" {
		t.Errorf("data.count 应回放 54，实得 %v", got)
	}
}

// TestMissIsAnErrorNeverARealRequest：
// 未命中必须报错 —— "零联网"不能只是口头约定。
func TestMissIsAnErrorNeverARealRequest(t *testing.T) {
	client := NewFixtureClient()
	_, err := client.Send(context.Background(), core.NewRequestGet("https://example.com/nothing"))
	if err == nil {
		t.Fatal("未命中必须报错")
	}
	perr, ok := err.(*core.Error)
	if !ok {
		t.Fatalf("应返回 *core.Error，实得 %T", err)
	}
	// ⚠ 它在 Kind() 上看不出来（与 Internal 同类），只能用 IsFixtureMiss 判别
	if !perr.IsFixtureMiss() {
		t.Errorf("应判为 fixture 未命中，实得 kind=%v msg=%q", perr.Kind(), perr.Message())
	}
	if perr.Kind().IsRetryable() {
		t.Error("未命中不是可重试错误 —— 重试只是再 miss 一次")
	}
	if strings.Contains(perr.Message(), "http://") || strings.Contains(perr.Message(), "https://") {
		t.Errorf("未命中的文案不该回吐整条 URL，实得 %q", perr.Message())
	}
}

// TestRecordedRequestsSnapshotIncludesTheMiss：
// 未命中的那条也要进快照 —— 排查缺口时最有用的正是它。
func TestRecordedRequestsSnapshotIncludesTheMiss(t *testing.T) {
	client := NewFixtureClient()
	// ⚠ 不能写 `_ = client.Send(…)`：Go 里 `_ =` 只接得住**单个**返回值，
	// 多返回值函数是**编译错误**（Rust 的 `let _ = f()` 能吞掉任意个）。
	_, _ = client.Send(context.Background(), core.NewRequestGet("https://a.example.com/missing").WithHeader("x", "1"))

	recorded := client.RecordedRequests()
	if len(recorded) != 1 {
		t.Fatalf("未命中的请求也要记录，实得 %d 条", len(recorded))
	}
	if recorded[0].Method != core.MethodGet {
		t.Errorf("方法应记 GET，实得 %v", recorded[0].Method)
	}
	if len(recorded[0].Headers) != 1 || recorded[0].Headers[0][0] != "x" {
		t.Errorf("头应原样记下，实得 %v", recorded[0].Headers)
	}
}

// TestRecordedRequestsIsACopyNotAView：
// 交出去的快照必须是副本 —— Go 的切片是视图，直接给等于把内部状态也给了。
func TestRecordedRequestsIsACopyNotAView(t *testing.T) {
	client := clientWith(t, EntryFromParts(core.MethodGet, "https://a.example.com/x", 200, map[string]any{}))
	_, err := client.Send(context.Background(), core.NewRequestGet("https://a.example.com/x"))
	if err != nil {
		t.Fatal(err)
	}

	snapshot := client.RecordedRequests()
	// ⚠ 改**已有元素**而不是 append：append 在 len==cap 时会分配新数组，
	// 那样即便交出去的是本体也看不出问题。原地写才能真正探到底层数组是不是共享的。
	snapshot[0].URL = "https://evil.example.com/"
	snapshot[0].Headers = nil

	again := client.RecordedRequests()
	if again[0].URL == "https://evil.example.com/" {
		t.Error("外部对快照元素的写入不该影响客户端内部状态 —— 返回的是视图不是副本")
	}
	if len(again[0].Headers) != 0 {
		t.Errorf("外部对快照元素的写入不该影响客户端内部状态，实得 %v", again[0].Headers)
	}
}

// TestDuplicateEntriesAreConsumedInOrder：
// 同一接口多条记录按顺序消费，用尽后保留最后一条（幂等重放）。
func TestDuplicateEntriesAreConsumedInOrder(t *testing.T) {
	client := clientWith(t,
		EntryFromParts(core.MethodGet, "https://a.example.com/x", 200, map[string]any{"n": 1}),
		EntryFromParts(core.MethodGet, "https://a.example.com/x", 200, map[string]any{"n": 2}),
	)

	count := func() string {
		response, err := client.Send(context.Background(), core.NewRequestGet("https://a.example.com/x"))
		if err != nil {
			t.Fatalf("回放失败: %v", err)
		}
		value, _ := core.ParseJSON(string(response.Body))
		root, _ := value.(map[string]any)
		n, _ := root["n"]
		return n.(interface{ String() string }).String()
	}

	if got := count(); got != "1" {
		t.Errorf("第一次应回放 1，实得 %s", got)
	}
	if got := count(); got != "2" {
		t.Errorf("第二次应回放 2，实得 %s", got)
	}
	if got := count(); got != "2" {
		t.Errorf("用尽后应保留最后一条（保证幂等重放），实得 %s", got)
	}
}

// TestReplayKeepsRecordedResponseHeaders：回放要连头一起给。
func TestReplayKeepsRecordedResponseHeaders(t *testing.T) {
	entry := EntryFromParts(core.MethodGet, "https://a.example.com/x", 200, map[string]any{})
	entry.Response.Headers = [][2]string{{"content-type", "application/json"}, {"x-trace", "abc"}}
	client := clientWith(t, entry)

	response, err := client.Send(context.Background(), core.NewRequestGet("https://a.example.com/x"))
	if err != nil {
		t.Fatal(err)
	}
	if len(response.Headers) != 2 {
		t.Fatalf("应回放两个头，实得 %v", response.Headers)
	}
	if response.Headers[1][0] != "x-trace" || response.Headers[1][1] != "abc" {
		t.Errorf("头应原样回放，实得 %v", response.Headers[1])
	}
}

// TestANonJSONResponseFallsBackToTheRawBody：
// `body_json: null` 是**判据**不是缺省 —— 非 JSON 响应必须走 body。
func TestANonJSONResponseFallsBackToTheRawBody(t *testing.T) {
	entry := EntryFromParts(core.MethodGet, "https://a.example.com/page", 200, nil)
	if entry.Response.BodyJSON != nil {
		t.Fatal("测试自身有误：bodyJSON 为 nil 时不应填 body_json")
	}
	entry.Response.Body = "<html>活动页</html>"

	client := clientWith(t, entry)
	response, err := client.Send(context.Background(), core.NewRequestGet("https://a.example.com/page"))
	if err != nil {
		t.Fatal(err)
	}
	if got := string(response.Body); got != "<html>活动页</html>" {
		t.Errorf("非 JSON 响应应原样回放 body，实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 加载
// ─────────────────────────────────────────────────────────────────────────────

// TestLoadDirIsRecursiveAndSortsFiles：
// 递归加载 `*.json`，忽略其它后缀。
func TestLoadDirIsRecursiveAndSortsFiles(t *testing.T) {
	dir := t.TempDir()
	nested := filepath.Join(dir, "m.mcloud.139.com")
	if err := os.MkdirAll(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	entry := EntryFromParts(core.MethodGet, "https://m.mcloud.139.com/a", 200, map[string]any{"ok": true})
	raw, err := core.MarshalJSON(entry)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(nested, "exchangeList.json"), raw, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(nested, "readme.md"), []byte("not a fixture"), 0o644); err != nil {
		t.Fatal(err)
	}

	client := NewFixtureClient()
	loaded, err := client.LoadDir(dir)
	if err != nil {
		t.Fatalf("加载失败: %v", err)
	}
	if loaded != 1 {
		t.Errorf("应加载 1 条（.md 要被忽略），实得 %d", loaded)
	}
	if client.KeyCount() != 1 {
		t.Errorf("应有 1 个接口，实得 %d", client.KeyCount())
	}
}

// TestLoadDirOnAMissingDirectoryIsNotAnError：
// 目录不存在不是错误 —— 否则每个调用点都得先 stat 一遍。
func TestLoadDirOnAMissingDirectoryIsNotAnError(t *testing.T) {
	client := NewFixtureClient()
	loaded, err := client.LoadDir(filepath.Join(t.TempDir(), "does-not-exist"))
	if err != nil {
		t.Fatalf("目录不存在不该报错，实得 %v", err)
	}
	if loaded != 0 {
		t.Errorf("应加载 0 条，实得 %d", loaded)
	}
}

// TestLoadFileAcceptsBothASingleEntryAndAnArray：
// 单条与数组两种文件形态都要收。
func TestLoadFileAcceptsBothASingleEntryAndAnArray(t *testing.T) {
	one := EntryFromParts(core.MethodGet, "https://a.example.com/one", 200, map[string]any{"n": 1})
	two := EntryFromParts(core.MethodGet, "https://a.example.com/two", 200, map[string]any{"n": 2})

	single, err := core.MarshalJSON(one)
	if err != nil {
		t.Fatal(err)
	}
	array, err := core.MarshalJSON([]FixtureEntry{one, two})
	if err != nil {
		t.Fatal(err)
	}

	dir := t.TempDir()
	pathSingle := filepath.Join(dir, "single.json")
	pathArray := filepath.Join(dir, "array.json")
	if err := os.WriteFile(pathSingle, single, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathArray, array, 0o644); err != nil {
		t.Fatal(err)
	}

	client := NewFixtureClient()
	if n, err := client.LoadFile(pathSingle); err != nil || n != 1 {
		t.Errorf("单条文件应加载 1 条，实得 %d, err=%v", n, err)
	}
	if n, err := client.LoadFile(pathArray); err != nil || n != 2 {
		t.Errorf("数组文件应加载 2 条，实得 %d, err=%v", n, err)
	}
	if client.KeyCount() != 2 {
		t.Errorf("两个不同的接口，实得 %d", client.KeyCount())
	}
}

// TestLoadFileOnBrokenJSONReportsWhichFile：报错要点名是哪个文件。
func TestLoadFileOnBrokenJSONReportsWhichFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "broken.json")
	if err := os.WriteFile(path, []byte("{not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	client := NewFixtureClient()
	_, err := client.LoadFile(path)
	if err == nil {
		t.Fatal("坏 JSON 必须报错")
	}
	if !strings.Contains(err.Error(), "broken.json") {
		t.Errorf("报错应点名是哪个文件，实得 %q", err.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 自检
// ─────────────────────────────────────────────────────────────────────────────

// TestAuditFindsALeakyFixture：自检必须能发现未脱敏的 fixture。
func TestAuditFindsALeakyFixture(t *testing.T) {
	// ⚠ 哨兵值要**短且不像真凭据**：scripts/check-identifiers.cjs 会扫测试里的
	// 疑似凭据，写长了会让门禁变红（那是它该做的）。脱敏按**键名**判断，
	// 所以短值一样能被它挡住 —— 断言仍然是承重的。
	entry := EntryFromParts(core.MethodPost, "https://a.example.com/login", 200,
		map[string]any{"auth": "sEntrySecret"})
	entry.Response.Body = `{"auth":"sEntrySecret"}`
	entry.Response.BodyJSON = nil // 逼它走文本那一支

	client := clientWith(t, entry)
	leaks := client.AuditLeaks()
	if len(leaks) == 0 {
		t.Fatal("自检必须能发现未脱敏的 fixture")
	}
	// 报告里不该出现值本身 —— 否则自检自己成了新的泄漏通道
	for _, leak := range leaks {
		if strings.Contains(leak, "sEntrySecret") {
			t.Errorf("自检报告不该回吐值本身，实得 %q", leak)
		}
	}
}

// TestAuditAcceptsACleanFixture：脱敏过的 fixture 不该报。
func TestAuditAcceptsACleanFixture(t *testing.T) {
	entry := EntryFromParts(core.MethodPost, "https://a.example.com/login", 200,
		map[string]any{"auth": core.Placeholder, "nickname": "账号2"})
	client := clientWith(t, entry)
	if leaks := client.AuditLeaks(); len(leaks) != 0 {
		t.Errorf("干净的 fixture 不该报泄漏，实得 %v", leaks)
	}
}

// TestAuditCoversTheRequestSideToo：
// 请求段同样要扫 —— 请求体里带着凭据是很常见的形态。
func TestAuditCoversTheRequestSideToo(t *testing.T) {
	entry := EntryFromParts(core.MethodPost, "https://a.example.com/login", 200, map[string]any{})
	entry.Request.Body = `{"password":"sReqSecret"}`
	client := clientWith(t, entry)
	if leaks := client.AuditLeaks(); len(leaks) == 0 {
		t.Error("请求段也要过自检 —— 只扫响应等于漏掉一半")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 录制侧
// ─────────────────────────────────────────────────────────────────────────────

// TestBuildEntryRoundTripsThroughTheFixtureClient：
// 录制出来的记录必须能被回放客户端直接消费 —— 这两侧一旦分叉，
// 加载会静默得到一堆零值（Go 对**未知字段**是忽略的，见 #52）。
func TestBuildEntryRoundTripsThroughTheFixtureClient(t *testing.T) {
	entry := BuildEntry("POST",
		"https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=12.6.0",
		[]core.HeaderPair{{"content-type", "application/json"}},
		[]byte(`{"page":1}`),
		200,
		[]core.HeaderPair{{"content-type", "application/json"}},
		[]byte(`{"code":0,"data":{"count":54}}`),
	)
	if entry.Match.Key != "POST m.mcloud.139.com/ycloud/signin/page/exchangeList" {
		t.Errorf("匹配键应丢掉 query，实得 %q", entry.Match.Key)
	}
	if entry.Response.Status != 200 {
		t.Errorf("状态码应记 200，实得 %d", entry.Response.Status)
	}
	if entry.Response.BodyJSON == nil {
		t.Fatal("JSON 响应必须有结构化的那一份")
	}

	client := clientWith(t, entry)
	response, err := client.Send(context.Background(), core.NewRequestPost(
		"https://m.mcloud.139.com/ycloud/signin/page/exchangeList?clientVersion=99&_t=1"))
	if err != nil {
		t.Fatalf("录制出来的记录应能直接回放，实得 %v", err)
	}
	if !strings.Contains(string(response.Body), `"count"`) {
		t.Errorf("回放内容不对: %s", response.Body)
	}
	if leaks := client.AuditLeaks(); len(leaks) != 0 {
		t.Errorf("录制侧脱敏后不该有泄漏，实得 %v", leaks)
	}
}

// TestBuildEntryRedactsSecretsBeforeArchiving：
// 录制时必须先脱敏 —— 这是"夹具里不能有真凭据"的唯一保障。
func TestBuildEntryRedactsSecretsBeforeArchiving(t *testing.T) {
	entry := BuildEntry("POST", "https://a.example.com/login?token=sUrlSecret",
		nil,
		[]byte(`{"password":"sReqSecret"}`),
		200, nil,
		[]byte(`{"auth":"sRespSecret"}`),
	)
	raw, err := core.MarshalJSON(entry)
	if err != nil {
		t.Fatal(err)
	}
	text := string(raw)
	for _, secret := range []string{"sUrlSecret", "sReqSecret", "sRespSecret"} {
		if strings.Contains(text, secret) {
			t.Errorf("存档里不该出现 %q", secret)
		}
	}
}

// TestEveryArchivedFixtureIsLeakFree：
// 对仓库里**全部**抓包夹具跑一遍自检 —— "脱敏是否真的生效"的最后一道闸。
//
// 没有 CI，所以这条测试就是那道闸：新增夹具、改动脱敏规则，都由它兜住。
func TestEveryArchivedFixtureIsLeakFree(t *testing.T) {
	dir := filepath.Join("..", "..", "test", "fixtures", "http")
	if _, err := os.Stat(dir); err != nil {
		// ⚠ 只在**目录不存在**时跳过。若目录在却一条都没加载出来，那是
		// "路径写错 / 格式变了"的静默失败 —— 必须响，不能 skip 掉。
		t.Skipf("没有抓包夹具目录 %s，跳过自检", dir)
	}
	client := NewFixtureClient()
	loaded, err := client.LoadDir(dir)
	if err != nil {
		t.Fatalf("加载 %s 失败: %v", dir, err)
	}
	if loaded == 0 {
		t.Fatalf("%s 里一条夹具都没加载出来 —— 路径或格式变了，自检不能空跑", dir)
	}
	// 覆盖率断言：光"加载成功"不够，得真的认出了若干接口。
	// 否则"整份通过"可能只是没扫到任何东西。
	if keys := client.KeyCount(); keys == 0 {
		t.Fatal("一条接口都没认出来")
	}

	leaks := client.AuditLeaks()
	if len(leaks) != 0 {
		for i, leak := range leaks {
			if i >= 20 {
				t.Errorf("…… 另有 %d 条", len(leaks)-20)
				break
			}
			t.Errorf("泄漏: %s", leak)
		}
	}
}
