// 旧项目（mcloud-sign）数据文档的读取与迁移映射 —— **纯逻辑，无 IO**。
//
// 对应 Rust 侧 legacy/crates/mcloud-core/src/legacy.rs。
//
// # 为什么单独一层
//
// 迁移的失败方式是**静默的**：把历史记录挂到错误的账号上不会报错，只会让人看到
// 一份"看起来正常但串了号"的数据；把「旧版其实从未生效」的定时任务照搬过来，
// 更会让人以为它一直在跑。所以这里不碰文件，只做两件可被穷尽测试的事：
//
//  1. **原样还原旧文档的形状**（字段名/可选性），并用真实数据生成的脱敏夹具钉住；
//  2. **逐条复刻旧版的规整规则与查找规则**，把"旧版会怎么做"和"新版应该怎么做"
//     分开表达，差异点显式暴露成 [ReferenceAdvice]，交给迁移报告而不是静默决定。
//
// 夹具：`test/fixtures/legacy/`，由 `scripts/gen-legacy-fixtures.cjs`
// 从真实数据脱敏生成。
//
// # 旧文档一览
//
// | 文件 | 内容 | 谁在读它 |
// |---|---|---|
// | `asign.json` | 账号（`auth` + `nickname`） | 账号来源，唯一的"真配置" |
// | `data/state.json` | 各账号 `jwtToken`/`deviceId`/… | 键是**手机号原文**（旧 `accountKey`） |
// | `data/history.json` | 运行历史（最多 200 条） | `account` 存的是**展示名**，不是标识 |
// | `data/pipelines.json` | 任务组合 | 有已废弃字段需要迁移 |
// | `data/schedules.json` | 定时任务 | |
// | `data/exchange.json` | 定时抢兑 | 窗口 `HH:mm:ss` → 6 段 cron |
// | `data/webui.json` | 界面偏好 | |
//
// # 三个必须知道的旧版怪癖（**不要"顺手修好"**）
//
//  1. **`state.json` 的键是手机号原文**，而 `history.json` 的 `account` 是展示名 ——
//     两者指向同一个账号却用了两套标识，迁移必须分别映射（见 [LegacyAccountView]）。
//  2. **纯数字字符串一律被当作"序号"**：旧 `findAccount` 里 `/^\d+$/` 分支在昵称匹配
//     之前，于是把手机号当字符串传进去**永远匹配不到**，定时任务会每次记
//     `账号 x 不存在` 后什么都不做。
//  3. `webui` 的两个布尔字段默认值方向相反：`rememberLastPage` 只要不是 `false`
//     就是 `true`，而 `logExpanded` 必须严格等于 `true`。见 [NormalizeWebui]。
//
// # ⚠ 这一层的字段名是**外部契约**，而且写错了不会报错
//
// 旧文档的所有字段都带 `#[serde(default)]` ⇒ **字段名写错时解析照常成功，
// 只是静默读到零值**。表现不是"解析失败"，而是"历史数据读出来是空的"。
// 所以字段名由第九份黄金向量 `legacy-shape-vectors.json`（由**真实的**
// mcloud-core 的 serde 产出）逐条比对，不靠人眼。
package core

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
)

// LegacyHistoryCap 是旧版历史文件的上限（`runner.ts`：超过就 `slice(-200)`）。
const LegacyHistoryCap = 200

// ─────────────────────────────────────────────────────────────────────────────
// 1. 旧文档类型
// ─────────────────────────────────────────────────────────────────────────────

// LegacyConfig 是旧 `asign.json`。
//
// 账号字段**刻意全为可选**：`nickname`/`phone` 在真实配置里经常缺席，
// 缺席时要从 `auth` 现场解析 —— 这正是 [ResolvePhone] 存在的理由。
type LegacyConfig struct {
	Version *int64                `json:"version"`
	Caiyun  []LegacyAccountConfig `json:"caiyun"`
}

// LegacyAccountConfig 是旧配置里的一个账号。
//
// `Auth` 是 `Basic base64("平台:手机号:token")`。
type LegacyAccountConfig struct {
	Auth     string  `json:"auth"`
	Nickname *string `json:"nickname,omitempty"`
	Phone    *string `json:"phone,omitempty"`
}

// NicknameOrNone 返回该账号的昵称（空串按缺席处理，对齐 JS 的 falsy）。
func (c LegacyAccountConfig) NicknameOrNone() *string {
	if c.Nickname == nil || *c.Nickname == "" {
		return nil
	}
	return c.Nickname
}

