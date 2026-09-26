package app

// ─────────────────────────────────────────────────────────────────────────────
// 小结那句"有几条没做成"要分档（`failurePhrase` / `absorb` 的 `Fired` 那一格）
// ─────────────────────────────────────────────────────────────────────────────
//
// 来 2026-09-26 第廿四趟整趟真跑（两个测试号各一趟，号码不进成文）。号二那句是
// `2 条上报失败`，而底下两条**根本不是一回事**：
//   - 434：这一趟前面没有任务往网盘传过东西 ⇒ 一枪都没发（本仓刻意不发）；
//   - 1020：真发了那一发邮箱备份 ⑧、上游回 `HTTP 404`。
//
// 原来那一句无条件写"上报失败"，于是两种失败在界面上长得一样，而用户的下一步
// 完全不同（前者是 Q41 那一问 / 等前面有上传，后者要去查这一族的入口）。
// 分档判据是 [TaskStepRecord.Fired]（"出过网"，**不是**"上游认了"）。
//
// ⚠ 这一族是**纯单元**用例：直接手造 `TaskMatrixRunResult`，不走夹具。
// 整趟形状的反向用例在 `task_matrix_share_run_test.go`
// （`TestTaskMatrixRunShareSaysNothingWasSentWhenTheGateBlocksIt`）。

import (
	"strings"
	"testing"
)

func TestSummaryLineSeparatesUnsentFromFailedReports(t *testing.T) {
	cases := []struct {
		name    string
		counts  TaskRunCounts
		want    string
		notWant string
	}{
		{
			// 号一那一档：唯一的失败是 434 的"没有可分享的文件"。
			name:    "全部没出网 ⇒ 不许再念『上报』",
			counts:  TaskRunCounts{Read: 27, Pending: 10, Failed: 1, FailedNoShot: 1},
			want:    "1 条没做成（一枪都没发：缺前置）",
			notWant: "上报失败",
		},
		{
			// 号二那一档：434 没出网 + 1020 真发过被打回。
			name:   "两半都有 ⇒ 各报各的数",
			counts: TaskRunCounts{Read: 26, Pending: 9, Failed: 2, FailedNoShot: 1},
			want:   "2 条没做成（1 条发出去了、1 条一枪没发：缺前置）",
		},
		{
			// 反向改坏它：把三档合回一句 ⇒ 这一条红（它要求仍念"上报失败"）。
			name:    "全部发过枪 ⇒ 维持原来的『上报失败』",
			counts:  TaskRunCounts{Read: 27, Pending: 10, Failed: 1, FailedNoShot: 0},
			want:    "1 条上报失败",
			notWant: "一枪都没发",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			res := TaskMatrixRunResult{Counts: tc.counts}
			line := res.SummaryLine()
			if !strings.Contains(line, tc.want) {
				t.Errorf("小结要念出 %q，实得 %q", tc.want, line)
			}
			if tc.notWant != "" && strings.Contains(line, tc.notWant) {
				t.Errorf("小结不该出现 %q（一枪没发却写『上报』= 让用户去查上游），实得 %q",
					tc.notWant, line)
			}
		})
	}
}

// TestAbsorbSplitsFailedByWhetherAShotWentOut 钉住计数那一半：`FailedNoShot` 是
// `Failed` 的**子集**，而它只在 `Fired == false` 时上涨。
//
// 反向改坏它：把 `absorb` 里那三处 `if failedNoShot` 删掉 ⇒ 这一条红（`Failed` 仍对，
// 于是**只有**这一条能证明分档没被静默丢掉；`SummaryLine` 那三条单测造的是手搓计数，
// 抓不到"接线断了"）。
func TestAbsorbSplitsFailedByWhetherAShotWentOut(t *testing.T) {
	var counts TaskRunCounts
	counts.absorb(TaskStepRecord{TaskID: 434, Kind: StepRunner, Err: "没有可分享的文件", Fired: false})
	counts.absorb(TaskStepRecord{TaskID: 1020, Kind: StepRunner, Err: "不存在: HTTP 404", Fired: true})
	counts.absorb(TaskStepRecord{TaskID: 548, Kind: StepClick, Err: "任务已过期", Fired: true})
	counts.absorb(TaskStepRecord{TaskID: 550, Kind: StepClick, Fired: true})
	counts.absorb(TaskStepRecord{TaskID: 120, Kind: StepBlockedByRule, Fired: false})
	if counts.Failed != 3 {
		t.Errorf("三条带错误的都算 Failed，实得 %+v", counts)
	}
	if counts.FailedNoShot != 1 {
		t.Errorf("只有 434 那条一枪没发，实得 %+v", counts)
	}
	if counts.Clicked != 1 {
		t.Errorf("成功那一发才进 Clicked，实得 %+v", counts)
	}
}
