// 登录会话表：把"一次登录"从**一次阻塞的调用**变成**浏览器能驱动的多步**。
//
// # 为什么要这张表
//
// [QRLogin] 与 [AppSmsLogin] 的设计前提是"调用方一直在场"：CLI 打印二维码，
// 然后在同一个进程里等 120 秒。HTTP 没有这个前提 —— 一个请求不能挂两分钟，
// 而浏览器只能在两次请求之间携带状态。
//
// 于是这里做的事**不是**重写一遍轮询循环，而是**替浏览器持有那次调用**：
// 驱动照原样跑（含它自己的 TTL、退避、终态不可逆），跑完把结论放进表里，
// 前端每次轮询只是来读一眼结论。
//
// # 为什么复用而不是并行实现一套
//
// 见 [capabilities.QrLogin] 的包注释：CLI 与 WebUI 的差别只该在
// "谁来等这 1000 毫秒"和"谁来渲染二维码"。多一套循环就多一套
// "忘了 clearInterval"的风险 —— 那是客户端源码里专门写过提醒的那种错。
package app

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const (
	// LoginSessionRetention 是**终态**会话留在表里的时长。
	//
	// 留一段而不是立刻删，是因为前端可能在"轮询到成功"与"刷新账号列表"之间
	// 断掉（切后台、网络抖一下）。立刻删的表现是"我扫了码，页面什么也没发生"。
	LoginSessionRetention = 5 * time.Minute

	// maxLiveSessionsPerUser 是一个用户手里**未完成**的登录会话上限。
	//
	// 这不是防 DoS 的通用配额（那是部署者的事），而是防风控：
	// 出码与发码都是**上游侧有代价**的动作（发码还连带解一次滑块），
	// 界面上连点几次就可能撞进 `200059545 扫码登录过于频繁`。
	// 宁可在这里挡下并给一句人话，也不要让上游去教用户怎么点。
	maxLiveSessionsPerUser = 3

	// smsCodeValidity 是验证码会话的存活上限。
	//
	// 上游侧的有效期不由我们控制，这里只是**别让表里堆着一堆不可能再用上的号码**：
	// 用户十分钟没回来，基本就是放弃了。
	smsCodeValidity = 10 * time.Minute
)

// LoginPhase 是会话对外可见的阶段。
//
// 只有这四个值：它是**前端要分支**的东西，不是内部细节的转储。
// 上游的码（`200059541` 未扫、`200059548` 已扫未确认…）留在 [capabilities.QrProgress] 里，
// 失败时以人话文本带出（[LoginSessionView.Message]）。
type LoginPhase string

const (
	// LoginWaiting 是"还在等用户动手"（码没扫 / 没确认）。
	LoginWaiting LoginPhase = "waiting"
	// LoginScanned 是"已扫码，等手机上确认"。
	LoginScanned LoginPhase = "scanned"
	// LoginSucceeded 是登录成功且**已落库**。
	LoginSucceeded LoginPhase = "succeeded"
	// LoginFailed 是终态失败。
	LoginFailed LoginPhase = "failed"
)

// LoginSessionView 是前端能拿到的全部结论。
//
// ⚠ 这里**没有**任何凭据字段：成功只给掩码形状的账号 id。
// 落库由会话内部完成，凭据从驱动直接进存储，不经过这条出网路径。
type LoginSessionView struct {
	Phase     LoginPhase
	AccountID string
	// Message 是失败时的人话（来自驱动的 `*core.Error` 文本）。
	Message string
	// RemainGetTimes / SolvedSlider / CaptchaURL 只在"发短信验证码"这条路上有效，
	// 语义照抄 [AppCodeOutcome] 的同名字段。
	RemainGetTimes *int64
	SolvedSlider   bool
	CaptchaURL     string
}

// IsTerminal 报告前端是否该停止轮询。
func (v LoginSessionView) IsTerminal() bool {
	return v.Phase == LoginSucceeded || v.Phase == LoginFailed
}

// LoginHub 是会话表本身。
//
// 设备指纹由**两个文件路径**提供（不是两个字符串）：指纹必须跨重启稳定，
// 所以在第一次用到时才读盘、读不到就生成并写回（[LoadOrCreateDeviceID]）。
// 传字符串进来会让"每次重启换一台设备"这种配置错误编译得过、跑得起来也看不出问题。
type LoginHub struct {
	store         core.StateStore
	up            *upstream.Upstream
	clock         core.Clock
	rng           core.Random
	decryptor     upstream.LoginPayloadDecryptor
	appCrypto     *upstream.AppLoginCrypto
	pcDeviceFile  string
	appDeviceFile string
	waiter        Waiter

	mu       sync.Mutex
	sessions map[string]*loginEntry
}

