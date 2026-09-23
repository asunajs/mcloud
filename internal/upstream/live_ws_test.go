package upstream

// 直播弹幕**帧状态机**的测试（`KoulingListener`）。
//
// # 为什么这一处最该有测试
//
// 2026-09-22 那轮抓包未开播 —— `roomBeat` 回 `chatRoomId: null`
// ⇒ `member/apply` 与 WS **结构性不存在**（坑表 #107）。所以
// "帧长什么样、收到之后该回什么"这一整块**一条实拍都没有**，全部来自原项目。
//
// 纯协议实现里，这类"没有实拍、只有参照实现"的代码只有两条出路：
// 写死进连接回调（那就只能等开播才知道对不对），或者做成**可断言的返回值**
// （那就能在单测里把整条时序跑完）。这里是后者。
//
// ⇒ 下面每一条断言都是"开播后第一次抓到真帧时，对不上的地方会以
// **一条失败的测试**出现"的落点 —— 而不是"弹幕忽然收不到了"。

import (
	"encoding/base64"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 造帧的小助手
//
// ⚠ 一律用**本包自己的编码器**（[appendMessage] / [appendBytes] / [appendString]）
// 拼，不手写字节。手写字节的话，"测试通过"只证明"我抄的字节与实现一致"，
// 而两处都错时它照样绿。
// ─────────────────────────────────────────────────────────────────────────────

// wsFrameRng 是确定性随机源：帧里的 `identification` 要可复现。
type wsFrameRng struct{ next byte }

func (r *wsFrameRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

var _ core.Random = (*wsFrameRng)(nil)

func wsTestTime() time.Time { return time.UnixMilli(1790000000000).UTC() }

// wsDanmakuPayload 造 `CommSyncUL{meta: Meta{payload: MessageBody{contents:[{text}]}}}`。
//
// 字段号逐个对着 [ExtractDanmakuText] 的实现：
//
//	CommSyncUL.meta   = 1   （⚠ NOTICE 那边是 2，见下）
//	Meta.payload      = 6
//	MessageBody.contents = 4  （repeated，wire type 2）
//	Content.text      = 2
func wsDanmakuPayload(text string) []byte {
	content := appendString(nil, 2, text)
	body := appendMessage(nil, 4, content)
	meta := appendBytes(nil, 6, body)
	return appendMessage(nil, 1, meta)
}

// wsNoticePayload 与上面同形，但 `meta` 是**字段 2**。
//
// ⚠ 两个消息长得像、字段号不同 —— 抄错的表现是"NOTICE 这条路永远解不出文本"，
// 而那正好是"弹幕收不到"里最难查的一类。
func wsNoticePayload(text string) []byte {
	content := appendString(nil, 2, text)
	body := appendMessage(nil, 4, content)
	meta := appendBytes(nil, 6, body)
	return appendMessage(nil, 2, meta)
}

// wsFlatBody 造"整段 payload 直接就是 MessageBody"那一级**兜底**用的载荷。
//
// 这一级的存在本身就是"猜错过一次"的证据（原实现先写死 SYNC 那一层，
// 后来撞上解不出文本才补的兜底）—— 所以它必须单独被钉住。
func wsFlatBody(text string) []byte {
	return appendMessage(nil, 4, appendString(nil, 2, text))
}

// wsDataFrame 把一段 MSync 报文包成 SockJS 的数据帧文本。
func wsDataFrame(command int, payload []byte) string {
	raw := EncodeMSync(LiveMSyncFrame("test-access-token", "19900000000",
		command, payload, wsTestTime(), &wsFrameRng{}))
	return `a["` + base64.StdEncoding.EncodeToString(raw) + `"]`
}

// wsNewListener 造一个状态机。
func wsNewListener() *KoulingListener {
	return NewKoulingListener("test-access-token", "1470000000000000000000000000000",
		"19900000000", &wsFrameRng{})
}

// wsDecodeFrame 把一条发出去的 SockJS 帧解回 MSync，供断言用。
func wsDecodeFrame(t *testing.T, frame string) MSyncMessage {
	t.Helper()
	if !strings.HasPrefix(frame, `["`) || !strings.HasSuffix(frame, `"]`) {
		t.Fatalf("发出去的帧不是 SockJS 的 `[\"…\"]` 形状：%s", frame)
	}
	inner := frame[2 : len(frame)-2]
	raw, err := base64.StdEncoding.DecodeString(inner)
	if err != nil {
		t.Fatalf("发出去的帧不是合法 base64：%v", err)
	}
	message, derr := DecodeMSync(raw)
	if derr != nil {
		t.Fatalf("发出去的帧不是合法 MSync：%v", derr)
	}
	return message
}

// ─────────────────────────────────────────────────────────────────────────────
// 时序：`o` → PROVISION → ack → **UNREAD**
// ─────────────────────────────────────────────────────────────────────────────

// TestKoulingListenerSendsProvisionOnOpen 钉住第一帧。
func TestKoulingListenerSendsProvisionOnOpen(t *testing.T) {
	listener := wsNewListener()

	actions, err := listener.OnFrame("o", wsTestTime())
	if err != nil {
		t.Fatalf("`o` 不该出错：%s", err.Message())
	}
	if actions.Kind != FrameOpen {
		t.Fatalf("`o` 应归为 FrameOpen，实际 %v", actions.Kind)
	}
	if len(actions.Outbound) != 1 {
		t.Fatalf("`o` 之后应发且只发 1 帧，实际 %d 帧", len(actions.Outbound))
	}
	if !listener.ProvisionSent() {
		t.Fatal("PROVISION 应被记为已发")
	}

	// 发出去的必须是 PROVISION（command=3），且 payload 解得回来。
	sent := wsDecodeFrame(t, actions.Outbound[0])
	if sent.Command != CmdProvision {
		t.Fatalf("第一帧应是 PROVISION(command=3)，实际 command=%d", sent.Command)
	}
	if sent.Auth != "$t$test-access-token" {
		t.Fatalf("auth 应是 `$t$` + accessToken，实际 %q", sent.Auth)
	}
	provision, derr := DecodeProvision(sent.Payload)
	if derr != nil {
		t.Fatalf("PROVISION 的 payload 解不开：%v", derr)
	}
	if provision.OSType != ProvisionOSTypeJS {
		t.Fatalf("osType 应是 %d（OS_JS），实际 %d", ProvisionOSTypeJS, provision.OSType)
	}
	if provision.AppSign != ProvisionAppSign {
		t.Fatalf("appSign 应是 %q，实际 %q", ProvisionAppSign, provision.AppSign)
	}
	if provision.Auth != "$t$test-access-token" {
		t.Fatalf("PROVISION 的 auth 也应是 `$t$` + accessToken，实际 %q", provision.Auth)
	}
}

// TestKoulingListenerSendsProvisionOnlyOnce 钉住"重复的 `o` 不再发 PROVISION"。
//
// SockJS 重连时会再发一次 `o`。重发 PROVISION 会让服务端把这条连接当成**新会话**
// —— 而它的表现是"心跳正常、弹幕从头开始丢"。
func TestKoulingListenerSendsProvisionOnlyOnce(t *testing.T) {
	listener := wsNewListener()
	if _, err := listener.OnFrame("o", wsTestTime()); err != nil {
		t.Fatalf("第一次 `o` 出错：%s", err.Message())
	}
	actions, err := listener.OnFrame("o", wsTestTime())
	if err != nil {
		t.Fatalf("第二次 `o` 出错：%s", err.Message())
	}
	if len(actions.Outbound) != 0 {
		t.Fatalf("重复的 `o` 不该再发帧，实际 %d 帧", len(actions.Outbound))
	}
}

// TestKoulingListenerRepliesHeartbeat 钉住"心跳要**原样回一个 `h`**"。
//
// 不回的话 SockJS 超时后会主动断开，而断开的样子与"直播结束了"一模一样。
func TestKoulingListenerRepliesHeartbeat(t *testing.T) {
	listener := wsNewListener()
	actions, err := listener.OnFrame("h", wsTestTime())
	if err != nil {
		t.Fatalf("`h` 不该出错：%s", err.Message())
	}
	if actions.Kind != FrameHeartbeat {
		t.Fatalf("`h` 应归为 FrameHeartbeat，实际 %v", actions.Kind)
	}
	if len(actions.Outbound) != 1 || actions.Outbound[0] != "h" {
		t.Fatalf("`h` 应原样回一个 `h`，实际 %v", actions.Outbound)
	}
}

// TestKoulingListenerSendsUnreadAfterProvisionAck 钉住**整条链最容易漏的一步**。
//
//	服务端 → `o`
//	客户端 → PROVISION(3)
//	服务端 → MSync(command=3)      ← 本测试喂的就是这一帧
//	客户端 → **UNREAD(1)**         ← 少了它，服务端**不推弹幕**
//
// 漏了之后 WS 连得上、心跳也正常、只是永远收不到弹幕 —— 排查方向会指向
// "是不是没人在发"，而不是"少发了一帧"。
func TestKoulingListenerSendsUnreadAfterProvisionAck(t *testing.T) {
	listener := wsNewListener()
	if _, err := listener.OnFrame("o", wsTestTime()); err != nil {
		t.Fatalf("`o` 出错：%s", err.Message())
	}
	if listener.UnreadSent() {
		t.Fatal("PROVISION 的响应还没来，UNREAD 不该已经发出去")
	}

	// PROVISION 的响应：一个 command=3 的数据帧，payload 为空。
	actions, err := listener.OnFrame(wsDataFrame(CmdProvision, nil), wsTestTime())
	if err != nil {
		t.Fatalf("PROVISION 响应出错：%s", err.Message())
	}
	if actions.Kind != FrameProvisionAck {
		t.Fatalf("应归为 FrameProvisionAck，实际 %v", actions.Kind)
	}
	if len(actions.Outbound) != 1 {
		t.Fatalf("PROVISION 响应之后应发且只发 1 帧（UNREAD），实际 %d 帧", len(actions.Outbound))
	}
	sent := wsDecodeFrame(t, actions.Outbound[0])
	if sent.Command != CmdUnread {
		t.Fatalf("补发的应是 UNREAD(command=1)，实际 command=%d", sent.Command)
	}
	if !listener.UnreadSent() {
		t.Fatal("UNREAD 应被记为已发")
	}

	// 再来一次 ack：**不能**再发一次 UNREAD。
	again, err := listener.OnFrame(wsDataFrame(CmdProvision, nil), wsTestTime())
	if err != nil {
		t.Fatalf("第二次 ack 出错：%s", err.Message())
	}
	if len(again.Outbound) != 0 {
		t.Fatalf("UNREAD 只该发一次，实际又发了 %d 帧", len(again.Outbound))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 入站：从推送帧里取口令
// ─────────────────────────────────────────────────────────────────────────────

// TestKoulingListenerExtractsFromSyncFrame 钉住 SYNC 那条路。
func TestKoulingListenerExtractsFromSyncFrame(t *testing.T) {
	listener := wsNewListener()
	actions, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsDanmakuPayload("小红花口令：云盘AI高效复盘")), wsTestTime())
	if err != nil {
		t.Fatalf("SYNC 帧不该出错：%s", err.Message())
	}
	if actions.Kind != FrameDanmaku {
		t.Fatalf("应归为 FrameDanmaku，实际 %v", actions.Kind)
	}
	if !wsStringsEqual(actions.Codes, []string{"云盘AI高效复盘"}) {
		t.Fatalf("应从弹幕里取出一个口令，实际 %v", actions.Codes)
	}
	if !wsStringsEqual(listener.Codes(), []string{"云盘AI高效复盘"}) {
		t.Fatalf("状态机应记住口令，实际 %v", listener.Codes())
	}
}

// TestKoulingListenerExtractsFromNoticeFrame 钉住 NOTICE 那条路。
//
// ⚠ 它与 SYNC 的 `meta` 字段号不同（2 vs 1）。抄错的表现是"NOTICE 永远解不出文本"。
func TestKoulingListenerExtractsFromNoticeFrame(t *testing.T) {
	listener := wsNewListener()
	actions, err := listener.OnFrame(
		wsDataFrame(CmdNotice, wsNoticePayload("小红花口令：国庆出游口令")), wsTestTime())
	if err != nil {
		t.Fatalf("NOTICE 帧不该出错：%s", err.Message())
	}
	if !wsStringsEqual(actions.Codes, []string{"国庆出游口令"}) {
		t.Fatalf("NOTICE 那条路应也能取出口令，实际 %v", actions.Codes)
	}
}

// TestKoulingListenerFallsBackToFlatMessageBody 钉住**第二级兜底**。
//
// 服务端推的是哪一层没有实拍。原实现是先写死 SYNC 那一层、撞上解不出文本
// 才补的兜底 ⇒ 这一级的存在本身就是"猜错过一次"的证据。
// 只留一条路等于把那次踩坑重新踩一遍。
func TestKoulingListenerFallsBackToFlatMessageBody(t *testing.T) {
	listener := wsNewListener()
	actions, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsFlatBody("小红花口令：兜底口令")), wsTestTime())
	if err != nil {
		t.Fatalf("兜底那一级不该出错：%s", err.Message())
	}
	if !wsStringsEqual(actions.Codes, []string{"兜底口令"}) {
		t.Fatalf("整段 payload 当 MessageBody 那一级应生效，实际 %v", actions.Codes)
	}
}

