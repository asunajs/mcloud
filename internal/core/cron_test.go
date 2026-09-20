package core

import (
	"strings"
	"testing"
	"time"

	// 内嵌 IANA 时区库，让夏令时那两条测试不依赖宿主机的 /usr/share/zoneinfo。
	// 只在测试二进制里生效，不影响生产构建体积。
	_ "time/tzdata"
)

func cronUTC(t *testing.T, text string) time.Time {
	t.Helper()
	parsed, err := time.Parse(time.RFC3339, text)
	if err != nil {
		t.Fatalf("测试自身的 RFC3339 写错了: %q: %v", text, err)
	}
	return parsed.UTC()
}

// 6 段：秒 分 时 日 月 周。
func TestCronSixFieldExpressionFiresAtTheNamedSecond(t *testing.T) {
	schedule, err := ParseCron("40 59 9 * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	// 2026-09-20 01:59:39 UTC = 09:59:39 +08:00 → 下一秒就是 09:59:40
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-20T01:59:39Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-20T01:59:40Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s", next, want)
	}
}

// 5 段：秒补 `0`，**不是**「每秒都跑」。
func TestCronFiveFieldExpressionMeansSecondZeroNotEverySecond(t *testing.T) {
	schedule, err := ParseCron("30 7 * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-19T20:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	// 07:30:00 +08:00 = 前一日 23:30:00 UTC
	if want := cronUTC(t, "2026-09-19T23:30:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s", next, want)
	}

	// 再往后一次必须是**次日**的 07:30，而不是同一分钟里的下一秒
	again, ok := schedule.NextAfter(cronUTC(t, "2026-09-19T23:30:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-20T23:30:00Z"); !again.Equal(want) {
		t.Fatalf("again = %s，want %s", again, want)
	}
}

// 两种段数都要能解析 —— 但也别把它们混为一谈。
func TestCronFiveAndSixFieldFormsAreBothAccepted(t *testing.T) {
	if _, err := ParseCron("30 7 * * *"); err != nil {
		t.Fatalf("5 段 = 分 时 日 月 周，应当合法: %v", err)
	}
	if _, err := ParseCron("0 30 7 * * *"); err != nil {
		t.Fatalf("6 段 = 秒 分 时 日 月 周，应当合法: %v", err)
	}
	// 5 段里「时」的位置写 30 就是非法的 —— 这条断言的价值在提醒两者顺序不同
	if _, err := ParseCron("0 30 7 * *"); err == nil {
		t.Fatal("5 段的时不能是 30，应当报错")
	}
}

// ⭐ 两种段数**语义真的不同**，而不只是「都能解析」。
//
// `30 7 * * *` 是 5 段（分=30 时=7）→ 每天 07:30:00；
// `30 7 * * * *` 是 6 段（秒=30 分=7）→ 每小时的 07:30 那一秒。
// 把这两条写成同一个期望值，正是「段数分流写错」时不会被发现的那种测试。
func TestCronFiveAndSixFieldFormsMeanDifferentThings(t *testing.T) {
	five, err := ParseCron("30 7 * * *")
	if err != nil {
		t.Fatalf("ParseCron(5 段): %v", err)
	}
	six, err := ParseCron("30 7 * * * *")
	if err != nil {
		t.Fatalf("ParseCron(6 段): %v", err)
	}
	from := cronUTC(t, "2026-09-19T20:00:00Z") // 北京 2026-09-20 04:00:00

	// 5 段：当天 07:30:00 +08:00
	got5, ok := five.NextAfter(from, LocalTZ())
	if !ok {
		t.Fatal("5 段应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-19T23:30:00Z"); !got5.Equal(want) {
		t.Fatalf("5 段 next = %s，want %s", got5, want)
	}

	// 6 段：04:07:30 +08:00（同一个小时里就到）
	got6, ok := six.NextAfter(from, LocalTZ())
	if !ok {
		t.Fatal("6 段应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-19T20:07:30Z"); !got6.Equal(want) {
		t.Fatalf("6 段 next = %s，want %s", got6, want)
	}
}

// 与旧项目生成口径对齐：HMSToCron 的输出必须能解析，且触发时刻就是那个时刻。
func TestCronHMSToCronRoundTrips(t *testing.T) {
	expr := HMSToCron(7, 30, 0)
	if expr != "0 30 7 * * *" {
		t.Fatalf("HMSToCron(7,30,0) = %q，want %q", expr, "0 30 7 * * *")
	}
	next, ok, err := NextFire(expr, TZName, cronUTC(t, "2026-09-19T23:00:00Z"))
	if err != nil {
		t.Fatalf("NextFire: %v", err)
	}
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-19T23:30:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s（07:30 +08:00）", next, want)
	}
}

func TestCronStepsRangesAndListsWork(t *testing.T) {
	// 每 15 秒
	every15s, err := ParseCron("*/15 * * * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok := every15s.NextAfter(cronUTC(t, "2026-09-20T00:00:16Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-20T00:00:30Z"); !next.Equal(want) {
		t.Fatalf("每 15 秒: next = %s，want %s", next, want)
	}

	// 工作时间每 10 分钟（9-18 点）
	work, err := ParseCron("0 */10 9-18 * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok = work.NextAfter(cronUTC(t, "2026-09-20T01:55:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-20T02:00:00Z"); !next.Equal(want) {
		t.Fatalf("工作时间(09:55 → 10:00): next = %s，want %s", next, want)
	}
	next, ok = work.NextAfter(cronUTC(t, "2026-09-20T10:59:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-21T01:00:00Z"); !next.Equal(want) {
		t.Fatalf("工作时间(18:59 后跨到次日 09:00): next = %s，want %s", next, want)
	}

	// 列表
	listed, err := ParseCron("0 0 8,12,20 * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok = listed.NextAfter(cronUTC(t, "2026-09-20T04:30:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-20T12:00:00Z"); !next.Equal(want) {
		t.Fatalf("列表(12:30 +08:00 → 20:00): next = %s，want %s", next, want)
	}
}

// 周日既能写 `0` 也能写 `7`。
func TestCronSeventhWeekdayIsAlsoSunday(t *testing.T) {
	zero, err := ParseCron("0 0 9 * * 0")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	seven, err := ParseCron("0 0 9 * * 7")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	from := cronUTC(t, "2026-09-20T00:00:00Z") // 2026-09-20 是周日
	gotZero, okZero := zero.NextAfter(from, LocalTZ())
	gotSeven, okSeven := seven.NextAfter(from, LocalTZ())
	if okZero != okSeven {
		t.Fatalf("两条表达式的可触发性应当一致: 0→%v, 7→%v", okZero, okSeven)
	}
	if !okZero {
		t.Fatal("应当有下次触发时刻")
	}
	if !gotZero.Equal(gotSeven) {
		t.Fatalf("0 与 7 都表示周日，结果应当相同: %s vs %s", gotZero, gotSeven)
	}
}

// `*` 在周字段里表示「不限制」—— 周一到周日每一天都该命中。
//
// 这条不是废话：weekdays 字段的 max 是 7（为了容下 POSIX 的 `7`），
// 而 time.Weekday() 只产出 0..6。coversAll 是拿 min..max 逐个查位图的，
// 所以它必须真的覆盖 0..7 才算「没被限制」。改错一个边界，
// `0 0 9 * * *` 就会退化成「只在周日跑」—— 而它看起来仍然会触发，只是少跑六天。
func TestCronStarWeekdayMeansEveryDay(t *testing.T) {
	schedule, err := ParseCron("0 0 9 * * *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	if !schedule.weekdays.coversAll() {
		t.Fatal("`*` 应当覆盖整个周字段（含 POSIX 的 7）")
	}
	// 2026-09-23 是周三。周这一维没被限制 → 应当命中周三 09:00 +08:00。
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-23T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-23T01:00:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s（周三 09:00 +08:00）", next, want)
	}
}

// `7` 归一化成 `0` 之后：既要在周日触发，也**只**在周日触发。
//
// Rust 侧那条测试只断言了「0 与 7 两条表达式结果相同」—— 两条都错成
// 「永不触发」时它同样成立。这里补上「非周日不触发」，
// 才能把归一化与「7 会不会顺手把周这一维变成不限制」一起钉住。
func TestCronSundaySevenFiresOnSundayAndOnlyOnSunday(t *testing.T) {
	schedule, err := ParseCron("0 0 9 * * 7")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	if schedule.weekdays.coversAll() {
		t.Fatal("只写 `7` 时周这一维是**被限制**的，coversAll 不应当为真")
	}
	// 2026-09-23 是周三 → 下一次应当是 09-27（周日）09:00 +08:00
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-23T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-27T01:00:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s（周日 09:00 +08:00）", next, want)
	}
	// 反向确认：落在周日，而不是「随便哪一天」
	if wd := next.In(LocalTZ()).Weekday(); wd != time.Sunday {
		t.Fatalf("落点应当是周日，实际 %v", wd)
	}
}

// 日与周同时被限制 → **OR**（那条反直觉但必须照抄的语义）。
func TestCronDayAndWeekdayAreOredWhenBothRestricted(t *testing.T) {
	// 每月 1 号 **或** 每周一，10:00（+08:00）
	schedule, err := ParseCron("0 0 10 1 * 1")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}

	// 2026-09-21 是周一：应当命中
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-20T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-21T02:00:00Z"); !next.Equal(want) {
		t.Fatalf("周一的 10:00: next = %s，want %s", next, want)
	}

	// 2026-09-29 是周二：下一个命中的是 **10-01（1 号，周四）** ——
	// 注意别把这条写成「下一个周一的 10:00」：周一先到就会先命中（见上一条）
	next, ok = schedule.NextAfter(cronUTC(t, "2026-09-29T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-10-01T02:00:00Z"); !next.Equal(want) {
		t.Fatalf("1 号的 10:00: next = %s，want %s", next, want)
	}
}

