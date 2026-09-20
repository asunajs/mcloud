// 系统用户与会话的测试。
//
// 对应 Rust 侧 legacy/crates/mcloud-core/src/user.rs 的 `#[cfg(test)]` 模块，
// **外加**一份黄金向量消费者（见文件后半）—— Rust 侧没有那一半，
// 因为那份向量的用途恰恰是"跨语言对拍"，在单语言内部没有意义。
package core

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/golden"
)

// seqRandom 是**逐字节 +1** 的确定性随机源。
//
// 与 Rust 侧 user.rs 测试里的 `SeqRandom(u8)` 同构 —— 黄金向量的 `$hashPassword`
// 段就是用那个生成的，所以这里必须逐字节对齐。
//
// ⚠ 它与本包已有的 [SeededRng] **不是一回事**：SeededRng 是 splitmix64，
// 用途是钉住设备指纹那条字节流；这里的用途只是"让盐可复现"。
// 混用会让向量里每一串盐都对不上，而现象只是"哈希不同"——
// 看起来像实现错了，其实是夹具用错了。
type seqRandom struct{ next uint8 }

// FillBytes 实现 [Random]。
//
// ⚠ `next++` 在 uint8 上的回绕是 Go 的**定义行为**（无符号整数回绕），
// 与 Rust 的 `wrapping_add(1)` 一致 —— 那里必须显式写 `wrapping_add`，
// 因为在 debug 构建下 Rust 的 `+` 会 panic。
func (r *seqRandom) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 平移自 Rust 侧的 10 条
// ─────────────────────────────────────────────────────────────────────────────

func TestPasswordRoundtrip(t *testing.T) {
	hash, err := HashPassword("correct-horse", &seqRandom{next: 7})
	if err != nil {
		t.Fatalf("哈希失败: %v", err)
	}

	ok, err := VerifyPassword("correct-horse", hash)
	if err != nil {
		t.Fatalf("正确口令应校验成功，得到错误: %v", err)
	}
	if !ok {
		t.Fatal("正确口令应校验成功")
	}

	ok, err = VerifyPassword("wrong-horse", hash)
	if err != nil {
		// ⚠ 这一条是关键：**口令不对是正常业务结果，不是错误**。
		t.Fatalf("口令不对应是正常结果，不该报错: %v", err)
	}
	if ok {
		t.Fatal("错误口令不该校验通过")
	}
}

func TestSamePasswordWithSameSaltIsReproducible(t *testing.T) {
	// 盐注入的意义：同一盐必须得到同一编码串，否则这条链路无法被单测钉住。
	a, err := HashPassword("correct-horse", &seqRandom{next: 1})
	if err != nil {
		t.Fatalf("哈希失败: %v", err)
	}
	b, err := HashPassword("correct-horse", &seqRandom{next: 1})
	if err != nil {
		t.Fatalf("哈希失败: %v", err)
	}
	if a != b {
		// 同上：显式转 string，绕开 PasswordHash 的 Stringer。
		t.Fatalf("同一口令 + 同一盐必须得到同一串:\n%s\n%s", string(a), string(b))
	}

	// 不同盐必须得到不同串（否则说明盐没起作用）。
	c, err := HashPassword("correct-horse", &seqRandom{next: 2})
	if err != nil {
		t.Fatalf("哈希失败: %v", err)
	}
	if a == c {
		t.Fatalf("不同盐不该得到同一串: %s", string(a))
	}
}

func TestPasswordHashIsArgon2idNotABareDigest(t *testing.T) {
	hash, err := HashPassword("correct-horse", &seqRandom{next: 3})
	if err != nil {
		t.Fatalf("哈希失败: %v", err)
	}
	s := string(hash)
	if !strings.HasPrefix(s, "$argon2id$") {
		t.Fatalf("应以 $argon2id$ 开头: %s", s)
	}
	// 裸 SHA-256 是 64 位十六进制；这里是带参数的 PHC 串。
	if !strings.Contains(s, "$m=") {
		t.Fatalf("应带 argon2 参数段: %s", s)
	}
}

