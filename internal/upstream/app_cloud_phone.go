// 红包派对（云机）族：`cpactiv.buy.139.com/cloudphone-market/*` 的统一入口、鉴权头、
// 信封判据，以及**登录 / 余额 / 播报**这三条有实拍的口。
// 任务清单与任务动作在 [`app_cloud_phone_task.go`]（app_cloud_phone_task.go），
// 云机与安装、答题在 [`app_cloud_phone_install.go`]（app_cloud_phone_install.go）。
//
// 对应旧实现里的 `cloud-phone-redpack` 那一条活动任务
// （`docs/business-feature-inventory.md` §2.4 第 367–380 行；产出是**钱**、单位分）。
//
// ⚠⚠ 本层只做**上游层**：请求构造 + 响应解析。
// "跑一个账号"那条链（先查清单 → 逐任务判状态 → 动作 → 复查余额）与能力注册**都不在这里**，
// 所以本文件里没有任何"该不该发这一枪"的业务判断 —— 那是编排层的活。
//
// # 这一族有哪些实拍（说清"39 条里只有 10 条是本族"）
//
// 夹具 `test/fixtures/http/红包派对.json` 是**整个会话**的流量，不是只有活动页：
// 39 条里落在 `cpactiv.buy.139.com` 上的只有第 **29/30/31/32/33/34/35/36/37/38** 条，
// 逐条是
//
//	29 user/tokenValidate          32 redpacket/configTaskList
//	30 activity/common/ruleInfo    35 redpacket/userAccountInfo
//	31 bilog/upload                36 redpacket/configTaskLoginList
//	33 activity/common/ruleInfo    37 redpacket/userToastInfo
//	34 bilog/upload                38 rule/config/list
//
// 其余 29 条是**同一台 App 在同期发的别的域**（`cloud.139.com/uniphone/*`、
// `cloud.139.com/community/*`、`orches.*`、`ai.*`、`ucp.*`）⇒ 它们**不是**本族的样本，
// 连"同一次抓包"这件事都不构成把它们外推过来的理由。最险的一处就在这里：
//
//   - 第 1 条 `POST cloud.139.com/uniphone/user/tokenValidate`
//     body `{"pintype":5,"token":"…","version":"1.2"}`、响应 `header.appId` = `3099801`；
//   - 第 29 条 `POST cpactiv.buy.139.com/cloudphone-market/user/tokenValidate`
//     body `{"deviceId":"…","loginConfig":"redpacketLoginConfig","pintype":13,"token":"…","version":"1.0"}`、
//     响应 `header.appId` = `12345681`。
//
// 两条**尾段同名**而主机、`pintype`、`version`、`loginConfig` 有无、`appId` **全不一样**
// （`AGENTS.md` §1.12 事实②说的就是这种）。⇒ 本族只认第 29 条那一形状，
// 第 1 条那一份的字段表**一条都不许搬进来**（连 `expireTime` 都不能：见 [CloudPhoneTokenValidate] 那一段）。
//
// # 零样本清单（旧 TS 有、这批夹具没抓到 —— 照样实现，但形状只来自成文）
//
// `userSign`、`userCompleteTask`、`userBrowse`、`configAppList`、`gethwToken`、
// `instanceApkInstallStatus`、`apkInstall`、`appInstallTaskStatus`、
// `thirdQueryUserRelationshipV2`、`configTopicList`、`userTopicAnswer`、`getRcsToken`
// —— 对这 12 个路径名在**整份夹具**里 grep 过，命中 0（`getRcsToken` 那 4 处命中全在
// `cloud.139.com/uniphone/user/getRcsToken` 上，是**另一个主机、另一个 appId**，见
// `app_cloud_phone_install.go` 文件头）。每个函数自己的注释都另写一遍"零样本"，
// 因为将来改动的人未必读得到这里。
//
// # ⚠ 复核结果与成文不一致的一处：`version` **不是**全族统一
//
// 成文 §2.4 第 370 行写"body `{version:"SYS_CONFIG_Y", ...data}`"。逐条核过夹具：
//
//   - `redpacket/configTaskList`、`configTaskLoginList`、`userToastInfo`：
//     body 逐字 `{"version":"SYS_CONFIG_Y"}` ✅；
//   - `redpacket/userAccountInfo`：`{"platformType":1,"version":"SYS_CONFIG_Y"}` ✅（多一个键）；
//   - `user/tokenValidate`：**`"version":"1.0"`**（不是 SYS_CONFIG_Y！第 29 条逐字）；
//   - `activity/common/ruleInfo`、`rule/config/list`、`bilog/upload`：**根本没有 `version` 这个键**。
//
// ⇒ 所以本族**不能**做成"一个统一 body 拼法打所有口"：那会给三条不该带的口凭空加上
// `version`，又给 tokenValidate 送错版本号。[CloudPhoneRequest] 因此只服务"成文那一类"
// （`redpacket/*`），其余各自写死。
//
// # 复核不到的东西（**没有一条被验过，别当结论用**）
//
//   - [CloudPhoneSignSalt]：成文 §2.4 第 371 行说它等于 `md5("123456")`。夹具里 `sign`
//     头值逐条 `<REDACTED>`（脱敏器按头名遮的，头**名**可信、**值**不可信），
//     ⇒ 这条**只来自成文**，本仓库没验过。怎么验：抓一条 `sign`/`requestId`/`timestamp`/`token`
//     四个头都**未脱敏**的请求，把四个值按算式拼一遍比对。
//   - `timestamp` 头的单位（秒还是毫秒）：**零证据** —— 值被遮了，成文也只写了变量名。
//     所以它是**调用方给的字符串**，本层不格式化（见 [CloudPhoneAuth]）。
//   - `requestId` 的形状：成文给了 `YYYYMMDDHHmmss + Date.now() + random36(8)`，
//     而**响应** `header.requestId` 第 31/34 条是 `2071905980258680833` 这种 19 位纯数字。
//     ⇒ 两种读法都自洽（服务端自己生成的 id / 回显客户端 id），夹具分不开：
//     请求侧那个头在十条样本里**全部**被遮。若它是回显，成文那串形状就是错的。
//     怎么验：抓一条请求侧 `requestId` 未脱敏的样本，比对请求与响应两个值是否相同。
//     本层的 [CloudPhoneRequestID] 按**成文**那一形状实现，且只是"造一个客户端 id"，
//     不参与任何判据 —— 判错的表现是上游回非 `"200"`，不是静默。
//   - `x-DeviceInfo` 的形状：成文说"iPad UA + `<ts><random>` + `375X667` 拼出来的指纹"。
//     十条样本该头**全部**被遮 ⇒ 本层**不提供 builder**，做成调用方给的字段
//     （[CloudPhoneAuth]）。理由见那个字段的注释：本仓库已有另一族的 [AppDevice.DeviceInfo]
//     （14 段、含 `375 * 667`），长得像不等于同一个，搬过来出错**不会有任何断言变红**。
//   - `x-origin`：实拍十条**都带**这个头，值全被遮 ⇒ 本层**不发**。
//     服务端要不要它：未验证（要核：发一条不带它的请求，看是不是照样回 `"200"`）。
//   - `Cookie`：实拍十条都带一整套 WebView cookie。本仓库纯协议实现一律不发 cookie
//     （同 [TyrzLoginRequest] 那条口径："不需要 cookie jar"）；这一族是否**只**认头里的
//     `token` ⇒ 未测试。
//   - 非 `"200"` 的响应：**十条样本全是 `"200"`** ⇒ `header.status` 有哪几种失败取值、
//     `data.errorMsg` / `header.errMsg` / `header.respMsg` 三处文案各自长什么样，
//     **一条都没有**。所以 [EnsureCloudPhoneOK] 只把非 200 报成**一句**错误，
//     不分类、不自造码表（分类要等样本，见 §2.4 第 370 行那句优先级只用于取文案）。
package upstream

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 入口与常量（逐条标注"实拍"还是"只来自成文"）
// ─────────────────────────────────────────────────────────────────────────────