// 时区是**参数**，不是写死的默认值。
func TestCronTheTimezoneActuallyShiftsTheAnswer(t *testing.T) {
	const expr = "0 0 9 * * *"
	from := cronUTC(t, "2026-09-20T00:00:00Z")

	inShanghai, ok, err := NextFire(expr, "Asia/Shanghai", from)
	if err != nil || !ok {
		t.Fatalf("NextFire(Asia/Shanghai): ok=%v err=%v", ok, err)
	}
	inUTC, ok, err := NextFire(expr, "UTC", from)
	if err != nil || !ok {
		t.Fatalf("NextFire(UTC): ok=%v err=%v", ok, err)
	}
	if want := cronUTC(t, "2026-09-20T01:00:00Z"); !inShanghai.Equal(want) {
		t.Fatalf("09:00 +08:00: got %s，want %s", inShanghai, want)
	}
	if want := cronUTC(t, "2026-09-20T09:00:00Z"); !inUTC.Equal(want) {
		t.Fatalf("09:00 UTC: got %s，want %s", inUTC, want)
	}
}

// 未知时区要报错，而不是静默退回 UTC。
//
// 静默退回的后果很具体：用户写错时区，任务照样跑，只是跑在了另一个时刻上 ——
// 没有人会去看那一眼日志。
func TestCronUnknownTimezoneIsRejected(t *testing.T) {
	_, _, err := NextFire("0 0 9 * * *", "Asia/Shanghaii", cronUTC(t, "2026-09-20T00:00:00Z"))
	if err == nil {
		t.Fatal("未知时区应当报错")
	}
	if !strings.Contains(err.Error(), "未知时区") {
		t.Fatalf("报错应当点明是时区问题，实际: %v", err)
	}
}

