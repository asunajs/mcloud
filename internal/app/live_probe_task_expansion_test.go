package app

// 活体探针：「任务拓展」（`task-expansion`）那一族真打上游一次。
//
// # 这一族缺的是哪两格读数
//
//   - **读口**（`GET /ycloud/signin/page/taskExpansion`）有实拍，但**只有一条**、
//     而且那一条**不含 `acceptDate`** ⇒ "这份响应到底有哪些键"仍未收口。
//     本探针第一档就是拿**真实响应形状**回答它（[liveProbeSink.dump] 只出形状）。
//   - **凑备份那一支**（`curMonthBackup === false` ⇒ 往网盘传一个文件）**零实拍**：
//     夹具那条压根没进这一支。它今天能不能进，取决于这个号当月开没开备份 ——
//     **不由探针决定**。所以探针可能只报"跳过凑备份"，那一行本身就是读数
//     （登记成"这一族仍缺一条 backup=false 的号"，见 `docs/pending-decisions.md`）。
//   - **领取口**（`receiveTaskExpansion`）**零实拍**：连"`cloudCount` 给不给"都不知道。
//     它只在 `curMonthTaskRecordCount > 0` 时才发 ⇒ 同样可能这一趟碰不上。
//
// # 附带的第二枪（只读，专门用来对照同一枚键）
//
// `infoV3` 里有一枚**同名**的 `curMonthBackup`（签到翻倍那一族的门槛，见
// [SignInStatus.CurMonthBackup]）。两处的值**是否同一件事**零样本，而它直接决定
// "凑备份那一支的判据是不是我们以为的那个"：两张表若读出不一样，那"传一个文件"
// 刷的是哪一格就成了问题。这一枪只是读，代价是一发。
//
// # 边界
//
//   - 这一族会**写用户网盘**（传一个文件）且这条链**不删**它（见 `task_expansion_run.go`
//     文件头那一节）⇒ 探针跑完把**文件名与 fileId 念进事件流**，由人自己去网盘处置；
//   - 会**等那 20 秒**（只在真传了东西之后）：探针刻意用生产默认档，
//     因为"20 秒够不够服务端把这次备份记进当月"就是本族要收的那一格；
//   - 探针不写库（换票与建会话都不落库），只读副本库里的凭据。
//
// # ④ 那一档要另开一枚开关（`MCLOUD_LIVE_PROBE_TE_UPLOAD=1`）
//
// ③ 走的是生产路径，而它**进不进凑备份那一支由账号当月开没开备份决定** —— 2026-09-26
// 那两个测试号都读成 `curMonthBackup=true` ⇒ 生产路径永远碰不到上传那一枪。
// 这一族还有**另一格**空着，与"能不能刷成 true"无关：
//
// **`backup` 档（`x-yun-op-type: 2` / `x-yun-sub-op-type: 200`）在真上游走不走得通**。
// 本仓此前所有真跑上传都走的是 `upload` 档（`1`/`100`，见 `task_matrix_run.go` 那些形状），
// `2`/`200` 一次样本都没有 —— 而这两枚头一旦与上游要求不符，表现就是
// **这一族唯一那条写网盘的链一枪都发不出去**（或发出去被判成普通上传，那就更不是"备份"了）。
//
// 所以 ④ 直接调**生产那一层**（[TaskExpansionRun.uploadBackup]）强制传一个，
// 再把默认那 20 秒等完、重读一次，念出档位 / 产物 / 重读读数。
// ⚠ 它**不收**「`false` 会不会被这一个文件刷成 `true`」那一问 —— 那需要一个当月
// 未开备份的号；这里拿到的只是"`true` 的号上再传一个会怎样"。
// 开关独立成一是因为这一档**在用户网盘里多留一个文件**且这条链不删；
// **agent 侧默认（非用户要求）**是不带开关就一个字都不写。

import (
	"context"
	"os"
	"testing"

	"github.com/catlair/mcloud/internal/upstream"
)

