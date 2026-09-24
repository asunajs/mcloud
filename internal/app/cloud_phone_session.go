package app

// 红包派对（#18）这一族的**会话**：把"库里那个账号"变成"能直接发请求的活动站票"。
//
// # 这一族与签到页那一条**不是同一张票**
//
// 签到那一条交的是 `jwttoken`（H5 会话），而本族每个请求要的是**活动站自己的 token**：
// `sign = md5(requestId + APP_ID + token + SIGN_SALT + timestamp)`，
// 且 `requestId`/`timestamp` **每次重算**（成文 §2.4 第 371 行；旧 TS `task.ts:57-71` 的
// `getHeaders()` 每次调用都新取一个 `Date.now()`）。
// ⇒ 所以这里持有的不是"一组头"，而是**换头的那枚票 + 拼头的算式所需的时钟与随机源**
// （上游那一层刻意只放素材与算式，理由写在 `upstream.CloudPhoneAuth` 的头注里）。
//
// # 建会话是两步，而且第①步有**两个端点**可选（本文件选的是哪一个，见下面那段）
//
//	① 换 SSO token   ② POST …/user/tokenValidate {pintype:13, token:<①>, deviceId, loginConfig}
//
// 旧 TS 走的是 `querySpecTokenNew`（**GET** `m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/
// querySpecToken?targetSourceId=001216`，带 H5 签名四件套）；本仓已实跑通的是
// `(u *Upstream).AppQuerySpecToken`（**POST** `user-njs.yun.139.com/user/querySpecTokenV2`，
// body `{toSourceId}`）。**两个端点都没有一条 001216 的实拍**。
//
// 这里选 **POST 那一条（本仓已实跑通）**，理由与代价都摆明：
//   - 理由：同一枚入参换 SSO token，本仓那一条在 001005 / 017027 上**真跑通过**
//     （`OpenSignInSession`、直播门那两处），而 GET 那一条在本仓**一次都没发过**
//     ——"旧 TS 跑过"是它的证据，但跑的是**它那份 HTTP 客户端与签名装配**，不是我们的。
//   - 代价：这一枪的 body 键名（`toSourceId`）与 GET 那一条的查询键（`targetSourceId`）
//     **不是同一个字面量**，而两处的 sourceId 值都是 `001216` ⇒ 上游若按端点区分活动，
//     这条会失败。**怎么定**：真打时若第②步回非 200，先发一条 GET 那一条比对（构造器在
//     `upstream.PlayAIQuerySpecTokenRequest`，同一 URL 换个 sourceId 就是本族要的形态）。
//   - ⚠ 这不叫"特有取法已查明"：`AGENTS.md` §1.12 的特有优先针对的是 **`jwttoken`**，
//     本族压根不用那张票；此处两条候选都属"有出处、无实拍"，选实跑通的那条是**工程判断**，
//     写成这样是为了下一个读者能一眼看见它不是结论。
//
// # 活动站 token 的失效判据：**没有**
//
// 上游只给了 `expireTime`（实拍数字 `3600`），而同一个键名在另一条样本上是**毫秒时间戳**
// ⇒ 同名两义（`upstream.CloudPhoneTokenValidate.ExpireTime` 那条注释记的就是这件事）。
// 所以本文件**不自造 TTL**（`AGENTS.md` §1.12：失效判据只有上游那一句，而上游这一族
// 连那一句都没给 —— "票过期了上游回什么"零样本）。
// 后果说清楚：票自然过期后的第一次运行会**整片**回非 200，而我们**不自动重建**
// （自动重建需要一个判据，猜一个就是把"活动下线"读成"票坏了"然后白打一轮）。
// 要补这一格：抓一条"明知过期"的响应原文，看它的 `header.status` 与 `errMsg` 是哪一码。

