package app

// 签到**翻倍**与**备份好礼** —— 两条**互相独立**的每月一次收入。
//
// 两者都以「开了自动备份」为前提，所以 `state = -1` 时两条会一起拿不到，
// 但**别因此把它们合并**：它们的接口、周期、门槛字段都不是同一个。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 签到翻倍（`multiple`）—— 旧项目 `mcloud-sign` 的 `executeSignInMultiple`
// ─────────────────────────────────────────────────────────────────────────────

// MultiplePlan 是签到翻倍**能不能领** ——
// 判据逐字来自旧项目 `mcloud-sign`（`packages/core/src/tasks/sign/task.ts`）：
//
//	if (curMonthBackup && !curMonthBackupSignAccept) { await executeSignInMultiple(ctx); }
//
// 两个值都在 `infoV3` 里，见 [upstream.SignInStatus.CurMonthBackup] /
// [upstream.SignInStatus.CurMonthBackupSignAccept]。
//
// # 为什么要有这个枚举，而不是在调用点写个 `if`
//
// **每月只有一次机会** —— 条件不成立还发，是白白消耗掉这个月的名额。
// 而"条件不成立"有三种完全不同的原因（已领过 / 没开备份 / 上游没说），
// 它们的**处置相同、说法不同**。做成枚举就能：① 单独测；② 原样报给用户，
// 而不是笼统一句"跳过"。
type MultiplePlan int

const (
	// MultiplePlanClaim 是**可以领**：本月开着备份，且这一次机会还没用掉。
	MultiplePlanClaim MultiplePlan = iota
	// MultiplePlanAlreadyAccepted 是本月**已经领过** —— 不再发请求。
	MultiplePlanAlreadyAccepted
	// MultiplePlanNoBackup 是本月**没开自动备份** —— 翻倍机会根本没产生。
	//
	// ⚠ 旧项目在这种情况下会**上传一个文件把备份打开**再重试
	// （`backFile` + 等 `backupWaitTime`，默认 20 秒）。我们**没有网盘上传能力**，
	// 所以这里只如实报告，**不假装做过**。要补的是"上传"那条线，不是这条判定。
	MultiplePlanNoBackup
	// MultiplePlanUnknown 是上游**没给**这两个字段 —— 判不了。
	//
	// 单独成一档是为了**安全**：缺失既不是"开了"也不是"没开"。
	// 当成"开了"会白发一个写请求、把名额用掉；当成"没开"会漏领。
	// 所以这一档的处置是**什么都不发**，并把这个"判不了"如实报出来。
	MultiplePlanUnknown
)

// PlanMultiple 按 `infoV3` 的两个门槛字段判 [MultiplePlan]。
//
// ⚠ 匹配顺序有意义：`(true, true)` 要排在 `(false, _)` 前面 ——
// 两个都"不领"，但"本月已领过"是更具体的说法，报给用户更有用。
func PlanMultiple(status upstream.SignInStatus) MultiplePlan {
	backup, hasBackup := derefBool(status.CurMonthBackup)
	accept, hasAccept := derefBool(status.CurMonthBackupSignAccept)
	switch {
	case hasBackup && backup && hasAccept && !accept:
		return MultiplePlanClaim
	case hasBackup && backup && hasAccept && accept:
		return MultiplePlanAlreadyAccepted
	case hasBackup && !backup:
		return MultiplePlanNoBackup
	}
	return MultiplePlanUnknown
}

// MultipleOutcome 是领签到翻倍的结果。
type MultipleOutcome struct {
	// Kind 决定下面哪些字段有意义。
	Kind MultipleOutcomeKind
	// CloudCount 是领到的合计。nil = 上游没说领到多少，**不是 0**。
	CloudCount *int64
	// Multiple 是**倍数**（`N` 倍），与 `CloudCount`（合计）不是一个量。
	Multiple *int64
	// Error 是失败原因（[MultipleOutcomeFailed]）。
	Error string
}

// MultipleOutcomeKind 是 [MultipleOutcome] 的五种结局。
type MultipleOutcomeKind int

