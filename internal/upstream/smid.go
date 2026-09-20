// smid 设备指纹 —— 请求头 `deviceid` 是怎么来的。
//
// 对应 Rust 侧 legacy/crates/mcloud-upstream/src/smid.rs。
//
// # 它不是"我们本地算出来的一个串"
//
// `deviceid` 是**中国移动统一认证（cmpassport）的风控服务发下来的**：
// 客户端把一份"浏览器 / WebView 特征"加密上报，服务端回一个 `deviceId`，
// 客户端在它前面**拼一个字母 `B`** 当作自己的设备 id 用。
//
//	uid = randomUUID()
//	  ├─ ep     = RSA(uid)                 公钥见 core.SklandPublicKeyB64
//	  └─ priId  = md5(uid)[0..16]          下面那份特征的 AES 密钥
//	特征体（约 30 个字段，含一个当场随机出来的 smid）
//	  → tn = md5(GetTn(特征体))             ← 先算摘要，再写回 tn 字段
//	  → 按 DES 规则逐字段加密 + 改名（appId→um、ua→pu、…）
//	  → JSON → gzip（OS 字节写 19）→ base64
//	  → AES-CBC/PKCS7 固定 IV，密钥 priId → hex      = data
//	POST https://slw.h5cmpassport.com:9090/deviceprofile/v4
//	  { appId, organization, ep, data, os:"web", encode:5, compress:2 }
//	  → code == 1100 → **deviceId = "B" + detail.deviceId**
//
// 来源：旧项目 `packages/shared/src/smid/index.ts`（`getSmId` / `getTn` /
// `encryptDes` / `encryptObjectByDesRules` / `getSmData` / `getDeviceId`）。
//
// # ⚠ 绑定账号的是**特征**，不是 smid，也不是 deviceId
//
// 这三样东西的生命周期**完全不同**，混起来就会把该变的东西钉死：
//
//	|              | 会不会变 | 本项目的处理 |
//	|--------------|----------|--------------|
//	| **设备特征**（屏幕、dpr、platform、canvas、cpu 数、ua…） | 真机上**不变**（那是同一台设备） | **绑定到账号**：首次生成一份，落进 `account_state.smid_fingerprint`，之后一直复用 |
//	| **smid 字段** | **每次都变** —— 它内嵌 `yyyyMMddHHmmss` 与一个现生成的 uuid | 每次上报现算，不落库 |
//	| **deviceId**（`B…`） | 服务端发的，会变（换指纹、过期都会换） | 每次上报重新取；库里的 `smid_device_id` 只是"最近拿到的那一个" |
//
// 旧项目**每跑一次任务就重新随机一份特征**（`core/src/utils/init-account.ts:33`
// 调 `getDeviceId()`，而 `getSmData` 里那些字段全是当场 `Math.random`），
// 等于同一台机器每次上报都长成另一台机器的样子。本项目不跟：
// 特征生成一次、按账号存下来复用，**只在主动更换指纹时**才重生成。
//
// 于是本模块被拆成"稳定的一半 + 每次现算的一半"：
// [SmFingerprint]（可序列化、进库）与 [SmUpload] + 那几个 `rng` 参数。
//
// # 这套特征是 **web / H5 侧**的（`os: "web"`）
//
// `platform: "Linux armv8l"`、`plugins`（Chrome PDF 查看器那一串）、`ua`
// 都是**网页**特征，不是原生 App 特征。旧项目用它是为了给自己所有请求补一个
// `deviceid` 头（`shared/src/http/base.ts:94`）。
//
// [CaiyunAndroidWebFingerprint] 逐字沿用旧项目那套池子，不"顺手改成 iOS"：
// 我们手上没有原生侧 smid 的抓包，改了等于凭空发明一份特征。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量
// ─────────────────────────────────────────────────────────────────────────────

// SmAPIHost 是 cmpassport 风控服务（**不是**云盘的域，所以不与 [BaseUserYun] 那批放一起）。
//
// 刻意分开：这台机器不认云盘凭据，把 `Authorization` 发过去等于把凭据交给第三方风控服务。
const SmAPIHost = "https://slw.h5cmpassport.com:9090"

// SmAPIPath 是上报路径（旧 `SKLAND_SM_CONFIG.apiPath`）。
const SmAPIPath = "/deviceprofile/v4"

