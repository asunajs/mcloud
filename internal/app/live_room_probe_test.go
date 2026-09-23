package app

// 只读探针（`ProbeLiveRoom`）的用例。
//
// 这一层的全部价值在"**只读**"两个字上，所以断言分两拨：
//
//  1. 该发的发了：门那三步（换 SSO token → 换直播侧 JWT → 两个页签）
//     + 云盘侧建会话三步（换 SSO token → 服务端时间戳 → `tyrzLogin`）+ 一次余额；
//  2. **不该发的一次都没发**：`redFlower/draw`（`draw` 是**写操作** —— 投出去就以
//     这个账号的名义换了一朵）、`firstParticipate/give`、`visitlog/journaling`、
//     `userToken` 与外站小红书。
//
// ⭐ 第 2 拨才是本文件的主断言。它错的时候界面上什么都对 —— 余额显示出来了、
// 房间也列出来了，只是"看一眼"顺手**把口令投了出去**（换了一朵花、动了这个账号的
// 兑换状态），而用户点开页面本来只想要"有没有直播 / 我有几朵"。
//
// ⚠ 措辞**不要**写成"消耗掉了那两个口令"：口令是**全场通用**的，投过也不影响别人
// （五条事实见 `internal/upstream/live_flower.go` 文件头；2026-09-23 改）。

import (
	"context"
	"reflect"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

func probeProfile() *upstream.AppProfile {
	return upstream.NewAppProfile(
		upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"))
}

// probeCredential 是探针的凭据入参。`wholeAuth` 是库里 `account_state.authorization`
// 那个整串原文的合成样本（见 signin_test.go）。
func probeCredential() *SignInCredential {
	return &SignInCredential{
		authorization: wholeAuth,
		accountID:     core.NewAccountID("19****01"),
	}
}

// jwtHeaderOn 取某个端点上实际发出去的某个头（名字大小写不敏感，取第一个命中）。
//
// 单独立一个 helper 是因为这一类事实**只能这样断**：探针与六步链的返回值里都不带凭据
// （那是 [TestProbeLiveRoomCarriesNoCredential] 钉住的），所以"发出去的是哪一份 JWT"
// 只存在于请求头上。
//
// ⚠ 两个域的头名**不一样**：直播侧是 `Authorization`，云盘侧（小红花那一族）是
// 小写 `jwttoken` —— 拿同一个名字去两处找，只会得到一句"没带头"的误报。
func jwtHeaderOn(t *testing.T, client *liveTaskScript, match, header string) string {
	t.Helper()
	for _, request := range client.snapshot() {
		if !strings.Contains(request.URL, match) {
			continue
		}
		for _, pair := range request.Headers {
			// `HeaderPair` 是 `[2]string`（`pair[0]` = 名、`pair[1]` = 值），不是具名字段。
			if strings.EqualFold(pair[0], header) {
				return pair[1]
			}
		}
		t.Fatalf("%s 这一枪没带 %s 头：%+v", match, header, request.Headers)
	}
	t.Fatalf("没有请求命中 %s", match)
	return ""
}

// TestProbeLiveRoomReadsBalanceWithoutTouchingAnything 是主用例。
func TestProbeLiveRoomReadsBalanceWithoutTouchingAnything(t *testing.T) {
	client := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235)},
	})...)
	probe, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(client),
		probeProfile(), probeCredential(), testClock(), testRng())
	if perr != nil {
		t.Fatalf("门开着、余额也读到了，不该失败：%s", perr.Message())
	}
	if !probe.Gate.Active {
		t.Fatal("样本里那主播正开着一间，门必须判为开")
	}
	if probe.Balance == nil || probe.Balance.FlowerCnt != 235 {
		t.Fatalf("余额应取到 235，实得 %+v", probe.Balance)
	}
	if probe.BalanceError != "" {
		t.Fatalf("读到了就不该带失败句：%q", probe.BalanceError)
	}
	// 门是**两个页签**，不是"顺手拉一次"（漏一个页签会整场漏口令，见 QueryKoulingRooms）。
	if n := client.count("tabContentList"); n != 2 {
		t.Errorf("应打两个页签，实得 %d", n)
	}
	if n := client.count("/liveRoomFeedback/user/info"); n != 1 {
		t.Errorf("余额应恰好读一次，实得 %d", n)
	}
	// ⭐ 一条写操作都不许有。
	for _, match := range []string{"/redFlower/draw", "/firstParticipate/give",
		"/visitlog/journaling", "/redFlower/queryRevivalFlowers", "xiaohongshu.com",
		"/customer/userToken", "/room/front/roomBeat"} {
		if n := client.count(match); n != 0 {
			t.Errorf("只读探针打了 %s %d 次", match, n)
		}
	}
}

