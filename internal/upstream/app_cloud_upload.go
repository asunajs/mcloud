// 网盘"传一个文件上去"这一族的四枪（#35，106 / 113 / 522 / 1014 四条玩法的共同前置）：
// `personal-kd-njs.yun.139.com/hcy/file/create` → `PUT <uploadUrl>` →
// `hcy/file/complete` → `hcy/recyclebin/batchTrash`。
//
// 旧实现是 `packages/core/src/apis/file.ts`（四口）+ `utils/file.ts`（编排），
// 调用点在 `runners.ts:43/95/115/318` 与 `after.ts:135`（清理）。
//
// # ⚠ 证据分两半，而且这一族比 rabactive 那一族更薄
//
//   - **四枪里只有 `file/create` 有实拍，而且实拍的是"建目录"那一型**
//     （`test/fixtures/http/3d.json` 两条：`type:"folder"`，响应 `data.uploadId` 与
//     `data.partInfos` 都是 `null`、`exist:true`）。⇒ 它只钉得住三件事：
//     ①URL 与**信封形状**（`{code:"0000"(字符串), message, success, data}`，载荷在 `data`）；
//     ②`data.fileId` 这一格确实存在；③这台主机收这个客户端发的请求。
//   - **"上传文件"那一型、`PUT`、`complete`、`batchTrash` 零实拍**：
//     `test/fixtures/http/*.json` 里 grep 不到任何一条。⇒ 这四口的请求面
//     **只对齐到"与旧 TS 字面量一致"**，一个真打都没有过。
//
// 按 v4 口径（接口层按旧实现落码、业务层照它推进）：本文件把四口都落下来，
// 但测试**只钉"发得出 + 形状对"，不钉语义** —— 哪些格子是推断的逐条写在各自注释里。
// 真要判"上游认不认这一套"，唯一的办法是拿一个测试号真跑一次四枪（P1 第三档，需用户点头）。
//
// # 预签名 `uploadUrl` 与凭据同类：**绝不进日志、夹具、错误文本**
//
// `create` 回的那个 URL 是**带签名的写入入口**（本仓从未抓到过长什么样）。
// 所以：① 它只作为 [CloudFileCreated.UploadURL] 交回调用方，本包任何函数都
// 不把它拼进 `core.Error` 的 Message；② 夹具与测试里只出现 `SYNTH-*` 合成值。
//
// # 与同一台主机上那一口 `hcy/file/list` 用了**两套画像**（差异登记为 R27）
//
// [CloudFileListRequest] 按实拍选了 iOS 的 [AppProfile]（`WithAppHeaders` 那一整族
// `x-yun-*`）。这一族零实拍 ⇒ 按**旧 TS** 发：主 http 实例的静态头（[AiAvatarHeaders]）
// 加 `getFileOpHeader` 那三档。同一账号同一趟里两套并存是**已知不一致**，
// 不是笔误；谁对未判定（`file/list` 的实拍只证明"那一份是另一个客户端发的"，
// 见 R16 同一处置）。
package upstream

