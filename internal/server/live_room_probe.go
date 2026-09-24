package server

// 只读探针：`GET /api/accounts/{id}/live-room` —— 直播小红花页的第一屏。
//
// # 为什么单独一条接口，而不是"跑一趟然后看结果"
//
// 跑一趟（`POST /api/runs`）会**投口令**，而 `draw` 是**写操作**。
// ⚠ 别写"口令投出去就收不回来、每场全世界只有两条" —— 口令是**全场通用**的，
// 谁投过都不影响别人（见 `internal/upstream/live_flower.go` 文件头五条事实②）；
// 变的是**这个账号**的兑换状态（同一账号重复投同一条正确口令才回 `200112`）。
// 用户点开页面时想要的只是两个答案：
// **今晚有没有那场直播（下一场几点）** 与 **这一号手上有多少朵**。
// 这两个答案都由只读接口给（判定见 [app.ProbeLiveRoom]，那边钉了"一条写操作都不发"）。
//
// # 为什么按账号而不是全局
//
// 余额是**每个账号各一份**的（直播侧按手机号定位），而门要看凭据才打得开。
// 于是这一条与 `cloud-stats` 同形：账号维度、只读、归属错了 404。
//
// # 出网形状仍然逐字段手写
//
// 与 [accountPayload] / [runPayload] 同一条纪律：这里递出去的是**拼出来的 map**，
// 不是 [app.LiveRoomProbe] 本身 —— 那个结构体将来会加字段，
// 而"加了就自动出网"正是凭据泄漏最常见的路径（它今天不带 JWT，明天未必）。
//
// # 一次点按钮打八枪
//
// 门那三步（换 SSO token → 换直播侧 JWT → 两个页签）+ 云盘侧建会话三步
// （换 SSO token → 服务端时间戳 → `tyrzLogin`）+ 一次余额读数。
// 这比"界面上那个按钮只做本地计算"贵，但**没有更便宜的正确做法**：
// 少拉一个页签就会整场漏掉口令（实拍依据见 [app.QueryKoulingRooms] 的注释），
// 而余额那一族在 `m.mcloud.139.com/ycloud`，**只认云盘侧那一份 JWT** ——
// 拿直播侧 JWT 去读实测回 `90001`（补记与判据见 [app.ProbeLiveRoom]）。
// 所以这一条**只在进页面与手动点"重新看"时**调，不做轮询。

import (
	"net/http"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// liveRoomStatusToken 把上游那个 `status` 数字翻成**稳定英文码**。
//
// 为什么不在服务端直接写中文：本项目对外的判据一律是英文码、
// 中文由界面翻（见 [refreshAuthPayload] 的 `decision`）。上游哪天多一档状态时，
// 这里回 `unknown` 而不是替它编一个中文名字。
func liveRoomStatusToken(status int64) string {
	switch status {
	case upstream.LiveRoomStatusNotStarted:
		return "not-started"
	case upstream.LiveRoomStatusLive:
		return "live"
	case upstream.LiveRoomStatusEnded:
		return "ended"
	}
	return "unknown"
}

func (s *Server) probeLiveRoom(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := core.NewAccountID(r.PathValue("id"))
	ctx := r.Context()
	scope := scopeOf(auth)

	// 存在性与归属一次查完，两者都 404（报 403 等于确认"这个 id 确实存在"）。
	account, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if account == nil {
		ErrDomain(core.NewNotFound("云盘账号 " + id.String())).Write(w)
		return
	}
	if s.runs == nil {
		// 探针复用执行面那一份出站装配（同一个上游客户端、同一份设备指纹）。
		ErrDomain(core.NewInternal("这个服务端没有装配执行面")).Write(w)
		return
	}

	// 下面四步与 [app.LiveRoomRun.RunAccount] 的开头**逐字同序**：
	// 设备档案 → 域内档案（`userDomainId` 要用库里那一份）→ 凭据 → 时间戳。
	// 少任何一步的后果都是"今天没开播"式的假结论，而不是一个响亮的错误。
	device, perr := s.runs.appDevice()
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	profile, perr := app.ProfileWithStoredDomainID(ctx, s.store, scope,
		upstream.NewAppProfile(device), id)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	credential, perr := app.CredentialFromAccount(ctx, s.store, scope, id)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}

	// 票走 `s.runs.sessions` 那份**进程级**票池：界面上连着按几次"看一眼"应当只建一条
	// 会话，而不是每次重新打三枪（口径见 `internal/app/signin_session_pool.go` 头注）。
	probe, perr := app.ProbeLiveRoom(ctx, s.runs.upstream, profile,
		&credential, s.clock, s.rng, s.runs.sessions)
	if perr != nil {
		// 门坏了**如实报错**，不降级成"没有直播"：那是原项目那句"失败即放行"的
		// 表现方式，而它把"凭据过期"盖成了"今天没开播"（判据见 [app.OpenKoulingGate]）。
		ErrDomain(perr).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, liveRoomProbePayload(id, probe))
}

// liveRoomProbePayload 拼这一条的响应体（字段名 camelCase，与其余接口一致）。
func liveRoomProbePayload(id core.AccountID, probe app.LiveRoomProbe) map[string]any {
	rooms := make([]map[string]any, 0, len(probe.Rooms))
	for _, room := range probe.Rooms {
		rooms = append(rooms, map[string]any{
			"roomId":          room.ID,
			"title":           room.Title,
			"status":          room.Status,
			"statusToken":     liveRoomStatusToken(room.Status),
			"expectStartTime": room.ExpectStartTime,
			"anchor":          room.AnchorNickname,
		})
	}

	// `balance` 为 null 时**必须**配上 `balanceError` 那句话：
	// null + 空原因 = "我们不知道"，而界面要能说出是**哪一次读数**坏了。
	var balance map[string]any
	if probe.Balance != nil {
		balance = map[string]any{
			"flowerCnt":      probe.Balance.FlowerCnt,
			"validDate":      probe.Balance.RedFlowerValidDate,
			"hasNotReceived": probe.Balance.HasNotReceivedRedFlower,
			"quarterDrawed":  probe.Balance.IsQuarterDrawed,
			"prizeId":        probe.Balance.PrizeID,
			"secondPrizeId":  probe.Balance.SecondPrizeID,
		}
	}

	return map[string]any{
		"accountId": id.String(),
		// 门的判定：`koulingLive` 才是"现在去听有没有口令"，
		// 而它假 + `liveCount` 真 = "有别的主播在播，但不是那场"。
		"koulingLive":  probe.Gate.Active,
		"liveCount":    probe.Gate.LiveCount,
		"rooms":        rooms,
		"anchor":       upstream.KoulingAnchor,
		"balance":      balance,
		"balanceError": probe.BalanceError,
	}
}