// TestProbeLiveRoomReadsBalanceWithTheCloudSessionJWT 钉住"余额用的是哪一份 JWT"。
//
// 这条不是洁癖，是 2026-09-22 的实测教训：把门换到的**直播侧** JWT 直接带给
// `liveRoomFeedback/user/info`，两个真账号都回 `90001 未登录` —— 因为那一族打在
// `m.mcloud.139.com/ycloud`（与签到同域同族），要的是 `tyrzLogin` 那一份。
// 反过来的写法编译得过、测试也照样能构造，所以只能在**头**上断。
//
// 夹具里两份 JWT 是**两个不同的值**（[liveTestJWT] / [liveTestH5JWT]），
// 断言才有读者；两边都给同一个串的话这条测试等于没写。
func TestProbeLiveRoomReadsBalanceWithTheCloudSessionJWT(t *testing.T) {
	client := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoBody(235)},
	})...)
	if _, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(client),
		probeProfile(), probeCredential(), testClock(), testRng()); perr != nil {
		t.Fatalf("探针不该失败：%s", perr.Message())
	}
	if got := jwtHeaderOn(t, client, "/liveRoomFeedback/user/info", "jwttoken"); got != liveTestH5JWT {
		t.Errorf("余额该带云盘侧 JWT，实得 %q（该是 %q）", got, liveTestH5JWT)
	}
	// 门那一侧仍然用直播侧 JWT（头名也不同）—— 换成云盘那份会表现为"门打不开"，
	// 两件事都得钉住。
	if got := jwtHeaderOn(t, client, "/channel/channel/tabContentList", "Authorization"); got != liveTestJWT {
		t.Errorf("页签该带直播侧 JWT，实得 %q（该是 %q）", got, liveTestJWT)
	}
}

// TestProbeLiveRoomStillReadsBalanceWhenGateClosed 钉住"没开播≠没有余额"：
// 界面上这一屏要同时回答"今晚有没有那场直播"和"这一号有多少朵"，
// 前者为否不该把后者一起抹掉。
//
// 样本里混了一间**别的主播**的（147490 福建移动，实拍出现过），
// 于是同一条用例顺手钉住"日程只列那个主播"。
func TestProbeLiveRoomStillReadsBalanceWhenGateClosed(t *testing.T) {
	const tabWithPreview = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147608,"title":"国庆出游AI搞定攻略！","status":0,"channelId":134,` +
		`"expectStartTime":"2026-09-23 14:30:00","anchor":{"nickname":"中国移动云盘"}}},` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,"channelId":29,` +
		`"anchor":{"nickname":"福建移动"}}}]}}`
	client := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		tabs:     []string{tabWithPreview},
		balances: []string{liveUserInfoBody(88)},
	})...)
	probe, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(client),
		probeProfile(), probeCredential(), testClock(), testRng())
	if perr != nil {
		t.Fatalf("没开播是正常结论，不该失败：%s", perr.Message())
	}
	// 那间在播的不是这个主播 ⇒ LiveCount=1 而 Active=false（两种"没有"要分得开）。
	if probe.Gate.Active || probe.Gate.LiveCount != 1 {
		t.Fatalf("门判定不符：Active=%v LiveCount=%d", probe.Gate.Active, probe.Gate.LiveCount)
	}
	if probe.Balance == nil || probe.Balance.FlowerCnt != 88 {
		t.Fatalf("门关着也要给出余额，实得 %+v", probe.Balance)
	}
	// ⭐ 预告场（`status == 0`）被门筛掉了，但**日程不能跟着丢**：
	// 界面上"下一场 2026-09-23 14:30"读的就是这一条。
	if len(probe.Rooms) != 1 || probe.Rooms[0].ID != 147608 {
		t.Fatalf("日程应只剩那个主播的一间（含预告），实得 %+v", probe.Rooms)
	}
	if probe.Rooms[0].ExpectStartTime != "2026-09-23 14:30:00" {
		t.Errorf("预告时间没带回来：%q", probe.Rooms[0].ExpectStartTime)
	}
}

