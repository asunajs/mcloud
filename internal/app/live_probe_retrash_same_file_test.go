package app

// 活体探针：**对同一枚 `fileId` 重复打 `batchTrash`** 会怎么样（第卅趟，R7 剩下的那一格）。
//
// # 这一趟只答一问
//
// 第廿六趟～第廿九趟把"删一枚目录 ⇒ 回收站里留几条"与"`trashedDirStatistics` 数哪一档"都读过了，
// 但**清理链每天在打的这一枪，从没对同一枚对象打过第二发**（capture §10 与 R7 都把它登记成
// "要第二发写 ⇒ 留单独一趟"）。这一格不是学术题，它决定三件正在用的东西：
//
//   - **重复执行安不安全**：同一条任务被调度跑两次、或用户点两次清理 ⇒ 同一枚 `fileId`
//     会被点两次名。今天不知道第二发是"照样回 OK"还是"报错"。
//   - **回收站会不会被我们自己灌满**：若第二发**又记一条**，那"重复清理"的代价就不只是多打几枪，
//     而是用户回收站里出现同名的两条（还原时他会看到两份）。
//   - **反向的那一档更要紧**：若第二发把那一枚从回收站里**弄没了**（不管是去还原还是去彻底删除），
//     那"清理链重试一次 = 用户的东西真没了"，这一档必须在成文里点出来。
//
// 三个判据面合起来就是 ④ 那句 `[幂等=…]`：`仍一条` / `重复入账` / `条目消失` / `另一档` / `未答`。
//
// # 为什么带**两枚**对象（空目录 + 一枚文件）
//
// 清理链删的就是**这两种**（玩法 106 / 522 / 1014 那几族传的是文件，目录是这一族探针自己在造），
// 而"幂等"是按**对象类型**可能不同的行为 ⇒ 同一趟里对两枚各问一次同一问，仍然只有**一问**
// （与"一次动两问"不同：那是两个判据互相污染，这里是同一判据两个样本）。
// 两枚各自单独打枪（不合并进同一发的 `fileIds` 数组），因为"一条名单里两个 id"与
// "同一枚 id 打两次"是两个问题 —— 本趟只问后者。
//
// # 三遍回读与"等"（第廿七趟那条秒级不回）
//
// 读数按**同一把尺**打三遍：⓪ 删之前（对照）、② 第一发之后、④ 第二发之后。
// 每遍之间唯一动作是**等** `[liveProbeBinWait]`（20s），期间一枪不发 —— 第廿七趟实测过
// "刚删的那一枚当场不在、等过之后才在"，所以**不等就读**会让"第一发没进"与"还没进索引"混掉。
// ⓪ 那一遍是对照：它若命中 ≥1，说明认名认错（用户本来就有同名 / 中段撞车），
// 那么"条数"这一整格判据**不成立** ⇒ [幂等=未答]。
//
// ⚠ 这一趟只有**一把尺**（解析器翻页），不像第廿八/廿九趟那样"裸读 + 解析器"两发并排 ——
// 因为本趟判的是**条数**而不是"第几遍第一次出现"，两把尺那格教训（坑表 #199）在这里不触发。
// 但那一族的另一句仍然成立：**"这一遍没看到"不等于"它不在回收站里"**，
// 所以判定只用"等过之后"的两遍（② 与 ④），⓪ 只当对照用。
//
// ⚠ **每一发都念出我方发出时刻**（格式对齐上游 `trashedAt`）：本趟要回答"回收站里那一条是
// 第一发留下的、还是第二发新记的"，而条数相同时（1 条 vs 1 条）**只有时间戳能分开这两档**。
// 2026-09-26 第一趟**两格都缺**（既漏了那两行 `sleep`，也没念发出时刻）⇒ 那一趟 4 发写枪
// 只换回一行 `[幂等=未答]`，读数只登记成"过程"，不进任何一档结论（坑表 #202）。
//
// # 写面与残留
//
// 造 2 枚（1 空目录 + 1 随机图）、打 **4 枪** `batchTrash`（每枚各两发）⇒
// 回收站里的净增条数**就是这一趟的读数**（可能是 1 条/枚，也可能是 2 条/枚）。
// 本仓没有"彻底删除"口 ⇒ 残留只能人工清，并入 **R37ⓒ** 那批待拍板。
// 中途造失败 ⇒ 把已造出的那枚**各打一发**收场（不留裸文件在用户网盘里）。
//
// # 凭据与隐私边界（沿用第廿七～廿九趟那一族，一条不放宽）
//
//   - 名字只用 `asign-` 前缀（我们自己造的），用户条目**只计数、不念名字与字段值**；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；判定按**整名**（#196）；
//   - 翻页游标的**值不打印**（本趟只念"到底没有"：`游标到底=true/false` + 是否被闸门截断）；
//   - 第二发的回执念 `code` / `success` / `msg` 三格：`msg` 过 [liveProbeSink.mask] 并截断 ——
//     上游在错误档里可能把 URL 塞进那句话（#195 同族：错误文本是第二条泄漏通路）。

