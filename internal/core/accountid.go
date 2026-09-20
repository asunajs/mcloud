// 账号标识与时区常量 —— 对应 Rust 侧 legacy/crates/mcloud-core/src/model.rs
// 的 AccountId，以及 lib.rs 的 PRODUCT_NAME / TZ_NAME / local_tz。
//
// ⚠ model.rs 的其余部分（Account / Schedule / Pipeline / WebuiConfig …）
// 尚未迁移 —— 那些类型要等它们的消费者（app / server / io）落地时一起带过来，
// 现在写只会是"没人读的结构体"。
package core

import "time"

// ProductName 是产品名。
//
// 单独定义而不是从模块名推：`/api/version` 与 `--version` 对外暴露的必须是
// **产品名**（`mcloud`），而不是"碰巧实现它的那个包"的名字。
const ProductName = "mcloud"

// TZName 是日快照与时间窗口统一使用的时区（项目面向国内用户）。
const TZName = "Asia/Shanghai"

// shanghaiOffsetSeconds 是 `Asia/Shanghai` 的固定偏移（UTC+8）。
//
// 用固定偏移兜底是**安全**的而不是权宜：中国自 1991 年起不再实行夏令时，
// 所以对 1991 年之后的所有时刻，UTC+8 与 IANA 数据库的结果逐秒相同。
// 兜底存在的理由很具体：Go 在部分环境（精简容器、Windows）里读不到 zoneinfo，
// 而 `time.LoadLocation` 那时会返回错误 —— 那会让"今天"算错一天，
// 也就是把昨天的余额算进今天。
const shanghaiOffsetSeconds = 8 * 3600

var shanghaiLoc = func() *time.Location {
	if loc, err := time.LoadLocation(TZName); err == nil {
		return loc
	}
	return time.FixedZone("CST", shanghaiOffsetSeconds)
}()

// LocalTZ 返回项目统一使用的时区。
//
// 月份/日快照的"今天"必须按此时区切分，否则跨零点会算错增量。
func LocalTZ() *time.Location { return shanghaiLoc }

// AccountID 是账号标识。
//
// 用脱敏手机号或昵称派生的稳定字符串，**不使用凭据**作为标识。
type AccountID string

// NewAccountID 构造一个账号标识。
func NewAccountID(id string) AccountID { return AccountID(id) }

// String 返回底层字符串（实现 fmt.Stringer，便于日志与拼接）。
func (a AccountID) String() string { return string(a) }

// CloudStats 是某一天的云朵快照。
type CloudStats struct {
	AccountID AccountID
	// Date 是 `YYYY-MM-DD`（按 [LocalTZ]）。
	Date       string
	CloudCount int64
	// CloudDiff 是与上一个**有记录**之日之差。
	CloudDiff int64
	// CloudDiffWeek 是与 7 天前（或最早记录）之差。
	CloudDiffWeek int64
}
