// 第卅二趟活体探针：`fileRenameMode:"refuse"` **撞上同名目录**那一档上游回什么。
//
// # 这一问从哪来（R7 成文里剩下的最后一格）
//
// R7 已于 2026-09-26 的四趟真跑闭合（capture §10／§11／§12／§13／**§14**），但那一格旁边
// 一直挂着同一句"剩下的只有一格"：`fileRenameMode:"refuse"` 撞同名目录时上游回什么。
// 出处不是猜的 —— [upstream.CloudRenameModeRefuse] 的注释写着"同名目录已存在时上游不另起一个，
// 而是在响应里给 `data.exist:true` + **已有那枚** `fileId`"，而那一整段的**唯一来源**是
// 实拍那两条 `type:"folder"`（`test/fixtures/http/3d.json`）：本仓自己**一次都没真打过**这一档 ——
// 前面五趟造对象的探针（第廿六／廿八／廿九／卅／卅一趟）全用随机 hex 名，每一发读到的都是
// `exist:false` ⇒ 那一格到今天仍是**零样本**。
//
// 为什么这一格值得单独跑一趟（不是"把实拍再念一遍"）：
//   - 生产链**读的就是这一格**：[CloudUploader.Upload] 在 `created.Exist` 时走
//     [CloudUploadExists] 那一支（不上传、不记账，见 `cloud_upload.go`），而
//     `ArtifactLedger.RegisterCreated` 拿同一格决定"这条产物是不是本来就属于用户"
//     ⇒ 判反了的表现是"清理链把用户原有的对象删掉"，不是少一条日志；
//   - 它还是那五趟探针各自那条边界的出处：每一支都写着「`exist:true` ⇒ 当场收手不删」。
//     那条收手指令**从来没有一次真的被上游触发过** ⇒ "收手时手里有什么信息"（有没有 fileId、
//     回执算成功还是算失败）我们并不知道，而它决定收手那一支还要不要补一发删除。
//
// ⚠ 与"一趟只答一问"的纪律不冲突：三格读数（`exist` / 回的是不是**同一枚** fileId /
// 网盘侧这一整名有几枚）回答的是**同一个问**——"撞名时上游怎么处置"。
//
// # 形状（两发 create 参数**逐字相同**）
//
//   - ⓪ **先读网盘根目录**：这一整名命中几枚（对照。随机名撞上一趟残留的概率极低，
//     但这一格是"下面那两发撞的是谁"的唯一依据 ⇒ 命中数 > 0 时**当场收手**，见边界第二条）；
//   - ① 第一发 `file/create`（`type:"folder"` + `fileRenameMode:"refuse"`）—— 占名，不是判据；
//   - ② 第二发**完全相同**的请求（同 parentFileId、同 name、同四键 body）—— 这一发才是本问；
//   - ③ 再读一次网盘根目录：同一整名现在几枚（1 枚 ⇒ "不另起一个"；2 枚 ⇒ refuse 又建了一枚）；
//   - ⑤ 收场：把本趟造出的那枚（或那两枚）`batchTrash` 收走。
//
// 两发都走 [upstream.Upstream.Call] + [upstream.ParseCloudFileCreate] 这一条**手工**路，
// 不走 `AppCloudFolderCreate` 那个组合口：后者把信封折没了，而"第二发回的那三格
// （`code` / `success` / `msg`）算成功还是算失败"正是本问要读的东西之一（与第卅趟
// [liveProbeRetrashTrash] 同一处置，理由也同：两发走同一条路，差的才只是上游）。
//
// # 判定分档（纯函数，不跑上游就有表钉着）
//
// [liveProbeRefuseWord] 把两发读数 + 前后两次网盘侧枚数折成一句带 `[refuse=…]` 短标的判定：
// 「回已有那枚」／「又建一枚」／「撞名回失败」／「exist 但给了别的 id」／「自相矛盾」／「未答」。
// ⭐ 短标是**成文口径**：capture 与 status 只抄短标，不重述散文（坑表 #201② 的教训）。
//
// # 写面与残留
//
// 造 1～2 枚空目录（第二枚只在"refuse 其实另起一枚"那一档才会出现）、打 1 发 `batchTrash`
// ⇒ 正常路径下网盘侧**不留东西**，回收站里净增 1 条（R7：空目录删一次留一条）。
// 本仓无"彻底删除"口 ⇒ 回收站那一条并入 **R37ⓒ** 那批待人工处理。
//
// # 凭据与隐私边界（沿用第廿七～卅一趟那一族，一条不放宽）
//
//   - 名字只用 `asign-` 前缀（我们自己造的，可整名打印）；网盘侧那一页**只数命中枚数**，
//     用户条目的名字与任何字段值都不进输出；
//   - 真实 `fileId` 一律 `sink.see` 登记后**只念长度**；"两发回的是不是同一枚"只念
//     `同一枚=true/false`（比较在内存里做，值不外流）；
//   - 回执 `msg` 与错误原文过 [liveProbeSink.mask] 并截断（#195 同族：错误文本是第二条泄漏通路）；
//   - `file/list` 那一页的 `nextPageCursor` **值不打印**，只念"有没有下一页游标"——
//     这一口今天不翻页（[upstream.AppCloudFileList] 固定一页），所以命中数只在"没有下一页"时才穷尽。

