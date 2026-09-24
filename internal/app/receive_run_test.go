package app

// [ReceiveRun] 的用例：只领取、不签到。
//
// 这里钉两件事：
//
//  1. **取不到 H5 `deviceId` 时照发**（只把原因记下来）。反向改法"没 deviceId 就跳过"
//     看起来更保守，实际是把一次本可以成的领取变成"什么都没做"——
//     而 `615` 本身就是明确答案（成文见 `receive_run.go` 的 ⚠ 注释）。
//  2. 逐条实况 → 四态：**全成 / 无可领 = 成功，部分失败 = 部分成功，一笔没成 = 失败**。

import (
	"context"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

func int64Ptr(v int64) *int64 { return &v }

// TestReceiveRunSendsTheWriteRequestsAndSurvivesMissingDeviceID：
// 该领的领掉，`deviceId` 没换到也**照发**。
func TestReceiveRunSendsTheWriteRequestsAndSurvivesMissingDeviceID(t *testing.T) {
	script := liveTaskScriptWith(
		append(sessionRoutes(),
			liveTaskRoute{"/ycloud/signin/page/infoV3", []string{statusPayload(
				`"cal":[],"receiveList":[{"cloudType":0,"cloudNum":50,"recordId":"R1"}]`)}},
			liveTaskRoute{"/ycloud/signin/page/receiveV3", []string{
				`{"code":0,"msg":"success","result":{"receive":50}}`}},
		)...,
	)
	// [poolSignInStore] 的 `GetSmidState` 刻意回错误 ⇒ [DeviceID] 失败，
	// 于是本用例走的就是"`deviceId` 没换到"那一条支路（真实存在的那条）。
	run := ReceiveRun{SignInWorld: signInWorldFor(&poolSignInStore{*liveTaskStore()}, script)}
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("少一个 deviceId 不该让整趟报错：%v", perr)
	}
	if res.DeviceFailure == "" {
		t.Errorf("取不到 deviceId 必须把原因记下来，否则小结里那句「deviceId 没换到」没有出处")
	}
	if len(res.Outcomes) != 1 || res.Outcomes[0].Kind != ReceiveOutcomeImmediate {
		t.Fatalf("应有一条立即领取，实得 %+v", res.Outcomes)
	}
	if got := script.count("/ycloud/signin/page/receiveV3"); got != 1 {
		t.Errorf("该发的那一条必须发出去，receiveV3 实得 %d 次", got)
	}
	// 它仍然算成功：这一趟该做的都做了，风控那一头回什么由 `615` 说。
	if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
		t.Errorf("实得 %v", report.Outcome)
	}
}

// TestReceiveResultMapsOutcomesToOutcome：逐条 → 四态的映射表（纯函数，不出网）。
func TestReceiveResultMapsOutcomesToOutcome(t *testing.T) {
	cases := []struct {
		name    string
		res     ReceiveResult
		want    core.RunOutcome
		summary string
	}{
		{
			name: "账上没东西",
			res:  ReceiveResult{Account: core.NewAccountID(liveTestAccountID)},
			want: core.OutcomeSuccess, summary: "账上没有可领的云朵",
		},
		{
			name: "全领到了",
			res: ReceiveResult{Outcomes: []ReceiveOutcome{
				{Kind: ReceiveOutcomeImmediate, Received: 50},
				{Kind: ReceiveOutcomeMonthlyExpansion, CloudCount: int64Ptr(100)},
			}},
			want: core.OutcomeSuccess, summary: "领到 150 豆（2 笔）",
		},
		{
			name: "一部分失败",
			res: ReceiveResult{Outcomes: []ReceiveOutcome{
				{Kind: ReceiveOutcomeImmediate, Received: 50},
				{Kind: ReceiveOutcomeFailed, Error: "上游回 615"},
			}},
			want: core.OutcomePartialSuccess, summary: "领到 50 豆（1 笔）；1 笔失败",
		},
		{
			name: "一笔都没成",
			res: ReceiveResult{Outcomes: []ReceiveOutcome{
				{Kind: ReceiveOutcomeFailed, Error: "上游回 615"},
			}},
			want: core.OutcomeFailed, summary: "领到 0 豆（0 笔）；1 笔失败",
		},
		{
			name: "下月才可领的不算失败",
			res: ReceiveResult{Outcomes: []ReceiveOutcome{
				{Kind: ReceiveOutcomeNextMonth, CloudNum: 30},
			}},
			want: core.OutcomeSuccess, summary: "领到 0 豆（0 笔）；1 笔下月才可领",
		},
		{
			name: "deviceId 没换到会进小结",
			res: ReceiveResult{Outcomes: []ReceiveOutcome{
				{Kind: ReceiveOutcomeImmediate, Received: 10},
			}, DeviceFailure: "库里没有 smid 状态"},
			want:    core.OutcomeSuccess,
			summary: "领到 10 豆（1 笔）；deviceId 没换到",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.res.RunReport()
			if report.Outcome != c.want {
				t.Errorf("实得 %v，期望 %v（小结 %q）", report.Outcome, c.want, summaryOf(report))
			}
			if got := summaryOf(report); got != c.summary {
				t.Errorf("小结应逐字为 %q，实得 %q", c.summary, got)
			}
			if report.ManualSteps == nil {
				t.Errorf("ManualSteps 应是空 slice 而不是 nil")
			}
		})
	}
}

// TestReceiveRunReturnsErrorWhenTheStatusQueryFails：查账目失败交回错误。
//
// 与 [ClaimRun] 那条同理：走错误那条路才会多发一条 `errored` 事件、
// 并把上游原文写进 `runs.message`。
func TestReceiveRunReturnsErrorWhenTheStatusQueryFails(t *testing.T) {
	script := liveTaskScriptWith(
		append(sessionRoutes(), liveTaskRoute{"/ycloud/signin/page/infoV3",
			[]string{`{"code":1,"msg":"系统繁忙，请稍后重试"}`}})...,
	)
	run := ReceiveRun{SignInWorld: signInWorldFor(&poolSignInStore{*liveTaskStore()}, script)}
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("账目没查到就该交回错误")
	}
	if got := script.count("/ycloud/signin/page/receiveV3"); got != 0 {
		t.Errorf("没查到账目就不该发领取请求，实得 %d 次", got)
	}
}
