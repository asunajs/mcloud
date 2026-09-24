package app

// 「消息推送任务」（旧项目 code `msg-push`）：查状态 → `secondTaskStatus == 2` 才领。
//
// ⚠⚠ **这一族零样本**：URL / 方法 / body / 字段名全部来自
// `docs/business-feature-inventory.md` §2.3 对旧 TS 的**文字描述**，
// `test/fixtures/http/` 里一个 `msgPushOn` 都没有。开源 Go 参考实现**有**这一族
// （`docs/caiyun-go-feature-inventory.md` §2 第 90 行），但它那两篇只记行为、
// 不记端点与字段 ⇒ 形状口径仍只有旧 TS 一处。
// 完整的三方核对、以及"它说『未开启只 Warn、保持绿』而我们落第四态"这一处
// **刻意不照抄**的理由，写在 `internal/upstream/app_msg_push.go` 文件头（那一份才是
// 口径的出处，这里不重复）。
//
// ⇒ 因此本文件的每一条判据都写成"旧项目说……"，**不**写成"上游会……"。
// 唯一的例外是那条**拼写**（`onDuaration`）：它是"照抄上游的错键名"这条纪律的对象，
// 由 `app_msg_push_test.go` 钉住，不许被"改对"。
//
// # 为什么这里不发第二个请求去复查（与公告任务那条**相反**）
//
// [ClaimNotice] 领完要再查一次，是因为它至少**有**一个可查的读数口。
// 这一条复查要看的是同一个 `task/status`，而 `obtain` 之后它会不会变**零样本** ——
// 为一个零样本接口多打一次零样本请求，换来的读数解释不了自己，只会把
// "两次读数不同"变成新的疑点。要加，等抓包说明 `obtain` 前后状态怎么变之后再加。

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// msgPushCapabilityCode 是这条能力在注册表里的 code。
//
// 写在子代理那一批文件里时注册表还没有这一条（接线归编排层），当时是**字面量**；
// 现在指回 [capabilities.CodeMsgPush] —— 与 [noticeCapabilityCode] 同一处置：
// 注册表是 code 的唯一出处，这里再留一份字面量就会和声明各自漂移。
const msgPushCapabilityCode = capabilities.CodeMsgPush

// ─────────────────────────────────────────────────────────────────────────────
// 判定
// ─────────────────────────────────────────────────────────────────────────────

// MsgPushPlan 是 [PlanMsgPush] 的结论档位。
type MsgPushPlan int

const (
	// MsgPushPlanObtain 是该领第二次奖励：`secondTaskStatus == 2`。
	MsgPushPlanObtain MsgPushPlan = iota
	// MsgPushPlanManual 是 `pushOn == 0` —— 通知本身没开，得有人去 App 里打开。
	MsgPushPlanManual
	// MsgPushPlanNothing 是判据给了、但不是 `2` ⇒ 现在不该领（算成功）。
	MsgPushPlanNothing
	// MsgPushPlanUnknown 是**判据字段缺失** ⇒ 一个写请求都不发。
	MsgPushPlanUnknown
)

// 下面三个数是这一族**全部**的判据，每一个都标着"旧项目说的"。
//
// ⚠ 集中成常量是因为它们只该有一处定义：`MsgPushObtainBodyType`（发出去的 `type`）
// 与 [msgPushSecondTaskStatusClaimable]（触发发的那个状态）**必须是同一个 2** ——
// 旧项目那份描述里两处都是 2，抄成两个不同数字的话，这条链就永远不发。
const (
	// msgPushPushOnOff 读作"消息通知已关闭"（旧项目：`pushOn === 0` ⇒ 报错并 return）。
	msgPushPushOnOff int64 = 0
	// msgPushSecondTaskStatusClaimable 读作"第二次奖励该领"（旧项目：`=== 2` 才发 obtain）。
	msgPushSecondTaskStatusClaimable int64 = 2
	// msgPushFirstTaskStatusDone 读作"首次奖励已领"（旧项目：`!== 3` ⇒ 只打一句 info）。
	// ⚠ 这一条**不影响成功**，所以我们也不把它做成待办（见 [PlanMsgPush] 的顺序说明）。
	msgPushFirstTaskStatusDone int64 = 3
)

