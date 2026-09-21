// 系统用户、会话，以及"云盘账号归属"——**纯逻辑**部分。
// 对应 Rust 侧 ../mcloud-rust/crates/mcloud-core/src/user.rs。
//
// # 为什么这一层是新的
//
// 旧项目**完全没有**它：`/api/*` 没有任何鉴权中间件，CORS 全开，
// 账号只是配置数组里的下标。而本项目的目标是"用户用自己的账号登录**本系统**，
// 每个用户各自管理属于自己的移动云盘账号，数据按用户隔离"，
// 所以它是一个领域模型，不是"加个中间件"。
//
// # 三个概念必须分清（混起来就会设计错）
//
//	| 概念 | 是什么 | 由谁提供 |
//	| --- | --- | --- |
//	| **系统用户**（[User]） | 登录**本系统**的账号 | 我们自己 |
//	| **会话**（[Session]） | 用户登录后的凭据 | 我们自己 |
//	| **云盘账号**（[AccountID]） | 移动云盘账号（含 `auth`） | 移动侧，**归属**某个系统用户 |
//
// # 本模块只做纯计算
//
// 口令哈希（argon2id）与 token 派生（SHA-256）都是纯计算，**不碰 IO** ——
// 随机源从 [Random] 注入，所以"同一口令加同一盐得到同一哈希"能在单测里钉死。
// 持久化在 internal/io，HTTP 层的鉴权在 internal/server。
//
// # ⚠⚠ 跨语言等价性是**实测**出来的，不是推出来的
//
// PHC 串的解析边界（盐下限、参数默认值、版本号、未知参数名……）逐条来自
// test/golden/password-hash-vectors.json，那份向量由 **RustCrypto argon2 0.5.3**
// 生成 —— 生成侧与消费侧刻意是两份实现。用 Go 生成再用 Go 验证是**自证**：
// 两边一起错的时候照样是绿的。
//
// 其中两条**Go 侧无法复现**，已在向量的 `$crossLanguageGaps` 里显式登记，
// 见 [VerifyPassword] 的 default 分支。
package core

import (
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"golang.org/x/crypto/argon2"
)

// ─────────────────────────────────────────────────────────────────────────────
// 标识与枚举
// ─────────────────────────────────────────────────────────────────────────────

// UserID 是**系统用户**标识。
//
// 单独做成一个类型而不是复用 [AccountID]：两者都是字符串，但"把云盘账号 id
// 当系统用户 id 用"是一个会**静默通过类型检查**的越权 bug。
//
// ⚠ Go 的具名类型挡的是"顺手传错"（`fn(user.ID)` 传成 `account.ID` 编译不过），
// 挡不住"显式绕过"（`UserID(accountID)` 合法）—— 与 Rust newtype 的强度相同。
type UserID string

// NewUserID 构造一个系统用户标识。
func NewUserID(id string) UserID { return UserID(id) }

// String 返回底层字符串（实现 fmt.Stringer，便于日志与拼接）。
func (u UserID) String() string { return string(u) }

// Role 是角色。**只有两级**，不做细粒度授权矩阵（见 docs/rust-rewrite-plan.md §9 的非目标）。
type Role int

const (
	// RoleUser 是默认角色，取**最小权限**：新增用户忘了指定角色时，
	// 不该默认拿到管理员。
	//
	// ⚠ 它的值是 0，所以 Go 的零值正好落在"最小权限"上 —— 与 Rust 的
	// `#[default] User` 对齐。**顺序不能调**：把 RoleAdmin 放前面会让
	// 任何"忘了赋值"的路径静默提权。
	RoleUser Role = iota
	// RoleAdmin 是管理员。
	RoleAdmin
)

