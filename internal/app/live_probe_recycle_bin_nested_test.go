package app

// 活体探针：**删掉最外层那枚目录，这棵子树在回收站里占几条**（第廿八趟）。
//
// # 这一趟答的两问（外加一格顺带）
//
//  1. **条数**：第廿七趟读到"删一枚**里面有一枚文件**的目录 ⇒ 回收站里只多**一条**（那条目录），
//     子文件不单列"。但那一趟的子树只有一层、只有一枚文件 ⇒ 它没法区分两种解释：
//     **A「整棵子树压成一条」**与 **B「只把被点名的那一枚记一条，子目录另算」**。
//     本趟的子树是 `outer ─ inner ─ leaf.png` 外加 `outer ─ sib.png`（**两层、四枚**）⇒
//     三种读法各自对应不同的命中集合：只 outer 在 = A；outer+inner 在 = 每层目录各一条；
//     连 leaf/sib 也在 = 逐文件拆条（那会**推翻**第廿七趟那一档）。
//  2. **`trashedDirStatistics` 的语义**：第廿七趟那枚"刚放进 1 个文件就被整棵收走"的目录，
//     这一格回的是 `0/0/0`（成文里那三种解释分不开，见 `internal/upstream/app_cloud_recyclebin.go`
//     的 [CloudRecycleBinDirStats] 注释）。本趟的 fixture 刻意让**子树**与**直接子级**不同数：
//     子树 = 1 目录 + 2 文件（每枚 64 字节），直接子级 = 1 目录 + 1 文件（64 字节）⇒
//     `1/2/128` 才是"统计整棵子树"、`1/1/64` 是"只统计直接子级"、`0/0/0` 则与上一趟同档
//     （这一格在"刚被删的目录"上就是不参与统计，要分它仍得拿一枚**没删**的目录读同一格）。
//
// 顺带的第三格：`ParseCloudRecycleBinPage` 今天**只有合成用例**（`app_cloud_recyclebin_test.go`
// 那八条都是喂手写 JSON）。这一趟在裸读之外**再走一遍解析器**，让第一次真上游读数与裸读对照 ——
// 解析器对真形状跑通没有，本身也是成文里缺的一格。
//
// # 与上一趟那条"只打一枪"不矛盾
//
// 第廿七趟抬头写的"嵌套目录与'重复删同一枚'是另一趟的问题，混做一案两问都收不到答案"，
// 指的是**要第二发写**的那一格（幂等）。本趟那两问读的是**同一发 batchTrash 造成的同一条条目**
// （条数 = 命中几条、统计 = 那一条上的三格），一枪就够 ⇒ **仍然只打一枪**。
// "对同一枚 fileId 重复打 batchTrash"本趟不碰，留给单独一趟（它需要第二发，且判据是"条数变不变"，
// 与本趟"一棵子树几条"混在一起两问都收不到干净答案）。
//
// # 写面与残留
//
// 造 4 枚（2 目录 + 2 文件）、删 1 枚（最外层那枚目录）⇒ 若上面读数是"一条"，
// 用户回收站里的净增就是 **1 条**（子树跟着，不另计）；若是 2 条或 4 条，残留就是那个数 ——
// 这一格本趟会**逐字念出来**（它是判据的副产品，也是 R37ⓒ 那笔残留账的一部分）。
// 中途任何一步造失败 ⇒ 把**已经造出来的最外层那一枚**batchTrash 掉收场，不在用户网盘里留坑
// （与第廿六/廿七趟同一档处置：本仓没有"彻底删除"口，收场只能进回收站）。
//
// # 凭据与隐私边界（沿用第廿七趟那一族，一条不放宽）
//
//   - 响应**只念形状**走 [liveProbeShapeValue]；逐条读数只收带 `asign-` 前缀的（我们自己造的名），
//     用户条目**只计数、不念名字与字段值**；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；解析器那一发**不打印** `FileID` 字段
//     （[liveProbeParsedEntryWord] 的键名单里没有它，且有一条非活体测试钉住这一格）；
//   - 翻页游标的**值不打印**，只念"有没有、长度多少"；
//   - `thumbnailUrls` 里那枚读取入口与预签名 `uploadUrl` 同档 ⇒ 本层根本不读它。
//
// # 闸门
//
// 翻页沿用 [liveProbeBinMaxPages] / [liveProbeBinMaxItems]；回读最多 [liveProbeBinMaxPasses] 遍，
// 遍与遍之间**只等、一枪不发**。为什么要多遍：第廿七趟实测这一口**有秒级不回**
// （整名命中 `false → true`、条数 `33 → 34`）⇒ 单遍"没找到"分不开"还没进"与"真没进"，
// 而这一趟的判据（几条）必须先确认 outer 那一条**到了**才念得出口。

import (
	"context"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// 四枚名字的中段（都可打印：整名带 `asign-` 前缀 ⇒ 是我们自己造的）。
	liveProbeNestDirInfix  = "nestdir-"  // 最外层那枚目录：**只有它进 batchTrash**
	liveProbeNestKidInfix  = "nestkid-"  // outer 里的子目录
	liveProbeNestLeafInfix = "nestleaf-" // inner 里那枚文件（第二层）
	liveProbeNestSibInfix  = "nestsib-"  // outer 里那枚文件（第一层）

	// liveProbeNestMaxPasses 是回读的上限遍数（每遍之间等 [liveProbeBinWait]）。
	// 3 遍 = 最多约 40s：第廿七趟读到 20s 那一档已经够了，这里只是不让"没找到"
	// 变成单遍读数。它是**闸门不是判据**：念判据前会先念"在第几遍找到的"。
	liveProbeNestMaxPasses = 3
)

