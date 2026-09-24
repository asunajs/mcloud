package app

// 消息推送任务（`msg-push`）跑一个账号那一段的用例。
//
// ⚠⚠ 这一族**零抓包样本**（口径与三方核对见 `internal/upstream/app_msg_push.go` 文件头）。
// 正因为零样本，这一层的用例只钉**我们自己定的规则**，不钉"上游会怎样"：
//
//  1. **门槛不成立就一条写请求都不发** —— 与 `notice` / `claims` 同一族。
//     这一条最重要：判据写反的表现是"对着真实账号多发一发 `obtain`"，
//     而它幂等与否**没有任何样本**可推。
//  2. ⭐ **"上游没回 `obtainCode`"不许伪造成成功** —— 写请求已经发出去了，
//     报成成功就等于让用户永远不会去查这一笔（`ClaimMsgPush` 头注第二条失败走法）。
//  3. **`pushOn` 缺失不判 Manual** —— 反向也要钉：不许哪天有人拿"没给大概是关了"
//     去凭空造一条待办（用户会为一个可能根本没坏的东西去翻 App 设置）。
//  4. **结论里的 code 串 = 注册表那一个** —— 待办的 `Capability` 与历史列表那一列
//     同源；写错了不报错，只是那条待办在界面上归不到这一条能力名下。
//
// 档位判定里那三个数字（`pushOn==0` / `secondTaskStatus==2` / `firstTaskStatus==3`）
// 的**出处**是旧项目的文字描述，这里按同一份描述断言，
// 拿到样本之后要改的是 `msg_push_run.go` 那三个常量，不是这里的期望值。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 这一族两条端点的子串（断言两头都要用：该打的打了几次、另一条一次没打）。
const (
	msgPushStatusEndpoint = "/market/msgPushOn/task/status"
	msgPushObtainEndpoint = "/market/msgPushOn/task/obtain"
)

// 四份 `task/status` 读数：该领、通知关着、不该领、判据缺失。
// ⚠ 键名 `onDuaration` 是上游的错拼，**不许改对**（上游侧由 `app_msg_push_test.go` 钉）。
const (
	msgPushStatusClaimable = `{"code":0,"msg":"success","result":{` +
		`"pushOn":1,"firstTaskStatus":3,"secondTaskStatus":2,"onDuaration":41}}`
	msgPushStatusPushOff = `{"code":0,"msg":"success","result":{` +
		`"pushOn":0,"firstTaskStatus":1,"secondTaskStatus":2,"onDuaration":41}}`
	msgPushStatusNotYet = `{"code":0,"msg":"success","result":{` +
		`"pushOn":1,"firstTaskStatus":3,"secondTaskStatus":3,"onDuaration":41}}`
	msgPushStatusNoCriteria = `{"code":0,"msg":"success","result":{` +
		`"pushOn":1,"onDuaration":41}}`
	// 首次奖励未领（`firstTaskStatus != 3`）：旧项目这一句只是 info。
	msgPushStatusFirstPending = `{"code":0,"msg":"success","result":{` +
		`"pushOn":1,"firstTaskStatus":1,"secondTaskStatus":2,"onDuaration":41}}`
)

const (
	msgPushObtainOK      = `{"code":0,"msg":"success","result":{"obtainCode":0}}`
	msgPushObtainNoCode  = `{"code":0,"msg":"success"}`
	msgPushObtainNonZero = `{"code":0,"msg":"success","result":{"obtainCode":5}}`
)

func msgPushRuns(store core.StateStore, routes ...liveTaskRoute) (MsgPushRun, *liveTaskScript) {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	return MsgPushRun{SignInWorld: signInWorldFor(store, script)}, script
}

func msgPushAccount() core.AccountID { return core.NewAccountID(liveTestAccountID) }

