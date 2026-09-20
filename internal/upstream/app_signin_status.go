// `infoV3` —— 签到状态，**判断"今天签没签"的唯一权威依据**。
//
// 对应 Rust 侧的 app_signin/status.rs。
//
// # 为什么是 `infoV3` 而不是 `startSignIn` 的响应
//
// 实测（账号 B）：`startSignIn` 回 `todaySignIn:false`、`cal` 里 19 号还是 `s:false`，
// 但紧接着的 `infoV3` 已经是 `s:true` —— 那一次**确实签上了**。
// 即 `startSignIn` 的响应是**调用前**的快照，不是成败判据。
package upstream

import (
	"encoding/json"

	"github.com/catlair/mcloud/internal/core"
)

// SignInDay 是 `infoV3` 的 `cal[]` 里的一天。
type SignInDay struct {
	// Day 是日（1..=31）。
	Day uint8
	// Signed 是该日是否已签（`s`）。
	Signed bool
	// CurrentMonth 是是否属于本月（`currentMonth == 1`）；false 的是上/下月的补位格。
	CurrentMonth bool
	// Points 是该日可获得云朵数（`n`）。
	Points int64
	// Today 是服务端是否把该日标为"今天"（`t`）。
	Today bool
}

// ReceiveItem 是 `infoV3` 的 `receiveList[]` 里的一条 —— "有一笔云朵挂在账上"。
//
// 它同时是 [ReceiveV3Request] 的**输入来源**（`cloudId` / `cloudType` 都取自这里）。
//
// ⚠ **名字里的 "receive" 不等于"现在能领"** —— 见 [ReceiveItem.CloudType]。
type ReceiveItem struct {
	// CloudType 决定这一笔现在能不能领。
	//
	// 判据是当前版 H5 源码 `newsignin.336f18c3.js`（本仓库 `har/h5-signin/`）里的
	// 那个点击分流，逐字如下：
	//
	//	1 === t.cloudType ? A("2", t.acceptDate, e)   // receiveTaskExpansion：延期领取
	//	: 2 === t.cloudType ? s.$toast("下月可领取")    // ← 纯提示，**不发任何请求**
	//	: n("receivePoints", t, "", …)                 // ← 只有这一支走 receiveV3
	//
	// 同一份源码里还有两处独立旁证：模板上 `"is-next-month": 2 === g[0]?.cloudType`，
	// 以及标签 `2 === g[0]?.cloudType ? "" : "可领取"` —— 即 `2` **不显示"可领取"**。
	//
	// ⚠ 曾经有人写"新版 H5 一律走 `receiveV3`"——**没有依据，已删**。抓包里那条账号的
	// `receiveList` 只有一条 `{cloudType:2}`，而整个 HAR 里 `receiveV3` 的请求数是 **0**：
	// 官方客户端正因为 `cloudType==2` 而**从未调用过它**。
	CloudType int64
	// CloudNum 是这笔有多少云朵（`cloudNum`）。
	CloudNum int64
	// RecordID 是 `recordId` —— [ReceiveV3Request] 的 `cloudID` 来源。
	//
	// ⚠ 实测抓到的条目里**没有这个字段**（只有 `cloudType` / `cloudNum`），
	// 于是 H5 的 `e.recordId ?? ""` 实际给的就是空串。这里如实做成可选，
	// **不替上游编一个默认值**。
	RecordID *string
	// AcceptDate 是 `acceptDate` —— **只有 `CloudType == 1` 才用得上**。
	//
	// H5 里 `1 === t.cloudType ? A("2", t.acceptDate, e)`，那个 `A` 就是
	// `receiveTaskExpansion({acceptDate})`。少了它，`CloudType == 1` 那条路
	// 就**根本发不出去**。
	AcceptDate *string
}

