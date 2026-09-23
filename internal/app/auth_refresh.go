package app

// 刷新 auth 的**驱动**：读库认出这是哪个端 → 备齐那一端要的头 → 打一次 → 落库。
//
// # 这一层在补什么（与 [capabilities.RefreshAuth] 的分工）
//
// 判据（该不该刷）与新串的形状（落成什么样）都在 capabilities，那两份**两端共用**。
// 本层只剩一件两端不共用的事：**这个端的这次请求要哪些头**——
//
//   - App：设备档案（`deviceId` + `deviceShort`）+ `x-yun-uni`（用户域）+ 微秒时间戳；
//   - PC：客户端档案（平台 + `deviceId`）+ 一个**每次新生成**的 `x-yun-tid`。
//
// 这两套准备与网络/磁盘都有关（设备档案可能现生成），所以它们必须在驱动层，
// 不能下沉到 capabilities（那层零 IO），也不能上浮到 CLI/路由（那两层只做形状）。
//
// # 端别**不**作为参数收
//
// 端别从库里那一串 `Authorization` 的第一段读出来。让它可由调用方指定，
// 就等于允许"PC 登录的账号用 App 的头去刷" —— 那种组合我们没有实测过，
// 而它的失败长得像"凭据坏了"（同 [SignInCredential] 那道前缀闸的理由）。
//
// # 不重试
//
// 刷新是**写库**操作：一次请求可能已经把 token 换掉了，只是响应没回来
// （实测那一次慢到 28.4 秒 —— 超过任何合理的本地超时）。
// 这时自动重试等于"拿一个可能已经作废的旧凭据再刷一次"。
// 所以本层一次只发**一个**刷新请求，失败就把原因原样交回调用方，由人决定要不要再来。

