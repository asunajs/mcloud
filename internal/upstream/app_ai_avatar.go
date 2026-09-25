// AI 头像（#10 `ai-avatar`，旧项目「AI新头像」）这一族的上游口：
// `m.mcloud.139.com/market/rabactive/**` 六口 + 网盘侧 `personal-kd-njs.yun.139.com/hcy/file/list`。
//
// 对应 `docs/business-feature-inventory.md` §2.3 的 `ai-avatar` 那一节，
// 旧实现是 `packages/core/src/tasks/ai-avatar/api.ts`（八口全在那一个对象里）。
//
// # ⚠ 证据分两半，而且不对称 —— 读代码前先看这一段
//
//   - **`hcy/file/list` 有实拍**：`test/fixtures/http/3d.json` 之外的
//     `test/fixtures/http/ref.json` 里有两条 `POST personal-kd-njs.yun.139.com/hcy/file/list`
//     （App 真机流量）。信封是 `{code:"0000", success:true, message:"请求成功", data:{items, nextPageCursor, parentFilePathInfo}}`
//     —— `code` 是**字符串** `"0000"`（不是数字 0），条目里 `fileId` / `name` 两格是这一族唯一要用的。
//   - **`rabactive/*` 六口零实拍**：本仓所有归档里 `grep rabactive` **一条都没有**
//     （那六口是活动 H5 页发的，抓包面当时没覆盖这一页）。⇒ 这六口的请求面**只对齐到
//     "与旧 TS 字面量一致"**。按 v4 口径：接口层按旧实现落码、业务层照它推进，
//     但**只钉"发得出"，不钉语义**（哪些格子是猜的逐条写在各自的注释里）。
//     ⚠ 2026-09-25 真账号跑过一趟，"一个真打都没有过"那句**已不成立**：真打过，而且
//     **六口全部停在 HTTP 状态这一层** —— 合成那一口回 `405`（编排按 5 次重试，每次同码），
//     读剩余次数与抽奖两口回 `404`，两个号一字不差。⇒ 信封解析一次都没进，
//     "哪几格是猜的"这一问今天**还没资格被回答**；`405` 最可疑的是方法、`404` 最可疑的是路径，
//     但"这一期活动没了"与"我们少一环"**同形**，只能靠人工在活动站 H5 里打开这一页
//     抓一次加载链分辨（登记在 `docs/pending-decisions.md` **R17**，界面怎么摆是 **Q38**）。
//
// # 这是本仓库的**第三套**请求头集合（不是笔误）
//
// 签到族走 [WithH5Headers]（H5 那套 `activityid`/`appversion`/`referer`/`showloading`，
// 由 `createTaskHttp` 而来）；App 原生走 [WithAppHeaders]（`x-yun-*` 那一整族）。
// 这一族两边都不是：旧 TS 用的是**主 http 实例**（`packages/shared/src/http/base.ts:34`
// 的 `createHttp`，账号初始化见 `utils/init-account.ts:40-55`），它的静态头只有
// `user-agent` + `content-type`（带 charset）+ `deviceid`，动态头按**主机**决定：
//
//   - `authorization`（Basic，账号侧）：主机以 `yun.139.com` / `mcloud.139.com` /
//     `feixin.10086.cn` / `mail.10086.cn` 结尾才带（`base.ts:12`）。
//     ⇒ `m.mcloud.139.com` 与 `personal-kd-njs.yun.139.com` **两个都命中**。
//   - `jwttoken`：**只有** `caiyun.feixin.10086.cn` / `mrp.mcloud.139.com` /
//     `m.mcloud.139.com` 这三个主机带（`base.ts:14`）。
//     ⇒ 所以 `hcy/file/list` 那一口**有 authorization、没有 jwttoken** —— 这不是漏写。
//
// 于是本文件的 [AiAvatarHeaders] 单独一份，且**故意不复用** [WithH5Headers]：
// 复用会让 `activityid: sign_in_3`、`referer` 这些这一页从不发的头混进去，
// 而"上游认不认多出来的头"这件事零样本 ⇒ 宁缺勿滥，按旧实现字面发。
//
// # ⚠ 这一页的 `jwttoken` **使用公共来源**（临时态，AGENTS §1.12）
//
// 旧 TS 的那串票来自 `init-account.ts:46` 的 `getJwtToken(http, toSourceId)`，
// 与本仓签到页那条建会话配方同源 ⇒ 接线时沿用 `RecipeSignInPage`（`internal/app/signin_world.go`）。
// 这一族**没有**任何"该页特有取法"的样本（连一次 rabactive 请求都没抓过）。
// 抓到特有取法时要**主动更正**这一处并把来源补记进 `docs/capture/`。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// 这一族的两个基址。
//
// ⚠ `BaseRabactive` 的前缀是 `/market/` 而不是这一族其他活动接口常见的 `/ycloud/`
// （坑表 §5.1"签到域的主机与路径前缀有两套"）。拼成 `/ycloud/market/rabactive/…`
// 不会报"路径不存在"，会回一个 404 页面 —— 而 404 页面在 [ParseEnvelope] 那一层
// 表现为"响应体不是合法 JSON"，看着像解析器的错。
//
// ⚠ 2026-09-25 实测校正：真跑那两口的 404 / 405 **在状态码那一层就被拦下了**
// （`errorFromStatus` → 一句"不存在: HTTP 404"），所以上面那条"会掉进解析器"的担心
// 只在**上游回 200 + 一个 HTML 错误页**时才成立。今天那两口的现场是状态码，
// 因此**不能**拿它当"我们前缀拼错了"的证据 —— 分辨办法仍是 R17 那一次人工抓包。
const (
	// BaseRabactive 是 AI 头像活动站的接口根（旧 TS `api.ts:4-5` 的两个前缀合成一个）。
	BaseRabactive = "https://m.mcloud.139.com/market/rabactive"
	// BasePersonalKD 是网盘个人云的文件接口（这一族只用它的 `hcy/file/list`）。
	//
	// ⚠ 它**不在** [BaseNoteMcloud] 那个域上：旧 TS 写死 `personal-kd-njs.yun.139.com`
	// （`api.ts:6`），实拍也是这一台。两者是不同入口，别"顺手"换成仓库里现成的那个。
	BasePersonalKD = "https://personal-kd-njs.yun.139.com"
)