import (
	"context"
	"encoding/json"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// 两枚名字的中段（都可打印：整名带 `asign-` 前缀 ⇒ 是我们自己造的）。
	liveProbeRetrashDirInfix  = "retrashdir-"
	liveProbeRetrashFileInfix = "retrashfile-"
	// 闸门沿用回收站那一族那两枚，不另立面值：同一口、同一档，读数才可跨趟比。
	liveProbeRetrashMaxPages = liveProbeBinMaxPages
	// liveProbeRetrashClockLayout **刻意对上**上游 `trashedAt` 的形状（毫秒 + 带区号的时区偏移）。
	//
	// 为什么要一模一样：本趟要把"回收站里那一条"归到"两发里的哪一发"，唯一的办法是拿
	// 我方发出时刻与上游入站时刻**逐位比**。形状不同时读者得自己心算偏移与毫秒位，
	// 而这种心算正是"把 A 的条目读成 B 的条目"最容易出错的地方。
	liveProbeRetrashClockLayout = "2006-01-02T15:04:05.000Z07:00"
)

// liveProbeRetrashRead 是一遍回读的读数（**解析器**翻页数整名命中）。
//
// 为什么带 `items`/`pages`/`truncated`：念"没看到"之前必须先念"是不是被闸门挡住了"
// （第廿七趟那条：翻到第三页停住 ≠ 回收站里没有它）。
type liveProbeRetrashRead struct {
	ok        bool     // 这一遍**打没打通**（第一页都没回 ⇒ false，此时 hits 不可用）
	hits      int      // 整名命中的**条数**（本趟的判据就落在这一格）
	times     []string // 命中那几枚的 `trashedAt`（原文可打印：是时间不是身份）
	items     int      // 扫到的总条数
	pages     int      // 翻了几页
	truncated bool     // 是否被闸门截断（还有游标却没继续翻）
}

// liveProbeRetrashReply 是一发 `batchTrash` 的回执读数。
type liveProbeRetrashReply struct {
	fired   bool   // 请求构造成功、发出去了（空列表那一档由调用方挡）
	shot    bool   // 拿到了 HTTP 回执（传输层报错 ⇒ false）
	cells   string // 回执三格的原文短码（code / success / msg，已过掩码与截断）
	verdict string // [upstream.ParseCloudFileBatchTrash] 的判读：通了念「判成成功」
}

