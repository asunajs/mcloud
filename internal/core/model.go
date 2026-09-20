// 领域模型 —— 对应 Rust 侧 legacy/crates/mcloud-core/src/model.rs。
//
// # 多账号是一等公民
//
// `Account` 覆盖余额、凭据到期、健康度、独立调度，而不是把账号当成一个附属字段。
//
// # ⚠⚠ 本文件里"字段的 JSON 名"是外部契约，一个都不能随手改
//
// Rust 侧这些结构体都 `#[derive(Serialize, Deserialize)]`，字段名默认是
// **snake_case**；而 Go 的默认规则是**原样输出字段名**（`CloudCurrent`）。
// 少写一个 tag 就是"同一个字段在库里有两个名字"，而它不报错。
//
// ⇒ 本文件每个字段都显式写了 `json:"…"`，与 `test/golden/enum-json-vectors.json`
// 里 serde 真跑出来的形状逐字对拍。新增字段时先去那份向量里确认名字。
//
// # Option 的形状：`null` 还是"字段消失"
//
// Rust 的 `Option<T>` 默认是 `null`，只有带 `skip_serializing_if` 的才省略。
// 两侧必须一致，所以判据是：**看 Rust 侧那个字段有没有 skip 属性** ——
//
//	| Rust 侧写法                              | Go 侧写法              |
//	| ---                                     | ---                  |
//	| `Option<T>`                             | `*T` + `json:"x"`     |
//	| `Option<T>` + `skip_serializing_if`     | `*T` + `,omitempty`   |
//	| `T`                                     | `T` + `json:"x"`      |
//
// ⚠ **`omitempty` 不能顺手加**：前端 `data.summary === null` 与
// `!('summary' in data)` 是两条不同的分支，加了就把其中一条永久关掉。
//
// # ⚠ 空数组的形状
//
// Rust 的 `Vec::new()` 是 `[]`，Go 的 nil slice 是 `null` ——
// 前端 `.map()` 在 null 上直接炸。所以 `account_ids` / `steps` / `bindings`
// 这类字段**构造函数一律给非 nil**，与 [RunReport.ManualSteps] 同一个理由。
package core

import (
	"encoding/json"
	"fmt"
	"time"
	"unicode/utf8"
)

// ─────────────────────────────────────────────────────────────────────────────
// 打印脱敏用的两个具名类型
//
// ⚠ 为什么是"字段级"而不是给 AccountState 写一个 String()：
// Go 的 `%+v` 会**展开字段、不走 struct 的 Stringer**，struct 级方法挡不住它。
// 把敏感字段做成有 Stringer/GoStringer 的类型，三条路（%v / %+v / %#v）才都堵住。
// ─────────────────────────────────────────────────────────────────────────────

// SecretValue 是可**直接复用**的凭据或设备特征。打印只显示"有没有 + 多长"。
//
// 与 Rust 侧 `AccountState` 手写 `Debug` 里的 `describe()` 同口径：
// 足以判断"状态是否被清空"，又不泄漏可复用的内容。
type SecretValue string

// String 实现 fmt.Stringer —— 一律脱敏。
//
// ⚠ 用 **rune 数**而不是字节数：Rust 那边是 `v.chars().count()`。
func (s SecretValue) String() string {
	return fmt.Sprintf("<redacted %d 字符>", utf8.RuneCountInString(string(s)))
}

// GoString 接住 `%#v` —— 那条路不走 Stringer。
func (s SecretValue) GoString() string { return s.String() }

// Value 返回真值。落库与上游请求一律走它（JSON 序列化**不受影响**：
// String/GoString 不是 encoding.TextMarshaler，encoding/json 直出底层值）。
func (s SecretValue) Value() string { return string(s) }

// NewSecretValue 从"拿到的"字符串构造一个凭据字段。
func NewSecretValue(v string) SecretValue { return SecretValue(v) }

// FullPhone 是**完整手机号**。
//
// # 它现在的角色：标识，不是"拼凭据的原料"
//
// `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`，
// 中间那段确实是完整手机号。但**不要拿它去拼** —— 整串 auth 由
// [AccountState.Authorization] 原样存着，且那一列**只装"拿到的"、不装"拼出来的"**。
//
// 这一列留下来是因为它有两个别处替代不了的用途：日志里认出"签的是谁"
// （掩码不够），以及排障时确认端与号对得上。
//
// # 打印是**掩码**而不是 `<redacted>`
//
// 手机号不是凭据，但打日志时没有任何理由用完整的。掩码之后排障还看得出
// "这是哪个账号"，整段抹掉就看不出了 —— 多账号时这个区别很要紧。
type FullPhone string

// String 返回掩码形式（见 [MaskPhone]）。
func (p FullPhone) String() string { return MaskPhone(string(p)) }

// GoString 接住 `%#v`。
func (p FullPhone) GoString() string { return p.String() }