// AiAvatarCatalogMarketName 是 `catalogFullPath` 的默认市场名（旧 TS `api.ts:70` 的默认参数）。
//
// ⚠ 它是"活动侧给这个玩法建的那个网盘目录"的键名，语义**未验证**（零实拍）；
// 这一口只用来把 `fullPath_sub` 的最后一段当 `parentFileId` 用。
const AiAvatarCatalogMarketName = "rabbityearpic"

// AiAvatarImageExt 是 `merge` 请求体里那枚固定的扩展名（旧 TS 写死 `"jpg"`）。
//
// ⚠ 与"喂进去的图其实是 PNG"这件事**不矛盾也不要"顺手改对"**：
// 旧实现就是这么发、且这一族的图片是本地造的噪点图（`png-generator.ts`），
// 上游按哪个扩展名存这张图**未测试**。改这一个字节等于改一次接口。
const AiAvatarImageExt = "jpg"

// 合成状态的三档（旧 TS `task.ts:57-76` 的字面判据）。
//
// ⚠ **档位表未验证**：`3` 被读成"成功"、`4` 被读成"失败"，其余值（含 `1`/`2`）一律"继续等"。
// 这一族的实拍响应**一条都没有** ⇒ 只有几档、会不会有第五档、终态是不是 3/4，全是从
// 旧实现的分支形状反推的。要核它：真跑一次生成，把每次 `getStatus` 的 `status` 记下来。
const (
	AiAvatarStatusSuccess int64 = 3
	AiAvatarStatusFailed  int64 = 4
)

// AiAvatarDrawCodeStopped 是 `draw` 的那个"今天就到此为止"的码（旧 TS `task.ts:128` 的 `408`）。
//
// ⚠ 旧实现**只认这一个非 0 码**并 `break`（其余非 0 是 `continue` 下一次）。
// `408` 到底是"次数用完"还是"上游限流"没有任何成文与样本 ⇒ 只在 app 层当"停止"用，
// 不对外解释它。
const AiAvatarDrawCodeStopped int64 = 408

