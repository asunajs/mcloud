package app

// 签到页那一条会话的**共用装配**：给一个账号，交出"能直接发请求的那张票"。
//
// # 为什么单独一层
//
// 装到"能发请求"这一步，每条走签到页的链都要做一遍，而它有三处
// **少写一句就静默出错**的地方（原来只写在 [SignInRun.RunAccount] 里）：
//
//   - 档案里的 `x-yun-uni`（用户域）必须**从库里那个账号回填**，拿别人的或拿不到
//     的表现是"凭据都在、却回 90001 未登录"；
//   - 票必须按 (账号, 页面配方) 从 [SessionPool] 拿，漏接池的表现是
//     "界面上连点三次签到 = 重打三枪建会话"（用户 2026-09-23 点名不许）；
//   - 取凭据失败与建会话失败**不能混成一句**：前者是库里没这个角色登录过，
//     后者是登录态坏了，处置完全不同。
//
// 只有签到那一条链时用不着抽出来；等"领取""翻倍""备份好礼""快照""奖品"都要跑
// 同一条会话，各写一遍的代价是这三处**各自漂移**，而漂移不会让任何测试变红。
// 所以 [SignInRun] 也改成走这里的 [SignInWorld.Source]（一份实现，两个产物）。
//
// # 这一层不做什么
//
//   - **不换 H5 `deviceId`**：那是 [DeviceID] 的事，只有领取那条链硬需要它
//     （不带回 `615`），签到与只读查询都不需要 —— 在这里顺手换会让每次"查一下待领取奖品"
//     多一次设备指纹上报。
//   - **不判门槛**：门槛在各能力函数里（[PlanMultiple]、[PlanReceive]…）。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// SignInWorld 是"以签到页身份跑一件事"需要的**世界**。
//
// 字段与 [SignInRun] 同源（同一份装配），差别只在它不带 `Receive` 那类**任务开关**：
// 世界是"能跑"，开关是"跑什么"。
type SignInWorld struct {
	Store    core.StateStore
	Scope    core.Scope
	Upstream *upstream.Upstream
	Device   upstream.AppDevice
	Clock    core.Clock
	Random   core.Random

	// Sessions 是进程内的会话票池；nil ⇒ 回落成"这一次那一条"（CLI 单次调用与测试夹具）。
	// ⚠ 服务端那条路**必须**接，接线点见 `internal/server/run_dispatch.go`。
	Sessions *SessionPool

	// Progress 收"给人看的过程话"，可为 nil（理由同 [SignInRun.Progress]）。
	Progress RunProgress
}

// SignInOpening 是"以签到页身份跑一条链"要**同时**拿在手里的三样。
//
// 为什么要有这一个而不是只给 [SignInWorld.Source]：绝大多数链只要票，但有两条链
// 还要档案（`hcy/file/list` 那一套 `x-yun-*` 头必须由 [SignInWorld.Source] 里那份
// **回填过 `x-yun-uni` 的**档案来发）。自己再 `NewAppProfile(device)` 一份的表现是
// "请求看着完全正常，只有网盘侧回未登录" —— 那正是文件头第一条要防的漂移。
// credential 同理：只有需要 `Authorization` 头的链用得着，交出来比让它再查一遍库好。
type SignInOpening struct {
	Profile    *upstream.AppProfile
	Credential *SignInCredential
	Source     *SignInSessionSource
}

// Open 备档案 → 取凭据 → 拿票，把三样一起交回。
//
// 与 [SignInWorld.Source] 同一份装配（后者就是这里的一层投影），口径也完全一样：
// **不发请求**（建会话是懒的），错误已带前缀、调用方不要再包一层。
func (w SignInWorld) Open(
	ctx context.Context, account core.AccountID,
) (SignInOpening, *core.Error) {
	profile, perr := ProfileWithStoredDomainID(ctx, w.Store, w.Scope,
		upstream.NewAppProfile(w.Device), account)
	if perr != nil {
		return SignInOpening{}, perr
	}
	credential, perr := CredentialFromAccount(ctx, w.Store, w.Scope, account)
	if perr != nil {
		return SignInOpening{}, perr
	}
	return SignInOpening{
		Profile: profile, Credential: &credential,
		Source: sessionSource(w.Sessions, w.Upstream, profile, &credential,
			w.Clock, w.Random, RecipeSignInPage),
	}, nil
}

// Source 备档案 → 取凭据 → 拿票，返回这一 (账号, 签到页) 的那条会话源。
//
// **不发请求**：建会话是懒的（第一次要票时才打那三枪），所以"取一次没用上"不花代价。
// 接了池时它可能**比这一次调用更老** —— 上一次点击建的那条还没被上游判死就继续用
// （口径见 `signin_session_pool.go` 头注）。
//
// 返回的错误已经带前缀，调用方**不要**再包一层"取凭据失败"之类的措辞：
// 两类失败的处置全靠那句话区分（见文件头第三条）。
//
// ⚠ 需要**档案**或**凭据**的链走 [SignInWorld.Open]，不要在这里拿完票再去别处凑档案。
func (w SignInWorld) Source(
	ctx context.Context, account core.AccountID,
) (*SignInSessionSource, *core.Error) {
	opening, perr := w.Open(ctx, account)
	if perr != nil {
		return nil, perr
	}
	return opening.Source, nil
}

// DeviceIDFor 换这条链要的 **H5 `deviceId`**（领取类请求硬需要它）。
//
// 失败**不**在这里判：调用方要么降级继续（签到那条链 —— 少它只是领不到，签到仍做得完），
// 要么把它当成整趟失败（只做领取的那条链）。这里只负责"取一次"，
// 因为取它要**上报设备指纹**，是否值得为一个可选头去上报由调用方决定
// （[ReceiveRevivalReward] 那条就选择不取，见 `signin_claims_run.go`）。
func (w SignInWorld) DeviceIDFor(
	ctx context.Context, account core.AccountID,
) (string, *core.Error) {
	return DeviceID(ctx, w.Upstream, w.Store, w.Scope, account, w.Clock, w.Random)
}

func (w SignInWorld) progress(level core.LogLevel, message string) {
	if w.Progress != nil {
		w.Progress(level, message)
	}
}
