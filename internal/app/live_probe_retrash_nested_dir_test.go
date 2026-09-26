// 第卅一趟活体探针：对**一枚非空目录**的 `fileId` 重复打 `batchTrash` 会怎样。
//
// # 这一问从哪来（不是新开战场，是收第卅趟自己划的那条边界）
//
// 第卅趟（[`live_probe_retrash_same_file_test.go`](./live_probe_retrash_same_file_test.go)）
// 答了"同一枚 `fileId` 点两次名"，但它在成文里**自己写了边界四条**，其中一条是：
// 对象只测了**空目录**与**单个文件**两型 ⇒ **非空目录被点两次名仍未测**。
// 而清理链真正天天点名的正是**非空目录**那一型（R36 / #185 那一族：1014 的转换产物落在
// `/139邮箱/我的附件/PDF转换`，链尾 `cleanupCloudFiles` 删的是带内容的对象）⇒
// 第卅趟那一份结论**不能**直接搬去给清理链用。这一趟就是把它搬过来：同一判据、第三型对象。
//
// ⚠ 与"一次动两问"那条纪律不冲突（第廿六趟的教训）：这一趟**只有一个判据**（重复点名之后
// 回收站里那一条还在不在、多不多），非空只是**该判据的第三种样本**。子文件是否单列是
// R7 那一格在"重复点名"这个前提下**顺带**的对照，不是第二问 —— 它不参与主问的判定。
//
// # 形状
//
//   - 造**一枚非空目录**：`file/create` + `type:"folder"` 建 outer，再往 outer 里
//     `UploadRandom(ParentFileID=outer)` 传一枚文件（两型都已在第廿六趟真打过）；
//   - 对 **outer 那一枚** `fileId` 打**两发** `batchTrash`（第一发是"进回收站"，第二发才是本问）；
//   - 回读三遍：**⓪ 删之前**（对照）/ **② 第一发之后** / **④ 第二发之后**，两遍之间**只等、不发**；
//   - 每一发都念**我方发出时刻**（格式对齐上游 `trashedAt`）—— 第卅趟那条 #202 的方法格，
//     这里同样成立：条数相同时只有时间戳能把"第一发留下的"与"第二发新记的"分开。
//
// # 判定与读数口径（全部复用第卅趟那把尺）

// 条数判据走 [liveProbeRetrashWord]（同一支函数、同一批 `[幂等=…]` 短标）⇒ 两趟的读数
// **可以逐字对比**，不需要读者自己换算。另加一句本趟自有的短标：
// [liveProbeRetrashKidWord] 回答"重复点名之后，里面那枚子文件会不会单列出来"——
// 它只有三档（不单列 / 单列 N 条 / 未答），且**不**参与主问判定。
//
// ⚠ 只有**一把尺**（解析器翻页数整名），不像第廿八/廿九趟那样"裸读 + 解析器"并排：
// 判的是**条数**而不是"第几遍第一次出现"，那一族教训（#199）在这里不触发。
// 但"这一遍没看到 ≠ 它不在回收站里"仍然成立 ⇒ 判定只用**等过之后**的 ② 与 ④，⓪ 只当对照。
//
// # 写面与残留
//
// 造 2 枚（1 目录 + 1 文件）、打 **2 发** `batchTrash`（同一枚目录点名两次）⇒
// 回收站里的净增条数**就是这一趟的读数**（R7 说非空目录只留 1 条，本趟复查它在重复点名后还成不成立）。
// 本仓没有"彻底删除"口 ⇒ 残留只能人工清，并入 **R37ⓒ** 那批待拍板。
// 中途造子文件失败 ⇒ 把已造出的那枚目录**打一发**收场（不在用户网盘里留裸目录）。
//
// # 凭据与隐私边界（沿用第廿七～卅趟那一族，一条不放宽）
//
//   - 名字只用 `asign-` 前缀（我们自己造的），用户条目**只计数、不念名字与字段值**；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；判定按**整名**（#196）；
//   - 翻页游标的**值不打印**（只念 `游标到底=true/false` + 是否被闸门截断）；
//   - 两发回执各念 `code` / `success` / `msg` 三格：`msg` 过 [liveProbeSink.mask] 并截断
//     （#195 同族：错误文本是第二条泄漏通路）。