// AiAvatarHeaders 给这一族的请求补上"主 http 实例"那套头（见文件头第三套头的说明）。
//
// `authorization` / `jwt` 传空串 = **不发那一枚**（旧实现是 `if (basicToken)` /
// `if (jwtToken)` 两个独立开关，缺一个不会缺另一个）。
//
// ⚠ `content-type` 与 [WithH5Headers] 的做法**刻意不同**：这里**恒发**（哪怕 GET 裸体），
// 因为旧 TS 把它放在静态头里、与有没有 body 无关。没有实拍能判"上游在不在乎"，
// 所以取"与旧实现逐字一致"这一侧。
func AiAvatarHeaders(
	request core.HTTPRequest, authorization, jwt, deviceID string,
) core.HTTPRequest {
	request.Headers = dropHeader(request.Headers, "content-type")
	request = request.WithHeader("User-Agent", CaiyunAndroidWebUA)
	if deviceID != "" {
		request = request.WithHeader("deviceid", deviceID)
	}
	if authorization != "" {
		// 旧 TS 走 `headers.set`，名字是 `authorization`；这里用规范拼法（HTTP 头大小写不敏感，
		// 而 [core.HTTPRequest.WithHeader] 是**追加** ⇒ 同名只会有一枚，见 [dropHeader] 那条）。
		request = request.WithHeader("Authorization", authorization)
	}
	if jwt != "" {
		request = request.WithHeader("jwttoken", jwt)
	}
	return request.WithHeader("Content-Type", H5ContentType)
}

// ─────────────────────────────────────────────────────────────────────────────
// 读判据：getHashLeft / draw
// ─────────────────────────────────────────────────────────────────────────────

// AiAvatarHashLeftRequest 构造 `GET /market/rabactive/image/getHashLeft`。**只读**。
//
// 旧 TS `api.ts:43-45`：无查询串、无 body。
func AiAvatarHashLeftRequest(authorization, jwt, deviceID string) core.HTTPRequest {
	return AiAvatarHeaders(
		core.NewRequestGet(BaseRabactive+"/image/getHashLeft"), authorization, jwt, deviceID)
}

// ParseAiAvatarHashLeft 解析"还剩几次可抽"。
//
// ⚠ `result` 在旧 TS 的类型是 `number`，但**这一格零样本**：
// 上游可能回 `null`（旧实现用 `typeof === "number"` 挡这一手，见 `task.ts:114`）。
// 所以回 `nil` 表示"没给"，**不是**"0 次"—— 把它读成 0 会让"还能抽"变成"已用完"。
func ParseAiAvatarHashLeft(envelope *Envelope) (*int64, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return nil, perr
	}
	if payload == nil {
		return nil, nil
	}
	left, ok := asInt64(payload)
	if !ok {
		return nil, core.NewParse(fmt.Sprintf(
			"getHashLeft 的 result 不是数字（实得 %T；旧实现按 typeof==='number' 挡这一手）", payload))
	}
	return &left, nil
}

// AiAvatarPrize 是 `draw` 抽出的一格奖品名。
type AiAvatarPrize struct {
	// PrizeName 是 `result.prizeName`。
	PrizeName *string
	// ContentName 是 `result.contentName`。
	//
	// ⚠ 旧实现**优先读它**、读不到才读 `prizeName`（`task.ts:133`）。两格谁才是"奖品名"
	// 未验证（零样本），所以这里两格都交出去、由 app 层按那个优先序拼文案。
	ContentName *string
}

// AiAvatarDrawRequest 构造 `GET /market/rabactive/image/draw` —— **写请求**（抽一次）。
//
// 该不该发由调用方管（额度见 [ParseAiAvatarHashLeft]），本层只负责发出去、解析回来。
func AiAvatarDrawRequest(authorization, jwt, deviceID string) core.HTTPRequest {
	return AiAvatarHeaders(
		core.NewRequestGet(BaseRabactive+"/image/draw"), authorization, jwt, deviceID)
}

// ParseAiAvatarPrize 解析一次抽奖的响应。
//
// 与 [ParseFunaiLottery] 同一处置：**没有 `result` 不判失败**（成败已由 `code` 说过一次），
// 两格名字全是指针（没有任何一档样本能告诉我们哪一格是必填）。
func ParseAiAvatarPrize(envelope *Envelope) (AiAvatarPrize, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return AiAvatarPrize{}, perr
	}
	obj, _ := payload.(map[string]any)
	return AiAvatarPrize{
		PrizeName:   optStrField(obj, "prizeName"),
		ContentName: optStrField(obj, "contentName"),
	}, nil
}

