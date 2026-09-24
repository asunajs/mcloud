package app

// 「公告任务」（旧项目叫 `notice`，界面上那句是「开启通知领云朵」）：
// 查两个槽的状态 → 该领就领 → **复查一次**拿读数。
//
// # 这一条为什么要做成独立能力，而不是挂在签到后面顺手跑
//
// 它是**另一条链**：主机带端口（`:7071`）、路径前缀是 `openemailsms-service`
// 而不是 `signin`，判据字段一个都不在 `infoV3` 里。挂在签到后面跑，
// 签到那一条链就多两个出网请求，而这两条的失败互不相干。
//
// # ⚠ 本文件的判定**建在两处冲突之上**，口径写在 `upstream/app_notice.go` 头注
//
// 一句话版本：旧 TS 说链路是 `getTaskInfo → reward → taskListV2 → reportAppNoticeStatus`，
// 而实拍（`开启通知领云朵.json` / `完成通知领云朵.json`）里看到的是
// `getTaskInfo ×2 → reward` 与另一场里的 `receiveTask?taskId=1021`，**没有** `taskListV2`。
// 这里的处置：**按实拍最强的那条裁**（判据只用 `getTaskInfo` 的四个字段），
// 另一条入账口（`receiveTask`）与上报口（`reportAppNoticeStatus`）
// 已在 upstream 层实现并测过，**接哪一个由编排层拍板**，本文件不替它选。
//
// # jwttoken 的来源：这一页用**公共来源**
//
// 实拍的四条请求都带 `jwttoken`，但**换到那张票的链路没被抓到**
// （抓包里 `referer`、`jwttoken` 全被脱敏，也没有前置的 `tyrzLogin`）。
// ⇒ 按 `AGENTS.md` §1.12：无法确定该页特有取法时复用既有建会话入口，
// 即 [SignInWorld.Source] 那一条（签到页配方 `RecipeSignInPage`），并在此声明
// "**jwttoken 使用公共来源**"。这是**临时态**：哪天抓到了「开启通知」页自己的
// 那三枪，就改用特有取法并把这段改掉，同时把新链路补进 `docs/capture/`。
// ⚠ 连带未验证：签到页那张票的 `referer` 指向 `m.mcloud.139.com`，
// 而这里的请求打在 `caiyun.feixin.10086.cn:7071` 上 —— 跨主机带 referer
// 上游认不认，**没有样本**（实拍那条的 referer 被脱敏，看不出它当时带的是哪个）。

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// noticeCapabilityCode 是这条能力在注册表里的 code。
//
// 写在子代理那一批文件里时注册表还没有这一条（接线归编排层），当时是**字面量**；
// 现在指回 [capabilities.CodeNotice] —— 一处定义，`runs.capability` 那一列与
// [core.ManualStep.Capability] 不会再各自漂移。
const noticeCapabilityCode = capabilities.CodeNotice

// ─────────────────────────────────────────────────────────────────────────────
// 判定：两个槽的读数 →「该不该领」
// ─────────────────────────────────────────────────────────────────────────────

// NoticePlan 是 [PlanNotice] 的结论档位。
type NoticePlan int

const (
	// NoticePlanReward 是**该领**：至少一个槽的 `cloudStatus` 读成"未领"、
	// 而它的 `taskStatus` 读成"已完成"。⇒ 发 `reward`。
	//
	// ⚠ 这条映射是**待验证**的，集中在一处的实现见 [noticeSlotClaimable]。
	NoticePlanReward NoticePlan = iota
	// NoticePlanNothing 是**没得领**：能判的槽全都读成"已领"，或者上游一槽都没给。
	//
	// 它算**成功**：这一趟的目标是"该领的时候领掉"，来一趟确认"现在不该领"就是达成了
	// （与 [ClaimNothing] 同一档口径）。
	NoticePlanNothing
	// NoticePlanManual 是**得有人去 App 里点一下**（开邮箱 / 短信通知）才会有的那一档。
	//
	// ⚠⚠ 这一档**今天到不了**：判据读不出来，见 [noticeNeedsManual] 那段。
	// 保留它是因为结果层（[NoticeManual] → [core.OutcomeManualRequired]）必须一直通着，
	// 抓到"未开启"那条例文之后**只改一个函数**。
	NoticePlanManual
	// NoticePlanUnknown 是**判不了**：没有任何一个槽把两个判据字段都给全。
	//
	// 处置是**一个请求都不发** —— 与 [MultiplePlanUnknown] 同一条理由：
	// 当成"该领"会白发一次写请求（而 `reward` 幂等与否**零样本**），
	// 当成"不该领"会漏领。两边代价不对称，但都不如"什么都不发、把判不了说清楚"。
	NoticePlanUnknown
)

