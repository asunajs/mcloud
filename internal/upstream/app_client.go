// App 端（手机客户端）档案：iOS App 的请求头与设备串。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-upstream/src/app_client.rs。
//
// # 与 PC 端的差别不是"多几个头"，而是**另一套设备串语法**
//
// PC 端（[ClientProfile]）的设备串是 14 段、第 5 段是 `PC`、
// 第 6 段是 `QkYtMjAyMDAzMTAxNjQ3`（base64 的机型）；
// App 端也是 14 段、第 5 段是 `APPLE`、第 6 段是 `iPad8,6`（机型**明文**），
// 而它的 `x-yun-client-info` 在设备串之后再追加两段（`x-DeviceName` 的 base64、`0`）
// 并以 `;` 收尾 —— 所以那个头是 **16 段**。
//
// 两套串的字段表见 docs/client-profiles.md；本文件只实现**已抓到的 iOS 形态**。
//
// # 为什么没有 `Android`
//
// `clientType` / `cpid` / 设备串语法在 Android 上是另一组值，**没有实测**。
// 按项目铁律（docs/rust-rewrite-plan.md §7.0：抓包前不写猜测性接口代码），
// 这里不预先造一个 `Android` 变体 ——
// 那只会让"传错端"变成一个可以静默通过的选项。
//
// # 实测来源
//
// docs/capture/app-sms-login-samples.txt（FXTrace 网络日志，2026-09-18 22:54-22:56）。
package upstream

