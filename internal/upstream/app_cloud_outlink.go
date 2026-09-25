// 网盘"分享外链"那一族的两口（玩法 434 的前置）：
// `POST yun.139.com/orchestration/personalCloud-rebuild/outlink/v1.0/getOutLink` →
// 同前缀的 `delOutLink`。
//
// 对应旧实现 `packages/core/src/apis/share.ts`（两口 + `GetOutLinkResponse` 类型），
// 调用点是 `runners.ts:68-88`（玩法 434：把这一趟传上去的第一个文件分享出去、再立刻取消分享）。
//
// # ⚠ 两口**零 REST 实拍**：本层只保证"发的是旧实现那一份请求"
//
// 本仓所有 har / 夹具里**没有一条** `orchestration/personalCloud-rebuild/outlink/*`。
// 仓里有几处名字像、但**不能当本族样本**的东西，逐处说清：
//   - `internal/upstream/app_task_click.go` 与 `live_room_flower.go` 里写到的
//     `IOutLink/queryRecentFriendLinkList`（查"最近收到的分享"，不建也不删；
//     它的响应里那枚 `linkInfo.url` 是**明文外链**，与 `aiAppLink` 同档处理）；
//   - `internal/upstream/app_funai.go:157` 说的 `getOutLink` 是签到任务 `taskInfo` 里的
//     一个**字段键名**（与 `getFile` 平级），不是这一口；
//   - `docs/capture/pcclient-api.js:280-348` 与 `apiServer.js:2725-2751` 的
//     `getOutLinkV3`（**PC 客户端的 XML 传输**）。它与本族同服务，能佐证
//     `getOutLinkRes` / `getOutLinkResSet` 这两级键名真实存在，但它请求侧用的是
//     `linkType` 与顶层 `account`（不是 `subLinkType` / `commonAccountInfo`）、
//     `shareChannel` 是 `3003`（本族旧 TS 是 `"3001"`），响应侧判据是 `code === "S_OK"`
//     且载荷包在 `data.result.…getOutLinkResOne` 里（多一层 `result` 与一层 `…One` 包装）。
//     ⇒ 它证明的是"这个服务认这一族概念"，**不是**"REST v1.0 这一口回什么形状"；
//   - `docs/caiyun-go-feature-inventory.md:127` 的 `IOutLink/getOutLinkInfoV2`
//     （开源 Go 版用来"按链接码取分享信息"，是**接收**侧另一口）。
//
// ⇒ 本族两口的**唯一**证据源仍是旧 TS `apis/share.ts`：请求面按它逐字落码。
//
// 所以与上传那一族（`app_cloud_upload.go` 文件头）同一处置：
//   - 请求面**逐字**对齐旧 TS 的字面量，测试只钉"发得出 + 形状对"，**不钉语义**；
//   - 哪些格子是推断的，逐条写在各自注释里；
//   - 要判"上游认不认这一套"只有真跑一次（§1.13：跑一次就有终态、事件流与小结算四格）。
//
// # 与同一台主机上的其它族用了**三套**画像
//
// 这一族走 `yun.139.com` 的 orchestration 网关，头是旧 TS **逐请求**给的那十三枚
// （`caller: web` + `mcloud-*` + 一串 `x-yun-*`）；`personal-kd-njs.yun.139.com` 那一族
// 用 `getFileOpHeader` 的三档；`hcy/file/list` 用 iOS 的 `WithAppHeaders`。
// 三套并存是**已知不一致**（R27 同族），不是笔误；谁对未判定。
//
// `authorization` 在这一族**是带的**：`yun.139.com` 命中旧 TS `base.ts:10` 的
// `AUTH_DOMAINS`，而 `jwttoken` **不带**（那一台不在 `base.ts:12` 的 `TARGET_HOSTNAMES` 名单里）。
//
// # ⚠ 两口的**头集合不对称**，照抄不"顺手统一"
//
// `getOutLink` 在旧 TS 里带 `{headers: {...}}` 第三参数（`apis/share.ts:44-60`），
// `delOutLink` **没有**那一个参数（`apis/share.ts:67-79`）⇒ 它只有主 http 实例的静态头。
// 看着像旧作者漏写，但上游收到过的就是"一个带、一个不带"⇒ 统一两处的头集合
// 等于改一次接口，而那一格零样本。
//
// # ⚠ 与旧实现**刻意不一致**的一处：`code` 的判据
//
// 旧 runner 判的是 `data?.code === "0"`（`runners.ts:74`）—— **字符串字面相等**。
// 本仓走 [Envelope.EnsureOK]，它把 `code` 经 [asInt64] 归一：**`"0"` 与 `"0000"` 与数字 `0` 都算成功**。
//
// 这一处分歧**有后果**，而且两侧不对称：本仓这一族里 `file/create` 的实拍回的是
// `"0000"`（`3d.json`），而同族的 `orchestration` 网关回什么**没有样本**。
// 若上游这一族回 `"0000"`：
//   - 旧实现判成"分享失败" ⇒ 不删 ⇒ **外链留在用户的网盘里**（一个能被陌生人打开的链接）；
//   - 本仓判成成功 ⇒ 接着删 ⇒ 拿不到 `linkID` 时才留下那一个。
//
// 也就是说这一处"放宽"落在**不可回滚的那一侧更安全**的方向上：少删一次外链的后果
// 比多试一次删除重。反过来的分歧（上游回非零码却其实建好了链接）由
// [CloudOutLink] 那一格兜：拿不到 `linkID` 时上层必须把它播成失败，不许静默。
//
// # 2026-09-25 真跑过两趟 `app-task`：两口**仍一条都没发出去**（本文件头那句"零真打"**没有**被这一轮消掉）
//
// 玩法已接上（`internal/app/task_matrix_run.go` 的 `runShare`），434 当天在两个测试号的
// 待做表里、runner 也被调用了，但两趟都停在**发枪之前**那一格：
// "这一趟前面没有任务往网盘传过东西 ⇒ 没有可分享的文件"。根因是**表序**（434 的 id 小于
// 当天能上传的那几条：522 / 614），成文与四个样本见 `runShare` 头注与
// [`docs/pending-decisions.md`](../../docs/pending-decisions.md) 的 **R30** / **Q41**。
//
// ⇒ 本文件里所有关于**响应形状**的说法（`linkID` 在哪一级、`delOutLink` 回什么码、
// 上面那两条关于 `0` / `"0000"` 的分歧会不会真的发生）**至今仍是推断**，
// 一句都没被上游确认过。要收口得先让 434 真的拿到一个文件（Q41 那一问）。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// BaseYun139Orchestration 是网盘 orchestration 网关的根（旧 TS 两口写死的同一前缀）。
//
// ⚠ 它与 [BasePersonalKD] **不是同一台**：那是 `personal-kd-njs.yun.139.com`（文件操作），
// 这一台是 `yun.139.com`（编排层）。两者都以 `yun.139.com` 结尾 ⇒ 都命中带
// `authorization` 那条规则，但**路径与头画像完全不同**，别互相搬。
const BaseYun139Orchestration = "https://yun.139.com/orchestration/personalCloud-rebuild"

