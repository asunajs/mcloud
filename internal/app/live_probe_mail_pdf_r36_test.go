package app

// 活体探针：玩法 1014 的**转换产物**到底落没落在网盘里（R36 的那一问）。
//
// # 为什么现在才问得出这一格
//
// R36 从登记那天起就卡在同一处：⑥（`disk:convertPdfFromYunFile`）**已被上游认**
// （`code:"S_OK"`，2026-09-26 三次同型），但"转出来的那一份在哪、是不是另存"零样本，
// 而唯一能自查的只读口就是 `hcy/file/list` —— 它当时回 `00010003`（**R16**）。
// 2026-09-26 那一格收口（卡的只是 `x-yun-client-info` 那一串**值**）⇒ 回读读得出条目了，
// 剩下的动作就是把"**转换前 / 转换后**"两遍读数放进**同一趟**里比。本文件就是那一趟。
//
// # 三遍读数各答哪一问
//
//   - **转换前**：根目录这一页有多少项、其中我们那族前缀（[CloudUploadNamePrefix]）几个 ⇒ 基准；
//   - **转换后、清理前**：多出来哪几个 —— 这是本探针的**主判据**：
//     "另存一份"表现为多出一枚（多半与素材同名或同干、后缀换成 `.pdf`），
//     "原地换内容"表现为**一枚都不多**（只有我们上传的那一枚）；
//   - **清理后**：链尾那次 `batchTrash` 只覆盖 `executor.cloudFileIDs`（= 素材那一枚），
//     所以这一遍问的是"**转换产物会不会跟着素材一起被删掉**"。
//
// 三遍都是**同一枚游标口径**：只列根目录**一页**（`pageSize` 与生产同档）。
//
// # 四条边界（免得下一轮把这一趟读成它没说的事）
//
//   - **根目录没有 ≠ 没落盘**：转换产物若落在别的目录（邮箱附件目录、`/synbox` 子目录），
//     这一页看不见 ⇒ 那一格仍是问句，本探针只能说"根目录这一页里没有"；
//   - **一页 100 个装不下**时后面那些看不到 ⇒ 每一遍都念 `nextPageCursor` 有没有，
//     基准那一遍就**不是**"整个网盘"的计数；
//   - **多出来的不一定是我们造的**：账号若同时在别处上传，差集里会有别人的文件
//     ⇒ 所以差集里**只有前缀是 `asign-` 的那几枚念名字**（那本来就是我们造的），
//     其余只念**扩展名与长度**，用户自己的文件名不进日志（与"真实 fileId 不进成文"同一纪律）；
//   - 这一族**是写**（往用户网盘传一份 PDF 素材并请求转换）⇒ 用户口径明确"包括上传网盘文件"
//     都要实测（2026-09-25），所以不设额外开关，但那四个 env 齐了才动（默认 `t.Skipf`）。

