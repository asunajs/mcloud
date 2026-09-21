// 移动云盘 **App 端（手机客户端）验证码登录**。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-upstream/src/app_login.rs。
//
// # 规格来源
//
// 全部依据实测，不含猜测性字段：
//
//   - `docs/capture/app-sms-login-samples.txt`：iOS App 12.6.0 的完整登录抓包
//     （`getSmsCode` → 滑块 H5 → `thirdlogin` → 会话载荷）
//   - 反汇编 `MCLoginCryptoUtil` / `MCLoginService`（`mCloud_iPhone` 99.99% 未加密，
//     可直接 `otool -tV`）—— 加解密算法的唯一依据
//   - 活体探针：合成号码打真实上游，`9441` / `9103` 构成二值判别器
//
// # 与 PC 端（[login.go]）的关系：**同一个接口，两套形状**
//
// `POST /user/thirdlogin` 两端都调，但**没有一处是可以复用的**：
//
//	|              | PC 端          | App 端                          |
//	|--------------|----------------|--------------------------------|
//	| pintype      | 21 / 5 / 9     | **8**（短信也是 8，不是 5）     |
//	| clienttype   | 656 / 710      | **412**                        |
//	| cpid         | 277 / 332      | **55**                         |
//	| 请求体       | 明文 JSON      | **AES-128-ECB + 末 10 位错位 的 hex** |
//	| 响应 data    | AES-ECB hex    | AES-ECB hex **+ 错位**         |
//	| AES 密钥     | qPqDw263XgFgL3u8 | **lYzvvBMUjucw+h^R**        |
//	| extInfo.ifOpenAccount | **数字** 1 | **字符串** "1"              |
//	| getSmsCode 的 mode/reqType | **数字** | **字符串**                |
//
// 所以本模块**刻意不参数化平台**：把两端塞进一个泛型构造器，
// 只会让"传错端"变成一个能静默通过的选项（而这正好是最容易犯的错 ——
// 把 PC 的密钥搬到 App 路径上，上游会回 `9103`）。
//
// # 三层封装，第三层是 App 独有的
//
//	第 1 层  明文 JSON
//	第 2 层  AES-128-ECB/PKCS7  →  384 个 hex 字符
//	第 3 层  末 10 个字符移到最前（错位）        ← App 独有，PC 没有
//
// 第 3 层是 `MCLoginCryptoUtil +privateEncrypt:` 里那两行
// `substringFromIndex:` / `substringToIndex:` 拼出来的。它**不是加密**，
// 只是一次字符串旋转 —— 但少做这一步，服务端会回 `9103 输入参数不合法`，
// 因为服务端要先反旋转再解密。
//
// # 验证码这条链路：`puzzleVerfycode` **就是纯协议入口**（2026-09-19 补测更正）
//
// 早先抓包看到三条 `getSmsCode` 的 `puzzleVerfycode` **全是空串**，
// 于是本模块一度只把它做成"可传入的字符串参数、不做任何编码"，并留下一条
// "App 端没有协议面、只能开 H5" 的结论。**那条结论是错的**，错在把
// **客户端行为**当成了**协议能力**：
//
//   - App 客户端确实**选择**了开 WebView（`200059554` 之后不重发 `getSmsCode`，
//     短信由那个 H5 自己发），但那是它的实现选择，不是服务端的限制；
//   - 实测：把这个字段填成非空，服务端**就不再回 `200059554`** —— 它确实会处理它；
//   - 而**第一方滑块取图接口就在同一台主机上**（[BaseUserYun]）：
//     `POST /user/verfycode {"account":…,"type":2}`，**裸请求即可**
//     （不需要 `Authorization`、不需要设备头、不需要 `x-yun-tid`）。
//
// 所以完整通路是（实测跑通，返回 `success:true` + `remainGetTimes` 递减）：
//
//	getSmsCode(空) → 200059554
//	  → POST /user/verfycode 取图
//	  → puzzleLeft 用公开常量 CREATPUZZLE03F9A 解开 = 拖动坐标
//	  → getSmsCode(puzzleVerfycode = PC 那套编码的坐标) → 0000，短信已发
//
// **编码与 PC 端完全一样**，所以这不是新协议，是同一个第一方滑块，
// 只是 `clientType` 与请求头换成 App 的。
//
// ⚠ 正因如此，别把这一节读成"App 端的滑块已实现自动化"：过掉的是**借来的** PC 那套
// 取题与编码。App 自己那条路（原生开 WebView、短信由 H5 发）没有做自动化，
// 真机走的仍是它 —— 上面被推翻的只是"**协议**上不允许"那一层，
// "真机在开 H5"这个观察从来没错过。
//
// H5（[CaptchaH5URL]）退化为**兜底**：协议路径被限流时才交还给用户。
//
// ## 两个 IV 不是一回事（最容易搞错的地方）
//
//   - **取题**响应的 `puzzleLeft`：**动态 IV**，就藏在它自己的前 16 字节里；
//   - **提交**答案的 `puzzleVerfycode`：**固定 IV** `E526E1F3F9765444`（[PuzzleAESIV]）。
//
// 实测把提交侧也换成随机 IV 会失败。
//
// ## 这个接口**计失败次数**
//
// `200059553`（[CodeSliderExhausted]）就是"该挑战作废、重新取图"的信号。
// 所以**不能拿伪造答案反复试探** —— 补测时踩到过一次。
// 每次都必须取**新题**并用**解出来的真坐标**。
package upstream

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 密码学
// ─────────────────────────────────────────────────────────────────────────────

