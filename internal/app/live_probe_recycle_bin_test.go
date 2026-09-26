package app

// 活体探针：**回收站那一页到底回什么**（R7 剩下的那一半，也是本仓第一次打这一口）。
//
// # 这一趟要答的两问（都只答这两问，不夹带）
//
//  1. **形状**：`POST /hcy/recyclebin/list` 回的信封里 `data` 到底长什么样 ——
//     条目数组挂在哪个键下、每条目有哪些键、游标在哪一格。跑这一趟之前它**零样本**：
//     那一份 App JS（`docs/capture/apiServer.js` 的 `getVirDirInfo`）里有定义、
//     **没有任何调用点、也不读任何一个字段**，旧实现与开源实现也都给不出响应形状
//     （开源那一份只有一个 `DeleteAll`）⇒ 本探针交回的**第一份读数就是这一族的形状基准**，
//     已经逐字落进 `docs/capture/app-cloud-recyclebin-samples.txt` §3，
//     上游层的响应解析器也按那份形状落好了（`internal/upstream/app_cloud_recyclebin.go`）。
//  2. **位置**：第廿六趟读到"删非空目录 ⇒ 上游认了、根目录不再列它、那个目录里也 0 项"，
//     但跑这一趟之前"内容在**回收站里**"那一句是从"两处都不再列它"**推**出来的，成文标着 `未验证`。
//     这一趟把推导换成读数：造一枚目录 + 往里放一枚文件 → 删目录 → 列回收站 →
//     看**那两枚具名东西**在不在里面。
//     ⭐ 已读到（2026-09-26 第廿七趟，五趟：账号 0 写两趟 + 只读一趟，账号 1 只读两趟）：
//     删非空目录 ⇒ 回收站里**只多一条**（那条目录），子文件**不单列**；且这一口**有秒级不回**
//     （同一趟"删 → 当场读 → 等 20s → 再读"，整名命中 `false → true`、条数 `33 → 34`）
//     ⇒ 「当场回读说没有」不能当「没进回收站」的判据。逐字读数见
//     `docs/capture/app-cloud-recyclebin-samples.txt` §5。
//
// # 为什么这一趟要**写**（写面与第廿六趟那一发完全同形，不多一分）
//
// 问的是"回收站里有没有我们刚删的那一枚"，而回收站里别人删的东西我们不认得 ⇒
// 必须先造两枚**名字归我们**的（`asign-` 前缀）再删：一枚目录 + 一枚文件 + 一枪 batchTrash。
//   - **只打一枪**：嵌套目录与"重复删同一枚"是另一趟的问题，混做一案两问都收不到答案
//     （坑表 #194 那条教训就来自上一趟）；
//   - **收尾什么都不删**：东西留在回收站。本仓没有"彻底删除"那一口（同族三口都有逐字形状、
//     都无落码），所以这一趟在用户侧的残留与生产清理链同档。
//
// # 凭据与隐私边界（这一族最严的一处：回收站里**全是用户自己删过的文件**）
//
//   - 响应**只念形状**：走 [liveProbeShapeValue]（键 + 类型 + 长度；值只有
//     [liveProbeValueKeys] 那几枚状态码/人话才念），所以条目里的 `name` 只留长度 ⇒
//     **用户文件名不进日志**；
//   - 逐条扫描时**只把带 `asign-` 前缀的那几枚的名字念出来**（我们自己造的名 ⇒ 可打印），
//     其余条目**只计数、不收集名字**；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；
//   - 翻页那枚游标的**值不打印**（它可能就是某条用户条目的标识），只念"有没有、长度多少"。
//
// # 闸门（防止一发只读探针变成全回收站扫描）
//
// 最多翻 [liveProbeBinMaxPages] 页、每趟最多扫 [liveProbeBinMaxItems] 条。两枚都是
// **闸门不是判据**：念"没找到"之前会先念"是否被闸门截断"，因为
// "翻到第三页停住了"与"回收站里没有它"是两件事。

import (
	"context"
	"encoding/json"
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// liveProbeBinPath 只为最后那句账（这一族今天发了几枪），不是判据。
	liveProbeBinPath = "/hcy/recyclebin/list"
	// liveProbeBinFolderInfix / liveProbeBinChildInfix：我方那两枚名字的中段（可打印）。
	liveProbeBinFolderInfix = "binlistdir-"
	liveProbeBinChildInfix  = "binlistkid-"

	// liveProbeBinPrevInfix：第廿六趟写探针那枚目录名字的中段。本趟把它当**对照条目**：
	// 那一族在上一趟已经读到"根目录不再列它"，如果这里也读到"它在回收站里"，
	// 那"删目录 ⇒ 进回收站"这一档就是**实测**而不是推导。
	liveProbeBinPrevInfix = "trashdir-"

	// liveProbeBinWait 是**两遍回读之间唯一**的动作：等。第廿七趟真跑读到"刚删的那一枚
	// 当场不在第一页、几分钟后它在第 0 项"，这一格把那个"几分钟"压成一趟内可复现的量。
	liveProbeBinWait = 20 * time.Second

	liveProbeBinMaxPages = 4
	liveProbeBinMaxItems = 400
)

