package mcloudio

import (
	"context"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// HTTP fixture 回放
// ─────────────────────────────────────────────────────────────────────────────

// 编译期断言：回放客户端必须真的满足 [core.HTTPClient]。
//
// 它不是"真实"的客户端，但它是 L2 测试里**唯一**被允许的那个 ——
// 断言写在定义处，测试里就不用再证明一遍。
var _ core.HTTPClient = (*FixtureClient)(nil)

// FixtureEntry 是一条 fixture 记录。
//
// 字段与 [core.BuildFixtureEntry] 的输出一致 —— 它不是"又一个模型"，
// 而是那个函数的**解码侧**。两边形状一旦分叉，加载就会静默得到一堆零值
// （Go 的结构体解码对**未知字段**是忽略的，见 #52）。
type FixtureEntry struct {
	Match    MatchSpec        `json:"match"`
	Request  RecordedRequest  `json:"request"`
	Response RecordedResponse `json:"response"`
}

// MatchSpec 是匹配段。
type MatchSpec struct {
	// Key 是 `METHOD host/path`，由 [core.MatchKey] 生成。
	Key string `json:"key"`
	// Method 是大写方法名。
	//
	// ⚠ 它**不参与匹配**（匹配只看 Key），存档它是为了让人能读懂夹具。
	// 别把它改成 [core.HTTPMethod]：Rust 侧这里是 `String`，
	// 而 core.HTTPMethod 的解码是**大小写敏感**的等值匹配 ——
	// 一份手写的 `"get"` 会让整条记录加载失败。
	Method string `json:"method"`
}

// RecordedRequest 是已存档的请求（供 golden 对拍用，不参与匹配）。
type RecordedRequest struct {
	URL     string      `json:"url"`
	Headers [][2]string `json:"headers"`
	Body    string      `json:"body"`
}

// RecordedResponse 是已存档的响应。
type RecordedResponse struct {
	Status   uint16      `json:"status"`
	Headers  [][2]string `json:"headers"`
	Body     string      `json:"body"`
	BodyJSON any         `json:"body_json"`
}

// FixtureClient 回放客户端。
//
// 用途有两个：
//
//  1. **L2 测试**：用真实抓包转出的脱敏 fixture 验证上游协议层的请求构造
//     与响应解析，全程零联网。
//  2. **调试台离线模式**：对任意接口做参数化执行而不真的打上游。
//
// # 设计取舍：未命中必须报错
//
// 未命中时返回 [core.NewFixtureMiss]（判别用 [core.Error.IsFixtureMiss]，
// 它在 Kind 上看不出来 —— 与 Internal 同类），**不静默回退到真实网络**。
// 否则"零联网"就只是口头约定 —— 测试会因为悄悄打了真网而变绿，
// 掩盖 fixture 缺口。
type FixtureClient struct {
	mu       sync.Mutex
	queue    map[string][]FixtureEntry
	recorded []core.HTTPRequest
}

// NewFixtureClient 构造一个空的回放客户端。
//
// ⚠ 必须走构造函数：Go 的零值是 nil map，**读**没问题但**写**会 panic。
// Rust 的 `impl Default` 没有这个陷阱 —— 见 §6.1 #23。
func NewFixtureClient() *FixtureClient {
	return &FixtureClient{queue: map[string][]FixtureEntry{}}
}

// LoadFile 从单个 JSON 文件加载（文件内容可以是单条，也可以是数组）。
func (c *FixtureClient) LoadFile(path string) (int, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return 0, core.NewInternal("读取 fixture 失败 " + path + ": " + err.Error())
	}
	entries, perr := parseFixtureEntries(string(raw))
	if perr != nil {
		return 0, core.NewInternal("解析 fixture 失败 " + path + ": " + perr.Message())
	}
	return c.insert(entries), nil
}

