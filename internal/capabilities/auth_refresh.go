// 换 token（刷新凭据）的**判据与落库形状** —— 两端共用一份。
//
// # 为什么判据在这里，不在 app 也不在 server
//
// "该不该刷"与"刷出来的东西落成什么样"是两个**纯**决定，与谁来点这个按钮无关。
// 放在 CLI 或路由里就会出现两份判据：CLI 提前刷、WebUI 压着不刷（或反之），
// 而两边都"看起来对"。本层与 [DecideQrPoll] 那套同理 —— 状态机一份，等待归调用方。
//
// # 三档结论，为什么不是两档
//
// 旧实现在这里留了**两份互相矛盾**的判据（`auth-service.ts` 把 `expire===0` 判成
// "恒需刷新"，`after.ts` 却在 `!expire` 时提前 return），而且任务层把刷新结果**丢弃**
// —— 那个 `auth-refresh` 任务实际不生效（见 `docs/business-feature-inventory.md` §2.2）。
// 收敛成一份时按 2026-09-19 的用户订正裁：
//
//   - `expire < 0` ⇒ [RefreshManualOnly]：**刷新不了，只能人工换 auth**。
//     不自动重试（重试只会白白打上游），也不照抄"恒需刷新"那半边。
//   - `expire == 0`（token 里根本没有第 4 段）⇒ [RefreshNeeded]。
//     这与 [core.ParseAuth] 那处"非有限值归零 → 判已过期 → 触发刷新"同向：
//     **宁可多刷一次，也不把一个来路不明的 token 当成永不过期**。
//   - 其余走 [core.AuthInfo.IsNeedRefresh]（提前 N 天），它**不是**
//     [core.AuthInfo.IsExpiredAt] 的别名 —— "没过期"不代表"不用刷"。
package capabilities