package app

import (
	"context"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// liveProbeRefuseDirInfix 是本趟那枚目录名的中段（整名带 `asign-` ⇒ 可打印）。
	// 刻意与第卅／卅一趟那几族的中段**互不为前缀**：那几支的只读复跑按中段数条，
	// 混进本趟的对象会把两族的读数搅成一份（`liveProbeRetrashNestDirInfix` 那里同一条理由）。
	liveProbeRefuseDirInfix = "refusedir-"
	// liveProbeFileListPath 只为最后那句"这一族今天发了几枪"的账，不是判据。
	liveProbeFileListPath = "/hcy/file/list"
)

// liveProbeRefuseShot 是**一发** `file/create`（folder 型）的读数。
//
// 为什么把"打通没"（`shot`）与"解析成没成"（`parsed`）分成两格：撞名那一档最可能的两种
// 非预期读数分别是"HTTP 层就挂"与"信封回的是失败码"，而它们在成文里是**两档不同的结论**
// （前者根本没有读数；后者本身就是答案 —— 上游把撞名当失败，`exist` 那一格永远读不到）。
type liveProbeRefuseShot struct {
	fired     bool   // 请求拼出来了、发出去了
	shot      bool   // 拿到了信封（传输层报错 ⇒ false）
	cells     string // 回执三格短码（code / success / msg，已过掩码与截断）
	parsed    bool   // [upstream.ParseCloudFileCreate] 没报错（=我方判成"这一发成功"）
	errText   string // 没打通／没解析成时的错误原文（已掩码；只进日志，不进成文）
	exist     bool   // `data.exist`
	hasFileID bool   // `data.fileId` 在不在（不在 ⇒ 收场那一发没有对象）
	fileID    string // 真实 fileId：**只在内存里比**，念出来一律走长度
}

// idLen 念 fileId 的长度（真实值不进成文）。
func (s liveProbeRefuseShot) idLen() string {
	if !s.hasFileID || s.fileID == "" {
		return "无"
	}
	return strconv.Itoa(len(s.fileID))
}

