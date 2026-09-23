package cli

// `live-room` 这一层的测试。
//
// # 这一层要钉的是"只有它看得见"的三件事
//
//   - **用法闸**：`--kouling-only` 与 `--skip-kouling` 同时给是**语义**矛盾
//     （一个说"不投"、一个说"只投"），必须退 2 且 stdout 干净；
//   - **退出码**：「今天没开播」是**正常**结果（退 0）——
//     与签到里"今天已经签过了"同一档；只有"门那三步走不通"才算运行错误（退 1）；
//   - **有没有拨号**：测试里那条 WS 是假的，所以"这次到底开没开连接"
//     本身就是一条可断言的事实（`chatRoomId: null` 时**不该**拨号）。
//
// 库用**真实的 SQLite**（与 run_auth_test.go / run_smid_test.go 同理）：
// 这一层判的就是"从库里读出了什么"，换成假存储等于把被测对象换掉。

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 响应体样本
// ─────────────────────────────────────────────────────────────────────────────

const (
	// liveRoomTestPhone 是合成号段（`199` 开头）—— 标识守卫的唯一不变量。
	liveRoomTestPhone = "19900000001"

	// liveRoomTestToken 是 WS 用的合成 access_token。
	//
	// ⚠ 它只有 17 字符。标识守卫那条规则认的是「敏感键名 + **20 字符以上**的值」，
	// 所以这里短一点不是"为了过门禁把数据改小"（那方向是反的），
	// 而是它本来就该是一眼假的合成值 —— 真实值约 66 字符、`A03` 前缀。
	liveRoomTestToken = "A03test-ws-token"

	// liveRoomKouling 是弹幕里那条口令的**正文**。
	liveRoomKouling = "中秋口令甲"

	// liveRoomKoulingSecond 是重听那一次听到的口令。
	liveRoomKoulingSecond = "国庆口令乙"

	// liveRoomDrawInvalidBody 是 `200108`（口令不对）。
	//
	// ⭐ 它**不在**原项目里 —— 2026-09-22 实拍才第一次见到。
	// `msg` 那句「口令需当场直播获取才有效哦」是排查时唯一的上下文，别整理掉。
	liveRoomDrawInvalidBody = `{"code":200108,"msg":"口令不对耶，口令需当场直播获取才有效哦"}`

	// liveRoomDrawAlreadyDrawnBody 是 `200112`（这个口令已经兑换过）。
	//
	// ⚠⚠ **这条 `msg` 是合成的**：码取自原项目 `standalone.ts:72` 的字面量，
	// 而它到 2026-09-22 **仍无实拍**（那一轮拿到的两个口令都是无效的）。
	// 这里只要码对得上就够 —— 而正因为它是合成的，**别把这段中文抄进任何文档**，
	// 那会把"编的"变成"看起来像实拍的"。
	liveRoomDrawAlreadyDrawnBody = `{"code":200112,"msg":"（合成样本，不是实拍原文）"}`

	// liveRoomDrawNoCodeBody 是**信封里没有 `code`** 的响应（判定为 Unparsable）。
	//
	// 它存在的理由很具体：`--json` 的 `draws[].upstreamCode` 在"没解出业务码"时
	// 必须是 `null`，而"码是 0"与"根本没有码"是两件事 ——
	// 没有这个样本，那个 `null` 分支就没被断言过。
	liveRoomDrawNoCodeBody = `{"msg":"（合成样本：信封里没有 code）"}`

	// liveRoomDanmakuText 是弹幕**原文** —— 注意它带着 `小红花口令：` 那个头。
	//
	// ⚠ 提取器认的是那个头（实拍口径）：只给口令本身，四个分支一个都不成立
	// ⇒ 一条都提不出来。第一次写这个文件时就是这么错的，而表现是
	// "WS 那一源静默地没收到任何口令"——看起来像连接或帧解析的问题。
	liveRoomDanmakuText = "小红花口令：" + liveRoomKouling

	// 门的四步（`querySpecTokenV2` → `cmpassLoginByCmToken` → 两个页签）。
	liveRoomSSOBody  = `{"code":0,"msg":"success","result":{"token":"199-sso-token"}}`
	liveRoomJWTBody  = `{"rtnMsg":null,"rtnCode":"0","object":"199.jwt.token"}`
	liveRoomTabEmpty = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[]}}`

	// 云盘会话那三步的第二、第三步（第一步与门共用 `querySpecTokenV2`，只是 `toSourceId` 不同）。
	// ⚠ 这里的 token 是**云盘侧**那一份 JWT，与 `liveRoomJWTBody`（直播侧）不是一支。
	liveRoomH5TimestampBody = `{"code":0,"msg":"success","result":1789800128813}`
	liveRoomH5LoginBody     = `{"code":0,"msg":"success","result":{"token":"199.h5.jwt.token"}}`

	// liveRoomTabLive：一间**在播的**官方直播间。
	//
	// ⚠ 门的判据是**两个条件同时成立**：`status == 1` **且**
	// `anchor.nickname == 中国移动云盘`。只判前者会把 147490「福建移动」也算进去。
	liveRoomTabLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":1,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	// liveRoomTabTwoLive：**同一个主播两间都在播** —— 用来钉住"只听第一间"这个取舍。
	//
	// 实拍依据：2026-09-22 那轮里 147606 同时出现在 127/129 两个页签，
	// 而 147608（国庆场）只在 129 里 ⇒ "同一主播多间在播"是会发生的，
	// 不是理论情况。⚠ 第二间的 `channelId` 刻意给 243（实拍里返回过的值之一）。
	//
	// ⚠ 这类**两层嵌套 + 数组**的夹具，括号数靠数是不行的：第一次写就多了一个 `}`，
	// 而"夹具本身不是合法 JSON"在没人消费它的时候**完全没有信号**
	// （它躺在 const 块里，`go build` 与 `go vet` 都不会看一眼）。
	// 表现是跑起来才炸，且错误信息指向**页签那一步**（"响应体不是合法 JSON"），
	// 看起来像上游格式变了 —— 排查方向完全被带偏。写完就用 `json.loads` 过一遍。
	liveRoomTabTwoLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":1,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}},` +
		`{"room":{"id":147608,"title":"国庆专场","status":1,` +
		`"channelId":243,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	// liveRoomTabOtherLive：**有人**在播，但不是那个主播（实拍踩过 147490）。
	liveRoomTabOtherLive = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,"channelId":29,` +
		`"anchor":{"nickname":"福建移动"}}}]}}`

	// 进直播间那四步。
	liveRoomCurrentUserBody = `{"rtnMsg":null,"rtnCode":"0","object":{"userId":47776930}}`
	liveRoomBeatLiveBody    = `{"rtnMsg":null,"rtnCode":"0","object":{` +
		`"status":1,"channelId":134,"chatRoomId":"1470000000000000000000000000001",` +
		`"anchor":{"id":1430,"nickname":"中国移动云盘"}}}`
	liveRoomApplyBody = `{"code":0,"msg":"success","timestamp":1790000000123,` +
		`"data":{"appId":"cmos","appKey":"cmos#ngvlcs",` +
		`"roomId":"1470000000000000000000000000001"}}`

	// 六步链。
	liveRoomJournalingBody = `{"code":0,"msg":"success","result":null}`
	liveRoomGiveGainedBody = `{"code":0,"msg":"success","result":{"flowerNum":3}}`
	liveRoomDrawOKBody     = `{"code":0,"msg":"success","result":{"prizeId":1,` +
		`"flowerNum":5,"prizeName":"小红花","prizeType":1}}`
	liveRoomRevivalBody       = `{"code":0,"msg":"success","result":0}`
	liveRoomXiaohongshuHTML   = `<html><body><div><span>小红花口令：小红书来的口令</span></div></body></html>`
	liveRoomXiaohongshuNoNote = `<html><body><div>暂无笔记</div></body></html>`
)

