package app

// 兑换的**判定层**：在清单里定位一件 → 逐档判"现在能不能发这一笔真扣豆的请求"。
//
// 对应旧实现里的 `exchange`（`packages/core/src/tasks/exchange/`），
// 逐条摘要见 `docs/business-feature-inventory.md` §2.5。
//
// # 为什么判定要独立成纯函数（链在 `exchange_run.go`）
//
// 下单是**不可逆消耗**：这一族没有任何幂等键，`oid` 由服务端生成，发出去就不收回来
// （判据与实测见 `docs/capture/app-exchange-samples.txt` §5.2 / §6）。代价因此**完全不对称** ——
// 该不发时发了 = 花掉真云朵；该发时不发 = 少一次机会，重跑还有。
// 于是每一条"发"都必须指得出是**哪一档判据**放行的，而逐档的可观察后果只有纯函数才测得准
// （反向改坏哪一档，哪一条用例红）。
//
// # 这一层只读四样东西
//
// `limit`（本账号状态）、三个余量数（`count` / `dailyRemainderCount` / `minRemainderCount`）、
// `pOrder`（价格，两拼法已在解析层收口）与余额快照。
// **不读** `type` / `nextTimeStatus` / `prizeSectionStatus` / `riskUserEnable` 那一族：
// 前两个只有一档实拍（`1035` / `1`），后一组在参考项目里**全仓无读取点** ——
// 拿望文生义的名字当判据，错的时候不报错，只会多发或少发一笔真扣豆的请求。