// 待验证的映射**只写在这两个常量与这一个函数里**。
//
// ⚠⚠ 这是**猜的对应关系**，本仓库关于它的实拍只有两条读数，而且两条互相分不清方向：
//
//	taskOneInfo{cloudStatus: 2, sum: 200,             taskStatus: 3}
//	taskTwoInfo{cloudStatus: 1, sum: 100, progress: 31, taskStatus: 3}
//
// 两条的 `taskStatus` **相同**而 `cloudStatus` **不同** ⇒ `taskStatus` 不是"这个槽
// 领没领"的读数，`cloudStatus` 才是。但**哪个值是"未领"**实拍定不出来。
// 唯一能加分的一条旁证：`taskTwoInfo.sum == 100`，而另一场抓包里
// `receiveTask?taskId=1021` 回的正是 `result: 100` ⇒ "槽二当时还没入账"与
// "`cloudStatus == 1` 是未领"自洽。旁证不是判据（两件事跨了两份抓包、两次会话）。
//
// ⚠ **反向映射也照样让实拍那一条走 Reward**：两个值里总有一个被读成"未领"，
// 而实拍里真实客户端**紧接着就发了 `reward`** —— 所以"该不该发"这一半是稳的，
// 不稳的是"两个槽都已领"那一半：方向猜反的表现是**该领时说不领**（漏领），
// 而不是"多发一次写请求"。
// 怎么定：拿一个从没开过通知的号看两槽读数 → 开启 → 再读一次；
// 或开一次、`reward` 一次、再读一次。**两个方向只能这样分开**。
const (
	// noticeCloudStatusUnclaimed 读作"这一槽的云朵**还没领**"。
	noticeCloudStatusUnclaimed int64 = 1
	// noticeCloudStatusClaimed 读作"这一槽**已入账**"。
	noticeCloudStatusClaimed int64 = 2
	// noticeTaskStatusDone 读作"任务本身**已完成**"（实拍两槽都是 3）。
	noticeTaskStatusDone int64 = 3
)

// noticeSlotClaimable 报告"这一槽该领"。
//
// ⚠ 两个判据字段**缺任何一个**都回 false（= 这一槽不参与"该领"的判断），
// 而不是参与判断后落成"不该领"：`nil` 与 `0` 不是一回事，
// 把"上游没说"当成"说了没完成"会让这条链静默漏领。
func noticeSlotClaimable(slot *upstream.NoticeTaskSlot) bool {
	if slot == nil {
		return false
	}
	cloud, hasCloud := noticeInt(slot.CloudStatus)
	status, hasStatus := noticeInt(slot.TaskStatus)
	if !hasCloud || !hasStatus {
		return false
	}
	return cloud == noticeCloudStatusUnclaimed && status == noticeTaskStatusDone
}

// noticeSlotJudged 报告"这一槽**能不能判**"（两个判据字段都给全了）。
func noticeSlotJudged(slot *upstream.NoticeTaskSlot) bool {
	if slot == nil {
		return false
	}
	_, hasCloud := noticeInt(slot.CloudStatus)
	_, hasStatus := noticeInt(slot.TaskStatus)
	return hasCloud && hasStatus
}

// noticeNeedsManual 是"得有人去 App 里开通知"的**唯一**入口。
//
// ⚠⚠ **待验证，且今天恒回 false** —— 这一档因此不可达，判据本身**未被断言**
// （`AGENTS.md` §1.3：造不出反例的分支要就地声明，不许因"没红过"当作不重要）。
// 为什么不硬凑一个条件：
//   - 实拍的两个字段读不出"通知开没开"：`cloudStatus` 只有 `1` / `2` 两个读数，
//     而它们与"领没领"已经绑在一起用了（见 [noticeSlotClaimable]），
//     同一个字段同时当"开没开"与"领没领"用是自相矛盾的；
//   - 旧 TS 的判据说是 `day` / `canReceive` 两个字段，**一条实拍都没有**，
//     连它们在哪个响应里都不确定；
//   - "未开启"态的响应**零样本** —— 上游很可能是回一个**非 0 码**，
//     而那种形态今天会走 [NoticeOutcomeFailed] 之前的错误那条路
//     （`EnsureOKWithHint` 直接报错），压根到不了判定层。要核它得抓一条
//     "从没开过通知的账号打 getTaskInfo"的原文。
//
// 拿到那条例文之后：**只改这一个函数**，结果层不用动。
func noticeNeedsManual(_ upstream.NoticeTaskInfo) bool { return false }