import (
	"context"
	"strconv"
	"sync"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// CloudPhoneSession 是一条已登录活动站的会话素材。
//
// 三个字段里只有 `token` 是凭据；`referer` 与 `xOrigin` 是**成文过的地址字面量**
// （旧 TS 那两枚都不带 token，所以这里也没有"把凭据拼进 URL"的可能）。
//
// ⚠ 刻意**不**实现 `String()`：`token` 会跟着 `%v` 一起出去（同 [SignInSession] 的处置）。
// 需要描述时用 [CloudPhoneSession.Describe]。
type CloudPhoneSession struct {
	token   string
	referer string
	xOrigin string
}

// Describe 返回可安全打印的形态：只给长度，不给内容。
func (s CloudPhoneSession) Describe() string {
	return "CloudPhoneSession{token: <" + strconv.Itoa(len(s.token)) + " 字符>" +
		", referer: " + s.referer + ", xOriginSet: " + strconv.FormatBool(s.xOrigin != "") + "}"
}

// same 按**内容**比两条会话（与 [SignInSession.same] 同一用途：测试要能钉住"复用的是同一条"）。
func (s CloudPhoneSession) same(other CloudPhoneSession) bool {
	return s.token == other.token && s.referer == other.referer && s.xOrigin == other.xOrigin
}

// AuthFor 按这一张票**现算**一组鉴权头素材。
//
// # 为什么每次调用都新的
//
// `requestId` 与 `timestamp` 按成文是"每次重算"，而 `sign` 是它们的 md5 ——
// 复用一组等于把同一个 `requestId` 发给 N 条请求。真实客户端不是这么发的
// （旧 TS 每条请求各调一次 `getHeaders()`）。
//
// # 两枚可选头**在这里一起算**，不在会话里存
//
// 旧 TS `task.ts:57-71` 里，`x-DeviceInfo` 第 7 段那串尾巴的**前缀就是同一趟的
// `timestamp`**（`${timestamp}${randomStr}`）⇒ 头与设备串共用一个 `Date.now()` 读数。
// 所以这两样必须由同一个函数同时产出：分开取时钟就会出现"头说 12:00:00.500、
// 设备串说 12:00:00.503"。⚠ 上游会不会因此判失败 ⇒ **未测试**；这么写是为了
// **不主动制造**一个真实客户端没有的不一致。
//
// 时钟与随机源都由参数给（不从 session 里持有）：[CloudPhoneSession] 是个值类型，
// 一份票可能被多个 goroutine 同时用它算头。
func (s CloudPhoneSession) AuthFor(clock core.Clock, rng core.Random) upstream.CloudPhoneAuth {
	now := clock.Now()
	milli := now.UnixMilli()
	timestamp := upstream.CloudPhoneTimestampMilli(milli)
	return upstream.CloudPhoneAuth{
		RequestID: upstream.CloudPhoneRequestID(now, milli, upstream.CloudPhoneRequestIDSuffix(rng)),
		Timestamp: timestamp,
		Token:     s.token,
		DeviceInfo: upstream.CloudPhoneDeviceInfo(timestamp,
			upstream.CloudPhoneDeviceInfoSuffix(rng)),
		XOrigin: s.xOrigin,
	}
}

// OpenCloudPhoneSession 建一条活动站会话：换 SSO token → `tokenValidate`。
//
// `deviceID` 是 `tokenValidate` body 那一格。它的出处是旧 TS 的 `ctx.state.deviceId` ——
// 与签到那一条要的 H5 `deviceId` **同一来源**（`hc1t/task.ts:283` 把上报换回的 id
// 存进 state，本仓 [DeviceID] 做的是同一件事）。⚠ "红包派对认的就是这一枚"未核实。
//
// # `tokenValidate` 那一步的 `token` 头带什么：**空串**，而且这不是猜
//
// 旧 TS `task.ts:119-124` 传的是 `getHeaders("")` ⇒ 真实客户端在这一枪上带的就是空 token
// （活动站票此刻还没换到）。夹具里那一位被遮 ⇒ 看得见的是"旧实现这么发过"，
// 看不见"上游认不认"。真打回非 200 时第一个要试的是换成 `ssoToken`
// （`upstream.CloudPhoneTokenValidateRequest` 的头注写的是同一句）。
func OpenCloudPhoneSession(
	ctx context.Context,
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	deviceID string,
	clock core.Clock,
	rng core.Random,
) (CloudPhoneSession, *core.Error) {
	ssoToken, perr := up.AppQuerySpecToken(ctx, profile,
		UserTimestamp(clock), credential.Authorization(), upstream.CloudPhoneSpecTokenSourceID)
	if perr != nil {
		return CloudPhoneSession{}, perr
	}

	// 第②步自己的那组头：同一套算式，但**票位是空串**（见函数头注第三条）。
	validate, perr := up.AppCloudPhoneTokenValidate(ctx, cloudPhoneAnonymousAuth(clock, rng),
		upstream.CloudPhoneRefererHomeURL, deviceID, ssoToken)
	if perr != nil {
		return CloudPhoneSession{}, perr
	}
	return CloudPhoneSession{
		token:   validate.Token,
		referer: upstream.CloudPhoneRefererHomeURL,
		xOrigin: upstream.CloudPhoneXOriginHomeURL,
	}, nil
}

// cloudPhoneAnonymousAuth 是"票还没换到"那一趟用的头素材（`token` 位为空串）。
//
// 单独成一个函数是因为这一格**容易被写成借用别的活动的票**：
// 换票那一枪带旧 token（本函数）与带 SSO token 是两种不同发法，混起来就看不出现在
// 打的是哪一枪。旧 TS 的那一条是空串，见 [OpenCloudPhoneSession]。
func cloudPhoneAnonymousAuth(clock core.Clock, rng core.Random) upstream.CloudPhoneAuth {
	now := clock.Now()
	milli := now.UnixMilli()
	timestamp := upstream.CloudPhoneTimestampMilli(milli)
	return upstream.CloudPhoneAuth{
		RequestID: upstream.CloudPhoneRequestID(now, milli, upstream.CloudPhoneRequestIDSuffix(rng)),
		Timestamp: timestamp,
		DeviceInfo: upstream.CloudPhoneDeviceInfo(timestamp,
			upstream.CloudPhoneDeviceInfoSuffix(rng)),
		XOrigin: upstream.CloudPhoneXOriginHomeURL,
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 持有者：一份票、一个懒建的开关、一个可断言的计数
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneSessionSource 是"(账号 × 红包派对页) 这一张票"的持有者。
//
// 作用域与 [SignInSessionSource] 同一口径（`AGENTS.md` §1.12：票属于账号 × 页面）：
// 经 [CloudPhoneSessionPool.Source] 拿到的是跨轮复用的那一条，
// 直接 [NewCloudPhoneSessionSource] 拿到的是"这一轮那一份"。
//
// 与签到那条的**一处不同**：这里没有"撞某个码就刷一次重放一次"的自救路径 ——
// 那一族的判据是 `90001`（实测过），本族的失效表现**零样本**（见文件头）。
// 所以 [cloudPhoneCall] 只做"确保有票，然后发"，失败原样交回。
type CloudPhoneSessionSource struct {
	up         *upstream.Upstream
	profile    *upstream.AppProfile
	credential *SignInCredential
	clock      core.Clock
	rng        core.Random

	// deviceID 是**懒**的：换票那一枪要它，而取它可能要上报设备指纹
	// （见 [DeviceID]）—— 没建成会话就不该花这一次出网。
	deviceID func(context.Context) (string, *core.Error)

	buildMu sync.Mutex
	mu      sync.Mutex
	session *CloudPhoneSession
	opens   int
}

// NewCloudPhoneSessionSource 建一条会话源。
//
// `deviceID` 传的是**取法**不是值：给函数才能做到"没要用就不取"。
// 传 nil ⇒ 换票时报错（不是回落成空串：空 `deviceId` 发出去的是一条确定不像客户端的请求）。
func NewCloudPhoneSessionSource(
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	clock core.Clock,
	rng core.Random,
	deviceID func(context.Context) (string, *core.Error),
) *CloudPhoneSessionSource {
	return &CloudPhoneSessionSource{
		up: up, profile: profile, credential: credential,
		clock: clock, rng: rng, deviceID: deviceID,
	}
}

// Session 返回这一条会话；还没有就现建一条（**这里才出网**）。
//
// 并发：两个 goroutine 同时进来只建一条（`buildMu` 串行化整段建会话，
// 与 [SignInSessionSource] 同一处置，理由也在那儿）。
func (s *CloudPhoneSessionSource) Session(ctx context.Context) (*CloudPhoneSession, *core.Error) {
	s.mu.Lock()
	if s.session != nil {
		session := s.session
		s.mu.Unlock()
		return session, nil
	}
	s.mu.Unlock()

	s.buildMu.Lock()
	defer s.buildMu.Unlock()
	return s.buildLocked(ctx)
}

func (s *CloudPhoneSessionSource) buildLocked(
	ctx context.Context,
) (*CloudPhoneSession, *core.Error) {
	s.mu.Lock()
	if s.session != nil {
		session := s.session
		s.mu.Unlock()
		return session, nil
	}
	s.mu.Unlock()

	if s.deviceID == nil {
		return nil, core.NewInternal("红包派对换票没接 deviceId 取法（不回落成空串）")
	}
	id, perr := s.deviceID(ctx)
	if perr != nil {
		return nil, perr
	}
	session, perr := OpenCloudPhoneSession(ctx, s.up, s.profile, s.credential, id, s.clock, s.rng)
	if perr != nil {
		return nil, perr
	}
	s.mu.Lock()
	s.opens++
	s.session = &session
	s.mu.Unlock()
	return &session, nil
}

// Opens 报告实际建过几次会话。存在的意义与 [SignInSessionSource.Opens] 相同：
// 没有这个计数，"跨轮复用同一张票"就只能靠读日志断言。
func (s *CloudPhoneSessionSource) Opens() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.opens
}

// cloudPhoneCall 取票后带着票跑 `f`；`f` 里的错误原样交回（**不重放**，理由见类型头注）。
//
// 形状与 [SignInSessionSource.Call] 对齐，是为了接线时两条链读起来是同一件事 ——
// 差别（不刷、不重放）写在上面那一段，而不是藏在实现里。
func cloudPhoneCall[T any](
	ctx context.Context, s *CloudPhoneSessionSource,
	f func(session CloudPhoneSession, auth upstream.CloudPhoneAuth) (T, *core.Error),
) (T, *core.Error) {
	session, perr := s.Session(ctx)
	if perr != nil {
		var zero T
		return zero, perr
	}
	return f(*session, session.AuthFor(s.clock, s.rng))
}

// ─────────────────────────────────────────────────────────────────────────────
// 票池：同账号跨轮复用同一张活动站票
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneSessionPool 是进程内的活动站票池：按 (账号, 凭据指纹) 持有会话源。
//
// 键里**没有配方**那一位（与 [SessionPool] 不同）：签到那一条今天要区分
// "签到页 / 小红花页"两种取法，而本族只有一个活动、一张票 ⇒ 现在放一位配方等于
// 凭空声称"这一族有多个页面各自的取法"。哪天红包派对页真的分岔，再加那一位。
type CloudPhoneSessionPool struct {
	mu    sync.Mutex
	items map[cloudPhonePoolKey]*CloudPhoneSessionSource
}

type cloudPhonePoolKey struct {
	account core.AccountID
	// authHash 是登录凭据的**指纹**（不是凭据本身），理由与 [SessionPool] 那一位相同：
	// 外圈凭据轮换之后旧会话源再也刷不出票，留着只会把失败读成"票刷不动"。
	authHash string
}

// NewCloudPhoneSessionPool 建一条空池。一个进程一份。
func NewCloudPhoneSessionPool() *CloudPhoneSessionPool {
	return &CloudPhoneSessionPool{items: map[cloudPhonePoolKey]*CloudPhoneSessionSource{}}
}

// Source 返回这一账号当前的活动站会话源；没有就现装一条并记住。
//
// ⚠ 与 [SessionPool.Source] 一样，**不发请求** —— 建会话是懒的。
func (p *CloudPhoneSessionPool) Source(
	up *upstream.Upstream, profile *upstream.AppProfile, credential *SignInCredential,
	clock core.Clock, rng core.Random,
	deviceID func(context.Context) (string, *core.Error),
) *CloudPhoneSessionSource {
	key := cloudPhonePoolKey{
		account:  credential.AccountID(),
		authHash: authFingerprint(credential.Authorization()),
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if src, ok := p.items[key]; ok {
		return src
	}
	// 外圈凭据换过 ⇒ 该账号名下任何一条旧会话源都刷不出新票，清掉（同 [SessionPool]）。
	for k := range p.items {
		if k.account == key.account && k.authHash != key.authHash {
			delete(p.items, k)
		}
	}
	src := NewCloudPhoneSessionSource(up, profile, credential, clock, rng, deviceID)
	p.items[key] = src
	return src
}

// Entries 是池里当前有几条会话源（测试与排障用；不含任何凭据）。
func (p *CloudPhoneSessionPool) Entries() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.items)
}

// cloudPhoneSessionSource 是"接了池就拿池里那条、没接就回落成轮级"那一句共用写法
// （与 [sessionSource] 同一形状、同一理由：CLI 单次调用与测试夹具不需要池）。
func cloudPhoneSessionSource(
	pool *CloudPhoneSessionPool, up *upstream.Upstream, profile *upstream.AppProfile,
	credential *SignInCredential, clock core.Clock, rng core.Random,
	deviceID func(context.Context) (string, *core.Error),
) *CloudPhoneSessionSource {
	if pool != nil {
		return pool.Source(up, profile, credential, clock, rng, deviceID)
	}
	return NewCloudPhoneSessionSource(up, profile, credential, clock, rng, deviceID)
}