// Value 返回真值。
func (p FullPhone) Value() string { return string(p) }

// UserDomainID 是上游的用户域标识（**纯标识，不是凭据**）。
//
// 单独成类型的理由只有一条：它要**带 Stringer**。用裸 `*string` 的话
// `%v` / `%+v` 只打印指针地址，而这个字段存在的全部理由是"排障时看得见是哪个主体"。
type UserDomainID string

// String 返回真值（实现 fmt.Stringer）。
func (u UserDomainID) String() string { return string(u) }

// GoString 接住 `%#v`。
func (u UserDomainID) GoString() string { return string(u) }

// ─────────────────────────────────────────────────────────────────────────────
// Account
// ─────────────────────────────────────────────────────────────────────────────

// Account 是账号快照。
//
// ⚠ 它**不含凭据** —— 凭据在 [AccountState]。分开放是刻意的：
// `Account` 会被 `/api/accounts` 直接吐给前端，而凭据不该被顺手序列化出去。
type Account struct {
	ID AccountID `json:"id"`
	// Owner 是归属的**系统用户**。nil = 未归属（v2 及更早的历史数据）。
	//
	// # 为什么是可选的而不是必有
	//
	// v2 及更早的库里已经有账号数据，而 schema 迁移是**纯 SQL**、
	// 执行时"首个管理员"还不存在（管理员要靠 `mcloud user init` 之后才有）。
	// 迁移期硬编一个 owner 只能靠猜，所以历史数据落到 nil（未归属），
	// 由初始化首个管理员时**认领**。
	//
	// ⚠ 语义上 nil 只能出现在"刚迁移完还没初始化管理员"这一个窗口里；
	// 隔离判据（[Account.IsVisibleTo]）对它的处理是**谁都看不见**，
	// 而不是"谁都看得见" —— **漏判的代价不对称**。
	Owner *UserID `json:"owner,omitempty"`
	// Nickname 是昵称（优先展示）。
	Nickname *string `json:"nickname"`
	// PhoneMasked 是脱敏手机号，形如 `19****01`。
	PhoneMasked *string `json:"phone_masked"`
	Platform    *string `json:"platform"`
	Enabled     bool    `json:"enabled"`
	SortOrder   int64   `json:"sort_order"`

	// ---- 云朵余额：原项目 `UserState.totalCloud` 从未被采集（写入被注释），这里是一等字段 ----
	CloudCurrent   *int64     `json:"cloud_current"`
	CloudUpdatedAt *time.Time `json:"cloud_updated_at"`

	// ---- 健康度 ----
	// AuthExpire 是凭据到期时间（**epoch 毫秒**）。
	//
	// ⚠ 单位是毫秒不是秒 —— 见 [AuthInfo] 那套约定。
	// 只暴露到期时间，**不暴露 token 本体**。
	AuthExpire *int64     `json:"auth_expire"`
	LastOKAt   *time.Time `json:"last_ok_at"`
	FailStreak int64      `json:"fail_streak"`

	Note *string `json:"note"`
}

// NewAccount 构造一个账号，`Enabled` 默认为 true（Rust 侧 `Account::new` 同款）。
func NewAccount(id string) Account { return Account{ID: NewAccountID(id), Enabled: true} }

// DisplayName 返回展示名：优先昵称，否则脱敏手机号，否则 ID。
//
// ⚠ 三级都可能是空串：`nickname` 为 `Some("")` 时 Rust 的 `as_deref().or(...)`
// **不会**跳到下一级（空串是 `Some`）。这里同口径返回空串 ——
// 顺手"判空再降级"会让用户自己设的空昵称被 ID 顶掉。
func (a Account) DisplayName() string {
	if a.Nickname != nil {
		return *a.Nickname
	}
	if a.PhoneMasked != nil {
		return *a.PhoneMasked
	}
	return a.ID.String()
}

// IsAuthExpiredAt 报告凭据是否已过期（`nowMillis` 为 **epoch 毫秒**）。
//
// nil = 未知，按**未过期**处理以免误报 —— 判成过期会让一个其实能用的账号
// 被强制重新登录，而判成没过期最多是稍后发现真的过期了。
func (a Account) IsAuthExpiredAt(nowMillis int64) bool {
	return a.AuthExpire != nil && *a.AuthExpire <= nowMillis
}

// IsVisibleTo 报告这个云盘账号对该系统用户是否可见。
//
// **未归属（Owner == nil）对谁都不可见** —— 迁移完还没初始化管理员的窗口里，
// 宁可是"账号暂时不出现"，也不要出现"谁都拿得到"。
func (a Account) IsVisibleTo(viewer UserID) bool {
	if a.Owner == nil {
		return false
	}
	return IsVisibleTo(*a.Owner, viewer)
}

// ─────────────────────────────────────────────────────────────────────────────
// RunRecord
// ─────────────────────────────────────────────────────────────────────────────