// liveProbeBinScan 是**裸读**一份回收站响应收出来的那几格（只服务探针，不落上游层）。
//
// 为什么不按"已知键名"写：这一族的响应今天零样本，按猜的键名扫，结果只会一律"没找到"。
// 每一枚键只按**形态**归类（挂着对象数组 / 名字含 cursor / 名字含 total|count）。
type liveProbeBinScan struct {
	// arrays 记"哪一条路径下面挂着对象数组"——第一份读数最要的就是这一格
	// （`data.items`？`data.fileList`？还是包了两层？今天没人知道）。
	arrays []string
	// total 是看到过的"总数"类那一格（可能没有）。
	total string
	// cursorWords 念游标类键的形态（只带长度，不带值）。
	cursorWords []string
	// cursorValue 是下一发要用的那一枚游标值。⚠ 它**不进**任何输出，只回灌给请求。
	cursorValue string

	items int
	ours  int
	// oursDetail 是**我方**那几枚条目的逐条读数（名字 / 类型 / 大小 / 剩余天数 /
	// 入站时间 / 目录统计）。只收带 `asign-` 前缀的 ⇒ 用户条目一个字段都不进这里。
	//
	// 为什么逐条要带值：第廿六趟留下的那一问（"内容跟着一起没了"到底是"整棵子树
	// 记成一条"还是"子文件另起一条"）就落在 `trashedDirStatistics.subFileCount` 这一格上，
	// 只念形状（键 + 长度）答不了它。
	oursDetail []string
	// ourTimes 是**我方**条目的 `trashedAt`（按数组顺序）。用处：念出这一族的**排列方向** ——
	// "刚删的那一枚不在第一页"到底意味着"它还没进索引"还是"新的排在最后一页"，
	// 完全取决于方向；没有这一格，两种读法分不开（第廿七趟就是靠它收口的）。
	ourTimes []string
	// mine 是本探针在找的**名字中段** → 命中与否。跨页**累加**，不被上一页的计数覆盖。
	//
	// 为什么按中段而不是按整名：只读复跑那一趟（[TestLiveProbeRecycleBinReread]）
	// 拿不到写探针那一趟现造的名字，只能按 `binlistdir-` 这一族的中段认。
	mine map[string]bool
	// exact 是写探针**本趟现造**那两枚的整名 → 命中与否。
	//
	// 为什么中段不够（2026-09-26 第廿七趟第二发实拍到的）：同一族复跑两次，
	// 回收站里就同时有**上一趟那枚同名中段的残留**与**本趟刚删的那一枚** ——
	// 只按中段认时，当场那一遍会先命中上一趟的残留，于是"两遍都命中"这一句
	// 听起来成立、其实说的不是本趟那一枚。整名才答得了"刚删的它到底在不在"。
	exact map[string]bool
	// truncated 记"被闸门截断过"：没找到时它是唯一的免责出处。
	truncated bool
}

// liveProbeScanBinPayload 递归扫一份裸 JSON（深度上限 4）。
func liveProbeScanBinPayload(sink *liveProbeSink, value any, path string, depth int, out *liveProbeBinScan) {
	if depth > 4 {
		return
	}
	if out.items > liveProbeBinMaxItems {
		out.truncated = true
		return
	}
	switch typed := value.(type) {
	case map[string]any:
		keys := make([]string, 0, len(typed))
		for name := range typed {
			keys = append(keys, name)
		}
		sort.Strings(keys)
		for _, name := range keys {
			lower := strings.ToLower(name)
			child := path + "." + name
			// ⚠ 游标这一格**先于**类型分派来一遍：2026-09-26 第廿七趟实拍到的是
			// `data.nextPageCursor = null` —— 上一版只认字符串，于是念成"没读到名字含 cursor 的键"，
			// 而真实读数正好相反：**有这一格，且上游用它说"没有下一页"**。
			if strings.Contains(lower, "cursor") {
				out.cursorWords = append(out.cursorWords, child+"（"+liveProbeBinCursorWord(typed[name])+"）")
			}
			switch v := typed[name].(type) {
			case []any:
				out.arrays = append(out.arrays, child+"（"+strconv.Itoa(len(v))+" 项）")
				for _, entry := range v {
					out.items++
					if out.items > liveProbeBinMaxItems {
						out.truncated = true
						sink.say("  ⚠ 已扫到 %d 条 ⇒ 停下（这是**闸门**，不是'回收站里只有这些'）",
							liveProbeBinMaxItems)
						return
					}
					liveProbeScanBinEntry(sink, entry, out.items-1, out)
				}
			case string:
				if strings.Contains(lower, "cursor") && v != "" && out.cursorValue == "" {
					out.cursorValue = v // 只留第一枚非空游标，用于翻页；**值不外传**
				}
				if strings.Contains(lower, "total") || strings.Contains(lower, "count") {
					out.total = child + "=字符串 " + liveProbeClip(v, 16)
				}
			case json.Number:
				if strings.Contains(lower, "total") || strings.Contains(lower, "count") {
					out.total = child + "=" + v.String() + "（数字）"
				}
			case map[string]any:
				liveProbeScanBinPayload(sink, v, child, depth+1, out)
			}
		}
	case []any:
		for _, entry := range typed {
			out.items++
			if out.items > liveProbeBinMaxItems {
				out.truncated = true
				return
			}
			liveProbeScanBinEntry(sink, entry, out.items-1, out)
		}
	}
}