// SmOrganization 是组织标识（旧 `SKLAND_SM_CONFIG.organization`）。
const SmOrganization = "FXlyfmWg2AzwbrxDKSv5"

// SmAppID 是固定 `appId`（旧 `SKLAND_SM_CONFIG.appId`）。
const SmAppID = "default"

// SmOKCode 是成功码 —— **数字**，与云盘那批字符串码（`"0000"`）不是一类，
// 所以不走 [Envelope]。
const SmOKCode int64 = 1100

// SmDeviceIDPrefix 是 `deviceId` 的前缀：旧 `getDeviceId()` 拿到 `detail.deviceId`
// 后硬拼上去的那一个字母。
const SmDeviceIDPrefix = "B"

// CaiyunPageURL 是上报里"这次指纹发生在哪个页面上"（旧 `desTarget.url`）。
const CaiyunPageURL = "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin&sourceid=1002&token="

// CaiyunAndroidWebUA 是旧项目的 `CAIYUN_BASE_UA`（Android WebView）。
const CaiyunAndroidWebUA = "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36"

// SmPlugins 是特征体里 `plugins` 的取值（旧实现写死的一条，不是随机池）。
const SmPlugins = "ChromePDFViewerPortableDocumentFormatinternal-pdf-viewer2,ChromiumPDFViewerPortableDocumentFormatinternal-pdf-viewer2,MicrosoftEdgePDFViewerPortableDocumentFormatinternal-pdf-viewer2,PDFViewerPortableDocumentFormatinternal-pdf-viewer2,WebKitbuilt-inPDFPortableDocumentFormatinternal-pdf-viewer2"

// SmRequestOrigin 是请求头 `origin`（旧 `getDeviceId` 里写死的那个）。
const SmRequestOrigin = "https://m.mcloud.139.com"

// SmRequestReferer 是请求头 `referer`。
//
// ⚠ 与特征体里的 `referer` **不是一回事**：体里那个是空串，头里这个是页面地址。
// 旧代码就是这么分的，别合并。
const SmRequestReferer = "https://m.mcloud.139.com/portal/mobilecloud/index.html"

// 随机池（旧 `generateRandomScreenSize()` / `platforms` / …），逐条沿用旧项目的取值。
var (
	// SmScreens 是屏幕尺寸池。
	SmScreens = [][2]uint32{{375, 667}, {390, 844}, {414, 896}, {360, 640}, {360, 740}, {412, 915}, {1920, 1080}, {1366, 768}}
	// SmPlatforms 是 platform 池。
	SmPlatforms = []string{"Linux armv8l", "Linux aarch64"}
	// SmDprs 是设备像素比池。
	SmDprs = []int64{2, 3}
	// SmMaxTouchPoints 是触屏点数池。
	SmMaxTouchPoints = []int64{1, 5}
	// SmConnectionRtt 是网络 rtt 池。
	SmConnectionRtt = []int64{50, 100, 150}
	// SmCPUCounts 是 cpu 数池。
	SmCPUCounts = []int64{4, 6, 8, 10, 12}
)

// ─────────────────────────────────────────────────────────────────────────────
// 稳定的一半：绑定账号的设备特征
// ─────────────────────────────────────────────────────────────────────────────

// SmFingerprint 是**绑定到账号**的那一份设备特征。
//
// 这就是"这台设备长什么样"：真机上它是不会变的，所以本项目生成一次就存下来。
// 旧项目把它每次运行重掷一遍 —— 那是同一个 `deviceId` 反复拿不到、
// 或者风控侧看到"一台机器天天换屏幕分辨率"的根源。
//
// 序列化后进 `account_state.smid_fingerprint`（JSON 文本）。
//
// ⚠ 字段名是 **camelCase**（`#[serde(rename_all = "camelCase")]`）：它落库，
// 是跨版本契约 —— 改一个键名等于让所有老账号的指纹读不出来。
type SmFingerprint struct {
	Ua             string `json:"ua"`
	PageURL        string `json:"pageUrl"`
	Platform       string `json:"platform"`
	ScreenWidth    uint32 `json:"screenWidth"`
	ScreenHeight   uint32 `json:"screenHeight"`
	Dpr            int64  `json:"dpr"`
	MaxTouchPoints int64  `json:"maxTouchPoints"`
	ConnectionRtt  int64  `json:"connectionRtt"`
	CPUCount       int64  `json:"cpucount"`
	Canvas         string `json:"canvas"`
}

