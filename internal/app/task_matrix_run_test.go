package app

// #9 写侧**执行**那一半的用例（`task_matrix_run.go`）。
//
// 分派层的判据在 `task_matrix_dispatch_test.go`（那里一个请求都不发）；这一层只判
// "决定落成请求之后"的四件事，它们都是**只有发出请求才会露出来**的错：
//
//  1. **打到哪个域**：`task/click` 有三支同名端点（`m.mcloud.139.com/ycloud/signin/task/click`
//     是本族、`:7071/ycloud/signin/task/click` 属 notice 链、`:7071/market/task/task/click`
//     属分享码族）。写成后任何一支，上游都不会把这条记进本族的任务表 ⇒
//     复查永远"没认"，而界面上只看到"没认"。
//  2. **一条都不许多发**：这一族是**写**请求（云朵可能当场入账、且没有撤销那一口）。
//     缺口 / 门挡 / 需人工三种处置必须**零枪**，所以几乎每条用例都带着 forbid 侧的计数。
//  3. **节奏**：旧实现只在 `handleTaskClick` 末尾 sleep，default 那次盲点不等。
//     混成一个的表现是"十几条零间隔连发"，那是真实客户端不会有的节奏。
//  4. **一趟只用一张票**：读 32 枪 + 写 N 枪共用一条会话（用户 2026-09-23 的口径）。
//
// ⚠ 这里的响应**全是合成夹具**。本族那一发 `task/click`（`m.mcloud.139.com`）只有
// **两发**实拍（另两发同名端点在 `:7071`，属别的族；表见 `app_task_click.go` 文件头），
// 但"打完之后上游会把 `state` 改成 FINISH"这件事**一次真打都没有** ⇒ 复查那几格里
// 的"认下"是**夹具给的读数**，不是上游行为的证据。真打的第一个样本要等手工点一次
// （所以这一条 `RunInBatch: false`，见 `registry.go`）。

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

const (
	// mxClickEndpoint 是本族那一发打卡的**完整路径**（含 `/ycloud/`，用来把另外两支同名端点排除掉）。
	mxClickEndpoint  = "/ycloud/signin/task/click"
	mxEntryEndpoint  = "/market/signin/page/doTask"
	mxStatusEndpoint = "/ycloud/signin/page/infoV3"
	// mxClickOK 逐字取实拍那一句 —— ⚠ 实拍里**没有一条带回 `result`**
	// （成文见 `app_task_click.go` 的 `TaskClick.Notice` 那条注释），所以夹具也**不给**：
	// 造一份带 `result.msg` 的成功响应，会让"上游会回一句人话"看起来像实测过的事。
	mxClickOK         = `{"code":0,"msg":"success"}`
	mxEntryOK         = `{"code":0,"msg":"success"}`
	mxClickBizFailure = `{"code":1,"msg":"任务已过期"}`
)

func mxBool(v bool) *bool { return &v }

// mxItem 造一条任务条目。`state` 传空串 ⇒ **不给**那一格；`market` 空 ⇒ 云盘侧。
func mxItem(id int64, group, name, state, market string, step int64) string {
	if market == "" {
		market = upstream.MarketName
	}
	parts := []string{
		`"id":` + itoa64(id),
		`"name":` + quoteJSONString(name),
		`"groupid":` + quoteJSONString(group),
		`"marketname":` + quoteJSONString(market),
		`"enable":1`, `"process":0`, `"currstep":` + itoa64(step),
	}
	if state != "" {
		parts = append(parts, `"state":`+quoteJSONString(state))
	}
	return "{" + strings.Join(parts, ",") + "}"
}

// mxBody 把若干条目放进**一次**响应（键名用 `new`；解析器摊平时不看键名）。
func mxBody(items ...string) string {
	if len(items) == 0 {
		return taskMatrixEmptyBody
	}
	return `{"code":0,"msg":"success","result":{"new":[` +
		strings.Join(items, ",") + `]},"groupState":{}}`
}

// mxRound 铺出一趟的 16 份响应：只有 `overrides` 点名的那几个下标非空。
//
// 下标口径与请求顺序一致：`市场名索引 × 8 + group 索引`（0 = sign_in_3/new）。
func mxRound(overrides map[int]string) []string {
	out := make([]string, 0, TaskMatrixFetches())
	for i := 0; i < TaskMatrixFetches(); i++ {
		if body, ok := overrides[i]; ok {
			out = append(out, body)
			continue
		}
		out = append(out, taskMatrixEmptyBody)
	}
	return out
}

// mxItems 是"条目全放在第一组"的简写（绝大多数用例只关心条目本身）。
func mxItems(items ...string) []string { return mxRound(map[int]string{0: mxBody(items...)}) }

// mxAllFail 是一整趟全挂的那份读数（复查挂掉那一档用）。
func mxAllFail() []string {
	out := make([]string, 0, TaskMatrixFetches())
	for i := 0; i < TaskMatrixFetches(); i++ {
		out = append(out, `{"code":1,"msg":"系统繁忙"}`)
	}
	return out
}