// 这一族自己的五枚字面量（逐字来自旧 TS `apis/share.ts:45-58`）。
//
// ⚠ 它们看着像"我们这台设备/这个客户端"的值，实际是**素材字面量**：旧 TS 对任何账号
// 都发同一串。整串照抄，别按仓库里其它族的常量重拼。
const (
	// OutLinkWebCaller 是 `caller` 那一枚（旧实现固定 `web` —— 这一族走的是网页端的编排入口）。
	OutLinkWebCaller = "web"
	// OutLinkMcloudChannel 是 `mcloud-channel`。
	//
	// ⚠ 它与上传那一族的 [CloudChannelWeb]（`10000034`）、iOS 那一带的 [IOSChannel]
	// （`10000020`）**是三族不同的值**，本文件里 `x-yun-app-channel` 才是 `10000034` 那一枚。
	// 把它们"顺手统一"会静默改掉一次接口。
	OutLinkMcloudChannel = "1000101"
	// OutLinkMcloudVersion 是 `mcloud-version`：网页端版本号。
	//
	// ⚠ 它是 `7.14.4`，而上传那一族的设备信息串里是 `7.15.3`
	// （`cloudClientInfoWeb`）—— 两族各有各的版本，不是"漏了一个没改"。
	OutLinkMcloudVersion = "7.14.4"
	// OutLinkDeviceInfo 是 `x-deviceinfo`。
	//
	// ⚠ 它与下面那枚 [OutLinkClientInfo] **只差结尾的管道数**（这一枚以 `zh-CN|||` 结尾，
	// 那一枚以 `zh-CN|||||` 结尾）。这一格被"顺手统一"的表现是上游按另一种设备画像处理，
	// 而两枚都在旧实现里发过、哪一枚才是对的没有样本 —— 所以有专门一条测试钉它。
	OutLinkDeviceInfo = "||9|7.14.4|edge||||linux unknow||zh-CN|||"
	// OutLinkClientInfo 是 `x-yun-client-info`（与上一枚同形、结尾多两根管道）。
	OutLinkClientInfo = "||9|7.14.4|edge||||linux unknow||zh-CN|||||"
)