// PlanMsgPush 按 `task/status` 的读数判 [MsgPushPlan]。**纯函数**，不碰网络。
//
// # 顺序就是旧项目的顺序
//
// 描述逐字是：先 `if (pushOn === 0) { logger.error(...); return; }` —— 即它是**前置**开关，
// 关着的时候连 `secondTaskStatus` 都不必看。之后才是 `secondTaskStatus === 2` 才发。
//
// # 为什么 `pushOn` 缺失**不**判 Manual
//
// 旧项目用的是 `=== 0`（严格相等）：`undefined === 0` 是 `false` ⇒ 它往下走。
// 我们照这个口径，而且它本来也是更诚实的那个：`pushOn` 没给**不等于**给了 0，
// 把"上游没说"落成"要人去开通知"是**凭空造一条待办** ——
// 用户会为了一个可能根本没坏的东西去翻 App 设置。
// 反向后果（少提示一次）只在"真的关了但上游没报"时发生，而那一形态**零样本**。
func PlanMsgPush(status upstream.MsgPushStatus) MsgPushPlan {
	if pushOn, ok := msgPushInt(status.PushOn); ok && pushOn == msgPushPushOnOff {
		return MsgPushPlanManual
	}
	second, ok := msgPushInt(status.SecondTaskStatus)
	if !ok {
		return MsgPushPlanUnknown
	}
	if second == msgPushSecondTaskStatusClaimable {
		return MsgPushPlanObtain
	}
	return MsgPushPlanNothing
}

