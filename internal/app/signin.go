package app

// 签到（云朵中心）的编排：**换 SSO token → 建会话 → 查信息 → 签到 → 领取**。
//
// # 这一层在补什么
//
// [upstream] 的 `app_signin` 只给"一个请求怎么拼、一个响应怎么读"；
// 它不知道"先做哪一步、什么时候该停、凭什么算成功"。那些判断都在本文件这一族里。
//
// 整条链路是四段（详见 `docs/capture/app-signin-samples.txt`）：
//
//	① querySpecTokenV2  原生 A 族头 + Authorization  → SSO token（很短命）
//	② api/timestamp     取服务端毫秒 → 算 x-signature
//	③ tyrzLogin         带签名 → jwtToken（905 字符，2 小时）
//	④ infoV3 / startSignIn   带 jwttoken 头 —— 真正的签到
//
// # 三条判据（都是踩过的坑，不是设计洁癖）
//
//  1. **成败一律回查 `infoV3`**，不看 `startSignIn` 的 `todaySignIn`。
//     那个字段是**调用前**的快照：实测 `todaySignIn:false` 的那次恰恰把今天签上了。
//     见 [SignIn] 末尾的回查与 [SignInReport.SignedNow]。
//  2. **`x-timestamp` 必须取自服务端**（第 ② 步），不能用本地时钟 ——
//     本地偏一点就是 `401`，而 `401` 在日志里和"盐抄错了"长得一模一样。
//  3. **`Authorization` 拼不出来就不要猜**。见 [SignInCredential]。
//
// # 不做什么
//
//   - **不把 H5 的 `jwtToken` 落库**。它 2 小时就废，而
//     `account_state.jwt_token` 装的是**登录凭据**（App 的 `authToken`）——
//     两者同名不同物，写进去等于把可长期复用的凭据覆盖成一张两小时的票。
//   - **不写云朵余额**。`infoV3` 的 `beforeTotal` 只读出来给人看；
//     落库走 `cloud`（当前值 + 日快照），由**调用方**显式调。
//     不在这里顺手写，是因为余额有四个上游来源（见那个模块的说明），
//     签到只是最早跑通的那一个 —— 写在这里，第二个来源出现时就只能再写一次。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// UserTimestamp 是 `x-yun-user-timestamp` 的值：**微秒**，不是毫秒。
//
// 16 位是微秒、13 位才是毫秒 —— 写错一个量级的表现是上游回 401，
// 而 401 在日志里和"盐抄错了"长得一模一样。
//
// ⚠ 它原本在 Rust 侧的 `app_login` 里（`app_login::user_timestamp`），
// 签到这一族是它目前唯一的消费者，所以先落在这里；`app_login` 迁过来时
// 从本文件取，不要在那边再写一份。
func UserTimestamp(clock core.Clock) int64 { return clock.Now().UnixMicro() }

// ─────────────────────────────────────────────────────────────────────────────
// 凭据
// ─────────────────────────────────────────────────────────────────────────────

// SignInCredential 是一次签到的凭据。
//
// # 为什么单独成结构，而不是三个字符串参数
//
// 这三样凑齐才是"以谁的身份签到"，分开传容易在调用点被摆错顺序，
// 而它们都是字符串，摆错了编译器不会吭声。
//
// # 手机号还需要吗（**v7 起不需要了**）
//
// `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`。
// 早先的写法是把它**拆开存**（只留第三段 token），用的时候再拼回去 —— 于是必须凑齐
// 前缀与手机号，而这两样都推不出来（`accounts.id` 是掩码、`jwt_token` 里没有号码），
// 只能由调用方每次在命令行上补（`--phone`）。那是缺口不是设计。
//
// **v7 起登录把整串原文存进 `account_state.authorization`**，用的时候原样发出去 ——
// 一段都不用凑，`phone` 列从此只是**标识**（打印、排障），不参与拼头。
// 旧项目 `mcloud-sign` 一直就是这么做的（它的 `asign.json` 里 `auth` 存的就是整串），
// 没有重新发明的必要。
//
// 老行（v7 之前登录、库里没有整串的）退回"用 `phone` + `jwt_token` 现拼"，
// 那条路的前缀由 [upstream.AppAuthorization] 硬编码成 `mobile` —— 见 [loadCredential]。
//
// ⚠ 无论走哪条路，**都不要**拿掩码值去凑这个头 —— 服务端认不认掩码是个未知数，
// 而这类"能过但不该过"的东西一旦过了一次，就会变成一条没人敢动的隐性依赖。
type SignInCredential struct {
	// phone 是完整手机号。**不参与拼头**（v7 起），只用于日志里认出"签的是谁"。
	phone string
	// authorization 是 `Authorization` 头。含凭据，[SignInCredential.String] 里隐去。
	authorization string
	// accountID 是账号 id（掩码），用于定位库里的行。
	accountID core.AccountID
}