// 三种账目类型的取值。
const (
	// CloudTypeNow 是**当前可领取**（2026-09-19 实测）。
	//
	// 账目 `[{2,240},{0,6}]` 时 `toReceive` 是 `6`；走 `receiveV3(cloudType=0)` 之后
	// `toReceive` 变 `0`、`cloudType:0` 那条**从账目里消失**，而 `cloudType:2` 的 240
	// **一动没动**。
	//
	// ⇒ `0` 是**合法取值**（"当前可领取"），不是"上游没给" ——
	// 这正是 [ReceiveV3Request] 把缺字段写成空串、而不是写成 `0` 的原因。
	CloudTypeNow int64 = 0
	// CloudTypeDeferred 是**延期领取**（月度任务奖励那条线）：走
	// `receiveTaskExpansion`，**不是** `receiveV3`。
	//
	// 领不到会回 `1302`「开启自动备份按钮才可领取哦～」——
	// 门槛是 [SignInStatus.CurMonthBackup]。
	CloudTypeDeferred int64 = 1
	// CloudTypeNextMonth 是**「下月可领取」**，不是"待领取"。
	//
	// 给它发 `receiveV3` 是**官方客户端不会做的事**：实测回 `code=0`
	// 但 `result.receive = 0`（什么都没领到），而表面上看起来是成功的。
	CloudTypeNextMonth int64 = 2
)

// CanReceiveNow 报告这一笔是不是"现在就能领"。
//
// ⚠ 只有 [CloudTypeNow] 为真。把 [CloudTypeNextMonth] 也算进来，
// 表现是"请求成功（code=0）但一个云朵都没到手" —— 最难发现的那种错。
func (r ReceiveItem) CanReceiveNow() bool { return r.CloudType == CloudTypeNow }

// IsNextMonth 报告这一笔是不是"下月才可领"（H5 只弹一句 toast，不发请求）。
func (r ReceiveItem) IsNextMonth() bool { return r.CloudType == CloudTypeNextMonth }

// IsDeferred 报告这一笔是不是要走"延期领取"那条线（需要 `acceptDate`）。
func (r ReceiveItem) IsDeferred() bool { return r.CloudType == CloudTypeDeferred }

// SignInStatus 是 `infoV3` 的解析结果。
type SignInStatus struct {
	// BeanTotal 是**现行余额（豆）** —— 上游字段 `total`。
	//
	// # 为什么是 `total` 而不是 `beforeTotal`（这里记错过一次）
	//
	// H5 把两个数都显示出来，原话逐字：
	//
	//	total >= 600 && 1 == isCloudEnough
	//	  ? "恭喜你，现拥有" + total + "豆，足够兑换" + …
	//
	// 即 **`total` 是「现拥有」，`lackCloudNum` 是「还差多少」**。
	// 实测自洽：兑换门槛 `canExchangeText[0].pOrder = 12000`，
	// 而 `12000 - 10502(total) = 1498 = lackCloudNum` ✓。
	//
	// 第三个独立证据（2026-09-19）：`GET cloudRecordV2?aiDou=1` 那一档的
	// **全部条目求和，逐字等于 `total`**（账号 1 = 7431、账号 2 = 10502）。
	// ⇒ 这个数不是"某个接口报的另一个值"，而是**那本流水从头到尾的累加**。
	//
	// ⚠ 云朵与 AI豆是**同一个东西的两个名字**（2026-07-31 按 1:1 改名），
	// 所以别把这两个字段说成"两种货币" —— 它们只是**两个口径的数**。
	BeanTotal int64
	// CloudTotal 是上游字段 `beforeTotal` —— **改名那一刻的结转额**，不是余额。
	//
	// 它就是改名交接的那一条：`cloudRecordV2?aiDou=1` 里
	// `mark: "shouTiaoAiDou"` / `summary: "云朵转换AI豆"` 那笔的 `num`。
	//
	//	账号 1：shouTiaoAiDou 5392 == beforeTotal 5392 ✓；AI豆账本合计 7431 == total 7431 ✓
	//	账号 2：8027 == 8027 ✓；10502 == 10502 ✓
	//
	// ⇒ 它是一张**冻结的历史快照**，永远不变（先前"签到前后都不动"的实测，原因在这里）。
	//
	// ⚠ 我们**曾把它当成余额**记进 `cloud_stats`，趋势图因此是一条直线。
	// 现在它仍然解析出来，但**不再当作余额用**。
	//
	// ⚠ 还有一处**没查清**、别当成已知：云朵账本（`aiDou=0`）的合计比它
	// **恰好多 100**，两个账号都是，且那 100 都是 2024 年那笔 `mark:"1003"`。
	// 推测是**云朵过期不入账**，但**没有证据**，故只记现象。
	//
	// # 为什么它是可选（而 [SignInStatus.BeanTotal] 不是）
	//
	// 判据是"缺了它还能不能回答本结构的问题"：余额是我们要记、要报的那个数，
	// 缺了就等于没答；而这个是不用的旁证数，缺了只是少一条线索。
	// 让一个不用的字段有能力把整条状态查询打挂，是没必要付的代价。
	CloudTotal *int64
	// SignInPoints 是本次签到可得数（`signInPoints`）。
	SignInPoints int64
	// SignCount 是连续签到天数。
	SignCount int64
	// MonthDays 是服务端报的本月已签天数（`monthDays`）。
	MonthDays int64
	// Days 是日历格。
	Days []SignInDay
	// ReceiveList 是可领取的条目（`receiveList`）。
	//
	// 上游不给这个字段时是**空**，不是错误 —— 它只影响"要不要去领"，
	// 不影响"今天签没签"这个主判据。
	ReceiveList []ReceiveItem
	// ToReceive 是**现在就能领**的合计（`toReceive`）。上游有时不给。
	//
	// 语义是实测定的：账目 `[{2,240},{0,6}]` 时它是 `6`，领掉 `0` 那条之后变 `0`，
	// 而 `2` 那条**不计入** ⇒ 它只统计「立即领取」那一类，不含月度线。
	ToReceive *int64
	// CurMonthBackup 是本月**是否已开启自动备份**（`curMonthBackup`）。
	//
	// 它是**两条翻倍线的总门槛**：月度任务奖励（[CloudTypeDeferred]）与签到翻倍
	// 都要求它为真。nil = 上游没给（**不是 false**）——
	// 别把 nil 当 false，那会让"上游没报"变成"这个账号没开备份"。
	CurMonthBackup *bool
	// CurMonthBackupSignAccept 是本月的**签到翻倍**是否**已经领过**。
	//
	// 每月只有一次机会。旧项目 `mcloud-sign` 的判据逐字是
	// `if (curMonthBackup && !curMonthBackupSignAccept)` —— 两个都要成立才去领。
	CurMonthBackupSignAccept *bool
}