// liveRoomUserInfoBody 造 `user/info` 的响应（余额读的就是 `flowerCnt`）。
func liveRoomUserInfoBody(flowerCnt int64) string {
	return fmt.Sprintf(`{"code":0,"msg":"success","result":{"isQuarterDrawed":0,`+
		`"flowerCnt":%d,"prizeId":0,"redFlowerValidDate":"2026年12月31日"}}`, flowerCnt)
}

// liveRoomTokenBody 造 `userToken` 的响应。
//
// ⚠ 与 app 层那份同理：它是**函数**而不是常量，因为标识守卫扫的是**文件文本** ——
// 写成 `"access_token":"<20+ 字符>"` 时那个字面量一超 20 字符就命中，
// 而走 `%s` 之后那个位置只有两个字符。
func liveRoomTokenBody() string {
	return fmt.Sprintf(`{"rtnMsg":null,"rtnCode":"0","object":`+
		`{"access_token":"%s","expires_in":5184000,"refresh_token":"x"}}`,
		liveRoomTestToken)
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试替身
// ─────────────────────────────────────────────────────────────────────────────

// liveRoomRoutes 是按 URL 分发的脚本客户端，**匹配即消耗**。
//
// # 为什么要"消耗"
//
// `user/info` 在六步链的首尾各打一次，两次的响应**必须不同**（那才是余额的
// 两个读数）。"第一个匹配永远命中"的路由表会让两次读到同一个值 ⇒
// `Gained()` 恒为 0，而"进账 0"看起来完全正常（一次没投中的运行就是这样）——
// 这种测试**绿着骗人**，比红更糟。
//
// ⚠ 也不能改成"按顺序发、不看 URL"（app 层那种顺序队列）：收口令的两个来源
// 是**并发**的（WS 一支要听满一段，小红书只是一次 GET），顺序队列会把给 WS
// 那支的响应喂给小红书那支 —— 在 app 层踩过一次，表现是"响应莫名其妙地错位"。
//
// ⇒ "按 URL 匹配 + 匹配即消耗"同时满足这两条：同一 URL 的多次请求各拿各的，
// 而两个并发来源之间不需要定序。
//
// 认不出的 URL **显式回 404** 并记进 `unmatched`（见 [liveRoomRoutes.assertRouted]）：
// 安静地给空体会让"少配了一条路由"变成"解析失败"，排查方向完全不同。
type liveRoomRoutes struct {
	mu        sync.Mutex
	routes    []liveRoomRoute
	requests  []core.HTTPRequest
	unmatched []string
}

type liveRoomRoute struct {
	match string
	body  string
}

func newLiveRoomRoutes(routes ...liveRoomRoute) *liveRoomRoutes {
	return &liveRoomRoutes{routes: routes}
}

func (c *liveRoomRoutes) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.requests = append(c.requests, request)

	for i, route := range c.routes {
		if strings.Contains(request.URL, route.match) {
			c.routes = append(c.routes[:i:i], c.routes[i+1:]...)
			return core.NewResponse(200, []byte(route.body)), nil
		}
	}
	c.unmatched = append(c.unmatched, request.URL)
	return core.NewResponse(404, []byte(`{"error":"liveRoomRoutes 没有为这个 URL 配路由"}`)), nil
}

// assertRouted 报告"有请求没配到路由"。
//
// ⚠ 它存在的理由很具体，而且已经用上过一次：路由是按**手写的 URL 片段**匹配的，
// 打错一个字母的后果是 404，而上游层把它包成一句 `HTTP 404` ——
// 失败信息指向"直播·登录失败"，看起来像协议问题，看不出是"这个 URL 没配路由"。
// 第一次写这个文件时就把 `cmpassLoginByCmToken` 打成了 `cpass…`。
//
// ⇒ 所以它要在**退出码断言之前**调用：那才是"错误信息指向对方向"的那一下。
func (c *liveRoomRoutes) assertRouted(t *testing.T) {
	t.Helper()
	c.mu.Lock()
	defer c.mu.Unlock()
	if len(c.unmatched) > 0 {
		t.Fatalf("这些 URL 没配路由（多半是 match 片段打错了）：%v", c.unmatched)
	}
}

// urls 返回这次打过的所有 URL（快照，便于并发场景下断言）。
func (c *liveRoomRoutes) urls() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, 0, len(c.requests))
	for _, r := range c.requests {
		out = append(out, r.URL)
	}
	return out
}

// countMatching 数打了几个 URL 含某子串的请求。
func (c *liveRoomRoutes) countMatching(fragment string) int {
	count := 0
	for _, url := range c.urls() {
		if strings.Contains(url, fragment) {
			count++
		}
	}
	return count
}

// bodyMatching 返回第一个 URL 含某子串的请求的**请求体**（没有则空串）。
//
// 为什么需要它：有些事实**不在 URL 里**。房间号就是典型 ——
// 进房那一步（`roomBeat`）的 URL 是固定的一条，房间号在**请求体**里
// （`{"roomId":"147606","channelId":"134","sourceApp":0}`）；
// 而 WS 地址的形状是 `/ws/<serverId>/<sessionId>/websocket`，里面**也没有房间号**。
// ⇒ "到底进了哪一间"这件事，只能从请求体上看。
//
// ⚠ 只取第一个匹配的：用它做断言时要先确认那个 URL 只被打了一次
// （不然拿到的是哪一次说不清）—— 与 [countMatching] 配对使用。
func (c *liveRoomRoutes) bodyMatching(fragment string) string {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, r := range c.requests {
		if strings.Contains(r.URL, fragment) {
			return string(r.Body)
		}
	}
	return ""
}

// liveRoomWSConn 是脚本化的 WebSocket 连接：读按顺序吐，写完记下来。
type liveRoomWSConn struct {
	reads []string
	wrote []string
}

func (c *liveRoomWSConn) ReadText(context.Context) (string, error) {
	if len(c.reads) == 0 {
		// 听满一段的正常结束方式之一：服务端关连接。
		return "", io.EOF
	}
	next := c.reads[0]
	c.reads = c.reads[1:]
	return next, nil
}

func (c *liveRoomWSConn) WriteText(_ context.Context, text string) error {
	c.wrote = append(c.wrote, text)
	return nil
}

func (c *liveRoomWSConn) Close() error { return nil }

