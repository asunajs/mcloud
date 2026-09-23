// 直播口令提取（弹幕 + 小红书主页）—— 纯字符串处理。
//
// 对应原项目 `packages/core/src/live-room/kouling.ts` 与 `live-room/xiaohongshu.ts`。
//
// # 为什么要**逐字**复刻原实现，包括它的假阳性
//
// 口令是**服务端当场校验**的：解错一个，代价是 `200108`（实拍证明**零消耗**）
// 外加一次 60 秒重听。而"顺手把某个分支写得更严"的代价不对称 ——
// 严过头会把**真口令**滤掉，而那一侧没有第二次机会（口令只在直播那一小时有效）。
//
// 所以：**行为与原实现逐字一致**，包括 `口令是相伴什么` 这种会被解成
// `相伴什么` 的假阳性（实拍弹幕里真有这一条，见黄金向量的 `live-false-positive`）。
//
// # ⚠ Go 的 `regexp` 没有前瞻，有一处必须换写法
//
// 原实现的"同一行多个口令"用了 `(?=\s+(?:复制)?口令…|无随机|$)`。
// RE2 不支持前瞻 ⇒ 这里改成**显式扫描**（见 [sameLineCodes]），
// 而不是"改成一个差不多的正则" —— 后者会静默改变切分行为。
// 边界条件（下一个口令头**必须**以空白开头、`无随机` 不要前导空白）都照抄。
//
// # ⚠ 另一个静默差异：`\s`
//
// JS 的 `\s` 含全角空格 `U+3000`，**Go 的 `\s` 只有 ASCII 那六个**。
// 弹幕里全角空格很常见（手机输入法默认就是全角），照 Go 的 `\s` 写会让
// `口令1：AAA　口令2：BBB` 变成"只有一个口令 AAA　口令2：BBB"。
// 所以下面所有 `\s` 一律走 [jsSpaceClass]。黄金向量里的
// `live-ideographic-space` 就是为这条准备的。
package upstream

