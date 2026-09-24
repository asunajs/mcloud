package server

// 只读选品器：`GET /api/accounts/{id}/exchange-prizes` —— 兑换页的第 ③ 块。
//
// # 为什么单独一条接口，而不是"跑一趟然后看结果"
//
// 兑换那一趟会**真扣云朵且不可回滚**（判据在 `internal/app/exchange_run.go` 文件头）。
// 用户点开这一页时想要的是"此刻这个账号看得见哪些件、哪件还轮得到我"，
// 那一读只对应**一枪** `exchangeList`（[app.ListExchangePrizes]），没有任何写操作。
//
// # 为什么按账号、且**不**用旧项目那条 `GET /api/exchange/prizes` 的路径
//
// 旧项目把它做成一条**独立轨**（免登录、`ofetch` 绕过它自己的 HTTP 层）。
// 用户 2026-09-19 已就地裁定：那既不是刻意设计也不算缺陷，新项目**当普通鉴权缺口补齐**
// （见 `docs/pending-decisions.md` 第四节）。补齐之后的形态必然是"要会话 + 带账号"，
// 而带账号的读接口在本仓库只有一个形状：`/api/accounts/{id}/<读面>`
// （`cloud-stats`、`live-room` 都是这一形）。所以这里**跟着本仓库的形**，
// 不照抄那条路径 —— 路径本身不是对拍面，**上游**那一口才是要对齐的（已在
// `internal/upstream/app_exchange.go`，逐字按实拍）。
//
// # 为什么清单里每一件带 `plan` / `reason` 而不是一个 bool
//
// "不能兑"有七种，处置完全不同：`limit=2` 是换一件、`limit=1` 是等明天、
// `sold_out` 是等下一场。合成一个 bool 之后界面只能显示"不可兑"，
// 而那句话不告诉用户下一步做什么（与 `planExchangeLimit` 拒绝复用
// `CanExchangeNow` 是同一条理由）。`reason` 出的是 [app.ExchangePlan.Reason]
// 那一份**中文判据原文** —— 文案只在 Go 侧有一处，前端不再抄一份颜色/话术表。
//
// ⚠ 这里的 `plan` 出自 [app.PlanExchangeWithoutBalance]：**不含余额那一档**。
// 所以界面上的"可兑"比下单前的真判据**宽**（余额不够要到了下单那一步才会被挡）。
// 为什么不多打一枪去读余额，理由与后果都写在 `internal/app/exchange_list.go` 文件头。
//
// # 出网形状仍然逐字段手写
//
// 与 [liveRoomProbePayload] / [accountPayload] 同一纪律：递出去的是拼出来的 map，
// 不是 [upstream.ExchangePrize] 本身 —— 那个结构体将来会加字段，
// 而"加了就自动出网"是凭据泄漏最常见的路径。

import (
	"net/http"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// exchangePrizes 处理 `GET /api/accounts/{id}/exchange-prizes`。
func (s *Server) exchangePrizes(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
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
		// 与探针同一条：复用执行面那一份出站装配（同一个上游客户端、同一份票池）。
		ErrDomain(core.NewInternal("这个服务端没有装配执行面")).Write(w)
		return
	}
	world, perr := s.signInWorldBase(scope)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	// 票走 `s.runs.sessions` 那份**进程级**票池：界面上连着按几次"刷新清单"应当只建
	// 一条会话（口径见 `internal/app/signin_session_pool.go` 头注与 AGENTS §1.12）。
	src, perr := world.Source(ctx, id)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	list, perr := app.ListExchangePrizes(ctx, src)
	if perr != nil {
		// 上游回什么就是什么，**不**降级成"清单是空的"：那一档的表现是界面上
		// "今天没有可兑的"，而真相可能是"票过期了"（与探针"门坏了如实报错"同一条纪律）。
		ErrDomain(perr).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, exchangePrizesPayload(id, list, s.clock.Now()))
}

// exchangePrizesPayload 拼这一条的响应体（字段名 camelCase，与其余接口一致）。
//
// 顺序**只有一处真相**：[upstream.ExchangeList.Order] 是分组键按**数值**升序，
// 摊平走 [upstream.ExchangeList.All]。这里不再排序，也不按 `0..n` 遍历 ——
// 实拍那九个键是 `0,1,2,5,7,8,9,11,15`，自己排一遍就会把那条坑重新踩一遍。
func exchangePrizesPayload(
	id core.AccountID, list upstream.ExchangeList, now time.Time,
) map[string]any {
	items := list.All()
	prizes := make([]map[string]any, 0, len(items))
	for index := range items {
		prize := items[index]
		plan := app.PlanExchangeWithoutBalance(prize, now)
		group := ""
		// 分组键反查一次即可（清单规模是"几十件"，不是"几万"）：界面要按组排版，
		// 而 `All()` 摊平之后这一格就丢了。找不到时留空串而不是"0" —— 后者会
		// 把"没归上组"念成"在第 0 组"。
		for _, key := range list.Order {
			for _, member := range list.Groups[key] {
				if member.PrizeID == prize.PrizeID {
					group = key
					break
				}
			}
			if group != "" {
				break
			}
		}
		prizes = append(prizes, map[string]any{
			"prizeId":  prize.PrizeID,
			"groupId":  group,
			"name":     prize.Name,
			"price":    prize.Price,
			"limit":    prize.Limit,
			"plan":     plan.Slug(),
			"reason":   plan.Reason(),
			"canOrder": plan == app.ExchangePlanGo,
			// 三格余量都是**时刻量**（会因为别的用户在同一分钟里兑掉而变，抓包 §5.4），
			// 所以界面只能当下拉时的读数用，不能当流水。
			"count":               prize.Count,
			"totalCount":          prize.TotalCount,
			"dailyRemainderCount": prize.DailyRemainderCount,
			"minRemainderCount":   prize.MinRemainderCount,
			"onLine":              prize.OnLine,
			"startTime":           prize.StartTime,
			"endTime":             prize.EndTime,
		})
	}
	return map[string]any{
		"accountId": id.String(),
		"readAt":    now.UTC().Format(time.RFC3339),
		"groups":    list.Order,
		"total":     len(prizes),
		"prizes":    prizes,
	}
}