// OutLinkHeaders 给 `getOutLink` 那一口补头：主 http 实例的静态头 + 旧 TS 逐请求给的那十三枚。
//
// 先套 [AiAvatarHeaders]（`User-Agent` + `deviceid` + `Authorization` + 恒发的
// `Content-Type`；`jwt` 传空 —— 见文件头那条 `TARGET_HOSTNAMES` 判定），再叠这一族自己的。
// 名字带着另一族的族名，因为它是那套静态头的**唯一实现**（复用而不复制，同上传那一族的理由）。
//
// ⚠ [core.HTTPRequest.WithHeader] 是**追加**，而旧 TS 的对象赋值是**覆盖**。这里十三枚
// 与静态头**没有同名**（`content-type` 不在十三枚里），所以不需要覆盖；
// 将来要加一枚与静态头同名的，必须走 [setCloudHeader]，否则会发出去两根线。
func OutLinkHeaders(request core.HTTPRequest, authorization, deviceID string) core.HTTPRequest {
	request = AiAvatarHeaders(request, authorization, "", deviceID)
	return request.
		WithHeader("caller", OutLinkWebCaller).
		WithHeader("cms-device", "default").
		WithHeader("mcloud-channel", OutLinkMcloudChannel).
		WithHeader("mcloud-version", OutLinkMcloudVersion).
		WithHeader("x-deviceinfo", OutLinkDeviceInfo).
		WithHeader("x-huawei-channelsrc", string(CloudChannelWeb)).
		WithHeader("x-svctype", "1").
		WithHeader("x-yun-api-version", "v1").
		WithHeader("x-yun-app-channel", string(CloudChannelWeb)).
		WithHeader("x-yun-channel-source", string(CloudChannelWeb)).
		WithHeader("x-yun-client-info", OutLinkClientInfo).
		WithHeader("x-yun-module-type", "100").
		WithHeader("x-yun-svc-type", "1")
}

// ─────────────────────────────────────────────────────────────────────────────
// 第一口：POST /outlink/v1.0/getOutLink（**写**：造一条可被任何人打开的外链）
// ─────────────────────────────────────────────────────────────────────────────

// CloudOutLinkInput 是"分享哪几个文件、叫什么名"的调用方输入。
//
// 做成参数结构体与 [CloudFileCreateInput] 同一条理由：这一口有**十一个**体里的键，
// 连排的位置参数会让"传错顺序"变成一个能编译通过的 bug。
type CloudOutLinkInput struct {
	// Account 是 `commonAccountInfo.account`。旧实现传的是 `ctx.config.phone`
	// （`runners.ts:69` 的 `phone`）⇒ **手机号明文进请求体**。
	//
	// ⚠ 它是凭据族：不出现在任何错误文本与日志里（本文件所有错误串都只说"缺 account"，
	// 不 echo 值）。它进的是发给上游的请求体，那是旧实现本来就在做的事。
	Account string
	// FileIDs 是 `coIDLst`：要分享出去的网盘文件 id 列表。
	//
	// ⚠ 旧实现只分享**第一个**（`[fileId]`，`runners.ts:73`），但那一口认多个。
	// "多个会回多条 linkID"未验证 ⇒ 上层只传一个，本层不假装知道多文件的形状。
	FileIDs []string
	// DedicatedName 是 `dedicatedName`：外链的自定义名。旧实现**恒传空串**
	// （`runners.ts:73` 的第三个实参就是 `""`）⇒ 这里不设默认，由调用方显式给。
	DedicatedName string
}

