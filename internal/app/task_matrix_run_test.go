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
	"fmt"
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
	// mxClick58999 **逐字**取自 2026-09-25 用户提供的线上采集（同一个形状也钉在
	// `internal/upstream/envelope_test.go`）。放两份的理由不同：上游那份验"错误文本里有它"，
	// 这一份验"**这一格能不能一路走到逐条播报那一行**"—— app 层不碰信封的键，
	// 所以这一条是"接线没断"的证据，不是解析的证据。
	mxClick58999 = `{"code":58999,"message":"当前用户信息读取异常，请尝试重新登录或重启手机",` +
		`"detail":"任务不存在","tipMsg":null}`
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
		// 清理那一枪的**默认档 = 删成功**。2026-09-25 起链尾真的会发这一枪
		// （[TaskMatrixRun.KeepUploadedFiles] 没设时），所以"跑通的一趟"必须带上它，
		// 否则每一跑都平白多一条"清理失败 404"。夹具取**第一条**匹配 ⇒ 想演
		// "上游拒了"的用例把自定义那一支放在**前面**即可（见 mxTrashRoute）。
		{mxTrashEndpoint, []string{mxTrashOKBody}},
	}
}

// mxTrashOKBody 是 `batchTrash` 的成功响应（判据只有顶层 `success`）。
const mxTrashOKBody = `{"code":"0000","message":"成功","success":true,"result":{}}`

// mxTrashRoute 给清理那一枪指定响应（成功 / 被拒两档都用它）。
func mxTrashRoute(body string) liveTaskRoute {
	return liveTaskRoute{mxTrashEndpoint, []string{body}}
}

// mxTrashBodies 取出这一趟**清理那一枪**的请求体（最多一枚；没有则空）。
func mxTrashBodies(script *liveTaskScript) []string {
	var bodies []string
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxTrashEndpoint) {
			bodies = append(bodies, string(request.Body))
		}
	}
	return bodies
}

// mxTrashCount 数清理那一枪发了几次（"一次删多个"⇒ 期望恰好 1）。
func mxTrashCount(script *liveTaskScript) int {
	return len(mxTrashBodies(script))
}

// mxUploads 是"往网盘传文件"那三枪的端点与夹具（#36 接进这一条链的那一半）。
//
// ⚠ 全部响应**都是合成**的：这一族在本仓所有归档里零实拍（R27）。所以这些用例
// 钉的是"编排层把三枪按顺序发出去、id 记进账"，**不是**"上游会这样回"。
const (
	mxCreateEndpoint   = "/hcy/file/create"
	mxCompleteEndpoint = "/hcy/file/complete"
	// mxTrashEndpoint 是清理那一枪（第四枪）。它**不**在 [mxUploadRoutes] 里：
	// 那一族只给"传出去"的三枪，删不删是链尾另一步（见 [mxTrashRoutes]），
	// 这样"忘了接清理"与"接了但上游拒了"在夹具里是两件事。
	mxTrashEndpoint = "/hcy/recyclebin/batchTrash"
	// mxPutHost 是预签名直传地址的**主机**（路由按子串匹配）。真实那一枪的 URL 里
	// 带一次性签名 ⇒ 夹具用合成域，且任何断言都**不**比整串 URL。
	mxPutHost = "synth-oss.example.invalid"
	mxPutURL  = "https://synth-oss.example.invalid/put/SYNTH-TOKEN"
)

// mxCreateOK 造一份 `file/create` 的成功响应；`n` 进 fileId，便于"传了几个就是几个 id"。
func mxCreateOK(n int) string {
	return `{"code":"0000","success":true,"data":{"exist":false,"rapidUpload":false,` +
		`"fileId":"SYNTH-FID-` + itoa64(int64(n)) + `","uploadId":"SYNTH-UID",` +
		`"partInfos":[{"uploadUrl":"` + mxPutURL + `"}]}}`
}

// mxUploadRoutes 给 n 轮上传用的三条路由（create 回 n 份不同的 fileId）。
func mxUploadRoutes(n int) []liveTaskRoute {
	creates := make([]string, 0, n)
	for i := 1; i <= n; i++ {
		creates = append(creates, mxCreateOK(i))
	}
	return []liveTaskRoute{
		{mxCreateEndpoint, creates},
		{mxPutHost, []string{""}},
		{mxCompleteEndpoint, []string{`{"code":"0000","success":true}`}},
	}
}

