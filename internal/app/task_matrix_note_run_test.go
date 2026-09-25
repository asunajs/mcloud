package app

// ─────────────────────────────────────────────────────────────────────────────
// 云笔记那一半（107：换票 → 建一条随机笔记 → 移进回收站）
// ─────────────────────────────────────────────────────────────────────────────
//
// 这一族的形状（三枪各发几次、失败时怎么说）在 `task_matrix_run.go` 的 [runNote]；
// 三口请求与响应判定在 `internal/upstream/app_note.go`（那里有 20 来条形状用例）。
// 这一节只判**编排层**：什么时候不发、失败时说了什么、错误文本里带不带凭据。
//
// ⭐ 这一节存在的另一半理由，是盘点里那句先决断言（`docs/business-feature-inventory.md:383`
// "重写时先补一条'107 能被选中'的断言再动逻辑"）—— 旧 TS 那条 runner 因为读错了返回值
// 形状而**恒跳过**，一枪都没发过。所以 [TestPlanTaskStepSelectsNoteTask] 排在最前：
// 它钉的是"分派表把 107 认成一条能跑的玩法"，与网络无关，也是那个 bug 在这仓里的**反例**。
//
// ⚠ 夹具**全是合成**的：这一族三口在本仓所有归档里零实拍（成文见 `app_note.go` 文件头）。
// 所以这些用例钉的是"编排层怎么处置每一种读数"，**不是**"上游会这样回"。

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	mxNoteAuthEndpoint    = "/noteServer/api/authTokenRefresh.do"
	mxNoteCreateEndpoint  = "/noteServer/api/createNote.do"
	mxNoteRecycleEndpoint = "/noteServer/api/moveToRecycleBin.do"
	// mxNoteAppAuth 进夹具也进断言：它是"往下游透传了哪一枚"的唯一线索。
	mxNoteAppAuth = "SYNTH-APP-AUTH"
	mxNoteOKBody  = `{"code":"0","message":"成功","success":true}`
	mxNoteReject  = `{"code":"9999","message":"合成拒绝","success":false}`
)

// noteHeaderClient 给鉴权那一发的响应**补上 `app_auth` 头**。
//
// 为什么要包一层：[liveTaskScript] 只会回 `core.NewResponse(200, body)`，而这一族的
// 判据恰恰在**响应头**上（两家参考实现都不看体）。不补这一枚，所有用例都会停在
// "换票失败"那一档，后两枪的形状就永远测不到。
// `appAuth` 传空串 ⇒ **不给**那一枚（演"上游没给凭据"那一档）。
type noteHeaderClient struct {
	script  *liveTaskScript
	appAuth string
	// normalize 决定要不要把上游那枚头**故意写成别的拼法**（钉大小写不敏感那一条
	// 在编排层也成立：Go 会把它规范化成 `App_auth`）。
	normalize string
}

func (c noteHeaderClient) Send(
	ctx context.Context, request core.HTTPRequest,
) (core.HTTPResponse, error) {
	response, err := c.script.Send(ctx, request)
	if err != nil || !strings.Contains(request.URL, mxNoteAuthEndpoint) {
		return response, err
	}
	name := c.normalize
	if name == "" {
		name = upstream.NoteAppAuthHeader
	}
	if c.appAuth != "" {
		response.Headers = append(response.Headers, core.HeaderPair{name, c.appAuth})
	}
	return response, nil
}

// noteIDOf 从 `createNote` 的请求体里取出那一枚 noteid。
//
// 为什么在测试里再解析一次，而不是直接断言体里含某个常量：noteid 由随机源造
// （[randomHex]），编排层不许把它的值钉成字面量 —— 要钉的是"两枪用的是**同一个**值"。
func noteIDOf(t *testing.T, body string) string {
	t.Helper()
	_, rest, ok := strings.Cut(body, `"noteid":"`)
	if !ok {
		t.Fatalf("createNote 的体里没有顶层 noteid 那一格：%q", body)
	}
	id, _, ok := strings.Cut(rest, `"`)
	if !ok || id == "" {
		t.Fatalf("noteid 取值不成形：%q", body)
	}
	return id
}