func TestLiveProbeRecycleBinNested(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器（本探针要先造四枚再删最外层）：%s", binErr(sink, uerr))
	}

	// ⚠ 名字只在这里造一次（建、删、回读三处比的是同一批整名 —— 第廿七趟 #196 那条教训）。
	outerName := CloudUploadNamePrefix + liveProbeNestDirInfix + randomHex(uploader.Rng, cloudRandomNameHexLen)
	innerName := CloudUploadNamePrefix + liveProbeNestKidInfix + randomHex(uploader.Rng, cloudRandomNameHexLen)
	leafName := CloudUploadNamePrefix + liveProbeNestLeafInfix + randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"
	sibName := CloudUploadNamePrefix + liveProbeNestSibInfix + randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"

	// ── ① 造四枚：outer / outer 里的 inner / inner 里的 leaf / outer 里的 sib ──
	created, cerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, "/", outerName)
	switch {
	case cerr != nil:
		sink.say("① 造最外层目录失败：%s ⇒ 下面全部不发（没有我方认得出的对象，列回收站只会读到用户自己的条目）",
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
	outerID := *created.FileID
	sink.see(outerID)

	// bail 是"中途造失败"那一档的收场：把已经造出来的**最外层**那一枚删掉。
	// 为什么删最外层而不是逐枚删：inner 与那两枚文件都在它下面 ⇒ 一枪就把这半棵树带走
	// （第廿七趟实测删非空目录上游认了），而逐枚删要打四枪、把"重复删"那一格顺手污染。
	bail := func(what string) {
		sink.say("  收场：把最外层 %q 那一枚 batchTrash 掉（不在用户网盘里留半棵树；"+
			"它会进回收站，本仓无'彻底删除'口）：%s", outerName, what)
		if sent, derr := uploader.DeleteCloudFiles(ctx, []string{outerID}); derr != nil || !sent {
			sink.say("  收场那一枪也没打通（发过=%v）：%s", sent, binErr(sink, derr))
		}
		recorder.sayTally()
	}

	innerCreated, icerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, outerID, innerName)
	if icerr != nil || innerCreated.Exist || innerCreated.FileID == nil {
		sink.say("① 在最外层里造子目录没成（同名已存在=%v）：%s ⇒ 两问都答不了（子树只剩一层，"+
			"和上一趟那一档分不开）", innerCreated.Exist, binErr(sink, icerr))
		bail("造子目录这一枚没成")
		return
	}
	innerID := *innerCreated.FileID
	sink.see(innerID)

	leaf, lerr := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: innerID, Name: leafName})
	if lerr != nil || leaf.FileID == "" {
		sink.say("① 往子目录里放文件没成：%s", binErr(sink, lerr))
		bail("第二层那枚文件没传上")
		return
	}
	sink.see(leaf.FileID)
	sib, serr := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: outerID, Name: sibName})
	if serr != nil || sib.FileID == "" {
		sink.say("① 往最外层里放文件没成：%s ⇒ 子树只剩一枚文件，'直接子级'与'整棵子树'"+
			"两档分不开 ⇒ 不做这一趟的判据", binErr(sink, serr))
		bail("第一层那枚文件没传上")
		return
	}
	sink.see(sib.FileID)
	sink.say("① 造好了四枚：目录 %q（fileId 长度 %d）/ 里面目录 %q（长度 %d）/ "+
		"再里面 %q（长度 %d）/ 最外层里另有 %q（长度 %d）",
		outerName, len(outerID), innerName, len(innerID), leafName, len(leaf.FileID),
		sibName, len(sib.FileID))
	sink.say("① 这一棵的形状：子树 = 1 枚子目录 + 2 枚文件，**直接**子级 = 1 枚子目录 + 1 枚文件"+
		"（两档不同数 ⇒ 才读得出 %q 到底统计哪一档）", "trashedDirStatistics")

	// ── ② 只打一枪：删**最外层**那一枚（inner 与两枚文件都不点名）──
	sent, derr := uploader.DeleteCloudFiles(ctx, []string{outerID})
	if derr != nil || !sent {
		sink.say("② batchTrash(最外层) 没成（发过=%v）：%s ⇒ 这一趟不回答'这棵子树几条'",
			sent, binErr(sink, derr))
		recorder.sayTally()
		return
	}
	sink.say("② batchTrash(最外层)：上游认了（**只这一枚**，其余三枚都没点名）")

	// ── ③ 回读：最多 liveProbeNestMaxPasses 遍，遍与遍之间只等 ──
	wanted := []string{outerName, innerName, leafName, sibName}
	exactAt := func() map[string]bool {
		m := make(map[string]bool, len(wanted))
		for _, n := range wanted {
			m[n] = false
		}
		return m
	}
	var (
		last      *liveProbeBinScan
		lastPages int
		// 两份"第一次"分开记：③ 的**裸读**（翻页扫到底、按形态扫）与每遍后面那一发
		// **解析器**（只读第一页）是**相邻的两枪**，而第廿八趟三趟实拍里都有某一遍
		// 一个有一个没有（run2：24 项 vs 25 条；run3：25 项 vs 26 条，见 capture §11）
		// ⇒ 判定所依据的那一发必须说清是哪一发，不然"第几遍进索引"这句是假的精确。
		foundAtNaked  int
		foundAtParsed int
		passCounts    []int
		passStats     []string
		union         = exactAt()
	)
	// nameSet 只取**键集合**（值一律 true）：解析器那一发用它筛"本趟那四枚"，
	// 而 `union` 的值是命中与否（false 也合法）⇒ 拿 union 当过滤器会把没命中的那几枚
	// 永远滤掉，读数是错的。
	nameSet := make(map[string]bool, len(wanted))
	for _, n := range wanted {
		nameSet[n] = true
	}
	for pass := 1; pass <= liveProbeNestMaxPasses; pass++ {
		if pass > 1 {
			sink.say("③ 第 %d 遍前先等 %s，中间**一枪都不发**（要分清「还没进」与「真没进」，"+
				"出处：第廿七趟实测的秒级不回）", pass, liveProbeBinWait)
			time.Sleep(liveProbeBinWait)
		}
		scan := &liveProbeBinScan{exact: exactAt()} // mine 留 nil：本趟不按中段认（#196）
		pages, ok := liveProbeBinSweep(sink, run, uploader, "", "③ 第 "+strconv.Itoa(pass)+" 遍", scan)
		if !ok {
			sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是读数）⇒ 两问都不判")
			recorder.sayTally()
			return
		}
		last, lastPages = scan, pages
		passCounts = append(passCounts, scan.items)
		for name, hit := range scan.exact {
			union[name] = union[name] || hit
		}
		if scan.exact[outerName] && foundAtNaked == 0 {
			foundAtNaked = pass
		}
		// ⚠ 每一遍**都**走一遍解析器读那三格，不在"条目一出现"就收工：判据二有实拍两档
		// （子树里只有文件的目录是 `0/0/0`、子树含目录的是 `1/0/0`，两档同趟可见，
		// 见 `docs/capture/app-cloud-recyclebin-samples.txt` §11）⇒ "统计格比条目本身晚填"
		// 与"两种形状本就算得不一样"只有在这里逐遍读同一格才分得开。
		read := liveProbeNestStatsAt(ctx, sink, run, uploader, outerName, nameSet, pass)
		passStats = append(passStats, read.token)
		if word := liveProbeNestPassDiffWord(pass, scan.items, read.items); word != "" {
			sink.say("③ %s", word)
		}
		if read.outerSeen {
			if foundAtParsed == 0 {
				foundAtParsed = pass
			}
			// 并集也吃解析器那一份：判据一（"这棵子树占几条"）问的是"有没有出现过"，
			// 而同一遍内解析器那一发已经列出来的条目，裸读那一发可能还没列（§11）。
			union[outerName] = true
		}
		if !read.ok {
			break // 解析器那一发本身不通 ⇒ 后面几遍也读不到新东西，省下枪
		}
	}
	if foundAtNaked == 0 && foundAtParsed == 0 {
		sink.say("③ 翻了 %d 遍（每遍 %v 条，闸门 %d 页 / 最多 %d 条）都没在最外层那一枚上读到 ⇒ "+
			"下面那两问一律不判（'没读到'有三种读法：还没进索引 / 被闸门挡住 / 这一口回的不是回收站条目）",
			len(passCounts), passCounts, liveProbeBinMaxPages, liveProbeBinMaxItems)
	} else {
		sink.say("③ 最外层那一条**第一次**出现在：裸读第 %s 遍 / 解析器第 %s 遍（共读 %d 遍，"+
			"每遍先裸读、紧接着单发解析器读第一页 ⇒ 两把尺各记各的，两发不一致时上面有单行点名；"+
			"上一趟口径：只认等过之后那一份）；末遍页数 %d、%s",
			liveProbeNestPassWord(foundAtNaked), liveProbeNestPassWord(foundAtParsed),
			len(passCounts), lastPages, liveProbeTruncWord(last.truncated))
	}
	sink.say("③ 每遍的三格读数（%d 遍）：%s", len(passStats), strings.Join(passStats, " | "))
	sink.say("③ ⇒ %s", liveProbeNestStatsSequence(passStats))

	// ── ④ 判据一：这棵子树在回收站里占几条 ──
	shapeWord, shapeCount := liveProbeNestShapeWord(last.exact)
	unionWord, unionCount := liveProbeNestShapeWord(union)
	sink.say("④ 四枚整名的命中（末遍）：目录(最外层)=%v 子目录=%v 第二层文件=%v 第一层文件=%v ⇒ 命中 %d 条",
		last.exact[outerName], last.exact[innerName], last.exact[leafName], last.exact[sibName], shapeCount)
	sink.say("④ 各遍**并集**：命中 %d 条（%s）", unionCount, unionWord)
	sink.say("④ ⇒ %s", shapeWord)
	if shapeCount != unionCount {
		sink.say("④ ⚠ 末遍与并集不一致 ⇒ 这一族里有条目**在趟内消失过**（被还原 / 被彻底删除 / 上游侧回收）" +
			"⇒ 这一趟的命中数以**并集**为准，且'条数的差'不能当'本趟净增几条'的判据")
	}
	sink.say("④ 反向核对（这一格**不靠**上面那句推断）：上一趟那两枚的中段是 %q / %q，"+
		"本趟四枚的中段互不相同 ⇒ 命中数只可能来自**本趟现造**的那几枚，不是同族残留（#196）",
		liveProbeBinFolderInfix, liveProbeBinChildInfix)

	// ── ⑤ 判据二的落点：三格的逐遍读数已在 ③ 里**每遍都走解析器**念过了 ──
	// 这里只把**最后一遍**当判据（第廿七趟那条"只认等过之后那一份"的口径，延伸到同一格上）：
	// 当场那份如果与后面几遍不一致，那就是"这一格晚填"，不是"这一族算得不一样"。
	lastStats := "一遍都没读到"
	if len(passStats) > 0 {
		lastStats = passStats[len(passStats)-1]
	}
	sink.say("⑤ 判据二取**最后一遍**那份：三格短码 %q（共读 %d 遍）。对照：第廿七趟"+
		"「子树 = 1 枚文件」那格读到 0/0/0，本趟「子树 = 1 枚子目录 + 2 枚文件」读到 %q ⇒ "+
		"读法与分档见 `docs/capture/app-cloud-recyclebin-samples.txt` §11", lastStats, len(passStats), lastStats)

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪；写侧只有那一次 batchTrash(最外层) ⇒ 残留 = 用户回收站里本趟那棵子树"+
		"（净增几条就看 ④ 那个数），本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeBinPath, recorder.countFor(liveProbeBinPath))
}