// TestKoulingListenerStitchesAcrossFrames 钉住**跨帧拼接**。
//
// 弹幕长度有限，口令长的时候主播会拆成两条发：
//
//	第 1 帧：`复制口令：`        ← 整条就是一个口令头
//	第 2 帧：`云盘AI高效复盘`     ← 整条就是口令
//
// ⚠ 这条只能靠"上一帧结尾是口令头"这个状态来拼。逐帧独立解析的话，
// 第 1 帧解不出（口令头后面没东西）、第 2 帧也解不出（不含任何口令关键词）
// ⇒ **两条都丢掉**，而日志里看起来像"主播没发"。
func TestKoulingListenerStitchesAcrossFrames(t *testing.T) {
	listener := wsNewListener()

	first, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsDanmakuPayload("复制口令：")), wsTestTime())
	if err != nil {
		t.Fatalf("第 1 帧出错：%s", err.Message())
	}
	if len(first.Codes) != 0 {
		t.Fatalf("口令头本身不是口令，第 1 帧不该产出：%v", first.Codes)
	}

	second, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsDanmakuPayload("云盘AI高效复盘")), wsTestTime())
	if err != nil {
		t.Fatalf("第 2 帧出错：%s", err.Message())
	}
	if !wsStringsEqual(second.Codes, []string{"云盘AI高效复盘"}) {
		t.Fatalf("第 2 帧整条应被当成口令，实际 %v", second.Codes)
	}
}