// mxStatusBody 给一份 `infoV3`。`backup` 为 nil ⇒ **不给** `curMonthBackup` 那一格
// —— 那道门要的正是要区分"读不到"与"读到 false"（旧实现那句是严格 `=== false`）。
//
// ⚠ 必须走 [statusPayload]：`infoV3` 的解析器对 `result.cal` 是**必填**
// （缺了整份状态报 parse 错），而本层的读法是"解析挂 ⇒ `BackupKnown=false` ⇒ 门不挡"。
// 夹具少了那一格，红出来的是"读到 false 却没挡"，看着像门的 bug，
// 实际是响应体压根没解析成功 —— 方向会被带偏。
func mxStatusBody(backup *bool) string {
	if backup == nil {
		return statusPayload(`"cal":[]`)
	}
	value := "false"
	if *backup {
		value = "true"
	}
	return statusPayload(`"cal":[],"curMonthBackup":` + value)
}

// mxStore 只补这一条链会碰的那三个入口：登录态、账号、设备特征。
//
// ⚠ 与 [poolSignInStore] 的关键不同：这里 `GetSmidState` 默认回**一份绑好且带 deviceId**
// 的状态。那两条 runner 需要 H5 `deviceId` 才发入口上报，而它们对"换不到 id"的处置是
// **记一条失败继续跑**（不像 AI 头像那条整趟停）⇒ 用回错误的那份夹具，每条 runner 用例
// 都会停在取 id 上，红成"doTask 应打 1 次实得 0 次"，把排查方向带偏。
type mxStore struct {
	fakeStore
	smid *core.SmidState
}

func (s *mxStore) GetSmidState(context.Context, core.Scope, core.AccountID) (*core.SmidState, error) {
	return s.smid, nil
}

// mxSmid 造一份设备状态；`withDevice` 决定里面带不带 `deviceId`。
func mxSmid(withDevice bool) *core.SmidState {
	fingerprint := upstream.GenerateSmFingerprint(testRng())
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		panic("夹具造不出设备特征：" + perr.Message())
	}
	feature := core.NewSecretValue(text)
	state := &core.SmidState{
		AccountID: core.NewAccountID(liveTestAccountID), FingerprintJSON: &feature,
	}
	if withDevice {
		device := core.NewSecretValue("BSYNTH-DEVICE-199")
		state.DeviceID = &device
	}
	return state
}

func mxStoreBound() *mxStore {
	auth := core.SecretValue(wholeAuth)
	return &mxStore{
		fakeStore: fakeStore{state: &core.AccountState{Authorization: &auth}},
		smid:      mxSmid(true),
	}
}

// mxStoreWithoutDeviceID 回"有特征、没有 deviceId"⇒ [DeviceID] 会真去上报换一个新的。
//
// 本文件**不给**那一枪编路由（[liveTaskScript] 对没料到的端点回 404 + 一句人话），
// 所以换 id 失败 —— 那正是这一份夹具要的状态。
func mxStoreWithoutDeviceID() *mxStore {
	bound := mxStoreBound()
	bound.smid = mxSmid(false)
	return bound
}

// mxClockAt 给一个**UTC 时刻**的时钟。
//
// ⚠ 参数刻意是 UTC：那道门读的是本地日（[appTaskLocation]），所以"差一天"那种错
// 只有在这里传 UTC 才测得出来（见 `TestTaskMatrixRunBackupGateDoesNotBlockWhenStatusMissing`
// 的 19 号 20:00 那一档）。
func mxClockAt(year int, month time.Month, day, hourUTC int) fixedClock {
	return fixedClock{at: time.Date(year, month, day, hourUTC, 0, 0, 0, time.UTC)}
}

// mxRun 装配一条链（票池由调用方传，`nil` 就是"不接池"那一档对照）。
func mxRun(
	t *testing.T, store core.StateStore, script *liveTaskScript,
	pool *SessionPool, clock core.Clock, wait Waiter,
) TaskMatrixRun {
	t.Helper()
	return TaskMatrixRun{
		SignInWorld: SignInWorld{
			Store:    store,
			Scope:    core.ScopeLocal(),
			Upstream: upstream.NewUpstream(script),
			Device: upstream.NewIPadAppDevice(
				"67F4C56A00000000000000000000315C", "8A708867DBC3"),
			Clock:    clock,
			Random:   testRng(),
			Sessions: pool,
		},
		Wait: wait,
	}
}

// mxDefaultRun 是"绑好设备 + 接池 + 不真等"那份默认装配。
func mxDefaultRun(t *testing.T, script *liveTaskScript) TaskMatrixRun {
	t.Helper()
	return mxRun(t, mxStoreBound(), script, NewSessionPool(), testClock(), &recordingWaiter{})
}

// mxResult 跑一趟。
func mxResult(t *testing.T, run TaskMatrixRun) (TaskMatrixRunResult, *core.Error) {
	t.Helper()
	return run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
}