const (
	// CloudPhoneBaseURL 是本族的统一入口前缀。
	//
	// 实拍：十条样本的 URL 逐字以它开头（如
	// `https://cpactiv.buy.139.com/cloudphone-market/redpacket/userAccountInfo`）。
	// ⚠ 它**不在** [envelope.go] 那批基址常量里（那一份归别人维护、本次不许改），
	// 所以在本族自己的文件里落一个名字；接线时别在别处再拼一份字面量。
	CloudPhoneBaseURL = "https://cpactiv.buy.139.com/cloudphone-market"
	// CloudPhoneOrigin 是实拍的 `Origin` 头值（未脱敏，逐字抄自十条样本）。
	CloudPhoneOrigin = "https://cpactiv.buy.139.com"
	// CloudPhoneUserAgent 是实拍的 `User-Agent`（iPad 内嵌 WebView，未脱敏）。
	//
	// ⚠ 它与 [LiveUserAgent]（直播 H5 那族）**逐字相同**，却仍然单列一个名字：
	// 那一份归直播族，哪天那边按自己的窗口换 UA 会把本族一起带走 ——
	// 而同值互搬的表现是"没有任何本地断言会变红"（[LiveMSyncUserAgent] 那条注释
	// 记的就是同一件事）。这一条由 `app_cloud_phone_test.go` 钉在实拍字面量上。
	CloudPhoneUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15 " +
		"(KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"
	// CloudPhoneAccept 是实拍的 `Accept` 头值（十条一致，未脱敏）。
	//
	// ⚠ 它**不等于** [WithH5Headers] 那一族用的 `*/*`：这一族的入口是另一台主机，
	// 照它自己的实拍写。
	CloudPhoneAccept = "application/json, text/plain, */*"

	// CloudPhoneAppID 是成文 §2.4 第 371 行那个 `APP_ID`，也是签名里的第二段。
	//
	// 实拍支持的是**响应侧**：十条样本 `header.appId` 全部是 `"12345681"`（未脱敏）。
	// ⚠ **请求**侧那个 `appId` 头的值被脱敏了 ⇒ "请求里带的确实是同一个 12345681"
	// 这句是响应侧旁证出来的，不是直接看到的。
	CloudPhoneAppID = "12345681"
	// CloudPhoneVersion 是成文 §2.4 第 357–358 行的 `VERSION`。
	//
	// 实拍：四条 `redpacket/*` 的 body 里逐字出现（第 32/35/36/37 条）。
	// ⚠ 它是**请求体里的一个键**，不是头；也**不是**全族都带（见文件头那段复核结果）。
	CloudPhoneVersion = "SYS_CONFIG_Y"
	// CloudPhoneSignSalt 是签名算式里的固定盐值段。
	//
	// ⚠⚠ **来源未核实**：唯一出处是成文 §2.4 第 371 行那句"`SIGN_SALT` 是应用级常量
	// （`md5("123456")`）"。夹具里算式的**四个输入与输出全是 `<REDACTED>`**，
	// ⇒ 本仓库没有任何一条证据能确认这串十六进制就是它，也没有证据说不是。
	// 测试里那条断言钉的是"它确实等于 `md5("123456")`"这层**成文关系**（有人改这个常量时
	// 要能看见"它和成文那句不再一致"），**不是**"上游认这一串"。
	// 它是应用级白盒常量、不是账号凭据（成文 §5 表里"应用级白盒常量，非账号凭据"那一行），
	// 所以可以成文；但**不许**据此打真上游后再把任何 `sign` 值写回仓库。
	CloudPhoneSignSalt = "e10adc3949ba59abbe56e057f20f883e"
	// CloudPhoneChannelSrc 是渠道串。
	//
	// 成文 §2.4 第 371 行把它叫 `CHANNEL_SRC`，说它进 `x-channelSrc` 头。
	// 实拍：这个字面值出现在第 9 条 `linkwhitelist/urlIsExist` 的**请求体**里那个活动页 URL
	// 上（`…&channelSrc=red-mcloudapp-001`，未脱敏）⇒ "**这个字符串在本会话里活动页用过**"
	// 是有实拍的；"`x-channelSrc` 头带的就是它"这半句**未核实**（该头十条全被遮）。
	// ⚠ 同一份夹具里还有 `red-cloudphone-003` / `red-cloudphone-013` 两个渠道值，
	// 它们出现在**上游下发的 jumpLink** 上，不是客户端出去的那一枚 ⇒ 别混。
	CloudPhoneChannelSrc = "red-mcloudapp-001"

	// CloudPhoneSpecTokenSourceID 是登录第 0 步"换 SSO token"要带的那一枚活动站 id。
	//
	// 出处两句：成文 §2.4 第 372 行（`querySpecToken?targetSourceId=001216`）与
	// §5 那张 sourceId 表里"`001216`（红包派对）"那一格。
	// 实拍旁证：第 16/23 条上游下发的跳转链接里带着 `targetSourceId=001216`（未脱敏）
	// ⇒ "这个活动站的 id 是 001216"有样本；而"**换 SSO token 那一枪的请求体**里带的是它"
	// 这半句仍未实拍（`querySpecTokenV2` 一条都没进本夹具）。
	//
	// ⚠ **同一个值，旧 TS 走的是另一个端点**（本轮复核到，别当它已经定论）：
	// `cloud-phone-redpack/task.ts:114` 调的是 `apis/auth.ts:64-73` 的 `querySpecTokenNew` ⇒
	// **GET** `m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/querySpecToken
	// ?targetSourceId=001216&hideLoading=true`（带 H5 签名四件套），
	// 与本仓已实跑通的 [QuerySpecTokenRequest]（**POST** `user-njs.yun.139.com/user/querySpecTokenV2`，
	// body `{toSourceId}`）是**两个不同端点**。⇒ 与拍拍那一页同一形状（见
	// `app_play_ai_session.go` 的 [PlayAIQuerySpecTokenRequest]，那里连 URL 都已按旧 TS 落地）。
	//
	// 换哪一条的**处置权在编排层**（`internal/app/cloud_phone_session.go` 头注写了取哪条与理由），
	// 本层只贡献这一枚入参 + 另一条的构造器，不替它选。
	CloudPhoneSpecTokenSourceID = "001216"
)