// liveProbeNestPassRead 是解析器那一发的四格读数（做成一枚结构体是因为调用方要用到
// 四格里的三格，而"这一发通没通"与"这一发里有没有那一条"是两件事）。
type liveProbeNestPassRead struct {
	token     string // 三格短码（"1/0/0" / "缺键" / "nil" / "缺席" / "没打通"）
	ok        bool   // 这一发**打没打通**（不通则后面几遍也没有新东西）
	outerSeen bool   // 这一发里有没有最外层那一条（按**整名**，#196 同口径）
	items     int    // 这一发念到的第一页条数（与裸读那一发的条数**各记各的**）
}

// liveProbeNestPassDiffWord 念"同一遍内两发条数不一致"，一致时交回空串（调用方不印）。
//
// 为什么要专门念这一句：这一趟有两把尺（裸读按形态扫、解析器按字段读），而它们各自
// **单发**打上游。第廿八趟三趟实拍里都有某一遍两发差一条 ⇒ 如果只留一个"第几遍"，
// 读者会以为"这一遍"是一个时刻的快照，而它其实是**两秒钟内的两发**。
// 反向改坏它：把 `if naked == parsed { return "" }` 删掉 ⇒ `TestLiveProbeNestPassDiffWord` 红。
func liveProbeNestPassDiffWord(pass, naked, parsed int) string {
	if naked == parsed {
		return ""
	}
	return fmt.Sprintf("第 %d 遍两发不一致：裸读 %d 条 / 解析器 %d 条 ⇒ 相邻两发就能一个有一个没有"+
		"（不是同一发的对照，别把其中一个当'这一遍的条数'；口径见 capture §11）", pass, naked, parsed)
}

