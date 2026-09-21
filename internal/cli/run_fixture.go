package cli

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// `fixture extract` —— HAR → 脱敏 fixture 的抽取器。
//
// 对应 Rust 侧 legacy/crates/mcloud-cli/src/fixture_extract.rs（769 行）。
//
// # 为什么在**生成侧**脱敏
//
// `har/*.har` 里含可直接复用的登录凭据（这是方案里的 P0：仓库里那 36 份 HAR
// **一个都没被用于测试**，就是因为没人敢把它们转成夹具）。夹具要进版本库，
// 所以必须在写盘之前脱敏，而不是靠"记得别提交"。
//
// # Fail closed
//
// 写盘之前会先把整份产物再扫一遍（[core.ScanForLeaks]）。
// **只要发现残留就不写、并报错退出**，绝不"先生成了再说" ——
// 一份漏了凭据的夹具一旦进版本库，删掉它也不等于它没有被人 clone 过。

// ─────────────────────────────────────────────────────────────────────────────
// HAR 结构（只读我们关心的那几层）
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ HAR 的键名是 **camelCase**（`postData` / `mimeType`）—— 那是 HAR 1.2 规范定的，
// 不是 serde 的 snake_case。别按"进 JSON 就改 snake_case"去改这些 tag：
// 这层读的是**别人写出来的**文件，键名不归我们定。
type harFile struct {
	Log harLog `json:"log"`
}

type harLog struct {
	Entries []harEntry `json:"entries"`
}

type harEntry struct {
	Request  harRequest  `json:"request"`
	Response harResponse `json:"response"`
}

type harRequest struct {
	Method   string         `json:"method"`
	URL      string         `json:"url"`
	Headers  []harNameValue `json:"headers"`
	PostData *harPostData   `json:"postData"`
}

type harPostData struct {
	Text *string `json:"text"`
}

type harResponse struct {
	Status  uint16         `json:"status"`
	Headers []harNameValue `json:"headers"`
	Content harContent     `json:"content"`
}

type harContent struct {
	Text     *string `json:"text"`
	MimeType *string `json:"mimeType"`
}

type harNameValue struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}

// ─────────────────────────────────────────────────────────────────────────────
// 报告
// ─────────────────────────────────────────────────────────────────────────────

// ExtractReport 是一次抽取的结果。
type ExtractReport struct {
	HARFiles         int
	EntriesKept      int
	SkippedPreflight int
	SkippedStatic    int
	SkippedNonJSON   int
	SkippedForeign   int
	SkippedOversize  int
	SkippedDuplicate int
	// RemovedStale 是被删掉的**陈旧夹具**数：某个 HAR 这一轮一条都没保留，
	// 但上一轮留下过同名文件。
	//
	// 不删的话它会一直躺在 `test/fixtures/` 里 —— 而它是在**更早、更宽松的
	// 脱敏规则**下生成的，恰恰是最不该继续留在版本库里的东西。
	RemovedStale int
	// ForeignHosts 是被主机白名单挡掉的主机（去重，供需要时按需放宽）。
	ForeignHosts []string
	Leaks        []string
	Written      []string
}

// ─────────────────────────────────────────────────────────────────────────────
// 门槛常量
// ─────────────────────────────────────────────────────────────────────────────

// keptHostSuffixes 是只保留这些**主机后缀**的流量。
//
// 夹具的用途是回放上游接口，而 HAR 里混着大量与之无关的流量：自家 APM 埋点、
// 系统与浏览器遥测、第三方 SDK。它们体量大、对"请求构造 + 响应解析"的验证
// 价值为零，留着只会把夹具从几 MB 撑到几十 MB，并淹没真正要验证的接口。
//
// 用后缀匹配而不是逐个枚举主机：上游主机名带环境前缀
// （`user-njs` / `personal-kd-njs` / `share-kd-njs`），域名后缀才是稳定特征。
// 挡掉的主机在报告里列出 —— **默认拒绝、可见地放宽**。
var keptHostSuffixes = []string{
	"139.com",        // mcloud.139.com / *.yun.139.com / h.139.com
	"10086.cn",       // caiyun.feixin.10086.cn / *.mail.10086.cn
	"cmcc-cs.cn",     // v.cmcc-cs.cn
	"cmpassport.com", // 短信登录链路
}