// AppLoginAESKey 是 App 端登录体的 AES 密钥（`isCurrentEnvironment = YES` 分支）。
//
// 反汇编实证：`+[MCLoginCryptoUtil AESEncrypt:isCurrentEnvironment:]` 里是
// `csel x20, keyA, keyB, ne` 选出来的**字面量**，不是从 `SecretKeyConfig` 取的
// —— 所以它在 App 包里改名字也不影响我们，改的是这个常量本身。
const AppLoginAESKey = "lYzvvBMUjucw+h^R"

// AppLoginAESKeyAlt 是备用密钥（`isCurrentEnvironment = NO` 分支）。
//
// **它不是"测试环境密钥"**：客户端只是按一个布尔位二选一，
// 实测生产流量走的是 [AppLoginAESKey]。留它是因为
// "用错这把会得到 9103"是排查密钥问题最快的判据。
const AppLoginAESKeyAlt = "+09T5lHhO+smS7x@"

// AppLoginCrypto 是 App 端登录体的加解密器：AES-128-ECB/PKCS7 + 末 10 位错位。
//
// 与 PC 端的 [LoginDataDecryptor] **不通用** —— 它多了错位那一层，且密钥不同。
// 刻意做成两个类型，就是为了让"用错端的解密器"在编译期就不可能发生。
type AppLoginCrypto struct {
	key string
}

// NewAppLoginCryptoProduction 用生产密钥构造（实测流量走这把）。
func NewAppLoginCryptoProduction() *AppLoginCrypto { return &AppLoginCrypto{key: AppLoginAESKey} }

// NewAppLoginCryptoAltEnv 用备用密钥分支构造。
func NewAppLoginCryptoAltEnv() *AppLoginCrypto { return &AppLoginCrypto{key: AppLoginAESKeyAlt} }

// NewAppLoginCryptoWithKey 用指定密钥构造（测试用）。
func NewAppLoginCryptoWithKey(key string) *AppLoginCrypto { return &AppLoginCrypto{key: key} }

// Key 返回当前密钥。
func (c *AppLoginCrypto) Key() string { return c.key }

// EncryptBody 明文 → 线上请求体（hex 字符串）。
func (c *AppLoginCrypto) EncryptBody(plaintext string) (string, *core.Error) {
	ecb, err := core.AESECBPKCS7EncryptHex(c.key, plaintext)
	if err != nil {
		return "", asCoreError(err, "加密登录体失败")
	}
	return core.ShiftTail10ToFront(ecb), nil
}