// String 返回存储与展示用的文本。
//
// ⚠ 它同时是**落库形式**（`role` 列存 `admin`/`user`）与调试形式 ——
// 两者在这里恰好一致，所以不必拆成两个函数。
//
// ⚠ 未知值返回**空串**，不能写成 `if r == RoleAdmin { … }; return "user"`。
// 那种写法会让任何越界值都有名字，于是"已知态有几个"再也数不出来 ——
// 枚举漏态守卫就静默失效了。两个变体的枚举最容易顺手写成 if/else。
func (r Role) String() string {
	switch r {
	case RoleUser:
		return "user"
	case RoleAdmin:
		return "admin"
	}
	return ""
}

// ParseRole 从存储里读回。无法识别的值一律降级为 [RoleUser] ——
// 反过来（未知 → RoleAdmin）会让一次脏数据变成提权。
func ParseRole(raw string) Role {
	if raw == "admin" {
		return RoleAdmin
	}
	return RoleUser
}

// SessionKind 是会话来源。WebUI 是 HTTP（有到期与登出），CLI 是本机单机
// （登录态可长期有效）。
type SessionKind int

const (
	// SessionWeb 是 HTTP 会话。
	SessionWeb SessionKind = iota
	// SessionCLI 是本机会话。
	//
	// ⚠ 零值是 [SessionWeb] 而不是它 —— 方向是对的：忘赋值时拿到"有到期的
	// 那个"，比拿到"长期有效的那个"保守。
	SessionCLI
)

// String 返回存储与展示用的文本。未知值返回空串（理由同 [Role.String]）。
func (k SessionKind) String() string {
	switch k {
	case SessionWeb:
		return "web"
	case SessionCLI:
		return "cli"
	}
	return ""
}

// ParseSessionKind 从存储里读回。未知值降级为 [SessionCLI]（对齐 Rust 侧）。
func ParseSessionKind(raw string) SessionKind {
	if raw == "web" {
		return SessionWeb
	}
	return SessionCLI
}

// ─────────────────────────────────────────────────────────────────────────────
// User
// ─────────────────────────────────────────────────────────────────────────────

// PasswordHash 是 argon2id 的 PHC 编码串。
//
// # 为什么是独立类型而不是裸 string
//
// 与 Rust 侧手写 `Debug` 同源，但 Go 里的触发面**更宽**：Rust 只有 `{:?}` 一条路，
// 而 Go 有 `%v`、`%+v`、`%#v` 三条。它自带盐与参数（所以库里不需要额外的 salt 列），
// 本身不是"可复用凭据"，但仍不该出现在日志或 API 响应里 —— 与 `auth` 同一条边界：
// 只落本地存储。
//
// 把字段做成**有 Stringer 的类型**，`%+v` 也会脱敏（fmt 打印字段值时按 `%v` 规则走），
// `%#v` 由 GoString 接住。裸 string 字段则三条路全泄漏。
//
// ⚠ JSON 序列化**不受影响**：[String] 不是 [encoding.TextMarshaler]，
// 而 encoding/json 对 `kind == String` 的具名类型直接输出底层值。落库要真值。
type PasswordHash string

// String 实现 fmt.Stringer —— 打印一律脱敏。
func (PasswordHash) String() string { return "<已隐藏>" }

// GoString 覆盖 `%#v`。
func (PasswordHash) GoString() string { return `core.PasswordHash("<已隐藏>")` }

// User 是系统用户。
// ⚠ 每个字段的 `json` tag 都是外部契约：Rust 侧 `User` 派生了 `Serialize`，
// 字段名是 **snake_case**（`password_hash` / `created_at` …）；
// 而 Go 不带 tag 会输出字段名原样（`PasswordHash` / `CreatedAt` …）。
//
// 这一处**曾经整个漏写**（`user` 那一步带过来的），而且没有任何测试变红 ——
// 因为往返测试用什么字段名都是绿的。现在由黄金向量（`cloud-stats` 那一条的
// 同类）守着，形状由 **serde** 给出，不是从命名规则推的。
type User struct {
	ID   UserID `json:"id"`
	Name string `json:"name"`
	// PasswordHash 是 argon2id 编码串（**自带盐与参数**）。
	PasswordHash PasswordHash `json:"password_hash"`
	Role         Role         `json:"role"`
	CreatedAt    time.Time    `json:"created_at"`
}

