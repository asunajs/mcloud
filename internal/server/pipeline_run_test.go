package server

// 组合执行（`pipeline_run.go`）的用例。
//
// 判据分三层，这里只钉中间那一层：每条链**自己**跑对没有，在 `internal/app`
// 各自的 `_run_test.go`；清单读写与那枚按钮的 HTTP 形状，在 `pipelines_test.go`。
// 本文件管的是"逐步跑"这一层独有的六件事，它们在前两层都测不出来：
//
//  1. **内置那条默认链的成员与顺序**（[allRunPipeline]）—— 顺序与"哪些不进默认链"
//     是这一轮唯一的产品决定，抄自旧实现 `packages/cli/src/index.ts:127-152`。
//     少一步、多一步、调换两步，界面上都只表现为"跑得跟以前不一样"，没人会报错。
//  2. **每一步真的各自起跑了一次**（走 [Server.launchRun]，与单趟与调度同一条路），
//     两步 ⇒ 两行 `runs`、两个不同 runId。
//  3. **整条链只推一条**（AGENTS 里"多打一次上游"之外另一类代价：用户手机上十七八条
//     通知与"推送坏了"难以区分）。判据是 bark 那一家**恰好**收到 1 次。
//  4. **失败之后继不继续**：默认继续（`StopOnError=false`），勾了才停 ——
//     而且"停"必须真的停（后面的端点一枪都不打），否则那一栏是个摆设。
//  5. **预检在一个请求都没发之前就把清单判死**：第 9 步键名拼错的最坏表现不是
//     "那一步没跑"，而是**前面 8 步已经把真上游打了一遍**（签到、领奖都是写操作）。
//     所以每一条 400 都配一句 `client.sent() == 0`。
//  6. **记账只碰那四列**：一次全跑要几分钟，用整行 `Upsert` 会把这段时间里用户
//     改过的名字与步骤抹掉；而 ok 时 `last_error` 要清空。
//
// ⚠ 这里的上游响应**全是合成夹具**（与 `run_dispatch_signin_test.go` 同源），
// 只用来让每一步走到终局；钉的是**接线**，不是上游行为。
//
// ⚠ 每条用例都换 [distinctRunIDs]：一步一个 runId，两步就是**连着两次**
// [core.NewRunID]，而 [fixedRandom] 的环长会让它们拿到同一个 id ⇒ 第二步被判成
// `duplicate_run_id` 而根本没跑（这不是本层要测的东西，会指错方向）。

import (
	"context"
	"encoding/json"
	"net/http"
	"slices"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// 这三条链是本文件的夹具：两条读得到终局、一条故意不给路由（⇒ 那一步必红）。
// 路径写全，是为了让"某一步打到了别人的端点"这件事在断言文本里一眼看得见。
const (
	pipelinePrizesRoute   = "/ycloud/signin/public/record"
	pipelineSnapshotRoute = "/ycloud/signin/page/infoV3"
	pipelineTaskListRoute = "/ycloud/signin/task/taskListV3"
)

// pipelineHarness 起一台"账号凭据齐全、上游按脚本回话"的服务端。
//
// 只给**一个**账号：两条以上账号会让"这一步跑了几个账号"同时受账号数与
// 那把互斥锁影响，判点就混了（被挡那一档另有 [TestRunPipelineBlockedStepIsPartial]）。
func pipelineHarness(t *testing.T, routes ...probeRoute) (*harness, string, *probeScript) {
	t.Helper()
	h := newHarness(t).initialized(t)
	accountID := seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(), routes...)...)
	h.replaceRunWorld(t, client)
	distinctRunIDs(h)
	return h, accountID, client
}

// pStep 造组合里的一步；body 传空串 = 不给参数。
func pStep(task, body string) core.PipelineStep {
	if body == "" {
		body = "{}"
	}
	return core.PipelineStep{Task: task, Body: json.RawMessage(body)}
}

// pPipeline 造一条测试用的组合（默认串行、失败继续、目标 = 全部启用账号）。
func pPipeline(steps ...core.PipelineStep) core.Pipeline {
	pipeline := core.NewPipeline("job", "夹具组合")
	pipeline.Steps = steps
	return pipeline
}

