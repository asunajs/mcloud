// 登录的**驱动**：把能力层的状态机接到真实的等待与渲染上。
//
// 对应 Rust 侧 legacy/crates/mcloud-app/src/login.rs。
//
// # 为什么等待被做成注入项（[Waiter]），而不是直接 `time.Sleep`
//
// Rust 侧是靠 tokio 的 `start_paused` 虚拟化定时器来测这条循环 ——
// 那是**运行时**提供的能力，Go 的 `time` 包没有对应物：
// `time.Sleep` 就是真的等，于是"限频退避 30 秒"这条语义要么测不了，
// 要么让测试真等 30 秒（方案 §6.1 #72）。
// 把"等"收敛成一个接口后：断言的是"退避了多久、退避了几次"，
// 而不是"等完了没"。
//
// # 什么仍然不在这里
//
// 轮询节奏（间隔与 TTL）依然由能力层的 [capabilities.QrLogin.NextDelayMS] 回答，
// 本层只负责"照它说的等"。**两条退出路径**都保留：
// 终态（能力层判的）与"下次别再问了"（`NextDelayMS` 返回 0）——
// 后者存在的理由是"某天状态机多加了一种停止条件"时不至于变成死循环。
//
// # 凭据边界
//
// [LoginOutcome.UserInfo] 里带着 `authToken`：它**不打印、不写日志**，
// 只用于落库与即时展示摘要（[LoginOutcome.Summary]）。
package app

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 等待
// ─────────────────────────────────────────────────────────────────────────────

// Waiter 是"等待"这一动作的注入点。
//
// 登录是本项目里唯一需要**真的等几秒到几十秒**的业务流程
// （轮询间隔 1 秒、限频退避 30 秒）。把等收敛成接口后，
// 这些时长的语义可以被断言，而不必让测试真的睡过去。
type Waiter interface {
	// Wait 等一段时间。实现**必须**响应 ctx 取消 ——
	// 一次扫码最长要等 120 秒，用户 Ctrl-C 了却还在等是没法接受的。
	Wait(ctx context.Context, d time.Duration) *core.Error
}

// RealWaiter 是生产实现：真的睡，且可被 ctx 打断。
type RealWaiter struct{}

// Wait 实现 [Waiter]。
//
// 用 `time.After` 而不是裸 `time.Sleep`，就是为了能同时等 ctx.Done()。
func (RealWaiter) Wait(ctx context.Context, d time.Duration) *core.Error {
	if d <= 0 {
		return nil
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return core.NewTimeout("等待被取消")
	case <-timer.C:
		return nil
	}
}

// NoWait 是"不等"的实现：用于测试与非交互场景。
//
// 它**不是**空实现占位 —— 空实现会让"该退避却没退避"这类回归
// 在测试里完全不可见（反正没人等）。用法是配 [RecordingWaiter] 断言时长。
type NoWait struct{}

// Wait 实现 [Waiter]：立即返回。
func (NoWait) Wait(_ context.Context, _ time.Duration) *core.Error { return nil }

// ─────────────────────────────────────────────────────────────────────────────
// 结果
// ─────────────────────────────────────────────────────────────────────────────

// LoginOutcome 是登录成功的结果。
type LoginOutcome struct {
	AccountID core.AccountID
	// UserInfo 是已解密的 `userInfo`。**含凭据**，只用于落库与展示摘要。
	UserInfo any
}

// Summary 返回展示用摘要：只给手机号掩码这类可以直接印在终端上的信息。
//
// 账号字段缺失时退回账号 id —— 那也是掩码（[core.NewAccountID] 由掩码得来），
// 所以这条退路不会把完整手机号带出来。
func (o LoginOutcome) Summary() string {
	account := ""
	if obj, ok := o.UserInfo.(map[string]any); ok {
		if text, isStr := obj["account"].(string); isStr {
			account = text
		}
	}
	if account == "" {
		return o.AccountID.String()
	}
	return core.MaskPhone(account)
}

// LoginObserver 是登录过程的观察者：二维码只给一次，进展每次轮询后给一次。
//
// 做成接口而不是两个闭包参数：CLI 要打印到终端、WebUI 要推给浏览器，
// 两者的状态（比如"已经推过了"）各自持有，接口只需要这两个动作。
type LoginObserver interface {
	// OnQR 拿到二维码载荷（**只调一次**，在开始轮询之前）。
	OnQR(payload string)
	// OnProgress 每次轮询后的进展（含终态）。
	OnProgress(progress capabilities.QrProgress)
}