// NewUser 构造一个系统用户。
func NewUser(id, name string, passwordHash PasswordHash, role Role, createdAt time.Time) User {
	return User{
		ID:           UserID(id),
		Name:         name,
		PasswordHash: passwordHash,
		Role:         role,
		CreatedAt:    createdAt,
	}
}

// IsAdmin 报告该用户是否为管理员。
func (u User) IsAdmin() bool { return u.Role == RoleAdmin }

// String 实现 fmt.Stringer —— **不含**口令哈希。
//
// 对应 Rust 侧手写的 `Debug`：`%v` 是调试时最容易顺手打出来的东西
// （日志、断言失败信息都会用它），这里只暴露能印的字段。
//
// ⚠ 覆盖不到 `%+v`（fmt 对 struct 的 `%+v` 会**展开字段**，不走 Stringer）——
// 那一条由 [PasswordHash] 自己接住。这正是把字段做成具名类型的理由：
// **struct 级的 Stringer 挡不住 `%+v`，字段级的 Stringer 才挡得住。**
func (u User) String() string {
	return fmt.Sprintf("User{id=%s name=%s role=%s created_at=%s password_hash=<已隐藏>}",
		u.ID, u.Name, u.Role, u.CreatedAt.Format(time.RFC3339))
}

// ─────────────────────────────────────────────────────────────────────────────
// Session
// ─────────────────────────────────────────────────────────────────────────────

// Session 是会话。
//
// **只存 token 的哈希**：明文只在签发那一刻返回一次。这样即便库被读走，
// 也无法直接拿来冒充登录（与口令存哈希是同一个理由）。
// 字段名的理由同 [User]。
type Session struct {
	// TokenHash 是 `sha256(token)` 的十六进制。
	TokenHash string `json:"token_hash"`
	UserID    UserID `json:"user_id"`
	// ExpiresAt 序列化成 RFC3339（`…Z`）。
	//
	// ⚠ 两端在**秒以下**不一定同形：chrono 用 AutoSi（0/3/6/9 位），
	// Go 的 `time.Time` 用 RFC3339Nano（去掉末尾的 0）。整秒时两者一致，
	// 所以向量里只钉整秒 —— 亚秒精度是**已知未对齐**的一条，真要用到时
	// 得显式格式化，不能指望默认行为一致。
	ExpiresAt time.Time   `json:"expires_at"`
	Kind      SessionKind `json:"kind"`
}

// NewSession 构造一个会话。
func NewSession(tokenHash string, userID UserID, expiresAt time.Time, kind SessionKind) Session {
	return Session{TokenHash: tokenHash, UserID: userID, ExpiresAt: expiresAt, Kind: kind}
}

// IsExpiredAt 报告会话在 now 时刻是否已过期（**时间从外部传入**，不在内部读时钟）。
//
// ⚠ 边界是**闭**的：到期时刻本身算过期。宁可让用户重登，
// 也不要放行一个刚好到点的会话。
//
// ⚠ 判据写成 `!ExpiresAt.After(now)` 而不是 `ExpiresAt == now` 或
// `ExpiresAt.Before(now) || ExpiresAt == now`：`time.Time` 的 `==` 还比较
// location 与单调时钟读数，两个"同一瞬时"的时间可以 `!=` ——
// 用 `==` 拼边界会让"刚好到点"随机地算过期或不过期。
func (s Session) IsExpiredAt(now time.Time) bool { return !s.ExpiresAt.After(now) }

// IsValidAt 报告会话在 now 时刻是否有效。
func (s Session) IsValidAt(now time.Time) bool { return !s.IsExpiredAt(now) }

// ─────────────────────────────────────────────────────────────────────────────
// 口令
// ─────────────────────────────────────────────────────────────────────────────

