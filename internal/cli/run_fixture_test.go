package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// `fixture extract` 的测试。
//
// # 这层要钉住的是**边界**
//
// 抽取器的产出要进版本库，而 HAR 里含可直接复用的登录凭据（P0）。所以判据
// 不是"文件写出来了"，而是：
//
//   - 写出来的东西**回放侧能读回去**（否则这份夹具等于没生成）
//   - 凭据**没有**落在盘上
//   - 自检说有残留时**一个字节都不写**（fail closed）
//
// ⚠ 每条用例都**显式**传 `--har` / `--out`：这两个 flag 的默认值是相对路径
// （`har` 与 `test/fixtures/http`），不传就会在包目录下读写真实仓库内容。

// ─────────────────────────────────────────────────────────────────────────────
// HAR 夹具
// ─────────────────────────────────────────────────────────────────────────────

// harSpec 是一条 HAR 条目。只填要用到的字段，零值就是"没有"。
type harSpec struct {
	method  string
	url     string
	status  uint16
	reqBody string
	resBody string
	mime    string
	headers []string // 交替的 name / value
}

func (s harSpec) withDefaults() harSpec {
	if s.method == "" {
		s.method = "GET"
	}
	if s.url == "" {
		s.url = "https://mcloud.139.com/orchestration/api/v1/info"
	}
	if s.status == 0 {
		s.status = 200
	}
	if s.mime == "" {
		s.mime = "application/json"
	}
	if s.resBody == "" {
		s.resBody = `{"code":0,"msg":"success"}`
	}
	return s
}

// writeHAR 在 dir 里写一份 HAR。
func writeHAR(t *testing.T, dir, name string, specs ...harSpec) string {
	t.Helper()
	entries := make([]map[string]any, 0, len(specs))
	for _, spec := range specs {
		spec = spec.withDefaults()
		entry := map[string]any{
			"request": map[string]any{
				"method": spec.method,
				"url":    spec.url,
			},
			"response": map[string]any{
				"status": spec.status,
				"content": map[string]any{
					"text":     spec.resBody,
					"mimeType": spec.mime,
				},
			},
		}
		if spec.reqBody != "" {
			entry["request"].(map[string]any)["postData"] = map[string]any{"text": spec.reqBody}
		}
		if len(spec.headers) > 0 {
			pairs := make([]map[string]any, 0, len(spec.headers)/2)
			for i := 0; i+1 < len(spec.headers); i += 2 {
				pairs = append(pairs, map[string]any{"name": spec.headers[i], "value": spec.headers[i+1]})
			}
			entry["request"].(map[string]any)["headers"] = pairs
		}
		entries = append(entries, entry)
	}
	raw, err := json.Marshal(map[string]any{"log": map[string]any{"entries": entries}})
	if err != nil {
		t.Fatalf("构造 HAR 失败: %v", err)
	}
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, raw, 0o644); err != nil {
		t.Fatalf("写入 HAR 失败: %v", err)
	}
	return path
}

// extractInto 跑一次 `fixture extract`，把产物落到 dir 下。
func extractInto(t *testing.T, harPath, outDir string, extra ...string) (stdout, stderr string, code int) {
	t.Helper()
	args := []string{"fixture", "extract", "--har", harPath, "--out", outDir}
	return runCLI(t, "", append(args, extra...)...)
}

