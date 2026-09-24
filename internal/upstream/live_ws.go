// 直播弹幕 WebSocket 的**帧状态机**（纯层，不持有连接）。
//
// 对应原项目 `packages/core/src/tasks/live-room-kouling.ts` 的
// `getKoulingFromWebSocket`（那一大段 `ws.onMessage` 里的分支）。
//
// # 为什么状态机在纯层，连接在 io 层
//
// 这条链最难的部分**不是连上**，而是**认帧**：SockJS 的四种文本帧、
// MSync 的 command 分派、三级解码兜底、跨帧口令拼接 —— 它们全是纯函数。
// 把它们和 `net.Conn` 绑在一起的话，"收到 `a["…"]` 之后该回什么"
// 就只能靠真的开一条 WS 来验证，而 WS 只有在**主播开播那一小时**才存在。
// 拆开之后，帧序列可以用一串字面量在单测里跑完（见 live_ws_test.go）。
//
// # ⚠⚠ 最关键的一条时序：PROVISION 之后必须补发 UNREAD
//
//	服务端 → `o`                      （SockJS 打开）
//	客户端 → PROVISION(command=3)      （带 Provision payload）
//	服务端 → MSync(command=3)          （PROVISION 的响应）
//	客户端 → **UNREAD(command=1)**     ← 少了这一条，服务端**不推弹幕**
//
// 这是原实现里那句注释「关键：触发服务器推送消息」的来源，也是整条链最容易漏的
// 一步：漏了之后 WS 连得上、心跳也正常、只是永远收不到弹幕 ——
// 排查方向会指向"是不是没人在发"，而不是"少发了一帧"。
//
// # 未验证的部分（2026-09-22 未开播，`chatRoomId: null` ⇒ WS 结构性不存在）
//
// 帧的**顺序**、`command` 的取值、UNREAD 是否真的必需 —— 这三条都**没有实拍**，
// 全部来自原项目。所以本文件把"每一帧要回什么"做成**可断言的返回值**，
// 而不是直接写在连接回调里：开播后第一次抓到真帧序列时，
// 对不上的地方会以一条失败的测试出现，而不是"弹幕忽然收不到了"。
package upstream

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// SockJS 的帧前缀。
const (
	// sockJSFrameOpen 是 `o`（连接已打开）—— 收到它才发 PROVISION。
	sockJSFrameOpen = "o"
	// sockJSFrameHeartbeat 是 `h`（心跳）—— **要原样回一个 `h`**。
	//
	// 不回的话 SockJS 会在超时后主动断开，而断开的样子与"直播结束了"一样。
	sockJSFrameHeartbeat = "h"
	// sockJSFrameDataPrefix 是 `a["`（一条或多条消息的 JSON 数组）。
	sockJSFrameDataPrefix = "a["
)

// LiveWSHeaders 是 WS 升级请求要带的头。
//
// ⚠ 与 `/cmlive` 那套 HTTP 头**不是**同一组，只有 `Origin` 是共用的。
//
// UA 用 [LiveUserAgent]（iPad）—— ⭐ 2026-09-23 实拍
// （`docs/capture/app-live-ws-samples.txt` §一）证明**真实 H5 客户端在 WS 升级请求上
// 发的就是它**，且与**同一次抓包里 HTTP 侧那份逐字相同**。
// ⚠ 早先这里是**照抄原项目**的 Android Chrome UA，注释写着"这一族没有实拍（未开播）"——
// 那句现在**不成立**（已经实拍到了）。那个值既没有实拍支撑，又与同文件的
// [LiveWSInfoRequest]（一直用 iPad UA）**自相矛盾**：同一个 host 发两套 UA。
//
// ⚠⚠ 但**别把这次改动当成"修复"**：我们**没有**任何"服务端校验 UA"的判据 ——
// 实拍只能证明客户端**发了什么**，证明不了服务端**认什么**（坑表 **#136** 的教训）。
// 两个 UA 都是真实客户端在用的值 ⇒ 这次只是**抄得更准**，不是"修好了什么"。
func LiveWSHeaders() []core.HeaderPair {
	return []core.HeaderPair{
		{"User-Agent", LiveUserAgent},
		{"Origin", LiveOrigin},
	}
}

