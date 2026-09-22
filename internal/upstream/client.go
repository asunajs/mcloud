// 端档案（**雏形**）：请求头里"我们是哪个客户端、哪台设备"。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-upstream/src/client.rs。
//
// 完整的多端档案设计见 docs/client-profiles.md；这里先落地 PC 端
// **登录链路必需**的那部分 —— 等接第二端时再按那份文档长成完整形态。
//
// # 为什么必须有它（一次实测换来的）
//
// 同一个 `/user/thirdlogin` 请求：
//
// | 请求头 | 上游响应 |
// | --- | --- |
// | 只带 `x-yun-tid` | `{"code":"9999","message":"登录失败！"}` |
// | 加上下面的设备头 | `{"code":"200059541","message":"未找到设备的二维码信息"}` |
//
// 后者才是文档里"码还没被扫"的正常状态。所以这几个头**不是装饰**：
// 少了它们，扫码登录连第一步都进不去，而错误码（9999）还不是文档里任何一个。
//
// # 设备信息串的形状
//
// 客户端 `apiServer.js: getClientDeviceInfo()` 在**拿不到原生能力时**用的兜底模板：
//
// ```js
// `||11|${appVersion}|PC|QkYtMjAyMDAzMTAxNjQ3|${windowMachine}|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||`
// ```
//
// 我们正是"没有原生能力"的那种客户端，所以**照抄这个模板**是最忠实的选择：
// 它是客户端源码里的公开常量，不是我们编的。14 段的字段表见
// docs/client-profiles-fingerprint-and-landing.md §5.2。
package upstream

import "fmt"

// 设备信息串的各段常量（兜底模板里的值）。
const (
	// deviceInfoEnv 是环境枚举段（第 2 段）。
	deviceInfoEnv = "11"
	// deviceInfoModelB64 是设备型号码（第 5 段）：base64 的 `BF-202003101647`。
	deviceInfoModelB64 = "QkYtMjAyMDAzMTAxNjQ3"
	// deviceInfoLocaleB64 是语言（第 10 段）：base64 的 `Chinese (Simplified)`。
	deviceInfoLocaleB64 = "Q2hpbmVzZSAoU2ltcGxpZmllZCk="
	// deviceInfoOS 是 OS 描述（第 8 段）。**前导空格是原样**，别顺手 trim 掉。
	deviceInfoOS = " Windows 10 (10.0)"
	// deviceInfoResolution 是分辨率（第 9 段）。
	deviceInfoResolution = "1920X1040"

	// mmSourceFallback 是市场来源缺失时的兜底值（客户端源码：`x-MM-Source: "000"`）。
	mmSourceFallback = "000"
	// huaweiChannelSrc 是华为渠道号（客户端源码在 pre-login 分支里硬编码）。
	huaweiChannelSrc = "10200153"
)

// DeviceInfo 拼设备信息串（14 段，`|` 分隔）。
//
// # 关于平台差异
//
// `clientType` / `cpid` / 版本号是**按平台**变的（见 [PcPlatform]），
// 但这条兜底模板只有 Windows 版有实测依据。macOS 分支暂时复用同一串常量：
// 它们本身是客户端源码里的公开值（不是编的），只是"mac 客户端到底发什么"
// **尚无实测** —— 这一点记在 docs/client-profiles-fingerprint-and-landing.md 的未知项（§7）里，
// 拿到 macOS 流量后要回来替换。
func DeviceInfo(platform PcPlatform, deviceID string) string {
	return fmt.Sprintf(
		"||%s|%s|PC|%s|%s||%s|%s|%s|||",
		deviceInfoEnv, platform.AppVersion(), deviceInfoModelB64,
		deviceID, deviceInfoOS, deviceInfoResolution, deviceInfoLocaleB64,
	)
}

// ClientProfile 是客户端档案：一次登录里不变的部分。
//
// 它把"我们是哪个端、哪台设备"收成一个值，于是每个请求构造点只需要
// `profile.Headers()`，不会有人漏掉某个头 —— 而漏一个头的代价是 9999。
type ClientProfile struct {
	platform   PcPlatform
	deviceID   string
	deviceInfo string
}

// NewClientProfile 构造档案（设备信息串只算一次）。
func NewClientProfile(platform PcPlatform, deviceID string) *ClientProfile {
	return &ClientProfile{
		platform:   platform,
		deviceID:   deviceID,
		deviceInfo: DeviceInfo(platform, deviceID),
	}
}

// Platform 返回平台形态。
func (p *ClientProfile) Platform() PcPlatform { return p.platform }

// DeviceID 返回设备指纹（`windowMachine`）。它同时嵌在设备信息串里。
func (p *ClientProfile) DeviceID() string { return p.deviceID }

// DeviceInfo 返回设备信息串原文。
func (p *ClientProfile) DeviceInfo() string { return p.deviceInfo }

// Headers 返回登录链路要带的头
// （`Content-Type` 与每次新的 `x-yun-tid` 由请求构造补上）。
//
// 逐个对应客户端 `common/utils.js: sendRequest` 的 **pre-login 分支**
// （登录时还没有 `UserInfo`，走的正是这一支）：
//
// ```js
// xhr.setRequestHeader("x-yun-client-info", windowDeviceInfo);
// xhr.setRequestHeader("x-DeviceInfo",      windowDeviceInfo);  // 同一个值、两个头
// xhr.setRequestHeader("x-MM-Source",       appSource || "000");
// xhr.setRequestHeader("x-huawei-channelSrc","10200153");
// xhr.setRequestHeader("x-UserAgent",       "PC");
// ```
func (p *ClientProfile) Headers() [][2]string {
	return [][2]string{
		// 两个头同一个值：不是笔误，客户端就是这么发的（大小写不同）
		{"x-yun-client-info", p.deviceInfo},
		{"x-DeviceInfo", p.deviceInfo},
		{"x-MM-Source", mmSourceFallback},
		{"x-huawei-channelSrc", huaweiChannelSrc},
		{"x-UserAgent", "PC"},
	}
}
