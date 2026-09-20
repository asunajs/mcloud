// 账号身份解析 —— 与 TS 原版逐字对齐。
// 对应 Rust 侧 legacy/crates/mcloud-core/src/auth.rs。
//
// # 为什么这层必须精确到"怪癖"
//
// `auth` 字符串解出来的 phone 决定 [AccountID]，而 AccountID 是旧
// `data/state.json` 的唯一键。解析错一位，历史数据就会挂到**错误的账号**上 ——
// 而且不会报错，只会静默串号。所以这里连"token 里带冒号会被截断"这种怪癖都要保留。
//
// 对拍基准：test/golden/auth-vectors.json，由 scripts/gen-auth-vectors.cjs
// **直接 import 原版编译产物**产出。
//
// # auth 的格式
//
//	auth  = ["Basic "] + base64("<platform>:<phone>:<token>")
//	token = "<f0>|<f1>|<f2>|<expire>|..."   // 第 4 段（下标 3）是过期时间
//
// # expire 的单位是**毫秒**（实测，不是推断）
//
// 旧项目把它和 `Date.now()` 直接相减（`isNeedRefresh` 里
// `expireTime - Date.now() < days * 86400000`），实测真实数据里也是 **13 位**。
// **单位不是秒。**
//
// 这一点值得单独写一段，因为它错得毫无声息：拿 13 位的毫秒值去和秒级时间戳比，
// 结果永远是"没过期"，于是 auth 刷新永不触发、直到请求开始 401。
// 所以本模块的所有访问器**把单位写进名字**（[AuthInfo.ExpireMillis]，
// 而不是含义随上下文漂移的 `ExpireUnix`），让"混用单位"在阅读时就显形。
//
// # 保留的原版怪癖（**不要"顺手修好"**）
//
//  1. `replace("Basic ", "")` 在 JS 里**只替换第一处**，所以 `"Basic Basic x"`
//     只会去掉一个前缀；
//  2. 解码后按 `:` 切分且**只取前 3 段**，第 4 段起直接丢弃 → token 里含 `:`
//     会被截断；
//  3. `Number(...) || 0`：非法/空/NaN/0 全部归零，且**允许小数**（如 `1.5`）；
//     JS 的 `Number` 会先 trim 再解析 —— 这里由 [isPlainDecimal] 对齐，
//     顺带挡掉 Go 会认而 JS 不认的 `"inf"`；
//  4. 解析结果的 `auth` 字段是 `"Basic " + 去掉前缀后的原文`，
//     **不是重新编码**的结果。
//
// # 一处刻意的不一致
//
// Node 的 `Buffer.from(s, "base64")` 是**宽松**的：非法字符被忽略、且**不报错**。
// 所以原版遇到 `"!!!not-base64!!!"` 不会失败，而是解码出乱码字节、再被 UTF-8
// 有损解码成一堆 U+FFFD，最后产出一个**平台标识是乱码但对象非 nil** 的结果。
// 这是实现缺陷而不是设计意图（真实输入的 auth 一定是合法 base64）。
//
// 因此本实现在以下三种情况下判为"不可用"：**长度对不齐 4**、
// **解不出合法 UTF-8**、**解出零字节**。这是**更严格，且不继承乱码**。
// 向量里这类用例标记为 `malformed: true`，只断言"被判为不可用"。
//
// 为什么不逐字复刻 Node 的宽松解码：实测它并不遵守"滤掉非法字符再按 4 对齐"
// （`"!!!not-base64!!!"` → 7 字节、`"basic " + 48 字符 base64` → 39 字节，
// 都推不出来），复刻既无价值又脆弱。功能上真正要保证的只有两条：
// **合法 auth 解析结果逐字一致**；**垃圾 auth 一律判为不可用**
// （从而回退到昵称/`"account"`，而不是把垃圾串映射成某个"看起来像账号"的 id）。
package core