package app

import (
	"context"
	"strconv"
	"strings"
	"testing"
	"time"
)

const (
	// 两枚名字的中段（都可打印：整名带 `asign-` 前缀 ⇒ 是我们自己造的）。
	liveProbeRetrashNestDirInfix = "retrashnest-"    // 最外层那枚：**两发都点它**
	liveProbeRetrashNestKidInfix = "retrashnestkid-" // 它在网盘里的子文件（不单列才是预期）
	// liveProbeRetrashNestMaxPages 沿用回收站那一族的闸门，不另立面值：读数才可跨趟比。
	liveProbeRetrashNestMaxPages = liveProbeRetrashMaxPages
)

// TestLiveProbeNestedDirRetrash 写探针：非空目录被同一份 `batchTrash` 点名两次。
//
// 跑法与第卅趟同档（四个 env + 副本库，日志一律落 `target/s31probe/runK.txt`，#198）；
// 一趟约 45s（两遍 `time.Sleep` 各 20s）⇒ `-timeout` 给到 900s。
// ⚠ 如果一趟只跑十几秒就出"②=0 ④=1"，第一件要查的是**那两行 sleep 还在不在**（#202）。
func TestLiveProbeNestedDirRetrash(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器（本探针要先造一枚非空目录再打两发）：%s", binErr(sink, uerr))
	}

	dirName := CloudUploadNamePrefix + liveProbeRetrashNestDirInfix +
		randomHex(uploader.Rng, cloudRandomNameHexLen)
	kidName := CloudUploadNamePrefix + liveProbeRetrashNestKidInfix +
		randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"

	// ── ① 造一枚非空目录：先建 outer，再往它里面传一枚文件 ──
	created, cerr := run.Upstream.AppCloudFolderCreate(ctx, uploader.Auth, uploader.DeviceID, "/", dirName)
	switch {
	case cerr != nil:
		sink.say("① 造目录（%q）失败：%s ⇒ 下面全部不发", dirName, binErr(sink, cerr))
		recorder.sayTally()
		return
	case created.Exist:
		sink.say("① 上游说 %q 同名已存在 ⇒ 当场收手（那一枚属于用户本来就有的目录，不删）", dirName)
		recorder.sayTally()
		return
	case created.FileID == nil:
		sink.say("① 建目录回了 OK 但没有 fileId ⇒ 没有可打名的对象，下面全部不发")
		recorder.sayTally()
		return
	}
	dirID := *created.FileID
	sink.see(dirID)
	kid, kerr := uploader.UploadRandom(ctx, CloudUploadOptions{ParentFileID: dirID, Name: kidName})
	if kerr != nil || kid.FileID == "" {
		sink.say("① 往 %q 里传子文件（%q）没成：%s ⇒ 收场：把已造的目录打一发删掉（不在用户网盘里留裸目录）",
			dirName, kidName, binErr(sink, kerr))
		if sent, derr := uploader.DeleteCloudFiles(ctx, []string{dirID}); derr != nil || !sent {
			sink.say("  收场那一枪也没打通（发过=%v）：%s", sent, binErr(sink, derr))
		}
		recorder.sayTally()
		return
	}
	sink.see(kid.FileID)
	sink.say("① 造好了一枚非空目录：%q（fileId 长度 %d）里面装着 %q（长度 %d）⇒ "+
		"对**目录那一枚**各打两发 `batchTrash`（第一发是「进回收站」，第二发才是本趟的问）",
		dirName, len(dirID), kidName, len(kid.FileID))

	// ── ⓪ 删之前的对照读数（两枚都读：目录是判据，子文件是 R7 那一格的对照）──
	controlDir := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "⓪ 删之前·目录")
	controlKid := liveProbeRetrashCount(ctx, sink, run, uploader, kidName, "⓪ 删之前·子文件")

	// ── ①′ 第一发：把这枚非空目录送进回收站 ──
	first := liveProbeRetrashTrash(ctx, sink, run, uploader, dirID, "①′ 非空目录第一发")

	// ── ② 等过之后读第一遍 ──
	sink.say("② 第一发打过 ⇒ 等 %s，中间**一枪都不发**，然后读第一遍（理由同第卅趟：秒级不回）",
		liveProbeBinWait)
	time.Sleep(liveProbeBinWait)
	afterFirstDir := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "② 第一发之后·目录")
	afterFirstKid := liveProbeRetrashCount(ctx, sink, run, uploader, kidName, "② 第一发之后·子文件")

	// ── ③ 第二发：**同一枚目录 fileId** 再点一次名（本趟要问的那一枪）──
	second := liveProbeRetrashTrash(ctx, sink, run, uploader, dirID, "③ 非空目录第二发（同一枚 fileId）")

	// ── ④ 再等一遍，读第二遍 ──
	sink.say("④ 第二发打完 ⇒ 再等 %s（与 ② 同一档：两遍之间只等、一枪不发），然后读第二遍", liveProbeBinWait)
	time.Sleep(liveProbeBinWait)
	afterSecondDir := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "④ 第二发之后·目录")
	afterSecondKid := liveProbeRetrashCount(ctx, sink, run, uploader, kidName, "④ 第二发之后·子文件")

	// ── ⑤ 判定 ──
	wordDir := liveProbeRetrashWord("非空目录", controlDir, afterFirstDir, afterSecondDir)
	sink.say("⑤ 主问：%s", wordDir)
	sink.say("⑤ 顺带（R7 那一格在重复点名之后的对照）：%s",
		liveProbeRetrashKidWord(afterFirstKid, afterSecondKid))
	sink.say("⑤ 回执对照（第二发 vs 第一发，逐字）：%s", liveProbeRetrashReplyWord(first, second))
	sink.say("⑤ 三遍的条数（同一把尺：解析器翻页，闸门 %d 页 / 最多 %d 条）：目录 ⓪=%s ②=%s ④=%s ｜ "+
		"子文件 ⓪=%s ②=%s ④=%s；末遍是否被闸门截断：目录=%s 子文件=%s",
		liveProbeRetrashNestMaxPages, liveProbeBinMaxItems,
		liveProbeRetrashHitsWord(controlDir), liveProbeRetrashHitsWord(afterFirstDir),
		liveProbeRetrashHitsWord(afterSecondDir),
		liveProbeRetrashHitsWord(controlKid), liveProbeRetrashHitsWord(afterFirstKid),
		liveProbeRetrashHitsWord(afterSecondKid),
		strconv.FormatBool(afterSecondDir.truncated), strconv.FormatBool(afterSecondKid.truncated))
	sink.say("⑤ 命中那几枚的入站时间（`trashedAt`，按翻页顺序）：目录 %s ｜ 子文件 %s ⇒ "+
		"目录两枚时那是两条独立条目；子文件若在第一遍是「无」而第二遍出现，那是**重复点名把它单列出来了**",
		liveProbeRetrashTimesWord(afterSecondDir), liveProbeRetrashTimesWord(afterSecondKid))
	sink.say("⑤ 与第卅趟对照：那一趟的空目录与文件两枚都是 %s ⇒ 本趟这一型（%s）"+
		"若同档，第卅趟那句结论才升成「清理链点名的三型都测过」",
		"[幂等=仍一条]", liveProbeRetrashTagOf(wordDir))

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪、%s 发了 %d 枪 ⇒ 写侧是那两发 batchTrash（同一枚点名两次），"+
		"残留 = 用户回收站里本趟那一枚（条数就看 ⑤ 那两个数）＋网盘侧那枚子文件随目录一起进回收站；"+
		"本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeBatchTrashPath, recorder.countFor(liveProbeBatchTrashPath),
		liveProbeBinPath, recorder.countFor(liveProbeBinPath))
}