// CredentialFromAccount 从库里已存的登录态取签到凭据（**推荐路径**，
// 也是唯一不需要用户补参数的路）。
//
// v7 起库里存的是整串 `Authorization`，所以只要一个掩码 id 就够了 ——
// 手机号、前缀、token 全在里面。老行（v7 之前登录的）会退回现拼，
// 那时才需要 `phone` 列；该列也是 NULL 就报错并给出两条出路，
// **不会**退化成用掩码去凑。
func CredentialFromAccount(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	accountID core.AccountID,
) (SignInCredential, *core.Error) {
	return loadCredential(ctx, store, scope, accountID, "")
}

// CredentialFromPhone 同上，但**强制**用调用方给的手机号现拼 —— 给 `phone`
// 列还是 NULL 的老账号用。
//
// `phone` 必须是**完整手机号**。账号定位方式与登录落库时一致
// （同一个 [core.AccountLabel]），所以"登录时存进去的"和"签到时要读的"必然是同一行。
//
// ⚠ 这条会**跳过**整串优先（哪怕库里存着整串），拼出来的前缀固定是 `mobile`。
// 所以它只是老行的逃生口，新调用方一律走 [CredentialFromAccount]。
//
// ⚠⚠ **它的 `Authorization()` 是拼出来的，不是拿到的** —— 调用方可以把
// `phone` 落库（那是用户给的事实），但**绝不要**把这个串写进
// `account_state.authorization`。那一列的口径是"服务端给的原文"，写进去等于
// 给一个本地合成物伪造来源：它会走整串优先路径、**绕过那道前缀闸**，
// 于是此后每个请求都带着一个我们自己安上去的前缀，且再也没人核对。
// 只记 `phone` 就够了 —— 下次照旧现拼，闸照旧生效。
func CredentialFromPhone(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	phone string,
) (SignInCredential, *core.Error) {
	return loadCredential(ctx, store, scope, core.NewAccountID(core.AccountLabel(nil, phone)), phone)
}

