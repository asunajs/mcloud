package app

// 「刷新 auth」走编排时的**四态映射**用例（[RefreshReport.RunReport]）。
//
// ⚠ 判据本身（哪一档该刷、新串长成什么样）在 `internal/capabilities`，
// 打哪个端、怎么落库在 `auth_refresh_test.go`。这里只管一格判断：
// **调度拿到的是成功、待人工，还是失败**。
//
// 三格都会改变外部行为，所以逐条钉：
//   - `not-yet` 报成失败 ⇒ 调度按补跑再来一次，而它每一轮都会再判一次 `not-yet`，
//     历史列表被同一句"还没到提前量"刷满；
//   - `manual-only` 报成失败 ⇒ 界面上是红色 + "重试"，而重试一万次也不会变；
//   - `manual-only` 报成成功 ⇒ 那个账号的凭据真的过期时，它名下所有能力都跑不动，
//     而历史里看不到任何一句"要人去登录一次"。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

func refreshOutcome(decision capabilities.RefreshDecision, rotated bool) RefreshReport {
	return RefreshReport{
		AccountID: core.NewAccountID("19****01"),
		End:       "mobile",
		Decision:  decision,
		Rotated:   rotated,
	}
}

func TestRefreshRunReportFourStates(t *testing.T) {
	cases := []struct {
		name        string
		report      RefreshReport
		want        core.RunOutcome
		wantSteps   int
		wantMessage string
	}{
		{
			name:   "换成了",
			report: refreshOutcome(capabilities.RefreshNeeded, true),
			want:   core.OutcomeSuccess, wantSteps: 0,
		},
		{
			// 这一趟的目标是"到点就换"，而"没到点所以什么都没做"正是它该做的。
			name:   "还没到提前量",
			report: refreshOutcome(capabilities.RefreshNotYet, false),
			want:   core.OutcomeSuccess, wantSteps: 0,
		},
		{
			name:        "只能人工换 auth",
			report:      refreshOutcome(capabilities.RefreshManualOnly, false),
			want:        core.OutcomeManualRequired,
			wantSteps:   1,
			wantMessage: "重新登录",
		},
		{
			// ⚠ 这一档在 [RefreshAccountAuth] 的成功返回里不该出现，
			// 但**必须**有落点：Go 的 switch 漏一档不报错，会静默走 default。
			// 反向改坏它的方式：把 [RefreshReport.RunReport] 那个 default 删掉。
			name:   "判到该刷却没换成",
			report: refreshOutcome(capabilities.RefreshNeeded, false),
			want:   core.OutcomeFailed, wantSteps: 0,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.report.RunReport()
			if report.Outcome != c.want {
				t.Errorf("应落在 %v，实得 %v", c.want, report.Outcome)
			}
			if len(report.ManualSteps) != c.wantSteps {
				t.Fatalf("待办条数应是 %d，实得 %d：%+v", c.wantSteps, len(report.ManualSteps), report.ManualSteps)
			}
			if c.wantMessage == "" {
				return
			}
			step := report.ManualSteps[0]
			if !strings.Contains(step.Instruction, c.wantMessage) {
				t.Errorf("待办要说清下一步，期望含 %q，实得 %q", c.wantMessage, step.Instruction)
			}
			// 落点必须是**这条能力**：待办按 capability 去重与聚合，
			// 写成别的 code 会聚到另一个任务名下，而那个人永远收不到提醒。
			if step.Capability != capabilities.CodeAuthRefresh {
				t.Errorf("待办的能力应是 %s，实得 %q", capabilities.CodeAuthRefresh, step.Capability)
			}
			if step.Target != core.TargetWebui {
				t.Errorf("这一步是在网页的账号页做的，落点应是 webui，实得 %v", step.Target)
			}
			if step.Account == nil || step.Account.String() != "19****01" {
				t.Errorf("待办要带上是哪个账号，实得 %+v", step.Account)
			}
		})
	}
}

// TestRefreshSummaryCarriesNoCredential：小结里不许出现凭据形态的东西。
//
// 这句话进 `runs.message` 与终局事件，是**出网**的（同 `refreshAuthPayload` 那条纪律）。
// 换成的新令牌只落库，不回填给调用方。
func TestRefreshSummaryCarriesNoCredential(t *testing.T) {
	line := RefreshReport{
		AccountID: core.NewAccountID("19****01"),
		End:       "mobile",
		Decision:  capabilities.RefreshNeeded,
		Rotated:   true,
	}.SummaryLine()
	for _, unwanted := range []string{"Basic ", "bW9iaWxl", "TOKEN"} {
		if strings.Contains(line, unwanted) {
			t.Errorf("小结里出现了 %q：%q", unwanted, line)
		}
	}
	if !strings.Contains(line, "mobile") {
		t.Errorf("小结要说出是哪一端换成了，实得 %q", line)
	}
}
