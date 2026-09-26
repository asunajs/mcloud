package app

// 活体探针：`batchTrash` 打在**非空目录**上会怎样（**R7** 的那一问）。
//
// # 这一格为什么现在才答得出
//
// R7 从登记起就卡在同一个前置上："同一测试号上：建目录 → 往里放一个 → 删它"。
// 三件里缺两件：
//   - **建目录**这一口本仓今天才有构造器（[upstream.CloudFolderCreateRequest] /
//     [upstream.AppCloudFolderCreate]）。它**有实拍**（`test/fixtures/http/3d.json` 两条
//     `type:"folder"`），但从没在本仓真打过 —— 唯一的消费者就是本探针。
//   - **往里放一个**需要"上传时带非根 `parentFileId`"。生产那三条玩法（106 / 522 / 1014）
//     全都传 `"\""` 那一档（根目录），所以这一档今天**零生产调用点** ⇒ 本探针第一步
//     就是在问"上游认不认我们把文件放进目录"，它不成，R7 就无从问起。
//
// `batchTrash` 本身已经在生产链尾跑通过几十次（R27 / R28），但每一次删的都是**文件**，
// 从来没有删过目录。所以 R7 剩下的那一半不是"这一口通不通"，是"它对非空目录通不通"。
//
// # 五段读数各答哪一问（顺序不可换，换了两问都收不到）
//
//  1. **传进目录成不成立**：列那一枚目录，看我们传进去的那个文件名在不在里面。
//     不在 ⇒ 后面全是"删一个空目录"，R7 这一格**没被走到**（探针当场就把这句念出来）；
//  2. **删目录那一枪上游认不认**：`batchTrash([dirID])` 的结果原文；
//  3. **根目录那一页里目录还在不在**（"没删掉"这一档）；
//  4. **列那个目录还回什么**：回空 = "目录空了 ⇒ 内容跟着没了"；回错误 = "目录本身已经
//     不可达"；仍然列得出那个文件 = "**连目录带内容都没删掉**"，那一档要接进清理判定；
//  5. **那个文件出现在根目录了吗**（"内容被退回到根"这一档 —— 旧实现的清理链是按
//     fileId 记的，所以这一档在真实产品里的表现是"清理报成功、用户网盘里多一个文件"）。
//
// 三档**互斥且不穷尽**：还可能"目录删掉了、文件也一起没了"（回收站语义按整棵子树收），
// 那是本探针最希望读到的那一档，因为现网清理链照它落就是安全的。
//
// # 四条边界
//
//   - **`exist:true` 当场停手，不删**：上游说"同名已存在"时交回的那枚 fileId 属于
//     **用户本来就有**的那个目录，删它等于拿我们的清理链去动用户的内容。
//     随机名撞上同名几乎不可能，但这一条不给例外（与 `cloud_upload.go` 文件头
//     "上游说已存在的那一枚从不进清理名单"同一口径）。
//   - **收尾只删我们造的那两枚**（目录 + 里面那个文件），且**都按 `asign-` 前缀命名** ⇒
//     日志里念全名不越界；真实 fileId 一律 `sink.see` 登记后**只念长度**（与网盘其它探针同一纪律）。
//   - **不重试、不补删**：上面任何一步失败都只是"这一格没拿到读数"，探针不据此改代码；
//     判读口径写在最后那行 `读数怎么用`。
//   - **是写**（往测试号网盘造一个目录和一个文件，并把它们移进回收站）⇒ 用户口径明确
//     "包括上传网盘文件"都要实测（2026-09-25），所以不设额外开关，但那四个 env 齐了才动
//     （默认 `t.Skipf`）。回收站那一族今天没有"彻底删除"接口 ⇒ 留下的东西在回收站里，
//     与生产链尾清理的处置面**完全相同**，不多一分。

import (
	"context"
	"testing"
	"time"
)