// liveRoomDialer 按顺序给出连接，并记下被拨过的 URL ——
// "有没有开连接"本身就是被断言的事实之一。
//
// ⚠ 它给的是**队列**而不是同一个连接：重听一次会再拨一次号，而真实的拨号器
// 每次都返回一条**新**连接。复用同一条会让第二次"读"接在第一次的 EOF 之后，
// 于是重听**永远收不到东西** —— 而表现是"重听没用"，看起来像服务端的问题。
type liveRoomDialer struct {
	conns []*liveRoomWSConn
	urls  []string
}

func newLiveRoomDialer(conns ...*liveRoomWSConn) *liveRoomDialer {
	return &liveRoomDialer{conns: conns}
}

// liveRoomOneConn 造一个"只有一条连接"的拨号器（读序列就是那几个字符串）。
func liveRoomOneConn(reads ...string) *liveRoomDialer {
	return newLiveRoomDialer(&liveRoomWSConn{reads: reads})
}

func (d *liveRoomDialer) Dial(
	_ context.Context, url string, _ []core.HeaderPair,
) (core.WebSocketConn, error) {
	d.urls = append(d.urls, url)
	if len(d.conns) == 0 {
		return nil, fmt.Errorf("liveRoomDialer 没有更多连接了（这是第 %d 次拨号）", len(d.urls))
	}
	next := d.conns[0]
	d.conns = d.conns[1:]
	return next, nil
}

// liveRoomDanmakuFrame 造一条含口令的弹幕帧。
//
// ⚠ 用 [upstream.EncodeMessageBody] + [upstream.SockJSDataFrame] 而不是硬编码
// base64：硬编码的测试只证明"我抄的字节与实现一致"，两处都错时照样绿。
func liveRoomDanmakuFrame(t *testing.T, text string) string {
	t.Helper()
	payload := upstream.EncodeMessageBody(upstream.MessageBody{
		Contents: []upstream.MessageContent{{Type: 1, Text: text}},
	})
	return upstream.SockJSDataFrame(upstream.LiveMSyncFrame(
		liveRoomTestToken, liveRoomTestPhone,
		upstream.CmdSync, payload,
		time.UnixMilli(1790000000000).UTC(), &cliRng{}))
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// liveRoomSeedAccount 是库里一个账号的样子。
type liveRoomSeedAccount struct {
	phone   string
	enabled bool
}

// liveRoomSeed 建一个只有一个启用账号的库，返回库路径。
func liveRoomSeed(t *testing.T) string {
	t.Helper()
	return liveRoomSeedWith(t, liveRoomSeedAccount{liveRoomTestPhone, true})
}

// liveRoomSeedWith 建库并塞进这些账号。
//
// 凭据按 **v7 口径**（整串 `Authorization`）落库 —— 那是新账号的真实形态，
// 而"拆开存、用时再拼"那条老路只在 v7 之前的行上才走。
func liveRoomSeedWith(t *testing.T, accounts ...liveRoomSeedAccount) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "state.db")
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("建临时库失败: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	for _, seed := range accounts {
		id := core.AccountIDFor(seed.phone, nil)
		account := core.Account{
			ID: id, Enabled: seed.enabled, Platform: authStrPtr(upstream.IOSAuthPrefix),
		}
		if err := store.UpsertAccount(ctx, core.ScopeLocal(), &account); err != nil {
			t.Fatalf("写入账号失败: %v", err)
		}

		token := "SYNTHAAA|1|RCS|" + authI64Text(cliNow().UnixMilli()) + "|AAAA"
		authorization := core.SecretValue(authStringOf(upstream.IOSAuthPrefix, seed.phone, token))
		jwt := core.SecretValue(token)
		domain := core.UserDomainID("1990000000000000001")
		state := core.AccountState{
			AccountID: id, Authorization: &authorization, JWTToken: &jwt, UserDomainID: &domain,
		}
		if err := store.UpsertAccountState(ctx, core.ScopeLocal(), &state); err != nil {
			t.Fatalf("写入登录态失败: %v", err)
		}
	}
	return path
}

// runLiveRoomCLI 跑一次 `mcloud live-room`。
//
// ⚠ 三个"必须顶掉"的默认值：`--db`（默认相对路径，会在包目录下建真库）、
// `--app-device-file`（同理，而且它会**写盘**）、时钟（不然 `x-timestamp` 会漂）。
func runLiveRoomCLI(
	t *testing.T, client core.HTTPClient, dialer core.WebSocketDialer, db string, args ...string,
) (string, string, int) {
	t.Helper()
	dir := t.TempDir()
	argv := append([]string{
		"live-room",
		"--db", db,
		"--app-device-file", filepath.Join(dir, "app-device.txt"),
	}, args...)

	deps := &Deps{Clock: core.NewFixedClock(cliNow()), Random: &cliRng{next: 7}}
	if client != nil {
		deps.HTTP = client
	}
	if dialer != nil {
		deps.WebSocket = dialer
	}
	return runCLIWithDeps(t, deps, "", argv...)
}

// liveRoomGateOpen 是"门"的四步：换 SSO token → 换直播侧 JWT → 两个页签各拉一次。
//
// ⚠ 两个页签**都要配**（`tabId=127` 盘盘福利 / `129` 精彩回放）：实拍里同一间房
// 两边都有，而下一场（147608 国庆场）**只在 129 里** —— 只查 127 会在
// "房间还没进福利页签"时判成"没开播"，那是"漏掉整场口令"这一侧的错误。
func liveRoomGateOpen(tabBody string) []liveRoomRoute {
	return []liveRoomRoute{
		{"/user/querySpecTokenV2", liveRoomSSOBody},
		{"/cmpassLoginByCmToken", liveRoomJWTBody},
		{"/tabContentList", tabBody},
		{"/tabContentList", tabBody},
	}
}

// liveRoomSixStepRoutes 按**请求顺序**排好六步链的响应。
//
// 为什么不能"一次配完"：`user/info` 在链首与链尾各打一次，
// 而路由表是"第一个匹配即命中"，两次读数会拿到同一个值 —— 于是 `Gained()`
// 恒为 0，而"进账 0"看起来完全正常（一次没投中的运行就是这样）。
// 这个坑在 app 层是用**顺序队列**避开的；CLI 层要按 URL 分发（因为收口令
// 那两支并发），所以这里显式展开成"每个请求一条"。
// liveRoomSixStepRoutes 是"投这几个口令、每个都成功"的六步链。
func liveRoomSixStepRoutes(before, after int64, codes []string) []liveRoomRoute {
	draws := make([]string, 0, len(codes))
	for range codes {
		draws = append(draws, liveRoomDrawOKBody)
	}
	return liveRoomSixStepRoutesDraws(before, after, draws)
}