// CloudPhoneSuccessStatus 是**唯一**实拍过的 `header.status` 取值。
//
// ⚠ 它是**字符串**而不是数字：十条样本里逐字 `"status":"200"`。成文 §1.2 表最后那一行
// 点名的就是这件事（"红包派对域**完全没有 `code` 字段**，`String(header.status)==="200"`
// 才算成功"）。所以：
//   - 不能拿 [Envelope.EnsureOK] 判这一族 —— 它找 `code`，而这一族**没有** `code`，
//     "没有 code"在它那里是**通过**（见那段 `!hasCode || code == 0`），
//     于是上游回 `"status":"500"` 也会被当成成功。**这就是 [EnsureCloudPhoneOK] 存在的全部理由。**
//   - 数字 200 与字符串 "200" 之间不做兼容：只认字符串形态，变了要在这里立刻响。
const CloudPhoneSuccessStatus = "200"

// ─────────────────────────────────────────────────────────────────────────────
// 鉴权头（每次重算）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneAuth 是一次请求要带的鉴权头素材。
//
// # 为什么所有值都由调用方给、这里一个都不生成
//
// 除 `RequestID` 外，本层**没有任何一条**这些值的实拍（头值全被脱敏），
// 而"每次重算"这件事（成文 §2.4 第 371 行）本身就不是纯函数的活：
// 它要时钟、要活动站 token、要设备指纹。⇒ 这里只放**素材 + 算式**，
// 采集与复用策略（一张票用多久、什么时候换新的）归 `internal/app/` 那一层，
// 与 `AGENTS.md` §1.12 的"账号 × 页面"作用域口径一起判。
type CloudPhoneAuth struct {
	// RequestID 是 `requestId` 头，同时是算式的第一段。
	// 形状见 [CloudPhoneRequestID]（⚠ 该形状未实拍）。
	RequestID string
	// Timestamp 是 `timestamp` 头，同时是算式的最后一段。
	//
	// ⚠ **单位未核实**（秒？毫秒？）⇒ 定成 string 而不是 int64：
	// 收成整数再格式化等于替调用方赌一个单位，而赌错的表现为上游回非 200、
	// 文案里什么都看不出来。原样传字符串，本层**不做**任何转换。
	Timestamp string
	// Token 是活动站 token（[CloudPhoneTokenValidate] 的 `data.token`）。
	//
	// ⚠ 它是**凭据**：只能进请求头与 md5 的输入，
	// 不许进错误文案、不许进日志、不许进测试断言（夹具里那个位置本来就是 `<REDACTED>`）。
	Token string
	// DeviceInfo 是 `x-DeviceInfo` 指纹串。空 ⇒ **不发**这个头。
	//
	// 上一版这里写的是"**没有 builder**（夹具十条全被遮）"。那句话把"看不到实拍的值"
	// 与"没有任何一处实现拼过它"混在了一起 —— 旧 TS `task.ts:57-71` 就是逐字拼这一串的代码。
	// ⇒ builder 补在 `app_cloud_phone_headers.go` 的 [CloudPhoneDeviceInfo]，
	//   取值时机与"两处读数要不要同一个时钟"由**调用方**（`internal/app/` 那一层）定。
	// ⚠ 仍然是**零实拍**：这一串现在钉的是"与旧 TS 字面量一致"。
	DeviceInfo string
	// XOrigin 是 `x-origin`。空 ⇒ **不发**（与 [CloudPhoneAuth.DeviceInfo] 同一取向）。
	//
	// 实拍十条**带**这一枚头（头名可信），值全被遮；旧 TS `task.ts:67-68` 给的是
	// 一枚不带 token 的活动页地址 ⇒ 候选值成文在 [CloudPhoneXOriginHomeURL]。
	// 为什么不在这里写死成默认值：真机 WebView 换页时这一枚会变（未核实），
	// 把"首页"当默认等于替调用方赌一个没证据的不变量。
	XOrigin string
	// ChannelSrc 是 `x-channelSrc`。空 ⇒ 落回 [CloudPhoneChannelSrc]。
	ChannelSrc string
}

// CloudPhoneSign 算 `md5(requestId + APP_ID + token + SIGN_SALT + timestamp)`。
//
// 出处只有成文 §2.4 第 371 行那一句（算式与**段序**都在那一句里）。
// ⚠ 段序**不许"顺手整理"**：调换任意两段的输出都是另一个 md5，而失败表现只有一句
// 非 200 —— 没有任何输入侧信息能告诉你顺序错了。`app_cloud_phone_test.go` 里那条
// 用**测试自己拼的串**去比对，就是为了段序被改时立刻红。
func CloudPhoneSign(requestID, token, timestamp string) string {
	return core.MD5Hex([]byte(requestID + CloudPhoneAppID + token + CloudPhoneSignSalt + timestamp))
}

// Sign 返回本组素材对应的 `sign` 值。
func (a CloudPhoneAuth) Sign() string {
	return CloudPhoneSign(a.RequestID, a.Token, a.Timestamp)
}

