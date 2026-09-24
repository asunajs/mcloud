// 「每日签到」**跑一个账号**的唯一实现。
//
// # 为什么流程住在这里，而不是两个产物各写一遍
//
// CLI 的 `mcloud signin` 与 WebUI / 定时任务跑的是一条链：
// 备档案 → 取凭据 → 换 H5 `deviceId` → 签到 → 记余额 → （可选）领取。
// 这条链上有三处"少写一句就静默出错"的地方：
//
//   - 档案里的 `x-yun-uni`（用户域）必须**从库里那个账号回填**，拿别人的或拿不到
//     的表现是"凭据都在、却回 90001 未登录"；（这一处连同"取凭据 + 拿票"一起
//     收在 [SignInWorld.Source]，几条走签到页的链共用那一份）
//     ⚠ 接了 [SignInSessionSource] 之后这条**不会**被"自动刷票"掩盖：
//     刷的那条会话用的是同一个档案，域不对照样回 90001，最终报的是
//     "jwttoken 刷新后重放仍失败"——刷新只是让同一个错更贵，不是更含糊。
//   - 余额落库取 `infoV3.total`（现行余额），取 `beforeTotal` 会让趋势图变成
//     一条直线（见 [RecordCloud] 那张表）；
//   - 领取不带 H5 `deviceId` 会回 `615` —— 而 `615` 在日志里不像"少了个头"。
//
// 两条产物各写一遍的话，这三处会各自漂移，而漂移**不会让任何测试变红**。
// 所以这里只留一份。（CLI 那一侧目前仍走 `internal/cli/run_signin.go` 自己的编排 ——
// 它带着五个分支与逐字对位 Rust 的输出契约，收拢成一份是独立一轮，见 `docs/status.md`。）
//
// # 这一层不做什么
//
//   - **不判"今天签没签"** —— 判据是 [SignInReport.SignedToday]（一律回查 `infoV3`）；
//   - **不判"该领哪几笔"** —— [PlanReceive]；
//   - **不解析响应** —— `internal/upstream`；
//   - **不打印、不知道账号是第几个** —— 过程话走 [SignInRun.Progress]。
package app

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// SignInRun 是跑一个账号需要的**世界**。
//
// `Device` 由调用方在**账号循环之外**解析一次：同一个安装就该是同一台设备，
// 放进循环里会让每个账号拿到不同的 `device_short`（与登录时那台对不上）。
type SignInRun struct {
	Store    core.StateStore
	Scope    core.Scope
	Upstream *upstream.Upstream
	Device   upstream.AppDevice
	Clock    core.Clock
	Random   core.Random

	// Receive 是"签到之后顺手把该领的领掉"。
	//
	// ⚠ 它是**必选项而不是默认值**：调用点必须显式写，因为两档都不是安全的缺省 ——
	// 关掉它，能力声明里那句"并领取奖励"就名不副实；打开它，
	// 每一次签到都会多出"取 `infoV3` + 若干次 `receiveV3`"的写请求。
	// CLI 那条把它开成 `--receive`，服务端与调度这一路固定打开（见 `internal/server`）。
	Receive bool

	// Progress 收"给人看的过程话"，可为 nil（理由同 [LiveRoomRun.Progress]）。
	Progress RunProgress

	// Sessions 是进程内的会话票池：同一账号在签到页这一份票**跨轮复用**，
	// 于是界面上连点三次签到只建一条会话（口径与理由见 `signin_session_pool.go` 头注）。
	//
	// 可为 nil ⇒ 回落成"这一轮那一条"（CLI 单次调用与测试用；服务端那条路**必须**接，
	// 接线点见 `internal/server/run_dispatch.go`）。
	Sessions *SessionPool
}

// SignInAccountResult 是一个账号这一趟的全部产出。
//
// ⚠ `Received` 与 `ReceiveFailure` **不是**账号的成败：签到本身成了，
// 这一趟就不算彻底失败。它们只决定 [SignInAccountResult.RunReport] 落在
// 四态里的哪一格。
type SignInAccountResult struct {
	// Report 是签到那一步的结论（含签到前后的 `infoV3`）。
	Report SignInReport
	// Received 是领取逐条实况；`Receive` 关掉时为 nil。
	Received []ReceiveOutcome
	// ReceiveFailure 是**整批**领取失败的原因（例如取 `infoV3` 就挂了）。
	//
	// 它与 `Received` 里那些 [ReceiveOutcomeFailed] 是两件事：那条是"逐条各自失败"，
	// 这一条是"一批都没走到"。
	ReceiveFailure string
	// Cloud 是余额落库那条快照；空串理由见 [SignInRun.RunAccount] 里那一步。
	Cloud *core.CloudStats
	// CloudFailure 是余额没落库的原因。**不影响成败**，只影响过程话。
	CloudFailure string
}

