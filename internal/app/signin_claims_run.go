package app

// 月度 / 一次性**领取开关**跑一个账号那一段：签到翻倍、备份好礼、复活卡。
//
// # 为什么三条链共用一个 [ClaimRun]，而不是各写一个 `XxxRun`
//
// 三者的**结论形状**是同一个：门槛成立 ⇒ 发一次写请求；门槛不成立 ⇒ 一个请求都不发；
// 请求发了但失败 ⇒ 这一趟没办成。差别只在"门槛是谁判的、领到的数叫什么"。
// 分成三个类型的话，[ClaimResult.RunReport] 那一套四态映射要抄三遍 ——
// 而抄三遍的代价是"未开备份该落 ManualRequired"这类修正只会落在其中一条上
// （[SignInRun.finish] 的注释说过同一种漂移）。
//
// # 为什么仍留在 app 层而不是塞进各能力函数
//
// [ClaimMultiple] / [ClaimBackupGift] 刻意**不知道**四态：它们同时服务 CLI 与
// 将来可能的账号页按钮，那两处的呈现方式各不相同。只有走编排的这一段需要结论。

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// ClaimKind 是"这一趟领哪一种"。
type ClaimKind int

const (
	// ClaimKindSignInMultiple 是签到翻倍（[ClaimMultiple]）—— 每月一次，把**今日签到奖励**
	// 翻 2–50 倍，前提是本月开着自动备份。
	ClaimKindSignInMultiple ClaimKind = iota
	// ClaimKindBackupGift 是备份好礼（[ClaimBackupGift]）—— 同样每月一次，与翻倍**互相独立**。
	ClaimKindBackupGift
	// ClaimKindRevival 是复活卡 / 回馈卡（[ReceiveRevivalReward]）——
	// **卡用掉就没了**，且 H5 明写"关闭弹窗视为放弃机会，无法补发"。
	ClaimKindRevival
)

// Name 是给人说的那三个字（进小结与过程话，不进 code 那一列）。
func (k ClaimKind) Name() string {
	switch k {
	case ClaimKindSignInMultiple:
		return "签到翻倍"
	case ClaimKindBackupGift:
		return "备份好礼"
	}
	return "复活卡"
}

// CapabilityCode 是这一种对应能力在注册表里的 code（[core.ManualStep.Capability] 要用）。
func (k ClaimKind) CapabilityCode() string {
	switch k {
	case ClaimKindSignInMultiple:
		return capabilities.CodeMultiple
	case ClaimKindBackupGift:
		return capabilities.CodeBackupGift
	}
	return capabilities.CodeRevival
}

// ClaimStatus 是一次领取的结局档位。
type ClaimStatus int

const (
	// ClaimDone 是**领到了**。
	ClaimDone ClaimStatus = iota
	// ClaimNothing 是**本来就没得领**（本月已领过 / 本月没有这份礼 / 判不了）。
	//
	// 它算**成功**：这一趟的目标是"该领的时候领掉"，来一趟确认"现在不该领"就是达成了
	// —— 与签到里"今天已经签过了"、直播里"今天没开播"同一档。
	ClaimNothing
	// ClaimManual 是门槛不成立且**得有人去客户端点一下**才会成立（未开自动备份）。
	//
	// ⚠ 与 [ClaimNothing] 分开是因为处置不同：一条等下个月就行，另一条等到天荒地老
	// 也不会自己好。旧项目把这一档压成"成功 + 一句 info"，于是没人知道要去开备份。
	ClaimManual
	// ClaimFailed 是请求发了但失败了 —— 这一趟没办成。
	//
	// ⚠ 落 [core.OutcomeFailed] 而不是 PartialSuccess：这一条链上**只有**这一次动作，
	// 它没成就等于整趟没成（签到那条链上 PartialSuccess 是"签上了但没领完"，两回事）。
	ClaimFailed
)

