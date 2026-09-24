package app

// 签到的**只读查询**与**领取**那一族。
//
// 与 [CloudSignIn] 的分工：签到只是打卡，云朵要另外领
// （所以 `infoV3` 的 `beforeTotal` 在签到前后不变）。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 只读查询
// ─────────────────────────────────────────────────────────────────────────────

// SignInStatus 只查状态，不签到（CLI 的 `--status-only`）。
func SignInStatus(
	ctx context.Context, src *SignInSessionSource,
) (upstream.SignInStatus, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.SignInStatus, *core.Error) {
		return src.up.AppSignInStatus(ctx, s.jwt, s.referer)
	})
}

// SignInStatusRaw 同上，但返回 **`infoV3` 的原始响应体**（不解析）。
//
// 排障用：字段对不上时，只有原文能说清是"响应里没有"还是"我们没解析出来" ——
// 「模板没渲染」不等于「响应没有」，这条区别只有原文能给出。
func SignInStatusRaw(
	ctx context.Context, src *SignInSessionSource,
) (any, *core.Error) {
	return do(ctx, src, func(s SignInSession) (any, *core.Error) {
		return src.up.AppSignInStatusRaw(ctx, s.jwt, s.referer)
	})
}

// RecordsQuery 是 `cloudRecordV2` 的查询条件。
//
// 做成结构而不是四个裸参数，理由与 [SignInOptions] 同源：`AiDou` 与 `Filter`
// 都是 `int64`，`PageNumber` 与 `PageSize` 也都是 —— 四个裸 `int64` 连排时，
// 传反了编译期一点提示都没有（实测这种错会静默变成"查错账本、取错页"）。
type RecordsQuery struct {
	// AiDou 是账本：取 [upstream.RecordBookCloud]（改名前的云朵）
	// 或 [upstream.RecordBookBean]（改名后的 AI 豆）。
	AiDou int64
	// Filter 是收支方向过滤。全部用 [upstream.RecordFilterAll]。
	Filter int64
	// PageNumber 是页码，**从 1 开始**。
	PageNumber int64
	// PageSize 是每页条数。
	PageSize int64
}

// CloudRecords 查一档**豆的收支流水**（`cloudRecordV2`）。**只读**，审计 / 排障用。
//
// # 它跟 `--status-only` 的分工
//
// `--status-only` 回答"**现在有多少**"（`infoV3.total`）；这一条回答"**这些是怎么来的**"。
// 余额对不上时，只有流水能指出差在哪一笔 —— 光看两个余额是永远对不出来的。
func CloudRecords(
	ctx context.Context, src *SignInSessionSource, query RecordsQuery,
) (upstream.CloudRecords, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.CloudRecords, *core.Error) {
		return src.up.AppCloudRecords(ctx, s.jwt, s.referer,
			query.AiDou, query.Filter, query.PageNumber, query.PageSize)
	})
}

