// 移动云盘登录：扫码与短信两条链路的**状态机**。
//
// 对应 Rust 侧的 legacy/crates/mcloud-capabilities/src/login.rs。
//
// # 为什么状态机在这里，而不是在 upstream::login
//
// 上游那层只做两件事：**请求怎么拼**（逐字段对齐实测）与**响应怎么读**（业务码 → 语义）。
// 本层负责"接下来怎么办"：什么时候再问一次、什么时候必须停、拿到 `userInfo` 之后落成什么。
//
// 这么切分是为了让 CLI 与 WebUI **共用同一份状态机**（方案 P2.5 的验收点）：
// 两个入口的差别只是"谁来等这 1000 毫秒"和"谁来渲染二维码"，
// 而不是各写一套轮询循环 —— 客户端源码里那句"登录成功后必须 `clearInterval`"
// 说明这套循环很容易写错一次（停了不该停的、或者忘了停）。
//
// # 两个刻意的设计
//
//   - **本层不 sleep**：`NextDelay` 只回答"下次隔多久、要不要停"，
//     等多久交给调用方。于是"轮询节奏"能被单测直接断言，
//     "每次等 1 秒"不需要靠真的等 1 秒来验证。
//   - **终态不可逆**：一旦出结果（成功或失败），后续 `Poll` 直接返回同一结论且**不发请求**。
//     `clearInterval` 那条教训因此从"记得写"变成"写不错"。
package capabilities