// noteFailingWaiter 是"等不住"那份 waiter：记下载距，但**每次都返回错误**。
//
// 为什么用它而不是 `Wait: nil`：nil 会走 [RealWaiter] ⇒ 每条用例真睡 2 秒
// （那一族唯一的两个消费者都用它）。"没注入 waiter 时仍会等"这一格由 [wait] 那一句
// 保证，形状本身在 `login_test.go` 已钉；这里要钉的是**等不住之后怎么办**。
type noteFailingWaiter struct{ durations []time.Duration }

func (w *noteFailingWaiter) Wait(_ context.Context, d time.Duration) *core.Error {
	w.durations = append(w.durations, d)
	return core.NewInternal("合成：等不下去")
}

// noteRun 装配一条链（与 [mxDefaultRun] 同一份绑定，只是客户端换成会回响应头的那一层）。
func noteRun(
	t *testing.T, script *liveTaskScript, appAuth string, wait Waiter, progress RunProgress,
) TaskMatrixRun {
	t.Helper()
	return TaskMatrixRun{
		SignInWorld: SignInWorld{
			Store: mxStoreBound(),
			Scope: core.ScopeLocal(),
			Upstream: upstream.NewUpstream(noteHeaderClient{
				script: script, appAuth: appAuth,
			}),
			Device: upstream.NewIPadAppDevice(
				"67F4C56A00000000000000000000315C", "8A708867DBC3"),
			Clock:    testClock(),
			Random:   testRng(),
			Sessions: NewSessionPool(),
			Progress: progress,
		},
		Wait: wait,
	}
}

func noteCount(script *liveTaskScript, endpoint string) int { return script.count(endpoint) }

func noteBodies(script *liveTaskScript, endpoint string) []string {
	var out []string
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, endpoint) {
			out = append(out, string(request.Body))
		}
	}
	return out
}

func noteRequest(script *liveTaskScript, endpoint string) (core.HTTPRequest, bool) {
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, endpoint) {
			return request, true
		}
	}
	return core.HTTPRequest{}, false
}

// TestPlanTaskStepSelectsNoteTask ⭐ "107 能被选中"（盘点写明的先决断言）。
//
// 反向改坏它：把 107 从 runner 表挪回缺口表 ⇒ 这一条红，而红出来的正是旧实现那个
// "恒跳过"（它连打卡都不发，只在结论里少一条）。
func TestPlanTaskStepSelectsNoteTask(t *testing.T) {
	item := dispatchItem(107, "day", "创建云笔记", "WAIT", 0)
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepRunner {
		t.Fatalf("107 该走专属玩法，实得 %v（%s）", plan.Kind, plan.Reason)
	}
	if plan.Runner == "" {
		t.Error("结论要能说出是哪一条玩法做的，实得空")
	}
	if plan.ClickKey != taskClickKeyDefault {
		t.Errorf("前置那一发仍按通用门走（key=task），实得 %q", plan.ClickKey)
	}
	// 它**不**上传、**不**分享 —— 三格互斥，别"顺手"给它挂一份上传形状。
	spec, ok := taskMatrixRunners[107]
	if !ok {
		t.Fatal("107 不在 runner 表里")
	}
	if !spec.Notes || spec.Upload != nil || spec.Shares || spec.FinalKey != "" {
		t.Errorf("107 的形状只该有 Notes 那一格，实得 %+v", spec)
	}
	if _, gap := taskMatrixToolGaps[107]; gap {
		t.Error("接上后没从缺口表删掉 ⇒ 界面上会永远显示'没接玩法'")
	}
}

