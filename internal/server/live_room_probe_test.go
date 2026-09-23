package server

// `GET /api/accounts/{id}/live-room`（只读探针）的用例。
//
// # 这一层只钉"出网形状"，不重演那条链
//
// 门怎么判、余额从哪个接口读、失败该不该降级，全在 [app.ProbeLiveRoom]
// （用例在 `internal/app/live_room_probe_test.go`）。本包重复断言那些等于抄第二份同源副本。
// 这边钉的是**只有站在 HTTP 面上才成立**的四件：
//
//  1. **一条写操作都不发**（`/redFlower/draw` 是**写操作** —— 投出去就以这个账号的名义换了一朵）；
//  2. **归属与免登录**：别人的 / 从未存在的都是 404 且零请求，匿名 401；
//  3. **响应体逐字段手写**：出网的只有拼出来的那份 map，凭据（直播侧 JWT、整串 auth）
//     一个字都不许出现在体里；
//  4. **降级形状**：余额读失败仍是 200 + `balance:null` + 一句原因，
//     而不是替用户显示"0 朵"。
//
// 号码与 token 一律合成值（199 号段，见 login_test.go 的夹具说明）。

import (
	"context"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具：按端点分派的脚本客户端
// ─────────────────────────────────────────────────────────────────────────────

// probeScript 与 [scriptClient] 的区别只有一个，但它是必需的：探针要按**端点**
// 认响应（两个页签回两份不同的房间列表），而不是按"第 n 个请求"。
// 全局顺序在并发下不可靠，而这里恰好是并发的邻居（执行面那条链会并发收口令）。
type probeScript struct {
	mu       sync.Mutex
	routes   []probeRoute
	requests []core.HTTPRequest
}

type probeRoute struct {
	match  string
	bodies []string
	popped int
}

func newProbeScript(routes ...probeRoute) *probeScript {
	return &probeScript{routes: routes}
}

var _ core.HTTPClient = (*probeScript)(nil)

func (c *probeScript) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.requests = append(c.requests, request)
	for i := range c.routes {
		route := &c.routes[i]
		if !strings.Contains(request.URL, route.match) || len(route.bodies) == 0 {
			continue
		}
		at := route.popped
		route.popped++
		if at >= len(route.bodies) {
			at = len(route.bodies) - 1
		}
		return core.NewResponse(200, []byte(route.bodies[at])), nil
	}
	// 没路由 = 这条用例没料到会打这个端点。给 404 而不是空体：空体会表现成
	// "解析失败"，把排查方向带去解析层，而真正的问题是"多打了一个接口"。
	return core.NewResponse(404, []byte("no route for "+request.URL)), nil
}

func (c *probeScript) count(match string) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	n := 0
	for _, request := range c.requests {
		if strings.Contains(request.URL, match) {
			n++
		}
	}
	return n
}

func (c *probeScript) sent() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.requests)
}