// GenerateSmFingerprint 掷一份新特征（每个字段从旧项目那个池子里等概率取）。
//
// 顺序固定为：屏幕 → dpr → platform → 触屏点数 → rtt → cpu 数 → canvas。
// [core.Random] 一旦注入，这个顺序就是内容本身 —— 测试钉它。
func GenerateSmFingerprint(rng core.Random) SmFingerprint {
	width, height := pickScreen(rng)
	return SmFingerprint{
		Ua:             CaiyunAndroidWebUA,
		PageURL:        CaiyunPageURL,
		Platform:       pickStr(rng, SmPlatforms),
		ScreenWidth:    width,
		ScreenHeight:   height,
		Dpr:            pickNum(rng, SmDprs),
		MaxTouchPoints: pickNum(rng, SmMaxTouchPoints),
		ConnectionRtt:  pickNum(rng, SmConnectionRtt),
		CPUCount:       pickNum(rng, SmCPUCounts),
		Canvas:         canvasFromUUID(rng),
	}
}

// CaiyunAndroidWebFingerprint 逐字沿用旧项目那套池子里的**默认值**
// （屏幕 375×667、dpr 2、Linux armv8l…）。
//
// 用途是"不要随机也要有一份能用的"：夹具回放里想让整条链路字节级稳定时。
func CaiyunAndroidWebFingerprint() SmFingerprint {
	return SmFingerprint{
		Ua:             CaiyunAndroidWebUA,
		PageURL:        CaiyunPageURL,
		Platform:       SmPlatforms[0],
		ScreenWidth:    375,
		ScreenHeight:   667,
		Dpr:            2,
		MaxTouchPoints: 5,
		ConnectionRtt:  50,
		CPUCount:       8,
		Canvas:         "00000000",
	}
}

// ClientSize 是 `clientSize`：`0_0_0_0_宽_高_可用宽_可用高`
// （旧代码里可用尺寸就等于屏幕尺寸）。
func (f SmFingerprint) ClientSize() string {
	return fmt.Sprintf("0_0_0_0_%d_%d_%d_%d",
		f.ScreenWidth, f.ScreenHeight, f.ScreenWidth, f.ScreenHeight)
}

// Res 是 `res`：`宽_高_dpr_2`。
func (f SmFingerprint) Res() string {
	return fmt.Sprintf("%d_%d_%d_2", f.ScreenWidth, f.ScreenHeight, f.Dpr)
}

// WithCanvas 换掉 canvas 那一项（其余不动）。
//
// 存在的理由：`canvas` 是唯一一个"我们冒充不了真实渲染结果"的字段，
// 万一哪天要对拍发现它可疑，可以只换它而不是整份重掷。
func (f SmFingerprint) WithCanvas(canvas string) SmFingerprint {
	f.Canvas = canvas
	return f
}

// ToJSONString 是存进库时的形态（JSON 文本）。
func (f SmFingerprint) ToJSONString() (string, *core.Error) {
	buf, err := core.MarshalJSON(f)
	if err != nil {
		return "", asCoreError(err, "特征体序列化失败")
	}
	return string(buf), nil
}

// SmFingerprintFromJSON 从库里读出来。
//
// 读失败**不回落到"重新掷一份"**：那等于静默换掉这台设备的长相，
// 而换指纹在这个服务侧是可观测事件，必须让调用方看见错误。
func SmFingerprintFromJSON(raw string) (SmFingerprint, *core.Error) {
	value, ok := core.ParseJSONTo[SmFingerprint](raw)
	if !ok {
		return SmFingerprint{}, core.NewParse("库里的 smid 特征体读不出来")
	}
	return value, nil
}

// canvasFromUUID 是 canvas 指纹：`md5(uuid)` 的前 8 位。
// 真机上由渲染结果决定，**稳定**。
func canvasFromUUID(rng core.Random) string {
	return core.MD5Hex([]byte(core.UUIDv4(rng)))[:8]
}

// ─────────────────────────────────────────────────────────────────────────────
// 每次现算的一半
// ─────────────────────────────────────────────────────────────────────────────

