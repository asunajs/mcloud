// App 端（iOS 客户端）验证码登录的**驱动**。
//
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-app/src/app_login.rs。
//
// # 与 PC 那条链路（[QRLogin] / [SmsLogin]）刻意分开，因为**形状**不同
//
// |        | PC                | App                                    |
// | ------ | ----------------- | -------------------------------------- |
// | 请求体   | 明文 JSON          | **加密**（AES-ECB + 末 10 位前移）            |
// | `Authorization` 前缀 | `pc` | `mobile`（[upstream.IOSAuthPrefix]） |
// | 滑块    | 第一方取图接口，可自动解   | **同一个第一方取图接口，可自动解**              |
//
// # 发码的滑块：**协议优先，H5 兜底**
//
// 这里曾经写着"App 端没有自动重试环，短信是那个 H5 发的"。**那条结论是错的** ——
// 错在把 **App 客户端的行为**（它选择开 WebView）当成了**协议的限制**。
//
// 实测（2026-09-19，跑通）：
//
//	getSmsCode(空) → 200059554
//	  → POST /user/verfycode 取第一方题图（**与 getSmsCode 同一台主机**）
//	  → puzzleLeft 用公开常量解开 = 拖动坐标
//	  → getSmsCode(带坐标) → 0000，短信已发
//
// 所以 App 端**有**和 PC 一样可自动完成的滑块，而且**编码完全一样**
// （复用 [upstream.PuzzleAESKey]）。H5 地址（[upstream.CaptchaH5URL]）退化为兜底：
// **只在滑块重试用尽时**才交给用户。
//
// ⚠ 那个接口**计失败次数**（`200059553` = 该挑战作废）。所以重试有上限，
// 且每次都**重取新题**——拿旧答案再试只会再失败一次。
//
// # 追加澄清：上面那个环**不是**"App 自己的滑块被自动化了"
//
// 它复用的是 **PC 端那套**取题与编码，只是 `clientType` 与请求头换成 App 的 ——
// 属**围魏救赵**。所以"App 端滑块已实现自动化"这句**不成立**：
// App 自己那条路（原生开 WebView、短信由 H5 发）并没有被自动化，真机走的仍是它。
//
// 也就是说，上面被推翻的只是"**协议**不允许 App 走自动这条路"这一层推断，
// 而"真机在开 H5"那个观察从来没错过 —— 两层别混，改这段的人（包括 AI）尤其注意。
package app

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// AppSmsMaxAttempts 是滑块这条路最多试几次。
//
// 上限存在的意义是**别把账号试进限流**：那个接口计失败次数
// （`200059553` = 该挑战作废）。与 PC 端同一个理由，只是 App 端
// 每次重试都要多打一次 `/user/verfycode`，代价更高。
const AppSmsMaxAttempts = 3

// `x-yun-user-timestamp` 的取值（**微秒**，实测 16 位 `1789743328974257`）
// 用本包已有的 [UserTimestamp]（落在 signin.go —— 签到那一族先迁到，
// 它是这里目前唯一的另一批消费者）。
//
// ⚠ **不要在本文件再写一份**：时间戳写错一个量级的表现是上游回 401，
// 而 401 在日志里和"盐抄错了"长得一模一样 —— 两份实现迟早会在某一次
// "顺手改成毫秒"里分叉，而分叉的那天没人看得出来。

// ─────────────────────────────────────────────────────────────────────────────
// 发码
// ─────────────────────────────────────────────────────────────────────────────

// AppCodeKind 是 App 端发验证码的结果类别。
type AppCodeKind int

const (
	// AppCodeSent 是上游受理，短信已下发。
	AppCodeSent AppCodeKind = iota
	// AppCodeNeedCaptcha 是协议路径走不通了，**把 H5 地址交给用户**。
	//
	// **这不是常态** —— 只有滑块重试用尽（[AppSmsMaxAttempts]）才会到这里。
	// 打开并完成拖动之后，短信会由那个页面发出，此时**直接调 [AppSmsLogin]**，
	// **不要**再回来重试发码（实测客户端就是这么走的：
	// H5 回调 `{code:200}` 之后直接 `thirdlogin`，中间没有第二次 `getSmsCode`）。
	AppCodeNeedCaptcha
)

