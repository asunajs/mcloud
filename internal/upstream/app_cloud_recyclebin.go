package upstream

// 回收站那一族的**只读**口：`POST /hcy/recyclebin/list`。
//
// # 这一口今天为什么值得落下来
//
// 网盘清理链尾那一枪（[CloudBatchTrashRequest]）在生产里已经打过几十次，R7 也已在
// 2026-09-26 闭合（"非空目录一次枪就连内容一起收走"，见 `docs/capture/app-cloud-folder-trash-samples.txt`）。
// 但那一族的**判读**里留了一格只能靠这一口答：
//
//   - "内容跟着目录一起没了"是**两处回读都不再列它**推出来的（根目录那一页 + 那个被删的目录），
//     而"它在回收站里"当时是一句**推断** —— 那一格已由 2026-09-26 第廿七趟换成读数：
//     删一枚非空目录 ⇒ 回收站里**只多一条**（那条目录），里面的文件**不单列**。
//   - `docs/pending-decisions.md` R37ⓒ 那一档量到"两号合计 67 枚我方前缀残留"，用的是**网盘侧**
//     的目录树走查；同一批东西有没有一部分**其实在回收站里**，也要靠这一口才分得开。
//
// 所以这一口的价值不在"补一个功能"，在于**把推断换成读数**。它只读，无副作用。
// 上面第一格**已经**读完（R7 收口，逐字读数见下面那份 capture 样本）；第二格（R37ⓒ 那批
// 残留里有多少其实躺在回收站）**仍零读数** —— 探针只认 `asign-*`，而那一格要的是**全量计数**，
// 今天这一口不回总数、翻页又有闸门 ⇒ 要做得另开一趟。
//
// # 证据等级（这一族的四口一起说清楚，别把"形状有出处"读成"真打过了"）
//
//   - **请求形状**：逐字来自 App 端 JS 包（[`../../docs/capture/apiServer.js`](../../docs/capture/apiServer.js)
//     的 `getVirDirInfo`，那里它就叫"回收站相关接口"）：`POST /hcy/recyclebin/list`，
//     体只有一枚键 `pageInfo:{pageCursor,pageSize}`，`pageSize` 在那个包里**写死 100**
//     （原文还留着 `//item.pageSize`，说明页长本来可调，App 选了固定值）。
//   - **响应形状**：**已有两份实测**（2026-09-26 第廿七趟两个号各真打过；同日更晚第廿八趟
//     在账号 0 上又跑三趟，专门读 folder 型那三格。逐字读数见
//     [`docs/capture/app-cloud-recyclebin-samples.txt`](../../docs/capture/app-cloud-recyclebin-samples.txt)
//     §3／§4／§11）。
//     形状是 `data:{items:[…], nextPageCursor:…}`，条目里 folder 型与 file 型**不同形**
//     （`trashedDirStatistics` 与 `thumbnailUrls` 各自可能为 `null`）⇒ 解析器只读
//     **两型都有且实拍稳定**的那几格，其余一律不解释（见 [CloudRecycleBinEntry] 那条字段级说明）。
//     ⚠ 这份读数是**本探针自己造的对象**给的：条目里的 `name` 全是 `asign-*` 与我方历史残留，
//     "回收站里真放着用户内容时这一口回什么"只在一个号上见过 4 条（名字不进成文，形状同档）。
//   - **同一族另三口**（`batchRestore` 还原、`file/batchDelete` 彻底删除、`recyclebin/clear` 清空）
//     在那份 JS 里同样有逐字请求形状，本仓**一口都没落**：还原与彻底删除都是**不可回滚的写**
//     且今天没有消费者（清理链的终态就是"进回收站"，见 `internal/app/cloud_upload.go` 文件头）。
//     ⚠ 这一句不是"以后也不该落"，是"要落先按 AGENTS.md §1.11 把它要动的东西说清楚"。
//   - **鉴权画像**：沿用的是同族 [CloudBatchTrashRequest] 那一档（web 渠道 + `op*100`，
//     旧 TS `apis/file.ts:114` 写死的画像）。**这一档今天有读数了**：2026-09-26 两个号五趟
//     全部回 `code:"0000"`，没有一档被头画像挡下 ⇒ "list 认这一档"成立。
//     ⚠ 它**不**覆盖同族另三口，也**不**说明别的渠道档认不认（只试过这一档）。
//   - **这三型结构体一律不带 `json` tag**（`CloudRecycleBinEntry` / `CloudRecycleBinDirStats` /
//     `CloudRecycleBinPage`，已进 jsontags 白名单）：本层是**手工从 `map[string]any` 读**的
//     （`optStrField` / `asInt64`），从不 marshal / unmarshal 它们 ⇒ 补 tag 不会让解析多认一格，
//     只会把"这些字段名是对外契约"这件事**假成文**（AGENTS §1.4：JSON 字段名是冻结基准，
//     与黄金向量比对过才算数；本族的出处是探针 stdout，不是对拍面）。同族先例见
//     `CloudFileCreated` —— 它也没有 tag，理由那一格更硬（`UploadURL` 是预签名入口）。

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// CloudRecycleBinPageSize 是 App 那一页写死的页长（`apiServer.js` 里 `pageSize:100`，
// 旁边留着 `//item.pageSize` ⇒ 页长本来可调，App 选了固定值）。
//
// ⚠ 与 [CloudFileListRequest] 那一份分页**不是一件事**：那一口的游标在响应里另有键名，
// 这一口实拍是 `data.nextPageCursor`（第廿七趟，两个号：一页回完时它是 `null`，
// 没回完时是一枚长度 104 的串）。页长这一格**只在一个号上验证过 100 这一档生效**
// （另一个号当场回满 100 条并给出游标）⇒ "换个别的面值上游认不认"**未测试**，
// 所以这里跟着 App 那份写死，不给调用方开口子。
const CloudRecycleBinPageSize = 100