// SilentObserver 是什么都不做的观察者（非交互场景与测试用）。
type SilentObserver struct{}

// OnQR 实现 [LoginObserver]。
func (SilentObserver) OnQR(_ string) {}

// OnProgress 实现 [LoginObserver]。
func (SilentObserver) OnProgress(_ capabilities.QrProgress) {}

// ─────────────────────────────────────────────────────────────────────────────
// 上下文
// ─────────────────────────────────────────────────────────────────────────────

// LoginContext 是一次登录里不变的那部分：**以谁的身份、冒充哪个端、设备指纹是什么**。
//
// 单独成结构而不是三个参数：这三样凑齐才是"一次登录"（凭据要落到谁名下、
// 上游看到的是哪一端、服务端靠哪个 dID 定位会话），
// 分开传容易在调用点被摆错顺序 —— 而它们都是字符串/枚举，摆错了编译器不会吭声。
type LoginContext struct {
	// Scope 是凭据归属谁。
	//
	// WebUI 用 [core.ScopeUser]（当前会话用户）；CLI 用 [core.ScopeLocal]
	// （本地运维不替谁认领，账号保持无主、等首个管理员初始化时认领）。
	Scope core.Scope
	// Platform 是冒充哪个客户端（影响 `clientType` / `cpid` / 版本号）。
	Platform upstream.PcPlatform
	// DeviceID 是设备指纹（`windowMachine`）—— **必须持久化且永不漂移**。
	DeviceID string
	// ttlSeconds 是二维码等待窗口（秒）——**只有扫码流程读它**，
	// 所以字段私有，用 [LoginContext.WithTTL] 改。短信流程写
	// [NewLoginContext] 即可，不会被这个只属于另一条链路的旋钮打扰。
	ttlSeconds int64
}

// NewLoginContext 构造常规上下文（二维码有效期取默认
// [capabilities.QRTTLSeconds] 秒）。
//
// ⚠ 默认值**必须**来自能力层的常量，不能在这里再写一个 120：
// "两处各写一份、后来只改了一处"在 Go 里不报错，只会让 CLI 显示的
// 有效期与状态机实际用的有效期悄悄分叉（同 §6.1 #67 那条教训）。
func NewLoginContext(scope core.Scope, platform upstream.PcPlatform, deviceID string) LoginContext {
	return LoginContext{
		Scope:      scope,
		Platform:   platform,
		DeviceID:   deviceID,
		ttlSeconds: capabilities.QRTTLSeconds,
	}
}

// WithTTL 放宽二维码有效期。用于"把码发给别人扫"（终端打印、远程协助）。
func (c LoginContext) WithTTL(seconds int64) LoginContext {
	c.ttlSeconds = seconds
	return c
}

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// TooFrequentBackoff 是被限频（`200059545 扫码登录过于频繁`）后的退避时长。
//
// 30 秒是**拍的经验值**：太短会继续撞风控，太长则用户以为程序卡住了。
// 真正的上限由 TTL 兜着（TTL 到了就结束，所以不会无限等）。
const TooFrequentBackoff = 30 * time.Second

