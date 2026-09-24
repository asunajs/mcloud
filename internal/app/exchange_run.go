package app

// 兑换的**链**：读清单 → 判 → （按需等整点）→ 换 deviceId → 取滑块题面 → 本地解算 →
// 下一单 → 复查余额差。判定部分在 `exchange_plan.go`。
//
// # 这一条是全仓唯一"发出去就收不回来"的链
//
// `exchangeV3` 真扣云朵、**不可逆**，而请求里**没有任何幂等键**（`oid` 由服务端每次新生成，
// 见 [upstream.ExchangeOrder.OrderID]）。所以本文件的全部结构都可以归结成一件事：
// **每一枪发出去之前，说得出它是被哪一条判据放行的；每一枪发出去之后，说得出结果凭什么判。**
//
// # 链上的枪序（与实拍对得上）
//
//	1  GET  /ycloud/signin/page/exchangeList   清单：`limit` 与三个余量、价格
//	2  GET  /ycloud/signin/page/infoV3         余额（判"够不够"，也是成交后的对照基准）
//	3  GET  /user/... 设备指纹上报              换 H5 `deviceId`（下单要回传它）
//	4  POST /ycloud/auth-service/slide/getSlide 取滑块题面（每次下单前现取）
//	5  POST /ycloud/signin/page/exchangeV3      下单
//	6  GET  /ycloud/signin/page/infoV3          复查余额（判据链第 2 条）
//
// 实拍四条 `exchangeV3` 每一条前面都紧跟一条 `getSlide`（抓包 §6 第 4 条），
// 所以 4、5 是**成对**的：`514` 重来 = 再取一题 + 再下一单，而不是拿旧 offset 重发。
//
// # `jwttoken` 使用公共来源（临时态，不是终判）
//
// 这一条链走 [SignInWorld.Source] 那套签到页的换票配方（[RecipeSignInPage]）。
// 理由与证据：抓包里那笔**真成交**的兑换（`251230053`，扣 1800 豆）就是借签到页那条会话打的
// （`docs/capture/app-exchange-samples.txt` §1 与 §5.4 写明用的是 `OpenSignInSession` 那一份票），
// ⇒ 公共票在这条链上**已实测可用**。
// 但"兑换页自己有没有特有换票参数"**没抓过**（清单与下单两条请求都只带 `jwttoken` 头，
// 看不出上游怎么校验它的出处）⇒ 按退路走（口径见 `AGENTS.md` §1.12）。
// ⇒ 抓到特有取法时**主动更正**这一处调用点并改掉这段说明，不留"注释说公共、代码走特有"。
//
// # 两处与参考实现的**刻意**不同
//
//   - **不发 `visitlog/journaling` 那两条埋点**。旧实现在下单前打两条埋点、中间 `sleep(500)`，
//     且埋点失败会**中断整个兑换**（`business-feature-inventory.md` §2.5 记的那条两路径不一致）。
//     兑换是否依赖这两条埋点，**无任何证据**（我们的成功样本里没带它们也一样成交 ——
//     抓包 §5.4 那一笔就是直接 `getSlide` → `exchangeV3`）。⇒ 这是 **agent 侧默认（非用户要求）**，
//     登记成"待用户拍板的问题"；要核它：同一件商品各发一单（一条埋点都不带 / 带两条），
//     比对 `code` 与余额差 —— 只能在有可兑奖品且愿意真扣豆时做。
//   - **下单后复查余额**。旧实现只看 `code === 0 && result.prizeName`。
//     而抓包 §5.4 把判据钉成了三条（`code 0` + 两次 `infoV3.total` 差 == 商品价格 + 账目/待领取），
//     只看 `code` 的写法在"上游说成功却没扣豆"这种情形下会**报出一条绿的假成功**。
//
// # 重试口径（写操作零自动重试）
//
// 只有 `514`（滑块算错）可以再来一轮，且**上限 [ExchangeSliderRounds] 轮**：那三条实拍
// 期间余额始终没动（抓包 §6 第 7 条），代价为零。其余一律不重发 ——
// 特别是传输层错误：**发出去超时的这一笔可能已经成交**，重发的表现是同一件扣两次豆。
// ⚠ 90001（未登录）那一档的"刷票 + 重放一次"在会话层（[SignInSessionSource.Call]），
// 不在本文件；它成立的前提是"未登录的请求根本没进到扣豆那一步"，
// 那是 `AGENTS.md` §1.12 定的口径（一账号一页一票，撞 90001 只刷那一页）。