func TestLiveProbeSameFileRetrash(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器（本探针要先造两枚再各打两发）：%s", binErr(sink, uerr))
	}

	dirName := CloudUploadNamePrefix + liveProbeRetrashDirInfix +
		randomHex(uploader.Rng, cloudRandomNameHexLen)
	fileName := CloudUploadNamePrefix + liveProbeRetrashFileInfix +
		randomHex(uploader.Rng, cloudRandomNameHexLen) + ".png"

	// ── ① 造两枚：一枚空目录 + 一枚文件 ──
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
	file, ferr := uploader.UploadRandom(ctx, CloudUploadOptions{Name: fileName})
	if ferr != nil || file.FileID == "" {
		sink.say("① 传文件（%q）没成：%s ⇒ 收场：把已造的目录各打一发删掉（不在用户网盘里留东西）",
			fileName, binErr(sink, ferr))
		if sent, derr := uploader.DeleteCloudFiles(ctx, []string{dirID}); derr != nil || !sent {
			sink.say("  收场那一枪也没打通（发过=%v）：%s", sent, binErr(sink, derr))
		}
		recorder.sayTally()
		return
	}
	sink.see(file.FileID)
	sink.say("① 造好了两枚：空目录 %q（fileId 长度 %d）/ 文件 %q（长度 %d）⇒ "+
		"两枚**各打两发** `batchTrash`（每枚第一发是「进回收站」，第二发才是本趟的问）",
		dirName, len(dirID), fileName, len(file.FileID))

	// ── ⓪ 删之前的对照读数（这一遍只当对照，不参与幂等判定）──
	controlDir, controlFile := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "⓪ 删之前"),
		liveProbeRetrashCount(ctx, sink, run, uploader, fileName, "⓪ 删之前")

	// ── ①′ 第一发：每枚各一枪（把它们送进回收站）──
	firstDir := liveProbeRetrashTrash(ctx, sink, run, uploader, dirID, "①′ 目录第一发")
	firstFile := liveProbeRetrashTrash(ctx, sink, run, uploader, file.FileID, "①′ 文件第一发")

	// ── ② 等过之后读第一遍（第廿七趟那条"秒级不回" ⇒ 不等就读，判据不成立）──
	sink.say("② 两发第一枪都打过 ⇒ 等 %s，中间**一枪都不发**，然后读第一遍", liveProbeBinWait)
	sink.say("② （等这一段的理由：第廿七趟实拍到「当场不在、等过之后它在第 0 项」，" +
		"而本趟的判据是**条数** ⇒ 把「还没进」读成「只进了一条」是同一族错误）")
	time.Sleep(liveProbeBinWait)
	afterFirstDir := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "② 第一发之后")
	afterFirstFile := liveProbeRetrashCount(ctx, sink, run, uploader, fileName, "② 第一发之后")

	// ── ③ 第二发：**同一枚 fileId** 再点一次名（这一发就是本趟要问的那一枪）──
	secondDir := liveProbeRetrashTrash(ctx, sink, run, uploader, dirID, "③ 目录第二发（同一枚 fileId）")
	secondFile := liveProbeRetrashTrash(ctx, sink, run, uploader, file.FileID, "③ 文件第二发（同一枚 fileId）")

	// ── ④ 再等一遍，读第二遍 ──
	sink.say("④ 第二发打完 ⇒ 再等 %s（与 ② 同一档：两遍之间只等、一枪不发），然后读第二遍", liveProbeBinWait)
	time.Sleep(liveProbeBinWait)
	afterSecondDir := liveProbeRetrashCount(ctx, sink, run, uploader, dirName, "④ 第二发之后")
	afterSecondFile := liveProbeRetrashCount(ctx, sink, run, uploader, fileName, "④ 第二发之后")

	// ── ⑤ 判定：两枚各一句，加一句"两枚是否同档" ──
	wordDir := liveProbeRetrashWord("目录", controlDir, afterFirstDir, afterSecondDir)
	wordFile := liveProbeRetrashWord("文件", controlFile, afterFirstFile, afterSecondFile)
	sink.say("⑤ 主问（目录那枚）：%s", wordDir)
	sink.say("⑤ 主问（文件那枚）：%s", wordFile)
	sink.say("⑤ ⇒ %s", liveProbeRetrashSameWord(wordDir, wordFile))
	sink.say("⑤ 回执对照（第二发 vs 第一发，逐字）：目录 %s", liveProbeRetrashReplyWord(firstDir, secondDir))
	sink.say("⑤ 回执对照（第二发 vs 第一发，逐字）：文件 %s", liveProbeRetrashReplyWord(firstFile, secondFile))
	sink.say("⑤ 三遍的条数（同一把尺：解析器翻页，闸门 %d 页 / 最多 %d 条）：目录 ⓪=%s ②=%s ④=%s ｜ "+
		"文件 ⓪=%s ②=%s ④=%s；末遍是否被闸门截断：目录=%s 文件=%s",
		liveProbeRetrashMaxPages, liveProbeBinMaxItems,
		liveProbeRetrashHitsWord(controlDir), liveProbeRetrashHitsWord(afterFirstDir),
		liveProbeRetrashHitsWord(afterSecondDir),
		liveProbeRetrashHitsWord(controlFile), liveProbeRetrashHitsWord(afterFirstFile),
		liveProbeRetrashHitsWord(afterSecondFile),
		strconv.FormatBool(afterSecondDir.truncated), strconv.FormatBool(afterSecondFile.truncated))
	sink.say("⑤ 命中那几枚的入站时间（`trashedAt`，按翻页顺序）：目录 %s ｜ 文件 %s ⇒ "+
		"两枚都命中且时间**不同**时，那是两条独立条目（各自有入站时刻），不是一条被改写了时间",
		liveProbeRetrashTimesWord(afterSecondDir), liveProbeRetrashTimesWord(afterSecondFile))

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪、%s 发了 %d 枪 ⇒ 写侧是那四发 batchTrash，"+
		"残留 = 用户回收站里本趟那两枚（条数就看 ⑤ 那两个数）；本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeBatchTrashPath, recorder.countFor(liveProbeBatchTrashPath),
		liveProbeBinPath, recorder.countFor(liveProbeBinPath))
}

