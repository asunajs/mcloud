package cli

// `mcloud live-room` 的**编排层**：选账号 → 交给 [app.LiveRoomRun] 跑每个账号 → 打印。
//
// ⚠ 那条链本身（开门 / 收口令 / 投 / 重听）**不在这里**，在 [app.LiveRoomRun.RunAccount]。
// 服务端 `POST /api/runs` 跑的是同一条，两处各写一遍必然漂移，而漂移不红。
//
// # 为什么把原项目的两个任务合成一个命令
//
// 原项目里 `kouling`（收口令）与 `red-flower`（抽小红花）是**两个独立任务**，
// 而且 `red-flower` 的 `codes` 缺省时会自己去调 `kouling` 的抓取链。
// 那个形状在本项目里不成立：`red-flower` 单独跑的时候，用户要自己想清楚
// "口令从哪来"，而**能来的地方只有直播**。
//
// 合起来之后，"收 → 投"是一条命令里的两个阶段，`--kouling-only` 与
// `--skip-kouling` 分别关掉其中一个 —— 两个开关合起来正好覆盖原项目那两条任务。
//
// # 这一层不做什么
//
//   - **不判门的开合** —— [app.JudgeKoulingGate]；
//   - **不判"要不要重听"** —— [app.RetryPlan] / [app.PickNewCodes]；
//   - **不判六步链走到哪** —— [app.RedFlowerSession]；
//   - **不解析响应** —— `internal/upstream`。
//
// 它只负责：**把用户的选择翻成参数、把结果翻成人话**。

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// liveRoomOutcome 是一个账号这一趟的全部产出。
//
// 流程本身在 [app.LiveRoomRun]（CLI 与服务端共用一份，理由见那个文件）；
// 这里只留"第几个账号、叫什么名、这一趟崩在哪"。
type liveRoomOutcome struct {
	index  int
	label  string
	result app.LiveRoomAccountResult
	// failure 是这一趟的**唯一**错误出口。已经拿到的门与口令仍在 `result` 里，
	// 所以"崩在第几步"从输出上看得出来。
	failure *core.Error
}

