package app

// [PlanExchange] / [LocateExchangePrize] / [PlanExchangeWait] 的用例：逐档判定。
//
// 这一层的价值全在"每一档都红得出来"：下单是**真扣豆且不可逆**的，
// 所以每一条放行都必须有一条反向用例盯着（把那一档改坏 ⇒ 本文件的某一条红）。
// 反过来，"挡住"的每一档也要钉住它**说的是哪一句** —— 界面只念 `Reason()`，
// 档位在文案里被混成一句"不能兑"就等于没判。
//
// 链（发了几枪、什么顺序）在 `exchange_run_test.go`；这里一个请求都不发。

import (
	"strconv"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/upstream"
)

// i64 是指针夹具：本层的判据字段全是指针，"没给"与"给了 0"是两种答案。
func i64(value int64) *int64 { return &value }

func text(value string) *string { return &value }

// exchangePrizeFixture 造一件"全部判据都放行"的商品，测试各用例只改自己那一格。
//
// 数值取自抓包 §5.1 那一件（`260630012` / 50 豆 / 每日 5000 余 4998）——
// 用实拍数而不是 1/2/3 是为了让"判据长什么样"在测试里也可查。
func exchangePrizeFixture() upstream.ExchangePrize {
	return upstream.ExchangePrize{
		PrizeID:             260630012,
		Name:                text("移动云盘100万tokens叠加包"),
		Price:               i64(50),
		Limit:               i64(upstream.ExchangeLimitAvailable),
		Count:               i64(399291),
		TotalCount:          i64(400000),
		DailyCount:          i64(5000),
		DailyRemainderCount: i64(4998),
		MinRemainderCount:   i64(1),
		OnLine:              i64(1),
	}
}

// exchangeNoon 是"没有窗口字段时"用的时刻（窗口两串缺席 ⇒ 时间档不挡）。
var exchangeNoon = time.Date(2026, 9, 22, 4, 16, 0, 0, time.UTC) // CST 12:16，实拍那一笔的时刻

func TestPlanExchangeAllowsFullyQualifiedPrize(t *testing.T) {
	if got := PlanExchange(exchangePrizeFixture(), 50, exchangeNoon); got != ExchangePlanGo {
		t.Fatalf("全部判据放行时应可下单，实得 %v（%s）", got, got.Reason())
	}
	// 余额**等于**价格是"够"：H5 那句判据是 `x < pOrder ⇒ 不够`，等于时不拦。
	// 反向：把 `>` 改成 `>=` 这一条就红。
	if got := PlanExchange(exchangePrizeFixture(), 49, exchangeNoon); got != ExchangePlanInsufficientCloud {
		t.Fatalf("余额 49 对价格 50 应判不够，实得 %v", got)
	}
}

func TestPlanExchangeEachLimitTier(t *testing.T) {
	cases := []struct {
		limit *int64
		want  ExchangePlan
	}{
		{nil, ExchangePlanLimitMissing},
		{i64(0), ExchangePlanGo},
		{i64(1), ExchangePlanLimitTomorrow},
		{i64(2), ExchangePlanExchangedThisPrize},
		{i64(3), ExchangePlanLimitThree},
		{i64(4), ExchangePlanExchangedThisGroup},
		{i64(5), ExchangePlanLimitUnknown},
		{i64(-1), ExchangePlanLimitUnknown},
	}
	for _, tc := range cases {
		prize := exchangePrizeFixture()
		prize.Limit = tc.limit
		prizeID := "nil"
		if tc.limit != nil {
			prizeID = strconv.FormatInt(*tc.limit, 10)
		}
		if got := PlanExchange(prize, 5000, exchangeNoon); got != tc.want {
			t.Errorf("limit=%s 应落 %v，实得 %v（%s）", prizeID, tc.want, got, got.Reason())
		}
	}
}

