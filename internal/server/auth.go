package server

// 会话与初始化：网络路径上的"你是谁"。
//
// # 两条规则
//
//  1. **没初始化就只暴露初始化**：库里一个用户都没有时，除健康检查与 `/api/setup`
//     外一律 `503 setup_required`。否则一个刚部署、还没建管理员的实例，
//     会把"添加账号""跑任务"这类接口暴露给任何一个扫到端口的人。
//  2. **其余接口一律要会话**：解析出 [AuthUser] 才继续，解析不出来 401。
//     处理器里再取 `core.ScopeUser(&auth.User.ID)` —— 数据隔离本身在存储层那份
//     过滤表达式里，这一层只负责"把请求映射成某个人"。
//
// # 为什么身份放在 request context 里
//
// 处理器再解析一次就是第二次查库（每个请求两次会话查询），而且会出现
// "中间件判过了、处理器忘了判"的空窗。放进 context 之后，处理器只能从
// [AuthFrom] 取 —— 拿不到就取不到，不会悄悄变成"匿名也能过"。
//
// # cookie 的三个属性都是必要的
//
//   - `HttpOnly`：JS 读不到 token（XSS 拿不走会话）；
//   - `SameSite=Lax`：跨站 POST 不带 cookie（挡掉最常见的 CSRF 形态），
//     同时保留"从外部链接点进来仍是已登录"的正常体验；
//   - `Path=/`：整站共用。
//
// `Secure` 由 [Config.SecureCookie] 决定，默认**关** —— 本地是
// `http://127.0.0.1`，开了它浏览器根本不会回传，表现为"登录成功但一直是未登录"。