import (
	"context"
	"encoding/json"
	"fmt"
	"path"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

// liveProbeFileListPageSize 与生产路径同档（旧 TS 固定 100）：探针要比的是"同一口径的两遍"。
const liveProbeFileListPageSize = 100

// liveProbeListing 读根目录一页，并把"这一页读没读出来"分开：读不出来时**不返回空集**冒充"网盘是空的"。
//
// 返回值第二个是"这一页有没有下一页"—— 有则下面所有差集都只在**这一页内**成立。
func liveProbeListing(
	what string, run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink,
) (map[string]string, bool, bool) {
	page, lerr := run.Upstream.AppCloudFileList(context.Background(), opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(), "/",
		liveProbeFileListPageSize)
	if lerr != nil {
		sink.say("%s：根目录这一页**没读到**（%s）⇒ 后面所有差集都不成立",
			what, sink.mask(lerr.Error()))
		return nil, false, false
	}
	names := make(map[string]string, len(page.Items))
	for _, file := range page.Items {
		sink.see(file.FileID) // 真实 fileId 不进成文
		names[file.Name] = file.FileID
	}
	hasMore := page.NextPageCursor != nil && *page.NextPageCursor != ""
	ours := 0
	for name := range names {
		if strings.HasPrefix(name, CloudUploadNamePrefix) {
			ours++
		}
	}
	sink.say("%s：根目录这一页 %d 项（其中前缀 %q 的 %d 枚），下一页游标=%s",
		what, len(names), CloudUploadNamePrefix, ours, liveProbeCursorWord(hasMore))
	return names, true, hasMore
}

func liveProbeCursorWord(hasMore bool) string {
	if hasMore {
		return "有（⇒ 这一页之外的不在本探针的对照面里）"
	}
	return "缺"
}

// liveProbeSayAdded 念"后一遍比前一遍多出来的那几枚"。
//
// 名字要不要念，按**是谁造的**分档：前缀是我们那族的才念全名（那是我们自己造的文件，
// 它出现在差集里就是判据本身）；其余只念扩展名与长度 —— 用户自己的文件名不进日志。
func liveProbeSayAdded(when string, before, after map[string]string, sink *liveProbeSink) {
	if before == nil || after == nil {
		sink.say("%s：**没有可比的前一遍**（上面那行说了为什么）⇒ 这一格仍是空白", when)
		return
	}
	var added []string
	for name := range after {
		if _, ok := before[name]; !ok {
			added = append(added, name)
		}
	}
	removed := 0
	for name := range before {
		if _, ok := after[name]; !ok {
			removed++
		}
	}
	sort.Strings(added)
	sink.say("%s：净差 = 多 %d 枚 / 少 %d 枚", when, len(added), removed)
	for _, name := range added {
		if strings.HasPrefix(name, CloudUploadNamePrefix) {
			sink.say("%s 多出来（**我方造的那一枚**）：%s", when, name)
			continue
		}
		sink.say("%s 多出来（不是我方前缀 ⇒ 名字不念）：扩展名 %q，名字长度 %d",
			when, strings.ToLower(path.Ext(name)), len([]rune(name)))
	}
	// 少了的那几枚**连名字都不收集**：它们多半是用户自己的文件被别处移走，与本探针无关，
	// 搬进日志只是把用户内容多抄一份。所以上面只数个数。
}

// TestLiveProbeMailPDFResidue 就是"同一趟里比转换前后两遍"那一发。
//
// 它刻意**只**跑 ⑥ 那一条（不打 ⑧ 备份、也不打 ③④ 那几枪）：
// 上一轮的教训是"一次动两格 ⇒ 两问都收不到答案"，而这里两遍回读之间只允许
// 有"传素材 + 转换"这两件事（它们本来就是同一格）。
func TestLiveProbeMailPDFResidue(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())
	executor := newTaskExecutor(run, opening, account)

	// ── 第一遍：转换**前**的根目录 ──
	before, beforeOK, _ := liveProbeListing("转换前", run, opening, sink)

	// ── 走生产那一层：传素材 + ⑥ 转换（**写**）──
	pdfItem := liveProbeTaskItem(1014, "PDF 转换")
	fired, problems := executor.runMailPDF(ctx, pdfItem, &TaskStepRecord{TaskID: 1014})
	sink.say("⑥ 走完整条玩法（1014）：发过枪=%v 失败 %d 条", fired, len(problems))
	for _, problem := range problems {
		sink.say("⑥ 原因：%s", sink.mask(problem))
	}
	if !fired || len(problems) > 0 {
		// ⑥ 没做成时后两遍读数只说明"传了一份素材上去"，说明不了转换 ⇒ 当场停，
		// 别把"根目录没多东西"读成"转换不落盘"。
		sink.say("⑥ 这一趟**没做成** ⇒ 后面两遍回读只到'素材有没有留下'，**不**回答转换产物（停在这里）")
	}

	// ── 第二遍：转换**后**、清理**前** —— 本探针的主判据 ──
	after, afterOK, afterMore := liveProbeListing("转换后（清理前）", run, opening, sink)
	liveProbeSayAdded("转换后 vs 转换前", before, after, sink)
	if !beforeOK || !afterOK {
		sink.say("⇒ 两遍里有一遍没读出来 ⇒ R36 那一格**仍空**，这一趟只算" +
			"确认「回读这一枪今天能不能打」")
	}

	// ── 链尾清理：只覆盖 executor.cloudFileIDs（= 素材那一枚）──
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		sink.say("清理没做：拿不到上传器（%s）⇒ 第三遍只是重复第二遍", sink.mask(uerr.Error()))
	} else {
		for _, fileID := range executor.cloudFileIDs {
			sink.see(fileID)
		}
		sent, derr := uploader.DeleteCloudFiles(ctx, executor.cloudFileIDs)
		switch {
		case derr != nil:
			sink.say("链尾清理失败（发过枪=%v）：%s", sent, sink.mask(derr.Error()))
		case sent:
			sink.say("链尾清理：一次 batchTrash，覆盖这一趟记下的 %d 枚 fileId（值不打印）",
				len(executor.cloudFileIDs))
		default:
			sink.say("链尾清理：没有要删的（这一趟没拿到 fileId）")
		}
	}

	// ── 第三遍：清理**后** —— 问"转换产物会不会跟着素材一起被删" ──
	cleaned, cleanedOK, _ := liveProbeListing("清理后", run, opening, sink)
	liveProbeSayAdded("清理后 vs 转换后", after, cleaned, sink)
	if beforeOK && afterOK && cleanedOK && !afterMore {
		// ⚠ 这一句是**判读口径**，不是结论；措辞要窄到"根目录这一页"，因为
		// "没有第二枚"与"产物不存在"隔着一整个目录树 + 一次异步（详见下面那发只读走目录探针）。
		sink.say("读数怎么用（三档，逐档对上本趟的差集）：" +
			"①「前两遍之间只多一枚我方前缀、清理后回到基准」⇒ 只说明**根目录这一页里没有第二份**，" +
			"它不排除产物落在别的目录、也不排除'异步、回读太早'（那一格由走目录那一发继续问）；" +
			"②「两遍都不多」⇒ 素材本身没在根目录出现，这一趟的差集不可比（要看上传那一行的播报）；" +
			"③「多出一枚不是我方前缀、且清理后仍在」⇒ **另存一份且不在我们的清理链上**，那才要接清理。")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 只读那一发：产物在**整棵目录树**里的哪一格（R36 剩下的那一半）
// ─────────────────────────────────────────────────────────────────────────────

// 上限两枚都是"别让一发只读探针变成全盘扫描"的闸门，不是判据。
const (
	// liveProbeFolderTypeWord 是**上一发读出来的**那一档（本发只拿它记账，不当判据）：
	// 2026-09-26 实测 `data.items[].type` 是小写串 `file` / `folder`。
	// ⚠ 它**不进**生产解析器：本仓今天没有任何一处按 `type` 分流（要加分流先按 §1.5 登记）。
	liveProbeFolderTypeWord = "folder"

	liveProbeWalkMaxDepth = 3
	liveProbeWalkMaxDirs  = 200
)

// liveProbeRawItem 是 `data.items[]` 里**本层不读的那几枚**：`type` 与 `createdAt`。
//
// 为什么在探针里裸读一份：生产解析器 [upstream.ParseCloudFileList] 只取 `fileId` 与
// `name`（成文：`type` / `size` / `trashedAt` 三枚本层不读），而"这一枚是不是目录"
// "这一份是什么时候生成的"正是 R36 剩下的那一问要的东西。裸读只服务探针 ⇒
// **不许**据此往生产层加字段（要加先按 §1.5 过 json tag 门禁并登记判据）。
type liveProbeRawItem struct {
	fileID   string
	name     string
	typeWord string // `type` 那一格的**字面**（实拍是字符串，不是数字 ⇒ 见 liveProbeScalarWord）
	created  string // `createdAt` 那一格的字面（实拍是日期串，不是毫秒数）
}

// liveProbeRawListing 列一个目录**一页**，交回裸读数与"这一发成没成"。
func liveProbeRawListing(
	run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink, parentFileID string,
) ([]liveProbeRawItem, bool) {
	request, perr := upstream.CloudFileListRequest(opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(),
		parentFileID, liveProbeFileListPageSize)
	if perr != nil {
		sink.say("列 %s 失败（构造请求）：%s", liveProbeClip(parentFileID, 12), sink.mask(perr.Error()))
		return nil, false
	}
	envelope, cerr := run.Upstream.Call(context.Background(), request)
	if cerr != nil {
		sink.say("列 %s 失败：%s", liveProbeClip(parentFileID, 12), sink.mask(cerr.Error()))
		return nil, false
	}
	payload, oerr := envelope.EnsureOK()
	if oerr != nil {
		sink.say("列 %s 回非 OK：%s", liveProbeClip(parentFileID, 12), sink.mask(oerr.Error()))
		return nil, false
	}
	root, ok := payload.(map[string]any)
	if !ok {
		sink.say("列 %s 的 data 不是对象，实得 %T", liveProbeClip(parentFileID, 12), payload)
		return nil, false
	}
	rawItems, _ := root["items"].([]any)
	out := make([]liveProbeRawItem, 0, len(rawItems))
	for _, entry := range rawItems {
		item, ok := entry.(map[string]any)
		if !ok {
			continue
		}
		fileID, _ := item["fileId"].(string)
		name, _ := item["name"].(string)
		sink.see(fileID)
		typeWord := "-"
		if value, present := item["type"]; present {
			typeWord = liveProbeScalarWord(value)
		}
		created := "-"
		if value, present := item["createdAt"]; present {
			// ⚠ 这里**不截断**：截断过的串再也解析不回来（第一发就把 2015 那个日期剪成
			// 24 字带省略号，于是"距今"那一列念成"读不成时间"—— 那是**我们自己剪的**，
			// 不是上游的格式不认识）。要防的是超长值进日志，所以只设一个宽上限。
			created = liveProbeScalarRaw(value)
		}
		out = append(out, liveProbeRawItem{fileID: fileID, name: name, typeWord: typeWord, created: created})
	}
	return out, true
}

// liveProbeTallyWord 把 tally 摊成"值→个数"的有序串（有序是为了两趟读数可比）。
func liveProbeTallyWords(tally map[string]int) string {
	keys := make([]string, 0, len(tally))
	for key := range tally {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	parts := make([]string, 0, len(keys))
	for _, key := range keys {
		parts = append(parts, fmt.Sprintf("%s→%d", key, tally[key]))
	}
	return strings.Join(parts, " ")
}

// liveProbeScalarWord 把 `items[]` 里那一格摊成**可读的字面**，并按形态分档。
//
// 为什么不能只接数字：2026-09-26 这一发实测 `type` 与 `createdAt` 两格都是**字符串**
// ⇒ 一个"按 int 摊"的 helper 会把 808 项全念成 `?`，而那看起来很像"上游没给这一格"
// （坑表 #182：与 #176 同族 —— 上游这一族的字段常常回串，判据别按数字形状写）。
// 值本身短且是元数据（`FILE` / `DIR` / 日期串），截断后可以直接念。
func liveProbeScalarWord(value any) string {
	switch typed := value.(type) {
	case string:
		return liveProbeClip(typed, 24)
	case json.Number:
		return typed.String()
	case bool:
		return strconv.FormatBool(typed)
	case nil:
		return "null"
	default:
		return fmt.Sprintf("?(%T)", value)
	}
}

// liveProbeScalarRaw 与 [liveProbeScalarWord] 同一档，只多一句"剪宽一点"：
// 凡**后面还要拿去算**的格子都用它，凡只用来显示的用剪过的版本。
func liveProbeScalarRaw(value any) string {
	switch typed := value.(type) {
	case string:
		return liveProbeClip(typed, 64)
	default:
		return liveProbeScalarWord(value)
	}
}

// liveProbeMinutesAgo 把一格的"创建时间"换算成距今多少分钟；读不出来交回 false。
//
// 两种形态都试：上游这一族既有毫秒数（`UserTimestamp` 那类接口）也有日期串
// （本发实拍到的是串）⇒ 只接一种会把另一格念成"未知"。
func liveProbeMinutesAgo(raw string, now int64) (int64, bool) {
	if asInt, err := strconv.ParseInt(raw, 10, 64); err == nil && asInt > 0 {
		return (now - asInt) / 60000, true
	}
	for _, layout := range []string{time.RFC3339Nano, time.RFC3339, "2006-01-02T15:04:05", "2006-01-02 15:04:05"} {
		parsed, perr := time.Parse(layout, raw)
		if perr == nil {
			return (now - parsed.UnixMilli()) / 60000, true
		}
	}
	return 0, false
}

// TestLiveProbeCloudTreeForPDF 在**不再产生任何写入**的前提下问"上一发那一枚产物在哪"。
//
// 为什么值得单开一发：上面那一趟只读了根目录一页，而"根目录没有"离"网盘里没有"
// 还隔着一整棵树（邮箱附件目录、`/synbox` 子目录都是候选）。这一发只做 `file/list`
// ⇒ 可以多跑，也不占"带写"那一档的额度。
//
// 念三样：① 每一层 `type` 的**取值分布**（这一格今天零样本：R15 那一族的老毛病）；
// ② 我方前缀（[CloudUploadNamePrefix]）出现在哪些路径下 —— 那是清理链今天**看不见**的
// 残留，念出来既是判据也是卫生；③ 不是我方前缀、但扩展名是 `.pdf` 的那几枚，
// 只念**距今多少分钟**与名字长度（用户自己的文件名不进日志）。
//
// ⚠ 三条边界：只到 `liveProbeWalkMaxDepth` 层、最多列 `liveProbeWalkMaxDirs` 个目录、
// 每个目录只看第一页 ⇒ "没找到"**只**意味着"在这个对照面里没有"。
func TestLiveProbeCloudTreeForPDF(t *testing.T) {
	run, account, sink, _ := liveProbeWorld(t)
	// ⚠ 单位：`UserTimestamp` 是**微秒**（它是请求头那一枚），而上游 `createdAt` 是毫秒日期串。
	// 顺手拿 `UserTimestamp` 来算"距今"会得到 56,700 年这样一个**不报错**的读数
	// —— 本发第一跑就是这么错的（坑表 #183）。
	now := run.Clock.Now().UnixMilli()

	opening, perr := run.SignInWorld.Open(context.Background(), account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}

	type queueItem struct {
		fileID string
		depth  int
		trail  string
	}
	pending := []queueItem{{fileID: "/", depth: 0, trail: "/"}}
	listed := 0
	typeTally := map[string]int{}
	dirTypeTally := map[string]int{} // 被我们当目录去列的那一枚，它的 `type` 取值分布
	missedFolders := 0               // 粗筛漏掉的 `folder`（名字里带点）⇒ 对照面的洞有多大
	ours := 0
	others := 0
	for len(pending) > 0 && listed < liveProbeWalkMaxDirs {
		current := pending[0]
		pending = pending[1:]
		items, ok := liveProbeRawListing(run, opening, sink, current.fileID)
		if !ok {
			continue
		}
		listed++
		dirs := 0
		for _, item := range items {
			typeTally[item.typeWord]++
			if strings.HasPrefix(item.name, CloudUploadNamePrefix) {
				ours++
				// 我方造的文件 ⇒ 名字与路径都可以念（那本来就是我们写进去的那一枚）。
				// ⚠ 路径里夹着用户自己的目录名：只进测试 stdout，**不进成文**（成文只写
				// "命中 N 枚 / 在不在根目录"这一级）。
				sink.say("命中我方前缀：%s（在 %s，第 %d 层，type=%s）",
					item.name, current.trail, current.depth, item.typeWord)
			} else if strings.EqualFold(path.Ext(item.name), ".pdf") {
				others++
				ageWord := "未知（这一格读不成时间）"
				if minutes, okAge := liveProbeMinutesAgo(item.created, now); okAge {
					ageWord = fmt.Sprintf("%d 分钟", minutes)
				}
				sink.say("一枚不是我方前缀的 .pdf：名字长度 %d，createdAt=%s（距今 %s），type=%s，第 %d 层",
					len([]rune(item.name)), item.created, ageWord, item.typeWord, current.depth)
			}
			// 候选目录先按"名字里没有点"粗筛，同时把**粗筛漏掉的 `folder`** 数出来：
			// 本仓今天没有"这一枚是不是目录"的生产判据（`type` 那一格 [upstream.ParseCloudFileList]
			// 不读），所以粗筛一定有洞（名字里带点的目录，如 `2024.5 相册`）——
			// 那个洞不修成"按 type 筛"，是因为**这一发的目的**只是"产物在不在树里"，
			// 而把每一枚 `folder` 都列一遍要打的只读枪数由用户网盘结构决定，不由探针决定。
			// ⚠ 念出来是为了让下一轮知道该按哪个口径改（读数里 `type` 已经有值了）。
			isFolderShape := strings.EqualFold(item.typeWord, liveProbeFolderTypeWord)
			if current.depth < liveProbeWalkMaxDepth && item.fileID != "" && item.fileID != "/" {
				if path.Ext(item.name) == "" && !strings.Contains(item.name, ".") {
					dirs++
					dirTypeTally[item.typeWord]++
					pending = append(pending, queueItem{
						fileID: item.fileID,
						depth:  current.depth + 1,
						trail:  current.trail + "▸" + item.name,
					})
				} else if isFolderShape {
					missedFolders++
				}
			}
		}
		sink.say("列完第 %d 个目录（深度 %d）：%d 项，其中候选目录 %d 枚，待列 %d 枚",
			listed, current.depth, len(items), dirs, len(pending))
	}
	shapes := liveProbeTallyWords(typeTally)
	dirShapes := liveProbeTallyWords(dirTypeTally)
	sink.say("小结：列了 %d 个目录（上限 %d，深度上限 %d，没列完的不算找不到）；"+
		"type 读数 = %s；被当成目录去列的那些枚的 type = %s；我方前缀命中 %d 枚；非我方 .pdf %d 枚",
		listed, liveProbeWalkMaxDirs, liveProbeWalkMaxDepth, shapes, dirShapes, ours, others)
	sink.say("覆盖：队列里还剩 %d 枚没列（上限 %d 用掉 %d）；粗筛漏掉的 folder %d 枚"+
		"（名字里带点的目录，本发没进去）⇒ \"走完\"与\"到上限\"是两种读数",
		len(pending), liveProbeWalkMaxDirs, listed, missedFolders)
	sink.say("读法：后一份 tally 若只有一档 ⇒ 那一档就是「目录」的 type 取值（下一轮可以按值筛，" +
		"不必再靠「名字里有没有点」粗筛）；两档都有 ⇒ 粗筛与 type 不一致，仍按粗筛口径说话")
	sink.say("边界：①只到上面那两个上限；②每个目录只看第一页；③回收站里的不算 ⇒ " +
		"这一发只能说\"在这个对照面里有没有\"，不能说\"网盘里有没有\"")
}