// AiAvatarDrawResult 是"抽一次"的**三态**结果：抽中了 / 被叫停 / 真失败。
//
// 为什么要有这一层，而不是让调用方去判 [ParseAiAvatarPrize] 的错误串：
// 旧实现（`task.ts:120-135`）对 `code` 非 0 分两种处置 —— `408` 是 `break`（今天到此为止），
// 其余是 `continue`（换下一次尝试）。这个分叉**必须**在类型上分得出来，
// 否则调用方只能拿错误消息做子串匹配 —— 而错误消息是我们自己拼的中文句子
// （见 [EnsureOKWithHint]），改一个字就会把"停止"念成"重试"，
// 表现是对一个已经拒绝的账号继续连打五枪。
//
// ⚠ 三态的第三态（真失败）里 `Err` 非空，另两格**都不作数**。
type AiAvatarDrawResult struct {
	// Prize 是抽中的那一格（仅当 `code` 为 0）。
	Prize AiAvatarPrize
	// Stopped 表示上游回了 [AiAvatarDrawCodeStopped]。
	//
	// ⚠ 这一档的**响应体形状零样本**：`408` 那条有没有 `result`、里面是什么，
	// 本仓没有任何依据 ⇒ 这里不下钻，`Prize` 留空。
	// 要核它：真抽到次数用完那一次，把响应体逐字记进 `docs/capture/`。
	Stopped bool
}

// ParseAiAvatarDraw 解析一次抽奖，并把 [AiAvatarDrawCodeStopped] 折成"停止"那一态。
//
// 判定只看 `code`（[Envelope.Code]），**不看错误消息**（理由见 [AiAvatarDrawResult]）。
// ⚠ `code` 字段本身缺失时按"不是 408"处理 ⇒ 交给 [ParseAiAvatarPrize] 里的
// [EnsureOKWithHint]：它认"没有 code 就没有信封判定"这一手（见 [Envelope.EnsureOK]）。
func ParseAiAvatarDraw(envelope *Envelope) (AiAvatarDrawResult, *core.Error) {
	if code, ok := envelope.Code(); ok && code == AiAvatarDrawCodeStopped {
		return AiAvatarDrawResult{Stopped: true}, nil
	}
	prize, perr := ParseAiAvatarPrize(envelope)
	return AiAvatarDrawResult{Prize: prize}, perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 合成一张：merge → getStatus（轮询）→ upload
// ─────────────────────────────────────────────────────────────────────────────

// AiAvatarTask 是 `merge` 与 `getStatus` **共用**的解析结果（旧 TS 同一个 `AiAvatarStatus`）。
type AiAvatarTask struct {
	// TaskID 是 `result.taskId` —— `merge` 给出、`getStatus` 带回去。
	//
	// 旧实现在 `merge` 之后立刻判 `!taskId` ⇒ 没有它这条链走不下去（`task.ts:52`）。
	// 这里仍做成指针：**缺了要能被叫出来**，而不是被默认成空串继续发下一枪。
	TaskID *string
	// Status 是 `result.status` —— 见 [AiAvatarStatusSuccess] / [AiAvatarStatusFailed] 那条"档位未验证"。
	Status *int64
	// ImageURL 是 `result.imageUrl` —— 只在"成功"那一档有意义；app 层要**下载它**再 base64 才能发 upload。
	ImageURL *string
}

// AiAvatarMergeRequest 构造 `POST /market/rabactive/ai/image/merge`。
//
// body 两键逐字（旧 TS `api.ts:51-56`）：`imageBase64`、`imageExt: "jpg"`。
//
// ⚠ `imageBase64` 是**裸 base64**，不带 `data:image/png;base64,` 前缀
// （旧 TS `task.ts:38-40` 专门把那一段正则去掉）。带上前缀发出去会怎样：零样本。
func AiAvatarMergeRequest(
	authorization, jwt, deviceID, imageBase64 string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(BaseRabactive + "/ai/image/merge").
		WithJSONBody(map[string]any{"imageBase64": imageBase64, "imageExt": AiAvatarImageExt})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return AiAvatarHeaders(request, authorization, jwt, deviceID), nil
}

// AiAvatarStatusRequest 构造 `POST /market/rabactive/ai/image/getStatus`（查一次合成进度）。
//
// body 只有 `taskId` 一键（旧 TS `api.ts:58-60`）。轮询节奏（几次、间隔多久）在 app 层。
func AiAvatarStatusRequest(
	authorization, jwt, deviceID, taskID string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(BaseRabactive + "/ai/image/getStatus").
		WithJSONBody(map[string]any{"taskId": taskID})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return AiAvatarHeaders(request, authorization, jwt, deviceID), nil
}

// ParseAiAvatarTask 解析 `merge` / `getStatus` 的响应。
//
// 三格全是指针：`merge` 那一次只有 `taskId`，`getStatus` 才有 `status` 与 `imageUrl`，
// 而**同一段代码**要吃这两次 —— 所以"缺哪格"是调用方的判据，不是这一层的报错理由
// （旧 TS 也正是分两处各判各的：`task.ts:52` 与 `task.ts:60-69`）。
func ParseAiAvatarTask(envelope *Envelope) (AiAvatarTask, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return AiAvatarTask{}, perr
	}
	obj, _ := payload.(map[string]any)
	return AiAvatarTask{
		TaskID:   optStrField(obj, "taskId"),
		Status:   optInt64Field(obj, "status"),
		ImageURL: optStrField(obj, "imageUrl"),
	}, nil
}