// SmUpload 是这一次上报的时间与"上一次的 deviceId"。
//
// # 为什么时间是外面给的
//
// 旧代码用 `new Date()` 取**本地时间**拼 `yyyyMMddHHmmss`，而特征体里
// `timezone: -480` 是**写死**的东八区 —— 两者必须一致，否则等于自报
// "我在东八区，可我的时间戳是别处来的"。本项目统一按东八区算，
// 偏移由调用方给出。
type SmUpload struct {
	// TimeDigits 是 14 位 `yyyyMMddHHmmss`（东八区）。
	TimeDigits string
	// NowMillis 是 `Date.now()`：特征体里 `svm` 与 `pmf` 用它。
	NowMillis int64
	// OldDeviceID 是上一次拿到的 `deviceId`（含 `B` 前缀）；**首次上报给空串**。
	//
	// 它进的是特征体的 `box` 字段：换 id 时把旧 id 带上，让风控服务看出
	// "还是那台机器在换 id"，而不是一台凭空出现的设备。
	OldDeviceID string
}

// SmidString 是特征体里的 `smid` 字段（对齐旧 `getSmId()`）。
//
//	v       = time_digits + md5(uid) + "00"
//	smskWeb = md5("smsk_web_" + v)[0..14]
//	smid    = v + smskWeb + "0"
//
// 长度固定为 `14 + 32 + 2 + 14 + 1 = 63`。
//
// **注意这个 uid 与 [BuildSmData] 那个 uid 不是同一个**：旧 `getSmId()`
// 内部自己又 `randomUUID()` 了一次。照搬，不"顺手统一"。
func SmidString(timeDigits, uid string) string {
	v := fmt.Sprintf("%s%s00", timeDigits, core.MD5Hex([]byte(uid)))
	smskWeb := core.MD5Hex([]byte("smsk_web_" + v))[:14]
	return v + smskWeb + "0"
}

// SmPrivateID 是 `priId`：AES 密钥，`md5(uid)` 的前 16 个字符
// （旧 `getSmData` 开头两行）。
func SmPrivateID(uid string) string {
	return core.MD5Hex([]byte(uid))[:16]
}

// SmEp 是 `ep`：用 skland 公钥包住 `uid`（旧 `encryptRsa(uid, publicKey)`）。
//
// ⚠ 输出**不可对拍**：PKCS#1 v1.5 的填充本身含随机字节，所以只验形状与可解性。
func SmEp(uid string, rng core.Random) (string, *core.Error) {
	out, err := core.RSAPKCS1v15EncryptBase64WithB64Key(core.SklandPublicKeyB64, []byte(uid), rng)
	if err != nil {
		return "", asCoreError(err, "RSA 加密 uid 失败")
	}
	return out, nil
}

// SmFingerprintTarget 组一份**加密前**的特征体：
// 稳定的 [SmFingerprint] + 现掷的 smid / vpw / trees / 电量。
//
// 除了那四个 volatile 字段，其余每一项都完全由入参决定 —— 这就是把特征拆出来
// 换来的东西：**给定同一份指纹、同一个时间、同一个随机源，整份特征体可复现**。
func SmFingerprintTarget(
	fingerprint SmFingerprint, upload SmUpload, rng core.Random,
) map[string]any {
	// 旧代码的求值顺序：getSmId() 在字面量里排第 9 个，vpw / trees 在它后面
	smidUID := core.UUIDv4(rng)
	vpw := core.UUIDv4(rng)
	trees := core.UUIDv4(rng)
	// 电池是真·易变量：旧项目每次现掷，本项目也每次现掷（它不属于"设备长相"）
	charging := pickNum(rng, []int64{0, 1})
	// 旧：Math.round(Math.random() * 100) / 100 —— 两位小数
	level := math.Round(smFraction(rng)*100) / 100

	return map[string]any{
		"protocol":       4,
		"organization":   SmOrganization,
		"appId":          SmAppID,
		"os":             "web",
		"version":        "3.0.0",
		"sdkver":         "3.0.0",
		"box":            upload.OldDeviceID,
		"rtype":          "all",
		"smid":           SmidString(upload.TimeDigits, smidUID),
		"subVersion":     "1.0.0",
		"time":           7,
		"platform":       fingerprint.Platform,
		"clientSize":     fingerprint.ClientSize(),
		"res":            fingerprint.Res(),
		"status":         "0010",
		"timezone":       -480,
		"vpw":            vpw,
		"svm":            upload.NowMillis,
		"trees":          trees,
		"pmf":            upload.NowMillis,
		"cdp":            0,
		"maxTouchPoints": fingerprint.MaxTouchPoints,
		"connectionRtt":  fingerprint.ConnectionRtt,
		"cpucount":       fingerprint.CPUCount,
		"battery":        map[string]any{"charging": charging, "level": level},
		"canvas":         fingerprint.Canvas,
		"plugins":        SmPlugins,
		"referer":        "",
		"ua":             fingerprint.Ua,
		"url":            fingerprint.PageURL,
		"tn":             "",
	}
}