import (
	"context"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// RefreshDefaultLeadDays 是"还剩几天就该刷"的默认提前量。
//
// 对齐旧实现的 `DEFAULT_REFRESH_DAYS = 7`（`docs/business-feature-inventory.md` §2.2）。
// 它是**产品取值**：太短会在真过期后才动手，太长会白打上游。
const RefreshDefaultLeadDays = 7.0

// RefreshDecision 是"这次要不要刷上游"的三档结论。
type RefreshDecision int

const (
	// RefreshNeeded 是该刷 —— 包含"无过期信息"这一档（理由见包注释）。
	RefreshNeeded RefreshDecision = iota
	// RefreshNotYet 是还没到提前量，**默认不发请求**。
	//
	// 唯一越过它的入口是 `force`（见 [RefreshAuth] 那条参数注释）：那是人已拍板
	// "现在就换"，本层不再替他决定"你其实不需要"。
	RefreshNotYet
	// RefreshManualOnly 是上游刷不了（`expire < 0`），只能人工换 auth。
	//
	// ⚠ 它**不是**失败：把它当失败会记进 `fail_streak`，于是"这个账号本来就没法刷"
	// 过几天变成"账号被停了"。
	RefreshManualOnly
)

// String 返回结论的中文名（给 CLI 与日志用，不给机器解析）。
func (d RefreshDecision) String() string {
	switch d {
	case RefreshNeeded:
		return "需要刷新"
	case RefreshNotYet:
		return "还没到提前量"
	case RefreshManualOnly:
		return "只能人工换 auth"
	}
	return "未知结论"
}

// Code 是给接口用的稳定标识（`POST /api/accounts/{id}/refresh-auth` 响应里的
// `decision`）。
//
// 与 [RefreshDecision.String] 分成两个方法，是因为两者的改动节奏不同：
// 中文那句是文案，随时可以改措辞；这句进了 JSON 契约，前端按它分支，
// 改它的值等于改契约。所以要加一档就**加新值**，别改这三个旧值。
//
// 兜底那句给的是 `"unknown"`：真收到它说明有新档位没同步到前端，
// 那比让前端拿到一个空串去 `switch` 更容易查。
func (d RefreshDecision) Code() string {
	switch d {
	case RefreshNeeded:
		return "needed"
	case RefreshNotYet:
		return "not-yet"
	case RefreshManualOnly:
		return "manual-only"
	}
	return "unknown"
}

// DecideRefresh 由整串 `auth` 与"现在"判该不该刷（**纯函数**，时间由调用方注入）。
//
// `now` 用 [time.Time] 而不是毫秒数：本层其它地方（落库的 `updated_at`）也是这个类型，
// 混两种时间表示正是"单位漂移"最常发生的地方。
//
// # 为什么解不出来时报错而不是"当作需要刷新"
//
// 判据三档里没有任何一档对应"这串根本不是 auth"。把它归进 [RefreshNeeded]
// 会让一次注定失败的请求替我们说出"刷不动"，而真正的原因（这串用不了）被埋掉。
func DecideRefresh(auth string, now time.Time, leadDays float64) (RefreshDecision, *core.Error) {
	parsed, ok := core.ParseAuth(auth)
	if !ok {
		return RefreshNeeded, core.NewBadRequest("auth 解析不出来，无法判断该不该刷新")
	}
	expire := parsed.ExpireMillis()
	if expire < 0 {
		return RefreshManualOnly, nil
	}
	if expire == 0 {
		return RefreshNeeded, nil
	}
	if parsed.IsNeedRefresh(now.UnixMilli(), leadDays) {
		return RefreshNeeded, nil
	}
	return RefreshNotYet, nil
}

// RefreshedCredential 是刷新结果落库前的**全部**新值（纯数据，不含账号 id）。
//
// 只有这三样是有新值的：其余列（`device_id` / `user_domain_id` / `smid_*` / 昵称…）
// 一律不动 —— 换钥匙不换设备长相（同一理由见 [core.StateStore.RotateAccountAuth]）。
type RefreshedCredential struct {
	// Authorization 是新的整串（`Basic base64("<端>:<手机号>:<新 token>")`）。
	Authorization core.SecretValue
	// AuthToken 是新 token 本体，也就是整串解码后的第三段。
	AuthToken core.SecretValue
	// ExpireMillis 是新串的过期时刻（**epoch 毫秒**）；nil = 新串不带过期信息。
	//
	// ⚠ 单位刻意写进名字：上游 `expireTime` 给的是**秒**且是"还能用多久"
	// （见 [upstream.RefreshTokenResult]），与这一列不是一回事。
	ExpireMillis *int64
}

// CredentialFromRefresh 用**当前整串**与新 token 拼出新凭据（纯函数）。
//
// # 为什么前缀与手机号都从当前整串里取，而不是当参数传进来
//
// 这两样决定"新凭据冒充的是谁、在哪个端"。让它们成为入参，就等于让调用方
// 有机会把 PC 串的端别写成 App（`mobile` ↔ `pc` 互换静默通过 —— 那正是
// [AuthFromUserInfoWithPrefix] 拒绝默认值的同一个理由），而刷新结果只有到
// 上游报 4xx 那天才看得见。
//
// # 为什么要"拼完再解一遍"
//
// 拼出来的串必须能解回**同一个**手机号与 token。解不出、或对不上，
// 说明新 token 里带了 `:` 之类把分段搞坏的东西（[core.ParseAuth] 只取前三段，
// 第四个冒号之后直接丢弃 —— 这是原版怪癖，见 core/auth.go 包注释）。
// 不兜这一下的话，坏串会被写进库、当场把账号刷成不可用。
func CredentialFromRefresh(currentAuth string, newToken string) (RefreshedCredential, *core.Error) {
	if newToken == "" {
		return RefreshedCredential{}, core.NewBadRequest("新 token 是空的，不能覆盖库里那份")
	}
	current, ok := core.ParseAuth(currentAuth)
	if !ok {
		return RefreshedCredential{}, core.NewBadRequest("当前 auth 解析不出来，无法拼新串")
	}
	if current.Platform == nil || *current.Platform == "" {
		return RefreshedCredential{}, core.NewBadRequest(
			"当前 auth 里没有端别那一段（`pc` / `mobile`），拼不出新串")
	}
	phone := ""
	if p := current.PhoneOrNil(); p != nil {
		phone = *p
	}
	if phone == "" {
		return RefreshedCredential{}, core.NewBadRequest("当前 auth 里没有手机号，无法确定刷的是谁")
	}

	auth, perr := authFromParts(*current.Platform, phone, newToken)
	if perr != nil {
		return RefreshedCredential{}, perr
	}
	reparsed, ok := core.ParseAuth(auth)
	if !ok {
		return RefreshedCredential{}, core.NewParse("拼出来的新 auth 解不回去（token 形状有问题）")
	}
	if reparsed.Token == nil || *reparsed.Token != newToken {
		return RefreshedCredential{}, core.NewParse(
			"新 auth 解出的 token 与上游给的不是同一个（token 里带了冒号，被分段规则截断）")
	}
	if p := reparsed.PhoneOrNil(); p == nil || *p != phone {
		return RefreshedCredential{}, core.NewParse("新 auth 解出的手机号与原串不一致")
	}

	credential := RefreshedCredential{
		Authorization: core.NewSecretValue(reparsed.Auth),
		AuthToken:     core.NewSecretValue(newToken),
	}
	if millis := reparsed.ExpireMillis(); millis != 0 {
		credential.ExpireMillis = &millis
	}
	return credential, nil
}

// UserDomainForRefresh 取这个账号刷上游要用的 `userDomainId`。
//
// 与登录那条的 [ProfileWithStoredDomainID] **相反**：那条缺失时原样放行
// （登录不需要它），这条缺失时直接判"刷不了"。理由在请求体里 ——
// `{"userDomainId":…,"clientType":…}` 缺了它，上游要么拒、要么定位到**别人的**用户域，
// 两种都比"这次不刷"糟。编一个更不能接受。
func UserDomainForRefresh(
	ctx context.Context, store core.StateStore, scope core.Scope, id core.AccountID,
) (string, *core.Error) {
	state, err := store.GetAccountState(ctx, scope, id)
	if err != nil {
		return "", asError(err, "读登录态失败")
	}
	if state == nil || state.UserDomainID == nil || state.UserDomainID.String() == "" {
		return "", core.NewBadRequest(
			"这个账号没有用户域标识（`user_domain_id` 为空），刷不了 —— " +
				"通常是粘贴进来的凭据或旧数据；重新登录一次即可拿到")
	}
	return state.UserDomainID.String(), nil
}

// PersistRefreshedAuth 把上游给的新 token 落成库里那一套新凭据。
//
// 返回的 [RefreshedCredential] 就是实际写进去的东西（调用方要汇报"过期时刻从
// 哪天挪到哪天"时用它，而不是自己再拼一遍）。
//
// # 为什么这里不重复判"该不该刷"
//
// 判据只有一份（[DecideRefresh]），由调用方在**打上游之前**跑。
// 落库前再判一次不会更保险，只会多出第二种"为什么没写进去"。
func PersistRefreshedAuth(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	id core.AccountID,
	newToken string,
	now time.Time,
) (RefreshedCredential, *core.Error) {
	state, err := store.GetAccountState(ctx, scope, id)
	if err != nil {
		return RefreshedCredential{}, asError(err, "读登录态失败")
	}
	if state == nil || state.Authorization == nil || string(*state.Authorization) == "" {
		// 没有整串就没有"当前端别 + 当前手机号"，新串拼不出来（见
		// [CredentialFromRefresh]：这两样都不作为参数收）。
		return RefreshedCredential{}, core.NewBadRequest(
			"这个账号没有存 `Authorization` 整串，无法刷新；重新登录一次即可")
	}

	credential, perr := CredentialFromRefresh(string(*state.Authorization), newToken)
	if perr != nil {
		return RefreshedCredential{}, perr
	}
	if storeErr := store.RotateAccountAuth(ctx, scope, id,
		credential.Authorization, credential.AuthToken, credential.ExpireMillis, now); storeErr != nil {
		return RefreshedCredential{}, asError(storeErr, "写入新凭据失败")
	}
	return credential, nil
}

// RefreshSender 打**一次**刷新请求并给出解析结果。
//
// 它收 `userDomainId`（而不是自己再去读一遍库）：本层已经把它读出来并判过
// "缺失即刷不了"，让实现方重读一次等于把那条判据写成两份。
//
// # 为什么是一个函数参数，而不是直接收 `*upstream.Upstream`
//
// 两端的"打一次"要准备的东西完全不同：App 端要先取服务端毫秒时间戳
// （`x-timestamp` 与本地时钟差一点就是 401），PC 端要先有一个 `x-yun-tid`。
// 本层若把这两种准备都收进来，就要么持有 [upstream.AppProfile] 的构造细节、
// 要么在函数签名上为两端各开一条路 —— 而"该不该刷、刷完落成什么样"这部分
// 其实两端**共用一份**。把它对上"怎么打"这个差异，收成注入点就够了。
//
// 等待与重试也在这条缝外面（见 internal/app 的驱动层，与该包里 `Waiter`
// 的注入是同一套理由）。
type RefreshSender func(ctx context.Context, userDomainID string) (upstream.RefreshTokenResult, *core.Error)

// RefreshOutcome 是一次刷新尝试的结论。
type RefreshOutcome struct {
	// Decision 是最终判到的那一档。**不保证**打过上游：
	// [RefreshNotYet] 与 [RefreshManualOnly] 两档下 `Credential` 是零值。
	Decision RefreshDecision
	// Credential 是实际写进库里的新凭据（打过上游且成功时才有值）。
	Credential RefreshedCredential
	// Rotated 表示这一轮确实换成了新凭据（用来把"刷成功了"与"没到点、什么都没做"
	// 分开 —— 后者不该被汇报成成功）。
	Rotated bool
}

// RefreshAuth 走完一条刷新链路：判 → （必要时）取 `userDomainId` → 打一次上游 → 落库。
//
// `now` 由调用方注入而不是这里 `time.Now()`：判据是**纯函数**的一部分，
// 测试要能在"提前 6 天 / 提前 8 天"两侧各钉一次，靠真实时钟做不到。
//
// # `force` 越过的只有 [RefreshNotYet] 那一档
//
// `force = true` 是"人已拍板现在就换"，于是提前量那档不再拦 —— 提前量是给**批量扫**
// 用的省请求判据（一天扫一次、没到点的一个都不发），不是给一次带确认弹窗的手动
// 操作设的门槛。但 [RefreshManualOnly] **不**跟着越：那一档说的是"库里这串根本
// 刷不了"（`expire < 0`，上游不认），越过它只会白打一次上游并换一个注定失败的请求。
//
// `Decision` 仍如实报判到的那一档，`Rotated` 报有没有真换成 —— 于是
// "未到点但被手动换了"是 `not-yet` + `rotated=true` 这一对组合，不需要新加一档
// （[RefreshDecision.Code] 那三个码进了 JSON 契约，加档要前端同步改分支）。
//
// # 为什么"没到提前量"是正常返回而不是错误
//
// 定时扫一遍所有账号时，绝大多数轮次都落在这一档。把它做成错误会让
// 调度层用 `fail_streak` 计数，于是"今天还不用刷"几天后变成"账号被停了"
// （同一理由见 [RefreshManualOnly]）。
func RefreshAuth(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	id core.AccountID,
	now time.Time,
	leadDays float64,
	force bool,
	send RefreshSender,
) (RefreshOutcome, *core.Error) {
	state, err := store.GetAccountState(ctx, scope, id)
	if err != nil {
		return RefreshOutcome{}, asError(err, "读登录态失败")
	}
	if state == nil || state.Authorization == nil || string(*state.Authorization) == "" {
		return RefreshOutcome{}, core.NewBadRequest(
			"这个账号没有存 `Authorization` 整串，无法刷新；重新登录一次即可")
	}
	currentAuth := string(*state.Authorization)

	decision, derr := DecideRefresh(currentAuth, now, leadDays)
	if derr != nil {
		return RefreshOutcome{}, derr
	}
	outcome := RefreshOutcome{Decision: decision}
	// 只有 manual-only 是"force 也救不了"的那一档（理由见上面那条注释）。
	if decision == RefreshManualOnly || (decision == RefreshNotYet && !force) {
		return outcome, nil
	}

	// 只有真要打上游时才要求 `userDomainId`：不刷的时候缺它也无所谓，
	// 报"刷不了"反而把一个能用的账号说成坏的。
	userDomainID, uerr := UserDomainForRefresh(ctx, store, scope, id)
	if uerr != nil {
		return outcome, uerr
	}
	result, serr := send(ctx, userDomainID)
	if serr != nil {
		return outcome, serr
	}
	// 落库走那一份共用的实现（读整串 → 拼新串 → 窄写），不在这里重写一遍：
	// "新凭据落成什么样"有两份的话，漂移只会在下次登录报 401 时被发现。
	credential, perr := PersistRefreshedAuth(ctx, store, scope, id, result.Token, now)
	if perr != nil {
		return outcome, perr
	}
	outcome.Credential = credential
	outcome.Rotated = true
	return outcome, nil
}