// AiAvatarUploadRequest 构造 `POST /market/rabactive/image/upload` —— **这一口会写用户网盘**。
//
// body 两键（旧 TS `api.ts:62-64`）：`originImage`（本地那张噪点原图）与
// `mergeImage`（上游生成图下载回来后的 base64），**两张同时上传**。
//
// ⚠⚠ 这是本文件唯一的"往用户网盘写东西"的口：旧实现随后要靠
// [AiAvatarCatalogPathRequest] + [AiAvatarWorkListRequest] + [CloudFileListRequest]
// 三枪**按文件名**把它找回来（`#7 after` 清理那一族的前置）。
// "同名碰撞即错删/错用"这条风险写在 `business-feature-inventory.md` §2.3 `ai-avatar`。
// `file/create` 本身就回 `fileId`，旧代码这里没用它 —— 那是旧实现的偷懒，
// 但在拿到该口的响应实拍之前，**不擅自改成"从上传响应拿 id"**（那等于凭空猜新接口）。
func AiAvatarUploadRequest(
	authorization, jwt, deviceID, originImage, mergeImage string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(BaseRabactive + "/image/upload").
		WithJSONBody(map[string]any{"originImage": originImage, "mergeImage": mergeImage})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return AiAvatarHeaders(request, authorization, jwt, deviceID), nil
}

// ParseAiAvatarUploadID 取 `upload` 回的那个 id（`result` 是**裸字符串**）。
//
// ⚠ 旧 TS 的类型是 `ApiResponse<string>`，且只检查 `code === 0` 就收下
// （`task.ts:72`）。**空串怎么办没有样本** ⇒ 这里把空串判成解析失败：
// 拿着空 id 继续走，后面那三枪"按名字找文件"会静默匹配不到任何东西，
// 于是清理那一族什么都收不到 —— 表现是"网盘里多了张图，没人知道"。
func ParseAiAvatarUploadID(envelope *Envelope) (string, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return "", perr
	}
	text, ok := payload.(string)
	if !ok {
		return "", core.NewParse(fmt.Sprintf("upload 的 result 不是字符串，实得 %T", payload))
	}
	if text == "" {
		return "", core.NewParse("upload 回了空字符串 id（旧实现会照收，但拿着它后续三枪都白打）")
	}
	return text, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 找回那张图（清理的前置）：getWorkList / catalogFullPath / hcy/file/list
// ─────────────────────────────────────────────────────────────────────────────

// AiAvatarWork 是 `getWorkList` 的一条作品。
type AiAvatarWork struct {
	// WorkID 是 `result[].id` —— 与 `upload` 回的那个 id 对得上才作数。
	WorkID string
	// ImgURL / QrImgURL 是 `imgUrl` / `qrImgUrl` —— 只用它们的**文件名**去网盘里匹配。
	ImgURL   *string
	QrImgURL *string
}

// AiAvatarWorkListRequest 构造 `GET /market/rabactive/image/getWorkList?type=<n>`。
//
// 旧 TS 默认 `type = 1`（`api.ts:66`）。⚠ 这套 type 的取值表没有成文，
// 本层不枚举（只把参数交出去），app 层固定传 1。
func AiAvatarWorkListRequest(
	authorization, jwt, deviceID string, workType int64,
) core.HTTPRequest {
	return AiAvatarHeaders(
		core.NewRequestGet(fmt.Sprintf("%s/image/getWorkList?type=%d", BaseRabactive, workType)),
		authorization, jwt, deviceID)
}