func TestLiveProbeTaskExpansion(t *testing.T) {
	matrix, account, sink, client := liveProbeWorld(t)
	world := matrix.SignInWorld
	ctx := context.Background()

	opening, perr := world.Open(ctx, account)
	if perr != nil {
		t.Fatalf("装配签到页那份世界失败：%s", sink.mask(perr.Error()))
	}
	sink.see(opening.Credential.Authorization())
	sink.see(opening.Credential.Phone())

	session, serr := opening.Source.Session(ctx)
	if serr != nil {
		t.Fatalf("建会话失败：%s", sink.mask(serr.Error()))
	}
	sink.see(session.jwt)

	// deviceid：读口**可以**不带（它是可选参数），但上传那一族每一枪都带 ⇒
	// 这里把"换得到换不到"当成一档读数念出来，而不是当错误。
	deviceID, derr := world.DeviceIDFor(ctx, account)
	var deviceArg *string
	if derr == nil && deviceID != "" {
		deviceArg = &deviceID
		sink.say("deviceId：换到了（长度 %d，值不打印）", len(deviceID))
	} else {
		// 两种"换不到"分开念：报错的那一条是上游/库侧的事，空串那一条是我们自己
		// 库里没这一格 —— 它们将来都要往不同的地方查。
		reason := "库里没有这一格（空串）"
		if derr != nil {
			reason = sink.mask(derr.Error())
		}
		sink.say("deviceId：换不到（%s）⇒ 凑备份那一支会被整段挡下", reason)
	}

	// ── ① 读口的**真实响应形状**（本族唯一有实拍的那一口，键集合仍未收口）
	request := upstream.TaskExpansionRequest(session.jwt, session.referer, deviceArg)
	sink.say("① 请求：GET %s（URL 不带参数 ⇒ 无查询串），头 %d 枚",
		request.URL, len(request.Headers))
	response, terr := client.Send(ctx, request)
	if terr != nil {
		sink.say("① 传输层失败：%s", sink.mask(terr.Error()))
		t.FailNow()
	}
	sink.dump("① taskExpansion 真实响应", response)
	envelope, eerr := upstream.ParseEnvelope(response.Body)
	if eerr != nil {
		sink.say("① 信封解不出来：%s", eerr.Error())
		t.FailNow()
	}
	reading, rerr := upstream.ParseTaskExpansion(envelope)
	if rerr != nil {
		sink.say("① 我们的解析器读不出：%s ⇒ ② 那一趟仍会自己再读一次", rerr.Error())
	} else {
		sink.say("① 解析器读数：%s", taskExpansionReadings(reading))
		sink.say("① 另外两格：preMonthBackup=%s curMonthBackupTaskAccept=%s",
			liveProbeBoolLabel(reading.PreMonthBackup),
			liveProbeBoolLabel(reading.CurMonthBackupTaskAccept))
	}

	// ── ② 同名键的**对照枪**（只读）：`infoV3` 那一份 `curMonthBackup`
	status, sterr := world.Upstream.AppSignInStatus(ctx, session.jwt, session.referer)
	if sterr != nil {
		sink.say("② infoV3 读失败：%s ⇒ 这一格对照没拿到", sink.mask(sterr.Error()))
	} else {
		sink.say("② infoV3.curMonthBackup=%s（与 ① 那一份**是否同一件事**零样本 ⇒ "+
			"两边不一致时不许把 ① 的判据当备份开关用）",
			liveProbeBoolLabel(status.CurMonthBackup))
	}

	// ── ③ **生产路径**整条链（读 → 可能上传 → 可能等 20 秒 → 可能重读 → 可能领）
	//
	// 这一档才是 AGENTS.md §1.14 要的那个读数："程序自己说了什么"。
	// 等待用生产默认档（`Wait: nil` ⇒ [RealWaiter]、`BackupWait: 0` ⇒ 20 秒），
	// 因为"这一等够不够"本身就是待收的那一格，压成零就测不到了。
	run := TaskExpansionRun{SignInWorld: world}
	result, perr := run.RunAccount(ctx, account)
	if perr != nil {
		sink.say("③ 整条链交回错误：%s", sink.mask(perr.Error()))
	}
	sink.say("③ 档位=%v 上传=%v 读了几遍=%d 凑过几次=%d 实际等了=%v",
		result.Status, result.Upload, result.Reads, result.BackupRetries, result.WaitedTime)
	sink.say("③ 领到的朵数=%v 备份仍未开=%v", result.CloudCount, result.BackupStillOff)
	if result.Reason != "" {
		sink.say("③ 原因：%s", result.Reason)
	}
	if result.Error != "" {
		sink.say("③ 错误：%s", sink.mask(result.Error))
	}
	for _, note := range result.Notes {
		sink.say("③ 附注：%s", sink.mask(note))
	}
	sink.say("③ 小结：%s", result.SummaryLine())
	report := result.RunReport()
	sink.say("③ 四态=%v 待办 %d 条", report.Outcome, len(report.ManualSteps))
	if result.UploadFileID != "" {
		// ⚠ 这一行**是**产物清单：这一条链不删自己写的文件（理由见文件头那一节）。
		sink.say("③ 产物：网盘里留下 %q（fileId 长度 %d）⇒ **没有**自动清理，"+
			"要删请人去网盘或回收站（回收站那一口今天只接在 app-task 链尾）",
			result.UploadName, len(result.UploadFileID))
	}
	// 读数的落点说清楚：哪些问句这一趟答了、哪些仍空着。
	if result.Upload == TaskExpansionUploadNotNeeded {
		sink.say("③ 这一趟**没有**走进凑备份那一支（当月备份那一格没读到 false）⇒ " +
			"「传一个文件之后 curMonthBackup 会不会变 true」「20 秒够不够」两格仍是空档，" +
			"补它需要一个当月未开备份的号")
	}
	if result.Status != TaskExpansionStatusClaimed && result.Status != TaskExpansionStatusClaimFailed {
		sink.say("③ 这一趟**没有**发领取那一发（记录数没读到 >0）⇒ " +
			"「receiveTaskExpansion 回什么形状、cloudCount 给不给」两格仍是空档")
	}

	// ── ④ 强制走一次 `backup` 档上传（默认**不跑**，理由见文件头那一节）
	if os.Getenv("MCLOUD_LIVE_PROBE_TE_UPLOAD") == "1" {
		probeForceBackupUpload(ctx, sink, run, opening, deviceArg)
	} else {
		sink.say("④ 跳过（没带 MCLOUD_LIVE_PROBE_TE_UPLOAD=1）⇒ " +
			"「`backup` 档那两枚头（op-type=2 / sub-op-type=200）在真上游走不走得通」仍无样本")
	}
}

