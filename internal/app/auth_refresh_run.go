// 「刷新 auth 凭据」**跑一个账号**的领域层入口，以及一次刷新的**四态映射**。
//
// # 为什么又是一层，而不是让 [AuthRefresher] 直接充当
//
// [AuthRefresher] 是 HTTP 那枚按钮的常驻形态：一次请求刷一个账号，所以它
// "每次调用现读一次设备档案"是等价的。而编排层是**一次运行 N 个账号、串行跑**，
// 设备档案必须在账号循环**之外**定一次 —— 放进循环会让每个账号拿到不同的
// `deviceShort`，与它登录时那台对不上（[AuthRefresher.Refresh] 里写的正是这件事）。
// 所以本结构体收的是**已经解析好的**那一份，谁解析的、解析几次由调用方决定。
//
// # 这一层与 [RefreshAccountAuth] 的分工
//
// 判据（该不该刷）与落库仍在 [RefreshAccountAuth]，这里只多做一件它没有的事：
// 把结论翻成 [core.RunReport] 的四态。那一层刻意**不**知道四态 —— 它同时服务
// CLI 与账号页那枚按钮，那两处的呈现方式各不相同；只有走编排的这一段需要它。
package app

import (
	"context"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// RefreshRun 是跑一个账号的刷新需要的**世界**。
type RefreshRun struct {
	Store    core.StateStore
	Scope    core.Scope
	Upstream *upstream.Upstream
	Clock    core.Clock
	Random   core.Random

	// Device 是 App 端那一支要的设备档案，由调用方在**账号循环之外**解析一次。
	Device upstream.AppDevice
	// PCPlatform 是 PC 端那一支的平台（`clientType` 与 `DEVICE_INFO` 都依赖它）。
	//
	// ⚠ 库里没记 windows 还是 macos，只能由调用方拍板（同 [RefreshDeps].PCPlatform）。
	// 只在账号真是 PC 端登录时才会被读到，所以这里给个值不算承诺什么。
	PCPlatform upstream.PcPlatform
	// LeadDays 是提前量；0 落到 [capabilities.RefreshDefaultLeadDays]。
	//
	// ⚠ 它是**这台机器替所有人定的那一个数**，不是每行任务各调一份的旋钮 ——
	// 能力声明的参数面里刻意没有它（见 `authRefreshParams`）。
	LeadDays float64

	// Progress 收"给人看的过程话"，可为 nil。
	Progress RunProgress
}

// RunAccount 刷**一个**账号，并如实带回判到的那一档。
//
// ⚠ `Force` 恒为 **false**：这一条链是"到点才换"，而 `force` 的语义是
// "人已经拍板现在就换"（账号页那一枚带确认弹窗的按钮）。走编排的这一刻**没有人**，
// 把它打开等于"每个触发点都硬换一次 token"，而轮换对旧串的影响至今未实测
// （见 `docs/auth-refresh.md` §8）。
func (r RefreshRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (RefreshReport, *core.Error) {
	device := r.Device
	report, perr := RefreshAccountAuth(ctx, &RefreshDeps{
		Upstream:   r.Upstream,
		Store:      r.Store,
		Scope:      r.Scope,
		Clock:      r.Clock,
		Random:     r.Random,
		AppDevice:  &device,
		PCPlatform: r.PCPlatform,
		LeadDays:   r.LeadDays,
	}, account)
	if perr != nil {
		// 出错时也把已经拿到的那一半交回去（端别、判到的档位），理由同
		// [LiveRoomRun.RunAccount]：调用方据此能说出"崩在第几步"。
		return report, perr
	}
	if report.Rotated {
		r.progress(core.LevelInfo, "已换成新凭据（"+report.End+" 端）")
	} else {
		r.progress(core.LevelInfo, report.Decision.String()+"，本次没打上游")
	}
	return report, nil
}

// RunReport 把一次刷新合成领域层结论。
//
// # 四态怎么给
//
//   - **换成了** ⇒ 成功。
//   - **还没到提前量** ⇒ 也是**成功**：这一趟的目标是"到点就换"，而"没到点所以什么都没做"
//     正是它该做的事 —— 与签到里"今天已经签过了"、直播里"今天没开播"同一档。
//   - **只能人工换 auth**（库里那串的到期时刻是负数）⇒ [core.OutcomeManualRequired]。
//     它既不是成功（问题还在）也不是失败（重跑一万次也不会有变化），
//     而这一态是**唯一**能把"得有人去动一下手"传到推送与历史上的通路。
//   - 判到"该刷"却没换成 ⇒ 失败。这一档在 [RefreshAccountAuth] 的成功返回里
//     不该出现（刷不动的那几种都会直接返回错误），但这里**必须**给个落点：
//     Go 的 switch 漏一档不报错，会静默走 default 报成功 —— 那正是本仓库
//     `core/run.go` 文件头点名的"枚举漏态"。
func (res RefreshReport) RunReport() core.RunReport {
	summary := res.SummaryLine()
	switch {
	case res.Rotated:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	case res.Decision == capabilities.RefreshManualOnly:
		account := res.AccountID
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			capabilities.CodeAuthRefresh, &account,
			"这个账号的凭据上游刷不了，请在账号页重新登录一次（或粘贴整串 auth 凭据）",
			core.TargetWebui,
		)}, summary)
	case res.Decision == capabilities.RefreshNotYet:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	default:
		return core.NewRunReportFailed(summary)
	}
}

// SummaryLine 是给人看的一句话小结。
//
// ⚠ 里面**没有**任何凭据形态的东西（令牌、整串 auth、用户域）—— 这句话会进
// `runs.message` 与终局事件，是**出网**的。到期时刻那两个数是 epoch 毫秒的元信息，
// 与凭据无关，但也只对排查有用，界面上那句人话由账号页负责。
func (res RefreshReport) SummaryLine() string {
	switch {
	case res.Rotated:
		return "已换新凭据（" + res.End + " 端）"
	case res.Decision == capabilities.RefreshManualOnly:
		return "上游刷不了这一串凭据，要人工重新登录（" + res.End + " 端）"
	case res.Decision == capabilities.RefreshNotYet:
		return "还没到提前量，本次未刷（" + res.End + " 端）"
	default:
		return "判到该刷但没换成（" + res.End + " 端）"
	}
}

func (r RefreshRun) progress(level core.LogLevel, message string) {
	if r.Progress != nil {
		r.Progress(level, message)
	}
}
