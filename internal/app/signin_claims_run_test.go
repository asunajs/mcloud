package app

// [ClaimRun] 的用例：签到翻倍 / 备份好礼 / 复活卡三条开关。
//
// 这一层只有两件事值得钉，而且都是**抄一遍就会漂移**的那类：
//
//  1. `Kind` 选对链 —— 三条链的**写请求端点各不相同**，而 `ClaimKind` 是个 int，
//     传错不报错。界面上"备份好礼"点出"翻倍"的写请求，两边都回"成功"，
//     只有账目会少一笔（判据因此在请求上而不是在返回值里）。
//  2. 四档结论映射到四态 —— 尤其"未开自动备份"必须落 [core.OutcomeManualRequired]
//     而不是成功：旧项目把它压成一句 info，于是没人知道要去 App 里点那一下。
//
// ⚠ 门槛判定本身（`PlanMultiple` / `PlanBackupGift`）在 `signin_test.go` 已经逐档钉过，
// 这里不重复；这里只测"档位之后的那段"。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 三条链各自的端点子串。写成常量是因为断言两头都要用：
// "该打的那条打了一次"与"另外两条一次都没打"。
const (
	multipleEndpoint  = "/ycloud/signin/page/multiple"
	giftInfoEndpoint  = "backupgift/info"
	giftWriteEndpoint = "backupgift/receive"
	revivalEndpoint   = "receiveRevivalReward"
)

// signInWorldFor 造一份"只到能发请求"的世界：库里那份凭据 + 按端点分派的脚本。
//
// 用 [liveTaskScript]（按端点数第 n 次）而不是顺序回放的 [scriptedUpstream]：
// 三条链**消费的申请条数不同**（翻倍先查 `infoV3`、复活卡什么都不查），
// 顺序脚本一变长就整片错位，而这里要的读数恰好是"哪个端点被打了几次"。
func signInWorldFor(store core.StateStore, client core.HTTPClient) SignInWorld {
	return SignInWorld{
		Store:    store,
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(client),
		Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
		Clock:    testClock(),
		Random:   testRng(),
	}
}

// sessionRoutes 是建会话那三枪。三条链都从它开始（共用签到页那一条会话）。
func sessionRoutes() []liveTaskRoute {
	return []liveTaskRoute{
		liveTaskRoute{"/user/querySpecTokenV2", []string{ssoOK("SSO-1")}},
		liveTaskRoute{"/ycloud/api/timestamp", []string{tsOK}},
		liveTaskRoute{"tyrzLogin", []string{jwtOK("JWT-1")}},
	}
}

func claimRuns(store core.StateStore, routes ...liveTaskRoute) (ClaimRun, *liveTaskScript) {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	return ClaimRun{
		SignInWorld: signInWorldFor(store, script),
	}, script
}