func TestLiveProbeRefuseNameCollision(t *testing.T) {
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
		t.Fatalf("装不出网盘上传器（本探针要连发两发建目录 + 一发收场）：%s", binErr(sink, uerr))
	}

	dirName := CloudUploadNamePrefix + liveProbeRefuseDirInfix +
		randomHex(uploader.Rng, cloudRandomNameHexLen)

	// ── ⓪ 网盘侧对照：这个名字**本来**有几枚 ──
	before := liveProbeCloudNameCount(ctx, sink, run, opening, "/", dirName, "⓪ 第一发之前")
	if before.ok && before.hits > 0 {
		// 边界第二条：撞的对象不属于本趟 ⇒ 收手且**不删**（那是上一趟没清干净的残留，
		// 该并进 R37ⓒ 那批一起处置，不该被这一趟顺手动掉）。
		sink.say("⓪ ⇒ 根目录里**已经**有 %d 枚 %q ⇒ 第一发撞的是残留，本趟判据不成立 ⇒ 当场收手（不删）",
			before.hits, dirName)
		recorder.sayTally()
		return
	}

	// ── ① 第一发：把名字占上（不是判据）──
	first := liveProbeRefuseCreate(ctx, sink, run, uploader, "/", dirName, "① 第一发（占名）")
	switch {
	case !first.parsed:
		sink.say("① 第一发就没有读数 ⇒ 名字没占上，第二发无从撞起 ⇒ 本趟不判任何事")
		recorder.sayTally()
		return
	case !first.hasFileID:
		sink.say("① 第一发回 OK 但没有 fileId ⇒ 没有可比对的对象，收手（下面那一发照样发：它回答的是" +
			"「上游认不认第二发」，但「同一枚吗」那一格从此读不出来）")
	case first.exist:
		sink.say("① 第一发就回 exist=true（网盘侧那 %d 枚命中就是它的出处）⇒ 撞名这一档落在**第一发**上，"+
			"而它撞的是本趟之前就在的那一枚 ⇒ 本趟判据不成立，收手不删", before.hits)
		recorder.sayTally()
		return
	}
	sink.say("① 第一发：回执 %s ⇒ fileId 长度 %s、exist=%v（占名成功，第二发撞的就是它）",
		first.cells, first.idLen(), first.exist)

	// ── ② 第二发：参数**逐字相同**再打一遍（本问那一枪）──
	second := liveProbeRefuseCreate(ctx, sink, run, uploader, "/", dirName, "② 第二发（同名，本趟要问的一枪）")

	// ── ③ 网盘侧复查：这一整名现在几枚 ──
	after := liveProbeCloudNameCount(ctx, sink, run, opening, "/", dirName, "③ 两发之后")

	// ── ④ 判定 ──
	word := liveProbeRefuseWord(first, second, before.hits, after.hits)
	sink.say("④ 主问：%s", word)
	sink.say("④ 两发对照（逐字）：第一发 %s ｜ 第二发 %s", first.cells, second.cells)
	if second.errText != "" {
		sink.say("④ 第二发错误原文（已掩码）：%s", second.errText)
	}
	sink.say("④ exist：%v → %v ｜ fileId 长度：%s → %s ｜ 同一枚=%v ｜ 网盘侧整名枚数：%d → %d ⇒ "+
		"「同一枚」是**内存里比**出来的，两串真实 fileId 都不进输出",
		first.exist, second.exist, first.idLen(), second.idLen(),
		first.fileID != "" && first.fileID == second.fileID, before.hits, after.hits)
	sink.say("④ 网盘侧那两遍是否穷尽（这一口不翻页 ⇒ 有下一页游标时命中数只是**下界**）：第一遍 %s ｜ 第二遍 %s",
		liveProbeCloudExhaustedWord(before), liveProbeCloudExhaustedWord(after))

	// ── ⑤ 收场：把本趟造出的那枚（或那两枚）收走。⓪ 已证这个名字原本是空的 ⇒ 命中的都是我们的 ──
	targets := liveProbeRefuseCleanupIDs(first, second)
	if len(targets) == 0 {
		sink.say("⑤ 收场：手里**没有**任何 fileId 可用（两发都没回 id）⇒ 这一枚留在用户网盘里，并入 R37ⓒ")
	} else {
		sink.say("⑤ 收场名单 %d 枚（长度 %s）⇒ 打一发 `batchTrash`", len(targets), liveProbeIDLensWord(targets))
		sent, derr := uploader.DeleteCloudFiles(ctx, targets)
		if derr != nil || !sent {
			sink.say("  收场那一发：发过=%v ⇒ %s（网盘侧可能留着本趟那枚目录，并入 R37ⓒ）",
				sent, binErr(sink, derr))
		} else {
			sink.say("  收场那一发上游认了（回执见上面 %s 那一枪的账）", liveProbeBatchTrashPath)
		}
	}

	recorder.sayTally()
	sink.say("这一趟 %s 发了 %d 枪、%s 发了 %d 枪、%s 发了 %d 枪 ⇒ 写侧是那两发建目录 + 收场那一发；"+
		"残留 = 回收站里本趟那一枚（或两枚，看 ④ 那个数）；本仓无'彻底删除'口（要清得人去 App 里操作）",
		liveProbeFileCreatePath, recorder.countFor(liveProbeFileCreatePath),
		liveProbeFileListPath, recorder.countFor(liveProbeFileListPath),
		liveProbeBatchTrashPath, recorder.countFor(liveProbeBatchTrashPath))
}