// liveProbeRetrashKidWord 念"重复点名之后，那枚子文件在回收站里单列了吗"。
//
// 为什么单独做一支函数、且不并进 [liveProbeRetrashWord]：那一支的判据是**同一枚对象的条数变化**，
// 而子文件从头到尾**没被点过名**（两发都只点目录），它的读数回答的是另一格
// （"整棵子树压成一条"在第二发之后还成不成立 = R7）。两格混进一句会重演第廿八趟那条错：
// 读者把其中一个判据当成另一个。
//
// ⚠ 这里"②=0"与"④=0"**都是预期**（R7：子文件不单列），所以这一支不给"通过/失败"的口径，
// 只登记读数；它唯一带结论的一档是「④ 比 ② 多」 —— 那才是本趟可能挖出来的新缺陷。
//
// 反向改坏它：把 `second.hits > first.hits` 那一支改成 `>=` ⇒ [TestLiveProbeRetrashKidWord]
// 里"两遍都 0 条"那一行会红（它就不该报"单列"）。
func liveProbeRetrashKidWord(first, second liveProbeRetrashRead) string {
	switch {
	case !first.ok || !second.ok:
		return "[子文件=未答] 两遍里有一遍没打通（② ok=" + strconv.FormatBool(first.ok) +
			" / ④ ok=" + strconv.FormatBool(second.ok) + "）⇒ 这一格没有读数"
	case second.hits > first.hits:
		return "[子文件=重复点名后单列] 第一发之后 " + strconv.Itoa(first.hits) +
			" 条，第二发之后 " + strconv.Itoa(second.hits) + " 条 ⇒ 子文件是在**第二发**之后才单列出来的" +
			"（这与 R7 那句「整棵子树压成一条」相反 ⇒ 要重开 R7，不能只补记本趟）"
	case second.hits == 0:
		return "[子文件=不单列] 两遍都 0 条 ⇒ R7 那一格在重复点名之后仍然成立" +
			"（⚠ 这一格只到「今天这一枚」为止：非空目录里的**层数与件数**没变过，仍是两层 1 枚那一型）"
	default:
		return "[子文件=两遍同数且非零] 第一发之后 " + strconv.Itoa(first.hits) +
			" 条、第二发之后 " + strconv.Itoa(second.hits) + " 条 ⇒ 它在第一发之后**就**单列了（与 R7 相反，" +
			"那一档要对着 R7 那一族复核，不是本趟改口径），而第二发**没有再加条**"
	}
}

