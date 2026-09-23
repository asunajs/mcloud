package app

// 「每日签到」走编排时的**四态映射**用例（[SignInAccountResult.RunReport]）。
//
// ⚠ 这条链本身（打哪些接口、余额取哪个数）不在这里测 —— 那些判据在
// `signin_test.go` 与 `receive_test.go`。本文件只管一件事：
// **一趟跑完，编排层拿到的是四态里的哪一格**。
// 那一格错了的后果不是显示不准，是**调度行为**：报成失败会被补跑，
// 而补跑会对着一个"今天已经签过"的账号再打一遍写请求。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// signedToday 造一份"服务端把今天标成已签 / 未签"的 `infoV3`。
func signedToday(signed bool) upstream.SignInStatus {
	return upstream.SignInStatus{
		MonthDays: 3,
		Days:      []upstream.SignInDay{{Day: 22, Today: true, Signed: signed, CurrentMonth: true}},
	}
}

// signedNowReport 是"来的时候没签、这一趟签上了"那一份结论。
func signedNowReport() SignInReport {
	return SignInReport{
		Before: signedToday(false),
		After:  signedToday(true),
		Result: &upstream.SignInResult{TodaySignIn: false, SignInPoints: 5},
	}
}

// TestSignInRunReportFourStates：⭐ 一格一档，逐条钉。
//
// 表里每一条都对应一个**会改变调度行为**的判断，所以不合并成一句"看情况"。
func TestSignInRunReportFourStates(t *testing.T) {
	cases := []struct {
		name   string
		result SignInAccountResult
		want   core.RunOutcome
	}{
		{
			name:   "本来就已经签过",
			result: SignInAccountResult{Report: SignInReport{Before: signedToday(true), After: signedToday(true)}},
			want:   core.OutcomeSuccess,
		},
		{
			name:   "这次签上了，没有可领的",
			result: SignInAccountResult{Report: signedNowReport()},
			want:   core.OutcomeSuccess,
		},
		{
			name: "这次签上了，也领到了",
			result: SignInAccountResult{Report: signedNowReport(),
				Received: []ReceiveOutcome{{Kind: ReceiveOutcomeImmediate, Received: 50}}},
			want: core.OutcomeSuccess,
		},
		{
			name: "签上了但有一笔领取失败",
			result: SignInAccountResult{Report: signedNowReport(),
				Received: []ReceiveOutcome{
					{Kind: ReceiveOutcomeImmediate, Received: 50},
					{Kind: ReceiveOutcomeFailed},
				}},
			want: core.OutcomePartialSuccess,
		},
		{
			name:   "签上了但领取整批失败",
			result: SignInAccountResult{Report: signedNowReport(), ReceiveFailure: "取 receiveList 回 90001"},
			want:   core.OutcomePartialSuccess,
		},
		{
			// ⚠ 这一格的判据是**两个条件同时成立**：没发写请求（`Result == nil`）
			// 且回查说今天没签。只看后一个会把"来了就已经签过"误判成失败。
			name:   "没发写请求、回查也说没签上",
			result: SignInAccountResult{Report: SignInReport{Before: signedToday(false), After: signedToday(false)}},
			want:   core.OutcomeFailed,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.result.RunReport()
			if report.Outcome != c.want {
				t.Errorf("应落在 %v，实得 %v（小结：%s）", c.want, report.Outcome, summaryOf(report))
			}
			// 这条链上**没有**"要人去客户端点一下"的动作：签到不需要人工，
			// 领取失败也不是人工能补的（重跑就是）。出现待办就是有人在凭错误串猜。
			if len(report.ManualSteps) != 0 {
				t.Errorf("签到不该产出人工待办，实得 %+v", report.ManualSteps)
			}
		})
	}
}

// TestSignInSummarySaysWhatHappened：小结要能分辨"签上了"与"已经签过"。
//
// 定时任务连跑一个月，这两句混成一团的话，日志里就看不出它到底有没有在干活
// （[SignInReport.WasAlreadySigned] 那条注释说的正是这件事）。
func TestSignInSummarySaysWhatHappened(t *testing.T) {
	signed := SignInAccountResult{Report: signedNowReport()}.SummaryLine()
	if !strings.Contains(signed, "本次签到成功") || !strings.Contains(signed, "+5 豆") {
		t.Errorf("签上了的小结该说\"本次签到成功（+N 豆）\"，实得 %q", signed)
	}
	already := SignInAccountResult{
		Report: SignInReport{Before: signedToday(true), After: signedToday(true)},
	}.SummaryLine()
	if !strings.Contains(already, "今天已经签过") {
		t.Errorf("本来就签过了的小结该与\"这次签上\"分开，实得 %q", already)
	}
	unconfirmed := SignInAccountResult{
		Report: SignInReport{Before: signedToday(false), After: signedToday(false)},
	}.SummaryLine()
	if !strings.Contains(unconfirmed, "未确认") {
		t.Errorf("没签上时不能写成成功，实得 %q", unconfirmed)
	}
}

// TestSignInSummaryCarriesNoCredential：小结是**出网**的那一句（进 `runs.message` 与终局事件）。
//
// ⚠ 反向改坏它的方式：把 `ReceiveFailure` 那句原样拼进小结 —— 上游的错误响应体里
// 可能带着请求头回显，而那里面就有 `Authorization`。本实现只写"领取整批失败"这一句
// 人话，把原因留在事件流的那条 error 里（由 [core] 那一层的脱敏负责）。
func TestSignInSummaryCarriesNoCredential(t *testing.T) {
	line := SignInAccountResult{
		Report:         signedNowReport(),
		ReceiveFailure: "夹具：Authorization Basic bW9iaWxlOjE5OTAwMDAwMDAwMDpUT0tFTg==",
	}.SummaryLine()
	if strings.Contains(line, "Basic ") || strings.Contains(line, "bW9iaWxl") {
		t.Errorf("小结里出现了凭据形态的内容：%q", line)
	}
}

// summaryOf 取报告里那一句（断言失败时把它打出来，便于看出判到哪一格）。
func summaryOf(report core.RunReport) string {
	if report.Summary == nil {
		return "（无小结）"
	}
	return *report.Summary
}