// PlanNotice 按 `getTaskInfo` 的两个槽判 [NoticePlan]。**纯函数**，不碰网络。
//
// ⚠ 顺序有意义：Manual 排在 Reward 前面（真开着通知但已领过，也不该再发一次写请求），
// 而 Unknown 是兜底 —— "什么都没给"必须落 Unknown，不能落 Nothing
// （落 Nothing 会让小结说成"现在没得领"，那是一句**没根据的肯定话**）。
func PlanNotice(info upstream.NoticeTaskInfo) NoticePlan {
	if noticeNeedsManual(info) {
		return NoticePlanManual
	}
	slots := []*upstream.NoticeTaskSlot{info.TaskOneInfo, info.TaskTwoInfo}
	judged := 0
	for _, slot := range slots {
		if noticeSlotClaimable(slot) {
			return NoticePlanReward
		}
		if noticeSlotJudged(slot) {
			judged++
		}
	}
	if judged > 0 {
		return NoticePlanNothing
	}
	return NoticePlanUnknown
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// NoticeOutcomeKind 是 [NoticeOutcome] 的结局档位。
type NoticeOutcomeKind int

const (
	// NoticeOutcomeNone 是"**没走到能报结论的那一步**"（查判据就挂了）。
	//
	// ⚠ 它刻意占**零值**：[ClaimNotice] 有两条提前 return 错误的路径，那些路径上
	// 结构体是 `NoticeOutcome{}`。零值若是"已领"，一句"`reward` 打成、领到了"
	// 就会在**一个请求都没发**的那趟上被念出来 —— 而错误那条路已经把这一趟记成失败，
	// 两句对不上就是将来排查时的头号噪声来源。
	NoticeOutcomeNone NoticeOutcomeKind = iota
	// NoticeOutcomeClaimed 是 `reward` 发出去且上游认了。
	// 到账读数在 [NoticeOutcome.Recheck] 里 —— ⚠ **不在** `Gained` 里，
	// 因为实拍那条 `reward` 响应**连 `result` 都没有**。
	NoticeOutcomeClaimed
	// NoticeOutcomeNothing 是没得领 —— 没发写请求。
	NoticeOutcomeNothing
	// NoticeOutcomeManual 是得有人去 App 里开通知 —— 没发写请求。⚠ 今天不可达（[noticeNeedsManual]）。
	NoticeOutcomeManual
	// NoticeOutcomeUnknown 是判不了 —— **一个请求都不发**。
	NoticeOutcomeUnknown
	// NoticeOutcomeFailed 是 `reward` 发了但失败。
	//
	// 不返回错误：理由同 [MultipleOutcomeFailed] —— 这一趟的实况是"发过、没成"，
	// 报成错误会把"发过"这件事抹掉。
	NoticeOutcomeFailed
)

// NoticeOutcome 是 [ClaimNotice] 的产出。
type NoticeOutcome struct {
	// Kind 决定下面哪些字段有意义。
	Kind NoticeOutcomeKind
	// Before 是**发写请求之前**那份 `getTaskInfo`（判据用它算的）。
	Before upstream.NoticeTaskInfo
	// Recheck 是**发完 `reward` 之后**再查一次的那份；`nil` = 没复查（没发）或复查挂了。
	//
	// ⚠ 它是这一条唯一的"到账读数"：`reward` 报不出数（见 [NoticeOutcome.Gained]）。
	Recheck *upstream.NoticeTaskInfo
	// Gained 是 `reward` 报出的云朵数。实拍**恒无**（响应里没有 `result`）⇒ nil。
	// 留这个字段是"哪天上游给了别丢"，不是"这里应该有数"。
	Gained *int64
	// Error 是失败原因（[NoticeOutcomeFailed]）。
	Error string
}

// ClaimNotice 跑这一族的链：`getTaskInfo` → 判 → （该领才）`reward` → **复查** `getTaskInfo`。
//
// # 为什么领完还要再查一次
//
// `reward` 的实拍响应是 `{"code":0,"msg":"success"}` —— **一个数都没有**。
// 只看它，"到底领没领上"就只剩"上游说了句 success"。复查那一份里
// `cloudStatus` 会不会从 1 变 2、什么时候变，**未验证**；但它是眼下**唯一**
// 能对拍的读数，也是复查存在的全部理由。
//
// # 三种失败的走法不同（这一处最容易写错）
//
//   - **查判据失败** ⇒ 交回错误，且结局是 [NoticeOutcomeNone]（零值）：整趟没有结论可报。
//     ⚠ 零值刻意**不是**"已领"，理由见那个常量的注释。
//   - **`reward` 失败** ⇒ 折成 [NoticeOutcomeFailed]，**不**回错误：请求发过，这是实况。
//   - **复查失败** ⇒ **交回错误**，但结论仍是 [NoticeOutcomeClaimed] ——
//     "领成功了但复查挂了"不许写成失败（那会让人以为没领上、再去点一次，
//     而 `reward` 幂等与否**零样本**）。编排层按错误记这一趟，
//     界面因此看到的是"这一趟没走完"而不是"没领到"，两句说的是不同的事。
func ClaimNotice(
	ctx context.Context, src *SignInSessionSource,
) (NoticeOutcome, *core.Error) {
	before, perr := NoticeTaskInfo(ctx, src)
	if perr != nil {
		return NoticeOutcome{}, perr
	}

	switch plan := PlanNotice(before); plan {
	case NoticePlanManual:
		return NoticeOutcome{Kind: NoticeOutcomeManual, Before: before}, nil
	case NoticePlanNothing:
		return NoticeOutcome{Kind: NoticeOutcomeNothing, Before: before}, nil
	case NoticePlanUnknown:
		return NoticeOutcome{Kind: NoticeOutcomeUnknown, Before: before}, nil
	case NoticePlanReward:
	}

	reward, perr := do(ctx, src, func(s SignInSession) (upstream.NoticeReward, *core.Error) {
		return src.up.AppNoticeReward(ctx, s.jwt, s.referer)
	})
	if perr != nil {
		return NoticeOutcome{Kind: NoticeOutcomeFailed, Before: before, Error: perr.Error()}, nil
	}

	after, perr := NoticeTaskInfo(ctx, src)
	if perr != nil {
		// ⚠ 交回错误**同时**把结论写成"已领"（见函数头第三条）。
		return NoticeOutcome{
			Kind: NoticeOutcomeClaimed, Before: before, Gained: reward.CloudCount,
		}, perr
	}
	return NoticeOutcome{
		Kind:    NoticeOutcomeClaimed,
		Before:  before,
		Recheck: &after,
		Gained:  reward.CloudCount,
	}, nil
}

// NoticeTaskInfo 只查公告任务的两个槽，不领（给"看一眼状态"那种用法留着）。
func NoticeTaskInfo(
	ctx context.Context, src *SignInSessionSource,
) (upstream.NoticeTaskInfo, *core.Error) {
	return do(ctx, src, func(s SignInSession) (upstream.NoticeTaskInfo, *core.Error) {
		return src.up.AppNoticeTaskInfo(ctx, s.jwt, s.referer)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// NoticeStatus 是一次公告任务的结局档位（[NoticeOutcomeKind] 的编排层投影）。
type NoticeStatus int

const (
	// NoticeDone 是领到了。
	NoticeDone NoticeStatus = iota
	// NoticeNothing 是没得领 —— 算成功。
	NoticeNothing
	// NoticeManual 是得有人去 App 里开通知。
	NoticeManual
	// NoticeUnknown 是判不了（没发请求），与 [NoticeNothing] 分开是因为**说法**不同：
	// 一句是有根据的"现在不该领"，另一句是"我们没根据"。
	NoticeUnknown
	// NoticeFailed 是 `reward` 发了但失败。
	NoticeFailed
)

// NoticeRun 跑一次公告任务。
type NoticeRun struct {
	SignInWorld
}

// NoticeResult 是一个账号这一趟的产出。
//
// # 为什么这里只放**读数**、不放"领到多少朵"
//
// 上游那一口 `reward` 什么都不报（实拍没有 `result`），而两个槽的
// `cloudStatus` / `sum` 语义**未验证**（见 `noticeSlotClaimable` 那一大段）。
// 于是"进账 N 朵"这句话眼下**没有来源** —— 与其编一个（比如拿 `sum` 当 N），
// 不如把两份读数原样交出去，让人自己看得见。
type NoticeResult struct {
	Account core.AccountID
	Status  NoticeStatus
	// Gained 是 `reward` 报出的数，实拍**恒 nil**（≠ 0，也≠"领到 0 朵"）。
	Gained *int64
	// TaskOne / TaskTwo 是**结论对应那一次**的槽读数：
	// 走过 `reward` 的那一趟（[NoticeDone]）是**复查之后**的；没发写请求的那几档是判据那一次的。
	// nil = 上游没给这一槽，或**复查没跑成**（见 [NoticeResult.RecheckFailed]）。
	TaskOne *upstream.NoticeTaskSlot
	TaskTwo *upstream.NoticeTaskSlot
	// RecheckFailed 标的是"`reward` 发成了、但**复查挂了**"这一种叠加态：
	// 结论是"已领"（实况），错误同时交回编排层（这一趟没走完）。
	// ⚠ 不留这个位，两种"没数"就分不开：一种是真的没读到、一种本来就不该有数。
	RecheckFailed bool
	// Reason 是 [NoticeNothing] / [NoticeManual] / [NoticeUnknown] 的人话依据。
	Reason string
	// Error 是 [NoticeFailed] 的失败原因，或 [RecheckFailed] 那句复查失败的原话。
	Error string
}

// RunAccount 跑**一个账号**的这一笔。
//
// 取不到票 / 查判据失败 / **复查失败** ⇒ 交回错误（见 [ClaimNotice] 头注第三条）；
// 判定之后的一切结果都落成 [NoticeResult] 的档位，不报错。
func (r NoticeRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (NoticeResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return NoticeResult{Account: account}, perr
	}
	outcome, perr := ClaimNotice(ctx, src)
	if perr != nil {
		// 结论**照 outcome 的档**写（复查挂了那一趟确实领成了），错误同时交回。
		result := r.project(account, outcome)
		if result.Status == NoticeDone {
			result.RecheckFailed = true
		}
		result.Error = perr.Error()
		return result, perr
	}
	result := r.project(account, outcome)
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// project 把能力层的结局投影成跑一趟的结论。
//
// ⚠ 单独成一个函数，是因为 `RunAccount` 有**两条**出口（错误那条也要先投影一次），
// 内联的话两条出口各写一遍字段，改一档只会改到一半。
func (r NoticeRun) project(
	account core.AccountID, outcome NoticeOutcome,
) NoticeResult {
	result := NoticeResult{Account: account, Gained: outcome.Gained}
	// 没发写请求的那几档（含 [NoticeOutcomeNone]）：读数就是判据那一次的。
	claimSlots := func() {
		result.TaskOne, result.TaskTwo = outcome.Before.TaskOneInfo, outcome.Before.TaskTwoInfo
	}
	switch outcome.Kind {
	case NoticeOutcomeClaimed:
		// ⚠ 只认 [NoticeOutcome.Recheck]：**不许**回落到 `Before`。
		// 拿"领之前"那份读数去填"复查之后"那两个字段，表现是小结念出
		// "复查：cloudStatus=1"，而它其实是领之前的数 —— 那正是复查要看的东西，
		// 念错了比念不出来更糟（会把"到底变没变"这个唯一的问题答成假的）。
		result.Status = NoticeDone
		if seen := outcome.Recheck; seen != nil {
			result.TaskOne, result.TaskTwo = seen.TaskOneInfo, seen.TaskTwoInfo
		}
	case NoticeOutcomeNothing:
		result.Status = NoticeNothing
		result.Reason = "能判的槽都读成已入账"
		claimSlots()
	case NoticeOutcomeManual:
		result.Status = NoticeManual
		result.Reason = "有槽读成通知未开启"
		claimSlots()
	case NoticeOutcomeUnknown:
		result.Status = NoticeUnknown
		result.Reason = "上游没给判据字段（cloudStatus / taskStatus），判不了，本次没发请求"
		claimSlots()
	case NoticeOutcomeFailed:
		result.Status = NoticeFailed
		result.Error = outcome.Error
		claimSlots()
	case NoticeOutcomeNone:
		// 查判据就挂了：一个写请求都没发，"失败"在这里说的是"这一趟没办成"。
		// `Error` 由 `RunAccount` 用交回的那条错误原文填（这里编不出一条准确的）。
		result.Status = NoticeFailed
	}
	return result
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
//
// ⚠ 里面只有档位与上游给的两个读数，没有任何凭据形态的东西。
// 念读数而不解释它 —— "槽一 cloudStatus=2"是**照抄**，
// 翻成"槽一已领"就等于把上面那份未验证的映射写成结论。
func (res NoticeResult) SummaryLine() string {
	head := "公告任务"
	switch res.Status {
	case NoticeDone:
		text := head + "已领"
		if res.Gained != nil {
			text = fmt.Sprintf("%s领到 %d", head, *res.Gained)
		}
		if res.RecheckFailed {
			// ⚠ 这里**不**念 `TaskOne` / `TaskTwo`：这一档它们刻意是 nil
			// （见 [NoticeRun.project]），念出来只会得到两个"无槽"，
			// 而"无槽"在上游真没给那一槽时也是一句真话 —— 两种情况就混了。
			return text + "（复查没跑成，拿不到到账读数）"
		}
		return text + "（复查：" + noticeSlotLabel(res.TaskOne) +
			"，" + noticeSlotLabel(res.TaskTwo) + "）"
	case NoticeNothing:
		return head + "没领：" + res.Reason
	case NoticeManual:
		return head + "没领：" + res.Reason + "，要去 App 里开启邮箱/短信通知"
	case NoticeUnknown:
		return head + "没领：" + res.Reason
	}
	if res.Error == "" {
		return head + "失败"
	}
	return head + "失败：" + res.Error
}

// noticeSlotLabel 把一个槽念成一句短语。**缺的键念成"缺"**，不念成 0 ——
// "上游没给 progress"与"上游给了 0"是两件事，混起来会让人觉得任务进度是零。
func noticeSlotLabel(slot *upstream.NoticeTaskSlot) string {
	if slot == nil {
		return "无槽"
	}
	parts := make([]string, 0, 4)
	for _, field := range []struct {
		key   string
		value *int64
	}{
		{"cloudStatus", slot.CloudStatus},
		{"taskStatus", slot.TaskStatus},
		{"sum", slot.Sum},
		{"progress", slot.Progress},
	} {
		if value, ok := noticeInt(field.value); ok {
			parts = append(parts, fmt.Sprintf("%s=%d", field.key, value))
			continue
		}
		parts = append(parts, field.key+"=缺")
	}
	return strings.Join(parts, " ")
}

// RunReport 把五档结论映射到 [core.RunReport]。
//
// 唯一有信息量的是 [NoticeManual]：它是**唯一**能把"得有人去 App 里点一下"
// 传到推送与历史列表的通路（旧项目把它压成一句 info 的后果见 `docs/business-feature-inventory.md`
// §2.3 与 `core/run.go` 文件头）。⚠ 这一档今天不可达（[noticeNeedsManual] 恒 false），
// 但映射本身有测试钉着 —— 判据补上时不该还得先测一遍这条路。
func (res NoticeResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	switch res.Status {
	case NoticeManual:
		account := res.Account
		return core.NewRunReportManualRequired([]core.ManualStep{core.NewManualStep(
			noticeCapabilityCode, &account,
			"去移动云盘 App 里开启「邮箱通知 / 短信通知」，这两条没开时公告任务的云朵不会入账；"+
				"本工具代劳不了这一步",
			core.TargetMobileApp,
		)}, summary)
	case NoticeFailed:
		return core.NewRunReportFailed(summary)
	default:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	}
}

// noticeInt 把 `*int64` 拆成「值 + 有没有给」（与 `derefBool` 同一判据：
// 合成一件事就会把"上游没说"当成"说了某个值"）。
func noticeInt(value *int64) (int64, bool) {
	if value == nil {
		return 0, false
	}
	return *value, true
}