// runPipelineNow 用当前 scope 同步跑一条组合。
//
// 走 [Server.runPipeline] 而不是 HTTP：路由那一路的账（202 / 400 / 响应形状）在
// `pipelines_test.go`。这里要的是"跑完之后拿结论直接断言"，不必和 goroutine 抢时序。
func runPipelineNow(t *testing.T, h *harness, pipeline core.Pipeline) (PipelineResult, *core.Error) {
	t.Helper()
	return h.server.runPipeline(context.Background(), core.ScopeUser(userIDOf(t, h)), pipeline)
}

// taskNames 把清单摊成 code 序列（比对顺序用）。
func taskNames(pipeline core.Pipeline) []string {
	out := make([]string, 0, len(pipeline.Steps))
	for _, step := range pipeline.Steps {
		out = append(out, step.Task)
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 内置那条默认链：成员与顺序
// ─────────────────────────────────────────────────────────────────────────────

// TestAllRunPipelineIsTheOldCLIDefaultChain 逐字钉住默认链的成员**与顺序**。
//
// ⚠ 名单在这里是**手抄**的（不从 `allRunPipeline()` 反推）：反推的话这条就只剩
// "每步都接上了"那一半有用，而"签到排在翻倍之前"这种顺序决定会整条失去守卫 ——
// 翻倍那条读的正是签到写上去的状态，换序不是"无所谓"，是会把当月名额用错地方。
func TestAllRunPipelineIsTheOldCLIDefaultChain(t *testing.T) {
	pipeline := allRunPipeline()

	if pipeline.ID != PipelineRunAllID {
		t.Errorf("内置全跑的 id 必须是保留值 %q，实得 %q —— 换掉它，界面上那枚按钮与调度目标都会指到别处",
			PipelineRunAllID, pipeline.ID)
	}
	if pipeline.StopOnError {
		t.Errorf(`内置全跑必须"一步失败继续跑后面的"（StopOnError=false）：` +
			"签到坏了不该连累领取。旧实现那串 await 外面是一个包住整条链的 try/catch，" +
			"本仓刻意改成『失败记账、不截断』")
	}

	want := []string{
		capabilities.CodeSign,
		capabilities.CodeRevival,
		capabilities.CodeMultiple,
		capabilities.CodePlayAISpecial,
		capabilities.CodeFunAI,
		capabilities.CodeCloudPhoneRedpack,
		capabilities.CodeMsgPush,
		capabilities.CodeNotice,
		capabilities.CodeBackupGift,
		capabilities.CodeAppTask,
		capabilities.CodePrizes,
		capabilities.CodeReceive,
		capabilities.CodeCloudSnapshot,
		capabilities.CodeAuthRefresh,
	}
	if got := taskNames(pipeline); !slices.Equal(want, got) {
		t.Errorf("默认链的成员或顺序与旧实现（index.ts:127-152）对不上：\n  期望 %v\n  实得 %v", want, got)
	}

	// 排除项单独钉一遍：它们**不在**上面那份名单里这件事，才是这一轮的产品决定。
	// 理由逐条写在 `allRunPipeline` 的抬头注释，这里只复述最短的那一句。
	for _, excluded := range []string{
		capabilities.CodeExchange, // 不可逆扣豆、要人当场选品
		capabilities.CodeLiveRoom, // 旧实现默认关；且每账号要听 60 秒弹幕
		capabilities.CodeAiAvatar, // 旧实现默认关；那条链不清理自己写进网盘的文件
		capabilities.CodeTaskList, // 旧实现没有这一步（本仓的"看任务表"接口）
	} {
		if slices.Contains(taskNames(pipeline), excluded) {
			t.Errorf("%s 不该进默认链：放进『谁都会按的一枚按钮』里，等于把它的代价隐到默认路径上", excluded)
		}
	}

	// 每一步都得"登记过 + 接上了执行"：否则这枚按钮一点就整体 400（预检挡下），
	// 而用户看到的是一句跟他的点击无关的话。
	for i, step := range pipeline.Steps {
		if _, found := capabilities.Get(step.Task); !found {
			t.Errorf("默认链第 %d 步 %s 在能力表里查不到", i+1, step.Task)
		}
		if _, wired := builderFor(step.Task); !wired {
			t.Errorf("默认链第 %d 步 %s 没接上执行：整条链会在预检就被拒掉", i+1, step.Task)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 逐步跑：每步各自起跑、只推一条
// ─────────────────────────────────────────────────────────────────────────────

// TestRunPipelineRunsEachStepOnceAndPushesOnce 两步都跑成 ⇒ 两行 run、一条推送。
func TestRunPipelineRunsEachStepOnceAndPushesOnce(t *testing.T) {
	h, accountID, client := pipelineHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}},
		probeRoute{match: pipelineSnapshotRoute, bodies: []string{signInStatusClaimable}},
	)
	sender := &notifyClient{}
	h.withNotifySender(t, sender)
	// ⚠ `enabled:true` 不能省：总开关默认**关**（`core/notify.go` 那句"默认关闭"），
	// 而 `/api/notify/test` 那一条接口是**绕开**开关的 —— 照它配会得到"配置没生效"的假结论。
	if rec := putNotify(t, h, `{"enabled":true,"channels":{"bark":{"key":"`+barkKey+`"}}}`); rec.Code != http.StatusOK {
		t.Fatalf("开推送应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	result, perr := runPipelineNow(t, h,
		pPipeline(pStep(capabilities.CodePrizes, ""), pStep(capabilities.CodeCloudSnapshot, "")))
	if perr != nil {
		t.Fatalf("这条清单该过预检：%s", perr.Message())
	}
	if len(result.Steps) != 2 {
		t.Fatalf("两步清单该有两条结论，实得 %d：%+v", len(result.Steps), result.Steps)
	}
	if !result.OK() {
		t.Errorf("两步都该跑成，实得 %s（note=%q，各步=%+v）", result.Status, result.Note, result.Steps)
	}
	if result.StoppedAfter != -1 {
		t.Errorf("跑到清单末尾时 StoppedAfter 该是 -1，实得 %d", result.StoppedAfter)
	}
	for i, want := range []string{capabilities.CodePrizes, capabilities.CodeCloudSnapshot} {
		step := result.Steps[i]
		if step.Task != want {
			t.Errorf("第 %d 步该是 %s，实得 %s", i+1, want, step.Task)
		}
		if step.Ran != 1 || step.Blocked != 0 {
			t.Errorf("第 %d 步（%s）该是 1 个账号跑成、0 个被挡，实得 ran=%d blocked=%d",
				i+1, want, step.Ran, step.Blocked)
		}
		if step.Title == "" {
			t.Errorf("第 %d 步没带能力标题：推送与界面那句话会只剩一个 code", i+1)
		}
	}

	// 每一步打**自己**那条链，各一次。
	for _, route := range []string{pipelinePrizesRoute, pipelineSnapshotRoute} {
		if got := client.count(route); got != 1 {
			t.Errorf("组合里 %s 应打 1 次，实得 %d 次（全部 URL：%v）", route, got, urlsOf(client))
		}
	}
	// ⭐ 整条链只推一条：两步各推一条的话这里就是 2（而用户手机上那两条与"推送坏了"
	// 难以区分）。这一条同时是 [suppressNotify] 的守卫 —— 传了 suppress 却没人补，
	// 表现是"配了推送而一次全跑什么也没收到"，比多推几条更难查。
	if hits := sender.hostHits(); hits["bark"] != 1 {
		t.Errorf("整条链该只推 1 条 bark，实得 %d 条（URL：%v）", hits["bark"], sender.urls())
	}
	// ⭐ 两步共用一张票（AGENTS §1.12：票 = 账号 × 页面，而今天这一族全映射到
	// [RecipeSignInPage] 那一份配方）。每步重建的表现是"每跑一次组合多打三枪"。
	if got := client.count("/ycloud/auth-service/auth/tyrzLogin"); got != 1 {
		t.Errorf("两步应共用一张会话票，建会话应 1 次，实得 %d 次（URL：%v）", got, urlsOf(client))
	}

	// 组合不是另一套执行器：每一步都在 `runs` 表留下自己那一行，两个 runId 不同。
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 2 {
		t.Errorf("两步该留 2 行运行记录，实得 %d 行：%v", len(ids), ids)
	}
	first, second := result.Steps[0].Runs, result.Steps[1].Runs
	if len(first) != 1 || len(second) != 1 {
		t.Fatalf("每步该给出 1 个账号↔runId 配对，实得 %v / %v", first, second)
	}
	if first[0]["runId"] == second[0]["runId"] {
		t.Errorf("两步拿到了同一个 runId：%v —— 界面上订阅哪一条都分不清", first[0]["runId"])
	}
	if first[0]["accountId"] != accountID {
		t.Errorf("这一步该跑在 %s 上，实得 %v", accountID, first[0])
	}
	// 小结那句话三格都要有（跑了几步 / 几步失败 / 停在第几步）。
	if line := result.SummaryLine(); !strings.Contains(line, "跑了 2 步、其中 0 步失败") {
		t.Errorf("小结该说清跑了几步、几步失败，实得 %q", line)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 失败之后：默认继续，勾了才停
// ─────────────────────────────────────────────────────────────────────────────

// failedThenOKPipeline 是"第 1 步必红、第 2 步能成"的那份清单：
// `task-list` 的端点**故意不给路由**（脚本客户端未匹配 ⇒ 404，那一步跑到终局就是失败）。
//
// 用它而不是"随便一条会红的链"，是因为这一半判据要**分开**看：第 1 步的端点红、
// 第 2 步的端点绿，才分得出"继续跑了"与"跟着一起红"。
func failedThenOKPipeline() core.Pipeline {
	return pPipeline(pStep(capabilities.CodeTaskList, ""), pStep(capabilities.CodePrizes, ""))
}

// failedThenOKHarness 是配好那一步路由的夹具（第 2 步的端点可打，第 1 步的没编路由）。
func failedThenOKHarness(t *testing.T) (*harness, string, *probeScript) {
	t.Helper()
	return pipelineHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}})
}

// TestRunPipelineContinuesAfterFailureByDefault：一步失败不截断后面的。
func TestRunPipelineContinuesAfterFailureByDefault(t *testing.T) {
	h, _, client := failedThenOKHarness(t)

	result, perr := runPipelineNow(t, h, failedThenOKPipeline())
	if perr != nil {
		t.Fatalf("跑起来了就不该往外交 error：%v", perr)
	}
	if len(result.Steps) != 2 {
		t.Fatalf("第一步失败后第二步该照跑（默认 StopOnError=false），实得 %d 条结论：%+v",
			len(result.Steps), result.Steps)
	}
	if result.Steps[0].Status != core.PipelineError {
		t.Errorf("端点没路由的那一步该记 error，实得 %s", result.Steps[0].Status)
	}
	if result.Steps[0].Note == "" {
		t.Errorf("失败那一步必须带一句原因（组合没有事件流，这句是界面与推送唯一的读数）")
	}
	if result.Steps[1].Status != core.PipelineOK {
		t.Errorf("第二步该独立跑成，实得 %s（note=%q）", result.Steps[1].Status, result.Steps[1].Note)
	}
	if got := client.count(pipelinePrizesRoute); got != 1 {
		t.Errorf("第一步红了，第二步的端点仍该被打 1 次，实得 %d 次（URL：%v）", got, urlsOf(client))
	}
	if result.Status != core.PipelineError {
		t.Errorf("整体结论该是 error（有一步失败），实得 %s", result.Status)
	}
	if result.StoppedAfter != -1 {
		t.Errorf("按设定跑到底时 StoppedAfter 该是 -1，实得 %d", result.StoppedAfter)
	}
	// 红了也推：漏掉收尾那一条，表现是"这次全跑出了事，手机上安静得像跑成功了"。
	if result.Notify == nil {
		t.Errorf("跑完该有一条推送结论，实得 nil")
	}
}

// TestRunPipelineStopOnErrorHaltsTheChain：勾了"遇错就停"才真的停。
//
// ⚠ 这一条与上一条是**一对**：只留上一条的话，把 `ContinuesAfterFailure` 写成恒真
// 或恒假都只会红一边，而"恒真"这一半没人查过 —— 停不下来时界面上看不出区别
// （只是多打了几枪真上游）。
func TestRunPipelineStopOnErrorHaltsTheChain(t *testing.T) {
	h, _, client := failedThenOKHarness(t)
	pipeline := failedThenOKPipeline()
	pipeline.StopOnError = true

	result, perr := runPipelineNow(t, h, pipeline)
	if perr != nil {
		t.Fatalf("清单合法：%v", perr)
	}
	if len(result.Steps) != 1 {
		t.Fatalf("第 1 步失败就该停，实得 %d 条结论：%+v", len(result.Steps), result.Steps)
	}
	if got := client.count(pipelinePrizesRoute); got != 0 {
		t.Errorf("停下来了就不该打第 2 步的端点，实得 %d 次（URL：%v）", got, urlsOf(client))
	}
	if result.StoppedAfter != 0 {
		t.Errorf("StoppedAfter 该是 0（停在第 1 步），实得 %d", result.StoppedAfter)
	}
	// 那一格在这里才是**唯一**能分辨"停在第 1 步"与"跑完了但第 1 步红"的读数，
	// 而处置完全不同：前者要查为什么停了，后者只修那一步。
	if !strings.Contains(result.SummaryLine(), "停在第 1 步") {
		t.Errorf("小结该说出停在第几步，实得 %q", result.SummaryLine())
	}
}

// TestStepContinueOnErrorOverridesThePipeline：步骤级那一栏盖过组合级。
//
// 模型层 [core.Pipeline.ContinuesAfterFailure] 的"三态"（nil 跟随、显式覆盖）
// 在 app 层没有使用者，本层是第一个；写反的表现是"用户在某一步勾了继续，整条链照停"。
func TestStepContinueOnErrorOverridesThePipeline(t *testing.T) {
	h, _, client := failedThenOKHarness(t)
	yes := true
	pipeline := failedThenOKPipeline()
	pipeline.StopOnError = true
	pipeline.Steps[0].ContinueOnError = &yes

	if _, perr := runPipelineNow(t, h, pipeline); perr != nil {
		t.Fatalf("清单合法：%v", perr)
	}
	if got := client.count(pipelinePrizesRoute); got != 1 {
		t.Errorf("第 1 步勾了『失败继续』，第 2 步就该跑（端点应打 1 次），实得 %d 次（URL：%v）",
			got, urlsOf(client))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 被另一趟占住：partial，而不是 error
// ─────────────────────────────────────────────────────────────────────────────

// TestRunPipelineBlockedStepIsPartial 一个账号都没跑成、全被挡 ⇒ partial。
//
// 不判 error 的理由见 [pipelineStepStatus]（"上一轮还在跑"在组合里是常态，
// 判红会让人以为清单坏了）；不判 ok 的理由更硬：那等于把"这一趟什么都没做"藏起来。
//
// ⚠ 占锁用的是**这一步真正会算出来的那把键**（先问 builder 要 spec）：
// 自己拼 `capability#account` 会在参数指纹非空时挡不住任何东西，用例假绿。
func TestRunPipelineBlockedStepIsPartial(t *testing.T) {
	h, accountID, client := pipelineHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}})
	pipeline := pPipeline(pStep(capabilities.CodePrizes, ""))

	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	requests, perr := pipelineRequests(pipeline)
	if perr != nil {
		t.Fatalf("一步合法清单该过预检：%s", perr.Message())
	}
	spec, perr := h.server.pipelineSpecFor(ctx, scope, requests[0])
	if perr != nil {
		t.Fatalf("建这一步的计划失败：%s", perr.Message())
	}
	if len(spec.plan.Accounts) != 1 || spec.plan.Accounts[0].String() != accountID {
		t.Fatalf("前提没成立：这一步的目标账号应是夹具那一个，实得 %v", spec.plan.Accounts)
	}
	holder := core.NewRunID(h.server.rng)
	key := mcloudexec.NewMutexKey(spec.plan.Capability, accountID, spec.plan.ParamsDigest)
	if admission := h.server.runs.registry.Admit(holder, key); admission.Kind != mcloudexec.AdmissionAdmitted {
		t.Fatalf("夹具占锁失败（前提没成立）：%s", admission.Kind)
	}
	defer func() {
		if !h.server.runs.registry.Release(holder) {
			t.Errorf("夹具占的锁没放回去，可能污染后面的用例")
		}
	}()

	result, perr := runPipelineNow(t, h, pipeline)
	if perr != nil {
		t.Fatalf("被挡下不是清单失败：%v", perr)
	}
	if len(result.Steps) != 1 {
		t.Fatalf("该有一条结论，实得 %d", len(result.Steps))
	}
	step := result.Steps[0]
	if step.Status != core.PipelinePartial {
		t.Errorf("全被挡的那一步该记 partial，实得 %s（note=%q）", step.Status, step.Note)
	}
	if !strings.Contains(step.Note, "正被另一趟占着") {
		t.Errorf("那句话该说清是『被占着』而不是别的失败，实得 %q", step.Note)
	}
	if step.Ran != 0 || step.Blocked != 1 {
		t.Errorf("该是 ran=0 blocked=1，实得 ran=%d blocked=%d", step.Ran, step.Blocked)
	}
	if got := client.count(pipelinePrizesRoute); got != 0 {
		t.Errorf("被挡下的一趟不该打任何上游请求，实得 %d 次（URL：%v）", got, urlsOf(client))
	}
	// 整条链都是这一档 ⇒ 调度那一侧记 skipped（不是 error、也不是"跑过了"）。
	if !pipelineAllBlocked(result) {
		t.Errorf("ran=0 且 blocked>0 该判成『整条链都没跑成』")
	}
	if pipelineAllBlocked(PipelineResult{Steps: []PipelineStepOutcome{{Ran: 1, Blocked: 1}}}) {
		t.Errorf("只要有一步真跑了账号就不能记 skipped：那一趟确实往上游打了请求")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 预检：不合格 ⇒ 一枪不发
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelinePrecheckRefusesToSendNothing：六种"这份清单跑不了"全部在动手前挡下。
//
// 判据不是"回什么码"，而是 `client.sent() == 0`：预检写在跑之前，
// 一旦顺序反过来，第 9 步的键名拼错就会让前面 8 步把真上游打一遍 ——
// 签到、领奖都是**不可逆**动作，而这句错误在响应里照样回得出来，HTTP 面看不出错。
func TestPipelinePrecheckRefusesToSendNothing(t *testing.T) {
	cases := []struct {
		name   string
		steps  []core.PipelineStep
		tweak  func(pipeline *core.Pipeline)
		want   string
		unwire string
	}{
		{
			name:  "一步都没有",
			steps: nil,
			want:  "一步都没有",
		},
		{
			name:  "能力表里查不到这一步",
			steps: []core.PipelineStep{pStep("no-such-task", "")},
			want:  "第 1 步没有这个能力：no-such-task",
		},
		{
			name:  "参数里有声明不认识的键",
			steps: []core.PipelineStep{pStep(capabilities.CodeSign, `{"receive":true}`)},
			want:  "第 1 步",
		},
		{
			name:  "某一步的参数不是对象",
			steps: []core.PipelineStep{pStep(capabilities.CodePrizes, `[1,2]`)},
			want:  "不是一个 JSON 对象",
		},
		{
			name:  "执行方式选了并行",
			steps: []core.PipelineStep{pStep(capabilities.CodePrizes, "")},
			tweak: func(pipeline *core.Pipeline) { pipeline.AccountMode = core.ModeParallel },
			want:  "只有串行",
		},
		{
			name:   "能力登记了但没接上执行",
			steps:  []core.PipelineStep{pStep(capabilities.CodePrizes, "")},
			want:   "还没有接上执行",
			unwire: capabilities.CodePrizes,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			h, _, client := pipelineHarness(t,
				probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}})
			if tc.unwire != "" {
				builder, wired := runBuilders[tc.unwire]
				if !wired {
					t.Fatalf("前提是 %q 已接上（否则不需要摘表）", tc.unwire)
				}
				delete(runBuilders, tc.unwire)
				defer func() { runBuilders[tc.unwire] = builder }()
			}
			pipeline := pPipeline(tc.steps...)
			if tc.tweak != nil {
				tc.tweak(&pipeline)
			}

			result, perr := runPipelineNow(t, h, pipeline)
			if perr == nil {
				t.Fatalf("这一种清单该在预检就被拒，实得到终局：%+v", result)
			}
			if !strings.Contains(perr.Message(), tc.want) {
				t.Errorf("预检那句话该含 %q，实得 %q", tc.want, perr.Message())
			}
			if kind := perr.Kind(); kind != core.KindBadRequest {
				// 判 kind 而不是 HTTP 码：这一层还没有"错误→状态码"那一步（那是 `ErrDomain`，
				// 它的账在 `pipelines_test.go`）。这里要钉的是"这是使用者的清单有问题，
				// 不是本服务坏了"—— 翻成 500 会让用户以为要重启服务，而正确处置是改清单。
				t.Errorf("预检不过该是使用者那一侧的问题（bad request），实得 %v：%s",
					kind, perr.Message())
			}
			if len(result.Steps) != 0 {
				t.Errorf("预检没过就不该有任何步结论，实得 %+v", result.Steps)
			}
			if got := client.sent(); got != 0 {
				t.Errorf("预检没过就该一枪不发，实得 %d 次请求（URL：%v）", got, urlsOf(client))
			}
		})
	}
}

