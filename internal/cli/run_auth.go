package cli

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// `auth refresh` 的**命令行编排**：挑账号 → 备一次依赖 → 逐个刷 → 把结论说给人听。
//
// # 这一层只剩三件事
//
// 判据在 `capabilities`（该不该刷、新串长什么样），头的准备在 `app`（哪一端配哪套头）。
// 本层只剩三件事：这次跑哪些账号、依赖备一份、每一行怎么印。
//
// # 为什么批量时逐个继续、最后才报错
//
// 刷新是**写库**操作，一个账号失败不影响另一个的凭据；中途 return 的话，
// 用户看到的是"跑了 3 个就停了"，而不知道剩下 5 个到底动没动。
// 所以先把每一行都跑完、印出来，再用退出码告诉脚本"这一批里有红的"。
//
// # 为什么输出里没有手机号与 token
//
// 账号一律用 id（脱敏手机号）或昵称；`auth_expire` 是**日期**，不是凭据。

func runAuthRefresh(deps *Deps, out *output, a *AuthRefreshArgs) error {
	if a.Account == "" && !a.All {
		return fmt.Errorf(
			"要指定刷谁：`--account <账号 id>` 或 `--all`（后者只刷到点的那些）: %w", ErrUsage)
	}
	if a.LeadDays <= 0 {
		// 0 与负数**不**当成"用默认值"：那样 `--lead-days 0`（想"到期当天才刷"）
		// 会静默变成 7 天，而那恰好是用户想避开的那个含义。
		return fmt.Errorf("`--lead-days` 要是正数（给 0 或负数没有含义）: %w", ErrUsage)
	}

	store, err := openLocal(a.DB)
	if err != nil {
		return err
	}
	defer store.Close()

	ctx := context.Background()
	owner, err := preferredOwner(ctx, store)
	if err != nil {
		return err
	}
	scope := scopeForNewAccounts(owner)

	targets, err := selectRefreshTargets(ctx, store, scope, a)
	if err != nil {
		return err
	}
	if len(targets) == 0 {
		// `--all` 在"库里全是停用账号"时是合法的空批。这里不报错：
		// 报"失败"会让人以为刷新坏了，而实况是"没有要刷的"。
		out.human("没有要刷的账号（`--all` 只看启用中的）")
		return nil
	}

	// 设备形态在**整次运行里只解析一次**：同一个安装就该是同一台设备。
	// 放进循环里会让每个账号拿到不同的 `deviceShort`，与它登录时那台对不上。
	appDeviceID, perr := app.LoadOrCreateAppDeviceID(a.AppDeviceFile, deps.Random)
	if perr != nil {
		return perr
	}
	device := upstream.NewIPadAppDevice(appDeviceID, app.NewAppDeviceShort(deps.Random))

	refreshDeps := &app.RefreshDeps{
		Upstream:   upstream.NewUpstream(deps.http()),
		Store:      store,
		Scope:      scope,
		Clock:      deps.Clock,
		Random:     deps.Random,
		AppDevice:  &device,
		PCPlatform: pcPlatform(a.Platform),
		LeadDays:   a.LeadDays,
		Force:      a.Force,
	}

	failed := 0
	for _, account := range targets {
		// 先印账号再动手：这一支可能要几十秒，进度行要在那之前出现，
		// 否则批量时看起来像卡住了。
		out.human("%s", accountTitle(account))
		report, rerr := app.RefreshAccountAuth(ctx, refreshDeps, account.ID)
		switch {
		case rerr != nil:
			failed++
			out.human("  ✗ %s", refreshFailureText(rerr))
		case !report.Rotated:
			out.human("  %s", describeNotRotated(report, a))
		case report.Decision == capabilities.RefreshNotYet:
			// 到这一支说明是 `--force` 越过判据换的：把"本未到点"说出来，
			// 否则同一句"已换新凭据"会让人以为提前量那道闸生效了。
			out.human("  已换新凭据（按 `--force` 换的：还剩 %g 天，本未到提前量）："+
				"%s", a.LeadDays, describeExpiryMove(report))
		default:
			out.human("  已换新凭据（以 %s 那一支刷的）：%s",
				refreshEndName(report.End), describeExpiryMove(report))
		}
	}
	if failed > 0 {
		return fmt.Errorf("%d 个账号刷新失败（其余的已各自落库，未受影响）", failed)
	}
	return nil
}

