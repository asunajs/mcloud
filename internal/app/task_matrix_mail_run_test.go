package app

// ─────────────────────────────────────────────────────────────────────────────
// 139 邮箱那一半（472 / 473：① 换票 → ② 换会话 → ③ 活跃 → ④ 打卡）
// ─────────────────────────────────────────────────────────────────────────────
//
// 这一族的形状（四枪各发几次、失败时怎么说）在 `task_matrix_mail.go` 的
// [runMailExperience] 与 [mailSessionFor]；四枪的请求面与响应判定在
// `internal/upstream/app_mail139.go`（那里有 14 条形状用例）。
// 这一节只判**编排层**：什么时候不发、同一趟复用不复用、失败时念哪一档、
// 错误文本里带不带凭据。
//
// ⭐ 这一节存在的另一半理由，是"一趟内换一次会话"那一格（472 与 473 是同名的两条）：
// 旧实现靠 `ctx.state.mail139` 做到，而我们**只在成功时缓存** —— 没有这一节断言，
// 把缓存写丢（每条各换一次）与写死（失败也不换第二回）都不会让任何测试变红，
// 而前者是"多打两枪去敲邮箱"，后者是"473 永远做不了"。
//
// ⚠ 夹具**全是合成**的：② 的响应形状在本仓零实拍（实拍只捕获到请求面，
// 成文见 `app_mail139.go` 文件头），③④ 连请求面都没有。所以这些用例钉的是
// "编排层怎么处置每一种读数"，**不是**"上游会这样回"。

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
