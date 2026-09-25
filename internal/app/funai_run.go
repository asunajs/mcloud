package app

// 「全网趣玩AI赢好礼」抽奖跑一个账号那一段 —— 读判据 ⇒ 判 ⇒ **最多抽一次**。
//
// 对应旧实现里的 `play-ai-lottery` / `fun_ai`（`docs/business-feature-inventory.md` §2.4）。
//
// # 链只有两枪
//
//	GET  /ycloud/funai/api/page/getmodule   读 drawTimes / drawsUsed / 活动起止
//	POST /ycloud/funai/api/page/lottery     body 只有 {"moduleId":"…"}，值取自上一枪
//
// 抓包里其余那几类（`timed/getmodule`、规则文本、邀请码、广告位与素材）为什么不在链上，
// 逐条写在 `internal/upstream/app_funai.go` 文件头。
//
// # `jwttoken` 使用公共来源（临时态，不是终判）
//
// 这一页的票按 [SignInWorld.Source] 走签到页那一条建会话配方（`RecipeSignInPage`）。
// 实拍归档里**没有**换出该页 jwttoken 的那一步（也没有 `getAccount(National_playAI)`
// 与活动门户预热），所以该页的特有取法无从得知 ⇒ 按退路走（口径见 `AGENTS.md` §1.12）。
// ⇒ 抓到特有取法时**主动更正**这一处调用点并改掉这段说明，不留"注释说公共、代码走特有"。
//
// # 为什么门槛判在这里而不是各能力函数里写个 if
//
// 抽奖是**写请求**且每天只有一次额度：判错的两种代价都不对称 ——
// 该不发时发了会**用掉**这一次（重跑也回不来），该发时不发只是少一次。
// 判定因此做成纯函数 [PlanFunai] 单独测（逐档见 `funai_run_test.go`）。

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// FunaiDailyDrawLimit 是**本工具**每天最多抽几次 = 1。
//
// # 这个 1 是从哪来的（三种数最容易混成一个）
//
//   - **上游规则文本说 5**：`getByMarketName?marketName=National_playAI` 里那句
//     「每个用户每日最多可通过**完成任务**获得 5 次抽奖机会…当日有效，过期失效」——
//     说的是**做任务能挣到几次**，不是"每天最多能抽几次"。⚠ 别拿它当本常量的出处。
//   - **实拍的两个计数是 1/0**（`drawTimes: 1`、`drawsUsed: 0`）：这是**当期这一个模块**
//     给出来的数，不是"上限"这个规则本身 —— 一份样本定不了它跟规则的关系。
//   - **本常量 1**：用户 2026-09 拍板的取向，与旧 TypeScript 实现一致
//     （`play-ai-lottery` 就是「只抽一次，不看剩余次数循环」）。
//     ⇒ 这是**刻意的**比上游可能允许的更少，理由是不拿写请求去试探额度。
//
// ⚠ "上游会不会因为一天只抽一次而少给什么"没有样本，**未测试**。
const FunaiDailyDrawLimit int64 = 1

// ─────────────────────────────────────────────────────────────────────────────
// 门槛判定（纯函数，不碰网络）
// ─────────────────────────────────────────────────────────────────────────────

// FunaiPlan 是"这一趟该不该抽"。
//
// 每一档都必须说得出**为什么不是别的一档**：这六个值最后都会变成界面上一句话，
// 而"跳过"这句话没有用的（用户会问"那我明天来还是后天来"）。
type FunaiPlan int

