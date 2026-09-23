// 直播侧（`/cmlive` + `videoim`）的上游接口。
//
// 对应原项目 `packages/core/src/apis/live-room-chat.ts`。
//
// # ⚠⚠ 这一族用的是**第三套响应约定**
//
//	/ycloud/signin/...      → `{code: 0, msg, result}`        数字码 + result
//	/cmlive/api/...         → `{rtnCode: "0", rtnMsg, object}` **字符串码** + object
//	videoim .../member/apply → `{code: 0, msg, data, timestamp}` 数字码 + data
//
// 三套都在同一条链路上，**不能共用解析**。最要命的是 `/cmlive` 那一套：
// [Envelope.Payload] 只认 `data` / `result`，喂给它 `/cmlive` 的响应会**静默返回
// `(nil, false)` 且不报错** —— 表现为"登录成功却取不到 token"（坑表 #108）。
// 所以这里单独给一个 [CmliveResult]。
//
// # `rtnCode` 刻意**只认字符串**
//
// 有"顺手把数字也转成字符串"的写法（[Envelope.Code] 那种）。这里**不**这么干：
// `rtnCode` 是字符串这件事本身就是这一族的**身份特征**，把类型差异抹平之后，
// "上游把这一族改成了数字码"这个信号就再也不会浮出来了 ——
// 而那正是最该立刻看见的一类协议变化。
package upstream

import (
	"encoding/hex"
	"fmt"
	"strconv"

	"github.com/catlair/mcloud/internal/core"
)

// 直播侧基址与常量。
const (
	// BaseCmlive 是 H5 直播页与 `/cmlive/api/...` 的入口。
	BaseCmlive = "https://v.cmcc-cs.cn/cmlive"
	// BaseVideoIM 是 IM 的 HTTPS 入口（`member/apply` / `member/quit`）。
	//
	// ⚠ 端口 **20103**，与 WS 的 20102 **不是同一个** —— 写错端口拿到的是超时，
	// 不是错误码，很难往"端口写错了"上想。
	BaseVideoIM = "https://videoim.cmcc-cs.cn:20103"
	// BaseVideoIMWS 是 IM 的 WebSocket 入口（SockJS）。
	BaseVideoIMWS = "wss://videoim.cmcc-cs.cn:20102"

	// LiveChannelID 是移动云盘频道的 id。
	//
	// ⚠ 它**只是查询参数**，不是过滤条件：实拍（docs/capture/app-live-redflower-samples.txt §5.3）
	// 里 `?channelId=134` 拉回来的房间，`channelId` 有 `29 / 51 / 243 / 134` 四种 ——
	// 那是**房间的归属**，不是"我拉的是哪个频道"。拿它当过滤条件会一间都筛不出来。
	LiveChannelID = "134"
	// LiveSourceID 是直播房间页 URL 上的 `targetSourceId`（**只在直播侧**）。
	//
	// ⚠ 别拿它当全局过滤器：小红花那一族的 `targetSourceId` 是 `001005`
	// （见 [ToSourceID]），用 `017027` 去筛 `/ycloud` 侧会一条都筛不出来（坑表 #106）。
	LiveSourceID = "017027"

	// KoulingAnchor 是**会发口令**的那个主播的昵称。
	//
	// ⚠ 判据是**精确相等**，不是包含：实拍那张表里还有「福建移动」「移动心选」「移动臻选」
	// 三个昵称，`status===1` 的房间里就有「福建移动」⇒ **只看 `status` 会误判**，
	// 真正干活的是这个名字。主播改名即失效（原项目如此）。
	KoulingAnchor = "中国移动云盘"

	// LiveTabIDBenefits 是「盘盘福利」页签。
	LiveTabIDBenefits int64 = 127
	// LiveTabIDReplay 是「精彩回放」页签。
	//
	// ⚠ 原项目**只查 127**。这里两个都查，依据是 2026-09-22 的实拍：
	// 同一间房（147606）**同时**出现在 127 与 129 里，而下一场（147608，国庆场）
	// **只在 129 里**。⇒ 只查 127 会在"房间还没进福利页签"时判成"没开播"，
	// 而那正是"漏掉整场口令"这一侧的错误（代价远大于多打一个 GET）。
	LiveTabIDReplay int64 = 129

	// LiveRoomStatusNotStarted 是 `status = 0`：**未开播（预告）**。
	//
	// 三档语义由 2026-09-22 实拍钉死（§5.3）：`1` 直播中 / `6` 已结束 / `0` 未开播。
	// 原项目只记了前两档，`0` 是这次补上的 —— 而它恰好就是"今天抓不到 WS"的原因。
	LiveRoomStatusNotStarted int64 = 0
	// LiveRoomStatusLive 是 `status = 1`：直播中。
	LiveRoomStatusLive int64 = 1
	// LiveRoomStatusEnded 是 `status = 6`：已结束（回放）。
	LiveRoomStatusEnded int64 = 6

	// LiveUserAgent 是直播 H5 的 UA（iPad App 内嵌 WebView 的原样值）。
	LiveUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_5 like Mac OS X) AppleWebKit/605.1.15 " +
		"(KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"
	// LiveOrigin 是 `/cmlive` 请求的 `Origin`。
	LiveOrigin = "https://v.cmcc-cs.cn"
	// LiveApplyVersion 是 `member/apply` 的 `Version` 头。
	//
	// ⚠ **未实拍**：抓包里这个头是 `<REDACTED>`（fixture 脱敏时连带把版本号一起遮了）。
	// 取值来自原项目 `apis/live-room-chat.ts` 的字面量 `v23.0.2`，与
	// [ProvisionVersion] 的 `23.0.2` 对得上（只差 `v` 前缀）。**这是本文件里
	// 唯一一个"取自原项目而非实拍"的常量**，开播后第一次抓包要回来核对。
	LiveApplyVersion = "v23.0.2"
)

