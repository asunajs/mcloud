package app

// **一个页面一张票，跨轮复用** —— 这一层管的只有"什么时候把同一张票再交出去一次"。
//
// # 为什么作用域是"页面"而不是"一轮"
//
// 上一轮把口径落成"一轮任务用同一个 `jwttoken`"（见 `signin_session.go` 头注）。
// 那只解决了一半：一趟之内的多个能力不再各建一条会话了，但**两次点击**之间还是各建各的 ——
// 用户在短窗口里连点三次签到，就会打三次 `tyrzLogin`，而真实客户端是"页面还开着，票就用到底"。
// 用户 2026-09-23 把作用域说清（原话口径）：
//
//   - 短期（**且已超过节流窗口**）内点三次签到 ⇒ 三次**共用同一张** `jwttoken`；
//   - 之后去点领取小红花 ⇒ 取**并使用小红花页面那张**；
//   - 再回头点领取云朵 ⇒ 带的**还是签到页那张**（没被小红花那一张顶掉）；
//   - ⚠ **WebUI 下连点三次签到也必须共用同一张**，不许频繁去请求新票。
//
// ⇒ 所以持有者必须比"一轮"活得久，而"页面"是它天然的键。
//
// # 为什么键里还要有"配方"，而不是直接写页面名
//
// 真正决定一条会话长什么样的是**换票那三枪用的参数**（`toSourceId`、referer 那个页面、
// `activityid` 那一族），不是页面上有个什么按钮。所以键叫 [SessionRecipe]：
//
//   - **今天它只有一个取值** [RecipeSignInPage] —— 抓包还没证明小红花页要按自己的参数换票，
//     于是那一条链**借用**签到页这一份（按 `AGENTS.md` §1.12 必须在调用点标注
//     "jwttoken 使用公共来源"，见 `live_room_task.go` / `live_room_probe.go`）；
//   - 抓包证明不同那天：**新增一个取值**，并让 [OpenSignInSession] 按配方取那三个参数，
//     然后把小红花那条链映射到新取值上 —— **键、池、刷新逻辑一行都不用改**。
//
// ⚠ 反过来，现在就凭"页面名不同"拆成两条会话，表现是每跑一次六步链**多打三枪**、
// 而两把票内容其实一模一样（没有特有取法可依据）。那不是"特有优先"，是空转。
//
// # 为什么不设 TTL（不设"到点主动重建"）
//
// 抓包只给了"这张票 2 小时即废"这一个数，**没有**给"该在哪个时刻算它过期"的依据
// （服务端有没有别的失效路径、时钟偏差怎么算，全未实测）。所以失效判据仍然只有
// 上游回的那一句 `90001` —— 与 [SignInSessionSource] 同一取向（那里也刻意没自造 TTL）。
//
// 代价要说清楚：票**自然过期**后的第一次点击会先撞一次 `90001`，再由
// [SignInSessionSource.Call] 刷一次、重放那一条 ⇒ 用户看到的是"多等一次往返"，不是失败。
// 反过来如果写死 2 小时重建，那是拿客户端猜测替换服务端语义（参考项目 caiyun 的毛病，
// `docs/caiyun-go-exchange-and-runtime.md` §4.4 已点名不搬）。
//
// # 键里那第三位（登录凭据指纹）为什么不能省
//
// [SignInSessionSource] 里揣着 `credential`：外圈 `Authorization` 一旦轮换（刷新那条路），
// 旧的 source 再建会话必然失败，而失败会被读成"这张票刷不动"。所以键带上凭据指纹，
// 并且**同账号旧指纹的条目在建新条目时一并清掉**（不保留 = 不泄漏、也不会被下次误用）。
// 存的是 SHA-256 前 8 字节的 hex，**不是**凭据本身 —— 这个结构会进日志与测试输出。