// QRLogin 是扫码登录的完整驱动：出码 → 轮询 → 成功后落库。
//
// 退出条件全部交给状态机（[capabilities.QrLogin.NextDelayMS]）：
// 成功、失败、超时三种情况都返回 0，所以这个循环**不可能**出现
// "该停没停"（客户端源码专门为它写过 `clearInterval` 的提醒）。
func QRLogin(
	ctx context.Context,
	store core.StateStore,
	up *upstream.Upstream,
	loginContext LoginContext,
	decryptor upstream.LoginPayloadDecryptor,
	clock core.Clock,
	rng core.Random,
	observer LoginObserver,
	wait Waiter,
) (LoginOutcome, *core.Error) {
	qr := capabilities.StartQrLogin(
		loginContext.Platform, loginContext.DeviceID, rng, clock.Now()).WithTTL(loginContext.ttlSeconds)
	observer.OnQR(qr.QrPayload())

	for {
		// 每个请求一个新的 `x-yun-tid`（实测要求）
		tid := core.UUIDv4(rng)
		progress, perr := qr.Poll(ctx, up, tid, decryptor, clock.Now())
		if perr != nil {
			return LoginOutcome{}, perr
		}
		observer.OnProgress(progress)

		switch progress.Kind {
		case capabilities.QrLoggedIn:
			accountID, perr := capabilities.PersistLogin(
				ctx, store, loginContext.Scope, loginContext.DeviceID, progress.UserInfo, clock.Now())
			if perr != nil {
				return LoginOutcome{}, perr
			}
			return LoginOutcome{AccountID: accountID, UserInfo: progress.UserInfo}, nil

		case capabilities.QrNotScanned, capabilities.QrScanned:

		case capabilities.QrFailed:
			// 超时单独给 Timeout：调用方（脚本 / 监控）据此区分"等太久了"
			// 与"被上游拒了"，混进 BadRequest 会让两者不可区分。
			if progress.Reason.Kind == capabilities.FailureTTLElapsed {
				return LoginOutcome{}, core.NewTimeout(qrTimeoutMessage)
			}
			// 限流：**退避后继续**，不是终态。
			//
			// 会话还有效，服务端只是要求慢一点；把用户赶回去重开一张码，
			// 只会让他更深地撞进风控（实测连着跑几次就是这个下场）。
			// 总时长仍受 TTL 约束，所以不会无限等下去。
			if progress.Reason.Kind == capabilities.FailureTooFrequent {
				if waitErr := wait.Wait(ctx, TooFrequentBackoff); waitErr != nil {
					return LoginOutcome{}, waitErr
				}
				continue
			}
			return LoginOutcome{}, core.NewBadRequest(fmt.Sprintf(
				"扫码登录未成功：%s（%s）。重新执行即可换一张新码",
				progress.Reason.Error(), progress.Reason.Kind))
		}

		// 兜底：状态机的 0 是"别再问了"的唯一裁判（终态与超时都在上面处理过）。
		// 留着这一支是为了让"某天状态机多加了一种停止条件"时不会变成死循环。
		delayMS := qr.NextDelayMS(clock.Now())
		if delayMS == 0 {
			return LoginOutcome{}, core.NewTimeout(qrTimeoutMessage)
		}
		if waitErr := wait.Wait(ctx, time.Duration(delayMS)*time.Millisecond); waitErr != nil {
			return LoginOutcome{}, waitErr
		}
	}
}

// qrTimeoutMessage 是超时文案。
//
// 两处退出（TTL 到了、`NextDelayMS` 返回 0）共用同一句：它们对调用方是
// **同一件事**（本轮不再等），分别写会让文案在某一处悄悄变掉。
const qrTimeoutMessage = "二维码已超时（本轮不再等待）。重新执行即可换一张新码"

// ─────────────────────────────────────────────────────────────────────────────
// 短信
// ─────────────────────────────────────────────────────────────────────────────

// SmsMaxAttempts 是发短信验证码的最大尝试次数。
//
// 滑块挑战有失败上限（实测 `200059553` 之后挑战作废），无限重试只会在风控那里
// 越陷越深。3 次是**经验值**：够覆盖"一次判废后重取图"，又不至于把配额刷掉。
const SmsMaxAttempts = 3

// SmsCodeSent 是发短信验证码的结果。
type SmsCodeSent struct {
	// RemainGetTimes 是配额余量（上游给的 `remainGetTimes`）。
	RemainGetTimes *uint32
	// SolvedSlider 报告是否经过了一次滑块校验（用于给用户一句"过了一次风控"的提示）。
	SolvedSlider bool
}

