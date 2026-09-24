package app

// 同一页、同一轮用**同一张** `jwttoken`，以及它唯一的一条自救路径：
// 命中 `90001` ⇒ 刷新会话一次 ⇒ 用新 token 把**那一条请求**重放一次 ⇒ 仍失败就交回调用方按任务失败处理。
//
// # 为什么要有这个类型（而不是每个能力各自 [OpenSignInSession]）
//
// 改之前：12 个建会话的调用点，每个自己开一条、用完就丢，一次"签到 + 顺手领取"要建两条
// （见 `signin_run.go` 里"领取要再建一次会话"那段），一轮多能力就是 N×3 枪。
// 上游对同一账号短窗口内反复 `tyrzLogin` 会怎么看，**未实测**；收敛成一轮一条至少让
// 请求序列更像"一次会话里连着做几件事"的那个真实客户端。
//
// # 四条口径（2026-09-23 用户拍板；第 1 条同日由"一轮"扩成"一页"）
//
//  1. **同一页用同一个 jwttoken** —— 作用域是 **(账号, 页面配方)**，比"一轮"活得久：
//     同一页短窗口内连点三次 ⇒ 只建一条会话（跨轮复用由 [SessionPool] 持有，
//     那里写了"为什么键是配方而不是页面名"与"为什么不设 TTL"）；
//     任务归属哪个页面**未知**时，也共享默认那一条（就是签到页那一套参数，
//     见 `upstream.MarketName` 的说明，并按 `AGENTS.md` §1.12 标成"借签到页 scene"的临时态）。
//     一轮之内当然还是这一条 —— 那是本条的**下界**，不是全部。
//  2. **`90001` 触发刷新**：判据是变体 [core.Error.IsSessionExpired]，**不是**消息文本 ——
//     提示串改一个字就会让重放静默失效。
//  3. **刷新后重放一次，读写一视同仁**；重放仍失败 ⇒ 原样交回，由调用方判任务失败。
//     ⚠ 写请求（`startSignIn` / `receiveV3` / 兑换）重放的后果**未实测**：
//     服务端对"同一天重复签到"实测是幂等的（`docs/capture/app-signin-samples.txt` §3.2，
//     已签状态下真实客户端也调了它），但**领取类**是否幂等没有任何样本 ——
//     要核它得在活窗口里拿一个号连发两次同一笔 `receiveV3` 并比对云朵余额。
//  4. **一张票只属于一个配方**：小红花那一条链刷新了自己的票，**不许**把签到页那张一起顶掉
//     （用户 2026-09-23 的场景口径："去点小红花 → 回头点签到，仍带签到页那张"）。
//     今天两页共用一份配方 ⇒ 池里其实只有一条，这条要等抓包把它分开才看得出差额，
//     所以它由 [SessionPool] 的测试钉住（键不同 ⇒ 条目不同、刷新互不影响）。
//
// # 不做的事
//
//   - **不落库**：这张票 2 小时即废，而 `account_state.jwt_token` 装的是**登录凭据**
//     （同名不同物，见 `signin.go` 包注释）。落库等于给一张短期票开一条长期复用路径。
//   - **不自造 TTL**：失效判据只有服务端回的 `90001`。参考项目 caiyun 那套
//     "内存寿命硬编码 `now+15min`、与真实 `exp` 无关"是把服务端语义换成自己的猜测
//     （`docs/caiyun-go-exchange-and-runtime.md` §4.4），不搬。
//   - **不限刷几次**：`90001` 每次出现都刷一次 + 重放一次，这是上面第 3 条的原样落地。
//     "整轮系统性失效会不会刷 N 次"是**待验证**项（怎么测：拿一个必回 90001 的坏会话打一轮
//     多能力，数 `tyrzLogin` 的次数）；要加闸门得先有那个数，而不是先假设上游会腻。

