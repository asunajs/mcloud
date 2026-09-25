package app

// 「全网趣玩AI赢好礼」抽奖的用例：门槛判定（纯函数逐档）+ 跑一个账号那一段。
//
// 这一条链值得钉的只有两件事，而且都是**改一行就静默错**的那类：
//
//  1. 该不该发那个**写请求**。抽奖每天只有一次额度，判错的两种代价都不对称 ——
//     该不发时发了会把这一次**用掉**（重跑回不来），该发时不发只是少一次。
//     所以判定逐档测（[PlanFunai]），并且"门槛不成立 ⇒ 一条写请求都不发"要在
//     **接好线的 RunAccount 上**再测一遍：中间隔着 [SignInWorld.Source]，
//     装配写错的表现正是"发了一条本不该发的请求"。
//  2. 发出去的那个 `moduleId` **来自上一枪的响应**。写死一份夹具就等于把"换期"
//     变成一次静默打空（两档 id 各测一次）。
//
// ⚠ 夹具纪律：本文件没有任何凭据形态的值（`jwttoken` 用 [jwtOK] 给的合成串 `JWT-1`），
// 账号只用脱敏形态 [liveTestAccountID]；抽奖响应那份实拍里有个 `phone` 字段，
// 这里**不抄进来**（连合成手机号都不需要它）。
//
// ⚠ 三处**没有样本**的东西（`isDraw` 一族非 null 形态、"无次数"那句失败回什么码、
// 服务端要不要 `deviceid` 头）在这里一律**不断言行为**，只钉"我们这一侧不发/不据此分支"。
// 各自怎么补样写在对应的实现注释里。