const (
	// FunaiPlanDraw 是**可以抽**：活动期内、且次数判据显示还有额度。
	FunaiPlanDraw FunaiPlan = iota
	// FunaiPlanAlreadyDrawn 是**今天已经抽过** —— `drawsUsed` 已经达到 [FunaiDailyDrawLimit]。
	//
	// 它排在"没次数"之前：两个条件同时成立时（实拍那一档抽完后就是 `1/1`），
	// "今天已经抽过"是更有用的那句 —— 它同时回答了"明天再来有没有"。
	FunaiPlanAlreadyDrawn
	// FunaiPlanNoChances 是**没有次数**：`drawTimes - drawsUsed <= 0`，
	// 但今天这一次额度还没用掉（多做任务能挣到次数，见常量那段）。
	FunaiPlanNoChances
	// FunaiPlanNoCriteria 是上游**没给判据字段**（`drawTimes` 或 `drawsUsed` 缺 / 是 `null`）
	// ⇒ 判不了。
	//
	// ⚠ 单独成一档，且**不发请求**：当成"有次数"会白发一次写请求把今天的额度用掉，
	// 当成"没次数"会漏抽。与 [MultiplePlanUnknown] 同一个道理。
	// 实拍里 `timed/getmodule` 那一份三个计数**全是 null** —— 这一档不是假想分支。
	FunaiPlanNoCriteria
	// FunaiPlanOutOfWindow 是**活动不在期**：`startTime` / `endTime` 与当前时刻比过了，
	// 在区间之外。
	FunaiPlanOutOfWindow
	// FunaiPlanWindowUnknown 是**活动期判不了**：`startTime` / `endTime` 没给，
	// 或形态不认识（[upstream.FunaiActivityTimeLayout] 解不出来）。
	//
	// ⚠ 这一档**不发请求**。判不了就是判不了 —— 拿"多半还在期"去发写请求，
	// 代价是可能花掉今天唯一那一次。
	// （区间倒挂那份坏数据不在这一档：它必然落在"不在期"，理由见 [funaiWindowOf]。）
	FunaiPlanWindowUnknown
)

// Reason 是这一档说给用户的那半句（前面会接上"趣玩AI没抽："）。
//
// ⚠ 只复述判据本身，不解释上游语义：`drawTimes` 与 `drawsUsed` 的语义
// 只有一档实拍（见 [upstream.FunaiModule] 那段），把它们说成"总共能抽 N 次"
// 就是把猜测写成断言。
func (p FunaiPlan) Reason() string {
	switch p {
	case FunaiPlanAlreadyDrawn:
		return "今天已经抽过一次（每天最多抽 " +
			strconv.FormatInt(FunaiDailyDrawLimit, 10) + " 次）"
	case FunaiPlanNoChances:
		return "上游报的次数判据显示没有可用次数"
	case FunaiPlanNoCriteria:
		return "上游没给次数判据字段，判不了，本次没发请求"
	case FunaiPlanOutOfWindow:
		return "不在活动起止时间内"
	case FunaiPlanWindowUnknown:
		return "活动起止时间判不了，本次没发请求"
	}
	return "可以抽"
}

// funaiWindow 是活动期三态。**必须有"判不了"这一格**：两值布尔会把"上游没给时间"
// 落成"不在期"或"在期"中的一个，而这两种处置都不对（见 [FunaiPlanWindowUnknown]）。
type funaiWindow int

const (
	// funaiWindowIn 是落在 `startTime`~`endTime` 之内（含端点）。
	funaiWindowIn funaiWindow = iota
	// funaiWindowOut 是明确在区间之外。
	funaiWindowOut
	// funaiWindowUnknown 是一个边界都判不出来。
	funaiWindowUnknown
)