// TestPipelineAccountsColumnOverridesEveryStep：组合级账号列表盖过每一步的 body。
//
// 与定时任务那一条同一判据（"界面上选了 A、不能实际跑 body 里那个人"），
// 但这里是**另一份实现**（[stepParams] 自己写 `accounts`），所以要各自钉。
func TestPipelineAccountsColumnOverridesEveryStep(t *testing.T) {
	h, first, client := pipelineHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}},
		probeRoute{match: pipelineSnapshotRoute, bodies: []string{signInStatusClaimable}},
	)
	second := seedLoginAccount(t, h, fullPhone)
	if second == first {
		t.Fatalf("夹具坏了：两个账号同一个 id")
	}

	pipeline := pPipeline(
		pStep(capabilities.CodePrizes, `{"accounts":[`+quoteJSON(first)+`]}`),
		pStep(capabilities.CodeCloudSnapshot, `{"accounts":[`+quoteJSON(first)+`]}`),
	)
	pipeline.AccountIDs = []core.AccountID{core.NewAccountID(second)}

	result, perr := runPipelineNow(t, h, pipeline)
	if perr != nil {
		t.Fatalf("清单合法：%s", perr.Message())
	}
	for i, step := range result.Steps {
		if len(step.Runs) != 1 || step.Runs[0]["accountId"] != second {
			t.Errorf("第 %d 步该跑在组合选的那个账号上（%s），实得 %v", i+1, second, step.Runs)
		}
	}
	if ids := runIDsForAccount(t, h, first); len(ids) != 0 {
		t.Errorf("被步骤 body 提名、而组合没选的那个账号不该留下运行记录：%v", ids)
	}
	if got := client.count(pipelinePrizesRoute); got != 1 {
		t.Errorf("换目标账号不该改变请求数，%s 应打 1 次，实得 %d 次（URL：%v）",
			pipelinePrizesRoute, got, urlsOf(client))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 记账：只碰那四列
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelineBookkeepingTouchesOnlyFourColumns：跑完的账不许抹掉用户在跑动期间的编辑。
//
// 一次一键全跑要跑几分钟。这段时间里用户完全可能改了名字、加了步骤 ——
// 若记账走整行 [core.StateStore.UpsertPipeline]，那几分钟的编辑会被**安静地**打回
// 起跑前那一份（而界面上只看得见"上次执行"变了）。
func TestPipelineBookkeepingTouchesOnlyFourColumns(t *testing.T) {
	h, _, _ := failedThenOKHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	// ① 库里存的就是"要跑的那一份"：一步 `prizes`，能跑成。
	// ⚠ 直接走存储层时要自己带 owner（它拒绝在用户范围下写无主行，与 `seedSchedule` 同一判据）；
	// 界面上那一路由 `putPipeline` 填这一格，所以这条用例绕不开。
	runMe := pPipeline(pStep(capabilities.CodePrizes, ""))
	runMe.Name = "跑的那一份"
	runMe.Owner = userIDOf(t, h)
	if err := h.store.UpsertPipeline(ctx, scope, &runMe); err != nil {
		t.Fatalf("存组合失败: %v", err)
	}
	result, perr := runPipelineNow(t, h, runMe)
	if perr != nil {
		t.Fatalf("这一步该跑成：%s", perr.Message())
	}

	// ② 模拟"跑动期间用户改了定义"：改名、换描述、多加一步。
	const editedName = "跑的时候被用户改掉了"
	edited := runMe
	edited.Name = editedName
	edited.Description = ptrString("新写的描述")
	edited.Steps = append(append([]core.PipelineStep{}, edited.Steps...),
		pStep(capabilities.CodeCloudSnapshot, ""))
	if err := h.store.UpsertPipeline(ctx, scope, &edited); err != nil {
		t.Fatalf("改写组合失败: %v", err)
	}

	// ③ 落跑完之后的账（用**起跑时**那一份定义 + 那一次的结论）。
	if perr := h.server.recordPipelineRun(ctx, scope, runMe, result); perr != nil {
		t.Fatalf("记账失败：%s", perr.Message())
	}

	got, err := h.store.GetPipeline(ctx, scope, runMe.ID)
	if err != nil || got == nil {
		t.Fatalf("读不回那一行：%v", err)
	}
	if got.Name != editedName || len(got.Steps) != 2 || got.Description == nil {
		t.Errorf("定义被记账打回了起跑前那一份：name=%q steps=%d description=%v",
			got.Name, len(got.Steps), got.Description)
	}
	// 四列都要有值，且 ok 时 `last_error` 是**空**的。
	if got.LastStatus == nil || *got.LastStatus != core.PipelineOK {
		t.Errorf("跑成之后 last_status 该是 ok，实得 %v", got.LastStatus)
	}
	if got.LastRunAt == nil {
		t.Errorf("last_run_at 该写")
	}
	if got.LastDurationMS == nil || *got.LastDurationMS < 0 {
		t.Errorf("last_duration_ms 该写且不为负，实得 %v", got.LastDurationMS)
	}
	if got.LastError != nil {
		t.Errorf(`跑成那一趟的 last_error 该清空：`+
			"『ok』配一句上次的错误并排显示，是查不出原因最常见的形态，实得 %q", *got.LastError)
	}
}

// TestPipelineBookkeepingSkipsTheBuiltInChain：内置那一版没有行，也就没有四列可写。
func TestPipelineBookkeepingSkipsTheBuiltInChain(t *testing.T) {
	h, _, _ := failedThenOKHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	result := PipelineResult{PipelineID: PipelineRunAllID, Status: core.PipelineError,
		Note: "有步骤失败", StoppedAfter: -1}
	if perr := h.server.recordPipelineRun(ctx, scope, allRunPipeline(), result); perr != nil {
		t.Errorf("跑内置那一版之后记账该直接返回 nil（那不是故障，是这条路的正常形态），实得：%s",
			perr.Message())
	}
	row, err := h.store.GetPipeline(ctx, scope, PipelineRunAllID)
	if err != nil {
		t.Fatalf("查库失败: %v", err)
	}
	if row != nil {
		t.Errorf("内置全跑不该在库里凭空造一行（那等于让第一个人替所有人改默认值），实得有行：%+v", row)
	}
}

// TestPipelineErrorNoteIsScrubbedBeforeItReachesTheRow：落库那一句先过脱敏。
//
// `last_error` 会被界面显示、被用户截图发出来，而它装的是编排与上游的原话。
// 与 [recordScheduledRun] 同一条纪律，所以判据也一样：给一句带长 token 的原文，
// 查库里那一行**不含**原文。
func TestPipelineErrorNoteIsScrubbedBeforeItReachesTheRow(t *testing.T) {
	h, _, _ := failedThenOKHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	pipeline := pPipeline(pStep(capabilities.CodePrizes, ""))
	pipeline.Owner = userIDOf(t, h)
	if err := h.store.UpsertPipeline(ctx, scope, &pipeline); err != nil {
		t.Fatalf("存组合失败: %v", err)
	}

	// 48 个 token 形状字符：过 [core.RedactTokenLikeRuns] 的 32 字阈值。
	const secret = "SYNTHETICTOKENabcdefghijklmnopqrstuvwxyz0123456789"
	if perr := h.server.recordPipelineRun(ctx, scope, pipeline, PipelineResult{
		PipelineID: pipeline.ID, Name: pipeline.Name, Status: core.PipelineError,
		Note: "上游说这张票 " + secret + " 被拒了", StoppedAfter: -1,
	}); perr != nil {
		t.Fatalf("记账失败：%s", perr.Message())
	}

	got, err := h.store.GetPipeline(ctx, scope, pipeline.ID)
	if err != nil || got == nil {
		t.Fatalf("读不回那一行：%v", err)
	}
	if got.LastError == nil {
		t.Fatalf("非 ok 的那一趟必须写 last_error")
	}
	if strings.Contains(*got.LastError, secret) {
		t.Errorf("库里那一行带着原文凭据：%q", *got.LastError)
	}
	if !strings.Contains(*got.LastError, core.Placeholder) {
		t.Errorf("凭据那一段该被换成占位符，实得 %q", *got.LastError)
	}
}
