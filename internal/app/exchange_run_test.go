package app

// [Exchange] / [ExchangeRun] 的用例：链上「发了几枪、按什么顺序、什么算成交」。
//
// # 这一条链的测试口径与别的链不同
//
// 别的链可以「照发不误、失败了重跑」，这一条**每一枪都花真豆**，所以本文件最重要的一组
// 断言是**反向**的：判据没过 / 算不出 offset / 换不到 deviceId ⇒ `exchangeV3` **一次都不许打**。
// 「少发一枪」在本链上从不构成损失，而「多发一枪」没有回滚入口（请求里没有幂等键）。
//
// # 这一轮**不**真打上游
//
// 兑换族的上游层已有实拍对拍（`internal/upstream/app_exchange_test.go`，含 `content-length`
// 字节级），本文件只在夹具/回放层验链的走法；「链跑成真扣豆」那一格仍标 `待验证`
// （怎么验：拿一个可兑、低价、且愿意花豆的账号，按 §5.4 那三条判据对账 ——
// 见 `docs/capture/app-exchange-samples.txt`）。

import (
	"context"
	"encoding/base64"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 链上的四个端点。写成常量是因为每条用例都要查两头：「该打的打了几次」与「不该打的一次都没打」。
const (
	exchangeListEndpoint   = "/ycloud/signin/page/exchangeList"
	exchangeStatusEndpoint = "/ycloud/signin/page/infoV3"
	getSlideEndpoint       = "/ycloud/auth-service/slide/getSlide"
	exchangeV3Endpoint     = "/ycloud/signin/page/exchangeV3"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// exchangePrizeJSON 造一件商品的 JSON 片段。默认全部判据放行（limit 0 / 有价格 / 有余量 / 在架），
// 数值取自抓包 §5.1 那一件（`260630012` / 50 豆 / 每日 5000 余 4998）。
//
// `overrides` 是**成对**给的额外键（`"limit", "2"`），值是原样拼进去的 JSON 片段：
// 要造脏数据（形态不对的串、缺字段）时不必再抄一遍默认值。
//
// ⚠ 它按**键覆盖**上面那批默认值，而不是在对象尾部追加一个重复键 ——
// 重复键怎么算由解码器决定，夹具不该依赖那种行为（同一份夹具里出现两个 `limit`，
// 读出来是哪个，换一次 Go 版本就可能变）。
func exchangePrizeJSON(prizeID int64, name string, price int64, overrides ...string) string {
	keys := []string{
		"prizeId", "prizeName", "groupId", "pOrder", "limit",
		"count", "totalCount", "dailyCount", "dailyRemainderCount", "minRemainderCount", "onLine",
	}
	values := map[string]string{
		"prizeId":             strconv.FormatInt(prizeID, 10),
		"prizeName":           `"` + name + `"`,
		"groupId":             `0`,
		"pOrder":              strconv.FormatInt(price, 10),
		"limit":               `0`,
		"count":               `399291`,
		"totalCount":          `400000`,
		"dailyCount":          `5000`,
		"dailyRemainderCount": `4998`,
		"minRemainderCount":   `1`,
		"onLine":              `1`,
	}
	if len(overrides)%2 != 0 {
		panic("exchangePrizeJSON 的 overrides 必须成对（键、值、键、值……）")
	}
	for i := 0; i < len(overrides); i += 2 {
		key := overrides[i]
		if _, seen := values[key]; !seen {
			keys = append(keys, key)
		}
		values[key] = overrides[i+1]
	}
	fields := make([]string, 0, len(keys))
	for _, key := range keys {
		fields = append(fields, `"`+key+`":`+values[key])
	}
	return `{` + strings.Join(fields, ",") + `}`
}

// exchangeListJSON 把若干件拼成一份清单响应。
//
// ⚠ 顶层 `result` 必须是**按 groupId 分组的对象**（实拍形状；解析层对形状变化是硬响的）。
func exchangeListJSON(items ...string) string {
	return `{"code":0,"msg":"success","result":{"0":[` + strings.Join(items, ",") + `]}}`
}

// exchangeStatusJSON 造 `infoV3`：只关心余额那一个数（其余三个必需整数字段照签到夹具补齐）。
//
// ⚠ 这里**不**复用 `statusPayload`：那位把 `"total":10496` 钉死在尾部，
// 再往里塞一个 total 就成了重复键，读出来是 10496 而不是我们要的那个数
// —— 表现是"余额永远够"，判据那一档静默失效。
func exchangeStatusJSON(total int64) string {
	return `{"code":0,"msg":"success","result":{"cal":[],"total":` +
		strconv.FormatInt(total, 10) + `,"signInPoints":3,"signCount":7,"monthDays":7}}`
}

// slideJSON 造一份能过解析的滑块题面（两段是**带填充的标准 base64**，实拍口径）。
//
// 图的内容是假的 —— 解算器由 `Solve` 注入点顶替，本文件不测图像算法（那在 `core` 层）。
func slideJSON(tag string) string {
	pad := func(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }
	return `{"code":0,"msg":"success","result":{` +
		`"picture":"` + pad("PICTURE:"+tag) + `","puzzle":"` + pad("PUZZLE:"+tag) + `",` +
		`"picWidth":"680","picHeight":"400","puzzleWidth":"96"}}`
}

const (
	exchangeOKJSON = `{"code":0,"msg":"success","result":{"oid":1272221744,"prizeId":260630012,` +
		`"prizeName":"移动云盘100万tokens叠加包","uorder":50}}`
	exchangeSliderFailJSON = `{"code":514,"msg":"验证失败"}`
	exchangeDuplicatedJSON = `{"code":2301,"msg":"重复兑奖"}`
	exchangeNotTargetJSON  = `{"code":407,"msg":"非移动用户不可领奖"}`
	exchangeSoldOutJSON    = `{"code":415,"msg":"奖品单日已耗尽"}`
)

// okSolve 是「每次都算得出 offset」的解算注入点。
func okSolve(offset int) ExchangeSolve {
	return func(_, _ []byte) (core.SlideSolution, error) {
		return core.SlideSolution{Offset: offset}, nil
	}
}

// errSolve 是「两个口径分歧太大 ⇒ 解算器自己拒答」（[core.SlideMaxDeviation] 那一档）。
func errSolve(_, _ []byte) (core.SlideSolution, error) {
	return core.SlideSolution{}, core.NewInternal("两个口径分歧 7px（上限 3）⇒ 不要提交")
}

func fixedDeviceID(value string) ExchangeDeviceIDFunc {
	return func(context.Context) (string, *core.Error) { return value, nil }
}

func failDeviceID(context.Context) (string, *core.Error) {
	return "", core.NewInternal("设备指纹上报失败")
}

// waitRecord 是等待注入点，顺手记下「被要求等到什么时候」。
type waitRecord struct {
	calls []time.Time
	err   error
}

func (w *waitRecord) fn() ExchangeWaitFunc {
	return func(_ context.Context, until time.Time) error {
		w.calls = append(w.calls, until)
		return w.err
	}
}

// exchangeChain 跑一次链，返回结论、那份按端点计数的脚本与错误。
func exchangeChain(
	t *testing.T, routes []liveTaskRoute, opts ExchangeOptions, clock core.Clock,
) (ExchangeOutcome, *liveTaskScript, *core.Error) {
	t.Helper()
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	src, perr := signInWorldFor(&poolSignInStore{*liveTaskStore()}, script).
		Source(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("取票失败：%v", perr)
	}
	outcome, err := Exchange(context.Background(), src, clock, opts)
	return outcome, script, err
}

// exchangeRoutes 拼一份「清单 + 余额」齐全的链上夹具，参数顺序**就是枪序**。
//
// `balances` 是 `infoV3` 的**读数序列**（队列用尽后重复最后一条，见 [liveTaskScript]）：
// ⚠ 成交那一路必须给**两个**数 —— 只给一个会让复查读到同一个值，差值恒为 0，
// 于是"余额差对不上价格"那一档判据会把这条用例自己打红。
// 题面与下单队列可不给（判据没过时用不上）。
func exchangeRoutes(list string, balances []int64, slide, order []string) []liveTaskRoute {
	statuses := make([]string, 0, len(balances))
	for _, total := range balances {
		statuses = append(statuses, exchangeStatusJSON(total))
	}
	routes := []liveTaskRoute{
		{exchangeListEndpoint, []string{list}},
		{exchangeStatusEndpoint, statuses},
	}
	if len(slide) > 0 {
		routes = append(routes, liveTaskRoute{getSlideEndpoint, slide})
	}
	if len(order) > 0 {
		routes = append(routes, liveTaskRoute{exchangeV3Endpoint, order})
	}
	return routes
}

// exchangeOptions 是「默认参数 + 两个必给注入点」的那份：可兑、不等整点、算得出 offset。
func exchangeOptions(prize string) ExchangeOptions {
	return ExchangeOptions{
		Prize:    prize,
		SkipWait: true,
		DeviceID: fixedDeviceID("DEV-89"),
		Solve:    okSolve(320),
	}
}

// prizeNamed 给小结用例造「有名字、有价格」的那一件。
func prizeNamed(name string, price int64) upstream.ExchangePrize {
	prize := upstream.ExchangePrize{Name: &name, Price: &price}
	return prize
}

// ─────────────────────────────────────────────────────────────────────────────
// 一条都不发：本文件最重要的一组
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeSendsNothingWhenPrizeIsMissing：查无此件 ⇒ 连余额都不查。
//
// ⭐ 断言的是**枪序**：`infoV3` 也在「不该打」的名单里 —— 定位失败时读余额没有任何用途，
// 而它是账号级请求，少打一次就少一分风控噪声（链上那句注释就为了这一格）。
func TestExchangeSendsNothingWhenPrizeIsMissing(t *testing.T) {
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000}, nil, nil),
		exchangeOptions("999999999"), testClock())
	if err != nil {
		t.Fatalf("判据没过不是错误：%v", err)
	}
	if outcome.Status != ExchangeStatusSkipped || outcome.Plan != ExchangePlanNotFound {
		t.Fatalf("应落 Skipped/NotFound，实得 %v / %v", outcome.Status, outcome.Plan)
	}
	for _, endpoint := range []string{exchangeStatusEndpoint, getSlideEndpoint, exchangeV3Endpoint} {
		if got := script.count(endpoint); got != 0 {
			t.Errorf("%s 一次都不该打，实得 %d 次", endpoint, got)
		}
	}
	if got := script.count(exchangeListEndpoint); got != 1 {
		t.Errorf("清单该读一次，实得 %d 次", got)
	}
	if report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport(); report.Outcome !=
		core.OutcomePartialSuccess {
		t.Error("用户要的那件没找到 ⇒ 部分成功（既不是失败，也不是成功）")
	}
}