import (
	"context"
	"strconv"
	"sync"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// SignInSession 是一条已建好的 H5 会话：`referer` 与 `jwttoken` **成对**使用。
//
// 两者必须同源 —— [OpenSignInSession] 的注释说过"后面每个请求都要带同一个 referer"，
// 换 token 不换 referer（或反之）的表现是"凭据都在、却回 90001"，看起来像凭据坏了。
// 所以这里把它们捆成一个值，不给调用方摆错顺序的机会（两个裸 string 参数是摆错了也不报错的）。
//
// ⚠ 刻意**不**实现 `String()` / 也不暴露 `%+v` 之外的便捷打印：`jwt` 是可直接复用的凭据，
// 而 Go 的 `%v` 会连未导出字段一起打出来 —— 需要描述时用 [SignInSession.Describe]。
type SignInSession struct {
	referer string
	jwt     string
}

// Describe 返回可安全打印的形态：只给长度，不给内容。
func (s SignInSession) Describe() string {
	return "SignInSession{referer: <" + strconv.Itoa(len(s.referer)) + " 字符>" +
		", jwttoken: <" + strconv.Itoa(len(s.jwt)) + " 字符>}"
}

// same 报告两条会话是否是同一条（按**内容**比，不是按指针）。
func (s SignInSession) same(other SignInSession) bool {
	return s.referer == other.referer && s.jwt == other.jwt
}

// SignInSessionSource 是一个作用域内的 jwttoken 持有者。
//
// 作用域由**谁持有它**决定：经 [SessionPool.Source] 拿到的是"(账号, 页面) 这一份"，
// 跨轮复用；直接 [NewSignInSessionSource] 拿到的是"这一轮那一份"（CLI 单次调用、
// 测试夹具，以及没接池的路径的回落）。两种用法的**自救口径完全相同**（90001 → 刷一次 → 重放一次）。
//
// 生命周期都在进程内：不落库、不做 TTL，[core.NewSessionToken] 那种"存哈希"
// 的纪律在这里用不上（它压根不进库）。并发用法：同一实例可以在多个 goroutine 上安全使用
// （[SignInSessionSource.Call] 期间不持锁）。
type SignInSessionSource struct {
	up         *upstream.Upstream
	profile    *upstream.AppProfile
	credential *SignInCredential
	clock      core.Clock
	rng        core.Random

	// buildMu 串行化"建会话"这一段：两个 goroutine 同时进来只建一条
	// （不串行化的代价是 2 次 `tyrzLogin` + 其中一条票被丢掉，而票 2 小时才废 ——
	//  不报错，只是白建）。
	// ⚠ 三枪网络请求就**在这把锁里面**跑，这是刻意的：建一条会话本来就是这段临界区
	//  要做的事，把它挪到锁外会退化成"谁先到谁建、后面的人再判一遍"，而那条被抢建的
	//  票照样发不出去。真正**不**持锁的是 [SignInSessionSource.Call] 里 `f` 那一段
	//  —— 业务请求可以并发，只有建会话排队。
	buildMu sync.Mutex
	// mu 只保护 session / opens / recipe 这几个字段，绝不跨网络持有。
	mu      sync.Mutex
	session *SignInSession
	// opens 是实际建过几次会话，[SignInSessionSource.Opens] 读它。
	// 存在的意义是让它**可断言**：没有这个计数，"一轮只建一次"就只能靠读日志。
	opens int
	// recipe 是"这一条会话是按哪一页的方式换出来的"，由 [SessionPool.Source] 装上。
	// 只用于排障与测试断言（[SignInSessionSource.Recipe]）—— 网络路径不看它。
	recipe SessionRecipe
}

// NewSignInSessionSource 建一条轮级会话源。
//
// 参数就是 [OpenSignInSession] 那五个 —— 不做任何默认值：档案、凭据、时钟、随机源
// 每一样在这里缺失都会变成"拼出来的请求不像任何真实客户端"，宁可编译不过。
func NewSignInSessionSource(
	up *upstream.Upstream,
	profile *upstream.AppProfile,
	credential *SignInCredential,
	clock core.Clock,
	rng core.Random,
) *SignInSessionSource {
	return &SignInSessionSource{
		up: up, profile: profile, credential: credential, clock: clock, rng: rng,
	}
}

// Opens 是到当前为止实际建过几次会话（`tyrzLogin` 打了几次）。
func (s *SignInSessionSource) Opens() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.opens
}

// Recipe 是这一条源所属的页面配方；空串 = 不经 [SessionPool] 直接建的（轮级用法）。
func (s *SignInSessionSource) Recipe() SessionRecipe {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.recipe
}

// Session 返回这一轮当前那条会话；还没有就现建一条并记住。
func (s *SignInSessionSource) Session(ctx context.Context) (SignInSession, *core.Error) {
	if current, ok := s.cached(); ok {
		return current, nil
	}
	return s.build(ctx)
}

// Refresh 丢掉 `stale` 那条（如果它还是当前这条）并重建一条。
//
// `stale` 是**刚刚失败的那一条**，给它是因为并发：两条请求各自撞上 `90001` 时，
// 后进场的那个如果看到"当前已经不是它失败的那条了"，就直接复用别人刚建好的新会话 ——
// 否则一次会话过期能刷出 N 条新会话，而那 N 条里只有一条会被用上。
func (s *SignInSessionSource) Refresh(
	ctx context.Context, stale SignInSession,
) (SignInSession, *core.Error) {
	s.buildMu.Lock()
	defer s.buildMu.Unlock()

	if current, ok := s.cached(); ok && !current.same(stale) {
		// 别人已经换过了：这次不用重建，直接用新的。
		return current, nil
	}
	s.drop(stale)
	return s.buildLocked(ctx)
}