func TestPlanExchangeStockTiers(t *testing.T) {
	cases := []struct {
		name  string
		patch func(*upstream.ExchangePrize)
		want  ExchangePlan
	}{
		{"余量阈值为 0（H5「奖品被抢光啦」）",
			func(p *upstream.ExchangePrize) { p.MinRemainderCount = i64(0) }, ExchangePlanMinRemainderZero},
		{"总投放为 0",
			func(p *upstream.ExchangePrize) { p.Count = i64(0) }, ExchangePlanSoldOut},
		{"总投放为负（上游哪天给个脏数）",
			func(p *upstream.ExchangePrize) { p.Count = i64(-3) }, ExchangePlanSoldOut},
		{"今日余量为 0",
			func(p *upstream.ExchangePrize) { p.DailyRemainderCount = i64(0) }, ExchangePlanDailyStockEmpty},
		// ⚠ 三个余量**缺字段不挡**：它们是时刻量（会因别的用户在同一分钟里兑掉而变化），
		// 而"上游没给"不等于"是 0"。
		{"总投放没给 ⇒ 不挡",
			func(p *upstream.ExchangePrize) { p.Count = nil }, ExchangePlanGo},
		{"今日余量没给 ⇒ 不挡",
			func(p *upstream.ExchangePrize) { p.DailyRemainderCount = nil }, ExchangePlanGo},
		// 但 `minRemainderCount` 是 H5 按钮判据的另一半，缺它同样**不挡**：
		// 本层的硬闸是 `limit` 与余额，其余是"有值才判"。
		{"余量阈值没给 ⇒ 不挡",
			func(p *upstream.ExchangePrize) { p.MinRemainderCount = nil }, ExchangePlanGo},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			prize := exchangePrizeFixture()
			tc.patch(&prize)
			if got := PlanExchange(prize, 5000, exchangeNoon); got != tc.want {
				t.Fatalf("应落 %v，实得 %v（%s）", tc.want, got, got.Reason())
			}
		})
	}
}

// TestPlanExchangePriceMissingAndFree 判的是"价格这一栏**缺席**"与"价格为零"。
//
// ⚠ 名字里没有"余额不够"是有意的：那一档钉在
// [TestPlanExchangeAllowsFullyQualifiedPrize]（49 对 50 ⇒ `InsufficientCloud`，
// 且 50 对 50 放行 —— 边界只在那里判一次，两处各判会让改口径时只改到一处）。
func TestPlanExchangePriceMissingAndFree(t *testing.T) {
	noPrice := exchangePrizeFixture()
	noPrice.Price = nil
	if got := PlanExchange(noPrice, 999999, exchangeNoon); got != ExchangePlanPriceMissing {
		t.Fatalf("没给价格时余额再高也不能下单，实得 %v", got)
	}
	// 价格 0（上游哪天给个免费件）：余额 0 也放行 —— 判据是"不够"而不是"为零"。
	free := exchangePrizeFixture()
	free.Price = i64(0)
	if got := PlanExchange(free, 0, exchangeNoon); got != ExchangePlanGo {
		t.Fatalf("价格 0 且余额 0 应放行，实得 %v（%s）", got, got.Reason())
	}
}

func TestPlanExchangeOnLine(t *testing.T) {
	off := exchangePrizeFixture()
	off.OnLine = i64(0)
	if got := PlanExchange(off, 5000, exchangeNoon); got != ExchangePlanOffShelf {
		t.Fatalf("onLine=0 应判下架，实得 %v", got)
	}
	// 缺字段不挡（实拍只有一档 `1`，"没给"这一形态没有样本 ⇒ 少判一条，不多猜一档）。
	unknown := exchangePrizeFixture()
	unknown.OnLine = nil
	if got := PlanExchange(unknown, 5000, exchangeNoon); got != ExchangePlanGo {
		t.Fatalf("onLine 没给时不该挡，实得 %v", got)
	}
}