// LiveWSInfoRequest 构造 `GET https://videoim.cmcc-cs.cn:20102/ws/info`。
//
// SockJS 的探活端点。真实客户端会先打它（实拍夹具里有），拿到
// `{"cookie_needed":false,"websocket":true}` 再升级。纯协议实现**不依赖**它，
// 但打一下成本极低，而且"它回了什么"是"这个端点还活着吗"的唯一判据
// —— 出问题时不打它就少一条线索。
func LiveWSInfoRequest() core.HTTPRequest {
	return core.NewRequestGet(BaseVideoIMWS+"/ws/info").
		WithHeader("Accept", "*/*").
		WithHeader("Origin", LiveOrigin).
		WithHeader("User-Agent", LiveUserAgent)
}

// FrameActions 是"处理完一帧之后要做什么"。
//
// 做成返回值而不是回调，是为了让整条时序**可断言**：测试里喂一串帧文本，
// 比一串"发出去的帧"和"收到的口令"（见 live_ws_test.go）。
type FrameActions struct {
	// Outbound 是要按顺序写出去的 SockJS 文本帧（`h` / `["<base64>"]`）。
	Outbound []string
	// Danmaku 是这一帧解出的弹幕文本（空 = 这一帧没有文本）。
	Danmaku string
	// Codes 是这一帧**新**提取到的口令（已按全局去重）。
	Codes []string
	// Kind 说明这一帧被归成了哪一类，供日志用。
	Kind FrameKind
}

// FrameKind 是帧的归类（只用于日志与测试断言）。
type FrameKind int

const (
	// FrameIgnored 是不认识的帧（原实现是 debug 打一行然后丢掉）。
	FrameIgnored FrameKind = iota
	// FrameHeartbeat 是 `h`。
	FrameHeartbeat
	// FrameOpen 是 `o`。
	FrameOpen
	// FrameProvisionAck 是 `command == 3` 的响应帧（触发 UNREAD）。
	FrameProvisionAck
	// FrameDanmaku 是解出了文本的 SYNC/NOTICE 帧。
	FrameDanmaku
	// FrameSilent 是数据帧但解不出文本（心跳类的 SYNC、别的 command）。
	FrameSilent
	// FrameMalformed 是数据帧但 base64 / protobuf 解不开。
	FrameMalformed
)

func (k FrameKind) String() string {
	switch k {
	case FrameIgnored:
		return "ignored"
	case FrameHeartbeat:
		return "heartbeat"
	case FrameOpen:
		return "open"
	case FrameProvisionAck:
		return "provision_ack"
	case FrameDanmaku:
		return "danmaku"
	case FrameSilent:
		return "silent"
	case FrameMalformed:
		return "malformed"
	}
	return ""
}

// KoulingListener 是"进房间 → 收口令"这条链的状态机。
//
// 它**不持有连接**，也**不 sleep**：每收到一帧就调一次 [KoulingListener.OnFrame]，
// 由调用方决定什么时候停（原项目是超时或"收到一个就返回"）。
type KoulingListener struct {
	accessToken string
	chatRoomID  string
	phone       string
	rng         core.Random

	codes []string
	seen  map[string]struct{}

	// pendingHead 表示"上一帧整条是个口令头，下一帧整条就是口令"。
	pendingHead bool
	// provisionSent / unreadSent 记录两条关键帧是否已发 —— 它们各**只能发一次**。
	provisionSent bool
	unreadSent    bool
}

// NewKoulingListener 构造状态机。
//
// `chatRoomID` 是 `roomBeat` 给的 31 位聊天室 id（**不是**房间 id）：
// `guid.name` 用的是 `phone`，而 `roomId` 只进 PROVISION 的参数里 —— 两个别搞混。
func NewKoulingListener(accessToken, chatRoomID, phone string, rng core.Random) *KoulingListener {
	return &KoulingListener{
		accessToken: accessToken,
		chatRoomID:  chatRoomID,
		phone:       phone,
		rng:         rng,
		seen:        make(map[string]struct{}),
	}
}

// Codes 返回已收集到的口令（保序）。
func (l *KoulingListener) Codes() []string {
	out := make([]string, len(l.codes))
	copy(out, l.codes)
	return out
}

// ProvisionSent 报告 PROVISION 是否已发出。
func (l *KoulingListener) ProvisionSent() bool { return l.provisionSent }

// UnreadSent 报告 UNREAD 是否已发出。
func (l *KoulingListener) UnreadSent() bool { return l.unreadSent }