// LiveRoomReferer 拼直播间页的 `Referer`。
//
// 真实客户端的这一串里还有一个 `cmpassToken=<…>`（那是进房间时带的凭据）。
// 纯协议实现不重复带它 —— `Referer` 在服务端不参与鉴权（鉴权走 `Authorization` 头），
// 而把一个会过期的凭据拼进 URL 只会让日志里多一处凭据。
func LiveRoomReferer(roomID string) string {
	return fmt.Sprintf("%s/live/room?id=%s&cmliveChannelId=%s&targetSourceId=%s",
		BaseCmlive, PercentEncode(roomID), LiveChannelID, LiveSourceID)
}

// ─────────────────────────────────────────────────────────────────────────────
// `/cmlive` 的响应信封
// ─────────────────────────────────────────────────────────────────────────────

// CmliveResult 是 `/cmlive/api/...` 的统一响应：`{rtnCode, rtnMsg, object}`。
type CmliveResult struct {
	rtnCode string
	rtnMsg  *string
	object  any
	hasObj  bool
}

// ParseCmlive 从响应体解析 `/cmlive` 的信封。
//
// ⚠ `rtnCode` **必须是字符串**：它可能是数字是这一族的协议特征，抹平类型等于
// 把"上游换了一套码"这件事藏起来（见文件头）。
func ParseCmlive(envelope *Envelope) (CmliveResult, *core.Error) {
	raw, ok := envelope.Raw().(map[string]any)
	if !ok {
		return CmliveResult{}, core.NewParse("/cmlive 响应不是对象")
	}
	code, ok := raw["rtnCode"].(string)
	if !ok {
		return CmliveResult{}, core.NewParse(
			"/cmlive 响应的 rtnCode 不是字符串（这一族用字符串码，见 upstream/live_room.go 文件头）")
	}
	out := CmliveResult{rtnCode: code}
	if msg, ok := raw["rtnMsg"].(string); ok {
		out.rtnMsg = &msg
	}
	if value, ok := raw["object"]; ok {
		out.object = value
		out.hasObj = true
	}
	return out, nil
}

// Code 返回 `rtnCode` 原文（字符串）。
func (r CmliveResult) Code() string { return r.rtnCode }

// OK 报告业务是否成功。
//
// 判据是**与 `"0"` 相等**，不是"能不能转成数字"：`-9999`（`getCmPassToken` 实测值）
// 与 `"0"` 在字符串意义上就不同，而它转数字也是负数 —— 但真正要避免的是
// 把某个将来出现的非数字码（如 `"AUTH_FAIL"`）静默当成失败之外的东西。
func (r CmliveResult) OK() bool { return r.rtnCode == "0" }