// loginEntry 是一次未结束或刚结束的登录。
//
// 它同时承载扫码与短信两条路，但**同一时刻只有一组字段有意义**
// （kind 决定读哪组）—— 分两个类型会让"表"变成两张，
// 而"一个用户手里有几个未完成会话"这条配额恰恰要跨两类统计。
type loginEntry struct {
	id      string
	kind    string
	owner   core.UserID
	created time.Time
	updated time.Time

	mu      sync.Mutex
	payload string // 扫码：二维码载荷
	phase   LoginPhase
	message string
	landed  *core.AccountID // 成功落库后的账号 id

	// 短信验证码这一路
	phone   string
	profile *upstream.AppProfile
	// pcLogin 是 **PC 端**那条短信路的登录上下文（端档案 + 设备指纹）。
	// 与上面的 profile 互斥，由 kind 决定读哪个：发码与填码是两次 HTTP，
	// 中间那份"上游按哪个 dID 记的这次会话"必须原样带回来，否则第二段换不到码。
	pcLogin *LoginContext
}

const (
	loginKindQR     = "qr"
	loginKindAppSMS = "app-sms"
	loginKindPcSMS  = "pc-sms"
)

// NewLoginHub 组装会话表。
//
// `up` 由调用方给（生产是 [upstream.NewUpstream]）：测试要能换成脚本化的假客户端，
// 而"能不能换"这件事必须在这一层就成立 —— 否则只能真打上游。
//
// `waiter` 是轮询之间的等待器，nil 表示真的等（[RealWaiter]）。理由与
// [QRLogin] 那条一样：一条"轮询到未扫码再轮到成功"的用例若真等两个间隔，
// 测试会变慢且**挂住**的样子与超时无异。
func NewLoginHub(
	store core.StateStore,
	up *upstream.Upstream,
	clock core.Clock,
	rng core.Random,
	pcDeviceFile, appDeviceFile string,
	waiter Waiter,
) *LoginHub {
	if waiter == nil {
		waiter = RealWaiter{}
	}
	return &LoginHub{
		store:         store,
		up:            up,
		clock:         clock,
		rng:           rng,
		decryptor:     upstream.NewLoginDataDecryptorProduction(),
		appCrypto:     upstream.NewAppLoginCryptoProduction(),
		pcDeviceFile:  pcDeviceFile,
		appDeviceFile: appDeviceFile,
		waiter:        waiter,
		sessions:      map[string]*loginEntry{},
	}
}

// newID 造一个会话 id。
//
// 用 [core.UUIDv4] 而不是自增计数：计数会在重启后重复，
// 而"重启后两张不同的登录撞上同一个 id"正是最难复现的那类 bug。
func (h *LoginHub) newID() string { return core.UUIDv4(h.rng) }

// sweepLocked 丢掉过期的会话。调用方必须已持有 h.mu。
//
// 在**每次公开入口**顺手扫一遍，而不是起一个后台协程：
// 表不大（配额每人几条），而后台协程要回答"什么时候停"——
// 那需要一个 Close 生命周期，服务层现在没有、也不该为一个 map 的清理长出一种。
func (h *LoginHub) sweepLocked(now time.Time) {
	for id, entry := range h.sessions {
		entry.mu.Lock()
		terminal := entry.phase == LoginSucceeded || entry.phase == LoginFailed
		entry.mu.Unlock()

		age := now.Sub(entry.created)
		switch {
		case terminal && age > LoginSessionRetention:
			delete(h.sessions, id)
		case isSmsKind(entry.kind) && age > smsCodeValidity:
			delete(h.sessions, id)
		}
	}
}

// isSmsKind 报告这个会话是不是"发了短信验证码、在等用户回填"的那一类。
//
// 这一类要单独有一条 TTL（[smsCodeValidity]）：扫码那一路的会话由后台协程自己收到终态，
// 而短信会话没人轮询 —— 用户十分钟没回来，表里就该把它清掉，
// 否则它一直占着 [maxLiveSessionsPerUser] 的一个名额。
// 写成函数而不是 `||`：再加第三种端（PC/App 之外）时，漏掉这一处的表现是
// "某种会话永远清不掉、把配额吃满"，而那很难看出现场。
func isSmsKind(kind string) bool {
	return kind == loginKindAppSMS || kind == loginKindPcSMS
}