// OnFrame 处理一帧 SockJS 文本帧，返回要回发的帧与新口令。
//
// `now` 由调用方给（本层不取时钟）：它同时决定 `resource`（`js_<毫秒>`）、
// PROVISION 的 `deviceUuid` 与 MSync 的 8 字节时间戳 —— 三处都来自"同一刻"。
func (l *KoulingListener) OnFrame(text string, now time.Time) (FrameActions, *core.Error) {
	switch {
	case text == sockJSFrameHeartbeat:
		// ⚠ 心跳要**原样回一个 `h`**（SockJS 的被动心跳）。
		return FrameActions{Kind: FrameHeartbeat, Outbound: []string{sockJSFrameHeartbeat}}, nil

	case text == sockJSFrameOpen:
		if l.provisionSent {
			// 重复的 `o`：SockJS 重连时会再发一次，但 PROVISION 只能发一次
			// （重复发会让服务端把这条连接当成新会话）。
			return FrameActions{Kind: FrameOpen}, nil
		}
		frame, err := l.provisionFrame(now)
		if err != nil {
			return FrameActions{Kind: FrameOpen}, err
		}
		l.provisionSent = true
		return FrameActions{Kind: FrameOpen, Outbound: []string{frame}}, nil
	}

	if !strings.HasPrefix(text, sockJSFrameDataPrefix) {
		return FrameActions{Kind: FrameIgnored}, nil
	}

	messages, err := decodeSockJSData(text)
	if err != nil {
		return FrameActions{Kind: FrameMalformed}, core.NewParse("SockJS 数据帧不是合法 JSON 数组: " + err.Error())
	}

	actions := FrameActions{Kind: FrameSilent}
	for _, message := range messages {
		raw, err := base64.StdEncoding.DecodeString(message)
		if err != nil {
			actions.Kind = FrameMalformed
			continue
		}
		sync, err := DecodeMSync(raw)
		if err != nil {
			actions.Kind = FrameMalformed
			continue
		}

		// PROVISION 的响应：**必须**补发 UNREAD，否则服务端不推弹幕。
		if sync.Command == CmdProvision {
			actions.Kind = FrameProvisionAck
			if !l.unreadSent {
				frame, err := l.unreadFrame(now)
				if err != nil {
					return actions, err
				}
				l.unreadSent = true
				actions.Outbound = append(actions.Outbound, frame)
			}
			continue
		}

		danmaku, err := ExtractDanmakuText(sync.Command, sync.Payload)
		if err != nil {
			// 解不开**不算**这一帧的失败：还有下一帧。
			// 但要把它记下来 —— 只回一个空文本的话，"一直收不到弹幕"
			// 与"弹幕本来就没有"就分不开了。
			actions.Kind = FrameMalformed
			continue
		}
		if danmaku == "" {
			continue
		}
		actions.Kind = FrameDanmaku
		actions.Danmaku = danmaku

		// 跨帧拼接：上一条是"复制口令N："结尾，本条整条就是口令。
		if l.pendingHead {
			l.pendingHead = false
			trimmed := strings.TrimSpace(danmaku)
			if trimmed != "" && !isKoulingHeadFragment(trimmed) {
				actions.Codes = append(actions.Codes, l.add(trimmed)...)
			}
		}

		actions.Codes = append(actions.Codes, l.addMany(ExtractKouling(danmaku))...)

		if IsKoulingHead(danmaku) {
			l.pendingHead = true
		}
	}
	return actions, nil
}

// isKoulingHeadFragment 报告这段文本"本身还是个口令头/收尾词"，不能当口令用。
//
// 对齐原实现的 `!/(?:复制)?口令|无随机/.test(trimmed)`。
func isKoulingHeadFragment(text string) bool {
	return strings.Contains(text, "口令") || strings.Contains(text, noRandomToken)
}

func (l *KoulingListener) add(code string) []string {
	if code == "" {
		return nil
	}
	if _, ok := l.seen[code]; ok {
		return nil
	}
	l.seen[code] = struct{}{}
	l.codes = append(l.codes, code)
	return []string{code}
}

func (l *KoulingListener) addMany(codes []string) []string {
	var out []string
	for _, code := range codes {
		out = append(out, l.add(code)...)
	}
	return out
}