// liveProbeRetrashTrash 对**一枚** id 打一枪 `batchTrash` 并交回回执读数。
//
// 为什么走 [upstream.Upstream.Call] 而不是编排层那一份 [CloudUploader.DeleteCloudFiles]：
// 后者把回执折成"*core.Error（nil=成功）"，而本趟要的正是**第二发回的那三格原文**
// （上游可能照样回 `0000`、可能回另一句、可能 HTTP 层就挂）。同一趟里第一发与第二发
// **都**走这一条路 ⇒ 两者可比（拿第一发走编排层、第二发走 Call，差的就不仅是上游而是我们的代码）。
func liveProbeRetrashTrash(
	ctx context.Context, sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader,
	fileID string, what string,
) liveProbeRetrashReply {
	reply := liveProbeRetrashReply{}
	request, perr := upstream.CloudBatchTrashRequest(uploader.Auth, uploader.DeviceID, []string{fileID})
	if perr != nil {
		sink.say("%s：请求就没拼出来：%s ⇒ 这一发**没发**", what, binErr(sink, perr))
		reply.verdict = "请求构造失败"
		return reply
	}
	reply.fired = true
	idLen := len(fileID) // 只念长度：真实 fileId 不进成文（第廿七趟那条纪律）
	// ⚠ **发出时刻**必须念：本趟靠回收站那一格的 `trashedAt` 判断"这一条是第一 sent 的还是第二发"，
	// 而没有发出时刻时两种解释都成立（2026-09-26 第一趟就吃过这一格亏，见坑表 #202）。
	sink.say("%s：发出时刻 %s（fileId 长度 %d）", what,
		time.Now().Format(liveProbeRetrashClockLayout), idLen)
	envelope, cerr := run.Upstream.Call(ctx, request)
	if cerr != nil {
		sink.say("%s（fileId 长度 %d）：这一发没打通：%s", what, idLen, binErr(sink, cerr))
		reply.verdict = "没打通"
		return reply
	}
	reply.shot = true
	reply.cells = liveProbeRetrashEnvelopeWord(envelope)
	if parsed := upstream.ParseCloudFileBatchTrash(envelope); parsed != nil {
		reply.verdict = "我方判成失败"
		sink.say("%s（fileId 长度 %d）：回执 %s ⇒ 我方那份判据（[upstream.ParseCloudFileBatchTrash]）"+
			"把它判成**失败**：%s", what, idLen, reply.cells, binErr(sink, parsed))
		return reply
	}
	reply.verdict = "判成成功"
	sink.say("%s（fileId 长度 %d）：回执 %s ⇒ 我方判据念「成功」", what, idLen, reply.cells)
	return reply
}

// liveProbeRetrashEnvelopeWord 念信封那三格的短码（`code` / `success` / `msg`）。
//
// 为什么走 [liveProbeShapeValue] 而不是自己拼：`code` / `msg` / `success` 本来就在
// "值可原样打印"那份名单（`liveProbeValueKeys`）里，而那一格同时负责**按 rune 截断** ——
// 本趟是新开打印面（回执原文以前从没进过日志），复用同一把尺才不会在这一行上自己定一套宽严。
// 缺席的键念成「缺键」而不是空串：`{"code":"0000"}` 与 `{"code":"0000","msg":""}` 是两档读数。
func liveProbeRetrashEnvelopeWord(envelope *upstream.Envelope) string {
	raw, isObject := envelope.Raw().(map[string]any)
	if !isObject {
		return "回执不是对象（" + liveProbeShapeValue("x", envelope.Raw()) + "）"
	}
	parts := make([]string, 0, 3)
	for _, key := range []string{"code", "success", "msg"} {
		value, has := raw[key]
		if !has {
			parts = append(parts, key+"=缺键")
			continue
		}
		parts = append(parts, liveProbeShapeValue(key, value))
	}
	return strings.Join(parts, " ")
}

// liveProbeRetrashCount 用**解析器**翻页数"整名 == wantName 的条目有几条"。
//
// 为什么按整名而不是中段（#196）：本趟判的是**条数**，而同一族复跑时回收站里会同时有
// **更早那趟**同中段的残留 ⇒ 按中段数出来的"两条"可能是"本趟一条 + 上一趟一条"，判据是假的。
// 中段只用在**只读**复跑那一支（它拿不到现造的整名），那一支也因此只念"这一族在不在"、不判条数。
func liveProbeRetrashCount(
	ctx context.Context, sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader,
	wantName, what string,
) liveProbeRetrashRead {
	out := liveProbeRetrashRead{}
	cursor := ""
	for page := 1; page <= liveProbeRetrashMaxPages; page++ {
		list, lerr := run.Upstream.AppCloudRecycleBinList(ctx, uploader.Auth, uploader.DeviceID, cursor)
		if lerr != nil {
			sink.say("%s：第 %d 页没打通：%s ⇒ 这一遍没有条数", what, page, binErr(sink, lerr))
			return out
		}
		out.ok = true
		out.pages = page
		out.items += len(list.Items)
		for _, entry := range list.Items {
			if entry.Name == nil || *entry.Name != wantName {
				continue // 只数本趟那两枚：用户条目一个字段都不进输出
			}
			out.hits++
			word := "trashedAt 缺键"
			if entry.TrashedAt != nil {
				word = "trashedAt=" + strconv.Quote(*entry.TrashedAt)
			}
			out.times = append(out.times, word)
		}
		if list.NextPageCursor == "" {
			break // 上游说没有下一页
		}
		cursor = list.NextPageCursor
		if page == liveProbeRetrashMaxPages {
			out.truncated = true // 还有游标却没继续翻 ⇒ "没看到"这一档不可用
		}
	}
	sink.say("%s：第一页起共翻 %d 页、扫到 %d 条，整名 %q 命中 %d 条，游标到底=%v、%s ⇒ 逐枚 %s",
		what, out.pages, out.items, wantName, out.hits, !out.truncated,
		liveProbeTruncWord(out.truncated), liveProbeRetrashTimesWord(out))
	return out
}