// UnmarshalJSON 把 `caiyun` 归一化成非 nil 的空切片。
//
// ⚠ 必须归一化：Rust 的 `Vec::new()` 序列化成 `[]`，Go 的 nil slice 序列化成
// `null`。而这一层的**输出形状就是外部契约**（迁移报告要原样写回旧文档），
// 差一个字符就会让"读回来再看"的那条路径读到 null。
//
// `type plain LegacyConfig` 是 Go 里绕开 `UnmarshalJSON` 递归的标准写法：
// 新类型不继承方法，于是解码走默认路径。
func (c *LegacyConfig) UnmarshalJSON(data []byte) error {
	type plain LegacyConfig
	var v plain
	if err := decodeJSONStrict(data, &v); err != nil {
		return err
	}
	*c = LegacyConfig(v)
	if c.Caiyun == nil {
		c.Caiyun = []LegacyAccountConfig{}
	}
	return nil
}

// LegacyStateFile 是旧 `data/state.json`：`{ accounts: { <手机号原文>: … } }`。
//
// ⚠ 那层的**键是数据**（旧 `accountKey`），不是字段名。
type LegacyStateFile struct {
	Accounts map[string]LegacyAccountState `json:"accounts"`
}

// LegacyAccountState 是旧 `UserState`。
//
// `Unknown` 用"剩下的都收进来"兜住**旧版本写过、现在不再产生**的字段
// （例如 `totalCloud`：旧代码把它的写入注释掉了，所以真实数据里没有，
// 但别人机器上可能有）。迁移时这些字段会被原样保留进"未识别字段"清单并报告，
// 而不是静默丢弃。
//
// ⚠ Go 没有 serde 的 `#[serde(flatten)]`，只能手写 [LegacyAccountState.UnmarshalJSON]。
// 手写就意味着"已知字段清单"在这份代码里有第二份副本 —— 所以向量里专门有一节
// `$fixtureKeys` 钉住夹具里实际出现的键名，两边对不上就会红。
type LegacyAccountState struct {
	JWTToken               *string `json:"jwtToken"`
	DeviceID               *string `json:"deviceId"`
	UserDomainID           *string `json:"userDomainId"`
	CloudPhoneRedpackToken *string `json:"cloudPhoneRedpackToken"`
	// Unknown 是未识别字段。键名按字典序（Rust 侧是 `BTreeMap`）。
	Unknown map[string]any `json:"-"`
}

// UnmarshalJSON 手工实现 flatten：已知键各自解析，其余进 [LegacyAccountState.Unknown]。
func (s *LegacyAccountState) UnmarshalJSON(data []byte) error {
	var raw map[string]json.RawMessage
	if err := decodeJSONStrict(data, &raw); err != nil {
		return err
	}
	s.JWTToken, s.DeviceID, s.UserDomainID, s.CloudPhoneRedpackToken = nil, nil, nil, nil
	s.Unknown = nil

	targets := map[string]**string{
		"jwtToken":               &s.JWTToken,
		"deviceId":               &s.DeviceID,
		"userDomainId":           &s.UserDomainID,
		"cloudPhoneRedpackToken": &s.CloudPhoneRedpackToken,
	}
	for key, chunk := range raw {
		dst, known := targets[key]
		if !known {
			var value any
			if err := decodeJSONStrict(chunk, &value); err != nil {
				return err
			}
			if s.Unknown == nil {
				s.Unknown = make(map[string]any)
			}
			s.Unknown[key] = value
			continue
		}
		var text *string
		if err := decodeJSONStrict(chunk, &text); err != nil {
			return err
		}
		*dst = text
	}
	return nil
}

// IsEmpty 报告是否一个凭据都没有（全空状态没有迁移价值）。
func (s LegacyAccountState) IsEmpty() bool {
	return derefOrEmpty(s.JWTToken) == "" &&
		derefOrEmpty(s.DeviceID) == "" &&
		derefOrEmpty(s.UserDomainID) == "" &&
		derefOrEmpty(s.CloudPhoneRedpackToken) == ""
}

