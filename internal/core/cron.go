// cron 表达式：解析 + 「下次什么时候触发」。
// 对应 Rust 侧 legacy/crates/mcloud-core/src/cron.rs。
//
// # 为什么自己写，而不是引一个库
//
//   - **语义要跟旧项目对齐**：它是 6 段（含秒），HMSToCron 生成的就是 6 段；
//   - **需要的子集很小**：`*` / 数字 / 范围 / 步长 / 列表，没有名字（JAN/MON）也没有 @daily；
//   - 这是个纯函数，测试能把它钉死。而它算错一次的后果很具体：**漏跑或重跑** ——
//     前者让人以为任务已经跑过了，后者会重复领奖。
//
// # 段数：5 段或 6 段
//
//   - 6 段：秒 分 时 日 月 周（旧项目与 HMSToCron 用的形态）
//   - 5 段：分 时 日 月 周（POSIX 形态），秒按 `0` 补齐
//
// 段数不同**不算错**，但必须知道 `0 30 7 * * *` 与 `0 30 7 * *` 不是一回事：
// 前者是「07:30:00」，后者也是「07:30:00」（秒补 0）—— 真正的差别在
// `30 7 * * *`（5 段，07:30）与 `30 7 * * * *`（6 段，每分钟的第 30 秒）上。
// 所以解析时按段数明确分流。
//
// # 两条容易踩的语义
//
//  1. **日与周同时被限制时是 OR**（Vixie cron）：`0 0 0 1 * 1` = 每月 1 号**或**每周一。
//     反直觉但必须照抄 —— 用户与旧项目都按这个语义写表达式。
//  2. **周日的写法有两种**：`0` 与 `7` 都表示周日（POSIX 如此）。解析时归一化，
//     否则 `0 0 9 * * 7` 会永远不触发。
//
// # 时区
//
// 计算在**调用方指定的时区**里进行（Schedule.timezone，默认 Asia/Shanghai）。
// 把默认值写死在算法里，将来多时区就要改核心逻辑；把它留给调用方，多时区只是多传一个参数。
//
// # 与 Rust 侧的两处刻意保留的差异
//
//  1. Rust 的 `{:?}`（Debug 格式）与 Go 的 strconv.Quote 对**非 ASCII 可打印字符**
//     处理不同：Rust 会把 `中` 转义成 `\u{4e2d}` 之外的形态而 Go 原样保留。
//     本模块的报错里带引号的只有**用户输入的表达式片段**，测试全部用 ASCII 片段，
//     两者输出逐字相同。若将来要报含中文的表达式，这条差异会显形。
//  2. Rust 的 `u32::from_str` **接受**前导 `+`（`"+5"` → 5），Go 的 strconv.ParseUint
//     不接受。这里显式剥掉一个前导 `+` 以保持等价。
package core

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// searchLimitDays 是搜索「下次触发」的时间上界（天）。
//
// `0 0 0 30 2 *`（2 月 30 日）这类表达式**永远不成立**：没有上界就是死循环。
// 4 年覆盖闰年周期，过了就返回 false —— 那表示「这个表达式不会触发」，
// 而不是「我算不出来」。
const searchLimitDays = 366 * 4

// cronField 是一个 cron 字段（位图）。
//
// 用位图而不是切片：判断「这个值在不在集合里」是每次推进都要做的事，
// 而位图让它是**一次按位与**；字段上界最大 59，uint64 装得下。
type cronField struct {
	bits     uint64
	min, max uint32
}

func newCronField(min, max uint32) cronField {
	return cronField{min: min, max: max}
}

func (f *cronField) set(value uint32, name string) error {
	if value < f.min || value > f.max {
		return NewBadRequest(fmt.Sprintf(
			"cron 的%s字段取值 %d 超出范围 %d~%d", name, value, f.min, f.max))
	}
	f.bits |= 1 << value
	return nil
}

func (f *cronField) contains(value uint32) bool {
	return f.bits&(1<<value) != 0
}

func (f *cronField) isEmpty() bool { return f.bits == 0 }

// coversAll 判断是否覆盖了整段（`*` 或等价的写法）。
//
// 日/周的 OR 语义要靠它判断「这一维到底被限定了没有」。
func (f *cronField) coversAll() bool {
	for v := f.min; v <= f.max; v++ {
		if !f.contains(v) {
			return false
		}
	}
	return true
}

// values 升序列出命中的值。
//
// 升序很重要：timeOn 靠「第一个大于 after 的候选」来保证最早。
func (f *cronField) values() []uint32 {
	out := make([]uint32, 0, f.max-f.min+1)
	for v := f.min; v <= f.max; v++ {
		if f.contains(v) {
			out = append(out, v)
		}
	}
	return out
}

func (f *cronField) parse(text, name string) error {
	if text == "" {
		return NewBadRequest("cron 的" + name + "字段为空")
	}
	for _, term := range strings.Split(text, ",") {
		if err := f.parseTerm(term, name); err != nil {
			return err
		}
	}
	if f.isEmpty() {
		return NewBadRequest(fmt.Sprintf(
			"cron 的%s字段没有任何有效取值: %s", name, strconv.Quote(text)))
	}
	return nil
}