// TestTaskMatrixRunNoteBuildsThenRecycles ⭐ 主路径：换票 → 建 → 等 2s → 移进回收站。
func TestTaskMatrixRunNoteBuildsThenRecycles(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxNoteAuthEndpoint, []string{`{"code":"0"}`}},
			{mxNoteCreateEndpoint, []string{mxNoteOKBody}},
			{mxNoteRecycleEndpoint, []string{mxNoteOKBody}},
		}, mxHappy()...)...)
	wait := &recordingWaiter{}
	log := &mxLog{}
	run := noteRun(t, script, mxNoteAppAuth, wait, log.handle())
	res, perr := mxResult(t, run)
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	for endpoint, want := range map[string]int{
		mxNoteAuthEndpoint: 1, mxNoteCreateEndpoint: 1, mxNoteRecycleEndpoint: 1,
	} {
		if got := noteCount(script, endpoint); got != want {
			t.Errorf("%s 应发 %d 次，实得 %d", endpoint, want, got)
		}
	}
	// 时序：换票在建之前（没票就没法建），建在回收站之前。
	var order []string
	for _, url := range mxURLs(script) {
		switch {
		case strings.Contains(url, mxNoteAuthEndpoint):
			order = append(order, "auth")
		case strings.Contains(url, mxNoteCreateEndpoint):
			order = append(order, "create")
		case strings.Contains(url, mxNoteRecycleEndpoint):
			order = append(order, "recycle")
		}
	}
	if got := strings.Join(order, ">"); got != "auth>create>recycle" {
		t.Errorf("三口时序实得 %s", got)
	}
	// ⭐ 往下游只透传 `app_auth` **一枚**：这是本层与旧 TS 那句 `...headers` 唯一的形状
	// 差别（而旧 TS 从没跑到过那一行 ⇒ 全量透传没有任何实现验证过）。
	create, has := noteRequest(script, mxNoteCreateEndpoint)
	if !has {
		t.Fatal("没看到 createNote 那一枪")
	}
	if v := headerValue(create, upstream.NoteAppAuthHeader); v != mxNoteAppAuth {
		t.Errorf("createNote 要带换到的那枚 app_auth，实得 %q", v)
	}
	recycle, _ := noteRequest(script, mxNoteRecycleEndpoint)
	if v := headerValue(recycle, upstream.NoteAppAuthHeader); v != mxNoteAppAuth {
		t.Errorf("moveToRecycleBin 也要带它，实得 %q", v)
	}
	for _, request := range []core.HTTPRequest{create, recycle} {
		for _, leak := range []string{"set-cookie", "content-length", "transfer-encoding"} {
			if headerValue(request, leak) != "" {
				t.Errorf("把传输层头 %q 透发给上游了（没有任何实现这样做过）", leak)
			}
		}
	}
	// 两口头集合不对称（回收站那一口不带 `APP_NUMBER`）在协议层已钉；这里钉编排层
	// 没有"顺手补齐"：整串 auth 两枪都带，手机号只在建那一枪的体里。
	if headerValue(create, "APP_NUMBER") == "" {
		t.Error("createNote 该带 APP_NUMBER")
	}
	if headerValue(recycle, "APP_NUMBER") != "" {
		t.Error("moveToRecycleBin 不该带 APP_NUMBER（两家一致，补齐等于改一次接口）")
	}
	// 建与删用的是**同一个** noteid：删错一条比不删更糟。
	built := noteBodies(script, mxNoteCreateEndpoint)
	trashed := noteBodies(script, mxNoteRecycleEndpoint)
	if len(built) != 1 || len(trashed) != 1 {
		t.Fatalf("两枪各一发，实得 %d / %d", len(built), len(trashed))
	}
	id := noteIDOf(t, built[0])
	if !strings.Contains(trashed[0], id) {
		t.Errorf("回收站那一枪删的应是刚建的那一条（%s），实得 %q", id, trashed[0])
	}
	// 节奏：等的是**那 2 秒**（不是通用的 500ms）。
	var sawNoteWait bool
	for _, d := range wait.durations {
		if d == noteRecycleWait {
			sawNoteWait = true
		}
	}
	if !sawNoteWait {
		t.Errorf("建完该等 %v，实得 %v", noteRecycleWait, wait.durations)
	}
	if n := mxCountPaced(wait); n != 1 {
		t.Errorf("runner 末尾那一拍仍该有（500ms 一次），实得 %d：%v", n, wait.durations)
	}
	// 播报：建与删**各一行**，且删那一行不许念成"彻底删除"。
	if !log.has("云笔记已建 创建云笔记 107") {
		t.Errorf("少了「已建」那一行，实得：\n%s", log.joined())
	}
	if !log.has("云笔记已移进回收站 创建云笔记 107") {
		t.Errorf("少了「已移进回收站」那一行，实得：\n%s", log.joined())
	}
	if strings.Contains(log.joined(), "彻底删除") || strings.Contains(log.joined(), "已删除") {
		t.Errorf("它只进了回收站，不许念成删干净了：\n%s", log.joined())
	}
	if res.Counts.Ran != 1 || res.Counts.Failed != 0 {
		t.Errorf("应记 1 条玩法、0 条失败，实得 %+v", res.Counts)
	}
	// ⚠ 笔记**不进**网盘清理链：那是另一族（R32）。
	if len(res.CloudFileIDs) != 0 {
		t.Errorf("笔记 id 不该混进网盘文件清理名单，实得 %v", res.CloudFileIDs)
	}
	if n := script.count(mxTrashEndpoint); n != 0 {
		t.Errorf("这一趟没传过文件 ⇒ 不该发 batchTrash，实得 %d", n)
	}
}