// SignedOn 报告本月某天是否已签；`day` 为 0 或 >31 时返回 false。
//
// 只看 `CurrentMonth` 的格子 —— `cal` 里还混着上月最后一天与下月开头几天，
// 按 `Day` 裸查会命中错的月份（实测抓包里就有 `d:31` 的上月格）。
func (s SignInStatus) SignedOn(day uint8) (bool, bool) {
	if day == 0 || day > 31 {
		return false, false
	}
	for _, d := range s.Days {
		if d.CurrentMonth && d.Day == day {
			return d.Signed, true
		}
	}
	return false, false
}

// SignedToday 报告服务端标为"今天"的那格是否已签。
//
// 用它**不需要本地时钟** —— 这也是它比"自己算今天几号"更稳的地方
// （服务端与时区才是判"今天"的那一方）。
func (s SignInStatus) SignedToday() (bool, bool) {
	for _, d := range s.Days {
		if d.Today {
			return d.Signed, true
		}
	}
	return false, false
}

// CountedMonthDays 是按 `cal` 数出来的本月已签天数。
//
// 与 [SignInStatus.MonthDays] 是两条独立路径（一个来自日历格、一个来自服务端字段），
// 相等才说明解析没走样。
func (s SignInStatus) CountedMonthDays() int {
	n := 0
	for _, d := range s.Days {
		if d.CurrentMonth && d.Signed {
			n++
		}
	}
	return n
}

