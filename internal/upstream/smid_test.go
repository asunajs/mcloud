package upstream

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"reflect"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// ─────────────────────────────────────────────────────────────────────────────
// TS 黄金向量
// ─────────────────────────────────────────────────────────────────────────────

// smidVectors 是 `test/golden/crypto-vectors.json` 里 `smid*` 那四段。
//
// ⚠ 生成侧是 **TS 原版**（`shared/src/smid/index.ts` + crypto-js 4.2.0），
// 消费侧是本项目自带的 DES / AES / gzip ⇒ 两边同值才叫交叉验证。
// 这与"Go 自己算、Go 自己验"不是一回事。
type smidVectors struct {
	SmidString []struct {
		Name       string `json:"name"`
		TimeDigits string `json:"timeDigits"`
		UID        string `json:"uid"`
		Expect     string `json:"expect"`
	} `json:"smidString"`
	SmidPriID []struct {
		Name   string `json:"name"`
		UID    string `json:"uid"`
		Expect string `json:"expect"`
	} `json:"smidPriId"`
	SmidTn []struct {
		Name   string `json:"name"`
		Target any    `json:"target"`
		Expect string `json:"expect"`
	} `json:"smidTn"`
	SmidDesRules []struct {
		Name   string `json:"name"`
		Target any    `json:"target"`
		Expect any    `json:"expect"`
	} `json:"smidDesRules"`
}