// 不可能成立的表达式返回 false，而不是死循环。
func TestCronAnImpossibleExpressionReturnsFalseInsteadOfLooping(t *testing.T) {
	schedule, err := ParseCron("0 0 0 30 2 *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	if _, ok := schedule.NextAfter(cronUTC(t, "2026-09-20T00:00:00Z"), LocalTZ()); ok {
		t.Fatal("2 月 30 日永远不会触发，应当返回 false")
	}
}

func TestCronBadExpressionsAreRejectedWithActionableMessages(t *testing.T) {
	cases := []struct{ expr, want string }{
		{"", "段"},
		{"* * * *", "段"},
		{"* * * * * * *", "段"},
		{"60 * * * * *", "超出范围"}, // 秒
		{"* * 24 * * *", "超出范围"}, // 时（6 段里第 3 个才是「时」）
		{"* * * 32 * *", "超出范围"},
		{"* * * * 13 *", "超出范围"},
		{"* * * * * 8", "超出范围"},
		{"*/0 * * * * *", "不能是 0"},
		{"a * * * * *", "不是数字"},
		{"5-1 * * * * *", "起点大于终点"},
		{"1--5 * * * * *", "不合法"},
		{"*/x * * * * *", "步长不是数字"},
	}
	for _, c := range cases {
		_, err := ParseCron(c.expr)
		if err == nil {
			t.Errorf("%q 应当被拒绝", c.expr)
			continue
		}
		if !strings.Contains(err.Error(), c.want) {
			t.Errorf("%q 的报错应包含 %q，实际: %v", c.expr, c.want, err)
		}
	}
}

// 跨年要能算过去（4 年上界不是为了兜住「每年 12 月 31 日」这类正常需求）。
func TestCronYearlyExpressionCrossesTheYearBoundary(t *testing.T) {
	schedule, err := ParseCron("0 0 0 1 1 *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok := schedule.NextAfter(cronUTC(t, "2026-03-01T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	// 从 2026-03-01 起，下一次是 2027-01-01 00:00 +08:00
	if want := cronUTC(t, "2026-12-31T16:00:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s", next, want)
	}
}

