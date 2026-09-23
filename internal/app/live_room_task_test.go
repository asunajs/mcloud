package app

// 「跑一个账号」这一层的测试。
//
// # 为什么判定与流程要分开测
//
// [live_room_test.go] 钉的是**纯函数**（六步链走到哪、什么时候该重听），
// [live_room_drive_test.go] 钉的是**驱动**（第几步发哪个请求）。
// 这一层的价值恰好在那两者**之间**：把判定结果接回流程，于是只有这里能回答
//
//   - "手工给了口令，还会不会去拨 WebSocket？"（多花几十秒墙钟是小事，
//     界面上凭空多出一条根本没发生的"在播房间 X"是大事）
//   - "重听那一次的 exclude 里到底有没有第一轮**全部**口令？"
//   - "两轮结果合并之后，起始余额是谁的？无效口令是谁的？"
//
// ⚠⚠ 后两条是本文件最值钱的断言。它们错的时候**不会**有任何一条已有测试变红：
// exclude 漏了成功的口令 ⇒ 第二轮把它再投一次 ⇒ **同一账号**的第二次换回 `200112`，
// 日志读起来像"这批口令坏了"；余额取错那一轮 ⇒ "进账"少算一轮，
// 而那个数字看着完全正常。这两处恰恰是"CLI 与界面各写一遍流程"会各自漂移的地方
// —— 现在只有一份实现，所以漂移只能在这里被钉住。

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 样本与夹具
// ─────────────────────────────────────────────────────────────────────────────