// msgPushFirstRewardPending 报告"首次奖励还没领"（旧项目在这里只打一句 info）。
//
// ⚠ 判据字段缺失时回 `false`：旧项目写的是 `!== 3`，`undefined !== 3` 在 JS 里是
// `true` ⇒ 它**会**打那句 info。我们反过来选"不提示"，因为这句提示的落点不是日志
// 而是界面上的一句话，而"上游没给 firstTaskStatus"不该被念成"首次奖励没领"。
// 这一处**主动偏离**旧项目，理由是那句提示在旧项目里没有对应后果（不影响成功），
// 而我们这句话会被当事实读。
func msgPushFirstRewardPending(status upstream.MsgPushStatus) bool {
	first, ok := msgPushInt(status.FirstTaskStatus)
	return ok && first != msgPushFirstTaskStatusDone
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// MsgPushOutcomeKind 是 [MsgPushOutcome] 的结局档位。
type MsgPushOutcomeKind int

const (
	// MsgPushOutcomeNone 是"没走到能报结论的那一步"（查状态就挂了）。刻意占**零值**，
	// 理由同 [NoticeOutcomeNone]：别让一条提前 return 的错误路径被念成"领到了"。
	MsgPushOutcomeNone MsgPushOutcomeKind = iota
	// MsgPushOutcomeObtained 是领到了（`obtainCode == 0`）。
	MsgPushOutcomeObtained
	// MsgPushOutcomeNothing 是不该领 —— 没发写请求。
	MsgPushOutcomeNothing
	// MsgPushOutcomeManual 是通知没开 —— 没发写请求。
	MsgPushOutcomeManual
	// MsgPushOutcomeUnknown 是判据缺失 —— 没发写请求。
	MsgPushOutcomeUnknown
	// MsgPushOutcomeFailed 是 `obtain` 发了但没成（含"发了、上游没回 `obtainCode`"）。
	//
	// 不返回错误：请求**已经发出去了**，这一趟的实况是"发过、判不出成"。
	// 报成错误会把"发过"抹掉，而"发过"恰恰是将来对账时唯一要找的那件事
	// （理由同 [MultipleOutcomeFailed]）。
	MsgPushOutcomeFailed
)

// MsgPushOutcome 是 [ClaimMsgPush] 的产出。
type MsgPushOutcome struct {
	// Kind 决定下面哪些字段有意义。
	Kind MsgPushOutcomeKind
	// Status 是**发写请求之前**那份 `task/status` 读数（判据、以及界面上念的那几个数）。
	Status upstream.MsgPushStatus
	// ObtainCode 是 `result.obtainCode` 的读数。nil = **上游没给**（≠ 0，≠ 成功）。
	ObtainCode *int64
	// FirstRewardPending 是"首次奖励未领"那句话要不要说（旧项目：只打 info、不影响成功）。
	FirstRewardPending bool
	// Error 是失败原因（[MsgPushOutcomeFailed]）。
	Error string
}

// ClaimMsgPush 跑这一族的链：`task/status` → 判 → （仅当 `secondTaskStatus == 2`）`task/obtain`。
//
// # 三种失败的走法（与 [ClaimNotice] 同一套口径）
//
//   - **查状态失败** ⇒ 交回错误，结局是 [MsgPushOutcomeNone]。
//   - **`obtain` 失败**（网络 / 非 0 码）⇒ 折成 [MsgPushOutcomeFailed]，**不**回错误。
//   - **`obtainCode` 缺失或非 0** ⇒ 同样折成 [MsgPushOutcomeFailed]。
//     ⚠ 缺 `obtainCode` 也算"没成"而不是"成了"：写请求已经发出去了，
//     把"判不了"伪造成"成功"，用户就再也不会去查这一笔。
//
// # 这一条链**不会**自己跑起来
//
// 只有 `MsgPushRun.RunAccount` 经过编排层显式点名 `msg-push` 才会走到这里；
// 本文件没有任何 `init` / 后台 goroutine ⇒ 不存在"没接线就打到真账号上"的路径。
// 在拿到第一批样本之前，接线那一步应当带上二次确认（口径同 `RiskHigh` 那几条）。
func ClaimMsgPush(
	ctx context.Context, src *SignInSessionSource,
) (MsgPushOutcome, *core.Error) {
	status, perr := MsgPushStatus(ctx, src)
	if perr != nil {
		return MsgPushOutcome{}, perr
	}
	outcome := MsgPushOutcome{Status: status}

	switch plan := PlanMsgPush(status); plan {
	case MsgPushPlanManual:
		outcome.Kind = MsgPushOutcomeManual
		return outcome, nil
	case MsgPushPlanNothing:
		outcome.Kind = MsgPushOutcomeNothing
		return outcome, nil
	case MsgPushPlanUnknown:
		outcome.Kind = MsgPushOutcomeUnknown
		return outcome, nil
	case MsgPushPlanObtain:
	}
	outcome.FirstRewardPending = msgPushFirstRewardPending(status)

	obtained, perr := do(ctx, src, func(s SignInSession) (upstream.MsgPushObtain, *core.Error) {
		return src.up.AppMsgPushObtain(ctx, s.jwt, s.referer)
	})
	if perr != nil {
		outcome.Kind = MsgPushOutcomeFailed
		outcome.Error = perr.Error()
		return outcome, nil
	}
	outcome.ObtainCode = obtained.ObtainCode
	switch code, ok := msgPushInt(obtained.ObtainCode); {
	case !ok:
		outcome.Kind = MsgPushOutcomeFailed
		outcome.Error = "上游没回 obtainCode，判不出这次到底成没成"
	case code == 0:
		outcome.Kind = MsgPushOutcomeObtained
	default:
		outcome.Kind = MsgPushOutcomeFailed
		outcome.Error = fmt.Sprintf("上游回 obtainCode=%d", code)
	}
	return outcome, nil
}

// MsgPushStatus 只查状态、不领（给"看一眼"那种用法留着）。
func MsgPushStatus(
	ctx context.Context, src *SignInSessionSource,
) (upstream.MsgPushStatus, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.MsgPushStatus, *core.Error) {
		return src.up.AppMsgPushStatus(ctx, s.jwt, s.referer)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// MsgPushVerdict 是一次推送任务的结局档位。
//
// ⚠ 名字不叫 `MsgPushStatus`：那个名字已经是上游那个**读数结构**
// （[upstream.MsgPushStatus]），两件事共用一个名字会让人读 `MsgPushResult` 时
// 分不清"档位"还是"读数"。
type MsgPushVerdict int

const (
	// MsgPushDone 是领到了。
	MsgPushDone MsgPushVerdict = iota
	// MsgPushNothing 是不该领 —— 算成功。
	MsgPushNothing
	// MsgPushManual 是得有人去 App 里打开消息通知。
	MsgPushManual
	// MsgPushUnknown 是判据缺失（没发请求）。与 [MsgPushNothing] 分开是因为
	// 一句有根据、一句没根据（同 [NoticeUnknown] 的理由）。
	MsgPushUnknown
	// MsgPushFailed 是 `obtain` 发了但没成。
	MsgPushFailed
)

// MsgPushRun 跑一次消息推送任务。
type MsgPushRun struct {
	SignInWorld
}

// MsgPushResult 是一个账号这一趟的产出。
//
// # 为什么把三个读数也带出来
//
// 这一族**零样本**：将来排查时唯一能比对的就是"上游当时到底给了什么数"。
// 只留一个档位，等于把第一批真跑读数扔掉、再靠猜走第二遍。
// 念的时候仍按读数念（见 [MsgPushResult.SummaryLine]），不解释语义。
type MsgPushResult struct {
	Account core.AccountID
	Verdict MsgPushVerdict
	// ObtainCode 是 `result.obtainCode` 读数；nil = 上游没给（≠ 0）。
	ObtainCode *int64
	// PushOn / FirstTaskStatus / SecondTaskStatus 是 `task/status` 的原样读数。
	PushOn            *int64
	FirstTaskStatus   *int64
	SecondTaskStatus  *int64
	FirstRewardNotice bool
	// Reason 是 [MsgPushNothing] / [MsgPushManual] / [MsgPushUnknown] 的人话依据。
	Reason string
	// Error 是 [MsgPushFailed] 的失败原因。
	Error string
}

// RunAccount 跑**一个账号**的这一笔。
//
// 取不到票 / 查状态失败 ⇒ 交回错误；判定之后的结果落成 [MsgPushResult] 的档位。
func (r MsgPushRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (MsgPushResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return MsgPushResult{Account: account}, perr
	}
	outcome, perr := ClaimMsgPush(ctx, src)
	result := r.project(account, outcome)
	if perr != nil {
		result.Error = perr.Error()
		return result, perr
	}
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// project 把能力层的结局投影成跑一趟的结论（两条出口共用一份，理由同
// [NoticeRun.project]）。
func (r MsgPushRun) project(
	account core.AccountID, outcome MsgPushOutcome,
) MsgPushResult {
	result := MsgPushResult{
		Account:           account,
		ObtainCode:        outcome.ObtainCode,
		PushOn:            outcome.Status.PushOn,
		FirstTaskStatus:   outcome.Status.FirstTaskStatus,
		SecondTaskStatus:  outcome.Status.SecondTaskStatus,
		FirstRewardNotice: outcome.FirstRewardPending,
	}
	switch outcome.Kind {
	case MsgPushOutcomeObtained:
		result.Verdict = MsgPushDone
	case MsgPushOutcomeNothing:
		result.Verdict = MsgPushNothing
		result.Reason = "secondTaskStatus 不是该领的那个值"
	case MsgPushOutcomeManual:
		result.Verdict = MsgPushManual
		result.Reason = "pushOn=0，消息通知是关着的"
	case MsgPushOutcomeUnknown:
		result.Verdict = MsgPushUnknown
		result.Reason = "上游没给 secondTaskStatus，判不了，本次没发请求"
	case MsgPushOutcomeFailed:
		result.Verdict = MsgPushFailed
		result.Error = outcome.Error
	case MsgPushOutcomeNone:
		// 连状态都没读到：一个写请求都没发，这里的"失败"说的是"这一趟没办成"。
		result.Verdict = MsgPushFailed
	}
	return result
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 只有档位与照抄的读数，没有任何凭据形态的东西；读数**不解释** ——
// 这一族连"pushOn 有几种取值"都没样本，翻成中文就是编。
func (res MsgPushResult) SummaryLine() string {
	head := "消息推送任务"
	note := ""
	if res.FirstRewardNotice {
		// 旧项目这一句是 info 级、**不影响成功**（`docs/business-feature-inventory.md`
		// §2.3 `msg-push`）。我们照这个口径：它只进小结，**不**升成待办。
		note = "；另外首次奖励看着也没领"
	}
	switch res.Verdict {
	case MsgPushDone:
		return head + "已领（" + res.readings() + "）" + note
	case MsgPushNothing, MsgPushManual, MsgPushUnknown:
		text := head + "没领：" + res.Reason + "（" + res.readings() + "）" + note
		if res.Verdict == MsgPushManual {
			text += "，要去 App 里打开消息通知"
		}
		return text
	}
	if res.Error == "" {
		return head + "失败"
	}
	return head + "失败：" + res.Error
}

// readings 把三个判据读数念成一句短语。缺的键念"缺"、不念 0
// （口径同 [noticeSlotLabel]：把"没说"写成"说了 0"是最难发现的一类假话）。
func (res MsgPushResult) readings() string {
	parts := make([]string, 0, 3)
	for _, field := range []struct {
		key   string
		value *int64
	}{
		{"pushOn", res.PushOn},
		{"firstTaskStatus", res.FirstTaskStatus},
		{"secondTaskStatus", res.SecondTaskStatus},
	} {
		if value, ok := msgPushInt(field.value); ok {
			parts = append(parts, fmt.Sprintf("%s=%d", field.key, value))
			continue
		}
		parts = append(parts, field.key+"=缺")
	}
	return strings.Join(parts, " ")
}

// RunReport 把档位映射到 [core.RunReport]。
//
// [MsgPushManual] 是这一条唯一的非成功非失败档：它是**唯一**能把
// "得有人去 App 里点一下"传到推送与历史列表的通路。
// 与公告任务那条不同，这一档**今天可达** —— 判据（`pushOn == 0`）是旧项目逐字写的，
// 只是**没有样本**：真跑时如果上游把"关了"报成别的数，这里会漏提示而不是错提示。
func (res MsgPushResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	switch res.Verdict {
	case MsgPushManual:
		account := res.Account
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			msgPushCapabilityCode, &account,
			"去移动云盘 App 里打开「消息通知」，通知关着的时候这条任务不会入账；"+
				"本工具改不了 App 的系统通知开关，代劳不了这一步",
			core.TargetMobileApp,
		)}, summary)
	case MsgPushFailed:
		return core.NewRunReportFailed(summary)
	default:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
}

// msgPushInt 把 `*int64` 拆成「值 + 有没有给」（与 [noticeInt] 同一判据）。
func msgPushInt(value *int64) (int64, bool) {
	if value == nil {
		return 0, false
	}
	return *value, true
}