import (
	"encoding/json"
	"regexp"
	"strings"
	"unicode/utf8"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 与 JS 对齐的字符类
// ─────────────────────────────────────────────────────────────────────────────

// jsSpaceClass 是 JS `\s` 的字符类。
//
// JS 的 `\s` = `[\t\n\v\f\r \u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]`。
// Go 的 `\s` 只有 `[\t\n\f\r ]`（**没有** `\v`，也没有全角空格）。
const jsSpaceClass = `[\t\n\v\f\r \x{00a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]`

// noRandomToken 是主播口令格式的收尾标记。
//
// ⚠ 它同时是"格式 4 兜底"的**触发条件**（文本含它才走那支）和
// 格式 2 的**内容终止符**。两个用法不是一回事，别合并。
const noRandomToken = "无随机"

// 口令格式的正则。
//
// 每一条都对应原实现里的一个字面量；改任何一条都要同时改
// `test/golden/live-room-vectors.json` 的 `koulingExtract` 段。
var (
	// koulingPrefixRe 是格式 1：`小红花口令：XXX`。
	koulingPrefixRe = regexp.MustCompile(`小红花口令[：:]` + jsSpaceClass + `*(.+)`)

	// koulingHeadOnlyRe 是"整行只是一个口令头"（内容在下一行）。
	koulingHeadOnlyRe = regexp.MustCompile(`(?:复制)?口令` + jsSpaceClass + `*\d*` + jsSpaceClass + `*[：:]` + jsSpaceClass + `*$`)

	// koulingHeadRe 是"口令头"本身（不带行尾锚点），同一行切分用。
	koulingHeadRe = regexp.MustCompile(`(?:复制)?口令` + jsSpaceClass + `*\d*` + jsSpaceClass + `*[：:]` + jsSpaceClass + `*`)

	// koulingHeadPrefixRe 判断一行**以口令开头**（`/^(?:复制)?口令/`）。
	//
	// ⚠ 它与 [koulingHeadOnlyRe] **不是一回事**，别合并：
	// 前者只要求"以口令二字开头"（`口令1：AAA` 也算），
	// 后者要求"整行就是个头"（必须以冒号结尾）。
	// 用途也不同：这一条用在"下一行是不是又是个头"的排除里。
	koulingHeadPrefixRe = regexp.MustCompile(`^(?:复制)?口令`)

	// koulingIsRe 是格式 3：`口令是XXX`。
	koulingIsRe = regexp.MustCompile(`口令是(.+)`)

	// koulingDigitRe 是格式 4 的单条：`1:XXX`。
	koulingDigitRe = regexp.MustCompile(`^\d+` + jsSpaceClass + `*[：:]` + jsSpaceClass + `*(.+)`)

	// koulingNoRandomRestRe 匹配 `无随机` 及其**同一行**的剩余部分（JS 的 `.` 不跨行）。
	koulingNoRandomRestRe = regexp.MustCompile(noRandomToken + `.*`)

	// trailingParticleRe 是疑问句末尾的语气词。
	trailingParticleRe = regexp.MustCompile(`[呀啊吧呢嘛哦]+$`)

	// jsSpaceRunRe 是 JS 的 `/\s+/`。
	jsSpaceRunRe = regexp.MustCompile(jsSpaceClass + `+`)
)

// questionWords 是"整条就是一句提问"的那几个词。
//
// ⚠ 判据是**去语气词后整条相等**，不是"包含"：`口令是相伴什么` 不在表里，
// 所以它会被解成口令（原实现如此，见文件头）。
var questionWords = []string{"什么", "啥", "多少", "几", "哪个", "哪些"}

// IsKoulingHead 报告这条弹幕是否**只是一个口令头**（内容在下一帧）。
//
// 用它的地方是"跨帧拼接"：主播先发 `复制口令1：`、再发 `短漫上新免费看`
// —— 分两条弹幕时，第一条单独解不出任何东西，必须记下"下一条整条就是口令"。
func IsKoulingHead(text string) bool {
	return koulingHeadOnlyRe.MatchString(strings.TrimSpace(text))
}

// HasKouling 报告这条文本**是否值得送进提取器**。
//
// 它的用处是**过滤**（小红书那边只对含关键词的候选文本做提取），
// 所以它比 [ExtractKouling] 宽松得多 —— 宽了只是多算几次，窄了会漏。
func HasKouling(text string) bool {
	return strings.Contains(text, "口令") || strings.Contains(text, "小红花")
}

// IsQuestion 报告解出来的"口令"其实是观众的一句提问。
//
// 弹幕里大量出现 `口令是什么` / `口令是啥` / `第2个口令是什么？` ——
// 它们是**观众在问**，不是主播在发。不滤掉的话每一次都会换回一个 `200108`。
func IsQuestion(code string) bool {
	if strings.ContainsAny(code, "?？") {
		return true
	}
	trimmed := trailingParticleRe.ReplaceAllString(code, "")
	for _, word := range questionWords {
		if trimmed == word {
			return true
		}
	}
	return false
}

// ExtractKouling 从一条弹幕里提取口令。
//
// # 四个分支**按顺序累加**（不是"命中一个就返回"）
//
//	① `小红花口令：XXX`
//	② 逐行扫口令头：整行是头 ⇒ 取**下一行**；行内有头 ⇒ 用 [sameLineCodes] 切
//	③ `口令是XXX`
//	④ **仅当 ①②③ 全空**且文本含 `无随机` ⇒ 按空白切、找 `N:XXX`
//
// 顺序不是风格：③ 的 `口令是(.+)` 会命中 ② 里那些行的内容，
// ④ 是"上面都没认出来时的最后兜底"，条件里那句 `result.length === 0` 不能省
// （省了它，多行格式会额外多出一批垃圾）。
//
// 最后**保序去重**（`[...new Set()]`）再过一遍疑问句过滤 ——
// 顺序对"先拿到哪个口令"有影响，去重则防止同一个口令被投两次。
func ExtractKouling(msg string) []string {
	var result []string

	// ① `小红花口令：XXX`
	if m := koulingPrefixRe.FindStringSubmatch(msg); m != nil {
		result = append(result, strings.TrimSpace(m[1]))
	}

	// ② 逐行
	//
	// ⚠ 两个分支是**互斥**的：整行只是口令头 ⇒ 取下一行，然后 `continue`
	// （**不再**做同一行切分）。原实现那个 `continue` 就是这个意思，
	// 漏掉它会让 `复制口令1：` 这一行既取到下一行、又被同一行切分算一遍。
	lines := strings.Split(normalizeNewlines(msg), "\n")
	for i, rawLine := range lines {
		line := strings.TrimSpace(rawLine)
		if line == "" {
			continue
		}
		if IsKoulingHead(line) {
			if i+1 < len(lines) {
				content := strings.TrimSpace(lines[i+1])
				// 两道排除：下一行自己也是个口令头、或含 `无随机`（那是收尾标记）。
				if content != "" && !koulingHeadPrefixRe.MatchString(content) &&
					!strings.Contains(content, noRandomToken) {
					result = append(result, content)
				}
			}
			continue
		}
		result = append(result, sameLineCodes(line)...)
	}

	// ③ `口令是XXX`
	if m := koulingIsRe.FindStringSubmatch(msg); m != nil {
		result = append(result, strings.TrimSpace(m[1]))
	}

	// ④ 纯数字编号兜底：**只在前面全空**时才走。
	if len(result) == 0 && strings.Contains(msg, noRandomToken) {
		// JS 的 `msg.replace(/无随机.*/, "")` 是**非全局**替换：只去掉第一处
		// 以及它**本行**剩下的部分（`.` 不跨行）。用 ReplaceAll 会多去掉后面几行，
		// 而那正好是"多行格式"的形状。
		if loc := koulingNoRandomRestRe.FindStringIndex(msg); loc != nil {
			msg = msg[:loc[0]] + msg[loc[1]:]
		}
		for _, part := range jsSpaceRunRe.Split(strings.TrimSpace(msg), -1) {
			if m := koulingDigitRe.FindStringSubmatch(part); m != nil {
				if code := strings.TrimSpace(m[1]); code != "" {
					result = append(result, code)
				}
			}
		}
	}

	return filterCodes(result)
}

// normalizeNewlines 把 `\r\n` 与 `\r` 归一成 `\n`（对齐 JS 的 `split(/\r?\n/)`）。
//
// ⚠ JS 那个正则其实是 `/\r?\n/`：**孤立的 `\r`**（老 Mac 换行）不会被当换行。
// 这里跟着归一化，因为弹幕是 JSON 里来的、`\r` 只会出现在 `\r\n` 里；
// 而把孤立 `\r` 当换行只会多切一刀（多切一刀最多多一个候选，不会漏）。
func normalizeNewlines(text string) string {
	return strings.ReplaceAll(text, "\r\n", "\n")
}

// sameLineCodes 处理一行里的口令头，返回这一行能解出的口令。
//
// # 它替代的是原实现那个带前瞻的正则
//
// 原式：`/(?:复制)?口令\s*\d*\s*[：:]\s*(.*?)(?=\s+(?:复制)?口令\s*\d*\s*[：:]|无随机|$)/g`
//
// 逐条照抄它的语义：
//
//   - 每个口令头的**内容**从头的末尾开始，到下面三者中**最先**出现的一个为止：
//     ① 一个**以空白开头**的下一个口令头（`\s+口令…`）；
//     ② `无随机`（**没有**前导空白要求）；
//     ③ 行尾。
//   - 所以 `口令1:A口令2:B`（中间没空格）**不会**被切成两条 ——
//     内容是 `A口令2:B`。这不是 bug，是那个前瞻要求 `\s+` 的直接后果。
//   - 下一个匹配从上一个头结束处继续扫（`g` 的语义），所以中间那段空白
//     会被跳过，不会变成一个"没有头的内容"。
//
// 返回值已 trim；空内容与含 `无随机` 的内容都丢弃（对齐原式里那两处判断）。
func sameLineCodes(line string) []string {
	var out []string
	for pos := 0; pos < len(line); {
		loc := koulingHeadRe.FindStringIndex(line[pos:])
		if loc == nil {
			break
		}
		headEnd := pos + loc[1]
		contentEnd := len(line)

		// 从头的末尾往后找第一个边界。
		for j := headEnd; j < len(line); {
			if strings.HasPrefix(line[j:], noRandomToken) {
				contentEnd = j
				break
			}
			r, size := utf8.DecodeRuneInString(line[j:])
			if size == 0 {
				break
			}
			if isJSSpace(r) {
				// 跳过整段空白，看紧跟着的是不是一个口令头。
				k := j
				for k < len(line) {
					next, n := utf8.DecodeRuneInString(line[k:])
					if n == 0 || !isJSSpace(next) {
						break
					}
					k += n
				}
				if k > j {
					if next := koulingHeadRe.FindStringIndex(line[k:]); next != nil && next[0] == 0 {
						contentEnd = j
						break
					}
				}
				j = k
				continue
			}
			j += size
		}

		code := strings.TrimSpace(line[headEnd:contentEnd])
		if code != "" && !strings.Contains(code, noRandomToken) {
			out = append(out, code)
		}
		pos = headEnd
	}
	return out
}

// isJSSpace 报告一个 rune 是否落在 JS 的 `\s` 里。
func isJSSpace(r rune) bool {
	switch r {
	case '\t', '\n', '\v', '\f', '\r', ' ',
		'\u00a0', '\u1680', '\u2028', '\u2029', '\u202f', '\u205f', '\u3000', '\ufeff':
		return true
	}
	return r >= '\u2000' && r <= '\u200a'
}

// filterCodes 保序去重 + 疑问句过滤（对齐 `[...new Set(r)].filter(c => !isQuestion(c))`）。
func filterCodes(codes []string) []string {
	seen := make(map[string]struct{}, len(codes))
	out := make([]string, 0, len(codes))
	for _, code := range codes {
		if _, ok := seen[code]; ok {
			continue
		}
		seen[code] = struct{}{}
		if IsQuestion(code) {
			continue
		}
		out = append(out, code)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 源 B：小红书主页
// ─────────────────────────────────────────────────────────────────────────────

// XiaohongshuProfileURL 是口令发布方的小红书主页。
//
// ⚠ 它是**硬编码的公开主页**（没有凭据）—— 口令除了直播弹幕之外还会在这里
// 以笔记形式发一遍，所以它是 WS 之外唯一的口令来源。
const XiaohongshuProfileURL = "https://www.xiaohongshu.com/user/profile/568caf13aed7585949773b30"

// XiaohongshuProfileRequest 构造拉主页 HTML 的请求。
//
// UA/referer 伪装成桌面 Chrome：小红书对非常规 UA 会回一个空壳页面，
// 而"空壳页面"与"这期没有口令"在解析层看起来**一模一样**（都返回 `[]`）。
func XiaohongshuProfileRequest() core.HTTPRequest {
	return core.NewRequestGet(XiaohongshuProfileURL).
		WithHeader("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8").
		WithHeader("Accept-Language", "zh-CN,zh;q=0.9").
		WithHeader("Referer", "https://www.xiaohongshu.com/").
		WithHeader("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "+
			"AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
}

var (
	xsJSONFieldRe = regexp.MustCompile(`"(?:displayTitle|title|desc)"` + jsSpaceClass + `*:` +
		jsSpaceClass + `*"((?:\\.|[^"\\])*)"`)
	xsElementRe = regexp.MustCompile(`(?i)<(?:a|article|div|h\d|p)[^>]*>([\s\S]*?)<\/(?:a|article|div|h\d|p)>`)
	xsMetaRe    = regexp.MustCompile(`(?i)<meta\b[^>]*(?:content=["']([^"']+)["'])[^>]*>`)
	xsEntityRe  = regexp.MustCompile(`(?i)&(?:#(\d+)|#x([\da-f]+)|(\w+));`)
	xsBRRe      = regexp.MustCompile(`(?i)<br\s*\/?\s*>`)
	xsBlockEnd  = regexp.MustCompile(`(?i)<\/(?:a|article|div|h\d|li|p|section)>`)
	xsTagRe     = regexp.MustCompile(`<[^>]+>`)
	xsRunSpaces = regexp.MustCompile(`[ \t]+`)
	xsRunBreaks = regexp.MustCompile(`\n` + jsSpaceClass + `+`)
	xsLineRe    = regexp.MustCompile(`\r?\n`)
)

// namedEntities 是原实现认的那六个具名实体。
//
// ⚠ 只有六个：多认几个**看起来**更对，但那是**行为变化**（原实现会把
// `&copy;` 原样留在文本里）。口令里出现实体的概率极低，而"两边解出不同的文本"
// 会让黄金向量失去意义。
var namedEntities = map[string]string{
	"amp": "&", "apos": "'", "gt": ">", "lt": "<", "nbsp": " ", "quot": `"`,
}

// DecodeHTMLEntities 解 HTML 实体（数字、hex、六个具名）。
//
// 认不出的具名实体**原样保留**（含 `&` 与 `;`），对齐原实现的
// `namedEntities[name] ?? entity`。
func DecodeHTMLEntities(text string) string {
	matches := xsEntityRe.FindAllStringSubmatchIndex(text, -1)
	if matches == nil {
		return text
	}
	var out strings.Builder
	last := 0
	for _, m := range matches {
		out.WriteString(text[last:m[0]])
		replacement := text[m[0]:m[1]] // 兜底：整段原样
		switch {
		case m[2] >= 0: // &#123;
			replacement = codePointOrRaw(text[m[2]:m[3]], 10, replacement)
		case m[4] >= 0: // &#x1f;
			replacement = codePointOrRaw(text[m[4]:m[5]], 16, replacement)
		case m[6] >= 0: // &name;
			if named, ok := namedEntities[strings.ToLower(text[m[6]:m[7]])]; ok {
				replacement = named
			}
		}
		out.WriteString(replacement)
		last = m[1]
	}
	out.WriteString(text[last:])
	return out.String()
}

func codePointOrRaw(digits string, base int, fallback string) string {
	var value int64
	for i := 0; i < len(digits); i++ {
		var digit int64
		switch c := digits[i]; {
		case c >= '0' && c <= '9':
			digit = int64(c - '0')
		case c >= 'a' && c <= 'f':
			digit = int64(c-'a') + 10
		case c >= 'A' && c <= 'F':
			digit = int64(c-'A') + 10
		default:
			return fallback
		}
		value = value*int64(base) + digit
		if value > utf8.MaxRune {
			// JS 的 `String.fromCodePoint` 在这里**抛异常**（整个提取失败）。
			// 我们保留原文 —— 更宽，而且失败的那一侧（"少一个候选"）本来就是安全的一侧。
			return fallback
		}
	}
	return string(rune(value))
}

// DecodeJSONString 把一个 JSON 字符串**体**（不含引号）解成原文。
//
// 先按 JSON 解（正确路径），解不开再手工替换 —— 顺序照抄原实现
// （`\\n` → `\\r` → `\\t` → `\\"` → `\\\\`）。顺序有意义：
// 先替换 `\\"` 会把 `\\\\\\"` 这种转义序列切错。
func DecodeJSONString(value string) string {
	var out string
	if err := json.Unmarshal([]byte(`"`+value+`"`), &out); err == nil {
		return out
	}
	replaced := value
	replaced = strings.ReplaceAll(replaced, `\n`, "\n")
	replaced = strings.ReplaceAll(replaced, `\r`, "\r")
	replaced = strings.ReplaceAll(replaced, `\t`, "\t")
	replaced = strings.ReplaceAll(replaced, `\"`, `"`)
	replaced = strings.ReplaceAll(replaced, `\\`, `\`)
	return replaced
}

// HTMLToText 把一段 HTML 片段压成纯文本。
func HTMLToText(html string) string {
	text := xsBRRe.ReplaceAllString(html, "\n")
	text = xsBlockEnd.ReplaceAllString(text, "\n")
	text = xsTagRe.ReplaceAllString(text, " ")
	text = DecodeHTMLEntities(text)
	text = xsRunSpaces.ReplaceAllString(text, " ")
	text = xsRunBreaks.ReplaceAllString(text, "\n")
	return strings.TrimSpace(text)
}

// collectCandidateTexts 按四级顺序收集**含关键词**的候选文本。
//
//	① 内嵌 JSON 的 `displayTitle` / `title` / `desc`（服务端注水的那份状态）
//	② 服务端渲染的笔记卡片（`<a|article|div|h\d|p>` 里的文本）
//	③ `<meta content="…">`（SEO 元数据）
//	④ 整页纯文本逐行兜底
//
// 顺序就是**页面顺序**，而主页是按时间倒序的 ⇒ 第一条命中即最新笔记
// （见 [ExtractKoulingFromXiaohongshuHTML]）。
func collectCandidateTexts(html string) []string {
	var candidates []string
	seen := make(map[string]struct{})
	add := func(value string) {
		// ⚠ 这里是 JS 的 `/\s+/g`，**不是** [HTMLToText] 里那个 `[ \t]+`。
		// 差别在换行：内嵌 JSON 的 `"口令是A\nB"` 解出来是**带换行**的两行，
		// 折成空格才是原实现喂给提取器的形状。用 `[ \t]+` 的话换行留着，
		// 而 [ExtractKouling] 的 `口令是(.+)` 遇换行就停 ⇒ 解出半个口令。
		text := strings.TrimSpace(jsSpaceRunRe.ReplaceAllString(DecodeHTMLEntities(value), " "))
		if text == "" || !HasKouling(text) {
			return
		}
		if _, ok := seen[text]; ok {
			return
		}
		seen[text] = struct{}{}
		candidates = append(candidates, text)
	}

	for _, m := range xsJSONFieldRe.FindAllStringSubmatch(html, -1) {
		add(DecodeJSONString(m[1]))
	}
	for _, m := range xsElementRe.FindAllStringSubmatch(html, -1) {
		add(HTMLToText(m[1]))
	}
	for _, m := range xsMetaRe.FindAllStringSubmatch(html, -1) {
		add(m[1])
	}
	for _, line := range xsLineRe.Split(HTMLToText(html), -1) {
		add(line)
	}
	return candidates
}

// ExtractKoulingFromXiaohongshuHTML 从主页 HTML 里提取**最新一条**口令。
//
// # 为什么"第一条命中即返回"
//
// 主页按时间倒序 ⇒ 页面顺序第一条可解析的口令笔记就是最新的那一期。
// 继续往下找会把**历史口令**也交出去，而那些一律回 `200108`
// —— 除了白跑一趟，还会让"无效口令 ⇒ 重听一次"这条逻辑被自己触发的噪声带偏。
func ExtractKoulingFromXiaohongshuHTML(html string) []string {
	for _, candidate := range collectCandidateTexts(html) {
		if codes := ExtractKouling(candidate); len(codes) > 0 {
			return codes
		}
	}
	return nil
}

// ParseXiaohongshuHTML 从 HTTP 响应体解出候选口令。
//
// 与 [ExtractKoulingFromXiaohongshuHTML] 分开是因为响应体要**先过 UTF-8 校验**：
// [core.HTTPResponse.Text] 会拒掉非法 UTF-8，而 `string(bytes)` 不会
// —— 后者会把一个被截断的页面变成一堆 U+FFFD 然后"解析成功、返回空"。
func ParseXiaohongshuHTML(response core.HTTPResponse) ([]string, *core.Error) {
	text, err := response.Text()
	if err != nil {
		return nil, core.NewParse("小红书主页不是合法 UTF-8: " + err.Error())
	}
	return ExtractKoulingFromXiaohongshuHTML(text), nil
}