const (
	// 门**开着**：那主播正开着一间。形状逐字照 [liveTab127Body]，只把 status 改成 1。
	liveTaskTabOneLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":1,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	// 该主播**同时开两间**时，第二间在 129 页签里（127 给第一间、129 给第二间）。
	liveTaskTabSecondLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147608,"title":"国庆出游AI搞定攻略！","status":1,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	// 有在播、但**不是那个主播**。
	liveTaskTabOtherLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,` +
		`"channelId":29,"anchor":{"nickname":"福建移动"}}}]}}`

	// 一间都没在播（连别家也没有）。
	liveTaskTabNobodyLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":0,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	liveTaskSSOBody = `{"code":0,"msg":"success","result":{"token":"199-sso-token"}}`

	// liveTaskH5TimestampBody 是云盘侧建会话第二枪（服务端毫秒时间戳，签名要用它）。
	liveTaskH5TimestampBody = `{"code":0,"msg":"success","result":1789800128813}`

	// liveTestH5JWT 是 `tyrzLogin` 回的那一份，与直播侧的 [liveTestJWT] **必须不同值** ——
	// 六步链到底带的是哪一份，只有两个值不一样时才断得出来（断言在
	// [TestLiveRoomSixStepsUseCloudSessionJWT]）。
	liveTestH5JWT = "test-h5-jwt-not-a-real-credential"
)

func liveTaskJWTBody() string {
	return `{"rtnMsg":null,"rtnCode":"0","object":"` + liveTestJWT + `"}`
}

func liveTaskH5LoginBody() string {
	return `{"code":0,"msg":"success","result":{"token":"` + liveTestH5JWT + `"}}`
}

// liveTaskH5SessionBodies 是**云盘侧**建会话那三枪的脚本（换 SSO token → 服务端时间戳 → `tyrzLogin`）。
//
// 顺序就是 [OpenSignInSession] 的三步。六步链与探针的余额都要它 ——
// 少这一段，夹具里的表现是 `scriptedUpstream` 越界报"脚本用完了"，
// 而不是"少了一个请求"，看着像夹具坏了。
func liveTaskH5SessionBodies() []string {
	return []string{liveTaskSSOBody, liveTaskH5TimestampBody, liveTaskH5LoginBody()}
}

// liveTaskGateBodies 拼出门那一段的脚本（两个页签回同一份，合并后去重）。
func liveTaskGateBodies(tab string) []string {
	return []string{liveTaskSSOBody, liveTaskJWTBody(), tab, tab}
}

// liveTaskXHSBody 造小红书主页那一源。
//
// 写成 `口令N：内容` 同行多条，是因为 [upstream.ExtractKouling] 的 ② 会在一行里
// 按口令头切多条；而"一次解出两个口令"正是实测的常态（每场只有两条）。
func liveTaskXHSBody(codes ...string) string {
	var parts []string
	for i, code := range codes {
		parts = append(parts, fmt.Sprintf("口令%d：%s", i+1, code))
	}
	return `<html><body><div>` + strings.Join(parts, " ") + `</div></body></html>`
}

// liveTaskStore 是一个只带整串凭据的库（手机号从串里解出来，不需要 phone 列）。
func liveTaskStore() *fakeStore {
	auth := core.SecretValue(wholeAuth)
	return &fakeStore{state: &core.AccountState{Authorization: &auth}}
}

// liveTaskRun 把这一层要注入的世界一次装配好（假拨号器默认"连不上"，
// 因为弹幕那一支的走法在 drive 层已经钉过了）。
func liveTaskRun(script core.HTTPClient, params LiveRoomParams, progress RunProgress) LiveRoomRun {
	return liveTaskRunWithDialer(script, &fakeDialer{err: errDialRefused}, params, progress)
}

func liveTaskRunWithDialer(
	script core.HTTPClient, dialer core.WebSocketDialer, params LiveRoomParams, progress RunProgress,
) LiveRoomRun {
	return LiveRoomRun{
		Store: liveTaskStore(),
		Scope: core.ScopeLocal(),
		Driver: LiveRoomDriver{
			Upstream: upstream.NewUpstream(script),
			Dialer:   dialer,
			Random:   &liveDriverRng{},
			Clock:    liveDriverClock(),
		},
		Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
		Params:   params,
		Progress: progress,
	}
}

const liveTestAccountID = "19****01"

// liveTaskProgress 收过程话，供断言"说了什么、什么级别、按什么顺序"。
type liveTaskProgress struct {
	levels   []core.LogLevel
	messages []string
}

func (p *liveTaskProgress) handle() RunProgress {
	return func(level core.LogLevel, message string) {
		p.levels = append(p.levels, level)
		p.messages = append(p.messages, message)
	}
}

func (p *liveTaskProgress) joined() string { return strings.Join(p.messages, "\n") }

func (p *liveTaskProgress) has(level core.LogLevel, substr string) bool {
	for i, message := range p.messages {
		if p.levels[i] == level && strings.Contains(message, substr) {
			return true
		}
	}
	return false
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试替身：按端点分派的**队列**
// ─────────────────────────────────────────────────────────────────────────────

// liveTaskScript 与 [scriptedUpstream] 的区别只有一个，但它是必需的：
// [CollectKouling] 会**同时**跑两个 goroutine（弹幕源与小红书源），全局请求顺序
// 不确定 ⇒ "按第 n 个请求回什么"这种脚本没法写。这里改成"每个端点自己数第 n 次"，
// 于是并发之下每一条响应仍然是确定的。
//
// ⚠ 队列用尽后**重复最后一条**（而不是报错）：六步链里 `user/info` 要打两次、
// 埋点要打五次，而"多打了一次"由各条断言自己抓（计数、draw 序列）。
type liveTaskScript struct {
	mu       sync.Mutex
	routes   []liveTaskRoute
	popped   map[string]int
	requests []core.HTTPRequest
}

type liveTaskRoute struct {
	match  string
	bodies []string
}

func liveTaskScriptWith(routes ...liveTaskRoute) *liveTaskScript {
	return &liveTaskScript{routes: routes, popped: map[string]int{}}
}

func (c *liveTaskScript) Send(
	_ context.Context, request core.HTTPRequest,
) (core.HTTPResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.requests = append(c.requests, request)
	for i, route := range c.routes {
		if !strings.Contains(request.URL, route.match) || len(route.bodies) == 0 {
			continue
		}
		key := strconv.Itoa(i)
		at := c.popped[key]
		c.popped[key] = at + 1
		if at >= len(route.bodies) {
			at = len(route.bodies) - 1
		}
		return core.NewResponse(200, []byte(route.bodies[at])), nil
	}
	// 没路由 = 这个测试没料到会打这个端点 ⇒ 给出明确的失败形状，
	// 不悄悄回空体（那会表现成"解析失败"，把排查方向带去解析层）。
	return core.NewResponse(404, []byte("no route for "+request.URL)), nil
}

func (c *liveTaskScript) snapshot() []core.HTTPRequest {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]core.HTTPRequest{}, c.requests...)
}

// count 数某个端点被打了几次 —— 并发下唯一可靠的断言单位。
func (c *liveTaskScript) count(match string) int {
	n := 0
	for _, request := range c.snapshot() {
		if strings.Contains(request.URL, match) {
			n++
		}
	}
	return n
}

// requestBodies 把所有请求体拼成一块，用来查"哪个房间号真的被发出去了"
// （`roomBeat` 的 roomId 在**体**里，不在 URL 里）。
func (c *liveTaskScript) requestBodies() string {
	var out strings.Builder
	for _, request := range c.snapshot() {
		out.Write(request.Body)
		out.WriteByte('\n')
	}
	return out.String()
}

// liveTaskFixture 是"门开着 + 两个源各按队列给东西"的一整套响应。
type liveTaskFixture struct {
	// tabs 是页签队列（默认一份就够：两个页签回同一间，合并时去重）。
	tabs []string
	// xhs 是小红书那一源的队列（第 n 次收集回第 n 条）。
	xhs []string
	// balances / draws 是六步链里"同一端点每次回不同东西"的两条队列。
	balances []string
	draws    []string
}

func liveTaskRoutes(f liveTaskFixture) []liveTaskRoute {
	tabs := f.tabs
	if len(tabs) == 0 {
		tabs = []string{liveTaskTabOneLive}
	}
	return []liveTaskRoute{
		{"/user/querySpecTokenV2", []string{liveTaskSSOBody}},
		{"/cmlive/api/user/auth/cmpassLoginByCmToken", []string{liveTaskJWTBody()}},
		// 云盘侧会话的另两枪（`tyrzLogin` 那一份 JWT 才是六步链与余额用的，
		// 补记见 [ProbeLiveRoom]）。SSO 那一枪与门共用上面那条队列。
		{"/ycloud/api/timestamp", []string{liveTaskH5TimestampBody}},
		{"/ycloud/auth-service/auth/tyrzLogin", []string{liveTaskH5LoginBody()}},
		{"/channel/channel/tabContentList", tabs},
		// 弹幕那一支的三步 HTTP（它连不上 WS 之前必须先把这几步走完，
		// 否则"只听第一间"那条断言就没有 roomBeat 可查）。
		{"/cmlive/api/user/auth/getCurrentUserInfoNew", []string{liveCurrentUserBody}},
		{"/cmlive/api/user/customer/userToken", []string{liveUserTokenBody()}},
		{"/cmlive/api/room/front/roomBeat", []string{liveRoomBeatLive}},
		{"/v3/room/member/apply", []string{liveApplyBody}},
		{"xiaohongshu.com", f.xhs},
		{"/liveRoomFeedback/user/info", f.balances},
		{"/visitlog/journaling", []string{liveJournalingBody}},
		{"/firstParticipate/give", []string{liveGiveNoQualificationBody}},
		{"/redFlower/draw", f.draws},
		{"/redFlower/queryRevivalFlowers", []string{liveRevivalBody}},
	}
}

// liveTaskSummary 把小结取出来供断言（`%+v` 打指针，失败时看不出差别在哪）。
func liveTaskSummary(report core.RunReport) string {
	if report.Summary == nil {
		return "<nil>"
	}
	return *report.Summary
}

func liveTaskDrawCodes(requests []core.HTTPRequest) []string {
	var out []string
	for _, request := range requests {
		if !strings.Contains(request.URL, "/redFlower/draw") {
			continue
		}
		_, query, _ := strings.Cut(request.URL, "?")
		for _, field := range strings.Split(query, "&") {
			if value, ok := strings.CutPrefix(field, "code="); ok {
				out = append(out, value)
			}
		}
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// ① 手工给了口令 ⇒ 一次都不该去收集
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomRunAccountWithManualCodesNeverCollects 钉住"给了口令就不听弹幕"。
//
// 反过来的写法（先听再投）在这里没有任何报错，只是白等 `listen` 秒，
// 而界面上多出一条**根本没发生**的"在播房间 X，听 60 秒弹幕…"。
// 更糟的是它顺手打了一次外站（小红书），那是用户没要求的一次出站请求。
func TestLiveRoomRunAccountWithManualCodesNeverCollects(t *testing.T) {
	client := &scriptedUpstream{responses: append(
		append(liveTaskGateBodies(liveTaskTabOneLive), liveTaskH5SessionBodies()...),
		liveUserInfoBody(235),
		liveJournalingBody, liveJournalingBody, liveJournalingBody,
		liveJournalingBody, liveJournalingBody,
		liveGiveNoQualificationBody,
		liveDrawOKBody(3),
		liveDrawOKBody(2),
		liveRevivalBody,
		liveUserInfoBody(240),
	)}
	dialer := &fakeDialer{conn: &fakeWSConn{}}
	progress := &liveTaskProgress{}

	run := liveTaskRunWithDialer(client, dialer, LiveRoomParams{
		Codes: []string{"云盘AI高效复盘", "国庆红包口令"},
		// ⚠ 故意给一个大秒数：这一趟**不该用到它**。
		ListenSeconds: 3600,
	}, progress.handle())

	result, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("整趟不该失败：%s", perr.Message())
	}
	if !result.Gate.Active || result.Gate.LiveCount != 1 {
		t.Fatalf("门应开着（1 间在播），实得 %+v", result.Gate)
	}
	if !result.HasJWT {
		t.Fatal("应记着「换到了直播侧凭据」")
	}
	if !liveStringsEqual(result.Codes, []string{"云盘AI高效复盘", "国庆红包口令"}) {
		t.Fatalf("投出去的口令应与给定的相同，实得 %v", result.Codes)
	}
	if result.Report == nil || result.Report.Gained() != 5 {
		t.Fatalf("应有报告且进账 5，实得 %+v", result.Report)
	}

	// ⭐ 一条 WS 都不拨，也不打小红书 / roomBeat / WS 凭据。
	if len(dialer.urls) != 0 {
		t.Errorf("给了手工口令还去拨 WS：%v", dialer.urls)
	}
	for _, request := range client.requests {
		for _, forbidden := range []string{
			"xiaohongshu.com", "/room/front/roomBeat", "/user/customer/userToken",
		} {
			if strings.Contains(request.URL, forbidden) {
				t.Errorf("手工口令那一趟不该打 %s（整个请求序列：%v）",
					forbidden, liveStepTrace(t, client))
			}
		}
	}
	liveAssertTrace(t, client, []string{
		"query_spec_token", "cm_pass_login", "tab_content", "tab_content",
		// 投之前先建云盘会话（第二枪的 SSO token 与门那一枪**是同一个端点、不同 toSourceId**，
		// 所以在步骤名上长得一样 —— 这一点在 [TestLiveRoomRunAccountSixStepsUseCloudJWT] 里按头断）。
		"query_spec_token", "h5_timestamp", "tyrz_login",
		"user_info",
		"journaling", "journaling", "journaling", "journaling", "journaling",
		"first_participate",
		"draw", "draw",
		"revival_flowers",
		"user_info",
	})

	// 过程话的**顺序与内容**：界面上那一列日志就是它。
	if len(progress.messages) != 2 ||
		progress.messages[0] != "开门中…" ||
		progress.messages[1] != "投 2 个口令…" {
		t.Fatalf("过程话不符，实得 %q", progress.joined())
	}
	for _, level := range progress.levels {
		if level != core.LevelInfo {
			t.Errorf("这一趟没有任何取舍要告知，不该出现非 info 级别：%q", progress.joined())
		}
	}
}

// TestLiveRoomRunAccountSixStepsUseCloudJWT 钉住"六步链带的是哪一份 JWT"。
//
// 2026-09-22 实测：拿门换到的**直播侧** JWT 去投，`user/info` 直接回 `90001 未登录`
// —— 这一族六个请求全打在 `m.mcloud.139.com/ycloud`（与签到同域），要的是
// `tyrzLogin` 那一份。写法上"顺手把 opened.JWT 传下去"编译得过、请求也发得出去，
// 只是每一枪都是未登录，所以只能在**头**上断（判据全文见 [ProbeLiveRoom] 的补记）。
//
// 反向改坏它：把 `NewRedFlowerSession(h5JWT, …)` 换回 `opened.JWT` ⇒ 本条与
// [TestProbeLiveRoomReadsBalanceWithTheCloudSessionJWT] 一起变红。
func TestLiveRoomRunAccountSixStepsUseCloudJWT(t *testing.T) {
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235), liveUserInfoBody(240)},
		draws:    []string{liveDrawOKBody(5)},
	})...)
	run := liveTaskRun(script, LiveRoomParams{Codes: []string{"口令甲"}}, nil)

	result, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("整趟不该失败：%s", perr.Message())
	}
	if result.Report == nil || result.Report.Gained() != 5 {
		t.Fatalf("六步链应跑完并进账 5，实得 %+v", result.Report)
	}

	for _, match := range []string{
		"/liveRoomFeedback/user/info", "/visitlog/journaling",
		"/liveRoomFeedback/firstParticipate/give", "/redFlower/draw",
		"/redFlower/queryRevivalFlowers",
	} {
		if got := jwtHeaderOn(t, script, match, "jwttoken"); got != liveTestH5JWT {
			t.Errorf("%s 该带云盘侧 JWT，实得 %q（该是 %q）", match, got, liveTestH5JWT)
		}
	}
	// 直播侧那一族**不受影响**：页签仍然带直播侧 JWT，而且头名也不同。
	if got := jwtHeaderOn(t, script, "/channel/channel/tabContentList", "Authorization"); got != liveTestJWT {
		t.Errorf("页签该带直播侧 JWT，实得 %q（该是 %q）", got, liveTestJWT)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 门的两种"没有"必须分开说
// ─────────────────────────────────────────────────────────────────────────────

func TestLiveRoomRunAccountGateClosedSaysWhichKindOfNothing(t *testing.T) {
	for _, tc := range []struct {
		name      string
		tab       string
		wantWarn  string
		wantIdle  string
		liveCount int
	}{
		{
			name:      "一间都没在播",
			tab:       liveTaskTabNobodyLive,
			wantWarn:  "今天没有直播（" + upstream.KoulingAnchor + " 一间都没在播）",
			wantIdle:  "今天没有直播，没有口令可收",
			liveCount: 0,
		},
		{
			// ⭐ 这两条**下一步完全不同**：前者是"等开播"，后者是"这一场没有口令环节"。
			// 混成一句的话用户会去查收集那一步，而那一步根本没被走到。
			name:      "有在播但不是那个主播",
			tab:       liveTaskTabOtherLive,
			wantWarn:  "有 1 间在播，但没有一间是「" + upstream.KoulingAnchor + "」",
			wantIdle:  "有 1 间在播，但没有一间是「" + upstream.KoulingAnchor + "」",
			liveCount: 1,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client := &scriptedUpstream{responses: liveTaskGateBodies(tc.tab)}
			progress := &liveTaskProgress{}
			run := liveTaskRunWithDialer(client, &fakeDialer{conn: &fakeWSConn{}},
				LiveRoomParams{ListenSeconds: 60}, progress.handle())

			result, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
			if perr != nil {
				t.Fatalf("门没开不是错误，实得 %s", perr.Message())
			}
			if result.Gate.Active || result.Gate.LiveCount != tc.liveCount {
				t.Fatalf("门判定不符：%+v", result.Gate)
			}
			if !progress.has(core.LevelWarn, tc.wantWarn) {
				t.Errorf("该说出「%s」，实得 %q", tc.wantWarn, progress.joined())
			}
			if result.Report != nil {
				t.Fatal("门没开不该有六步链报告")
			}
			if report := result.RunReport(); report.Outcome != core.OutcomeSuccess ||
				liveTaskSummary(report) != tc.wantIdle {
				t.Fatalf("没投是**正常结果**（等开播而已），但两种「没有」要分开说，实得 %v / %q",
					report.Outcome, liveTaskSummary(report))
			}
			// 门没开 ⇒ 收集与投一次都不该发生。
			liveAssertTrace(t, client, []string{
				"query_spec_token", "cm_pass_login", "tab_content", "tab_content",
			})
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 收集 → 投 → 重听 → 合并（这一层独有的三处"少写一句就静默出错"）
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomRunAccountRetriesWithFullExcludeAndMergesRounds 是本文件的主断言。
//
// 场景：小红书先给出甲、乙两条，甲被判 `200108`（口令不对）⇒ 重听一次；
// 第二次那一页上**四条都在**（笔记是累加的），于是 exclude 里有没有第一轮
// **全部**口令（含成功那条乙）就成了"会不会白投两次"的分水岭。
func TestLiveRoomRunAccountRetriesWithFullExcludeAndMergesRounds(t *testing.T) {
	甲, 乙, 丙, 丁 := "口令甲", "口令乙", "口令丙", "口令丁"
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		// 两次收集各给一版页面（第二次那版**包含**第一次那两条）。
		xhs: []string{liveTaskXHSBody(甲, 乙), liveTaskXHSBody(甲, 乙, 丙, 丁)},
		// ①⑥ 各两次：起始 235 → 第一轮后 243 → 第二轮后 251。
		balances: []string{liveUserInfoBody(235), liveUserInfoBody(243),
			liveUserInfoBody(243), liveUserInfoBody(251)},
		// 甲无效、乙成 5；丙成 3、丁无效。
		draws: []string{liveDrawInvalidBody, liveDrawOKBody(5), liveDrawOKBody(3), liveDrawInvalidBody},
	})...)
	progress := &liveTaskProgress{}
	run := liveTaskRun(script, LiveRoomParams{ListenSeconds: 30}, progress.handle())

	result, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("整趟不该失败：%s", perr.Message())
	}
	if !result.Retried {
		t.Fatal("有无效口令时应触发重听")
	}
	report := result.Report
	if report == nil {
		t.Fatal("应有合并后的报告")
	}

	// ⭐ 每个口令**只投一次**，顺序是"第一轮全部 → 第二轮新增"。
	// 这一条就是 exclude 的断言：漏了乙的话它会变成 [甲,乙,乙,丙,丁]。
	if got := liveTaskDrawCodes(script.snapshot()); !liveStringsEqual(got, []string{
		upstream.PercentEncode(甲), upstream.PercentEncode(乙),
		upstream.PercentEncode(丙), upstream.PercentEncode(丁),
	}) {
		t.Fatalf("draw 序列不符（exclude 含不含成功的那条，看这里）：%v", got)
	}
	if len(report.Draws) != 4 || report.Succeeded() != 2 {
		t.Fatalf("应 4 投 2 成，实得 %d 投 %d 成", len(report.Draws), report.Succeeded())
	}
	// ⭐ 起始取第一轮、终值取第二轮。
	if report.BalanceBefore.FlowerCnt != 235 || report.BalanceAfter.FlowerCnt != 251 {
		t.Fatalf("余额两端取错：%d → %d",
			report.BalanceBefore.FlowerCnt, report.BalanceAfter.FlowerCnt)
	}
	if report.Gained() != 16 {
		t.Errorf("进账应是两轮之差 16，实得 %d", report.Gained())
	}
	// ⭐ 无效口令是**两轮并集**（第二轮那份里没有甲，只因为它被 exclude 排除了）。
	if !liveStringsEqual(report.InvalidCodes, []string{甲, 丁}) {
		t.Fatalf("无效口令应含第一轮那条，实得 %v", report.InvalidCodes)
	}
	if !liveStringsEqual(result.Codes, []string{甲, 乙, 丙, 丁}) {
		t.Fatalf("结果里的口令应是两轮之和，实得 %v", result.Codes)
	}
	// 两次收集：WS 那一源被试过两次（它坏了不影响这一趟，见路由处的说明）。
	if got := script.count("xiaohongshu.com"); got != 2 {
		t.Errorf("应收集两次，小红书实被打 %d 次", got)
	}
	if !progress.has(core.LevelWarn, "有 1 个口令无效，重听 30 秒…") {
		t.Errorf("该告知重听，实得 %q", progress.joined())
	}
	if !progress.has(core.LevelInfo, "又收到 2 个新口令，再投一轮…") {
		t.Errorf("该告知第二轮，实得 %q", progress.joined())
	}

	// 两投两成 ⇒ 部分成功（不是失败：确实进账了；也不是成功：有一半没成）。
	if got := result.RunReport(); got.Outcome != core.OutcomePartialSuccess {
		t.Fatalf("一半口令没成应是部分成功，实得 %v", got.Outcome)
	}
}

// TestLiveRoomRunAccountListenOnlyDrawsNothing 钉住"只听不投"真的一个口令都不投给上游。
func TestLiveRoomRunAccountListenOnlyDrawsNothing(t *testing.T) {
	甲, 乙 := "口令甲", "口令乙"
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		xhs:      []string{liveTaskXHSBody(甲, 乙)},
		balances: []string{liveUserInfoBody(1)},
		draws:    []string{liveDrawOKBody(1)},
	})...)
	run := liveTaskRun(script, LiveRoomParams{ListenSeconds: 30, KoulingOnly: true}, nil)

	result, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("只听不投不该失败：%s", perr.Message())
	}
	if result.Report != nil {
		t.Fatal("只收不投不该有六步链报告")
	}
	if got := script.count("/redFlower/draw"); got != 0 {
		t.Fatalf("只收不投时一个口令都不该投，实得 %d 次", got)
	}
	if !liveStringsEqual(result.Codes, []string{甲, 乙}) {
		t.Fatalf("收到的口令要留在结果里，实得 %v", result.Codes)
	}
	if got := result.RunReport(); got.Outcome != core.OutcomeSuccess ||
		liveTaskSummary(got) != "收到 2 个口令，本次只收不投" {
		t.Fatalf("小结要说明是「只收不投」，实得 %v / %q", got.Outcome, liveTaskSummary(got))
	}
}

// TestLiveRoomRunAccountOnlyListensFirstRoomButSaysSo 钉住"只听第一间"这笔取舍。
//
// 该主播同时开两间时只覆盖第一间（见 [LiveRoomRun.RunAccount] 里那段长注释）。
// 这一支不是漏了循环，是**拿"漏口令"换"墙钟时间"**的刻意选择 —— 所以它**必须说出来**：
// 不说出来，用户看到的只有一句"在播房间 147606，听 60 秒"，会以为已经听全了。
// ⚠ 这条断言同时也是"哪天改成循环两间"时的报警：真去循环这里会红，
// 改测试的人才会顺着注释去核代价。
func TestLiveRoomRunAccountOnlyListensFirstRoomButSaysSo(t *testing.T) {
	script := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		// 两个页签各给一间**都在播**的正主房 ⇒ 合并后 Rooms = [147606, 147608]。
		tabs: []string{liveTaskTabOneLive, liveTaskTabSecondLive},
		xhs:  []string{liveTaskXHSBody("口令甲")},
	})...)
	progress := &liveTaskProgress{}
	run := liveTaskRun(script, LiveRoomParams{ListenSeconds: 30, KoulingOnly: true}, progress.handle())

	if _, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("这一趟不该失败：%s", perr.Message())
	}
	if !progress.has(core.LevelWarn, "该主播有 2 间在播，本次**只听第一间**（其余 1 间这一趟不覆盖）") {
		t.Fatalf("两间在播时必须告知取舍，实得 %q", progress.joined())
	}
	// `roomBeat` 的 roomId 在**请求体**里（不在 URL），所以这里查体。
	bodies := script.requestBodies()
	if !strings.Contains(bodies, "147606") {
		t.Error("第一间没被心跳过（收集那一步该用它）")
	}
	if strings.Contains(bodies, "147608") {
		t.Error("第二间也被心跳了 —— 与「只听第一间」的取舍不符（真要改成循环，先看注释里的代价）")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 参数拆分与结论措辞（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// TestSplitKoulingSeparators 钉住分隔符集合。
//
// ⚠ 它是**边界**不是内部实现：`docs/cli-usage.md` 那句"多个用换行或逗号分隔"
// 与界面那一栏的占位文案都指向这里，所以 CLI 与 WebUI 必须共用一份。
// 全角逗号与顿号也收，是因为口令常从聊天窗口整段粘进来。
func TestSplitKoulingSeparators(t *testing.T) {
	got := SplitKouling(" 云盘AI高效复盘\n国庆红包口令，第三条、第四条，，\r\n第四条 ")
	want := []string{"云盘AI高效复盘", "国庆红包口令", "第三条", "第四条"}
	if !liveStringsEqual(got, want) {
		t.Fatalf("拆分不符\n实际: %v\n期望: %v", got, want)
	}
	// 空列表在这一层有语义："没给手工口令 ⇒ 去收集"，所以不能把分隔符当成口令。
	if len(SplitKouling("   \n , ，、")) != 0 {
		t.Error("全是分隔符时应得到空列表")
	}
}

func TestMergeRedFlowerReportsTakesFirstStartAndLastEnd(t *testing.T) {
	succeeded := func(code string) RedFlowerDrawRecord {
		return RedFlowerDrawRecord{Code: code,
			Outcome: upstream.RedFlowerDrawOutcome{Kind: upstream.RedFlowerDrawSucceeded}}
	}
	first := RedFlowerReport{
		BalanceBefore: upstream.RedFlowerBalance{FlowerCnt: 100},
		BalanceAfter:  upstream.RedFlowerBalance{FlowerCnt: 105},
		Draws:         []RedFlowerDrawRecord{succeeded("甲")},
		InvalidCodes:  []string{"乙"},
		OtherCodes:    []string{"丙"},
	}
	second := RedFlowerReport{
		BalanceBefore:  upstream.RedFlowerBalance{FlowerCnt: 105},
		BalanceAfter:   upstream.RedFlowerBalance{FlowerCnt: 110},
		Draws:          []RedFlowerDrawRecord{succeeded("丁")},
		InvalidCodes:   []string{"戊"},
		RevivalFlowers: 7,
	}
	merged := mergeRedFlowerReports(first, second)

	if merged.BalanceBefore.FlowerCnt != 100 || merged.BalanceAfter.FlowerCnt != 110 {
		t.Fatalf("起始应取第一轮、终值取第二轮，实得 %d → %d",
			merged.BalanceBefore.FlowerCnt, merged.BalanceAfter.FlowerCnt)
	}
	if merged.Gained() != 10 {
		t.Errorf("两轮加起来进账应是 10，实得 %d", merged.Gained())
	}
	if len(merged.Draws) != 2 {
		t.Fatalf("Draws 应保序拼接，实得 %d 条", len(merged.Draws))
	}
	if !liveStringsEqual(merged.InvalidCodes, []string{"乙", "戊"}) {
		t.Fatalf("无效口令应是并集，实得 %v", merged.InvalidCodes)
	}
	if !liveStringsEqual(merged.OtherCodes, []string{"丙"}) {
		t.Fatalf("未分类口令应保留第一轮那条，实得 %v", merged.OtherCodes)
	}
	if merged.RevivalFlowers != 7 {
		t.Errorf("复活花应取有值那一轮，实得 %d", merged.RevivalFlowers)
	}
	// ⚠ 合并不许改动入参：第一轮那份还要被"第二轮失败时保留第一轮"那条分支用。
	if len(first.Draws) != 1 || len(first.InvalidCodes) != 1 || first.BalanceAfter.FlowerCnt != 105 {
		t.Fatalf("合并把第一轮改掉了：%+v", first)
	}
}

func TestRedFlowerReportSummaryLinePutsNotesInTheirOwnWords(t *testing.T) {
	record := func(code string, kind upstream.RedFlowerDrawKind) RedFlowerDrawRecord {
		return RedFlowerDrawRecord{Code: code, Outcome: upstream.RedFlowerDrawOutcome{Kind: kind}}
	}
	report := RedFlowerReport{
		BalanceBefore: upstream.RedFlowerBalance{FlowerCnt: 235},
		BalanceAfter:  upstream.RedFlowerBalance{FlowerCnt: 251},
		Draws: []RedFlowerDrawRecord{
			record("甲", upstream.RedFlowerDrawSucceeded),
			record("乙", upstream.RedFlowerDrawInvalidKouling),
			record("丙", upstream.RedFlowerDrawOther),
			record("丁", upstream.RedFlowerDrawAlreadyDrawn),
		},
		InvalidCodes:     []string{"乙"},
		OtherCodes:       []string{"丙"},
		FirstParticipate: upstream.FirstParticipateOutcome{Kind: upstream.FirstParticipateGained, FlowerNum: 3},
		RevivalFlowers:   2,
	}
	// ⚠ 进账是两次读数之差（把各口令的 flowerNum 相加会漏掉首次参与那 3 朵）。
	// ⚠ 已兑换那条（丁）**不进**任何备注：它是"本账号投过这个口令"，不是口令坏了。
	want := "小红花 235 → 251（进账 16，成功 1/4）（无效口令 乙；未分类口令 丙；首次参与 +3；复活花 2）"
	if got := report.SummaryLine(); got != want {
		t.Fatalf("小结措辞不符\n实际: %s\n期望: %s", got, want)
	}

	quiet := RedFlowerReport{
		BalanceBefore: upstream.RedFlowerBalance{FlowerCnt: 10},
		BalanceAfter:  upstream.RedFlowerBalance{FlowerCnt: 15},
		Draws: []RedFlowerDrawRecord{
			record("甲", upstream.RedFlowerDrawSucceeded),
		},
	}
	// ⭐ 这一条钉住一个零值陷阱：[upstream.FirstParticipateGained] 是**零值**，
	// 所以"这一步压根没走过"的零值报告与"走了但服务端给 0 朵"同形，
	// 两者都会印成"首次参与 +0"那句废话。少了 `FlowerNum > 0` 那道判断，
	// 合并出来的报告（以及任何提前断掉的链）就会拖上它。
	if got := quiet.SummaryLine(); got != "小红花 10 → 15（进账 5，成功 1/1）" {
		t.Errorf("正常一趟不该拖括号尾巴，实得 %q", got)
	}
}

// TestLiveRoomAccountResultRunReportFourStates 钉住四态怎么给。
//
// ⚠ "没投"是**成功**而不是失败：门开着与否由服务端数据决定，与签到里
// "今天已经签过了"同一档。写成 Failed 会让历史里出现一堆失败，
// 而它们其实只是在等开播。
func TestLiveRoomAccountResultRunReportFourStates(t *testing.T) {
	record := func(code string, kind upstream.RedFlowerDrawKind) RedFlowerDrawRecord {
		return RedFlowerDrawRecord{Code: code, Outcome: upstream.RedFlowerDrawOutcome{Kind: kind}}
	}
	full := func(before, after int64, records ...RedFlowerDrawRecord) *RedFlowerReport {
		return &RedFlowerReport{
			BalanceBefore: upstream.RedFlowerBalance{FlowerCnt: before},
			BalanceAfter:  upstream.RedFlowerBalance{FlowerCnt: after},
			Draws:         records,
		}
	}

	for _, tc := range []struct {
		name        string
		result      LiveRoomAccountResult
		wantOutcome core.RunOutcome
		wantSummary string
	}{
		{
			name:        "门没开",
			result:      LiveRoomAccountResult{Gate: KoulingGate{LiveCount: 0}},
			wantOutcome: core.OutcomeSuccess,
			wantSummary: "今天没有直播，没有口令可收",
		},
		{
			name:        "开了门但没收到口令",
			result:      LiveRoomAccountResult{Gate: KoulingGate{Active: true, LiveCount: 1}},
			wantOutcome: core.OutcomeSuccess,
			wantSummary: "开了门但没收到口令",
		},
		{
			name: "全成了",
			result: LiveRoomAccountResult{
				Report: full(100, 105, record("甲", upstream.RedFlowerDrawSucceeded)),
			},
			wantOutcome: core.OutcomeSuccess,
			wantSummary: "小红花 100 → 105（进账 5，成功 1/1）",
		},
		{
			name: "成一半",
			result: LiveRoomAccountResult{
				Report: full(100, 105,
					record("甲", upstream.RedFlowerDrawSucceeded),
					record("乙", upstream.RedFlowerDrawInvalidKouling)),
			},
			wantOutcome: core.OutcomePartialSuccess,
			wantSummary: "小红花 100 → 105（进账 5，成功 1/2）",
		},
		{
			// ⭐ 投了但一个都没成是**失败**（目标没达成），不是"没跑到"。
			// 已兑换（`200112`）也算没成 —— 本账号拿不到花，但它不进"无效口令"。
			name: "一个都没成",
			result: LiveRoomAccountResult{
				Report: full(100, 100,
					record("甲", upstream.RedFlowerDrawAlreadyDrawn),
					record("乙", upstream.RedFlowerDrawInvalidKouling)),
			},
			wantOutcome: core.OutcomeFailed,
			wantSummary: "小红花 100 → 100（进账 0，成功 0/2）",
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			report := tc.result.RunReport()
			if report.Outcome != tc.wantOutcome {
				t.Fatalf("四态判错：应为 %v，实得 %v", tc.wantOutcome, report.Outcome)
			}
			if liveTaskSummary(report) != tc.wantSummary {
				t.Fatalf("小结不符\n实际: %s\n期望: %s", liveTaskSummary(report), tc.wantSummary)
			}
			// ⚠ 这条链上没有"要人去客户端点一下"的动作，所以不该出现需人工。
			if report.Outcome.NeedsHumanAction() {
				t.Error("不该要求人工步骤")
			}
			if report.ManualSteps == nil {
				t.Error("ManualSteps 要是空切片而不是 nil（JSON 里那是两种形状）")
			}
		})
	}
}

// TestLiveRoomRunListensForDoesNotPretendNonPositiveIsValid 钉住 `listenFor` 那句"不假装合法"。
//
// 非正数在这一层**不当错误抛**（用法校验的措辞是两个产物各自的），但它会把那一次
// WS 与并行的那次 GET **一起**变成立刻超时 ⇒ 收集回"两个口令来源都失败了"，
// 一条看得见却方向错的通知。所以调用方仍须先挡住非正数 ——
// 这条断言记录的是"这里没有兜住"，不是"这里兜住了"。
func TestLiveRoomRunListensForDoesNotPretendNonPositiveIsValid(t *testing.T) {
	for _, seconds := range []int{0, -5} {
		if got := (LiveRoomRun{Params: LiveRoomParams{ListenSeconds: seconds}}).listenFor(); got != 0 {
			t.Errorf("非正数秒（%d）不该变成负时长（那会让 context.WithTimeout 立刻过期），实得 %v",
				seconds, got)
		}
	}
	if got := (LiveRoomRun{Params: LiveRoomParams{ListenSeconds: 90}}).listenFor(); got != 90*time.Second {
		t.Errorf("90 秒应是 90 秒，实得 %v", got)
	}
}
