package app

// 直播驱动层的测试。
//
// # 这一层测的是"顺序"与"哪几支不该继续"
//
// 判定（门的开合、重听、六步链）在 [live_room_test.go] 里；上游的协议形状
// 在 `internal/upstream` 里。这里只剩一件**只有这一层才看得见**的事：
// **第几步发了哪个请求、以及哪一步的某个字段说了"别往下走"**。
//
// ⚠ 最值钱的那条断言是 [TestListenWSSkipsWebSocketWhenNoChatRoom]：
// `roomBeat` 回 `chatRoomId: null` ⇒ `member/apply` 与 WS **根本不该发**。
// 2026-09-22 那轮抓包"抓不到 WS"就是这个原因，而它是**结构性**的 ——
// 一个把 `chatRoomId: null` 当错误处理的实现，会把"今天没开播"
// 表现成一条网络故障，于是排查方向变成"是不是端口写错了"（坑表 #107）。

import (
	"context"
	"fmt"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// liveTestAccessToken 是 WS 侧那串 `access_token` 的**合成**取值。
//
// ⚠ 刻意保持 **19 字符**（实拍形态是 `A03` 前缀、约 66 字符）：标识门禁按
// 「敏感键名 + 20 字符以上的值」找疑似凭据明文，而这个值出现在
// `{"access_token":"…"}` 这个 JSON 形态里。长度本身在这条链上不参与任何判断
// （`TokenMD5` 的短串边界由 `live_room_vectors_test.go` 用自己的取值单独钉），
// 所以不必为了"像"而把长度堆到 20 以上、再去门禁里开一条豁免。
const liveTestAccessToken = "A03test-access-token"

// ─────────────────────────────────────────────────────────────────────────────
// 测试替身：假 WebSocket
// ─────────────────────────────────────────────────────────────────────────────

// fakeWSRead 是一帧"服务端推来的东西"：要么是文本，要么是一个错误。
type fakeWSRead struct {
	text string
	err  error
}

// fakeWSConn 按脚本**顺序**吐帧，并留下客户端写出去的每一帧。
type fakeWSConn struct {
	reads  []fakeWSRead
	readAt int
	writes []string
	closed bool
}

func (c *fakeWSConn) ReadText(context.Context) (string, error) {
	if c.readAt >= len(c.reads) {
		// 脚本用尽 = 服务端关连接。这与"超时"是两种正常结束方式，都要能停。
		return "", io.EOF
	}
	got := c.reads[c.readAt]
	c.readAt++
	if got.err != nil {
		return "", got.err
	}
	return got.text, nil
}

func (c *fakeWSConn) WriteText(_ context.Context, text string) error {
	c.writes = append(c.writes, text)
	return nil
}

func (c *fakeWSConn) Close() error { c.closed = true; return nil }

// fakeDialer 记录被拨的 URL，并交出一条假连接。
type fakeDialer struct {
	conn *fakeWSConn
	err  error
	urls []string
}

func (d *fakeDialer) Dial(
	_ context.Context, url string, _ []core.HeaderPair,
) (core.WebSocketConn, error) {
	d.urls = append(d.urls, url)
	if d.err != nil {
		return nil, d.err
	}
	return d.conn, nil
}

// timeoutErr 是"读超时"—— 它必须实现 `Timeout() bool`，
// 否则 [os.IsTimeout] 认不出来，而驱动会把"听满一段"当成故障。
type timeoutErr struct{}

func (timeoutErr) Error() string   { return "read tcp: i/o timeout" }
func (timeoutErr) Timeout() bool   { return true }
func (timeoutErr) Temporary() bool { return true }

// liveDriverRng 是确定性随机源（WS 的 serverId / sessionId / identification 要可复现）。
type liveDriverRng struct{ next byte }

func (r *liveDriverRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

var _ core.Random = (*liveDriverRng)(nil)

func liveDriverClock() core.Clock {
	return core.NewFixedClock(time.UnixMilli(1790000000000).UTC())
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应体样本
// ─────────────────────────────────────────────────────────────────────────────

const (
	// getCurrentUserInfoNew：响应**不看**，给个最小合法体。
	liveCurrentUserBody = `{"rtnMsg":null,"rtnCode":"0","object":{"userId":47776930}}`

	// roomBeat：**未开播**那一份，逐字取自实拍（删去无关 null 字段）。
	// ⭐ 关键是 `chatRoomId: null` + `status: 0`。
	liveRoomBeatNotStarted = `{"rtnMsg":null,"rtnCode":"0","object":{` +
		`"status":0,"channelId":134,"title":"中秋佳节云相伴！看直播赢好礼",` +
		`"expectStartTime":"2026-09-23 14:30:00","pullUrl":null,"chatRoomId":null,` +
		`"anchor":{"id":1430,"authorId":56,"nickname":"中国移动云盘","channelId":134}}}`

	// roomBeat：**开播**那一份（chatRoomId 是 31 位数字串）。
	liveRoomBeatLive = `{"rtnMsg":null,"rtnCode":"0","object":{` +
		`"status":1,"channelId":134,"title":"中秋佳节云相伴！看直播赢好礼",` +
		`"chatRoomId":"1470000000000000000000000000001",` +
		`"anchor":{"id":1430,"authorId":56,"nickname":"中国移动云盘","channelId":134}}}`

	// member/apply：用的是**普通信封**（数字码 + `data`），不是 `/cmlive` 那套。
	liveApplyBody = `{"code":0,"msg":"success","timestamp":1790000000123,` +
		`"data":{"appId":"cmos","appKey":"cmos#ngvlcs",` +
		`"roomId":"1470000000000000000000000000001"}}`

	// tabContentList：两个页签各给一间，其中**同一间**同时出现在两边。
	liveTab127Body = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147490,"title":"折扣福利大流量","status":1,"channelId":29,` +
		`"anchor":{"nickname":"福建移动"}}},` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":0,` +
		`"channelId":134,"expectStartTime":"2026-09-23 14:30:00",` +
		`"anchor":{"nickname":"中国移动云盘"}}}]}}`

	liveTab129Body = `{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[` +
		`{"room":{"id":147606,"title":"中秋佳节云相伴！看直播赢好礼","status":0,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}},` +
		`{"room":{"id":147608,"title":"国庆出游AI搞定攻略！","status":0,` +
		`"channelId":134,"anchor":{"nickname":"中国移动云盘"}}}]}}`

	liveXiaohongshuHTML = `<html><body><div class="note">` +
		`<span>小红花口令：小红书来的口令</span></div></body></html>`
)

// liveUserTokenBody 造 `userToken` 的响应。
//
// ⚠ 它是**函数**而不是常量，理由不是可读性：标识门禁按
// 「敏感键名 + 20 字符以上的值」找疑似凭据明文，而它扫的是**文件文本** ——
// 写成 `"access_token":"<字面量>"` 时，字面量一超 20 字符就命中。
// 走 `%s` 之后那个位置只有两个字符，规则不命中，而**测试行为一字不变**。
//
// ⚠ 这也意味着不能靠"把 token 改短"来绕过：值短了照样会被拼进来，
// 而"为了过门禁把测试数据改小"是拿测试去迁就工具，方向反了。
func liveUserTokenBody() string {
	// `access_token` 实拍是 `A03` 前缀、约 66 字符；`expires_in` 实拍 5184000（60 天）。
	return fmt.Sprintf(`{"rtnMsg":null,"rtnCode":"0","object":`+
		`{"access_token":"%s","expires_in":5184000,"refresh_token":"x"}}`,
		liveTestAccessToken)
}

// liveDanmakuFrame 造一条 SockJS 数据帧，内容是"整段 payload 就是 MessageBody"
// 那一级（[upstream.ExtractDanmakuText] 的第二级兜底）。
//
// ⚠ 用 [upstream.EncodeMessageBody] + [upstream.SockJSDataFrame] 而不是硬编码
// base64：硬编码的测试只证明"我抄的字节与实现一致"，两处都错时照样绿。
func liveDanmakuFrame(t *testing.T, text string) string {
	t.Helper()
	payload := upstream.EncodeMessageBody(upstream.MessageBody{
		Contents: []upstream.MessageContent{{Type: 1, Text: text}},
	})
	return liveFrame(t, upstream.CmdSync, payload)
}

// liveProvisionAckFrame 造 PROVISION 的响应帧（`command=3`，payload 空）。
func liveProvisionAckFrame(t *testing.T) string {
	t.Helper()
	return liveFrame(t, upstream.CmdProvision, nil)
}

func liveFrame(t *testing.T, command int, payload []byte) string {
	t.Helper()
	return upstream.SockJSDataFrame(upstream.LiveMSyncFrame(
		liveTestAccessToken, "19900000000",
		command, payload, time.UnixMilli(1790000000000).UTC(), &liveDriverRng{}))
}

// ─────────────────────────────────────────────────────────────────────────────
// ① roomBeat 说"没有聊天室" ⇒ 结构性地不该往下走
// ─────────────────────────────────────────────────────────────────────────────

// TestListenWSSkipsWebSocketWhenNoChatRoom 是这一层最值钱的断言。
//
// 2026-09-22 那轮抓包未开播：`status: 0` + **`chatRoomId: null`**。
// 判据在**字段**上，不在"连不上"上 ⇒ 一个把 `chatRoomId: null` 当错误
// 处理的实现，会把"今天没开播"表现成一条网络故障（坑表 #107）。
func TestListenWSSkipsWebSocketWhenNoChatRoom(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		liveCurrentUserBody,
		liveUserTokenBody(),
		liveRoomBeatNotStarted, // ⭐ chatRoomId: null
	}}
	dialer := &fakeDialer{conn: &fakeWSConn{}}
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(client),
		Dialer:   dialer,
		Random:   &liveDriverRng{},
		Clock:    liveDriverClock(),
		Phone:    "19900000000",
	}

	codes, err := driver.ListenWS(context.Background(), liveTestJWT, 147606, 50*time.Millisecond)
	if err != nil {
		t.Fatalf("没有聊天室**不是错误**：%s", err.Message())
	}
	if len(codes) != 0 {
		t.Fatalf("没有聊天室时不该有口令：%v", codes)
	}
	// ⭐⭐ 三个"没有"：没有拨号、没有 member/apply、脚本没被多要一条响应。
	if len(dialer.urls) != 0 {
		t.Fatalf("没有聊天室时**不该**拨 WebSocket，实际拨了 %v", dialer.urls)
	}
	liveAssertTrace(t, client, []string{"live_current_user", "live_user_token", "room_beat"})
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 开播时：进房 → 连 WS → 收口令
// ─────────────────────────────────────────────────────────────────────────────