// probeForceBackupUpload 直接调**生产那一层**传一个 `backup` 档文件，等完默认那 20 秒，
// 再按生产路径重读一次 —— 收的是"这一档上传在真上游成不成立"，不是"能不能刷 true"。
//
// 刻意复用 [TaskExpansionRun.uploadBackup] 而不是自己拼四枪：这一档要证的正是
// 那条装配（`Op: CloudOpBackup` + `ParentFileID:"/"` + 带 deviceId）在上游眼里对不对，
// 自己拼会证成另一件事。
func probeForceBackupUpload(
	ctx context.Context, sink *liveProbeSink, run TaskExpansionRun,
	opening SignInOpening, deviceArg *string,
) {
	upload, fileID, name, reason := run.uploadBackup(ctx, opening, deviceArg)
	sink.say("④ 上传档位=%v（念出来：%s）", upload, upload.word())
	if reason != "" {
		sink.say("④ 原因：%s", sink.mask(reason))
	}
	if name != "" {
		sink.say("④ 产物：%q 落在根目录，fileId 长度 %d ⇒ 这条链**不删**它，"+
			"要处置请人去网盘（回收站那一口今天只接在 app-task 链尾）",
			name, len(fileID))
	}
	if !upload.needsWait() {
		sink.say("④ 这一档不需要等 ⇒ 「20 秒够不够」这一格本趟仍无读数")
		return
	}

	wait := run.backupWait()
	if werr := run.waiter().Wait(ctx, wait); werr != nil {
		sink.say("④ 等不住：%s ⇒ 重读没做", sink.mask(werr.Error()))
		return
	}
	sink.say("④ 生产默认档等 %v 后重读（等的就是「服务端有没有把这次备份记进当月」）", wait)
	second, perr := run.read(ctx, opening.Source, deviceArg)
	if perr != nil {
		sink.say("④ 重读失败：%s", sink.mask(perr.Error()))
		return
	}
	sink.say("④ 重读读数：%s", taskExpansionReadings(second))
	sink.say("④ 这一趟答不了的那一问：这个号**上传前**就读成 curMonthBackup=true ⇒ " +
		"读数没变**不能**说明「传文件刷不动备份」；要收那一格需要一个当月未开备份的号")

	// ── ⑤ 把自己刚传的那一枚移进回收站（默认**不跑**）
	//
	// 生产链**不删**（理由见 `task_expansion_run.go` 文件头那一节，那是待拍板的问题），
	// 但探针留下的东西不该算进用户的网盘 —— 所以这一腿既是**清理**，也是**读数**：
	// 「传上去的文件移进回收站之后，当月备份读数会不会回滚」今天**零样本**，
	// 而它正是"要不要接自动清理"那道拍板缺的那一格依据。
	if os.Getenv("MCLOUD_LIVE_PROBE_TE_CLEANUP") != "1" {
		if fileID != "" {
			sink.say("⑤ 跳过（没带 MCLOUD_LIVE_PROBE_TE_CLEANUP=1）⇒ 网盘里留着 ④ 那一枚，" +
				"且「删掉之后当月读数会不会回滚」仍无读数")
		}
		return
	}
	if fileID == "" {
		sink.say("⑤ 没有可清理的产物（④ 那一档没给出 fileId）")
		return
	}
	uploader := CloudUploader{
		Up:       run.Upstream,
		Auth:     opening.Credential.Authorization(),
		DeviceID: *deviceArg,
	}
	sent, terr2 := uploader.DeleteCloudFiles(ctx, []string{fileID})
	sink.say("⑤ 清理那一发：发过=%v 结果=%s", sent, func() string {
		if terr2 != nil {
			return "失败 " + sink.mask(terr2.Error())
		}
		return "上游认了"
	}())
	after, aerr := run.read(ctx, opening.Source, deviceArg)
	if aerr != nil {
		sink.say("⑤ 清理后重读失败：%s ⇒ 「删掉之后读数会不会回滚」这一格没拿到",
			sink.mask(aerr.Error()))
		return
	}
	sink.say("⑤ 清理后读数：%s（对照 ④ 那一份看 curMonthBackup 与两个计数有无变化 —— "+
		"**只对照，不下结论**：这一格今天只有一条样本）", taskExpansionReadings(after))
}

// liveProbeBoolLabel 把 `*bool` 念成"缺 / true / false"三档 ——
// "上游没给这一格"与"给了 false"在这一族里是两个处置（见 [taskExpansionShouldUpload]），
// 探针的读数不许把它们并成一格。
func liveProbeBoolLabel(value *bool) string {
	if value == nil {
		return "缺"
	}
	if *value {
		return "true"
	}
	return "false"
}