// fixtureFiles 列出 outDir 下写出的夹具文件（排序）。
func fixtureFiles(t *testing.T, outDir string) []string {
	t.Helper()
	paths, err := filepath.Glob(filepath.Join(outDir, "*.json"))
	if err != nil {
		t.Fatalf("列举夹具失败: %v", err)
	}
	return paths
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据
// ─────────────────────────────────────────────────────────────────────────────

// TestFixtureExtractOutputIsLoadableByTheReplaySide 钉住**产出的形状**。
//
// 抽取器唯一的消费者是回放侧的 [mcloudio.FixtureClient]：写出来的文件它读不回去，
// 这份夹具就等于没生成 —— 而且不报错，只是"回放时一条都匹配不上"。
func TestFixtureExtractOutputIsLoadableByTheReplaySide(t *testing.T) {
	dir := t.TempDir()
	har := writeHAR(t, dir, "sample.har", harSpec{})
	outDir := filepath.Join(dir, "out")

	stdout, stderr, code := extractInto(t, har, outDir)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	if len(fixtureFiles(t, outDir)) != 1 {
		t.Fatalf("应写出 1 个夹具文件，实得 %v", fixtureFiles(t, outDir))
	}

	client := mcloudio.NewFixtureClient()
	loaded, err := client.LoadDir(outDir)
	if err != nil {
		t.Fatalf("回放侧读不回去: %v", err)
	}
	if loaded != 1 {
		t.Errorf("回放侧应加载 1 条，实得 %d", loaded)
	}
	if client.KeyCount() != 1 {
		t.Errorf("应有 1 个接口键，实得 %d", client.KeyCount())
	}
}

// TestFixtureExtractRedactsCredentialsBeforeTheyTouchTheDisk 钉住 P0。
//
// HAR 里带着可直接复用的凭据。断言落在**写出来的字节**上，不落在"调用了脱敏"
// 上 —— 后者换个函数名就绿了，而漏掉的凭据一旦进版本库，删掉也不等于没被 clone 过。
func TestFixtureExtractRedactsCredentialsBeforeTheyTouchTheDisk(t *testing.T) {
	const (
		token = "abcd1234efgh5678ijkl9012mnop3456qrst7890"
		jwt   = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMzkAwMDAwMDAwMSJ9.ZmFrZXNpZ25hdHVyZQ"
	)
	dir := t.TempDir()
	har := writeHAR(t, dir, "cred.har", harSpec{
		url:     "https://mcloud.139.com/api/v1/info?accessToken=" + token,
		headers: []string{"authorization", "Bearer " + jwt},
	})
	outDir := filepath.Join(dir, "out")

	if _, _, code := extractInto(t, har, outDir); code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}

	written, err := os.ReadFile(fixtureFiles(t, outDir)[0])
	if err != nil {
		t.Fatalf("读夹具失败: %v", err)
	}
	for _, needle := range []string{token, jwt, "Bearer " + jwt} {
		if strings.Contains(string(written), needle) {
			t.Errorf("凭据落盘了：%q\n  夹具=%s", needle, written)
		}
	}
}