// leakScan 是脱敏自检的入口，做成变量而不是直接调 [core.ScanForLeaks]。
//
// # 为什么必须留这个缝
//
// 自检是"凭据绝不进版本库"的**最后一道闸**，所以它自己必须能被验证"真的会关上"。
// 但真实数据**触发不了**它：脱敏规则刻意铺得比自检规则更宽（阈值 32 vs 40、
// 键名黑名单两边同一份），否则每一次抽取都会因为"该被打码的长串"报警。
// 于是"有残留 → 不写盘"这条分支在线上永远走不到 —— 不注入就没有任何办法证明
// 它不是死代码，而一条从未被执行过的 fail closed 等于没有 fail closed。
//
// 这与测试里掐掉联网（[refusingClient]）是同一类缝：外部世界由调用方给定，
// 被测的是"闸关上时程序做了什么"。
var leakScan = core.ScanForLeaks

// maxResponseBytes 是单个条目响应体的上限。
//
// 超限的基本都是响应里内嵌的媒体 / base64 载荷（实测有单条 300 KB+），
// 对"请求构造 + 响应解析"的验证没有价值，却能让夹具体积翻几倍。
// 超限**整条跳过**而不是截断：截断会得到一份非法 JSON，反而更难查。
const maxResponseBytes = 128 * 1024

// maxSamplesPerEndpoint 是同一个接口最多保留几条采样。
//
// 一次抓包会把同一个接口打几十上百次（实测最多的一个 232 次），内容却几乎一样。
//
// 取 2 而不是 1：同一接口在一条流程里出现多次是常态，回放侧对重复记录是
// "按顺序消费、用完保留最后一条"，留 2 条才能覆盖到"顺序消费"这个行为本身。
const maxSamplesPerEndpoint = 2

// staticExtensions 是静态资源扩展名。
var staticExtensions = []string{
	".js", ".mjs", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp",
	".ico", ".woff", ".woff2", ".ttf", ".map", ".mp4", ".mp3",
}

// ─────────────────────────────────────────────────────────────────────────────
// 抽取
// ─────────────────────────────────────────────────────────────────────────────

func runFixtureExtract(out *output, harPath, outDir string, dryRun bool) error {
	files, err := collectHARFiles(harPath)
	if err != nil {
		return err
	}
	if len(files) == 0 {
		return fmt.Errorf("未找到任何 .har 文件：%s", harPath)
	}

	report := &ExtractReport{HARFiles: len(files)}
	if !dryRun {
		if err := os.MkdirAll(outDir, 0o755); err != nil {
			return fmt.Errorf("创建输出目录失败 %s: %w", outDir, err)
		}
	}

	// 采样数**跨 HAR 累计** —— 上限要管的是夹具总量，不是"每个文件里各自算一遍"。
	samples := map[string]int{}

	for _, path := range files {
		raw, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("读取 HAR 失败 %s: %w", path, err)
		}
		var parsed harFile
		if err := json.Unmarshal(raw, &parsed); err != nil {
			return fmt.Errorf("解析 HAR 失败 %s: %w", path, err)
		}

		entries := make([]map[string]any, 0, len(parsed.Log.Entries))
		for _, entry := range parsed.Log.Entries {
			switch classify(entry) {
			case skipPreflight:
				report.SkippedPreflight++
				continue
			case skipStatic:
				report.SkippedStatic++
				continue
			case skipNonJSON:
				report.SkippedNonJSON++
				continue
			case skipForeignHost:
				report.SkippedForeign++
				report.addForeignHost(hostOf(entry.Request.URL))
				continue
			case skipOversize:
				report.SkippedOversize++
				continue
			}

			key := core.MatchKey(entry.Request.Method, entry.Request.URL)
			if samples[key] >= maxSamplesPerEndpoint {
				report.SkippedDuplicate++
				continue
			}
			samples[key]++

			entries = append(entries, core.BuildFixtureEntry(
				entry.Request.Method, entry.Request.URL,
				flatten(entry.Request.Headers),
				[]byte(postDataText(entry.Request.PostData)),
				entry.Response.Status,
				flatten(entry.Response.Headers),
				[]byte(contentText(entry.Response.Content)),
			))
			report.EntriesKept++
		}

		outFile := filepath.Join(outDir, slugify(stemOf(path))+".json")

		if len(entries) == 0 {
			// 只删本工具自己会写出的那个文件名，不碰目录里的其他内容。
			if !dryRun {
				if _, statErr := os.Stat(outFile); statErr == nil {
					_ = os.Remove(outFile)
					report.RemovedStale++
				}
			}
			continue
		}

		serialized, err := core.MarshalJSON(entries)
		if err != nil {
			return fmt.Errorf("序列化 fixture 失败: %w", err)
		}

		// Fail closed：写盘**之前**先自检
		leaks := leakScan(string(serialized))
		for _, leak := range leaks {
			report.Leaks = append(report.Leaks, fmt.Sprintf("%s: %s", path, leak))
		}
		if dryRun || len(leaks) > 0 {
			continue
		}
		if err := os.WriteFile(outFile, serialized, 0o644); err != nil {
			return fmt.Errorf("写入 fixture 失败 %s: %w", outFile, err)
		}
		report.Written = append(report.Written, outFile)
	}

	renderExtractReport(out, report, dryRun)
	// ⚠ 有残留就**报错退出**：这是 fail closed 的另一半。
	// 只打一行警告的话，CI 会绿、夹具会进版本库。
	if len(report.Leaks) > 0 {
		return fmt.Errorf("脱敏自检失败：%d 处残留（未写出任何含残留的 fixture）", len(report.Leaks))
	}
	return nil
}