// liveProbeScanBinEntry 看**一条**条目：只认"名字"与"我方前缀"这两格。
//
// 名字的键名不确定（`name` / `fileName` 在本仓见过的两族里都出现过）⇒ 两枚都试；
// 都读不出时**不猜**，只念这一条的键集合（那一格本身就是第一次读数要的形状）。
func liveProbeScanBinEntry(sink *liveProbeSink, entry any, index int, out *liveProbeBinScan) {
	obj, ok := entry.(map[string]any)
	if !ok {
		return
	}
	for _, idKey := range []string{"fileId", "id"} {
		if v, ok := obj[idKey].(string); ok {
			sink.see(v) // 真实 fileId：登记进掩码表，只念长度
		}
	}
	name, nameKey := "", ""
	for _, candidate := range []string{"name", "fileName"} {
		if v, ok := obj[candidate].(string); ok && v != "" {
			name, nameKey = v, candidate
			break
		}
	}
	if name == "" {
		keys := make([]string, 0, len(obj))
		for k := range obj {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		sink.say("  ⚠ 有一条读不出名字（没有 name/fileName）；它的键集合：%s", strings.Join(keys, " "))
		return
	}
	if !strings.HasPrefix(name, CloudUploadNamePrefix) {
		return // 用户自己的条目：只计数，名字与字段值都不进日志
	}
	out.ours++
	out.oursDetail = append(out.oursDetail, liveProbeBinEntryWord(obj, nameKey, index))
	if when, ok := obj["trashedAt"].(string); ok {
		out.ourTimes = append(out.ourTimes, when)
	}
	if _, want := out.exact[name]; want {
		out.exact[name] = true
		sink.say("  ⭐⭐ 命中的正是**本趟刚造**的那一枚：%s",
			liveProbeBinEntryWord(obj, nameKey, index))
	}
	hit := false
	for infix := range out.mine {
		if strings.Contains(name, infix) {
			out.mine[infix] = true
			hit = true
		}
	}
	if hit {
		sink.say("  ⭐ 命中本探针在找的那一枚（中段在 %q 那一格）：%s", nameKey,
			liveProbeBinEntryWord(obj, nameKey, index))
	}
}

// liveProbeBinCursorWord 念游标那一格**在 JSON 里到底是什么形态**。
//
// 这一格是第廿七趟真跑之后补的：上一版只认字符串，于是 `nextPageCursor: null` 被念成
// "没读到名字含 cursor 的键" —— 那是**把'上游说没有下一页'念成了'这一族没有翻页键'**，
// 两种读法对"回收站到底多少条"的回答正好相反。
func liveProbeBinCursorWord(value any) string {
	switch typed := value.(type) {
	case nil:
		return "null ⇒ 上游用它说'没有下一页'"
	case string:
		if typed == "" {
			return "空串"
		}
		return "值长 " + strconv.Itoa(len(typed)) + "，值不打印"
	}
	return liveProbeBinScalarWord(value)
}

// liveProbeBinScalarWord 念一个标量（我方条目才走到这里 ⇒ 值可原样念；fileId 不在名单里）。
func liveProbeBinScalarWord(value any) string {
	switch typed := value.(type) {
	case nil:
		return "null"
	case json.Number:
		return typed.String() + "(数字)"
	case string:
		return strconv.Quote(typed)
	case bool:
		return strconv.FormatBool(typed)
	case []any:
		return "（数组 " + strconv.Itoa(len(typed)) + " 项）"
	case map[string]any:
		return "（对象）"
	}
	return strconv.Quote(fmt.Sprint(value))
}

// liveProbeBinEntryWord 把**我方**的一条回收站条目摊成一行读数。
//
// 键名单是**白名单**（不是"把所有键都念一遍"）：`fileId` 刻意不在里面（真实标识只念长度，
// 而这一行的用处是"哪一枚、什么时候进的、算几条"，不需要它的键）。
// `trashedDirStatistics` 单挑出来：R7 剩下的那一问（子文件是被并成一条还是另起一条）
// 就落在它的 `subFileCount` 上。
func liveProbeBinEntryWord(obj map[string]any, nameKey string, index int) string {
	parts := make([]string, 0, 9)
	for _, key := range []string{"name", "type", "size", "remainingTime", "trashedReason", "trashedAt"} {
		if value, ok := obj[key]; ok {
			parts = append(parts, key+"="+liveProbeBinScalarWord(value))
		}
	}
	if stats, ok := obj["trashedDirStatistics"].(map[string]any); ok {
		inner := make([]string, 0, 3)
		for _, key := range []string{"subDirCount", "subFileCount", "totalSize"} {
			if value, ok := stats[key]; ok {
				inner = append(inner, key+"="+liveProbeBinScalarWord(value))
			}
		}
		parts = append(parts, "目录统计{"+strings.Join(inner, " ")+"}")
	}
	return "第 " + strconv.Itoa(index) + " 项（名字在 " + strconv.Quote(nameKey) + " 格）" + strings.Join(parts, " ")
}

// liveProbeBinPage 打一发回收站列口（**只读**）。交回裸 payload 与"这一发成没成"。
func liveProbeBinPage(
	sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader, cursor, what string,
) (any, bool) {
	request, perr := upstream.CloudRecycleBinListRequest(uploader.Auth, uploader.DeviceID, cursor)
	if perr != nil {
		sink.say("%s：构造请求失败 %s", what, binErr(sink, perr))
		return nil, false
	}
	envelope, cerr := run.Upstream.Call(context.Background(), request)
	if cerr != nil {
		sink.say("%s：这一枪没打通：%s", what, binErr(sink, cerr))
		return nil, false
	}
	payload, oerr := envelope.EnsureOK()
	if oerr != nil {
		sink.say("%s：回非 OK：%s", what, binErr(sink, oerr))
		return nil, false
	}
	sink.say("%s：通了，data 形状 %s", what, liveProbeShapeValue("data", payload))
	return payload, true
}

// liveProbeBinSweep 从 start 游标起把这一口**翻到底**（带闸门），读数累进 scan。
//
// 为什么要有"游标起点"这个入参：翻页键今天才第一次读到（`data.nextPageCursor`，
// 实拍是 `null`），翻页那一档**尚未被断言过** —— 传非空游标这条路径今天没跑过真上游。
func liveProbeBinSweep(
	sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader, start, what string,
	scan *liveProbeBinScan,
) (int, bool) {
	payload, ok := liveProbeBinPage(sink, run, uploader, start, what+" 第 1 页")
	if !ok {
		return 0, false
	}
	liveProbeScanBinPayload(sink, payload, "data", 0, scan)
	liveProbeSayBinScan(sink, scan, what+" 第 1 页")
	pages := 1
	for scan.cursorValue != "" && pages < liveProbeBinMaxPages && !liveProbeBinAllFound(scan) {
		next, nok := liveProbeBinPage(sink, run, uploader, scan.cursorValue, what+" 续页")
		if !nok {
			break
		}
		// mine 与明细累加，页内计数（items/ours/游标）每页重数：
		// "这一页有没有"与"整趟有没有"是两格，混在一起就说不清命中在哪一页。
		pageScan := liveProbeBinScan{mine: scan.mine, exact: scan.exact}
		liveProbeScanBinPayload(sink, next, "data", 0, &pageScan)
		scan.arrays = append(scan.arrays, pageScan.arrays...)
		scan.ours += pageScan.ours
		scan.items += pageScan.items
		scan.oursDetail = append(scan.oursDetail, pageScan.oursDetail...)
		scan.ourTimes = append(scan.ourTimes, pageScan.ourTimes...)
		if pageScan.truncated {
			scan.truncated = true
		}
		pages++
		liveProbeSayBinScan(sink, &pageScan, what+" 第 "+strconv.Itoa(pages)+" 页")
		scan.cursorValue = pageScan.cursorValue
		scan.total = pageScan.total
	}
	return pages, true
}

func TestLiveProbeRecycleBinListing(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", binErr(sink, perr))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	executor := newTaskExecutor(run, opening, account)
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		t.Fatalf("装不出网盘上传器（本探针要先造两枚再删）：%s", binErr(sink, uerr))
	}

	// ⚠ 名字只在这里造一次（建、删、回读三处比的是同一个名字 —— 上一趟的教训）。
	dirName := CloudUploadNamePrefix + liveProbeBinFolderInfix + randomHex(uploader.Rng, cloudRandomNameHexLen)
	childName := CloudUploadNamePrefix + liveProbeBinChildInfix + randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"

	// ── ① 造两枚（与第廿六趟同形）──
	created, cerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, "/", dirName)
	switch {
	case cerr != nil:
		sink.say("① 造目录失败：%s ⇒ 下面全部不发（没有我方认得出的残留，列回收站只会读到用户自己的条目）",
			binErr(sink, cerr))
		recorder.sayTally()
		return
	case created.Exist:
		sink.say("① 上游说同名已存在 ⇒ 当场收手（那一枚属于用户本来就有的目录，不删）")
		recorder.sayTally()
		return
	case created.FileID == nil:
		sink.say("① 回了 OK 但没有 fileId ⇒ 没有可删的对象，这一格没拿到")
		recorder.sayTally()
		return
	}
	dirID := *created.FileID
	sink.see(dirID)
	receipt, uerr2 := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: dirID, Name: childName})
	if uerr2 != nil || receipt.FileID == "" {
		sink.say("① 往里放文件没成：%s ⇒ 只剩一枚空目录，先把它删掉收场（不留空目录给用户）",
			binErr(sink, uerr2))
		if _, derr := uploader.DeleteCloudFiles(ctx, []string{dirID}); derr != nil {
			sink.say("  收场那一枪也没打通：%s", binErr(sink, derr))
		}
		recorder.sayTally()
		return
	}
	sink.see(receipt.FileID)
	sink.say("① 造好了两枚：目录 %q（fileId 长度 %d）、里面 %q（fileId 长度 %d）",
		dirName, len(dirID), childName, len(receipt.FileID))

	// ── ② 删（只打一枪，见抬头"只打一枪"那一条）──
	sent, derr := uploader.DeleteCloudFiles(ctx, []string{dirID})
	if derr != nil || !sent {
		sink.say("② batchTrash(目录) 没成（发过=%v）：%s ⇒ 这一趟不回答'回收站里有没有'",
			sent, binErr(sink, derr))
		recorder.sayTally()
		return
	}
	sink.say("② batchTrash(目录)：上游认了（只这一枚，没有补第二枪）")

	// ── ③ 第一遍：**删完当场**列（形状 + 那两枚在不在）──
	// 找的是**中段**而不是整名：命中的是名字里含这一串的那些条目（复跑也认得出）。
	binWanted := func() *liveProbeBinScan {
		return &liveProbeBinScan{
			mine:  map[string]bool{liveProbeBinFolderInfix: false, liveProbeBinChildInfix: false},
			exact: map[string]bool{dirName: false, childName: false},
		}
	}
	scan := binWanted()
	pages, ok := liveProbeBinSweep(sink, run, uploader, "", "③ 当场这一遍", scan)
	if !ok {
		sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是这一格的读数）。它**不推翻**任何一条既有" +
			"结论：R7 那三格是从网盘侧回读来的，与这里是不是通无关。")
		recorder.sayTally()
		return
	}

	// ── ④ 第二遍：两遍之间**只等、一枪不发**（一次只动一格，这一格动的是时间）──
	sink.say("④ 等 %s，中间**一枪都不发**（要测的就是「刚删的那一枚是不是过一会儿才出现」）",
		liveProbeBinWait)
	time.Sleep(liveProbeBinWait)
	second := binWanted()
	pages2, ok2 := liveProbeBinSweep(sink, run, uploader, "", "⑤ 等过之后这一遍", second)
	if !ok2 {
		sink.say("⑤ 等过之后这一遍没读出来 ⇒ 只剩当场那一份读数")
	}

	// ── ⑥ 命中对照：这一趟真正要念的那一行 ──
	// ⚠ 判据用**整名**（exact）而不是中段：只按中段认时，上一趟同名中段的残留会先被命中，
	// "两遍都命中"那句话说的就不是本趟那一枚了（第廿七趟第二发的账本里正是这一档）。
	// 中段那一格仍然念，但它回答的是"这一族在回收站里留了几条"。
	sink.say("⑥ 命中对照（当场 / 等后，按**本趟整名**）：目录=%v / %v；文件=%v / %v"+
		"（页数 %d / %d；闸门 %d 页 / 最多 %d 条；%s）",
		scan.exact[dirName], second.exact[dirName],
		scan.exact[childName], second.exact[childName],
		pages, pages2, liveProbeBinMaxPages, liveProbeBinMaxItems,
		liveProbeTruncWord(scan.truncated || second.truncated))
	sink.say("⑥ 同族残留这一格（按**中段**，含更早趟那几枚）：目录中段=%v / %v；文件中段=%v / %v；"+
		"条数 当场 %d / 等后 %d（差 %d）",
		scan.mine[liveProbeBinFolderInfix], second.mine[liveProbeBinFolderInfix],
		scan.mine[liveProbeBinChildInfix], second.mine[liveProbeBinChildInfix],
		scan.items, second.items, second.items-scan.items)
	switch {
	case !ok2:
		sink.say("⇒ 两遍只成了一遍 ⇒ 下面这几档一律不判")
	case scan.exact[dirName] != second.exact[dirName] || scan.exact[childName] != second.exact[childName]:
		sink.say("⇒ **当场那份与等后那份不一致** ⇒ 这一口有延迟：「当场回读说没有」**不能**当成" +
			"「它没进回收站」的判据。要判「进没进」只认**等过之后**那一份。")
	case second.exact[dirName]:
		sink.say("⇒ 两遍一致且本趟那枚目录**都在** ⇒ 「删目录 ⇒ 进回收站」这一档是实测（不是推导）")
	default:
		sink.say("⇒ 两遍一致且本趟那枚**都不在** ⇒ 才轮到原来那几档读法（见下一行）")
	}
	sink.say("读数怎么用（四档）：① 目录在、文件也在 ⇒ 子文件**各自成条**；" +
		"② 目录在、文件不在且**翻页键是 null、闸门没截断** ⇒ 整棵子树记成**一条**，" +
		"子文件不单列（这一档要看上面那行「目录统计」里的 subFileCount 配合）；" +
		"③ 两遍都不在 ⇒ 这一口回的不是回收站条目或被头画像挡了，要先补人工抓包再谈响应解析器；" +
		"④ 当场不在、等后在 ⇒ 上面那行延迟档成立，本探针只认等后那一份。")

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪；写侧只有那一次 batchTrash ⇒ 残留 = 用户回收站里这 2 枚，"+
		"本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeBinPath, recorder.countFor(liveProbeBinPath))
}