// liveProbeNestStatsAt 走**解析器**读一遍第一页，念我方那几枚，并交回最外层那一条的三格短码。
//
// 为什么要单独一发（而不是只看 ③ 的裸读）：裸读按"形态"扫、只念键与长度，而这一趟的判据二
// 要的是 `subDirCount/subFileCount/totalSize` 的**数**；同一发顺带就是
// `ParseCloudRecycleBinPage` 对真上游的第一批读数（它此前只有合成用例）。
//
// `outerSeen` 必须交回调用方单独记一遍"第几遍"：裸读与解析器是**相邻的两枪**，而三趟实拍里
// 同一遍内它们就一个有一个没有（capture §11）⇒ 只记一个"第几遍"会让人以为两把尺一致。
func liveProbeNestStatsAt(
	ctx context.Context, sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader,
	outerName string, wanted map[string]bool, pass int,
) liveProbeNestPassRead {
	page, perr := run.Upstream.AppCloudRecycleBinList(ctx, uploader.Auth, uploader.DeviceID, "")
	if perr != nil {
		sink.say("③ 第 %d 遍（解析器）：这一发没通：%s ⇒ 三格这一遍没读数", pass, binErr(sink, perr))
		return liveProbeNestPassRead{token: "没打通"}
	}
	ours := 0
	for _, entry := range page.Items {
		if entry.Name != nil && strings.HasPrefix(*entry.Name, CloudUploadNamePrefix) {
			ours++
		}
	}
	token, word, _ := liveProbeNestStatsWord(page, outerName)
	sink.say("③ 第 %d 遍（解析器：第一页 %d 条 / 我方前缀 %d 条、游标键在=%v、%s）最外层那条的三格 = %q ⇒ %s",
		pass, len(page.Items), ours, page.HasCursorField,
		liveProbeParsedCursorWord(page.NextPageCursor), token, word)
	read := liveProbeNestPassRead{token: token, ok: true, items: len(page.Items)}
	for _, entry := range page.Items {
		if entry.Name == nil || !wanted[*entry.Name] {
			continue // 只念本趟那四枚：用户条目一个字段都不进输出（含同名中段的更早残留）
		}
		if *entry.Name == outerName {
			read.outerSeen = true
		}
		sink.say("   %s", liveProbeParsedEntryWord(entry))
	}
	return read
}

// liveProbeNestPassWord 念"第几遍第一次看到"，0 遍那一档念"这一趟没看到"。
//
// 为什么不做成 `%d` 再让 0 自己念出来：`0` 会被读成"第 0 遍"（一个不存在的遍数），
// 而真实意思是"这一发从来没列出它"—— 这一档在判据里是三件事之一（见 ③ 那句"没读到有三种读法"）。
func liveProbeNestPassWord(pass int) string {
	if pass == 0 {
		return "—"
	}
	return strconv.Itoa(pass)
}

