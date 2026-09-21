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
}

// New 组装服务端状态。
//
// 事件总线由本函数创建而不是让调用方传进来：总线**没有配置项**，
// 而多一个参数就多一种"两个处理器拿到不同总线"的接错法。
func New(db *mcloudio.SqliteStore, clock core.Clock, rng core.Random, cfg Config) *Server {
	return &Server{store: db, clock: clock, rng: rng, config: cfg, bus: mcloudexec.NewEventBus()}
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
func (s *Server) current(headers http.Header) (*AuthUser, *core.Error) {
	token := cookieToken(headers)
	if token == "" {
		return nil, nil
	}
	user, session, perr := app.CurrentByToken(context.Background(), s.store, token, s.Now())
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
		auth, perr := s.current(r.Header)
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
		count, err := s.store.CountUsers(context.Background())
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
	mux.HandleFunc("/api/", s.apiNotFound)

	return s.requireSetup(s.requireSession(mux))
}

// registerAPIRoutes 挂业务路由（能力清单 / 账号 / 运行 / 定时任务）。
//
// 它们自带"要会话"的前提：每一条都过 [Server.authed]，
// 于是"哪条路径免登录"仍然只有 [isPublic] 一处判断。
func (s *Server) registerAPIRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/version", s.authed(s.getVersion))
	mux.HandleFunc("GET /api/capabilities", s.authed(s.getCapabilities))

	mux.HandleFunc("GET /api/accounts", s.authed(s.listAccounts))
	mux.HandleFunc("GET /api/accounts/{id}/cloud-stats", s.authed(s.cloudStats))

	mux.HandleFunc("GET /api/runs", s.authed(s.listRuns))
	mux.HandleFunc("GET /api/runs/{id}/events", s.authed(s.runEvents))

	mux.HandleFunc("GET /api/schedules", s.authed(s.listSchedules))
	mux.HandleFunc("PUT /api/schedules/{id}", s.authed(s.putSchedule))
	mux.HandleFunc("DELETE /api/schedules/{id}", s.authed(s.deleteSchedule))
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