import (
	"context"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ExchangeSliderRounds 是"取题 → 解算 → 下单"最多跑几轮 = 2。
//
// 出处：旧实现 `code === 514` 时"重取滑块 + 再下单一次"（一次，不是循环）。
// ⚠ 为什么第二轮**不**换成"再取题但不下单"：第二次 `514` 之后再取题就只是白花一次
// 滑块额度（滑块接口计失败次数，见 [core.SlideMaxDeviation] 那段）。
const ExchangeSliderRounds = 2

// ExchangeSolve 是解算滑块题面的注入点 —— 与 [core.SolveSlide] 同签名。
type ExchangeSolve func(picturePNG, puzzlePNG []byte) (core.SlideSolution, error)

// ExchangeWaitFunc 是"等到某个时刻"的注入点（必须可被 ctx 取消）。
type ExchangeWaitFunc func(ctx context.Context, until time.Time) error

// ExchangeDeviceIDFunc 是换 H5 `deviceId` 的注入点。
//
// 做成闭包而不是把 [SignInWorld] 传进来：这一枪要**上报设备指纹**，
// 只有全部判据都放行之后才值得打（见 [Exchange] 里那段取用时机）。
type ExchangeDeviceIDFunc func(ctx context.Context) (string, *core.Error)

// ExchangeOptions 是这一次兑换的输入与三个注入点。
type ExchangeOptions struct {
	// Prize 是参数 `prizeIdOrName`：纯数字按 prizeId 找，其余按商品名精确找。
	Prize string
	// SkipWait 为 false 时，若当前时刻离整点还远就先等到整点（[PlanExchangeWait]）。
	// ⚠ "整点"是参考实现抢兑那一场的时序取向，**不是**上游的硬条件 ——
	// 我们自己的成功兑换发生在 12:16（抓包 §5.4）。默认参数是 `true`（直接按现在判）。
	SkipWait bool

	// DeviceID 为 nil ⇒ 这一趟**不发**下单（下单体里那个 `deviceId` 键必须有值）。
	DeviceID ExchangeDeviceIDFunc
	// Wait 为 nil ⇒ 用 [DefaultExchangeWait]（生产路径不会"忘了注入"而跳过等待）。
	Wait ExchangeWaitFunc
	// Solve 为 nil ⇒ 用 [core.SolveSlide]。
	Solve ExchangeSolve
}

// ─────────────────────────────────────────────────────────────────────────────
// 结论档位
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeStatus 是这一趟的结局。
type ExchangeStatus int

const (
	// ExchangeStatusNotAttempted 是"没走到能报结论的那一步"（取票失败、清单读失败…）。
	//
	// ⭐ 它**刻意占住零值**：本链唯一的写请求花的是真豆，而"漏填 Status 的结果结构体"
	// 如果读成 [ExchangeStatusOrdered]，界面上就是一条绿色的"已兑到"。
	// 这一档在 [ExchangeResult.RunReport] 里落**失败**，不会静默。
	ExchangeStatusNotAttempted ExchangeStatus = iota
	// ExchangeStatusOrdered 是**下单成功且余额差对得上**（判据链两条都满足）。
	ExchangeStatusOrdered
	// ExchangeStatusSkipped 是**判据不成立 ⇒ 一条写请求都没发**。
	// 它是成功还是部分成功，看的是"用户要的那件到底兑没兑到"（见 RunReport 那段分流）。
	ExchangeStatusSkipped
	// ExchangeStatusAlreadyExchanged 是上游回 `2301 重复兑奖`：这一件此前已经兑过，
	// 本次没有重复发奖 ⇒ 目标其实已经达成，不算失败。
	ExchangeStatusAlreadyExchanged
	// ExchangeStatusSliderUnresolved 是**滑块这一关没过**：两个口径分歧太大而没算出来
	// （一枪都没发），或 `[ExchangeSliderRounds]` 轮都被上游判错。
	ExchangeStatusSliderUnresolved
	// ExchangeStatusNotTargetUser 是上游回 `407`（实拍那句 `msg` 是「非移动用户不可领奖」）。
	//
	// ⚠ 它是**账号级**闸门：两件不同奖品回的是同一个码（抓包 §6）。
	ExchangeStatusNotTargetUser
	// ExchangeStatusUnknown 是**结果判不了**：下单那枪遇到传输层错误（可能已成交也可能没有），
	// 或上游回 `code 0` 而余额差与价格**对不上**。⇒ 必须人去核对，程序不能替它编一个结论。
	ExchangeStatusUnknown
	// ExchangeStatusFailed 是上游明确回了失败（`415`/`420`/`421`/`610`/`614`/`615`…），
	// 以及 `501`（那是我们自己的代码漏带了 offset —— 领域层本不该走到那里）。
	ExchangeStatusFailed
)

// ExchangeOutcome 是 [Exchange] 的实况：链上每一个环节的读数都在这里。
type ExchangeOutcome struct {
	Status ExchangeStatus
	// Plan 是最后**生效**的那一次判定（等待之后重判过就是重判那份）。
	Plan ExchangePlan
	// Prize 是判据来自的那一件（清单读数，含价格与名称）。
	Prize upstream.ExchangePrize
	// Balance 是下单**之前**那次余额读数。
	Balance int64
	// AfterBalance 是下单**之后**复查的那次读数；nil = 没读到（不判，见 [ExchangeStatusOrdered]）。
	AfterBalance *int64

	// WaitedUntil 非 nil ⇒ 这一趟先等到这个时刻（`skipWait == false` 那条路）。
	WaitedUntil *time.Time
	// Rounds 是"取题 → 解算"跑了几轮；Attempts 是**实际发出的下单枪数**。
	//
	// ⭐ 分开数是因为 `Attempts == 0` 是本文件最重要的一条不变式：
	// 凡判据没过 / 算不出 offset，一枪都不能发（测试就钉这个）。
	Rounds   int
	Attempts int
	// Offset 是最后一枪带出去的 `puzzleOffset`；nil ⇒ 没发过单。
	Offset *int64
	// DeviceIDFailed 是换不到 `deviceId` 的原因（有值 ⇒ 一枪没发）。
	DeviceIDFailed string

	// Code / Message 是上游回的业务码与那句原文（`msg` 只进小结与日志，不进 API 响应）。
	Code    int64
	Message string
	// Order 是成交那一份（只在 [ExchangeStatusOrdered] / 余额差没核到时非 nil）。
	Order *upstream.ExchangeOrder
	// Error 是本地失败原因（解算报错、等待被取消…），与上游码是两回事。
	Error string
}

// Spent 算"这一趟看见余额少了多少"；核不到（缺任一次读数）时回 false。
func (o ExchangeOutcome) Spent() (int64, bool) {
	if o.AfterBalance == nil || o.Prize.Price == nil {
		return 0, false
	}
	return o.Balance - *o.AfterBalance, true
}

// ─────────────────────────────────────────────────────────────────────────────
// 链
// ─────────────────────────────────────────────────────────────────────────────

// judge 是"读清单 + 读余额 + 判"这一段的产物。
//
// 独立成一个函数只为了一件事：`skipWait == false` 等完整点之后要**原样再跑一遍**
// （清单与余额都是时刻量，等了一小时的那份判据不能继续用）。
// 写成两段会漂移 —— 漂移的表现是"等完之后的那一轮少查了余额"，而它不报错。
type judge struct {
	list    upstream.ExchangeList
	prize   upstream.ExchangePrize
	balance int64
	plan    ExchangePlan
}

func judgeExchange(
	ctx context.Context, src *SignInSessionSource, clock core.Clock, query string,
) (judge, *core.Error) {
	list, perr := do(ctx, src, func(s SignInSession) (upstream.ExchangeList, *core.Error) {
		return src.up.AppExchangeList(ctx, s.jwt, s.referer)
	})
	if perr != nil {
		return judge{}, perr
	}
	prize, plan := LocateExchangePrize(list, query)
	if plan.Blocks() {
		return judge{list: list, plan: plan}, nil
	}
	// 余额只在**定位成功**之后才读：查无此件时那一枪只是白打（`infoV3` 也是账号级请求，
	// 少打一次就少一分风控噪声）。
	status, perr := SignInStatus(ctx, src)
	if perr != nil {
		return judge{}, perr
	}
	return judge{
		list: list, prize: prize, balance: status.BeanTotal,
		plan: PlanExchange(prize, status.BeanTotal, clock.Now()),
	}, nil
}

// skipped 把"判据没过"折成结论（一枪都没发）。
func skipped(g judge, plan ExchangePlan, outcome ExchangeOutcome) ExchangeOutcome {
	outcome.Status = ExchangeStatusSkipped
	outcome.Plan = plan
	outcome.Prize = g.prize
	outcome.Balance = g.balance
	return outcome
}

// Exchange 跑一次兑换：**最多**下一单（真扣豆）。
//
// # 错误与结论的分界
//
// 只有"**没走到能报结论的那一步**"才回 `*core.Error`：取票之后的只读查询失败
// （清单、余额、第一次取题）、入参本身不成立。凡已经判过 / 已经发过单的，
// 一律折成 [ExchangeOutcome] 的档位 —— 否则"这件本月已兑过"会在历史里
// 变成一条红色的errored，而它是一句正常答复（与 [FunaiDraw] 同一口径）。
//
// # 一枪都不发的三条闸
//
// 判据没过、换不到 `deviceId`、offset 算不出来。前两条各自有原因字段，
// 第三条记在 [ExchangeOutcome.Error] 里（`Attempts` 必须仍是 0，测试钉这条）。
func Exchange(
	ctx context.Context, src *SignInSessionSource, clock core.Clock, opts ExchangeOptions,
) (ExchangeOutcome, *core.Error) {
	if src == nil {
		return ExchangeOutcome{}, core.NewInternal("兑换没有会话源")
	}
	if clock == nil {
		return ExchangeOutcome{}, core.NewInternal("兑换没有时钟：整点等待与商品时间窗都判不了")
	}
	if strings.TrimSpace(opts.Prize) == "" {
		return ExchangeOutcome{}, core.NewBadRequest("兑换没给奖品：prizeIdOrName 是必填")
	}
	if opts.Solve == nil {
		opts.Solve = core.SolveSlide
	}
	if opts.Wait == nil {
		opts.Wait = DefaultExchangeWait(clock)
	}

	g, perr := judgeExchange(ctx, src, clock, opts.Prize)
	if perr != nil {
		return ExchangeOutcome{Status: ExchangeStatusNotAttempted, Error: perr.Error()}, perr
	}
	outcome := ExchangeOutcome{Plan: g.plan, Prize: g.prize, Balance: g.balance}
	if g.plan.Blocks() {
		return skipped(g, g.plan, outcome), nil
	}

	if !opts.SkipWait {
		wait := PlanExchangeWait(clock.Now())
		if wait.Needed {
			until := wait.Until
			outcome.WaitedUntil = &until
			if perr := opts.Wait(ctx, until); perr != nil {
				return skipped(g, g.plan, withError(outcome, perr.Error())), nil
			}
			// 等到点之后**重读重判**：清单与余额都是时刻量，等完这一小时里
			// 别人可能已经把池子兑空、而上游只有一句 `415` 等着。
			g, perr = judgeExchange(ctx, src, clock, opts.Prize)
			if perr != nil {
				return ExchangeOutcome{Status: ExchangeStatusNotAttempted, Error: perr.Error()}, perr
			}
			outcome.Plan, outcome.Prize, outcome.Balance = g.plan, g.prize, g.balance
			if g.plan.Blocks() {
				return skipped(g, g.plan, outcome), nil
			}
		}
	}

	if opts.DeviceID == nil {
		// 下单体里那个 `deviceId` 键必须有值：实拍四条每一条都带了非空值（抓包 §5.2）。
		// ⚠ "少了它会回什么码"**没有样本** ⇒ 不猜，直接不发。
		outcome.DeviceIDFailed = "这一趟没接上 deviceId 来源（下单硬需要它，一枪都没发）"
		return skipped(g, g.plan, outcome), nil
	}
	deviceID, derr := opts.DeviceID(ctx)
	if derr != nil {
		outcome.DeviceIDFailed = derr.Error()
		return skipped(g, g.plan, outcome), nil
	}
	if strings.TrimSpace(deviceID) == "" {
		outcome.DeviceIDFailed = "换到的 deviceId 是空串（不发，避免打到未定义的那一档）"
		return skipped(g, g.plan, outcome), nil
	}

	for round := 1; round <= ExchangeSliderRounds; round++ {
		puzzle, perr := do(ctx, src, func(s SignInSession) (upstream.SlidePuzzle, *core.Error) {
			return src.up.AppGetSlide(ctx, s.jwt, s.referer)
		})
		if perr != nil {
			if round == 1 {
				// 一枪未发，且连题都没有 ⇒ 这一趟确实什么都没办成，交回错误。
				return ExchangeOutcome{
					Status: ExchangeStatusNotAttempted, Plan: g.plan, Prize: g.prize,
					Balance: g.balance, Error: perr.Error(),
				}, perr
			}
			// 第二轮取题失败**不**推翻第一轮的 `514`：那一枪确实被上游判错了，
			// 而重试的机会已经用完。
			break
		}
		outcome.Rounds = round

		solution, serr := opts.Solve(puzzle.PicturePNG, puzzle.PuzzlePNG)
		if serr != nil {
			// 算不出来就**不发**：拿 `0` 或上一次的答案去凑，是拿真豆去赌（旧实现
			// 取题失败时用 `0` 兜底照常下单，那条路径我们不照搬 —— `0` 会回什么**未验证**）。
			outcome.Status = ExchangeStatusSliderUnresolved
			outcome.Error = serr.Error()
			if round > 1 {
				outcome.Error = "重取题后算不出 offset：" + serr.Error()
			}
			return outcome, nil
		}
		offset := int64(solution.Offset)
		outcome.Offset = &offset

		result, perr := do(ctx, src, func(s SignInSession) (upstream.ExchangeResult, *core.Error) {
			return src.up.AppExchangeV3(ctx, s.jwt, s.referer, deviceID, g.prize.PrizeID, &offset)
		})
		outcome.Attempts++
		if perr != nil {
			// ⚠ 这一枪**可能已经成交**：请求发出去了，只是我们没读到响应。
			// 不重发（写操作零自动重试），并报成"结果未知"。
			outcome.Status = ExchangeStatusUnknown
			outcome.Error = perr.Error()
			return outcome, nil
		}
		outcome.Code, outcome.Message = result.Code, result.Message

		switch result.Code {
		case upstream.CodeOK:
			outcome.Order = result.Order
			after, perr := SignInStatus(ctx, src)
			if perr != nil {
				// 上游说成功了，而余额差**没核到** ⇒ 仍报成交（`code 0` + `oid` 就是证据），
				// 但把"没核到"留在结论里（小结会念出来，见 SummaryLine）。
				outcome.Status = ExchangeStatusOrdered
				outcome.Error = "余额复查失败：" + perr.Error()
				return outcome, nil
			}
			balanceAfter := after.BeanTotal
			outcome.AfterBalance = &balanceAfter
			if g.prize.Price != nil && balanceAfter == outcome.Balance-*g.prize.Price {
				outcome.Status = ExchangeStatusOrdered
				return outcome, nil
			}
			// 差值对不上：**不能**默认成功。要么上游异步扣豆还没落到这个读数上，
			// 要么这一笔根本没成交（而它回了 0）。两种处置都得人去核对。
			outcome.Status = ExchangeStatusUnknown
			return outcome, nil

		case upstream.ExchangeCodeSliderFailed:
			// 代价为零（三条 `514` 期间余额始终没动，抓包 §6 第 7 条）⇒ 允许再来一轮。
			if round == ExchangeSliderRounds {
				outcome.Status = ExchangeStatusSliderUnresolved
				return outcome, nil
			}

		case upstream.ExchangeCodeDuplicated:
			outcome.Status = ExchangeStatusAlreadyExchanged
			return outcome, nil

		case upstream.ExchangeCodeNotTargetUser:
			outcome.Status = ExchangeStatusNotTargetUser
			return outcome, nil

		default:
			outcome.Status = ExchangeStatusFailed
			return outcome, nil
		}
	}
	outcome.Status = ExchangeStatusSliderUnresolved
	return outcome, nil
}

// withError 只在"没走到发单"那几条早退路上补一句原因，不改档位。
func withError(outcome ExchangeOutcome, message string) ExchangeOutcome {
	outcome.Error = message
	return outcome
}

// DefaultExchangeWait 是 [ExchangeOptions.Wait] 为 nil 时用的那份等待。
//
// 它是**分片**睡的（每次最多 [exchangeWaitSlice]）：一段长等待里 ctx 可能被取消，
// 而 `time.After(一小时)` 那种写法要么取消不了、要么到点前醒不来（时钟被人调过就更离谱）。
// 每一片都重新拿一次 [core.Clock]，所以"离整点还有多久"始终是现算的。
func DefaultExchangeWait(clock core.Clock) ExchangeWaitFunc {
	return func(ctx context.Context, until time.Time) error {
		for {
			remaining := until.Sub(clock.Now())
			if remaining <= 0 {
				return nil
			}
			if remaining > exchangeWaitSlice {
				remaining = exchangeWaitSlice
			}
			timer := time.NewTimer(remaining)
			select {
			case <-ctx.Done():
				timer.Stop()
				return core.NewTimeout("等整点开抢时被取消：" + ctx.Err().Error())
			case <-timer.C:
			}
		}
	}
}

// exchangeWaitSlice 是分片sleep的上限。
//
// ⚠ 为什么是 2 秒而不是"一次睡到点"：等待最长约一小时，而这段时间里
// 停止服务 / 取消这一趟都得在**一两秒内**生效（编排层的 ctx 就是这么设计的）。
const exchangeWaitSlice = 2 * time.Second

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeRun 跑一次兑换（真扣豆）。
type ExchangeRun struct {
	SignInWorld
	// Prize 是参数 `prizeIdOrName`（必填）。
	Prize string
	// SkipWait 与参数面同名：false ⇒ 先等到整点再判一次。
	SkipWait bool
}

// ExchangeResult 是一个账号这一趟的实况。
type ExchangeResult struct {
	Account core.AccountID
	ExchangeOutcome
}

// RunAccount 跑**一个账号**的这一笔。
//
// ⚠ 下单失败**不**回错误（折成档位），否则"这件本月已兑过"会在历史里多一条红色；
// 只有"没走到能报结论的那一步"（取票 / 清单 / 余额 / 第一次取题）才交回错误。
func (r ExchangeRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (ExchangeResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return ExchangeResult{
			Account: account,
			ExchangeOutcome: ExchangeOutcome{
				Status: ExchangeStatusNotAttempted, Error: perr.Error(),
			},
		}, perr
	}
	outcome, perr := Exchange(ctx, src, r.Clock, ExchangeOptions{
		Prize:    r.Prize,
		SkipWait: r.SkipWait,
		DeviceID: func(ctx context.Context) (string, *core.Error) {
			return r.DeviceIDFor(ctx, account)
		},
	})
	if perr != nil {
		// 与 [FunaiRun.RunAccount] 同一处置：走错误那条路时编排层仍会取 `SummaryLine`，
		// 留一个零值会渲染成一句没人看得懂的话。
		return ExchangeResult{Account: account, ExchangeOutcome: outcome}, perr
	}
	result := ExchangeResult{Account: account, ExchangeOutcome: outcome}
	switch outcome.Status {
	case ExchangeStatusOrdered, ExchangeStatusAlreadyExchanged:
		r.progress(core.LevelInfo, result.SummaryLine())
	case ExchangeStatusSkipped:
		r.progress(core.LevelInfo, "没兑换："+result.Plan.Reason())
	default:
		r.progress(core.LevelWarn, result.SummaryLine())
	}
	return result, nil
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 里面只有档位、商品名、价格与余额这些**账号内**的数，没有任何凭据形态的东西。
// 上游那句 `msg` 会被带出去（它是上游写给人看的文案），但它不含凭据 —— 见
// [upstream.ExchangeResult.Message] 那段。
func (res ExchangeResult) SummaryLine() string {
	name := "未知商品"
	if res.Prize.Name != nil && strings.TrimSpace(*res.Prize.Name) != "" {
		name = strings.TrimSpace(*res.Prize.Name)
	}
	price := ""
	if res.Prize.Price != nil {
		price = "，价 " + strconv.FormatInt(*res.Prize.Price, 10) + " 豆"
	}
	switch res.Status {
	case ExchangeStatusOrdered:
		line := "兑到「" + name + "」" + price
		if spent, ok := res.Spent(); ok {
			return line + "；余额 " + strconv.FormatInt(res.Balance, 10) + " → " +
				strconv.FormatInt(*res.AfterBalance, 10) + "（差 " + strconv.FormatInt(spent, 10) + "）"
		}
		if res.Error != "" {
			return line + "；⚠ 余额差没核到：" + res.Error
		}
		return line + "；⚠ 余额差没核到"
	case ExchangeStatusAlreadyExchanged:
		return "「" + name + "」此前已兑过（上游回「重复兑奖」），本次没有重复发奖"
	case ExchangeStatusSkipped:
		if res.DeviceIDFailed != "" {
			return "没兑换：" + res.DeviceIDFailed
		}
		return "没兑换：" + res.Plan.Reason()
	case ExchangeStatusSliderUnresolved:
		if res.Attempts == 0 {
			return "没兑换：滑块解算没过（一枪都没发）" + tail(res.Error)
		}
		return "没兑到：滑块验证 " + strconv.Itoa(res.Attempts) + " 次都被上游判错" + tail(res.Error)
	case ExchangeStatusNotTargetUser:
		return "没兑到：上游回「" + orUpstream(res.Message, "非移动用户不可领奖") +
			"」—— 这一族是账号级闸门，这个账号兑不了兑换页的奖品"
	case ExchangeStatusUnknown:
		line := "兑换结果判不了"
		if res.Code != 0 {
			line += "（上游回 " + strconv.FormatInt(res.Code, 10) + "）"
		}
		if spent, ok := res.Spent(); ok && res.Prize.Price != nil {
			line += "：余额差 " + strconv.FormatInt(spent, 10) + " 与价格 " +
				strconv.FormatInt(*res.Prize.Price, 10) + " 对不上"
		} else if res.Error != "" {
			line += "：" + res.Error
		}
		return line + "，需要去 App 核对余额与待领取奖品"
	case ExchangeStatusFailed:
		return "兑换失败：上游回 " + strconv.FormatInt(res.Code, 10) +
			"「" + orUpstream(res.Message, "没给原因") + "」"
	}
	return "兑换没判成" + tail(res.Error)
}

// orUpstream 优先用上游那句原文，没有时退回我们自己的说法。
//
// ⚠ 不把空 `msg` 渲染成「」—— 界面上那一对空引号会被读成"上游说了句空话"。
func orUpstream(message, fallback string) string {
	if strings.TrimSpace(message) == "" {
		return fallback
	}
	return strings.TrimSpace(message)
}

// tail 把本地失败原因拼成"（…）"，空原因就不留括号。
func tail(reason string) string {
	if strings.TrimSpace(reason) == "" {
		return ""
	}
	return "（" + strings.TrimSpace(reason) + "）"
}

// RunReport 把档位映射到 [core.RunReport]。
//
// 分流只有一条判据：**用户要的那一件到底兑到没有**。
//   - 兑到了（含上游判为"重复兑奖"= 本来就有了）⇒ 成功；
//   - 判据没过、且没过的是**本账号已经兑过**（`limit` 2 / 4）⇒ 也算成功 ——
//     那一句"本月已兑"说的就是目标已达成（与 [FunaiPlanAlreadyDrawn] 落成功同一口径）；
//   - 其余"判据没过"（查无此件 / 余额不够 / 库存没了 / 时间不对 / 判不了）⇒ **部分成功**：
//     目标没达成，但**没有**一件"必须人去做、程序代劳不了"的事 ——
//     ⚠ 这里刻意不用 [core.OutcomeManualRequired]，因为挂不出一个具体的动作；
//   - 滑块没过 / 结果判不了 ⇒ [core.OutcomeManualRequired]：这两条**确实**要人去
//     App 里点那一下（手动滑一次、或核对这一笔到底成交没有）；
//   - 上游明确回失败、以及一枪都没发就取不到题 ⇒ 失败。
func (res ExchangeResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	manual := func(instruction string, target core.ManualTarget) core.RunReport {
		account := res.Account
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			capabilities.CodeExchange, &account, instruction, target,
		)}, summary)
	}
	switch res.Status {
	case ExchangeStatusOrdered, ExchangeStatusAlreadyExchanged:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	case ExchangeStatusSkipped:
		if res.Plan == ExchangePlanExchangedThisPrize || res.Plan == ExchangePlanExchangedThisGroup {
			return core.RunReport{
				Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
			}
		}
		return core.RunReport{
			Outcome: core.OutcomePartialSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	case ExchangeStatusSliderUnresolved:
		return manual(
			"到 App 的签到页手动兑一次这件：自动解算的滑块 offset 上游判错（或两个口径分歧太大而没发单），"+
				"而滑块题面只在页面上给",
			core.TargetMobileApp,
		)
	case ExchangeStatusUnknown:
		return manual(
			"到 App 核对云朵余额与待领取奖品，确认刚才这一笔到底成交没有："+
				"请求里的幂等键是空的，程序分不清「没成交」与「成交了但响应丢了」",
			core.TargetMobileApp,
		)
	case ExchangeStatusNotTargetUser:
		return core.NewRunReportFailed(summary)
	default:
		return core.NewRunReportFailed(summary)
	}
}