// RunAccount 跑**一个账号**：备档案 → 取凭据 → 换 `deviceId` → 签到 → 记余额 → 领取。
//
// 返回的 `SignInAccountResult` 在出错时也尽量带上已经拿到的那一段（签到结论），
// 调用方据此能说出"崩在第几步"，而不是一句错误串。
func (r SignInRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (SignInAccountResult, *core.Error) {
	// 装配那三步（回填用户域 → 取凭据 → 拿票）在 [SignInWorld.Source]：
	// 与"只领不签""只查不签"那几条链**共用一份**，理由写在那文件的头注。
	// 这一趟用的票在 source 里：签到与领取都向它要，所以整趟最多建一条会话
	// （旧写法是每步各开一条）。接了池时它可能**比这一趟更老** —— 上一次点击建的那条
	// 还没被上游判死，就继续用它（口径见 `signin_session_pool.go`）。
	src, perr := r.world().Source(ctx, account)
	if perr != nil {
		return SignInAccountResult{}, perr
	}

	// H5 `deviceId`：**先换再签**，而不是"要领取时才换"。
	//
	// 签到不带它也能签上（2026-09-19 实测），但领取不带会回 `615` ——
	// 所以这一趟只要开了 `Receive` 就必须有它。换不到时**降级继续**（不整趟失败）：
	// 那是风控侧的问题，签到本身仍然做得完，而把整趟判失败会连带丢掉一次成功的签到。
	deviceID, perr := r.world().DeviceIDFor(ctx, account)
	if perr != nil {
		if r.Receive {
			// 说清"这次没有它 ⇒ 领取会回 615"，否则 `615` 会变成一个没人看得懂的码。
			r.progress(core.LevelWarn, fmt.Sprintf(
				"⚠ 取 H5 deviceId 失败，本次领取大概率回 615：%s", perr.Error()))
		}
		return r.finish(ctx, account, src, nil)
	}
	return r.finish(ctx, account, src, &deviceID)
}

// finish 是"凭据与档案都齐了"之后的那一段：签到 → 记余额 → 领取。
//
// 单独成函数只为了让上面那条降级支路不必把三步抄两遍（抄两遍的话，
// 第二步将来只会修在其中一条上）。
//
// `src` 在这一趟**开头**建（[SignInRun.RunAccount]），签到与领取共用它：
// 一轮只有一条 `jwttoken`（口径见 `signin_session.go` 头注）。
func (r SignInRun) finish(
	ctx context.Context,
	account core.AccountID,
	src *SignInSessionSource,
	deviceID *string,
) (SignInAccountResult, *core.Error) {
	r.progress(core.LevelInfo, "以 iOS App 身份签到…")
	report, perr := CloudSignIn(ctx, src,
		// ⚠ `Force` **不给**：它是协议自检那一档（今天已签也发一次写请求），
		// 定时任务天天跑，开了它等于每天多打一次没人要的写请求、多记一次风控数。
		SignInOptions{DeviceID: deviceID})
	if perr != nil {
		return SignInAccountResult{Report: report}, perr
	}
	outcome := SignInAccountResult{Report: report}

	// 余额是**顺带采集**的副产品：写不进去不该把一次成功的签到变成失败
	// （CLI 那侧同一件事退化成一句 stderr 警告）。但也不能静默 ——
	// 悄悄不采集的表现是趋势图上少一段，而几个月后没人会想到写库失败。
	//
	// ⚠ 取 `After.BeanTotal`（现行余额），不是 `Before`、更不是 `BeforeTotal`。
	stats, cerr := RecordCloud(ctx, r.Store, r.Scope, account, report.After.BeanTotal, r.Clock)
	if cerr != nil {
		outcome.CloudFailure = cerr.Error()
		r.progress(core.LevelWarn, "⚠ 余额没能落库（不影响本次签到）："+cerr.Error())
	} else {
		outcome.Cloud = &stats
	}

	if !r.Receive {
		return outcome, nil
	}
	// 领取**复用**上面那条会话，不再另开一条：它读的仍是"签到之后"的账目
	// （刚签上的那一笔要出现在 `receiveList` 里才算数），但票可以是同一张 ——
	// 票本来活 2 小时，[CloudSignIn] 那一步刚把它验过一次。
	device := ""
	if deviceID != nil {
		device = *deviceID
	}
	outcome.Received, perr = ReceiveAll(ctx, src, device)
	if perr != nil {
		// ⚠ 这里**不**把 perr 交回去：签到那一半已经真的成了，整趟报失败会
		// 让调度把一次成功的签到记成 error，而下一轮补跑会**再签一次**。
		outcome.ReceiveFailure = perr.Error()
		outcome.Received = nil
	}
	return outcome, nil
}

// RunReport 把这一趟的产出合成领域层结论。
//
// # 四态怎么给
//
//   - **今天本来就签过了** ⇒ **成功**。与直播那条链上"今天没开播"同一档：
//     来一趟、确认今天已经办了，就是这一趟的目标达成了。
//   - 签上了、领取**没有失败项**（包括"没有可领的"）⇒ 成功。
//   - 签上了但**有领取失败** ⇒ **部分成功**。签到不可撤回，重跑这一趟
//     只会"今天已经签过 + 再领一次"，所以它既不是失败（有产出）也不该报成功
//     （声明里那半句"并领取奖励"没做到）。
//
// ⚠ 这里**不放** [core.ManualStep]：这条链上没有"要人去客户端点一下"就能继续的动作。
// 领取失败里确实有一类是"未开备份"（`1302`），但那一档的判据在
// [ClaimMultiple] 那条月度链上，本函数没有调它 —— 不在这里凭错误串猜。
func (res SignInAccountResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	switch {
	case res.Report.Result == nil && !res.Report.SignedToday():
		// 没发写请求、回查又说今天没签 —— 这一趟什么都没办成。
		return core.NewRunReportFailed(summary)
	case res.ReceiveFailure != "" || res.failedReceives() > 0:
		return core.RunReport{
			Outcome: core.OutcomePartialSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	default:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
}

// SummaryLine 是给人看的一句话小结（终局事件与 `runs.message` 用的就是它）。
func (res SignInAccountResult) SummaryLine() string {
	head := "今天已经签过"
	if res.Report.SignedNow() {
		head = "本次签到成功"
		if points := res.Report.SignInPoints(); points != nil {
			head += fmt.Sprintf("（+%d 豆）", *points)
		}
	}
	if !res.Report.SignedToday() {
		head = "签到未确认（回查 infoV3 今天仍未签）"
	}
	parts := []string{fmt.Sprintf("%s，本月已签 %d 天", head, res.Report.MonthDays())}
	if res.Cloud != nil {
		parts = append(parts, fmt.Sprintf("余额 %d 豆", res.Cloud.CloudCount))
	}
	if res.CloudFailure != "" {
		parts = append(parts, "余额没落库")
	}
	if received := res.receivedTotal(); received > 0 {
		parts = append(parts, fmt.Sprintf("领取 %d 豆", received))
	}
	if failed := res.failedReceives(); failed > 0 {
		parts = append(parts, fmt.Sprintf("%d 笔领取失败", failed))
	}
	if res.ReceiveFailure != "" {
		parts = append(parts, "领取整批失败")
	}
	return strings.Join(parts, "；")
}

// failedReceives 是逐条失败的那几笔。
func (res SignInAccountResult) failedReceives() int {
	n := 0
	for _, item := range res.Received {
		if item.Kind == ReceiveOutcomeFailed {
			n++
		}
	}
	return n
}

// receivedTotal 是这次领到的合计（上游没报数的那些按 0 计，不猜）。
func (res SignInAccountResult) receivedTotal() int64 {
	var total int64
	for _, item := range res.Received {
		switch item.Kind {
		case ReceiveOutcomeImmediate:
			total += item.Received
		case ReceiveOutcomeMonthlyExpansion:
			if item.CloudCount != nil {
				total += *item.CloudCount
			}
		}
	}
	return total
}

// SignInRunFrom 从一份 [SignInWorld] 造出签到那条链（`Receive` 仍由调用点显式给）。
//
// 为什么要有它：装配世界的那八个字段现在由服务端一处、CLI 一处填，
// 再让每个调用点手抄一遍 `SignInRun{Store: w.Store, ...}` 的话，
// **漏抄一个字段不会报错**（Go 允许少写字段），而漏掉 `Sessions` 的表现
// 恰好是"界面上连点三次签到 = 重打三枪"。
func SignInRunFrom(w SignInWorld, receive bool) SignInRun {
	return SignInRun{
		Store:    w.Store,
		Scope:    w.Scope,
		Upstream: w.Upstream,
		Device:   w.Device,
		Clock:    w.Clock,
		Random:   w.Random,
		Receive:  receive,
		Progress: w.Progress,
		Sessions: w.Sessions,
	}
}

// world 把这份装配交成 [SignInWorld] 的形状（字段同名同义，逐字搬一遍）。
//
// 为什么不直接内嵌：内嵌会把 `SignInRun` 的**构造点**改成 `World: SignInWorld{...}`，
// 而 CLI、服务端与一批测试都在按字段名平铺填它。多一个 accessor 换的是"构造点不动"。
func (r SignInRun) world() SignInWorld {
	return SignInWorld{
		Store:    r.Store,
		Scope:    r.Scope,
		Upstream: r.Upstream,
		Device:   r.Device,
		Clock:    r.Clock,
		Random:   r.Random,
		Sessions: r.Sessions,
		Progress: r.Progress,
	}
}

func (r SignInRun) progress(level core.LogLevel, message string) {
	if r.Progress != nil {
		r.Progress(level, message)
	}
}