// TestExchangeSendsNothingWhenLimitBlocks：`limit=2`（本月已兑）⇒ 一枪不发，且算成功。
func TestExchangeSendsNothingWhenLimitBlocks(t *testing.T) {
	list := exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50, "limit", "2"))
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(list, []int64{1000}, nil, nil),
		exchangeOptions("260630012"), testClock())
	if err != nil {
		t.Fatalf("判据没过不是错误：%v", err)
	}
	if outcome.Plan != ExchangePlanExchangedThisPrize {
		t.Fatalf("应落 ExchangedThisPrize，实得 %v", outcome.Plan)
	}
	if got := script.count(exchangeV3Endpoint); got != 0 {
		t.Errorf("下单一次都不该发，实得 %d 次", got)
	}
	// 这一档的结论是**成功**：那句「本月已兑」说的就是目标已达成。
	if report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport(); report.Outcome !=
		core.OutcomeSuccess {
		t.Errorf("本月已兑应算成功，实得 %v", report.Outcome)
	}
}

// TestExchangeSendsNothingWhenBalanceIsShort：余额不够 ⇒ 不发，且不换 deviceId、不取题。
//
// ⭐ 反向改坏点：把余额那一档挪到「取题之后」，这一条就红（`getSlide` 变成 1 次）。
// 判「余额不够」之前先花掉一次滑块机会，是本链最没必要的浪费。
func TestExchangeSendsNothingWhenBalanceIsShort(t *testing.T) {
	deviceCalls := 0
	opts := exchangeOptions("260630012")
	opts.DeviceID = func(ctx context.Context) (string, *core.Error) {
		deviceCalls++
		return "DEV-89", nil
	}
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 1800)),
			[]int64{1000}, []string{slideJSON("a")}, nil),
		opts, testClock())
	if err != nil {
		t.Fatalf("判据没过不是错误：%v", err)
	}
	if outcome.Plan != ExchangePlanInsufficientCloud {
		t.Fatalf("余额 1000 对价格 1800 应判不够，实得 %v", outcome.Plan)
	}
	if got := script.count(getSlideEndpoint); got != 0 {
		t.Errorf("余额不够时不该取滑块题面，实得 %d 次", got)
	}
	if deviceCalls != 0 {
		t.Errorf("余额不够时不该上报设备指纹换 deviceId，实得 %d 次", deviceCalls)
	}
	if got := script.count(exchangeStatusEndpoint); got != 1 {
		t.Errorf("余额读一次就够（判完就停），实得 %d 次", got)
	}
}