// CloudGetOutLinkRequest 构造那一发（**写**：在用户网盘里建一条分享外链）。
//
// body 形状逐字来自旧 TS `apis/share.ts:23-43`。那些**数字**（`subLinkType:0` /
// `encrypt:1` / `pubType:1` / `period:1` / `periodUnit:1` / `accountType:1` /
// `isWatermark:0`）没有任何成文解释：`period:1 + periodUnit:1` 看着像"有效期 1 天"，
// 但单位表没有样本 ⇒ **全部照抄，不解释、不参数化**（要调成"有效期 7 天"得先实拍）。
//
// ⚠ 空 [CloudOutLinkInput.FileIDs] ⇒ 不发这一枪（回错误）。旧实现在调用侧挡
// （`runners.ts:70-73` 那句 `if (!files || files.length === 0)`）；与
// [CloudBatchTrashRequest] 同一处置：请求层能挡住的"少发一枪"不指望每个调用方记得挡。
func CloudGetOutLinkRequest(
	authorization, deviceID string, in CloudOutLinkInput,
) (core.HTTPRequest, *core.Error) {
	if len(in.FileIDs) == 0 {
		return core.HTTPRequest{}, core.NewInternal("getOutLink 收到空文件列表：旧实现在这里根本不发请求")
	}
	if in.Account == "" {
		// ⚠ 错误文本里**不**出现 account 的值：它是手机号（见结构体那一格）。
		return core.HTTPRequest{}, core.NewInternal("getOutLink 缺 account：它是 commonAccountInfo 的唯一键，旧实现总是有")
	}
	body := map[string]any{
		"getOutLinkReq": map[string]any{
			"subLinkType":   0,
			"encrypt":       1,
			"coIDLst":       in.FileIDs,
			"caIDLst":       []any{},
			"pubType":       1,
			"dedicatedName": in.DedicatedName,
			"period":        1,
			"periodUnit":    1,
			"viewerLst":     []any{},
			"extInfo": map[string]any{
				"isWatermark":  0,
				"shareChannel": "3001",
			},
			"commonAccountInfo": map[string]any{
				"account":     in.Account,
				"accountType": 1,
			},
		},
	}
	request, err := core.NewRequestPost(BaseYun139Orchestration + "/outlink/v1.0/getOutLink").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return OutLinkHeaders(request, authorization, deviceID), nil
}

// CloudOutLink 是 `getOutLink` 交回来的那一格外链标识。
//
// # ⚠ `LinkID` 为空**不是**"成功且没事可做"
//
// 旧实现在这一格是 `if (linkId) await delOutLink(...)`（`runners.ts:78-80`）⇒
// 拿不到 id 时它**静默地什么都不做**，于是那一条外链**留在用户的网盘里**（可被任何人打开）。
// 本层照旧实现的形状交回 nil，但**上层必须把它播成失败**（见 `internal/app` 那一族的
// `runShare`）—— 静默是"用户以为分享流程走完了、其实留了一条公开链接"的唯一来源。
type CloudOutLink struct {
	// LinkID 是 `data.getOutLinkRes.getOutLinkResSet[0].linkID`。
	LinkID *string
}