// liveProbeNestStatsSequence 把"每遍读到的三格短码"折成一句读法。
//
// 这一格是本趟现学的：两趟实拍里同一枚"刚删的目录"一趟给 `0/0/0`、另一趟给 `1/0/0`，
// 而差别可能只是**读的时刻**不同 ⇒ 只有"同一趟内逐遍读同一格"能分开"晚填"与"算得不一样"。
func liveProbeNestStatsSequence(tokens []string) string {
	switch len(tokens) {
	case 0:
		return "一遍都没读到 ⇒ 三格这一问今天没读数"
	case 1:
		return "只读到一遍（第 " + tokens[0] + "）⇒ **单遍读数分不开'晚填'与'本就如此'**，这一格标未答"
	}
	same := true
	for _, token := range tokens {
		if token != tokens[0] {
			same = false
			break
		}
	}
	if same {
		return "⭐ " + strconv.Itoa(len(tokens)) + " 遍读到的三格**逐字相同**（都是 " + tokens[0] +
			"）⇒ 这一格在趟内是稳定的：当场读到的那个数就是终值，不是「晚填了一半」"
	}
	return "⭐ 三格随遍数**变了**（" + strings.Join(tokens, " → ") + "）⇒ 这一格比条目本身**更晚填**：" +
		"「当场读到的数」不能当终值判据，要判它只认**最后一遍**那一份"
}

// TestLiveProbeRecycleBinNestedReread ⭐ **只读**复跑：把"这一棵子树在回收站里占几条"
// 变成一支不写任何东西、随时能再读一遍的探针。
//
// 为什么要有它（第廿八趟当天现加的）：写探针那一趟（[TestLiveProbeRecycleBinNested]）的
// stdout 只在终端里看了一眼 ⇒ 逐字读数没留档。这一族**每一次复跑都会新增残留**（造四枚、
// 删一枚），所以"重跑一遍好把日志留下"不是免费的；而这一趟判据（几条 + 那条的三格）
// 在**只读**侧同样读得到 —— 只要认得出我方那几枚。
//
// 认法只能是**中段**（`nestdir-` 那四串），不是整名：整名是写探针现造的，复跑拿不到。
// 这与坑表 #196 不冲突 —— 那条讲的是"**当场 vs 等后**两遍对照必须用整名"，
// 这里问的是"这一族在不在回收站里"，同名中段的残留恰好都算这一族的读数。
// 但要注意：命中数会把**更早趟同中段的残留**一起算进来 ⇒ 这一趟念的是"至少几条"，
// 逐条明细里每枚的整名都会念出来，判"本趟那一枚"要对着整名看。
func TestLiveProbeRecycleBinNestedReread(t *testing.T) {
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

	scan := liveProbeBinScan{mine: map[string]bool{
		liveProbeNestDirInfix:  false,
		liveProbeNestKidInfix:  false,
		liveProbeNestLeafInfix: false,
		liveProbeNestSibInfix:  false,
	}}
	pages, ok := liveProbeBinSweep(sink, run, uploader, "", "① 只读复跑", &scan)
	if !ok {
		sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是读数）")
		recorder.sayTally()
		return
	}
	infixes := make([]string, 0, len(scan.mine))
	for infix := range scan.mine {
		infixes = append(infixes, infix)
	}
	sort.Strings(infixes)
	for _, infix := range infixes {
		sink.say("② 中段 %q 在回收站里：%v（翻了 %d 页；闸门 %d 页 / 最多 %d 条；%s）",
			infix, scan.mine[infix], pages, liveProbeBinMaxPages, liveProbeBinMaxItems,
			liveProbeTruncWord(scan.truncated))
	}

	page, pgerr := run.Upstream.AppCloudRecycleBinList(ctx, uploader.Auth, uploader.DeviceID, "")
	if pgerr != nil {
		sink.say("③ 解析器这一发没通：%s", binErr(sink, pgerr))
	} else {
		sink.say("③ 解析器（第一页，页长写死 %d）：本页 %d 条、游标键在=%v、下一发游标%s",
			upstream.CloudRecycleBinPageSize, len(page.Items), page.HasCursorField,
			liveProbeParsedCursorWord(page.NextPageCursor))
		for _, entry := range page.Items {
			if entry.Name == nil || !strings.Contains(*entry.Name, "asign-") {
				continue // 用户条目：一个字段都不念
			}
			for _, infix := range infixes {
				if strings.Contains(*entry.Name, infix) {
					sink.say("   %s", liveProbeParsedEntryWord(entry))
				}
			}
		}
	}
	sink.say("④ 这一趟**只发只读那一口**（账本里除它以外应当一枪都没有）⇒ 残留没有增加，可反复跑")
	recorder.sayTally()
	if n := recorder.countFor(liveProbeBinPath); n == 0 {
		sink.say("⚠ 账本里连 `%s` 都没有 ⇒ 上面那些读数不是这一口给的，别用", liveProbeBinPath)
	}
}

// liveProbeNestShapeWord 把"四枚整名命中了哪几枚"折成一句档位读数。
//
// 为什么单独成函数并带非活体测试（[TestLiveProbeNestShapeWord]）：这一段是**判据本身**
// （命中集合 → 结论），而它在活体探针里只在真跑时执行一次；把映射抽出来，"改坏它哪条测试红"
// 就说得出（AGENTS §1.3）。四个档位一一对应抬头那几种解释，彼此**互斥**，所以命中集合
// 落不进任何一档时走"另一档"那一条 —— 不硬贴一个结论。
func liveProbeNestShapeWord(exact map[string]bool) (string, int) {
	hits := 0
	for _, hit := range exact {
		if hit {
			hits++
		}
	}
	if hits == 0 {
		return "一条都没命中 ⇒ 这一档本趟不判（见 ③ 那行：先确认不是延迟/闸门）", 0
	}
	return liveProbeNestShapeByHits(exact, hits), hits
}