// CloudRecycleBinListRequest 列**回收站**的一页（**只读**，无副作用）。
//
// 线格式按上面那节说的证据等级钉：体里只有 `pageInfo`，其下只有 `pageCursor` 与 `pageSize`
// 两枚（用例 `TestCloudRecycleBinListRequestIsAppJSLiteralShape` 钉这一格）。
//
// `pageCursor` 给空串是**第一页**那一档（App 那句 `item.nextPageCursor || ""` ⇒ 空游标不是
// "省略这一枚键"，而是**带着空串发**）。⚠ "省略这一枚键会怎样"零样本，不要拿这个函数去试：
// 这里两种情况发的是同一份形状（空串），因为那才是出处里第一页的样子。
func CloudRecycleBinListRequest(
	authorization, deviceID, pageCursor string,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"pageInfo": map[string]any{
			"pageCursor": pageCursor,
			"pageSize":   CloudRecycleBinPageSize,
		},
	}
	request, err := core.NewRequestPost(BasePersonalKD + "/hcy/recyclebin/list").WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return CloudFileOpHeaders(request, authorization, deviceID, CloudChannelWeb, CloudOpUpload), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应侧：一页回收站条目
// ─────────────────────────────────────────────────────────────────────────────

// CloudRecycleBinEntry 是回收站里的**一条**。
//
// # 为什么只读这几格（字段级证据，逐格对上第廿七趟实拍）
//
// 实拍条目共有 14 枚键（`category createdAt fileId mediaMetaInfo name remainingTime size
// thumbnailUrls trashedAt trashedDirStatistics trashedReason type updatedAt userTags`），
// 这里只留**判读用得上的**五格，其余一律不落地：
//
//   - `mediaMetaInfo` / `userTags` / `category` / `updatedAt`：**不解释**。实拍里
//     `userTags` 是 5 项 `{key,value}`、`category` 是长度 5~6 的串，本仓没有任何成文说它们
//     是什么 ⇒ 落成字段就是把我的猜测固化成接口（AGENTS §1.7 两个方向都不许断言）。
//   - `thumbnailUrls`：file 型是**数组**（实拍一项里含长度 644 的 url）、folder 型是 `null`。
//     那一串 url 是**带签名的读取入口** ⇒ 与预签名 `uploadUrl` 同档，**不得**进日志与错误文本，
//     所以本层干脆不读它。
//   - `trashedDirStatistics`：folder 型是对象、file 型是 `null` ⇒ 交回 [CloudRecycleBinEntry.DirStats]
//     指针（缺席/null 都是 nil），不猜"0 项目录"与"这是文件"是同一件事。
//   - `remainingTime` ⭐ 实拍见过 `14` 与 `0` 两档（后者是第二页尾部那批**更早**的条目）⇒
//     "是不是过期了 / 还剩几天"这一问**没有成文判据**，本层只把数字交回，不解释语义。
//
// ⚠ 这一族的 `fileId` 与网盘侧其他口同形（长度 33 的串），但**能否**拿去
// `batchRestore` / `file/batchDelete` 零读数（那两口本仓没落码）。
type CloudRecycleBinEntry struct {
	FileID        *string
	Name          *string
	Type          *string
	Size          *int64
	TrashedAt     *string
	RemainingTime *int64
	// DirStats 只在 folder 型那一条上有；file 型实拍是 `null`。
	DirStats *CloudRecycleBinDirStats
}