// liveCountLocked 数某个用户手里**未完成**的会话数。
func (h *LoginHub) liveCountLocked(owner core.UserID) int {
	n := 0
	for _, entry := range h.sessions {
		if entry.owner != owner {
			continue
		}
		entry.mu.Lock()
		terminal := entry.phase == LoginSucceeded || entry.phase == LoginFailed
		entry.mu.Unlock()
		if !terminal {
			n++
		}
	}
	return n
}

// tooManyLive 是配额挡下时给的那一句。
func tooManyLive() *core.Error {
	return core.NewBadRequest(fmt.Sprintf(
		"你已经有 %d 个未完成的登录，先等它们结束或过期", maxLiveSessionsPerUser))
}

// take 按 id 取会话，**并核对归属**。
//
// 不属于你时返回 false —— 调用方据此给 404，与存储层"看不见即不存在"同一口径。
// 这一条不是锦上添花：会话 id 是 uuid，但"能不能读到别人的登录结论"
// 决定了这个 id 空间是不是一个横向越权的入口。
func (h *LoginHub) take(id string, scope core.Scope) (*loginEntry, bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	entry, ok := h.sessions[id]
	if !ok {
		return nil, false
	}
	if owner := scope.Owner(); owner == nil || *owner != entry.owner {
		return nil, false
	}
	return entry, true
}

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// StartQRLogin 出一张码并开始等，返回会话 id 与二维码载荷。
//
// 返回前**一定**已经拿到载荷：驱动的第一句就是 `observer.OnQR(...)`，
// 出码是本地拼字符串、一个请求都不发（这条反直觉的事实见 docs/upstream/login-protocol.md）。
// 不在这里等的话，"前端拿到 id 却立刻去要图"会撞上一个空载荷，
// 表现是一张 500 的二维码 —— 而那一刻没人会想到是竞态。
func (h *LoginHub) StartQRLogin(
	ctx context.Context, scope core.Scope, platform upstream.PcPlatform,
) (string, string, *core.Error) {
	deviceID, perr := LoadOrCreateDeviceID(h.pcDeviceFile, h.rng)
	if perr != nil {
		return "", "", perr
	}

	entry, perr := h.reserve(scope, loginKindQR)
	if perr != nil {
		return "", "", perr
	}

	payload := make(chan string, 1)
	observer := &hubObserver{entry: entry, qr: payload}
	loginContext := NewLoginContext(scope, platform, deviceID)

	go func() {
		outcome, err := QRLogin(
			context.WithoutCancel(ctx), h.store, h.up, loginContext,
			h.decryptor, h.clock, h.rng, observer, h.waiter)
		h.finish(entry, outcome, err)
	}()

	select {
	case text := <-payload:
		return entry.id, text, nil
	case <-ctx.Done():
		// 请求被掐断：会话留在表里，驱动那边会因为 ctx 取消自己收摊
		return "", "", core.NewTimeout("出码被取消")
	}
}

// QRImage 返回这张码的 SVG。
func (h *LoginHub) QRImage(id string, scope core.Scope) ([]byte, bool, *core.Error) {
	entry, ok := h.take(id, scope)
	if !ok {
		return nil, false, nil
	}
	entry.mu.Lock()
	payload := entry.payload
	entry.mu.Unlock()
	if payload == "" {
		return nil, false, core.NewInternal("会话里没有二维码载荷（出码那一步没跑到）")
	}
	svg, err := QRSVG(payload)
	if err != nil {
		return nil, false, core.NewBadRequest(err.Error())
	}
	return svg, true, nil
}

// View 读一次会话结论（前端轮询用）。
func (h *LoginHub) View(id string, scope core.Scope) (LoginSessionView, bool) {
	entry, ok := h.take(id, scope)
	if !ok {
		return LoginSessionView{}, false
	}
	entry.mu.Lock()
	defer entry.mu.Unlock()
	view := LoginSessionView{Phase: entry.phase, Message: entry.message}
	if entry.landed != nil {
		view.AccountID = entry.landed.String()
	}
	return view, true
}