// funaiWindowOf 按当前时刻判活动期。
//
// # 只用上游给的两个串，不自造 TTL
//
// `startTime` / `endTime` 的形态与按哪个时区解释，见
// [upstream.FunaiActivityTimeLayout] 与 [upstream.FunaiActivityLocation]。
// 这里只补一句本地判断：**两个边界都没给**（或形态都不认识）⇒ 判不了；
// 只给了一个就只判那一边（上游确实可能只挂一个 —— 未验证，所以按最保守的可用信息判）。
func funaiWindowOf(module upstream.FunaiModule, now time.Time) funaiWindow {
	if module.StartTime == nil && module.EndTime == nil {
		return funaiWindowUnknown
	}
	if module.StartTime != nil && now.Before(*module.StartTime) {
		return funaiWindowOut
	}
	if module.EndTime != nil && now.After(*module.EndTime) {
		return funaiWindowOut
	}
	// 到这里：已知的每一边都夹住了 now。
	//
	// ⚠ 区间本身坏掉（`startTime` 晚于 `endTime`）时**不需要**单独一格，也不用补一格
	// "判不了"：那两条越界检查已经把这种区间的全部时刻都判成"不在期" ——
	// 设 `start > end`，则任一时刻要么 `< start`（第一条命中）、要么 `> end`（第二条命中），
	// 想同时躲开两条得满足 `start <= now <= end`，而它无解。
	// 落"不在期"而不是"判不了"也更诚实：一个倒挂的窗口里确实没有任何时刻在期。
	return funaiWindowIn
}