// SmsSendCode 发短信验证码；被风控要求滑块时**自动解出答案并重试**。
//
// # 为什么这段循环在共享层
//
// "被要求滑块 → 取图 → 解坐标 → 带答案重发 → 挑战作废则重取图"是**协议语义**，
// 不是某个前端的交互细节。放这里，CLI 与 WebUI 才共用同一份，
// 而不是各自演化出"一个会重取图、另一个不会"的差异。
//
// 每次尝试用新的 `x-yun-tid` 与新的 `random`（实测客户端就是这么做的）。
func SmsSendCode(
	ctx context.Context,
	up *upstream.Upstream,
	platform upstream.PcPlatform,
	deviceID, phone string,
	rng core.Random,
) (SmsCodeSent, *core.Error) {
	sms := capabilities.StartSmsLogin(platform, deviceID, phone, rng)
	solvedSlider := false
	// 记下最后发上去的坐标：滑块这条路目前是**强推断**
	// （见 [upstream.SliderPuzzle.Answer]），万一推断错了，失败信息里能直接看到
	// "我们发了什么"，比一句"滑块未通过"有用得多。
	lastAnswer := ""

	for attempt := 1; attempt <= SmsMaxAttempts; attempt++ {
		tid := core.UUIDv4(rng)

		var outcome upstream.SendSmsOutcome
		if solvedSlider {
			// 每次重新取图再解：`200059553` 的语义是**这个挑战作废了**，
			// 复用上一轮的答案只会再失败一次。
			answer, perr := sms.SolveSlider(ctx, up, core.UUIDv4(rng))
			if perr != nil {
				return SmsCodeSent{}, perr
			}
			lastAnswer = answer
			outcome, perr = sms.SendCodeWithSlider(ctx, up, tid, answer)
			if perr != nil {
				return SmsCodeSent{}, perr
			}
		} else {
			var perr *core.Error
			outcome, perr = sms.SendCode(ctx, up, tid)
			if perr != nil {
				return SmsCodeSent{}, perr
			}
		}

		switch outcome.Kind {
		case upstream.SmsSent:
			return SmsCodeSent{
				RemainGetTimes: outcome.RemainGetTimes,
				SolvedSlider:   solvedSlider,
			}, nil

		// 需要滑块、或旧挑战已作废：都要**重新取图**
		case upstream.SmsNeedSlider, upstream.SmsSliderExhausted:
			if attempt == SmsMaxAttempts {
				return SmsCodeSent{}, core.NewRateLimited(fmt.Sprintf(
					"连续 %d 次滑块未通过，已放弃（最后发上去的坐标：%q）。"+
						"若坐标明显不对，说明服务端要的答案口径与自动解出的不同 —— "+
						"请把这一行留证。稍后再试，或改用扫码登录",
					SmsMaxAttempts, lastAnswer))
			}
			solvedSlider = true
			sms.Reroll(rng)

		case upstream.SmsOther:
			// 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
			if outcome.Code != nil {
				return SmsCodeSent{}, core.NewBadRequest(fmt.Sprintf(
					"发送验证码失败 code=%d: %s", *outcome.Code, outcome.Message))
			}
			return SmsCodeSent{}, core.NewBadRequest(
				fmt.Sprintf("发送验证码失败: %s", outcome.Message))
		}
	}

	// 结构上到不了：上面每个分支都返回。
	// 这里用一句防御性错误而不是 panic：免得哪天有人把
	// [SmsMaxAttempts] 改成 0，把一个配置笔误变成一次崩溃。
	return SmsCodeSent{}, core.NewInternal("发码流程没有产出结果")
}

// SmsCredential 是短信登录的凭据：**手机号 + 收到的验证码**。
//
// 合成一个类型，是因为它们总是一起出现、一起使用；而两个并排的 `string`
// 参数最容易在调用点被写反 —— 写反了编译器不会吭声。
type SmsCredential struct {
	Phone string
	// Code 是 6 位明文验证码（短信方式下 `dycpwd` 就是明文，不是哈希）。
	Code string
}

// SmsLogin 用短信验证码登录并落库。
//
// 与 [QRLogin] 共用同一套落库路径（[capabilities.PersistLogin]）：
// 两者最终都产出同一个 `userInfo`，差别只在拿到它的方式。
//
// ⚠ 它**只负责用验证码换凭据**：发码在 [SmsSendCode]，
// 因为发码要等用户输验证码，中间隔着一个人在。
func SmsLogin(
	ctx context.Context,
	store core.StateStore,
	up *upstream.Upstream,
	loginContext LoginContext,
	credential SmsCredential,
	decryptor upstream.LoginPayloadDecryptor,
	clock core.Clock,
	rng core.Random,
) (LoginOutcome, *core.Error) {
	tid := core.UUIDv4(rng)
	sms := capabilities.StartSmsLogin(
		loginContext.Platform, loginContext.DeviceID, credential.Phone, rng)
	userInfo, perr := sms.Login(ctx, up, tid, credential.Code, decryptor)
	if perr != nil {
		return LoginOutcome{}, perr
	}

	accountID, perr := capabilities.PersistLogin(
		ctx, store, loginContext.Scope, loginContext.DeviceID, userInfo, clock.Now())
	if perr != nil {
		return LoginOutcome{}, perr
	}
	return LoginOutcome{AccountID: accountID, UserInfo: userInfo}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 会话到期
// ─────────────────────────────────────────────────────────────────────────────

// SessionExpiry 返回会话过期时刻。
//
// CLI 的登录态"可长期有效"（本机单机），但仍然给一个明确的到期时间 ——
// 无到期的会话在库里无法清理，也无法回答"这个登录态还能用多久"。
func SessionExpiry(now time.Time, days int) time.Time {
	return now.AddDate(0, 0, days)
}