func TestPlanExchangePrizeWindow(t *testing.T) {
	// 窗口按东八区的裸串（`2026-07-01 09-30-00` 那一族形态）。
	started := exchangePrizeFixture()
	started.StartTime = text("2026-09-22 09-30-00") // CST，= 01:30 UTC
	started.EndTime = text("2026-09-22 23-59-59")
	if got := PlanExchange(started, 5000, exchangeNoon); got != ExchangePlanGo {
		t.Fatalf("12:16 在 09:30~23:59 之间应放行，实得 %v（%s）", got, got.Reason())
	}

	notYet := exchangePrizeFixture()
	notYet.StartTime = text("2026-09-22 20-00-00")
	if got := PlanExchange(notYet, 5000, exchangeNoon); got != ExchangePlanNotStarted {
		t.Fatalf("开售时间没到应落 NotStarted，实得 %v", got)
	}

	ended := exchangePrizeFixture()
	ended.EndTime = text("2026-09-21 00-00-00")
	if got := PlanExchange(ended, 5000, exchangeNoon); got != ExchangePlanEnded {
		t.Fatalf("兑换时间已过应落 Ended，实得 %v", got)
	}

	// ⚠ 形态不认识 ⇒ **放行**（与趣玩AI那一族刻意相反，理由见 [exchangeWindowUnknown]）：
	// 这一族的硬闸是 limit 与余额，拿一条没有样本的时间规则挡在所有商品前面，
	// 代价是"所有没给窗口的都兑不了"。
	weird := exchangePrizeFixture()
	weird.StartTime = text("2026/09/22 20:00")
	weird.EndTime = text("明天")
	if got := PlanExchange(weird, 5000, exchangeNoon); got != ExchangePlanGo {
		t.Fatalf("时间串形态不认识时应放行（判不了≠不在期），实得 %v（%s）", got, got.Reason())
	}
	// 一边解得出、一边解不出：解得出的那一边仍然判（越界就挡）。
	halfBroken := exchangePrizeFixture()
	halfBroken.StartTime = text("坏数据")
	halfBroken.EndTime = text("2026-09-21 00-00-00")
	if got := PlanExchange(halfBroken, 5000, exchangeNoon); got != ExchangePlanEnded {
		t.Fatalf("只给了一个能解的边界也要判，实得 %v", got)
	}
}

func TestPlanExchangeOrderIsMeaningful(t *testing.T) {
	// 多条同时不成立时，念**最可执行**的那句：`limit`（本账号状态）压过库存与余额。
	both := exchangePrizeFixture()
	both.Limit = i64(upstream.ExchangeLimitExchanged)
	both.Price = i64(999999)
	both.Count = i64(0)
	if got := PlanExchange(both, 10, exchangeNoon); got != ExchangePlanExchangedThisPrize {
		t.Fatalf("limit=2 应优先于库存与余额，实得 %v（%s）", got, got.Reason())
	}
	// 余额不够压过时间窗：时间那一档只有"明确越界"才判，而"我不够豆"是当下的事实。
	rich := exchangePrizeFixture()
	rich.Price = i64(999999)
	rich.EndTime = text("2026-09-21 00-00-00")
	if got := PlanExchange(rich, 10, exchangeNoon); got != ExchangePlanInsufficientCloud {
		t.Fatalf("余额不够应优先于时间已过，实得 %v", got)
	}
	// 余量阈值 0 与总投放 0 同时成立 ⇒ 念阈值那句（H5 在这一档给用户的就是"被抢光啦"）。
	empty := exchangePrizeFixture()
	empty.MinRemainderCount = i64(0)
	empty.Count = i64(0)
	if got := PlanExchange(empty, 5000, exchangeNoon); got != ExchangePlanMinRemainderZero {
		t.Fatalf("两句都成立时应念阈值那句，实得 %v", got)
	}
}