import (
	"context"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 两条请求各自的端点。断言两头都要用：一条数"该打的打了几次"，
// 另一条数"门槛不成立时**零次**"。
const (
	funaiModuleEndpoint  = "/ycloud/funai/api/page/getmodule"
	funaiLotteryEndpoint = "/ycloud/funai/api/page/lottery"
)

// funaiCapturedBody 是实拍（顺序 11）那一档 `page/lottery` 的成功响应，逐字段照抄，
// 只删掉两类东西：`prizeUrl`（图片素材地址，本链不读）与 `phone`（账号标识，不落夹具）。
const funaiCapturedBody = `{"code":0,"msg":"success","result":{` +
	`"id":3228298,"isWin":1,"lotteryDate":"20260629",` +
	`"lotteryTime":"2026-06-29T20:13:47.890625379",` +
	`"marketName":"National_playAI","moduleId":"1-250730",` +
	`"prizeId":260107009,"prizeName":"30云朵","prizeType":2,"type":"1"}}`

// funaiNullCountsBody 是实拍的**另一档**：同一批抓包的 `timed/getmodule` 里
// 三个计数**全是 null** —— "上游没给判据字段"不是假想分支，所以它必须有一档夹具。
const funaiNullCountsBody = `{"code":0,"msg":"success","result":{` +
	`"moduleId":"1-250730","drawTimes":null,"drawsUsed":null,"todayGetLotteryCount":null,` +
	`"startTime":"2025-05-28T00:00:00","endTime":"2026-12-31T23:59:59"}}`

// funaiModuleBody 拼一份 `page/getmodule` 的响应。
//
// `startTime` / `endTime` 传空串 = **不发这个键**（用来测"活动期判不了"那一档）。
// 那三个不读的字段（`isDraw` / `drawTime` / `drawResult`）按实拍保持 `null`：
// 夹具里有它们，才有人不会误以为"解析器漏了三个字段"。
func funaiModuleBody(moduleID string, drawTimes, drawsUsed int64, startTime, endTime string) string {
	parts := []string{
		`"moduleId":"` + moduleID + `"`,
		`"drawTimes":` + strconv.FormatInt(drawTimes, 10),
		`"drawsUsed":` + strconv.FormatInt(drawsUsed, 10),
		`"todayGetLotteryCount":1`,
	}
	if startTime != "" {
		parts = append(parts, `"startTime":"`+startTime+`"`)
	}
	if endTime != "" {
		parts = append(parts, `"endTime":"`+endTime+`"`)
	}
	parts = append(parts, `"isDraw":null,"drawTime":null,"drawResult":null,"flag":1`)
	return `{"code":0,"msg":"success","result":{` + strings.Join(parts, ",") + `}}`
}

// funaiRuns 造一份"只到能发请求"的世界（不接票池：CLI 单次调用就是这个形状）。
func funaiRuns(store core.StateStore, routes ...liveTaskRoute) (FunaiRun, *liveTaskScript) {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	return FunaiRun{SignInWorld: signInWorldFor(store, script)}, script
}

// funaiRequests 取打到某个端点的那些请求（判"发了什么"要看请求本身，
// 返回值里读不出 `moduleId` 是从哪一枪拿的）。
func funaiRequests(script *liveTaskScript, endpoint string) []core.HTTPRequest {
	var out []core.HTTPRequest
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, endpoint) {
			out = append(out, request)
		}
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 门槛判定：逐档
// ─────────────────────────────────────────────────────────────────────────────

// funaiLocal 按上游那两个**无时区**串的口径（东八区，见
// [upstream.FunaiActivityLocation]）造一个活动边界。
//
// 测试里刻意用同一个函数造边界，而不是直接写 UTC：UTC+8 这个解释本身是一处
// 判断（`core.Clock` 交 UTC、上游串不带时区），写在这里才会在改的时候响。
func funaiLocal(year int, month time.Month, day, hour, minute, second int) *time.Time {
	at := time.Date(year, month, day, hour, minute, second, 0, upstream.FunaiActivityLocation())
	return &at
}

var (
	// 实拍那一期活动的起止（`2025-05-28T00:00:00` ~ `2026-12-31T23:59:59`）。
	funaiCapturedStart = funaiLocal(2025, time.May, 28, 0, 0, 0)
	funaiCapturedEnd   = funaiLocal(2026, time.December, 31, 23, 59, 59)
	// funaiTestNow 是 [testClock] 的那一刻：东八区 2026-09-20 20:00，落在实拍窗口内。
	funaiTestNow = testClock().Now()
)

// TestPlanFunaiBranches ⭐ 逐档钉门槛。
//
// 每档注释写的都是**反过来判会怎样**（这才是这一格的代价所在）：
// 六档里只有 [FunaiPlanDraw] 会发请求，其余五档都必须不发。
func TestPlanFunaiBranches(t *testing.T) {
	// 每档都从"实拍那一档"改一两处，其余照抄：这样任何一格被改坏，
	// 差异只有那一处，读失败输出时不用先重读整行夹具。
	cases := []struct {
		name   string
		module upstream.FunaiModule
		want   FunaiPlan
	}{
		{
			// 实拍那一档：抽过 0 次、额度 1 次、在活动期内 ⇒ 该抽。
			// 反例：把它判成"没次数"就等于每天白跑一趟，且永远不会红。
			name:   "实拍档：可抽",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), TodayGetLotteryCount: int64Ptr(1), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanDraw,
		},
		{
			// 抽过一次之后（`drawsUsed` 到上限）⇒ 今天不再抽。
			// 反例：这时 `drawTimes - drawsUsed` 也正好是 0，
			// 若落到"没有次数"那句，用户听到的是"去做任务挣次数"，
			// 而真答案是"明天再来"。
			name:   "今天已抽过",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(1), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanAlreadyDrawn,
		},
		{
			// ⚠ 每日上限是**本工具**定的 1（见 [FunaiDailyDrawLimit]），
			// 不是上游给的 `drawTimes`：额度 3、已用 2 时上游还有次数，我们也不抽了。
			// 反例：拿 `drawTimes` 当上限 ⇒ 一趟发三条写请求。
			name:   "上游额度更大也只用掉每天那一次",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(3), DrawsUsed: int64Ptr(2), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanAlreadyDrawn,
		},
		{
			// 上游一次额度都没给（`0/0`）⇒ 没次数。
			// 反例：当成"判不了"会漏掉这句更有用的话；当成"可抽"会白发一条写请求。
			name:   "没有次数",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(0), DrawsUsed: int64Ptr(0), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanNoChances,
		},
		{
			// 上游给的数自相矛盾（已用比总额还多）⇒ 也**不发**。
			// 这一格真正钉的是"落不到可抽"：`drawsUsed` 已到本工具上限，
			// 所以先被"今天已抽过"那一档接走（顺序见 [PlanFunai]），
			// 不会因为减法得负数就滑到"还有次数"。
			name:   "计数倒挂也不发",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(2), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanAlreadyDrawn,
		},
		{
			// `drawTimes` 缺键 ⇒ **判不了**，既不是 0 也不是 1。
			// 反例：Go 零值会把这一档伪造成"没有次数"（漏抽），
			// 而"缺字段当有额度"会白发一条写请求把今天那一次用掉。
			name:   "判不了：drawTimes 缺",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawsUsed: int64Ptr(0), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanNoCriteria,
		},
		{
			// `null` 与"没这个键"在解析层都落成 `nil`（[upstream.FunaiModule] 全指针就是
			// 为了这一格）⇒ 同一档。反例：把 `null` 当 0 就与上面那格走岔了。
			name:   "判不了：计数是 null",
			module: upstream.FunaiModule{ModuleID: "1-250730", StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanNoCriteria,
		},
		{
			// 活动已结束。
			// 反例：这一档若还去查计数，会给用户一句"今天已经抽过"，
			// 而真答案是"活动结束了，明天来也没有"。
			name:   "不在期：已结束",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), StartTime: funaiLocal(2025, time.May, 28, 0, 0, 0), EndTime: funaiLocal(2026, time.September, 19, 23, 59, 59)},
			want:   FunaiPlanOutOfWindow,
		},
		{
			// 活动还没开始：`now.Before(start)`。
			// 反例：只判 `endTime` 单边 ⇒ 未开始也放行，抽了一次就没了。
			name:   "不在期：还没开始",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), StartTime: funaiLocal(2026, time.November, 1, 0, 0, 0), EndTime: funaiCapturedEnd},
			want:   FunaiPlanOutOfWindow,
		},
		{
			// ⭐ 顺序：明确不在期 **优先于**"今天已抽过"（两个条件同时成立）。
			// 反例：先判计数 ⇒ 活动都结束了还告诉用户"明天再来"。
			name:   "不在期优先于已抽过",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(1), StartTime: funaiCapturedStart, EndTime: funaiLocal(2026, time.September, 1, 0, 0, 0)},
			want:   FunaiPlanOutOfWindow,
		},
		{
			// ⭐ 顺序：明确不在期 **优先于**"判不了计数"——
			// 前者是一句能用的话，后者只是一句"我们不知道"。
			name:   "不在期优先于判不了",
			module: upstream.FunaiModule{ModuleID: "1-250730", StartTime: funaiCapturedStart, EndTime: funaiLocal(2026, time.January, 1, 0, 0, 0)},
			want:   FunaiPlanOutOfWindow,
		},
		{
			// 两个时间键都没给 ⇒ 活动期判不了（计数齐也不发）。
			// 反例：把"没给时间"当成"没有边界 = 在期"，就是拿一次写请求赌上游忘了挂字段。
			name:   "活动期判不了：两个时间都没给",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0)},
			want:   FunaiPlanWindowUnknown,
		},
		{
			// 只给了 `startTime` 且在期 ⇒ 有可用信息就照它判（**不**要求两个都有）。
			// 反例：要求成对出现 ⇒ 上游只挂一边时我们整年都不抽。
			name:   "单边边界也判：在期",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), StartTime: funaiCapturedStart},
			want:   FunaiPlanDraw,
		},
		{
			// 恰好压在 `endTime` 上（含端点）。
			// 反例：写成 `!now.Before(end)` 会把这一格判成已结束 ——
			// 差一秒不响，差一天少抽一次。
			name:   "端点：正好是 endTime 那一刻算在期",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), StartTime: funaiCapturedStart, EndTime: &funaiTestNow},
			want:   FunaiPlanDraw,
		},
		{
			// 两个边界互相矛盾（`start` 晚于 `end`）⇒ 落"不在期"，**不发**。
			// 反例：有人"顺手"把它判成在期 ⇒ 白发一条写请求。
			// ⚠ 这一格也是 [funaiWindowOf] 那段"倒挂必然被两条越界检查接住"的证据：
			// 换成任一时刻跑这条夹具都不会落 WindowUnknown。
			name:   "区间自相矛盾时不发",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), StartTime: funaiLocal(2026, time.November, 1, 0, 0, 0), EndTime: funaiLocal(2026, time.October, 1, 0, 0, 0)},
			want:   FunaiPlanOutOfWindow,
		},
		{
			// ⚠ `todayGetLotteryCount` **不参与判定**：一份样本说不出它与另两个计数的关系
			// （见 [upstream.FunaiModule]）。这一格钉的是"它怎么变都不改结论"。
			// 反例：哪天有人拿它当"今天还能抽几次"，这一格会立刻红。
			name:   "todayGetLotteryCount 不参与判定",
			module: upstream.FunaiModule{ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0), TodayGetLotteryCount: int64Ptr(0), StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd},
			want:   FunaiPlanDraw,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanFunai(c.module, funaiTestNow); got != c.want {
				t.Errorf("实得 %v（%s），期望 %v", got, got.Reason(), c.want)
			}
			// ⭐ 六档里只有 Draw 发请求：其余每一档的 `Reason()` 都得说得出人话，
			// 否则界面上会出现一句"趣玩AI没抽："后面空的。
			if c.want != FunaiPlanDraw && c.want.Reason() == "" {
				t.Errorf("%v 这一档没有可展示的说明", c.want)
			}
			if c.want == FunaiPlanDraw && PlanFunai(c.module, funaiTestNow).Reason() != "可以抽" {
				t.Errorf("可抽那一档的说明不该进'没抽'那句话，实得 %q", c.want.Reason())
			}
		})
	}
}