// ─────────────────────────────────────────────────────────────────────────────
// 判定（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanMsgPushBranches：四档各自怎么到。
func TestPlanMsgPushBranches(t *testing.T) {
	ptr := func(v int64) *int64 { return &v }
	cases := []struct {
		name   string
		status upstream.MsgPushStatus
		want   MsgPushPlan
	}{
		{
			name:   "通知开着 + secondTaskStatus=2 ⇒ 该领",
			status: upstream.MsgPushStatus{PushOn: ptr(1), SecondTaskStatus: ptr(2)},
			want:   MsgPushPlanObtain,
		},
		{
			// ⭐ 顺序：`pushOn==0` 是**前置**开关，第二个数再"该领"也不发。
			name:   "通知关着 + secondTaskStatus=2 ⇒ 仍判人工，不发请求",
			status: upstream.MsgPushStatus{PushOn: ptr(0), SecondTaskStatus: ptr(2)},
			want:   MsgPushPlanManual,
		},
		{
			name:   "通知开着 + secondTaskStatus 不是 2 ⇒ 不该领",
			status: upstream.MsgPushStatus{PushOn: ptr(1), SecondTaskStatus: ptr(3)},
			want:   MsgPushPlanNothing,
		},
		{
			name:   "上游没给 secondTaskStatus ⇒ 判不了",
			status: upstream.MsgPushStatus{PushOn: ptr(1)},
			want:   MsgPushPlanUnknown,
		},
		{
			name:   "两个判据字段都没给 ⇒ 判不了（不是 Manual）",
			status: upstream.MsgPushStatus{},
			want:   MsgPushPlanUnknown,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanMsgPush(c.status); got != c.want {
				t.Errorf("实得 %v，期望 %v", got, c.want)
			}
		})
	}
}

// TestPlanMsgPushNeverJudgesManualWithoutPushOn：⭐ 反向钉"不许凭空造待办"。
//
// 把 [PlanMsgPush] 里那句 `ok &&` 去掉（即"没给就当关了"），本条立刻红：
// 那种写法会让每一次"上游少给一个字段"都在界面上多出一条"去 App 里开通知"的假待办。
func TestPlanMsgPushNeverJudgesManualWithoutPushOn(t *testing.T) {
	for _, second := range []*int64{nil, msgPtrFor(0), msgPtrFor(2), msgPtrFor(3)} {
		if got := PlanMsgPush(upstream.MsgPushStatus{SecondTaskStatus: second}); got == MsgPushPlanManual {
			t.Errorf("pushOn 缺失时判成 Manual（secondTaskStatus=%v）："+
				"上游没说等于没说，不许当成「关着」", second)
		}
	}
}

// msgPtrFor 让表里能写 `nil` 与数值混排。
func msgPtrFor(v int64) *int64 { return &v }

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// TestMsgPushRunSendsNoWriteRequestWhenTheGateIsClosed：三档不发 `obtain`。
//
// 三条都同时断言"`status` 只打一次"：这一族**刻意不复查**（零样本，理由见
// `msg_push_run.go` 头注），多打一发在别处是稳妥，在这里是把未验证的行为当默认。
func TestMsgPushRunSendsNoWriteRequestWhenTheGateIsClosed(t *testing.T) {
	cases := []struct {
		name     string
		body     string
		verdict  MsgPushVerdict
		hasError bool
	}{
		{"通知关着", msgPushStatusPushOff, MsgPushManual, false},
		{"secondTaskStatus 不是该领的值", msgPushStatusNotYet, MsgPushNothing, false},
		{"判据字段缺失", msgPushStatusNoCriteria, MsgPushUnknown, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, script := msgPushRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{msgPushStatusEndpoint, []string{c.body}},
				liveTaskRoute{msgPushObtainEndpoint, []string{msgPushObtainOK}},
			)
			res, perr := run.RunAccount(context.Background(), msgPushAccount())
			if perr != nil {
				t.Fatalf("判得出来这一趟不该报错：%v", perr)
			}
			if got := script.count(msgPushObtainEndpoint); got != 0 {
				t.Errorf("门槛不成立就一条写请求都不该发，obtain 实得 %d 次", got)
			}
			if got := script.count(msgPushStatusEndpoint); got != 1 {
				t.Errorf("status 只该打 1 次（这一族刻意不复查），实得 %d 次", got)
			}
			if res.Verdict != c.verdict {
				t.Errorf("实得 %v，期望 %v（理由 %q）", res.Verdict, c.verdict, res.Reason)
			}
			if res.ObtainCode != nil {
				t.Errorf("没发写请求就不该有 obtainCode，实得 %d", *res.ObtainCode)
			}
			if c.hasError != (res.Error != "") {
				t.Errorf("Error 该是 %v，实得 %q", c.hasError, res.Error)
			}
		})
	}
}