// mxScript 把"建会话三枪 + 两趟读表 + 调用方那几条路由"编成一份脚本。
//
// ⚠ [liveTaskScript] 取**第一条**匹配的路由 ⇒ 会话那三枪只能在构造时给，
// 事后追加的那支永远轮不到（同 `play_ai_run_test.go` 里踩过的那个坑）。
// `read2` 给 nil ⇒ 复查那一趟沿用 `read1` 的最后一份（"队列用尽重复末条"那条口径）。
func mxScript(tickets, read1, read2 []string, routes ...liveTaskRoute) *liveTaskScript {
	jwts := make([]string, 0, len(tickets))
	for _, ticket := range tickets {
		jwts = append(jwts, jwtOK(ticket))
	}
	session := []liveTaskRoute{
		{"/user/querySpecTokenV2", []string{ssoOK("SSO-1")}},
		{"/ycloud/api/timestamp", []string{tsOK}},
		{"tyrzLogin", jwts},
	}
	bodies := append(append([]string{}, read1...), read2...)
	read := []liveTaskRoute{{taskListV2Endpoint, bodies}}
	return liveTaskScriptWith(append(append(read, session...), routes...)...)
}

// mxHappy 是"打卡都成功 + 入口上报成功 + 状态读得到"那三条默认路由（放在最后）。
func mxHappy() []liveTaskRoute {
	return []liveTaskRoute{
		{mxClickEndpoint, []string{mxClickOK}},
		{mxEntryEndpoint, []string{mxEntryOK}},
		{mxStatusEndpoint, []string{mxStatusBody(mxBool(true))}},
	}
}

// mxClicks 按发出顺序列出这一趟的**打卡** URL。
func mxClicks(script *liveTaskScript) []string {
	out := []string{}
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxClickEndpoint) {
			out = append(out, request.URL)
		}
	}
	return out
}

// mxURLs 按发出顺序列出全部请求 URL（比序列时用）。
func mxURLs(script *liveTaskScript) []string {
	out := []string{}
	for _, request := range script.snapshot() {
		out = append(out, request.URL)
	}
	return out
}

// mxCountPaced 数 waiter 里那一点间隔被等了几次。
func mxCountPaced(wait *recordingWaiter) int {
	n := 0
	for _, d := range wait.durations {
		if d == taskMatrixPace {
			n++
		}
	}
	return n
}

// mxVerifyNotes 把复查记录里的说明拼成一句（断言"这一枪有没有在结论里看得见"）。
func mxVerifyNotes(res TaskMatrixRunResult) string {
	parts := make([]string, 0, len(res.Verified))
	for _, record := range res.Verified {
		parts = append(parts, record.Note)
	}
	return strings.Join(parts, "；")
}

// ─────────────────────────────────────────────────────────────────────────────
// 用例
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunClicksOnTheAppDomainAndVerifies ⭐ 主路径：打哪一枪、打到哪、复查认了。
//
// 反向改坏的三种样子都会被本条抓到：换域（forbid 那两支）、多发（计数恰好两发）、
// 复查不读（`VerifiedAgain` 与那四格计数）。
func TestTaskMatrixRunClicksOnTheAppDomainAndVerifies(t *testing.T) {
	// 596 在 beiyong1 且 currstep=0 ⇒ 打卡**且**该等那一拍；777 是 default 盲点 ⇒ 不等。
	first := mxItems(
		mxItem(596, "beiyong1", "备份一个文件", "WAIT", "", 0),
		mxItem(777, "day", "看个广告", "WAIT", "", 0),
	)
	second := mxItems(
		mxItem(596, "beiyong1", "备份一个文件", "FINISH", "", 1),
		mxItem(777, "day", "看个广告", "FINISH", "", 1),
	)
	script := mxScript([]string{"JWT-1"}, first, second, mxHappy()...)
	wait := &recordingWaiter{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}

	clicks := mxClicks(script)
	if len(clicks) != 2 {
		t.Fatalf("两条待做应恰好两发打卡，实得 %v", clicks)
	}
	// 执行顺序按 id 升序（读侧那条"先排序后去重"的口径在这里的可见结果）。
	for i, want := range []string{"key=task&id=596", "key=task&id=777"} {
		if !strings.Contains(clicks[i], want) {
			t.Errorf("第 %d 发应是 %s，实得 %q（全部：%v）", i+1, want, clicks[i], mxURLs(script))
		}
		if !strings.HasPrefix(clicks[i], upstream.BaseMcloud) {
			t.Errorf("打卡必须打在 %s（另两支同名端点属别的域），实得 %q",
				upstream.BaseMcloud, clicks[i])
		}
	}
	// ⭐ 另外两支同名 `task/click` 与那两个写口一发都不该打。
	for _, forbidden := range []string{":7071", "/market/task/task/click",
		"/ycloud/signin/page/doTaskPost", "/market/signin/page/receiveTask",
		"taskListV3", mxEntryEndpoint} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("这一趟不该打 %s，实得 %d 次", forbidden, n)
		}
	}
	// ⭐ 只有走 `handleTaskClick` 的那一条等（beiyong1）；default 盲点不等。
	if got := mxCountPaced(wait); got != 1 {
		t.Errorf("该等 1 次 500ms（另一发是 default 盲点，旧实现不等），实得 %d 次：%v",
			got, wait.durations)
	}
	c := res.Counts
	if c.Read != 2 || c.Pending != 2 || c.Clicked != 2 || c.Failed != 0 {
		t.Errorf("计数应读 2 / 待做 2 / 打卡 2 / 失败 0，实得 %+v", c)
	}
	if res.Fetches != 16 || res.VerifiedAgain != 16 {
		t.Errorf("两趟各 16 次读取，实得 %d / %d", res.Fetches, res.VerifiedAgain)
	}
	if c.DoneNow != 2 {
		t.Errorf("复查该认下 2 条，实得 %+v", c)
	}
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("32 次读 + 2 次写共用一张票，建会话应 1 次，实得 %d", n)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "认下 2") {
		t.Errorf("小结要带复查读数，实得 %q", got)
	}
	if got := res.RunReport().Outcome; got != core.OutcomeSuccess {
		t.Errorf("全认下应是成功，实得 %v", got)
	}
}

