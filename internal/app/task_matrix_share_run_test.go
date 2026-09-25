package app

// ─────────────────────────────────────────────────────────────────────────────
// 分享那一半（434：建一条外链 → 立刻取消它）
// ─────────────────────────────────────────────────────────────────────────────
//
// 这一族的形状（消费谁的产物、两枪各发几次）在 `task_matrix_run.go` 的 [runShare]；
// 两口请求与响应判定在 `internal/upstream/app_cloud_outlink.go`（那里有 14 条形状用例）。
// 这一节只判**编排层**：什么时候不发、失败时说了什么、错误文本里带不带凭据。
//
// ⚠ 夹具**全是合成**的 —— 这一族两口在本仓所有归档里零实拍（成文见那个文件的文件头）。
// 所以这些用例钉的是"编排层怎么处置每一种读数"，**不是**"上游会这样回"。
//
// ⚠ [runShare] 里"这一族的 auth 整串取不到 ⇒ 一枪不发"那一档**未被断言**：
// 这一趟的文件 id 只能由上传写入，而上传自己也要求 deviceId ⇒ 拿不到 id 时
// 先撞上"没有可分享的文件"那一档。要断言它得给执行器塞一份手造的 cloudFileIDs，
// 那是内部状态而不是行为，这里不造。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	mxGetOutLinkEndpoint = "/outlink/v1.0/getOutLink"
	mxDelOutLinkEndpoint = "/outlink/v1.0/delOutLink"
	// mxLinkID 进夹具也进断言：它是"删的那一发删的是哪一条"的唯一线索。
	mxLinkID = "SYNTH-LINK-1"
	// mxShareTaskName 是实拍里 434 那一族的名字形状（`N/7` 是多日进度，见 [ShareProgress]）。
	mxShareTaskName = "分享文件有好礼（1/7）"
)

// mxOutLinkBody 造一份 `getOutLink` 的响应。`linkID` 传空串 ⇒ 给**空数组**，
// 也就是"上游建了链接、却没把可删的那一格交回来"那一档（本层交回 `LinkID == nil`，
// 上层**必须**把它播成失败 —— 见 [runShare] 函数头那三处刻意不同）。
func mxOutLinkBody(linkID string) string {
	set := "[]"
	if linkID != "" {
		set = `[{"linkID":"` + linkID + `"}]`
	}
	return `{"code":"0000","message":"成功","success":true,` +
		`"data":{"getOutLinkRes":{"getOutLinkResSet":` + set + `}}}`
}

// mxDelOutLinkOK 是取消分享的成功响应（判据只有码位，本层不读任何字段）。
const mxDelOutLinkOK = `{"code":"0000","message":"成功","success":true,"data":{}}`

// mxOutLinkRejected 是"码位说失败"那一份合成响应。
//
// ⚠ 上游为什么拒、拒的时候回什么形状**都无样本**：这里只要一个非 0 码，用来钉
// 两种失败各自怎么走 —— 建失败 ⇒ 不发删除；删失败 ⇒ 报出来且不补发。
const mxOutLinkRejected = `{"code":"0001","message":"合成拒绝","success":false,"data":{}}`

// mxShareRoutes 给这两枪编路由（两档各自可换：建成的两态 + 删成的两态）。
func mxShareRoutes(getBody, delBody string) []liveTaskRoute {
	return []liveTaskRoute{
		{mxGetOutLinkEndpoint, []string{getBody}},
		{mxDelOutLinkEndpoint, []string{delBody}},
	}
}

// mxShareBodies 按发出顺序取出某一枪的请求体。
func mxShareBodies(script *liveTaskScript, endpoint string) []string {
	var bodies []string
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, endpoint) {
			bodies = append(bodies, string(request.Body))
		}
	}
	return bodies
}