func (r *ExtractReport) addForeignHost(host string) {
	for _, known := range r.ForeignHosts {
		if known == host {
			return
		}
	}
	r.ForeignHosts = append(r.ForeignHosts, host)
}

// ─────────────────────────────────────────────────────────────────────────────
// 分类
// ─────────────────────────────────────────────────────────────────────────────

type keep int

const (
	keepYes keep = iota
	skipPreflight
	skipStatic
	skipNonJSON
	skipForeignHost
	skipOversize
)

func classify(entry harEntry) keep {
	// 预检请求没有任何业务信息
	if strings.EqualFold(entry.Request.Method, "OPTIONS") {
		return skipPreflight
	}
	if !isUpstreamHost(entry.Request.URL) {
		return skipForeignHost
	}
	if hasStaticExtension(entry.Request.URL) {
		return skipStatic
	}
	if len(contentText(entry.Response.Content)) > maxResponseBytes {
		return skipOversize
	}
	// 只保留 JSON 响应：非 JSON 的响应体对"请求构造 + 响应解析"的验证没有价值
	if isJSON(entry.Response.Content) {
		return keepYes
	}
	return skipNonJSON
}

func isJSON(content harContent) bool {
	if content.MimeType != nil && strings.Contains(strings.ToLower(*content.MimeType), "json") {
		return true
	}
	text := contentText(content)
	if text == "" {
		return false
	}
	var probe any
	return json.Unmarshal([]byte(text), &probe) == nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// hostOf 取 URL 的主机名（去 scheme、去端口、去路径、去用户信息）。
func hostOf(rawURL string) string {
	rest := rawURL
	if i := strings.Index(rest, "://"); i >= 0 {
		rest = rest[i+3:]
	}
	if i := strings.IndexAny(rest, "/?#"); i >= 0 {
		rest = rest[:i]
	}
	if i := strings.LastIndex(rest, "@"); i >= 0 {
		rest = rest[i+1:]
	}
	if i := strings.Index(rest, ":"); i >= 0 {
		rest = rest[:i]
	}
	return strings.ToLower(rest)
}

// isUpstreamHost 报告主机是否属于上游（后缀匹配，未登记的一律不算）。
func isUpstreamHost(rawURL string) bool {
	host := hostOf(rawURL)
	for _, suffix := range keptHostSuffixes {
		if host == suffix || strings.HasSuffix(host, "."+suffix) {
			return true
		}
	}
	return false
}

// hasStaticExtension 看 URL 的**路径**部分是否落在静态资源扩展名上。
//
// ⚠ 只看 `?` / `#` 之前的那段：查询串里带 `.png` 的接口（真有）不该被当成静态资源。
func hasStaticExtension(rawURL string) bool {
	path := rawURL
	if i := strings.IndexAny(path, "?#"); i >= 0 {
		path = path[:i]
	}
	path = strings.ToLower(path)
	for _, ext := range staticExtensions {
		if strings.HasSuffix(path, ext) {
			return true
		}
	}
	return false
}

func flatten(headers []harNameValue) [][2]string {
	out := make([][2]string, 0, len(headers))
	for _, header := range headers {
		out = append(out, [2]string{header.Name, header.Value})
	}
	return out
}

func postDataText(data *harPostData) string {
	if data == nil || data.Text == nil {
		return ""
	}
	return *data.Text
}

func contentText(content harContent) string {
	if content.Text == nil {
		return ""
	}
	return *content.Text
}

// collectHARFiles 收集 `.har` 文件：单个文件，或目录（**递归**）。
//
// 结果**排序**：同一份 HAR 集合必须产出同一份夹具，
// 而 os.ReadDir 的顺序在不同文件系统上不保证一致 ——
// 采样上限是按遍历顺序生效的，顺序变了夹具内容就变了。
func collectHARFiles(path string) ([]string, error) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, fmt.Errorf("路径不存在：%s", path)
	}
	if !info.IsDir() {
		return []string{path}, nil
	}
	var files []string
	walkErr := filepath.WalkDir(path, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		if strings.EqualFold(filepath.Ext(p), ".har") {
			files = append(files, p)
		}
		return nil
	})
	if walkErr != nil {
		return nil, fmt.Errorf("读取目录失败 %s: %w", path, walkErr)
	}
	sort.Strings(files)
	return files, nil
}