// TestPlanFunaiUsesThePassedClockNotNow：判定只看传进来的那个 `now`。
//
// 反例：实现里若写成 `time.Now()`，本仓库的时钟口径就废了（活动边界差 8 小时都测不出来），
// 而且这一格换成任何一天都会响 —— 所以用两个相距很远的 `now` 各判同一份判据。
func TestPlanFunaiUsesThePassedClockNotNow(t *testing.T) {
	module := upstream.FunaiModule{
		ModuleID: "1-250730", DrawTimes: int64Ptr(1), DrawsUsed: int64Ptr(0),
		StartTime: funaiCapturedStart, EndTime: funaiCapturedEnd,
	}
	if got := PlanFunai(module, funaiTestNow); got != FunaiPlanDraw {
		t.Errorf("窗口内应可抽，实得 %v", got)
	}
	// 窗口外两年后：同一份判据，只有 `now` 变了。
	if got := PlanFunai(module, funaiTestNow.AddDate(2, 0, 0)); got != FunaiPlanOutOfWindow {
		t.Errorf("窗口外应判'不在期'，实得 %v", got)
	}
}

// TestFunaiDailyDrawLimitIsOne：每天最多抽一次是本工具定的取向，别漂回上游规则文本里
// 那个"5"。
//
// ⚠ 那个 5 说的是"做任务**能挣到**几次抽奖机会"（`getByMarketName` 的规则原文），
// 不是"每天最多能抽几次" —— 两者混成一个数，表现就是一趟发五条写请求。
// 反例：有人把常量改成 5 并引用规则文本当出处 ⇒ 这一格红。
func TestFunaiDailyDrawLimitIsOne(t *testing.T) {
	if FunaiDailyDrawLimit != 1 {
		t.Errorf("每日上限应为 1（出处见常量注释，不是上游规则文本里的 5），实得 %d",
			FunaiDailyDrawLimit)
	}
	if !strings.Contains(FunaiPlanAlreadyDrawn.Reason(), "1") {
		t.Errorf("说明里该带上限这个数，实得 %q", FunaiPlanAlreadyDrawn.Reason())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号
// ─────────────────────────────────────────────────────────────────────────────

// TestFunaiRunDrawsWithTheModuleIDFromTheResponse ⭐ 抽那一枪的 `moduleId`
// **来自上一枪的响应**，不是写死的。
func TestFunaiRunDrawsWithTheModuleIDFromTheResponse(t *testing.T) {
	// 两档 id：实拍那一条，以及同一批抓包的 `timed/getmodule` 里同期并存的另一条。
	// 只测实拍那一条的话，"从响应里取"与"写死"在测试里长得一模一样。
	for _, moduleID := range []string{"1-250730", "2-260324-1"} {
		t.Run(moduleID, func(t *testing.T) {
			run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
					moduleID, 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}},
				liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("抽成了不该报错：%v", perr)
			}
			if got := script.count(funaiModuleEndpoint); got != 1 {
				t.Errorf("判据只读一次，实得 %d 次", got)
			}
			if got := script.count(funaiLotteryEndpoint); got != 1 {
				t.Fatalf("一趟最多抽一次，实得 %d 次", got)
			}
			// ⭐ 逐字钉请求体：`content-length: 23`，只有 `moduleId` 一个键，
			// 值是**上一枪**给的那个。
			lottery := funaiRequests(script, funaiLotteryEndpoint)[0]
			if want := `{"moduleId":"` + moduleID + `"}`; string(lottery.Body) != want {
				t.Errorf("请求体应逐字是 %s，实得 %s", want, string(lottery.Body))
			}
			if lottery.Method != core.MethodPost {
				t.Errorf("抽奖应是 POST，实得 %v", lottery.Method)
			}
			if res.Status != FunaiDone {
				t.Errorf("实得 %v（%q），期望 %v", res.Status, res.Reason, FunaiDone)
			}
			if res.PrizeName == nil || *res.PrizeName != "30云朵" {
				t.Errorf("应带出上游报的奖品名，实得 %v", res.PrizeName)
			}
			if got := res.SummaryLine(); got != "趣玩AI抽到「30云朵」" {
				t.Errorf("小结实得 %q", got)
			}
			if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
				t.Errorf("抽到了应记成功，实得 %v", report.Outcome)
			}
		})
	}
}

