// 直播口令：把「门 → 双源收口令 → 抽小红花」串起来的**驱动**。
//
// 判定全在 [JudgeKoulingGate] / [RetryPlan] / [RedFlowerSession] 里（纯函数，可单测）；
// 本文件只做一件纯层做不了的事：**按顺序把请求发出去**。
//
// # 为什么这一层必须存在（而不是把三步写进 CLI）
//
// 这条链的每一步都需要"上一步的结果"（SSO → JWT → 房间 → 聊天室 → token → WS），
// 而它一年里只有主播开播的那几小时能真跑一次。写进 CLI 的话，
// "哪一步该发什么"就只能靠读代码，而读代码看不出**顺序**里的坑
// （比如 `roomBeat` 回 `chatRoomId: null` 时**不该**继续发 `member/apply`）。
// 拆到这里之后，注入一个假拨号器就能把整条链在单测里走完。
package app

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// callBuilt 把"构造请求 → 发出去"之间的样板收成一处。
//
// 这一族的构造函数大多返回 `(core.HTTPRequest, *core.Error)`（因为要序列化 JSON body），
// 而每次调用的错误处理都是同一句。展开写会让驱动函数比它的逻辑长三倍，
// 而长函数里"漏掉一次错误检查"是最容易发生的一类错。
//
// `step` 进错误前缀 —— 这条链上打的是四个**不同域**的接口，
// 不带步名的错误会让人分不清是直播侧还是小红花侧。
func callBuilt(
	ctx context.Context,
	up *upstream.Upstream,
	step string,
	build func() (core.HTTPRequest, *core.Error),
) (*upstream.Envelope, *core.Error) {
	request, err := build()
	if err != nil {
		return nil, err.WithPrefix("[" + step + "] ")
	}
	envelope, err := up.Call(ctx, request)
	if err != nil {
		return nil, err.WithPrefix("[" + step + "] ")
	}
	return envelope, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 门
// ─────────────────────────────────────────────────────────────────────────────

// LiveRoomGate 是「门」的结论 + 顺带换到的直播侧凭据。
//
// 两样东西一起返回而不是只返回 `bool`：换 JWT 是**门的前两步**，
// 分开写会让"判定"与"拿凭据"各跑一遍登录 —— 而登录是这条链上唯一
// 会换掉服务端 session 的动作，跑两遍不是浪费，是**可能互相踢掉**。
type LiveRoomGate struct {
	// Gate 是判定结果（含"两种没有"的区分）。
	Gate KoulingGate
	// JWT 是直播侧凭据（`cmpassLoginByCmToken` 换来的那一串）。
	//
	// ⚠ 它与签到/兑换那串 `pc:` / `mobile:` 凭据**不是同一个东西**
	// （payload 含 `phone` / `userId`，≈30 天），别互搬。
	JWT string
	// AllRooms 是两个页签合并去重后的**全部**房间（不按主播、也不按状态筛）。
	//
	// 门用不到它（判定已经在 [Gate] 里），给它只为了一件事：
	// 界面上那一行"下一场几点"要读预告场的 `ExpectStartTime`，
	// 而预告场**必然**被门筛掉（它不是 `status == 1`）。
	// ⚠ 别让调用方拿它当"有没有在播"—— 那是 [Gate] 的活。
	AllRooms []upstream.LiveRoom
}

// LoginLiveRoom 用 SSO token 换直播侧 JWT。
func LoginLiveRoom(
	ctx context.Context, up *upstream.Upstream, cmpassToken string,
) (string, *core.Error) {
	envelope, err := callBuilt(ctx, up, "直播·登录", func() (core.HTTPRequest, *core.Error) {
		return upstream.CmpassLoginRequest(cmpassToken)
	})
	if err != nil {
		return "", err
	}
	jwt, perr := upstream.ParseCmpassLogin(envelope)
	if perr != nil {
		return "", perr.WithPrefix("[直播·登录] ")
	}
	return jwt, nil
}

// QueryKoulingRooms 拉**两个**页签的房间并合并（按房间 id 去重）。
//
// # 为什么不查 `tabList`
//
// 原项目的门是**三个请求**（`querySpecTokenV2` → `cmpassLoginByCmToken` →
// `tabContentList?tabId=127`），**没有** `tabList`。抓包文档 §5.3 的探针里
// 多了一步 `tabList`，那是**探针为了发现页签 id** 才打的，不是协议要求。
//
// 这里比原项目多打**一个** `tabContentList`（`129`），依据是 2026-09-22 实拍：
// 同一间房（147606）同时在 127 与 129 里，而下一场（147608，国庆场）
// **只在 129 里**。只查 127 会在"房间还没进福利页签"时判成"没开播"
// —— 那正是"漏掉整场口令"这一侧的错误，代价远大于多打一个 GET。
func QueryKoulingRooms(
	ctx context.Context, up *upstream.Upstream, jwt string,
) ([]upstream.LiveRoom, *core.Error) {
	tabs := []int64{upstream.LiveTabIDBenefits, upstream.LiveTabIDReplay}
	groups := make([][]upstream.LiveRoom, 0, len(tabs))
	for _, tabID := range tabs {
		step := fmt.Sprintf("直播·页签 %d", tabID)
		envelope, err := up.Call(ctx, upstream.TabContentListRequest(tabID, jwt))
		if err != nil {
			return nil, err.WithPrefix("[" + step + "] ")
		}
		rooms, perr := upstream.ParseTabContentList(envelope)
		if perr != nil {
			return nil, perr.WithPrefix("[" + step + "] ")
		}
		groups = append(groups, rooms)
	}
	return upstream.MergeRooms(groups...), nil
}

// OpenKoulingGate 走完门的三步并给出判定。
//
// # ⚠ 这里**不照抄**原项目那条"失败即放行"
//
// 原实现在 SSO / 登录失败时 `return true` 放行（注释说"避免因临时故障漏掉口令"）。
// 那条在本项目里不成立：它让"凭据坏了"表现为"门开着、却一条口令也听不到"，
// 而真正的原因（凭据）被那句放行盖住了 —— 排查会从"今天是不是没开播"开始，
// 方向完全错了。
//
// ⇒ 这里把失败**如实返回**，要不要继续由调用方决定（CLI 有 `--kouling-only`
// 这类开关，它才是"我就是要试试看"的正确表达位置）。
func OpenKoulingGate(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	timestamp int64,
	authorization string,
) (LiveRoomGate, *core.Error) {
	// ⚠ `toSourceId` 用 `017027`（**直播侧**那个），不是签到用的 `001005`。
	// 两者都在 `targetSourceId` 这个名下出现过，混用会一条都筛不出来（坑表 #106）。
	sso, err := up.AppQuerySpecToken(ctx, profile, timestamp, authorization, upstream.LiveSourceID)
	if err != nil {
		return LiveRoomGate{}, err.WithPrefix("[门·换 SSO token] ")
	}
	jwt, err := LoginLiveRoom(ctx, up, sso)
	if err != nil {
		return LiveRoomGate{}, err
	}
	rooms, err := QueryKoulingRooms(ctx, up, jwt)
	if err != nil {
		return LiveRoomGate{}, err
	}
	return LiveRoomGate{Gate: JudgeKoulingGate(rooms), JWT: jwt, AllRooms: rooms}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 源 A：WebSocket
// ─────────────────────────────────────────────────────────────────────────────

// LiveRoomDriver 是「进直播间听口令」需要的注入依赖。
//
// 收成一个结构体而不是九个参数：这条链上"谁是手机号、谁是房间号、
// 谁是聊天室号"极易搞混（三者都是长数字串），
// 参数列表越长越容易在调用点把两个位置写反 —— 而写反的表现是超时，不是报错。
type LiveRoomDriver struct {
	Upstream *upstream.Upstream
	// Dialer 是 WebSocket 拨号器（纯层只认 [core.WebSocketDialer] 契约）。
	Dialer core.WebSocketDialer
	Random core.Random
	Clock  core.Clock
	// Phone 用于 `guid.name` / `userName` / `invoker` —— 三处**同一个值**
	// （`cmlive_c_<phone>`，由上游层拼）。
	Phone string
}

// ListenWS 进直播间、连上弹幕流、听一段时间，返回听到的口令。
//
// # 返回 `(nil, nil)` 是**正常**的一支
//
// `roomBeat` 回 `chatRoomId: null` ⇒ 今天没开播 ⇒ `member/apply` 与 WS
// **根本不该发**。这是**结构性**事实，不是"连不上"（坑表 #107）：
// 判据在**字段**上，所以这里在字段那一层就返回，不去尝试连接。
//
// 把它当错误的话，"今天没开播"会表现成一条网络故障，
// 而下一轮的排查方向就变成了"是不是端口写错了"。
func (d LiveRoomDriver) ListenWS(
	ctx context.Context, jwt string, roomID int64, listenFor time.Duration,
) ([]string, *core.Error) {
	if d.Dialer == nil {
		return nil, core.NewInternal("没有注入 WebSocket 拨号器")
	}
	if d.Clock == nil || d.Random == nil {
		return nil, core.NewInternal("LiveRoomDriver 缺少 Clock 或 Random")
	}

	// ① 建立服务端 session。响应**不看**（原项目也是），但少了它
	// ② 的 `userToken` 会拿不到 token。
	if _, err := d.Upstream.Call(ctx, upstream.CurrentUserInfoRequest(jwt)); err != nil {
		return nil, err.WithPrefix("[直播·建会话] ")
	}

	// ② 取 WS 凭据。
	tokenEnvelope, err := callBuilt(ctx, d.Upstream, "直播·取 WS 凭据",
		func() (core.HTTPRequest, *core.Error) { return upstream.UserTokenRequest(jwt) })
	if err != nil {
		return nil, err
	}
	token, perr := upstream.ParseUserToken(tokenEnvelope)
	if perr != nil {
		return nil, perr.WithPrefix("[直播·取 WS 凭据] ")
	}

	// ③ 房间心跳 —— 它同时是"有没有聊天室"的唯一判据。
	beatEnvelope, err := callBuilt(ctx, d.Upstream, "直播·房间心跳",
		func() (core.HTTPRequest, *core.Error) {
			return upstream.RoomBeatRequest(upstream.LiveRoomIDString(roomID), jwt)
		})
	if err != nil {
		return nil, err
	}
	beat, perr := upstream.ParseRoomBeat(beatEnvelope)
	if perr != nil {
		return nil, perr.WithPrefix("[直播·房间心跳] ")
	}
	if beat.ChatRoomID == nil {
		return nil, nil
	}

	// ④ 进房间。
	userName := "cmlive_c_" + d.Phone
	timestamp := strconv.FormatInt(d.Clock.Now().UnixMilli(), 10)
	applyEnvelope, err := callBuilt(ctx, d.Upstream, "直播·进房间",
		func() (core.HTTPRequest, *core.Error) {
			return upstream.ApplyRoomRequest(*beat.ChatRoomID, userName, userName,
				token.AccessToken, upstream.LiveDeviceInfoHex, timestamp)
		})
	if err != nil {
		return nil, err
	}
	if _, perr := upstream.ParseApplyRoom(applyEnvelope); perr != nil {
		return nil, perr.WithPrefix("[直播·进房间] ")
	}

	// ⑤ 连 WS 并听。
	url := upstream.LiveWSURL(core.Below(d.Random, 1000), upstream.SockJSSessionID(d.Random))
	listener := upstream.NewKoulingListener(
		token.AccessToken, *beat.ChatRoomID, d.Phone, d.Random)

	wsCtx, cancel := context.WithTimeout(ctx, listenFor)
	defer cancel()

	conn, dialErr := d.Dialer.Dial(wsCtx, url, upstream.LiveWSHeaders())
	if dialErr != nil {
		return nil, core.NewNetwork("连接直播间 WebSocket 失败: " + dialErr.Error())
	}
	defer func() { _ = conn.Close() }()

	for {
		text, readErr := conn.ReadText(wsCtx)
		if readErr != nil {
			// ⚠ 超时与 `io.EOF` **都不是失败**：
			//   - 超时 = 听满了一段（这是这条链的正常结束方式）；
			//   - `io.EOF` = 服务端关连接（直播结束 / SockJS 主动断）。
			// 把它们当错误的话，一次完全正常的运行会以一句
			// "读 WebSocket 帧失败" 结束，读起来像故障。
			if errors.Is(readErr, io.EOF) || wsCtx.Err() != nil || os.IsTimeout(readErr) {
				break
			}
			return listener.Codes(), core.NewNetwork("读 WebSocket 帧失败: " + readErr.Error())
		}

		actions, frameErr := listener.OnFrame(text, d.Clock.Now())
		if frameErr != nil {
			return listener.Codes(), frameErr.WithPrefix("[直播·解析帧] ")
		}
		for _, frame := range actions.Outbound {
			if writeErr := conn.WriteText(wsCtx, frame); writeErr != nil {
				return listener.Codes(), core.NewNetwork("写 WebSocket 帧失败: " + writeErr.Error())
			}
		}
	}
	return listener.Codes(), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 源 B：小红书
// ─────────────────────────────────────────────────────────────────────────────

// FetchXiaohongshuKouling 从口令发布方的小红书主页取口令。
//
// ⚠ 返回空切片**不是错误**：主页按时间倒序，这一期可能还没发笔记。
// 与"页面被风控挡了"在解析层看起来一样（都返回空）—— 那是这一源固有的不确定性，
// 而它在设计上被容忍：两个来源**地位相同**，任一为空不影响另一个。
func FetchXiaohongshuKouling(ctx context.Context, up *upstream.Upstream) ([]string, *core.Error) {
	response, err := up.Raw(ctx, upstream.XiaohongshuProfileRequest())
	if err != nil {
		return nil, err.WithPrefix("[小红书] ")
	}
	codes, perr := upstream.ParseXiaohongshuHTML(response)
	if perr != nil {
		return nil, perr.WithPrefix("[小红书] ")
	}
	return codes, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 双源合并
// ─────────────────────────────────────────────────────────────────────────────

// Collect 是 [CollectKouling] 的方法形态（便于调用点只拿一个 driver）。
func (d LiveRoomDriver) Collect(
	ctx context.Context, jwt string, roomID int64, listenFor time.Duration,
) ([]string, *core.Error) {
	return CollectKouling(ctx, d, jwt, roomID, listenFor)
}

// CollectKouling 并发收两个来源的口令并合并。
//
// # 为什么**必须**并发
//
// WS 那一源要**听满**一段（原项目 60 秒）才有意义，而小红书只是一次 GET。
// 串行的话总耗时就是 `60s + 1s`，而其中 60 秒里网络是空闲的。
// 原项目也是 `Promise.all` 两个源。
//
// # 一个源坏了不影响另一个
//
// 两个来源**地位相同**（原项目如此）。只有**两个都坏**才算失败 ——
// 一坏一好时应当照常抽奖，而不是因为"小红书被风控了"整场放弃。
//
// 合并顺序**固定**（WS 在前、小红书在后）：并发完成顺序是不确定的，
// 而口令顺序决定先投哪一个。顺序不定会让日志每次都不一样，
// 也会让"同一个输入跑两遍结果不同"变成常态。
func CollectKouling(
	ctx context.Context,
	driver LiveRoomDriver,
	jwt string,
	roomID int64,
	listenFor time.Duration,
) ([]string, *core.Error) {
	type outcome struct {
		source string
		codes  []string
		err    *core.Error
	}
	results := make(chan outcome, 2)
	go func() {
		codes, err := driver.ListenWS(ctx, jwt, roomID, listenFor)
		results <- outcome{source: "websocket", codes: codes, err: err}
	}()
	go func() {
		codes, err := FetchXiaohongshuKouling(ctx, driver.Upstream)
		results <- outcome{source: "xiaohongshu", codes: codes, err: err}
	}()

	collected := map[string]outcome{}
	for i := 0; i < 2; i++ {
		got := <-results
		collected[got.source] = got
	}

	var failures []string
	for _, source := range []string{"websocket", "xiaohongshu"} {
		if got, ok := collected[source]; ok && got.err != nil {
			failures = append(failures, source+"："+got.err.Message())
		}
	}
	if len(failures) == 2 {
		return nil, core.NewNetwork("两个口令来源都失败了 —— " + strings.Join(failures, "；"))
	}
	return MergeKouling(collected["websocket"].codes, collected["xiaohongshu"].codes), nil
}