// TestExchangePlanReasonsAreAllDifferent：每一档说一句**不同**的话。
//
// 这条用例守的是复制粘贴：新加一档却忘了在 [ExchangePlan.Reason] 里写它那一句，
// 表现是两档共用一句"（默认）"话 —— 界面上看不出来，但用户按那句做的动作是错的。
func TestExchangePlanReasonsAreAllDifferent(t *testing.T) {
	all := []ExchangePlan{
		ExchangePlanGo, ExchangePlanNotFound, ExchangePlanAmbiguous, ExchangePlanOffShelf,
		ExchangePlanLimitMissing, ExchangePlanLimitTomorrow, ExchangePlanExchangedThisPrize,
		ExchangePlanLimitThree, ExchangePlanExchangedThisGroup, ExchangePlanLimitUnknown,
		ExchangePlanMinRemainderZero, ExchangePlanSoldOut, ExchangePlanDailyStockEmpty,
		ExchangePlanPriceMissing, ExchangePlanInsufficientCloud,
		ExchangePlanNotStarted, ExchangePlanEnded,
	}
	seen := map[string]ExchangePlan{}
	for _, plan := range all {
		reason := plan.Reason()
		if reason == "" {
			t.Errorf("%v 的 Reason 是空的", plan)
			continue
		}
		if previous, ok := seen[reason]; ok {
			t.Errorf("%v 与 %v 说了同一句话：%q", previous, plan, reason)
		}
		seen[reason] = plan
	}
	if got := ExchangePlan(999).Reason(); got != "可以下单" {
		t.Errorf("越界档位该退回那句中性话，实得 %q", got)
	}
}

