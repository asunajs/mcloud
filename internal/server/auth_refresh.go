package server

// 刷新凭据（`POST /api/accounts/{id}/refresh-auth`）—— 账号写面的第四位。
//
// # 这一层只剩形状
//
// 判据（该不该刷、新串落成什么样）在 `capabilities`，两端的头怎么备在 `app`
// （[app.AuthRefresher]）。本文件只做四件事：认账号、收那两个可选参数、
// 把结论写成 JSON、把 `*core.Error` 换成状态码。出现"该刷几天 / 打哪个 URL /
// 哪个端配哪套头"的字样就是放错了层。
//
// # 凭据边界（比 [accounts_write.go] 那三条多一条）
//
//   - **只进不出**：出网的是 [refreshAuthPayload]，里面只有 id / 端别名 / 结论码 /
//     两个过期时刻（epoch 毫秒）。新 token 落库后**不回填**给调用方 ——
//     界面要显示的"到哪天"由回读的 [accountPayload] 里那个 `authExpire` 给出，
//     那是同一个事实的日期形态，不是凭据；
//   - **不进 URL**：这条路径上没有凭据，id 是掩码手机号；
//   - **失败不回显**：错误走 [ErrDomain]，5xx 只给类别文案（原文里可能带上游响应体）。
//
// # 这一条路由**没有**提前量入参：点一下就换
//
// 提前量那道省请求的闸属于**到点才换**那一路（调度循环走的是同一个判据，
// CLI 的 `--all` 也走它 —— 外部定时复用的就是那条命令），它的读者是"一台机器一天扫一次"。
// 而走到这条路由的每一次请求
// 背后都是**刚在确认弹窗上点过一下的人** —— 再拿判据去替他决定"你其实不用刷"，
// 表现就是"我点了，它说还早，什么都没发生"。所以这里固定 `force = true`。
//
// # 那 200 里还能剩下什么
//
// 只剩 [capabilities.RefreshManualOnly]（库里那串 `expire < 0`，上游刷不了）。
// 它也不是失败：编成 4xx 会让前端显示成红色，而正确出口是"重新登录 / 贴整串 auth"，
// 不是"重试"。同一理由见 [capabilities.RefreshNotYet] 那条。
// `decision` 仍如实回：`not-yet` + `rotated=true` 这一对就是"还没到提前量，
// 但按你这次点击换了"，前端拿它拼文案。

import (
	"net/http"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
)

// refreshAuthBody 是刷新请求体：**只剩 `platform` 这一位**，而且整个请求体可以不给
// （`curl -XPOST …` 是最自然的试法，那一支走 Windows 档案）。
//
// `platform` 只在账号是 PC 端登录时才被读到 —— 端别本身**不是**参数，
// 它从库里那一串 `Authorization` 的第一段读出来（理由见 app 层那条）。
// 提前量与"要不要直接换"都不在这张表上，理由见文件头那一条。
type refreshAuthBody struct {
	Platform string `json:"platform"`
}

func (s *Server) refreshAccountAuth(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := core.NewAccountID(r.PathValue("id"))
	body := refreshAuthBody{}
	if r.ContentLength != 0 {
		decoded, ok := decodeJSON[refreshAuthBody](w, r)
		if !ok {
			return
		}
		body = decoded
	}
	platform, perr := loginPlatformOf(body.Platform)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}

	ctx := r.Context()
	scope := scopeOf(auth)
	// 先确认这个账号在这个范围内存在：存储层的可见性判据会让别人的账号
	// 查不出登录态，那一句错误（"没有存 Authorization 整串"）会把"不存在"
	// 说成"坏了"，而且绕过了 [accountPayload] 那条"归属错误一律 NotFound"的口径。
	account, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if account == nil {
		ErrDomain(core.NewNotFound("云盘账号 " + id.String())).Write(w)
		return
	}

	// `leadDays` 给 0（在能力层落到默认 7 天）只为了把 `decision` 算出来：界面对
	// "换了"那一支只看 `rotated`、不看它，`decision` 只剩"没换成时那句怎么说"
	// （manual-only）。拦不拦由下一位参数决定 —— 这条路上**固定 force**，理由见文件头那一条。
	report, rerr := s.refresher.Refresh(ctx, scope, id, 0, platform, true)
	if rerr != nil {
		ErrDomain(rerr).Write(w)
		return
	}

	// 回读而不是自己拼"新的那一行"：过期时刻已经落库，界面上要显示的就是库里的事实
	// （同一理由见 [addAccount] 末尾那一段）。
	fresh, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if fresh == nil {
		// 刚刷完就读不到：中间有人删了它。给 404 而不是 500，对使用者结论同一个。
		//
		// ⚠ 这一支**未被断言**（§1.3）：测试用的是真 SQLite，"两次读之间被别人删掉"
		// 需要一个能插在中间的并发钩子，而为了测它去给存储层加钩子不值。
		// 同 [setAccountEnabled] 里那一支的处境，删掉它**不会**让任何用例变红。
		ErrDomain(core.NewNotFound("云盘账号 " + id.String())).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"refresh": refreshAuthPayload(report),
		"account": accountPayload(*fresh, s.Now().UnixMilli()),
	})
}

// refreshAuthPayload 是一次刷新的对外形状（**不含任何凭据**）。
//
// 手写而不是给 [app.RefreshReport] 加 JSON tag，理由与 [accountPayload] 那条一样：
// 那个结构体将来会加字段，而"加了就自动出网"正是凭据泄漏最常见的路径。
func refreshAuthPayload(report app.RefreshReport) map[string]any {
	return map[string]any{
		"accountId": report.AccountID.String(),
		// 端别是库里那一串的第一段（`pc` / `mobile`），原样回给前端去翻译成人话
		"end": report.End,
		// `decision` 走 [capabilities.RefreshDecision.Code]（稳定英文码），
		// 不给 `String()` 那句中文 —— 那是给终端与日志看的文案。
		"decision":     report.Decision.Code(),
		"rotated":      report.Rotated,
		"beforeExpire": report.BeforeExpire,
		"afterExpire":  report.AfterExpire,
	}
}
