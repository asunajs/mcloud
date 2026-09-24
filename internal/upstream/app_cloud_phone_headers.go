package upstream

// 红包派对（云机）族 · **两条曾被判为"造不出来"的头**：`x-DeviceInfo` 与 `x-origin`。
//
// # 为什么单独成一个文件（而不是长在 `app_cloud_phone.go` 里）
//
// `app_cloud_phone.go` 原来在 [CloudPhoneAuth.DeviceInfo] 那一格写的是"**没有 builder**"，
// 理由是"夹具里十条全被遮，拼不出来"。那句话当时是对的，但它把一件事混在了一起：
// **被遮**说的是"我们看不到实拍的值长什么样"，**不等于**"没有任何一处实现跑过它"。
// 旧 TS `cloud-phone-redpack/task.ts:57-71` 的 `getHeaders()` 就是**逐字拼这一串**的代码，
// 而且是一版跑过的实现 ⇒ 按主提示词的接口真相顺序（抓包成功样本 > 开源 Go > 旧 TS > 猜测），
// 在这一格里旧 TS 就是眼下最高一级可用出处。于是这一条从"猜不出来"变成"有出处、仍待实拍"。
//
// ⚠ 纪律不变：这里是**零实拍**。下面每个函数钉的都是"与旧 TS 那一段字面量一致"，
// 不是"上游认这一串"。真打时若这一族整片回非 200，这两枚头与 `sign` 的段序
// （[CloudPhoneSign]）是头两个要复核的方向 —— 而**只有**抓到一条未脱敏的同接口请求才能定论。
//
// # 与 [AppDevice.DeviceInfo] 不是一回事，别"顺手统一"
//
// 仓库里另一族（原生 App 那套）的设备串是**另一份形状**：14 段、以 `|` 分隔、含 `375 * 667`
// 与机型明文。本文件这一串是 **H5 WebView 里那台 iPad** 的形状（12 段、含 `375X667`），
// 两者段数与分隔都不同 ⇒ 搬过来不会有任何本地断言变红，只会让上游看到一台不存在的设备形态。