// liveRoomSixStepRoutesDraws 同 [liveRoomSixStepRoutes]，但每个口令的响应自己给
// —— 重听那条路要的是"第一轮被判无效"。
//
// ⚠ 开头那三步是**云盘会话**（`querySpecTokenV2(001005)` → `api/timestamp` → `tyrzLogin`）：
// 六步链要的是 `tyrzLogin` 那一份 JWT，用直播侧那一份去投每一枪都回 `90001 未登录`
// （判据见 app.ProbeLiveRoom 的补记）。所以这里必须配**两次** `querySpecTokenV2`
// —— 门那一次 + 换云盘会话这一次。
func liveRoomSixStepRoutesDraws(before, after int64, drawBodies []string) []liveRoomRoute {
	routes := []liveRoomRoute{
		{"/user/querySpecTokenV2", liveRoomSSOBody},
		{"/ycloud/api/timestamp", liveRoomH5TimestampBody},
		{"/ycloud/auth-service/auth/tyrzLogin", liveRoomH5LoginBody},
		{"/liveRoomFeedback/user/info", liveRoomUserInfoBody(before)},
	}
	for i := 0; i < 5; i++ {
		routes = append(routes, liveRoomRoute{"/visitlog/journaling", liveRoomJournalingBody})
	}
	routes = append(routes, liveRoomRoute{"/firstParticipate/give", liveRoomGiveGainedBody})
	for _, body := range drawBodies {
		routes = append(routes, liveRoomRoute{"/redFlower/draw", body})
	}
	routes = append(routes,
		liveRoomRoute{"/redFlower/queryRevivalFlowers", liveRoomRevivalBody},
		liveRoomRoute{"/liveRoomFeedback/user/info", liveRoomUserInfoBody(after)},
	)
	return routes
}

// liveRoomCollectionRoutes 是收口令那一支要的四步进房 + 小红书。
//
// ⚠ 重听一次会把这一整串**再打一遍**（`Collect` 的语义就是"两个来源各跑一次"），
// 所以需要它的用例要配**两份** —— "匹配即消耗"的设计正好把这件事显式化：
// 少配一份的话第二次收集会撞 404，而不是安静地拿到上一次的响应。
func liveRoomCollectionRoutes(hasNote bool) []liveRoomRoute {
	note := liveRoomXiaohongshuNoNote
	if hasNote {
		note = liveRoomXiaohongshuHTML
	}
	return []liveRoomRoute{
		{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveRoomCurrentUserBody},
		{"/cmlive/api/user/customer/userToken", liveRoomTokenBody()},
		{"/cmlive/api/room/front/roomBeat", liveRoomBeatLiveBody},
		{"/v3/room/member/apply", liveRoomApplyBody},
		{"xiaohongshu.com", note},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 用法闸
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomReadOnlySwitchesAreMutuallyExclusive 钉住那一对**语义**矛盾。
//
// 一个说"不投"、一个说"只投"，同时给的话无论怎么解释都会让一半的意图静默失效 ——
// 而"静默失效"正是这类开关最坏的形态。
func TestLiveRoomReadOnlySwitchesAreMutuallyExclusive(t *testing.T) {
	db := liveRoomSeed(t)
	_, stderr, code := runLiveRoomCLI(t, nil, nil, db,
		"--kouling-only", "--skip-kouling", "--codes", liveRoomKouling)
	if code != ExitUsage {
		t.Fatalf("两个开关互斥，应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "互斥") {
		t.Errorf("stderr 应说清是互斥，实得 %q", stderr)
	}
}

// TestLiveRoomSkipKoulingNeedsCodes 钉住"把唯一来源关掉就必须另给一条"。
func TestLiveRoomSkipKoulingNeedsCodes(t *testing.T) {
	db := liveRoomSeed(t)
	_, stderr, code := runLiveRoomCLI(t, nil, nil, db, "--skip-kouling")
	if code != ExitUsage {
		t.Fatalf("`--skip-kouling` 没有 `--codes` 应退 %d，实得 %d\n  stderr=%q",
			ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "--codes") {
		t.Errorf("stderr 应指出缺的是 --codes，实得 %q", stderr)
	}
}

// TestLiveRoomRejectsNonPositiveListenSeconds 钉住"听 0 秒"是用法错误。
//
// 它不能被当成"立刻收工"：听 0 秒的结果**恒为空**，而空结果与
// "这段时间里主播没喊口令"在输出上完全一样 —— 那会让人以为今天没口令。
func TestLiveRoomRejectsNonPositiveListenSeconds(t *testing.T) {
	db := liveRoomSeed(t)
	for _, seconds := range []string{"0", "-1"} {
		_, stderr, code := runLiveRoomCLI(t, nil, nil, db, "--listen-seconds", seconds)
		if code != ExitUsage {
			t.Errorf("--listen-seconds %s 应退 %d，实得 %d（stderr=%q）",
				seconds, ExitUsage, code, stderr)
		}
	}
}

// TestLiveRoomRejectsAccountIndexOutOfRange 钉住下标越界是**用法**错误。
//
// `--account N` 里的 N 是展示下标（从 1 起）。越界是"调用写错了"，
// 不是"这次没跑成" —— 退出码必须分开，否则脚本分不清该不该重试。
func TestLiveRoomRejectsAccountIndexOutOfRange(t *testing.T) {
	db := liveRoomSeed(t)
	_, stderr, code := runLiveRoomCLI(t, nil, nil, db, "--account", "5", "--codes", liveRoomKouling)
	if code != ExitUsage {
		t.Fatalf("账号下标越界应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "超出范围") {
		t.Errorf("stderr 应说清是越界，实得 %q", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 手工口令那条路（不开 WS）
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomManualCodesDrawWithoutOpeningTheSocket 钉住 `--skip-kouling` 的形状。
//
// 判据有三条，缺一条这个开关就没被真正钉住：
//   - **一次都没拨号**（`--skip-kouling` 的语义就是不收）；
//   - 六步链**走完整**（5 条埋点一条不少 —— 少发一条上游只会回 `200108`，
//     而那看起来像"口令抄错了"）；
//   - 进账按**两次读数相减**算（7 = 17 − 10），不是把口令的 flowerNum 加起来
//     （那会漏掉"首次参与惊喜奖"的 3 朵）。
func TestLiveRoomManualCodesDrawWithoutOpeningTheSocket(t *testing.T) {
	db := liveRoomSeed(t)
	codes := []string{liveRoomKouling, "国庆口令乙"}
	routes := append(liveRoomGateOpen(liveRoomTabLive), liveRoomSixStepRoutes(10, 17, codes)...)
	client := newLiveRoomRoutes(routes...)
	dialer := &liveRoomDialer{conns: []*liveRoomWSConn{{}}}

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db,
		"--skip-kouling", "--codes", strings.Join(codes, ","))
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("手工口令这条路应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	if len(dialer.urls) != 0 {
		t.Fatalf("`--skip-kouling` 不该拨号，实际拨了 %v", dialer.urls)
	}
	if n := client.countMatching("/visitlog/journaling"); n != 5 {
		t.Errorf("埋点应发 5 条，实际 %d 条 —— 少发一条上游只会回 200108", n)
	}
	for _, want := range codes {
		if n := client.countMatching("code=" + upstream.PercentEncode(want)); n != 1 {
			t.Errorf("口令 %q 应被投一次（且 URL 编码），实际匹配 %d 次：%v",
				want, n, client.urls())
		}
	}
	if !strings.Contains(stdout, "10 → 17") || !strings.Contains(stdout, "进账 7") {
		t.Errorf("进账应按两次读数相减（10 → 17，进账 7），实得 %q", stdout)
	}
	if !strings.Contains(stdout, "首次参与 +3") {
		t.Errorf("首次参与那 3 朵要单独说出来（它不在任何口令的结果里），实得 %q", stdout)
	}
}

// TestLiveRoomKoulingOnlyNeverDraws 钉住 `--kouling-only` 是**只读**的。
//
// 这一条的代价最实在：`draw` 是**写操作**，一个"只是看看有没有口令"
// 的调用不该**投出任何一个口令**（口令本身全场通用，动的是这个账号的兑换状态）。
func TestLiveRoomKoulingOnlyNeverDraws(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabLive),
		liveRoomRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveRoomCurrentUserBody},
		liveRoomRoute{"/cmlive/api/user/customer/userToken", liveRoomTokenBody()},
		liveRoomRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLiveBody},
		liveRoomRoute{"/v3/room/member/apply", liveRoomApplyBody},
		liveRoomRoute{"xiaohongshu.com", liveRoomXiaohongshuNoNote},
	)
	client := newLiveRoomRoutes(routes...)
	conn := &liveRoomWSConn{reads: []string{"o", liveRoomDanmakuFrame(t, liveRoomDanmakuText)}}
	dialer := &liveRoomDialer{conns: []*liveRoomWSConn{conn}}

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db, "--kouling-only")
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("`--kouling-only` 应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	if n := client.countMatching("/redFlower/draw"); n != 0 {
		t.Fatalf("`--kouling-only` 不该投口令，实际投了 %d 次：%v", n, client.urls())
	}
	if n := client.countMatching("/liveRoomFeedback/user/info"); n != 0 {
		t.Errorf("只收口令不该碰六步链，实际打了 %d 次 user/info", n)
	}
	if !strings.Contains(stdout, liveRoomKouling) {
		t.Errorf("收口令那条路要把口令打出来（否则这个开关没有用处），实得 %q", stdout)
	}
	// 弹幕那条口令来自 WS；小红书那一支这次没有笔记 ⇒ 只剩一条。
	if strings.Contains(stdout, "小红书来的口令") {
		t.Errorf("小红书这次没发笔记，不该凭空多出一条口令：%q", stdout)
	}
}

// TestLiveRoomCollectsFromBothSources 钉住两个来源**都在用**、且合并且去重。
func TestLiveRoomCollectsFromBothSources(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabLive),
		liveRoomRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveRoomCurrentUserBody},
		liveRoomRoute{"/cmlive/api/user/customer/userToken", liveRoomTokenBody()},
		liveRoomRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLiveBody},
		liveRoomRoute{"/v3/room/member/apply", liveRoomApplyBody},
		liveRoomRoute{"xiaohongshu.com", liveRoomXiaohongshuHTML},
	)
	client := newLiveRoomRoutes(routes...)
	conn := &liveRoomWSConn{reads: []string{"o", liveRoomDanmakuFrame(t, liveRoomDanmakuText)}}

	stdout, stderr, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{conn}}, db, "--kouling-only")
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	for _, want := range []string{liveRoomKouling, "小红书来的口令"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("两个来源都该被收进来，缺 %q：%q", want, stdout)
		}
	}
}

