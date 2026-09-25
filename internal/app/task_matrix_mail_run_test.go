package app

// ─────────────────────────────────────────────────────────────────────────────
// 139 邮箱那一族（会话 472 / 473：① 换票 → ② 换会话 → ③ 活跃 → ④ 打卡；
// 资产侧 1020：①②→⑧，1014：①②→传一份 PDF→⑥）
// ─────────────────────────────────────────────────────────────────────────────
//
// 这一族的形状（每一枪发几次、失败时怎么说）在 `task_matrix_mail.go` 的
// [runMailExperience]、[runMailBackup]、[runMailPDF] 与 [mailSessionFor]；
// 请求面与响应判定在 `internal/upstream/app_mail139.go` 与 `app_mail139_assets.go`。
// 这一层只判**编排层**：什么时候不发、同一趟复用不复用、失败时念哪一档、
// 错误文本里带不带凭据、写进网盘的那一份清不清。
//
// ⭐ 这一节存在的另一半理由，是"一趟内换一次会话"那一格：旧实现靠 `ctx.state.mail139`
// 做到，而我们**只在成功时缓存** —— 没有这一节断言，把缓存写丢（每条各换一次）与写死
// （失败也不换第二回）都不会让任何测试变红，而前者是"多打两枪去敲邮箱"，
// 后者是"下一条永远做不了"。资产侧那两条各自在拿到会话之后打**别的口**（一个先传文件），
// 所以"共用一张会话"在会话族与资产族各有一条用例（[TestTaskMatrixRunMailAssetTasksShareOneSession]）。
//
// ⚠ 夹具**大部分是合成**的：② 的响应形状在本仓零实拍（实拍只捕获到请求面），
// ③④ 连请求面都没有，⑥⑧ 两口的**成功响应**同样零样本。
// 反过来说，2026-09-26 的真跑让**同一张会话**在另外两台子域上各拿到过一次真实读数
// （⑤⑦ 都回 `code:"S_OK"`，成文 `docs/capture/app-mail139-samples.txt` §8）⇒
// "这张票是活的"不再是假设，但那些读数**不**支持本文件任何一条断言：这些用例钉的仍是
// "编排层怎么处置每一种读数"，不是"上游会这样回"。

import (
	"regexp"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// mxMailInLoginEndpoint / mxMailAutoLoginEndpoint 按**路径**匹配（两台主机共用
	// `mail.10086.cn`，靠路径分开；inlogin 与 autologin 互不为子串）。
	mxMailInLoginEndpoint   = "/login/inlogin.action"
	mxMailAutoLoginEndpoint = "/login/autologin.action"
	// mxMailClickEndpoint 是 ④。⚠ 它**不是** [mxClickEndpoint] 的子串，也不含它
	// （`task/click` vs `task/taskClickV2`）⇒ "通用打卡发了几次"这一类断言不会把
	// 邮箱那一发算进去，反之亦然。这一格是刻意的：整套用例里两处计数都靠它。
	mxMailClickEndpoint = "/ycloud/signin/task/taskClickV2"
	// mxSpecTokenEndpoint 是 ①，也是**签到建会话**那一发打的同一个口 ⇒ 计数必须按
	// 体里的 `toSourceId` 分，不能按 URL 分（按 URL 分的表现是"邮箱换票 1 次实得 2 次"，
	// 而多出来的那一次是会话自己的，改坏了邮箱这一族也不会红）。
	mxSpecTokenEndpoint = "/user/querySpecTokenV2"

	mxMailSID    = "SYNTHMAILSID000000000000abcd"
	mxMailRMKey  = "SYNTH+RMKEY/with=chars"
	mxMailOKBody = `{"code":0,"msg":"success"}`
	// mxMailNoSession 演"② 回了 200 但没给那两枚"—— 这正是本族**唯一有依据**的失败档
	// （判据照旧实现：只看 `var.sid` / `var.rmkey`，谁都不看 code）。
	mxMailNoSession = `{"var":{},"code":"S_ERROR"}`

	// ── 资产侧那两口（玩法 1020 的 ⑧ / 玩法 1014 的 ⑥）──
	//
	// mxMailBackupEndpoint 按**路径**匹配（⑧ 与 ③ 同一台主机，两条路径互不为子串）。
	mxMailBackupEndpoint = "/mailsms/sms/backup"
	// mxMailDiskEndpoint 是 ⑤⑥ **共用**的路径（两口只换查询里的 `func`）⇒ 今天编排层
	// 只发 ⑥，按它计数就是"转换那一发"；将来接 ⑤ 时必须改成按 `func=` 分，
	// 与 [mxSpecTokenEndpoint] 同一口径（按 URL 分会把两口算成一口）。
	mxMailDiskEndpoint = "/mw2/file/disk"
	// mxMailConvertFunc 就是上一句"按 `func` 分"的那一枚，用来认"这一发是转换"。
	mxMailConvertFunc = "disk:convertPdfFromYunFile"
	// mxMailAssetOK / mxMailAssetNo 是这一族的两个读数：成功码是**字符串** `S_OK`
	// （与签到族那个数字 `0` 不是一回事，成文在 `app_mail139_assets.go` 里
	// `mailAssetOKCode` 那一条注释）。
	mxMailAssetOK = `{"code":"S_OK"}`
	mxMailAssetNo = `{"code":"S_ERROR","summary":"额度不足"}`
)

// mailSessionBody 是 ② 的成功响应（`var.sid` + `var.rmkey`）。
func mailSessionBody() string {
	return `{"var":{"sid":"` + mxMailSID + `","rmkey":"` + mxMailRMKey + `"}}`
}