// TestClaimRunKindPicksTheChain：⭐ `Kind` → 端点。
//
// 每一档都跑**同一种夹具形状**（门槛成立 ⇒ 该发一次写请求），
// 断言只有它自己那条写请求被打了一次、另外两条一次没打。
func TestClaimRunKindPicksTheChain(t *testing.T) {
	cases := []struct {
		name   string
		kind   ClaimKind
		routes []liveTaskRoute
		wanted string // 该被打一次的端点
		forbid []string
		check  func(t *testing.T, res ClaimResult)
	}{
		{
			name: "翻倍打 multiple",
			kind: ClaimKindSignInMultiple,
			routes: []liveTaskRoute{
				{"/ycloud/signin/page/infoV3", []string{statusPayload(
					`"cal":[],"curMonthBackup":true,"curMonthBackupSignAccept":false`)}},
				{multipleEndpoint, []string{
					`{"code":0,"msg":"success","result":{"cloudCount":100,"multiple":2}}`}},
			},
			wanted: multipleEndpoint,
			forbid: []string{giftWriteEndpoint, revivalEndpoint},
			check: func(t *testing.T, res ClaimResult) {
				t.Helper()
				if res.Gained == nil || *res.Gained != 100 {
					t.Errorf("应记下领到 100，实得 %v", res.Gained)
				}
				if res.Detail != "2 倍" {
					t.Errorf("倍数与合计是两回事，Detail 应写倍数，实得 %q", res.Detail)
				}
			},
		},
		{
			name: "备份好礼打 backupgift/receive",
			kind: ClaimKindBackupGift,
			routes: []liveTaskRoute{
				{giftInfoEndpoint, []string{
					`{"code":0,"msg":"success","result":{"state":0,"curMonth":true}}`}},
				{giftWriteEndpoint, []string{`{"code":0,"msg":"success","result":88}`}},
			},
			wanted: giftWriteEndpoint,
			forbid: []string{multipleEndpoint, revivalEndpoint},
			check: func(t *testing.T, res ClaimResult) {
				t.Helper()
				if res.Gained == nil || *res.Gained != 88 {
					t.Errorf("应记下领到 88，实得 %v", res.Gained)
				}
			},
		},
		{
			name: "复活卡打 receiveRevivalReward",
			kind: ClaimKindRevival,
			routes: []liveTaskRoute{
				{revivalEndpoint, []string{`{"code":0,"msg":"success","result":` +
					`{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}`}},
			},
			wanted: revivalEndpoint,
			forbid: []string{multipleEndpoint, giftWriteEndpoint, "/ycloud/signin/page/infoV3"},
			check: func(t *testing.T, res ClaimResult) {
				t.Helper()
				// `cardType` 决定文案：`1` = 复活卡，其它 = 回馈卡（H5 也这么分流）。
				if res.Detail != "复活卡" {
					t.Errorf("Detail 应是卡型，实得 %q", res.Detail)
				}
				// ⭐ 只接 `rewardClouds`（本次进账），**不**接 `totalClouds`：
				// 后者的口径未验证（H5 文案说的是"转换成 AI豆"），接进余额会混两个口径。
				if res.Gained == nil || *res.Gained != 114 {
					t.Errorf("应记 114（本次复活到的），实得 %v", res.Gained)
				}
			},
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			// `GetSmidState` 回错误 ⇒ [DeviceID] 失败：复活卡那条本来就不换 deviceId，
			// 另外两条也不需要它（见 `signin_world.go` 头注）。
			run, script := claimRuns(&poolSignInStore{*liveTaskStore()}, c.routes...)
			run.Kind = c.kind
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("门槛成立这一趟不该报错：%v", perr)
			}
			if res.Status != ClaimDone {
				t.Fatalf("应领到，实得 %v（%s）", res.Status, res.Error)
			}
			if got := script.count(c.wanted); got != 1 {
				t.Errorf("%s 应被打 1 次，实得 %d", c.wanted, got)
			}
			for _, forbidden := range c.forbid {
				if got := script.count(forbidden); got != 0 {
					t.Errorf("%s 一次都不该打，实得 %d 次（Kind=%v 走错链了）",
						forbidden, got, c.kind)
				}
			}
			c.check(t, res)
			if res.Account.String() != liveTestAccountID {
				t.Errorf("结论里要带上是哪个账号，实得 %v", res.Account)
			}
		})
	}
}

// TestClaimRunSendsNoWriteRequestWhenTheGateIsClosed：门槛不成立 ⇒ **一条写请求都不发**。
//
// 与 `signin_test.go` 里那条同名能力层用例分开：那条测 [ClaimMultiple] 自己，
// 这一条测 `Kind` 分支接过去之后**仍然**不发 —— 中间隔了 [SignInWorld.Source]，
// 装配写错（例如把备份好礼接成了翻倍）的表现正是"发了一条本不该发的写请求"。
func TestClaimRunSendsNoWriteRequestWhenTheGateIsClosed(t *testing.T) {
	cases := []struct {
		name       string
		kind       ClaimKind
		routes     []liveTaskRoute
		wantStatus ClaimStatus
		wantWrites []string // 这些端点该被打 0 次
	}{
		{
			name: "翻倍：本月没开备份",
			kind: ClaimKindSignInMultiple,
			routes: []liveTaskRoute{
				{"/ycloud/signin/page/infoV3", []string{statusPayload(
					`"cal":[],"curMonthBackup":false,"curMonthBackupSignAccept":false`)}},
			},
			wantStatus: ClaimManual,
			wantWrites: []string{multipleEndpoint},
		},
		{
			name: "翻倍：上游没给门槛字段",
			kind: ClaimKindSignInMultiple,
			routes: []liveTaskRoute{
				{"/ycloud/signin/page/infoV3", []string{statusPayload(`"cal":[]`)}},
			},
			wantStatus: ClaimNothing,
			wantWrites: []string{multipleEndpoint},
		},
		{
			name: "好礼：本月没有这份礼",
			kind: ClaimKindBackupGift,
			routes: []liveTaskRoute{
				{giftInfoEndpoint, []string{
					`{"code":0,"msg":"success","result":{"state":0,"curMonth":false}}`}},
			},
			wantStatus: ClaimNothing,
			wantWrites: []string{giftWriteEndpoint},
		},
		{
			name: "好礼：本月已领过",
			kind: ClaimKindBackupGift,
			routes: []liveTaskRoute{
				{giftInfoEndpoint, []string{
					`{"code":0,"msg":"success","result":{"state":1,"curMonth":true}}`}},
			},
			wantStatus: ClaimNothing,
			wantWrites: []string{giftWriteEndpoint},
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run, script := claimRuns(&poolSignInStore{*liveTaskStore()}, c.routes...)
			run.Kind = c.kind
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("门槛不成立不该报错：%v", perr)
			}
			if res.Status != c.wantStatus {
				t.Errorf("实得 %v，期望 %v（理由 %q）", res.Status, c.wantStatus, res.Reason)
			}
			for _, write := range c.wantWrites {
				if got := script.count(write); got != 0 {
					t.Errorf("门槛不成立就一条写请求都不该发，%s 实得 %d 次", write, got)
				}
			}
			// 门槛不成立也是**成功**（除了 Manual 那一档）：这一趟的答案就是"现在不该领"。
			if report := res.RunReport(); report.Outcome == core.OutcomeFailed {
				t.Errorf("没发请求的分支不该报失败：%s", res.SummaryLine())
			}
		})
	}
}

