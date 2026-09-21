// 账号身份解析的黄金向量测试。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/auth.rs 的 `mod tests`。
// 向量由 scripts/gen-auth-vectors.cjs **直接 import 原版编译产物**产出。
//
// ⚠ 取向量那三个 helper 把文件名以**字面量**写进 golden.Load：黄金向量门禁靠
// 正则探测"这份向量有没有人读"，藏进变量会让探测静默失配。
package core

import (
	"encoding/json"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/golden"
)

func authVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("auth-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	return decodeVectors(t, raw, "auth-vectors.json")
}

// optStr 取一个**可缺省**的字符串字段：缺省或显式 null → nil。
//
// 与 [str] 的区别是刻意的：`nil` 与 `""` 在本模块里是**两种不同的情况**
// （见 AuthInfo 的字段注释），所以测试必须能区分"键不在"与"键是空串"。
func optStr(o map[string]any, key string) *string {
	got, ok := o[key]
	if !ok || got == nil {
		return nil
	}
	s, ok := got.(string)
	if !ok {
		return nil
	}
	return &s
}

func strEq(t *testing.T, label, field string, got, want *string) {
	t.Helper()
	switch {
	case got == nil && want == nil:
	case got == nil:
		t.Fatalf("%s: %s 应为 %q，实际缺失（nil）", label, field, *want)
	case want == nil:
		t.Fatalf("%s: %s 应为缺失（nil），实际 %q", label, field, *got)
	case *got != *want:
		t.Fatalf("%s: %s 得到 %q，期望 %q", label, field, *got, *want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// parseAuth
// ─────────────────────────────────────────────────────────────────────────────

func TestParseAuthMatchesOriginalBuild(t *testing.T) {
	v := authVectors(t)
	cases := seg(t, v, "parseAuth")
	if len(cases) == 0 {
		t.Fatal("向量不能为空")
	}

	for _, raw := range cases {
		c := obj(t, raw, "parseAuth 用例")
		name := str(t, c, "name")
		input := str(t, c, "input")
		malformed, _ := c["malformed"].(bool)
		expect, hasExpect := c["expect"].(map[string]any)

		got, ok := ParseAuth(input)

		// 两条"必须判为不可用"的路径：
		//   ① 向量自己标了 malformed —— 原版在这类输入上会产出**乱码 platform**
		//      （Node 的宽松 base64 解码 + UTF-8 有损替换），那是实现缺陷，
		//      本项目刻意**不继承**（见 auth.go 包注释"一处刻意的不一致"）；
		//   ② 向量没给 expect（空串输入）。
		if malformed || !hasExpect {
			if ok {
				t.Fatalf("%s：垃圾 auth 必须判为不可用，实际解出 %s", name, got.Describe())
			}
			continue
		}

		if !ok {
			t.Fatalf("%s：合法 auth 应能解析", name)
		}
		strEq(t, name, "auth", &got.Auth, optStr(expect, "auth"))
		strEq(t, name, "platform", got.Platform, optStr(expect, "platform"))
		strEq(t, name, "phone", got.Phone, optStr(expect, "phone"))
		strEq(t, name, "token", got.Token, optStr(expect, "token"))

		if wantExpire, ok := expect["expire"].(json.Number); ok {
			gotExpire := strconv.FormatFloat(got.Expire, 'f', -1, 64)
			if gotExpire != wantExpire.String() {
				t.Fatalf("%s: expire 得到 %s，期望 %s", name, gotExpire, wantExpire.String())
			}
		}
	}
}

// `auth` 字段是 `"Basic " + 去掉前缀后的原文`，**不是重新编码**的结果。
//
// 这条单独钉住：把 `auth` 改成"重新 base64 编码"在多数用例上看不出差别
// （向量里的输入本来就都是 4 的倍数），只有在 padding 被去掉的用例上才会分叉。
func TestParseAuthKeepsTheOriginalPayloadNotReencoded(t *testing.T) {
	// ⚠ 必须选一个**真的需要补 =** 的输入（长度 % 4 != 0），否则补与不补同形，
	// 这条断言就退化成了恒真 —— 本测试第一版就是这么写错的。
	const stripped = "MjoxOTkwMDAwMDAwMTpha3xia3xjaw" // 27 字符 → 需补一个 =
	if len(stripped)%4 == 0 {
		t.Fatal("这条断言需要一个**未对齐 4** 的输入")
	}
	info, ok := ParseAuth(stripped)
	if !ok {
		t.Fatal("应能解析")
	}
	if info.Auth != "Basic "+stripped {
		t.Fatalf("auth 应原样保留 payload：%q", info.Auth)
	}
	if info.Auth == "Basic "+PadBase64(stripped) {
		t.Fatal("不应把 base64 的 = 补回去 —— 那不是「原文」")
	}
}

// `replace("Basic ", "")` 只替换**第一处**（JS 语义）。
//
// 判据不是"解出来对不对"，而是"**去干净了会解出来、只去一处解不出来**"：
// 剩下的那半个 "Basic " 会被当成 base64 正文的一部分，滤掉空格后长度 % 4 == 1，
// 于是判为不可用。所以这条输入在两个实现下结果**相反**，能真正区分两者。
func TestParseAuthRemovesOnlyTheFirstBasicPrefix(t *testing.T) {
	const payload = "MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr"

	// 对照组：无前缀的 payload 本身是可用的
	if _, ok := ParseAuth(payload); !ok {
		t.Fatal("对照组：无前缀的 payload 应能解析")
	}
	if _, ok := ParseAuth("Basic Basic " + payload); ok {
		t.Fatal("只应去掉第一处前缀 —— 去干净会让这条输入被解出来")
	}
}

// token 里含 `:` 会被**截断**（只取前 3 段）—— 这是原版怪癖，不要"修好"。
func TestParseAuthTruncatesTokenAtColon(t *testing.T) {
	info, ok := ParseAuth("MjoxOTkwMDAwMDAwMTphOmI6Yw==")
	if !ok {
		t.Fatal("应能解析")
	}
	if info.Token == nil || *info.Token != "a" {
		t.Fatalf("token 应被截断成 \"a\"，实际 %s", orNil(info.Token))
	}
}

// `Number(...) || 0`：非法 / 空 / 0 全部归零，且**允许小数**。
func TestParseAuthExpireNormalization(t *testing.T) {
	cases := []struct {
		name  string
		input string
		want  float64
	}{
		{"expire-absent", "MjoxOTkwMDAwMDAwMTpha3xia3xjaw==", 0},
		{"not-a-number", "MjoxOTkwMDAwMDAwMTphfGJ8Y3xub3RhbnVtYmVyfGQ=", 0},
		{"zero-is-falsy", "MjoxOTkwMDAwMDAwMTphfGJ8Y3wwfGQ=", 0},
		{"negative-is-kept", "MjoxOTkwMDAwMDAwMTphfGJ8Y3wtMXxk", -1},
		{"float-is-allowed", "MjoxOTkwMDAwMDAwMTphfGJ8Y3wxLjV8ZA==", 1.5},
	}
	for _, c := range cases {
		info, ok := ParseAuth(c.input)
		if !ok {
			t.Fatalf("%s：应能解析", c.name)
		}
		if info.Expire != c.want {
			t.Fatalf("%s: expire 得到 %v，期望 %v", c.name, info.Expire, c.want)
		}
	}
}

// ⚠ `expire` 是**毫秒**。小数向零截断，与 JS 的隐式转换一致。
//
// 这条存在的理由：拿毫秒值去和秒级时间戳比会**永远判"没过期"**，
// 于是 auth 刷新永不触发、直到请求开始 401 —— 错得毫无声息。
func TestExpireMillisIsMillisecondsAndTruncatesTowardZero(t *testing.T) {
	info, ok := ParseAuth("MjoxOTkwMDAwMDAwMTphfGJ8Y3wxLjV8ZA==")
	if !ok {
		t.Fatal("应能解析")
	}
	if got := info.ExpireMillis(); got != 1 {
		t.Fatalf("1.5 应向零截断成 1，实际 %d", got)
	}

	// 13 位是毫秒的形态；秒级是 10 位。用它把单位钉死在断言里。
	full, ok := ParseAuth("MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr")
	if !ok {
		t.Fatal("应能解析")
	}
	const wantMillis = 1893456000000
	if got := full.ExpireMillis(); got != wantMillis {
		t.Fatalf("expire 得到 %d，期望 %d", got, wantMillis)
	}
	if full.ExpireMillis()/1000 != 1893456000 {
		t.Fatal("除以 1000 后应是秒级 —— 这个比值就是「单位是毫秒」的判据")
	}
}

func TestIsExpiredAtAndIsNeedRefreshAreDifferentQuestions(t *testing.T) {
	info, ok := ParseAuth("MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr")
	if !ok {
		t.Fatal("应能解析")
	}
	const expire = 1893456000000
	// 还有 30 天：没过期
	now := int64(expire - 30*86_400_000)
	if info.IsExpiredAt(now) {
		t.Fatal("还有 30 天，不该判过期")
	}
	// 但"提前 7 天刷新"的判据下，30 天 > 7 天 → 不用刷新
	if info.IsNeedRefresh(now, 7) {
		t.Fatal("还有 30 天，不该判需要刷新")
	}
	// 只剩 3 天：仍未过期，但**已经该刷新**了 —— 这就是两者不是同一件事的证据
	now = expire - 3*86_400_000
	if info.IsExpiredAt(now) {
		t.Fatal("还有 3 天，不该判过期")
	}
	if !info.IsNeedRefresh(now, 7) {
		t.Fatal("只剩 3 天，应判需要刷新")
	}
	// expire == 0 视为"无过期信息"，判为已过期（偏安全的一侧）
	zero := &AuthInfo{}
	if !zero.IsExpiredAt(0) {
		t.Fatal("expire=0 应判为已过期")
	}
}

// ⚠ 凭据不得出现在任何"顺手打印"里。
func TestDescribeRedactsTheToken(t *testing.T) {
	const secretToken = "ak|bk|ck|1893456000000|fk"
	info, ok := ParseAuth("MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr")
	if !ok {
		t.Fatal("应能解析")
	}
	desc := info.Describe()
	if strings.Contains(desc, secretToken) {
		t.Fatalf("Describe 泄漏了 token：%s", desc)
	}
	if !strings.Contains(desc, "redacted") {
		t.Fatalf("Describe 应说明 token 被隐去：%s", desc)
	}
	if !strings.Contains(desc, "19900000001") {
		t.Fatalf("手机号不在凭据之列，应可打印（便于排查）：%s", desc)
	}
}

func TestParseAuthEmptyIsUnavailable(t *testing.T) {
	if _, ok := ParseAuth(""); ok {
		t.Fatal("空串应判为不可用")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// padBase64
// ─────────────────────────────────────────────────────────────────────────────

func TestPadBase64MatchesOriginalBuild(t *testing.T) {
	v := authVectors(t)
	cases := seg(t, v, "padBase64")
	if len(cases) == 0 {
		t.Fatal("向量不能为空")
	}
	for _, raw := range cases {
		c := obj(t, raw, "padBase64 用例")
		got := PadBase64(str(t, c, "input"))
		if want := str(t, c, "expect"); got != want {
			t.Fatalf("%s：得到 %q，期望 %q", str(t, c, "name"), got, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 身份派生（identity）
// ─────────────────────────────────────────────────────────────────────────────

func TestIdentityDerivationMatchesOriginalBuild(t *testing.T) {
	v := authVectors(t)
	cases := seg(t, v, "identity")
	if len(cases) == 0 {
		t.Fatal("向量不能为空")
	}

	for _, raw := range cases {
		c := obj(t, raw, "identity 用例")
		name := str(t, c, "name")
		account := obj(t, c["account"], "account")
		auth := str(t, account, "auth")
		nickname := optStr(account, "nickname")
		configPhone := optStr(account, "phone")

		// parsedPhone 是"从 auth 现场解析出来的号码"，与"最终采用的号码"是两件事：
		// 向量里 explicit-phone-wins 那条两者的值**不同**（19999999999 vs 19900000001）。
		var parsedPhone *string
		if info, ok := ParseAuth(auth); ok {
			parsedPhone = info.PhoneOrNil()
		}
		strEq(t, name, "parsedPhone", parsedPhone, optStr(c, "parsedPhone"))

		resolved := ResolvePhone(configPhone, auth)

		if got, want := LegacyAccountKey(resolved, nickname), str(t, c, "legacyKey"); got != want {
			t.Fatalf("%s: legacyKey 得到 %q，期望 %q", name, got, want)
		}
		if got, want := string(AccountIDFor(resolved, nickname)), str(t, c, "accountId"); got != want {
			t.Fatalf("%s: accountId 得到 %q，期望 %q", name, got, want)
		}
		if got, want := AccountLabel(nickname, resolved), str(t, c, "label"); got != want {
			t.Fatalf("%s: label 得到 %q，期望 %q", name, got, want)
		}
	}
}

// 旧 state.json 的键与新的 AccountId **只在"手机号走掩码"这一点上不同** ——
// 昵称与兜底值两边必须完全一致，否则迁移时对不上。
//
// ⚠ 掩码会撞车（两个账号前 2 后 2 相同）。所以这里顺便把"撞车是可发生的"
// 记成断言：迁移必须做冲突检测，不能静默合并。
func TestLegacyKeyAndAccountIdDifferOnlyByMasking(t *testing.T) {
	nick := "账号A"
	if got := LegacyAccountKey("", &nick); got != "账号A" {
		t.Fatalf("昵称兜底应原样沿用：%q", got)
	}
	if got := string(AccountIDFor("", &nick)); got != "账号A" {
		t.Fatalf("昵称兜底应原样沿用：%q", got)
	}
	if got := LegacyAccountKey("", nil); got != "account" {
		t.Fatalf("兜底值应为 account：%q", got)
	}
	if got := string(AccountIDFor("", nil)); got != "account" {
		t.Fatalf("兜底值应为 account：%q", got)
	}

	// 掩码撞车：两个**不同**号码，前 2 后 2 相同 ⇒ 掩码相同。
	// 这正是"迁移必须检测冲突"的理由 —— 光看 AccountId 分不出这两个账号。
	a := AccountIDFor("19900000001", nil)
	b := AccountIDFor("19912345001", nil)
	if a != b {
		t.Fatalf("这两个号码的掩码应当相同（都是 19****01），实际 %q / %q", a, b)
	}
	// 而旧的键不会撞
	if LegacyAccountKey("19900000001", nil) == LegacyAccountKey("19912345001", nil) {
		t.Fatal("旧键用完整号码，不应撞车")
	}
}