// ParseCloudOutLink 解析这一口的响应：只看 `linkID` 那一格。
//
// 判据来自旧 TS 的类型（`apis/share.ts:8-15`）+ runner 读的那一条可选链。两处要说清：
//
//   - **`code` 的口径与本文件头部那条"刻意不一致"**：这里走 [Envelope.EnsureOK]
//     （数字 0 即成功），不复制旧实现那句字面相等；
//   - **载荷整块缺失 ⇒ 报错**，而"载荷在但没有 linkID"⇒ 交回 `LinkID == nil`。
//     两档分开是因为它们的下一步不同：前者是"这一族的形状变了"，
//     后者是"上游建了一条我们删不掉的链接"。把前者兜成后者会静默留下公开链接。
//
// ⚠ `getOutLinkResSet` 有多条时会怎样**未测试**（旧实现读 `[0]`，本层也读 `[0]`）。
// 而 PC 客户端那一条 XML 传输里同一级键读的是 `.getOutLinkResSet.getOutLinkResOne`
// （`docs/capture/pcclient-api.js:329`）⇒ 若这一口回的也是那一种**对象包装**而不是数组，
// 下面的 `[]any` 断言会不成立、落到"没有 linkID"那一档。这是**刻意选的一侧**：
// 宁可把这一条播成失败让人去核对，也不猜一种包装形状把它读成成功。
func ParseCloudOutLink(envelope *Envelope) (CloudOutLink, *core.Error) {
	payload, perr := envelope.EnsureOK()
	if perr != nil {
		return CloudOutLink{}, perr
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return CloudOutLink{}, core.NewParse(fmt.Sprintf(
			"getOutLink 的 data 不是对象，实得 %T（载荷整块缺失时不能读成'没有 linkID'，见函数头）", payload))
	}
	res, isObj := obj["getOutLinkRes"].(map[string]any)
	if !isObj {
		return CloudOutLink{}, core.NewParse(fmt.Sprintf(
			"getOutLink 的 data 里没有 getOutLinkRes 对象，实得 %T：形状与本层已知的两份出处都不符", obj["getOutLinkRes"]))
	}
	if set, isList := res["getOutLinkResSet"].([]any); isList && len(set) > 0 {
		// 只读**第一项**（旧实现 `getOutLinkResSet?.[0]?.linkID`）。多条会怎样未测试。
		first, _ := set[0].(map[string]any)
		return CloudOutLink{LinkID: optStrField(first, "linkID")}, nil
	}
	// 空数组与"那一格缺席"落到同一档：上层要的是"能不能删"，两档都答"不能"，
	// 而它必须把这一档**播成失败**（见 [CloudOutLink] 那一格）。
	return CloudOutLink{}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二口：POST /outlink/v1.0/delOutLink（**写**：取消分享；不可回滚）
// ─────────────────────────────────────────────────────────────────────────────

// CloudDelOutLinkRequest 构造取消分享那一发（**写**）。
//
// body 两键逐字（`apis/share.ts:70-78`）。⚠ 这一口**不带** [OutLinkHeaders] 那十三枚
// —— 旧实现那里没有第三参数，只有主 http 实例的静态头（见文件头那处不对称）。
//
// 空列表 ⇒ 不发（与 [CloudBatchTrashRequest] 同一条纪律）。
func CloudDelOutLinkRequest(
	authorization, deviceID, account string, linkIDs []string,
) (core.HTTPRequest, *core.Error) {
	if len(linkIDs) == 0 {
		return core.HTTPRequest{}, core.NewInternal("delOutLink 收到空外链列表：没有要取消的东西就不发这一枪")
	}
	if account == "" {
		return core.HTTPRequest{}, core.NewInternal("delOutLink 缺 account：commonAccountInfo 是这一枪的定位依据（错误文本刻意不带它的值）")
	}
	body := map[string]any{
		"delOutLinkReq": map[string]any{
			"linkIDs": linkIDs,
			"commonAccountInfo": map[string]any{
				"account":     account,
				"accountType": 1,
			},
		},
	}
	request, err := core.NewRequestPost(BaseYun139Orchestration + "/outlink/v1.0/delOutLink").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return AiAvatarHeaders(request, authorization, "", deviceID), nil
}

// ParseCloudDelOutLink 只判"这一枪成不成"。
//
// 旧实现**完全不看**它的响应（`runners.ts:80` 那句 `await tryRequest(delOutLink, ...)`
// 把返回的元组直接丢弃）⇒ 一次失败的取消分享在旧实现里是**静默**的，
// 而那意味着"用户网盘里留了一条公开外链，日志里一个字都没有"。
// 这里不复制那份静默：[Envelope.EnsureOK] 非 0 码与 `success:false` 都回错误。
//
// ⚠ 它**不要求**任何字段（旧实现不读，我们也没有样本能读出 `data` 的形状）。
func ParseCloudDelOutLink(envelope *Envelope) *core.Error {
	_, perr := envelope.EnsureOK()
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudGetOutLink 建一条分享外链（**写**：这条链接在用户网盘里是公开的，
// 直到 [Upstream.AppCloudDelOutLink] 把它取消）。
//
// ⚠ 错误文本里不会出现 `account`（手机号）与文件 id 之外的任何凭据；
// 这一族的 `authorization` 整串只进请求头，不进日志。
func (u *Upstream) AppCloudGetOutLink(
	ctx context.Context, authorization, deviceID string, in CloudOutLinkInput,
) (CloudOutLink, *core.Error) {
	request, perr := CloudGetOutLinkRequest(authorization, deviceID, in)
	if perr != nil {
		return CloudOutLink{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudOutLink{}, perr
	}
	return ParseCloudOutLink(envelope)
}

// AppCloudDelOutLink 取消一批分享外链（**写**，不可回滚）。
func (u *Upstream) AppCloudDelOutLink(
	ctx context.Context, authorization, deviceID, account string, linkIDs []string,
) *core.Error {
	request, perr := CloudDelOutLinkRequest(authorization, deviceID, account, linkIDs)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	return ParseCloudDelOutLink(envelope)
}