func TestShortPasswordIsRejected(t *testing.T) {
	_, err := HashPassword("short", &seqRandom{})
	if err == nil {
		t.Fatal("短口令应被拒")
	}
	if !strings.Contains(err.Error(), "至少") {
		t.Fatalf("错误文案应提到长度下限: %v", err)
	}
	if kind, ok := KindOf(err); !ok || kind != KindBadRequest {
		t.Fatalf("短口令应是参数错误，得到: %v", err)
	}
	if MinPasswordLen != 8 {
		t.Fatalf("口令下限应为 8，得到 %d", MinPasswordLen)
	}

	// ⚠ 按**字符数**而不是字节数算：否则中文口令的下限会悄悄变成"3 个字"。
	// 8 个汉字 = 24 字节，按字节算早就过了，按字符算刚好卡在下限上。
	if _, err := HashPassword("八个汉字的口令啊", &seqRandom{}); err != nil {
		t.Fatalf("8 个字符的中文口令应被接受: %v", err)
	}
	if _, err := HashPassword("七个汉字的口令", &seqRandom{}); err == nil {
		t.Fatal("7 个字符的中文口令应被拒（判据是字符数，不是字节数）")
	}
}

func TestBrokenHashIsAnErrorNotAWrongPassword(t *testing.T) {
	// 库里的哈希被写坏 ≠ 用户输错密码。混为一谈会让故障永远查不出来。
	ok, err := VerifyPassword("whatever", PasswordHash("not-a-phc-string"))
	if err == nil {
		t.Fatalf("格式非法的哈希应报错，而不是返回'不匹配'（ok=%v）", ok)
	}
	if kind, isCore := KindOf(err); !isCore || kind != KindInternal {
		t.Fatalf("应归为内部错误，得到: %v", err)
	}
}

func TestSessionTokenIsHighEntropyAndOnlyHashIsStored(t *testing.T) {
	token := NewSessionToken(&seqRandom{next: 9})
	if len(token) != SessionTokenBytes*2 {
		t.Fatalf("应是 %d 位十六进制，得到 %d 位", SessionTokenBytes*2, len(token))
	}

	stored := HashSessionToken(token)
	if len(stored) != 64 {
		t.Fatalf("SHA-256 十六进制应为 64 位，得到 %d 位", len(stored))
	}
	if stored == token {
		t.Fatal("库里存的不能是明文")
	}
	// 同样的 token 必须得到同样的哈希（否则登录态校验不成立）。
	if HashSessionToken(token) != stored {
		t.Fatal("同一 token 必须得到同一哈希")
	}
}

func TestSessionExpiryIsPureAndBoundaryIsClosed(t *testing.T) {
	expires := time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC)
	session := NewSession("h", NewUserID("u1"), expires, SessionWeb)

	before := expires.Add(-time.Second)
	if !session.IsValidAt(before) {
		t.Fatal("到期前应有效")
	}
	// 到期时刻本身算过期（`<=` 而不是 `<`）：宁可让用户重登，
	// 也不要放行一个刚好到点的会话。
	if !session.IsExpiredAt(expires) {
		t.Fatal("到期时刻本身应算过期")
	}
	if session.IsValidAt(expires) {
		t.Fatal("到期时刻不该算有效")
	}

	// ⚠ 边界判据必须是"比较瞬时"而不是 `ExpiresAt == now`：
	// `time.Time` 的 `==` 还比较 location 与单调时钟读数，
	// 两个"同一瞬时"的时间可以 `!=`。用同一个瞬时、但**换个 Location** 构造，
	// 就是这条的判据 —— 用 `==` 实现的话它会漏。
	shanghai := expires.In(LocalTZ())
	if !session.IsExpiredAt(shanghai) {
		t.Fatalf("同一瞬时换个时区表示后，边界判定必须不变（%s）", shanghai)
	}
}