func liveProbeNestShapeByHits(exact map[string]bool, hits int) string {
	switch {
	case hits == 1 && exactHas(exact, "nestdir-"):
		return "⭐ 只有**最外层**那一条 ⇒ 整棵子树压成**一条**（子目录与两枚文件都不单列）" +
			"—— 这一档与第廿七趟那一档（一层一文件）同形，且**排除了**'只把点名的那枚记一条、" +
			"子目录另算'那种读法里跟子目录有关的部分"
	case hits == 2 && exactHas(exact, "nestdir-") && exactHas(exact, "nestkid-"):
		return "最外层 + 子目录**各一条**、两枚文件都不在 ⇒ 目录**逐层**各记一条，文件不单列"
	case hits >= 3:
		return "命中 " + strconv.Itoa(hits) + " 条（含文件或多层目录）⇒ **推翻**第廿七趟那句" +
			"'子文件不单列'，那一档要按这一趟的命中集合改写"
	default:
		return "命中集合落不进上面任何一档（hits=" + strconv.Itoa(hits) +
			"，只有非最外层那几枚在）⇒ 不硬贴结论，这一格要人对着逐条读数再看一遍"
	}
}

func exactHas(exact map[string]bool, infix string) bool {
	for name, hit := range exact {
		if hit && strings.Contains(name, infix) {
			return true
		}
	}
	return false
}

// liveProbeParsedEntryWord 念**解析器**交回的一条（只念白名单四格 + 目录统计三格）。
//
// ⚠ `FileID` 刻意不在里面：它是真实对象标识，与 `auth`/预签名 URL 同档（只念长度）。
// 这一格由 [TestLiveProbeParsedEntryWordHidesFileID] 钉住 —— 隐私边界里唯一"反向改坏就会红"的位。
func liveProbeParsedEntryWord(entry upstream.CloudRecycleBinEntry) string {
	parts := []string{
		"name=" + liveProbeParsedStr(entry.Name),
		"type=" + liveProbeParsedStr(entry.Type),
		"size=" + liveProbeParsedInt(entry.Size),
		"remainingTime=" + liveProbeParsedInt(entry.RemainingTime),
	}
	if entry.DirStats == nil {
		parts = append(parts, "目录统计=nil（file 型实拍是 null；folder 型读到 nil ⇒ 这一格形状变了）")
	} else {
		parts = append(parts, fmt.Sprintf("目录统计{subDirCount=%s subFileCount=%s totalSize=%s}",
			liveProbeParsedInt(entry.DirStats.SubDirCount),
			liveProbeParsedInt(entry.DirStats.SubFileCount),
			liveProbeParsedInt(entry.DirStats.TotalSize)))
	}
	return strings.Join(parts, " ")
}

// liveProbeNestStatsWord 从解析器那一页里挑出**最外层那一条**，交回三格短码 + 一句读法。
//
// 短码（"1/0/0" / "nil" / "缺键" / "缺席"）是给 [liveProbeNestStatsSequence] 折"逐遍变没变"用的；
// 判据映射放在这里，是因为它**跨趟**：第廿七趟那格是 `0/0/0`（子树 = 1 枚文件），
// 第廿八趟同一格是 `1/0/0`（子树 = 1 枚子目录 + 2 枚文件）⇒ 目录那一格**会**跟着子树里的
// 目录数动，而 `subFileCount` 与 `totalSize` 在两趟三种子树下**都是 0**。
// ⚠ 这一句只到"实拍里文件两格没反映内容"，不许写成"这一族不统计文件"（那是从两趟读数
// 归纳出的全称判断，AGENTS §1.7）；分得开它的下一趟设计见 capture §11 末。
func liveProbeNestStatsWord(page upstream.CloudRecycleBinPage, outerName string) (string, string, bool) {
	for _, entry := range page.Items {
		if entry.Name == nil || *entry.Name != outerName {
			continue
		}
		if entry.DirStats == nil {
			return "nil", "⑤ 最外层那一条的 trashedDirStatistics 读出的是 nil ⇒ 这一问没答（folder 型" +
				"本该是对象）：要么形状变了，要么上游对'刚删的目录'就不填", false
		}
		if entry.DirStats.SubDirCount == nil || entry.DirStats.SubFileCount == nil || entry.DirStats.TotalSize == nil {
			// ⚠ 这一档**不折进**下面那几档：`nil` 是"这一枚键不在 / 不是数字"，`0` 是"读到 0"。
			// 拿 -1 或 0 去比会把"上游改版了"念成"这一格就是不填"（第廿七趟 #197 同族）。
			return "缺键", "⑤ 三格里有读不出的那一格 ⇒ 只念形状不解释：" +
				strconv.Quote(liveProbeParsedEntryWord(entry)), false
		}
		dirs, files, size := intOf(entry.DirStats.SubDirCount), intOf(entry.DirStats.SubFileCount), intOf(entry.DirStats.TotalSize)
		token := fmt.Sprintf("%d/%d/%d", dirs, files, size)
		switch {
		case dirs == 1 && files == 2:
			return token, fmt.Sprintf("⭐ 三格读的是 %s ⇒ **统计整棵子树**（子目录 1、文件 2、字节 %d）："+
				"上一趟那格 0/0/0 不是'不统计'，而是那一棵只有一层一文件、当时读法分不开", token, size), true
		case dirs == 1 && files == 1:
			return token, "⭐ 三格读的是 " + token + " ⇒ 这一档**等于直接子级**（子树里那枚第二层文件没算进去，" +
				"整棵子树应是 1/2/128）；⚠ 但**同一形状**另两趟读到的是 1/0/0 ⇒ 这一格不由形状唯一决定，" +
				"而 `subDirCount` 在'只数直接子级'与'整棵子树'两档下同值（都是 1）⇒ 目录那一格今天仍分不开，" +
				"三档对照见 capture §11", true
		case dirs == 1 && files == 0 && size == 0:
			return token, "⭐ 实拍档：三格读的是 1/0/0 ⇒ `subDirCount` 跟到了子树里那枚目录，" +
				"而 `subFileCount` 与 `totalSize` 在子树明明有 2 枚文件（64+64 字节）时是 0" +
				" ⇒ 与同形状另一趟的 1/1/64 并见：**文件那两格填不填不是确定的**。" +
				"'晚填'那一档对早两枚已被隔十几分钟的回头读否掉（capture §11）⇒ 剩下的读法是" +
				"'算的时候没赶上'与'这一格本就算法不同'，分开它得拿一枚**没删**的目录读同一格", true
		case dirs == 0 && files == 0 && size == 0:
			return token, "⭐ 三格仍是 0/0/0 ⇒ 与第廿七趟同档：这一格在**刚被删的目录**上就是不填，" +
				"和子树大小无关（要分它仍得拿一枚**没删**的目录读同一格）", true
		default:
			return token, "三格读的是 " + token + " ⇒ 不落在上面任何一档（1/2、1/1、1/0/0、0/0/0）⇒ 不硬解释，" +
				"逐字读数就是这一行", true
		}
	}
	return "缺席", "⑤ 解析器那一页里没有最外层那一条 ⇒ 三格这一问这一发答不了", false
}