// CloudPhoneTimeLocation 是本族格式化 `requestId` 前缀用的时区：东八区（固定 +8，无夏令时）。
//
// ⚠ 这是**推断**，没有实拍：成文只写了 `YYYYMMDDHHmmss` 这一段是"本地时间"。
// 取 +8 的理由与 [FunaiActivityLocation] 同源（面向中国大陆用户的活动页）。
// 影响面也在那里说过一次：这类时区选择只影响拼出来的 id 长什么样，
// 不参与任何判据 —— 上游若按自己的时钟校验前缀，我们**没有证据**说它校验。
func CloudPhoneTimeLocation() *time.Location { return time.FixedZone("UTC+8", 8*3600) }

// CloudPhoneRequestIDStamp 是 `requestId` 第一段用的布局。
//
// ⚠ `20060102150405` 是 Go 的参考式写法：这里要的是**逐位无分隔**的
// `YYYYMMDDHHmmss`，所以布局串里**不能**有 `-` 与 `:`（写成 RFC3339 那种带分隔的，
// 拼出来的 id 会多出两个字符，而 md5 整个变掉）。
const CloudPhoneRequestIDStamp = "20060102150405"

// CloudPhoneRequestIDSuffixLen 是第三段（base36 随机尾巴）的长度，成文里那个 `random36(8)`。
const CloudPhoneRequestIDSuffixLen = 8

// CloudPhoneRequestIDSuffix 从注入的随机源取 8 个 base36 字符。
//
// 字符集取 `0-9a-z`（base36 的标准写法）。⚠ 真实客户端那一头是哪个库、
// 是不是 `Math.random().toString(36).slice(2, 10)` 那种"先丢前缀再截 8 位"的写法，
// **零样本** ⇒ 这一条只保证"长度 8、字符集是 base36"，不保证与 TS 逐字符同分布。
// 它**不是**判据也不是凭据：形状对不上的后果最多是上游按"陌生 id"处理，
// 而那件事今天无人知道长什么样（未测试）。
func CloudPhoneRequestIDSuffix(rng core.Random) string {
	const alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
	buf := make([]byte, CloudPhoneRequestIDSuffixLen)
	rng.FillBytes(buf)
	for i, b := range buf {
		buf[i] = alphabet[int(b)%len(alphabet)]
	}
	return string(buf)
}

// CloudPhoneRequestID 按成文那一形状拼 `requestId`：
// `YYYYMMDDHHmmss`（东八区）+ `Date.now()` 那一枚**毫秒**整数 + [CloudPhoneRequestIDSuffix]。
//
// `unixMilli` 由调用方给而不是从 `at` 乘出来：成文里这两段一个是"格式化的本地时间"、
// 一个是"另一个 API 取的毫秒"，本仓库没有证据说它们必须来自同一个时钟读数
// （真机上二者相差几毫秒是必然的）。要"严格一致"还是"允许差"，交给调用方判。
func CloudPhoneRequestID(at time.Time, unixMilli int64, suffix string) string {
	return fmt.Sprintf("%s%d%s", at.In(CloudPhoneTimeLocation()).Format(CloudPhoneRequestIDStamp),
		unixMilli, suffix)
}

// WithCloudPhoneAuth 把成文那一族的鉴权头贴到请求上。
//
// # 头名与实拍逐字对齐（含大小写）
//
// 十条样本的请求头名一致，本层发其中本层**知道值从哪来**的那些：
// `appId`、`token`、`timestamp`、`requestId`、`sign`、`x-channelSrc`，
// 以及 `DeviceInfo` 非空时的 `x-DeviceInfo`。
//
// 实拍有、这里**按调用方给不给**的两枚：`x-DeviceInfo` 与 `x-origin`
// （十条样本都带这两个头名、值全被遮 ⇒ 不发是"不编值"，不是"上游不带"。
// 值有旧 TS 出处，见 [CloudPhoneDeviceInfo] 与 [CloudPhoneXOriginHomeURL]）。
//
// 实拍有、这里**一律不发**的，以及理由（都不是"忘了"）：
//   - `Cookie`：整套 WebView cookie ⇒ 纯协议实现不带；
//   - `Host` / `Content-Length` / `Connection` / `Sec-Fetch-*` / `Accept-Encoding` /
//     `Accept-Language`：传输层与 WebView 生成的那批，本仓库其它族同样不发。
//     上游据此判什么 ⇒ 未测试。
func WithCloudPhoneAuth(request core.HTTPRequest, auth CloudPhoneAuth) core.HTTPRequest {
	channelSrc := auth.ChannelSrc
	if channelSrc == "" {
		channelSrc = CloudPhoneChannelSrc
	}
	if auth.DeviceInfo != "" {
		request = request.WithHeader("x-DeviceInfo", auth.DeviceInfo)
	}
	if auth.XOrigin != "" {
		request = request.WithHeader("x-origin", auth.XOrigin)
	}
	return request.
		WithHeader("User-Agent", CloudPhoneUserAgent).
		WithHeader("Origin", CloudPhoneOrigin).
		WithHeader("Accept", CloudPhoneAccept).
		WithHeader("appId", CloudPhoneAppID).
		WithHeader("x-channelSrc", channelSrc).
		WithHeader("requestId", auth.RequestID).
		WithHeader("timestamp", auth.Timestamp).
		WithHeader("token", auth.Token).
		WithHeader("sign", auth.Sign())
}

// CloudPhoneRequest 构造 `POST <CloudPhoneBaseURL>/<path>`，body =
// `{version:"SYS_CONFIG_Y"} ∪ data`（成文 §2.4 第 370 行那一形状）。
//
// ⚠ 这一条**只适用于**成文那一类（实拍里就是 `redpacket/*` 那四条）。
// `user/tokenValidate`、`activity/common/ruleInfo`、`rule/config/list`、`bilog/upload`
// 各自的 body 都不带（或不只带）这个 `version` ⇒ 它们走 [CloudPhoneRawRequest]，
// 别"顺手统一"（统一的表现是给三条不该带的口凭空加一个键）。
//
// # 为什么 data 里的键名一律由调用方给
//
// 本层的构造函数**不猜**没在成文或实拍里出现过的键名。有实拍的口把 body 写死；
// 只有动作名、没有键名的口把 `data` 开放成入参，并在注释里写"键名未核实"。
// 猜一个键名的代价是**静默**：上游按"这个参数没传"处理，而我们看到的是"请求成功了"。
func CloudPhoneRequest(path string, data map[string]any, auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	body := make(map[string]any, len(data)+1)
	body["version"] = CloudPhoneVersion
	for key, value := range data {
		body[key] = value
	}
	return CloudPhoneRawRequest(path, body, auth, referer)
}