// TestMsgPushRunClaimsOnceWhenTheGateOpens：该领 ⇒ `obtain` **恰好一次**，且不复查。
func TestMsgPushRunClaimsOnceWhenTheGateOpens(t *testing.T) {
	run, script := msgPushRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{msgPushStatusEndpoint, []string{msgPushStatusClaimable}},
		liveTaskRoute{msgPushObtainEndpoint, []string{msgPushObtainOK}},
	)
	res, perr := run.RunAccount(context.Background(), msgPushAccount())
	if perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	if got := script.count(msgPushObtainEndpoint); got != 1 {
		t.Errorf("obtain 应打 1 次，实得 %d 次", got)
	}
	if got := script.count(msgPushStatusEndpoint); got != 1 {
		t.Errorf("status 应打 1 次（不复查），实得 %d 次", got)
	}
	if res.Verdict != MsgPushDone {
		t.Fatalf("应判已领，实得 %v（%s）", res.Verdict, res.Error)
	}
	if res.ObtainCode == nil || *res.ObtainCode != 0 {
		t.Errorf("应记下 obtainCode=0，实得 %v", res.ObtainCode)
	}
	// 读数照抄、不解释：小结里出现的必须是**数值**而不是中文译名。
	summary := res.SummaryLine()
	if !strings.Contains(summary, "secondTaskStatus=2") {
		t.Errorf("小结该带原样读数，实得 %q", summary)
	}
	// ⚠ 凭据卫生：小结进 `runs.message` 与推送。
	if strings.Contains(summary, "JWT-1") || strings.Contains(summary, "jwttoken") {
		t.Errorf("小结里漏了凭据形态的东西：%q", summary)
	}
}

// TestMsgPushMissingObtainCodeIsNotFakedAsSuccess：⭐ 上游没回 `obtainCode` ⇒ 报失败，
// 但**不**交回错误（写请求已经发出去了，"发过"是这一趟唯一的事实）。
//
// 反向改坏它：把 `ClaimMsgPush` 里 `case !ok:` 那一格改成 `Obtained`（"缺了当 0"），
// 本条立刻红 —— 而界面上什么都不会变红，只有账对不上。
func TestMsgPushMissingObtainCodeIsNotFakedAsSuccess(t *testing.T) {
	run, script := msgPushRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{msgPushStatusEndpoint, []string{msgPushStatusClaimable}},
		liveTaskRoute{msgPushObtainEndpoint, []string{msgPushObtainNoCode}},
	)
	res, perr := run.RunAccount(context.Background(), msgPushAccount())
	if perr != nil {
		t.Fatalf("发过写请求的失败要折成结论，不该交回错误：%v", perr)
	}
	if got := script.count(msgPushObtainEndpoint); got != 1 {
		t.Errorf("前置夹具：obtain 应打了 1 次，实得 %d 次", got)
	}
	if res.Verdict != MsgPushFailed {
		t.Errorf("缺 obtainCode 不许伪造成成功，实得 %v", res.Verdict)
	}
	if res.ObtainCode != nil {
		t.Errorf("上游没给就该是 nil（≠ 0），实得 %d", *res.ObtainCode)
	}
	if !strings.Contains(res.SummaryLine(), "obtainCode") {
		t.Errorf("小结里要说清判不出的是什么，实得 %q", res.SummaryLine())
	}
}