// TestProbeLiveRoomKeepsGateWhenCloudSessionFails 钉住"会话建不起来"那一支：
// 它**不发** `user/info`（没有 JWT 可带，发出去必然又是一句未登录，白多一枪），
// 而门的结论照旧带回去 —— "今晚有一场"这件事跟云盘会话坏没坏无关。
//
// ⚠ `BalanceError` 那句是要显示在界面上的，所以这里顺手钉"句子里不含凭据"。
func TestProbeLiveRoomKeepsGateWhenCloudSessionFails(t *testing.T) {
	routes := liveTaskRoutes(liveTaskFixture{balances: []string{liveUserInfoBody(235)}})
	replaced := false
	for i, route := range routes {
		if route.match == "/ycloud/auth-service/auth/tyrzLogin" {
			routes[i].bodies = []string{`{"code":90001,"msg":"未登录"}`}
			replaced = true
		}
	}
	// 夹具改了名而这条测试没跟着改 ⇒ 表现会是"会话其实建成了、余额读到了"，
	// 而那正是本条要挡的那一类假通过。
	if !replaced {
		t.Fatal("夹具里没有 tyrzLogin 那条路由，本条测试无从下手")
	}
	client := liveTaskScriptWith(routes...)
	probe, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(client),
		probeProfile(), probeCredential(), testClock(), testRng())
	if perr != nil {
		t.Fatalf("云盘会话坏了不该让整条探针失败：%s", perr.Message())
	}
	if !probe.Gate.Active {
		t.Error("门不受云盘会话影响，应保持判为开")
	}
	if probe.Balance != nil {
		t.Errorf("会话没建起来时余额必须是 nil，实得 %+v", probe.Balance)
	}
	if probe.BalanceError == "" {
		t.Fatal("余额那一格必须带一句原因，不能空着（空着的界面表现是\"就是 0 朵\"）")
	}
	for _, forbidden := range []string{liveTestH5JWT, liveTestJWT, wholeAuth} {
		if strings.Contains(probe.BalanceError, forbidden) {
			t.Errorf("那句会原样显示在界面上，不该含凭据：%q", probe.BalanceError)
		}
	}
	if n := client.count("/liveRoomFeedback/user/info"); n != 0 {
		t.Errorf("会话都没建起来就不该再去读余额，实得 %d 次", n)
	}
}

// TestProbeLiveRoomKeepsGateWhenBalanceReadFails 钉住 [LiveRoomProbe.BalanceError] 那条设计：
// 余额接口抽风时，"今晚有一场"仍然要说得出来，而余额那一格得是**空 + 一句原因**，
// 不能是 0 —— 0 朵与读不到是两件事。
func TestProbeLiveRoomKeepsGateWhenBalanceReadFails(t *testing.T) {
	client := liveTaskScriptWith(liveTaskRoutes(liveTaskFixture{
		balances: []string{liveUserInfoWithoutResultBody},
	})...)
	probe, perr := ProbeLiveRoom(context.Background(), upstream.NewUpstream(client),
		probeProfile(), probeCredential(), testClock(), testRng())
	if perr != nil {
		t.Fatalf("余额读不到不该让整条探针失败：%s", perr.Message())
	}
	if !probe.Gate.Active {
		t.Error("门不受余额影响，应保持判为开")
	}
	if probe.Balance != nil {
		t.Errorf("读失败时余额必须是 nil，实得 %+v", probe.Balance)
	}
	if !strings.Contains(probe.BalanceError, "user/info") {
		t.Errorf("该说出是哪一次读数失败的，实得 %q", probe.BalanceError)
	}
}

// TestProbeLiveRoomFailsWhenGateFails 钉住"不照抄原项目的失败即放行"：
// 门都换不到凭据时，一句"今天没有直播"或"0 朵"都是**假结论**，必须整条失败。
func TestProbeLiveRoomFailsWhenGateFails(t *testing.T) {
	probe, perr := ProbeLiveRoom(context.Background(),
		upstream.NewUpstream(alwaysFailingClient{}),
		probeProfile(), probeCredential(), testClock(), testRng())
	if perr == nil {
		t.Fatal("门失败必须报错")
	}
	if probe.Balance != nil || probe.Gate.Active {
		t.Errorf("失败时不该带任何结论：%+v", probe)
	}
	// 错误要说清是**门**那一层：只回"上游 500"会让人去查余额接口。
	if !strings.Contains(perr.Message(), "门") && !strings.Contains(perr.Message(), "直播") {
		t.Logf("失败文案不含层名（可读性偏弱，不判红）：%s", perr.Message())
	}
}

// TestProbeLiveRoomCarriesNoCredential 钉住结构体本身不带着凭据出去。
//
// 探针的结论会直接进 HTTP 响应。JWT 是这条链上最不该 marshal 的一个
// （见 `scripts/check-go-jsontags.cjs` 登记的那批结构体），所以这里断的是
// **结构里没有那一位** —— 哪天有人加一个"顺手把 JWT 也带上方便前端"，这条会红。
func TestProbeLiveRoomCarriesNoCredential(t *testing.T) {
	typ := reflect.TypeOf(LiveRoomProbe{})
	for i := 0; i < typ.NumField(); i++ {
		name := strings.ToLower(typ.Field(i).Name)
		for _, forbidden := range []string{"jwt", "token", "authorization", "credential", "cookie"} {
			if strings.Contains(name, forbidden) {
				t.Errorf("LiveRoomProbe 有字段 %q：它是会出网的响应体，凭据只能留在函数内部",
					typ.Field(i).Name)
			}
		}
	}
}
