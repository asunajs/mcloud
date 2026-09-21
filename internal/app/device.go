package app

// 设备指纹的**本地持久化**。
//
// # 为什么是文件，不是数据库
//
// 扫码登录的第一件事就是把 `dID` 拼进二维码 —— 那一刻数据库**可能还没建**
// （全新安装、甚至还没决定库放哪）。而 `dID` 一旦漂移，服务端就按 `dID + sID`
// 定位不到会话，实测表现是轮询一直停在 `200059541 未找到设备的二维码信息`：
// 二维码扫了、手机上点了确认，这边却毫无反应 —— 这是最难查的一类故障。
//
// 所以它必须是最早存在、且永不改变的那一份本地状态，单独一个文件最合适。
//
// # 形状的证据
//
// 客户端 `apiServer.js: getClientDeviceInfo()`：原生接口拿不到时
// `windowMachine = Utils.getGUID32() + '-ENDIN'`，而 `getGUID32()` 是
// `MD5(随机 UUID)` 的大写十六进制。
//
// 也就是说这是"按安装随机生成"的值 —— **不需要真实硬件信息**，
// 我们自己生成完全成立（见 `docs/client-profiles.md` §5.1）。

import (
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// DeviceIDSuffix 是设备 id 的后缀。客户端实测值形如 `<32 位大写 HEX>-ENDIN`。
const DeviceIDSuffix = "-ENDIN"

// NewDeviceID 生成一个新的设备 id：32 位大写十六进制 + `-ENDIN`。
func NewDeviceID(rng core.Random) string {
	var bytes [16]byte
	rng.FillBytes(bytes[:])
	return strings.ToUpper(hex.EncodeToString(bytes[:])) + DeviceIDSuffix
}

// LoadOrCreateDeviceID 读设备 id；文件不存在就生成一个并写盘。
//
// 写入是**先写临时文件再改名**：直接写目标文件时，如果进程在写到一半时退出，
// 留下的是一个被截断的 id —— 而它下次会被当成"已经存在的设备 id"读进去，
// 于是这台机器的身份永远变了（服务端再也定位不到会话）。
func LoadOrCreateDeviceID(path string, rng core.Random) (string, *core.Error) {
	if existing, ok := readTrimmed(path); ok {
		return existing, nil
	}
	deviceID := NewDeviceID(rng)
	if perr := writeAtomically(path, deviceID); perr != nil {
		return "", perr
	}
	return deviceID, nil
}

// readTrimmed 读文件里的值；不存在或只有空白都算"没有"。
//
// 空白也算没有：一个被截断/清空的文件若被当成"已有设备 id"，
// 就等于每次运行都换一个身份（见 [LoadOrCreateDeviceID] 的说明）。
func readTrimmed(path string) (string, bool) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", false
	}
	trimmed := strings.TrimSpace(string(raw))
	if trimmed == "" {
		return "", false
	}
	return trimmed, true
}

// writeAtomically 原子落盘：先写 `<path>.tmp` 再改名。
//
// ⚠ 临时文件必须是 `path + ".tmp"`，即**与目标同一个目录**：`rename` 只有在
// 同一文件系统内才是原子的；写到 `os.TempDir()` 再搬过来，跨设备时会退化成
// "复制 + 删除"，中间态与直接写目标文件没有区别。
//
// ⚠ **原子性本身在单测里测不出来**（见 `device_test.go` 里那条说明）：
// 它依赖"进程在写到一半时退出"这个我们造不出来的时序。能测的只有两条
// 后果 —— 不留临时文件、以及失败时不破坏已有文件。别把前者当成原子性的判据。
func writeAtomically(path, value string) *core.Error {
	if parent := filepath.Dir(path); parent != "" && parent != "." {
		if err := os.MkdirAll(parent, 0o755); err != nil {
			return core.NewInternal(fmt.Sprintf("创建设备指纹目录失败 %s: %v", parent, err))
		}
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, []byte(value+"\n"), 0o600); err != nil {
		return core.NewInternal(fmt.Sprintf("写设备指纹失败 %s: %v", tmp, err))
	}
	if err := os.Rename(tmp, path); err != nil {
		return core.NewInternal(fmt.Sprintf("落盘设备指纹失败 %s: %v", path, err))
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// App 端（iOS）的设备指纹
// ─────────────────────────────────────────────────────────────────────────────

// NewAppDeviceID 生成 App 端的 `deviceId`：32 位大写 HEX，**没有** `-ENDIN` 后缀。
//
// 与 PC 的 [NewDeviceID] 只差那个后缀，但设备串第 6 段是整串比对的一部分，
// 带上后缀会让上游看到一台不存在的设备形态。
func NewAppDeviceID(rng core.Random) string {
	var bytes [16]byte
	rng.FillBytes(bytes[:])
	return strings.ToUpper(hex.EncodeToString(bytes[:]))
}

// NewAppDeviceShort 生成 App 端的 `deviceShort`：12 位大写 HEX（设备串第 7 段）。
//
// # 它是**每次运行**生成的，不持久化（实测依据）
//
// 同一台设备（`deviceId` 完全相同）在两个时间点的 `deviceShort` **不一样**：
//
// | 观测时间 | `deviceId` | `deviceShort` |
// | --- | --- | --- |
// | 2026-08-12 | `67F4C56A…315C` | `8A708867DBC3` |
// | 2026-09-18 | `67F4C56A…315C` | `EAB9BF94B8DE` |
//
// 也就是说 **`deviceId` 稳定、`deviceShort` 会变** —— 所以后者**不能**当"每安装指纹"
// 持久化（那会声称一个已被证伪的稳定性）。它随每次运行新生成。
//
// **未确定的部分**：它到底按什么变（每次启动 / 每天 / 某个事件）只有两个观测点，
// 定不下来。记录在 `docs/client-profiles.md` §5.4 的待确认项里。
func NewAppDeviceShort(rng core.Random) string {
	var bytes [6]byte
	rng.FillBytes(bytes[:])
	return strings.ToUpper(hex.EncodeToString(bytes[:]))
}

// LoadOrCreateAppDeviceID 读 App 端 `deviceId`；文件不存在就生成一个并写盘。
//
// **只持久化 `deviceId`**，`deviceShort` 每次运行用 [NewAppDeviceShort] 现生成
// （理由见该函数的文档）。
func LoadOrCreateAppDeviceID(path string, rng core.Random) (string, *core.Error) {
	if existing, ok := readTrimmed(path); ok {
		return existing, nil
	}
	deviceID := NewAppDeviceID(rng)
	if perr := writeAtomically(path, deviceID); perr != nil {
		return "", perr
	}
	return deviceID, nil
}