import (
	"context"
	"net/http"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// CookieName 是会话 cookie 名。
const CookieName = "mcloud_session"

// Config 是服务端配置。
type Config struct {
	// SecureCookie 给 cookie 打 `Secure`（HTTPS 部署时开）。
	//
	// 默认关，理由见模块文档：本地 http 下打开它等于"登录永远不生效"。
	SecureCookie bool
}

// Server 是路由状态：它持有一次进程里**唯一**的那几样东西。
//
// ⚠ 事件总线住在这里而不是某个处理器里：它的生命周期**比一次请求长** ——
// 发布方是跑任务的那条路（[app.Runner]），订阅方是
// `GET /api/runs/{id}/events` 的 SSE —— 两者必须是**同一个实例**，
// 否则 SSE 永远看不到事件。
type Server struct {
	store  *mcloudio.SqliteStore
	clock  core.Clock
	rng    core.Random
	config Config
	bus    *mcloudexec.EventBus
	// hub 是登录会话表（扫码 / App 验证码）。
	//
	// 它与 [Server.bus] 同一条理由住在这里而不是处理器里：**生命周期比一次请求长** ——
	// 出码那次请求起了一轮后台轮询，读结论的是另外几次请求。
	hub *app.LoginHub
	// refresher 是"刷一个账号的凭据"那份常驻装配（见 [app.AuthRefresher]）。
	//
	// 它像 [Server.hub] 一样住在 Server 上而不是在处理器里现拼：拼它要一个
	// **注入的**出站 HTTP 客户端，而本包自己不造真实客户端 —— 那样"测试里漏注入
	// 假客户端"会表现成"真的打了上游"（而且刷新会把真凭据换掉），不是失败。
	refresher *app.AuthRefresher
	// runs 是"跑任务"那份常驻装配（见 [RunWorld]）。
	//
	// 第三个同一条理由住在这里的东西，而它多两分：除了出站客户端，它还得换掉
	// **WebSocket 拨号器**（收口令那一步开的是 WS，不是 HTTP），并且它持有
	// [mcloudexec.RunRegistry] —— 登记表必须跨请求，每次现拼一份的话
	// "同一账号不许并发跑"这一条守卫一句都不成立。
	//
	// ⚠ 它是 [New] 的必填项：留 nil 只可能是装配漏了一行，
	// 而那时 `POST /api/runs` 会回 500 而不是编译不过。
	runs *RunWorld
	// notify 是"推送"那一份常驻装配（见 [NotifyWorld]）。
	//
	// 第四个同一条理由住在这里的东西。它**可以**是 nil，而那不是漏装配：
	// 单独构造的 `Server`（测试夹具）本来就不该带一副能往第三方发消息的面孔。
	// nil 的表现是"推送这一族不响 + `POST /api/notify/test` 回 500"，
	// 而**不是**静默成功 —— 配置读写那两条只用库，与它无关，照常工作。
	notify *NotifyWorld
	// schedulerOn 记着调度循环在不在跑（`GET /api/status` 的那个字段）。
	//
	// 它是 [Server.startScheduler] 自己写的：起循环置真、退出置假（defer 那一条
	// 覆盖 ctx 结束与 panic 两种走法）。用 [atomic.Bool] 而不是普通 bool 是因为
	// 读它的是另一个 goroutine 里的 HTTP 请求。
	schedulerOn atomic.Bool
}

// New 组装服务端状态。
//
// 事件总线由本函数创建而不是让调用方传进来：总线**没有配置项**，
// 而多一个参数就多一种"两个处理器拿到不同总线"的接错法。
//
// `hub` 反过来**必须**由调用方给（见 [Run]）：它要一个出站 HTTP 客户端，
// 而本包自己不造真实客户端 —— 那样"测试里漏注入假客户端"会表现成
// "真的打了上游"（卡在超时上、不变红），而不是一个响亮的失败。
//
// 2026-09-22 补记：`refresher` 同一条理由，而它多一分风险 —— 刷新是**写库**并
// 轮换真凭据的动作，漏注入的后果不是慢，是库里那一套 `authorization` / `jwt_token`
// 被覆盖成另一套（窄写四列，见 `docs/auth-refresh.md` §4）。
// ⚠ 覆盖之后旧串还能不能用、会不会影响端上正登着的会话，**未实测**（同一篇 §8）。
//
// 同一条补记给 `runs`：它是"跑任务"那一份，出站面比前两个多一条 WS，
// 而且它必须与 [Server.bus] 配套 —— 事件的发布方与订阅方共用同一条总线，
// 登记表则决定"同一账号不许并发跑"，两者都跨请求，所以只能在这里递进来一次。
func New(
	db *mcloudio.SqliteStore, clock core.Clock, rng core.Random,
	cfg Config, hub *app.LoginHub, refresher *app.AuthRefresher, runs *RunWorld,
	opts ...ServerOption,
) *Server {
	s := &Server{
		store: db, clock: clock, rng: rng, config: cfg,
		bus: mcloudexec.NewEventBus(), hub: hub, refresher: refresher, runs: runs,
	}
	for _, opt := range opts {
		if opt != nil {
			opt(s)
		}
	}
	return s
}

// ServerOption 是[New]的可选装配项。
//
// 用变参而不是往参数表上再加第七个位置：`New` 已有三个调用点（进程入口 + 两份测试夹具），
// 而推送这一族**允许不接**（见 [Server.notify]）—— 摊进必填项就会逼每一个
// 只关心登录、跑任务的调用方多造一份用不到的装配。
type ServerOption func(*Server)

// WithNotifyWorld 接上推送的发送侧。
//
// 传 nil 与"根本不调这一条"是同一个结果（[Server.notify] 留空 ⇒ 这一族不响）。
// 它存在的意义不是开一个"关掉推送"的开关 —— 那个开关是配置里的 `enabled` ——
// 而是让组装根那一行（`run.go`）把"这里接上了出站发消息的能力"写得看得见。
func WithNotifyWorld(world *NotifyWorld) ServerOption {
	return func(s *Server) { s.notify = world }
}

// Store 是存储（组装层与处理器都从这里取）。
func (s *Server) Store() *mcloudio.SqliteStore { return s.store }

// Bus 是运行事件总线（发事件的跑任务侧与收事件的 SSE 侧共用这一份）。
func (s *Server) Bus() *mcloudexec.EventBus { return s.bus }

// Now 是当前时间（注入的时钟）—— 会话到期判定必须能被测试拨动。
func (s *Server) Now() time.Time { return s.clock.Now() }

// ─────────────────────────────────────────────────────────────────────────────
// 身份
// ─────────────────────────────────────────────────────────────────────────────

// AuthUser 是已认证的请求身份。
//
// ⚠ `Token` 是 cookie 里的**明文**，留着它只有一个用途：登出时交给
// [app.SignOutToken] 去走领域层那条"空 token 不许删"的守卫。
// 任何别的用法都会把明文传播出去 —— 需要身份时一律用 `User`，不要用它。
type AuthUser struct {
	User    core.User
	Session core.Session
	Token   string
}

type authKey struct{}

// withAuth 把身份放进 context。只有中间件该调它。
func withAuth(req *http.Request, auth *AuthUser) *http.Request {
	return req.WithContext(context.WithValue(req.Context(), authKey{}, auth))
}

// AuthFrom 取出中间件放进去的身份。
func AuthFrom(req *http.Request) (*AuthUser, bool) {
	auth, ok := req.Context().Value(authKey{}).(*AuthUser)
	return auth, ok && auth != nil
}

// authed 把"需要身份"的处理器包一层：取不到身份就是 401。
//
// 它是**第二道**闸（第一道是 [Server.requireSession]）。留两道的理由不是不信任
// 中间件，而是：中间件判断的是"这条路径要不要登录"，而这条路径列表将来会被改动；
// 缺了这一层，漏登记一条就等于把那个接口公开了，**且不会有任何测试变红**。
// 这一层由 `TestHandlersRefuseARequestWithoutAnIdentity` 直接钉住。
func (s *Server) authed(h func(http.ResponseWriter, *http.Request, *AuthUser)) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		auth, ok := AuthFrom(r)
		if !ok {
			ErrUnauthorized().Write(w)
			return
		}
		h(w, r, auth)
	}
}