// reserve 登记一个新会话，配额不够时拒绝。
func (h *LoginHub) reserve(scope core.Scope, kind string) (*loginEntry, *core.Error) {
	owner := scope.Owner()
	if owner == nil {
		// 本地范围（CLI）不该用这张表：它没有"谁在等"这个概念，
		// 归属为空的会话会让 [LoginHub.take] 永远取不到自己登记的东西。
		return nil, core.NewInternal("登录会话表只服务有归属用户的范围（WebUI）")
	}
	now := h.clock.Now()

	h.mu.Lock()
	defer h.mu.Unlock()
	h.sweepLocked(now)
	if h.liveCountLocked(*owner) >= maxLiveSessionsPerUser {
		return nil, tooManyLive()
	}
	entry := &loginEntry{
		id: h.newID(), kind: kind, owner: *owner,
		created: now, updated: now, phase: LoginWaiting,
	}
	h.sessions[entry.id] = entry
	return entry, nil
}

// hubObserver 把驱动的回调写进会话表。
type hubObserver struct {
	entry *loginEntry
	qr    chan<- string
}

// OnQR 实现 [LoginObserver]。
func (o *hubObserver) OnQR(payload string) {
	o.entry.mu.Lock()
	o.entry.payload = payload
	o.entry.mu.Unlock()
	// 缓冲为 1，且 StartQRLogin 一定会收这一次：不阻塞驱动协程。
	o.qr <- payload
}

// OnProgress 实现 [LoginObserver]：把阶段映射成对外那几个值。
//
// 限流（`FailureTooFrequent`）**不**写成失败：驱动会退避后继续，
// 这里跟着报"还在等"才是对的结论 —— 报了失败而它其实还在轮询，
// 前端会停轮询并显示一个永远不会发生的结果。
func (o *hubObserver) OnProgress(progress capabilities.QrProgress) {
	phase := LoginWaiting
	switch progress.Kind {
	case capabilities.QrScanned:
		phase = LoginScanned
	case capabilities.QrLoggedIn:
		phase = LoginSucceeded
	case capabilities.QrFailed:
		if progress.Reason.Kind != capabilities.FailureTooFrequent {
			phase = LoginFailed
		}
	}
	o.entry.mu.Lock()
	o.entry.phase = phase
	o.entry.mu.Unlock()
}

// finish 收尾：成功记下账号 id，失败记下人话文本。
func (h *LoginHub) finish(entry *loginEntry, outcome LoginOutcome, perr *core.Error) {
	entry.mu.Lock()
	defer entry.mu.Unlock()
	entry.updated = h.clock.Now()

	if perr != nil {
		entry.phase = LoginFailed
		entry.message = perr.Error()
		return
	}
	id := outcome.AccountID
	entry.landed = &id
	entry.phase = LoginSucceeded
}

// ─────────────────────────────────────────────────────────────────────────────
// App 端短信验证码
// ─────────────────────────────────────────────────────────────────────────────