// TestExchangeSendsNothingWhenOffsetCannotBeComputed：解算器拒答 ⇒ 一枪不发。
//
// 旧实现在取题失败时用 `0` 兜底**照常下单**（`business-feature-inventory.md` §2.5 那条）。
// 我们不照搬：`0` 会回什么**没有样本**，而那一枪花的是真豆。
func TestExchangeSendsNothingWhenOffsetCannotBeComputed(t *testing.T) {
	opts := exchangeOptions("260630012")
	opts.Solve = errSolve
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
		opts, testClock())
	if err != nil {
		t.Fatalf("解算失败折成结论，不该是错误：%v", err)
	}
	if outcome.Status != ExchangeStatusSliderUnresolved {
		t.Fatalf("应落 SliderUnresolved，实得 %v", outcome.Status)
	}
	if outcome.Attempts != 0 || script.count(exchangeV3Endpoint) != 0 {
		t.Errorf("算不出 offset 时一枪都不能发，实得 Attempts=%d / 端点 %d 次",
			outcome.Attempts, script.count(exchangeV3Endpoint))
	}
	if script.count(getSlideEndpoint) != 1 {
		t.Errorf("题面只取一次（没算出来就不必再取），实得 %d 次", script.count(getSlideEndpoint))
	}
	if report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport(); report.Outcome !=
		core.OutcomeManualRequired {
		t.Errorf("滑块没过要人去 App 里滑 ⇒ ManualRequired，实得 %v", report.Outcome)
	}
}

