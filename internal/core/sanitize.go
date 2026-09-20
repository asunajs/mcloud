// HAR → fixture 的脱敏规则。**纯函数**，因此脱敏逻辑本身可以单测。
// 对应 Rust 侧 legacy/crates/mcloud-core/src/sanitize.rs。
//
// # 为什么必须脱敏
//
// har/*.har 是真实流量，含可直接复用的登录凭据。fixture 要入库，
// 所以脱敏必须在**生成侧**完成，而不是靠「记得别提交」。
//
// # 与日志脱敏的区别（刻意不同）
//
// 日志里我们**保留**配置绝对路径、完整收件地址、临时文件 ID —— 它们有实际排障价值；
// 只剔除可直接复用的凭据。而 fixture 是要进版本库的静态文件，
// 因此这里对手机号等标识信息也一并遮蔽。
// 两者目标不同，不要互相套用。
//
// # ⚠ Go 侧与 Rust 侧的三处编码差异（全部已实测，不是推测）
//
//  1. **HTML 转义**：Go 的 json.Marshal 默认把 `<` `>` `&` 转义成 `\u003c` 等，
//     而 serde_json **不转义**。本模块的输出里 `<REDACTED>` 遍地都是，
//     用 json.Marshal 会产出 `\u003cREDACTED\u003e` —— 夹具内容看着还是对的，
//     但文本层面再也对不上，且 `scan_for_leaks` 与仓库守卫扫的是**文本**。
//     ⇒ 一律走 marshalJSON()，内部 SetEscapeHTML(false)。
//  2. **数字精度**：Go 默认把 JSON 数字解成 float64，19 位标识会**静默丢精度**
//     —— 末尾几位归零（float64 只有 53 位有效位，装不下 19 位十进制）。
//     ⇒ 解析一律 UseNumber()，让数字保持字面量。
//  3. **Encoder 的尾换行**：json.Encoder.Encode 会在末尾补一个 `\n`，
//     serde_json::to_vec 不会。⇒ marshalJSON() 负责裁掉。
//
// ⚠ 一处**刻意保留**的差异：科学计数法字面量（`1e2`）Rust 侧经 f64 往返会变成
// `100.0`，Go 侧按字面量原样保留 `1e2`。保留字面量**更准**（Rust 那条路会在
// 20 位以上整数上丢精度），且两侧解析出来的**值**相同；本模块的输出没有
// 任何门禁做逐字节对拍。见 TestSanitizeNumberLiteralsArePreservedNotRoundTripped。
package core