// liveProbeRefuseCleanupIDs 拼收场名单。
//
// 为什么"两枚同 id 时只留一枚"：第卅趟实测过「同一枚点名两次无害」（回收站仍 1 条、不新增条目），
// 但那一格是**那一趟的判据**，不该在本趟顺手当"反正重复也没事"的免检条 —— 名单去重之后
// 这一趟的残留读数只由"上游认了几枚"决定，不由我们的重复提交决定。
func liveProbeRefuseCleanupIDs(first, second liveProbeRefuseShot) []string {
	ids := make([]string, 0, 2)
	for _, shot := range []liveProbeRefuseShot{first, second} {
		if shot.fileID == "" {
			continue
		}
		if len(ids) > 0 && ids[0] == shot.fileID {
			continue
		}
		ids = append(ids, shot.fileID)
	}
	return ids
}

// TestLiveProbeRefuseCleanupIDs 钉住名单那三档（不跑上游）。
//
// 反向改坏它：去掉 `ids[0] == shot.fileID` 那一支 ⇒ "两发回同一枚 id"那行会红（名单变两枚，
// 而那一档的正解是"上游只该被点名一次"）。
func TestLiveProbeRefuseCleanupIDs(t *testing.T) {
	withID := func(id string) liveProbeRefuseShot {
		return liveProbeRefuseShot{fileID: id}
	}
	cases := []struct {
		name          string
		first, second liveProbeRefuseShot
		want          int
	}{
		{"两发同一枚 id ⇒ 名单只有一枚", withID("AAAA1111"), withID("AAAA1111"), 1},
		{"两发不同 id ⇒ 两枚都收", withID("AAAA1111"), withID("BBBB2222"), 2},
		{"第二发没回 id ⇒ 只收第一发那枚", withID("AAAA1111"), liveProbeRefuseShot{}, 1},
		{"两发都没回 id ⇒ 名单为空（收场那一发不发）", liveProbeRefuseShot{}, liveProbeRefuseShot{}, 0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := liveProbeRefuseCleanupIDs(tc.first, tc.second)
			if len(got) != tc.want {
				t.Errorf("名单实得 %d 枚（%v），期望 %d 枚", len(got), liveProbeIDLensWord(got), tc.want)
			}
		})
	}
}

// liveProbeRefuseCreate 发**一发**建目录并交回读数（参数与 [upstream.AppCloudFolderCreate] 逐字同）。
//
// 为什么在这里重造一遍组合口已经有的三步：见文件头"两发都走 Call"那一段 —— 组合口把信封折没了，
// 而本问要的正是**第二发的信封那三格**。
// ⚠ 请求体仍由 [upstream.CloudFolderCreateRequest] 单造，不在这里手拼 body：
// 那四键（含 `fileRenameMode:"refuse"`）是本问的**自变量**，拼错了这一趟就白跑。
func liveProbeRefuseCreate(
	ctx context.Context, sink *liveProbeSink, run TaskMatrixRun, uploader CloudUploader,
	parentFileID, name, what string,
) liveProbeRefuseShot {
	out := liveProbeRefuseShot{}
	request, perr := upstream.CloudFolderCreateRequest(
		uploader.Auth, uploader.DeviceID, parentFileID, name,
		upstream.CloudChannelAndroid, upstream.CloudOpUpload)
	if perr != nil {
		sink.say("%s：请求就没拼出来：%s ⇒ 这一发**没发**", what, binErr(sink, perr))
		return out
	}
	out.fired = true
	// 发出时刻要念：万一读回"两枚同名"，唯一的办法是把每一发与网盘侧那一遍的时刻对上
	// （第卅趟那条 #202 的方法格在这里同样成立）。
	sink.say("%s：发出时刻 %s（名字 %q）", what,
		time.Now().Format(liveProbeRetrashClockLayout), strconv.Quote(name))
	envelope, cerr := run.Upstream.Call(ctx, request)
	if cerr != nil {
		out.errText = binErr(sink, cerr)
		sink.say("%s：这一发没打通：%s", what, out.errText)
		return out
	}
	out.shot = true
	out.cells = liveProbeRetrashEnvelopeWord(envelope)
	created, ferr := upstream.ParseCloudFileCreate(envelope)
	if ferr != nil {
		out.errText = binErr(sink, ferr)
		out.parsed = false
		sink.say("%s：回执 %s ⇒ 我方那份解析（[upstream.ParseCloudFileCreate]）读成**失败**：%s",
			what, out.cells, out.errText)
		return out
	}
	out.parsed = true
	out.exist = created.Exist
	out.hasFileID = created.FileID != nil && *created.FileID != ""
	if created.FileID != nil {
		out.fileID = *created.FileID
		sink.see(out.fileID)
	}
	sink.say("%s：回执 %s ⇒ exist=%v、fileId 长度=%s、uploadId 是否缺=%v、rapidUpload=%v",
		what, out.cells, created.Exist, out.idLen(), created.UploadID == nil, created.RapidUpload)
	return out
}