// DecryptBody 线上响应 `data` → 明文。
//
// 两步都**必须**成功：错位不改变字符集，所以"只反错位不解密"也能得到一个
// 看起来正常的 hex 串 —— 那种情况下会解出乱码而不是报错，
// 所以这里靠 AES 的 PKCS7 校验来兜底（解错必然填充非法）。
func (c *AppLoginCrypto) DecryptBody(wireHex string) (string, *core.Error) {
	ecb := core.ShiftHead10ToBack(strings.TrimSpace(wireHex))
	plain, err := core.AESECBPKCS7DecryptHex(c.key, ecb)
	if err != nil {
		return "", asCoreError(err, "解密登录体失败")
	}
	return plain, nil
}

// DecryptUserInfo 实现 [LoginPayloadDecryptor]。
func (c *AppLoginCrypto) DecryptUserInfo(ciphertext string) (any, *core.Error) {
	plain, perr := c.DecryptBody(ciphertext)
	if perr != nil {
		return nil, perr
	}
	value, ok := core.ParseJSON(plain)
	if !ok {
		return nil, core.NewParse(
			"App 端登录载荷不是合法 JSON（密钥不对或漏了错位都会落到这里）")
	}
	return value, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求体（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// SendSmsRandom 是 `getSmsCode` 请求体里的 `random`：**客户端硬编码的常量**。
//
// 这不是"一个随机值的示例"，而是实测确认的**固定字面量**：
// `FXTrace` 日志里跨 2026-08-12 ~ 09-18 的全部 4 次 `getSmsCode` 请求，
// `random` 都是同一个 `qwert`（见 `docs/capture/app-sms-login-samples.txt` §1）。
// 一个真正随机的值不可能跨一个月每次相同。
//
// 所以它**不做成参数** —— 参数会暗示"这里可以也应该变化"，
// 而实际上客户端从不变化它。要改这个值，应当先有新的抓包证据。
const SendSmsRandom = "qwert"

// AppSendSmsCodeBody 是 `/user/sms/getSmsCode` 的请求体。
//
// 三个**易错点**，全都在抓包里能对上：
//
//  1. **所有值都是字符串**（PC 端这里 `mode` / `reqType` 是数字）——
//     `"mode":"0"` 不是 `"mode":0`；
//  2. `puzzleVerfycode` 字段名是 `verfy` 不是 `verify`（与 PC 端同款拼写错误）；
//  3. 首次请求它是**空串**，不是 `null`、也不是省略字段。
//
// `random` 固定为 [SendSmsRandom]（理由见该常量的文档）。
// **注意它与 `thirdlogin` 的 `random` 不是同一个值** —— 后者是每请求随机的 6 字符。
//
// ⚠⚠ 字段**按 JSON 键字典序**声明，不是按 `json!` 宏里的书写顺序 ——
//
// Rust 侧 `body.to_string()` 走的是 `serde_json` 的 Map（**BTreeMap，默认按键排序**；
// 本仓库没有开 `preserve_order` 特性），所以线上字节是**字典序**的；
// 而 Go 的 struct 按**声明顺序**输出。顺手照抄 `json!` 里那几行的书写顺序，
// 两边就会发出字节不同的请求体 —— 服务端不校验键序（活体探针验证过两种键序都回 `9441`），
// 所以这种不一致**不会报错**，只会让两边的对拍无法直接比整串。
//
// ⇒ 本包所有进请求体的结构体都按字典序声明字段（与 `app_signin` 那批 body 同一约定）。
type AppSendSmsCodeBody struct {
	ClientType      string `json:"clientType"`
	Mode            string `json:"mode"`
	PhoneNumber     string `json:"phoneNumber"`
	PuzzleVerfycode string `json:"puzzleVerfycode"`
	Random          string `json:"random"`
	ReqType         string `json:"reqType"`
}

// NewAppSendSmsCodeBody 构造发码请求体。
func NewAppSendSmsCodeBody(phone, puzzleVerfycode string) AppSendSmsCodeBody {
	return AppSendSmsCodeBody{
		Mode:            "0",
		PhoneNumber:     phone,
		ReqType:         "3",
		ClientType:      IOSClientType,
		Random:          SendSmsRandom,
		PuzzleVerfycode: puzzleVerfycode,
	}
}

// NewAppSendSmsCodeBodyWithSlider 是带滑块的发码请求体：在基础 body 上填上
// **已编码**的滑块答案。
//
// # 为什么直接复用 PC 端的编码（[PuzzleAESKey] + [PuzzleAESIV]）
//
// 因为实测证明这就是**同一个第一方滑块**：取图接口是同一个 `/user/verfycode`、
// 同一台主机，编码出来也是同样的 `base64(固定IV ‖ AES-128-CBC/PKCS7)`。
// 2026-09-19 实测：取新题解出坐标 `466`、按这套编码填进去，
// 上游回 `{"success":true,"code":"0000","data":{"remainGetTimes":8,…}}`，短信真的发出去了。
//
// 单独成函数而不是给 [NewAppSendSmsCodeBody] 加参数，理由与 PC 端同款：
// 走滑块是**异常路径**，让它在调用点显式可见比埋一个 `Option` 更难忽略。
func NewAppSendSmsCodeBodyWithSlider(phone, sliderAnswer string) (AppSendSmsCodeBody, *core.Error) {
	encoded, err := core.AESDynamicIVEncrypt(PuzzleAESKey, sliderAnswer, PuzzleAESIV)
	if err != nil {
		return AppSendSmsCodeBody{}, asCoreError(err, "加密滑块答案失败")
	}
	return NewAppSendSmsCodeBody(phone, encoded), nil
}

// AppThirdLoginBody 是 `/user/thirdlogin` 的请求体（**明文**，
// 加密由 [AppLoginCrypto.EncryptBody] 负责）。
//
// 字段表来自抓包（`docs/capture/app-sms-login-samples.txt` §3.1），逐字段：
//
//	| 字段                    | 值          | 备注 |
//	|-------------------------|-------------|------|
//	| cpid                    | "55"        | App 端固定 |
//	| clienttype              | "412"       | **全小写 t**，与 getSmsCode 的 clientType 大小写不同 |
//	| pintype                 | "8"         | **短信登录也是 8**，不是 PC 的 5 |
//	| dycpwd                  | 6 位验证码明文 | 不做哈希（账密登录才哈希） |
//	| loginMode / verType     | "0" / "1"   | App 端独有 |
//	| extInfo.ifOpenAccount   | "1"         | **字符串**，PC 端这里是数字 |
//	| version                 | "12.6.0"    | 与设备串第 4 段一致 |
//	| random                  | 6 字符      | 与 getSmsCode 的 random **不是同一个值** |
//
// ⚠⚠ 字段**按 JSON 键字典序**声明（理由见 [AppSendSmsCodeBody] 那条警告）。
// 键序与客户端字典序不同（`serde_json` 默认按键排序）—— 实测这不影响服务端：
// 服务端是"先解密再解析 JSON"，键序不参与任何校验（活体探针验证过两种键序都回 `9441`）。
//
// ⚠ 别把抓包里那段明文（`docs/capture/app-sms-login-samples.txt` §3.1 的字面量顺序，
// 也是 Rust 侧测试常量 `APP_LOGIN_PLAINTEXT` 的顺序）当成线上字节 ——
// 那是**客户端**的顺序，不是 `serde_json` 输出的顺序。两个顺序长度相同（都是 186），
// 所以"长度 = 186"这条自检**发现不了**这个差异。
type AppThirdLoginBody struct {
	ClientType string         `json:"clienttype"`
	Cpid       string         `json:"cpid"`
	Dycpwd     string         `json:"dycpwd"`
	ExtInfo    AppExtInfoBody `json:"extInfo"`
	LoginMode  string         `json:"loginMode"`
	Msisdn     string         `json:"msisdn"`
	Pintype    string         `json:"pintype"`
	Random     string         `json:"random"`
	VerType    string         `json:"verType"`
	Version    string         `json:"version"`
}

// AppExtInfoBody 是 `extInfo`。
//
// ⚠ `ifOpenAccount` 在 App 端是**字符串** `"1"`，PC 端是数字 `1` ——
// 这是两端"同一个接口两套形状"里最容易漏的一处。
type AppExtInfoBody struct {
	IfOpenAccount string `json:"ifOpenAccount"`
}

// NewAppSmsThirdLoginBody 构造短信登录的明文请求体。
func NewAppSmsThirdLoginBody(phone, smsCode, random string) AppThirdLoginBody {
	return AppThirdLoginBody{
		Cpid:       IOSCpid,
		ExtInfo:    AppExtInfoBody{IfOpenAccount: "1"},
		Dycpwd:     smsCode,
		Pintype:    "8",
		LoginMode:  "0",
		VerType:    "1",
		Version:    IOSAppVersion,
		Msisdn:     phone,
		Random:     random,
		ClientType: IOSClientType,
	}
}

// LoginRandomAlphabet 是登录体 `random` 的字符集（大小写字母 + 数字）。
const LoginRandomAlphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"

// NewLoginRandom 生成登录体里的 `random`：6 位大小写字母 + 数字。
//
// 形状取自实测（`hOjseB`）。它**不参与任何可观测的校验** ——
// 依据是：`getSmsCode` 那个同名字段是硬编码常量 [SendSmsRandom]
// （`qwert`），服务端照样正常受理，说明这个字段只被串联/回显。
// 所以这里不追求"某个精确长度"，只保证字符集与量级一致。
//
// 但要诚实记一笔：**`thirdlogin` 的 `random` 只有 1 个样本**，
// "每次不同"是从"客户端为什么要随机"推的，不是实测到的。
// 它是否为定值，需要第二次登录抓包才能确认。
func NewLoginRandom(rng core.Random) string {
	var bytes [6]byte
	rng.FillBytes(bytes[:])
	out := make([]byte, 0, len(bytes))
	for _, b := range bytes {
		out = append(out, LoginRandomAlphabet[int(b)%len(LoginRandomAlphabet)])
	}
	return string(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// 滑块验证码 H5
// ─────────────────────────────────────────────────────────────────────────────

// CaptchaH5Base 是滑块验证码 H5 的基址（实测）。
const CaptchaH5Base = "https://yun.139.com/fe/captcha/"

// CaptchaH5URL 是滑块验证码 H5 的落地页。
//
// # 它为什么是个纯函数
//
// 与 PC 端的二维码一样（见 [QrPayload]），这个 URL 是
// **客户端本地拼装**的，零网络请求：`200059554` 之后 App 直接开 WebView 加载它。
//
// # `data` 参数里有什么
//
// `base64(JSON)`，JSON 是三段：
//
//	{
//	  "headers": { "x-yun-app-channel": "10000020",
//	               "x-yun-client-info": "1|192.168.3.179|2|12.6.0|…|" },
//	  "captchaType": 2,
//	  "params": { "clientType": "412", "reqType": "3",
//	              "nationCode": "+86", "phoneNumber": "19900000000" }
//	}
//
// 关键结论：**H5 页自己带着完整请求头和手机号**，也就是说
// "过滑块"与"发短信"是 H5 会话内完成的，原生侧只等回调。
// 这就是为什么抓包里 `200059554` 之后**没有**第二次 `getSmsCode`。
//
// 注意 `headers` 里**只有两个头**（`x-yun-app-channel` 与 `x-yun-client-info`），
// 不是 [AppProfile.Headers] 的全集 —— 别顺手把整个头集合塞进去。
func CaptchaH5URL(profile *AppProfile, phone string) string {
	payload := map[string]any{
		"headers": map[string]any{
			"x-yun-app-channel": IOSChannel,
			// 第 14 段（`x-DeviceName` 的 base64）之后的 `|0|;` 在这里**没有**，
			// 与 x-yun-client-info 请求头不同 —— 抓包原样如此。
			"x-yun-client-info": clientInfoForCaptcha(profile.Device()),
		},
		"captchaType": 2,
		"params": map[string]any{
			"clientType":  IOSClientType,
			"reqType":     "3",
			"nationCode":  "+86",
			"phoneNumber": phone,
		},
	}
	buf, err := core.MarshalJSON(payload)
	if err != nil {
		// 上面全是字面量与字符串，序列化不可能失败；
		// 真失败了也不该静默给出一个缺 `data` 的 URL —— 那会让人以为是"滑块服务挂了"。
		return CaptchaH5Base + "?data="
	}
	return CaptchaH5Base + "?data=" + core.B64Encode(buf)
}

// clientInfoForCaptcha 是 H5 的 `data` 里那个 `x-yun-client-info`。
//
// 与 [AppDevice.ClientInfo] 的唯一差别是**末尾的 `0|;` 没有了**（`|` 留着）——
// 抓包原值是 `…|aVBhZA==|`，而请求头里是 `…|aVBhZA==|0|;`。
// 这个差异很小但真实存在，所以单列一个函数而不是加个布尔开关
// （开关会在调用点被传错，而这种差异一旦传错就是"滑块过不去"且看不出原因）。
func clientInfoForCaptcha(device AppDevice) string {
	full := device.ClientInfo()
	return strings.TrimSuffix(full, "0|;")
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求构造
// ─────────────────────────────────────────────────────────────────────────────

// AppSendSmsCodeRequest 构造 `getSmsCode` 请求（**明文 JSON**，这一条不加密）。
//
// # 为什么**不发** `hcy-cool-flag`
//
// 抓包里 App 是发的（`hcy-cool-flag: 1`，三条请求都有）。但实测发现：
// **这个头只要出现，服务端就把响应换成加密体** ——
//
//	| 请求                | 响应 |
//	|---------------------|------|
//	| 不带 hcy-cool-flag  | `{"success":false,"code":"200059554",…}` |
//	| hcy-cool-flag: 1    | `"<192 个 base64 字符>"`（裸 JSON 字符串，144 字节密文） |
//	| hcy-cool-flag: 0    | 同上 —— 只要**出现**就切，与取值无关 |
//
// 那 144 字节既不是 `MCLoginCryptoUtil` 那套（试过错位前后）、也不是
// `MCStateCryptoHelper` 的 SM4-ECB（带上我们自己的 `yun-encrypt-*` 会话密钥试过，
// 解不出来），响应头里也没有任何密钥 —— 密钥来源未知。
//
// 所以这里**主动不发**这个头：发一个自己兑现不了的能力声明，换来的是
// "服务端回了一个本客户端解不开的响应"，而它在**不发**时的语义与抓包完全一致
// （同一个 `200059554`）。这是"照抄抓包"与"能跑起来"冲突时唯一说得通的选择。
//
// 待办：等拿到 App 解这类响应时的密钥来源（大概率与 `MCStateCryptoHelper`
// 的 `keyStatus`/`hadCache` 有关），再把开关打开。
func AppSendSmsCodeRequest(
	profile *AppProfile, timestamp int64, body any,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/user/sms/getSmsCode", BaseUserYun)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithAppHeaders(request, profile, timestamp), nil
}

// AppSliderPuzzleRequest 构造 `/user/verfycode` 请求（**明文**）：取**第一方**滑块拼图。
//
// # 为什么 App 端也用它（而不是去开那个 H5）
//
// 实测：这个接口就在 **App 自己的主机**上（[BaseUserYun] 与 `getSmsCode` 是同一台），
// 而且**裸请求就能用** —— 不需要 `Authorization`、不需要设备头、不需要 `x-yun-tid`。
// 这里仍然叠上 App 的头，是因为**取题与提交答案必须用同一套头**才稳妥，
// 而提交那一步（`getSmsCode`）本来就要带全套 App 头。
//
// 路径与字段名都没有 `i`：是 `verfycode` 不是 `verifycode` —— 上游就这么拼的
// （与 PC 端同一个接口，所以直接复用 [NewSliderPuzzleBody]）。
func AppSliderPuzzleRequest(
	profile *AppProfile, timestamp int64, phone string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/user/verfycode", BaseUserYun)).WithJSONBody(NewSliderPuzzleBody(phone))
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithAppHeaders(request, profile, timestamp), nil
}

// AppThirdLoginRequest 构造 `thirdlogin` 请求（**加密体**）。
//
// 传进来的是明文，加密在这里完成 —— 这样"忘了加密"在类型层面
// 就不可能发生（PC 端那个 `ThirdLoginRequest` 收的是已经构造好的 body，
// 因为它不需要加密）。
//
// 注意请求体的 Content-Type 仍然是 `application/json;charset=UTF-8`，
// 尽管内容是 hex 文本 —— 抓包原样如此，别"顺手改成 text/plain"。
func AppThirdLoginRequest(
	profile *AppProfile, timestamp int64, crypto *AppLoginCrypto, body any,
) (core.HTTPRequest, *core.Error) {
	plaintext, err := core.MarshalJSON(body)
	if err != nil {
		return core.HTTPRequest{}, asCoreError(err, "序列化登陆体失败")
	}
	wire, perr := crypto.EncryptBody(string(plaintext))
	if perr != nil {
		return core.HTTPRequest{}, perr
	}
	request := WithAppHeaders(
		core.NewRequestPost(fmt.Sprintf("%s/user/thirdlogin", BaseUserYun)),
		profile, timestamp)
	request.Body = []byte(wire)
	return request, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应语义
// ─────────────────────────────────────────────────────────────────────────────

// AppSendSmsKind 是一次"发送验证码"的结果分类。
type AppSendSmsKind int

const (
	// AppSmsSent 是已发出。
	AppSmsSent AppSendSmsKind = iota
	// AppSmsNeedCaptcha 是需要先过滑块拼图。
	//
	// **这是首次发码的常态**，不是错误：客户端应当接着去取图、解出坐标、
	// 带答案重发（[NewAppSendSmsCodeBodyWithSlider]）。只有当那条路也走不通
	// （见 [AppSmsSliderExhausted]）时，才把 H5 地址交给用户。
	AppSmsNeedCaptcha
	// AppSmsSliderExhausted 是 `200059553 同一滑动拼图验证码在单位时间内校验失败次数超限`
	// —— **这个挑战作废了，必须重新取图**。
	//
	// 实测语义（补测时亲手撞到）：它**不是**"你这个账号被限流了"，
	// 而是"这一道题的尝试次数用完了"。所以正确的反应是重取一道新题，
	// 而不是放弃、也不是拿旧答案再试一次。
	AppSmsSliderExhausted
	// AppSmsOther 是其余。
	AppSmsOther
)

// AppSendSmsOutcome 是一次"发送验证码"的结果。
//
// **刻意不复用 PC 端的 [SendSmsOutcome]**：两端的 `NeedSlider` 名字一样，
// **下一步动作完全不同** ——
//
//   - PC 端：去调 `POST /user/verfycode` 取图，本地识别后把答案
//     AES-CBC 加密成 `puzzleVerfycode` 再重发 `getSmsCode`；
//   - App 端：**开 WebView** 加载 [CaptchaH5URL]，然后等一个回调
//     （发短信是 H5 干的，原生侧不会重发 `getSmsCode`）。
//
// 共用一个枚举会让调用方写错分支却仍然编译通过。
type AppSendSmsOutcome struct {
	// Kind 是分类。
	Kind AppSendSmsKind
	// RemainGetTimes 是配额余量（仅 [AppSmsSent] 有意义）。
	RemainGetTimes *int64
	// CaptchaType 是 `data.captchaType`（仅滑块两档有意义）。
	CaptchaType *int64
	// Code 是上游业务码（仅 [AppSmsOther] 有意义）。
	Code *int64
	// Message 是上游 msg。
	Message string
}

// ClassifyAppSendSms 归类 `getSmsCode` 的响应。
func ClassifyAppSendSms(envelope *Envelope) AppSendSmsOutcome {
	code, _ := envelope.Code()
	message, _ := envelope.Message()
	captchaType := func() *int64 {
		payload, _ := envelope.Payload()
		obj, _ := payload.(map[string]any)
		return optInt64Field(obj, "captchaType")
	}

	switch code {
	case CodeOKZero, CodeOK200:
		payload, _ := envelope.Payload()
		obj, _ := payload.(map[string]any)
		return AppSendSmsOutcome{
			Kind:           AppSmsSent,
			RemainGetTimes: optInt64Field(obj, "remainGetTimes"),
		}
	case CodeNeedSlider:
		return AppSendSmsOutcome{Kind: AppSmsNeedCaptcha, CaptchaType: captchaType()}
	case CodeSliderExhausted:
		return AppSendSmsOutcome{Kind: AppSmsSliderExhausted, CaptchaType: captchaType()}
	}
	return AppSendSmsOutcome{Kind: AppSmsOther, Code: &code, Message: message}
}

// AppLoginKind 是一次 App 端登录的结果分类。
type AppLoginKind int

const (
	// AppLoginSuccess 是登录成功，`Encrypted` 是 `data` 原文（密文）。
	AppLoginSuccess AppLoginKind = iota
	// AppLoginSmsCodeWrong 是 `9441 验证码不正确` —— **参数已被正确解析**，
	// 只是码不对/过期。
	//
	// 这个码是排查加密问题最有用的信号：看到它就说明密钥、模式、错位方向、
	// hex 大小写全都对了。
	AppLoginSmsCodeWrong
	// AppLoginUndecryptable 是 `9103 输入参数不合法` —— 服务端**解不开**请求体。
	//
	// 实测的三种成因（都亲手撞过）：体没加密、密钥用成了备用那把、
	// 错位方向写反。排查顺序就按这个来。
	AppLoginUndecryptable
	// AppLoginFailed 是 `9999 登录失败！` —— 设备头缺失时的表现（PC 端实测同款）。
	AppLoginFailed
	// AppLoginOther 是其余。
	AppLoginOther
)

// AppLoginOutcome 是一次 App 端登录的结果。
type AppLoginOutcome struct {
	// Kind 是分类。
	Kind AppLoginKind
	// Encrypted 是 `data` 原文（密文），交给 [AppLoginCrypto] 解（仅成功时有意义）。
	Encrypted string
	// Code 是上游业务码（仅 [AppLoginOther] 有意义）。
	Code *int64
	// Message 是上游 msg 或本层给出的形状说明。
	Message string
}

// ClassifyAppLogin 归类 `thirdlogin` 的响应。
func ClassifyAppLogin(envelope *Envelope) AppLoginOutcome {
	code, hasCode := envelope.Code()
	message, _ := envelope.Message()
	if hasCode && IsSuccessCode(code) {
		payload, _ := envelope.Payload()
		cipher, ok := payload.(string)
		if !ok {
			// 成功码但载荷不是密文字符串：如实报告形状不对，不伪造空凭据
			return AppLoginOutcome{
				Kind:    AppLoginOther,
				Code:    &code,
				Message: "登录成功但 data 不是密文字符串",
			}
		}
		return AppLoginOutcome{Kind: AppLoginSuccess, Encrypted: cipher}
	}
	if !hasCode {
		return AppLoginOutcome{Kind: AppLoginOther, Message: message}
	}
	switch code {
	case CodeSmsCodeWrong:
		return AppLoginOutcome{Kind: AppLoginSmsCodeWrong, Code: &code, Message: message}
	case CodeUndecryptableBody:
		return AppLoginOutcome{Kind: AppLoginUndecryptable, Code: &code, Message: message}
	case CodeLoginFailed:
		return AppLoginOutcome{Kind: AppLoginFailed, Code: &code, Message: message}
	}
	return AppLoginOutcome{Kind: AppLoginOther, Code: &code, Message: message}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送
// ─────────────────────────────────────────────────────────────────────────────

// AppSendSmsCode 是 App 端发送短信验证码（明文体）。
func (u *Upstream) AppSendSmsCode(
	ctx context.Context, profile *AppProfile, timestamp int64, body any,
) (*Envelope, *core.Error) {
	request, perr := AppSendSmsCodeRequest(profile, timestamp, body)
	if perr != nil {
		return nil, perr
	}
	return u.Call(ctx, request)
}

// AppFetchSliderPuzzle 是 App 端取**第一方**滑块挑战（`/user/verfycode`，明文）。
//
// 解坐标不需要图像识别：上游把缺口横坐标加密后直接给了出来，
// 见 [SliderPuzzle.Answer]。
func (u *Upstream) AppFetchSliderPuzzle(
	ctx context.Context, profile *AppProfile, timestamp int64, phone string,
) (*SliderPuzzle, *core.Error) {
	request, perr := AppSliderPuzzleRequest(profile, timestamp, phone)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseSliderPuzzle(envelope)
}

// AppThirdLogin 是 App 端登录（`thirdlogin`，加密体）。
func (u *Upstream) AppThirdLogin(
	ctx context.Context,
	profile *AppProfile,
	timestamp int64,
	crypto *AppLoginCrypto,
	body any,
) (*Envelope, *core.Error) {
	request, perr := AppThirdLoginRequest(profile, timestamp, crypto, body)
	if perr != nil {
		return nil, perr
	}
	return u.Call(ctx, request)
}
