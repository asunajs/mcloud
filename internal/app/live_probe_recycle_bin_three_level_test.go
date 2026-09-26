package app

// 活体探针：**三层**子树删最外层 ⇒ `trashedDirStatistics.subDirCount` 数的是哪一档（第廿九趟）。
//
// # 这一趟只答一问（外加两格顺带）
//
// **主问（唯一动笔的理由）**：第廿八趟 capture §11 判据二剩下的那一格 ——
// `subDirCount` 到底是"**直接子级**里几枚目录"还是"**整棵子树**里几枚目录"。
// 那一趟分不开，是因为它的两档在 `subDirCount` 上**同值**：
// 直接子级 = 1 枚目录、子树也只有 1 枚目录（outer→inner，inner 里是文件）⇒ 读到 `1` 说明不了口径。
// 本趟把形状改成 **outer→A→B** 三层 ⇒ 直接子级 = **1** 枚目录，子树 = **2** 枚目录，
// 两档在 `subDirCount` 上**必然不同数** ⇒ 一个读数就能分开。
//
// 顺带格①（同一发读的，不额外开枪）：`subFileCount` 与 `totalSize` 的口径 ——
// 本趟直接子级 = 1 枚文件 / 64 字节，子树 = 3 枚文件 / 192 字节 ⇒ 与上一格同样被设计成不同数。
// ⚠ 但这两格在第廿八趟已经证明"**填不填本身就不确定**"（同形状三趟两档）⇒
// 它们只作为"这一格又添一个读数"登记，**不**当这一趟的成败判据（成败只看 `subDirCount`）。
//
// 顺带格②：判据一（"整棵子树压成一条"）在**第三个深度**上是否仍成立 —— 第廿七趟一层、
// 第廿八趟两层（三趟），本趟三层 ⇒ 同一族再加一个深度点。这一格是副产品，不是主问。
//
// # 为什么不做成"幂等 + 三层"一案
//
// 第廿六/廿七/廿八趟立下的口径：**一次动两问 ⇒ 两问都收不到干净答案**（坑表 #194 同族）。
// "对同一枚 `fileId` 重复打 `batchTrash`"要**第二发写**、判据是"第二发回什么 + 条数变不变"，
// 与本趟"同一格读哪个口径"不是一回事 ⇒ 仍留单独一趟（R7 剩下的那一格，未做）。
//
// # 写面与残留
//
// 造 6 枚（3 目录 + 3 文件）、删 1 枚（最外层）⇒ 回收站里的净增按判据一看（预期 1 条）。
// 中途造失败 ⇒ 把已造出的**最外层**那一枚 batchTrash 掉收场（与第廿七/廿八趟同一档处置：
// 本仓没有"彻底删除"口，收场只能进回收站）。这一笔并入 **R37ⓒ** 那批待拍板残留。
//
// # 凭据与隐私边界（沿用第廿七/廿八趟那一族，一条不放宽）
//
//   - 响应**只念形状**走 [liveProbeShapeValue]；逐条读数只收带 `asign-` 前缀的（我们自己造的名），
//     用户条目**只计数、不念名字与字段值**；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；解析器那一发**不打印** `FileID`
//     （由 [TestLiveProbeParsedEntryWordHidesFileID] 钉住）；
//   - 翻页游标的**值不打印**，只念"有没有、长度多少"；
//   - `thumbnailUrls` 里那枚读取入口与预签名 `uploadUrl` 同档 ⇒ 本层根本不读它。
//
// # 闸门与"两把尺"
//
// 翻页沿用 [liveProbeBinMaxPages] / [liveProbeBinMaxItems]；回读最多 [liveProbeNestMaxPasses] 遍，
// 遍与遍之间**只等、一枪不发**。每遍仍是"裸读 + 紧接着单发解析器"**两枪** ⇒
// 第廿八趟那条口径（坑表 #199："第几遍"要指名是哪把尺）原样沿用，两把尺各记各的。

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
	// 六枚名字的中段（都可打印：整名带 `asign-` 前缀 ⇒ 是我们自己造的）。
	liveProbeL3DirInfix   = "l3dir-"   // 最外层：**只有它进 batchTrash**
	liveProbeL3KidInfix   = "l3kid-"   // outer 里的 A
	liveProbeL3GrandInfix = "l3grand-" // A 里的 B（第三层目录 ⇒ 让两档的目录数不同）
	liveProbeL3TopInfix   = "l3top-"   // outer 里的文件（直接子级那一枚）
	liveProbeL3MidInfix   = "l3mid-"   // A 里的文件
	liveProbeL3DeepInfix  = "l3deep-"  // B 里的文件（第三层）

	// 下面六格是**设计值**（本趟刻意造成这个形状）：直接子级与子树在三格上全都不同数，
	// 这样一个读数就能分开口径。它们不是上游读数 —— 判据是 ④ 念出来的那个数。
	liveProbeL3DirectDirs   int64 = 1
	liveProbeL3SubtreeDirs  int64 = 2
	liveProbeL3DirectFiles  int64 = 1
	liveProbeL3SubtreeFiles int64 = 3
	// 每枚随机图 64 字节（`UploadRandom` 的形状，见第廿六趟那一族的成文）。
	liveProbeL3DirectBytes  int64 = 64
	liveProbeL3SubtreeBytes int64 = 192
)