// liveProbeCloudCount 是**网盘侧**一页的读数（本趟问的是"同一整名有几枚"）。
//
// ⚠ 与 [liveProbeDirPage] 的区别是必要的而不是重复：那一支交回 `map[string]bool`，
// 同名两枚在 map 里**塌成一格** ⇒ 用它数不出枚数，而枚数正是本趟的第三格读数。
type liveProbeCloudCount struct {
	ok        bool     // 这一遍打没打通（没打通 ⇒ hits 不可用）
	hits      int      // 整名命中的**枚数**
	ids       []string // 命中那几枚的真实 fileId（只进内存与 sink.see，不进输出）
	items     int      // 这一页扫到几条
	hasCursor bool     // 上游给了下一页游标 ⇒ 命中数只是下界（本口不翻页）
}

func liveProbeCloudNameCount(
	ctx context.Context, sink *liveProbeSink, run TaskMatrixRun, opening SignInOpening,
	parent, wantName, what string,
) liveProbeCloudCount {
	out := liveProbeCloudCount{}
	page, lerr := run.Upstream.AppCloudFileList(ctx, opening.Profile,
		UserTimestamp(run.Clock), opening.Credential.Authorization(), parent, liveProbeFileListPageSize)
	if lerr != nil {
		sink.say("%s：网盘这一页**没读到**：%s ⇒ 这一遍没有枚数", what, binErr(sink, lerr))
		return out
	}
	out.ok = true
	out.items = len(page.Items)
	out.hasCursor = page.NextPageCursor != nil && *page.NextPageCursor != ""
	for _, file := range page.Items {
		if file.Name != wantName {
			continue // 用户条目一个字段都不进输出：只数我方那一枚
		}
		out.hits++
		sink.see(file.FileID)
		out.ids = append(out.ids, file.FileID)
	}
	sink.say("%s：这一页 %d 条，整名 %q 命中 %d 枚（长度 %s）%s",
		what, out.items, wantName, out.hits, liveProbeIDLensWord(out.ids),
		liveProbeCursorWord(out.hasCursor))
	return out
}

// liveProbeIDLensWord 念一串 fileId 的**长度**清单（真实值不进成文）。
func liveProbeIDLensWord(ids []string) string {
	if len(ids) == 0 {
		return "无"
	}
	parts := make([]string, 0, len(ids))
	for _, id := range ids {
		parts = append(parts, strconv.Itoa(len(id)))
	}
	return strings.Join(parts, "/")
}

// liveProbeCloudExhaustedWord 念"这一遍的命中数是不是穷尽读数"。
//
// 为什么单独一支：`hits=1` 在"没有下一页"时是**答案**，在"有下一页"时只是**下界** ——
// 两者的成文写法不同（后者要写成「1（下界）」）。本口今天不翻页 ⇒ 这一格不能省。
func liveProbeCloudExhaustedWord(read liveProbeCloudCount) string {
	if !read.ok {
		return "没读数"
	}
	if read.hasCursor {
		return "不穷尽（有下一页游标，本口不翻页）"
	}
	return "穷尽（没有下一页游标）"
}