// TestFixtureExtractFailsClosedWhenTheSelfCheckFindsALeak 钉住"闸真的会关上"。
//
// 注入是因为**真实数据触发不了**：脱敏规则刻意比自检规则更宽（阈值 32 vs 40），
// 否则每次抽取都报警。于是"发现残留 → 不写盘"这条分支在线上永远走不到 ——
// 不注入就没有任何办法证明它不是死代码。
func TestFixtureExtractFailsClosedWhenTheSelfCheckFindsALeak(t *testing.T) {
	dir := t.TempDir()
	har := writeHAR(t, dir, "sample.har", harSpec{})
	outDir := filepath.Join(dir, "out")

	original := leakScan
	leakScan = func(string) []string { return []string{"$.response.body: 疑似 token 明文"} }
	defer func() { leakScan = original }()

	stdout, stderr, code := extractInto(t, har, outDir)
	if code != ExitRun {
		t.Errorf("自检失败应表现为运行错误（退 %d），实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "残留") {
		t.Errorf("错误信息该点名残留，实得 %q", stderr)
	}
	if files := fixtureFiles(t, outDir); len(files) != 0 {
		t.Errorf("fail closed：有残留就**一个字节都不该写**，实得 %v", files)
	}
	if !strings.Contains(stdout, "脱敏自检  : 失败") {
		t.Errorf("报告该说明是自检拦下的，stdout=%q", stdout)
	}
}

// TestFixtureExtractDryRunWritesNothing：`--dry-run` 只扫描与自检，不落盘。
func TestFixtureExtractDryRunWritesNothing(t *testing.T) {
	dir := t.TempDir()
	har := writeHAR(t, dir, "sample.har", harSpec{})
	outDir := filepath.Join(dir, "out")

	if _, _, code := extractInto(t, har, outDir, "--dry-run"); code != ExitSuccess {
		t.Fatalf("dry-run 应退 0，实得 %d", code)
	}
	// ⚠ 目录**存在**是可以的（实现会先建目录再分支），关键是里面没有产物。
	if files := fixtureFiles(t, outDir); len(files) != 0 {
		t.Errorf("dry-run 不该写文件，实得 %v", files)
	}
}

// TestFixtureExtractKeepsOnlyUpstreamJSONAndSkipsTheRest 钉住分类。
//
// 六类跳过各有一条理由，但这条用例的价值在**合并**断言：
// 少跳过一类，夹具就会从几 MB 涨到几十 MB，而"体积变大了"没有测试会报。
func TestFixtureExtractKeepsOnlyUpstreamJSONAndSkipsTheRest(t *testing.T) {
	dir := t.TempDir()
	har := writeHAR(t, dir, "mixed.har",
		harSpec{url: "https://mcloud.139.com/api/v1/info"},                                                          // 保留
		harSpec{method: "OPTIONS", url: "https://mcloud.139.com/api/v1/info"},                                       // 预检
		harSpec{url: "https://mcloud.139.com/static/app.js"},                                                        // 静态
		harSpec{url: "https://mcloud.139.com/api/v1/page", mime: "text/html", resBody: "<html>"},                    // 非 JSON
		harSpec{url: "https://collector.github.com/track", resBody: `{"ok":1}`},                                     // 非上游主机
		harSpec{url: "https://mcloud.139.com/api/v1/big", resBody: `{"d":"` + strings.Repeat("x", 200*1024) + `"}`}, // 超大
	)
	outDir := filepath.Join(dir, "out")

	stdout, stderr, code := extractInto(t, har, outDir)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "保留条目 : 1") {
		t.Errorf("只该保留 1 条，stdout=%q", stdout)
	}
	if !strings.Contains(stdout, "预检 1 / 静态 1 / 非 JSON 1 / 非上游主机 1 / 超大响应 1 / 重复采样 0") {
		t.Errorf("分类计数不对（六类应当各归各的桶）: %q", stdout)
	}
	if !strings.Contains(stdout, "collector.github.com") {
		t.Errorf("挡掉的主机该在报告里点名（默认拒绝、**可见地**放宽）: %q", stdout)
	}
}

// TestFixtureExtractCapsSamplesPerEndpointAcrossHARFiles 钉住采样上限。
//
// 上限必须**跨 HAR 累计**：它管的是夹具总量。每个文件里各自算一遍的话，
// 36 份 HAR 能把同一个接口写进 72 条记录 —— 而"夹具变大了"没有测试会报。
func TestFixtureExtractCapsSamplesPerEndpointAcrossHARFiles(t *testing.T) {
	dir := t.TempDir()
	same := harSpec{url: "https://mcloud.139.com/api/v1/info"}
	writeHAR(t, dir, "a.har", same, same, same)
	writeHAR(t, dir, "b.har", same, same)
	outDir := filepath.Join(dir, "out")

	stdout, stderr, code := extractInto(t, dir, outDir)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "HAR 文件") || !strings.Contains(stdout, "2 个") {
		t.Errorf("该说明扫了 2 个 HAR: %q", stdout)
	}
	if !strings.Contains(stdout, "保留条目 : 2") {
		t.Errorf("同一个接口最多留 2 条采样（跨文件累计），stdout=%q", stdout)
	}
	if !strings.Contains(stdout, "重复采样 3") {
		t.Errorf("该报出被上限挡掉的 3 条，stdout=%q", stdout)
	}
}

// TestFixtureExtractRemovesFixtureThatTheCurrentHARNoLongerYields 钉住陈旧清理。
//
// 某个 HAR 这一轮一条都没留下，上一轮的产物却还在 —— 那份是在**更早、更宽松的
// 脱敏规则**下生成的，恰恰是最不该继续留在版本库里的东西。
func TestFixtureExtractRemovesFixtureThatTheCurrentHARNoLongerYields(t *testing.T) {
	dir := t.TempDir()
	// 上一轮留下过产物
	writeHAR(t, dir, "empty.har", harSpec{url: "https://collector.github.com/track"})
	outDir := filepath.Join(dir, "out")
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		t.Fatal(err)
	}
	stale := filepath.Join(outDir, "empty.json")
	if err := os.WriteFile(stale, []byte(`[{"match":{"key":"GET /x"}}]`), 0o644); err != nil {
		t.Fatal(err)
	}

	stdout, stderr, code := extractInto(t, dir, outDir)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if _, err := os.Stat(stale); !os.IsNotExist(err) {
		t.Errorf("陈旧夹具该被删掉，实得 err=%v", err)
	}
	if !strings.Contains(stdout, "清理陈旧  : 1") {
		t.Errorf("报告该说明清理了 1 个，stdout=%q", stdout)
	}
}