// TestExchangeSendsNothingWithoutDeviceID：换不到 deviceId ⇒ 不发，且**不取题**。
//
// 顺序在这里是省出来的：取题要占一次滑块机会，而缺 deviceId 时那一枪注定发不出去。
func TestExchangeSendsNothingWithoutDeviceID(t *testing.T) {
	for _, tc := range []struct {
		name string
		fn   ExchangeDeviceIDFunc
	}{
		{"上报失败", failDeviceID},
		{"换回来是空串", fixedDeviceID("")},
		{"没有注入点", nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			opts := exchangeOptions("260630012")
			opts.DeviceID = tc.fn
			outcome, script, err := exchangeChain(t,
				exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
					[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
				opts, testClock())
			if err != nil {
				t.Fatalf("换不到 deviceId 不是错误：%v", err)
			}
			if outcome.Status != ExchangeStatusSkipped || outcome.DeviceIDFailed == "" {
				t.Fatalf("应落 Skipped 并给出原因，实得 %v / %q", outcome.Status, outcome.DeviceIDFailed)
			}
			if outcome.Attempts != 0 || script.count(exchangeV3Endpoint) != 0 {
				t.Errorf("一枪都不能发，实得 %d 次", script.count(exchangeV3Endpoint))
			}
			if script.count(getSlideEndpoint) != 0 {
				t.Errorf("不该取题（白花一次滑块机会），实得 %d 次", script.count(getSlideEndpoint))
			}
			if !strings.HasPrefix((ExchangeResult{ExchangeOutcome: outcome}).SummaryLine(), "没兑换：") {
				t.Errorf("小结要说清是「没兑换」而不是「兑换失败」，实得 %q",
					(ExchangeResult{ExchangeOutcome: outcome}).SummaryLine())
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 成交与重试
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeHappyPathSendsOneOrderAndVerifiesBalance：走通那一趟的枪序与读数。
//
// ⭐ 「余额读了**两次**」是这条用例的重点之一：判「够不够」那一次不能当成交证据
// （成交后要拿差值对价格）。只读一次、只看 `code 0` 的写法，在「上游说成功却没扣豆」
// 时会报一条绿的假成功。
func TestExchangeHappyPathSendsOneOrderAndVerifiesBalance(t *testing.T) {
	// 两个读数 `1000 → 950`：差值正好等于价格 50，这是判据链的第 2 条（抓包 §5.4）。
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000, 950}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
		exchangeOptions("260630012"), testClock())
	if err != nil {
		t.Fatalf("走通的一趟不该报错：%v", err)
	}
	if outcome.Status != ExchangeStatusOrdered {
		t.Fatalf("应落 Ordered，实得 %v（%s）", outcome.Status, outcome.Error)
	}
	if outcome.Order == nil || outcome.Order.OrderID != 1272221744 {
		t.Errorf("该把订单号带出来（对账唯一线索），实得 %+v", outcome.Order)
	}
	for endpoint, want := range map[string]int{
		exchangeListEndpoint: 1, exchangeStatusEndpoint: 2, getSlideEndpoint: 1, exchangeV3Endpoint: 1,
	} {
		if got := script.count(endpoint); got != want {
			t.Errorf("%s 应打 %d 次，实得 %d 次", endpoint, want, got)
		}
	}
	if spent, ok := outcome.Spent(); !ok || spent != 50 {
		t.Errorf("余额差应核到 50，实得 %d / %v", spent, ok)
	}
	if outcome.Offset == nil || *outcome.Offset != 320 {
		t.Errorf("应提交解算出的 offset 320，实得 %v", outcome.Offset)
	}
	// ⭐ 真发出去的那一体：`puzzleOffset` 与 `deviceId` 都得上身（判据在请求上，不在返回值里）。
	bodies := script.requestBodies()
	for _, want := range []string{`"puzzleOffset":320`, `"deviceId":"DEV-89"`, `"prizeId":260630012`} {
		if !strings.Contains(bodies, want) {
			t.Errorf("下单体里应有 %s，实得 %q", want, bodies)
		}
	}
	if report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport(); report.Outcome !=
		core.OutcomeSuccess {
		t.Errorf("成交应算成功，实得 %v", report.Outcome)
	}
}

// TestExchangeRetriesSliderOnlyOnceAndRecomputesOffset：`514` ⇒ 重取题 + 重下一单，最多两轮。
//
// ⭐ 三处计数要一起看：「重发同一枪」（getSlide 1 次 / V3 2 次）与「每次下单前重新取题」
// （各 2 次）在上游两边都回同样的码时只有次数分得开；而实拍四条 `exchangeV3`
// 每一条前面都紧跟一条 `getSlide`（抓包 §6 第 4 条）。
func TestExchangeRetriesSliderOnlyOnceAndRecomputesOffset(t *testing.T) {
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			// 中间那一枪 `514` **没扣豆**，所以两个读数分在"判"与"成交后复查"两头。
			[]int64{1000, 950},
			[]string{slideJSON("first"), slideJSON("second")},
			[]string{exchangeSliderFailJSON, exchangeOKJSON}),
		exchangeOptions("260630012"), testClock())
	if err != nil {
		t.Fatalf("514 之后重来不是错误：%v", err)
	}
	if outcome.Status != ExchangeStatusOrdered {
		t.Fatalf("第二轮成交应落 Ordered，实得 %v", outcome.Status)
	}
	if outcome.Rounds != 2 || outcome.Attempts != 2 {
		t.Errorf("应跑两轮、发两枪，实得 %d / %d", outcome.Rounds, outcome.Attempts)
	}
	if got := script.count(getSlideEndpoint); got != 2 {
		t.Errorf("每一轮都该重新取题，实得 %d 次", got)
	}
	if got := script.count(exchangeV3Endpoint); got != 2 {
		t.Errorf("两轮各下一单，实得 %d 次", got)
	}
	// 余额只读两次：`514` 那一枪**没有**扣豆（三条实拍期间余额始终没动，抓包 §6 第 7 条），
	// 中间再查一次是白打 —— 只有 `code 0` 之后才复查。
	if got := script.count(exchangeStatusEndpoint); got != 2 {
		t.Errorf("余额应读两次（判一次 + 成交后复查一次），实得 %d 次", got)
	}
	if outcome.Code != 0 {
		t.Errorf("结论该记最后那一枪的码，实得 %d", outcome.Code)
	}
}

// TestExchangeStopsAfterSecondSliderFailure：第二次 `514` 就收手 ⇒ 需要人去滑。
func TestExchangeStopsAfterSecondSliderFailure(t *testing.T) {
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000},
			[]string{slideJSON("a"), slideJSON("b")},
			[]string{exchangeSliderFailJSON, exchangeSliderFailJSON}),
		exchangeOptions("260630012"), testClock())
	if err != nil {
		t.Fatalf("两轮都判错是结论，不是错误：%v", err)
	}
	if outcome.Status != ExchangeStatusSliderUnresolved || outcome.Attempts != 2 {
		t.Fatalf("应落 SliderUnresolved 且发了 2 枪，实得 %v / %d",
			outcome.Status, outcome.Attempts)
	}
	// ⚠ 上面两格写**字面量 2**而不是 `ExchangeSliderRounds`：判据是"最多两次"这件事本身。
	// 拿常量断言等于自我引用 —— 把上限改成 3 这里不会红（夹具的队列用尽后重复最后一条，
	// 第三轮同样失败），而第三轮的代价是**多发一枪 `exchangeV3`**（实测：改成 3 之后
	// 本文件所有用例仍全绿，2026-09-24 的反向验证 M5）。
	if got := script.count(getSlideEndpoint); got != 2 {
		t.Errorf("取题应 2 次（轮数上限），实得 %d 次", got)
	}
	if got := script.count(exchangeV3Endpoint); got != 2 {
		t.Errorf("下单最多两枪，实得 %d 次 —— 第三枪是多扣一次豆的风险敞口", got)
	}
	report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport()
	if report.Outcome != core.OutcomeManualRequired || len(report.ManualSteps) != 1 {
		t.Fatalf("要人去 App 里手动滑一次 ⇒ ManualRequired，实得 %v / %d 条待办",
			report.Outcome, len(report.ManualSteps))
	}
	step := report.ManualSteps[0]
	if step.Target != core.TargetMobileApp || step.Capability != "exchange" {
		t.Errorf("待办该指到移动 App 的兑换能力，实得 %v / %s", step.Target, step.Capability)
	}
}