// TestTaskMatrixRunNoteSendsNothingWhenAuthMissing ⭐ 拿不到 `app_auth` ⇒ 后两枪一枪不发。
//
// 与旧 TS 同向（它在这一档 return），但旧实现是 `logger.info` 一句就完事、那条任务
// **照样**记进它的"成功"名单。这里记失败：这一条**没做成**。
func TestTaskMatrixRunNoteSendsNothingWhenAuthMissing(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{{mxNoteAuthEndpoint, []string{mxNoteReject}}}, mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, noteRun(t, script, "", &recordingWaiter{}, log.handle()))
	if perr != nil {
		t.Fatalf("单条失败不该让整趟报错: %v", perr)
	}
	if n := noteCount(script, mxNoteCreateEndpoint); n != 0 {
		t.Errorf("没拿到凭据却建笔记，实得 %d 次", n)
	}
	if n := noteCount(script, mxNoteRecycleEndpoint); n != 0 {
		t.Errorf("更不该发删除，实得 %d 次", n)
	}
	if n := noteCount(script, mxNoteAuthEndpoint); n != 1 {
		t.Errorf("换票那一发仍该发（它是唯一能知道上游给不给的手段），实得 %d", n)
	}
	if text := res.Steps[0].Err; !strings.Contains(text, "换笔记鉴权失败") {
		t.Errorf("要记一条失败原因，否则界面上是「跑了玩法、什么都没做」，实得 %q", text)
	}
	if !strings.Contains(res.Steps[0].Err, "9999") {
		t.Errorf("上游说了话就要带进原因（两家在这里都只报一句空话），实得 %q", res.Steps[0].Err)
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落在失败，实得 %+v", res.Counts)
	}
	if !log.hasAt(core.LevelError, "云笔记失败") {
		t.Errorf("这一档必须是 error，不能像旧实现那样 info 一句，实得：\n%s", log.joined())
	}
}