// TestLiveRoomListensToOnlyTheFirstRoom 钉住"只听第一间"这个**取舍**。
//
// ⚠ 它是**取舍**不是优化：原项目是 `for (const room of rooms)` 逐间 `await`，
// 这里只取 `Rooms[0]` —— 拿"可能漏掉第一间之外的口令"换"墙钟时间不变成
// N × `--listen-seconds`"。既然是有代价的选择，就必须**同时**钉住两半：
//
//   - **只进了一间**：`roomBeat` 的请求体里是第一间（147606），
//     第二间（147608）一次都没被碰过；
//   - **而且说出来了**：房间数 > 1 时 note 里必须点明"只听第一间" ——
//     不说的话用户看到的是「在播房间 147606，听 60 秒」，会以为已经听全了。
//
// 少钉任一半，这个取舍就退化成"看起来像漏了循环"或"静默漏口令"。
//
// ⚠ 为什么"只进一间"**不靠 WS 地址断言**：WS 地址的形状是
// `/ws/<serverId>/<sessionId>/websocket`，里面**没有房间号** ⇒
// 光看拨号次数分不出听的是哪一间。带房间号的是 `roomBeat` 的**请求体**
// （见 [liveRoomRoutes.bodyMatching]）。
//
// ⚠ 关于最后那条"拨号次数 == 1"：它是**同一件事的第二个读数**，
// 不是独立的判据面 —— 反向测试里"改回逐间听"那条变异会先在
// `roomBeat` 计数上红掉，所以它**永远轮不到自己承重**。
// 留着它的理由是排查方向不同：`roomBeat` 计数红是"多进了房"，
// 拨号数红是"进了房但没连上/连重了"。**它未被独立断言**，别把它当成
// "两间都听也拦得住"的证据。
//
// ⚠ 路由**只给第一间配**：如果实现真的去听第二间，第二次
// `getCurrentUserInfoNew` 会撞 404 → `Collect` 失败 → 退 `ExitRun`。
// 所以"退 0"这一条本身也在守着这件事 —— 而 `assertRouted` 会把
// "少配了一条路由"与"实现多听了一间"这两种红区分开（前者说 URL 没配路由）。
func TestLiveRoomListensToOnlyTheFirstRoom(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabTwoLive),
		liveRoomRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveRoomCurrentUserBody},
		liveRoomRoute{"/cmlive/api/user/customer/userToken", liveRoomTokenBody()},
		liveRoomRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLiveBody},
		liveRoomRoute{"/v3/room/member/apply", liveRoomApplyBody},
		liveRoomRoute{"xiaohongshu.com", liveRoomXiaohongshuNoNote},
	)
	client := newLiveRoomRoutes(routes...)
	conn := &liveRoomWSConn{reads: []string{"o", liveRoomDanmakuFrame(t, liveRoomDanmakuText)}}
	dialer := &liveRoomDialer{conns: []*liveRoomWSConn{conn}}

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db, "--kouling-only")
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("`--kouling-only` 应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}

	// ── 前半：只进了第一间 ───────────────────────────────────────────────
	if n := client.countMatching("/cmlive/api/room/front/roomBeat"); n != 1 {
		t.Fatalf("两间在播也只该进第一间，`roomBeat` 实际打了 %d 次：%v", n, client.urls())
	}
	beat := client.bodyMatching("/cmlive/api/room/front/roomBeat")
	if !strings.Contains(beat, "147606") {
		t.Errorf("进的应该是第一间（147606），实际请求体是 %q", beat)
	}
	if strings.Contains(beat, "147608") {
		t.Errorf("第二间（147608）这一趟不该被碰，实际请求体是 %q", beat)
	}
	if len(dialer.urls) != 1 {
		t.Errorf("只听第一间就该只拨一次号，实际拨了 %d 次：%v", len(dialer.urls), dialer.urls)
	}

	// ── 后半：而且说出来了 ───────────────────────────────────────────────
	if !strings.Contains(stderr, "只听第一间") {
		t.Errorf("两间在播时必须点明只听第一间"+
			"（否则用户看到的是「在播房间 147606，听 60 秒」，会以为已经听全了）：stderr=%q", stderr)
	}
	if !strings.Contains(stderr, "2 间在播") {
		t.Errorf("那句 note 要说清是几间，实得 stderr=%q", stderr)
	}
}