import (
	"bytes"
	"encoding/json"
	"io"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// Placeholder 是替换占位符。
const Placeholder = "<REDACTED>"

// allowedHeaders 是允许**明文**进版本库的 header 名单（已归一化：小写、去 `-`/`_`）。
//
// # 为什么 header 这边必须是白名单，而不是黑名单
//
// 黑名单在真实流量上被证伪了。把 36 份真实 HAR 跑一遍，
// cookieToken、cookieTokenKey、x-yun-tid、hecaiyundata2021jssdkcross
// 这几个**名字不在名单里**的 cookie / 追踪头带着可直接复用的凭据原样通过，
// 自检报出三万余处残留 —— 结果是一份 fixture 都写不出来。
//
// 逐个往黑名单里加名字是打地鼠：header 名由上游与抓包工具决定，我们穷举不完，
// 而漏掉一个就意味着凭据进了版本库、推上去收不回来。
// 反过来，「夹具需要哪些 header」是**有限且已知**的：匹配键只看 method + path，
// 回放只读响应体，请求 header 仅作留档。所以未登记的一律打码 ——
// 代价是偶尔多打一个无害的值，收益是**新的 cookie / 追踪头一出现就被默认挡住**。
var allowedHeaders = []string{
	// 请求内容协商
	"accept",
	"acceptencoding",
	"acceptlanguage",
	"contentencoding",
	"contentlength",
	"contenttype",
	"origin",
	"useragent",
	"xrequestedwith",
	// 注意：HTTP/2 伪头（`:path` / `:authority` / …）**刻意不在此列**。
	// 它们的值是完整路径与主机，与 request.url / method 完全重复，
	// 而 `:path` 那种长路径又会被形状规则盯上 —— 留着只会白占注意力，不如一律打码。
	// 传输与缓存
	"cachecontrol",
	"connection",
	"date",
	"expires",
	"host",
	"lastmodified",
	"pragma",
	"server",
	"transferencoding",
	"vary",
	// CORS：调试台看响应时会用到
	"accesscontrolallowcredentials",
	"accesscontrolallowheaders",
	"accesscontrolallowmethods",
	"accesscontrolalloworigin",
	"accesscontrolexposeheaders",
	"accesscontrolmaxage",
	// 抓包时机（无凭据）
	"secfetchdest",
	"secfetchmode",
	"secfetchsite",
	"secfetchuser",
}

// sensitiveFieldKeys 是敏感 body / query 字段（已归一化）。
var sensitiveFieldKeys = []string{
	"auth",
	"token",
	"accesstoken",
	"refreshtoken",
	"idtoken",
	"jwt",
	"cookie",
	"userdomainid",
	"deviceid",
	"phone",
	"mobile",
	"phonenumber",
	"password",
	"pass",
	"secret",
	"corpsecret",
	"apikey",
	"apisecret",
	"sign",
	"signature",
	"sessionid",
	"smscode",
	"verifycode",
	"authcode",
	"wxcode",
}

// volatileQueryKeys 是 URL 中每次调用都会变的参数 —— 不能进匹配键，否则 fixture 永远命中不了。
var volatileQueryKeys = []string{
	"_",
	"_t",
	"t",
	"ts",
	"timestamp",
	"nonce",
	"rand",
	"random",
	"callback",
	"_r",
}

func normalizeKey(key string) string {
	var b strings.Builder
	b.Grow(len(key))
	for _, r := range key {
		if r == '-' || r == '_' {
			continue
		}
		b.WriteRune(unicode.ToLower(r))
	}
	return b.String()
}

func containsString(list []string, want string) bool {
	for _, item := range list {
		if item == want {
			return true
		}
	}
	return false
}

// IsSensitiveField 判断 key 是否属于敏感字段（大小写/连字符/下划线无关）。
func IsSensitiveField(key string) bool {
	return containsString(sensitiveFieldKeys, normalizeKey(key))
}

// IsAllowedHeader 判断该 header 是否允许明文进版本库。**未登记的一律不允许**（见 allowedHeaders）。
func IsAllowedHeader(key string) bool {
	return containsString(allowedHeaders, normalizeKey(key))
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON 编解码 —— 全模块只走这两个入口
// ─────────────────────────────────────────────────────────────────────────────

// parseJSON 解析一个完整的 JSON 值。
//
// ⚠ UseNumber 不是可选项：不用它，19 位 userDomainId 会被解成 float64 并**静默丢精度**
// —— 末尾几位归零，而丢掉的恰好是末几位（float64 的 53 位有效位装不下 19 位十进制）。
// 用 json.Number 保留字面量，重映射后的值才写得回去。
//
// 尾随内容一律判失败：serde_json::from_str 要求整个输入就是一个 JSON 值，
// 而 json.Decoder.Decode 读完第一个值就返回 —— 不补这一刀，
// `{"a":1} trailing garbage` 在 Go 侧会被当成合法 JSON。
func parseJSON(text string) (any, bool) {
	dec := json.NewDecoder(strings.NewReader(text))
	dec.UseNumber()
	var value any
	if err := dec.Decode(&value); err != nil {
		return nil, false
	}
	if _, err := dec.Token(); err != io.EOF {
		return nil, false
	}
	return value, true
}

// marshalJSON 序列化，且**与 serde_json::to_vec 逐字节同形**。
//
// 两处必须显式处理，见包注释第 1、3 条：
//   - SetEscapeHTML(false)：否则 `<REDACTED>` 变成 `\u003cREDACTED\u003e`；
//   - 裁掉 Encode 补的尾换行：serde_json::to_vec 不补。
func marshalJSON(value any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(value); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// header
// ─────────────────────────────────────────────────────────────────────────────

// SanitizeHeaders 脱敏 header 列表。
//
//   - 不在白名单里 → **整值打码**（名字保留，留痕便于排查）；
//   - 在白名单里 → 保留，但仍要过一遍长串遮蔽。
//
// 第二条不是多余的：白名单里的 user-agent 实测就带着一截 40 字符的构建/会话 token
// （`Mozilla/5.0 … <40 字符>`），它是「这个头有用」与「这个值里有一段该藏起来」
// 同时成立 —— 整值保留会漏，整值打掉又会把有用的部分一起丢掉。
func SanitizeHeaders(headers [][2]string) [][2]string {
	out := make([][2]string, 0, len(headers))
	for _, h := range headers {
		if IsAllowedHeader(h[0]) {
			out = append(out, [2]string{h[0], ScrubText(h[1])})
		} else {
			out = append(out, [2]string{h[0], Placeholder})
		}
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON 递归脱敏
// ─────────────────────────────────────────────────────────────────────────────

// SanitizeJSON 递归脱敏 JSON，返回**新的值**与「是否发生了修改」。
//
// 返回值必须接住：map / slice 是引用类型，原地改了；但**根是字符串或数字时改不了**
// （Go 的 any 装的是值），只有靠返回值传出来。Rust 那边是 `&mut Value`，
// 所以它没有这个问题 —— 这正是从 Rust 照搬签名会踩空的地方。
//
// 两条规则**叠加**，缺一不可：
//
//  1. 键名命中黑名单 → 整值打码；
//  2. 其余**字符串值**里的长串再按形状打码。
//
// 第 2 条不是锦上添花。只按键名判断在真实流量上会大量放行 ——
// `{"xxx":"<base64 的 mobile:手机号:token>"}`、`{"params":"<JWT>"}`
// 这类「键名无害、值是凭据」的形态很常见，实测 36 份 HAR 里到处都是。
// 键名由上游决定，我们无法预先知道哪个字段会装凭据，所以值这一侧必须照形状兜住。
// 这与 ScanForLeaks 的兜底是**同一条思路的两次实现**，阈值刻意不同（见 maskStringValue）。
func SanitizeJSON(value any) (any, bool) {
	changed := false

	switch v := value.(type) {
	case map[string]any:
		for key, child := range v {
			if IsSensitiveField(key) {
				// 已经是占位符就不算改动 —— 否则每次生成夹具都报「改过」，
				// 「本次脱敏动了什么」这个信号就没了。
				if s, ok := child.(string); !ok || s != Placeholder {
					v[key] = Placeholder
					changed = true
				}
				continue
			}
			next, childChanged := SanitizeJSON(child)
			if childChanged {
				v[key] = next
				changed = true
			}
		}
	case []any:
		for i, item := range v {
			next, childChanged := SanitizeJSON(item)
			if childChanged {
				v[i] = next
				changed = true
			}
		}
	case string:
		masked := maskStringValue(v)
		if masked != v {
			return masked, true
		}
	case json.Number:
		// 数字也要过一遍：JSON 里 19 位标识常常写成**裸数字**而不是字符串，
		// 只处理字符串会让它带着真实值进版本库（守卫按形状扫原始文本，扫得到）。
		text := v.String()
		if isAllASCIIDigits(text) {
			synthesized := SynthesizeIdentifiers(text)
			if synthesized != text {
				if replacement, err := strconv.ParseUint(synthesized, 10, 64); err == nil {
					return json.Number(strconv.FormatUint(replacement, 10)), true
				}
			}
		}
	}

	return value, changed
}

// maskStringValue 对单个字符串值做脱敏。
//
//   - 整串是 URL → 走 RedactURL：**保留路径**，只处理 query。
//     路径是夹具可读性与匹配的来源，不该被当成 token 打掉；
//     而 query 里的凭据由 RedactURL 负责。
//   - 整串是**内嵌 JSON** → 递归脱敏后再序列化回来。
//   - 其余 → 把长串打码，阈值取 32。
//
// # 为什么「内嵌 JSON」必须单独处理
//
// 真实 HAR 里这**不是特例而是常态**：抓包工具会把整份请求体当成 JSON 字符串再包一层，
// 于是真正有意义的字段在第二层。只看外层会把它当成一段普通文本 ——
// 键名黑名单在第二层完全失效，而自检会递归进去（见 scanValue），
// 两边深度不一致，结果就是「自检报出一堆本该被脱敏的残留」。
// 实测 userdomain_id 就是这么漏的。
//
// 阈值比 ScanForLeaks 的 40 **更低**是刻意的：脱敏要覆盖得比自检更宽，
// 自检才可能对「脱敏漏网的串」报警。若两边同阈值同字符集，自检就会与脱敏一起盲掉。
func maskStringValue(text string) string {
	if looksLikeURL(text) {
		return RedactURL(text)
	}
	// 只有**结构化**的内嵌内容才递归：纯数字/纯文本串按 JSON 解析也会成功
	// （`123` 是合法 JSON），不加以限制的话 `1e2` 这种会被「解析成功、脱敏无改动」
	// 一路走到重序列化，把字面量改写成 `1e2` 之外的形态。
	if inner, ok := parseJSONStructured(text); ok {
		if sanitized, changed := SanitizeJSON(inner); changed {
			if out, err := marshalJSON(sanitized); err == nil {
				return string(out)
			}
			return text
		}
	}
	return ScrubText(text)
}

// parseJSONStructured 只在解析结果是**对象或数组**时返回 true。
func parseJSONStructured(text string) (any, bool) {
	value, ok := parseJSON(text)
	if !ok {
		return nil, false
	}
	switch value.(type) {
	case map[string]any, []any:
		return value, true
	}
	return nil, false
}

// ─────────────────────────────────────────────────────────────────────────────
// body
// ─────────────────────────────────────────────────────────────────────────────

// SanitizeBody 脱敏请求体：JSON 走结构化脱敏，表单体逐对脱敏，其余退化为「长 token 串」遮蔽。
func SanitizeBody(body []byte) []byte {
	if len(body) == 0 {
		return nil
	}
	// Rust 的 std::str::from_utf8 就是「整段必须是合法 UTF-8」，Go 用 utf8.Valid 等价。
	if !utf8.Valid(body) {
		// 二进制体（如图片/压缩流）不解析，整体打码以免误带凭据。
		return []byte("<BINARY " + strconv.Itoa(len(body)) + " bytes>")
	}
	text := string(body)

	if value, ok := parseJSON(text); ok {
		sanitized, _ := SanitizeJSON(value)
		if out, err := marshalJSON(sanitized); err == nil {
			return out
		}
		return body
	}
	if isFormEncoded(text) {
		return []byte(strings.Join(redactFormPairs(text), "&"))
	}
	return []byte(ScrubText(text))
}

// ─────────────────────────────────────────────────────────────────────────────
// 形状规则
// ─────────────────────────────────────────────────────────────────────────────

// isTokenChar 是「token 长相」的字符集。
//
// 脱敏（RedactTokenLikeRuns）与自检（tokenLikeRuns）**必须共用它**。
// 少一个 `-` 就会让带连字符的 CDN URL 被切成碎片（`/a-<80 字符>-b.png` 断成三段），
// 碎片的形状已经不像 URL，于是逐个被当成疑似凭据报警 —— 而脱敏侧把整串当 URL
// 保留、什么都没做。两边定义不一致，门禁就会永远红且无从修复。
// 两份规则现在只差阈值（32 对 40），这才是刻意留的差异。
func isTokenChar(r rune) bool {
	return (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') ||
		r == '+' || r == '/' || r == '=' || r == '_' || r == '.' || r == '-'
}

// RedactTokenLikeRuns 把形如长 token 的连续串遮蔽。
//
// 判定标准：`[A-Za-z0-9+/=_.-]` 且长度 ≥ 32，且**不是 URL 形状**（URL 刻意保留）。
//
// 这是脱敏侧唯一的「形状兜底」，RedactURL 的路径段、header 值、非 JSON 文本体
// 全都走它 —— 好处是这几处的判定完全一致，不会出现
// 「URL 里放行、header 里打码」这种两侧对不上的缝。
//
// 阈值 32 比 ScanForLeaks 的 40 **更低**是刻意的：脱敏要覆盖得比自检更宽，
// 自检才可能对「脱敏漏网的串」报警。若两边同阈值，自检就会与脱敏一起盲掉。
func RedactTokenLikeRuns(text string) string {
	const minLen = 32
	var out strings.Builder
	out.Grow(len(text))
	var run strings.Builder

	flush := func() {
		if run.Len() >= minLen && !looksLikeURL(run.String()) {
			out.WriteString(Placeholder)
		} else {
			out.WriteString(run.String())
		}
		run.Reset()
	}

	for _, r := range text {
		if isTokenChar(r) {
			run.WriteRune(r)
		} else {
			flush()
			out.WriteRune(r)
		}
	}
	flush()
	return out.String()
}

// syntheticPrefix 是合成号段前缀（11 位手机号与 19 位 userDomainId 共用）。
const syntheticPrefix = "199"

// ScrubText 是文本级脱敏的**唯一入口**：先按形状打码，再把标识形状的数字改成合成号段。
//
// 两件事必须捆在一起走同一个入口 —— 这一路上踩到的坑几乎都是「某条分支只做了其中一件」：
// URL 分支只跑 RedactURL、非 JSON 文本只跑形状打码、JSON 字符串没递归……
// 分散调用迟早会再裂开一条缝。
func ScrubText(text string) string {
	return SynthesizeIdentifiers(RedactTokenLikeRuns(text))
}

// SynthesizeIdentifiers 把「标识形状」的数字重映射到合成号段，其余原样保留。
//
// # 为什么必须做，而且判据要和仓库守卫逐条对齐
//
// scripts/check-identifiers.cjs 是一条**形状**判定：11 位或 19 位、以 `1` 开头、
// 两侧不是字母数字。它分不清「这是 userDomainId 还是 taskId」，也不该去分 ——
// 它要的是一条可机检的不变量：**进版本库的这个形状的值必须 199 开头**。
//
// 而真实流量里撞上这个形状的远不止号码（实测 taskId 就是 19 位 `1…`）。
// 于是「反正不是标识」这条退路并不存在，生成侧必须把这类值改掉。
// 这里是**重映射**而不是打码：taskId 这类字段要参与响应解析，掩码会让它失去形状。
//
// # 映射是确定性的
//
// 用原值派生新数字（sha256），同输入永远同输出。
// 夹具要进版本库，随机重映射会让每次重新生成的 diff 全是噪声，
// 也让人没法用「两次生成的差异」判断上游是否真的变了。
func SynthesizeIdentifiers(text string) string {
	runes := []rune(text)
	var out strings.Builder
	out.Grow(len(text))

	for i := 0; i < len(runes); {
		if !isASCIIDigit(runes[i]) {
			out.WriteRune(runes[i])
			i++
			continue
		}
		start := i
		for i < len(runes) && isASCIIDigit(runes[i]) {
			i++
		}
		var before, after rune
		var hasBefore, hasAfter bool
		if start > 0 {
			before, hasBefore = runes[start-1], true
		}
		if i < len(runes) {
			after, hasAfter = runes[i], true
		}
		out.WriteString(remapIdentifier(string(runes[start:i]), before, hasBefore, after, hasAfter))
	}
	return out.String()
}

// remapIdentifier 处理单个数字串。判据与守卫的 SHAPES 一致，包括「两侧不能是字母数字」。
func remapIdentifier(run string, before rune, hasBefore bool, after rune, hasAfter bool) string {
	length := len([]rune(run))
	isIdentifierShape := (length == 11 || length == 19) &&
		strings.HasPrefix(run, "1") &&
		!(hasBefore && isASCIIAlphanumeric(before)) &&
		!(hasAfter && isASCIIAlphanumeric(after))

	if !isIdentifierShape {
		return run
	}
	tailLen := length - len(syntheticPrefix)
	return syntheticPrefix + syntheticDigits(run, tailLen)
}

// syntheticDigits 由原值确定性地派生 count 个十进制数字。
//
// ⚠ 走的是 **hex 字符串的每个字符**（不是摘要的每个字节）：
// sha256_hex 产出 64 个 ASCII 字符，逐字符取十六进制位值再 mod 10。
// 「按字节取模」会得到另一套完全不同的数字 —— 而它同样「看起来是随机的」，
// 所以错了不会被发现，只会让已入库的夹具与重新生成的夹具对不上。
func syntheticDigits(original string, count int) string {
	hexText := SHA256Hex([]byte(original))
	out := make([]byte, 0, count)
	for i := 0; i < len(hexText) && len(out) < count; i++ {
		nibble, ok := hexDigitValue(hexText[i])
		if !ok {
			continue
		}
		out = append(out, byte('0')+nibble%10)
	}
	return string(out)
}

func hexDigitValue(b byte) (byte, bool) {
	switch {
	case b >= '0' && b <= '9':
		return b - '0', true
	case b >= 'a' && b <= 'f':
		return b - 'a' + 10, true
	case b >= 'A' && b <= 'F':
		return b - 'A' + 10, true
	}
	return 0, false
}

func isASCIIDigit(r rune) bool { return r >= '0' && r <= '9' }

func isAllASCIIDigits(text string) bool {
	if text == "" {
		return false
	}
	for i := 0; i < len(text); i++ {
		if text[i] < '0' || text[i] > '9' {
			return false
		}
	}
	return true
}

func isASCIIAlphanumeric(r rune) bool {
	return (r >= '0' && r <= '9') || (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z')
}

// looksLikeURL 判断一个「token 长相」的长串其实是 URL / 路径 —— 这类内容我们**故意保留**
// （fixture 回放必须靠它匹配请求），不能当成泄漏误报。
//
// 只按形状判断，不依赖上下文：
//   - 以 `//` 开头：URL 被 `:` 截断后就是这个样子（`https://x` → `https` + `//x`）；
//   - 或 `host/path` 形状：第一段像域名。
//
// # 主机名里允许 `:` 与 `-`
//
// 少了 `:` 会把带端口的主机名判成凭据（`data.example.com:8080/newapp/action`
// 在实测里就是这么被误报的），少了 `-` 会漏掉连字符域名。
// 放宽这两个字符是安全的：真正的凭据走 base64/hex，**base64 字母表里没有 `.`**，
// 而这里的判定要求第一段含 `.` —— 这个条件本身就把 base64 挡在外面了。
//
// 之所以要放宽到「只看形状」：误报会让门禁永远红，比漏报更有害
// —— 真正的第一道防线是 IsSensitiveField 的字段名黑名单。
func looksLikeURL(run string) bool {
	if strings.HasPrefix(run, "//") || strings.HasPrefix(run, "http") {
		return true
	}
	// 只取第一段当主机名判断。取整串会在路径含 `:` 时误判 ——
	// 而长串里一旦出现 `:`，后面的片段就已经不带主机名了。
	host := run
	if idx := strings.IndexByte(run, '/'); idx >= 0 {
		host = run[:idx]
	}
	if host == "" || !strings.Contains(host, ".") {
		return false
	}
	for _, r := range host {
		if r >= 'a' && r <= 'z' {
			continue
		}
		if r >= '0' && r <= '9' {
			continue
		}
		if r == '.' || r == '-' || r == ':' {
			continue
		}
		return false
	}
	return true
}

func tokenLikeRuns(text string) []string {
	const minLen = 40
	var result []string
	var run strings.Builder

	flush := func() {
		if run.Len() >= minLen && !looksLikeURL(run.String()) {
			result = append(result, run.String())
		}
		run.Reset()
	}

	for _, r := range text {
		if isTokenChar(r) {
			run.WriteRune(r)
		} else {
			flush()
		}
	}
	flush()
	return result
}

// ─────────────────────────────────────────────────────────────────────────────
// URL
// ─────────────────────────────────────────────────────────────────────────────

// MatchKey 是 URL 的匹配键：`METHOD host/path`，丢弃全部 query。
//
// 这是 fixture 查找的键 —— 只看「打到哪个接口」，
// 不看易变的 query，否则每次抓包都生成一批命不中的 fixture。
func MatchKey(method, url string) string {
	noScheme := url
	if idx := strings.Index(url, "://"); idx >= 0 {
		noScheme = url[idx+3:]
	}
	noQuery := noScheme
	if idx := strings.IndexAny(noQuery, "?#"); idx >= 0 {
		noQuery = noQuery[:idx]
	}
	return strings.ToUpper(method) + " " + noQuery
}

// RedactURL 返回 URL 的可存档形式：去掉易变参数、遮蔽敏感参数值、遮蔽没名分的长串。
//
// 与 MatchKey 的分工：MatchKey 用于查表，本函数用于**存档展示**。
//
// # 为什么还要管「没名分的长串」
//
// 只按参数名判断是不够的（同 SanitizeJSON 的道理）：实测有 URL 把整段
// base64 塞在 query 里（`?e=<1280 字符>`），参数名 `e` 无害，值才是要藏的东西。
// 这类参数没有任何业务价值 —— 匹配键本来就丢弃整个 query —— 所以直接打码。
func RedactURL(url string) string {
	rawBase := url
	query := ""
	if idx := strings.IndexByte(url, '?'); idx >= 0 {
		rawBase, query = url[:idx], url[idx+1:]
	}
	// 路径段也要过一遍长串遮蔽：一条 URL 里出现 `:` 或 `?` 之后，剩下的片段
	// 已经不带主机名，形状上就和凭据无法区分。整条是 URL 形状的部分照旧原样保留
	// （`https://host/a/b/c` 是一个 URL 形状的长串，判定会放过它），可读性不受影响。
	base := ScrubText(rawBase)
	if query == "" {
		return base
	}

	kept := redactFormPairs(query)
	if len(kept) == 0 {
		return base
	}
	return base + "?" + strings.Join(kept, "&")
}

// maxFormKeyLen 是表单键名的最大长度。超了就不是字段名了。
const maxFormKeyLen = 64

// redactFormPairs 处理 `k=v&k=v` 形状的逐对脱敏。**query 与表单体共用**。
//
// # 为什么必须逐对处理，而不是整串按形状打码
//
// 形状打码的字符集含 `=`，于是 `optkeyword=<长值>` 会被整段替换掉 ——
// **参数名一起没了**。实测真实流量里这个接口就是把参数放在表单体里的，
// 结果夹具里只剩 `<REDACTED>`，既不看不出调的是哪个字段，也没法用来验证请求构造。
// 逐对处理只打码值，键名留下；同时 volatileQueryKeys 与敏感键名判断照旧生效。
func redactFormPairs(pairs string) []string {
	var out []string
	for _, pair := range strings.Split(pairs, "&") {
		if pair == "" {
			continue
		}
		k, v := pair, ""
		if idx := strings.IndexByte(pair, '='); idx >= 0 {
			k, v = pair[:idx], pair[idx+1:]
		}
		// 键名也要过一遍脱敏：判据之外还留着一条口子 ——
		// 整串就是一段 base64 载荷、里面恰好有 `=` 时，`k` 就是那一段载荷。
		key := ScrubText(k)
		if containsString(volatileQueryKeys, normalizeKey(k)) {
			continue
		}
		if IsSensitiveField(k) || RedactTokenLikeRuns(v) != v {
			out = append(out, key+"="+Placeholder)
		} else {
			out = append(out, key+"="+ScrubText(v))
		}
	}
	return out
}

// isFormEncoded 判断这段文本是否是 `k=v&k=v` 形状的表单体。
//
// 判据刻意保守，两条都要：
//  1. 每个非空段都必须含 `=`；
//  2. **键名要像字段名**（非空且不超过 maxFormKeyLen）。
//
// 第 2 条是补上来的：只判「含 `=`」会把**一整段 base64 载荷**（末尾带 `=` 填充）
// 误判成表单，于是键名位置留了一整段载荷 —— 实测正是这样漏出 60 处残留。
// 宁可漏判（退回形状打码，照样安全），也不要把一段载荷当成表单拆开。
func isFormEncoded(text string) bool {
	if text == "" || strings.ContainsAny(text, "{}\n") {
		return false
	}
	for _, pair := range strings.Split(text, "&") {
		if pair == "" {
			continue
		}
		idx := strings.IndexByte(pair, '=')
		if idx <= 0 || idx > maxFormKeyLen {
			return false
		}
	}
	return true
}

// ─────────────────────────────────────────────────────────────────────────────
// 自检
// ─────────────────────────────────────────────────────────────────────────────

// maxEmbeddedJSONDepth 是内嵌 JSON 的最大解析层数。
//
// 「JSON 里套 JSON」在本项目里是常态而非特例：BuildFixtureEntry 会把整份请求体
// 序列化成**一个字符串**存进 request.body。解析出来再按结构扫，才能区分「键名」与「值」——
// 否则 telemetry 类请求体里那些 URL 形状的**键名**会被当成疑似凭据：
// 它们既不该报，也没法靠脱敏规则消掉（打码键名会毁掉结构）。
const maxEmbeddedJSONDepth = 4

// ScanForLeaks 扫描一段文本（通常是已脱敏的 fixture 序列化结果），列出疑似残留。
//
// **这个函数本身也要被测** —— 否则「检查通过」可能只是检查器失灵。
//
// # 为什么报告里必须带位置
//
// 只报「长度 89 的疑似 token」是**没法据以修复**的：修复者不知道是哪个接口、
// 哪个字段、哪一侧（请求还是响应），只能靠猜。实测在 36 份真实 HAR 上跑本自检时，
// 报出的三万余条残留全是这种没有位置的信息 —— 门禁是红的，但没人能把它转绿。
// 所以 JSON 输入一律**按叶子逐条定位**，报出 `$` 路径。
//
// # 为什么只按叶子扫，而不是扫整段序列化文本
//
// JSON 里一个「token 长相的长串」必然落在某个字符串叶子里；按叶子扫描之后，
// `$.request.headers[3].value` 这样的路径才既指得出位置、又对得上修复动作。
// 顺带消掉一类噪音：整段文本扫描会把键名与长数字也算进「疑似 token」。
//
// # 报告里不出现值本身
//
// 只给 JSON 路径、形状分类与长度，**连前缀都不给**。这份报告是给修复者定位用的，
// 不该反过来成为一条新的泄漏通道。
func ScanForLeaks(text string) []string {
	var leaks []string
	if value, ok := parseJSON(text); ok {
		scanValue(value, "$", 0, &leaks)
	} else {
		// 非 JSON（表单体、日志片段）没有结构可依，退回整段形状扫描
		collectRunLeaks(text, "$（非 JSON 文本）", &leaks)
	}
	return leaks
}

// scanValue 按 JSON 结构逐叶子定位残留。
func scanValue(value any, path string, depth int, leaks *[]string) {
	switch v := value.(type) {
	case map[string]any:
		for key, child := range v {
			childPath := path + "." + key
			if IsSensitiveField(key) {
				redacted := false
				switch c := child.(type) {
				case string:
					redacted = c == Placeholder || c == ""
				case nil:
					redacted = true
				}
				if !redacted {
					*leaks = append(*leaks, "敏感字段未脱敏: "+childPath)
				}
			}
			// `match.key` 是**查表键**，不是载荷，因此不按载荷来查。
			// 两个理由都是结构性的，不是「为了让它变绿」：
			// 1. 它一旦被打码，FixtureHttpClient 用真实请求算出的键就永远命不中它；
			// 2. 它的形状天然不像 URL —— 主机带端口时（`host:7071/…`），
			//    `:` 会把主机名切掉，只剩 `7071/<长路径>` 这种「端口 + 路径」碎片，
			//    任何按形状的判定都必然误报。真正该查的是它派生自的 request.url。
			if key == "key" && strings.HasSuffix(path, ".match") {
				continue
			}
			// `headers` 是 `[[名字, 值], …]`。名字是元数据（SanitizeHeaders 也不动它），
			// 按载荷查会误报：实测响应头 `cross-origin-embedder-policy-report-only`
			// 本身就是 41 个字符的连字符串，形状上和一个 token 没有区别。
			if key == "headers" {
				if pairs, ok := child.([]any); ok {
					for index, pair := range pairs {
						items, ok := pair.([]any)
						if !ok || len(items) < 2 {
							continue
						}
						scanValue(items[1], childPath+"["+strconv.Itoa(index)+"][1]", depth, leaks)
					}
					continue
				}
			}
			scanValue(child, childPath, depth, leaks)
		}
	case []any:
		for index, item := range v {
			scanValue(item, path+"["+strconv.Itoa(index)+"]", depth, leaks)
		}
	case string:
		// 只有**结构化**的内嵌内容才递归：纯数字/纯文本串按 JSON 解析也会成功
		// （`123` 是合法 JSON），若不加限制，一条 40 位纯数字的串会被当成
		// 「已解析、无残留」而静默放过。
		if depth < maxEmbeddedJSONDepth {
			if inner, ok := parseJSONStructured(v); ok {
				scanValue(inner, path, depth+1, leaks)
				return
			}
		}
		collectRunLeaks(v, path, leaks)
	}
}

// collectRunLeaks 收集一段文本里「token 长相」的长串残留。
func collectRunLeaks(text, path string, leaks *[]string) {
	for _, run := range tokenLikeRuns(text) {
		*leaks = append(*leaks, "疑似 token 明文于 "+path+"（"+describeRun(run)+"，长度 "+strconv.Itoa(len(run))+"）")
	}
}

// describeRun 把「token 长相」分类。修复者据此判断该往哪条规则里补 ——
// 是补键名黑名单，还是补形状规则。
func describeRun(run string) string {
	if strings.HasPrefix(run, "eyJ") && strings.Count(run, ".") >= 2 {
		return "JWT"
	}
	if isAllASCIIDigits(run) {
		return "纯数字串"
	}
	if isAllASCIIHex(run) {
		return "十六进制串"
	}
	return "base64/编码串"
}

func isAllASCIIHex(text string) bool {
	if text == "" {
		return false
	}
	for i := 0; i < len(text); i++ {
		if _, ok := hexDigitValue(text[i]); !ok {
			return false
		}
	}
	return true
}

// ─────────────────────────────────────────────────────────────────────────────
// fixture 生成
// ─────────────────────────────────────────────────────────────────────────────

// BuildFixtureEntry 生成一条 fixture 记录（已脱敏）。
func BuildFixtureEntry(
	method, url string,
	requestHeaders [][2]string,
	requestBody []byte,
	status uint16,
	responseHeaders [][2]string,
	responseBody []byte,
) map[string]any {
	// JSON 响应只留结构化那一份。
	//
	// body 与 body_json 是同一份内容的两种写法，两份都存等于把夹具体积翻倍 ——
	// 实测 36 份 HAR 转出来 70 MB，其中一半是这份重复。FixtureHttpClient
	// 本来就优先用 body_json，回放行为不受影响；非 JSON 响应没有结构化写法，
	// 仍然存 body（body_json 为 null 是判据）。
	responseJSON := responseBodyJSON(responseBody)
	var responseText string
	if responseJSON == nil {
		responseText = string(SanitizeBody(responseBody))
	}

	return map[string]any{
		"match": map[string]any{
			"key":    MatchKey(method, url),
			"method": strings.ToUpper(method),
		},
		"request": map[string]any{
			"url":     RedactURL(url),
			"headers": headersToAny(SanitizeHeaders(requestHeaders)),
			"body":    string(SanitizeBody(requestBody)),
		},
		"response": map[string]any{
			"status":  status,
			"headers": headersToAny(SanitizeHeaders(responseHeaders)),
			"body":    responseText,
			// 结构化响应体（已脱敏），回放时优先用它
			"body_json": responseJSON,
		},
	}
}

// headersToAny 把 header 列表转成 JSON 可序列化的 `[["名字","值"], …]` 形状。
//
// [2]string 本身能序列化成数组，但它是**定长数组**，混进 []any 组成的树里
// 会让 scanValue 的类型开关漏掉它 —— 自检于是看不见 header 值，
// 「无泄漏」的断言就成了空话。所以统一转成 []any。
func headersToAny(headers [][2]string) []any {
	out := make([]any, 0, len(headers))
	for _, h := range headers {
		out = append(out, []any{h[0], h[1]})
	}
	return out
}

// responseBodyJSON 解析并脱敏响应体。
//
// 用 let-chain 把「能解析成 UTF-8」与「能解析成 JSON」两步合成一个条件：
// 任一步失败都返回 nil（非 JSON 响应本来也没有结构化内容可回放）。
func responseBodyJSON(body []byte) any {
	if !utf8.Valid(body) {
		return nil
	}
	value, ok := parseJSON(string(body))
	if !ok {
		return nil
	}
	sanitized, _ := SanitizeJSON(value)
	return sanitized
}