// CloudPhoneRawRequest 构造一条**自己决定 body** 的本族请求：
// `POST <base>/<path>` + 实拍那一组固定头 + 鉴权头。
//
// `path` 传的是 `CloudPhoneBaseURL` **之后**那一段，逐字照抄实拍（含驼峰与大小写：
// 这一族的尾段全是 `tokenValidate` / `userAccountInfo` / `configTaskLoginList` 这种驼峰，
// 而**主机段与目录段**是全小写 `cloudphone-market`。改对任何一处的表现是 404）。
//
// `referer`：实拍这一族的 `Referer` **十条全被遮** ⇒ 该带哪个页面地址**未核实**。
// 从第 9 条那个未脱敏的活动页 URL 可以推出**首页**是
// `https://cpactiv.buy.139.com/#/redEnvelopeParty/home?…`（另见 `app_cloud_phone_task.go` 里的
// [CloudPhoneActivityHomeURL]），但"每条请求的 referer 都等于首页"这句没有证据
// （WebView 里换页它就变）。所以它是参数，不写死。
func CloudPhoneRawRequest(path string, body any, auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/%s", CloudPhoneBaseURL, path)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	// WithJSONBody 补的是 `application/json`，实拍是**带 charset 的那个** ⇒ 换掉。
	request.Headers = dropHeader(request.Headers, "content-type")
	request = request.WithHeader("Content-Type", H5ContentType)
	if referer != "" {
		request = request.WithHeader("Referer", referer)
	}
	return WithCloudPhoneAuth(request, auth), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 信封判据
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneHeader 是信封的 `header` 段。
//
// 实拍十个里各段都是**字符串**（`status`、`errMsg`、`appId`、`requestId` 都是带引号的）。
// ⚠ `status` 尤其别当数字（[CloudPhoneSuccessStatus] 那段说了为什么）。
type CloudPhoneHeader struct {
	// Status 是 `header.status`。nil = 上游**没给**这一位（≠ 给了空串）。
	Status *string
	// ErrMsg 是 `header.errMsg` —— 实拍十条全是 `"OK"`。
	ErrMsg *string
	// RespMsg 是 `header.respMsg`。⚠ **零样本**：夹具里连一次都没出现过这个键，
	// 出处只有成文 §2.4 第 370 行那句错误文案优先级。留着的唯一理由是"哪天给了别丢"。
	RespMsg *string
	// AppID 是 `header.appId`（实拍 `"12345681"`）。
	AppID *string
	// RequestID 是 `header.requestId`。⚠ 它与请求侧那个头的关系**未核实**（见文件头）。
	RequestID *string
}

// cloudPhoneString 取一个字符串键：缺失或非字符串 ⇒ nil（**不**做数字转字符串的兼容）。
//
// 为什么不做：这一族的 `status` 恰恰因为"字符串/数字不分"最容易出事 —— 哪天它变成
// 数字 200，[EnsureCloudPhoneOK] 要**立刻报错**而不是照样判成功。
func cloudPhoneString(obj map[string]any, key string) *string {
	if obj == nil {
		return nil
	}
	text, ok := obj[key].(string)
	if !ok {
		return nil
	}
	return &text
}

// ParseCloudPhoneHeader 解出信封的 `header` 段。
//
// 没有 `header` ⇒ 各字段全是 nil（**不报错**）：报错的活是 [EnsureCloudPhoneOK] 的，
// 这里只交读数。分成两个函数的理由是 §1.7：调用方有时只想看"上游到底说了什么"。
func ParseCloudPhoneHeader(envelope *Envelope) CloudPhoneHeader {
	obj, _ := envelope.Raw().(map[string]any)
	head, _ := obj["header"].(map[string]any)
	return CloudPhoneHeader{
		Status:    cloudPhoneString(head, "status"),
		ErrMsg:    cloudPhoneString(head, "errMsg"),
		RespMsg:   cloudPhoneString(head, "respMsg"),
		AppID:     cloudPhoneString(head, "appId"),
		RequestID: cloudPhoneString(head, "requestId"),
	}
}

// cloudPhoneSection 取信封的一个对象段；整包不是对象、该段缺失或非对象 ⇒ nil。
func cloudPhoneSection(envelope *Envelope, key string) map[string]any {
	obj, _ := envelope.Raw().(map[string]any)
	section, _ := obj[key].(map[string]any)
	return section
}

// EnsureCloudPhoneOK 判本族的信封，成功时给出 `data`（对象）。
//
// # 三条判据，各自挡住一种静默失败
//
//  1. 整包不是对象 / 没有 `header` 段 / `header.status` 读不出字符串
//     ⇒ [core.NewParse]："信封不认识"必须**显式**报错（成文 §1.2 最后那段要求的
//     正是这一类；旧代码把它落到"其他 code 只打日志"是本项目点名要修的）。
//     ⚠ 尤其"缺 status"这一档：拿 [Envelope.EnsureOK] 判会得到**通过**。
//  2. `status != "200"` ⇒ [core.NewBadRequest]，文案按成文 §2.4 第 370 行的优先级
//     `data.errorMsg → header.errMsg → header.respMsg` 取**第一处非空**。
//     ⚠ 非 200 的响应**零样本** ⇒ 不分类、不猜哪个码该重试。
//     （本族的"该不该重来"要靠 `data` 里的业务状态位，见 [CloudPhoneUserStatus] 与
//     各口自己的注释；信封这一层只回答"这一枪打没打上"。）
//  3. `status == "200"` 但 `data` 缺失或不是对象 ⇒ **不报错**，交 `nil`。
//     实拍里就有这种口（第 31/34 条 `bilog/upload` 的 `data` 是 `{"ret":1}`，
//     而将来某天它可能什么都不给），"打上了但没载荷"与"没打上"是两件事。
//     要读载荷的口在解析函数里各自判自己的字段。
//
// # 错误文案里为什么允许带上游原文
//
// 这三处 msg 都是**人读的说明**（实拍只有 `"OK"` 这一种），不含凭据；
// 而 `core.Error` 的对外文案另有白名单机制（见 [errorFromStatus] 那段）。
// ⚠ 但**不许**把整份响应体或任何头值拼进来 —— 那才是会带出 token 的路。
func EnsureCloudPhoneOK(envelope *Envelope) (map[string]any, *core.Error) {
	head := cloudPhoneSection(envelope, "header")
	if head == nil {
		return nil, core.NewParse("红包派对信封不认识：没有 header 段")
	}
	status := cloudPhoneString(head, "status")
	if status == nil {
		return nil, core.NewParse("红包派对信封不认识：header.status 不是字符串或缺失")
	}
	data := cloudPhoneSection(envelope, "data")
	if *status == CloudPhoneSuccessStatus {
		return data, nil
	}
	text, ok := cloudPhoneFailureText(head, data)
	if !ok {
		return nil, core.NewBadRequest(fmt.Sprintf("红包派对上游回 status=%q，且三处文案位都是空的", *status))
	}
	return nil, core.NewBadRequest(fmt.Sprintf("红包派对上游回 status=%q: %s", *status, text))
}

// cloudPhoneFailureText 按成文 §2.4 第 370 行的优先级取一句错误文案：
// `data.errorMsg → header.errMsg → header.respMsg`。
//
// ⚠ "空串算不算给了"这一条**未核实**（零样本）。这里按**空串 = 没给**处理，
// 继续往下一档找：把 `"errorMsg":""` 当成"上游说了一句空话"来报，
// 得到的是一条没有任何信息量的错误，而下游的 `errMsg` 明明还在。
func cloudPhoneFailureText(head, data map[string]any) (string, bool) {
	for _, candidate := range []*string{
		cloudPhoneString(data, "errorMsg"),
		cloudPhoneString(head, "errMsg"),
		cloudPhoneString(head, "respMsg"),
	} {
		if candidate != nil && *candidate != "" {
			return *candidate, true
		}
	}
	return "", false
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务状态位（成文 §2.4 第 379 行那张 `statusText` 表）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneUserStatus 是任务条目的状态位（`userStatus`）。
//
// ⚠ 为什么做成**有名字的类型**而不是四处散着的 `== 1`：这一族五档里有两档
// 的处置方向**相反**（`1` 是"已完成 ⇒ 跳过"、`2` 是"未定义但旧实现会进执行分支"），
// 写成裸数字早晚有人把 `2` 折进"其它 ⇒ 跳过"或"未完成 ⇒ 去做"，
// 而两种折法都会让**真金白银**的那一步发出去或漏掉。
//
// 证据分两档，别当成一张同级字典：
//   - `0` / `1` **有实拍**：第 36 条的四张任务表里 `userStatus` 只出现这两个值
//     （`NOVICE_1` = 0 未做过、`MONTHLY_1` = 1 已完成、`userCompleteNum` 分别是 0 / 1）；
//   - `2` / `3` / `4` **零样本**，出处只有成文那一句。`2` 那一档成文明确标着
//     "未定义但会进入执行分支" ⇒ 本仓库把它**原样命名成未定义**，不给它编语义。
type CloudPhoneUserStatus int64

const (
	// CloudPhoneUserStatusUnfinished 是 `0`：未完成（实拍）。
	CloudPhoneUserStatusUnfinished CloudPhoneUserStatus = 0
	// CloudPhoneUserStatusCompleted 是 `1`：已完成 ⇒ 旧实现跳过（实拍）。
	CloudPhoneUserStatusCompleted CloudPhoneUserStatus = 1
	// CloudPhoneUserStatusUndefined2 是 `2`：**成文说这一档没定义**，
	// 而旧实现的分支写法会把它放进"去做"那一支。
	//
	// ⚠ 不许把它折进 Completed 也不许折进 Unfinished：前者等于凭空少做任务，
	// 后者等于对一件状态未知的任务发写请求。本层只把读数交出去，
	// "这一档要不要发"是编排层要单独回答的问题（并且今天回答不了，见 `2` 零样本）。
	CloudPhoneUserStatusUndefined2 CloudPhoneUserStatus = 2
	// CloudPhoneUserStatusPrizeExhausted 是 `3`：奖品已兑完 ⇒ 旧实现跳过。**零样本**。
	CloudPhoneUserStatusPrizeExhausted CloudPhoneUserStatus = 3
	// CloudPhoneUserStatusTomorrow 是 `4`：明天再来 ⇒ 旧实现跳过。**零样本**。
	CloudPhoneUserStatusTomorrow CloudPhoneUserStatus = 4
)

// String 返回档位名，用于日志与结论（不是上游字面量，**不进任何对拍面**）。
//
// ⚠ 未知取值（比如哪天上游给 7）**单独成一档文本**而不是折叠成 `0`：
// `fmt` 走的是这个方法，折错就会把"没见过"写成"未完成 ⇒ 去做"。
func (s CloudPhoneUserStatus) String() string {
	switch s {
	case CloudPhoneUserStatusUnfinished:
		return "未完成"
	case CloudPhoneUserStatusCompleted:
		return "已完成"
	case CloudPhoneUserStatusUndefined2:
		return "未定义(2)"
	case CloudPhoneUserStatusPrizeExhausted:
		return "奖品已兑完"
	case CloudPhoneUserStatusTomorrow:
		return "明天再来"
	}
	return fmt.Sprintf("未知(%d)", int64(s))
}

// Known 报告这一档是不是成文那张表里有的五档之一。
//
// 用途只有一个：让编排层把"没见过的状态位"落成**未知**那一档而不是随便选一支。
func (s CloudPhoneUserStatus) Known() bool {
	switch s {
	case CloudPhoneUserStatusUnfinished, CloudPhoneUserStatusCompleted,
		CloudPhoneUserStatusUndefined2, CloudPhoneUserStatusPrizeExhausted,
		CloudPhoneUserStatusTomorrow:
		return true
	}
	return false
}

// cloudPhoneStatusAt 取一个状态位（数字与字符串两种形态都收，见 [asInt64]）。
//
// ⚠ 收字符串这一条**不是**兼容癖：这一族里金额位实拍就是字符串
// （`"canAmount":"210"`、`"signAmount":"20"`），状态位实拍是数字。
// 零样本的那几口到底是哪种没人知道，所以两种都认 —— 认不出来的落到 nil。
func cloudPhoneStatusAt(obj map[string]any, key string) *CloudPhoneUserStatus {
	value := optInt64Field(obj, key)
	if value == nil {
		return nil
	}
	status := CloudPhoneUserStatus(*value)
	return &status
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录：`user/tokenValidate`（第 0 步 `querySpecTokenV2` 复用包里现成的实现）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneLoginConfig 是 tokenValidate body 里那个 `loginConfig`，实拍逐字
// `"redpacketLoginConfig"`（第 29 条）。
const CloudPhoneLoginConfig = "redpacketLoginConfig"

// CloudPhonePintype 是 tokenValidate body 里那个 `pintype`，实拍数字 `13`。
//
// ⚠ **不能**与同一份夹具里第 1 条那个 `pintype: 5` 混：那一条是
// `cloud.139.com/uniphone/user/tokenValidate`，**另一个主机、另一个 appId、另一个入口**
// （文件头把它两条的字段差异列全了）。13 与 5 各归各的域。
const CloudPhonePintype int64 = 13

// CloudPhoneTokenValidateVersion 是 tokenValidate body 里那个 `version`，实拍 `"1.0"`。
//
// ⚠ 它**不是** [CloudPhoneVersion]（`SYS_CONFIG_Y`）。同一个键名在同一次抓包的
// 两个口上取着不同的值，这是"别做统一 body 拼法"那条理由最硬的一处。
const CloudPhoneTokenValidateVersion = "1.0"

// CloudPhoneTokenValidateRequest 构造 `POST …/cloudphone-market/user/tokenValidate`。
//
// body 逐字四键 + 一个 token（第 29 条，实拍顺序即字典序）：
//
//	{"deviceId":"…","loginConfig":"redpacketLoginConfig","pintype":13,"token":"…","version":"1.0"}
//
// # `ssoToken` 是哪来的
//
// 成文 §2.4 第 372 行：`querySpecToken?targetSourceId=001216`（签名 saltKey=2）换回的
// 那一串。本层**不重写**那一步，见 [CloudPhoneSpecTokenSourceID] 的注释。
//
// # ⚠ 登录那一枪的**鉴权头**里 `token` 带什么：未核实
//
// 第 29 条的 `token` 头与 body 里的 `token` **两个位置都被遮了**，而按流程这时
// 活动站 token 还没换到 ⇒ 头里那一位更像是 SSO token 本身。这里把 `auth` 整个交回
// 调用方填（[CloudPhoneAuth]），不替它选。真跑时若这一枪回非 200，**第一个**要试的
// 就是把 `auth.Token` 也换成 `ssoToken`。
func CloudPhoneTokenValidateRequest(
	auth CloudPhoneAuth, referer, deviceID, ssoToken string,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"deviceId":    deviceID,
		"loginConfig": CloudPhoneLoginConfig,
		"pintype":     CloudPhonePintype,
		"token":       ssoToken,
		"version":     CloudPhoneTokenValidateVersion,
	}
	return CloudPhoneRawRequest("user/tokenValidate", body, auth, referer)
}

// CloudPhoneTokenValidate 是 `user/tokenValidate` 的解析结果。
//
// 实拍 `data` 五个键：`account`（上游给的**掩码手机号**，形态"前三位 + 星号 + 后四位"
// —— 带尾号，按仓库口径成文不复述原值，只看这里就够了）、`expireTime`（数字 `3600`）、
// `phone`（被遮）、`riskLevel`（`"PASS"`）、`token`（被遮）。
type CloudPhoneTokenValidate struct {
	// Token 是 `data.token` —— 后面所有口要带的活动站 token。
	//
	// **必填**：换不到它就是登录失败，绝不能判成成功。
	// ⚠ 它是**凭据**：只进内存与请求头，不许进错误文案 / 日志 / 测试断言。
	Token string
	// Account 是 `data.account` —— **掩码**手机号（实拍形态是"前三位 + `****` + 后四位"，
	// 按本仓口径此处只记形态、不复述原值）。
	// 它可以直接进结论，因为上游给的就是掩码。⚠ 别把它和 `phone` 混用：后者实拍被遮，
	// 形态与是否含明文号码**未测试**。
	Account *string
	// RiskLevel 是 `data.riskLevel`，实拍 `"PASS"`。⚠ 其它取值**零样本** ⇒ 不解释。
	RiskLevel *string
	// ExpireTime 是 `data.expireTime`，实拍数字 `3600`。
	//
	// ⚠⚠ **不解释它**，尤其不许拿它当 TTL 用（`AGENTS.md` §1.12：失效判据只有上游那一句，
	// 不自造 TTL）。为什么：同一个键名在同一份夹具的另一条上（第 1 条，另一域）
	// 取值是 `1785407853245` —— 那是一枚**毫秒时间戳**，而这里的 3600 看着像**秒数**。
	// 同名两义，而哪一义都没有成文说过 ⇒ 只交读数。
	ExpireTime *int64
}

// ParseCloudPhoneTokenValidate 解析登录响应。
//
// 缺 `data.token` ⇒ [core.NewParse]：这一条的失败必须是**错误**。
// 它是整族的入口，"登录成功但没有 token"往下走的每一枪都带一个空头，
// 而那些口的失败长得很像"活动结束了"。
// 反过来其它三个字段缺了**不报错**：本层要交出的判据只有"有没有换到票"这一格。
func ParseCloudPhoneTokenValidate(envelope *Envelope) (CloudPhoneTokenValidate, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneTokenValidate{}, perr
	}
	token := cloudPhoneString(data, "token")
	if token == nil || *token == "" {
		return CloudPhoneTokenValidate{}, core.NewParse(
			"tokenValidate 的 data 里没有字符串 token（不判成登录成功）")
	}
	return CloudPhoneTokenValidate{
		Token:      *token,
		Account:    cloudPhoneString(data, "account"),
		RiskLevel:  cloudPhoneString(data, "riskLevel"),
		ExpireTime: optInt64Field(data, "expireTime"),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 余额与播报
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhonePlatformType 是 `userAccountInfo` body 里那个 `platformType`，实拍数字 `1`。
//
// ⚠ 它是这条口的**独有**键（另外三条 `redpacket/*` 都没有它）。
// 语义未记录：本层只照抄，不解释成"iOS/Android"之类 —— 那纯是望文生义。
const CloudPhonePlatformType int64 = 1

// CloudPhoneAccountInfoRequest 构造 `POST …/redpacket/userAccountInfo`。
//
// 实拍 body（第 35 条，逐字）：`{"platformType":1,"version":"SYS_CONFIG_Y"}`。
func CloudPhoneAccountInfoRequest(auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest("redpacket/userAccountInfo",
		map[string]any{"platformType": CloudPhonePlatformType}, auth, referer)
}

// CloudPhoneAccountInfo 是余额（**产出是钱**那一格，成文 §2.4 第 369 行）。
//
// 实拍 `data` 只有一个嵌套 `info`（六个键全是字符串）：
//
//	account "<掩码手机号>"  canAmount "210"  totalAmount "370"
//	id "7086772"           createTime "2024-10-17 18:56:33"  updateTime "2026-06-17 20:02:58"
type CloudPhoneAccountInfo struct {
	// CanAmount 是 `data.info.canAmount` —— **可兑余额，单位分**（成文 §2.4 第 369 行：
	// 代码 `/100` 显示元）。实拍字符串 `"210"`。
	//
	// ⚠ 用 `*int64` 而不是"缺了当 0"：`0` 是"这账号确实没有可兑余额"这一条**结论**，
	// nil 是"上游没给这一位、我们不知道"。本族产出的是钱，把"不知道"写成"没有"
	// 会让用户以为余额被清零了。`/100` 那一步归展示层，本层不做除法。
	CanAmount *int64
	// TotalAmount 是 `data.info.totalAmount` —— **累计**，单位分（实拍 `"370"`）。
	// 与 CanAmount 的关系（累计是否含已兑走的部分）成文与样本都没说 ⇒ **未验证**。
	TotalAmount *int64
	// Account 是 `data.info.account`（掩码手机号，可直接进结论）。
	Account *string
	// AccountID 是 `data.info.id`（实拍字符串 `"7086772"`）。
	// ⚠ 它是**红包账户的 id**，不是 userDomainId、不是手机号。
	AccountID *string
	// CreateTime / UpdateTime 是 `createTime` / `updateTime`，实拍形态
	// `"2024-10-17 18:56:33"`（**无时区**裸串）。
	//
	// ⚠ 本层**不解析成 time.Time**：没有任何判据要按它做时间窗，
	// 而按哪个时区解释它、`updateTime` 到底随什么变，都没有证据。
	CreateTime *string
	UpdateTime *string
}

// ParseCloudPhoneAccountInfo 解析余额。
//
// `info` 缺失 ⇒ 全字段 nil、**不报错**：这条是**只读**口，读不出来交 nil，
// 由调用方落成"这一格不知道"。⚠ 别在这里报错：成对拍"余额前后各一次"那一类链
// （成文 §2.4 第 373 行）来说，"前一次没读到"和"接口挂了"要能分开。
func ParseCloudPhoneAccountInfo(envelope *Envelope) (CloudPhoneAccountInfo, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneAccountInfo{}, perr
	}
	info, _ := data["info"].(map[string]any)
	return CloudPhoneAccountInfo{
		CanAmount:   optInt64Field(info, "canAmount"),
		TotalAmount: optInt64Field(info, "totalAmount"),
		Account:     cloudPhoneString(info, "account"),
		AccountID:   cloudPhoneString(info, "id"),
		CreateTime:  cloudPhoneString(info, "createTime"),
		UpdateTime:  cloudPhoneString(info, "updateTime"),
	}, nil
}

// CloudPhoneToastInfoRequest 构造 `POST …/redpacket/userToastInfo`。
//
// 实拍 body（第 37 条，逐字）：`{"version":"SYS_CONFIG_Y"}`。
//
// 成文 §2.4 第 373 行把它记成"签到 + `userToastInfo` **播报金额**"那一环。
// ⚠ 但实拍响应里**只有一个** `isShowCloud`，没有任何金额 ⇒
// "它播报的是金额"这句与这条口的实际载荷**对不上**（未验证，见 [CloudPhoneToastInfo]）。
func CloudPhoneToastInfoRequest(auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest("redpacket/userToastInfo", nil, auth, referer)
}

// CloudPhoneToastInfo 是 `userToastInfo` 的解析结果。
//
// 实拍 `data` **只有一个键**：`{"isShowCloud":1}`（数字）。
type CloudPhoneToastInfo struct {
	// IsShowCloud 是 `data.isShowCloud`，实拍 `1`。
	//
	// ⚠ 语义**未验证**，本仓库不解释：非 0 是什么、0 又是什么，一条样本都没有
	// （同一次会话里第 32/36 两条也没有别的样本能给 `1` 配一个对照组）。
	// 名字里的 "Toast" 与成文那句"播报金额"都对不上 ⇒ 拿它做判据之前先补样本。
	IsShowCloud *int64
}

// ParseCloudPhoneToastInfo 解析播报口。缺 `isShowCloud` ⇒ nil（≠ 0）。
func ParseCloudPhoneToastInfo(envelope *Envelope) (CloudPhoneToastInfo, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneToastInfo{}, perr
	}
	return CloudPhoneToastInfo{IsShowCloud: optInt64Field(data, "isShowCloud")}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层（本文件自带的三条；其余在本族另两份文件里各自带）
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudPhoneTokenValidate 换活动站 token（`user/tokenValidate`）。
//
// ⚠ 这是一条**建立会话**的口，不带活动站 token 就能打（`auth` 里那一位填什么未核实，
// 见 [CloudPhoneTokenValidateRequest]）。返回的 token 是凭据，交调用方保管。
func (u *Upstream) AppCloudPhoneTokenValidate(
	ctx context.Context, auth CloudPhoneAuth, referer, deviceID, ssoToken string,
) (CloudPhoneTokenValidate, *core.Error) {
	request, perr := CloudPhoneTokenValidateRequest(auth, referer, deviceID, ssoToken)
	if perr != nil {
		return CloudPhoneTokenValidate{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneTokenValidate{}, perr
	}
	return ParseCloudPhoneTokenValidate(envelope)
}

// AppCloudPhoneAccountInfo 查红包余额（`userAccountInfo`）。**只读**。
//
// ⚠ 单位是**分**，见 [CloudPhoneAccountInfo]。
func (u *Upstream) AppCloudPhoneAccountInfo(
	ctx context.Context, auth CloudPhoneAuth, referer string,
) (CloudPhoneAccountInfo, *core.Error) {
	request, perr := CloudPhoneAccountInfoRequest(auth, referer)
	if perr != nil {
		return CloudPhoneAccountInfo{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneAccountInfo{}, perr
	}
	return ParseCloudPhoneAccountInfo(envelope)
}

// AppCloudPhoneToastInfo 查播报口（`userToastInfo`）。**只读**，且载荷语义未验证。
func (u *Upstream) AppCloudPhoneToastInfo(
	ctx context.Context, auth CloudPhoneAuth, referer string,
) (CloudPhoneToastInfo, *core.Error) {
	request, perr := CloudPhoneToastInfoRequest(auth, referer)
	if perr != nil {
		return CloudPhoneToastInfo{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneToastInfo{}, perr
	}
	return ParseCloudPhoneToastInfo(envelope)
}
