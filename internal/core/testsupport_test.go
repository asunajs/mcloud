package core

// 跨测试文件共享的**断言** —— 对应 Rust 侧 legacy/crates/mcloud-core/src/test_support.rs。
//
// 这里放的不是测试数据，而是"逻辑微妙、抄一份就会跑偏"的东西。
// 典型例子就是下面的标识扫描：它的边界判定有讲究（见函数注释），
// 两份实现迟早只有一份是对的。

import (
	"encoding/base64"
	"strings"
	"testing"
	"unicode/utf8"
)

// ⚠ 合成前缀复用 `sanitize.go` 里那个常量，**不在这里再写一个**。
//
// 两套生成器统一把手机号与 `userDomainId` 重映射到 `199` 号段，
// 于是"只允许 199 开头"成了一条可机检的不变量。
// 而这条不变量**正是 `SynthesizeIdentifiers` 在生产的那个** ——
// 守卫与生产各写一个常量，改了一边就会让"守卫放行的"与"生产产出的"悄悄分叉。

// assertOnlySyntheticIdentifiers 断言文本里的标识值**全部**是合成值（`199` 开头）。
//
// 覆盖两类形状：11 位 → 手机号；19 位 → `userDomainId`。
// 裸文本与其中的 base64 载荷都会扫一遍（[decodeBase64Payloads]）。
//
// # 为什么用"199 开头"当判据
//
// `test/fixtures/legacy/` 与 `test/golden/` 都是**要进版本库**的生成物。
// 它们由脚本从本机真实数据产出，脚本负责脱敏；这里断言的是"脱敏真的生效了"。
// 一旦有人拿真实值重新生成或手改夹具，测试立刻红，而不是等它被推到远端。
//
// # 已知盲区（剩下的那个）
//
// **掩码**（如 `19****01`）不受本断言约束 —— 它含 `*`，不是连续数字串。
// 但掩码同样是从号码派生的：由真实号 `19…01` 生成的掩码，等于泄漏前 2 后 2 位。
// 这一类只能靠人工审 —— 约定是所有示例掩码都用合成号派生。
func assertOnlySyntheticIdentifiers(t *testing.T, label, text string) {
	t.Helper()
	scanIdentifiers(tReporter{t: t}, label, text)
	for _, decoded := range decodeBase64Payloads(text) {
		scanIdentifiers(tReporter{t: t}, label+"（base64 解码后）", decoded)
	}
}

// ⚠⚠ 为什么扫描逻辑要接受一个"报告器"而不是直接拿 `*testing.T`
//
// 因为**反向测试需要接住失败**，而 Go 的 `t.Fatalf` 走 `runtime.Goexit`、
// **不是 panic** —— 它没法被 `recover` 捕获，子测试里也接不住。
//
// 于是"报告一个失败"被抽成一层：生产路径喂真正的 `*testing.T`，
// 反向测试喂一个只收集文本的替身（[collector]）。
// ⚠ 扫描逻辑本身**只有一份**（这是本文件存在的全部理由），
// 报告器换掉的只是"往哪儿报"。
type reporter interface {
	helper()
	fail(msg string)
}

type tReporter struct{ t *testing.T }

func (r tReporter) helper()         { r.t.Helper() }
func (r tReporter) fail(msg string) { r.t.Fatal(msg) }

type collector struct{ out *string }

func (c collector) helper() {}
func (c collector) fail(msg string) {
	// 只留第一条：断言要的是"它会失败"，不是"它失败几次"。
	if *c.out == "" {
		*c.out = msg
	}
}

func scanIdentifiers(rep reporter, label, text string) {
	rep.helper()
	scanWidth(rep, label, text, 11, "手机号")
	scanWidth(rep, label, text, 19, "userDomainId")
}

// scanWidth 在单段文本里扫指定宽度的数字串。
//
// # 为什么必须卡"两侧不是字母数字"
//
// 不卡边界会全是误报：`$sourceSha256` 的十六进制串里就有 `2a18531489942b83…`
// 这种恰好 11 位、又恰好以 1 开头的一段。真实标识在 JSON 里一定被引号、
// 空白或标点包着，所以"两侧非字母数字"能精确区分两者。
//
// 这条边界同时挡住了"19 位号码被 11 位规则切一段"的误判：
// `1990000000000000001` 的第 12 位是数字，右边界不成立，于是跳过。
func scanWidth(rep reporter, label, text string, width int, kind string) {
	rep.helper()
	buf := []byte(text)

	for start := 0; start+width <= len(buf); start++ {
		if buf[start] != '1' {
			continue
		}
		end := start + width
		allDigits := true
		for i := start; i < end; i++ {
			if buf[i] < '0' || buf[i] > '9' {
				allDigits = false
				break
			}
		}
		if !allDigits {
			continue
		}
		// 串首/串尾也算"边界"。
		bounded := func(i int) bool {
			if i < 0 || i >= len(buf) {
				return true
			}
			return !isASCIIAlnum(buf[i])
		}
		if !bounded(start-1) || !bounded(end) {
			continue
		}
		candidate := text[start:end]
		if !strings.HasPrefix(candidate, syntheticPrefix) {
			rep.fail(label + " 里出现了疑似真实" + kind + " " + candidate +
				"（只有 " + syntheticPrefix + " 开头的合成值才允许出现）")
			return
		}
	}
}