// liveProbeRetrashHitsWord 念"这一遍的命中条数"，没打通那一档念成「没读数」而不是「0 条」。
//
// 为什么单独做：`0` 在这里有**三种**意思（真没有 / 这一遍没打通 / 被闸门挡住），
// 直接把 int 印出来会让读者只看到"0"（第廿七趟那条"未知不许念成空话"，#190 同族）。
func liveProbeRetrashHitsWord(read liveProbeRetrashRead) string {
	if !read.ok {
		return "没读数"
	}
	return strconv.Itoa(read.hits)
}

// liveProbeRetrashTimesWord 念命中那几枚的入站时间；没有命中时念「无」。
func liveProbeRetrashTimesWord(read liveProbeRetrashRead) string {
	if len(read.times) == 0 {
		return "无"
	}
	return strings.Join(read.times, " / ")
}

// liveProbeRetrashWord 把三遍读数折成一句带**机器可核短标**的判定。
//
// 为什么要短标而不是只写一句话（第廿九趟 #201② 同族）：这一族的读数句里为了对照会**提到反面
// 那一档**（"不是重复入账"这种句子），测试只 grep 文字就会假绿 ⇒ 判定档只由 `[幂等=…]` 给。
//
// 反向改坏它：把 `second.hits == first.hits` 那一支删掉 ⇒ `TestLiveProbeRetrashWord` 里
// "仍一条"那一行红；把 `control.hits != 0` 那一支删掉 ⇒ "对照不成立"那一行红。
func liveProbeRetrashWord(
	what string, control, first, second liveProbeRetrashRead,
) string {
	switch {
	case !control.ok || !first.ok || !second.ok:
		return "[幂等=未答] " + what + "：三遍里有一遍没打通（⓪/②/④ 各自 ok=" +
			strconv.FormatBool(control.ok) + "/" + strconv.FormatBool(first.ok) + "/" +
			strconv.FormatBool(second.ok) + "）⇒ 没有条数可比，这一问不判（**不等于**「上游不幂等」）"
	case control.hits != 0:
		return "[幂等=未答] " + what + "：删之前那一遍就命中 " + strconv.Itoa(control.hits) +
			" 条 ⇒ 认名认错（用户本来就有同名 / 中段撞车），这一趟的「条数」整格判据不成立"
	case first.hits == 0:
		return "[幂等=未答] " + what + "：第一发之后（等过 20s 那一遍）仍 0 条 ⇒ 这枚**根本没进**回收站，" +
			"第二发没有「已存在的条目」可重复作用于它 ⇒ 幂等这一问落空（先要判的是「为什么没进」）"
	case second.hits == first.hits:
		return "[幂等=仍一条] " + what + "：第二发之后还是 " + strconv.Itoa(second.hits) +
			" 条（与第一发之后同数）⇒ 对同一枚 fileId 重复打 batchTrash **没有**多出一条"
	case second.hits == 0:
		return "[幂等=条目消失] " + what + "：第一发之后 " + strconv.Itoa(first.hits) +
			" 条，第二发之后 **0** 条 ⇒ 第二发把它从回收站那一页里**拿走了**（这一档与「幂等」相反，" +
			"后果最重：清理链重试一次可能等于让用户的东西从回收站消失）⇒ 要分「被还原」还是「被彻底删除」" +
			"得另开一趟，本趟只登记「它不在了」"
	case second.hits > first.hits:
		return "[幂等=重复入账] " + what + "：第一发之后 " + strconv.Itoa(first.hits) +
			" 条，第二发之后 " + strconv.Itoa(second.hits) + " 条 ⇒ 重复点名会**多记条目**" +
			"（用户在回收站里会看到同名两份，还原时也是两份）"
	default:
		return "[幂等=另一档] " + what + "：⓪/②/④ 三遍命中 " + liveProbeRetrashHitsWord(control) +
			"/" + liveProbeRetrashHitsWord(first) + "/" + liveProbeRetrashHitsWord(second) +
			" 条 ⇒ 不落在上面任何一档 ⇒ 不硬解释，逐字读数就是这一行"
	}
}

// liveProbeRetrashSameWord 念"两枚（目录 / 文件）是不是同一档"。
//
// 为什么值得单念一句：这一趟带了两个样本，为的就是"幂等这一档按对象类型分不分"。
// 两枚同档 ⇒ 这一格可以说"两种被清理链点名的对象读数一致"；不同档 ⇒ **不能**合并成一句结论，
// 得按类型分开写进成文（否则下一轮改清理链的人会拿错那一档）。
func liveProbeRetrashSameWord(wordDir, wordFile string) string {
	tagDir, tagFile := liveProbeRetrashTagOf(wordDir), liveProbeRetrashTagOf(wordFile)
	switch {
	case tagDir == "" || tagFile == "":
		return "两枚里有一枚没打出短标 ⇒ 这句不判"
	case tagDir == tagFile:
		return "⭐ 目录与文件**同一档**（都是 " + tagDir + "）⇒ 这一档今天没有按对象类型分开的读数"
	default:
		return "⭐ 目录与文件**不同档**（" + tagDir + " vs " + tagFile +
			"）⇒ 幂等这一格**按类型**分，成文与清理链的判断都要分开写，不许折成一句"
	}
}