// UnknownFields 返回出现过的未识别字段名（迁移报告用）。
//
// ⚠ 按字典序返回：Rust 侧的容器是 `BTreeMap`（有序），而 Go 的 map 无序 ——
// 不排序的话"先报哪个字段"每次运行都可能不同，而报错内容是会进报告的。
func (s LegacyAccountState) UnknownFields() []string {
	if len(s.Unknown) == 0 {
		return nil
	}
	out := make([]string, 0, len(s.Unknown))
	for k := range s.Unknown {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// LegacyHistoryFile 是旧 `data/history.json`。
type LegacyHistoryFile struct {
	Runs []LegacyRun `json:"runs"`
}

// LegacyRun 是旧 `RunRecord`。
type LegacyRun struct {
	// Task 是任务子项目名（`sign` / `prizes` / `exchange` …）。
	Task string `json:"task"`
	// Account 是**展示名**（昵称优先，否则脱敏手机号）—— 不是账号标识。见包注释怪癖 1。
	Account    string           `json:"account"`
	OK         bool             `json:"ok"`
	Start      string           `json:"start"`
	End        string           `json:"end"`
	DurationMS *int64           `json:"durationMs"`
	Logs       []LegacyLogEntry `json:"logs"`
	Result     *any             `json:"result"`
	Error      *string          `json:"error"`
}

// UnmarshalJSON 把 `logs` 归一化成非 nil 的空切片（理由见 [LegacyConfig.UnmarshalJSON]）。
func (r *LegacyRun) UnmarshalJSON(data []byte) error {
	type plain LegacyRun
	var v plain
	if err := decodeJSONStrict(data, &v); err != nil {
		return err
	}
	*r = LegacyRun(v)
	if r.Logs == nil {
		r.Logs = []LegacyLogEntry{}
	}
	return nil
}

// LegacyLogEntry 是旧日志条目（`PushLogEntry`）。
type LegacyLogEntry struct {
	// Level 是数值严重级。真实数据里只出现过 `0`（error）/ `3`（info）/ `4`（debug）。
	Level *int64 `json:"level"`
	// Kind 是语义类型：`info` / `start` / `success` / `debug` / `error`。
	//
	// **同一个 `level` 会对应多个 `type`**（`3` 同时对应 info/start/success），
	// 所以语义必须看 `type`，不能只看 `level`。
	Kind *string `json:"type"`
	Msg  *string `json:"msg"`
	Date *string `json:"date"`
}

// LegacyPipelinesFile 是旧 `data/pipelines.json`。
type LegacyPipelinesFile struct {
	Pipelines []LegacyPipeline `json:"pipelines"`
}

// LegacyPipeline 是旧 `PipelineDef`。
type LegacyPipeline struct {
	ID          string       `json:"id"`
	Name        string       `json:"name"`
	Description *string      `json:"description"`
	Steps       []LegacyStep `json:"steps"`
	// LegacyAccount 是**已废弃**的单账号。读取时迁移为 `accounts`
	// （见 [NormalizePipelineAccounts]）。
	LegacyAccount  *any    `json:"account"`
	Accounts       *[]any  `json:"accounts"`
	AccountMode    *string `json:"accountMode"`
	StopOnError    *bool   `json:"stopOnError"`
	CreatedAt      *string `json:"createdAt"`
	UpdatedAt      *string `json:"updatedAt"`
	LastRunAt      *string `json:"lastRunAt"`
	LastStatus     *string `json:"lastStatus"`
	LastError      *string `json:"lastError"`
	LastDurationMS *int64  `json:"lastDurationMs"`
}

// UnmarshalJSON 把 `steps` 归一化成非 nil 的空切片（理由见 [LegacyConfig.UnmarshalJSON]）。
func (p *LegacyPipeline) UnmarshalJSON(data []byte) error {
	type plain LegacyPipeline
	var v plain
	if err := decodeJSONStrict(data, &v); err != nil {
		return err
	}
	*p = LegacyPipeline(v)
	if p.Steps == nil {
		p.Steps = []LegacyStep{}
	}
	return nil
}

// LegacyStep 是旧 `PipelineStep`。
type LegacyStep struct {
	Task string `json:"task"`
	Body *any   `json:"body"`
	// ContinueOnError 是该步失败后是否继续；缺省取组合级 `stopOnError` 的反值。
	ContinueOnError *bool `json:"continueOnError"`
	// LegacyAccount 是**已废弃**的步骤级账号。旧 `normalizeStored` 会显式丢掉它，这里也丢。
	LegacyAccount *any `json:"account"`
}

// LegacySchedulesFile 是旧 `data/schedules.json`。
type LegacySchedulesFile struct {
	Schedules []LegacySchedule `json:"schedules"`
}

// LegacySchedule 是旧 `ScheduleDef`。
type LegacySchedule struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	// Task 是任务子项目名（与 `pipelineId` 二选一）。
	Task       *string `json:"task"`
	PipelineID *string `json:"pipelineId"`
	// Cron 是 5 段或 6 段（含秒）cron。
	Cron     string  `json:"cron"`
	Timezone *string `json:"timezone"`
	// Account 是目标账号：序号（数字或纯数字字符串）/ 昵称。见包注释怪癖 2。
	Account    *any    `json:"account"`
	Body       *any    `json:"body"`
	Enabled    *bool   `json:"enabled"`
	CreatedAt  *string `json:"createdAt"`
	LastRunAt  *string `json:"lastRunAt"`
	LastStatus *string `json:"lastStatus"`
	LastError  *string `json:"lastError"`
}

// LegacyExchangeFile 是旧 `data/exchange.json`。
type LegacyExchangeFile struct {
	Enabled  bool            `json:"enabled"`
	Windows  []string        `json:"windows"`
	Bindings []LegacyBinding `json:"bindings"`
	// ScheduleIDs 由该模块创建并维护的定时任务 id（迁移时用来识别"自动生成"的调度）。
	ScheduleIDs []string `json:"scheduleIds"`
	UpdatedAt   *string  `json:"updatedAt"`
}

// LegacyBinding 是旧 `ExchangeBinding`。
type LegacyBinding struct {
	Account   *any    `json:"account"`
	PrizeID   *any    `json:"prizeId"`
	PrizeName *string `json:"prizeName"`
}

// LegacyWebuiFile 是旧 `data/webui.json`。
type LegacyWebuiFile struct {
	RememberLastPage *bool   `json:"rememberLastPage"`
	LastPage         *string `json:"lastPage"`
	LogExpanded      *bool   `json:"logExpanded"`
	BackgroundRun    *bool   `json:"backgroundRun"`
	UpdatedAt        *string `json:"updatedAt"`
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 旧版规整规则（逐条复刻）
// ─────────────────────────────────────────────────────────────────────────────

// LegacyErrorKind 是迁移期错误的四类。
type LegacyErrorKind int

const (
	// LegacyErrBadExchangeWindow 表示时刻格式不是 `HH:mm:ss`。
	LegacyErrBadExchangeWindow LegacyErrorKind = iota
	// LegacyErrNoExchangeWindow 表示一个抢兑时刻都没有。
	LegacyErrNoExchangeWindow
	LegacyErrBadPrizeID
	LegacyErrMaskCollision
)

// LegacyError 是迁移期的错误。
//
// **每一条都必须能指向一个具体的、可人工修复的问题。**
type LegacyError struct {
	Kind LegacyErrorKind
	// Window 是 [LegacyErrBadExchangeWindow] 的细节。
	Window string
	// PrizeID 是 [LegacyErrBadPrizeID] 的细节。
	PrizeID string
	// AccountID / Keys 是 [LegacyErrMaskCollision] 的细节。
	AccountID string
	Keys      []string
}

// Error 实现 error。文案与 Rust 侧 `#[error(…)]` 逐字对齐。
func (e *LegacyError) Error() string {
	switch e.Kind {
	case LegacyErrBadExchangeWindow:
		return "时刻格式应为 HH:mm:ss，收到「" + e.Window + "」"
	case LegacyErrNoExchangeWindow:
		return "至少需要一个抢兑时刻"
	case LegacyErrBadPrizeID:
		return "奖品 ID 无效：" + e.PrizeID
	case LegacyErrMaskCollision:
		return "账号掩码冲突：" + e.AccountID + " 同时来自 " + fmt.Sprintf("%q", e.Keys) +
			"（两个手机号的前 2 位与后 2 位相同）。继续迁移会把两个账号的数据混在一起，" +
			"因此中止；请为其中一个账号显式指定不同的 id"
	}
	return "未知的迁移错误"
}

// LegacyErrorIs 报告 err 是否是指定类别的 [LegacyError]。
//
// ⚠ 走 [errors.As] 而不是裸类型断言：Go 的惯例是用 `fmt.Errorf("…: %w", err)`
// 包一层，裸断言在那种情况下**静默返回 false**（§6.1 #6）。
func LegacyErrorIs(err error, kind LegacyErrorKind) bool {
	var le *LegacyError
	if !errors.As(err, &le) {
		return false
	}
	return le.Kind == kind
}

// ExchangeWindowToCron 复刻 `windowToCron`：`HH:mm:ss` → 6 段 cron（含秒）。
//
// 注意 `Number()` 会**去掉前导零**：`"09:59:40"` → `"40 59 9 * * *"`。
// 这不是笔误 —— croner 与本仓的 cron 解析器都接受无前导零的字段。
func ExchangeWindowToCron(window string) (string, error) {
	parts := strings.Split(strings.TrimSpace(window), ":")
	bad := func() (string, error) { return "", &LegacyError{Kind: LegacyErrBadExchangeWindow, Window: window} }
	if len(parts) != 3 {
		return bad()
	}
	// 小时 00-23，分秒 00-59，且都必须写满两位
	ok2 := func(s string, max int) (int, bool) {
		if len(s) != 2 {
			return 0, false
		}
		for i := 0; i < len(s); i++ {
			if s[i] < '0' || s[i] > '9' {
				return 0, false
			}
		}
		n, err := strconv.Atoi(s)
		if err != nil || n > max {
			return 0, false
		}
		return n, true
	}
	h, okH := ok2(parts[0], 23)
	m, okM := ok2(parts[1], 59)
	s, okS := ok2(parts[2], 59)
	if !okH || !okM || !okS {
		return bad()
	}
	return fmt.Sprintf("%d %d %d * * *", s, m, h), nil
}

// NormalizeExchangeWindows 复刻 `normalizeWindows`：
// `trim` → 5 位补 `:00` → 逐个校验 → **去重并排序**。
//
// 排序是字符串序，对定长 `HH:mm:ss` 等价于时间序。
func NormalizeExchangeWindows(raw []string) ([]string, error) {
	cleaned := make([]string, 0, len(raw))
	for _, w := range raw {
		w = strings.TrimSpace(w)
		if w == "" {
			continue
		}
		if len(w) == 5 {
			w += ":00"
		}
		cleaned = append(cleaned, w)
	}
	for _, w := range cleaned {
		if _, err := ExchangeWindowToCron(w); err != nil {
			return nil, err
		}
	}
	if len(cleaned) == 0 {
		return nil, &LegacyError{Kind: LegacyErrNoExchangeWindow}
	}
	sort.Strings(cleaned)
	out := cleaned[:0]
	for i, w := range cleaned {
		if i == 0 || w != cleaned[i-1] {
			out = append(out, w)
		}
	}
	return out, nil
}

// NormalizePipelineAccounts 复刻 `normalizeAccounts`（组合的账号选择）。
//
// 规则与怪癖：
//   - 只有**真数组**才算数：`accounts` 不是数组时被当作"没给"，
//     转而去看废弃的单值字段；
//   - 单值 `account` → `[account]`；
//   - 剔除 `""` / `null`（JS 会连 `undefined` 一起剔，但 `undefined` 在 JSON 里不存在）；
//   - 去重，保持首次出现顺序；
//   - **结果为空一律返回 nil，语义是"全部账号"**（而不是"没有账号"）。
//
// ⚠ 两个参数的"没给"必须用**指针/切片 nil**区分，不能用零值：
// `accounts == nil`（没给）与 `accounts` 是空切片（**给了空数组**）是两种输入，
// 而"给了空数组"会**盖掉**废弃字段（旧版先判 `Array.isArray`）。
// 用 `[]any{}` vs `nil` 表达前者、`*any` 的 nil 表达后者。
func NormalizePipelineAccounts(accounts []any, legacyAccount *any) []any {
	var raw []any
	if accounts != nil {
		raw = accounts
	} else if legacyAccount != nil {
		raw = []any{*legacyAccount}
	}

	out := make([]any, 0, len(raw))
	for _, value := range raw {
		drop := false
		switch v := value.(type) {
		case nil:
			drop = true
		case string:
			drop = v == ""
		}
		if !drop && !containsJSONValue(out, value) {
			out = append(out, value)
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// NormalizeWebui 复刻 `normalize`（界面偏好）。
//
// 两个布尔的默认方向**刻意相反**，这是旧版的真实行为：
//   - `remember_last_page`：`raw !== false` → **缺省就是 `true`**；
//   - `log_expanded` / `background_run`：`raw === true` → **缺省就是 `false`**。
//
// `last_page` 不在 [WebuiPages] 白名单内则回落到 [DefaultWebuiPage]。
//
// 返回的是**新领域类型** [WebuiConfig]，不另设一个"规整后"的孪生类型 ——
// 两个只差字段类型的结构体，迟早会有一边忘记同步。
func NormalizeWebui(raw *LegacyWebuiFile) WebuiConfig {
	page := DefaultWebuiPage
	if raw.LastPage != nil && containsString(WebuiPages, *raw.LastPage) {
		page = *raw.LastPage
	}
	return WebuiConfig{
		RememberLastPage: derefBoolOrTrue(raw.RememberLastPage),
		LastPage:         page,
		LogExpanded:      derefBoolStrict(raw.LogExpanded),
		BackgroundRun:    derefBoolStrict(raw.BackgroundRun),
		// 只用于展示，解析不出来的直接丢掉，不为一条时间戳保留原文
		UpdatedAt: ParseRFC3339(derefOrEmpty(raw.UpdatedAt)),
	}
}

// ParseRFC3339 宽松解析 RFC 3339 时间戳：解不出返回 nil。
//
// 旧文件里的时间戳是 `toISOString()` 产物（如 `2026-09-16T12:03:33.972Z`），
// 但**手改过的文件里什么都可能有** —— 而迁移不该因为一个时间戳格式不对就失败。
func ParseRFC3339(value string) *time.Time {
	if value == "" {
		return nil
	}
	t, err := time.Parse(time.RFC3339, value)
	if err != nil {
		return nil
	}
	utc := t.UTC()
	return &utc
}

// CapHistory 复刻 `appendHistory` 的截断：只保留**最后** `cap` 条。
func CapHistory[T any](runs *[]T, cap int) {
	if len(*runs) > cap {
		cut := len(*runs) - cap
		*runs = append((*runs)[:0:0], (*runs)[cut:]...)
	}
}

// ParseBad 是一条解析失败的条目。
type ParseBad struct {
	Index int
	Msg   string
}

// ParseEach 逐条解析，坏掉的条目只被记录下来而不会拖垮整份文件。
//
// 迁移的底线是"能救的都要救"：旧文件里一条被手改坏的记录，
// 不该导致另外 95 条历史全部丢失。
func ParseEach[T any](items []any) ([]T, []ParseBad) {
	ok := make([]T, 0, len(items))
	var bad []ParseBad
	for index, item := range items {
		chunk, err := marshalJSON(item)
		if err != nil {
			bad = append(bad, ParseBad{Index: index, Msg: err.Error()})
			continue
		}
		var value T
		if err := decodeJSONStrict(chunk, &value); err != nil {
			bad = append(bad, ParseBad{Index: index, Msg: err.Error()})
			continue
		}
		ok = append(ok, value)
	}
	return ok, bad
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. 账号视图与"旧键 → 新标识"映射
// ─────────────────────────────────────────────────────────────────────────────

// LegacyAccountView 是一个旧账号被解析后的全部派生信息。
//
// 迁移期间要反复用到（旧 `state.json` 的键、历史里的展示名、组合/定时任务里的
// 账号引用），而每项都要解析一次 `auth` —— 所以解析一次、到处复用。
//
// ⚠ Rust 侧只 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize ——
// 它是中间结果，不是落库/出响应的形状。
type LegacyAccountView struct {
	// Index 从 1 开始的序号，对应旧 `findAccount` 的序号语义。
	Index    int
	Nickname *string
	// Phone 是 [ResolvePhone] 的结果，可能为空串（既没配 `phone` 也解不出）。
	Phone string
	// Label 是展示名。**这就是 `history.json` 里 `runs[].account` 存的东西。**
	Label string
	// LegacyKey 是旧 `state.json` 的键（手机号原文，否则昵称，否则 `"account"`）。
	LegacyKey string
	// AccountID 是新标识：手机号走掩码，避免把完整号码当主键四处引用。
	AccountID AccountID
	// AuthExpire 是凭据到期时间（**epoch 毫秒**，`0` = 无过期信息）。
	//
	// 单位是毫秒而非秒：旧版拿它和 `Date.now()` 直接相减。见 auth.go 包注释。
	AuthExpire int64
	// Platform 是平台号（`auth` 的第 1 段，如 `"2"`）；解不出为 nil。
	Platform *string
	// ConfigPhone 是旧配置里显式写出的 `phone`（与从 `auth` 解出来的是两回事）。
	ConfigPhone *string
}

// LegacyAccountViewFromConfig 从一条旧配置解析出账号视图。
func LegacyAccountViewFromConfig(index int, config LegacyAccountConfig) LegacyAccountView {
	nickname := config.NicknameOrNone()
	configPhone := config.Phone
	if configPhone != nil && *configPhone == "" {
		configPhone = nil
	}
	phone := ResolvePhone(configPhone, config.Auth)

	authExpire := int64(0)
	var platform *string
	if info, ok := ParseAuth(config.Auth); ok {
		authExpire = info.ExpireMillis()
		platform = info.Platform
	}

	return LegacyAccountView{
		Index:       index,
		Nickname:    nickname,
		Label:       AccountLabel(nickname, phone),
		LegacyKey:   LegacyAccountKey(phone, nickname),
		AccountID:   AccountIDFor(phone, nickname),
		AuthExpire:  authExpire,
		Platform:    platform,
		ConfigPhone: configPhone,
		Phone:       phone,
	}
}

// MatchesName 报告昵称与手机号是否都能匹配到这个字符串（用于识别"展示名/手机号"引用）。
func (v LegacyAccountView) MatchesName(who string) bool {
	return (v.Nickname != nil && *v.Nickname == who) || (v.Phone != "" && v.Phone == who)
}

// DescribeLegacyAccounts 把整份旧配置解析成账号视图（序号从 1 开始，与旧版一致）。
func DescribeLegacyAccounts(configs []LegacyAccountConfig) []LegacyAccountView {
	out := make([]LegacyAccountView, 0, len(configs))
	for i, cfg := range configs {
		out = append(out, LegacyAccountViewFromConfig(i+1, cfg))
	}
	return out
}

// BuildIdentityMap 建立「旧键（`state.json` 的键）→ 新 [AccountID]」的映射。
//
// **这是历史数据能对上号的唯一根据**，所以撞车必须硬失败：两个手机号的掩码相同
// （前 2 后 2 相同）时，两个账号会塌缩成同一个 [AccountID]，
// 迁移的结果是一份"云朵余额与凭据混在一起"的数据 —— 而且不会报错。
//
// ⚠ 撞车时报**哪一个**必须确定：Rust 侧靠 `BTreeMap` 的键序（字典序）保证，
// 而 Go 的 map **遍历顺序是随机的**（语言规范刻意如此）。
// 不显式排序的话，报错内容会**每次运行都不同** —— 而它是要进迁移报告给人看的。
func BuildIdentityMap(views []LegacyAccountView) (map[string]AccountID, error) {
	mapOut := make(map[string]AccountID, len(views))
	sources := make(map[string][]string)

	for _, view := range views {
		mapOut[view.LegacyKey] = view.AccountID
		id := string(view.AccountID)
		entry := sources[id]
		if !containsString(entry, view.LegacyKey) {
			sources[id] = append(entry, view.LegacyKey)
		}
	}

	ids := make([]string, 0, len(sources))
	for id := range sources {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		if keys := sources[id]; len(keys) > 1 {
			return nil, &LegacyError{Kind: LegacyErrMaskCollision, AccountID: id, Keys: keys}
		}
	}
	return mapOut, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. 账号引用解析（旧语义 vs 迁移建议）
// ─────────────────────────────────────────────────────────────────────────────

// LegacyTargetKind 是旧版 `findAccount` 的四类解析结果。
type LegacyTargetKind int

const (
	// LegacyTargetAll 表示未指定 → 全部账号。
	LegacyTargetAll LegacyTargetKind = iota
	// LegacyTargetIndex 表示命中第 n 个账号（1 起）。
	LegacyTargetIndex
	// LegacyTargetNamed 表示按昵称或手机号命中第 n 个账号。
	LegacyTargetNamed
	// LegacyTargetUnresolved 表示解析不到。旧版此时记 `账号 x 不存在`，
	// **并且一个账号都不跑**。
	LegacyTargetUnresolved
)

// LegacyTarget 是**旧版** `findAccount` 的解析结果 —— 只描述旧版会怎么做。
//
// ⚠ Rust 侧只 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize。
type LegacyTarget struct {
	Kind LegacyTargetKind
	// Index 是 [LegacyTargetIndex] / [LegacyTargetNamed] 的序号（1 起）。
	Index int
	// Raw 是 [LegacyTargetUnresolved] 的原文。
	Raw string
}

// ResolveLegacyTarget 复刻旧 `findAccount` 的查找顺序。
//
// **`/^\d+$/` 分支在昵称匹配之前**，所以纯数字字符串一律按序号解释，
// 手机号字符串永远匹配不到。这条怪癖原样保留 —— 迁移要识别出
// "过去从未生效的引用"，而不是假装它一直有效。
func ResolveLegacyTarget(views []LegacyAccountView, reference *any) LegacyTarget {
	if reference == nil {
		return LegacyTarget{Kind: LegacyTargetAll}
	}

	lookupIndex := func(n float64) (int, bool) {
		if n < 1.0 || n-math.Trunc(n) != 0 {
			return 0, false
		}
		// ⚠ NaN / Inf 必须**显式**挡：Rust 的 `n as usize` 是 saturating 转换
		// （NaN→0、Inf→usize::MAX），靠它把这两种输入挡在 `idx <= len` 之外；
		// 而 Go 的浮点转整型**超出范围时是实现相关的**（语言规范不定义），
		// 不挡就会拿到一个碰巧合法的数。
		if math.IsNaN(n) || math.IsInf(n, 0) {
			return 0, false
		}
		idx := int(n)
		return idx, idx <= len(views)
	}

	switch value := (*reference).(type) {
	case json.Number:
		n, err := value.Float64()
		if err == nil {
			if idx, ok := lookupIndex(n); ok {
				return LegacyTarget{Kind: LegacyTargetIndex, Index: idx}
			}
		}
		return LegacyTarget{Kind: LegacyTargetUnresolved, Raw: value.String()}
	case string:
		if value != "" && isAllASCIIDigits(value) {
			// 纯数字串 = 序号（**不是手机号**）
			var n float64
			n, err := strconv.ParseFloat(value, 64)
			if err == nil {
				if idx, ok := lookupIndex(n); ok {
					return LegacyTarget{Kind: LegacyTargetIndex, Index: idx}
				}
			}
			return LegacyTarget{Kind: LegacyTargetUnresolved, Raw: value}
		}
		for _, v := range views {
			if v.MatchesName(value) {
				return LegacyTarget{Kind: LegacyTargetNamed, Index: v.Index}
			}
		}
		return LegacyTarget{Kind: LegacyTargetUnresolved, Raw: value}
	default:
		return LegacyTarget{Kind: LegacyTargetUnresolved, Raw: renderJSONValue(*reference)}
	}
}

// ReferenceAdviceKind 是迁移诊断的四类。
type ReferenceAdviceKind int

const (
	// AdviceAllAccounts 表示未指定 → 全部账号，照搬。
	AdviceAllAccounts ReferenceAdviceKind = iota
	// AdviceSame 表示旧版能解析，照搬到同一个账号。
	AdviceSame
	// AdviceRecoveredFromPhone 表示旧版把它当序号且**越界**（于是从未生效），
	// 但按手机号能唯一匹配到一个账号。
	//
	// 建议改指到该账号；这是**行为变更**，必须让用户确认。
	AdviceRecoveredFromPhone
	// AdviceWasNeverEffective 表示旧版解析不到，按新规则也匹配不到
	// → 必须人工确认，不要猜。
	AdviceWasNeverEffective
)

// ReferenceAdvice 是迁移诊断：这条旧引用**新版**该怎么解释，
// 以及它过去是否真的生效过。
//
// 与 [LegacyTarget] 分开，是因为两者回答的是不同的问题：
// 前者是"旧版怎么理解它"，后者是"该不该照搬"。
//
// ⚠ Rust 侧只 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize。
type ReferenceAdvice struct {
	Kind ReferenceAdviceKind
	// Index 是 [AdviceSame] / [AdviceRecoveredFromPhone] 的序号（1 起）。
	Index int
	// Digits 是 [AdviceRecoveredFromPhone] 的原始数字串。
	Digits string
	// Raw 是 [AdviceWasNeverEffective] 的原文。
	Raw string
}

// AdviseReference 给出一条旧引用的迁移建议。
func AdviseReference(views []LegacyAccountView, reference *any) ReferenceAdvice {
	switch target := ResolveLegacyTarget(views, reference); target.Kind {
	case LegacyTargetAll:
		return ReferenceAdvice{Kind: AdviceAllAccounts}
	case LegacyTargetIndex, LegacyTargetNamed:
		return ReferenceAdvice{Kind: AdviceSame, Index: target.Index}
	default:
		// 越界的纯数字串：旧版当成序号 → 落空；但它很可能本来就是手机号。
		var recovered *LegacyAccountView
		for i := range views {
			if views[i].Phone != "" && views[i].Phone == target.Raw {
				recovered = &views[i]
				break
			}
		}
		if recovered == nil {
			// 手机号可能写在配置的 phone 字段里（与 auth 解出的不一定相同）
			for i := range views {
				if views[i].ConfigPhone != nil && *views[i].ConfigPhone == target.Raw {
					recovered = &views[i]
					break
				}
			}
		}
		if recovered == nil {
			return ReferenceAdvice{Kind: AdviceWasNeverEffective, Raw: target.Raw}
		}
		return ReferenceAdvice{Kind: AdviceRecoveredFromPhone, Digits: target.Raw, Index: recovered.Index}
	}
}

// ResolveReferenceStrict 把旧引用解析成新 [AccountID]
// （hasID=false 语义为"全部账号"，由调用方决定怎么展开）。
//
// 只走 [ReferenceAdvice] 里**明确**的分支：拿不准的一律返回 bad，
// 迫使调用方把问题摆到迁移报告里，而不是替用户做决定。
func ResolveReferenceStrict(views []LegacyAccountView, reference *any) (id AccountID, hasID bool, bad *ReferenceAdvice) {
	advice := AdviseReference(views, reference)
	switch advice.Kind {
	case AdviceAllAccounts:
		return "", false, nil
	case AdviceSame, AdviceRecoveredFromPhone:
		if advice.Index >= 1 && advice.Index <= len(views) {
			return views[advice.Index-1].AccountID, true, nil
		}
		return "", false, nil
	default:
		return "", false, &advice
	}
}

// FindByLabel 按**展示名**找账号 —— 迁移 `history.json` 的 `runs[].account` 用。
//
// 展示名可能撞车（两个账号同名，或两个账号的掩码手机号相同），
// 所以返回全部候选，由调用方判断是否为 1 个。
func FindByLabel(views []LegacyAccountView, label string) []LegacyAccountView {
	var out []LegacyAccountView
	for _, v := range views {
		if v.Label == label {
			out = append(out, v)
		}
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

// decodeJSONStrict 把一段 JSON 解到具体类型，与 [parseJSON] 同源：
// `UseNumber()`（不丢 19 位标识的精度）+ 尾随内容判失败。
//
// ⚠ 不能直接用 `json.Unmarshal`：它会把数字解成 float64，
// 而这一层的 `userDomainId` / `prizeId` 是 19 位十进制（§6.1 #2）。
func decodeJSONStrict(data []byte, v any) error {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if err := dec.Decode(v); err != nil {
		return err
	}
	if _, err := dec.Token(); err != io.EOF {
		return fmt.Errorf("JSON 之后还有多余内容")
	}
	return nil
}

// containsJSONValue 判断一组 JSON 值里是否已有语义相等的那个。
//
// ⚠ 用 DeepEqual 而不是 `==`：JSON 值里可能有 slice / map，那些不能用 `==` 比。
//
// ⚠ 已知差异：数字按 `json.Number` 的**字面量**比，所以 `1` 与 `1.0` 不相等，
// 而 serde 的 `Value::Number` 是**数值**相等。这批输入里没有同值异形的数字，
// 差异不影响结果，但如果要放宽就得显式转 float64 再比 —— 那会丢精度。
func containsJSONValue(list []any, want any) bool {
	for _, item := range list {
		if reflect.DeepEqual(item, want) {
			return true
		}
	}
	return false
}

// renderJSONValue 把一个 JSON 值渲染成文本，对齐 Rust 的 `Value::to_string()`。
func renderJSONValue(v any) string {
	raw, err := marshalJSON(v)
	if err != nil {
		return fmt.Sprintf("%v", v)
	}
	return string(raw)
}

func derefOrEmpty(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

// derefBoolOrTrue 对齐 `raw !== false`：nil（缺席）也算 true。
func derefBoolOrTrue(p *bool) bool {
	return p == nil || *p
}

// derefBoolStrict 对齐 `raw === true`：只有显式 true 才算 true。
func derefBoolStrict(p *bool) bool {
	return p != nil && *p
}
