// 移动云盘 PC 客户端登录协议（扫码 / 短信 / 账密 / 退出登录）。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-upstream/src/login.rs。
//
// # 规格来源
//
// 全部依据实测，不含任何猜测性字段：
//
//   - docs/upstream/login-protocol.md：扫码登录（本地拼装二维码 + `thirdlogin` 轮询 + 状态机）
//   - docs/upstream/logout-sms-login.md：退出登录、短信登录全链路（含滑块降级）
//   - 客户端 `common/utils.js`（从本机 `mCloud.app` 的 `app.asar` 提取）：三个密码学原语的实现与密钥，
//     对应 `test/golden/pcclient-crypto-vectors.json`
//
// # 三条"反直觉"的实测结论（照直觉写就会错）
//
//  1. **不存在"取码接口"**：二维码 URL 由客户端**本地拼装**，零网络请求。
//     所以这里只有 [QrPayload] 一个纯函数，没有任何"申请二维码"的请求。
//  2. **扫码与短信、账密共用同一个接口** `POST /user/thirdlogin`，靠 `pintype` 区分。
//  3. **登录成功的 `data` 是 AES 密文**，不是明文 JSON —— 见 [LoginPayloadDecryptor]。
//
// # 本模块的边界
//
// 只做**请求构造**与**响应语义**，不含轮询循环与持久化。
// 因此所有 body 构造函数与状态判定都是**纯函数**，可以离线逐字段断言。
//
// # 三个密码学原语（密钥来自客户端源码，滑块那条还被真实流量反向验证过）
//
// 客户端 `common/utils.js` 里的加解密都不是标准做法，照"常识"写一定会错：
//
// | 原语 | 用在哪 | 实参 |
// | --- | --- | --- |
// | `decryptByAes` | 成功响应的 `data` | AES-128-**ECB**/PKCS7，**输入 hex 不是 base64** |
// | `encryptByAesCBC` | 滑块答案 `puzzleVerfycode` | AES-128-CBC/PKCS7，**固定 IV**，输出 `base64(IV‖CT)` |
// | `encryptByHash` | 账密登录的 `dycpwd` | `SHA1("fetion.com.cn:" + 口令)` 取**大写** hex |
//
// 验证方式不是"看起来能解开"：归档里两条**真实** `puzzleVerfycode` 用上面的 key/IV
// 反解出来的明文是 `461` / `391`（拖动像素偏移，末尾 13 字节 `0x0d` 正是 PKCS7 填充）。
// 这同时**补正了协议文档 §2.3 的一处推测**：前 16 字节不是"服务端挑战标识"，
// 而是源码里硬编码的 IV —— 所以它当然每次都一样。
//
// # ⚠ 尚未落地的部分
//
// 四个**请求构造器**（`thirdlogin_request` / `send_sms_code_request` /
// `slider_puzzle_request` / `logout_request`）与"带端头发送"那一层，
// 依赖 core 的 `HttpRequest` / `HttpClient`（traits）——
// 那份正在由另一个工作项迁移，**落地后补在这里**，不要在本包自己造一套请求类型。
package upstream