// RunRecord 是一次运行的历史记录。
//
// 与 [RunEvent] 的分工：那是**过程中**的事件流（给前端看进度），
// 这是**结束后**的一行（给历史列表看）。
type RunRecord struct {
	ID         string     `json:"id"`
	AccountID  *AccountID `json:"account_id"`
	Capability string     `json:"capability"`
	Status     RunStatus  `json:"status"`
	StartedAt  time.Time  `json:"started_at"`
	FinishedAt *time.Time `json:"finished_at"`
	// CloudGained 是本次净增云朵（**可负**）。
	CloudGained *int64 `json:"cloud_gained"`
	// CloudAfter 是运行后余额。
	CloudAfter *int64  `json:"cloud_after"`
	Message    *string `json:"message"`
}

// ─────────────────────────────────────────────────────────────────────────────
// 能力注册表
// ─────────────────────────────────────────────────────────────────────────────

// Group 是能力分组。
type Group int

const (
	GroupDaily Group = iota
	GroupActivity
	GroupQuery
	GroupExchange
	GroupMaintenance
)

// String 返回落库与 JSON 用的文本。未知值返回空串（枚举漏态守卫，见包注释）。
func (g Group) String() string {
	switch g {
	case GroupDaily:
		return "daily"
	case GroupActivity:
		return "activity"
	case GroupQuery:
		return "query"
	case GroupExchange:
		return "exchange"
	case GroupMaintenance:
		return "maintenance"
	}
	return ""
}

// ParseGroup 从存储里读回。未知值归零值（Daily）。
func ParseGroup(raw string) Group {
	switch raw {
	case "daily":
		return GroupDaily
	case "activity":
		return GroupActivity
	case "query":
		return GroupQuery
	case "exchange":
		return GroupExchange
	case "maintenance":
		return GroupMaintenance
	}
	return GroupDaily
}

// CapabilityKind 是能力性质。
type CapabilityKind int

const (
	// KindAction 表示有副作用（签到、领取、兑换）。
	KindAction CapabilityKind = iota
	// KindQuery 表示无副作用、可安全反复执行（查余额、查待领取奖品）。
	//
	// 这一类**不写运行历史、不产生日志噪音**，输出以结构化卡片呈现。
	KindQuery
)

// String 返回落库与 JSON 用的文本。
//
// ⚠ 未知值必须返回**空串**，不能写成 `if k == KindQuery { … }; return "action"`。
// 那种写法会让**任何**越界值都有名字，于是"已知态有几个"再也数不出来 ——
// 枚举漏态守卫（model_test.go 的真值表）就静默失效了。
// 两个变体的枚举最容易顺手写成 if/else，这里刻意用 switch。
func (k CapabilityKind) String() string {
	switch k {
	case KindAction:
		return "action"
	case KindQuery:
		return "query"
	}
	return ""
}

// ParseCapabilityKind 从存储里读回。
//
// ⚠ 未知值归 [KindAction]（**更保守的那一档**）：把一个查余额的能力误判成
// "有副作用"最多是它写了一条多余的历史；反过来会把一个真有副作用的能力
// 当成可随便重跑 —— 那会重复签到、重复兑换。
func ParseCapabilityKind(raw string) CapabilityKind {
	if raw == "query" {
		return KindQuery
	}
	return KindAction
}

// RiskLevel 是风险等级，影响默认并发与前端二次确认。
type RiskLevel int

const (
	RiskLow RiskLevel = iota
	// RiskMedium 表示消耗资源但可恢复（如领取）。
	RiskMedium
	// RiskHigh 表示不可逆消耗（如兑换扣云朵）—— 前端必须二次确认并显示影响面。
	RiskHigh
)

// String 返回落库与 JSON 用的文本。未知值返回空串。
func (r RiskLevel) String() string {
	switch r {
	case RiskLow:
		return "low"
	case RiskMedium:
		return "medium"
	case RiskHigh:
		return "high"
	}
	return ""
}

// ParseRiskLevel 从存储里读回。
//
// ⚠ 未知值归 [RiskHigh]（**要求确认的那一档**）：把低风险误判成高风险只是
// 多一次确认；反过来会把"兑换扣云朵"当成可随手点的按钮。
func ParseRiskLevel(raw string) RiskLevel {
	switch raw {
	case "low":
		return RiskLow
	case "medium":
		return RiskMedium
	case "high":
		return RiskHigh
	}
	return RiskHigh
}

// NeedsConfirmation 报告前端是否必须二次确认。
func (r RiskLevel) NeedsConfirmation() bool { return r == RiskHigh }

// AccountMode 是多账号执行方式。默认串行，防风控。
type AccountMode int

const (
	// ModeSequential 是串行。**零值**，与 Rust 的 `#[default]` 对齐 ——
	// "忘了指定"时拿到串行（慢但安全），不是并行。
	ModeSequential AccountMode = iota
	ModeParallel
)