// TestTaskMatrixRunNeverClicksToolGapTasks ⭐ 缺端点族 ⇒ 零枪，且界面上看得见。
//
// 这一条是"与旧实现刻意不同"那一段的行为落点：把它改回盲点打卡，本条立刻红，
// 而红出来的正是"用户的账上多了一条没做过的事"。
func TestTaskMatrixRunNeverClicksToolGapTasks(t *testing.T) {
	waiting := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, waiting, waiting, mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if clicks := mxClicks(script); len(clicks) != 0 {
		t.Errorf("缺玩法的两条一条枪都不该发，实得 %v", clicks)
	}
	if got := script.count(taskListV2Endpoint); got != 32 {
		t.Errorf("门挡住也要复查（16+16），实得 %d", got)
	}
	c := res.Counts
	if c.ToolGap != 2 || c.Clicked != 0 || c.Ran != 0 {
		t.Errorf("应报 2 条未接玩法，实得 %+v", c)
	}
	if c.StillOpen != 2 {
		t.Errorf("没做的事不该被念成完成，复查应仍待办 2 条，实得 %+v", c)
	}
	if got := res.RunReport().Outcome; got != core.OutcomePartialSuccess {
		t.Errorf("有玩法缺口应判部分成功（那是工单，不是需人工），实得 %v", got)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "2 条本工具还没接上玩法") {
		t.Errorf("小结要写明几条没接上，实得 %q", got)
	}
	// ⭐ 缺口**理由**要逐条带到结论里（那是"下次补哪一族"的唯一出处）。
	for _, record := range res.Steps {
		if record.Kind != StepBlockedToolGap || !strings.Contains(record.Reason, "未实现") {
			t.Errorf("每条缺口要逐条说清缺什么，实得 %+v", record)
		}
	}
}