// TestFixtureExtractIsDeterministic：同一份 HAR 必须产出同一份夹具。
//
// 采样上限是按**遍历顺序**生效的，所以目录遍历顺序一变，保留下来的条目就变了。
// 这条用例是"夹具进了版本库之后不会每次跑都产生 diff"的唯一保证。
func TestFixtureExtractIsDeterministic(t *testing.T) {
	dir := t.TempDir()
	same := harSpec{url: "https://mcloud.139.com/api/v1/info"}
	other := harSpec{url: "https://mcloud.139.com/api/v1/list"}
	writeHAR(t, dir, "z.har", same, other)
	writeHAR(t, dir, "a.har", other, same)

	first := filepath.Join(dir, "out1")
	if _, _, code := extractInto(t, dir, first); code != ExitSuccess {
		t.Fatalf("第一次应退 0，实得 %d", code)
	}
	second := filepath.Join(dir, "out2")
	if _, _, code := extractInto(t, dir, second); code != ExitSuccess {
		t.Fatalf("第二次应退 0，实得 %d", code)
	}

	for _, name := range []string{"a.json", "z.json"} {
		left, err := os.ReadFile(filepath.Join(first, name))
		if err != nil {
			t.Fatalf("读第一次产物失败: %v", err)
		}
		right, err := os.ReadFile(filepath.Join(second, name))
		if err != nil {
			t.Fatalf("读第二次产物失败: %v", err)
		}
		if string(left) != string(right) {
			t.Errorf("%s 两次产出不一致：\n  第一次=%s\n  第二次=%s", name, left, right)
		}
	}
}