// MinPasswordLen 是口令最小长度。
//
// 这是**自建系统的下限**，不是安全策略的全部：真正的防线是 argon2id 的默认参数
// （Argon2id，m=19456 KiB / t=2 / p=1，见 [HashPassword]）。
//
// ⚠ 按 **Unicode 字符数**算，不是字节数 —— 与 Rust 的 `chars().count()` 对齐。
// 写成 `len(plain)` 会让"口令至少 8 位"对中文变成"至少 3 个字"。
const MinPasswordLen = 8

// argon2id 的默认参数 —— 与 Rust 侧 `Argon2::default()` 逐字段对齐。
//
// ⚠ 这几个值**不是随便定的**：它们决定 PHC 串里写什么、以及校验时用什么算。
// 改动任何一个都会让**已落库的哈希全部失效**（校验一律变成"不匹配"），
// 而现象只是"所有人都登录不上"，不会有任何编译或测试错误。
const (
	argon2MemoryKiB uint32 = 19456
	argon2TimeCost  uint32 = 2
	argon2Threads   uint8  = 1
	argon2OutputLen uint32 = 32
	// argon2Version 是 PHC 串里的 `v=`，也是 Go 侧唯一能算的版本
	// （x/crypto 把 0x13 写死在 initHash 里）。
	argon2Version = 19
	// passwordSaltBytes 是盐的字节数。
	passwordSaltBytes = 16
)

// PHC 解析的三条长度/成本下限。
//
// ⚠ 所有数字都来自**实测**，不是从文档推的（见向量的 `$saltLengthBoundary`
// 与 `$costBoundary` 两段扫描）：
//
//   - 文档里常见的说法是"盐至少 4 字节"，而 RustCrypto 实测拒掉了 3/4/6 字节、
//     放行了 8 —— 下限是 **8**；
//   - 哈希那条对应 `Output::MIN_LENGTH`，实测空哈希段的报错信息里写着 `expected: 10`；
//   - `m` 的下限实测是 **8**（7 拒、8 放行）—— 这条**最阴**：Go 的 `argon2.IDKey`
//     会把过小的 memory 悄悄抬到 `2*4*threads` 再算，于是 `m=1` 在 Go 侧
//     算出的是"某个值"、报**不匹配**，而 Rust 侧报的是**格式非法**。
//     同一个串在两边一个回"用户名或口令不正确"、一个回 500。
const (
	phcMinSaltBytes   = 8
	phcMinOutputBytes = 10
	phcMinMemoryKiB   = 8
)

// HashPassword 计算口令哈希（argon2id，默认参数）。
//
// 盐从 [Random] 注入而不是内部取全局随机源：这样"同一口令 + 同一盐 → 同一哈希"
// 可以在单测里断言，而"两次哈希不同"也能被钉住（盐确实是随机的）。
func HashPassword(plain string, rng Random) (PasswordHash, error) {
	if utf8.RuneCountInString(plain) < MinPasswordLen {
		return "", NewBadRequest(fmt.Sprintf("口令至少 %d 位", MinPasswordLen))
	}

	saltBytes := make([]byte, passwordSaltBytes)
	rng.FillBytes(saltBytes)

	sum := argon2.IDKey([]byte(plain), saltBytes, argon2TimeCost, argon2MemoryKiB, argon2Threads, argon2OutputLen)

	// 与 RustCrypto 的 `PasswordHash::to_string()` 逐字对齐：
	// `$argon2id$v=19$m=19456,t=2,p=1$<盐>$<哈希>`，两段 base64 都是
	// **标准字母表、无填充**（PHC 用的是 `base64ct::Base64Unpadded`，
	// 对应 Go 的 [base64.RawStdEncoding]；用 `StdEncoding` 会多出 `=` 填充，
	// 于是**校验自己的输出都会失败**）。
	//
	// ⚠ 参数顺序固定为 `m,t,p`：换个顺序仍能被解析，但串**逐字不等** ——
	// 而黄金向量是逐字比对的，这类错误会被它抓住。
	return PasswordHash(fmt.Sprintf("$argon2id$v=%d$m=%d,t=%d,p=%d$%s$%s",
		argon2Version, argon2MemoryKiB, argon2TimeCost, argon2Threads,
		base64.RawStdEncoding.EncodeToString(saltBytes),
		base64.RawStdEncoding.EncodeToString(sum),
	)), nil
}