import (
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

// ExchangePlan 是"这一件现在能不能下单"的档位。
//
// 除 [ExchangePlanGo] 之外每一档都说得出**为什么不是别的一档**：这些句子会直接变成
// 界面上的一行，"跳过"那一类如果不说清是哪条判据挡的，用户就不知道该等明天还是换一件。
type ExchangePlan int

const (
	// ExchangePlanGo 是**可以下单**：下面每一条判据都有值、且都放行。
	//
	// ⚠ 它是本类型的**零值**（与 [FunaiPlanDraw] 同形），于是"忘了填 plan 的结构体"
	// 读起来就是"可以下单"。这里靠两条守住：判定值只能出自 [PlanExchange] /
	// [LocateExchangePrize]（链上没有别处会造一个 ExchangePlan），
	// 而"零值当放行"这条后果由 `exchange_run_test.go` 的"没判过就一发都没发"用例盯着。
	ExchangePlanGo ExchangePlan = iota
	// ExchangePlanNotFound 是清单里**没有**入参说的那件（既不是这里的 prizeId，也没有同名商品）。
	ExchangePlanNotFound
	// ExchangePlanAmbiguous 是**同名多件** —— 不能凭猜挑一件下单。
	ExchangePlanAmbiguous
	// ExchangePlanOffShelf 是 `onLine` 明确不是 `1`（下架）。⚠ 非 `1` 的取值**零样本**。
	ExchangePlanOffShelf
	// ExchangePlanLimitMissing 是上游**没给** `limit` ⇒ 本账号状态判不了。
	//
	// ⚠ 单独一档且**不发**：`limit` 是 H5 那枚按钮的第一判据，缺了它就没有任何依据
	// 说这件可兑；当成"多半可兑"会白发一笔真扣豆的请求。
	ExchangePlanLimitMissing
	// ExchangePlanLimitTomorrow 是 `limit == 1`（H5 文案"明日再来"）。⚠ 本轮样本里**一件都没有**。
	ExchangePlanLimitTomorrow
	// ExchangePlanExchangedThisPrize 是 `limit == 2`：**这一件本身**本月已兑过。
	//
	// ⭐ 这一档有字段依据：成功兑掉 `251230053` 之后它的 `limit` 由 `0` 变成 `2`（抓包 §5.4）。
	ExchangePlanExchangedThisPrize
	// ExchangePlanLimitThree 是 `limit == 3`：**两处文案冲突**那一档
	// （H5 说"已兑换，查看奖品"，参考项目说"暂不可用"）⇒ 不挑一个信，一样不发。
	ExchangePlanLimitThree
	// ExchangePlanExchangedThisGroup 是 `limit == 4`：**同组别的件**本月已兑（组名额用掉了）。
	ExchangePlanExchangedThisGroup
	// ExchangePlanLimitUnknown 是 `limit` 是这五档之外的值 —— 上游改了形态，别猜。
	ExchangePlanLimitUnknown
	// ExchangePlanMinRemainderZero 是 `minRemainderCount == 0`：H5 在这一档显示"奖品被抢光啦"。
	ExchangePlanMinRemainderZero
	// ExchangePlanSoldOut 是 `count <= 0`：总投放余量为零。
	ExchangePlanSoldOut
	// ExchangePlanDailyStockEmpty 是 `dailyRemainderCount <= 0`：今日余量为零。
	ExchangePlanDailyStockEmpty
	// ExchangePlanPriceMissing 是上游**没给**价格 ⇒ "余额够不够"这一条判不了。
	//
	// ⚠ 当成"不要豆"会白发一笔；当成"不够"会漏兑。缺判据只有停下来这一种处置。
	ExchangePlanPriceMissing
	// ExchangePlanInsufficientCloud 是余额快照低于价格（H5 那句 `x < t.pOrder ⇒ "AI豆不够啦"`）。
	ExchangePlanInsufficientCloud
	// ExchangePlanNotStarted 是 `startTime` 解得出、且还没到。
	ExchangePlanNotStarted
	// ExchangePlanEnded 是 `endTime` 解得出、且已经过了。
	ExchangePlanEnded
)

// Reason 是这一档说给用户的那半句（前面会接上"没兑换："）。
//
// ⚠ 只复述**判据本身**，不解释上游语义：`limit` 五档里有两档只有文案没有字段依据、
// 三个余量数是时刻量（会因为**别的用户**在同一分钟里兑掉而变化，抓包 §5.4）。
// 具体数值（价格、余额、那个不认识的 limit）不在这里 —— 链把它们放进
// [ExchangeResult]，由 `SummaryLine` 那一段拼，纯函数才留得住可测性。
func (p ExchangePlan) Reason() string {
	switch p {
	case ExchangePlanNotFound:
		return "兑换清单里没有这一件（prizeId 与同名商品都没找到）"
	case ExchangePlanAmbiguous:
		return "清单里同名的商品不止一件，不能凭猜下单"
	case ExchangePlanOffShelf:
		return "这件商品的上架状态不是「在架」（onLine 非 1）"
	case ExchangePlanLimitMissing:
		return "上游没给这件的可兑状态（limit 缺失），判不了，本次没发请求"
	case ExchangePlanLimitTomorrow:
		return "上游状态是 limit=1（H5 那句文案是「明日再来」）"
	case ExchangePlanExchangedThisPrize:
		return "这一件本月已经兑过了（limit=2）"
	case ExchangePlanLimitThree:
		return "上游状态是 limit=3，它的含义两处说法互相冲突（H5「已兑换」/ 旧实现「暂不可用」），不猜，本次没发请求"
	case ExchangePlanExchangedThisGroup:
		return "同组奖品本月已兑过（limit=4，组内互斥）"
	case ExchangePlanLimitUnknown:
		return "上游给了一个我们不认识的 limit 取值，本次没发请求"
	case ExchangePlanMinRemainderZero:
		return "这件的余量阈值已经是 0（H5 在这一档显示「奖品被抢光啦」）"
	case ExchangePlanSoldOut:
		return "上游报的当前剩余为 0（count）"
	case ExchangePlanDailyStockEmpty:
		return "上游报的今日余量为 0（dailyRemainderCount）"
	case ExchangePlanPriceMissing:
		return "上游没给这件商品的价格，余额够不够判不了，本次没发请求"
	case ExchangePlanInsufficientCloud:
		return "余额不够这件的价格，本次没发请求"
	case ExchangePlanNotStarted:
		return "这件的开售时间还没到（startTime 晚于当前时刻）"
	case ExchangePlanEnded:
		return "这件的兑换时间已过（endTime 早于当前时刻）"
	}
	return "可以下单"
}

// Blocks 报告这一档是不是"不该发请求"。
//
// 写成方法而不是让各调用点比 `plan != ExchangePlanGo`：链上有一处要把"放行"当成
// 继续往下走的条件、另两处（等待之后重判、界面渲染）只想知道"这算不算拦住了"，
// 一个词表比六处字面量比较不容易漏。
func (p ExchangePlan) Blocks() bool { return p != ExchangePlanGo }

// Slug 是这一档的**机器可读**名字，只给选品接口（
// `GET /api/accounts/{id}/exchange-prizes`）的响应体用。
//
// # 为什么不把 [ExchangePlan] 直接 marshal 成一个数
//
// 那等于把"档位的**顺序**"变成对外契约：加一档（或按 §7.1 的便利把某档挪个位置）
// 会让前端那张颜色表**静默错配** —— 界面把"本月已兑"染成"可兑"的绿。
// 串就不一样了：新档位没登记 slug 时 `default` 那一支会露馅，而测试
// （`TestEveryExchangePlanSlugIsDistinctAndDeclared`）要求十七档**逐一**有名字。
//
// ⚠ 这些串**不是**上游回的字段，是我们自己的档位名。
func (p ExchangePlan) Slug() string {
	switch p {
	case ExchangePlanGo:
		return "go"
	case ExchangePlanNotFound:
		return "not_found"
	case ExchangePlanAmbiguous:
		return "ambiguous"
	case ExchangePlanOffShelf:
		return "off_shelf"
	case ExchangePlanLimitMissing:
		return "limit_missing"
	case ExchangePlanLimitTomorrow:
		return "limit_tomorrow"
	case ExchangePlanExchangedThisPrize:
		return "exchanged_this_prize"
	case ExchangePlanLimitThree:
		return "limit_three_conflict"
	case ExchangePlanExchangedThisGroup:
		return "exchanged_this_group"
	case ExchangePlanLimitUnknown:
		return "limit_unknown"
	case ExchangePlanMinRemainderZero:
		return "min_remainder_zero"
	case ExchangePlanSoldOut:
		return "sold_out"
	case ExchangePlanDailyStockEmpty:
		return "daily_stock_empty"
	case ExchangePlanPriceMissing:
		return "price_missing"
	case ExchangePlanInsufficientCloud:
		return "insufficient_cloud"
	case ExchangePlanNotStarted:
		return "not_started"
	case ExchangePlanEnded:
		return "ended"
	}
	// 越界取值（例如 JSON 里塞个 99 再反序列化）：**不能**回 "go"。
	// 这一档没被测试覆盖（写不出"将来某个新档位"的断言），所以按 §1.3 声明：
	// **未被断言**，只是不让它落在零值那一档上冒充"可下单"。
	return "unknown"
}

// ─────────────────────────────────────────────────────────────────────────────
// 定位：入参那一句 → 清单里的那一件
// ─────────────────────────────────────────────────────────────────────────────

// LocateExchangePrize 按参数 `prizeIdOrName` 在清单里定位一件。
//
// 返回的 plan 只会是 [ExchangePlanGo]（找到且唯一）/ [ExchangePlanNotFound] /
// [ExchangePlanAmbiguous] 三档，其余档位由 [PlanExchange] 负责 —— 分开放是因为
// 它们**回答的是两个问题**："你说的是哪一件" 与 "那一件现在能不能兑"。
//
// # 匹配规则：纯数字按 prizeId，其余按名称**精确相等**
//
// 与参考实现同一取向（它是"入参是字符串时先查清单做名称→ID 解析"）。
// 名称匹配取**精确相等**而不是"包含"：包含匹配在同名前缀的一堆商品里
// （实拍就有"移动云盘100G云空间月卡 / 年卡 / 白银会员月卡 / 季卡…"这一类）
// 会把"我要月卡"打到**别的**一件上 —— 而这是真扣豆。宁可说"没找到"。
//
// ⚠ "旧实现用的是不是包含匹配"**未核**（本仓库没有那份 TS 源码，
// 只有 `docs/business-feature-inventory.md` §2.5 那段摘要）。要核：拿到那份源码后
// 比 `resolvePrizeId` 的匹配表达式；本仓这条规则的代价上界是"少兑一次"，安全侧。
func LocateExchangePrize(list upstream.ExchangeList, query string) (upstream.ExchangePrize, ExchangePlan) {
	trimmed := strings.TrimSpace(query)
	if trimmed == "" {
		return upstream.ExchangePrize{}, ExchangePlanNotFound
	}
	// 纯数字 ⇒ 按 prizeId。这里**不**用 `strconv.ParseInt` 的错误分支决定走哪条：
	// "12 34" 这种带空格的串既不是 id 也不是名字，两边走哪条都是猜 ⇒ 落到名称匹配，
	// 名称里没有带空格的商品自然回"没找到"，那句话比"按 id 猜了一半"诚实。
	if prizeID, err := strconv.ParseInt(trimmed, 10, 64); err == nil {
		if prize, found := list.Find(prizeID); found {
			return prize, ExchangePlanGo
		}
		return upstream.ExchangePrize{}, ExchangePlanNotFound
	}

	var matches []upstream.ExchangePrize
	for _, item := range list.All() {
		if item.Name == nil {
			continue // 没名字的那件不参与名称匹配（也不报错：清单照样能按 id 兑）
		}
		if strings.TrimSpace(*item.Name) == trimmed {
			matches = append(matches, item)
		}
	}
	switch len(matches) {
	case 1:
		return matches[0], ExchangePlanGo
	case 0:
		return upstream.ExchangePrize{}, ExchangePlanNotFound
	default:
		return upstream.ExchangePrize{}, ExchangePlanAmbiguous
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判定（纯函数，不碰网络）
// ─────────────────────────────────────────────────────────────────────────────

// PlanExchange 判"这一件、这个余额、这个时刻，能不能下单"。**纯函数**，不碰网络。
//
// # 匹配顺序 = 从"这件不存在"到"我不够豆"
//
// 先状态（`onLine` → `limit`）、再供给（三个余量）、再需求（价格与余额）、最后时间。
// 两个方向的考虑：
//   - **具体原因优先**：`limit=2`（本月已兑）比"库存不足"更少出现，也更可执行；
//   - **便宜的挡在前面**：`limit` 与余量都来自**同一次**清单读取，不额外花请求；
//     余额那一条要用**另一次**读取的结果（链上先取后判），判不过就不必再去换 deviceId
//     与取滑块题面 —— 那两枪各有副作用（设备指纹上报、滑块失败计数）。
//
// ⚠ 时间档**排在最后**是刻意的：`startTime` / `endTime` 的形态只有一档实拍，
// 而它俩解不出来时**不挡**（见 [exchangeWindowOf]）—— 把一条可能因形态而误判的
// 规则放在最前，会让所有真判据（limit / 余量 / 余额）一句都念不出来。
func PlanExchange(
	prize upstream.ExchangePrize, balance int64, now time.Time,
) ExchangePlan {
	amount := balance
	return planExchange(prize, &amount, now)
}

// PlanExchangeWithoutBalance 是"手上**没有**余额读数"时的那一份判定 ——
// 界面上的选品器（`GET /api/accounts/{id}/exchange-prizes`）用的就是它。
//
// # 为什么不干脆多打一枪去读余额
//
// `infoV3` 是**账号级**请求（与 `exchangeList` 不同一条链），多打一枪就多一分风控噪声，
// 而"看一眼清单"这一件事的收益不足以付它：余额那一栏页面上本来就有别的路
// （`GET /api/accounts` 与 cloud-stats）。⚠ 代价写清楚：**这一档判不出
// [ExchangePlanInsufficientCloud]**，所以界面上"可兑"比下单前的真判据**宽**，
// 用户点了仍然可能被链挡下（那一次挡下是有钱包错误码的，不会静默）。
//
// 与 [PlanExchange] **共用同一个** [planExchange] 内核，所以两条路的档位顺序
// 不会漂移 —— 分两份写的话，"选品器说可兑、链说不可兑"这种分歧将来没人能对齐。
func PlanExchangeWithoutBalance(
	prize upstream.ExchangePrize, now time.Time,
) ExchangePlan {
	return planExchange(prize, nil, now)
}

func planExchange(
	prize upstream.ExchangePrize, balance *int64, now time.Time,
) ExchangePlan {
	if prize.OnLine != nil && *prize.OnLine != 1 {
		return ExchangePlanOffShelf
	}
	switch plan := planExchangeLimit(prize.Limit); plan {
	case ExchangePlanGo:
		// limit 放行（== 0），继续判下面的。
	default:
		return plan
	}
	switch {
	case prize.MinRemainderCount != nil && *prize.MinRemainderCount == 0:
		return ExchangePlanMinRemainderZero
	case prize.Count != nil && *prize.Count <= 0:
		return ExchangePlanSoldOut
	case prize.DailyRemainderCount != nil && *prize.DailyRemainderCount <= 0:
		return ExchangePlanDailyStockEmpty
	}
	if prize.Price == nil {
		return ExchangePlanPriceMissing
	}
	if balance != nil && *prize.Price > *balance {
		return ExchangePlanInsufficientCloud
	}
	return planExchangeWindow(prize, now)
}

// planExchangeLimit 把 `limit` 五档翻成档位。放行时返回 [ExchangePlanGo]。
//
// ⚠ 这里**不**复用 [upstream.ExchangePrize.CanExchangeNow]：它把 `limit == 0` 与
// `minRemainderCount != 0` 合成一个 bool（H5 那枚按钮的口径），而本函数要的是
// "哪一条挡的" —— 合成之后 `limit=2` 与"余量为 0"说的是同一句"不可兑"，
// 界面上就成了"不能兑"，用户不知道该等明天还是该换一件。
func planExchangeLimit(limit *int64) ExchangePlan {
	if limit == nil {
		return ExchangePlanLimitMissing
	}
	switch *limit {
	case upstream.ExchangeLimitAvailable:
		return ExchangePlanGo
	case upstream.ExchangeLimitTomorrow:
		return ExchangePlanLimitTomorrow
	case upstream.ExchangeLimitExchanged:
		return ExchangePlanExchangedThisPrize
	case upstream.ExchangeLimitUnknown3:
		return ExchangePlanLimitThree
	case upstream.ExchangeLimitGroupExchanged:
		return ExchangePlanExchangedThisGroup
	default:
		return ExchangePlanLimitUnknown
	}
}

// exchangeWindow 是奖品自身时间窗三态。**必须有"判不了"这一格**（同 [funaiWindow]）：
// 两值布尔会把"上游没给 / 形态不认识"落成"在期"或"不在期"之一，两个都不对。
type exchangeWindow int

const (
	exchangeWindowIn exchangeWindow = iota
	exchangeWindowOut
	// exchangeWindowUnknown 是**这一边判不了**。
	//
	// ⚠ 与趣玩AI那一族**刻意不同**的处置：那里判不了就**不发**（写请求会花掉当天
	// 唯一一次额度）；这里判不了**放行**。理由是这一族的硬判据是 `limit` 与余额，
	// 而 `startTime` / `endTime` 更像"这一件挂在页面上的时间" ——
	// ⚠ 这句是**推断**，没有样本能说清越界时上游回什么码。放行一侧的代价是
	// 可能白发一笔并被上游挡（那会落在链的失败/未知档，不会静默扣豆而不吭声），
	// 挡住一侧的代价是**所有**没给窗口的商品都兑不了。
	exchangeWindowUnknown
)

// exchangeWindowOf 只判**解得出来**的那一边；两边都解不出来就是判不了。
//
// 串的形态与时区见 [upstream.ExchangeTimeLayout] / [upstream.ExchangeTimeLocation]；
// [upstream.ParseExchangeTime] 解不出来只回 false，**不报错** —— 那正是这里要的三态输入。
func exchangeWindowOf(prize upstream.ExchangePrize, now time.Time) exchangeWindow {
	known, out := 0, false
	if prize.StartTime != nil {
		if start, ok := upstream.ParseExchangeTime(*prize.StartTime); ok {
			known++
			out = out || now.Before(start)
		}
	}
	if prize.EndTime != nil {
		if end, ok := upstream.ParseExchangeTime(*prize.EndTime); ok {
			known++
			out = out || now.After(end)
		}
	}
	if known == 0 {
		return exchangeWindowUnknown
	}
	if out {
		return exchangeWindowOut
	}
	return exchangeWindowIn
}

// planExchangeWindow 把时间窗三态落到档位：判不了与在期都放行，只有**明确越界**才挡。
//
// 越界那一侧还要再分一次（"还没到"与"已经过了"处置完全不同：等一场 vs 换一件）。
func planExchangeWindow(prize upstream.ExchangePrize, now time.Time) ExchangePlan {
	if exchangeWindowOf(prize, now) != exchangeWindowOut {
		return ExchangePlanGo
	}
	if prize.StartTime != nil {
		if start, ok := upstream.ParseExchangeTime(*prize.StartTime); ok && now.Before(start) {
			return ExchangePlanNotStarted
		}
	}
	return ExchangePlanEnded
}

// ─────────────────────────────────────────────────────────────────────────────
// 等待窗口（`skipWait == false` 才用得上）
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeWaitLeadIntoHour 是"整点后多久之内算还在上一场里"= 20 秒。
//
// 出处是参考实现的那条组合判据（有效窗口 `hh:59:40 ~ (hh+1):00:15`，
// `getMsToNextHour` 在 `nowMs < 20000` 时不再等待，见
// `docs/business-feature-inventory.md` §2.5）。
// ⚠ **上游是否真的整点开抢，我们没有证据**：我们自己那条成功兑换发生在
// `12:16`（抓包 §5.4，CST），根本不在整点窗口里 ⇒ "整点开抢"**不是**上游的硬条件，
// 至少对那一件不是。所以这一档只服务 `skipWait=false`（想赶下一场）那条路，
// 默认参数（`skipWait=true`）完全不碰它。
const ExchangeWaitLeadIntoHour = 20 * time.Second

// ExchangeWait 是"这一趟要不要等、等到什么时候"。
type ExchangeWait struct {
	Needed bool
	// Until 是等待目标时刻（`Needed == false` 时无意义）。
	Until time.Time
}

// PlanExchangeWait 按参考实现的取向算等待目标：
// 整点后 20 秒内不等（还在上一场尾巴里），否则等到**下一个整点**。
//
// # 不减"提前量"（旧实现那个 `offset = 10` 毫秒）
//
// 注册表**没有** `offset` 这一栏（`internal/capabilities/registry.go` 的 `exchangeParams()`
// 只有 `prizeIdOrName` + `skipWait`），而 10 毫秒小于我们一次请求的往返 ——
// 拿它当判据只会让"什么时候发出去"这件事不可复现。⚠ 要不要把这一栏补回参数面
// 已登记成待用户拍板的问题，见 `docs/status.md` 本批那一段。
//
// # 时区不影响结果
//
// 目标取的是"下一个整点"这个**时刻**，而 [upstream.ExchangeTimeLocation] 是整小时偏移，
// 所以按 UTC 截断与按东八区截断给出同一个时刻（半小时偏移的时区才会错开，本仓不涉及）。
func PlanExchangeWait(now time.Time) ExchangeWait {
	hour := now.Truncate(time.Hour)
	if now.Sub(hour) < ExchangeWaitLeadIntoHour {
		return ExchangeWait{Needed: false}
	}
	return ExchangeWait{Needed: true, Until: hour.Add(time.Hour)}
}