// liveProbeRetrashTagOf 从判定句里取出 `[幂等=…]` / `[回执=…]` 那一枚短标（取不到返回空串）。
func liveProbeRetrashTagOf(word string) string {
	start := strings.Index(word, "[")
	if start < 0 {
		return ""
	}
	end := strings.Index(word[start:], "]")
	if end < 0 {
		return ""
	}
	return word[start : start+end+1]
}

// liveProbeRetrashReplyWord 对照同一枚对象的第二发与第一发回执。
//
// ⚠ 这一句**不是**幂等判据：回执相同不等于"没有多记条目"，回执不同也不等于"多记了"——
// 条数那一格在 [liveProbeRetrashWord]，这一格只回答"上游对第二发念的是同一句还是另一句"。
// 分开成两句是因为第廿八/廿九趟学到：一格里混两个判据，读的人会把其中一个当另一个。
func liveProbeRetrashReplyWord(first, second liveProbeRetrashReply) string {
	switch {
	case !first.fired || !second.fired:
		return "[回执=未答] 两发里有一发**没发出去**（第一发 fired=" +
			strconv.FormatBool(first.fired) + " / 第二发 fired=" + strconv.FormatBool(second.fired) +
			"）⇒ 没有可比的回执"
	case !first.shot || !second.shot:
		return "[回执=未答] 两发里有一发没拿到 HTTP 回执（第一发=" + strconv.FormatBool(first.shot) +
			" / 第二发=" + strconv.FormatBool(second.shot) + "）⇒ 只比得上「打通没打通」这一格"
	case first.cells == second.cells:
		return "[回执=与第一发逐字同] 两发都是 " + second.cells + " ⇒ 上游对第二发念的是同一句" +
			"（⚠ 这**不**证明它没多做任何事，条数那一格看 ⑤）"
	default:
		return "[回执=与第一发不同] 第一发 " + first.cells + " / 第二发 " + second.cells +
			" ⇒ 上游**认得出**这是重复请求（这一档本身就是要登记的读数）"
	}
}

// TestLiveProbeSameFileRetrashReread ⭐ **只读**复跑：回头看本族写探针留下的那些 `retrash*` 条目。
//
// 为什么要有它（沿用第廿八/廿九趟那一族的做法）：写探针每复跑一次都**新增残留**（打四枪、
// 造两枚），而"隔一段时间再看同一格"这一问不需要任何写：
// 我方那几枚的名字中段是固定的（`retrashdir-` / `retrashfile-`），按中段认就能回答
// "这一族在回收站里现在有几条 / 有没有自己消失过"。
//
// ⚠ 按**中段**只可能数出"这一族"，不可能数出"本趟那一枚"（#196）⇒ 所以这一支
// **只念族级读数**，不判幂等：条数、逐枚整名与入站时间都念出来，判"重复入账有没有继续发生"
// 要对着"同一整名出现几次"那一格看（这一支会逐枚念整名，因为我们自己的名字可打印）。
func TestLiveProbeSameFileRetrashReread(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器：%s", binErr(sink, uerr))
	}

	page, lerr := run.Upstream.AppCloudRecycleBinList(ctx, uploader.Auth, uploader.DeviceID, "")
	if lerr != nil {
		sink.say("⓪ 只读复跑：第一页就没打通：%s ⇒ 这一趟没有任何读数", binErr(sink, lerr))
		recorder.sayTally()
		return
	}
	var dirHits, fileHits, ours int
	byName := map[string]int{}
	times := map[string]string{}
	for _, entry := range page.Items {
		if entry.Name == nil {
			continue
		}
		if !strings.HasPrefix(*entry.Name, CloudUploadNamePrefix) {
			continue // 用户条目一个字段都不进输出：只按前缀数我方那几枚
		}
		ours++
		switch {
		case strings.Contains(*entry.Name, liveProbeRetrashDirInfix):
			dirHits++
			byName[*entry.Name]++
		case strings.Contains(*entry.Name, liveProbeRetrashFileInfix):
			fileHits++
			byName[*entry.Name]++
		}
		if entry.TrashedAt != nil {
			times[*entry.Name] = *entry.TrashedAt
		}
	}
	names := make([]string, 0, len(byName))
	for name := range byName {
		names = append(names, name)
	}
	sort.Strings(names)
	sink.say("⓪ 只读复跑：第一页 %d 条 / 我方前缀 %d 条；这一族（中段 %q / %q）命中 目录系 %d 条、文件系 %d 条",
		len(page.Items), ours, liveProbeRetrashDirInfix, liveProbeRetrashFileInfix, dirHits, fileHits)
	for _, name := range names {
		sink.say("   整名 %q 出现 %d 次  trashedAt=%s", name, byName[name], times[name])
	}
	if dirHits == 0 && fileHits == 0 {
		sink.say("⓪ ⇒ 这一族今天**不在**回收站里（第一页；游标到底=%v）⇒ 两种读法分不开："+
			"写探针那几趟没打过 / 打了但已被还原或彻底删除（本仓无这两口）⇒ 这一趟不判任何事",
			page.NextPageCursor == "")
	} else {
		sink.say("⓪ ⇒ 上面「同一整名出现几次」那一格就是这一问的读数：" +
			"同一整名 1 次 ⇒ 与写探针 ⑤ 那档相容；2 次 ⇒ 重复入账在**跨趟**上也成立（另当别论：" +
			"这里数的是**同一趟内第一页**，不是「同一枚打两发」那一问）")
	}
	if n := recorder.countFor(liveProbeBinPath); n == 0 {
		sink.say("⚠ 账本里连 `%s` 都没有 ⇒ 上面那些读数不是这一口给的，别用", liveProbeBinPath)
	}
	if n := recorder.countFor(liveProbeBatchTrashPath); n != 0 {
		t.Errorf("只读复跑打了 %d 发 batchTrash ⇒ 这一支的前提是「残留不增加」，写枪一进来就不再能反复跑", n)
	}
	recorder.sayTally()
	sink.say("这一趟只发只读那一口 ⇒ 残留没有增加，可反复跑")
}