func liveProbeParsedStr(value *string) string {
	if value == nil {
		return "nil"
	}
	return strconv.Quote(*value)
}

func liveProbeParsedInt(value *int64) string {
	if value == nil {
		return "nil"
	}
	return strconv.FormatInt(*value, 10)
}

func intOf(value *int64) int {
	if value == nil {
		return -1 // "读不出"与"读到 0"是两件事：上面每一档都比的是**具体数**，-1 落不进任何一档
	}
	return int(*value)
}

// liveProbeParsedCursorWord 念游标：**值不打印**，只念有没有。
func liveProbeParsedCursorWord(cursor string) string {
	if cursor == "" {
		return "为空（= 上游说没有下一页）"
	}
	return "长度 " + strconv.Itoa(len(cursor)) + "（值不打印）"
}

// TestLiveProbeNestShapeWord 钉"命中集合 → 结论"那一段映射（非活体，`go test` 默认就跑）。
//
// 反向改坏它：把 `hits >= 3` 那一档删掉 ⇒ "逐层 + 逐文件"那条用例红；
// 把 `hits == 1` 改成 `hits <= 1` ⇒ "一条都没命中"那条用例红。
func TestLiveProbeNestShapeWord(t *testing.T) {
	outer, inner, leaf, sib := "asign-nestdir-aa", "asign-nestkid-bb", "asign-nestleaf-cc.png", "asign-nestsib-dd.png"
	for _, tc := range []struct {
		name       string
		exact      map[string]bool
		wantCount  int
		wantPrefix string
	}{
		{"子树压成一条", map[string]bool{outer: true, inner: false, leaf: false, sib: false}, 1, "⭐ 只有**最外层**"},
		{"逐层各一条", map[string]bool{outer: true, inner: true, leaf: false, sib: false}, 2, "最外层 + 子目录"},
		{"文件也单列（推翻上一档）", map[string]bool{outer: true, inner: true, leaf: true, sib: true}, 4, "命中 4 条"},
		{"一条都没有", map[string]bool{outer: false, inner: false, leaf: false, sib: false}, 0, "一条都没命中"},
		{"只有非最外层（不硬贴）", map[string]bool{outer: false, inner: true, leaf: false, sib: false}, 1, "命中集合落不进"},
	} {
		word, count := liveProbeNestShapeWord(tc.exact)
		if count != tc.wantCount {
			t.Fatalf("%s：命中条数 = %d，期望 %d（读数 %q）", tc.name, count, tc.wantCount, word)
		}
		if !strings.HasPrefix(word, tc.wantPrefix) {
			t.Fatalf("%s：档位文案开头 = %q，期望以 %q 开头", tc.name, word, tc.wantPrefix)
		}
	}
}

// TestLiveProbeParsedEntryWordHidesFileID 钉住"解析器那一行不带真实 fileId"这一格。
//
// 为什么单独一条：⑤ 那一段是这一族**第一次**把解析器交回的条目直接拼进日志，
// 而 `CloudRecycleBinEntry` 里就有 `FileID` —— 今天不念，是因为改一行 `parts = append(...,
// "fileId="+*entry.FileID)` 就能把它写进真跑日志（进而写进成文）。这条测试就是那"一行"的哨兵。
func TestLiveProbeParsedEntryWordHidesFileID(t *testing.T) {
	id := "SYNTH-FILE-ID-not-a-real-one"
	name, kind := "asign-nestdir-aa", "folder"
	dirs, files, size := int64(1), int64(2), int64(128)
	word := liveProbeParsedEntryWord(upstream.CloudRecycleBinEntry{
		FileID: &id, Name: &name, Type: &kind,
		DirStats: &upstream.CloudRecycleBinDirStats{SubDirCount: &dirs, SubFileCount: &files, TotalSize: &size},
	})
	if strings.Contains(word, id) || strings.Contains(strings.ToLower(word), "fileid") {
		t.Fatalf("解析器那一行漏了 fileId（隐私边界：只念长度）：%s", word)
	}
	for _, want := range []string{`name="asign-nestdir-aa"`, "subDirCount=1", "subFileCount=2", "totalSize=128"} {
		if !strings.Contains(word, want) {
			t.Fatalf("解析器那一行少了 %q：%s", want, word)
		}
	}
}