// binErr 把"可能是 nil 的错误"念成一句**可进日志**的原文：先过掩码表，再把任何
// `http(s)://…` 整串换成占位。
//
// 为什么要第二道（2026-09-26 第廿六趟那笔账的延续）：预签名 `uploadUrl` 与凭据同类，
// 而**传输层错误文本自带那一串** —— `internal/io/client.go:189/279` 把 `http.Client`
// 的原文直接包进 `core.Error`，Go 那句的形状就是 `Put "https://主机/对象键?签名": dial tcp …`。
// 上一趟泄漏走的是传输层账本（已在记录器里换成占位），**错误原文是同一个破口的另一条路**：
// 掩码表只认"登记过的串"，而那一枚 URL 是上游现给的、探针从没 `see` 过它。
// 所以这里不依赖"这一趟恰好没失败"，凡这一族要打错误原文都过这一道。
//
// `derr` 在"一枪没发"那一档是 nil，而 `derr.Error()` 会在 nil 上 panic（上一趟账本里就有这一档）。
func binErr(sink *liveProbeSink, err *core.Error) string {
	if err == nil {
		return "没有错误原文（一枪没发：名单为空）"
	}
	return liveProbeHideWriteEntry(sink.mask(err.Error()))
}

// liveProbeWriteEntryPattern 认一整串 URL（到空白 / 引号 / 中文标点为止）。
var liveProbeWriteEntryPattern = regexp.MustCompile(`(?i)https?://[^\s"'）)、,，;；]+`)