// ClaimResult 是跑完一个账号的实况。
type ClaimResult struct {
	Kind    ClaimKind
	Account core.AccountID
	Status  ClaimStatus
	// Gained 是领到的数量。nil = **上游没说**，不是 0
	// （复活卡上游一定会报数，翻倍/好礼的 `cloudCount` 是可选字段）。
	Gained *int64
	// Detail 是跟在数量后面那句（"2 倍"、卡型），可为空。
	Detail string
	// Reason 是 [ClaimNothing] / [ClaimManual] 的人话依据。
	Reason string
	// Error 是 [ClaimFailed] 的失败原因。
	Error string
}

// ClaimRun 跑一条领取开关。
type ClaimRun struct {
	SignInWorld
	// Kind 决定发哪个请求、判哪个门槛。**必填**（零值是翻倍那一档，填错不报错）。
	Kind ClaimKind
}

// RunAccount 跑**一个账号**的这一笔。
//
// 取不到票 / 查门槛失败 ⇒ 返回错误（这一趟没走到能报结论的那一步）；
// 走到了门槛判定之后的一切结果都落成 [ClaimResult] 的四档，**不**回错误 ——
// 否则"这个月已领过"会变成历史里的一条红色。
func (r ClaimRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (ClaimResult, *core.Error) {
	switch r.Kind {
	case ClaimKindSignInMultiple:
		return r.multiple(ctx, account)
	case ClaimKindBackupGift:
		return r.backupGift(ctx, account)
	}
	return r.revival(ctx, account)
}

func (r ClaimRun) multiple(
	ctx context.Context, account core.AccountID,
) (ClaimResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return ClaimResult{Kind: r.Kind, Account: account}, perr
	}
	outcome, perr := ClaimMultiple(ctx, src)
	if perr != nil {
		// 查门槛那一步就挂了（`infoV3` 打不通）：整趟没有结论可报，交回错误。
		return r.failed(account, perr.Error()), perr
	}
	switch outcome.Kind {
	case MultipleOutcomeClaimed:
		detail := ""
		if outcome.Multiple != nil {
			detail = fmt.Sprintf("%d 倍", *outcome.Multiple)
		}
		return ClaimResult{Kind: r.Kind, Account: account,
			Status: ClaimDone, Gained: outcome.CloudCount, Detail: detail}, nil
	case MultipleOutcomeAlreadyAccepted:
		return r.nothing(account, "本月已经领过一次翻倍"), nil
	case MultipleOutcomeNoBackup:
		// 旧项目在这里会**上传一个文件把备份打开**再重试。我们没有网盘上传能力
		// （见 [MultiplePlanNoBackup]），所以只如实报成"要人去开"，**不假装做过**。
		return r.manual(account, "本月没开自动备份，翻倍机会没有产生"), nil
	case MultipleOutcomeFailed:
		return r.failed(account, outcome.Error), nil
	}
	// 上游没给那两个门槛字段 ⇒ **一个请求都不发**（当成"开了"会白发一次写请求
	// 把这个月的名额用掉，当成"没开"会漏领 —— 见 [MultiplePlanUnknown]）。
	return r.nothing(account, "上游没给备份门槛字段，判不了，本次没发请求"), nil
}

func (r ClaimRun) backupGift(
	ctx context.Context, account core.AccountID,
) (ClaimResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return ClaimResult{Kind: r.Kind, Account: account}, perr
	}
	outcome, perr := ClaimBackupGift(ctx, src)
	if perr != nil {
		return r.failed(account, perr.Error()), perr
	}
	switch outcome.Kind {
	case BackupGiftOutcomeClaimed:
		return ClaimResult{Kind: r.Kind, Account: account,
			Status: ClaimDone, Gained: outcome.Received}, nil
	case BackupGiftOutcomeAlreadyReceived:
		return r.nothing(account, "本月已经领过备份好礼"), nil
	case BackupGiftOutcomeNoBackup:
		return r.manual(account, "未开启自动备份，本月这份礼没产生"), nil
	case BackupGiftOutcomeFailed:
		return r.failed(account, outcome.Error), nil
	}
	return r.nothing(account, "本月没有备份好礼"), nil
}

