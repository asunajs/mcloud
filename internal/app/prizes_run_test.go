package app

// [PrizesRun] 的用例：列待领取奖品（**只读**）。
//
// 这一条只有两处容易悄悄坏掉：
//
//  1. 分页参数**真的传下去了** —— `PrizeListPageSize` 是个常量，写在这里不等于生效；
//     而"只取一页"是**已知会静默截断**的行为（照搬旧项目，见 [PendingPrizes]），
//     所以要能在请求上看清它取的是第几页、每页几条。
//  2. 空列表是**成功** —— "来看一眼，没有要过期的奖品"正是这一趟想要的答复。
//     把空当成"什么都没干成"的话，挂上定时任务后每天都会报一条失败。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// upstreamPrize 造一条奖品记录，只填本用例关心的那个字段。
func upstreamPrize(name string) upstream.PrizeRecordItem {
	value := name
	return upstream.PrizeRecordItem{PrizeName: &value}
}

const prizeRecordEndpoint = "/ycloud/signin/public/record"

func prizeBody(names ...string) string {
	items := make([]string, 0, len(names))
	for _, name := range names {
		items = append(items, `{"prizeName":"`+name+`","flag":1,"insertTime":"2026-09-20 10:00:00",`+
			`"expireTime":"2026-10-04T15:59:59.000+00:00","oid":1270932714,"type":1057}`)
	}
	return `{"code":0,"msg":"success","result":{"records":[` + strings.Join(items, ",") + `]}}`
}

func prizesRun(store core.StateStore, script *liveTaskScript) PrizesRun {
	return PrizesRun{SignInWorld: signInWorldFor(store, script)}
}

// TestPrizesRunAsksForOnePageOfTen：分页参数出在**请求上**。
//
// ⭐ 断言看 URL 不看结果：把 `PrizeListPageSize` 换成 100 也照样能解析，
// 只有请求会说出我们取了多少条 —— 而"取多了"在这个接口上是无害的，
// 真正的风险反过来（哪天有人把它改成 1，界面就只显示一条却说什么都是"全的"）。
func TestPrizesRunAsksForOnePageOfTen(t *testing.T) {
	script := liveTaskScriptWith(append(sessionRoutes(),
		liveTaskRoute{prizeRecordEndpoint, []string{prizeBody("一个月会员")}})...)
	if _, perr := prizesRun(&poolSignInStore{*liveTaskStore()}, script).
		RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("查询失败：%v", perr)
	}
	if got := script.count(prizeRecordEndpoint); got != 1 {
		t.Fatalf("只读查询就该打一次，实得 %d 次", got)
	}
	url := script.snapshot()[len(script.snapshot())-1].URL
	for _, want := range []string{"pageNumber=1", "pageSize=10"} {
		if !strings.Contains(url, want) {
			t.Errorf("请求里应有 %s，实得 %q", want, url)
		}
	}
	// `type` = 待领取那一档。它决定"列出来的是待领还是已过期"，
	// 而三种页签的响应形状完全一样 —— 传错了看不出来，只会念错数。
	if !strings.Contains(url, "type=1") {
		t.Errorf("应查「待领取」页签（type=1），实得 %q", url)
	}
}

// TestPrizesResultSummary：小结列**至多三个**奖品名。
//
// 这一句会进 `runs.message` 与推送标题，而"待领取"可以有十几条 ——
// 全列等于把一条通知变成一段清单。
func TestPrizesResultSummary(t *testing.T) {
	three := []string{"甲", "乙", "丙"}
	cases := []struct {
		name    string
		names   []string
		summary string
	}{
		{name: "空", summary: "没有待领取的奖品"},
		{name: "一条", names: []string{"一个月会员"}, summary: "待领取奖品 1 条：一个月会员"},
		{name: "三条以内列全", names: three, summary: "待领取奖品 3 条：甲、乙、丙"},
		{name: "四条起截断", names: append(three, "丁"), summary: "待领取奖品 4 条：甲、乙、丙…"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			res := PrizesResult{Account: core.NewAccountID(liveTestAccountID)}
			for _, name := range c.names {
				res.Records = append(res.Records, upstreamPrize(name))
			}
			if got := res.SummaryLine(); got != c.summary {
				t.Errorf("应逐字为 %q，实得 %q", c.summary, got)
			}
			// 只读链**恒成功**：空列表也是答复而不是失败。
			if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
				t.Errorf("实得 %v", report.Outcome)
			}
		})
	}
}

// TestPrizesRunReturnsErrorWhenTheQueryFails：查询失败交回错误，不折成"没有奖品"。
//
// 反过来的表现是"界面上写着没有待领奖品，其实是那次查询挂了" —— 而奖品会过期。
func TestPrizesRunReturnsErrorWhenTheQueryFails(t *testing.T) {
	script := liveTaskScriptWith(append(sessionRoutes(), liveTaskRoute{
		prizeRecordEndpoint, []string{`{"code":1,"msg":"系统繁忙"}`}})...)
	res, perr := prizesRun(&poolSignInStore{*liveTaskStore()}, script).
		RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatalf("查询失败必须交回错误，实得 %+v", res)
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("要带上上游那句话，实得 %q", perr.Error())
	}
}