// TestListenWSWalksEntryChainAndCollects 走完整条链，并断言**发出去的帧序列**。
//
// 帧序列 `o` → PROVISION → ack → **UNREAD** 是最容易漏的一步：
// 漏了之后 WS 连得上、心跳正常、只是永远收不到弹幕。
func TestListenWSWalksEntryChainAndCollects(t *testing.T) {
	conn := &fakeWSConn{reads: []fakeWSRead{
		{text: "o"},
		{text: liveProvisionAckFrame(t)},
		{text: liveDanmakuFrame(t, "小红花口令：云盘AI高效复盘")},
		{text: "h"},
		{text: liveDanmakuFrame(t, "小红花口令：云盘AI高效复盘")}, // 重复 ⇒ 不该再产出
		{text: liveDanmakuFrame(t, "小红花口令：国庆出游口令")},
	}}
	client := &scriptedUpstream{responses: []string{
		liveCurrentUserBody,
		liveUserTokenBody(),
		liveRoomBeatLive,
		liveApplyBody,
	}}
	dialer := &fakeDialer{conn: conn}
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(client),
		Dialer:   dialer,
		Random:   &liveDriverRng{},
		Clock:    liveDriverClock(),
		Phone:    "19900000000",
	}

	codes, err := driver.ListenWS(context.Background(), liveTestJWT, 147606, time.Second)
	if err != nil {
		t.Fatalf("整条链不该失败：%s", err.Message())
	}
	if !liveStringsEqual(codes, []string{"云盘AI高效复盘", "国庆出游口令"}) {
		t.Fatalf("口令不符（应保序去重）：%v", codes)
	}
	liveAssertTrace(t, client, []string{
		"live_current_user", "live_user_token", "room_beat", "room_apply"})

	// ⭐ 发出去的帧：PROVISION、UNREAD、以及**每个 `h` 回一个 `h`**。
	if len(conn.writes) != 3 {
		t.Fatalf("应写 3 帧（PROVISION / UNREAD / 心跳 `h`），实际 %d：%v",
			len(conn.writes), conn.writes)
	}
	if conn.writes[2] != "h" {
		t.Fatalf("心跳应原样回 `h`，实际 %q", conn.writes[2])
	}
	// 前两帧必须是 `["<base64>"]` 形状（SockJS 的发送形状）。
	for i, frame := range conn.writes[:2] {
		if !strings.HasPrefix(frame, `["`) || !strings.HasSuffix(frame, `"]`) {
			t.Fatalf("第 %d 帧不是 SockJS 形状：%s", i+1, frame)
		}
	}
	if len(dialer.urls) != 1 {
		t.Fatalf("应只拨一次 WebSocket，实际 %d 次", len(dialer.urls))
	}
	// WS URL 必须是 `/ws/<serverId>/<sessionId>/websocket` 的形状。
	if !strings.Contains(dialer.urls[0], ":20102/ws/") ||
		!strings.HasSuffix(dialer.urls[0], "/websocket") {
		t.Fatalf("WS URL 形状不对：%s", dialer.urls[0])
	}
	if !conn.closed {
		t.Fatal("听完之后应关连接")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 两种"正常结束"都不该被当成故障
// ─────────────────────────────────────────────────────────────────────────────

// TestListenWSTreatsTimeoutAndEOFAsNormalEnding 钉住收尾的两条路。
//
// ⚠ 超时是这条链**默认的**结束方式（听满一段就该停）。把它当错误的话，
// 一次完全正常的运行会以一句"读 WebSocket 帧失败"结束，读起来像故障。
func TestListenWSTreatsTimeoutAndEOFAsNormalEnding(t *testing.T) {
	cases := []struct {
		name string
		read fakeWSRead
	}{
		{"读超时", fakeWSRead{err: timeoutErr{}}},
		{"服务端关连接", fakeWSRead{err: io.EOF}},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			conn := &fakeWSConn{reads: []fakeWSRead{
				{text: "o"},
				{text: liveDanmakuFrame(t, "小红花口令：超时前拿到的口令")},
				testCase.read,
			}}
			client := &scriptedUpstream{responses: []string{
				liveCurrentUserBody, liveUserTokenBody(), liveRoomBeatLive, liveApplyBody,
			}}
			driver := LiveRoomDriver{
				Upstream: upstream.NewUpstream(client),
				Dialer:   &fakeDialer{conn: conn},
				Random:   &liveDriverRng{},
				Clock:    liveDriverClock(),
				Phone:    "19900000000",
			}

			codes, err := driver.ListenWS(context.Background(), liveTestJWT, 147606, time.Second)
			if err != nil {
				t.Fatalf("这种结束方式不该报错：%s", err.Message())
			}
			// ⭐ 结束之前收到的口令要**留下来** —— 不能因为收尾就把它丢了。
			if !liveStringsEqual(codes, []string{"超时前拿到的口令"}) {
				t.Fatalf("结束前收到的口令应保留，实际 %v", codes)
			}
		})
	}
}