func TestLiveProbeRecycleBinThreeLevel(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器（本探针要先造六枚再删最外层）：%s", binErr(sink, uerr))
	}

	l3name := func(infix, ext string) string {
		return CloudUploadNamePrefix + infix + randomHex(uploader.Rng, cloudRandomNameHexLen) + ext
	}
	// ⚠ 名字只在这里造一次（建、删、回读三处比的是同一批整名 —— 第廿七趟 #196 那条教训）。
	outerName := l3name(liveProbeL3DirInfix, "")
	kidName := l3name(liveProbeL3KidInfix, "")
	grandName := l3name(liveProbeL3GrandInfix, "")
	topName := l3name(liveProbeL3TopInfix, ".png")
	midName := l3name(liveProbeL3MidInfix, ".png")
	deepName := l3name(liveProbeL3DeepInfix, ".png")

	mkDir := func(at, name, what string) (string, bool) {
		created, cerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, at, name)
		switch {
		case cerr != nil:
			sink.say("① 造%s（%q）失败：%s", what, name, binErr(sink, cerr))
		case created.Exist:
			sink.say("① 上游说 %q 同名已存在 ⇒ 当场收手（那一枚属于用户本来就有的目录，不删）", name)
		case created.FileID == nil:
			sink.say("① %s 回了 OK 但没有 fileId ⇒ 没有可删的对象", what)
		default:
			sink.see(*created.FileID)
			return *created.FileID, true
		}
		return "", false
	}
	mkFile := func(at, name, what string) (string, bool) {
		uploaded, uerr2 := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: at, Name: name})
		if uerr2 != nil || uploaded.FileID == "" {
			sink.say("① 往%s放文件（%q）没成：%s", what, name, binErr(sink, uerr2))
			return "", false
		}
		sink.see(uploaded.FileID)
		return uploaded.FileID, true
	}

	// ── ① 造六枚：outer / A / B / 三层各一枚文件 ──
	outerID, ok := mkDir("/", outerName, "最外层目录")
	if !ok {
		sink.say("① 最外层都没造出来 ⇒ 下面全部不发（没有我方认得出的对象，列回收站只会读到用户自己的条目）")
		recorder.sayTally()
		return
	}
	// bail：中途造失败就把已造出的**最外层**那一枚删掉 —— 一枪带走半棵树
	// （第廿六/廿七/廿八趟同一档处置；逐枚删要打六枪，还会顺手污染"重复删"那一格）。
	bail := func(what string) {
		sink.say("  收场：把最外层 %q 那一枚 batchTrash 掉（不在用户网盘里留半棵树；"+
			"它会进回收站，本仓无'彻底删除'口）：%s", outerName, what)
		if sent, derr := uploader.DeleteCloudFiles(ctx, []string{outerID}); derr != nil || !sent {
			sink.say("  收场那一枪也没打通（发过=%v）：%s", sent, binErr(sink, derr))
		}
		recorder.sayTally()
	}
	kidID, ok := mkDir(outerID, kidName, "第二层目录")
	if !ok {
		bail("第二层目录没造出来")
		return
	}
	grandID, ok := mkDir(kidID, grandName, "第三层目录")
	if !ok {
		// ⚠ 这一档不能继续：少了第三层 ⇒ 直接子级与子树的目录数**又相同了**（都 1），
		// 主问当场失效（与第廿八趟"少了 sib 文件就分不开两档"同族）。
		bail("第三层目录没造出来 ⇒ 两档的目录数同值，主问没判据")
		return
	}
	if _, ok = mkFile(outerID, topName, "最外层"); !ok {
		bail("直接子级那枚文件没传上")
		return
	}
	if _, ok = mkFile(kidID, midName, "第二层"); !ok {
		bail("第二层那枚文件没传上")
		return
	}
	if _, ok = mkFile(grandID, deepName, "第三层"); !ok {
		bail("第三层那枚文件没传上")
		return
	}
	sink.say("① 造好了六枚：目录 %q / 里面 %q / 再里面 %q，文件 %q（第一层）/ %q（第二层）/ %q（第三层）",
		outerName, kidName, grandName, topName, midName, deepName)
	sink.say("① 这一棵的形状：子树 = %d 枚目录 + %d 枚文件（约 %d 字节），"+
		"**直接**子级 = %d 枚目录 + %d 枚文件（约 %d 字节）⇒ 两档在**三格上全都不同数**，"+
		"一个读数就能分开口径",
		liveProbeL3SubtreeDirs, liveProbeL3SubtreeFiles, liveProbeL3SubtreeBytes,
		liveProbeL3DirectDirs, liveProbeL3DirectFiles, liveProbeL3DirectBytes)

	// ── ② 只打一枪：删**最外层**那一枚（其余五枚都不点名）──
	sent, derr := uploader.DeleteCloudFiles(ctx, []string{outerID})
	if derr != nil || !sent {
		sink.say("② batchTrash(最外层) 没成（发过=%v）：%s ⇒ 这一趟不回答任何一问",
			sent, binErr(sink, derr))
		recorder.sayTally()
		return
	}
	sink.say("② batchTrash(最外层)：上游认了（**只这一枚**，其余五枚都没点名）")

	// ── ③ 回读：最多 liveProbeNestMaxPasses 遍，每遍裸读 + 单发解析器 ──
	wanted := []string{outerName, kidName, grandName, topName, midName, deepName}
	exactAt := func() map[string]bool {
		m := make(map[string]bool, len(wanted))
		for _, n := range wanted {
			m[n] = false
		}
		return m
	}
	var (
		last          *liveProbeBinScan
		lastPages     int
		foundAtNaked  int
		foundAtParsed int
		passCounts    []int
		passDirs      []string
		union         = exactAt()
	)
	nameSet := make(map[string]bool, len(wanted))
	for _, n := range wanted {
		nameSet[n] = true
	}
	for pass := 1; pass <= liveProbeNestMaxPasses; pass++ {
		if pass > 1 {
			sink.say("③ 第 %d 遍前先等 %s，中间**一枪都不发**（分清「还没进」与「真没进」，"+
				"出处：第廿七趟实测的秒级不回）", pass, liveProbeBinWait)
			time.Sleep(liveProbeBinWait)
		}
		scan := &liveProbeBinScan{exact: exactAt()} // mine 留 nil：本趟不按中段认（#196）
		pages, sweepOK := liveProbeBinSweep(sink, run, uploader, "", "③ 第 "+strconv.Itoa(pass)+" 遍", scan)
		if !sweepOK {
			sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是读数）⇒ 不判")
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
		read := liveProbeNestStatsAt(ctx, sink, run, uploader, outerName, nameSet, pass)
		passDirs = append(passDirs, liveProbeL3DirToken(read))
		if word := liveProbeNestPassDiffWord(pass, scan.items, read.items); word != "" {
			sink.say("③ %s", word)
		}
		if read.outerSeen {
			if foundAtParsed == 0 {
				foundAtParsed = pass
			}
			union[outerName] = true // 两把尺各记各的，但"出现过"这件事取并集（#199）
		}
		if !read.ok {
			break
		}
	}
	if foundAtNaked == 0 && foundAtParsed == 0 {
		sink.say("③ 翻了 %d 遍（每遍 %v 条，闸门 %d 页 / 最多 %d 条）都没在最外层那一枚上读到 ⇒ "+
			"下面几问一律不判（'没读到'有三种读法：还没进索引 / 被闸门挡住 / 这一口回的不是回收站条目）",
			len(passCounts), passCounts, liveProbeBinMaxPages, liveProbeBinMaxItems)
	} else {
		sink.say("③ 最外层那一条**第一次**出现在：裸读第 %s 遍 / 解析器第 %s 遍（共读 %d 遍，"+
			"每遍两枪各记各的；不一致时上面有单行点名，出处：第廿八趟 #199）；末遍页数 %d、%s",
			liveProbeNestPassWord(foundAtNaked), liveProbeNestPassWord(foundAtParsed),
			len(passCounts), lastPages, liveProbeTruncWord(last.truncated))
	}
	sink.say("③ 每遍的 `subDirCount`（%d 遍）：%s", len(passDirs), strings.Join(passDirs, " | "))
	sink.say("③ ⇒ 口径判据只认 `subDirCount`：第廿八趟已证明文件那两格**填不填本身不确定**" +
		"（同形状三趟两档）⇒ 本趟不因它们没填而判失败。")

	// ── ④ 主问：`subDirCount` 数的是直接子级还是整棵子树 ──
	// 取**最后一遍**那份（第廿八趟口径："当场那份"可能就是没填完的那份）。
	lastDir := "一遍都没读到"
	if len(passDirs) > 0 {
		lastDir = passDirs[len(passDirs)-1]
	}
	sink.say("④ 主问：末遍 `subDirCount` 短码 %q ⇒ %s", lastDir, liveProbeL3DirWord(lastDir))

	// ── ⑤ 顺带格②：三层在回收站里占几条（副产品，不是本趟的动笔理由）──
	shapeWord, shapeCount := liveProbeL3ShapeWord(last.exact)
	unionWord, unionCount := liveProbeL3ShapeWord(union)
	sink.say("⑤ 六枚整名的命中（末遍）：目录(最外层)=%v 第二层目录=%v 第三层目录=%v "+
		"文件(一/二/三层)=%v %v %v ⇒ 命中 %d 条",
		last.exact[outerName], last.exact[kidName], last.exact[grandName],
		last.exact[topName], last.exact[midName], last.exact[deepName], shapeCount)
	sink.say("⑤ 各遍**并集**：命中 %d 条 ⇒ %s", unionCount, unionWord)
	sink.say("⑤ ⇒ %s", shapeWord)
	if shapeCount != unionCount {
		sink.say("⑤ ⚠ 末遍与并集不一致 ⇒ 有条目**在趟内消失过**，命中数以**并集**为准，" +
			"且'条数的差'不能当'本趟净增几条'的判据（第廿八趟 capture §9 那句口径）")
	}
	sink.say("⑤ 反向核对（这一格**不靠**上面那句推断）：本趟六枚的中段（%s / %s / %s / %s / %s / %s）"+
		"与本仓任何更早一趟都不同 ⇒ 命中只可能来自**本趟现造**的那几枚（#196）",
		liveProbeL3DirInfix, liveProbeL3KidInfix, liveProbeL3GrandInfix,
		liveProbeL3TopInfix, liveProbeL3MidInfix, liveProbeL3DeepInfix)

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪；写侧只有那一次 batchTrash(最外层) ⇒ 残留 = 用户回收站里本趟那棵子树"+
		"（净增几条就看 ⑤ 那个数），本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeBinPath, recorder.countFor(liveProbeBinPath))
}