// SendAppCode 发一次 App 端验证码，登记会话。
//
// # 先占额度，再打上游
//
// 配额的意义是"别让发码这件事被连点刷出去"（发码连带一次滑块、还计短信配额），
// 所以挡下的时机必须在**发出去之前**。登记完再发、发失败就撤销登记 ——
// 于是"发不出去时表里不留条目"这条依然成立，只是改由 defer 保证。
//
// # 需要滑块兜底时**照样登记**
//
// 那条路的语义是"去这个地址把滑块过了，再回来填码"，会话还得留着等第二步。
// 反过来，若把它当成失败撤掉登记，前端就手里没 id、第二步没有对象可填。
func (h *LoginHub) SendAppCode(
	ctx context.Context, scope core.Scope, phone string,
) (string, LoginSessionView, *core.Error) {
	phone = strings.TrimSpace(phone)
	if phone == "" {
		return "", LoginSessionView{}, core.NewBadRequest("手机号不能为空")
	}
	entry, perr := h.reserve(scope, loginKindAppSMS)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	// 只要没走到"会话可继续"那一步，就把登记撤回来 —— 否则额度会被
	// 一堆永远 verify 不上的条目吃掉，而用户看到的是"什么都不点也登录不了"。
	keep := false
	defer func() {
		if !keep {
			h.drop(entry.id)
		}
	}()

	deviceID, perr := LoadOrCreateAppDeviceID(h.appDeviceFile, h.rng)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	// `deviceShort` 每次现生成（它不是"每安装"指纹，实测同一 deviceId 换过 short），
	// 但**这一次登录内部两段必须用同一个** —— 所以它跟着会话走，而不是全局一份。
	profile := upstream.NewAppProfile(
		upstream.NewIPadAppDevice(deviceID, NewAppDeviceShort(h.rng)))

	// 登录过一次之后 `userDomainId` 就在库里了，第二次起要带上（真机也一样）。
	profile, perr = ProfileWithStoredDomainID(
		ctx, h.store, scope, profile, core.NewAccountID(core.AccountLabel(nil, phone)))
	if perr != nil {
		return "", LoginSessionView{}, perr
	}

	outcome, perr := AppSendSmsCode(ctx, h.up, profile, phone, h.clock)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	entry.phone = phone
	entry.profile = profile

	view := LoginSessionView{Phase: LoginWaiting}
	switch outcome.Kind {
	case AppCodeSent:
		view.RemainGetTimes = outcome.RemainGetTimes
		view.SolvedSlider = outcome.SolvedSlider
	case AppCodeNeedCaptcha:
		view.CaptchaURL = outcome.URL
	default:
		// 结构上到不了：[AppSendSmsCode] 只返回这两种。
		// 留着是"哪天加了第三种"时让这里显式想一遍，而不是静默当成已发送。
		return "", LoginSessionView{}, core.NewInternal(
			fmt.Sprintf("未预期的发码结果类别 %s", outcome.Kind))
	}
	keep = true
	return entry.id, view, nil
}