import (
	"encoding/base64"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

// AuthInfo 是解析后的账号身份。
//
// ⚠ 本结构体**没有实现 fmt.Stringer、也没有暴露字段的 %v 打印**，
// 这是刻意的：`Token` 是可直接复用的凭据，任何一次日志打印都会把它写进日志。
// 需要打印时用 [AuthInfo.Describe]，它只输出"有没有"与长度。
//
// （Rust 侧靠手写 `Debug` 达到同样效果；Go 的 `%v` 对结构体默认打印全部字段，
// 所以这里的做法是**不给**便捷打印入口，逼调用方显式选 [AuthInfo.Describe]。）
type AuthInfo struct {
	// Platform 是平台号（如 "2" / "4"）。原版缺字段时是 undefined → nil。
	Platform *string
	// Phone 是手机号。原版缺字段时是 undefined → nil；
	// ⚠ **空串与缺失是两种情况**。
	Phone *string
	// Token 是业务 token（凭据）。同上，缺失与空串有别。
	Token *string
	// Auth 是规范化后的 auth：`"Basic " + 原始 payload`（不重新 base64 编码）。
	Auth string
	// Expire 是过期时间（**epoch 毫秒**，见包注释）。
	//
	// 保留 float64 是为了逐字对齐原版的 `Number(...) || 0` —— 它**允许小数**
	// （向量里就有 `1.5` 这一例）。要整数时用 [AuthInfo.ExpireMillis]。
	Expire float64
}

// Describe 返回可安全打印的描述：凭据只暴露"有没有"与长度。
//
// 存在这个方法的理由：不给它，各处就会自己拼 `fmt.Sprintf("%+v", info)`，
// 而那会把 token 写进日志 —— 一次就够了。
func (a *AuthInfo) Describe() string {
	token := "<nil>"
	if a.Token != nil {
		token = "<redacted " + strconv.Itoa(utf8.RuneCountInString(*a.Token)) + " 字符>"
	}
	return "AuthInfo{Platform: " + orNil(a.Platform) +
		", Phone: " + orNil(a.Phone) +
		", Token: " + token +
		", Expire: " + strconv.FormatFloat(a.Expire, 'f', -1, 64) + "}"
}

// ExpireMillis 返回过期时间的整数形式（**epoch 毫秒**）。
//
// 名字里带单位是刻意的：原版这层叫 `expire_unix` 一类的名字，
// 读者很容易默认它是秒，然后把毫秒值拿去和秒级时间戳比 —— 那会永远判"没过期"。
// 小数向零截断，与 JS 的隐式转换一致（`1.5` → `1`，`-1` → `-1`）。
func (a *AuthInfo) ExpireMillis() int64 {
	if math.IsNaN(a.Expire) || math.IsInf(a.Expire, 0) {
		return 0
	}
	return int64(a.Expire)
}

// IsExpiredAt 判断是否已过期（nowMillis 为 **epoch 毫秒**）。
//
// `Expire == 0` 视为"无过期信息"，判为已过期 —— 与旧版 `isNeedRefresh(0, …)` 同向。
func (a *AuthInfo) IsExpiredAt(nowMillis int64) bool {
	return a.ExpireMillis() <= nowMillis
}

// IsNeedRefresh 判断是否到了该刷新的时候，对齐旧版 `isNeedRefresh(expireTime, days)`：
// `expireTime - Date.now() < days * 86400000`。
//
// **不是** [AuthInfo.IsExpiredAt] 的别名：它提前 `days` 天触发，
// 所以"没过期"不代表"不用刷新"。两件事分开命名，避免将来有人拿错一个。
func (a *AuthInfo) IsNeedRefresh(nowMillis int64, days float64) bool {
	remaining := a.Expire - float64(nowMillis)
	return remaining < days*86_400_000
}

// PhoneOrNil 返回手机号（缺失或空串都返回 nil），供身份派生使用。
//
// 之所以把空串也当缺失：原版写的是 `account.phone || parseAuth(...)?.phone || ""`，
// JS 里 `""` 是 falsy，会继续往下回退。
func (a *AuthInfo) PhoneOrNil() *string {
	if a.Phone == nil || *a.Phone == "" {
		return nil
	}
	return a.Phone
}

// PadBase64 复刻 `padBase64`：补 `=` 到 4 的倍数（已经是倍数则原样返回）。
//
// 用**字符数**而非字节数对齐 JS 的 `String.length`：对纯 ASCII 的 base64 输入
// 两者相同，只有在含星体平面字符时才与 JS 的 UTF-16 码元数不同
// （真实 auth 不会出现）。
func PadBase64(s string) string {
	rem := utf8.RuneCountInString(s) % 4
	if rem == 0 {
		return s
	}
	return s + strings.Repeat("=", 4-rem)
}

// nodeLikeB64Decode 是宽松解码：容忍 `=` 之后的垃圾与不属于字母表的字符，
// 但**拒绝空洞结果**。
//
// 规则：
//   - 只取第一个 `=` 之前的部分（`=` 即结束）；
//   - 丢弃所有不属于**标准** base64 字母表的字符（刻意**不**接受 base64url 的
//     `-`/`_`：真实 auth 是标准编码，把它们当合法字符只会让垃圾输入更容易
//     "解码成功"）；
//   - 剩余长度 `% 4 == 1` → 失败（末尾一个孤立字符凑不出完整字节，属于编码错误）；
//   - 解码失败，或解出**零字节** → 失败。
//
// 这里的宽松是**照抄输入习惯**，不是为了复刻 Node 的字节级行为（理由见包注释）。
func nodeLikeB64Decode(s string) ([]byte, bool) {
	var payload strings.Builder
	for _, c := range s {
		if c == '=' {
			break
		}
		if isStdB64Char(c) {
			payload.WriteRune(c)
		}
	}
	body := payload.String()
	if body == "" || len(body)%4 == 1 {
		return nil, false
	}
	out, err := base64.RawStdEncoding.DecodeString(body)
	if err != nil || len(out) == 0 {
		return nil, false
	}
	return out, true
}

func isStdB64Char(c rune) bool {
	switch {
	case c >= 'A' && c <= 'Z', c >= 'a' && c <= 'z', c >= '0' && c <= '9':
		return true
	case c == '+' || c == '/':
		return true
	}
	return false
}

// ParseAuth 解析 `auth` 字符串。对齐 `parseAuth` / `doParseAuth`。
//
//   - 入参为空串 → 不可用（第二个返回值为 false）；
//   - 解不出合法 UTF-8 → 不可用（见包注释"一处刻意的不一致"）；
//   - 其余情况一律返回 `true`，**即使 Phone/Token 缺失**
//     （原版也只在这时返回非 null）。
func ParseAuth(auth string) (*AuthInfo, bool) {
	if auth == "" {
		return nil, false
	}

	// JS 的 String.replace(字符串, "") 只替换第一处
	clean := strings.Replace(auth, "Basic ", "", 1)
	padded := PadBase64(clean)
	decoded, ok := nodeLikeB64Decode(padded)
	if !ok {
		return nil, false
	}
	if !utf8.Valid(decoded) {
		return nil, false
	}
	raw := string(decoded)

	// 只取前 3 段：第 4 段起被丢弃（token 含 ":" 会被截断）
	parts := strings.SplitN(raw, ":", 4)
	var platform, phone, token *string
	if len(parts) > 0 {
		platform = strPtr(parts[0])
	}
	if len(parts) > 1 {
		phone = strPtr(parts[1])
	}
	if len(parts) > 2 {
		token = strPtr(parts[2])
	}

	// Number(token?.split("|")[3]) || 0
	var expire float64
	if token != nil {
		fields := strings.Split(*token, "|")
		if len(fields) > 3 {
			field := strings.TrimSpace(fields[3])
			if isPlainDecimal(field) {
				if v, err := strconv.ParseFloat(field, 64); err == nil && v != 0 {
					// JS 的 `|| 0`：0 与 -0 都是 falsy
					expire = v
				}
			}
		}
	}

	return &AuthInfo{
		Platform: platform,
		Phone:    phone,
		Token:    token,
		Auth:     "Basic " + clean,
		Expire:   expire,
	}, true
}

// isPlainDecimal 判断该字段是否是 JS `Number()` 会认的**普通十进制数**
// （可带正负号、小数、指数）。
//
// 直接解析会有两处与 JS 不同，都必须挡住：
//  1. Go 的 ParseFloat **不接受**首尾空白，JS 的 `Number` 会 trim 后取 12 ——
//     所以调用方先 TrimSpace 再进来；
//  2. Go 的 ParseFloat **接受** `"inf"` / `"infinity"` / `"nan"`（大小写不敏感），
//     而 `Number("inf")` 是 `NaN`。
//
// 非有限值在这里统一归零，含义是"无过期信息 → 判为已过期 → 触发刷新"。
// 这是**故意偏安全的一侧**：宁可多刷一次，也不要把一个来路不明的 token
// 当成永不过期。
func isPlainDecimal(s string) bool {
	b := []byte(s)
	if len(b) == 0 {
		return false
	}
	i := 0
	if b[i] == '+' || b[i] == '-' {
		i++
	}
	intStart := i
	for i < len(b) && isDigit(b[i]) {
		i++
	}
	intDigits := i - intStart

	fracDigits := 0
	if i < len(b) && b[i] == '.' {
		i++
		fracStart := i
		for i < len(b) && isDigit(b[i]) {
			i++
		}
		fracDigits = i - fracStart
	}
	// `.` 两侧至少要有数字，否则 `.`、`+.` 之类会被当成 0
	if intDigits == 0 && fracDigits == 0 {
		return false
	}
	if i == len(b) {
		return true
	}

	// 指数部分
	if b[i] != 'e' && b[i] != 'E' {
		return false
	}
	i++
	if i < len(b) && (b[i] == '+' || b[i] == '-') {
		i++
	}
	expStart := i
	for i < len(b) && isDigit(b[i]) {
		i++
	}
	return i > expStart && i == len(b)
}

func isDigit(c byte) bool { return c >= '0' && c <= '9' }

// ResolvePhone 解析账号手机号。对齐 `resolvePhone`：配置里的 phone 优先，
// 其次从 auth 现场解析。
//
// 空串按缺失处理（JS falsy 语义）。
func ResolvePhone(configPhone *string, auth string) string {
	if configPhone != nil && *configPhone != "" {
		return *configPhone
	}
	if info, ok := ParseAuth(auth); ok {
		if p := info.PhoneOrNil(); p != nil {
			return *p
		}
	}
	return ""
}

// AccountLabel 是账号展示名。对齐 `accountLabel`：昵称优先，否则手机号掩码，
// 都没有则 `"账号"`。
func AccountLabel(nickname *string, phone string) string {
	if nickname != nil && *nickname != "" {
		return *nickname
	}
	if phone == "" {
		return "账号"
	}
	return MaskPhone(phone)
}

// LegacyAccountKey 是旧项目的存储键。对齐 `accountKey`：手机号 → 昵称 → `"account"`。
//
// ⚠ **这就是旧 `data/state.json` 的键**，迁移时靠它把历史数据对到账号上。
func LegacyAccountKey(phone string, nickname *string) string {
	if phone != "" {
		return phone
	}
	if nickname != nil && *nickname != "" {
		return *nickname
	}
	return "account"
}

// AccountIDFor 是新项目的账号标识 [AccountID]。对齐 `accountId`。
//
// 与 [LegacyAccountKey] 的唯一差别：**手机号走掩码**（`19****01`），
// 免得把完整号码当成数据库主键四处引用；昵称/兜底值则原样沿用。
//
// ⚠ 代价是掩码可能撞车（两个账号前 2 后 2 相同）。因此迁移时**必须做冲突检测
// 并报错**，绝不能静默合并 —— 见 io 层的导入实现。
func AccountIDFor(phone string, nickname *string) AccountID {
	if phone == "" {
		return AccountID(LegacyAccountKey(phone, nickname))
	}
	return AccountID(MaskPhone(phone))
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// strPtr 取字符串的地址。Go 没有"取字面量地址"的语法，
// 而 [AuthInfo] 的三个字段必须是"可能不存在"的 —— 用 `*string` 表达，
// 因为 nil 与空串在原版里是**两种不同的情况**（见字段注释）。
func strPtr(s string) *string { return &s }

// orNil 把可空字符串渲染成 `"<nil>"` 或原值。
func orNil(p *string) string {
	if p == nil {
		return "<nil>"
	}
	return *p
}