func (k AppCodeKind) String() string {
	switch k {
	case AppCodeSent:
		return "sent"
	case AppCodeNeedCaptcha:
		return "need_captcha"
	}
	return ""
}

// AppCodeOutcome 是 App 端发验证码的结果。
//
// Go 没有 sum type，用"类别 + 字段"近似 —— 与 [upstream.AppLoginOutcome]
// 同一套写法。⚠ **读字段前先看 [AppCodeOutcome.Kind]**：
// `URL` 只在 `NeedCaptcha` 下有意义，`RemainGetTimes` 只在 `Sent` 下有意义；
// 不看类别就取值会得到零值，而零值在这里是一个**看起来正常**的答案。
type AppCodeOutcome struct {
	Kind AppCodeKind
	// RemainGetTimes 在 [AppCodeSent] 下有效：配额余量。
	RemainGetTimes *int64
	// SolvedSlider 在 [AppCodeSent] 下有效：是不是**自动过了一次滑块**才拿到的
	// （给用户一句提示用）。
	SolvedSlider bool
	// URL 在 [AppCodeNeedCaptcha] 下有效：交给用户的 H5 地址。
	URL string
	// CaptchaType 在 [AppCodeNeedCaptcha] 下有效：上游最后一次给的 `captchaType`。
	CaptchaType *int64
	// Attempts 在 [AppCodeNeedCaptcha] 下有效：已经自动试过几次。
	Attempts int
	// LastAnswer 在 [AppCodeNeedCaptcha] 下有效：最后发上去的坐标。
	//
	// **留着是为了诊断**：万一服务端要的答案口径与自动解出的不同，这一行是唯一的证据。
	LastAnswer string
}

// AppSendSmsCode 发短信验证码（App 端）：**先自己过滑块，过不去才把 H5 交给用户**。
//
// 每次调用都会重新构造请求体并**明文**发送 ——
// `getSmsCode` 是这条链路里唯一不加密的请求。
func AppSendSmsCode(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	phone string,
	clock core.Clock,
) (AppCodeOutcome, *core.Error) {
	solvedSlider := false
	lastAnswer := ""
	var lastCaptchaType *int64

	for attempt := 1; attempt <= AppSmsMaxAttempts; attempt++ {
		var body any
		if solvedSlider {
			// 每次都**重取新题**：`200059553` 的语义是"这个挑战作废了"，
			// 复用上一轮的答案只会再失败一次，而且白记一次失败次数。
			puzzle, perr := up.AppFetchSliderPuzzle(
				ctx, profile, UserTimestamp(clock), phone)
			if perr != nil {
				return AppCodeOutcome{}, perr
			}
			answer, perr := puzzle.Answer()
			if perr != nil {
				return AppCodeOutcome{}, perr
			}
			lastAnswer = answer
			sliderBody, perr := upstream.NewAppSendSmsCodeBodyWithSlider(phone, answer)
			if perr != nil {
				return AppCodeOutcome{}, perr
			}
			body = sliderBody
		} else {
			// 首次：`puzzleVerfycode` 照抄实测的**空串**（不是 `null`、也不是省略字段）。
			body = upstream.NewAppSendSmsCodeBody(phone, "")
		}

		envelope, perr := up.AppSendSmsCode(ctx, profile, UserTimestamp(clock), body)
		if perr != nil {
			return AppCodeOutcome{}, perr
		}

		switch outcome := upstream.ClassifyAppSendSms(envelope); outcome.Kind {
		case upstream.AppSmsSent:
			return AppCodeOutcome{
				Kind:           AppCodeSent,
				RemainGetTimes: outcome.RemainGetTimes,
				SolvedSlider:   solvedSlider,
			}, nil

		// 需要滑块、或旧挑战已作废：都要**重取新题**
		case upstream.AppSmsNeedCaptcha, upstream.AppSmsSliderExhausted:
			if outcome.CaptchaType != nil {
				lastCaptchaType = outcome.CaptchaType
			}
			if attempt == AppSmsMaxAttempts {
				// break 出循环，落到下面的 H5 兜底
				return h5Fallback(profile, phone, lastCaptchaType, lastAnswer), nil
			}
			solvedSlider = true

		case upstream.AppSmsOther:
			// 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
			if outcome.Code != nil {
				return AppCodeOutcome{}, core.NewBadRequest(fmt.Sprintf(
					"发送验证码失败 code=%d: %s", *outcome.Code, outcome.Message))
			}
			return AppCodeOutcome{}, core.NewBadRequest(
				fmt.Sprintf("发送验证码失败: %s", outcome.Message))
		}
	}

	// 结构上到不了：最后一次尝试在上面已经返回兜底。
	// 留着是为了"哪天有人把上限改成 0"时不至于掉进这里却什么都不返回。
	return h5Fallback(profile, phone, lastCaptchaType, lastAnswer), nil
}