// TestFunaiRunSendsNoWriteRequestWhenTheGateIsClosed ⭐ 五档关闭的门槛 ⇒
// **该不发时一条请求都不发**。
//
// 与 [PlanFunai] 那张表分开测：那张只证明"判得对"，这一条证明"判完真的没发" ——
// 中间隔着取票与装配，漏一层判断的表现全在请求数上，返回值里一点看不出来。
func TestFunaiRunSendsNoWriteRequestWhenTheGateIsClosed(t *testing.T) {
	cases := []struct {
		name    string
		body    string
		wantSub string // `Reason` 里该出现的那几个字
	}{
		{"今天已抽过", funaiModuleBody("1-250730", 1, 1, "2025-05-28T00:00:00", "2026-12-31T23:59:59"), "今天已经抽过"},
		{"没有次数", funaiModuleBody("1-250730", 0, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59"), "没有可用次数"},
		{"上游没给判据字段", funaiNullCountsBody, "没给次数判据字段"},
		{"活动不在期", funaiModuleBody("1-250730", 1, 0, "2025-05-28T00:00:00", "2026-09-19T23:59:59"), "不在活动起止时间内"},
		{"活动期判不了", funaiModuleBody("1-250730", 1, 0, "", ""), "活动起止时间判不了"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{funaiModuleEndpoint, []string{c.body}})
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("没得抽不是错误：%v", perr)
			}
			if got := script.count(funaiLotteryEndpoint); got != 0 {
				t.Errorf("门槛不成立就一条写请求都不该发，实得 %d 次", got)
			}
			// 判据那一枪每趟都要打（它是"该不该抽"的唯一来源），但只打一次。
			if got := script.count(funaiModuleEndpoint); got != 1 {
				t.Errorf("判据应读一次，实得 %d 次", got)
			}
			if res.Status != FunaiNothing {
				t.Errorf("实得 %v，期望 %v", res.Status, FunaiNothing)
			}
			if !strings.Contains(res.Reason, c.wantSub) {
				t.Errorf("理由里应有 %q，实得 %q", c.wantSub, res.Reason)
			}
			if got := res.SummaryLine(); !strings.HasPrefix(got, "趣玩AI没抽：") {
				t.Errorf("小结应是'没抽：'+理由，实得 %q", got)
			}
			// "来一趟确认现在不该抽"就是达成了 ⇒ **成功**（挂上定时任务才不会每天一条红）。
			if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
				t.Errorf("没发请求的分支应记成功，实得 %v", report.Outcome)
			}
		})
	}
}