// String 返回落库与 JSON 用的文本。未知值返回空串（理由同 [CapabilityKind.String]）。
func (m AccountMode) String() string {
	switch m {
	case ModeSequential:
		return "sequential"
	case ModeParallel:
		return "parallel"
	}
	return ""
}

// ParseAccountMode 从存储里读回。未知值归 [ModeSequential]。
func ParseAccountMode(raw string) AccountMode {
	if raw == "parallel" {
		return ModeParallel
	}
	return ModeSequential
}

// FieldKind 是参数控件类型。
type FieldKind int

const (
	FieldText FieldKind = iota
	FieldNumber
	FieldBool
	FieldSelect
	// FieldMultiSelect 是多选（账号选择走这一类）。
	//
	// ⚠ JSON 名是 `multi_select`（**带下划线**）—— serde 的 `snake_case`
	// 会插分隔符，而 `lowercase` 只压大小写。实测见黄金向量。
	FieldMultiSelect
	// FieldJSON 是兜底：复杂入参用原始 JSON。
	FieldJSON
)

// String 返回落库与 JSON 用的文本。未知值返回空串。
func (k FieldKind) String() string {
	switch k {
	case FieldText:
		return "text"
	case FieldNumber:
		return "number"
	case FieldBool:
		return "bool"
	case FieldSelect:
		return "select"
	case FieldMultiSelect:
		return "multi_select"
	case FieldJSON:
		return "json"
	}
	return ""
}

// ParseFieldKind 从存储里读回。未知值归 [FieldText]。
func ParseFieldKind(raw string) FieldKind {
	switch raw {
	case "text":
		return FieldText
	case "number":
		return FieldNumber
	case "bool":
		return FieldBool
	case "select":
		return FieldSelect
	case "multi_select":
		return FieldMultiSelect
	case "json":
		return FieldJSON
	}
	return FieldText
}

// SelectOption 是下拉/多选的一项。
type SelectOption struct {
	Label string `json:"label"`
	// Value 是这一项的值。用 [json.RawMessage] 而不是 `any`：
	// 保留**字面字节**，既不丢精度（见 §6.1 里 float64 那条），
	// 也不会因为重新编码而改变形状（前端靠它回传）。
	Value json.RawMessage `json:"value"`
	// Help 是该项的说明。`nil` 时**省略**（Rust 侧带 `skip_serializing_if`）。
	Help *string `json:"help,omitempty"`
}

// FieldSpec 是声明式字段描述。
//
// 前端表单与结果卡片**全部由它生成**，新增能力不需要改前端 ——
// 所以它的 JSON 形状是**跨端契约**。
type FieldSpec struct {
	Key      string          `json:"key"`
	Label    string          `json:"label"`
	Kind     FieldKind       `json:"kind"`
	Required bool            `json:"required"`
	Default  json.RawMessage `json:"default,omitempty"`
	Options  []SelectOption  `json:"options,omitempty"`
	Help     *string         `json:"help,omitempty"`
}

// NewFieldSpecText 构造一个文本字段。
func NewFieldSpecText(key, label string) FieldSpec {
	return FieldSpec{Key: key, Label: label, Kind: FieldText}
}

// NewFieldSpecBool 构造一个布尔字段，并写入默认值。
//
// ⚠ 默认值**总是**写（`true` 与 `false` 都写）：Rust 侧 `FieldSpec::bool`
// 无条件填 `Some(Value::Bool(default))`。用 `omitempty` 的话 `false`
// 会被当成"没有默认值"而消失 —— 而"默认关"恰恰是最常见的那种。
func NewFieldSpecBool(key, label string, def bool) FieldSpec {
	raw := "false"
	if def {
		raw = "true"
	}
	return FieldSpec{Key: key, Label: label, Kind: FieldBool, Default: json.RawMessage(raw)}
}

// MarkRequired 标记必填（链式）。
//
// ⚠ 名字里带 `Mark` 是因为 `Required` 已被字段占用 —— Rust 那边叫 `required()`
// 没有这个冲突（字段与方法不同命名空间），Go 里同名的字段与方法**不能共存**。
func (f FieldSpec) MarkRequired() FieldSpec { f.Required = true; return f }

// WithHelp 附加说明（链式）。
func (f FieldSpec) WithHelp(help string) FieldSpec { f.Help = &help; return f }

// WithDefault 设置默认值（链式）。
func (f FieldSpec) WithDefault(value json.RawMessage) FieldSpec { f.Default = value; return f }

// ─────────────────────────────────────────────────────────────────────────────
// AccountState（凭据）
// ─────────────────────────────────────────────────────────────────────────────