func TestExchangePlanBlocks(t *testing.T) {
	if ExchangePlanGo.Blocks() {
		t.Error("Go 那一档不该算拦住")
	}
	for _, plan := range []ExchangePlan{
		ExchangePlanNotFound, ExchangePlanInsufficientCloud, ExchangePlanLimitThree,
	} {
		if !plan.Blocks() {
			t.Errorf("%v 应算拦住", plan)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 定位
// ─────────────────────────────────────────────────────────────────────────────

func exchangeListFixture(items ...upstream.ExchangePrize) upstream.ExchangeList {
	return upstream.ExchangeList{
		Groups: map[string][]upstream.ExchangePrize{"0": items},
		Order:  []string{"0"},
	}
}

func TestLocateExchangePrizeMatchesIDAndExactName(t *testing.T) {
	first := exchangePrizeFixture()
	second := exchangePrizeFixture()
	second.PrizeID = 251230028
	second.Name = text("移动云盘100G云空间月卡")
	list := exchangeListFixture(first, second)

	prize, plan := LocateExchangePrize(list, "251230028")
	if plan != ExchangePlanGo || prize.PrizeID != 251230028 {
		t.Fatalf("按 id 应定位到第二件，实得 %+v / %v", prize.PrizeID, plan)
	}
	// 前后空格是界面上常见的粘贴形态。
	prize, plan = LocateExchangePrize(list, "  移动云盘100G云空间月卡 ")
	if plan != ExchangePlanGo || prize.PrizeID != 251230028 {
		t.Fatalf("带空格的名字应照常命中，实得 %+v / %v", prize.PrizeID, plan)
	}
}

// TestLocateExchangePrizeDoesNotMatchByPrefix：⭐ 名称**精确相等**，不做包含匹配。
//
// 实拍清单里有一整排同前缀商品（"移动云盘100G云空间月卡 / 年卡"、"白银会员月卡 / 季卡 / 年卡"）。
// 改成包含匹配（`strings.Contains`）不报错、也不会让别的用例红，但它会把"我要月卡"
// 打到清单里**先出现**的那一件上 —— 而那一枪是真扣豆。
func TestLocateExchangePrizeDoesNotMatchByPrefix(t *testing.T) {
	list := exchangeListFixture(exchangePrizeFixture())
	if _, plan := LocateExchangePrize(list, "移动云盘"); plan != ExchangePlanNotFound {
		t.Errorf("只给前缀应判没找到（不做包含匹配），实得 %v（%s）", plan, plan.Reason())
	}
	if _, plan := LocateExchangePrize(list, "tokens叠加"); plan != ExchangePlanNotFound {
		t.Errorf("只给中段应判没找到，实得 %v", plan)
	}
}

func TestLocateExchangePrizeAmbiguousAndMissing(t *testing.T) {
	left := exchangePrizeFixture()
	right := exchangePrizeFixture()
	right.PrizeID = 999
	list := exchangeListFixture(left, right) // 两件同名
	if _, plan := LocateExchangePrize(list, *left.Name); plan != ExchangePlanAmbiguous {
		t.Fatalf("同名多件应判歧义，实得 %v", plan)
	}
	if _, plan := LocateExchangePrize(list, "260630013"); plan != ExchangePlanNotFound {
		t.Fatalf("清单里没有的 id 应判没找到，实得 %v", plan)
	}
	for _, query := range []string{"", "   "} {
		if _, plan := LocateExchangePrize(list, query); plan != ExchangePlanNotFound {
			t.Fatalf("空入参应判没找到（不是拿清单第一件顶上去），实得 %v", plan)
		}
	}
	// `prizeName` 缺字段的那件不参与名称匹配，但按 id 照样能找到。
	noName := exchangePrizeFixture()
	noName.Name = nil
	list = exchangeListFixture(noName)
	if _, plan := LocateExchangePrize(list, "移动云盘100万tokens叠加包"); plan != ExchangePlanNotFound {
		t.Fatalf("没有名字的件不该被名称匹配命中，实得 %v", plan)
	}
	if _, plan := LocateExchangePrize(list, "260630012"); plan != ExchangePlanGo {
		t.Fatalf("没有名字的件按 id 仍应找得到，实得 %v", plan)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 整点等待
// ─────────────────────────────────────────────────────────────────────────────

func TestPlanExchangeWait(t *testing.T) {
	cases := []struct {
		name       string
		now        string
		wantNeeded bool
		wantUntil  string
	}{
		{"整点后 5 秒：还在上一场尾巴里，不等", "13:00:05", false, ""},
		{"整点后 19.999 秒：不等（边界内）", "13:00:19", false, ""},
		{"整点后整 20 秒：等（`< 20s` 才不等）", "13:00:20", true, "14:00:00"},
		{"半点：等到下一个整点", "13:30:00", true, "14:00:00"},
		{"实拍那一笔的时刻（12:16 CST）：也要等一整场", "04:16:00", true, "05:00:00"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			utc, err := time.Parse("2006-01-02 15:04:05", "2026-09-22 "+tc.now)
			if err != nil {
				t.Fatal(err)
			}
			got := PlanExchangeWait(utc)
			if got.Needed != tc.wantNeeded {
				t.Fatalf("Needed 应为 %v，实得 %v", tc.wantNeeded, got.Needed)
			}
			if !tc.wantNeeded {
				return
			}
			want, err := time.Parse("2006-01-02 15:04:05", "2026-09-22 "+tc.wantUntil)
			if err != nil {
				t.Fatal(err)
			}
			if !got.Until.Equal(want) {
				t.Errorf("等待目标应是 %s，实得 %s", want, got.Until)
			}
		})
	}
}

// TestPlanExchangeWaitIsZoneAgnostic：换个时区表达同一时刻，目标还是同一个**时刻**。
//
// 守住文件里那句"整小时偏移的时区不会错开"：把 `Truncate` 换成"先转本地再取整点"
// 这种写法在 UTC+8 下仍对，但在实现漂移成按墙上时钟字符串截取时会红。
func TestPlanExchangeWaitIsZoneAgnostic(t *testing.T) {
	utc := time.Date(2026, 9, 22, 4, 30, 0, 0, time.UTC)
	east := utc.In(upstream.ExchangeTimeLocation()) // CST 12:30
	if !utc.Equal(east) {
		t.Fatal("夹具本身应表达同一时刻")
	}
	fromUTC, fromEast := PlanExchangeWait(utc), PlanExchangeWait(east)
	if !fromUTC.Until.Equal(fromEast.Until) {
		t.Errorf("两种表示应给出同一个目标时刻：%s vs %s", fromUTC.Until, fromEast.Until)
	}
	if !fromEast.Until.Equal(time.Date(2026, 9, 22, 5, 0, 0, 0, time.UTC)) {
		t.Errorf("应等到 05:00 UTC（CST 13:00），实得 %s", fromEast.Until)
	}
}