// TestTaskMatrixRunNoteReportsCreateFailureWithoutDeleting 建失败 ⇒ 不再发删除那一枪。
//
// 旧 TS 在这里是静默的（`tryRequest` 把返回值整个丢弃 ⇒ 建失败也照删）。
// 这里停在"没做成"，并在原因里说清"不会留下东西"这一格**是**有条件的：
// 上游可能其实建成功了却没说 ⇒ 那句不敢写成"一定没留"。
func TestTaskMatrixRunNoteReportsCreateFailureWithoutDeleting(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxNoteAuthEndpoint, []string{`{"code":"0"}`}},
			{mxNoteCreateEndpoint, []string{mxNoteReject}},
			{mxNoteRecycleEndpoint, []string{mxNoteOKBody}},
		}, mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, noteRun(t, script, mxNoteAppAuth, &recordingWaiter{}, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := noteCount(script, mxNoteRecycleEndpoint); n != 0 {
		t.Errorf("建失败就不该发删除（拿一个没建成的 id 去删是凭空一发），实得 %d", n)
	}
	if text := res.Steps[0].Err; !strings.Contains(text, "建笔记失败") {
		t.Errorf("要报出建失败，实得 %q", text)
	}
	if log.has("云笔记已建") {
		t.Errorf("没建成却播「已建」是谎报，实得：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落在失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunNoteSaysLeftoverWhenRecycleFails ⭐ 删失败 ⇒ 必须说"东西留在账号里"。
//
// 这一档的不对称：建成功 + 删失败 = 用户的云笔记里多一条随机标题的笔记，而没有人知道。
// 旧实现在这里是完全静默的。这里三样都要有：报失败、播 error、给出路。
func TestTaskMatrixRunNoteSaysLeftoverWhenRecycleFails(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxNoteAuthEndpoint, []string{`{"code":"0"}`}},
			{mxNoteCreateEndpoint, []string{mxNoteOKBody}},
			{mxNoteRecycleEndpoint, []string{mxNoteReject}},
		}, mxHappy()...)...)
	log := &mxLog{}
	// ⚠ 这一条**会**走到那一拍（建成功了）⇒ 必须注入 waiter，不然测试真睡 2 秒。
	res, perr := mxResult(t, noteRun(t, script, mxNoteAppAuth, &recordingWaiter{}, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := noteCount(script, mxNoteRecycleEndpoint); n != 1 {
		t.Errorf("删失败也不该在同一趟补第二发（口径同 [runShare]），实得 %d", n)
	}
	text := res.Steps[0].Err
	if !strings.Contains(text, "移进回收站失败") {
		t.Errorf("要报出删除失败，实得 %q", text)
	}
	if !strings.Contains(text, "留在") {
		t.Errorf("结论里要给出路（那条笔记还在），实得 %q", text)
	}
	if !log.has("云笔记已建") {
		t.Errorf("建是**真的**成功了，那一行该在，实得：\n%s", log.joined())
	}
	if log.has("云笔记已移进回收站") {
		t.Errorf("删失败却播「已移进回收站」是谎报，实得：\n%s", log.joined())
	}
	if res.Counts.Failed != 1 {
		t.Errorf("应落在失败，实得 %+v", res.Counts)
	}
}

// TestTaskMatrixRunNoteRecyclesAnywayWhenWaitFails ⭐ 等不住 ⇒ **照发**删除那一枪。
//
// 这一档的取舍写成断言：跳过等待的代价是"上游可能还没落库、那一枪删不掉"，
// 而不发那一枪的代价是"一定留一条笔记在用户账号里"。前者可重试、后者不可见 ⇒
// 代码选了照发，并播一行 warn 把"没等够"这件事说出来。
// 反向改坏（把等不住当成"整条不做"）的表现：删除那一发计数为 0、没有 warn 行。
func TestTaskMatrixRunNoteRecyclesAnywayWhenWaitFails(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxNoteAuthEndpoint, []string{`{"code":"0"}`}},
			{mxNoteCreateEndpoint, []string{mxNoteOKBody}},
			{mxNoteRecycleEndpoint, []string{mxNoteOKBody}},
		}, mxHappy()...)...)
	wait := &noteFailingWaiter{}
	log := &mxLog{}
	res, perr := mxResult(t, noteRun(t, script, mxNoteAppAuth, wait, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := noteCount(script, mxNoteRecycleEndpoint); n != 1 {
		t.Errorf("等不住也要发删除那一枪（否则一定留一条笔记），实得 %d", n)
	}
	if !log.hasAt(core.LevelWarn, "云笔记没等够 2 秒") {
		t.Errorf("要播一行 warn 说明「没等够」，实得：\n%s", log.joined())
	}
	if !log.has("云笔记已移进回收站") {
		t.Errorf("删成了就该有那一行，实得：\n%s", log.joined())
	}
	// 等不住**不算**这一条失败：三枪都发完了，上游也认了。
	if res.Counts.Failed != 0 {
		t.Errorf("等不住只是少一点保险，不该落在失败，实得 %+v", res.Counts)
	}
	// 它等的仍是那一族自己的 2 秒（不是被顺手改成通用拍）。
	var sawNoteWait bool
	for _, d := range wait.durations {
		if d == noteRecycleWait {
			sawNoteWait = true
		}
	}
	if !sawNoteWait {
		t.Errorf("等的是 %v，实得 %v", noteRecycleWait, wait.durations)
	}
}

// TestTaskMatrixRunNoteFailureTextCarriesNoCredential ⭐ 凭据边界。
//
// 这一族把三样都发了出去：整串 `Authorization`、`authToken`（体里）、`app_auth`（头里），
// 外加手机号。`app_auth` 与 `authToken` 同样是会话凭据 ⇒ 一个都不许进原因文本
// （原因会进事件流、进小结、可能被推送渠道发出去）。
// 反向改坏：把请求体或响应体拼进错误文本 ⇒ 本条红。
func TestTaskMatrixRunNoteFailureTextCarriesNoCredential(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			// 鉴权那一口回**被拒的体**、却照样给 `app_auth` ⇒ 判定仍按"拿到票"走
			// （两家都不看码）；真正挂的是建那一枪 ⇒ 走"要记一条失败"那条岔路。
			{mxNoteAuthEndpoint, []string{mxNoteReject}},
			{mxNoteCreateEndpoint, []string{mxNoteReject}},
			{mxNoteRecycleEndpoint, []string{mxNoteOKBody}},
		}, mxHappy()...)...)
	log := &mxLog{}
	res, perr := mxResult(t, noteRun(t, script, mxNoteAppAuth, &recordingWaiter{}, log.handle()))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	text := log.joined() + "\n" + res.Steps[0].Err + "\n" + res.SummaryLine()
	for _, leak := range []string{mxNoteAppAuth, wholeAuth, "Basic ", "19900000001", "ak|bk|ck"} {
		if strings.Contains(text, leak) {
			t.Errorf("失败文本里出现了 %q：\n%s", leak, text)
		}
	}
}