import (
	"encoding/hex"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 平台与登录方式
// ─────────────────────────────────────────────────────────────────────────────

// PcPlatform 是客户端平台形态。
//
// `clientType` / `cpid` 在 Windows 与 macOS 上是**两组不同的值**（客户端源码 L11-12），
// 混用会被上游当成另一个客户端。做成枚举而不是两个字符串参数，
// 是为了让"传错平台"只能发生在选择平台的那一刻，而不是每一个调用点。
type PcPlatform int

const (
	// PlatformWindows 是 Windows 客户端。
	PlatformWindows PcPlatform = iota
	// PlatformMacOS 是 macOS 客户端。
	PlatformMacOS
)

// String 返回平台名（诊断用）。未知值返回空串（枚举漏态守卫）。
func (p PcPlatform) String() string {
	switch p {
	case PlatformWindows:
		return "windows"
	case PlatformMacOS:
		return "macos"
	}
	return ""
}

// ClientType 返回 `clientType` 业务参数。
func (p PcPlatform) ClientType() string {
	switch p {
	case PlatformWindows:
		return "656"
	case PlatformMacOS:
		return "710"
	}
	return ""
}

// Cpid 返回 `cpid` 业务参数。
func (p PcPlatform) Cpid() string {
	switch p {
	case PlatformWindows:
		return "277"
	case PlatformMacOS:
		return "332"
	}
	return ""
}

// AppVersion 返回该平台客户端上报的版本号（源码里是 `window.appVersion`）。
//
// 与 [CaiyunAppVersion] 同类：它是**业务参数**而非展示信息 ——
// 同时出现在请求体的 `version` 字段和二维码的 `ver` 参数上，
// 改它等于把自己伪装成另一个版本的客户端（上游按版本分流）。
//
// 两个值都来自实测而非挑选：
//   - Windows `8.8.6.20260829`：抓包归档里的真实请求体（docs/upstream/login-protocol.md 证据链 A）；
//   - macOS `4.2.1`：本机 `mCloud.app/Contents/Resources/app.asar` 的 `package.json`。
//
// 所以它**必须跟着平台走**：mac 的 `clientType` 配 Windows 的版本号，
// 是一个上游一眼可辨的矛盾组合。
func (p PcPlatform) AppVersion() string {
	switch p {
	case PlatformWindows:
		return "8.8.6.20260829"
	case PlatformMacOS:
		return "4.2.1"
	}
	return ""
}

// LoginKind 是 `/user/thirdlogin` 的登录方式，取值为 `pintype`。
//
// 同一个接口承载三种登录方式；这是实测确认的（协议 §6）。
type LoginKind int

const (
	// LoginQrCode 是扫码：`dycpwd` = 二维码里的 `sID`，`msisdn` 为空。
	LoginQrCode LoginKind = iota
	// LoginSms 是短信验证码：`dycpwd` = 6 位验证码明文，`msisdn` = 手机号。
	LoginSms
	// LoginPassword 是账号密码：`dycpwd` = 口令哈希，`msisdn` = 用户名。
	LoginPassword
)

// Pintype 返回 `pintype` 取值。
func (k LoginKind) Pintype() string {
	switch k {
	case LoginQrCode:
		return "21"
	case LoginSms:
		return "5"
	case LoginPassword:
		return "9"
	}
	return ""
}

// IfOpenAccount 返回 `extInfo.ifOpenAccount`：短信登录是 `1`，扫码与账密是 `0`
// （协议 §2.4 对照表）。
func (k LoginKind) IfOpenAccount() int64 {
	switch k {
	case LoginSms:
		return 1
	case LoginQrCode, LoginPassword:
		return 0
	}
	return 0
}

// 上游业务码（协议 §4 的 192 条实测汇总）。
//
// 单列成常量而不是散在 `switch` 里，是为了让"这个码的含义有实测依据"这件事可见 ——
// 这些值不是猜的，每一个都在文档里带着出现次数。
const (
	// CodeOKZero 是成功。扫码与短信两条链路都见过 `0000`，扫码还见过 `200`。
	CodeOKZero int64 = 0
	// CodeOK200 同上（`200` 那条）。
	CodeOK200 int64 = 200

	// CodeQRNotScanned 是未找到设备的二维码信息 —— 码还没被扫。
	CodeQRNotScanned int64 = 200_059_541
	// CodeQRScanned 是二维码信息未授权 —— **已扫码，等用户在手机上确认**。
	CodeQRScanned int64 = 200_059_548
	// CodeQRMismatch 是二维码信息与登录二维码信息不一致 —— `sID` 不匹配
	// （换了码却还在用旧值）。
	CodeQRMismatch int64 = 200_059_544
	// CodeQRExpired 是二维码已失效 —— 超时，必须重新出码。
	CodeQRExpired int64 = 200_059_542
	// CodeQRTooFrequent 是扫码登录过于频繁 —— **实测**（连着跑几次扫码登录时撞上）。
	//
	// 它不在文档原先的错误码表里，是实践中补的。语义是限流：
	// 正确反应是**退避**，而不是换张码立刻重试（那只会更深地撞进风控）。
	CodeQRTooFrequent int64 = 200_059_545

	// CodeNeedSlider 是请先获取滑块拼图验证码。
	CodeNeedSlider int64 = 200_059_554
	// CodeSliderExhausted 是同一滑动拼图验证码校验失败次数超限 ——
	// **该挑战作废，必须重新取图**。
	CodeSliderExhausted int64 = 200_059_553

	// CodeTokenEmpty 是用户 token 不能为空（退出登录的实测常态：
	// 客户端先清 token 再调 logout）。
	CodeTokenEmpty int64 = 1_000_001

	// ── App 端实测新增（docs/capture/app-sms-login-samples.txt）──
	//
	// 这三个码 PC 端没见过，是 App 端 `thirdlogin` 上撞出来的。
	// 它们的价值在于构成一个**二值判别器**：调加密参数时不需要真的登录成功，
	// 只看回 `9103` 还是别的，就能判断"服务端有没有解开这个体"。

	// CodeUndecryptableBody 是输入参数不合法 —— 服务端**解不开请求体**。
	//
	// 实测成因只有三类：体没加密、AES 密钥选错（用了备用那把）、
	// 错位方向写反。看到它就往这三处查。
	CodeUndecryptableBody int64 = 9_103
	// CodeSmsCodeWrong 是验证码不正确 —— **请求体已被正确解析**，
	// 只是短信码不对/过期。
	//
	// 与 [CodeUndecryptableBody] 相对：拿到它说明密钥、模式、错位方向、
	// hex 大小写全部正确，加密这一层可以结案了。
	CodeSmsCodeWrong int64 = 9_441
	// CodeLoginFailed 是登录失败！—— 设备头缺失/不完整时的表现。
	CodeLoginFailed int64 = 9_999
)

// ─────────────────────────────────────────────────────────────────────────────
// 二维码（本地拼装）
// ─────────────────────────────────────────────────────────────────────────────

// QRLoginURL 是扫码登录的二维码落地页。`sID` / `dID` 以 query 形式拼在 `#/` 之后。
const QRLoginURL = "https://yun.139.com/w/#/qrcLogin"

// QrPayload 拼出二维码载荷。
//
// # 为什么没有"申请二维码"的请求
//
// 客户端源码 `createLoginQRcode()` 里就是这四行字符串拼接，**不发起任何网络请求**。
// 实测 1622 个 body 文件里也没有对应的"取码"接口 —— 这是本次抓包最重要的发现，
// 它推翻了"取码要调接口"的直觉假设。
//
// `sID` 与后续 `thirdlogin` 的 `dycpwd` 是**同一个值**
// （源码注释：`// QRCodeID也是sessionId`），
// 所以调用方拿它出码之后，要把它一直带到轮询结束。
//
// `dID`（设备指纹）必须**持久化**：每次重新生成会让服务端定位不到会话
// （轮询会一直停在"未找到设备的二维码信息"）。
func QrPayload(platform PcPlatform, deviceID, sessionID string) string {
	return fmt.Sprintf("%s?dID=%s&sID=%s&cType=11&ver=%s",
		QRLoginURL, deviceID, sessionID, platform.AppVersion())
}

// NewSessionID 生成一个 `sID`：32 位**大写**十六进制
// （对齐客户端 `Utils.getGUID32()`）。
func NewSessionID(r core.Random) string {
	buf := make([]byte, 16)
	r.FillBytes(buf)
	return strings.ToUpper(hex.EncodeToString(buf))
}

// NewSmsRandom 生成发码请求里的 `random`：**24 位**小写十六进制。
//
// # 为什么不用 UUID
//
// 客户端源码里它是两段 `Math.random().toString(16).substring(2)` 直接拼接的结果，
// 既不是 UUID 也不是 16/32 位 —— 长度（24）与字符集（小写十六进制）都由服务端校验，
// 换成 UUID 会得到 36 字符带连字符的串，形态就不对了。
func NewSmsRandom(r core.Random) string {
	buf := make([]byte, 12)
	r.FillBytes(buf)
	return hex.EncodeToString(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求体（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ 下面几个结构体的字段**按 JSON 键的字典序**排列，不是随手排的。
//
// Rust 侧是 `json!({...})`，而 serde_json 的 Map 默认按**字典序**输出
// （未开 `preserve_order`）；Go 的 struct 按声明顺序输出。
// 让声明顺序等于字典序，两边序列化出来的字节就**逐字节相同** ——
// 将来做请求体对拍时不需要额外解释顺序差异。
// 新增字段时请把它插到正确的位置。

// ExtInfoBody 是 `thirdlogin` 请求体里的 `extInfo`。
type ExtInfoBody struct {
	IfOpenAccount int64 `json:"ifOpenAccount"`
}

// ThirdLoginBody 是 `/user/thirdlogin` 的请求体。
//
// 三种登录方式共用这一个构造器 —— 差异全部收敛在 [LoginKind] 与两个入参上，
// 这样"新增一种登录方式"只会动一处。
type ThirdLoginBody struct {
	ClientType string      `json:"clientType"`
	Cpid       string      `json:"cpid"`
	Dycpwd     string      `json:"dycpwd"`
	ExtInfo    ExtInfoBody `json:"extInfo"`
	LoginMode  string      `json:"loginMode"`
	Msisdn     string      `json:"msisdn"`
	Pintype    string      `json:"pintype"`
	VerType    string      `json:"verType"`
	Version    string      `json:"version"`
}

// NewThirdLoginBody 构造 `/user/thirdlogin` 的请求体。
func NewThirdLoginBody(platform PcPlatform, kind LoginKind, dycpwd, msisdn string) ThirdLoginBody {
	return ThirdLoginBody{
		ClientType: platform.ClientType(),
		Cpid:       platform.Cpid(),
		Dycpwd:     dycpwd,
		ExtInfo:    ExtInfoBody{IfOpenAccount: kind.IfOpenAccount()},
		LoginMode:  "0",
		Msisdn:     msisdn,
		Pintype:    kind.Pintype(),
		VerType:    "2",
		Version:    platform.AppVersion(),
	}
}

// NewQRThirdLoginBody 是扫码登录的请求体：`dycpwd` 直传二维码里的 `sID`，
// **无需任何变换**。
func NewQRThirdLoginBody(platform PcPlatform, sessionID string) ThirdLoginBody {
	return NewThirdLoginBody(platform, LoginQrCode, sessionID, "")
}

// NewSmsThirdLoginBody 是短信登录的请求体：`dycpwd` 是 **6 位验证码明文**（不是哈希）。
func NewSmsThirdLoginBody(platform PcPlatform, phone, smsCode string) ThirdLoginBody {
	return NewThirdLoginBody(platform, LoginSms, smsCode, phone)
}

// NewPasswordThirdLoginBody 是账号密码登录的请求体：`dycpwd` 是**哈希后的口令**，
// `msisdn` 是用户名。
//
// 哈希规则见 [PasswordHash]。安全登录分支（服务端下发 `smsCheckRandom` 时改用
// `secinfo`、`dycpwd` 置空）暂未实现：实测没走到那条分支，
// 且它依赖 `safeVerifyCodeEncrypt`（`MD5(...)`，纯计算已具备），
// 等真遇到再补，不先写猜测性代码。
func NewPasswordThirdLoginBody(platform PcPlatform, username, password string) ThirdLoginBody {
	return NewThirdLoginBody(platform, LoginPassword, PasswordHash(password), username)
}

// SendSmsCodeBody 是 `/user/sms/getSmsCode` 的请求体。
//
// `mode` / `reqType` 是固定值（实测），不要当可变参数暴露出去。
//
// `PuzzleVerfycode` 为 nil 时该键**不出现**（普通发码）；
// 走滑块降级时由 [NewSendSmsCodeBodyWithSlider] 填上。
type SendSmsCodeBody struct {
	ClientType      string  `json:"clientType"`
	Mode            int     `json:"mode"`
	PhoneNumber     string  `json:"phoneNumber"`
	PuzzleVerfycode *string `json:"puzzleVerfycode,omitempty"`
	Random          string  `json:"random"`
	ReqType         int     `json:"reqType"`
}

// NewSendSmsCodeBody 构造发码请求体。
func NewSendSmsCodeBody(platform PcPlatform, phone, random string) SendSmsCodeBody {
	return SendSmsCodeBody{
		ClientType:  platform.ClientType(),
		Mode:        0,
		PhoneNumber: phone,
		Random:      random,
		ReqType:     3,
	}
}

// SliderPuzzleBody 是 `/user/verfycode` 的请求体：取滑块拼图。
//
// `type: 2` 表示滑块（源码注释：目前仅支持 2）。
// 路径与字段名都**没有 `i`**：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的。
type SliderPuzzleBody struct {
	Account string `json:"account"`
	Type    int    `json:"type"`
}

// NewSliderPuzzleBody 构造取滑块图的请求体。
func NewSliderPuzzleBody(phone string) SliderPuzzleBody {
	return SliderPuzzleBody{Account: phone, Type: 2}
}

// LogoutBody 是 `/user/logout` 的请求体。
type LogoutBody struct {
	PhoneNumber string `json:"phoneNumber"`
}

// NewLogoutBody 构造退出登录的请求体。
func NewLogoutBody(phone string) LogoutBody { return LogoutBody{PhoneNumber: phone} }

// ─────────────────────────────────────────────────────────────────────────────
// 滑块拼图
// ─────────────────────────────────────────────────────────────────────────────

// SliderPuzzle 是滑块拼图：两张图 + **缺口坐标**。
//
// 字段映射来自客户端源码 `apiServer.js: getVerfyCode()` 的 `L497-499`
// （`backGroundImg: obj.data?.picture`、`puzzleImg: obj.data?.puzzle`），
// 并在 2026-09-18 用一次真实调用核对过响应字段
// （`picture` / `puzzle` / `puzzleWidth` / `picWidth` / `picHeight` / `picType` / `puzzleLeft`）。
type SliderPuzzle struct {
	// BackgroundB64 是背景图（带缺口的那张，实测是 JPEG）。
	BackgroundB64 string
	// PuzzleB64 是拼图块本身（实测是 PNG）。
	PuzzleB64 string
	// PicWidth 是背景图宽度（**上游给的是字符串** `"680"`）。
	PicWidth *int64
	// PuzzleWidth 是拼图块宽度（字符串 `"96"`）。
	PuzzleWidth *int64
	// LeftEncrypted 是加密后的**缺口横坐标** —— 用 [SliderPuzzle.Answer] 解。
	LeftEncrypted string
}

// Answer 解出拖动答案（缺口横坐标的十进制字符串）。
//
// # 这个接口把答案直接给了出来
//
// `data.puzzleLeft` 是 `base64(IV ‖ AES-128-CBC/PKCS7 密文)`，密钥就是客户端
// `common/utils.js: encryptByAesCBC` 里那个**硬编码公开常量** `CREATPUZZLE03F9A`。
//
// 2026-09-18 实测：解出来是 `370`，而同一响应里 `picWidth = 680` —— 落在图宽之内，
// 与文档归档的两条真实拖动答案（`391` / `461`）同一量级与同一结构。
//
// 所以**不需要图像识别，也不需要人眼看图量像素**：解出来就是拖动答案。
//
// 解出来会做两道自检（必须是十进制数、必须落在图宽内）：宁可报错也不要
// 把一个荒谬的坐标发上去 —— 那会被判滑块失败，而失败次数是有限的。
func (p SliderPuzzle) Answer() (string, *core.Error) {
	plain, err := core.AESDynamicIVDecrypt(PuzzleAESKey, p.LeftEncrypted)
	if err != nil {
		return "", asCoreError(err, "解缺口坐标失败")
	}
	answer := strings.TrimSpace(plain)

	if answer == "" || !isAllASCIIDigits(answer) {
		return "", core.NewParse(fmt.Sprintf("缺口坐标不是十进制数: %q", answer))
	}
	x, convErr := strconv.ParseInt(answer, 10, 64)
	if convErr != nil {
		return "", core.NewParse(fmt.Sprintf("缺口坐标解析失败: %v", convErr))
	}
	if p.PicWidth != nil && (x < 0 || x > *p.PicWidth) {
		return "", core.NewParse(fmt.Sprintf("缺口坐标 %d 超出图宽 %d", x, *p.PicWidth))
	}
	return answer, nil
}

// ParseSliderPuzzle 从 `/user/verfycode` 的响应里取出滑块挑战。
//
// 取不到就报错而不是返回空串：空图片会让人以为"图挂了"，
// 而真实原因往往是上游换了字段名（那时应该立刻看见字段名不对）。
func ParseSliderPuzzle(envelope *Envelope) (*SliderPuzzle, *core.Error) {
	payload, ok := envelope.Payload()
	if !ok {
		return nil, core.NewParse("取滑块响应里没有 data")
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return nil, core.NewParse("取滑块响应的 data 不是对象")
	}

	field := func(name string) (string, *core.Error) {
		value, ok := obj[name]
		if !ok {
			return "", core.NewParse(fmt.Sprintf("取滑块响应里缺少 %s", name))
		}
		text, ok := value.(string)
		if !ok || text == "" {
			return "", core.NewParse(fmt.Sprintf("取滑块响应里缺少 %s", name))
		}
		return text, nil
	}
	// 尺寸上游给的是字符串（`"680"`），但也容忍数字 —— 这类"本该是数字却包成字符串"
	// 的字段在小步演进的接口里很常见，为此报错不值得。
	size := func(name string) *int64 {
		n, ok := asInt64(obj[name])
		if !ok {
			return nil
		}
		return &n
	}

	var out SliderPuzzle
	var err *core.Error
	if out.BackgroundB64, err = field("picture"); err != nil {
		return nil, err
	}
	if out.PuzzleB64, err = field("puzzle"); err != nil {
		return nil, err
	}
	if out.LeftEncrypted, err = field("puzzleLeft"); err != nil {
		return nil, err
	}
	out.PicWidth = size("picWidth")
	out.PuzzleWidth = size("puzzleWidth")
	return &out, nil
}

// NewSendSmsCodeBodyWithSlider 是带滑块的发码请求体：在基础 body 上追加 `puzzleVerfycode`。
//
// 单独成一个函数而不是给 [NewSendSmsCodeBody] 加可选参数：
// 走滑块是**异常路径**（实测首次发码被风控拦下才走），
// 让它在调用点显式可见，比在每个调用点传 nil 更不容易埋掉。
func NewSendSmsCodeBodyWithSlider(
	platform PcPlatform, phone, random, sliderAnswer string,
) (SendSmsCodeBody, *core.Error) {
	body := NewSendSmsCodeBody(platform, phone, random)
	encoded, err := core.AESDynamicIVEncrypt(PuzzleAESKey, sliderAnswer, puzzleAESIV)
	if err != nil {
		return body, asCoreError(err, "加密滑块答案失败")
	}
	body.PuzzleVerfycode = &encoded
	return body, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令与滑块（客户端密码学）
// ─────────────────────────────────────────────────────────────────────────────

// PasswordHashPrefix 是口令哈希的前缀（客户端 `common/utils.js` 的 `HASHTEXT`）。
const PasswordHashPrefix = "fetion.com.cn:"

// PasswordHash 是账密登录 `dycpwd` 的取值：`SHA1("fetion.com.cn:" + 口令)` 的**大写** hex。
//
// 这不是"密码存储"意义上的哈希（固定前缀、无盐、SHA-1），
// 它只是移动服务端用来校验口令的协议字段。本系统自己的口令存储走
// `core.HashPassword`（argon2id + 随机盐），两者**不要混用**。
func PasswordHash(password string) string {
	return core.SHA1HexUpper([]byte(PasswordHashPrefix + password))
}

// PuzzleAESKey 是滑块答案的 AES 密钥（客户端 `encryptByAesCBC` 的 `aesKey`）。
const PuzzleAESKey = "CREATPUZZLE03F9A"

// puzzleAESIV 是滑块答案的 AES IV。
//
// **硬编码**，不是服务端下发的挑战值 —— 协议文档里曾把它推测为"服务端挑战标识"
// （因为两次请求间保持不变），从源码看它就是这一行常量。
var puzzleAESIV = [16]byte{'E', '5', '2', '6', 'E', '1', 'F', '3', 'F', '9', '7', '6', '5', '4', '4', '4'}

// ─────────────────────────────────────────────────────────────────────────────
// 响应语义
// ─────────────────────────────────────────────────────────────────────────────

// PollKind 是一次扫码轮询的类别。
//
// 与 [Admission] 同理：Go 没有 sum type，用"类别 + 字段"表达，
// 并在字段注释里写明"哪个类别下它才有效"。
type PollKind int

const (
	// PollNotScanned 是码还没被扫。
	PollNotScanned PollKind = iota
	// PollScanned 是已扫码，等用户在手机上确认 ——
	// 这是**唯一值得对外提示进度**的中间态。
	PollScanned
	// PollSessionMismatch 是 `sID` 不匹配：换码之后还在用旧 `sID` 轮询。
	// 属于调用方的状态错误。
	PollSessionMismatch
	// PollExpired 是二维码失效，必须重新出码。
	PollExpired
	// PollTooFrequent 是触发限流（`200059545 扫码登录过于频繁`）——
	// 该退避，不该立刻换码重试。
	PollTooFrequent
	// PollSuccess 是登录成功。
	PollSuccess
	// PollOther 是其它码：原样带回交给上层判断。
	PollOther
)

// PollOutcome 是一次扫码轮询的结果。
//
// 刻意**不把"未扫码"当成错误**：轮询期间绝大多数响应（实测 121/192）都是它，
// 当成错误会让调用方拿异常做流程控制，日志也会被刷满。
type PollOutcome struct {
	Kind PollKind
	// Encrypted 在 [PollSuccess] 下有效：`data` 原文（AES 密文），
	// 需交给 [LoginPayloadDecryptor]。
	Encrypted string
	// Code 在 [PollOther] 下有效：上游原样带回的码（`nil` 表示响应里没有 code）。
	Code *int64
	// Message 在 [PollOther] 下有效。
	Message string
}

// ClassifyPoll 把一次轮询响应归类。
//
// `success: false` 优先于 `code`（见 [Envelope.EnsureOK] 的说明的**反面**）：
// 这里**不复用 `EnsureOK`** —— 轮询的正常中间态本来就不是"成功"，
// 用同一套判定会把"未扫码"变成错误。
func ClassifyPoll(envelope *Envelope) PollOutcome {
	message, _ := envelope.Message()

	code, hasCode := envelope.Code()
	if !hasCode {
		return PollOutcome{Kind: PollOther, Message: message}
	}

	switch code {
	case CodeQRNotScanned:
		return PollOutcome{Kind: PollNotScanned}
	case CodeQRScanned:
		return PollOutcome{Kind: PollScanned}
	case CodeQRMismatch:
		return PollOutcome{Kind: PollSessionMismatch}
	case CodeQRExpired:
		return PollOutcome{Kind: PollExpired}
	case CodeQRTooFrequent:
		return PollOutcome{Kind: PollTooFrequent}
	}

	if IsSuccessCode(code) {
		payload, ok := envelope.Payload()
		if cipher, isStr := payload.(string); ok && isStr {
			return PollOutcome{Kind: PollSuccess, Encrypted: cipher}
		}
		// 成功码但没有密文载荷：如实报告"形状不对"，不要伪造一个空凭据。
		return PollOutcome{
			Kind:    PollOther,
			Code:    &code,
			Message: "登录成功但 data 不是密文字符串",
		}
	}

	return PollOutcome{Kind: PollOther, Code: &code, Message: message}
}

// IsSuccessCode 判断是不是成功码：`0000` 与 `200` 都见过
// （`code` 可能是字符串，[Envelope.Code] 已统一成数字）。
func IsSuccessCode(code int64) bool { return code == CodeOKZero || code == CodeOK200 }

// SendSmsKind 是一次"发送验证码"的类别。
type SendSmsKind int

const (
	// SmsSent 是已发出。
	SmsSent SendSmsKind = iota
	// SmsNeedSlider 是需要先过滑块：必须去取图。
	SmsNeedSlider
	// SmsSliderExhausted 是滑块挑战已作废（失败次数超限）：
	// **必须重新取图**，不能复用旧挑战。
	SmsSliderExhausted
	// SmsOther 是其它码，原样带回。
	SmsOther
)

// SendSmsOutcome 是一次"发送验证码"的结果。
type SendSmsOutcome struct {
	Kind SendSmsKind
	// RemainGetTimes 在 [SmsSent] 下有效：配额余量，调用方应当跟踪以免无谓消耗。
	//
	// ⚠ 用 `*int64` 而不是无符号窄类型：它是上游给的一个**计数**，
	// Go 里计数就是带符号的整型。收窄成 `uint32` 会让"某天上游给个负数"
	// 变成一次静默的绕回（4294967295 次余量），而那种错只在真跑时才看得见。
	RemainGetTimes *int64
	// CaptchaType 在 [SmsNeedSlider] / [SmsSliderExhausted] 下有效。
	CaptchaType *int64
	// Code / Message 在 [SmsOther] 下有效。
	Code    *int64
	Message string
}

// ClassifySendSms 把一次发码响应归类。
func ClassifySendSms(envelope *Envelope) SendSmsOutcome {
	captchaType := func() *int64 {
		payload, ok := envelope.Payload()
		if !ok {
			return nil
		}
		obj, ok := payload.(map[string]any)
		if !ok {
			return nil
		}
		n, ok := asInt64(obj["captchaType"])
		if !ok {
			return nil
		}
		return &n
	}

	code, hasCode := envelope.Code()
	if !hasCode {
		message, _ := envelope.Message()
		return SendSmsOutcome{Kind: SmsOther, Message: message}
	}

	if IsSuccessCode(code) {
		payload, ok := envelope.Payload()
		if ok {
			if obj, isObj := payload.(map[string]any); isObj {
				if n, ok := asInt64(obj["remainGetTimes"]); ok && n >= 0 {
					return SendSmsOutcome{Kind: SmsSent, RemainGetTimes: &n}
				}
			}
		}
		return SendSmsOutcome{Kind: SmsSent}
	}

	switch code {
	case CodeNeedSlider:
		return SendSmsOutcome{Kind: SmsNeedSlider, CaptchaType: captchaType()}
	case CodeSliderExhausted:
		return SendSmsOutcome{Kind: SmsSliderExhausted, CaptchaType: captchaType()}
	}

	message, _ := envelope.Message()
	return SendSmsOutcome{Kind: SmsOther, Code: &code, Message: message}
}

// ─────────────────────────────────────────────────────────────────────────────
// 成功响应的解密
// ─────────────────────────────────────────────────────────────────────────────

// LoginPayloadDecryptor 解密登录成功响应里的 `data`。
//
// 抽成接口的目的仍然是**可测**：真实路径走 [LoginDataDecryptor]，
// 而"上游把 `data` 换成另一套加密 / 干脆回明文"这种变化，
// 可以用别的实现验证调用方的行为，不需要联网也不需要真凭据。
type LoginPayloadDecryptor interface {
	// DecryptUserInfo 解密 `data` 密文并解析成 `userInfo` 结构。
	DecryptUserInfo(ciphertext string) (any, *core.Error)
}

// LoginDataDecryptor 是生产环境的登录数据解密：
// AES-128-**ECB**/PKCS7，密钥 `qPqDw263XgFgL3u8`。
//
// 密钥与模式来自客户端 `common/utils.js: decryptByAes`
// （见 test/golden/pcclient-crypto-vectors.json 的出处说明）。
type LoginDataDecryptor struct {
	key string
}

// NewLoginDataDecryptorProduction 用生产密钥构造。
func NewLoginDataDecryptorProduction() *LoginDataDecryptor {
	return &LoginDataDecryptor{key: LoginAESKey}
}

// NewLoginDataDecryptorTestEnv 用测试环境密钥构造。
//
// 客户端在 `systemEnvironment` 为 11 / 22 时会换这把 key，
// 拿生产 key 去打测试环境的表现是"解出来是乱码"。
func NewLoginDataDecryptorTestEnv() *LoginDataDecryptor {
	return &LoginDataDecryptor{key: LoginAESKeyTest}
}

// NewLoginDataDecryptorWithKey 指定密钥（测试用）。
func NewLoginDataDecryptorWithKey(key string) *LoginDataDecryptor {
	return &LoginDataDecryptor{key: key}
}

// DecryptUserInfo 实现 [LoginPayloadDecryptor]。
func (d *LoginDataDecryptor) DecryptUserInfo(ciphertext string) (any, *core.Error) {
	plain, err := core.AESECBPKCS7DecryptHex(d.key, ciphertext)
	if err != nil {
		return nil, asCoreError(err, "解密登录数据失败")
	}
	value, ok := core.ParseJSON(plain)
	if !ok {
		return nil, core.NewParse("userInfo 不是合法 JSON（密钥不对或用错模式都会落到这里）")
	}
	return value, nil
}

// LoginAESKey 是生产环境的登录数据 AES 密钥。
const LoginAESKey = "qPqDw263XgFgL3u8"

// LoginAESKeyTest 是测试环境的登录数据 AES 密钥（`systemEnvironment` 为 11 / 22）。
const LoginAESKeyTest = "2ErfJus1Ofr@2o24"

// ─────────────────────────────────────────────────────────────────────────────
// 内部辅助
// ─────────────────────────────────────────────────────────────────────────────

// asCoreError 把 core 里那些返回 `error` 的密码学函数统一成 `*core.Error`。
//
// ⚠ 用 [errors.As] 而不是裸类型断言：调用链上任何一处 `fmt.Errorf("%w")`
// 都会让裸断言**静默返回 false**，于是真实错误被换成一句笼统的话。
func asCoreError(err error, context string) *core.Error {
	var ce *core.Error
	if errors.As(err, &ce) {
		return ce
	}
	return core.NewParse(fmt.Sprintf("%s: %v", context, err))
}

func isAllASCIIDigits(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return len(s) > 0
}