// TestLiveProbeRetrashKidWord 钉住上面那四档分派（不跑上游）。
//
// 为什么这一支要有单测而第卅趟那几支也有：判定函数一旦只在上游跑，"读数落进哪一档"就没人管 ——
// 而本趟最可能踩的恰是**分派写反**（`>` 与 `>=`、0 与非 0），那种错在真实日志里读得出来、
// 在测试没跑过时读不出来。
func TestLiveProbeRetrashKidWord(t *testing.T) {
	okRead := func(hits int) liveProbeRetrashRead {
		return liveProbeRetrashRead{ok: true, hits: hits}
	}
	dead := liveProbeRetrashRead{}
	cases := []struct {
		name          string
		first, second liveProbeRetrashRead
		want          string
	}{
		{"两遍都没打通 ⇒ 未答", dead, dead, "[子文件=未答]"},
		{"② 那遍没打通 ⇒ 未答（即使 ④ 有读数）", dead, okRead(1), "[子文件=未答]"},
		{"两遍都 0 条 ⇒ 不单列（R7 那一格仍成立）", okRead(0), okRead(0), "[子文件=不单列]"},
		{"第二遍比第一遍多 ⇒ 重复点名后单列（与 R7 相反）", okRead(0), okRead(1), "[子文件=重复点名后单列]"},
		{"两遍同数且非零 ⇒ 不并进「单列」那一档", okRead(1), okRead(1), "[子文件=两遍同数且非零]"},
		{"2→1：第二发没加条，也不报「单列」", okRead(2), okRead(1), "[子文件=两遍同数且非零]"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := liveProbeRetrashKidWord(tc.first, tc.second)
			if !strings.HasPrefix(got, tc.want) {
				t.Errorf("档位短标实得 %q，期望以 %q 开头", got, tc.want)
			}
			if liveProbeRetrashTagOf(got) != tc.want {
				t.Errorf("短标取不到：liveProbeRetrashTagOf(%q) = %q", got, liveProbeRetrashTagOf(got))
			}
		})
	}
}
