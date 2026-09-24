// 纯函数层的测试 —— 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/pure.rs 的 `mod tests`。
//
// 这一层没有黄金向量（它是原 TS 项目里"藏在任务函数内部、只能靠真机试跑验证"
// 的逻辑，从来没有独立实现可供对拍），所以断言全部是**手写的边界值**。
// 手写不等于弱：下面每条都尽量选一个"写错了就会得到另一个数"的输入。
package core

import (
	"strconv"
	"strings"
	"testing"
	"time"
)

// ─────────────────────────────────────────────────────────────────────────────
// 时区 / 日键
// ─────────────────────────────────────────────────────────────────────────────

func TestLocalTZIsUTCPlusEight(t *testing.T) {
	// 中国自 1991 年起不实行夏令时 ⇒ 全年固定 +8。
	// 这条断言同时兜住"zoneinfo 读不到、退回 FixedZone"那条路径：
	// 两条路径的偏移必须一样，否则跨零点会算错一天。
	_, offset := time.Date(2026, 9, 16, 12, 0, 0, 0, LocalTZ()).Zone()
	if offset != 8*3600 {
		t.Fatalf("Asia/Shanghai 偏移应为 +8h，实际 %d 秒", offset)
	}
}

func TestDailyKeyUsesShanghaiTimezone(t *testing.T) {
	// UTC 2026-09-16T16:30Z == 上海 2026-09-17T00:30 → 属于"17 号"
	now := time.Date(2026, 9, 16, 16, 30, 0, 0, time.UTC)
	if got := DailyKey(now); got != "2026-09-17" {
		t.Fatalf("得到 %q，期望 2026-09-17", got)
	}
	// UTC 2026-09-16T15:30Z == 上海 2026-09-16T23:30 → 仍属于"16 号"
	now = time.Date(2026, 9, 16, 15, 30, 0, 0, time.UTC)
	if got := DailyKey(now); got != "2026-09-16" {
		t.Fatalf("得到 %q，期望 2026-09-16", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 云朵快照增量
// ─────────────────────────────────────────────────────────────────────────────

func stats(date string, count int64) CloudStats {
	return CloudStats{AccountID: "acc-1", Date: date, CloudCount: count}
}

// deltaIs 断言一个**可选**增量等于 want。
//
// ⚠ 为什么不直接写 `*got == want`：这一族最典型的错法是"把没有基准悄悄落成 0"，
// 而 nil 与 0 在界面上是两句话（前者"这个号才第一次采到数"，后者"今天没涨"）。
// 所以 nil 必须**单独报错**，见 [deltaText]。
func deltaIs(got *int64, want int64) bool { return got != nil && *got == want }

// deltaText 把可选增量打印成"看得懂是哪一种空"。
func deltaText(got *int64) string {
	if got == nil {
		return "nil（没有基准）"
	}
	return strconv.FormatInt(*got, 10)
}

func TestNoHistoryLeavesDeltasNilNotZero(t *testing.T) {
	s := NextCloudStats("acc-1", "2026-09-16", 1286, nil)
	if s.CloudCount != 1286 {
		t.Fatalf("余额应原样带上：%d", s.CloudCount)
	}
	// ⚠ 这一条钉的是"首次采集**不写 0**"。写成 0 的话这里红：
	// 0 会被界面念成"今天没涨"，而事实是"还没有可减的基准"。
	if s.CloudDiff != nil {
		t.Fatalf("首次采集的日增量必须是 nil，实际 %s", deltaText(s.CloudDiff))
	}
	if s.CloudDiffWeek != nil {
		t.Fatalf("首次采集的周增量必须是 nil，实际 %s", deltaText(s.CloudDiffWeek))
	}
}

func TestDiffIsAgainstPreviousRecordedDay(t *testing.T) {
	history := []CloudStats{stats("2026-09-14", 1274), stats("2026-09-15", 1278)}
	s := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if !deltaIs(s.CloudDiff, 8) {
		t.Fatalf("应取最近一条 1278 作为基准，实际 %s", deltaText(s.CloudDiff))
	}
}

func TestDiffCanBeNegativeAfterSpendingClouds(t *testing.T) {
	history := []CloudStats{stats("2026-09-15", 1500)}
	s := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if !deltaIs(s.CloudDiff, -214) {
		t.Fatalf("兑换消耗后余额下降应记为负数，实际 %s", deltaText(s.CloudDiff))
	}
}

func TestZeroDiffIsAValueNotAnAbsentBaseline(t *testing.T) {
	// 余额没变 ⇒ 增量是**数** 0，不是 nil。反向：把"相等"判成 nil 会在这里红。
	history := []CloudStats{stats("2026-09-15", 1286)}
	s := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if s.CloudDiff == nil {
		t.Fatalf("余额相同时增量应是 0 而不是 nil")
	}
	if *s.CloudDiff != 0 {
		t.Fatalf("实得 %s", deltaText(s.CloudDiff))
	}
}

func TestWeekDiffPrefersRecordAtLeastSevenDaysOld(t *testing.T) {
	history := []CloudStats{
		stats("2026-09-08", 1200), // 距 09-16 第 8 天 → 命中 7 天线
		stats("2026-09-10", 1250),
		stats("2026-09-15", 1278),
	}
	s := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if !deltaIs(s.CloudDiff, 8) {
		t.Fatalf("日增量应取最近一条 1278：%s", deltaText(s.CloudDiff))
	}
	if !deltaIs(s.CloudDiffWeek, 86) {
		t.Fatalf("周增量基准应为 09-08 的 1200（得 86），实际 %s", deltaText(s.CloudDiffWeek))
	}
}

func TestWeekDiffDegradesToEarliestWhenHistoryIsShort(t *testing.T) {
	history := []CloudStats{stats("2026-09-14", 1274), stats("2026-09-15", 1278)}
	s := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if !deltaIs(s.CloudDiffWeek, 12) {
		t.Fatalf("历史不足 7 天时退化为最早一条之差（得 12），实际 %s", deltaText(s.CloudDiffWeek))
	}
}

func TestExactSevenDaysOldCountsAsWeekBase(t *testing.T) {
	history := []CloudStats{stats("2026-09-09", 1000), stats("2026-09-15", 1278)}
	s := NextCloudStats("acc-1", "2026-09-16", 1100, history)
	if !deltaIs(s.CloudDiffWeek, 100) {
		t.Fatalf("09-09 恰好是 7 天前，应作为周增量基准（得 100），实际 %s", deltaText(s.CloudDiffWeek))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 增量那一小句话（CLI 的"余额已记录"与服务端运行历史共用）
// ─────────────────────────────────────────────────────────────────────────────
//
// ⚠ 这一族的断言全部写成**整串相等**，不是 `Contains`。
// 理由就在这次踩的坑上：把 `*int64` 直接递进 `%+d` 时，nil 打 `+0`、有值打**地址**
// （实测 `+126914284740856`），两种都是"看起来像一句正常话"的输出；
// 只断关键词的测试（历史上 CLI 只断"余额已记录：10502 豆"）对它们完全无感。

func TestDiffClauseOnFirstCollectionSaysNoBaseline(t *testing.T) {
	first := NextCloudStats("acc-1", "2026-09-16", 1286, nil)
	if got := first.DiffClause(); got != "第一次采集，暂无增量基准" {
		t.Fatalf("实得 %q", got)
	}
}

func TestDiffClauseKeepsZeroAsAReading(t *testing.T) {
	history := []CloudStats{stats("2026-09-15", 1286)}
	same := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if got := same.DiffClause(); got != "日增 +0，周增 +0" {
		t.Fatalf("余额没变该念 +0（不是「没有基准」），实得 %q", got)
	}
}

func TestDiffClauseSignsBothDirections(t *testing.T) {
	history := []CloudStats{stats("2026-09-08", 1200), stats("2026-09-15", 1278)}
	grown := NextCloudStats("acc-1", "2026-09-16", 1286, history)
	if got := grown.DiffClause(); got != "日增 +8，周增 +86" {
		t.Fatalf("实得 %q", got)
	}

	spent := NextCloudStats("acc-1", "2026-09-17", 772, []CloudStats{stats("2026-09-16", 1286)})
	if got := spent.DiffClause(); got != "日增 -514，周增 -514" {
		t.Fatalf("兑换花掉后两个增量都该带负号，实得 %q", got)
	}
}

// 只有其中一个基准缺失时（现实里不会由 [NextCloudStats] 产出，但结构体允许），
// 也不许把缺的那一个念成 0。
func TestDiffClauseGuardsEveryMissingBaseline(t *testing.T) {
	one, week := int64(3), int64(9)
	partial := CloudStats{AccountID: "acc-1", Date: "2026-09-16", CloudCount: 1289,
		CloudDiff: &one, CloudDiffWeek: nil}
	if got := partial.DiffClause(); got != "第一次采集，暂无增量基准" {
		t.Fatalf("周基准缺失时不许只念日增，实得 %q", got)
	}
	// 反向：`CloudDiff: nil, CloudDiffWeek: &week` 同一条分支，改坏判据（`&&`）会红在这里。
	other := CloudStats{AccountID: "acc-1", Date: "2026-09-16", CloudCount: 1289,
		CloudDiff: nil, CloudDiffWeek: &week}
	if got := other.DiffClause(); got != "第一次采集，暂无增量基准" {
		t.Fatalf("日基准缺失时同样不许念数字，实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 余额取值 / 时间纠偏
// ─────────────────────────────────────────────────────────────────────────────

func TestPickCloudValueSkipsEmptySources(t *testing.T) {
	one, two, zero := int64(1286), int64(1), int64(0)
	cases := []struct {
		name    string
		sources []*int64
		want    int64
		ok      bool
	}{
		{"skip-nil", []*int64{nil, &one, &two}, 1286, true},
		{"all-nil", []*int64{nil, nil}, 0, false},
		{"empty", nil, 0, false},
		// ⚠ 0 是**合法余额**，不能被当成"空"。这条是本函数的判据所在：
		// 用"值是不是真"做判断（而不是"来源在不在"）会在这里失败。
		{"zero-is-a-real-value", []*int64{&zero}, 0, true},
	}
	for _, c := range cases {
		got, ok := PickCloudValue(c.sources)
		if ok != c.ok || (ok && got != c.want) {
			t.Fatalf("%s：得到 (%d, %v)，期望 (%d, %v)", c.name, got, ok, c.want, c.ok)
		}
	}
}

func TestServerOffsetPositiveWhenServerAhead(t *testing.T) {
	if got := ServerOffsetMS(1_000, 3_500); got != 2_500 {
		t.Fatalf("得到 %d，期望 2500", got)
	}
	if got := ServerOffsetMS(1_000, 1_000); got != 0 {
		t.Fatalf("得到 %d，期望 0", got)
	}
	if got := ServerOffsetMS(3_500, 1_000); got != -2_500 {
		t.Fatalf("得到 %d，期望 -2500", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 时刻
// ─────────────────────────────────────────────────────────────────────────────

func TestCronMappingMatchesExistingScheduleSemantics(t *testing.T) {
	cases := []struct {
		h, m, s int
		want    string
	}{
		{9, 59, 40, "40 59 9 * * *"},
		{11, 59, 40, "40 59 11 * * *"},
		{0, 0, 0, "0 0 0 * * *"},
	}
	for _, c := range cases {
		if got := HMSToCron(c.h, c.m, c.s); got != c.want {
			t.Fatalf("HMSToCron(%d,%d,%d) 得到 %q，期望 %q", c.h, c.m, c.s, got, c.want)
		}
	}
}

func TestInvalidHMSIsRejected(t *testing.T) {
	if !IsValidHMS(23, 59, 59) {
		t.Fatal("23:59:59 应合法")
	}
	for _, c := range [][3]int{{24, 0, 0}, {0, 60, 0}, {0, 0, 60}, {-1, 0, 0}} {
		if IsValidHMS(c[0], c[1], c[2]) {
			t.Fatalf("%v 应被判非法", c)
		}
	}
}

func TestMsUntilRollsToTomorrowWhenTimeHasPassed(t *testing.T) {
	tz := LocalTZ()
	// 上海 09:00:00，目标 09:59:40 → 今天还有 59 分 40 秒
	now := time.Date(2026, 9, 16, 9, 0, 0, 0, tz)
	if got, want := MsUntilNextHMS(now, 9, 59, 40), int64((59*60+40)*1000); got != want {
		t.Fatalf("得到 %d，期望 %d", got, want)
	}

	// 上海 10:00:00，目标 09:59:40 → 已过，指向明天（23h59m40s）
	now = time.Date(2026, 9, 16, 10, 0, 0, 0, tz)
	want := int64(((24*60-1)*60 + 40) * 1000)
	if got := MsUntilNextHMS(now, 9, 59, 40); got != want {
		t.Fatalf("得到 %d，期望 %d", got, want)
	}
}

func TestMsUntilExactTimePointsToTomorrow(t *testing.T) {
	tz := LocalTZ()
	now := time.Date(2026, 9, 16, 9, 59, 40, 0, tz)
	const oneDayMS = 24 * 60 * 60 * 1000
	if got := MsUntilNextHMS(now, 9, 59, 40); got != oneDayMS {
		t.Fatalf("恰好等于目标时刻应指向明天（避免立即触发），得到 %d", got)
	}
}

// 非法时刻返回 0（"立刻"）—— 与 Rust 侧 `with_hour(24) → None → 0` 同义。
// 校验是调用方的责任，所以这条要显式钉住，免得有人把 0 解读成"算错了"。
func TestMsUntilInvalidTimeIsZero(t *testing.T) {
	now := time.Date(2026, 9, 16, 9, 0, 0, 0, LocalTZ())
	if got := MsUntilNextHMS(now, 24, 0, 0); got != 0 {
		t.Fatalf("非法时刻应返回 0（立刻），得到 %d", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 脱敏
// ─────────────────────────────────────────────────────────────────────────────

func TestMaskPhoneFollowsCLIConvention(t *testing.T) {
	if got := MaskPhone("19900000001"); got != "19****01" {
		t.Fatalf("得到 %q，期望 19****01", got)
	}
	if got := MaskPhone("账号2"); got != "账号2" {
		t.Fatalf("非纯数字应原样返回，得到 %q", got)
	}
	if got := MaskPhone("123456"); got != "123456" {
		t.Fatalf("过短不脱敏，得到 %q", got)
	}
}

func TestMaskPhoneNeverRevealsTheMiddle(t *testing.T) {
	masked := MaskPhone("13812345678")
	if strings.Contains(masked, "81234") {
		t.Fatalf("中间段必须被遮蔽：%q", masked)
	}
	if masked != "13****78" {
		t.Fatalf("得到 %q，期望 13****78", masked)
	}
}

// ⚠ 判据是"**至少 7 位且全数字**"，不是"长度等于 11"。
// 写成 `len == 11` 会让 12 位及以上的号码**原样泄漏** —— 而这是脱敏函数，
// 泄漏是它唯一不能犯的错。
func TestMaskPhoneHandlesNonElevenDigitNumbers(t *testing.T) {
	if got := MaskPhone("138123456789"); got != "13****89" {
		t.Fatalf("12 位号码也必须脱敏，得到 %q", got)
	}
	// 恰好 7 位是下界：6 位不脱敏、7 位脱敏
	if got := MaskPhone("1234567"); got != "12****67" {
		t.Fatalf("7 位应脱敏，得到 %q", got)
	}
	if got := MaskPhone("123456"); got != "123456" {
		t.Fatalf("6 位不脱敏，得到 %q", got)
	}
}