func loadSmidVectors(t *testing.T) smidVectors {
	t.Helper()
	raw, err := golden.Load("crypto-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	out, ok := core.ParseJSONTo[smidVectors](string(raw))
	if !ok {
		t.Fatal("黄金向量里解析不出 smid* 四段")
	}
	return out
}

// scriptedRng 按脚本吐字节的随机源 —— 本模块的**全部**随机都收敛到了
// [core.Random]，所以给定脚本，整份特征、uid、RSA 填充都可复现。
//
// ⚠ Go 的 [core.Random] 只有 `FillBytes` 一个方法（core 不许有别的随机入口），
// 所以**注入不了任意字符串** —— 这与 Rust 侧那个能直接返回 uuid 字符串的
// `Random` 不同，见 [TestEveryFieldOfTheFeatureBodyTracesBackToTheVector]。
type scriptedRng struct{ script []byte }

func (r *scriptedRng) FillBytes(dst []byte) {
	n := copy(dst, r.script)
	r.script = r.script[n:]
}

// uuidBytes 把形如 `6b4a3c2d-1111-4222-8333-444444444444` 的 uuid 还原成 16 字节。
//
// 之所以能这么做：[core.UUIDv4] 只对 b[6] / b[8] 做版本位与 variant 位的**置位**，
// 而向量里那两个 uuid 已经带着正确的位 ⇒ 掩完还是原值。
func uuidBytes(t *testing.T, uuid string) []byte {
	t.Helper()
	raw, err := hex.DecodeString(strings.ReplaceAll(uuid, "-", ""))
	if err != nil || len(raw) != 16 {
		t.Fatalf("uuid 不是 16 字节的十六进制: %q", uuid)
	}
	return raw
}

func testUpload() SmUpload {
	return SmUpload{TimeDigits: "20260919153000", NowMillis: 1_789_743_328_974, OldDeviceID: ""}
}

func desOf(t *testing.T, key, message string) string {
	t.Helper()
	out, err := core.DESECBNoPaddingEncryptBase64(key, message)
	if err != nil {
		t.Fatalf("DES 失败: %v", err)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯计算那几段的对拍
// ─────────────────────────────────────────────────────────────────────────────

// TestSmidStringMatchesTheTSVectors：`smid` 的拼接规则由 TS 侧钉住
// （`v = 时间 + md5(uid) + "00"`，`smid = v + md5("smsk_web_"+v)[0..14] + "0"`）。
func TestSmidStringMatchesTheTSVectors(t *testing.T) {
	cases := loadSmidVectors(t).SmidString
	if len(cases) == 0 {
		t.Fatal("smidString 段不能为空")
	}
	for _, c := range cases {
		got := SmidString(c.TimeDigits, c.UID)
		if got != c.Expect {
			t.Errorf("用例 %s: 应为 %s，实得 %s", c.Name, c.Expect, got)
		}
		if len(got) != 63 {
			t.Errorf("用例 %s: 长度应恒为 63，实得 %d", c.Name, len(got))
		}
	}
}

// TestPrivateIDMatchesTheTSVectors：`priId` 是 AES 密钥，必须正好 16 个字符
// （短一位 AES-256、长一位被截断，两种都不报错）。
func TestPrivateIDMatchesTheTSVectors(t *testing.T) {
	cases := loadSmidVectors(t).SmidPriID
	if len(cases) == 0 {
		t.Fatal("smidPriId 段不能为空")
	}
	for _, c := range cases {
		got := SmPrivateID(c.UID)
		if got != c.Expect {
			t.Errorf("用例 %s: 应为 %s，实得 %s", c.Name, c.Expect, got)
		}
		if len(got) != 16 {
			t.Errorf("用例 %s: AES 密钥必须是 16 个字符，实得 %d", c.Name, len(got))
		}
	}
}

// TestTnMatchesTheTSVectors：`tn` 是"特征值拼接串"的 md5。
// 向量给的 target 里 `tn` **就是空串** —— 这条同时锁住"摘要先算、再写回"的顺序。
func TestTnMatchesTheTSVectors(t *testing.T) {
	cases := loadSmidVectors(t).SmidTn
	if len(cases) == 0 {
		t.Fatal("smidTn 段不能为空")
	}
	for _, c := range cases {
		obj, _ := c.Target.(map[string]any)
		if text, _ := obj["tn"].(string); text != "" {
			t.Errorf("用例 %s: 向量里的 tn 必须还是空串（摘要就是在这一刻算的），实得 %q",
				c.Name, text)
		}
		if got := core.GetTnMD5(c.Target); got != c.Expect {
			t.Errorf("用例 %s: tn 应为 %s，实得 %s", c.Name, c.Expect, got)
		}
	}
}

// TestTheWholeDesRuleTableMatchesTheTSVectors：25 条规则**整份**比对而不是抽查
// —— 键名或密钥写歪一条，这里就断。
//
// 外加一条覆盖率断言：向量必须真的覆盖到每一条规则，
// 否则"整份通过"可能只是恰好没测到某条。
func TestTheWholeDesRuleTableMatchesTheTSVectors(t *testing.T) {
	cases := loadSmidVectors(t).SmidDesRules
	if len(cases) == 0 {
		t.Fatal("smidDesRules 段不能为空")
	}
	covered := map[string]bool{}
	for _, c := range cases {
		obj, ok := c.Target.(map[string]any)
		if !ok {
			t.Fatalf("用例 %s: target 不是对象", c.Name)
		}
		got, perr := SmEncryptByDesRules(obj)
		if perr != nil {
			t.Fatalf("用例 %s: %v", c.Name, perr)
		}
		if !reflect.DeepEqual(got, c.Expect) {
			t.Errorf("用例 %s:\n实得 %v\n期望 %v", c.Name, got, c.Expect)
		}
		expect, _ := c.Expect.(map[string]any)
		for k := range expect {
			covered[k] = true
		}
	}
	for _, name := range SmWireNames() {
		if !covered[name] {
			t.Errorf("规则 → %s 没被任何向量覆盖 —— “整份通过”可能只是没测到它", name)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 特征体：绑定账号那一半
// ─────────────────────────────────────────────────────────────────────────────

// fingerprintOf 按向量给的特征体反推出那份"稳定特征"。
//
// 故意**不**在测试里再抄一遍常量：这样下面那条断言才等于证明
// 「整份特征体只由 [SmFingerprint] + 时间 + 旧 deviceId 决定」，
// 而不是"两边都抄了同一份字面量"。
func fingerprintOf(t *testing.T, target any) SmFingerprint {
	t.Helper()
	obj, _ := target.(map[string]any)
	text := func(key string) string { s, _ := obj[key].(string); return s }
	num := func(key string) int64 {
		n, ok := asInt64(obj[key])
		if !ok {
			return 0
		}
		return n
	}
	// 屏幕与 dpr 在特征体里只以 clientSize / res 的形式出现
	parts := strings.Split(text("res"), "_")
	if len(parts) < 3 {
		t.Fatalf("res 形如 宽_高_dpr_2，实得 %q", text("res"))
	}
	width, _ := asInt64(parts[0])
	height, _ := asInt64(parts[1])
	dpr, _ := asInt64(parts[2])
	return SmFingerprint{
		Ua:             text("ua"),
		PageURL:        text("url"),
		Platform:       text("platform"),
		ScreenWidth:    uint32(width),
		ScreenHeight:   uint32(height),
		Dpr:            dpr,
		MaxTouchPoints: num("maxTouchPoints"),
		ConnectionRtt:  num("connectionRtt"),
		CPUCount:       num("cpucount"),
		Canvas:         text("canvas"),
	}
}

// TestEveryFieldOfTheFeatureBodyTracesBackToTheVector —— "绑定账号"最硬的一条：
// 从向量反推指纹，再正向拼回去，逐字段相等。
//
// # 为什么 `vpw` / `trees` 两条被排除
//
// Go 的 [core.Random] 只有一个 `FillBytes`，注入不了任意字符串；而向量里
// 这两个字段是**带前缀的夹具标签**（`vpw-00000000-0000-4000-8000-…`），
// 不是 uuid —— [core.UUIDv4] 产不出它。
//
// ⇒ 这不是"测不了"，是**夹具本身不可复现**；对协议无影响（线上那两个值就是
// `randomUUID()`）。用形状断言补上，别把这条读成"少测了两个字段"。
func TestEveryFieldOfTheFeatureBodyTracesBackToTheVector(t *testing.T) {
	cases := loadSmidVectors(t).SmidTn
	smidUIDs := []string{
		"6b4a3c2d-1111-4222-8333-444444444444",
		"9a2b3c4d-1111-4222-8333-444444444444",
	}
	for i, c := range cases {
		if i >= len(smidUIDs) {
			break
		}
		obj, _ := c.Target.(map[string]any)
		fingerprint := fingerprintOf(t, c.Target)
		smid, _ := obj["smid"].(string)
		if len(smid) < 14 {
			t.Fatalf("用例 %s: smid 应以 14 位时间打头，实得 %q", c.Name, smid)
		}
		upload := SmUpload{
			TimeDigits:  smid[:14],
			NowMillis:   0,
			OldDeviceID: "",
		}
		if n, ok := asInt64(obj["svm"]); ok {
			upload.NowMillis = n
		}
		if box, ok := obj["box"].(string); ok {
			upload.OldDeviceID = box
		}

		// 字节脚本：smid 用的 uid（16）+ vpw（16）+ trees（16）+ charging（2）+ level（2）
		// level 要落到 0.5，得让那两字节 = 32768（32768/65535*100 四舍五入 = 50）
		rng := &scriptedRng{script: concat(
			uuidBytes(t, smidUIDs[i]),
			make([]byte, 16),
			make([]byte, 16),
			[]byte{0, 0},
			[]byte{0x80, 0x00},
		)}
		built := SmFingerprintTarget(fingerprint, upload, rng)

		for field, want := range obj {
			if field == "vpw" || field == "trees" {
				continue // 见上面的说明：夹具值不可复现
			}
			if !sameValue(built[field], want) {
				t.Errorf("用例 %s 字段 %s：实得 %v，期望 %v", c.Name, field, built[field], want)
			}
		}
		if len(built) != len(obj) {
			t.Errorf("用例 %s: 字段数应一致（实得 %d，期望 %d）—— 多一个或少一个都说明有字段来自别处",
				c.Name, len(built), len(obj))
		}
		// 那两条只验形状：36 位 uuid
		for _, field := range []string{"vpw", "trees"} {
			text, _ := built[field].(string)
			if len(text) != 36 {
				t.Errorf("用例 %s: %s 应是 uuid 形状（36 位），实得 %q", c.Name, field, text)
			}
		}

		// 顺带钉住那几个"写死但不是指纹"的字段：值来自旧代码，改一个就是换协议
		for field, want := range map[string]any{
			"protocol":     int64(4),
			"version":      "3.0.0",
			"sdkver":       "3.0.0",
			"subVersion":   "1.0.0",
			"rtype":        "all",
			"status":       "0010",
			"timezone":     int64(-480),
			"time":         int64(7),
			"cdp":          int64(0),
			"os":           "web",
			"organization": SmOrganization,
			"appId":        SmAppID,
			"plugins":      SmPlugins,
			"referer":      "",
		} {
			if text, ok := want.(string); ok {
				if got, _ := built[field].(string); got != text {
					t.Errorf("用例 %s: %s 应为 %q，实得 %q", c.Name, field, text, got)
				}
				continue
			}
			if got, ok := asInt64(built[field]); !ok || got != want.(int64) {
				t.Errorf("用例 %s: %s 应为 %v，实得 %v", c.Name, field, want, built[field])
			}
		}
	}
}

// numericText 把数字统一成文本。
//
// ⚠ 见 §6.1 #34：[core.ParseJSON] 为了不让 19 位标识丢精度返回的是
// [json.Number]（**字符串**形态），而 Go 侧自己拼出来的特征体里是 `int64` /
// `float64`。直接 `reflect.DeepEqual` 会得到"实得 -480，期望 -480"这种
// 一眼看不出哪里不同的报错。⇒ 数字一律**比文本**。
func numericText(value any) (string, bool) {
	switch v := value.(type) {
	case json.Number:
		return v.String(), true
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64), true
	case int:
		return strconv.Itoa(v), true
	case int64:
		return strconv.FormatInt(v, 10), true
	}
	return "", false
}

// sameValue 比两个"JSON 值"，数字按文本比、对象逐键递归比。
func sameValue(a, b any) bool {
	at, aIsNum := numericText(a)
	bt, bIsNum := numericText(b)
	if aIsNum || bIsNum {
		return aIsNum && bIsNum && at == bt
	}
	am, aIsObj := a.(map[string]any)
	bm, bIsObj := b.(map[string]any)
	if aIsObj || bIsObj {
		if !aIsObj || !bIsObj || len(am) != len(bm) {
			return false
		}
		for k, v := range am {
			if !sameValue(v, bm[k]) {
				return false
			}
		}
		return true
	}
	return reflect.DeepEqual(a, b)
}

func concat(parts ...[]byte) []byte {
	out := []byte{}
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}

// TestTheBoundFingerprintRoundTripsThroughTheFeatureBody：
// 反推出来的指纹再序列化一次，必须还在"稳定字段"的范围内 ——
// 这条防的是"向量悄悄改了，而测试只比对了拼接结果"。
func TestTheBoundFingerprintRoundTripsThroughTheFeatureBody(t *testing.T) {
	cases := loadSmidVectors(t).SmidTn
	fingerprint := fingerprintOf(t, cases[0].Target)
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		t.Fatalf("序列化失败: %v", perr)
	}
	back, perr := SmFingerprintFromJSON(text)
	if perr != nil {
		t.Fatalf("读回来失败: %v", perr)
	}
	if !reflect.DeepEqual(back, fingerprint) {
		t.Errorf("往返应回到同一份指纹，实得 %+v，期望 %+v", back, fingerprint)
	}
	obj, _ := cases[0].Target.(map[string]any)
	if want, _ := obj["clientSize"].(string); back.ClientSize() != want {
		t.Errorf("clientSize 应为 %s，实得 %s", want, back.ClientSize())
	}
}

// TestFingerprintFromJSONRefusesInsteadOfReRolling：读不出来必须**报错**，
// 不能回落到"重新掷一份"—— 那等于静默换掉这台设备的长相，
// 而换指纹在服务侧是可观测事件。
func TestFingerprintFromJSONRefusesInsteadOfReRolling(t *testing.T) {
	if _, perr := SmFingerprintFromJSON("{" + `"ua":`); perr == nil {
		t.Fatal("坏 JSON 应报错；静默重掷一份会让账号悄悄换脸")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 载荷编码
// ─────────────────────────────────────────────────────────────────────────────

// decodeSmData 把 `data` 一路解回特征体：
// AES(固定 IV, priId) → hex → base64 → gzip → JSON。
func decodeSmData(t *testing.T, dataHex, uid string) map[string]any {
	t.Helper()
	b64, err := core.AESCBCFixedIVDecryptHex(SmPrivateID(uid), dataHex)
	if err != nil {
		t.Fatalf("解 data 失败: %v", err)
	}
	gz, err := core.B64Decode(b64)
	if err != nil {
		t.Fatalf("base64 解码失败: %v", err)
	}
	plain, err := core.Gunzip(gz)
	if err != nil {
		t.Fatalf("gunzip 失败: %v", err)
	}
	value, ok := core.ParseJSON(string(plain))
	if !ok {
		t.Fatalf("解出来的不是合法 JSON: %s", plain)
	}
	obj, _ := value.(map[string]any)
	return obj
}

// TestPayloadDecodesBackToTheBoundFingerprint：这条是"绑定账号"这件事的
// **唯一**硬证据 —— 不解开看，就只能断言"算出了一个 hex 串"，那等于没测。
func TestPayloadDecodesBackToTheBoundFingerprint(t *testing.T) {
	uid := "11111111-2222-4333-8444-555555555555"
	fingerprint := CaiyunAndroidWebFingerprint()
	upload := testUpload()
	// 电池是这一族里唯一"每次现掷"的字段，所以用**脚本**字节把它的值钉住，
	// 而不是事后断言"它要么是 0 要么是 1"（那等于没断言）。
	rng := &scriptedRng{script: concat(
		make([]byte, 16),   // smid 用的 uid
		make([]byte, 16),   // vpw
		make([]byte, 16),   // trees
		[]byte{0, 0},       // charging → 0 % 2 == 0
		[]byte{0x80, 0x00}, // level → 32768/65535*100 ≈ 50.004 → 0.5
	)}
	target := SmFingerprintTarget(fingerprint, upload, rng)
	data, perr := SmEncodeTarget(target, uid)
	if perr != nil {
		t.Fatalf("编码失败: %v", perr)
	}
	decoded := decodeSmData(t, data, uid)

	want := map[string]string{
		"lz": desOf(t, "j862vr1f", "web"),                    // os
		"um": desOf(t, "a9kbown8", SmAppID),                  // appId
		"nl": desOf(t, "14e30pp1", SmOrganization),           // organization
		"pu": desOf(t, "bewicpxu", fingerprint.Ua),           // ua
		"lm": desOf(t, "pgutfbo7", fingerprint.Platform),     // platform
		"na": desOf(t, "7s9eixjh", fingerprint.ClientSize()), // clientSize
		"ut": desOf(t, "ht2n6dwf", fingerprint.Res()),        // res
	}
	for field, expect := range want {
		if got, _ := decoded[field].(string); got != expect {
			t.Errorf("%s 应为 %s，实得 %s", field, expect, got)
		}
	}
	// 不加密的三条：改了名，值原样
	if got, ok := asInt64(decoded["protocol"]); !ok || got != 4 {
		t.Errorf("protocol 应原样为 4，实得 %v", decoded["protocol"])
	}
	if got, _ := decoded["version"].(string); got != "3.0.0" {
		t.Errorf("version 应原样为 3.0.0，实得 %q", got)
	}
	if got, _ := decoded["aw"].(string); got != "" {
		t.Errorf("aw（box 改名）应原样为空串，实得 %q", got)
	}
	// 没有规则的：原名原值
	if got, ok := asInt64(decoded["cdp"]); !ok || got != 0 {
		t.Errorf("cdp 应原名为 0，实得 %v", decoded["cdp"])
	}
	battery, _ := decoded["battery"].(map[string]any)
	if got, ok := asInt64(battery["charging"]); !ok || got != 0 {
		t.Errorf("battery.charging 应原样为 0，实得 %v", battery["charging"])
	}
	// level 是 `Math.round(x*100)/100` —— 两位小数，且**不加密、不改名**
	if got, ok := numericText(battery["level"]); !ok || got != "0.5" {
		t.Errorf("battery.level 应原样为 0.5，实得 %v", battery["level"])
	}
	if len(battery) != 2 {
		t.Errorf("battery 只有 charging / level 两项，实得 %v", battery)
	}
	smid, _ := decoded["smid"].(string)
	if len(smid) != 63 {
		t.Errorf("smid 应为 63 位，实得 %d", len(smid))
	}
	// tn：摘要是在 `tn` 还是空串时算的
	if got := desOf(t, "crnw0x7e", core.GetTnMD5(target)); decoded["as"] != got {
		t.Errorf("as（tn 改名）应为 %s，实得 %v", got, decoded["as"])
	}
}

// TestTnMustBeDigestedWhileTnIsStillEmpty：回归护栏 —— 先写回 tn 再算摘要，
// data 会整串不同（而线上不会报错，只会认不出一台设备）。
func TestTnMustBeDigestedWhileTnIsStillEmpty(t *testing.T) {
	uid := "11111111-2222-4333-8444-555555555555"
	target := SmFingerprintTarget(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{})
	good, _ := SmEncodeTarget(target, uid)

	badTarget := make(map[string]any, len(target)+1)
	for k, v := range target {
		badTarget[k] = v
	}
	badTarget["tn"] = core.GetTnMD5(target)
	bad, _ := SmEncodeTarget(badTarget, uid)
	if good == bad {
		t.Error("顺序错了必须能被测出来 —— 两种顺序得到了同一个 data")
	}
}

// TestBuildSmDataProducesTheLegacyEnvelope。
func TestBuildSmDataProducesTheLegacyEnvelope(t *testing.T) {
	payload, perr := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	for _, key := range []string{"ep", "data", "organization", "appId"} {
		if _, ok := payload[key]; !ok {
			t.Errorf("上报体应含 %s，实得 keys=%v", key, payload)
		}
	}
	if payload["os"] != "web" {
		t.Errorf("os 应为 web，实得 %v", payload["os"])
	}
	if got, ok := asInt64(payload["encode"]); !ok || got != 5 {
		t.Errorf("encode 应为 5，实得 %v", payload["encode"])
	}
	if got, ok := asInt64(payload["compress"]); !ok || got != 2 {
		t.Errorf("compress 应为 2，实得 %v", payload["compress"])
	}
	if payload["appId"] != SmAppID || payload["organization"] != SmOrganization {
		t.Errorf("appId / organization 不对: %v / %v", payload["appId"], payload["organization"])
	}
	data, _ := payload["data"].(string)
	if len(data)%2 != 0 || !isAllHex(data) {
		t.Errorf("data 是 AES 的 hex 文本，实得 %q", data)
	}
}

// TestTheSameFingerprintAndSeedGiveTheSamePayload：
// 注入同一个随机源就要字节级复现（fixture 回放的前提）。
//
// # ⚠ `ep` 是**唯一**复现不了的字段，而且它复现不了是**对的**
//
// `ep = RSA_PKCS1v15(uid)`，PKCS#1 v1.5 的填充里是一串随机**非零**字节；
// Go 的实现在这一步还叠了盲化，所以**即使注入了确定性随机源，两次的输出也不同**
// （已实测：同一个 `seqRandomUpstream{b: 5}` 调两次，两个密文不同但没有一个字节相同）。
// Rust 侧用 `rsa` crate 时同样如此 —— 这是算法性质，不是换语言换出来的差异。
//
// ⇒ 真正能复现的是 `data`（特征体那一整串）。所以这条断言**显式排除** `ep`，
// 并另开一条专门钉住"ep 不可复现"—— 否则将来有人想给 `ep` 写 fixture，
// 会以为是随机源没注入到位，白查一轮。
func TestTheSameFingerprintAndSeedGiveTheSamePayload(t *testing.T) {
	a, perrA := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{b: 5})
	b, perrB := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{b: 5})
	if perrA != nil || perrB != nil {
		t.Fatalf("构造失败: %v / %v", perrA, perrB)
	}
	if len(a) != len(b) {
		t.Fatalf("两次的键集应一致：%v vs %v", a, b)
	}
	for k, av := range a {
		if k == "ep" {
			continue
		}
		if !sameValue(av, b[k]) {
			t.Errorf("字段 %s 应字节级复现，\n实得 %v\n期望 %v", k, av, b[k])
		}
	}
	// data 是这一族唯一"整串可复现"的东西 —— 单独再钉一次，别被 map 比较糊过去
	if da, _ := a["data"].(string); da == "" || da != b["data"] {
		t.Errorf("data 应整串复现，实得 %q vs %q", a["data"], b["data"])
	}
}

// TestEpIsNotReproducibleByDesign：`ep` 每次都不同 —— 上面那条排除它不是"测不了"。
func TestEpIsNotReproducibleByDesign(t *testing.T) {
	a, _ := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{b: 5})
	b, _ := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{b: 5})
	ea, _ := a["ep"].(string)
	eb, _ := b["ep"].(string)
	if ea == "" || eb == "" {
		t.Fatalf("ep 不该为空: %q / %q", ea, eb)
	}
	if ea == eb {
		t.Error("ep 每次都应不同（PKCS#1 v1.5 填充含随机字节）；一样说明它被悄悄换成了确定性算法")
	}
	// 形状：这把公钥是 **1024 位**，所以密文是 128 字节。
	// 钉住长度是因为"换了把 2048 位的公钥"会静默产出更长的密文，
	// 服务端解不开也不报具体原因 —— 那条要在这里响。
	raw, err := core.B64Decode(ea)
	if err != nil {
		t.Fatalf("ep 应是 base64: %v", err)
	}
	if len(raw) != 128 {
		t.Errorf("ep 应是 RSA-1024 密文（128 字节），实得 %d —— 公钥被换过了？", len(raw))
	}
}

// TestGenerateFingerprintPicksOneScreenPair：屏幕宽高必须来自**同一档**
// （`SCREENS[idx]` 一次解构出一对），取两次会编出 `1920×667` 这种不存在的组合。
func TestGenerateFingerprintPicksOneScreenPair(t *testing.T) {
	valid := map[[2]uint32]bool{}
	for _, pair := range SmScreens {
		valid[pair] = true
	}
	for seed := 0; seed < 64; seed++ {
		got := GenerateSmFingerprint(&seqRandomUpstream{b: byte(seed)})
		if !valid[[2]uint32{got.ScreenWidth, got.ScreenHeight}] {
			t.Fatalf("宽高 (%d,%d) 不在池子里 —— 说明取了两次 rng", got.ScreenWidth, got.ScreenHeight)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求与响应
// ─────────────────────────────────────────────────────────────────────────────

// TestDeviceProfileRequestCarriesNoCloudCredentials：这台主机不是云盘的，
// 所以**不带**任何云盘凭据 —— 它既不需要，也不该看到。
func TestDeviceProfileRequestCarriesNoCloudCredentials(t *testing.T) {
	fingerprint := CaiyunAndroidWebFingerprint()
	request, perr := SmDeviceProfileRequest(map[string]any{"a": 1}, fingerprint)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if !strings.HasPrefix(request.URL, SmAPIHost) || !strings.HasSuffix(request.URL, SmAPIPath) {
		t.Errorf("应打 %s%s，实得 %q", SmAPIHost, SmAPIPath, request.URL)
	}
	for _, name := range []string{"authorization", "jwttoken", "x-yun-uni", "cookie"} {
		if countHeader(request.Headers, name) != 0 {
			t.Errorf("不该把 %s 发给第三方风控服务", name)
		}
	}
	if got := headerValue(request.Headers, "origin"); got != SmRequestOrigin {
		t.Errorf("origin 应为 %s，实得 %q", SmRequestOrigin, got)
	}
	if got := headerValue(request.Headers, "referer"); got != SmRequestReferer {
		t.Errorf("referer 应为 %s，实得 %q", SmRequestReferer, got)
	}
	// ⚠ 头里的 ua 用的是**特征**里那份（网页 UA），不是设备档案那份
	if got := headerValue(request.Headers, "user-agent"); got != fingerprint.Ua {
		t.Errorf("user-agent 应取特征里那份，实得 %q", got)
	}
}

// TestParseDeviceIDPrefixedWithB：服务端回的是裸 `deviceId`，
// 客户端在它前面硬拼一个 `B` —— 最终进 `deviceid` 头的是拼好的那个。
func TestParseDeviceIDPrefixedWithB(t *testing.T) {
	got, perr := SmParseDeviceID(map[string]any{
		"code": 1100,
		"detail": map[string]any{
			"deviceId": "1990000000000000000000000000000000000000000000001",
		},
	})
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if !strings.HasPrefix(got, SmDeviceIDPrefix) {
		t.Errorf("应带 %q 前缀，实得 %q", SmDeviceIDPrefix, got)
	}
	if !strings.HasSuffix(got, "1990000000000000000000000000000000000000000000001") {
		t.Errorf("应保留服务端给的原值，实得 %q", got)
	}
}

// TestParseDeviceIDRefusesInsteadOfReturningEmptyString：
// 旧实现 `catch` 掉一切返回空串 —— 空串一旦被当成"成功"存进库，
// 下次读出来是非空，于是那个账号永远绑着一个空设备、再也不会触发重新上报。
func TestParseDeviceIDRefusesInsteadOfReturningEmptyString(t *testing.T) {
	// ⚠ 用一个**不可能出现在错误文案里**的哨兵值当 deviceId：
	// 判据是"响应体里的**值**一个都不许进错误串"（它会进日志、会被断言引用），
	// 而"字段路径"（`detail.deviceId`）出现是允许的 —— 那是给人看的提示。
	const sentinel = "SENTINEL-DEVICE-VALUE"
	cases := map[string]any{
		"码不对":        map[string]any{"code": 1200, "detail": map[string]any{"deviceId": sentinel}},
		"没有 code":    map[string]any{"detail": map[string]any{"deviceId": sentinel}},
		"没有 detail":  map[string]any{"code": 1100},
		"deviceId 空": map[string]any{"code": 1100, "detail": map[string]any{"deviceId": ""}},
	}
	for name, raw := range cases {
		t.Run(name, func(t *testing.T) {
			got, perr := SmParseDeviceID(raw)
			if perr == nil {
				t.Fatalf("应报错，实得 %q", got)
			}
			if got != "" {
				t.Errorf("失败时不应返回任何值，实得 %q", got)
			}
			// 错误文案里只放**码值**，不回吐响应体（它可能带别的机器能认的标识）
			if strings.Contains(perr.Message(), sentinel) {
				t.Errorf("报错文案不该回吐响应体，实得 %q", perr.Message())
			}
		})
	}
}

// TestSmidDeviceIDReturnsErrorNotAnEmptyString：发送层同样不许把失败变成空串。
func TestSmidDeviceIDReturnsErrorNotAnEmptyString(t *testing.T) {
	up, _ := respondWith(t, map[string]any{"code": 1200})
	payload, _ := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{})
	got, perr := up.SmidDeviceID(context.Background(), CaiyunAndroidWebFingerprint(), payload)
	if perr == nil {
		t.Fatalf("被拒时应报错，实得 %q", got)
	}
	if got != "" {
		t.Errorf("失败时不应返回任何值，实得 %q", got)
	}
}

// TestSmidDeviceIDHappyPath。
func TestSmidDeviceIDHappyPath(t *testing.T) {
	up, _ := respondWith(t, map[string]any{
		"code": 1100,
		"detail": map[string]any{
			"deviceId": "1990000000000000000000000000000000000000000000001",
		},
	})
	payload, _ := BuildSmData(CaiyunAndroidWebFingerprint(), testUpload(), &seqRandomUpstream{})
	got, perr := up.SmidDeviceID(context.Background(), CaiyunAndroidWebFingerprint(), payload)
	if perr != nil {
		t.Fatalf("取 deviceId 失败: %v", perr)
	}
	if got != "B1990000000000000000000000000000000000000000000001" {
		t.Errorf("应拿到拼好前缀的 deviceId，实得 %q", got)
	}
}