import (
	"crypto/sha256"
	"encoding/hex"
	"sync"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// SessionRecipe 是"这一页按什么方式换票"的身份。
//
// ⚠ 它是**配方编号**，不是页面名：两个页面映射到同一个取值，意思是
// "目前按同一套参数换票（公共来源）"，不是"这两个页面是同一个页面"。
type SessionRecipe string

// RecipeSignInPage 是签到页那一套换票参数（`toSourceId=001005` → `/ycloud/api/timestamp` →
// `tyrzLogin`，referer 是签到页）。
//
// 今天**所有**需要 `jwttoken` 的链都映射到它，包括小红花那两条 ——
// 那是 `AGENTS.md` §1.12 的临时态，不是终判：抓到该页特有取法那天，
// 新增取值、把那条链改指过去、并**同时**改掉调用点那句"使用公共来源"。
const RecipeSignInPage SessionRecipe = "signin-page"

// SessionPool 是进程内的会话票池：按 (账号, 配方, 凭据指纹) 持有 [SignInSessionSource]。
//
// 生命周期就是进程：不入库（这张票 2 小时即废，而库里那一列装的是**登录凭据**，
// 同名不同物，见 `signin.go` 包注释）。并发安全：[SessionPool.Source] 只做map 读写，
// 真正的三枪网络在 [SignInSessionSource] 自己的 `buildMu` 里，不在这里。
type SessionPool struct {
	mu    sync.Mutex
	items map[sessionPoolKey]*SignInSessionSource
}

type sessionPoolKey struct {
	account core.AccountID
	recipe  SessionRecipe
	// authHash 是登录凭据的指纹（非可逆），存在的理由见文件头那一条。
	authHash string
}

// NewSessionPool 建一条空池。一个进程（服务端 / 一次 CLI 调用）一份。
func NewSessionPool() *SessionPool {
	return &SessionPool{items: map[sessionPoolKey]*SignInSessionSource{}}
}

// Source 返回这一 (账号, 页面) 当前那条会话源；池里没有就现装一条并记住。
//
// ⚠ 它**不发请求** —— 建会话仍然是懒的（[SignInSessionSource.Session] 第一次要票时才打三枪）。
// 所以"取了一次没用"不花任何代价，调用点不必自己判"这轮到底要不要票"。
func (p *SessionPool) Source(
	up *upstream.Upstream, profile *upstream.AppProfile, credential *SignInCredential,
	clock core.Clock, rng core.Random, recipe SessionRecipe,
) *SignInSessionSource {
	key := sessionPoolKey{
		account:  credential.AccountID(),
		recipe:   recipe,
		authHash: authFingerprint(credential.Authorization()),
	}
	p.mu.Lock()
	defer p.mu.Unlock()

	if src, ok := p.items[key]; ok {
		return src
	}
	// 建新的一条之前，先把**同账号**旧凭据那几条清掉：它们再也刷不出票（外圈凭据已换），
	// 留着只会让下一次误用变成一句"刷新失败"。
	p.dropAccountExcept(key.account, key.authHash)

	src := NewSignInSessionSource(up, profile, credential, clock, rng)
	src.recipe = recipe
	p.items[key] = src
	return src
}

// Entries 是池里当前有几条会话源（测试与排障用；不含任何凭据）。
func (p *SessionPool) Entries() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.items)
}

// dropAccountExcept 清掉该账号下**凭据指纹不是 `authHash`** 的那些条目（连带配方不同的也清 ——
// 换过外圈凭据之后，那个账号名下任何一条旧会话都刷不出新票，没有留着的价值）。
func (p *SessionPool) dropAccountExcept(account core.AccountID, authHash string) {
	for key := range p.items {
		if key.account == account && key.authHash != authHash {
			delete(p.items, key)
		}
	}
}

// sessionSource 是三条入口共用的那一句"拿票"：接了池就拿池里那一条（跨轮复用），
// 没接池就回落成"这一轮那一条"。
//
// 为什么要留回落：CLI 一次调用只跑一轮、测试夹具想钉住"这一趟的请求序列"，
// 两边都不需要池；把池做成必填参数会让那两侧为了装配而各造一份空池。
// ⚠ 服务端那条路**必须**接池 —— 那里"三次点击"就是三轮，漏接的表现是
// 每次都重新打三枪建会话，而那正是用户 2026-09-23 点名不许的样子。
func sessionSource(
	pool *SessionPool, up *upstream.Upstream, profile *upstream.AppProfile,
	credential *SignInCredential, clock core.Clock, rng core.Random, recipe SessionRecipe,
) *SignInSessionSource {
	if pool != nil {
		return pool.Source(up, profile, credential, clock, rng, recipe)
	}
	return NewSignInSessionSource(up, profile, credential, clock, rng)
}

// authFingerprint 取登录凭据的短指纹。
//
// ⚠ 只可能取指纹：这个键会出现在测试失败输出里（`%v` 一个 map），
// 放原文就等于把整条可冒充登录的凭据打进日志（同 [SignInCredential.String] 里那条约束）。
func authFingerprint(authorization string) string {
	if authorization == "" {
		// 空串不是"没登录"，是**这一路压根没带凭据**（测试夹具常见）。
		// 不特判的话它会与"另一个也没带的"共用一条会话，那正是这里要避免的复用。
		return "<empty>"
	}
	sum := sha256.Sum256([]byte(authorization))
	return hex.EncodeToString(sum[:8])
}