// TestClaimResultMapsStatusToOutcome：四档 → 四态的映射表。
//
// 这是**纯函数**用例（不出网），因为漂移就发生在这一小张表上：
// 加了第五档、或把 Manual 挪到 default 分支里，都不会有任何别的地方变红。
func TestClaimResultMapsStatusToOutcome(t *testing.T) {
	gained := int64(50)
	cases := []struct {
		name    string
		res     ClaimResult
		want    core.RunOutcome
		steps   int
		inLine  string
		summary string
	}{
		{
			name:    "领到了",
			res:     ClaimResult{Kind: ClaimKindSignInMultiple, Status: ClaimDone, Gained: &gained},
			want:    core.OutcomeSuccess,
			inLine:  "领到 50",
			summary: "签到翻倍领到 50",
		},
		{
			name:    "上游没报数量",
			res:     ClaimResult{Kind: ClaimKindBackupGift, Status: ClaimDone},
			want:    core.OutcomeSuccess,
			summary: "备份好礼已领到（上游没报具体数量）",
		},
		{
			name:    "本月已领过",
			res:     ClaimResult{Kind: ClaimKindRevival, Status: ClaimNothing, Reason: "本月已经领过"},
			want:    core.OutcomeSuccess,
			inLine:  "没领",
			summary: "复活卡没领：本月已经领过",
		},
		{
			name: "要人去开备份",
			res: ClaimResult{Kind: ClaimKindBackupGift, Account: core.NewAccountID(liveTestAccountID),
				Status: ClaimManual, Reason: "未开启自动备份，本月这份礼没产生"},
			want:    core.OutcomeManualRequired,
			steps:   1,
			inLine:  "要去 App 开自动备份",
			summary: "备份好礼没领：未开启自动备份，本月这份礼没产生，要去 App 开自动备份",
		},
		{
			name:    "发了请求但失败",
			res:     ClaimResult{Kind: ClaimKindSignInMultiple, Status: ClaimFailed, Error: "上游回 1302"},
			want:    core.OutcomeFailed,
			inLine:  "失败：上游回 1302",
			summary: "签到翻倍失败：上游回 1302",
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
			if c.inLine != "" && !strings.Contains(summary, c.inLine) {
				t.Errorf("小结该含 %q，实得 %q", c.inLine, summary)
			}
			// ⚠ 只有 ManualRequired 这一档**不许**把 steps 留空：空了就等于
			// "报了一种要人做事、却没说做什么"，推送里是一句没有下文的话。
			if c.want == core.OutcomeManualRequired {
				step := report.ManualSteps[0]
				if step.Target != core.TargetMobileApp {
					t.Errorf("开备份这件事只能在 App 里做，target 实得 %v", step.Target)
				}
				if step.Capability != capabilities.CodeBackupGift {
					t.Errorf("待办要按能力去重，code 实得 %q", step.Capability)
				}
				if step.Account == nil || step.Account.String() == "" {
					t.Errorf("多账号跑时不指出是哪个号，这条待办就没法兑现")
				}
			}
			// 空 slices 而不是 nil：Rust 侧序列化 `[]`，前端直接 `.map` 不会炸。
			if report.ManualSteps == nil {
				t.Errorf("ManualSteps 应为空 slice 而不是 nil")
			}
		})
	}
}