// TestLiveProbeNestStatsWord 钉三格那四种读法各自落到哪一句（判据映射，非活体）。
func TestLiveProbeNestStatsWord(t *testing.T) {
	name := "asign-nestdir-aa"
	entry := func(d, f, s *int64, stats bool) upstream.CloudRecycleBinEntry {
		out := upstream.CloudRecycleBinEntry{Name: &name, Type: ptrString("folder")}
		if stats {
			out.DirStats = &upstream.CloudRecycleBinDirStats{SubDirCount: d, SubFileCount: f, TotalSize: s}
		}
		return out
	}
	n := func(v int64) *int64 { return &v }
	for _, tc := range []struct {
		word       string
		entry      upstream.CloudRecycleBinEntry
		wantPrefix string
		wantRead   bool
		wantToken  string
	}{
		{"整棵子树", entry(n(1), n(2), n(128), true), "⭐ 三格读的是 1/2/128", true, "1/2/128"},
		{"只直接子级", entry(n(1), n(1), n(64), true), "⭐ 三格读的是 1/1/64", true, "1/1/64"},
		// ⭐ 第廿八趟的**实拍档**（子树 = 1 目录 + 2 文件，两枚文件各 64 字节）：
		{"实拍：目录算了文件没算", entry(n(1), n(0), n(0), true), "⭐ 实拍档：三格读的是 1/0/0", true, "1/0/0"},
		{"仍不填（第廿七趟档）", entry(n(0), n(0), n(0), true), "⭐ 三格仍是 0/0/0", true, "0/0/0"},
		{"另有一档", entry(n(3), n(7), n(9), true), "不落在上面任何一档", true, "3/7/9"},
		{"三格仍是 nil 对象（有键读不出）", entry(n(1), nil, n(64), true), "有读不出的那一格", false, "缺键"},
		{"folder 读到 nil", entry(nil, nil, nil, false), "读出的是 nil", false, "nil"},
		{"没有这一条", upstream.CloudRecycleBinEntry{}, "没有最外层那一条", false, "缺席"},
	} {
		token, word, read := liveProbeNestStatsWord(upstream.CloudRecycleBinPage{Items: []upstream.CloudRecycleBinEntry{tc.entry}}, name)
		if read != tc.wantRead {
			t.Fatalf("%s：答得上来 = %v，期望 %v（%q）", tc.word, read, tc.wantRead, word)
		}
		if !strings.Contains(word, tc.wantPrefix) {
			t.Fatalf("%s：文案缺 %q：%s", tc.word, tc.wantPrefix, word)
		}
		if token != tc.wantToken {
			t.Fatalf("%s：短码 = %q，期望 %q（短码是给'逐遍对照'用的，念错一句就折错一档）",
				tc.word, token, tc.wantToken)
		}
	}
}

// TestLiveProbeNestStatsSequence 钉"逐遍短码 → 有没有晚填"那一段。
func TestLiveProbeNestStatsSequence(t *testing.T) {
	for _, tc := range []struct {
		name       string
		tokens     []string
		wantPrefix string
	}{
		{"没读到", nil, "一遍都没读到"},
		{"只一遍", []string{"1/0/0"}, "只读到一遍"},
		{"三遍相同", []string{"1/0/0", "1/0/0", "1/0/0"}, "逐字相同"},
		{"后一遍变了", []string{"0/0/0", "1/0/0", "1/0/0"}, "随遍数**变了**"},
	} {
		word := liveProbeNestStatsSequence(tc.tokens)
		if !strings.Contains(word, tc.wantPrefix) {
			t.Fatalf("%s：%q 里没有 %q", tc.name, word, tc.wantPrefix)
		}
	}
}

// TestLiveProbeNestPassWord 钉"这一趟没看到"与"第 0 遍"不是一件事（判据三档里最容易被读错的一档）。
//
// 反向改坏它：把 helper 改成 `strconv.Itoa(pass)` ⇒ 本条红（0 会念成"第 0 遍"，
// 而 ③ 那一行同时念两把尺，读者会以为存在第 0 遍）。
func TestLiveProbeNestPassWord(t *testing.T) {
	if got := liveProbeNestPassWord(0); got != "—" {
		t.Fatalf("0 遍应念 %q（不是'第 0 遍'），实得 %q", "—", got)
	}
	if got := liveProbeNestPassWord(2); got != "2" {
		t.Fatalf("第 2 遍应念 %q，实得 %q", "2", got)
	}
}

// TestLiveProbeNestPassDiffWord 钉"同一遍内两发条数不一致"那一行只在**真的不一致**时念。
//
// 反向改坏它：删掉 `naked == parsed` 那一句 ⇒ 第二行红（一致时也念"不一致"，等于把探针
// 自己的两把尺每次都互相指认矛盾，读数就废了）。
func TestLiveProbeNestPassDiffWord(t *testing.T) {
	if got := liveProbeNestPassDiffWord(1, 25, 25); got != "" {
		t.Fatalf("两发一致时应交回空串（不印），实得 %q", got)
	}
	got := liveProbeNestPassDiffWord(1, 25, 26)
	for _, want := range []string{"裸读 25 条", "解析器 26 条", "别把其中一个"} {
		if !strings.Contains(got, want) {
			t.Fatalf("缺 %q：%s", want, got)
		}
	}
}

func ptrString(v string) *string { return &v }