// liveProbeHideWriteEntry 把文本里任何 `http(s)://…` 换成占位（**只**给探针日志用）。
func liveProbeHideWriteEntry(text string) string {
	return liveProbeWriteEntryPattern.ReplaceAllString(text, "<写入入口/主机与路径不打印>")
}

// liveProbeTruncWord 念"闸门有没有截断过"。
func liveProbeTruncWord(truncated bool) string {
	if truncated {
		return "已被闸门截断"
	}
	return "未截断"
}

// liveProbeSayBinScan 念一份扫描结果（只念形态与我方条目，用户条目不念名字）。
func liveProbeSayBinScan(sink *liveProbeSink, scan *liveProbeBinScan, pageWord string) {
	sink.say("④ %s读数：%d 条（其中我方前缀 %d 条），条目数组见于 %d 处：%s",
		pageWord, scan.items, scan.ours, len(scan.arrays), strings.Join(scan.arrays, "、"))
	if scan.total != "" {
		sink.say("④ %s总数那一格：%s", pageWord, scan.total)
	} else {
		sink.say("④ %s没有 total/count 那一格 ⇒ '回收站一共多少条'这一问这一份读数答不了", pageWord)
	}
	if len(scan.cursorWords) == 0 {
		sink.say("④ %s这一份里**没有**名字含 cursor 的键 ⇒ 这一族的翻页键不叫 cursor"+
			"（注意：这与「有这一格但是 null」是两码事）", pageWord)
	} else {
		sink.say("④ %s游标类键：%s", pageWord, strings.Join(scan.cursorWords, " "))
	}
	if len(scan.oursDetail) == 0 {
		sink.say("④ %s没有我方前缀的条目 ⇒ 逐条读数这一档为空", pageWord)
	}
	const detailLimit = 12
	for i, line := range scan.oursDetail {
		if i >= detailLimit {
			sink.say("④ %s其余 %d 条我方条目略（要全量把 detailLimit 改大再跑）",
				pageWord, len(scan.oursDetail)-detailLimit)
			break
		}
		sink.say("④ %s我方条目：%s", pageWord, line)
	}
}

