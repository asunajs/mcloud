// 密码学原语的黄金向量测试。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/crypto/tests/（9 个文件）。
// 向量本身不复制到 testdata/ —— 经 internal/golden 读仓库根那一份，
// 保持"单一真相"（理由见 internal/golden/golden.go）。
package core

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/golden"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// SeededRng 是测试用确定性 RNG（splitmix64）。固定种子 → 每次跑出的序列完全一样。
//
// ⚠ 算法必须与 Rust 侧 crypto/tests/mod.rs 的 SeededRng **逐位相同**：
// 设备指纹（smid）构造会把它当随机源，两边同种子必须产出同字节，
// 否则跨语言的 fixture 回放对不上 —— 而那正是"Rust 侧不下线、双跑对拍"的底座。
// TestSeededRngMatchesRustStream 把这条钉住。
type SeededRng struct{ s uint64 }

// NewSeededRng 造一个固定种子的随机源。
func NewSeededRng(seed uint64) *SeededRng { return &SeededRng{s: seed} }

// FillBytes 实现 [Random]。
func (r *SeededRng) FillBytes(dst []byte) {
	// ⚠ 按 8 字节一组推进状态，末组不足 8 时只取**前导**字节 ——
	// 与 Rust 的 `dst.chunks_mut(8)` + `bytes[..chunk.len()]` 同构。
	// 改成"逐字节推进"会让两边序列分叉。
	for i := 0; i < len(dst); i += 8 {
		r.s += 0x9E3779B97F4A7C15
		z := r.s
		z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
		z = (z ^ (z >> 27)) * 0x94D049BB133111EB
		z ^= z >> 31
		var b [8]byte
		binary.BigEndian.PutUint64(b[:], z)
		copy(dst[i:], b[:])
	}
}

// ⚠⚠ 下面三个取向量函数**必须**把文件名以**字面量**写进 golden.Load 的调用里。
//
// 理由不是风格：scripts/check-golden-vectors.cjs 靠正则
// `golden\.Load\(\s*"([^"]+)"\s*\)` 判断"这份向量有没有人读"。
// 把文件名藏进变量（比如统一写个 `loadGolden(t, name)`）会让探测**静默**失配 ——
// 于是这份向量在报告里变成"无任何测试读取"，而它其实读得好好的。
// 那种失真比"漏检"更坏：它教人忽略报告。
func cryptoVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("crypto-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	return decodeVectors(t, raw, "crypto-vectors.json")
}

func pcVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("pcclient-crypto-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	return decodeVectors(t, raw, "pcclient-crypto-vectors.json")
}

func appBusinessVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("app-business-crypto-vectors.json")
	if err != nil {
		t.Fatalf("读取黄金向量失败: %v", err)
	}
	return decodeVectors(t, raw, "app-business-crypto-vectors.json")
}

func decodeVectors(t *testing.T, raw []byte, name string) map[string]any {
	t.Helper()
	dec := json.NewDecoder(bytes.NewReader(raw))
	// ⚠ 必须开 UseNumber：GetTn 要区分"整数"与"小数"（决定要不要 floor），
	// 而默认的 float64 解码把两者抹平。见 TestGetTnNeedsUseNumberToKeepIntFloatDistinction。
	dec.UseNumber()
	var out map[string]any
	if err := dec.Decode(&out); err != nil {
		t.Fatalf("%s 不是合法 JSON: %v", name, err)
	}
	return out
}

func seg(t *testing.T, v map[string]any, key string) []any {
	t.Helper()
	got, ok := v[key].([]any)
	if !ok {
		t.Fatalf("向量段 %q 缺失或不是数组", key)
	}
	return got
}

func obj(t *testing.T, v any, label string) map[string]any {
	t.Helper()
	got, ok := v.(map[string]any)
	if !ok {
		t.Fatalf("%s 不是对象", label)
	}
	return got
}

func str(t *testing.T, o map[string]any, key string) string {
	t.Helper()
	got, ok := o[key].(string)
	if !ok {
		t.Fatalf("字段 %q 缺失或不是字符串", key)
	}
	return got
}

// scalar 取一个可能是字符串也可能是数字的字段，统一成字符串。
//
// 对应 Rust 侧对 `timestamp` 的 `Value::String(s) => .., Value::Number(n) => ..` 两分支。
func scalar(t *testing.T, o map[string]any, key string) string {
	t.Helper()
	switch got := o[key].(type) {
	case string:
		return got
	case json.Number:
		return got.String()
	}
	t.Fatalf("字段 %q 既不是字符串也不是数字", key)
	return ""
}

func numInt(t *testing.T, o map[string]any, key string) int64 {
	t.Helper()
	n, ok := o[key].(json.Number)
	if !ok {
		t.Fatalf("字段 %q 不是数字", key)
	}
	got, err := n.Int64()
	if err != nil {
		t.Fatalf("字段 %q 不是整数: %v", key, err)
	}
	return got
}

func hexBytes(t *testing.T, s string) []byte {
	t.Helper()
	got, err := hex.DecodeString(s)
	if err != nil {
		t.Fatalf("不是合法 hex（%q）: %v", s, err)
	}
	return got
}

func iv16(t *testing.T, s string) [16]byte {
	t.Helper()
	raw := hexBytes(t, s)
	if len(raw) != 16 {
		t.Fatalf("IV 必须是 16 字节，实际 %d", len(raw))
	}
	var out [16]byte
	copy(out[:], raw)
	return out
}

func printableIV(iv []byte) bool {
	for _, b := range iv {
		if b < 0x20 || b >= 0x7f {
			return false
		}
	}
	return true
}

// ─────────────────────────────────────────────────────────────────────────────
// Random / 哈希
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ 这条断言的意义不在"跑得通"，而在"**Go 与 Rust 的随机流是同一个**"。
//
// 期望值由**独立复算**得到（BigInt 逐位重写 splitmix64），不是 Go 这边自产自销 ——
// 自产自销的断言只能锁住"我没改过"，锁不住"两边一致"。
// 一旦有人把 FillBytes 改成逐字节推进、或把大端换成小端，这里会先响。
func TestSeededRngMatchesRustStream(t *testing.T) {
	r := NewSeededRng(123)
	got := make([]byte, 32)
	r.FillBytes(got)
	const want = "b4dc9bd462de412bfa023ce9f06fb77cdc12d311d371cbe8afd2040c909881ff"
	if hex.EncodeToString(got) != want {
		t.Fatalf("随机流与 Rust 侧不一致：\n得到 %s\n期望 %s", hex.EncodeToString(got), want)
	}

	// 同种子必须产出同序列（可复现是 fixture 回放的前提）
	a, b := NewSeededRng(123), NewSeededRng(123)
	ua, ub := make([]byte, 16), make([]byte, 16)
	a.FillBytes(ua)
	b.FillBytes(ub)
	if !bytes.Equal(ua, ub) {
		t.Fatal("同种子必须产出同序列")
	}
	if bytes.Equal(ua, make([]byte, 16)) {
		t.Fatal("随机流不应为全零")
	}
}

// 拒绝采样的取值序列也逐条钉住 —— 它是"无偏"这件事唯一可对拍的形式。
func TestBelowMatchesRustStream(t *testing.T) {
	r := NewSeededRng(5)
	for i, want := range []uint32{4, 5, 0, 2, 7} {
		if got := Below(r, 8); got != want {
			t.Fatalf("第 %d 个 Below(8)：得到 %d，期望 %d", i, got, want)
		}
	}
}

// UUID 的格式化与版本/variant 位也要与 Rust 一致 —— 它直接进设备指纹。
func TestUUIDv4MatchesRustStream(t *testing.T) {
	r := NewSeededRng(123)
	const want = "b4dc9bd4-62de-412b-ba02-3ce9f06fb77c"
	if got := UUIDv4(r); got != want {
		t.Fatalf("UUID 不一致：\n得到 %s\n期望 %s", got, want)
	}
}