// TestMsgPushNonZeroObtainCodeIsFailure：`obtainCode != 0` ⇒ 失败，且把那个数带出来。
func TestMsgPushNonZeroObtainCodeIsFailure(t *testing.T) {
	run, _ := msgPushRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{msgPushStatusEndpoint, []string{msgPushStatusClaimable}},
		liveTaskRoute{msgPushObtainEndpoint, []string{msgPushObtainNonZero}},
	)
	res, perr := run.RunAccount(context.Background(), msgPushAccount())
	if perr != nil {
		t.Fatalf("同样不该交回错误：%v", perr)
	}
	if res.Verdict != MsgPushFailed {
		t.Fatalf("实得 %v，期望失败", res.Verdict)
	}
	if res.ObtainCode == nil || *res.ObtainCode != 5 {
		t.Errorf("应把上游给的那个数带出来，实得 %v", res.ObtainCode)
	}
	if !strings.Contains(res.Error, "5") {
		t.Errorf("失败原因里要有那个数，实得 %q", res.Error)
	}
}

// TestMsgPushStatusFailureReturnsError：查状态那一枪挂了 ⇒ 交回错误。
//
// 这里**不给** `status` 路由：脚本会回 404 + 一句"no route"，落成就读失败。
// 结论同时得是"这一趟没办成"（[MsgPushFailed]），不留一个会被念成"领到了"的零值。
func TestMsgPushStatusFailureReturnsError(t *testing.T) {
	run, script := msgPushRuns(&poolSignInStore{*liveTaskStore()})
	res, perr := run.RunAccount(context.Background(), msgPushAccount())
	if perr == nil {
		t.Fatal("查状态失败必须交回错误（编排层要多发一条 errored 事件）")
	}
	if got := script.count(msgPushObtainEndpoint); got != 0 {
		t.Errorf("状态没读到就一条写请求都不该发，实得 %d 次", got)
	}
	if res.Verdict != MsgPushFailed {
		t.Errorf("走错误那条路时也要有结论可念，实得 %v", res.Verdict)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 结论 → 四态
// ─────────────────────────────────────────────────────────────────────────────

// TestMsgPushRunReportMapsVerdicts：⭐ 四档各自的走法，含 code 串。
//
// `Manual` 那一格是这一族**唯一**能把"得有人去 App 里点一下"传到历史列表与推送的通路：
// 压成成功就是旧项目那句 info 的老毛病（没人知道要去开）。
// ⚠ 这里同时钉住待办上的能力 code：它写错不报错，只是那条待办归不到这一条能力名下。
func TestMsgPushRunReportMapsVerdicts(t *testing.T) {
	cases := []struct {
		name        string
		result      MsgPushResult
		wantOutcome core.RunOutcome
		wantSteps   int
	}{
		{
			name:        "已领 ⇒ 成功",
			result:      MsgPushResult{Account: msgPushAccount(), Verdict: MsgPushDone},
			wantOutcome: core.OutcomeSuccess, wantSteps: 0,
		},
		{
			name:        "不该领 ⇒ 成功",
			result:      MsgPushResult{Account: msgPushAccount(), Verdict: MsgPushNothing, Reason: "r"},
			wantOutcome: core.OutcomeSuccess, wantSteps: 0,
		},
		{
			name:        "判不了 ⇒ 成功（没发请求，也没什么可办）",
			result:      MsgPushResult{Account: msgPushAccount(), Verdict: MsgPushUnknown, Reason: "r"},
			wantOutcome: core.OutcomeSuccess, wantSteps: 0,
		},
		{
			name:        "通知没开 ⇒ 要人动手",
			result:      MsgPushResult{Account: msgPushAccount(), Verdict: MsgPushManual, Reason: "r"},
			wantOutcome: core.OutcomeManualRequired, wantSteps: 1,
		},
		{
			name: "发过但没成 ⇒ 失败",
			result: MsgPushResult{
				Account: msgPushAccount(), Verdict: MsgPushFailed, Error: "上游回 obtainCode=5",
			},
			wantOutcome: core.OutcomeFailed, wantSteps: 0,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.result.RunReport()
			if report.Outcome != c.wantOutcome {
				t.Errorf("档位映射错：实得 %v，期望 %v", report.Outcome, c.wantOutcome)
			}
			if len(report.ManualSteps) != c.wantSteps {
				t.Fatalf("待办条数：实得 %d，期望 %d（%+v）",
					len(report.ManualSteps), c.wantSteps, report.ManualSteps)
			}
			if c.wantSteps == 1 {
				step := report.ManualSteps[0]
				if step.Capability != capabilities.CodeMsgPush {
					t.Errorf("待办的能力 code 应取注册表那一个，实得 %q", step.Capability)
				}
				if step.Account == nil || *step.Account != c.result.Account {
					t.Errorf("待办要指名是哪个账号，实得 %v", step.Account)
				}
				if step.Target != core.TargetMobileApp {
					t.Errorf("这一步只能在 App 里做，Target 实得 %v", step.Target)
				}
			}
			if report.Summary == nil || *report.Summary != c.result.SummaryLine() {
				t.Errorf("小结没带上那一句话：%v", report.Summary)
			}
		})
	}
}

// TestMsgPushFirstRewardNoticeStaysASentence：`firstTaskStatus != 3` 只进小结，
// **不**升成待办 —— 旧项目那句是 info、不影响成功，我们照这个口径。
//
// ⚠ 但"没给 `firstTaskStatus`"连那句话都不说（`msgPushFirstRewardPending` 的
// 一处**主动偏离**旧项目，理由写在它上面）：本条两头都钉。
func TestMsgPushFirstRewardNoticeStaysASentence(t *testing.T) {
	cases := []struct {
		name     string
		body     string
		wantNote bool
	}{
		{"首次未领（=1）⇒ 小结里带那一句", msgPushStatusFirstPending, true},
		{"首次已领（=3）⇒ 不带", msgPushStatusClaimable, false},
		{"上游没给 firstTaskStatus ⇒ 不带（不凭空提示）",
			`{"code":0,"msg":"success","result":{"pushOn":1,"secondTaskStatus":2}}`, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, _ := msgPushRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{msgPushStatusEndpoint, []string{c.body}},
				liveTaskRoute{msgPushObtainEndpoint, []string{msgPushObtainOK}},
			)
			res, perr := run.RunAccount(context.Background(), msgPushAccount())
			if perr != nil {
				t.Fatalf("这一趟该走通：%v", perr)
			}
			hasNote := strings.Contains(res.SummaryLine(), "首次奖励")
			if hasNote != c.wantNote {
				t.Errorf("那一句在不在小结里：实得 %v，期望 %v（%q）",
					hasNote, c.wantNote, res.SummaryLine())
			}
			// 无论带不带那一句，这一趟的档位与待办都不受影响。
			report := res.RunReport()
			if report.Outcome != core.OutcomeSuccess {
				t.Errorf("首次奖励那句话不该改变档位，实得 %v", report.Outcome)
			}
			if len(report.ManualSteps) != 0 {
				t.Errorf("不该因此多出一条待办：%+v", report.ManualSteps)
			}
		})
	}
}

// TestMsgPushReadingsSayQueNotZero：缺的读数念"缺"，不念 0。
//
// 口径与 [noticeSlotLabel] 同一条：把"没说"写成"说了 0"是最难发现的一类假话，
// 而在这条链上它还额外有害 —— `pushOn=0` 恰恰是"通知关着"那个判据的值。
func TestMsgPushReadingsSayQueNotZero(t *testing.T) {
	res := MsgPushResult{Account: msgPushAccount(), Verdict: MsgPushUnknown, Reason: "r"}
	summary := res.readings()
	if !strings.Contains(summary, "pushOn=缺") || !strings.Contains(summary, "secondTaskStatus=缺") {
		t.Errorf("三个读数都缺时应逐格念\"缺\"，实得 %q", summary)
	}
	if strings.Contains(summary, "pushOn=0") {
		t.Errorf("不许把缺失念成 0：%q", summary)
	}
}