// TestClaimManualStepCarriesThisAccountsID：待办里那个账号是**本次的**，不是共享的。
//
// ⚠ 挡的改法是把 [ClaimRun] 里的账号做成一份**共用变量**再 `&` 出去 ——
// 多账号串行跑时全部待办会指向最后那个号，界面上"给甲的待办"就消失了，
// 而单账号用例照样全绿（这里因此逐账号造一次）。
func TestClaimManualStepCarriesThisAccountsID(t *testing.T) {
	for _, id := range []string{"19****01", "19****02"} {
		res := ClaimResult{Kind: ClaimKindSignInMultiple,
			Account: core.NewAccountID(id), Status: ClaimManual, Reason: "没开备份"}
		step := res.RunReport().ManualSteps[0]
		if step.Account == nil || step.Account.String() != id {
			t.Errorf("待办应指向 %s，实得 %v", id, step.Account)
		}
		if !strings.Contains(step.Instruction, "自动备份") {
			t.Errorf("待办要说清去做什么，实得 %q", step.Instruction)
		}
	}
}

// TestClaimRunReturnsTheErrorWhenTheGateQueryFails：
// 读门槛失败**交回错误**，不自己折成失败结论。
//
// 两条路的差别不是修辞：走错误那条，编排层会多发一条 `errored` 事件、
// 并把原文写进 `runs.message`（`run.go` 的 `taskErr` 那一支）；
// 自己折成失败就只剩一条 finished。反向改法是把 `return ..., perr` 的 perr 换成 nil。
func TestClaimRunReturnsTheErrorWhenTheGateQueryFails(t *testing.T) {
	// 门槛查询回一个业务错误码：这一趟没有结论可报，原文必须一路传到历史里。
	run, script := claimRuns(&poolSignInStore{*liveTaskStore()}, liveTaskRoute{
		"/ycloud/signin/page/infoV3",
		[]string{`{"code":1,"msg":"系统繁忙，请稍后重试"}`},
	})
	run.Kind = ClaimKindSignInMultiple
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatalf("查门槛失败必须交回错误，实得 %+v", res)
	}
	if res.Status != ClaimFailed {
		t.Errorf("交回错误的同时结论也该是失败，实得 %v", res.Status)
	}
	if got := script.count(multipleEndpoint); got != 0 {
		t.Errorf("门槛没查到就不该发写请求，实得 %d 次", got)
	}
	// 错误串要带着上游那句话，否则排查时只剩一句"请求失败"。
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带着失败的请求，实得 %q", perr.Error())
	}
}

// TestClaimRunWithoutCredentialDoesNotGoOut：库里没凭据 ⇒ 一次出网都不该有。
//
// 这一条钉的是 [SignInWorld.Source] 的**顺序**：先取票再查库（写反了的表现是
// "为一个库里没有的账号建了一条会话"，而那三枪会带上别人的 token）。
func TestClaimRunWithoutCredentialDoesNotGoOut(t *testing.T) {
	store := &poolSignInStore{fakeStore{state: nil}}
	run, script := claimRuns(store)
	run.Kind = ClaimKindRevival
	if _, perr := run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("库里没有凭据应该报错")
	}
	if len(script.snapshot()) != 0 {
		t.Errorf("取不到凭据就不该出网，实得 %d 条请求", len(script.snapshot()))
	}
}

// TestClaimKindNamesAreDistinct：三种开关说给人听的名字与 code 各不重复。
//
// `Name()` 进小结的第一词，`CapabilityCode()` 进待办与历史分组 ——
// 两条各自撞车时的表现是"两条能力在历史里混成一条"。
func TestClaimKindNamesAreDistinct(t *testing.T) {
	kinds := []ClaimKind{ClaimKindSignInMultiple, ClaimKindBackupGift, ClaimKindRevival}
	names := map[string]bool{}
	codes := map[string]bool{}
	for _, kind := range kinds {
		if names[kind.Name()] {
			t.Errorf("名字 %q 重复了", kind.Name())
		}
		names[kind.Name()] = true
		if codes[kind.CapabilityCode()] {
			t.Errorf("code %q 重复了", kind.CapabilityCode())
		}
		codes[kind.CapabilityCode()] = true
		if _, found := capabilities.Get(kind.CapabilityCode()); !found {
			t.Errorf("%v 的 code %q 不在注册表里", kind, kind.CapabilityCode())
		}
	}
}