// decodeBase64Payloads 取出文本里所有"看起来是 base64"的片段并解码，
// 返回其中能当文本读的部分。
//
// 存在的理由是那个踩过的坑：base64 把号码从"字面"里藏起来，
// 任何基于字面正则的审计都会整段漏过。
// （Rust 侧的注释记着：`mcloud-io`/`mcloud-server` 的测试字面量里真就藏过
// 真实手机号，而当时的两道守卫都只扫字面、一个都没报。）
//
// 只收"解码后没有控制字符"的结果，用来滤掉十六进制串（sha256 之类）
// 被当成 base64 硬解出来的二进制垃圾。
func decodeBase64Payloads(text string) []string {
	// 太短的串不具备判别力，且会大量误入（真 auth 有 40+ 字符）。
	const minChunk = 16

	isB64 := func(b byte) bool {
		return isASCIIAlnum(b) || b == '+' || b == '/'
	}

	var out []string
	buf := []byte(text)
	i := 0
	for i < len(buf) {
		if !isB64(buf[i]) {
			i++
			continue
		}
		start := i
		for i < len(buf) && isB64(buf[i]) {
			i++
		}
		// 尾部填充也吃进来，否则标准引擎会因长度不对而失败。
		end := i
		for end < len(buf) && buf[end] == '=' {
			end++
		}
		chunk := string(buf[start:end])
		if len(chunk) < minChunk {
			continue
		}

		raw, err := base64.StdEncoding.DecodeString(chunk)
		if err != nil {
			raw, err = base64.RawStdEncoding.DecodeString(chunk)
		}
		if err != nil || !utf8.Valid(raw) {
			continue
		}
		decoded := string(raw)
		if hasControlRune(decoded) {
			continue
		}
		out = append(out, decoded)
	}
	return out
}

func isASCIIAlnum(b byte) bool {
	return (b >= 'A' && b <= 'Z') || (b >= 'a' && b <= 'z') || (b >= '0' && b <= '9')
}

// hasControlRune 判断是否含控制字符（`\n` / `\t` 除外，它们是合法文本）。
//
// ⚠ 用 `range` 遍历 rune 而不是逐字节：一段多字节 UTF-8 的**续字节**
// 恰好落在 0x80–0x9f 时会被逐字节判成控制字符，于是整段被误丢。
func hasControlRune(s string) bool {
	for _, r := range s {
		if r == '\n' || r == '\t' {
			continue
		}
		if r < 0x20 || r == 0x7f {
			return true
		}
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// 守卫自己的自测
// ─────────────────────────────────────────────────────────────────────────────

// TestIdentifierGuardRejectsNonSyntheticValues 守卫自己也要被守卫：
// 先证明它对**裸文本**和 **base64** 都能失败。
//
// 一个不会失败的守卫等于没有守卫 —— 这道守卫历史上就漏过一次，所以补上这组自测。
//
// ⚠ 这里刻意用**非真实的**非 199 号码：判据是形状，不需要真值。
// 把真实号码写进本文件，等于让守卫自己制造一次它要防的那件事。
func TestIdentifierGuardRejectsNonSyntheticValues(t *testing.T) {
	b64 := func(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

	cases := []struct {
		name string
		text string
		want string
	}{
		{"手机号", "phone=13800000000;", "疑似真实手机号"},
		{"userDomainId", `"userDomainId":"1000000000000000001"`, "疑似真实userDomainId"},
		{
			"藏进 base64 的手机号",
			`"auth":"Basic ` + b64("2:13800000000:t") + `"`,
			"base64 解码后",
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			var got string
			rep := collector{out: &got}
			scanIdentifiers(rep, "t", c.text)
			for _, decoded := range decodeBase64Payloads(c.text) {
				scanIdentifiers(rep, "t（base64 解码后）", decoded)
			}
			if got == "" {
				t.Fatalf("守卫本该失败，实际通过了")
			}
			if !strings.Contains(got, c.want) {
				t.Fatalf("报错应含 %q，实际 %q", c.want, got)
			}
		})
	}
}

// TestIdentifierGuardAcceptsSyntheticAndIgnoresHex 不该失败的那三类：
// 合成值放行，十六进制串里的"恰好 11 位"不误报。
func TestIdentifierGuardAcceptsSyntheticAndIgnoresHex(t *testing.T) {
	b64 := func(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }
	ok := []string{
		`"auth":"Basic ` + b64("2:19900000001:ak|bk") + `"`,
		`"userDomainId":"1990000000000000001"`,
		`"$sourceSha256":{"a":"2a18531489942b83aa2a18531489942b83"}`,
	}
	for _, text := range ok {
		var got string
		rep := collector{out: &got}
		scanIdentifiers(rep, "t", text)
		for _, decoded := range decodeBase64Payloads(text) {
			scanIdentifiers(rep, "t（base64 解码后）", decoded)
		}
		if got != "" {
			t.Fatalf("不该失败，实际报了 %q：%s", got, text)
		}
	}
}