import (
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// CloudPhoneDeviceInfoUA 是 `x-DeviceInfo` **第 6 段**里那枚 UA。
//
// ⚠ 它与本族的请求头 [CloudPhoneUserAgent] **不是同一个串**（旧 TS 那两份就是不同的字面量）：
// 这里少了一段 `like Mac OS X`，且 `CPU OS 18_5` 后面直接接 `)`。
// 于是"把 UA 变量抽出来共用"是**错的方向** —— 表现是本族发出去的两个 UA 突然一致，
// 而真实客户端那两个从来就不一致。这一条由测试钉在旧 TS 的字面量上。
const CloudPhoneDeviceInfoUA = "Mozilla/5.0 (iPad; CPU OS 18_5) AppleWebKit/605.1.15 " +
	"(KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"

// CloudPhoneDeviceInfoSegments 是这一串的**段数**（按 `|` 分隔，含空段）。
//
// 旧 TS 那一行拆出来是 13 段（首段为空 ⇒ 串以 `|` 开头；末尾 `zh||` 留两个空段）。
// 列出来的用处不是"校验"，而是让人一眼看见**空段是有意的**：
// 删掉任何一枚空段都会把后面每一段往前挪一位，而**没有本地断言会红**（除非钉住整串）。
const CloudPhoneDeviceInfoSegments = 13

// CloudPhoneDeviceInfoRandomLen 是第 8 段里那枚随机尾巴的长度。
//
// 出处：`Math.random().toString(36).substring(2, 18)` ⇒ 最多 16 个 base36 字符。
// ⚠ "最多"是 JS 那边的性质（尾巴可能不足 16 位），本函数按**恰好 16** 生成 ——
// 这是与本族 [CloudPhoneRequestIDSuffixLen] 同一类的"形状对得上即可"的选择：
// 它既不是判据也不是凭据，上游据此区分什么**未测试**。
const CloudPhoneDeviceInfoRandomLen = 16

// CloudPhoneDeviceInfoSuffix 取第 8 段随机尾巴的字符源（base36，与
// [CloudPhoneRequestIDSuffix] 同一字符集，但**长度不同**，别互相顶替）。
func CloudPhoneDeviceInfoSuffix(rng core.Random) string {
	const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
	buf := make([]byte, CloudPhoneDeviceInfoRandomLen)
	rng.FillBytes(buf)
	for i, b := range buf {
		buf[i] = alphabet[int(b)%len(alphabet)]
	}
	return string(buf)
}

// CloudPhoneDeviceInfo 拼 `x-DeviceInfo` 那一整串。
//
// 逐字对照旧 TS `task.ts:66`：
//
//	|h5|1.0.0|v1.0.0||<UA>|<timestamp><randomStr>|||375X667|zh||
//
// 末段是**两个**空段（`zh||` 结尾），少一个就不是同一串 —— 所以这里用 slice 把每一格
// 摆出来而不是拼字面量：挪动任何一格都会红。
//
// 两个输入都不是本层能生成的：
//   - `timestamp` 必须与**同一趟**的 `timestamp` 头**逐字相同**（旧 TS 用的是同一个
//     `Date.now()` 读数）⇒ 由调用方传进来，不在这里再取一次时钟。
//     ⚠ 两处读数不一致会不会被判成"设备串与请求不匹配"⇒ **未测试**。
//   - `suffix` 是随机尾巴（[CloudPhoneDeviceInfoSuffix]）⇒ 同样由调用方给，
//     为的是让纯层不碰随机源（本函数可拿固定输入做字节级断言）。
//
// ⚠ `timestamp` 与 `suffix` 是**直接相邻、没有分隔符**的（`|${timestamp}${randomStr}|`），
// 中间加一个 `|` 或空格都会让第 8 段变成两截。
func CloudPhoneDeviceInfo(timestamp, suffix string) string {
	return strings.Join([]string{
		"", "h5", "1.0.0", "v1.0.0", "", CloudPhoneDeviceInfoUA,
		timestamp + suffix, "", "", "375X667", "zh", "", "",
	}, "|")
}

// CloudPhoneXOriginHomeURL 是 `x-origin` 头的值（**不带 token** 的那一版）。
//
// 出处：旧 TS `task.ts:67-68` 那枚字面量 ——
// `https://cpactiv.buy.139.com/#/redEnvelopeParty/home?loginType=0&enableShare=1&channelSrc=red-mcloudapp-001`。
//
// 它比 [CloudPhoneActivityHomeURL] 多三个查询键，而**顺序**也照抄（`loginType` → `enableShare` →
// `channelSrc`）。⚠ 为什么把顺序当回事：这一条零实拍，能追到出处的只有那段字面量，
// 按字节可 diff 的写法优先；乱序的代价今天看不见，等真出问题时就再也对不回出处。
const CloudPhoneXOriginHomeURL = CloudPhoneOrigin +
	"/#/redEnvelopeParty/home?loginType=0&enableShare=1&channelSrc=" + CloudPhoneChannelSrc

// CloudPhoneRefererHomeURL 是 `Referer` 头的值（旧 TS `task.ts:69`：Origin 根路径，
// **不带** hash 路由那一段）。
//
// ⚠ 它与 [CloudPhoneXOriginHomeURL] **刻意不同**：一个是根、一个是带路由的完整地址。
// "顺手让 referer 也指向首页"会让两枚头变成同一串，而真实客户端不是。
// 实拍十条的 `Referer` 全被遮 ⇒ "这一族每条都带根路径"这句**未核实**，
// 所以 [CloudPhoneRawRequest] 仍把 referer 留成**参数**，这里只是把候选值成文。
const CloudPhoneRefererHomeURL = CloudPhoneOrigin + "/"

// CloudPhoneTimestampMilli 把毫秒读数念成头里那串十进制。
//
// 存在的理由：[CloudPhoneAuth.Timestamp] 定成 string 是**刻意的**（单位未核实），
// 而旧 TS 那一格是 `Date.now().toString()` ⇒ **毫秒**。这里不替调用方决定用哪个时钟，
// 只把"整数毫秒 → 十进制串"这一步收在一处，免得每个调用点各写一遍 `Sprintf` 而格式漂移。
func CloudPhoneTimestampMilli(unixMilli int64) string {
	return fmt.Sprintf("%d", unixMilli)
}