// CloudRecycleBinDirStats 是 folder 型条目上的 `trashedDirStatistics` 那三格。
//
// ⚠ **它不是"这个目录里现在有什么"的读数，也不是一个"由形状唯一决定"的数**：
// 第廿八趟拿**同一棵两层子树**（子树 = 1 枚子目录 + 2 枚文件，直接子级 = 1 枚子目录 + 1 枚文件）
// 连打三趟，`batchTrash(最外层)` 之后在回收站里读到的是**两档**：
//
//   - `1/0/0`（两趟，且那两枚在十几分钟后的另一趟里回读**仍是** `1/0/0`）；
//   - `1/1/64`（一趟，数值恰好等于**直接子级**：1 枚目录 + 1 枚文件 + 64 字节；整棵子树应是 `1/2/128`）。
//
// 而第廿七趟那档"子树只有一枚文件"的目录读到的是 `0/0/0` ⇒ 三条读法各自的分界写在这里：
// 目录那一格会跟着子树里的目录动（`0/0/0` vs `1/x/x`），文件那两格**填不填不确定**；
// "晚填"这一解释对上面那两枚已被隔了十几分钟的回头读否掉，但**不排除**更长（小时 / 天）才补，
// 也仍未解释第三枚为什么当趟就填上了。逐字读数见
// [`docs/capture/app-cloud-recyclebin-samples.txt`](../../docs/capture/app-cloud-recyclebin-samples.txt) §11。
//
// ⇒ 落码取向：三格**只交回数字**（指针：键缺席 / 不是数字都是 `nil`，与"读到 0"分开，#197 同族），
// 不做任何"用 `totalSize` 当目录大小 / 用 `subFileCount` 数内容"的判断。⚠ 同一趟里条目自身的
// `size` 与 `totalSize` 逐枚相等（0/0、0/0、64/64）—— 那**只是三次读数一致**，不是上游保证，
// 所以也不拿 `size` 去兜这一格。要真分口径，得另造"直接 1 枚目录 / 子树 2 枚目录"的三层形状
// （今天这两档在 `subDirCount` 上同值，分不开）。
type CloudRecycleBinDirStats struct {
	SubDirCount  *int64
	SubFileCount *int64
	TotalSize    *int64
}

// CloudRecycleBinPage 是一页回收站读数。
type CloudRecycleBinPage struct {
	// Items 是这一页的条目，**顺序原样保留**（实拍是 `trashedAt` 倒序：新的在第一页前面。
	// ⚠ "上游保证倒序"没有成文，这里只是复述实拍，排序不要依赖它）。
	Items []CloudRecycleBinEntry
	// NextPageCursor 是**下一发的 `pageCursor`**；空串表示没有下一页。
	NextPageCursor string
	// HasCursorField 记 `data` 里到底**有没有** `nextPageCursor` 这一枚键。
	//
	// 为什么要单独一格：实拍两档都见过这一枚键，一值是 `null`（= 没有下一页）、
	// 一值是长串（= 还有下一页）。"键缺席"是**第三种**情况（形状变了），
	// 把它和 `null` 都读成"没有下一页"会让一次上游改版静默变成"回收站只有一页"。
	HasCursorField bool
}