// mailRun 装配一条链（与 [mxDefaultRun] 同一份绑定，只是带上播报与自定义路由）。
func mailRun(
	t *testing.T, script *liveTaskScript, progress RunProgress,
) TaskMatrixRun {
	t.Helper()
	run := mxDefaultRun(t, script)
	run.Progress = progress
	return run
}

// mailRoutes 给这一族那三枪（②③④）编响应。① 复用 [mxScript] 里签到族那一条
// （同一端点、同一份 `ssoOK` 响应，编排层不关心票的值）。
func mailRoutes(inLogin, click string) []liveTaskRoute {
	return []liveTaskRoute{
		{mxMailInLoginEndpoint, []string{inLogin}},
		{mxMailAutoLoginEndpoint, []string{`{"result":"ok"}`}},
		{mxMailClickEndpoint, []string{click}},
	}
}

// mailSpecTokenCount 只数**邮箱那一次**换票：按体里的 `001003`，不按 URL（理由见
// [mxSpecTokenEndpoint] 那条注释）。
func mailSpecTokenCount(script *liveTaskScript) int {
	n := 0
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxSpecTokenEndpoint) &&
			strings.Contains(string(request.Body), upstream.Mail139SourceID) {
			n++
		}
	}
	return n
}

// uuidShape 是 [core.UUIDv4] 的规范形状。用它而不是比字面量：cguid 由随机源造，
// 编排层不许把它的值钉成一个串（要钉的是"带带了、而且是个像样的 cguid"）。
var uuidShape = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)

// mailDiskDigitsShape 是 ⑤⑥ 那两枚查询参数的形状（旧 TS：`"0"+8 位十进制` 与
// `0.` 后 15 位）。同样只比形状 —— 值由 [mailDiskRandoms] 从随机源造，钉字面量等于
// 把"每一发都换一枚"这件事写成"永远是那一枚"。
var mailDiskDigitsShape = regexp.MustCompile(`^0[0-9]{8}$`)

var mailDiskRndShape = regexp.MustCompile(`^0\.[0-9]{15}$`)

// ─────────────────────────────────────────────────────────────────────────────
// 资产侧夹具：② 成功 + ⑧⑥ 两口的响应 + `file/create` 的自定义档
// ─────────────────────────────────────────────────────────────────────────────

// mailSessionOnly 只给 ② 编成功响应。资产侧那两条**不发** ③④ ⇒ 这里刻意不给它们路由：
// 误发就是 404，而各条用例的 forbid 计数会立刻红在"多打了邮箱会话之外那一枪"上。
func mailSessionOnly() []liveTaskRoute {
	return []liveTaskRoute{{mxMailInLoginEndpoint, []string{mailSessionBody()}}}
}

// mailAssetRoutes 给 ⑧⑥ 两口编响应（顺序无关：两条路径互不为子串）。
func mailAssetRoutes(backup, convert string) []liveTaskRoute {
	return []liveTaskRoute{
		{mxMailBackupEndpoint, []string{backup}},
		{mxMailDiskEndpoint, []string{convert}},
	}
}

// mailCreateBody 造一份**自定义**的 `file/create` 响应（演"这一趟没有 fileId"那一档）。
// 三枪一起给，因为漏掉 PUT/complete 的路由会让"没传成"变成"传到一半撞 404"，
// 那是另一件事（由 [TestTaskMatrixRunMailPDFSkipsConvertWithoutFileID] 之外的用例管）。
func mailCreateBody(body string) []liveTaskRoute {
	return []liveTaskRoute{
		{mxCreateEndpoint, []string{body}},
		{mxPutHost, []string{""}},
		{mxCompleteEndpoint, []string{`{"code":"0000","success":true}`}},
	}
}

// queryValue 取出 URL 查询里那一枚参数的值（按 `&` 截断）。
//
// ⚠ 要能匹配**非首位**的参数：⑤⑥ 那条 URL 的首位是 `func`，`sid` / `rnd` / `cguid`
// 全在后面（旧 TS `buildQuery` 写死的顺序），只按 `?key=` 找会永远返回空串，
// 红出来像"参数没带"。
func queryValue(url, key string) string {
	for _, sep := range []string{"?" + key + "=", "&" + key + "="} {
		if _, rest, ok := strings.Cut(url, sep); ok {
			value, _, _ := strings.Cut(rest, "&")
			return value
		}
	}
	return ""
}