// TestFunaiRunQuotesReadingsOnTheNothingBranch ⭐ "没抽"那一档必须把上游读数带出来。
//
// 2026-09-25 真账号那轮的实际观感：界面上只留下"趣玩AI没抽：上游报的次数判据显示
// 没有可用次数"，于是这一趟**答不出**任何一条判据问题（`drawTimes` 是几、`drawsUsed`
// 满没满、活动期到哪天）。R3 与"次数判据语义"两格都只能靠**下一次真跑的读数**收口，
// 所以读数进小结是功能而不是修饰。
//
// 反例：把 `SummaryLine` 里那段 `res.Status != FunaiDone` 的判断删掉 ⇒ 抽成那一档
// 也跟着念抽奖**之前**的读数，[TestFunaiRunDrawsWithTheModuleIDFromTheResponse]
// 那句逐字断言会红；反过来把整段追加拿掉 ⇒ 本条红。
func TestFunaiRunQuotesReadingsOnTheNothingBranch(t *testing.T) {
	cases := []struct {
		name string
		body string
		// wantSubs 是这一档读数里该出现的片段。
		wantSubs []string
	}{
		{
			"次数读到了",
			funaiModuleBody("1-250730", 0, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59"),
			[]string{"drawTimes=0", "drawsUsed=0", "todayGetLotteryCount=1",
				"起 2025-05-28", "止 2026-12-31"},
		},
		{
			// "缺"与 0 是两件事：这里三个计数在实拍里就全是 null。
			"上游没给判据字段",
			funaiNullCountsBody,
			[]string{"drawTimes=缺", "drawsUsed=缺", "todayGetLotteryCount=缺"},
		},
		{
			// 两界读不到就不念：念成空串会让人以为上游给了个空日期。
			"活动期判不了",
			funaiModuleBody("1-250730", 1, 0, "", ""),
			[]string{"drawTimes=1"},
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, _ := funaiRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{funaiModuleEndpoint, []string{c.body}})
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("没得抽不是错误：%v", perr)
			}
			got := res.SummaryLine()
			if !strings.HasPrefix(got, "趣玩AI没抽：") {
				t.Fatalf("这一档仍应以'没抽：'+理由开头，实得 %q", got)
			}
			if !strings.Contains(got, "（读数：") {
				t.Fatalf("没抽的那一趟是唯一还拿得到抽奖前读数的趟，小结里必须有读数：%q", got)
			}
			for _, want := range c.wantSubs {
				if !strings.Contains(got, want) {
					t.Errorf("读数里应有 %q，实得 %q", want, got)
				}
			}
			switch c.name {
			case "活动期判不了":
				if strings.Contains(got, "起 ") || strings.Contains(got, "止 ") {
					t.Errorf("两界读不到时不许念日期：%q", got)
				}
			case "上游没给判据字段":
				if strings.Contains(got, "drawTimes=0") {
					t.Errorf("缺的键念成 0 就把'上游没给'说成了'给了 0'：%q", got)
				}
			}
		})
	}
}

// TestFunaiRunDoesNotQuoteReadingsAfterDrawing：抽成了不念读数 ——
// 那几格是 `page/getmodule` 在抽奖**之前**读的，`drawsUsed` 那时还没 +1。
//
// 反向改法是把 `SummaryLine` 的 `res.Status != FunaiDone` 改成无条件追加 ⇒ 这里红。
func TestFunaiRunDoesNotQuoteReadingsAfterDrawing(t *testing.T) {
	run, _ := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
			"1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}},
		liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("抽成了不该报错：%v", perr)
	}
	if res.Status != FunaiDone {
		t.Fatalf("前置条件坏了：这一趟应抽成，实得 %v（%q）", res.Status, res.SummaryLine())
	}
	// 读数仍在结果里（它属于事实，只是不念），只是别出现在出网的那句话上。
	if res.Readings.DrawsUsed == nil || *res.Readings.DrawsUsed != 0 {
		t.Errorf("结果里应带着抽奖前的原始读数，实得 %v", res.Readings.DrawsUsed)
	}
	if got := res.SummaryLine(); strings.Contains(got, "读数") {
		t.Errorf("抽成之后念抽奖前的读数会被读成'还剩这些'：%q", got)
	}
}

