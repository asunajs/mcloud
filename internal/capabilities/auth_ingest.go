// 把一串**已经在用户手里**的 `auth` 落成账号。
//
// 与登录那条链路（[AccountAndStateFromLoginWithPrefix]）的区别不是"谁先谁后"，
// 而是**谁知道的东西不一样**：登录是我们自己跑完协议拿到的，
// 所以前缀由代码指定、`userInfo` 里还有 `userDomainId`；
// 粘贴则是用户从别处（浏览器控制台、另一个部署）抄来的整串，
// 前缀**自带**、其余元信息一概没有。
//
// # 为什么这里不共用"前缀闸"
//
// 登录路径的 `prefix` 参数是防呆用的：给 App 的结果套 PC 前缀会静默通过，
// 所以那一步必须由调用方显式表态。粘贴路径**没有**这个可表态的位置 ——
// 字符串里写的是 `pc` 还是 `mobile`，我们只能照抄。
// ⇒ 后果要写清楚：**粘贴进来的凭据，端别不受校验**。
//
//	一个 App 端抄来的 `mobile:` 串会被原样接受（能用与否由上游说了算，不由我们说了算）。
package capabilities

import (
	"context"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// AccountAndStateFromAuth 把 `auth` 整串拆成账号 + 登录态（**纯函数**，便于逐字段断言）。
//
// 入参带不带 `Basic ` 前缀都接受：[core.ParseAuth] 本来就折掉第一处 `Basic `，
// 而落库时统一用 `parsed.Auth`（它保证是 `Basic <base64>` 那一形）。
// 这样"用户从 curl 里连着 `Basic ` 一起抄"与"只抄 base64"得到**同一个**结果，
// 不会出现两个长得不一样的字符串冒充同一个凭据。
//
// # 为什么这四条拒绝是 BadRequest 而不是 Parse
//
// [core.NewParse] 的语义是"**上游响应**不合形状"（见 [core.KindParse] 那条注释：
// 通常是上游改了），而这里的四条全是"你贴进来的这串用不了" —— 与
// [PersistAuth] 那句"账号已存在"是同一类，都归 BadRequest。
// 差别在 HTTP 那一层是要命的：Parse 落 500，而 5xx 只回类别文案，
// 于是"把口令字段粘错了"在界面上显示成"服务器内部错误"——
// 使用者改不了自己的错，还以为是这台服务坏了。
func AccountAndStateFromAuth(
	scope core.Scope, rawAuth string, now time.Time,
) (core.Account, core.AccountState, *core.Error) {
	trimmed := strings.TrimSpace(rawAuth)
	if trimmed == "" {
		return core.Account{}, core.AccountState{}, core.NewBadRequest("auth 是空的")
	}

	parsed, ok := core.ParseAuth(trimmed)
	// ⚠ 下面每一条错误串都**不插值**用户给的那串东西：
	// 它是凭据本体，进了错误串就等于进了响应体与日志。
	if !ok {
		return core.Account{}, core.AccountState{}, core.NewBadRequest(
			"auth 解析不出来：不是 base64，或解出来不是文本")
	}

	// 手机号与 token 缺一不可：前者决定账号 id（缺了两个粘贴会撞成同一个"账号"），
	// 后者是这次添加唯一的目的。登录路径容忍缺失（上游只在这时返回非 null），
	// 粘贴路径没有"先收下再说"的理由。
	phone := ""
	if parsed.Phone != nil {
		phone = strings.TrimSpace(*parsed.Phone)
	}
	if phone == "" {
		return core.Account{}, core.AccountState{}, core.NewBadRequest(
			"auth 里没有手机号，无法确定这是哪个账号")
	}
	token := ""
	if parsed.Token != nil {
		token = strings.TrimSpace(*parsed.Token)
	}
	if token == "" {
		return core.Account{}, core.AccountState{}, core.NewBadRequest(
			"auth 里没有 token，这份凭据不能用")
	}

	accountID := core.NewAccountID(core.AccountLabel(nil, phone))
	// 与 [AccountAndStateFromLoginWithPrefix] 同一条：AccountID 本身就是掩码，
	// 复用它而不是在这里再掩一遍，免得两处掩码规则跑偏。
	masked := accountID.String()

	account := core.NewAccount(accountID.String())
	account.Owner = scope.Owner()
	account.PhoneMasked = &masked
	account.Platform = parsed.Platform
	if millis := parsed.ExpireMillis(); millis != 0 {
		expire := millis
		account.AuthExpire = &expire
	}

	jwtToken := core.SecretValue(token)
	authorization := core.SecretValue(parsed.Auth)
	full := core.FullPhone(phone)

	state := core.NewAccountState(accountID.String())
	state.JWTToken = &jwtToken
	state.Authorization = &authorization
	state.Phone = &full
	// `device_id` / `user_domain_id` **留空**：粘贴路径没有它们，
	// 而"留空"与"填一个猜出来的值"在库里的差别，
	// 是要到上游报 4xx 那天才看得见（见 [core.AccountState] 上那条"只装拿到的"）。
	state.UpdatedAt = &now

	return account, state, nil
}

// PersistAuth 把粘贴来的 `auth` 落库，返回账号 id。
//
// # 已存在时报错，而不是覆盖
//
// [core.StateStore.UpsertAccount] 与 [core.StateStore.UpsertAccountState] 都是
// **整行覆盖**：走"已存在就覆盖"这条路，粘贴一次会把昵称、排序、启用状态
// 一起按这份新快照写回去，而用户只想要"换个凭据"。
// 更重的一条是**凭据可能被降级**：粘贴路径无从判断这串比库里那串新还是旧，
// 覆盖一旦选中旧的，症状是"过几天突然全挂"，事后无从追查。
// 所以这里明确拒绝，把选择权交回用户（先删除再加，或走登录那条路）。
//
// ⚠ 检查与写入之间不是原子的：并发粘贴同一账号时后写的那次会覆盖前一次。
//
//	两次都是用户主动提交的凭据，覆盖与语义相符；要真正的"仅新增"得给存储
//	再加一条 insert-only 写 —— 为这个窗口不值。
func PersistAuth(
	ctx context.Context, store core.StateStore, scope core.Scope, rawAuth string, now time.Time,
) (core.AccountID, *core.Error) {
	account, state, perr := AccountAndStateFromAuth(scope, rawAuth, now)
	if perr != nil {
		return "", perr
	}

	existing, err := store.GetAccount(ctx, scope, account.ID)
	if err != nil {
		return "", asError(err, "读取账号失败")
	}
	if existing != nil {
		// 只回掩码：这一串会直接显示在界面上，带完整手机号就等于把凭据抄给前端
		return "", core.NewBadRequest("账号 " + account.ID.String() + " 已存在，不做覆盖")
	}

	// 顺序与 [PersistLogin] 一致：先账号后凭据，
	// 因为凭据那条写入会校验账号归属（见 [core.StateStore] 的隔离契约）。
	if storeErr := store.UpsertAccount(ctx, scope, &account); storeErr != nil {
		return "", asError(storeErr, "写入账号失败")
	}
	if storeErr := store.UpsertAccountState(ctx, scope, &state); storeErr != nil {
		return "", asError(storeErr, "写入登录态失败")
	}
	return account.ID, nil
}