// TestTaskMatrixRunSharesTheUploadedFileThenUnshares ⭐ 主路径：传一个 → 分享它 → 立刻取消。
//
// 反向改坏的四种样子：忘了消费上传那一份产物（getOutLink 零发、或 body 里没有那个
// fileId）、建完不删（delOutLink 零发 ⇒ 用户网盘里留一条公开外链）、多删一发
// （计数恰好 1）、给这一族套上 jwttoken（域规则那条断言）。
func TestTaskMatrixRunSharesTheUploadedFileThenUnshares(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(434, "day", mxShareTaskName, "WAIT", "", 0),
	)
	second := mxItems(mxItem(106, "day", "上传一个文件", "FINISH", "", 1))
	routes := append(mxUploadRoutes(1),
		mxShareRoutes(mxOutLinkBody(mxLinkID), mxDelOutLinkOK)...)
	script := mxScript([]string{"JWT-1"}, first, second, append(routes, mxHappy()...)...)
	wait := &recordingWaiter{}
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxGetOutLinkEndpoint); n != 1 {
		t.Errorf("getOutLink 应恰好一发，实得 %d", n)
	}
	if n := script.count(mxDelOutLinkEndpoint); n != 1 {
		t.Errorf("delOutLink 应恰好一发（建完就删），实得 %d", n)
	}
	// ⭐ 分享的是**这一趟传上去的那个文件**（旧实现 `files[0]`，见 [runShare]）。
	got := mxShareBodies(script, mxGetOutLinkEndpoint)
	if len(got) != 1 || !strings.Contains(got[0], "SYNTH-FID-1") {
		t.Errorf("getOutLink 的文件列表里应是刚传上去的 fileId，实得 %q", got)
	}
	// `commonAccountInfo.account` 用的是登录手机号（旧实现传的就是它）。
	if len(got) == 1 && !strings.Contains(got[0], `"account":"19900000001"`) {
		t.Errorf("body 里要带定位账号的那一格，实得 %q", got[0])
	}
	if dels := mxShareBodies(script, mxDelOutLinkEndpoint); len(dels) == 1 &&
		!strings.Contains(dels[0], mxLinkID) {
		t.Errorf("删的必须是刚建出来那条 linkID，实得 %q", dels[0])
	}
	// 时序：外链那一族排在**上传之后**（它消费的是前面那些任务的产物）。
	var order []string
	for _, url := range mxURLs(script) {
		switch {
		case strings.Contains(url, mxClickEndpoint):
			order = append(order, "click")
		case strings.Contains(url, mxCreateEndpoint):
			order = append(order, "create")
		case strings.Contains(url, mxPutHost):
			order = append(order, "put")
		case strings.Contains(url, mxCompleteEndpoint):
			order = append(order, "complete")
		case strings.Contains(url, mxGetOutLinkEndpoint):
			order = append(order, "get")
		case strings.Contains(url, mxDelOutLinkEndpoint):
			order = append(order, "del")
		}
	}
	if s := strings.Join(order, ">"); s != "click>create>put>complete>click>get>del" {
		t.Errorf("时序不对，实得 %s", s)
	}
	// 两枪的鉴权画像与上传那一族同一条域规则：带整串 Authorization + deviceid，**不**带
	// jwttoken（`yun.139.com` 不在旧 TS 的 jwttoken 主机名单里，成文见 outlink 文件头）。
	for _, endpoint := range []string{mxGetOutLinkEndpoint, mxDelOutLinkEndpoint} {
		var shot *core.HTTPRequest
		for i, request := range script.snapshot() {
			if strings.Contains(request.URL, endpoint) {
				snapshot := script.snapshot()
				shot = &snapshot[i]
				break
			}
		}
		if shot == nil {
			t.Fatalf("没找到 %s 那一枪", endpoint)
		}
		if v := headerValue(*shot, "Authorization"); v != wholeAuth {
			t.Errorf("%s 要带库里的整串 Authorization，实得 %q", endpoint, v)
		}
		if v := headerValue(*shot, "deviceid"); v != "BSYNTH-DEVICE-199" {
			t.Errorf("%s 要带 deviceId，实得 %q", endpoint, v)
		}
		if v := headerValue(*shot, "jwttoken"); v != "" {
			t.Errorf("%s 不该带 jwttoken，实得 %q", endpoint, v)
		}
	}
	// 计数：两条都是专属玩法，且都没失败。
	if res.Counts.Ran != 2 || res.Counts.Failed != 0 {
		t.Errorf("应记 2 条玩法、0 条失败，实得 %+v", res.Counts)
	}
	if len(res.Steps) != 2 {
		t.Fatalf("应恰好两条记录，实得 %+v", res.Steps)
	}
	if res.Steps[1].Runner != "分享一个文件并立刻取消" || res.Steps[1].Err != "" {
		t.Errorf("434 该记成「跑的是外链玩法、没失败」，实得 %+v", res.Steps[1])
	}
	// 播报：建与删**各一行**（旧实现删完不吭声 ⇒ 日志里看着像留着一条分享）。
	if !log.has("分享文件成功 " + mxShareTaskName + " 434") {
		t.Errorf("少了「分享成功」那一行，实得：\n%s", log.joined())
	}
	if !log.has("已取消分享 " + mxShareTaskName + " 434") {
		t.Errorf("少了「已取消分享」那一行，实得：\n%s", log.joined())
	}
	// 节奏：106 与 434 各等一拍（runner 走 [apply] 末尾那一拍）。
	if n := mxCountPaced(wait); n != 2 {
		t.Errorf("应等 2 拍，实得 %d：%v", n, wait.durations)
	}
	// 434 不进那四格（它是多日任务，见 [TaskMatrixRun.verify]），只认 106。
	if res.Counts.DoneNow != 1 {
		t.Errorf("复查只该认下 106，实得 %+v", res.Counts)
	}
	// 传上去的文件仍进清理名单（外链删掉不等于文件不用管）。
	if n := mxTrashCount(script); n != 1 {
		t.Errorf("网盘产物该删一次，实得 %d", n)
	}
}