// ── 纯单元测试（不打上游，钉判定分档与短标）──

func TestLiveProbeRetrashWord(t *testing.T) {
	okRead := func(hits int) liveProbeRetrashRead {
		return liveProbeRetrashRead{ok: true, hits: hits}
	}
	cases := []struct {
		why                    string
		control, first, second liveProbeRetrashRead
		want                   string
	}{
		{"第一发之后 1 条、第二发之后仍 1 条", okRead(0), okRead(1), okRead(1), "[幂等=仍一条]"},
		{"第二发之后变 2 条", okRead(0), okRead(1), okRead(2), "[幂等=重复入账]"},
		{"第二发之后变 0 条", okRead(0), okRead(1), okRead(0), "[幂等=条目消失]"},
		{"删之前就有同名条目 ⇒ 判据不成立", okRead(1), okRead(2), okRead(2), "[幂等=未答]"},
		{"第一发之后就没进 ⇒ 幂等落空", okRead(0), okRead(0), okRead(0), "[幂等=未答]"},
		{"有一遍没打通 ⇒ 没有条数可比", okRead(0), liveProbeRetrashRead{}, okRead(1), "[幂等=未答]"},
		{"多记不止一条（3→5）也算重复入账", okRead(0), okRead(3), okRead(5), "[幂等=重复入账]"},
		{"第二发之后少了但没到 0（2→1）⇒ 不硬解释", okRead(0), okRead(2), okRead(1), "[幂等=另一档]"},
	}
	for _, c := range cases {
		got := liveProbeRetrashWord("目录", c.control, c.first, c.second)
		if !strings.Contains(got, c.want) {
			t.Errorf("%s ⇒ 判定行 %q 里没有 %s", c.why, got, c.want)
		}
		// 短标只能有**一枚**：两枚并排说明这一句在同时宣布两个结论（#201② 同族）。
		if n := strings.Count(got, "[幂等="); n != 1 {
			t.Errorf("%s ⇒ 判定行里有 %d 枚短标，应当只有 1 枚：%q", c.why, n, got)
		}
	}
}

func TestLiveProbeRetrashSameWord(t *testing.T) {
	dir := liveProbeRetrashWord("目录", liveProbeRetrashRead{ok: true},
		liveProbeRetrashRead{ok: true, hits: 1}, liveProbeRetrashRead{ok: true, hits: 1})
	file := liveProbeRetrashWord("文件", liveProbeRetrashRead{ok: true},
		liveProbeRetrashRead{ok: true, hits: 1}, liveProbeRetrashRead{ok: true, hits: 2})
	same := liveProbeRetrashWord("文件", liveProbeRetrashRead{ok: true},
		liveProbeRetrashRead{ok: true, hits: 1}, liveProbeRetrashRead{ok: true, hits: 1})
	if got := liveProbeRetrashSameWord(dir, same); !strings.Contains(got, "同一档") {
		t.Errorf("两枚同档却念成不同：%s", got)
	}
	if got := liveProbeRetrashSameWord(dir, file); !strings.Contains(got, "不同档") {
		t.Errorf("两枚不同档却念成同一：%s", got)
	}
	if got := liveProbeRetrashSameWord(dir, "这句没有短标"); !strings.Contains(got, "这句不判") {
		t.Errorf("一枚没短标却仍然给了结论：%s", got)
	}
}