// ParseAiAvatarWorkList 解析作品列表。
//
// `result` 不是数组 ⇒ 解析失败（旧 TS 直接 `result || []` 兜成空，
// 那会把"形状变了"念成"这个号没有作品"，而后者的界面读法是"干净"）。
func ParseAiAvatarWorkList(envelope *Envelope) ([]AiAvatarWork, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return nil, perr
	}
	if payload == nil {
		return nil, nil
	}
	items, ok := payload.([]any)
	if !ok {
		return nil, core.NewParse(fmt.Sprintf("getWorkList 的 result 不是数组，实得 %T", payload))
	}
	works := make([]AiAvatarWork, 0, len(items))
	for _, raw := range items {
		obj, ok := raw.(map[string]any)
		if !ok {
			continue
		}
		id, _ := obj["id"].(string)
		if id == "" {
			// 数字 id 也认（旧 TS 声明成 string，但活动接口常回数字）。
			if n, numOK := asInt64(obj["id"]); numOK {
				id = fmt.Sprintf("%d", n)
			}
		}
		if id == "" {
			continue
		}
		works = append(works, AiAvatarWork{
			WorkID:   id,
			ImgURL:   optStrField(obj, "imgUrl"),
			QrImgURL: optStrField(obj, "qrImgUrl"),
		})
	}
	return works, nil
}

// AiAvatarCatalogPathRequest 构造
// `GET /market/rabactive/image/catalogFullPath?marketName=<name>`。
func AiAvatarCatalogPathRequest(
	authorization, jwt, deviceID, marketName string,
) core.HTTPRequest {
	return AiAvatarHeaders(
		core.NewRequestGet(
			fmt.Sprintf("%s/image/catalogFullPath?marketName=%s", BaseRabactive, PercentEncode(marketName))),
		authorization, jwt, deviceID)
}

// ParseAiAvatarCatalogParentFileID 把 `result.fullPath_sub` 切成"这个目录的 fileId"。
//
// 旧 TS（`task.ts:146`）：`fullPath_sub.split("/").filter(Boolean).pop()`
// —— 按 `/` 切、丢空段、取最后一段。回 `nil` = 拿不到（旧实行的读法是"那就别找了"）。
//
// ⚠ "路径的最后一段就是 `fileId`"这一格**未验证**：网盘侧的 `fileId` 确实是
// 这种形态（实拍 `hcy/file/list` 里 `"fileId":"/synbox"` 这种带斜杠的全路径也在），
// 但那是**别的目录**。要核它：真跑一次后比对 `catalogFullPath` 与 `file/list` 的返回。
//
// **没有 `result` 交回 `nil`（拿不到）而不是报错**：旧实现走的是 `result?.fullPath_sub`
// —— 一路可选链，缺了就是"那就别找了"。把"上游这次没给"报成解析失败，
// 表现是清理那一族整条链红掉，而不是"这一轮没找到目录，跳过"。
func ParseAiAvatarCatalogParentFileID(envelope *Envelope) (*string, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return nil, perr
	}
	if payload == nil {
		return nil, nil
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return nil, core.NewParse(fmt.Sprintf("catalogFullPath 的 result 不是对象，实得 %T", payload))
	}
	full, ok := obj["fullPath_sub"].(string)
	if !ok || full == "" {
		return nil, nil
	}
	segments := make([]string, 0, 8)
	start := 0
	for i := 0; i <= len(full); i++ {
		if i == len(full) || full[i] == '/' {
			if segment := full[start:i]; segment != "" {
				segments = append(segments, segment)
			}
			start = i + 1
		}
	}
	if len(segments) == 0 {
		return nil, nil
	}
	last := segments[len(segments)-1]
	return &last, nil
}

// CloudFile 是 `hcy/file/list` 条目里本族要用的两格。
type CloudFile struct {
	// FileID 是 `data.items[].fileId`（实拍形态如 `/synbox`、`FFa...`）。
	FileID string
	// Name 是 `data.items[].name` —— 与作品图片的**文件名**比对。
	Name string
}

// CloudFileList 是 `hcy/file/list` 一页的结果。
type CloudFileList struct {
	Items []CloudFile
	// NextPageCursor 是 `data.nextPageCursor` —— 实拍两条里**都有这一格**（值可能是 `null`）。
	//
	// ⚠ 本族**不翻页**：旧实现固定 `pageSize: 100` 一次（`api.ts:76-80`），
	// 多于 100 个文件时后面那些**静默匹配不到**，表现是"临时文件没被清掉"而不是报错。
	// 与 `prizes` 那一族同一处置（见 pending-decisions Q11），登记不翻。
	NextPageCursor *string
}

