package upstream

import "testing"

func TestDeviceInfoReproducesTheClientFallbackTemplate(t *testing.T) {
	info := DeviceInfo(PlatformWindows, "DEV-1")
	// 逐段对齐源码模板，尤其注意第 0/1 段是空的（前导 `||`）、尾部是 `|||`
	const want = "||11|8.8.6.20260829|PC|QkYtMjAyMDAzMTAxNjQ3|DEV-1|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||"
	if info != want {
		t.Fatalf("实得 %q", info)
	}

	segments := split(info, '|')
	if len(segments) != 14 {
		t.Errorf("字段表是 14 段，实得 %d（段数变了说明模板抄错了）", len(segments))
	}
	if segments[0] != "" || segments[1] != "" {
		t.Errorf("前两段应是空串：%q %q", segments[0], segments[1])
	}
	if segments[2] != "11" {
		t.Errorf("环境段实得 %q", segments[2])
	}
	if segments[4] != "PC" {
		t.Errorf("第 4 段实得 %q", segments[4])
	}
	if segments[6] != "DEV-1" {
		t.Errorf("第 6 段应是设备指纹（明文），实得 %q", segments[6])
	}
	if segments[8] != " Windows 10 (10.0)" {
		t.Errorf("OS 段的前导空格是原样，实得 %q", segments[8])
	}
	if segments[13] != "" {
		t.Errorf("末段应为空，实得 %q", segments[13])
	}
}

// TestDeviceInfoFollowsThePlatformVersion 版本号在设备串里，
// 所以它必须跟着平台走（mac 的 clientType 配 win 的版本 = 自相矛盾）。
func TestDeviceInfoFollowsThePlatformVersion(t *testing.T) {
	mac := DeviceInfo(PlatformMacOS, "D")
	if !contains(mac, "|4.2.1|PC|") {
		t.Errorf("macOS 版本应出现在设备串里：%s", mac)
	}
	if !contains(DeviceInfo(PlatformWindows, "D"), "|8.8.6.20260829|PC|") {
		t.Error("Windows 版本应出现在设备串里")
	}
	if contains(mac, "8.8.6") {
		t.Errorf("macOS 串里不该出现 Windows 版本号：%s", mac)
	}
}

func TestHeadersCarryBothDeviceInfoNamesAndTheChannel(t *testing.T) {
	profile := NewClientProfile(PlatformWindows, "DEV-1")
	get := func(name string) string {
		for _, h := range profile.Headers() {
			if h[0] == name {
				return h[1]
			}
		}
		t.Fatalf("缺少头 %s", name)
		return ""
	}

	// 客户端发的是两个不同大小写的头、同一个值
	if get("x-yun-client-info") != get("x-DeviceInfo") {
		t.Error("两个设备头应是同一个值")
	}
	if get("x-yun-client-info") != profile.DeviceInfo() {
		t.Error("设备头的值应等于设备信息串")
	}
	if got := get("x-MM-Source"); got != "000" {
		t.Errorf("x-MM-Source 实得 %q", got)
	}
	if got := get("x-huawei-channelSrc"); got != "10200153" {
		t.Errorf("x-huawei-channelSrc 实得 %q", got)
	}
	if got := get("x-UserAgent"); got != "PC" {
		t.Errorf("x-UserAgent 实得 %q", got)
	}
	// 设备指纹必须在设备串里（服务端按 dID + sID 定位会话）
	if !contains(get("x-yun-client-info"), profile.DeviceID()) {
		t.Error("设备串里应含设备指纹")
	}
}

func split(s string, sep byte) []string {
	out := []string{}
	start := 0
	for i := 0; i < len(s); i++ {
		if s[i] == sep {
			out = append(out, s[start:i])
			start = i + 1
		}
	}
	return append(out, s[start:])
}