// current 解析请求里的会话。
//
// ctx 取自请求本身：客户端断开/上游超时要能传进那次 DB 查询，
// 塞 context.Background() 等于每笔未完成的会话解析都成了孤儿查询。
func (s *Server) current(ctx context.Context, headers http.Header) (*AuthUser, *core.Error) {
	token := cookieToken(headers)
	if token == "" {
		return nil, nil
	}
	user, session, perr := app.CurrentByToken(ctx, s.store, token, s.Now())
	if perr != nil {
		return nil, perr
	}
	if user == nil || session == nil {
		return nil, nil
	}
	return &AuthUser{User: *user, Session: *session, Token: token}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 中间件
// ─────────────────────────────────────────────────────────────────────────────

// isPublic 判断免登录的端点。
//
// 用**白名单**而不是黑名单：以后新加一条路由时，默认是"需要登录"。
// 反过来的话，漏登记一条就等于把那个接口公开了。
func isPublic(method, path string) bool {
	if path == "/api/health" || path == "/api/setup" {
		return true
	}
	// 登录（POST）必须免登录，否则是个死结
	return method == http.MethodPost && path == "/api/session"
}

// isSetupAllowed 判断未初始化时仍放行的路径。
func isSetupAllowed(path string) bool {
	return path == "/api/health" || path == "/api/setup"
}

// requireSession 是"其余接口一律要会话"那一条。
func (s *Server) requireSession(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if isPublic(r.Method, r.URL.Path) {
			next.ServeHTTP(w, r)
			return
		}
		auth, perr := s.current(r.Context(), r.Header)
		if perr != nil {
			ErrDomain(perr).Write(w)
			return
		}
		if auth == nil {
			ErrUnauthorized().Write(w)
			return
		}
		next.ServeHTTP(w, withAuth(r, auth))
	})
}