func TestUnknownRoleDegradesToLeastPrivilege(t *testing.T) {
	// 反过来（未知 → Admin）会让一次脏数据变成提权。
	if ParseRole("admin") != RoleAdmin {
		t.Fatal("admin 应解析为管理员")
	}
	if ParseRole("") != RoleUser {
		t.Fatal("空值应降级为普通用户")
	}
	if ParseRole("root") != RoleUser {
		t.Fatal("未知名应降级为普通用户")
	}
	// ⚠ Go 的零值必须落在最小权限上：任何"忘了赋值"的路径都不该拿到管理员。
	var zero Role
	if zero != RoleUser {
		t.Fatalf("Role 的零值应是最小权限，得到 %v", zero)
	}

	if ParseSessionKind("web") != SessionWeb {
		t.Fatal("web 应解析为 Web")
	}
	if ParseSessionKind("cli") != SessionCLI {
		t.Fatal("cli 应解析为 CLI")
	}
	if ParseSessionKind("unknown") != SessionCLI {
		t.Fatal("未知名应降级为 CLI")
	}
}

func TestDataIsolationOnlyMatchesTheSameUser(t *testing.T) {
	alice := NewUserID("user-alice")
	bob := NewUserID("user-bob")

	if !IsVisibleTo(alice, alice) {
		t.Fatal("自己的资源必须可见")
	}
	// 交叉读取必须不可见 —— 这是"数据按用户隔离"的最小断言。
	if IsVisibleTo(alice, bob) {
		t.Fatal("别人的资源必须不可见")
	}
	if IsVisibleTo(bob, alice) {
		t.Fatal("别人的资源必须不可见")
	}
}