// liveProbeL3DirToken 从解析器那一发的三格短码里只取 `subDirCount` 那一格。
//
// 为什么单独一格（不复用 [liveProbeNestPassRead].token 那个三格短码）：本趟的判据只有
// `subDirCount` 一格，把三格糊成一枚短码会让"文件那两格没填"看起来像主问没答上
// （第廿八趟正是靠把三格折成一枚短码才先误判过一次口径，坑表 #200）。
// 三格短码的四档非数值 token（"没打通" / "缺键" / "nil" / "缺席"）原样透传，
// 因为它们在判据里各是三件不同的事（见 [liveProbeNestStatsWord] 那段"不折档"的注释）。
func liveProbeL3DirToken(read liveProbeNestPassRead) string {
	switch read.token {
	case "没打通", "缺键", "nil", "缺席":
		return read.token
	}
	first, _, found := strings.Cut(read.token, "/")
	if !found {
		return read.token // 形状不是"a/b/c"：不猜，原样交回（调用方那支会念"不是一个数"）
	}
	return first
}

// liveProbeL3DirWord 把 `subDirCount` 的短码折成一句口径读数。
//
// 分档只按**本趟的形状**：设计值 直接=1 / 子树=2（见抬头）。两档之外的数（3、负数…）
// 走"另一档"那一条，**不硬贴**；`0` 走"没答"，因为第廿七趟那档 `0/0/0` 是"这一格在刚删的
// 目录上根本不填"，把它读成"子树里没有目录"是反的（AGENTS §1.7）。
//
// 反向改坏它：把 `== liveProbeL3DirectDirs` 改成 `<=` ⇒ [TestLiveProbeL3DirWord] 红。
func liveProbeL3DirWord(token string) string {
	// 每一档都带一枚 `[口径=…]` 短标：判据句子长，读者（和测试）要能在 stdout 里
	// 一眼认出"这一趟到底答没答"。反向改坏它：把两档的短标对调 ⇒ [TestLiveProbeL3DirWord] 红。
	switch token {
	case "没打通", "缺键", "nil", "缺席", "一遍都没读到":
		return "[口径=没读数] 这一遍没读到可用的 `subDirCount`（短码 " + token +
			"）⇒ 主问不判，上面那行是读数"
	}
	n, err := strconv.ParseInt(token, 10, 64)
	if err != nil {
		return "[口径=不判] 短码不是一个数（" + token + "）⇒ 这一档本趟没见过，形状可能变了"
	}
	switch n {
	case liveProbeL3DirectDirs:
		return "⭐ [口径=直接子级] `subDirCount` = " + strconv.FormatInt(n, 10) +
			" = 本趟的直接子级目录数（子树是 " + strconv.FormatInt(liveProbeL3SubtreeDirs, 10) +
			"，两档不同数才判得出来）⇒ 这一格数的是**直接子级**。" +
			"⚠ 这是一趟的读数不是证明：要钉死口径得换深度再对一次（本趟就是第廿八趟那档的对立面）"
	case liveProbeL3SubtreeDirs:
		return "⭐ [口径=整棵子树] `subDirCount` = " + strconv.FormatInt(n, 10) +
			" = 本趟的子树目录数（直接子级是 " + strconv.FormatInt(liveProbeL3DirectDirs, 10) +
			"）⇒ 这一格数的是**整棵子树**"
	case 0:
		return "[口径=未答] 读到 0 ⇒ 与第廿七趟那档「刚删的目录上这一格根本不填」同形。" +
			"⚠ 不许读成「子树里没有目录」：本趟子树里有 " +
			strconv.FormatInt(liveProbeL3SubtreeDirs, 10) + " 枚，这一档只说明**这一格没填**"
	default:
		return fmt.Sprintf("[口径=另一档] 读到 %d：既不是直接子级（%d）也不是子树（%d），也不是 0"+
			" ⇒ 不硬贴结论，这一格要人对着逐条读数再看一遍", n, liveProbeL3DirectDirs, liveProbeL3SubtreeDirs)
	}
}

