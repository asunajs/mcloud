package app

// `ListExchangePrizes` 与 `PlanExchangeWithoutBalance` / `ExchangePlan.Slug` 的用例。
//
// # 这一族要钉的是什么
//
// 界面上的选品器是**第一次**把 `exchangeList` 暴露成"点一下就会发出去"的东西。
// 它只读，所以风险面不在"扣了豆"，而在两格别的地方：
//
//  1. **它不许顺手多打账号级的枪**（`infoV3` / `getSlide` / `exchangeV3`）——
//     多打一枪的代价是风控噪声，而这一条在响应里**看不见**，只能数请求；
//  2. **判定口径与下单那条链同源** —— 界面上说"可兑"、链上说"不发"，
//     两边各写一份判据时迟早漂移。所以这里断言的是同一个 `ExchangePlan` 档位，
//     而"不含余额"那一格是**刻意的宽**（写死成一条用例，改窄会红）。
//
// 90001 那一档也在这里验：走的是 [do] 那份"刷一次票、重放一次"，
// server 层不重复实现（`internal/app/exchange_list.go` 文件头那条理由）。

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// exchangeListHarness 取一份「按端点分派」的脚本 + 那个账号的会话来源。
//
// 与 `exchangeChain` 同一套夹具（`sessionRoutes()` 那三枪 + 传进来的业务端点），
// 差别只是这里不跑整条下单链，只跑那一枪只读。
func exchangeListHarness(
	t *testing.T, routes ...liveTaskRoute,
) (*liveTaskScript, *SignInSessionSource) {
	t.Helper()
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	src, perr := signInWorldFor(&poolSignInStore{*liveTaskStore()}, script).
		Source(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("取票失败：%v", perr)
	}
	return script, src
}

// TestListExchangePrizesReadsOnlyTheList：一枪清单，账号级那三枪一次都不打。
func TestListExchangePrizesReadsOnlyTheList(t *testing.T) {
	list := exchangeListJSON(
		exchangePrizeJSON(260630012, "甲", 50),
		exchangePrizeJSON(260630013, "乙", 80, "limit", "2"),
	)
	script, src := exchangeListHarness(t, liveTaskRoute{exchangeListEndpoint, []string{list}})

	got, perr := ListExchangePrizes(context.Background(), src)
	if perr != nil {
		t.Fatalf("只读那一枪应成功：%s", perr.Message())
	}
	items := got.All()
	if len(items) != 2 {
		t.Fatalf("应解析出两件，实得 %d 件（分组 %+v）", len(items), got.Groups)
	}
	if items[0].PrizeID != 260630012 || items[1].PrizeID != 260630013 {
		t.Errorf("两件应为 260630012 / 260630013，实得 %d / %d", items[0].PrizeID, items[1].PrizeID)
	}
	if script.count(exchangeListEndpoint) != 1 {
		t.Errorf("清单应只读一次，实得 %d 次", script.count(exchangeListEndpoint))
	}
	// ⭐ 余额 / 滑块 / 下单：这一条接口一次都不该出现。
	for _, endpoint := range []string{exchangeStatusEndpoint, getSlideEndpoint, exchangeV3Endpoint} {
		if n := script.count(endpoint); n != 0 {
			t.Errorf("只读选品不该打 %s，实得 %d 次", endpoint, n)
		}
	}
}

// TestListExchangePrizesReplaysOnceOn90001：票被判死 ⇒ 刷一次、重放一次（与下单链同一份 [do]）。
func TestListExchangePrizesReplaysOnceOn90001(t *testing.T) {
	script, src := exchangeListHarness(t, liveTaskRoute{
		exchangeListEndpoint, []string{notLoggedIn, exchangeListJSON(exchangePrizeJSON(1, "甲", 50))},
	})
	got, perr := ListExchangePrizes(context.Background(), src)
	if perr != nil {
		t.Fatalf("刷完票就该成功：%s", perr.Message())
	}
	if len(got.All()) != 1 {
		t.Errorf("重放之后应拿到那一件，实得 %d 件", len(got.All()))
	}
	if n := script.count(exchangeListEndpoint); n != 2 {
		t.Errorf("同一枪应重放一次（共 2 次），实得 %d 次", n)
	}
	// 只刷这一页：建会话从 1 趟变 2 趟，不多刷。
	if n := script.count("/user/querySpecTokenV2"); n != 2 {
		t.Errorf("应重建一次会话（共 2 趟），实得 %d 趟", n)
	}
}

// TestListExchangePrizesFailsLoudlyOnBadCode：非 90001 的失败**如实回错**，不重放、不降级。
//
// "降级成空清单"是这一条最坏的失效：界面上会念成"今天没有可兑的奖品"，
// 而真相是上游回了一个码。所以这里判两头：错误要非空，且那一枪**只打一次**。
func TestListExchangePrizesFailsLoudlyOnBadCode(t *testing.T) {
	script, src := exchangeListHarness(t, liveTaskRoute{
		exchangeListEndpoint, []string{`{"code":610,"msg":"合成失败"}`},
	})
	got, perr := ListExchangePrizes(context.Background(), src)
	if perr == nil {
		t.Fatal("上游回了非 0 码，这里必须报错而不是交回一份清单")
	}
	if len(got.All()) != 0 {
		t.Errorf("失败时不该交回任何一件，实得 %d 件", len(got.All()))
	}
	if n := script.count(exchangeListEndpoint); n != 1 {
		t.Errorf("非会话类失败不该重放，实得 %d 次", n)
	}
}