// AccountState 是账号登录态 —— 旧 `data/state.json` 里 `accounts[<手机号>]` 的内容。
//
// # 为什么单独一张表、单独一个类型
//
// 这里面是**可直接复用的凭据**（`jwtToken` 能直接冒充登录用户）。
// 把它和 [Account]（昵称/余额/健康度，会被 `/api/accounts` 直接吐给前端）
// 放在一起，等于每次列表查询都把凭据读进内存、随时可能被顺手序列化出去。
// 分表之后，"导出账号列表"这类操作天然拿不到凭据。
//
// # 打印一律脱敏
//
// 同 Rust 侧手写 `Debug` 的理由：否则任何一次 `log.Printf("%+v", state)`
// 都会把凭据写进日志。见 [SecretValue] 与 [FullPhone]。
type AccountState struct {
	AccountID AccountID    `json:"account_id"`
	JWTToken  *SecretValue `json:"jwt_token"`
	DeviceID  *SecretValue `json:"device_id"`
	// UserDomainID 是上游的用户域标识（**纯标识，不是凭据**），保留以便排障。
	//
	// ⚠ 它是这里唯一一个**不脱敏**的字段：整段抹掉之后排障就看不出是哪个主体。
	// ⚠ 用带 Stringer 的具名类型而不是 `*string`：裸指针在 `%v` / `%+v` 里
	// 只打印**地址**（`0x6978…`），而保留这一列的全部理由就是"排障时看得见"。
	UserDomainID *UserDomainID `json:"user_domain_id"`
	// RedpackToken 是话费红包活动专用 token。
	RedpackToken *SecretValue `json:"cloud_phone_redpack_token"`
	// Phone 是完整手机号（v6 起）。见 [FullPhone]。
	//
	// `v6` 之前登录的账号这一列是 `NULL` —— 那是**如实记录"当时没存"**，
	// 不要拿掩码去填。
	Phone *FullPhone `json:"phone"`
	// Authorization 是**原样存下的** `Authorization` 头值（`Basic …`，v7 起）。
	//
	// # ⚠⚠ 这一列只装「**拿到的**」，不装「拼出来的」
	//
	// 合法的来源只有两个，都是"服务端或旧配置原样给的"：
	//  1. 登录响应里那个 `auth` 整串；
	//  2. 旧项目 `asign.json` 的 `auth` 字段（迁移回填）。
	//
	// **不要**把本地合成的串写进来。读取侧会**优先采信**它，而整串路径
	// **不校验前缀**（前缀就在串里，没有可校验的对象）—— 于是一串拼错的凭据
	// 一旦落库，就会被永久当成原文使用，现象只是"忽然回 90001 未登录"。
	//
	// 补录手机号时只写 [AccountState.Phone] 即可 —— 下次照旧现拼，前缀闸照旧生效。
	Authorization *SecretValue `json:"authorization"`
	UpdatedAt     *time.Time   `json:"updated_at"`
}

// NewAccountState 构造一个空登录态。
func NewAccountState(accountID string) AccountState {
	return AccountState{AccountID: NewAccountID(accountID)}
}

// IsEmpty 报告是否一个字段都没有 —— 空状态没有迁移价值，导入时应被跳过。
//
// ⚠ [AccountState.Phone] 与 [AccountState.Authorization] **都算内容**：
// 前者是 `mcloud signin --account` 的唯一来源（判成空就会被导入流程跳过），
// 后者是 v7 之后真正拿来用的那个凭据。
func (s AccountState) IsEmpty() bool {
	return s.JWTToken == nil &&
		s.DeviceID == nil &&
		s.UserDomainID == nil &&
		s.RedpackToken == nil &&
		s.Phone == nil &&
		s.Authorization == nil
}

// ─────────────────────────────────────────────────────────────────────────────
// SmidState（设备特征）
// ─────────────────────────────────────────────────────────────────────────────

// SmidState 是账号的 smid 状态：那份绑定账号的设备特征，与最近一次上报换回的 `deviceId`。
//
// # 为什么不并进 [AccountState]
//
// 同一个主体（这个账号在这台设备上的状态）、同一张表，但**写入契约相反**：
// `AccountState` 是"整体覆盖的凭据快照"（重新登录要把失效的 token 冲掉），
// 而这里的两列必须**活过重新登录** —— 换凭据不该换掉设备长相，换长相也不该动凭据。
// 放进同一个结构里，"覆盖"就会顺手把特征清掉（登录路径根本不知道它的存在）。
//
// # ⚠⚠ 刻意**不可**序列化
//
// Rust 侧刻意**不派生 `Serialize`**：这两样都不是能吐给前端的东西，
// `device_id` 是第三方风控服务给这台设备的标识。少一个 `Serialize`，
// 就少一条"顺手塞进响应"的路。
//
// Go 没有"不实现 trait"这种表达（任何结构体都能被 `json.Marshal`），
// 所以这里用 [SmidState.MarshalJSON] **主动报错**来近似 ——
// Rust 那边的表现是编译错误，这里是运行时错误，弱一档但仍是**响的**：
// 一处误用会立刻红，而不是静默把设备特征写进响应。
type SmidState struct {
	AccountID AccountID `json:"-"`
	// FingerprintJSON 是绑定账号的设备特征（JSON 文本）。nil = 还没绑过。
	//
	// 存 JSON 串而不是强类型，是刻意的：类型放在 core 会让 core 反向依赖协议层。
	// 代价是"库里那串读不出来"必须由调用方处理 —— 那里的规矩是**报错，
	// 不是悄悄重掷一份**。
	FingerprintJSON *SecretValue `json:"-"`
	// DeviceID 是最近一次上报换回的 `deviceId`（含 `B` 前缀）。**会变**，
	// 只用作下一次上报的 `box`。
	DeviceID *SecretValue `json:"-"`
}