// parseTerm 处理单个项：`*` / `a` / `a-b` / `*/n` / `a-b/n` / `a/n`。
func (f *cronField) parseTerm(term, name string) error {
	rangeText := term
	step := uint32(1)

	if r, s, ok := strings.Cut(term, "/"); ok {
		rangeText = r
		parsed, err := parseCronUint(s)
		if err != nil {
			return NewBadRequest(fmt.Sprintf(
				"cron 的%s字段步长不是数字: %s", name, strconv.Quote(term)))
		}
		if parsed == 0 {
			return NewBadRequest(fmt.Sprintf(
				"cron 的%s字段步长不能是 0: %s", name, strconv.Quote(term)))
		}
		step = parsed
	}

	var start, end uint32
	switch {
	case rangeText == "*":
		start, end = f.min, f.max
	default:
		if from, to, ok := strings.Cut(rangeText, "-"); ok {
			// `1--5` 这种：切一次还剩 `-`，说明写法不合法。
			// 注意这里必须查 `to`（Rust 侧查的也是 `to`）——
			// 查 `rangeText` 会把合法的 `5-1` 也判成「不合法」，
			// 而 `5-1` 应该报的是「起点大于终点」。
			if strings.Contains(to, "-") {
				return NewBadRequest(fmt.Sprintf(
					"cron 的%s字段范围写法不合法: %s", name, strconv.Quote(term)))
			}
			fromNum, err := f.number(from, name)
			if err != nil {
				return err
			}
			toNum, err := f.number(to, name)
			if err != nil {
				return err
			}
			start, end = fromNum, toNum
		} else {
			single, err := f.number(rangeText, name)
			if err != nil {
				return err
			}
			// `a/n` = 从 a 起按步长取到上界；单独的 `a` = 只取 a 这一个值
			if step == 1 && !strings.Contains(term, "/") {
				start, end = single, single
			} else {
				start, end = single, f.max
			}
		}
	}

	if start > end {
		return NewBadRequest(fmt.Sprintf(
			"cron 的%s字段范围起点大于终点: %s", name, strconv.Quote(term)))
	}

	// ⚠ 这里不能用 `for value := start; value <= end; value += step`。
	// 字段上界最大 59，而 step 来自用户输入（`*/4294967295`），
	// `value + step` 会**回绕**成一个小数，循环于是继续跑 ——
	// 结果虽然因为位图去重而相同，但循环次数变成 step 量级。
	// （Rust 侧同样的写法在 debug 构建下直接 panic: attempt to add with overflow。）
	// 先判「再加一步就超上界」再自增，就完全绕开了回绕。
	for value := start; value <= end; {
		if err := f.set(value, name); err != nil {
			return err
		}
		if step > end-value {
			break
		}
		value += step
	}
	return nil
}

func (f *cronField) number(text, name string) (uint32, error) {
	value, err := parseCronUint(strings.TrimSpace(text))
	if err != nil {
		return 0, NewBadRequest(fmt.Sprintf(
			"cron 的%s字段不是数字: %s（本实现不支持 JAN/MON 这类名字）",
			name, strconv.Quote(text)))
	}
	return value, nil
}

// parseCronUint 解析一个十进制非负整数。
//
// 剥掉一个前导 `+`：Rust 的 `u32::from_str` 接受它，Go 的 strconv.ParseUint 不接受。
// 不剥的话 `+5` 在两侧一个合法一个报错 —— 而表达式来自用户配置，
// 「同一份配置两端行为不同」正是最该避免的那种差异。
func parseCronUint(text string) (uint32, error) {
	text = strings.TrimPrefix(text, "+")
	if text == "" {
		return 0, fmt.Errorf("empty")
	}
	value, err := strconv.ParseUint(text, 10, 32)
	if err != nil {
		return 0, err
	}
	return uint32(value), nil
}

// CronSchedule 是解析好的 cron 表达式。
type CronSchedule struct {
	seconds, minutes, hours, days, months, weekdays cronField
}