// revival 用一张复活卡 / 回馈卡。
//
// # 它没有门槛可查，所以**一定**发写请求
//
// 上游没有"还剩几张卡"的查询口（旧项目与 caiyun 都是在本地记当月额度，见
// `docs/caiyun-go-parameters-and-decisions.md` 的 `revivalreward` 那一行）。
// 本地额度表**还没建**（要动 schema 与 `StateStore`，已登记成待用户拍板的问题），
// 所以这一条的唯一防线是注册表上的 `RunInBatch: false` + `RiskHigh` 弹框 ——
// 表现上的风险是"手工点两次 = 用掉两张卡"，这一点必须写在界面上而不是藏在代码里。
//
// # 为什么不换 H5 `deviceId`
//
// 两个证据不一致（H5 定义里没标 `isDeviceId`，老抓包里却带了一个 `deviceid` **头**，
// 见 [ReceiveRevivalReward]），而换它要**真上报一次设备指纹**。
// 为一个未验证的可选头去多打一次上报，不如先按"不带"跑，真出问题时再 A/B。
func (r ClaimRun) revival(
	ctx context.Context, account core.AccountID,
) (ClaimResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return ClaimResult{Kind: r.Kind, Account: account}, perr
	}
	reward, perr := ReceiveRevivalReward(ctx, src, nil)
	if perr != nil {
		// 交回错误而不是折成 [ClaimFailed]：这一条**没有门槛可查**，
		// "没有可用卡"与"网络 / 凭据坏了"只能从同一个返回里分出来，
		// 而两者都该让这一趟报失败 —— 走错误那条路还会多发一条 `errored` 事件。
		return ClaimResult{Kind: r.Kind, Account: account}, perr
	}
	card := "回馈卡"
	if reward.CardType == 1 {
		card = "复活卡"
	}
	gained := reward.RewardClouds
	r.progress(core.LevelInfo, "用掉一张"+card)
	return ClaimResult{Kind: r.Kind, Account: account,
		Status: ClaimDone, Gained: &gained, Detail: card}, nil
}

func (r ClaimRun) nothing(account core.AccountID, reason string) ClaimResult {
	return ClaimResult{Kind: r.Kind, Account: account, Status: ClaimNothing, Reason: reason}
}

func (r ClaimRun) manual(account core.AccountID, reason string) ClaimResult {
	return ClaimResult{Kind: r.Kind, Account: account, Status: ClaimManual, Reason: reason}
}

func (r ClaimRun) failed(account core.AccountID, reason string) ClaimResult {
	return ClaimResult{Kind: r.Kind, Account: account, Status: ClaimFailed, Error: reason}
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 里面只有数量与档位，没有任何凭据形态的东西。
func (res ClaimResult) SummaryLine() string {
	head := res.Kind.Name()
	switch res.Status {
	case ClaimDone:
		if res.Gained == nil {
			return head + "已领到（上游没报具体数量）"
		}
		text := fmt.Sprintf("%s领到 %d", head, *res.Gained)
		if res.Detail != "" {
			text += "（" + res.Detail + "）"
		}
		return text
	case ClaimNothing:
		return head + "没领：" + res.Reason
	case ClaimManual:
		return head + "没领：" + res.Reason + "，要去 App 开自动备份"
	}
	if res.Error == "" {
		return head + "失败"
	}
	return head + "失败：" + res.Error
}

// RunReport 把四档结论映射到 [core.RunOutcome]。
//
// 档位本身已经互斥，映射因此很薄；这里唯一有信息量的是 [ClaimManual] 那一档 ——
// 它是**唯一**能把"得有人去 App 里点一下"传到推送与历史列表的通路
// （`core/run.go` 文件头就说过旧项目把这一态压成 info 日志的后果）。
func (res ClaimResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	switch res.Status {
	case ClaimManual:
		account := res.Account
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			res.Kind.CapabilityCode(), &account,
			"去移动云盘 App 打开「自动备份」，这个月的翻倍机会与备份好礼才会产生；"+
				"本工具没有网盘上传能力，代劳不了这一步",
			core.TargetMobileApp,
		)}, summary)
	case ClaimFailed:
		return core.NewRunReportFailed(summary)
	default:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
}