// TestLiveProbeRecycleBinReread ⭐ **只读**复跑：列回收站，不再造也不删任何东西。
//
// 为什么单独一条：第廿六趟那一次写探针（[TestLiveProbeRecycleBinListing]）当场读到
// "刚删的那枚目录**不在**回收站这一页里，而更早几趟删的 `asign-trashdir-*` 都在"。
// 那一格有两种读法 —— ① 这一口有**索引延迟**（几秒内不回），② 这一口按某种顺序排、
// 新的一枚在没翻到的页里。两种都能解释同一条读数 ⇒ 只有"过一会儿**只**读一遍"能把它们分开，
// 而这一趟**一枪写都不发**，因此可以反复跑（换 `MCLOUD_LIVE_PROBE_ACCOUNT_INDEX` 也行）。
//
// 它同时在找的还包括上一趟的中段（`trashdir-`）：那一族今天**确实在**回收站里，
// 是"删目录 ⇒ 进回收站"的实测出处，本探针把它当**对照条目**用。
func TestLiveProbeRecycleBinReread(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", binErr(sink, perr))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	executor := newTaskExecutor(run, opening, account)
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		t.Fatalf("装不出网盘上传器（只需要它的 auth 与设备档案）：%s", binErr(sink, uerr))
	}

	// ── 在找的中段：本趟写探针那两枚 + 上一趟（第廿六趟）那枚目录 ──
	scan := liveProbeBinScan{mine: map[string]bool{
		liveProbeBinFolderInfix: false,
		liveProbeBinChildInfix:  false,
		liveProbeBinPrevInfix:   false,
	}}
	payload, ok := liveProbeBinPage(sink, run, uploader, "", "① 列回收站（只读复跑，第一页）")
	if !ok {
		sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是读数）。")
		recorder.sayTally()
		return
	}
	liveProbeScanBinPayload(sink, payload, "data", 0, &scan)
	liveProbeSayBinScan(sink, &scan, "第 1 页")
	pages := 1
	for scan.cursorValue != "" && pages < liveProbeBinMaxPages {
		next, nok := liveProbeBinPage(sink, run, uploader, scan.cursorValue, "② 续页")
		if !nok {
			break
		}
		pageScan := liveProbeBinScan{mine: scan.mine}
		liveProbeScanBinPayload(sink, next, "data", 0, &pageScan)
		scan.arrays = append(scan.arrays, pageScan.arrays...)
		scan.ours += pageScan.ours
		scan.items += pageScan.items
		scan.oursDetail = append(scan.oursDetail, pageScan.oursDetail...)
		scan.ourTimes = append(scan.ourTimes, pageScan.ourTimes...)
		if pageScan.truncated {
			scan.truncated = true
		}
		pages++
		scan.cursorValue = pageScan.cursorValue
	}

	// ── 排列方向：拿**我方**条目首末的 `trashedAt` 念一遍 ──
	if len(scan.ourTimes) >= 2 {
		direction := "第 0 项**不早于**末项 ⇒ 看起来是**新的在前**"
		if scan.ourTimes[0] < scan.ourTimes[len(scan.ourTimes)-1] {
			direction = "第 0 项**早于**末项 ⇒ 看起来是**旧的在前**（那「刚删的不在第一页」就可能只是在最后一页）"
		}
		sink.say("③ 顺序读数（我方条目共 %d 条）：%s；首 %q 末 %q",
			len(scan.ourTimes), direction, scan.ourTimes[0], scan.ourTimes[len(scan.ourTimes)-1])
	} else {
		sink.say("③ 我方条目不足两条 ⇒ 排列方向这一格没拿到（需要多趟残留才有对照）")
	}
	infixes := make([]string, 0, len(scan.mine))
	for infix := range scan.mine {
		infixes = append(infixes, infix)
	}
	sort.Strings(infixes) // 输出要可逐字比对（map 随机序会让两趟的日志看着不一样）
	for _, infix := range infixes {
		sink.say("④ 中段 %q 在回收站里：%v（翻了 %d 页；闸门 %d 页 / 最多 %d 条；%s）",
			infix, scan.mine[infix], pages, liveProbeBinMaxPages, liveProbeBinMaxItems,
			liveProbeTruncWord(scan.truncated))
	}
	sink.say("⑤ 这一趟**只发只读那一口**（下面账本里除它以外应当一枪都没有）⇒ 残留没有增加，可反复跑")
	recorder.sayTally()
	if n := recorder.countFor(liveProbeBinPath); n == 0 {
		sink.say("⚠ 账本里连 `%s` 都没有 ⇒ 上面那些读数不是这一口给的，别用", liveProbeBinPath)
	}
}