func TestLiveProbeRetrashReplyWord(t *testing.T) {
	first := liveProbeRetrashReply{fired: true, shot: true, cells: `code="0000" success=true msg="成功"`}
	same := liveProbeRetrashReply{fired: true, shot: true, cells: `code="0000" success=true msg="成功"`}
	other := liveProbeRetrashReply{fired: true, shot: true, cells: `code="1000" success=false msg="文件不存在"`}
	noShot := liveProbeRetrashReply{fired: true}
	notFired := liveProbeRetrashReply{}
	cases := []struct {
		why  string
		a, b liveProbeRetrashReply
		want string
	}{
		{"两发回执逐字相同", first, same, "[回执=与第一发逐字同]"},
		{"第二发回另一句", first, other, "[回执=与第一发不同]"},
		{"第二发没拿到回执", first, noShot, "[回执=未答]"},
		{"第一发就没发出去", notFired, other, "[回执=未答]"},
	}
	for _, c := range cases {
		got := liveProbeRetrashReplyWord(c.a, c.b)
		if !strings.Contains(got, c.want) {
			t.Errorf("%s ⇒ %q 里没有 %s", c.why, got, c.want)
		}
		if n := strings.Count(got, "[回执="); n != 1 {
			t.Errorf("%s ⇒ 短标出现 %d 次：%q", c.why, n, got)
		}
	}
}

// TestLiveProbeRetrashEnvelopeWord 钉住**新开的这一格打印面**（回执原文第一次进日志）。
//
// 三条各挡一种事故：
//   - ①②：三格得走 [liveProbeShapeValue] 那把尺（`code`/`msg` 在"值可打印"名单里 ⇒ 带引号，
//     `success` 是布尔 ⇒ 带类型标）。这一格是"第二发到底回了什么"的**唯一**读数来源，
//     念错形态就等于读错事实（`code` 有 `"0000"` 与 `0000` 两种写法，见坑表 R26 那一族）；
//   - ③：**缺键**要念成缺键而不是空串 —— `{"code":"0000"}` 与 `{"code":"0000","msg":""}` 是两档；
//   - ④：超长那句（上游把整段 HTML 塞进 `msg` 时就是这样）必须被截断，尾部那一格
//     （例子里是一条带 `token=` 的签名入口）**不许**留在输出里。
func TestLiveProbeRetrashEnvelopeWord(t *testing.T) {
	// ① 三格齐全，且 success 是真布尔。
	full := liveProbeRetrashEnvelopeWord(upstream.NewEnvelope(map[string]any{
		"code": "0000", "success": true, "msg": "成功",
	}))
	for _, want := range []string{`code="0000"`, `success=true(布尔)`, `msg="成功"`} {
		if !strings.Contains(full, want) {
			t.Errorf("三格齐全那份念成 %q，缺 %s", full, want)
		}
	}

	// ② `code` 是数字形状时不能念成字符串（那一档决定"我方判不判失败"）。
	numeric := liveProbeRetrashEnvelopeWord(upstream.NewEnvelope(map[string]any{"code": json.Number("0")}))
	if !strings.Contains(numeric, `code=0(数字)`) {
		t.Errorf("数字 code 念成 %q ⇒ 与字符串 \"0\" 分不开", numeric)
	}

	// ③ 只有 code ⇒ 另两格念「缺键」。
	missing := liveProbeRetrashEnvelopeWord(upstream.NewEnvelope(map[string]any{"code": "0000"}))
	if !strings.Contains(missing, "success=缺键") || !strings.Contains(missing, "msg=缺键") {
		t.Errorf("缺键那份念成 %q ⇒ 与「空串」两档混了", missing)
	}

	// ④ 超长 msg：只念前 24 字，尾部那条带凭据的入口一个字都不留。
	long := strings.Repeat("长", 60) + " https://example.com/一次性签名入口?token=abcdef"
	clipped := liveProbeRetrashEnvelopeWord(upstream.NewEnvelope(map[string]any{"msg": long}))
	if !strings.Contains(clipped, "…") {
		t.Errorf("超长那句没念成截断：%s", clipped)
	}
	if strings.Contains(clipped, "example.com") || strings.Contains(clipped, "token=") {
		t.Errorf("截断之后仍留着 URL 那一格（%s）⇒ 打印面新开的这一格必须过截断", clipped)
	}

	// ⑤ 回执不是对象（上游直接回数组时）⇒ 明说，不硬当三格读。
	array := liveProbeRetrashEnvelopeWord(upstream.NewEnvelope([]any{map[string]any{"code": "0000"}}))
	if !strings.Contains(array, "回执不是对象") {
		t.Errorf("数组回执念成 %q ⇒ 把「没有信封」读成了「信封都空」", array)
	}
}