import (
	"context"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// RefreshDeps 是一次刷新要注入的东西。
//
// # 为什么 `AppDevice` 与 `PCPlatform` 可以同为 nil
//
// 一次只会用到其中一个：账号是 App 端登录的就用 `AppDevice`，PC 端的就用
// `PCPlatform`。另一侧留 nil 是**正常**的（"这台机器上只有 PC 账号"），
// 所以缺失只在"真要走近那一支"时才报错 —— 启动时一把检查两个都会误伤。
type RefreshDeps struct {
	// Upstream 是发请求的上游层。
	Upstream *upstream.Upstream
	// Store 是账号库（读整串 + 窄写新凭据）。
	Store core.StateStore
	// Scope 是可见范围（读与写用同一个，见 [capabilities.RefreshAuth]）。
	Scope core.Scope
	// Clock 提供"现在"（判提前量）与 App 端的微秒时间戳。
	Clock core.Clock
	// Random 给 PC 端现生成 `x-yun-tid`。
	Random core.Random
	// AppDevice 是 App 端的设备档案（`deviceId` + `deviceShort`）。
	AppDevice *upstream.AppDevice
	// PCPlatform 是 PC 端档案的平台（决定 `clientType` 与 `DEVICE_INFO`）。
	PCPlatform upstream.PcPlatform
	// LeadDays 是"还剩几天就该刷"；0 用 [capabilities.RefreshDefaultLeadDays]。
	//
	// 它只在 `Force` 为 false 时起作用 —— 那是一台机器替人决定"今天还不用动"，
	// 而手动那一路（WebUI 上带确认弹窗的这一枚）不该有这道闸，见下。
	LeadDays float64
	// Force 是"人已经拍板现在就换"：它只越过 [capabilities.RefreshNotYet] 那一档，
	// manual-only（库里那串根本刷不了）与"没有整串 / 没有设备档案"那几道闸照旧。
	//
	// ⚠ 它做的事是**覆盖库里那套凭据**（`RotateAccountAuth` 写 `authorization` /
	// `jwt_token` / `accounts.auth_expire`，窄写门禁断言过其余列不动）。至于换下去之后
	// 旧那串还能不能用、会不会影响别的端上正在用的会话 —— **未实测**，本仓库今天
	// 所有验证都走假上游，`rotated=true` 一次真的都没发生过，所以这里不下断言。
	Force bool
}

// RefreshReport 是一次刷新的结果（**不含任何凭据**，可直接序列化给前端）。
type RefreshReport struct {
	// AccountID 是账号 id（掩码形态）。
	AccountID core.AccountID
	// End 是这一支走的是哪个端（`Authorization` 的第一段，原样）。
	End string
	// Decision 是判到的那一档。
	Decision capabilities.RefreshDecision
	// Rotated 表示真的换成了新凭据。
	Rotated bool
	// BeforeExpire / AfterExpire 是换前后的过期时刻（epoch 毫秒）。
	//
	// ⚠ 单位在名字里：库里的 `auth_expire` 与 token 第 4 段都是**毫秒**，
	// 而上游响应里那个 `expireTime` 是**秒且是相对时长**（见
	// [upstream.RefreshTokenResult]）—— 三者同屏时最容易看错的就是这里。
	// nil = 那一侧根本没有过期信息。
	BeforeExpire *int64
	AfterExpire  *int64
}

// RefreshAccountAuth 刷**一个**账号的凭据。
//
// 返回错误时库里那一套**原样未动**（窄写在一个事务里，见
// [core.StateStore.RotateAccountAuth]）。
func RefreshAccountAuth(
	ctx context.Context, deps *RefreshDeps, accountID core.AccountID,
) (RefreshReport, *core.Error) {
	leadDays := deps.LeadDays
	if leadDays == 0 {
		leadDays = capabilities.RefreshDefaultLeadDays
	}

	state, err := deps.Store.GetAccountState(ctx, deps.Scope, accountID)
	if err != nil {
		return RefreshReport{}, asAppError(err, "读登录态失败")
	}
	if state == nil || state.Authorization == nil || string(*state.Authorization) == "" {
		return RefreshReport{}, core.NewBadRequest(
			"账号 " + accountID.String() + " 没有存 `Authorization` 整串，刷不了；重新登录一次即可")
	}
	parsed, ok := core.ParseAuth(string(*state.Authorization))
	if !ok {
		// 与 [capabilities.DecideRefresh] 同一个判据，但这里的报错多带一个账号 id：
		// 批量刷新时"哪一个刷不了"只有在这一层才知道。
		return RefreshReport{AccountID: accountID}, core.NewBadRequest(
			"账号 " + accountID.String() + " 库里那串 auth 解析不出来，无法刷新")
	}
	end := ""
	if parsed.Platform != nil {
		end = *parsed.Platform
	}
	report := RefreshReport{AccountID: accountID, End: end}
	if millis := parsed.ExpireMillis(); millis > 0 {
		value := millis
		report.BeforeExpire = &value
	}

	sender, serr := refreshSenderFor(deps, end, accountID,
		string(*state.Authorization), deviceIDOf(state))
	if serr != nil {
		return report, serr
	}

	outcome, rerr := capabilities.RefreshAuth(
		ctx, deps.Store, deps.Scope, accountID, deps.Clock.Now(), leadDays, deps.Force, sender)
	report.Decision = outcome.Decision
	report.Rotated = outcome.Rotated
	if outcome.Credential.ExpireMillis != nil {
		value := *outcome.Credential.ExpireMillis
		report.AfterExpire = &value
	}
	if rerr != nil {
		return report, rerr
	}
	return report, nil
}

// refreshSenderFor 按端别准备好"打一次"要用的东西，返回那个闭包。
//
// 它只做**准备**，不发请求、也不判该不该刷（那在 capabilities）。
// 之所以在判定之前就把它建好：建这一支要的头（设备档案、tid）与"要不要刷"无关，
// 而建不出来（这个端没有档案）是一件**该立刻说清楚**的事，
// 不该等用户按了"刷新"、看到"还没到提前量"之后再发现。
func refreshSenderFor(
	deps *RefreshDeps,
	end string,
	accountID core.AccountID,
	authorization string,
	deviceID string,
) (capabilities.RefreshSender, *core.Error) {
	switch end {
	case upstream.IOSAuthPrefix:
		return appRefreshSender(deps, accountID, authorization)
	case capabilities.PCAuthPrefix:
		// 这一支**未实测**（理由见 internal/upstream/auth_refresh.go 文件头那张表）。
		//
		// 档案的 `deviceID` 一律取自库里：`DEVICE_INFO` 里带着它，它属于
		// "这台设备当时登录用的那一台"。现生成的表现是"凭据都对、却回未登录"，
		// 看起来像凭据坏了。没有它就只能报错，**不**退化成用掩码 id 去凑。
		if deviceID == "" {
			return nil, core.NewBadRequest(
				"账号 " + accountID.String() + " 库里没有 `device_id`，" +
					"PC 端刷新拼不出设备档案；重新登录一次即可")
		}
		// `clientType` 取 Windows 还是 macOS 库里没记，所以由调用方显式给
		// （[RefreshDeps.PCPlatform]），本层不设默认值 —— 猜一个的表现是
		// "刷成功了，但下次登录被当成另一台设备"。
		profile := upstream.NewClientProfile(deps.PCPlatform, deviceID)
		return func(ctx context.Context,
			userDomainID string) (upstream.RefreshTokenResult, *core.Error) {
			// `x-yun-tid` 每次新生成：实测 PC 端每个会话都是一个新 tid，
			// 复用上一次的表现是风控把它当同一次重放。
			return deps.Upstream.PCRefreshToken(
				ctx, profile, upstream.NewSessionID(deps.Random), authorization, userDomainID)
		}, nil
	}
	return nil, core.NewBadRequest(
		"账号 " + accountID.String() + " 的 auth 端别是 `" + end +
			"`，本工具只刷 `" + capabilities.PCAuthPrefix + "`（PC 客户端）与 `" +
			upstream.IOSAuthPrefix + "`（App）这两种")
}

// appRefreshSender 走**实拍过**的那一支。
//
// 时间戳用 [UserTimestamp]（**微秒**）：这个头与本地时钟一致就行，
// 不像 H5 签名那样必须取服务端时间戳 —— 别把那一条规矩搬到这里来多加一次 IO。
func appRefreshSender(
	deps *RefreshDeps, accountID core.AccountID, authorization string,
) (capabilities.RefreshSender, *core.Error) {
	if deps.AppDevice == nil {
		return nil, core.NewBadRequest(
			"账号 " + accountID.String() + " 是 App 端登录的，但这次没给 App 设备档案" +
				"（`--app-device-file` / 组装层的 `AppDevice`）")
	}
	profile := upstream.NewAppProfile(*deps.AppDevice)
	timestamp := UserTimestamp(deps.Clock)
	return func(ctx context.Context, userDomainID string) (upstream.RefreshTokenResult, *core.Error) {
		// 用户域由 capabilities 读出来传进来（它同时是"缺了就刷不了"的那道闸），
		// 这里只把它装回头里 —— 不重读一遍库，免得两处对"有没有用户域"说法不一。
		withDomain := profile.WithUserDomainID(userDomainID)
		return deps.Upstream.AppRefreshToken(
			ctx, withDomain, timestamp, authorization, userDomainID)
	}, nil
}

// deviceIDOf 取登录态里的 `device_id`，缺失时给空串（调用方按"没有"处理）。
func deviceIDOf(state *core.AccountState) string {
	if state == nil || state.DeviceID == nil {
		return ""
	}
	return string(*state.DeviceID)
}

// ─────────────────────────────────────────────────────────────────────────────
// 服务端装配：把"随时能刷一次"要的世界收在一处
// ─────────────────────────────────────────────────────────────────────────────

// AuthRefresher 是 HTTP 那一侧的刷新入口：常驻世界持有一份，每次调用只给
// "刷哪个账号 + 判据参数"。
//
// # 为什么路由不直接拼 [RefreshDeps]
//
// 拼那一份要**读盘**（App 端的 `deviceId` 是"第一次用到才生成"的文件），
// 而"哪一端要哪些档案"这张表只能有一份 —— 路由抄一遍的话，漂移出来的表现是
// "CLI 刷 App 账号成功、WebUI 失败"（或反之），而那正是最难查的那种差别。
// 本层已经因为同一个理由持住 [RefreshAccountAuth]，这个类型只是它的常驻形态。
type AuthRefresher struct {
	store core.StateStore
	up    *upstream.Upstream
	clock core.Clock
	rng   core.Random
	// appDeviceFile 是**路径**而不是字符串：指纹必须跨重启稳定，
	// 所以第一次用到才读盘、读不到就生成并写回（理由同 [LoginHub] 那两个字段）。
	appDeviceFile string
}

// NewAuthRefresher 装配服务端那份刷新入口。设备档案**不在这里读**：
// 没登录过 App 的安装不该因为"起了一个服务端"就多一个文件。
func NewAuthRefresher(
	store core.StateStore, up *upstream.Upstream, clock core.Clock, rng core.Random,
	appDeviceFile string,
) *AuthRefresher {
	return &AuthRefresher{store: store, up: up, clock: clock, rng: rng, appDeviceFile: appDeviceFile}
}

// Refresh 刷**一个**账号。
//
// `leadDays` 给 0 走默认；`platform` 只在走到 PC 那一支时才用（库里没记
// windows/macos，只能由调用方拍板 —— 见 [RefreshDeps].PCPlatform 那条注释）。
//
// `force` 是"人已拍板现在就换"（WebUI 那一枚带确认弹窗的按钮给 true）。
// 这一层是**单个账号**的入口，所以默认值偏保守不设在这里，由调用方显式给：
// 到点才换的那两路（服务进程内的调度循环、CLI 的 `--all`）走的是
// [RefreshDeps].LeadDays 那道省请求的判据，不该被谁顺手置 true。
//
// ⚠ 每次调用现读一次设备档案：一次 HTTP 请求就刷一个账号，
// 这已经等价于 CLI 的"整次运行只解析一次"。若把它挪进循环，
// 每个账号会拿到不同的 `deviceShort`，与它登录时那台对不上。
func (r *AuthRefresher) Refresh(
	ctx context.Context,
	scope core.Scope,
	accountID core.AccountID,
	leadDays float64,
	platform upstream.PcPlatform,
	force bool,
) (RefreshReport, *core.Error) {
	deviceID, perr := LoadOrCreateAppDeviceID(r.appDeviceFile, r.rng)
	if perr != nil {
		return RefreshReport{}, perr
	}
	device := upstream.NewIPadAppDevice(deviceID, NewAppDeviceShort(r.rng))
	return RefreshAccountAuth(ctx, &RefreshDeps{
		Upstream:   r.up,
		Store:      r.store,
		Scope:      scope,
		Clock:      r.clock,
		Random:     r.rng,
		AppDevice:  &device,
		PCPlatform: platform,
		LeadDays:   leadDays,
		Force:      force,
	}, accountID)
}
