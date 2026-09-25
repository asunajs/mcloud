package app

// 公告任务（`notice`）跑一个账号那一段的用例。
//
// 这一层的判点只有三个，而且每一个都是"错了不会有人发现"的那类：
//
//  1. **判据不成立时一条写请求都不发** —— `reward` 幂等与否零样本，多发一次的后果
//     没人说得出（与 `TestClaimRunSendsNoWriteRequestWhenTheGateIsClosed` 同一族）。
//  2. **领完确实再查了一次** —— 不看这一步，"到底领没领上"就只剩上游那句 `success`。
//  3. **三种失败走三条路**：查判据失败交回错误、`reward` 失败折成结论、
//     复查失败交回错误但结论仍是"已领"。最后那条最容易写反（写成失败会让人再点一次）。
//
// ⚠ 档位判定本身（`PlanNotice` 的映射方向）在这里**只钉形状不钉语义**：
// 实拍只支持"该发"这一半，方向是猜的，写死成断言反而是假的确定 ——
// 见 `notice_run.go` 里 `noticeSlotClaimable` 上面那一大段。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 这一族的端点子串。写成常量是因为断言两头都要用：
// "该打的那条打了几次"与"另一条一次都没打"。
const (
	noticeInfoEndpoint   = "/openEmailsms/getTaskInfo"
	noticeRewardEndpoint = "/openEmailsms/reward"
)

