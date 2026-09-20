package mcloudio

import (
	"fmt"
	"time"
)

// 本文件只做一件事：把 `time.Time` 写成**与 chrono 的 `to_rfc3339()` 逐字节一致**的串。
//
// # 为什么不能直接用 [time.RFC3339Nano]
//
// `mcloud-io` 把时间列存成 TEXT，而 SQL 里对它做**字符串比较与排序**：
//
//	DELETE FROM sessions WHERE expires_at <= ?1
//	SELECT ... FROM users ORDER BY created_at ASC, id ASC
//	DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE started_at < ?)
//
// 所以"这一列里只有一种写法"是**功能性的前提**，不是风格问题。而两边的默认写法不同：
//
//	                     UTC 后缀      亚秒位数
//	chrono to_rfc3339()  +00:00       0 / 3 / 6 / 9（AutoSi）
//	Go RFC3339Nano      Z            去掉末尾的 0（1–9 位都可能）
//
// 混着的后果很具体：`+` 是 0x2B、`Z` 是 0x5A，于是**同一时刻**的 Rust 行
// 永远排在 Go 行之前 —— `ORDER BY created_at ASC, id ASC` 的次级键被抢先，
// `expires_at <= ?` 在边界那一格上对两种来源的行给出不同答案。
// 它不会报错，只会让"同一件事在两行上表现不一样"。
//
// # 规则从哪来
//
// **不是从文档推的**，是 chrono 0.4.45 真跑出来的：见
// `test/golden/datetime-format-vectors.json` 的 `$emit` 段
// （生成器 `scripts/gen-datetime-format-vectors.rs`）。
// 跨语言对拍要求生成侧 ≠ 消费侧 —— 要验证的正是"Go 写得对不对"，
// 用 Go 产出期望值等于自证。
//
// # 已知且**刻意保留**的差异
//
//   - 闰秒：chrono 能表示 `12:00:60`，Go 的 [time.Time] 表示不了。
//     实际数据里不会出现 —— `Utc::now()` 从不返回闰秒，`to_rfc3339()` 也不会写出它。
//   - 年份超出 0..9999：chrono 写 `+10000-01-01T…`，Go 的布局给 `10000-01-01T…`。
//     同样不会出现在实际数据里。
//
// 两条都只影响"手写进去的串"，而那种情况本来就该报错。
func formatTime(t time.Time) string {
	t = t.UTC()
	base := t.Format("2006-01-02T15:04:05")
	n := t.Nanosecond()
	switch {
	case n == 0:
		// AutoSi 的第一档：整秒不带小数点。
		return base + "+00:00"
	case n%1_000_000 == 0:
		return base + fmt.Sprintf(".%03d", n/1_000_000) + "+00:00"
	case n%1_000 == 0:
		return base + fmt.Sprintf(".%06d", n/1_000) + "+00:00"
	default:
		return base + fmt.Sprintf(".%09d", n) + "+00:00"
	}
}