import (
	"context"
	"encoding/base64"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// QRPollInterval 是扫码轮询间隔。
//
// 实测客户端是 `setInterval` 1000ms（见 docs/upstream-login-protocol.md §3）。
// ⚠ 用 [time.Duration] 而不是"毫秒数"：Go 里一段时间就是 `time.Duration`，
// 传裸数字（尤其还带着 `MS` 后缀）等于把单位塞进名字里 —— 哪天有人按秒传，
// 编译器不会说话，只有轮询会忽然快 1000 倍。
const QRPollInterval = time.Second

// QRTTL 是二维码在我们这一侧的有效期。
//
// # 这是一个**产品取值**，不是实测值
//
// 归档里 `200059542 二维码已失效` 只出现过 1 次，推不出时长。这里取 120 秒：
// 宁可早一点失效让用户重新出码（代价是一次点击），
// 也不要让用户对着一个已经无效的码干等（代价是"这软件坏了"）。
const QRTTL = 120 * time.Second

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// QrProgressKind 是扫码登录的推进结果类别。
type QrProgressKind int

const (
	// QrNotScanned 是码还没被扫 —— 轮询期的绝大多数情况（实测 121/192），不是错误。
	QrNotScanned QrProgressKind = iota
	// QrScanned 是已扫码、等手机上确认 —— 唯一值得对外显示进度的中间态。
	QrScanned
	// QrLoggedIn 是成功，且 `user_info` 已解密。
	QrLoggedIn
	// QrFailed 是终态失败（但**不一定**不可逆，见 [QrFailure.TTL] 之外那条）。
	QrFailed
)

func (k QrProgressKind) String() string {
	switch k {
	case QrNotScanned:
		return "not_scanned"
	case QrScanned:
		return "scanned"
	case QrLoggedIn:
		return "logged_in"
	case QrFailed:
		return "failed"
	}
	return ""
}

// QrFailureKind 是扫码失败的几种终态。
type QrFailureKind int

const (
	// FailureTTLElapsed 是我们自己的 TTL 到了（码可能还有效，但不再等）。
	FailureTTLElapsed QrFailureKind = iota
	// FailureExpiredUpstream 是上游说码已失效（`200059542`）。
	FailureExpiredUpstream
	// FailureSessionMismatch 是 `sID` 不匹配（`200059544`）：
	// 换码之后还在用旧 `sID` 轮询，属于调用方的状态错误。
	FailureSessionMismatch
	// FailureTooFrequent 是触发限流（`200059545 扫码登录过于频繁`）——
	// **该退避**，不是换张码立刻重试。
	FailureTooFrequent
	// FailureRejected 是上游回了别的码。**原样带回**：上游会在不通知的情况下加新码，
	// 硬编码成"未知错误"会让新码永远查不出来。
	FailureRejected
)

func (k QrFailureKind) String() string {
	switch k {
	case FailureTTLElapsed:
		return "ttl_elapsed"
	case FailureExpiredUpstream:
		return "expired_upstream"
	case FailureSessionMismatch:
		return "session_mismatch"
	case FailureTooFrequent:
		return "too_frequent"
	case FailureRejected:
		return "rejected"
	}
	return ""
}

// QrFailure 是一次扫码失败。
type QrFailure struct {
	Kind QrFailureKind
	// TTL 在 [FailureTTLElapsed] 下有效：我们这一侧的等待窗口。
	TTL time.Duration
	// Code 在 [FailureRejected] 下有效（`nil` = 响应里没有 code）。
	Code *int64
	// Message 在 [FailureRejected] 下有效。
	Message string
}

func (f QrFailure) Error() string {
	switch f.Kind {
	case FailureTTLElapsed:
		// ⚠ 秒只出现在**给人看**的这一句里：内部一律 `time.Duration`，
		// 秒是用户单位（`--ttl-seconds`），不该渗进类型里。
		return fmt.Sprintf("二维码等待超时（%d 秒）", int(f.TTL/time.Second))
	case FailureExpiredUpstream:
		return "二维码已失效"
	case FailureSessionMismatch:
		return "sID 不匹配（换码后仍用旧 sID 轮询）"
	case FailureTooFrequent:
		return "扫码登录过于频繁，请稍后再试"
	}
	if f.Code != nil {
		return fmt.Sprintf("登录被拒 code=%d: %s", *f.Code, f.Message)
	}
	return fmt.Sprintf("登录被拒: %s", f.Message)
}

// QrProgress 是扫码登录的推进结果。
type QrProgress struct {
	Kind QrProgressKind
	// UserInfo 在 [QrLoggedIn] 下有效：已解密的 `userInfo`。
	UserInfo any
	// Reason 在 [QrFailed] 下有效。
	Reason QrFailure
}

// IsTerminal 报告是否已到终态（调用方据此停止轮询）。
//
// # 限流为什么**不是**终态
//
// `200059545 扫码登录过于频繁` 的语义是"慢一点再问"，会话本身仍然有效。
// 把它当终态会逼用户回去重开一张码，而重开只会更深地撞进风控 ——
// 所以这里返回 false，由调用方退避后继续问（总时长仍受 TTL 约束）。
func (p QrProgress) IsTerminal() bool {
	if p.Kind == QrLoggedIn {
		return true
	}
	if p.Kind == QrFailed {
		return p.Reason.Kind != FailureTooFrequent
	}
	return false
}

// QrLogin 是扫码登录会话。
type QrLogin struct {
	profile   *upstream.ClientProfile
	sessionID string
	startedAt time.Time
	ttl       time.Duration
	// finished 是终态缓存。`nil` = 还在进行中。
	finished *QrProgress
}

// StartQrLogin 出码。
//
// **零网络请求**：二维码是本地拼的（源码 `createLoginQRcode()` 就是字符串拼接），
// 这是本次抓包推翻的直觉假设里最重要的一个。
func StartQrLogin(
	platform upstream.PcPlatform, deviceID string, rng core.Random, now time.Time,
) *QrLogin {
	return &QrLogin{
		profile:   upstream.NewClientProfile(platform, deviceID),
		sessionID: upstream.NewSessionID(rng),
		startedAt: now,
		ttl:       QRTTL,
	}
}

// QrPayload 返回二维码载荷（交给前端渲染成二维码）。
func (q *QrLogin) QrPayload() string {
	return upstream.QrPayload(q.profile.Platform(), q.profile.DeviceID(), q.sessionID)
}

// Profile 返回端档案（调用方需要它来发别的请求时用）。
func (q *QrLogin) Profile() *upstream.ClientProfile { return q.profile }

// SessionID 返回 `sID`。换码时要一起换掉，否则会得到 [FailureSessionMismatch]。
func (q *QrLogin) SessionID() string { return q.sessionID }

// WithTTL 覆盖默认有效期。
//
// 用于"把二维码发给别人扫"的场景（比如终端里打印出来、或者远程协助）——
// 那时 120 秒不够，而重出一次码的代价只是重新扫。
func (q *QrLogin) WithTTL(d time.Duration) *QrLogin {
	q.ttl = d
	return q
}

// NextDelay 回答下次该隔多久再问；`0` = 别再问了。
//
// 终态与超时都返回 0 —— 调用方只认这一个判断，
// 不需要自己再记一套"成功了吗 / 超时了吗"。
func (q *QrLogin) NextDelay(now time.Time) time.Duration {
	if q.finished != nil {
		return 0
	}
	if q.IsExpired(now) {
		return 0
	}
	return QRPollInterval
}

// IsExpired 报告是否已超过我们这一侧的 TTL。
func (q *QrLogin) IsExpired(now time.Time) bool {
	return !now.Before(q.startedAt.Add(q.ttl))
}

// Poll 轮询一次并推进状态。
//
// 已到终态时**不发请求**、直接返回上次的结论（对应客户端的 `clearInterval`）。
func (q *QrLogin) Poll(
	ctx context.Context,
	up *upstream.Upstream,
	tid string,
	decryptor upstream.LoginPayloadDecryptor,
	now time.Time,
) (QrProgress, *core.Error) {
	if q.finished != nil {
		return *q.finished, nil
	}

	if q.IsExpired(now) {
		done := QrProgress{Kind: QrFailed, Reason: QrFailure{
			Kind: FailureTTLElapsed, TTL: q.ttl,
		}}
		q.finished = &done
		return done, nil
	}

	body := upstream.NewQRThirdLoginBody(q.profile.Platform(), q.sessionID)
	envelope, err := up.ThirdLogin(ctx, q.profile, tid, body)
	if err != nil {
		return QrProgress{}, err
	}

	progress := classifyQrPoll(envelope, decryptor)
	if progress.IsTerminal() {
		q.finished = &progress
	}
	return progress, nil
}

// classifyQrPoll 把上游的轮询判定翻译成状态机的推进结果。
func classifyQrPoll(
	envelope *upstream.Envelope, decryptor upstream.LoginPayloadDecryptor,
) QrProgress {
	outcome := upstream.ClassifyPoll(envelope)
	switch outcome.Kind {
	case upstream.PollNotScanned:
		return QrProgress{Kind: QrNotScanned}
	case upstream.PollScanned:
		return QrProgress{Kind: QrScanned}
	case upstream.PollExpired:
		return QrProgress{Kind: QrFailed, Reason: QrFailure{Kind: FailureExpiredUpstream}}
	case upstream.PollSessionMismatch:
		return QrProgress{Kind: QrFailed, Reason: QrFailure{Kind: FailureSessionMismatch}}
	case upstream.PollTooFrequent:
		return QrProgress{Kind: QrFailed, Reason: QrFailure{Kind: FailureTooFrequent}}
	case upstream.PollSuccess:
		userInfo, err := decryptor.DecryptUserInfo(outcome.Encrypted)
		if err != nil {
			return QrProgress{Kind: QrFailed, Reason: QrFailure{
				Kind:    FailureRejected,
				Message: err.Message(),
			}}
		}
		return QrProgress{Kind: QrLoggedIn, UserInfo: userInfo}
	}
	return QrProgress{Kind: QrFailed, Reason: QrFailure{
		Kind: FailureRejected, Code: outcome.Code, Message: outcome.Message,
	}}
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信
// ─────────────────────────────────────────────────────────────────────────────

// SmsLogin 是短信登录会话。
//
// 链路的完整形状（含降级）：
//
//	发码 ──成功──▶ 等用户输验证码 ──▶ thirdlogin(pintype=5)
//	 │
//	 └─200059554──▶ 取滑块图 ──▶ 带滑块再发码
//	                  │
//	                  └─200059553──▶ **必须重新取图**（旧挑战作废），不能复用
//
// 所以本类型只提供"一步一步"的入口，把"要不要重试、取哪张图"留给调用方 ——
// 滑块图是 IO（要渲染给人看），本层不碰。
type SmsLogin struct {
	profile *upstream.ClientProfile
	phone   string
	// random 是发码请求里的 `random`：**24 位**十六进制（不是 UUID）。
	random string
	// remainGetTimes 是服务端给的配额余量。跟踪它是为了不把配额无谓耗光。
	remainGetTimes *int64
}

// StartSmsLogin 开始一次短信登录。
func StartSmsLogin(
	platform upstream.PcPlatform, deviceID, phone string, rng core.Random,
) *SmsLogin {
	return &SmsLogin{
		profile: upstream.NewClientProfile(platform, deviceID),
		phone:   phone,
		random:  upstream.NewSmsRandom(rng),
	}
}

// Phone 返回手机号。
func (s *SmsLogin) Phone() string { return s.phone }

// Profile 返回端档案（取滑块图等额外请求要用）。
func (s *SmsLogin) Profile() *upstream.ClientProfile { return s.profile }

// RemainGetTimes 返回配额余量（服务端每次成功发码时返回）。
func (s *SmsLogin) RemainGetTimes() *int64 { return s.remainGetTimes }

// Random 返回本轮发码用的 `random`。换一次发码要换一个值，所以失败重试时用 [SmsLogin.Reroll]。
func (s *SmsLogin) Random() string { return s.random }

// Reroll 换一个新的 `random`（重试发码前调用）。
func (s *SmsLogin) Reroll(rng core.Random) { s.random = upstream.NewSmsRandom(rng) }

// SendCode 不带滑块的发码。
func (s *SmsLogin) SendCode(ctx context.Context, up *upstream.Upstream, tid string) (upstream.SendSmsOutcome, *core.Error) {
	body := upstream.NewSendSmsCodeBody(s.profile.Platform(), s.phone, s.random)
	return s.finishSend(up.SendSmsCode(ctx, s.profile, tid, body))
}

// SendCodeWithSlider 带滑块的发码。
//
// `sliderAnswer` 是**拖动像素偏移的十进制字符串**（实测两条真实请求解出来是 `461` / `391`）。
// 走这条路的前提是先调 `/user/verfycode` 拿到图 —— 那是调用方的事。
func (s *SmsLogin) SendCodeWithSlider(
	ctx context.Context, up *upstream.Upstream, tid, sliderAnswer string,
) (upstream.SendSmsOutcome, *core.Error) {
	body, err := upstream.NewSendSmsCodeBodyWithSlider(
		s.profile.Platform(), s.phone, s.random, sliderAnswer)
	if err != nil {
		return upstream.SendSmsOutcome{}, err
	}
	return s.finishSend(up.SendSmsCode(ctx, s.profile, tid, body))
}

// SolveSlider 取滑块挑战并**自动解出**拖动答案。
//
// # 为什么不需要图像识别
//
// 服务端把缺口横坐标加密后直接给了出来（`data.puzzleLeft`，密钥是客户端源码里
// 那个公开常量），见 [upstream.SliderPuzzle.Answer]。
// 2026-09-18 实测解出 `370`，而同一响应的 `picWidth` 是 `680`。
func (s *SmsLogin) SolveSlider(ctx context.Context, up *upstream.Upstream, tid string) (string, *core.Error) {
	puzzle, err := up.FetchSliderPuzzle(ctx, s.profile, tid, s.phone)
	if err != nil {
		return "", err
	}
	return puzzle.Answer()
}

// SliderPuzzle 只取滑块挑战（需要把图给用户看时用；正常路径走 [SmsLogin.SolveSlider]）。
//
// 留着它是为了"自动解不出来"时仍有退路：拿到图 → 人工量缺口 →
// [SmsLogin.SendCodeWithSlider]。那条路不需要新代码，
// 但接口得先在这里，不然退路等于不存在。
func (s *SmsLogin) SliderPuzzle(
	ctx context.Context, up *upstream.Upstream, tid string,
) (*upstream.SliderPuzzle, *core.Error) {
	return up.FetchSliderPuzzle(ctx, s.profile, tid, s.phone)
}

func (s *SmsLogin) finishSend(
	envelope *upstream.Envelope, err *core.Error,
) (upstream.SendSmsOutcome, *core.Error) {
	if err != nil {
		return upstream.SendSmsOutcome{}, err
	}
	outcome := upstream.ClassifySendSms(envelope)
	if outcome.Kind == upstream.SmsSent {
		s.remainGetTimes = outcome.RemainGetTimes
	}
	return outcome, nil
}

// Login 用验证码登录，返回**已解密**的 `userInfo`。
//
// `smsCode` 是 6 位明文（`thirdlogin` 的 `dycpwd` 在短信方式下就是明文，不是哈希）。
func (s *SmsLogin) Login(
	ctx context.Context,
	up *upstream.Upstream,
	tid, smsCode string,
	decryptor upstream.LoginPayloadDecryptor,
) (any, *core.Error) {
	body := upstream.NewSmsThirdLoginBody(s.profile.Platform(), s.phone, smsCode)
	envelope, err := up.ThirdLogin(ctx, s.profile, tid, body)
	if err != nil {
		return nil, err
	}

	outcome := upstream.ClassifyPoll(envelope)
	if outcome.Kind == upstream.PollSuccess {
		return decryptor.DecryptUserInfo(outcome.Encrypted)
	}
	// 短信登录不会经过二维码那几个中间态；走到这里说明是这个码不对之类的业务失败。
	// 原样带回上游结论，不臆造"验证码错误"这种结论。
	if outcome.Code != nil {
		return nil, core.NewBadRequest(fmt.Sprintf("登录失败 code=%d: %s", *outcome.Code, outcome.Message))
	}
	return nil, core.NewBadRequest(fmt.Sprintf("登录失败: %s", outcome.Message))
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录结果落库
// ─────────────────────────────────────────────────────────────────────────────

// PCAuthPrefix 是 PC 端 `Authorization` 载荷的第一段。
//
// 客户端源码里是字面量 `pc`（不是平台号，Windows 与 macOS 都用它）。
const PCAuthPrefix = "pc"

// AuthFromUserInfo 用登录得到的 `userInfo` 拼出与客户端**逐字一致**的 `auth`。
//
// # 格式的证据
//
// 客户端 `common/utils.js: sendRequest` L230 写得很直接：
//
//	"Basic "+btoa("pc:"+window.UserInfo.account+":"+window.UserInfo.authToken)
//
// 也就是 `base64("pc:<account>:<authToken>")` —— 注意**第一段是 `pc` 而不是平台号**。
func AuthFromUserInfo(userInfo any) (string, *core.Error) {
	return AuthFromUserInfoWithPrefix(userInfo, PCAuthPrefix)
}

// AuthFromUserInfoWithPrefix 同 [AuthFromUserInfo]，但显式指定载荷的第一段。
//
// 之所以不把前缀做成"默认 `pc`"的入参：默认值会让"给 App 的登录结果套了 PC 的前缀"
// 变成一个静默通过的调用，而那正是最难在事后发现的一类错误。
// （App 端传 `"mobile"`：实测 1076 处 `Authorization` 解码后全是 `mobile:<手机号>:<authToken>`，
// 见 docs/client-profiles.md §3.3。）
func AuthFromUserInfoWithPrefix(userInfo any, prefix string) (string, *core.Error) {
	field := func(name string) (string, *core.Error) {
		text, ok := userInfoField(userInfo, name)
		if !ok || text == "" {
			return "", core.NewParse(fmt.Sprintf("登录结果缺少字段 %s", name))
		}
		return text, nil
	}

	account, err := field("account")
	if err != nil {
		return "", err
	}
	authToken, err := field("authToken")
	if err != nil {
		return "", err
	}
	raw := base64.StdEncoding.EncodeToString([]byte(prefix + ":" + account + ":" + authToken))
	return "Basic " + raw, nil
}

// AccountAndStateFromLogin 把登录结果 → 领域对象（**纯函数**，便于逐字段断言）。
//
// 返回 `(账号, 登录态)`：前者是"能直接给前端看的信息"，后者是"能直接冒充登录的凭据"，
// 两者在存储里本来就分表（见 internal/io 的 schema 说明）。
func AccountAndStateFromLogin(
	scope core.Scope, deviceID string, userInfo any, now time.Time,
) (core.Account, core.AccountState, *core.Error) {
	return AccountAndStateFromLoginWithPrefix(scope, deviceID, userInfo, PCAuthPrefix, now)
}

// AccountAndStateFromLoginWithPrefix 同 [AccountAndStateFromLogin]，
// 但显式指定 `Authorization` 载荷的第一段（App 端传 `"mobile"`）。
//
// # 为什么这里能容忍两种用户域字段名
//
// 用户域标识的**响应字段名是 `userDomainId`**（形如 `1990000000000000001`）——
// 这条已由一次真机登录打印出的字段清单证实；
// `userId` 是**请求体**里常见的键名，由 `userDomainId` 赋过去。
//
// 既然 `userId` 一次都没在响应里出现过，这里为什么还留着它？
// 因为**别的端的响应/别的流程可能用它**，认两个名字的成本是零，
// 而认错名字的代价是"用户域标识静默为空"。
//
// ⚠ **别把 `userid`（全小写）并进这条兜底链**：它在同一份响应里与
// `userDomainId` **同时出现、值不同**（12 字符 vs 19 位），不是一回事。
func AccountAndStateFromLoginWithPrefix(
	scope core.Scope, deviceID string, userInfo any, prefix string, now time.Time,
) (core.Account, core.AccountState, *core.Error) {
	auth, err := AuthFromUserInfoWithPrefix(userInfo, prefix)
	if err != nil {
		return core.Account{}, core.AccountState{}, err
	}

	// 复用既有解析：它已经有黄金向量对拍，没必要在这里重写一遍分段逻辑。
	parsed, ok := core.ParseAuth(auth)
	if !ok {
		return core.Account{}, core.AccountState{},
			core.NewParse("拼出来的 auth 解析失败（格式与 core.ParseAuth 不一致）")
	}

	phone := ""
	if parsed.Phone != nil {
		phone = *parsed.Phone
	}
	accountID := core.NewAccountID(core.AccountLabel(nil, phone))

	account := core.NewAccount(accountID.String())
	// Scope 为本地范围时是"无主"：本地运维不替谁认领账号，
	// 之后由首个管理员初始化时的认领流程接手。
	account.Owner = scope.Owner()
	// 手机号非空时 AccountID 本身就是掩码，直接复用，避免两处掩码逻辑跑偏
	if phone != "" {
		masked := accountID.String()
		account.PhoneMasked = &masked
	}
	account.Platform = parsed.Platform
	// 旧语义里 `0` 是"没有过期信息"，不是"1970 年过期"
	if millis := parsed.ExpireMillis(); millis != 0 {
		expire := millis
		account.AuthExpire = &expire
	}

	token := ""
	if parsed.Token != nil {
		token = *parsed.Token
	}
	jwtToken := core.SecretValue(token)
	authorization := core.SecretValue(auth)

	state := core.NewAccountState(accountID.String())
	state.JWTToken = &jwtToken
	if deviceID != "" {
		device := core.SecretValue(deviceID)
		state.DeviceID = &device
	}
	// `userId` 只是**请求体**里的键名，`userDomainId` 是**响应**字段名，值是同一个。
	if text, ok := userInfoField(userInfo, "userId"); ok {
		domain := core.UserDomainID(text)
		state.UserDomainID = &domain
	} else if text, ok := userInfoField(userInfo, "userDomainId"); ok {
		domain := core.UserDomainID(text)
		state.UserDomainID = &domain
	}
	// **完整手机号**（v6 起落库）。这里它是**给人看的标识**（日志里认出"签的是谁"），
	// **不是**拼凭据的原料 —— 凭据见下面的 Authorization。
	//
	// 用 `parsed.Phone` 而不是 `phone`（它把空串折成了 `""`）：
	// 库里要区分"有号码"与"上游没给"，`Some("")` 会让下游以为有值。
	if parsed.Phone != nil {
		full := core.FullPhone(*parsed.Phone)
		state.Phone = &full
	}
	// **整串 auth 原样落库**（v7 起）。
	//
	// 注意上面那两行 `AuthFromUserInfoWithPrefix` → `ParseAuth` 的写法：
	// 拼出整串、紧接着又把它拆回三段 —— 那只是**为了拿 `phone` / `platform` /
	// `expire` 三个派生值**（账号表要它们）。凭据本身不再走"拆开再拼"，
	// 所以哪怕将来这个拼装规则变了、或者某天 `phone` 解不出来，
	// 发出去的 `Authorization` 仍然是登录那一刻原样的那一串。
	state.Authorization = &authorization
	state.UpdatedAt = &now

	return account, state, nil
}

// PersistLogin 把登录结果落库，返回账号 id。
//
// 顺序是**先账号、后登录态**：`account_state` 的写入会校验账号归属
// （见 [core.StateStore] 的隔离契约），账号不在就先写凭据会被正确地拒掉。
func PersistLogin(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	deviceID string,
	userInfo any,
	now time.Time,
) (core.AccountID, *core.Error) {
	return PersistLoginWithPrefix(ctx, store, scope, deviceID, userInfo, PCAuthPrefix, now)
}

// PersistLoginWithPrefix 同 [PersistLogin]，
// 但显式指定 `Authorization` 载荷的第一段（App 端传 `"mobile"`）。
func PersistLoginWithPrefix(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	deviceID string,
	userInfo any,
	prefix string,
	now time.Time,
) (core.AccountID, *core.Error) {
	account, state, err := AccountAndStateFromLoginWithPrefix(scope, deviceID, userInfo, prefix, now)
	if err != nil {
		return "", err
	}
	if storeErr := store.UpsertAccount(ctx, scope, &account); storeErr != nil {
		return "", asError(storeErr, "写入账号失败")
	}
	if storeErr := store.UpsertAccountState(ctx, scope, &state); storeErr != nil {
		return "", asError(storeErr, "写入登录态失败")
	}
	return account.ID, nil
}

// userInfoField 从已解码的 `userInfo` 里取一个字符串字段。
//
// ⚠ 只认**字符串**：上游的 `account` / `authToken` 从来都是字符串，
// 而"顺手把数字也转成字符串"会让一个类型变化（上游把 account 改成数字）
// 静默通过 —— 那正是要立刻看见的那类变化。
func userInfoField(userInfo any, name string) (string, bool) {
	obj, ok := userInfo.(map[string]any)
	if !ok {
		return "", false
	}
	text, ok := obj[name].(string)
	return text, ok
}

// asError 把存储层返回的 `error` 归一成 `*core.Error`。
func asError(err error, context string) *core.Error {
	if ce, ok := err.(*core.Error); ok {
		return ce
	}
	return core.NewInternal(fmt.Sprintf("%s: %v", context, err))
}