// LoadDir 递归加载目录下全部 `*.json`。
//
// 目录不存在时返回 0 而不是报错：调用方常常传一个"有就有、没有就没有"的
// 可选目录（CI 里没拉子模块、本地没跑过录制），把"没有"当成错误会让
// 每个调用点都得先 `stat` 一遍。
func (c *FixtureClient) LoadDir(dir string) (int, error) {
	files, err := collectJSONFiles(dir)
	if err != nil {
		return 0, core.NewInternal("遍历 fixture 目录失败 " + dir + ": " + err.Error())
	}
	// 排序不是装饰：加载顺序决定了同一接口多条记录的**消费顺序**，
	// 不排序的话结果依赖文件系统遍历顺序（macOS 与 Linux 不同）。
	sort.Strings(files)

	total := 0
	for _, file := range files {
		n, ferr := c.LoadFile(file)
		if ferr != nil {
			return total, ferr
		}
		total += n
	}
	return total, nil
}

// Insert 直接喂内存里的记录（测试用）。
func (c *FixtureClient) Insert(entries []FixtureEntry) int { return c.insert(entries) }

func (c *FixtureClient) insert(entries []FixtureEntry) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, entry := range entries {
		key := entry.Match.Key
		c.queue[key] = append(c.queue[key], entry)
	}
	return len(entries)
}

// KeyCount 返回已加载的接口数量（去重后）。
func (c *FixtureClient) KeyCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.queue)
}

// RecordedRequests 返回实际收到的请求快照（golden 对拍用）。
//
// ⚠ 返回**副本**，不是内部切片。Go 的切片是**视图**：直接交出去等于把
// 内部状态的所有权也交了，调用方一次 `append` 就可能踩到另一处正在读的
// 数据。Rust 侧这里返回 `Vec` 的克隆是**语言强制**的（借用检查），
// Go 没有任何东西逼你这么做 —— 见 §6.1 #51。
func (c *FixtureClient) RecordedRequests() []core.HTTPRequest {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]core.HTTPRequest, len(c.recorded))
	copy(out, c.recorded)
	return out
}

// AuditLeaks 自检：把所有已加载 fixture 重新序列化后扫一遍泄漏。
//
// 这是"脱敏是否真的生效"的最后一道闸。它由测试对所有 fixture 跑一次 ——
// 没有 CI，**测试就是那道闸**。
func (c *FixtureClient) AuditLeaks() []string {
	c.mu.Lock()
	snapshot := make(map[string][]FixtureEntry, len(c.queue))
	for key, entries := range c.queue {
		cloned := make([]FixtureEntry, len(entries))
		copy(cloned, entries)
		snapshot[key] = cloned
	}
	c.mu.Unlock()

	var leaks []string
	// map 遍历顺序是**故意随机化**的（§6.1 #21），排序只为让报告稳定可读。
	keys := make([]string, 0, len(snapshot))
	for key := range snapshot {
		keys = append(keys, key)
	}
	sort.Strings(keys)

	for _, key := range keys {
		for _, entry := range snapshot[key] {
			raw, err := core.MarshalJSON(entry)
			if err != nil {
				leaks = append(leaks, "["+key+"] 序列化失败: "+err.Error())
				continue
			}
			for _, leak := range core.ScanForLeaks(string(raw)) {
				leaks = append(leaks, "["+key+"] "+leak)
			}
		}
	}
	return leaks
}

// Send 回放一条记录。
func (c *FixtureClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	// ⚠ 先记录**再**匹配：未命中的请求同样要出现在快照里。
	// 排查 fixture 缺口时最有用的正是那条没命中的请求 ——
	// 只看"成功回放了什么"永远找不出缺哪一条。
	c.recorded = append(c.recorded, request)

	key := core.MatchKey(request.Method.String(), request.URL)
	list, ok := c.queue[key]
	if !ok || len(list) == 0 {
		return core.HTTPResponse{}, core.NewFixtureMiss(
			key + "（未命中即报错，绝不回退真实网络）")
	}
	// 同一接口多条记录时按顺序消费，用完保留最后一条（幂等重放）。
	entry := list[0]
	if len(list) > 1 {
		list[0] = FixtureEntry{} // 让出底层数组的引用，避免已消费的记录被一直持有
		c.queue[key] = list[1:]
	}

	body := responseBodyBytes(entry.Response)
	response := core.NewResponse(entry.Response.Status, body)
	for _, h := range entry.Response.Headers {
		response.Headers = append(response.Headers, core.HeaderPair(h))
	}
	return response, nil
}