// PendingPrizes 查**待领取奖品**（`record` 的「待领取」页签）。只读。
//
// # 它解决什么
//
// 旧项目 `mcloud-sign` 的 `printPendingPrizes`：把"中了但还没领"的奖品列出来，
// 连过期时间一起 —— 免得奖品躺着过期。
//
// ⚠ 分页：这一条**只取一页**（`pageSize` 由调用方给），所以多于那一页会**静默截断** ——
// 旧项目 `printPendingPrizes` 就是这么写的（`pageNumber=1,pageSize=10`），我们照搬了行为，
// 但它被列为"重写要修"的一条（`docs/business-feature-inventory.md` §2.1 `prizes`）。
//
// ⚠ 字段名以**实拍**为准：`expireTime` 在响应里**是存在的**（中奖后 14 天到期），
// 只是当前版 H5 的**列表项模板**只渲染「中奖时间 `insertTime`」。
// 「模板没渲染」不等于「响应没有」—— 这一句此前在这里写反过，正确结论见
// [upstream.PrizeRecordItem.ExpireTime] 那段（实拍与更正都在那里）。
//
// `pageSize` 只取一页：这条的用途是"看有没有待领的"，不是把历史全拉下来。
func PendingPrizes(
	ctx context.Context, src *SignInSessionSource, pageSize int64,
) (upstream.PrizeRecords, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.PrizeRecords, *core.Error) {
		return src.up.AppPrizeRecords(ctx, s.jwt, s.referer,
			upstream.PrizeMarketName, upstream.PrizeStatusPending, 1, pageSize)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 领取（指定一笔）
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveTarget 是 [ReceiveCloud] 的输入：领哪一笔、以及拿什么设备标识去领。
//
// 做成结构而不是三个裸参数，理由与 [SignInOptions] 同源：
// 它们全是字符串 / 可选值，位置一错就是"把 cloudType 当成了 deviceId"，
// 而编译器不会吭声。
type ReceiveTarget struct {
	// CloudID 是 `receiveV3` 的 `cloudId` —— 来自 `infoV3` 的 `receiveList[].recordId`。
	//
	// 实拍里那个数组**没有 `recordId`**，所以 H5 的 `e.recordId ?? ""` 给的就是空串。
	CloudID string
	// CloudType 是 `receiveV3` 的 `cloudType` —— 同样来自 `receiveList[]`。
	//
	// `nil` 表示"上游没给"，body 里会写成空串（照 H5 的 `?? ""`）。
	//
	// ⚠ **不要**在这里缺省成 `0`：`0` 是一个**真实的** cloudType（立即领取那一类），
	// 而"上游没给"与"上游给了 0"在下游是两个分支。缺省会把"不知道"伪造成"知道"。
	CloudType *int64
	// DeviceID 是 H5 的 `deviceId` —— **就是 [DeviceID] 换回来的那个值**。
	//
	// ⚠ 上游**校验它的内容**（不带回 `615`、内容错回 `614`），所以不能给编的值 ——
	// 但也不需要"用官方 App 打开一次签到页"：那个值我们自己就能换。
	DeviceID string
}

// ReceiveCloud 领云朵（`receiveV3`）。**这是写请求** —— 上游认为有可领的就会真的领走。
//
// # 为什么要单独一个函数，而不是塞进 [CloudSignIn]
//
// 两者**不是一回事**：签到只是打卡，云朵要另外领（所以 `infoV3` 的 `beforeTotal`
// 在签到前后不变）。日常流程里它们通常连着跑，但也有人只想签到、不想领。
// 分开之后"领不领"由调用方决定，而不是被签到顺手决定掉。
//
// # `DeviceID`
//
// 这是**已知唯一一个不带就失败**的接口（H5 标了 `isDeviceId: true`）。
//
// ⚠ 会话走 [SignInSessionSource]：与同一条链上的别的请求**共用**这一轮那一条，
// 不再自己开一条（自检 / 探测那档单独跑时，源里也只有它需要的那一条）。
func ReceiveCloud(
	ctx context.Context, src *SignInSessionSource, target ReceiveTarget,
) (upstream.ReceiveResult, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.ReceiveResult, *core.Error) {
		return src.up.AppReceiveV3(ctx, s.jwt, s.referer,
			target.CloudID, target.CloudType, target.DeviceID)
	})
}