// TestTaskMatrixRunRunnerSendsPreambleEntryAndFinalKey ⭐ 409 那一族的三枪与顺序。
//
// 顺序**是**判据的一部分（旧实现：前置打卡 → 入口上报 → 等 → 玩法打卡）。
// 换序在计数上看不出来，所以这里逐条比 URL 序列。
func TestTaskMatrixRunRunnerSendsPreambleEntryAndFinalKey(t *testing.T) {
	first := mxItems(mxItem(409, "day", "进入云朵中心", "WAIT", "", 0))
	second := mxItems(mxItem(409, "day", "进入云朵中心", "FINISH", "", 1))
	script := mxScript([]string{"JWT-1"}, first, second, mxHappy()...)
	wait := &recordingWaiter{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	clicks := mxClicks(script)
	if len(clicks) != 2 {
		t.Fatalf("runner 应是前置 + 玩法两发打卡，实得 %v", clicks)
	}
	if !strings.Contains(clicks[0], "key=task&id=409") {
		t.Errorf("第一发该是通用前置 key=task，实得 %q", clicks[0])
	}
	if !strings.Contains(clicks[1], "key=task2&id=409") {
		t.Errorf("409 的玩法那一发是实拍的 task2，实得 %q", clicks[1])
	}
	if got := script.count(mxEntryEndpoint); got != 1 {
		t.Errorf("入口上报应 1 次，实得 %d", got)
	}
	var entry string
	var order []string
	for _, url := range mxURLs(script) {
		switch {
		case strings.Contains(url, mxClickEndpoint):
			order = append(order, "click")
		case strings.Contains(url, mxEntryEndpoint):
			order = append(order, "entry")
			entry = url
		}
	}
	if got := strings.Join(order, ">"); got != "click>entry>click" {
		t.Errorf("三枪顺序应是 打卡>入口上报>打卡，实得 %s（%v）", got, order)
	}
	// ⚠ 这一发打在**活动侧那个域**，与打卡那支不同（`app_task_click.go` 文件头那张表）。
	if !strings.HasPrefix(entry, upstream.BaseMrpMcloud) ||
		!strings.Contains(entry, "client=app") ||
		!strings.Contains(entry, "deviceId=BSYNTH-DEVICE-199") {
		t.Errorf("入口上报要打在 mrp 域、带 client=app 与库里那个 deviceId，实得 %q", entry)
	}
	if got := mxCountPaced(wait); got != 1 {
		t.Errorf("runner 走 handleTaskClick ⇒ 该等 1 次，实得 %d（%v）", got, wait.durations)
	}
	if res.Counts.Ran != 1 || res.Counts.Clicked != 0 {
		t.Errorf("这一条该记成玩法而非普通打卡，实得 %+v", res.Counts)
	}
	if res.Counts.DoneNow != 1 {
		t.Errorf("复查应认下这一条，实得 %+v", res.Counts)
	}
	// 结论要能说出是哪条 runner 做的（"这条是谁做的"只有这一处能回答）。
	if !strings.Contains(res.Steps[0].Runner, "云朵中心入口") {
		t.Errorf("Runner 名要进记录，实得 %+v", res.Steps[0])
	}
}

// TestTaskMatrixRunRunnerSkipsPreambleWhenStepNonZero：门挡的是前置那一发，**不是**玩法。
//
// 反过来（整条跳过）的表现是"这个号的云朵中心入口永远没报过"，而结论里一条错都没有。
func TestTaskMatrixRunRunnerSkipsPreambleWhenStepNonZero(t *testing.T) {
	mid := mxItems(mxItem(409, "day", "进入云朵中心", "WAIT", "", 3))
	script := mxScript([]string{"JWT-1"}, mid, mid, mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	clicks := mxClicks(script)
	// 409 不在复查补打卡那两条里 ⇒ 全程只有玩法那一发。
	if len(clicks) != 1 || !strings.Contains(clicks[0], "key=task2&id=409") {
		t.Errorf("currstep=3 ⇒ 前置不发、玩法照发，实得 %v", clicks)
	}
	if got := script.count(mxEntryEndpoint); got != 1 {
		t.Errorf("玩法本体该照跑（入口上报 1 次），实得 %d", got)
	}
	if res.Counts.Ran != 1 || res.Counts.RuleSkipped != 0 {
		t.Errorf("这一条仍算跑了玩法（不是按规则跳过），实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunContinuesAfterClickFailure：单条失败不中断，原文进结论。
//
// 判反的两个方向：一次失败就中断 ⇒ 表里靠前的条目坏掉时后面全没跑；
// 失败被吞 ⇒ 界面上"上报 2 条"，而其中一条上游根本没接。
func TestTaskMatrixRunContinuesAfterClickFailure(t *testing.T) {
	first := mxItems(
		mxItem(777, "day", "先坏的那条", "WAIT", "", 0),
		mxItem(778, "day", "后面的那条", "WAIT", "", 0),
	)
	second := mxItems(
		mxItem(777, "day", "先坏的那条", "WAIT", "", 0),
		mxItem(778, "day", "后面的那条", "FINISH", "", 1),
	)
	// ⚠ 打卡那条路由放在 `mxHappy()` **之前**：同一条 URL 命中先来的那条。
	script := mxScript([]string{"JWT-1"}, first, second, append(
		[]liveTaskRoute{{mxClickEndpoint, []string{mxClickBizFailure, mxClickOK}}},
		mxHappy()[1:]...)...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("单条打卡失败不该让整趟报错: %v", perr)
	}
	if clicks := mxClicks(script); len(clicks) != 2 {
		t.Errorf("第一条失败后该继续打第二条，实得 %v", clicks)
	}
	c := res.Counts
	if c.Failed != 1 || c.Clicked != 1 {
		t.Errorf("应 1 失败 1 成功，实得 %+v", c)
	}
	if got := res.Steps[0].Err; !strings.Contains(got, "任务已过期") {
		t.Errorf("上游原文要留在结论里，实得 %q", got)
	}
	if c.NotAcknowledged != 1 || c.DoneNow != 1 {
		t.Errorf("复查应把那条念成做了没认，实得 %+v", c)
	}
	if got := res.RunReport().Outcome; got != core.OutcomePartialSuccess {
		t.Errorf("有失败/没认应判部分成功，实得 %v", got)
	}
}

// TestTaskMatrixRunSkipVerifyOnlyWhenNothingSent ⭐ 省复查那一格**不许多省**。
//
// 两档对照（同一份脚本形状、只差表里有没有可发的条目）：一枪没发 ⇒ 省掉第二趟；
// 发过一枪 ⇒ `SkipVerify` 无效。反过来（发过也省）的表现是
// "结论说做了 3 条，而没有任何一处证据说上游认了"。
func TestTaskMatrixRunSkipVerifyOnlyWhenNothingSent(t *testing.T) {
	for _, c := range []struct {
		name  string
		items []string
		want  int
	}{
		{"一枪没发（只有缺口）", mxItems(mxItem(106, "day", "上传", "WAIT", "", 0)), 16},
		{"发过一枪", mxItems(mxItem(777, "day", "看广告", "WAIT", "", 0)), 32},
	} {
		script := mxScript([]string{"JWT-1"}, c.items, nil, mxHappy()...)
		run := mxDefaultRun(t, script)
		run.SkipVerify = true
		res, perr := mxResult(t, run)
		if perr != nil {
			t.Fatalf("%s 跑一趟失败: %v", c.name, perr)
		}
		if got := script.count(taskListV2Endpoint); got != c.want {
			t.Errorf("%s：读取应是 %d 次，实得 %d", c.name, c.want, got)
		}
		if c.want == 16 && !strings.Contains(res.SummaryLine(), "未复查") {
			t.Errorf("%s：省掉复查要在小结里说出来，实得 %q", c.name, res.SummaryLine())
		}
	}
}

// TestTaskMatrixRunBackupGateIsReportedAsManual ⭐ 未开备份 ⇒ 派给人，不派给机器。
func TestTaskMatrixRunBackupGateIsReportedAsManual(t *testing.T) {
	backup := false
	waiting := mxItems(mxItem(801, "month", "每月签满 7 天", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, waiting,
		[]liveTaskRoute{
			{mxStatusEndpoint, []string{mxStatusBody(&backup)}},
			{mxClickEndpoint, []string{mxClickOK}},
		}...)
	run := mxRun(t, mxStoreBound(), script, NewSessionPool(),
		mxClockAt(2026, time.September, 15, 3), &recordingWaiter{})
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if clicks := mxClicks(script); len(clicks) != 0 {
		t.Errorf("被门挡住的每月任务一发都不该打，实得 %v", clicks)
	}
	if res.Counts.NeedsHuman != 1 {
		t.Errorf("应报 1 条需人工，实得 %+v", res.Counts)
	}
	report := res.RunReport()
	if report.Outcome != core.OutcomeManualRequired {
		t.Errorf("需人工那一档优先于其余结论，实得 %v", report.Outcome)
	}
	if len(report.ManualSteps) != 1 ||
		!strings.Contains(report.ManualSteps[0].Instruction, "自动备份") {
		t.Errorf("人工步骤要写明要去做什么，实得 %+v", report.ManualSteps)
	}
	// 这一趟一枪没发 ⇒ 复查仍然照打（`SkipVerify` 没开，那与门无关）。
	if got := script.count(taskListV2Endpoint); got != 32 {
		t.Errorf("默认仍该复查一次，实得 %d", got)
	}
}

// TestTaskMatrixRunBackupGateDoesNotBlockWhenStatusMissing：读不到 ⇒ 不挡。
//
// 与上一条是一对（旧实现那句是严格 `=== false`）。反过来（读不到就当没开）的表现是
// 一次 `infoV3` 抖动让整组每月任务"今天没事可做"。
//
// ⭐ 19 号 20:00 UTC 那一档钉的是**时区**：那一刻本地已是 20 号 ⇒ 门不该挡。
// 拿 UTC 的日去比（`Clock.Now().Day()`），那一档会判成"19 号"而把任务挡掉。
func TestTaskMatrixRunBackupGateDoesNotBlockWhenStatusMissing(t *testing.T) {
	for _, c := range []struct {
		name       string
		body       string
		clock      fixedClock
		wantManual bool
	}{
		{"读到 false · 15 号", mxStatusBody(mxBool(false)),
			mxClockAt(2026, time.September, 15, 3), true},
		{"读到 false · 20 号当天", mxStatusBody(mxBool(false)),
			mxClockAt(2026, time.September, 20, 3), false},
		{"读到 false · 19 号 20:00 UTC（本地已 20 号）", mxStatusBody(mxBool(false)),
			mxClockAt(2026, time.September, 19, 20), false},
		// 上游没给那一格 ⇒ 与"读到 true"同向（不挡）。
		{"没给备份字段 · 15 号", mxStatusBody(nil),
			mxClockAt(2026, time.September, 15, 3), false},
		{"读到 true · 15 号", mxStatusBody(mxBool(true)),
			mxClockAt(2026, time.September, 15, 3), false},
	} {
		waiting := mxItems(mxItem(801, "month", "每月签满 7 天", "WAIT", "", 0))
		script := mxScript([]string{"JWT-1"}, waiting, waiting,
			[]liveTaskRoute{
				{mxStatusEndpoint, []string{c.body}},
				{mxClickEndpoint, []string{mxClickOK}},
			}...)
		run := mxRun(t, mxStoreBound(), script, NewSessionPool(), c.clock, &recordingWaiter{})
		res, perr := mxResult(t, run)
		if perr != nil {
			t.Fatalf("%s 跑一趟失败: %v", c.name, perr)
		}
		got := res.Counts.NeedsHuman > 0
		if got != c.wantManual {
			t.Errorf("%s：需人工应 %v，实得 %v（%+v）", c.name, c.wantManual, got, res.Counts)
		}
		clicks := mxClicks(script)
		if c.wantManual && len(clicks) != 0 {
			t.Errorf("%s：挡下时不该有枪，实得 %v", c.name, clicks)
		}
		if !c.wantManual && len(clicks) == 0 {
			t.Errorf("%s：门不该挡 ⇒ 该有那一发盲点，实得 %v", c.name, clicks)
		}
	}
}

// TestTaskMatrixRunVerifyFailureKeepsTheFirstRound：复查挂了不推翻"枪已经发出去了"。
func TestTaskMatrixRunVerifyFailureKeepsTheFirstRound(t *testing.T) {
	first := mxItems(mxItem(777, "day", "看个广告", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, mxAllFail(), mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("复查失败不该让整趟失败: %v", perr)
	}
	if len(res.Notes) == 0 || !strings.Contains(res.Notes[0], "复查没读到") {
		t.Errorf("要写明完成数无法核对，实得 %v", res.Notes)
	}
	if res.Counts.Clicked != 1 {
		t.Errorf("第一趟的读数不该被复查带跑，实得 %+v", res.Counts)
	}
	if res.VerifiedAgain != 0 {
		t.Errorf("复查没读成 ⇒ 那一趟的读数不该记账，实得 %d", res.VerifiedAgain)
	}
	if got := res.RunReport().Outcome; got != core.OutcomeSuccess {
		t.Errorf("发出去且上游回了成功 ⇒ 不该判红，实得 %v", got)
	}
}

// TestTaskMatrixRunCountsReadFailures ⭐ 残表不许被念成全表。
func TestTaskMatrixRunCountsReadFailures(t *testing.T) {
	overrides := map[int]string{0: mxBody(mxItem(777, "day", "看个广告", "WAIT", "", 0))}
	overrides[3] = `{"code":1,"msg":"这一组挂了"}`
	script := mxScript([]string{"JWT-1"}, mxRound(overrides), mxItems(), mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if res.ReadFailures != 1 {
		t.Errorf("应记下 1 组没读到，实得 %d", res.ReadFailures)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "1/16 组没读到") {
		t.Errorf("小结要写明这不是全表，实得 %q", got)
	}
}

// TestTaskMatrixRunVerifyReClicksRandomCloudTasks：复查那两枚补打卡（key 零实拍）。
func TestTaskMatrixRunVerifyReClicksRandomCloudTasks(t *testing.T) {
	both := mxItems(
		mxItem(481, "day", "随机云朵 A", "WAIT", "", 2),
		mxItem(478, "day", "随机云朵 B", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, both, both, mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	// 第一趟：两条各一发盲点（default 不过门）；复查趟：两条各补一发 `randomCloudTask`。
	clicks := mxClicks(script)
	if len(clicks) != 4 {
		t.Fatalf("应 2 发盲点 + 2 发补打卡，实得 %v", clicks)
	}
	var preamble, again []string
	for _, url := range clicks {
		if strings.Contains(url, "key=randomCloudTask") {
			again = append(again, url)
			continue
		}
		preamble = append(preamble, url)
	}
	if len(preamble) != 2 || len(again) != 2 {
		t.Fatalf("两发的种类对不上：前置 %v / 补的 %v", preamble, again)
	}
	for _, want := range []string{"id=481", "id=478"} {
		if !strings.Contains(strings.Join(again, " "), want) {
			t.Errorf("补打卡要点名是哪条（%s），实得 %v", want, again)
		}
	}
	if notes := mxVerifyNotes(res); !strings.Contains(notes, "零实拍") {
		t.Errorf("这一枚 key 没有实拍支持，必须在结论里看得见，实得 %q", notes)
	}
}

// TestTaskMatrixRunEntryReportFailureIsRecordedNotFatal：入口上报挂了 ⇒ 记一条、继续跑。
//
// 这一族今天零实拍，所以"上游回了失败"正是最需要看得到的一档：静默吞掉的话，
// 界面上只剩"玩法跑了 1 条"。
func TestTaskMatrixRunEntryReportFailureIsRecordedNotFatal(t *testing.T) {
	first := mxItems(mxItem(409, "day", "进入云朵中心", "WAIT", "", 0))
	second := mxItems(mxItem(409, "day", "进入云朵中心", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, second, append(
		[]liveTaskRoute{
			{mxEntryEndpoint, []string{`{"code":1,"success":false,"msg":"活动已结束"}`}},
		}, mxHappy()...)...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("入口上报失败不该让整趟报错: %v", perr)
	}
	if len(res.Steps) != 1 || !strings.Contains(res.Steps[0].Err, "入口上报失败") {
		t.Fatalf("要写明挂的是入口上报那一发，实得 %+v", res.Steps)
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应记 1 条失败（而不是仍算成功的一条玩法），实得 %+v", res.Counts)
	}
	if clicks := mxClicks(script); len(clicks) != 2 {
		t.Errorf("前置与玩法那两发仍该打（旧实现不因为一发挂掉就收尾），实得 %v", clicks)
	}
	if res.Counts.NotAcknowledged != 1 {
		t.Errorf("做了没认这一格该涨，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunHiddenGroupCountedSeparately：复查看到 `hidden` 组 ⇒ 只计数、不进那四格。
func TestTaskMatrixRunHiddenGroupCountedSeparately(t *testing.T) {
	first := mxItems(mxItem(900, "day", "一条待做", "WAIT", "", 0))
	second := mxItems(
		mxItem(900, "day", "一条待做", "FINISH", "", 1),
		mxItem(901, "hidden", "投放中的隐藏任务", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, first, second, mxHappy()...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if res.Counts.Hidden != 1 {
		t.Errorf("隐藏组要有自己的计数（否则会被念成仍待办），实得 %+v", res.Counts)
	}
	if res.Counts.StillOpen != 0 {
		t.Errorf("隐藏组不该混进那四格，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunWithoutPoolReopensSessionEachRound ⭐ 漏接池的代价是可数的。
//
// 与 [TestAppTaskMatrixTwoRoundsShareOneTicket] 同一对照法（同脚本、只差那份池）。
// 这一条特意挂在**写侧**：写侧一趟就有 32 读 + N 写，漏接池的表现是每趟多三枪、
// 其中一枪还是真换票。
func TestTaskMatrixRunWithoutPoolReopensSessionEachRound(t *testing.T) {
	items := mxItems(mxItem(777, "day", "看个广告", "WAIT", "", 0))
	for _, c := range []struct {
		name      string
		pool      *SessionPool
		wantOpens int
	}{
		{"接池", NewSessionPool(), 1},
		{"不接池", nil, 2},
	} {
		script := mxScript([]string{"JWT-1", "JWT-2"}, items, items, mxHappy()...)
		for round := 0; round < 2; round++ {
			run := mxRun(t, mxStoreBound(), script, c.pool, testClock(), &recordingWaiter{})
			if _, perr := mxResult(t, run); perr != nil {
				t.Fatalf("%s 第 %d 轮失败: %v", c.name, round+1, perr)
			}
		}
		if got := script.count("tyrzLogin"); got != c.wantOpens {
			t.Errorf("%s：两轮合计应建会话 %d 次，实得 %d", c.name, c.wantOpens, got)
		}
	}
}

// TestTaskMatrixRunEntryDeviceFailureDoesNotSinkTheRun ⭐ 取不到 deviceId ⇒ 只挡住需要它的那条。
//
// 这一条钉的是"一个活动侧接口挂了不该让整趟变成这个号没跑成"——
// 反过来的表现是 409 那一条把 777 也带崩。
func TestTaskMatrixRunEntryDeviceFailureDoesNotSinkTheRun(t *testing.T) {
	first := mxItems(
		mxItem(409, "day", "进入云朵中心", "WAIT", "", 0),
		mxItem(777, "day", "看个广告", "WAIT", "", 0),
	)
	second := mxItems(
		mxItem(409, "day", "进入云朵中心", "WAIT", "", 0),
		mxItem(777, "day", "看个广告", "FINISH", "", 1),
	)
	// 不给 smid 上报那一枪编路由 ⇒ 换不到 deviceId（404 + 一句"没有路由"）。
	script := mxScript([]string{"JWT-1"}, first, second, mxHappy()...)
	run := mxRun(t, mxStoreWithoutDeviceID(), script, NewSessionPool(), testClock(),
		&recordingWaiter{})
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("取不到 deviceId 不该让整趟失败: %v", perr)
	}
	if len(res.Steps) != 2 {
		t.Fatalf("两条任务都该有记录，实得 %+v", res.Steps)
	}
	if !strings.Contains(res.Steps[0].Err, "deviceId") {
		t.Errorf("409 那条要写明卡在取 deviceId，实得 %+v", res.Steps[0])
	}
	// ⚠ 这里钉的是**另一条**：409 自己那一条该记失败（`Failed` 里有它一格），
	// 777 的盲点必须干净。拿聚合计数当"谁失败了"会看不出是谁，所以两条一起断。
	if res.Counts.Clicked != 1 || res.Counts.Failed != 1 || res.Counts.Ran != 0 {
		t.Errorf("应 1 条打卡成功 + 1 条玩法失败，实得 %+v", res.Counts)
	}
	if got := res.Steps[1]; got.TaskID != 777 || got.Kind != StepClick || got.Err != "" {
		t.Errorf("777 不该被 409 带崩，实得 %+v", got)
	}
	if got := script.count(mxEntryEndpoint); got != 0 {
		t.Errorf("换不到 id 就不该发出入口上报（旧实现会发一发空 deviceId），实得 %d", got)
	}
}
