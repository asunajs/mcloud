package upstream

import (
	"encoding/hex"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// loginVectors 读 PC 端登录密码学的黄金向量。
//
// # 它和 [pcVectors] 不是同一份，也不是重复
//
// 两份的**段名不相交**：`pcclient-crypto-vectors.json` 是
// `sha1Upper` / `puzzleLeftDecrypt` / `loginDataDecrypt` / `puzzleAesCbcIvPrefixed`，
// 这一份是 `sha1Password` / `puzzleAnswer` / `aesEcbHexDecrypt` / `aesEcbEncrypt` /
// `md5SafeVerify` / `guid32`。合并不了，也不该合并 ——
// 一份锁的是"我们**发出去**的字节"，另一份锁的是"客户端源码里那套算法的**输出**"。
//
// # 为什么它此前是死向量
//
// 生成器 `scripts/gen-login-crypto-vectors.cjs` 一直存在，消费者一个都没有
// （冻结的 Rust 侧只把它当素材，没有 `include_str!`）。
// 于是门禁的消费者那一栏长期打 `⚠ 无任何测试读取（死向量）`。
// 本文件就是它的消费者 —— ⚠ 门禁靠**字面量** `golden.Load("…")` 认这件事。
func loginVectors(t *testing.T) map[string]any {
	t.Helper()
	raw, err := golden.Load("login-crypto-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	var out map[string]any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	return out
}

// vectorObject 把向量里的一条断言成对象。
func vectorObject(t *testing.T, value any, label string) map[string]any {
	t.Helper()
	out, ok := value.(map[string]any)
	if !ok {
		t.Fatalf("%s 不是对象：%#v", label, value)
	}
	return out
}

// vectorSection 取一段向量，并断言它**非空**。
//
// ⚠ 空段必须报错，不能"循环零次然后全绿"：段名写错时拿到的是空数组，
// 而一个不跑任何断言的测试与一个通过的测试在输出上**长得一模一样**。
func vectorSection(t *testing.T, v map[string]any, section string) []any {
	t.Helper()
	raw, ok := v[section]
	if !ok {
		t.Fatalf("向量里没有 §%s 这一段（段名改了？）", section)
	}
	entries, ok := raw.([]any)
	if !ok {
		t.Fatalf("§%s 不是数组：%#v", section, raw)
	}
	if len(entries) == 0 {
		t.Fatalf("§%s 是空数组 —— 这样得到的「全绿」什么都没测", section)
	}
	return entries
}

// ─────────────────────────────────────────────────────────────────────────────
// 常量表
// ─────────────────────────────────────────────────────────────────────────────

// TestLoginVectorConstantsAllHaveAGoCounterpart 钉住向量 `constants` 段与 Go 常量的对应。
//
// 判据是**双向**的，因为两侧都可能单方面漂移：
//
//   - 向量里的每个键，要么在 `asserted` 里、要么在 `notInGo` 里登记过。
//     挡的是"向量新增一个常量，Go 侧静默没人接" —— 没人接的常量就是一行注释；
//   - `asserted` 里的每个键都必须在向量里。挡的是"向量改名/删键之后，
//     Go 这边还在断言一个已不存在的名字" —— 那时循环体一次都不进，
//     测试**照样是绿的**（这个坑比报错难发现得多）。
func TestLoginVectorConstantsAllHaveAGoCounterpart(t *testing.T) {
	constants := vectorObject(t, loginVectors(t)["constants"], "constants")

	// 键 → Go 侧的值。这张表就是"哪些常量真的被断言了"的唯一真相。
	asserted := map[string]string{
		"AES_ECB_KEY_PROD": LoginAESKey,
		"AES_ECB_KEY_TEST": LoginAESKeyTest,
		"PUZZLE_KEY":       PuzzleAESKey,
		"PUZZLE_IV":        string(PuzzleAESIV[:]),
		"HASHTEXT":         PasswordHashPrefix,
	}

	// 向量里有、Go 侧**刻意没有**对应物的常量。逐个写明理由，
	// 而不是让它们落进上面那条"新增必须有人接"的检查里。
	notInGo := map[string]string{
		"ENV_PROD": "客户端 `systemEnvironment` 的取值。Go 侧由调用方显式选构造函数" +
			"（NewLoginDataDecryptorProduction / NewLoginDataDecryptorTestEnv），" +
			"不建模环境码 —— 环境是**配置**，不是协议字段。",
		"ENV_TEST": "同上。`11 / 22` 这两个数只出现在 LoginAESKeyTest 的注释里，没有常量。",
		"DEVICE_FINGERPRINT_SUFFIX": "断言在 internal/app（`DeviceIDSuffix` 是 app 包的常量，" +
			"设备 id 的生成也在那边）。在这里断言就得 import app，而 app 已经 import 本包。",
	}

	for key, value := range constants {
		if want, ok := asserted[key]; ok {
			if got := vectorString(t, value); got != want {
				t.Errorf("常量 %s：向量 %q vs 代码 %q", key, got, want)
			}
			continue
		}
		if _, ok := notInGo[key]; ok {
			continue
		}
		t.Errorf("向量新增了常量 %s，Go 侧既没断言也没登记为「无对应物」 —— "+
			"新常量必须有人接，否则它只是一行注释", key)
	}

	for key := range asserted {
		if _, ok := constants[key]; !ok {
			t.Errorf("Go 侧在断言常量 %s，但向量里已经没有这个键了"+
				"（改名或删除？循环不会报错，只会少测一条）", key)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令哈希（客户端 `encryptByHash`）
// ─────────────────────────────────────────────────────────────────────────────

func TestLoginVectorSha1PasswordHashesWithTheSourcePrefix(t *testing.T) {
	for _, raw := range vectorSection(t, loginVectors(t), "sha1Password") {
		e := vectorObject(t, raw, "sha1Password 用例")
		password := vectorString(t, e["password"])
		want := vectorString(t, e["expectHexUpper"])
		if got := PasswordHash(password); got != want {
			t.Errorf("PasswordHash(%q) = %s，期望 %s", password, got, want)
		}
	}

	// 判别力：**漏掉前缀得到的也是一串合法的大写 hex**，不会报错，
	// 上游只会说"口令不对"。所以"前缀真的参与了运算"必须单独钉一条 ——
	// 挑 "123456" 是因为向量里就有它，不是随手取的。
	if PasswordHash("123456") == core.SHA1HexUpper([]byte("123456")) {
		t.Error("口令哈希没有拼前缀 —— 两边都是合法大写 hex，只有这条能分辨")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 安全验证码（客户端 `safeVerifyCodeEncrypt`）
// ─────────────────────────────────────────────────────────────────────────────

// TestLoginVectorMd5SafeVerifyIsUppercaseMd5 钉住 `secinfo` 那条规则。
//
// ⚠ Go 侧**没有**这个函数，而且是有意的：`login.go` 里
// [NewPasswordThirdLoginBody] 的注释写着安全登录分支（服务端下发 `smsCheckRandom`
// 时改用 `secinfo`）"实测没走到那条分支…等真遇到再补，不先写猜测性代码"。
//
// 所以这条测的不是"我们的实现"，而是**实现时要照抄的那条规则**：
// `MD5(secinfo)` 取**大写** hex。真到要写的那天，它就是现成的判据。
// 现在不写函数，是因为写了没有消费者 —— 见 plan §6.0。
func TestLoginVectorMd5SafeVerifyIsUppercaseMd5(t *testing.T) {
	for _, raw := range vectorSection(t, loginVectors(t), "md5SafeVerify") {
		e := vectorObject(t, raw, "md5SafeVerify 用例")
		input := vectorString(t, e["input"])
		want := vectorString(t, e["expectHexUpper"])

		// core.MD5Hex 返回**小写**（对齐 TS 的 crypto.ts），向量给的是大写。
		// 这个大小写差异在这里是**边界**不是风格：值要进请求体，服务端按 hex 解析。
		lower := core.MD5Hex([]byte(input))
		// 判别力先自检：这一组输入若算不出小写字母，那"必须大写"就无从验起
		// （`strings.ToUpper` 会变成一个把小写"修好"的掩饰）。
		if lower == strings.ToUpper(lower) {
			t.Errorf("%s: 算出的值里没有小写字母，验不了「必须大写」", input)
			continue
		}
		if got := strings.ToUpper(lower); got != want {
			t.Errorf("safeVerifyCodeEncrypt(%q) = %s，期望 %s", input, got, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录响应 data（AES-128-ECB/PKCS7，hex 入参）
// ─────────────────────────────────────────────────────────────────────────────

// 向量把**同一批密文**写了两遍，两个方向的表述不同：
//
//	aesEcbHexDecrypt[].inputHex    是 hex（客户端 `decryptByAes` 的入参形态）
//	aesEcbEncrypt[].expectB64      是 base64（客户端 `encryptByAes` 的输出形态）
//
// Go 侧只有 hex 那一个形态（[core.AESECBPKCS7EncryptHex]）—— 这不是缺口：
// base64 那个方向 PC 端根本不用（PC 只解密响应），它是 App 端才有的，
// 而 App 端那套输出的是 hex。所以下面按 Go 的形态断言，
// 再单独对拍"两段说的确实是同一批字节"。

func TestLoginVectorAesEcbDecryptsHexCiphertext(t *testing.T) {
	for _, raw := range vectorSection(t, loginVectors(t), "aesEcbHexDecrypt") {
		e := vectorObject(t, raw, "aesEcbHexDecrypt 用例")
		name := vectorString(t, e["name"])
		got, err := core.AESECBPKCS7DecryptHex(vectorString(t, e["key"]), vectorString(t, e["inputHex"]))
		if err != nil {
			t.Errorf("%s: %v", name, err)
			continue
		}
		if want := vectorString(t, e["expectPlain"]); got != want {
			t.Errorf("%s: 得到 %q，期望 %q", name, got, want)
		}
	}
}

func TestLoginVectorAesEcbEncryptsToTheSameBytes(t *testing.T) {
	for _, raw := range vectorSection(t, loginVectors(t), "aesEcbEncrypt") {
		e := vectorObject(t, raw, "aesEcbEncrypt 用例")
		name := vectorString(t, e["name"])
		cipherHex, err := core.AESECBPKCS7EncryptHex(vectorString(t, e["key"]), vectorString(t, e["plainText"]))
		if err != nil {
			t.Errorf("%s: %v", name, err)
			continue
		}
		ct, err := hex.DecodeString(cipherHex)
		if err != nil {
			t.Errorf("%s: 加密输出不是 hex: %v", name, err)
			continue
		}
		if got, want := core.B64Encode(ct), vectorString(t, e["expectB64"]); got != want {
			t.Errorf("%s: 密文 base64 得到 %s，期望 %s", name, got, want)
		}
	}
}

// TestLoginVectorAesEcbTwoSectionsDescribeTheSameCiphertext 对拍那两段。
//
// 生成器是**用一次加密同时填两段**的（`b64 = encryptByAes(plain)`、
// `inputHex = hex(b64)`），所以逐条按 `name` 对齐后两段必须互相解释得通。
//
// 这条挡的是"有人手改其中一段的一个字符"：那时**单独看每一段都还是自洽的**
// （hex 仍是合法 hex、base64 仍是合法 base64），只有对拍才看得出来。
func TestLoginVectorAesEcbTwoSectionsDescribeTheSameCiphertext(t *testing.T) {
	v := loginVectors(t)
	decrypt := vectorSection(t, v, "aesEcbHexDecrypt")
	encrypt := vectorSection(t, v, "aesEcbEncrypt")
	if len(decrypt) != len(encrypt) {
		t.Fatalf("两段条数不同：%d vs %d", len(decrypt), len(encrypt))
	}

	for i := range decrypt {
		d := vectorObject(t, decrypt[i], "aesEcbHexDecrypt 用例")
		e := vectorObject(t, encrypt[i], "aesEcbEncrypt 用例")
		name := vectorString(t, d["name"])
		if other := vectorString(t, e["name"]); name != other {
			t.Fatalf("[%d] 两段的 name 不同：%s vs %s（顺序变了？）", i, name, other)
		}
		if dk, ek := vectorString(t, d["key"]), vectorString(t, e["key"]); dk != ek {
			t.Errorf("%s: 密钥两段不同：%q vs %q", name, dk, ek)
		}
		if dp, ep := vectorString(t, d["expectPlain"]), vectorString(t, e["plainText"]); dp != ep {
			t.Errorf("%s: 明文两段不同：%q vs %q", name, dp, ep)
		}
		ct, err := hex.DecodeString(vectorString(t, d["inputHex"]))
		if err != nil {
			t.Errorf("%s: inputHex 不是合法 hex: %v", name, err)
			continue
		}
		if got, want := core.B64Encode(ct), vectorString(t, e["expectB64"]); got != want {
			t.Errorf("%s: 解密段的 hex 编成 base64 是 %s，加密段写的是 %s", name, got, want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 滑块答案（客户端 `encryptByAesCBC`）
// ─────────────────────────────────────────────────────────────────────────────

// TestLoginVectorPuzzleAnswerMatchesTheRealCaptures 逐条复算滑块答案。
//
// 这一段的证据强度是整份向量里最高的：前两条来自**真实抓包**的
// `puzzleVerfycode`，生成器用**同版本 crypto-js** 复算后逐字节命中才算数
// （算不出来生成器会直接拒绝入库）。命中即同时证明
// 密钥 / IV / 模式 / padding / 分段五件事都对 —— 缺任何一件都是完全不同的字节。
func TestLoginVectorPuzzleAnswerMatchesTheRealCaptures(t *testing.T) {
	for _, raw := range vectorSection(t, loginVectors(t), "puzzleAnswer") {
		e := vectorObject(t, raw, "puzzleAnswer 用例")
		name := vectorString(t, e["name"])

		// IV 是**硬编码常量**，不是服务端下发的挑战值 ——
		// "两次请求前缀一样"曾经被读成"同一个挑战"，实际是同一个 IV。
		// 实拍那两条自带 `ivAscii`，合成的那六条省略了它（生成器只给实拍写）；
		// 两者都必须等于同一个常量，否则"前缀恒定"就不是常量造成的。
		if iv, ok := e["ivAscii"]; ok {
			if got, want := vectorString(t, iv), string(PuzzleAESIV[:]); got != want {
				t.Errorf("%s: 向量声明的 IV 是 %q，代码里是 %q", name, got, want)
			}
		}
		if flag, ok := e["ivIsConstant"].(bool); ok && !flag {
			t.Errorf("%s: 向量声明 IV 不是常量 —— 那本测试的前提就不成立", name)
		}

		plain := vectorString(t, e["plainText"])
		got, err := core.AESDynamicIVEncrypt(PuzzleAESKey, plain, PuzzleAESIV)
		if err != nil {
			t.Errorf("%s: %v", name, err)
			continue
		}
		want := vectorString(t, e["expectB64"])
		if got != want {
			t.Errorf("%s: 得到 %s，期望 %s", name, got, want)
			continue
		}

		// 载荷结构：Base64(IV(16 字节 ASCII) ‖ CT)。
		// 结构错了**密文段却可能是对的**（CBC 只有第一块依赖 IV），
		// 所以长度与前置 IV 要单独验，不能只比整串。
		payload, err := core.B64Decode(got)
		if err != nil {
			t.Errorf("%s: %v", name, err)
			continue
		}
		if total, ok := e["totalBytes"].(float64); ok && len(payload) != int(total) {
			t.Errorf("%s: 载荷 %d 字节，向量说 %d", name, len(payload), int(total))
		}
		if len(payload) < 16 || string(payload[:16]) != string(PuzzleAESIV[:]) {
			t.Errorf("%s: 前 16 字节不是常量 IV", name)
		}
		if cipher, ok := e["cipherB64"]; ok {
			if got, want := core.B64Encode(payload[16:]), vectorString(t, cipher); got != want {
				t.Errorf("%s: 密文段 %s，向量说 %s", name, got, want)
			}
		}

		// 最强的那条：与真实抓包逐字节相同。
		// 生成器已经算过一次，这里再钉一次 —— 防止有人改掉 `realCaptureB64`
		// 让上面那条对拍**静默失效**（改了它，对拍就变成了自比）。
		if flag, ok := e["soMatchesRealCapture"].(bool); ok && flag {
			if capture := vectorString(t, e["realCaptureB64"]); got != capture {
				t.Errorf("%s: 与真实抓包不一致：算出 %s，抓包 %s", name, got, capture)
			}
		}
	}
}