// responseBodyBytes 回放响应体：优先用结构化的 `body_json`，否则退回文本。
//
// # 为什么优先 body_json
//
// 脱敏后的结构化值**保留类型信息**（数字仍是数字，不是 `"54"`）。
// 用文本回放时所有数字都变成字符串，解析层就得顺手做一次转换 ——
// 那等于把"上游到底回了什么形状"这件事实挪进了解析层的容错里。
//
// ⚠ `body_json: null` 是**判据**不是缺省：非 JSON 响应（HTML 页面、
// 加密串）本来就没有结构化写法，此时必须走 `body`。
func responseBodyBytes(recorded RecordedResponse) []byte {
	if recorded.BodyJSON != nil {
		raw, err := core.MarshalJSON(recorded.BodyJSON)
		if err == nil {
			return raw
		}
		// 序列化失败就退回文本：宁可回放一份形状差一点的响应，
		// 也不要在这里抛一个与被测逻辑无关的错。
	}
	return []byte(recorded.Body)
}

// parseFixtureEntries 解析文件内容（单条或数组）。
func parseFixtureEntries(text string) ([]FixtureEntry, *core.Error) {
	value, ok := core.ParseJSON(text)
	if !ok {
		return nil, core.NewParse("fixture 不是合法 JSON")
	}
	if _, isArray := value.([]any); isArray {
		parsed, ok := core.ParseJSONTo[[]FixtureEntry](text)
		if !ok {
			return nil, core.NewParse("fixture 数组里有不合形状的记录")
		}
		return parsed, nil
	}
	parsed, ok := core.ParseJSONTo[FixtureEntry](text)
	if !ok {
		return nil, core.NewParse("fixture 记录不合形状")
	}
	return []FixtureEntry{parsed}, nil
}

// collectJSONFiles 递归收集 `*.json`。
func collectJSONFiles(dir string) ([]string, error) {
	if _, err := os.Stat(dir); err != nil {
		// 目录不存在不是错误 —— 见 [FixtureClient.LoadDir]。
		return nil, nil
	}
	var out []string
	walkErr := filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		if strings.EqualFold(filepath.Ext(path), ".json") {
			out = append(out, path)
		}
		return nil
	})
	if walkErr != nil {
		return nil, walkErr
	}
	return out, nil
}

// EntryFromParts 便捷构造：直接喂内存里的记录，便于单测。
//
// bodyJSON 为 nil 时只填 `body`（非 JSON 响应的形状）。
func EntryFromParts(method core.HTTPMethod, url string, status uint16, bodyJSON any) FixtureEntry {
	var body string
	if bodyJSON != nil {
		if raw, err := core.MarshalJSON(bodyJSON); err == nil {
			body = string(raw)
		}
	}
	return FixtureEntry{
		Match: MatchSpec{
			Key:    core.MatchKey(method.String(), url),
			Method: method.String(),
		},
		Request: RecordedRequest{URL: core.RedactURL(url)},
		Response: RecordedResponse{
			Status:   status,
			Body:     body,
			BodyJSON: bodyJSON,
		},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 从抓包生成 fixture（录制侧）
// ─────────────────────────────────────────────────────────────────────────────

// BuildEntry 把一次真实交互转成一条可存档的记录（已脱敏）。
//
// 它是 [core.BuildFixtureEntry] 的**收口**：那个函数返回 `map[string]any`，
// 而存档与自检要的是结构体 —— 两边都从同一个函数出，形状就不会分叉。
//
// ⚠ 直接传 `[]core.HeaderPair` 给需要 `[][2]string` 的参数能编译，
// 因为 [core.HeaderPair] 是 `[2]string` 的**别名**而非定义 —— 两者是
// 同一个类型。这一点反过来也成立：给 HeaderPair 加方法会**连
// [2]string 一起改**，别顺手加。
func BuildEntry(
	method string,
	url string,
	requestHeaders []core.HeaderPair,
	requestBody []byte,
	status uint16,
	responseHeaders []core.HeaderPair,
	responseBody []byte,
) FixtureEntry {
	raw := core.BuildFixtureEntry(method, url, requestHeaders, requestBody,
		status, responseHeaders, responseBody)

	text, err := core.MarshalJSON(raw)
	if err != nil {
		return FixtureEntry{}
	}
	entry, ok := core.ParseJSONTo[FixtureEntry](string(text))
	if !ok {
		return FixtureEntry{}
	}
	return entry
}