// TestExchangeBusinessCodesAreDataNotRetried：认得的码各归各档，且**一律不重发**。
//
// ⚠ 这一条守的是「写操作零自动重试」：把 default 分支写成 `continue` 的后果
// 是同一件商品可能被扣多次（请求里没有幂等键）。
func TestExchangeBusinessCodesAreDataNotRetried(t *testing.T) {
	for _, tc := range []struct {
		name string
		body string
		want ExchangeStatus
	}{
		{"415 单日已耗尽", exchangeSoldOutJSON, ExchangeStatusFailed},
		{"2301 重复兑奖", exchangeDuplicatedJSON, ExchangeStatusAlreadyExchanged},
		{"407 非目标用户", exchangeNotTargetJSON, ExchangeStatusNotTargetUser},
	} {
		t.Run(tc.name, func(t *testing.T) {
			outcome, script, err := exchangeChain(t,
				exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
					[]int64{1000}, []string{slideJSON("a")}, []string{tc.body}),
				exchangeOptions("260630012"), testClock())
			if err != nil {
				t.Fatalf("业务码是数据不是错误：%v", err)
			}
			if outcome.Status != tc.want {
				t.Fatalf("应落 %v，实得 %v", tc.want, outcome.Status)
			}
			if got := script.count(exchangeV3Endpoint); got != 1 {
				t.Errorf("非 0 码一律不重发，实得 %d 次", got)
			}
			if got := script.count(getSlideEndpoint); got != 1 {
				t.Errorf("不该再取一次题，实得 %d 次", got)
			}
			if got := script.count(exchangeStatusEndpoint); got != 1 {
				t.Errorf("没成交就不该复查余额（白花一次读），实得 %d 次", got)
			}
		})
	}
}

// TestExchangeCodeZeroWithWrongBalanceDiffIsUnknown：`code 0` 而余额差对不上 ⇒ 结果未知。
//
// ⭐ 这是全链最「反直觉」的一格：上游明明说了成功。而判据链（抓包 §5.4）有**三条**，
// 余额差是其中唯一能证伪「钱没扣」的那一条 ⇒ 对不上就不能报成交，只能交回人来核。
func TestExchangeCodeZeroWithWrongBalanceDiffIsUnknown(t *testing.T) {
	outcome, script, _ := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
		exchangeOptions("260630012"), testClock())
	if outcome.Attempts != 1 || script.count(exchangeV3Endpoint) != 1 {
		t.Fatalf("该发一枪，实得 %d", outcome.Attempts)
	}
	// 上面的队列到 1000 就「重复最后一条」，所以复查读到的还是 1000 ⇒ 差值 0 ≠ 价格 50。
	if outcome.Status != ExchangeStatusUnknown {
		t.Fatalf("余额没动却回 code 0 ⇒ 应判「结果未知」，实得 %v", outcome.Status)
	}
	line := (ExchangeResult{ExchangeOutcome: outcome}).SummaryLine()
	if !strings.Contains(line, "余额差 0") || !strings.Contains(line, "去 App 核对") {
		t.Errorf("小结要说清差值对不上并指出去核对，实得 %q", line)
	}
	if report := (ExchangeResult{ExchangeOutcome: outcome}).RunReport(); report.Outcome !=
		core.OutcomeManualRequired {
		t.Errorf("结果未知必须挂出人要做的事，实得 %v", report.Outcome)
	}
}

// TestExchangeCodeZeroWithoutRecheckIsStillOrdered：复查那一枪读不到 ⇒ 不推翻成交。
//
// ⚠ 反方向也立着：这时小结里要留「余额差没核到」，不能念成「已核对」。
func TestExchangeCodeZeroWithoutRecheckIsStillOrdered(t *testing.T) {
	routes := exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
		[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON})
	// 把余额那条路由成「先给一个正常数、再给一份缺必需字段的」⇒ 复查报错，
	// 且不走 90001 那条刷票重放的路（那会多打几枪，把计数断言搞混）。
	routes[1] = liveTaskRoute{exchangeStatusEndpoint, []string{
		exchangeStatusJSON(1000),
		`{"code":0,"msg":"success","result":{"cal":[]}}`,
	}}
	outcome, script, err := exchangeChain(t, routes, exchangeOptions("260630012"), testClock())
	if outcome.Status != ExchangeStatusOrdered {
		t.Fatalf("上游回 0 + oid 就是成交证据，实得 %v（%v）", outcome.Status, err)
	}
	if outcome.AfterBalance != nil {
		t.Errorf("复查没成 ⇒ 不该有第二个读数，实得 %v", *outcome.AfterBalance)
	}
	line := (ExchangeResult{ExchangeOutcome: outcome}).SummaryLine()
	if !strings.Contains(line, "余额差没核到") {
		t.Errorf("小结要标明没核到，实得 %q", line)
	}
	if got := script.count(exchangeStatusEndpoint); got != 2 {
		t.Errorf("复查失败也不重读（一枪只判一次），实得 %d 次", got)
	}
}