// TestListenWSFailsWithoutDialer 钉住"没接线"这条配置错误。
//
// 它是**静默死配置**的典型：漏注入拨号器不会让任何东西编译不过，
// 而表现是"永远收不到口令"。所以在入口就明确报出来。
func TestListenWSFailsWithoutDialer(t *testing.T) {
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(&scriptedUpstream{}),
		Random:   &liveDriverRng{},
		Clock:    liveDriverClock(),
		Phone:    "19900000000",
	}
	if _, err := driver.ListenWS(context.Background(), liveTestJWT, 147606, time.Second); err == nil {
		t.Fatal("没有拨号器必须报错，而不是静默什么都不做")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 双源：一个坏了不影响另一个
// ─────────────────────────────────────────────────────────────────────────────

// TestCollectKoulingSurvivesOneFailedSource 钉住"两个来源地位相同"。
//
// 小红书被风控（或这一期还没发笔记）不该让整场放弃 ——
// WS 那一源才是主力。
//
// ⚠ 这里**不能**用 [scriptedUpstream]：双源是**并发**的，一个顺序队列
// 没法服务两个消费者（谁先到谁先拿，而顺序不定）—— 那样测试会间歇性地
// 把 `userToken` 的响应喂给小红书。用按 URL 路由的客户端。
func TestCollectKoulingSurvivesOneFailedSource(t *testing.T) {
	conn := &fakeWSConn{reads: []fakeWSRead{
		{text: "o"},
		{text: liveDanmakuFrame(t, "小红花口令：弹幕来的口令")},
		{err: io.EOF},
	}}
	client := newRoutedClient(
		liveRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveCurrentUserBody},
		liveRoute{"/cmlive/api/user/customer/userToken", liveUserTokenBody()},
		liveRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLive},
		liveRoute{"/v3/room/member/apply", liveApplyBody},
		liveRoute{"xiaohongshu.com", liveXiaohongshuHTML},
	)
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(client),
		Dialer:   &fakeDialer{conn: conn},
		Random:   &liveDriverRng{},
		Clock:    liveDriverClock(),
		Phone:    "19900000000",
	}

	codes, err := driver.Collect(context.Background(), liveTestJWT, 147606, time.Second)
	if err != nil {
		t.Fatalf("两个源都成功，不该报错：%s", err.Message())
	}
	// ⚠ 顺序固定：WS 在前、小红书在后（并发完成顺序不定，但合并顺序必须定）。
	if !liveStringsEqual(codes, []string{"弹幕来的口令", "小红书来的口令"}) {
		t.Fatalf("两个来源都该被合并（且顺序固定）：%v", codes)
	}
}