// 下面是探针会打到的那几个端点的样本响应（逐字取自实拍，凭据换成合成值）。
const (
	probeSSOBody = `{"code":0,"msg":"success","result":{"token":"199-sso-token"}}`
	// probeJWT 是直播侧那串凭据 —— 它**不许**出现在任何响应里，
	// 所以下面每一条用例都拿它查一遍体。
	probeJWT     = "SYNTH-JWT-not-a-real-credential"
	probeJWTBody = `{"rtnMsg":null,"rtnCode":"0","object":"` + probeJWT + `"}`

	// probeTabLive：那个主播的一间在播 + 一间**别的主播**的在播（实拍出现过）。
	probeTabLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":1,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}},` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,"channelId":29,` +
		`"anchor":{"nickname":"福建移动"}}}]}}`
	// probeTabPreview：另一页签里只有**预告场**（被门筛掉，但界面要显示开播时间）。
	probeTabPreview = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147608,"title":"国庆出游AI搞定攻略！","status":0,"channelId":134,` +
		`"expectStartTime":"2026-09-23 14:30:00","anchor":{"nickname":"中国移动云盘"}}}]}}`

	probeBalanceBody = `{"code":0,"msg":"success","result":{"isQuarterDrawed":0,` +
		`"flowerCnt":235,"prizeId":0,"secondPrizeId":0,` +
		`"redFlowerValidDate":"2026年12月31日","hasNotReceivedRedFlower":1}}`
	probeNoResultBody = `{"code":0,"msg":"success"}`

	// 云盘会话那两步（`api/timestamp` → `tyrzLogin`）。
	//
	// ⚠ 为什么探针要建这一支会话：`liveRoomFeedback/*` 那一族虽然挂在
	// `/ycloud` 路径下，认的却是 `tyrzLogin` 换出来的 `jwttoken`，
	// 拿直播侧那串 `Authorization` 去读实测回 `90001 未登录`
	// （判据全文见 app.ProbeLiveRoom 的补记）。
	probeH5TimestampBody = `{"code":0,"msg":"success","result":1789800128813}`
	// probeH5JWT 同 probeJWT：合成值，**不许**出现在响应体里（下面逐条查）。
	probeH5JWT     = "SYNTH-H5-JWT-not-a-real-credential"
	probeH5LoginOk = `{"code":0,"msg":"success","result":{"token":"` + probeH5JWT + `"}}`
)

// probeRoutes 给出门那三步 + 云盘会话 + 一次余额读数的脚本（两个页签回同一份常见样本）。
func probeRoutes(balance string) *probeScript {
	return probeRoutesWithTabs(probeTabLive, probeTabPreview, balance)
}

// probeRoutesWithTabs 给出门那三步、云盘会话那两步与一次余额读数的脚本。
//
// ⚠ `querySpecTokenV2` 只配一条就够：门（`toSourceId=017027`）与云盘会话
// （`001005`）打的是**同一个 URL**，而这份脚本在样本用完后会重复最后一条。
func probeRoutesWithTabs(firstTab, secondTab, balance string) *probeScript {
	return newProbeScript(
		probeRoute{match: "/user/querySpecTokenV2", bodies: []string{probeSSOBody}},
		probeRoute{match: "/cmlive/api/user/auth/cmpassLoginByCmToken", bodies: []string{probeJWTBody}},
		probeRoute{match: "/channel/channel/tabContentList", bodies: []string{firstTab, secondTab}},
		probeRoute{match: "/ycloud/api/timestamp", bodies: []string{probeH5TimestampBody}},
		probeRoute{match: "/ycloud/auth-service/auth/tyrzLogin", bodies: []string{probeH5LoginOk}},
		probeRoute{match: "/liveRoomFeedback/user/info", bodies: []string{balance}},
	)
}

// seedProbeAccount 落一个"App 端、凭据齐全"的账号，返回界面上那一行的 id。
func seedProbeAccount(t *testing.T, h *harness) string {
	t.Helper()
	return seedRefreshable(t, h, appDue())
}

// liveRoomGet 打这一条并返回解码后的响应。
func liveRoomGet(t *testing.T, h *harness, id string) map[string]any {
	t.Helper()
	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("探针应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	return decode(t, rec)
}

// ─────────────────────────────────────────────────────────────────────────────
// 主用例：形状 + 只读
// ─────────────────────────────────────────────────────────────────────────────

// TestProbeLiveRoomRouteReportsBalanceAndRoomsWithoutWriting：端到端形状。
//
// 四件事一起成立才算通过：余额与房间都到位、两个页签都拉了、
// **一条写操作都没发**、体里不含任何凭据。
func TestProbeLiveRoomRouteReportsBalanceAndRoomsWithoutWriting(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	auth := readAuth(t, h, id)

	client := probeRoutes(probeBalanceBody)
	h.replaceRunWorld(t, client)

	payload := liveRoomGet(t, h, id)

	if payload["accountId"] != id {
		t.Errorf("accountId 应是掩码 id %q，实得 %v", id, payload["accountId"])
	}
	if payload["anchor"] != upstream.KoulingAnchor {
		t.Errorf("anchor 应是后端自述的那个主播，实得 %v", payload["anchor"])
	}
	// 门：`LiveCount` 数的是**所有**在播房间（不限主播），它与 `koulingLive` 一起
	// 才分得开两种"没有"：0 = 谁都没播，>0 而 koulingLive 假 = 有别家在播。
	// 样本里有两间在播（那主播的一间 + 福建移动的一间），所以这里是 2 而不是 1。
	if payload["koulingLive"] != true {
		t.Errorf("样本里那个主播正开着一间，koulingLive 应为 true，实得 %v", payload["koulingLive"])
	}
	if got := numField(payload["liveCount"]); got != "2" {
		t.Errorf("liveCount 应是在播房间总数 2（不限主播），实得 %v", payload["liveCount"])
	}

	rooms, ok := payload["rooms"].([]any)
	if !ok || len(rooms) != 2 {
		t.Fatalf("rooms 应是那个主播的两间（在播 + 预告），实得 %+v", payload["rooms"])
	}
	first, _ := rooms[0].(map[string]any)
	if numField(first["roomId"]) != "147606" {
		t.Errorf("第一间应是 147606，实得 %v", first["roomId"])
	}
	if first["statusToken"] != "live" {
		t.Errorf("在播那间的 statusToken 应是 live，实得 %v", first["statusToken"])
	}
	second, _ := rooms[1].(map[string]any)
	if second["statusToken"] != "not-started" || second["expectStartTime"] != "2026-09-23 14:30:00" {
		t.Errorf("预告场必须带着开播时间回来（界面上\"下一场几点\"读它），实得 %+v", second)
	}
	// 别的主播那一间不出现：上面 `len(rooms) != 2` 已经钉住数量，
	// 这里再钉住"福建移动那间没混进来"——它是最容易被"顺手全列出来"写进去的。
	for _, raw := range rooms {
		room, _ := raw.(map[string]any)
		if numField(room["roomId"]) == "147490" {
			t.Errorf("别的房间混进了日程：%+v", room)
		}
	}

	balance, ok := payload["balance"].(map[string]any)
	if !ok {
		t.Fatalf("balance 应是一节对象，实得 %v", payload["balance"])
	}
	if got := numField(balance["flowerCnt"]); got != "235" {
		t.Errorf("flowerCnt 应是 235，实得 %v", balance["flowerCnt"])
	}
	if balance["validDate"] != "2026年12月31日" {
		t.Errorf("validDate 应是上游那串中文，实得 %v", balance["validDate"])
	}
	if got := numField(balance["hasNotReceived"]); got != "1" {
		t.Errorf("hasNotReceived 应原样带出（界面按它提示\"有未领\"），实得 %v", balance["hasNotReceived"])
	}
	if message, _ := payload["balanceError"].(string); message != "" {
		t.Errorf("读到了就不该带失败句：%q", message)
	}

	// ⭐ 主断言：一条写操作都不许有。错的时候界面上一切正常，
	// 只是"看一眼"顺手把那两个口令投掉了。
	for _, match := range []string{"/redFlower/draw", "/firstParticipate/give",
		"/visitlog/journaling", "/redFlower/queryRevivalFlowers", "xiaohongshu.com",
		"/customer/userToken", "/room/front/roomBeat", "/v3/room/member/apply",
		"/user/auth/getCurrentUserInfoNew"} {
		if n := client.count(match); n != 0 {
			t.Errorf("只读探针打了 %s %d 次", match, n)
		}
	}
	// 门是**两个页签**，不是"顺手拉一次"（漏一个页签会整场漏口令）。
	if n := client.count("tabContentList"); n != 2 {
		t.Errorf("应打两个页签，实得 %d", n)
	}
	if n := client.count("/liveRoomFeedback/user/info"); n != 1 {
		t.Errorf("余额应恰好读一次，实得 %d", n)
	}

	// 直播侧 JWT、云盘侧 JWT 与整串 auth 都不许出现在体里。
	body, _ := core.MarshalJSON(payload)
	text := string(body)
	for _, forbidden := range []string{probeJWT, probeH5JWT, auth,
		strings.TrimPrefix(auth, "Basic "), "199-sso-token"} {
		if forbidden != "" && strings.Contains(text, forbidden) {
			t.Errorf("响应里带着凭据（%q）：%s", brief(forbidden), brief(text))
		}
	}
	if strings.Contains(text, "19900000001") {
		t.Errorf("响应里出现完整手机号，界面只该看到掩码 id：%s", brief(text))
	}
}

// TestProbeLiveRoomDegradesBalanceButKeepsGate：余额读数坏了仍是 200。
//
// "今晚有一场"与"这一号有多少朵"是这一屏要回答的**两个独立**问题；
// 后者取不到时要把原因说出去，而不是整屏报错、也不是显示成 0 朵。
func TestProbeLiveRoomDegradesBalanceButKeepsGate(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	h.replaceRunWorld(t, probeRoutes(probeNoResultBody))

	payload := liveRoomGet(t, h, id)
	if payload["balance"] != nil {
		t.Errorf("读失败时 balance 必须是 null，实得 %v", payload["balance"])
	}
	message, _ := payload["balanceError"].(string)
	if !strings.Contains(message, "user/info") {
		t.Errorf("balanceError 该说出是哪一次读数坏了，实得 %q", message)
	}
	if payload["koulingLive"] != true {
		t.Errorf("余额坏了不影响门的结论，koulingLive 应保持 true，实得 %v", payload["koulingLive"])
	}
}

// TestProbeLiveRoomReportsGateClosedAsSuccess：一间都不是那个主播的 ⇒ 200 + koulingLive=false。
//
// "今天没开播"是**正常结论**，不是错误；只有门本身坏了（换不到凭据）才该报状态码。
func TestProbeLiveRoomReportsGateClosedAsSuccess(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	const otherAnchor = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,"channelId":29,` +
		`"anchor":{"nickname":"福建移动"}}}]}}`
	// 两个页签都只给那间**别的主播**的在播房：于是 LiveCount=1 而 koulingLive=false，
	// 界面该说"有 1 间在播，但没有一间是「中国移动云盘」"，而不是"今天没有直播"。
	h.replaceRunWorld(t, probeRoutesWithTabs(otherAnchor, otherAnchor, probeBalanceBody))

	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("没开播应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	payload := decode(t, rec)
	if payload["koulingLive"] != false {
		t.Errorf("都不是那个主播的房间时 koulingLive 应为 false，实得 %v", payload["koulingLive"])
	}
	if got := numField(payload["liveCount"]); got != "1" {
		t.Errorf("有一间别的主播在播 ⇒ liveCount=1（两种\"没有\"要靠它分开），实得 %v", payload["liveCount"])
	}
	if rooms, ok := payload["rooms"].([]any); !ok || len(rooms) != 0 {
		t.Errorf("日程应是空数组而不是 null，实得 %+v", payload["rooms"])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属 / 免登录 / 装配
// ─────────────────────────────────────────────────────────────────────────────

// TestProbeLiveRoomNeedsAnAccountInTheCallersScope：别人的与从未存在的都是 404 且零请求。
//
// 这一条会打上游（八枪），所以"越权的请求连网络都不该摸到"比刷新那条更要钉 ——
// 它读的是**别人账号的余额**，那是数据泄漏面，不只是省一次请求。
func TestProbeLiveRoomNeedsAnAccountInTheCallersScope(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	admin := h.cookie

	client := probeRoutes(probeBalanceBody)
	h.replaceRunWorld(t, client)

	h.cookie = secondUser(t, h, "bob")
	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("别人的账号应 404（403 会泄露 id 存在），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}

	h.cookie = admin
	rec = h.do(t, "GET", "/api/accounts/19****99/live-room", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("从未存在的 id 应 404，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	if client.sent() != 0 {
		t.Errorf("越权与不存在的 id 一次上游都不该打，实得 %d 次", client.sent())
	}
}

// TestProbeLiveRoomRefusesAnonymousRequests：未登录拿不到任何账号的余额。
func TestProbeLiveRoomRefusesAnonymousRequests(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)

	client := probeRoutes(probeBalanceBody)
	h.replaceRunWorld(t, client)

	h.cookie = nil
	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("未登录应 401，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 0 {
		t.Errorf("未登录的请求一次上游都不该打，实得 %d 次", client.sent())
	}
}

// TestProbeLiveRoomWithoutARunWorld：没装配执行面 ⇒ 500，不是 200 带个空结论。
//
// 这一条分支在正常装配下走不到（`Run` 总会建那份世界），所以它测的是"装配漏了"
// 这种**部署期**错误：界面此时该显示"服务端没装配"，而不是"今天没有直播"——
// 后者会让人白等一场。
func TestProbeLiveRoomWithoutARunWorld(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	h.server.runs = nil

	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusInternalServerError {
		t.Errorf("缺执行面应 500，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
}

// TestProbeLiveRoomFailsWhenTheGateFails：换不到凭据时报错，不降级成"没有直播"。
//
// 夹具**不**换客户端：默认那份 [refusingClient] 就是"打不出去"。
// 5xx 走类别文案，所以这里断的是"message 里没有内部原文、也没有凭据"。
func TestProbeLiveRoomFailsWhenTheGateFails(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)

	rec := h.do(t, "GET", "/api/accounts/"+id+"/live-room", "", nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("打不出去应 500，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	assertNoPastedAuth(t, "探针失败的响应", rec.Body.String(), readAuth(t, h, id))
	message, _ := decode(t, rec)["message"].(string)
	if strings.Contains(message, "拒绝联网") {
		t.Errorf("5xx 把内部原文端给了使用者：%q", message)
	}
}

// TestProbeLiveRoomIsNotReachableByPost：写方法落在同一路径上必须 404。
//
// 这条接口是只读的，而"只读"在它身上是**契约**（点一下不许投出口令）。
// 挂上 POST 就等于给界面多开一条"看着像刷新、其实是投口令"的路。
func TestProbeLiveRoomIsNotReachableByPost(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)

	client := probeRoutes(probeBalanceBody)
	h.replaceRunWorld(t, client)

	rec := h.do(t, "POST", "/api/accounts/"+id+"/live-room", `{}`, nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("POST 这条路径应 404（只读接口不认领写方法），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	if client.sent() != 0 {
		t.Errorf("不该被路由到的请求打了上游 %d 次", client.sent())
	}
}

// TestLiveRoomStatusToken 钉住状态码 → 稳定英文码那张表。
//
// 为什么不靠上面的端到端用例覆盖：`status == 6`（回放）与"上游哪天多一档"
// 这两支在真实样本里都不在当前页签里，造一条假响应要走完整个门。
// 未知值必须落 `unknown` 而不是替上游编一个中文名。
func TestLiveRoomStatusToken(t *testing.T) {
	for status, want := range map[int64]string{
		upstream.LiveRoomStatusNotStarted: "not-started",
		upstream.LiveRoomStatusLive:       "live",
		upstream.LiveRoomStatusEnded:      "ended",
		7:                                 "unknown",
		-1:                                "unknown",
	} {
		if got := liveRoomStatusToken(status); got != want {
			t.Errorf("status %s 应翻成 %q，实得 %q",
				strconv.FormatInt(status, 10), want, got)
		}
	}
}