// ─────────────────────────────────────────────────────────────────────────────
// 分派：这两条该被认成玩法，而不是缺口
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanTaskStepSelectsMailTasks ⭐ 472 / 473 走 runner，且形状只有 Mail 那一格。
//
// 反向改坏它：把任一条挪回缺口表 ⇒ 这一条红；给 spec 顺手挂上 `FinalKey` ⇒ 也红
// （那会多打一发通用 `task/click`，而旧实现这两条**不发**那一发）。
func TestPlanTaskStepSelectsMailTasks(t *testing.T) {
	for _, id := range []int64{472, 473} {
		item := dispatchItem(id, "day", "体验 139 邮箱", "WAIT", 0)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepRunner {
			t.Errorf("%d 该走专属玩法，实得 %v（%s）", id, plan.Kind, plan.Reason)
		}
		if plan.Runner == "" {
			t.Errorf("%d 的结论要能说出是哪一条玩法做的，实得空", id)
		}
		if plan.ClickKey != taskClickKeyDefault {
			t.Errorf("%d 的前置那一发仍按通用门走（key=task），实得 %q", id, plan.ClickKey)
		}
		spec, ok := taskMatrixRunners[id]
		if !ok {
			t.Fatalf("%d 不在 runner 表里", id)
		}
		if !spec.Mail || spec.Upload != nil || spec.Shares || spec.Notes ||
			spec.FinalKey != "" || spec.OpensEntry {
			t.Errorf("%d 的形状只该有 Mail 那一格，实得 %+v", id, spec)
		}
		if _, gap := taskMatrixToolGaps[id]; gap {
			t.Errorf("%d 接上后没从缺口表删掉 ⇒ 界面上会永远显示'没接玩法'", id)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 主路径：四枪各一发、顺序、凭据往下带
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunMailSendsFourShotsInOrder ⭐ 一条任务把四枪各打一发。
func TestTaskMatrixRunMailSendsFourShotsInOrder(t *testing.T) {
	waiting := mxItems(mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, waiting, append(
		mailRoutes(mailSessionBody(), mxMailOKBody), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := mailSpecTokenCount(script); n != 1 {
		t.Errorf("① 该恰好一发（带 001003 的那一发），实得 %d", n)
	}
	for endpoint, want := range map[string]int{
		mxMailInLoginEndpoint: 1, mxMailAutoLoginEndpoint: 1, mxMailClickEndpoint: 1,
	} {
		if got := script.count(endpoint); got != want {
			t.Errorf("%s 应发 %d 次，实得 %d", endpoint, want, got)
		}
	}
	// ① 用的是**邮箱那一个** sourceId，不是签到族那一个（两枚值不同是实拍事实）。
	var bodies []string
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxSpecTokenEndpoint) {
			bodies = append(bodies, string(request.Body))
		}
	}
	if len(bodies) < 2 {
		t.Fatalf("① 与签到建会话共用这一口，至少要看到两份体，实得 %d", len(bodies))
	}
	var sawSignIn, sawMail bool
	for _, body := range bodies {
		switch {
		case strings.Contains(body, upstream.Mail139SourceID):
			sawMail = true
		case strings.Contains(body, upstream.ToSourceID):
			sawSignIn = true
		}
	}
	if !sawMail || !sawSignIn {
		t.Errorf("两族各换各的票（%s / %s），实得 %v",
			upstream.Mail139SourceID, upstream.ToSourceID, bodies)
	}
	// 时序：① → ② → ③ → ④。
	var order []string
	for _, url := range mxURLs(script) {
		switch {
		case strings.Contains(url, mxSpecTokenEndpoint):
			order = append(order, "spec")
		case strings.Contains(url, mxMailInLoginEndpoint):
			order = append(order, "in")
		case strings.Contains(url, mxMailAutoLoginEndpoint):
			order = append(order, "active")
		case strings.Contains(url, mxMailClickEndpoint):
			order = append(order, "click")
		}
	}
	// 签到建会话那一发 `spec` 在最前（拉表要先有票）⇒ 这里只比**尾部那四步**。
	if got := strings.Join(order[len(order)-4:], ">"); got != "spec>in>active>click" {
		t.Errorf("邮箱四枪时序实得 %s（全部：%v）", got, order)
	}
	// ② 的 URL 带 cguid，且形状是 UUID（实拍那一枚的形状；值由随机源造 ⇒ 只比形状）。
	in, has := mailRequest(script, mxMailInLoginEndpoint)
	if !has {
		t.Fatal("没看到 inlogin 那一枪")
	}
	_, cguid, ok := strings.Cut(in.URL, "?cguid=")
	if !ok || !uuidShape.MatchString(cguid) {
		t.Errorf("inlogin 的 URL 要带一枚 UUID 形状的 cguid，实得 %q", in.URL)
	}
	// ③ 的 URL 带 ② 换到的那两枚（活跃用的就是这一张会话）。
	active, has := mailRequest(script, mxMailAutoLoginEndpoint)
	if !has {
		t.Fatal("没看到 autologin 那一枪")
	}
	for _, want := range []string{mxMailSID, "SYNTH%2BRMKEY%2Fwith%3Dchars"} {
		if !strings.Contains(active.URL, want) {
			t.Errorf("autologin 要带 ② 换到的那一枚（编码后 %q），实得 %q", want, active.URL)
		}
	}
	// ④ 的体：`marketname` 用签到族那一枚、`taskId` 是这一条。
	click, _ := mailRequest(script, mxMailClickEndpoint)
	if body := string(click.Body); !strings.Contains(body, `"marketname":"sign_in_3"`) ||
		!strings.Contains(body, `"taskId":472`) {
		t.Errorf("④ 的体要带 marketname=sign_in_3 与 taskId=472，实得 %q", body)
	}
	if v := headerValue(click, "activityid"); v != upstream.MarketName {
		t.Errorf("④ 的 activityid 跟着本次的 marketname，实得 %q", v)
	}
	// ⭐ 这一条**只**打前置那一发通用打卡（门放开时），玩法自己那四枪不碰它；
	// 也**不**传文件、不建笔记。
	if clicks := mxClicks(script); len(clicks) != 1 || !strings.Contains(clicks[0], "id=472") {
		t.Errorf("472 只该有一发前置打卡（玩法不发 FinalKey），实得 %v", clicks)
	}
	if n := script.count(mxTrashEndpoint); n != 0 {
		t.Errorf("这一族不在用户网盘里留东西 ⇒ 不该发 batchTrash，实得 %d", n)
	}
	// 播报：两行 info（已活跃、已打卡），一行 error 都不该有。
	if !log.has("邮箱已活跃 体验 139 邮箱 472") {
		t.Errorf("少了「已活跃」那一行，实得：\n%s", log.joined())
	}
	if !log.has("邮箱市场已打卡 体验 139 邮箱 472") {
		t.Errorf("少了「已打卡」那一行，实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "邮箱任务失败") {
		t.Errorf("全成功的一趟不许念失败：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
}

func mailRequest(script *liveTaskScript, endpoint string) (core.HTTPRequest, bool) {
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, endpoint) {
			return request, true
		}
	}
	return core.HTTPRequest{}, false
}

// ─────────────────────────────────────────────────────────────────────────────
// 会话缓存：一趟只换一次，但 ③ 每条各发一次
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunMailReusesSessionAcrossTwoTasks ⭐ 472 + 473 同一趟：①② 各一发、③④ 各两发。
//
// 这一格是这一节唯一"只有编排层能测"的事：协议层四口各自都不知道有几条任务在用它。
// 反向改坏：删掉 [taskExecutor.mailSession] 那一格缓存 ⇒ ①② 变 2 次；
// 把缓存改成"连 ③ 一起跳" ⇒ ③ 变 1 次。两种都红。
//
// ⚠ 两份条目刻意**不同名**：读表那层按 `name` 去重、id 小的赢
// （`task_matrix.go` 文件头第 23 行那句口径），上游真给成同名时 473 会被挡在编排层**之外**
// —— 那是既有口径，不是这一族的 bug，但它意味着"一趟只换一次会话"在真数据上
// 可能只服务一条任务。那一格留在 `docs/pending-decisions.md`（本轮新登记）。
func TestTaskMatrixRunMailReusesSessionAcrossTwoTasks(t *testing.T) {
	waiting := mxItems(
		mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0),
		mxItem(473, "day", "阅读邮件", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, waiting, waiting, append(
		mailRoutes(mailSessionBody(), mxMailOKBody), mxHappy()...)...)
	res, perr := mxResult(t, mailRun(t, script, (&mxLog{}).handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := mailSpecTokenCount(script); n != 1 {
		t.Errorf("两条任务共用一次换票，实得 %d", n)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("两条任务共用一次换会话，实得 %d", n)
	}
	if n := script.count(mxMailAutoLoginEndpoint); n != 2 {
		t.Errorf("活跃要逐条各发一次（旧实现把它放在缓存**之外**），实得 %d", n)
	}
	if n := script.count(mxMailClickEndpoint); n != 2 {
		t.Errorf("两条各打一发 ④，实得 %d", n)
	}
	// 两发 ④ 的 taskId 必须是各自那一条（打重了等于一条做两次、另一条没做）。
	var ids []string
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxMailClickEndpoint) {
			_, rest, _ := strings.Cut(string(request.Body), `"taskId":`)
			id, _, _ := strings.Cut(rest, "}")
			ids = append(ids, id)
		}
	}
	if got := strings.Join(ids, ","); got != "472,473" {
		t.Errorf("两发 ④ 的 taskId 应各自一条，实得 %q", got)
	}
	if res.Counts.Ran != 2 || res.Counts.Failed != 0 {
		t.Errorf("应记 2 条玩法，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailRefetchesSessionAfterFailure ⭐ ② 换不到 ⇒ 下一条**再换一次**。
//
// 这一格刻意与上一条成对：只测"成功时复用"会漏掉反向的那一半 —— 把缓存写成
// "第一次失败也记下'别再试'"，473 就永远做不了，而事件流里只有一条失败。
// ⚠ 代价（照旧实现）：失败一次 = 多打两枪。取舍的理由写在 [mailSessionFor] 头注。
func TestTaskMatrixRunMailRefetchesSessionAfterFailure(t *testing.T) {
	waiting := mxItems(
		mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0),
		mxItem(473, "day", "阅读邮件", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, waiting, waiting, append(
		mailRoutes(mxMailNoSession, mxMailOKBody), mxHappy()...)...)
	res, perr := mxResult(t, mailRun(t, script, (&mxLog{}).handle()))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 2 {
		t.Errorf("两条各试一次换会话（不缓存失败），实得 %d", n)
	}
	if n := mailSpecTokenCount(script); n != 2 {
		t.Errorf("换会话前各自换一次票，实得 %d", n)
	}
	if n := script.count(mxMailAutoLoginEndpoint) + script.count(mxMailClickEndpoint); n != 0 {
		t.Errorf("没换到会话就不该发 ③④，实得 %d", n)
	}
	if res.Counts.Failed != 2 || res.Counts.Ran != 0 {
		t.Errorf("两条都该落失败，实得 %+v", res.Counts)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 失败分档：哪一枪挂了，说的是不同的事
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunMailStopsWhenSessionMissing ⭐ ② 没给那两枚 ⇒ ③④ 一枪不发，且不说凭据。
func TestTaskMatrixRunMailStopsWhenSessionMissing(t *testing.T) {
	waiting := mxItems(mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		mailRoutes(mxMailNoSession, mxMailOKBody), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("② 仍该发（它是唯一能知道上游给不给的手段），实得 %d", n)
	}
	if n := script.count(mxMailAutoLoginEndpoint) + script.count(mxMailClickEndpoint); n != 0 {
		t.Errorf("③④ 一枪都不该发，实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "邮箱会话（②）没换到") {
		t.Errorf("要记一条失败原因，否则界面上是「跑了玩法、什么都没做」，实得 %q", text)
	}
	if !strings.Contains(text, "S_ERROR") {
		t.Errorf("上游说了话就要带进原因（协议层只取 code/message 两格），实得 %q", text)
	}
	// ⭐ 凭据边界：这一条链上出过的**任何**一票都不许进结论或事件流。
	// `SSO-1` 是夹具里那枚假票 ⇒ 它出现就说明编排层把入参 echo 出来了。
	for _, leak := range []string{"SSO-1", mxMailSID, mxMailRMKey} {
		if strings.Contains(text, leak) || strings.Contains(log.joined(), leak) {
			t.Errorf("错误文本里带了 %q（结论与事件流都不许有凭据）", leak)
		}
	}
	if !log.hasAt(core.LevelError, "邮箱任务失败 体验 139 邮箱 472") {
		t.Errorf("这一档必须是 error，实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "邮箱市场已打卡") {
		t.Errorf("没打成卡就不许念「已打卡」：\n%s", log.joined())
	}
}

// TestTaskMatrixRunMailClickFailureSaysTaskNotAcknowledged ⭐ ③ 成了、④ 被拒 ⇒ 说的是两件事。
//
// 这一格是这一族**最容易被写坏**的地方：把 ④ 的失败也念成"邮箱没打开"，
// 读的人会去查邮箱那一侧，而真正要查的是签到族那一口（坑表 #149：同一路径的兄弟口
// 对 22 个 id 全回 `58999 任务不存在`）。
func TestTaskMatrixRunMailClickFailureSaysTaskNotAcknowledged(t *testing.T) {
	waiting := mxItems(mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		mailRoutes(mailSessionBody(), `{"code":58999,"msg":"任务不存在"}`), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := script.count(mxMailAutoLoginEndpoint); n != 1 {
		t.Errorf("③ 该成功发过一发，实得 %d", n)
	}
	if !log.has("邮箱已活跃 体验 139 邮箱 472") {
		t.Errorf("③ 成功那一行要在，否则分不清挂的是哪一枪：\n%s", log.joined())
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "邮箱市场打卡（④）失败") ||
		!strings.Contains(text, "前三枪做了、这一条任务上游没记上") {
		t.Errorf("要说清「活跃做了、任务没记上」，实得 %q", text)
	}
	if !strings.Contains(text, "58999") {
		t.Errorf("上游那句原文要带进来，实得 %q", text)
	}
	if strings.Contains(text, "没换到会话") {
		t.Errorf("会话明明换到了，不许混成同一档：%q", text)
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落失败 1 条，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailAutologinFailureStillClicks ⭐ ③ 挂了**不挡** ④（照旧实现），但仍记失败。
//
// 旧实现那里是 `tryRequest` 吞掉、④ 无条件照发。我们跟着发，但不跟着静默。
// ⚠ 这一格钉的是两个方向：既不许"③ 挂就不发 ④"（那是少做），也不许"③ 挂了不吭声"
// （那是本仓反复批评旧实现的那一处）。
func TestTaskMatrixRunMailAutologinFailureStillClicks(t *testing.T) {
	waiting := mxItems(mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0))
	// 故意**不给** ③ 编路由 ⇒ [liveTaskScript] 回 404 + 一句人话，正是"没发出去"那一档。
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxMailInLoginEndpoint, []string{mailSessionBody()}},
			{mxMailClickEndpoint, []string{mxMailOKBody}},
		}, mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxMailClickEndpoint); n != 1 {
		t.Errorf("③ 失败仍要发 ④（旧实现那一发不受它约束），实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "邮箱活跃（③）失败") {
		t.Errorf("③ 的失败要单独一句，实得 %q", text)
	}
	if !strings.Contains(text, "仍照旧实现往下发 ④") {
		t.Errorf("要说清后面的枪照发，否则读的人会以为整条没跑，实得 %q", text)
	}
	if strings.Contains(text, "邮箱市场打卡（④）失败") {
		t.Errorf("④ 其实成功了，不许顺手记它：%q", text)
	}
	if res.Counts.Failed != 1 || res.Counts.Ran != 0 {
		t.Errorf("有失败就落失败那一档，实得 %+v", res.Counts)
	}
	if !log.hasAt(core.LevelError, "邮箱任务失败") {
		t.Errorf("③ 失败要 error 一行，实得：\n%s", log.joined())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 资产侧（玩法 1020「邮箱资产备份」= ①②→⑧ / 玩法 1014「PDF 转换」= ①②→上传→⑥）
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanTaskStepSelectsMailAssetTasks ⭐ 这两条走 runner，且各自只有一个资产侧开关。
//
// 反向改坏：把任一条挪回缺口表 ⇒ 红；给 1020 顺手挂 `FinalKey`（"邮箱任务都要打市场卡"
// 那种想当然）⇒ 红；给 1014 挂上 `Upload` 那一份配置 ⇒ 也红（它要的是**固定内容**的
// 那份 PDF，不是随机文件，见 [taskRunnerSpec.MailPDF]）。
func TestPlanTaskStepSelectsMailAssetTasks(t *testing.T) {
	for _, id := range []int64{1014, 1020} {
		item := dispatchItem(id, "day", "邮箱资产侧玩法", "WAIT", 0)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepRunner {
			t.Errorf("%d 该走专属玩法，实得 %v（%s）", id, plan.Kind, plan.Reason)
		}
		if plan.Runner == "" {
			t.Errorf("%d 的结论要能说出是哪一条玩法做的，实得空", id)
		}
		spec, ok := taskMatrixRunners[id]
		if !ok {
			t.Fatalf("%d 不在 runner 表里", id)
		}
		if spec.MailPDF == spec.MailBackup {
			t.Errorf("%d 的形状只该有一个资产侧开关（两个都真=同一趟既传 PDF 又备份），实得 %+v",
				id, spec)
		}
		if spec.Mail || spec.Upload != nil || spec.Shares || spec.Notes ||
			spec.FinalKey != "" || spec.OpensEntry {
			t.Errorf("%d 不该带别的玩法开关，实得 %+v", id, spec)
		}
		if _, gap := taskMatrixToolGaps[id]; gap {
			t.Errorf("%d 接上后没从缺口表删掉 ⇒ 界面上会永远显示'没接玩法'", id)
		}
	}
}

// TestTaskMatrixRunMailBackupSendsSessionThenBackup ⭐ 1020：会话两枪 + ⑧ 一枪，别的一枪没有。
//
// ⭐ 这一条的核心是 forbid 那一半：旧实现 1020 那一条 runner **只**取会话再打 ⑧
// （`runners.ts:160-165`），它不碰 ③ 的活跃、也不碰 472 那一发市场打卡。
// 把两条邮箱玩法抄成同一条的表现是多打两枪 —— 而 ③④ 今天真实回 404（实拍
// `docs/capture/app-mail139-samples.txt` §2/§4），多打就是往上游发两句没人认的请求，
// 事件流里还各带一行"失败"，把这一条本来成功的结果搅浑。
func TestTaskMatrixRunMailBackupSendsSessionThenBackup(t *testing.T) {
	waiting := mxItems(mxItem(1020, "day", "邮箱资产备份", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		append(mailAssetRoutes(mxMailAssetOK, mxMailAssetOK), mailSessionOnly()...),
		mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := mailSpecTokenCount(script); n != 1 {
		t.Errorf("① 该恰好一发（带 %s 的那一发），实得 %d", upstream.Mail139SourceID, n)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("② 该一发，实得 %d", n)
	}
	if n := script.count(mxMailBackupEndpoint); n != 1 {
		t.Errorf("⑧ 该恰好一发，实得 %d", n)
	}
	for _, forbidden := range []string{mxMailAutoLoginEndpoint, mxMailClickEndpoint,
		mxMailDiskEndpoint, mxCreateEndpoint, mxCompleteEndpoint, mxTrashEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("1020 不该打 %s，实得 %d 次", forbidden, n)
		}
	}
	if clicks := mxClicks(script); len(clicks) != 1 || !strings.Contains(clicks[0], "id=1020") {
		t.Errorf("1020 只该有前置那一发通用打卡，实得 %v", clicks)
	}
	backup, has := mailRequest(script, mxMailBackupEndpoint)
	if !has {
		t.Fatal("没看到 ⑧ 那一枪")
	}
	// ⑧ 吃的是**这一趟**换到的那张会话（URL 里那枚 sid），不是别的号留下的。
	if got := queryValue(backup.URL, "sid"); got != mxMailSID {
		t.Errorf("⑧ 的 URL 要带 ② 换到的 sid，实得 %q", strings.Split(backup.URL, "?")[0])
	}
	// ⭐ 这一口经旧实现的**主实例**发 ⇒ 带 authorization（与 ③ 同一处置，成文在
	// `app_mail139_assets.go` 的 [MailBackupRequest] 头注）。少了它是最难查的一种坏：
	// 上游回 401，而事件流只念"备份失败"。
	if got := headerValue(backup, "authorization"); got != wholeAuth {
		t.Errorf("⑧ 要带库里的整串 authorization（这一口走主实例），实得非空=%v", got != "")
	}
	if got := string(backup.Body); got != "{}" {
		t.Errorf("⑧ 的体是空对象（旧实现那一份），实得 %q", got)
	}
	if !log.has("邮箱备份已提交 邮箱资产备份 1020") {
		t.Errorf("成功那一档要念「已提交」而不是「已完成」（成败由复查说），实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "邮箱备份失败") {
		t.Errorf("全成功的一趟不许念失败：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailBackupFailureKeepsCredentialsOut ⭐ ⑧ 非 2xx ⇒ 一句失败、不泄 sid。
//
// ⚠ **这一条钉的边界要说清**（否则会被误读成"脱敏有测试了"）：夹具对没料到的端点回
// `404 + "no route for <完整 URL>"`，而协议层 [errorFromStatus] 只把状态码写进错误文本
// （体不进）⇒ 摘掉 `redactError` **不会**让本条红。那一格的反向用例在协议层：
// `TestAppMailBackupRedactsTransportError`（它复刻了真实传输层错误那句含 URL 的形状）。
//
// 本条因此钉的是**编排层自己不 echo**：将来谁在 [runMailBackup] 那句 problem 里
// 顺手补一段"发往 <URL>"（这是最常见的一种"方便排查"），sid 就会进结论与事件流 ——
// 那时本条红。两个方向合起来才是这一族的凭据边界。
func TestTaskMatrixRunMailBackupFailureKeepsCredentialsOut(t *testing.T) {
	waiting := mxItems(mxItem(1020, "day", "邮箱资产备份", "WAIT", "", 0))
	// 只给 ⑥ 与 ② 编路由、**不**给 ⑧ ⇒ 它撞夹具那句 404。
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		append(mailAssetRoutes(mxMailAssetOK, mxMailAssetOK)[1:], mailSessionOnly()...),
		mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := script.count(mxMailBackupEndpoint); n != 1 {
		t.Errorf("⑧ 该发出去过（它是唯一能知道上游认不认的手段），实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "邮箱备份（⑧）失败") {
		t.Errorf("要说清挂的是 ⑧，否则与'会话没换到'分不开，实得 %q", text)
	}
	if !strings.Contains(text, "HTTP 404") {
		t.Errorf("上游的状态码要带进来，实得 %q", text)
	}
	for _, leak := range []string{mxMailSID, mxMailRMKey, "SSO-1"} {
		if strings.Contains(text, leak) || strings.Contains(log.joined(), leak) {
			t.Errorf("失败文本里带了凭据（sid/rmkey/票都只准待在内存里）：%q", leak)
		}
	}
	if !log.hasAt(core.LevelError, "邮箱备份失败 邮箱资产备份 1020") {
		t.Errorf("这一档必须是 error，实得：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 || res.Counts.Ran != 0 {
		t.Errorf("应落失败 1 条，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailPDFUploadsThenConverts ⭐ 1014：会话 → 传一份 PDF（三枪）→ ⑥。
//
// 三格判据各挡一种错：
//   - **先换会话再传**：反过来会出现"会话都没换到，已经在用户网盘里写了一个文件"。
//   - **⑥ 的 fileId 来自这一次上传的响应**：旧实现读 `ctx.state.files` 的末位，那是
//     共享可变状态（同一趟里别的任务也往里塞）⇒ 用固定夹具时看不出来，一旦前一格
//     传了 3 个文件就转错了对象。这里把 create 回的那一枚钉进 ⑥ 的体。
//   - **链尾清掉传上去那一份**（转换产物不在这一条链上，R36）。
func TestTaskMatrixRunMailPDFUploadsThenConverts(t *testing.T) {
	waiting := mxItems(mxItem(1014, "day", "PDF 转换", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		append(append(mailAssetRoutes(mxMailAssetOK, mxMailAssetOK), mailSessionOnly()...),
			mxUploadRoutes(1)...), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("② 该一发，实得 %d", n)
	}
	if creates, puts, completes := mxUploadCounts(script); creates != 1 || puts != 1 ||
		completes != 1 {
		t.Errorf("该恰好三枪，实得 create %d / PUT %d / complete %d", creates, puts, completes)
	}
	if n := script.count(mxMailDiskEndpoint); n != 1 {
		t.Errorf("⑥ 该恰好一发，实得 %d", n)
	}
	// 时序：会话（in）→ 占位（create）→ 转换（disk）。
	in, create, convert := requestIndex(script, mxMailInLoginEndpoint),
		requestIndex(script, mxCreateEndpoint), requestIndex(script, mxMailConvertFunc)
	if !(in >= 0 && create > in && convert > create) {
		t.Errorf("时序应为 换会话>传文件>转换，实得 %d / %d / %d（全部：%v）",
			in, create, convert, mxURLs(script))
	}
	converted, has := mailRequest(script, mxMailConvertFunc)
	if !has {
		t.Fatal("没看到 ⑥ 那一枪")
	}
	// ⑥ 走的是旧 TS 原生 fetch 那一档 ⇒ **不**带 authorization（与 ⑧ 相反，两处都成文）。
	if got := headerValue(converted, "authorization"); got != "" {
		t.Errorf("⑥ 不该带 authorization（这一口在旧实现里没经过主实例），实得非空")
	}
	if !strings.Contains(headerValue(converted, "cookie"), "RMKEY=") {
		t.Errorf("⑥ 靠 Cookie 里的会话认人，实得头集合 %v", headerNames(converted))
	}
	// 两枚随机参数：带上了、形状对（值由随机源造 ⇒ 只比形状）。
	if got := queryValue(converted.URL, "cguid"); !mailDiskDigitsShape.MatchString(got) {
		t.Errorf("⑥ 的 cguid 要形如 0+8 位十进制，实得 %q", got)
	}
	if got := queryValue(converted.URL, "rnd"); !mailDiskRndShape.MatchString(got) {
		t.Errorf("⑥ 的 rnd 要形如 0.<15 位>，实得 %q", got)
	}
	body := string(converted.Body)
	if !strings.Contains(body, "<string name=\"fileId\">SYNTH-FID-1</string>") {
		t.Errorf("⑥ 转的必须是这一次传上去的那一份，实得 %q", body)
	}
	if !strings.Contains(body, `<int name="convertType">1</int>`) {
		t.Errorf("⑥ 的 convertType 用旧实现那两处的定值 1，实得 %q", body)
	}
	if !strings.Contains(body, `<string name="fileName">asign-pdf-`) {
		t.Errorf("⑥ 的 fileName 要跟上到网盘的那一个名字（同一枚时间戳），实得 %q", body)
	}
	// 网盘那一格进了清理账本，链尾删的是**同一枚** id。
	if len(res.CloudFileIDs) != 1 || res.CloudFileIDs[0] != "SYNTH-FID-1" {
		t.Errorf("传上去的 fileId 要进清理链，实得 %v", res.CloudFileIDs)
	}
	if n := mxTrashCount(script); n != 1 || !strings.Contains(mxTrashBodies(script)[0], "SYNTH-FID-1") {
		t.Errorf("链尾该把这一份移进回收站，实得 %d 次 / %v", n, mxTrashBodies(script))
	}
	if res.Steps[0].Uploads != 1 || res.Counts.Uploaded != 1 {
		t.Errorf("这一条要按'写了一个文件'记账，实得 %+v / %+v", res.Steps[0], res.Counts)
	}
	if !strings.Contains(log.joined(), "PDF 转换已提交") {
		t.Errorf("成功那一档要念「已提交」（不念「已完成」：成败由复查说），实得：\n%s", log.joined())
	}
	if !strings.Contains(log.joined(), "PDF 素材已上传") {
		t.Errorf("上传那一行要单独念（它说了走了哪一档），实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "PDF 转换失败") {
		t.Errorf("全成功的一趟不许念失败：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailPDFSkipsConvertWithoutFileID ⭐ 上游说"同名已存在" ⇒ ⑥ 不发、也不删。
//
// 这一档刻意**不**"退回用别的 id 转一次"：手上这一枚没有，转哪个都是猜。
// 反向改坏两种：把 `receipt.FileID == ""` 那一支删掉 ⇒ ⑥ 带着空 fileId 发出去
// （协议层会拦，红成"⑥ 发了 1 次"）；把"已存在"当成成功 ⇒ 事件流念「已上传」。
func TestTaskMatrixRunMailPDFSkipsConvertWithoutFileID(t *testing.T) {
	waiting := mxItems(mxItem(1014, "day", "PDF 转换", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(append(append(
		mailAssetRoutes(mxMailAssetOK, mxMailAssetOK), mailSessionOnly()...),
		mailCreateBody(`{"code":"0000","success":true,"data":`+
			`{"exist":true,"rapidUpload":false}}`)...), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("这一档不该让整趟报错: %v", perr)
	}
	if n := script.count(mxMailDiskEndpoint); n != 0 {
		t.Errorf("没有 fileId 就不该发 ⑥，实得 %d", n)
	}
	if n := script.count(mxCompleteEndpoint); n != 0 {
		t.Errorf("同名已存在 ⇒ 后两枪都不发（旧实现同一处置），实得 %d", n)
	}
	if n := mxTrashCount(script); n != 0 {
		t.Errorf("不是我们造的那一格不许删，实得 %d 次", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "没有 fileId") || !strings.Contains(text, "已存在") {
		t.Errorf("要说清'为什么没转'与'上游那句是已存在'，实得 %q", text)
	}
	if !log.hasAt(core.LevelWarn, "PDF 转换跳过") {
		t.Errorf("这一档是 warn（没做成、但也没写坏任何东西），实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "PDF 素材已上传") {
		t.Errorf("没拿到 fileId 不许念「已上传」：\n%s", log.joined())
	}
	if len(res.CloudFileIDs) != 0 {
		t.Errorf("没有新对象就不该有清理账，实得 %v", res.CloudFileIDs)
	}
	if res.Counts.Failed != 1 {
		t.Errorf("没做成要落失败那一档（不是'跑过了'），实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailPDFFailureStillTrashesUploadedFile ⭐ ⑥ 被拒 ⇒ 传上去那一份仍要删。
//
// 这一格钉的是**副作用收尾**：写已经发生了，玩法成没成不改变"要把它清掉"。
// 反过来的写法（只在成功时记账）会留下一个用户网盘里没人认领的文件，
// 而那正是本仓给这一族加清理链的全部理由。
func TestTaskMatrixRunMailPDFFailureStillTrashesUploadedFile(t *testing.T) {
	waiting := mxItems(mxItem(1014, "day", "PDF 转换", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(append(append(
		mailAssetRoutes(mxMailAssetOK, mxMailAssetNo), mailSessionOnly()...),
		mxUploadRoutes(1)...), mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, mailRun(t, script, log.handle()))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := script.count(mxMailDiskEndpoint); n != 1 {
		t.Errorf("⑥ 该发出去过，实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "PDF 转换（⑥）失败") {
		t.Errorf("要说清挂的是 ⑥（不是会话、也不是上传），实得 %q", text)
	}
	if !strings.Contains(text, "S_ERROR") {
		t.Errorf("上游那句 code 要带进来，实得 %q", text)
	}
	if !strings.Contains(text, "转换产物") {
		t.Errorf("要写明产物那一格未知（R36），别让人以为'跑完什么都没留下'，实得 %q", text)
	}
	if n := mxTrashCount(script); n != 1 || !strings.Contains(mxTrashBodies(script)[0], "SYNTH-FID-1") {
		t.Errorf("失败也要清理那一份素材，实得 %d 次 / %v", n, mxTrashBodies(script))
	}
	if !log.hasAt(core.LevelError, "PDF 转换失败 PDF 转换 1014") {
		t.Errorf("⑥ 被拒是 error，实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "PDF 转换已提交") {
		t.Errorf("上游没认就不许念「已提交」：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落失败 1 条，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunMailAssetTasksShareOneSession ⭐ 1014 + 1020 同一趟：①② 各一发。
//
// 与 [TestTaskMatrixRunMailReusesSessionAcrossTwoTasks] 同一格判据，但**必须单独有这一条**：
// 共用会话那两格（472/473）走的是 [runMailExperience]，而资产侧这两条各自在拿到会话之后
// 打**别的口**（一个先传文件）。把缓存改成只在某一族生效，上面那条不会红。
// ⚠ 两条任务刻意不同名（读表按 name 去重、id 小的赢）。
func TestTaskMatrixRunMailAssetTasksShareOneSession(t *testing.T) {
	waiting := mxItems(
		mxItem(1014, "day", "PDF 转换", "WAIT", "", 0),
		mxItem(1020, "day", "邮箱资产备份", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(append(append(
		mailAssetRoutes(mxMailAssetOK, mxMailAssetOK), mailSessionOnly()...),
		mxUploadRoutes(1)...), mxHappy()...)...)
	res, perr := mxResult(t, mailRun(t, script, (&mxLog{}).handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := mailSpecTokenCount(script); n != 1 {
		t.Errorf("两条共用一次换票，实得 %d", n)
	}
	if n := script.count(mxMailInLoginEndpoint); n != 1 {
		t.Errorf("两条共用一次换会话，实得 %d", n)
	}
	if n := script.count(mxMailBackupEndpoint); n != 1 {
		t.Errorf("⑧ 只属于 1020，该一发，实得 %d", n)
	}
	if n := script.count(mxMailDiskEndpoint); n != 1 {
		t.Errorf("⑥ 只属于 1014，该一发，实得 %d", n)
	}
	if n := script.count(mxMailAutoLoginEndpoint) + script.count(mxMailClickEndpoint); n != 0 {
		t.Errorf("资产侧这两条都不该碰 ③④，实得 %d", n)
	}
	if res.Counts.Ran != 2 || res.Counts.Failed != 0 {
		t.Errorf("应记 2 条玩法，实得 %+v", res.Counts)
	}
}