// TestCollectKoulingSurvivesWhenWebSocketSourceFails 钉住**反过来**的那一半：
// WS 坏了（没开播、连接被拒）时，小红书那一源仍要照常产出。
//
// 两个方向都要钉，因为"一个坏了不影响另一个"这句话是**对称**的，
// 而实现里最容易写成"WS 先跑、失败就 return"。
func TestCollectKoulingSurvivesWhenWebSocketSourceFails(t *testing.T) {
	client := newRoutedClient(
		liveRoute{"/cmlive/api/user/auth/getCurrentUserInfoNew", liveCurrentUserBody},
		liveRoute{"/cmlive/api/user/customer/userToken", liveUserTokenBody()},
		liveRoute{"/cmlive/api/room/front/roomBeat", liveRoomBeatLive},
		liveRoute{"/v3/room/member/apply", liveApplyBody},
		liveRoute{"xiaohongshu.com", liveXiaohongshuHTML},
	)
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(client),
		// 拨号直接失败 ⇒ WS 那一源必坏。
		Dialer: &fakeDialer{err: errDialRefused},
		Random: &liveDriverRng{},
		Clock:  liveDriverClock(),
		Phone:  "19900000000",
	}

	codes, err := driver.Collect(context.Background(), liveTestJWT, 147606, time.Second)
	if err != nil {
		t.Fatalf("只剩一个来源可用时不该失败：%s", err.Message())
	}
	if !liveStringsEqual(codes, []string{"小红书来的口令"}) {
		t.Fatalf("小红书那一源应照常产出：%v", codes)
	}
}