// CloudFileListRequest 构造 `POST /hcy/file/list`（**只读**：列一个目录下的文件）。
//
// # 这一口是本文件唯一有实拍的口
//
// `test/fixtures/http/ref.json` 两条（App 真机）：`parentFileId: "/"`、
// body 里另有 `fields`、`ownerId: null`、`pageInfo:{needTotalCount:0,pageCursor:null,pageSize:100}`
// 等键。⚠ **本层不照抄那一份全 body**：旧 TS 的 builder（`api.ts:76-80`）只发
// `parentFileId` / `pageInfo.pageSize` / `imageThumbnailStyleList` / `orderBy` / `orderDirection`
// 五键，实拍那一份是**另一个客户端**（okhttp + 完整 `x-yun-*`）发的。
// 取向：按**本仓这条链**的旧实现发（它曾经可用），实拍只用来钉
// ①URL 与信封形状、②`items[].fileId/name` 确实存在。差异登记成 R16。
//
// 头集合：走 [WithAppHeaders]（网盘侧要那一整套 `x-yun-*`）+ `Authorization` +
// 旧 TS 那两枚（`x-yun-module-type: 100`、`x-yun-op-type: 1`）。
// ⚠ **不带 `jwttoken`** —— 这台主机不在旧 TS 的 `TARGET_HOSTNAMES` 名单里（见文件头）。
func CloudFileListRequest(
	profile *AppProfile, timestamp int64, authorization, parentFileID string, pageSize int64,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"parentFileId":            parentFileID,
		"pageInfo":                map[string]any{"pageSize": pageSize},
		"imageThumbnailStyleList": []string{"Big", "Small"},
		"orderDirection":          "DESC",
		"orderBy":                 "updated_at",
	}
	request, err := core.NewRequestPost(BasePersonalKD + "/hcy/file/list").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = WithAppHeaders(request, profile, timestamp)
	if authorization != "" {
		request = request.WithHeader("Authorization", authorization)
	}
	return request.
		WithHeader("x-yun-module-type", "100").
		WithHeader("x-yun-op-type", "1"), nil
}

