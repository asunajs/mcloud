// **网络路径**的会话：登录本系统（WebUI 用；**CLI 刻意不用**）。
//
// 对应 Rust 侧 legacy/crates/mcloud-app/src/session.rs。
//
// # 为什么只有网络路径需要"你是谁"
//
// 数据库就在本机：能跑 CLI 的人本来就能直接读库，给他加一道"你是谁"的关卡
// **不增加任何安全性**，却把本地运维变别扭（列个账号都要先登录）。
// 所以 CLI 走 [core.ScopeLocal]：本地持有库文件的人本来就能操作所有数据，
// 这在逻辑上是一句实话，而不是一个权限漏洞。
//
// 真正需要会话的是**网络路径**：WebUI 的用户拿不到库文件，"A 不能看到 B 的
// 账号"才有意义 —— 那里每个请求都要先解析出 [core.UserID]，再据此取
// [core.ScopeUser]。
//
// # 携带方式：cookie 里是**明文 token**，库里只有哈希
//
//   - 明文 token 只在签发那一刻返回一次（[SignIn] 的返回值），
//     调用方立刻写进 `Set-Cookie`，**不落任何持久介质**；
//   - 库里存的是 `sha256(token)`（[core.HashSessionToken]）：
//     拿到库文件也无法直接冒充登录；
//   - 撤销 = 删掉那一行，**立刻生效** —— 而不是"等 cookie 自己过期"。
//
// # 三种情况都返回"没有"（刻意不区分）
//
// 没带 cookie、会话已过期、会话已被撤销。对调用方来说它们该导出同一个动作 ——
// 去登录；区分只会诱使人写出"过期了但还能再试试"这种分支。
//
// # 这一层不碰 HTTP
//
// cookie 的读写、状态码、`Set-Cookie` 属性都在 `internal/server`。
// 放这里就会把 HTTP 框架拖进组装层，而 CLI 也要依赖组装层。
package app

import (
	"context"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// WebSessionDays 是 WebUI 会话的有效期（天）。
//
// 与 CLI 那边的 365 天刻意不同：cookie 会被浏览器带着走**每一个**请求，
// 泄露面比"本机上一个只存哈希的文件"大得多。
//
// 到期不是**唯一**的保护（撤销立刻生效），所以不必短到几分钟 ——
// 那会让用户每隔一会儿就被踢回登录页，而安全性并没有实质变化。
const WebSessionDays = 7

// SignInWith 登录本系统：校验口令 → 签发会话，返回**明文 token（只此一次）**。
//
// kind / ttl 由调用方给：WebUI 用 [SignIn]，其它携带方式各自决定。
// 不在这里写死，是因为"多久算过期"与"这是哪种会话"是**产品决定**，
// 而库里那一行要如实记下是什么会话。
//
// # 用户不存在与口令错误给**同一个**结论
//
// 区分两者会让登录页变成用户名枚举器 —— 那条由
// `TestWrongPasswordAndUnknownUserAreIndistinguishable` 钉住。
func SignInWith(
	ctx context.Context, store core.UserStore,
	name, password string, kind core.SessionKind, ttl time.Duration,
	rng core.Random, now time.Time,
) (core.User, string, core.Session, *core.Error) {
	invalid := func() *core.Error {
		return core.NewAuth("用户名或口令不正确")
	}

	user, err := store.GetUserByName(ctx, name)
	if err != nil {
		return core.User{}, "", core.Session{}, asAppError(err, "读用户失败")
	}
	if user == nil {
		return core.User{}, "", core.Session{}, invalid()
	}
	ok, err := core.VerifyPassword(password, user.PasswordHash)
	if err != nil {
		return core.User{}, "", core.Session{}, asAppError(err, "校验口令失败")
	}
	if !ok {
		return core.User{}, "", core.Session{}, invalid()
	}

	token := core.NewSessionToken(rng)
	session := core.NewSession(core.HashSessionToken(token), user.ID, now.Add(ttl), kind)
	if err := store.InsertSession(ctx, &session); err != nil {
		return core.User{}, "", core.Session{}, asAppError(err, "写入会话失败")
	}
	return *user, token, session, nil
}

// SignIn 是 WebUI 登录：[core.SessionWeb]，有效期 [WebSessionDays] 天。
func SignIn(
	ctx context.Context, store core.UserStore,
	name, password string, rng core.Random, now time.Time,
) (core.User, string, core.Session, *core.Error) {
	return SignInWith(ctx, store, name, password,
		core.SessionWeb, WebSessionDays*24*time.Hour, rng, now)
}

// CurrentByToken 按**明文 token** 解析当前会话（cookie 携带的就是明文）。
//
// 空 token 直接返回"没有"：[core.HashSessionToken]("") 是一个合法哈希，
// 万一库里真有个空串派生的会话，就会变成"不带 cookie 也能登录"。
func CurrentByToken(
	ctx context.Context, store core.UserStore, token string, now time.Time,
) (*core.User, *core.Session, *core.Error) {
	if token == "" {
		return nil, nil, nil
	}
	session, err := store.GetSession(ctx, core.HashSessionToken(token))
	if err != nil {
		return nil, nil, asAppError(err, "读会话失败")
	}
	if session == nil || session.IsExpiredAt(now) {
		return nil, nil, nil
	}
	// 会话存在但用户被删了（外键 CASCADE 会清会话，这里是双保险）
	user, err := store.GetUser(ctx, session.UserID)
	if err != nil {
		return nil, nil, asAppError(err, "读用户失败")
	}
	if user == nil {
		return nil, nil, nil
	}
	return user, session, nil
}

// SignOutToken 撤销一个会话（登出）。返回是否真的删掉了一行。
//
// 空 token 返回 false 而不是去删：删空串的哈希可能命中一个真实存在的行 ——
// 那等于"没带 cookie 的人登出了别人"。
func SignOutToken(ctx context.Context, store core.UserStore, token string) (bool, *core.Error) {
	if token == "" {
		return false, nil
	}
	deleted, err := store.DeleteSession(ctx, core.HashSessionToken(token))
	if err != nil {
		return false, asAppError(err, "撤销会话失败")
	}
	return deleted, nil
}