// selectRefreshTargets 挑出这次要刷的账号。
//
// # 为什么走 `ListAccounts(scope)` 而不是 `GetAccount`
//
// 可见范围规则（谁的账号谁能看）只有那一份。自己再按 id 查一次，
// 就会造出"CLI 能刷到 WebUI 看不见的账号"这种第二套判据。
//
// # 为什么批量只挑**启用中**的
//
// 停用的账号是用户主动停的（多半已经在别处重登或准备删）。刷新会轮换它的凭据，
// 把一个已被停用的账号悄悄刷成"看起来还能用"，是把用户的决定抹掉了。
func selectRefreshTargets(
	ctx context.Context, store core.StateStore, scope core.Scope, a *AuthRefreshArgs,
) ([]core.Account, error) {
	accounts, err := store.ListAccounts(ctx, scope)
	if err != nil {
		return nil, fmt.Errorf("读账号列表失败: %w", err)
	}
	// ⚠ 这里**不**复用 `smid` 那边的 [requireAccount]：它按 `ScopeLocal` 查，
	// 而多用户库里账号是**归属到用户**的 —— 沿用会出现"CLI 刷得到、WebUI 看不见"
	// 的第二套可见性判据。列表只有一份，就按列表匹配。
	if a.Account != "" {
		want := strings.TrimSpace(a.Account)
		for _, account := range accounts {
			if account.ID.String() == want {
				return []core.Account{account}, nil
			}
		}
		return nil, fmt.Errorf("没有账号 %s（该范围里现有：%s）", want, joinAccountIDs(accounts))
	}
	var targets []core.Account
	for _, account := range accounts {
		if account.Enabled {
			targets = append(targets, account)
		}
	}
	return targets, nil
}

func joinAccountIDs(accounts []core.Account) string {
	if len(accounts) == 0 {
		return "（一个都没有）"
	}
	ids := make([]string, 0, len(accounts))
	for _, account := range accounts {
		ids = append(ids, account.ID.String())
	}
	return strings.Join(ids, ", ")
}

// refreshFailureText 决定这一行失败文案给多细。
//
// 分界线与 internal/server/error.go 的 `APIError.Message` **同一条**：
// 使用者看得懂、且修它需要看到的细节就给（"上游返回 code=xxxx: …"），
// 而 5xx / 网络 / 内部错误那三类原文里实测带过 SQL 片段与上游响应体，
// 只给类别文案。
//
// ⚠ 用 `Message()` 不用 `Error()`：后者带"凭据失效:"这类类别前缀，
// 那是给日志看的，端给人会变成"凭据失效: …"这种半截话。
func refreshFailureText(err *core.Error) string {
	switch err.Kind() {
	case core.KindInternal, core.KindTimeout, core.KindNetwork:
		return err.Kind().PublicMessage()
	case core.KindUpstream:
		if err.Status() >= 500 {
			return err.Kind().PublicMessage()
		}
	}
	return err.Message()
}

// describeNotRotated 是"没刷"那一档的人话。
//
// 两档都**不是失败**，所以不带 ✗；但也要说清下一步该做什么，
// 否则"没动"会被读成"坏了"。
func describeNotRotated(report app.RefreshReport, a *AuthRefreshArgs) string {
	if report.Decision == capabilities.RefreshManualOnly {
		return "  刷不了：库里那串 auth 的过期时刻是负数，只能人工换 auth" +
			"（重新登录一次，或把新的 auth 贴进 WebUI）"
	}
	return fmt.Sprintf("  还没到提前量（剩 %g 天以内才刷），没动上游", a.LeadDays)
}

func describeExpiryMove(report app.RefreshReport) string {
	if report.BeforeExpire == nil && report.AfterExpire == nil {
		return "新 token 不带过期信息（库里 `auth_expire` 已置空）"
	}
	if report.BeforeExpire == nil {
		return "→ " + formatExpiry(report.AfterExpire)
	}
	if report.AfterExpire == nil {
		return formatExpiry(report.BeforeExpire) + " → 新 token 不带过期信息"
	}
	return formatExpiry(report.BeforeExpire) + " → " + formatExpiry(report.AfterExpire)
}

// formatExpiry 把 epoch **毫秒**印成本地时区的日期时间。
//
// ⚠ 单位写进注释不是多余的：token 第 4 段与 `auth_expire` 都是毫秒，
// 而上游刷新响应里的 `expireTime` 是秒 —— 拿秒值进这个函数会印成 1970 年。
func formatExpiry(millis *int64) string {
	if millis == nil {
		return "（无）"
	}
	return time.UnixMilli(*millis).In(core.LocalTZ()).Format("2006-01-02 15:04")
}

// refreshEndName 把端别翻成给人看的名字。
func refreshEndName(end string) string {
	switch end {
	case "pc":
		return "PC 客户端"
	case "mobile":
		return "手机 App"
	}
	return "未知端 " + end
}
