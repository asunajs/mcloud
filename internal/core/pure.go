// 纯函数层 —— 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/pure.rs。
//
// 这些是原 TS 版本里"藏在任务函数内部、只能靠真机试跑验证"的逻辑，
// 现在被提出来成为可验证的最小单元：日快照增量、开抢时刻、时间纠偏。
package core

import (
	"fmt"
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
//   - **完全无历史时两者均为 nil**（不是 0）：0 是一句"今天没涨"的读数，
//     而这里要说的是"没有可减的基准"。首次采集不虚报增量，也不虚报"零增量"。
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
		diff := count - prev.CloudCount
		out.CloudDiff = &diff
	}
	if weekBase != nil {
		diffWeek := count - weekBase.CloudCount
		out.CloudDiffWeek = &diffWeek
	}
	return out
}

// DiffClause 是快照增量那半句话，CLI 的"余额已记录"与服务端运行历史的
// [CloudSnapshotResult]-类小结**共用同一份**（见 `internal/cli/run_signin.go`、
// `internal/app/cloud_snapshot_run.go`）。
//
// # 为什么要收成一处（这不是"避免重复"的口味问题）
//
// 两个调用点各写一遍 nil 判断，是本项目 2026-09-24 真踩出来的坑：把 `CloudDiff`
// 改成 `*int64` 之后，`fmt.Sprintf("%+d", stats.CloudDiff)` **照样编译、照样过 vet**，
// 实测打出来的是 ——
//   - 指针为 nil：`+0`（读起来正是我们刚刚要消灭的那句"今天没涨"）；
//   - 指针有值：`+126914284740856`（`%+d` 打的是**地址**，不是数）。
//
// 而这两个形状都不会让任何一条既有断言变红：CLI 那边只断"余额已记录：10502 豆"，
// 增量那一小句当时没人钉。所以"记得判 nil"不能作为规则，得让**不判 nil 的写法无处可写**。
//
// ⚠ 反过来说：新增第三个消费方时**不要**再自己解引用这两个字段拼句子，
// 要么复用本函数，要么给它加一档措辞。
func (s CloudStats) DiffClause() string {
	if s.CloudDiff == nil || s.CloudDiffWeek == nil {
		// ⚠ 措辞说的是"**没有往日的记录**"，不是"第一次采集"（2026-09-25 真账号那轮改的）：
		// `history` 按 [NextCloudStats] 的契约**不含当天**那一行 ⇒ 同一天连跑五次，
		// 每一次都会走到这一支。当时那句"第一次采集"在一天里的第五次读数上就是错的，
		// 而它恰好是这条能力最常被使用的形态（一天里多次采）。
		return "暂无往日记录，算不出日增/周增"
	}
	// ⚠ `%+d` 配的是**解引用之后**的整数（nil 那一支在上面已经分流了）：
	// 涨 → `+3`，持平 → `+0`，跌 → `-500`。
	return fmt.Sprintf("日增 %+d，周增 %+d", *s.CloudDiff, *s.CloudDiffWeek)
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