// loadCredential 是两条公开路径的共同实现。`phoneOverride` 为空时从库里读。
func loadCredential(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	accountID core.AccountID,
	phoneOverride string,
) (SignInCredential, *core.Error) {
	state, err := store.GetAccountState(ctx, scope, accountID)
	if err != nil {
		return SignInCredential{}, asAppError(err, "读登录态失败")
	}
	if state == nil {
		return SignInCredential{}, core.NewAuth(
			"本地库里没有账号 " + accountID.String() +
				"。先跑一次 App 端登录：`mcloud login sms --client app`")
	}

	// ── 首选：整串 auth（v7 起）────────────────────────────────────────
	//
	// 库里存的就是 `Basic base64("<前缀>:<手机号>:<authToken>")` 的**原文**，
	// 前缀在里面、手机号在里面、token 在里面 —— **一段都不用凑**。
	// 所以走这条路时既不需要 `phone` 列，也不需要猜前缀。
	//
	// # 为什么它排在下面那两道闸**前面**
	//
	// 那两道闸的前提在整串这条路上**都不成立**：
	//   - **前缀闸**拦的是"我们硬编码 `mobile`、配上一个 PC 发的 token"。整串自带
	//     前缀，不存在配错的可能 —— 发出去的就是登录时拿到的那个字符串。
	//   - **token 闸**拦的是"拼不出凭据"。整串里就带着 token，不依赖 `jwt_token` 列。
	//
	// 把它们留在前面，就会出现"库里明明有完整凭据、却被判成没有"的假阴性 ——
	// 而 v7 的迁移回填恰恰会造出这种"只有整串"的行。
	if phoneOverride == "" && state.Authorization != nil && *state.Authorization != "" {
		// 手机号只用于打印与排障，**不参与拼头**。整串里就带着，顺手解出来 ——
		// 免得"库里 `phone` 是 NULL"这种纯展示问题又变成一条要用户去补数据的报错。
		phone := ""
		if state.Phone != nil {
			phone = string(*state.Phone)
		}
		if phone == "" {
			if info, ok := core.ParseAuth(string(*state.Authorization)); ok && info.Phone != nil {
				phone = *info.Phone
			}
		}
		return SignInCredential{
			phone:         phone,
			authorization: string(*state.Authorization),
			accountID:     accountID,
		}, nil
	}

	// ── 退路：老行（v7 之前）没有整串，用 `phone` + `jwt_token` 现拼 ──
	//
	// 这条路的两个代价都写在下面，**不要再往里加新调用方**：
	//   ① 必须凑齐手机号，缺了就只能让用户在命令行上补（`--phone`）；
	//   ② 前缀由 [upstream.AppAuthorization] **硬编码**成 `mobile`，
	//      它只对 App 端的 token 成立 —— 下面那道 `platform` 闸拦的就是这个。
	//
	// 新登录一律写整串，所以这条路只会随老行自然消亡。

	// 空串与缺失同等对待：空串拼出来的头服务端只会回 90001，
	// 那时错误信息指向"未登录"，而真正的原因是"库里存了个空值"。
	if state.JWTToken == nil || *state.JWTToken == "" {
		return SignInCredential{}, core.NewAuth(
			"账号 " + accountID.String() + " 没有可用的登录凭据（authToken 为空），重新登录一次")
	}

	// 这道闸**只会新增拒绝，不会改变既有行为**：
	// 只有"能证明是另一个端登录的"才拦；`platform` 缺失（老数据）时照旧放行 ——
	// 那正是 `--phone` 这条老路径一直以来的行为，不在这里悄悄收紧。
	//
	// 拦的理由与 `app_client` 不提供 Android 变体同源：**这是我们没有实测过的组合**，
	// 宁可让调用方显式去重登一次，也不要拿一个"PC 发的 token 配 App 的前缀"去试探上游 ——
	// 那种失败长得像"未登录"，会把人带到凭据坏了的错误方向上。
	account, err := store.GetAccount(ctx, scope, accountID)
	if err != nil {
		return SignInCredential{}, asAppError(err, "读账号失败")
	}
	if account != nil && account.Platform != nil && *account.Platform != "" &&
		*account.Platform != upstream.IOSAuthPrefix {
		return SignInCredential{}, core.NewAuth(
			"账号 " + accountID.String() + " 是用 `--client " + *account.Platform +
				"` 登录的，而签到走的是 App 端凭据（`Authorization` 前缀 `" +
				upstream.IOSAuthPrefix + "`）。重新登录一次：`mcloud login sms --client app`")
	}

	phone := phoneOverride
	if phone == "" {
		if state.Phone == nil || *state.Phone == "" {
			return SignInCredential{}, core.NewAuth(
				"账号 " + accountID.String() + " 库里既没有整串 `Authorization`，也没有完整手机号，" +
					"拼不出签到要用的凭据。两条出路：" +
					"重新登录一次 `mcloud login sms --client app`（推荐，会存下整串）；" +
					"或者用 `--phone <完整手机号>` 补一次（补完会记进库里，不必再输）。" +
					"⚠ 不要拿掩码 id 去凑 —— 服务端认不认是未知数。")
		}
		phone = string(*state.Phone)
	}

	return SignInCredential{
		phone:         phone,
		authorization: upstream.AppAuthorization(phone, string(*state.JWTToken)),
		accountID:     accountID,
	}, nil
}

// AccountID 是账号 id（掩码），可直接打印。
func (c SignInCredential) AccountID() core.AccountID { return c.accountID }

// Phone 是完整手机号。**含隐私**，打印前先掩码（[core.AccountLabel]）。
//
// v7 起它不参与拼头，只是"签的是谁"的标识 —— 日志与排障用。
func (c SignInCredential) Phone() string { return c.phone }

// Authorization 是 `Authorization` 头。**含凭据**，只在发请求时用。
func (c SignInCredential) Authorization() string { return c.authorization }