// NewSmidState 构造一个空设备状态。
func NewSmidState(accountID string) SmidState {
	return SmidState{AccountID: NewAccountID(accountID)}
}

// IsEmpty 报告两样都没有 —— 这个账号还从没上报过指纹。
func (s SmidState) IsEmpty() bool {
	return s.FingerprintJSON == nil && s.DeviceID == nil
}

// MarshalJSON 一律报错 —— 见类型注释里"刻意不可序列化"那条。
//
// ⚠ 不要为了"让它能用"把这里删掉。若哪天确实需要落库，
// 正确做法是**由存储层逐列写**（那正是 Rust 侧 `StateStore` 的做法），
// 而不是给整个结构体开一个 JSON 出口。
func (SmidState) MarshalJSON() ([]byte, error) {
	return nil, fmt.Errorf("SmidState 刻意不可序列化：它含第三方风控的设备特征，不该进响应或单条 JSON 列；请由存储层逐列写")
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务
// ─────────────────────────────────────────────────────────────────────────────

// ScheduleTargetKind 是定时任务执行目标的两类。
type ScheduleTargetKind int

const (
	// TargetCapability 是单个能力，值是能力 code（旧版的"任务子项目名"）。
	TargetCapability ScheduleTargetKind = iota
	// TargetPipeline 是任务组合。
	TargetPipeline
)

// String 返回 JSON 里的 `kind` 标签。未知值返回空串。
func (k ScheduleTargetKind) String() string {
	if k == TargetPipeline {
		return "pipeline"
	}
	return "capability"
}

// ScheduleTarget 是定时任务的执行目标（旧版的 `task?` 与 `pipelineId?` 二选一）。
//
// # ⚠ Go 侧的形状取舍
//
// Rust 是 `#[serde(tag = "kind")]` 的内部标签枚举。Go 没有 sum type，
// 这里用"结构体 + Kind"，靠 [ScheduleTarget.MarshalJSON] 分派，
// 对外形状**仍然**是 `{"kind":"capability","code":"sign"}`。
//
// ⚠ `tag` 属性**漏不得**：漏了 serde 会退回默认的**外部标签**
// （`{"capability":{"code":"sign"}}`），与库里已存的形状完全不同。
// 我在写黄金向量生成器时就漏过一次 —— 这个错类型系统抓不到。
type ScheduleTarget struct {
	Kind ScheduleTargetKind `json:"-"`
	// Code 是能力 code。仅 [TargetCapability] 用。
	Code string `json:"-"`
	// ID 是组合 id。仅 [TargetPipeline] 用。
	ID string `json:"-"`
}

// NewScheduleTargetCapability 构造"单个能力"目标。
func NewScheduleTargetCapability(code string) ScheduleTarget {
	return ScheduleTarget{Kind: TargetCapability, Code: code}
}

// NewScheduleTargetPipeline 构造"任务组合"目标。
func NewScheduleTargetPipeline(id string) ScheduleTarget {
	return ScheduleTarget{Kind: TargetPipeline, ID: id}
}

// MarshalJSON 按 Kind 分派，产出 `{"kind":"capability","code":…}` /
// `{"kind":"pipeline","id":…}`。走 [marshalJSON] 以免 HTML 转义。
func (t ScheduleTarget) MarshalJSON() ([]byte, error) {
	switch t.Kind {
	case TargetCapability:
		return marshalJSON(struct {
			Kind string `json:"kind"`
			Code string `json:"code"`
		}{t.Kind.String(), t.Code})
	case TargetPipeline:
		return marshalJSON(struct {
			Kind string `json:"kind"`
			ID   string `json:"id"`
		}{t.Kind.String(), t.ID})
	}
	return nil, fmt.Errorf("未知的调度目标类型 %d", int(t.Kind))
}

// UnmarshalJSON 是 [ScheduleTarget.MarshalJSON] 的逆。
func (t *ScheduleTarget) UnmarshalJSON(data []byte) error {
	var v struct {
		Kind string `json:"kind"`
		Code string `json:"code"`
		ID   string `json:"id"`
	}
	if err := json.Unmarshal(data, &v); err != nil {
		return err
	}
	switch v.Kind {
	case "capability":
		t.Kind, t.Code, t.ID = TargetCapability, v.Code, ""
		return nil
	case "pipeline":
		t.Kind, t.Code, t.ID = TargetPipeline, "", v.ID
		return nil
	}
	return fmt.Errorf("未知的调度目标 kind %q", v.Kind)
}

// ScheduleStatus 是定时任务的上次执行结果。
type ScheduleStatus int

const (
	ScheduleOK ScheduleStatus = iota
	ScheduleError
	// ScheduleSkipped 表示到点触发时同「任务+账号」已在运行，本次跳过（**不排队**）。
	ScheduleSkipped
)

// String 返回落库与 JSON 用的文本。未知值返回空串。
func (s ScheduleStatus) String() string {
	switch s {
	case ScheduleOK:
		return "ok"
	case ScheduleError:
		return "error"
	case ScheduleSkipped:
		return "skipped"
	}
	return ""
}

// ParseScheduleStatus 从存储里读回。未知值归 [ScheduleError] ——
// "上次成功"会让一次真失败被历史掩盖，反过来只是多提示一次。
func ParseScheduleStatus(raw string) ScheduleStatus {
	switch raw {
	case "ok":
		return ScheduleOK
	case "skipped":
		return ScheduleSkipped
	}
	return ScheduleError
}

// Schedule 是定时任务定义（旧 `data/schedules.json` → `schedules` 表）。
type Schedule struct {
	ID    string  `json:"id"`
	Owner *UserID `json:"owner,omitempty"`
	Name  string  `json:"name"`
	// Target 是执行目标。nil 即无法执行 —— 导入时会作为问题报出，而不是猜一个目标。
	Target *ScheduleTarget `json:"target"`
	// Cron 是 5 段或 6 段（含秒）。
	Cron string `json:"cron"`
	// Timezone 归一化后一定非空；旧版缺省 `Asia/Shanghai`。
	Timezone string `json:"timezone"`
	// AccountIDs 是目标账号。**空表示全部账号**（不是"没有账号"）。
	//
	// ⚠ 空的时候是 `[]` 不是 `null` —— 见包注释"空数组的形状"。
	AccountIDs []AccountID `json:"account_ids"`
	// Body 是透传给能力的参数。
	Body    json.RawMessage `json:"body"`
	Enabled bool            `json:"enabled"`

	CreatedAt  *time.Time      `json:"created_at"`
	LastRunAt  *time.Time      `json:"last_run_at"`
	LastStatus *ScheduleStatus `json:"last_status"`
	LastError  *string         `json:"last_error"`
}

// NewSchedule 构造一个定时任务，`AccountIDs` 给非 nil（空是 `[]` 不是 `null`）。
func NewSchedule(id, name, cron, timezone string) Schedule {
	return Schedule{
		ID: id, Name: name, Cron: cron, Timezone: timezone,
		AccountIDs: []AccountID{},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务组合
// ─────────────────────────────────────────────────────────────────────────────

// PipelineStatus 是组合的终态。
//
// **与 [ScheduleStatus] 刻意不同**：旧版的组合没有 `skipped`，
// 却多一个 `partial`（部分步骤失败但没中断）。别把两者合并成一个枚举。
type PipelineStatus int

const (
	PipelineOK PipelineStatus = iota
	PipelineError
	// PipelinePartial 表示有步骤失败但未中断后续步骤。
	PipelinePartial
)

// String 返回落库与 JSON 用的文本。未知值返回空串。
func (s PipelineStatus) String() string {
	switch s {
	case PipelineOK:
		return "ok"
	case PipelineError:
		return "error"
	case PipelinePartial:
		return "partial"
	}
	return ""
}

// ParsePipelineStatus 从存储里读回。未知值归 [PipelineError]。
func ParsePipelineStatus(raw string) PipelineStatus {
	switch raw {
	case "ok":
		return PipelineOK
	case "partial":
		return PipelinePartial
	}
	return PipelineError
}

// PipelineStep 是组合中的单个步骤。
type PipelineStep struct {
	// Task 是能力 code。
	Task string          `json:"task"`
	Body json.RawMessage `json:"body"`
	// ContinueOnError 是该步失败后是否继续；nil 表示取组合级 `stop_on_error` 的反值。
	ContinueOnError *bool `json:"continue_on_error"`
}

// Pipeline 是任务组合定义（旧 `data/pipelines.json` → `pipelines` 表）。
type Pipeline struct {
	ID          string  `json:"id"`
	Owner       *UserID `json:"owner,omitempty"`
	Name        string  `json:"name"`
	Description *string `json:"description"`
	// Steps 是有序步骤，执行顺序即数组顺序。
	Steps []PipelineStep `json:"steps"`
	// AccountIDs 是目标账号。**空表示全部账号。**
	AccountIDs  []AccountID `json:"account_ids"`
	AccountMode AccountMode `json:"account_mode"`
	StopOnError bool        `json:"stop_on_error"`

	CreatedAt      *time.Time      `json:"created_at"`
	UpdatedAt      *time.Time      `json:"updated_at"`
	LastRunAt      *time.Time      `json:"last_run_at"`
	LastStatus     *PipelineStatus `json:"last_status"`
	LastError      *string         `json:"last_error"`
	LastDurationMS *int64          `json:"last_duration_ms"`
}

// NewPipeline 构造一个组合，`Steps` / `AccountIDs` 给非 nil（空是 `[]` 不是 `null`）。
func NewPipeline(id, name string) Pipeline {
	return Pipeline{ID: id, Name: name, Steps: []PipelineStep{}, AccountIDs: []AccountID{}}
}

// ContinuesAfterFailure 报告某一步失败后是否继续执行后续步骤
// （步骤级优先，其次取组合级 `stop_on_error` 的**反值**）。
//
// ⚠ 是**反值**：`stopOnError=true` 意味着"遇到错就停"，所以未指定时**不继续**。
// 少写那个 `!` 会让"严格模式"变成"一路冲到底"，而它在只有一步失败的场景里
// 表现完全正常 —— 要到第二步也失败时才看得出。
func (p Pipeline) ContinuesAfterFailure(step PipelineStep) bool {
	if step.ContinueOnError != nil {
		return *step.ContinueOnError
	}
	return !p.StopOnError
}

// ─────────────────────────────────────────────────────────────────────────────
// 定时抢兑
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeBinding 是「账号 × 奖品」绑定。
type ExchangeBinding struct {
	AccountID AccountID `json:"account_id"`
	PrizeID   int64     `json:"prize_id"`
	// PrizeName 仅展示用。
	PrizeName *string `json:"prize_name"`
}

// DefaultExchangeWindows 是抢兑窗口的默认值（旧 `exchange-schedule.ts` 的 `DEFAULT_WINDOWS`）。
//
// 这两个时刻是 10 点与 12 点开抢前的 20 秒 —— **提前量由旧版实测得出，不是随手写的。**
var DefaultExchangeWindows = []string{"09:59:40", "11:59:40"}

// ExchangeConfig 是定时抢兑配置（旧 `data/exchange.json`，单行表）。
type ExchangeConfig struct {
	Enabled bool `json:"enabled"`
	// Windows 是抢兑窗口时刻（`HH:mm:ss`，本地时区），已排序去重。
	Windows []string `json:"windows"`
	// Bindings 是账号 × 奖品绑定。
	Bindings []ExchangeBinding `json:"bindings"`
	// ScheduleIDs 是由本模块创建并维护的定时任务 id。
	//
	// 必须存下来：保存配置时要整体重建这些任务，没有这份清单就删不掉上一次留下的。
	ScheduleIDs []string   `json:"schedule_ids"`
	UpdatedAt   *time.Time `json:"updated_at"`
}

// NewExchangeConfig 返回默认配置：默认**关闭**，窗口取 [DefaultExchangeWindows]。
//
// ⚠ 默认关闭不是保守，是**必须**：抢兑会真的扣云朵，默认开启等于
// 用户一升级就自动开始花钱。
func NewExchangeConfig() ExchangeConfig {
	windows := make([]string, len(DefaultExchangeWindows))
	copy(windows, DefaultExchangeWindows)
	return ExchangeConfig{
		Enabled:     false,
		Windows:     windows,
		Bindings:    []ExchangeBinding{},
		ScheduleIDs: []string{},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 界面偏好
// ─────────────────────────────────────────────────────────────────────────────

// WebuiPages 是允许记忆的页面标识（与前端左侧菜单一致）。
var WebuiPages = []string{"tasks", "pipelines", "schedules", "accounts", "history", "config"}

// DefaultWebuiPage 是界面偏好默认页。
const DefaultWebuiPage = "tasks"

// WebuiConfig 是界面偏好（旧 `data/webui.json`，单行表）。
//
// 存在服务端而不是浏览器 `localStorage`：这是旧版的既有选择，
// 好处是换浏览器/换设备后偏好还在。
type WebuiConfig struct {
	RememberLastPage bool       `json:"remember_last_page"`
	LastPage         string     `json:"last_page"`
	LogExpanded      bool       `json:"log_expanded"`
	BackgroundRun    bool       `json:"background_run"`
	UpdatedAt        *time.Time `json:"updated_at"`
}

// NewWebuiConfig 返回默认偏好。
func NewWebuiConfig() WebuiConfig {
	return WebuiConfig{RememberLastPage: true, LastPage: DefaultWebuiPage}
}

// NormalizePage 把 `last_page` 收拢到白名单内（非法值回落默认）。
//
// ⚠ 这是**安全**检查不只是整洁：`last_page` 会进前端的路由，
// 不收白名单就是一个不受限的路径输入。
func (c *WebuiConfig) NormalizePage() {
	for _, p := range WebuiPages {
		if c.LastPage == p {
			return
		}
	}
	c.LastPage = DefaultWebuiPage
}
