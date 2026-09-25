package app

// [CloudSnapshotResult.SummaryLine] 那一句话的三档。
//
// # 为什么单独钉这一族
//
// 这句话会进 `runs.message`，也就是**运行历史列表里用户读到的那一格**。
// 它最容易出的错不是崩，而是把"没有基准"念成"日增 0"：
// 表现是一个刚加进来的号，历史里写着"今天没涨"，而事实是"今天才有第一条数"。
// 两种读法导出的动作完全不同（前者要查签到为什么没给豆，后者什么都不用做）。
//
// 三档必须各有各的样子：
//  1. `Stats == nil`（没写进库）⇒ 只报余额，不编造增量；
//  2. 增量是 nil（库里没有**往日**记录可比）⇒ 明说"暂无往日记录"；
//  3. 增量是数（**包括 0**）⇒ 照数念，0 就念 `+0`。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

func TestCloudSnapshotSummaryLineWithoutStatsReportsOnlyBalance(t *testing.T) {
	line := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 8708, Stats: nil,
	}.SummaryLine()
	if line != "余额 8708 豆" {
		t.Fatalf("没写进库时只许报余额原值，实得 %q", line)
	}
	if strings.Contains(line, "增") {
		t.Fatalf("这一档不许出现任何增量表述（库里没有这一格）：%q", line)
	}
}

func TestCloudSnapshotSummaryLineOnFirstCollectionSaysNoBaseline(t *testing.T) {
	// 第一次采集：[core.NextCloudStats] 交回的是两个 nil
	first := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-20", 8708, nil)
	if first.CloudDiff != nil {
		t.Fatalf("前置条件坏了：首次采集的日增量应是 nil，实得 %s", deltaText(first.CloudDiff))
	}

	line := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 8708, Stats: &first,
	}.SummaryLine()

	// ⚠ 反向改坏它的方式：把 nil 当 0 念（`%+d` 配一个 0）⇒ 这里得到
	// "余额 8708 豆；日增 +0，周增 +0"，读起来是一句"今天没涨"。
	// 断言盯的是"**带符号的数字**"，不是"日增"三个字本身 ——
	// 没有基准那一档的话里也要提日增/周增（说的是它俩算不出来）。
	if strings.Contains(line, "日增 +") || strings.Contains(line, "日增 -") ||
		strings.Contains(line, "周增 +") || strings.Contains(line, "周增 -") {
		t.Fatalf("没有基准时不许念出增量数字：%q", line)
	}
	if !strings.Contains(line, "暂无往日记录") {
		t.Fatalf("应明说库里还没有可比的前一天记录，实得 %q", line)
	}
	if !strings.Contains(line, "8708") {
		t.Fatalf("余额那一格仍然要给（它是今天唯一可信的数），实得 %q", line)
	}
}

// TestCloudSnapshotSummaryLineOnSameDayRepeatDoesNotClaimFirstCollection：
// ⭐ 同一天里的第二次、第五次采集，也不许念成"第一次采集"。
//
// 2026-09-25 真账号那一趟撞出来的措辞问题：这一天里这条能力被跑了五次，
// 而 `history` 按 [core.NextCloudStats] 的契约**不含当天**那些行 ⇒ 每一次都走进
// "没有基准"这一支，于是第五次仍被念成"第一次"。那一支本身是对的
// （当天记录确实不能当自己的基准），错的只有措辞 —— 这就是它现在的话。
func TestCloudSnapshotSummaryLineOnSameDayRepeatDoesNotClaimFirstCollection(t *testing.T) {
	earlier := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-25", 7474, nil)
	// 同一天的第二趟：调用方交给它的是"往日"历史（空），不是今天那一行。
	again := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-25", 7534, nil)
	if again.CloudDiff != nil {
		t.Fatalf("前置条件坏了：没有可比记录，日增应是 nil，实得 %s", deltaText(again.CloudDiff))
	}

	line := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 7534, Stats: &again,
	}.SummaryLine()
	if strings.Contains(line, "第一次") {
		t.Fatalf("今天已经采过 %d 豆那一发了，不许再念成第一次采集：%q", earlier.CloudCount, line)
	}
	if !strings.Contains(line, "暂无往日记录") {
		t.Fatalf("要说清是「没有可比的前一天记录」，实得 %q", line)
	}
}

func TestCloudSnapshotSummaryLineReportsZeroAsAValue(t *testing.T) {
	first := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-19", 8708, nil)
	// 第二天余额一模一样 ⇒ 增量是**数** 0，不是"没有基准"
	second := core.NextCloudStats(
		core.NewAccountID("acc-1"), "2026-09-20", 8708, []core.CloudStats{first})
	if !deltaIs(second.CloudDiff, 0) {
		t.Fatalf("前置条件坏了：日增量应是 0，实得 %s", deltaText(second.CloudDiff))
	}

	line := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 8708, Stats: &second,
	}.SummaryLine()

	// 这一档反过来钉"0 不该被念成没有基准"：把 nil 判断写成 `*p == 0` 会红在这里。
	if strings.Contains(line, "暂无往日记录") {
		t.Fatalf("增量为 0 是有读数的，不该走「没有基准」那一支：%q", line)
	}
	if !strings.Contains(line, "日增 +0") || !strings.Contains(line, "周增 +0") {
		t.Fatalf("应照数念出 +0，实得 %q", line)
	}
}

func TestCloudSnapshotSummaryLineWithDeltas(t *testing.T) {
	first := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-19", 8708, nil)
	grown := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-20", 8711, []core.CloudStats{first})
	line := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 8711, Stats: &grown,
	}.SummaryLine()
	if !strings.Contains(line, "日增 +3") || !strings.Contains(line, "周增 +3") {
		t.Fatalf("实得 %q", line)
	}

	// 负数那一档：云朵被花掉时不能念成"没涨"，也不能丢符号。
	spent := core.NextCloudStats(core.NewAccountID("acc-1"), "2026-09-21", 8211,
		[]core.CloudStats{first, grown})
	spentLine := CloudSnapshotResult{
		Account: core.NewAccountID("acc-1"), Count: 8211, Stats: &spent,
	}.SummaryLine()
	if !strings.Contains(spentLine, "日增 -500") {
		t.Fatalf("花掉云朵要念成负数，实得 %q", spentLine)
	}
}