// ParseCloudRecycleBinPage 解 [CloudRecycleBinListRequest] 那一枪的回执。
//
// 信封与同族 [ParseCloudFileCreate] 一致（载荷在 `data`，`code` 是字符串 `"0000"`）。
// `data` 缺失 / 不是对象 ⇒ 解析失败，**不**兜成"空的一页"：那会把"这一口改版了"
// 念成"回收站是空的"，而后者听起来完全正常（#190 那句"未知变成空话"同一族）。
// `data.items` 缺席则按空页处理（实拍两号都带这一枚键；这一档**未被断言为上游行为**，
// 只保证本层不 panic）。
func ParseCloudRecycleBinPage(envelope *Envelope) (CloudRecycleBinPage, *core.Error) {
	payload, perr := envelope.EnsureOK()
	if perr != nil {
		return CloudRecycleBinPage{}, perr
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return CloudRecycleBinPage{}, core.NewParse(fmt.Sprintf(
			"recyclebin/list 的 data 不是对象，实得 %T（载荷在 data，与 rabactive 那族不同）", payload))
	}
	out := CloudRecycleBinPage{}
	raw, hasList := obj["items"].([]any)
	if hasList {
		out.Items = make([]CloudRecycleBinEntry, 0, len(raw))
		for _, item := range raw {
			entry, isObj := item.(map[string]any)
			if !isObj {
				continue // 非对象的一项：跳过而不判死（这一档零样本，见头注）
			}
			out.Items = append(out.Items, cloudRecycleBinEntry(entry))
		}
	}
	cursor, hasCursor := obj["nextPageCursor"]
	out.HasCursorField = hasCursor
	if hasCursor {
		if text, isStr := cursor.(string); isStr {
			out.NextPageCursor = text
		}
		// `null`（实拍那一档）与数字之类：一律留空串 = "没有下一页"，
		// 而"键在不在"由 [CloudRecycleBinPage.HasCursorField] 单独答。
	}
	return out, nil
}

func cloudRecycleBinEntry(obj map[string]any) CloudRecycleBinEntry {
	entry := CloudRecycleBinEntry{
		FileID:    optStrField(obj, "fileId"),
		Name:      optStrField(obj, "name"),
		Type:      optStrField(obj, "type"),
		TrashedAt: optStrField(obj, "trashedAt"),
	}
	if value, ok := asInt64(obj["size"]); ok {
		size := value
		entry.Size = &size
	}
	if value, ok := asInt64(obj["remainingTime"]); ok {
		remaining := value
		entry.RemainingTime = &remaining
	}
	if stats, isObj := obj["trashedDirStatistics"].(map[string]any); isObj {
		parsed := CloudRecycleBinDirStats{}
		for key, target := range map[string]**int64{
			"subDirCount":  &parsed.SubDirCount,
			"subFileCount": &parsed.SubFileCount,
			"totalSize":    &parsed.TotalSize,
		} {
			if value, ok := asInt64(stats[key]); ok {
				clone := value
				*target = &clone
			}
		}
		entry.DirStats = &parsed
	}
	return entry
}

// AppCloudRecycleBinList 列回收站的一页（**只读**，无副作用）。
//
// `pageCursor` 给空串 = 第一页（见 [CloudRecycleBinListRequest] 那一处：App 发的是
// `item.nextPageCursor || ""` ⇒ 空游标**带着空串**发，不是省略键）。
//
// ⚠ **今天的消费者只有探针**：本仓生产链一处都不列回收站（清理链的终态就是"进回收站"，
// 见 `internal/app/cloud_upload.go` 文件头）。这一口的用处是**判读**（R7 剩下的那一格、
// R37ⓒ 那批残留到底在不在回收站），不是功能。别把它接进链里当"清理的一部分"。
func (u *Upstream) AppCloudRecycleBinList(
	ctx context.Context, authorization, deviceID, pageCursor string,
) (CloudRecycleBinPage, *core.Error) {
	request, perr := CloudRecycleBinListRequest(authorization, deviceID, pageCursor)
	if perr != nil {
		return CloudRecycleBinPage{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudRecycleBinPage{}, perr
	}
	return ParseCloudRecycleBinPage(envelope)
}