// 两份读数：`capturedNoticeTaskInfo` 的复刻（app 层不能引 upstream 测试包里的常量），
// 与一份"两槽都读成已入账"的夹具。
const (
	noticeInfoCaptured = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},` +
		`"taskTwoInfo":{"cloudStatus":1,"progress":31,"sum":100,"taskStatus":3}}}`
	noticeInfoBothClaimed = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},` +
		`"taskTwoInfo":{"cloudStatus":2,"sum":100,"taskStatus":3}}}`
	// noticeInfoBothZero 复刻 **2026-09-25 真账号那一趟**读到的两槽（两个号里的一号）：
	// `cloudStatus` 两槽都是 0，`taskStatus` 分别是 0 与 1。0 既不是可领（1）也不是
	// 已入账（2）⇒ 语义未验证，见 `notice_run.go` 里那一段读数面。
	noticeInfoBothZero = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"cloudStatus":0,"sum":200,"taskStatus":0},` +
		`"taskTwoInfo":{"cloudStatus":0,"sum":100,"taskStatus":1}}}`
	noticeInfoNoFields = `{"code":0,"msg":"success","result":{` +
		`"taskOneInfo":{"sum":200},"taskTwoInfo":{"sum":100}}}`
	noticeInfoNoSlots = `{"code":0,"msg":"success","result":{}}`
	noticeOK          = `{"code":0,"msg":"success"}`
)

func noticeRuns(store core.StateStore, routes ...liveTaskRoute) (NoticeRun, *liveTaskScript) {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	return NoticeRun{SignInWorld: signInWorldFor(store, script)}, script
}

// noticePtr 造 `*int64`（表里要区分"给了 0"与"没给"）。
func noticePtr(v int64) *int64 { return &v }

// ─────────────────────────────────────────────────────────────────────────────
// 判定（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// TestPlanNoticeBranches：三档可达、第四档今天不可达。
//
// ⚠ "该领"这一档**只**由实拍那两条支持，且**方向是猜的**：
// 把 `noticeCloudStatusUnclaimed` 从 1 改成 2，本条**照样全绿** ——
// 因为实拍那两条在两个方向上都会得出 Reward（见 `notice_run.go` 里那段"反向映射"）。
// 真正会红的是"两槽都已入账"那一档：方向改反 ⇒ 下面 `都已领 ⇒ Nothing` 那条变红。
// 这就是眼下能诚实钉到的边界，再钉多一点就是编。
func TestPlanNoticeBranches(t *testing.T) {
	cases := []struct {
		name string
		info upstream.NoticeTaskInfo
		want NoticePlan
	}{
		{
			name: "实拍那一份：槽二读成未领 ⇒ 该领",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(2), Sum: noticePtr(200), TaskStatus: noticePtr(3),
				},
				TaskTwoInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(1), Sum: noticePtr(100),
					Progress: noticePtr(31), TaskStatus: noticePtr(3),
				},
			},
			want: NoticePlanReward,
		},
		{
			name: "两槽都读成已入账 ⇒ 没得领",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(2), TaskStatus: noticePtr(3),
				},
				TaskTwoInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(2), TaskStatus: noticePtr(3),
				},
			},
			want: NoticePlanNothing,
		},
		{
			name: "taskStatus 不是「已完成」那个值 ⇒ 没得领",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(1), TaskStatus: noticePtr(1),
				},
			},
			want: NoticePlanNothing,
		},
		{
			name: "判据字段一个都没给 ⇒ 判不了",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{Sum: noticePtr(200)},
				TaskTwoInfo: &upstream.NoticeTaskSlot{Sum: noticePtr(100)},
			},
			want: NoticePlanUnknown,
		},
		{
			name: "两槽都不存在 ⇒ 判不了",
			info: upstream.NoticeTaskInfo{},
			want: NoticePlanUnknown,
		},
		{
			name: "只缺一个判据字段 ⇒ 这一槽不参与判断，另一槽也判不了 ⇒ Unknown",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{CloudStatus: noticePtr(1)},
				TaskTwoInfo: &upstream.NoticeTaskSlot{TaskStatus: noticePtr(3)},
			},
			want: NoticePlanUnknown,
		},
		{
			// 一槽能判（已入账）、另一槽缺字段 ⇒ **Nothing** 而不是 Unknown：
			// 有一个可判的槽就够了，但"能不能判"这件事仍由 Unknown 那档负责（上面两条）。
			name: "一槽可判且读成已入账、另一槽缺字段 ⇒ Nothing",
			info: upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(2), TaskStatus: noticePtr(3),
				},
				TaskTwoInfo: &upstream.NoticeTaskSlot{Sum: noticePtr(100)},
			},
			want: NoticePlanNothing,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := PlanNotice(c.info); got != c.want {
				t.Errorf("实得 %v，期望 %v", got, c.want)
			}
		})
	}
}

// TestPlanNoticeManualIsUnreachableToday：Manual 那一档**今天没有代码路径**能到。
//
// ⭐ 这条断言的作用不是"证明 Manual 对"，而是**挡住偷偷造一个判据**：
// 哪天有人拿 `cloudStatus==0` 之类的直觉去填 `noticeNeedsManual`，本条立刻变红，
// 逼他先去把"未开启"那条例文抓到并落档（`AGENTS.md` §1.7：未验证的不许断言）。
// 结果层（[NoticeManual] → [core.OutcomeManualRequired]）另有用例钉着，见下面那条。
func TestPlanNoticeManualIsUnreachableToday(t *testing.T) {
	for _, cloud := range []int64{0, 1, 2, 3, -1} {
		for _, status := range []int64{0, 1, 2, 3, -1} {
			info := upstream.NoticeTaskInfo{
				TaskOneInfo: &upstream.NoticeTaskSlot{
					CloudStatus: noticePtr(cloud), TaskStatus: noticePtr(status),
				},
			}
			if got := PlanNotice(info); got == NoticePlanManual {
				t.Errorf("cloudStatus=%d taskStatus=%d 竟判成 Manual："+
					"这一档的判据还没抓到（见 noticeNeedsManual），不许凭直觉点亮", cloud, status)
			}
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// TestNoticeRunClaimsThenRechecks：⭐ 领完**必须**再查一次，且小结念的是**复查**那份读数。
//
// 夹具故意让两份读数不同（判据那份槽二是 `cloudStatus=1`、复查那份是 `2`），
// 这样"把领之前的数当复查结果念出来"会当场红 —— 而那正是最容易犯、也最难看出来的一处。
func TestNoticeRunClaimsThenRechecks(t *testing.T) {
	run, script := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{
			noticeInfoCaptured,
			`{"code":0,"msg":"success","result":{` +
				`"taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},` +
				`"taskTwoInfo":{"cloudStatus":2,"sum":100,"taskStatus":3}}}`,
		}},
		liveTaskRoute{noticeRewardEndpoint, []string{noticeOK}},
	)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	if got := script.count(noticeInfoEndpoint); got != 2 {
		t.Errorf("reward 之后**必须**再查一次 getTaskInfo（那口报不出数），实得打了 %d 次", got)
	}
	if got := script.count(noticeRewardEndpoint); got != 1 {
		t.Errorf("reward 应打 1 次，实得 %d", got)
	}
	if res.Status != NoticeDone {
		t.Fatalf("应判已领，实得 %v（%s）", res.Status, res.Error)
	}
	// 复查那份里两槽都是 cloudStatus=2 —— 念成判据那份（槽二 = 1）就是这条断言要抓的错。
	if res.TaskTwo == nil || res.TaskTwo.CloudStatus == nil || *res.TaskTwo.CloudStatus != 2 {
		t.Fatalf("TaskTwo 该是**复查之后**的读数（cloudStatus=2），实得 %+v", res.TaskTwo)
	}
	summary := res.SummaryLine()
	if !strings.Contains(summary, "复查") || !strings.Contains(summary, "cloudStatus=2") {
		t.Errorf("小结该带复查读数，实得 %q", summary)
	}
	if strings.Contains(summary, "cloudStatus=1") {
		t.Errorf("小结里不该再有判据那份的 cloudStatus=1，实得 %q", summary)
	}
	// 这条链上**没有** reward 报出的数（实拍那条响应连 result 都没有）。
	if res.Gained != nil {
		t.Errorf("reward 报不出数，Gained 该是 nil，实得 %d", *res.Gained)
	}
	// ⚠ 凭据卫生：小结会进 `runs.message` 与推送，绝不能带票。
	if strings.Contains(summary, "JWT-1") || strings.Contains(summary, "jwttoken") {
		t.Errorf("小结里漏了凭据形态的东西：%q", summary)
	}
}