// stemOf 取文件名（去目录与扩展名）。
func stemOf(path string) string {
	base := filepath.Base(path)
	return strings.TrimSuffix(base, filepath.Ext(base))
}

// slugify 把 HAR 文件名转成符合仓库命名约定的夹具名（kebab-case）。
//
// HAR 名是抓包工具按页面标题写出来的，实测混着中文、空格、大写、下划线、点
// （`sign_in_3任务列表` / `玩 AI 抽支付宝红包` / `mobile.events.data.microsoft.com_2026_06_03`）。
// 直接拿来当文件名，夹具目录里就会同时出现四五种风格。
//
// 中文**保留**：它本身就是这个名字的信息量所在，折成 `-` 只会剩下一个看不懂的空壳。
func slugify(name string) string {
	var out strings.Builder
	pendingDash := false
	for _, ch := range name {
		if isASCIIAlnum(ch) || ch > 0x7f {
			if pendingDash && out.Len() > 0 {
				out.WriteRune('-')
			}
			pendingDash = false
			out.WriteRune(unicodeToLower(ch))
			continue
		}
		pendingDash = true
	}
	return out.String()
}

func isASCIIAlnum(ch rune) bool {
	return (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9')
}

func unicodeToLower(ch rune) rune {
	if ch >= 'A' && ch <= 'Z' {
		return ch + ('a' - 'A')
	}
	// 非 ASCII 用 [unicode.ToLower]：中文不受影响，带重音的拉丁字母会正确小写
	if ch > 0x7f {
		for _, lower := range strings.ToLower(string(ch)) {
			return lower
		}
	}
	return ch
}

// ─────────────────────────────────────────────────────────────────────────────
// 报告渲染
// ─────────────────────────────────────────────────────────────────────────────

func renderExtractReport(out *output, report *ExtractReport, dryRun bool) {
	verb := "已处理"
	if dryRun {
		verb = "[dry-run] 扫描"
	}
	out.human("%s %d 个 HAR 文件", verb, report.HARFiles)
	out.human("  保留条目 : %d", report.EntriesKept)
	out.human("  跳过      : 预检 %d / 静态 %d / 非 JSON %d / 非上游主机 %d / 超大响应 %d / 重复采样 %d",
		report.SkippedPreflight, report.SkippedStatic, report.SkippedNonJSON,
		report.SkippedForeign, report.SkippedOversize, report.SkippedDuplicate)
	if report.RemovedStale > 0 {
		out.human("  清理陈旧  : %d 个", report.RemovedStale)
	}
	if len(report.ForeignHosts) > 0 {
		out.human("  非上游主机: %s", strings.Join(report.ForeignHosts, ", "))
	}
	out.human("  已写出    : %d 个 fixture 文件", len(report.Written))
	for _, file := range report.Written {
		out.human("    - %s", file)
	}

	if len(report.Leaks) == 0 {
		out.human("  脱敏自检  : 通过（无残留）")
		return
	}
	// 按"残留形状"合并计数后再展示。
	//
	// 逐条罗列在真实数据上没有可读性：36 份 HAR 会产出三万余条，而同一条规则
	// 缺失会在每个条目的同一个字段上重复成百上千次。合并之后剩下的是几十条
	// "该补哪条规则"，这才是修复者要的信息。
	grouped := map[string]int{}
	order := []string{}
	for _, leak := range report.Leaks {
		desc := leak
		if _, rest, ok := strings.Cut(leak, ": "); ok {
			desc = rest
		}
		if _, seen := grouped[desc]; !seen {
			order = append(order, desc)
		}
		grouped[desc]++
	}
	sort.Strings(order)
	out.human("  脱敏自检  : 失败，%d 处残留", len(report.Leaks))
	for _, desc := range order {
		out.human("    ! [%d 次] %s", grouped[desc], desc)
	}
}