// TestTaskMatrixRunShareSendsNothingWithoutUploadedFiles 没有可分享的文件 ⇒ 两枪都不发。
//
// 旧实现这一档只 `logger.info` 一句就 return，而那条任务**照样**进了它的"成功"名单。
// 这里改成记一条失败：前置那一发打卡很可能已经把这条报给上游了（门的例外
// `currstep == 22` 也打，见 [taskClickGate]），"打了卡却没分享"是最该看见的不一致。
func TestTaskMatrixRunShareSendsNothingWithoutUploadedFiles(t *testing.T) {
	first := mxItems(mxItem(434, "day", mxShareTaskName, "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, first, append(
		mxShareRoutes(mxOutLinkBody(mxLinkID), mxDelOutLinkOK), mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxGetOutLinkEndpoint); n != 0 {
		t.Errorf("没有文件却发了 getOutLink，实得 %d 次", n)
	}
	if n := script.count(mxDelOutLinkEndpoint); n != 0 {
		t.Errorf("没有文件却发了 delOutLink，实得 %d 次", n)
	}
	// 那一发前置打卡照打（`currstep == 0` 放行）；"一枪不发"只指外链那两枪。
	if n := script.count(mxClickEndpoint); n != 1 {
		t.Errorf("前置打卡仍该发，实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "没有可分享的文件") {
		t.Errorf("要记一条失败原因，否则界面上是「跑了玩法、什么都没做」，实得 %q", text)
	}
	if res.Counts.Failed != 1 {
		t.Errorf("打了卡却没做成玩法 ⇒ 落在失败，实得 %+v", res.Counts)
	}
	if !log.hasAt(core.LevelWarn, "分享文件跳过") {
		t.Errorf("跳过那一行应是 warn，实得：\n%s", log.joined())
	}
}

// TestTaskMatrixRunShareKeepsLinkVisibleWhenUpstreamGivesNoLinkID ⭐ 上游没给 linkID ⇒ 不许静默。
//
// 旧实现那句是 `if (linkId) await delOutLink(...)` ⇒ 拿不到 id 时**什么都不做**，
// 于是那条公开外链留在用户网盘里而日志一个字都没有。这里三样都要有：
// 不补发删除（没有可删的标识）、播 error、并把"要去 App 里手动取消"说出口。
func TestTaskMatrixRunShareKeepsLinkVisibleWhenUpstreamGivesNoLinkID(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(434, "day", mxShareTaskName, "WAIT", "", 0),
	)
	routes := append(mxUploadRoutes(1), mxShareRoutes(mxOutLinkBody(""), mxDelOutLinkOK)...)
	script := mxScript([]string{"JWT-1"}, first, first, append(routes, mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := script.count(mxGetOutLinkEndpoint); n != 1 {
		t.Errorf("getOutLink 应发一次，实得 %d", n)
	}
	if n := script.count(mxDelOutLinkEndpoint); n != 0 {
		t.Errorf("没有 linkID 就不该发删除那一枪（拿什么删？），实得 %d", n)
	}
	text := res.Steps[1].Err
	if !strings.Contains(text, "没给 linkID") {
		t.Errorf("要记下这一档，实得 %q", text)
	}
	if !strings.Contains(text, "手动") {
		t.Errorf("结论里要给出路（人去 App 里取消），实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "分享文件取消失败") {
		t.Errorf("这一档必须是 error，不能像旧实现那样静默，实得：\n%s", log.joined())
	}
	if log.has("已取消分享") {
		t.Errorf("没删成却播「已取消分享」是谎报，实得：\n%s", log.joined())
	}
	// 它仍算"今天动过"：那一发建外链真的出网了（口径见 [apply]）。
	if res.Counts.Failed != 1 {
		t.Errorf("应落在失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunShareReportsDeleteFailure 删除那一枪被上游拒了 ⇒ 报出来、且**不补发**。
//
// 旧实现把 `tryRequest` 的返回值整个丢弃 ⇒ 删失败也是静默。这里判它的码（口径见
// `internal/upstream/app_cloud_outlink.go`）。"不补第二发"是另一格判据：一次失败的
// 删除该由人决定要不要再试（与 [TaskMatrixRun.cleanupCloudFiles] 同一条纪律）。
func TestTaskMatrixRunShareReportsDeleteFailure(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(434, "day", mxShareTaskName, "WAIT", "", 0),
	)
	routes := append(mxUploadRoutes(1),
		mxShareRoutes(mxOutLinkBody(mxLinkID), mxOutLinkRejected)...)
	script := mxScript([]string{"JWT-1"}, first, first, append(routes, mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxDelOutLinkEndpoint); n != 1 {
		t.Errorf("删失败也不该在同一趟补第二发，实得 %d", n)
	}
	if text := res.Steps[1].Err; !strings.Contains(text, "取消分享失败") {
		t.Errorf("要报出删除失败，实得 %q", text)
	}
	if !log.hasAt(core.LevelError, "分享文件取消失败") {
		t.Errorf("删除失败那一行应是 error，实得：\n%s", log.joined())
	}
	if log.has("已取消分享") {
		t.Errorf("删失败却播「已取消分享」是谎报，实得：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落在失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunShareFailureTextCarriesNoCredential ⭐ 凭据边界：原因里没有整串 auth、也没有手机号。
//
// 这一族的 `commonAccountInfo.account` 是**完整手机号**、`Authorization` 是整串凭据，
// 两样都进了请求体 —— 而失败原因会进事件流、进小结、可能被推送渠道发出去。
// 反向改坏：把请求体或 URL 拼进错误文本 ⇒ 本条红。
//
// ⚠ 网盘文件 id **不在**这一格：清理那一节刻意把 id 打出来（那是删失败之后人手动删的
// 唯一线索，成文见 `task_matrix_report.go`），别照着这一条把它也滤掉。
func TestTaskMatrixRunShareFailureTextCarriesNoCredential(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(434, "day", mxShareTaskName, "WAIT", "", 0),
	)
	routes := append(mxUploadRoutes(1),
		mxShareRoutes(mxOutLinkRejected, mxDelOutLinkOK)...)
	script := mxScript([]string{"JWT-1"}, first, first, append(routes, mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxDelOutLinkEndpoint); n != 0 {
		t.Errorf("建外链就失败了，不该再发删除，实得 %d", n)
	}
	text := log.joined() + "\n" + res.Steps[1].Err + "\n" + res.SummaryLine()
	for _, leak := range []string{wholeAuth, "Basic ", "19900000001"} {
		if strings.Contains(text, leak) {
			t.Errorf("失败文本里出现了 %q（凭据与手机号都不许进这一族的原因）：\n%s", leak, text)
		}
	}
}