// Call 用当前会话跑一次 `f`；只在 `f` 回 `90001 未登录` 时刷新并重放**一次**。
//
// 返回非 nil 的三种情况，调用方按"任务失败"处理即可，它们的成因完全不同：
//
//   - 建会话自己就失败（三枪里的任一枪）—— 这张票压根没拿到；
//   - 刷新失败（重开一条会话时失败）—— 错误串里明写"jwttoken 刷新失败"，
//     这是给用户看的那一句（口径来自用户 2026-09-23 的要求：以 jwttoken 失败通知）；
//   - 重放后仍失败 —— 带 `jwttoken 刷新后重放仍失败:` 前缀，用来和"第一次就失败"分开。
//
// ⚠ 这里**不**按 kind 而按变体判 `90001`（[core.Error.IsSessionExpired]）：
// 上游的 `614` / `615` 与 `11029` 消息长得几乎一样（都在说"火爆"），
// 而前两者是参数问题、刷新一万次也没用（见 `upstream.CodeReceiveFailedAlt` 那段实测）。
func (s *SignInSessionSource) Call(
	ctx context.Context, f func(SignInSession) *core.Error,
) *core.Error {
	session, err := s.Session(ctx)
	if err != nil {
		return err
	}
	first := f(session)
	if first == nil {
		return nil
	}
	if !first.IsSessionExpired() {
		// 不是"会话过期"这一类：不刷新、不重放，原样交回。
		// 这条分支很重要 —— 把它写成"任何错都刷一次再来"，等于给参数错开一次
		// 重复的**写**请求，而那正是第 3 条口径里唯一被实拍证明幂等的场景之外的事。
		return first
	}

	fresh, rerr := s.Refresh(ctx, session)
	if rerr != nil {
		return core.NewSessionExpired(
			"jwttoken 刷新失败（原错误 " + rerr.Error() + "），本次请求判失败；" +
				"通常是这一轮的登录态或设备档案出了问题，先看原错误")
	}
	if second := f(fresh); second != nil {
		return second.WithPrefix("jwttoken 刷新后重放仍失败: ")
	}
	return nil
}

// do 是能力函数那一侧的那层糖：**拿这一轮的会话跑一次请求**，
// 失败时按口径 2/3 刷一次、重放一次。
//
// 为什么要这层糖：[SignInSessionSource.Call] 的 `f` 只回错误，而每个能力函数
// 都要把成功那次的结果带出去。不封这层，11 个函数各写一遍"`var out T` + 闭包里赋值"，
// 而漏掉"失败时把 out 归零"的代价是调用点拿到一份**上一次失败尝试的残值** ——
// 那是"看起来成功了一半"的最坏形态。
func do[T any](
	ctx context.Context, src *SignInSessionSource,
	f func(SignInSession) (T, *core.Error),
) (T, *core.Error) {
	var out T
	err := src.Call(ctx, func(s SignInSession) *core.Error {
		value, perr := f(s)
		if perr != nil {
			return perr
		}
		out = value
		return nil
	})
	if err != nil {
		var zero T
		return zero, err
	}
	return out, nil
}

// cached 读当前会话。第二个返回值是"有没有"，**不是**"对不对" ——
// 过期的票在这里看不出来，只能问上游（见 [SignInSessionSource.Call]）。
func (s *SignInSessionSource) cached() (SignInSession, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.session == nil {
		return SignInSession{}, false
	}
	return *s.session, true
}

// drop 在"确认当前就是失败的那一条"之后把它清掉。
func (s *SignInSessionSource) drop(stale SignInSession) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.session != nil && s.session.same(stale) {
		s.session = nil
	}
}

// build 拿建会话锁后走一次双检再建 —— 双检是防"排队等锁的人重复建"。
func (s *SignInSessionSource) build(ctx context.Context) (SignInSession, *core.Error) {
	s.buildMu.Lock()
	defer s.buildMu.Unlock()
	if current, ok := s.cached(); ok {
		return current, nil
	}
	return s.buildLocked(ctx)
}

// buildLocked 真正打那三枪。调用方必须持有 [SignInSessionSource.buildMu]。
//
// 建会话**失败也要留痕**：`opens` 只在成功时加一。失败三枪同样打了三次上游，
// 但"成功建了几条会话"才是这一族要断言的量（口径 1："一轮一个 jwttoken"）。
func (s *SignInSessionSource) buildLocked(ctx context.Context) (SignInSession, *core.Error) {
	referer, jwt, err := OpenSignInSession(
		ctx, s.up, s.profile, s.credential, s.clock, s.rng)
	if err != nil {
		return SignInSession{}, err
	}
	session := SignInSession{referer: referer, jwt: jwt}
	s.mu.Lock()
	s.session = &session
	s.opens++
	s.mu.Unlock()
	return session, nil
}