// String 掩码后的人读形式。
//
// ⚠ 这个结构里有一整条可冒充登录的凭据，而 `%v` 是最容易在排障时顺手打出来的
// 东西 —— Rust 侧为此手写了 `Debug`，Go 侧靠实现 [fmt.Stringer]：
// `%v` / `%+v` / `%s` 都会走这里。手机号也一并掩码 —— 它虽然不算凭据，
// 但打日志时没有任何理由用完整的。
func (c SignInCredential) String() string {
	return "SignInCredential{account_id: " + c.accountID.String() +
		", phone: " + core.AccountLabel(nil, c.phone) +
		", authorization: <已隐去>}"
}

// ─────────────────────────────────────────────────────────────────────────────
// 会话
// ─────────────────────────────────────────────────────────────────────────────

// OpenSignInSession 建会话：换 SSO token → 取服务端时间戳 → 签名 → `tyrzLogin`。
//
// 返回 `(referer, jwtToken)`。`referer` 是签到页地址（带本次的 SSO token），
// 后面每个请求都要带同一个 —— 它同时也是"这条会话属于哪个页面"的凭据。
//
// 抽出来是因为 [SignInStatus] 与 [SignIn] 的前三步**逐字相同**；
// 复制一份的话，"签名少了一个头"这种错只会修在其中一条路径上。
func OpenSignInSession(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	clock core.Clock,
	rng core.Random,
) (string, string, *core.Error) {
	// ① 换 SSO token。`x-yun-user-timestamp` 是**微秒**，由 [UserTimestamp] 统一换算。
	ssoToken, perr := up.AppQuerySpecToken(ctx, profile,
		UserTimestamp(clock), credential.Authorization(), upstream.ToSourceID)
	if perr != nil {
		return "", "", perr
	}

	referer := upstream.SigninPageURL(ssoToken)

	// ② 服务端毫秒时间戳。**不能**用本地时钟替代：签名里用的就是它。
	timestamp, perr := up.AppH5Timestamp(ctx)
	if perr != nil {
		return "", "", perr
	}
	sign := upstream.SignH5Request(rng, timestamp)

	// ③ 建会话。SSO token 很短命（换完几十秒内就要用），所以它到这一步之间没有别的 IO。
	login, perr := up.AppH5Login(ctx, sign, referer, ssoToken)
	if perr != nil {
		return "", "", perr
	}
	return referer, login.JWTToken, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 报告
// ─────────────────────────────────────────────────────────────────────────────

// SignInReport 是一次签到调用的结果。
//
// `Before` / `After` 都是**服务端给的**状态，不是从 `startSignIn` 的响应推的 ——
// 见 [SignInReport.SignedNow]。
type SignInReport struct {
	// Before 是签到**前**的状态（`infoV3`）。
	Before upstream.SignInStatus
	// After 是签到**后**的状态。今天本来就签过时**不会**再查一次，两者相同。
	After upstream.SignInStatus
	// Result 是 `startSignIn` 的响应。`nil` = 这次没发（查的时候今天已经签过了）。
	Result *upstream.SignInResult
}

// SignedToday 是今天（调用之后）是否已签。**这是唯一的成败判据。**
func (r SignInReport) SignedToday() bool {
	today, ok := r.After.SignedToday()
	return ok && today
}

// WasAlreadySigned 是来的时候今天**就已经**签过了。
//
// 与 [SignInReport.SignedNow] 是互斥的两件事，而它们在日志里长得很像：
// "已签"既可能是"这次刚签上"，也可能是"本来就已经签过"。
// 定时任务连跑一个月，只有分开报才看得出它到底有没有在干活。
func (r SignInReport) WasAlreadySigned() bool {
	today, ok := r.Before.SignedToday()
	return ok && today
}

// SignedNow 是这次调用**是否真的把今天从「未签」变成了「已签」**。
//
// 注意 `Before` 那一项：`Force`（协议自检）会在已签的情况下也发一次
// `startSignIn`，那次的 `Result` 非 nil、`After` 也是已签，
// 但**不是**"这次签上的"。
func (r SignInReport) SignedNow() bool {
	return r.Result != nil && !r.WasAlreadySigned() && r.SignedToday()
}

// SignInPoints 是本次签到可得云朵数（`startSignIn` 里那个）。没发请求时为 nil。
func (r SignInReport) SignInPoints() *int64 {
	if r.Result == nil {
		return nil
	}
	points := r.Result.SignInPoints
	return &points
}

// MonthDays 是服务端报的本月已签天数。
func (r SignInReport) MonthDays() int64 { return r.After.MonthDays }

// SignInOptions 是 [CloudSignIn] 的两个可选开关。
//
// 做成结构而不是两个裸参数：一个是 `*string`、一个是 `bool`，位置一错就是
// "把 deviceId 当成了 force"，而编译器不会吭声。
type SignInOptions struct {
	// DeviceID 是 H5 那个 `deviceId`（`"B"` + 88 字符 base64 → 64 字节）。
	// 见 [CloudSignIn] 的说明。
	DeviceID *string
	// Force 是今天已签也发一次 `startSignIn`。
	//
	// **唯一的用途是协议自检**：确认我们拼出来的请求被上游接受
	// （而不是回 `401` / `90001`）。服务端对同一天的重复签到是幂等的 ——
	// 实拍里真实客户端在已签状态下也调了它，回 `success` + `todaySignIn:true`。
	//
	// 日常签到**不要**开它：那会让"今天已经签过"这条捷径失效，多打一次写请求。
	Force bool
}

// CloudSignIn 签到（云朵中心）。今天已签过就**不发** `startSignIn`，直接回报当前状态。
//
// ⚠ Rust 侧这一族叫 `sign_in::sign_in`；Go 的**包级命名空间是平的**（没有嵌套模块），
// 而本包已有一个 [SignIn]（口令登录）。两者不是一回事，改名比让它们共存更安全 ——
// 调用点写错一个字的代价是"用口令登录去跑签到"，而那在编译期看不出来。
//
// # `DeviceID`
//
// 真实客户端会带 H5 那个 `deviceId`（`"B"` + 88 字符 base64），
// **它就是 [DeviceID] 换回来的那个值** —— 签到页的 `fp.min.js` 与本项目的
// `upstream::smid` 打的是同一个 `deviceprofile/v4`
// （同 host / organization / appId / 公钥，逐字比对过），所以**纯 Go 就能拿到**。
//
// 但它是 `*string`：实测签到**不带也能签上**（2026-09-19 账号 B），
// 所以调用点可以给 `nil`；给值时是"更像真实客户端"的那一版。
// 别让它变成"忘了传" —— 调用点必须显式写。
//
// ⚠ 但**领取类接口（`receiveV3` 领云朵 / AI 豆）不带就报错**（回 `615`）——
// 所以"签到能不带"**不能**外推到同族的别的接口。
func CloudSignIn(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	options SignInOptions,
	clock core.Clock,
	rng core.Random,
) (SignInReport, *core.Error) {
	referer, jwt, perr := OpenSignInSession(ctx, up, profile, credential, clock, rng)
	if perr != nil {
		return SignInReport{}, perr
	}

	before, perr := up.AppSignInStatus(ctx, jwt, referer)
	if perr != nil {
		return SignInReport{}, perr
	}
	if signed, ok := before.SignedToday(); ok && signed && !options.Force {
		// 已经签过了：**不发** `startSignIn`。少一次写请求，也少一次风控记数。
		return SignInReport{Before: before, After: before}, nil
	}

	result, perr := up.AppStartSignIn(ctx, jwt, referer, options.DeviceID)
	if perr != nil {
		return SignInReport{}, perr
	}
	after, perr := up.AppSignInStatus(ctx, jwt, referer)
	if perr != nil {
		return SignInReport{}, perr
	}

	// ⚠ 这里**必须**回查，不能拿 `result.TodaySignIn` 当结论。
	// 那个字段是调用**前**的快照 —— 实测 `todaySignIn:false` 的那次恰恰把今天签上了
	// （见 `docs/capture/app-signin-samples.txt` §3.2）。反过来，
	// 只看 `code == 0` 也不行：上游对"今天已经签过"同样回 `success`。
	// 所以唯一可信的判据是重新查一次 `infoV3`。
	if signed, ok := after.SignedToday(); !ok || !signed {
		return SignInReport{}, core.NewInternal(
			"startSignIn 回了成功，但回查 infoV3 显示今天仍未签（signed_today 缺失或为 false）。" +
				"以 infoV3 为准；todaySignIn 是调用前的快照，不是成败判据")
	}

	return SignInReport{Before: before, After: after, Result: &result}, nil
}