// BuildSmData 组好整份上报体（旧 `getSmData` 的返回）。
func BuildSmData(
	fingerprint SmFingerprint, upload SmUpload, rng core.Random,
) (map[string]any, *core.Error) {
	// 外层这个 uid 同时是 ep 包住的那一个和 priId 推出来的那一个：
	// 分两次生成会让服务端看到"包裹用的 uid"与"解密用的密钥"对不上，那直接是失败。
	uid := core.UUIDv4(rng)
	target := SmFingerprintTarget(fingerprint, upload, rng)
	ep, perr := SmEp(uid, rng)
	if perr != nil {
		return nil, perr
	}
	data, perr := SmEncodeTarget(target, uid)
	if perr != nil {
		return nil, perr
	}
	return map[string]any{
		"appId":        SmAppID,
		"organization": SmOrganization,
		"ep":           ep,
		"data":         data,
		"os":           "web",
		"encode":       5,
		"compress":     2,
	}, nil
}

// SmEncodeTarget 是特征体 → 上报体里那个 `data` 字段（tn → DES 改名 → gzip → AES）。
//
// # `tn` 那一步的顺序不能换
//
// 旧代码是 `desTarget.tn = md5Hash(getTn(desTarget))`：**求值时 `tn` 还是空串**。
// 先写回再算，摘要里会多出一段 32 位十六进制，整份指纹当场作废。
func SmEncodeTarget(target map[string]any, uid string) (string, *core.Error) {
	withTn := make(map[string]any, len(target)+1)
	for k, v := range target {
		withTn[k] = v
	}
	withTn["tn"] = core.GetTnMD5(target)

	encoded, perr := SmEncryptByDesRules(withTn)
	if perr != nil {
		return "", perr
	}
	// 旧：gzip(JSON.stringify(特征体)) → base64 → AES-CBC(那个 base64 串, priId) → hex
	plain, err := core.MarshalJSON(encoded)
	if err != nil {
		return "", asCoreError(err, "特征体序列化失败")
	}
	gzipped, err := core.GzipWithOS19Base64(plain)
	if err != nil {
		return "", asCoreError(err, "gzip 失败")
	}
	out, err := core.AESCBCFixedIVEncryptHex(SmPrivateID(uid), gzipped)
	if err != nil {
		return "", asCoreError(err, "AES 加密 data 失败")
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// DES 字段规则
// ─────────────────────────────────────────────────────────────────────────────

// SmRule 是一个字段的上报规则。
type SmRule struct {
	// CipherKey 为 `""` = **不加密**（旧 `is_encrypt: 0`），但仍然改名。
	CipherKey string
	// WireName 是混淆后的键名（上报体里真正出现的那个）。
	WireName string
}

// smRules 是旧 `DES_RULE` 逐条照搬（25 条）。
//
// 注意"没有规则"与"有规则但不加密"是两件事：前者**原名**输出
// （`smid`、`cdp`、`battery` 那批），后者**改名**输出
// （`box`→`aw`、`protocol`、`version`）。
//
// ⚠ 它是 map 而不是 slice：Go 没有 `const [](&str, Rule)`，
// 而**查表顺序在这里没有语义**（每条规则只按字段名命中一次），
// 所以无序容器是安全的 —— 真要靠顺序的地方（rng 取值顺序）都在函数里显式写出。
var smRules = map[string]SmRule{
	"appId":        {CipherKey: "a9kbown8", WireName: "um"},
	"box":          {CipherKey: "", WireName: "aw"},
	"canvas":       {CipherKey: "ultuhs59", WireName: "iu"},
	"clientSize":   {CipherKey: "7s9eixjh", WireName: "na"},
	"organization": {CipherKey: "14e30pp1", WireName: "nl"},
	"os":           {CipherKey: "j862vr1f", WireName: "lz"},
	"platform":     {CipherKey: "pgutfbo7", WireName: "lm"},
	"plugins":      {CipherKey: "x98c92bm", WireName: "hy"},
	"pmf":          {CipherKey: "gwvvsmr4", WireName: "yu"},
	"protocol":     {CipherKey: "", WireName: "protocol"},
	"referer":      {CipherKey: "gka1ljjt", WireName: "mf"},
	"res":          {CipherKey: "ht2n6dwf", WireName: "ut"},
	"rtype":        {CipherKey: "k2p9ifij", WireName: "fm"},
	"sdkver":       {CipherKey: "q4ij0mv5", WireName: "ir"},
	"status":       {CipherKey: "moyspi7i", WireName: "kp"},
	"subVersion":   {CipherKey: "egsyl21v", WireName: "dk"},
	"svm":          {CipherKey: "yf780e9w", WireName: "ee"},
	"time":         {CipherKey: "btb17oag", WireName: "jt"},
	"timezone":     {CipherKey: "l94h8clc", WireName: "bl"},
	"tn":           {CipherKey: "crnw0x7e", WireName: "as"},
	"trees":        {CipherKey: "x6the9nq", WireName: "ij"},
	"ua":           {CipherKey: "bewicpxu", WireName: "pu"},
	"url":          {CipherKey: "llj8yd1x", WireName: "sb"},
	"version":      {CipherKey: "", WireName: "version"},
	"vpw":          {CipherKey: "f1zlae4b", WireName: "qb"},
}

// SmRuleFor 查字段规则。
func SmRuleFor(field string) (SmRule, bool) {
	rule, ok := smRules[field]
	return rule, ok
}

// SmWireNames 是全部 25 条规则的目标键名 —— 供"向量必须覆盖到每一条"那条断言用。
func SmWireNames() []string {
	out := make([]string, 0, len(smRules))
	for _, rule := range smRules {
		out = append(out, rule.WireName)
	}
	return out
}

// jsToString 是 `String(value)`（旧 `encryptDes(String(value), key)` 里的那次转换）。
//
// 只有进 DES 的字段会经过它，而那些字段实际只有字符串与整数两种，
// 所以这里只保证这两类与 JS 一致；其余按 JSON 文本兜底。
func jsToString(value any) string {
	switch v := value.(type) {
	case string:
		return v
	case json.Number:
		return v.String()
	case float64:
		// JS 的 `Number.toString()` 与 Rust 的 f64 Display 在小数上同为最简形式
		return strconv.FormatFloat(v, 'f', -1, 64)
	case int:
		return strconv.Itoa(v)
	case int64:
		return strconv.FormatInt(v, 10)
	case bool:
		return strconv.FormatBool(v)
	case nil:
		return "null"
	}
	return fmt.Sprintf("%v", value)
}

// SmEncryptByDesRules 按规则表把特征体改写成上报体（加密 + 换键名）。
//
// 没有规则的字段**原名原值**带出去（旧 `encryptObjectByDesRules` 的 `else` 分支）。
func SmEncryptByDesRules(target map[string]any) (map[string]any, *core.Error) {
	out := make(map[string]any, len(target))
	for field, value := range target {
		rule, ok := smRules[field]
		if !ok {
			out[field] = value
			continue
		}
		if rule.CipherKey == "" {
			out[rule.WireName] = value
			continue
		}
		wire, err := core.DESECBNoPaddingEncryptBase64(rule.CipherKey, jsToString(value))
		if err != nil {
			return nil, asCoreError(err, fmt.Sprintf("DES 加密字段 %s 失败", field))
		}
		out[rule.WireName] = wire
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 随机源
// ─────────────────────────────────────────────────────────────────────────────

// smTwoBytes 取两字节（旧 `randomChoice` 里那次 `Math.random` 的等价物）。
func smTwoBytes(rng core.Random) uint16 {
	var b [2]byte
	rng.FillBytes(b[:])
	return uint16(b[0])<<8 | uint16(b[1])
}

// pickScreen 用**一次**取值同时定下宽与高 —— Rust 侧也是
// `SCREENS[two_bytes(rng) % len]`，一次解构出一对。
// 取两次会让"宽高必须来自同一档"这件事悄悄失效（1920×667 这种组合是编不出来的）。
func pickScreen(rng core.Random) (uint32, uint32) {
	pair := SmScreens[int(smTwoBytes(rng))%len(SmScreens)]
	return pair[0], pair[1]
}

func pickStr(rng core.Random, pool []string) string {
	return pool[int(smTwoBytes(rng))%len(pool)]
}

func pickNum(rng core.Random, pool []int64) int64 {
	return pool[int(smTwoBytes(rng))%len(pool)]
}

// smFraction 是 `Math.random()` 的位置：换成注入的 [core.Random]。
//
// 分布细节与 `Math.random` 不同（这里 16 位均匀，那边 53 位），
// **不影响正确性**：这是设备特征不是签名校验 —— 旧项目每次运行现掷一份都能过，
// 说明服务端不比对内容，只吃"像个浏览器"这个形状。
func smFraction(rng core.Random) float64 {
	return float64(smTwoBytes(rng)) / float64(math.MaxUint16)
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求与响应
// ─────────────────────────────────────────────────────────────────────────────

// SmDeviceProfileRequest 构造 POST 上报体（旧 `getDeviceId` 里那个 `fetch`）。
//
// 这台主机不是云盘的，所以**不带**任何云盘凭据（`Authorization` / `jwttoken` /
// `x-yun-uni` 一个都不发）—— 它既不需要，也不该看到。
func SmDeviceProfileRequest(
	payload any, fingerprint SmFingerprint,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(SmAPIHost + SmAPIPath).WithJSONBody(payload)
	if err != nil {
		return request, asCoreError(err, "序列化上报体失败")
	}
	return request.
		WithHeader("origin", SmRequestOrigin).
		WithHeader("referer", SmRequestReferer).
		WithHeader("user-agent", fingerprint.Ua), nil
}

// SmParseDeviceID 从响应里取 `deviceId`（旧 `code === 1100 → "B" + detail.deviceId`）。
//
// 返回的是**拼好前缀**的那个，也就是最终进 `deviceid` 头的值。
//
// # 为什么失败时不回吐响应体
//
// 成功响应里就带着 `deviceId`，失败响应可能带别的机器能认的标识；
// 而错误串会被打日志、会被测试断言引用。所以这里只放**码值**。
func SmParseDeviceID(raw any) (string, *core.Error) {
	obj, _ := raw.(map[string]any)
	code, _ := asInt64(obj["code"])
	if code != SmOKCode {
		return "", core.NewParse(fmt.Sprintf("smid 上报被拒：code=%d（期望 %d）", code, SmOKCode))
	}
	detail, _ := obj["detail"].(map[string]any)
	device, _ := detail["deviceId"].(string)
	if device == "" {
		return "", core.NewParse(fmt.Sprintf("smid 响应 code=%d 但没有 detail.deviceId", SmOKCode))
	}
	return SmDeviceIDPrefix + device, nil
}

// SmidDeviceID 上报一份特征，换回一个 `deviceId`（带 `B` 前缀）。
//
// # 与旧实现的一处**刻意**不同
//
// 旧 `getDeviceId()` 把一切异常 `catch` 掉返回**空串**，由调用方判空。
// 这里返回错误：空串一旦被当成"成功"存进库，下次读出来是非空，
// 于是那个账号永远绑着一个空设备、再也不会触发重新上报。
func (u *Upstream) SmidDeviceID(
	ctx context.Context, fingerprint SmFingerprint, payload any,
) (string, *core.Error) {
	request, perr := SmDeviceProfileRequest(payload, fingerprint)
	if perr != nil {
		return "", perr
	}
	response, err := u.http.Send(ctx, request)
	if err != nil {
		return "", asCoreError(err, "上报设备特征失败")
	}
	if !response.IsSuccess() {
		return "", errorFromStatus(response.Status)
	}
	value, ok := core.ParseJSON(string(response.Body))
	if !ok {
		return "", core.NewParse("smid 响应不是合法 JSON")
	}
	return SmParseDeviceID(value)
}