// TestExchangeUnreadableOrderResponseNeverResends：下单那枪没读到有效响应 ⇒ 一枪，且「结果未知」。
//
// ⭐ 本文件最贵的一条：这一枪**可能已经成交**。重发的表现不是「多试一次」，
// 而是同一件奖品扣两次豆（或撞 `2301`，反而让人以为第一次没成）。
//
// ⚠ 这里用「200 + 不是 JSON 的响应体」模拟：[liveTaskScript] 只会回 200，
// 而"传输层失败"与"响应读不懂"在链上是**同一条错误通道**（都拿不到码），
// 落同一档。真要验 5xx 那一支，得换一份能回状态码的假客户端（`upstream` 层的用例）。
func TestExchangeUnreadableOrderResponseNeverResends(t *testing.T) {
	routes := exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
		[]int64{1000}, []string{slideJSON("a")},
		[]string{`<!doctype html><title>网关挂了</title>`})
	outcome, script, err := exchangeChain(t, routes, exchangeOptions("260630012"), testClock())
	if err != nil {
		t.Fatalf("分不清成交没有 ⇒ 折成结论，不该是错误：%v", err)
	}
	if outcome.Status != ExchangeStatusUnknown {
		t.Fatalf("应落 Unknown，实得 %v", outcome.Status)
	}
	if outcome.Attempts != 1 || script.count(exchangeV3Endpoint) != 1 {
		t.Errorf("绝不能重发，实得 Attempts=%d / 端点 %d 次",
			outcome.Attempts, script.count(exchangeV3Endpoint))
	}
	if outcome.AfterBalance != nil {
		t.Error("响应都没读到，不该有复查读数")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 等待整点（`skipWait == false`）
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeWaitsAndRejudges：等完整点要**重读重判**，旧判据不能继续用。
//
// 夹具故意让「等之前可兑、等之后 limit=2」，于是判据必须换成新那份 ——
// 少重读一次清单的表现是拿一小时前的 `limit` 去下一单。
func TestExchangeWaitsAndRejudges(t *testing.T) {
	rec := &waitRecord{}
	opts := exchangeOptions("260630012")
	opts.SkipWait = false
	opts.Wait = rec.fn()
	routes := exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
		[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON})
	routes[0] = liveTaskRoute{exchangeListEndpoint, []string{
		exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
		exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50, "limit", "2")),
	}}
	// 12:16（实拍那一笔的时刻）⇒ 离整点还远，需要等。
	clock := fixedClock{at: time.Date(2026, 9, 22, 4, 16, 0, 0, time.UTC)}
	outcome, script, err := exchangeChain(t, routes, opts, clock)
	if err != nil {
		t.Fatalf("等完之后判「不该兑」不是错误：%v", err)
	}
	want := time.Date(2026, 9, 22, 5, 0, 0, 0, time.UTC)
	if len(rec.calls) != 1 || !rec.calls[0].Equal(want) {
		t.Fatalf("应等一次、等到 05:00 UTC，实得 %v", rec.calls)
	}
	if outcome.WaitedUntil == nil || !outcome.WaitedUntil.Equal(want) {
		t.Errorf("结论里要留下「等到什么时候」，实得 %v", outcome.WaitedUntil)
	}
	if got := script.count(exchangeListEndpoint); got != 2 {
		t.Errorf("等完之后要重读清单，实得 %d 次", got)
	}
	if outcome.Plan != ExchangePlanExchangedThisPrize || outcome.Status != ExchangeStatusSkipped {
		t.Fatalf("应改用等完那份判据，实得 %v / %v", outcome.Plan, outcome.Status)
	}
	if got := script.count(exchangeV3Endpoint); got != 0 {
		t.Errorf("重判不该兑 ⇒ 一枪不发，实得 %d 次", got)
	}
}

// TestExchangeDoesNotWaitWhenSkipWaitIsTrue：默认参数**完全不碰**等待。
//
// ⚠ 时钟故意停在 04:16 —— 那是"需要等"的那一刻（实拍那一笔的本地 12:16），
// 所以这一条同时钉住两件事：没等、且**照样下了单**。
func TestExchangeDoesNotWaitWhenSkipWaitIsTrue(t *testing.T) {
	rec := &waitRecord{}
	opts := exchangeOptions("260630012")
	opts.Wait = rec.fn()
	outcome, _, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000, 950}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
		opts, fixedClock{at: time.Date(2026, 9, 22, 4, 16, 0, 0, time.UTC)})
	if err != nil {
		t.Fatalf("走通的一趟不该报错：%v", err)
	}
	if len(rec.calls) != 0 {
		t.Errorf("skipWait=true 不该等，实得 %v", rec.calls)
	}
	if outcome.Status != ExchangeStatusOrdered || outcome.WaitedUntil != nil {
		t.Errorf("不等也要照常成交，实得 %v / %v", outcome.Status, outcome.WaitedUntil)
	}
}