// mxUploadBodies 是"这一趟一个上传请求都没打"那一份断言要数的那三枚。
func mxUploadCounts(script *liveTaskScript) (creates, puts, completes int) {
	for _, request := range script.snapshot() {
		switch {
		case strings.Contains(request.URL, mxCreateEndpoint):
			creates++
		case strings.Contains(request.URL, mxCompleteEndpoint):
			completes++
		case strings.Contains(request.URL, mxPutHost):
			puts++
		}
	}
	return creates, puts, completes
}

// mxItemProcessAt 把条目里的 `process` 改成给定值（522 那一格停表用得上）。
//
// ⚠ [mxItem] 造的默认是 `"process":0`（与实拍那三条同形）。若停表与上限共用 0，
// "把 process 判反方向"这种错不会红 ⇒ 这一格必须能单独给值。
func mxItemProcessAt(item string, process int64) string {
	return strings.Replace(item, `"process":0`, `"process":`+itoa64(process), 1)
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
//
// ⚠ 这里用的两个 id 是**仍缺端点族**的那两个（都在 139 邮箱那一族）。106 曾在这张表里，
// 2026-09-24 上传族接上后它落到 runner 那一支了（用例见
// [TestTaskMatrixRunUploadsOneFileForTask106]）；107 也一样 —— 笔记那一族三口于
// 2026-09-25 落码，它今天会**真的发三枪** ⇒ 再拿它当"缺口"样本，这一条测的就不是
// "不发枪"而是"发错了还绿着"。
func TestTaskMatrixRunNeverClicksToolGapTasks(t *testing.T) {
	waiting := mxItems(
		mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0),
		mxItem(473, "day", "阅读邮件", "WAIT", "", 0),
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
	// 这里只钉"没被丢成空白"：**具体措辞与互不相同**由 `taskMatrix_dispatch_test.go`
	// 里那条按表遍历的用例钉 —— 表已经改过口径（上传那一族 2026-09-24 从"未实现"
	// 变成"已落码、缺调用点"），在这一层再锁一个子串就会锁住"该说哪一族"之外的东西。
	for _, record := range res.Steps {
		if record.Kind != StepBlockedToolGap || record.Reason == "" {
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
		{"一枪没发（只有缺口）", mxItems(mxItem(472, "day", "体验 139 邮箱", "WAIT", "", 0)), 16},
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

// ─────────────────────────────────────────────────────────────────────────────
// 上传那一半（106 / 522 与 beiyong1 的"名字写着上传"）
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunUploadsOneFileForTask106 ⭐ 主路径：打卡 → 三枪 → 记账。
//
// 反向改坏的三种样子都会被本条抓到：漏掉玩法（create 0 次）、把三枪打成两枪
// （PUT 或 complete 缺席）、多打一发（计数恰好）。
//
// ⚠ 顺序**是**判据：`file/create` 占位 → `PUT` 预签名 → `file/complete`，
// 换序在计数上看不出来，所以逐条比 URL 序列。
func TestTaskMatrixRunUploadsOneFileForTask106(t *testing.T) {
	first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
	second := mxItems(mxItem(106, "day", "上传一个文件", "FINISH", "", 1))
	script := mxScript([]string{"JWT-1"}, first, second,
		append(mxUploadRoutes(1), mxHappy()...)...)
	wait := &recordingWaiter{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(mxClickEndpoint); n != 1 {
		t.Errorf("106 只有前置那一发打卡（runner 体自己不打卡），实得 %d", n)
	}
	creates, puts, completes := mxUploadCounts(script)
	if creates != 1 || puts != 1 || completes != 1 {
		t.Errorf("该恰好三枪，实得 create %d / PUT %d / complete %d", creates, puts, completes)
	}
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
		}
	}
	if got := strings.Join(order, ">"); got != "click>create>put>complete" {
		t.Errorf("时序应是 打卡>create>PUT>complete，实得 %s", got)
	}
	// 三枪的鉴权画像：`Authorization` 整串来自凭据、带 `deviceid`、**不**带 jwttoken
	// （personal-kd 那台不在旧 TS 的 TARGET_HOSTNAMES 里，成文见 app_cloud_upload.go 文件头）。
	var create *core.HTTPRequest
	for i, request := range script.snapshot() {
		if strings.Contains(request.URL, mxCreateEndpoint) {
			create = &script.snapshot()[i]
			break
		}
	}
	if create == nil {
		t.Fatal("没找到 create 那一枪")
	}
	if got := headerValue(*create, "Authorization"); got != wholeAuth {
		t.Errorf("create 要带库里的整串 Authorization，实得 %q", got)
	}
	if got := headerValue(*create, "deviceid"); got != "BSYNTH-DEVICE-199" {
		t.Errorf("create 要带库里那个 deviceId，实得 %q", got)
	}
	if got := headerValue(*create, "jwttoken"); got != "" {
		t.Errorf("这一族不带 jwttoken（旧 TS 只对三台主机发它，personal-kd 不在其中），实得 %q", got)
	}
	// 计数与账本。
	if res.Counts.Uploaded != 1 {
		t.Errorf("应记 1 个文件，实得 %+v", res.Counts)
	}
	if len(res.CloudFileIDs) != 1 || res.CloudFileIDs[0] != "SYNTH-FID-1" {
		t.Errorf("fileId 要一路带到结果（清理那一族的输入），实得 %v", res.CloudFileIDs)
	}
	if res.Steps[0].Uploads != 1 || res.Steps[0].Err != "" {
		t.Errorf("这一条该记成「传了一个、没失败」，实得 %+v", res.Steps[0])
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("类别仍是专属玩法，实得 %+v", res.Counts)
	}
	// 旧实现这一条只等**一拍**（runner 体里没有 sleep，只有 handleTaskClick 末尾那一句）。
	if got := mxCountPaced(wait); got != 1 {
		t.Errorf("106 应只等 1 拍（循环体里不等），实得 %d 次：%v", got, wait.durations)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "网盘新增 1 个文件（已移进回收站）") {
		t.Errorf("写了几个、删没删必须在小结里看得见，实得 %q", got)
	}
	if res.Counts.DoneNow != 1 {
		t.Errorf("复查该认下这一条，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunUploadLoopForTask522 ⭐ 循环那一档：五轮 × 三枪，每轮之间等一拍。
//
// 这一条钉的是"上限兜住最坏情况"：`process` 那一格语义未验证（R15），实拍恒 0 ⇒
// 停表永远不响，真正决定"这一趟写几个文件"的是 [taskMatrixUploadRoundsPerRun]。
// 有人把上限当"反正会停"删掉，本条会红在计数上。
func TestTaskMatrixRunUploadLoopForTask522(t *testing.T) {
	first := mxItems(mxItem(522, "month", "上传满100个", "WAIT", "", 0))
	second := mxItems(mxItem(522, "month", "上传满100个", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, second,
		append(mxUploadRoutes(taskMatrixUploadRoundsPerRun), mxHappy()...)...)
	wait := &recordingWaiter{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	rounds := taskMatrixUploadRoundsPerRun
	if creates, puts, completes := mxUploadCounts(script); creates != rounds ||
		puts != rounds || completes != rounds {
		t.Errorf("应 %d 轮各三枪，实得 create %d / PUT %d / complete %d",
			rounds, creates, puts, completes)
	}
	if res.Counts.Uploaded != rounds || len(res.CloudFileIDs) != rounds {
		t.Errorf("应传 %d 个、记 %d 个 id，实得 %+v / %v",
			rounds, rounds, res.Counts, res.CloudFileIDs)
	}
	// 522 旧实现每轮末尾 sleep（含末轮），之后 `handleTaskClick` 再 sleep 一次
	// ⇒ 共 rounds+1 拍。少一拍就是改了节奏，多一拍同理。
	if got := mxCountPaced(wait); got != rounds+1 {
		t.Errorf("应等 %d 拍（每轮一拍 + 末尾一拍），实得 %d：%v", rounds+1, got, wait.durations)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "网盘新增 5 个文件（已移进回收站）") {
		t.Errorf("小结要说清写了几个，实得 %q", got)
	}
}

// TestTaskMatrixRunUploadLoopStopsAtProcess：读到 `process` 已达 100 ⇒ 一个都不传。
//
// ⚠ 这一档**只钉形状**：这里给的是"旧实现那句 `task.process >= 100` 会停"的形状。
// 2026-09-25 同日两趟真跑给了它一条**旁证**（不是证明）：522 那一趟传 5 个文件 ⇒ 复查读到
// `process` 从 64 涨到 69；另一号该条早已 `process=100` 且当天读成 `FINISH`（见 R15 与
// `docs/status.md`「同日第六趟真跑」）。上游从没**声明**过这一格是百分比 ⇒ 仍按未验证对待。
func TestTaskMatrixRunUploadLoopStopsAtProcess(t *testing.T) {
	first := mxItems(mxItemProcessAt(mxItem(522, "month", "上传满100个", "WAIT", "", 0), 100))
	// ⚠ 那一发打卡要有成功路由：缺了它 404 会让前置那一发失败 ⇒ 玩法整条不跑，
	// 这条用例红的是"为什么没停表"，而真正挡在前面的是打卡 —— 那已经是另一条用例（
	// [TestTaskMatrixRunPreambleClickFailureWritesNoFiles]）钉的事了。
	script := mxScript([]string{"JWT-1"}, first, first,
		append(mxUploadRoutes(1), mxHappy()...)...)
	wait := &recordingWaiter{}
	run := mxDefaultRun(t, script)
	run.Wait = wait
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if creates, puts, completes := mxUploadCounts(script); creates+puts+completes != 0 {
		t.Errorf("process 已达上限 ⇒ 三枪都不该发，实得 %d/%d/%d", creates, puts, completes)
	}
	if n := script.count(mxClickEndpoint); n != 1 {
		t.Errorf("停的是上传，那一发打卡照旧实现仍该打，实得 %d", n)
	}
	// 停表这一档本身不该有失败：有 Err 说明这一趟被别的一枪挡住了，
	// 而"三枪 0 发"就成了一句碰巧成立的断言。
	if text := res.Steps[0].Err; text != "" {
		t.Errorf("只该停表、不该报失败（实得 %q）—— 先确认打卡那一枪有成功路由", text)
	}
	// 那一拍**照等**：旧实现末尾的 `sleep(500)` 在 `handleTaskClick` 里，条件是"打卡发出去了"
	// 而不是"上传传成了" ⇒ 这一档只有 `handleTaskClick` 末尾那一拍，没有每轮那一拍。
	if got := mxCountPaced(wait); got != 1 {
		t.Errorf("停表只免掉每轮那一拍，末尾那一拍照旧实现仍该等，实得 %d：%v", got, wait.durations)
	}
	if res.Counts.Uploaded != 0 || len(res.CloudFileIDs) != 0 {
		t.Errorf("停表时不该留下任何账，实得 %+v / %v", res.Counts, res.CloudFileIDs)
	}
	if note := res.Steps[0].UploadNote; !strings.Contains(note, "process=100") {
		t.Errorf("为什么一个都没传要说清（否则界面上只看到「跑了玩法、0 个文件」），实得 %q", note)
	}
}

// TestTaskMatrixRunPreambleClickFailureWritesNoFiles ⭐ 前置那一发挂了 ⇒ 整条玩法不跑。
//
// 旧实现 `handleTaskClick` 的 `if` 包着 runner 那一句。这一格在只有 409 / 1005 时
// 只是"少一发入口上报"，接上传之后它决定的是"**上游都没接这条任务，还要不要往
// 用户网盘写一个文件**"—— 那是这一条链上最贵的一种"多做了"。
func TestTaskMatrixRunPreambleClickFailureWritesNoFiles(t *testing.T) {
	first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, first, append(
		[]liveTaskRoute{{mxClickEndpoint, []string{mxClickBizFailure}}},
		mxUploadRoutes(1)...)...)
	res, perr := mxResult(t, mxDefaultRun(t, script))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if creates, puts, completes := mxUploadCounts(script); creates+puts+completes != 0 {
		t.Errorf("前置打卡回失败 ⇒ 一枪上传都不该发，实得 %d/%d/%d", creates, puts, completes)
	}
	if len(res.CloudFileIDs) != 0 {
		t.Errorf("网盘里不该多任何东西，实得 %v", res.CloudFileIDs)
	}
	if res.Counts.Uploaded != 0 {
		t.Errorf("计数也不该涨，实得 %+v", res.Counts)
	}
	if got := res.Steps[0].Err; !strings.Contains(got, "整条不跑") {
		t.Errorf("要说出「玩法被前置那一发挡住了」，否则只看到一条打卡失败，实得 %q", got)
	}
	// ⭐ 但它仍算"今天动过"：那一发确实出网了（口径见 apply 函数头）。
	if res.Counts.Failed != 1 || res.Counts.NotAcknowledged != 1 {
		t.Errorf("发过但没认 ⇒ 落在「做了没认」，不是「仍待办」，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunBeiyong1UploadsAfterThePace ⭐ beiyong1 那一支：门只挡打卡，不挡上传。
//
// 两档对照：`currstep=0` 打卡 + 等一拍 + 传一个；`currstep=4` **不打卡**、
// 但仍等那一拍、仍传那一个（旧实现那句 `clickTaskWithStep` 回 true）。
// 把两格混成一个的表现：实拍那三条 `currstep` 4/6/8 的上传任务一条都不做。
func TestTaskMatrixRunBeiyong1UploadsAfterThePace(t *testing.T) {
	for _, c := range []struct {
		name      string
		step      int64
		wantClick int
	}{
		{"门放行（currstep=0）", 0, 1},
		{"门挡住打卡（currstep=4）", 4, 0},
	} {
		first := mxItems(mxItem(598, "beiyong1", "上传照片到云端", "WAIT", "", c.step))
		script := mxScript([]string{"JWT-1"}, first, first,
			append(mxUploadRoutes(1), mxHappy()...)...)
		wait := &recordingWaiter{}
		run := mxDefaultRun(t, script)
		run.Wait = wait
		res, perr := mxResult(t, run)
		if perr != nil {
			t.Fatalf("%s 跑一趟失败: %v", c.name, perr)
		}
		if n := script.count(mxClickEndpoint); n != c.wantClick {
			t.Errorf("%s：打卡应 %d 发，实得 %d", c.name, c.wantClick, n)
		}
		if creates, puts, completes := mxUploadCounts(script); creates != 1 ||
			puts != 1 || completes != 1 {
			t.Errorf("%s：门挡的是打卡，三枪仍该发，实得 %d/%d/%d",
				c.name, creates, puts, completes)
		}
		// 那一拍在**上传之前**（旧实现：sleep 在 handleTaskClick 里，上传在它外面）。
		if got := mxCountPaced(wait); got != 1 {
			t.Errorf("%s：应等 1 拍且在上传之前，实得 %d：%v", c.name, got, wait.durations)
		}
		var order []string
		for _, url := range mxURLs(script) {
			switch {
			case strings.Contains(url, mxClickEndpoint):
				order = append(order, "click")
			case strings.Contains(url, mxCreateEndpoint):
				order = append(order, "create")
			}
		}
		if c.wantClick == 1 && strings.Join(order, ">") != "click>create" {
			t.Errorf("%s：应先打卡再上传，实得 %v", c.name, order)
		}
		if res.Counts.Uploaded != 1 || len(res.CloudFileIDs) != 1 {
			t.Errorf("%s：这一条仍该记账，实得 %+v / %v", c.name, res.Counts, res.CloudFileIDs)
		}
		if c.step == 4 {
			// 打卡被挡 ⇒ 类别是"按规则跳过"，但文件**真的传了** ⇒ 别念成"什么都没做"。
			if res.Steps[0].Kind != StepBlockedByRule {
				t.Errorf("门挡住时类别应是按规则跳过，实得 %+v", res.Steps[0])
			}
			if res.Counts.RuleSkipped != 1 {
				t.Errorf("仍该按规则计一条，实得 %+v", res.Counts)
			}
			// ⭐ 这一条今天动过（传了三枪）⇒ 复查落"做了没认"，不是"仍待办"。
			if res.Counts.NotAcknowledged != 1 {
				t.Errorf("只传了文件也算动过，实得 %+v", res.Counts)
			}
		}
	}
}

// TestTaskMatrixRunUploadOutcomesThatWriteNothing：`exist` 与"占位了但没传完"两档。
//
// 两档都**不该**被念成"传成功了"，但对**清理**的要求相反：
//   - `exist` ⇒ 网盘里那一个不是我们造的 ⇒ 不交 id（交了就是删用户自己的东西）；
//   - create 占下了坑却没传完（响应缺 uploadUrl、或 PUT 那一枪挂了）⇒ 必须交 id，
//     否则它就是用户网盘里那个删不掉的孤儿。
func TestTaskMatrixRunUploadOutcomesThatWriteNothing(t *testing.T) {
	t.Run("上游说已存在", func(t *testing.T) {
		first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
		script := mxScript([]string{"JWT-1"}, first, first, append(
			// 夹具**故意带上一个 fileId**：上游说"已存在"时把已存在那一个的 id 也交回来，
			// 是最可能顺手记进清理名单的一种形状（删掉的就是用户自己的文件）。
			[]liveTaskRoute{{mxCreateEndpoint,
				[]string{`{"code":"0000","success":true,"data":{"exist":true,"rapidUpload":false,` +
					`"fileId":"SYNTH-PREEXISTING"}}`}}},
			append(mxUploadRoutes(1), mxHappy()...)...)...)
		res, perr := mxResult(t, mxDefaultRun(t, script))
		if perr != nil {
			t.Fatalf("跑一趟失败: %v", perr)
		}
		if creates, puts, completes := mxUploadCounts(script); creates != 1 || puts+completes != 0 {
			// creates==1 是"这一趟真的走到了上传"的凭据。只钉后两枪时，
			// "前置那一发被 404 挡住 ⇒ 整条玩法没跑"也会让它偶然成立。
			t.Errorf("已存在 ⇒ 只该发 create，实得 %d/%d/%d", creates, puts, completes)
		}
		if len(res.CloudFileIDs) != 0 {
			t.Errorf("那一个不是我们造的，不该进清理名单，实得 %v", res.CloudFileIDs)
		}
		if res.Counts.Uploaded != 0 {
			t.Errorf("没传成就不该计数，实得 %+v", res.Counts)
		}
		if got := res.Steps[0].Err; !strings.Contains(got, "已存在") {
			t.Errorf("要说清「为什么一个都没传」，实得 %q", got)
		}
	})
	t.Run("占位了但没传完（create 不给 uploadUrl）", func(t *testing.T) {
		first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
		script := mxScript([]string{"JWT-1"}, first, first, append(
			// create 成功但**不给 uploadUrl** ⇒ 编排层判失败（零实拍最可能撞的那一格）。
			[]liveTaskRoute{{mxCreateEndpoint,
				[]string{`{"code":"0000","success":true,"data":{"fileId":"SYNTH-ORPHAN"}}`}}},
			append(mxUploadRoutes(1), mxHappy()...)...)...)
		res, perr := mxResult(t, mxDefaultRun(t, script))
		if perr != nil {
			t.Fatalf("跑一趟失败: %v", perr)
		}
		if creates, puts, completes := mxUploadCounts(script); creates != 1 || puts+completes != 0 {
			t.Errorf("缺 uploadUrl ⇒ 占位之后就不该再发后两枪，实得 %d/%d/%d",
				creates, puts, completes)
		}
		if len(res.CloudFileIDs) != 1 || res.CloudFileIDs[0] != "SYNTH-ORPHAN" {
			t.Errorf("占过的坑必须交回去清理，实得 %v", res.CloudFileIDs)
		}
		if res.Counts.Uploaded != 0 {
			t.Errorf("没传完不算「新增文件」，实得 %+v", res.Counts)
		}
		if got := res.SummaryLine(); !strings.Contains(got, "另有 1 个占位未传完") {
			t.Errorf("这种「有 id 但没计数」的差值要单独说出来，实得 %q", got)
		}
	})
}

// TestTaskMatrixRunUploadNeedsDeviceID：换不到 deviceId ⇒ 三枪一枪都不发。
//
// 这一族每一枪都带 `deviceid` 头（旧实现是主 http 实例的静态头），而"少这一上游
// 理不理"零样本 ⇒ 不赌，与入口上报那一格同一处置（记一条失败、继续跑别的任务）。
func TestTaskMatrixRunUploadNeedsDeviceID(t *testing.T) {
	first := mxItems(
		mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(777, "day", "看个广告", "WAIT", "", 0),
	)
	script := mxScript([]string{"JWT-1"}, first, first,
		append(mxUploadRoutes(1), mxHappy()...)...)
	run := mxRun(t, mxStoreWithoutDeviceID(), script, NewSessionPool(), testClock(),
		&recordingWaiter{})
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("换不到 deviceId 不该让整趟失败: %v", perr)
	}
	if creates, puts, completes := mxUploadCounts(script); creates+puts+completes != 0 {
		t.Errorf("换不到 id ⇒ 一枪都不发，实得 %d/%d/%d", creates, puts, completes)
	}
	if len(res.CloudFileIDs) != 0 {
		t.Errorf("没发枪就不该有 id，实得 %v", res.CloudFileIDs)
	}
	if got := res.Steps[0].Err; !strings.Contains(got, "deviceId") {
		t.Errorf("要写明卡在 deviceId，实得 %q", got)
	}
	// 另一条不该被带崩。
	if got := res.Steps[1]; got.TaskID != 777 || got.Err != "" {
		t.Errorf("777 的盲点不该被 106 带崩，实得 %+v", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 清理那一半（#37：链尾那一发 `batchTrash`）
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskMatrixRunTrashesUploadedFiles ⭐ 主路径：传了几个就一次删几个。
//
// 钉四件事，各自"改了会怎样"：
//   - **恰好一发**（不是逐条删）：旧实现 `after.ts:135` 把整份名单一次交给
//     `deleteFiles`，活体日志里就是一行 id 逗号相连 + 一句"删除文件成功"。
//     改成逐条删的表现是多 N-1 发请求，界面上看不出差别 ⇒ 只能在这里数。
//   - **body 里是这一趟那批 id**：少一个就是"网盘里留了东西"。
//   - **id 那一行排在分节之后、结局之前**（清理失败时它是唯一能照着手动删的信息）。
//   - 小结里那句括号跟着结局走（"已移进回收站"而不是"已删除"：`batchTrash` 的语义
//     是进回收站，说"已删除"会让人不去回收站看那一份占位）。
func TestTaskMatrixRunTrashesUploadedFiles(t *testing.T) {
	first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0),
		mxItem(522, "month", "当月上传文件满100个", "WAIT", "", 0))
	second := mxItems(mxItem(106, "day", "上传一个文件", "FINISH", "", 1),
		mxItem(522, "month", "当月上传文件满100个", "FINISH", "", 1))
	script := mxScript([]string{"JWT-1"}, first, second,
		append(mxUploadRoutes(1+taskMatrixUploadRoundsPerRun), mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	rounds := 1 + taskMatrixUploadRoundsPerRun
	if got := mxTrashCount(script); got != 1 {
		t.Fatalf("清理应恰好一发（一次删多个），实得 %d 次", got)
	}
	if res.Cleanup != TaskCleanupDone {
		t.Errorf("结局应是「删成了」，实得 %v / %q", res.Cleanup, res.CleanupReason)
	}
	if len(res.CloudFileIDs) != rounds {
		t.Fatalf("该记 %d 个 id，实得 %v", rounds, res.CloudFileIDs)
	}
	body := mxTrashBodies(script)[0]
	for _, id := range res.CloudFileIDs {
		if !strings.Contains(body, `"`+id+`"`) {
			t.Errorf("清理名单里少了 %s，实得 %s", id, body)
		}
	}
	if n := strings.Count(body, "SYNTH-FID-"); n != rounds {
		t.Errorf("名单应是 %d 枚 id 一次交出去，实得 %d 枚：%s", rounds, n, body)
	}
	// 播报那三行的**顺序**（计数对、顺序错也一样是缺陷：失败时人要靠 id 那一行手动删）。
	section := log.indexOf("------【清理临时文件】------")
	ids := log.indexOf("删除文件 SYNTH-FID-")
	done := log.indexOf("删除文件成功")
	if section < 0 || ids < 0 || done < 0 {
		t.Fatalf("清理那一节三行都该在，实得：\n%s", log.joined())
	}
	if !(section < ids && ids < done) {
		t.Errorf("顺序应为 分节<id行<结局，实得 %d/%d/%d：\n%s",
			section, ids, done, log.joined())
	}
	if summary := log.indexOf("任务表 "); summary < 0 || summary < done {
		t.Errorf("小结必须排在清理之后（它要带清理的结局），实得：\n%s", log.joined())
	}
	// ⚠ 位置的另一半：清理必须排在**复查那些行之后**（旧实现那一节属于 `afterTask`，
	// 整条链跑完才做）。挪到前面的表现是"还没复查就先删了"，而复查读的就是网盘那些文件
	// 对应的任务状态 —— 顺序错会让人以为删掉的东西是"复查之后又多出来的"。
	if verify := log.indexOf("------【每日任务】------"); verify < 0 || section < verify {
		t.Errorf("清理那节应排在复查分节之后，实得 verify=%d section=%d：\n%s",
			verify, section, log.joined())
	}
	if !log.hasAt(core.LevelInfo, "删除文件成功") {
		t.Errorf("成功那一行应是 info，实得：\n%s", log.joined())
	}
	if got := res.SummaryLine(); !strings.Contains(got, fmt.Sprintf("网盘新增 %d 个文件（已移进回收站）", rounds)) {
		t.Errorf("小结要按结局改写那一句，实得 %q", got)
	}
}

// TestTaskMatrixRunKeepsUploadedFilesWhenAsked 钉"刻意不删"那一档。
//
// 它必须**一枪不发**（不是"发了但当成没发"），并且那一行落 warn —— 这一档的读法是
// "这些文件还在你的网盘里，是设置让它留的"，与"想删没删掉"（下一条，error）分开。
func TestTaskMatrixRunKeepsUploadedFilesWhenAsked(t *testing.T) {
	first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, first,
		append(mxUploadRoutes(1), mxHappy()...)...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	run.KeepUploadedFiles = true
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if got := mxTrashCount(script); got != 0 {
		t.Errorf("设了保留就该一枪不发，实得 %d 次", got)
	}
	if res.Cleanup != TaskCleanupKept {
		t.Errorf("结局应是「刻意留着」，实得 %v", res.Cleanup)
	}
	if !log.hasAt(core.LevelWarn, "未清理 1 个") {
		t.Errorf("要播一行 warn 说明没删，实得：\n%s", log.joined())
	}
	if got := res.SummaryLine(); !strings.Contains(got, "（本次保留，未清理）") {
		t.Errorf("小结要写清是「保留」不是「删了」，实得 %q", got)
	}
}

// TestTaskMatrixRunReportsCleanupFailure 钉"发了那一枪而没成"那一档。
//
// 这一档最要紧的不是级别，而是**id 那一行仍在**：清理没成 ⇒ 那些文件还挂在用户网盘里，
// 人必须能照着日志手动删。旧实现失败时只打一句 error（连 id 都看不到），这里不照抄。
func TestTaskMatrixRunReportsCleanupFailure(t *testing.T) {
	first := mxItems(mxItem(106, "day", "上传一个文件", "WAIT", "", 0))
	routes := append(mxUploadRoutes(1),
		mxTrashRoute(`{"code":"0000","message":"合成拒绝","success":false,"result":{}}`))
	// ⚠ mxTrashRoute 那一条要排在 mxHappy **之前**：夹具取第一条匹配的路由。
	routes = append(routes, mxHappy()...)
	script := mxScript([]string{"JWT-1"}, first, first, routes...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("清理没成不该让整趟报错: %v", perr)
	}
	if got := mxTrashCount(script); got != 1 {
		t.Fatalf("仍该发那一发，实得 %d 次", got)
	}
	if res.Cleanup != TaskCleanupFailed {
		t.Errorf("结局应是「没删掉」，实得 %v / %q", res.Cleanup, res.CleanupReason)
	}
	if len(res.CloudFileIDs) != 1 {
		t.Errorf("没删成更要留着 id，实得 %v", res.CloudFileIDs)
	}
	if !log.hasAt(core.LevelError, "删除文件失败") {
		t.Errorf("失败那一行应是 error，实得：\n%s", log.joined())
	}
	if !log.has("删除文件 SYNTH-FID-1") {
		t.Errorf("失败时 id 那一行必须在（那是手动删的唯一线索），实得：\n%s", log.joined())
	}
	if got := res.SummaryLine(); !strings.Contains(got, "（清理失败：") {
		t.Errorf("小结要带失败原因，实得 %q", got)
	}
}

// TestTaskMatrixRunWithoutUploadsHasNoCleanupSection 反向：没写过网盘就不许播"清理"那一节。
//
// 旧实现那一节属于 `afterTask`（无论有没有文件都会打一句"无临时文件需要清理"），
// 这里刻意不照抄：这一节出现在 app-task 链内，一个文件都没写过还播一节"清理"，
// 读日志的人会以为这一趟写过东西然后删了。
func TestTaskMatrixRunWithoutUploadsHasNoCleanupSection(t *testing.T) {
	first := mxItems(mxItem(777, "day", "看个广告", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, first, first, mxHappy()...)
	log := &mxLog{}
	run := mxDefaultRun(t, script)
	run.Progress = log.handle()
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if got := mxTrashCount(script); got != 0 {
		t.Errorf("没有文件就不该发清理那一枪，实得 %d 次", got)
	}
	if log.has("清理临时文件") {
		t.Errorf("没写过网盘不该播那一节，实得：\n%s", log.joined())
	}
	if res.Cleanup != TaskCleanupNotRun {
		t.Errorf("结局应是「没有要清理的东西」，实得 %v", res.Cleanup)
	}
	if got := res.SummaryLine(); strings.Contains(got, "网盘新增") {
		t.Errorf("没写网盘就不该有那一句，实得 %q", got)
	}
}