func TestRoleAndSessionKindRoundTripThroughTextStorage(t *testing.T) {
	// SQLite 里 Role / SessionKind 以文本存储，这里钉住"取值与解析一致"。
	//
	// ⚠ Rust 侧这条测的是 serde 的 rename_all="lowercase"；Go 侧没有 serde，
	// 对应物是 String() / ParseX 这一对 —— 但**落库字面量必须完全相同**，
	// 否则同一个库里 Rust 写的行 Go 读不回来。
	if RoleAdmin.String() != "admin" {
		t.Fatalf("RoleAdmin 的存储形式应为 admin，得到 %q", RoleAdmin.String())
	}
	if RoleUser.String() != "user" {
		t.Fatalf("RoleUser 的存储形式应为 user，得到 %q", RoleUser.String())
	}
	if SessionWeb.String() != "web" {
		t.Fatalf("SessionWeb 的存储形式应为 web，得到 %q", SessionWeb.String())
	}
	if SessionCLI.String() != "cli" {
		t.Fatalf("SessionCLI 的存储形式应为 cli，得到 %q", SessionCLI.String())
	}

	for _, r := range []Role{RoleAdmin, RoleUser} {
		if ParseRole(r.String()) != r {
			t.Fatalf("Role 往返失败: %v", r)
		}
	}
	for _, k := range []SessionKind{SessionWeb, SessionCLI} {
		if ParseSessionKind(k.String()) != k {
			t.Fatalf("SessionKind 往返失败: %v", k)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 打印脱敏
// ─────────────────────────────────────────────────────────────────────────────

// TestPasswordHashIsRedactedWhenPrinted 钉住"**打印时**脱敏"，而不是"存储时脱敏"。
//
// ⚠ 这是 Go 相对 Rust 多出来的一层风险：Rust 只有 `{:?}` 一条路能打出结构体，
// Go 有三条 —— `%v`（走 Stringer）、`%+v`（**展开字段，不走 struct 的 Stringer**）、
// `%#v`（走 GoStringer）。所以"给 User 写个 String()"只挡住了第一条；
// 把 PasswordHash 做成**有 Stringer 的类型**才挡得住 `%+v`。
func TestPasswordHashIsRedactedWhenPrinted(t *testing.T) {
	const secret = "$argon2id$v=19$m=19456,t=2,p=1$AQIDBAUGBwgJCgsMDQ4PEA$878U+0v6GO2t1pLRqMja0KD8VQdkhMB93Wh2uOGAhmc"
	u := NewUser("u1", "alice", PasswordHash(secret), RoleAdmin, time.Unix(0, 0).UTC())

	for _, verb := range []string{"%v", "%+v", "%#v"} {
		if got := fmt.Sprintf(verb, u); strings.Contains(got, secret) {
			t.Fatalf("`%s` 泄漏了口令哈希: %s", verb, got)
		}
	}
	if got := fmt.Sprintf("%v", PasswordHash(secret)); got != "<已隐藏>" {
		t.Fatalf("PasswordHash 的 %%v 应为脱敏值，得到 %q", got)
	}

	// ⚠ 但 JSON 序列化必须**保持真值**：落库要的就是它。
	// 脱敏只能发生在"给人看"的那条路上，不能顺手把存储也改了。
	raw, err := json.Marshal(PasswordHash(secret))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if string(raw) != `"`+secret+`"` {
		t.Fatalf("PasswordHash 的 JSON 应为真值，得到 %s", raw)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 黄金向量（跨语言对拍）
// ─────────────────────────────────────────────────────────────────────────────

// passwordHashVectors 读 Argon2 的黄金向量。
//
// ⚠ 文件名必须以**字面量**出现在 golden.Load 的调用里，理由见 crypto_test.go 顶部。
func passwordHashVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("password-hash-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	return decodeVectors(t, raw, "password-hash-vectors.json")
}

// TestHashPasswordVectors 逐字比对本包产出的 PHC 串与 Rust 侧。
//
// 这条同时钉住四件事，任何一件偏了都会让串逐字不等：
// 盐由注入的 Random 决定、盐的 base64 是**无填充**、参数段的**顺序**是 m,t,p、
// 以及 argon2 的默认参数与输出长度。
func TestHashPasswordVectors(t *testing.T) {
	v := passwordHashVectors(t)
	cases := seg(t, v, "$hashPassword")
	if len(cases) == 0 {
		t.Fatal("$hashPassword 不能为空")
	}

	for _, raw := range cases {
		c := obj(t, raw, "$hashPassword 用例")
		plain := str(t, c, "plain")
		seed := numInt(t, c, "seed")
		want := str(t, c, "encoded")

		got, err := HashPassword(plain, &seqRandom{next: uint8(seed)})
		if err != nil {
			t.Fatalf("哈希 %q 失败: %v", plain, err)
		}
		if string(got) != want {
			// ⚠ 这里必须显式 `string(got)`：`PasswordHash` 实现了 Stringer，
			// 直接 `%s` 只会印出 `<已隐藏>` —— 于是断言失败时看不到"哪里不同"。
			// 脱敏是给**生产日志**用的，不是给测试失败信息用的
			// （向量里的串本来就是公开的测试数据，不是真凭据）。
			t.Fatalf("口令 %q（seed=%d）逐字不等:\n得到 %s\n期望 %s", plain, seed, string(got), want)
		}
	}
}

// TestVerifyPasswordVectors 逐条复算 `$verify` 的三态。
//
// ⚠⚠ 这里必须处理 `$crossLanguageGaps`：向量的 `expect` 是 **Rust 侧**的事实，
// 而有两处 Go 物理上做不到（argon2d 无导出实现、IDKey 的版本写死），
// 硬按 Rust 的 expect 断言会得到"Go 实现有 bug"的假象。
// 但**也不能把它们从向量里删掉** —— 删掉就没人知道差异存在。
func TestVerifyPasswordVectors(t *testing.T) {
	v := passwordHashVectors(t)

	// 用 encoded 当键而不是下标：向量的顺序会变，下标对上只是巧合。
	gaps := map[string]string{}
	for _, raw := range seg(t, v, "$crossLanguageGaps") {
		g := obj(t, raw, "$crossLanguageGaps 条目")
		gaps[str(t, g, "encoded")] = str(t, g, "goExpect")
	}

	hit := map[string]bool{}
	verified := 0
	for _, raw := range seg(t, v, "$verify") {
		c := obj(t, raw, "$verify 用例")
		plain := str(t, c, "plain")
		encoded := str(t, c, "encoded")
		rustExpect := str(t, c, "expect")

		if goExpect, isGap := gaps[encoded]; isGap {
			hit[encoded] = true
			// 登记过的差异：按 Go 侧自己的期望断言，并把 Rust 侧的期望印进失败信息 ——
			// 将来 Rust 侧行为变了，从日志里能立刻分辨是"差异消失了"还是"又错了一处"。
			assertVerifyOutcome(t, plain, encoded, goExpect,
				"跨语言差异，Rust 侧为 "+rustExpect)
			continue
		}

		assertVerifyOutcome(t, plain, encoded, rustExpect, str(t, c, "rustError"))
		verified++
	}

	// ⚠ 反向检查：登记的每条差异都必须在 $verify 里找得到。
	// 否则这张表会慢慢变成"没人验证的注释"，而它恰恰是留给未来的人看的。
	for encoded := range gaps {
		if !hit[encoded] {
			t.Fatalf("$crossLanguageGaps 里登记的串不在 $verify 中: %s", encoded)
		}
	}

	// ⚠ 再钉一条"数量下限"：如果哪天 $crossLanguageGaps 被误改成覆盖全部用例，
	// 上面的循环会一条不剩地走 gap 分支，测试**照样绿** —— 这正是本项目最怕的
	// "守卫静默失效"。所以这里数一下真正按 Rust 期望校验过的条数。
	if verified < 16 {
		t.Fatalf("只按 Rust 期望校验了 %d 条（少于 16 条）—— 检查 $crossLanguageGaps 是否吞掉了用例", verified)
	}
}

// assertVerifyOutcome 断言一次校验落在期望的那一态上。
//
// ⚠ 这里挂了 recover：`argon2.IDKey` 在 `t=0` / `p=0` 时**panic**，
// 而哈希串是可能被写坏的 —— 在 HTTP 服务里那是一次请求打崩整个进程。
// 向量里恰好有这几条（见 `$costBoundary`），所以这条断言顺带把"不许 panic"也钉住了。
func assertVerifyOutcome(t *testing.T, plain, encoded, want, detail string) {
	t.Helper()
	defer func() {
		if r := recover(); r != nil {
			t.Fatalf("校验 %q 时 panic 了（哈希串可能被写坏，不允许 panic）: %v", encoded, r)
		}
	}()

	ok, err := VerifyPassword(plain, PasswordHash(encoded))
	switch want {
	case "ok":
		if err != nil {
			t.Fatalf("期望匹配，得到错误: %v（%s）", err, detail)
		}
		if !ok {
			t.Fatalf("期望匹配，得到不匹配（%s）", detail)
		}
	case "mismatch":
		if err != nil {
			t.Fatalf("期望不匹配（正常业务结果），得到错误: %v（%s）", err, detail)
		}
		if ok {
			t.Fatalf("期望不匹配，却匹配上了（%s）", detail)
		}
	case "error":
		if err == nil {
			t.Fatalf("期望格式非法，却成功了（ok=%v，%s）", ok, detail)
		}
		if kind, isCore := KindOf(err); !isCore || kind != KindInternal {
			t.Fatalf("格式非法应归为内部错误，得到: %v", err)
		}
	default:
		t.Fatalf("未知的期望值 %q", want)
	}
}

// TestSaltLengthBoundaryVectors 扫盐长度下限。
//
// ⚠ 这条**不能靠推**：文档里常见的说法是"盐至少 4 字节"，而实测 3/4/6 全被拒、
// 8 才通过。推错的方向是"Go 放行一个 Rust 会拒的串"。
func TestSaltLengthBoundaryVectors(t *testing.T) {
	v := passwordHashVectors(t)
	cases := seg(t, v, "$saltLengthBoundary")
	if len(cases) == 0 {
		t.Fatal("$saltLengthBoundary 不能为空")
	}

	seenBelow, seenAbove := false, false
	for _, raw := range cases {
		c := obj(t, raw, "盐边界用例")
		n := numInt(t, c, "saltBytes")
		b64 := str(t, c, "saltB64")
		outcome := str(t, c, "outcome")

		// 串的形状与生成器里的完全一致（哈希段是一段固定的合法 base64，
		// 好让"报错"只可能来自盐）。
		encoded := fmt.Sprintf(
			"$argon2id$v=19$m=19456,t=2,p=1$%s$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", b64)

		if n < phcMinSaltBytes {
			seenBelow = true
			if !strings.HasPrefix(outcome, "error:") {
				t.Fatalf("盐 %d 字节低于下限 %d，应报错，实测 %s", n, phcMinSaltBytes, outcome)
			}
		} else {
			seenAbove = true
			if strings.HasPrefix(outcome, "error:") {
				t.Fatalf("盐 %d 字节达到下限 %d，不该报错，实测 %s", n, phcMinSaltBytes, outcome)
			}
		}

		want := "mismatch"
		if strings.HasPrefix(outcome, "error:") {
			want = "error"
		}
		assertVerifyOutcome(t, "whatever", encoded, want, outcome)
	}

	// ⚠ 确认向量**真的跨过了边界**：若哪天有人把边界用例裁到只剩一侧，
	// 上面的循环仍然会全绿，而下限就再也没有任何东西钉住了。
	if !seenBelow || !seenAbove {
		t.Fatalf("盐长度向量必须同时覆盖下限的两侧（低于下限的 error 与达到下限的非 error）")
	}
}

// TestCostBoundaryVectors 扫 m / t / p 三条下限。
//
// ⚠ `m` 那条最阴：低于下限时 Go 的 `argon2.IDKey` **不 panic，而是悄悄把 memory
// 抬到下限再算** —— 于是同一个串在 Go 侧报"口令不对"、在 Rust 侧报"格式非法"。
// 不扫出来就会以为"两边都是拒绝，差不多"，而对外一个是 401、一个是 500。
func TestCostBoundaryVectors(t *testing.T) {
	v := passwordHashVectors(t)
	cases := seg(t, v, "$costBoundary")
	if len(cases) == 0 {
		t.Fatal("$costBoundary 不能为空")
	}

	boundaries := map[string]int64{"m": phcMinMemoryKiB, "t": 1, "p": 1}
	below, above := map[string]bool{}, map[string]bool{}

	for _, raw := range cases {
		c := obj(t, raw, "$costBoundary 用例")
		name := str(t, c, "param")
		value := numInt(t, c, "value")
		outcome := str(t, c, "outcome")

		base := "$argon2id$v=19$m=19456,t=2,p=1$AQIDBAUGBwgJCgsMDQ4PEA$878U+0v6GO2t1pLRqMja0KD8VQdkhMB93Wh2uOGAhmc"
		var encoded string
		switch name {
		case "m":
			encoded = strings.Replace(base, "m=19456", fmt.Sprintf("m=%d", value), 1)
		case "t":
			encoded = strings.Replace(base, "t=2", fmt.Sprintf("t=%d", value), 1)
		case "p":
			encoded = strings.Replace(base, "p=1", fmt.Sprintf("p=%d", value), 1)
		default:
			t.Fatalf("未知的参数名 %q", name)
		}

		want := "mismatch"
		if strings.HasPrefix(outcome, "error:") {
			want = "error"
		}
		assertVerifyOutcome(t, "correct-horse", encoded, want, outcome)

		// 顺带把"下限在哪"钉住：低于下限必须 error，达到下限必须不是 error。
		if limit, ok := boundaries[name]; ok {
			if value < limit {
				below[name] = true
				if want != "error" {
					t.Fatalf("%s=%d 低于下限 %d，应报错", name, value, limit)
				}
			} else {
				above[name] = true
				if want == "error" {
					t.Fatalf("%s=%d 达到下限 %d，不该报错", name, value, limit)
				}
			}
		}
	}

	for name := range boundaries {
		if !below[name] || !above[name] {
			t.Fatalf("成本参数 %q 的向量必须同时覆盖下限的两侧", name)
		}
	}
}