// TestExchangeWaitFailureSendsNothing：等待本身失败（被取消）⇒ 一枪不发。
func TestExchangeWaitFailureSendsNothing(t *testing.T) {
	rec := &waitRecord{err: core.NewTimeout("等整点开抢时被取消")}
	opts := exchangeOptions("260630012")
	opts.SkipWait = false
	opts.Wait = rec.fn()
	outcome, script, err := exchangeChain(t,
		exchangeRoutes(exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50)),
			[]int64{1000}, []string{slideJSON("a")}, []string{exchangeOKJSON}),
		opts, fixedClock{at: time.Date(2026, 9, 22, 4, 16, 0, 0, time.UTC)})
	if err != nil {
		t.Fatalf("等待失败折成结论：%v", err)
	}
	if outcome.Status != ExchangeStatusSkipped || outcome.Error == "" {
		t.Fatalf("应落 Skipped 并留原因，实得 %v / %q", outcome.Status, outcome.Error)
	}
	if got := script.count(exchangeV3Endpoint); got != 0 {
		t.Errorf("没等到点就不该发单，实得 %d 次", got)
	}
}

// TestDefaultExchangeWaitIsCancellableAndRechecksClock：那份默认等待的三条行为。
//
// ⚠ 它一次都不发请求，所以单独立一条：分片睡的意义就在于「取消要在一两秒内生效」
// 与「还剩多久」每次现算（编排层的 ctx 就是这么设计的）。
func TestDefaultExchangeWaitIsCancellableAndRechecksClock(t *testing.T) {
	t.Run("目标已过就直接回", func(t *testing.T) {
		clock := fixedClock{at: time.Date(2026, 9, 22, 5, 0, 0, 0, time.UTC)}
		if err := DefaultExchangeWait(clock)(
			context.Background(), clock.Now().Add(-time.Minute)); err != nil {
			t.Fatalf("目标在过去 ⇒ 应立刻返回，实得 %v", err)
		}
	})
	t.Run("ctx 已取消 ⇒ 立刻失败", func(t *testing.T) {
		clock := fixedClock{at: time.Date(2026, 9, 22, 4, 0, 0, 0, time.UTC)}
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		start := time.Now()
		err := DefaultExchangeWait(clock)(ctx, clock.Now().Add(time.Hour))
		if err == nil {
			t.Fatal("取消后应回错误，而不是一直睡")
		}
		if !strings.Contains(err.Error(), "取消") {
			t.Errorf("错误串要说清是被取消，实得 %q", err.Error())
		}
		if time.Since(start) > 3*time.Second {
			t.Errorf("取消应在一片（2 秒）之内生效，实得 %v", time.Since(start))
		}
	})
	t.Run("时钟跨过目标即返回", func(t *testing.T) {
		base := time.Date(2026, 9, 22, 4, 0, 0, 0, time.UTC)
		clock := &advancingClock{at: base, step: time.Hour}
		if err := DefaultExchangeWait(clock)(context.Background(), base.Add(time.Millisecond)); err != nil {
			t.Fatalf("应在第二次读时钟时返回，实得 %v", err)
		}
		if clock.read < 2 {
			t.Errorf("每一片都要重新读一次时钟，实得读了 %d 次", clock.read)
		}
	})
}

// advancingClock 是「每读一次就前进一段」的时钟：只用来把分片循环推到下一格。
type advancingClock struct {
	at   time.Time
	step time.Duration
	read int
}

func (c *advancingClock) Now() time.Time {
	c.read++
	if c.read > 1 {
		return c.at.Add(c.step)
	}
	return c.at
}

// ─────────────────────────────────────────────────────────────────────────────
// 入参守卫与 Run 那一段
// ─────────────────────────────────────────────────────────────────────────────

func TestExchangeRejectsBadInputsBeforeAnyRequest(t *testing.T) {
	for _, tc := range []struct {
		name  string
		src   *SignInSessionSource
		clock core.Clock
		opts  ExchangeOptions
	}{
		{"没有会话源", nil, testClock(), exchangeOptions("260630012")},
		{"没有时钟", &SignInSessionSource{}, nil, exchangeOptions("260630012")},
		{"没给奖品", &SignInSessionSource{}, testClock(), exchangeOptions("   ")},
	} {
		t.Run(tc.name, func(t *testing.T) {
			outcome, err := Exchange(context.Background(), tc.src, tc.clock, tc.opts)
			if err == nil {
				t.Fatal("入参不成立应回错误")
			}
			if outcome.Status != ExchangeStatusNotAttempted {
				t.Errorf("零值档位应是「没判成」，实得 %v", outcome.Status)
			}
		})
	}
}

// TestExchangeStatusZeroValueIsNotSuccess：⭐ 本链最要紧的一条守约。
//
// 结果结构体被漏填时，界面上**不许**读成「已兑到」。零值那一格是
// [ExchangeStatusNotAttempted]，而它映射成失败 —— 有人把 `Ordered` 挪到 `iota` 第一位，这条就红。
func TestExchangeStatusZeroValueIsNotSuccess(t *testing.T) {
	var status ExchangeStatus
	if status != ExchangeStatusNotAttempted {
		t.Fatalf("零值应是 NotAttempted，实得 %v", status)
	}
	res := ExchangeResult{Account: core.NewAccountID(liveTestAccountID)}
	if report := res.RunReport(); report.Outcome != core.OutcomeFailed {
		t.Errorf("零值结论应落失败，实得 %v", report.Outcome)
	}
	if strings.Contains(res.SummaryLine(), "兑到") {
		t.Errorf("零值小结里不许出现「兑到」，实得 %q", res.SummaryLine())
	}
}

