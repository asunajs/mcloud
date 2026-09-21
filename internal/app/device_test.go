package app

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/golden"
)

// seqRng 已在 signin_test.go 里定义（字节脚本回绕），这里复用。

func TestDeviceIDMatchesTheCapturedShape(t *testing.T) {
	id := NewDeviceID(&seqRng{script: []byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F}})
	// 实测：32 位大写 HEX + "-ENDIN"
	if id != "000102030405060708090A0B0C0D0E0F-ENDIN" {
		t.Errorf("实得 %q", id)
	}
	if len(id) != 32+len(DeviceIDSuffix) {
		t.Errorf("长度应为 32 + 后缀，实得 %d", len(id))
	}
	if !strings.HasSuffix(id, "-ENDIN") {
		t.Errorf("应带 -ENDIN 后缀，实得 %q", id)
	}
}

// TestDeviceFingerprintMatchesTheGoldenVectorShape 用黄金向量
// `test/golden/login-crypto-vectors.json` 的 `§guid32` 钉住**形态**。
//
// # 为什么只钉形态，不复刻 `MD5(uuid)`
//
// 客户端是 `getGUID32() = MD5(随机 UUID).toString(Hex).toUpperCase()`，再拼 `-ENDIN`
// （规则逐字写在生成器 `scripts/gen-login-crypto-vectors.cjs` 里）。
// 但**上游只看得到那 32 个字符**：它分辨不出这个值来自 `md5(uuid)` 还是来自
// 16 个随机字节 —— 两者都是 128 位均匀随机，字节分布完全一样。
// 所以「怎么算出来的」是**客户端内部实现**，不是要对位的边界（见 plan §6.0）。
//
// ⚠ 这不是 Go 单方面的偷懒：**冻结的 Rust 侧也是
// `hex::encode_upper(16 个随机字节)`**（`../mcloud-rust/crates/mcloud-app/src/device.rs`），
// 同样没有 `md5(uuid)`。向量里那两行 `uuidV4 → expectHexUpper` **两侧都没复刻**；
// 现在照它补一个函数，只会新长出一个没有消费者的 `md5(uuid)`。
//
// 反过来，**形态是边界**：`<32 位大写 HEX>-ENDIN` 直接进二维码的 `dID`，
// 形态错了上游就按 `dID + sID` 定位不到会话（轮询永远停在"未找到设备的二维码信息"）。
func TestDeviceFingerprintMatchesTheGoldenVectorShape(t *testing.T) {
	raw, err := golden.Load("login-crypto-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	// 用带 tag 的结构体而不是 map[string]any：字段名拼错会**编译期**就报出来，
	// 而 map 版本会静默取到零值 —— 于是断言变成"零值 == 零值"，永远绿。
	var vectors struct {
		Constants struct {
			DeviceFingerprintSuffix string `json:"DEVICE_FINGERPRINT_SUFFIX"`
		} `json:"constants"`
		Guid32 []struct {
			UUID              string `json:"uuidV4"`
			ExpectHexUpper    string `json:"expectHexUpper"`
			ExpectLen         int    `json:"expectLen"`
			DeviceFingerprint string `json:"deviceFingerprint"`
		} `json:"guid32"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}

	if got := vectors.Constants.DeviceFingerprintSuffix; got != DeviceIDSuffix {
		t.Errorf("后缀常量：向量 %q vs 代码 %q", got, DeviceIDSuffix)
	}
	if len(vectors.Guid32) == 0 {
		t.Fatal("§guid32 是空数组 —— 这样得到的「全绿」什么都没测")
	}

	for _, e := range vectors.Guid32 {
		if len(e.ExpectHexUpper) != e.ExpectLen {
			t.Errorf("%s: expectLen 说 %d，值却是 %d 位", e.UUID, e.ExpectLen, len(e.ExpectHexUpper))
		}
		// 向量的位数必须等于 Go 生成侧的位数 —— 这条把向量里的数字
		// 与 `NewDeviceID` 的实际输出**绑在一起**，而不是各自说各自的。
		if e.ExpectLen != len(NewDeviceID(&seqRng{script: []byte{0}}))-len(DeviceIDSuffix) {
			t.Errorf("%s: 向量说 %d 位，Go 生成的是 %d 位",
				e.UUID, e.ExpectLen, len(NewDeviceID(&seqRng{script: []byte{0}}))-len(DeviceIDSuffix))
		}
		if !isUpperHex(e.ExpectHexUpper) {
			t.Errorf("%s: expectHexUpper 不是全大写十六进制：%q", e.UUID, e.ExpectHexUpper)
		}
		if want := e.ExpectHexUpper + DeviceIDSuffix; e.DeviceFingerprint != want {
			t.Errorf("%s: 设备指纹应为 %q，向量写的是 %q", e.UUID, want, e.DeviceFingerprint)
		}
	}

	// Go 生成侧的实际输出：32 位大写 HEX + 后缀。
	// `TestDeviceIDMatchesTheCapturedShape` 钉的是**某个种子下的确定值**，
	// 这条钉的是**形状**（换种子也得成立）。
	id := NewDeviceID(&seqRng{script: []byte{0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89,
		0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44}})
	body, ok := strings.CutSuffix(id, DeviceIDSuffix)
	if !ok {
		t.Fatalf("设备 id 必须带 %s 后缀，实得 %q", DeviceIDSuffix, id)
	}
	if !isUpperHex(body) {
		t.Errorf("后缀前必须是全大写十六进制，实得 %q", body)
	}
	if len(body) != vectors.Guid32[0].ExpectLen {
		t.Errorf("实得 %d 位，向量说 %d 位", len(body), vectors.Guid32[0].ExpectLen)
	}
}

// isUpperHex 判断 s 是否非空且全为大写十六进制字符。
func isUpperHex(s string) bool {
	if s == "" {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !(c >= '0' && c <= '9') && !(c >= 'A' && c <= 'F') {
			return false
		}
	}
	return true
}

// TestDeviceIDIsStableAcrossReads：
// 第二次用**完全不同的随机源**，读到的必须还是同一个 id ——
// 否则"刷新二维码导致设备漂移"这个故障就复现了。
func TestDeviceIDIsStableAcrossReads(t *testing.T) {
	path := filepath.Join(t.TempDir(), "device.txt")
	first, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{1}})
	if perr != nil {
		t.Fatalf("首次: %v", perr)
	}
	second, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{200}})
	if perr != nil {
		t.Fatalf("第二次: %v", perr)
	}
	if first != second {
		t.Errorf("设备 id 必须稳定，实得 %q 与 %q", first, second)
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("文件应存在: %v", err)
	}
}

// TestEmptyOrBlankFileIsTreatedAsMissing：
// 空白文件若被当成"已有设备 id"，等于每次运行都换一个身份。
func TestEmptyOrBlankFileIsTreatedAsMissing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "device.txt")
	if err := os.WriteFile(path, []byte("   \n"), 0o600); err != nil {
		t.Fatal(err)
	}
	id, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{3}})
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if !strings.HasSuffix(id, "-ENDIN") {
		t.Errorf("实得 %q", id)
	}
	// 空白文件必须被真正重写，而不是每次都重新生成一个
	again, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{9}})
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if id != again {
		t.Errorf("写盘后应稳定，实得 %q 与 %q", id, again)
	}
}

func TestMissingParentDirectoryIsCreated(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nested", "deeper", "device.txt")
	id, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{0}})
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("父目录应被自动创建: %v", err)
	}
	if !strings.HasSuffix(id, "-ENDIN") {
		t.Errorf("实得 %q", id)
	}
}

// ⚠ **原子性本身没有断言。** 它依赖"进程写到一半退出"这个造不出来的时序；
// 反向测试实测过：把 `writeAtomically` 换成**直接写目标文件**，
// 下面这条"不留临时文件"**照样是绿的** —— 它测的是另一件事。
//
// 所以别把这条当成原子性的判据。真正能守住它的只有代码评审与上面那条
// "必须是同目录 `.tmp`"的约束。
//
// TestNoTempFileIsLeftBehind：临时文件留着会让"下一次读到的到底是哪个"
// 变成不确定的事 —— 尤其当两个进程同时启动时。
func TestNoTempFileIsLeftBehind(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "device.txt")
	if _, perr := LoadOrCreateDeviceID(path, &seqRng{script: []byte{0}}); perr != nil {
		t.Fatalf("%v", perr)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	var leftovers []string
	for _, e := range entries {
		if e.Name() != "device.txt" {
			leftovers = append(leftovers, e.Name())
		}
	}
	if len(leftovers) != 0 {
		t.Errorf("不该留下临时文件: %v", leftovers)
	}
}

// TestAppDeviceIDHasNoEndinSuffix：
// 带 `-ENDIN` 的是 PC 的形态，App 端没有 —— 混用的后果是"上游看到一台不存在的设备"。
func TestAppDeviceIDHasNoEndinSuffix(t *testing.T) {
	id := NewAppDeviceID(&seqRng{script: []byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F}})
	if id != "000102030405060708090A0B0C0D0E0F" {
		t.Errorf("实得 %q", id)
	}
	if len(id) != 32 {
		t.Errorf("App 的 deviceId 是纯 32 位 HEX，实得 %d 位", len(id))
	}
	if strings.HasSuffix(id, DeviceIDSuffix) {
		t.Errorf("带 -ENDIN 的是 PC 的形态，App 端没有，实得 %q", id)
	}
}

func TestAppDeviceShortIs12Hex(t *testing.T) {
	short := NewAppDeviceShort(&seqRng{script: []byte{0, 1, 2, 3, 4, 5}})
	if short != "000102030405" {
		t.Errorf("实得 %q", short)
	}
	if len(short) != 12 {
		t.Errorf("实得 %d 位", len(short))
	}
}

func TestAppDeviceIDIsStableAcrossReads(t *testing.T) {
	path := filepath.Join(t.TempDir(), "device-app.txt")
	first, perr := LoadOrCreateAppDeviceID(path, &seqRng{script: []byte{1}})
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	second, perr := LoadOrCreateAppDeviceID(path, &seqRng{script: []byte{200}})
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if first != second {
		t.Errorf("deviceId 必须稳定（实测跨月不变），实得 %q 与 %q", first, second)
	}
	if len(first) != 32 {
		t.Errorf("实得 %d 位", len(first))
	}
}

// TestAppDeviceShortIsNotPersisted：
// `deviceShort` 不落盘 —— 声称它是"每安装指纹"是一个已被证伪的稳定性。
func TestAppDeviceShortIsNotPersisted(t *testing.T) {
	path := filepath.Join(t.TempDir(), "device-app.txt")
	if _, perr := LoadOrCreateAppDeviceID(path, &seqRng{script: []byte{1}}); perr != nil {
		t.Fatalf("%v", perr)
	}
	// 两个不同的种子必须给出不同的 short（它每次运行都现生成）
	a := NewAppDeviceShort(&seqRng{script: []byte{1}})
	b := NewAppDeviceShort(&seqRng{script: []byte{2}})
	if a == b {
		t.Errorf("deviceShort 每次都该是新的，实得 %q 两次", a)
	}
	// 而文件里只有 deviceId 一行，没有 short 的位置
	content, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if got := len(strings.Split(strings.TrimSpace(string(content)), "\n")); got != 1 {
		t.Errorf("文件里只该有一行，实得 %d: %q", got, content)
	}
}