// PlanFunai 按一次 `page/getmodule` 的解析结果判 [FunaiPlan]。**纯函数**，不碰网络。
//
// # 为什么只用这三个计数，不碰 `isDraw` / `drawTime` / `drawResult`
//
// 那三个字段在实拍里**全是 `null`**，它们的非 `null` 形态**一个样本都没有** ——
// 拿"望文生义的名字"当判据，等于把猜测变成断言（且它错的时候不报错，只是抽或不抽）。
// 歧义登记与"要核它得怎么做"见 [upstream.FunaiModule] 的字段注释。
//
// # 匹配顺序有意义
//
// 先「明确不在期」（最硬、且与次数无关）→ 再「判不了次数」→ 再「今天已抽过」→
// 再「没有次数」→ 最后才用「期次判不了」兜住那一档不发。
// 换句话说：**能给出具体原因的排前面**，"判不了"这种没有信息量的说法排最后。
func PlanFunai(module upstream.FunaiModule, now time.Time) FunaiPlan {
	window := funaiWindowOf(module, now)
	switch {
	case window == funaiWindowOut:
		return FunaiPlanOutOfWindow
	case module.DrawTimes == nil || module.DrawsUsed == nil:
		return FunaiPlanNoCriteria
	case *module.DrawsUsed >= FunaiDailyDrawLimit:
		return FunaiPlanAlreadyDrawn
	case *module.DrawTimes-*module.DrawsUsed <= 0:
		return FunaiPlanNoChances
	case window == funaiWindowUnknown:
		return FunaiPlanWindowUnknown
	default:
		return FunaiPlanDraw
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 能力函数：查判据 + 最多抽一次
// ─────────────────────────────────────────────────────────────────────────────

// FunaiStatus 是这一趟的结局档位。
type FunaiStatus int

const (
	// FunaiDone 是**抽到了**：请求发了，上游回 `code == 0`。
	//
	// ⚠ 它不等于"中奖" —— 实拍的 `isWin` 只有 `1` 这一档，"没中奖长什么样"未验证
	// （见 [upstream.FunaiLottery.IsWin]）。这一档说的是"这一次抽完了"。
	FunaiDone FunaiStatus = iota
	// FunaiNothing 是**本来就没得抽**（今天已抽过 / 没次数 / 判不了 / 不在期）——
	// 一条请求都没发。它算**成功**：这一趟的目标是"该抽的时候抽掉"，
	// 来一趟确认"现在不该抽"就是达成了（与 [ClaimNothing] 同一口径）。
	FunaiNothing
	// FunaiFailed 是**请求发了但失败** —— 这一趟没办成。
	//
	// ⚠ 不返回错误：与 [MultipleOutcomeFailed] / [BackupGiftOutcomeFailed] 同一理由 ——
	// 折成结论才能把"上游回什么"带出去。"没有次数"那一类失败会落在这一档
	// （上游这句话**尚无样本**，所以不猜码值，一律按失败如实报）。
	FunaiFailed
)

// FunaiReadings 是这一趟从 `page/getmodule` 读到的**判据原值**，只用于播报。
//
// # 为什么把数一路带到小结（2026-09-25 真账号那轮加的）
//
// 那一轮两个号都判成"没抽"，界面上只留下一句
// "趣玩AI没抽：上游报的次数判据显示没有可用次数" —— 于是这一趟**答不出**任何一条
// 判据问题：`drawTimes` 到底是几、`drawsUsed` 是不是满了、活动期到哪天，
// 全都知道但全都没说。R3（"没中奖长什么样"）与"次数判据的语义"这两条
// 都要靠**下一次真跑的读数**才能收口，所以读数必须进小结而不是只进日志。
//
// ⚠ 仍然只**照抄**、不解释：这几个键的语义未验证（见 [upstream.FunaiModule]），
// 念成"今天还能抽 0 次"就是把猜测写成断言。
type FunaiReadings struct {
	// DrawTimes / DrawsUsed / TodayGetLotteryCount 是上游那三个计数，`nil` = 没给。
	DrawTimes            *int64
	DrawsUsed            *int64
	TodayGetLotteryCount *int64
	// StartTime / EndTime 是活动期两界，`nil` = 没给**或**形态不认识。
	StartTime *time.Time
	EndTime   *time.Time
}

// Line 把读数念成一句短语。缺的键念成"缺"而不是 0 ——
// "上游没给这一格"与"上游给了 0"在判据上是两件事（与 `noticeSlotLabel` 同一口径）。
func (r FunaiReadings) Line() string {
	counters := []struct {
		key   string
		value *int64
	}{
		{"drawTimes", r.DrawTimes},
		{"drawsUsed", r.DrawsUsed},
		{"todayGetLotteryCount", r.TodayGetLotteryCount},
	}
	parts := make([]string, 0, len(counters)+2)
	any := false
	for _, c := range counters {
		if c.value != nil {
			any = true
			parts = append(parts, fmt.Sprintf("%s=%d", c.key, *c.value))
			continue
		}
		parts = append(parts, c.key+"=缺")
	}
	// 两界只在真读到时才念：那两格决定"不在期"这一档，而"没给"与"给了但解不开"
	// 在这一层都是 nil，念出来反而像上游给了空串。
	for _, bound := range []struct {
		key   string
		value *time.Time
	}{{"起", r.StartTime}, {"止", r.EndTime}} {
		if bound.value != nil {
			any = true
			parts = append(parts, bound.key+" "+bound.value.Format("2006-01-02"))
		}
	}
	if !any {
		// 五个键一个都没读到：这句话没有信息量，宁可不念。
		return ""
	}
	return strings.Join(parts, "，")
}

// funaiReadingsOf 从上游解析结果里取播报要用的那几格。
func funaiReadingsOf(module upstream.FunaiModule) FunaiReadings {
	return FunaiReadings{
		DrawTimes:            module.DrawTimes,
		DrawsUsed:            module.DrawsUsed,
		TodayGetLotteryCount: module.TodayGetLotteryCount,
		StartTime:            module.StartTime,
		EndTime:              module.EndTime,
	}
}

// FunaiOutcome 是 [FunaiDraw] 的实况。
type FunaiOutcome struct {
	// Status 决定下面哪些字段有意义。
	Status FunaiStatus
	// Plan 是发请求之前的判定（[FunaiNothing] 时它就是原因）。
	Plan FunaiPlan
	// ModuleID 是这一趟用的模块 id（来自 `page/getmodule`，**不是**写死的）。
	ModuleID string
	// PrizeName 是上游报的奖品名。`nil` = 上游没说，**不是**"没中奖"。
	PrizeName *string
	// Readings 是判据原值（抽奖那一枪发没发都带着它，见 [FunaiReadings]）。
	Readings FunaiReadings
	// Error 是 [FunaiFailed] 的失败原因。
	Error string
}

// FunaiDraw 读一次判据、**最多抽一次**（`page/lottery`）—— 含**写请求**。
//
// 门槛不成立 ⇒ **一个写请求都不发**（读那一枪仍会发，它是判据的唯一来源）。
//
// # 错误只有一种：读判据那一枪失败
//
// 那时整趟没有结论可报，交回错误（编排层会多发一条 `errored` 事件、
// 把原文写进 `runs.message`）。抽奖那一枪的失败**不**走这条路，折进
// [FunaiOutcome] 的 [FunaiFailed] —— 理由见那里。
//
// # `clock` 为什么是入参
//
// 活动期是判据的一部分，而本仓库禁止业务代码直接 `time.Now()`（[core.Clock] 的说明）。
// `clock == nil` 时报错而不是"当成在期"：没有时钟就是判不了，
// 而判不了还发写请求会花掉今天唯一那一次。
//
// # 不带 H5 `deviceId`
//
// 实拍两条请求都带 `deviceid` **头**，但归档里那个值已脱敏（连形态都核对不了）。
// 服务端是否要它**未验证** ⇒ 传 `nil`（缺省不发）。要测它：拿一个真账号在同一天内
// 各发一次"带 / 不带 deviceid"的 `page/getmodule`，比对 `code` 与 `result`
// —— 只能在活动窗口内做，窗外补不了样。
func FunaiDraw(
	ctx context.Context, src *SignInSessionSource, clock core.Clock,
) (FunaiOutcome, *core.Error) {
	if clock == nil {
		return FunaiOutcome{}, core.NewInternal("趣玩AI没有时钟：活动起止判不了，不能凭猜发抽奖请求")
	}
	module, perr := do(ctx, src, func(s SignInSession) (upstream.FunaiModule, *core.Error) {
		return src.up.AppFunaiModule(ctx, s.jwt, s.referer, nil)
	})
	if perr != nil {
		return FunaiOutcome{}, perr
	}

	plan := PlanFunai(module, clock.Now())
	readings := funaiReadingsOf(module)
	if plan != FunaiPlanDraw {
		return FunaiOutcome{Status: FunaiNothing, Plan: plan, ModuleID: module.ModuleID,
			Readings: readings}, nil
	}

	lottery, perr := do(ctx, src, func(s SignInSession) (upstream.FunaiLottery, *core.Error) {
		// 模块 id 一律取**这一次** `page/getmodule` 给的那个：
		// 它与判据同批，写死了就会在换期时打到过期模块上。
		return src.up.AppFunaiLottery(ctx, s.jwt, s.referer, module.ModuleID, nil)
	})
	if perr != nil {
		return FunaiOutcome{
			Status: FunaiFailed, Plan: plan, ModuleID: module.ModuleID,
			Readings: readings, Error: perr.Error(),
		}, nil
	}
	return FunaiOutcome{
		Status: FunaiDone, Plan: plan, ModuleID: module.ModuleID,
		Readings: readings, PrizeName: lottery.PrizeName,
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// FunaiRun 跑一次趣玩AI抽奖。
type FunaiRun struct {
	SignInWorld
}

// FunaiResult 是一个账号这一趟的实况。
//
// ⚠ 读判据失败**不**在这一格里 —— 它作为 `*core.Error` 交回编排层
// （那条路多发一条 `errored` 事件，比在这里折成失败说得更清楚，
// 与 [PrizesResult] / [ReceiveResult] 同一口径）。
type FunaiResult struct {
	Account core.AccountID
	Status  FunaiStatus
	// Plan 是门槛判定。[FunaiNothing] 时它**就是**原因（见 [FunaiPlan.Reason]）。
	Plan FunaiPlan
	// PrizeName 是上游报的奖品名，`nil` = 上游没说（≠ 没中奖）。
	PrizeName *string
	// Reason 是 [FunaiNothing] 的人话依据（由 [Plan.Reason] 填）。
	Reason string
	// Readings 是判据原值，随 [FunaiOutcome] 一路带到这里（为什么进小结见 [FunaiReadings]）。
	Readings FunaiReadings
	// Error 是 [FunaiFailed] 的失败原因。
	Error string
}

// RunAccount 跑**一个账号**的这一笔。
//
// 取不到票 / 读判据失败 ⇒ 返回错误（这一趟没走到能报结论的那一步）；
// 判完之后的一切结果都落成 [FunaiResult] 的三档，**不**回错误 ——
// 否则"今天已经抽过了"会变成历史里的一条红色。
func (r FunaiRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (FunaiResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return FunaiResult{Account: account}, perr
	}
	outcome, perr := FunaiDraw(ctx, src, r.Clock)
	if perr != nil {
		// 同时把结论摆成失败：走错误那条路时编排层仍会取 `SummaryLine`，
		// 留一个零值会渲染成一句没人看得懂的话（与 `ClaimRun.multiple` 同一处置）。
		return FunaiResult{Account: account, Status: FunaiFailed, Error: perr.Error()}, perr
	}

	result := FunaiResult{
		Account: account, Status: outcome.Status, Plan: outcome.Plan,
		PrizeName: outcome.PrizeName, Readings: outcome.Readings, Error: outcome.Error,
	}
	switch outcome.Status {
	case FunaiNothing:
		result.Reason = outcome.Plan.Reason()
		// 念 [SummaryLine] 而不是自己拼一遍前半句：读数那一截只有一个来源，
		// 否则将来小结加了数、播报还停在旧的那句话上。
		r.progress(core.LevelInfo, result.SummaryLine())
	case FunaiDone:
		r.progress(core.LevelInfo, "趣玩AI已抽一次："+result.SummaryLine())
	}
	return result, nil
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 里面只有档位、奖品名与上游给的计数，没有任何凭据形态的东西。
// 念计数而不解释它 —— 判据语义未验证（见 [FunaiReadings]），
// "drawTimes=1 drawsUsed=1"翻成"今天还能抽 0 次"就是把猜测写成断言。
func (res FunaiResult) SummaryLine() string {
	line := ""
	switch res.Status {
	case FunaiDone:
		if res.PrizeName == nil || *res.PrizeName == "" {
			line = "趣玩AI已抽一次（上游没报奖品名）"
		} else {
			line = "趣玩AI抽到「" + *res.PrizeName + "」"
		}
	case FunaiFailed:
		if res.Error == "" {
			line = "趣玩AI抽奖失败"
		} else {
			line = "趣玩AI抽奖失败：" + res.Error
		}
	default:
		// Reason 没填时退回按档位说：这一档不发请求，写成失败会让历史里多一条红色。
		line = "趣玩AI没抽：" + res.Reason
		if res.Reason == "" {
			line = "趣玩AI没抽：" + res.Plan.Reason()
		}
	}
	if res.Status != FunaiDone {
		// ⚠ **抽成了那一档不念读数**：这几格是 `page/getmodule` 的**抽奖之前**读的，
		// 抽成之后 `drawsUsed` 已经变了，念出来会被读成"抽完还剩这些"——
		// 与 `NoticeRun.project` 不许把领之前的槽当复查结果念，是同一类错。
		if readings := res.Readings.Line(); readings != "" {
			return line + "（读数：" + readings + "）"
		}
	}
	return line
}

// RunReport 把三档结论映射到 [core.RunOutcome]。
//
// 只有 [FunaiFailed] 落失败；抽到了与三种"没发请求"都算成功。
// 这里**没有** [core.OutcomeManualRequired] 那一档：趣玩AI的"做任务挣次数"要人去
// App 里用各个 AI 工具，但本能力只承诺"每天该抽的那一次抽掉"，
// 不承诺把任务做完 ⇒ 没有一件"必须人去做、程序代劳不了"的事要挂出来
// （要做那条线得先有任务清单的判据，见 [FunaiOutcome] 的 `ModuleID` 同源响应里那 16 项任务）。
func (res FunaiResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	if res.Status == FunaiFailed {
		return core.NewRunReportFailed(summary)
	}
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