// TestPlanExchangeWithoutBalanceSkipsOnlyTheCloudCheck：少读数只少那一档。
//
// ⭐ 这一条钉的是"宽"这件事本身：价格 5000 而**没有**余额读数时仍然放行。
// 哪天有人"顺手"把余额也读进来（多打一枪），改窄的表现就是这条变红 ——
// 那一枪该不该多打是**接口形状**的决定，不该由一次顺手改动定。
// 另一半同样重要：`limit=2` 那一档**照旧挡**，所以"少一个读数"不是"少判一切"。
func TestPlanExchangeWithoutBalanceSkipsOnlyTheCloudCheck(t *testing.T) {
	expensive := upstream.ExchangePrize{
		PrizeID: 260630099,
		Name:    ptr("贵得很"),
		Price:   ptr(int64(5000)),
		Limit:   ptr(int64(0)),
		Count:   ptr(int64(10)), TotalCount: ptr(int64(10)),
		DailyRemainderCount: ptr(int64(5)), MinRemainderCount: ptr(int64(5)),
		OnLine: ptr(int64(1)),
	}
	if plan := PlanExchangeWithoutBalance(expensive, time.Unix(0, 0)); plan != ExchangePlanGo {
		t.Errorf("没有余额读数时不该判余额那一档（那是刻意的宽），实得 %v", plan)
	}
	// 同一个奖品，给足余额仍然放行、给不够就挡 —— 两条路共用同一个内核。
	if plan := PlanExchange(expensive, 10, time.Unix(0, 0)); plan != ExchangePlanInsufficientCloud {
		t.Errorf("余额 10 对价格 5000 应判不够，实得 %v", plan)
	}
	if plan := PlanExchange(expensive, 5000, time.Unix(0, 0)); plan != ExchangePlanGo {
		t.Errorf("余额正好够时应放行，实得 %v", plan)
	}
	blocked := expensive
	blocked.Limit = ptr(int64(2))
	if plan := PlanExchangeWithoutBalance(blocked, time.Unix(0, 0)); plan != ExchangePlanExchangedThisPrize {
		t.Errorf("limit=2 那一档不该因为\"没读余额\"就漏判，实得 %v", plan)
	}
}

// TestEveryExchangePlanSlugIsDistinctAndDeclared：十七档各有一个**唯一**的 slug。
//
// 为什么钉"唯一"而不是逐条写死十六个串：这一族将来会加档（`limit` 的第三档就是
// 两处文案冲突那种"等着上游订正"的格子），而加档时**最容易忘**的就是 `Slug()` 那一支
// —— 忘了就落 `default` 的 "unknown"，界面上那一件会显示成"不认识的状态"，
// 而它是**静默**的（HTTP 200、清单照出）。逐条写死反而会在加档时红不起来
// （新档位不在这份清单里），所以这里遍历档位本身。
func TestEveryExchangePlanSlugIsDistinctAndDeclared(t *testing.T) {
	plans := []ExchangePlan{
		ExchangePlanGo, ExchangePlanNotFound, ExchangePlanAmbiguous, ExchangePlanOffShelf,
		ExchangePlanLimitMissing, ExchangePlanLimitTomorrow, ExchangePlanExchangedThisPrize,
		ExchangePlanLimitThree, ExchangePlanExchangedThisGroup, ExchangePlanLimitUnknown,
		ExchangePlanMinRemainderZero, ExchangePlanSoldOut, ExchangePlanDailyStockEmpty,
		ExchangePlanPriceMissing, ExchangePlanInsufficientCloud, ExchangePlanNotStarted,
		ExchangePlanEnded,
	}
	if len(plans) != 17 {
		t.Fatalf("这份清单应当穷尽十七档，实得 %d 条（档位改了要同步这里）", len(plans))
	}
	seen := make(map[string]ExchangePlan, len(plans))
	for _, plan := range plans {
		slug := plan.Slug()
		if slug == "" || slug == "unknown" {
			t.Errorf("档位 %d 没有登记 slug（落到了兜底那一支）", int(plan))
			continue
		}
		if strings.ToLower(slug) != slug || strings.ContainsAny(slug, " -") {
			t.Errorf("slug %q 不是小写 snake_case（响应体里它是前端那张颜色表的键）", slug)
		}
		if previous, dup := seen[slug]; dup {
			t.Errorf("档位 %d 与 %d 共用 slug %q ⇒ 前端那张颜色表会错配",
				int(previous), int(plan), slug)
		}
		seen[slug] = plan
		if plan.Reason() == "" {
			t.Errorf("档位 %d 的 reason 是空串（界面上会显示成一句空白）", int(plan))
		}
	}
	if ExchangePlan(99).Slug() != "unknown" {
		t.Error("越界取值必须落 unknown，不能落回零值那一档（\"可以下单\"）")
	}
}