// liveProbeRefuseWord 把两发读数 + 前后两次网盘侧枚数折成一句短标判定。
//
// 反向改坏它：把 `second.fileID == first.fileID` 那一支改成 `second.exist`（恒真）
// ⇒ [TestLiveProbeRefuseWord] 里「exist 但给了别的 id」那一行会红。
func liveProbeRefuseWord(first, second liveProbeRefuseShot, before, after int) string {
	switch {
	case !first.parsed:
		return "[refuse=未答] 第一发（占名那一发）没有读数 ⇒ 名字没占上，第二发撞的是谁无从判断"
	case before > 0:
		return "[refuse=未答] 第一发之前网盘侧就有 " + strconv.Itoa(before) +
			" 枚同名 ⇒ 撞的对象不属于本趟（上一趟残留），这一趟不判任何事"
	case !second.fired:
		return "[refuse=未答] 第二发没发出去（请求就没拼出来）⇒ 本问没有读数"
	case !second.shot:
		return "[refuse=未答] 第二发传输层就没打通 ⇒ 上游回什么没读到（错误原文见 ④ 那一行）"
	case !second.parsed:
		return "[refuse=撞名回失败] 第二发回执 " + second.cells + " ⇒ 上游把「同名已存在」报成**失败**，" +
			"而我方 [upstream.ParseCloudFileCreate] 在读到 `exist` 之前就交回错误 ⇒ 编排层那一支" +
			"`created.Exist` 在这一型上永远走不到（要回查 [CloudUploadExists] 那一档到底怎么被触发）"
	case !second.exist && after >= 2:
		return "[refuse=又建一枚] 第二发没回 exist，而网盘侧同名变成 " + strconv.Itoa(after) +
			" 枚 ⇒ `fileRenameMode:\"refuse\"` **没有**拦住同名 ⇒ 实拍那两条读出来的那句「不另起一个」" +
			"不适用于这一型（要改 [upstream.CloudRenameModeRefuse] 的头注，并回查建目录那一族）"
	case !second.exist && after == 1:
		return "[refuse=自相矛盾] 第二发没回 exist、也没多出一枚（网盘侧仍 1 枚）⇒ 这一发既没被拒也没被接受" +
			"⇒ 对着 ④ 那两行回执复核，不据此改口径"
	case !second.hasFileID:
		return "[refuse=exist但没给id] 第二发回 exist=true 而 `fileId` 缺 ⇒ 收手那一支拿不到可删的对象" +
			"（那五趟探针里「当场收手不删」这一条在这一档是唯一安全读法）"
	case second.fileID == first.fileID:
		if after == 1 {
			return "[refuse=回已有那枚] 第二发回 exist=true 且 fileId 与第一发**同一枚**、网盘侧仍 1 枚 ⇒ " +
				"实拍那两条读出的那句在**本仓真打上**成立：同名不另起一个，交回已有那枚的 id"
		}
		return "[refuse=回已有那枚但形状变了] exist=true 且同一枚 id，但网盘侧枚数=" + strconv.Itoa(after) +
			" ⇒ 同一枚 id 却数出多枚，两种解释（那一页把别的什么算进来了 / 命中数不是穷尽读数）分开不了，" +
			"先查 ④ 那一句「穷尽 / 不穷尽」"
	default:
		return "[refuse=exist但给了别的id] 第二发回 exist=true 而 fileId 与第一发**不同**（长度 " +
			first.idLen() + " → " + second.idLen() + "）、网盘侧 " + strconv.Itoa(after) +
			" 枚 ⇒ 「已有那枚」指的是**另一枚**（同名多枚时上游给谁，成文里没有这一格），" +
			"收场名单要按网盘侧那一遍来"
	}
}