// requireSetup 是"没初始化就只暴露初始化"那一条。
//
// ⚠ 查库失败时**不给结论**，让后面的层去报错 ——
// 把"读不到用户数"当成"已初始化"等于在最需要保守的时刻放行。
//
// ⚠⚠ 那个 `err == nil` 是**不可测**的（§6.1 #59 那一类）：测试用的是真 SQLite，
// 造不出"CountUsers 失败"这个分支，所以删掉它**不会让任何测试变红**
// （反向测试已确认）。留着的理由是它决定了失败时的**方向** ——
// 这一层的默认应该是"保守"，而默认往往正是没人测的那一支。
// ⇒ 别因为"没有红过"就以为它不重要，也别因为写了它就以为它被断言了。
func (s *Server) requireSetup(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		count, err := s.store.CountUsers(r.Context())
		if err == nil && count == 0 && !isSetupAllowed(r.URL.Path) {
			ErrSetupRequired().Write(w)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// Handler 组装路由。
//
// 静态前端产物**不在这里**：那是二进制启动时才知道的目录（见 [App]），
// 路由本身不该依赖文件系统。
//
// 中间件顺序：`requireSetup` 在外层，先跑。
func (s *Server) Handler() http.Handler {
	return s.requireSetup(s.requireSession(s.routes()))
}

// routes 是**裸路由表**（不带任何中间件）。
//
// ⚠ 单独抽出来是给测试用的，且必须是这样：路由表的"是否逐条对位冻结基准"
// **只能在这个粒度上测**。从 HTTP 面测不出来 —— 会话闸包在整个 mux 外层，
// 于是"未登录"对所有路径都是 401；而兜底模式 `/api/` 不带方法，
// 会把"方法不匹配"与"路径不存在"**一并吞成 404**（实测：全程没有 405、没有
// `Allow` 头）。也就是说 `GET /api/runs/1/events`（路由在、资源不在）与
// `/api/nope`（路由不在）在 HTTP 面上**长得一模一样**。
//
// 只有问 `ServeMux` "这条请求命中了哪个模式"才能把两者分开。
func (s *Server) routes() *http.ServeMux {
	mux := http.NewServeMux()

	mux.HandleFunc("GET /api/health", s.health)
	mux.HandleFunc("GET /api/setup", s.setupStatus)
	mux.HandleFunc("POST /api/setup", s.setupCreate)
	mux.HandleFunc("POST /api/session", s.login)
	mux.HandleFunc("GET /api/session", s.authed(s.currentSession))
	mux.HandleFunc("DELETE /api/session", s.authed(s.logout))

	s.registerAPIRoutes(mux)

	// 兜底：`/api` 下不存在的路径必须**明确报 404**，不能落到前端 index.html ——
	// 那会让"路径写错"表现成"返回了一段 HTML"，前端解析失败且毫无线索。
	//
	// ⚠ 两条都要：`"/api/"` 是子树模式（管 `/api/x`），而 `/api` 本身
	// **不**在它里面 —— Go 的 `ServeMux` 会先给 `/api` 一个 301 到 `/api/`。
	// 那个 301 是 Go 的产物，不是我们的契约（Rust 侧这里就是 404），
	// 所以显式补一条精确匹配，让两种写法给出**同一个**错误形状。
	mux.HandleFunc("/api/", s.apiNotFound)
	mux.HandleFunc("/api", s.apiNotFound)

	return mux
}

// registerAPIRoutes 挂业务路由（能力清单 / 账号 / 运行 / 定时任务）。
//
// 它们自带"要会话"的前提：每一条都过 [Server.authed]，
// 于是"哪条路径免登录"仍然只有 [isPublic] 一处判断。
func (s *Server) registerAPIRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/version", s.authed(s.getVersion))
	mux.HandleFunc("GET /api/capabilities", s.authed(s.getCapabilities))
	mux.HandleFunc("GET /api/status", s.authed(s.getStatus))

	mux.HandleFunc("GET /api/accounts", s.authed(s.listAccounts))
	mux.HandleFunc("GET /api/accounts/{id}/cloud-stats", s.authed(s.cloudStats))
	// 直播小红花页的第一屏：那一号**现在**有没有那场直播、有多少朵。
	// 只读，但**会打上游**（五枪，见 [Server.probeLiveRoom] 的注释），
	// 所以它与 `refresh-auth` 一样共用 [Deps.HTTP] 那一道闸，测试里漏注入就是真出门。
	mux.HandleFunc("GET /api/accounts/{id}/live-room", s.authed(s.probeLiveRoom))
	// 兑换页的第 ③ 块：这一号**此刻**看得见的清单，以及每一件挡在哪一条判据上。
	// 同样只读、同样会打上游（一枪 `exchangeList` + 建会话那三枪），
	// 所以也走 [Deps.HTTP] 那一道闸。形状与"为什么不叫 `/api/exchange/prizes`"
	// 写在 `exchange_prizes.go` 文件头。
	mux.HandleFunc("GET /api/accounts/{id}/exchange-prizes", s.authed(s.exchangePrizes))
	// 账号的三条写面。凭据**只进不出**：这三条的响应里没有 `authorization`
	// 与 `jwtToken` 这两个键（出网形状见 [accountPayload]）。
	mux.HandleFunc("POST /api/accounts", s.authed(s.addAccount))
	mux.HandleFunc("PUT /api/accounts/{id}/enabled", s.authed(s.setAccountEnabled))
	mux.HandleFunc("DELETE /api/accounts/{id}", s.authed(s.deleteAccount))
	// 第四条写面：换 token。它与上面三条同一套纪律（凭据只进不出），
	// 但**会打上游** —— 所以它的出站客户端与登录那几条共用 [Deps.HTTP] 那一道闸。
	mux.HandleFunc("POST /api/accounts/{id}/refresh-auth", s.authed(s.refreshAccountAuth))

	mux.HandleFunc("GET /api/runs", s.authed(s.listRuns))
	mux.HandleFunc("GET /api/runs/{id}/events", s.authed(s.runEvents))
	// 触发执行。它**不是**冻结基准里的路由（基准自己写着"要等 P4"），
	// 是本仓库新增的那一组之一 —— 所以登记在 routes_test.go 的 `goAdded` 表里，
	// 而不是那张 15 条的对位表里。
	mux.HandleFunc("POST /api/runs", s.authed(s.triggerRun))

	// 登录两条路。它们**不是**免登录接口：这里的"登录"指的是登录**移动云盘账号**，
	// 而操作者必须先是本服务的已登录用户 —— 否则任何人都能拿这台服务当发码代理。
	mux.HandleFunc("POST /api/login/qr", s.authed(s.startQRLogin))
	mux.HandleFunc("GET /api/login/qr/{id}", s.authed(s.loginSessionView))
	mux.HandleFunc("GET /api/login/qr/{id}/image.svg", s.authed(s.qrImage))
	mux.HandleFunc("POST /api/login/sms/app", s.authed(s.sendAppLoginCode))
	mux.HandleFunc("POST /api/login/sms/app/{id}/verify", s.authed(s.verifyAppLoginCode))
	mux.HandleFunc("POST /api/login/sms/pc", s.authed(s.sendPcLoginCode))
	mux.HandleFunc("POST /api/login/sms/pc/{id}/verify", s.authed(s.verifyPcLoginCode))

	// 任务组合这一族：清单读写 + 那枚"一键全跑"。四条都**不是**冻结基准里的路由
	// （那边没有 pipelines 的对外面），所以同样登记在 routes_test.go 的 `goAdded` 表里。
	//
	// `run` 与 `POST /api/runs` 同族（受理即返回 202），差别只在它**给不出** runId 配对
	// —— 一次组合有十七八步，后面的 runId 要等前面的跑完才分配得出来（判据见
	// [Server.runPipelineRoute]）。
	mux.HandleFunc("GET /api/pipelines", s.authed(s.listPipelines))
	mux.HandleFunc("PUT /api/pipelines/{id}", s.authed(s.putPipeline))
	mux.HandleFunc("DELETE /api/pipelines/{id}", s.authed(s.deletePipeline))
	mux.HandleFunc("POST /api/pipelines/{id}/run", s.authed(s.runPipelineRoute))

	mux.HandleFunc("GET /api/schedules", s.authed(s.listSchedules))
	mux.HandleFunc("PUT /api/schedules/{id}", s.authed(s.putSchedule))
	mux.HandleFunc("DELETE /api/schedules/{id}", s.authed(s.deleteSchedule))

	// 推送这一族。三条都**不是**冻结基准里的路由（那边没有这一族），
	// 所以同样登记在 routes_test.go 的 `goAdded` 表里。
	//
	// ⚠ 读写的是**全安装一份**的 `notify_config`（单行表、无归属），而 GET 的响应
	// 是脱敏视图：各家凭据只出"有没有填 + 字符数"（判据与那张字段表见 `notify_view.go`）。
	mux.HandleFunc("GET /api/notify/config", s.authed(s.getNotifyConfig))
	mux.HandleFunc("PUT /api/notify/config", s.authed(s.putNotifyConfig))
	// 试发**会往站外发消息**（用户手机上就多一条），所以它单列一条 POST，
	// 而不是在 `PUT /api/notify/config` 里顺手带一个 `test: true` 标志 ——
	// 后者会让"我只改了个标题"也可能变成"十家各收到一条测试"。
	mux.HandleFunc("POST /api/notify/test", s.authed(s.testNotify))
}

// ─────────────────────────────────────────────────────────────────────────────
// 处理器：健康与初始化
// ─────────────────────────────────────────────────────────────────────────────

func (s *Server) health(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "name": core.ProductName})
}