// 2 月 29 日：闰年才有的那一天必须算得对。
func TestCronLeapDayIsHandled(t *testing.T) {
	schedule, err := ParseCron("0 0 0 29 2 *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	next, ok := schedule.NextAfter(cronUTC(t, "2026-03-01T00:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2028-02-28T16:00:00Z"); !next.Equal(want) {
		t.Fatalf("2028 是闰年: next = %s，want %s", next, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Go 侧新增：两条 Rust 原测试没覆盖、但换语言后**新长出来**的风险
// ─────────────────────────────────────────────────────────────────────────────

// ⭐ 步长极大时不许回绕。
//
// Rust 原写法是 `value += step`（u32）。`end` 最大 59 而 `step` 来自用户输入，
// `59 + 4294967295` 在 u32 里回绕成 58 —— 于是循环继续、一路减到 0 才停。
// 位图去重让**结果**恰好相同，所以这条 bug 在结果上不可见；它在 Rust debug 构建下
// 表现为 panic（attempt to add with overflow），在 release 下表现为白跑 step 量级的循环。
// Go 无溢出 panic，只会静默回绕 —— 所以这里必须有一条**测得到**的断言。
func TestCronHugeStepDoesNotWrapAround(t *testing.T) {
	done := make(chan struct{})
	go func() {
		defer close(done)
		schedule, err := ParseCron("*/4294967295 * * * * *")
		if err != nil {
			t.Errorf("超大但合法的步长不应当报错: %v", err)
			return
		}
		// 秒这一维只该命中 0 一个值：从 0 起、步长超过上界，只取起点。
		if got := schedule.seconds.values(); len(got) != 1 || got[0] != 0 {
			t.Errorf("秒字段应当只命中 {0}，实际 %v", got)
		}
		// 直接验行为：从 00:00:00 起，下一次是下一分钟的 00 秒
		next, ok := schedule.NextAfter(cronUTC(t, "2026-09-20T00:00:00Z"), LocalTZ())
		if !ok {
			t.Error("应当有下次触发时刻")
			return
		}
		if want := cronUTC(t, "2026-09-20T00:01:00Z"); !next.Equal(want) {
			t.Errorf("next = %s，want %s", next, want)
		}
	}()

	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("步长回绕导致循环没有终止（这正是 Rust 侧那条写法的 release 行为）")
	}
}

// `+5` 这类前导加号：Rust 的 u32::from_str 接受，Go 的 ParseUint 不接受。
// 表达式来自用户配置，「同一份配置两端一个合法一个报错」是最该避免的差异。
func TestCronLeadingPlusIsAcceptedLikeRust(t *testing.T) {
	schedule, err := ParseCron("0 +30 7 * * *")
	if err != nil {
		t.Fatalf("Rust 侧接受前导 +，Go 侧也应当接受: %v", err)
	}
	next, ok := schedule.NextAfter(cronUTC(t, "2026-09-19T23:00:00Z"), LocalTZ())
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	if want := cronUTC(t, "2026-09-19T23:30:00Z"); !next.Equal(want) {
		t.Fatalf("next = %s，want %s", next, want)
	}
}

// ⭐ 夏令时「跳过」的那一小时里没有这个本地时刻 —— 必须跳过，而不是被 Go 归一化到邻点。
//
// Go 的 time.Date 不报错，而是把不存在的墙钟时间归一化到相邻时刻。
// 不校验就等于凭空多跑一次（用户看到的执行时间还不是他写的那一个）。
// 这里用 America/New_York：2026-03-08 的 02:30 EST 不存在（02:00 直接跳到 03:00）。
func TestCronSkipsTheHourThatDaylightSavingTimeSkips(t *testing.T) {
	schedule, err := ParseCron("0 30 2 8 3 *")
	if err != nil {
		t.Fatalf("ParseCron: %v", err)
	}
	ny, err := time.LoadLocation("America/New_York")
	if err != nil {
		t.Fatalf("加载 America/New_York: %v", err)
	}

	next, ok := schedule.NextAfter(cronUTC(t, "2026-01-01T00:00:00Z"), ny)
	if !ok {
		t.Fatal("应当有下次触发时刻")
	}
	// 2026-03-08 02:30 不存在 → 跳过那一天，落到 2027-03-08 02:30 EST（UTC-5）
	if want := cronUTC(t, "2027-03-08T07:30:00Z"); !next.Equal(want) {
		t.Fatalf("应当跳过夏令时缺口落到 2027-03-08T07:30Z，实际 %s", next)
	}
	// 反向确认：这一天在纽约确实是 02:30（而不是被归一化后的 03:30）
	if h, m := next.In(ny).Hour(), next.In(ny).Minute(); h != 2 || m != 30 {
		t.Fatalf("落点应当是纽约本地 02:30，实际 %02d:%02d", h, m)
	}
}