// ParseCloudFileList 解析 `hcy/file/list` 的一页。
//
// # 信封与另外六口**不同**
//
// 实拍是 `{code:"0000", success:true, message:"请求成功", data:{items:[…]}}` ——
// `code` 是**字符串** `"0000"`（[asInt64] 认字符串数字，所以 [Envelope.EnsureOK] 判它成功），
// 载荷在 `data` 而不是 `result`（[Envelope.Payload] 两键都认，先 `data`）。
//
// 旧 TS 只看 `success`（`task.ts:150`）。这里同样**以 `success` 为准**，
// 但 `code` 不是 `"0000"` 时也照样交回载荷？—— 不：`EnsureOK` 已经把
// `success:false` 与**非 0 数字码**都拦掉了，这里只补一件事：
// `data.items` 不是数组 ⇒ 解析失败（旧实现兜成 `[]`，那会把"形状变了"念成"目录是空的"）。
func ParseCloudFileList(envelope *Envelope) (CloudFileList, *core.Error) {
	payload, perr := envelope.EnsureOK()
	if perr != nil {
		return CloudFileList{}, perr
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return CloudFileList{}, core.NewParse(fmt.Sprintf(
			"file/list 的 data 不是对象，实得 %T（这一口与 rabactive 那六口不同信封）", payload))
	}
	rawItems, present := obj["items"]
	if !present {
		// `data` 里没有 `items`：实拍两条都有这一格，所以"没有"只能是"这一页确实没文件"
		// 或形状变了 —— 两种都交回**空列表**，因为调用方拿它做的是"按名字匹配"，
		// 匹配不到就是匹配不到，不会因此少删多删（这一口只读）。
		return CloudFileList{Items: []CloudFile{}}, nil
	}
	items, ok := rawItems.([]any)
	if !ok {
		return CloudFileList{}, core.NewParse(fmt.Sprintf("file/list 的 data.items 不是数组，实得 %T", rawItems))
	}
	out := CloudFileList{Items: make([]CloudFile, 0, len(items)), NextPageCursor: optStrField(obj, "nextPageCursor")}
	for _, raw := range items {
		entry, ok := raw.(map[string]any)
		if !ok {
			continue
		}
		fileID := optStrField(entry, "fileId")
		name := optStrField(entry, "name")
		if fileID == nil || name == nil {
			continue
		}
		out.Items = append(out.Items, CloudFile{FileID: *fileID, Name: *name})
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppAiAvatarHashLeft 读"还能抽几次"（只读）。见 [ParseAiAvatarHashLeft] 那条"nil ≠ 0"。
func (u *Upstream) AppAiAvatarHashLeft(
	ctx context.Context, authorization, jwt, deviceID string,
) (*int64, *core.Error) {
	envelope, perr := u.Call(ctx, AiAvatarHashLeftRequest(authorization, jwt, deviceID))
	if perr != nil {
		return nil, perr
	}
	return ParseAiAvatarHashLeft(envelope)
}

// AppAiAvatarDraw 抽一次（**写**）。该不该抽由调用方判（额度 + [AiAvatarDrawCodeStopped]）。
//
// 返回三态（[AiAvatarDrawResult]）：`code == 0` 是抽中了，`408` 是"今天到此为止"，
// 其余非 0 交回错误 —— 这个分叉就是旧实现里 `break` 与 `continue` 的分叉。
func (u *Upstream) AppAiAvatarDraw(
	ctx context.Context, authorization, jwt, deviceID string,
) (AiAvatarDrawResult, *core.Error) {
	envelope, perr := u.Call(ctx, AiAvatarDrawRequest(authorization, jwt, deviceID))
	if perr != nil {
		return AiAvatarDrawResult{}, perr
	}
	return ParseAiAvatarDraw(envelope)
}

// AppAiAvatarMerge 提交本地噪点图换一次合成任务（只读性质：不改网盘）。
func (u *Upstream) AppAiAvatarMerge(
	ctx context.Context, authorization, jwt, deviceID, imageBase64 string,
) (AiAvatarTask, *core.Error) {
	request, perr := AiAvatarMergeRequest(authorization, jwt, deviceID, imageBase64)
	if perr != nil {
		return AiAvatarTask{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return AiAvatarTask{}, perr
	}
	return ParseAiAvatarTask(envelope)
}

// AppAiAvatarStatus 查一次合成进度（只读）。轮询节奏在调用方。
func (u *Upstream) AppAiAvatarStatus(
	ctx context.Context, authorization, jwt, deviceID, taskID string,
) (AiAvatarTask, *core.Error) {
	request, perr := AiAvatarStatusRequest(authorization, jwt, deviceID, taskID)
	if perr != nil {
		return AiAvatarTask{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return AiAvatarTask{}, perr
	}
	return ParseAiAvatarTask(envelope)
}

// AppAiAvatarUpload 把"原图 + 合成图"两张一起写进用户网盘（**写，且产物落在网盘**）。
//
// ⚠ 调用方有责任把返回的 id 交给清理那一族（`getWorkList` + `catalogFullPath` + `file/list`）。
// 本仓库的账本层是 `internal/exec/artifacts.go` 的 `ArtifactLedger`（今天**没有调用点**）。
func (u *Upstream) AppAiAvatarUpload(
	ctx context.Context, authorization, jwt, deviceID, originImage, mergeImage string,
) (string, *core.Error) {
	request, perr := AiAvatarUploadRequest(authorization, jwt, deviceID, originImage, mergeImage)
	if perr != nil {
		return "", perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return "", perr
	}
	return ParseAiAvatarUploadID(envelope)
}

// AppAiAvatarWorkList 读作品列表（只读）。
func (u *Upstream) AppAiAvatarWorkList(
	ctx context.Context, authorization, jwt, deviceID string, workType int64,
) ([]AiAvatarWork, *core.Error) {
	envelope, perr := u.Call(ctx, AiAvatarWorkListRequest(authorization, jwt, deviceID, workType))
	if perr != nil {
		return nil, perr
	}
	return ParseAiAvatarWorkList(envelope)
}

// AppAiAvatarCatalogParentFileID 读活动目录的 `fileId`（只读）。见 [ParseAiAvatarCatalogParentFileID]。
func (u *Upstream) AppAiAvatarCatalogParentFileID(
	ctx context.Context, authorization, jwt, deviceID, marketName string,
) (*string, *core.Error) {
	envelope, perr := u.Call(ctx, AiAvatarCatalogPathRequest(authorization, jwt, deviceID, marketName))
	if perr != nil {
		return nil, perr
	}
	return ParseAiAvatarCatalogParentFileID(envelope)
}

// AppCloudFileList 列一个目录下的文件（只读，**这一口有实拍**）。
func (u *Upstream) AppCloudFileList(
	ctx context.Context, profile *AppProfile, timestamp int64, authorization, parentFileID string, pageSize int64,
) (CloudFileList, *core.Error) {
	request, perr := CloudFileListRequest(profile, timestamp, authorization, parentFileID, pageSize)
	if perr != nil {
		return CloudFileList{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudFileList{}, perr
	}
	return ParseCloudFileList(envelope)
}