// TestKoulingListenerStitchingRejectsHeadFragment 钉住拼接的**排除**。
//
// 下一帧如果**自己还是个口令头**（或含"无随机"），它就不是口令内容。
// 不排除的话，`口令是` 这种残句会被当成一个口令投出去 —— 换回 `200108`，
// 而日志里看不出它是怎么来的。
func TestKoulingListenerStitchingRejectsHeadFragment(t *testing.T) {
	listener := wsNewListener()
	if _, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsDanmakuPayload("复制口令：")), wsTestTime()); err != nil {
		t.Fatalf("第 1 帧出错：%s", err.Message())
	}
	second, err := listener.OnFrame(
		wsDataFrame(CmdSync, wsDanmakuPayload("复制口令：无随机")), wsTestTime())
	if err != nil {
		t.Fatalf("第 2 帧出错：%s", err.Message())
	}
	if len(second.Codes) != 0 {
		t.Fatalf("下一帧自己还是口令头，不该被当成口令：%v", second.Codes)
	}
}

// TestKoulingListenerDeduplicatesCodes 钉住全局去重。
//
// 同一个口令会被**反复刷**（主播喊、观众复读）。不去重的话同一条口令会投十几次，
// 每次都换回 `200112`，而"无效口令 ⇒ 重听一次"那条逻辑会被自己触发的噪声带偏。
func TestKoulingListenerDeduplicatesCodes(t *testing.T) {
	listener := wsNewListener()
	payload := wsDanmakuPayload("小红花口令：云盘AI高效复盘")

	first, _ := listener.OnFrame(wsDataFrame(CmdSync, payload), wsTestTime())
	second, _ := listener.OnFrame(wsDataFrame(CmdSync, payload), wsTestTime())

	if !wsStringsEqual(first.Codes, []string{"云盘AI高效复盘"}) {
		t.Fatalf("第一次应产出：%v", first.Codes)
	}
	if len(second.Codes) != 0 {
		t.Fatalf("第二次不该再产出同一条口令：%v", second.Codes)
	}
	if len(listener.Codes()) != 1 {
		t.Fatalf("状态机里应只有 1 条，实际 %v", listener.Codes())
	}
}