// TestNoticeRunSendsNoWriteRequestWhenTheGateIsClosed：判据说"不该领"或"判不了"
// ⇒ `reward` 与小节的走法。
//
// 与 `TestClaimRunSendsNoWriteRequestWhenTheGateIsClosed` 同一族：中间隔了
// [SignInWorld.Source] 与 [ClaimNotice] 两层装配，装配写错的表现正是
// "发了一条本不该发的写请求"，而这条链上那一次的后果**没人说得清**。
func TestNoticeRunSendsNoWriteRequestWhenTheGateIsClosed(t *testing.T) {
	cases := []struct {
		name       string
		body       string
		wantStatus NoticeStatus
	}{
		{"两槽都已入账", noticeInfoBothClaimed, NoticeNothing},
		{"判据字段缺失", noticeInfoNoFields, NoticeUnknown},
		{"上游一槽都没给", noticeInfoNoSlots, NoticeUnknown},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, script := noticeRuns(&poolSignInStore{*liveTaskStore()},
				liveTaskRoute{noticeInfoEndpoint, []string{c.body}},
				liveTaskRoute{noticeRewardEndpoint, []string{noticeOK}},
			)
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("判得出来这一趟不该报错：%v", perr)
			}
			if got := script.count(noticeRewardEndpoint); got != 0 {
				t.Errorf("判据不成立就一条写请求都不该发，reward 实得 %d 次", got)
			}
			// 也**不该**复查：判不了还多打一发，等于把"什么都不发"改成"发两发只读"。
			if got := script.count(noticeInfoEndpoint); got != 1 {
				t.Errorf("getTaskInfo 只该打 1 次（没有可领的东西就没得复查），实得 %d", got)
			}
			if res.Status != c.wantStatus {
				t.Errorf("实得 %v，期望 %v（理由 %q）", res.Status, c.wantStatus, res.Reason)
			}
			if report := res.RunReport(); report.Outcome == core.OutcomeFailed {
				t.Errorf("没发请求的分支不该报失败：%s", res.SummaryLine())
			}
		})
	}
}