// ReceiveRevivalReward 用复活卡 / 回馈卡（`receiveRevivalReward`）。
// **这是写请求** —— 卡用掉就没了。
//
// # 它跟 [ReceiveCloud] 不是同一件事
//
// `receiveV3` 领的是**当期可领的云朵**；这一条用的是**卡**：
// H5 弹窗写得很清楚 ——「关闭弹窗视为放弃机会，**无法补发**」。
// 所以调用点必须是有意为之，不能"顺手也跑一下"。
//
// # `deviceID` 为什么是 `*string`
//
// 两个证据不一致：H5 定义里**没有** `isDeviceId`（⇒ 不发），
// 老抓包里却带了一个 `deviceid` **头**。哪个是服务端真正要求的**未验证**，
// 所以留成可选参数 —— 真出问题时能一次 A/B 试出来（`receiveV3` 就是这么定的）。
// ⚠ 命中 `90001` 时 [SignInSessionSource.Call] 会刷一次票**并把这一笔重放**。
// "卡用掉就没了"与"重放会不会用掉两张"这里**都没有样本** ——
// 要核它得在有效期内拿一个号连发两次同一笔 `receiveRevivalReward` 并比对卡数
// （口径来自用户 2026-09-23：重放对读写一视同仁；见 `signin_session.go` 头注第 3 条）。
func ReceiveRevivalReward(
	ctx context.Context, src *SignInSessionSource, deviceID *string,
) (upstream.RevivalReward, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.RevivalReward, *core.Error) {
		return src.up.AppReceiveRevivalReward(ctx, s.jwt, s.referer, deviceID)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 领取的分流：`receiveList[]` → 该发哪个请求
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveAction 是 `receiveList[]` 里一条**该怎么处理** ——
// 这就是 H5 的点击分流，只是不依赖 UI。
//
// # 两条完全不同的线（这是最容易搞混的地方）
//
// | | **月度任务奖励** | **立即领取** |
// | --- | --- | --- |
// | `cloudType` | `1`（**已到期、现在能领**）/ `2`（**还没到期**，下月才可领）| 其它值 |
// | 来源 | 签到 / 做任务攒下的记录，**跨月结算** | 当期挂在账上的 |
// | 接口 | `receiveTaskExpansion(acceptDate)` | `receiveV3(client, cloudId, cloudType)` |
// | 门槛 | **要开启自动备份**（否则 `1302`）| 无 |
// | 生效 | 按 `acceptDate` 结算，**不是立即** | **直接生效** |
//
// ⇒ 所以「`receiveList` 里有个数」**推不出**「现在能领到」：
// 那个数可能是**下个月**才作数的那一类（[ReceiveActionNextMonth]）。
//
// ⚠ `1` 与 `2` 是**同一个特性的两个时点**，不是两种奖励：
// `2` 是"本月攒的、下月才能领"的预告，等它到期就变成 `1`（那时才带 `acceptDate`）。
// 这也解释了"为什么 `acceptDate` 只在 `1` 上出现" —— 只有到期的那条才需要结算日。
//
// # 判据出处
//
// H5 `newsignin.336f18c3.js` 里那三行（逐字见 [upstream.ReceiveItem.CloudType]）：
//
//	1 === t.cloudType ? A("2", t.acceptDate, e)   // receiveTaskExpansion
//	: 2 === t.cloudType ? s.$toast("下月可领取")  // 纯提示，不发请求
//	: n("receivePoints", t, "", …)                // receiveV3
type ReceiveAction struct {
	// Kind 决定这一个动作走哪条路。
	Kind ReceiveActionKind
	// AcceptDate 只在 [ReceiveActionMonthlyExpansion] 上有意义：
	// 上游在**同一条条目**上给的结算日，不是我们编的日期。
	//
	// ⚠ `""` = 上游没给 —— 这时**照样发**，只是不带查询参数。理由：H5 的调用点
	// 逐字是 `A("2", t.acceptDate, e)` → `receiveTaskExpansion({acceptDate: t})`，
	// 而 `t.acceptDate` 缺席时是 `undefined`，axios 的默认序列化器会把
	// `undefined` **整个键丢掉** ⇒ 真实客户端发出去的就是**裸 GET**。
	//
	// 这里曾经做成"缺了就跳过不发"，那会与真实客户端**行为分叉**：
	// 上游明明受理，我们却什么都没发，还看不出为什么。要"可见"就该把
	// "没给 acceptDate"这件事**打进结果**（见 [ReceiveOutcome]），
	// 而不是替上游决定不发。
	AcceptDate string
	// CloudNum 是这一条有多少云朵。
	CloudNum int64
	// CloudID 只在 [ReceiveActionImmediate] 上有意义：照 H5 的 `e.recordId ?? ""`
	// —— 缺了就是空串。
	CloudID string
	// CloudType 只在 [ReceiveActionImmediate] 上有意义。
	CloudType int64
}

// ReceiveActionKind 是 [ReceiveAction] 的三种去向。
type ReceiveActionKind int

const (
	// ReceiveActionMonthlyExpansion 是**月度任务奖励已到期**（`cloudType == 1`）
	// —— 走 `receiveTaskExpansion`。
	ReceiveActionMonthlyExpansion ReceiveActionKind = iota
	// ReceiveActionNextMonth 是**下月才能领**（`cloudType == 2`）
	// —— 现在**什么都不发**。
	ReceiveActionNextMonth
	// ReceiveActionImmediate 是**立即领取、直接生效** —— 走 `receiveV3`。
	ReceiveActionImmediate
)

// PlanReceive 把 `infoV3` 的 `receiveList` 按上面的规则分流。**纯函数**，不碰网络。
//
// 抽成纯函数是为了能单测：这条分流逻辑是"领错东西"的唯一防线，
// 而它一旦错，表现只是"领到 0 朵"——**看不出来**。
func PlanReceive(receiveList []upstream.ReceiveItem) []ReceiveAction {
	out := make([]ReceiveAction, 0, len(receiveList))
	for _, item := range receiveList {
		switch item.CloudType {
		case 1:
			// 月度任务奖励：`AcceptDate` 有就用，没有就发裸请求（见字段的说明）。
			// 空串按"没有"处理 —— H5 那边 `""` 也会被当成有值发出去，但一个空日期
			// 对服务端没有意义，**发 `?acceptDate=` 反而多一个空参数**，不如不发。
			date := ""
			if item.AcceptDate != nil {
				date = *item.AcceptDate
			}
			out = append(out, ReceiveAction{
				Kind:       ReceiveActionMonthlyExpansion,
				AcceptDate: date,
				CloudNum:   item.CloudNum,
			})
		case 2:
			// 下月可领取：H5 连请求都不发。
			out = append(out, ReceiveAction{Kind: ReceiveActionNextMonth, CloudNum: item.CloudNum})
		default:
			// 其余：立即领取。`CloudID` 照 H5 的 `e.recordId ?? ""` —— 缺了就是空串。
			cloudID := ""
			if item.RecordID != nil {
				cloudID = *item.RecordID
			}
			out = append(out, ReceiveAction{
				Kind:      ReceiveActionImmediate,
				CloudID:   cloudID,
				CloudType: item.CloudType,
				CloudNum:  item.CloudNum,
			})
		}
	}
	return out
}

// ReceiveOutcome 是一条 [ReceiveAction] 跑完之后的实况。
type ReceiveOutcome struct {
	// Kind 决定下面哪些字段有意义。
	Kind ReceiveOutcomeKind
	// AcceptDate 是**发出去的那个值**（[ReceiveActionMonthlyExpansion]）：
	// `""` = 没带查询参数（裸 GET）。
	//
	// 如实记下来，是为了让"这条到底发了什么"可查 —— 它不影响成功与否。
	AcceptDate string
	// CloudCount 是领到的合计。`nil` 之外的 `*int64` 为 nil 表示上游没说领到多少。
	CloudCount *int64
	// CloudNum 是 [ReceiveOutcomeNextMonth] 那个"下月能领多少"。
	CloudNum int64
	// CloudType 标出是哪一条失败 / 哪一条领到。
	CloudType int64
	// Received 是上游回的 `result.receive`。
	Received int64
	// Error 是失败原因（[ReceiveOutcomeFailed]）。
	Error string
}

// ReceiveOutcomeKind 是 [ReceiveOutcome] 的四种结局。
type ReceiveOutcomeKind int

const (
	// ReceiveOutcomeMonthlyExpansion 是月度任务奖励领成功。
	// `CloudCount` 见 [upstream.ReceiveTaskExpansion.CloudCount]
	// （nil = 上游没说领到多少）。
	ReceiveOutcomeMonthlyExpansion ReceiveOutcomeKind = iota
	// ReceiveOutcomeNextMonth 是下月才能领 —— 没发请求。
	ReceiveOutcomeNextMonth
	// ReceiveOutcomeImmediate 是立即领取成功，`Received` 是上游回的 `result.receive`。
	ReceiveOutcomeImmediate
	// ReceiveOutcomeFailed 是请求发了但**失败了** ——
	// 不中断其余条目，如实记下来。
	ReceiveOutcomeFailed
)

// ReceiveAll 按 [PlanReceive] 的分流**把该领的都领一遍**。
//
// # 与 [ReceiveCloud] 的区别
//
// [ReceiveCloud] 是"**我指定**领哪一笔"（`--cloud-type` 那种自检用法）；
// 本函数是"**按上游的账目自己判断**该领哪些" —— 这才是日常该走的路。
//
// 之所以要有它：把"该领哪个"交给用户，等于让用户去猜 `cloudType` 的含义，
// 而猜错的表现是"领到 0 朵"，**看起来像上游没发**。
//
// # 会话只建一条
//
// `infoV3` 与随后每个领取请求共用 [SignInSessionSource] 里那一条会话 ——
// 但每条请求**各自**过一遍 `Call`：撞 `90001` 时刷一次票、把**那一条**重放一次，
// 后面的条目接着用新票。整批共用一次刷新，而不是"一过期整批重来"。
func ReceiveAll(
	ctx context.Context, src *SignInSessionSource, deviceID string,
) ([]ReceiveOutcome, *core.Error) {
	status, perr := SignInStatus(ctx, src)
	if perr != nil {
		return nil, perr
	}

	outcomes := make([]ReceiveOutcome, 0, len(status.ReceiveList))
	for _, action := range PlanReceive(status.ReceiveList) {
		switch action.Kind {
		case ReceiveActionNextMonth:
			outcomes = append(outcomes, ReceiveOutcome{
				Kind:     ReceiveOutcomeNextMonth,
				CloudNum: action.CloudNum,
			})
		case ReceiveActionMonthlyExpansion:
			// ⚠ 空串必须落成 **nil**（不带查询参数），不是 `&""`（带一个空的
			// `acceptDate=`）。后者与真实客户端不同 —— H5 那边 `undefined` 会被
			// axios **整个键丢掉**，发出去的是裸 GET；多一个空参数就是分叉。
			var acceptDate *string
			if action.AcceptDate != "" {
				acceptDate = &action.AcceptDate
			}
			expansion, perr := do(ctx, src, func(s SignInSession) (upstream.ReceiveTaskExpansion, *core.Error) {
				return src.up.AppReceiveTaskExpansion(ctx, s.jwt, s.referer, acceptDate)
			})
			if perr != nil {
				outcomes = append(outcomes, ReceiveOutcome{
					Kind:      ReceiveOutcomeFailed,
					CloudType: 1,
					Error:     perr.Error(),
				})
				continue
			}
			outcomes = append(outcomes, ReceiveOutcome{
				Kind:       ReceiveOutcomeMonthlyExpansion,
				AcceptDate: action.AcceptDate,
				CloudCount: expansion.CloudCount,
			})
		case ReceiveActionImmediate:
			result, perr := do(ctx, src, func(s SignInSession) (upstream.ReceiveResult, *core.Error) {
				// 先拷一份再取地址：`AppReceiveV3` 收的是 `*int64`，
				// 把计划项里的字段地址交出去等于给它一个改我们账目的口子。
				cloudType := action.CloudType
				return src.up.AppReceiveV3(ctx, s.jwt, s.referer,
					action.CloudID, &cloudType, deviceID)
			})
			if perr != nil {
				outcomes = append(outcomes, ReceiveOutcome{
					Kind:      ReceiveOutcomeFailed,
					CloudType: action.CloudType,
					Error:     perr.Error(),
				})
				continue
			}
			outcomes = append(outcomes, ReceiveOutcome{
				Kind:      ReceiveOutcomeImmediate,
				CloudType: action.CloudType,
				Received:  result.Receive,
			})
		}
	}
	return outcomes, nil
}