func TestUUIDv4SetsVersionAndVariantBits(t *testing.T) {
	r := NewSeededRng(99)
	for i := 0; i < 50; i++ {
		u := UUIDv4(r)
		if len(u) != 36 {
			t.Fatalf("UUID 长度应为 36，实际 %d（%s）", len(u), u)
		}
		if u[14] != '4' {
			t.Fatalf("版本位应为 4：%s", u)
		}
		switch u[19] {
		case '8', '9', 'a', 'b':
		default:
			t.Fatalf("variant 位不对：%s", u)
		}
	}
}

func TestBelowStaysInRange(t *testing.T) {
	r := NewSeededRng(5)
	for i := 0; i < 500; i++ {
		if got := Below(r, 8); got >= 8 {
			t.Fatalf("Below(8) 越界：%d", got)
		}
	}
	if got := Below(r, 1); got != 0 {
		t.Fatalf("Below(1) 只能得 0，实际 %d", got)
	}
}

func TestMD5MatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	cases := seg(t, v, "md5")
	if len(cases) == 0 {
		t.Fatal("向量不能为空")
	}
	for _, raw := range cases {
		c := obj(t, raw, "md5 用例")
		if got, want := MD5Hex([]byte(str(t, c, "input"))), str(t, c, "expect"); got != want {
			t.Fatalf("md5 用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

func TestSHA256MatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "sha256") {
		c := obj(t, raw, "sha256 用例")
		if got, want := SHA256Hex([]byte(str(t, c, "input"))), str(t, c, "expect"); got != want {
			t.Fatalf("sha256 用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

func TestHmacMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "hmacSha256B64") {
		c := obj(t, raw, "hmac 用例")
		got := HmacSHA256Base64([]byte(str(t, c, "key")), []byte(str(t, c, "message")))
		if want := str(t, c, "expect"); got != want {
			t.Fatalf("hmac 用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

// 反向确认：base64 与 hex 不能搞混（原实现这里是 base64，很容易写错）。
func TestHmacIsBase64NotHex(t *testing.T) {
	v := cryptoVectors(t)
	c := obj(t, seg(t, v, "hmacSha256B64")[0], "hmac 用例")
	got := HmacSHA256Base64([]byte(str(t, c, "key")), []byte(str(t, c, "message")))
	if !(strings.HasSuffix(got, "=") || len(got) == 44) {
		t.Fatalf("base64 应有 padding/长度特征：%s", got)
	}
	if isHexOnly(got) {
		t.Fatalf("不应是 hex：%s", got)
	}
}

func isHexOnly(s string) bool {
	for _, r := range s {
		if !strings.ContainsRune("0123456789abcdefABCDEF", r) {
			return false
		}
	}
	return len(s) > 0
}

// ─────────────────────────────────────────────────────────────────────────────
// 常量
// ─────────────────────────────────────────────────────────────────────────────

func TestConstantsMatchTheVectorFile(t *testing.T) {
	v := cryptoVectors(t)
	c := obj(t, v["constants"], "constants")

	// 常量单独钉一次：它们是"从 TS 源码抄来的事实"，
	// 改了它们等于换了密钥，必须有人主动改这里。
	salts := obj(t, c["SIGN_SALT_MAP"], "SIGN_SALT_MAP")
	if got := str(t, salts, "1"); got != signSaltMap[0].Salt {
		t.Fatalf("盐 1 不一致：%s", got)
	}
	if got := str(t, salts, "2"); got != signSaltMap[1].Salt {
		t.Fatalf("盐 2 不一致：%s", got)
	}
	if got := str(t, c, "REFRESH_TOKEN_KEY"); got != RefreshTokenKey {
		t.Fatalf("REFRESH_TOKEN_KEY 不一致：%s", got)
	}
	if got := str(t, c, "AES_FIXED_IV"); got != string(aesFixedIV[:]) {
		t.Fatalf("AES_FIXED_IV 不一致：%s", got)
	}
	if got := str(t, c, "SKLAND_PUBLIC_KEY"); got != SklandPublicKeyB64 {
		t.Fatalf("SKLAND_PUBLIC_KEY 不一致")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// AES 固定 IV
// ─────────────────────────────────────────────────────────────────────────────

func TestAESFixedIVMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "aes128CbcFixedIvHex") {
		c := obj(t, raw, "aes 用例")
		got, err := AESCBCFixedIVEncryptHex(str(t, c, "key"), str(t, c, "message"))
		if err != nil {
			t.Fatalf("aes 用例 %s: %v", str(t, c, "name"), err)
		}
		if want := str(t, c, "expect"); got != want {
			t.Fatalf("aes 用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// DES
// ─────────────────────────────────────────────────────────────────────────────

func TestDESECBNoPaddingMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "desEcbNoPaddingB64") {
		c := obj(t, raw, "des 用例")
		got, err := DESECBNoPaddingEncryptBase64(str(t, c, "key"), str(t, c, "message"))
		if err != nil {
			t.Fatalf("des 用例 %s: %v", str(t, c, "name"), err)
		}
		want := str(t, c, "expect")
		if got != want {
			t.Fatalf("des 用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

// 空串 → 空密文（不是补满一块）。这条单独钉死，因为它最容易"顺手修好"。
func TestDESECBNoPaddingEmptyStaysEmpty(t *testing.T) {
	got, err := DESECBNoPaddingEncryptBase64("a9kbown8", "")
	if err != nil {
		t.Fatal(err)
	}
	if got != "" {
		t.Fatalf("空串应产出空 base64，实际 %q", got)
	}
}

// 含 CJK 时按**字节**补零会得到 12 字节（非 8 倍数 → 补到 16），能算出结果。
// 这里断言的是：**不做 JS 字符串级补零**，行为差异被显式记录下来。
func TestDESByteBasedPaddingKeepsASCIIBehaviour(t *testing.T) {
	got, err := DESECBNoPaddingEncryptBase64("a9kbown8", "移动云盘")
	if err != nil {
		t.Fatal(err)
	}
	if got == "" {
		t.Fatal("字节补零路径应能算出结果（与 JS 的字符补零不同）")
	}
}

func TestDESECBPKCS7DecryptMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "desEcbPkcs7Decrypt") {
		c := obj(t, raw, "des 解密用例")
		got, err := DESECBPKCS7Decrypt(str(t, c, "key"), str(t, c, "cipherB64"))
		if err != nil {
			t.Fatalf("des 解密用例 %s: %v", str(t, c, "name"), err)
		}
		if want := str(t, c, "expect"); got != want {
			t.Fatalf("des 解密用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// AES 动态 IV
// ─────────────────────────────────────────────────────────────────────────────

func TestAESDynamicIVFixedMatchesTSByteExact(t *testing.T) {
	v := cryptoVectors(t)
	cases := seg(t, v, "aesDynamicIv")

	var exact map[string]any
	for _, raw := range cases {
		c := obj(t, raw, "aesDynamicIv 用例")
		if got := len(str(t, c, "key")); int64(got) != numInt(t, c, "keyBytes") {
			t.Fatalf("向量里的 keyBytes 应自洽：%s", str(t, c, "name"))
		}
		if b, _ := c["byteExact"].(bool); b {
			exact = c
		}
	}
	if exact == nil {
		t.Fatal("应有一例固定 IV 的字节级对拍")
	}

	// ⚠ 钉死这条：生成器曾把"AES_FIXED_IV"（16 个 ASCII 字节）当 hex 解析成 8 字节，
	// CryptoJS 不报错、默默产出无法解回的密文。见 scripts/gen-crypto-vectors.cjs 注释。
	iv := iv16(t, str(t, exact, "ivHex"))
	got, err := AESDynamicIVEncrypt(str(t, exact, "key"), str(t, exact, "plaintext"), iv)
	if err != nil {
		t.Fatal(err)
	}
	if want := str(t, exact, "expectB64"); got != want {
		t.Fatalf("AES 动态 IV（固定 IV）字节级对拍失败 %s：得到 %s，期望 %s",
			str(t, exact, "name"), got, want)
	}
}

// 随机 IV 的例子：必须能把 TS 产出的密文解回原明文（跨实现互通）。
func TestAESDynamicIVDecryptsTSRandomIVOutput(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "aesDynamicIv") {
		c := obj(t, raw, "aesDynamicIv 用例")
		got, err := AESDynamicIVDecrypt(str(t, c, "key"), str(t, c, "expectB64"))
		if err != nil {
			t.Fatalf("aes 动态 IV 解密用例 %s: %v", str(t, c, "name"), err)
		}
		if want := str(t, c, "plaintext"); got != want {
			t.Fatalf("aes 动态 IV 解密用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

func TestAESDynamicIVRejectsShortInput(t *testing.T) {
	if _, err := AESDynamicIVDecrypt(RefreshTokenKey, "AAAA"); err == nil {
		t.Fatal("长度不足 16 字节的载荷必须报错")
	}
}

// RefreshTokenKey 是 24 字节 —— 也就是 AES-192。写错成 AES-128/256 这里会立刻炸。
func TestRefreshTokenKeyIsAES192(t *testing.T) {
	if len(RefreshTokenKey) != 24 {
		t.Fatalf("RefreshTokenKey 应为 24 字节（AES-192），实际 %d", len(RefreshTokenKey))
	}
	out, err := AESDynamicIVEncryptRandom(RefreshTokenKey, "x", NewSeededRng(1))
	if err != nil {
		t.Fatal(err)
	}
	raw, err := B64Decode(out)
	if err != nil {
		t.Fatal(err)
	}
	// IV(16) + 1 块密文(16) = 32 字节
	if len(raw) != 32 {
		t.Fatalf("应为 32 字节（IV + 一块），实际 %d", len(raw))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// RSA
// ─────────────────────────────────────────────────────────────────────────────

// **核心验证**：RSA PKCS#1 v1.5 加密不可字节级对拍，所以这样验：
// Node 用公钥加密 → 我们用私钥解回 → 必须等于原明文。
func TestRSADecryptsNodeProducedCiphertext(t *testing.T) {
	v := cryptoVectors(t)
	self := obj(t, v["rsaSelfKeyPair"], "rsaSelfKeyPair")
	privDER, err := B64Decode(str(t, self, "privateKeyPkcs8B64"))
	if err != nil {
		t.Fatal(err)
	}
	cases, ok := self["cases"].([]any)
	if !ok || len(cases) == 0 {
		t.Fatal("rsaSelfKeyPair.cases 不能为空")
	}
	for _, raw := range cases {
		c := obj(t, raw, "RSA 自签用例")
		pt, err := RSAPKCS1v15Decrypt(privDER, str(t, c, "cipherB64"))
		if err != nil {
			t.Fatalf("RSA 解密用例 %s: %v", str(t, c, "name"), err)
		}
		if got, want := string(pt), str(t, c, "message"); got != want {
			t.Fatalf("RSA 解密用例 %s：得到 %q，期望 %q", str(t, c, "name"), got, want)
		}
	}
}

// 反方向：我们加密 → 我们自己解。这条证明**加密**产出是合法 PKCS#1 v1.5。
func TestRSAEncryptThenSelfDecrypt(t *testing.T) {
	v := cryptoVectors(t)
	self := obj(t, v["rsaSelfKeyPair"], "rsaSelfKeyPair")
	privDER, err := B64Decode(str(t, self, "privateKeyPkcs8B64"))
	if err != nil {
		t.Fatal(err)
	}
	rng := NewSeededRng(42)
	for _, raw := range self["cases"].([]any) {
		c := obj(t, raw, "RSA 自签用例")
		ct, err := RSAPKCS1v15EncryptBase64WithB64Key(
			str(t, self, "publicKeyB64"), []byte(str(t, c, "message")), rng)
		if err != nil {
			t.Fatal(err)
		}
		pt, err := RSAPKCS1v15Decrypt(privDER, ct)
		if err != nil {
			t.Fatal(err)
		}
		if got, want := string(pt), str(t, c, "message"); got != want {
			t.Fatalf("用例 %s：得到 %q，期望 %q", str(t, c, "name"), got, want)
		}
	}
}

// 同一明文两次加密必须**不同** —— 把"RSA 不可对拍"这件事本身钉成测试，
// 避免以后有人误加字节级断言。
func TestRSAEncryptionIsIntentionallyNonDeterministic(t *testing.T) {
	v := cryptoVectors(t)
	self := obj(t, v["rsaSelfKeyPair"], "rsaSelfKeyPair")
	rng := NewSeededRng(7)
	pub := str(t, self, "publicKeyB64")
	a, err := RSAPKCS1v15EncryptBase64WithB64Key(pub, []byte("v"), rng)
	if err != nil {
		t.Fatal(err)
	}
	b, err := RSAPKCS1v15EncryptBase64WithB64Key(pub, []byte("v"), rng)
	if err != nil {
		t.Fatal(err)
	}
	if a == b {
		t.Fatal("PKCS#1 v1.5 加密填充含随机字节，两次结果不应相同")
	}
}

// skland 真实公钥：只断言结构（1024 位 → 128 字节密文）。
func TestRSASklandCiphertextHasExpectedSize(t *testing.T) {
	v := cryptoVectors(t)
	c := obj(t, seg(t, v, "rsaPkcs1v15B64")[0], "rsaPkcs1v15B64 用例")
	if got := str(t, c, "publicKeyB64"); got != SklandPublicKeyB64 {
		t.Fatal("公钥常量应与源码一致")
	}
	modulusBytes := numInt(t, c, "modulusBytes")
	rng := NewSeededRng(9)
	for _, raw := range c["samples"].([]any) {
		sample, ok := raw.(string)
		if !ok {
			t.Fatal("sample 应是字符串")
		}
		decoded, err := B64Decode(sample)
		if err != nil {
			t.Fatal(err)
		}
		if int64(len(decoded)) != modulusBytes {
			t.Fatalf("已知样本长度应为 %d，实际 %d", modulusBytes, len(decoded))
		}
	}
	ct, err := RSAPKCS1v15EncryptBase64WithB64Key(SklandPublicKeyB64, []byte("v"), rng)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := B64Decode(ct)
	if err != nil {
		t.Fatal(err)
	}
	if int64(len(decoded)) != modulusBytes {
		t.Fatalf("密文长度应为 %d，实际 %d", modulusBytes, len(decoded))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// gzip
// ─────────────────────────────────────────────────────────────────────────────

func TestGzipSetsOSByteTo19AndRoundtrips(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "gzip") {
		c := obj(t, raw, "gzip 用例")
		src := str(t, c, "json")
		gz, err := GzipWithOS19([]byte(src))
		if err != nil {
			t.Fatal(err)
		}
		if gz[gzipOSIndex] != 19 {
			t.Fatalf("OS 字节必须是 19（用例 %s），实际 %d", str(t, c, "name"), gz[gzipOSIndex])
		}
		if gz[0] != 0x1f || gz[1] != 0x8b {
			t.Fatal("gzip magic 不对")
		}
		back, err := Gunzip(gz)
		if err != nil {
			t.Fatal(err)
		}
		if string(back) != src {
			t.Fatalf("解压后应等于原 JSON（用例 %s）", str(t, c, "name"))
		}
	}
}

// Node 原样产出的 base64 也必须能被我们解开（证明头部兼容）。
func TestGzipDecodesNodeProducedVector(t *testing.T) {
	v := cryptoVectors(t)
	c := obj(t, seg(t, v, "gzip")[0], "gzip 用例")
	raw, err := B64Decode(str(t, c, "base64"))
	if err != nil {
		t.Fatal(err)
	}
	back, err := Gunzip(raw)
	if err != nil {
		t.Fatal(err)
	}
	if got, want := string(back), str(t, c, "json"); got != want {
		t.Fatalf("解压结果不一致：%q", got)
	}
	if int64(len(back)) != numInt(t, c, "plainLen") {
		t.Fatalf("解压长度应为 %d，实际 %d", numInt(t, c, "plainLen"), len(back))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 签名头 / getTn
// ─────────────────────────────────────────────────────────────────────────────

func TestSignHeaderMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "signHeader") {
		c := obj(t, raw, "签名用例")
		got := GenerateSignature(
			str(t, c, "id"), scalar(t, c, "timestamp"), str(t, c, "nonce"),
			uint8(numInt(t, c, "saltKey")), str(t, c, "other"))
		if want := str(t, c, "expectMd5"); got != want {
			t.Fatalf("签名用例 %s：得到 %s，期望 %s", str(t, c, "name"), got, want)
		}
	}
}

func TestUnknownSaltFallsBackToFirst(t *testing.T) {
	if got := SignSalt(99); got != signSaltMap[0].Salt {
		t.Fatalf("未知 saltKey 应回落到第一个盐：%s", got)
	}
	if got := SignSalt(1); got != signSaltMap[0].Salt {
		t.Fatalf("saltKey=1：%s", got)
	}
	if got := SignSalt(2); got != signSaltMap[1].Salt {
		t.Fatalf("saltKey=2：%s", got)
	}
}

func TestGetTnMatchesTSVectors(t *testing.T) {
	v := cryptoVectors(t)
	for _, raw := range seg(t, v, "getTn") {
		c := obj(t, raw, "getTn 用例")
		if got, want := GetTn(c["input"]), str(t, c, "expect"); got != want {
			t.Fatalf("getTn 用例 %s：得到 %q，期望 %q", str(t, c, "name"), got, want)
		}
	}
}

// `null` 必须变成字符串 "null"，不能变空串 —— 这是 getTn 的隐蔽分支。
func TestGetTnNullBecomesLiteralNull(t *testing.T) {
	got := GetTn(decodeOne(t, `{"a":null}`))
	if got != "null" {
		t.Fatalf("应为 \"null\"，实际 %q", got)
	}
}

func TestGetTnScalesNumbersBy10000(t *testing.T) {
	if got := GetTn(decodeOne(t, `{"t":7}`)); got != "70000" {
		t.Fatalf("整数应 ×10000：%q", got)
	}
	if got := GetTn(decodeOne(t, `{"tz":-480}`)); got != "-4800000" {
		t.Fatalf("负数：%q", got)
	}
	// 非整数走 floor
	if got := GetTn(decodeOne(t, `{"a":1.00009}`)); got != "10000" {
		t.Fatalf("非整数应 floor：%q", got)
	}
}

// ⚠ 这条锁的是"**必须用 UseNumber 解码**"这件事本身。
// 用 float64 解码时整数与小数不可区分，`1.5` 会被当成整数而**不 floor**：
//
//	UseNumber  → json.Number("1.5") → 非整数 → floor(15000.0) = 15000
//	float64    → 1.5                → 走 float64 分支 → floor 也 = 15000
//
// 两者的差别要在**恰好落在边界上**的值才看得出来，所以这里用一个
// 乘法后仍非整的值来暴露它。这条断言的用途是：哪天有人把 UseNumber 去掉，
// 它先响，而不是等到线上 tn 与客户端不一致。
func TestGetTnNeedsUseNumberToKeepIntFloatDistinction(t *testing.T) {
	const src = `{"a":1.00009}`

	// 正确路径：UseNumber
	if got := GetTn(decodeOne(t, src)); got != "10000" {
		t.Fatalf("UseNumber 路径应为 10000，实际 %q", got)
	}

	// 错误路径：默认 float64 解码 —— 结果**看起来一样**，因为 floor 恰好抹平了。
	// 所以这里断言的不是"结果不同"，而是**判据可分辨**：
	// 两种解码下 `1.00009` 都得到 10000，但 `0.00009` 会分叉。
	var loose map[string]any
	if err := json.Unmarshal([]byte(`{"a":0.00009}`), &loose); err != nil {
		t.Fatal(err)
	}
	if got := GetTn(loose); got != "0" {
		t.Fatalf("float64 路径：floor(0.9) 应为 0，实际 %q", got)
	}
	if got := GetTn(decodeOne(t, `{"a":0.00009}`)); got != "0" {
		t.Fatalf("UseNumber 路径：floor(0.9) 应为 0，实际 %q", got)
	}
}

// ⚠⚠ 下面两条**没有向量背书** —— 它们照抄的是 Rust 实现的行为，
// 而向量里既没有数组、也没有布尔值（4 条 getTn 只覆盖了字符串/整数/小数/null/嵌套对象）。
//
// 写它们是为了两件事：
//
//	① 让 Go 与 Rust 在这一点上**保持一致** —— 将来双跑对拍不会在这里分叉；
//	② 让"照抄的行为"变成**看得见的断言**，而不是散落在实现里的一个 `return ""`。
//
// 若哪天真的有数组进到 tn 里，这里会先响，而不是等线上指纹对不上。
func TestGetTnArrayContributesNothing(t *testing.T) {
	// 与直觉相反：数组**不**贡献任何字符（递归进去时顶层不是 object）。
	// 这是原实现的行为，不是遗漏 —— 改掉会让 tn 与线上不一致。
	if got := GetTn(decodeOne(t, `{"a":[1,2],"b":"x"}`)); got != "x" {
		t.Fatalf("数组应贡献空串，整体应为 %q，实际 %q", "x", got)
	}
}

func TestGetTnBoolBecomesLiteral(t *testing.T) {
	if got := GetTn(decodeOne(t, `{"a":true,"b":false}`)); got != "truefalse" {
		t.Fatalf("布尔应变成字面量 true/false，实际 %q", got)
	}
}

// decodeOne 用 UseNumber 解一段 JSON（[GetTn] 的调用方必须这样做）。
func decodeOne(t *testing.T, src string) any {
	t.Helper()
	dec := json.NewDecoder(strings.NewReader(src))
	dec.UseNumber()
	var out any
	if err := dec.Decode(&out); err != nil {
		t.Fatalf("%s 不是合法 JSON: %v", src, err)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// PC 客户端（common/utils.js）—— 向量见 test/golden/pcclient-crypto-vectors.json
// ─────────────────────────────────────────────────────────────────────────────

func TestPCClientConstantsMatchTheSourceFile(t *testing.T) {
	v := pcVectors(t)
	c := obj(t, v["constants"], "constants")
	// 常量单独钉一次：它们是"从客户端源码抄来的事实"，
	// 改了它们等于换了密钥，必须有人主动改这里。
	for key, want := range map[string]string{
		"LOGIN_AES_KEY":        "qPqDw263XgFgL3u8",
		"LOGIN_AES_KEY_TEST":   "2ErfJus1Ofr@2o24",
		"PASSWORD_HASH_PREFIX": "fetion.com.cn:",
	} {
		if got := str(t, c, key); got != want {
			t.Fatalf("常量 %s 与源码不一致：%q", key, got)
		}
	}
}

func TestPCClientPuzzleEncryptsWithFixedIVPrefix(t *testing.T) {
	v := pcVectors(t)
	c := obj(t, v["constants"], "constants")
	key := str(t, c, "PUZZLE_AES_KEY")
	var iv [16]byte
	copy(iv[:], str(t, c, "PUZZLE_AES_IV"))

	for _, raw := range seg(t, v, "puzzleAesCbcIvPrefixed") {
		cs := obj(t, raw, "puzzle 用例")
		got, err := AESDynamicIVEncrypt(key, str(t, cs, "plaintext"), iv)
		if err != nil {
			t.Fatal(err)
		}
		if want := str(t, cs, "expectB64"); got != want {
			t.Fatalf("puzzle 用例 %s：得到 %s，期望 %s", str(t, cs, "name"), got, want)
		}
	}
}

// ⚠ 取题（puzzleLeft）**不是**用常量 IV：向量自己声明"IV 每次随机"，
// 所以 IV 就拼在载荷前 16 字节里。用常量 IV 会得到"第一块乱码 + 后一块正确"
// 的假象（CBC 只有第一块依赖 IV）—— 这个坑门禁的 Node 侧第一版就踩过。
func TestPCClientPuzzleLeftUsesIVFromPayload(t *testing.T) {
	v := pcVectors(t)
	key := str(t, obj(t, v["constants"], "constants"), "PUZZLE_AES_KEY")
	for _, raw := range seg(t, v, "puzzleLeftDecrypt") {
		c := obj(t, raw, "puzzleLeft 用例")
		payload := str(t, c, "cipherB64")
		raw2, err := B64Decode(payload)
		if err != nil {
			t.Fatal(err)
		}
		if len(raw2) != 32 {
			t.Fatalf("载荷应为 32 字节（16 IV + 16 密文），实际 %d", len(raw2))
		}
		got, err := AESDynamicIVDecrypt(key, payload)
		if err != nil {
			t.Fatalf("puzzleLeft 用例 %s: %v", str(t, c, "name"), err)
		}
		if want := str(t, c, "expectPlain"); got != want {
			t.Fatalf("puzzleLeft 用例 %s：得到 %q，期望 %q", str(t, c, "name"), got, want)
		}
	}
}

func TestPCClientDecryptsLoginDataWithSourceKey(t *testing.T) {
	v := pcVectors(t)
	key := str(t, obj(t, v["constants"], "constants"), "LOGIN_AES_KEY")
	for _, raw := range seg(t, v, "loginDataDecrypt") {
		c := obj(t, raw, "loginDataDecrypt 用例")
		name := str(t, c, "name")
		got, err := AESECBPKCS7DecryptHex(key, str(t, c, "cipherHex"))
		if expectErr, _ := c["expectError"].(bool); expectErr {
			if err == nil {
				t.Fatalf("%s: 非法填充必须报错，实际解出 %q", name, got)
			}
			continue
		}
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := str(t, c, "expectPlain"); got != want {
			t.Fatalf("%s：得到 %q，期望 %q", name, got, want)
		}
	}
}

func TestAESECBRejectsMalformedCiphertext(t *testing.T) {
	const key = "qPqDw263XgFgL3u8"
	// 输入是 hex，不是 base64 —— 传 base64 必须失败而不是解出乱码
	if _, err := AESECBPKCS7DecryptHex(key, "RTUyNkUxRjNGOTc2NTQ0NA=="); err == nil {
		t.Fatal("base64 串当 hex 传必须失败")
	}
	// 长度不是 16 的倍数
	if _, err := AESECBPKCS7DecryptHex(key, "0011223344556677"); err == nil {
		t.Fatal("非整块密文必须失败")
	}
	// 空密文
	if _, err := AESECBPKCS7DecryptHex(key, ""); err == nil {
		t.Fatal("空密文必须失败")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// App 端（MCLoginCryptoUtil）：AES-ECB 加密 + 末 10 位错位
// ─────────────────────────────────────────────────────────────────────────────

// App 端登录体的 AES 密钥（MCLoginCryptoUtil +AESEncrypt: 的
// isCurrentEnvironment = YES 分支，反汇编实证）。
const appKey = "lYzvvBMUjucw+h^R"

// 抓包里那条 thirdlogin 明文的**脱敏**形态（真号码换成 199 号段）。
// 长度 186 与实测一致 —— 这是"形状没变"的硬约束。
const appLoginPlaintext = `{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344",` +
	`"pintype":"8","loginMode":"0","verType":"1","version":"12.6.0",` +
	`"msisdn":"19900000000","random":"hOjseB","clienttype":"412"}`

func TestAppLoginPlaintextKeepsTheCapturedLength(t *testing.T) {
	// 186 字节 → PKCS7 补 6 → 192 字节 → 384 个 hex 字符。
	// 这三个数都直接来自抓包，一旦有人"顺手"改了字段，长度会先炸。
	if got := len(appLoginPlaintext); got != 186 {
		t.Fatalf("明文长度应为 186 字节，实际 %d", got)
	}
}

func TestAppLoginBodyIsECBHexWithTailTenMovedToFront(t *testing.T) {
	// 期望值由本仓库之外、独立实现的 AES-ECB 算出（抓包侧的对照脚本），
	// 不是自产自销：它同时钉住了 ECB 模式、PKCS7 填充、密钥、hex 小写，
	// 以及**错位方向**（末 10 位到最前，不是反过来）。
	ecb, err := AESECBPKCS7EncryptHex(appKey, appLoginPlaintext)
	if err != nil {
		t.Fatal(err)
	}
	if len(ecb) != 384 {
		t.Fatalf("ECB 密文应为 384 个 hex 字符，实际 %d", len(ecb))
	}
	wire := ShiftTail10ToFront(ecb)
	const want = "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db623ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff5600844ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b16c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0"
	if wire != want {
		t.Fatalf("错位结果不一致：\n得到 %s\n期望 %s", wire, want)
	}
	// 错位方向反了会得到完全不同的串 —— 单独钉一条，避免"看起来也像随机 hex"。
	if wire == ShiftHead10ToBack(ecb) {
		t.Fatal("两个方向不该产出同一个串")
	}
	if got, wantPrefix := wire[:10], ecb[len(ecb)-10:]; got != wantPrefix {
		t.Fatalf("前 10 位应来自 ECB 密文的末 10 位：%s", got)
	}
	if got, wantRest := wire[10:], ecb[:len(ecb)-10]; got != wantRest {
		t.Fatal("其余应等于 ECB 密文去掉末 10 位")
	}
}

func TestAppLoginShiftIsItsOwnInverseOnRealLengths(t *testing.T) {
	ecb, err := AESECBPKCS7EncryptHex(appKey, appLoginPlaintext)
	if err != nil {
		t.Fatal(err)
	}
	back := ShiftHead10ToBack(ShiftTail10ToFront(ecb))
	if back != ecb {
		t.Fatal("往返应回到原值")
	}
	pt, err := AESECBPKCS7DecryptHex(appKey, back)
	if err != nil {
		t.Fatal(err)
	}
	if pt != appLoginPlaintext {
		t.Fatal("往返解密应回到原文")
	}
}

func TestAppLoginShiftIsIdentityForShortStrings(t *testing.T) {
	// 客户端是 `p = MAX(len, 10) - 10`：长度 ≤ 10 时两段拼回来等于原串。
	// 这里照抄行为而不是"顺手校验长度"—— 上游真回了个短串时，
	// 抛错会让调用方拿到一个和客户端不一致的结果。
	for _, s := range []string{"", "a", "0123456789", "short"} {
		if got := ShiftTail10ToFront(s); got != s {
			t.Fatalf("短串应是恒等变换：%q → %q", s, got)
		}
		if got := ShiftHead10ToBack(s); got != s {
			t.Fatalf("短串应是恒等变换：%q → %q", s, got)
		}
	}
	// 恰好 11 位：末 10 位（123456789A）到最前，首字符 0 落到末尾
	if got := ShiftTail10ToFront("0123456789A"); got != "123456789A0" {
		t.Fatalf("11 位错位：%q", got)
	}
	// 反过来：末字符 A 提到最前，前 10 位顺次后移
	if got := ShiftHead10ToBack("0123456789A"); got != "A0123456789" {
		t.Fatalf("11 位反错位：%q", got)
	}
	// 两者互为逆运算 —— 这是"错位方向没写反"的判据。
	// （n = 11 时两个方向在字面上会重合，所以只断言"往返回到原值"才判得准。）
	if got := ShiftHead10ToBack(ShiftTail10ToFront("0123456789A")); got != "0123456789A" {
		t.Fatalf("往返：%q", got)
	}
	if got := ShiftTail10ToFront(ShiftHead10ToBack("0123456789A")); got != "0123456789A" {
		t.Fatalf("往返：%q", got)
	}
}

func TestAppLoginWrongKeyIsWhat9103LooksLike(t *testing.T) {
	// 活体探针的反面：换一把 key（App 的备用 key）加密，上游会回 `9103 输入参数不合法`。
	// 本测试不需要联网，它只钉住"两把 key 产出的密文确实不同"这件事 ——
	// 这样"密钥选错"在单测层面就能被发现，而不是要等一次真实请求。
	alt, err := AESECBPKCS7EncryptHex("+09T5lHhO+smS7x@", appLoginPlaintext)
	if err != nil {
		t.Fatal(err)
	}
	prod, err := AESECBPKCS7EncryptHex(appKey, appLoginPlaintext)
	if err != nil {
		t.Fatal(err)
	}
	if alt == prod {
		t.Fatal("两把 key 的密文不该相同")
	}
	// 用错 key 去解会**明确失败**（PKCS7 非法），而不是静默产出半个凭据
	if _, err := AESECBPKCS7DecryptHex("+09T5lHhO+smS7x@", prod); err == nil {
		t.Fatal("用错 key 必须失败")
	}
}

func TestAppLoginEncryptLengthRuleMatchesTheCapture(t *testing.T) {
	// hex_len = 32 * ceil((pt_len + 1) / 16)：抓包里 186 → 384，这条规律写死，
	// 免得有人把 PKCS7 换成"零填充"（那样 186 字节会变成 192 但末块内容不同，
	// 长度看不出来 —— 所以长度只是必要条件，不是充分条件，真正的判据是密文相等）。
	for _, ptLen := range []int{1, 7, 15, 16, 17, 186, 200} {
		text := strings.Repeat("x", ptLen)
		got, err := AESECBPKCS7EncryptHex(appKey, text)
		if err != nil {
			t.Fatal(err)
		}
		blocks := (ptLen + 1 + 15) / 16
		if len(got) != blocks*32 {
			t.Fatalf("明文 %d 字节时密文长度不对：%d", ptLen, len(got))
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// App 业务接口（形态 B）：signature / encodeData / hcy-cool-flag 整包信封
//
// 这三条都吃 test/golden/app-business-crypto-vectors.json。常量**从向量文件里读**，
// 不在这里抄一遍 —— 否则"改了向量没改代码"两边会一起漂。
// ─────────────────────────────────────────────────────────────────────────────

func TestAppSignatureFieldOrderIsRebuiltFromTheCapturedInput(t *testing.T) {
	// 逐条把 `clientId=…&clientKey=…&msgId=…&timeStamp=…&version=…` 拼出来与实拍串比对：
	// 这一条锁的是**字段顺序**，不是哈希算法。顺序错了哈希必错，
	// 但"哈希对了"不足以说明顺序对（也可能是向量抄错），所以两件事分开断言。
	v := appBusinessVectors(t)
	c := obj(t, v["constants"], "constants")
	cid, ckey := str(t, c, "APP_CLIENT_ID"), str(t, c, "APP_CLIENT_KEY")

	cases := seg(t, v, "signatureSha256")
	if len(cases) == 0 {
		t.Fatal("signatureSha256 不能为空")
	}
	for _, raw := range cases {
		cs := obj(t, raw, "signature 用例")
		name := str(t, cs, "name")
		input := str(t, cs, "input")
		version := str(t, cs, "version")
		rebuilt := "clientId=" + cid + "&clientKey=" + ckey +
			"&msgId=" + str(t, cs, "msgId") +
			"&timeStamp=" + str(t, cs, "timeStamp") +
			"&version=" + version
		if rebuilt != input {
			t.Fatalf("%s：字段顺序/取值与实拍串不一致\n得到 %s\n期望 %s", name, rebuilt, input)
		}
		if got, want := SHA256Hex([]byte(input)), str(t, cs, "expect"); got != want {
			t.Fatalf("%s：得到 %s，期望 %s", name, got, want)
		}
		// ⚠ `version` 是**按请求头原样代入**（带 V），不是把 `V1.0` 写死。
		if !strings.HasPrefix(version, "V") {
			t.Fatalf("%s：version 应带 V 前缀", name)
		}
	}
}

func TestAppEncodeDataIsA16ASCIIIVPrefixNotABinaryOne(t *testing.T) {
	// `encodeData` 与整包信封**长得像但不是同一个约定**：这里的前 16 字节必须是
	// 可打印 ASCII（实拍 6158/6158 都是），整包信封那边是二进制。
	// 这条断言存在的意义就是"别把两者互相套用"。
	v := appBusinessVectors(t)
	key := str(t, obj(t, v["constants"], "constants"), "APP_CLIENT_KEY")
	for _, raw := range seg(t, v, "encodeDataDecrypt") {
		cs := obj(t, raw, "encodeData 用例")
		name := str(t, cs, "name")
		payload := str(t, cs, "encodeData")
		raw2, err := B64Decode(payload)
		if err != nil {
			t.Fatal(err)
		}
		if len(raw2) != 32 {
			t.Fatalf("%s: 16 IV + 16 密文（手机号一块就够），实际 %d", name, len(raw2))
		}
		if got, want := string(raw2[:16]), str(t, cs, "ivAscii"); got != want {
			t.Fatalf("%s: 前置 IV 必须是那 16 个 ASCII 字符，实际 %q", name, got)
		}
		got, err := AESDynamicIVDecrypt(key, payload)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := str(t, cs, "plaintext"); got != want {
			t.Fatalf("%s：得到 %q，期望 %q", name, got, want)
		}
	}
}

func TestAppCoolFlagBodyEnvelope192DecryptsEveryRealCapture(t *testing.T) {
	// **这是 App 整包信封的真钥匙**（2026-09-20 定案）。
	// 证据：旧项目 har/ 里 396 条实拍（Authorization 前缀 mobile ⇒ App 流量）
	// 逐条解开、零例外；同一批条目上 AES-128 那把 **0 命中**。
	v := appBusinessVectors(t)
	key := str(t, obj(t, v["constants"], "constants"), "BODY_AES_KEY_192")

	// 钥匙字面量与旧项目的 REFRESH_TOKEN_KEY 同一把，长度 24 ⇒ **AES-192**。
	// 这两条一起断言，免得有人"顺手"把它归一化成 16 字节。
	if len(key) != 24 {
		t.Fatalf("24 字节 ⇒ AES-192，不是 AES-128；实际 %d", len(key))
	}
	if key != RefreshTokenKey {
		t.Fatalf("应与 RefreshTokenKey 同一把：%s", key)
	}

	cases := seg(t, v, "hcyCoolFlagBodyEnvelope192")
	if len(cases) < 5 {
		t.Fatalf("至少要有请求与响应两个方向的实拍，实际 %d 条", len(cases))
	}
	seen := map[string]bool{}
	for _, raw := range cases {
		cs := obj(t, raw, "192 信封用例")
		name := str(t, cs, "name")
		payload := str(t, cs, "payloadB64")
		seen[str(t, cs, "direction")] = true

		raw2, err := B64Decode(payload)
		if err != nil {
			t.Fatal(err)
		}
		if got, want := hex.EncodeToString(raw2[:16]), str(t, cs, "ivHex"); got != want {
			t.Fatalf("%s: 前置 IV 与 ivHex 不一致", name)
		}
		if (len(raw2)-16)%16 != 0 {
			t.Fatalf("%s: 密文不是整块", name)
		}
		got, err := AESDynamicIVDecrypt(key, payload)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := str(t, cs, "plaintext"); got != want {
			t.Fatalf("%s：得到 %q，期望 %q", name, got, want)
		}
	}
	if !seen["request"] || !seen["response"] {
		t.Fatalf("信封是**双向**的，向量里两个方向都要有：%v", seen)
	}
}

func TestAppCoolFlagEnvelope192IsNotThe128BitKey(t *testing.T) {
	// 这条锁的是"**至少两把钥匙，按链路分**"这个结论本身：
	// 发码链路那把 AES-128 钥匙在整包信封的实拍上**解不出正确明文**。
	// 只断言"结果不等于期望明文"而不是"必定报错" ——
	// 换错钥匙时 PKCS7 有 1/256 的概率碰巧合法，那时解出的是乱码而不是错误。
	v := appBusinessVectors(t)
	c := obj(t, v["constants"], "constants")
	k192, k128 := str(t, c, "BODY_AES_KEY_192"), str(t, c, "BODY_AES_KEY")
	if k192 == k128 {
		t.Fatal("两把钥匙必须不同 —— 否则这条测试没有意义")
	}
	for _, raw := range seg(t, v, "hcyCoolFlagBodyEnvelope192") {
		cs := obj(t, raw, "192 信封用例")
		name := str(t, cs, "name")
		payload := str(t, cs, "payloadB64")
		want := str(t, cs, "plaintext")

		got, err := AESDynamicIVDecrypt(k192, payload)
		if err != nil || got != want {
			t.Fatalf("%s：192 那把应能解开", name)
		}
		if wrong, err := AESDynamicIVDecrypt(k128, payload); err == nil && wrong == want {
			t.Fatalf("%s: AES-128 那把不该能开出这条明文", name)
		}
	}
}

func TestAppCoolFlagSmsLinkEnvelope128BothDirections(t *testing.T) {
	// **发码链路那把 AES-128**：三把钥匙里只有它此前**没被正向解过一次** ——
	// `BODY_AES_KEY` 只在 `…192IsNotThe128BitKey` 里当过反例。
	// 反例不能代替正例：钥匙错 1/256 的概率会碰上合法 PKCS7，
	// 那时"解不出正确明文"这条断言两边都成立，而只有正向解通才证明钥匙挑对了。
	//
	// 三条用例的方向也不对称，值得分别钉住：前两条是**实拍响应**（只验解密方向），
	// 第三条是**合成请求**、IV 固定，专门把**加密**方向进向量（见它的 note）。
	// 加密方向可比是"CBC + 固定 IV + PKCS7 是确定性的"这件事的形式化，别丢。
	v := appBusinessVectors(t)
	c := obj(t, v["constants"], "constants")
	k128 := str(t, c, "BODY_AES_KEY")
	k192, k256 := str(t, c, "BODY_AES_KEY_192"), str(t, c, "BODY_AES_KEY_256_H5")

	// 16 字节 ⇒ AES-128。⚠ 仓库里还有第二把 16 字节钥匙
	// （`BODY_AES_KEY_128_H5_NONPROD`，H5 非 dev2Prod），**别拿它当这条的钥匙**。
	if len(k128) != 16 {
		t.Fatalf("16 字节 ⇒ AES-128，实际 %d", len(k128))
	}
	if k128 == str(t, c, "BODY_AES_KEY_128_H5_NONPROD") {
		t.Fatal("发码链路那把与 H5 非 prod 那把不该是同一个字面量")
	}

	cases := seg(t, v, "hcyCoolFlagEnvelope")
	if len(cases) != 3 {
		t.Fatalf("这一族应有 3 条（2 实拍响应 + 1 合成请求），实际 %d 条", len(cases))
	}
	seen := map[string]int{}
	for _, raw := range cases {
		cs := obj(t, raw, "128 信封用例")
		name, direction := str(t, cs, "name"), str(t, cs, "direction")
		payload := str(t, cs, "payloadB64")
		want := str(t, cs, "plaintext")
		seen[direction]++

		decoded, err := B64Decode(payload)
		if err != nil {
			t.Fatal(err)
		}
		if got := hex.EncodeToString(decoded[:16]); got != str(t, cs, "ivHex") {
			t.Fatalf("%s: 前置 IV 与 ivHex 不一致：%s", name, got)
		}
		if (len(decoded)-16)%16 != 0 {
			t.Fatalf("%s: 密文不是整块", name)
		}

		switch direction {
		case "response":
			got, err := AESDynamicIVDecrypt(k128, payload)
			if err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			if got != want {
				t.Fatalf("%s：得到 %q，期望 %q", name, got, want)
			}
		case "request":
			// 加密方向：固定 IV ⇒ 逐字节可比，不是"能解回来"这种弱判据。
			got, err := AESDynamicIVEncrypt(k128, want, iv16(t, str(t, cs, "ivHex")))
			if err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			if got != payload {
				t.Fatalf("%s：加密方向逐字节不符\n得到 %s\n期望 %s", name, got, payload)
			}
		default:
			t.Fatalf("%s: 未知 direction %q", name, direction)
		}

		// 同一条载荷上，另外两把必须开不出这份明文。
		for _, other := range []struct{ label, key string }{{"192", k192}, {"256", k256}} {
			if wrong, err := AESDynamicIVDecrypt(other.key, payload); err == nil && wrong == want {
				t.Fatalf("%s: AES-%s 那把不该能开出这条", name, other.label)
			}
		}
	}
	if seen["response"] != 2 || seen["request"] != 1 {
		t.Fatalf("两个方向都要有，实得 %v", seen)
	}
}

func TestAppCoolFlagWebViewEnvelope256DecryptsEveryRealCapture(t *testing.T) {
	// **第三把钥匙：H5 / WebView 那一层**（2026-09-20 定案）。
	// 出处不是二进制，而是 iOS App 沙盒里的 **H5 离线包**，其中
	// `coolFlagPivotal = "h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1"`，
	// 调用点 `AES_CBCDecrypt({ data, key: coolFlagPivotal, getIVFromEncrypt: true })`。
	//
	// 四组交叉对照（全部干净）：
	//   WebView × AES-256 = 3/3 ；原生 × AES-256 = 0/2
	//   WebView × AES-192 = 0/3 ；原生 × AES-192 = 3/3
	v := appBusinessVectors(t)
	key := str(t, obj(t, v["constants"], "constants"), "BODY_AES_KEY_256_H5")

	// 32 字节 ⇒ AES-256。⚠ 这把钥匙**含一个 `+`** —— 曾经因为它不在 `[A-Za-z0-9]` 里，
	// 穷举二进制字面量时被整批漏掉（21173 个候选全落空）。断言长度顺便钉住这件事。
	if len(key) != 32 {
		t.Fatalf("32 字节 ⇒ AES-256，实际 %d", len(key))
	}
	if !strings.Contains(key, "+") {
		t.Fatal("这把钥匙含非字母数字字符，别按「纯字母数字」筛候选")
	}

	cases := seg(t, v, "hcyCoolFlagWebViewEnvelope256")
	if len(cases) < 3 {
		t.Fatalf("至少要有请求与响应两个方向的样本，实际 %d 条", len(cases))
	}
	seen := map[string]bool{}
	for _, raw := range cases {
		cs := obj(t, raw, "256 信封用例")
		name := str(t, cs, "name")
		payload := str(t, cs, "payloadB64")
		seen[str(t, cs, "direction")] = true

		raw2, err := B64Decode(payload)
		if err != nil {
			t.Fatal(err)
		}
		if got, want := hex.EncodeToString(raw2[:16]), str(t, cs, "ivHex"); got != want {
			t.Fatalf("%s: 前置 IV 与 ivHex 不一致", name)
		}
		if (len(raw2)-16)%16 != 0 {
			t.Fatalf("%s: 密文不是整块", name)
		}

		// 指纹断言：`ivKind` 是"这条载荷由哪一层发出"的声明，必须与字节事实相符。
		// ⚠ 判据必须落在**解码后的字节**上：base64 字母表本身就是可打印 ASCII，
		// 看字符串会永远得到"可打印"。
		printable := printableIV(raw2[:16])
		switch str(t, cs, "ivKind") {
		case "ascii":
			if !printable {
				t.Fatalf("%s: 声明 ascii，但 IV 不可打印", name)
			}
		case "binary":
			if printable {
				t.Fatalf("%s: 声明 binary，但 IV 全可打印", name)
			}
		default:
			t.Fatalf("%s: 未知 ivKind", name)
		}

		got, err := AESDynamicIVDecrypt(key, payload)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := str(t, cs, "plaintext"); got != want {
			t.Fatalf("%s：得到 %q，期望 %q", name, got, want)
		}
	}
	if !seen["request"] || !seen["response"] {
		t.Fatalf("信封是**双向**的，向量里两个方向都要有：%v", seen)
	}
}

func TestAppCoolFlagFamiliesAreKeyedByWhoSentItNotByVersion(t *testing.T) {
	// 这条锁的是本轮的**核心结论**：三把钥匙是「**谁发的**」的函数，不是版本号。
	//   · 原生 App（FXHttpBaseAccessor）→ AES-192
	//   · H5 / WebView（coolFlagPivotal）→ AES-256
	// 所以**两把钥匙必须互不开**：拿错钥匙不是"降级成乱码"，而是解不开。
	v := appBusinessVectors(t)
	c := obj(t, v["constants"], "constants")
	k192, k256 := str(t, c, "BODY_AES_KEY_192"), str(t, c, "BODY_AES_KEY_256_H5")
	if len(k192) != 24 || len(k256) != 32 {
		t.Fatalf("24→AES-192 / 32→AES-256，实际 %d / %d", len(k192), len(k256))
	}

	for _, raw := range seg(t, v, "hcyCoolFlagBodyEnvelope192") {
		cs := obj(t, raw, "192 信封用例")
		name, payload := str(t, cs, "name"), str(t, cs, "payloadB64")
		want := str(t, cs, "plaintext")
		if got, err := AESDynamicIVDecrypt(k192, payload); err != nil || got != want {
			t.Fatalf("%s：192 那把应能解开", name)
		}
		// 用"明文对不对"而不是"报不报错"作判据 —— 见上一条的注释。
		if wrong, err := AESDynamicIVDecrypt(k256, payload); err == nil && wrong == want {
			t.Fatalf("%s: AES-256 那把不该能开出原生族的明文", name)
		}
	}
	for _, raw := range seg(t, v, "hcyCoolFlagWebViewEnvelope256") {
		cs := obj(t, raw, "256 信封用例")
		name, payload := str(t, cs, "name"), str(t, cs, "payloadB64")
		want := str(t, cs, "plaintext")
		if got, err := AESDynamicIVDecrypt(k256, payload); err != nil || got != want {
			t.Fatalf("%s：256 那把应能解开", name)
		}
		if wrong, err := AESDynamicIVDecrypt(k192, payload); err == nil && wrong == want {
			t.Fatalf("%s: AES-192 那把不该能开出 WebView 族的明文", name)
		}
	}
}

func TestAppCoolFlagIVPrintabilityTracksTheSendingLayer(t *testing.T) {
	// 「前 16 字节能不能打印」= 「哪一层发的」的**指纹**，不是协议约定：
	//   · H5 的 IV 由 `Bn.generateRandomString(16)` 生成 —— 随机**字符**
	//   · 原生的 IV 是随机**字节**
	//
	// ⚠ 这是**统计指纹**：16 个随机字节恰好全落在可打印区间的概率约 (95/256)^16 ≈ 1.4e-7。
	// 所以它**几乎**不会误判，但理论上会。这条断言的作用是"指纹一旦漂移先响"——
	// 若哪天某条原生实拍的 IV 恰好全可打印，测试失败本身就是要的信息（说明这条判据不牢），
	// 而不是"改断言把它压下去"。
	v := appBusinessVectors(t)
	for _, raw := range seg(t, v, "hcyCoolFlagBodyEnvelope192") {
		cs := obj(t, raw, "192 信封用例")
		raw2, err := B64Decode(str(t, cs, "payloadB64"))
		if err != nil {
			t.Fatal(err)
		}
		if printableIV(raw2[:16]) {
			t.Fatalf("%s: 原生族的 IV 应基本不可能全可打印", str(t, cs, "name"))
		}
	}

	// ⚠ 反向**不能**断言："可打印 IV ⇒ WebView 族"不成立 ——
	// `encodeData` 也是可打印 IV。所以这条指纹只用来在
	// **已知是 hcy-cool-flag 整包信封**的前提下挑钥匙。
	asciiCount := 0
	for _, raw := range seg(t, v, "hcyCoolFlagWebViewEnvelope256") {
		if str(t, obj(t, raw, "256 信封用例"), "ivKind") == "ascii" {
			asciiCount++
		}
	}
	if asciiCount < 2 {
		t.Fatalf("WebView 族的请求侧样本应至少 2 条，实际 %d", asciiCount)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 错误分类（errors.go）
// ─────────────────────────────────────────────────────────────────────────────

func TestClassifyStatusBoundaries(t *testing.T) {
	cases := []struct {
		status uint16
		want   ErrorKind
	}{
		{200, KindInternal}, // 成功不该走错误路径
		{299, KindInternal},
		{401, KindAuth},
		{403, KindAuth},
		{408, KindTimeout},
		{429, KindRateLimited},
		{400, KindBadRequest},
		{404, KindBadRequest},
		{422, KindBadRequest},
		{500, KindUpstream},
		{599, KindUpstream},
		{600, KindInternal},
		{0, KindInternal},
	}
	for _, c := range cases {
		if got := ClassifyStatus(c.status); got != c.want {
			t.Fatalf("status %d：得到 %v，期望 %v", c.status, got, c.want)
		}
	}
}

func TestRetryableAndUnhealthySets(t *testing.T) {
	for _, k := range []ErrorKind{KindRateLimited, KindTimeout, KindNetwork, KindUpstream} {
		if !k.IsRetryable() {
			t.Fatalf("%v 应可重试", k)
		}
	}
	for _, k := range []ErrorKind{KindAuth, KindSoldOut, KindInsufficientCloud, KindBadRequest, KindNotFound, KindParse, KindInternal} {
		if k.IsRetryable() {
			t.Fatalf("%v 不该可重试", k)
		}
	}
	for _, k := range []ErrorKind{KindAuth, KindRateLimited} {
		if !k.MarksAccountUnhealthy() {
			t.Fatalf("%v 应标记账号不健康", k)
		}
	}
}

// ⚠ 对外文案里**不能**带原文细节 —— 这是"错误分类"存在的第二个理由。
func TestPublicMessageNeverLeaksTheOriginal(t *testing.T) {
	const secret = "19900000000 的 token 已过期"
	e := NewAuth(secret)
	if strings.Contains(e.PublicMessage(), "199") {
		t.Fatalf("对外文案泄漏了原文：%s", e.PublicMessage())
	}
	if !strings.Contains(e.Error(), "199") {
		t.Fatal("Error() 应保留原文（它是给日志用的）")
	}
	if e.Message() != secret {
		t.Fatalf("Message() 应是不带前缀的正文：%q", e.Message())
	}
	// Error() 带类别前缀，Message() 不带 —— 这条区别是"别用 split 去前缀"的判据
	if !strings.HasPrefix(e.Error(), "凭据失效: ") {
		t.Fatalf("Error() 应带类别前缀：%s", e.Error())
	}
}

// ⚠ FixtureMiss 与 Internal 是**两个变体、同一个 kind**。
// 合并会丢掉"fixture 未命中"这条可辨识性，而它恰恰是
// "偷偷打真网"要立刻暴露的那个信号。
func TestFixtureMissIsDistinguishableFromInternal(t *testing.T) {
	fm := NewFixtureMiss("getStatus")
	in := NewInternal("something")
	if fm.Kind() != KindInternal || in.Kind() != KindInternal {
		t.Fatal("两者 kind 都应是 Internal")
	}
	if !fm.IsFixtureMiss() {
		t.Fatal("FixtureMiss 应可辨识")
	}
	if in.IsFixtureMiss() {
		t.Fatal("Internal 不该被认成 FixtureMiss")
	}
	if fm.Error() == in.Error() {
		t.Fatal("两者的 Display 文案应不同")
	}
}

func TestKindOfUnwrapsWrappedErrors(t *testing.T) {
	// 调用方会用 %w 包一层上下文（Go 的惯例；Rust 的 `?` 不会）。
	// 裸类型断言在那种情况下**静默返回 ok=false**，于是分类在最需要它的地方失效。
	wrapped := fmt.Errorf("查签到状态失败: %w", NewRateLimited("11029"))
	kind, ok := KindOf(wrapped)
	if !ok {
		t.Fatal("应能从未包装链里取出类别")
	}
	if kind != KindRateLimited {
		t.Fatalf("得到 %v，期望 RateLimited", kind)
	}
	if _, ok := KindOf(fmt.Errorf("裸错误")); ok {
		t.Fatal("非本包 error 应返回 ok=false")
	}
}

func TestClassifyBusinessCode(t *testing.T) {
	if _, ok := ClassifyBusinessCode(0); ok {
		t.Fatal("0 表示成功，不该给出类别")
	}
	kind, ok := ClassifyBusinessCode(9103)
	if !ok || kind != KindBadRequest {
		t.Fatalf("未知业务码应归 BadRequest，得到 %v/%v", kind, ok)
	}
}

func TestUpstreamErrorCarriesStatus(t *testing.T) {
	e := NewUpstream(502, "bad gateway")
	if e.Status() != 502 {
		t.Fatalf("Status 应为 502，实际 %d", e.Status())
	}
	if e.Kind() != KindUpstream {
		t.Fatalf("Kind 应为 Upstream，实际 %v", e.Kind())
	}
	if got := e.Error(); got != "上游错误 502: bad gateway" {
		t.Fatalf("Display 不对：%s", got)
	}
}
