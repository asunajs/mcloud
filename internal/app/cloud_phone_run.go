package app

// 红包派对（#18）· **只读那一半**的编排：看余额、看任务清单、看播报位。
//
// # 为什么这一轮只做只读（把"做任务"留在后面）
//
// 这一族的产出是**现金（分）**，而它挣钱的三条路各自的代价都不一样：
//
//   - **浏览类**（`userCompleteTask` → `userBrowse`）：会发写请求，但只是"看 15 秒页面"这一类；
//   - **答题类**（`configTopicList` → `userTopicAnswer`）：答案来自旧实现**内置的 15 条 FAQ 题库**，
//     改版即静默失效（成文 §2.4 那条点了这件事），要迁得连"库里没有这条题怎么办"一起定；
//   - **安装类**（`thirdQueryUserRelationshipV2` → … → `apkInstall`）：**真的往用户云机装 APK**，
//     且旧实现"先记账后动作" ⇒ 已记完成但装失败时任务不再重跑。
//     这一条属**待拍板 P3**（`docs/pending-decisions.md`），本轮**不接**。
//
// 于是这里只交"看得见的一层"：余额与清单。它单独成立（用户能核对数字对不对），
// 而且它顺带把**会话与鉴权头那一条最容易错的路**（每次重算的 `sign`）跑在真实链路上 ——
// 将来接写请求时不用第一次就在鉴权上失败。
//
// # ⚠ 一句话都不解释"这些任务为什么是这些状态"
//
// 上游那一族的档位语义只有 `0` / `1` 有实拍，`2` 那一档成文明确写着"未定义但会进执行分支"
// （`upstream.CloudPhoneUserStatus` 那里有完整一段）。这里只念读数。