// TestLiveRoomReusesOneWebSocketForBothPhases 钉住"收与投共用同一个 JWT"。
//
// ⚠ 它是一条**顺序**断言而不是"值相等"断言：门那三步只该走一次。
// 走两次的话第二次 `cmpassLoginByCmToken` 会换掉服务端 session，
// 而表现是"口令收到了、投的时候却说不认识" —— 排查方向会指向口令。
func TestLiveRoomReusesOneWebSocketForBothPhases(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabLive),
		liveRoomRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveRoomCurrentUserBody},
		liveRoomRoute{"/cmlive/api/user/customer/userToken", liveRoomTokenBody()},
		liveRoomRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLiveBody},
		liveRoomRoute{"/v3/room/member/apply", liveRoomApplyBody},
		liveRoomRoute{"xiaohongshu.com", liveRoomXiaohongshuNoNote},
	)
	routes = append(routes, liveRoomSixStepRoutes(10, 15, []string{liveRoomKouling})...)
	client := newLiveRoomRoutes(routes...)
	conn := &liveRoomWSConn{reads: []string{"o", liveRoomDanmakuFrame(t, liveRoomDanmakuText)}}

	_, stderr, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{conn}}, db)
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	// ⚠ `querySpecTokenV2` 现在是**两次**，不是一次：直播侧（`017027`）与云盘侧（`001005`）
	// 各换一支 SSO token。"门只走一次"这件事**不能**再用它的总数来判 —— 那会把
	// 正常建的第二支会话读成"重复开门"。真正只该一次的是一支票换一次的**直播侧登录**。
	if n := client.countMatching("/user/querySpecTokenV2"); n != 2 {
		t.Errorf("该各换一次票（直播侧 + 云盘侧），实际 %d 次", n)
	}
	if n := client.countMatching("/cmpassLoginByCmToken"); n != 1 {
		t.Errorf("直播侧登录只该走一次（换 session 会互相踢掉），实际 %d 次", n)
	}
	if n := client.countMatching("/ycloud/auth-service/auth/tyrzLogin"); n != 1 {
		t.Errorf("云盘会话只该在建口令之前换一次，实际 %d 次", n)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 门没开：两种"没有"必须分开说，且都退 0
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomReportsNobodyLive 钉住"谁都没在播"。
//
// ⭐ 这是 2026-09-22 抓包那天的真实处境：`status: 0` + `chatRoomId: null`。
// 它是**正常**结果（门开不开由服务端数据决定），所以退 0 ——
// 与签到里"今天已经签过了"同一档。把它当运行错误会让定时任务天天报红。
func TestLiveRoomReportsNobodyLive(t *testing.T) {
	db := liveRoomSeed(t)
	client := newLiveRoomRoutes(liveRoomGateOpen(liveRoomTabEmpty)...)
	dialer := &liveRoomDialer{conns: []*liveRoomWSConn{{}}}

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db)
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("今天没开播是正常结果，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stderr, "一间都没在播") {
		t.Errorf("要说清是「谁都没在播」，实得 stderr=%q", stderr)
	}
	if len(dialer.urls) != 0 {
		t.Errorf("门没开就不该拨号，实际拨了 %v", dialer.urls)
	}
	if stdout != "" {
		t.Errorf("没有可投的口令时 stdout 不该有结果行（那是 note 的事），实得 %q", stdout)
	}
}

// TestLiveRoomSeparatesSomeoneElseLiveFromNobodyLive 钉住门的两种"没有"。
//
// ⚠ 只报 `Active=false` 会把这两件事混成一句"没有直播"，而它们的下一步
// 完全不同：一个是等，另一个是别等了（实拍 147490「福建移动」在播）。
func TestLiveRoomSeparatesSomeoneElseLiveFromNobodyLive(t *testing.T) {
	db := liveRoomSeed(t)
	client := newLiveRoomRoutes(liveRoomGateOpen(liveRoomTabOtherLive)...)
	_, stderr, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{{}}}, db)
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("有人播但不是正主，仍是正常结果（退 0），实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stderr, "没有一间是") {
		t.Errorf("要说清是「有人在播、但不是那个主播」，实得 stderr=%q", stderr)
	}
	if !strings.Contains(stderr, "1 间在播") {
		t.Errorf("要把在播的间数说出来（它是判断依据），实得 stderr=%q", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 真的没跑成
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomGateFailureIsARunError 钉住"门那三步走不通"是**运行**错误。
//
// ⚠ 这里刻意**不照抄**原项目那条"失败即放行"：那条会让"凭据坏了"表现成
// "门开着、却一条口令也听不到"，而真正的原因（凭据）被那句放行盖住了 ——
// 排查会从"今天是不是没开播"开始，方向完全错了。
func TestLiveRoomGateFailureIsARunError(t *testing.T) {
	db := liveRoomSeed(t)
	// 一条路由都不配 ⇒ 第一个请求拿到 404 ⇒ 换 SSO token 失败。
	client := newLiveRoomRoutes()
	dialer := &liveRoomDialer{conns: []*liveRoomWSConn{{}}}

	_, stderr, code := runLiveRoomCLI(t, client, dialer, db, "--codes", liveRoomKouling)
	if code != ExitRun {
		t.Fatalf("门走不通是运行错误（退 %d），实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "门·换 SSO token") {
		t.Errorf("错误里要带步名（这条链跨四个域，不带步名分不清是哪一侧），实得 %q", stderr)
	}
	if len(dialer.urls) != 0 {
		t.Errorf("门都没开就不该拨号，实际拨了 %v", dialer.urls)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON 形状
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomJSONIsCleanAndCarriesBothBalances 钉住 `--json` 的形状。
//
// 两条判据：stdout 必须是**一份干净 JSON**（`note` 走 stderr 才做得到），
// 以及余额的**两个读数**都在 —— 只给终值的话"进账多少"就没法从输出里看出来。
func TestLiveRoomJSONIsCleanAndCarriesBothBalances(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabLive), liveRoomSixStepRoutes(10, 18, []string{liveRoomKouling})...)
	client := newLiveRoomRoutes(routes...)

	stdout, stderr, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{{}}}, db,
		"--skip-kouling", "--codes", liveRoomKouling, "--json")
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}

	var payload struct {
		LiveRoom []struct {
			Account       string   `json:"account"`
			Index         int      `json:"index"`
			GateOpen      bool     `json:"gateOpen"`
			LiveCount     int      `json:"liveCount"`
			Kouling       []string `json:"kouling"`
			Retried       bool     `json:"retried"`
			Failed        bool     `json:"failed"`
			BalanceBefore int64    `json:"balanceBefore"`
			BalanceAfter  int64    `json:"balanceAfter"`
			Gained        int64    `json:"gained"`
			Succeeded     int      `json:"succeeded"`
		} `json:"liveRoom"`
	}
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout 不是一份干净 JSON（note 混进来了？）：%v\n  stdout=%q", err, stdout)
	}
	if len(payload.LiveRoom) != 1 {
		t.Fatalf("应有一个账号的结果，实得 %d 条：%q", len(payload.LiveRoom), stdout)
	}
	got := payload.LiveRoom[0]
	if !got.GateOpen {
		t.Errorf("这次门是开的，gateOpen 应为 true：%+v", got)
	}
	if got.BalanceBefore != 10 || got.BalanceAfter != 18 || got.Gained != 8 {
		t.Errorf("两个读数都要在（10 → 18，进账 8），实得 %+v", got)
	}
	if got.Succeeded != 1 {
		t.Errorf("成功口令数应为 1，实得 %d", got.Succeeded)
	}
	if got.Failed {
		t.Errorf("这趟没失败，failed 不该为 true：%+v", got)
	}
	if len(got.Kouling) != 1 || got.Kouling[0] != liveRoomKouling {
		t.Errorf("收/投过的口令要列出来，实得 %v", got.Kouling)
	}
}