// TestKoulingListenerIgnoresUnknownAndMalformedFrames 钉住三类"不是弹幕"的帧。
//
// ⚠ 关键的一条：**它们都不该返回错误**。WS 的帧流里混杂着各种控制帧，
// 把任何一个不认识的东西当错误 ⇒ 整条监听在收到第一个新类型帧时就断掉，
// 而表现是"听着听着就不出声了"。
func TestKoulingListenerIgnoresUnknownAndMalformedFrames(t *testing.T) {
	cases := []struct {
		name  string
		frame string
		kind  FrameKind
	}{
		{"不认识的前缀", `c[2010,"x"]`, FrameIgnored},
		{"空的 JSON 数组", `a[]`, FrameSilent},
		{"base64 不是合法数据", `a["!!!not-base64!!!"]`, FrameMalformed},
		{"base64 合法但不是 protobuf", `a["` + base64.StdEncoding.EncodeToString([]byte{0xff, 0xff, 0xff}) + `"]`, FrameMalformed},
		{"心跳类的 SYNC（没有 payload）", wsDataFrame(CmdSync, nil), FrameSilent},
		{"别的 command", wsDataFrame(9, wsDanmakuPayload("小红花口令：不该被解出来")), FrameSilent},
	}
	for _, testCase := range cases {
		t.Run(testCase.name, func(t *testing.T) {
			listener := wsNewListener()
			actions, err := listener.OnFrame(testCase.frame, wsTestTime())
			if err != nil {
				t.Fatalf("这一帧不该返回错误：%s", err.Message())
			}
			if actions.Kind != testCase.kind {
				t.Fatalf("应归为 %v，实际 %v", testCase.kind, actions.Kind)
			}
			if len(actions.Codes) != 0 {
				t.Fatalf("不该产出任何口令：%v", actions.Codes)
			}
		})
	}
}