// apiNotFound 是 `/api` 下的未知路径：用**同一个错误形状**报 404。
func (s *Server) apiNotFound(w http.ResponseWriter, r *http.Request) {
	ErrDomain(core.NewNotFound("没有这个接口: " + r.URL.Path)).Write(w)
}

func (s *Server) setupStatus(w http.ResponseWriter, r *http.Request) {
	users, err := s.store.CountUsers(r.Context())
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	// 顺带把口令长度下限**发给前端**：那是 core.MinPasswordLen 定的策略，
	// 前端硬编一份必然与它漂移，而漂移的表现是"前端说可以、后端拒绝"。
	writeJSON(w, http.StatusOK, map[string]any{
		"needsSetup":     users == 0,
		"minPasswordLen": core.MinPasswordLen,
	})
}

func (s *Server) setupCreate(w http.ResponseWriter, r *http.Request) {
	body, ok := decodeJSON[credentials](w, r)
	if !ok {
		return
	}
	ctx := r.Context()

	// 走的是 CLI `mcloud user init` **同一个用例**：同一张表、同一套校验
	// （用户名规则、口令长度、argon2id 参数）、同一个"无主数据认领"。
	// 这里只多了一件事：顺手把会话发出去，免得用户刚设完口令又被要求输一遍。
	outcome, perr := app.InitFirstAdmin(ctx, s.store, body.Name, body.Password, s.rng, s.Now())
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	user, token, session, perr := app.SignIn(ctx, s.store, body.Name, body.Password, s.rng, s.Now())
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	s.writeWithSession(w, http.StatusCreated, token, session, map[string]any{
		"user": userPayload(user),
		"claimed": map[string]any{
			"accounts":  outcome.Claimed.Accounts,
			"schedules": outcome.Claimed.Schedules,
			"pipelines": outcome.Claimed.Pipelines,
		},
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 处理器：会话
// ─────────────────────────────────────────────────────────────────────────────

type credentials struct {
	Name     string `json:"name"`
	Password string `json:"password"`
}

func (s *Server) login(w http.ResponseWriter, r *http.Request) {
	body, ok := decodeJSON[credentials](w, r)
	if !ok {
		return
	}
	user, token, session, perr := app.SignIn(r.Context(), s.store, body.Name, body.Password, s.rng, s.Now())
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	s.writeWithSession(w, http.StatusOK, token, session, map[string]any{"user": userPayload(user)})
}

func (s *Server) currentSession(w http.ResponseWriter, _ *http.Request, auth *AuthUser) {
	writeJSON(w, http.StatusOK, map[string]any{
		"user":      userPayload(auth.User),
		"expiresAt": auth.Session.ExpiresAt.UTC().Format(time.RFC3339),
	})
}

// logout 撤销自己这个会话 —— **立刻生效**（删掉那一行，不是等它到期）。
//
// ⚠ 走 [app.SignOutToken] 而不是直接 `store.DeleteSession(TokenHash)`：
// 前者带着"空 token 不许删"那条守卫（删空串的哈希会命中一个真实存在的行，
// 等于"没带 cookie 的人登出了别人"）。绕过它省下的只是几次哈希运算，
// 而代价是把那条守卫变成"只在 CLI 那条路上有效"。
func (s *Server) logout(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	if _, perr := app.SignOutToken(r.Context(), s.store, auth.Token); perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	w.Header().Set("Set-Cookie", cookieHeader("", 0, s.config.SecureCookie))
	w.WriteHeader(http.StatusNoContent)
}

// ─────────────────────────────────────────────────────────────────────────────
// 形状与 cookie
// ─────────────────────────────────────────────────────────────────────────────

// userPayload 是用户的**对外形状**。
//
// 手写而不是给 [core.User] 加 JSON tag：那上面有 `PasswordHash`，
// 一旦它能直接序列化，某天一个 `writeJSON(w, 200, user)` 就会把口令哈希发到浏览器。
// 这里逐字段列出**允许出网**的名字。
func userPayload(user core.User) map[string]any {
	return map[string]any{
		"id":   user.ID.String(),
		"name": user.Name,
		"role": user.Role.String(),
	}
}

// cookieToken 从 `Cookie` 头里取会话 token。
//
// ⚠ 必须先剥名字再要求紧接 `=`：否则 `mcloud_session_extra=...` 会被误认成它。
func cookieToken(headers http.Header) string {
	raw := headers.Get("Cookie")
	if raw == "" {
		return ""
	}
	for _, part := range strings.Split(raw, ";") {
		part = strings.TrimSpace(part)
		rest, ok := strings.CutPrefix(part, CookieName)
		if !ok {
			continue
		}
		value, ok := strings.CutPrefix(rest, "=")
		if !ok || value == "" {
			// 空值不算带了 cookie。
			//
			// ⚠ 这是**双保险不是唯一防线**：[app.CurrentByToken] 自己也会拦空
			// token（那条由 app 包的用例钉住），所以删掉这里**不会让本包的任何
			// 测试变红**（反向测试已确认）。留着的理由是 `sha256("")` 是个合法
			// 哈希 —— 万一库里真有空串派生那一行，就会被认成一次登录。
			// ⇒ 真正的判据在 app 层，**这里不是**。
			continue
		}
		return value
	}
	return ""
}

// cookieHeader 拼一个 `Set-Cookie`。
func cookieHeader(token string, maxAge int, secure bool) string {
	if maxAge < 0 {
		maxAge = 0
	}
	var b strings.Builder
	b.WriteString(CookieName)
	b.WriteString("=")
	b.WriteString(token)
	b.WriteString("; Path=/; HttpOnly; SameSite=Lax; Max-Age=")
	b.WriteString(strconv.Itoa(maxAge))
	if secure {
		b.WriteString("; Secure")
	}
	return b.String()
}

// writeWithSession 把会话 cookie 写进响应。
func (s *Server) writeWithSession(
	w http.ResponseWriter, status int, token string, session core.Session, payload any,
) {
	maxAge := int(time.Until(session.ExpiresAt) / time.Second)
	w.Header().Set("Set-Cookie", cookieHeader(token, maxAge, s.config.SecureCookie))
	writeJSON(w, status, payload)
}