// ParseSignInStatus 解析 `infoV3` 的响应。
func ParseSignInStatus(envelope *Envelope) (SignInStatus, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return SignInStatus{}, err
	}
	if payload == nil {
		return SignInStatus{}, core.NewParse("infoV3 响应没有 result")
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return SignInStatus{}, core.NewParse("infoV3 的 result 不是对象")
	}

	cells, ok := obj["cal"].([]any)
	if !ok {
		return SignInStatus{}, core.NewParse("infoV3 响应缺少 result.cal 数组")
	}

	days := make([]SignInDay, 0, len(cells))
	for _, cell := range cells {
		day, err := RequireInt64(cell, "d")
		if err != nil {
			return SignInStatus{}, err
		}
		if day < 0 || day > 31 {
			return SignInStatus{}, core.NewParse("infoV3 的 cal[].d 越界")
		}
		days = append(days, SignInDay{
			Day:          uint8(day),
			Signed:       boolAt(cell, "s"),
			CurrentMonth: intAt(cell, "currentMonth") == 1,
			Points:       intAt(cell, "n"),
			Today:        boolAt(cell, "t"),
		})
	}

	// `receiveList` 与主判据无关：上游不给时给空，而不是报错 ——
	// 那会让"今天签没签"这件确定的事被一个可选的附属字段拖垮。
	var receiveList []ReceiveItem
	if items, ok := obj["receiveList"].([]any); ok {
		for _, item := range items {
			m, ok := item.(map[string]any)
			if !ok {
				continue
			}
			cloudType, okType := intAtOK(m, "cloudType")
			cloudNum, okNum := intAtOK(m, "cloudNum")
			// 这两个字段缺任一个就整条跳过（与 Rust 的 `filter_map` 同）：
			// 一条字段不全的账目，下游拿着它去发领取请求只会得到一个莫名其妙的码。
			if !okType || !okNum {
				continue
			}
			entry := ReceiveItem{CloudType: cloudType, CloudNum: cloudNum}
			if text, ok := m["recordId"].(string); ok {
				entry.RecordID = &text
			}
			if text, ok := m["acceptDate"].(string); ok {
				entry.AcceptDate = &text
			}
			receiveList = append(receiveList, entry)
		}
	}

	status := SignInStatus{
		Days:        days,
		ReceiveList: receiveList,
	}
	if v, ok := intAtOK(obj, "beforeTotal"); ok {
		status.CloudTotal = &v
	}
	if v, ok := intAtOK(obj, "toReceive"); ok {
		status.ToReceive = &v
	}
	// 两个门槛字段**只认真正的布尔**：实测上游给的就是 `true` / `false`；
	// 万一哪天变成 `1` / `0`，这里会得到 nil，于是调用方判"未知"而**不去发写请求**
	// —— 这是个安全的失败方向。
	if v, ok := obj["curMonthBackup"].(bool); ok {
		status.CurMonthBackup = &v
	}
	if v, ok := obj["curMonthBackupSignAccept"].(bool); ok {
		status.CurMonthBackupSignAccept = &v
	}

	for _, spec := range []struct {
		key string
		out *int64
	}{
		{"total", &status.BeanTotal},
		{"signInPoints", &status.SignInPoints},
		{"signCount", &status.SignCount},
		{"monthDays", &status.MonthDays},
	} {
		v, err := RequireInt64(payload, spec.key)
		if err != nil {
			return SignInStatus{}, err
		}
		*spec.out = v
	}
	return status, nil
}

// boolAt 取一个可选布尔，缺了或非布尔一律当作 false。
func boolAt(payload any, key string) bool {
	obj, ok := payload.(map[string]any)
	if !ok {
		return false
	}
	v, ok := obj[key].(bool)
	if !ok {
		return false
	}
	return v
}

// intAt 取一个可选整数，缺了或非数字一律当作 0。
func intAt(payload any, key string) int64 {
	v, _ := intAtOK(payload, key)
	return v
}

// intAtOK 取一个可选整数，并报告它**在不在且是不是数字**。
//
// ⚠ [core.ParseJSON] 用的是 `json.Number`，所以数字要同时认 `json.Number`
// 与 `float64`（见方案 §6.1 #34）。
func intAtOK(payload any, key string) (int64, bool) {
	obj, ok := payload.(map[string]any)
	if !ok {
		return 0, false
	}
	switch v := obj[key].(type) {
	case json.Number:
		n, err := v.Int64()
		return n, err == nil
	case float64:
		return int64(v), true
	case int64:
		return v, true
	}
	return 0, false
}