// ParseCron 解析 5 段或 6 段表达式。
func ParseCron(expr string) (*CronSchedule, error) {
	parts := strings.Fields(expr)

	var sec, min, hour, day, month, weekday string
	switch len(parts) {
	case 6:
		sec, min, hour, day, month, weekday = parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]
	case 5:
		// 5 段：秒补齐为 `0`（而不是「每秒都行」）—— POSIX 里没有秒这一维，
		// 补 `*` 会让「每天 7:30」变成「7:30 那一分钟里跑 60 次」。
		sec = "0"
		min, hour, day, month, weekday = parts[0], parts[1], parts[2], parts[3], parts[4]
	default:
		return nil, NewBadRequest(fmt.Sprintf(
			"cron 必须是 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），实际 %d 段: %s",
			len(parts), strconv.Quote(expr)))
	}

	s := &CronSchedule{
		seconds:  newCronField(0, 59),
		minutes:  newCronField(0, 59),
		hours:    newCronField(0, 23),
		days:     newCronField(1, 31),
		months:   newCronField(1, 12),
		weekdays: newCronField(0, 7), // 周日有 0 与 7 两种写法（POSIX），上界放到 7
	}
	for _, step := range []struct {
		field *cronField
		text  string
		name  string
	}{
		{&s.seconds, sec, "秒"},
		{&s.minutes, min, "分"},
		{&s.hours, hour, "时"},
		{&s.days, day, "日"},
		{&s.months, month, "月"},
		{&s.weekdays, weekday, "周"},
	} {
		if err := step.field.parse(step.text, step.name); err != nil {
			return nil, err
		}
	}
	// 归一化：`7` 也当周日，否则 `0 0 9 * * 7` 永远不触发。
	// ⚠ 归一化之后 `coversAll()` 才判得对 —— 少了这一步，
	// `0 0 9 * * 0,7` 会被当成「周这一维被限制过」而走 OR 分支。
	if s.weekdays.contains(7) {
		if err := s.weekdays.set(0, "周"); err != nil {
			return nil, err
		}
	}
	return s, nil
}

// NextAfter 返回严格晚于 after 的下一个触发时刻（在 tz 时区里计算），以 UTC 表示。
//
// 第二个返回值为 false 表示**这个表达式不会触发**（如 2 月 30 日），而不是算不出来。
func (s *CronSchedule) NextAfter(after time.Time, tz *time.Location) (time.Time, bool) {
	local := after.In(tz)
	year, month, day := local.Date()
	date := time.Date(year, month, day, 0, 0, 0, 0, time.UTC)

	for i := 0; i < searchLimitDays; i++ {
		if s.dayMatches(date) {
			if hit, ok := s.timeOn(date, tz, after); ok {
				return hit, true
			}
		}
		date = date.AddDate(0, 0, 1)
	}
	return time.Time{}, false
}

// dayMatches 判断这一天的日期维度（月 + 日/周）是否命中。
func (s *CronSchedule) dayMatches(date time.Time) bool {
	if !s.months.contains(uint32(date.Month())) {
		return false
	}
	domRestricted := !s.days.coversAll()
	dowRestricted := !s.weekdays.coversAll()
	dom := s.days.contains(uint32(date.Day()))
	// Go 的 time.Weekday 就是 Sunday=0，与 num_days_from_sunday 一致。
	dow := s.weekdays.contains(uint32(date.Weekday()))

	// 见模块文档第 1 条：两者都被限制时是 **OR**
	switch {
	case domRestricted && dowRestricted:
		return dom || dow
	case domRestricted:
		return dom
	case dowRestricted:
		return dow
	default:
		return true
	}
}

// timeOn 返回这一天里第一个晚于 after 的时刻。
//
// ⚠ 日期以 **UTC 零点** 承载「哪一天」，只取它的 Y/M/D 三个字段；
// 真正的本地时刻由 time.Date(…, tz) 重新构造。用本地零点承载也可以，
// 但夏令时切换那天本地零点本身可能不存在 —— 用 UTC 承载就没有这个问题。
func (s *CronSchedule) timeOn(date time.Time, tz *time.Location, after time.Time) (time.Time, bool) {
	year, month, day := date.Date()
	for _, hour := range s.hours.values() {
		for _, minute := range s.minutes.values() {
			for _, second := range s.seconds.values() {
				local := time.Date(year, month, day,
					int(hour), int(minute), int(second), 0, tz)
				// 夏令时「跳过」的那一小时里没有这个本地时刻。
				// Go 的 time.Date 不会报错，而是把不存在的墙钟时间**归一化**到
				// 相邻时刻（例如 02:30 变成 03:30）—— 不校验就等于凭空多跑一次。
				// 校验方式是把墙钟读回来逐字段比对。
				ly, lm, ld := local.Date()
				if ly != year || lm != month || ld != day ||
					local.Hour() != int(hour) ||
					local.Minute() != int(minute) ||
					local.Second() != int(second) {
					continue
				}
				candidate := local.UTC()
				// 值都是升序枚举的，所以第一个大于 after 的就是最早的那个
				if candidate.After(after) {
					return candidate, true
				}
			}
		}
	}
	return time.Time{}, false
}

// NextFire 是 Schedule.cron + Schedule.timezone 的便捷入口。
//
// 单独抽出来是因为调度侧拿到的永远是**两个字符串**（都来自库里），
// 让每个调用点各写一遍「解析表达式 + 解析时区」只会把这两步的顺序写歪。
func NextFire(cron, timezone string, after time.Time) (time.Time, bool, error) {
	schedule, err := ParseCron(cron)
	if err != nil {
		return time.Time{}, false, err
	}
	tz, err := time.LoadLocation(timezone)
	if err != nil {
		return time.Time{}, false, NewBadRequest("未知时区: " + strconv.Quote(timezone))
	}
	next, ok := schedule.NextAfter(after, tz)
	return next, ok, nil
}