// 本探针盯着的两条路径：只为最后那句"这一族今天发了几枪"的账，不是判据。
const (
	liveProbeBatchTrashPath   = "/hcy/recyclebin/batchTrash"
	liveProbeFileCreatePath   = "/hcy/file/create"
	liveProbeTrashFolderInfix = "trashdir-"
	liveProbeTrashChildInfix  = "trashkid-"
	// liveProbeTrashSettleWait 是"删除枪打完之后、什么枪都不补，让回读自己稳定"的那一段。
	//
	// 为什么是 8 秒：本仓已有的两档"上游自己需要时间"的读数里，最短的一档（任务拓展那一条
	// 的备份重读）用的是 20 秒，而网盘侧的删除在生产链尾是**打完就往下走**（没有等待）⇒
	// 这一格只需要"比一次往返长"就够把滞后分开，不必照抄 20 秒（照抄会让探针慢三倍）。
	// ⚠ 它**不是判据**：判据是"等之前 / 等之后两遍读数有没有变"，改这个数只会挪动
	// "能不能观察到变化"的灵敏度。
	liveProbeTrashSettleWait = 8 * time.Second
)

// liveProbeDirPage 列**任意一个目录**一页，交回"名字 → fileId 长度"与"这一发成没成"。
//
// 与根目录那一发（`liveProbeListing`）分开写是因为问的格子不同：那一发比**差集**，
// 所以要名字集合；这一发问的是"某两枚具名文件在不在这**一个**目录里"，
// 于是名字直接当键用，且**不**返回 fileId（值不进成文；要拿 id 用调用方自己记下的那一份）。
func liveProbeDirPage(
	what string, run TaskMatrixRun, opening SignInOpening, sink *liveProbeSink, parent string,
) (map[string]bool, bool) {
	page, lerr := run.Upstream.AppCloudFileList(context.Background(), opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(), parent,
		liveProbeFileListPageSize)
	if lerr != nil {
		sink.say("%s：这一页**没读到**（%s）", what, sink.mask(lerr.Error()))
		return nil, false
	}
	names := make(map[string]bool, len(page.Items))
	for _, file := range page.Items {
		sink.see(file.FileID) // 真实 fileId 不进成文（连长度都不在这里念）
		names[file.Name] = true
	}
	hasMore := page.NextPageCursor != nil && *page.NextPageCursor != ""
	sink.say("%s：%d 项，下一页游标=%s", what, len(names), liveProbeCursorWord(hasMore))
	return names, true
}