// liveProbeBinAllFound 问"在找的那些都齐了吗"（齐了就不必再翻页）。中段与整名**都要齐**。
func liveProbeBinAllFound(scan *liveProbeBinScan) bool {
	found := false
	for _, set := range []map[string]bool{scan.mine, scan.exact} {
		for _, hit := range set {
			found = true
			if !hit {
				return false
			}
		}
	}
	return found
}

// TestLiveProbeScanBinEntryMatchesExactNotInfix 钉"整名与中段是两格"，顺带钉隐私那一格。
//
// 反向改坏它的样子：把整名判定删掉（只留中段）⇒ 第一段断言红；
// 把"非我方前缀直接 return"改成也收进 oursDetail ⇒ 最后两条红
// （那两条要守的是**用户文件名一个字节都不进日志**）。
func TestLiveProbeScanBinEntryMatchesExactNotInfix(t *testing.T) {
	sink := newLiveProbeSink(t)
	const myNew = CloudUploadNamePrefix + "binlistdir-aaaaaaaa"
	scan := liveProbeBinScan{
		mine:  map[string]bool{liveProbeBinFolderInfix: false},
		exact: map[string]bool{myNew: false},
	}
	// ① 上一趟同名中段的残留：中段命中、整名**不**命中。
	liveProbeScanBinEntry(sink, map[string]any{
		"name": CloudUploadNamePrefix + "binlistdir-bbbbbbbb", "type": "folder",
	}, 0, &scan)
	if !scan.mine[liveProbeBinFolderInfix] {
		t.Error("同名中段的残留应把中段那一格置真")
	}
	if scan.exact[myNew] {
		t.Error("残留被当成「本趟那一枚」了 ⇒ 整名与中段混成一格")
	}
	// ② 本趟那一枚：整名命中。
	liveProbeScanBinEntry(sink, map[string]any{"name": myNew, "type": "folder"}, 1, &scan)
	if !scan.exact[myNew] {
		t.Error("本趟整名没被命中")
	}
	// ③ 用户自己的条目：不计数、不收明细、名字不出现在任何一行输出里。
	const userEntry = "用户的私密合同.pdf"
	before := len(sink.lines)
	liveProbeScanBinEntry(sink, map[string]any{"name": userEntry, "type": "file"}, 2, &scan)
	if scan.ours != 2 {
		t.Errorf("我方计数被用户条目带大了：实得 %d", scan.ours)
	}
	if len(scan.oursDetail) != 2 {
		t.Errorf("明细里混进了用户条目：实得 %d 行", len(scan.oursDetail))
	}
	for _, line := range append(append([]string{}, sink.lines[before:]...), scan.oursDetail...) {
		if strings.Contains(line, userEntry) {
			t.Errorf("用户文件名进了输出：%s", line)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 日志边界（**不是**活体探针，默认档就跑）
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveProbeHideWriteEntryKeepsPresignedURLOutOfOutput ⭐ 错误原文里那条**预签名写入
// 入口**不得进日志 —— 第廿六趟的泄漏走的是传输层账本（修法见
// `docs/capture/app-cloud-folder-trash-samples.txt` §6），这一条钉的是**同一个破口的另一条路**：
// `internal/io/client.go:189/279` 把 `http.Client` 的原文包进 `core.Error`，而 Go 那句
// 长得就是 `Put "<整条带签名的 URL>": dial tcp …`。
//
// 反向改坏它的样子：把 [liveProbeHideWriteEntry] 改成 `return text` ⇒ 前两条断言红；
// 把正则放宽到"整行替换" ⇒ "只该换掉 URL"那一条红（读数不能一起丢了）。
func TestLiveProbeHideWriteEntryKeepsPresignedURLOutOfOutput(t *testing.T) {
	const synthesized = `上传失败: Put "https://oss-synth.example.com/2000000070/asign-synth.png` +
		`?X-Amz-Signature=deadbeefcafe": dial tcp 192.0.2.1:443: connect: connection refused`
	got := liveProbeHideWriteEntry(synthesized)
	for _, wantGone := range []string{"oss-synth.example.com", "asign-synth.png", "deadbeefcafe"} {
		if strings.Contains(got, wantGone) {
			t.Errorf("%q 原样进了日志：%s", wantGone, got)
		}
	}
	if !strings.Contains(got, "dial tcp") || !strings.Contains(got, "connection refused") {
		t.Errorf("把整句都吞了（该只换掉那一串 URL）：%s", got)
	}
	if !strings.Contains(got, "上传失败") {
		t.Errorf("错误前缀也没了：%s", got)
	}
}

// TestBinErrIsNilSafe 钉"一枪没发"那一档：`derr` 为 nil 时念的是一句人话，而不是 panic。
func TestBinErrIsNilSafe(t *testing.T) {
	sink := newLiveProbeSink(t)
	word := binErr(sink, nil)
	if word == "" || !strings.Contains(word, "一枪没发") {
		t.Errorf("实得 %q", word)
	}
	// 另一档：有错误时掩码表要生效（凭据串不得原样出来）。
	const secret = "SYNTH-AUTH-TOKEN-VALUE"
	sink.see(secret)
	wrapped := core.NewNetwork("发送失败: " + secret)
	if got := binErr(sink, wrapped); strings.Contains(got, secret) {
		t.Errorf("凭据没被掩掉：%s", got)
	}
}