const (
	// MultipleOutcomeClaimed 是领到了。
	MultipleOutcomeClaimed MultipleOutcomeKind = iota
	// MultipleOutcomeAlreadyAccepted 是本月已领过 —— 没发请求。
	MultipleOutcomeAlreadyAccepted
	// MultipleOutcomeNoBackup 是本月没开自动备份 —— 没发请求。
	MultipleOutcomeNoBackup
	// MultipleOutcomeUnknown 是门槛字段上游没给 —— 没发请求。
	MultipleOutcomeUnknown
	// MultipleOutcomeFailed 是门槛成立、请求也发了，但**失败**（例如回 `1302`）。
	//
	// 不返回错误：这个开关的用途恰恰是"看上游到底回什么码"，
	// 返回错误就只剩一句错误信息。与 [ReceiveOutcomeFailed] 同策。
	MultipleOutcomeFailed
)

// ClaimMultiple 领签到翻倍（`multiple`）—— **写请求**，每月一次。
//
// 自己先取一次 `infoV3` 判门槛（[PlanMultiple]），不成立就**一个请求都不发**。
//
// # 它解决什么
//
// 「自动备份」每月送一次翻倍机会，把**今日签到奖励**翻 2–50 倍。
// 旧项目 `mcloud-sign` 把它放在 `signIn` 里顺手做掉（`executeSignInMultiple`），
// 很容易被忽略 —— 而它是一条**独立的、每月一次的**收入。
func ClaimMultiple(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	clock core.Clock,
	rng core.Random,
) (MultipleOutcome, *core.Error) {
	referer, jwt, perr := OpenSignInSession(ctx, up, profile, credential, clock, rng)
	if perr != nil {
		return MultipleOutcome{}, perr
	}
	status, perr := up.AppSignInStatus(ctx, jwt, referer)
	if perr != nil {
		return MultipleOutcome{}, perr
	}

	if plan := PlanMultiple(status); plan != MultiplePlanClaim {
		switch plan {
		case MultiplePlanAlreadyAccepted:
			return MultipleOutcome{Kind: MultipleOutcomeAlreadyAccepted}, nil
		case MultiplePlanNoBackup:
			return MultipleOutcome{Kind: MultipleOutcomeNoBackup}, nil
		default:
			return MultipleOutcome{Kind: MultipleOutcomeUnknown}, nil
		}
	}

	result, perr := up.AppClaimMultiple(ctx, jwt, referer)
	if perr != nil {
		return MultipleOutcome{Kind: MultipleOutcomeFailed, Error: perr.Error()}, nil
	}
	return MultipleOutcome{
		Kind:       MultipleOutcomeClaimed,
		CloudCount: result.CloudCount,
		Multiple:   result.Multiple,
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 备份好礼（`backupgift/*`）—— 旧项目 `mcloud-sign` 的 `backupGiftTask`
// ─────────────────────────────────────────────────────────────────────────────

// BackupGiftPlan 是「备份好礼」的门槛判定（纯函数，可单测）。
type BackupGiftPlan int

const (
	// BackupGiftPlanClaim 是**可以领**：本月有礼、且还没领过。
	BackupGiftPlanClaim BackupGiftPlan = iota
	// BackupGiftPlanAlreadyReceived 是本月**已领过** —— 不再发请求。
	BackupGiftPlanAlreadyReceived
	// BackupGiftPlanNoBackup 是**未开启自动备份** —— 没有礼可领。
	//
	// 与 [MultiplePlanNoBackup] 同源（旧项目这里也是提示"前往 APP 手动开启"），
	// 同样**不上传文件去开备份** —— 我们没有网盘上传能力。
	BackupGiftPlanNoBackup
	// BackupGiftPlanNothingThisMonth 是本月**没有**备份好礼（`curMonth` 为假）。
	//
	// 它比 [BackupGiftPlanNoBackup] 更靠前：旧项目先判 `if (!curMonth)` 就直接返回，
	// 连 `state` 都不看。所以这一档**不是**"未开备份"，别把两者说成一回事。
	BackupGiftPlanNothingThisMonth
)

// PlanBackupGift 按 `backupgift/info` 的两个字段判 [BackupGiftPlan]。
//
// ⚠ 顺序有意义：`curMonth` 为假时**先**返回 [BackupGiftPlanNothingThisMonth]，
// 因为旧项目就是这么写的（`curMonth` 是前置开关，`state` 是它的细分类）。
func PlanBackupGift(gift upstream.BackupGift) BackupGiftPlan {
	if !gift.CurMonth {
		return BackupGiftPlanNothingThisMonth
	}
	// 旧项目的 `switch (state)`：`0` 可领、`1` 已领、`-1` 未开备份。
	switch gift.State {
	case 0:
		return BackupGiftPlanClaim
	case 1:
		return BackupGiftPlanAlreadyReceived
	}
	return BackupGiftPlanNoBackup
}

// BackupGiftOutcome 是领「备份好礼」的结果。
type BackupGiftOutcome struct {
	// Kind 决定下面哪些字段有意义。
	Kind BackupGiftOutcomeKind
	// Received 是领到的数量。nil = 上游没说。
	Received *int64
	// Error 是失败原因（[BackupGiftOutcomeFailed]）。
	Error string
}

// BackupGiftOutcomeKind 是 [BackupGiftOutcome] 的五种结局。
type BackupGiftOutcomeKind int

const (
	// BackupGiftOutcomeClaimed 是领到了。
	BackupGiftOutcomeClaimed BackupGiftOutcomeKind = iota
	// BackupGiftOutcomeAlreadyReceived 是本月已领过 —— 没发请求。
	BackupGiftOutcomeAlreadyReceived
	// BackupGiftOutcomeNoBackup 是未开启自动备份 —— 没发请求。
	BackupGiftOutcomeNoBackup
	// BackupGiftOutcomeNothingThisMonth 是本月没有备份好礼 —— 没发请求。
	BackupGiftOutcomeNothingThisMonth
	// BackupGiftOutcomeFailed 是门槛成立、请求也发了，但**失败**。
	// 不返回错误，理由同 [MultipleOutcomeFailed]。
	BackupGiftOutcomeFailed
)

// ClaimBackupGift 领「备份好礼」（`backupgift/receive`）—— **写请求**，每月一次。
//
// 先查一次状态判门槛（[PlanBackupGift]），不成立就**一个请求都不发**。
func ClaimBackupGift(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	clock core.Clock,
	rng core.Random,
) (BackupGiftOutcome, *core.Error) {
	referer, jwt, perr := OpenSignInSession(ctx, up, profile, credential, clock, rng)
	if perr != nil {
		return BackupGiftOutcome{}, perr
	}
	gift, perr := up.AppBackupGift(ctx, jwt, referer)
	if perr != nil {
		return BackupGiftOutcome{}, perr
	}

	if plan := PlanBackupGift(gift); plan != BackupGiftPlanClaim {
		switch plan {
		case BackupGiftPlanAlreadyReceived:
			return BackupGiftOutcome{Kind: BackupGiftOutcomeAlreadyReceived}, nil
		case BackupGiftPlanNoBackup:
			return BackupGiftOutcome{Kind: BackupGiftOutcomeNoBackup}, nil
		default:
			return BackupGiftOutcome{Kind: BackupGiftOutcomeNothingThisMonth}, nil
		}
	}

	result, perr := up.AppReceiveBackupGift(ctx, jwt, referer)
	if perr != nil {
		return BackupGiftOutcome{Kind: BackupGiftOutcomeFailed, Error: perr.Error()}, nil
	}
	return BackupGiftOutcome{Kind: BackupGiftOutcomeClaimed, Received: result.Received}, nil
}

// derefBool 把 `*bool` 拆成「值 + 有没有给」两件事。
//
// ⚠ **不要用 `if p != nil && *p` 去替代它**：那会把"上游没给"与"上游给了 false"
// 合成同一个结果，而 [MultiplePlanUnknown] 这一档存在的全部理由就是把它们分开 ——
// 当成 false 会漏领，当成 true 会白发一次写请求把名额用掉。
func derefBool(value *bool) (bool, bool) {
	if value == nil {
		return false, false
	}
	return *value, true
}