import (
	"fmt"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// iOS App 的固定业务参数。
//
// 这几个值是**按端**变的（见 docs/client-profiles.md 的端对照表），
// 混用会被上游当成另一个客户端。做成常量而不是散在构造点，
// 是为了让"改版本号"只发生在一处。
//
// ⚠ Go 没有嵌套模块，所以这组常量统一带 `IOS` 前缀 ——
// 前缀不是装饰：`IOSClientType` 与 [ClientProfile] 的 `clientType`
// 是**两个端的值**，混用的后果是上游认成另一个客户端。
const (
	// IOSChannel 是 `x-yun-app-channel` 与 `x-huawei-channelSrc` 的实测值。
	IOSChannel = "10000020"
	// IOSClientType 是请求体 `clienttype`。
	IOSClientType = "412"
	// IOSCpid 是请求体 `cpid`。
	IOSCpid = "55"
	// IOSAppVersion 是请求体 `version` 与设备串第 4 段。
	IOSAppVersion = "12.6.0"
	// IOSUABuild 是 `x-UserAgent` 的第 4 段（版本 + 构建号）。
	IOSUABuild = "12.6.0-200"
	// IOSUAProduct 是 `User-Agent` 的产品段。
	IOSUAProduct = "mCloud_iPhone/12.6.0"

	// IOSServiceIdentifier 是 `service_identifier_key` 的实测值。
	//
	// PC 端**没有**这个头；它是 App 端用来把请求路由到用户域的关键，
	// 少了它 `/user/thirdlogin` 会落到别的服务上。
	//
	// ⚠⚠ **它是按服务变的，不是一个全局常量**：同一份日志里出现过 4 个取值
	// （`service_identifier_caixun` / `_cloudDisk` / `_recommend` / `_userDomain`）。
	// 这里给的是**登录链路**要用的那个；其它接口各自的值在接那些接口时再逐个确认。
	//
	// ⇒ 判据同 [core] 那条"跨端常量不许互搬"：**这个头能在同一端的两个接口上
	// 取不同值**，所以它不能钉成"App 端常量"，只能钉成"这个接口的值"。
	IOSServiceIdentifier = "service_identifier_userDomain"

	// IOSAuthPrefix 是 `Authorization` 头的第一段
	// （`Basic base64("<prefix>:<account>:<authToken>")`）。
	//
	// **PC 端是 `pc`，App 端是 `mobile`** —— 这是两端登录态最容易被忽略的差异：
	// 两端的 `authToken` 形状完全一样（`前缀|1|RCS|13 位毫秒|载荷`），
	// 只有这一段的字面量不同。
	//
	// 证据（docs/capture/app-sms-login-samples.txt §3.5）：App 自身日志里
	// 1076 处 `Authorization`，解码后全部是 `mobile:<手机号>:<authToken>`。
	IOSAuthPrefix = "mobile"
)

// AnonymousAuthorization 是未登录时 App 发的 `Authorization` 占位值。
//
// 实测原样是 `Basic base64("mobile:(null):(null)")` ——
// 字面量 `(null)`（不是 JSON `null`、不是空串）。
// 也就是说 App **即使没登录也会带这个头**，只是三段都是占位。
//
// 记下来是为了两件事：① 解析上游日志时认得它，别当成一个坏 token；
// ② 将来若要逐字节复刻 App 的未登录请求，这里就是那个值。
const AnonymousAuthorization = "Basic bW9iaWxlOihudWxsKToobnVsbCk="

// AppDevice 是 App 端设备信息（一次登录里不变的部分）。
//
// # 为什么把 `OSVersion` 拆出来
//
// 同一个系统版本在三个地方有**三种写法**，实测原样如下：
//
//	`x-yun-client-info` / `x-DeviceInfo` 第 9 段 → `ipados 26.6`（小写、iPad 上是 `ipados`）
//	`x-UserAgent` / `x-yun-user-agent`             → `ios 26.6`
//	`User-Agent` 括号内                            → `iOS 26.6`
//
// 统一成一个字段再按位置拼，是为了让"升系统版本"只改一个字段；
// 但它**不是**可以随便归一化的 —— 三个写法的差异是上游按正则解析的，
// 所以这里保留三段拼接函数而不是把它们塞进一个模板常量。
type AppDevice struct {
	// DeviceID 是设备 UUID：32 位**大写** hex。对应 PC 端的 `windowMachine`。
	deviceID string
	// DeviceShort 是设备短标识：12 位大写 hex。服务端按它 + `deviceID` 定位。
	//
	// # 它**不是**每安装稳定值（实测）
	//
	// 同一台设备（`deviceID` 完全相同）在两个时间点的这一段的取值**不一样**：
	//
	//	2026-08-12  `67F4C56A…315C` → `8A708867DBC3`
	//	2026-09-18  `67F4C56A…315C` → `EAB9BF94B8DE`
	//
	// 所以它**不能**和 `deviceID` 一起当"每安装指纹"持久化。
	// 到底按什么变（每次启动 / 每天 / 某个事件）只有两个观测点，定不下来 ——
	// 实现侧按"每次运行现生成"处理（见 `mcloud_app::device::new_app_device_short`）。
	deviceShort string
	// deviceNameB64 是 `x-DeviceName` 的 base64（实测 `aVBhZA==` = `iPad`）。
	deviceNameB64 string
	// Model 是机型，实测 `iPad8,6`（**明文**，PC 端那一段是 base64）。
	model string
	// OSVersion 是系统版本号，实测 `26.6`。
	osVersion string
	// Screen 是分辨率段，实测 `375 * 667`（**带空格**，别 trim）。
	screen string
	// Locale 是语言段，实测 `zh-Hans-CN`。
	locale string
}

// NewIPadAppDevice 用实测形状的默认值构造，只要求设备指纹。
//
// 默认值全部来自抓包原值（脱敏后），不是编的：
// `iPad8,6` / `ipados 26.6` / `375 * 667` / `zh-Hans-CN` / `aVBhZA==`。
func NewIPadAppDevice(deviceID, deviceShort string) AppDevice {
	return AppDevice{
		deviceID:      deviceID,
		deviceShort:   deviceShort,
		deviceNameB64: "aVBhZA==",
		model:         "iPad8,6",
		osVersion:     "26.6",
		screen:        "375 * 667",
		locale:        "zh-Hans-CN",
	}
}

// DeviceID 返回设备指纹。
func (d AppDevice) DeviceID() string { return d.deviceID }

// DeviceShort 返回设备短标识。
func (d AppDevice) DeviceShort() string { return d.deviceShort }

// WithModel 覆盖机型（换设备形态时用）。
func (d AppDevice) WithModel(model string) AppDevice { d.model = model; return d }

// WithOSVersion 覆盖系统版本。
func (d AppDevice) WithOSVersion(version string) AppDevice { d.osVersion = version; return d }

// WithDeviceNameB64 覆盖 `x-DeviceName` 的 base64。
func (d AppDevice) WithDeviceNameB64(nameB64 string) AppDevice {
	d.deviceNameB64 = nameB64
	return d
}

// DeviceInfo 是 `x-DeviceInfo`：**14 段**（末段为空，因为原串以 `|` 结尾）。
//
// 实测原串（脱敏）：
//
//	1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|<uuid>|<short>|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|
func (d AppDevice) DeviceInfo() string {
	return fmt.Sprintf("1|%s|2|%s|APPLE|%s|%s|%s|ipados %s|%s|%s|0.0000|0.0000|",
		defaultIP, IOSAppVersion, d.model, d.deviceID, d.deviceShort,
		d.osVersion, d.screen, d.locale)
}

// ClientInfo 是 `x-yun-client-info`：在 [AppDevice.DeviceInfo] 之后**追加**两段
// （`x-DeviceName` 的 base64、`0`），并以 `;` 收尾 —— 合计 16 段。
//
// ⚠ `x-DeviceInfo` 与 `x-yun-client-info` **不是同一个值**
// （PC 端那两个头才是同值双名，App 端不是 —— 这是两端最容易抄错的一处）。
func (d AppDevice) ClientInfo() string {
	return fmt.Sprintf("%s%s|0|;", d.DeviceInfo(), d.deviceNameB64)
}

// DeviceName 返回 `x-DeviceName`。
func (d AppDevice) DeviceName() string { return d.deviceNameB64 }

// XUserAgent 是 `x-UserAgent`：`412|iPad|ios 26.6|12.6.0-200`。
func (d AppDevice) XUserAgent() string {
	return fmt.Sprintf("%s|iPad|ios %s|%s", IOSClientType, d.osVersion, IOSUABuild)
}

// YunUserAgent 是 `x-yun-user-agent`：`iPad|iPad|ios 26.6|mCloud12.6.0-200`。
func (d AppDevice) YunUserAgent() string {
	return fmt.Sprintf("iPad|iPad|ios %s|mCloud%s", d.osVersion, IOSUABuild)
}

// UserAgent 是 `User-Agent`：`mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)`。
func (d AppDevice) UserAgent() string {
	return fmt.Sprintf("%s (iPad; iOS %s; Scale/2.00)", IOSUAProduct, d.osVersion)
}

// defaultIP 是设备串第 2 段上报的 IP。
//
// 它是**客户端自己报的局域网地址**，服务端不据此鉴权（实测换网后同一账号
// 只这一段的取值变化，登录不受影响），所以做成常量而不是必须注入的字段。
const defaultIP = "192.168.3.179"

// AppProfile 是 App 端档案：一次登录里不变的部分。
type AppProfile struct {
	device AppDevice
	// userDomainID 是 `x-yun-uni`。登录前就已经带上（实测值 = 响应里的 `userDomainId`），
	// 但首次登录时调用方**没有**这个值 —— 所以它是可选的，
	// 而不是"随便填一个"（填错会让上游定位到别人的用户域）。
	userDomainID *string
}

// NewAppProfile 用设备构造档案（**未登录**形态：不带 `x-yun-uni`）。
func NewAppProfile(device AppDevice) *AppProfile { return &AppProfile{device: device} }

// AppAuthorization 构造 App 的 `Authorization` 头：
// `Basic base64("mobile:<account>:<authToken>")`。
//
// # 为什么是**包级函数**而不是 [AppProfile] 的方法
//
// 这个头只依赖凭据，不依赖设备；挂在 `AppProfile` 上会暗示
// "换个 profile 就换个头"，而实际上不是。
//
// 与 PC 的 [ClientProfile] 只有第一段不同（`mobile` vs `pc`），
// 但那一段错了服务端就认不出这是谁 —— 所以由本函数统一构造，
// 调用点不自己拼字符串。
func AppAuthorization(account, authToken string) string {
	payload := fmt.Sprintf("%s:%s:%s", IOSAuthPrefix, account, authToken)
	return "Basic " + core.B64Encode([]byte(payload))
}

// WithUserDomainID 带上 `x-yun-uni`（登录后 / 已登录态复用会话时）。
func (p *AppProfile) WithUserDomainID(id string) *AppProfile {
	p.userDomainID = &id
	return p
}

// Device 返回设备信息。
func (p *AppProfile) Device() AppDevice { return p.device }

// UserDomainID 返回 `x-yun-uni`（nil = 尚未登录，头里不带）。
func (p *AppProfile) UserDomainID() *string { return p.userDomainID }

// Headers 返回 App 端请求要带的头。
//
// 逐个对应抓包实测的头集合（docs/capture/app-sms-login-samples.txt §3.2）。
// 每次请求会变的头（`x-yun-user-timestamp`）由调用方注入 ——
// 这样请求构造函数仍是**纯函数**、可逐字段断言。
func (p *AppProfile) Headers(timestamp int64) []core.HeaderPair {
	device := p.device
	headers := []core.HeaderPair{
		{"Pragma", "no-cache"},
		{"x-yun-user-timestamp", strconv.FormatInt(timestamp, 10)},
		{"x-yun-app-channel", IOSChannel},
		{"x-yun-net-type", "1"},
		{"x-yun-svc-type", "1"},
		{"x-huawei-channelSrc", IOSChannel},
		{"x-yun-api-version", "v1"},
		{"x-UserAgent", device.XUserAgent()},
		{"x-yun-client-info", device.ClientInfo()},
		{"Accept-Language", "zh-CN"},
		{"service_identifier_key", IOSServiceIdentifier},
		{"User-Agent", device.UserAgent()},
		{"x-DeviceName", device.DeviceName()},
		{"x-DeviceInfo", device.DeviceInfo()},
		{"x-yun-user-agent", device.YunUserAgent()},
		{"x-SvcType", "1"},
	}
	if p.userDomainID != nil {
		headers = append(headers, core.HeaderPair{"x-yun-uni", *p.userDomainID})
	}
	return headers
}