// h5Fallback 是协议路径用尽后的兜底：**把 H5 地址交给用户**。
//
// 这是**兜底**，不是首选 —— 首选是上面那个自动解滑块的环。
func h5Fallback(
	profile *upstream.AppProfile, phone string, captchaType *int64, lastAnswer string,
) AppCodeOutcome {
	return AppCodeOutcome{
		Kind:        AppCodeNeedCaptcha,
		URL:         upstream.CaptchaH5URL(profile, phone),
		CaptchaType: captchaType,
		Attempts:    AppSmsMaxAttempts,
		LastAnswer:  lastAnswer,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录
// ─────────────────────────────────────────────────────────────────────────────

// AppLoginContext 是一次 App 登录里不变的那部分：
// **以谁的身份、冒充哪台设备、用哪张手机号的哪个码**。
//
// 单独成结构而不是三个参数 —— 与 [LoginContext] 同一个理由：
// 这三样凑齐才是"一次登录"，分开传容易在调用点被摆错顺序，
// 而它们都是字符串/指针，摆错了编译器不会吭声。
type AppLoginContext struct {
	// Scope 是凭据归属谁（WebUI 用当前会话用户，CLI 用 [core.ScopeLocal]）。
	Scope core.Scope
	// Profile 是冒充哪台设备。`deviceId` 是**每安装**指纹，必须持久化且永不漂移。
	Profile *upstream.AppProfile
	// Credential 是手机号 + 收到的验证码。
	Credential SmsCredential
}

// AppSmsLogin 用短信验证码登录（App 端）并落库。
//
// 与 PC 的 [SmsLogin] 共用同一条落库路径
// （[capabilities.PersistLoginWithPrefix]），差别只在 `Authorization`
// 载荷的第一段是 `mobile`。
//
// # 为什么加密在这里、不在调用点
//
// 请求体由 [upstream.Upstream.AppThirdLogin] 内部加密（它收
// [upstream.AppLoginCrypto]），所以调用点**没有机会**把明文发上去 ——
// 那会得到 `9103`，而 `9103` 的三种成因（没加密 / 用错密钥 / 错位方向反了）
// 在日志里长得一模一样。
func AppSmsLogin(
	ctx context.Context,
	store core.StateStore,
	up *upstream.Upstream,
	loginContext AppLoginContext,
	crypto *upstream.AppLoginCrypto,
	clock core.Clock,
	rng core.Random,
) (LoginOutcome, *core.Error) {
	random := upstream.NewLoginRandom(rng)
	body := upstream.NewAppSmsThirdLoginBody(
		loginContext.Credential.Phone, loginContext.Credential.Code, random)

	envelope, perr := up.AppThirdLogin(
		ctx, loginContext.Profile, UserTimestamp(clock), crypto, body)
	if perr != nil {
		return LoginOutcome{}, perr
	}

	encrypted, perr := classifyAppLogin(envelope)
	if perr != nil {
		return LoginOutcome{}, perr
	}

	userInfo, perr := crypto.DecryptUserInfo(encrypted)
	if perr != nil {
		return LoginOutcome{}, perr
	}

	accountID, perr := capabilities.PersistLoginWithPrefix(
		ctx, store, loginContext.Scope, loginContext.Profile.Device().DeviceID(),
		userInfo, upstream.IOSAuthPrefix, clock.Now())
	if perr != nil {
		return LoginOutcome{}, perr
	}
	return LoginOutcome{AccountID: accountID, UserInfo: userInfo}, nil
}

// classifyAppLogin 把上游的登录判定翻成"成功则带回密文、失败则带回错误"。
func classifyAppLogin(envelope *upstream.Envelope) (string, *core.Error) {
	outcome := upstream.ClassifyAppLogin(envelope)
	switch outcome.Kind {
	case upstream.AppLoginSuccess:
		return outcome.Encrypted, nil

	case upstream.AppLoginSmsCodeWrong:
		// `9441` 是**好**消息：它证明加密这一层全对了
		// （密钥 / 模式 / 错位方向 / hex 大小写）。
		return "", core.NewBadRequest(
			"验证码不正确或已过期（9441）。加密这一层是通的，重新取一次验证码即可")

	case upstream.AppLoginUndecryptable:
		// `9103` 是**我们**的问题，不是用户的问题 —— 归到 Internal 而不是
		// BadRequest，免得前端把它渲染成"请检查你输入的参数"。
		return "", core.NewInternal(
			"上游解不开请求体（9103）：本端的加密参数与实测不符，" +
				"按 密钥 → 模式 → 错位方向 的顺序排查")

	case upstream.AppLoginFailed:
		return "", core.NewBadRequest(
			"上游拒绝登录（9999 登录失败！）。App 端出现这个码通常是设备头缺失或不完整")
	}

	// 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
	if outcome.Code != nil {
		return "", core.NewBadRequest(fmt.Sprintf(
			"登录失败 code=%d: %s", *outcome.Code, outcome.Message))
	}
	return "", core.NewBadRequest(fmt.Sprintf("登录失败: %s", outcome.Message))
}

// ─────────────────────────────────────────────────────────────────────────────
// 已登录态的档案回填
// ─────────────────────────────────────────────────────────────────────────────

// ProfileWithStoredDomainID 把库里已存的 `user_domain_id` 回填进 App 档案
// （也就是 `x-yun-uni` 头）。
//
// # 为什么读不到就算正常、而不是报错
//
// 这个值是 `thirdlogin` **响应**里的 `userDomainId`，首次登录时本地确实没有 ——
// 而上游对它的态度是"填错就把你定位到别人的用户域"，所以缺失时原样返回档案
// （[upstream.AppProfile] 因此把它做成可选），绝不编一个。
// 登录一次之后它就在 `account_state` 里了，第二次起这个函数会带上它 ——
// 与真机的行为一致（真机也不会为每次登录重新握手）。
//
// # 为什么 `scope` 由调用方给
//
// 传入的范围应当与**这一次登录写库时用的同一个**。不一致会出现
// "读到了某个账号的用户域、却把它发给另一个账号"，那比读不到更糟。
//
// # 为什么收 [core.AccountID] 而不是手机号
//
// **调用方不总是有手机号**：`mcloud signin --account <掩码id>` 那条路径手上只有 id
// （完整号码在登录态里，而它可能为空）。让"从手机号推 id"这件事只发生在真正有
// 手机号的那一处，比在这里再推一次更不容易跑偏。
//
// ⚠ **返回的档案是新对象**：[upstream.AppProfile.WithUserDomainID] 改的是它的 receiver
// （Go 是指针语义，Rust 那版是值语义 + 返回新值），直接改会把调用方手上那份
// 也改掉 —— "查一下用户域"这样的只读动作不该有副作用（§6.1 #73）。
func ProfileWithStoredDomainID(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	profile *upstream.AppProfile,
	accountID core.AccountID,
) (*upstream.AppProfile, *core.Error) {
	state, err := store.GetAccountState(ctx, scope, accountID)
	if err != nil {
		return nil, asAppError(err, "读登录态失败")
	}
	if state == nil || state.UserDomainID == nil {
		return profile, nil
	}
	domain := string(*state.UserDomainID)
	// 空串**不等于有值**：库里存空串是"上游没给 / 旧数据"，
	// 带上它会让上游把这次请求定位到一个不存在的用户域。
	if domain == "" {
		return profile, nil
	}
	clone := *profile
	return clone.WithUserDomainID(domain), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 会话到期
// ─────────────────────────────────────────────────────────────────────────────

// appSessionExpiryDays 是 App 端登录态的兜底天数。
//
// 与 PC 同款：本机单机场景"可长期有效"，但仍给一个明确到期时间 ——
// 无到期的会话在库里无法清理。真正该用多久由上游的 `atExpiretime` 决定，
// 这里只是落库时的兜底值。
//
// 到期时刻的算法与 PC **共用** [SessionExpiry]，不在这里再写一份。
const appSessionExpiryDays = 30

// AppSessionExpiry 返回 App 端登录态的到期时刻。
func AppSessionExpiry(now time.Time) time.Time {
	return SessionExpiry(now, appSessionExpiryDays)
}
