// 纯函数层 —— 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/pure.rs。
//
// 这些是原 TS 版本里"藏在任务函数内部、只能靠真机试跑验证"的逻辑，
// 现在被提出来成为可验证的最小单元：日快照增量、开抢时刻、时间纠偏。
package core

import (
	"strconv"
	"time"
)

// ─────────────────────────────────────────────────────────────────────────────
// 时间 / 日期
// ─────────────────────────────────────────────────────────────────────────────

// DailyKey 按 [TZName] 取"今天"，用作日快照的日期键。
//
// 跨零点必须按本地时区切分，否则会把昨天的余额算进今天。
func DailyKey(now time.Time) string {
	return now.In(LocalTZ()).Format("2006-01-02")
}

// ServerOffsetMS 是服务器时间与本地时间的偏差（毫秒，正数表示服务器更快）。
//
// 抢兑需要它：客户端时钟慢 2 秒就可能整点抢不到。
func ServerOffsetMS(localBeforeMS, serverMS int64) int64 {
	return serverMS - localBeforeMS
}

// MsUntilNextHMS 返回距离下一次 `h:m:s` 还有多少毫秒。
//
// 若今天该时刻已过（**含正好等于**），则指向明天。纯函数，不读系统时间。
//
// 时刻非法（如 24:00:00）返回 0 表示"立刻" —— 与 Rust 侧一致：
// 那边是 `with_hour(24)` 返回 None，调用方负责校验（见 [IsValidHMS]）。
func MsUntilNextHMS(now time.Time, h, m, s int) int64 {
	if !IsValidHMS(h, m, s) {
		return 0
	}
	target := time.Date(now.Year(), now.Month(), now.Day(), h, m, s, 0, now.Location())
	if !target.After(now) {
		// 加**恰好 24 小时**（不是 AddDate）：与 Rust 的 `Duration::days(1)` 同义。
		// 中国无夏令时，两者在本项目用到的时区里结果相同；写成 24h 是为了
		// 把"这是一个定长量"这件事留在代码里。
		target = target.Add(24 * time.Hour)
	}
	return target.Sub(now).Milliseconds()
}

// HMSToCron 把 `HH:mm:ss` 转成 6 段 cron（秒 分 时 日 月 周）。
//
// 原 TS 版本用 `ss mm hh * * *` 生成定时任务（如 09:59:40 → `40 59 9 * * *`）。
func HMSToCron(h, m, s int) string {
	return strconv.Itoa(s) + " " + strconv.Itoa(m) + " " + strconv.Itoa(h) + " * * *"
}

// IsValidHMS 校验时刻是否合法。抢兑时刻是用户输入，必须挡在入口。
func IsValidHMS(h, m, s int) bool {
	return h >= 0 && h < 24 && m >= 0 && m < 60 && s >= 0 && s < 60
}

// ─────────────────────────────────────────────────────────────────────────────
// 余额
// ─────────────────────────────────────────────────────────────────────────────

// PickCloudValue 是多源余额取值：优先取第一个可信的非空来源。
//
// 原 TS 版本 `UserState.totalCloud` 的写入被注释掉，余额从未被采集；
// 上游其实有 4 处能拿到余额，这里按优先级收敛成一个确定结果，
// 而不是散落在各任务里。
//
// ⚠ `0` 是**合法余额**，不能被当成"空" —— 判据是"有没有这个来源"，
// 不是"值是不是真"。（Rust 侧用 `Option`，Go 侧用 `*int64`：
// 两者的共同点是"空"必须能被表达，`0` 必须能被表达，且二者不同。）
func PickCloudValue(sources []*int64) (int64, bool) {
	for _, s := range sources {
		if s != nil {
			return *s, true
		}
	}
	return 0, false
}

// NextCloudStats 计算今天的云朵快照（含日增量与周增量）。
//
// history 必须是**过去**的快照，按日期升序；不含 date 当天。
//
// 增量语义：
//   - CloudDiff：与上一个**有记录**之日之差；
//   - CloudDiffWeek：与 7 天前（或更早）最近一条之差；若历史不足 7 天，
//     退化为最早一条之差；
//   - 完全无历史时两者均为 0（首次采集不虚报增量）。
func NextCloudStats(accountID AccountID, date string, count int64, history []CloudStats) CloudStats {
	var prev *CloudStats
	if n := len(history); n > 0 {
		prev = &history[n-1]
	}

	var cutoff time.Time
	hasCutoff := false
	if today, err := time.ParseInLocation("2006-01-02", date, LocalTZ()); err == nil {
		cutoff = today.AddDate(0, 0, -7)
		hasCutoff = true
	}

	// 优先取"7 天前或更早"里最近的一条；没有则退化为最早一条。
	var weekBase *CloudStats
	if hasCutoff {
		for i := len(history) - 1; i >= 0; i-- {
			d, err := time.ParseInLocation("2006-01-02", history[i].Date, LocalTZ())
			if err == nil && !d.After(cutoff) {
				weekBase = &history[i]
				break
			}
		}
	}
	if weekBase == nil && len(history) > 0 {
		weekBase = &history[0]
	}

	out := CloudStats{AccountID: accountID, Date: date, CloudCount: count}
	if prev != nil {
		out.CloudDiff = count - prev.CloudCount
	}
	if weekBase != nil {
		out.CloudDiffWeek = count - weekBase.CloudCount
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 脱敏
// ─────────────────────────────────────────────────────────────────────────────

// MaskPhone 把脱敏手机号按"前两位 + **** + 后两位"格式化。
//
// 原 CLI 的账号展示约定；非 11 位手机号（如已是昵称）原样返回。
//
// ⚠ 判据是"**至少 7 位且全是 ASCII 数字**"，不是"长度等于 11"：
// 写成 `len == 11` 会让 12 位的号码原样泄漏出来。
func MaskPhone(phone string) string {
	rs := []rune(phone)
	if len(rs) < 7 {
		return phone
	}
	for _, r := range rs {
		if r < '0' || r > '9' {
			return phone
		}
	}
	return string(rs[:2]) + "****" + string(rs[len(rs)-2:])
}