// liveProbeL3ShapeWord 把"六枚整名命中了哪几枚"折成一句档位读数（顺带格②：第三个深度）。
//
// 与 [liveProbeNestShapeWord] 分开写而不是复用：那一支的档是按**它的四枚**设计的
// （`nestdir-` / `nestkid-`），本趟六枚里有第三层目录与三枚文件 ⇒ 命中集合的语义不同。
// 硬套会把"目录逐层各一条"那一档念错。
func liveProbeL3ShapeWord(exact map[string]bool) (string, int) {
	hits := 0
	for _, hit := range exact {
		if hit {
			hits++
		}
	}
	if hits == 0 {
		return "一条都没命中 ⇒ 这一档本趟不判（见 ③ 那行：先确认不是延迟/闸门）", 0
	}
	return liveProbeL3ShapeByHits(exact, hits), hits
}

func liveProbeL3ShapeByHits(exact map[string]bool, hits int) string {
	// 一律按**中段**判"是哪一枚"：整名是现造的，中段才是这一族稳定的那一格（#196 同口径）。
	hit := func(infix string) bool { return exactHas(exact, infix) }
	switch {
	case hits == 1 && hit(liveProbeL3DirInfix):
		return "⭐ 只有**最外层**那一条 ⇒ **三层**子树仍然压成**一条**（第二、三层目录与三枚文件都不单列）" +
			"—— 与第廿七趟（一层）、第廿八趟（两层，三趟）同一档 ⇒ '只留一条'到第三个深度还没有反例"
	case hits == 2 && hit(liveProbeL3DirInfix) && hit(liveProbeL3KidInfix):
		return "最外层 + 第二层目录**各一条**、第三层与文件都不在 ⇒ 目录**逐层**各记一条（到第二层为止）" +
			"，这一档**推翻**上面那句'只留一条'"
	case hit(liveProbeL3TopInfix) || hit(liveProbeL3MidInfix) || hit(liveProbeL3DeepInfix):
		return "有**文件**单列 ⇒ **推翻**第廿七趟那句'子文件不单列'，那一档要按这一趟的命中集合改写"
	case hit(liveProbeL3KidInfix) || hit(liveProbeL3GrandInfix):
		return "命中的是**内层**目录而不是最外层那一条（hits=" + strconv.Itoa(hits) +
			"）⇒ 与'删最外层点名那一枚'的预期形状不同，不硬贴结论，要对着逐条读数看"
	default:
		return "命中集合落不进上面任何一档（hits=" + strconv.Itoa(hits) + "）⇒ 不硬贴结论"
	}
}