// Object 返回 `object` 原文。
func (r CmliveResult) Object() (any, bool) { return r.object, r.hasObj }

// Message 返回 `rtnMsg`（可能为空串）。
func (r CmliveResult) Message() string {
	if r.rtnMsg == nil {
		return ""
	}
	return *r.rtnMsg
}

// EnsureOK 业务失败时返回错误，成功时给出 `object`。
//
// `rtnMsg` 只进 [core.Error.Message]（内部日志），不进对外文案 ——
// 上游的原文里常回显凭据（同 [errorFromStatus] 的理由）。
func (r CmliveResult) EnsureOK() (any, *core.Error) {
	if r.OK() {
		return r.object, nil
	}
	if msg := r.Message(); msg != "" {
		return nil, core.NewBadRequest(fmt.Sprintf("直播接口返回 rtnCode=%s: %s", r.rtnCode, msg))
	}
	return nil, core.NewBadRequest(fmt.Sprintf("直播接口返回 rtnCode=%s", r.rtnCode))
}

// ─────────────────────────────────────────────────────────────────────────────
// 公共头
// ─────────────────────────────────────────────────────────────────────────────

// WithCmliveHeaders 给 `/cmlive` 的请求补上头。
//
// 逐个对应 2026-09-17 抓包实测的头集合（`test/fixtures/http/直播716.json`）。
// **不含** `Cookie` / `Sec-Fetch-*` / `Connection`：前三者是浏览器自己产生的，
// 纯协议实现里既无 cookie jar 也不受 fetch 元数据约束。
//
// `jwt` 为 nil 表示这一步还没有凭据（只有 `cmpassLoginByCmToken` 是这样）。
func WithCmliveHeaders(request core.HTTPRequest, jwt *string) core.HTTPRequest {
	request = request.
		WithHeader("Accept", "application/json, text/plain, */*").
		WithHeader("X-Requested-With", "XMLHttpRequest").
		WithHeader("Accept-Language", "zh-CN,zh-Hans;q=0.9").
		WithHeader("Origin", LiveOrigin).
		WithHeader("User-Agent", LiveUserAgent)
	if jwt != nil {
		// 直播侧的凭据是**独立一套 JWT**（payload 含 `phone` / `userId`，≈30 天），
		// 不是签到/兑换那串 `pc:` / `mobile:` 凭据 —— 别把两者互搬。
		request = request.WithHeader("Authorization", *jwt)
	}
	return request
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录链
// ─────────────────────────────────────────────────────────────────────────────

// CmpassLoginRequest 构造 `POST /cmlive/api/user/auth/cmpassLoginByCmToken`。
//
// 成功时 `object` 是**一串 JWT 本身**（不是包着它的对象）—— 实拍如此（§5.3）。
func CmpassLoginRequest(cmpassToken string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/api/user/auth/cmpassLoginByCmToken", BaseCmlive)).
		WithJSONBody(map[string]any{"cmpassToken": cmpassToken})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithCmliveHeaders(request, nil), nil
}

// ParseCmpassLogin 取 `cmpassLoginByCmToken` 响应里的 JWT。
func ParseCmpassLogin(envelope *Envelope) (string, *core.Error) {
	result, err := ParseCmlive(envelope)
	if err != nil {
		return "", err
	}
	object, err := result.EnsureOK()
	if err != nil {
		return "", err
	}
	jwt, ok := object.(string)
	if !ok || jwt == "" {
		return "", core.NewParse("cmpassLoginByCmToken 的 object 不是非空字符串（应当就是那串 JWT）")
	}
	return jwt, nil
}

// CurrentUserInfoRequest 构造 `GET /cmlive/api/user/auth/getCurrentUserInfoNew`。
//
// 它的作用只是**建立服务端 session**（原项目连响应都不看），但少了它
// 后面的 `userToken` 会拿不到 token。
func CurrentUserInfoRequest(jwt string) core.HTTPRequest {
	return WithCmliveHeaders(
		core.NewRequestGet(fmt.Sprintf("%s/api/user/auth/getCurrentUserInfoNew", BaseCmlive)), &jwt)
}

// LiveUserToken 是 `userToken` 的解析结果 —— 后面开 WS 要用的那一串。
type LiveUserToken struct {
	// AccessToken 是 WS 的 `auth`（原实现拼成 `"$t$" + access_token`）。
	//
	// 实测形态：`A03` 前缀、约 66 字符（原项目注释如此；本仓库的实拍是脱敏值）。
	AccessToken string
	// ExpiresIn 是有效期秒数。实拍 `5184000`（= 60 天）。
	ExpiresIn int64
}

// UserTokenRequest 构造 `POST /cmlive/api/user/customer/userToken`（body 是空对象）。
func UserTokenRequest(jwt string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/api/user/customer/userToken", BaseCmlive)).
		WithJSONBody(map[string]any{})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithCmliveHeaders(request, &jwt), nil
}

// ParseUserToken 解析 `userToken`。
func ParseUserToken(envelope *Envelope) (LiveUserToken, *core.Error) {
	result, err := ParseCmlive(envelope)
	if err != nil {
		return LiveUserToken{}, err
	}
	object, err := result.EnsureOK()
	if err != nil {
		return LiveUserToken{}, err
	}
	if object == nil {
		return LiveUserToken{}, core.NewParse("userToken 响应没有 object")
	}
	token, err := RequireStr(object, "access_token")
	if err != nil {
		return LiveUserToken{}, err
	}
	return LiveUserToken{AccessToken: token, ExpiresIn: intAt(object, "expires_in")}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 频道与房间
// ─────────────────────────────────────────────────────────────────────────────

// TabListRequest 构造 `GET /cmlive/api/channel/channel/tabList?channelId=134`。
func TabListRequest(jwt string) core.HTTPRequest {
	return WithCmliveHeaders(
		core.NewRequestGet(fmt.Sprintf("%s/api/channel/channel/tabList?channelId=%s",
			BaseCmlive, LiveChannelID)), &jwt)
}

// TabContentListRequest 构造 `GET /cmlive/api/channel/channel/tabContentList?...`。
//
// 查询参数顺序与实拍逐字一致（`tabId` → `contentType` → `pageNo` → `channelId`）。
func TabContentListRequest(tabID int64, jwt string) core.HTTPRequest {
	return WithCmliveHeaders(
		core.NewRequestGet(fmt.Sprintf(
			"%s/api/channel/channel/tabContentList?tabId=%d&contentType=0&pageNo=1&channelId=%s",
			BaseCmlive, tabID, LiveChannelID)), &jwt)
}

// LiveRoom 是 `tabContentList` 里的一间房。
//
// 只保留**判据用得到**的字段。上游给的还有 `coverUrl` / `pullUrl` / `uv` 等十几个，
// 全搬进来只会让"哪些字段在参与判断"变得看不出来。
type LiveRoom struct {
	ID int64
	// Title 是房间标题。
	Title string
	// Status 取 [LiveRoomStatusLive] / [LiveRoomStatusEnded] / [LiveRoomStatusNotStarted]。
	Status int64
	// ChannelID 是**房间的归属频道**（不是"我拉的是哪个频道"，见 [LiveChannelID]）。
	ChannelID int64
	// ExpectStartTime 是预告开播时间（`2026-09-23 14:30:00` 形态的**本地**字符串）。
	ExpectStartTime string
	AnchorID        int64
	AuthorID        int64
	// AnchorNickname 是主播昵称 —— 门的**真正判据**（见 [KoulingAnchor]）。
	AnchorNickname string
}

// ParseTabContentList 解析 `tabContentList`。
//
// `dataList[].room` 里嵌套一层 —— 别按 `dataList[].id` 找房间，
// 那一层是 `type` / `activity` / `video` 的包装。
func ParseTabContentList(envelope *Envelope) ([]LiveRoom, *core.Error) {
	result, err := ParseCmlive(envelope)
	if err != nil {
		return nil, err
	}
	object, err := result.EnsureOK()
	if err != nil {
		return nil, err
	}
	obj, ok := object.(map[string]any)
	if !ok {
		return nil, core.NewParse("tabContentList 的 object 不是对象")
	}
	rawList, ok := obj["dataList"].([]any)
	if !ok {
		// `dataList: null` 是合法的"这个页签没有房间"，不是解析错误。
		return nil, nil
	}
	out := make([]LiveRoom, 0, len(rawList))
	for _, item := range rawList {
		entry, ok := item.(map[string]any)
		if !ok {
			continue
		}
		room, ok := entry["room"].(map[string]any)
		if !ok {
			continue
		}
		out = append(out, LiveRoom{
			ID:              intAt(room, "id"),
			Title:           strAt(room, "title"),
			Status:          intAt(room, "status"),
			ChannelID:       intAt(room, "channelId"),
			ExpectStartTime: strAt(room, "expectStartTime"),
			AnchorID:        intAt(room, "anchorId"),
			AuthorID:        intAt(room, "authorId"),
			AnchorNickname:  nestedStrAt(room, "anchor", "nickname"),
		})
	}
	return out, nil
}

// KoulingRooms 从房间列表里挑出**会发口令**的那些。
//
// 两个条件**都要**（实拍 §5.3 那张表钉死）：
//   - `status == 1`（直播中）—— 单看它不够：同一页里有「福建移动」在播；
//   - `AnchorNickname == "中国移动云盘"` —— **这个才真正干活**。
func KoulingRooms(rooms []LiveRoom) []LiveRoom {
	out := make([]LiveRoom, 0, len(rooms))
	for _, room := range rooms {
		if room.Status == LiveRoomStatusLive && room.AnchorNickname == KoulingAnchor {
			out = append(out, room)
		}
	}
	return out
}

// AnchorRooms 从房间列表里挑出「那个主播」名下的那些，**不限状态**。
//
// 与 [KoulingRooms] 只差状态那一档，而用途完全不同：
//   - 门的判据要"现在值不值得去听" ⇒ 只算 `status == 1`；
//   - 界面上那一行要"下一场几点、上一场是否已结束" ⇒ 预告（`0`）与
//     回放（`6`）都得留着，因为 `expectStartTime` 只在预告那一档有用。
//
// ⚠ 别拿它当门用：把预告与回放算进"在播"会让界面在没开播时喊去打弹幕，
// 而那一趟必然空手而归。
func AnchorRooms(rooms []LiveRoom) []LiveRoom {
	out := make([]LiveRoom, 0, len(rooms))
	for _, room := range rooms {
		if room.AnchorNickname == KoulingAnchor {
			out = append(out, room)
		}
	}
	return out
}

// MergeRooms 按房间 id 去重合并多个页签的结果（先出现的赢）。
//
// 两个页签会返回**同一间房**（实拍：147606 同时在 127 与 129 里），
// 不去重就会对同一间房做两次 `roomBeat` + 两条 WS。
func MergeRooms(groups ...[]LiveRoom) []LiveRoom {
	seen := make(map[int64]struct{})
	var out []LiveRoom
	for _, group := range groups {
		for _, room := range group {
			if _, ok := seen[room.ID]; ok {
				continue
			}
			seen[room.ID] = struct{}{}
			out = append(out, room)
		}
	}
	return out
}

// RoomBeatRequest 构造 `POST /cmlive/api/room/front/roomBeat`。
//
// # 两个字段都是**字符串**（实拍）
//
// `{"channelId":"134","roomId":"146316","sourceApp":0}` —— 原项目传的是**数字**
// `{roomId: 146316, channelId: 134}`。照实拍走：`roomId` 本来就是 31 位的大整数，
// 过一遍 JS 的 Number 有精度风险，客户端把它写成字符串是**对的**。
//
// # 它同时是"这间房有没有聊天室"的唯一判据
//
// 未开播时 `object.chatRoomId` 是 `null` ⇒ `member/apply` 与 WS **根本不会发**
// —— 这是结构性事实，不是"抓漏了"（坑表 #107）。
func RoomBeatRequest(roomID, jwt string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/api/room/front/roomBeat", BaseCmlive)).
		WithJSONBody(map[string]any{
			"roomId":    roomID,
			"channelId": LiveChannelID,
			"sourceApp": 0,
		})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithCmliveHeaders(request, &jwt), nil
}

// RoomBeat 是 `roomBeat` 的解析结果。
type RoomBeat struct {
	Status          int64
	Title           string
	ChannelID       int64
	ExpectStartTime string
	AnchorNickname  string
	// ChatRoomID 是聊天室 id（31 位数字串）。
	//
	// nil = 上游给的是 `null`，即**没有聊天室** ⇒ WS 与 `member/apply` 都不该发。
	// 用指针而不是空串，是为了让"没有"与"有但是空"在类型上就分得开。
	ChatRoomID *string
}

// ParseRoomBeat 解析 `roomBeat`。
func ParseRoomBeat(envelope *Envelope) (RoomBeat, *core.Error) {
	result, err := ParseCmlive(envelope)
	if err != nil {
		return RoomBeat{}, err
	}
	object, err := result.EnsureOK()
	if err != nil {
		return RoomBeat{}, err
	}
	if object == nil {
		return RoomBeat{}, core.NewParse("roomBeat 响应没有 object")
	}
	obj, ok := object.(map[string]any)
	if !ok {
		return RoomBeat{}, core.NewParse("roomBeat 的 object 不是对象")
	}
	out := RoomBeat{
		Status:          intAt(obj, "status"),
		Title:           strAt(obj, "title"),
		ChannelID:       intAt(obj, "channelId"),
		ExpectStartTime: strAt(obj, "expectStartTime"),
		AnchorNickname:  nestedStrAt(obj, "anchor", "nickname"),
	}
	if text, ok := obj["chatRoomId"].(string); ok && text != "" {
		out.ChatRoomID = &text
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 进房间：member/apply
// ─────────────────────────────────────────────────────────────────────────────

// TimestampSuffix10 取时间戳字符串的**末 10 位**（两个签名头都用它）。
//
// 长度不足 10 时原样返回 —— 对齐 JS 的 `String.prototype.slice(-10)`：
// 它对短串是"整串"，不是错误。别在这里加长度校验：那会把一次协议差异
// 变成一个"参数错误"，而真正该看见的是签名对不上。
func TimestampSuffix10(timestamp string) string {
	if len(timestamp) <= 10 {
		return timestamp
	}
	return timestamp[len(timestamp)-10:]
}

// SystemMD5 是 `member/apply` 的 `systemMd5` 头。
//
//	md5(deviceInfo 的 48 字节 ‖ utf8(timestamp 末 10 位))
//
// ⚠ 第一段是**解码后的字节**（48 字节），不是那 96 个 hex 字符 ——
// 传错的表现是"签名不对"，而它看起来和"算法抄错了"一模一样。
func SystemMD5(deviceInfoHex, timestamp string) (string, *core.Error) {
	deviceInfo, err := hex.DecodeString(deviceInfoHex)
	if err != nil {
		return "", core.NewBadRequest("deviceInfo 不是合法 hex: " + err.Error())
	}
	payload := append(deviceInfo, []byte(TimestampSuffix10(timestamp))...)
	return core.MD5Hex(payload), nil
}

// TokenMD5 是 `member/apply` 的 `tokenMd5` 头。
//
//	md5("Bearer " + token[0:10] + ts 末 10 位 + token[10:])
//
// ⚠ 它不是"对 token 求哈希"：时间戳被**插在 token 的第 10 个字符之后**。
// 这个形状只能照抄 —— 任何"看起来更合理"的写法（比如 `md5(token + ts)`）
// 都会得到一个长度正确、但服务端不认的签名。
//
// `token[0:10]` 在 JS 里按 UTF-16 码元切；access_token 是 ASCII（`A03` 前缀），
// 所以按字节切等价。**短于 10 字符时** JS 的 `slice(0,10)` 返回整串、
// `slice(10)` 返回空串 —— Go 的切片会 panic，所以这里显式处理。
func TokenMD5(accessToken, timestamp string) string {
	ts10 := TimestampSuffix10(timestamp)
	head, tail := accessToken, ""
	if len(accessToken) > 10 {
		head, tail = accessToken[:10], accessToken[10:]
	}
	return core.MD5Hex([]byte("Bearer " + head + ts10 + tail))
}

// ApplyRoomRequest 构造 `POST /v3/room/member/apply`。
//
// # 头集合以**实拍**为准（`test/fixtures/http/直播716.json`）
//
//	Content-Type: application/json      Accept: */*
//	Version: v23.0.2                    timestamp: <13 位毫秒>
//	Authorization: Bearer <token>       Origin: https://v.cmcc-cs.cn
//	deviceInfo: <48 字节 hex>            systemMd5 / tokenMd5
//
// ⚠ **不**加 `appkey` 头。原项目加了 `appkey: cmos/ngvlcs`，但实拍那 19 个头里
// **没有它** —— 那个值是从 apply 的**响应**（`data.appKey`）里抄来的，
// 属于把响应字段误当成请求要求。多发一个头通常无害，但"照抄原项目"在这里
// 会掩盖一个事实：我们并不知道服务端要不要它。
//
// # `message` 字段**要发**
//
// 原项目漏了它（`{roomId, userName, invoker}`），实拍是
// `{"invoker":…,"message":"","roomId":…,"userName":…}` —— 补上。
func ApplyRoomRequest(
	chatRoomID, userName, invoker, accessToken, deviceInfoHex, timestamp string,
) (core.HTTPRequest, *core.Error) {
	systemMD5, err := SystemMD5(deviceInfoHex, timestamp)
	if err != nil {
		return core.HTTPRequest{}, err
	}
	request, bodyErr := core.NewRequestPost(
		fmt.Sprintf("%s/v3/room/member/apply", BaseVideoIM)).
		WithJSONBody(map[string]any{
			"invoker":  invoker,
			"message":  "",
			"roomId":   chatRoomID,
			"userName": userName,
		})
	if bodyErr != nil {
		return request, asCoreError(bodyErr, "序列化请求体失败")
	}
	return request.
		WithHeader("Accept", "*/*").
		WithHeader("Version", LiveApplyVersion).
		WithHeader("timestamp", timestamp).
		WithHeader("Authorization", "Bearer "+accessToken).
		WithHeader("Origin", LiveOrigin).
		WithHeader("User-Agent", LiveUserAgent).
		WithHeader("deviceInfo", deviceInfoHex).
		WithHeader("systemMd5", systemMD5).
		WithHeader("tokenMd5", TokenMD5(accessToken, timestamp)), nil
}

// ApplyRoomResult 是 `member/apply` 的解析结果。
//
// ⚠ 这一条用的是**普通信封**（`{code: 0, msg, data, timestamp}`）——
// 数字码 + `data`，与 `/cmlive` 的 `rtnCode` + `object` **不是一套**。
// 别"顺手统一"成 [CmliveResult]：那会让 `data` 永远取不到。
type ApplyRoomResult struct {
	AppID  string
	AppKey string
	// RoomID 是回显的聊天室 id（与请求里的 `roomId` 同值）。
	RoomID string
	// ServerTimestamp 是服务端的毫秒时间戳（13 位数字）。
	ServerTimestamp int64
}

// ParseApplyRoom 解析 `member/apply`。
func ParseApplyRoom(envelope *Envelope) (ApplyRoomResult, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return ApplyRoomResult{}, err
	}
	if payload == nil {
		return ApplyRoomResult{}, core.NewParse("member/apply 响应没有 data")
	}
	return ApplyRoomResult{
		AppID:           strAt(payload, "appId"),
		AppKey:          strAt(payload, "appKey"),
		RoomID:          strAt(payload, "roomId"),
		ServerTimestamp: intAt(envelope.Raw(), "timestamp"),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 取字段的小助手
// ─────────────────────────────────────────────────────────────────────────────

// strAt 取一个可选字符串，缺了或不是字符串一律当作空串。
//
// ⚠ 与 [RequireStr] 的分工：那个用于"缺了就不该继续"的字段，
// 这个用于"缺了只是少一条信息"的字段（标题、预告时间…）。
func strAt(payload any, key string) string {
	obj, ok := payload.(map[string]any)
	if !ok {
		return ""
	}
	text, _ := obj[key].(string)
	return text
}

// nestedStrAt 取 `obj[outer][inner]`（如 `room.anchor.nickname`）。
func nestedStrAt(payload any, outer, inner string) string {
	obj, ok := payload.(map[string]any)
	if !ok {
		return ""
	}
	child, ok := obj[outer].(map[string]any)
	if !ok {
		return ""
	}
	return strAt(child, inner)
}

// LiveRoomIDString 把房间 id 转成请求里要用的字符串形态。
//
// 单独一个函数而不是各调用点 `strconv.FormatInt`：房间 id 是 31 位的大整数，
// 任何一处误用 `float64` 都会静默丢精度，收敛到一处才好审。
func LiveRoomIDString(roomID int64) string { return strconv.FormatInt(roomID, 10) }