// VerifyAppCode 用验证码登录并落库。
//
// # 一次会话只能落一次
//
// 验证码错了可以重来（`9441` 会带着那句"重新取一次验证码"回来），
// 但**成功之后**再拿同一个会话去 verify 必须被挡：那次会再打一次上游、
// 再覆盖一次凭据，而用户界面上早已看到"成功"。
// 挡在 `landed` 上而不是挡在"前端不会再点了"上 —— 前端的按钮禁用不是保证。
func (h *LoginHub) VerifyAppCode(
	ctx context.Context, scope core.Scope, id, code string,
) (LoginSessionView, *core.Error) {
	entry, ok := h.take(id, scope)
	if !ok {
		return LoginSessionView{}, core.NewNotFound("登录会话")
	}
	entry.mu.Lock()
	if entry.landed != nil {
		accountID := *entry.landed
		entry.mu.Unlock()
		return LoginSessionView{Phase: LoginSucceeded, AccountID: accountID.String()}, nil
	}
	phone, profile := entry.phone, entry.profile
	entry.mu.Unlock()

	if profile == nil {
		// 这个会话不是 App 短信那一类（扫码会话没有 profile，PC 会话带的是
		// 另一套端档案）：与 [VerifyPcCode] 同一口径给 404，不解释表里有几种会话。
		// 少了这一句就是**空指针 panic** —— 前端串了线（或有人拿别人的 id 乱试）
		// 会把这一个请求打成 500 并往日志里堆栈。
		return LoginSessionView{}, core.NewNotFound("登录会话")
	}
	if strings.TrimSpace(code) == "" {
		return LoginSessionView{}, core.NewBadRequest("验证码不能为空")
	}

	// 归属直接用**请求带来的那个 scope**（[take] 已经核对过会话是它的）：
	// 从 entry 里再拼一个 ScopeUser 等于把"谁的凭据"这件事交给表里的副本，
	// 而表与请求不一致时（重启、并发接管）它才是那个静默的错。
	outcome, perr := AppSmsLogin(
		ctx, h.store, h.up,
		AppLoginContext{
			Scope:      scope,
			Profile:    profile,
			Credential: SmsCredential{Phone: phone, Code: strings.TrimSpace(code)},
		},
		h.appCrypto, h.clock, h.rng)
	if perr != nil {
		entry.mu.Lock()
		entry.phase, entry.message = LoginFailed, perr.Error()
		entry.updated = h.clock.Now()
		entry.mu.Unlock()
		return LoginSessionView{}, perr
	}

	entry.mu.Lock()
	landed := outcome.AccountID
	entry.landed = &landed
	entry.phase, entry.message = LoginSucceeded, ""
	entry.updated = h.clock.Now()
	entry.mu.Unlock()

	return LoginSessionView{Phase: LoginSucceeded, AccountID: outcome.AccountID.String()}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// PC 端短信验证码
// ─────────────────────────────────────────────────────────────────────────────

// SendPcCode 发一次 PC 端验证码，登记会话。
//
// # 与 App 那条的三处不同（都是上游本来就不一样，不是我们分的档）
//
//   - 端档案是 `clientType`/`cpid`（Windows / macOS 两份），App 那条是 iPad 设备串；
//   - 滑块在这里是**自动解掉**的（[SmsSendCode] 内部循环），所以没有
//     `captchaUrl` 那一支 —— 解不动就直接失败，不给"去这个地址拖一下"的兜底；
//   - 设备指纹用的是 PC 那份文件，与扫码共用同一个：同一台"电脑"不该因为
//     换了登录方式就变成两台设备，那会让上游侧的风控画像前后不一致。
//
// 配额挡在打上游之前、失败即撤销登记，这两条与 [SendAppCode] 同理。
func (h *LoginHub) SendPcCode(
	ctx context.Context, scope core.Scope, platform upstream.PcPlatform, phone string,
) (string, LoginSessionView, *core.Error) {
	phone = strings.TrimSpace(phone)
	if phone == "" {
		return "", LoginSessionView{}, core.NewBadRequest("手机号不能为空")
	}
	deviceID, perr := LoadOrCreateDeviceID(h.pcDeviceFile, h.rng)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	entry, perr := h.reserve(scope, loginKindPcSMS)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	keep := false
	defer func() {
		if !keep {
			h.drop(entry.id)
		}
	}()

	sent, perr := SmsSendCode(ctx, h.up, platform, deviceID, phone, h.rng)
	if perr != nil {
		return "", LoginSessionView{}, perr
	}
	loginContext := NewLoginContext(scope, platform, deviceID)
	entry.mu.Lock()
	entry.phone = phone
	entry.pcLogin = &loginContext
	entry.mu.Unlock()

	keep = true
	return entry.id, LoginSessionView{
		Phase:          LoginWaiting,
		RemainGetTimes: sent.RemainGetTimes,
		SolvedSlider:   sent.SolvedSlider,
	}, nil
}

// VerifyPcCode 用 PC 端验证码换登录并落库。
//
// 幂等口径与 [VerifyAppCode] 同一条：成功后同一会话再提交不再打上游、不再覆盖凭据。
func (h *LoginHub) VerifyPcCode(
	ctx context.Context, scope core.Scope, id, code string,
) (LoginSessionView, *core.Error) {
	entry, ok := h.take(id, scope)
	if !ok {
		return LoginSessionView{}, core.NewNotFound("登录会话")
	}
	entry.mu.Lock()
	landed := entry.landed
	pcLogin, phone := entry.pcLogin, entry.phone
	entry.mu.Unlock()
	if landed != nil {
		return LoginSessionView{Phase: LoginSucceeded, AccountID: landed.String()}, nil
	}
	if pcLogin == nil {
		// kind 对不上（拿 App 的会话来走 PC 的填码口）：那是前端串了线，
		// 给 404 与"会话不存在"同一口径，不解释内部有哪几种会话。
		return LoginSessionView{}, core.NewNotFound("登录会话")
	}
	if strings.TrimSpace(code) == "" {
		return LoginSessionView{}, core.NewBadRequest("验证码不能为空")
	}

	outcome, perr := SmsLogin(
		ctx, h.store, h.up, *pcLogin,
		SmsCredential{Phone: phone, Code: strings.TrimSpace(code)},
		h.decryptor, h.clock, h.rng)
	if perr != nil {
		entry.mu.Lock()
		entry.phase, entry.message = LoginFailed, perr.Error()
		entry.updated = h.clock.Now()
		entry.mu.Unlock()
		return LoginSessionView{}, perr
	}

	entry.mu.Lock()
	landedNow := outcome.AccountID
	entry.landed = &landedNow
	entry.phase, entry.message = LoginSucceeded, ""
	entry.updated = h.clock.Now()
	entry.mu.Unlock()

	return LoginSessionView{Phase: LoginSucceeded, AccountID: outcome.AccountID.String()}, nil
}

// drop 丢掉一个刚登记的会话（发码结果不属于可继续的情形）。
func (h *LoginHub) drop(id string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	delete(h.sessions, id)
}