// TestFunaiRunDailyLimitAcrossTwoRounds ⭐ "每天最多抽一次"这条规则本身。
//
// 这里**不接票池**（`Sessions` 留 nil），所以两轮各自都会重新建会话、重新读判据 ——
// 唯一的依赖就是上游那份 `drawsUsed`。第一趟抽完，第二趟上游回"已用过"，
// 于是第二趟**一条写请求都不发**。
//
// 反例：漏掉门槛改成"每趟必抽"⇒ 第二趟也会打一次，总数变 2；
// 反例：把判据那一枪也省了（缓存"今天抽过了"）⇒ 第一格 `getmodule` 计数变 1，
// 而我们的口径是"每次都以现场判据为准"。
func TestFunaiRunDailyLimitAcrossTwoRounds(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{
			// 第一轮：还没抽；第二轮：抽过了。队列用尽后重复最后一条，
			// 所以第三、四轮也仍是"已抽过" —— 与真实上游行为一致。
			funaiModuleBody("1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59"),
			funaiModuleBody("1-250730", 1, 1, "2025-05-28T00:00:00", "2026-12-31T23:59:59"),
		}},
		liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})

	for round := 1; round <= 2; round++ {
		res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
		if perr != nil {
			t.Fatalf("第 %d 轮报错：%v", round, perr)
		}
		if got := script.count(funaiLotteryEndpoint); got != 1 {
			t.Fatalf("第 %d 轮之后全天累计应只有 1 次抽奖，实得 %d 次", round, got)
		}
		if got := script.count(funaiModuleEndpoint); got != round {
			t.Errorf("第 %d 轮应各读一次判据，累计实得 %d 次", round, got)
		}
		want := FunaiDone
		if round == 2 {
			want = FunaiNothing
		}
		if res.Status != want {
			t.Errorf("第 %d 轮实得 %v，期望 %v（%q）", round, res.Status, want, res.SummaryLine())
		}
	}
}

// TestFunaiRunReturnsErrorWhenTheCriteriaQueryFails：读判据那一枪失败 ⇒ 回错误，
// 并且**不发**抽奖请求。
//
// 反例：把"读判据失败"折成 [FunaiNothing] ⇒ 界面上一句"今天没得抽"，
// 而真答案是"上游那句话都没读到"（处置完全不同：重试还是收手）。
func TestFunaiRunReturnsErrorWhenTheCriteriaQueryFails(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{`{"code":1,"msg":"系统繁忙"}`}})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("读判据失败应回错误")
	}
	if got := script.count(funaiLotteryEndpoint); got != 0 {
		t.Errorf("判据没读到就一条写请求都不该发，实得 %d 次", got)
	}
	// 错误串要带着上游那句原话，否则排查时只剩一句"请求失败"。
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Error())
	}
	if res.Status != FunaiFailed {
		t.Errorf("走错误那条路时结论也该摆成失败（编排层仍会取小结），实得 %v", res.Status)
	}
	if res.Account != core.NewAccountID(liveTestAccountID) {
		t.Errorf("即便报错也该带上账号，实得 %q", res.Account)
	}
}

// TestFunaiRunFoldsLotteryFailureIntoAFailedStatusNotAnError：抽奖那一枪回非 0 ⇒
// 折成**失败结论**，不回错误。
//
// 与 [ClaimMultiple] / [ClaimBackupGift] 同一口径：折成结论才能把"上游回什么"带进
// `runs.message`，而回错误会多发一条 `errored` 并把这一趟的结论丢掉。
//
// ⚠ 这里的响应是**合成**的："无次数"那一类失败回什么码、什么文案，归档里
// **一个样本都没有** ⇒ 不猜码值、也不猜"是否算已抽过这一次"。
// 补样办法：只能拿真账号在额度耗尽后故意再抽一次，看 `code` 与 `msg` 逐字
// （活动窗外补不了）。
func TestFunaiRunFoldsLotteryFailureIntoAFailedStatusNotAnError(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
			"1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}},
		liveTaskRoute{funaiLotteryEndpoint, []string{`{"code":1,"msg":"合成：非 0 码"}`}})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("抽奖失败应折成结论而不是错误：%v", perr)
	}
	if res.Status != FunaiFailed {
		t.Errorf("实得 %v，期望 %v", res.Status, FunaiFailed)
	}
	if !strings.Contains(res.Error, "合成：非 0 码") {
		t.Errorf("失败原因该带着上游那句话，实得 %q", res.Error)
	}
	if got := script.count(funaiLotteryEndpoint); got != 1 {
		t.Errorf("失败了也不自动重试（重试可能重复扣额度），实得 %d 次", got)
	}
	report := res.RunReport()
	if report.Outcome != core.OutcomeFailed {
		t.Errorf("请求发了但失败 ⇒ 应记失败，实得 %v", report.Outcome)
	}
	if report.Summary == nil || !strings.HasPrefix(*report.Summary, "趣玩AI抽奖失败") {
		t.Errorf("小结实得 %v", report.Summary)
	}
}