// VerifyPassword 校验口令。
//
// 三个分支必须分清：**匹配**、**不匹配**（正常业务结果）、**哈希串本身非法/算法出错**
// （环境问题）。把后两者合并会让"库里的哈希被写坏"表现成"用户密码错了"，
// 从而永远查不到真正的原因。
//
// ⚠ `(false, nil)` 与 `(false, err)` 是**两件事**：前者该回"用户名或口令不正确"，
// 后者该回 500 并告警。调用方不能都当成"密码不对"。
func VerifyPassword(plain string, encoded PasswordHash) (bool, error) {
	p, err := parsePHC(string(encoded))
	if err != nil {
		return false, err
	}

	var sum []byte
	switch {
	case p.algorithm == "argon2id" && p.version == argon2Version:
		sum = argon2.IDKey([]byte(plain), p.salt, p.t, p.m, p.p, uint32(len(p.hash)))
	case p.algorithm == "argon2i" && p.version == argon2Version:
		sum = argon2.Key([]byte(plain), p.salt, p.t, p.m, p.p, uint32(len(p.hash)))
	default:
		// ⚠⚠ 跨语言不等价的两条，已登记在向量的 `$crossLanguageGaps`：
		// Rust 侧这两条都算得出结果、返回"不匹配"，而 Go 侧**只能保守拒绝**。
		//
		//   - `argon2d`：golang.org/x/crypto/argon2 的 `argon2d` 是**私有 iota**，
		//     没有导出实现（`Key` 是 argon2i、`IDKey` 是 argon2id）；
		//   - `v=16`：`argon2.IDKey` 把版本**写死在 initHash 里**（`Version = 0x13`），
		//     没有参数可传。
		//
		// ⚠ 后者尤其**不能**"顺手用 v=19 顶上"：那样
		// `$argon2id$v=16$…$<v19 生成的哈希>` 会被算成**匹配** ——
		// 等于"把版本字段改掉反而能过"。保守拒绝是唯一安全的解。
		return false, NewInternal(fmt.Sprintf(
			"不支持的口令哈希：算法 %q 版本 %d（Go 侧无对应实现）", p.algorithm, p.version))
	}

	// 常数时间比较：`bytes.Equal` 在第一个不同字节处提前返回，
	// 会把"前缀猜对了几个字节"通过耗时泄漏出去。
	return subtle.ConstantTimeCompare(sum, p.hash) == 1, nil
}

// phcParams 是 PHC 串的解析结果。
//
// **不导出**：它的形状是"实现细节"，导出会诱使别处拿它自己算哈希，
// 从而绕过 [VerifyPassword] 的常数时间比较。
type phcParams struct {
	algorithm string
	version   int
	m, t      uint32
	p         uint8
	salt      []byte
	hash      []byte
}

// isParamsField 判断这一段是不是 PHC 的参数表。
//
// 判据是**形状**（非空、且每个逗号分隔项都含 `=`）而不是"名字都合法"：
// `m=19456,t=2,p=1,x=1` 也必须被认成参数表，否则它会掉进盐段、
// 报出一个与真实问题无关的"盐太短"。
//
// ⚠ 这个判据之所以不会误判盐段：PHC base64 **无填充**，字母表里没有 `=`。
func isParamsField(s string) bool {
	if s == "" {
		return false
	}
	for _, item := range strings.Split(s, ",") {
		name, value, ok := strings.Cut(item, "=")
		if !ok || name == "" || value == "" {
			return false
		}
	}
	return true
}