// TestFixtureExtractReportsAMissingPathAsARunError：路径不存在是运行错误，不是用法错误。
//
// 退 2 会让调用方以为是自己命令写错了，而去改一个其实没写错的参数。
func TestFixtureExtractReportsAMissingPathAsARunError(t *testing.T) {
	dir := t.TempDir()
	_, stderr, code := extractInto(t, filepath.Join(dir, "nope.har"), filepath.Join(dir, "out"))
	if code != ExitRun {
		t.Errorf("路径不存在应退 %d，实得 %d（stderr=%q）", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "路径不存在") {
		t.Errorf("错误信息该点名路径不存在，实得 %q", stderr)
	}
}

// TestFixtureExtractRejectsADirectoryWithoutAnyHAR。
//
// 空目录与"路径写错"是两种不同的处境：前者说明**这个目录选错了**，
// 静默产出 0 个文件会让"抽取成功"的错觉一直持续到有人去查夹具目录。
func TestFixtureExtractRejectsADirectoryWithoutAnyHAR(t *testing.T) {
	dir := t.TempDir()
	empty := filepath.Join(dir, "empty")
	if err := os.MkdirAll(empty, 0o755); err != nil {
		t.Fatal(err)
	}
	_, stderr, code := extractInto(t, empty, filepath.Join(dir, "out"))
	if code != ExitRun {
		t.Errorf("没有 .har 应退 %d，实得 %d（stderr=%q）", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "未找到任何 .har") {
		t.Errorf("错误信息该说明没找到 HAR，实得 %q", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// slugify
// ─────────────────────────────────────────────────────────────────────────────

// TestSlugifyKeepsChineseAndKebabizesTheRest。
//
// HAR 名是抓包工具按页面标题写出来的，实测混着中文、空格、大写、下划线、点。
// 中文**保留**是刻意的：它本身就是这个名字的信息量所在，折成 `-` 只剩空壳。
func TestSlugifyKeepsChineseAndKebabizesTheRest(t *testing.T) {
	cases := []struct{ in, want string }{
		{"sign_in_3", "sign-in-3"},
		{"任务列表", "任务列表"},
		{"玩 AI 抽支付宝红包", "玩-ai-抽支付宝红包"},
		{"mobile.events.data.microsoft.com_2026_06_03", "mobile-events-data-microsoft-com-2026-06-03"},
		{"  leading and trailing  ", "leading-and-trailing"},
	}
	for _, tc := range cases {
		if got := slugify(tc.in); got != tc.want {
			t.Errorf("slugify(%q) = %q，期望 %q", tc.in, got, tc.want)
		}
	}
}

// TestFixtureNamesComeFromTheHARName：产出文件名必须是 kebab-case 的。
//
// 这是上一条用例的**对价**：slugify 改坏了，夹具目录里会同时出现四五种风格，
// 而这条用例会当场红 —— 光有单元测试不够，得有人真的用它的产出。
func TestFixtureNamesComeFromTheHARName(t *testing.T) {
	dir := t.TempDir()
	writeHAR(t, dir, "sign_in_3任务列表.har", harSpec{})
	outDir := filepath.Join(dir, "out")

	if _, _, code := extractInto(t, dir, outDir); code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	want := filepath.Join(outDir, "sign-in-3任务列表.json")
	if _, err := os.Stat(want); err != nil {
		t.Errorf("产物该叫 %s，实得 %v", filepath.Base(want), fixtureFiles(t, outDir))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 分类的小工具
// ─────────────────────────────────────────────────────────────────────────────

// TestHostOfStripsSchemePortUserAndPath：主机白名单是按**主机**判的。
//
// 端口与用户信息必须先剥掉：上游主机名带环境前缀，而 URL 里混着
// `user:pass@host:7071/…` 这种写法，不剥就匹配不上后缀。
func TestHostOfStripsSchemePortUserAndPath(t *testing.T) {
	cases := []struct{ in, want string }{
		{"https://mcloud.139.com/orchestration/api/v1/info?a=1", "mcloud.139.com"},
		{"http://user-njs.yun.139.com:7071/a/b", "user-njs.yun.139.com"},
		{"https://user:pw@caiyun.feixin.10086.cn/x", "caiyun.feixin.10086.cn"},
		{"https://MCLOUD.139.com/X", "mcloud.139.com"},
	}
	for _, tc := range cases {
		if got := hostOf(tc.in); got != tc.want {
			t.Errorf("hostOf(%q) = %q，期望 %q", tc.in, got, tc.want)
		}
	}
}

// TestIsUpstreamHostRefusesUnregisteredSuffixes：未登记的主机一律不算。
func TestIsUpstreamHostRefusesUnregisteredSuffixes(t *testing.T) {
	upstream := []string{
		"https://mcloud.139.com/a",
		"https://personal-kd-njs.yun.139.com/a",
		"https://caiyun.feixin.10086.cn/a",
	}
	foreign := []string{
		"https://collector.github.com/t",
		"https://mobile.events.data.microsoft.com/t",
		// ⚠ 后缀必须匹配**域边界**：`not139.com` 不是 `139.com` 的子域
		"https://evilnot139.com/t",
		"https://139.com.evil.test/t",
	}
	for _, url := range upstream {
		if !isUpstreamHost(url) {
			t.Errorf("%q 该算上游", url)
		}
	}
	for _, url := range foreign {
		if isUpstreamHost(url) {
			t.Errorf("%q 不该算上游（白名单是默认拒绝）", url)
		}
	}
}

// TestHasStaticExtensionIgnoresTheQueryString。
//
// 只看 `?` / `#` 之前那段：查询串里带 `.png` 的接口（真有）不该被当成静态资源，
// 而那条接口往往正是要回放的那一条 —— 跳过它不会报错，只会"少一条夹具"。
func TestHasStaticExtensionIgnoresTheQueryString(t *testing.T) {
	if !hasStaticExtension("https://mcloud.139.com/static/app.js?v=2") {
		t.Error("`/static/app.js?v=2` 该算静态资源")
	}
	if hasStaticExtension("https://mcloud.139.com/api/icon?name=a.png") {
		t.Error("`/api/icon?name=a.png` 是接口，不算静态资源")
	}
}

// 编译期钉住：回放侧的接口还在（夹具格式契约的另一端）。
var _ = core.HTTPRequest{}