// TestLiveProbeRefuseWord 钉住上面那八档分派（不跑上游）。
//
// 为什么这一支必须有表：判定函数只在真跑时用一次，而本趟最可能读到的恰恰是**非预期**那几档
// （撞名回失败 / exist 但没 id / 又建一枚）—— 分派写反时日志里读得出来，但没人会在
// 一趟几十秒的现场输出里逐字核对八档。
func TestLiveProbeRefuseWord(t *testing.T) {
	base := liveProbeRefuseShot{
		fired: true, shot: true, parsed: true, cells: `code="0000" success=true msg=缺键`,
		exist: false, hasFileID: true, fileID: "AAAA1111",
	}
	second := func(mut func(*liveProbeRefuseShot)) liveProbeRefuseShot {
		out := base
		out.exist = true
		mut(&out)
		return out
	}
	cases := []struct {
		name          string
		first, sec    liveProbeRefuseShot
		before, after int
		want          string
	}{
		{"第一发没解析成 ⇒ 未答（撞的对象无从判断）",
			liveProbeRefuseShot{fired: true}, base, 0, 1, "[refuse=未答]"},
		{"第一发之前就有同名 ⇒ 未答（残留不属于本趟）",
			base, base, 1, 1, "[refuse=未答]"},
		{"第二发没发出去 ⇒ 未答",
			base, liveProbeRefuseShot{}, 0, 1, "[refuse=未答]"},
		{"第二发传输层挂了 ⇒ 未答",
			base, second(func(s *liveProbeRefuseShot) { s.shot = false; s.parsed = false }), 0, 1,
			"[refuse=未答]"},
		{"第二发被判成失败 ⇒ 撞名回失败（exist 那一格读不到）",
			base, second(func(s *liveProbeRefuseShot) { s.parsed = false }), 0, 1, "[refuse=撞名回失败]"},
		{"没回 exist 且变成两枚 ⇒ 又建一枚（refuse 没拦住）",
			base, second(func(s *liveProbeRefuseShot) { s.exist = false }), 0, 2, "[refuse=又建一枚]"},
		{"没回 exist 也没多出一枚 ⇒ 自相矛盾",
			base, second(func(s *liveProbeRefuseShot) { s.exist = false }), 0, 1, "[refuse=自相矛盾]"},
		{"回 exist 但没有 fileId ⇒ 收手那一支没有对象",
			base, second(func(s *liveProbeRefuseShot) { s.hasFileID = false; s.fileID = "" }), 0, 1,
			"[refuse=exist但没给id]"},
		{"回 exist 且同一枚、网盘侧仍 1 枚 ⇒ 实拍那句在本仓成立",
			base, second(func(*liveProbeRefuseShot) {}), 0, 1, "[refuse=回已有那枚]"},
		{"回 exist、同一枚，但网盘侧两枚 ⇒ 形状变了",
			base, second(func(*liveProbeRefuseShot) {}), 0, 2, "[refuse=回已有那枚但形状变了]"},
		{"回 exist 但 fileId 与第一发不同 ⇒ 给了别的 id",
			base, second(func(s *liveProbeRefuseShot) { s.fileID = "BBBB2222" }), 0, 2,
			"[refuse=exist但给了别的id]"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := liveProbeRefuseWord(tc.first, tc.sec, tc.before, tc.after)
			if !strings.HasPrefix(got, tc.want) {
				t.Errorf("档位短标实得 %q，期望以 %q 开头", got, tc.want)
			}
			if liveProbeRetrashTagOf(got) != tc.want {
				t.Errorf("短标取不到：liveProbeRetrashTagOf(%q) = %q", got, liveProbeRetrashTagOf(got))
			}
		})
	}
	// ⚠ 这一行单独钉**分派顺序**："第一发没读数"必须排在"第二发没读数"**之前**。
	// 两档调换时，第一发失败那一趟会被念成"第二发没打通"，而那是两个不同现场（前者连名字
	// 都没占上，后者是撞名那一枪挂了）—— 顺序错了不会少读数，只会把现场念错。
	if !strings.HasPrefix(
		liveProbeRefuseWord(liveProbeRefuseShot{fired: true}, second(func(*liveProbeRefuseShot) {}), 0, 1),
		"[refuse=未答] 第一发") {
		t.Error(`"第一发没读数"那一档的措辞没落在最前面 ⇒ 读者会以为没读数的是第二发`)
	}
}