// TestNoticeRunReadsSlotsIntoTheNothingSummary：⭐ "没领"那一档也要把**读数**念出来。
//
// 2026-09-25 真账号那一趟跑出来的就是这一档，而它只留下一句
// "公告任务没领：能判的槽都读成已入账"（旧文案，现已改写，见 project）—— 这一趟**答不出** R1
// （"`cloudStatus` 哪个值是未领"），尽管数就在手上。补上读数后，
// 下一次真跑无论判成哪一档都会带着 `cloudStatus=` 的原样读数。
// 反向改法：把 `noticeSlotReadings` 从 Nothing 那一支摘掉 ⇒ 这一条红。
func TestNoticeRunReadsSlotsIntoTheNothingSummary(t *testing.T) {
	run, _ := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{noticeInfoBothClaimed}},
		liveTaskRoute{noticeRewardEndpoint, []string{noticeOK}},
	)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("这一趟判得出来，不该报错：%v", perr)
	}
	if res.Status != NoticeNothing {
		t.Fatalf("夹具该判成没领，实得 %v", res.Status)
	}
	summary := res.SummaryLine()
	if !strings.Contains(summary, "cloudStatus=2") {
		t.Errorf("没领那一档也要带上游读数：%q", summary)
	}
	// 照抄，不解释：这句未验证的映射不许出现在小结里（判据见 SummaryLine 头注）。
	for _, forbidden := range []string{"已领", "未领", "未入账"} {
		if strings.Contains(summary, forbidden) && !strings.Contains(summary, "没领") {
			t.Errorf("小结不许替读数下结论，出现 %q：%q", forbidden, summary)
		}
	}
	if strings.Contains(summary, "JWT-1") || strings.Contains(summary, "jwttoken") {
		t.Errorf("小结里漏了凭据形态的东西：%q", summary)
	}

	// 一槽都没读到时**不**念"无槽，无槽"：那是占掉小结一半的没信息量的话。
	empty, _ := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{noticeInfoNoSlots}},
	)
	res, perr = empty.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("判不了也不报错：%v", perr)
	}
	if got := res.SummaryLine(); strings.Contains(got, "无槽") {
		t.Errorf("没读到槽时该一个字都不加，实得 %q", got)
	}

	// 只缺判据字段那一档：念"缺"而不是念 0（"没给"与"给了 0"是两件事）。
	sparse, _ := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{noticeInfoNoFields}},
	)
	res, perr = sparse.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("判不了也不报错：%v", perr)
	}
	if got := res.SummaryLine(); !strings.Contains(got, "cloudStatus=缺") {
		t.Errorf("缺的键要念成缺，实得 %q", got)
	}
}

// TestNoticeRunZeroReadingIsNotNarratedAsSettled：⭐ "没领"那一档**不许**替一个
// 两档之外的读数下"已入账"的结论（2026-09-25 真账号那一趟读到的就是 `cloudStatus=0`）。
//
// 判据的形状：`cloudStatus` 的已知映射只有 `1=未领` / `2=已入账`（且那本身是猜的，
// 见 `notice_run.go` 那两个常量），而 `0` 不在里面 ⇒ 判定照旧落"没领、一枪不发"
// （保守那一侧不变），但**文案**必须只说"没有一槽读成可领"，不能说"都读成已入账"。
// 反向改法有两处，各自会红：把 0 当 2 用（→ 念"已入账"，本条红）；
// 或把 `noticeCloudStatusUnclaimed` 改成 0（→ 这一趟真发 `reward`，`res.Status` 变 Done，本条也红）。
// 它钉不住的是"0 到底是什么"—— 那一问仍要拿一个从没开过通知的号读一次（R1）。
func TestNoticeRunZeroReadingIsNotNarratedAsSettled(t *testing.T) {
	run, _ := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{noticeInfoBothZero}},
		liveTaskRoute{noticeRewardEndpoint, []string{noticeOK}},
	)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("判成没领不该报错：%v", perr)
	}
	if res.Status != NoticeNothing {
		t.Fatalf("两槽都读成 0 时仍该判没领（一枪不发），实得 %v", res.Status)
	}
	summary := res.SummaryLine()
	if !strings.Contains(summary, "cloudStatus=0") {
		t.Errorf("读数原样带上：%q", summary)
	}
	if strings.Contains(summary, "已入账") {
		t.Errorf("0 不是已知两档之一，不许念成已入账：%q", summary)
	}
}

// TestNoticeRunReportsClaimedWhenTheRecheckFails：⭐ 复查挂了 ⇒ **交回错误**，
// 但结论**仍是**"已领"，且不把领之前的读数当复查念。
//
// 写成"失败"的代价是可观察的：用户以为没领上、再点一次，而 `reward` 幂等与否**零样本**。
// 反向改法是把 `return ..., perr` 改成 `return ..., nil` —— 那会少一条 `errored` 事件，
// 历史里这一趟看着像"顺利领完"。
func TestNoticeRunReportsClaimedWhenTheRecheckFails(t *testing.T) {
	run, script := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{
			noticeInfoCaptured, `{"code":1,"msg":"系统繁忙，请稍后重试"}`,
		}},
		liveTaskRoute{noticeRewardEndpoint, []string{noticeOK}},
	)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("复查失败必须交回错误（否则这一趟被记成顺利跑完）")
	}
	if got := script.count(noticeRewardEndpoint); got != 1 {
		t.Fatalf("reward 该已经发出去了，实得 %d 次", got)
	}
	if res.Status != NoticeDone {
		t.Errorf("领成功这件事不因复查挂了就消失，实得 %v（%s）", res.Status, res.Error)
	}
	if !res.RecheckFailed {
		t.Error("要能指出「是复查没跑成」，否则两个「没读数」混成一回事")
	}
	if res.TaskOne != nil || res.TaskTwo != nil {
		t.Errorf("复查没跑成时不许回填领之前的读数，实得 %+v / %+v", res.TaskOne, res.TaskTwo)
	}
	if !strings.Contains(res.SummaryLine(), "复查没跑成") {
		t.Errorf("小结要说清是复查没跑成，实得 %q", res.SummaryLine())
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Error())
	}
}