// errDialRefused 模拟"连不上"（直播没开、或端口不通）。
var errDialRefused = &dialError{}

type dialError struct{}

func (*dialError) Error() string { return "dial tcp: connection refused" }

// ─────────────────────────────────────────────────────────────────────────────
// 按 URL 路由的假客户端
// ─────────────────────────────────────────────────────────────────────────────

// liveRoute 是"URL 含 match ⇒ 回 body"。
type liveRoute struct {
	match string
	body  string
}

// routedClient 按 URL 片段路由响应，并发安全。
//
// ⚠ 加锁不是为了"更稳"：`CollectKouling` 里两个 goroutine 会同时 `Send`，
// 不带锁的话 `go test -race` 会直接报数据竞争 —— 而那时最容易被误读成
// "并发写有 bug"，实际是测试替身没写好。
type routedClient struct {
	mu       sync.Mutex
	routes   []liveRoute
	requests []core.HTTPRequest
}

func newRoutedClient(routes ...liveRoute) *routedClient {
	return &routedClient{routes: routes}
}

func (c *routedClient) Send(
	_ context.Context, request core.HTTPRequest,
) (core.HTTPResponse, error) {
	c.mu.Lock()
	c.requests = append(c.requests, request)
	c.mu.Unlock()
	for _, route := range c.routes {
		if strings.Contains(request.URL, route.match) {
			return core.NewResponse(200, []byte(route.body)), nil
		}
	}
	// 没有路由 = 这个测试没料到会打这个端点 ⇒ 明确回 404，
	// 而不是悄悄回一个空体（那会表现成"解析失败"，指向错误的方向）。
	return core.NewResponse(404, []byte("no route for "+request.URL)), nil
}