// TestTaskMatrixRunNoteReadsAppAuthCaseInsensitively 钉头名拼法变了仍要读得到。
//
// 夹具刻意用 `App_Auth`（Go 规范化后的样子）而不是 `app_auth`：换成精确匹配时
// 这一条会红，而红出来的正是本层手里那份 [core.HeaderPair] 列表没有规范化那一层。
func TestTaskMatrixRunNoteReadsAppAuthCaseInsensitively(t *testing.T) {
	waiting := mxItems(mxItem(107, "day", "创建云笔记", "WAIT", "", 0))
	script := mxScript([]string{"JWT-1"}, waiting, nil, append(
		[]liveTaskRoute{
			{mxNoteAuthEndpoint, []string{`{"code":"0"}`}},
			{mxNoteCreateEndpoint, []string{mxNoteOKBody}},
			{mxNoteRecycleEndpoint, []string{mxNoteOKBody}},
		}, mxHappy()...)...)
	res, perr := mxResult(t, TaskMatrixRun{
		SignInWorld: SignInWorld{
			Store: mxStoreBound(), Scope: core.ScopeLocal(),
			Upstream: upstream.NewUpstream(noteHeaderClient{
				script: script, appAuth: mxNoteAppAuth, normalize: "App_Auth"}),
			Device: upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
			Clock:  testClock(), Random: testRng(), Sessions: NewSessionPool(),
		},
		Wait: &recordingWaiter{},
	})
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := noteCount(script, mxNoteCreateEndpoint); n != 1 {
		t.Errorf("头名拼法变了也该读到，createNote 实得 %d 次", n)
	}
	// 读到 ⇒ 整条走完（否则"红成换票失败"会被当成夹具问题，而不是大小写问题）。
	if res.Counts.Failed != 0 || res.Counts.Ran != 1 {
		t.Errorf("这一条该记成「跑成了玩法、没失败」，实得 %+v", res.Counts)
	}
}

// TestNoteRecycleWaitIsNotTheGenericPace 钉那两拍**没被合并**。
//
// 合并成 `taskMatrixPace` 的表现是"删紧跟在建之后"，而这一族两枪之间的 2s 是
// 旧实现与在跑的那家**都有**的一格；两值相等会让上面那条时序断言再也分不出谁是谁。
func TestNoteRecycleWaitIsNotTheGenericPace(t *testing.T) {
	if noteRecycleWait == taskMatrixPace {
		t.Errorf("两拍不该相等（%v vs %v）", noteRecycleWait, taskMatrixPace)
	}
	if noteRecycleWait != 2*time.Second {
		t.Errorf("那一拍应是旧实现那 2 秒，实得 %v", noteRecycleWait)
	}
}