import (
	"context"
	"fmt"
	"strconv"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// cloudPhoneCapabilityCode 是这一条能力在注册表里的 code。
const cloudPhoneCapabilityCode = capabilities.CodeCloudPhoneRedpack

// CloudPhoneWorld 是"以红包派对活动站身份跑一件事"需要的世界。
//
// 与 [SignInWorld] 同构，但**没有** `Sessions` 复用那一层的配方概念：
// 这一族的票不是 `jwttoken`（见 `cloud_phone_session.go` 文件头）。
type CloudPhoneWorld struct {
	Store    core.StateStore
	Scope    core.Scope
	Upstream *upstream.Upstream
	Device   upstream.AppDevice
	Clock    core.Clock
	Random   core.Random

	// Sessions 是活动站票池；nil ⇒ 回落成"这一次那一张"（CLI 单次调用与测试夹具）。
	// ⚠ 服务端那条路**必须**接：漏接的表现与签到那一条一样 —— 每次点击多打两枪换票。
	Sessions *CloudPhoneSessionPool

	// Progress 收"给人看的过程话"，可为 nil。
	Progress RunProgress
}

// Source 备档案 → 取凭据 → 拿票源，**不发请求**。
//
// `deviceId` 的取法在这里**包成闭包**交出去（换票那一枪才用得上它，
// 而取它可能要上报设备指纹 —— 一次"只看余额"不该顺手做那件事）。
func (w CloudPhoneWorld) Source(
	ctx context.Context, account core.AccountID,
) (*CloudPhoneSessionSource, *core.Error) {
	profile, perr := ProfileWithStoredDomainID(ctx, w.Store, w.Scope,
		upstream.NewAppProfile(w.Device), account)
	if perr != nil {
		return nil, perr
	}
	credential, perr := CredentialFromAccount(ctx, w.Store, w.Scope, account)
	if perr != nil {
		return nil, perr
	}
	deviceID := func(ctx context.Context) (string, *core.Error) {
		return DeviceID(ctx, w.Upstream, w.Store, w.Scope, account, w.Clock, w.Random)
	}
	return cloudPhoneSessionSource(w.Sessions, w.Upstream, profile, &credential,
		w.Clock, w.Random, deviceID), nil
}

func (w CloudPhoneWorld) progress(level core.LogLevel, message string) {
	if w.Progress != nil {
		w.Progress(level, message)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 产出
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneAssets 是一次只读盘点的三份读数。
//
// ⚠ 三个字段**都可能是零值而不是"没有"**：`Toast` 是指针，`Balance`/`Tasks` 的每个键
// 又各自是指针。念它的一律走 [CloudPhoneAssets.SummaryLine]，别在别处 `!= nil` 一遍。
type CloudPhoneAssets struct {
	// Balance 是 `userAccountInfo`（**产出是钱**那一格，单位分）。
	Balance upstream.CloudPhoneAccountInfo
	// Tasks 是 `configTaskLoginList`（成文 §2.4 第 373 行说主流程用的就是这一条）。
	Tasks upstream.CloudPhoneTaskList
	// Toast 是 `userToastInfo`；nil = 这一枪没打或打挂了 —— **不影响整趟结论**
	// （它那个 `isShowCloud` 的语义连旧实现的"播报金额"都对不上，见 upstream 那一条）。
	Toast *upstream.CloudPhoneToastInfo
}

// CloudPhoneStatus 是一次只读盘点的结局档位。
type CloudPhoneStatus int

const (
	// CloudPhoneDone 是"余额与清单都拿到了"。
	//
	// ⚠ 它**不**要求余额位非 nil：上游给了一个合法信封但 `info` 是空的，
	// 结论仍是"看过、这一格没数"，由 [CloudPhoneAssets.SummaryLine] 念成"未知"。
	CloudPhoneDone CloudPhoneStatus = iota
	// CloudPhoneFailed 是没走到能报结论的那一步（换票挂 / 清单挂）。
	CloudPhoneFailed
)

// CloudPhoneResult 是一个账号这一趟的产出。
type CloudPhoneResult struct {
	Account core.AccountID
	Status  CloudPhoneStatus
	Assets  CloudPhoneAssets
	Error   string
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路
// ─────────────────────────────────────────────────────────────────────────────

// CollectCloudPhoneAssets 跑只读那三枪：`userAccountInfo` → `configTaskLoginList` → `userToastInfo`。
//
// # 为什么余额排在清单前面
//
// 这一族的钱是产出，"这次进去到底有没有看到钱"是界面上唯一会被读的一格；
// 清单挂了余额还在，念出来的是"钱有、任务读不到"，反过来则两句都是虚的。
// ⚠ 旧实现的顺序是"清单在前、余额前后各一次"（成文 §2.4 第 373 行），因为它要算差值 ——
// 这里**不写**，所以没有"前一次"，也不假装在算差值。
//
// # 三枪的失败处置**不一样**
//
//   - 余额挂 ⇒ 整趟失败：这一条是这一族对外的产出，报不出它就没有结论可写。
//   - 清单挂 ⇒ 整趟失败：任务表决定"这个号还有没有什么可做的"，缺它等于让用户以为没事做。
//   - 播报挂 ⇒ **只**留 nil：那一格语义未验证（`isShowCloud` 与"播报金额"对不上），
//     为一个读不懂的格子判整趟失败是拿别人的不确定换自己的结论。
func CollectCloudPhoneAssets(
	ctx context.Context, src *CloudPhoneSessionSource,
) (CloudPhoneAssets, *core.Error) {
	balance, perr := cloudPhoneCall(ctx, src,
		func(s CloudPhoneSession, a upstream.CloudPhoneAuth) (upstream.CloudPhoneAccountInfo, *core.Error) {
			return s.callBalance(ctx, src.up, a)
		})
	if perr != nil {
		return CloudPhoneAssets{}, perr
	}

	tasks, perr := cloudPhoneCall(ctx, src,
		func(s CloudPhoneSession, a upstream.CloudPhoneAuth) (upstream.CloudPhoneTaskList, *core.Error) {
			return s.callTaskList(ctx, src.up, a)
		})
	if perr != nil {
		return CloudPhoneAssets{}, perr
	}

	assets := CloudPhoneAssets{Balance: balance, Tasks: tasks}
	// 播报那一枪失败只留 nil —— 见函数头第三条。
	toast, terr := cloudPhoneCall(ctx, src,
		func(s CloudPhoneSession, a upstream.CloudPhoneAuth) (upstream.CloudPhoneToastInfo, *core.Error) {
			return s.callToast(ctx, src.up, a)
		})
	if terr == nil {
		assets.Toast = &toast
	}
	return assets, nil
}

// callBalance / callTaskList / callToast 把"用这张票打这一枪"收在一处。
//
// 为什么要方法而不是直接在 [CollectCloudPhoneAssets] 里写：`referer` 与 `auth`
// 必须**同源同趟**（`sign` 就是这一趟的 `requestId` + `timestamp` 算出来的），
// 收成 session 上的方法之后，调用点没有"漏传 referer 或拿错 auth"的写法。
func (s CloudPhoneSession) callBalance(
	ctx context.Context, up *upstream.Upstream, auth upstream.CloudPhoneAuth,
) (upstream.CloudPhoneAccountInfo, *core.Error) {
	return up.AppCloudPhoneAccountInfo(ctx, auth, s.referer)
}

func (s CloudPhoneSession) callTaskList(
	ctx context.Context, up *upstream.Upstream, auth upstream.CloudPhoneAuth,
) (upstream.CloudPhoneTaskList, *core.Error) {
	return up.AppCloudPhoneConfigTaskLoginList(ctx, auth, s.referer)
}

func (s CloudPhoneSession) callToast(
	ctx context.Context, up *upstream.Upstream, auth upstream.CloudPhoneAuth,
) (upstream.CloudPhoneToastInfo, *core.Error) {
	return up.AppCloudPhoneToastInfo(ctx, auth, s.referer)
}

// ─────────────────────────────────────────────────────────────────────────────
// 跑一个账号那一段
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneRun 跑一次只读盘点。
type CloudPhoneRun struct {
	CloudPhoneWorld
}

// RunAccount 跑**一个账号**的这一笔。
func (r CloudPhoneRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (CloudPhoneResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return CloudPhoneResult{Account: account, Status: CloudPhoneFailed, Error: perr.Error()}, perr
	}
	assets, perr := CollectCloudPhoneAssets(ctx, src)
	if perr != nil {
		result := CloudPhoneResult{Account: account, Status: CloudPhoneFailed, Error: perr.Error()}
		r.progress(core.LevelWarn, result.SummaryLine())
		return result, perr
	}
	result := CloudPhoneResult{Account: account, Status: CloudPhoneDone, Assets: assets}
	r.progress(core.LevelInfo, result.SummaryLine())
	return result, nil
}

// SummaryLine 是给人看的一句话小结（进 `runs.message` 与终局事件，**出网面**）。
//
// # 单位换算只做这一处
//
// 上游那一族金额是**分**（成文 §2.4 第 369 行 + `prizeAmount "20"` 配 `taskExplain "可获得0.2元"`
// 的同条目旁证），而 `upstream` 刻意不做除法。这里是念给人看的那一层，所以**在这里**除，
// 并且**两句都留着**："2.10 元（210 分）" —— 除错了能被看见，而不是只剩一个小数。
//
// # ⚠ 不念 `Balance.Account`（上游给的掩码手机号）
//
// 仓库口径是"成文与输出里的手机号一律掩码且**不带尾号**"（机检抓不到，只能人工审），
// 而上游那个字段的实拍形态是"前三位 + 星号 + **后四位**" —— 带尾号。
// 它是别人家的掩码，不是本仓库可以照抄的写法 ⇒ 不进取决句。
func (res CloudPhoneResult) SummaryLine() string {
	head := "红包派对"
	if res.Status == CloudPhoneFailed {
		if res.Error == "" {
			return head + "没读成"
		}
		return head + "没读成：" + res.Error
	}
	assets := res.Assets
	line := head + "可兑 " + cloudPhoneYuan(assets.Balance.CanAmount) +
		"，累计 " + cloudPhoneYuan(assets.Balance.TotalAmount)
	line += "；任务 " + strconv.Itoa(len(assets.Tasks.Novice)) + "新手/" +
		strconv.Itoa(len(assets.Tasks.Daily)) + "每日/" +
		strconv.Itoa(len(assets.Tasks.Monthly)) + "每月"
	if n := cloudPhonePendingTasks(assets.Tasks); n > 0 {
		line += "（读成未完成的 " + strconv.Itoa(n) + " 件，**本工具今天一件都不代做**）"
	}
	if assets.Toast == nil {
		line += "；播报位没读到（不影响上面两句）"
	}
	return line
}

// cloudPhoneYuan 把"分"念成"X.XX 元（N 分）"。
//
// ⚠ `nil` 念成"未知"而不是"0.00 元"：0 是上游说了"确实没有"，
// 两者混起来的表现是用户以为余额被清零（`upstream.CloudPhoneAccountInfo` 那条注释
// 记的就是这个风险）。
func cloudPhoneYuan(fen *int64) string {
	if fen == nil {
		return "未知（上游没给这一位）"
	}
	return fmt.Sprintf("%.2f 元（%d 分）", float64(*fen)/100, *fen)
}

// cloudPhonePendingTasks 数"读成未完成"的条目。
//
// ⚠ 只数 `userStatus == 0`：`2` 那一档成文写着"未定义"，把它折进"未完成"
// 就是凭空给一件状态未知的任务计数（将来接写请求时这一格会直接决定要不要发请求）。
// 缺 `userStatus` 的条目也**不**数（缺 ≠ 0）。
func cloudPhonePendingTasks(list upstream.CloudPhoneTaskList) int {
	pending := 0
	for _, group := range [][]upstream.CloudPhoneTaskItem{list.Novice, list.Daily, list.Monthly} {
		for _, item := range group {
			if item.UserStatus != nil && *item.UserStatus == upstream.CloudPhoneUserStatusUnfinished {
				pending++
			}
		}
	}
	return pending
}

// RunReport 把结论映射到 [core.RunReport]。
//
// 只读那一半**不产 ManualStep**：ManualStep 的语义是"得有人去 App 里点一下才继续"
// （见 [core.ManualStep]），而今天这一条的用途就是"看一眼"。
// 哪天接了写请求，"哪几件必须人做"（安装类要拍板、`DAILY_1` 需云机内实际使用）
// 才从这里出去 —— 那时要一并过的还有 `runs.capability` 那一列的取值。
func (res CloudPhoneResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	if res.Status == CloudPhoneFailed {
		return core.NewRunReportFailed(summary)
	}
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