// TestExchangeRunUsesWorldAndProgress：跑一个账号那一段（走世界，不注入注入点）。
//
// 这一条只验两件事：判据没过时**不会**去碰设备指纹（那份假库的 `GetSmidState` 故意回错误，
// 碰了就炸），以及结论会**流过 Progress**（界面上过程可见）。
func TestExchangeRunUsesWorldAndProgress(t *testing.T) {
	var lines []string
	script := liveTaskScriptWith(append(sessionRoutes(),
		liveTaskRoute{exchangeListEndpoint, []string{
			exchangeListJSON(exchangePrizeJSON(260630012, "甲", 50))}},
		liveTaskRoute{exchangeStatusEndpoint, []string{exchangeStatusJSON(1000)}},
	)...)
	world := signInWorldFor(&poolSignInStore{*liveTaskStore()}, script)
	world.Progress = func(level core.LogLevel, message string) { lines = append(lines, message) }

	result, perr := ExchangeRun{SignInWorld: world, Prize: "查无此件"}.
		RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("判据没过不该交回错误：%v", perr)
	}
	if result.Status != ExchangeStatusSkipped || result.Plan != ExchangePlanNotFound {
		t.Fatalf("应落 Skipped/NotFound，实得 %v / %v", result.Status, result.Plan)
	}
	if got := script.count(exchangeV3Endpoint); got != 0 {
		t.Errorf("一枪都不该发，实得 %d 次", got)
	}
	if strings.Contains(result.SummaryLine(), "兑换失败") {
		t.Errorf("没发单不是「兑换失败」，实得 %q", result.SummaryLine())
	}
	if len(lines) != 1 || !strings.HasPrefix(lines[0], "没兑换：") {
		t.Errorf("过程话该有一句且以「没兑换：」开头，实得 %v", lines)
	}
}

// TestExchangeRunReportsErrorsAsNotAttempted：取票那一步失败 ⇒ 错误与结论同时给。
//
// 走错误那条路时编排层仍会取 `SummaryLine`，留零值会渲染成一句没人看得懂的话。
func TestExchangeRunReportsErrorsAsNotAttempted(t *testing.T) {
	script := liveTaskScriptWith(liveTaskRoute{"/user/querySpecTokenV2",
		[]string{`{"success":false,"code":"500","message":"SSO 挂了"}`}})
	result, perr := ExchangeRun{
		SignInWorld: signInWorldFor(&poolSignInStore{*liveTaskStore()}, script),
		Prize:       "260630012",
	}.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("取票失败应交回错误")
	}
	if result.Status != ExchangeStatusNotAttempted {
		t.Errorf("档位要同时摆成没判成，实得 %v", result.Status)
	}
	if result.SummaryLine() == "" {
		t.Error("小结不能是空句")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小结文案
// ─────────────────────────────────────────────────────────────────────────────

func TestExchangeSummaryLinesCarryTheirEvidence(t *testing.T) {
	after := int64(950)
	notAfter := int64(1000)
	for _, tc := range []struct {
		name string
		res  ExchangeResult
		want []string
	}{
		{"成交且核到差值", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusOrdered, Prize: prizeNamed("甲商品", 50),
			Balance: 1000, AfterBalance: &after,
		}}, []string{"兑到「甲商品」", "50 豆", "1000 → 950"}},
		{"重复兑奖", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusAlreadyExchanged, Prize: prizeNamed("甲商品", 50),
		}}, []string{"此前已兑过", "没有重复发奖"}},
		{"余额差对不上", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusUnknown, Prize: prizeNamed("甲商品", 50),
			Balance: 1000, AfterBalance: &notAfter,
		}}, []string{"兑换结果判不了", "余额差 0", "去 App 核对"}},
		{"滑块算不出（一枪没发）", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusSliderUnresolved, Error: "两个口径分歧 7px",
		}}, []string{"没兑换：滑块解算没过", "一枪都没发"}},
		{"非目标用户", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusNotTargetUser, Message: "非移动用户不可领奖",
		}}, []string{"非移动用户不可领奖", "账号级"}},
		{"上游回失败码", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusFailed, Code: 415, Message: "奖品单日已耗尽",
		}}, []string{"兑换失败", "415", "奖品单日已耗尽"}},
		// ⚠ 空 `msg` 不能渲染成一对空引号 —— 那会被读成「上游说了句空话」。
		{"上游没给文案", ExchangeResult{ExchangeOutcome: ExchangeOutcome{
			Status: ExchangeStatusFailed, Code: 610,
		}}, []string{"兑换失败", "610", "没给原因"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			line := tc.res.SummaryLine()
			for _, want := range tc.want {
				if !strings.Contains(line, want) {
					t.Errorf("小结里应有 %q，实得 %q", want, line)
				}
			}
		})
	}
}

// TestExchangeSummaryLineHasNoCredentialShapes：小结出网（进 `runs.message` 与推送），
// 所以里面不许出现任何凭据形态的东西。
//
// 这一条不是形式检查：链上那几个结构体带着 `jwttoken` 与 `deviceId`（[SignInSession.Describe]
// 只报长度就是为了这个）。拼接句子里塞进原值就是把票写进历史表。
func TestExchangeSummaryLineHasNoCredentialShapes(t *testing.T) {
	res := ExchangeResult{ExchangeOutcome: ExchangeOutcome{
		Status: ExchangeStatusOrdered, Prize: prizeNamed("甲商品", 50),
		Balance: 1000, AfterBalance: func() *int64 { v := int64(950); return &v }(),
		DeviceIDFailed: "DEV-89", Offset: func() *int64 { v := int64(320); return &v }(),
	}}
	line := res.SummaryLine()
	for _, forbidden := range []string{"DEV-89", "jwttoken", "Bearer", "authorization"} {
		if strings.Contains(line, forbidden) {
			t.Errorf("小结里不许出现 %q，实得 %q", forbidden, line)
		}
	}
}