// resource 是 `js_<毫秒>`（PROVISION 与两条控制帧共用同一个形状）。
//
// ⚠ 每次现取（原项目就是这样）——它不是"会话 id"，而是**这一次请求的标识**。
func resourceAt(now time.Time) string {
	return fmt.Sprintf("js_%d", now.UnixMilli())
}

func (l *KoulingListener) provisionFrame(now time.Time) (string, *core.Error) {
	resource := resourceAt(now)
	payload := EncodeProvision(Provision{
		OSType:       ProvisionOSTypeJS,
		Version:      ProvisionVersion,
		NetworkType:  ProvisionNetwork,
		AppSign:      ProvisionAppSign,
		CompressType: []uint32{0},
		EncryptType:  []uint32{0},
		// `deviceUuid` 是 `String(Date.now())` —— 与 resource 同源，但**不是同一个串**
		// （少了 `js_` 前缀）。照抄。
		DeviceUUID: fmt.Sprintf("%d", now.UnixMilli()),
		DeviceName: ProvisionDeviceName,
		Resource:   resource,
		Auth:       "$t$" + l.accessToken,
		// province_code 原实现给空串 ⇒ 字段不出现。
		ProvinceCode: "",
	})
	return l.encodeFrame(CmdProvision, payload, now)
}

func (l *KoulingListener) unreadFrame(now time.Time) (string, *core.Error) {
	return l.encodeFrame(CmdUnread, nil, now)
}

// encodeFrame 把一条 MSync 报文包成 SockJS 的发送形状。
//
// ⚠ 报文本身走 [LiveMSyncFrame] 构造，**不在这里手拼结构体** ——
// 这条报文里有六项恒定字段（含那个必须显式给零值的 `pov`），
// 手拼时漏一项不会被服务端立刻拒掉，只会让报文与原客户端不同。
func (l *KoulingListener) encodeFrame(command int, payload []byte, now time.Time) (string, *core.Error) {
	raw := EncodeMSync(LiveMSyncFrame(l.accessToken, l.phone, command, payload, now, l.rng))
	// SockJS 的发送形状：一个**只含一个字符串**的 JSON 数组。
	return `["` + base64.StdEncoding.EncodeToString(raw) + `"]`, nil
}

// decodeSockJSData 解 `a["<base64>"]` 里的消息数组。
//
// # ⚠ 这里**刻意**与原实现不同
//
// 原实现是 `text.slice(3, text.lastIndexOf('"]'))` —— 只对**单条**消息成立。
// SockJS 在缓冲时会一次发多条：`a["QUJD","REVG"]`，那种帧被它切出来是
// `QUJD","REVG`，base64 解不开 ⇒ 整帧被丢。
//
// 换成按 JSON 解之后，单条的行为**逐字相同**（同一个 base64 串），
// 多条则从"丢掉"变成"都能处理"。这是一处只有收益的偏离，写在这里是为了
// 下一次有人对着原实现核代码时不会以为抄错了。
func decodeSockJSData(text string) ([]string, error) {
	var messages []string
	if err := json.Unmarshal([]byte(text[len(sockJSFrameDataPrefix)-1:]), &messages); err != nil {
		return nil, err
	}
	return messages, nil
}

// SockJSDataFrame 把一条 MSync 报文包成 SockJS **推下来**的数据帧文本（`a["<base64>"]`）。
//
// # ⚠ 别与 [KoulingListener.encodeFrame] 混起来
//
// 两者的 base64 主体完全一样，只有外层不同 —— 而那个外层正是 SockJS 的帧协议：
//
//	服务端推 → `a["<base64>"]`   （前缀 `a[`，**本函数**）
//	客户端发 → `["<base64>"]`    （无前缀，[KoulingListener.encodeFrame]）
//
// 前缀写错的表现是"帧被归成不认识的一类、安静丢掉" —— 也就是"连上了、
// 心跳也正常、就是收不到弹幕"，与漏发 UNREAD 的症状**一模一样**。
//
// 它的另一个用途是让上层（`internal/app` 的驱动层）能造出入站帧来测
// "收到帧之后该回什么"，而不必硬编码一段 base64（见 [EncodeMessageBody] 的说明）。
func SockJSDataFrame(message MSyncMessage) string {
	return sockJSFrameDataPrefix + `"` +
		base64.StdEncoding.EncodeToString(EncodeMSync(message)) + `"]`
}