// TestLiveProbeRecycleBinThreeLevelReread ⭐ **只读**复跑：把"这三枚目录在回收站里是什么形状"
// 变成一支不写任何东西、随时能再读一遍的探针。
//
// 为什么要有它：① 主问那格（`subDirCount`）如果本趟读到的是"没填"那一档，
// 隔几十分钟 / 隔天再读同一枚**就有读数了**，而写探针每次现造新名字 ⇒ 拿不回旧那一枚；
// ② 与第廿八趟那条一致：这一族**每次复跑写探针都会新增残留**，只读口才是免费的。
//
// 认法只能是**中段**（`l3dir-` 那六串）：整名是写探针现造的，复跑拿不到。
// 这与 #196 不冲突 —— 那条讲的是"当场 vs 等后两遍对照必须用整名"。
// ⚠ 命中数会把**更早趟同中段的残留**一起算进来 ⇒ 这一趟念的是"至少几条"。
func TestLiveProbeRecycleBinThreeLevelReread(t *testing.T) {
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

	infixes := []string{liveProbeL3DirInfix, liveProbeL3KidInfix, liveProbeL3GrandInfix,
		liveProbeL3TopInfix, liveProbeL3MidInfix, liveProbeL3DeepInfix}
	scan := &liveProbeBinScan{mine: make(map[string]bool, len(infixes))}
	for _, infix := range infixes {
		scan.mine[infix] = false
	}
	pages, ok := liveProbeBinSweep(sink, run, uploader, "", "① 只读复跑", scan)
	if !ok {
		sink.say("⇒ 这一口**今天打不通**（上面那行错误原文就是读数）")
		recorder.sayTally()
		return
	}
	sorted := append([]string(nil), infixes...)
	sort.Strings(sorted)
	for _, infix := range sorted {
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
			if entry.Name == nil || !strings.Contains(*entry.Name, CloudUploadNamePrefix) {
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

// TestLiveProbeL3DirWord 钉住主问的判据映射（AGENTS §1.3：改坏它要说得出哪条红）。
//
// 三档各自挡一种偷懒：把 1 读成"子树"（口径反了）、把 0 读成"没有目录"（方向反了，
// 它只是"没填"）、把"这一发根本没读到"折成一个数（形状变了会被念成正常读数）。
// 断言只认 `[口径=…]` 那枚短标：判据句子会互相提到对方那一档作对照，拿关键词判会误伤。
func TestLiveProbeL3DirWord(t *testing.T) {
	cases := []struct {
		token string
		want  string // 必须出现的那枚短标
		notIn []string
	}{
		{"1", "[口径=直接子级]", []string{"[口径=整棵子树]", "[口径=未答]"}},
		{"2", "[口径=整棵子树]", []string{"[口径=直接子级]", "[口径=未答]"}},
		{"0", "[口径=未答]", []string{"[口径=直接子级]", "[口径=整棵子树]"}},
		{"3", "[口径=另一档]", []string{"[口径=直接子级]", "[口径=整棵子树]", "[口径=未答]"}},
		{"-1", "[口径=另一档]", []string{"[口径=未答]"}},
		{"nil", "[口径=没读数]", []string{"[口径=未答]", "[口径=直接子级]"}},
		{"没打通", "[口径=没读数]", []string{"[口径=直接子级]"}},
		{"缺键", "[口径=没读数]", []string{"[口径=直接子级]"}},
		{"缺席", "[口径=没读数]", []string{"[口径=直接子级]"}},
		{"", "[口径=不判]", []string{"[口径=没读数]"}},
	}
	for _, c := range cases {
		word := liveProbeL3DirWord(c.token)
		if !strings.Contains(word, c.want) {
			t.Errorf("短码 %q 的读数里没有 %q：%s", c.token, c.want, word)
		}
		for _, bad := range c.notIn {
			if strings.Contains(word, bad) {
				t.Errorf("短码 %q 的读数里不该出现 %q：%s", c.token, bad, word)
			}
		}
	}
}

// TestLiveProbeL3ShapeByHits 钉住"命中集合 → 结论"那四档，彼此互斥。
func TestLiveProbeL3ShapeByHits(t *testing.T) {
	mk := func(names ...string) map[string]bool {
		m := map[string]bool{
			"asign-l3dir-a": false, "asign-l3kid-b": false, "asign-l3grand-c": false,
			"asign-l3top-d.png": false, "asign-l3mid-e.png": false, "asign-l3deep-f.png": false,
		}
		for _, n := range names {
			m[n] = true
		}
		return m
	}
	if w := liveProbeL3ShapeByHits(mk("asign-l3dir-a"), 1); !strings.Contains(w, "三层") ||
		strings.Contains(w, "推翻") {
		t.Errorf("只命中最外层那一档念错了：%s", w)
	}
	if w := liveProbeL3ShapeByHits(mk("asign-l3dir-a", "asign-l3kid-b"), 2); !strings.Contains(w, "推翻") {
		t.Errorf("目录逐层各一条那一档没点明它推翻上一档：%s", w)
	}
	if w := liveProbeL3ShapeByHits(mk("asign-l3top-d.png"), 1); !strings.Contains(w, "文件") {
		t.Errorf("文件单列那一档没认出来：%s", w)
	}
	if w := liveProbeL3ShapeByHits(mk("asign-l3grand-c"), 1); !strings.Contains(w, "不硬贴") {
		t.Errorf("只命中内层目录那一档该走'不硬贴'：%s", w)
	}
}