// TestNoticeRunFoldsRewardFailureIntoOutcome：`reward` 发了但失败 ⇒
// **不**交回错误（请求发过是实况），也不复查（那一次读数没有意义）。
func TestNoticeRunFoldsRewardFailureIntoOutcome(t *testing.T) {
	run, script := noticeRuns(&poolSignInStore{*liveTaskStore()},
		liveTaskRoute{noticeInfoEndpoint, []string{noticeInfoCaptured}},
		liveTaskRoute{noticeRewardEndpoint, []string{`{"code":1,"msg":"领取失败"}`}},
	)
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("发过、失败，这一趟的实况该留在结论里而不是变成一句错误：%v", perr)
	}
	if res.Status != NoticeFailed {
		t.Errorf("实得 %v，期望 Failed（理由 %q）", res.Status, res.Error)
	}
	if got := script.count(noticeInfoEndpoint); got != 1 {
		t.Errorf("reward 没成就没有东西可复查，getTaskInfo 实得 %d 次", got)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeFailed {
		t.Errorf("发了写请求又失败 ⇒ 整趟失败，实得 %v", report.Outcome)
	}
	if !strings.Contains(res.SummaryLine(), "领取失败") {
		t.Errorf("小结该带上游那句话，实得 %q", res.SummaryLine())
	}
}

// TestNoticeRunReturnsTheErrorWhenTheGateQueryFails：查判据失败 ⇒ 交回错误，
// 结论是"这一趟没办成"，且**一条写请求都不发**。
func TestNoticeRunReturnsTheErrorWhenTheGateQueryFails(t *testing.T) {
	run, script := noticeRuns(&poolSignInStore{*liveTaskStore()}, liveTaskRoute{
		noticeInfoEndpoint, []string{`{"code":1,"msg":"系统繁忙，请稍后重试"}`},
	})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("查判据失败必须交回错误")
	}
	if res.Status != NoticeFailed {
		t.Errorf("结论也该是失败（这一趟什么都没办成），实得 %v", res.Status)
	}
	if got := script.count(noticeRewardEndpoint); got != 0 {
		t.Errorf("判据没读到就不该发写请求，实得 %d 次", got)
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Error())
	}
}

