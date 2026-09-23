package server

// 账号的**写**面：粘贴 auth 添加、启用开关、删除。
//
// # 这一组与读面的差别只在一条纪律
//
// 读面（[listAccounts]）拿到的是展示字段；写面经手的正是"能直接冒充登录的那串东西"。
// 所以这里的三条一起成立，缺一不可：
//
//   - **只写不读**：响应继续走逐字段白名单的 [accountPayload]，
//     它没有 `authorization` / `jwtToken` 这两个键 —— 新增字段默认"不出网"这条仍然有效；
//   - **错误串不插值**：任何失败都只说"哪一类问题"，绝不把用户贴进来的那串带回去
//     （错误体会进浏览器历史、代理日志与前端的错误缓存）；
//   - **不做 merge**：粘贴路径不提供"更新凭据"，已存在就拒绝，理由写在
//     [capabilities.PersistAuth] 那条注释里（整行覆盖会把凭据降级，而且看不出来）。

import (
	"net/http"
	"strings"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// addAccountBody 是"粘贴 auth 添加账号"的请求体。
//
// 键名叫 `auth` 而不是 `authorization`：用户在浏览器控制台里看到、复制的就是
// 那个叫 `auth` 的东西（客户端把它整串存进本地配置），而 `Authorization` 是
// 它在 HTTP 头上的名字。对得上用户手里的词，比对上我们的列名重要。
type addAccountBody struct {
	Auth string `json:"auth"`
}

// addAccount 把一串粘贴进来的 auth 落成一个账号。
//
// 成功只回**展示字段**（HTTP 201），其中 `id` 是掩码手机号 ——
// 前端要靠它做后续的开关与删除，而这三样没有一个需要凭据本体。
func (s *Server) addAccount(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[addAccountBody](w, r)
	if !ok {
		return
	}
	if strings.TrimSpace(body.Auth) == "" {
		// 不提"你贴的东西"的任何片段：这一句会被原样显示在界面上
		ErrDomain(core.NewBadRequest("auth 不能为空")).Write(w)
		return
	}

	ctx := r.Context()
	scope := scopeOf(auth)
	id, perr := capabilities.PersistAuth(ctx, s.store, scope, body.Auth, s.Now())
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}

	// 回读而不是回显入参拼出来的对象：让"响应里的这一行"与"库里的那一行"
	// 是同一份事实，字段口径（谁掩码了、谁没落库）由存储层说了算。
	account, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if account == nil {
		// 刚写完就读不到：只可能是归属判据出了岔子，不是"用户没这个账号"
		ErrDomain(core.NewInternal("写入后读不到该账号（归属判据与写入口径不一致）")).Write(w)
		return
	}
	writeJSON(w, http.StatusCreated, accountPayload(*account, s.Now().UnixMilli()))
}

// enabledBody 是启用开关的请求体。
//
// `*bool` 而不是 `bool`：省略字段与给 `false` 必须是两回事，
// 否则"漏传"会静默变成一个停用操作 —— 而停用是用户最难察觉的那种改变。
type enabledBody struct {
	Enabled *bool `json:"enabled"`
}

// setAccountEnabled 开/停一个账号。
//
// 用 `PUT` 而不是 `PATCH`：这条路径管的子资源就一个布尔值，
// "整份替换它的值"正是 PUT 的语义；用 PATCH 反而要额外解释"只给部分字段时怎么办"。
func (s *Server) setAccountEnabled(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := core.NewAccountID(r.PathValue("id"))
	body, ok := decodeJSON[enabledBody](w, r)
	if !ok {
		return
	}
	if body.Enabled == nil {
		ErrDomain(core.NewBadRequest(`请求体要有布尔字段 enabled`)).Write(w)
		return
	}

	ctx := r.Context()
	scope := scopeOf(auth)
	// 窄写：只动 `enabled` 一列（见 [core.StateStore.SetAccountEnabled]）。
	// 走整行覆盖会顺手把昵称、排序、余额按这份请求里根本没有的值写回去。
	if err := s.store.SetAccountEnabled(ctx, scope, id, *body.Enabled); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}

	account, err := s.store.GetAccount(ctx, scope, id)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	if account == nil {
		// 写的时候还在、读的时候没了：中间有人删了它。给 404 而不是 500，
		// 因为对使用者而言结论是同一个（这一行不再属于他）。
		ErrDomain(core.NewNotFound("云盘账号 " + id.String())).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, accountPayload(*account, s.Now().UnixMilli()))
}

// deleteAccount 删掉账号连同它的凭据与历史（连带范围见 [core.StateStore.DeleteAccount]）。
//
// 成功回 204 无体：前端已经知道那一行长什么样，再回一份"刚被删掉的东西"
// 只会让它去显示一个不存在的内容。
func (s *Server) deleteAccount(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	id := core.NewAccountID(r.PathValue("id"))
	if err := s.store.DeleteAccount(r.Context(), scopeOf(auth), id); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