// TestFunaiResultOutcomeMapping：三档结论 → 四态 + 那句话逐字。
//
// 这些字符串会进 `runs.message` 与推送标题，**出网**，所以逐字钉。
// ⚠ 没有 [core.OutcomeManualRequired] 那一档（理由见 [FunaiResult.RunReport]）：
// 这里断言 `ManualSteps` 是**非 nil 空切片**，而不是"整格没填"。
func TestFunaiResultOutcomeMapping(t *testing.T) {
	prize := "30云朵"
	noPrizeName := ""
	cases := []struct {
		name    string
		result  FunaiResult
		summary string
		outcome core.RunOutcome
	}{
		{
			name:    "抽到了",
			result:  FunaiResult{Status: FunaiDone, PrizeName: &prize},
			summary: "趣玩AI抽到「30云朵」", outcome: core.OutcomeSuccess,
		},
		{
			// 上游没说奖品名 ≠ 没中奖：那句话得留着，不能编一个"未中奖"。
			name:    "抽到了但上游没报奖品名",
			result:  FunaiResult{Status: FunaiDone},
			summary: "趣玩AI已抽一次（上游没报奖品名）", outcome: core.OutcomeSuccess,
		},
		{
			name:    "奖品名是空串",
			result:  FunaiResult{Status: FunaiDone, PrizeName: &noPrizeName},
			summary: "趣玩AI已抽一次（上游没报奖品名）", outcome: core.OutcomeSuccess,
		},
		{
			name: "没抽：理由已填",
			result: FunaiResult{Status: FunaiNothing, Plan: FunaiPlanAlreadyDrawn,
				Reason: FunaiPlanAlreadyDrawn.Reason()},
			summary: "趣玩AI没抽：" + FunaiPlanAlreadyDrawn.Reason(), outcome: core.OutcomeSuccess,
		},
		{
			// ⚠ 只给档位、没填 `Reason`（编排层直接拿零值时）：也要说一句能懂的话。
			// 反例：写成空串 ⇒ 历史里一条空白记录。
			name:    "没抽：理由没填时退回按档位说",
			result:  FunaiResult{Status: FunaiNothing, Plan: FunaiPlanNoCriteria},
			summary: "趣玩AI没抽：" + FunaiPlanNoCriteria.Reason(), outcome: core.OutcomeSuccess,
		},
		{
			name:    "失败：带原因",
			result:  FunaiResult{Status: FunaiFailed, Error: "上游回了合成文案"},
			summary: "趣玩AI抽奖失败：上游回了合成文案", outcome: core.OutcomeFailed,
		},
		{
			name:    "失败：原因空的",
			result:  FunaiResult{Status: FunaiFailed},
			summary: "趣玩AI抽奖失败", outcome: core.OutcomeFailed,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := c.result.SummaryLine(); got != c.summary {
				t.Errorf("小结逐字应 %q，实得 %q", c.summary, got)
			}
			report := c.result.RunReport()
			if report.Outcome != c.outcome {
				t.Errorf("实得 %v，期望 %v", report.Outcome, c.outcome)
			}
			if report.ManualSteps == nil {
				t.Error("ManualSteps 应是非 nil 空切片（与其余链同口径，服务端直接遍历它）")
			}
			if len(report.ManualSteps) != 0 {
				t.Errorf("本能力没有'必须人去做'的事，实得 %d 条", len(report.ManualSteps))
			}
			if report.Summary == nil || *report.Summary != c.summary {
				t.Errorf("报告里的小结应同一句，实得 %v vs %q", report.Summary, c.summary)
			}
		})
	}
}

// TestFunaiRunProgressSaysWhy：跳过那一趟必须**说人话**地报出来。
//
// 反例：只 return 结论不发过程 ⇒ 界面上"今天没抽"与"这一趟没跑到"长得一样。
func TestFunaiRunProgressSaysWhy(t *testing.T) {
	t.Run("没抽", func(t *testing.T) {
		progress := &liveTaskProgress{}
		run, _ := funaiRuns(&poolSignInStore{*liveTaskStore()},
			liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
				"1-250730", 1, 1, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}})
		run.Progress = progress.handle()
		if _, perr := run.RunAccount(context.Background(),
			core.NewAccountID(liveTestAccountID)); perr != nil {
			t.Fatalf("不该报错：%v", perr)
		}
		if !progress.has(core.LevelInfo, "趣玩AI没抽：今天已经抽过") {
			t.Errorf("应有一句 info 说明为什么没抽，实得 %q", progress.joined())
		}
	})
	t.Run("抽了", func(t *testing.T) {
		progress := &liveTaskProgress{}
		run, _ := funaiRuns(&poolSignInStore{*liveTaskStore()},
			liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
				"1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}},
			liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})
		run.Progress = progress.handle()
		if _, perr := run.RunAccount(context.Background(),
			core.NewAccountID(liveTestAccountID)); perr != nil {
			t.Fatalf("不该报错：%v", perr)
		}
		if !progress.has(core.LevelInfo, "趣玩AI已抽一次：趣玩AI抽到") {
			t.Errorf("应有一句 info 说抽到了什么，实得 %q", progress.joined())
		}
	})
}