// TestNoticeRunWithNoCredentialDoesNotGoOut：库里没凭据 ⇒ 一次出网都不该有。
func TestNoticeRunWithNoCredentialDoesNotGoOut(t *testing.T) {
	run, script := noticeRuns(&poolSignInStore{fakeStore{state: nil}})
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("库里没有凭据应该报错")
	}
	if len(script.snapshot()) != 0 {
		t.Errorf("取不到凭据就不该出网，实得 %d 条请求", len(script.snapshot()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 档位 → 四态
// ─────────────────────────────────────────────────────────────────────────────

// TestNoticeResultMapsStatusToOutcome：纯函数用例（不出网）。
//
// 漂移就发生在这一小张表上：加一档、或把 Manual 挪进 default 分支，
// 都不会让别的地方变红 —— 而"未开启通知"一旦不报待办，就又回到旧项目那句 info 的老路。
func TestNoticeResultMapsStatusToOutcome(t *testing.T) {
	cases := []struct {
		name    string
		res     NoticeResult
		want    core.RunOutcome
		steps   int
		summary string
	}{
		{
			name:    "没得领也算成功",
			res:     NoticeResult{Status: NoticeNothing, Reason: "能判的槽里没有一个读成可领"},
			want:    core.OutcomeSuccess,
			summary: "公告任务没领：能判的槽里没有一个读成可领",
		},
		{
			name:    "判不了也算成功（本次没发请求）",
			res:     NoticeResult{Status: NoticeUnknown, Reason: "上游没给判据字段"},
			want:    core.OutcomeSuccess,
			summary: "公告任务没领：上游没给判据字段",
		},
		{
			name: "要人去开通知",
			res: NoticeResult{Account: core.NewAccountID(liveTestAccountID),
				Status: NoticeManual, Reason: "有槽读成通知未开启"},
			want:    core.OutcomeManualRequired,
			steps:   1,
			summary: "公告任务没领：有槽读成通知未开启，要去 App 里开启邮箱/短信通知",
		},
		{
			name:    "发了请求但失败",
			res:     NoticeResult{Status: NoticeFailed, Error: "上游回 1"},
			want:    core.OutcomeFailed,
			summary: "公告任务失败：上游回 1",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.res.RunReport()
			if report.Outcome != c.want {
				t.Errorf("实得 %v，期望 %v", report.Outcome, c.want)
			}
			if len(report.ManualSteps) != c.steps {
				t.Errorf("待办条数应 %d，实得 %d", c.steps, len(report.ManualSteps))
			}
			summary := ""
			if report.Summary != nil {
				summary = *report.Summary
			}
			if summary != c.summary {
				t.Errorf("小结应逐字为 %q，实得 %q", c.summary, summary)
			}
			if c.want == core.OutcomeManualRequired {
				step := report.ManualSteps[0]
				if step.Target != core.TargetMobileApp {
					t.Errorf("开通知只能在 App 里做，target 实得 %v", step.Target)
				}
				if step.Capability != "notice" {
					t.Errorf("待办按能力去重，code 实得 %q", step.Capability)
				}
				if step.Account == nil || step.Account.String() == "" {
					t.Error("多账号跑时不指出是哪个号，这条待办没法兑现")
				}
				if !strings.Contains(step.Instruction, "去移动云盘 App") {
					t.Errorf("待办要说清去哪儿做什么，实得 %q", step.Instruction)
				}
			}
			// 空 slices 而不是 nil：Rust 侧序列化 `[]`，前端 `.map` 不会炸。
			if report.ManualSteps == nil {
				t.Error("ManualSteps 应为空 slice 而不是 nil")
			}
		})
	}
}

// TestNoticeManualStepCarriesThisAccountsID：待办里那个账号是**本次的**。
//
// ⚠ 挡的改法是把账号做成一份共用变量再 `&` 出去 —— 多账号串行时全部待办
// 指向最后那个号，而单账号用例照样全绿。
func TestNoticeManualStepCarriesThisAccountsID(t *testing.T) {
	for _, id := range []string{"19****01", "19****02"} {
		res := NoticeResult{Account: core.NewAccountID(id),
			Status: NoticeManual, Reason: "有槽读成通知未开启"}
		step := res.RunReport().ManualSteps[0]
		if step.Account == nil || step.Account.String() != id {
			t.Errorf("待办应指向 %s，实得 %v", id, step.Account)
		}
	}
}

// TestNoticeSlotLabelKeepsAbsentDifferentFromZero：小结里的读数把"没说"念成"缺"。
//
// "槽二 progress 缺"与"progress=0"是两句话 —— 后者会让"要不要再等一天"这个判断
// 建立在一个上游没说过的零上。
func TestNoticeSlotLabelKeepsAbsentDifferentFromZero(t *testing.T) {
	full := noticeSlotLabel(&upstream.NoticeTaskSlot{
		CloudStatus: noticePtr(1), TaskStatus: noticePtr(3),
		Sum: noticePtr(100), Progress: noticePtr(31),
	})
	if !strings.Contains(full, "progress=31") || strings.Contains(full, "缺") {
		t.Errorf("四个键都给全时不该出现\"缺\"，实得 %q", full)
	}
	sparse := noticeSlotLabel(&upstream.NoticeTaskSlot{CloudStatus: noticePtr(2)})
	if !strings.Contains(sparse, "taskStatus=缺") || !strings.Contains(sparse, "progress=缺") {
		t.Errorf("缺的键要念成\"缺\"，实得 %q", sparse)
	}
	if strings.Contains(sparse, "progress=0") {
		t.Errorf("\"没说\"不许念成\"说了 0\"，实得 %q", sparse)
	}
	if got := noticeSlotLabel(nil); got != "无槽" {
		t.Errorf("整槽没给要念成\"无槽\"，实得 %q", got)
	}
}