import (
	"context"
	"fmt"
	"net/url"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 三档"渠道"与两种操作类型（旧 TS `apis/file.ts:3-40` + `constants.ts:66-70`）
// ─────────────────────────────────────────────────────────────────────────────

// CloudChannel 是文件操作头里的"哪个端在传"。
//
// ⚠ 它是**字符串枚举**而不是 int：这一族的分派条件就是比字面量
// （旧 TS `if (channelSrc === CHANNEL_SRC.pc)`），做成 int 会多一层映射表，
// 而那一层映射恰恰是"传错端"能静默通过的地方。
//
// ⚠ 与 [IOSChannel]（`10000020`）**不是一个值族**：那是 iOS App 的渠道号，
// 这里是旧 TS 文件操作三档里的 android 档 `10000023`。别互相搬。
type CloudChannel string

const (
	// CloudChannelAndroid 是旧 TS 的**默认档**（`getFileOpHeader` 的 `channelSrc`
	// 缺省值，`apis/file.ts:14`）—— 也就是说 `uploadRandomFile` 那条链默认按安卓端传。
	CloudChannelAndroid CloudChannel = "10000023"
	// CloudChannelWeb 是网页端档。旧实现的**删除**那一口固定用它
	// （`apis/file.ts:114` 的 `channelSrc: "10000034"`），与本文件的
	// [CloudBatchTrashRequest] 一致。
	CloudChannelWeb CloudChannel = "10000034"
	// CloudChannelPC 是 PC 客户端档。`runners.ts:95`（玩法 113）在上传前先
	// `refreshAuthToken` 再按 PC 档传 —— 那是那个玩法自己的要求，不是这一族的默认。
	CloudChannelPC = CloudChannel(huaweiChannelSrc)
)

// CloudOpType 是操作类型。旧 TS 只有两档（`"backup" | "upload"`）。
//
// ⚠ 它**只对 android 档有效果**：另两档不看这个参数（`apis/file.ts:17-40` 的
// 两个分支里 `opType` 只被 android 分支读）。所以"backup 在 web 档会怎样"这一格
// 的答案是"和 upload 一样"——那是从代码形状读出来的，不是实测。
type CloudOpType string

const (
	// CloudOpUpload 是普通上传：`x-yun-op-type: 1`、`x-yun-sub-op-type: 100`。
	CloudOpUpload CloudOpType = "upload"
	// CloudOpBackup 是备份上传：`op-type: 2`、`sub-op-type: 200`。
	// 旧 TS 的 `task-expansion.ts:12` 用这一档（备份类玩法那条链）。
	CloudOpBackup CloudOpType = "backup"
)

// 请求体里的固定字面量（逐字来自旧 TS `apis/file.ts`）。
const (
	// CloudFileTypeFile 是 `create` 的 `type`：这一族只传文件。
	// ⚠ 实拍那两条是 `"folder"`（建目录），"这一口认不认 `file`"由那两条**证不了**。
	CloudFileTypeFile = "file"
	// CloudRenameModeAuto 是**上传**那一型的重名策略（`apis/file.ts:65`）。
	// 实拍两条是 `"refuse"`（建目录用 refuse）⇒ 两档各有出处，不是猜的。
	CloudRenameModeAuto = "auto_rename"
	// CloudHashAlgorithm 是 `contentHashAlgorithm`：`SHA256`（大写，两处都用它）。
	CloudHashAlgorithm = "SHA256"
	// CloudCreateContentType 是**请求体里那个 `contentType` 键**的值：
	// `application/oct-stream`（少一个 `e`）。
	//
	// ⚠ 这是旧 TS 的字面量（`apis/file.ts:68`），看着像拼错，但**上游收到过这一串**。
	// 与 [CloudPutContentType]（PUT 那一条，拼写正常）不是一处：一个是 MIME 键的值、
	// 一个是真实 HTTP 头。改任何一个都等于改一次接口 ⇒ 照抄，见坑表里
	// `AiAvatarImageExt` 同一处置。
	CloudCreateContentType = "application/oct-stream"
	// CloudPutContentType 是 PUT 那一枪**真实的** `content-type` 头（`apis/file.ts:101`）。
	CloudPutContentType = "application/octet-stream"
)

// 三档 client-info 串（旧 TS `constants.ts:21-27`）。
//
// ⚠ 它们是**素材字面量**：整串照抄，别按"我们这一台设备"重拼。三档的版本号各不相同
// （App `12.6.0` / Web `7.15.3` / PC `8.3.3.20250521`），与仓库里那几个
// `12.6.0` 常量（[IOSAppVersion]、[TaskClientVersion]）不同族 —— 那几枚是请求体/头里的
// 版本号，这里是设备信息串的第 4 段，两者只是**恰好同号**。
const (
	cloudClientInfoAndroid = "1|127.0.0.1|1|12.6.0|Xiaomi|22041216C||02-00-00-00-00-00|android 14|1080X2360|zh||||032|0|"
	cloudClientInfoWeb     = "||9|7.15.3|firefox|139.0|||linux unknow||zh-CN|||"
	cloudClientInfoPC      = "||11|8.3.3.20250521|PC|TGktMjAyNDA5MTcxMDA2||| Windows 11 (11.0)|1920X1020|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||"
)

// Yun139HostSuffixes 是旧 TS 判定"这台主机要带 authorization"的四段后缀
// （`shared/src/http/base.ts:10` 的 `AUTH_DOMAINS`）。
//
// ⚠ 刻意不导出、且只在 [CloudUploadPutRequest] 用：其余三枪的**主机是本包常量**
// （`personal-kd-njs.yun.139.com` 命中 `yun.139.com`），所以那一格是"写在注释里的定值"，
// 不需要运行时判断。只有 PUT 那一枪的主机**来自响应**（预签名 URL，本仓零样本），
// 判断才有意义 —— 旧实现在那一条上同样按这个规则决定带不带。
var yun139AuthHostSuffixes = []string{"yun.139.com", "mcloud.139.com", "feixin.10086.cn", "mail.10086.cn"}

// sendsAuthorizationTo 复刻 base.ts 那条"按主机后缀决定是否注入 Basic"的规则。
//
// 解析不出主机（相对路径、坏 URL）⇒ **不发**：这是"宁可缺一枚头"的一侧，
// 因为把凭据发到一个认不出的主机上不可回滚，而缺一枚头的表现是这一枪失败。
// ⚠ 本仓没有"预签名 URL 长什么样"的样本 ⇒ 这条规则对 PUT 那一枪的实际效果**未验证**。
func sendsAuthorizationTo(rawURL string) bool {
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return false
	}
	host := strings.ToLower(parsed.Hostname())
	if host == "" {
		return false
	}
	for _, suffix := range yun139AuthHostSuffixes {
		if strings.HasSuffix(host, suffix) {
			return true
		}
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求头：三档 `getFileOpHeader`（旧 TS `apis/file.ts:9-42`）
// ─────────────────────────────────────────────────────────────────────────────

// CloudFileOpHeaders 给文件操作那一族补头。
//
// 它做两件事，顺序**有意如此**：
//  1. 先套 [AiAvatarHeaders]（主 http 实例那套静态头：`User-Agent` + `deviceid` +
//     `Authorization` + **恒发** `Content-Type`）。名字带着另一族的族名是因为它是
//     这一套头的**唯一实现** —— 复用而不复制，否则两处会分叉。
//     `jwt` 传空：`personal-kd-njs.yun.139.com` **不在**旧 TS 的 `TARGET_HOSTNAMES`
//     名单里（`base.ts:12`），那一台从来收不到 `jwttoken`，这不是漏写。
//  2. 再叠上 `getFileOpHeader` 的三档。⚠ 这些头**不带** `content-type` ⇒ 请求体的
//     MIME 由第 1 步那枚 `application/json;charset=UTF-8` 决定（与旧实现一致：
//     它的环境头是 `application/json;charset=UTF-8`，见 `base.ts:29`）。
//
// `authorization` / `deviceID` 传空串 = 不发那一枚（沿用 [AiAvatarHeaders] 的口径）。
func CloudFileOpHeaders(
	request core.HTTPRequest, authorization, deviceID string,
	channel CloudChannel, op CloudOpType,
) core.HTTPRequest {
	request = AiAvatarHeaders(request, authorization, "", deviceID)
	// base 三枚：三档都有（`apis/file.ts:15`）。
	request = request.
		WithHeader("x-yun-api-version", "v1").
		WithHeader("x-yun-app-channel", string(channel)).
		WithHeader("x-yun-op-type", "1")
	switch channel {
	case CloudChannelPC:
		// PC 档：另加四枚 + PC 设备串，且 `User-Agent` 被改成裸的 `Mozilla/5.0`。
		// ⚠ 必须**覆盖式**地设：[core.HTTPRequest.WithHeader] 是**追加**，
		// 而旧 TS 是对象赋值（⇒ 只有一枚）。同名两枚在 Go 这边就是两根 header 线，
		// "上游按哪一根读"未测试，而"只留旧实现那一个值"是可证的忠实。
		request = setCloudHeader(request, "User-Agent", "Mozilla/5.0")
		return request.
			WithHeader("x-yun-market-source", "001").
			WithHeader("x-yun-module-type", "100").
			WithHeader("x-yun-svc-type", "1").
			WithHeader("x-yun-client-info", cloudClientInfoPC)
	case CloudChannelAndroid:
		opCode := 1
		if opTypeIsBackup(op) {
			opCode = 2
		}
		// ⚠ 这一枚在**备份档**下要覆盖 base 的 `1` ⇒ 必须走 setCloudHeader。
		// 表现成两号线不是"难看"而已：旧实现发的是 `2`，而现在追加会变成
		// 同时发 `1` 与 `2`（上游读哪一枚未测试），备份玩法的判定就会飘。
		request = request.WithHeader("x-huawei-channelSrc", string(channel)).
			WithHeader("x-yun-client-info", cloudClientInfoAndroid).
			WithHeader("x-yun-sub-op-type", strconv.Itoa(opCode*100))
		return setCloudHeader(request, "x-yun-op-type", strconv.Itoa(opCode))
	default:
		// web 档（以及任何未识别的渠道值 ⇒ 与旧 TS 的 fall-through 同一侧）。
		// ⚠ `x-yun-device-id` 在旧 TS 里被赋成**设备信息串**（`apis/file.ts:41`），
		// 看着像笔误，但上游收到过的就是这一串 ⇒ 照抄。
		request = request.WithHeader("x-huawei-channelSrc", string(channel))
		return request.WithHeader("x-yun-client-info", cloudClientInfoWeb).
			WithHeader("x-yun-device-id", cloudClientInfoWeb)
	}
}

// setCloudHeader 覆盖式地设一枚头（先按大小写不敏感摘掉同名，再追加）。
//
// 存在的理由：**旧 TS 的 `{...base, k: v}` 是覆盖**，而
// [core.HTTPRequest.WithHeader] 是**追加** —— 不写成覆盖，
// "改一个头的值"会静默变成"加一根线"，而上游读哪一根未测试。
//
// 它必须**留在链的最后一步**：先 set 再 WithHeader 同名会又把那一枚加回来。
func setCloudHeader(request core.HTTPRequest, name, value string) core.HTTPRequest {
	request.Headers = dropHeader(request.Headers, name)
	return request.WithHeader(name, value)
}

// opTypeIsBackup 把 [CloudOpType] 折成旧实现那个 `opType === "backup"` 判断。
//
// 单独一个函数是为了让"其它值 ⇒ upload"这一格可断言：`CloudOpType` 是字符串类型，
// 调用方可以传任意串（含空串），而空串在旧 TS 里就是 upload。
func opTypeIsBackup(op CloudOpType) bool { return op == CloudOpBackup }

// ─────────────────────────────────────────────────────────────────────────────
// 第一枪：POST /hcy/file/create
// ─────────────────────────────────────────────────────────────────────────────

// CloudFileCreateInput 是"要传什么"的调用方输入。
//
// 刻意做成一个参数结构体而不是六个位置参数：这一枪有**六个可选格**，
// 六个 string/int64 连排会让"传错顺序"变成一个能编译通过的 bug。
type CloudFileCreateInput struct {
	// ParentFileId 是目标目录 id；空 ⇒ 按旧实现补 `"/"`（根目录）。
	// 旧 TS `apis/file.ts:57`：`parentFileId: parentFileId || "/"`。
	ParentFileID string
	// Name 是文件名。旧 TS 的默认值是 `asign-<8 hex><ext||.png>`，那是**编排层**的事
	// （见 `utils/file.ts:30`），不在这一层。
	Name string
	// Size 是字节数。⚠ 上游拿它和 PUT 的实际长度对不对得上**未测试**；
	// 旧实现按"字符串就取 length、字节就取 byteLength"算，两者对 ASCII 内容同值。
	Size int64
	// ContentHash 是内容的 SHA256 十六进制（小写）。旧实现用 `sha256(file)`，
	// 且**同一个串**在 complete 那一枪再发一次 ⇒ 两枪必须一致，见 [CloudFileCompleteRequest]。
	ContentHash string
	// Channel 是渠道档；零值按 [CloudChannelAndroid]（旧 TS 的默认参数）。
	Channel CloudChannel
	// Op 是操作类型；零值按 [CloudOpUpload]。
	Op CloudOpType
}

// parentFileIDOrDefault 补旧实现那个 `|| "/"`。
func (in CloudFileCreateInput) parentFileIDOrDefault() string {
	if in.ParentFileID == "" {
		return "/"
	}
	return in.ParentFileID
}

// channelOrDefault 补旧实现的两处默认（参数默认值 + `channelSrc || CHANNEL_SRC.web`）。
func (in CloudFileCreateInput) channelOrDefault() CloudChannel {
	if in.Channel == "" {
		return CloudChannelAndroid
	}
	return in.Channel
}

// CloudFileCreateRequest 构造 `POST /hcy/file/create`（**写**：建一个待上传的文件位）。
//
// body 十键逐字来自旧 TS `apis/file.ts:56-70`。三件要说清的：
//
//   - **单分片**：`partInfos` 只有一项、`parallelUpload:false`。旧实现也是这样，
//     所以"大于某个尺寸要不要分片"这一格**两版都没答案**（未测试）。
//     本仓的调用方只传几十字节的合成内容，落在这一档里。
//   - **键序**与旧 TS 不同：Go 的 `map[string]any` 按字典序 marshal。
//     实拍那条 `file/create` 的请求体**恰好也是字典序**
//     （`{"fileRenameMode":"refuse","name":…,"parentFileId":…,"type":"folder"}`），
//     所以"上游是否依赖声明序"从样本里**读不出来** ⇒ 不加"必须按声明序"的断言，
//     也不认为无所谓。
//   - `exist` / `rapidUpload` 两格的语义见 [CloudFileCreated]。
func CloudFileCreateRequest(
	authorization, deviceID string, in CloudFileCreateInput,
) (core.HTTPRequest, *core.Error) {
	if in.Name == "" {
		return core.HTTPRequest{}, core.NewInternal("file/create 缺 name：旧实现总有一个 asign-* 名字，空名这一格未测试")
	}
	if in.ContentHash == "" {
		return core.HTTPRequest{}, core.NewInternal("file/create 缺 contentHash：旧实现总是先 sha256 再发")
	}
	body := map[string]any{
		"parentFileId":         in.parentFileIDOrDefault(),
		"name":                 in.Name,
		"type":                 CloudFileTypeFile,
		"size":                 in.Size,
		"fileRenameMode":       CloudRenameModeAuto,
		"contentHash":          in.ContentHash,
		"contentHashAlgorithm": CloudHashAlgorithm,
		"contentType":          CloudCreateContentType,
		"parallelUpload":       false,
		"partInfos": []any{map[string]any{
			"parallelHashCtx": map[string]any{"partOffset": 0},
			"partNumber":      1,
			"partSize":        in.Size,
		}},
	}
	request, err := core.NewRequestPost(BasePersonalKD + "/hcy/file/create").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return CloudFileOpHeaders(request, authorization, deviceID, in.channelOrDefault(), in.Op), nil
}

// CloudFileCreated 是 `create` 回的那一格 `data`。
//
// # 四格各自"缺"意味着什么（旧实现的分叉就在这里）
//
//   - [Exist] ⇒ 旧实现**直接返回 false 且不上传**（`utils/file.ts:52-55`）。
//     实拍两条 `folder` 型都是 `exist:true` —— 那是"目录已经建过了"，与"文件已存在"
//     是不是同一格语义**未测试**。
//   - [RapidUpload] ⇒ 旧实现**跳过 PUT 与 complete**、只把 fileId 记账（`utils/file.ts:57-61`）。
//     也就是"秒传"这一支**不发后三枪**，所以清理那一族拿到的 id 仍然要删得掉。
//   - [UploadURL] 与 [FileID] 任一为空 ⇒ 旧实现判失败（`utils/file.ts:64-67`）。
//   - [UploadID] 旧实现**不判空**就带进 complete ⇒ 秒传之外 `uploadId` 为 null 时
//     会发生什么，零样本。
type CloudFileCreated struct {
	FileID      *string
	UploadID    *string
	Exist       bool
	RapidUpload bool
	UploadURL   *string
}

// ParseCloudFileCreate 解析 `file/create` 的响应。
//
// 信封与 rabactive 那六口**不同**：载荷在 `data`（不是 `result`），`code` 是字符串
// `"0000"`。这两件事实拍已证（`3d.json` 两条 folder 型）。
//
// ⚠ `data` 缺失 ⇒ 解析失败，**不**兜成"全空的 CloudFileCreated"：
// 旧实现是 `createResult.data` 直接解构（缺了会抛），把它翻译成"没有 fileId"
// 会让上层念成"这一枪发成功了但没给 URL"，那是两种不同的病。
func ParseCloudFileCreate(envelope *Envelope) (CloudFileCreated, *core.Error) {
	payload, perr := envelope.EnsureOK()
	if perr != nil {
		return CloudFileCreated{}, perr
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return CloudFileCreated{}, core.NewParse(fmt.Sprintf(
			"file/create 的 data 不是对象，实得 %T（这一口与 rabactive 六口不同信封：载荷在 data）", payload))
	}
	out := CloudFileCreated{
		FileID:      optStrField(obj, "fileId"),
		UploadID:    optStrField(obj, "uploadId"),
		Exist:       cloudFlag(obj, "exist"),
		RapidUpload: cloudFlag(obj, "rapidUpload"),
	}
	// `partInfos` 是数组，取**第一项**的 `uploadUrl`（旧实现 `partInfos?.[0]?.uploadUrl`）。
	// 实拍那条是 `null`（folder 型没有分片）⇒ "缺席 / null" 是合法形状，交回 nil。
	if parts, isList := obj["partInfos"].([]any); isList && len(parts) > 0 {
		if first, isObj := parts[0].(map[string]any); isObj {
			out.UploadURL = optStrField(first, "uploadUrl")
		}
	}
	return out, nil
}

// cloudFlag 读一个布尔格：非布尔（含 `null`、数字、缺席）一律 false。
//
// ⚠ 刻意**不**"把 `1` 当 true"：旧 TS 用的是 `if (exist)`，那确实会把 `1` 当真值，
// 但这一族的响应里这两格在实拍中都是**布尔** ⇒ 出现数字就意味着形状变了，
// 而"形状变了却念成 false"与"念成 true"一样危险，两档都不如让它可断言。
func cloudFlag(obj map[string]any, key string) bool {
	flag, _ := obj[key].(bool)
	return flag
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二枪：PUT <uploadUrl>（预签名直传）
// ─────────────────────────────────────────────────────────────────────────────

// CloudUploadPutRequest 构造那一发 PUT。
//
// # 这一枪为什么不走 [Upstream.Call]
//
// 预签名 URL 的响应**不是** hcy 那套 JSON 信封（本仓零样本，连状态码都没抓到过）。
// 交给 [ParseEnvelope] 只会得到"响应不是 JSON"，而那句话会掩盖真正的失败。
// ⇒ 发送层走 [Upstream.Raw]：非 2xx 是错误，2xx 只回"这一枪成了"。
//
// # 三个头 + 按主机决定的 authorization
//
// `content-type` / `origin` / `referer` 是旧 TS `apis/file.ts:98-104` 的逐字三枚；
// `Authorization` 与 `deviceid` 来自主实例的注入规则 ⇒ authorization 是否发**由 URL 现判**
// （见 [sendsAuthorizationTo]）：主机不命中就**不带**，哪怕调用方给了凭据 ——
// 把 Basic 凭据发往一台规则里没写的主机是不可回滚的一侧。
//
// ⚠ **不带幂等键** ⇒ [core.HTTPRequest.IsRetryable] 判它不可重试。这是刻意的：
// 重放同一条 PUT 的语义未验证，而代价是用户网盘里多一段坏文件。
func CloudUploadPutRequest(
	uploadURL string, content []byte, authorization, deviceID string,
) core.HTTPRequest {
	request := core.NewRequestPut(uploadURL)
	request.Body = core.Body(content)
	request = request.WithHeader("content-type", CloudPutContentType).
		WithHeader("origin", "https://yun.139.com").
		WithHeader("referer", "https://yun.139.com/").
		WithHeader("User-Agent", CaiyunAndroidWebUA)
	if deviceID != "" {
		request = request.WithHeader("deviceid", deviceID)
	}
	if authorization != "" && sendsAuthorizationTo(uploadURL) {
		request = request.WithHeader("Authorization", authorization)
	}
	return request
}

// ─────────────────────────────────────────────────────────────────────────────
// 第三枪：POST /hcy/file/complete
// ─────────────────────────────────────────────────────────────────────────────

// CloudFileCompleteRequest 构造 `POST /hcy/file/complete`（**写**：告诉上游这段传完了）。
//
// body 四键逐字（`apis/file.ts:90`）。`contentHash` 必须与 `create` 那一枪同一个串
// —— 旧实现传的就是同一个变量（`utils/file.ts:80-86`），服务端比对的正是"PUT 上去的
// 字节"与"这两处声明的哈希"，**比对不上会怎样未测试**。
//
// ⚠ 这一枪的 `channelSrc` 在旧实现里**默认换成 android 档**（`utils/file.ts:84` 的
// `options.channelSrc || "10000023"`）—— 也就是"create 用 web 档、complete 用 android 档"
// 是旧实现的真实行为（当调用方没显式给 channelSrc 时）。本层不做那个默认，
// 由调用方把同一个 channel 传两遍；要复刻旧行为请在编排层显式传。
func CloudFileCompleteRequest(
	authorization, deviceID, fileID, uploadID, contentHash string,
	channel CloudChannel, op CloudOpType,
) (core.HTTPRequest, *core.Error) {
	if fileID == "" || contentHash == "" {
		return core.HTTPRequest{}, core.NewInternal("file/complete 缺 fileId 或 contentHash：这两个是这一枪的全部依据")
	}
	body := map[string]any{
		"fileId":               fileID,
		"uploadId":             uploadID,
		"contentHash":          contentHash,
		"contentHashAlgorithm": CloudHashAlgorithm,
	}
	request, err := core.NewRequestPost(BasePersonalKD + "/hcy/file/complete").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return CloudFileOpHeaders(request, authorization, deviceID, channel, op), nil
}

// ParseCloudFileComplete 只判"这一枪成不成"。
//
// 旧实现拿到 complete 的响应后**什么都不读**（`utils/file.ts:88-91` 只看有没有抛错），
// 所以这里也不声明任何字段 —— 等真有一次实拍能读出 `data` 的形状再加。
func ParseCloudFileComplete(envelope *Envelope) *core.Error {
	_, perr := envelope.EnsureOK()
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 第四枪：POST /hcy/recyclebin/batchTrash（移进回收站，不是彻底删除）
// ─────────────────────────────────────────────────────────────────────────────

// CloudBatchTrashRequest 构造批量删除（**写**，且不可回滚：文件会进回收站）。
//
// # 空列表 ⇒ 不发
//
// 旧实现在**调用侧**挡（`after.ts:133` 的 `if (files && files.length > 0)`）。
// 这里把它挪进构造函数：请求层能挡住的"少发一枪"不该指望每个调用方都记得挡。
//
// # 渠道档固定 web
//
// `apis/file.ts:114` 写死 `channelSrc: "10000034"`，**与前面三枪用的档位无关** ——
// 也就是说旧实现里"用 PC 档上传、用 web 档删除"是常态。照抄，不"顺手统一"。
func CloudBatchTrashRequest(
	authorization, deviceID string, fileIds []string,
) (core.HTTPRequest, *core.Error) {
	if len(fileIds) == 0 {
		return core.HTTPRequest{}, core.NewInternal("batchTrash 收到空文件列表：旧实现在这里根本不发请求")
	}
	body := map[string]any{"fileIds": fileIds}
	request, err := core.NewRequestPost(BasePersonalKD + "/hcy/recyclebin/batchTrash").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return CloudFileOpHeaders(request, authorization, deviceID, CloudChannelWeb, CloudOpUpload), nil
}

// ParseCloudFileBatchTrash 以**顶层 `success`** 为准。
//
// 旧实现判的是 `result?.success`（`utils/file.ts:120`），那个 `result` 是
// `tryRequest` 元组里的**整个响应对象**，不是信封里的 `result` 字段 ——
// 也就是说它读的是顶层 `success`，而且**读不到就当失败**（`undefined` 是假值）。
// 这里同一口径：`EnsureOK` 先拦掉 `success:false` 与非 0 码，再要求
// 顶层确实有一个 `success:true`；那一格缺席 ⇒ 回错误，不念成"没失败就是成功了"。
func ParseCloudFileBatchTrash(envelope *Envelope) *core.Error {
	if _, perr := envelope.EnsureOK(); perr != nil {
		return perr
	}
	if flag, ok := envelope.Success(); !ok || !flag {
		return core.NewParse("batchTrash 的响应里没有 success:true（旧实现把这一格当删除失败）")
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudFileCreate 开一个待上传的文件位（**写**：会在网盘里占一个坑，直到 complete
// 或被 batchTrash）。返回的形状与四格语义见 [CloudFileCreated]。
func (u *Upstream) AppCloudFileCreate(
	ctx context.Context, authorization, deviceID string, in CloudFileCreateInput,
) (CloudFileCreated, *core.Error) {
	request, perr := CloudFileCreateRequest(authorization, deviceID, in)
	if perr != nil {
		return CloudFileCreated{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudFileCreated{}, perr
	}
	return ParseCloudFileCreate(envelope)
}

// AppCloudUploadPut 把字节写进预签名 URL（**写**，且**不重试**）。
//
// `authorization` 给了也未必发：那一枚是否带上由 URL 的主机决定
// （见 [CloudUploadPutRequest]），因为这一枪的主机**来自上一枪的响应**、本仓零样本。
//
// ⚠ 任何错误文本里都不出现 `uploadURL`（见文件头那条凭据边界：它是带签名的写入入口）。
// [Upstream.Raw] 在失败时只回状态码（见 [errorFromStatus] 那条"不带响应体原文"），
// 所以这一条今天成立；改 `Raw` 的错误构造时要回看这里。
func (u *Upstream) AppCloudUploadPut(
	ctx context.Context, uploadURL string, content []byte, authorization, deviceID string,
) *core.Error {
	_, perr := u.Raw(ctx, CloudUploadPutRequest(uploadURL, content, authorization, deviceID))
	return perr
}

// AppCloudFileComplete 通知上游"这段传完了"（**写**）。
func (u *Upstream) AppCloudFileComplete(
	ctx context.Context, authorization, deviceID, fileID, uploadID, contentHash string,
	channel CloudChannel, op CloudOpType,
) *core.Error {
	request, perr := CloudFileCompleteRequest(authorization, deviceID, fileID, uploadID, contentHash, channel, op)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	return ParseCloudFileComplete(envelope)
}

// AppCloudBatchTrash 把一批文件移进回收站（**写**，不可回滚）。
//
// 空列表在这里直接回错误（不发请求），调用方按"没有要清理的东西"处理即可 ——
// 旧实现连日志都是分开的两句（`after.ts:135` / `:137`）。
func (u *Upstream) AppCloudBatchTrash(
	ctx context.Context, authorization, deviceID string, fileIds []string,
) *core.Error {
	request, perr := CloudBatchTrashRequest(authorization, deviceID, fileIds)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	return ParseCloudFileBatchTrash(envelope)
}
