// 「看一眼直播间与小红花余额」——**只读**探针，一个口令都不投。
//
// 它是 [OpenKoulingGate] 的前半段 + 六步链的第①步（`user/info`）拼出来的：
// 界面上进入直播小红花页时要先回答两个问题 ——
// **现在有没有那场直播、这一号手上有多少朵**。这两个问题都不需要写操作，
// 所以它们不该被"跑一趟"（跑一趟会**投口令**，而 `draw` 是**写操作** ——
// 投出去就以这个账号的名义换了一朵）。
//
// ⚠ 这里**不说**"口令投出去就收不回来"：口令是**全场通用**的，谁投过都不影响
// 别人再投（见 `internal/upstream/live_flower.go` 文件头五条事实②）。
// 会变的只是**这个账号**的兑换状态 —— 早先那句"收不回来"把"按账号去重"
// 误写成了"口令被消耗"（2026-09-23 改）。
//
// # 为什么余额失败不整体失败
//
// 门（登录 + 两个页签）与余额（`user/info`）是**两个接口**，坏在不同处：
// 门坏了意味着"这一号进不去直播侧"（多半是凭据过期），此时余额也必然读不到，
// 整条探针失败是对的；而门开着、只有 `user/info` 抽风时，
// "今晚 19:30 有一场"这件事仍然成立且有用 —— 把它一起报成失败，
// 用户就看不到他唯一要看的那一句。所以余额的失败**单独带一句话**回去
// （[LiveRoomProbe.BalanceError]），而不是折进返回值里的 0。
package app

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// LiveRoomProbe 是只读探针的结论。
//
// ⚠ 里面**没有**直播侧 JWT：那是凭据本体，探针自己用完就丢
// （与 [LiveRoomAccountResult.HasJWT] 同一条纪律 —— 它连"有没有"都只存一个 bool）。
type LiveRoomProbe struct {
	// Gate 是门的判定：`Active`（有没有那场口令直播）、`LiveCount` 与 `Rooms`
	// 一起才分得清两种"没有"（谁都没在播 / 有在播但不是那个主播）。
	Gate KoulingGate
	// Rooms 是**那个主播名下**的房间（预告 / 在播 / 回放都在，按页签返回的顺序）。
	//
	// 它存在的理由只有一条：界面上要写"下一场 2026-09-23 14:30"，
	// 而预告场在 [Gate] 里必然被筛掉（它不是 `status == 1`）。
	// ⚠ 不要拿它判"现在能不能听" —— 那是 [Gate.Active] 的活。
	Rooms []upstream.LiveRoom
	// Balance 是小红花读数；nil 表示这一次没读到，原因在 [BalanceError]。
	Balance *upstream.RedFlowerBalance
	// BalanceError 是"门开着但余额没读到"的那句话；读到了就是空串。
	BalanceError string
}

// ProbeLiveRoom 换直播侧凭据 → 判门 → 读小红花余额。
//
// 给的是**凭据 + 时钟 + 随机源**而不是 `(timestamp, authorization)`：这一条要看的两件事
// 分属**两套会话**（见下面的补记），只给一份凭据就只读得到一件。
// 本函数**不查库**：账号定位与凭据读取留在调用方，这样服务端与以后任何产物
// 共用的是同一次"看一眼"，而不是各写一遍顺序。
//
// # 补记（2026-09-22 实测）：余额用的**不是**直播侧那个 JWT
//
// 本函数最初把 [OpenKoulingGate] 换到的直播侧 JWT 直接带给 `user/info`，
// 实机两个账号都回 `90001 未登录`（WebUI 探针第一跑，22:30 前后）。
// 对着抓包看宿主才对上：
//
//	门（`cmpassLoginByCmToken` / `tabContentList`）在 **`v.cmcc-cs.cn`** —— 直播侧，认那个 JWT；
//	余额（`liveRoomFeedback/user/info`）在 **`m.mcloud.139.com/ycloud`** —— 与签到**同域同族**
//	                                （公共头里那个 `activityid: sign_in_3` 就是证据），
//	                                它要的 `jwttoken` 是 **`tyrzLogin` 那一份**（905 字符，
//	                                实拍里它与 cookie `jwtToken` 同值：`app-signin-samples.txt` §2.4）。
//
// ⇒ 余额前先建一条云盘会话拿 JWT（现经 [SignInSessionSource]，见下面那段）。
// 多打三枪换"读得到 vs 永远 90001"，这一笔没有别的省法 —— 能省的只有"别为每一次
// 看一眼都重打一遍"，那由下面的 `sessions` 负责。
//
// ⚠ [LiveRoomProbe.BalanceError] 只带**消息**不带结构：那句会原样显示在界面上，
// 而上游原文里可能夹着账号侧标识。
//
// `sessions` 是进程内那份票池（界面"连着看几次"应当只建一条会话，见 [SessionPool]）；
// 传 nil 时这一枪自己建一条、用完即弃。
func ProbeLiveRoom(
	ctx context.Context, up *upstream.Upstream, profile *upstream.AppProfile,
	credential *SignInCredential, clock core.Clock, rng core.Random, sessions *SessionPool,
) (LiveRoomProbe, *core.Error) {
	opened, err := OpenKoulingGate(ctx, up, profile, UserTimestamp(clock), credential.Authorization())
	if err != nil {
		return LiveRoomProbe{}, err
	}
	probe := LiveRoomProbe{
		Gate:  opened.Gate,
		Rooms: upstream.AnchorRooms(opened.AllRooms),
	}

	// 云盘侧会话。**失败只折进 [LiveRoomProbe.BalanceError]**：门开着就说明
	// "今晚有没有那场直播"这件事已经答完了，把它一起报成失败会让用户看不到他唯一要看的那一句。
	//
	// 票从 `sessions` 拿：界面上同一个账号连着刷几次这一屏 ⇒ 只建一条会话
	// （没接池时回落成"这一次看一眼自己建一条"，口径见 `signin_session_pool.go` 头注）。
	// 余额那一枪撞 `90001` 时由 source 刷一次、重放一次（`signin_session.go` 头注）。
	// 会话在这里**先逼一把**是为了保住那句 `[小红花·建云盘会话]` ——
	// 让 `do` 去懒建的话，同一个失败会被包成"小红花·余额 …"，看不出坏在建会话。
	//
	// **jwttoken 使用公共来源**（`AGENTS.md` §1.12 要求留痕）：这一枪要的票来自签到页
	// 那一套参数，直播页自己的取法尚未抓包确认；查明那天要改的是这里的**配方** + 这句注释。
	src := sessionSource(sessions, up, profile, credential, clock, rng, RecipeSignInPage)
	if _, perr := src.Session(ctx); perr != nil {
		probe.BalanceError = perr.WithPrefix("[小红花·建云盘会话] ").Message()
		return probe, nil
	}
	balance, perr := do(ctx, src, func(s SignInSession) (upstream.RedFlowerBalance, *core.Error) {
		// 余额走**六步链的①**那个接口（`liveRoomFeedback/user/info`），
		// 不是直播侧的 `getCurrentUserInfoNew` —— 后者只是建会话，响应里没有花。
		envelope, perr := callBuilt(ctx, up, "小红花·余额", func() (core.HTTPRequest, *core.Error) {
			return upstream.RedFlowerUserInfoRequest(s.jwt)
		})
		if perr != nil {
			return upstream.RedFlowerBalance{}, perr
		}
		return upstream.ParseRedFlowerBalance(envelope)
	})
	if perr != nil {
		probe.BalanceError = perr.Message()
		return probe, nil
	}
	probe.Balance = &balance
	return probe, nil
}