// TestKoulingListenerHandlesMultipleMessagesInOneFrame 钉住"一帧多条消息"。
//
// ⚠ 这里**刻意与原实现不同**：原实现是
// `text.slice(3, text.lastIndexOf('"]'))` —— 只对单条消息成立。
// SockJS 在缓冲时会一次发多条，那种帧被它切出来是 `QUJD","REVG`，base64 解不开
// ⇒ 整帧被丢。换成按 JSON 解之后，单条行为逐字相同，多条从"丢掉"变成"都能处理"。
func TestKoulingListenerHandlesMultipleMessagesInOneFrame(t *testing.T) {
	first := wsDataFrame(CmdSync, wsDanmakuPayload("小红花口令：第一条口令"))
	second := wsDataFrame(CmdSync, wsDanmakuPayload("小红花口令：第二条口令"))
	// 把两条的 base64 串拼成一个 JSON 数组。
	multi := `a[` + first[2:len(first)-1] + `,` + second[2:len(second)-1] + `]`

	listener := wsNewListener()
	actions, err := listener.OnFrame(multi, wsTestTime())
	if err != nil {
		t.Fatalf("多消息帧不该出错：%s", err.Message())
	}
	if !wsStringsEqual(actions.Codes, []string{"第一条口令", "第二条口令"}) {
		t.Fatalf("一帧里的多条消息都该被处理，实际 %v", actions.Codes)
	}
}

func wsStringsEqual(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