// TestFunaiRunWithdrawsNothingWithoutCredential：库里没凭据 ⇒ 一次出网都不该有。
//
// 这一条钉 [SignInWorld.Source] 的顺序：先取票再查库。写反了的表现是
// "为一个库里没有的账号建了一条会话"，那三枪会带上别人的 token。
func TestFunaiRunWithdrawsNothingWithoutCredential(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{fakeStore{state: nil}})
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("库里没有凭据应该报错")
	}
	if len(script.snapshot()) != 0 {
		t.Errorf("取不到凭据就不该出网，实得 %d 条请求", len(script.snapshot()))
	}
}

// TestFunaiRunWithoutClockDoesNotGoOut ⭐ 没有时钟 ⇒ 活动期判不了 ⇒ 一条请求都不发。
//
// 这一格是 [core.Clock] 作为入参的全部理由：反例是实现里 fallback 成 `time.Now()`
// （编译不出错、跑起来也对，只有测试能抓住"忘了传时钟"这件事），
// 或 fallback 成"当成在期" —— 后者会拿今天唯一那一次去赌。
func TestFunaiRunWithoutClockDoesNotGoOut(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
			"1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}})
	run.Clock = nil
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("没有时钟应报错")
	}
	if !strings.Contains(perr.Error(), "没有时钟") {
		t.Errorf("错误里应说清是时钟的事，实得 %q", perr.Error())
	}
	if res.Status != FunaiFailed {
		t.Errorf("结论也该摆成失败，实得 %v", res.Status)
	}
	if len(script.snapshot()) != 0 {
		t.Errorf("判不了就一条请求都不该发，实得 %d 条", len(script.snapshot()))
	}
}

// TestFunaiRunSendsNoDeviceHeaders：这一条链**缺省不带** `deviceid` 头。
//
// 实拍两条请求都带（值已脱敏），服务端是否要它未验证 ⇒ 实现传 `nil`。
// 这一格钉的是"我们这一侧不发"：哪天有人把它接成必发，这里会红，
// 那时再决定（要先补样，见 `FunaiDraw` 的注释）。
// 顺带钉 `isdeviceid` 一个都没有，以及每条 funai 请求恰好带一次 `jwttoken`。
func TestFunaiRunSendsNoDeviceHeaders(t *testing.T) {
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{funaiModuleBody(
			"1-250730", 1, 0, "2025-05-28T00:00:00", "2026-12-31T23:59:59")}},
		liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("不该报错：%v", perr)
	}
	total := 0
	for _, request := range script.snapshot() {
		if !strings.Contains(request.URL, "/funai/") {
			continue
		}
		total++
		jwt := 0
		for _, pair := range request.Headers {
			switch strings.ToLower(pair[0]) {
			case "deviceid", "isdeviceid":
				t.Errorf("%s 不该带 %s 头", request.URL, pair[0])
			case "jwttoken":
				jwt++
			}
		}
		if jwt != 1 {
			t.Errorf("每条 funai 请求应恰好带一次 jwttoken，实得 %d 次", jwt)
		}
	}
	if total != 2 {
		t.Errorf("判据 + 抽奖应共两条 funai 请求，实得 %d 条", total)
	}
}

// TestFunaiRunDoesNotBranchOnTheUnverifiedFields：`isDraw` / `drawTime` / `drawResult`
// 在实拍里全是 `null`，非 `null` 形态**一个样本都没有** ⇒ 本链不据它们分支。
//
// 这一格的用处是**防漂移**：现在解析器根本不声明这三个字段，将来有人加进来并拿它当
// "今天抽过没"的判据，这里会立刻红（那三个字段的歧义登记在
// [upstream.FunaiModule]，补样办法也写在那里）。
func TestFunaiRunDoesNotBranchOnTheUnverifiedFields(t *testing.T) {
	// 把三个字段填成"看起来像今天抽过了"的形状，判据其余部分与实拍"可抽"那一档相同。
	withUnverified := `{"code":0,"msg":"success","result":{` +
		`"moduleId":"1-250730","drawTimes":1,"drawsUsed":0,"todayGetLotteryCount":1,` +
		`"startTime":"2025-05-28T00:00:00","endTime":"2026-12-31T23:59:59",` +
		`"isDraw":1,"drawTime":"2026-09-20T20:00:00","drawResult":"合成"}}`
	run, script := funaiRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{funaiModuleEndpoint, []string{withUnverified}},
		liveTaskRoute{funaiLotteryEndpoint, []string{funaiCapturedBody}})
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("不该报错：%v", perr)
	}
	if got := script.count(funaiLotteryEndpoint); got != 1 {
		t.Errorf("结论只该来自三个计数，那三个字段是摆设：抽奖请求实得 %d 次，期望 1 次", got)
	}
}
