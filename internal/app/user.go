package app

// 系统用户的启动流程：**首个管理员的初始化**。
//
// # 为什么 CLI 与 WebUI 必须共用这一个用例
//
// 方案 §3.7 点名了一个坑：**两套 bootstrap**。
// 如果 CLI 有自己的一套"建第一个用户"、WebUI 首次访问又有一套，
// 就会出现"CLI 建了 A、浏览器又建了 B"，而账号归属只有一个 —— 数据从此分家。
// 所以这里只有一条路径，两个入口都调它。
//
// # 为什么"只有从零到一"
//
// 初始化只在 `CountUsers() == 0` 时成立。加第二个用户是**另一件事**：
// 它涉及权限（谁能建、建成什么角色），混进这里会让"手滑多跑一次 init"
// 变成"系统里多出一个人"，而且这个人拿的还是管理员。

import (
	"context"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/catlair/mcloud/internal/core"
)

// MaxUsernameLen 是用户名长度上限。
//
// 它是**产品约束**（终端里要能对齐打印、URL 里要能放），不是协议限制。
const MaxUsernameLen = 64

// UserInitStore 是 [InitFirstAdmin] 需要的存储能力。
//
// Rust 侧写作 `S: UserStore + StateStore` 两个约束；Go 没有"多重约束"语法糖，
// 得显式合成一个接口。⚠ 别图省事只写 `core.UserStore` —— 认领历史数据
// （[core.StateStore.ClaimUnowned]）在另一个接口上，漏掉它编译器会报，
// 但**反过来**（只写 StateStore）缺的是用户表那三个方法，一样能报 ——
// 真正危险的是将来有人把其中一个接口"简化"掉，那时这个合成接口就是唯一还在的地方。
type UserInitStore interface {
	core.StateStore
	core.UserStore
}

// InitOutcome 是初始化的结果。
type InitOutcome struct {
	// User 是建出来的那个用户。
	User core.User
	// Claimed 是被认领的历史数据（账号 / 定时任务 / 组合）。
	//
	// 单独返回它是为了让人看见"升级上来的数据去哪了" ——
	// 从旧项目升上来的用户最关心的就是这个数（0 和 2 是两种心情）。
	Claimed core.ClaimedRows
}

// UserIDFromName 把用户名映射成稳定 id。
//
// 直接用名字当 id（不做任何变形）：`mcloud login --user alice` 里的 `alice`
// 就是标识，用户不需要另记一串 uuid。`users.name` 上有大小写不敏感的唯一约束，
// 所以撞名会在写入时被拒 —— 而"变形"（比如加哈希后缀）会把"改名"变成"换人"。
func UserIDFromName(name string) core.UserID { return core.NewUserID(name) }

// validateName 校验用户名。空、过长、含空白字符都会被拒。
func validateName(name string) (string, *core.Error) {
	trimmed := strings.TrimSpace(name)
	if trimmed == "" {
		return "", core.NewBadRequest("用户名不能为空")
	}
	if utf8.RuneCountInString(trimmed) > MaxUsernameLen {
		return "", core.NewBadRequest(fmt.Sprintf("用户名最长 %d 个字符", MaxUsernameLen))
	}
	// 空白字符单独拒掉：它会让"看起来一样"的两个名字变成两个用户，
	// 而在终端里根本分辨不出来
	if strings.ContainsAny(trimmed, " \t\r\n\v\f") {
		return "", core.NewBadRequest("用户名不能包含空白字符")
	}
	return trimmed, nil
}

// InitFirstAdmin 初始化首个管理员，并把无主账号认领给它。
//
// 口令强度由 [core.HashPassword] 把关（argon2id + 最小长度），
// 这里不重复实现一遍 —— 策略只能有一处。
//
// 📌 顺序是有意的：**先**校验名字 → **再**看有没有用户 → **最后**才看口令。
// 口令那一步（argon2id）是慢的，把它放在最前面等于让"已经有用户了"这种
// 廉价失败也付一次哈希的代价。
//
// ⚠ 下面那句"口令至少 N 位"的显式检查**不是必需的**：[core.HashPassword]
// 第一件事就是判同一个下限，文案也一模一样 —— 所以删掉它**不会让任何测试
// 变红**（反向测试已确认）。留着的唯一理由是：口令策略将来若从 core 挪走，
// 这一层会当场响，而不是"悄悄开始建 3 位口令的管理员"。
// ⇒ 别把它当成"这里才是判据"，真正的判据在 core。
func InitFirstAdmin(
	ctx context.Context,
	store UserInitStore,
	name string,
	password string,
	rng core.Random,
	now time.Time,
) (InitOutcome, *core.Error) {
	name, perr := validateName(name)
	if perr != nil {
		return InitOutcome{}, perr
	}

	existing, err := store.CountUsers(ctx)
	if err != nil {
		return InitOutcome{}, asAppError(err, "读用户数失败")
	}
	if existing > 0 {
		return InitOutcome{}, core.NewBadRequest(fmt.Sprintf(
			"系统里已经有 %d 个用户了。初始化只负责「从零到一」；"+
				"新增用户要走用户管理（涉及角色权限），不能靠再跑一次初始化", existing))
	}
	// 早失败：先别写用户，免得留下一个"建了但口令太短"的半成品
	if utf8.RuneCountInString(password) < core.MinPasswordLen {
		return InitOutcome{}, core.NewBadRequest(
			fmt.Sprintf("口令至少 %d 位", core.MinPasswordLen))
	}

	hash, err := core.HashPassword(password, rng)
	if err != nil {
		return InitOutcome{}, asAppError(err, "口令哈希失败")
	}

	// id 与 name 取同一个值：见 [UserIDFromName] 的说明
	user := core.NewUser(name, name, hash, core.RoleAdmin, now)
	if err := store.UpsertUser(ctx, &user); err != nil {
		return InitOutcome{}, asAppError(err, "写入用户失败")
	}

	// 认领从旧库迁移过来、还没归属的数据（`owner IS NULL`）：账号、定时任务、组合。
	// 这是"先 migrate 再 init"顺序下它们的去处；反过来（先 init）则是导入时直接归属。
	claimed, err := store.ClaimUnowned(ctx, user.ID)
	if err != nil {
		// ⚠ 用户**已经写进去了**，所以这里失败不能当作"什么都没发生"：
		// 那会让人再跑一次 init，而第二次必然被"已经有用户了"挡住 ——
		// 于是这个账号永远认领不到自己的历史数据，且没有任何报错。
		// 所以如实报出来，让调用方决定（CLI 会提示手动重跑认领）。
		return InitOutcome{User: user}, asAppError(err, "用户已建立，但认领历史数据失败")
	}

	return InitOutcome{User: user, Claimed: claimed}, nil
}