// TestLiveRoomJSONCarriesTheRawUpstreamDrawCode 钉住 `--json` 里有**原始业务码**。
//
// # 为什么这条断言值得单独写
//
// `otherCodes` 是个**集合** —— `200112`（已兑换）与任何一个我们没见过的码
// 都落进去 ⇒ 光看它**分不出抓到的是哪一个**。而未验证清单里正有一条
// 「`200112` 至今没有样本」（见 [upstream.CodeRedFlowerAlreadyDrawn]）。
// 没有 `draws[].upstreamCode`，那条**只能靠代理抓包**才能关掉 ——
// 而"要另起一套工具"正是这类核对项最容易一直拖下去的原因。
//
// 三条判据，缺一条这个字段就没被真正钉住：
//   - 原始码如实带出来（`200112` 就是 `200112`，不是"某个未分类码"）；
//   - `draws` **保序**且带口令本身（不然"哪个口令换回了这个码"无从对应）；
//   - 信封里**没有** `code` 时是 `null`，不是 `0` ——
//     "码是 0"（成功）与"根本没解出码"是两件事。
func TestLiveRoomJSONCarriesTheRawUpstreamDrawCode(t *testing.T) {
	db := liveRoomSeed(t)
	// ⚠ 三个口令各配一条响应：`200112` / `200108` / 没有 `code`。
	codes := []string{liveRoomKouling, liveRoomKoulingSecond, "国庆口令丙"}
	routes := append(liveRoomGateOpen(liveRoomTabLive),
		liveRoomSixStepRoutesDraws(10, 10, []string{
			liveRoomDrawAlreadyDrawnBody,
			liveRoomDrawInvalidBody,
			liveRoomDrawNoCodeBody,
		})...)
	client := newLiveRoomRoutes(routes...)

	stdout, stderr, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{{}}}, db,
		"--skip-kouling", "--codes", strings.Join(codes, ","), "--json")
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}

	var payload struct {
		LiveRoom []struct {
			Draws []struct {
				Kouling      string `json:"kouling"`
				UpstreamCode *int64 `json:"upstreamCode"`
			} `json:"draws"`
		} `json:"liveRoom"`
	}
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout 不是一份干净 JSON：%v\n  stdout=%q", err, stdout)
	}
	if len(payload.LiveRoom) != 1 || len(payload.LiveRoom[0].Draws) != len(codes) {
		t.Fatalf("每个口令都该有一条 draw 记录，实得 %q", stdout)
	}
	draws := payload.LiveRoom[0].Draws
	for i, want := range codes {
		if draws[i].Kouling != want {
			t.Errorf("draws 要**保序**且带上口令本身：第 %d 条应是 %q，实得 %q", i, want, draws[i].Kouling)
		}
	}
	if draws[0].UpstreamCode == nil || *draws[0].UpstreamCode != 200112 {
		t.Errorf("第一个口令的原始码应是 200112（这就是那条「至今没有样本」的判据），实得 %v",
			draws[0].UpstreamCode)
	}
	if draws[1].UpstreamCode == nil || *draws[1].UpstreamCode != 200108 {
		t.Errorf("第二个口令的原始码应是 200108，实得 %v", draws[1].UpstreamCode)
	}
	if draws[2].UpstreamCode != nil {
		t.Errorf("信封里没有 code 时该是 null（不是 0 —— 「码是 0」与「没解出码」是两件事），实得 %v",
			*draws[2].UpstreamCode)
	}
}

// TestLiveRoomJSONMarksFailure 钉住失败那条路的 JSON 也要是干净的。
//
// 退出码红了但 stdout 是半截 JSON 的话，`--json | jq` 会报语法错，
// 而真正的错因（stderr 里那句）被 jq 的报错顶掉了。
func TestLiveRoomJSONMarksFailure(t *testing.T) {
	db := liveRoomSeed(t)
	client := newLiveRoomRoutes() // 一条路由都没有 ⇒ 门必失败

	stdout, _, code := runLiveRoomCLI(t, client, &liveRoomDialer{conns: []*liveRoomWSConn{{}}}, db,
		"--codes", liveRoomKouling, "--json")
	if code != ExitRun {
		t.Fatalf("应退 %d，实得 %d", ExitRun, code)
	}
	var payload struct {
		LiveRoom []struct {
			Failed bool   `json:"failed"`
			Error  string `json:"error"`
		} `json:"liveRoom"`
	}
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("失败时 stdout 仍必须是合法 JSON：%v\n  stdout=%q", err, stdout)
	}
	if len(payload.LiveRoom) != 1 || !payload.LiveRoom[0].Failed {
		t.Fatalf("失败要如实标出来，实得 %q", stdout)
	}
	if !strings.Contains(payload.LiveRoom[0].Error, "门·换 SSO token") {
		t.Errorf("JSON 里的错误也要带步名，实得 %q", payload.LiveRoom[0].Error)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 重听一次：只在**有口令被判无效**时做
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomRetryMergesBothRounds 钉住重听那条路上三件最容易写错的事。
//
// 它们是**互相纠缠**的，所以一条用例一起钉：
//
//  1. **重听要真的再拨一次号**（`freshLiveCodes` 走的是完整的 `Collect`）；
//  2. **已经试过的口令不重投** —— 不排除的话第二轮会把甲再投一遍、
//     全换回 `200112`（已兑换），看起来像"口令坏了"；
//  3. **两轮结果要合并**，且起始余额保留第一轮的 —— 用第二轮的起始余额算，
//     "进账"会少算第一轮那段（这里 6 会变成 5）。
//
// 判据用 `10 → 16（进账 6，成功 1/2）`：这三个数只有"合并得对"时同时成立。
func TestLiveRoomRetryMergesBothRounds(t *testing.T) {
	db := liveRoomSeed(t)
	routes := liveRoomGateOpen(liveRoomTabLive)
	routes = append(routes, liveRoomCollectionRoutes(false)...)
	// 第一轮：起始 10 → 终值 10（甲被判 `200108`，没进账）。
	routes = append(routes, liveRoomSixStepRoutesDraws(10, 10, []string{liveRoomDrawInvalidBody})...)
	// 重听：整串再打一遍。
	routes = append(routes, liveRoomCollectionRoutes(false)...)
	// 第二轮：11 → 16。
	routes = append(routes, liveRoomSixStepRoutes(11, 16, []string{liveRoomKoulingSecond})...)

	client := newLiveRoomRoutes(routes...)
	dialer := newLiveRoomDialer(
		&liveRoomWSConn{reads: []string{"o",
			liveRoomDanmakuFrame(t, "小红花口令："+liveRoomKouling)}},
		&liveRoomWSConn{reads: []string{"o",
			liveRoomDanmakuFrame(t, "小红花口令："+liveRoomKoulingSecond)}},
	)

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db)
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	if len(dialer.urls) != 2 {
		t.Fatalf("重听要再拨一次号（共 2 次），实际 %d 次：%v", len(dialer.urls), dialer.urls)
	}
	if !strings.Contains(stdout, "10 → 16（进账 6，成功 1/2）") {
		t.Errorf("两轮要合并、且起始余额保留第一轮的（10 → 16，进账 6），实得 %q", stdout)
	}
	if !strings.Contains(stdout, "无效口令 "+liveRoomKouling) {
		t.Errorf("第一轮那条无效口令要说出来，实得 %q", stdout)
	}
	// ⭐ 每个口令各投**一次** —— 甲被重投的话这里会是 2。
	for _, want := range []string{liveRoomKouling, liveRoomKoulingSecond} {
		if n := client.countMatching("code=" + upstream.PercentEncode(want)); n != 1 {
			t.Errorf("口令 %q 应恰好投一次，实际 %d 次：%v", want, n, client.urls())
		}
	}
}