func TestLiveProbeFolderTrash(t *testing.T) {
	run, account, sink, recorder := liveProbeWorldRecording(t)
	ctx := context.Background()

	opening, perr := run.SignInWorld.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	// 上传器一次装配、三处复用（建目录要 deviceId、传文件要随机源、删要同一串凭据）。
	// 刻意走 [taskExecutor.cloudUploader] 而不是自己拼：它同时把"取不到 deviceId / 没有随机源
	// 就不发这一族"这两条拦截带过来，探针不该比生产更宽容。
	executor := newTaskExecutor(run, opening, account)
	uploader, uerr := executor.cloudUploader(ctx)
	if uerr != nil {
		t.Fatalf("装不出网盘上传器（R7 的前置三件事一件都做不了）：%s", sink.mask(uerr.Error()))
	}

	// ⚠ 名字只在这里造**一次**：下面建目录发的是它、回读比的是它、根目录那一页查的也是它。
	// 每一句现造一个 hex 会让"删掉的那一枚"和"建出来的那一枚"根本不是同一个名字（读数作废）。
	dirName := CloudUploadNamePrefix + liveProbeTrashFolderInfix + randomHex(uploader.Rng, cloudRandomNameHexLen)
	childName := CloudUploadNamePrefix + liveProbeTrashChildInfix + randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"

	// ── 前置读数：这个目录名在根目录里**本来**有没有 ──
	// 不为判据，只为"建完之后出现的那一枚是我们造的"这一句有出处。
	rootBefore, rootBeforeOK := liveProbeDirPage("建目录前的根目录", run, opening, sink, "/")

	// ── ① 建目录（写）──
	created, cerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, "/", dirName)
	if cerr != nil {
		sink.say("① 建目录**失败**：%s ⇒ R7 这一格今天拿不到（后面四段全部不发，避免把"+
			"「删不掉」读成「删了一个空目录」）", sink.mask(cerr.Error()))
		recorder.sayTally()
		return
	}
	if created.FileID == nil {
		sink.say("① 建目录回了 OK 但**没有 fileId** ⇒ 没有可删的对象，这一格没拿到（现场：exist=%v）",
			created.Exist)
		recorder.sayTally()
		return
	}
	dirID := *created.FileID
	sink.see(dirID)
	if created.Exist {
		// ⚠ 上面边界第一条：这一枚属于用户本来就有的目录，**不删**。
		sink.say("① 上游说同名**已存在**（exist=true，fileId 长度 %d）⇒ 当场收手："+
			"这一枚不是我们造的，拿它做删除实验等于动用户内容。换名重跑一次即可。", len(dirID))
		recorder.sayTally()
		return
	}
	sink.say("① 建目录成功：名字 %q，fileId 长度 %d（存在标记=%v，uploadId 是否缺=%v）",
		dirName, len(dirID), created.Exist, created.UploadID == nil)

	// ── ② 往里放一个文件（写；这一档今天**零生产调用点**）──
	receipt, uerr2 := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: dirID, Name: childName})
	if uerr2 != nil {
		sink.say("② 往目录里传文件**失败**：%s ⇒ R7 退化成「删空目录」，那一格不是本探针要问的",
			sink.mask(uerr2.Error()))
		recorder.sayTally()
		return
	}
	sink.see(receipt.FileID)
	sink.say("② 传文件进目录：%v，名字 %q fileId 长度 %d",
		receipt.Outcome, receipt.Name, len(receipt.FileID))

	// ── 前置成立性回读：那枚文件真在目录里吗（R7 的"往里放一个"到底放没放进去）──
	insideFirst, insideOK := liveProbeDirPage("删之前列那个目录", run, opening, sink, dirID)
	if !insideOK || !insideFirst[childName] {
		sink.say("⚠ 那一枚文件**没有出现在这个目录的这一页里** ⇒ 「非空」这个前置不成立，" +
			"下面那删只等于删空目录（R7 那一格仍未走到）。两种可能都要记：② 那一枪其实落在了别处、" +
			"或者列目录时上游把新文件排在第二页（看上面有没有「下一页游标=有」）。")
	}

	// ── ③ 删目录（本探针的主判据）──
	sent, derr := uploader.DeleteCloudFiles(ctx, []string{dirID})
	switch {
	case derr != nil:
		sink.say("③ batchTrash(目录) 发过=%v ⇒ **上游不认这一枪**：%s", sent, sink.mask(derr.Error()))
	case sent:
		sink.say("③ batchTrash(目录) 发过=true，上游认了（fileId 长度 %d，只这一枚）", len(dirID))
	default:
		sink.say("③ 一枪没发（名单为空）⇒ 本探针的调用面有问题")
	}

	// ── ④⑤ 删完之后的三格回读 ──
	rootAfter, rootAfterOK := liveProbeDirPage("删目录后的根目录", run, opening, sink, "/")
	insideAfter, insideAfterOK := liveProbeDirPage("删目录后列那个目录", run, opening, sink, dirID)

	switch {
	case !rootBeforeOK || !rootAfterOK:
		sink.say("④ 根目录两遍里有一遍没读出来 ⇒ 「目录还在不在」这一格没拿到")
	default:
		sink.say("④ 目录名在根目录：删前=%v 删后=%v（%s）",
			rootBefore[dirName], rootAfter[dirName], liveProbeStillWord(rootAfter[dirName]))
	}
	switch {
	case !insideAfterOK:
		sink.say("④ 删后列那个目录**读不出来** ⇒ 「目录已不可达」这一档（与「空了」不是一件事，" +
			"上一行的错误原文就是这一档的现场）")
	case insideAfter[childName]:
		sink.say("④ 删后列那个目录：%q **还在里面** ⇒ 整棵子树都没被清掉，现网清理链删目录不可靠",
			childName)
	default:
		sink.say("④ 删后列那个目录：读得出来且那枚文件不在里面")
	}
	if rootAfterOK {
		if rootAfter[childName] {
			sink.say("⑤ 那枚文件**出现在根目录** ⇒ 「退回到根」这一档成立（清理链按 fileId 记，" +
				"表现为「删掉了」而用户网盘里多一个文件）")
		} else {
			sink.say("⑤ 那枚文件不在根目录这一页（它要么跟着目录进了回收站，要么在别的目录）")
		}
	}

	// ── ④′ 等几秒**只回读、不再补枪**：把"回读滞后"与"要第二枪才删掉目录"分开 ──
	//
	// 为什么单独加这一遍：2026-09-26 号一那一趟读到的正是这两种解释**共用**的那副样子
	// （`batchTrash(目录)` 认了 ⇒ 紧接着根目录那一页**仍列着**那枚目录、列它自己回 **0 项**，
	// 而把子文件再删一枪之后目录才从根目录那一页消失）。不等待、直接补第二枪，
	// 就永远分不出"目录本来就没删掉"与"删掉了但回读太早"，而这两种对**清理链**的处置不同。
	wait := run.Wait
	if wait == nil {
		wait = RealWaiter{}
	}
	if werr := wait.Wait(ctx, liveProbeTrashSettleWait); werr != nil {
		sink.say("④′ 等不住（%s）⇒ 「滞后 vs 要补一枪」这一格没拿到", sink.mask(werr.Error()))
	} else {
		settled, settledOK := liveProbeDirPage("等 "+liveProbeTrashSettleWait.String()+" 后的根目录",
			run, opening, sink, "/")
		if settledOK {
			sink.say("④′ 期间**没有补任何删除枪** ⇒ 目录名在根目录=%v、那枚文件在根目录=%v"+
				"（对照上面 ④ 那一行：由 true 变 false = 回读滞后；仍 true = 这一枪当时真没删掉目录）",
				settled[dirName], settled[childName])
			rootAfter, rootAfterOK = settled, true
		}
	}

	// ── 收尾：把里面那枚也删掉（它可能已经跟着目录没了，那一档同样要念出来）──
	if receipt.FileID != "" {
		sentC, derrC := uploader.DeleteCloudFiles(ctx, []string{receipt.FileID})
		if derrC != nil {
			sink.say("收尾删那枚文件：发过=%v ⇒ %s（若上面 ④ 显示它已跟着目录没了，"+
				"这一句就是「已经不在回收站之外了」的旁证，不是新缺陷）", sentC, sink.mask(derrC.Error()))
		} else {
			sink.say("收尾删那枚文件：发过=%v，上游认了", sentC)
		}
	}
	finalRoot, finalRootOK := liveProbeDirPage("收尾后的根目录", run, opening, sink, "/")
	if finalRootOK {
		sink.say("收尾读数：目录名在根目录=%v，文件名在根目录=%v",
			finalRoot[dirName], finalRoot[childName])
	}

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪、%s 发了 %d 枪（本探针只删自己造的那两枚 ⇒ "+
		"删的枪数应等于「建 + 传 + 删目录 + 收尾删文件」里那两次删除）",
		liveProbeFileCreatePath, recorder.countFor(liveProbeFileCreatePath),
		liveProbeBatchTrashPath, recorder.countFor(liveProbeBatchTrashPath))
	sink.say("读数怎么用（对着上面那几行逐档落 R7）：① ③ 那行「上游不认」⇒ 非空目录删不掉，" +
		"清理链**不许**接目录（现状就是不接：[TaskMatrixRunResult.CloudFileIDs] 只装文件）；" +
		"② ③ 认了且 ④ 显示那枚文件**不**在根目录 → 跟着目录一起没了 ⇒ 接目录是安全的；" +
		"③ ⑤ 显示文件退回根目录 ⇒ 接目录会「删一个、漏一个」，那才要把子文件也记进名单。" +
		"⚠ 三档都以「② 那一枚真在目录里」为前置，那一行念了警告时本趟只回答了「删空目录会怎样」。")
}

// liveProbeStillWord 把"还在不在"念成一句人话（不给结论：结论在 R7 那一行）。
func liveProbeStillWord(still bool) string {
	if still {
		return "还在"
	}
	return "不在"
}