func runLiveRoom(deps *Deps, out *output, a *LiveRoomArgs) error {
	// 用法校验放在**打库之前**：参数写错了不该被"打不开库"盖掉
	// （那会把退出码从 2 变成 1，见 cli.go 那三个退出码的说明）。
	if err := validateLiveRoomArgs(a); err != nil {
		return err
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

	accounts, err := listSigninAccounts(ctx, store, scope)
	if err != nil {
		return err
	}
	targets, err := selectLiveRoomTargets(accounts, a.Account)
	if err != nil {
		return err
	}
	if len(targets) == 0 {
		reportNoSigninTargets(out, len(accounts))
		return nil
	}

	// 设备形态在**整次运行里只解析一次**：同一个安装就该是同一台设备。
	// 放进循环里会让每个账号拿到不同的 `device_short`（不传 `--device-short` 时
	// 现生成），于是"一次运行"对应了好几种设备形态 —— 与登录时的形态也对不上。
	// 与 [runSignin] 同一条纪律，理由见那边的注释。
	appDeviceID, perr := app.LoadOrCreateAppDeviceID(a.AppDeviceFile, deps.Random)
	if perr != nil {
		return perr
	}
	// ⚠ `--device-short ""` 与"没给"必须分得开，所以判据是 [LiveRoomArgs.given]
	// 而不是 `a.DeviceShort != ""` —— 前者才是 Rust 那个 `Option` 的对位。
	deviceShort := app.NewAppDeviceShort(deps.Random)
	if a.given("device-short") {
		deviceShort = strings.ToUpper(a.DeviceShort)
	}
	device := upstream.NewIPadAppDevice(appDeviceID, deviceShort)

	run := app.LiveRoomRun{
		Store: store,
		Scope: scope,
		// ⚠ `Sessions` 不给（= 轮级票，见 [app.SessionPool] 与 CLI 签到那侧的同款注释）：
		// 一次调用里每个账号只跑一轮，"连点两次共用一张票"是服务端那一路的事。
		Driver: app.LiveRoomDriver{
			Upstream: upstream.NewUpstream(deps.http()),
			Dialer:   deps.websocket(),
			Random:   deps.Random,
			Clock:    deps.Clock,
		},
		Device: device,
		Params: app.LiveRoomParams{
			Codes:         app.SplitKouling(a.Codes),
			ListenSeconds: a.ListenSeconds,
			KoulingOnly:   a.KoulingOnly,
			SkipKouling:   a.SkipKouling,
		},
	}

	outcomes := make([]liveRoomOutcome, 0, len(targets))
	for _, target := range targets {
		// 一个账号失败不影响其余：与签到同一条纪律（旧项目对每个账号各自 try/catch）。
		outcomes = append(outcomes, runLiveRoomOne(ctx, out, run, target))
	}
	return renderLiveRoom(out, a, outcomes)
}

// validateLiveRoomArgs 是"解析期之后、动手之前"的用法校验。
//
// 判据本身在 [app.LiveRoomParams.Check]（两个产物共用一份，理由见那边）；
// 这里只留 CLI 的措辞与退码。
//
// ⚠ 与 2026-09-22 之前那版唯一的口径差别：第二条看的是**拆完之后**的口令列表，
// 于是 `--skip-kouling --codes ",,"` 这一类"只给了分隔符"的输入现在退 2，
// 而以前会带着一个空列表走下去、最后打一句"开了门但没收到口令"（退 0）。
// 那句输出把"你没给口令"说成了"没有口令可收" —— 换到退 2 才是它真正的意思。
func validateLiveRoomArgs(a *LiveRoomArgs) error {
	params := app.LiveRoomParams{
		Codes:         app.SplitKouling(a.Codes),
		ListenSeconds: a.ListenSeconds,
		KoulingOnly:   a.KoulingOnly,
		SkipKouling:   a.SkipKouling,
	}
	switch params.Check() {
	// ⚠ 这一对互斥是**语义**上的，不是形式上的：一个说"不投"、一个说"只投"，
	// 同时给的话无论怎么解释都会让一半的意图静默失效。
	case app.LiveRoomFaultExclusiveSwitches:
		return fmt.Errorf("`--kouling-only` 与 `--skip-kouling` 互斥: %w", ErrUsage)
	// `--skip-kouling` 把唯一的来源关掉了，那就必须有另一条来源。
	case app.LiveRoomFaultSkipWithoutCodes:
		return fmt.Errorf("`--skip-kouling` 需要同时给 `--codes`（否则没有任何口令可投）: %w", ErrUsage)
	case app.LiveRoomFaultListenNotPositive:
		return fmt.Errorf("`--listen-seconds` 必须为正（给的是 %d）: %w", a.ListenSeconds, ErrUsage)
	}
	if a.Account < 0 {
		return fmt.Errorf("`--account` 不能为负（给的是 %d）: %w", a.Account, ErrUsage)
	}
	return nil
}

// runLiveRoomOne 跑**一个账号**的一趟。
//
// 流程本身住在 [app.LiveRoomRun]（CLI 与服务端共用一份，理由见那个文件）。
// 这里只剩一件**只有 CLI 知道**的事：给那一句句过程话加上"第几个账号、叫什么"的前缀。
func runLiveRoomOne(
	ctx context.Context,
	out *output,
	run app.LiveRoomRun,
	target liveRoomTarget,
) liveRoomOutcome {
	prefix := fmt.Sprintf("[%d] %s", target.index, target.label)
	// `run` 是值拷贝，所以改这里的 Progress 不会串到下一个账号。
	run.Progress = func(_ core.LogLevel, message string) {
		// ⚠ 过程话一律走 stderr（`note`）：`--json` 时 stdout 只能是那份 JSON。
		// 级别在这条路上不改变落点，但服务端要用它决定事件的 `level` 字段
		// ⇒ 判级发生在 [app.LiveRoomRun] 那一层，两个产物共用同一份判断。
		out.note("%s %s", prefix, message)
	}
	result, perr := run.RunAccount(ctx, target.accountID)
	return liveRoomOutcome{
		index:   target.index,
		label:   target.label,
		result:  result,
		failure: perr,
	}
}

// liveRoomTarget 是本命令要跑的一个账号。
type liveRoomTarget struct {
	// index 是展示用下标（从 1 起，与 `mcloud signin --list` 一致）。
	index int
	// accountID 是账号 id（掩码）。
	accountID core.AccountID
	// label 是给人看的账号名。
	label string
}

// selectLiveRoomTargets 选账号。
//
// `--account N` 里的 N 是**展示下标**（从 1 起），不是数组下标 ——
// 与 `mcloud signin --account` 同一口径。`0` 表示"全部启用账号"。
//
// ⚠⚠ 上面那句"同一口径"**只指"从 1 起"那半句** —— `0` 两边**不一样**，别顺手改：
// `signin --account 0` 是**用法错误（退 2）**（`args.go` 的 `accountRequested` 把
// "没给"与"给了 0"分开，后者判成用户的错），而这里 `0` = 全部启用账号。
// 实测（2026-09-22，**空库**）：`signin --account 0` ⇒ 退 2；`live-room --account 0` ⇒ 退 0
// —— 后者那格是"没有账号可跑"的值，判据是"**它没被判成用法错误**"，不是"它退 0"。
// ⚠ 这条 `0` 的语义被本文件的 `--help` 文本（`args_live.go`）与
// `internal/server/runs.go` 的"留空 = 全部启用"引用着 ⇒ 要不要统一**留待用户定**，
// 见 `docs/status.md` §六 末尾。
//
// ⚠ 只选**启用中**的账号（与签到一致）。停用的账号要显式启用才该被写操作碰到。
func selectLiveRoomTargets(accounts []core.Account, index int) ([]liveRoomTarget, error) {
	enabled := make([]core.Account, 0, len(accounts))
	for _, account := range accounts {
		if account.Enabled {
			enabled = append(enabled, account)
		}
	}
	if index == 0 {
		out := make([]liveRoomTarget, 0, len(enabled))
		for i, account := range enabled {
			out = append(out, liveRoomTarget{
				index:     i + 1,
				accountID: account.ID,
				label:     liveRoomAccountLabel(account),
			})
		}
		return out, nil
	}
	if index > len(enabled) {
		return nil, fmt.Errorf("--account %d 超出范围（启用中的账号有 %d 个）: %w",
			index, len(enabled), ErrUsage)
	}
	account := enabled[index-1]
	return []liveRoomTarget{{
		index:     index,
		accountID: account.ID,
		label:     liveRoomAccountLabel(account),
	}}, nil
}

// liveRoomAccountLabel 取账号的展示名（昵称优先，退回脱敏手机号，再退回 id）。
func liveRoomAccountLabel(account core.Account) string {
	if account.Nickname != nil && *account.Nickname != "" {
		return *account.Nickname
	}
	if account.PhoneMasked != nil && *account.PhoneMasked != "" {
		return *account.PhoneMasked
	}
	return string(account.ID)
}

// renderLiveRoom 打印结果并定退出码。
//
// # 退出码：只有"真的没跑成"才算运行错误
//
// 「今天没开播」是**正常**结果（门开着才有口令，而门开着与否由服务端数据决定），
// 所以它退 0 —— 与签到里"今天已经签过了"同一档。
// 只有"门那三步走不通"「六步链解析失败」这类才算运行错误。
func renderLiveRoom(out *output, a *LiveRoomArgs, outcomes []liveRoomOutcome) error {
	if a.JSON {
		payload := make([]map[string]any, 0, len(outcomes))
		for _, outcome := range outcomes {
			payload = append(payload, liveRoomJSON(outcome))
		}
		if err := out.json(map[string]any{"liveRoom": payload}); err != nil {
			return err
		}
	} else {
		for _, outcome := range outcomes {
			renderLiveRoomHuman(out, a, outcome)
		}
	}

	// 有任何一个账号失败 ⇒ 运行错误（退出码 1）。
	var failures []string
	for _, outcome := range outcomes {
		if outcome.failure != nil {
			failures = append(failures,
				fmt.Sprintf("[%d] %s: %s", outcome.index, outcome.label, outcome.failure.Message()))
		}
	}
	if len(failures) > 0 {
		return errors.New(strings.Join(failures, "\n"))
	}
	return nil
}

func liveRoomJSON(outcome liveRoomOutcome) map[string]any {
	res := outcome.result
	payload := map[string]any{
		"account":   outcome.label,
		"index":     outcome.index,
		"gateOpen":  res.Gate.Active,
		"liveCount": res.Gate.LiveCount,
		"kouling":   res.Codes,
		"retried":   res.Retried,
		"failed":    outcome.failure != nil,
	}
	if outcome.failure != nil {
		payload["error"] = outcome.failure.Message()
	}
	if report := res.Report; report != nil {
		payload["balanceBefore"] = report.BalanceBefore.FlowerCnt
		payload["balanceAfter"] = report.BalanceAfter.FlowerCnt
		payload["gained"] = report.Gained()
		payload["succeeded"] = report.Succeeded()
		payload["invalidCodes"] = report.InvalidCodes
		payload["expiredCodes"] = report.ExpiredCodes
		payload["otherCodes"] = report.OtherCodes
		payload["revivalFlowers"] = report.RevivalFlowers
		payload["draws"] = liveRoomDrawsJSON(report.Draws)
	}
	return payload
}

// liveRoomDrawsJSON 把每个口令的 `draw` 结果摊平，**带上上游那个原始业务码**。
//
// # 为什么非要有这个字段
//
// 报告上 `200112`（已兑换）是**隐身**的：它既不进 `InvalidCodes`
// （[upstream.RedFlowerDrawOutcome.RejectedKouling] 只认 `200108`，已兑换说明口令本身是对的），
// 也不进 `OtherCodes`（那一栏只收 [upstream.RedFlowerDrawOther]）⇒
// 一个"已兑换"的口令在 `SummaryLine` 里**一行都不会出现**，只剩"成功 0 朵"这个
// 看不出原因的结果。
//
// ⚠ 早先这里写的是"`200112` 与没见过的码都落进未分类那一栏" —— **理由是错的**
// （结论对）：已兑换有自己那一类 [upstream.RedFlowerDrawAlreadyDrawn]，根本不进那栏。
// 真正需要这个字段的理由是上面那句"它在报告里隐身"。
//
// ⭐ 2026-09-23：抓包拿到了第一条真 `200112`
// （`{"code":200112,"msg":"口令已兑换过了，每个口令只能兑换一次!"}`，见 capture §10.2），
// 未验证清单里的这一条已关掉。⭐ 而"这个一次"的适用范围**有答案**：
// **按账号**算 —— **两个不同账号投同一条口令，各自都会成功**（**用户口述**：
// 2026-09-22 两次纠正、2026-09-23 定稿）⇒ ⚠ **不存在"口令被消耗"**。
// ⚠⚠ 早先这里写着"仍无证据，别往下写" —— **那句是错的**（坑表 **#142**）：
// 把"本仓无实拍"当成了"这个说法没依据"。
//
// ⚠ 只出 `upstreamCode`（一个整数），**不出** [upstream.RedFlowerDrawOutcome.Message]：
// 那是上游原文，jsontags 白名单里写明它"能自动 marshal 就离顺手把整个结构写进
// 日志/响应只差一步"。码是证据，原文不是。
//
// `upstreamCode` 为 `null` 表示这一步**没解出业务码**（信封形状不认识）——
// 那与"码是 0"是两件事，所以用 null 而不是 0。
func liveRoomDrawsJSON(draws []app.RedFlowerDrawRecord) []map[string]any {
	out := make([]map[string]any, 0, len(draws))
	for _, draw := range draws {
		entry := map[string]any{"kouling": draw.Code, "upstreamCode": nil}
		if draw.Outcome.Code != nil {
			entry["upstreamCode"] = *draw.Outcome.Code
		}
		out = append(out, entry)
	}
	return out
}

func renderLiveRoomHuman(out *output, a *LiveRoomArgs, outcome liveRoomOutcome) {
	prefix := fmt.Sprintf("[%d] %s", outcome.index, outcome.label)
	res := outcome.result
	if outcome.failure != nil {
		out.human("%s ✗ %s", prefix, outcome.failure.Message())
		return
	}
	if len(res.Codes) == 0 && res.Report == nil {
		// ⚠ 门没开时**不打**这一行。那种情况下 stderr 的 `note` 已经说清是
		// "谁都没在播"还是"别的主播在播"，而 stdout 上再来一句
		// "没有可投的口令"会让人以为是**收**这一步失败了 —— 那是另一条排查线，
		// 而且它指向的是一个根本没被走到过的步骤。
		//
		// 门开着却没收到口令才是这条信息真正要指的事（"听了一分钟，
		// 主播没喊"），它与"今天没开播"的下一步完全不同。
		if res.Gate.Active {
			out.human("%s 开了门但没收到口令", prefix)
		}
		return
	}
	if a.KoulingOnly {
		out.human("%s 收到 %d 个口令：%s", prefix, len(res.Codes), strings.Join(res.Codes, " / "))
		return
	}
	report := res.Report
	if report == nil {
		return
	}
	// 那句话的正文由领域层拼（与服务端 `runs.message` 同一份），这里只加账号前缀。
	out.human("%s %s", prefix, report.SummaryLine())
}