// TestLiveRoomSkipKoulingDoesNotRelisten 钉住 `--skip-kouling` 之后**不会**去重听。
//
// ⚠ 这一条看着像废话（"跳过收集"当然不重听），但它是一条**独立**的判据：
// 重听走的是"再收一次"，而它由 [app.RetryPlan] 单独触发 —— 少了那个
// `!a.SkipKouling`，`--skip-kouling` 会在口令被判无效时反过来去拨号，
// 与开关的名字正好相反。
//
// 拨号器**一条连接都不给**：真去拨的话它会当场报错，而这里断言的是
// "压根没拨"（`len(urls) == 0`）。
func TestLiveRoomSkipKoulingDoesNotRelisten(t *testing.T) {
	db := liveRoomSeed(t)
	routes := append(liveRoomGateOpen(liveRoomTabLive),
		liveRoomSixStepRoutesDraws(10, 10, []string{liveRoomDrawInvalidBody})...)
	client := newLiveRoomRoutes(routes...)
	dialer := newLiveRoomDialer() // 没有连接：真拨就会报错

	stdout, stderr, code := runLiveRoomCLI(t, client, dialer, db,
		"--skip-kouling", "--codes", liveRoomKouling)
	client.assertRouted(t)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q\n  stderr=%q", code, stdout, stderr)
	}
	if len(dialer.urls) != 0 {
		t.Fatalf("`--skip-kouling` 不该去重听，实际拨了 %v", dialer.urls)
	}
	if !strings.Contains(stdout, "无效口令 "+liveRoomKouling) {
		t.Errorf("口令被判无效要如实说出来（这是这个开关下唯一的结论），实得 %q", stdout)
	}
}

// TestLiveRoomSkipsDisabledAccounts 钉住停用账号不被写操作碰到。
//
// 与签到同一条纪律（`selectLiveRoomTargets` 只收启用中的）。
// 判据用"下标越界"：停用的那个不进候选表，所以 `--account 2` 落在表外 ——
// 少了那层过滤的话它会解析到停用账号并真的跑起来（退出码也就不是 2 了）。
func TestLiveRoomSkipsDisabledAccounts(t *testing.T) {
	db := liveRoomSeedWith(t,
		liveRoomSeedAccount{liveRoomTestPhone, true},
		liveRoomSeedAccount{"19900000002", false},
	)
	_, stderr, code := runLiveRoomCLI(t, nil, nil, db, "--account", "2", "--codes", liveRoomKouling)
	if code != ExitUsage {
		t.Fatalf("停用账号不进候选表 ⇒ `--account 2` 应退 %d，实得 %d\n  stderr=%q",
			ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "启用中的账号有 1 个") {
		t.Errorf("要说清候选表里只有一个（停用的没算），实得 %q", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 帮助文本
// ─────────────────────────────────────────────────────────────────────────────

// TestLiveRoomHelpShowsFlagTypesNotBacktickText 钉住帮助文本里**不许有反引号**。
//
// ⚠ pflag 会把帮助文本里**第一对反引号**之间的内容当成**参数占位符**：
// `--account` 的帮助里写了 `mcloud signin --list` 之后，help 里印出来的是
// `--account mcloud signin --list`，而正确形状是 `--account int`。
//
// ⚠⚠ 它**只**在 `--help` 的渲染里看得见 —— 任何"跑一遍命令看退出码 / 看输出"
// 的用例都发现不了（这个缺陷就是靠人肉跑 `--help` 发现的）。
// 所以判据必须**真的去要一次 help**，不能指望别处的断言顺带覆盖到。
func TestLiveRoomHelpShowsFlagTypesNotBacktickText(t *testing.T) {
	dir := t.TempDir()
	stdout, _, code := runCLI(t, "", "live-room", "--help",
		"--db", filepath.Join(dir, "state.db"),
		"--app-device-file", filepath.Join(dir, "app-device.txt"))
	if code != ExitSuccess {
		t.Fatalf("`--help` 应退 0，实得 %d\n  stdout=%q", code, stdout)
	}
	for _, want := range []string{"--account int", "--codes string", "--listen-seconds int"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("help 里应有带类型占位符的 %q，实得：\n%s", want, stdout)
		}
	}
	// 反引号被当成占位符时的**具体**形状 —— 这两条是这次真的印出来过的。
	for _, broken := range []string{"--account mcloud", "--skip-kouling --codes"} {
		if strings.Contains(stdout, broken) {
			t.Errorf("帮助文本里的反引号被 pflag 当成了占位符（印出 %q）—— 去掉反引号：\n%s",
				broken, stdout)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 口令拆分
// ─────────────────────────────────────────────────────────────────────────────

// TestSplitLiveCodesAcceptsCommonSeparators 钉住 `--codes` 的分隔符面。
//
// 四种分隔符都认（换行 / 回车 / 半角逗号 / 全角逗号 / 顿号）：
// 口令是从弹幕里抄来的，抄的人不会去想"这里该用哪个逗号"。
func TestSplitLiveCodesAcceptsCommonSeparators(t *testing.T) {
	got := app.SplitKouling("甲,乙\n丙、丁\r\n戊")
	want := []string{"甲", "乙", "丙", "丁", "戊"}
	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Fatalf("应拆成 %v，实得 %v", want, got)
	}
	// 空串与重复项都要被丢掉（重复投同一条口令只会换回 `200112`）。
	got = app.SplitKouling("甲,, 甲 ,\n")
	if strings.Join(got, "|") != "甲" {
		t.Fatalf("空串与重复项都该丢掉，实得 %v", got)
	}
}