// parsePHC 解析 `$argon2id$v=19$m=19456,t=2,p=1$<盐>$<哈希>`。
//
// # 判定顺序与边界**逐条来自实测**
//
// 见 test/golden/password-hash-vectors.json。几条最容易写错的：
//
//   - 段是**按位置**取的，不是按内容：`$argon2id$v=19$$<盐>$<哈希>` 里那个空段
//     会被当成**盐**（于是报"盐太短"），而不是"空参数表"；
//   - 参数**可以缺项**：缺 `p` 时取默认 1，校验结果仍是"匹配" ——
//     PHC 参数是可选的、有默认值的；
//   - 盐的下限是 **8 字节**，哈希的下限是 **10 字节**，且**先校验盐**；
//   - `v=16` 是**合法**版本（不像 `v=99`），所以它能过解析、只在计算时才分道扬镳。
//
// ⚠⚠ 本函数**必须自己挡住成本参数的下限**（`m>=8`、`t>=1`、`p>=1`）：
//
//   - `t=0` / `p=0` 会让 `argon2.IDKey` **panic**（`argon2: number of rounds too small` /
//     `parallelism degree too low`），而库里的哈希串是**可能被写坏的** ——
//     在 HTTP 服务里 panic 就是一次请求打崩整个进程；
//   - `m` 低于下限时它**不 panic，而是悄悄把 memory 抬到下限再算** —— 那是更坏的一种：
//     它让"格式非法的串"表现成"口令不对"，故障从此查不出来。
//
// 三条下限都是实测的，且 Rust 侧对同样的输入返回的都是**错误** —— 所以 Go 侧也必须报错，
// 不能"算算看"。这是 Go 侧必须补的一道闸，不是可选优化。
func parsePHC(s string) (*phcParams, error) {
	if !strings.HasPrefix(s, "$") {
		return nil, NewInternal("口令哈希格式非法: 不是 PHC 串（缺少前导 $）")
	}

	// 前导 `$` 会切出一个空段。
	fields := strings.Split(s, "$")[1:]
	if len(fields) < 2 {
		return nil, NewInternal("口令哈希格式非法: 段数不足")
	}

	algorithm := fields[0]
	switch algorithm {
	case "argon2id", "argon2i", "argon2d":
	default:
		// 实测：未注册的算法名在**解析期**就被拒（`$argon2xyz$` → Algorithm），
		// 而不是留到计算时才报错。
		return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 未知算法 %q", algorithm))
	}
	rest := fields[1:]

	// 版本段可选。只有 `v=` 前缀才算 —— 判据是前缀，不是"看起来像数字"。
	version := argon2Version
	if len(rest) > 0 && strings.HasPrefix(rest[0], "v=") {
		v, err := strconv.Atoi(rest[0][2:])
		// 实测：已注册的版本只有 16 与 19；`v=99` 报 Version 错。
		if err != nil || (v != 16 && v != argon2Version) {
			return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 未知版本 %q", rest[0]))
		}
		version = v
		rest = rest[1:]
	}

	var paramsField *string
	if len(rest) > 0 && isParamsField(rest[0]) {
		paramsField = &rest[0]
		rest = rest[1:]
	}

	m, t := argon2MemoryKiB, argon2TimeCost
	p := argon2Threads
	if paramsField != nil {
		for _, item := range strings.Split(*paramsField, ",") {
			name, value, _ := strings.Cut(item, "=")
			n, err := strconv.ParseUint(value, 10, 32)
			if err != nil {
				return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 参数 %q 的值不是十进制数", item))
			}
			switch name {
			case "m":
				m = uint32(n)
			case "t":
				t = uint32(n)
			case "p":
				if n > 255 {
					return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: p=%d 超出 uint8 范围", n))
				}
				p = uint8(n)
			default:
				// 实测：多一个 `,x=1` 报的是 ParamNameInvalid（解析期就拒），
				// **不是**"忽略未知参数"。
				return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 未知参数名 %q", name))
			}
		}
		// ⚠⚠ 闸门在这里，理由见函数注释：让 IDKey 见到 t=0 / p=0 就是 panic。
		//
		// 三条下限都是实测的（见 [phcMinMemoryKiB]）：低于下限时 Rust 侧报的是
		// **格式非法**，所以这里也必须报错，不能"算算看"。
		if t == 0 {
			return nil, NewInternal("口令哈希格式非法: t=0（argon2 要求至少 1 轮）")
		}
		if p == 0 {
			return nil, NewInternal("口令哈希格式非法: p=0（argon2 要求至少 1 条车道）")
		}
		if m < phcMinMemoryKiB {
			return nil, NewInternal(fmt.Sprintf(
				"口令哈希格式非法: m=%d 低于下限（至少 %d KiB）", m, phcMinMemoryKiB))
		}
	}

	if len(rest) == 0 {
		return nil, NewInternal("口令哈希格式非法: 缺少盐段")
	}
	salt, err := base64.RawStdEncoding.DecodeString(rest[0])
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 盐不是合法的 base64: %v", err))
	}
	if len(salt) < phcMinSaltBytes {
		return nil, NewInternal(fmt.Sprintf(
			"口令哈希格式非法: 盐太短（%d 字节，至少 %d）", len(salt), phcMinSaltBytes))
	}
	rest = rest[1:]

	// 盐通过之后才轮到哈希 —— 实测里"盐 2 字节 + 哈希 2 字节"报的是**盐**的错，
	// 顺序反了就会报错报错到别处。
	if len(rest) == 0 {
		return nil, NewInternal("口令哈希格式非法: 缺少哈希段")
	}
	if len(rest) > 1 {
		return nil, NewInternal("口令哈希格式非法: 哈希段之后还有多余字段")
	}
	sum, err := base64.RawStdEncoding.DecodeString(rest[0])
	if err != nil {
		return nil, NewInternal(fmt.Sprintf("口令哈希格式非法: 哈希不是合法的 base64: %v", err))
	}
	if len(sum) < phcMinOutputBytes {
		return nil, NewInternal(fmt.Sprintf(
			"口令哈希格式非法: 哈希太短（%d 字节，至少 %d）", len(sum), phcMinOutputBytes))
	}

	return &phcParams{
		algorithm: algorithm,
		version:   version,
		m:         m,
		t:         t,
		p:         p,
		salt:      salt,
		hash:      sum,
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 会话 token
// ─────────────────────────────────────────────────────────────────────────────

// SessionTokenBytes 是会话 token 的随机字节数（256 位，足以抵御离线爆破，
// 因此只需 SHA-256 存哈希）。
const SessionTokenBytes = 32

// NewSessionToken 签发一个新的会话 token（**明文，只在这一次返回**）。
func NewSessionToken(rng Random) string {
	bytes := make([]byte, SessionTokenBytes)
	rng.FillBytes(bytes)
	return hex.EncodeToString(bytes)
}

// HashSessionToken 计算会话 token 的存储形式。
//
// 用 SHA-256 而不是 argon2：token 是 256 位随机值，不存在"弱口令"问题，
// 而它每个请求都要校验一次 —— 用慢哈希会把并发能力白白吃掉。
func HashSessionToken(token string) string { return SHA256Hex([]byte(token)) }

// ─────────────────────────────────────────────────────────────────────────────
// 数据隔离
// ─────────────────────────────────────────────────────────────────────────────

// IsVisibleTo 报告资源是否对该用户可见。
//
// # 为什么做成一个函数
//
// 隔离规则一旦有多个副本，漏掉任何一处就是越权。全项目只走这一个判据，
// 新增查询时"忘了带 UserID"就只能发生在**没有调用它**的地方 ——
// 那在 review 时是可见的。
//
// # 为什么返回 bool 而不是 error
//
// "不属于你"与"不存在"必须给出**同一个对外结果**（404，不是 403）：
// 回 403 等于确认了"这个资源存在"，把账号 id 空间变成可枚举的。
// 返回 bool 是为了让调用方把两种情形收敛到同一个"没有"，
// 而不是各自决定要不要写一个 Forbidden。
//
// ⚠ 这里用普通 `==` 而不是常数时间比较是**对的**：用户 id 不是秘密
// （自己知道自己的 id），比较耗时泄漏不了任何东西。
func IsVisibleTo(owner, viewer UserID) bool { return owner == viewer }