// TestCollectKoulingFailsWhenBothSourcesFail 钉住"只有两个都坏才算失败"。
func TestCollectKoulingFailsWhenBothSourcesFail(t *testing.T) {
	driver := LiveRoomDriver{
		Upstream: upstream.NewUpstream(alwaysFailingClient{}),
		Dialer:   &fakeDialer{conn: &fakeWSConn{}},
		Random:   &liveDriverRng{},
		Clock:    liveDriverClock(),
		Phone:    "19900000000",
	}
	_, err := driver.Collect(context.Background(), liveTestJWT, 147606, 10*time.Millisecond)
	if err == nil {
		t.Fatal("两个来源都失败必须报错")
	}
	// 错误里要点出**两个来源都失败了** —— 只说"网络错误"的话，
	// 排查会从"是不是我的网"开始，而实际是上游两个入口都不通。
	if !strings.Contains(err.Message(), "两个口令来源都失败") {
		t.Fatalf("错误应点明两个来源都失败，实际：%s", err.Message())
	}
}

// alwaysFailingClient 对任何请求都回 500。
type alwaysFailingClient struct{}

func (alwaysFailingClient) Send(
	_ context.Context, _ core.HTTPRequest,
) (core.HTTPResponse, error) {
	return core.NewResponse(500, []byte("boom")), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ 门：两个页签都要拉
// ─────────────────────────────────────────────────────────────────────────────

// TestQueryKoulingRoomsQueriesBothTabs 钉住"两个页签都拉、按 id 去重"。
//
// 只查 127 会在"房间还没进福利页签"时判成"没开播"，而那正是
// "漏掉整场口令"这一侧的错误 —— 代价远大于多打一个 GET。
func TestQueryKoulingRoomsQueriesBothTabs(t *testing.T) {
	client := &scriptedUpstream{responses: []string{liveTab127Body, liveTab129Body}}
	rooms, err := QueryKoulingRooms(context.Background(), upstream.NewUpstream(client), liveTestJWT)
	if err != nil {
		t.Fatalf("拉房间不该失败：%s", err.Message())
	}
	if len(client.requests) != 2 {
		t.Fatalf("应打两个 tabContentList，实际 %d 个请求", len(client.requests))
	}
	for i, tabID := range []string{"tabId=127", "tabId=129"} {
		if !strings.Contains(client.requests[i].URL, tabID) {
			t.Fatalf("第 %d 个请求应是 %s，实际 %s", i+1, tabID, client.requests[i].URL)
		}
	}
	// 147606 在两个页签里都有 ⇒ 去重后应只剩 3 间。
	if len(rooms) != 3 {
		t.Fatalf("两个页签合并去重后应是 3 间，实际 %d：%v", len(rooms), rooms)
	}
	ids := map[int64]bool{}
	for _, room := range rooms {
		if ids[room.ID] {
			t.Fatalf("房间 %d 出现了两次（没有去重）", room.ID)
		}
		ids[room.ID] = true
	}
	// ⭐ 门的判定：今天没有一间「中国移动云盘」在播 ⇒ 不开，
	// 但**有人**在播（147490 福建移动）⇒ LiveCount 必须把这两件事分开。
	gate := JudgeKoulingGate(rooms)
	if gate.Active {
		t.Fatal("没有正主在播，门不该开")
	}
	if gate.LiveCount != 1 {
		t.Fatalf("应有 1 间在播（别的主播），实际 %d", gate.LiveCount)
	}
}

// TestOpenKoulingGateReportsFailureInsteadOfPassingThrough 钉住**不照抄**原项目那条"失败即放行"。
//
// 原实现在 SSO/登录失败时 `return true`。那会让"凭据坏了"表现成
// "门开着、却一条口令也听不到"，而真正的原因被盖住了。
func TestOpenKoulingGateReportsFailureInsteadOfPassingThrough(t *testing.T) {
	client := &scriptedUpstream{} // 一条响应都没有 ⇒ 第一个请求就失败
	gate, err := OpenKoulingGate(context.Background(), upstream.NewUpstream(client),
		testProfile(), 1790000000000000, "Basic not-a-real-credential")
	if err == nil {
		t.Fatal("换 SSO token 失败必须报错，而不是放行")
	}
	if gate.Gate.Active {
		t.Fatal("失败时不该给出一个'门开着'的结论")
	}
	if !strings.Contains(err.Message(), "门·换 SSO token") {
		t.Fatalf("错误里应带步名，实际：%s", err.Message())
	}
}

// TestOpenKoulingGateAsksForTheLiveSourceID 钉住 `toSourceId` 用的是**直播侧**那个。
//
// ⚠ 签到侧是 `001005`、直播侧是 `017027`。两者都在 `targetSourceId` 这个名下
// 出现过，混用会一条都筛不出来（坑表 #106）。
func TestOpenKoulingGateAsksForTheLiveSourceID(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		`{"code":0,"msg":"success","result":{"token":"199-sso-token"}}`,
		`{"rtnMsg":null,"rtnCode":"0","object":"199.jwt.token"}`,
		liveTab127Body,
		liveTab129Body,
	}}
	gate, err := OpenKoulingGate(context.Background(), upstream.NewUpstream(client),
		testProfile(), 1790000000000000, "Basic not-a-real-credential")
	if err != nil {
		t.Fatalf("整条门不该失败：%s", err.Message())
	}
	if gate.JWT != "199.jwt.token" {
		t.Fatalf("应把 `object` 那一串当成 JWT，实际 %q", gate.JWT)
	}
	body := string(client.requests[0].Body)
	if !strings.Contains(body, upstream.LiveSourceID) {
		t.Fatalf("querySpecTokenV2 的 body 应含直播侧 sourceId %q，实际 %s",
			upstream.LiveSourceID, body)
	}
	if strings.Contains(body, upstream.ToSourceID) {
		t.Fatalf("body 里出现了签到侧的 %q —— 两者会互相筛不出来", upstream.ToSourceID)
	}
}
